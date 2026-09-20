//! PTY 抽象 + Windows ConPTY 实现（B1 #4 决定的「PTY 藏在 trait 后面」）。
//!
//! ⚠️ PoC 取舍：这里用 **`windows-sys` 原生 ConPTY**，不引 `alacritty_terminal`。
//! 原因：本票要验的是 **ConPTY + IPC + 存活/重放 + 吞吐**，不是 PTY crate 选型；
//! 零重依赖也顺带证明了这个 trait 的抽象够用（换实现不动上层）。VT 内核在 #42。
//!
//! ⚠️ 踩过的两个 ConPTY 坑（都对齐了 MS 官方样例 / `portable-pty` 的一手实现）：
//! 1. `UpdateProcThreadAttribute(PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE)` 的 `lpValue` 是
//!    **句柄值本身**，不是「指向句柄的指针」；传 `&hpc` 会让子进程拿不到伪控制台。
//! 2. `CreatePipe` 出来的两端要等 **`CreateProcess` 之后**才释放。
//! 3. 必须给子进程显式 `STARTF_USESTDHANDLES` + 三句柄 NULL，否则它**继承父进程的 std handle**
//!    （daemon 的 stdio 是 NUL → 子进程 stdin 变 NUL → cmd 立刻 EOF 干净退 0）。

use std::io;

/// 拆出「可执行文件」与「其余参数」；容忍调用方已经给可执行文件加了引号。
pub fn split_prog(cmd: &str) -> (&str, &str) {
    if let Some(stripped) = cmd.strip_prefix('"') {
        if let Some(i) = stripped.find('"') {
            let end = i + 2; // 含两侧引号
            return (cmd[1..end - 1].trim(), &cmd[end..]);
        }
    }
    match cmd.find(' ') {
        Some(i) => (&cmd[..i], &cmd[i..]),
        None => (cmd, ""),
    }
}

#[cfg(test)]
mod split_prog_tests {
    use super::split_prog;

    #[test]
    fn splits_prog_from_args() {
        assert_eq!(
            split_prog("cmd.exe /c type x & timeout /t 3 >nul"),
            ("cmd.exe", " /c type x & timeout /t 3 >nul")
        );
        assert_eq!(
            split_prog("C:\\WINDOWS\\system32\\cmd.exe"),
            ("C:\\WINDOWS\\system32\\cmd.exe", "")
        );
        // 已经带引号的、路径含空格的
        assert_eq!(
            split_prog("\"C:\\Program Files\\Git\\bin\\bash.exe\" -l -i"),
            ("C:\\Program Files\\Git\\bin\\bash.exe", " -l -i")
        );
        // 未加引号但含空格（调用方失误时也能救）
        assert_eq!(
            split_prog("C:\\Program Files\\x.exe arg"),
            ("C:\\Program", " Files\\x.exe arg")
        );
    }
}

pub struct PtySpawn {
    pub shell: String,
    pub cols: u16,
    pub rows: u16,
}

/// 一个已启动的会话句柄。`read` 由调用方（daemon）用**阻塞线程**驱动。
pub trait Pty: Send + Sync {
    fn pid(&self) -> u32;
    /// 子进程是否仍在运行
    fn alive(&self) -> bool;
    /// 退出码；仍在运行 = `STILL_ACTIVE`(259)
    fn exit_code(&self) -> u32;
    fn write(&self, data: &[u8]) -> io::Result<()>;
    fn resize(&self, cols: u16, rows: u16) -> io::Result<()>;
    fn close(&self) -> io::Result<()>;
}

