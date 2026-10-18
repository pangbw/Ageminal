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
use support::{wait_for_exit as wait_for_daemon_exit, Fixture, Spawned};
use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

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

/// #75 验收 1：UI 进程断开后换一条连接，按 `sid` 列会话并 attach 快照，shell 继续跑。
#[test]
fn a_new_connection_lists_and_attaches_a_live_session() {
    let fixture = Fixture::new("reattach");
    let daemon = Spawned::start(&fixture);

    let mut first = client::connect(&daemon.info, Duration::from_secs(5)).unwrap();
    let created = first.create_session(request("reattach")).expect("建会话");
    first
        .write_input(created.handle, b"echo BEFORE-REATTACH\r\n")
        .expect("写");
    let mut collected = Collected::default();
    collected.wait_for(&mut first, created.handle, "BEFORE-REATTACH", TIMEOUT);

    // 断开后让 shell 继续产生输出；标记用 %RANDOM%，回显里只有 `%RANDOM%`，不会误命中。
    first
        .write_input(
            created.handle,
            b"ping -n 3 127.0.0.1 >nul & echo WHILE-AWAY-%RANDOM%\r\n",
        )
        .expect("写");
    drop(first);
    std::thread::sleep(Duration::from_secs(4));

    let mut second = client::connect(&daemon.info, Duration::from_secs(5)).unwrap();
    let sessions = second.list_sessions().expect("listSessions");
    let info = sessions
        .iter()
        .find(|info| info.sid == "reattach")
        .expect("断开后会话仍应在清单里");
    assert_eq!(info.incarnation_id, created.incarnation_id);
    assert_eq!(info.cwd, request("reattach").cwd);

    let attached = second.attach("reattach", 0).expect("attach 快照");
    assert_eq!(attached.from_seq, attached.replay_end);
    let snapshot = attached.snapshot.expect("新连接应拿快照");
    assert!(
        number_after_marker(&snapshot, "WHILE-AWAY-").is_some(),
        "快照里应有断开期间产生的输出：{snapshot:?}"
    );

    second
        .write_input(attached.handle, b"echo LIVE-AFTER\r\n")
        .expect("写");
    collected.wait_for(&mut second, attached.handle, "LIVE-AFTER", TIMEOUT);

    second.terminate("reattach", false).expect("terminate");
}

