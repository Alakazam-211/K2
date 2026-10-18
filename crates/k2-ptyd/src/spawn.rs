//! Child spawn (PRD §7.3 `spawn`, P14). `openpty`, then in the child:
//! `setsid` → `chdir` → `TIOCSCTTY` → slave on 0/1/2 → close **all** other
//! fds → reset **every** signal disposition and the mask → `execve`.
//! Stricter than alacritty's `pre_exec` on purpose (V31). `IUTF8` is set on
//! the master. Everything the child needs is prepared before `fork`, so the
//! child only makes async-signal-safe calls.

use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::Path;

use crate::sys::{cvt, set_cloexec, set_nonblocking};

pub struct SpawnSpec<'a> {
    pub argv: &'a [String],
    pub env: &'a [(String, String)],
    pub cwd: Option<&'a str>,
    pub cols: u16,
    pub rows: u16,
}

pub struct Spawned {
    pub pid: i32,
    pub master: OwnedFd,
}

/// Resolve `argv[0]` against the CHILD's `PATH` (from `env`), like
/// `execvp` would in the child. Absolute / relative-with-slash names are
/// used as given.
pub fn resolve_program(prog: &str, env: &[(String, String)]) -> io::Result<String> {
    if prog.contains('/') {
        return Ok(prog.to_string());
    }
    let path = env
        .iter()
        .find(|(k, _)| k == "PATH")
        .map(|(_, v)| v.as_str())
        .unwrap_or("/usr/bin:/bin");
    for dir in path.split(':').filter(|d| !d.is_empty()) {
        let cand = Path::new(dir).join(prog);
        if let Ok(md) = std::fs::metadata(&cand) {
            use std::os::unix::fs::PermissionsExt;
            if md.is_file() && md.permissions().mode() & 0o111 != 0 {
                return Ok(cand.to_string_lossy().into_owned());
            }
        }
    }
    Err(io::Error::new(io::ErrorKind::NotFound, format!("{prog}: not found on the child PATH")))
}

fn cstr(s: &str) -> io::Result<CString> {
    CString::new(s).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL byte in argv/env/cwd"))
}

#[cfg(target_os = "linux")]
const LAST_SIGNAL: libc::c_int = 64;
#[cfg(not(target_os = "linux"))]
const LAST_SIGNAL: libc::c_int = 31;

fn max_fd() -> RawFd {
    // SAFETY: sysconf has no preconditions.
    let n = unsafe { libc::sysconf(libc::_SC_OPEN_MAX) };
    if n <= 0 {
        1024
    } else {
        n.min(65_536) as RawFd
    }
}

pub fn spawn(spec: &SpawnSpec) -> io::Result<Spawned> {
    if spec.argv.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty argv"));
    }
    let program = cstr(&resolve_program(&spec.argv[0], spec.env)?)?;
    let argv: Vec<CString> = spec.argv.iter().map(|a| cstr(a)).collect::<io::Result<_>>()?;
    let envp: Vec<CString> = spec
        .env
        .iter()
        .map(|(k, v)| cstr(&format!("{k}={v}")))
        .collect::<io::Result<_>>()?;
    let cwd = spec.cwd.map(cstr).transpose()?;
    let mut argv_p: Vec<*const libc::c_char> = argv.iter().map(|c| c.as_ptr()).collect();
    argv_p.push(std::ptr::null());
    let mut envp_p: Vec<*const libc::c_char> = envp.iter().map(|c| c.as_ptr()).collect();
    envp_p.push(std::ptr::null());
    let top_fd = max_fd();

    let mut ws = libc::winsize { ws_row: spec.rows.max(1), ws_col: spec.cols.max(1), ws_xpixel: 0, ws_ypixel: 0 };
    let (mut m, mut s): (RawFd, RawFd) = (-1, -1);
    // SAFETY: openpty fills two fds.
    cvt(unsafe { libc::openpty(&mut m, &mut s, std::ptr::null_mut(), std::ptr::null_mut(), &mut ws) })?;
    // SAFETY: both fds were just returned by openpty and are owned here.
    let master = unsafe { OwnedFd::from_raw_fd(m) };
    let slave = unsafe { OwnedFd::from_raw_fd(s) };
    set_cloexec(m, true)?;
    set_cloexec(s, true)?;
    set_iutf8(m)?;

    let mut p = [0 as RawFd; 2];
    // SAFETY: pipe fills two fds.
    cvt(unsafe { libc::pipe(p.as_mut_ptr()) })?;
    // SAFETY: owned from here on.
    let err_r = unsafe { OwnedFd::from_raw_fd(p[0]) };
    let err_w = unsafe { OwnedFd::from_raw_fd(p[1]) };
    set_cloexec(p[0], true)?;
    set_cloexec(p[1], true)?;
    let errw = err_w.as_raw_fd();
    let slave_fd = slave.as_raw_fd();

    // SAFETY: fork; the child branch below only uses async-signal-safe calls
    // on memory prepared before the fork.
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(io::Error::last_os_error());
    }
    if pid == 0 {
        unsafe { child_after_fork(slave_fd, errw, top_fd, cwd.as_ref(), &program, &argv_p, &envp_p) }
    }
    drop(slave);
    drop(err_w);
    // Exec success closes err_w (CLOEXEC) → read returns 0.
    let mut buf = [0u8; 4];
    let mut got = 0;
    loop {
        // SAFETY: reading into a local buffer.
        let n = unsafe { libc::read(err_r.as_raw_fd(), buf[got..].as_mut_ptr() as *mut libc::c_void, 4 - got) };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        if n == 0 {
            break;
        }
        got += n as usize;
        if got == 4 {
            break;
        }
    }
    if got == 4 {
        let errno = i32::from_ne_bytes(buf);
        let mut st = 0;
        // SAFETY: reaping our own failed child.
        unsafe { libc::waitpid(pid, &mut st, 0) };
        return Err(io::Error::from_raw_os_error(errno));
    }
    set_nonblocking(master.as_raw_fd(), true)?;
    Ok(Spawned { pid, master })
}

