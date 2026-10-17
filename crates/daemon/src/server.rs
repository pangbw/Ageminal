//! 命名管道服务端：单例守卫 + 握手 + ping/pong（issue #64 / #65）。
//!
//! 单例：`FILE_FLAG_FIRST_PIPE_INSTANCE` 让**第二个** `CreateNamedPipeW` 直接失败——内核级守卫，
//! 不需要锁文件或 `Mutex`。**但「名字被占」的错误码各家实现口径不一**（CI 实测是
//! `ERROR_PIPE_BUSY`，文档却写 `ERROR_ACCESS_DENIED`），所以不认错误码，改**探测**：建不上管道
//! 就去问一句「这条管道上有人应答吗」。是则「已在运行」，否则如实报错。
//!
//! 管道 DACL 只放当前用户（[`crate::acl`]），所以「非当前用户被拒绝」由内核完成，不靠应用层判断。

use std::collections::HashMap;
use std::fs::File;
use std::mem::ManuallyDrop;
use std::os::windows::io::FromRawHandle;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ageminal_protocol::{
    negotiate, Caps, ErrorCode, Hello, Message, Negotiation, ProtocolError, Welcome,
    PROTOCOL_VERSION, PROTOCOL_VERSION_MIN,
};
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_NO_DATA, ERROR_PIPE_CONNECTED, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PeekNamedPipe, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
};

use crate::outbound::Outbound;
use crate::pty::PtySize;
use crate::session::SessionTable;
use crate::{acl, framed, launch, logging, paths, win, DaemonError, DaemonInfo};

/// 管道的内核缓冲（控制流量很小；这不是我们自己的读缓冲）。
const BUFFER_BYTES: u32 = 8 * 1024;

/// 建管道失败后，给「上一个 daemon 已建好管道、状态文件还没落盘」这个窗口留的宽限时间。
const COLLISION_GRACE: Duration = Duration::from_secs(2);

/// 等对端数据时的轮询间隔（见 [`wait_for_incoming`]）。
const READ_POLL: Duration = Duration::from_millis(2);

/// 服务端配置。
pub struct ServerConfig {
    /// 命名管道完整路径（`\\.\pipe\…`）。
    pub pipe: String,
    /// 配置根：状态文件与日志落在这里。
    pub config_dir: PathBuf,
}

/// daemon 的进程入口：解析 `--pipe` / `--config-dir`，缺省走按用户派生的落点。
///
/// **已有一个 daemon 在跑**属正常结果（调用方复用它即可），因此返回成功码。
pub fn run(args: &[String]) -> ExitCode {
    let Ok((pipe, config_dir)) = resolve(args) else {
        // 连落点都拿不到，无处可记。
        return ExitCode::FAILURE;
    };
    let config = ServerConfig { pipe, config_dir };

    match serve(&config) {
        Ok(()) => ExitCode::SUCCESS,
        Err(DaemonError::AlreadyRunning { .. }) => ExitCode::SUCCESS,
        Err(error) => {
            logging::append(
                &paths::log_file_in(&config.config_dir),
                &format!("daemon 退出：{error}"),
            );
            ExitCode::FAILURE
        }
    }
}

/// 解析 argv 得到 `(管道名, 配置根)`。
fn resolve(args: &[String]) -> Result<(String, PathBuf), DaemonError> {
    let config_dir = match arg_value(args, launch::ARG_CONFIG_DIR) {
        Some(value) => PathBuf::from(value),
        None => paths::config_dir()?,
    };
    let pipe = match arg_value(args, launch::ARG_PIPE) {
        Some(value) => value,
        None => paths::pipe_name()?,
    };
    Ok((pipe, config_dir))
}

/// 取 `--flag value` 里的 `value`。
fn arg_value(args: &[String], flag: &str) -> Option<String> {
    let index = args.iter().position(|arg| arg == flag)?;
    args.get(index + 1).cloned()
}

