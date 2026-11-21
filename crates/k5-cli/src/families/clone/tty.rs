//! The clone engine's terminal and its stops, as the shell's Python has
//! them (`cli/src/clone.sh`): `open_tty`, `read_line`, `password`,
//! `ask_choice`, and Ctrl-C (`KeyboardInterrupt`) or SIGTERM and SIGHUP
//! during a download (`SystemExit(128 + signum)`).
//!
//! A password is read from this person's terminal only (`/dev/tty`
//! opened for real, `O_RDWR|O_NOCTTY`), echo off with `TCSAFLUSH`, never
//! from stdin. It is a [`Secret`] from the first byte: wiped when dropped.

use super::engine::Stop;
use crate::prompt::Secret;
use std::sync::atomic::{AtomicI32, Ordering};

/// `NEEDS_A_PERSON`.
pub const NEEDS_A_PERSON: &str =
    "this needs your confirm: run it from a terminal (not --yes, not a pipe)";

/// The signal that came and is not yet answered (0: none).
static PENDING: AtomicI32 = AtomicI32::new(0);

extern "C" fn on_signal(sig: libc::c_int) {
    PENDING.store(sig, Ordering::SeqCst);
}

/// Catches `sig` (no `SA_RESTART`, so a blocked read stops with EINTR)
/// until the guard goes; the handler before it is put back then.
pub struct Caught {
    sig: libc::c_int,
    old: libc::sigaction,
}

impl Caught {
    pub fn start(sig: libc::c_int) -> Option<Caught> {
        // SAFETY: a zeroed sigaction with our handler; sigaction fills old.
        unsafe {
            let mut new: libc::sigaction = std::mem::zeroed();
            new.sa_sigaction = on_signal as extern "C" fn(libc::c_int) as usize;
            new.sa_flags = 0;
            libc::sigemptyset(&mut new.sa_mask);
            let mut old: libc::sigaction = std::mem::zeroed();
            (libc::sigaction(sig, &new, &mut old) == 0).then_some(Caught { sig, old })
        }
    }
}

impl Drop for Caught {
    fn drop(&mut self) {
        // SAFETY: puts back the action sigaction gave us.
        unsafe {
            libc::sigaction(self.sig, &self.old, std::ptr::null_mut());
        }
    }
}

/// The signal that came since the last look, taken (Python raises once).
/// SIGINT is `KeyboardInterrupt` ([`Stop::Cancelled`]); SIGTERM and SIGHUP
/// are `SystemExit(128 + signum)`.
pub fn take() -> Option<Stop> {
    match PENDING.swap(0, Ordering::SeqCst) {
        0 => None,
        libc::SIGINT => Some(Stop::Cancelled),
        sig => Some(Stop::Exit(128 + sig)),
    }
}

/// A signal came and is not yet answered.
pub fn pending() -> bool {
    PENDING.load(Ordering::SeqCst) != 0
}

/// Raises what [`take`] found.
pub fn check() -> Result<(), Stop> {
    take().map_or(Ok(()), Err)
}

/// `open_tty()`: this person's terminal, opened for real, or `None`.
pub fn open_tty() -> Option<i32> {
    // SAFETY: a plain open of a fixed path.
    let fd = unsafe {
        libc::open(
            c"/dev/tty".as_ptr(),
            libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC,
        )
    };
    (fd >= 0).then_some(fd)
}

/// `has_tty()`.
pub fn has_tty() -> bool {
    match open_tty() {
        Some(fd) => {
            close(fd);
            true
        }
        None => false,
    }
}

pub fn close(fd: i32) {
    // SAFETY: fd is ours.
    unsafe {
        libc::close(fd);
    }
}

/// `os.write(fd, data)` all of it; an error is the `OSError`.
fn write_all(fd: i32, mut data: &[u8]) -> Result<(), ()> {
    while !data.is_empty() {
        // SAFETY: writes from a live slice.
        let n = unsafe { libc::write(fd, data.as_ptr().cast(), data.len()) };
        if n < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                check_raw()?;
                continue;
            }
            return Err(());
        }
        data = &data[n as usize..];
    }
    Ok(())
}

/// A pending Ctrl-C, seen inside a terminal read or write: kept for
/// [`read_line`] to raise.
fn check_raw() -> Result<(), ()> {
    if PENDING.load(Ordering::SeqCst) != 0 {
        return Err(());
    }
    Ok(())
}

