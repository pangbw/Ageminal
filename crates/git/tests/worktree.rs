//! porcelain `-z` 的解析与模型映射：夹具驱动（纯逻辑，无进程）。

use std::fs;
use std::path::{Path, PathBuf};

use ageminal_git::worktree::{parse_porcelain_z, to_worktrees, Marker};
use ageminal_git::GitError;

fn fixture(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/worktree")
        .join(name);
    fs::read(&path).unwrap_or_else(|error| panic!("读取夹具 {} 失败：{error}", path.display()))
}

// ---------------------------------------------------------------------------
// 解析
// ---------------------------------------------------------------------------

#[test]
fn parses_single_fixture() {
    let raws = parse_porcelain_z(&fixture("single.porcelain-z")).unwrap();

    assert_eq!(raws.len(), 1);
    assert_eq!(raws[0].path, PathBuf::from("/repo/single"));
    assert_eq!(raws[0].branch.as_deref(), Some("refs/heads/main"));
    assert!(!raws[0].detached);
    assert!(!raws[0].bare);
    assert!(!raws[0].locked.is_present());
    assert!(!raws[0].prunable.is_present());
}

#[test]
fn parses_multi_fixture() {
    let raws = parse_porcelain_z(&fixture("multi.porcelain-z")).unwrap();

    assert_eq!(raws.len(), 6);

    // 主工作树：分支
    assert_eq!(raws[0].path, PathBuf::from("/repo/alpha"));
    assert_eq!(raws[0].branch.as_deref(), Some("refs/heads/main"));

    // 路径含引号：**原样**，解析器不去引号
    assert_eq!(raws[1].path, PathBuf::from("/repo/alpha-q\"uote"));
    assert!(raws[1].detached);
    assert!(raws[1].branch.is_none());

    // 路径含空格 + 中文
    assert_eq!(raws[2].path, PathBuf::from("/repo/alpha-wt-detach 中文"));
    assert!(raws[2].detached);

    // locked 带原因：原因与键同一 token
    assert_eq!(
        raws[3].locked,
        Marker::Present {
            reason: Some("hold for review".to_owned())
        }
    );
    assert_eq!(raws[3].locked.reason(), Some("hold for review"));

    // locked 无原因
    assert_eq!(raws[4].locked, Marker::Present { reason: None });
    assert!(raws[4].locked.is_present());
    assert_eq!(raws[4].locked.reason(), None);

    // prunable
    assert!(raws[5].prunable.is_present());
    assert_eq!(
        raws[5].prunable.reason(),
        Some("gitdir file points to non-existent location")
    );
}

#[test]
fn parses_bare_fixture() {
    let raws = parse_porcelain_z(&fixture("bare.porcelain-z")).unwrap();

    assert_eq!(raws.len(), 2);
    // bare 主工作树只有 worktree + bare：没有 HEAD / branch
    assert!(raws[0].bare);
    assert!(raws[0].head.is_none());
    assert!(raws[0].branch.is_none());
    assert!(!raws[0].detached);
    // 其链接 worktree 正常
    assert_eq!(raws[1].branch.as_deref(), Some("refs/heads/main"));
    assert!(!raws[1].bare);
}

#[test]
fn ignores_unknown_keys() {
    // git 未来加字段时不应炸。
    let raws = parse_porcelain_z(b"worktree /repo/x\0future field\0\0").unwrap();

    assert_eq!(raws.len(), 1);
    assert_eq!(raws[0].path, PathBuf::from("/repo/x"));
}

#[test]
fn accepts_record_without_trailing_separator() {
    // 字段本身以 \0 结尾；缺失记录收尾分隔时按已读字段收尾（不静默丢）。
    let raws = parse_porcelain_z(b"worktree /repo/x\0HEAD abc\0branch refs/heads/main").unwrap();

    assert_eq!(raws.len(), 1);
    assert_eq!(raws[0].path, PathBuf::from("/repo/x"));
    assert_eq!(raws[0].head.as_deref(), Some("abc"));
}

#[test]
fn rejects_empty_output() {
    assert!(matches!(
        parse_porcelain_z(b""),
        Err(GitError::MalformedOutput { .. })
    ));
}

#[test]
fn rejects_non_utf8_field() {
    assert!(matches!(
        parse_porcelain_z(b"worktree /repo/\xff\xfe\0\0"),
        Err(GitError::MalformedOutput { .. })
    ));
}

#[test]
fn rejects_record_without_path() {
    assert!(matches!(
        parse_porcelain_z(b"HEAD abc\0\0"),
        Err(GitError::MalformedOutput { .. })
    ));
}

// ---------------------------------------------------------------------------
// 模型映射
// ---------------------------------------------------------------------------

#[test]
fn maps_ui_model() {
    let raws = parse_porcelain_z(&fixture("multi.porcelain-z")).unwrap();
    let trees = to_worktrees(&raws, "alpha", |_| true);

    assert_eq!(trees.len(), 6);

    // 主工作树用项目名，linked 用目录 basename
    assert_eq!(trees[0].name, "alpha");
    assert!(trees[0].is_main);
    assert_eq!(trees[0].branch_label.as_deref(), Some("main"));
    assert!(!trees[0].stale);

    assert_eq!(trees[1].name, "alpha-q\"uote");
    assert!(!trees[1].is_main);
    // detached → 短 SHA（7 位）
    assert_eq!(trees[1].branch.as_deref(), None);
    assert_eq!(trees[1].branch_label.as_deref(), Some("bc8ce16"));

    // 目录名含空格 / 中文
    assert_eq!(trees[2].name, "alpha-wt-detach 中文");

    // 分支名含斜杠：去掉 refs/heads/ 前缀，保留短名
    assert_eq!(trees[3].branch.as_deref(), Some("feature/x"));
    assert_eq!(trees[3].branch_label.as_deref(), Some("feature/x"));
    assert!(trees[3].locked.is_present());

    // prunable → stale；普通 worktree 不 stale
    assert!(!trees[4].stale);
    assert!(trees[5].stale);
}

#[test]
fn bare_main_has_no_branch_label() {
    let raws = parse_porcelain_z(&fixture("bare.porcelain-z")).unwrap();
    let trees = to_worktrees(&raws, "bare", |_| true);

    assert!(trees[0].bare);
    assert_eq!(trees[0].name, "bare");
    assert_eq!(trees[0].branch_label, None);
    assert_eq!(trees[1].name, "bare-wt");
    assert_eq!(trees[1].branch_label.as_deref(), Some("main"));
}

#[test]
fn missing_directory_marks_stale() {
    let raws = parse_porcelain_z(&fixture("single.porcelain-z")).unwrap();
    let trees = to_worktrees(&raws, "single", |_| false);

    assert!(trees[0].stale);
}
