//! 一层很薄的 Windows 原语封装：宽字符串、当前用户 SID、随机令牌。
//!
//! 这里只放**被多处复用**的底层动作；具体的安全描述符构造在 [`crate::acl`]，管道在
//! [`crate::server`] / [`crate::client`]。

use std::ffi::c_void;
use std::fmt::Write as _;
use std::os::windows::io::{FromRawHandle, OwnedHandle};

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE};
use windows_sys::Win32::Security::Cryptography::{
    BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
};
use windows_sys::Win32::Security::{GetTokenInformation, TokenUser, SID, TOKEN_QUERY, TOKEN_USER};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use crate::DaemonError;

/// UTF-8 → NUL 结尾的 UTF-16（Win32 的 `…W` 系列都要它）。
pub fn to_wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 把刚失败的 Win32 调用 + `GetLastError()` 包成 [`DaemonError`]。
///
/// **必须紧接着失败的那次调用**取错误码：中间插入别的 Win32 调用会把它冲掉。
pub fn windows_error(call: &'static str) -> DaemonError {
    DaemonError::Windows {
        call,
        reason: format!("GetLastError={}", unsafe { GetLastError() }),
    }
}

/// 小写十六进制（用于 SID 与令牌）。
pub fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// 按 pid 打开进程句柄，只用于「等它退出」。
///
/// 会话层用它把「观察退出」和「控制 PTY」解耦：`terminate` 里的 `ClosePseudoConsole` 可能长时间
/// 阻塞，观察者不该被同一把锁卡住（#67 的 CI 实测：刚创建就终止时，退出事件永远发不出来）。
///
/// 进程对象此刻仍被 PTY 的进程句柄持有，所以即使子进程已经退出，`OpenProcess` 也拿得到。
pub fn open_process(pid: u32) -> Result<OwnedHandle, DaemonError> {
    use windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        return Err(windows_error("OpenProcess"));
    }
    // SAFETY：句柄所有权交给 `OwnedHandle`，由它负责关闭。
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

/// 当前用户的 SID（字节串）。
///
/// `SID` 的布局是 `Revision | SubAuthorityCount | Authority(6) | SubAuthority×n`。
/// 用字节而不是 `ConvertSidToStringSidW` 的字符串形式：我们只需要一个**稳定、按用户隔离**
/// 的标识（喂给管道名与 DACL），不追求可读。
pub fn current_user_sid() -> Result<Vec<u8>, DaemonError> {
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(windows_error("OpenProcessToken"));
        }
        let sid = token_user_sid(token);
        CloseHandle(token);
        sid
    }
}

/// 从进程令牌里取出用户 SID。
///
/// # Safety
///
/// `token` 必须是带 `TOKEN_QUERY` 权限的有效令牌句柄。
unsafe fn token_user_sid(token: HANDLE) -> Result<Vec<u8>, DaemonError> {
    // `TOKEN_USER` 结构体**和它指向的 SID** 都会写回这块缓冲；`u64` 保证指针对齐。
    let mut buffer = [0u64; 64];
    let capacity = (buffer.len() * std::mem::size_of::<u64>()) as u32;
    let mut needed = 0u32;
    if GetTokenInformation(
        token,
        TokenUser,
        buffer.as_mut_ptr().cast(),
        capacity,
        &mut needed,
    ) == 0
    {
        return Err(windows_error("GetTokenInformation(TokenUser)"));
    }

    let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
    let sid = user.User.Sid.cast::<SID>();
    // SID 变长：8 字节头 + SubAuthorityCount × 4。
    let length = 8 + usize::from((*sid).SubAuthorityCount) * 4;
    Ok(std::slice::from_raw_parts(sid.cast::<u8>(), length).to_vec())
}

/// 32 字节随机 → 64 位十六进制。**每次 daemon 启动重生成**（REQUIREMENTS.md §10.5）。
pub fn random_token() -> Result<String, DaemonError> {
    let mut bytes = [0u8; 32];
    let status = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut::<c_void>(),
            bytes.as_mut_ptr(),
            bytes.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status != 0 {
        return Err(DaemonError::Windows {
            call: "BCryptGenRandom",
            reason: format!("NTSTATUS=0x{status:08X}"),
        });
    }
    Ok(hex(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sid_is_variable_length_and_non_empty() {
        let sid = current_user_sid().unwrap();
        // 8 字节头 + n×4 子权限。
        assert!(sid.len() >= 12);
        assert_eq!((sid.len() - 8) % 4, 0);
        assert_eq!(usize::from(sid[1]), (sid.len() - 8) / 4);
    }

    #[test]
    fn tokens_are_unique_and_well_formed() {
        let a = random_token().unwrap();
        let b = random_token().unwrap();
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn wide_strings_are_nul_terminated() {
        assert_eq!(to_wide("ab"), vec![0x61, 0x62, 0x00]);
    }
}
