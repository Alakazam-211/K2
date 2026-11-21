//! The clone engine's requests, as its Python sends them: `call()` (urllib
//! to this computer's daemon, with the hand-over headers, or to the remote
//! server, without them) and `vault_sock()` (http.client over the vault
//! socket). Every read and write watches for a stop ([`super::tty::take`]):
//! the socket's own timeout is cut into short waits, so Ctrl-C or SIGTERM
//! stops a request at once, as Python's signal handlers do.
//!
//! No request follows a redirect: the answer's place is refused
//! ([`redirect_line`]), as the shell's `_K2NoRedirect`.

use crate::transport::urllib::{self, Answered, Outcome, Raised, Request};
use std::io::{self, BufReader, Read, Write};
use std::time::{Duration, Instant};

/// How long each wait inside a read or write is, between looks for a stop.
const SLICE: Duration = Duration::from_millis(200);

/// The stop's marker in an I/O error.
const STOPPED: &str = "the clone engine was stopped";

/// A stream whose reads and writes wait at most `limit` each (Python's
/// socket timeout), and give up at once on a stop.
pub struct Watched<S> {
    pub inner: S,
    pub limit: Duration,
}

fn waiting(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
    )
}

impl<S> Watched<S> {
    fn wait<T>(&mut self, mut step: impl FnMut(&mut S) -> io::Result<T>) -> io::Result<T> {
        let start = Instant::now();
        loop {
            match step(&mut self.inner) {
                Err(e) if waiting(&e) => {
                    if super::tty::pending() {
                        return Err(io::Error::other(STOPPED));
                    }
                    if start.elapsed() >= self.limit {
                        return Err(io::Error::new(io::ErrorKind::TimedOut, "timed out"));
                    }
                }
                r => return r,
            }
        }
    }
}

impl<S: Read> Read for Watched<S> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.wait(|s| s.read(buf))
    }
}

impl<S: Write> Write for Watched<S> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.wait(|s| s.write(buf))
    }

    fn flush(&mut self) -> io::Result<()> {
        self.wait(|s| s.flush())
    }
}

/// What one request came to: urllib's outcome and the answer's
/// `Location`, or a stop that came during it.
pub enum Sent {
    Done(Answered),
    Stopped,
}

fn stopped(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::Other && e.to_string() == STOPPED
}

/// Any stop error inside `a` (a read or write that gave up on a stop).
fn answered_or_stopped(a: Answered) -> Sent {
    if super::tty::pending() {
        return Sent::Stopped;
    }
    Sent::Done(a)
}

fn raised(r: Raised) -> Sent {
    if super::tty::pending() {
        return Sent::Stopped;
    }
    Sent::Done(Answered {
        outcome: Outcome::Raised(r),
        location: None,
        server_version: None,
    })
}

/// A `URLError` around the reason (the connect, the handshake, the send).
fn url_error(r: Raised) -> Raised {
    Raised {
        class: urllib::Class::UrlError,
        kind: r.kind,
        reason: r.reason,
    }
}

/// One urllib request over TCP (TLS for `tls`) to `host:port`: `bytes`
/// written, the answer read.
pub fn over_tcp(host: &str, port: u16, tls: bool, bytes: &[u8], timeout: Duration) -> Sent {
    let tcp = match urllib::connect(host, port, timeout) {
        Ok(s) => s,
        Err(r) => return raised(url_error(r)),
    };
    let conn = if tls {
        let limit = Instant::now() + timeout;
        match crate::transport::tls::connect(tcp, host, limit) {
            Ok(s) => {
                if let Err(e) = crate::transport::tls::stream_timeouts(&s, SLICE) {
                    return raised(url_error(urllib::os_error(&e)));
                }
                crate::transport::wire::Conn::Tls(Box::new(s))
            }
            Err(f) => {
                return raised(Raised {
                    class: urllib::Class::UrlError,
                    kind: urllib::Kind::Other,
                    reason: format!("[SSL] {f:?}"),
                })
            }
        }
    } else {
        if let Err(e) = tcp
            .set_read_timeout(Some(SLICE))
            .and_then(|()| tcp.set_write_timeout(Some(SLICE)))
        {
            return raised(url_error(urllib::os_error(&e)));
        }
        crate::transport::wire::Conn::Tcp(tcp)
    };
    let mut w = Watched {
        inner: conn,
        limit: timeout,
    };
    if let Err(e) = w.write_all(bytes).and_then(|()| w.flush()) {
        if stopped(&e) {
            return Sent::Stopped;
        }
        return raised(url_error(urllib::os_error(&e)));
    }
    let mut r = BufReader::with_capacity(8192, w);
    answered_or_stopped(urllib::read_answer(&mut r))
}

