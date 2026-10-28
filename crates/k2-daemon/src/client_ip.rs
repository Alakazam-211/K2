//! The real client address for tunnelled Apps (0.46.3; Rosson 2026-10-20).
//!
//! Before this, K2 Connect reached the box as raw bytes: HAProxy passthrough
//! → frps → frpc → the E2E listener, which terminates TLS and splices to the
//! App. Every App saw peer 127.0.0.1 and got the client's own
//! `X-Forwarded-For` unchanged, so a per-IP limit or lockout in an App was
//! bypassable.
//!
//! Now:
//!
//! 1. **Edge.** HAProxy sends PROXY v2 to frps (patched to read it, from
//!    loopback only), so frps knows the client. frpc's
//!    `transport.proxyProtocolVersion = "v2"` puts one PROXY v2 header in
//!    front of each connection it forwards to the E2E listener.
//! 2. **Box, trust.** Only the E2E listener reads that header (it is frpc's
//!    `localPort`, bound on loopback): [`read_tunnel_preamble`]. A header
//!    from a non-loopback peer is never read. No other listener (main HTTP,
//!    tunnel ingress, App front ports, the :443 front) ever parses one. A
//!    stream that starts with a TLS record has no header (frpc on an old
//!    config): no client IP. A loopback / unspecified source (an old relay,
//!    where frps only saw HAProxy) is no client IP either. The relay only
//!    passes streams that start with a TLS ClientHello, so the first bytes
//!    can't be a client-written header.
//! 3. **Box, Apps.** Where the E2E listener splices to an App (unless its
//!    manifest says `host.clientIp: false`; a `--cmd` App starts raw), or to
//!    a nested service that opted in (`k2 publish client-ip <label> on`;
//!    nested services stay a raw splice by default, any protocol),
//!    [`forward`] runs every HTTP/1.1 request on the connection
//!    through a rewrite: every client copy of the headers K2 owns
//!    ([`OWNED`]: `X-K2-Client-IP`, `X-Forwarded-For`, `X-Real-IP`,
//!    `Forwarded`, …; `_` spellings too) is dropped, and when K2 knows the
//!    client it adds exactly one `X-K2-Client-IP`, `X-Forwarded-For` and
//!    `X-Real-IP` with that address. No client IP → none of them, never a
//!    client's. Keep-alive, pipelining, chunked bodies, `Expect:
//!    100-continue` and WebSocket upgrades work; after a `101` the
//!    connection is raw both ways. HTTP/2 can't arrive: the listener's ALPN
//!    offers only `http/1.1`, and anything that isn't HTTP/1.x (the h2
//!    preface included) is answered `400` and closed rather than passed
//!    through unrewritten.
//! 4. **Box, K2 itself.** The splice to the tunnel-ingress dispatcher is a
//!    loopback hop; the splicer records `hop local address → client IP`
//!    ([`register_hop`]) and the dispatcher reads it back by its peer
//!    address ([`hop_client_ip`]) for the login throttle and the auth
//!    audit. Only this process writes the table, so nothing a client sends
//!    can put an address in it.
//!
//! An App's local front port (`apps::router`) gets the same rewrite in
//! [`Mode::Local`]: K2's own hop keeps its address; any other local caller
//! only loses `X-K2-Client-IP` (a local reverse proxy's `X-Forwarded-For`
//! is its own business, as today).
//!
//! **K2's own answers on an App's host** (App phone pairing, 0.46.5):
//! [`forward_with`] takes a [`LocalAnswer`]. A `GET`/`HEAD` with no body
//! that it answers (the pairing link files, `app_pairing::links`) never
//! reaches the App: K2's answer is written in order, after every earlier
//! response on the connection, then the connection closes
//! (`Connection: close`), at any position on a keep-alive connection.
//! [`peek_head`] reads a connection's first request head before the App
//! is dialed, so the files are served while the App is down.

use std::collections::{HashMap, VecDeque};
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Mutex, OnceLock};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{watch, Notify};

use crate::front443::proxy_v2;

/// The one header Apps should trust for the client address.
pub const HEADER: &str = "X-K2-Client-IP";

/// Request headers K2 owns on the way into an App (lowercase, `-` form).
/// Every client copy is dropped; K2 sets the first three itself.
pub const OWNED: &[&str] = &[
    "x-k2-client-ip",
    "x-forwarded-for",
    "x-real-ip",
    "forwarded",
    "x-client-ip",
    "true-client-ip",
    "cf-connecting-ip",
    "x-cluster-client-ip",
    "fastly-client-ip",
    "x-original-forwarded-for",
];

