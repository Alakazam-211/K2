//! Windows: start the K2 daemon at logon, as macOS's launchd agent
//! `dev.k2.daemon` (`RunAtLoad`) does there.
//!
//! A per-user value `K2 Daemon` under
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` names the installed
//! `k2-daemon.exe` (a windows-subsystem binary in release builds, so no
//! console window). No admin rights; the uninstaller removes the value.
//! Windows has no launchd `KeepAlive`: a crashed daemon comes back when the
//! app opens (it spawns a missing daemon) or at the next logon.

use std::ffi::OsStr;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::Path;

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ,
};

/// The per-user Run key (relative to HKCU).
pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// The value K2 owns there.
pub const RUN_VALUE: &str = "K2 Daemon";

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// The command a Run value holds for `program`: the quoted path.
pub fn command_for(program: &Path) -> String {
    format!("\"{}\"", program.display())
}

/// The command K2's value holds, if any.
pub fn registered() -> Result<Option<String>, String> {
    read_value(RUN_KEY, RUN_VALUE)
}

/// Point K2's Run value at `program`. `Ok(true)` when it was missing or
/// named another path, `Ok(false)` when it was already right.
pub fn ensure(program: &Path) -> Result<bool, String> {
    ensure_at(RUN_KEY, RUN_VALUE, program)
}

/// Remove K2's Run value (missing is fine).
pub fn remove() -> Result<(), String> {
    remove_at(RUN_KEY, RUN_VALUE)
}

pub(crate) fn ensure_at(key: &str, value: &str, program: &Path) -> Result<bool, String> {
    let want = command_for(program);
    if read_value(key, value)?.as_deref() == Some(want.as_str()) {
        return Ok(false);
    }
    let data = wide(OsStr::new(&want));
    // SAFETY: NUL-terminated key/value names and data; the byte count
    // includes the terminating NUL as REG_SZ requires.
    let rc = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            wide(OsStr::new(key)).as_ptr(),
            wide(OsStr::new(value)).as_ptr(),
            REG_SZ,
            data.as_ptr() as *const core::ffi::c_void,
            (data.len() * 2) as u32,
        )
    };
    if rc != ERROR_SUCCESS {
        return Err(format!("set HKCU\\{key}\\{value}: {}", std::io::Error::from_raw_os_error(rc as i32)));
    }
    Ok(true)
}

pub(crate) fn read_value(key: &str, value: &str) -> Result<Option<String>, String> {
    let (k, v) = (wide(OsStr::new(key)), wide(OsStr::new(value)));
    let mut bytes: u32 = 0;
    // SAFETY: a size query (null data) then a read into a buffer of that size.
    unsafe {
        let rc = RegGetValueW(HKEY_CURRENT_USER, k.as_ptr(), v.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), std::ptr::null_mut(), &mut bytes);
        if rc == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        if rc != ERROR_SUCCESS {
            return Err(format!("read HKCU\\{key}\\{value}: {}", std::io::Error::from_raw_os_error(rc as i32)));
        }
        let mut buf = vec![0u16; (bytes as usize).div_ceil(2)];
        let rc = RegGetValueW(
            HKEY_CURRENT_USER,
            k.as_ptr(),
            v.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr() as *mut core::ffi::c_void,
            &mut bytes,
        );
        if rc != ERROR_SUCCESS {
            return Err(format!("read HKCU\\{key}\\{value}: {}", std::io::Error::from_raw_os_error(rc as i32)));
        }
        let n = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
        Ok(Some(std::ffi::OsString::from_wide(&buf[..n]).to_string_lossy().into_owned()))
    }
}

pub(crate) fn remove_at(key: &str, value: &str) -> Result<(), String> {
    // SAFETY: NUL-terminated names.
    let rc = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, wide(OsStr::new(key)).as_ptr(), wide(OsStr::new(value)).as_ptr()) };
    if rc == ERROR_SUCCESS || rc == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        Err(format!("delete HKCU\\{key}\\{value}: {}", std::io::Error::from_raw_os_error(rc as i32)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::Registry::RegDeleteTreeW;

    /// A throwaway key, never the real Run key.
    struct TestKey(String);
    impl Drop for TestKey {
        fn drop(&mut self) {
            // SAFETY: NUL-terminated key under HKCU that this test made.
            unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, wide(OsStr::new(&self.0)).as_ptr()) };
        }
    }

    #[test]
    fn ensure_writes_once_reads_back_and_remove_clears() {
        let key = TestKey(format!(r"Software\K2Test\Run-{}", std::process::id()));
        let exe = Path::new(r"C:\Program Files\K2\k2-daemon.exe");
        assert_eq!(read_value(&key.0, RUN_VALUE).unwrap(), None);
        assert!(ensure_at(&key.0, RUN_VALUE, exe).unwrap(), "missing -> written");
        assert_eq!(read_value(&key.0, RUN_VALUE).unwrap().as_deref(), Some(r#""C:\Program Files\K2\k2-daemon.exe""#));
        assert!(!ensure_at(&key.0, RUN_VALUE, exe).unwrap(), "already right -> untouched");
        assert!(ensure_at(&key.0, RUN_VALUE, Path::new(r"D:\K2\k2-daemon.exe")).unwrap(), "moved -> rewritten");
        remove_at(&key.0, RUN_VALUE).unwrap();
        assert_eq!(read_value(&key.0, RUN_VALUE).unwrap(), None);
        remove_at(&key.0, RUN_VALUE).unwrap();
    }
}
