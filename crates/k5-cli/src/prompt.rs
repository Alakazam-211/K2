//! Prompts, no-echo reads and secrets from stdin, as the shell CLI does
//! them. Shared by every family that asks a person something or takes a
//! secret on stdin (vault, app, users, llm, connect, compute and others).
//!
//! The shell's forms, and what answers each here:
//!
//! | shell | here |
//! |---|---|
//! | `[ -t 0 ]`, `[ -t 1 ]`, `[ -t 2 ]` | [`stdin_is_tty`], [`stdout_is_tty`], [`stderr_is_tty`] |
//! | `IFS= read -r v \|\| v=""` | [`read_line`]`(Echo::Shown, AtEof::Drop)` |
//! | `IFS= read -r v \|\| true` | [`read_line`]`(Echo::Shown, AtEof::Keep)` |
//! | `IFS= read -rs v \|\| v=""` | [`read_line`]`(Echo::Hidden, AtEof::Drop)` |
//! | `read -r v` under `set -e` (the end of the input ends the command) | [`read_line_or_end`] |
//! | `printf '<p>' >&2; IFS= read -rs v \|\| v=""; echo >&2` | [`ask_hidden`] |
//! | `printf '<p>' >&2; IFS= read -r v \|\| v=""` | [`ask`] |
//! | `_cli_read_secret_stdin "<p>"` (`cli/src/cli-tools-policy.sh`) | [`read_secret_stdin`] |
//! | `IFS= read -r v </dev/tty \|\| v=""` | [`read_dev_tty_line`] |
//! | `v="$(cat)"` | [`read_all_stdin`] |
//!
//! `read` reads fd 0 one byte at a time, as bash does on a pipe or a
//! terminal, so what follows the line stays on stdin for the next read
//! or for a program the command starts. NUL bytes are skipped, as bash's
//! `read` skips them. `-r` keeps backslashes and `IFS=` keeps every space.
//! At the end of the input with no newline, `read` fails with what it
//! read so far in the variable: the caller's `|| v=""` drops it
//! ([`AtEof::Drop`]), `|| true` keeps it ([`AtEof::Keep`]).
//!
//! `read -s` turns the terminal's echo off (`ECHO`, `ECHOK` and `ECHONL`,
//! as bash's `ttnoecho`) only when stdin is a terminal, and puts it back
//! after the line, or when SIGINT, SIGTERM, SIGHUP or SIGQUIT ends the
//! command during the read (bash restores it then too). Echo goes off
//! with `TCSAFLUSH`, as getpass does: what was typed before is dropped,
//! so a secret never comes from a line the terminal showed. A hidden
//! question ([`ask_hidden`]) turns echo off BEFORE its prompt shows, as
//! the shell's `_k2_read_hidden` does, so nothing typed after the prompt
//! can be echoed.
//!
//! What is read is a [`Secret`]: its bytes are wiped when it is dropped,
//! and its `Debug` never shows them. Nothing here puts a value on argv or
//! in the environment of any process.
//!
//! Each `--x-stdin` flag's wording (`--token-stdin`, `--password-stdin`,
//! `--key-stdin` ...) is its family's own, so it stays with the family;
//! the reading is here.

use std::fmt;
use std::io::{self, Write};

/// Bytes read from a person or from stdin: wiped on drop, never shown by
/// `Debug`.
#[derive(Default, Clone, PartialEq, Eq)]
pub struct Secret(Vec<u8>);

impl Secret {
    pub fn new(bytes: Vec<u8>) -> Secret {
        Secret(bytes)
    }

    /// The bytes. Callers put them in a request body, never on argv, in
    /// the environment or on stdout.
    pub fn expose(&self) -> &[u8] {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Takes the trailing newlines off, as `$(…)` does.
    pub fn trim_trailing_newlines(&mut self) {
        while self.0.last() == Some(&b'\n') {
            self.0.pop();
        }
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Secret({} bytes)", self.0.len())
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        for b in self.0.iter_mut() {
            // A volatile write, so the wipe isn't optimised away.
            unsafe { std::ptr::write_volatile(b, 0) };
        }
    }
}

/// `read -s` or not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Echo {
    Shown,
    /// No echo when stdin is a terminal.
    Hidden,
}

