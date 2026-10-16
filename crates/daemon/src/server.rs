//! 命名管道服务端：单例守卫 + token 握手 + ping/pong（issue #64）。
//!
//! 单例：`FILE_FLAG_FIRST_PIPE_INSTANCE` 让**第二个** `CreateNamedPipeW` 直接失败——内核级守卫，
//! 不需要锁文件或 `Mutex`。**但「名字被占」的错误码各家实现口径不一**（CI 实测是
//! `ERROR_PIPE_BUSY`，文档却写 `ERROR_ACCESS_DENIED`），所以不认错误码，改**探测**：建不上管道
//! 就去问一句「这条管道上有人应答吗」。是则「已在运行」，否则如实报错。
//!
//! 管道 DACL 只放当前用户（[`crate::acl`]），所以「非当前用户被拒绝」由内核完成，不靠应用层判断。

use std::fs::File;
use std::io::{BufReader, ErrorKind};
use std::mem::ManuallyDrop;
use std::os::windows::io::FromRawHandle;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use ageminal_protocol::{ClientMessage, ServerMessage, PROTOCOL_VERSION};
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_NO_DATA, ERROR_PIPE_CONNECTED, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
};

use crate::{acl, launch, logging, paths, win, wire, DaemonError, DaemonInfo};

/// 管道的内核缓冲（握手流量很小；这不是我们自己的读缓冲）。
const BUFFER_BYTES: u32 = 8 * 1024;

/// 建管道失败后，给「上一个 daemon 已建好管道、状态文件还没落盘」这个窗口留的宽限时间。
const COLLISION_GRACE: Duration = Duration::from_secs(2);

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

    let info = DaemonInfo {
        pid: std::process::id(),
        pipe: config.pipe.clone(),
        token: win::random_token()?,
        protocol: PROTOCOL_VERSION,
        version: env!("CARGO_PKG_VERSION").to_owned(),
        started_at_ms: logging::now_ms(),
    };
    // 状态文件必须在「开始 accept」之前落盘：客户端靠它拿管道名与令牌。
    info.write(&config.config_dir)?;
    logging::append(
        &log_path,
        &format!("启动：pid={} pipe={}", info.pid, info.pipe),
    );

    let result = accept_loop(&pipe, &info, &log_path);

    let _ = DaemonInfo::remove(&config.config_dir);
    logging::append(
        &log_path,
        &format!("停止：{}", if result.is_ok() { "正常" } else { "异常" }),
    );
    result
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
fn accept_loop(pipe: &PipeHandle, info: &DaemonInfo, log_path: &Path) -> Result<(), DaemonError> {
    loop {
        wait_for_client(pipe.0, log_path)?;
        if let Err(error) = serve_connection(pipe.0, info, log_path) {
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

/// 招呼一个连接：握手 → 进入 ping/pong 循环，直到对端关闭。
fn serve_connection(handle: HANDLE, info: &DaemonInfo, log_path: &Path) -> Result<(), DaemonError> {
    // **只借用**句柄：`ManuallyDrop` 防止 `File` 析构时把它关掉——这个管道实例还要复用。
    let owned = ManuallyDrop::new(unsafe { File::from_raw_handle(handle) });
    let stream: &File = &owned;
    let mut reader = BufReader::new(stream);
    let mut writer = stream;

    let hello: ClientMessage = wire::read_message(&mut reader)?;
    let ClientMessage::Hello { token, protocol } = hello else {
        return Err(DaemonError::Unexpected(
            "连接后的第一条消息必须是 hello".to_owned(),
        ));
    };

    if let Some(reason) = handshake_rejection(info, &token, protocol) {
        logging::append(log_path, &format!("握手被拒：{reason}"));
        reject(&mut writer, &reason)?;
        return wait_for_close(&mut reader);
    }

    wire::write_message(
        &mut writer,
        &ServerMessage::Welcome {
            protocol: PROTOCOL_VERSION,
            pid: info.pid,
            version: info.version.clone(),
        },
    )?;
    logging::append(log_path, "握手通过");

    loop {
        match wire::read_message::<_, ClientMessage>(&mut reader) {
            Ok(ClientMessage::Ping { nonce }) => {
                wire::write_message(&mut writer, &ServerMessage::Pong { nonce })?;
            }
            Ok(ClientMessage::Hello { .. }) => {
                return Err(DaemonError::Unexpected("重复的 hello".to_owned()));
            }
            // 对端正常关闭。
            Err(error) if error.kind() == ErrorKind::UnexpectedEof => return Ok(()),
            Err(error) => return Err(DaemonError::Io(error)),
        }
    }
}

/// 握手校验：通过返回 `None`，否则返回**拒绝理由**。
fn handshake_rejection(info: &DaemonInfo, token: &str, protocol: u32) -> Option<String> {
    if token != info.token {
        return Some("令牌不匹配".to_owned());
    }
    if protocol != PROTOCOL_VERSION {
        return Some(format!(
            "协议版本不一致：daemon {PROTOCOL_VERSION} / 客户端 {protocol}"
        ));
    }
    None
}

/// 发拒绝消息。
fn reject(writer: &mut &File, reason: &str) -> Result<(), DaemonError> {
    wire::write_message(
        writer,
        &ServerMessage::Rejected {
            reason: reason.to_owned(),
        },
    )
    .map_err(DaemonError::from)
}

/// 等对端把拒绝消息读完再断开——`DisconnectNamedPipe` 会**丢弃未读数据**，立刻断开会让客户端
/// 读不到「为什么被拒」。
///
/// 读到 EOF 为止，期间收到的消息一律丢弃。**没有超时**：只有当前用户连得上（管道 DACL），所以
/// 挂起风险可接受；真要紧缩得换 overlapped IO 加超时。
fn wait_for_close(reader: &mut BufReader<&File>) -> Result<(), DaemonError> {
    loop {
        if wire::read_message::<_, ClientMessage>(reader).is_err() {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_KERNEL_OBJECT};
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACL, DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ALL_ACCESS, OPEN_EXISTING, READ_CONTROL,
    };

    use super::*;

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

    #[test]
    fn handshake_checks_token_then_protocol() {
        let info = DaemonInfo {
            pid: 1,
            pipe: r"\\.\pipe\x".to_owned(),
            token: "good".to_owned(),
            protocol: PROTOCOL_VERSION,
            version: "0.0.0".to_owned(),
            started_at_ms: 0,
        };
        assert_eq!(handshake_rejection(&info, "good", PROTOCOL_VERSION), None);
        assert_eq!(
            handshake_rejection(&info, "bad", PROTOCOL_VERSION).as_deref(),
            Some("令牌不匹配")
        );
        assert!(handshake_rejection(&info, "good", PROTOCOL_VERSION + 1)
            .unwrap()
            .contains("协议版本不一致"));
    }
}
