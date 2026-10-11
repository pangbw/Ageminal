//! git 可执行文件的发现、版本校验与短超时执行（#17 D3 / D4 / D8）。
//!
//! 这里是**唯一**碰进程与文件系统的地方；porcelain 解析与模型映射留在
//! [`crate::worktree`]，以便纯逻辑跨平台可测。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use crate::error::{GitError, GitVersion, MIN_GIT_VERSION};
use crate::worktree::{self, RawWorktree, Worktree};

/// 单条 git 命令的超时（#17 D8「短超时 + kill」）。
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);

/// 超时轮询的间隔。
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// 已定位且版本达标的 git 可执行文件。
#[derive(Debug, Clone)]
pub struct Git {
    exe: PathBuf,
    version: GitVersion,
}

impl Git {
    /// 按 #17 D4 的规则定位 git。
    ///
    /// `override_exe` 来自设置项，**优先且权威**：一旦指定，就只用它，且失败会明确报错
    /// （不静默回退到别的 git）。未指定时走 [`Git::locate_among`] 的候选序列：
    /// PATH → Windows 标准安装位。
    pub fn locate(override_exe: Option<PathBuf>) -> Result<Self, GitError> {
        let Some(exe) = override_exe else {
            return Self::locate_among(auto_candidates());
        };
        let version = probe_version(&exe)?;
        Self::checked(exe, version)
    }

    /// 从候选序列里探测第一个**可用且版本达标**的 git。
    ///
    /// 候选不可用（路径不存在 / 不是 git）就跳过；所有候选都不可用时，
    /// 若其间看到过低版本则报 [`GitError::TooOld`]，否则报 [`GitError::NotFound`]。
    pub fn locate_among(candidates: impl IntoIterator<Item = PathBuf>) -> Result<Self, GitError> {
        let mut too_old: Option<GitVersion> = None;
        for exe in candidates {
            match probe_version(&exe) {
                Ok(version) if version >= MIN_GIT_VERSION => return Self::checked(exe, version),
                Ok(version) => too_old = Some(version),
                Err(_) => continue,
            }
        }
        match too_old {
            Some(found) => Err(GitError::TooOld {
                found,
                min: MIN_GIT_VERSION,
            }),
            None => Err(GitError::NotFound),
        }
    }

    /// 已校验的 git 版本。
    pub fn version(&self) -> GitVersion {
        self.version
    }

    /// `git -C <repo_root> worktree list --porcelain -z` 的原始解析结果（不落盘）。
    pub fn worktrees(&self, repo_root: &Path) -> Result<Vec<RawWorktree>, GitError> {
        let output = self.run(
            repo_root,
            &["worktree", "list", "--porcelain", "-z"],
            "worktree list",
        )?;
        worktree::parse_porcelain_z(&output)
    }

    /// 发现并映射一个仓库的全部 worktree。
    ///
    /// 每次调用都直接问 git（不做缓存、不落盘）；`project_name` 只用于主工作树的显示名。
    pub fn discover(
        &self,
        repo_root: &Path,
        project_name: &str,
    ) -> Result<Vec<Worktree>, GitError> {
        let raws = self.worktrees(repo_root)?;
        Ok(worktree::to_worktrees(&raws, project_name, |path| {
            path.exists()
        }))
    }

    /// 以 `git -C <cwd> <args>` 执行并捕获 stdout（短超时 + kill）。
    ///
    /// `command` 只用于错误信息；建议用稳定的英文命令名（如 `"worktree list"`）。
    /// 供本模块与后续 git 调用（如 #58 的 `check-ignore`）复用。
    pub fn run(
        &self,
        cwd: &Path,
        args: &[&str],
        command: &'static str,
    ) -> Result<Vec<u8>, GitError> {
        let (status, stdout, stderr) = run_raw(&self.exe, Some(cwd), args, command)?;

        if status.success() {
            return Ok(stdout);
        }

        // git 的诊断可能被本地化，故命令一律带 `LC_ALL=C`（见 `command_for`）。
        let stderr = String::from_utf8_lossy(&stderr).trim().to_owned();
        if stderr.contains("not a git repository") {
            Err(GitError::NotARepository {
                repo_root: cwd.to_path_buf(),
            })
        } else {
            Err(GitError::Failed {
                command,
                code: status.code(),
                stderr,
            })
        }
    }