/// 标记后**紧跟**的数字（用于区分命令回显里的 `%RANDOM%` 与真正运行结果）。
fn number_after_marker(text: &str, marker: &str) -> Option<u32> {
    for (index, _) in text.match_indices(marker) {
        let digits: String = text[index + marker.len()..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        if let Ok(value) = digits.parse() {
            return Some(value);
        }
    }
    None
}

/// #74：历史确实落盘；UI detach 后 daemon 继续写，日志还能从文件读出。
#[test]
fn history_files_keep_growing_after_ui_detaches() {
    let fixture = Fixture::new("history");
    let daemon = Spawned::start(&fixture);
    let mut client = client::connect(&daemon.info, Duration::from_secs(5)).unwrap();
    let created = client.create_session(request("history")).expect("建会话");

    client
        .write_input(created.handle, b"echo HISTORY-BEFORE-DETACH\r\n")
        .expect("写");
    let mut collected = Collected::default();
    collected.wait_for(
        &mut client,
        created.handle,
        "HISTORY-BEFORE-DETACH",
        TIMEOUT,
    );

    let dir = fixture
        .config_dir
        .join("history")
        .join(ageminal_daemon::history::encode_sid("history"));
    let checkpoint_path = dir.join("checkpoint.json");
    let log_path = dir.join("output.log");
    assert!(checkpoint_path.is_file(), "应有 checkpoint.json");
    assert!(log_path.is_file(), "应有 output.log");

    let checkpoint: ageminal_daemon::history::Checkpoint =
        serde_json::from_slice(&std::fs::read(&checkpoint_path).unwrap()).unwrap();
    assert_eq!(checkpoint.sid, "history");
    assert_eq!(checkpoint.seq, 0, "首份 checkpoint 在会话起点");
    let log = std::fs::read(&log_path).unwrap();
    assert_eq!(&log[..4], b"AGMH");
    assert!(
        log.windows(b"HISTORY-BEFORE-DETACH".len())
            .any(|window| window == b"HISTORY-BEFORE-DETACH"),
        "日志应包含 detach 前的输出"
    );
    assert!(log.len() as u64 <= ageminal_daemon::history::CHECKPOINT_MAX);

    // UI detach：daemon 仍要读 PTY、写 VT、写历史。
    client.detach("history").expect("detach");
    let before = log.len() as u64;
    client
        .write_input(created.handle, b"echo HISTORY-AFTER-DETACH\r\n")
        .expect("detach 后仍可输入");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut after = before;
    while Instant::now() < deadline {
        after = std::fs::metadata(&log_path).map(|m| m.len()).unwrap_or(0);
        if after > before {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(after > before, "UI detach 后历史日志必须继续增长");

    let log = std::fs::read(&log_path).unwrap();
    assert!(
        log.windows(b"HISTORY-AFTER-DETACH".len())
            .any(|window| window == b"HISTORY-AFTER-DETACH"),
        "detach 期间输出应落盘，供之后的 Attach 续放"
    );
}

/// #74 验收：UI detach 后产出超过 4 MiB 内存环，`Attach(old_seq)` 仍从落盘日志完整续放。
#[test]
fn long_detached_output_is_replayed_from_disk_after_reattach() {
    let fixture = Fixture::new("history-disk");
    let daemon = Spawned::start(&fixture);
    let mut client = client::connect(&daemon.info, Duration::from_secs(5)).unwrap();
    let created = client
        .create_session(request("history-disk"))
        .expect("建会话");

    // 先 detach，再让 shell 产出 ~5.6 MiB：这段时间只有落盘历史，没有 UI 收流。
    client.detach("history-disk").expect("detach");
    client
        .write_input(
            created.handle,
            b"powershell -NoProfile -Command \"$s='A'*4096; 1..1400 | ForEach-Object { [Console]::Write($s) }; Write-Output ('HISTORY-' + 'END-REAL')\"\r\n",
        )
        .expect("写长输出命令");

    let dir = fixture
        .config_dir
        .join("history")
        .join(ageminal_daemon::history::encode_sid("history-disk"));
    let log_path = dir.join("output.log");
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(metadata) = std::fs::metadata(&log_path) {
            if metadata.len() > 4 * 1024 * 1024 {
                let log = std::fs::read(&log_path).unwrap_or_default();
                if log
                    .windows(b"HISTORY-END-REAL".len())
                    .any(|window| window == b"HISTORY-END-REAL")
                {
                    break;
                }
            }
        }
        assert!(
            Instant::now() < deadline,
            "长输出没在 30 s 内全部落盘；daemon.log:\n{}",
            support::daemon_log(&fixture)
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    // last_seq=1 已滑出 4 MiB 内存环；只有磁盘日志能提供完整回放。
    let attached = client.attach("history-disk", 1).expect("attach");
    assert!(
        attached.snapshot.is_none(),
        "磁盘日志覆盖 last_seq 时不能退化为快照"
    );
    assert_eq!(attached.from_seq, 1);
    let expected = attached.replay_end - attached.from_seq;
    assert!(
        expected > 4 * 1024 * 1024,
        "测试前提是输出超过内存环，实际回放只有 {expected} 字节"
    );

    let mut replay = Vec::with_capacity(expected as usize);
    while (replay.len() as u64) < expected {
        let Some(message) = client
            .next_event(Duration::from_secs(10))
            .expect("next_event")
        else {
            panic!(
                "回放中断：已收 {} / {} 字节；daemon.log:\n{}",
                replay.len(),
                expected,
                support::daemon_log(&fixture)
            );
        };
        if let Message::Data { bytes, .. } = message {
            let remaining = expected as usize - replay.len();
            replay.extend_from_slice(&bytes[..bytes.len().min(remaining)]);
        }
    }
    assert_eq!(replay.len() as u64, expected);
    assert!(
        replay
            .windows(b"HISTORY-END-REAL".len())
            .any(|window| window == b"HISTORY-END-REAL"),
        "从磁盘续放的内容应包含 detach 期间的结尾标记"
    );
}

/// #76：托盘退出走 daemon 的 `Shutdown` —— 所有会话被终止，daemon 自己退出并删发现文件。
#[test]
fn shutdown_terminates_all_sessions_and_stops_daemon() {
    let fixture = Fixture::new("shutdown");
    let mut daemon = Spawned::start(&fixture);
    let mut client = client::connect(&daemon.info, Duration::from_secs(5)).unwrap();
    let created = client.create_session(request("shutdown")).expect("建会话");

    client
        .write_input(created.handle, b"echo SHUTDOWN-READY\\r\\n")
        .expect("写");
    let mut collected = Collected::default();
    collected.wait_for(&mut client, created.handle, "SHUTDOWN-READY", TIMEOUT);
    assert!(process_alive(created.pid), "会话进程应该还活着");

    client.shutdown().expect("优雅停机");
    // daemon 在 ack 后等这条连接关闭再退出。
    drop(client);

    let status =
        wait_for_daemon_exit(&mut daemon.child, Duration::from_secs(10)).unwrap_or_else(|| {
            panic!(
                "daemon 没有在 Shutdown 后退出；daemon.log:\n{}",
                support::daemon_log(&fixture)
            )
        });
    assert!(status.success(), "daemon 应正常退出，实际 {status}");
    assert!(
        wait_until_dead(created.pid, Duration::from_secs(5)),
        "daemon 停机后 shell 进程（pid {}）仍然活着",
        created.pid
    );
    assert!(
        ageminal_protocol::DaemonInfo::read(&fixture.config_dir).is_err(),
        "daemon 退出时应删除 daemon.json"
    );
}

/// 轮询等某个 pid 消失 / 退出。
fn wait_until_dead(pid: u32, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !process_alive(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    !process_alive(pid)
}

fn process_alive(pid: u32) -> bool {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return false;
    }
    let mut code = 0;
    let ok = unsafe { GetExitCodeProcess(handle, &mut code) };
    unsafe { CloseHandle(handle) };
    ok != 0 && code == STILL_ACTIVE as u32
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
