//! One HTTP/1.1 exchange on one connection, written byte for byte as curl
//! writes it for the shell CLI's calls, and the answer read back.
//!
//! What curl sends (curl 8.5 on Linux, 8.7 on macOS and 8.23; the parity
//! test `tests/transport_parity.rs` holds the two the same on every run):
//!
//! ```text
//! <METHOD> <target> HTTP/1.1
//! Host: <host>
//! User-Agent: curl/<version>
//! Accept: */*
//! <each custom header, in the order curl was given them>
//! Content-Length: <n>                                   (a body only)
//! Content-Type: application/x-www-form-urlencoded       (curl's own, a body with no custom type)
//!
//! <body>
//! ```
//!
//! Each line ends in CRLF. A custom `Content-Type` is one of the custom
//! headers, so it comes before `Content-Length`; curl's own form type
//! comes after it. curl sends no `Connection` header and no
//! `Expect: 100-continue` below 1 MiB of body (8.x); this module never
//! sends either.
//!
//! The target: curl sends a URL's path and query as they are written,
//! but for the data `-G` appends to the URL (after `?`, or `&` when the
//! URL has a query): curl 8.5 and 8.7 write each `%XX` escape's hex there
//! in lower case, and curl 8.23 no longer does. Here those escapes go out
//! in upper case, as RFC 3986 asks (`%2f` goes out as `%2F`;
//! [`curl_get_query`]); the daemon decodes either. Only the appended part
//! changes; a query already in the URL keeps its case. A space or a
//! control byte in the URL makes curl fail before it connects (exit 3),
//! and a `#` cuts the URL there (the fragment); [`check_target`] refuses
//! all three instead.

use std::io::{self, Read, Write};
#[cfg(unix)]
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// curl's `Accept` header.
pub const ACCEPT: &str = "*/*";

/// The form type curl sets itself for a `-d` body with no custom type.
pub const FORM_TYPE: &str = "application/x-www-form-urlencoded";

/// The method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    /// The Connect control plane's repoint (`_connect_curl PUT`).
    Put,
}

impl Method {
    /// As it is on the request line.
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
        }
    }
}

/// A request body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Body {
    pub bytes: Vec<u8>,
    /// True for a body sent with curl's own form type (`--data-raw` with
    /// no `-H "Content-Type: …"`): it goes after `Content-Length`. False
    /// when the type is one of the custom headers.
    pub curl_form_type: bool,
}

/// One request, as curl would write it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: Method,
    /// The path and query.
    pub target: String,
    /// The `Host` header: `<host>:<port>` over TCP, `localhost` on the
    /// cell socket (the shell's URL there is `http://localhost<path>`).
    pub host: String,
    pub user_agent: String,
    /// The custom headers, in order, after `Accept`.
    pub headers: Vec<(String, String)>,
    pub body: Option<Body>,
}

impl Request {
    /// The bytes on the wire.
    pub fn encode(&self) -> Vec<u8> {
        let mut head = format!(
            "{} {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: {}\r\nAccept: {ACCEPT}\r\n",
            self.method.as_str(),
            self.target,
            self.host,
            self.user_agent
        );
        for (name, value) in &self.headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        if let Some(body) = &self.body {
            head.push_str(&format!("Content-Length: {}\r\n", body.bytes.len()));
            if body.curl_form_type {
                head.push_str(&format!("Content-Type: {FORM_TYPE}\r\n"));
            }
        }
        head.push_str("\r\n");
        let mut out = head.into_bytes();
        if let Some(body) = &self.body {
            out.extend_from_slice(&body.bytes);
        }
        out
    }
}

