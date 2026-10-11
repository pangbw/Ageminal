//! 全树搜索（#19 D8）：按文件**名**的子串匹配，走 `ignore::WalkBuilder`。
//!
//! 懒加载下「只过滤已加载节点」会给出误导性结果，所以搜索必须在**整棵树**上跑。忽略规则与
//! [`crate::read_dir`] 同一套（#18 的范围 + 两个开关），并额外保证：
//!
//! - **可取消**：调用方给一个 `should_continue`，新查询一来旧遍历就停（D8）；
//! - **有界**：结果上限 [`MAX_RESULTS`] 条、墙钟预算用完返回部分结果（D8）；
//! - `.git` 永不进结果（维度 1，同 [`crate::read_dir`]）。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ignore::WalkBuilder;

use crate::rules::IgnoreRules;

/// 搜索结果上限（#19 D8）。
pub const MAX_RESULTS: usize = 200;

/// 搜索的两个开关（与可见性过滤同一对，#18 D3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchOptions {
    /// 「显示隐藏文件」：打开才搜 dotfile。
    pub show_hidden: bool,
    /// 「显示忽略文件」：打开才把被忽略项纳入结果（并逐条标记，供弱化显示）。
    pub show_ignored: bool,
}

/// 一条命中。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    /// 绝对路径。
    pub path: PathBuf,
    /// 相对工作树根的展示路径（`/` 分隔），用于在树里展开祖先。
    pub relative: String,
    pub is_dir: bool,
    /// 被忽略规则命中（仅当 `show_ignored` 打开时才可能为 `true`）。
    pub ignored: bool,
}

/// 一次搜索的结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchOutcome {
    pub hits: Vec<SearchHit>,
    /// 命中数达到 [`MAX_RESULTS`] 上限。
    pub truncated: bool,
    /// 预算用尽，结果是部分的。
    pub timed_out: bool,
}

/// 在 `root` 下按名字搜 `query`（大小写不敏感的子串）。
///
/// `budget` 是墙钟预算（#19 D8 取 ~2 s）；`should_continue` 返回 `false` 时立即停（被新查询
/// 取代）。空查询返回空结果、不遍历。
pub fn search(
    root: &Path,
    query: &str,
    options: SearchOptions,
    budget: Duration,
    mut should_continue: impl FnMut() -> bool,
) -> SearchOutcome {
    let needle = query.trim().to_lowercase();
    let mut outcome = SearchOutcome::default();
    if needle.is_empty() {
        return outcome;
    }

    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(!options.show_hidden)
        // 与 read_dir 对齐：不认 `.ignore`、不读仓库根之上的父目录。
        .ignore(false)
        .parents(false)
        // `.git` 永不出现（维度 1）；`filter_entry` 会连整棵子树一起剪掉。
        .filter_entry(|entry| !crate::is_git_dir(&entry.file_name().to_string_lossy()));
    if options.show_ignored {
        // 「显示忽略文件」= 不让 walk 剪掉被忽略项，改为逐条标记（供前端弱化显示）。
        builder
            .git_ignore(false)
            .git_exclude(false)
            .git_global(false);
    }
    let mut rules = options.show_ignored.then(|| IgnoreRules::new(root));

    let deadline = Instant::now() + budget;
    for result in builder.build() {
        if !should_continue() {
            return outcome;
        }
        if Instant::now() >= deadline {
            outcome.timed_out = true;
            break;
        }

        let Ok(entry) = result else { continue };
        if entry.depth() == 0 {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.to_lowercase().contains(&needle) {
            continue;
        }

        let path = entry.path();
        let is_dir = entry.file_type().is_some_and(|kind| kind.is_dir());
        let relative = relative_string(root, path);
        let ignored = match rules.as_mut() {
            Some(rules) => rules.is_ignored(Path::new(&relative), is_dir),
            None => false,
        };
        outcome.hits.push(SearchHit {
            path: path.to_path_buf(),
            relative,
            is_dir,
            ignored,
        });

        if outcome.hits.len() >= MAX_RESULTS {
            outcome.truncated = true;
            break;
        }
    }

    outcome.hits.sort_by(|a, b| a.relative.cmp(&b.relative));
    outcome
}

/// `path` 相对 `root` 的展示路径，统一用 `/` 分隔。
fn relative_string(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}
