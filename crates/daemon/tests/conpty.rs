//! ConPTY 集成测试（**真进程**，`#[cfg(windows)]`）。
//!
//! # 三条硬约束 ↔ 回归测试
//!
//! 三条约束都属「沉默失败」：写错了不报错，只是终端里什么都没有。对应关系：
//!
//! | 约束（`REQUIREMENTS.md` §5.1） | 谁盯着它 |
//! | --- | --- |
//! | ① `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE` 传**句柄值本身** | `conpty.rs` 的 `pseudo_console_attribute_value_is_the_handle_itself`（白盒）+ `runs_a_shell_and_carries_bytes_both_ways`（功能：传地址会另开控制台，pty 里收不到字节） |
//! | ② `CreatePipe` 两端在 `CreateProcess` **之后**才释放 | `runs_a_shell_and_carries_bytes_both_ways`（提前放会断掉 ConPTY 通道：shell 起不来，或一个字节都收发不到） |
//! | ③ `STARTF_USESTDHANDLES` + **三句柄 NULL** | `runs_a_shell_and_carries_bytes_both_ways` 的 `exit 7`（被破坏时 cmd 会立刻**干净退 0**，退不到 7） |
//!
//! 其余两个验收：resize 生效（`resize_takes_effect`）、terminate 杀掉整棵树
//! （`terminate_kills_the_whole_process_tree`）。

#![cfg(windows)]

use std::io::{Read, Write};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use ageminal_daemon::pty::{self, Pty, PtySize, PtySpec};
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess};

/// PTY 输出泵：把**阻塞读**搬到辅助线程，测试侧就能带超时地等标记而不是裸卡在 read 上。
struct Output {
    receiver: Receiver<Vec<u8>>,
    bytes: Vec<u8>,
    /// 线程句柄只用来表明归属；drop 即分离（读端关闭后线程自己结束）。
    _reader: thread::JoinHandle<()>,
}

