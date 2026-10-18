//! daemon 的拉起接线（issue #64）。
//!
//! UI 侧只做一件事：**确保有一个 daemon 在跑**（能复用就复用）。真正的进程拉起、令牌握手与
//! ping/pong 都在 `ageminal-daemon` 里实现，并被它自己的 Windows 集成测试（真进程）覆盖——
//! 这里保持薄，避免出现「只有真机才能验」的逻辑。
//!
//! 为什么不新增 Tauri 命令：终端交互（#67 起）才需要命令面；现在只要求「守护进程常驻」，
//! 不新增命令就不会牵动 `src/bindings.ts`。
//!
//! 为什么放后台线程：首次拉起要等 daemon 就绪（最长 [`ageminal_daemon::launch::READY_TIMEOUT`]），
//! 不该卡住窗口创建。

#[cfg(windows)]
use std::time::Duration;

/// 在后台确保 daemon 在跑。失败只记录——daemon 起不来不应该让 app 起不来。
///
/// # ⚠️ 开发期：daemon 会锁住构建产物
///
/// daemon 拉起的是 `<主程序目录>\ageminal-daemon.exe`，在 dev 布局里就是
/// `target\<profile>\ageminal-daemon.exe`；而 `tauri-build` 每次构建都会把 sidecar **先删再拷**
/// 到同一个路径（`copy_binaries`）。Windows 上**运行中的 exe 删不掉**，于是构建会以
/// `Os { code: 5, PermissionDenied }` 失败（panic 在 build script 里，看起来像 cargo 的错）。
///
/// **重新构建前先结束它**：`taskkill /IM ageminal-daemon.exe /F`。
///
/// 装机后 daemon 跑在安装目录、构建产物在 `target\`，互不影响；跨 NSIS 更新的同类问题由镜像目录
/// `%LOCALAPPDATA%\Ageminal\daemon-host\<version>\` 解决（REQUIREMENTS.md §5.4 / #89）。
pub fn spawn_ensure() {
    std::thread::spawn(ensure_running);
}

#[cfg(windows)]
fn ensure_running() {
    let Some(exe) = daemon_exe() else {
        eprintln!("[daemon] 找不到 ageminal-daemon（sidecar 未就位？），跳过拉起");
        return;
    };
    match ageminal_daemon::launch::ensure_running(&exe) {
        Ok(info) => eprintln!("[daemon] 就绪：pid={} pipe={}", info.pid, info.pipe),
        Err(error) => eprintln!("[daemon] 拉起失败：{error}"),
    }
}

#[cfg(not(windows))]
fn ensure_running() {}

/// 兜底停止 daemon（正常路径是协议 `Shutdown`）。
///
/// 先 `try_reuse` 确认「当前 daemon.json 对应的 daemon 真的在线」，再按 pid 硬杀，
/// 避免拿上一世代文件里的 pid 误杀复用了 PID 的进程。
pub fn force_stop() {
    #[cfg(windows)]
    {
        let Ok(config) = ageminal_daemon::paths::config_dir() else {
            return;
        };
        if ageminal_daemon::launch::try_reuse(&config).is_none() {
            return;
        }
        let Ok(info) = ageminal_protocol::DaemonInfo::read(&config) else {
            return;
        };
        if !ageminal_daemon::launch::wait_stopped(&info, Duration::ZERO) {
            let _ = ageminal_daemon::launch::terminate_process(&info);
            let _ = ageminal_daemon::launch::wait_stopped(&info, Duration::from_secs(2));
        }
        let _ = ageminal_protocol::DaemonInfo::remove(&config);
    }
}

/// daemon 可执行文件：Tauri 把 `bundle.externalBin` 的 sidecar 放在**主程序同目录**
/// （dev 与安装后都是如此）。
#[cfg(windows)]
fn daemon_exe() -> Option<std::path::PathBuf> {
    let candidate = std::env::current_exe()
        .ok()?
        .parent()?
        .join("ageminal-daemon.exe");
    candidate.exists().then_some(candidate)
}
