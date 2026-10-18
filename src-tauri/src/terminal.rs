//! 终端会话接线：daemon ↔ app-core ↔ 前端（issue #68）。
//!
//! # 数据路径
//!
//! ```text
//! ConPTY ──daemon──▶ Client ──合批──▶ Tauri Channel（Raw）──▶ xterm.js
//!                        ▲                                        │
//!                        └──────────── ack（累计字节）────────────┘
//! ```
//!
//! 前端**每个会话一条** `Channel`：PTY 字节走 `Raw`（xterm 收到 `ArrayBuffer`），
//! 退出 / 断连等控制消息走同一条 Channel 的 JSON 帧——两者共用 Tauri 的序号队列，天然保序。
//! 合批阈值按 REQUIREMENTS.md §7.2：**≥16 KiB 或 ≥8 ms**；小于 1 KiB 的 Raw 帧在 Tauri 里会
//! 退化成 JSON 数字数组，所以合批同时是性能与正确性的要求。
//!
//! # 为什么是一条服务线程 + 轮询
//!
//! daemon 的 [`ageminal_daemon::client::Client`] 是同步、单线程的：后台读线程会握着管道句柄，
//! 让 `drop` 看不到 EOF；`PeekNamedPipe` 轮询又与 daemon 的写互锁（见 `client.rs` 的模块文档）。
//! 因此这里让**唯一一条线程**同时持有连接、命令队列和所有 Channel：每 2 ms 醒一次，先处理
//! 前端命令，再非阻塞地收 daemon 事件，最后按合批窗口冲刷。命令最多等 2 ms，输入延迟可接受，
//! 且不会出现「两个线程抢一个同步管道句柄」的老问题。
//!
//! # 隐藏会话
//!
//! 页签不可见时前端调 [`terminal_set_visible`]：app-core 向 daemon 发 `Detach`，不再转发字节。
//! 切回时 `Attach` 从当前进度续流；真正的快照回放归 #71 / #72，本票只保证「隐藏时停止收流」。
//!
//! # 尚未实现（后续票）
//!
//! - 流控高低水位与 daemon 侧暂停发送：见 #73；这里只把 ack 原样转给 daemon 记账。
//! - 关闭页签的「detach 而不是 terminate」语义：见 #75；本票的关闭是显式结束会话。

use std::sync::mpsc;
use std::time::Duration;

use serde::Serialize;
use specta::Type;
use tauri::ipc::{Channel, InvokeResponseBody, IpcResponse};

/// 合批上限：攒到这个字节数立刻发，不等 8 ms 窗口。
const BATCH_BYTES: usize = 16 * 1024;

/// 合批窗口：首字节之后最多等这么久。
const BATCH_WINDOW: Duration = Duration::from_millis(8);

/// 服务线程的轮询间隔。命令 / 输入的最坏延迟就是它。
const POLL_INTERVAL: Duration = Duration::from_millis(2);

/// 一次轮询最多处理多少条 daemon 消息，避免洪峰把命令面饿死。
const MAX_FRAMES_PER_TICK: usize = 64;

/// 首次连接 daemon 的等待上限（应用启动时 daemon 可能还在拉起）。
#[cfg(windows)]
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// 建会话的等待上限；daemon 内部的请求超时是 20 s，这里留一点余量。
#[cfg(windows)]
const OPEN_TIMEOUT: Duration = Duration::from_secs(22);

/// 前端拿到的建会话结果。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TerminalOpened {
    /// 逻辑会话 id。
    pub sid: String,
    /// shell 进程 id。
    pub pid: u32,
    /// 本次 spawn 的世代 id。
    pub incarnation_id: String,
}

