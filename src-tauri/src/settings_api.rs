//! 设置页的命令面（#86）：通用 / 外观 / 项目级覆盖。
//!
//! 落盘仍是 `settings.json`（[`crate::storage::Settings`]）。这里的视图类型只负责把
//! 嵌套字段拍平成前端好编辑的形状；写回时做范围校验与空值归一。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::State;

use crate::storage::{Appearance, CursorStyle, Settings, StartupBehavior, Store};

/// 通用设置视图。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct GeneralView {
    /// 默认 shell；`None` = 自动发现。
    pub default_shell: Option<String>,
    /// 关闭运行中的终端是否二次确认。
    pub confirm_close: bool,
    /// 启动行为。
    pub startup_behavior: StartupBehaviorView,
    /// 语言（BCP-47）；`None` = 自动检测。
    pub language: Option<String>,
    /// 日志目录（供「打开日志」）。
    pub logs_dir: String,
}

/// 启动行为的 TS 形状。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum StartupBehaviorView {
    /// 恢复上次工作区。
    Restore,
    /// 空工作区。
    Empty,
}

impl From<StartupBehavior> for StartupBehaviorView {
    fn from(value: StartupBehavior) -> Self {
        match value {
            StartupBehavior::Restore => Self::Restore,
            StartupBehavior::Empty => Self::Empty,
        }
    }
}

impl From<StartupBehaviorView> for StartupBehavior {
    fn from(value: StartupBehaviorView) -> Self {
        match value {
            StartupBehaviorView::Restore => Self::Restore,
            StartupBehaviorView::Empty => Self::Empty,
        }
    }
}

/// 通用设置输入。
#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct GeneralInput {
    /// 默认 shell；空白 = 自动发现。
    pub default_shell: Option<String>,
    /// 关闭运行中的终端是否二次确认。
    pub confirm_close: bool,
    /// 启动行为。
    pub startup_behavior: StartupBehaviorView,
    /// 语言；空白 = 自动检测。
    pub language: Option<String>,
}

/// 外观设置视图。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AppearanceView {
    /// 主题 id。
    pub theme: String,
    /// 终端字号（px）。
    pub font_size: u8,
    /// 终端行高倍数。
    pub line_height: f64,
    /// 光标样式。
    pub cursor_style: CursorStyleView,
    /// Acrylic（#87；重启生效）。
    pub acrylic: bool,
}

/// 光标样式的 TS 形状。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum CursorStyleView {
    /// 方块。
    Block,
    /// 竖线。
    Bar,
    /// 下划线。
    Underline,
}

impl From<CursorStyle> for CursorStyleView {
    fn from(value: CursorStyle) -> Self {
        match value {
            CursorStyle::Block => Self::Block,
            CursorStyle::Bar => Self::Bar,
            CursorStyle::Underline => Self::Underline,
        }
    }
}

impl From<CursorStyleView> for CursorStyle {
    fn from(value: CursorStyleView) -> Self {
        match value {
            CursorStyleView::Block => Self::Block,
            CursorStyleView::Bar => Self::Bar,
            CursorStyleView::Underline => Self::Underline,
        }
    }
}

/// 外观输入。
#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AppearanceInput {
    /// 主题 id；MVP 只接受 one-dark-pro-glass。
    pub theme: String,
    /// 字号（8–32）。
    pub font_size: u8,
    /// 行高（1.0–2.0）。
    pub line_height: f64,
    /// 光标样式。
    pub cursor_style: CursorStyleView,
    /// Acrylic。
    pub acrylic: bool,
}

/// 项目级环境变量。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProjectEnvView {
    /// 变量名。
    pub name: String,
    /// 变量值。
    pub value: String,
}

/// 项目级 agent 覆盖。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProjectAgentView {
    /// agent id。
    pub agent_id: String,
    /// 程序覆盖；`None` = 继承全局。
    pub command: Option<String>,
    /// 参数覆盖；`None` = 继承全局。
    pub args: Option<String>,
}

/// 项目级设置视图。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSettingsView {
    /// 项目根路径。
    pub path: String,
    /// 项目级默认 shell；`None` = 继承全局。
    pub default_shell: Option<String>,
    /// 项目级环境变量。
    pub env: Vec<ProjectEnvView>,
    /// 项目级 agent 覆盖。
    pub agents: Vec<ProjectAgentView>,
}

/// 项目级设置输入。
#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSettingsInput {
    /// 项目根路径。
    pub path: String,
    /// 项目级默认 shell；空白 = 继承全局。
    pub default_shell: Option<String>,
    /// 项目级环境变量。
    pub env: Vec<ProjectEnvView>,
    /// 项目级 agent 覆盖。
    pub agents: Vec<ProjectAgentView>,
}

