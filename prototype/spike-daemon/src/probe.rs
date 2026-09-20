//! `probe`：前台、聚焦、把发生的一切都打出来。
//!
//! 真机上 selftest 只留下「首包 16 字节后就没有了」这一个事实，不足以定因。这个子命令
//! 把每个环节都摊开：子进程存活 / 退出码、收到的每一段字节（转义后可读）、
//! 以及 **resize 前后输出是否变化**（排掉「ConPTY 要 resize 才吐首屏」这个可能）。

use crate::conpty::{self, PtySpawn};
use std::time::{Duration, Instant};

/// 把**本进程**的 stdin/stdout/stderr 换成 NUL 设备。
/// 用它把「stdio 指向 NUL」与「daemon 是子进程」两件事分开。
#[cfg(windows)]
fn nul_stdio() {
    use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_GENERIC_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Console::{
        SetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };
    unsafe {
        let name: Vec<u16> = "NUL\0".encode_utf16().collect();
        let h: HANDLE = CreateFileW(
            name.as_ptr(),
            FILE_GENERIC_READ | FILE_GENERIC_WRITE,
            3, // FILE_SHARE_READ | FILE_SHARE_WRITE
            std::ptr::null_mut(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        );
        if h == INVALID_HANDLE_VALUE {
            println!("⚠️ 打开 NUL 失败：{:?}", std::io::Error::last_os_error());
            return;
        }
        SetStdHandle(STD_INPUT_HANDLE, h);
        SetStdHandle(STD_OUTPUT_HANDLE, h);
        SetStdHandle(STD_ERROR_HANDLE, h);
    }
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// 把控制字符转成可读形式，便于肉眼看协议
fn escape(b: &[u8]) -> String {
    let mut s = String::new();
    for &c in b {
        match c {
            b'\x1b' => s.push_str("ESC"),
            b'\r' => s.push_str("\\r"),
            b'\n' => s.push_str("\\n\n"),
            0x20..=0x7e => s.push(c as char),
            _ => s.push_str(&format!("\\x{c:02x}")),
        }
    }
    s
}

fn hex(b: &[u8]) -> String {
    b.iter().take(64).map(|c| format!("{c:02x} ")).collect()
}

pub fn run(shell: Option<String>, seconds: u64, resize: bool, nul_stdio_flag: bool) -> i32 {
    if nul_stdio_flag {
        #[cfg(windows)]
        nul_stdio();
    }
    let shell =
        shell.unwrap_or_else(|| std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into()));
    println!("== ConPTY 前台探针 ==");
    println!("shell      : {shell}");
    println!("观察时长   : {seconds}s    start-resize: {resize}    stdio=NUL: {nul_stdio_flag}");
    println!("⚠️ 请留意：**正常情况不应出现新的控制台窗口**（子进程应被伪控制台托管）");
    println!();

    let req = PtySpawn {
        shell: shell.clone(),
        cols: 120,
        rows: 30,
    };
    let (pty, mut out) = match conpty::spawn(&req) {
        Ok(v) => v,
        Err(e) => {
            println!("❌ spawn 失败：{e}");
            return 1;
        }
    };
    println!("✅ ConPTY + 子进程已创建：pid={}", pty.pid());
    let t0 = Instant::now();

    // 先把读线程跑起来（阻塞读）
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        use std::io::Read;
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match out.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    if resize {
        // 假设：ConPTY 需要一次 resize 才吐首屏
        match pty.resize(120, 31) {
            Ok(()) => println!("[{}ms] 已做一次 resize 120x31", t0.elapsed().as_millis()),
            Err(e) => println!("[{}ms] resize 失败：{e}", t0.elapsed().as_millis()),
        }
    }

    let mut total = 0usize;
    let mut chunks = 0usize;
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(data) => {
                total += data.len();
                chunks += 1;
                println!(
                    "[{}ms] 收到 {} 字节（第 {} 段）",
                    now_ms().saturating_sub(now_ms()),
                    data.len(),
                    chunks
                );
                println!("        hex: {}", hex(&data));
                println!("        esc: {}", escape(&data));
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // 每 ~1s 报一次存活
                let alive = pty.alive();
                println!(
                    "[{:>4}ms] 空转中… 子进程 alive={} exit={}",
                    t0.elapsed().as_millis(),
                    alive,
                    pty.exit_code()
                );
                if !alive {
                    println!();
                    println!("❌ 子进程已退出，退出码 = {:#010x}", pty.exit_code());
                    println!("   常见值：0xC0000142 = STATUS_DLL_INIT_FAILED（未能接到伪控制台）");
                    break;
                }
            }
            Err(_) => break,
        }
    }

    println!();
    println!("== 小结 ==");
    println!("  收到 {} 字节 / {} 段", total, chunks);
    println!(
        "  子进程 alive={} exit={:#010x}",
        pty.alive(),
        pty.exit_code()
    );
    if total <= 16 {
        println!("  ⚠️ 只收到 conhost 的开场序列，子进程没有产出任何东西");
    }
    let _ = pty.close();
    0
}