    fn checked(exe: PathBuf, version: GitVersion) -> Result<Self, GitError> {
        if version < MIN_GIT_VERSION {
            return Err(GitError::TooOld {
                found: version,
                min: MIN_GIT_VERSION,
            });
        }
        Ok(Self { exe, version })
    }
}

/// 候选序列：PATH 上的 `git`，再补 Windows 标准安装位。
///
/// 从资源管理器启动时进程可能没有 PATH，故显式列出标准位置（#17 D4）。
fn auto_candidates() -> Vec<PathBuf> {
    let relative = Path::new("Git").join("cmd").join("git.exe");
    let mut candidates = vec![PathBuf::from(if cfg!(windows) { "git.exe" } else { "git" })];
    for variable in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(base) = std::env::var_os(variable) {
            candidates.push(PathBuf::from(base).join(&relative));
        }
    }
    if let Some(base) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(PathBuf::from(base).join("Programs").join(&relative));
    }
    candidates
}

/// 探测一个可执行文件的 `--version`。
fn probe_version(exe: &Path) -> Result<GitVersion, GitError> {
    let (status, stdout, stderr) = run_raw(exe, None, &["--version"], "version")?;

    if !status.success() {
        return Err(failed("version", status, &stderr));
    }

    let text = String::from_utf8_lossy(&stdout);
    GitVersion::parse(text.trim()).ok_or_else(|| GitError::MalformedOutput {
        reason: format!("无法解析 git --version 输出：{}", text.trim()),
    })
}

/// 统一的「起进程 → 等（带超时）→ 收三个出口」：版本探测与业务命令共用。
fn run_raw(
    exe: &Path,
    cwd: Option<&Path>,
    args: &[&str],
    command: &'static str,
) -> Result<(ExitStatus, Vec<u8>, Vec<u8>), GitError> {
    let mut process = command_for(exe);
    if let Some(cwd) = cwd {
        process.arg("-C").arg(cwd);
    }
    process.args(args);

    let child = process
        .spawn()
        .map_err(|source| GitError::Spawn { command, source })?;
    wait_with_timeout(child, command, COMMAND_TIMEOUT)
}

/// 非零退出的统一映射。
fn failed(command: &'static str, status: ExitStatus, stderr: &[u8]) -> GitError {
    GitError::Failed {
        command,
        code: status.code(),
        stderr: String::from_utf8_lossy(stderr).trim().to_owned(),
    }
}

/// 统一的子进程配置：管道化输出、关掉交互提示、固定诊断语言。
///
/// `LC_ALL=C` 让 git 的错误信息稳定为英文，`NotARepository` 才能靠子串可靠识别；
/// 它也**不影响** porcelain 输出（locale 无关）。
fn command_for(exe: &Path) -> Command {
    let mut command = Command::new(exe);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("LC_ALL", "C")
        .env("GIT_TERMINAL_PROMPT", "0");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // GUI 进程里不要弹出控制台窗口。
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// 等待子进程退出，超时则 kill；stdout / stderr 边跑边读，避免管道写满死锁。
fn wait_with_timeout(
    mut child: Child,
    command: &'static str,
    timeout: Duration,
) -> Result<(ExitStatus, Vec<u8>, Vec<u8>), GitError> {
    let stdout = child.stdout.take().expect("stdout 已被管道化");
    let stderr = child.stderr.take().expect("stderr 已被管道化");
    let read_stdout = std::thread::spawn(move || read_to_end(stdout));
    let read_stderr = std::thread::spawn(move || read_to_end(stderr));

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                // 让读线程收尾，避免游离句柄。
                let _ = read_stdout.join();
                let _ = read_stderr.join();
                return Err(GitError::Timeout { command, timeout });
            }
            Ok(None) => std::thread::sleep(POLL_INTERVAL),
            Err(source) => {
                let _ = read_stdout.join();
                let _ = read_stderr.join();
                return Err(GitError::Spawn { command, source });
            }
        }
    };

    let stdout = read_stdout.join().unwrap_or_default();
    let stderr = read_stderr.join().unwrap_or_default();
    Ok((status, stdout, stderr))
}

fn read_to_end(mut pipe: impl Read) -> Vec<u8> {
    let mut buffer = Vec::new();
    let _ = pipe.read_to_end(&mut buffer);
    buffer
}