/// Longest request / response head we buffer.
const MAX_HEAD: usize = 64 * 1024;
/// Most header lines in a request (more is a 431).
const MAX_REQ_HEADERS: usize = 128;
/// Most header lines in a response we can still frame.
const MAX_RESP_HEADERS: usize = 256;
/// Longest chunk-size or trailer line.
const MAX_LINE: usize = 8 * 1024;

/// A header name as frameworks compare it: lowercase, `_` read as `-`
/// (CGI-style stacks fold `X_Forwarded_For` into `X-Forwarded-For`).
fn normalized(name: &str) -> String {
    name.bytes().map(|b| if b == b'_' { '-' } else { b.to_ascii_lowercase() as char }).collect()
}

fn is_owned(norm: &str) -> bool {
    OWNED.contains(&norm)
}

// ── PROXY v2 on the E2E listener ──────────────────────────────────────

/// The client address a parsed header vouches for. Loopback and
/// unspecified sources are no client: an old relay's frps only saw HAProxy
/// on 127.0.0.1.
pub fn trusted_source(p: &proxy_v2::Parsed) -> Option<IpAddr> {
    match p {
        proxy_v2::Parsed::Tcp { src, .. } => {
            let ip = proxy_v2::canonical(src.ip());
            (!(ip.is_loopback() || ip.is_unspecified() || ip.is_multicast())).then_some(ip)
        }
        proxy_v2::Parsed::Local | proxy_v2::Parsed::Other => None,
    }
}

/// The [`read_tunnel_preamble`] error for a loopback peer that connected
/// and closed without sending a byte.
pub const CLOSED_BEFORE_FIRST_BYTE: &str = "closed before the first byte";

/// Read frpc's PROXY v2 header off a connection the E2E listener accepted,
/// before the TLS handshake, by `deadline`.
///
/// - `peer` not loopback → nothing is read, `Ok(None)` (frpc is local).
/// - The first byte isn't the v2 signature's (a TLS record: frpc on an
///   older config) → nothing is read, `Ok(None)`.
/// - A header → it is consumed; `Ok(Some(ip))` when it names a real
///   client ([`trusted_source`]), else `Ok(None)`.
/// - A malformed or truncated header, a close or the deadline → `Err`;
///   the caller drops the connection. A close before any byte is exactly
///   [`CLOSED_BEFORE_FIRST_BYTE`] (a loopback liveness probe, see
///   `tunnel_tls_listener::serve_one`).
pub async fn read_tunnel_preamble(
    tcp: &mut TcpStream,
    peer: SocketAddr,
    deadline: tokio::time::Instant,
) -> Result<Option<IpAddr>, String> {
    if !proxy_v2::canonical(peer.ip()).is_loopback() {
        return Ok(None);
    }
    let read = async {
        let mut first = [0u8; 1];
        let n = tcp.peek(&mut first).await.map_err(|e| format!("read before TLS: {e}"))?;
        if n == 0 {
            return Err(CLOSED_BEFORE_FIRST_BYTE.to_string());
        }
        if first[0] != proxy_v2::SIGNATURE[0] {
            return Ok(None);
        }
        let mut header = vec![0u8; proxy_v2::FIXED_LEN];
        tcp.read_exact(&mut header).await.map_err(|e| format!("PROXY v2 header: {e}"))?;
        let len = proxy_v2::block_len(&header).map_err(|e| e.to_string())?;
        header.resize(proxy_v2::FIXED_LEN + len, 0);
        tcp.read_exact(&mut header[proxy_v2::FIXED_LEN..])
            .await
            .map_err(|e| format!("PROXY v2 address block: {e}"))?;
        let (parsed, _) = proxy_v2::parse(&header).map_err(|e| e.to_string())?;
        Ok(trusted_source(&parsed))
    };
    match tokio::time::timeout_at(deadline, read).await {
        Ok(r) => r,
        Err(_) => Err("timed out before the TLS handshake (slowloris/half-open) — dropped".to_string()),
    }
}

// ── K2's own loopback hops ────────────────────────────────────────────

fn hops() -> &'static Mutex<HashMap<SocketAddr, IpAddr>> {
    static H: OnceLock<Mutex<HashMap<SocketAddr, IpAddr>>> = OnceLock::new();
    H.get_or_init(|| Mutex::new(HashMap::new()))
}

fn canonical_addr(a: SocketAddr) -> SocketAddr {
    SocketAddr::new(proxy_v2::canonical(a.ip()), a.port())
}

/// While alive, the hop's local address maps to the client IP.
#[must_use = "the hop is forgotten when this guard drops"]
pub struct Hop(Option<SocketAddr>);