/// 走 Channel JSON 帧的控制消息。
#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TerminalControl {
    /// 进程退出。
    Exit {
        /// 退出码；拿不到时为 `None`。
        code: Option<i32>,
    },
    /// 重新 attach 成功；`from_seq` 是本次续流的起始字节偏移。
    ///
    /// 前端据此把 ack 计数器清零（ack 是**本次 attach 内**的累计值，app-core 再加 `from_seq`）。
    Attached {
        /// 续流起点。
        from_seq: u64,
    },
    /// 操作失败（可见给用户）。
    Error {
        /// 失败理由。
        message: String,
    },
    /// 与 daemon 的连接断了。
    Closed {
        /// 断连理由。
        reason: String,
    },
}

/// 一条 Channel 消息：Raw 给 PTY 字节，JSON 给控制消息。
///
/// 前端声明 `Channel<ArrayBuffer | TerminalControl>`；Rust 侧这个类型只用来约束
/// [`IpcResponse`]（`Raw` 而不是 JSON 数字数组）。生成的 TS 是 `Channel<number>`（占位），
/// 前端在调用处收窄。
pub enum TerminalEvent {
    /// PTY 原始字节。
    Data(Vec<u8>),
    /// 控制消息。
    Control(TerminalControl),
}

impl IpcResponse for TerminalEvent {
    fn body(self) -> tauri::Result<InvokeResponseBody> {
        Ok(match self {
            TerminalEvent::Data(bytes) => InvokeResponseBody::Raw(bytes),
            TerminalEvent::Control(control) => {
                InvokeResponseBody::Json(serde_json::to_string(&control)?)
            }
        })
    }
}

// `Channel<T>` 的类型参数要求 `specta::Type`；运行时这里既可能是 ArrayBuffer 也可能是对象，
// 无法用一个普通的 Serialize 类型表达。生成成 `number` 只是占位——前端会把它收窄成
// `ArrayBuffer | TerminalControl`（见 `src/features/terminal/runtime.ts`）。
impl Type for TerminalEvent {
    fn definition(_types: &mut specta::Types) -> specta::datatype::DataType {
        specta::datatype::DataType::Primitive(specta::datatype::Primitive::u8)
    }
}

/// 终端服务句柄。Tauri 状态里只放这个；真正的连接归后台服务线程。
#[derive(Clone)]
pub struct TerminalService {
    #[cfg(windows)]
    sender: mpsc::Sender<Request>,
}

impl TerminalService {
    /// 启动服务线程。即使线程起不来，返回的句柄也只会让命令报错，不影响应用启动。
    pub fn start() -> Self {
        #[cfg(windows)]
        {
            let (sender, receiver) = mpsc::channel();
            let worker = sender.clone();
            if let Err(error) = std::thread::Builder::new()
                .name("terminal-service".to_owned())
                .spawn(move || service_loop(receiver))
            {
                eprintln!("[terminal] 服务线程启动失败：{error}");
            }
            Self { sender: worker }
        }
        #[cfg(not(windows))]
        {
            Self {}
        }
    }

