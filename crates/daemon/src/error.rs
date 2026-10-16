//! daemon 的类型化错误（REQUIREMENTS.md §15「错误边界」）。
//!
//! 与 app-core 的边界约定一致：库内用类型化错误，到了 Tauri 命令边界再 `to_string()`。

use std::path::PathBuf;
use std::time::Duration;

/// daemon 拉起 / 握手 / 管道 IO 期间的类型化错误。
#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    /// 已经有一个 daemon 占着这条管道（单例守卫生效）。**不是失败**，调用方应改去复用。
    #[error("daemon 已在运行（管道 {pipe}）")]
    AlreadyRunning { pipe: String },

    /// 连不上 daemon。
    #[error("daemon 未运行：{reason}")]
    NotRunning { reason: String },

    /// 命名管道创建失败。
    #[error("无法创建命名管道 {pipe}：{reason}")]
    Pipe { pipe: String, reason: String },

    /// 等待 daemon 就绪超时。
    #[error("等待 daemon 就绪超时（{timeout:?}）：{reason}")]
    Timeout { timeout: Duration, reason: String },

    /// 握手被 daemon 拒绝。
    #[error("握手被拒：{reason}")]
    Rejected { reason: String },

    /// 协议版本不一致。
    #[error("协议版本不一致：daemon {daemon} / 客户端 {client}")]
    ProtocolMismatch { daemon: u32, client: u32 },

    /// 收到了当下不该出现的消息（协议状态机被打破）。
    #[error("意外的消息：{0}")]
    Unexpected(String),

    /// 状态文件读写失败。
    #[error("状态文件读写失败 {path}：{source}")]
    State {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// 无法启动 daemon 进程。
    #[error("无法启动 daemon：{reason}")]
    Launch { reason: String },

    /// 环境变量缺失（生产上只有 `%LOCALAPPDATA%`）。
    #[error("环境变量 {name} 未设置")]
    MissingEnv { name: &'static str },

    /// Windows 调用失败。
    #[error("Windows 调用 {call} 失败：{reason}")]
    Windows { call: &'static str, reason: String },

    /// 编解码失败。
    #[error("编解码失败：{reason}")]
    Codec { reason: String },

    /// 管道 IO 失败。
    #[error("管道 IO 失败：{0}")]
    Io(#[from] std::io::Error),
}
