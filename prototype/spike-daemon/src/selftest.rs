//! 自测：一命令跑完「冒烟 → 存活/重放 → 吞吐 → 延迟 → CSI 6n」，产出 JSON 报告。
//!
//! 两条设计原则（都是被上一版坑出来的）：
//! 1. **失败要快**：先做冒烟；冒烟不过就立刻收尾出报告，不要在几十个超时里静默卡几分钟。
//! 2. **要看得见**：每个阶段打一行进度；daemon 侧关键节点写文件日志，出错时一并带回来。

use crate::client::{attach, await_marker, drain_ms, status_json, Attached, Scan};
use crate::logbuf;
use crate::marker::marker_cmd;
use crate::proto::*;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

pub const BUILD: &str = "#40 H7 · 2026-09-20";

const PHASES: u32 = 8;

#[derive(Serialize, Default)]
pub struct Hello {
    pub proto: u32,
    pub caps: u32,
    pub shell_pid: u32,
    pub alive: bool,
}

#[derive(Serialize, Default)]
pub struct Smoke {
    pub pass: bool,
    pub first_output_ms: u64,
}

#[derive(Serialize, Default)]
pub struct Survival {
    pub detached_ms: u64,
    pub tick1_before_detach: bool,
    pub ticks_in_replay: u32,
    pub tick2_seen_after_reattach: bool,
    pub shell_alive_after_detach: bool,
    pub replay_bytes: u64,
    pub pass: bool,
}

#[derive(Serialize, Default)]
pub struct Throughput {
    pub bytes: u64,
    pub frames: u64,
    pub ms: u64,
    pub kib_per_sec: f64,
}

#[derive(Serialize, Default)]
pub struct Latency {
    pub samples: u32,
    pub p50_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
}

#[derive(Serialize, Default)]
pub struct Csi6n {
    pub probe_ran: bool,
    pub responder_enabled: bool,
    pub requests_seen: u64,
    pub replies_sent: u64,
}

#[derive(Serialize, Default)]
pub struct Report {
    pub build: String,
    pub pipe: String,
    pub shell: String,
    pub ring_cap_bytes: usize,
    pub responder: bool,
    pub daemon_pid: u32,
    pub daemon_log_path: String,
    pub hello: Hello,
    pub smoke: Smoke,
    pub survival: Survival,
    pub throughput: Throughput,
    pub latency_echo: Latency,
    pub latency_ping: Latency,
    pub csi6n: Csi6n,
    pub status_final: Option<serde_json::Value>,
    pub daemon_log: Vec<String>,
    pub errors: Vec<String>,
    pub notes: Vec<String>,
}