/// What the caller does when `read` meets the end of the input before a
/// newline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtEof {
    /// `|| v=""`: what was read is dropped.
    Drop,
    /// `|| true`: what was read stays.
    Keep,
}

/// `[ -t 0 ]`.
pub fn stdin_is_tty() -> bool {
    is_tty(0)
}

/// `[ -t 1 ]`.
pub fn stdout_is_tty() -> bool {
    is_tty(1)
}

/// `[ -t 2 ]`.
pub fn stderr_is_tty() -> bool {
    is_tty(2)
}

#[cfg(unix)]
fn is_tty(fd: i32) -> bool {
    // SAFETY: isatty only reads the descriptor's state.
    unsafe { libc::isatty(fd) == 1 }
}

#[cfg(not(unix))]
fn is_tty(fd: i32) -> bool {
    use std::io::IsTerminal;
    match fd {
        0 => io::stdin().is_terminal(),
        1 => io::stdout().is_terminal(),
        _ => io::stderr().is_terminal(),
    }
}

/// `printf '%s' "$text" >&2`. A stderr that can't be written is no error
/// to the shell's `printf` either way the caller goes on.
pub fn prompt(text: &str) {
    crate::prelude::plain_http::run();
    let mut e = io::stderr().lock();
    let _ = e.write_all(text.as_bytes());
    let _ = e.flush();
}

/// `IFS= read -r[s] v` on stdin, then the caller's `||` ([`AtEof`]).
pub fn read_line(echo: Echo, at_eof: AtEof) -> Secret {
    crate::prelude::plain_http::run();
    let hidden = echo == Echo::Hidden && stdin_is_tty();
    let guard = if hidden { NoEcho::start(0) } else { None };
    let (line, ended) = read_until_newline(&mut Fd0);
    drop(guard);
    finish(line, ended, at_eof)
}

/// `read -r[s] v` with no `||` under `set -e`: the line, or `None` when
/// the input ended before a newline (`read` fails, and the command ends
/// there with exit 1, saying nothing). A terminal's Ctrl-D on an empty
/// line is such an end, not an empty answer.
pub fn read_line_or_end(echo: Echo) -> Option<Secret> {
    crate::prelude::plain_http::run();
    let hidden = echo == Echo::Hidden && stdin_is_tty();
    let guard = if hidden { NoEcho::start(0) } else { None };
    let (line, ended) = read_until_newline(&mut Fd0);
    drop(guard);
    line_or_end(line, ended)
}

/// [`read_line_or_end`]'s outcome from what was read.
fn line_or_end(line: Vec<u8>, ended: bool) -> Option<Secret> {
    let line = Secret::new(line);
    ended.then_some(line)
}

/// `_k2_read_hidden v "$prompt"` (the shell's `printf '%s' "$prompt" >&2;
/// IFS= read -rs v || v=""; echo >&2`, echo off first): the no-echo
/// question. At a terminal echo goes off before the prompt shows
/// ([`no_echo`]), and comes back after the line, or when a signal ends
/// the command. The newline after it goes to stderr whatever was read.
pub fn ask_hidden(prompt_text: &str) -> Secret {
    crate::prelude::plain_http::run();
    let guard = no_echo();
    prompt(prompt_text);
    let (line, ended) = read_until_newline(&mut Fd0);
    drop(guard);
    prompt("\n");
    finish(line, ended, AtEof::Drop)
}

/// The terminal's echo off on stdin until the guard is dropped, when stdin
/// is a terminal (`None` otherwise, or when its settings can't be read).
/// A signal that ends the command meanwhile puts the settings back first.
pub(crate) fn no_echo() -> Option<NoEcho> {
    if stdin_is_tty() {
        NoEcho::start(0)
    } else {
        None
    }
}

/// `printf '%s' "$prompt" >&2; IFS= read -r v || v=""`: a question whose
/// answer is shown as it is typed.
pub fn ask(prompt_text: &str) -> Secret {
    prompt(prompt_text);
    read_line(Echo::Shown, AtEof::Drop)
}