impl Drop for Hop {
    fn drop(&mut self) {
        if let Some(a) = self.0 {
            hops().lock().unwrap_or_else(|p| p.into_inner()).remove(&a);
        }
    }
}

/// Record that `upstream` (a loopback connection this daemon opened for a
/// tunnelled client) carries `ip`. Register before writing to it: the
/// other end looks it up once request bytes arrive.
pub fn register_hop(upstream: &TcpStream, ip: Option<IpAddr>) -> Hop {
    match (ip, upstream.local_addr()) {
        (Some(ip), Ok(local)) => {
            let local = canonical_addr(local);
            hops().lock().unwrap_or_else(|p| p.into_inner()).insert(local, ip);
            Hop(Some(local))
        }
        _ => Hop(None),
    }
}

/// The client behind a loopback peer, when the peer is one of K2's own
/// hops from the tunnel.
pub fn hop_client_ip(peer: SocketAddr) -> Option<IpAddr> {
    hops().lock().unwrap_or_else(|p| p.into_inner()).get(&canonical_addr(peer)).copied()
}

// ── The HTTP/1.1 rewrite ──────────────────────────────────────────────

/// Where the bytes came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Through the K2 Connect tunnel: drop every client copy of [`OWNED`];
    /// `Some(ip)` adds K2's own headers.
    Tunnel(Option<IpAddr>),
    /// An App's loopback front port. K2's own hop (found by `peer`) is
    /// treated as `Tunnel(Some(ip))`; anyone else only loses
    /// `X-K2-Client-IP`.
    Local { peer: Option<SocketAddr> },
}

/// What the rewrite does, resolved once per connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rule {
    /// Drop every [`OWNED`] header; add K2's when `Some`.
    Set(Option<IpAddr>),
    /// Drop `X-K2-Client-IP` only.
    StripK2,
}

impl Rule {
    fn drops(self, norm: &str) -> bool {
        match self {
            Rule::Set(_) => is_owned(norm),
            Rule::StripK2 => norm == "x-k2-client-ip",
        }
    }
}

fn resolve(mode: Mode) -> Rule {
    match mode {
        Mode::Tunnel(ip) => Rule::Set(ip),
        Mode::Local { peer } => match peer.and_then(hop_client_ip) {
            Some(ip) => Rule::Set(Some(ip)),
            None => Rule::StripK2,
        },
    }
}

/// A read half with its own buffer.
struct Reader<R> {
    r: R,
    buf: Vec<u8>,
    pos: usize,
}

impl<R: AsyncRead + Unpin> Reader<R> {
    fn new(r: R) -> Self {
        Self { r, buf: Vec::new(), pos: 0 }
    }

    fn data(&self) -> &[u8] {
        &self.buf[self.pos..]
    }

    fn consume(&mut self, n: usize) {
        self.pos += n;
        if self.pos >= self.buf.len() {
            self.buf.clear();
            self.pos = 0;
        }
    }

    /// Read more; `Ok(0)` at EOF. Cancel-safe (nothing is lost if the
    /// future is dropped before it completes).
    async fn fill(&mut self) -> io::Result<usize> {
        if self.pos > 0 {
            self.buf.drain(..self.pos);
            self.pos = 0;
        }
        let mut tmp = [0u8; 16 * 1024];
        let n = self.r.read(&mut tmp).await?;
        self.buf.extend_from_slice(&tmp[..n]);
        Ok(n)
    }

    async fn copy_exact<W: AsyncWrite + Unpin>(&mut self, mut n: u64, w: &mut W) -> io::Result<()> {
        while n > 0 {
            if self.data().is_empty() && self.fill().await? == 0 {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "body ended early"));
            }
            let take = (self.data().len() as u64).min(n) as usize;
            w.write_all(&self.data()[..take]).await?;
            self.consume(take);
            n -= take as u64;
        }
        Ok(())
    }

    /// Everything buffered, then the rest of the stream.
    async fn copy_rest<W: AsyncWrite + Unpin>(&mut self, w: &mut W) -> io::Result<()> {
        if !self.data().is_empty() {
            w.write_all(self.data()).await?;
            let n = self.data().len();
            self.consume(n);
        }
        tokio::io::copy(&mut self.r, w).await?;
        Ok(())
    }

    /// One CRLF-terminated line, without the CRLF. A bare LF, a stray CR
    /// or more than `MAX_LINE` bytes is an error. Returns the line and the
    /// raw bytes it took (for forwarding verbatim).
    async fn line(&mut self) -> io::Result<(Vec<u8>, Vec<u8>)> {
        loop {
            if let Some(i) = self.data().iter().position(|&b| b == b'\n') {
                let raw = self.data()[..=i].to_vec();
                self.consume(i + 1);
                let Some(body) = raw.strip_suffix(b"\r\n") else {
                    return Err(bad("line ends in a bare LF"));
                };
                if body.contains(&b'\r') {
                    return Err(bad("stray CR in a line"));
                }
                return Ok((body.to_vec(), raw));
            }
            if self.data().len() > MAX_LINE {
                return Err(bad("line too long"));
            }
            if self.fill().await? == 0 {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "stream ended mid-line"));
            }
        }
    }
}

