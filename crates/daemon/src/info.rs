//! daemon 的发现信息（落盘为 `daemon.json`）。

use serde::{Deserialize, Serialize};

/// daemon 的发现信息：客户端靠它拿到 **管道名 + 握手令牌**。
///
/// 为什么走文件而不是让 app 定义管道名：令牌要**每次 daemon 启动重生成**（REQUIREMENTS.md
/// §10.5 的 `AGEMINAL_TOKEN`），只能由 daemon 自己生成再告诉别人。`pid` / `started_at_ms`
/// 用来判断读到的是不是**当前世代**——上一世代留下的文件不能拿去握手（令牌已失效）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonInfo {
    /// daemon 进程 id。
    pub pid: u32,

    /// 命名管道完整路径（`\\.\pipe\…`）。
    pub pipe: String,

    /// 握手令牌。每次 daemon 启动重生成，进程退出即失效。
    pub token: String,

    /// daemon 的协议版本（`ageminal_protocol::PROTOCOL_VERSION`）。
    pub protocol: u32,

    /// daemon 的 crate 版本。
    pub version: String,

    /// 启动时刻（Unix 毫秒）。
    pub started_at_ms: u64,
}
