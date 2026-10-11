//! git 调用的类型化错误（REQUIREMENTS.md §15「错误边界」）。

use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

/// git 版本。只比较 major / minor —— `--porcelain -z` 的门槛就落在这两位上。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct GitVersion {
    pub major: u32,
    pub minor: u32,
}

/// 支持的最低 Git 版本：`-z` 随 Git 2.36 发布（#17 D3）。
pub const MIN_GIT_VERSION: GitVersion = GitVersion {
    major: 2,
    minor: 36,
};

impl GitVersion {
    /// 解析 `git --version` 的输出，如 `git version 2.36.1.windows.1`。
    ///
    /// 容错各种后缀（`.windows.1` / ` (Apple Git-146)` / `.rc1`）：只取前两段数字。
    pub fn parse(output: &str) -> Option<Self> {
        let rest = output.trim().strip_prefix("git version ")?;
        let mut parts = rest.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        Some(Self { major, minor })
    }
}

impl fmt::Display for GitVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// git 发现 / 调用期间的类型化错误。
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    /// 没有任何候选可执行文件可用。
    #[error("未找到可用的 git 可执行文件")]
    NotFound,

    /// 找到的 git 版本低于下限。
    #[error("Git 版本过低：需要 {min} 及以上，当前 {found}")]
    TooOld { found: GitVersion, min: GitVersion },

    /// 目标目录不是 git 仓库。
    #[error("不是 git 仓库：{repo_root}")]
    NotARepository { repo_root: PathBuf },

    /// git 以非零码退出，且不属于上面可识别的情形。
    #[error("git {command} 失败（退出码 {code:?}）：{stderr}")]
    Failed {
        command: &'static str,
        code: Option<i32>,
        stderr: String,
    },

    /// git 超时后被终止（#17 D8「短超时 + kill」）。
    #[error("git {command} 超时（{timeout:?}）后被终止")]
    Timeout {
        command: &'static str,
        timeout: Duration,
    },

    /// 无法启动 git 进程（路径不存在 / 无执行权限等）。
    #[error("无法启动 git {command}：{source}")]
    Spawn {
        command: &'static str,
        #[source]
        source: std::io::Error,
    },

    /// 输出不符合预期（版本输出无法解析、porcelain 非 UTF-8 / 为空 / 缺路径）。
    #[error("git 输出无法解析：{reason}")]
    MalformedOutput { reason: String },
}