/// `_cli_read_secret_stdin "$prompt"`: on a terminal, [`ask_hidden`];
/// else one line from the pipe, kept when it has no newline
/// (`IFS= read -r _K2_SECRET || true`). No prompt then.
pub fn read_secret_stdin(prompt_text: &str) -> Secret {
    if stdin_is_tty() {
        ask_hidden(prompt_text)
    } else {
        read_line(Echo::Shown, AtEof::Keep)
    }
}

/// `IFS= read -r v </dev/tty || v=""`: one line from the controlling
/// terminal, whatever stdin is. An error when there is none to open: the
/// shell prints its own `/dev/tty` line then, so callers check first, as
/// the shell's do (`[ -r /dev/tty ]`, [`dev_tty_readable`]).
#[cfg(unix)]
pub fn read_dev_tty_line() -> io::Result<Secret> {
    use std::os::unix::io::AsRawFd;
    let tty = std::fs::File::open("/dev/tty")?;
    let mut src = Raw(tty.as_raw_fd());
    let (line, ended) = read_until_newline(&mut src);
    Ok(finish(line, ended, AtEof::Drop))
}

#[cfg(not(unix))]
pub fn read_dev_tty_line() -> io::Result<Secret> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "no /dev/tty"))
}

/// `[ -r /dev/tty ]`: it can be opened for reading (there is a
/// controlling terminal).
pub fn dev_tty_readable() -> bool {
    std::fs::File::open("/dev/tty").is_ok()
}

/// How Python's `getpass.getpass` ended on the terminal.
#[derive(Debug, PartialEq, Eq)]
pub enum Getpass {
    /// The line, without its newline, as typed (NULs too).
    Line(Secret),
    /// The end of the input before anything was typed (`EOFError`), or
    /// Ctrl-C (`KeyboardInterrupt`): the caller's `except` says so.
    Cancelled,
}

/// `/dev/tty`, opened for reading and writing as Python's getpass opens
/// it (`O_RDWR|O_NOCTTY`), when it is a terminal whose settings can be
/// read (`tcgetattr`).
#[cfg(unix)]
fn getpass_tty_file() -> Option<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::io::AsRawFd;
    let f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOCTTY)
        .open("/dev/tty")
        .ok()?;
    let mut t = std::mem::MaybeUninit::<libc::termios>::uninit();
    // SAFETY: tcgetattr only fills the struct.
    (unsafe { libc::tcgetattr(f.as_raw_fd(), t.as_mut_ptr()) } == 0).then_some(f)
}

/// Python's getpass takes its first path here: a controlling terminal
/// whose echo it can turn off. Else it falls back to stdin, with a
/// warning that names its own source file and line, which the binary
/// can't print: a caller hands that case to the shell.
#[cfg(unix)]
pub fn getpass_tty_ok() -> bool {
    getpass_tty_file().is_some()
}

#[cfg(not(unix))]
pub fn getpass_tty_ok() -> bool {
    false
}

/// `getpass.getpass(prompt)` on the controlling terminal: the prompt
/// written there, its echo off for one line read from it, the terminal
/// put back, then a newline written there (not after an end of input or
/// Ctrl-C). Never stdin or stderr. `None`: no such terminal
/// ([`getpass_tty_ok`]).
#[cfg(unix)]
pub fn getpass_tty(prompt_text: &str) -> Option<Getpass> {
    use std::os::unix::io::AsRawFd;
    let tty = getpass_tty_file()?;
    Some(getpass_on(tty.as_raw_fd(), prompt_text))
}

#[cfg(not(unix))]
pub fn getpass_tty(_prompt_text: &str) -> Option<Getpass> {
    None
}

#[cfg(unix)]
mod cancel {
    //! Ctrl-C during [`super::getpass_on`]'s read: Python's
    //! `KeyboardInterrupt`, not the end of the process.
    use std::sync::atomic::{AtomicBool, Ordering};

    pub static HIT: AtomicBool = AtomicBool::new(false);

    pub extern "C" fn on_int(_sig: i32) {
        HIT.store(true, Ordering::SeqCst);
    }

