//! 把 daemon 以 **detached** 方式拉起（issue #64）。
//!
//! 拉起规格（REQUIREMENTS.md §5.1 / #40 真机实测）：`DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP`，
//! 且 **stdio 三句柄全指向 NUL**——daemon 与 app 的管道彻底脱钩。（#40 实测：这不影响 ConPTY；
//! 是否带 `CREATE_NO_WINDOW` 等都与本问题无关。）
//!
//! > 注意别和 `CONTEXT.md` §2 的「**启动规格**（launch spec）」混了——那是**会话**首次启动的参数
//! > （`sid` / cwd / 启动命令…，只存在 app 的 `state.json`）。这里说的是 daemon 自己的拉起。
//!
//! 拉起之后靠 `daemon.json` + 一次握手确认「真的活着」，而不是靠进程句柄——daemon 是长命进程，
//! 句柄随手丢掉。

use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
use windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE;
use windows_sys::Win32::System::Threading::{
    OpenProcess, TerminateProcess, WaitForSingleObject, CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS,
    PROCESS_TERMINATE,
};

use crate::{client, paths, DaemonError, DaemonInfo};

/// 覆盖管道名（集成测试用；生产不设 → 按当前用户派生）。
pub const ARG_PIPE: &str = "--pipe";
/// 覆盖配置根（集成测试用；生产不设 → `%LOCALAPPDATA%\Ageminal`）。
pub const ARG_CONFIG_DIR: &str = "--config-dir";

/// 拉起后等待就绪的上限。
pub const READY_TIMEOUT: Duration = Duration::from_secs(10);

/// 探活（读状态文件 + 握手）的单次超时。
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);

/// 轮询间隔。
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// 确保有一个 daemon 在跑：能复用就复用，不能就按**生产落点**拉起并等它就绪。
pub fn ensure_running(exe: &Path) -> Result<DaemonInfo, DaemonError> {
    ensure_running_with(exe, &paths::pipe_name()?, &paths::config_dir()?)
}

/// 落点可注入的版本：生产走 [`ensure_running`]；集成测试把管道名与配置根挪到临时目录，
/// 免得和用户机器上真在跑的 daemon 抢东西。
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

/// 试一次：读到状态文件、连上、ping 通 → 这个 daemon 活着可用。
pub fn try_reuse(config_dir: &Path) -> Option<DaemonInfo> {
    let info = DaemonInfo::read(config_dir).ok()?;
    let mut client = client::connect(&info, PROBE_TIMEOUT).ok()?;
    client.ping().ok()?;
    Some(info)
}

/// 等 daemon 进程退出；返回是否已经停下（进程不存在也视为已停）。
///
/// 用于「结束所有会话并退出」：收到 `ShutdownAck` 后再确认进程真的走了。
pub fn wait_stopped(info: &DaemonInfo, timeout: Duration) -> bool {
    let process = unsafe { OpenProcess(SYNCHRONIZE, 0, info.pid) };
    if process.is_null() {
        return true;
    }
    let milliseconds = timeout.as_millis().min(u32::MAX as u128) as u32;
    let waited = unsafe { WaitForSingleObject(process, milliseconds) };
    unsafe { CloseHandle(process) };
    waited == WAIT_OBJECT_0
}

/// 兜底硬杀 daemon（正常路径是先发 `Shutdown`）。只在已确认同一 daemon 在线时调用。
pub fn terminate_process(info: &DaemonInfo) -> bool {
    let process = unsafe { OpenProcess(PROCESS_TERMINATE | SYNCHRONIZE, 0, info.pid) };
    if process.is_null() {
        return true;
    }
    let terminated = unsafe { TerminateProcess(process, 1) } != 0;
    unsafe { CloseHandle(process) };
    terminated
}

/// 优雅结束正在运行的 daemon 及其所有会话（卸载钩子用）。
///
/// 没有 daemon / 已停止都视为成功；超时后兜底终止进程，保证卸载流程不悬挂。
pub fn shutdown_running(config_dir: &Path) -> Result<(), DaemonError> {
    let Some(info) = try_reuse(config_dir) else {
        return Ok(());
    };
    let mut client = client::connect(&info, Duration::from_secs(3))?;
    client.shutdown()?;
    drop(client);
    if !wait_stopped(&info, Duration::from_secs(10)) {
        let _ = terminate_process(&info);
        let _ = wait_stopped(&info, Duration::from_secs(3));
    }
    Ok(())
}

/// 在 `timeout` 内反复探测「有没有在线 daemon」。**`timeout` 为 0 也会先试一次**。
///
/// 两处用它：拉起后等就绪（[`wait_ready`]），以及 daemon 侧区分「单例冲突」与「真的建不起管道」
/// ——后者见 [`crate::server`]。
pub fn poll_running(config_dir: &Path, timeout: Duration) -> Option<DaemonInfo> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(info) = try_reuse(config_dir) {
            return Some(info);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// 轮询到就绪，或超时报错（带上卡在哪一步，便于诊断）。
pub fn wait_ready(config_dir: &Path, timeout: Duration) -> Result<DaemonInfo, DaemonError> {
    poll_running(config_dir, timeout).ok_or_else(|| DaemonError::Timeout {
        timeout,
        reason: match DaemonInfo::read(config_dir) {
            Ok(info) => format!("握手未通过（daemon pid={}）", info.pid),
            Err(_) => "状态文件尚未出现".to_owned(),
        },
    })
}
