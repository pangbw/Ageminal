//! 忽略过滤的保真度（#18 派生落点 / #58 验收 1）。
//!
//! 唯一的事实来源是 **`git check-ignore`**：本用例把夹具仓库里的每一条路径，用
//! `ageminal_files::read_dir` 的 `ignored` 标记与 `git check-ignore -z --stdin --no-index`
//! 逐条对照。运行时不用 `git check-ignore`（#18 D1），它只做这里的裁判。
//!
//! 夹具**在运行时搭**（而不是提交在 `tests/fixtures/`）：`git check-ignore` 需要真正的
//! 仓库，而 `.git` 目录没法进版本控制——这与 `crates/git/tests/discovery.rs` 的活仓库用例
//! 同一套路。全局 excludes 的那一半另见 `tests/global_excludes.rs`。

mod common;

use std::collections::BTreeMap;
use std::path::Path;

use ageminal_files::read_dir;
use common::{check_ignore, relative, run_git, write, TempDir};

/// 覆盖嵌套 `.gitignore` / `.git/info/exclude` / 取反 / 目录规则 / `.ignore` 的夹具仓库。
fn fixture() -> TempDir {
    let dir = TempDir::new("fidelity");
    let root = dir.path();
    run_git(root, &["init", "-b", "main"]);

    write(
        root,
        ".gitignore",
        "*.log\n!keep.log\nnode_modules/\n!node_modules/keep.txt\nbuild/\nsub/ignored.txt\n",
    );
    write(root, ".git/info/exclude", "excluded.txt\n");
    // `.ignore` 是 ripgrep / ag 的约定，git 不认（#18 D2）——我们也不该认。
    write(root, ".ignore", "not-ignored-by-git.txt\n");
    // 嵌套 `.gitignore`：规则只在它所在的子树生效。
    write(root, "sub/nested/.gitignore", "local-only.txt\n");

    write(root, "visible.txt", "v\n");
    write(root, "excluded.txt", "e\n");
    write(root, "app.log", "log\n");
    write(root, "keep.log", "kept\n");
    write(root, "not-ignored-by-git.txt", "n\n");
    write(root, "node_modules/lib/index.js", "js\n");
    write(root, "node_modules/keep.txt", "k\n");
    write(root, "build/out.js", "js\n");
    write(root, "sub/ignored.txt", "i\n");
    write(root, "sub/normal.txt", "n\n");
    write(root, "sub/nested/local-only.txt", "l\n");
    write(root, "sub/nested/kept.txt", "k\n");

    dir
}

/// 我们自己算出来的裁决（相对路径 → 是否被忽略）。
fn ours(root: &Path) -> BTreeMap<String, bool> {
    let mut verdicts = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in read_dir(root, &dir).unwrap().entries {
            verdicts.insert(relative(root, &entry.path), entry.ignored);
            if entry.is_dir {
                pending.push(entry.path.clone());
            }
        }
    }
    verdicts
}

#[test]
fn matches_git_check_ignore() {
    let fixture = fixture();
    let root = fixture.path();

    let verdicts = ours(root);
    assert!(
        verdicts.contains_key("node_modules/lib/index.js"),
        "夹具没被枚举到"
    );

    let ignored = check_ignore(root, &verdicts.keys().cloned().collect::<Vec<_>>());
    // 别在「两边都空」的假象里通过。
    assert!(!ignored.is_empty(), "夹具里应有被忽略的路径");

    for (path, we_ignore) in &verdicts {
        assert_eq!(
            *we_ignore,
            ignored.contains(path),
            "与 git check-ignore 不一致：{path}"
        );
    }
}

#[test]
fn keeps_a_whitelisted_file_inside_an_ignored_pattern() {
    // `!keep.log` 取反：不要被 `*.log` 吞掉（顺带守住 `ignore` 的取反语义）。
    let fixture = fixture();
    let root = fixture.path();

    let verdicts = ours(root);

    assert!(!verdicts["keep.log"]);
    assert!(verdicts["app.log"]);
}

#[test]
fn cannot_reinclude_a_file_inside_an_ignored_directory() {
    // git 的「父目录被排除则子项不可重新包含」：`!node_modules/keep.txt` 无效。
    // 我们与裁判同一条语义。（D4：这是**标记**层面的语义；「显示忽略文件」打开时前端
    // 一律展示，那条取舍归 #59 的渲染。）
    let fixture = fixture();
    let root = fixture.path();

    let verdicts = ours(root);

    assert!(verdicts["node_modules/keep.txt"]);
}

#[test]
fn does_not_honor_a_dot_ignore_file() {
    // #18 D2：不认 `.ignore`。若误开，我们会比 git 多忽略一条，保真度立刻破。
    let fixture = fixture();
    let root = fixture.path();

    let verdicts = ours(root);

    assert!(!verdicts["not-ignored-by-git.txt"]);
}
