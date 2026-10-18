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
//! 切回时用 `Attach(lastSeq = 0)` 取 ANSI 快照刷新屏幕（#73）；精确回放留给 UI 重启 /
//! 显式重连（#75）。数据帧按 `seq` 对账：重复只取尾巴，出现空洞就丢弃并请求 checkpoint（#72）。
//!
//! # 流控（#73）
//!
//! daemon 按未确认字节做 256 / 64 KiB 高低水位；前端在 xterm `write` 回调里每 32 KiB（或空闲
//! 25 ms）回一次累计 ack。高水位时 daemon 暂停读 PTY，背压经 ConPTY 传回子进程。
//!
//! # 关闭语义（#75）
//!
//! - **关闭页签** = 显式 `Terminate`（运行中先二次确认）——它会结束该会话。
//! - **关闭窗口 / 退出 UI** = app-core 进程结束，连接断开 → daemon `detach_all`；会话继续活，
//!   下次启动用 `ListSessions` 找回并 `Attach` 快照。
//! - 重连成功后，前端在回放边界之后写一行「—— 已重新连接 ——」；它由前端直接写进 xterm，
//!   不注入 PTY 字节流，因此不会截断可能落在边界中间的转义序列。
//!
//! # Agent 一等入口（#77）
//!
//! 前端只传 `agentId`；这里从 `settings.json` 解析命令 / 参数并交给 daemon 的
//! `CreateSession.command`。daemon 在 shell 起来后把这一行写进 PTY，所以 `.cmd` / `.exe` /
//! Git Bash 脚本都由 shell 解析，agent 仍留在 shell 的进程树里（#79 的可识别路径）。

use std::path::PathBuf;
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use specta::Type;
use tauri::ipc::{Channel, InvokeResponseBody, IpcResponse};

use crate::agents;
use crate::storage::{LaunchSpec, Store};

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

/// 建会话的等待上限。attach 路径可能先 list 再 attach（daemon 每个请求 20 s 上限），
/// 这里给两次请求留余量。
#[cfg(windows)]
const OPEN_TIMEOUT: Duration = Duration::from_secs(45);

/// 枚举会话的等待上限：连接等待 + daemon 单次请求的超时都要覆盖。
#[cfg(windows)]
const LIST_TIMEOUT: Duration = Duration::from_secs(32);

/// 「结束所有会话并退出」的总等待上限：daemon 排空会话 + 确认进程退出 + 兜底硬杀。
#[cfg(windows)]
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(20);

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
    /// `true` = 本次新建了会话；`false` = attach 了 daemon 里的已有会话。
    pub created: bool,
}

/// 解析后的启动参数：既要交给 daemon，也要写回 `state.json`。
#[derive(Debug, Clone)]
struct LaunchResolution {
    /// 本次实际写入 PTY 的命令。
    command: Option<String>,
    /// 首次启动的命令（resume 时不会把它覆盖成 resume 命令）。
    original_command: Option<String>,
    /// 程序与原始参数串；resume 时重新组合。
    program: Option<String>,
    args: Option<String>,
    /// 一等入口 agent id。
    agent_id: Option<String>,
    /// 已捕获的 agent 对话 id。
    agent_session_id: Option<String>,
}

/// 打开一个终端所需的参数（避免服务内部函数参数太多）。
#[derive(Debug, Clone)]
pub(crate) struct OpenSpec {
    sid: String,
    cwd: String,
    cols: u16,
    rows: u16,
    agent_id: Option<String>,
    command: Option<String>,
}

