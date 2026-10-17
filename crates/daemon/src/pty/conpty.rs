//! Windows ConPTY 实现（issue #66）。
//!
//! # 三条硬约束
//!
//! 全部来自 `REQUIREMENTS.md` §5.1（#40 真机实测，已与 MS 官方样例 / `portable-pty` 一手实现
//! 核对）。三条都**不靠返回值报错**——写错了 `CreateProcessW` 往往照样成功，只是行为不对：
//!
//! 1. [`UpdateProcThreadAttribute`] 的 `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE`，`lpValue` 必须是
//!    **句柄值本身**，不是 `&hpc`。传地址会让 Windows **另开一个控制台窗口**，子进程跑到那个
//!    控制台上——pty 管道里一个字节都收不到。
//! 2. [`CreatePipe`] 的两端要等 **`CreateProcessW` 之后**才释放，否则 ConPTY 的通道是断的。
//! 3. 子进程必须显式 `STARTF_USESTDHANDLES` + **三句柄 NULL**。否则 `CreateProcessW` 会把
//!    daemon 的 NUL std handle 复制给子进程 → `cmd` 读 stdin 立刻 EOF → **干净退 0**
//!    （只有一段开场序列，没有任何错误）——三条里最阴的一条。
//!
//! 回归测试的对应关系写在 `tests/conpty.rs` 顶部。
//!
//! # 关闭顺序
//!
//! `ClosePseudoConsole` 会**终止整棵进程树**，但它要等 conout 排空；MS 文档的原话是
//! 「failure to either close or drain the output pipe may cause `ClosePseudoConsole` to wait
//! indefinitely」。所以调用 [`Pty::terminate`] 时，输出流必须已经取走并有人持续在读。
//! [`Drop`] 只能对**还没被取走**的那一半兜底（自己起线程排空）；取走的那一半归调用方。

use std::ffi::c_void;
use std::fs::File;
use std::io::{Read, Write};
use std::mem::size_of;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr;

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::System::Console::{
    ClosePseudoConsole, CreatePseudoConsole, ResizePseudoConsole, COORD, HPCON,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, GetExitCodeProcess,
    InitializeProcThreadAttributeList, TerminateProcess, UpdateProcThreadAttribute,
    WaitForSingleObject, CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT, INFINITE,
    PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, STARTF_USESTDHANDLES, STARTUPINFOEXW,
};

use super::wincmd::{command_line, environment_block};
use super::{windows_error, ExitStatus, Pty, PtyError, PtySize, PtySpec};

/// 关控制台之后等进程退出的宽限；超时就强杀。
const TERMINATE_GRACE: u32 = 2_000;

/// 建伪控制台时**不开任何 flag**。
///
/// - `PSEUDOCONSOLE_INHERIT_CURSOR` 是把父控制台的光标位置抄给子会话——daemon 是
///   `DETACHED_PROCESS`（§5.1），没有父控制台。
/// - `PSEUDOCONSOLE_RESIZE_QUIRK` 是给「不支持 resize 的老程序」打的兼容补丁；本票验收只要
///   「resize 生效」，真遇到老程序再说。
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
    let mut hpc = Hpcon(hpc);

    let child = spawn_child(hpc.get(), spec);

    // **约束②**：`CreateProcessW` 已经返回，这时候才能放掉控制台侧的两端。
    // 提前放会断掉 ConPTY 的通道（子进程起来也收不到/发不出字节）。
    close_handles(&[conin_read, conout_write]);

    let (process, pid) = match child {
        Ok(child) => child,
        Err(error) => {
            // 先关控制台，再放剩下的管道：反过来可能撞上「排空等待」而卡住。
            hpc.close();
            close_handles(&[conin_write, conout_read]);
            return Err(error);
        }
    };

    Ok(Box::new(ConPty {
        hpc,
        process,
        pid,
        // SAFETY：这两个句柄的所有权从此刻起归 `File`。
        reader: Some(Box::new(unsafe { File::from_raw_handle(conout_read) })),
        writer: Some(Box::new(unsafe { File::from_raw_handle(conin_write) })),
    }))
}

/// 伪控制台句柄的薄包装：`close()` 幂等，`Drop` 兜底再关一次。
///
/// 单独建类型是为了让所有权显式——`HPCON` 只是个 `isize` 句柄值，裸放着很容易漏关，也容易被
/// 当成普通整数传错。
struct Hpcon(isize);

impl Hpcon {
    /// 关掉伪控制台（= 终止整棵进程树）。幂等。
    fn close(&mut self) {
        if self.0 != 0 {
            unsafe { ClosePseudoConsole(self.0) };
            self.0 = 0;
        }
    }

    fn get(&self) -> HPCON {
        self.0
    }