fn clean(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn logs_dir() -> String {
    ageminal_daemon::paths::config_dir()
        .map(|path| path.join("logs").to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn general_view(settings: &Settings) -> GeneralView {
    GeneralView {
        default_shell: settings.general.default_shell.clone(),
        confirm_close: settings.general.confirm_close,
        startup_behavior: settings.general.startup_behavior.into(),
        language: settings.general.language.clone(),
        logs_dir: logs_dir(),
    }
}

fn appearance_view(settings: &Settings) -> AppearanceView {
    AppearanceView {
        theme: settings.appearance.theme.clone(),
        font_size: settings.appearance.font_size,
        line_height: settings.appearance.line_height,
        cursor_style: settings.appearance.cursor_style.into(),
        acrylic: settings.appearance.acrylic,
    }
}

/// 读通用设置。
#[tauri::command]
#[specta::specta]
pub fn general_settings(store: State<'_, Mutex<Store>>) -> Result<GeneralView, String> {
    let store = store.lock().map_err(|_| "设置锁已被污染".to_owned())?;
    Ok(general_view(store.settings()))
}

/// 写通用设置并返回最新值。
#[tauri::command]
#[specta::specta]
pub fn set_general_settings(
    store: State<'_, Mutex<Store>>,
    input: GeneralInput,
) -> Result<GeneralView, String> {
    let mut store = store.lock().map_err(|_| "设置锁已被污染".to_owned())?;
    let default_shell = clean(input.default_shell);
    let language = clean(input.language);
    store
        .update_settings(|settings| {
            settings.general.default_shell = default_shell;
            settings.general.confirm_close = input.confirm_close;
            settings.general.startup_behavior = input.startup_behavior.into();
            settings.general.language = language;
        })
        .map_err(|error| error.to_string())?;
    Ok(general_view(store.settings()))
}

/// 读外观设置。
#[tauri::command]
#[specta::specta]
pub fn appearance_settings(store: State<'_, Mutex<Store>>) -> Result<AppearanceView, String> {
    let store = store.lock().map_err(|_| "设置锁已被污染".to_owned())?;
    Ok(appearance_view(store.settings()))
}

/// 写外观设置并返回归一后的值。
#[tauri::command]
#[specta::specta]
pub fn set_appearance_settings(
    store: State<'_, Mutex<Store>>,
    input: AppearanceInput,
) -> Result<AppearanceView, String> {
    let font_size = input.font_size.clamp(8, 32);
    let line_height = (input.line_height.clamp(1.0, 2.0) * 10.0).round() / 10.0;
    let theme = if input.theme.trim().is_empty() {
        "one-dark-pro-glass".to_owned()
    } else {
        input.theme.trim().to_owned()
    };
    let mut store = store.lock().map_err(|_| "设置锁已被污染".to_owned())?;
    store
        .update_settings(|settings| {
            settings.appearance = Appearance {
                theme: theme.clone(),
                font_size,
                line_height,
                cursor_style: input.cursor_style.into(),
                acrylic: input.acrylic,
                extra: Default::default(),
            };
        })
        .map_err(|error| error.to_string())?;
    Ok(appearance_view(store.settings()))
}

/// 读一个项目的覆盖设置；不存在时返回空覆盖。
#[tauri::command]
#[specta::specta]
pub fn project_settings_view(
    store: State<'_, Mutex<Store>>,
    path: String,
) -> Result<ProjectSettingsView, String> {
    let store = store.lock().map_err(|_| "设置锁已被污染".to_owned())?;
    let target = PathBuf::from(&path);
    let project = store
        .settings()
        .projects
        .iter()
        .find(|project| project.path == target);
    Ok(project_view(&path, project))
}

fn project_view(
    path: &str,
    project: Option<&ageminal_workspace::ProjectSettings>,
) -> ProjectSettingsView {
    let env = project
        .map(|project| {
            project
                .env
                .iter()
                .map(|(name, value)| ProjectEnvView {
                    name: name.clone(),
                    value: value.clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    let agents = project
        .map(|project| {
            project
                .agents
                .iter()
                .map(|(agent_id, over)| ProjectAgentView {
                    agent_id: agent_id.clone(),
                    command: over.command.clone(),
                    args: over.args.clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    ProjectSettingsView {
        path: path.to_owned(),
        default_shell: project.and_then(|project| project.default_shell.clone()),
        env,
        agents,
    }
}

/// 写项目级覆盖；全空时删除该项目条目。
#[tauri::command]
#[specta::specta]
pub fn set_project_settings(
    store: State<'_, Mutex<Store>>,
    input: ProjectSettingsInput,
) -> Result<ProjectSettingsView, String> {
    let target = PathBuf::from(&input.path);
    let default_shell = clean(input.default_shell);
    let mut env = BTreeMap::new();
    for entry in input.env {
        let name = entry.name.trim();
        if name.is_empty() {
            continue;
        }
        env.insert(name.to_owned(), entry.value);
    }
    let mut agents = BTreeMap::new();
    for entry in input.agents {
        if !crate::agents::AGENT_IDS.contains(&entry.agent_id.as_str()) {
            continue;
        }
        let command = clean(entry.command);
        let args = clean(entry.args);
        if command.is_none() && args.is_none() {
            continue;
        }
        agents.insert(
            entry.agent_id,
            ageminal_workspace::ProjectAgentOverride {
                command,
                args,
                extra: Default::default(),
            },
        );
    }
    let mut store = store.lock().map_err(|_| "设置锁已被污染".to_owned())?;
    store
        .update_settings(|settings| {
            let existing = settings
                .projects
                .iter()
                .position(|project| project.path == target);
            let mut project = existing
                .and_then(|index| settings.projects.get(index).cloned())
                .unwrap_or_else(|| ageminal_workspace::ProjectSettings::new(target.clone()));
            project.default_shell = default_shell;
            project.env = env;
            project.agents = agents;
            if project.is_empty() {
                if let Some(index) = existing {
                    settings.projects.remove(index);
                }
            } else if let Some(index) = existing {
                settings.projects[index] = project;
            } else {
                settings.projects.push(project);
            }
        })
        .map_err(|error| error.to_string())?;
    let project = store
        .settings()
        .projects
        .iter()
        .find(|project| project.path == target);
    Ok(project_view(&input.path, project))
}

/// 打开日志目录（#86 通用入口）。
#[tauri::command]
#[specta::specta]
pub fn open_logs_dir(app: tauri::AppHandle) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;

    let dir = ageminal_daemon::paths::config_dir()
        .map_err(|error| error.to_string())?
        .join("logs");
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    app.opener()
        .open_path(dir.to_string_lossy().into_owned(), None::<&str>)
        .map_err(|error| error.to_string())
}
