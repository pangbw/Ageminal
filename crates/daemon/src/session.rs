//! 会话表（issue #67）：daemon 全局的 `sid` → 会话。
//!
//! 为什么是 **daemon 全局**而不是按连接：会话要**活过 UI 关闭**（live reattach，
//! `REQUIREMENTS.md` §5.2）。连接只表示「当前谁在看」——由每个会话的 `attachment` 表示。
//!
//! 帧头的 `handle` 是**连接内**句柄（#65 定的），所以 handle ↔ `sid` 的映射归连接；输出线程从
//! 自己的 attachment 里拿 handle。
//!
//! # 回放（#72）
//!
//! 每个会话保留最近 **4 MiB** 输出，按累计字节偏移 `seq` 索引。`Attach` 时：
//! - `last_seq` 还在窗口里 → 从它之后回放，`replay_end` 之后接实时流；
//! - `last_seq == 0` 或已滑出窗口 → 发 ANSI 快照，实时流从当前 `seq` 起；
//! - 输出线程与 attach 共用 **VT → output** 锁序，快照 / `seq` / 回放窗口是同一个原子边界。
//!
//! **断开期间产生的退出事件仍会丢弃**（`attachment` 为空时无处可送），重连后表现为「没有会话」；
//! 更长的历史由落盘日志补足（#74），但「退出事件对账」仍归 #78。
//!
//! # 落盘历史（#74）
//!
//! 每个进程的 PTY 字节在喂过 VT 后按 `seq` 追加进 `history/<encoded-sid>/output.log`；内存环
//! 覆盖不到 `last_seq` 时，`attach` 从日志精确续放。checkpoint 每 1 MiB / 轮转 / 退出时更新，
//! 单会话 64 MiB、全部目录 256 MiB 预算 GC。历史 IO 失败只会关闭持久化，不影响会话。
//!
//! # 流控（#73）
//!
//! VS Code 式 high / low 水位 + 累计 ack：
//! - 未确认字节 = 已读入 PTY 的 `seq` − 客户端 ack；
//! - 超过 **256 KiB** 就暂停**读 PTY**（把背压传回 ConPTY / 子进程），低于 **64 KiB** 再恢复；
//! - 隐藏会话没有 attachment，不参与水位，继续读进 VT + 环形缓冲，切回时用快照刷新。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};

use ageminal_protocol::{
    Attached, CreateSession, Message, SessionCreated, SessionExited, SessionInfo, Snapshot,
};

use crate::history::{self, History, HistoryStore};
use crate::outbound::Outbound;
use crate::pty::{self, Pty, PtySize, PtySpec};
use crate::ring::RingBuffer;
use crate::vt::Vt;
use crate::DaemonError;

/// exit 监视线程的轮询间隔。
const EXIT_POLL: Duration = Duration::from_millis(50);

/// 一次从 PTY 读多少字节（合并成一条数据帧）。
const READ_CHUNK: usize = 16 * 1024;

/// 每会话保留的输出回放窗口（#72）。
const OUTPUT_RING_BYTES: usize = 4 * 1024 * 1024;

/// 回放时单条 `Data` 帧的上限；`MAX_FRAME_BYTES` 是 1 MiB，留足余量。
const REPLAY_CHUNK: usize = 64 * 1024;

/// 流控高水位：未确认字节超过它就暂停读 PTY。
const FLOW_HIGH_BYTES: u64 = 256 * 1024;

/// 流控低水位：未确认字节回落到它以下就恢复读 PTY。
const FLOW_LOW_BYTES: u64 = 64 * 1024;

/// 输出推给谁。
#[derive(Clone)]
struct Attachment {
    handle: u32,
    outbound: Arc<Outbound>,
}

/// 会话输出状态：`seq`、回放窗口与当前连接必须一起改，否则 attach 会看到撕裂的边界。
struct OutputState {
    /// 下一个字节的累计偏移。
    seq: u64,
    /// 最近一段输出（按绝对偏移索引）。
    ring: RingBuffer,
    /// 当前挂着的连接。
    attachment: Option<Attachment>,
}

/// 每会话的流控记账。
///
/// `sent` 是输出线程已经读入并计入 `seq` 的绝对偏移；`acked` 是渲染器确认消费到的绝对偏移。
/// 两者相减就是「在途未确认字节」。`paused` 只控制**是否继续读 PTY**——暂停后不会再产生新的
/// 在途字节，背压经 ConPTY 传回子进程。
#[derive(Debug, Default)]
struct FlowState {
    /// 客户端已确认到的绝对偏移。
    acked: u64,
    /// 已读入 / 已计入 `seq` 的绝对偏移。
    sent: u64,
    /// 是否因高水位暂停读 PTY。
    paused: bool,
}

impl FlowState {
    /// attach 换了一个确认基点：回放路径从 `from_seq` 起算，快照路径从当前 `sent` 起算。
    fn reset(&mut self, from_seq: u64, sent: u64) {
        self.acked = from_seq;
        self.sent = sent;
        self.paused = sent.saturating_sub(from_seq) > FLOW_HIGH_BYTES;
    }

    /// 记录新读入的输出；只有挂着可见连接时才可能触发高水位。
    fn observe(&mut self, sent: u64, attached: bool) {
        self.sent = sent;
        if attached && sent.saturating_sub(self.acked) > FLOW_HIGH_BYTES {
            self.paused = true;
        }
    }

    /// 累计 ack；返回是否从暂停恢复（调用方据此唤醒输出线程）。
    fn ack(&mut self, acked: u64) -> bool {
        // 迟到 / 跨 attach 的旧 ack 不能把确认值往回拽，也不能超过已发送量。
        self.acked = acked.min(self.sent).max(self.acked);
        if self.paused && self.sent.saturating_sub(self.acked) <= FLOW_LOW_BYTES {
            self.paused = false;
            true
        } else {
            false
        }
    }

    /// 隐藏 / 终止：不再需要水位门闩。
    fn release(&mut self) {
        self.paused = false;
    }
}

