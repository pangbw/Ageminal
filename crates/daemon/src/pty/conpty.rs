//! Windows ConPTY 实现（issue #66）。
//!
//! # 三条硬约束
//!
//! 全部来自 `REQUIREMENTS.md` §5.1（#40 真机实测，已与 MS 官方样例 / `portable-pty` 一手实现
//! 核对）。**每一条都是「沉默失败」型**：写错了不报错，只是终端里什么都没有。
//!
//! 1. [`UpdateProcThreadAttribute`] 的 `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE`，`lpValue` 必须是
//!    **句柄值本身**，不是 `&hpc`。传地址会让 Windows **另开一个控制台窗口**，子进程跑到那个
//!    控制台上——pty 管道里一个字节都收不到。
//! 2. [`CreatePipe`] 的两端要等 **`CreateProcessW` 之后**才释放。
//! 3. 子进程必须显式 `STARTF_USESTDHANDLES` + **三句柄 NULL**。否则 `CreateProcessW` 会把
//!    daemon 的 NUL std handle 复制给子进程 → `cmd` 读 stdin 立刻 EOF → **干净退 0**
//!    （只有一段开场序列，没有任何错误）。
//!
//! 回归测试的对应关系写在 `tests/conpty.rs` 顶部。
//!
//! # 关闭顺序
//!
//! `ClosePseudoConsole` 会**终止整棵进程树**，但它要等 conout 排空——所以调用
//! [`Pty::terminate`] 时，输出流必须已经取走并有人持续在读，否则会卡住
//! （`docs/research/zed-terminal.md` §3.2）。

use std::ffi::c_void;
use std::fs::File;
use std::io::{Read, Write};
use std::mem::size_of;
use std::os::windows::io::FromRawHandle;
use std::ptr;

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::System::Console::{
    ClosePseudoConsole, CreatePseudoConsole, ResizePseudoConsole, COORD, HPCON,
};
use windows_sys::Win32::System::Environment::{FreeEnvironmentStringsW, GetEnvironmentStringsW};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, GetExitCodeProcess,
    InitializeProcThreadAttributeList, UpdateProcThreadAttribute, WaitForSingleObject,
    CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT, INFINITE, PROCESS_INFORMATION,
    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, STARTF_USESTDHANDLES, STARTUPINFOEXW,
};

use super::{ExitStatus, Pty, PtyError, PtySize, PtySpec};

/// 建伪控制台时**不开任何 flag**。
///
/// - `PSEUDOCONSOLE_INHERIT_CURSOR` 是把父控制台的光标位置抄给子会话——daemon 没有父控制台。
/// - `PSEUDOCONSOLE_RESIZE_QUIRK` 是给「不支持 resize 的老程序」打的兼容补丁，我们不需要。
/// - `PSEUDOCONSOLE_WIN32_INPUT_MODE` 会把输入按 Win32 控制台语义翻译；我们要的是 **VT 输入**
///   （xterm.js 发上来的是原始字节），所以不能开。
const PSEUDOCONSOLE_FLAGS: u32 = 0;

/// 起一个 ConPTY 会话。
pub fn spawn(spec: &PtySpec) -> Result<Box<dyn Pty>, PtyError> {
    let size = coord(spec.size);

    // 两对匿名管道：
    //   conin ：我们写 → 控制台读（键盘输入）
    //   conout：控制台写 → 我们读（PTY 输出）
    let (conin_read, conin_write) = create_pipe()?;
    let (conout_read, conout_write) = create_pipe()?;

    let mut hpc: HPCON = 0;
    let result = unsafe {
        CreatePseudoConsole(
            size,
            conin_read,
            conout_write,
            PSEUDOCONSOLE_FLAGS,
            &mut hpc,
        )
    };
    if result < 0 {
        close_handles(&[conin_read, conin_write, conout_read, conout_write]);
        return Err(PtyError::Windows {
            call: "CreatePseudoConsole",
            reason: format!("HRESULT=0x{:08X}", result as u32),
        });
    }

    let child = spawn_child(hpc, spec);

    // **约束②**：`CreateProcessW` 已经返回，这时候才能放掉控制台侧的两端。
    // 提前放会断掉 ConPTY 的通道（子进程起来也收不到/发不出字节）。
    close_handles(&[conin_read, conout_write]);

    let (process, pid) = match child {
        Ok(child) => child,
        Err(error) => {
            // 先关控制台，再放剩下的管道：反过来可能撞上「排空等待」而卡住。
            unsafe { ClosePseudoConsole(hpc) };
            close_handles(&[conin_write, conout_read]);
            return Err(error);
        }
    };

    Ok(Box::new(ConPty {
        hpc,
        process,
        pid,
        // SAFETY：这两个句柄的所有权从此刻起归 `File`；`ConPty` 只持有 HPCON 与进程句柄。
        reader: Some(Box::new(unsafe { File::from_raw_handle(conout_read) })),
        writer: Some(Box::new(unsafe { File::from_raw_handle(conin_write) })),
        terminated: false,
    }))
}

