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

/// 在后台确保 daemon 在跑。失败只记录——daemon 起不来不应该让 app 起不来。
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