/// 一个会话（跨连接存活）。
struct Session {
    /// 逻辑会话 id。
    sid: String,
    /// 本次 spawn 的世代。**迟到 exit 靠它判**。
    incarnation_id: String,
    /// 子进程 pid（运行态元数据；`ListSessions` 与 attach 结果用）。
    pid: u32,
    /// spawn 时的工作目录（UI 重开时重建页签）。
    cwd: String,
    /// spawn 时间（Unix 毫秒）；用于稳定排序 / 后续启动对账。
    started_at_ms: u64,
    /// PTY 控制端（resize / terminate）。**可取走**：`terminate` 把 PTY 交给独立线程去关，
    /// 免得 `ClosePseudoConsole` 等 conout 排空时卡住连接线程。
    control: Mutex<Option<Box<dyn Pty>>>,
    /// 只用于「等退出」的进程句柄——**不经 `control` 锁**，否则 terminate 一卡，退出事件也发不出来。
    exit: Option<OwnedHandle>,
    /// PTY 写端（键盘输入）。
    writer: Mutex<Box<dyn Write + Send>>,
    /// 服务端 VT 内核（#70）：屏幕 / 光标 / alt-screen / SGR。
    vt: Mutex<Vt>,
    /// 输出偏移 / 回放窗口 / 当前连接（#72）。
    output: Mutex<OutputState>,
    /// 流控记账（#73）。
    flow: Mutex<FlowState>,
    /// 流控门闩的唤醒（ack / detach / terminate）。
    flow_wake: Condvar,
    /// 有界落盘历史（#74）；`None` = 测试 / 不支持持久化。
    history: Mutex<Option<History>>,
}

/// daemon 全局的会话表。
#[derive(Default)]
pub struct SessionTable {
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    next_incarnation: AtomicU64,
    /// 所有会话共享的历史根；测试里为 `None`，生产由 server 注入。
    history: Option<HistoryStore>,
}

impl SessionTable {
    /// 建空表（不落盘历史；测试用）。
    #[cfg(test)]
    pub fn new() -> Self {
        Self::default()
    }

    /// 建带落盘历史的表（生产用）。
    pub fn with_history(history: HistoryStore) -> Self {
        Self {
            history: Some(history),
            ..Self::default()
        }
    }

    /// 建会话：spawn PTY → 起输出线程 + exit 监视线程。
    ///
    /// `handle` 与 `outbound` 决定输出推给谁（连接内句柄由调用方分配）。
    pub fn create(
        self: &Arc<Self>,
        request: &CreateSession,
        handle: u32,
        outbound: Arc<Outbound>,
    ) -> Result<SessionCreated, DaemonError> {
        if request.command.is_some() {
            // 启动命令是 Agent 页签的东西（#79）；默默忽略会让用户以为生效了。
            return Err(DaemonError::Session {
                reason: "会话的启动命令（command）尚未支持——Agent 页签归 #79".to_owned(),
            });
        }

        // 先查一次（便宜且在锁外 spawn 之前），spawn 之后再原子地抢一次座。
        if self
            .sessions
            .lock()
            .expect("sessions")
            .contains_key(&request.sid)
        {
            return Err(DaemonError::Session {
                reason: format!("会话 {} 还活着；要重开请先 terminate", request.sid),
            });
        }

        // 历史 GC 放在 spawn 前：清掉旧目录不会碰到刚建好的会话。
        self.collect_history_garbage(Some(&request.sid));

        let mut control = pty::spawn(&spec_from(request))?;
        let pid = control.pid();
        // 解耦「等退出」：自己开一个进程句柄，`terminate` 阻塞也不会影响退出事件。
        let exit = crate::win::open_process(pid)?;
        let reader = control.take_reader().ok_or_else(|| DaemonError::Protocol {
            reason: "PTY 没有输出流".to_owned(),
        })?;
        let writer = control.take_writer().ok_or_else(|| DaemonError::Protocol {
            reason: "PTY 没有输入流".to_owned(),
        })?;

        let incarnation_id = format!(
            "inc-{}",
            self.next_incarnation.fetch_add(1, Ordering::Relaxed)
        );
        let vt = Vt::new(request.cols as usize, request.rows as usize);
        let history = self.open_history(&request.sid, &vt);
        let session = Arc::new(Session {
            sid: request.sid.clone(),
            incarnation_id: incarnation_id.clone(),
            pid,
            cwd: request.cwd.clone(),
            started_at_ms: crate::logging::now_ms(),
            control: Mutex::new(Some(control)),
            exit: Some(exit),
            writer: Mutex::new(writer),
            vt: Mutex::new(vt),
            output: Mutex::new(OutputState {
                seq: 0,
                ring: RingBuffer::new(OUTPUT_RING_BYTES),
                attachment: Some(Attachment { handle, outbound }),
            }),
            flow: Mutex::new(FlowState::default()),
            flow_wake: Condvar::new(),
            history: Mutex::new(history),
        });

        // spawn 期间表可能被塞进同名会话（单连接下不会，但别赌）。**保住活着的那一个**，
        // 把我们刚建的这个关掉——不能反过来杀掉别人的会话。
        {
            let mut sessions = self.sessions.lock().expect("sessions");
            if sessions.contains_key(&session.sid) {
                drop(sessions);
                if let Some(mut pty) = session.control.lock().expect("control").take() {
                    std::thread::spawn(move || {
                        let _ = pty.terminate();
                        drop(pty);
                    });
                }
                return Err(DaemonError::Session {
                    reason: format!("会话 {} 还活着；要重开请先 terminate", request.sid),
                });
            }
            sessions.insert(session.sid.clone(), session.clone());
        }

        spawn_output_pump(session.clone(), reader);
        spawn_exit_watcher(self.clone(), session);

        Ok(SessionCreated {
            sid: request.sid.clone(),
            handle,
            pid,
            incarnation_id,
        })
    }

