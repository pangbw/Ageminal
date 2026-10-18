//! `settings.json`：通用 / 外观 / 快捷键 / Agent / 项目级设置（REQUIREMENTS.md §11、§14）。

use std::collections::BTreeMap;

use ageminal_workspace::ProjectSettings;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::agents::AgentOverrides;

use super::doc::{Document, Migration};

/// 当前代码能写出的设置版本。
///
/// 2：`general.language` 由 `String` 改成 `Option<String>`（「未设置」≠「用户选了 zh-CN」）。
/// 首个公开发布前按 `Settings::migrations` 的约定走「备份 + 重置」，不写迁移。
///
/// **只加字段不改版本**：新字段带 `#[serde(default)]`，旧文档照读（`projects` 即如此）。
pub(crate) const CURRENT_SETTINGS_VERSION: u32 = 2;

/// 应用设置文档。
///
/// `extra` 原样保留未知字段——前向兼容要求「可写 + key 级保留未知字段」。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(default = "current_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub general: General,
    /// 外观设置（#86）：字号 / 行高 / 光标样式 / 主题 / Acrylic。
    #[serde(default)]
    pub appearance: Appearance,
    /// Agent 一等入口的覆盖项（#77）；缺省值在 [`crate::agents`]。
    #[serde(default)]
    pub agents: AgentOverrides,
    /// 项目级设置（§14；#57 只填 worktree 显示名，#86 再加 shell 覆盖 / 环境变量 / agent 覆盖）。
    #[serde(default)]
    pub projects: Vec<ProjectSettings>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SETTINGS_VERSION,
            general: General::default(),
            appearance: Appearance::default(),
            agents: BTreeMap::new(),
            projects: Vec::new(),
            extra: Map::new(),
        }
    }
}

/// 「通用」分页。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct General {
    /// 默认 shell 覆盖；`None` 表示自动发现。
    #[serde(default)]
    pub default_shell: Option<String>,
    /// 关闭运行中的终端页签是否二次确认。
    #[serde(default = "yes")]
    pub confirm_close: bool,
    /// 启动行为：恢复上次工作区 / 空工作区。
    #[serde(default)]
    pub startup_behavior: StartupBehavior,
    /// 界面语言（BCP-47）。
    ///
    /// `None` 表示**未设置**：语言检测按
    /// `插件 os.locale()` → `navigator.language` → 回退 `zh-CN` 走（见 issue #54）。
    /// 若在这里塞一个具体默认值，「未设置」与「用户选了默认值」就再也分不开。
    #[serde(default)]
    pub language: Option<String>,
    /// 未知字段原样保留：前向兼容在**嵌套层**同样成立。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for General {
    fn default() -> Self {
        Self {
            default_shell: None,
            confirm_close: true,
            startup_behavior: StartupBehavior::Restore,
            language: None,
            extra: Map::new(),
        }
    }
}

/// 启动行为（§11 通用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StartupBehavior {
    /// 恢复上次项目 / 页签（默认）。
    #[default]
    Restore,
    /// 启动时只显示空工作区。
    Empty,
}

/// 终端光标样式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CursorStyle {
    /// 方块。
    #[default]
    Block,
    /// 竖线。
    Bar,
    /// 下划线。
    Underline,
}

/// 「外观」分页（#86）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Appearance {
    /// 主题 id；MVP 只有 `one-dark-pro-glass`。
    #[serde(default = "default_theme")]
    pub theme: String,
    /// 终端字号（px）。
    #[serde(default = "default_font_size")]
    pub font_size: u8,
    /// 终端行高倍数。
    #[serde(default = "default_line_height")]
    pub line_height: f64,
    /// 终端光标样式。
    #[serde(default)]
    pub cursor_style: CursorStyle,
    /// 是否启用 Acrylic 窗口材质（#87；重启生效）。
    #[serde(default)]
    pub acrylic: bool,
    /// 未知字段原样保留。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: default_theme(),
            font_size: default_font_size(),
            line_height: default_line_height(),
            cursor_style: CursorStyle::Block,
            acrylic: false,
            extra: Map::new(),
        }
    }
}

fn default_theme() -> String {
    "one-dark-pro-glass".to_owned()
}

fn default_font_size() -> u8 {
    13
}

fn default_line_height() -> f64 {
    1.2
}

impl Document for Settings {
    const FILE: &'static str = "settings.json";
    const CURRENT_VERSION: u32 = CURRENT_SETTINGS_VERSION;

    fn migrations() -> &'static [Migration] {
        // 首个公开发布前：schema 变化走「备份 + 重置」，不写迁移。
        &[]
    }

    fn schema_version(&self) -> u32 {
        self.schema_version
    }
}

fn current_version() -> u32 {
    CURRENT_SETTINGS_VERSION
}

fn yes() -> bool {
    true
}
