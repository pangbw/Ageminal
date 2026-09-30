//! `settings.json`：通用 / 外观 / 快捷键 / Agent / 项目级设置（REQUIREMENTS.md §11、§14）。

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::doc::{Document, Migration};

/// 当前代码能写出的设置版本。
pub(crate) const CURRENT_SETTINGS_VERSION: u32 = 1;

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
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SETTINGS_VERSION,
            general: General::default(),
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
    /// 界面语言（BCP-47）。
    #[serde(default = "default_language")]
    pub language: String,
}

impl Default for General {
    fn default() -> Self {
        Self {
            default_shell: None,
            confirm_close: true,
            language: default_language(),
        }
    }
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

fn default_language() -> String {
    "zh-CN".to_owned()
}
