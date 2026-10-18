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
use tauri_plugin_dialog::DialogExt;
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
            path: crate::path_text(&entry.path),
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
            path: crate::path_text(&hit.path),
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
///
/// 返回 `Result`：async 命令带引用入参时 Tauri 要求如此（`State` 就是引用）。
#[tauri::command]
#[specta::specta]
pub async fn search_files(
    root: String,
    query: String,
    show_hidden: bool,
    show_ignored: bool,
    epoch: tauri::State<'_, Arc<SearchEpoch>>,
) -> Result<SearchResultView, String> {
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
    .map_err(|error| format!("搜索任务失败：{error}"))?;

    Ok(SearchResultView::from(outcome))
}

/// 内容类型（判定顺序的产物，#20 决议）。
#[derive(Debug, Clone, Copy, Serialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum PreviewKindView {
    Text,
    Image,
    Binary,
}

/// 文件预览的视图（与 `ageminal_files::Preview` 机械对应，扁平化以便生成简单 TS）。
///
/// 三个 `kind` 各自只填一部分字段：文本用 `text` 及其护栏位；图片用 `image_data_url` /
/// `image_too_large`；二进制只用 `total_bytes`。
#[derive(Debug, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PreviewView {
    pub kind: PreviewKindView,
    /// 文件总字节数（specta 禁止 `u64` 越界到 TS；预览元数据，> 4 GiB 饱和）。
    pub total_bytes: u32,
    /// 文本内容（`kind == text`）。
    pub text: Option<String>,
    /// 无效 UTF-8，已 lossy 解码（顶部警示，D2 #20）。
    pub lossy: bool,
    /// 命中字节上限或行数上限，文本已截断。
    pub truncated: bool,
    /// 回传文本的行数。
    pub lines: u32,
    /// 含超长行（> 100 KiB，顶部提示）。
    pub long_line: bool,
    /// 图片的 `data:` URL（`kind == image` 且未超上限）。
    pub image_data_url: Option<String>,
    /// 图片超过 20 MiB，未加载数据。
    pub image_too_large: bool,
}

/// `u64` 字节数 → TS 友好的 `u32`（specta 禁止 `u64`；预览元数据，> 4 GiB 饱和）。
fn total_bytes_u32(bytes: u64) -> u32 {
    u32::try_from(bytes).unwrap_or(u32::MAX)
}

impl From<ageminal_files::Preview> for PreviewView {
    fn from(preview: ageminal_files::Preview) -> Self {
        match preview {
            ageminal_files::Preview::Text(text) => Self {
                kind: PreviewKindView::Text,
                total_bytes: total_bytes_u32(text.total_bytes),
                text: Some(text.text),
                lossy: text.lossy,
                truncated: text.truncated,
                lines: text.lines as u32,
                long_line: text.long_line,
                image_data_url: None,
                image_too_large: false,
            },
            ageminal_files::Preview::Image(image) => Self {
                kind: PreviewKindView::Image,
                total_bytes: total_bytes_u32(image.total_bytes),
                text: None,
                lossy: false,
                truncated: false,
                lines: 0,
                long_line: false,
                image_data_url: image.data_url,
                image_too_large: image.too_large,
            },
            ageminal_files::Preview::Binary(binary) => Self {
                kind: PreviewKindView::Binary,
                total_bytes: total_bytes_u32(binary.total_bytes),
                text: None,
                lossy: false,
                truncated: false,
                lines: 0,
                long_line: false,
                image_data_url: None,
                image_too_large: false,
            },
        }
    }
}

