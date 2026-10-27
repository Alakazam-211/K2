use std::ffi::c_void;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, ERROR_SUCCESS, HANDLE};
use windows_sys::Win32::Security::Authorization::{GetNamedSecurityInfoW, SetNamedSecurityInfoW, SE_FILE_OBJECT};
use windows_sys::Win32::Security::{
    AddAccessAllowedAceEx, AclSizeInformation, EqualSid, GetAce, GetAclInformation, GetLengthSid, GetTokenInformation,
    InitializeAcl, TokenOwner, TokenUser, ACCESS_ALLOWED_ACE, ACL, ACL_REVISION, ACL_SIZE_INFORMATION,
    CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION, OBJECT_INHERIT_ACE, OWNER_SECURITY_INFORMATION,
    PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, TOKEN_INFORMATION_CLASS, TOKEN_QUERY,
};
use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// `ACCESS_ALLOWED_ACE_TYPE` (winnt.h).
const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
}

/// A SID copied out of a token buffer (the buffer is the owner).
struct TokenSid {
    buf: Vec<u64>,
}

impl TokenSid {
    /// TOKEN_USER / TOKEN_OWNER both start with the SID pointer.
    fn sid(&self) -> PSID {
        // SAFETY: buf holds a TOKEN_USER or TOKEN_OWNER whose first field
        // is a PSID pointing inside buf.
        unsafe { *(self.buf.as_ptr() as *const PSID) }
    }
}

fn token_sid(class: TOKEN_INFORMATION_CLASS) -> io::Result<TokenSid> {
    // SAFETY: plain Win32 calls on our own process token; every buffer is
    // sized from the call that reports the length, and the handle closed.
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut len = 0u32;
        GetTokenInformation(token, class, std::ptr::null_mut(), 0, &mut len);
        if len == 0 {
            let e = io::Error::last_os_error();
            CloseHandle(token);
            return Err(e);
        }
        let mut buf = vec![0u64; (len as usize).div_ceil(8)];
        let ok = GetTokenInformation(token, class, buf.as_mut_ptr() as *mut c_void, len, &mut len);
        let e = io::Error::last_os_error();
        CloseHandle(token);
        if ok == 0 {
            return Err(e);
        }
        Ok(TokenSid { buf })
    }
}

/// Give `path` a protected DACL with one entry: the current user, full
/// control, inherited by files and folders created inside. Existing
/// inherited entries (Users, Authenticated Users, …) are dropped.
pub fn set_owner_only(path: &Path) -> io::Result<()> {
    let user = token_sid(TokenUser)?;
    // SAFETY: the ACL buffer is sized for one ACE carrying the user SID
    // (ACL header + ACE minus its SidStart placeholder + the SID), aligned
    // for ACL (u32), and outlives SetNamedSecurityInfoW.
    unsafe {
        let sid = user.sid();
        let sid_len = GetLengthSid(sid) as usize;
        let size = std::mem::size_of::<ACL>() + std::mem::size_of::<ACCESS_ALLOWED_ACE>() - std::mem::size_of::<u32>() + sid_len;
        let mut acl_buf = vec![0u32; size.div_ceil(4)];
        let acl = acl_buf.as_mut_ptr() as *mut ACL;
        if InitializeAcl(acl, (acl_buf.len() * 4) as u32, ACL_REVISION) == 0 {
            return Err(io::Error::last_os_error());
        }
        if AddAccessAllowedAceEx(acl, ACL_REVISION, OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE, FILE_ALL_ACCESS, sid) == 0 {
            return Err(io::Error::last_os_error());
        }
        let name = wide(path);
        let rc = SetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            acl,
            std::ptr::null(),
        );
        if rc != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(rc as i32));
        }
    }
    Ok(())
}

/// A security descriptor GetNamedSecurityInfoW allocated (LocalFree'd).
struct Descriptor(PSECURITY_DESCRIPTOR);

