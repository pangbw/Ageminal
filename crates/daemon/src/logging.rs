//! 最小落盘日志（REQUIREMENTS.md §15 的雏形；正式 `tracing` 栈见 issue #88）。
//!
//! **运行期绝不写 stdout / stderr**：daemon 由 UI 以 `DETACHED_PROCESS` 拉起、stdio 指向 NUL，
//! `println!` / `eprintln!` 一旦写失败就会 panic（REQUIREMENTS.md §5.1）。所以运行信息只落文件。

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// 追加一行日志。**任何失败都静默**——日志系统不该反过来把 daemon 弄崩。
pub fn append(path: &Path, message: &str) {
    let Some(parent) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let _ = writeln!(file, "{} {message}", timestamp_ms());
}

/// Unix 毫秒。跨进程对齐够用；可读的格式化留给 #88 的 `tracing` 栈。
fn timestamp_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_lines_and_creates_parent_directories() {
        let dir = std::env::temp_dir().join(format!("ageminal-daemon-log-{}", std::process::id()));
        let path = dir.join("logs").join("daemon.log");
        let _ = std::fs::remove_dir_all(&dir);

        append(&path, "第一行");
        append(&path, "第二行");

        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 2);
        assert!(text.contains("第一行"));
        assert!(text.contains("第二行"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