fn phase(n: u32, msg: &str) {
    println!("[{n}/{PHASES}] {msg} …");
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// 只数「TICK-<数字>」（回显的命令行里是字面 `TICK-%i`，不能算）
fn count_ticks(s: &str) -> u32 {
    let b = s.as_bytes();
    let pat = b"TICK-";
    let mut n = 0u32;
    let mut i = 0usize;
    while i + pat.len() < b.len() {
        if &b[i..i + pat.len()] == pat && b[i + pat.len()].is_ascii_digit() {
            n += 1;
        }
        i += 1;
    }
    n
}

/// 输入是**微秒**，输出毫秒（IPC 往返常在 1ms 以下，用毫秒收集会全变 0）
fn summarize_us(v: Vec<u64>) -> Latency {
    if v.is_empty() {
        return Latency::default();
    }
    let mut s = v.clone();
    s.sort_unstable();
    let pick = |p: f64| {
        let idx = ((s.len() as f64 - 1.0) * p).round() as usize;
        s[idx] as f64 / 1000.0
    };
    Latency {
        samples: v.len() as u32,
        p50_ms: pick(0.50),
        p99_ms: pick(0.99),
        max_ms: *s.last().unwrap() as f64 / 1000.0,
    }
}

async fn await_hello(a: &mut Attached, timeout_ms: u64) -> Result<Hello, String> {
    let t0 = std::time::Instant::now();
    while t0.elapsed().as_millis() as u64 <= timeout_ms {
        match tokio::time::timeout(Duration::from_millis(100), a.rx.recv()).await {
            Ok(Some((KIND_HELLO_ACK, p))) => {
                if p.len() < 24 {
                    return Err("HELLO_ACK 长度异常".into());
                }
                return Ok(Hello {
                    proto: u32s(&p[0..4]),
                    caps: u32s(&p[4..8]),
                    shell_pid: u32s(&p[16..20]),
                    alive: u32s(&p[20..24]) != 0,
                });
            }
            Ok(Some(_)) => {}
            Ok(None) => return Err("连接已关闭".into()),
            Err(_) => {}
        }
    }
    Err("HELLO_ACK 超时".into())
}

pub async fn run(
    pipe: String,
    json_path: PathBuf,
    keep: bool,
    responder: bool,
    ring_bytes: usize,
    shell: Option<String>,
) -> i32 {
    let log_path = std::env::temp_dir().join(format!("agm-daemon-{}.log", std::process::id()));
    let _ = std::fs::remove_file(&log_path);

    let mut rep = Report {
        build: BUILD.into(),
        pipe: pipe.clone(),
        shell: shell.clone().unwrap_or_else(|| "(COMSPEC)".into()),
        ring_cap_bytes: ring_bytes,
        responder,
        daemon_log_path: log_path.display().to_string(),
        ..Default::default()
    };

    // ── 1) 拉起 daemon（detached，stdio 全脱开，关键节点写日志文件）──
    phase(1, "拉起 daemon（DETACHED_PROCESS）");
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            rep.errors.push(format!("取 current_exe 失败：{e}"));
            return finish(rep, json_path, &log_path);
        }
    };
    let mut cmd = Command::new(&exe);
    cmd.arg("daemon")
        .arg("--pipe")
        .arg(&pipe)
        .arg("--ring-bytes")
        .arg(ring_bytes.to_string())
        .arg("--log")
        .arg(&log_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if responder {
        cmd.arg("--csi6n-reply");
    }
    if let Some(sh) = &shell {
        cmd.arg("--shell").arg(sh);
    }
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    match cmd.spawn() {
        Ok(c) => {
            rep.daemon_pid = c.id();
            println!("      daemon pid={}", c.id());
        }
        Err(e) => {
            rep.errors.push(format!("拉起 daemon 失败：{e}"));
            return finish(rep, json_path, &log_path);
        }
    }

    // ── 2) attach ──
    phase(2, "attach + HELLO");
    let mut a = match attach(&pipe, 8000).await {
        Ok(a) => a,
        Err(e) => {
            rep.errors.push(format!("attach 失败：{e}"));
            return finish(rep, json_path, &log_path);
        }
    };
    match await_hello(&mut a, 5000).await {
        Ok(h) => {
            println!(
                "      proto={} caps={:#x} shell_pid={} alive={}",
                h.proto, h.caps, h.shell_pid, h.alive
            );
            rep.hello = h;
        }
        Err(e) => rep.errors.push(format!("hello: {e}")),
    }

    let mut scan = Scan::new();
    drain_ms(&mut a, &mut scan, 400).await;

    // ── 3) 冒烟：ConPTY 到底有没有在产出？不过就立刻收尾 ──
    phase(3, "冒烟：shell 是否真的响应");
    let (smoke_cmd, smoke_marker) = marker_cmd(now_ms() as u64);
    let t0 = std::time::Instant::now();
    if let Err(e) = a.input(&format!("{smoke_cmd}\r")).await {
        rep.errors.push(format!("冒烟：发送失败 {e}"));
    }
    match await_marker(&mut a, &mut scan, smoke_marker.as_bytes(), 6000).await {
        Ok(_) => {
            rep.smoke.pass = true;
            rep.smoke.first_output_ms = t0.elapsed().as_millis() as u64;
            println!("      通过（{} ms）", rep.smoke.first_output_ms);
        }
        Err(e) => {
            rep.errors.push(format!(
                "冒烟失败：ConPTY 没有产出任何可识别的输出（{e}）。若同时看到**一个新的 cmd 窗口**，\
                 说明伪控制台属性没生效——子进程被 Windows 另开控制台托管了。"
            ));
            // 关键判据：子进程的退出码（0xC0000142 = 未能接到伪控制台）
            if let Ok(st) = status_json(&mut a, 3000).await {
                println!("      daemon 状态：{st}");
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&st) {
                    rep.status_final = Some(v);
                }
            }
            rep.daemon_log = logbuf::tail(&log_path, 40);
            let _ = a.shutdown().await;
            return finish(rep, json_path, &log_path);
        }
    }

    // ── 4) 存活 + 重放（本票核心）──
    phase(4, "存活 + 重放（断开 5s 后重连）");
    rep.notes.push(
        "存活实验：先让 shell 持续产出，断开连接，5s 后重连，看回放里有没有断开期间的输出".into(),
    );
    if let Err(e) = a
        .input("for /l %i in (1,1,12) do @(echo TICK-%i & ping -n 2 127.0.0.1 >nul)\r")
        .await
    {
        rep.errors.push(format!("发送 TICK 命令失败：{e}"));
    }
    match await_marker(&mut a, &mut scan, b"TICK-1", 6000).await {
        Ok(_) => rep.survival.tick1_before_detach = true,
        Err(e) => rep.errors.push(format!("未看到 TICK-1：{e}")),
    }
    let _ = a.detach().await;
    drop(a); // 断开连接（等价于「UI 关闭」）
    tokio::time::sleep(Duration::from_millis(5000)).await;
    rep.survival.detached_ms = 5000;

    let mut b = match attach(&pipe, 8000).await {
        Ok(b) => b,
        Err(e) => {
            rep.errors.push(format!("重连失败：{e}"));
            rep.daemon_log = logbuf::tail(&log_path, 40);
            return finish(rep, json_path, &log_path);
        }
    };
    let _ = await_hello(&mut b, 5000).await;
    let t0 = std::time::Instant::now();
    let mut replay = String::new();
    while t0.elapsed().as_millis() < 2000 {
        match tokio::time::timeout(Duration::from_millis(100), b.rx.recv()).await {
            Ok(Some((KIND_REPLAY, p))) | Ok(Some((KIND_DATA, p))) => {
                replay.push_str(&String::from_utf8_lossy(&p));
            }
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(_) => {}
        }
    }
    rep.survival.replay_bytes = replay.len() as u64;
    rep.survival.ticks_in_replay = count_ticks(&replay);
    rep.survival.tick2_seen_after_reattach = replay.contains("TICK-2");
    if let Ok(s) = status_json(&mut b, 3000).await {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
            rep.survival.shell_alive_after_detach =
                v.get("alive").and_then(|x| x.as_bool()).unwrap_or(false);
        }
    }
    rep.survival.pass =
        rep.survival.tick2_seen_after_reattach && rep.survival.shell_alive_after_detach;
    println!(
        "      pass={} 回放 {} 字节 / {} 个 TICK / shell alive={}",
        rep.survival.pass,
        rep.survival.replay_bytes,
        rep.survival.ticks_in_replay,
        rep.survival.shell_alive_after_detach
    );

    // ── 5) 吞吐 ──
    phase(5, "吞吐（4000 行）");
    rep.notes
        .push("吞吐：cmd 的 for 循环打印 4000 行定长文本，统计到 DONE 标记为止的字节/时间（含 shell 与 ConPTY 开销）".into());
    let (thru_cmd, thru_marker) = marker_cmd(now_ms() as u64);
    let mut tscan = Scan::new();
    let (b0, f0, _, _, _, _) = b.stats.snapshot();
    let t_thru = std::time::Instant::now();
    let cmd = format!(
        "for /l %i in (1,1,4000) do @echo 0123456789abcdef0123456789abcdef0123456789abcdef\r\n{thru_cmd}\r"
    );
    let _ = b.input(&cmd).await;
    match await_marker(&mut b, &mut tscan, thru_marker.as_bytes(), 20_000).await {
        Ok(_) => {
            let ms = t_thru.elapsed().as_millis() as u64;
            let (b1, f1, _, _, _, _) = b.stats.snapshot();
            rep.throughput = Throughput {
                bytes: b1 - b0,
                frames: f1 - f0,
                ms,
                kib_per_sec: if ms > 0 {
                    (b1 - b0) as f64 / ms as f64
                } else {
                    0.0
                },
            };
            println!(
                "      {} 字节 / {} 帧 / {} ms = {:.0} KiB/s",
                rep.throughput.bytes,
                rep.throughput.frames,
                rep.throughput.ms,
                rep.throughput.kib_per_sec
            );
        }
        Err(e) => rep.errors.push(format!("吞吐测量失败：{e}")),
    }
    drain_ms(&mut b, &mut tscan, 300).await;

    // ── 6) echo 往返延迟 ──
    phase(6, "echo 往返延迟（20 次）");
    rep.notes.push(
        "echo 延迟：标记形如 L-<i>#，只匹配行首（\\r\\n 前缀）——ConPTY 会回显输入，不这样区分会命中回显、量到 0ms"
            .into(),
    );
    let mut echo_us = Vec::new();
    let mut fails = 0;
    for i in 0..20u32 {
        let (cmdline, marker) = marker_cmd(now_ms() as u64 ^ (i as u64) << 20);
        if b.input(&format!("{cmdline}\r")).await.is_err() {
            break;
        }
        let t = std::time::Instant::now();
        match await_marker(&mut b, &mut tscan, marker.as_bytes(), 2000).await {
            Ok(_) => {
                echo_us.push(t.elapsed().as_micros() as u64);
                fails = 0;
            }
            Err(e) => {
                fails += 1;
                rep.errors.push(format!("echo 延迟第 {i} 次失败：{e}"));
                if fails >= 3 {
                    rep.errors
                        .push("echo 延迟连续失败 3 次，提前结束该阶段".into());
                    break;
                }
            }
        }
    }
    rep.latency_echo = summarize_us(echo_us);
    println!(
        "      n={} p50={}ms p99={}ms",
        rep.latency_echo.samples, rep.latency_echo.p50_ms, rep.latency_echo.p99_ms
    );

    // ── 7) 纯 IPC 往返 ──
    phase(7, "纯 IPC Ping/Pong（50 次）");
    let mut ping_us = Vec::new();
    let mut pfails = 0;
    for seq in 0..50u64 {
        let t = std::time::Instant::now();
        if b.ping(seq).await.is_err() {
            break;
        }
        let mut got = false;
        match tokio::time::timeout(Duration::from_millis(600), async {
            loop {
                match b.rx.recv().await {
                    Some((KIND_PONG, p)) if p.len() >= 8 && u64s(&p) == seq => break,
                    Some((KIND_DATA, p)) | Some((KIND_REPLAY, p)) => tscan.push(&p),
                    Some(_) => {}
                    None => break,
                }
            }
        })
        .await
        {
            Ok(()) => {
                ping_us.push(t.elapsed().as_micros() as u64);
                got = true;
            }
            Err(_) => {}
        }
        if got {
            pfails = 0;
        } else {
            pfails += 1;
            if pfails >= 5 {
                rep.errors
                    .push("Ping/Pong 连续失败 5 次，提前结束该阶段".into());
                break;
            }
        }
    }
    rep.latency_ping = summarize_us(ping_us);
    println!(
        "      n={} p50={}ms p99={}ms",
        rep.latency_ping.samples, rep.latency_ping.p50_ms, rep.latency_ping.p99_ms
    );

    // ── 8) CSI 6n（daemon 侧最小应答器）──
    phase(8, "CSI 6n 探测（PowerShell 主动发 ESC[6n）");
    rep.notes.push(
        "CSI 6n：跑一段 PowerShell 主动发 ESC[6n，再看 daemon 是否检测到并（在开启时）应答。若 requests_seen=0，说明 ConPTY 未把该查询透传给终端侧——这本身也是结论"
            .into(),
    );
    // 标记同样用「命令行文本里不会出现的数字」：PowerShell 自己算出来
    let (csi_a, csi_b) = (100_000_001u64, 1u64);
    let csi_sum = (csi_a + csi_b).to_string();
    let probe = format!(
        "powershell -NoProfile -Command \"Write-Host -NoNewline ([char]27+'[6n'); Start-Sleep -Milliseconds 800; Write-Host ({csi_a}+{csi_b})\"\r"
    );
    if b.input(&probe).await.is_ok() {
        let _ = await_marker(&mut b, &mut tscan, csi_sum.as_bytes(), 12_000).await;
        rep.csi6n.probe_ran = tscan.contains(csi_sum.as_bytes());
    }
    drain_ms(&mut b, &mut tscan, 300).await;
    rep.csi6n.responder_enabled = responder;
    if let Ok(s) = status_json(&mut b, 3000).await {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
            rep.csi6n.requests_seen = v.get("csi6nRequests").and_then(|x| x.as_u64()).unwrap_or(0);
            rep.csi6n.replies_sent = v.get("csi6nReplies").and_then(|x| x.as_u64()).unwrap_or(0);
            rep.status_final = Some(v);
        }
    }
    println!(
        "      probe_ran={} 请求={} 应答={}",
        rep.csi6n.probe_ran, rep.csi6n.requests_seen, rep.csi6n.replies_sent
    );

    if !keep {
        let _ = b.shutdown().await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        rep.notes
            .push("已发送 SHUTDOWN（daemon 关闭 ConPTY → 会话进程树随之终止）".into());
    } else {
        rep.notes
            .push("--keep：daemon 与 shell 仍在运行，可手动 attach 继续观察".into());
    }
    finish(rep, json_path, &log_path)
}

