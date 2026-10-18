//! 应用侧落盘日志（#88）。
//!
//! - `%LOCALAPPDATA%\\Ageminal\\logs\\app.log`；
//! - 单行文本 + tracing 结构化字段（`sid` / `incarnation` 等）；
//! - 10 MB × 5 代滚动；daemon / notify 各自维护自己的文件。
//!
//! daemon 的 stdio 指向 NUL，不能 eprintln；应用侧 eprintln 也统一改成 `tracing::warn!`，
//! 统一从本 writer 落盘。

use std::fs::OpenOptions;
use std::io;
use std::path::{Path, PathBuf};

use tracing_subscriber::fmt::MakeWriter;

/// 单个日志文件上限。
pub const MAX_LOG_BYTES: u64 = 10 * 1024 * 1024;
/// 保留的历史代数（不含当前文件）。
pub const LOG_GENERATIONS: u32 = 5;

/// 打开日志目录；缺失环境变量时回退到临时目录，日志系统自身不能阻塞应用。
pub fn logs_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|path| path.join("Ageminal").join("logs"))
        .unwrap_or_else(|| std::env::temp_dir().join("ageminal-logs"))
}

struct RotatingWriter {
    path: PathBuf,
}

impl<'a> MakeWriter<'a> for RotatingWriter {
    type Writer = Box<dyn io::Write + Send>;

    fn make_writer(&'a self) -> Self::Writer {
        rotate_if_needed(&self.path);
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            Ok(file) => Box::new(file),
            Err(_) => Box::new(io::sink()),
        }
    }
}

fn rotate_if_needed(path: &Path) {
    let too_large = std::fs::metadata(path)
        .map(|metadata| metadata.len() >= MAX_LOG_BYTES)
        .unwrap_or(false);
    if !too_large {
        return;
    }
    let base = path.to_string_lossy();
    let oldest = PathBuf::from(format!("{base}.{}", LOG_GENERATIONS));
    let _ = std::fs::remove_file(oldest);
    for generation in (1..LOG_GENERATIONS).rev() {
        let from = PathBuf::from(format!("{base}.{generation}"));
        let to = PathBuf::from(format!("{base}.{}", generation + 1));
        let _ = std::fs::rename(from, to);
    }
    let _ = std::fs::rename(path, PathBuf::from(format!("{base}.1")));
}

/// 初始化应用日志；重复调用（测试 / 二次启动）忽略。
pub fn init() {
    let path = logs_dir().join("app.log");
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_target(false)
        .with_level(true)
        .with_writer(RotatingWriter { path })
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_at_the_size_limit_and_keeps_five_generations() {
        let root = std::env::temp_dir().join(format!("ageminal-app-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("app.log");
        for generation in 1..=5 {
            std::fs::write(
                root.join(format!("app.log.{generation}")),
                format!("old-{generation}"),
            )
            .unwrap();
        }
        std::fs::write(&path, vec![b'x'; MAX_LOG_BYTES as usize]).unwrap();
        rotate_if_needed(&path);

        assert!(!path.exists());
        assert!(root.join("app.log.1").exists());
        assert_eq!(
            std::fs::read_to_string(root.join("app.log.5")).unwrap(),
            "old-4"
        );
        assert!(!root.join("app.log.6").exists());
        let _ = std::fs::remove_dir_all(root);
    }
}
