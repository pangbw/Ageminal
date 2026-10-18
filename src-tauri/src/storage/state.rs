//! `state.json`：项目、侧栏、页签骨架、窗口尺寸 / 位置等（REQUIREMENTS.md §14）。
//!
//! # 页签骨架与 launch spec（#78）
//!
//! - [`WorktreeTabs`] 是前端页签的持久骨架：每个 worktree 的页签顺序、激活项。
//! - [`LaunchSpec`] 按 `sid` 保存**启动参数**：cwd / shell / 原始命令 / agent id /
//!   `agentSessionId` / 最近世代。daemon 只存运行态；daemon 死了以后重建仍靠这里。
//! - 两者通过 `sid` 关联：页签不复制会话语义，launch spec 也不存页签顺序（D3 #30）。

use std::path::PathBuf;

use ageminal_workspace::Project;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::doc::{Document, Migration};

/// 当前代码能写出的状态版本。
///
/// **加字段不改版本**：新字段都带 `#[serde(default)]`，旧文档照读、未知字段靠 `extra`
/// 原样保留（#53 的约定）。只有**既有字段的类型或语义变化**才 +1（如 `settings` 里
/// `general.language` 的 1 → 2）——那才需要迁移链或「备份 + 重置」。
pub(crate) const CURRENT_STATE_VERSION: u32 = 1;

/// 应用状态文档。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppState {
    #[serde(default = "current_version")]
    pub schema_version: u32,
    /// 窗口尺寸与位置（关闭时保存、启动时恢复）。
    #[serde(default)]
    pub window: Option<WindowState>,
    /// 已加入的项目（#57）；worktree 列表不落盘，只在 `pinned` 里记「已加入侧栏」的。
    #[serde(default)]
    pub projects: Vec<Project>,
    /// 最后激活的 worktree（§14）。
    #[serde(default)]
    pub active_worktree: Option<PathBuf>,
    /// 每个 worktree 的页签骨架（#78）。
    #[serde(default)]
    pub tabs: Vec<WorktreeTabs>,
    /// 按 `sid` 索引的 launch spec（#78）；与 `tabs` 里的终端 `sid` 关联。
    #[serde(default)]
    pub launch_specs: Vec<LaunchSpec>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_STATE_VERSION,
            window: None,
            projects: Vec::new(),
            active_worktree: None,
            tabs: Vec::new(),
            launch_specs: Vec::new(),
            extra: Map::new(),
        }
    }
}

/// 窗口几何。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowState {
    pub width: f64,
    pub height: f64,
    #[serde(default)]
    pub x: Option<f64>,
    #[serde(default)]
    pub y: Option<f64>,
    #[serde(default)]
    pub maximized: bool,
    /// 未知字段原样保留：前向兼容在**嵌套层**同样成立。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 一个 worktree 的页签骨架。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeTabs {
    /// worktree 根。
    pub root: PathBuf,
    /// 激活页签 id：终端 = `sid`，预览 = 绝对路径。
    #[serde(default)]
    pub active: Option<String>,
    #[serde(default)]
    pub tabs: Vec<SavedTab>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 页签类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SavedTabKind {
    Terminal,
    Preview,
}

/// 一条持久化页签。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedTab {
    pub kind: SavedTabKind,
    /// 显示标题（终端可能是 OSC 标题；普通终端缺省是「终端」）。
    pub name: String,
    /// 终端页签的 daemon 逻辑会话 id。
    #[serde(default)]
    pub sid: Option<String>,
    /// 预览页签的绝对路径。
    #[serde(default)]
    pub path: Option<PathBuf>,
    /// 终端页签的工作目录；预览不用。
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    /// 一等入口的 agent id。即使 launch spec 还没写入（页面挂载前崩溃），也能恢复入口类型。
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 一个终端会话的启动参数（由 app-core 在 `terminal_open` 时写入）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchSpec {
    /// 逻辑会话 id；`SavedTab.sid` 指向它。
    pub sid: String,
    /// 首次 / 重建时的工作目录。
    pub cwd: PathBuf,
    /// 显式 shell；`None` = daemon 缺省（Git Bash / PowerShell）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    /// 首次启动实际写入 PTY 的命令行（用于重建 / 展示；恢复时保留原值）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// 命令程序（可执行文件或 PATH 名）；agent resume 需要拆开程序与参数。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<String>,
    /// 首次启动的参数串原样保存；恢复时重传（#79 的注入参数也走这里）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<String>,
    /// 一等入口的 agent id；普通终端为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// agent CLI 自己的对话会话 id；由 #79 的事件通道回写。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session_id: Option<String>,
    /// 最近一次 spawn 的世代 id；用于日志 / 诊断，不用于判断存活。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incarnation_id: Option<String>,
    /// 最近更新时间（Unix 毫秒）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at_ms: Option<u64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Document for AppState {
    const FILE: &'static str = "state.json";
    const CURRENT_VERSION: u32 = CURRENT_STATE_VERSION;

    fn migrations() -> &'static [Migration] {
        &[]
    }

    fn schema_version(&self) -> u32 {
        self.schema_version
    }
}

fn current_version() -> u32 {
    CURRENT_STATE_VERSION
}
