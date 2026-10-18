//! app-core ↔ daemon 的帧协议——**Rust 单一来源**（REQUIREMENTS.md §19 / 研究稿
//! `docs/research/daemon-ipc-protocol.md`）。
//!
//! 四块：
//!
//! - [`frame`]：字节流上的自研分帧。**20 字节定长头 + payload**；数据帧 payload 是原始字节，
//!   控制帧 payload 是 UTF-8 JSON（研究稿 §3）。
//! - [`message`]：控制消息的类型与**版本协商**（取双方支持区间的最高交集；无交集要**明确报错**，
//!   不静默断连——研究稿 §6.1）。
//! - [`queue`]：发送侧的**控制优先队列**（研究稿 §五 做法 B：控制面永远不被数据面饿死）。
//! - [`discovery`]：发现文件 `daemon.json`——客户端据此拿到管道名与握手令牌。
//!
//! **线协议只有本 crate 拥有**：daemon 与 app-core 都不自己解字节、不自己拼 JSON。
//! 协议版本与 app 版本**解耦**，仅在帧形状变化时递增。

pub mod discovery;
pub mod frame;
pub mod message;
pub mod queue;

pub use discovery::{state_file_in, DaemonInfo, DiscoveryError, STATE_FILE_NAME};
pub use frame::{fragments, Flags, Frame, FrameError, FrameKind, HEADER_BYTES, MAX_FRAME_BYTES};
pub use message::{
    negotiate, Ack, Attach, Attached, Caps, Checkpoint, CreateSession, Detach, EnvVar, ErrorCode,
    Hello, Message, MessageError, Negotiation, ProtocolError, Resize, Resume, SessionCreated,
    SessionExited, SessionInfo, Sessions, Snapshot, Terminate, Welcome, PROTOCOL_VERSION,
    PROTOCOL_VERSION_MIN,
};
pub use queue::ControlFirstQueue;