impl Output {
    fn start(mut reader: Box<dyn Read + Send>) -> Self {
        let (sender, receiver) = mpsc::channel();
        let handle = thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        if sender.send(buffer[..read].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        Self {
            receiver,
            bytes: Vec::new(),
            _reader: handle,
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }

    /// 等到 `needle` 出现。
    fn wait_for(&mut self, needle: &str, timeout: Duration) -> bool {
        self.wait_until(timeout, |text| text.contains(needle).then_some(()))
            .is_some()
    }

    /// 等到出现「标记 + 数字」（标记本身也会被回显，所以要求**紧跟数字**）。
    fn wait_for_number(&mut self, prefix: &str, timeout: Duration) -> Option<u32> {
        self.wait_until(timeout, |text| scan_number(text, prefix))?;
        scan_number(&self.text(), prefix)
    }

    /// 反复收块直到 `matched` 为真或超时。
    fn wait_until<T>(
        &mut self,
        timeout: Duration,
        matched: impl Fn(&str) -> Option<T>,
    ) -> Option<()> {
        let deadline = Instant::now() + timeout;
        loop {
            if matched(&self.text()).is_some() {
                return Some(());
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            match self.receiver.recv_timeout(left) {
                Ok(chunk) => self.bytes.extend_from_slice(&chunk),
                Err(_) => return None,
            }
        }
    }
}

/// 扫「标记 + 一串数字」，跳过回显里那些不跟数字的标记。
fn scan_number(text: &str, prefix: &str) -> Option<u32> {
    for (index, _) in text.match_indices(prefix) {
        let rest = &text[index + prefix.len()..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(value) = digits.parse() {
            return Some(value);
        }
    }
    None
}

/// 取 `[tag]` 与 `[/tag]` 之间的第一段**能解析出尺寸**的内容。
///
/// 输入回显里也有一份 `[tag]`（紧跟着就是 `[/tag]`，中间没有数字），所以不能只找第一对。
fn size_between(text: &str, tag: &str) -> Option<(u16, u16)> {
    let open = format!("[{tag}]");
    let close = format!("[/{tag}]");
    let mut cursor = 0;
    while let Some(offset) = text[cursor..].find(&open) {
        let body_start = cursor + offset + open.len();
        let end = text[body_start..].find(&close)?;
        if let Some(size) = parse_console_size(&text[body_start..body_start + end]) {
            return Some(size);
        }
        cursor = body_start + end + close.len();
    }
    None
}

/// 从 `mode con` 的输出里读 (列, 行)。
///
/// `mode con` 的输出**是本地化**的（CI 上是英文、本机是中文），所以认两套标签：`Column`/`列` 与
/// `Line`/`行`。值取「冒号之后的第一串数字」——**不能要求整段都是数字**：ConPTY 在 resize 之后
/// 重绘屏幕时，每行尾部都会带 `\x1b[K`（erase-line），`"43\x1b[K"` 是解析不出来的（CI 上真踩过）。
///
/// 万一两套标签都没认出来（别的语言），按位置兜底：`mode con` 的字段顺序是 **Lines → Columns**。
fn parse_console_size(body: &str) -> Option<(u16, u16)> {
    let mut columns = None;
    let mut lines = None;
    for line in body.lines() {
        let lower = line.to_lowercase();
        let value = || number_after_colon(line);
        if columns.is_none() && (lower.contains("column") || line.contains('列')) {
            columns = value();
        } else if lines.is_none() && (lower.contains("line") || line.contains('行')) {
            lines = value();
        }
    }
    if let (Some(columns), Some(lines)) = (columns, lines) {
        return Some((columns, lines));
    }

    // 位置兜底：前两个数字依次是行数、列数。
    let numbers: Vec<u16> = body
        .lines()
        .filter_map(number_after_colon)
        .take(2)
        .collect();
    match numbers.as_slice() {
        [lines, columns] => Some((*columns, *lines)),
        _ => None,
    }
}

/// 取冒号之后的第一串数字（中英文冒号都认）。
fn number_after_colon(line: &str) -> Option<u16> {
    let tail = line.rsplit([':', '：']).next().unwrap_or(line);
    let start = tail.find(|character: char| character.is_ascii_digit())?;
    let digits: String = tail[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

fn cmd_spec(size: PtySize) -> PtySpec {
    PtySpec {
        program: String::from("cmd.exe"),
        // `/q`：关掉命令回显，输出里剩下的基本都是真结果。
        args: vec![String::from("/q")],
        cwd: std::env::temp_dir(),
        size,
        env: Vec::new(),
    }
}

fn await_exit(session: &mut dyn Pty, timeout: Duration) -> Option<u32> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = session.try_wait().expect("try_wait") {
            return Some(status.code);
        }
        if Instant::now() >= deadline {
            return None;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn is_alive(pid: u32) -> bool {
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;

    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return false;
    }
    let mut code: u32 = 0;
    let read = unsafe { GetExitCodeProcess(process, &mut code) };
    unsafe { CloseHandle(process) };
    read != 0 && code == STILL_ACTIVE
}

fn wait_until_dead(pid: u32, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !is_alive(pid) {
            return true;
        }
        thread::sleep(Duration::from_millis(50));
    }
    !is_alive(pid)
}

/// 验收 1「真实起 shell 能收发字节」——同时把约束 ①②③ 全部压住。
#[test]
fn runs_a_shell_and_carries_bytes_both_ways() {
    let cwd = std::env::temp_dir().join(format!("ageminal-pty-cwd-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cwd);
    std::fs::create_dir_all(&cwd).unwrap();

    let mut spec = cmd_spec(PtySize { cols: 80, rows: 24 });
    spec.cwd = cwd.clone();

    let mut session = pty::spawn(&spec).expect("cmd 应该起得来");
    assert_ne!(session.pid(), 0, "要拿到子进程 pid");

    let mut output = Output::start(session.take_reader().expect("输出流只能取一次"));
    let mut input = session.take_writer().expect("输入流只能取一次");
    assert!(
        session.take_reader().is_none() && session.take_writer().is_none(),
        "流只能取一次"
    );

    // 输出方向：`%CD%` 的结果里才有这个目录名，回显里只有 `echo %CD%` 这串字面量。
    input.write_all(b"echo %CD%\r\n").unwrap();
    input.flush().unwrap();
    let leaf = cwd.file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        output.wait_for(&leaf, Duration::from_secs(30)),
        "没收到 shell 的输出；已收到：{}",
        output.text()
    );

    // 输入方向 + 约束③：shell 真收到了字节并执行。约束③被破坏时 cmd 会立刻退 0，退不到 7。
    input.write_all(b"exit 7\r\n").unwrap();
    input.flush().unwrap();
    assert_eq!(
        await_exit(session.as_mut(), Duration::from_secs(20)),
        Some(7),
        "cmd 应执行 exit 7；已收到：{}",
        output.text()
    );

    let _ = std::fs::remove_dir_all(&cwd);
}

/// 验收 2「resize 生效」——让 shell 里的 `mode con` 报控制台尺寸。
///
/// 用 `mode con` 而不是 PowerShell 的 `[Console]::WindowWidth`：后者依赖 std 句柄，而我们的子进程
/// 三句柄都是 NULL（硬约束③）；`mode con` 是直接打开 `CONOUT$` 问控制台的，不受影响。
#[test]
fn resize_takes_effect() {
    let mut session =
        pty::spawn(&cmd_spec(PtySize { cols: 80, rows: 24 })).expect("cmd 应该起得来");
    let mut output = Output::start(session.take_reader().unwrap());
    let mut input = session.take_writer().unwrap();

    assert_eq!(
        probe_size(
            &mut output,
            input.as_mut(),
            "SIZE-1",
            Duration::from_secs(30)
        ),
        Some((80, 24)),
        "初始尺寸应是 80x24；已收到：{}",
        output.text()
    );

    session
        .resize(PtySize {
            cols: 132,
            rows: 43,
        })
        .expect("ResizePseudoConsole 应成功");

    assert_eq!(
        probe_size(
            &mut output,
            input.as_mut(),
            "SIZE-2",
            Duration::from_secs(30)
        ),
        Some((132, 43)),
        "resize 后应是 132x43；已收到：{}",
        output.text()
    );
}

/// 在 shell 里跑一次 `mode con`，用一对标记圈住它这段输出。
fn probe_size(
    output: &mut Output,
    input: &mut (dyn Write + Send),
    tag: &str,
    timeout: Duration,
) -> Option<(u16, u16)> {
    let command = format!("echo [{tag}] & mode con & echo [/{tag}]\r\n");
    input.write_all(command.as_bytes()).ok()?;
    input.flush().ok()?;
    output.wait_until(timeout, |text| size_between(text, tag))?;
    size_between(&output.text(), tag)
}

/// 验收 3「terminate 杀掉整棵进程树」。
#[test]
fn terminate_kills_the_whole_process_tree() {
    let mut session =
        pty::spawn(&cmd_spec(PtySize { cols: 80, rows: 24 })).expect("cmd 应该起得来");
    let mut output = Output::start(session.take_reader().unwrap());
    let mut input = session.take_writer().unwrap();

    // 在会话里再起一个**孙进程**：`-NoNewWindow` 让它留在同一个控制台上，才会被一起收走。
    input
        .write_all(
            b"powershell -NoProfile -NonInteractive -Command \"$p = Start-Process -NoNewWindow -PassThru ping -ArgumentList '-n','300','127.0.0.1'; Write-Output ('GC:' + $p.Id)\"\r\n",
        )
        .unwrap();
    input.flush().unwrap();

    let grandchild = output
        .wait_for_number("GC:", Duration::from_secs(40))
        .expect("孙进程没起来");
    assert!(is_alive(grandchild), "孙进程（pid {grandchild}）应该在跑");

    session.terminate().expect("terminate 应成功");
    assert!(session.terminate().is_ok(), "terminate 要幂等");

    assert!(
        await_exit(session.as_mut(), Duration::from_secs(15)).is_some(),
        "直接子进程应被终止；已收到：{}",
        output.text()
    );
    assert!(
        wait_until_dead(grandchild, Duration::from_secs(15)),
        "孙进程（pid {grandchild}）没被一起收走"
    );

    // 终止后还能不能再 resize？答案必须是否定的（会话已经没了）。
    assert!(
        session.resize(PtySize { cols: 10, rows: 10 }).is_err(),
        "终止后的 resize 应报错"
    );
}

/// 环境变量注入（`PtySpec.env`）要真的到子进程里。
#[test]
fn injected_environment_reaches_the_child() {
    let mut spec = cmd_spec(PtySize { cols: 80, rows: 24 });
    spec.env
        .push((String::from("AGEMINAL_PTY_TEST"), String::from("hello")));

    let mut session = pty::spawn(&spec).expect("cmd 应该起得来");
    let mut output = Output::start(session.take_reader().unwrap());
    let mut input = session.take_writer().unwrap();

    // `%VAR%` 回显里是 `echo %AGEMINAL_PTY_TEST%`，真值只在结果里。
    input
        .write_all(b"echo ENV[%AGEMINAL_PTY_TEST%]\r\n")
        .unwrap();
    input.flush().unwrap();

    assert!(
        output.wait_for("ENV[hello]", Duration::from_secs(30)),
        "注入的环境变量没到子进程；已收到：{}",
        output.text()
    );

    let _ = session.terminate();
}

/// 解析器的回归锁。
///
/// 形状**逐字取自 CI 日志**：resize 之后 ConPTY 会重绘屏幕，每行尾部都带 `\x1b[K`
/// （erase-line）——第一版解析器就是被它坑的（`"43\x1b[K".parse()` 失败，断言拿到 `None`）。
#[test]
fn parses_mode_con_output_with_erase_sequences_and_both_locales() {
    let english = concat!(
        "\r\nStatus for device CON:\r\n",
        "----------------------\r\n",
        "    Lines:          43\u{1b}[K\r\n",
        "    Columns:        132\u{1b}[K\r\n",
        "    Keyboard rate:  31\u{1b}[K\r\n",
        "    Keyboard delay: 1\u{1b}[K\r\n",
        "    Code page:      437\u{1b}[K\r\n",
    );
    assert_eq!(parse_console_size(english), Some((132, 43)));

    let chinese = concat!(
        "\r\n设备 CON 的状态:\r\n",
        "----------------------\r\n",
        "    行:             43\r\n",
        "    列:            132\r\n",
        "    键盘速率:        31\r\n",
    );
    assert_eq!(parse_console_size(chinese), Some((132, 43)));

    // 标签完全不认识（别的语言）：按位置兜底——`mode con` 的顺序是 Lines → Columns。
    let positional = "X:\r\n  1: 43\r\n  2: 132\r\n";
    assert_eq!(parse_console_size(positional), Some((132, 43)));
}
