//! `app shell <app-id> [-- <command…>]`, the interactive terminal inside
//! an App's guarded container, as the shell CLI answers it
//! (`cmd_app_shell` and its WebSocket client `_K2_APP_SHELL_PY`,
//! `cli/src/app-doctor.sh`), step by step.
//!
//! - The words: the App, then the command (after `--`, or from the first
//!   word after the App on); a flag before the command is a usage
//!   mistake. The command goes as `argv=<its JSON list>` in the query.
//! - One TCP connection to `HOST:PORT` (15 s to connect and for each
//!   read or write of the handshake), and the WebSocket handshake: `GET
//!   /cli/app/shell?app=…&cols=…&rows=…[&argv=…]` with the credential in
//!   the `Authorization` header, never in the URL, and no other header
//!   but the WebSocket's own. `cols` and `rows` are the terminal's size
//!   when stdin is a terminal, else 80 and 24.
//! - An answer that is not `101`: its body (read for 5 s at most, until
//!   the server closes) gives the line on stderr, `{"error": <code>,
//!   "hint": <hint>}` (`shell_refused` and the status line when the body
//!   is no JSON object with one), and exit 3.
//! - Then, with stdin a terminal, the terminal goes raw (as Python 3.12's
//!   `tty.setraw`) and a window resize sends `{"type": "resize", "cols":
//!   C, "rows": R}`. What is typed goes as binary frames; a pipe or file
//!   on stdin is sent the same way, then `\x04` at its end. Binary frames
//!   from the server are written to stdout as they come; its text frames
//!   say a banner or notice (on stderr, between `\r\n`), an error (its
//!   JSON line on stderr, exit 3 unless an exit follows) or the exit
//!   code; a ping gets its pong; a close ends it. The terminal is put
//!   back as it was (`TCSADRAIN`) and a close frame sent, whatever ended
//!   the session.
//! - The exit: the exit frame's code when it is an int from 0 to 255 (a
//!   boolean is 0 or 1), 0 when none came, else 1. Where the Python
//!   raises (no connection, a reset, a text frame that is JSON but no
//!   object), its traceback and exit 1.
//!
//! The frames are read as the Python reads them: the opcode and the
//! length only (no FIN bit, no mask, no continuation frames), and each
//! frame the client sends is masked with a fresh key.
//!
//! On Windows the same session runs on the console: raw is the console's
//! modes (no line input, echo or Ctrl-C handling; keys as VT sequences,
//! and VT output with no newline translation), put back after; the
//! window's size is looked at every 200 ms and a change sent as the
//! resize; stdin and the socket are read on threads of their own. The
//! daemon on a Windows box answers the handshake itself (guarded Apps are
//! Linux-only there: `exec_unavailable`, exit 3).
//!
//! Where this differs from the shell: the Python's traceback names its
//! own library's frames (the socket module's lines), which this doesn't
//! (the parity rows compare a traceback by its exception); and the named
//! difference `shell-raw-py311`: the raw mode is Python 3.12's on every
//! Python, where 3.11's `tty.setraw` clears fewer flags (echo, line
//! editing and signals are off on both; `tests/app_shell_parity.rs`
//! compares the whole set where python3 is 3.12 or newer). The `https`
//! branch of the Python is not here: the shell's `BASE_URL` for `app` is
//! always `http://HOST:PORT`.

use super::call::Ctx;
use super::{Answer, Stop};
use crate::exit::Exit;
use crate::families::{Raise, Shown};
use crate::out::json::{Text, Value};
use crate::out::py as pyfmt;
use crate::out::pylib;
use crate::transport::{urllib, Auth};
use k2_client::cli_name;
use std::io::{Read, Write};
use std::net::TcpStream;
#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(unix)]
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;

/// The usage line (exit 2).
pub mod usage {
    use super::cli_name;
    pub const SHELL: &str = concat!(
        "usage: ",
        cli_name!(),
        " app shell <app-id> [-- <command\u{2026}>]"
    );
}

/// `app shell <app> [-- <command…>]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    pub app: String,
    /// The command, or empty: the App's own shell.
    pub argv: Vec<String>,
}

/// `cmd_app_shell`'s loop. `None`: a help form.
pub fn parse(words: &[&str]) -> Option<Result<Ask, Stop>> {
    let mut app = String::new();
    let mut i = 0;
    while let Some(w) = words.get(i).copied() {
        match w {
            "-h" | "--help" => return None,
            "--" => {
                i += 1;
                break;
            }
            _ if w.starts_with('-') => return Some(Err(Stop::usage(format!("Unknown flag: {w}")))),
            _ if app.is_empty() => {
                app = w.to_string();
                i += 1;
            }
            _ => break,
        }
    }
    if app.is_empty() {
        return Some(Err(Stop::usage(usage::SHELL)));
    }
    let argv = words[i..].iter().map(|w| w.to_string()).collect();
    Some(Ok(Ask { app, argv }))
}

