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
pub fn spawn_ensure(version: String) {
    std::thread::spawn(move || ensure_running(&version));
}

#[cfg(windows)]
fn ensure_running(version: &str) {
    let Some(exe) = daemon_exe(version) else {
        tracing::warn!("[daemon] 找不到 ageminal-daemon（sidecar 未就位？），跳过拉起");
        return;
    };
    match ageminal_daemon::launch::ensure_running(&exe) {
        Ok(info) => tracing::warn!("[daemon] 就绪：pid={} pipe={}", info.pid, info.pipe),
        Err(error) => tracing::warn!("[daemon] 拉起失败：{error}"),
    }
}

#[cfg(not(windows))]
fn ensure_running(_version: &str) {}

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
fn daemon_exe(version: &str) -> Option<std::path::PathBuf> {
    let source = std::env::current_exe()
        .ok()?
        .parent()?
        .join("ageminal-daemon.exe");
    let config = ageminal_daemon::paths::config_dir().ok()?;
    match ensure_image(&source, &config.join("daemon-host"), version) {
        Ok(path) => Some(path),
        Err(error) => {
            tracing::warn!("[daemon] 镜像搬迁失败：{error}");
            source.exists().then_some(source)
        }
    }
}

/// 把 sidecar 复制到 `daemon-host/<version>/`，并只保留最近两个版本。
fn ensure_image(
    source: &std::path::Path,
    host_root: &std::path::Path,
    version: &str,
) -> Result<std::path::PathBuf, String> {
    let target = host_root.join(version).join("ageminal-daemon.exe");
    let same = target.is_file() && std::fs::read(&target).ok() == std::fs::read(source).ok();
    if !same {
        let bytes = std::fs::read(source).map_err(|error| format!("读取 sidecar 失败：{error}"))?;
        let parent = target
            .parent()
            .ok_or_else(|| "镜像路径无父目录".to_owned())?;
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        let temporary = parent.join("ageminal-daemon.exe.tmp");
        std::fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
        if target.exists() {
            std::fs::remove_file(&target)
                .map_err(|error| format!("替换旧镜像失败（daemon 仍在运行？）：{error}"))?;
        }
        std::fs::rename(&temporary, &target).map_err(|error| error.to_string())?;
    }
    prune_versions(host_root, version);
    Ok(target)
}

fn prune_versions(host_root: &std::path::Path, current: &str) {
    let Ok(entries) = std::fs::read_dir(host_root) else {
        return;
    };
    let mut others: Vec<(std::time::SystemTime, std::path::PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_dir() || entry.file_name() == current {
                return None;
            }
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .ok()?;
            Some((modified, path))
        })
        .collect();
    others.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    for (_, path) in others.into_iter().skip(1) {
        let _ = std::fs::remove_dir_all(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_the_daemon_into_a_versioned_host_and_keeps_two_versions() {
        let root =
            std::env::temp_dir().join(format!("ageminal-daemon-host-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("ageminal-daemon.exe");
        std::fs::write(&source, b"daemon").unwrap();
        let host = root.join("daemon-host");

        let first = ensure_image(&source, &host, "1.0.0").unwrap();
        assert!(first.is_file());
        let second = ensure_image(&source, &host, "1.1.0").unwrap();
        assert!(second.is_file());
        std::fs::create_dir_all(host.join("0.9.0")).unwrap();
        std::fs::write(host.join("0.9.0").join("old.exe"), b"old").unwrap();
        let _ = ensure_image(&source, &host, "1.2.0").unwrap();

        let versions: Vec<String> = std::fs::read_dir(&host)
            .unwrap()
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(versions.contains(&"1.2.0".to_owned()));
        assert!(versions.len() <= 2, "只保留最近两版：{versions:?}");
        let _ = std::fs::remove_dir_all(root);
    }
}