/// 起服务并**一直**跑到管道不可用为止。返回即代表 daemon 要退出了。
pub fn serve(config: &ServerConfig) -> Result<(), DaemonError> {
    let log_path = paths::log_file_in(&config.config_dir);

    // 先建管道：这一步同时完成单例判定（第二个实例会失败）。
    let security = acl::CurrentUserSecurity::build()?;
    let pipe = match create_pipe(&config.pipe, &security) {
        Ok(pipe) => pipe,
        Err(DaemonError::Pipe { pipe, reason }) => {
            // 管道名已被占用时 **Windows 给的错误码并不稳定**（CI 实测 `ERROR_PIPE_BUSY`，
            // 文档写 `ERROR_ACCESS_DENIED`）。所以不猜错误码，直接**探测**：真有 daemon 在线，
            // 就是「已在运行」（复用即可）；否则如实报错，别把真正的失败伪装成单例。
            // 宽限窗口覆盖「对方刚建好管道、状态文件还没落盘」的竞态。
            if launch::poll_running(&config.config_dir, COLLISION_GRACE).is_some() {
                return Err(DaemonError::AlreadyRunning { pipe });
            }
            return Err(DaemonError::Pipe { pipe, reason });
        }
        Err(other) => return Err(other),
    };

    // 脱离 stdout 的进程里，panic 默认写到 NUL 里——完全不可见。装个钩子把它落进 daemon.log。
    install_panic_hook(&log_path);

    let info = DaemonInfo {
        pid: std::process::id(),
        pipe: config.pipe.clone(),
        token: win::random_token()?,
        protocol: PROTOCOL_VERSION,
        daemon_version: env!("CARGO_PKG_VERSION").to_owned(),
        started_at_ms: logging::now_ms(),
    };
    // 状态文件必须在「开始 accept」之前落盘：客户端靠它拿管道名与令牌。
    info.write(&config.config_dir)?;
    logging::append(
        &log_path,
        &format!("启动：pid={} pipe={}", info.pid, info.pipe),
    );

    // 会话表是 **daemon 全局**的：会话要活过连接（live reattach，REQUIREMENTS.md §5.2）。
    let sessions = Arc::new(SessionTable::new());
    let result = accept_loop(&pipe, &info, &sessions, &log_path);

    let _ = DaemonInfo::remove(&config.config_dir);
    logging::append(
        &log_path,
        &format!("停止：{}", if result.is_ok() { "正常" } else { "异常" }),
    );
    result
}

/// 把所有线程的 panic 记进 daemon 日志。
///
/// daemon 的 stdio 三句柄全是 NUL（§5.1 的硬性要求），所以不装这个钩子的话，任何线程 panic 都是
/// **完全静默**的——排查起来连线索都没有。
fn install_panic_hook(log_path: &Path) {
    let log_path = log_path.to_path_buf();
    std::panic::set_hook(Box::new(move |info| {
        logging::append(&log_path, &format!("panic：{info}"));
    }));
}

/// 拥有管道 HANDLE；析构即 `CloseHandle`。
struct PipeHandle(HANDLE);

impl Drop for PipeHandle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

/// 创建管道实例（`FILE_FLAG_FIRST_PIPE_INSTANCE`：名字被占就直接失败）。
///
/// 失败一律包成 [`DaemonError::Pipe`]——「是不是别的 daemon 占着」由调用方用**探测**判断，
/// 不靠错误码（见 [`serve`]）。
fn create_pipe(pipe: &str, security: &acl::CurrentUserSecurity) -> Result<PipeHandle, DaemonError> {
    let name = win::to_wide(pipe);
    let handle = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            BUFFER_BYTES,
            BUFFER_BYTES,
            0,
            security.attributes(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        let error = unsafe { GetLastError() };
        return Err(DaemonError::Pipe {
            pipe: pipe.to_owned(),
            reason: format!("GetLastError={error}"),
        });
    }
    Ok(PipeHandle(handle))
}

/// 顺序接受连接。每个连接处理完就 `DisconnectNamedPipe`，把**同一个实例**留给下一个客户端
/// （单例下只有一个实例，REQUIREMENTS.md §5.1 的「单连接」）。
fn accept_loop(
    pipe: &PipeHandle,
    info: &DaemonInfo,
    sessions: &Arc<SessionTable>,
    log_path: &Path,
) -> Result<(), DaemonError> {
    loop {
        wait_for_client(pipe.0, log_path)?;
        if let Err(error) = serve_connection(pipe.0, info, sessions, log_path) {
            logging::append(log_path, &format!("连接结束：{error}"));
        }
        unsafe { DisconnectNamedPipe(pipe.0) };
    }
}

