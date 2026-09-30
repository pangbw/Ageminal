//! 持久化内核（REQUIREMENTS.md §14）：`%LOCALAPPDATA%\Ageminal\` 下的 JSON 文档。
//!
//! - 单一根目录，按文件分权（`settings.json` / `state.json`），**不需要文件锁**；
//! - 原子写（临时文件 → fsync → rename）；
//! - 每个文件带整数 `schemaVersion`，有序幂等迁移链，前向兼容可写 + 保留未知字段；
//! - 损坏 / 迁移失败备份进 `backups\`（`.corrupt-<ts>` / `.migrate-failed-<ts>`）并重建，不阻塞启动；
//! - 设置类**立即写**，状态类**去抖 ~500 ms**，退出前 [`Store::flush`]。

mod atomic;
mod doc;
mod settings;
mod state;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub use settings::{General, Settings};
pub use state::{AppState, WindowState};

/// 应用数据目录名（`%LOCALAPPDATA%\Ageminal\`）。
pub const APP_DIR: &str = "Ageminal";

/// 状态类文档的去抖窗口。
const STATE_DEBOUNCE: Duration = Duration::from_millis(500);

/// 加载期间产生的提示，由上层决定是否一次性告知用户。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// 文档由更高版本写入：可写，未知字段原样保留。
    WrittenByNewerVersion {
        file: String,
        found: u32,
        current: u32,
    },
    /// 文档损坏，已备份并重建。
    CorruptRecovered { file: String, backup: PathBuf },
    /// 迁移失败，已备份并重建。
    MigrationFailed {
        file: String,
        backup: PathBuf,
        reason: String,
    },
}

/// 持久化错误。
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// 环境缺少 `%LOCALAPPDATA%`，无法定位应用数据目录。
    #[error("环境变量 %LOCALAPPDATA% 缺失，无法定位应用数据目录")]
    NoLocalAppData,
    /// 文件读写失败。
    #[error("{file} 读写失败：{source}")]
    Io {
        file: String,
        #[source]
        source: std::io::Error,
    },
    /// 文档序列化失败。
    #[error("{file} 序列化失败：{source}")]
    Serialize {
        file: String,
        #[source]
        source: serde_json::Error,
    },
}

impl StoreError {
    pub(crate) fn io(file: &str, source: std::io::Error) -> Self {
        Self::Io {
            file: file.to_owned(),
            source,
        }
    }

    pub(crate) fn serialize(file: &str, source: serde_json::Error) -> Self {
        Self::Serialize {
            file: file.to_owned(),
            source,
        }
    }
}

/// 应用持久化的入口。
pub struct Store {
    root: PathBuf,
    settings: Settings,
    state: AppState,
    dirty_state_since: Option<Instant>,
    notices: Vec<Notice>,
}

impl Store {
    /// 打开指定根目录（测试与多环境用）。
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let root = root.into();
        let (settings, mut notices) = doc::load::<Settings>(&root)?;
        let (state, state_notices) = doc::load::<AppState>(&root)?;
        notices.extend(state_notices);
        Ok(Self {
            root,
            settings,
            state,
            dirty_state_since: None,
            notices,
        })
    }

    /// 打开 `%LOCALAPPDATA%\Ageminal\`。
    pub fn open_default() -> Result<Self, StoreError> {
        Self::open(default_root()?)
    }

    /// `%LOCALAPPDATA%\Ageminal\`。
    pub fn default_root() -> Result<PathBuf, StoreError> {
        default_root()
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn state(&self) -> &AppState {
        &self.state
    }

    /// 加载期间的提示。
    pub fn notices(&self) -> &[Notice] {
        &self.notices
    }

    /// 取走提示（调用方负责只提示一次）。
    pub fn take_notices(&mut self) -> Vec<Notice> {
        std::mem::take(&mut self.notices)
    }

    /// 修改设置并**立即落盘**。
    pub fn update_settings(&mut self, apply: impl FnOnce(&mut Settings)) -> Result<(), StoreError> {
        apply(&mut self.settings);
        doc::save(&self.root, &self.settings)
    }

    /// 修改状态；落盘推迟到 [`Store::flush_state_if_due`] 或 [`Store::flush`]。
    pub fn update_state(
        &mut self,
        apply: impl FnOnce(&mut AppState),
        now: Instant,
    ) -> Result<(), StoreError> {
        apply(&mut self.state);
        if self.dirty_state_since.is_none() {
            self.dirty_state_since = Some(now);
        }
        Ok(())
    }

    /// 去抖窗口（~500 ms）已过则落盘；返回是否真的写了。
    pub fn flush_state_if_due(&mut self, now: Instant) -> Result<bool, StoreError> {
        match self.dirty_state_since {
            Some(since) if now.saturating_duration_since(since) >= STATE_DEBOUNCE => {
                doc::save(&self.root, &self.state)?;
                self.dirty_state_since = None;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// 退出前 flush：把尚未落盘的状态写掉。
    pub fn flush(&mut self) -> Result<(), StoreError> {
        if self.dirty_state_since.is_some() {
            doc::save(&self.root, &self.state)?;
            self.dirty_state_since = None;
        }
        Ok(())
    }
}

fn default_root() -> Result<PathBuf, StoreError> {
    let base = std::env::var_os("LOCALAPPDATA").ok_or(StoreError::NoLocalAppData)?;
    Ok(PathBuf::from(base).join(APP_DIR))
}
