//! 工作区命令面（#57）：项目增删、侧栏成员、折叠、显示名、激活项。
//!
//! 纯逻辑在 `ageminal-workspace`（本机可测）；这里只做「调 git 发现 → 投影 → 落盘 →
//! 回快照」，加上系统目录选择器（#32 D1：`dialog` 插件**只从 Rust 侧调用**）。
//!
//! 快照即「回推」：每个改动命令都返回**新的完整快照**，前端整体替换投影
//! （§14 / #16 D5：单一事实来源 = Rust，前端不做乐观更新）。§14 写的是「回推事件」，
//! 这里用命令返回值承担同一件事——少一条事件通道、也就少一处漂移；多窗口（🕓 延后）再论。
//!
//! 视图类型与 `ageminal-workspace` 的对应类型字段一一对应、由 [`From`] 机械转换：
//! §19 把「app ↔ 前端」这条类型边界划归 app-core，核心 crate 因此不引 specta。
//!
//! ⚠️ specta 会把 **`Option<命名类型>` 内联展开**成匿名对象字面量（`Result<Option<T>, E>`
//! 的返回值就是这种），而 `Result<T, E>` 里引用的是命名类型。也就是说 `pnpm bindings`
//! 生成出来的形状可能比手写的「啰嗦」——**以生成物为准**，别为了绑定好看去改 Rust 签名。

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::Instant;

use ageminal_git::Git;
use ageminal_workspace::view::Discovery;
use ageminal_workspace::{project_view, Project, ProjectSettings, WorktreeName};
use serde::Serialize;
use specta::Type;
use tauri_plugin_dialog::DialogExt;

use crate::storage::{AppState, Settings, Store};

/// 启动时定位一次的 git。
///
/// 定位失败（未安装 / 版本过低）在这里定型，由快照转成每个项目的明确提示（§15），
/// 而不是让每条命令都去重试。
pub struct GitTool(Result<Git, String>);

impl GitTool {
    /// 定位 git；失败则记住错误信息。
    pub fn locate() -> Self {
        Self(Git::locate(None).map_err(|error| error.to_string()))
    }
}

/// 前端唯一的工作区事实来源。
#[derive(Debug, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSnapshot {
    pub projects: Vec<ProjectView>,
    /// 激活 worktree 的路径；`None` = 没有激活项。
    pub active_worktree: Option<String>,
}

/// 一个项目的视图（侧栏分组 + 设置页「项目」分页共用，#16 D6）。
#[derive(Debug, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProjectView {
    pub path: String,
    pub name: String,
    pub collapsed: bool,
    /// 发现到的**全部** worktree；侧栏只渲染 `pinned` 的那些，设置页要全量。
    pub worktrees: Vec<WorktreeView>,
    /// git 发现失败时的明确提示；成功为 `None`。
    pub error: Option<String>,
}

/// 一个 worktree 的视图。
///
/// 不含 agent 状态：那是运行期事件（#79 / #80），不属于工作区的落盘事实。
#[derive(Debug, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeView {
    pub path: String,
    /// 显示名：本地覆盖 > 默认（主工作树用项目名，其余用目录 basename）。
    pub name: String,
    pub branch_label: Option<String>,
    pub is_main: bool,
    pub bare: bool,
    pub locked: bool,
    pub detached: bool,
    pub stale: bool,
    pub pinned: bool,
}

impl From<ageminal_workspace::ProjectView> for ProjectView {
    fn from(view: ageminal_workspace::ProjectView) -> Self {
        Self {
            path: crate::path_text(&view.path),
            name: view.name,
            collapsed: view.collapsed,
            worktrees: view.worktrees.into_iter().map(WorktreeView::from).collect(),
            error: view.error,
        }
    }
}

impl From<ageminal_workspace::WorktreeView> for WorktreeView {
    fn from(view: ageminal_workspace::WorktreeView) -> Self {
        Self {
            path: crate::path_text(&view.path),
            name: view.name,
            branch_label: view.branch_label,
            is_main: view.is_main,
            bare: view.bare,
            locked: view.locked,
            detached: view.detached,
            stale: view.stale,
            pinned: view.pinned,
        }
    }
}

