//! The clone engine's requests, as its Python sends them: `call()` (urllib
//! to this computer's daemon, with the hand-over headers, or to the remote
//! server, without them) and `vault_sock()` (http.client over the vault
//! socket). Every read and write watches for a stop ([`super::tty::take`]):
//! the socket's own timeout is cut into short waits, so Ctrl-C or SIGTERM
//! stops a request at once, as Python's signal handlers do.
//!
//! No request follows a redirect: the answer's place is refused with the
//! shared rule's line ([`crate::transport::urllib::redirect_refusal`]),
//! as the shell's `_K2NoRedirect`.

use super::pyurl::Target;
use crate::transport::proxy::Proxy;
use crate::transport::urllib::{self, Answered, Outcome, Raised, Request};
use std::io::{self, BufReader, Read, Write};
use std::net::TcpStream;
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

/// The TCP connection urllib opens for `t`: straight there, or through
/// `via`'s CONNECT tunnel as `http.client` opens it, on the shared path
/// the urllib families' daemon opener uses ([`urllib::tunnel`]: Python
/// 3.12's CONNECT head, 200 alone opening it, each failure raised as
/// `http.client` and urllib raise it).
fn open_tcp(t: &Target, via: Option<&Proxy>, timeout: Duration) -> Result<TcpStream, Raised> {
    match via {
        Some(p) => urllib::tunnel(p, &t.host, t.port, timeout),
        None => urllib::connect(&t.host, t.port, timeout).map_err(url_error),
    }
}

/// A failed handshake as a `URLError`: its reason the failure's plain
/// words ([`crate::transport::wire::Failure`]'s `Display`), which the
/// clone's hint shows.
fn tls_failed(f: &crate::transport::wire::Failure) -> Raised {
    Raised {
        class: urllib::Class::UrlError,
        kind: urllib::Kind::Other,
        reason: format!("[SSL] {f}"),
    }
}

/// One urllib request to `t` (TLS for `https`), through `via` when the
/// person's proxy carries it: `bytes` written, the answer read.
pub fn over_tcp(t: &Target, via: Option<&Proxy>, bytes: &[u8], timeout: Duration) -> Sent {
    let tcp = match open_tcp(t, via, timeout) {
        Ok(s) => s,
        Err(r) => return raised(r),
    };
    let conn = if t.tls {
        let limit = Instant::now() + timeout;
        match crate::transport::tls::connect(tcp, &t.host, limit) {
            Ok(s) => {
                if let Err(e) = crate::transport::tls::stream_timeouts(&s, SLICE) {
                    return raised(url_error(urllib::os_error(&e)));
                }
                crate::transport::wire::Conn::Tls(Box::new(s))
            }
            Err(f) => return raised(tls_failed(&f)),
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
        wipe_bytes(d);
    }
}

/// Wipes bytes that held a request (its body may hold a password).
pub fn wipe_bytes(d: &mut [u8]) {
    for b in d.iter_mut() {
        // SAFETY: a volatile write into a live buffer.
        unsafe { std::ptr::write_volatile(b, 0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::wire::Failure;

    #[test]
    fn a_failed_handshake_s_reason_is_plain_words() {
        let f = Failure::Broken {
            curl: 60,
            why: "the server's certificate has expired".into(),
        };
        let r = tls_failed(&f);
        assert_eq!(r.reason, "[SSL] the server's certificate has expired");
        assert_eq!(r.class, urllib::Class::UrlError);
        assert_eq!(tls_failed(&Failure::ConnectTimedOut).reason, "[SSL] the connection timed out");
    }
}