/// The data `-G` appends to a URL, with the hex of each `%XX` escape in
/// upper case (RFC 3986; curl's own case changed between versions). A `%`
/// not followed by two hex digits stays as it is.
pub fn curl_get_query(data: &str) -> String {
    let bytes = data.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && bytes[i + 1].is_ascii_hexdigit()
            && bytes[i + 2].is_ascii_hexdigit()
        {
            out.push(b'%');
            out.push(bytes[i + 1].to_ascii_uppercase());
            out.push(bytes[i + 2].to_ascii_uppercase());
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    // Only ASCII bytes changed case, so the text is still UTF-8.
    String::from_utf8(out).expect("upper-casing ASCII hex keeps UTF-8")
}

/// `url` with `-G` data appended, as curl joins them: `?` when the URL
/// has no query yet, else `&`.
pub fn append_get_query(url: &str, data: &str) -> String {
    let sep = if url.contains('?') { '&' } else { '?' };
    format!("{url}{sep}{}", curl_get_query(data))
}

/// curl's `--data-urlencode name=content`: `name=` and the content with
/// every byte but ASCII letters, digits and `- . _ ~` escaped, and a
/// space as `+`.
pub fn curl_data_urlencode(name: &str, content: &str) -> String {
    let mut out = format!("{name}=");
    for &b in content.as_bytes() {
        if b == b' ' {
            out.push('+');
        } else if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A target curl would not send as given: a space or a control byte
/// (curl exits 3) or a `#` (curl drops it and what follows).
pub fn check_target(target: &str) -> Result<(), u8> {
    match target
        .bytes()
        .find(|b| *b <= b' ' || *b == 0x7f || *b == b'#')
    {
        Some(b) => Err(b),
        None => Ok(()),
    }
}

/// Where the request goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Tcp {
        host: String,
        port: u16,
    },
    /// `https://<host>[:<port>]`: TCP, then TLS for `host`
    /// ([`super::tls`]); through the person's proxy when `via` holds one
    /// ([`super::proxy`]). Only a host that is not this computer ever has
    /// one, and only this variant can hold one: the daemon's targets,
    /// [`Target::Tcp`] and [`Target::Unix`], never go through a proxy.
    Tls {
        host: String,
        port: u16,
        via: Option<Box<Tunnel>>,
    },
    #[cfg(unix)]
    Unix(PathBuf),
}

/// The proxy a [`Target::Tls`] goes through, and the `User-Agent` its
/// CONNECT head names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tunnel {
    pub proxy: super::proxy::Proxy,
    pub user_agent: String,
}

/// An open connection.
pub enum Conn {
    Tcp(std::net::TcpStream),
    Tls(Box<super::tls::Stream>),
    #[cfg(unix)]
    Unix(std::os::unix::net::UnixStream),
}

impl Conn {
    fn set_timeouts(&self, d: Duration) -> io::Result<()> {
        match self {
            Conn::Tcp(s) => {
                s.set_read_timeout(Some(d))?;
                s.set_write_timeout(Some(d))
            }
            Conn::Tls(s) => super::tls::stream_timeouts(s, d),
            #[cfg(unix)]
            Conn::Unix(s) => {
                s.set_read_timeout(Some(d))?;
                s.set_write_timeout(Some(d))
            }
        }
    }
}

impl Read for Conn {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Conn::Tcp(s) => s.read(buf),
            Conn::Tls(s) => super::tls::read(s, buf),
            #[cfg(unix)]
            Conn::Unix(s) => s.read(buf),
        }
    }
}

impl Write for Conn {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Conn::Tcp(s) => s.write(buf),
            Conn::Tls(s) => s.write(buf),
            #[cfg(unix)]
            Conn::Unix(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Conn::Tcp(s) => s.flush(),
            Conn::Tls(s) => s.flush(),
            #[cfg(unix)]
            Conn::Unix(s) => s.flush(),
        }
    }
}

/// How an exchange failed, in curl's terms.
#[derive(Debug)]
pub enum Failure {
    /// Nothing accepted the connection (curl exit 7).
    Connect(io::Error),
    /// The connect took longer than allowed (curl exit 28 with no
    /// "Connected to" in its trace).
    ConnectTimedOut,
    /// Connected, and not one byte of the request went out (the first
    /// write failed).
    NotSent(String),
    /// Connected, and no full answer within the time allowed (curl exit
    /// 28 after connecting).
    TimedOut,
    /// The connection closed before a single byte of answer (curl exit
    /// 52, "Empty reply from server").
    Empty,
    /// Anything else: a write or read that failed, an answer cut short, a
    /// head that is not HTTP. `curl` is curl's exit code for it.
    Broken { curl: i32, why: String },
    /// The proxy's CONNECT failed (not a 2xx, closed, not HTTP): curl's
    /// 56, and nothing of the request went out.
    Tunnel(String),
}

