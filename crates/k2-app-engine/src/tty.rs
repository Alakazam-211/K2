//! `k2 app shell` (S28) on the engine's side: a command on a new
//! terminal, bridged to the daemon over the engine socket with
//! `k2_root_proto::engine::tty` frames.
//!
//! The engine runs `podman exec -it` (so the shell lands inside the App's
//! container as the App, with its capabilities, seccomp and MAC profile:
//! the OCI shim patches the exec process) with this terminal as its
//! controlling tty. Bytes are passed through untouched: the daemon strips
//! escape sequences before they reach a person or an agent.

use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

use k2_root_proto::engine::tty;

fn os_err() -> std::io::Error {
    std::io::Error::last_os_error()
}

/// A new pseudo-terminal: `(master, slave)`, sized `cols`×`rows`.
pub fn open(cols: u16, rows: u16) -> std::io::Result<(OwnedFd, OwnedFd)> {
    // SAFETY: plain libc calls on fds this function owns.
    unsafe {
        let m = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
        if m < 0 {
            return Err(os_err());
        }
        let master = OwnedFd::from_raw_fd(m);
        let _ = libc::fcntl(m, libc::F_SETFD, libc::FD_CLOEXEC);
        if libc::grantpt(m) != 0 || libc::unlockpt(m) != 0 {
            return Err(os_err());
        }
        let name = slave_name(m)?;
        let s = libc::open(name.as_ptr(), libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC);
        if s < 0 {
            return Err(os_err());
        }
        let slave = OwnedFd::from_raw_fd(s);
        // Raw from the start: this terminal only carries bytes to
        // `podman exec -it`, whose own terminal inside the container does
        // the line discipline. A cooked outer terminal echoed input twice
        // and, for input that arrived before podman switched it to raw
        // (piped stdin), turned the client's closing ^D into an EOF that
        // ended podman's input with lines still unread.
        let mut t: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(s, &mut t) != 0 {
            return Err(os_err());
        }
        libc::cfmakeraw(&mut t);
        if libc::tcsetattr(s, libc::TCSANOW, &t) != 0 {
            return Err(os_err());
        }
        set_size(master.as_raw_fd(), cols, rows)?;
        Ok((master, slave))
    }
}

#[cfg(target_os = "linux")]
fn slave_name(m: RawFd) -> std::io::Result<std::ffi::CString> {
    let mut buf = vec![0 as libc::c_char; 128];
    // SAFETY: buf is valid for its length.
    if unsafe { libc::ptsname_r(m, buf.as_mut_ptr(), buf.len()) } != 0 {
        return Err(os_err());
    }
    // SAFETY: ptsname_r NUL-terminated it.
    Ok(unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) }.to_owned())
}

#[cfg(not(target_os = "linux"))]
fn slave_name(m: RawFd) -> std::io::Result<std::ffi::CString> {
    // SAFETY: ptsname returns a static buffer; copied out at once (tests
    // only off Linux).
    let p = unsafe { libc::ptsname(m) };
    if p.is_null() {
        return Err(os_err());
    }
    Ok(unsafe { std::ffi::CStr::from_ptr(p) }.to_owned())
}

/// `TIOCSWINSZ` on the master (the command gets SIGWINCH).
pub fn set_size(master: RawFd, cols: u16, rows: u16) -> std::io::Result<()> {
    let ws = libc::winsize { ws_row: rows.max(1), ws_col: cols.max(1), ws_xpixel: 0, ws_ypixel: 0 };
    // SAFETY: a valid fd and a valid winsize.
    if unsafe { libc::ioctl(master, libc::TIOCSWINSZ, &ws) } != 0 {
        return Err(os_err());
    }
    Ok(())
}

