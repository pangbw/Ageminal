//! 控制消息的类型与**版本协商**（研究稿 §3.4 / §6）。
//!
//! 两条规则先说清楚：
//!
//! 1. **控制消息一律走 JSON 负载**，帧头的 `handle` / `seq` **恒为 0**——帧是自描述的，不依赖
//!    任何句柄状态；逻辑 `sid`（字符串）就写在 JSON 里。
//! 2. **只有原始字节帧**（[`Message::Data`] / [`Message::Input`]）才用帧头承载**连接内会话句柄**
//!    与 `seq`——热路径不背 JSON 的包袱。句柄由 daemon 分配（见 [`SessionCreated`]）。
//!
//! > 与研究稿 §3.4 的一处**有意偏离**：那里 `Ack` / `Resize` 等也带 `sessionId`。这里让控制帧
//! > 一律用 `handle = 0` + JSON 里的 `sid`，好处是**每类帧只有一个事实来源**（不会出现「帧头
//! > 句柄」与「payload sid」不一致），代价是高频 `Ack` 要多一次 JSON 解析与 sid 查表——量级上
//! > 是微秒级，可接受。
//!
//! 版本协商（研究稿 §6.1）：客户端在 [`Hello`] 里给**区间** `protoMin..protoMax` + 能力位，
//! daemon 取交集最高版本；**没有交集就明确报错**（[`ErrorCode::UnsupportedVersion`]），
//! 而不是像 Contour 那样静默拒绝（本地单用户，可排障性更值钱）。

use serde::{Deserialize, Serialize};

use crate::frame::{Frame, FrameKind};

/// 当前协议版本（支持区间的**上界**）。
pub const PROTOCOL_VERSION: u16 = 1;

/// 支持区间的**下界**。
pub const PROTOCOL_VERSION_MIN: u16 = 1;

/// 能力位。
///
/// 与 [`crate::frame::Flags`] 同形——newtype 包住位图，不裸用 `u32`（免得能力位和别的整数混着传）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Caps(u32);

impl Caps {
    /// 数据帧原始二进制负载。
    pub const BINARY_PAYLOAD: u32 = 1 << 0;
    /// agent 状态机事件（#79）。
    pub const AGENT_EVENTS: u32 = 1 << 1;
    /// 负载压缩（v1 预留，恒不开）。
    pub const COMPRESSION: u32 = 1 << 2;

    /// 本实现支持的能力。
    pub fn supported() -> Self {
        Self(Self::BINARY_PAYLOAD)
    }

    /// 原始位。
    pub fn bits(self) -> u32 {
        self.0
    }

    /// 是否置了某个（或某几个）位。
    pub fn contains(self, bits: u32) -> bool {
        self.0 & bits == bits
    }

    /// 双方能力的**交集**——只有交集中的能力才真正生效。
    pub fn intersect(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
}

/// 版本协商结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Negotiation {
    /// 双方区间的最高交集版本。
    Agreed(u16),
    /// 区间不相交——要明确拒绝，不能装作没事。
    NoOverlap,
}

/// 取「客户端区间 ∩ daemon 区间」的最高版本。
pub fn negotiate(client_min: u16, client_max: u16) -> Negotiation {
    let min = client_min.max(PROTOCOL_VERSION_MIN);
    let max = client_max.min(PROTOCOL_VERSION);
    if min <= max {
        Negotiation::Agreed(max)
    } else {
        Negotiation::NoOverlap
    }
}

/// 握手：UI→D。连接后的**第一条**消息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    /// 客户端支持的版本区间下界。
    pub proto_min: u16,
    /// 客户端支持的版本区间上界。
    pub proto_max: u16,
    /// 能力位（[`Caps`]）。
    pub caps: Caps,
    /// 握手令牌（`AGEMINAL_TOKEN`；daemon 每次启动重生成）。
    pub token: String,
    /// 客户端实例 id（diagnostics 与「同一 UI 的多次连接」识别）。
    pub client_instance_id: String,
    /// 想接回的会话及各自已消费到的位置（研究稿 §6.4）。
    #[serde(default)]
    pub resume: Vec<Resume>,
}

/// [`Hello::resume`] 里的一项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resume {
    /// 逻辑会话 id。
    pub sid: String,
    /// 客户端已消费到的最后一个 `seq`。
    pub last_seq: u64,
}

