//! Agent 原生事件 → 统一状态的**纯逻辑**（#80），以及无原生事件时的降级启发式（#81）。
//!
//! 三层：
//! - [`AgentState`] / [`StatusSource`] / [`AgentStatusSnapshot`]：领域词汇与版本化快照；
//! - [`AgentReducer`]：单会话归约（优先级、逻辑序、幂等、防抖、子代理过滤）；
//! - [`StatusBook`]：app-core 生命周期内的多会话表；同时承载进程树 + 输出静默的降级状态。
//!
//! 本 crate **不读时钟、不做 I/O、不依赖 Tauri**，所以夹具与 proptest 跨平台可跑。
//! 输入使用 `ageminal-protocol::AgentEvent`（#79 的线上形状），进程退出使用
//! `SessionExited`；app-core 负责把 Tauri 事件 / 时钟传进来。

mod mapping;
mod processes;
mod reducer;
mod state;

pub use processes::{detect_agent, DetectedAgent, ProcessNode, RecognitionRule};
pub use reducer::{
    AgentReducer, DegradedObservation, StatusBook, DEBOUNCE_MS, T_IDLE_MS, T_INTERRUPT_MS,
};
pub use state::{AgentState, AgentStatusSnapshot, StatusSource, StatusUpdate};