/// curl's exit codes for the failures here (`man curl`, EXIT CODES).
pub mod curl {
    /// 1: the answer is not HTTP (curl: "Received HTTP/0.9 when not
    /// allowed").
    pub const UNSUPPORTED_PROTOCOL: i32 = 1;
    /// 3: a URL curl won't send as it is.
    pub const URL_MALFORMAT: i32 = 3;
    /// 6: the host name doesn't resolve.
    pub const COULDNT_RESOLVE_HOST: i32 = 6;
    /// 7: nothing accepted the connection.
    pub const COULDNT_CONNECT: i32 = 7;
    /// 8: an HTTP head curl can't read (a header with no colon, a
    /// `Content-Length` that is no number).
    pub const WEIRD_SERVER_REPLY: i32 = 8;
    /// 18: the answer ended before its `Content-Length`.
    pub const PARTIAL_FILE: i32 = 18;
    /// 28: no answer in time, or no connect in time.
    pub const OPERATION_TIMEDOUT: i32 = 28;
    /// 52: the connection closed with nothing sent back.
    pub const GOT_NOTHING: i32 = 52;
    /// 55: a write of the request failed.
    pub const SEND_ERROR: i32 = 55;
    /// 56: a read of the answer failed (a reset), or a chunked body is
    /// broken.
    pub const RECV_ERROR: i32 = 56;
}

impl Failure {
    /// True when some of the request may have reached the server: every
    /// failure but a connect that failed or timed out, a name that didn't
    /// resolve, a TLS handshake that failed ([`super::tls::before_request`])
    /// and a request that went out with no byte at all. Such a request may
    /// have been run.
    pub fn sent(&self) -> bool {
        match self {
            Failure::Connect(_)
            | Failure::ConnectTimedOut
            | Failure::NotSent(_)
            | Failure::Tunnel(_) => false,
            Failure::Broken { curl, .. } => !super::tls::before_request(*curl),
            _ => true,
        }
    }

    /// curl's exit code for the same failure.
    pub fn curl_code(&self) -> i32 {
        match self {
            Failure::Connect(_) => curl::COULDNT_CONNECT,
            Failure::ConnectTimedOut | Failure::TimedOut => curl::OPERATION_TIMEDOUT,
            Failure::NotSent(_) => curl::SEND_ERROR,
            Failure::Tunnel(_) => curl::RECV_ERROR,
            Failure::Empty => curl::GOT_NOTHING,
            Failure::Broken { curl, .. } => *curl,
        }
    }
}

fn broken(curl: i32, why: impl Into<String>) -> Failure {
    Failure::Broken {
        curl,
        why: why.into(),
    }
}

/// The answer: the last status, the body, and the daemon's version when
/// its head names one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub status: u16,
    pub body: Vec<u8>,
    /// The first `X-K5-Server-Version` of the last head, trimmed, as it
    /// came ([`SERVER_VERSION_HEADER`]). What it may hold is checked where
    /// it is used ([`super::skew`]).
    pub server_version: Option<String>,
}

/// The header a daemon on this machine puts on every `/cli/*` answer:
/// its version, the one `/boot-status` reports (slice 10a). A LAN, tunnel
/// or proxied answer has none, nor does an older daemon.
pub const SERVER_VERSION_HEADER: &str = "X-K5-Server-Version";

fn remaining(deadline: Instant) -> Option<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
}

fn is_timeout(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
    )
}

/// Opens the connection: within `connect` and before `deadline`,
/// whichever comes first, as curl's `--connect-timeout` and `--max-time`.
pub fn connect(target: &Target, connect: Duration, deadline: Instant) -> Result<Conn, Failure> {
    let start = Instant::now();
    let limit = (start + connect).min(deadline);
    match target {
        Target::Tcp { host, port } => tcp(host, *port, limit).map(Conn::Tcp),
        Target::Tls { host, port, via } => {
            let s = match via {
                None => tcp(host, *port, limit)?,
                Some(t) => super::proxy::tunnel(&t.proxy, host, *port, &t.user_agent, limit, tcp)?,
            };
            super::tls::connect(s, host, limit).map(|t| Conn::Tls(Box::new(t)))
        }
        #[cfg(unix)]
        Target::Unix(path) => {
            // std has no connect timeout for a Unix socket, and a full
            // listen queue can hold a blocking connect on Linux. The
            // connect runs on its own thread; one that never returns is
            // left behind when the time is up.
            let Some(budget) = remaining(limit) else {
                return Err(Failure::ConnectTimedOut);
            };
            let (tx, rx) = std::sync::mpsc::channel();
            let path = path.clone();
            std::thread::spawn(move || {
                let _ = tx.send(std::os::unix::net::UnixStream::connect(&path));
            });
            match rx.recv_timeout(budget) {
                Ok(Ok(s)) => Ok(Conn::Unix(s)),
                Ok(Err(e)) => Err(Failure::Connect(e)),
                Err(_) => Err(Failure::ConnectTimedOut),
            }
        }
    }
}