fn bad(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.to_string())
}

/// A strict chunk-size: hex digits, then optional `;extensions` (BWS
/// allowed before `;`).
fn chunk_size(line: &[u8]) -> io::Result<u64> {
    let size_part = match line.iter().position(|&b| b == b';') {
        Some(i) => line[..i].trim_ascii_end(),
        None => line,
    };
    if size_part.is_empty() || size_part.len() > 15 || !size_part.iter().all(u8::is_ascii_hexdigit) {
        return Err(bad("bad chunk size"));
    }
    u64::from_str_radix(std::str::from_utf8(size_part).map_err(|_| bad("bad chunk size"))?, 16)
        .map_err(|_| bad("bad chunk size"))
}

/// A trailer / header line's name, when it is a valid token followed
/// directly by `:`.
fn field_name(line: &[u8]) -> Option<&str> {
    let i = line.iter().position(|&b| b == b':')?;
    let name = &line[..i];
    let tchar = |b: &u8| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(b);
    (!name.is_empty() && name.iter().all(tchar)).then(|| std::str::from_utf8(name).ok()).flatten()
}

/// Copy a chunked body. `rule` set: re-serialize the framing canonically
/// and drop owned trailers (requests: the App sees exactly the framing we
/// read). `None`: forward the bytes verbatim (responses).
async fn copy_chunked<R, W>(rd: &mut Reader<R>, w: &mut W, rule: Option<Rule>) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    loop {
        let (line, raw) = rd.line().await?;
        let size = chunk_size(&line)?;
        match rule {
            Some(_) => w.write_all(format!("{size:x}\r\n").as_bytes()).await?,
            None => w.write_all(&raw).await?,
        }
        if size == 0 {
            loop {
                let (t, raw) = rd.line().await?;
                if t.is_empty() {
                    w.write_all(b"\r\n").await?;
                    return Ok(());
                }
                let name = field_name(&t).ok_or_else(|| bad("bad trailer line"))?;
                match rule {
                    Some(rule) if rule.drops(&normalized(name)) => {}
                    Some(_) => {
                        w.write_all(&t).await?;
                        w.write_all(b"\r\n").await?;
                    }
                    None => w.write_all(&raw).await?,
                }
            }
        }
        rd.copy_exact(size, w).await?;
        let (end, _) = rd.line().await?;
        if !end.is_empty() {
            return Err(bad("chunk data longer than its size"));
        }
        w.write_all(b"\r\n").await?;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Body {
    None,
    Length(u64),
    Chunked,
}

/// One rewritten request head.
#[derive(Debug)]
struct RequestHead {
    bytes: Vec<u8>,
    body: Body,
    is_head: bool,
    upgrade: bool,
    method: String,
    path: String,
    /// The request's `Host` (the last one when sent twice).
    host: Option<String>,
}

/// K2's own answer to a request on an App's connection: `(method,
/// path and query, Host)` → the whole response (it must end the
/// connection: `Connection: close`), or `None` for the App to answer.
pub type LocalAnswer = std::sync::Arc<dyn Fn(&str, &str, Option<&str>) -> Option<Vec<u8>> + Send + Sync>;

#[derive(Debug)]
enum ReadRequest {
    /// The client closed between requests.
    Eof,
    Head(RequestHead),
    /// Answer this ourselves and stop (malformed, too large, not HTTP/1.x).
    Refuse(&'static str),
}

/// K2's own answer to a request it won't forward.
fn refusal(status: &str) -> Vec<u8> {
    let body = "K2 could not forward this request.\r\n";
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nCache-Control: no-store\r\nContent-Length: {}\r\n{}Connection: close\r\n\r\n{body}",
        body.len(),
        k2_core::frame_policy::CONNECT_HEADER_LINES,
    )
    .into_bytes()
}

fn comma_tokens(v: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(v).split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect()
}

/// Rewrite one parsed request. `Err(status)` refuses it.
fn rewrite_request(req: &httparse::Request, rule: Rule) -> Result<RequestHead, &'static str> {
    let method = req.method.ok_or("400 Bad Request")?;
    let path = req.path.ok_or("400 Bad Request")?;
    let minor = req.version.ok_or("400 Bad Request")?;
    if method.eq_ignore_ascii_case("CONNECT") {
        return Err("400 Bad Request");
    }
    let mut out = Vec::with_capacity(512);
    out.extend_from_slice(format!("{method} {path} HTTP/1.{minor}\r\n").as_bytes());
    let mut te: Vec<String> = Vec::new();
    let mut cl: Vec<String> = Vec::new();
    let mut connection: Vec<String> = Vec::new();
    let mut has_upgrade = false;
    let mut host: Option<String> = None;
    for h in req.headers.iter() {
        let norm = normalized(h.name);
        let underscored = h.name.contains('_');
        match norm.as_str() {
            "transfer-encoding" | "content-length" | "connection" if underscored => {
                // A framing header spelled with `_`: some stacks fold it,
                // some don't — refuse rather than guess.
                return Err("400 Bad Request");
            }
            "transfer-encoding" => te.extend(comma_tokens(h.value)),
            "content-length" => cl.push(String::from_utf8_lossy(h.value).trim().to_string()),
            "connection" => connection.extend(comma_tokens(h.value)),
            n if rule.drops(n) => {}
            n => {
                if n == "upgrade" {
                    has_upgrade = true;
                }
                if n == "host" {
                    host = Some(String::from_utf8_lossy(h.value).trim().to_string());
                }
                out.extend_from_slice(h.name.as_bytes());
                out.extend_from_slice(b": ");
                out.extend_from_slice(h.value);
                out.extend_from_slice(b"\r\n");
            }
        }
    }
    let body = if !te.is_empty() {
        let chunked = te.iter().filter(|t| t.eq_ignore_ascii_case("chunked")).count();
        let last_chunked = te.last().is_some_and(|t| t.eq_ignore_ascii_case("chunked"));
        if minor == 0 || !cl.is_empty() || chunked != 1 || !last_chunked {
            return Err("400 Bad Request");
        }
        out.extend_from_slice(format!("Transfer-Encoding: {}\r\n", te.join(", ")).as_bytes());
        Body::Chunked
    } else if !cl.is_empty() {
        let first = cl[0].clone();
        if first.is_empty() || first.len() > 18 || !first.bytes().all(|b| b.is_ascii_digit()) || cl.iter().any(|v| *v != first) {
            return Err("400 Bad Request");
        }
        let n: u64 = first.parse().map_err(|_| "400 Bad Request")?;
        out.extend_from_slice(format!("Content-Length: {n}\r\n").as_bytes());
        if n == 0 {
            Body::None
        } else {
            Body::Length(n)
        }
    } else {
        Body::None
    };
    // A client can't name K2's headers as hop-by-hop either.
    connection.retain(|t| !rule.drops(&normalized(t)));
    let upgrade = has_upgrade && connection.iter().any(|t| t.eq_ignore_ascii_case("upgrade"));
    if !connection.is_empty() {
        out.extend_from_slice(format!("Connection: {}\r\n", connection.join(", ")).as_bytes());
    }
    if let Rule::Set(Some(ip)) = rule {
        out.extend_from_slice(format!("{HEADER}: {ip}\r\nX-Forwarded-For: {ip}\r\nX-Real-IP: {ip}\r\n").as_bytes());
    }
    out.extend_from_slice(b"\r\n");
    Ok(RequestHead {
        bytes: out,
        body,
        is_head: method.eq_ignore_ascii_case("HEAD"),
        upgrade,
        method: method.to_string(),
        path: path.to_string(),
        host,
    })
}