    /// 建会话。
    pub fn open(
        &self,
        sid: String,
        cwd: String,
        cols: u16,
        rows: u16,
        event: Channel<TerminalEvent>,
    ) -> Result<TerminalOpened, String> {
        #[cfg(windows)]
        {
            let (reply, receiver) = mpsc::sync_channel(1);
            self.send(Request::Open {
                sid,
                cwd,
                cols,
                rows,
                event,
                reply,
            })?;
            match receiver.recv_timeout(OPEN_TIMEOUT) {
                Ok(result) => result,
                Err(_) => Err("终端服务在等待 daemon 建会话时超时".to_owned()),
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (sid, cwd, cols, rows, event);
            Err(windows_only())
        }
    }

    /// 键盘输入（UTF-8 字节）。
    pub fn input(&self, sid: String, bytes: Vec<u8>) -> Result<(), String> {
        #[cfg(windows)]
        {
            self.send(Request::Input { sid, bytes })
        }
        #[cfg(not(windows))]
        {
            let _ = (sid, bytes);
            Err(windows_only())
        }
    }

    /// 同步终端尺寸。
    pub fn resize(&self, sid: String, cols: u16, rows: u16) -> Result<(), String> {
        #[cfg(windows)]
        {
            self.send(Request::Resize { sid, cols, rows })
        }
        #[cfg(not(windows))]
        {
            let _ = (sid, cols, rows);
            Err(windows_only())
        }
    }

    /// 累计已消费字节（ack）。
    pub fn ack(&self, sid: String, acked_bytes: u64) -> Result<(), String> {
        #[cfg(windows)]
        {
            self.send(Request::Ack { sid, acked_bytes })
        }
        #[cfg(not(windows))]
        {
            let _ = (sid, acked_bytes);
            Err(windows_only())
        }
    }

    /// 设置可见性：不可见 → `Detach` 并停止转发；可见 → `Attach` 续流。
    pub fn set_visible(&self, sid: String, visible: bool) -> Result<(), String> {
        #[cfg(windows)]
        {
            self.send(Request::SetVisible { sid, visible })
        }
        #[cfg(not(windows))]
        {
            let _ = (sid, visible);
            Err(windows_only())
        }
    }

    /// 结束会话（临时语义；关闭页签的 detach 语义归 #75）。
    pub fn close(&self, sid: String) -> Result<(), String> {
        #[cfg(windows)]
        {
            self.send(Request::Close { sid })
        }
        #[cfg(not(windows))]
        {
            let _ = sid;
            Err(windows_only())
        }
    }

    #[cfg(windows)]
    fn send(&self, request: Request) -> Result<(), String> {
        self.sender
            .send(request)
            .map_err(|_| "终端服务已停止".to_owned())
    }
}

#[cfg(not(windows))]
fn windows_only() -> String {
    "终端会话仅支持 Windows（daemon / ConPTY）".to_owned()
}

/// Tauri 命令：建会话。`async` + `spawn_blocking`，建会话期间不占住 UI 线程。
#[tauri::command]
#[specta::specta]
pub async fn terminal_open(
    state: tauri::State<'_, TerminalService>,
    sid: String,
    cwd: String,
    cols: u16,
    rows: u16,
    on_event: Channel<TerminalEvent>,
) -> Result<TerminalOpened, String> {
    let service = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || service.open(sid, cwd, cols, rows, on_event))
        .await
        .map_err(|error| format!("终端服务不可用：{error}"))?
}

/// Tauri 命令：键盘输入。`data` 是 xterm `onData` 给的字符串（UTF-8）。
#[tauri::command]
#[specta::specta]
pub fn terminal_write(
    state: tauri::State<'_, TerminalService>,
    sid: String,
    data: String,
) -> Result<(), String> {
    state.inner().input(sid, data.into_bytes())
}

/// Tauri 命令：同步 resize。
#[tauri::command]
#[specta::specta]
pub fn terminal_resize(
    state: tauri::State<'_, TerminalService>,
    sid: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    state.inner().resize(sid, cols, rows)
}

/// Tauri 命令：累计消费字节（xterm `write` 回调之后）。
///
/// 用 `f64` 而不是 `u64`：specta 拒绝对 `u64` 生成 TS（BigInt 精度问题）；JS 侧本来就是
/// number，2^53 字节（≈9 PB）远大于任何真实会话。
#[tauri::command]
#[specta::specta]
pub fn terminal_ack(
    state: tauri::State<'_, TerminalService>,
    sid: String,
    acked_bytes: f64,
) -> Result<(), String> {
    if !acked_bytes.is_finite() || acked_bytes < 0.0 {
        return Err(format!("非法的 ack 计数：{acked_bytes}"));
    }
    state.inner().ack(sid, acked_bytes as u64)
}

/// Tauri 命令：页签可见性变化。
#[tauri::command]
#[specta::specta]
pub fn terminal_set_visible(
    state: tauri::State<'_, TerminalService>,
    sid: String,
    visible: bool,
) -> Result<(), String> {
    state.inner().set_visible(sid, visible)
}

/// Tauri 命令：结束会话并释放服务端状态。
#[tauri::command]
#[specta::specta]
pub fn terminal_close(state: tauri::State<'_, TerminalService>, sid: String) -> Result<(), String> {
    state.inner().close(sid)
}

