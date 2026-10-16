//! 命名管道服务端：单例守卫 + token 握手 + ping/pong（issue #64）。
//!
//! 单例靠 `FILE_FLAG_FIRST_PIPE_INSTANCE`：**第二个** `CreateNamedPipeW` 会以
//! `ERROR_ACCESS_DENIED` 失败——这是内核级的原子守卫，不需要锁文件或 `Mutex`。
//!
//! 管道 DACL 只放当前用户（[`crate::acl`]），所以「非当前用户被拒绝」由内核完成，不靠应用层判断。

use std::fs::File;
use std::io::{BufReader, ErrorKind};
use std::mem::ManuallyDrop;
use std::os::windows::io::FromRawHandle;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use ageminal_protocol::{ClientMessage, ServerMessage, PROTOCOL_VERSION};
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, ERROR_NO_DATA,
    ERROR_PIPE_CONNECTED, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
};

use crate::{acl, launch, logging, paths, win, wire, DaemonError, DaemonInfo};

/// 管道的内核缓冲（握手流量很小；这不是我们自己的读缓冲）。
const BUFFER_BYTES: u32 = 8 * 1024;

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
    let pipe = create_pipe(&config.pipe, &security)?;

    let info = DaemonInfo {
        pid: std::process::id(),
        pipe: config.pipe.clone(),
        token: win::random_token()?,
        protocol: PROTOCOL_VERSION,
        version: env!("CARGO_PKG_VERSION").to_owned(),
        started_at_ms: now_ms(),
    };
    // 状态文件必须在「开始 accept」之前落盘：客户端靠它拿管道名与令牌。
    write_state(&paths::state_file_in(&config.config_dir), &info)?;
    logging::append(
        &log_path,
        &format!("启动：pid={} pipe={}", info.pid, info.pipe),
    );

    let result = accept_loop(&pipe, &info, &log_path);

    let _ = std::fs::remove_file(paths::state_file_in(&config.config_dir));
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

/// 创建管道实例。第二个进程走到这里会拿到 [`DaemonError::AlreadyRunning`]。
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
        // `FILE_FLAG_FIRST_PIPE_INSTANCE`：管道名已被占用时就是 ERROR_ACCESS_DENIED
        // （个别实现口径也提 ERROR_ALREADY_EXISTS，一并当作「已在运行」）。
        if error == ERROR_ACCESS_DENIED || error == ERROR_ALREADY_EXISTS {
            return Err(DaemonError::AlreadyRunning {
                pipe: pipe.to_owned(),
            });
        }
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

    if token != info.token {
        reject(&mut writer, "令牌不匹配")?;
        return wait_for_close(&mut reader);
    }
    if protocol != PROTOCOL_VERSION {
        reject(
            &mut writer,
            &format!("协议版本不一致：daemon {PROTOCOL_VERSION} / 客户端 {protocol}"),
        )?;
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

/// 等对端把拒绝消息读完再断开。
///
/// `DisconnectNamedPipe` 会**丢弃未读数据**——立刻断开会让客户端读不到「为什么被拒」。
fn wait_for_close(reader: &mut BufReader<&File>) -> Result<(), DaemonError> {
    let _ = wire::read_message::<_, ClientMessage>(reader);
    Ok(())
}

/// 原子落盘状态文件：先写 `.tmp` 再改名，读方永远看不到半截 JSON。
fn write_state(path: &Path, info: &DaemonInfo) -> Result<(), DaemonError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| DaemonError::State {
            path: path.to_path_buf(),
            source,
        })?;
    }
    let bytes = serde_json::to_vec_pretty(info).map_err(|error| DaemonError::Codec {
        reason: error.to_string(),
    })?;
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, bytes).map_err(|source| DaemonError::State {
        path: temporary.clone(),
        source,
    })?;
    std::fs::rename(&temporary, path).map_err(|source| DaemonError::State {
        path: path.to_path_buf(),
        source,
    })
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}