/// 阻塞到有客户端连上（或它在我们调用之前就连上了）。
fn wait_for_client(handle: HANDLE, log_path: &Path) -> Result<(), DaemonError> {
    if unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) } != 0 {
        return Ok(());
    }
    let error = unsafe { GetLastError() };
    // 客户端抢在 `ConnectNamedPipe` 之前连上（ERROR_PIPE_CONNECTED），或连上又立刻走了
    // （ERROR_NO_DATA）——都按「已连接」处理，继续往下招呼这一次连接即可。
    if error == ERROR_PIPE_CONNECTED || error == ERROR_NO_DATA {
        logging::append(
            log_path,
            &format!("连接竞态：GetLastError={error}（按已连接处理）"),
        );
        return Ok(());
    }
    Err(DaemonError::Windows {
        call: "ConnectNamedPipe",
        reason: format!("GetLastError={error}"),
    })
}

/// 招呼一个连接：握手（同步）→ 写线程 + 读循环（直到对端关闭）。
fn serve_connection(
    handle: HANDLE,
    info: &DaemonInfo,
    sessions: &Arc<SessionTable>,
    log_path: &Path,
) -> Result<(), DaemonError> {
    // **只借用**句柄：`ManuallyDrop` 防止 `File` 析构时把它关掉——这个管道实例还要复用。
    let owned = ManuallyDrop::new(unsafe { File::from_raw_handle(handle) });
    let stream: &File = &owned;
    // **不要 `BufReader`**：读之前要先 `PeekNamedPipe` 看管道，而 `BufReader` 会把后续帧吞进
    // 用户态缓冲——管道看着是空的，peek 就永远等下去（#67 的 CI 实测：`<- Ack` 之后再也读不到
    // 下一帧）。`Frame::read_from` 本来就按帧长精确读，不需要额外缓冲。
    let mut reader = stream;
    let mut writer = stream;

    let Some(Message::Hello(hello)) = framed::read_message(&mut reader)? else {
        return Err(DaemonError::Unexpected(
            "连接后的第一条消息必须是 hello".to_owned(),
        ));
    };

    let proto = match check_handshake(info, &hello) {
        Handshake::Accepted(proto) => proto,
        Handshake::Rejected(code, reason) => {
            logging::append(log_path, &format!("握手被拒：{reason}"));
            framed::send(
                &mut writer,
                &Message::Error(ProtocolError {
                    code,
                    message: reason,
                }),
            )?;
            return wait_for_close(&mut reader);
        }
    };

    framed::send(
        &mut writer,
        &Message::Welcome(Welcome {
            proto,
            // 研究稿 §6.1：`Welcome` 回**自己**的能力位；实际生效的是它与客户端能力的交集，
            // 由客户端算（[`Caps::intersect`]）。
            caps: Caps::supported(),
            daemon_version: info.daemon_version.clone(),
        }),
    )?;
    logging::append(log_path, "握手通过");

    // —— 握手之后：写端交给一条**专门的写线程**（多个会话的输出线程都要往里塞），读端留在本线程。
    // 帧边界只有写线程一个人在维护，所以不会写花。
    let outbound = Arc::new(Outbound::new(log_path.to_path_buf()));
    let handles = HandleMap::new();

    // 写线程**借用同一个句柄**，不 `try_clone()`：管道上出现过「原始句柄直写正常、复制句柄的写
    // 调用一直不返回」的现象（#67 的 CI 实测）。`thread::scope` 让线程只活到本连接结束，不用复制。
    let result = std::thread::scope(|scope| {
        let writer_thread = scope.spawn(|| outbound.run(&mut writer));

        let result = read_loop(&mut reader, handle, sessions, &outbound, &handles, log_path);

        outbound.close();
        match writer_thread.join() {
            Ok(Ok(())) => {}
            Ok(Err(error)) => logging::append(log_path, &format!("写出失败：{error}")),
            Err(_) => logging::append(log_path, "写线程 panic"),
        }
        result
    });

    // 连接断了：把这个连接挂着的会话都摘掉，免得它们的输出线程继续往死连接里塞。
    sessions.detach_all(&outbound);
    result
}

