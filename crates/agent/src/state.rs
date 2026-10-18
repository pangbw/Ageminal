//! 统一状态与版本化快照（#80）。
//!
//! 状态词汇严格照 `REQUIREMENTS.md` §10.3：
//! `running` / `waiting` / `permission` / `done` / `idle` / `error` / `exited`。
//! `done` 与 `idle` 是**不同**状态；本模块没有计时器，`done` 不会自动迁移。

use serde::{Deserialize, Serialize};

/// 一个 agent 会话的统一状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentState {
    /// 进程存在，agent 正在推进（模型 / 工具 / 重试 / 流式输出）。
    Running,
    /// 回合正常结束；**常驻**到新的活动事件把它带回 `running`。
    Done,
    /// 空闲（显式 `idle_prompt` 等）；不是 `done` 的自动超时结果。
    Idle,
    /// 等**非授权类**用户输入。
    Waiting,
    /// 卡在授权 / 审批弹窗。
    Permission,
    /// 回合或会话因错误终止。
    Error,
    /// 背后的进程结束（正常或非零退出码）。
    Exited,
}

impl AgentState {
    /// 冲突优先级：`exited > error > permission > waiting > running`。
    ///
    /// `done` / `idle` 与 `running` 的关系另由 [`crate::AgentReducer`] 的转换规则保护：
    /// 它们不能把 `running` 打回，所以不靠单纯的数字优先级表达。
    pub fn priority(self) -> u8 {
        match self {
            Self::Exited => 5,
            Self::Error => 4,
            Self::Permission => 3,
            Self::Waiting => 2,
            Self::Running => 1,
            Self::Done | Self::Idle => 0,
        }
    }

    /// 是否是终态（当前只有进程退出）。
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Exited)
    }

    /// 序列化 / 日志用的稳定小写名。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Done => "done",
            Self::Idle => "idle",
            Self::Waiting => "waiting",
            Self::Permission => "permission",
            Self::Error => "error",
            Self::Exited => "exited",
        }
    }
}

/// 状态事实的来源。
///
/// 首批只实现 `native`（agent 原生事件）与 `process`（进程退出）；#81 会加入 `heuristic`
/// 降级信号，届时 UI 需要显式标注「状态不完整」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StatusSource {
    /// agent 原生事件（hook / 插件 / 通知）。
    Native,
    /// 会话进程的硬事实（退出 / 退出码）。
    Process,
    /// 进程树 + 输出静默等降级启发式（#81 预留）。
    Heuristic,
}

/// 版本化状态快照。
///
/// 字段顺序对应 `REQUIREMENTS.md` §10.3 的契约：`state + source + timestamp + seq`；
/// `sid` / `agent` / `incarnationId` 是投影到多会话 UI 所需的键，`exitCode` / `reason`
/// 是事实附件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatusSnapshot {
    /// 逻辑会话 id。
    pub sid: String,
    /// agent id；进程退出事件没有 agent 时沿用上一次已知值。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// 本次 spawn 的世代 id。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incarnation_id: Option<String>,
    /// 统一状态。
    pub state: AgentState,
    /// 这条状态来自原生事件、进程事实还是降级启发式。
    pub source: StatusSource,
    /// 事件时间（Unix 毫秒）。
    pub timestamp_ms: u64,
    /// agent 提供的逻辑序号；没有时为 `0`，排序退回时间戳。
    pub seq: u64,
    /// 进程退出码；仅 `exited` 时存在。非零退出码**不会**把状态改成 `error`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// 最近一次错误原因（`error` 产生；进入 `exited` 时保留）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// 一次归约产生的结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusUpdate {
    /// 改变前的状态；第一次事件为 `None`。
    pub previous: Option<AgentState>,
    /// 归约后的完整快照。
    pub current: AgentStatusSnapshot,
    /// 状态是否真的发生了变化。重复事件 / 仅刷新版本时间时为 `false`。
    pub changed: bool,
}
