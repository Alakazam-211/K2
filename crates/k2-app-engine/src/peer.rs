//! Who is on the other end of the engine socket (`prd-k2-app-runtime-v1`
//! §10.6, SR13, escape smoke E18).
//!
//! 1. `SO_PEERCRED`: the peer's pid and real uid at `connect()`;
//! 2. hold the peer with a pidfd at once (`SO_PEERPIDFD`, Linux 6.5+, which
//!    names the exact process that connected; else `pidfd_open(pid)`);
//! 3. `daemon.toml` (root-owned) → the uid must be a configured daemon
//!    user's and the pid that unit's `MainPID` (shared check,
//!    `k2_root_proto::caller`);
//! 4. the held pidfd must still be alive (no pid reuse in between).
//!
//! Agents run as the daemon's uid but are its children, never its main
//! process, so they are refused (`caller_not_daemon`). User-scope daemons
//! are refused unless the user bus answers: the engine isn't root, so it
//! has no `runuser` fallback (K19's fallback is root-helper verbs).
//! [AR25] This stops naive or accidental use by an agent; it is not a wall
//! against a determined one that can ptrace the daemon.

#[cfg(target_os = "linux")]
use std::os::fd::FromRawFd;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};

use k2_root_proto::caller::{load_daemon_toml_file, match_main_pid, SystemdQuery};
use k2_root_proto::paths::DAEMON_TOML;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cred {
    pub pid: i32,
    pub uid: u32,
}

#[cfg(target_os = "linux")]
pub fn peer_cred(fd: RawFd) -> Option<Cred> {
    let mut c = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: valid fd, out-pointer sized for ucred.
    let r = unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_PEERCRED, &mut c as *mut _ as *mut libc::c_void, &mut len) };
    (r == 0 && c.pid > 0).then_some(Cred { pid: c.pid, uid: c.uid })
}

#[cfg(not(target_os = "linux"))]
pub fn peer_cred(_fd: RawFd) -> Option<Cred> {
    None
}

/// `SO_PEERPIDFD` (77) where the kernel has it, else `pidfd_open`.
#[cfg(target_os = "linux")]
pub fn peer_pidfd(fd: RawFd, pid: i32) -> Option<OwnedFd> {
    const SO_PEERPIDFD: libc::c_int = 77;
    let mut pfd: libc::c_int = -1;
    let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: valid fd, out-pointer sized for an int.
    let r = unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, SO_PEERPIDFD, &mut pfd as *mut _ as *mut libc::c_void, &mut len) };
    if r == 0 && pfd >= 0 {
        // SAFETY: the kernel just gave us this fd.
        return Some(unsafe { OwnedFd::from_raw_fd(pfd) });
    }
    // SAFETY: plain syscall, no pointers.
    let p = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if p < 0 {
        return None;
    }
    // SAFETY: fresh fd from the kernel.
    Some(unsafe { OwnedFd::from_raw_fd(p as RawFd) })
}

#[cfg(not(target_os = "linux"))]
pub fn peer_pidfd(_fd: RawFd, _pid: i32) -> Option<OwnedFd> {
    None
}

/// A pidfd polls readable once its process has exited.
pub fn pidfd_alive(pfd: &OwnedFd) -> bool {
    let mut p = libc::pollfd { fd: pfd.as_raw_fd(), events: libc::POLLIN, revents: 0 };
    // SAFETY: one valid pollfd, zero timeout.
    let r = unsafe { libc::poll(&mut p, 1, 0) };
    r == 0
}

/// `systemctl` as `k2app` (no root: user scope only through the user bus).
pub struct EngineQuery {
    pub passwd: String,
}

impl SystemdQuery for EngineQuery {
    fn run(&self, prog: &str, args: &[String]) -> Option<(i32, Vec<u8>)> {
        if !prog.starts_with('/') {
            return None;
        }
        let mut c = std::process::Command::new(prog);
        c.args(args).current_dir("/");
        c.env_clear().env("PATH", crate::sys::CHILD_PATH).env("LC_ALL", "C");
        let o = crate::sys::run(c, None, std::time::Duration::from_secs(10), 4096).ok()?;
        (!o.timed_out).then_some((o.status, o.stdout))
    }
    fn uid_of_user(&self, user: &str) -> Option<u32> {
        crate::sys::passwd_entry(&self.passwd, user).map(|(u, _, _)| u)
    }
    fn can_runuser(&self) -> bool {
        false
    }
}

/// The verdict for one connection: `Ok(daemon user)` or a static reason.
pub fn admit(fd: RawFd, q: &dyn SystemdQuery) -> Result<String, &'static str> {
    let cred = peer_cred(fd).ok_or("peer credentials unavailable")?;
    let held = peer_pidfd(fd, cred.pid).ok_or("peer process gone")?;
    let daemons = load_daemon_toml_file(std::path::Path::new(DAEMON_TOML))?;
    let d = match_main_pid(q, &daemons, cred.pid, cred.uid)?;
    if !pidfd_alive(&held) {
        return Err("caller process changed during the check");
    }
    Ok(d.user.clone())
}