/// 握手结果：D→UI。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Welcome {
    /// daemon 选定的协议版本（双方区间的最高交集）。
    pub proto: u16,
    /// daemon **自己**的能力位（研究稿 §6.1）。实际生效的是它与客户端能力的**交集**，
    /// 由客户端用 [`Caps::intersect`] 算。
    pub caps: Caps,
    /// daemon 的构建版本。
    pub daemon_version: String,
}

/// 一个**存活**会话的元数据（`ListSessions` 用）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    /// 逻辑会话 id。
    pub sid: String,
    /// 本次 spawn 的世代 id。
    pub incarnation_id: String,
    /// 子进程 pid。
    pub pid: u32,
    /// 会话的工作目录（用于 UI 重建页签）。
    pub cwd: String,
    /// 一等入口的 agent id（普通终端为 `None`）；UI 重开时恢复图标 / 名称。
    #[serde(default)]
    pub agent_id: Option<String>,
    /// 当前列数。
    #[serde(default)]
    pub cols: u16,
    /// 当前行数。
    #[serde(default)]
    pub rows: u16,
    /// spawn 时间（Unix 毫秒）；用于稳定排序 / 后续对账。
    #[serde(default)]
    pub started_at_ms: u64,
}

/// 会话清单：D→UI（响应 [`Message::ListSessions`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sessions {
    /// 当前 daemon 中仍然存活的会话。
    pub sessions: Vec<SessionInfo>,
}

/// 错误：D→UI。**版本不匹配**也走这里，带明确理由。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolError {
    /// 机器可读的错误码。
    pub code: ErrorCode,
    /// 给人看的理由。
    pub message: String,
}

/// 错误码。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// 版本区间无交集（[`negotiate`]）。
    UnsupportedVersion,
    /// 令牌不匹配。
    BadToken,
    /// 帧无法解析 / 用法不合协议。
    BadFrame,
    /// daemon 内部错误。
    Internal,
}

/// 建会话：UI→D。`sid` 是**逻辑**会话 id（字符串）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSession {
    /// 逻辑会话 id。
    pub sid: String,
    /// 工作目录（worktree 根）。
    pub cwd: String,
    /// 要起的 shell（缺省由 daemon 定）。
    #[serde(default)]
    pub shell: Option<String>,
    /// 启动命令（agent 页签用；daemon 在 shell 起来后写入 PTY）。
    #[serde(default)]
    pub command: Option<String>,
    /// 一等入口的 agent id（普通终端为 `None`）；仅作会话元数据，不参与 launch。
    #[serde(default)]
    pub agent_id: Option<String>,
    /// 初始列数。
    pub cols: u16,
    /// 初始行数。
    pub rows: u16,
    /// 追加的环境变量。
    #[serde(default)]
    pub env: Vec<EnvVar>,
}

/// 一个环境变量。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvVar {
    /// 变量名。
    pub name: String,
    /// 变量值。
    pub value: String,
}

/// 建好了：D→UI。**连接内句柄**由 daemon 分配，客户端之后用它收数据。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCreated {
    /// 逻辑会话 id。
    pub sid: String,
    /// 连接内句柄（数据帧帧头用的就是它）。
    pub handle: u32,
    /// 子进程 pid。
    pub pid: u32,
    /// 本次 spawn 的世代（`CONTEXT.md` 的 `incarnationId`；迟到 exit 据此丢弃）。
    pub incarnation_id: String,
}

/// 接回已有会话：UI→D。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attach {
    /// 逻辑会话 id。
    pub sid: String,
    /// 客户端已消费到的最后一个 `seq`。
    pub last_seq: u64,
}

/// 接上了：D→UI。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attached {
    /// 逻辑会话 id。
    pub sid: String,
    /// 连接内句柄。
    pub handle: u32,
    /// 回放将从哪个 `seq` 开始。
    pub from_seq: u64,
    /// 实时流从哪个 `seq` 开始；`from_seq..replay_end` 是环形缓冲回放。
    #[serde(default)]
    pub replay_end: u64,
    /// 无法精确回放时附带的 ANSI 快照（#71）；此时 `from_seq == replay_end`。
    #[serde(default)]
    pub snapshot: Option<String>,
    /// 快照 / 当前屏幕是否处于 alt-screen（前端决定是否插分隔线）。
    #[serde(default)]
    pub alt_screen: bool,
    /// 快照对应的终端列数（重连时前端据此判断是否需要 resize）。
    #[serde(default)]
    pub cols: u16,
    /// 快照对应的终端行数。
    #[serde(default)]
    pub rows: u16,
}

