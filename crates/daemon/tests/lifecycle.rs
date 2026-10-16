//! daemon 生命周期与握手的**真进程**集成测试（issue #64）。
//!
//! 这些用例必须有真进程才有意义：单例守卫（`FILE_FLAG_FIRST_PIPE_INSTANCE`）、stdio 指向 NUL、
//! 管道 ACL、握手，全都在进程边界上。`cargo test` 会把 `ageminal-daemon` 一起编出来，
//! 用 `CARGO_BIN_EXE_ageminal-daemon` 拿到它的路径。
//!
//! 非 Windows 整文件跳过——daemon 只有 Windows 实现。
//!
//! **未自动覆盖**：验收 4「**别的**用户被 ACL 拒绝」需要第二个 Windows 账户，属人工验收。
//! 自动化的那半（描述符真的被挂到管道上、且只有当前用户）在 `server` 的单测里。

#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use ageminal_daemon::launch::{self, READY_TIMEOUT};
use ageminal_daemon::{client, DaemonError, DaemonInfo};

/// `cargo test` 编出来的 daemon 可执行文件。
const DAEMON_EXE: &str = env!("CARGO_BIN_EXE_ageminal-daemon");

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// 每个用例专属的夹具：配置根与管道名都**不与用户机器上真在跑的 daemon 冲突**。
struct Fixture {
    config_dir: PathBuf,
    pipe: String,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let unique = format!(
            "{}-{tag}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let config_dir = std::env::temp_dir().join(format!("ageminal-daemon-{unique}"));
        let _ = std::fs::remove_dir_all(&config_dir);
        std::fs::create_dir_all(&config_dir).unwrap();
        Self {
            config_dir,
            pipe: format!(r"\\.\pipe\ageminal-test-{unique}"),
        }
    }

    fn exe(&self) -> &Path {
        Path::new(DAEMON_EXE)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.config_dir);
    }
}

/// 拉起的 daemon：握过手的发现信息 + 进程句柄。**析构一定杀掉**，坏了也不留孤儿进程。
struct Spawned {
    child: Child,
    info: DaemonInfo,
}

impl Spawned {
    fn start(fixture: &Fixture) -> Self {
        let child =
            launch::spawn_detached(fixture.exe(), &fixture.pipe, &fixture.config_dir).unwrap();
        let info = launch::wait_ready(&fixture.config_dir, READY_TIMEOUT).unwrap();
        Self { child, info }
    }
}

impl Drop for Spawned {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// 轮询等进程退出。
fn wait_for_exit(child: &mut Child, timeout: Duration) -> Option<ExitStatus> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match child.try_wait().unwrap() {
            Some(status) => return Some(status),
            None => std::thread::sleep(Duration::from_millis(25)),
        }
    }
    None
}

/// 把 daemon 自己的日志拼进断言消息——CI 上红了要一眼能看出原因。
fn daemon_log(fixture: &Fixture) -> String {
    std::fs::read_to_string(fixture.config_dir.join("logs").join("daemon.log"))
        .unwrap_or_else(|_| "<没有 daemon.log>".to_owned())
}

/// 验收 1 + 3：真进程拉起 → 握手 → ping/pong。
///
/// stdio 由 `launch::spawn_detached` 三个句柄全接到 NUL；能一路应答到 ping/pong，说明 daemon
/// 没有被「stdio 是 NUL」这件事打崩（它运行期不写 stdout，见 `crates/daemon/src/logging.rs`）。
#[test]
fn launches_and_replies_to_ping_pong() {
    let fixture = Fixture::new("ping");
    let daemon = Spawned::start(&fixture);

    assert_eq!(
        daemon.info.pid,
        daemon.child.id(),
        "状态文件应来自我们拉起的那个进程"
    );
    assert_eq!(daemon.info.pipe, fixture.pipe);
    assert_eq!(daemon.info.protocol, ageminal_protocol::PROTOCOL_VERSION);

    let mut client = client::connect(&daemon.info, Duration::from_secs(5)).unwrap();
    for _ in 0..4 {
        client.ping().unwrap();
    }
}

/// 验收 2：重复拉起**复用同一个 daemon**。两层都验：
///
/// 1. 应用侧入口 `ensure_running_with` 认出已有 daemon，**不再拉起新进程**；
/// 2. 就算硬拉起第二个进程，它也应认出并**正常退出**，不动原 daemon。
#[test]
fn reuses_the_same_daemon_on_relaunch() {
    let fixture = Fixture::new("reuse");
    let daemon = Spawned::start(&fixture);

    let reused =
        launch::ensure_running_with(fixture.exe(), &fixture.pipe, &fixture.config_dir).unwrap();
    assert_eq!(reused.pid, daemon.info.pid, "应用侧入口应复用同一个 daemon");
    assert_eq!(reused.token, daemon.info.token);

    let mut second =
        launch::spawn_detached(fixture.exe(), &fixture.pipe, &fixture.config_dir).unwrap();
    assert_ne!(second.id(), daemon.info.pid, "第二次拉起是另一个进程");
    let status = wait_for_exit(&mut second, Duration::from_secs(10));
    let _ = second.kill();
    let status = status.unwrap_or_else(|| {
        panic!(
            "第二个进程没有自行退出；daemon.log:\n{}",
            daemon_log(&fixture)
        )
    });
    assert!(
        status.success(),
        "重复拉起应视为正常结果（退出码 {status}）；daemon.log:\n{}",
        daemon_log(&fixture)
    );

    // 原 daemon 还在跑，状态文件没被改写。
    let info = DaemonInfo::read(&fixture.config_dir).unwrap();
    assert_eq!(info.pid, daemon.info.pid);
    assert_eq!(info.token, daemon.info.token);
    let mut client = client::connect(&info, Duration::from_secs(5)).unwrap();
    client.ping().unwrap();
}

/// 握手负路径：令牌不对要被明确拒绝，且拒绝之后真令牌照样能用。
#[test]
fn rejects_a_mismatched_token() {
    let fixture = Fixture::new("token");
    let daemon = Spawned::start(&fixture);

    let forged = DaemonInfo {
        token: "not-the-token".to_owned(),
        ..daemon.info.clone()
    };
    let error = client::connect(&forged, Duration::from_secs(5)).unwrap_err();
    assert!(
        matches!(error, DaemonError::Rejected { .. }),
        "应被拒：{error:?}"
    );

    let mut client = client::connect(&daemon.info, Duration::from_secs(5)).unwrap();
    client.ping().unwrap();
}