/// 启动 hydrate：前端拿到的唯一工作区事实。
#[tauri::command]
#[specta::specta]
pub fn workspace_snapshot(
    store: tauri::State<'_, Mutex<Store>>,
    git: tauri::State<'_, GitTool>,
) -> Result<WorkspaceSnapshot, String> {
    let store = lock(&store)?;
    Ok(build_snapshot(&store, git.inner()))
}

/// 弹系统目录选择器，把选中的仓库加为项目（自动发现其 worktree 并**全部加入侧栏**）。
///
/// 返回 `None` = 用户取消。
#[tauri::command]
#[specta::specta]
pub async fn add_project(
    app: tauri::AppHandle,
    store: tauri::State<'_, Mutex<Store>>,
    git: tauri::State<'_, GitTool>,
) -> Result<Option<WorkspaceSnapshot>, String> {
    // 阻塞式选择器不得在主线程调用（插件文档）；async 命令跑在异步运行时上。
    let Some(folder) = app.dialog().file().blocking_pick_folder() else {
        return Ok(None);
    };
    let path = folder.into_path().map_err(|error| error.to_string())?;

    let tool = git.inner();
    let mut store = lock(&store)?;
    if store
        .state()
        .projects
        .iter()
        .any(|project| project.path == path)
    {
        return Err("这个仓库已经在项目里了".to_owned());
    }

    // 加入即全量 pin（#57 验收「加仓库后侧栏出现其 worktree」），所以要先跑一次发现。
    let git = tool.0.as_ref().map_err(|message| message.clone())?;
    let name = Project::default_name(&path);
    let discovered = git
        .discover(&path, &name)
        .map_err(|error| error.to_string())?;
    let project = Project::new(path, name, &discovered);

    store
        .update_state(|state| state.projects.push(project), Instant::now())
        .map_err(|error| error.to_string())?;
    Ok(Some(build_snapshot(&store, tool)))
}

/// 移除项目：连同它的侧栏名单与项目级设置一起清掉。
#[tauri::command]
#[specta::specta]
pub fn remove_project(
    store: tauri::State<'_, Mutex<Store>>,
    git: tauri::State<'_, GitTool>,
    path: String,
) -> Result<WorkspaceSnapshot, String> {
    let target = PathBuf::from(&path);
    let mut store = lock(&store)?;

    // 先判「激活项是否落在被移除的项目里」（按路径分量，见 `Project::owns`），
    // 免得在闭包里同时借 `projects` 又改 `active_worktree`。
    let clears_active = store
        .state()
        .projects
        .iter()
        .find(|project| project.path == target)
        .zip(store.state().active_worktree.as_ref())
        .is_some_and(|(project, active)| project.owns(active));

    store
        .update_state(
            |state| {
                let removed = state
                    .projects
                    .iter()
                    .find(|project| project.path == target)
                    .cloned();
                state.projects.retain(|project| project.path != target);
                if let Some(project) = removed {
                    state.tabs.retain(|group| !project.owns(&group.root));
                    state.launch_specs.retain(|spec| !project.owns(&spec.cwd));
                }
                if clears_active {
                    state.active_worktree = None;
                }
            },
            Instant::now(),
        )
        .map_err(|error| error.to_string())?;

    store
        .update_settings(|settings| settings.projects.retain(|entry| entry.path != target))
        .map_err(|error| error.to_string())?;

    Ok(build_snapshot(&store, git.inner()))
}

/// 折叠 / 展开一个项目分组（侧栏状态，`state.json`，去抖落盘）。
#[tauri::command]
#[specta::specta]
pub fn set_project_collapsed(
    store: tauri::State<'_, Mutex<Store>>,
    git: tauri::State<'_, GitTool>,
    project_path: String,
    collapsed: bool,
) -> Result<WorkspaceSnapshot, String> {
    let target = PathBuf::from(&project_path);

    commit_state(&store, git.inner(), |state| {
        if let Some(project) = state
            .projects
            .iter_mut()
            .find(|project| project.path == target)
        {
            project.collapsed = collapsed;
        }
    })
}

/// 把 worktree 加入 / 移出左侧栏。
#[tauri::command]
#[specta::specta]
pub fn set_worktree_pinned(
    store: tauri::State<'_, Mutex<Store>>,
    git: tauri::State<'_, GitTool>,
    project_path: String,
    worktree_path: String,
    pinned: bool,
) -> Result<WorkspaceSnapshot, String> {
    let target = PathBuf::from(&project_path);
    let worktree = PathBuf::from(&worktree_path);

    commit_state(&store, git.inner(), |state| {
        if let Some(project) = state
            .projects
            .iter_mut()
            .find(|project| project.path == target)
        {
            project.set_pinned(&worktree, pinned);
        }
    })
}

