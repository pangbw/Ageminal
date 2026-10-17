//! 会话管理接线的**真进程**集成测试（issue #67）。
//!
//! 验收：create → 执行命令拿到字节 → ack → terminate；多会话输出不串线；迟到 exit 按
//! `incarnationId` 丢弃（机制的单测在 `src/session.rs`，这里验可观测的那半）。

#![cfg(windows)]

mod support;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use ageminal_daemon::client;
use ageminal_protocol::{CreateSession, Message, SessionExited};
use support::{Fixture, Spawned};

const TIMEOUT: Duration = Duration::from_secs(30);

/// 一个用 `cmd.exe` 的建会话请求。
fn request(sid: &str) -> CreateSession {
    CreateSession {
        sid: sid.to_owned(),
        cwd: std::env::temp_dir().to_string_lossy().into_owned(),
        shell: Some("cmd.exe".to_owned()),
        command: None,
        cols: 80,
        rows: 24,
        env: Vec::new(),
    }
}

/// 按会话句柄分别攒输出。
///
/// 必须**跨断言累积**：两个会话是交错出数据的，只挑自己关心的那条会把别人的帧吃掉。
#[derive(Default)]
struct Collected {
    text: HashMap<u32, String>,
    /// 各会话的退出事件。**必须记下来**：两个会话同时终止时，先到的退出很可能不是你在等的那条。
    exits: HashMap<String, SessionExited>,
}

impl Collected {
    fn absorb(&mut self, message: Message) {
        match message {
            Message::Data { handle, bytes, .. } => self
                .text
                .entry(handle)
                .or_default()
                .push_str(&String::from_utf8_lossy(&bytes)),
            Message::SessionExited(exited) => {
                self.exits.insert(exited.sid.clone(), exited);
            }
            Message::Error(error) => panic!("daemon 报错：{error:?}"),
            _ => {}
        }
    }

    /// 等到某个句柄的输出里出现 `needle`，返回它迄今为止的全部输出。
    fn wait_for(
        &mut self,
        client: &mut client::Client,
        handle: u32,
        needle: &str,
        timeout: Duration,
    ) -> String {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(text) = self.text.get(&handle) {
                if text.contains(needle) {
                    return text.clone();
                }
            }
            if Instant::now() >= deadline {
                panic!(
                    "没等到 {needle:?}；句柄 {handle} 收到：{:?}",
                    self.text.get(&handle)
                );
            }
            self.absorb(client.next_event().expect("next_event"));
        }
    }

    fn get(&self, handle: u32) -> String {
        self.text.get(&handle).cloned().unwrap_or_default()
    }
}

/// 等某个会话的退出事件（先看已经攒下的，再继续收）。
fn wait_for_exit(
    client: &mut client::Client,
    collected: &mut Collected,
    sid: &str,
    timeout: Duration,
) -> SessionExited {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(exited) = collected.exits.get(sid) {
            return exited.clone();
        }
        if Instant::now() >= deadline {
            panic!(
                "没等到 {sid} 的 sessionExited；已收到的退出：{:?}",
                collected.exits.keys().collect::<Vec<_>>()
            );
        }
        collected.absorb(client.next_event().expect("next_event"));
    }
}

