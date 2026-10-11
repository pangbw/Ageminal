//! 目录里的一个条目及其可见性标记。

use std::path::PathBuf;

/// 目录里的一个条目。
///
/// 三个可见性维度（#18 D3）在这里的落法：
///
/// 1. `.git` —— **永远隐藏**，由 [`crate::read_dir`] 直接不返回（没有对应字段）；
/// 2. dotfile —— [`Entry::hidden`]；
/// 3. 被忽略规则命中 —— [`Entry::ignored`]。
///
/// 标记是给前端过滤用的（#18 D4）：切换「显示隐藏文件 / 显示忽略文件」不必重扫目录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// 文件名（不含路径）。
    pub name: String,
    /// 绝对路径（`dir` 之下的完整路径）。
    pub path: PathBuf,
    /// 是否目录。
    pub is_dir: bool,
    /// 维度 2：dotfile（名字以 `.` 开头）。
    pub hidden: bool,
    /// 维度 3：被忽略规则命中。
    ///
    /// 范围（#18 D2）= 嵌套 `.gitignore` + `.git/info/exclude` + 全局 excludes
    /// （`core.excludesFile`）；**不认** `.ignore`。非 git 目录恒为 `false`。
    pub ignored: bool,
}