impl Drop for Descriptor {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: allocated by GetNamedSecurityInfoW for us.
            unsafe { LocalFree(self.0) };
        }
    }
}

/// True when every entry of `path`'s DACL allows only the current user (and
/// there is at least one): no other user or group can open it.
pub fn is_owner_only(path: &Path) -> io::Result<bool> {
    let user = token_sid(TokenUser)?;
    let name = wide(path);
    // SAFETY: out-pointers are filled by GetNamedSecurityInfoW and point into
    // the descriptor it allocates, freed by `Descriptor` after we are done.
    unsafe {
        let mut dacl: *mut ACL = std::ptr::null_mut();
        let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        let rc = GetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut dacl,
            std::ptr::null_mut(),
            &mut sd,
        );
        if rc != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(rc as i32));
        }
        let _sd = Descriptor(sd);
        if dacl.is_null() {
            // A NULL DACL grants everyone everything.
            return Ok(false);
        }
        let mut info = ACL_SIZE_INFORMATION::default();
        if GetAclInformation(
            dacl,
            &mut info as *mut _ as *mut c_void,
            std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        if info.AceCount == 0 {
            return Ok(false);
        }
        for i in 0..info.AceCount {
            let mut ace: *mut c_void = std::ptr::null_mut();
            if GetAce(dacl, i, &mut ace) == 0 {
                return Err(io::Error::last_os_error());
            }
            let ace = ace as *const ACCESS_ALLOWED_ACE;
            if (*ace).Header.AceType != ACCESS_ALLOWED_ACE_TYPE {
                return Ok(false);
            }
            let sid = std::ptr::addr_of!((*ace).SidStart) as PSID;
            if EqualSid(sid, user.sid()) == 0 {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// True when `path`'s owner is the current user (or the token's default
/// owner, which an elevated administrator's new files get).
pub fn owned_by_current_user(path: &Path) -> io::Result<bool> {
    let user = token_sid(TokenUser)?;
    let default_owner = token_sid(TokenOwner)?;
    let name = wide(path);
    // SAFETY: as in is_owner_only.
    unsafe {
        let mut owner: PSID = std::ptr::null_mut();
        let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        let rc = GetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut sd,
        );
        if rc != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(rc as i32));
        }
        let _sd = Descriptor(sd);
        if owner.is_null() {
            return Ok(false);
        }
        Ok(EqualSid(owner, user.sid()) != 0 || EqualSid(owner, default_owner.sid()) != 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("k2-winsec-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).expect("mkdir");
        p
    }

    #[test]
    fn a_new_temp_dir_is_not_owner_only_until_set() {
        let d = temp("dir");
        // The temp dir inherits its parent's entries (SYSTEM, Administrators).
        assert!(!is_owner_only(&d).expect("read dacl"));
        set_owner_only(&d).expect("set");
        assert!(is_owner_only(&d).expect("read dacl"));
        assert!(owned_by_current_user(&d).expect("owner"));
        std::fs::remove_dir_all(&d).expect("cleanup");
    }

    #[test]
    fn files_created_inside_inherit_owner_only() {
        let d = temp("inherit");
        set_owner_only(&d).expect("set");
        let f = d.join("f");
        std::fs::write(&f, b"x").expect("write");
        assert!(is_owner_only(&f).expect("read dacl"), "the user ACE is inherited, nothing else");
        let sub = d.join("sub");
        std::fs::create_dir(&sub).expect("mkdir");
        assert!(is_owner_only(&sub).expect("read dacl"));
        std::fs::remove_dir_all(&d).expect("cleanup");
    }

    #[test]
    fn a_missing_path_is_an_error_not_false() {
        let d = temp("missing");
        let e = is_owner_only(&d.join("nope")).expect_err("missing");
        assert_eq!(e.kind(), io::ErrorKind::NotFound);
        std::fs::remove_dir_all(&d).expect("cleanup");
    }
}
