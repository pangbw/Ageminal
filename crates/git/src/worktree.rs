//! `git worktree list --porcelain -z` 的解析与模型映射。
//!
//! 本模块是**纯逻辑**：输入是 git 输出的字节，输出是模型，不碰文件系统、不拉进程，
//! 因此夹具驱动、跨平台可测。
//!
//! ## `--porcelain -z` 的形状（#17 D5，真机实测）
//!
//! - 字段以 `\0` 结尾，**记录之间多一个 `\0`**（即 `\0\0`）；
//! - 字段形如 `key value`，`key` 后跟一个空格；无值的键（`detached` / `bare` / `locked`）
//!   只有键本身；
//! - 值**原样**给出，路径含空格 / 中文 / 引号都不转义 —— 所以解析器只按第一个空格切键，
//!   **不自行去引号**；
//! - `locked` / `prunable` 可带原因，且原因与键**同一 token**（`locked hold for review`）。
//!
//! ## 两个刻意的取舍
//!
//! - **按 UTF-8 解读字段**：本项目的唯一目标是 Windows（REQUIREMENTS.md §2），Git for
//!   Windows 输出的路径是 UTF-8；非 UTF-8 一律视为输出损坏。这样就不必为「非目标平台
//!   的合法非 UTF-8 路径」引入 `OsString` 的平台分支。
//! - **短 SHA 固定 7 位**：这是 git 默认的最短缩写；模型同时保留完整 `head`，
//!   将来若要严格对齐 git 的 `core.abbrev`，由消费方按需再算。

use std::path::{Path, PathBuf};

use crate::error::GitError;

/// 短 SHA 的长度（git 默认缩写）。
const SHORT_SHA_LEN: usize = 7;

/// git 报告的一条 worktree，字段基本原样。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RawWorktree {
    /// `worktree <path>`。
    pub path: PathBuf,
    /// `HEAD <sha>`；bare 主工作树没有这一行。
    pub head: Option<String>,
    /// 完整 ref，如 `refs/heads/main`。
    pub branch: Option<String>,
    pub detached: bool,
    pub bare: bool,
    pub locked: Marker,
    pub prunable: Marker,
}

/// `locked` / `prunable` 这类「出现即标记、可带原因」的字段。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Marker {
    /// 字段未出现。
    #[default]
    Absent,
    /// 字段出现；`reason` 是 git 给出的原因（也可能没有）。
    Present { reason: Option<String> },
}

impl Marker {
    /// 字段是否出现。
    pub fn is_present(&self) -> bool {
        matches!(self, Self::Present { .. })
    }

    /// 字段携带的原因（未出现时为 `None`）。
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Present { reason } => reason.as_deref(),
            Self::Absent => None,
        }
    }
}

/// 面向 UI 的 worktree 模型（REQUIREMENTS.md §4.1 / #17 D7）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    /// git 输出的路径（原样）。
    pub path: PathBuf,
    /// 显示名：主工作树用项目名，其余用目录 basename。
    pub name: String,
    /// 主工作树（`git worktree list` 的第一条）。
    pub is_main: bool,
    /// HEAD 完整 SHA（bare 主工作树为 `None`）。
    pub head: Option<String>,
    /// 分支短名（`refs/heads/` 之后）；detached / bare 为 `None`。
    pub branch: Option<String>,
    /// 分支展示：分支短名；detached 显示短 SHA；bare 为 `None`。
    pub branch_label: Option<String>,
    pub detached: bool,
    pub bare: bool,
    pub locked: Marker,
    pub prunable: Marker,
    /// `prunable` 或目录已不存在 → UI 标「已失效」。
    pub stale: bool,
}