/// seq 断档：UI→D，请求一份当前 checkpoint。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Checkpoint {
    /// 逻辑会话 id。
    pub sid: String,
}

/// checkpoint：D→UI。`seq` 是快照对应的累计字节偏移。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    /// 逻辑会话 id。
    pub sid: String,
    /// 快照对应的累计字节偏移；之后的实时数据从 `seq` 继续。
    pub seq: u64,
    /// ANSI 快照。
    pub ansi: String,
    /// 屏幕是否处于 alt-screen。
    #[serde(default)]
    pub alt_screen: bool,
}

/// 断开：UI→D。**≠ 终止**——会话继续活在 daemon 里（§5.2 的三层退出语义）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Detach {
    /// 逻辑会话 id。
    pub sid: String,
}

/// 终止：UI→D。唯一真正结束会话的入口。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Terminate {
    /// 逻辑会话 id。
    pub sid: String,
    /// 是否强杀（先礼后兵）。
    #[serde(default)]
    pub force: bool,
}

/// 会话进程退出了：D→UI。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionExited {
    /// 逻辑会话 id。
    pub sid: String,
    /// 世代 id（`incarnationId`）。
    pub incarnation_id: String,
    /// 退出码。
    #[serde(default)]
    pub exit_code: Option<i32>,
}

/// 改尺寸：UI→D。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resize {
    /// 逻辑会话 id。
    pub sid: String,
    /// 列数。
    pub cols: u16,
    /// 行数。
    pub rows: u16,
}

/// 累计已消费字节数：UI→D（流控，研究稿 §4.2）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ack {
    /// 逻辑会话 id。
    pub sid: String,
    /// 累计已消费字节数。
    pub acked_bytes: u64,
}

/// 一条控制消息（**类型 + 负载**），与 [`Frame`] 一一对应。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    /// [`Hello`]。
    Hello(Hello),
    /// [`Welcome`]。
    Welcome(Welcome),
    /// [`ProtocolError`]。
    Error(ProtocolError),
    /// 心跳。
    Ping,
    /// 心跳应答。
    Pong,
    /// 枚举存活会话：UI→D（UI 重开时按 sid 重建页签）。
    ListSessions,
    /// [`Sessions`]：D→UI。
    Sessions(Sessions),
    /// 优雅停机：UI→D。终止所有会话后 daemon 退出。
    Shutdown,
    /// 停机确认：D→UI。收到时会话已清空，daemon 即将关闭连接并退出。
    ShutdownAck,
    /// [`CreateSession`]。
    CreateSession(CreateSession),
    /// [`SessionCreated`]。
    SessionCreated(SessionCreated),
    /// [`Attach`]。
    Attach(Attach),
    /// [`Attached`]。
    Attached(Attached),
    /// [`Checkpoint`]。
    Checkpoint(Checkpoint),
    /// [`Snapshot`]。
    Snapshot(Snapshot),
    /// [`Detach`]。
    Detach(Detach),
    /// [`Terminate`]。
    Terminate(Terminate),
    /// [`SessionExited`]。
    SessionExited(SessionExited),
    /// [`Resize`]。
    Resize(Resize),
    /// [`Ack`]。
    Ack(Ack),
    /// 会话输出：**句柄与序号在帧头**，`bytes` 是原始 PTY 字节。
    Data {
        /// 连接内句柄。
        handle: u32,
        /// 会话内单调递增序号。
        seq: u64,
        /// 原始字节。
        bytes: Vec<u8>,
    },
    /// 会话输入：原始按键字节。
    Input {
        /// 连接内句柄。
        handle: u32,
        /// 会话内单调递增序号。
        seq: u64,
        /// 原始字节。
        bytes: Vec<u8>,
    },
}