/// 发往服务线程的请求。
#[cfg(windows)]
enum Request {
    Open {
        sid: String,
        cwd: String,
        cols: u16,
        rows: u16,
        event: Channel<TerminalEvent>,
        reply: mpsc::SyncSender<Result<TerminalOpened, String>>,
    },
    Input {
        sid: String,
        bytes: Vec<u8>,
    },
    Resize {
        sid: String,
        cols: u16,
        rows: u16,
    },
    Ack {
        sid: String,
        acked_bytes: u64,
    },
    SetVisible {
        sid: String,
        visible: bool,
    },
    Close {
        sid: String,
    },
}

/// 每个会话的 app-core 侧状态。
#[cfg(windows)]
struct SessionState {
    handle: u32,
    event: Channel<TerminalEvent>,
    visible: bool,
    /// 合批缓冲。
    batcher: Batcher,
    /// daemon 已发出的最后一个字节偏移。
    last_seq: u64,
    /// 本次 attach 的起点；ack 回给 daemon 时要加上它。
    ack_base: u64,
}

/// 合批器。抽成独立类型，好在非 Windows 上也能单测「不丢字节 / 阈值触发」。
#[derive(Default)]
struct Batcher {
    bytes: Vec<u8>,
    first_at: Option<std::time::Instant>,
}

impl Batcher {
    /// 追加一段字节；达到大小阈值时立刻取出整批。
    fn push(&mut self, bytes: &[u8], now: std::time::Instant) -> Option<Vec<u8>> {
        if self.bytes.is_empty() {
            self.first_at = Some(now);
        }
        self.bytes.extend_from_slice(bytes);
        (self.bytes.len() >= BATCH_BYTES)
            .then(|| self.take())
            .flatten()
    }

    /// 到 8 ms 窗口就取出；否则继续攒。
    fn take_if_due(&mut self, now: std::time::Instant) -> Option<Vec<u8>> {
        if self.bytes.is_empty() {
            return None;
        }
        let due = self
            .first_at
            .is_some_and(|first| now.duration_since(first) >= BATCH_WINDOW);
        if due || self.bytes.len() >= BATCH_BYTES {
            self.take()
        } else {
            None
        }
    }

    /// 无条件取出（发控制消息前要先冲刷，保证同会话内的顺序）。
    fn flush(&mut self) -> Option<Vec<u8>> {
        self.take()
    }

    fn take(&mut self) -> Option<Vec<u8>> {
        if self.bytes.is_empty() {
            return None;
        }
        self.first_at = None;
        Some(std::mem::take(&mut self.bytes))
    }
}

/// 服务线程的运行期状态。
#[cfg(windows)]
struct Runtime {
    client: Option<ageminal_daemon::client::Client>,
    sessions: std::collections::HashMap<String, SessionState>,
    by_handle: std::collections::HashMap<u32, String>,
}

