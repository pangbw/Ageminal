//! 命名管道客户端：连接 + 握手 + ping（issue #64 / #65）。
//!
//! 这是 app-core 侧唯一的对话入口；agent hook shim（`ageminal-notify`）将来也走同一套。
//! **只经 `ageminal-protocol` 的类型收发**（#65 验收 4），不自己碰字节。

use std::fs::File;
use std::io::BufReader;
use std::os::windows::io::FromRawHandle;
use std::time::{Duration, Instant};

use ageminal_protocol::{
    Caps, Hello, Message, ProtocolError, PROTOCOL_VERSION, PROTOCOL_VERSION_MIN,
};
use windows_sys::Win32::Foundation::{
    GetLastError, ERROR_PIPE_BUSY, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, OPEN_EXISTING};
use windows_sys::Win32::System::Pipes::WaitNamedPipeW;

use crate::{framed, win, DaemonError, DaemonInfo};

/// 管道忙时的单次重试等待（`WaitNamedPipeW` 的毫秒数）。
const RETRY_WAIT_MS: u32 = 250;

/// 与 daemon 的一条**已握手**连接。
#[derive(Debug)]
pub struct Client {
    reader: BufReader<File>,
    writer: File,
}

impl Client {
    /// 存活探测：发 `ping`，等 `pong`。
    pub fn ping(&mut self) -> Result<(), DaemonError> {
        framed::send(&mut self.writer, &Message::Ping)?;
        match framed::read_message(&mut self.reader)? {
            Some(Message::Pong) => Ok(()),
            other => Err(DaemonError::Unexpected(format!(
                "期待 pong，收到 {other:?}"
            ))),
        }
    }
}

/// 连接并握手。`timeout` 覆盖整个「管道忙 → 重试」窗口。
pub fn connect(info: &DaemonInfo, timeout: Duration) -> Result<Client, DaemonError> {
    let handle = open_pipe(&info.pipe, timeout)?;
    let stream = unsafe { File::from_raw_handle(handle) };
    // 读走 `BufReader`、写留副本：`File` 不能同时被两处可变借用，而管道句柄可以 `DuplicateHandle`。
    let mut writer = stream.try_clone().map_err(DaemonError::from)?;
    let mut reader = BufReader::new(stream);

    framed::send(
        &mut writer,
        &Message::Hello(Hello {
            proto_min: PROTOCOL_VERSION_MIN,
            proto_max: PROTOCOL_VERSION,
            caps: Caps::supported(),
            token: info.token.clone(),
            client_instance: client_instance(),
            resume: Vec::new(),
        }),
    )?;

    match framed::read_message(&mut reader)? {
        Some(Message::Welcome(welcome)) => {
            // 协商由 daemon 做（取交集最高版本）；这里只守一条不变量：它选出来的版本我们得认识。
            if !(PROTOCOL_VERSION_MIN..=PROTOCOL_VERSION).contains(&welcome.proto) {
                return Err(DaemonError::Protocol {
                    reason: format!(
                        "daemon 选定版本 {} 不在客户端支持区间 [{PROTOCOL_VERSION_MIN}..{PROTOCOL_VERSION}]",
                        welcome.proto
                    ),
                });
            }
            Ok(Client { reader, writer })
        }
        Some(Message::Error(ProtocolError { code, message })) => Err(DaemonError::Rejected {
            reason: format!("{code:?}：{message}"),
        }),
        other => Err(DaemonError::Unexpected(format!(
            "期待 welcome，收到 {other:?}"
        ))),
    }
}

/// 客户端实例 id（diagnostics、以及「同一 UI 的多次连接」识别）。
fn client_instance() -> String {
    format!("ui-{}-{}", std::process::id(), crate::logging::now_ms())
}

/// 打开管道客户端句柄；`ERROR_PIPE_BUSY` 时重试到 `timeout`。
fn open_pipe(pipe: &str, timeout: Duration) -> Result<HANDLE, DaemonError> {
    let name = win::to_wide(pipe);
    let deadline = Instant::now() + timeout;
    loop {
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };
        if handle != INVALID_HANDLE_VALUE {
            return Ok(handle);
        }

        let error = unsafe { GetLastError() };
        // 实例都被占着（daemon 正在招呼别的连接）——等它腾出来再试。
        if error == ERROR_PIPE_BUSY && Instant::now() < deadline {
            unsafe { WaitNamedPipeW(name.as_ptr(), RETRY_WAIT_MS) };
            continue;
        }
        return Err(DaemonError::NotRunning {
            reason: format!("CreateFileW({pipe}) 失败：GetLastError={error}"),
        });
    }
}