/// 改 worktree 的本地显示名；`name` 为 `None`（或只有空白）= 回到默认名。
#[tauri::command]
#[specta::specta]
pub fn rename_worktree(
    store: tauri::State<'_, Mutex<Store>>,
    git: tauri::State<'_, GitTool>,
    project_path: String,
    worktree_path: String,
    name: Option<String>,
) -> Result<WorkspaceSnapshot, String> {
    let project = PathBuf::from(&project_path);
    let worktree = PathBuf::from(&worktree_path);
    let mut store = lock(&store)?;

    // 显示名是**设置类**（`settings.json`，§14）→ 立即落盘。
    store
        .update_settings(|settings| {
            let name = ProjectSettings::normalize_name(name.as_deref());
            match settings
                .projects
                .iter_mut()
                .find(|entry| entry.path == project)
            {
                Some(entry) => entry.rename(&worktree, name),
                // 只是「恢复默认名」的话，不必凭空建一个表项。
                None if name.is_some() => {
                    let mut entry = ProjectSettings::new(project.clone());
                    entry.rename(&worktree, name);
                    settings.projects.push(entry);
                }
                None => {}
            }
            // 空表项没有保留价值（#86 的项目级设置落进来之前）。
            settings.projects.retain(|entry| !entry.is_empty());
        })
        .map_err(|error| error.to_string())?;

    Ok(build_snapshot(&store, git.inner()))
}

/// 设置激活的 worktree（窗口标题与侧栏高亮都跟着它走）。
#[tauri::command]
#[specta::specta]
pub fn set_active_worktree(
    store: tauri::State<'_, Mutex<Store>>,
    git: tauri::State<'_, GitTool>,
    worktree_path: Option<String>,
) -> Result<WorkspaceSnapshot, String> {
    let target = worktree_path.map(PathBuf::from);

    commit_state(&store, git.inner(), |state| state.active_worktree = target)
}

/// 状态类改动的统一收尾：改完 `state.json`（去抖落盘）→ 回新快照。
fn commit_state(
    store: &Mutex<Store>,
    tool: &GitTool,
    apply: impl FnOnce(&mut AppState),
) -> Result<WorkspaceSnapshot, String> {
    let mut guard = lock(store)?;
    guard
        .update_state(apply, Instant::now())
        .map_err(|error| error.to_string())?;
    Ok(build_snapshot(&guard, tool))
}

fn lock(store: &Mutex<Store>) -> Result<MutexGuard<'_, Store>, String> {
    store.lock().map_err(|_| "工作区状态锁已被污染".to_owned())
}

/// 组装快照：对每个项目跑一次 git 发现，再投影。
fn build_snapshot(store: &Store, tool: &GitTool) -> WorkspaceSnapshot {
    let settings = store.settings();
    let state = store.state();

    let projects = state
        .projects
        .iter()
        .map(|project| {
            let discovery = discover(&tool.0, project);
            let names = names_for(&project.path, settings);
            ProjectView::from(project_view(project, &discovery, &names))
        })
        .collect();

    WorkspaceSnapshot {
        projects,
        active_worktree: state
            .active_worktree
            .as_ref()
            .map(|path| crate::path_text(path)),
    }
}

/// 一次发现：git 不可用 / 仓库读不了都转成**可显示的提示**，而不是让整个快照失败。
fn discover(tool: &Result<Git, String>, project: &Project) -> Discovery {
    let git = match tool {
        Ok(git) => git,
        Err(message) => {
            return Discovery::Failed {
                message: message.clone(),
            }
        }
    };
    match git.discover(&project.path, &project.name) {
        Ok(worktrees) => Discovery::Found(worktrees),
        Err(error) => Discovery::Failed {
            message: error.to_string(),
        },
    }
}

fn names_for(project: &Path, settings: &Settings) -> Vec<WorktreeName> {
    settings
        .projects
        .iter()
        .find(|entry| entry.path == project)
        .map(|entry| entry.worktree_names.clone())
        .unwrap_or_default()
}