/// The TCP connect of [`connect`], before `limit`.
fn tcp(host: &str, port: u16, limit: Instant) -> Result<std::net::TcpStream, Failure> {
    use std::net::ToSocketAddrs;
    // A name that doesn't resolve is curl's exit 6, never retried.
    let addrs: Vec<_> = (host, port)
        .to_socket_addrs()
        .map_err(|e| broken(curl::COULDNT_RESOLVE_HOST, format!("resolve {host}: {e}")))?
        .collect();
    let mut last = io::Error::new(io::ErrorKind::NotFound, format!("{host} has no address"));
    for addr in addrs {
        let Some(budget) = remaining(limit) else {
            return Err(Failure::ConnectTimedOut);
        };
        match std::net::TcpStream::connect_timeout(&addr, budget) {
            Ok(s) => return Ok(s),
            Err(e) if is_timeout(&e) => return Err(Failure::ConnectTimedOut),
            Err(e) => last = e,
        }
    }
    Err(Failure::Connect(last))
}

/// Writes `request` on `conn` and reads the answer, before `deadline`.
/// A first write that puts no byte out is [`Failure::NotSent`]; any
/// failure after a byte went out may follow a request the server ran.
/// The answer is read up to its end (its length, its last chunk, or the
/// end of the stream when it has neither) and no further, so a reset
/// that follows a whole answer is never seen.
pub fn exchange(conn: &mut Conn, request: &Request, deadline: Instant) -> Result<Answer, Failure> {
    let bytes = request.encode();
    let budget =
        remaining(deadline).ok_or_else(|| Failure::NotSent("no time left to send".into()))?;
    conn.set_timeouts(budget)
        .map_err(|e| Failure::NotSent(format!("set a timeout: {e}")))?;
    let mut sent = 0;
    while sent < bytes.len() {
        match conn.write(&bytes[sent..]) {
            Ok(0) if sent == 0 => {
                return Err(Failure::NotSent(
                    "send: the connection took no bytes".into(),
                ))
            }
            Ok(0) => {
                return Err(broken(
                    curl::SEND_ERROR,
                    format!("send: the connection stopped after {sent} bytes"),
                ))
            }
            Ok(n) => sent += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) if sent == 0 => return Err(Failure::NotSent(format!("send: {e}"))),
            Err(e) if is_timeout(&e) => return Err(Failure::TimedOut),
            Err(e) => return Err(broken(curl::SEND_ERROR, format!("send: {e}"))),
        }
    }
    conn.flush()
        .map_err(|e| broken(curl::SEND_ERROR, format!("send: {e}")))?;
    read_answer(conn, deadline)
}

/// Reads with the time left before `deadline`.
struct Reader<'a> {
    conn: &'a mut Conn,
    deadline: Instant,
    buf: Vec<u8>,
    /// Bytes read from the connection, all told.
    total: usize,
    eof: bool,
}

impl Reader<'_> {
    /// Reads more into `buf`. False at the end of the stream.
    fn fill(&mut self) -> Result<bool, Failure> {
        if self.eof {
            return Ok(false);
        }
        let budget = remaining(self.deadline).ok_or(Failure::TimedOut)?;
        match self.conn.set_timeouts(budget) {
            Ok(()) => {}
            // macOS refuses a new timeout (EINVAL) on a socket whose peer
            // has closed, often right after a whole answer came in. A read
            // there never waits, so it goes ahead under the timeout set
            // before the request was written. Treating it as a failure lost
            // answers that had arrived.
            Err(e) if e.kind() == io::ErrorKind::InvalidInput => {}
            Err(e) => return Err(broken(curl::RECV_ERROR, format!("set a timeout: {e}"))),
        }
        let mut chunk = [0u8; 16 * 1024];
        loop {
            match self.conn.read(&mut chunk) {
                Ok(0) => {
                    self.eof = true;
                    return Ok(false);
                }
                Ok(n) => {
                    self.buf.extend_from_slice(&chunk[..n]);
                    self.total += n;
                    return Ok(true);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) if is_timeout(&e) => return Err(Failure::TimedOut),
                Err(e) => return Err(broken(curl::RECV_ERROR, format!("read: {e}"))),
            }
        }
    }

    /// Takes one line (without its CRLF or LF) from the front of `buf`.
    fn line(&mut self) -> Result<Vec<u8>, Failure> {
        loop {
            if let Some(at) = self.buf.iter().position(|b| *b == b'\n') {
                let mut line: Vec<u8> = self.buf.drain(..=at).collect();
                line.pop();
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                return Ok(line);
            }
            if !self.fill()? {
                return Err(if self.total == 0 {
                    Failure::Empty
                } else {
                    broken(curl::RECV_ERROR, "the answer ended inside its head")
                });
            }
        }
    }

    /// Takes exactly `n` bytes.
    fn exact(&mut self, n: usize) -> Result<Vec<u8>, Failure> {
        while self.buf.len() < n {
            if !self.fill()? {
                return Err(broken(
                    curl::PARTIAL_FILE,
                    format!("the answer ended {} bytes short", n - self.buf.len()),
                ));
            }
        }
        Ok(self.buf.drain(..n).collect())
    }

    fn rest(&mut self) -> Result<Vec<u8>, Failure> {
        while self.fill()? {}
        Ok(std::mem::take(&mut self.buf))
    }
}

