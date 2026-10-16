//! Ageminal 会话守护进程（issue #64：生命周期与握手）。
//!
//! 分工（REQUIREMENTS.md §5.1 / §15）：
//!
//! - **仅 Windows**：命名管道 + ConPTY 是 Windows 专有，所以实现全部挂在 `cfg(windows)` 下。
//!   非 Windows 平台本 crate 仍要**能编译**（CI 在 Linux 上跑 clippy / test），只是没有实现。
//! - **不依赖 stdout**：daemon 由 UI 以 `DETACHED_PROCESS` 拉起、**stdio 三句柄指向 NUL**，
//!   所以运行期只写文件日志（[`server`]），**绝不 `println!`**。
//! - **单例 + 按用户隔离**：管道名带当前用户 SID，且用 `FILE_FLAG_FIRST_PIPE_INSTANCE` 做
//!   原子单例守卫；管道 DACL 只放当前用户（[`acl`]）。
//! - **发现靠落盘**：daemon 把 管道名 + 握手令牌 写进 `%LOCALAPPDATA%\Ageminal\daemon.json`
//!   （[`DaemonInfo`]），客户端读它来连接。
//!
//! 面向 app 的入口是 [`launch::ensure_running`]；面向已连接会话的是 [`client`]。

mod error;
mod info;
pub mod wire;

pub use error::DaemonError;
pub use info::DaemonInfo;

#[cfg(windows)]
mod acl;
#[cfg(windows)]
pub mod client;
#[cfg(windows)]
pub mod launch;
#[cfg(windows)]
mod logging;
#[cfg(windows)]
pub mod paths;
#[cfg(windows)]
pub mod server;
#[cfg(windows)]
mod win;