/// Runs it: the session, then its exit (or the traceback where the
/// Python raises).
pub fn run(ctx: &mut Ctx, a: &Ask) -> Answer {
    let token = match &ctx.client.auth {
        Auth::None => String::new(),
        Auth::Owner(t) => t.clone(),
        Auth::Passport { passport, .. } => passport.as_str().to_string(),
    };
    // `urlparse("http://HOST:PORT")`: `hostname` is lower case, without
    // an IPv6 address's brackets; `port or 80`.
    let mut host = ctx.client.host.to_lowercase();
    if let Some(inner) = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        host = inner.to_string();
    }
    if host.is_empty() {
        host = "127.0.0.1".to_string();
    }
    let port = if ctx.client.port == 0 {
        80
    } else {
        ctx.client.port
    };
    Ok(session(&host, port, &token, a))
}

/// What the Python raises from `line` of its module (`frames` deeper in).
fn raised(frames: &[(u32, &'static str)], exception: String) -> Raise {
    Raise {
        frames: frames.to_vec(),
        exception,
    }
}

/// The `OSError` Python raises for `e` (its class by errno), at `frames`.
fn os_raise(frames: &[(u32, &'static str)], e: &std::io::Error) -> Raise {
    let r = urllib::os_error(e);
    raised(frames, format!("{}: {}", r.class.name(), r.text()))
}

/// `os.urandom(n)`.
fn urandom(n: usize) -> Vec<u8> {
    let mut b = vec![0u8; n];
    let _ = crate::sysrand::fill(&mut b);
    b
}

/// `os.get_terminal_size(0)`: `(columns, lines)`, or `None` (`OSError`).
#[cfg(unix)]
fn terminal_size() -> Option<(u16, u16)> {
    let mut ws = std::mem::MaybeUninit::<libc::winsize>::zeroed();
    // SAFETY: TIOCGWINSZ fills the struct when it returns 0.
    if unsafe { libc::ioctl(0, libc::TIOCGWINSZ, ws.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: zeroed, then filled by the ioctl.
    let ws = unsafe { ws.assume_init() };
    Some((ws.ws_col, ws.ws_row))
}

/// `os.get_terminal_size()` on Windows: the console window's columns and
/// lines (not the buffer's), or `None` when stdout is no console.
#[cfg(windows)]
fn terminal_size() -> Option<(u16, u16)> {
    use windows_sys::Win32::System::Console::{
        GetConsoleScreenBufferInfo, GetStdHandle, CONSOLE_SCREEN_BUFFER_INFO, STD_OUTPUT_HANDLE,
    };
    let mut info = std::mem::MaybeUninit::<CONSOLE_SCREEN_BUFFER_INFO>::zeroed();
    // SAFETY: the call fills the struct when it returns non-zero; the
    // handle is the process's own standard output.
    if unsafe { GetConsoleScreenBufferInfo(GetStdHandle(STD_OUTPUT_HANDLE), info.as_mut_ptr()) } == 0 {
        return None;
    }
    // SAFETY: zeroed, then filled by the call.
    let w = unsafe { info.assume_init() }.srWindow;
    let cols = u16::try_from(i32::from(w.Right) - i32::from(w.Left) + 1).ok()?;
    let rows = u16::try_from(i32::from(w.Bottom) - i32::from(w.Top) + 1).ok()?;
    Some((cols, rows))
}

#[cfg(not(any(unix, windows)))]
fn terminal_size() -> Option<(u16, u16)> {
    None
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// `json.dumps(v)`.
fn dumps(pairs: Vec<(&str, Value)>) -> String {
    crate::confirm::dumps(&Value::Object(
        pairs.into_iter().map(|(k, v)| (Text::from(k), v)).collect(),
    ))
}

/// `json.loads(data.decode())`, or `None` where that raises.
fn loads(data: &[u8]) -> Option<Value> {
    pylib::json::loads(std::str::from_utf8(data).ok()?).ok()
}

/// `d.get(key)` on a JSON object (`None` for a missing key).
fn get(d: &Value, key: &str) -> Value {
    match d {
        Value::Object(pairs) => pairs
            .iter()
            .find(|(k, _)| k.as_str() == Some(key))
            .map_or(Value::Null, |(_, v)| v.clone()),
        _ => Value::Null,
    }
}

fn has(d: &Value, key: &str) -> bool {
    matches!(d, Value::Object(pairs) if pairs.iter().any(|(k, _)| k.as_str() == Some(key)))
}

/// `bytes.decode()` of a status line, or the `UnicodeDecodeError`.
fn decode_status(status: &[u8]) -> Result<String, String> {
    std::str::from_utf8(status)
        .map(str::to_string)
        .map_err(|e| {
            let at = e.valid_up_to();
            format!(
                "UnicodeDecodeError: 'utf-8' codec can't decode byte 0x{:02x} in position {at}: invalid start byte",
                status[at]
            )
        })
}

/// The session: the connection, the handshake, then the frames.
fn session(host: &str, port: u16, token: &str, a: &Ask) -> Shown {
    let tty = crate::prompt::stdin_is_tty();
    let (mut cols, mut rows) = (80u16, 24u16);
    if tty {
        if let Some((c, r)) = terminal_size() {
            (cols, rows) = (c, r);
        }
    }
    let mut q = vec![
        ("app", a.app.clone()),
        ("cols", cols.to_string()),
        ("rows", rows.to_string()),
    ];
    if !a.argv.is_empty() {
        let argv = Value::Array(
            a.argv
                .iter()
                .map(|w| Value::Str(Text::from(w.as_str())))
                .collect(),
        );
        q.push(("argv", crate::confirm::dumps(&argv)));
    }
    let path = format!("/cli/app/shell?{}", urllib::urlencode(&q));
    let mut s = match urllib::connect(host, port, Duration::from_secs(15)) {
        Ok(s) => s,
        Err(r) => {
            let e = raised(
                &[(22, "<module>")],
                format!("{}: {}", r.class.name(), r.text()),
            );
            return Shown::err(e.text(), Exit::Error);
        }
    };
    let fifteen = Some(Duration::from_secs(15));
    let _ = s.set_read_timeout(fifteen);
    let _ = s.set_write_timeout(fifteen);
    let key = pylib::base64::b64encode(&urandom(16));
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\nAuthorization: Bearer {token}\r\n\r\n"
    );
    if let Err(e) = s.write_all(req.as_bytes()) {
        return Shown::err(os_raise(&[(28, "<module>")], &e).text(), Exit::Error);
    }
    let mut head = Vec::new();
    while find(&head, b"\r\n\r\n").is_none() {
        let mut c = [0u8; 4096];
        match s.read(&mut c) {
            Ok(0) => break,
            Ok(n) => head.extend_from_slice(&c[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Shown::err(os_raise(&[(31, "<module>")], &e).text(), Exit::Error),
        }
    }
    let (hdr, rest) = match find(&head, b"\r\n\r\n") {
        Some(at) => (&head[..at], &head[at + 4..]),
        None => (&head[..], &[][..]),
    };
    let status = find(hdr, b"\r\n").map_or(hdr, |at| &hdr[..at]);
    let mut spaced = status.to_vec();
    spaced.push(b' ');
    if find(&spaced, b" 101 ").is_none() {
        return refused(&mut s, status, rest);
    }
    let _ = s.set_read_timeout(None);
    let _ = s.set_write_timeout(None);
    let mut ws = Ws {
        s,
        buf: rest.to_vec(),
        code: Value::Null,
    };
    let raw = if tty { Raw::enter() } else { None };
    #[cfg(unix)]
    let r = {
        let winch = if tty { Winch::install() } else { None };
        let r = ws.drive(tty, winch.as_ref());
        drop(winch);
        r
    };
    #[cfg(not(unix))]
    let r = ws.drive(tty);
    // `finally:` the terminal as it was, then the close frame.
    if let Some(raw) = raw {
        raw.restore();
    }
    let _ = ws.send(8, b"");
    if let Err(e) = r {
        return Shown::err(e.text(), Exit::Error);
    }
    Shown {
        stdout: String::new(),
        stderr: String::new(),
        exit: exit_of(&ws.code),
    }
}

/// An answer that is not `101`: the line, exit 3.
fn refused(s: &mut TcpStream, status: &[u8], rest: &[u8]) -> Shown {
    let mut body = rest.to_vec();
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    loop {
        let mut c = vec![0u8; 65536];
        match s.read(&mut c) {
            Ok(0) => break,
            Ok(n) => body.extend_from_slice(&c[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    // `e = json.loads(body.decode() or "{}").get("error") or {}`, then the
    // line; anything that raises there is the `shell_refused` line.
    let line = (|| -> Option<String> {
        let text = std::str::from_utf8(&body).ok()?;
        let v = pylib::json::loads(if text.is_empty() { "{}" } else { text }).ok()?;
        if !matches!(v, Value::Object(_)) {
            return None;
        }
        let e = get(&v, "error");
        let e = if pyfmt::truthy(&e) {
            e
        } else {
            Value::Object(Vec::new())
        };
        if !matches!(e, Value::Object(_)) {
            return None;
        }
        let code = if has(&e, "code") {
            get(&e, "code")
        } else {
            Value::Str(Text::from("error"))
        };
        // The default is worked out first, as Python does.
        let status = decode_status(status).ok()?;
        let hint = if has(&e, "hint") {
            get(&e, "hint")
        } else {
            Value::Str(Text::from(status))
        };
        Some(dumps(vec![("error", code), ("hint", hint)]))
    })();
    let line = match line {
        Some(l) => l,
        None => match decode_status(status) {
            Ok(st) => dumps(vec![
                ("error", Value::Str(Text::from("shell_refused"))),
                ("hint", Value::Str(Text::from(st))),
            ]),
            Err(why) => {
                return Shown::err(raised(&[(52, "<module>")], why).text(), Exit::Error);
            }
        },
    };
    Shown::err(format!("{line}\n"), Exit::Refused)
}

/// `sys.exit(code if isinstance(code, int) and 0 <= code <= 255 else (0
/// if code is None else 1))`.
fn exit_of(code: &Value) -> Exit {
    match code {
        Value::Null | Value::Bool(false) => Exit::Ok,
        Value::Bool(true) => Exit::Error,
        Value::Int(d) => match d.parse::<i32>() {
            Ok(n @ 0..=255) => Exit::from_curl(n),
            _ => Exit::Error,
        },
        _ => Exit::Error,
    }
}

/// The open WebSocket: its buffer of unread bytes and the exit code the
/// server named.
struct Ws {
    s: TcpStream,
    buf: Vec<u8>,
    code: Value,
}

impl Ws {
    /// `send(op, data)`: one masked frame.
    fn send(&mut self, op: u8, data: &[u8]) -> std::io::Result<()> {
        let m = urandom(4);
        let n = data.len();
        let mut f = Vec::with_capacity(n + 14);
        f.push(0x80 | op);
        if n < 126 {
            f.push(0x80 | n as u8);
        } else if n < 65536 {
            f.push(0x80 | 126);
            f.extend_from_slice(&(n as u16).to_be_bytes());
        } else {
            f.push(0x80 | 127);
            f.extend_from_slice(&(n as u64).to_be_bytes());
        }
        f.extend_from_slice(&m);
        f.extend(data.iter().enumerate().map(|(i, b)| b ^ m[i % 4]));
        self.s.write_all(&f)
    }

    /// `send` called from line `at` of `func` (itself called from `from`).
    fn send_at(&mut self, from: &[(u32, &'static str)], op: u8, data: &[u8]) -> Result<(), Raise> {
        self.send(op, data).map_err(|e| {
            let mut frames = from.to_vec();
            frames.push((66, "send"));
            os_raise(&frames, &e)
        })
    }

    /// The next whole frame in the buffer, taken out of it.
    fn frame(&mut self) -> Option<(u8, Vec<u8>)> {
        let b = &self.buf;
        if b.len() < 2 {
            return None;
        }
        let op = b[0] & 0x0F;
        let mut n = u64::from(b[1] & 0x7F);
        let mut off = 2usize;
        if n == 126 {
            if b.len() < 4 {
                return None;
            }
            n = u64::from(u16::from_be_bytes([b[2], b[3]]));
            off = 4;
        } else if n == 127 {
            if b.len() < 10 {
                return None;
            }
            let mut eight = [0u8; 8];
            eight.copy_from_slice(&b[2..10]);
            n = u64::from_be_bytes(eight);
            off = 10;
        }
        let end = usize::try_from(n).ok()?.checked_add(off)?;
        if b.len() < end {
            return None;
        }
        let data = b[off..end].to_vec();
        self.buf.drain(..end);
        Some((op, data))
    }

    /// `handle()`, called from line `from`: each whole frame. `true` when
    /// the server closed.
    fn handle(&mut self, from: u32) -> Result<bool, Raise> {
        let at = |line: u32| [(from, "<module>"), (line, "handle")];
        while let Some((op, data)) = self.frame() {
            match op {
                2 => {
                    let mut out = std::io::stdout().lock();
                    if let Err(e) = out.write_all(&data).and_then(|()| out.flush()) {
                        return Err(os_raise(&at(107), &e));
                    }
                }
                1 => {
                    let Some(v) = loads(&data) else {
                        continue;
                    };
                    if !matches!(v, Value::Object(_)) {
                        return Err(raised(
                            &at(113),
                            format!(
                                "AttributeError: '{}' object has no attribute 'get'",
                                pyfmt::type_name(&v)
                            ),
                        ));
                    }
                    let t = get(&v, "type");
                    let is = |w: &str| t == Value::Str(Text::from(w));
                    if is("banner") || is("notice") {
                        let text = if has(&v, "text") {
                            get(&v, "text")
                        } else {
                            Value::Str(Text::from(""))
                        };
                        if !matches!(text, Value::Str(_)) {
                            return Err(raised(
                                &at(115),
                                format!(
                                    "TypeError: can only concatenate str (not \"{}\") to str",
                                    pyfmt::type_name(&text)
                                ),
                            ));
                        }
                        err_write(&format!("\r\n{}\r\n", pyfmt::str_of(&text)));
                    } else if is("error") {
                        let line =
                            dumps(vec![("error", get(&v, "code")), ("hint", get(&v, "hint"))]);
                        err_write(&format!("{line}\r\n"));
                        self.code = Value::Int("3".to_string());
                    } else if is("exit") {
                        self.code = get(&v, "code");
                    }
                }
                8 => return Ok(true),
                9 => self.send_at(&at(124), 10, &data)?,
                _ => {}
            }
        }
        Ok(false)
    }

    /// The `try:` block: the frames already read, then stdin and the
    /// socket until the server closes or ends the connection.
    #[cfg(unix)]
    fn drive(&mut self, tty: bool, winch: Option<&Winch>) -> Result<(), Raise> {
        let mut done = self.handle(128)?;
        let mut stdin_open = true;
        let sock = self.s.as_raw_fd();
        // macOS's poll answers POLLNVAL for a device such as /dev/null on
        // stdin, where the Python's select() says it can be read (and the
        // read then ends it): it counts as ready, so the end is seen and sent.
        let readable = libc::POLLIN | libc::POLLHUP | libc::POLLERR | libc::POLLNVAL;
        let ready = |re: libc::c_short| re & readable != 0;
        while !done {
            let mut fds = vec![libc::pollfd {
                fd: sock,
                events: libc::POLLIN,
                revents: 0,
            }];
            if stdin_open {
                fds.push(libc::pollfd {
                    fd: 0,
                    events: libc::POLLIN,
                    revents: 0,
                });
            }
            if let Some(w) = winch {
                fds.push(libc::pollfd {
                    fd: w.read_fd,
                    events: libc::POLLIN,
                    revents: 0,
                });
            }
            // SAFETY: `fds` is a live array of `fds.len()` pollfds.
            let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, -1) };
            if n < 0 {
                let e = std::io::Error::last_os_error();
                if e.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(os_raise(&[(132, "<module>")], &e));
            }
            if let Some(w) = winch {
                if fds.last().is_some_and(|f| ready(f.revents)) {
                    w.drain();
                    // `winch()`: the size now, sent; an OSError is passed.
                    if let Some((c, r)) = terminal_size() {
                        let msg = dumps(vec![
                            ("type", Value::Str(Text::from("resize"))),
                            ("cols", Value::Int(c.to_string())),
                            ("rows", Value::Int(r.to_string())),
                        ]);
                        if let Err(e) = self.send(1, msg.as_bytes()) {
                            return Err(os_raise(&[(95, "winch"), (66, "send")], &e));
                        }
                    }
                }
            }
            if stdin_open && ready(fds[1].revents) {
                let mut d = [0u8; 4096];
                // SAFETY: reads at most `d.len()` bytes into `d`.
                let got = unsafe { libc::read(0, d.as_mut_ptr().cast(), d.len()) };
                if got < 0 {
                    let e = std::io::Error::last_os_error();
                    if e.kind() != std::io::ErrorKind::Interrupted {
                        return Err(os_raise(&[(136, "<module>")], &e));
                    }
                } else if got > 0 {
                    self.send_at(&[(138, "<module>")], 2, &d[..got as usize])?;
                } else {
                    stdin_open = false;
                    if !tty {
                        self.send_at(&[(142, "<module>")], 2, b"\x04")?;
                    }
                }
            }
            if ready(fds[0].revents) {
                let mut c = vec![0u8; 65536];
                match self.s.read(&mut c) {
                    Ok(0) => break,
                    Ok(n) => {
                        self.buf.extend_from_slice(&c[..n]);
                        done = self.handle(148)?;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(e) => return Err(os_raise(&[(144, "<module>")], &e)),
                }
            }
        }
        Ok(())
    }
}

/// What the threads of the Windows session's loop ([`Ws::drive`]) hand it.
#[cfg(not(unix))]
enum Event {
    /// Bytes read from stdin.
    Stdin(Vec<u8>),
    /// The end of stdin.
    StdinEnd,
    StdinErr(std::io::Error),
    /// Bytes from the server.
    Sock(Vec<u8>),
    /// The server ended the connection.
    SockEnd,
    SockErr(std::io::Error),
    /// The console window's new size.
    Size(u16, u16),
}

#[cfg(not(unix))]
impl Ws {
    /// The `try:` block on Windows, where `select` takes no stdin: stdin,
    /// the socket and (on a console) the window's size are each watched
    /// on a thread, and their events handled here in the order they come,
    /// as the Unix loop handles what `poll` says is ready. The stdin
    /// thread may stay blocked in its read after the session ends; the
    /// process exits next.
    fn drive(&mut self, tty: bool) -> Result<(), Raise> {
        use std::sync::mpsc;
        if self.handle(128)? {
            return Ok(());
        }
        let (tx, rx) = mpsc::channel::<Event>();
        let mut from_server = self
            .s
            .try_clone()
            .map_err(|e| os_raise(&[(144, "<module>")], &e))?;
        let t = tx.clone();
        std::thread::spawn(move || {
            let mut c = vec![0u8; 65536];
            loop {
                let ev = match from_server.read(&mut c) {
                    Ok(0) => Event::SockEnd,
                    Ok(n) => Event::Sock(c[..n].to_vec()),
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) => Event::SockErr(e),
                };
                let last = matches!(ev, Event::SockEnd | Event::SockErr(_));
                if t.send(ev).is_err() || last {
                    return;
                }
            }
        });
        let t = tx.clone();
        std::thread::spawn(move || {
            let mut d = [0u8; 4096];
            loop {
                let ev = match std::io::stdin().read(&mut d) {
                    Ok(0) => Event::StdinEnd,
                    Ok(n) => Event::Stdin(d[..n].to_vec()),
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) => Event::StdinErr(e),
                };
                let last = matches!(ev, Event::StdinEnd | Event::StdinErr(_));
                if t.send(ev).is_err() || last {
                    return;
                }
            }
        });
        if tty {
            let t = tx.clone();
            std::thread::spawn(move || {
                let mut was = terminal_size();
                loop {
                    std::thread::sleep(Duration::from_millis(200));
                    let now = terminal_size();
                    if now != was {
                        was = now;
                        if let Some((c, r)) = now {
                            if t.send(Event::Size(c, r)).is_err() {
                                return;
                            }
                        }
                    }
                }
            });
        }
        drop(tx);
        for ev in rx {
            match ev {
                Event::Size(c, r) => {
                    // `winch()`: the size now, sent.
                    let msg = dumps(vec![
                        ("type", Value::Str(Text::from("resize"))),
                        ("cols", Value::Int(c.to_string())),
                        ("rows", Value::Int(r.to_string())),
                    ]);
                    if let Err(e) = self.send(1, msg.as_bytes()) {
                        return Err(os_raise(&[(95, "winch"), (66, "send")], &e));
                    }
                }
                Event::Stdin(d) => self.send_at(&[(138, "<module>")], 2, &d)?,
                Event::StdinEnd => {
                    if !tty {
                        self.send_at(&[(142, "<module>")], 2, b"\x04")?;
                    }
                }
                Event::StdinErr(e) => return Err(os_raise(&[(136, "<module>")], &e)),
                Event::Sock(c) => {
                    self.buf.extend_from_slice(&c);
                    if self.handle(148)? {
                        return Ok(());
                    }
                }
                Event::SockEnd => return Ok(()),
                Event::SockErr(e) => return Err(os_raise(&[(144, "<module>")], &e)),
            }
        }
        Ok(())
    }
}

/// The console in raw mode on Windows: its input and output modes as they
/// were, to put back.
#[cfg(windows)]
struct Raw {
    input: (windows_sys::Win32::Foundation::HANDLE, u32),
    output: Option<(windows_sys::Win32::Foundation::HANDLE, u32)>,
}

#[cfg(windows)]
impl Raw {
    /// `tty.setraw(0)` on the console: input with no line editing, echo
    /// or Ctrl-C handling (Ctrl-C goes to the App as `\x03`) and keys as
    /// VT sequences; output that reads VT sequences and leaves newlines as
    /// they are (the App's terminal sends `\r\n` itself). `None` when
    /// stdin is no console.
    fn enter() -> Option<Raw> {
        use windows_sys::Win32::System::Console::{
            GetConsoleMode, GetStdHandle, SetConsoleMode, DISABLE_NEWLINE_AUTO_RETURN,
            ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT, ENABLE_PROCESSED_INPUT,
            ENABLE_VIRTUAL_TERMINAL_INPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING, STD_INPUT_HANDLE,
            STD_OUTPUT_HANDLE,
        };
        // SAFETY: the process's own standard handles; GetConsoleMode only
        // fills the u32, and SetConsoleMode takes plain flags.
        unsafe {
            let hin = GetStdHandle(STD_INPUT_HANDLE);
            let mut old_in = 0u32;
            if GetConsoleMode(hin, &mut old_in) == 0 {
                return None;
            }
            let new_in = (old_in & !(ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_PROCESSED_INPUT))
                | ENABLE_VIRTUAL_TERMINAL_INPUT;
            if SetConsoleMode(hin, new_in) == 0 {
                return None;
            }
            let hout = GetStdHandle(STD_OUTPUT_HANDLE);
            let mut old_out = 0u32;
            let output = (GetConsoleMode(hout, &mut old_out) != 0).then(|| {
                SetConsoleMode(
                    hout,
                    old_out | ENABLE_VIRTUAL_TERMINAL_PROCESSING | DISABLE_NEWLINE_AUTO_RETURN,
                );
                (hout, old_out)
            });
            Some(Raw {
                input: (hin, old_in),
                output,
            })
        }
    }

    /// The console's modes as they were.
    fn restore(self) {
        use windows_sys::Win32::System::Console::SetConsoleMode;
        // SAFETY: the handles and modes `enter` read.
        unsafe {
            SetConsoleMode(self.input.0, self.input.1);
            if let Some((h, m)) = self.output {
                SetConsoleMode(h, m);
            }
        }
    }
}

#[cfg(not(any(unix, windows)))]
struct Raw;

#[cfg(not(any(unix, windows)))]
impl Raw {
    fn enter() -> Option<Raw> {
        None
    }
    fn restore(self) {}
}

/// `err.write(text); err.flush()`.
fn err_write(text: &str) {
    let _ = crate::out::write_flushed(&mut std::io::stderr().lock(), text.as_bytes());
}

/// The terminal in raw mode: what it was, to put back.
#[cfg(unix)]
struct Raw {
    old: libc::termios,
}

#[cfg(unix)]
impl Raw {
    /// `old = termios.tcgetattr(0); tty.setraw(0)`, as Python 3.12's
    /// `tty.cfmakeraw` sets it (`TCSAFLUSH`).
    fn enter() -> Option<Raw> {
        let mut t = std::mem::MaybeUninit::<libc::termios>::zeroed();
        // SAFETY: tcgetattr fills the struct when it returns 0.
        if unsafe { libc::tcgetattr(0, t.as_mut_ptr()) } != 0 {
            return None;
        }
        // SAFETY: filled by tcgetattr.
        let old = unsafe { t.assume_init() };
        let mut new = old;
        new.c_iflag &= !(libc::IGNBRK
            | libc::BRKINT
            | libc::IGNPAR
            | libc::PARMRK
            | libc::INPCK
            | libc::ISTRIP
            | libc::INLCR
            | libc::IGNCR
            | libc::ICRNL
            | libc::IXON
            | libc::IXANY
            | libc::IXOFF);
        new.c_oflag &= !libc::OPOST;
        new.c_cflag &= !(libc::PARENB | libc::CSIZE);
        new.c_cflag |= libc::CS8;
        new.c_lflag &= !(libc::ECHO
            | libc::ECHOE
            | libc::ECHOK
            | libc::ECHONL
            | libc::ICANON
            | libc::IEXTEN
            | libc::ISIG
            | libc::NOFLSH
            | libc::TOSTOP);
        new.c_cc[libc::VMIN] = 1;
        new.c_cc[libc::VTIME] = 0;
        // SAFETY: a plain tcsetattr with a struct tcgetattr filled.
        unsafe { libc::tcsetattr(0, libc::TCSAFLUSH, &new) };
        Some(Raw { old })
    }

    /// `termios.tcsetattr(0, termios.TCSADRAIN, old)`.
    fn restore(self) {
        // SAFETY: the struct tcgetattr filled.
        unsafe { libc::tcsetattr(0, libc::TCSADRAIN, &self.old) };
    }
}

/// The write end of the pipe SIGWINCH's handler writes a byte to.
#[cfg(unix)]
static WINCH_FD: AtomicI32 = AtomicI32::new(-1);

#[cfg(unix)]
extern "C" fn on_winch(_: libc::c_int) {
    let fd = WINCH_FD.load(Ordering::Relaxed);
    if fd >= 0 {
        let b = 1u8;
        // SAFETY: write(2) is async-signal-safe; one byte from a local.
        unsafe { libc::write(fd, (&b as *const u8).cast(), 1) };
    }
}

/// `signal.signal(signal.SIGWINCH, winch)`: the handler marks a pipe the
/// session's poll watches, so a resize is sent from the loop.
#[cfg(unix)]
struct Winch {
    read_fd: libc::c_int,
    write_fd: libc::c_int,
    before: libc::sigaction,
}

#[cfg(unix)]
impl Winch {
    fn install() -> Option<Winch> {
        let mut fds = [0 as libc::c_int; 2];
        // SAFETY: pipe fills two descriptors when it returns 0.
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return None;
        }
        for fd in fds {
            // SAFETY: flag changes on descriptors this owns.
            unsafe {
                let fl = libc::fcntl(fd, libc::F_GETFL);
                libc::fcntl(fd, libc::F_SETFL, fl | libc::O_NONBLOCK);
                libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            }
        }
        WINCH_FD.store(fds[1], Ordering::Relaxed);
        // SAFETY: a zeroed sigaction with a plain handler and SA_RESTART.
        let mut sa: libc::sigaction = unsafe { std::mem::zeroed() };
        sa.sa_sigaction = on_winch as extern "C" fn(libc::c_int) as usize;
        sa.sa_flags = libc::SA_RESTART;
        // SAFETY: zeroed, then filled by sigaction.
        let mut before: libc::sigaction = unsafe { std::mem::zeroed() };
        // SAFETY: installs the handler above; the old one is kept.
        unsafe {
            libc::sigemptyset(&mut sa.sa_mask);
            libc::sigaction(libc::SIGWINCH, &sa, &mut before);
        }
        Some(Winch {
            read_fd: fds[0],
            write_fd: fds[1],
            before,
        })
    }

    /// Reads every byte the handler wrote.
    fn drain(&self) {
        let mut b = [0u8; 64];
        // SAFETY: reads into a local buffer; the descriptor is non-blocking.
        while unsafe { libc::read(self.read_fd, b.as_mut_ptr().cast(), b.len()) } > 0 {}
    }
}

#[cfg(unix)]
impl Drop for Winch {
    fn drop(&mut self) {
        // SAFETY: puts back the handler there was, then closes this pipe.
        unsafe {
            libc::sigaction(libc::SIGWINCH, &self.before, std::ptr::null_mut());
        }
        WINCH_FD.store(-1, Ordering::Relaxed);
        // SAFETY: descriptors this owns, closed once.
        unsafe {
            libc::close(self.read_fd);
            libc::close(self.write_fd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_as_the_shell_reads_them() {
        let ask = |w: &[&str]| match parse(w) {
            Some(Ok(a)) => a,
            other => panic!("{other:?}"),
        };
        assert_eq!(ask(&["acme"]).argv, Vec::<String>::new());
        assert_eq!(ask(&["acme", "ls", "-l"]).argv, vec!["ls", "-l"]);
        assert_eq!(ask(&["acme", "--", "-x"]).argv, vec!["-x"]);
        // `--` first: no App, the usage line (the shell's loop stops there).
        assert!(matches!(parse(&["--", "acme"]), Some(Err(s)) if s.line == usage::SHELL));
        assert!(matches!(parse(&[]), Some(Err(s)) if s.line == usage::SHELL));
        assert!(matches!(parse(&["acme", "-x"]), Some(Err(s)) if s.line == "Unknown flag: -x"));
        assert_eq!(parse(&["-h"]), None);
    }

    #[test]
    fn exits_as_sys_exit_takes_them() {
        let v = |t: &str| crate::out::json::parse(t).unwrap();
        assert_eq!(exit_of(&Value::Null), Exit::Ok);
        assert_eq!(exit_of(&v("0")), Exit::Ok);
        assert_eq!(exit_of(&v("7")).code(), 7);
        assert_eq!(exit_of(&v("256")), Exit::Error);
        assert_eq!(exit_of(&v("-1")), Exit::Error);
        assert_eq!(exit_of(&v("true")), Exit::Error);
        assert_eq!(exit_of(&v("false")), Exit::Ok);
        assert_eq!(exit_of(&v("2.0")), Exit::Error);
        assert_eq!(exit_of(&v("\"3\"")), Exit::Error);
    }

    #[test]
    fn frames_as_the_python_reads_them() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let s = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let mut ws = Ws {
            s,
            buf: vec![0x82, 2, b'h', b'i', 0x81, 126, 0, 3, b'a', b'b', b'c', 0x88],
            code: Value::Null,
        };
        assert_eq!(ws.frame(), Some((2, b"hi".to_vec())));
        assert_eq!(ws.frame(), Some((1, b"abc".to_vec())));
        assert_eq!(ws.frame(), None);
        ws.buf.push(0);
        assert_eq!(ws.frame(), Some((8, Vec::new())));
    }
}