/// ConPTY 会话。
struct ConPty {
    /// 持有它 = 持有终端；`ClosePseudoConsole` 即终止整棵进程树。
    hpc: HPCON,
    /// 子进程句柄（等退出用）。
    process: HANDLE,
    /// 子进程 pid。
    pid: u32,
    /// conout 读端（取走后归调用方）。
    reader: Option<Box<dyn Read + Send>>,
    /// conin 写端（取走后归调用方）。
    writer: Option<Box<dyn Write + Send>>,
    /// 是否已关过控制台（让 `terminate` 幂等、`Drop` 不重复关）。
    terminated: bool,
}

// SAFETY：`HANDLE` 是裸指针，只是**进程级句柄表里的索引**，内核对象本身不属于任何线程。
// 把会话的所有权移交给另一个线程是安全的（与 `std::fs::File` 同理）；并发访问仍受 `&mut self`
// 约束（`resize` / `pid` 是只读，`terminate` / `wait` 要独占）。
unsafe impl Send for ConPty {}

impl Pty for ConPty {
    fn pid(&self) -> u32 {
        self.pid
    }

    fn resize(&self, size: PtySize) -> Result<(), PtyError> {
        if self.terminated {
            return Err(PtyError::Terminated);
        }
        let result = unsafe { ResizePseudoConsole(self.hpc, coord(size)) };
        if result < 0 {
            return Err(PtyError::Windows {
                call: "ResizePseudoConsole",
                reason: format!("HRESULT=0x{:08X}", result as u32),
            });
        }
        Ok(())
    }

    fn take_reader(&mut self) -> Option<Box<dyn Read + Send>> {
        self.reader.take()
    }

    fn take_writer(&mut self) -> Option<Box<dyn Write + Send>> {
        self.writer.take()
    }

    fn wait(&mut self) -> Result<ExitStatus, PtyError> {
        if unsafe { WaitForSingleObject(self.process, INFINITE) } != WAIT_OBJECT_0 {
            return Err(windows_error("WaitForSingleObject"));
        }
        exit_status(self.process)
    }

    fn try_wait(&mut self) -> Result<Option<ExitStatus>, PtyError> {
        match unsafe { WaitForSingleObject(self.process, 0) } {
            WAIT_OBJECT_0 => exit_status(self.process).map(Some),
            WAIT_TIMEOUT => Ok(None),
            _ => Err(windows_error("WaitForSingleObject")),
        }
    }

    fn terminate(&mut self) -> Result<(), PtyError> {
        if self.terminated {
            return Ok(());
        }
        self.terminated = true;
        unsafe { ClosePseudoConsole(self.hpc) };
        // 句柄已经关了，别再拿着这个值去调 API。
        self.hpc = 0;
        Ok(())
    }
}

impl Drop for ConPty {
    fn drop(&mut self) {
        // 兜底：谁忘了 terminate，也别把会话泄漏在后台。
        let _ = self.terminate();
        unsafe { CloseHandle(self.process) };
    }
}