    /// 接回已有会话；把输出改推到这条连接。
    ///
    /// 三种情况：
    /// - `last_seq` 还在环形窗口里 → 从它之后**精确回放**，`replay_end` 之后才是实时流；
    /// - `last_seq == 0`（新 UI）或已经滑出窗口 → 用 ANSI 快照建屏，实时流从当前 `seq` 开始；
    /// - `last_seq == 当前 seq` → 无需回放。
    ///
    /// 锁顺序固定为 **VT → output**：输出线程也按这个顺序拿锁，因此快照 / `seq` / 回放窗口
    /// 是同一个原子边界。
    pub fn attach(
        &self,
        sid: &str,
        last_seq: u64,
        handle: u32,
        outbound: Arc<Outbound>,
    ) -> Result<Attached, DaemonError> {
        let session = self.get(sid)?;
        let vt = session.vt.lock().expect("vt");
        let mut output = session.output.lock().expect("output");
        // 锁顺序：VT → output → history（输出线程同序）。
        let history = session.history.lock().expect("history");
        let current = output.seq;
        let alt_screen = vt.alt_screen();

        let disk_replay = |from: u64| -> Option<Vec<u8>> {
            let history = history.as_ref()?;
            match history.replay_from(from) {
                Ok(bytes) => bytes,
                Err(error) => {
                    history.warn(&format!("读取会话 {sid} 的历史失败：{error}"));
                    None
                }
            }
        };

        let (snapshot, from_seq, replay) = if last_seq > 0 && last_seq <= current {
            match output.ring.since(last_seq) {
                Some(bytes) => (None, last_seq, bytes),
                // 内存环只覆盖最近 4 MiB；更长的 detach 输出从落盘日志续放（#74）。
                None => match disk_replay(last_seq) {
                    Some(bytes) => (None, last_seq, bytes),
                    None => (Some(vt.snapshot()), current, Vec::new()),
                },
            }
        } else {
            // `last_seq == 0`：新 UI，没有可信的屏幕；用快照重建。
            (Some(vt.snapshot()), current, Vec::new())
        };

        // 先把回放帧排进出站队列，再让输出线程继续追；两者都在同一把 output 锁内，
        // 因此不会出现「实时流插到回放中间」。
        output.attachment = Some(Attachment {
            handle,
            outbound: outbound.clone(),
        });
        let mut offset = 0usize;
        for chunk in replay.chunks(REPLAY_CHUNK) {
            outbound.send_data(&Message::Data {
                handle,
                seq: from_seq + offset as u64,
                bytes: chunk.to_vec(),
            });
            offset += chunk.len();
        }

        // 新连接的新确认基点：回放字节算在途，快照 / 已到末尾则是零。
        session.flow.lock().expect("flow").reset(from_seq, current);
        session.flow_wake.notify_all();

        Ok(Attached {
            sid: sid.to_owned(),
            handle,
            from_seq,
            replay_end: current,
            snapshot,
            alt_screen,
            cols: vt.columns() as u16,
            rows: vt.screen_lines() as u16,
        })
    }

    /// 当前存活的会话清单（`ListSessions` / UI 重开恢复，#75）。
    ///
    /// 表里只有活着的会话——退出 watcher 摘除后会立刻消失；因此**不能用「清单里没有」推断
    /// 进程从未存在**，启动对账的四象限留给 #78。
    pub fn list(&self) -> Vec<SessionInfo> {
        let mut sessions: Vec<SessionInfo> = self
            .sessions
            .lock()
            .expect("sessions")
            .values()
            .map(|session| {
                // 一次锁拿到两个尺寸：在同一表达式里二次 lock 同一把 std Mutex 会死锁。
                let vt = session.vt.lock().expect("vt");
                SessionInfo {
                    sid: session.sid.clone(),
                    incarnation_id: session.incarnation_id.clone(),
                    pid: session.pid,
                    cwd: session.cwd.clone(),
                    cols: vt.columns() as u16,
                    rows: vt.screen_lines() as u16,
                    started_at_ms: session.started_at_ms,
                }
            })
            .collect();
        // HashMap 顺序不稳定；按 spawn 时间排序，同一毫秒再按 sid 兜底。
        sessions.sort_by(|left, right| {
            (left.started_at_ms, &left.sid).cmp(&(right.started_at_ms, &right.sid))
        });
        sessions
    }

    /// 断开（**≠ 终止**）：会话继续活着，只是不再往任何连接推输出。
    pub fn detach(&self, sid: &str) {
        if let Some(session) = self.sessions.lock().expect("sessions").get(sid) {
            session.output.lock().expect("output").attachment = None;
            release_flow(session);
        }
    }

