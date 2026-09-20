//! 会话：ConPTY + **环形缓冲** + 广播（B3 #6 的 PoC 版，不含 VT 内核与落盘）。
//!
//! 关键点（对应 ticket「UI 关闭后进程与输出存活 + 重开回放」）：
//! - PTY 由 **daemon 进程**持有 → 客户端（UI）断开不影响子进程；
//! - 输出被**无条件**读进环形缓冲，与是否有客户端无关 → 无人附着时也不丢；
//! - 每个 chunk 带**单调偏移** `start`，客户端按偏移去重（B2 #5 的累积 ack 同源思路）。

use crate::conpty::{self, Pty, PtySpawn};
use std::collections::VecDeque;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct Chunk {
    pub start: u64,
    pub bytes: Vec<u8>,
}

pub struct Ring {
    buf: VecDeque<u8>,
    pub total: u64,
    cap: usize,
}

impl Ring {
    fn push(&mut self, data: &[u8]) {
        self.buf.extend(data.iter().copied());
        self.total += data.len() as u64;
        while self.buf.len() > self.cap {
            self.buf.pop_front();
        }
    }
    pub fn snapshot(&self) -> (Vec<u8>, u64) {
        (self.buf.iter().copied().collect(), self.total)
    }
    /// 环形缓冲起点（= total - buf.len()）
    pub fn oldest(&self) -> u64 {
        self.total - self.buf.len() as u64
    }
}

pub struct Session {
    pub pty: Box<dyn Pty>,
    pub ring: Mutex<Ring>,
    pub live: broadcast::Sender<Chunk>,
    pub responder: bool,
    pub csi6n_requests: AtomicU64,
    pub csi6n_replies: AtomicU64,
    pub bytes_out: AtomicU64, // PTY → daemon
    pub bytes_in: AtomicU64,  // daemon → PTY
    pub reader_alive: AtomicBool,
    pub gaps: AtomicU64,
    pub shutdown: AtomicBool,
}

impl Session {
    pub fn spawn(req: &PtySpawn, ring_cap: usize, responder: bool) -> io::Result<Arc<Session>> {
        let (pty, mut out) = conpty::spawn(req)?;
        let (tx, _rx) = broadcast::channel(1024);
        let s = Arc::new(Session {
            pty: Box::new(pty),
            ring: Mutex::new(Ring {
                buf: VecDeque::new(),
                total: 0,
                cap: ring_cap,
            }),
            live: tx,
            responder,
            csi6n_requests: AtomicU64::new(0),
            csi6n_replies: AtomicU64::new(0),
            bytes_out: AtomicU64::new(0),
            bytes_in: AtomicU64::new(0),
            reader_alive: AtomicBool::new(true),
            gaps: AtomicU64::new(0),
            shutdown: AtomicBool::new(false),
        });

        // 阻塞读线程：与有无客户端**完全无关**，保证脱离 UI 后输出照样被缓存。
        let s2 = Arc::clone(&s);
        std::thread::spawn(move || {
            use std::io::Read;
            let mut buf = vec![0u8; 64 * 1024];
            let mut first = true;
            loop {
                match out.read(&mut buf) {
                    Ok(0) => {
                        crate::logbuf::log("PTY 读线程：EOF（子进程侧已关闭）");
                        break;
                    }
                    Ok(n) => {
                        if first {
                            crate::logbuf::log(&format!(
                                "PTY 首包 {n} 字节（说明 ConPTY 真的在产出）"
                            ));
                            first = false;
                        }
                        s2.on_output(&buf[..n]);
                    }
                    Err(e) => {
                        crate::logbuf::log(&format!("PTY 读线程出错：{e}"));
                        break;
                    }
                }
            }
            s2.reader_alive.store(false, Ordering::SeqCst);
        });

        Ok(s)
    }

    fn on_output(&self, data: &[u8]) {
        self.bytes_out
            .fetch_add(data.len() as u64, Ordering::Relaxed);
        let start = {
            let mut r = self.ring.lock().unwrap();
            let start = r.total;
            r.push(data);
            start
        };
        // 最小应答器：无 UI 附着时替 xterm.js 回答终端查询（§5.2 / WezTerm #6783）
        let n = data.windows(4).filter(|w| *w == b"\x1b[6n").count();
        if n > 0 {
            self.csi6n_requests.fetch_add(n as u64, Ordering::Relaxed);
            if self.responder {
                if self.pty.write(b"\x1b[1;1R").is_ok() {
                    self.bytes_in.fetch_add(6, Ordering::Relaxed);
                    self.csi6n_replies.fetch_add(n as u64, Ordering::Relaxed);
                }
            }
        }
        let _ = self.live.send(Chunk {
            start,
            bytes: data.to_vec(),
        });
    }

    pub fn write_input(&self, data: &[u8]) -> io::Result<()> {
        self.bytes_in
            .fetch_add(data.len() as u64, Ordering::Relaxed);
        self.pty.write(data)
    }

    pub fn status_json(&self) -> String {
        let (_, total) = {
            let r = self.ring.lock().unwrap();
            (r.oldest(), r.total)
        };
        format!(
            r#"{{"pid":{},"alive":{},"exitCode":{},"ringTotal":{},"bytesOut":{},"bytesIn":{},"csi6nRequests":{},"csi6nReplies":{},"readerAlive":{},"broadcastGaps":{}}}"#,
            self.pty.pid(),
            self.pty.alive(),
            self.pty.exit_code(),
            total,
            self.bytes_out.load(Ordering::Relaxed),
            self.bytes_in.load(Ordering::Relaxed),
            self.csi6n_requests.load(Ordering::Relaxed),
            self.csi6n_replies.load(Ordering::Relaxed),
            self.reader_alive.load(Ordering::SeqCst),
            self.gaps.load(Ordering::Relaxed),
        )
    }
}