/// 建属性列表 → 挂上伪控制台 → `CreateProcessW`。返回 `(进程句柄, pid)`。
fn spawn_child(hpc: HPCON, spec: &PtySpec) -> Result<(HANDLE, u32), PtyError> {
    // 属性列表的大小要先问一次：用 NULL 调用必然失败（`ERROR_INSUFFICIENT_BUFFER`），
    // 但 `*size` 会被填上需要多少字节——这是官方规定的用法。
    let mut attribute_bytes: usize = 0;
    unsafe { InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &mut attribute_bytes) };
    if attribute_bytes == 0 {
        return Err(windows_error("InitializeProcThreadAttributeList"));
    }

    // 用 `Vec<usize>` 而不是 `Vec<u8>`：属性列表要求**指针宽度对齐**，`u8` 只有 1 字节对齐。
    let mut storage = vec![0usize; attribute_bytes.div_ceil(size_of::<usize>())];
    let attributes = storage.as_mut_ptr().cast::<c_void>();
    if unsafe { InitializeProcThreadAttributeList(attributes, 1, 0, &mut attribute_bytes) } == 0 {
        return Err(windows_error("InitializeProcThreadAttributeList"));
    }

    // **约束①**：`lpValue` 传句柄值本身（不是 `&hpc`）。
    let updated = unsafe {
        UpdateProcThreadAttribute(
            attributes,
            0,
            PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
            pseudo_console_attribute_value(hpc),
            size_of::<HPCON>(),
            ptr::null_mut(),
            ptr::null(),
        )
    };
    if updated == 0 {
        let error = windows_error("UpdateProcThreadAttribute");
        unsafe { DeleteProcThreadAttributeList(attributes) };
        return Err(error);
    }

    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.lpAttributeList = attributes;
    // **约束③**：显式 `STARTF_USESTDHANDLES` + 三句柄 NULL，见模块文档。
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = ptr::null_mut();
    startup.StartupInfo.hStdOutput = ptr::null_mut();
    startup.StartupInfo.hStdError = ptr::null_mut();

    let mut command_line = to_wide(&command_line(&spec.program, &spec.args));
    let current_directory = to_wide(&spec.cwd.to_string_lossy());
    let environment = environment_block(&spec.env)?;

    let mut info = PROCESS_INFORMATION::default();
    let created = unsafe {
        CreateProcessW(
            ptr::null(),
            command_line.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            0, // bInheritHandles：三句柄都是 NULL，没有要继承的东西。
            EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
            environment.as_ptr().cast::<c_void>(),
            current_directory.as_ptr(),
            &startup.StartupInfo,
            &mut info,
        )
    };

    // `GetLastError` 要在别的 API 调用之前读走（后面那个 Delete 可能把它冲掉）。
    let error = if created == 0 {
        Some(unsafe { GetLastError() })
    } else {
        None
    };
    unsafe { DeleteProcThreadAttributeList(attributes) };
    drop(storage);

    match error {
        Some(code) => Err(PtyError::Windows {
            call: "CreateProcessW",
            reason: format!("GetLastError={code}"),
        }),
        None => {
            unsafe { CloseHandle(info.hThread) };
            Ok((info.hProcess, info.dwProcessId))
        }
    }
}

/// 硬约束①的实现点：把 HPCON **当作值**塞进指针槽位。
///
/// 单独抽成函数是为了能被回归测试盯住（见 `tests/conpty.rs`）。
fn pseudo_console_attribute_value(hpc: HPCON) -> *const c_void {
    hpc as *const c_void
}

/// `PtySize` → `COORD`。ConPTY 不接受 0，钳到 `[1, i16::MAX]`。
fn coord(size: PtySize) -> COORD {
    let clamp = |value: u16| value.clamp(1, i16::MAX as u16) as i16;
    COORD {
        X: clamp(size.cols),
        Y: clamp(size.rows),
    }
}

/// 建一对匿名管道。`nsize = 0` 走系统默认缓冲（与 `portable-pty` 一致）。
fn create_pipe() -> Result<(HANDLE, HANDLE), PtyError> {
    let mut read: HANDLE = ptr::null_mut();
    let mut write: HANDLE = ptr::null_mut();
    if unsafe { CreatePipe(&mut read, &mut write, ptr::null(), 0) } == 0 {
        return Err(windows_error("CreatePipe"));
    }
    Ok((read, write))
}

/// 关一组句柄（NULL 跳过）。
fn close_handles(handles: &[HANDLE]) {
    for &handle in handles {
        if !handle.is_null() {
            unsafe { CloseHandle(handle) };
        }
    }
}

/// 读进程退出码。
fn exit_status(process: HANDLE) -> Result<ExitStatus, PtyError> {
    let mut code: u32 = 0;
    if unsafe { GetExitCodeProcess(process, &mut code) } == 0 {
        return Err(windows_error("GetExitCodeProcess"));
    }
    Ok(ExitStatus { code })
}