#[cfg(windows)]
mod win {
    use super::*;
    use std::fs::File;
    use std::os::windows::io::FromRawHandle;
    use std::sync::atomic::{AtomicBool, Ordering};
    use windows_sys::Win32::Foundation::{
        CloseHandle, SetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE,
        STILL_ACTIVE, TRUE,
    };
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::System::Console::{
        ClosePseudoConsole, CreatePseudoConsole, ResizePseudoConsole, COORD, HPCON,
    };
    use windows_sys::Win32::System::Pipes::CreatePipe;
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, DeleteProcThreadAttributeList, GetExitCodeProcess,
        InitializeProcThreadAttributeList, UpdateProcThreadAttribute, CREATE_UNICODE_ENVIRONMENT,
        EXTENDED_STARTUPINFO_PRESENT, PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
        STARTF_USESTDHANDLES, STARTUPINFOEXW,
    };

    pub struct WinPty {
        hpc: HPCON,
        input: File,
        process: HANDLE,
        pid: u32,
        closed: AtomicBool,
    }

    // HPCON 与 HANDLE 都是裸句柄，这里只做传递，不 deref。
    unsafe impl Send for WinPty {}
    unsafe impl Sync for WinPty {}

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// 造一对匿名管道
    unsafe fn pipe() -> io::Result<(HANDLE, HANDLE)> {
        let mut sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: std::ptr::null_mut(),
            bInheritHandle: TRUE,
        };
        let (mut r, mut w) = (INVALID_HANDLE_VALUE, INVALID_HANDLE_VALUE);
        if CreatePipe(&mut r, &mut w, &mut sa, 0) == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((r, w))
    }

    pub fn spawn(req: &PtySpawn) -> io::Result<(WinPty, File)> {
        unsafe {
            let (pty_in_r, pty_in_w) = pipe()?; // 给 ConPTY pty_in_r；我们写 pty_in_w
            let (pty_out_r, pty_out_w) = pipe()?; // 给 ConPTY pty_out_w；我们读 pty_out_r

            let size = COORD {
                X: req.cols as i16,
                Y: req.rows as i16,
            };
            let mut hpc: HPCON = 0;
            let hr = CreatePseudoConsole(size, pty_in_r, pty_out_w, 0, &mut hpc);
            if hr < 0 {
                CloseHandle(pty_in_r);
                CloseHandle(pty_out_w);
                return Err(io::Error::from_raw_os_error(hr));
            }
            crate::logbuf::log(&format!("CreatePseudoConsole ok（hpc={hpc}）"));

            // 我们这侧的两端不该被继承
            SetHandleInformation(pty_in_w, HANDLE_FLAG_INHERIT, 0);
            SetHandleInformation(pty_out_r, HANDLE_FLAG_INHERIT, 0);

            // 把 HPCON 塞进 STARTUPINFOEXW 的属性表
            let mut attr_size: usize = 0;
            InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut attr_size);
            let mut attr_buf = vec![0u8; attr_size];
            let attr_list = attr_buf.as_mut_ptr() as *mut _;
            if InitializeProcThreadAttributeList(attr_list, 1, 0, &mut attr_size) == 0 {
                let e = io::Error::last_os_error();
                CloseHandle(pty_in_r);
                CloseHandle(pty_out_w);
                ClosePseudoConsole(hpc);
                return Err(e);
            }
            // ⚠️ lpValue 是**句柄值本身**（MS 样例与 portable-pty 都如此）。
            // 传 &hpc 会让属性指向一块栈内存：子进程拿不到伪控制台。
            if UpdateProcThreadAttribute(
                attr_list,
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                hpc as *const std::ffi::c_void,
                std::mem::size_of::<HPCON>(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            ) == 0
            {
                let e = io::Error::last_os_error();
                DeleteProcThreadAttributeList(attr_list);
                CloseHandle(pty_in_r);
                CloseHandle(pty_out_w);
                ClosePseudoConsole(hpc);
                return Err(e);
            }

            let mut si: STARTUPINFOEXW = std::mem::zeroed();
            si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
            si.lpAttributeList = attr_list;
            // ⚠️ 必须显式声明 `STARTF_USESTDHANDLES` + 三句柄 NULL。
            // 否则 `CreateProcessW` 会把**父进程的 std handle 复制给子进程**：daemon 的 stdio
            // 一向指向 NUL（B1 #4 要求它与 UI 管道脱钩），子进程的 stdin 就成了 NUL，
            // cmd 读一次即 EOF、干净退 0，而 ConPTY 只吐开场序列（实测 exitCode=0 / bytesOut=16）。
            // 置 NULL 后由控制台子系统把子进程的 std handle 填成伪控制台。
            si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
            si.StartupInfo.hStdInput = std::ptr::null_mut();
            si.StartupInfo.hStdOutput = std::ptr::null_mut();
            si.StartupInfo.hStdError = std::ptr::null_mut();

            let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
            // lpApplicationName 为 null：只有**第一个 token**（可执行文件）需要引号，
            // 后面是参数，必须原样保留（否则整条命令行会被当成一个带空格的文件名 → os error 2）。
            let (prog, rest) = super::split_prog(&req.shell);
            let mut cmdline = wide(&format!("\"{}\"{}", prog, rest));
            let ok = CreateProcessW(
                std::ptr::null(),
                cmdline.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
                std::ptr::null(),
                std::ptr::null(),
                &si.StartupInfo,
                &mut pi,
            );

            // ⚠️ 子进程已起，**现在**才释放我们手里的两端（MS 样例 / portable-pty 的顺序）
            CloseHandle(pty_in_r);
            CloseHandle(pty_out_w);
            DeleteProcThreadAttributeList(attr_list);

            if ok == 0 {
                let e = io::Error::last_os_error();
                ClosePseudoConsole(hpc);
                return Err(e);
            }
            CloseHandle(pi.hThread);
            crate::logbuf::log(&format!(
                "CreateProcessW ok：pid={} shell={}",
                pi.dwProcessId, req.shell
            ));

            let input = File::from_raw_handle(pty_in_w);
            let output = File::from_raw_handle(pty_out_r);

            Ok((
                WinPty {
                    hpc,
                    input,
                    process: pi.hProcess,
                    pid: pi.dwProcessId,
                    closed: AtomicBool::new(false),
                },
                output,
            ))
        }
    }

    impl WinPty {
        pub fn pid(&self) -> u32 {
            self.pid
        }
        /// STILL_ACTIVE = 259；比 WaitForSingleObject 更直白，也便于报出退出码
        pub fn alive(&self) -> bool {
            self.exit_code() == STILL_ACTIVE as u32
        }
        pub fn exit_code(&self) -> u32 {
            let mut code: u32 = 0;
            unsafe {
                if GetExitCodeProcess(self.process, &mut code) == 0 {
                    return u32::MAX; // 查询失败
                }
            }
            code
        }
        pub fn write(&self, data: &[u8]) -> io::Result<()> {
            use std::io::Write;
            let mut f = &self.input;
            f.write_all(data)
        }
        pub fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
            unsafe {
                let hr = ResizePseudoConsole(
                    self.hpc,
                    COORD {
                        X: cols as i16,
                        Y: rows as i16,
                    },
                );
                if hr < 0 {
                    return Err(io::Error::from_raw_os_error(hr));
                }
            }
            Ok(())
        }
        pub fn close(&self) -> io::Result<()> {
            if self.closed.swap(true, Ordering::SeqCst) {
                return Ok(());
            }
            unsafe {
                // ClosePseudoConsole 会终止该 ConPTY 下的整个进程树（MS 文档语义）。
                ClosePseudoConsole(self.hpc);
                CloseHandle(self.process);
            }
            Ok(())
        }
    }

    impl Drop for WinPty {
        fn drop(&mut self) {
            let _ = self.close();
        }
    }
}