/// 验收 1：create → 执行命令拿到字节 → ack → terminate 成功。
#[test]
fn create_write_ack_and_terminate() {
    let fixture = Fixture::new("session");
    let daemon = Spawned::start(&fixture);
    let mut client = client::connect(&daemon.info, Duration::from_secs(5)).unwrap();

    // 用一个专属 cwd：`%CD%` 的结果里才有这个名字，回显里只有 `echo %CD%` 这串字面量。
    let cwd = std::env::temp_dir().join(format!("ageminal-session-cwd-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cwd);
    std::fs::create_dir_all(&cwd).unwrap();
    // 放一个名字独特的文件：`dir /b` 的**输出**里才有它，`dir /b` 这串字面量里没有——
    // 这样「拿到字节」证明的是真的执行了，而不是把输入回显当输出。
    let probe = format!("ageminal-probe-{}.txt", std::process::id());
    std::fs::write(cwd.join(&probe), b"").unwrap();

    let mut create = request("s1");
    create.cwd = cwd.to_string_lossy().into_owned();

    let created = client.create_session(create).expect("建会话");
    assert_eq!(created.sid, "s1");
    assert_ne!(created.handle, 0, "要有连接内句柄");
    assert_ne!(created.pid, 0, "要有子进程 pid");
    assert!(!created.incarnation_id.is_empty(), "要有世代 id");

    // 写输入 → 收到字节（证明 PTY 通了、且 cwd 生效）。
    client
        .write_input(created.handle, b"dir /b\r\n")
        .expect("写输入");
    let mut collected = Collected::default();
    collected.wait_for(&mut client, created.handle, &probe, TIMEOUT);

    // ack（fire-and-forget；如果 daemon 不认，会回一条 Error，被 `absorb` 抓成 panic）。
    client
        .ack("s1", collected.get(created.handle).len() as u64)
        .expect("ack");

    // terminate → 等到同世代的退出事件。
    client.terminate("s1", false).expect("terminate");
    let exited = wait_for_exit(&mut client, &mut collected, "s1", TIMEOUT);
    assert_eq!(exited.incarnation_id, created.incarnation_id);
    assert!(
        exited.exit_code.is_some(),
        "被终止的进程也该有退出码：{exited:?}"
    );

    let _ = std::fs::remove_dir_all(&cwd);
}

/// 验收 2：多会话输出不串线。
#[test]
fn two_sessions_do_not_cross() {
    let fixture = Fixture::new("cross");
    let daemon = Spawned::start(&fixture);
    let mut client = client::connect(&daemon.info, Duration::from_secs(5)).unwrap();

    let first = client.create_session(request("a")).expect("建 a");
    let second = client.create_session(request("b")).expect("建 b");
    assert_ne!(first.handle, second.handle, "两会话要有不同的句柄");

    client
        .write_input(first.handle, b"echo ALPHA\r\n")
        .expect("写 a");
    client
        .write_input(second.handle, b"echo BETA\r\n")
        .expect("写 b");

    let mut collected = Collected::default();
    collected.wait_for(&mut client, first.handle, "ALPHA", TIMEOUT);
    collected.wait_for(&mut client, second.handle, "BETA", TIMEOUT);

    let alpha = collected.get(first.handle);
    let beta = collected.get(second.handle);
    assert!(!alpha.contains("BETA"), "a 收到了 b 的输出：{alpha}");
    assert!(!beta.contains("ALPHA"), "b 收到了 a 的输出：{beta}");

    for sid in ["a", "b"] {
        client.terminate(sid, false).expect("terminate");
    }
    wait_for_exit(&mut client, &mut collected, "a", TIMEOUT);
    wait_for_exit(&mut client, &mut collected, "b", TIMEOUT);
}

/// 验收 3 的可观测部分：同名会话重建后，世代 id 必须换；退出事件按各自的世代归属。
///
/// 「旧世代的迟到 exit 被丢掉」这条竞态**没法在集成测试里稳定复现**（窗口只有几十微秒），它的
/// 机制由 `session.rs` 里 `a_late_exit_from_an_old_incarnation_is_dropped` 这个单测直接盯着。
#[test]
fn recreated_session_gets_a_new_incarnation() {
    let fixture = Fixture::new("incarnation");
    let daemon = Spawned::start(&fixture);
    let mut client = client::connect(&daemon.info, Duration::from_secs(5)).unwrap();
    let mut collected = Collected::default();

    let first = client.create_session(request("same")).expect("第一次建");
    client.terminate("same", false).expect("terminate");
    let first_exit = wait_for_exit(&mut client, &mut collected, "same", TIMEOUT);
    assert_eq!(first_exit.incarnation_id, first.incarnation_id);

    let second = client
        .create_session(request("same"))
        .expect("重建同名会话");
    assert_ne!(
        second.incarnation_id, first.incarnation_id,
        "重建必须换世代 id，否则迟到 exit 就没法区分"
    );

    client.terminate("same", false).expect("terminate");
    let second_exit = wait_for_exit(&mut client, &mut collected, "same", TIMEOUT);
    assert_eq!(second_exit.incarnation_id, second.incarnation_id);
}

/// detach ≠ terminate：断开后会话继续活着（只是不再往这条连接推输出），attach 回来还能接着用。
#[test]
fn detach_keeps_the_session_alive() {
    let fixture = Fixture::new("detach");
    let daemon = Spawned::start(&fixture);
    let mut client = client::connect(&daemon.info, Duration::from_secs(5)).unwrap();
    let mut collected = Collected::default();

    let created = client.create_session(request("d")).expect("建会话");
    client
        .write_input(created.handle, b"echo BEFORE\r\n")
        .expect("写");
    collected.wait_for(&mut client, created.handle, "BEFORE", TIMEOUT);

    // 断开：不回退出事件，会话还在。
    client.detach("d").expect("detach 是 fire-and-forget");
    client
        .write_input(created.handle, b"echo WHILE-AWAY\r\n")
        .expect("写");
    std::thread::sleep(Duration::from_millis(500));

    // 重新接上：句柄可能变（这里同一连接内复用），从当前进度接实时流。
    let attached = client.attach("d", 0).expect("attach");
    assert_eq!(attached.sid, "d");
    assert!(attached.from_seq > 0, "应该报告从哪儿接上");

    client
        .write_input(attached.handle, b"echo AFTER\r\n")
        .expect("写");
    collected.wait_for(&mut client, attached.handle, "AFTER", TIMEOUT);
    assert!(
        !collected.get(attached.handle).contains("WHILE-AWAY"),
        "断开期间的输出本票是丢弃的（回放归 #72）"
    );

    client.terminate("d", false).expect("terminate");
    wait_for_exit(&mut client, &mut collected, "d", TIMEOUT);
}

/// 不存在的会话：daemon 回一条 `Error`，**连接不断**（回完还能继续用）。
#[test]
fn unknown_session_is_an_error_not_a_disconnect() {
    let fixture = Fixture::new("unknown");
    let daemon = Spawned::start(&fixture);
    let mut client = client::connect(&daemon.info, Duration::from_secs(5)).unwrap();

    client
        .terminate("nope", false)
        .expect("terminate 本身只是发送");
    let error = match client.next_event().expect("next_event") {
        Message::Error(error) => error,
        other => panic!("应当回一条 Error，收到 {other:?}"),
    };
    assert!(
        error.message.contains("nope"),
        "错误信息要指出是哪个会话：{error:?}"
    );

    // 连接还活着。
    client.ping().expect("连接应保持可用");
}