async fn read_request<R: AsyncRead + Unpin>(rd: &mut Reader<R>, rule: Rule) -> io::Result<ReadRequest> {
    loop {
        if !rd.data().is_empty() {
            let mut headers = [httparse::EMPTY_HEADER; MAX_REQ_HEADERS];
            let mut req = httparse::Request::new(&mut headers);
            match req.parse(rd.data()) {
                Ok(httparse::Status::Complete(n)) if n > MAX_HEAD => {
                    rd.consume(n);
                    return Ok(ReadRequest::Refuse("431 Request Header Fields Too Large"));
                }
                Ok(httparse::Status::Complete(n)) => {
                    let out = rewrite_request(&req, rule);
                    rd.consume(n);
                    return Ok(match out {
                        Ok(h) => ReadRequest::Head(h),
                        Err(status) => ReadRequest::Refuse(status),
                    });
                }
                Ok(httparse::Status::Partial) => {
                    if rd.data().len() > MAX_HEAD {
                        return Ok(ReadRequest::Refuse("431 Request Header Fields Too Large"));
                    }
                }
                Err(httparse::Error::TooManyHeaders) => {
                    return Ok(ReadRequest::Refuse("431 Request Header Fields Too Large"));
                }
                Err(_) => return Ok(ReadRequest::Refuse("400 Bad Request")),
            }
        }
        if rd.fill().await? == 0 {
            // EOF between requests (or mid-head: nothing was forwarded).
            return Ok(ReadRequest::Eof);
        }
    }
}