#[cfg(windows)]
fn service_loop(receiver: mpsc::Receiver<Request>) {
    let mut runtime = Runtime {
        client: None,
        sessions: std::collections::HashMap::new(),
        by_handle: std::collections::HashMap::new(),
    };

    loop {
        // 完全空闲（没有连接、没有会话）时阻塞等命令，别每 2 ms 空转。
        if runtime.client.is_none() && runtime.sessions.is_empty() {
            match receiver.recv_timeout(Duration::from_millis(100)) {
                Ok(request) => handle_request(&mut runtime, request),
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }

        // 1) 前端命令优先：命令最多等一轮轮询（2 ms）。
        loop {
            match receiver.try_recv() {
                Ok(request) => handle_request(&mut runtime, request),
                Err(mpsc::TryRecvError::Empty) => break,
                // 应用退出：drop client → daemon 看到 EOF → detach_all（会话继续活）。
                Err(mpsc::TryRecvError::Disconnected) => return,
            }
        }

        // 2) daemon 事件：非阻塞地收到空闲或上限为止。
        for _ in 0..MAX_FRAMES_PER_TICK {
            let next = match runtime.client.as_mut() {
                Some(client) => client.next_event(Duration::ZERO),
                None => break,
            };
            match next {
                Ok(Some(message)) => {
                    handle_message(&mut runtime, message);
                    flush_due(&mut runtime);
                }
                Ok(None) => break,
                Err(error) => {
                    lose_connection(&mut runtime, error.to_string());
                    break;
                }
            }
        }

        // 3) 到窗口的批次冲刷。
        flush_due(&mut runtime);

        // 4) 让出 CPU；命令 / 输入的最坏延迟就是这里。
        std::thread::sleep(POLL_INTERVAL);
    }
}

#[cfg(windows)]
fn handle_request(runtime: &mut Runtime, request: Request) {
    match request {
        Request::Open {
            sid,
            cwd,
            cols,
            rows,
            event,
            reply,
        } => open_session(runtime, sid, cwd, cols, rows, event, reply),
        Request::Input { sid, bytes } => {
            let Some(handle) = runtime.sessions.get(&sid).map(|session| session.handle) else {
                return;
            };
            if let Some(client) = runtime.client.as_mut() {
                if let Err(error) = client.write_input(handle, &bytes) {
                    lose_connection(runtime, error.to_string());
                }
            }
        }
        Request::Resize { sid, cols, rows } => {
            if let Some(client) = runtime.client.as_mut() {
                if let Err(error) = client.resize(&sid, pty_size(cols, rows)) {
                    // resize 失败通常只是会话已退出；别因此丢掉整条连接。
                    eprintln!("[terminal] resize {sid} 失败：{error}");
                }
            }
        }
        Request::Ack { sid, acked_bytes } => {
            let Some(base) = runtime.sessions.get(&sid).map(|session| session.ack_base) else {
                return;
            };
            if let Some(client) = runtime.client.as_mut() {
                if let Err(error) = client.ack(&sid, base.saturating_add(acked_bytes)) {
                    eprintln!("[terminal] ack {sid} 失败：{error}");
                }
            }
        }
        Request::SetVisible { sid, visible } => set_visible(runtime, &sid, visible),
        Request::Close { sid } => {
            if let Some(client) = runtime.client.as_mut() {
                let _ = client.terminate(&sid, false);
            }
            remove_session(runtime, &sid, None);
        }
    }
}

#[cfg(windows)]
fn open_session(
    runtime: &mut Runtime,
    sid: String,
    cwd: String,
    cols: u16,
    rows: u16,
    event: Channel<TerminalEvent>,
    reply: mpsc::SyncSender<Result<TerminalOpened, String>>,
) {
    if runtime.client.is_none() {
        match connect() {
            Ok(client) => runtime.client = Some(client),
            Err(error) => {
                let _ = reply.send(Err(error));
                return;
            }
        }
    }

    let request = ageminal_protocol::CreateSession {
        sid: sid.clone(),
        cwd,
        shell: None,
        command: None,
        cols,
        rows,
        env: Vec::new(),
    };

    let created = match runtime.client.as_mut() {
        Some(client) => client.create_session(request),
        None => unreachable!("连接已在上面建立"),
    };

    match created {
        Ok(created) => {
            runtime.sessions.insert(
                sid.clone(),
                SessionState {
                    handle: created.handle,
                    event,
                    visible: true,
                    batcher: Batcher::default(),
                    last_seq: 0,
                    ack_base: 0,
                },
            );
            runtime.by_handle.insert(created.handle, sid.clone());
            let opened = TerminalOpened {
                sid,
                pid: created.pid,
                incarnation_id: created.incarnation_id,
            };
            // 调用方超时 / 页签已关：别把孤儿会话留在 daemon 里。
            if reply.send(Ok(opened)).is_err() {
                if let Some(client) = runtime.client.as_mut() {
                    let _ = client.terminate(&created.sid, false);
                }
                remove_session(runtime, &created.sid, None);
            }
        }
        Err(error) => {
            // 建会话失败可能是 daemon 断了；让下一次 Open 重连。
            if matches!(error, ageminal_daemon::DaemonError::NotRunning { .. }) {
                lose_connection(runtime, error.to_string());
            }
            let _ = reply.send(Err(error.to_string()));
        }
    }
}

#[cfg(windows)]
fn set_visible(runtime: &mut Runtime, sid: &str, visible: bool) {
    let Some(current) = runtime.sessions.get(sid).map(|session| session.visible) else {
        return;
    };
    if current == visible {
        return;
    }

    if visible {
        if runtime.client.is_none() {
            match connect() {
                Ok(client) => runtime.client = Some(client),
                Err(error) => {
                    send_error(runtime, sid, error);
                    return;
                }
            }
        }
        let last_seq = runtime
            .sessions
            .get(sid)
            .map_or(0, |session| session.last_seq);
        let attached = match runtime.client.as_mut() {
            Some(client) => client.attach(sid, last_seq),
            None => return,
        };
        match attached {
            Ok(attached) => {
                let from_seq = attached.from_seq;
                if let Some(session) = runtime.sessions.get_mut(sid) {
                    if session.handle != attached.handle {
                        runtime.by_handle.remove(&session.handle);
                        runtime.by_handle.insert(attached.handle, sid.to_owned());
                        session.handle = attached.handle;
                    }
                    session.visible = true;
                    session.ack_base = from_seq;
                    session.batcher = Batcher::default();
                    send_control(session, TerminalControl::Attached { from_seq });
                }
            }
            Err(error) => send_error(runtime, sid, error.to_string()),
        }
    } else {
        // 先把已收到的尾巴送出去，再断开——切走前的最后一行提示不该丢。
        if let Some(session) = runtime.sessions.get_mut(sid) {
            if let Some(batch) = session.batcher.flush() {
                let _ = session.event.send(TerminalEvent::Data(batch));
            }
        }
        if let Some(client) = runtime.client.as_mut() {
            if let Err(error) = client.detach(sid) {
                eprintln!("[terminal] detach {sid} 失败：{error}");
            }
        }
        if let Some(session) = runtime.sessions.get_mut(sid) {
            session.visible = false;
            // 断开期间的输出不保留（快照归 #71 / #72）。
            session.batcher = Batcher::default();
        }
    }
}

#[cfg(windows)]
fn handle_message(runtime: &mut Runtime, message: ageminal_protocol::Message) {
    use ageminal_protocol::Message;

    match message {
        Message::Data { handle, seq, bytes } => {
            let Some(sid) = runtime.by_handle.get(&handle).cloned() else {
                return;
            };
            let now = std::time::Instant::now();
            let Some(session) = runtime.sessions.get_mut(&sid) else {
                return;
            };
            session.last_seq = seq.saturating_add(bytes.len() as u64);
            if !session.visible {
                return;
            }
            if let Some(batch) = session.batcher.push(&bytes, now) {
                let _ = session.event.send(TerminalEvent::Data(batch));
            }
        }
        Message::SessionExited(exited) => {
            if let Some(mut session) = runtime.sessions.remove(&exited.sid) {
                runtime.by_handle.remove(&session.handle);
                send_control(
                    &mut session,
                    TerminalControl::Exit {
                        code: exited.exit_code,
                    },
                );
            }
        }
        Message::Error(error) => {
            // 协议里的 `Error` 没有 sid；很可能是某条无响应命令（resize / ack / input）
            // 的失败。不能广播给所有会话，否则一个已退出的会话会让整屏终端都弹错误。
            eprintln!("[terminal] daemon 错误：{error:?}");
        }
        // 请求-响应已经在 Client::request 里消化；这里只忽略心跳等。
        _ => {}
    }
}

#[cfg(windows)]
fn flush_due(runtime: &mut Runtime) {
    let now = std::time::Instant::now();
    for session in runtime.sessions.values_mut() {
        if let Some(batch) = session.batcher.take_if_due(now) {
            let _ = session.event.send(TerminalEvent::Data(batch));
        }
    }
}

/// 发控制消息前先冲刷该会话的待发数据，保证同一条 Channel 内「字节 → 控制」的顺序。
#[cfg(windows)]
fn send_control(session: &mut SessionState, control: TerminalControl) {
    if let Some(batch) = session.batcher.flush() {
        let _ = session.event.send(TerminalEvent::Data(batch));
    }
    let _ = session.event.send(TerminalEvent::Control(control));
}

#[cfg(windows)]
fn send_error(runtime: &mut Runtime, sid: &str, message: String) {
    if let Some(session) = runtime.sessions.get_mut(sid) {
        send_control(session, TerminalControl::Error { message });
    }
}

#[cfg(windows)]
fn remove_session(runtime: &mut Runtime, sid: &str, control: Option<TerminalControl>) {
    if let Some(mut session) = runtime.sessions.remove(sid) {
        runtime.by_handle.remove(&session.handle);
        if let Some(control) = control {
            send_control(&mut session, control);
        }
    }
}

/// daemon 断了：通知所有前端并清空会话表；下一次 Open 再重连。
#[cfg(windows)]
fn lose_connection(runtime: &mut Runtime, reason: String) {
    for (_, mut session) in runtime.sessions.drain() {
        send_control(
            &mut session,
            TerminalControl::Closed {
                reason: reason.clone(),
            },
        );
    }
    runtime.by_handle.clear();
    runtime.client = None;
}

/// 连接 daemon。应用启动时 `daemon::spawn_ensure` 已在后台拉起，这里留 10 s 给它就绪。
#[cfg(windows)]
fn connect() -> Result<ageminal_daemon::client::Client, String> {
    let config = ageminal_daemon::paths::config_dir().map_err(|error| error.to_string())?;
    let deadline = std::time::Instant::now() + CONNECT_TIMEOUT;
    let mut last;
    loop {
        match ageminal_protocol::DaemonInfo::read(&config) {
            Ok(info) => match ageminal_daemon::client::connect(&info, Duration::from_secs(1)) {
                Ok(client) => return Ok(client),
                Err(error) => last = error.to_string(),
            },
            Err(error) => last = error.to_string(),
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!("无法连接 daemon：{last}"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(windows)]
fn pty_size(cols: u16, rows: u16) -> ageminal_daemon::pty::PtySize {
    ageminal_daemon::pty::PtySize { cols, rows }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn batches_flush_at_the_byte_threshold() {
        let mut batcher = Batcher::default();
        let now = Instant::now();
        assert!(batcher.push(&vec![1; 8 * 1024], now).is_none());
        let batch = batcher
            .push(&vec![2; 8 * 1024], now)
            .expect("到 16 KiB 必须立刻取出");
        assert_eq!(batch.len(), BATCH_BYTES);
        assert_eq!(batch[0], 1);
        assert_eq!(batch[8 * 1024], 2);
    }

    #[test]
    fn batches_wait_for_the_time_window() {
        let mut batcher = Batcher::default();
        let start = Instant::now();
        assert!(batcher.push(b"prompt", start).is_none());
        assert!(batcher
            .take_if_due(start + Duration::from_millis(7))
            .is_none());
        let batch = batcher
            .take_if_due(start + BATCH_WINDOW)
            .expect("窗口到了要冲刷");
        assert_eq!(batch, b"prompt");
    }

    #[test]
    fn bytes_are_never_dropped_or_reordered() {
        let mut batcher = Batcher::default();
        let now = Instant::now();
        let mut expected = Vec::new();
        let mut actual = Vec::new();

        for index in 0..1000u32 {
            let chunk = index.to_le_bytes();
            expected.extend_from_slice(&chunk);
            if let Some(batch) = batcher.push(&chunk, now) {
                actual.extend_from_slice(&batch);
            }
        }
        if let Some(batch) = batcher.flush() {
            actual.extend_from_slice(&batch);
        }

        assert_eq!(actual, expected);
    }
}