/// 取 `GetLastError` 并包成 [`PtyError`]。
fn windows_error(call: &'static str) -> PtyError {
    PtyError::Windows {
        call,
        reason: format!("GetLastError={}", unsafe { GetLastError() }),
    }
}

/// 拼成以 NUL 结尾的 UTF-16。
fn to_wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 拼命令行（`CreateProcessW` 的 `lpCommandLine`）。
fn command_line(program: &str, args: &[String]) -> String {
    let mut line = String::new();
    append_argument(&mut line, program);
    for arg in args {
        line.push(' ');
        append_argument(&mut line, arg);
    }
    line
}

/// 按 `CommandLineToArgvW` 的反解析规则追加一个参数。
///
/// 三件事：需要时整体加引号；引号内的 `"` 用 `\"` 转义、紧邻引号的反斜杠要翻倍；结尾的反斜杠
/// 也要翻倍，否则会把**收尾**引号转义掉（`"C:\a b\"` 会被解析成未闭合）。
fn append_argument(line: &mut String, argument: &str) {
    let needs_quotes = argument.is_empty() || argument.contains([' ', '\t', '"']);

    if !needs_quotes {
        line.push_str(argument);
        return;
    }

    line.push('"');
    let mut backslashes = 0usize;
    for character in argument.chars() {
        match character {
            '\\' => backslashes += 1,
            '"' => {
                // 2n 个反斜杠 + 引号 → n 个反斜杠 + 转义引号。
                line.extend(std::iter::repeat('\\').take(backslashes * 2 + 1));
                line.push('"');
                backslashes = 0;
            }
            other => {
                line.extend(std::iter::repeat('\\').take(backslashes));
                line.push(other);
                backslashes = 0;
            }
        }
    }
    // 结尾反斜杠若紧贴收尾引号，必须翻倍。
    line.extend(std::iter::repeat('\\').take(backslashes * 2));
    line.push('"');
}

/// 构造 `CREATE_UNICODE_ENVIRONMENT` 要的环境块：继承 daemon 的环境 + `overrides`，
/// 按 Windows 的要求**按名字排序**、**双 NUL 结尾**。
fn environment_block(overrides: &[(String, String)]) -> Result<Vec<u16>, PtyError> {
    let mut entries = inherited_environment()?;

    for (name, value) in overrides {
        // Windows 的环境变量名**不区分大小写**：覆盖时要连不同大小写的同名项一起删掉。
        entries.retain(|(existing, _)| !existing.eq_ignore_ascii_case(name));
        entries.push((name.clone(), value.clone()));
    }

    // 官方要求按名字排序（不区分大小写），否则 `CreateProcessW` 建出来的块不符合约定。
    entries.sort_by_key(|(name, _)| name.to_uppercase());

    let mut block = Vec::new();
    for (name, value) in entries {
        block.extend(name.encode_utf16());
        block.push(u16::from(b'='));
        block.extend(value.encode_utf16());
        block.push(0);
    }
    block.push(0); // 双 NUL 结尾。
    Ok(block)
}

