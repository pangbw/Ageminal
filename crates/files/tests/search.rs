//! 全树搜索（#19 D8 / #59 验收 2）：命中、两个开关、上限、取消、超时、`.git` 永不出现。

mod common;

use std::time::Duration;

use ageminal_files::{search, SearchOptions, MAX_RESULTS};
use common::{run_git, write, TempDir};

const GENEROUS: Duration = Duration::from_secs(30);

fn fixture() -> TempDir {
    let dir = TempDir::new("search");
    let root = dir.path();
    run_git(root, &["init", "-b", "main"]);
    write(root, ".gitignore", "node_modules/\n");

    write(root, "alpha.txt", "a\n");
    write(root, "beta.txt", "b\n");
    write(root, ".hidden-alpha", "h\n");
    write(root, "sub/alpha-two.txt", "a\n");
    write(root, "sub/deep/alpha-three.txt", "a\n");
    write(root, "node_modules/pkg/alpha.js", "js\n");

    dir
}

fn both_off() -> SearchOptions {
    SearchOptions {
        show_hidden: false,
        show_ignored: false,
    }
}

fn relative_paths(root: &std::path::Path, options: SearchOptions, query: &str) -> Vec<String> {
    search(root, query, options, GENEROUS, || true)
        .hits
        .into_iter()
        .map(|hit| hit.relative)
        .collect()
}

#[test]
fn matches_names_and_respects_the_default_toggles() {
    let fixture = fixture();
    let root = fixture.path();

    let hits = relative_paths(root, both_off(), "alpha");

    // 目录优先/字典序由 read_dir 管，搜索按相对路径排序。
    assert_eq!(
        hits,
        vec!["alpha.txt", "sub/alpha-two.txt", "sub/deep/alpha-three.txt"]
    );
}

#[test]
fn includes_ignored_entries_only_when_asked() {
    let fixture = fixture();
    let root = fixture.path();

    let options = SearchOptions {
        show_hidden: false,
        show_ignored: true,
    };
    let outcome = search(root, "alpha", options, GENEROUS, || true);

    let node_modules = outcome
        .hits
        .iter()
        .find(|hit| hit.relative == "node_modules/pkg/alpha.js")
        .expect("打开「显示忽略文件」后应搜到被忽略项");
    assert!(node_modules.ignored, "被忽略的命中要带标记，供弱化显示");
}

#[test]
fn includes_dotfiles_only_when_asked() {
    let fixture = fixture();
    let root = fixture.path();

    let options = SearchOptions {
        show_hidden: true,
        show_ignored: false,
    };

    assert!(relative_paths(root, options, "hidden").contains(&".hidden-alpha".to_owned()));
    assert!(relative_paths(root, both_off(), "hidden").is_empty());
}

#[test]
fn never_returns_paths_inside_the_git_directory() {
    let fixture = fixture();
    let root = fixture.path();
    let options = SearchOptions {
        show_hidden: true,
        show_ignored: true,
    };

    // `.git/` 里确实有 config 这类文件；打开两个开关也不该搜到。
    let hits = relative_paths(root, options, "config");

    assert!(
        hits.iter().all(|path| !path.starts_with(".git/")),
        "{hits:?}"
    );
}

#[test]
fn truncates_at_the_result_cap() {
    let dir = TempDir::new("search-cap");
    let root = dir.path();
    run_git(root, &["init", "-b", "main"]);
    for index in 0..MAX_RESULTS + 1 {
        write(root, &format!("hit-{index:03}.txt"), "h\n");
    }

    let outcome = search(root, "hit-", both_off(), GENEROUS, || true);

    assert_eq!(outcome.hits.len(), MAX_RESULTS);
    assert!(outcome.truncated);
    assert!(!outcome.timed_out);
}

#[test]
fn stops_early_when_the_caller_cancels() {
    let dir = TempDir::new("search-cancel");
    let root = dir.path();
    run_git(root, &["init", "-b", "main"]);
    for index in 0..MAX_RESULTS + 1 {
        write(root, &format!("hit-{index:03}.txt"), "h\n");
    }

    let mut calls = 0;
    let outcome = search(root, "hit-", both_off(), GENEROUS, || {
        calls += 1;
        calls <= 5
    });

    assert!(outcome.hits.len() < MAX_RESULTS);
    assert!(!outcome.truncated, "取消不是截断");
    assert!(!outcome.timed_out, "取消不是超时");
}

#[test]
fn reports_a_timeout_with_out_a_full_result() {
    let fixture = fixture();
    let root = fixture.path();

    let outcome = search(root, "alpha", both_off(), Duration::ZERO, || true);

    assert!(outcome.timed_out);
    assert!(outcome.hits.is_empty());
}

#[test]
fn ignores_an_empty_query() {
    let fixture = fixture();
    let root = fixture.path();

    let outcome = search(root, "   ", both_off(), GENEROUS, || true);

    assert!(outcome.hits.is_empty());
    assert!(!outcome.timed_out);
}
