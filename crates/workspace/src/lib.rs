//! Ageminal 的工作区模型：项目 / 侧栏 / 显示名（#57）。
//!
//! 三层：
//!
//! - [`model`]：`state.json` / `settings.json` 里**属于工作区的那部分形状**（含 serde）；
//! - [`view`]：把「落盘事实 + 本次 git 发现」投影成侧栏与设置页共用的视图；
//! - 规则（默认取名、加入即全量 pin、覆盖优先）都落在本 crate，夹具驱动。
//!
//! 为什么形状也放这里：本机（Linux）编译不了 `src-tauri`，把落盘形状与投影规则一起
//! 放进核心 crate，才能在本地把「默认值 / 前向兼容 / 取名规则」跑成红绿
//! （REQUIREMENTS.md §20「纯逻辑跨平台可测」）。

pub mod model;
pub mod view;

pub use model::{Project, ProjectSettings, WorktreeName};
pub use view::{project_view, Discovery, ProjectView, WorktreeView};