/// 读取文件的只读预览（#60）。
///
/// 内容判定与护栏都在 `ageminal-files`（本机可测）；这里只做路径出入参与视图转换。读大图
/// 与 base64 编码可能耗时，放阻塞线程池上跑。
#[tauri::command]
#[specta::specta]
pub async fn read_preview(root: String, path: String) -> Result<PreviewView, String> {
    let root = PathBuf::from(root);
    let path = PathBuf::from(path);

    tauri::async_runtime::spawn_blocking(move || {
        ageminal_files::read_preview(&root, &path)
            .map(PreviewView::from)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("预览任务失败：{error}"))?
}

/// 用系统默认程序打开 `path`（D6 #20），路径须在 `root` 之内。
///
/// 走 `tauri-plugin-opener` 的 **Rust** API：不把 `plugin:opener|*` 放开给前端（capabilities
/// 保持最小），路径校验与「用外部编辑器打开」同源，等于把能力限定在工作区路径内。
#[tauri::command]
#[specta::specta]
pub fn open_in_editor(app: tauri::AppHandle, root: String, path: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;

    let root = PathBuf::from(root);
    let path = PathBuf::from(path);
    ageminal_files::ensure_within(&root, &path).map_err(|error| error.to_string())?;
    app.opener()
        .open_path(path.to_string_lossy().into_owned(), None::<&str>)
        .map_err(|error| error.to_string())
}

/// 新建文件 / 目录（#62）；返回新条目的绝对路径。`name` 是单一名（不含分隔符）。
#[tauri::command]
#[specta::specta]
pub fn create_entry(
    root: String,
    dir: String,
    name: String,
    is_dir: bool,
) -> Result<String, String> {
    let root = PathBuf::from(root);
    let dir = PathBuf::from(dir);
    let created = if is_dir {
        ageminal_files::create_dir(&root, &dir, &name)
    } else {
        ageminal_files::create_file(&root, &dir, &name)
    }
    .map_err(|error| error.to_string())?;
    Ok(crate::path_text(&created))
}

/// 就地重命名（#62）；返回新路径。
#[tauri::command]
#[specta::specta]
pub fn rename_entry(root: String, path: String, name: String) -> Result<String, String> {
    ageminal_files::rename(&PathBuf::from(root), &PathBuf::from(path), &name)
        .map(|target| crate::path_text(&target))
        .map_err(|error| error.to_string())
}

/// 删除到**回收站**（#62 / §9）；回收站不可用时显式报错，绝不永久删除。
#[tauri::command]
#[specta::specta]
pub fn delete_entry(root: String, path: String) -> Result<(), String> {
    ageminal_files::delete(&PathBuf::from(root), &PathBuf::from(path))
        .map_err(|error| error.to_string())
}

/// 写文本到系统剪贴板（复制路径 / 相对路径，#62）。
#[tauri::command]
#[specta::specta]
pub fn write_clipboard(app: tauri::AppHandle, text: String) -> Result<(), String> {
    // 走插件的 Rust API：capabilities 不放开 `clipboard-manager:*`。
    use tauri_plugin_clipboard_manager::ClipboardExt;

    app.clipboard()
        .write_text(text)
        .map_err(|error| error.to_string())
}

/// 把文件放进系统剪贴板（`CF_HDROP`，可在资源管理器粘贴；#62；Windows 专有）。
#[tauri::command]
#[specta::specta]
pub fn copy_files(root: String, paths: Vec<String>) -> Result<(), String> {
    let root = PathBuf::from(root);
    for path in &paths {
        ageminal_files::ensure_within(&root, &PathBuf::from(path))
            .map_err(|error| error.to_string())?;
    }
    write_file_list(&paths)
}

/// 在文件资源管理器中显示（#62；选中该条目）。
#[tauri::command]
#[specta::specta]
pub fn reveal_in_explorer(app: tauri::AppHandle, root: String, path: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;

    let root = PathBuf::from(root);
    let path = PathBuf::from(path);
    ageminal_files::ensure_within(&root, &path).map_err(|error| error.to_string())?;
    app.opener()
        .reveal_item_in_dir(path)
        .map_err(|error| error.to_string())
}

/// 在系统终端中打开 `dir`（#62；终端子系统落地前先用系统终端）。
#[tauri::command]
#[specta::specta]
pub fn open_terminal(root: String, dir: String) -> Result<(), String> {
    let root = PathBuf::from(root);
    let dir = PathBuf::from(dir);
    ageminal_files::ensure_within(&root, &dir).map_err(|error| error.to_string())?;
    spawn_terminal(&dir)
}

/// 弹系统文件选择器（#69：从「+」菜单新建文件预览页签）。
///
/// 根固定为当前 worktree；用户仍可导航到别处，所以选完要再校验一次 `ensure_within`。
/// 返回 `None` = 用户取消。
#[tauri::command]
#[specta::specta]
pub async fn pick_file(app: tauri::AppHandle, root: String) -> Result<Option<String>, String> {
    let root = PathBuf::from(root);
    if !root.is_dir() {
        return Err("当前 worktree 目录不存在".to_owned());
    }
    // 阻塞式选择器不得在主线程调用（插件文档）；async 命令跑在异步运行时上。
    let Some(file) = app
        .dialog()
        .file()
        .set_directory(&root)
        .blocking_pick_file()
    else {
        return Ok(None);
    };
    let path = file.into_path().map_err(|error| error.to_string())?;
    ageminal_files::ensure_within(&root, &path).map_err(|error| error.to_string())?;
    Ok(Some(crate::path_text(&path)))
}

/// `CF_HDROP` 写文件列表（Windows；见 `clipboard-win`）。其它平台显式报错。
#[cfg(windows)]
fn write_file_list(paths: &[String]) -> Result<(), String> {
    // `write_clipboard` 要求剪贴板已打开，且要在写期间保持打开。
    use clipboard_win::formats::FileList;
    use clipboard_win::{Clipboard, Setter};

    let _clip = Clipboard::new_attempts(10).map_err(|error| error.to_string())?;
    FileList
        .write_clipboard(paths)
        .map_err(|error| error.to_string())
}

#[cfg(not(windows))]
fn write_file_list(_paths: &[String]) -> Result<(), String> {
    Err("复制文件到剪贴板仅支持 Windows".to_owned())
}

/// 开一个系统终端并 `cd` 到 `dir`：优先 Windows Terminal，回退 `cmd.exe`。
#[cfg(windows)]
fn spawn_terminal(dir: &std::path::Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    // CREATE_NEW_CONSOLE：GUI 进程没有控制台，`cmd` 得开个新的。
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

    if std::process::Command::new("wt.exe")
        .arg("-d")
        .arg(dir)
        .spawn()
        .is_ok()
    {
        return Ok(());
    }
    std::process::Command::new("cmd.exe")
        .arg("/K")
        .arg("cd")
        .arg("/d")
        .arg(dir)
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[cfg(not(windows))]
fn spawn_terminal(_dir: &std::path::Path) -> Result<(), String> {
    Err("在终端中打开当前仅支持 Windows".to_owned())
}
