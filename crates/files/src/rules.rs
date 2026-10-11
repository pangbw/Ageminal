//! 忽略规则匹配器（#18 D1）：`ignore` crate 的 `IncrementalIgnore` 的一层薄封装。
//!
//! 配置是这里最要紧的部分，逐条对应 #18 的决议：
//!
//! - `hidden(false)`：dotfile 是**另一个**维度（[`crate::Entry::hidden`]）。若让 matcher 也算
//!   hidden，`.env` 会被并进 `is_ignore`，三个可见性维度就混了；
//! - `ignore(false)`：D2 明确**不认** `.ignore`（ripgrep / ag 的约定，git 不认）；
//! - `parents(false)`：对齐 git——ignore 文件只在仓库内生效，不读仓库根**之上**的父目录。
//!
//! `.gitignore` / `.git/info/exclude` / 全局 excludes（`core.excludesFile`）与 `require_git`
//! 都取默认：非 git 目录因此自然不做忽略过滤（D5）。链接 worktree 的 `.git` **文件**也由
//! crate 自行解析到 commondir。

use std::path::Path;

use ignore::{IncrementalIgnore, WalkBuilder};

/// 一个工作树根的忽略规则匹配器。
///
/// **crate 内部**：目前只有 [`crate::read_dir`] 消费它；#59（搜索）/ #61（fs watch）需要时
/// 再决定是否提升为公开接口。
pub(crate) struct IgnoreRules {
    matcher: IncrementalIgnore,
}

impl IgnoreRules {
    /// 为工作树根建立匹配器。
    pub(crate) fn new(root: &Path) -> Self {
        let mut builder = WalkBuilder::new(root);
        builder.hidden(false).ignore(false).parents(false);
        // 只配置了一个 root，`build_matchers` 因此恰好返回一个匹配器。
        let matcher = builder
            .build_matchers()
            .pop()
            .expect("WalkBuilder 配了 root，必有一个 matcher");
        Self { matcher }
    }

    /// 判断**相对工作树根**的路径是否被忽略规则命中。
    ///
    /// `relative` 不含 `..`（由 [`crate::read_dir`] 的路径校验保证）；`is_dir` 决定目录类规则
    /// 是否命中。
    pub(crate) fn is_ignored(&mut self, relative: &Path, is_dir: bool) -> bool {
        self.matcher.matched(relative, is_dir).is_ignore()
    }
}
