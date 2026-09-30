//! `state.json`：项目、侧栏、页签骨架、窗口尺寸 / 位置等（REQUIREMENTS.md §14）。

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::doc::{Document, Migration};

/// 当前代码能写出的状态版本。
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
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_STATE_VERSION,
            window: None,
            extra: Map::new(),
        }
    }
}

/// 窗口几何。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