    /// SIGINT caught (no `SA_RESTART`, so the read stops) until the
    /// guard goes; the handler before it is put back then.
    pub struct Caught(libc::sigaction);

    impl Caught {
        pub fn start() -> Option<Caught> {
            HIT.store(false, Ordering::SeqCst);
            // SAFETY: a zeroed sigaction with our handler and an empty
            // mask; the old one is saved to put back.
            unsafe {
                let mut sa: libc::sigaction = std::mem::zeroed();
                sa.sa_sigaction = on_int as extern "C" fn(i32) as libc::sighandler_t;
                libc::sigemptyset(&mut sa.sa_mask);
                let mut old: libc::sigaction = std::mem::zeroed();
                (libc::sigaction(libc::SIGINT, &sa, &mut old) == 0).then_some(Caught(old))
            }
        }
    }

    impl Drop for Caught {
        fn drop(&mut self) {
            // SAFETY: puts back the action saved in `start`.
            unsafe {
                libc::sigaction(libc::SIGINT, &self.0, std::ptr::null_mut());
            }
        }
    }
}

/// [`getpass_tty`] on the terminal `fd`.
#[cfg(unix)]
fn getpass_on(fd: i32, prompt_text: &str) -> Getpass {
    use std::sync::atomic::Ordering;
    write_fd(fd, prompt_text.as_bytes());
    let echo = NoEcho::start(fd);
    let caught = cancel::Caught::start();
    let mut line = Vec::new();
    let mut cancelled = false;
    loop {
        let mut b = 0u8;
        // SAFETY: one byte into a live local.
        let n = unsafe { libc::read(fd, (&mut b as *mut u8).cast(), 1) };
        if n == 1 {
            if b == b'\n' {
                break;
            }
            line.push(b);
            continue;
        }
        if n < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            if cancel::HIT.load(Ordering::SeqCst) {
                cancelled = true;
                break;
            }
            continue;
        }
        // The end of the input: what was typed stands, if anything.
        cancelled = line.is_empty();
        break;
    }
    drop(caught);
    drop(echo);
    if cancelled {
        drop(Secret::new(line));
        return Getpass::Cancelled;
    }
    write_fd(fd, b"\n");
    Getpass::Line(Secret::new(line))
}

/// All of `bytes` to `fd`; a failure is dropped, as a prompt's is.
#[cfg(unix)]
fn write_fd(fd: i32, mut bytes: &[u8]) {
    while !bytes.is_empty() {
        // SAFETY: writes from a live slice.
        let n = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
        if n > 0 {
            bytes = &bytes[n as usize..];
        } else if n < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            continue;
        } else {
            return;
        }
    }
}

/// `v="$(cat)"`: all of stdin, NUL bytes dropped and the trailing
/// newlines taken off. (Bash 4.4 and later also warn on stderr about a
/// NUL; that line names the script and is not copied.)
pub fn read_all_stdin() -> Secret {
    let mut all = Vec::new();
    let mut src = Fd0;
    loop {
        match src.byte() {
            Some(0) => {}
            Some(b) => all.push(b),
            None => break,
        }
    }
    let mut s = Secret::new(all);
    s.trim_trailing_newlines();
    s
}

fn finish(line: Vec<u8>, ended: bool, at_eof: AtEof) -> Secret {
    if ended || at_eof == AtEof::Keep {
        Secret::new(line)
    } else {
        drop(Secret::new(line));
        Secret::default()
    }
}

/// One byte at a time, as bash's `read` takes them.
trait ByteSource {
    /// The next byte; `None` at the end or on an error.
    fn byte(&mut self) -> Option<u8>;
}

/// Reads up to a newline (not kept), NULs skipped. True when a newline
/// ended it.
fn read_until_newline(src: &mut impl ByteSource) -> (Vec<u8>, bool) {
    let mut line = Vec::new();
    loop {
        match src.byte() {
            Some(b'\n') => return (line, true),
            Some(0) => {}
            Some(b) => line.push(b),
            None => return (line, false),
        }
    }
}

/// fd 0, unbuffered: nothing past the line is taken from stdin.
struct Fd0;

