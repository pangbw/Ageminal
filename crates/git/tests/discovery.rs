//! 与真实 git 交互的发现层：可执行文件定位、版本门槛、类型化错误、活发现。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use ageminal_git::{Git, GitError, GitVersion, MIN_GIT_VERSION};

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// 用完即删的临时目录。
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "ageminal-git-{}-{tag}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// 在夹具仓库里跑一条 git 命令（提交身份用环境变量给全，避免依赖全局配置）。
fn run_git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Ageminal Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Ageminal Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .expect("运行 git 失败");
    assert!(
        output.status.success(),
        "git {args:?} 失败：{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// 本机 git（没有就跳过依赖它的用例）。
fn locate() -> Option<Git> {
    Git::locate(None).ok()
}

// ---------------------------------------------------------------------------
// 版本
// ---------------------------------------------------------------------------

#[test]
fn parses_real_world_versions() {
    let version = |major, minor| GitVersion { major, minor };

    assert_eq!(
        GitVersion::parse("git version 2.36.1.windows.1"),
        Some(version(2, 36))
    );
    assert_eq!(
        GitVersion::parse("git version 2.39.3 (Apple Git-146)"),
        Some(version(2, 39))
    );
    assert_eq!(
        GitVersion::parse("git version 2.36.0.rc1"),
        Some(version(2, 36))
    );
    assert_eq!(GitVersion::parse("git version 3.0"), Some(version(3, 0)));
    assert_eq!(GitVersion::parse("不是版本"), None);

    // 2.36 是 `-z` 的门槛（#17 D3）
    assert!(GitVersion::parse("git version 2.35.2").unwrap() < MIN_GIT_VERSION);
    assert!(GitVersion::parse("git version 2.36.0").unwrap() >= MIN_GIT_VERSION);
}

// ---------------------------------------------------------------------------
// 可执行文件定位
// ---------------------------------------------------------------------------

#[test]
fn missing_candidates_report_not_found() {
    let error = Git::locate_among([PathBuf::from("/ageminal/no/such/git")]).unwrap_err();

    assert!(matches!(error, GitError::NotFound), "{error}");
}

#[cfg(unix)]
#[test]
fn too_old_git_is_rejected() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new("too-old");
    let fake = dir.path().join("git");
    fs::write(&fake, "#!/bin/sh\necho 'git version 2.35.2'\n").unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();

    let error = Git::locate_among([fake]).unwrap_err();

    assert!(
        matches!(error, GitError::TooOld { found, min } if found == GitVersion { major: 2, minor: 35 } && min == MIN_GIT_VERSION),
        "{error}"
    );
}

#[test]
fn locates_system_git() {
    let git = Git::locate(None).expect("环境里应有 git");

    assert!(git.version() >= MIN_GIT_VERSION);
}

// ---------------------------------------------------------------------------
// 类型化错误
// ---------------------------------------------------------------------------

#[test]
fn non_repository_reports_typed_error() {
    let Some(git) = locate() else {
        return;
    };
    let dir = TempDir::new("not-repo");
    let inside = dir.path().join("empty");
    fs::create_dir_all(&inside).unwrap();

    let error = git.worktrees(&inside).unwrap_err();

    assert!(matches!(error, GitError::NotARepository { .. }), "{error}");
}

#[test]
fn missing_directory_reports_failure() {
    let Some(git) = locate() else {
        return;
    };
    let dir = TempDir::new("missing-dir");

    let error = git.worktrees(&dir.path().join("nope")).unwrap_err();

    assert!(matches!(error, GitError::Failed { .. }), "{error}");
}

// ---------------------------------------------------------------------------
// 活发现（不缓存 / 不落盘）
// ---------------------------------------------------------------------------

#[test]
fn discovers_worktrees_live_from_git() {
    let Some(git) = locate() else {
        return;
    };
    let dir = TempDir::new("discover");
    let repo = dir.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    run_git(&repo, &["init", "-b", "main"]);
    fs::write(repo.join("a.txt"), "a\n").unwrap();
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-m", "init"]);

    // 目录名带空格，验证路径原样往返
    let linked = dir.path().join("linked one");
    run_git(
        &repo,
        &["worktree", "add", "-b", "feat", linked.to_str().unwrap()],
    );

    let found = git.discover(&repo, "myproject").unwrap();
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].name, "myproject");
    assert!(found[0].is_main);
    assert_eq!(found[0].branch.as_deref(), Some("main"));
    assert_eq!(found[1].name, "linked one");
    assert_eq!(found[1].branch.as_deref(), Some("feat"));
    assert_eq!(found[1].branch_label.as_deref(), Some("feat"));
    assert!(!found[1].stale);

    // 不缓存 / 不落盘：新加的 worktree 下一次发现立刻可见
    let second = dir.path().join("linked-two");
    run_git(
        &repo,
        &["worktree", "add", "--detach", second.to_str().unwrap()],
    );
    let found = git.discover(&repo, "myproject").unwrap();
    assert_eq!(found.len(), 3);
    let detached = found.iter().find(|tree| tree.path == second).unwrap();
    assert!(detached.detached);
    assert_eq!(detached.branch_label.as_deref().map(str::len), Some(7));

    // 目录被删 → git 标 prunable，模型标 stale
    fs::remove_dir_all(&second).unwrap();
    let found = git.discover(&repo, "myproject").unwrap();
    let removed = found
        .iter()
        .find(|tree| tree.path == second)
        .expect("prunable 的 worktree 仍应被列出（绝不自动 prune）");
    assert!(removed.prunable.is_present());
    assert!(removed.stale);
}