impl Message {
    /// 对应的帧类型。
    pub fn kind(&self) -> FrameKind {
        match self {
            Message::Hello(_) => FrameKind::Hello,
            Message::Welcome(_) => FrameKind::Welcome,
            Message::Error(_) => FrameKind::Error,
            Message::Ping => FrameKind::Ping,
            Message::Pong => FrameKind::Pong,
            Message::ListSessions => FrameKind::ListSessions,
            Message::Sessions(_) => FrameKind::Sessions,
            Message::Shutdown => FrameKind::Shutdown,
            Message::ShutdownAck => FrameKind::ShutdownAck,
            Message::CreateSession(_) => FrameKind::CreateSession,
            Message::SessionCreated(_) => FrameKind::SessionCreated,
            Message::Attach(_) => FrameKind::Attach,
            Message::Attached(_) => FrameKind::Attached,
            Message::Checkpoint(_) => FrameKind::Checkpoint,
            Message::Snapshot(_) => FrameKind::Snapshot,
            Message::Detach(_) => FrameKind::Detach,
            Message::Terminate(_) => FrameKind::Terminate,
            Message::SessionExited(_) => FrameKind::SessionExited,
            Message::Resize(_) => FrameKind::Resize,
            Message::Ack(_) => FrameKind::Ack,
            Message::Data { .. } => FrameKind::Data,
            Message::Input { .. } => FrameKind::Input,
        }
    }

    /// 编码成帧：原始字节帧用帧头带句柄与序号，其余走 JSON 负载。
    pub fn to_frame(&self) -> Result<Frame, MessageError> {
        let kind = self.kind();
        let payload = match self {
            // 热路径：句柄与序号在**帧头**，负载是裸字节——不碰 JSON。
            Message::Data { handle, seq, bytes } | Message::Input { handle, seq, bytes } => {
                return Ok(Frame::raw(kind, *handle, *seq, bytes.clone()))
            }
            Message::Hello(value) => serde_json::to_vec(value)?,
            Message::Welcome(value) => serde_json::to_vec(value)?,
            Message::Error(value) => serde_json::to_vec(value)?,
            Message::Sessions(value) => serde_json::to_vec(value)?,
            Message::CreateSession(value) => serde_json::to_vec(value)?,
            Message::SessionCreated(value) => serde_json::to_vec(value)?,
            Message::Attach(value) => serde_json::to_vec(value)?,
            Message::Attached(value) => serde_json::to_vec(value)?,
            Message::Checkpoint(value) => serde_json::to_vec(value)?,
            Message::Snapshot(value) => serde_json::to_vec(value)?,
            Message::Detach(value) => serde_json::to_vec(value)?,
            Message::Terminate(value) => serde_json::to_vec(value)?,
            Message::SessionExited(value) => serde_json::to_vec(value)?,
            Message::Resize(value) => serde_json::to_vec(value)?,
            Message::Ack(value) => serde_json::to_vec(value)?,
            // 心跳与枚举请求没有负载。
            Message::Ping
            | Message::Pong
            | Message::ListSessions
            | Message::Shutdown
            | Message::ShutdownAck => Vec::new(),
        };
        Ok(Frame::control(kind, 0, 0, payload))
    }

    /// 由帧解出消息。
    ///
    /// 未知 / 尚未定义的类型返回 [`MessageError::UnsupportedKind`]——**由调用方决定跳过还是
    /// 报错**（研究稿 §6.1：未知帧一律忽略，不断连）。
    pub fn from_frame(frame: &Frame) -> Result<Self, MessageError> {
        match frame.kind {
            FrameKind::Data => Ok(Message::Data {
                handle: frame.handle,
                seq: frame.seq,
                bytes: frame.payload.clone(),
            }),
            FrameKind::Input => Ok(Message::Input {
                handle: frame.handle,
                seq: frame.seq,
                bytes: frame.payload.clone(),
            }),
            FrameKind::Ping => Ok(Message::Ping),
            FrameKind::Pong => Ok(Message::Pong),
            FrameKind::Hello => Ok(Message::Hello(serde_json::from_slice(&frame.payload)?)),
            FrameKind::Welcome => Ok(Message::Welcome(serde_json::from_slice(&frame.payload)?)),
            FrameKind::Error => Ok(Message::Error(serde_json::from_slice(&frame.payload)?)),
            FrameKind::ListSessions => Ok(Message::ListSessions),
            FrameKind::Sessions => Ok(Message::Sessions(serde_json::from_slice(&frame.payload)?)),
            FrameKind::Shutdown => Ok(Message::Shutdown),
            FrameKind::ShutdownAck => Ok(Message::ShutdownAck),
            FrameKind::CreateSession => Ok(Message::CreateSession(serde_json::from_slice(
                &frame.payload,
            )?)),
            FrameKind::SessionCreated => Ok(Message::SessionCreated(serde_json::from_slice(
                &frame.payload,
            )?)),
            FrameKind::Attach => Ok(Message::Attach(serde_json::from_slice(&frame.payload)?)),
            FrameKind::Attached => Ok(Message::Attached(serde_json::from_slice(&frame.payload)?)),
            FrameKind::Checkpoint => {
                Ok(Message::Checkpoint(serde_json::from_slice(&frame.payload)?))
            }
            FrameKind::Snapshot => Ok(Message::Snapshot(serde_json::from_slice(&frame.payload)?)),
            FrameKind::Detach => Ok(Message::Detach(serde_json::from_slice(&frame.payload)?)),
            FrameKind::Terminate => Ok(Message::Terminate(serde_json::from_slice(&frame.payload)?)),
            FrameKind::SessionExited => Ok(Message::SessionExited(serde_json::from_slice(
                &frame.payload,
            )?)),
            FrameKind::Resize => Ok(Message::Resize(serde_json::from_slice(&frame.payload)?)),
            FrameKind::Ack => Ok(Message::Ack(serde_json::from_slice(&frame.payload)?)),
            other => Err(MessageError::UnsupportedKind(other)),
        }
    }
}

