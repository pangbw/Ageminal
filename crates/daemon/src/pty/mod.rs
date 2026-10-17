//! PTY 层（issue #66）：藏在 [`Pty`] trait 之后，当前实现是 Windows ConPTY。
//!
//! `CONTEXT.md`：「PTY 层」藏在 `Pty` trait 之后（可替换实现）；**关闭 HPCON 即终止该会话的
//! 进程树**。
//!
//! 这一层只做四件事：**起进程、收发字节、改尺寸、杀掉整棵树**。会话表、输出路由、ack 流控都是
//! #67 的事——所以这里没有 `sid`，也不认识协议。
//!
//! **不含 VT 语义**：ConPTY 会朝终端侧发 `CSI 6n`（光标位置报告）之类的查询并等回复，没人应答
//! 会把上游程序卡住（`docs/research/session-daemon.md` §5.2）。应答它需要屏幕模型，属服务端 VT
//! 内核的职责，不在这一层。
//!
//! 读端与写端**分开取走**（[`Pty::take_reader`] / [`Pty::take_writer`]）：daemon 里「收 PTY
//! 输出」与「喂键盘输入」必须跑在不同线程，一条 `&mut self` 会让两边互相阻塞。

use std::io::{Read, Write};
use std::path::PathBuf;

#[cfg(windows)]
mod conpty;
#[cfg(windows)]
mod wincmd;

#[cfg(windows)]
pub use conpty::spawn;

/// 取 `GetLastError` 并包成 [`PtyError`]。
#[cfg(windows)]
fn windows_error(call: &'static str) -> PtyError {
    PtyError::Windows {
        call,
        reason: format!("GetLastError={}", unsafe {
            windows_sys::Win32::Foundation::GetLastError()
        }),
    }
}

/// 终端尺寸（列 / 行）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PtySize {
    /// 列数。
    pub cols: u16,
    /// 行数。
    pub rows: u16,
}

/// 起一个 PTY 会话需要的东西（会话语义在 #67，这里只描述「怎么起进程」）。
#[derive(Debug, Clone)]
pub struct PtySpec {
    /// 可执行文件：绝对路径，或能在 `PATH` 里找到的名字。
    pub program: String,

    /// 命令行参数（**未**加引号；拼命令行由实现负责）。
    pub args: Vec<String>,

    /// 工作目录（worktree 根）。
    pub cwd: PathBuf,

    /// 初始尺寸。
    pub size: PtySize,

    /// 追加 / 覆盖的环境变量——`AGEMINAL_*` 注入走这里（#25 / #67）。
    pub env: Vec<(String, String)>,
}

/// 子进程退出状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitStatus {
    /// 退出码。
    pub code: u32,
}

/// 一个**已启动**的 PTY 实例（主端）。
///
/// 注意它不是 `CONTEXT.md` 里的「会话（Session）」——那是有 `sid`、归 daemon 会话表管的东西
/// （#67）。这里只是「一个进程 + 一对管道 + 一个伪控制台」。
///
/// 实现方负责：持有伪控制台（HPCON）与子进程；`Drop` 时若还没终止，兜底终止。
pub trait Pty: Send {
    /// 子进程 pid。
    fn pid(&self) -> u32;

    /// 改终端尺寸（同步生效）。
    fn resize(&self, size: PtySize) -> Result<(), PtyError>;

    /// 取走输出流（**只能取一次**；第二次返回 `None`）。
    fn take_reader(&mut self) -> Option<Box<dyn Read + Send>>;

    /// 取走输入流（**只能取一次**；第二次返回 `None`）。
    fn take_writer(&mut self) -> Option<Box<dyn Write + Send>>;

    /// 等子进程退出（阻塞）。调用方负责把它搬到线程上。
    fn wait(&mut self) -> Result<ExitStatus, PtyError>;

    /// 非阻塞地问一句「退了没」。
    fn try_wait(&mut self) -> Result<Option<ExitStatus>, PtyError>;

    /// 终止**整棵进程树**（关 HPCON）；幂等。
    ///
    /// **调用前先把输出流取走并持续读**：关控制台要等 conout 排空，没人读会卡住
    /// （`docs/research/zed-terminal.md` §3.2 记的那条死锁；MS 文档也写了
    /// 「failure to either close or drain the output pipe may cause `ClosePseudoConsole` to wait
    /// indefinitely」）。实现会在 `Drop` 里对**还没被取走**的输出流自行排空兜底，但已经取走的
    /// 那一半必须由调用方负责读到 EOF。
    fn terminate(&mut self) -> Result<(), PtyError>;
}

/// PTY 层错误。
#[derive(Debug, thiserror::Error)]
pub enum PtyError {
    /// Windows 调用失败。
    #[error("PTY 调用 {call} 失败：{reason}")]
    Windows {
        /// 失败的 API。
        call: &'static str,
        /// 原始错误（`GetLastError` 或 HRESULT）。
        reason: String,
    },

    /// 会话已终止。
    #[error("PTY 会话已终止")]
    Terminated,
}
