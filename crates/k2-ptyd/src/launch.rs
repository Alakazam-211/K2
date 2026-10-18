//! Launching ptyd (PRD §7.1, P5, P6, V29).
//!
//! `k2-ptyd --launch` forks; the child calls `setsid` and forks again into
//! the real server; the middle process exits at once and the launcher
//! reaps it. The launcher itself exits after the server reports
//! listening (`R`) or that another instance owns the home (`B`). The caller
//! (the daemon) reaps the launcher, so ptyd is never the daemon's child and
//! no zombie is left behind. setsid puts ptyd outside launchd's group kill
//! on `kickstart -k` / `unload` / `bootout` (S0: verified on macOS 27).

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::Path;
use std::process::{Command, Stdio};

use crate::server::{self, Config, Exit};
use crate::sys::{cvt, set_cloexec};

/// Exit codes of the launcher / `--serve`.
pub const EXIT_OK: i32 = 0;
pub const EXIT_FAILED: i32 = 1;
pub const EXIT_ALREADY_RUNNING: i32 = 3;

/// The launcher body (runs in a fresh, single-threaded `k2-ptyd` process).
pub fn launcher(cfg_without_ready: Config) -> i32 {
    let mut p = [0 as RawFd; 2];
    // SAFETY: pipe fills two fds.
    if unsafe { libc::pipe(p.as_mut_ptr()) } != 0 {
        eprintln!("k2-ptyd: pipe: {}", io::Error::last_os_error());
        return EXIT_FAILED;
    }
    // SAFETY: owned from here.
    let r = unsafe { OwnedFd::from_raw_fd(p[0]) };
    let w = p[1];
    let _ = set_cloexec(r.as_raw_fd(), true);
    let _ = set_cloexec(w, true);
    // SAFETY: single-threaded process; both children only run our code.
    let middle = unsafe { libc::fork() };
    if middle < 0 {
        eprintln!("k2-ptyd: fork: {}", io::Error::last_os_error());
        return EXIT_FAILED;
    }
    if middle == 0 {
        // SAFETY: new session, then the real server in a grandchild.
        unsafe {
            libc::setsid();
            let gc = libc::fork();
            if gc != 0 {
                libc::_exit(if gc < 0 { EXIT_FAILED } else { EXIT_OK });
            }
            libc::close(p[0]);
            let dn = libc::open(c"/dev/null".as_ptr(), libc::O_RDWR);
            if dn >= 0 {
                libc::dup2(dn, 0);
                libc::dup2(dn, 1);
                libc::dup2(dn, 2);
                if dn > 2 {
                    libc::close(dn);
                }
            }
        }
        let cfg = Config { ready_fd: Some(w), ..cfg_without_ready };
        let code = match server::serve(cfg) {
            Ok(Exit::Clean) => EXIT_OK,
            Ok(Exit::AlreadyRunning(_)) => EXIT_ALREADY_RUNNING,
            Err(_) => EXIT_FAILED,
        };
        // SAFETY: leave without running the launcher's atexit state twice.
        unsafe { libc::_exit(code) }
    }
    // SAFETY: close our copy of the write end.
    unsafe { libc::close(w) };
    let mut st = 0;
    // SAFETY: reap the middle process (P6: no zombie).
    unsafe { libc::waitpid(middle, &mut st, 0) };
    let mut b = [0u8; 1];
    loop {
        // SAFETY: blocking read of the status byte.
        let n = unsafe { libc::read(r.as_raw_fd(), b.as_mut_ptr() as *mut libc::c_void, 1) };
        if n < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            continue;
        }
        return match (n, b[0]) {
            (1, b'R') => EXIT_OK,
            (1, b'B') => {
                eprintln!("k2-ptyd: already running for this K2 home");
                EXIT_ALREADY_RUNNING
            }
            _ => {
                eprintln!("k2-ptyd: server failed to start (see the ptyd log)");
                EXIT_FAILED
            }
        };
    }
}

/// Daemon side: run `<exe> --launch` with the §7.1 env allowlist and wait
/// for it (this reaps the launcher). `Ok(())` when a ptyd is listening for
/// `home` afterwards (freshly started or already running).
pub fn spawn_detached(exe: &Path, home: &Path, extra: &[(&str, &str)]) -> io::Result<()> {
    let mut cmd = Command::new(exe);
    cmd.arg("--launch").env_clear();
    cmd.envs(launch_env(std::env::vars_os(), home, extra));
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    let out = cmd.output()?;
    match out.status.code() {
        Some(EXIT_OK) | Some(EXIT_ALREADY_RUNNING) => Ok(()),
        other => Err(io::Error::other(format!(
            "k2-ptyd --launch exited {:?}: {}",
            other,
            String::from_utf8_lossy(&out.stderr).trim()
        ))),
    }
}

/// The complete environment ptyd is launched with (P5): the allowlisted
/// names from `parent`, `K2_HOME` = `home`, then allowlisted `extra`
/// entries. Nothing else — whatever the parent carries.
pub fn launch_env(
    parent: impl IntoIterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
    home: &Path,
    extra: &[(&str, &str)],
) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    let mut env: std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString> = parent
        .into_iter()
        .filter(|(k, _)| k.to_str().map(|n| crate::env_allowed(n) && n != "K2_HOME").unwrap_or(false))
        .collect();
    env.insert("K2_HOME".into(), home.as_os_str().to_owned());
    for (k, v) in extra {
        if crate::env_allowed(k) {
            env.insert((*k).into(), (*v).into());
        }
    }
    env.into_iter().collect()
}

/// `pipe` helper kept for callers that pass a ready fd to `--serve`.
pub fn pipe_cloexec() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut p = [0 as RawFd; 2];
    // SAFETY: pipe fills two fds.
    cvt(unsafe { libc::pipe(p.as_mut_ptr()) })?;
    set_cloexec(p[0], true)?;
    set_cloexec(p[1], true)?;
    // SAFETY: both fresh.
    Ok(unsafe { (OwnedFd::from_raw_fd(p[0]), OwnedFd::from_raw_fd(p[1])) })
}