/// What the response side expects next, in request order.
#[derive(Debug, Clone)]
enum Expect {
    Response { head: bool, upgrade: bool },
    /// K2's own answer (a refusal, or a [`LocalAnswer`]): written after the
    /// responses before it, then the connection closes.
    Own(Vec<u8>),
}

struct Shared {
    queue: Mutex<VecDeque<Expect>>,
    pushed: Notify,
}

impl Shared {
    fn push(&self, e: Expect) {
        self.queue.lock().unwrap_or_else(|p| p.into_inner()).push_back(e);
        self.pushed.notify_one();
    }

    fn front(&self) -> Option<Expect> {
        self.queue.lock().unwrap_or_else(|p| p.into_inner()).front().cloned()
    }

    fn pop(&self) -> Option<Expect> {
        self.queue.lock().unwrap_or_else(|p| p.into_inner()).pop_front()
    }
}

/// Upgrade decisions: `(upgrade requests decided so far, last one switched)`.
type Decision = (u64, bool);

/// Client → upstream: every request rewritten; one K2 answers itself
/// ([`LocalAnswer`]) ends the request side.
async fn requests<R, W>(
    mut rd: Reader<R>,
    mut up: W,
    mode: Mode,
    shared: &Shared,
    mut decided: watch::Receiver<Decision>,
    local: Option<&LocalAnswer>,
) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut rule: Option<Rule> = None;
    let mut upgrades = 0u64;
    loop {
        // Resolve once, when the first request is here (a hop is
        // registered before any byte is written to it).
        if rule.is_none() && rd.data().is_empty() && rd.fill().await? == 0 {
            let _ = up.shutdown().await;
            return Ok(());
        }
        let r = *rule.get_or_insert_with(|| resolve(mode));
        let head = match read_request(&mut rd, r).await? {
            ReadRequest::Eof => {
                let _ = up.shutdown().await;
                return Ok(());
            }
            ReadRequest::Refuse(status) => {
                shared.push(Expect::Own(refusal(status)));
                return Ok(());
            }
            ReadRequest::Head(h) => h,
        };
        // A body-less GET/HEAD K2 answers itself never reaches the App (a
        // request with a body passes on unchanged).
        if let Some(answer) = local {
            if head.body == Body::None && !head.upgrade {
                if let Some(bytes) = answer(&head.method, &head.path, head.host.as_deref()) {
                    shared.push(Expect::Own(bytes));
                    return Ok(());
                }
            }
        }
        shared.push(Expect::Response { head: head.is_head, upgrade: head.upgrade });
        up.write_all(&head.bytes).await?;
        match head.body {
            Body::None => {}
            Body::Length(n) => rd.copy_exact(n, &mut up).await?,
            Body::Chunked => copy_chunked(&mut rd, &mut up, Some(r)).await?,
        }
        up.flush().await?;
        if head.upgrade {
            upgrades += 1;
            let switched = match decided.wait_for(|d| d.0 >= upgrades).await {
                Ok(d) => d.1,
                // The response side is gone: the connection is ending.
                Err(_) => return Ok(()),
            };
            if switched {
                rd.copy_rest(&mut up).await?;
                let _ = up.shutdown().await;
                return Ok(());
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RespBody {
    None,
    Length(u64),
    Chunked,
    UntilClose,
}

struct ResponseHead {
    len: usize,
    status: u16,
    body: RespBody,
}

/// Parse a response head at the start of `buf`. `Ok(None)`: need more.
fn parse_response(buf: &[u8], to_head: bool) -> io::Result<Option<ResponseHead>> {
    let mut headers = [httparse::EMPTY_HEADER; MAX_RESP_HEADERS];
    let mut resp = httparse::Response::new(&mut headers);
    let len = match resp.parse(buf) {
        Ok(httparse::Status::Complete(n)) => n,
        Ok(httparse::Status::Partial) if buf.len() <= MAX_HEAD => return Ok(None),
        Ok(httparse::Status::Partial) => return Err(bad("response head too large")),
        Err(e) => return Err(bad(&format!("response head: {e}"))),
    };
    let status = resp.code.ok_or_else(|| bad("response without a status"))?;
    let mut te: Vec<String> = Vec::new();
    let mut cl: Option<String> = None;
    for h in resp.headers.iter() {
        if h.name.eq_ignore_ascii_case("transfer-encoding") {
            te.extend(comma_tokens(h.value));
        } else if h.name.eq_ignore_ascii_case("content-length") {
            let v = String::from_utf8_lossy(h.value).trim().to_string();
            if cl.as_ref().is_some_and(|c| *c != v) {
                return Err(bad("conflicting Content-Length"));
            }
            cl = Some(v);
        }
    }
    let body = if to_head || (100..200).contains(&status) || status == 204 || status == 304 {
        RespBody::None
    } else if !te.is_empty() {
        if te.last().is_some_and(|t| t.eq_ignore_ascii_case("chunked")) {
            RespBody::Chunked
        } else {
            RespBody::UntilClose
        }
    } else if let Some(v) = cl {
        RespBody::Length(v.parse().map_err(|_| bad("bad Content-Length"))?)
    } else {
        RespBody::UntilClose
    };
    Ok(Some(ResponseHead { len, status, body }))
}

/// Upstream → client: responses forwarded as they are, framed so a `101`
/// to an upgrade request is seen.
async fn responses<R, W>(mut rd: Reader<R>, mut cw: W, shared: &Shared, decide: watch::Sender<Decision>) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut decided = 0u64;
    loop {
        let expect = match shared.front() {
            Some(Expect::Own(bytes)) => {
                cw.write_all(&bytes).await?;
                let _ = cw.shutdown().await;
                return Ok(());
            }
            Some(e) => Some(e),
            None if rd.data().is_empty() => {
                tokio::select! {
                    _ = shared.pushed.notified() => continue,
                    n = rd.fill() => {
                        if n? == 0 {
                            let _ = cw.shutdown().await;
                            return Ok(());
                        }
                        continue;
                    }
                }
            }
            // Bytes nobody asked for (e.g. a 408 before closing).
            None => None,
        };
        let (to_head, upgrade) = match expect {
            Some(Expect::Response { head, upgrade }) => (head, upgrade),
            _ => (false, false),
        };
        let head = loop {
            if !rd.data().is_empty() {
                match parse_response(rd.data(), to_head) {
                    Ok(Some(h)) => break h,
                    Ok(None) => {}
                    Err(_) => {
                        // Can't frame it: pass the rest through as is. The
                        // request side keeps rewriting what the client sends.
                        rd.copy_rest(&mut cw).await?;
                        let _ = cw.shutdown().await;
                        return Ok(());
                    }
                }
            }
            if rd.fill().await? == 0 {
                if !rd.data().is_empty() {
                    cw.write_all(rd.data()).await?;
                }
                let _ = cw.shutdown().await;
                return Ok(());
            }
        };
        cw.write_all(&rd.data()[..head.len]).await?;
        rd.consume(head.len);
        if head.status == 101 && upgrade {
            shared.pop();
            decided += 1;
            let _ = decide.send((decided, true));
            cw.flush().await?;
            rd.copy_rest(&mut cw).await?;
            let _ = cw.shutdown().await;
            return Ok(());
        }
        if (100..200).contains(&head.status) {
            // Interim (100 Continue, 103 Early Hints; a 101 nobody asked
            // to upgrade to is passed on and the real response awaited).
            cw.flush().await?;
            continue;
        }
        if expect.is_some() {
            shared.pop();
        }
        match head.body {
            RespBody::None => {}
            RespBody::Length(n) => rd.copy_exact(n, &mut cw).await?,
            RespBody::Chunked => copy_chunked(&mut rd, &mut cw, None).await?,
            RespBody::UntilClose => {
                rd.copy_rest(&mut cw).await?;
                let _ = cw.shutdown().await;
                return Ok(());
            }
        }
        cw.flush().await?;
        if upgrade {
            decided += 1;
            let _ = decide.send((decided, false));
        }
    }
}

/// Splice `client` ↔ `upstream`, rewriting every HTTP/1.1 request the
/// client sends (see the module docs). Ends when the upstream side is done,
/// or at once on a request framing error.
pub async fn forward<C, U>(client: C, upstream: U, mode: Mode) -> io::Result<()>
where
    C: AsyncRead + AsyncWrite + Unpin,
    U: AsyncRead + AsyncWrite + Unpin,
{
    forward_with(client, upstream, mode, None).await
}

/// [`forward`] with K2's own answers for some requests ([`LocalAnswer`]).
pub async fn forward_with<C, U>(client: C, upstream: U, mode: Mode, local: Option<LocalAnswer>) -> io::Result<()>
where
    C: AsyncRead + AsyncWrite + Unpin,
    U: AsyncRead + AsyncWrite + Unpin,
{
    let (cr, cw) = tokio::io::split(client);
    let (ur, uw) = tokio::io::split(upstream);
    let shared = Shared { queue: Mutex::new(VecDeque::new()), pushed: Notify::new() };
    let (decide, decided) = watch::channel::<Decision>((0, false));
    let req = requests(Reader::new(cr), uw, mode, &shared, decided, local.as_ref());
    let resp = responses(Reader::new(ur), cw, &shared, decide);
    tokio::pin!(req, resp);
    tokio::select! {
        r = &mut resp => r,
        r = &mut req => match r {
            Ok(()) => resp.await,
            Err(e) => Err(e),
        },
    }
}

// ── the first request, before the App is dialed ──────────────────────

/// How long [`peek_head`] waits for a first request head.
pub const PEEK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// A connection's first request, read before the App is dialed.
#[derive(Debug)]
pub struct Peeked {
    /// Every byte read so far (the head and whatever followed it): replay
    /// them with [`Replay`] when the App is to answer.
    pub bytes: Vec<u8>,
    /// `(method, path and query, Host)` when the first head parsed and
    /// carries no body; `None` otherwise (the App answers).
    pub plain: Option<(String, String, Option<String>)>,
}

/// Read `client`'s first request head (at most [`MAX_HEAD`] bytes, by
/// [`PEEK_TIMEOUT`]). Never fails: whatever was read is in `bytes`, and
/// anything that isn't a body-less HTTP/1.x request head is `plain: None`.
pub async fn peek_head<S: AsyncRead + Unpin>(client: &mut S) -> Peeked {
    let mut bytes: Vec<u8> = Vec::new();
    let read = async {
        let mut tmp = [0u8; 4096];
        loop {
            {
                let mut headers = [httparse::EMPTY_HEADER; MAX_REQ_HEADERS];
                let mut req = httparse::Request::new(&mut headers);
                match req.parse(&bytes) {
                    Ok(httparse::Status::Complete(_)) => return,
                    Ok(httparse::Status::Partial) if bytes.len() <= MAX_HEAD => {}
                    _ => return,
                }
            }
            match client.read(&mut tmp).await {
                Ok(0) | Err(_) => return,
                Ok(n) => bytes.extend_from_slice(&tmp[..n]),
            }
        }
    };
    let _ = tokio::time::timeout(PEEK_TIMEOUT, read).await;
    let plain = {
        let mut headers = [httparse::EMPTY_HEADER; MAX_REQ_HEADERS];
        let mut req = httparse::Request::new(&mut headers);
        match req.parse(&bytes) {
            Ok(httparse::Status::Complete(_)) => {
                let mut host = None;
                let mut body = false;
                for h in req.headers.iter() {
                    match normalized(h.name).as_str() {
                        "host" => host = Some(String::from_utf8_lossy(h.value).trim().to_string()),
                        "transfer-encoding" => body = true,
                        "content-length" => body |= String::from_utf8_lossy(h.value).trim() != "0",
                        _ => {}
                    }
                }
                match (req.method, req.path, body) {
                    (Some(m), Some(p), false) => Some((m.to_string(), p.to_string(), host)),
                    _ => None,
                }
            }
            _ => None,
        }
    };
    Peeked { bytes, plain }
}

/// A stream that first yields bytes already read from it ([`peek_head`]).
pub struct Replay<'a, S> {
    pre: Vec<u8>,
    pos: usize,
    inner: &'a mut S,
}

impl<'a, S> Replay<'a, S> {
    pub fn new(pre: Vec<u8>, inner: &'a mut S) -> Self {
        Self { pre, pos: 0, inner }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Replay<'_, S> {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        let me = self.get_mut();
        if me.pos < me.pre.len() {
            let n = (me.pre.len() - me.pos).min(buf.remaining());
            buf.put_slice(&me.pre[me.pos..me.pos + n]);
            me.pos += n;
            return std::task::Poll::Ready(Ok(()));
        }
        std::pin::Pin::new(&mut *me.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Replay<'_, S> {
    fn poll_write(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>, buf: &[u8]) -> std::task::Poll<io::Result<usize>> {
        std::pin::Pin::new(&mut *self.get_mut().inner).poll_write(cx, buf)
    }
    fn poll_flush(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut *self.get_mut().inner).poll_flush(cx)
    }
    fn poll_shutdown(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut *self.get_mut().inner).poll_shutdown(cx)
    }
}

/// How long a test waits for anything.
#[cfg(test)]
const TEST_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

#[cfg(test)]
#[path = "client_ip_tests.rs"]
pub(crate) mod tests;
