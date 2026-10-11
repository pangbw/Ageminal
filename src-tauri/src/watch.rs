//! 文件变更监听的 Tauri 接线（#61）。
//!
//! 纯逻辑在 `ageminal-files::watch`（本机可测）；这里只做「命令入参 + State 持有 + 事件回推」。
//!
//! 前端在页签集合变化时用 [`set_watched_files`] 下发当前打开的预览文件，命中变更经
//! `fs-change` 事件回推（`paths` 为绝对路径）——**测不测、刷新不刷新由前端决定**（#61：
//! 正可见立即刷新、后台标 stale）。

use std::path::PathBuf;

use ageminal_files::watch::FileWatcher;
use serde::Serialize;

/// 回推给前端的一批文件变更。
#[derive(Debug, Clone, Serialize)]
pub struct FsChange {
    /// 变更文件的绝对路径（已去重）。
    pub paths: Vec<String>,
}

/// 监听器的持有者（Tauri `State`）。
pub struct FileWatch(pub FileWatcher);

/// 用前端当前打开着的预览文件替换监听集合（#61）。
#[tauri::command]
#[specta::specta]
pub fn set_watched_files(
    paths: Vec<String>,
    state: tauri::State<'_, FileWatch>,
) -> Result<(), String> {
    let files: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    state.set_files(&files).map_err(|error| error.to_string())
}