#[cfg(not(windows))]
mod stub {
    use super::*;

    pub struct StubPty;
    pub fn spawn(_req: &PtySpawn) -> io::Result<(StubPty, std::fs::File)> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "ConPTY 仅 Windows 可用",
        ))
    }
    impl StubPty {
        pub fn pid(&self) -> u32 {
            0
        }
        pub fn alive(&self) -> bool {
            false
        }
        pub fn exit_code(&self) -> u32 {
            0
        }
        pub fn write(&self, _d: &[u8]) -> io::Result<()> {
            Ok(())
        }
        pub fn resize(&self, _c: u16, _r: u16) -> io::Result<()> {
            Ok(())
        }
        pub fn close(&self) -> io::Result<()> {
            Ok(())
        }
    }
}

#[cfg(not(windows))]
pub use stub::{spawn, StubPty as PtyImpl};
#[cfg(windows)]
pub use win::{spawn, WinPty as PtyImpl};

impl Pty for PtyImpl {
    fn pid(&self) -> u32 {
        PtyImpl::pid(self)
    }
    fn alive(&self) -> bool {
        PtyImpl::alive(self)
    }
    fn exit_code(&self) -> u32 {
        PtyImpl::exit_code(self)
    }
    fn write(&self, data: &[u8]) -> io::Result<()> {
        PtyImpl::write(self, data)
    }
    fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        PtyImpl::resize(self, cols, rows)
    }
    fn close(&self) -> io::Result<()> {
        PtyImpl::close(self)
    }
}