    fn is_open(&self) -> bool {
        self.0 != 0
    }
}

impl Drop for Hpcon {
    fn drop(&mut self) {
        self.close();
    }
}

/// ConPTY 会话。
struct ConPty {
    /// 持有它 = 持有终端；`ClosePseudoConsole` 即终止整棵进程树。
    hpc: Hpcon,
    /// 子进程句柄（等退出用）；`OwnedHandle` 负责关闭。
    process: OwnedHandle,
    /// 子进程 pid。
    pid: u32,
    /// conout 读端（取走后归调用方）。
    reader: Option<Box<dyn Read + Send>>,
    /// conin 写端（取走后归调用方）。
    writer: Option<Box<dyn Write + Send>>,
}

impl Pty for ConPty {
    fn pid(&self) -> u32 {
        self.pid
    }

    fn resize(&self, size: PtySize) -> Result<(), PtyError> {
        if !self.hpc.is_open() {
            return Err(PtyError::Terminated);
        }
        let result = unsafe { ResizePseudoConsole(self.hpc.get(), coord(size)) };
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
        let process = self.process.as_raw_handle();
        if unsafe { WaitForSingleObject(process, INFINITE) } != WAIT_OBJECT_0 {
            return Err(windows_error("WaitForSingleObject"));
        }
        exit_status(process)
    }

    fn try_wait(&mut self) -> Result<Option<ExitStatus>, PtyError> {
        let process = self.process.as_raw_handle();
        match unsafe { WaitForSingleObject(process, 0) } {
            WAIT_OBJECT_0 => exit_status(process).map(Some),
            WAIT_TIMEOUT => Ok(None),
            _ => Err(windows_error("WaitForSingleObject")),
        }
    }

    fn terminate(&mut self) -> Result<(), PtyError> {
        // 先礼：关控制台。它会终止**已挂上去**的进程树，并等 conout 排空。
        self.hpc.close();

        // 后兵：控制台刚建好、子进程还没来得及挂上去时，`ClosePseudoConsole` 可能什么都没杀——
        // 进程会漏在后台，退出事件也永远不会来（#67 的 CI 实测：刚创建就 terminate 必挂）。
        // 等不到就强杀根进程；整棵树通常已经被关控制台带走，这里只兜根。
        let process = self.process.as_raw_handle();
        if unsafe { WaitForSingleObject(process, TERMINATE_GRACE) } == WAIT_TIMEOUT {
            unsafe { TerminateProcess(process, 1) };
        }
        Ok(())
    }
}

impl Drop for ConPty {
    fn drop(&mut self) {
        // 兜底：谁忘了 terminate，也别把会话泄漏在后台。
        //
        // 关控制台要等 conout 排空，而 `Drop` 里没法同步去读（会把自己卡死）。所以：
        // **输出流还在我们手上**就起个后台线程把它读到 EOF；已经取走的那一半归调用方
        // （见 [`Pty::terminate`] 的约定）。
        if let Some(reader) = self.reader.take() {
            drain_in_background(reader);
        }
        self.hpc.close();
    }
}

/// 把输出流读到 EOF（后台线程），只为让 `ClosePseudoConsole` 能排空。
fn drain_in_background(mut reader: Box<dyn Read + Send>) {
    std::thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        while matches!(reader.read(&mut buffer), Ok(read) if read > 0) {}
    });
}

/// 建属性列表 → 挂上伪控制台 → `CreateProcessW`。返回 `(进程句柄, pid)`。
fn spawn_child(hpc: HPCON, spec: &PtySpec) -> Result<(OwnedHandle, u32), PtyError> {
    // 先把**可能失败**的东西都造好：命令行、环境块。放在属性列表之前，免得 `?` 早退时
    // 要记得 `DeleteProcThreadAttributeList`（第一版就漏了这条）。
    let mut command_line = to_wide(&command_line(&spec.program, &spec.args));
    let current_directory = to_wide(&spec.cwd.to_string_lossy());
    let environment = environment_block(&spec.env)?;

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
            // SAFETY：`hProcess` 的所有权交给 `OwnedHandle`，之后只由它关。
            Ok((
                unsafe { OwnedHandle::from_raw_handle(info.hProcess) },
                info.dwProcessId,
            ))
        }
    }
}

/// 硬约束①的实现点：把 HPCON **当作值**塞进指针槽位。
///
/// 注意：这个单测只盯住**本函数**；调用点若改传 `&hpc`，抓它的是
/// `tests/conpty.rs` 里那条真进程测试（传地址会让子进程跑到另一个控制台上，pty 里收不到字节）。
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

/// 拼成以 NUL 结尾的 UTF-16。
fn to_wide(text: &str) -> Vec<u16> {
    crate::win::to_wide(text)
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
}
