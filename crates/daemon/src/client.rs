//! 命名管道客户端：连接 + token 握手 + ping（issue #64）。
//!
//! 这是 app-core 侧唯一的对话入口；agent hook shim（`ageminal-notify`）将来也走同一套。

use std::fs::File;
use std::io::BufReader;
use std::os::windows::io::FromRawHandle;
use std::time::{Duration, Instant};

use ageminal_protocol::{ClientMessage, ServerMessage, PROTOCOL_VERSION};
use windows_sys::Win32::Foundation::{
    GetLastError, ERROR_PIPE_BUSY, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, OPEN_EXISTING};
use windows_sys::Win32::System::Pipes::WaitNamedPipeW;

use crate::{win, wire, DaemonError, DaemonInfo};

/// 管道忙时的单次重试等待（`WaitNamedPipeW` 的毫秒数）。
const RETRY_WAIT_MS: u32 = 250;

/// 与 daemon 的一条**已握手**连接。
#[derive(Debug)]
pub struct Client {
    reader: BufReader<File>,
    writer: File,
}

impl Client {
    /// 存活探测：发 `ping`，核对回显的 `nonce`。
    pub fn ping(&mut self, nonce: u64) -> Result<(), DaemonError> {
        wire::write_message(&mut self.writer, &ClientMessage::Ping { nonce })?;
        let response: ServerMessage = wire::read_message(&mut self.reader)?;
        match response {
            ServerMessage::Pong { nonce: echo } if echo == nonce => Ok(()),
            other => Err(DaemonError::Unexpected(format!(
                "期待 pong({nonce})，收到 {other:?}"
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

    wire::write_message(
        &mut writer,
        &ClientMessage::Hello {
            token: info.token.clone(),
            protocol: PROTOCOL_VERSION,
        },
    )?;
    let response: ServerMessage = wire::read_message(&mut reader)?;
    match &response {
        ServerMessage::Welcome { protocol, .. } if *protocol == PROTOCOL_VERSION => {}
        ServerMessage::Welcome { protocol, .. } => {
            return Err(DaemonError::ProtocolMismatch {
                daemon: *protocol,
                client: PROTOCOL_VERSION,
            })
        }
        ServerMessage::Rejected { reason } => {
            return Err(DaemonError::Rejected {
                reason: reason.clone(),
            })
        }
        other => {
            return Err(DaemonError::Unexpected(format!(
                "期待 welcome，收到 {other:?}"
            )))
        }
    }

    Ok(Client { reader, writer })
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