#[cfg(unix)]
impl ByteSource for Fd0 {
    fn byte(&mut self) -> Option<u8> {
        Raw(0).byte()
    }
}

#[cfg(not(unix))]
impl ByteSource for Fd0 {
    fn byte(&mut self) -> Option<u8> {
        use std::io::Read;
        let mut b = [0u8; 1];
        match io::stdin().lock().read(&mut b) {
            Ok(1) => Some(b[0]),
            _ => None,
        }
    }
}

#[cfg(unix)]
struct Raw(i32);

#[cfg(unix)]
impl ByteSource for Raw {
    fn byte(&mut self) -> Option<u8> {
        let mut b = 0u8;
        loop {
            // SAFETY: one byte into a live local.
            let n = unsafe { libc::read(self.0, (&mut b as *mut u8).cast(), 1) };
            if n == 1 {
                return Some(b);
            }
            if n < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return None;
        }
    }
}

impl<T: io::Read> ByteSource for io::Bytes<T> {
    fn byte(&mut self) -> Option<u8> {
        self.next().and_then(Result::ok)
    }
}

/// The terminal's echo off for as long as it lives, and back on when it
/// is dropped or a signal ends the command.
#[cfg(unix)]
pub(crate) struct NoEcho {
    fd: i32,
    saved: libc::termios,
    /// The handlers before ours, put back after.
    before: [libc::sighandler_t; 4],
}

#[cfg(not(unix))]
pub(crate) struct NoEcho;

#[cfg(unix)]
mod restore {
    //! What a signal during a hidden read puts back. Only async-signal-safe
    //! calls run in the handler.
    use std::sync::atomic::{AtomicBool, Ordering};

    pub static ARMED: AtomicBool = AtomicBool::new(false);
    pub static mut SAVED: std::mem::MaybeUninit<libc::termios> = std::mem::MaybeUninit::uninit();
    pub static mut FD: i32 = 0;
    pub const SIGNALS: [i32; 4] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT];

    pub extern "C" fn on_signal(sig: i32) {
        if ARMED.swap(false, Ordering::SeqCst) {
            // SAFETY: SAVED was written before ARMED was set; tcsetattr,
            // signal and raise are async-signal-safe.
            unsafe {
                libc::tcsetattr(FD, libc::TCSADRAIN, (*std::ptr::addr_of!(SAVED)).as_ptr());
            }
        }
        unsafe {
            libc::signal(sig, libc::SIG_DFL);
            libc::raise(sig);
        }
    }
}

#[cfg(unix)]
impl NoEcho {
    fn start(fd: i32) -> Option<NoEcho> {
        use std::sync::atomic::Ordering;
        let mut saved = std::mem::MaybeUninit::<libc::termios>::uninit();
        // SAFETY: tcgetattr fills the struct when it returns 0.
        if unsafe { libc::tcgetattr(fd, saved.as_mut_ptr()) } != 0 {
            return None;
        }
        let saved = unsafe { saved.assume_init() };
        let mut quiet = saved;
        quiet.c_lflag &= !(libc::ECHO | libc::ECHOK | libc::ECHONL);
        // SAFETY: the statics are written before ARMED says they are set,
        // and only the handler reads them.
        unsafe {
            *std::ptr::addr_of_mut!(restore::SAVED) = std::mem::MaybeUninit::new(saved);
            *std::ptr::addr_of_mut!(restore::FD) = fd;
        }
        restore::ARMED.store(true, Ordering::SeqCst);
        let mut before = [libc::SIG_DFL; 4];
        for (i, sig) in restore::SIGNALS.into_iter().enumerate() {
            let handler = restore::on_signal as extern "C" fn(i32) as libc::sighandler_t;
            // SAFETY: installs a handler that only calls safe functions.
            before[i] = unsafe { libc::signal(sig, handler) };
            if before[i] == libc::SIG_IGN {
                // Ignored before (nohup): keep it ignored.
                unsafe { libc::signal(sig, libc::SIG_IGN) };
            }
        }
        let guard = NoEcho { fd, saved, before };
        // TCSAFLUSH, as getpass: input typed before this is dropped.
        if unsafe { libc::tcsetattr(fd, libc::TCSAFLUSH, &quiet) } != 0 {
            return None;
        }
        Some(guard)
    }
}

