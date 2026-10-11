//! 文件域的类型化错误（REQUIREMENTS.md §15「错误边界」）。

use std::path::PathBuf;

/// 列举目录期间的错误，都带上是哪个路径出的事。
#[derive(Debug, thiserror::Error)]
pub enum FilesError {
    /// 目标目录不在工作树根之内（路径规范化校验，§15）。
    #[error("目录不在工作树根之内：{dir}")]
    OutsideRoot { dir: PathBuf },

    /// 目标目录不存在。
    #[error("目录不存在：{path}")]
    NotFound { path: PathBuf },

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
}
