//! 运行期落点与命名（Windows）。
//!
//! 生产落点（REQUIREMENTS.md §5.4 / §15）：
//! - 状态：`%LOCALAPPDATA%\Ageminal\daemon.json`（格式在 [`crate::DaemonInfo`]）
//! - 日志：`%LOCALAPPDATA%\Ageminal\logs\daemon.log`
//!
//! `…_in(config_dir)` 版本是给集成测试用的：测试把配置根挪到临时目录，免得和用户机器上真在跑的
//! daemon 抢同一份状态文件。

use std::path::{Path, PathBuf};

use crate::{win, DaemonError};

/// 配置根：`%LOCALAPPDATA%\Ageminal`。
pub fn config_dir() -> Result<PathBuf, DaemonError> {
    let local = std::env::var_os("LOCALAPPDATA").ok_or(DaemonError::MissingEnv {
        name: "LOCALAPPDATA",
    })?;
    Ok(PathBuf::from(local).join("Ageminal"))
}

/// 日志文件：`<config>/logs/daemon.log`。
pub fn log_file_in(config_dir: &Path) -> PathBuf {
    config_dir.join("logs").join("daemon.log")
}

/// 终端历史根：`<config>/history`（#74）。
pub fn history_dir_in(config_dir: &Path) -> PathBuf {
    config_dir.join("history")
}

/// 命名管道名：**按当前用户隔离**（`\\.\pipe\ageminal-<sid hex>`）。
///
/// 名字带 SID 是「按当前用户隔离」的第一层；第二层是管道 DACL 只放当前用户（见 [`crate::acl`]）。
pub fn pipe_name() -> Result<String, DaemonError> {
    Ok(pipe_name_for(&win::current_user_sid()?))
}

/// 由 SID 字节派生管道名（便于单测，不必真去取令牌）。
pub fn pipe_name_for(sid: &[u8]) -> String {
    format!(r"\\.\pipe\ageminal-{}", win::hex(sid))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipe_names_are_user_scoped() {
        let a = pipe_name_for(&[1, 2, 3, 4]);
        let b = pipe_name_for(&[1, 2, 3, 5]);
        assert_eq!(a, r"\\.\pipe\ageminal-01020304");
        assert_ne!(a, b);
    }

    #[test]
    fn logs_hang_off_the_config_dir() {
        let config = Path::new(r"C:\Users\me\AppData\Local\Ageminal");
        assert_eq!(log_file_in(config), config.join("logs").join("daemon.log"));
    }

    #[test]
    fn history_hangs_off_the_config_dir() {
        let config = Path::new(r"C:\Users\me\AppData\Local\Ageminal");
        assert_eq!(history_dir_in(config), config.join("history"));
    }
}
