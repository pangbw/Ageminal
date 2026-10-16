//! 发现文件 `daemon.json`——app-core ↔ daemon 的**契约之一**（研究稿 §6 的前置：
//! 客户端得先知道「管道在哪、令牌是什么」才谈得上握手）。
//!
//! **这个文件格式只有这一处拥有**：读、写、删都在这里。daemon 侧写、客户端侧读，两边共用同一份
//! schema，改格式只动这个模块。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

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
#[serde(rename_all = "camelCase")]
pub struct DaemonInfo {
    /// daemon 进程 id。
    pub pid: u32,

    /// 命名管道完整路径（`\\.\pipe\…`）。
    pub pipe: String,

    /// 握手令牌。每次 daemon 启动重生成，进程退出即失效。
    pub token: String,

    /// daemon 的协议版本（[`crate::PROTOCOL_VERSION`]）。
    pub protocol: u16,

    /// daemon 的构建版本。
    pub daemon_version: String,

    /// 启动时刻（Unix 毫秒）。
    pub started_at_ms: u64,
}

impl DaemonInfo {
    /// 读 `daemon.json`。
    pub fn read(config_dir: &Path) -> Result<Self, DiscoveryError> {
        let path = state_file_in(config_dir);
        let bytes = std::fs::read(&path).map_err(|source| DiscoveryError::Io {
            path: path.clone(),
            source,
        })?;
        serde_json::from_slice(&bytes).map_err(|error| DiscoveryError::Format {
            path,
            reason: error.to_string(),
        })
    }

    /// **原子**写 `daemon.json`：先写 `.tmp` 再改名，读方永远看不到半截 JSON。
    pub fn write(&self, config_dir: &Path) -> Result<(), DiscoveryError> {
        let path = state_file_in(config_dir);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| DiscoveryError::Io {
                path: path.clone(),
                source,
            })?;
        }
        let bytes = serde_json::to_vec_pretty(self).map_err(|error| DiscoveryError::Format {
            path: path.clone(),
            reason: error.to_string(),
        })?;
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, bytes).map_err(|source| DiscoveryError::Io {
            path: temporary.clone(),
            source,
        })?;
        std::fs::rename(&temporary, &path).map_err(|source| DiscoveryError::Io { path, source })
    }

    /// 删掉状态文件（daemon 停机时）。
    pub fn remove(config_dir: &Path) -> std::io::Result<()> {
        std::fs::remove_file(state_file_in(config_dir))
    }
}

/// 发现文件读写错误。
#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    /// 文件读写失败。
    #[error("发现文件读写失败 {path}：{source}")]
    Io {
        /// 出问题的路径。
        path: PathBuf,
        /// 底层错误。
        #[source]
        source: std::io::Error,
    },

    /// 内容不是合法 JSON / 字段对不上。
    #[error("发现文件内容无法解析 {path}：{reason}")]
    Format {
        /// 出问题的路径。
        path: PathBuf,
        /// 解析失败的理由。
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> DaemonInfo {
        DaemonInfo {
            pid: 4242,
            pipe: r"\\.\pipe\ageminal-0102".to_owned(),
            token: "deadbeef".to_owned(),
            protocol: crate::PROTOCOL_VERSION,
            daemon_version: "0.0.0".to_owned(),
            started_at_ms: 1_700_000_000_000,
        }
    }

    #[test]
    fn round_trips() {
        let dir = std::env::temp_dir().join(format!("ageminal-discovery-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let info = sample();
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

    #[test]
    fn malformed_file_is_an_error() {
        let dir =
            std::env::temp_dir().join(format!("ageminal-discovery-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(state_file_in(&dir), b"{ nope").unwrap();

        assert!(matches!(
            DaemonInfo::read(&dir),
            Err(DiscoveryError::Format { .. })
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
