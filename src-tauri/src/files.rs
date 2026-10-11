//! 文件树命令面（#58）：列举目录并回带可见性标记。
//!
//! 纯逻辑在 `ageminal-files`（本机可测）；这里只做「路径出入参 + 视图类型转换」。
//!
//! 视图类型与 `ageminal-files::Entry` 字段一一对应、由 [`From`] 机械转换：
//! §19 把「app ↔ 前端」这条类型边界划归 app-core，核心 crate 因此不引 specta。

use std::path::PathBuf;

use serde::Serialize;
use specta::Type;

/// 目录里的一个条目（与 `ageminal-files::Entry` 字段一一对应）。
#[derive(Debug, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DirEntryView {
    /// 文件名。
    pub name: String,
    /// 绝对路径。
    pub path: String,
    /// 是否目录。
    pub is_dir: bool,
    /// 维度 2：dotfile（名字以 `.` 开头）。
    pub hidden: bool,
    /// 维度 3：被忽略规则命中（`.gitignore` / `.git/info/exclude` / 全局 excludes）。
    pub ignored: bool,
}

impl From<ageminal_files::Entry> for DirEntryView {
    fn from(entry: ageminal_files::Entry) -> Self {
        Self {
            name: entry.name,
            path: entry.path.display().to_string(),
            is_dir: entry.is_dir,
            hidden: entry.hidden,
            ignored: entry.ignored,
        }
    }
}

/// 列举 `dir`（须在 worktree 根 `root` 之内）下的条目，带可见性标记。
///
/// `.git` 永远不会出现在结果里；过滤由前端按「显示隐藏文件 / 显示忽略文件」两个开关做
/// （#18 D4：切开关不必重扫）。
#[tauri::command]
#[specta::specta]
pub fn read_dir(root: String, dir: String) -> Result<Vec<DirEntryView>, String> {
    ageminal_files::read_dir(&PathBuf::from(root), &PathBuf::from(dir))
        .map(|entries| entries.into_iter().map(DirEntryView::from).collect())
        .map_err(|error| error.to_string())
}