    /// 连接断了：把所有挂在它上面的会话摘掉（不然输出线程会往死连接里塞）。
    pub fn detach_all(&self, outbound: &Arc<Outbound>) {
        let sessions: Vec<Arc<Session>> = self
            .sessions
            .lock()
            .expect("sessions")
            .values()
            .cloned()
            .collect();
        for session in sessions {
            let mut output = session.output.lock().expect("output");
            if output
                .attachment
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(&current.outbound, outbound))
            {
                output.attachment = None;
                drop(output);
                release_flow(&session);
            }
        }
    }

    /// 终止会话。真正把这条会话从表里摘掉的是 exit 监视线程（它会发 `SessionExited`）。
    ///
    /// `force` 是协议里的「先礼后兵」，v1 忽略：一律 `ClosePseudoConsole` 硬终止
    /// （优雅路径要等 Agent 页签那批票）。
    pub fn terminate(&self, sid: &str, _force: bool) -> Result<(), DaemonError> {
        let session = self.get(sid)?;
        // 把 PTY **取出来**（锁只持有一瞬），再交给独立线程去关：
        // `ClosePseudoConsole` 要等 conout 排空，会话刚创建就终止时这一步可能很久。
        let pty = session.control.lock().expect("control").take();
        let Some(mut pty) = pty else {
            release_flow(&session);
            return Ok(()); // 已经终止过：幂等。
        };
        release_flow(&session);
        std::thread::spawn(move || {
            let _ = pty.terminate();
            drop(pty);
        });
        Ok(())
    }

    /// 终止**所有**会话（daemon 停机路径，issue #76）。
    ///
    /// 只负责发起终止；真正从表里摘除仍由各自的 exit watcher 完成。调用方用
    /// [`Self::wait_empty`] 等它们清空。
    pub fn terminate_all(&self) {
        let sids: Vec<String> = self
            .sessions
            .lock()
            .expect("sessions")
            .keys()
            .cloned()
            .collect();
        for sid in sids {
            let _ = self.terminate(&sid, true);
        }
    }

    /// 等会话表清空；返回是否在超时前清空。
    pub fn wait_empty(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if self.sessions.lock().expect("sessions").is_empty() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(EXIT_POLL);
        }
    }

    /// 改尺寸。
    pub fn resize(&self, sid: &str, size: PtySize) -> Result<(), DaemonError> {
        let session = self.get(sid)?;
        let control = session.control.lock().expect("control");
        let Some(pty) = control.as_ref() else {
            return Err(DaemonError::Session {
                reason: format!("会话 {sid} 已终止"),
            });
        };
        pty.resize(size)?;
        session
            .vt
            .lock()
            .expect("vt")
            .resize(size.cols as usize, size.rows as usize);
        Ok(())
    }

    /// 写键盘输入。
    pub fn write(&self, sid: &str, bytes: &[u8]) -> Result<(), DaemonError> {
        let session = self.get(sid)?;
        let mut writer = session.writer.lock().expect("pty writer");
        writer.write_all(bytes)?;
        writer.flush()?;
        Ok(())
    }

    /// 记下客户端已消费的绝对字节偏移；跨过低水位时唤醒输出线程（#73）。
    pub fn ack(&self, sid: &str, acked_bytes: u64) -> Result<(), DaemonError> {
        let session = self.get(sid)?;
        let resumed = session.flow.lock().expect("flow").ack(acked_bytes);
        if resumed {
            session.flow_wake.notify_all();
        }
        Ok(())
    }

    /// seq 断档后的 checkpoint：把当前屏幕 + 对应 `seq` 发给这条连接（#72）。
    ///
    /// 没有连接时不发；控制帧优先队列保证它先于后续数据到达客户端。
    pub fn checkpoint(&self, sid: &str) -> Result<(), DaemonError> {
        let session = self.get(sid)?;
        let vt = session.vt.lock().expect("vt");
        let output = session.output.lock().expect("output");
        let seq = output.seq;
        let alt_screen = vt.alt_screen();
        let ansi = vt.snapshot();
        let attachment = output.attachment.clone();
        drop(output);
        drop(vt);

        if let Some(attachment) = attachment {
            attachment
                .outbound
                .send_control(&Message::Snapshot(Snapshot {
                    sid: sid.to_owned(),
                    seq,
                    ansi,
                    alt_screen,
                }))
                .ok();
        }
        Ok(())
    }

    /// 为一个新会话开历史；失败只记录并降级为「不落盘」。
    fn open_history(&self, sid: &str, vt: &Vt) -> Option<History> {
        let store = self.history.as_ref()?;
        let ansi = vt.snapshot();
        let snapshot = history::Snapshot {
            ansi: &ansi,
            cols: vt.columns() as u16,
            rows: vt.screen_lines() as u16,
            alt_screen: vt.alt_screen(),
        };
        match store.create(sid, snapshot) {
            Ok(history) => Some(history),
            Err(error) => {
                store.warn(&format!("会话 {sid} 的历史初始化失败：{error}"));
                None
            }
        }
    }

    /// 按总预算清理历史；`extra` 用于「刚创建 / 还未插入会话表」的 sid。
    fn collect_history_garbage(&self, extra: Option<&str>) {
        let Some(store) = self.history.as_ref() else {
            return;
        };
        let mut active: Vec<String> = self
            .sessions
            .lock()
            .expect("sessions")
            .keys()
            .cloned()
            .collect();
        if let Some(sid) = extra {
            if !active.iter().any(|current| current == sid) {
                active.push(sid.to_owned());
            }
        }
        // `active` 用原始 sid；GC 内部统一编码成目录名再比较。
        let report = store.collect_garbage(&active);
        if report.removed_dirs > 0 {
            store.warn(&format!(
                "历史 GC 删除 {} 个目录 / {} 字节",
                report.removed_dirs, report.removed_bytes
            ));
        }
    }

    /// 取会话。
    fn get(&self, sid: &str) -> Result<Arc<Session>, DaemonError> {
        self.sessions
            .lock()
            .expect("sessions")
            .get(sid)
            .cloned()
            .ok_or_else(|| DaemonError::Session {
                reason: format!("没有会话 {sid}"),
            })
    }

    /// 往表里放一个会话；返回被顶掉的旧值（只给单测用——生产路径在 `create` 里原子地查+插）。
    #[cfg(test)]
    fn insert(&self, session: Arc<Session>) -> Option<Arc<Session>> {
        self.sessions
            .lock()
            .expect("sessions")
            .insert(session.sid.clone(), session)
    }

    /// 这个 `sid` 当前是否仍是这一代（最终 checkpoint 要在摘除前做，避免 GC 抢删目录）。
    fn is_current(&self, sid: &str, incarnation_id: &str) -> bool {
        self.sessions
            .lock()
            .expect("sessions")
            .get(sid)
            .is_some_and(|session| session.incarnation_id == incarnation_id)
    }

    /// 「只有**当前世代**的 exit 才上报」——判定与摘除在**同一次加锁**里完成。
    ///
    /// 这是验收 3 的机制：如果先查后删分两步，那么「查完→删掉→新世代建好→旧 exit 才发」这个
    /// 窗口里就会放出一条属于旧世代的 `SessionExited`。原子地 `remove_if_current` 把窗口关死。
    fn remove_if_current(&self, sid: &str, incarnation_id: &str) -> bool {
        let mut sessions = self.sessions.lock().expect("sessions");
        let current = sessions
            .get(sid)
            .is_some_and(|session| session.incarnation_id == incarnation_id);
        if current {
            sessions.remove(sid);
        }
        current
    }
}

/// 输出线程：PTY 字节 → **先喂 VT 内核** → 数据帧（带连接的 handle 与累计字节偏移）。
fn spawn_output_pump(session: Arc<Session>, mut reader: Box<dyn Read + Send>) {
    thread::spawn(move || pump_output(&session, &mut *reader));
}

