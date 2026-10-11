//! Ageminal 的文件域：目录列举与可见性标记（#58）、全树搜索（#59）。
//!
//! 列举只做「打标记」：给定工作树根 `root` 与目录 `dir`，返回 `dir` 下的条目，每条带上两个
//! 可见性标记，**过滤留给前端按开关做**（#18 D4）——切换「显示隐藏文件 / 显示忽略文件」因此
//! 不必重扫目录。搜索见 [`search`]。
//!
//! 三个可见性维度（#18 D3）：`.git` **永远隐藏**（这里直接不返回）、dotfile、被忽略。
//!
//! 忽略匹配用 `ignore` crate 的 `IncrementalIgnore`（#18 D1），范围 = 嵌套 `.gitignore`
//! + `.git/info/exclude` + 全局 excludes；**不认** `.ignore`；**非 git 目录不做忽略过滤**。
//!
//! 本层**无状态**：每次调用建一个新匹配器（`.gitignore` 改动立刻生效）。按目录缓存与失效
//! （#18 D5）连同 fs watch 归 #19 / #61——那需要一个比自己更长寿的持有者。

use std::path::{Component, Path, PathBuf};

mod entry;
mod error;
pub mod preview;
mod rules;
pub mod search;

use rules::IgnoreRules;

pub use entry::Entry;
pub use error::FilesError;
pub use preview::{read_preview, Preview, PreviewKind};
pub use search::{search, SearchHit, SearchOptions, SearchOutcome, MAX_RESULTS};

/// 校验 `path` 落在工作树根 `root` 之内（词法校验）。
///
/// 供需要「把能力限定在工作区路径内」的调用方复用（文件预览与「用外部编辑器打开」同源，
/// 见 #60 D6 #20）。
pub fn ensure_within(root: &Path, path: &Path) -> Result<(), FilesError> {
    relative_to(root, path).map(|_| ())
}

/// `.git` 永不出现在列举 / 搜索结果里（#18 D3 维度 1，不可通过开关显示）。
pub(crate) const GIT_DIR: &str = ".git";

/// 单目录**结果**上限（#19 D5）：超出即截断，避免把十万条塞过 IPC。
pub const MAX_ENTRIES_PER_DIR: usize = 5000;

/// 是不是 `.git`（维度 1）。ASCII 大小写不敏感：Windows 文件系统不区分大小写。
pub(crate) fn is_git_dir(name: &str) -> bool {
    name.eq_ignore_ascii_case(GIT_DIR)
}

/// 一个目录的列举结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirListing {
    pub entries: Vec<Entry>,
    /// 条目数超过 [`MAX_ENTRIES_PER_DIR`]，已截断（D5 #19）。
    pub truncated: bool,
}

/// 列举 `dir` 下的条目并打上可见性标记。
///
/// - `root` 是**工作树根**：忽略规则以它为基准（嵌套 `.gitignore` 的层级、仓库归属都由它定）。
/// - `dir` 必须落在 `root` 之内，否则 [`FilesError::OutsideRoot`]（§15 路径校验）。
///
/// 条目按「目录优先 → 名字大小写不敏感」排序，结果确定、消费方可直接渲染。
///
/// 超过 [`MAX_ENTRIES_PER_DIR`] 时取**排序后的前 N 项**并置 `truncated`——上限约束的是
/// **结果**（IPC / DOM 的量），不是遍历：先排好序再截，前 N 项才有确定含义。
pub fn read_dir(root: &Path, dir: &Path) -> Result<DirListing, FilesError> {
    list_dir(root, dir, MAX_ENTRIES_PER_DIR)
}

fn list_dir(root: &Path, dir: &Path, limit: usize) -> Result<DirListing, FilesError> {
    let relative_dir = relative_to(root, dir)?;
    let mut rules = IgnoreRules::new(root);

    let listing = std::fs::read_dir(dir).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            FilesError::NotFound {
                path: dir.to_path_buf(),
            }
        } else {
            FilesError::Io {
                path: dir.to_path_buf(),
                source,
            }
        }
    })?;

    let mut entries = Vec::new();
    for item in listing {
        // 单条读不到就跳过：一个坏条目不该让整棵目录列举失败。
        let Ok(item) = item else { continue };

        let name = item.file_name();
        let name_text = name.to_string_lossy().into_owned();
        if is_git_dir(&name_text) {
            continue;
        }

        let is_dir = item.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
        let relative = if relative_dir.as_os_str().is_empty() {
            PathBuf::from(&name)
        } else {
            relative_dir.join(&name)
        };

        entries.push(Entry {
            hidden: name_text.starts_with('.'),
            ignored: rules.is_ignored(&relative, is_dir),
            path: item.path(),
            is_dir,
            name: name_text,
        });
    }

    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });

    let truncated = entries.len() > limit;
    entries.truncate(limit);
    Ok(DirListing { entries, truncated })
}

/// `dir` 相对 `root` 的路径；不在其下就报 [`FilesError::OutsideRoot`]。
///
/// 校验是**词法**的：`strip_prefix` 按路径分量比较（`/a/proj-2` 不属于 `/a/proj`），并显式
/// 拒绝 `..` 分量——否则 `root/../x` 会以 `../x` 的相对路径溜过前缀检查，读到 `root` 之外。
///
/// `pub(crate)`：目录列举（[`list_dir`]）与文件预览（[`preview`]）共用同一道路径校验。
pub(crate) fn relative_to(root: &Path, dir: &Path) -> Result<PathBuf, FilesError> {
    let outside = || FilesError::OutsideRoot {
        dir: dir.to_path_buf(),
    };
    let relative = dir.strip_prefix(root).map_err(|_| outside())?;
    if relative
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(outside());
    }
    Ok(relative.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 单目录上限只在集成测试里要造 5001 个条目，成本不值——用一个小上限直接测截断语义。
    #[test]
    fn truncates_at_the_limit() {
        let root =
            std::env::temp_dir().join(format!("ageminal-files-limit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        for name in ["a.txt", "b.txt", "c.txt"] {
            std::fs::write(root.join(name), "x").unwrap();
        }

        let capped = list_dir(&root, &root, 2).unwrap();
        assert_eq!(
            capped
                .entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["a.txt", "b.txt"]
        );
        assert!(capped.truncated);

        let exact = list_dir(&root, &root, 3).unwrap();
        assert_eq!(exact.entries.len(), 3);
        assert!(!exact.truncated);

        let _ = std::fs::remove_dir_all(&root);
    }
}
