//! `read_dir` 的可见性标记（#58）：`.git` 永不出现、dotfile 标记、确定排序、错误边界。

mod common;

use std::fs;

use ageminal_files::{read_dir, FilesError};
use common::TempDir;

#[test]
fn omits_git_and_flags_dotfiles() {
    let dir = TempDir::new("listing");
    let root = dir.path();
    fs::create_dir_all(root.join(".git")).unwrap();
    fs::write(root.join(".git/config"), "x\n").unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join(".env"), "SECRET=1\n").unwrap();
    fs::write(root.join("README.md"), "readme\n").unwrap();
    fs::write(root.join("a.txt"), "a\n").unwrap();

    let entries = read_dir(root, root).unwrap();
    let names: Vec<_> = entries.iter().map(|entry| entry.name.as_str()).collect();

    // 目录优先，其余按名字（大小写不敏感）——`.git` 不在其中。
    assert_eq!(names, vec!["src", ".env", "a.txt", "README.md"]);

    let src = entries.iter().find(|entry| entry.name == "src").unwrap();
    assert!(src.is_dir);
    assert!(!src.hidden);

    // dotfile 是「维度 2」，与「被忽略」无关：这里不是 git 仓库，故 ignored 恒假。
    let env = entries.iter().find(|entry| entry.name == ".env").unwrap();
    assert!(env.hidden);
    assert!(!env.ignored);
}

#[test]
fn ignores_nothing_outside_a_git_repository() {
    let dir = TempDir::new("non-git");
    let root = dir.path();
    // 没有 `.git`：`.gitignore` 不该起作用（用户工作区场景，#93）。
    fs::write(root.join(".gitignore"), "secret.txt\n").unwrap();
    fs::write(root.join("secret.txt"), "s\n").unwrap();
    fs::write(root.join("visible.txt"), "v\n").unwrap();

    let entries = read_dir(root, root).unwrap();

    let secret = entries
        .iter()
        .find(|entry| entry.name == "secret.txt")
        .unwrap();
    assert!(!secret.ignored);
}

#[test]
fn lists_a_nested_directory_with_root_relative_marking() {
    let dir = TempDir::new("nested");
    let root = dir.path();
    let sub = root.join("sub");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("inner.txt"), "i\n").unwrap();

    let entries = read_dir(root, &sub).unwrap();

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "inner.txt");
    assert_eq!(entries[0].path, sub.join("inner.txt"));
}

#[test]
fn rejects_a_directory_outside_the_root() {
    let root = TempDir::new("root");
    let other = TempDir::new("other");

    let error = read_dir(root.path(), other.path()).unwrap_err();

    assert!(matches!(error, FilesError::OutsideRoot { .. }), "{error}");
}

#[test]
fn rejects_parent_directory_traversal() {
    let dir = TempDir::new("traversal");
    let root = dir.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    // `repo/..` 以字符串前缀看仍在 `repo` 之下，但它指向 root 之外。
    let escape = root.join("..");

    let error = read_dir(&root, &escape).unwrap_err();

    assert!(matches!(error, FilesError::OutsideRoot { .. }), "{error}");
}

#[test]
fn reports_a_missing_directory() {
    let dir = TempDir::new("missing");

    let error = read_dir(dir.path(), &dir.path().join("nope")).unwrap_err();

    assert!(matches!(error, FilesError::NotFound { .. }), "{error}");
}