/// 同步排空一个输出流（抽出来好让单测不启线程）。
fn pump_output(session: &Session, reader: &mut dyn Read) {
    let mut buffer = vec![0u8; READ_CHUNK];
    loop {
        wait_for_flow(session);
        let read = match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        // 服务端状态必须先于转发更新，且要与 attach 的「快照 + seq」共用一个原子边界。
        // 锁顺序固定为 VT → output → flow（attach / checkpoint 同序）。
        let mut vt = session.vt.lock().expect("vt");
        vt.feed(&buffer[..read]);
        let mut output = session.output.lock().expect("output");
        let seq = output.seq;
        output.seq += read as u64;
        output.ring.push(seq, &buffer[..read]);

        // 有界落盘历史（#74）：在 VT / output 锁内写，保证磁盘尾部与 `seq` 对齐；
        // 历史 IO 失败只关闭持久化，不影响会话本身。
        {
            let mut history = session.history.lock().expect("history");
            let mut disable = false;
            if let Some(writer) = history.as_mut() {
                let rotate = writer.would_rotate(read);
                let periodic = !rotate && writer.needs_checkpoint(read);
                let checkpoint_ansi = (rotate || periodic).then(|| vt.snapshot());
                let checkpoint = checkpoint_ansi.as_deref().map(|ansi| history::Snapshot {
                    ansi,
                    cols: vt.columns() as u16,
                    rows: vt.screen_lines() as u16,
                    alt_screen: vt.alt_screen(),
                });
                let mut failure = writer
                    .append(seq, &buffer[..read], if rotate { checkpoint } else { None })
                    .err();
                if failure.is_none() && periodic {
                    failure = writer
                        .checkpoint(
                            writer.end_seq(),
                            checkpoint.expect("周期性 checkpoint 快照"),
                        )
                        .err();
                }
                if let Some(error) = failure {
                    writer.warn(&format!("会话 {} 停止历史落盘：{error}", session.sid));
                    disable = true;
                }
            }
            if disable {
                *history = None;
            }
        }

        let attachment = output.attachment.clone();
        let attached = attachment.is_some();
        session
            .flow
            .lock()
            .expect("flow")
            .observe(output.seq, attached);
        drop(output);
        drop(vt);
        if let Some(attachment) = attachment {
            attachment.outbound.send_data(&Message::Data {
                handle: attachment.handle,
                seq,
                bytes: buffer[..read].to_vec(),
            });
        }
        // 没有连接 → 只留在环形缓冲里，等 attach 回放（#72）。
    }
}

/// 高水位期间在**读 PTY 之前**等 ack。隐藏会话没有收件人，不参与水位。
fn wait_for_flow(session: &Session) {
    loop {
        let attached = session.output.lock().expect("output").attachment.is_some();
        let flow = session.flow.lock().expect("flow");
        if !attached || !flow.paused {
            return;
        }
        drop(session.flow_wake.wait(flow).expect("flow wait"));
    }
}

/// 隐藏 / 终止 / 退出：解除门闩，让输出线程继续读（或读到 EOF）。
fn release_flow(session: &Session) {
    session.flow.lock().expect("flow").release();
    session.flow_wake.notify_all();
}

/// 会话退出前补最后一份 checkpoint（日志仍是完整增量，便于冷恢复）。
fn checkpoint_history(session: &Session) {
    let vt = session.vt.lock().expect("vt");
    let output = session.output.lock().expect("output");
    let mut history = session.history.lock().expect("history");
    let Some(writer) = history.as_mut() else {
        return;
    };
    let seq = output.seq;
    let ansi = vt.snapshot();
    let snapshot = history::Snapshot {
        ansi: &ansi,
        cols: vt.columns() as u16,
        rows: vt.screen_lines() as u16,
        alt_screen: vt.alt_screen(),
    };
    if let Err(error) = writer.checkpoint(seq, snapshot) {
        writer.warn(&format!(
            "会话 {} 最终 checkpoint 失败：{error}",
            session.sid
        ));
    }
}

/// exit 监视线程：在**自己的**进程句柄上轮询，不碰 `control` 锁。
///
/// 用轮询而不是阻塞 `wait`：关控制台那一步可能很久（`terminate` 已挪到独立线程），阻塞式等待会占着
/// 一条线程直到进程真的退出，对「刚创建就终止」这类场景反而更容易叠住。
fn spawn_exit_watcher(table: Arc<SessionTable>, session: Arc<Session>) {
    thread::spawn(move || {
        // 句柄在 `session` 里（`OwnedHandle`，Send）；裸 HANDLE 是 `*mut c_void`，不能直接跨线程。
        let Some(exit) = session.exit.as_ref() else {
            return;
        };
        let handle = exit.as_raw_handle();
        loop {
            match unsafe { WaitForSingleObject(handle, 0) } {
                WAIT_OBJECT_0 => {
                    let mut code: u32 = 0;
                    // 拿不到退出码也要继续：否则这个会话会**永久留在表里**，同名重建永远被拒。
                    let _ = unsafe { GetExitCodeProcess(handle, &mut code) };

                    // 只有「表里当前这一代」才上报（迟到 exit 按 incarnationId 丢弃）。
                    // 最终 checkpoint 在摘除前做：此时历史目录仍被 active 保护，不会被 GC 抢删。
                    if table.is_current(&session.sid, &session.incarnation_id) {
                        checkpoint_history(&session);
                    }
                    if table.remove_if_current(&session.sid, &session.incarnation_id) {
                        release_flow(&session);
                        let attachment = session.output.lock().expect("output").attachment.clone();
                        if let Some(attachment) = attachment {
                            let _ = attachment.outbound.send_control(&Message::SessionExited(
                                SessionExited {
                                    sid: session.sid.clone(),
                                    incarnation_id: session.incarnation_id.clone(),
                                    // Windows 退出码是 u32；按位转 i32（0xC000_0005 显示成负数，
                                    // 与任务管理器的惯例一致）。
                                    exit_code: Some(code as i32),
                                },
                            ));
                        }
                        // 会话已经退出：变成 GC 候选（总预算超出时删最旧的）。
                        table.collect_history_garbage(None);
                    }
                    return;
                }
                WAIT_TIMEOUT => thread::sleep(EXIT_POLL),
                // 句柄失效：没什么可上报的。
                _ => return,
            }
        }
    });
}