/// `read_line(fd, prompt, needs)`: the prompt on the terminal, then one
/// typed line, newline and all. The end of the input is Ctrl-D: stopped
/// (`KeyboardInterrupt`). A terminal that can't be written or read is no
/// person here.
pub fn read_line(fd: i32, prompt: &str, needs: &str) -> Result<Secret, Stop> {
    let mut typed: Vec<u8> = Vec::new();
    let r = (|| -> Result<(), Stop> {
        let failed = |typed: &mut Vec<u8>| {
            wipe(typed);
            take().unwrap_or_else(|| Stop::needs(needs))
        };
        if write_all(fd, prompt.as_bytes()).is_err() {
            return Err(failed(&mut typed));
        }
        while typed.last() != Some(&b'\n') {
            let mut buf = [0u8; 1024];
            // SAFETY: reads into a live buffer.
            let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
            if n < 0 {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    if let Some(stop) = take() {
                        wipe(&mut buf);
                        return Err(stop);
                    }
                    continue;
                }
                return Err(failed(&mut typed));
            }
            if n == 0 {
                return Err(Stop::Cancelled);
            }
            grow(&mut typed, &buf[..n as usize]);
            wipe(&mut buf);
        }
        Ok(())
    })();
    match r {
        Ok(()) => Ok(Secret::new(typed)),
        Err(stop) => {
            wipe(&mut typed);
            Err(stop)
        }
    }
}

fn wipe(b: &mut [u8]) {
    for x in b.iter_mut() {
        // SAFETY: a volatile write into a live slice, so it isn't dropped.
        unsafe { std::ptr::write_volatile(x, 0) };
    }
}

/// `typed += more`, wiping the old buffer when it must grow.
fn grow(typed: &mut Vec<u8>, more: &[u8]) {
    if typed.capacity() < typed.len() + more.len() {
        let mut bigger = Vec::with_capacity((typed.len() + more.len()).max(64) * 2);
        bigger.extend_from_slice(typed);
        wipe(typed);
        *typed = bigger;
    }
    typed.extend_from_slice(more);
}

/// `password(prompt)`: a password typed at this person's terminal with
/// echo off (`TCSAFLUSH`), the terminal put back and a newline written
/// there after, whatever happened. `tty`: the engine's `TTY`. The line
/// less its last byte, as bytes (the caller decodes it as UTF-8 with
/// replacement, as Python does).
pub fn password(tty: bool, prompt: &str) -> Result<Secret, Stop> {
    let fd = if tty { open_tty() } else { None };
    let Some(fd) = fd else {
        return Err(Stop::needs(NEEDS_A_PERSON));
    };
    let r = (|| {
        let mut old = std::mem::MaybeUninit::<libc::termios>::uninit();
        // SAFETY: tcgetattr fills the struct.
        if unsafe { libc::tcgetattr(fd, old.as_mut_ptr()) } != 0 {
            return Err(Stop::needs(NEEDS_A_PERSON));
        }
        // SAFETY: filled above.
        let old = unsafe { old.assume_init() };
        let mut quiet = old;
        quiet.c_lflag &= !libc::ECHO;
        let _int = Caught::start(libc::SIGINT);
        // SAFETY: sets the terminal from a valid struct.
        unsafe { libc::tcsetattr(fd, libc::TCSAFLUSH, &quiet) };
        let typed = read_line(fd, prompt, NEEDS_A_PERSON);
        // SAFETY: puts the terminal back.
        unsafe { libc::tcsetattr(fd, libc::TCSAFLUSH, &old) };
        let _ = write_all(fd, b"\n");
        typed
    })();
    close(fd);
    let typed = r?;
    let b = typed.expose();
    Ok(Secret::new(b[..b.len().saturating_sub(1)].to_vec()))
}

/// `ask_choice(row, default)`'s terminal: the line written, one line read
/// (`readline()`; the end of the input is an empty line).
pub struct Choice {
    fd: i32,
}

impl Choice {
    /// `open("/dev/tty", "r+")`.
    pub fn open() -> Result<Choice, Stop> {
        open_tty().map(|fd| Choice { fd }).ok_or_else(|| {
            Stop::Raised("OSError: [Errno 6] No such device or address: '/dev/tty'".into())
        })
    }

    pub fn write(&self, text: &str) -> Result<(), Stop> {
        write_all(self.fd, text.as_bytes())
            .map_err(|()| take().unwrap_or_else(|| Stop::Raised("OSError: [Errno 5] Input/output error".into())))
    }

    /// `t.readline()`.
    pub fn readline(&self) -> Result<String, Stop> {
        let mut line = Vec::new();
        loop {
            let mut b = [0u8; 1];
            // SAFETY: reads into a live byte.
            let n = unsafe { libc::read(self.fd, b.as_mut_ptr().cast(), 1) };
            if n < 0 {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    check()?;
                    continue;
                }
                return Err(Stop::Raised("OSError: [Errno 5] Input/output error".into()));
            }
            if n == 0 {
                break;
            }
            line.push(b[0]);
            if b[0] == b'\n' {
                break;
            }
        }
        Ok(String::from_utf8_lossy(&line).into_owned())
    }
}

impl Drop for Choice {
    fn drop(&mut self) {
        close(self.fd);
    }
}