/// Run `cmd` with `slave` as its controlling terminal and bridge it to
/// `conn` until it exits. Returns its exit code (-1 when killed). The
/// EXIT frame is the last thing written.
pub fn bridge(mut cmd: Command, cols: u16, rows: u16, conn: UnixStream) -> std::io::Result<i32> {
    let (master, slave) = open(cols, rows)?;
    let s = slave.as_raw_fd();
    let dup = |fd: RawFd| -> std::io::Result<Stdio> {
        // SAFETY: dup of a valid fd; the new fd is owned by the Stdio.
        let d = unsafe { libc::dup(fd) };
        if d < 0 {
            return Err(os_err());
        }
        Ok(unsafe { Stdio::from(OwnedFd::from_raw_fd(d)) })
    };
    cmd.stdin(dup(s)?).stdout(dup(s)?).stderr(dup(s)?);
    // SAFETY: async-signal-safe calls only, in the child before exec.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            #[allow(clippy::useless_conversion)]
            if libc::ioctl(0, libc::TIOCSCTTY.into(), 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = cmd.spawn()?;
    drop(slave);
    let pid = child.id() as libc::pid_t;

    // Terminal → daemon.
    let mut out_conn = conn.try_clone()?;
    let mfd = master.as_raw_fd();
    // SAFETY: a dup the reader thread owns.
    let rd = unsafe { libc::dup(mfd) };
    if rd < 0 {
        return Err(os_err());
    }
    let mut reader = unsafe { std::fs::File::from_raw_fd(rd) };
    let out = std::thread::spawn(move || {
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            match reader.read(&mut buf) {
                // EIO: every copy of the slave is closed (the command ended).
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if out_conn.write_all(&tty::encode(tty::OUTPUT, &buf[..n])).is_err() {
                        break;
                    }
                }
            }
        }
        out_conn
    });

    // Daemon → terminal.
    let mut in_conn = conn.try_clone()?;
    // SAFETY: a dup the writer thread owns.
    let wd = unsafe { libc::dup(mfd) };
    if wd < 0 {
        return Err(os_err());
    }
    let mut writer = unsafe { std::fs::File::from_raw_fd(wd) };
    let _input = std::thread::spawn(move || {
        loop {
            match tty::read(&mut in_conn) {
                Ok(Some((tty::INPUT, b))) => {
                    if writer.write_all(&b).is_err() {
                        break;
                    }
                }
                Ok(Some((tty::RESIZE, b))) if b.len() == 4 => {
                    let cols = u16::from_be_bytes([b[0], b[1]]);
                    let rows = u16::from_be_bytes([b[2], b[3]]);
                    let _ = set_size(writer.as_raw_fd(), cols, rows);
                }
                Ok(Some((tty::HANGUP, _))) | Ok(None) | Err(_) => {
                    // The person left: hang the terminal up.
                    // SAFETY: signalling our own child's process group.
                    unsafe {
                        libc::kill(-pid, libc::SIGHUP);
                        libc::kill(pid, libc::SIGHUP);
                    }
                    break;
                }
                Ok(Some(_)) => {}
            }
        }
    });

    let status = child.wait()?;
    drop(master);
    let code = status.code().unwrap_or(-1);
    let mut out_conn = out.join().map_err(|_| std::io::Error::other("output thread panicked"))?;
    let _ = out_conn.write_all(&tty::encode(tty::EXIT, &code.to_be_bytes()));
    let _ = out_conn.flush();
    let _ = conn.shutdown(std::net::Shutdown::Both);
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real command on a real pty: its output arrives as OUTPUT frames,
    /// input reaches it, the EXIT frame carries its code and comes last.
    #[test]
    fn a_command_runs_on_a_terminal_and_its_exit_comes_last() {
        let (daemon, engine) = UnixStream::pair().unwrap();
        let mut c = Command::new("/bin/sh");
        c.args(["-c", "read line; echo \"got:$line\"; test -t 0 && echo tty; exit 7"]);
        c.env_clear().env("PATH", "/usr/bin:/bin");
        let t = std::thread::spawn(move || bridge(c, 80, 24, engine).unwrap());
        let mut w = daemon.try_clone().unwrap();
        w.write_all(&tty::encode(tty::RESIZE, &[0, 100, 0, 30])).unwrap();
        w.write_all(&tty::encode(tty::INPUT, b"hello\n")).unwrap();
        let mut r = daemon;
        let mut text = Vec::new();
        let mut exit = None;
        while let Some((k, p)) = tty::read(&mut r).unwrap() {
            match k {
                tty::OUTPUT => {
                    assert!(exit.is_none(), "output after the exit frame");
                    text.extend_from_slice(&p);
                }
                tty::EXIT => exit = Some(i32::from_be_bytes([p[0], p[1], p[2], p[3]])),
                other => panic!("unexpected frame {other}"),
            }
        }
        assert_eq!(t.join().unwrap(), 7);
        assert_eq!(exit, Some(7));
        let text = String::from_utf8_lossy(&text);
        assert!(text.contains("got:hello"), "{text:?}");
        assert!(text.contains("tty"), "stdin is a terminal: {text:?}");
    }

    /// The engine's terminal is raw: input sent before the command reads
    /// it (piped stdin) is neither echoed nor cut at a ^D; every byte
    /// reaches the command.
    #[test]
    fn early_input_passes_through_untouched() {
        let (daemon, engine) = UnixStream::pair().unwrap();
        let mut w = daemon.try_clone().unwrap();
        w.write_all(&tty::encode(tty::INPUT, b"one\ntwo\n\x04")).unwrap();
        let mut c = Command::new("/bin/sh");
        c.args(["-c", "sleep 1; head -c 9 | od -An -c | tr -s ' '"]);
        c.env_clear().env("PATH", "/usr/bin:/bin");
        let t = std::thread::spawn(move || bridge(c, 80, 24, engine).unwrap());
        let mut r = daemon;
        let mut text = Vec::new();
        while let Some((k, p)) = tty::read(&mut r).unwrap() {
            if k == tty::OUTPUT {
                text.extend_from_slice(&p);
            }
        }
        assert_eq!(t.join().unwrap(), 0);
        let text = String::from_utf8_lossy(&text);
        assert!(text.contains("o n e \\n t w o \\n 004"), "every byte, the ^D included: {text:?}");
        assert!(!text.contains("one\r\n"), "no echo from the engine's terminal: {text:?}");
    }

    #[test]
    fn hanging_up_ends_the_command() {
        let (daemon, engine) = UnixStream::pair().unwrap();
        let mut c = Command::new("/bin/sh");
        c.args(["-c", "sleep 30"]);
        c.env_clear().env("PATH", "/usr/bin:/bin");
        let t = std::thread::spawn(move || bridge(c, 80, 24, engine).unwrap());
        let mut w = daemon.try_clone().unwrap();
        w.write_all(&tty::encode(tty::HANGUP, &[])).unwrap();
        let started = std::time::Instant::now();
        let code = t.join().unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(20), "hung up, not waited out");
        assert_ne!(code, 0);
    }
}