/// 读 daemon 自己的环境，拆成 `(名字, 值)`。
fn inherited_environment() -> Result<Vec<(String, String)>, PtyError> {
    let block = unsafe { GetEnvironmentStringsW() };
    if block.is_null() {
        return Err(windows_error("GetEnvironmentStringsW"));
    }

    let mut entries = Vec::new();
    let mut cursor = block;
    loop {
        let length = unsafe { (0..).take_while(|offset| *cursor.add(*offset) != 0).count() };
        if length == 0 {
            break;
        }
        let line = unsafe { std::slice::from_raw_parts(cursor, length) };
        if let Some((name, value)) = String::from_utf16_lossy(line).split_once('=') {
            // `=C:=C:\...` 这类伪变量名字是空串，跳过。
            if !name.is_empty() {
                entries.push((name.to_owned(), value.to_owned()));
            }
        }
        cursor = unsafe { cursor.add(length + 1) };
    }
    unsafe { FreeEnvironmentStringsW(block) };

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 硬约束①的回归测试：`lpValue` 必须是**句柄值**，不能是它的地址。
    ///
    /// 传地址的后果不是报错，而是 Windows 另开一个控制台窗口——pty 里一个字节都收不到。
    #[test]
    fn pseudo_console_attribute_value_is_the_handle_itself() {
        let handle: HPCON = 0x1234_5678;
        let value = pseudo_console_attribute_value(handle);

        assert_eq!(value as usize, handle as usize, "必须传值本身");
        assert_ne!(
            value as *const HPCON as usize,
            (&handle as *const HPCON) as usize,
            "绝不能传 &handle"
        );
    }

    #[test]
    fn command_line_quotes_like_command_line_to_argv_w() {
        let args = |items: &[&str]| {
            items
                .iter()
                .map(|item| item.to_string())
                .collect::<Vec<_>>()
        };

        assert_eq!(command_line("cmd.exe", &[]), "cmd.exe");
        assert_eq!(
            command_line(
                r"C:\Program Files\Git\bin\bash.exe",
                &args(&["--login", "-i"])
            ),
            r#""C:\Program Files\Git\bin\bash.exe" --login -i"#
        );
        // 空格 → 加引号。
        assert_eq!(command_line("x", &args(&["a b"])), r#"x "a b""#);
        // 空参数 → 一个空引号对。
        assert_eq!(command_line("x", &args(&[""])), r#"x """#);
        // 引号本身 → `\"`。
        assert_eq!(command_line("x", &args(&[r#"a"b"#])), r#"x "a\"b""#);
        // 结尾反斜杠必须翻倍，否则会转义掉收尾引号。
        assert_eq!(
            command_line("x", &args(&[r"C:\dir with space\"])),
            r#"x "C:\dir with space\\""#
        );
        // 制表符也算需要引号。
        assert_eq!(command_line("x", &args(&["a\tb"])), "x \"a\tb\"");
        // 纯反斜杠且无空格 → 不用引号，也不转义。
        assert_eq!(command_line("x", &args(&[r"C:\dir\"])), r"x C:\dir\");
    }

    #[test]
    fn environment_block_overrides_case_insensitively_and_ends_with_two_nuls() {
        let block =
            environment_block(&[(String::from("AgeminalTestVar"), String::from("v1"))]).unwrap();
        assert_eq!(block.last(), Some(&0));
        assert_eq!(
            block.get(block.len().wrapping_sub(2)),
            Some(&0),
            "必须双 NUL 结尾"
        );

        // 再塞一个同名但大小写不同的值：应该只剩后者。
        let overridden = environment_block(&[
            (String::from("AgeminalTestVar"), String::from("v1")),
            (String::from("AGEMINALTESTVAR"), String::from("v2")),
        ])
        .unwrap();
        let pairs = parse_block(&overridden);
        let hits: Vec<_> = pairs
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("AgeminalTestVar"))
            .collect();
        assert_eq!(hits.len(), 1, "同名项必须只剩一个：{hits:?}");
        assert_eq!(hits[0].1, "v2", "后写的覆盖先写的");

        // 排序：逐个比较相邻项。
        let names: Vec<String> = pairs.iter().map(|(name, _)| name.to_uppercase()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "环境块必须按名字排序");
    }

    #[test]
    fn coordinates_are_clamped_into_a_valid_range() {
        let normal = coord(PtySize { cols: 80, rows: 24 });
        assert_eq!((normal.X, normal.Y), (80, 24));

        let zero = coord(PtySize { cols: 0, rows: 0 });
        assert_eq!((zero.X, zero.Y), (1, 1), "ConPTY 不接受 0");

        let huge = coord(PtySize {
            cols: u16::MAX,
            rows: u16::MAX,
        });
        assert_eq!((huge.X, huge.Y), (i16::MAX, i16::MAX));
    }

    /// 把双 NUL 结尾的块拆回 `(名字, 值)`（测试用）。
    fn parse_block(block: &[u16]) -> Vec<(String, String)> {
        let mut pairs = Vec::new();
        let mut start = 0;
        while start < block.len() && block[start] != 0 {
            let end = block[start..]
                .iter()
                .position(|&unit| unit == 0)
                .map(|offset| start + offset)
                .unwrap();
            let line = String::from_utf16_lossy(&block[start..end]);
            if let Some((name, value)) = line.split_once('=') {
                pairs.push((name.to_owned(), value.to_owned()));
            }
            start = end + 1;
        }
        pairs
    }
}
