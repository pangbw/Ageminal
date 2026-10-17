//! 命名管道客户端：连接 + 握手 + 会话操作（issue #64 / #65 / #67）。
//!
//! 这是 app-core 侧唯一的对话入口（Tauri 命令面在 #68）。**只经 `ageminal-protocol` 的类型收发**
//! （#65 验收 4），不自己碰字节。
//!
//! # 为什么没有后台读线程
//!
//! 曾经有过：读端交给线程、消息进 channel。**它会死锁**——那条线程一直握着管道句柄，调用方
//! `drop(Client)` 之后 daemon 仍然看不到 EOF，于是不会 `DisconnectNamedPipe`，下一次连接就永远
//! `ERROR_PIPE_BUSY`；而 daemon 不关连接，读线程也永远等不到 EOF。（CI 上真踩过：握手能过、
//! `ping` 拿不到回复，随后所有连接都连不上。）
//!
//! 所以这里是**单线程 + 阻塞读**：没有常驻线程，`drop` 时两个句柄一起关，daemon 立刻看到 EOF。
//!
//! 也试过用 `PeekNamedPipe` 轮询来加读超时——**不可行**：daemon 侧写一帧时，`WriteFile` 要等
//! 对端**读**才完成，而 peek 在写完成前看不到数据，于是「客户端等 peek 有数据、daemon 等客户端读」
//! 互锁（CI 实测：daemon 收到了 Ping，写调用一直不返回，客户端 20 s 一点数据都没看到）。
//!
//! 后来定位到真正的根因在**服务端**：同步管道句柄上的读写会互相串行，daemon 一阻塞在 `ReadFile`，
//! 写线程就写不出去（见 `server::wait_for_incoming`）。服务端改成「先 peek 再读」之后，客户端这边
//! 用 `PeekNamedPipe` 等超时是安全的，也就恢复成带超时的等待。
//!
//! 真要支持取消，得把句柄开成 `FILE_FLAG_OVERLAPPED` 并全程走 `OVERLAPPED`——那是 #68 的事。

use std::collections::VecDeque;
use std::fs::File;
use std::io::BufReader;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::ptr;
use std::time::{Duration, Instant};

use ageminal_protocol::{
    Ack, Attach, Attached, Caps, CreateSession, Detach, Hello, Message, ProtocolError, Resize,
    SessionCreated, Terminate, PROTOCOL_VERSION, PROTOCOL_VERSION_MIN,
};
use windows_sys::Win32::Foundation::{
    GetLastError, ERROR_PIPE_BUSY, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, OPEN_EXISTING};
use windows_sys::Win32::System::Pipes::{PeekNamedPipe, WaitNamedPipeW};

use crate::pty::PtySize;
use crate::{framed, win, DaemonError, DaemonInfo};

/// 管道忙时的单次重试等待（`WaitNamedPipeW` 的毫秒数）。
const RETRY_WAIT_MS: u32 = 250;

/// 请求-响应的等待上限。**不能无限等**：daemon 卡住时调用方（乃至 CI）会一起挂死。
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(20);

/// 等管道可读时的轮询间隔。
const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// 与 daemon 的一条**已握手**连接。
#[derive(Debug)]
pub struct Client {
    reader: BufReader<File>,
    writer: File,
    /// 请求-响应时「抢跑」收到的事件，留给 [`Client::next_event`]。
    pending: VecDeque<Message>,
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
        if !self.await_readable(Instant::now() + timeout)? {
            return Ok(None);
        }
        self.read_incoming().map(Some)
    }

    /// 下一条入站消息（待取队列优先）；到 `deadline` 还没有就返回 `None`。
    fn next_incoming_before(&mut self, deadline: Instant) -> Result<Option<Message>, DaemonError> {
        if let Some(message) = self.pending.pop_front() {
            return Ok(Some(message));
        }
        if !self.await_readable(deadline)? {
            return Ok(None);
        }
        self.read_incoming().map(Some)
    }

    /// 读一条（调用前应已确认可读）。
    fn read_incoming(&mut self) -> Result<Message, DaemonError> {
        match framed::read_message(&mut self.reader)? {
            Some(message) => Ok(message),
            None => Err(DaemonError::Protocol {
                reason: "daemon 关闭了连接".to_owned(),
            }),
        }
    }

    /// 等管道里有字节可读；`deadline` 前没有就返回 `false`。
    fn await_readable(&self, deadline: Instant) -> Result<bool, DaemonError> {
        loop {
            let mut available: u32 = 0;
            let ok = unsafe {
                PeekNamedPipe(
                    self.reader.get_ref().as_raw_handle(),
                    ptr::null_mut(),
                    0,
                    ptr::null_mut(),
                    &mut available,
                    ptr::null_mut(),
                )
            };
            if ok == 0 {
                return Err(DaemonError::NotRunning {
                    reason: format!("PeekNamedPipe 失败：GetLastError={}", unsafe {
                        GetLastError()
                    }),
                });
            }
            if available > 0 {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            std::thread::sleep(POLL_INTERVAL);
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
        let deadline = Instant::now() + RESPONSE_TIMEOUT;
        loop {
            let Some(incoming) = self.next_incoming_before(deadline)? else {
                return Err(DaemonError::Protocol {
                    reason: format!("daemon 在 {RESPONSE_TIMEOUT:?} 内没有回应 {message:?}"),
                });
            };
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

    Ok(Client {
        reader,
        writer,
        pending: VecDeque::new(),
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