/// Reads one answer: skips any `1xx` head, then reads the body by
/// `Content-Length`, chunked, or to the end of the stream.
pub fn read_answer(conn: &mut Conn, deadline: Instant) -> Result<Answer, Failure> {
    let mut r = Reader {
        conn,
        deadline,
        buf: Vec::new(),
        total: 0,
        eof: false,
    };
    loop {
        let status_line = r.line()?;
        let status = parse_status(&status_line)?;
        let mut length: Option<usize> = None;
        let mut chunked = false;
        let mut server_version: Option<String> = None;
        loop {
            let line = r.line()?;
            if line.is_empty() {
                break;
            }
            let text = String::from_utf8_lossy(&line);
            let Some((name, value)) = text.split_once(':') else {
                return Err(broken(
                    curl::WEIRD_SERVER_REPLY,
                    format!("a header line with no colon: {text}"),
                ));
            };
            let value = value.trim();
            if name.eq_ignore_ascii_case("content-length") {
                length = Some(value.parse().map_err(|_| {
                    broken(
                        curl::WEIRD_SERVER_REPLY,
                        format!("Content-Length {value:?}"),
                    )
                })?);
            } else if name.eq_ignore_ascii_case("transfer-encoding")
                && value.to_ascii_lowercase().contains("chunked")
            {
                chunked = true;
            } else if name.eq_ignore_ascii_case(SERVER_VERSION_HEADER) && server_version.is_none() {
                server_version = Some(value.to_string());
            }
        }
        if (100..200).contains(&status) {
            continue;
        }
        let body = if status == 204 || status == 304 {
            Vec::new()
        } else if chunked {
            let mut body = Vec::new();
            loop {
                let size_line = r.line()?;
                let text = String::from_utf8_lossy(&size_line);
                let hex = text.split(';').next().unwrap_or("").trim();
                let size = usize::from_str_radix(hex, 16)
                    .map_err(|_| broken(curl::RECV_ERROR, format!("a chunk size {text:?}")))?;
                if size == 0 {
                    // Trailers, up to the empty line.
                    while !r.line()?.is_empty() {}
                    break;
                }
                body.extend(r.exact(size)?);
                if !r.line()?.is_empty() {
                    return Err(broken(curl::RECV_ERROR, "a chunk that doesn't end in CRLF"));
                }
            }
            body
        } else if let Some(n) = length {
            r.exact(n)?
        } else {
            r.rest()?
        };
        return Ok(Answer {
            status,
            body,
            server_version,
        });
    }
}