/// 读循环：把客户端来的控制消息分发给会话层。
fn read_loop(
    reader: &mut impl std::io::Read,
    handle: HANDLE,
    sessions: &Arc<SessionTable>,
    outbound: &Arc<Outbound>,
    handles: &HandleMap,
    log_path: &Path,
) -> Result<(), DaemonError> {
    loop {
        if !wait_for_incoming(handle) {
            return Ok(());
        }
        match framed::read_message(reader) {
            // 对端在帧边界处正常关闭。
            Ok(None) => return Ok(()),
            Ok(Some(message)) => {
                // 控制面留一条审计痕迹（数据面太吵，不记）：daemon 的 stdio 是 NUL，日志是唯一线索。
                if !message.kind().is_raw_payload() {
                    logging::append(log_path, &format!("<- {:?}", message.kind()));
                }
                dispatch(message, sessions, outbound, handles)?
            }
            Err(error) => return Err(error),
        }
    }
}

/// 等到管道里有数据可读。
///
/// **不能直接阻塞在 `ReadFile` 上**：同步（未开 `FILE_FLAG_OVERLAPPED`）的管道句柄上，读和写会
/// 互相串行——读线程一旦挂起，另一个线程的 `WriteFile` 就写不出去（#67 的 CI 实测：握手能写，
/// 之后写线程的每一帧都卡住，客户端也永远收不到）。
/// 所以这里先 `PeekNamedPipe` 探一下，有数据才进 `ReadFile`——这时读是立即返回的，不会占着句柄。
///
/// 代价是空闲时每 [`READ_POLL`] 醒一次；换真正的 overlapped IO 就没这个轮询，留给 #68。
fn wait_for_incoming(handle: HANDLE) -> bool {
    loop {
        let mut available: u32 = 0;
        let ok = unsafe {
            PeekNamedPipe(
                handle,
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &mut available,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            // 对端断了（ERROR_BROKEN_PIPE / ERROR_NO_DATA）——正常收尾。
            return false;
        }
        if available > 0 {
            return true;
        }
        std::thread::sleep(READ_POLL);
    }
}

/// 一条控制消息 → 会话层动作。
///
/// **业务错误回一条 `Error{badFrame}`，不断连接**（协议里 `Error` 就是干这个的）；只有协议被打破
/// （收到 daemon 不认识的消息）才结束连接。
fn dispatch(
    message: Message,
    sessions: &Arc<SessionTable>,
    outbound: &Arc<Outbound>,
    handles: &HandleMap,
) -> Result<(), DaemonError> {
    match message {
        Message::Ping => outbound.send_control(&Message::Pong),
        Message::CreateSession(request) => {
            let handle = handles.handle_for(&request.sid);
            let result = sessions.create(&request, handle, Arc::clone(outbound));
            reply(outbound, result.map(Message::SessionCreated))
        }
        Message::Attach(attach) => {
            let handle = handles.handle_for(&attach.sid);
            let result =
                sessions.attach(&attach.sid, attach.last_seq, handle, Arc::clone(outbound));
            reply(outbound, result.map(Message::Attached))
        }
        Message::Detach(detach) => {
            // 断开**不是**终止：会话继续活在 daemon 里（§5.2）。
            sessions.detach(&detach.sid);
            Ok(())
        }
        Message::Terminate(terminate) => acknowledge(
            outbound,
            sessions.terminate(&terminate.sid, terminate.force),
        ),
        Message::Resize(resize) => acknowledge(
            outbound,
            sessions.resize(
                &resize.sid,
                PtySize {
                    cols: resize.cols,
                    rows: resize.rows,
                },
            ),
        ),
        Message::Ack(ack) => acknowledge(outbound, sessions.ack(&ack.sid, ack.acked_bytes)),
        Message::Input { handle, bytes, .. } => {
            let Some(sid) = handles.sid_of(handle) else {
                return outbound.send_control(&Message::Error(ProtocolError {
                    code: ErrorCode::BadFrame,
                    message: format!("未知的会话句柄 {handle}"),
                }));
            };
            acknowledge(outbound, sessions.write(&sid, &bytes))
        }
        other => Err(DaemonError::Unexpected(format!(
            "daemon 不认识的消息：{other:?}"
        ))),
    }
}

/// 请求-响应型：成功回响应，失败回 `Error`。
fn reply(
    outbound: &Arc<Outbound>,
    result: Result<Message, DaemonError>,
) -> Result<(), DaemonError> {
    match result {
        Ok(message) => outbound.send_control(&message),
        Err(error) => failure(outbound, error),
    }
}

/// 无响应型（terminate / resize / ack / input）：成功什么都不回，失败回 `Error`。
fn acknowledge(
    outbound: &Arc<Outbound>,
    result: Result<(), DaemonError>,
) -> Result<(), DaemonError> {
    match result {
        Ok(()) => Ok(()),
        Err(error) => failure(outbound, error),
    }
}

fn failure(outbound: &Arc<Outbound>, error: DaemonError) -> Result<(), DaemonError> {
    outbound.send_control(&Message::Error(ProtocolError {
        code: ErrorCode::BadFrame,
        message: error.to_string(),
    }))
}

/// 连接内的 handle ↔ `sid` 映射。
///
/// 帧头只能放 `u32`（#65 定的「连接内句柄」），逻辑 `sid` 是字符串；这个映射就是两者之间的桥。
/// handle 只在**本条连接**内唯一，重连后重新分配（客户端从 `SessionCreated` / `Attached` 拿新的）。
#[derive(Default)]
struct HandleMap {
    inner: Mutex<Handles>,
}

#[derive(Default)]
struct Handles {
    by_handle: HashMap<u32, String>,
    by_sid: HashMap<String, u32>,
    next: u32,
}

impl HandleMap {
    fn new() -> Self {
        Self::default()
    }

    /// 某个 `sid` 的句柄；没有就分配一个（同一会话重复 create / attach 会复用它）。
    fn handle_for(&self, sid: &str) -> u32 {
        let mut handles = self.inner.lock().expect("handles poisoned");
        if let Some(handle) = handles.by_sid.get(sid) {
            return *handle;
        }
        handles.next += 1;
        let handle = handles.next;
        handles.by_sid.insert(sid.to_owned(), handle);
        handles.by_handle.insert(handle, sid.to_owned());
        handle
    }

    /// 句柄 → `sid`。
    fn sid_of(&self, handle: u32) -> Option<String> {
        self.inner
            .lock()
            .expect("handles poisoned")
            .by_handle
            .get(&handle)
            .cloned()
    }
}

/// 握手校验的结果。
enum Handshake {
    /// 通过：带上**协商出来的版本**（交给 `Welcome`，不重复算第二遍）。
    Accepted(u16),
    /// 拒绝：错误码 + 理由（理由会**明确写回**给客户端，不静默断连）。
    Rejected(ErrorCode, String),
}

/// 校验令牌与**版本区间**（纯函数，可单测）。
fn check_handshake(info: &DaemonInfo, hello: &Hello) -> Handshake {
    if hello.token != info.token {
        return Handshake::Rejected(ErrorCode::BadToken, "令牌不匹配".to_owned());
    }
    match negotiate(hello.proto_min, hello.proto_max) {
        Negotiation::Agreed(proto) => Handshake::Accepted(proto),
        Negotiation::NoOverlap => Handshake::Rejected(
            ErrorCode::UnsupportedVersion,
            format!(
                "协议版本无交集：客户端 [{}..{}]，daemon [{}..{}]",
                hello.proto_min, hello.proto_max, PROTOCOL_VERSION_MIN, PROTOCOL_VERSION
            ),
        ),
    }
}

/// 等对端把拒绝消息读完再断开——`DisconnectNamedPipe` 会**丢弃未读数据**，立刻断开会让客户端
/// 读不到「为什么被拒」。
///
/// 读到 EOF 为止，期间收到的消息一律丢弃。**没有超时**：只有当前用户连得上（管道 DACL），所以
/// 挂起风险可接受；真要紧缩得换 overlapped IO 加超时。
fn wait_for_close(reader: &mut impl std::io::Read) -> Result<(), DaemonError> {
    loop {
        match framed::read_message(reader) {
            Ok(Some(_)) => continue,
            Ok(None) | Err(_) => return Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use ageminal_protocol::Resume;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_KERNEL_OBJECT};
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACL, DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ALL_ACCESS, OPEN_EXISTING, READ_CONTROL,
    };

    use super::*;

    fn info() -> DaemonInfo {
        DaemonInfo {
            pid: 1,
            pipe: r"\\.\pipe\x".to_owned(),
            token: "good".to_owned(),
            protocol: PROTOCOL_VERSION,
            daemon_version: "0.0.0".to_owned(),
            started_at_ms: 0,
        }
    }

    fn hello(token: &str, proto_min: u16, proto_max: u16) -> Hello {
        Hello {
            proto_min,
            proto_max,
            caps: Caps::supported(),
            token: token.to_owned(),
            client_instance_id: "ui-1".to_owned(),
            resume: Vec::<Resume>::new(),
        }
    }

    /// 验收 3：版本不匹配有**明确处理**（而不是静默断连）。
    #[test]
    fn handshake_rejects_bad_token_and_unsupported_version() {
        match check_handshake(
            &info(),
            &hello("bad", PROTOCOL_VERSION_MIN, PROTOCOL_VERSION),
        ) {
            Handshake::Rejected(code, reason) => {
                assert_eq!(code, ErrorCode::BadToken);
                assert!(reason.contains("令牌"), "{reason}");
            }
            _ => panic!("令牌不对必须被拒"),
        }

        match check_handshake(&info(), &hello("good", 9, 12)) {
            Handshake::Rejected(code, reason) => {
                assert_eq!(code, ErrorCode::UnsupportedVersion, "无交集要用这个错误码");
                assert!(reason.contains("无交集"), "理由要写清楚：{reason}");
            }
            _ => panic!("版本无交集必须被明确拒绝"),
        }

        assert!(matches!(
            check_handshake(
                &info(),
                &hello("good", PROTOCOL_VERSION_MIN, PROTOCOL_VERSION)
            ),
            Handshake::Accepted(_)
        ));
        // 客户端比我们新但区间覆盖我们 → 取交集，接受。
        assert!(matches!(
            check_handshake(&info(), &hello("good", 0, 99)),
            Handshake::Accepted(_)
        ));
    }

    /// 自动化的那半：**内核实际挂在管道上**的 DACL 只有当前用户一条 ACE。
    ///
    /// `acl` 的单测只证明「我们造的描述符是对的」；这里把最后一环闭合——那个描述符**真的**被挂到
    /// 内核对象上了。（「**别的**用户被拒」仍需第二个 Windows 账户，属人工验收。）
    #[test]
    fn pipe_dacl_allows_only_the_current_user() {
        let pipe = format!(r"\\.\pipe\ageminal-dacl-{}", std::process::id());
        let security = acl::CurrentUserSecurity::build().unwrap();
        let server = create_pipe(&pipe, &security).unwrap();

        // 服务端句柄只请求了 `PIPE_ACCESS_DUPLEX`，读 DACL 需要 `READ_CONTROL`；所以另开一个
        // **客户端**句柄——我们给自己的 DACL 授了 `FILE_ALL_ACCESS`，其中含 `READ_CONTROL`。
        let name = win::to_wide(&pipe);
        let client = unsafe {
            CreateFileW(
                name.as_ptr(),
                READ_CONTROL,
                0,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };
        assert_ne!(
            client,
            INVALID_HANDLE_VALUE,
            "当前用户应能打开这条管道（GetLastError={}）",
            unsafe { GetLastError() }
        );

        unsafe {
            let mut dacl: *mut ACL = std::ptr::null_mut();
            let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
            let status = GetSecurityInfo(
                client,
                SE_KERNEL_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut dacl,
                std::ptr::null_mut(),
                &mut descriptor,
            );
            assert_eq!(status, 0, "GetSecurityInfo 失败：{status}");
            assert!(!dacl.is_null(), "管道必须带 DACL");
            assert_eq!((*dacl).AceCount, 1, "DACL 只该有一条 ACE");

            let ace = &*dacl
                .cast::<u8>()
                .add(std::mem::size_of::<ACL>())
                .cast::<ACCESS_ALLOWED_ACE>();
            assert_eq!(ace.Mask, FILE_ALL_ACCESS);

            let sid = (&ace.SidStart as *const u32).cast::<u8>();
            let length = 8 + usize::from(*sid.add(1)) * 4;
            assert_eq!(
                std::slice::from_raw_parts(sid, length),
                win::current_user_sid().unwrap().as_slice(),
                "ACE 的 SID 应为当前用户"
            );

            LocalFree(descriptor);
            CloseHandle(client);
        }
        drop(server);
    }
}
