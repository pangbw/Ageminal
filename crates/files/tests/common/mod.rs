#![allow(dead_code)]
//! 文件域集成测试的共用夹具：临时目录、git 调用与 `git check-ignore` 裁判。

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// 用完即删的临时目录。
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> Self {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "ageminal-files-{}-{tag}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// 在夹具里跑一条 git 命令（提交身份用环境变量给全，避免依赖全局配置）。
pub fn run_git(dir: &Path, args: &[&str]) {
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

/// 写一个文件（自动建父目录），路径用 `/` 分隔的相对路径。
pub fn write(root: &Path, relative: &str, body: &str) {
    let path = root.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, body).unwrap();
}

/// `path` 相对 `root` 的展示路径，统一用 `/` 分隔（与 git 输出对齐）。
pub fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap()
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// `git check-ignore -z --stdin --no-index` 的裁决集合：输入 `paths`，回被忽略的那些。
///
/// 这是保真度的**裁判**（#18 D1：运行时不用它）。`--no-index` 让裁判只看规则、不看暂存区。
pub fn check_ignore(root: &Path, paths: &[String]) -> BTreeSet<String> {
    let mut input = Vec::new();
    for path in paths {
        input.extend_from_slice(path.as_bytes());
        input.push(0);
    }

    let mut child = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["check-ignore", "-z", "--stdin", "--no-index"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("运行 git check-ignore 失败");
    child.stdin.take().unwrap().write_all(&input).unwrap();
    let output = child.wait_with_output().unwrap();

    // 0 = 有被忽略的路径，1 = 一条都没有；其余是错误。
    assert!(
        matches!(output.status.code(), Some(0) | Some(1)),
        "git check-ignore 失败：{}",
        String::from_utf8_lossy(&output.stderr)
    );

    output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|chunk| !chunk.is_empty())
        .map(|chunk| {
            String::from_utf8_lossy(chunk)
                .trim_end_matches('/')
                .replace('\\', "/")
        })
        .collect()
}