fn parse_status(line: &[u8]) -> Result<u16, Failure> {
    let text = String::from_utf8_lossy(line);
    let mut parts = text.splitn(3, ' ');
    let version = parts.next().unwrap_or("");
    let code = parts.next().unwrap_or("");
    // curl reads an answer that doesn't start with `HTTP/` as HTTP/0.9
    // and refuses it (exit 1); a status line it can't read is exit 1 too.
    let bad = || {
        broken(
            curl::UNSUPPORTED_PROTOCOL,
            format!("not an HTTP status line: {text}"),
        )
    };
    if !version.starts_with("HTTP/1.")
        || code.len() != 3
        || !code.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(bad());
    }
    code.parse().map_err(|_| bad())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn req(method: Method, body: Option<Body>, headers: &[(&str, &str)]) -> Request {
        Request {
            method,
            target: "/cli/x?token=t".into(),
            host: "127.0.0.1:9".into(),
            user_agent: "k5/1.2.3".into(),
            headers: headers
                .iter()
                .map(|(n, v)| (n.to_string(), v.to_string()))
                .collect(),
            body,
        }
    }

    #[test]
    fn a_get_is_curl_s_lines() {
        let r = req(Method::Get, None, &[("X-A", "1"), ("X-B", "two")]);
        assert_eq!(
            String::from_utf8(r.encode()).unwrap(),
            "GET /cli/x?token=t HTTP/1.1\r\nHost: 127.0.0.1:9\r\nUser-Agent: k5/1.2.3\r\nAccept: */*\r\nX-A: 1\r\nX-B: two\r\n\r\n"
        );
    }

    #[test]
    fn a_custom_type_comes_before_the_length_and_curl_s_own_after() {
        let json = req(
            Method::Post,
            Some(Body {
                bytes: b"{}".to_vec(),
                curl_form_type: false,
            }),
            &[("Content-Type", "application/json")],
        );
        assert_eq!(
            String::from_utf8(json.encode()).unwrap(),
            "POST /cli/x?token=t HTTP/1.1\r\nHost: 127.0.0.1:9\r\nUser-Agent: k5/1.2.3\r\nAccept: */*\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}"
        );
        let empty = req(
            Method::Post,
            Some(Body {
                bytes: Vec::new(),
                curl_form_type: true,
            }),
            &[],
        );
        assert_eq!(
            String::from_utf8(empty.encode()).unwrap(),
            "POST /cli/x?token=t HTTP/1.1\r\nHost: 127.0.0.1:9\r\nUser-Agent: k5/1.2.3\r\nAccept: */*\r\nContent-Length: 0\r\nContent-Type: application/x-www-form-urlencoded\r\n\r\n"
        );
    }

    #[test]
    fn get_data_has_upper_case_escapes() {
        assert_eq!(
            curl_get_query("project=%2ftmp%2Fa%20b&x=%7e"),
            "project=%2Ftmp%2Fa%20b&x=%7E"
        );
        // Not escapes: left alone.
        assert_eq!(curl_get_query("a=%zz&b=%2&c=%"), "a=%zz&b=%2&c=%");
        assert_eq!(curl_get_query("a=b%c3%a9"), "a=b%C3%A9");
        assert_eq!(append_get_query("/cli/x", "a=%2f"), "/cli/x?a=%2F");
        assert_eq!(
            append_get_query("/cli/x?y=%2f", "a=%2f"),
            "/cli/x?y=%2f&a=%2F"
        );
    }

    #[test]
    fn data_urlencode_is_curl_s() {
        assert_eq!(
            curl_data_urlencode("token", "a/b c~.-_é+"),
            "token=a%2Fb+c~.-_%C3%A9%2B"
        );
        assert_eq!(curl_data_urlencode("token", ""), "token=");
    }

    #[test]
    fn targets_curl_would_change_are_refused() {
        assert_eq!(check_target("/cli/x?a=%20"), Ok(()));
        assert_eq!(check_target("/cli/x?a= "), Err(b' '));
        assert_eq!(check_target("/cli/x#f"), Err(b'#'));
        assert_eq!(check_target("/cli/x\n"), Err(b'\n'));
        assert_eq!(check_target("/cli/x\x7f"), Err(0x7f));
    }

    /// Serves one connection with `reply`'s bytes, then closes.
    fn serve_once(reply: Vec<u8>, pause: Duration) -> (u16, std::thread::JoinHandle<Vec<u8>>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let h = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut got = Vec::new();
            let mut chunk = [0u8; 4096];
            while !got.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = s.read(&mut chunk).unwrap();
                if n == 0 {
                    break;
                }
                got.extend_from_slice(&chunk[..n]);
            }
            std::thread::sleep(pause);
            s.write_all(&reply).unwrap();
            got
        });
        (port, h)
    }

    fn one(reply: &[u8], max: Duration) -> Result<Answer, Failure> {
        let (port, h) = serve_once(reply.to_vec(), Duration::ZERO);
        let deadline = Instant::now() + max;
        let mut c = connect(
            &Target::Tcp {
                host: "127.0.0.1".into(),
                port,
            },
            Duration::from_secs(5),
            deadline,
        )
        .unwrap_or_else(|f| panic!("connect: {f:?}"));
        let r = req(Method::Get, None, &[]);
        let out = exchange(&mut c, &r, deadline);
        drop(c);
        assert_eq!(h.join().unwrap(), r.encode());
        out
    }

    #[test]
    fn answers_by_length_chunks_and_close() {
        let a = one(
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhelloEXTRA",
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(
            a,
            Answer {
                status: 200,
                body: b"hello".to_vec(),
                server_version: None
            }
        );
        let a = one(b"HTTP/1.1 403 Forbidden\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2;x=1\r\nde\r\n0\r\nT: 1\r\n\r\n", Duration::from_secs(5)).unwrap();
        assert_eq!(
            a,
            Answer {
                status: 403,
                body: b"abcde".to_vec(),
                server_version: None
            }
        );
        let a = one(
            b"HTTP/1.0 503 Service Unavailable\r\n\r\n{\"state\":\"migrating\"}",
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(
            a,
            Answer {
                status: 503,
                body: b"{\"state\":\"migrating\"}".to_vec(),
                server_version: None
            }
        );
        let a = one(
            b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 201 Created\r\nContent-Length: 0\r\n\r\n",
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(
            a,
            Answer {
                status: 201,
                body: Vec::new(),
                server_version: None
            }
        );
    }

    /// The version header of the last head, the first one there, by any
    /// case of its name; a 1xx head's never counts.
    #[test]
    fn the_server_version_is_read_from_the_last_head() {
        let a = one(b"HTTP/1.1 200 OK\r\nx-k5-server-version:  0.46.16 \r\nX-K5-Server-Version: 9.9.9\r\nContent-Length: 2\r\n\r\n{}", Duration::from_secs(5)).unwrap();
        assert_eq!(
            a,
            Answer {
                status: 200,
                body: b"{}".to_vec(),
                server_version: Some("0.46.16".into())
            }
        );
        let a = one(b"HTTP/1.1 100 Continue\r\nX-K5-Server-Version: 1.0.0\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n", Duration::from_secs(5)).unwrap();
        assert_eq!(a.server_version, None);
        let a = one(
            b"HTTP/1.1 403 Forbidden\r\nX-K5-Server-Version-Extra: 1\r\nContent-Length: 0\r\n\r\n",
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(a.server_version, None, "another header's name");
    }

    #[test]
    fn nothing_back_is_empty_and_a_short_answer_is_broken() {
        assert!(matches!(
            one(b"", Duration::from_secs(5)),
            Err(Failure::Empty)
        ));
        assert!(matches!(
            one(
                b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\nabc",
                Duration::from_secs(5)
            ),
            Err(Failure::Broken { curl: 18, .. })
        ));
        assert!(matches!(
            one(b"SSH-2.0-x\r\n\r\n", Duration::from_secs(5)),
            Err(Failure::Broken { curl: 1, .. })
        ));
        assert!(matches!(
            one(
                b"HTTP/1.1 200 OK\r\nbad header\r\n\r\n",
                Duration::from_secs(5)
            ),
            Err(Failure::Broken { curl: 8, .. })
        ));
    }

    #[test]
    fn an_answer_past_the_deadline_is_a_timeout() {
        let (port, h) = serve_once(
            b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_vec(),
            Duration::from_millis(1500),
        );
        let deadline = Instant::now() + Duration::from_millis(300);
        let mut c = match connect(
            &Target::Tcp {
                host: "127.0.0.1".into(),
                port,
            },
            Duration::from_secs(5),
            deadline,
        ) {
            Ok(c) => c,
            Err(f) => panic!("connect: {f:?}"),
        };
        let t0 = Instant::now();
        assert!(matches!(
            exchange(&mut c, &req(Method::Get, None, &[]), deadline),
            Err(Failure::TimedOut)
        ));
        assert!(
            t0.elapsed() < Duration::from_millis(1200),
            "{:?}",
            t0.elapsed()
        );
        drop(c);
        h.join().unwrap();
    }

    #[test]
    fn a_closed_port_is_a_connect_failure() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        let deadline = Instant::now() + Duration::from_secs(5);
        assert!(matches!(
            connect(
                &Target::Tcp {
                    host: "127.0.0.1".into(),
                    port
                },
                Duration::from_secs(5),
                deadline
            ),
            Err(Failure::Connect(_))
        ));
    }

    /// The whole answer, then the peer closed (the cell socket) or reset
    /// (TCP, `SO_LINGER` 0) before a byte was read: the answer stands.
    /// macOS refuses a read timeout on such a socket (EINVAL); that used to
    /// lose the answer.
    #[cfg(unix)]
    #[test]
    fn a_whole_answer_then_a_close_or_reset_is_the_answer() {
        use std::os::unix::net::UnixStream;
        let reply: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello";
        let deadline = Instant::now() + Duration::from_secs(5);
        let (ours, mut peer) = UnixStream::pair().unwrap();
        peer.write_all(reply).unwrap();
        peer.shutdown(std::net::Shutdown::Both).unwrap();
        drop(peer);
        let mut c = Conn::Unix(ours);
        assert_eq!(
            read_answer(&mut c, deadline).unwrap(),
            Answer {
                status: 200,
                body: b"hello".to_vec(),
                server_version: None
            }
        );

        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let mut c = connect(
            &Target::Tcp {
                host: "127.0.0.1".into(),
                port,
            },
            Duration::from_secs(5),
            deadline,
        )
        .unwrap_or_else(|f| panic!("connect: {f:?}"));
        let (mut s, _) = l.accept().unwrap();
        s.write_all(reply).unwrap();
        reset(s);
        // The reset lands before the first read.
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(
            read_answer(&mut c, deadline).unwrap(),
            Answer {
                status: 200,
                body: b"hello".to_vec(),
                server_version: None
            }
        );
    }

    /// A reset before any answer is a read that failed: curl's 56, and the
    /// request may have run.
    #[cfg(unix)]
    #[test]
    fn a_reset_before_the_answer_is_a_receive_failure() {
        let deadline = Instant::now() + Duration::from_secs(5);
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let mut c = connect(
            &Target::Tcp {
                host: "127.0.0.1".into(),
                port,
            },
            Duration::from_secs(5),
            deadline,
        )
        .unwrap_or_else(|f| panic!("connect: {f:?}"));
        let (s, _) = l.accept().unwrap();
        reset(s);
        std::thread::sleep(Duration::from_millis(200));
        let f = read_answer(&mut c, deadline).unwrap_err();
        assert!(matches!(f, Failure::Broken { curl: 56, .. }), "{f:?}");
        assert_eq!(f.curl_code(), 56);
        assert!(f.sent());
        assert_eq!(
            [
                Failure::Empty.curl_code(),
                Failure::TimedOut.curl_code(),
                Failure::ConnectTimedOut.curl_code(),
                Failure::NotSent("x".into()).curl_code()
            ],
            [52, 28, 28, 55]
        );
    }

    /// Closes `s` with a reset (`SO_LINGER` 0) instead of a FIN.
    #[cfg(unix)]
    fn reset(s: std::net::TcpStream) {
        use std::os::unix::io::AsRawFd;
        let linger = libc::linger {
            l_onoff: 1,
            l_linger: 0,
        };
        // SAFETY: a valid socket and a `linger` of the size passed.
        let rc = unsafe {
            libc::setsockopt(
                s.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_LINGER,
                (&linger as *const libc::linger).cast(),
                std::mem::size_of::<libc::linger>() as libc::socklen_t,
            )
        };
        assert_eq!(rc, 0, "SO_LINGER: {}", io::Error::last_os_error());
        drop(s);
    }

    /// A peer that closed before the request went out: nothing was sent.
    #[cfg(unix)]
    #[test]
    fn a_request_to_a_closed_peer_is_not_sent() {
        use std::os::unix::net::UnixStream;
        let (ours, peer) = UnixStream::pair().unwrap();
        drop(peer);
        let mut c = Conn::Unix(ours);
        let f = exchange(
            &mut c,
            &req(
                Method::Post,
                Some(Body {
                    bytes: b"x".to_vec(),
                    curl_form_type: true,
                }),
                &[],
            ),
            Instant::now() + Duration::from_secs(5),
        )
        .unwrap_err();
        assert!(matches!(f, Failure::NotSent(_)), "{f:?}");
        assert!(!f.sent());
        assert!(
            Failure::Empty.sent()
                && Failure::TimedOut.sent()
                && broken(curl::RECV_ERROR, "x").sent()
        );
        assert!(!broken(curl::COULDNT_RESOLVE_HOST, "x").sent());
        assert!(!Failure::ConnectTimedOut.sent());
    }

    #[cfg(unix)]
    #[test]
    fn a_missing_socket_is_a_connect_failure() {
        let path = std::env::temp_dir().join(format!("k5-wire-none-{}.sock", std::process::id()));
        let deadline = Instant::now() + Duration::from_secs(5);
        assert!(matches!(
            connect(&Target::Unix(path), Duration::from_secs(5), deadline),
            Err(Failure::Connect(_))
        ));
    }
}
