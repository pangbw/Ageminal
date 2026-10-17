//! 命名管道客户端：连接 + 握手 + 会话操作（issue #64 / #65 / #67）。
//!
//! 这是 app-core 侧唯一的对话入口（Tauri 命令面在 #68）。**只经 `ageminal-protocol` 的类型收发**
//! （#65 验收 4），不自己碰字节。
//!
//! 握手之后读端交给一条后台线程，把所有入站消息送进 channel；请求-响应型方法从 channel 里挑自己
//! 要的那条，其余（`Data` / `SessionExited` 等事件）进**待取队列**，由 [`Client::next_event`] 取。
//!
//! ⚠️ 待取队列目前是**无界**的：这一层还没有背压（#73）与按会话分发的 Channel（#68）。#67 的用途
//! 是无 UI 的集成测试，量小；#68 会换成每会话一条通道。

use std::collections::VecDeque;
use std::fs::File;
use std::io::BufReader;
use std::os::windows::io::FromRawHandle;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ageminal_protocol::{
    Ack, Attach, Attached, Caps, CreateSession, Detach, Hello, Message, ProtocolError, Resize,
    SessionCreated, Terminate, PROTOCOL_VERSION, PROTOCOL_VERSION_MIN,
};
use windows_sys::Win32::Foundation::{
    GetLastError, ERROR_PIPE_BUSY, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, OPEN_EXISTING};
use windows_sys::Win32::System::Pipes::WaitNamedPipeW;

use crate::pty::PtySize;
use crate::{framed, win, DaemonError, DaemonInfo};

/// 管道忙时的单次重试等待（`WaitNamedPipeW` 的毫秒数）。
const RETRY_WAIT_MS: u32 = 250;

/// 与 daemon 的一条**已握手**连接。
#[derive(Debug)]
pub struct Client {
    writer: File,
    /// 后台读线程送来的所有入站消息。
    incoming: Receiver<Message>,
    /// 请求-响应时「抢跑」收到的事件，留给 [`Client::next_event`]。
    pending: VecDeque<Message>,
    /// 读线程；drop 即分离（写端关掉后它自己会退出）。
    _reader: JoinHandle<()>,
}

impl Client {
    /// 存活探测：发 `ping`，等 `pong`。
    pub fn ping(&mut self) -> Result<(), DaemonError> {
        self.request(&Message::Ping, |message| matches!(message, Message::Pong))?;
        Ok(())
    }

    /// 建会话。
    pub fn create_session(
        &mut self,
        request: CreateSession,
    ) -> Result<SessionCreated, DaemonError> {
        let message = self.request(&Message::CreateSession(request), |message| {
            matches!(message, Message::SessionCreated(_))
        })?;
        match message {
            Message::SessionCreated(created) => Ok(created),
            other => Err(DaemonError::Unexpected(format!(
                "期待 sessionCreated，收到 {other:?}"
            ))),
        }
    }

    /// 接回已有会话。
    pub fn attach(&mut self, sid: &str, last_seq: u64) -> Result<Attached, DaemonError> {
        let message = self.request(
            &Message::Attach(Attach {
                sid: sid.to_owned(),
                last_seq,
            }),
            |message| matches!(message, Message::Attached(_)),
        )?;
        match message {
            Message::Attached(attached) => Ok(attached),
            other => Err(DaemonError::Unexpected(format!(
                "期待 attached，收到 {other:?}"
            ))),
        }
    }

    /// 写键盘输入（无响应）。
    pub fn write_input(&mut self, handle: u32, bytes: &[u8]) -> Result<(), DaemonError> {
        framed::send(
            &mut self.writer,
            &Message::Input {
                handle,
                seq: 0,
                bytes: bytes.to_vec(),
            },
        )
    }

    /// 改尺寸（无响应）。
    pub fn resize(&mut self, sid: &str, size: PtySize) -> Result<(), DaemonError> {
        framed::send(
            &mut self.writer,
            &Message::Resize(Resize {
                sid: sid.to_owned(),
                cols: size.cols,
                rows: size.rows,
            }),
        )
    }

    /// 断开（**≠ 终止**）：会话继续活在 daemon 里（无响应）。
    pub fn detach(&mut self, sid: &str) -> Result<(), DaemonError> {
        framed::send(
            &mut self.writer,
            &Message::Detach(Detach {
                sid: sid.to_owned(),
            }),
        )
    }

    /// 终止会话（无响应；结果看 `sessionExited` 事件）。
    pub fn terminate(&mut self, sid: &str, force: bool) -> Result<(), DaemonError> {
        framed::send(
            &mut self.writer,
            &Message::Terminate(Terminate {
                sid: sid.to_owned(),
                force,
            }),
        )
    }

    /// 确认已消费字节数（无响应）。
    pub fn ack(&mut self, sid: &str, acked_bytes: u64) -> Result<(), DaemonError> {
        framed::send(
            &mut self.writer,
            &Message::Ack(Ack {
                sid: sid.to_owned(),
                acked_bytes,
            }),
        )
    }

    /// 等下一条**事件**（`Data` / `SessionExited`）；超时返回 `None`。
    pub fn next_event(&mut self, timeout: Duration) -> Result<Option<Message>, DaemonError> {
        if let Some(message) = self.pending.pop_front() {
            return Ok(Some(message));
        }
        match self.incoming.recv_timeout(timeout) {
            Ok(message) => Ok(Some(message)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(DaemonError::Protocol {
                reason: "daemon 关闭了连接".to_owned(),
            }),
        }
    }

    /// 发一条消息，等**第一条**满足 `accept` 的回复。
    ///
    /// 中间收到的事件不丢，进待取队列；daemon 回的 `Error` 直接变成错误返回。
    fn request(
        &mut self,
        message: &Message,
        accept: impl Fn(&Message) -> bool,
    ) -> Result<Message, DaemonError> {
        framed::send(&mut self.writer, message)?;
        loop {
            let incoming = self.next_incoming()?;
            if let Message::Error(ProtocolError { code, message }) = &incoming {
                return Err(DaemonError::Rejected {
                    reason: format!("{code:?}：{message}"),
                });
            }
            if accept(&incoming) {
                return Ok(incoming);
            }
            self.pending.push_back(incoming);
        }
    }

    /// 下一条入站消息（阻塞；待取队列优先）。
    fn next_incoming(&mut self) -> Result<Message, DaemonError> {
        if let Some(message) = self.pending.pop_front() {
            return Ok(message);
        }
        self.incoming.recv().map_err(|_| DaemonError::Protocol {
            reason: "daemon 关闭了连接".to_owned(),
        })
    }
}

/// 连接并握手。
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
            client_instance_id: client_instance_id(),
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
        }
        Some(Message::Error(ProtocolError { code, message })) => {
            return Err(DaemonError::Rejected {
                reason: format!("{code:?}：{message}"),
            })
        }
        other => {
            return Err(DaemonError::Unexpected(format!(
                "期待 welcome，收到 {other:?}"
            )))
        }
    }

    // 握手完成后，读端交给后台线程：会话输出与事件都是异步来的，读线程不能只服务一次请求。
    let (sender, incoming) = mpsc::channel();
    let reader_thread = std::thread::spawn(move || loop {
        match framed::read_message(&mut reader) {
            Ok(Some(message)) => {
                if sender.send(message).is_err() {
                    return;
                }
            }
            Ok(None) | Err(_) => return,
        }
    });

    Ok(Client {
        writer,
        incoming,
        pending: VecDeque::new(),
        _reader: reader_thread,
    })
}

/// 客户端实例 id（diagnostics、以及「同一 UI 的多次连接」识别）。
fn client_instance_id() -> String {
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
