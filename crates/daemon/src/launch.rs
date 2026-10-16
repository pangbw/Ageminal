//! 把 daemon 以 **detached** 方式拉起（issue #64）。
//!
//! 启动规格（REQUIREMENTS.md §5.1 / #40 真机实测）：
//! `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP`，且 **stdio 三句柄全指向 NUL**——daemon 与
//! app 的管道彻底脱钩。（#40 实测：这不影响 ConPTY；是否带 `CREATE_NO_WINDOW` 等都与问题无关。）
//!
//! 拉起之后靠 `daemon.json` + 一次握手确认「真的活着」，而不是靠进程句柄——daemon 是长命进程，
//! 句柄随手丢掉。

use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS};

use crate::{client, paths, DaemonError, DaemonInfo};

/// 覆盖管道名（集成测试用；生产不设 → 按当前用户派生）。
pub const ARG_PIPE: &str = "--pipe";
/// 覆盖配置根（集成测试用；生产不设 → `%LOCALAPPDATA%\Ageminal`）。
pub const ARG_CONFIG_DIR: &str = "--config-dir";

/// 拉起后等待就绪的上限。
pub const READY_TIMEOUT: Duration = Duration::from_secs(10);

/// 探活（读状态文件 + 握手）的单次超时。
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);

/// 确保有一个 daemon 在跑：能复用就复用，不能就拉起并等它就绪。
pub fn ensure_running(exe: &Path) -> Result<DaemonInfo, DaemonError> {
    ensure_running_with(exe, &paths::pipe_name()?, &paths::config_dir()?)
}

/// [`ensure_running`] 的可注入版本（集成测试用显式的管道名 / 配置根）。
pub fn ensure_running_with(
    exe: &Path,
    pipe: &str,
    config_dir: &Path,
) -> Result<DaemonInfo, DaemonError> {
    if let Some(info) = try_reuse(config_dir) {
        return Ok(info);
    }
    // 子进程句柄随手丢掉：DETACHED 进程的存活不依赖它。
    let _child = spawn_detached(exe, pipe, config_dir)?;
    wait_ready(config_dir, READY_TIMEOUT)
}

/// 以 detached 方式拉起一个 daemon 进程。
pub fn spawn_detached(exe: &Path, pipe: &str, config_dir: &Path) -> Result<Child, DaemonError> {
    let mut command = Command::new(exe);
    command
        .arg(ARG_PIPE)
        .arg(pipe)
        .arg(ARG_CONFIG_DIR)
        .arg(config_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    command.spawn().map_err(|error| DaemonError::Launch {
        reason: format!("{}：{error}", exe.display()),
    })
}

/// 读到状态文件、连上、ping 通 → 复用这个 daemon。
pub fn try_reuse(config_dir: &Path) -> Option<DaemonInfo> {
    let info = read_info(config_dir).ok()?;
    let mut client = client::connect(&info, PROBE_TIMEOUT).ok()?;
    client.ping(1).ok()?;
    Some(info)
}

/// 读 `daemon.json`。
pub fn read_info(config_dir: &Path) -> Result<DaemonInfo, DaemonError> {
    let path = paths::state_file_in(config_dir);
    let bytes = std::fs::read(&path).map_err(|source| DaemonError::State {
        path: path.clone(),
        source,
    })?;
    serde_json::from_slice(&bytes).map_err(|error| DaemonError::Codec {
        reason: error.to_string(),
    })
}

/// 轮询状态文件 + 握手，直到就绪或超时。
pub fn wait_ready(config_dir: &Path, timeout: Duration) -> Result<DaemonInfo, DaemonError> {
    let deadline = Instant::now() + timeout;
    let mut reason = "状态文件尚未出现".to_owned();
    while Instant::now() < deadline {
        match try_reuse(config_dir) {
            Some(info) => return Ok(info),
            None => {
                if let Ok(info) = read_info(config_dir) {
                    reason = format!("握手未通过（daemon pid={}）", info.pid);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err(DaemonError::Timeout { timeout, reason })
}