fn set_iutf8(fd: RawFd) -> io::Result<()> {
    // SAFETY: termios get/set on our master fd.
    unsafe {
        let mut t: libc::termios = std::mem::zeroed();
        cvt(libc::tcgetattr(fd, &mut t))?;
        t.c_iflag |= libc::IUTF8;
        cvt(libc::tcsetattr(fd, libc::TCSANOW, &t))?;
    }
    Ok(())
}

/// Child side. Never returns.
unsafe fn child_after_fork(
    slave: RawFd,
    errw: RawFd,
    top_fd: RawFd,
    cwd: Option<&CString>,
    program: &CString,
    argv: &[*const libc::c_char],
    envp: &[*const libc::c_char],
) -> ! {
    unsafe fn fail(errw: RawFd) -> ! {
        let e = *errno_ptr();
        let b = e.to_ne_bytes();
        libc::write(errw, b.as_ptr() as *const libc::c_void, 4);
        libc::_exit(127)
    }
    if libc::setsid() < 0 {
        fail(errw);
    }
    if let Some(dir) = cwd {
        if libc::chdir(dir.as_ptr()) < 0 {
            fail(errw);
        }
    }
    if libc::ioctl(slave, libc::TIOCSCTTY as _, 0) < 0 {
        fail(errw);
    }
    for fd in 0..3 {
        if libc::dup2(slave, fd) < 0 {
            fail(errw);
        }
    }
    // Close every fd above 2 except the CLOEXEC error pipe (exec closes it).
    // Linux: close_range in two calls; elsewhere (or on an old kernel) a loop.
    #[cfg(target_os = "linux")]
    let ranged = errw >= 3
        && libc::syscall(libc::SYS_close_range, 3u32, (errw - 1) as u32, 0u32) == 0
        && libc::syscall(libc::SYS_close_range, (errw + 1) as u32, u32::MAX, 0u32) == 0;
    #[cfg(not(target_os = "linux"))]
    let ranged = false;
    if !ranged {
        for fd in 3..top_fd {
            if fd != errw {
                libc::close(fd);
            }
        }
    }
    // Every disposition back to default, empty mask.
    let mut sa: libc::sigaction = std::mem::zeroed();
    sa.sa_sigaction = libc::SIG_DFL;
    libc::sigemptyset(&mut sa.sa_mask);
    for sig in 1..=LAST_SIGNAL {
        if sig != libc::SIGKILL && sig != libc::SIGSTOP {
            libc::sigaction(sig, &sa, std::ptr::null_mut());
        }
    }
    let mut set: libc::sigset_t = std::mem::zeroed();
    libc::sigemptyset(&mut set);
    libc::sigprocmask(libc::SIG_SETMASK, &set, std::ptr::null_mut());
    libc::execve(program.as_ptr(), argv.as_ptr(), envp.as_ptr());
    fail(errw)
}

#[cfg(target_os = "linux")]
unsafe fn errno_ptr() -> *mut libc::c_int {
    libc::__errno_location()
}
#[cfg(not(target_os = "linux"))]
unsafe fn errno_ptr() -> *mut libc::c_int {
    libc::__error()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_program_uses_the_child_path_not_ptyds() {
        let env = vec![("PATH".to_string(), "/nonexistent:/bin:/usr/bin".to_string())];
        let sh = resolve_program("sh", &env).unwrap();
        assert!(sh == "/bin/sh" || sh == "/usr/bin/sh", "{sh}");
        let none = vec![("PATH".to_string(), "/nonexistent".to_string())];
        assert_eq!(resolve_program("sh", &none).unwrap_err().kind(), io::ErrorKind::NotFound);
        assert_eq!(resolve_program("./x", &none).unwrap(), "./x");
    }

    #[test]
    fn exec_failure_is_reported_not_a_zombie_session() {
        let argv = vec!["/nonexistent/agent".to_string()];
        let err = spawn(&SpawnSpec { argv: &argv, env: &[], cwd: None, cols: 80, rows: 24 }).err().unwrap();
        assert_eq!(err.raw_os_error(), Some(libc::ENOENT));
        let argv = vec!["/bin/sh".to_string()];
        let err = spawn(&SpawnSpec { argv: &argv, env: &[], cwd: Some("/nonexistent-dir"), cols: 80, rows: 24 }).err().unwrap();
        assert_eq!(err.raw_os_error(), Some(libc::ENOENT));
    }
}
