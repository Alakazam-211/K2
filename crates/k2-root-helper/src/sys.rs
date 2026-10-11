//! Production implementations of the [`crate::ctx`] traits. Linux is the
//! only target the helper ships for; on other OSes these compile (so the
//! unit tests run on a Mac) but the process table answers nothing, which
//! makes every `daemon` verb refuse.

use std::io;
use std::path::PathBuf;

use crate::ctx::{AuditSink, ProcHandle, Procs, Random};

pub fn self_exe() -> PathBuf {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_link("/proc/self/exe").unwrap_or_default()
    }
    #[cfg(not(target_os = "linux"))]
    {
        std::env::current_exe().unwrap_or_default()
    }
}

pub struct RealProcs;

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    pub struct PidFd(i32);

    impl ProcHandle for PidFd {
        fn alive(&self) -> bool {
            // pidfd_send_signal(fd, 0): 0 iff that exact process lives.
            // SAFETY: the fd is ours; a null siginfo is allowed.
            let rc = unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    self.0,
                    0,
                    std::ptr::null::<libc::siginfo_t>(),
                    0,
                )
            };
            rc == 0
        }
    }

    impl Drop for PidFd {
        fn drop(&mut self) {
            // SAFETY: we own the fd.
            unsafe { libc::close(self.0) };
        }
    }

    fn status_field(pid: i32, key: &str) -> Option<String> {
        let text = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
        text.lines()
            .find_map(|l| l.strip_prefix(key))
            .map(|v| v.trim().to_string())
    }

    impl Procs for RealProcs {
        fn parent_pid(&self) -> i32 {
            // SAFETY: no preconditions.
            unsafe { libc::getppid() }
        }
        fn hold(&self, pid: i32) -> Option<Box<dyn ProcHandle>> {
            // SAFETY: plain syscall; returns a new fd or -1.
            let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
            (fd >= 0).then(|| Box::new(PidFd(fd as i32)) as Box<dyn ProcHandle>)
        }
        fn exe(&self, pid: i32) -> Option<(PathBuf, u32)> {
            let p = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
            let owner = std::fs::symlink_metadata(&p).ok()?.uid();
            Some((p, owner))
        }
        fn ppid_of(&self, pid: i32) -> Option<i32> {
            status_field(pid, "PPid:")?.parse().ok()
        }
        fn ruid_of(&self, pid: i32) -> Option<u32> {
            // "Uid:\treal\teffective\tsaved\tfs"
            status_field(pid, "Uid:")?
                .split_whitespace()
                .next()?
                .parse()
                .ok()
        }
        fn uid_of_user(&self, user: &str) -> Option<u32> {
            super::uid_of_user(user)
        }
    }
}

#[cfg(not(target_os = "linux"))]
impl Procs for RealProcs {
    fn parent_pid(&self) -> i32 {
        // SAFETY: no preconditions.
        unsafe { libc::getppid() }
    }
    fn hold(&self, _pid: i32) -> Option<Box<dyn ProcHandle>> {
        None
    }
    fn exe(&self, _pid: i32) -> Option<(PathBuf, u32)> {
        None
    }
    fn ppid_of(&self, _pid: i32) -> Option<i32> {
        None
    }
    fn ruid_of(&self, _pid: i32) -> Option<u32> {
        None
    }
    fn uid_of_user(&self, user: &str) -> Option<u32> {
        uid_of_user(user)
    }
}

/// `getpwnam` uid, copied out before any other NSS call.
pub fn uid_of_user(user: &str) -> Option<u32> {
    ids_of_user(user).map(|(u, _)| u)
}

pub fn ids_of_user(user: &str) -> Option<(u32, u32)> {
    let c = std::ffi::CString::new(user).ok()?;
    // SAFETY: NUL-terminated; getpwnam returns static storage or null and
    // the ids are copied out immediately.
    let pw = unsafe { libc::getpwnam(c.as_ptr()) };
    if pw.is_null() {
        return None;
    }
    Some(unsafe { ((*pw).pw_uid, (*pw).pw_gid) })
}

pub struct RealRandom;

impl Random for RealRandom {
    fn fill(&self, buf: &mut [u8]) -> io::Result<()> {
        #[cfg(target_os = "linux")]
        {
            let mut done = 0;
            while done < buf.len() {
                // SAFETY: writes at most the remaining length into buf.
                let n = unsafe {
                    libc::getrandom(buf[done..].as_mut_ptr().cast(), buf.len() - done, 0)
                };
                if n < 0 {
                    let e = io::Error::last_os_error();
                    if e.kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    return Err(e);
                }
                done += n as usize;
            }
            Ok(())
        }
        #[cfg(not(target_os = "linux"))]
        {
            for chunk in buf.chunks_mut(256) {
                // SAFETY: getentropy fills at most 256 bytes per call.
                if unsafe { libc::getentropy(chunk.as_mut_ptr().cast(), chunk.len()) } != 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(())
        }
    }
}

/// syslog(3) → journald with `SYSLOG_IDENTIFIER=k2-root-helper`.
pub struct SyslogAudit;

impl AuditSink for SyslogAudit {
    fn line(&self, json: &str) {
        let Ok(msg) = std::ffi::CString::new(json.replace('\0', "")) else {
            return;
        };
        // SAFETY: static identifier and format; msg is NUL-terminated.
        unsafe {
            libc::openlog(
                c"k2-root-helper".as_ptr(),
                libc::LOG_PID,
                libc::LOG_AUTHPRIV,
            );
            libc::syslog(libc::LOG_INFO, c"%s".as_ptr(), msg.as_ptr());
            libc::closelog();
        }
    }
}
