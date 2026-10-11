//! Ageminal 的 git 访问层（#17 决议）：worktree 发现 + 薄命令执行。
//!
//! - **唯一真实来源 = git CLI**：`git worktree list --porcelain -z`，要求 Git 2.36+
//!   （`-z` 自 2.36 起；#17 D1/D3）。worktree 列表**不落盘**（REQUIREMENTS.md §4）。
//! - **纯逻辑与进程分离**：[`worktree`] 只做 porcelain 解析与模型映射（跨平台可测，
//!   夹具驱动）；[`exec`] 负责可执行文件发现、版本校验与短超时执行。
//! - **`-z` 而非行模式**：行模式遇到路径含空格 / 中文会做 C 式引号转义，`-z`
//!   把路径原样输出为字节（#17 实测），因此解析器**不自行去引号**。
//!
//! 本模块不做缓存，也不落盘：每次发现都直接问 git。

mod error;
mod exec;
pub mod worktree;

pub use error::{GitError, GitVersion, MIN_GIT_VERSION};
pub use exec::Git;