/// 消息（反）序列化错误。
#[derive(Debug, thiserror::Error)]
pub enum MessageError {
    /// 这个帧类型还没有对应的消息定义（或干脆不认识）。
    #[error("帧类型 {0:?} 没有对应的消息")]
    UnsupportedKind(FrameKind),

    /// 控制帧负载不是合法 JSON。
    #[error("控制帧负载不是合法 JSON：{0}")]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every_message() -> Vec<Message> {
        vec![
            Message::Hello(Hello {
                proto_min: PROTOCOL_VERSION_MIN,
                proto_max: PROTOCOL_VERSION,
                caps: Caps::supported(),
                token: "tok".to_owned(),
                client_instance_id: "ui-1".to_owned(),
                resume: vec![Resume {
                    sid: "wt/main/shell/1".to_owned(),
                    last_seq: 42,
                }],
            }),
            Message::Welcome(Welcome {
                proto: PROTOCOL_VERSION,
                caps: Caps::supported(),
                daemon_version: "0.0.0".to_owned(),
            }),
            Message::Error(ProtocolError {
                code: ErrorCode::UnsupportedVersion,
                message: "无交集".to_owned(),
            }),
            Message::Ping,
            Message::Pong,
            Message::ListSessions,
            Message::Shutdown,
            Message::ShutdownAck,
            Message::Sessions(Sessions {
                sessions: vec![SessionInfo {
                    sid: "wt/main/shell/1".to_owned(),
                    incarnation_id: "inc-1".to_owned(),
                    pid: 4242,
                    cwd: r"C:\code\wt".to_owned(),
                    agent_id: None,
                    cols: 120,
                    rows: 30,
                    started_at_ms: 1_700_000_000_000,
                }],
            }),
            Message::CreateSession(CreateSession {
                sid: "wt/main/shell/1".to_owned(),
                cwd: r"C:\code\wt".to_owned(),
                shell: Some("pwsh".to_owned()),
                command: None,
                agent_id: None,
                cols: 120,
                rows: 30,
                env: vec![EnvVar {
                    name: "AGEMINAL_SESSION_ID".to_owned(),
                    value: "wt/main/shell/1".to_owned(),
                }],
            }),
            Message::SessionCreated(SessionCreated {
                sid: "wt/main/shell/1".to_owned(),
                handle: 3,
                pid: 4242,
                incarnation_id: "inc-1".to_owned(),
            }),
            Message::Attach(Attach {
                sid: "wt/main/shell/1".to_owned(),
                last_seq: 7,
            }),
            Message::Attached(Attached {
                sid: "wt/main/shell/1".to_owned(),
                handle: 3,
                from_seq: 8,
                replay_end: 12,
                snapshot: None,
                alt_screen: false,
                cols: 120,
                rows: 30,
            }),
            Message::Checkpoint(Checkpoint {
                sid: "wt/main/shell/1".to_owned(),
            }),
            Message::Snapshot(Snapshot {
                sid: "wt/main/shell/1".to_owned(),
                seq: 12,
                ansi: "\u{1b}[2Jhello".to_owned(),
                alt_screen: false,
            }),
            Message::Detach(Detach {
                sid: "wt/main/shell/1".to_owned(),
            }),
            Message::Terminate(Terminate {
                sid: "wt/main/shell/1".to_owned(),
                force: true,
            }),
            Message::SessionExited(SessionExited {
                sid: "wt/main/shell/1".to_owned(),
                incarnation_id: "inc-1".to_owned(),
                exit_code: Some(0),
            }),
            Message::Resize(Resize {
                sid: "wt/main/shell/1".to_owned(),
                cols: 100,
                rows: 40,
            }),
            Message::Ack(Ack {
                sid: "wt/main/shell/1".to_owned(),
                acked_bytes: 65536,
            }),
            Message::Data {
                handle: 3,
                seq: 9,
                bytes: vec![0x1b, 0x5b, 0x33, 0x31, 0x6d, 0xff],
            },
            Message::Input {
                handle: 3,
                seq: 1,
                bytes: b"ls\r".to_vec(),
            },
        ]
    }

    #[test]
    fn every_message_round_trips_through_a_frame() {
        for message in every_message() {
            let frame = message.to_frame().unwrap();
            assert_eq!(frame.kind, message.kind(), "类型应一致：{message:?}");
            let back = Message::from_frame(&frame).unwrap();
            assert_eq!(back, message);
        }
    }

    #[test]
    fn raw_frames_carry_handle_and_seq_in_the_header() {
        let message = Message::Data {
            handle: 11,
            seq: 22,
            bytes: b"\x1b[0m".to_vec(),
        };
        let frame = message.to_frame().unwrap();
        assert_eq!(frame.handle, 11);
        assert_eq!(frame.seq, 22);
        assert!(frame.flags.contains(crate::frame::Flags::PAYLOAD_IS_RAW));
        // JSON 负载携带原始字节是禁忌：数据帧必须是裸字节。
        assert_eq!(frame.payload, b"\x1b[0m");
    }

    #[test]
    fn control_frames_leave_session_and_seq_zero() {
        let frame = Message::Resize(Resize {
            sid: "s".to_owned(),
            cols: 1,
            rows: 1,
        })
        .to_frame()
        .unwrap();
        assert_eq!(frame.handle, 0, "控制帧不占帧头句柄位");
        assert_eq!(frame.seq, 0);
    }

    #[test]
    fn unknown_kinds_are_reported_not_panicked() {
        let frame = Frame::control(FrameKind::Unknown(0x7777), 0, 0, Vec::new());
        assert!(matches!(
            Message::from_frame(&frame),
            Err(MessageError::UnsupportedKind(FrameKind::Unknown(0x7777)))
        ));
    }

    #[test]
    fn malformed_json_is_an_error_not_a_panic() {
        let frame = Frame::control(FrameKind::Resize, 0, 0, b"{not json".to_vec());
        assert!(matches!(
            Message::from_frame(&frame),
            Err(MessageError::Json(_))
        ));
    }

    #[test]
    fn caps_are_a_bitmap() {
        let supported = Caps::supported();
        assert!(supported.contains(Caps::BINARY_PAYLOAD));
        assert!(!supported.contains(Caps::COMPRESSION), "v1 不开压缩");

        // 客户端只声明压缩 → 交集为空，谁也别想用没谈拢的能力。
        let client = Caps::default();
        assert_eq!(supported.intersect(client).bits(), 0);
        assert_eq!(supported.intersect(supported), supported);
        assert_eq!(Caps::default().bits(), 0, "默认是空能力集");
    }

    #[test]
    fn negotiation_takes_the_highest_overlap() {
        assert_eq!(negotiate(1, 1), Negotiation::Agreed(1));
        // 客户端比我们新：取我们的上界。
        assert_eq!(negotiate(1, 9), Negotiation::Agreed(PROTOCOL_VERSION));
        // 客户端区间更宽：仍取交集上界。
        assert_eq!(negotiate(0, 9), Negotiation::Agreed(PROTOCOL_VERSION));
        // 无交集。
        assert_eq!(negotiate(2, 5), Negotiation::NoOverlap);
        // 区间自身非法。
        assert_eq!(negotiate(5, 2), Negotiation::NoOverlap);
    }
}
