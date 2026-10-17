//! 集成测试的共用夹具：**每个用例一个隔离的 daemon**。
//!
//! 被 `tests/lifecycle.rs` 与 `tests/sessions.rs` 各自 `mod support;` 进一份，所以这里允许有
//! 「另一个测试文件才会用到」的项。

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use ageminal_daemon::{launch, DaemonInfo};

/// `cargo test` 编出来的 daemon 可执行文件。
pub const DAEMON_EXE: &str = env!("CARGO_BIN_EXE_ageminal-daemon");

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// 每个用例专属的夹具：配置根与管道名都**不与用户机器上真在跑的 daemon 冲突**。
pub struct Fixture {
    pub config_dir: PathBuf,
    pub pipe: String,
}

impl Fixture {
    /// 建一个隔离的夹具。
    pub fn new(tag: &str) -> Self {
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

    /// daemon 可执行文件。
    pub fn exe(&self) -> &Path {
        Path::new(DAEMON_EXE)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.config_dir);
    }
}

/// 拉起的 daemon：握过手的发现信息 + 进程句柄。**析构一定杀掉**，坏了也不留孤儿进程。
pub struct Spawned {
    pub child: Child,
    pub info: DaemonInfo,
}

impl Spawned {
    /// 拉起并等到握手信息就绪。
    pub fn start(fixture: &Fixture) -> Self {
        let child =
            launch::spawn_detached(fixture.exe(), &fixture.pipe, &fixture.config_dir).unwrap();
        // 失败时把 daemon 自己的日志一起打出来——脱离 stdout 的进程，日志是唯一线索。
        let info = match launch::wait_ready(&fixture.config_dir, launch::READY_TIMEOUT) {
            Ok(info) => info,
            Err(error) => panic!(
                "daemon 没就绪：{error}\n--- daemon.log ---\n{}",
                daemon_log(fixture)
            ),
        };
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
pub fn wait_for_exit(child: &mut Child, timeout: Duration) -> Option<ExitStatus> {
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
pub fn daemon_log(fixture: &Fixture) -> String {
    std::fs::read_to_string(fixture.config_dir.join("logs").join("daemon.log"))
        .unwrap_or_else(|_| "<没有 daemon.log>".to_owned())
}