/// `CreateSession` → [`PtySpec`]。
fn spec_from(request: &CreateSession) -> PtySpec {
    let (program, args) = match &request.shell {
        Some(program) => (program.clone(), shell_args(program)),
        None => default_shell(),
    };
    PtySpec {
        program,
        args,
        cwd: PathBuf::from(&request.cwd),
        size: PtySize {
            cols: request.cols,
            rows: request.rows,
        },
        env: request
            .env
            .iter()
            .map(|variable| (variable.name.clone(), variable.value.clone()))
            .collect(),
    }
}

/// 默认 shell：Git Bash（`bash.exe --login -i`），找不到就回退 PowerShell（`REQUIREMENTS.md` §7.1）。
fn default_shell() -> (String, Vec<String>) {
    if let Some(bash) = find_git_bash() {
        let program = bash.to_string_lossy().into_owned();
        let args = shell_args(&program);
        return (program, args);
    }
    let program = String::from("powershell.exe");
    let args = shell_args(&program);
    (program, args)
}

/// 找 `bash.exe`：先看 Git for Windows 的常见安装位，再退到 `PATH`。
fn find_git_bash() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    for variable in [
        "ProgramFiles",
        "ProgramW6432",
        "ProgramFiles(x86)",
        "LOCALAPPDATA",
    ] {
        let Some(root) = std::env::var_os(variable) else {
            continue;
        };
        let mut path = PathBuf::from(root);
        if variable == "LOCALAPPDATA" {
            // Git 的「仅当前用户」安装落在 `%LOCALAPPDATA%\Programs\Git`。
            path.push("Programs");
        }
        path.push("Git");
        path.push("bin");
        path.push("bash.exe");
        candidates.push(path);
    }

    candidates
        .into_iter()
        .find(|path| path.is_file())
        .or_else(|| {
            // PATH 上的 `bash.exe` 可能是 **WSL 的** `C:\Windows\System32\bash.exe`（打开的是 Linux），
            // 与「不把 WSL 当运行目标」相悖。只认路径里带 `Git` 的那个。
            std::env::var_os("PATH").and_then(|path| {
                std::env::split_paths(&path)
                    .map(|directory| directory.join("bash.exe"))
                    .find(|candidate| {
                        candidate.is_file()
                            && candidate.to_string_lossy().to_lowercase().contains("git")
                    })
            })
        })
}