/// 一个存活终端会话的概要（`terminal_list`，UI 重开恢复用）。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSession {
    /// 逻辑会话 id。
    pub sid: String,
    /// 会话工作目录（用于重建页签）。
    pub cwd: String,
    /// shell 进程 id。
    pub pid: u32,
    /// 本次 spawn 的世代 id。
    pub incarnation_id: String,
    /// 一等入口的 agent id；普通终端为 `None`。
    pub agent_id: Option<String>,
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
    /// `snapshot` 非空时表示无法精确回放，前端要先用它重建屏幕。
    Attached {
        /// 续流起点。
        from_seq: u64,
        /// 实时流起点；`from_seq..replay_end` 是回放。
        replay_end: u64,
        /// ANSI 快照（无法精确回放时）。
        snapshot: Option<String>,
        /// 是否 alt-screen（前端提示 / #75 分隔线用）。
        alt_screen: bool,
        /// 是否是真正的重连；快照路径下前端在快照之后直接写分隔线。
        reconnected: bool,
        /// daemon 快照对应的列数。
        cols: u16,
        /// daemon 快照对应的行数。
        rows: u16,
    },
    /// 回放结束、实时流开始；前端据此显示提示并写流内分隔线。
    ///
    /// 分隔线由前端直接写进 xterm（不在 PTY 字节流里注入），因此不会截断回放边界。
    ReplayEnd {
        /// 是否 alt-screen（alt-screen 下前端不写流内分隔线，只留面板提示）。
        alt_screen: bool,
        /// 是否是真正的重连（新 UI attach / 断档恢复）；普通隐藏页签切回为 `false`，
        /// 不往滚屏历史里塞分隔线。
        reconnected: bool,
    },
    /// 断档恢复：用 checkpoint 快照重建屏幕。
    Snapshot {
        /// ANSI 快照。
        ansi: String,
        /// 快照对应的累计偏移。
        from_seq: u64,
        /// 是否 alt-screen。
        alt_screen: bool,
        /// 断档恢复也是重连；前端在快照之后写分隔线。
        reconnected: bool,
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
    pub(crate) fn open(
        &self,
        spec: OpenSpec,
        event: Channel<TerminalEvent>,
    ) -> Result<TerminalOpened, String> {
        #[cfg(windows)]
        {
            let (reply, receiver) = mpsc::sync_channel(1);
            self.send(Request::Open { spec, event, reply })?;
            match receiver.recv_timeout(OPEN_TIMEOUT) {
                Ok(result) => result,
                Err(_) => Err("终端服务在等待 daemon 建会话时超时".to_owned()),
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (spec, event);
            Err(windows_only())
        }
    }

    /// 枚举 daemon 中仍然存活的会话（UI 重开恢复，#75）。
    pub fn list(&self) -> Result<Vec<TerminalSession>, String> {
        #[cfg(windows)]
        {
            let (reply, receiver) = mpsc::sync_channel(1);
            self.send(Request::List { reply })?;
            match receiver.recv_timeout(LIST_TIMEOUT) {
                Ok(result) => result,
                Err(_) => Err("终端服务在等待 daemon 枚举会话时超时".to_owned()),
            }
        }
        #[cfg(not(windows))]
        {
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

    /// 结束 daemon 里**所有**会话并等 daemon 退出（#76 托盘的唯一真正结束入口）。
    pub fn shutdown_all(&self) -> Result<(), String> {
        #[cfg(windows)]
        {
            let (reply, receiver) = mpsc::sync_channel(1);
            self.send(Request::ShutdownAll { reply })?;
            match receiver.recv_timeout(SHUTDOWN_TIMEOUT) {
                Ok(result) => result,
                Err(_) => Err("终端服务在等待 daemon 停机时超时".to_owned()),
            }
        }
        #[cfg(not(windows))]
        {
            Ok(())
        }
    }

    /// 显式结束会话（关闭页签）；关窗 / 退出 UI 不走这里，连接断开只 detach。
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

/// 从持久化 launch spec / 当前设置解析本次启动参数。
///
/// 已有 spec 的 terminal 页签在 daemon 死后重建时，**优先使用首次启动参数**；如果
/// `agentSessionId` 已捕获，则改写成对应 CLI 的 resume 命令。新页签才读当前设置。
fn resolve_launch(
    store: &Store,
    sid: &str,
    agent_id: Option<&str>,
) -> Result<LaunchResolution, String> {
    let saved = store
        .state()
        .launch_specs
        .iter()
        .find(|spec| spec.sid == sid)
        .cloned();
    let Some(saved) = saved else {
        let Some(id) = agent_id else {
            return Ok(LaunchResolution {
                command: None,
                original_command: None,
                program: None,
                args: None,
                agent_id: None,
                agent_session_id: None,
            });
        };
        let config =
            agents::resolve(store.settings(), id).ok_or_else(|| format!("未知的 agent：{id}"))?;
        let command = agents::launch_command(store.settings(), id)?;
        return Ok(LaunchResolution {
            command: Some(command.clone()),
            original_command: Some(command),
            program: Some(config.command),
            args: Some(config.args),
            agent_id: Some(id.to_owned()),
            agent_session_id: None,
        });
    };

    let agent = agent_id
        .map(str::to_owned)
        .or_else(|| saved.agent_id.clone());
    let config = agent
        .as_deref()
        .and_then(|id| agents::resolve(store.settings(), id));
    let program = saved
        .program
        .clone()
        .or_else(|| config.as_ref().map(|config| config.command.clone()));
    let args = saved
        .args
        .clone()
        .or_else(|| config.as_ref().map(|config| config.args.clone()));
    let fresh_command = match agent.as_deref() {
        Some(id) => agents::launch_command(store.settings(), id).ok(),
        None => None,
    };
    let resume = match (agent.as_deref(), saved.agent_session_id.as_deref()) {
        (Some(id), Some(session_id)) => {
            match agents::resume_command(
                store.settings(),
                id,
                session_id,
                program.as_deref(),
                args.as_deref(),
            ) {
                Ok(command) => Some(command),
                Err(error) => {
                    eprintln!("[terminal] resume 命令生成失败，回退重建：{error}");
                    None
                }
            }
        }
        _ => None,
    };
    let command = resume
        .or_else(|| saved.command.clone())
        .or_else(|| fresh_command.clone());
    let original_command = saved.command.clone().or_else(|| {
        if saved.agent_session_id.is_none() {
            fresh_command
        } else {
            None
        }
    });

    Ok(LaunchResolution {
        command,
        original_command,
        program,
        args,
        agent_id: agent,
        agent_session_id: saved.agent_session_id.clone(),
    })
}

/// 建会话前先写占位 launch spec（没有 incarnation）；失败也不阻止建会话。
fn ensure_launch_spec(store: &Mutex<Store>, sid: &str, cwd: &str, resolution: &LaunchResolution) {
    let Ok(mut store) = store.lock() else {
        return;
    };
    let now = crate::now_ms();
    let result = store.update_state(
        |state| {
            if state.launch_specs.iter().any(|spec| spec.sid == sid) {
                return;
            }
            state.launch_specs.push(LaunchSpec {
                sid: sid.to_owned(),
                cwd: PathBuf::from(cwd),
                shell: None,
                command: resolution.original_command.clone(),
                program: resolution.program.clone(),
                args: resolution.args.clone(),
                agent_id: resolution.agent_id.clone(),
                agent_session_id: resolution.agent_session_id.clone(),
                incarnation_id: None,
                updated_at_ms: Some(now),
                extra: Default::default(),
            });
        },
        Instant::now(),
    );
    if let Err(error) = result {
        eprintln!("[terminal] 写入占位 launch spec 失败：{error}");
    }
}

/// 把本次 spawn 的 launch spec / 世代写回 `state.json`（去抖落盘）。
fn record_launch(
    store: &Mutex<Store>,
    sid: &str,
    cwd: &str,
    resolution: &LaunchResolution,
    opened: &TerminalOpened,
) {
    let Ok(mut store) = store.lock() else {
        eprintln!("[terminal] 状态锁已被污染，跳过 launch spec 写入");
        return;
    };
    let now = crate::now_ms();
    let result = store.update_state(
        |state| {
            let existing = state.launch_specs.iter().position(|spec| spec.sid == sid);
            let previous = existing
                .and_then(|index| state.launch_specs.get(index))
                .cloned();
            let extra = previous
                .as_ref()
                .map(|spec| spec.extra.clone())
                .unwrap_or_default();
            if opened.created {
                let spec = LaunchSpec {
                    sid: sid.to_owned(),
                    cwd: PathBuf::from(cwd),
                    shell: None,
                    command: resolution.original_command.clone(),
                    program: resolution.program.clone(),
                    args: resolution.args.clone(),
                    agent_id: resolution.agent_id.clone(),
                    // 事件可能在建会话返回前就回写了 id（#79），不能被 None 覆盖。
                    agent_session_id: resolution
                        .agent_session_id
                        .clone()
                        .or_else(|| previous.and_then(|spec| spec.agent_session_id)),
                    incarnation_id: Some(opened.incarnation_id.clone()),
                    updated_at_ms: Some(now),
                    extra,
                };
                if let Some(index) = existing {
                    state.launch_specs[index] = spec;
                } else {
                    state.launch_specs.push(spec);
                }
            } else if let Some(index) = existing {
                let spec = &mut state.launch_specs[index];
                spec.incarnation_id = Some(opened.incarnation_id.clone());
                spec.cwd = PathBuf::from(cwd);
                spec.updated_at_ms = Some(now);
                if spec.agent_id.is_none() {
                    spec.agent_id = resolution.agent_id.clone();
                }
            } else {
                // 孤儿会话：没有历史 launch spec，至少记下 cwd / agent / 世代。
                state.launch_specs.push(LaunchSpec {
                    sid: sid.to_owned(),
                    cwd: PathBuf::from(cwd),
                    shell: None,
                    command: None,
                    program: None,
                    args: None,
                    agent_id: resolution.agent_id.clone(),
                    agent_session_id: None,
                    incarnation_id: Some(opened.incarnation_id.clone()),
                    updated_at_ms: Some(now),
                    extra,
                });
            }
        },
        Instant::now(),
    );
    if let Err(error) = result {
        eprintln!("[terminal] 写入 launch spec 失败：{error}");
    }
}

/// Tauri 命令：建会话。`async` + `spawn_blocking`，建会话期间不占住 UI 线程。
#[tauri::command]
#[specta::specta]
// Tauri 的命令面就是扁平参数；内部用 `OpenSpec` 降低服务函数参数。
#[allow(clippy::too_many_arguments)]
pub async fn terminal_open(
    state: tauri::State<'_, TerminalService>,
    store: tauri::State<'_, Mutex<Store>>,
    sid: String,
    cwd: String,
    cols: u16,
    rows: u16,
    agent_id: Option<String>,
    on_event: Channel<TerminalEvent>,
) -> Result<TerminalOpened, String> {
    // launch spec 是恢复的权威：已有 spec 时优先用它（agent 有 agentSessionId 则走 resume），
    // 新页签才从当前设置解析。
    let resolution = {
        let store = store.lock().map_err(|_| "设置锁已被污染".to_owned())?;
        resolve_launch(&store, &sid, agent_id.as_deref())?
    };
    // 先落一个占位 spec：daemon 写完启动命令就可能触发 agent 事件（#79），
    // `terminal_set_agent_session` 不能因为 record_launch 还没执行而丢 id。
    ensure_launch_spec(store.inner(), &sid, &cwd, &resolution);
    let spec = OpenSpec {
        sid: sid.clone(),
        cwd: cwd.clone(),
        cols,
        rows,
        agent_id: resolution.agent_id.clone(),
        command: resolution.command.clone(),
    };
    let service = state.inner().clone();
    let opened = tauri::async_runtime::spawn_blocking(move || service.open(spec, on_event))
        .await
        .map_err(|error| format!("终端服务不可用：{error}"))??;
    record_launch(store.inner(), &sid, &cwd, &resolution, &opened);
    Ok(opened)
}

/// Tauri 命令：枚举存活会话（UI 重开时恢复终端页签，#75）。
#[tauri::command]
#[specta::specta]
pub async fn terminal_list(
    state: tauri::State<'_, TerminalService>,
) -> Result<Vec<TerminalSession>, String> {
    let service = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || service.list())
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
///
/// 同时删掉持久化的 launch spec 与页签骨架：用户关页签是明确意图，不能下次启动又被
/// 对账成「仅 UI 有 → 重建」。前端稍后的 `save_tabs` 会再覆盖一次，二者幂等。
#[tauri::command]
#[specta::specta]
pub fn terminal_close(
    state: tauri::State<'_, TerminalService>,
    store: tauri::State<'_, Mutex<Store>>,
    sid: String,
) -> Result<(), String> {
    let result = state.inner().close(sid.clone());
    if result.is_ok() {
        if let Ok(mut store) = store.lock() {
            let removed = store.update_state(
                |state| {
                    state.launch_specs.retain(|spec| spec.sid != sid);
                    for group in &mut state.tabs {
                        group
                            .tabs
                            .retain(|tab| tab.sid.as_deref() != Some(sid.as_str()));
                        if group.active.as_deref() == Some(sid.as_str()) {
                            group.active = None;
                        }
                    }
                    state.tabs.retain(|group| !group.tabs.is_empty());
                },
                Instant::now(),
            );
            if let Err(error) = removed {
                eprintln!("[terminal] 清理关闭页签的持久状态失败：{error}");
            }
        }
    }
    result
}

/// 发往服务线程的请求。
#[cfg(windows)]
enum Request {
    Open {
        spec: OpenSpec,
        event: Channel<TerminalEvent>,
        reply: mpsc::SyncSender<Result<TerminalOpened, String>>,
    },
    List {
        reply: mpsc::SyncSender<Result<Vec<TerminalSession>, String>>,
    },
    ShutdownAll {
        reply: mpsc::SyncSender<Result<(), String>>,
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
    /// 回放结束位置；`replay_done == false` 时收到的数据属于回放。
    replay_end: u64,
    replay_done: bool,
    /// 已请求 checkpoint，等 `Snapshot` 期间丢弃所有数据帧。
    awaiting_checkpoint: bool,
    /// 当前屏幕是否 alt-screen（前端提示用）。
    alt_screen: bool,
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
        // 没有会话时阻塞等命令，别每 2 ms 空转。`terminal_list` 会在启动时先连上 daemon，
        // 但列表本身没有需要实时消费的事件；等用户真正打开页签时再进轮询。
        if runtime.sessions.is_empty() {
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
        Request::Open { spec, event, reply } => open_session(runtime, spec, event, reply),
        Request::List { reply } => list_sessions(runtime, reply),
        Request::ShutdownAll { reply } => shutdown_all(runtime, reply),
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

/// 结束所有会话并让 daemon 退出；正常走协议，失败时在**确认 daemon 在线**后硬杀兜底。
#[cfg(windows)]
fn shutdown_all(runtime: &mut Runtime, reply: mpsc::SyncSender<Result<(), String>>) {
    let config = match ageminal_daemon::paths::config_dir() {
        Ok(config) => config,
        Err(error) => {
            let _ = reply.send(Err(error.to_string()));
            return;
        }
    };
    let info = ageminal_protocol::DaemonInfo::read(&config).ok();

    // 平时启动时的 terminal_list 已经连上；万一没有，先短连接，别为此等 10 s。
    let mut failure = None;
    if runtime.client.is_none() {
        if let Some(info) = info.as_ref() {
            match ageminal_daemon::client::connect(info, Duration::from_secs(1)) {
                Ok(client) => runtime.client = Some(client),
                Err(error) => {
                    eprintln!("[terminal] 停机前连接 daemon 失败：{error}");
                    failure = Some(error.to_string());
                }
            }
        }
    }

    let had_client = runtime.client.is_some();
    if let Some(client) = runtime.client.as_mut() {
        if let Err(error) = client.shutdown() {
            failure = Some(error.to_string());
        }
    }
    // daemon 在回 ack 后等客户端断开；先 drop，让它的 read_loop 看到 EOF 并退出。
    runtime.client = None;

    // 只有确实连上过 daemon 才碰它的 pid：daemon.json 可能是旧世代，不能误杀复用了 pid 的进程。
    if had_client {
        if let Some(info) = info.as_ref() {
            let mut stopped = ageminal_daemon::launch::wait_stopped(info, Duration::from_secs(5));
            if !stopped {
                eprintln!("[terminal] daemon 未在停机确认后退出，执行兜底终止");
                let _ = ageminal_daemon::launch::terminate_process(info);
                stopped = ageminal_daemon::launch::wait_stopped(info, Duration::from_secs(3));
                if stopped {
                    let _ = ageminal_protocol::DaemonInfo::remove(&config);
                }
            }
            if stopped {
                // 即使 shutdown 请求本身报错，只要 daemon 确实停了，这次停机就是成功的。
                failure = None;
            } else {
                failure = Some("daemon 在兜底终止后仍未退出".to_owned());
            }
        }
    }

    runtime.sessions.clear();
    runtime.by_handle.clear();
    let _ = reply.send(match failure {
        Some(error) => Err(error),
        None => Ok(()),
    });
}

#[cfg(windows)]
fn ensure_client(runtime: &mut Runtime) -> Result<(), String> {
    if runtime.client.is_some() {
        return Ok(());
    }
    match connect() {
        Ok(client) => {
            runtime.client = Some(client);
            Ok(())
        }
        Err(error) => Err(error),
    }
}

/// 枚举存活会话（`terminal_list`）。
#[cfg(windows)]
fn list_sessions(
    runtime: &mut Runtime,
    reply: mpsc::SyncSender<Result<Vec<TerminalSession>, String>>,
) {
    if let Err(error) = ensure_client(runtime) {
        let _ = reply.send(Err(error));
        return;
    }
    let result = runtime.client.as_mut().expect("连接已建立").list_sessions();
    match result {
        Ok(sessions) => {
            let _ = reply.send(Ok(sessions
                .into_iter()
                .map(|info| TerminalSession {
                    sid: info.sid,
                    cwd: info.cwd,
                    pid: info.pid,
                    incarnation_id: info.incarnation_id,
                    agent_id: info.agent_id,
                })
                .collect()));
        }
        Err(error) => {
            if matches!(error, ageminal_daemon::DaemonError::NotRunning { .. }) {
                lose_connection(runtime, error.to_string());
            }
            let _ = reply.send(Err(error.to_string()));
        }
    }
}

#[cfg(windows)]
fn open_session(
    runtime: &mut Runtime,
    spec: OpenSpec,
    event: Channel<TerminalEvent>,
    reply: mpsc::SyncSender<Result<TerminalOpened, String>>,
) {
    let OpenSpec {
        sid,
        cwd,
        cols,
        rows,
        agent_id,
        command,
    } = spec;
    if let Err(error) = ensure_client(runtime) {
        let _ = reply.send(Err(error));
        return;
    }

    // 先看 daemon 里是否已有这个 sid：UI 进程重启 / WebView reload 都必须走 attach，
    // 不能因为本地 SessionState 没了就新建一个把旧 shell 挤掉。
    let existing = match runtime.client.as_mut().expect("连接已建立").list_sessions() {
        Ok(sessions) => sessions.into_iter().find(|info| info.sid == sid),
        Err(error) => {
            if matches!(error, ageminal_daemon::DaemonError::NotRunning { .. }) {
                lose_connection(runtime, error.to_string());
            }
            let _ = reply.send(Err(error.to_string()));
            return;
        }
    };

    if let Some(info) = existing {
        match attach_existing(runtime, &sid, &info, event, cols, rows) {
            Ok(opened) => {
                if reply.send(Ok(opened)).is_err() {
                    // 调用方已走（页签在打开过程中被关）：只 detach，别杀这个已有会话。
                    if let Some(client) = runtime.client.as_mut() {
                        let _ = client.detach(&sid);
                    }
                    remove_session(runtime, &sid, None);
                }
            }
            Err(error) => {
                if matches!(error, ageminal_daemon::DaemonError::NotRunning { .. }) {
                    lose_connection(runtime, error.to_string());
                }
                let _ = reply.send(Err(error.to_string()));
            }
        }
        return;
    }

    let request = ageminal_protocol::CreateSession {
        sid: sid.clone(),
        cwd,
        shell: None,
        command,
        agent_id,
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
                    replay_end: 0,
                    replay_done: true,
                    awaiting_checkpoint: false,
                    alt_screen: false,
                },
            );
            runtime.by_handle.insert(created.handle, sid.clone());
            let opened = TerminalOpened {
                sid,
                pid: created.pid,
                incarnation_id: created.incarnation_id,
                created: true,
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

/// 把一条已有 daemon 会话接回新的 Channel（UI 重开 / WebView reload）。
///
/// 新 UI 没有可信的 `lastSeq`，一律 `Attach(sid, 0)` 取当前 ANSI 快照；实时流从快照的
/// `seq` 继续。快照路径直接在 `Attached{reconnected:true}` 里带上标记，前端写完快照后
/// 立刻写分隔线；只有有回放时才等 `ReplayEnd` 到边界再写。
#[cfg(windows)]
fn attach_existing(
    runtime: &mut Runtime,
    sid: &str,
    info: &ageminal_protocol::SessionInfo,
    event: Channel<TerminalEvent>,
    cols: u16,
    rows: u16,
) -> Result<TerminalOpened, ageminal_daemon::DaemonError> {
    let client = runtime.client.as_mut().expect("连接已建立");
    // 先按新面板尺寸 resize，再取快照：否则 attach 之后的 resize 会让 ConPTY 重绘，
    // 把刚写进 xterm 的「已重新连接」分隔线清掉。
    if info.cols != cols || info.rows != rows {
        client.resize(sid, pty_size(cols, rows))?;
    }
    let attached = client.attach(sid, 0)?;
    let from_seq = attached.from_seq;
    let replay_end = attached.replay_end;
    let alt_screen = attached.alt_screen;
    let has_snapshot = attached.snapshot.is_some();

    let state = SessionState {
        handle: attached.handle,
        event,
        visible: true,
        batcher: Batcher::default(),
        last_seq: from_seq,
        ack_base: from_seq,
        replay_end,
        replay_done: has_snapshot || replay_end <= from_seq,
        awaiting_checkpoint: false,
        alt_screen,
    };
    if let Some(old) = runtime.sessions.insert(sid.to_owned(), state) {
        runtime.by_handle.remove(&old.handle);
    }
    runtime.by_handle.insert(attached.handle, sid.to_owned());

    let session = runtime.sessions.get_mut(sid).expect("刚插入");
    send_control(
        session,
        TerminalControl::Attached {
            from_seq,
            replay_end,
            snapshot: attached.snapshot,
            alt_screen,
            reconnected: true,
            cols: attached.cols,
            rows: attached.rows,
        },
    );
    // 快照路径已在 Attached 里带上重连标记；有回放时等 forward_data 到边界再发 ReplayEnd。

    Ok(TerminalOpened {
        sid: sid.to_owned(),
        pid: info.pid,
        incarnation_id: info.incarnation_id.clone(),
        created: false,
    })
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
        // #73：隐藏页签切回一律用快照刷新（`lastSeq = 0`），不在可见性切换时回放
        // 整段隐藏输出；精确回放留给 UI 重启 / 显式重连（#75）。
        let attached = match runtime.client.as_mut() {
            Some(client) => client.attach(sid, 0),
            None => return,
        };
        match attached {
            Ok(attached) => {
                let from_seq = attached.from_seq;
                let replay_end = attached.replay_end;
                let alt_screen = attached.alt_screen;
                let cols = attached.cols;
                let rows = attached.rows;
                let has_snapshot = attached.snapshot.is_some();
                if let Some(session) = runtime.sessions.get_mut(sid) {
                    if session.handle != attached.handle {
                        runtime.by_handle.remove(&session.handle);
                        runtime.by_handle.insert(attached.handle, sid.to_owned());
                        session.handle = attached.handle;
                    }
                    session.visible = true;
                    session.ack_base = from_seq;
                    session.last_seq = from_seq;
                    session.replay_end = replay_end;
                    session.replay_done = has_snapshot || replay_end <= from_seq;
                    session.awaiting_checkpoint = false;
                    session.alt_screen = alt_screen;
                    session.batcher = Batcher::default();
                    send_control(
                        session,
                        TerminalControl::Attached {
                            from_seq,
                            replay_end,
                            snapshot: attached.snapshot,
                            alt_screen,
                            reconnected: false,
                            cols,
                            rows,
                        },
                    );
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
            // 断开期间的输出留在 daemon 环形缓冲里；切回时取快照刷新（#73）。
            session.batcher = Batcher::default();
        }
    }
}

/// 一条数据帧相对当前期望偏移的分类。
#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DataClass {
    /// 整帧都已消费。
    Drop,
    /// 当前偏移之后的前若干字节还没消费（重叠尾巴）。
    Tail(usize),
    /// 正好接上。
    Fresh,
    /// 中间有洞。
    Gap,
}

/// 纯函数：`expected` = 下一个期望字节的绝对偏移。
#[cfg(windows)]
fn classify_data(expected: u64, seq: u64, len: usize) -> DataClass {
    if len == 0 {
        return DataClass::Drop;
    }
    let end = seq.saturating_add(len as u64);
    if end <= expected {
        DataClass::Drop
    } else if seq < expected {
        DataClass::Tail((expected - seq) as usize)
    } else if seq == expected {
        DataClass::Fresh
    } else {
        DataClass::Gap
    }
}

/// 处理一条数据帧：去重 / 断档检测 / 回放与实时的边界。
#[cfg(windows)]
fn handle_data(runtime: &mut Runtime, handle: u32, seq: u64, mut bytes: Vec<u8>) {
    let Some(sid) = runtime.by_handle.get(&handle).cloned() else {
        return;
    };

    let mut request_checkpoint = false;
    {
        let Some(session) = runtime.sessions.get_mut(&sid) else {
            return;
        };
        if !session.visible || session.awaiting_checkpoint {
            return;
        }

        match classify_data(session.last_seq, seq, bytes.len()) {
            DataClass::Drop => return,
            DataClass::Gap => {
                // 洞：绝不把半截流画到屏幕上，等 checkpoint 快照重建。
                session.awaiting_checkpoint = true;
                session.batcher = Batcher::default();
                request_checkpoint = true;
            }
            DataClass::Tail(overlap) => {
                // 回放过 / 旧连接留下的重叠帧：只转发尚未消费的尾巴。
                bytes.drain(..overlap);
            }
            DataClass::Fresh => {}
        }

        if !request_checkpoint {
            forward_data(session, &mut bytes);
        }
    }

    if request_checkpoint {
        if let Some(client) = runtime.client.as_mut() {
            if let Err(error) = client.checkpoint(&sid) {
                lose_connection(runtime, error.to_string());
            }
        }
    }
}

/// 把一段数据按回放 / 实时边界切开，然后交给合批器。
#[cfg(windows)]
fn forward_data(session: &mut SessionState, bytes: &mut Vec<u8>) {
    if !session.replay_done {
        let remaining = session.replay_end.saturating_sub(session.last_seq) as usize;
        if remaining == 0 {
            send_control(
                session,
                TerminalControl::ReplayEnd {
                    alt_screen: session.alt_screen,
                    reconnected: true,
                },
            );
            session.replay_done = true;
        } else if bytes.len() > remaining {
            let live = bytes.split_off(remaining);
            push_data(session, bytes);
            session.last_seq += remaining as u64;
            // 先冲刷回放尾巴，再发分隔控制，最后把实时部分塞进新的批次。
            send_control(
                session,
                TerminalControl::ReplayEnd {
                    alt_screen: session.alt_screen,
                    reconnected: true,
                },
            );
            session.replay_done = true;
            if !live.is_empty() {
                push_data(session, &live);
                session.last_seq += live.len() as u64;
            }
            return;
        }
    }

    push_data(session, bytes);
    session.last_seq += bytes.len() as u64;
    if !session.replay_done && session.last_seq == session.replay_end {
        send_control(
            session,
            TerminalControl::ReplayEnd {
                alt_screen: session.alt_screen,
                reconnected: true,
            },
        );
        session.replay_done = true;
    }
}

#[cfg(windows)]
fn push_data(session: &mut SessionState, bytes: &[u8]) {
    if let Some(batch) = session.batcher.push(bytes, std::time::Instant::now()) {
        let _ = session.event.send(TerminalEvent::Data(batch));
    }
}

#[cfg(windows)]
fn handle_message(runtime: &mut Runtime, message: ageminal_protocol::Message) {
    use ageminal_protocol::Message;

    match message {
        Message::Data { handle, seq, bytes } => {
            handle_data(runtime, handle, seq, bytes);
        }
        Message::Snapshot(snapshot) => {
            let Some(session) = runtime.sessions.get_mut(&snapshot.sid) else {
                return;
            };
            if !session.visible {
                return;
            }
            // 只接受当前这次断档请求对应的 checkpoint：attach 之后迟到的旧快照
            // （或从未请求过的快照）不能把已经对齐的进度往回拽。
            if !session.awaiting_checkpoint || snapshot.seq < session.last_seq {
                return;
            }
            // checkpoint 是新的绝对锚点：丢掉旧批次与旧进度，之后的实时数据从 seq 接。
            session.last_seq = snapshot.seq;
            session.ack_base = snapshot.seq;
            session.replay_end = snapshot.seq;
            session.replay_done = true;
            session.awaiting_checkpoint = false;
            session.alt_screen = snapshot.alt_screen;
            session.batcher = Batcher::default();
            send_control(
                session,
                TerminalControl::Snapshot {
                    ansi: snapshot.ansi,
                    from_seq: snapshot.seq,
                    alt_screen: snapshot.alt_screen,
                    reconnected: true,
                },
            );
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
    use std::sync::Arc;
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

    fn test_session(
        replay_end: u64,
    ) -> (SessionState, Arc<std::sync::Mutex<Vec<InvokeResponseBody>>>) {
        let messages = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = messages.clone();
        let event = Channel::new(move |body| {
            sink.lock().expect("messages").push(body);
            Ok(())
        });
        (
            SessionState {
                handle: 1,
                event,
                visible: true,
                batcher: Batcher::default(),
                last_seq: 0,
                ack_base: 0,
                replay_end,
                replay_done: replay_end == 0,
                awaiting_checkpoint: false,
                alt_screen: false,
            },
            messages,
        )
    }

    fn flush_for_test(session: &mut SessionState) {
        if let Some(batch) = session.batcher.flush() {
            session.event.send(TerminalEvent::Data(batch)).unwrap();
        }
    }

    fn raw_bytes(body: &InvokeResponseBody) -> Option<Vec<u8>> {
        match body {
            InvokeResponseBody::Raw(bytes) => Some(bytes.clone()),
            InvokeResponseBody::Json(_) => None,
        }
    }

    fn control_type(body: &InvokeResponseBody) -> Option<String> {
        match body {
            InvokeResponseBody::Json(json) => {
                let value: serde_json::Value = serde_json::from_str(json).unwrap();
                Some(value.get("type")?.as_str()?.to_owned())
            }
            InvokeResponseBody::Raw(_) => None,
        }
    }

    /// #72 验收 1 的 app-core 半：回放尾巴先冲刷，随后才是 ReplayEnd，实时数据另起批次。
    #[test]
    fn replay_boundary_is_flushed_before_live_data() {
        let (mut session, messages) = test_session(3);
        let mut bytes = b"abcdef".to_vec();

        forward_data(&mut session, &mut bytes);
        flush_for_test(&mut session);

        let messages = messages.lock().expect("messages");
        assert_eq!(messages.len(), 3);
        assert_eq!(raw_bytes(&messages[0]).as_deref(), Some(b"abc".as_slice()));
        assert_eq!(control_type(&messages[1]).as_deref(), Some("replayEnd"));
        assert_eq!(raw_bytes(&messages[2]).as_deref(), Some(b"def".as_slice()));
        assert_eq!(session.last_seq, 6);
        assert!(session.replay_done);
    }

    /// 整帧刚好落在回放边界：先发字节，再发边界控制。
    #[test]
    fn replay_boundary_at_frame_end_still_marks_the_seam() {
        let (mut session, messages) = test_session(4);
        let mut bytes = b"wxyz".to_vec();

        forward_data(&mut session, &mut bytes);

        let messages = messages.lock().expect("messages");
        assert_eq!(raw_bytes(&messages[0]).as_deref(), Some(b"wxyz".as_slice()));
        assert_eq!(control_type(&messages[1]).as_deref(), Some("replayEnd"));
    }

    /// 没有回放时不该凭空插入边界控制。
    #[test]
    fn live_data_without_replay_has_no_seam_control() {
        let (mut session, messages) = test_session(0);
        let mut bytes = b"live".to_vec();

        forward_data(&mut session, &mut bytes);
        flush_for_test(&mut session);

        let messages = messages.lock().expect("messages");
        assert_eq!(raw_bytes(&messages[0]).as_deref(), Some(b"live".as_slice()));
        assert_eq!(messages.len(), 1);
    }

    fn runtime_with_session(
        last_seq: u64,
    ) -> (Runtime, u32, Arc<std::sync::Mutex<Vec<InvokeResponseBody>>>) {
        let (mut session, messages) = test_session(0);
        session.last_seq = last_seq;
        let mut sessions = std::collections::HashMap::new();
        sessions.insert("s".to_owned(), session);
        let mut by_handle = std::collections::HashMap::new();
        by_handle.insert(1, "s".to_owned());
        (
            Runtime {
                client: None,
                sessions,
                by_handle,
            },
            1,
            messages,
        )
    }

    /// #72 验收 2：洞出现时丢弃数据并进入「等 checkpoint」；Snapshot 到达后从新锚点继续。
    #[test]
    fn a_gap_waits_for_a_checkpoint_and_snapshot_resets_the_offset() {
        let (mut runtime, handle, messages) = runtime_with_session(10);

        handle_data(&mut runtime, handle, 20, b"discard-me".to_vec());
        {
            let session = runtime.sessions.get("s").expect("session");
            assert!(session.awaiting_checkpoint);
            assert_eq!(session.last_seq, 10, "断档帧不能推进偏移");
            assert!(messages.lock().expect("messages").is_empty());
        }

        handle_message(
            &mut runtime,
            ageminal_protocol::Message::Snapshot(ageminal_protocol::Snapshot {
                sid: "s".to_owned(),
                seq: 30,
                ansi: "SCREEN".to_owned(),
                alt_screen: false,
            }),
        );

        let session = runtime.sessions.get("s").expect("session");
        assert!(!session.awaiting_checkpoint);
        assert_eq!(session.last_seq, 30);
        assert!(session.replay_done);
        let messages = messages.lock().expect("messages");
        assert_eq!(control_type(&messages[0]).as_deref(), Some("snapshot"));
        assert_eq!(
            messages.len(),
            1,
            "checkpoint 快照已自带重连标记，不再另发 ReplayEnd"
        );
    }

    #[test]
    fn data_frames_are_classified_against_the_expected_offset() {
        assert_eq!(classify_data(10, 10, 5), DataClass::Fresh);
        assert_eq!(classify_data(10, 8, 5), DataClass::Tail(2));
        assert_eq!(classify_data(10, 8, 2), DataClass::Drop);
        assert_eq!(classify_data(10, 12, 5), DataClass::Gap);
        assert_eq!(classify_data(10, 10, 0), DataClass::Drop);
        assert_eq!(
            classify_data(u64::MAX - 1, u64::MAX, 4),
            DataClass::Gap,
            "溢出不能 panic"
        );
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