/// 解析 `git worktree list --porcelain -z` 的原始输出。
///
/// 未知键**忽略**（git 未来加字段时不炸，前向兼容）；但每条记录必须带 `worktree` 路径，
/// 否则视为输出损坏。
pub fn parse_porcelain_z(raw: &[u8]) -> Result<Vec<RawWorktree>, GitError> {
    if raw.is_empty() {
        return Err(malformed("worktree list 输出为空"));
    }

    let mut records = Vec::new();
    let mut current: Option<RawWorktree> = None;

    for token in raw.split(|byte| *byte == 0) {
        if token.is_empty() {
            // 记录间的空 token 收尾上一条记录。
            if let Some(record) = current.take() {
                records.push(record);
            }
            continue;
        }

        let text = std::str::from_utf8(token).map_err(|_| {
            malformed(&format!(
                "字段不是合法 UTF-8：{:?}",
                String::from_utf8_lossy(token)
            ))
        })?;
        let (key, value) = match text.split_once(' ') {
            Some((key, value)) => (key, Some(value)),
            None => (text, None),
        };

        let record = current.get_or_insert_with(RawWorktree::default);
        match key {
            "worktree" => record.path = PathBuf::from(required(key, value)?),
            "HEAD" => record.head = Some(required(key, value)?.to_owned()),
            "branch" => record.branch = Some(required(key, value)?.to_owned()),
            "detached" => record.detached = true,
            "bare" => record.bare = true,
            "locked" => {
                record.locked = Marker::Present {
                    reason: value.map(str::to_owned),
                }
            }
            "prunable" => {
                record.prunable = Marker::Present {
                    reason: value.map(str::to_owned),
                }
            }
            // 未知键忽略。
            _ => {}
        }
    }

    // 收尾分隔缺失（截断输出）时，把已读到的字段收成最后一条。
    if let Some(record) = current.take() {
        records.push(record);
    }

    if let Some(broken) = records
        .iter()
        .find(|record| record.path.as_os_str().is_empty())
    {
        return Err(malformed(&format!("记录缺少 worktree 路径：{broken:?}")));
    }
    Ok(records)
}

/// 把 git 的原始记录映射成面向 UI 的模型。
///
/// 纯函数：目录存在性由 `exists` 注入，便于测试「已失效」判定。
///
/// `is_main` 取列表顺序：git 的契约是**主工作树恒排第一条**（它就是仓库根，与 cwd
/// 无关，真机实测）——所以这里不需要拿路径去比仓库根；那样做反而在 Windows 上会
/// 被大小写 / 正反斜杠 / 前缀形式坑到。
pub fn to_worktrees(
    raws: &[RawWorktree],
    project_name: &str,
    mut exists: impl FnMut(&Path) -> bool,
) -> Vec<Worktree> {
    raws.iter()
        .enumerate()
        .map(|(index, raw)| {
            let branch = raw.branch.as_deref().map(short_branch);
            let branch_label = match (&branch, raw.head.as_deref(), raw.detached) {
                (Some(branch), _, _) => Some(branch.clone()),
                (None, Some(head), true) => Some(short_sha(head)),
                _ => None,
            };
            Worktree {
                path: raw.path.clone(),
                name: if index == 0 {
                    project_name.to_owned()
                } else {
                    dir_name(&raw.path)
                },
                is_main: index == 0,
                head: raw.head.clone(),
                branch,
                branch_label,
                detached: raw.detached,
                bare: raw.bare,
                locked: raw.locked.clone(),
                prunable: raw.prunable.clone(),
                stale: raw.prunable.is_present() || !exists(&raw.path),
            }
        })
        .collect()
}

fn required<'a>(key: &str, value: Option<&'a str>) -> Result<&'a str, GitError> {
    value.ok_or_else(|| malformed(&format!("{key} 缺值")))
}

fn malformed(reason: &str) -> GitError {
    GitError::MalformedOutput {
        reason: reason.to_owned(),
    }
}

/// 目录 basename；没有 basename 时退回整个路径。
fn dir_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// 去掉 `refs/heads/` 前缀得到分支短名。
fn short_branch(reference: &str) -> String {
    reference
        .strip_prefix("refs/heads/")
        .unwrap_or(reference)
        .to_owned()
}

/// 取 HEAD 的前 7 位作为短 SHA。
fn short_sha(head: &str) -> String {
    head.chars().take(SHORT_SHA_LEN).collect()
}