fn finish(mut rep: Report, json_path: PathBuf, log_path: &Path) -> i32 {
    if rep.daemon_log.is_empty() {
        rep.daemon_log = logbuf::tail(log_path, 40);
    }
    let s = match serde_json::to_string_pretty(&rep) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("序列化失败：{e}");
            return 1;
        }
    };
    if let Err(e) = std::fs::write(&json_path, &s) {
        eprintln!("写报告失败 {e}");
    } else {
        println!("报告已写入：{}", json_path.display());
    }
    println!("── 摘要 ──");
    println!("  build           {}", rep.build);
    println!("  pipe            {}", rep.pipe);
    println!(
        "  shell           {}（pid {}）",
        rep.shell, rep.hello.shell_pid
    );
    println!(
        "  冒烟            pass={}（{} ms）",
        rep.smoke.pass, rep.smoke.first_output_ms
    );
    println!(
        "  存活+重放        pass={}：断开 {}ms 后重连，回放 {} 字节 / {} 个 TICK；shell alive={}",
        rep.survival.pass,
        rep.survival.detached_ms,
        rep.survival.replay_bytes,
        rep.survival.ticks_in_replay,
        rep.survival.shell_alive_after_detach
    );
    println!(
        "  吞吐            {} 字节 / {} 帧 / {} ms = {:.0} KiB/s",
        rep.throughput.bytes, rep.throughput.frames, rep.throughput.ms, rep.throughput.kib_per_sec
    );
    println!(
        "  echo 延迟        n={} p50={}ms p99={}ms max={}ms",
        rep.latency_echo.samples,
        rep.latency_echo.p50_ms,
        rep.latency_echo.p99_ms,
        rep.latency_echo.max_ms
    );
    println!(
        "  ping 延迟        n={} p50={}ms p99={}ms max={}ms",
        rep.latency_ping.samples,
        rep.latency_ping.p50_ms,
        rep.latency_ping.p99_ms,
        rep.latency_ping.max_ms
    );
    println!(
        "  CSI 6n          probe_ran={} 请求={} 应答={}",
        rep.csi6n.probe_ran, rep.csi6n.requests_seen, rep.csi6n.replies_sent
    );
    if !rep.errors.is_empty() {
        println!("── 错误 ──");
        for e in &rep.errors {
            println!("  ⚠ {e}");
        }
        println!(
            "── daemon 日志（末尾 {} 行，路径 {}）──",
            rep.daemon_log.len(),
            rep.daemon_log_path
        );
        for l in &rep.daemon_log {
            println!("  {l}");
        }
        return 2;
    }
    0
}
