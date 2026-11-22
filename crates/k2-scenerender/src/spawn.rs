//! Start the worker with `posix_spawn` in a session of its own
//! (`POSIX_SPAWN_SETSID`: glibc 2.26+, macOS), so it is never in the
//! daemon's session, without the fork a pre-exec hook would cost (36–54 ms
//! from a ~1 GB daemon, against ~1 ms). std's `Command` can't ask for the
//! session on its `posix_spawn` path (`CommandExt::setsid` is unstable), so
//! this calls it directly. Unix only.
//!
//! The caller builds a [`Command`] as before (program, args, an empty
//! environment, a cwd); [`spawn`] reads them back and starts it with:
//! - stdin, stdout and stderr on pipes (the parent's ends close on exec);
//! - an empty environment (the hosts clear it; a variable set on the
//!   `Command` refuses the spawn rather than be dropped silently);
//! - the cwd through `posix_spawn_file_actions_addchdir_np`;
//! - an empty signal mask and SIGPIPE back to its default, as std does;
//! - on macOS `POSIX_SPAWN_CLOEXEC_DEFAULT`: no other fd of the daemon
//!   reaches the worker (on Linux every fd K2 opens is close-on-exec, and
//!   the worker closes anything past stdio first thing in `main`).
//!
//! This file is the same in `k2-scenerender` and `k2-deckrender`; a test in
//! k2-core keeps the two copies identical.

use std::ffi::{CString, OsStr};
use std::fs::File;
use std::io;
use std::os::fd::{FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, ExitStatus};

#[cfg(target_os = "linux")]
const POSIX_SPAWN_SETSID: libc::c_short = 0x80;
#[cfg(target_vendor = "apple")]
const POSIX_SPAWN_SETSID: libc::c_short = 0x0400;
#[cfg(target_vendor = "apple")]
const POSIX_SPAWN_CLOEXEC_DEFAULT: libc::c_short = 0x4000;

extern "C" {
    fn posix_spawn_file_actions_addchdir_np(actions: *mut libc::posix_spawn_file_actions_t, path: *const libc::c_char) -> libc::c_int;
}

/// A started worker: what the hosts use of `std::process::Child`.
pub struct Worker {
    pid: libc::pid_t,
    status: Option<ExitStatus>,
    pub stdin: Option<File>,
    pub stdout: Option<File>,
    pub stderr: Option<File>,
}

impl Worker {
    pub fn id(&self) -> u32 {
        self.pid as u32
    }

    /// SIGKILL, unless it was already reaped.
    pub fn kill(&mut self) -> io::Result<()> {
        if self.status.is_some() {
            return Ok(());
        }
        // SAFETY: a signal to the child this process started and hasn't reaped.
        if unsafe { libc::kill(self.pid, libc::SIGKILL) } == -1 {
            let e = io::Error::last_os_error();
            if e.raw_os_error() != Some(libc::ESRCH) {
                return Err(e);
            }
        }
        Ok(())
    }

    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        loop {
            if let Some(s) = self.reap(0)? {
                return Ok(s);
            }
        }
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.reap(libc::WNOHANG)
    }

    fn reap(&mut self, flags: libc::c_int) -> io::Result<Option<ExitStatus>> {
        if let Some(s) = self.status {
            return Ok(Some(s));
        }
        let mut st: libc::c_int = 0;
        // SAFETY: waits on this process's own child.
        let r = unsafe { libc::waitpid(self.pid, &mut st, flags) };
        match r {
            0 => Ok(None),
            -1 => {
                let e = io::Error::last_os_error();
                if e.raw_os_error() == Some(libc::EINTR) {
                    Ok(None)
                } else {
                    Err(e)
                }
            }
            _ => {
                let s = ExitStatus::from_raw(st);
                self.status = Some(s);
                Ok(Some(s))
            }
        }
    }
}

fn cstr(s: &OsStr) -> io::Result<CString> {
    CString::new(s.as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "a NUL byte in the worker's argv or path"))
}

fn check(rc: libc::c_int) -> io::Result<()> {
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(rc))
    }
}

