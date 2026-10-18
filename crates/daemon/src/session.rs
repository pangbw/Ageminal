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
//! 区分「从未存在 / 离开期间退了」需要更长的持久化历史，归 #74。
//!
//! # 流控
//!
//! `Ack` 只记账（[`SessionTable::ack`]）；高低水位暂停发送 / 停读 PTY 归 #73。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};

use ageminal_protocol::{
    Attached, CreateSession, Message, SessionCreated, SessionExited, Snapshot,
};

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

/// 一个会话（跨连接存活）。
struct Session {
    /// 逻辑会话 id。
    sid: String,
    /// 本次 spawn 的世代。**迟到 exit 靠它判**。
    incarnation_id: String,
    /// PTY 控制端（resize / terminate）。**可取走**：`terminate` 把 PTY 交给独立线程去关，
    /// 免得 `ClosePseudoConsole` 等 conout 排空时卡住连接线程。
    control: Mutex<Option<Box<dyn Pty>>>,
    /// 只用于「等退出」的进程句柄——**不经 `control` 锁**，否则 terminate 一卡，退出事件也发不出来。
    exit: Option<OwnedHandle>,
    /// PTY 写端（键盘输入）。
    writer: Mutex<Box<dyn Write + Send>>,
    /// 客户端已确认消费的字节数（#73 的水位输入）。
    acked_bytes: AtomicU64,
    /// 服务端 VT 内核（#70）：屏幕 / 光标 / alt-screen / SGR。
    vt: Mutex<Vt>,
    /// 输出偏移 / 回放窗口 / 当前连接（#72）。
    output: Mutex<OutputState>,
}

/// daemon 全局的会话表。
#[derive(Default)]
pub struct SessionTable {
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    next_incarnation: AtomicU64,
}

impl SessionTable {
    /// 建空表。
    pub fn new() -> Self {
        Self::default()
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
        let session = Arc::new(Session {
            sid: request.sid.clone(),
            incarnation_id: incarnation_id.clone(),
            control: Mutex::new(Some(control)),
            exit: Some(exit),
            writer: Mutex::new(writer),
            acked_bytes: AtomicU64::new(0),
            vt: Mutex::new(Vt::new(request.cols as usize, request.rows as usize)),
            output: Mutex::new(OutputState {
                seq: 0,
                ring: RingBuffer::new(OUTPUT_RING_BYTES),
                attachment: Some(Attachment { handle, outbound }),
            }),
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
        let current = output.seq;
        let alt_screen = vt.alt_screen();

        let (snapshot, from_seq, replay) = if last_seq > 0 && last_seq <= current {
            match output.ring.since(last_seq) {
                Some(bytes) => (None, last_seq, bytes),
                None => (Some(vt.snapshot()), current, Vec::new()),
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

        Ok(Attached {
            sid: sid.to_owned(),
            handle,
            from_seq,
            replay_end: current,
            snapshot,
            alt_screen,
        })
    }

    /// 断开（**≠ 终止**）：会话继续活着，只是不再往任何连接推输出。
    pub fn detach(&self, sid: &str) {
        if let Some(session) = self.sessions.lock().expect("sessions").get(sid) {
            session.output.lock().expect("output").attachment = None;
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
            return Ok(()); // 已经终止过：幂等。
        };
        std::thread::spawn(move || {
            let _ = pty.terminate();
            drop(pty);
        });
        Ok(())
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

    /// 记下客户端已消费的字节数（流控归 #73）。
    pub fn ack(&self, sid: &str, acked_bytes: u64) -> Result<(), DaemonError> {
        let session = self.get(sid)?;
        session.acked_bytes.store(acked_bytes, Ordering::SeqCst);
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
        let read = match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        // 服务端状态必须先于转发更新，且要与 attach 的「快照 + seq」共用一个原子边界。
        // 锁顺序固定为 VT → output（attach / checkpoint 同序）。
        let mut vt = session.vt.lock().expect("vt");
        vt.feed(&buffer[..read]);
        let mut output = session.output.lock().expect("output");
        let seq = output.seq;
        output.seq += read as u64;
        output.ring.push(seq, &buffer[..read]);
        let attachment = output.attachment.clone();
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
                    if table.remove_if_current(&session.sid, &session.incarnation_id) {
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
            control: Mutex::new(Some(Box::new(FakePty))),
            exit: None,
            writer: Mutex::new(Box::new(std::io::sink())),
            acked_bytes: AtomicU64::new(0),
            vt: Mutex::new(Vt::new(80, 24)),
            output: Mutex::new(OutputState {
                seq: 0,
                ring: RingBuffer::new(ring_bytes),
                attachment: None,
            }),
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