#[cfg(unix)]
impl Drop for NoEcho {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering;
        restore::ARMED.store(false, Ordering::SeqCst);
        // SAFETY: puts back the settings read in `start`.
        unsafe {
            libc::tcsetattr(self.fd, libc::TCSADRAIN, &self.saved);
        }
        for (i, sig) in restore::SIGNALS.into_iter().enumerate() {
            // SAFETY: puts back the handler that was there before.
            unsafe { libc::signal(sig, self.before[i]) };
        }
    }
}

#[cfg(not(unix))]
impl NoEcho {
    fn start(_fd: i32) -> Option<NoEcho> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn line(input: &[u8]) -> (Vec<u8>, bool) {
        read_until_newline(&mut input.bytes())
    }

    #[test]
    fn a_line_is_up_to_its_newline() {
        assert_eq!(line(b"abc\ndef\n"), (b"abc".to_vec(), true));
        assert_eq!(line(b"\n"), (Vec::new(), true));
        // IFS= and -r: spaces and backslashes stay.
        assert_eq!(line(b"  a\\b \t\n"), (b"  a\\b \t".to_vec(), true));
    }

    #[test]
    fn nuls_are_skipped() {
        assert_eq!(line(b"a\0b\0\n"), (b"ab".to_vec(), true));
    }

    #[test]
    fn the_end_without_a_newline_fails_with_what_was_read() {
        assert_eq!(line(b"abc"), (b"abc".to_vec(), false));
        assert_eq!(line(b""), (Vec::new(), false));
        assert!(finish(b"abc".to_vec(), false, AtEof::Drop).is_empty());
        assert_eq!(finish(b"abc".to_vec(), false, AtEof::Keep).expose(), b"abc");
        assert_eq!(finish(b"abc".to_vec(), true, AtEof::Drop).expose(), b"abc");
    }

    #[test]
    fn the_end_of_the_input_is_not_an_empty_line() {
        let got = |l: Vec<u8>, e: bool| line_or_end(l, e).map(|s| s.expose().to_vec());
        assert_eq!(got(Vec::new(), true), Some(Vec::new()));
        assert_eq!(got(b"y".to_vec(), true), Some(b"y".to_vec()));
        // Ctrl-D at once, or after a word with no newline: read fails.
        assert_eq!(got(Vec::new(), false), None);
        assert_eq!(got(b"y".to_vec(), false), None);
        let mut src = b"yes\nno".bytes();
        let (l, e) = read_until_newline(&mut src);
        assert_eq!(got(l, e), Some(b"yes".to_vec()));
        let (l, e) = read_until_newline(&mut src);
        assert_eq!(got(l, e), None);
    }

    #[test]
    fn only_the_line_is_taken() {
        let mut src = b"one\ntwo\nthree".bytes();
        assert_eq!(read_until_newline(&mut src), (b"one".to_vec(), true));
        assert_eq!(read_until_newline(&mut src), (b"two".to_vec(), true));
        assert_eq!(read_until_newline(&mut src), (b"three".to_vec(), false));
    }

    #[test]
    fn a_secret_never_shows_itself() {
        let s = Secret::new(b"hunter2".to_vec());
        assert_eq!(format!("{s:?}"), "Secret(7 bytes)");
        let mut t = Secret::new(b"x\n\n".to_vec());
        t.trim_trailing_newlines();
        assert_eq!(t.expose(), b"x");
    }