/// A pipe, both ends close-on-exec: (read, write).
fn pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds: [RawFd; 2] = [-1, -1];
    #[cfg(target_os = "linux")]
    // SAFETY: fills two fds.
    let rc = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) };
    #[cfg(not(target_os = "linux"))]
    // SAFETY: fills two fds; CLOEXEC set right after (macOS has no pipe2).
    let rc = unsafe {
        let r = libc::pipe(fds.as_mut_ptr());
        if r == 0 {
            libc::fcntl(fds[0], libc::F_SETFD, libc::FD_CLOEXEC);
            libc::fcntl(fds[1], libc::F_SETFD, libc::FD_CLOEXEC);
        }
        r
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: two fresh fds this function owns.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

struct Actions(libc::posix_spawn_file_actions_t);
impl Drop for Actions {
    fn drop(&mut self) {
        // SAFETY: initialised in `spawn` before this guard exists.
        unsafe { libc::posix_spawn_file_actions_destroy(&mut self.0) };
    }
}
struct Attr(libc::posix_spawnattr_t);
impl Drop for Attr {
    fn drop(&mut self) {
        // SAFETY: initialised in `spawn` before this guard exists.
        unsafe { libc::posix_spawnattr_destroy(&mut self.0) };
    }
}

/// Start `cmd` in a session of its own (see the module).
pub fn spawn(cmd: &Command) -> io::Result<Worker> {
    if cmd.get_envs().next().is_some() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "the worker starts with an empty environment; none may be set"));
    }
    let program = cstr(cmd.get_program())?;
    let cwd = cmd.get_current_dir().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "the worker needs a cwd"))?;
    let cwd = cstr(cwd.as_os_str())?;
    let mut argv_c: Vec<CString> = vec![program.clone()];
    for a in cmd.get_args() {
        argv_c.push(cstr(a)?);
    }
    let mut argv: Vec<*mut libc::c_char> = argv_c.iter().map(|a| a.as_ptr() as *mut libc::c_char).collect();
    argv.push(std::ptr::null_mut());
    let mut envp: Vec<*mut libc::c_char> = vec![std::ptr::null_mut()];

    let (in_r, in_w) = pipe()?;
    let (out_r, out_w) = pipe()?;
    let (err_r, err_w) = pipe()?;
    use std::os::fd::AsRawFd;

    // SAFETY: plain posix_spawn setup on zeroed structs, each destroyed by
    // its guard; every pointer passed lives until posix_spawn returns.
    unsafe {
        let mut fa: libc::posix_spawn_file_actions_t = std::mem::zeroed();
        check(libc::posix_spawn_file_actions_init(&mut fa))?;
        let mut fa = Actions(fa);
        check(libc::posix_spawn_file_actions_adddup2(&mut fa.0, in_r.as_raw_fd(), 0))?;
        check(libc::posix_spawn_file_actions_adddup2(&mut fa.0, out_w.as_raw_fd(), 1))?;
        check(libc::posix_spawn_file_actions_adddup2(&mut fa.0, err_w.as_raw_fd(), 2))?;
        check(posix_spawn_file_actions_addchdir_np(&mut fa.0, cwd.as_ptr()))?;

        let mut at: libc::posix_spawnattr_t = std::mem::zeroed();
        check(libc::posix_spawnattr_init(&mut at))?;
        let mut at = Attr(at);
        let mut empty: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut empty);
        check(libc::posix_spawnattr_setsigmask(&mut at.0, &empty))?;
        let mut pipe_default: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut pipe_default);
        libc::sigaddset(&mut pipe_default, libc::SIGPIPE);
        check(libc::posix_spawnattr_setsigdefault(&mut at.0, &pipe_default))?;
        #[allow(unused_mut)]
        let mut flags = POSIX_SPAWN_SETSID | libc::POSIX_SPAWN_SETSIGMASK as libc::c_short | libc::POSIX_SPAWN_SETSIGDEF as libc::c_short;
        #[cfg(target_vendor = "apple")]
        {
            flags |= POSIX_SPAWN_CLOEXEC_DEFAULT;
        }
        check(libc::posix_spawnattr_setflags(&mut at.0, flags))?;

        let mut pid: libc::pid_t = 0;
        check(libc::posix_spawn(&mut pid, program.as_ptr(), &fa.0, &at.0, argv.as_mut_ptr(), envp.as_mut_ptr()))?;
        drop((in_r, out_w, err_w));
        Ok(Worker {
            pid,
            status: None,
            stdin: Some(File::from(in_w)),
            stdout: Some(File::from(out_r)),
            stderr: Some(File::from(err_r)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn a_worker_leads_its_own_session() {
        let mut cmd = Command::new("/bin/sleep");
        cmd.arg("5").env_clear().current_dir("/");
        let mut w = spawn(&cmd).expect("spawn /bin/sleep");
        let pid = w.id() as i32;
        // SAFETY: getsid reads session ids.
        let (theirs, ours) = unsafe { (libc::getsid(pid), libc::getsid(0)) };
        w.kill().unwrap();
        let st = w.wait().unwrap();
        assert_eq!(theirs, pid, "the worker leads its own session");
        assert_ne!(theirs, ours, "the worker is not in this process's session");
        assert_eq!(st.signal(), Some(libc::SIGKILL));
        assert!(w.try_wait().unwrap().is_some(), "reaped once, the status stays");
    }

    #[test]
    fn pipes_cwd_args_and_an_empty_environment() {
        let dir = std::env::temp_dir();
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", "read x; echo \"got $x in $(pwd) env=$(env | wc -l | tr -d ' ')\"; echo err >&2; exit 3"]).env_clear().current_dir(&dir);
        let mut w = spawn(&cmd).expect("spawn /bin/sh");
        w.stdin.take().unwrap().write_all(b"hello\n").unwrap();
        let mut out = String::new();
        w.stdout.take().unwrap().read_to_string(&mut out).unwrap();
        let mut err = String::new();
        w.stderr.take().unwrap().read_to_string(&mut err).unwrap();
        let st = w.wait().unwrap();
        let real = std::fs::canonicalize(&dir).unwrap();
        assert!(out.starts_with(&format!("got hello in {}", real.display())), "{out}");
        assert_eq!(err, "err\n");
        assert_eq!(st.code(), Some(3));
        // macOS adds __CF_USER_TEXT_ENCODING on its own; sh adds PWD and the like.
        let n: usize = out.trim().rsplit('=').next().unwrap().parse().unwrap();
        assert!(n <= 6, "an (almost) empty environment: {out}");
    }

    #[test]
    fn a_set_variable_refuses_rather_than_vanish() {
        let mut cmd = Command::new("/bin/true");
        cmd.env_clear().env("X", "1").current_dir("/");
        assert!(spawn(&cmd).is_err());
    }

    /// Not a check: the spawn cost from a process holding `K2_SPAWN_BENCH_MB`
    /// of touched memory, this helper against a fork with a pre-exec hook.
    #[test]
    #[ignore = "a measurement, run by hand (K2_SPAWN_BENCH_MB=1024)"]
    fn spawn_cost_from_a_large_process() {
        let mb: usize = std::env::var("K2_SPAWN_BENCH_MB").ok().and_then(|v| v.parse().ok()).unwrap_or(1024);
        let mut ballast = vec![0u8; mb << 20];
        for i in (0..ballast.len()).step_by(4096) {
            ballast[i] = 1;
        }
        let n = 20;
        // macOS has `true` only in /usr/bin.
        let truth = ["/usr/bin/true", "/bin/true"].into_iter().find(|p| std::path::Path::new(p).exists()).expect("a true(1)");
        let mut cmd = Command::new(truth);
        cmd.env_clear().current_dir("/");
        let t = std::time::Instant::now();
        for _ in 0..n {
            let mut w = spawn(&cmd).unwrap();
            w.wait().unwrap();
        }
        let ours = t.elapsed() / n;
        let t = std::time::Instant::now();
        for _ in 0..n {
            use std::os::unix::process::CommandExt;
            let mut c = Command::new(truth);
            c.env_clear().current_dir("/");
            // SAFETY: setsid in the child, async-signal-safe.
            unsafe {
                c.pre_exec(|| {
                    libc::setsid();
                    Ok(())
                });
            }
            c.status().unwrap();
        }
        let forked = t.elapsed() / n;
        println!("spawn cost with {mb} MiB touched: posix_spawn + SETSID {ours:?} per spawn, fork + pre-exec setsid {forked:?} per spawn");
        assert!(ballast[4096] == 1);
    }
}
