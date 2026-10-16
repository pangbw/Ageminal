//! **只允许当前用户**访问的安全描述符（issue #64 验收 4）。
//!
//! `CreateNamedPipeW` 的默认安全描述符是「创建者令牌的默认 DACL」——通常含 `SYSTEM` 与
//! `Administrators`，且依赖令牌内容。这里显式给一条 DACL：**单条 ACE，当前用户，`FILE_ALL_ACCESS`**，
//! 于是「非当前用户」会被内核直接挡在管道外（不靠应用层判断）。
//!
//! 内存所有权是本模块最要紧的一点：`SetSecurityDescriptorDacl` 存的是**指向 ACL 的裸指针**
//! （非自相对形式），所以 ACL 缓冲必须活到 `CreateNamedPipeW` 返回之后——见
//! [`CurrentUserSecurity`]。全程不调 `LocalAlloc`：用 Rust 自己的对齐缓冲，避免手工 free。

use std::mem::size_of;

use windows_sys::Win32::Security::{
    AddAccessAllowedAce, InitializeAcl, InitializeSecurityDescriptor, SetSecurityDescriptorDacl,
    ACCESS_ALLOWED_ACE, ACL, ACL_REVISION, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES,
    SECURITY_DESCRIPTOR,
};
use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
use windows_sys::Win32::System::SystemServices::SECURITY_DESCRIPTOR_REVISION;

use crate::{win, DaemonError};

/// 一块保证 8 字节（指针对齐）的原始缓冲。
struct Aligned(Vec<u64>);

impl Aligned {
    fn new(bytes: usize) -> Self {
        Self(vec![0u64; bytes.div_ceil(size_of::<u64>())])
    }

    fn as_mut_ptr<T>(&mut self) -> *mut T {
        self.0.as_mut_ptr().cast()
    }
}

/// 当前用户专属安全描述符所依赖的三块内存 + `SECURITY_ATTRIBUTES`。
///
/// `sid` / `acl` / `sd` 存在的唯一理由是**保活**：描述符里存的是指向它们的裸指针，一旦被回收，
/// 描述符就悬空。只有测试会真正读它们（核对 DACL 内容）。
#[allow(dead_code)]
pub struct CurrentUserSecurity {
    sid: Vec<u8>,
    acl: Aligned,
    sd: Aligned,
    attributes: SECURITY_ATTRIBUTES,
}

impl CurrentUserSecurity {
    /// 构造「当前用户 + `FILE_ALL_ACCESS`」的 DACL。
    pub fn build() -> Result<Self, DaemonError> {
        let sid = win::current_user_sid()?;
        unsafe { Self::build_from_sid(sid) }
    }

    /// 交给 `CreateNamedPipeW` 的 `lpSecurityAttributes`。
    pub fn attributes(&self) -> *const SECURITY_ATTRIBUTES {
        &self.attributes
    }

    /// # Safety
    ///
    /// 无外部前置条件；标 `unsafe` 只是因为整段都是裸指针操作。
    unsafe fn build_from_sid(sid: Vec<u8>) -> Result<Self, DaemonError> {
        let mut sd = Aligned::new(size_of::<SECURITY_DESCRIPTOR>());
        let sd_ptr = sd.as_mut_ptr::<SECURITY_DESCRIPTOR>() as PSECURITY_DESCRIPTOR;
        if InitializeSecurityDescriptor(sd_ptr, SECURITY_DESCRIPTOR_REVISION) == 0 {
            return Err(win::windows_error("InitializeSecurityDescriptor"));
        }

        // ACL 大小 = 固定头 + 一条 ACCESS_ALLOWED_ACE（它的 `SidStart` 已含 4 字节 SID 占位）。
        let acl_bytes =
            size_of::<ACL>() + size_of::<ACCESS_ALLOWED_ACE>() - size_of::<u32>() + sid.len();
        let mut acl = Aligned::new(acl_bytes);
        let acl_ptr = acl.as_mut_ptr::<ACL>();
        if InitializeAcl(acl_ptr, acl_bytes as u32, ACL_REVISION) == 0 {
            return Err(win::windows_error("InitializeAcl"));
        }
        if AddAccessAllowedAce(acl_ptr, ACL_REVISION, FILE_ALL_ACCESS, sid.as_ptr() as PSID) == 0 {
            return Err(win::windows_error("AddAccessAllowedAce"));
        }
        // 指针存进描述符：`sd` / `acl` 之后会被 move，但堆缓冲地址不变，指针仍有效。
        if SetSecurityDescriptorDacl(sd_ptr, 1, acl_ptr, 0) == 0 {
            return Err(win::windows_error("SetSecurityDescriptorDacl"));
        }

        Ok(Self {
            sid,
            acl,
            sd,
            attributes: SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: sd_ptr,
                bInheritHandle: 0,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 自动化的那半：DACL **确实**只有一条 ACE，且它的 SID 就是当前用户。
    ///
    /// 另一半（「**别的**用户被拒绝」）需要第二个 Windows 账户，属人工验收——见 issue #64。
    #[test]
    fn dacl_grants_exactly_the_current_user() {
        let security = CurrentUserSecurity::build().unwrap();

        unsafe {
            let descriptor = &*security.sd.0.as_ptr().cast::<SECURITY_DESCRIPTOR>();
            assert!(!descriptor.Dacl.is_null(), "安全描述符必须带 DACL");

            let dacl = &*descriptor.Dacl;
            assert_eq!(dacl.AceCount, 1, "DACL 只该有一条 ACE");

            let ace = &*descriptor
                .Dacl
                .cast::<u8>()
                .add(size_of::<ACL>())
                .cast::<ACCESS_ALLOWED_ACE>();
            assert_eq!(ace.Mask, FILE_ALL_ACCESS);

            let sid = (&ace.SidStart as *const u32).cast::<u8>();
            let length = 8 + usize::from(*sid.add(1)) * 4;
            let ace_sid = std::slice::from_raw_parts(sid, length);
            assert_eq!(ace_sid, security.sid.as_slice(), "ACE 的 SID 应为当前用户");
        }
    }

    #[test]
    fn attributes_point_at_the_descriptor() {
        let security = CurrentUserSecurity::build().unwrap();
        let attributes = &security.attributes;
        assert_eq!(attributes.nLength, size_of::<SECURITY_ATTRIBUTES>() as u32);
        assert!(!attributes.lpSecurityDescriptor.is_null());
        assert_eq!(attributes.bInheritHandle, 0);
        assert_eq!(
            security.attributes(),
            attributes as *const SECURITY_ATTRIBUTES
        );
    }
}
