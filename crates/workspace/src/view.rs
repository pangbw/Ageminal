//! 侧栏与设置页共用的视图投影（#16 D6：同一份 store 数据的两个视图）。
//!
//! 纯函数：输入是「落盘的项目事实 + 一次 git 发现的结果 + 本地显示名」，输出是视图。
//! 不碰文件系统、不拉进程，因此夹具驱动、跨平台可测。

use std::path::PathBuf;

use ageminal_git::worktree::Worktree;

use crate::model::{Project, WorktreeName};

/// 一次 git 发现的结果。
///
/// 把失败做成**一等值**而不是旁路，是为了让 §15 的「明确提示」自然落到视图里：
/// 项目节点照常出现，只是带着错误信息、没有工作树。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Discovery {
    Found(Vec<Worktree>),
    Failed { message: String },
}

/// 一个项目的视图（侧栏分组 + 设置页「项目」分页共用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectView {
    pub path: PathBuf,
    pub name: String,
    pub collapsed: bool,
    /// 发现到的全部 worktree（**不过滤**：侧栏只用 `pinned` 的那些，设置页要全量）。
    pub worktrees: Vec<WorktreeView>,
    /// 发现失败时的明确提示；成功时为 `None`。
    pub error: Option<String>,
}

/// 一个 worktree 的视图。
///
/// 不含 agent 状态：那是运行期事件（#79 / #80 的统一状态），不落盘、不属于本 crate。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeView {
    /// git 报告的路径（身份）。
    pub path: PathBuf,
    /// 侧栏显示名：本地覆盖 > 默认（主工作树用项目名，其余用目录 basename）。
    pub name: String,
    /// 分支展示：分支短名；detached 显示短 SHA；bare 为 `None`。
    pub branch_label: Option<String>,
    pub is_main: bool,
    pub bare: bool,
    pub locked: bool,
    pub detached: bool,
    /// `prunable` 或目录已不存在（#56 的 `stale`）。
    pub stale: bool,
    /// 已加入左侧栏（记忆：git 的 worktree 列表**不落盘**，这份名单落盘）。
    pub pinned: bool,
}

/// 把落盘事实 + 发现结果投影成视图。
pub fn project_view(
    project: &Project,
    discovery: &Discovery,
    names: &[WorktreeName],
) -> ProjectView {
    let error = match discovery {
        Discovery::Found(_) => None,
        Discovery::Failed { message } => Some(message.clone()),
    };
    let worktrees = match discovery {
        Discovery::Found(worktrees) => worktrees
            .iter()
            .map(|worktree| worktree_view(project, worktree, names))
            .collect(),
        Discovery::Failed { .. } => Vec::new(),
    };

    ProjectView {
        path: project.path.clone(),
        name: project.name.clone(),
        collapsed: project.collapsed,
        worktrees,
        error,
    }
}

fn worktree_view(project: &Project, worktree: &Worktree, names: &[WorktreeName]) -> WorktreeView {
    let override_name = names
        .iter()
        .find(|entry| entry.path == worktree.path)
        .map(|entry| entry.name.clone());

    WorktreeView {
        path: worktree.path.clone(),
        // 默认名在发现时就按 §4.1 定好了（主工作树用项目名、其余用 basename）。
        name: override_name.unwrap_or_else(|| worktree.name.clone()),
        branch_label: worktree.branch_label.clone(),
        is_main: worktree.is_main,
        bare: worktree.bare,
        locked: worktree.locked.is_present(),
        detached: worktree.detached,
        stale: worktree.stale,
        pinned: project.is_pinned(&worktree.path),
    }
}
