//! Named Pipe 客户端 + 量测辅助。`attach` 亦供人手动验证「关掉再打开看回放」。

use crate::proto::*;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{ReadHalf, WriteHalf};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};
use tokio::sync::mpsc;

#[derive(Default)]
pub struct ConnStats {
    pub data_bytes: AtomicU64,
    pub data_frames: AtomicU64,
    pub replay_bytes: AtomicU64,
    pub gap_frames: AtomicU64,
    pub pongs: AtomicU64,
    pub other_frames: AtomicU64,
}

impl ConnStats {
    pub fn snapshot(&self) -> (u64, u64, u64, u64, u64, u64) {
        (
            self.data_bytes.load(Ordering::Relaxed),
            self.data_frames.load(Ordering::Relaxed),
            self.replay_bytes.load(Ordering::Relaxed),
            self.gap_frames.load(Ordering::Relaxed),
            self.pongs.load(Ordering::Relaxed),
            self.other_frames.load(Ordering::Relaxed),
        )
    }
}

pub struct Attached {
    pub wr: WriteHalf<NamedPipeClient>,
    pub rx: mpsc::UnboundedReceiver<(u8, Vec<u8>)>,
    pub stats: Arc<ConnStats>,
    reader: tokio::task::JoinHandle<()>,
}

impl Drop for Attached {
    fn drop(&mut self) {
        // 必须 abort：否则读任务仍持有 ReadHalf，daemon 侧看不到 EOF，
        // 「断开连接」就不是真的断开。
        self.reader.abort();
    }
}

