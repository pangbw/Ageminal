//! 文件域的类型化错误（REQUIREMENTS.md §15「错误边界」）。

use std::path::PathBuf;

/// 列举目录期间的错误，都带上是哪个路径出的事。
#[derive(Debug, thiserror::Error)]
pub enum FilesError {
    /// 目标目录不在工作树根之内（路径规范化校验，§15）。
    #[error("目录不在工作树根之内：{dir}")]
    OutsideRoot { dir: PathBuf },

    /// 目标不存在（文件或目录）。
    #[error("不存在：{path}")]
    NotFound { path: PathBuf },

    /// 目标是 worktree 根：禁止整体删除 / 改名（#62）。
    #[error("不能操作 worktree 根：{path}")]
    ProtectedRoot { path: PathBuf },

    /// 目标是目录，不是可预览的文件（预览只读文件）。
    #[error("不是文件：{path}")]
    NotAFile { path: PathBuf },

    /// 读取目录失败（权限不足、不是目录、超长路径等）。
    #[error("读取目录失败：{path}：{source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// 目标已存在（新建 / 重命名）。
    #[error("已存在：{path}")]
    AlreadyExists { path: PathBuf },

    /// 名称非法（空、`.` / `..`、含路径分隔符或控制字符）。
    #[error("名称非法：{name}")]
    InvalidName { name: String },

    /// 移入回收站失败（§9：回收站不可用时**显式报错，绝不静默永久删除**）。
    #[error("移入回收站失败：{path}：{source}")]
    Trash {
        path: PathBuf,
        #[source]
        source: trash::Error,
    },

    /// 建立 / 调整文件监听失败（#61）。
    #[error("文件监听失败：{source}")]
    Watch {
        #[source]
        source: notify::Error,
    },
}
