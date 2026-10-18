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
    /// 每个句柄收到的**字节数**（`ack` 记的是字节，不是字符）。
    bytes: HashMap<u32, u64>,
    /// 各会话的退出事件。**必须记下来**：两个会话同时终止时，先到的退出很可能不是你在等的那条。
    exits: HashMap<String, SessionExited>,
}

impl Collected {
    fn absorb(&mut self, message: Message) {
        match message {
            Message::Data { handle, bytes, .. } => {
                *self.bytes.entry(handle).or_default() += bytes.len() as u64;
                self.text
                    .entry(handle)
                    .or_default()
                    .push_str(&String::from_utf8_lossy(&bytes));
            }
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
            if let Some(message) = client
                .next_event(Duration::from_millis(100))
                .expect("next_event")
            {
                self.absorb(message);
            }
        }
    }

    fn get(&self, handle: u32) -> String {
        self.text.get(&handle).cloned().unwrap_or_default()
    }

    fn bytes(&self, handle: u32) -> u64 {
        self.bytes.get(&handle).copied().unwrap_or_default()
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
        if let Some(exited) = collected.exits.remove(sid) {
            return exited;
        }
        if Instant::now() >= deadline {
            panic!(
                "没等到 {sid} 的 sessionExited；已收到的退出：{:?}",
                collected.exits.keys().collect::<Vec<_>>()
            );
        }
        if let Some(message) = client
            .next_event(Duration::from_millis(100))
            .expect("next_event")
        {
            collected.absorb(message);
        }
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
        .ack("s1", collected.bytes(created.handle))
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

    // 把 BEFORE 提示符之后的残余输出收干净，避免「detach 后无数据」被旧帧误判。
    while let Some(message) = client
        .next_event(Duration::from_millis(50))
        .expect("next_event")
    {
        collected.absorb(message);
    }

    // 断开：不回退出事件，会话还在。
    let last = collected.bytes(created.handle);
    client.detach("d").expect("detach 是 fire-and-forget");
    client
        .write_input(created.handle, b"echo WHILE-AWAY\r\n")
        .expect("写");
    std::thread::sleep(Duration::from_millis(500));

    // #73：不可见时 daemon 不推流（输出只留在 VT / 环形缓冲里）。
    let hidden = client
        .next_event(Duration::from_millis(300))
        .expect("next_event");
    assert!(hidden.is_none(), "detach 后不应收到数据：{hidden:?}");

    // #72：从断开前的 last_seq 精确回放，回放里应包含断开期间产生的输出。
    let attached = client.attach("d", last).expect("attach");
    assert_eq!(attached.sid, "d");
    assert_eq!(attached.from_seq, last, "窗口内应从 lastSeq 之后回放");
    assert!(
        attached.snapshot.is_none(),
        "窗口覆盖 lastSeq 时不该退化为快照"
    );
    assert!(attached.replay_end >= last);

    let replayed = collected.wait_for(&mut client, attached.handle, "WHILE-AWAY", TIMEOUT);
    assert!(replayed.contains("WHILE-AWAY"), "断开期间输出必须被回放");

    client
        .write_input(attached.handle, b"echo AFTER\r\n")
        .expect("写");
    collected.wait_for(&mut client, attached.handle, "AFTER", TIMEOUT);

    // lastSeq = 0：新 UI 没有可信屏幕，必须拿快照。
    let snapshot = client.attach("d", 0).expect("attach with snapshot");
    assert!(snapshot.snapshot.is_some(), "lastSeq=0 必须回快照");
    assert_eq!(snapshot.from_seq, snapshot.replay_end);
    assert_eq!(snapshot.sid, "d");

    client.terminate("d", false).expect("terminate");
    wait_for_exit(&mut client, &mut collected, "d", TIMEOUT);
}

/// 按控制字符切出真正的输出 token：ConPTY / cmd 会插入 OSC 标题（`\x1b]0;…\x07`），
/// 且标题之后不保证有换行——不能只看 `lines()`。
fn flow_tokens(text: &str) -> impl Iterator<Item = &str> {
    text.split(['\r', '\n', '\x1b', '\x07'])
        .filter(|token| !token.is_empty())
}

/// 输出结束标记：`FLOW-<随机数>-DONE`；命令回显里的 `%RANDOM%` 不会误匹配。
fn flow_done(text: &str) -> bool {
    flow_tokens(text).any(|token| {
        token
            .strip_prefix("FLOW-")
            .and_then(|rest| rest.strip_suffix("-DONE"))
            .is_some_and(|number| {
                !number.is_empty() && number.chars().all(|character| character.is_ascii_digit())
            })
    })
}

/// #73 验收 1：高吞吐下不 ack 时输出在高水位停住；ack 后继续，且一行不丢。
#[test]
fn flow_control_pauses_at_high_water_and_resumes_without_loss() {
    const LINES: usize = 20_000;
    const HIGH: u64 = 256 * 1024;

    let fixture = Fixture::new("flow");
    let daemon = Spawned::start(&fixture);
    let mut client = client::connect(&daemon.info, Duration::from_secs(5)).unwrap();
    let created = client.create_session(request("flow")).expect("建会话");

    // 20k 行 × 43B ≈ 860 KiB，足够跨过 256 KiB 高水位多次。
    let command = format!(
        "for /L %i in (1,1,{LINES}) do @echo FLOW-%i-012345678901234567890123456789\r\necho FLOW-%RANDOM%-DONE\r\n"
    );
    client
        .write_input(created.handle, command.as_bytes())
        .expect("写");

    let mut text = String::new();
    let mut bytes = 0u64;
    let mut acked = 0u64;
    let mut paused_at = None;
    let deadline = Instant::now() + TIMEOUT;

    loop {
        if Instant::now() >= deadline {
            let tail: String = text
                .chars()
                .rev()
                .take(400)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            panic!("流控测试超时：收到 {bytes} 字节，paused_at={paused_at:?}，尾部={tail:?}");
        }
        match client
            .next_event(Duration::from_millis(100))
            .expect("next_event")
        {
            Some(Message::Data { bytes: chunk, .. }) => {
                bytes += chunk.len() as u64;
                text.push_str(&String::from_utf8_lossy(&chunk));
            }
            Some(Message::Error(error)) => panic!("daemon 报错：{error:?}"),
            Some(_) => {}
            None => {
                if flow_done(&text) {
                    break;
                }
                if bytes < HIGH {
                    // 命令可能还没开始输出；继续等。
                    continue;
                }
                if paused_at.is_none() {
                    // 第一次到达高水位：不 ack，确认输出真的停住（而不是恰好管道空了一下）。
                    let before = bytes;
                    let probe_deadline = Instant::now() + Duration::from_millis(300);
                    while Instant::now() < probe_deadline {
                        match client
                            .next_event(Duration::from_millis(50))
                            .expect("next_event")
                        {
                            Some(Message::Data { bytes: chunk, .. }) => {
                                bytes += chunk.len() as u64;
                                text.push_str(&String::from_utf8_lossy(&chunk));
                                panic!(
                                    "高水位后仍继续推流（背压未生效）：{} → {} 字节",
                                    before, bytes
                                );
                            }
                            Some(_) => {}
                            None => {}
                        }
                    }
                    assert!(
                        !flow_done(&text),
                        "输出已结束却没触发高水位暂停（命令太小或流控没生效）"
                    );
                    paused_at = Some(bytes);
                }
                if bytes > acked {
                    client.ack("flow", bytes).expect("ack");
                    acked = bytes;
                }
            }
        }
    }

    let paused_at = paused_at.expect("输出应触发高水位暂停");
    assert!(paused_at < bytes, "ack 之后应继续收到剩余输出");
    assert!(bytes > HIGH);

    let mut seen = vec![false; LINES + 1];
    for token in flow_tokens(&text) {
        let Some(rest) = token.strip_prefix("FLOW-") else {
            continue;
        };
        let Some((number, tail)) = rest.split_once('-') else {
            continue;
        };
        if tail != "012345678901234567890123456789" {
            continue;
        }
        if let Ok(number) = number.parse::<usize>() {
            if number <= LINES {
                seen[number] = true;
            }
        }
    }
    let missing: Vec<_> = (1..=LINES)
        .filter(|number| !seen[*number])
        .take(8)
        .collect();
    for number in &missing {
        let needle = format!("FLOW-{number}-");
        match text.find(&needle) {
            Some(position) => {
                let start = position.saturating_sub(40);
                let end = (position + needle.len() + 120).min(text.len());
                eprintln!(
                    "缺失 {number} 的上下文：{:?}",
                    text.get(start..end).unwrap_or("<非 UTF-8 边界>")
                );
            }
            None => eprintln!("缺失 {number}：原始文本里找不到 {needle:?}"),
        }
    }
    assert!(missing.is_empty(), "流控丢字，缺失行：{missing:?}");
    assert!(flow_done(&text), "没收到结束标记");
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
    let error = match client
        .next_event(Duration::from_secs(10))
        .expect("next_event")
    {
        Some(Message::Error(error)) => error,
        other => panic!("应当回一条 Error，收到 {other:?}"),
    };
    assert!(
        error.message.contains("nope"),
        "错误信息要指出是哪个会话：{error:?}"
    );

    // 连接还活着。
    client.ping().expect("连接应保持可用");
}