pub async fn wait_open(pipe: &str, timeout_ms: u64) -> io::Result<NamedPipeClient> {
    let started = std::time::Instant::now();
    loop {
        match ClientOptions::new().open(pipe) {
            Ok(c) => return Ok(c),
            Err(e) => {
                if started.elapsed().as_millis() as u64 >= timeout_ms {
                    return Err(e);
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

pub async fn attach(pipe: &str, timeout_ms: u64) -> io::Result<Attached> {
    let c = wait_open(pipe, timeout_ms).await?;
    let (mut rd, wr): (ReadHalf<NamedPipeClient>, WriteHalf<NamedPipeClient>) = tokio::io::split(c);
    let (tx, rx) = mpsc::unbounded_channel::<(u8, Vec<u8>)>();
    let stats = Arc::new(ConnStats::default());
    let st = Arc::clone(&stats);

    // Hello
    let mut w = wr;
    write_frame(&mut w, KIND_HELLO, &PROTO_VERSION.to_le_bytes()).await?;

    let reader = tokio::spawn(async move {
        loop {
            match read_frame(&mut rd).await {
                Ok((kind, payload)) => {
                    match kind {
                        KIND_DATA => {
                            st.data_bytes
                                .fetch_add(payload.len() as u64, Ordering::Relaxed);
                            st.data_frames.fetch_add(1, Ordering::Relaxed);
                        }
                        KIND_REPLAY => {
                            st.replay_bytes
                                .fetch_add(payload.len() as u64, Ordering::Relaxed);
                        }
                        KIND_GAP => {
                            st.gap_frames.fetch_add(1, Ordering::Relaxed);
                        }
                        KIND_PONG => {
                            st.pongs.fetch_add(1, Ordering::Relaxed);
                        }
                        _ => {
                            st.other_frames.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    if tx.send((kind, payload)).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    Ok(Attached {
        wr: w,
        rx,
        stats,
        reader,
    })
}

impl Attached {
    pub async fn input(&mut self, s: &str) -> io::Result<()> {
        write_frame(&mut self.wr, KIND_INPUT, s.as_bytes()).await
    }
    pub async fn input_bytes(&mut self, b: &[u8]) -> io::Result<()> {
        write_frame(&mut self.wr, KIND_INPUT, b).await
    }
    pub async fn ping(&mut self, seq: u64) -> io::Result<()> {
        write_frame(&mut self.wr, KIND_PING, &put_u64(seq)).await
    }
    pub async fn request_status(&mut self) -> io::Result<()> {
        write_frame(&mut self.wr, KIND_STATUS, &[]).await
    }
    pub async fn resize(&mut self, cols: u16, rows: u16) -> io::Result<()> {
        let mut p = Vec::with_capacity(4);
        p.extend_from_slice(&cols.to_le_bytes());
        p.extend_from_slice(&rows.to_le_bytes());
        write_frame(&mut self.wr, KIND_RESIZE, &p).await
    }
    pub async fn detach(&mut self) -> io::Result<()> {
        write_frame(&mut self.wr, KIND_DETACH, &[]).await
    }
    pub async fn shutdown(&mut self) -> io::Result<()> {
        write_frame(&mut self.wr, KIND_SHUTDOWN, &[]).await
    }
}

/// 跨帧累积搜索（标记用）
pub struct Scan {
    buf: Vec<u8>,
}

impl Scan {
    pub fn new() -> Self {
        Scan { buf: Vec::new() }
    }
    pub fn push(&mut self, d: &[u8]) {
        self.buf.extend_from_slice(d);
        if self.buf.len() > 256 * 1024 {
            let cut = self.buf.len() - 256 * 1024;
            self.buf.drain(..cut);
        }
    }
    pub fn contains(&self, m: &[u8]) -> bool {
        self.buf.windows(m.len()).any(|w| w == m)
    }
    pub fn tail(&self, n: usize) -> String {
        let s = String::from_utf8_lossy(&self.buf);
        let ch: Vec<char> = s.chars().collect();
        ch[ch.len().saturating_sub(n)..].iter().collect()
    }
}

/// 等到出现标记；返回（耗时 ms, 期间收到的 data 字节数, 期间帧数）
pub async fn await_marker(
    a: &mut Attached,
    scan: &mut Scan,
    marker: &[u8],
    timeout_ms: u64,
) -> Result<(u64, u64, u64), String> {
    let t0 = std::time::Instant::now();
    let (b0, f0, _, _, _, _) = a.stats.snapshot();
    loop {
        if scan.contains(marker) {
            let (b1, f1, _, _, _, _) = a.stats.snapshot();
            return Ok((t0.elapsed().as_millis() as u64, b1 - b0, f1 - f0));
        }
        if t0.elapsed().as_millis() as u64 > timeout_ms {
            return Err(format!(
                "等待标记超时（{} ms）；最近输出尾部：{}",
                timeout_ms,
                scan.tail(400).replace('\u{1b}', "^[")
            ));
        }
        match tokio::time::timeout(Duration::from_millis(50), a.rx.recv()).await {
            Ok(Some((KIND_DATA, p))) => scan.push(&p),
            Ok(Some((KIND_REPLAY, p))) => scan.push(&p),
            Ok(Some(_)) => {}
            Ok(None) => return Err("连接已关闭".into()),
            Err(_) => {}
        }
    }
}

/// 把接下来 `ms` 毫秒内的输出全部读完，返回（data 字节, data 帧）
pub async fn drain_ms(a: &mut Attached, scan: &mut Scan, ms: u64) -> (u64, u64) {
    let t0 = std::time::Instant::now();
    let (b0, f0, _, _, _, _) = a.stats.snapshot();
    while t0.elapsed().as_millis() as u64 <= ms {
        match tokio::time::timeout(Duration::from_millis(50), a.rx.recv()).await {
            Ok(Some((KIND_DATA, p))) | Ok(Some((KIND_REPLAY, p))) => scan.push(&p),
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(_) => {}
        }
    }
    let (b1, f1, _, _, _, _) = a.stats.snapshot();
    (b1 - b0, f1 - f0)
}

/// 取一条 STATUS_REPLY 的 JSON 文本
pub async fn status_json(a: &mut Attached, timeout_ms: u64) -> Result<String, String> {
    a.request_status().await.map_err(|e| e.to_string())?;
    let t0 = std::time::Instant::now();
    while t0.elapsed().as_millis() as u64 <= timeout_ms {
        match tokio::time::timeout(Duration::from_millis(100), a.rx.recv()).await {
            Ok(Some((KIND_STATUS_REPLY, p))) => return Ok(String::from_utf8_lossy(&p).to_string()),
            Ok(Some(_)) => {}
            Ok(None) => return Err("连接已关闭".into()),
            Err(_) => {}
        }
    }
    Err("STATUS 超时".into())
}

// ───────────────────────── 手动交互 attach（raw mode） ─────────────────────────

#[cfg(windows)]
pub async fn attach_interactive(pipe: &str) -> io::Result<()> {
    use std::io::{Read, Write};
    use windows_sys::Win32::System::Console::{
        GetConsoleMode, GetStdHandle, SetConsoleMode, ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT,
        ENABLE_PROCESSED_INPUT, ENABLE_VIRTUAL_TERMINAL_INPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING,
        STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };

    let hin = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    let hout = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    let mut in_mode = 0u32;
    let mut out_mode = 0u32;
    unsafe {
        GetConsoleMode(hin, &mut in_mode);
        GetConsoleMode(hout, &mut out_mode);
        SetConsoleMode(
            hin,
            ENABLE_VIRTUAL_TERMINAL_INPUT
                | (in_mode & !(ENABLE_ECHO_INPUT | ENABLE_LINE_INPUT | ENABLE_PROCESSED_INPUT)),
        );
        SetConsoleMode(hout, out_mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
    }

    let mut a = attach(pipe, 5000).await?;
    a.resize(120, 30).await?;
    // 请求一次快照：重新 attach 时应先看到回放
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(64);
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 4096];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    eprintln!("[attach] 已连接 {pipe}（Ctrl+] 断开并离开；子进程与输出会继续存活）");
    let mut out = std::io::stdout();
    let mut ctrl_bracket = false;
    loop {
        tokio::select! {
            Some(bytes) = rx.recv() => {
                if bytes.contains(&0x1d) {
                    ctrl_bracket = true;
                    break;
                }
                a.input_bytes(&bytes).await?;
            }
            Some((kind, payload)) = a.rx.recv() => {
                match kind {
                    KIND_DATA | KIND_REPLAY | KIND_STATUS_REPLY => { let _ = out.write_all(&payload); let _ = out.flush(); }
                    KIND_GAP => eprintln!("\r\n[attach] ⚠ 广播滞后，可能丢字节\r\n"),
                    _ => {}
                }
            }
            else => break,
        }
    }
    unsafe {
        SetConsoleMode(hin, in_mode);
        SetConsoleMode(hout, out_mode);
    }
    if ctrl_bracket {
        let _ = a.detach().await;
        eprintln!("[attach] 已 detach（daemon 与会话仍在运行）");
    }
    Ok(())
}