    /// A new pseudo terminal's far end (as `notice`'s tests open one).
    #[cfg(unix)]
    fn pty() -> (std::os::fd::OwnedFd, std::fs::File) {
        use std::ffi::CStr;
        use std::fs::OpenOptions;
        use std::os::fd::FromRawFd;
        use std::os::unix::fs::OpenOptionsExt;
        // SAFETY: posix_openpt hands back a new fd we own; grantpt,
        // unlockpt and ptsname read only that fd, and the name is copied
        // out before anything else calls ptsname.
        unsafe {
            let fd = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
            assert!(fd >= 0, "posix_openpt: {}", io::Error::last_os_error());
            let master = std::os::fd::OwnedFd::from_raw_fd(fd);
            assert_eq!(libc::grantpt(fd), 0, "grantpt");
            assert_eq!(libc::unlockpt(fd), 0, "unlockpt");
            let name = libc::ptsname(fd);
            assert!(!name.is_null(), "ptsname");
            let path = CStr::from_ptr(name).to_str().unwrap().to_owned();
            let far = OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_NOCTTY)
                .open(&path)
                .unwrap_or_else(|e| panic!("open {path}: {e}"));
            (master, far)
        }
    }

    #[cfg(unix)]
    fn lflag(fd: i32) -> libc::tcflag_t {
        let mut t = std::mem::MaybeUninit::<libc::termios>::uninit();
        // SAFETY: tcgetattr fills it when it returns 0.
        assert_eq!(unsafe { libc::tcgetattr(fd, t.as_mut_ptr()) }, 0);
        unsafe { t.assume_init() }.c_lflag
    }

    /// `read -s` on a terminal: no echo while the guard lives, the
    /// terminal as it was after, and the signal handlers as they were.
    #[cfg(unix)]
    #[test]
    fn the_echo_is_off_for_the_read_and_back_after() {
        use std::os::fd::AsRawFd;
        let (_master, far) = pty();
        let fd = far.as_raw_fd();
        let echo = libc::ECHO | libc::ECHOK | libc::ECHONL;
        let before = lflag(fd);
        assert_ne!(before & libc::ECHO, 0, "a new pty echoes");
        let guard = NoEcho::start(fd).expect("a terminal");
        assert_eq!(lflag(fd) & echo, 0);
        drop(guard);
        assert_eq!(lflag(fd), before);
        assert!(!restore::ARMED.load(std::sync::atomic::Ordering::SeqCst));
    }

    /// getpass on a terminal: the line without its newline, no echo of
    /// it, the terminal as it was after; the end of the input with
    /// nothing typed is a cancel.
    #[cfg(unix)]
    #[test]
    fn getpass_reads_a_line_with_the_echo_off() {
        use std::io::{Read as _, Write as _};
        use std::os::fd::{AsRawFd, FromRawFd};
        let (master, far) = pty();
        let fd = far.as_raw_fd();
        let before = lflag(fd);
        // SAFETY: dup hands back new fds this test owns.
        let mut m = unsafe { std::fs::File::from_raw_fd(libc::dup(master.as_raw_fd())) };
        let mut screen = unsafe { std::fs::File::from_raw_fd(libc::dup(master.as_raw_fd())) };
        // The terminal's screen side reads what is written to it, as a
        // terminal window does. On macOS, TCSADRAIN waits until the
        // prompt has been read from the pty, so with no reader the
        // echo switch would wait for ever.
        std::thread::spawn(move || {
            let mut buf = [0u8; 256];
            while matches!(screen.read(&mut buf), Ok(n) if n > 0) {}
        });
        let reader = std::thread::spawn(move || getpass_on(fd, "pw: "));
        std::thread::sleep(std::time::Duration::from_millis(300));
        m.write_all(b"s3cret\n").unwrap();
        match reader.join().unwrap() {
            Getpass::Line(s) => assert_eq!(s.expose(), b"s3cret"),
            other => panic!("{other:?}"),
        }
        assert_eq!(lflag(fd), before);
        let reader = std::thread::spawn(move || getpass_on(fd, "pw: "));
        std::thread::sleep(std::time::Duration::from_millis(300));
        m.write_all(&[4]).unwrap();
        assert_eq!(reader.join().unwrap(), Getpass::Cancelled);
        assert_eq!(lflag(fd), before);
        drop(far);
    }

    /// Not a terminal: nothing to turn off.
    #[cfg(unix)]
    #[test]
    fn no_guard_off_a_terminal() {
        use std::os::fd::AsRawFd;
        let null = std::fs::File::open("/dev/null").unwrap();
        assert!(NoEcho::start(null.as_raw_fd()).is_none());
    }
}