/// 按可执行文件名给默认参数——覆盖 `shell` 时也一样，免得调用方手填。
fn shell_args(program: &str) -> Vec<String> {
    let name = Path::new(program)
        .file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let args: &[&str] = match name.as_str() {
        "bash.exe" | "bash" => &["--login", "-i"],
        "pwsh.exe" | "pwsh" | "powershell.exe" | "powershell" => &["-NoLogo"],
        _ => &[],
    };
    args.iter().map(|arg| (*arg).to_owned()).collect()
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};

    use crate::pty::{ExitStatus, Pty, PtyError};

    use super::*;

    /// 不碰真进程的 PTY，只为测会话表的记账逻辑。
    struct FakePty;

    impl Pty for FakePty {
        fn pid(&self) -> u32 {
            0
        }

        fn resize(&self, _size: PtySize) -> Result<(), PtyError> {
            Ok(())
        }

        fn take_reader(&mut self) -> Option<Box<dyn Read + Send>> {
            None
        }

        fn take_writer(&mut self) -> Option<Box<dyn Write + Send>> {
            None
        }

        fn wait(&mut self) -> Result<ExitStatus, PtyError> {
            Ok(ExitStatus { code: 0 })
        }

        fn try_wait(&mut self) -> Result<Option<ExitStatus>, PtyError> {
            Ok(Some(ExitStatus { code: 0 }))
        }

        fn terminate(&mut self) -> Result<(), PtyError> {
            Ok(())
        }
    }

    fn session(sid: &str, incarnation_id: &str) -> Arc<Session> {
        session_with_ring(sid, incarnation_id, OUTPUT_RING_BYTES)
    }

    fn session_with_ring(sid: &str, incarnation_id: &str, ring_bytes: usize) -> Arc<Session> {
        Arc::new(Session {
            sid: sid.to_owned(),
            incarnation_id: incarnation_id.to_owned(),
            pid: 0,
            cwd: std::env::temp_dir().to_string_lossy().into_owned(),
            started_at_ms: 0,
            control: Mutex::new(Some(Box::new(FakePty))),
            exit: None,
            writer: Mutex::new(Box::new(std::io::sink())),
            vt: Mutex::new(Vt::new(80, 24)),
            output: Mutex::new(OutputState {
                seq: 0,
                ring: RingBuffer::new(ring_bytes),
                attachment: None,
            }),
            flow: Mutex::new(FlowState::default()),
            flow_wake: Condvar::new(),
            history: Mutex::new(None),
        })
    }

    fn session_with_history(
        sid: &str,
        incarnation_id: &str,
        ring_bytes: usize,
        store: &HistoryStore,
    ) -> Arc<Session> {
        let vt = Vt::new(80, 24);
        let ansi = vt.snapshot();
        let history = store
            .create(
                sid,
                history::Snapshot {
                    ansi: &ansi,
                    cols: vt.columns() as u16,
                    rows: vt.screen_lines() as u16,
                    alt_screen: vt.alt_screen(),
                },
            )
            .expect("history");
        Arc::new(Session {
            sid: sid.to_owned(),
            incarnation_id: incarnation_id.to_owned(),
            pid: 0,
            cwd: std::env::temp_dir().to_string_lossy().into_owned(),
            started_at_ms: 0,
            control: Mutex::new(Some(Box::new(FakePty))),
            exit: None,
            writer: Mutex::new(Box::new(std::io::sink())),
            vt: Mutex::new(vt),
            output: Mutex::new(OutputState {
                seq: 0,
                ring: RingBuffer::new(ring_bytes),
                attachment: None,
            }),
            flow: Mutex::new(FlowState::default()),
            flow_wake: Condvar::new(),
            history: Mutex::new(Some(history)),
        })
    }

    /// 验收 3 的机制：**迟到 exit 按 `incarnationId` 丢弃**。
    #[test]
    fn a_late_exit_from_an_old_incarnation_is_dropped() {
        let table = SessionTable::new();
        table.insert(session("s", "inc-1"));

        // 当前世代的 exit：算数，并且顺手把会话摘掉。
        assert!(table.remove_if_current("s", "inc-1"));
        assert!(table.get("s").is_err(), "摘掉之后表里就没有了");

        // 仿真「旧世代晚到」：表里已经是 inc-2，inc-1 的 exit 必须丢掉。
        table.insert(session("s", "inc-2"));
        assert!(
            !table.remove_if_current("s", "inc-1"),
            "旧世代的 exit 不能上报"
        );
        assert!(table.get("s").is_ok(), "也不能把新世代顶掉");

        // 同一代只摘一次。
        assert!(table.remove_if_current("s", "inc-2"));
        assert!(!table.remove_if_current("s", "inc-2"));
    }

    /// #73：高潮位暂停，确认量回落到低水位才恢复；ack 不能超过已发送量。
    #[test]
    fn flow_control_pauses_at_high_and_resumes_at_low() {
        let mut flow = FlowState::default();
        let sent = FLOW_HIGH_BYTES + 1;
        flow.observe(sent, true);
        assert!(flow.paused, "超过高水位必须暂停读 PTY");

        // 还差 1 字节才到低水位：继续等。
        assert!(!flow.ack(sent - FLOW_LOW_BYTES - 1));
        assert!(flow.paused);

        // 回落到低水位：恢复。
        assert!(flow.ack(sent - FLOW_LOW_BYTES));
        assert!(!flow.paused);

        // 迟到 / 超量 ack 被夹到 [当前确认, 已发送]。
        flow.ack(u64::MAX);
        assert_eq!(flow.acked, sent);
        flow.ack(0);
        assert_eq!(flow.acked, sent, "旧 ack 不能把确认值拽回去");
    }

    /// 隐藏会话没有收件人，水位不该让它停止更新 VT / 环形缓冲。
    #[test]
    fn hidden_output_does_not_pause_the_reader() {
        let mut flow = FlowState::default();
        flow.observe(FLOW_HIGH_BYTES * 2, false);
        assert!(!flow.paused);

        // 重新可见后，同样的在途量会立刻重新触发高水位。
        flow.observe(FLOW_HIGH_BYTES * 2, true);
        assert!(flow.paused);
    }

    /// 新 attach 换基点：快照是零在途；回放超过高水位则先暂停，等 ack 追上。
    #[test]
    fn attach_resets_the_flow_watermarks() {
        let mut flow = FlowState::default();
        flow.reset(1000, 1000);
        assert!(!flow.paused);
        assert_eq!((flow.acked, flow.sent), (1000, 1000));

        flow.reset(1000, 1000 + FLOW_HIGH_BYTES + 1);
        assert!(flow.paused, "大回放要先按高水位节流");

        flow.release();
        assert!(!flow.paused, "detach / terminate 必须解除门闩");
    }

    fn outbound() -> Arc<Outbound> {
        Arc::new(Outbound::new(
            std::env::temp_dir().join("ageminal-session-replay-test.log"),
        ))
    }

    /// 排空出站队列并解出所有消息（attach 的回放 / checkpoint 的 Snapshot 都在里面）。
    fn drain(outbound: &Arc<Outbound>) -> Vec<Message> {
        outbound.close();
        let mut bytes = Vec::new();
        outbound.run(&mut bytes).expect("outbound writer");
        let mut cursor = bytes.as_slice();
        let mut messages = Vec::new();
        while let Some(frame) = ageminal_protocol::Frame::read_from(&mut cursor).expect("frame") {
            messages.push(Message::from_frame(&frame).expect("message"));
        }
        messages
    }

    /// #72 验收 1：从 `last_seq` 回放，再接实时流；偏移连续、不重不漏。
    #[test]
    fn attach_replays_and_then_continues_live_without_gaps() {
        let table = SessionTable::new();
        table.insert(session("s", "inc-1"));
        PumpFixture::feed(&table, b"0123456789");

        let outbound = outbound();
        let attached = table.attach("s", 4, 7, outbound.clone()).expect("attach");
        assert_eq!(attached.from_seq, 4);
        assert_eq!(attached.replay_end, 10);
        assert!(attached.snapshot.is_none(), "窗口内不需要快照");
        assert!(!attached.alt_screen);

        PumpFixture::feed(&table, b"XYZ");

        let mut replay = Vec::new();
        let mut seqs = Vec::new();
        for message in drain(&outbound) {
            if let Message::Data { seq, bytes, .. } = message {
                seqs.push(seq);
                replay.extend(bytes);
            }
        }
        assert_eq!(replay, b"456789XYZ");
        assert_eq!(seqs, vec![4, 10], "回放与实时必须按累计偏移连续");
    }

    /// #72：窗口外退化为快照；窗口内精确回放；正好在末尾则什么都不用补。
    #[test]
    fn attach_falls_back_to_snapshot_outside_the_ring() {
        let table = SessionTable::new();
        table.insert(session_with_ring("s", "inc-1", 4));
        PumpFixture::feed(&table, b"abcdefghij");

        let snapshot_out = outbound();
        let snapshot = table
            .attach("s", 2, 1, snapshot_out.clone())
            .expect("attach");
        assert!(snapshot.snapshot.is_some(), "旧位置必须退化为快照");
        assert_eq!(snapshot.from_seq, 10);
        assert_eq!(snapshot.replay_end, 10);
        assert!(snapshot.snapshot.as_deref().unwrap().contains("abcdefghij"));
        assert!(drain(&snapshot_out).is_empty(), "快照路径不应再发旧字节");

        let replay_out = outbound();
        let replay = table.attach("s", 8, 1, replay_out.clone()).expect("attach");
        assert!(replay.snapshot.is_none());
        assert_eq!(replay.from_seq, 8);
        let bytes: Vec<u8> = drain(&replay_out)
            .into_iter()
            .flat_map(|message| match message {
                Message::Data { bytes, .. } => bytes,
                _ => Vec::new(),
            })
            .collect();
        assert_eq!(bytes, b"ij");

        let current_out = outbound();
        let current = table
            .attach("s", 10, 1, current_out.clone())
            .expect("attach");
        assert!(current.snapshot.is_none());
        assert_eq!(current.from_seq, 10);
        assert_eq!(current.replay_end, 10);
        assert!(drain(&current_out).is_empty());
    }

    /// #74：内存环滚掉后，`Attach(old_last_seq)` 仍能从落盘日志精确续放。
    #[test]
    fn attach_replays_detached_output_from_disk_after_the_ring_rolled() {
        let root = std::env::temp_dir().join(format!(
            "ageminal-session-history-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store = HistoryStore::new(root.clone());
        let table = SessionTable::new();
        table.insert(session_with_history("s", "inc-1", 4, &store));

        PumpFixture::feed(&table, b"0123456789");
        let outbound = outbound();
        let attached = table.attach("s", 2, 7, outbound.clone()).expect("attach");

        assert!(
            attached.snapshot.is_none(),
            "磁盘日志覆盖 last_seq 时不应退化为快照"
        );
        assert_eq!(attached.from_seq, 2);
        assert_eq!(attached.replay_end, 10);
        let bytes: Vec<u8> = drain(&outbound)
            .into_iter()
            .flat_map(|message| match message {
                Message::Data { bytes, .. } => bytes,
                _ => Vec::new(),
            })
            .collect();
        assert_eq!(bytes, b"23456789");

        drop(table);
        let _ = std::fs::remove_dir_all(root);
    }

    /// #72 验收 2：断档恢复走 checkpoint，快照与 seq 必须配对。
    #[test]
    fn checkpoint_sends_the_snapshot_at_the_current_offset() {
        let table = SessionTable::new();
        table.insert(session("s", "inc-1"));
        PumpFixture::feed(&table, b"hello");

        let outbound = outbound();
        let attached = table.attach("s", 5, 1, outbound.clone()).expect("attach");
        assert!(attached.snapshot.is_none());
        table.checkpoint("s").expect("checkpoint");

        let messages = drain(&outbound);
        let snapshot = messages
            .into_iter()
            .find_map(|message| match message {
                Message::Snapshot(snapshot) => Some(snapshot),
                _ => None,
            })
            .expect("应收到 Snapshot");
        assert_eq!(snapshot.seq, 5);
        assert!(snapshot.ansi.contains("hello"));
        assert!(!snapshot.alt_screen);
    }

    struct PumpFixture;

    impl PumpFixture {
        fn feed(table: &SessionTable, bytes: &[u8]) {
            let session = table.get("s").expect("session");
            let mut reader = std::io::Cursor::new(bytes.to_vec());
            pump_output(&session, &mut reader);
        }
    }

    /// #70：输出线程必须在转发之前把字节喂给服务端 VT 内核。
    #[test]
    fn output_pump_feeds_the_vt_kernel() {
        use alacritty_terminal::vte::ansi::{Color, NamedColor};

        let session = session("s", "inc-1");
        let mut reader = std::io::Cursor::new(b"\x1b[32mGREEN\x1b[0m vt\r\n".to_vec());

        pump_output(&session, &mut reader);

        let vt = session.vt.lock().expect("vt");
        assert!(
            vt.lines()[0].contains("GREEN vt"),
            "VT 内核应看到输出，实际：{:?}",
            vt.lines()
        );
        let first = vt.cell(0, 0).expect("cell");
        assert_eq!(first.character, 'G');
        assert_eq!(first.foreground, Color::Named(NamedColor::Green));

        // resize 也要同步到内核。
        drop(vt);
        session.vt.lock().expect("vt").resize(40, 12);
        assert_eq!(session.vt.lock().expect("vt").columns(), 40);
    }

    #[test]
    fn shell_arguments_follow_the_executable() {
        assert_eq!(
            shell_args(r"C:\Program Files\Git\bin\bash.exe"),
            vec!["--login", "-i"]
        );
        assert_eq!(shell_args("bash"), vec!["--login", "-i"]);
        assert_eq!(shell_args("pwsh.exe"), vec!["-NoLogo"]);
        assert_eq!(shell_args("powershell.exe"), vec!["-NoLogo"]);
        assert!(shell_args("cmd.exe").is_empty());
    }

    /// 默认 shell 必须解析到一个**真实存在**的可执行文件（CI 上有 Git for Windows）。
    #[test]
    fn default_shell_resolves_to_something_that_exists() {
        let (program, args) = default_shell();
        assert!(
            Path::new(&program).is_file() || program == "powershell.exe",
            "默认 shell 不存在：{program}"
        );
        assert!(!args.is_empty() || program == "cmd.exe");
    }

    #[test]
    fn create_session_spec_carries_cwd_size_and_env() {
        let request = CreateSession {
            sid: "s".to_owned(),
            cwd: r"C:\work".to_owned(),
            shell: Some("cmd.exe".to_owned()),
            command: None,
            cols: 100,
            rows: 40,
            env: vec![ageminal_protocol::EnvVar {
                name: "AGEMINAL_TOKEN".to_owned(),
                value: "t".to_owned(),
            }],
        };
        let spec = spec_from(&request);
        assert_eq!(spec.program, "cmd.exe");
        assert!(spec.args.is_empty());
        assert_eq!(
            spec.size,
            PtySize {
                cols: 100,
                rows: 40
            }
        );
        assert_eq!(
            spec.env,
            vec![("AGEMINAL_TOKEN".to_owned(), "t".to_owned())]
        );
    }
}
