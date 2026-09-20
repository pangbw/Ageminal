//! 极简文件日志。daemon 被 detached 拉起时 stdout 是空的，
//! 出问题时除了一条命名管道就无从观察——所以关键节点写文件，`selftest` 收尾时读回来。

use std::fs::OpenOptions;
use std::io::Write;
use std::sync::{Mutex, OnceLock};

static LOG: OnceLock<Mutex<Option<std::fs::File>>> = OnceLock::new();

fn cell() -> &'static Mutex<Option<std::fs::File>> {
    LOG.get_or_init(|| Mutex::new(None))
}

pub fn init(path: &std::path::Path) {
    match OpenOptions::new().create(true).append(true).open(path) {
        Ok(f) => {
            *cell().lock().unwrap() = Some(f);
            log(&format!("日志开始：{}", path.display()));
        }
        Err(e) => eprintln!("[log] 打开日志失败 {}：{e}", path.display()),
    }
}

pub fn log(msg: &str) {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    if let Ok(mut g) = cell().lock() {
        if let Some(f) = g.as_mut() {
            let _ = writeln!(f, "[{ts}] {msg}");
            let _ = f.flush();
        }
    }
    // 前台运行时（daemon 手动跑）也能看到
    #[cfg(windows)]
    let _ = std::io::stdout().flush();
    let _ = &ts;
}

/// 读回最后 `n` 行
pub fn tail(path: &std::path::Path, n: usize) -> Vec<String> {
    let s = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => return vec![format!("(读取日志失败：{e})")],
    };
    let lines: Vec<&str> = s.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].iter().map(|s| s.to_string()).collect()
}