/// One http.client request over the Unix socket at `path` (`_UConn`):
/// the connect's error is the `OSError` (`Err`).
#[cfg(unix)]
pub fn over_unix(path: &std::path::Path, bytes: &[u8], timeout: Duration) -> Result<Sent, Raised> {
    use std::os::unix::net::UnixStream;
    let s = UnixStream::connect(path).map_err(|e| urllib::os_error(&e))?;
    s.set_read_timeout(Some(SLICE))
        .and_then(|()| s.set_write_timeout(Some(SLICE)))
        .map_err(|e| urllib::os_error(&e))?;
    let mut w = Watched {
        inner: s,
        limit: timeout,
    };
    if let Err(e) = w.write_all(bytes).and_then(|()| w.flush()) {
        if stopped(&e) {
            return Ok(Sent::Stopped);
        }
        return Err(urllib::os_error(&e));
    }
    let mut r = BufReader::with_capacity(8192, w);
    Ok(answered_or_stopped(urllib::read_answer(&mut r)))
}

/// Wipes a request's body (a password may be in it) once it was sent.
pub fn wipe(req: &mut Request) {
    if let Some(d) = req.data.as_mut() {
        for b in d.iter_mut() {
            // SAFETY: a volatile write into a live buffer.
            unsafe { std::ptr::write_volatile(b, 0) };
        }
    }
}

/// The refusal `_K2NoRedirect` writes for a redirect from `url` to
/// `location` (`code`): both places by their netloc less any user part
/// (the new one joined to the old URL first), what isn't printable as `?`.
pub fn redirect_line(url: &str, code: u16, location: &str) -> String {
    let after_at = |s: &str| s.rsplit_once('@').map_or(s, |(_, h)| h).to_string();
    let here = super::rule::urlsplit(url)
        .map(|u| after_at(&u.netloc))
        .unwrap_or_default();
    let joined = join(url, location);
    let there = super::rule::urlsplit(&joined)
        .map(|u| after_at(&u.netloc))
        .filter(|s| !s.is_empty())
        .unwrap_or(joined);
    let there: String = there
        .chars()
        .map(|c| {
            if crate::out::py::isprintable(c) {
                c
            } else {
                '?'
            }
        })
        .collect();
    let p = k2_client::product_name!();
    format!(
        "refused: {here} answered {code}, sending {p} to {there}; {p} does not follow a redirect with your token or password, so nothing was sent there"
    )
}

/// `urljoin(base, url)` for the netloc only: an absolute URL or a
/// `//netloc` one names its own; anything else stays on the base's.
fn join(base: &str, url: &str) -> String {
    if url.starts_with("//") {
        let scheme = super::rule::urlsplit(base)
            .map(|u| u.scheme)
            .unwrap_or_default();
        return format!("{scheme}:{url}");
    }
    match super::rule::urlsplit(url) {
        Some(u) if !u.scheme.is_empty() && !u.netloc.is_empty() => url.to_string(),
        _ => base.to_string(),
    }
}

/// Is `code` one urllib's redirect handler takes, and is its new place
/// one it would go to? (`http`, `https`, `ftp` or none.)
pub fn redirects(code: u16, location: Option<&str>) -> Option<String> {
    if !matches!(code, 301 | 302 | 303 | 307 | 308) {
        return None;
    }
    let loc = location?;
    let scheme = super::rule::urlsplit(loc)
        .map(|u| u.scheme)
        .unwrap_or_default();
    matches!(scheme.as_str(), "http" | "https" | "ftp" | "").then(|| loc.to_string())
}
