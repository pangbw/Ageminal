//! 文件树命令面（#58 列举 / #59 搜索）。
//!
//! 纯逻辑在 `ageminal-files`（本机可测）；这里只做「路径出入参 + 视图类型转换 + 取消状态」。
//!
//! 视图类型与核心类型字段一一对应、由 [`From`] 机械转换：§19 把「app ↔ 前端」这条类型边界
//! 划归 app-core，核心 crate 因此不引 specta。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use ageminal_files::search::SearchOptions;
use serde::Serialize;
use specta::Type;

/// 搜索的墙钟预算（#19 D8）：用完返回部分结果并置 `timedOut`。
const SEARCH_BUDGET: Duration = Duration::from_secs(2);

/// 搜索取消用（#19 D8「新查询取消旧遍历」）。
///
/// 每次查询把 epoch 加一；旧遍历发现自己那一份不再是最新就停手。前端另有 generation 丢弃
/// 过期响应，两者互补：这里省的是 CPU，前端省的是错序渲染。
#[derive(Default)]
pub struct SearchEpoch(AtomicU64);

impl SearchEpoch {
    fn next(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

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

/// 一个目录的列举结果。
#[derive(Debug, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DirListingView {
    pub entries: Vec<DirEntryView>,
    /// 条目数超过单目录上限（5000），已截断。
    pub truncated: bool,
}

/// 一条搜索命中（与 `ageminal-files::SearchHit` 字段一一对应）。
#[derive(Debug, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SearchHitView {
    /// 绝对路径。
    pub path: String,
    /// 相对工作树根的展示路径（`/` 分隔）。
    pub relative: String,
    pub is_dir: bool,
    /// 被忽略规则命中（仅「显示忽略文件」打开时才可能为 `true`）。
    pub ignored: bool,
}

/// 一次搜索的结果。
#[derive(Debug, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SearchResultView {
    pub hits: Vec<SearchHitView>,
    /// 命中数达到上限（200）。
    pub truncated: bool,
    /// 预算用尽，结果是部分的。
    pub timed_out: bool,
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

impl From<ageminal_files::DirListing> for DirListingView {
    fn from(listing: ageminal_files::DirListing) -> Self {
        Self {
            entries: listing
                .entries
                .into_iter()
                .map(DirEntryView::from)
                .collect(),
            truncated: listing.truncated,
        }
    }
}

impl From<ageminal_files::SearchHit> for SearchHitView {
    fn from(hit: ageminal_files::SearchHit) -> Self {
        Self {
            path: hit.path.display().to_string(),
            relative: hit.relative,
            is_dir: hit.is_dir,
            ignored: hit.ignored,
        }
    }
}

impl From<ageminal_files::SearchOutcome> for SearchResultView {
    fn from(outcome: ageminal_files::SearchOutcome) -> Self {
        Self {
            hits: outcome.hits.into_iter().map(SearchHitView::from).collect(),
            truncated: outcome.truncated,
            timed_out: outcome.timed_out,
        }
    }
}

/// 列举 `dir`（须在 worktree 根 `root` 之内）下的条目，带可见性标记。
///
/// `.git` 永远不会出现在结果里；过滤由前端按「显示隐藏文件 / 显示忽略文件」两个开关做
/// （#18 D4：切开关不必重扫）。
#[tauri::command]
#[specta::specta]
pub fn read_dir(root: String, dir: String) -> Result<DirListingView, String> {
    ageminal_files::read_dir(&PathBuf::from(root), &PathBuf::from(dir))
        .map(DirListingView::from)
        .map_err(|error| error.to_string())
}

/// 按名字（大小写不敏感的子串）在 `root` 下全树搜索。
///
/// 在阻塞线程池上跑（遍历可能耗时）；新查询会让旧的一停手（`SearchEpoch`）。`~2 s` 预算
/// 用尽返回部分结果，`timedOut` 置位。
#[tauri::command]
#[specta::specta]
pub async fn search_files(
    root: String,
    query: String,
    show_hidden: bool,
    show_ignored: bool,
    epoch: tauri::State<'_, Arc<SearchEpoch>>,
) -> SearchResultView {
    let mine = epoch.next();
    let shared = epoch.inner().clone();
    let root = PathBuf::from(root);

    let outcome = tauri::async_runtime::spawn_blocking(move || {
        ageminal_files::search(
            &root,
            &query,
            SearchOptions {
                show_hidden,
                show_ignored,
            },
            SEARCH_BUDGET,
            move || shared.get() == mine,
        )
    })
    .await
    .unwrap_or_default();

    SearchResultView::from(outcome)
}
