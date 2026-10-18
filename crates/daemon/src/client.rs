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
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::ptr;
use std::time::{Duration, Instant};

use ageminal_protocol::{
    Ack, Attach, Attached, Caps, Checkpoint, CreateSession, Detach, Hello, Message, ProtocolError,
    Resize, SessionCreated, SessionInfo, Terminate, PROTOCOL_VERSION, PROTOCOL_VERSION_MIN,
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
    reader: File,
    writer: File,
    /// 请求-响应时「抢跑」收到的事件，留给 [`Client::next_event`]。
    ///
    /// ⚠️ **无界**：这一层还没有背压（#73）与按会话分发的通道（#68）。本票的用途是无 UI 的集成
    /// 测试（量小）；#68 会换成每会话一条通道，届时把这里删掉。
    pending: VecDeque<Message>,
}

impl Client {
    /// 存活探测：发 `ping`，等 `pong`。
    pub fn ping(&mut self) -> Result<(), DaemonError> {
        self.request(&Message::Ping, |message| matches!(message, Message::Pong))?;
        Ok(())
    }

    /// 枚举 daemon 中仍然存活的会话（UI 重开恢复 / 启动对账，#75）。
    pub fn list_sessions(&mut self) -> Result<Vec<SessionInfo>, DaemonError> {
        let message = self.request(&Message::ListSessions, |message| {
            matches!(message, Message::Sessions(_))
        })?;
        match message {
            Message::Sessions(sessions) => Ok(sessions.sessions),
            other => Err(DaemonError::Unexpected(format!(
                "期待 sessions，收到 {other:?}"
            ))),
        }
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

    /// 请求一份当前 checkpoint（无响应；结果看 `Snapshot` 事件）。
    pub fn checkpoint(&mut self, sid: &str) -> Result<(), DaemonError> {
        framed::send(
            &mut self.writer,
            &Message::Checkpoint(Checkpoint {
                sid: sid.to_owned(),
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
                    self.reader.as_raw_handle(),
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
            // ⚠️ **只从管道读，绝不从 `pending` 取**：`pending` 是留给 `next_event` 的事件队列。
            // 从里面取会把自己刚塞进去的事件原样再拿出来，`accept` 永远不成立——死循环空转。
            // （真机实测：第二个 `createSession` 撞上第一个会话的启动横幅时，100% CPU 打转不返回；
            //   CI 只是时序恰好躲开了。）
            if !self.await_readable(deadline)? {
                return Err(DaemonError::Protocol {
                    reason: format!("daemon 在 {RESPONSE_TIMEOUT:?} 内没有回应 {message:?}"),
                });
            }
            let incoming = self.read_incoming()?;
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
    // 与 daemon 侧同理：**不套 `BufReader`**，否则 peek 看不到被缓冲的帧（见模块文档）。
    let mut reader = stream;

    framed::send(
        &mut writer,
        &Message::Hello(Hello {
            proto_min: PROTOCOL_VERSION_MIN,
            proto_max: PROTOCOL_VERSION,
            caps: Caps::supported(),
            token: info.token.clone(),
            client_instance_id: client_instance_id(),
            // 跨 UI 重开走 `ListSessions` + `Attach(lastSeq = 0)`（#75）：新 UI 没有可信的
            // lastSeq，先拿快照。`Hello.resume` 预留给后续单连接批量恢复。
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

#[cfg(test)]
mod tests {
    use std::os::windows::io::FromRawHandle;
    use std::sync::mpsc;
    use std::time::Duration;

    use ageminal_protocol::{Caps, Message, Welcome};
    use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
    };

    use super::*;
    use crate::framed;

    /// **事件帧比响应帧先到**时，请求也必须返回。
    ///
    /// 真机上踩过：`request` 把不匹配的消息塞进 `pending`（`next_event` 的事件队列），再回到循环时
    /// 又**优先从 `pending` 取**，于是同一条 `Data` 帧被「取出 → 不匹配 → 塞回」无限循环，既不读管道
    /// 也不返回，20 s 的请求超时也不会触发（它只在等可读时检查）。CI 因时序恰好躲开，这里用假 daemon
    /// 把它变成确定性的：**先发一条 `Data`，再回 `Pong`**。
    #[test]
    fn a_response_is_not_starved_by_an_event_that_arrives_first() {
        let pipe = format!(r"\\.\pipe\ageminal-client-test-{}", std::process::id());
        let name = crate::win::to_wide(&pipe);
        let handle = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                1,
                8 * 1024,
                8 * 1024,
                0,
                std::ptr::null_mut(),
            )
        };
        assert!(!handle.is_null(), "建测试管道失败");

        // 裸 HANDLE 是 `*mut c_void`（不是 Send）。这里只把它交给**一条**线程用，包一层就够了。
        struct SendHandle(isize);
        unsafe impl Send for SendHandle {}
        let raw = SendHandle(handle as isize);

        // 假 daemon：握手 → **先**发事件 → **再**回 ping。
        let daemon = std::thread::spawn(move || {
            let handle = raw.0 as windows_sys::Win32::Foundation::HANDLE;
            unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) };
            let file = unsafe { File::from_raw_handle(handle) };
            let mut reader = &file;
            let mut writer = &file;
            assert!(matches!(
                framed::read_message(&mut reader).expect("握手"),
                Some(Message::Hello(_))
            ));
            framed::send(
                &mut writer,
                &Message::Welcome(Welcome {
                    proto: PROTOCOL_VERSION,
                    caps: Caps::supported(),
                    daemon_version: "test".to_owned(),
                }),
            )
            .expect("welcome");
            framed::send(
                &mut writer,
                &Message::Data {
                    handle: 1,
                    seq: 0,
                    bytes: b"banner".to_vec(),
                },
            )
            .expect("data");
            framed::send(&mut writer, &Message::Pong).expect("pong");
            // 别急着断：断开会变成「连接关闭」，把「被饿死」这个失败模式掩盖成另一个错误。
            std::thread::sleep(Duration::from_millis(500));
        });

        let info = DaemonInfo {
            pid: 1,
            pipe,
            token: "test".to_owned(),
            protocol: PROTOCOL_VERSION,
            daemon_version: "test".to_owned(),
            started_at_ms: 0,
        };

        // 请求放进线程 + 超时：回归时测试要**失败**，而不是把测试二进制一起挂住。
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let result =
                connect(&info, Duration::from_secs(5)).and_then(|mut client| client.ping());
            let _ = sender.send(result.is_ok());
        });
        match receiver.recv_timeout(Duration::from_secs(5)) {
            Ok(ok) => assert!(ok, "ping 应当成功"),
            Err(_) => panic!("请求被先到的事件帧饿死了（回归）"),
        }
        let _ = daemon.join();
    }
}
