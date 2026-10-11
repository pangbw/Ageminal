//! 工作区的落盘形状与规则（REQUIREMENTS.md §14）。
//!
//! 分权：**项目列表 / 侧栏 / 折叠状态**在 `state.json`（[`Project`]），
//! **worktree 显示名**在 `settings.json` 的项目级设置（[`ProjectSettings`]）。
//! 两个文档都按 §14 的约定保留未知字段（`extra`，嵌套层同样保留）。

use std::path::{Path, PathBuf};

use ageminal_git::worktree::{dir_name, Worktree};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// 一个已加入 Ageminal 的 git 仓库（`state.json`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    /// 仓库根：既是 `git -C` 的目标，也是项目身份。
    pub path: PathBuf,
    /// 项目名（加入时取目录 basename；§11 不提供项目改名）。
    pub name: String,
    /// 侧栏分组是否折叠。
    #[serde(default)]
    pub collapsed: bool,
    /// **已加入左侧栏**的 worktree 路径。
    ///
    /// 只存「已加入」的（§14），不存全部——侧栏渲染 = 发现结果 ∩ 这份名单。
    #[serde(default)]
    pub pinned: Vec<PathBuf>,
    /// 未知字段原样保留（前向兼容，§14）。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Project {
    /// 新项目：把**当前发现到的 worktree 全部加入侧栏**。
    ///
    /// #57 的验收「加仓库后侧栏出现其 worktree」——所以加入即全量 pin；之后由设置页的
    /// 「加入左侧栏」开关逐个移出。
    pub fn new(path: PathBuf, name: String, discovered: &[Worktree]) -> Self {
        let mut project = Self {
            path,
            name,
            collapsed: false,
            pinned: Vec::new(),
            extra: Map::new(),
        };
        project.pin_all(discovered);
        project
    }

    /// 默认项目名：仓库根目录的 basename。
    pub fn default_name(path: &Path) -> String {
        dir_name(path)
    }

    /// 把给定 worktree 全部加入侧栏（幂等）。
    pub fn pin_all(&mut self, discovered: &[Worktree]) {
        for worktree in discovered {
            if !self.is_pinned(&worktree.path) {
                self.pinned.push(worktree.path.clone());
            }
        }
    }

    /// 加入 / 移出左侧栏。
    pub fn set_pinned(&mut self, worktree: &Path, pinned: bool) {
        self.pinned.retain(|path| path != worktree);
        if pinned {
            self.pinned.push(worktree.to_path_buf());
        }
    }

    /// 是否已加入左侧栏。
    pub fn is_pinned(&self, worktree: &Path) -> bool {
        self.pinned.iter().any(|path| path == worktree)
    }
}

/// `settings.json` 里的**项目级设置**表项（§14；#57 只填 worktree 显示名）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSettings {
    /// 项目（仓库根）路径。
    pub path: PathBuf,
    /// worktree 的本地显示名。
    #[serde(default)]
    pub worktree_names: Vec<WorktreeName>,
    /// 未知字段原样保留；#86 的 shell 覆盖 / 环境变量 / agent 覆盖落在这里。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl ProjectSettings {
    /// 空的项目级设置。
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            worktree_names: Vec::new(),
            extra: Map::new(),
        }
    }

    /// 设置显示名；`name` 为 `None`（或只有空白）时**清除覆盖、回到默认名**。
    pub fn rename(&mut self, worktree: &Path, name: Option<&str>) {
        self.worktree_names.retain(|entry| entry.path != worktree);
        let Some(name) = name.map(str::trim).filter(|name| !name.is_empty()) else {
            return;
        };
        self.worktree_names.push(WorktreeName {
            path: worktree.to_path_buf(),
            name: name.to_owned(),
            extra: Map::new(),
        });
    }

    /// 该 worktree 的显示名覆盖（没有则 `None`，由投影退回默认名）。
    pub fn name_of(&self, worktree: &Path) -> Option<&str> {
        self.worktree_names
            .iter()
            .find(|entry| entry.path == worktree)
            .map(|entry| entry.name.as_str())
    }
}

/// 一个 worktree 的本地显示名（仅本地，不写回 git）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeName {
    pub path: PathBuf,
    pub name: String,
    /// 未知字段原样保留（嵌套层同样成立）。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
