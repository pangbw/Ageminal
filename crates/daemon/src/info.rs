//! daemon 的发现信息（落盘为 `daemon.json`）。
//!
//! **这个文件格式只有这一处拥有**：读、写、删都在这里。daemon 侧写、客户端侧读，
//! 两边共用同一份 schema，改格式只动这个模块。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::DaemonError;

/// 状态文件名（配置根下）。
pub const STATE_FILE_NAME: &str = "daemon.json";

/// 状态文件落点：`<config_dir>/daemon.json`。
pub fn state_file_in(config_dir: &Path) -> PathBuf {
    config_dir.join(STATE_FILE_NAME)
}

/// daemon 的发现信息：客户端靠它拿到 **管道名 + 握手令牌**。
///
/// 为什么走文件而不是让 app 定义管道名：令牌要**每次 daemon 启动重生成**（REQUIREMENTS.md
/// §10.5 的 `AGEMINAL_TOKEN`），只能由 daemon 自己生成再告诉别人。`pid` / `started_at_ms`
/// 用来判断读到的是不是**当前世代**——上一世代留下的文件不能拿去握手（令牌已失效）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonInfo {
    /// daemon 进程 id。
    pub pid: u32,

    /// 命名管道完整路径（`\\.\pipe\…`）。
    pub pipe: String,

    /// 握手令牌。每次 daemon 启动重生成，进程退出即失效。
    pub token: String,

    /// daemon 的协议版本（`ageminal_protocol::PROTOCOL_VERSION`）。
    pub protocol: u32,

    /// daemon 的 crate 版本。
    pub version: String,

    /// 启动时刻（Unix 毫秒）。
    pub started_at_ms: u64,
}

impl DaemonInfo {
    /// 读 `daemon.json`。
    pub fn read(config_dir: &Path) -> Result<Self, DaemonError> {
        let path = state_file_in(config_dir);
        let bytes = std::fs::read(&path).map_err(|source| DaemonError::State {
            path: path.clone(),
            source,
        })?;
        serde_json::from_slice(&bytes).map_err(|error| DaemonError::Codec {
            reason: error.to_string(),
        })
    }

    /// **原子**写 `daemon.json`：先写 `.tmp` 再改名，读方永远看不到半截 JSON。
    pub fn write(&self, config_dir: &Path) -> Result<(), DaemonError> {
        let path = state_file_in(config_dir);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| DaemonError::State {
                path: path.clone(),
                source,
            })?;
        }
        let bytes = serde_json::to_vec_pretty(self).map_err(|error| DaemonError::Codec {
            reason: error.to_string(),
        })?;
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, bytes).map_err(|source| DaemonError::State {
            path: temporary.clone(),
            source,
        })?;
        std::fs::rename(&temporary, &path).map_err(|source| DaemonError::State { path, source })
    }

    /// 删掉状态文件（daemon 停机时）。
    pub fn remove(config_dir: &Path) -> std::io::Result<()> {
        std::fs::remove_file(state_file_in(config_dir))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 落盘格式的往返（跨平台可测，不必起真进程）。
    #[test]
    fn state_file_round_trips() {
        let dir = std::env::temp_dir().join(format!("ageminal-daemon-info-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let info = DaemonInfo {
            pid: 4242,
            pipe: r"\\.\pipe\ageminal-0102".to_owned(),
            token: "deadbeef".to_owned(),
            protocol: 1,
            version: "0.0.0".to_owned(),
            started_at_ms: 1_700_000_000_000,
        };
        info.write(&dir).unwrap();
        assert_eq!(DaemonInfo::read(&dir).unwrap(), info);

        // 写是原子的：不留 `.tmp` 残骸。
        assert!(std::fs::read_dir(&dir).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp")));

        DaemonInfo::remove(&dir).unwrap();
        assert!(DaemonInfo::read(&dir).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
