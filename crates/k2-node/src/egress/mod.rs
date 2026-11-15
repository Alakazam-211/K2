//! The no-lan egress proxy (prd-compute-mac-no-lan-v1 §5.2, §5.5).
//!
//! On a Mac, Seatbelt can say "loopback only" or "everything", never
//! "public only". A `no-lan` job there keeps the `none` profile (no remote
//! IP, no DNS) and reaches the internet through its own proxy on
//! 127.0.0.1, a k2-node process outside its sandbox (`k2-node job-egress`,
//! [`proc`]). The job's `HTTPS_PROXY` points at it.
//!
//! Per connection:
//! 1. limits: at most [`MAX_TUNNELS`] at once; a [`HEAD_MAX`] request head
//!    within [`HEAD_TIMEOUT`]; a tunnel idle for [`IDLE`] closes;
//! 2. `CONNECT host:port` only (a name, an IPv4 literal or `[IPv6]`). An
//!    absolute-form `http://` request gets 403 `plain_http` (v1); anything
//!    else 400;
//! 3. local names (`localhost`, single-label names, `.local`, `.lan`,
//!    `.home.arpa`, `.internal`, `.localdomain`) are refused before any
//!    lookup;
//! 4. the name is resolved **once**, by the proxy (the job has no DNS);
//! 5. every address is vetted ([`deny_reason`]): not public
//!    ([`crate::sync::is_public_ip`]), in Linux's `NO_LAN_DENY`, or one of
//!    this node's own on-link prefixes and addresses (re-read every
//!    [`LAN_REFRESH`]). One bad address refuses the name;
//! 6. only the vetted addresses are dialed (no second lookup: a name that
//!    answers public, then private, can't rebind past the check);
//! 7. any port; the bytes are spliced both ways (TLS stays end to end).
//!
//! Every decision is one JSON line in the job's `egress.jsonl`
//! ([`Decision`]); the node turns them into the job log's live refusal
//! lines and the end-of-job host list ([`fold`]).

pub mod fold;

use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

/// The proxy port pool: one port per running no-lan job, `POOL_FIRST + i`,
/// `i < POOL_LEN` (just below the job port range, 20000+). Fixed ports are
/// what lets every other job's Seatbelt profile deny them.
pub const POOL_FIRST: u16 = 19880;
pub const POOL_LEN: u16 = k2_node_proto::env::JOB_PORT_BLOCKS;
/// Tunnels open at once per job (npm opens many sockets).
pub const MAX_TUNNELS: usize = 128;
/// Largest request head.
pub const HEAD_MAX: usize = 16 * 1024;
pub const HEAD_TIMEOUT: Duration = Duration::from_secs(10);
/// A tunnel with no bytes either way for this long is closed.
pub const IDLE: Duration = Duration::from_secs(300);
/// One lookup's limit.
pub const RESOLVE_TIMEOUT: Duration = Duration::from_secs(10);
/// One address's connect limit.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How often the node's own prefixes are read again (a Wi-Fi change).
pub const LAN_REFRESH: Duration = Duration::from_secs(30);
/// `egress.jsonl` stops growing here; later lines are only counted.
pub const LOG_CAP: u64 = 4 * 1024 * 1024;

/// Every pool port.
pub fn pool_ports() -> impl Iterator<Item = u16> {
    (0..POOL_LEN).map(|i| POOL_FIRST + i)
}

/// Stable refusal codes (the `X-K2-Refused` header, the decision log).
pub mod codes {
    pub const PLAIN_HTTP: &str = "plain_http";
    pub const PRIVATE: &str = "private_address";
    pub const LOCAL_NAME: &str = "local_name";
    pub const BAD_REQUEST: &str = "bad_request";
    pub const TOO_MANY: &str = "too_many_tunnels";
    pub const DNS_FAILED: &str = "dns_failed";
    pub const CONNECT_FAILED: &str = "connect_failed";
    pub const CONNECT_TIMEOUT: &str = "connect_timeout";
}

// ── the request ─────────────────────────────────────────────────────

/// What a request asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// Lowercase, no trailing dot; an IPv6 literal without brackets.
    pub host: String,
    pub port: u16,
    /// The host is an address literal.
    pub ip: Option<IpAddr>,
}

impl Target {
    /// `host:port` (`[v6]:port`).
    pub fn display(&self) -> String {
        hostport(&self.host, self.port)
    }
}

/// `host:port`, an IPv6 host in brackets.
pub fn hostport(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Why a request is refused (or failed upstream): the answer's status,
/// a stable code, the address it was about (if any) and a reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub status: u16,
    pub code: &'static str,
    pub addr: Option<IpAddr>,
    pub why: String,
}

fn refusal(status: u16, code: &'static str, why: impl Into<String>) -> Refusal {
    Refusal { status, code, addr: None, why: why.into() }
}

/// A request that wasn't a CONNECT: what to log for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejected {
    pub refusal: Refusal,
    /// The URL's host and port for a plain `http://` request.
    pub target: Option<Target>,
}

fn bad(why: &str) -> Rejected {
    Rejected { refusal: refusal(400, codes::BAD_REQUEST, why), target: None }
}

/// A host as a request names it: an address literal, or a DNS name of
/// letters, digits, `-`, `_` and `.` (at most 253 bytes, no empty label).
fn parse_host(h: &str, port: u16) -> Option<Target> {
    if let Some(v6) = h.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
        let ip: std::net::Ipv6Addr = v6.parse().ok()?;
        return Some(Target { host: ip.to_string(), port, ip: Some(IpAddr::V6(ip)) });
    }
    if let Ok(ip) = h.parse::<std::net::Ipv4Addr>() {
        return Some(Target { host: ip.to_string(), port, ip: Some(IpAddr::V4(ip)) });
    }
    let name = h.strip_suffix('.').unwrap_or(h).to_ascii_lowercase();
    if name.is_empty() || name.len() > 253 || name.split('.').any(|l| l.is_empty() || l.len() > 63) {
        return None;
    }
    if !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.') {
        return None;
    }
    Some(Target { host: name, port, ip: None })
}

/// Parse a request head (without its final blank line).
pub fn parse_head(head: &str) -> Result<Target, Rejected> {
    let first = head.split("\r\n").next().unwrap_or("");
    let mut parts = first.split(' ');
    let (method, target, version) = match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(m), Some(t), Some(v), None) => (m, t, v),
        _ => return Err(bad("malformed request line")),
    };
    if !matches!(version, "HTTP/1.1" | "HTTP/1.0") || method.is_empty() || !method.bytes().all(|b| b.is_ascii_alphabetic()) {
        return Err(bad("malformed request line"));
    }
    if method.eq_ignore_ascii_case("CONNECT") {
        let (h, p) = if target.starts_with('[') {
            let end = target.find(']').ok_or_else(|| bad("CONNECT needs host:port"))?;
            (&target[..=end], target[end + 1..].strip_prefix(':').ok_or_else(|| bad("CONNECT needs host:port"))?)
        } else {
            target.rsplit_once(':').ok_or_else(|| bad("CONNECT needs host:port"))?
        };
        let port = p.parse::<u16>().ok().filter(|p| *p > 0).ok_or_else(|| bad("CONNECT needs a port 1-65535"))?;
        return parse_host(h, port).ok_or_else(|| bad("CONNECT to something that isn't a host name"));
    }
    let lower = target.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("http://") {
        let auth = rest.split(['/', '?', '#']).next().unwrap_or("");
        let auth = auth.rsplit('@').next().unwrap_or("");
        let (h, port) = if auth.starts_with('[') {
            match auth.find(']') {
                Some(e) => (&auth[..=e], auth[e + 1..].strip_prefix(':').and_then(|p| p.parse().ok()).unwrap_or(80)),
                None => (auth, 80),
            }
        } else {
            match auth.rsplit_once(':') {
                Some((h, p)) => (h, p.parse().unwrap_or(80)),
                None => (auth, 80),
            }
        };
        return Err(Rejected {
            refusal: refusal(403, codes::PLAIN_HTTP, "plain HTTP isn't carried; use https"),
            target: parse_host(h, port),
        });
    }
    Err(bad("this is a no-lan job's egress proxy: it takes CONNECT host:port only (set HTTPS_PROXY)"))
}

/// A name the proxy never looks up: it can only mean this machine or the
/// local network.
pub fn local_name(host: &str) -> bool {
    let h = host.strip_suffix('.').unwrap_or(host).to_ascii_lowercase();
    if h == "localhost" || h.ends_with(".localhost") || !h.contains('.') {
        return true;
    }
    [".local", ".lan", ".home.arpa", ".internal", ".localdomain"].iter().any(|s| h.ends_with(s))
}

// ── the address check ───────────────────────────────────────────────

/// An address prefix (`10.0.0.0/8`, `fe80::/10`, a single `/32`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prefix {
    pub addr: IpAddr,
    pub len: u8,
}

impl Prefix {
    pub fn parse(s: &str) -> Option<Self> {
        let (a, l) = s.split_once('/')?;
        let addr: IpAddr = a.parse().ok()?;
        let len: u8 = l.parse().ok()?;
        (len <= if addr.is_ipv4() { 32 } else { 128 }).then_some(Self { addr, len })
    }

    pub fn contains(&self, ip: &IpAddr) -> bool {
        match (self.addr, plain(*ip)) {
            (IpAddr::V4(p), IpAddr::V4(a)) => {
                let m = if self.len == 0 { 0 } else { u32::MAX << (32 - self.len as u32) };
                u32::from(p) & m == u32::from(a) & m
            }
            (IpAddr::V6(p), IpAddr::V6(a)) => {
                let m = if self.len == 0 { 0 } else { u128::MAX << (128 - self.len as u32) };
                u128::from(p) & m == u128::from(a) & m
            }
            _ => false,
        }
    }
}

/// An IPv4-mapped IPv6 address as the IPv4 address it is.
fn plain(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(a) => a.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        v4 => v4,
    }
}

/// Parse prefix strings (bad ones are dropped).
pub fn prefixes(list: &[String]) -> Vec<Prefix> {
    list.iter().filter_map(|s| Prefix::parse(s)).collect()
}

/// Why `ip` is never dialed for a no-lan job, or `None` when it may be:
/// not a public address, in Linux no-lan's deny list (parity), or one of
/// this node's own prefixes or addresses (`own`).
pub fn deny_reason(ip: &IpAddr, own: &[Prefix]) -> Option<&'static str> {
    let p = plain(*ip);
    if p.is_loopback() {
        return Some("loopback address");
    }
    if !crate::sync::is_public_ip(&p) || crate::admin::jobs::NO_LAN_DENY.iter().filter_map(|s| Prefix::parse(s)).any(|x| x.contains(&p)) {
        return Some("private address");
    }
    if own.iter().any(|x| x.contains(&p)) {
        return Some("address on this node's own network");
    }
    None
}

// ── the policy, with its seams ──────────────────────────────────────

/// Resolve a name: every address it has (no port).
pub type Resolve = Arc<dyn Fn(&str) -> Result<Vec<IpAddr>, String> + Send + Sync>;

/// The system resolver (`getaddrinfo`), as the proxy uses it.
pub fn system_resolve() -> Resolve {
    Arc::new(|host: &str| {
        use std::net::ToSocketAddrs;
        let mut out: Vec<IpAddr> = Vec::new();
        for a in (host, 0u16).to_socket_addrs().map_err(|e| e.to_string())? {
            if !out.contains(&a.ip()) {
                out.push(a.ip());
            }
        }
        Ok(out)
    })
}

/// Where this node's own prefixes come from.
pub type OwnLan = Arc<dyn Fn() -> Vec<String> + Send + Sync>;

/// The node's own on-link prefixes and addresses (getifaddrs).
pub fn system_own_lan() -> OwnLan {
    Arc::new(crate::admin::jobs::lan_deny_extra)
}

/// One proxy's policy.
pub struct Policy {
    pub resolve: Resolve,
    own_lan: OwnLan,
    own: RwLock<(Vec<Prefix>, Instant)>,
    /// Tests only: addresses admitted as if public (a local upstream).
    test_public: Vec<SocketAddr>,
    pub idle: Duration,
    pub head_timeout: Duration,
}

impl Policy {
    pub fn new(resolve: Resolve, own_lan: OwnLan) -> Self {
        let own = prefixes(&own_lan());
        Self { resolve, own_lan, own: RwLock::new((own, Instant::now())), test_public: Vec::new(), idle: IDLE, head_timeout: HEAD_TIMEOUT }
    }

    /// Tests only: admit these addresses as public.
    pub fn with_test_public(mut self, a: Vec<SocketAddr>) -> Self {
        self.test_public = a;
        self
    }

    /// Tests: shorter idle and head limits.
    pub fn with_timeouts(mut self, idle: Duration, head: Duration) -> Self {
        self.idle = idle;
        self.head_timeout = head;
        self
    }

    /// The node's own prefixes, read again after [`LAN_REFRESH`].
    fn own(&self) -> Vec<Prefix> {
        {
            let g = self.own.read().unwrap_or_else(|e| e.into_inner());
            if g.1.elapsed() < LAN_REFRESH {
                return g.0.clone();
            }
        }
        let fresh = prefixes(&(self.own_lan)());
        *self.own.write().unwrap_or_else(|e| e.into_inner()) = (fresh.clone(), Instant::now());
        fresh
    }

    fn vet(&self, ip: IpAddr, port: u16, own: &[Prefix]) -> Result<(), &'static str> {
        if self.test_public.contains(&SocketAddr::new(ip, port)) {
            return Ok(());
        }
        deny_reason(&ip, own).map_or(Ok(()), Err)
    }

    /// The vetted addresses to dial for `t`, in the resolver's order, or
    /// why not. One lookup at most.
    pub async fn decide(&self, t: &Target) -> Result<Vec<SocketAddr>, Refusal> {
        let own = self.own();
        let private = |ip: IpAddr, what: &str| Refusal {
            status: 403,
            code: codes::PRIVATE,
            addr: Some(ip),
            why: what.to_string(),
        };
        if let Some(ip) = t.ip {
            return match self.vet(ip, t.port, &own) {
                Ok(()) => Ok(vec![SocketAddr::new(ip, t.port)]),
                Err(what) => Err(private(ip, what)),
            };
        }
        if local_name(&t.host) {
            return Err(refusal(403, codes::LOCAL_NAME, "local name (not looked up)"));
        }
        let (r, host) = (self.resolve.clone(), t.host.clone());
        let looked = tokio::time::timeout(RESOLVE_TIMEOUT, tokio::task::spawn_blocking(move || r(&host))).await;
        let addrs = match looked {
            Ok(Ok(Ok(a))) if !a.is_empty() => a,
            Ok(Ok(Ok(_))) => return Err(refusal(502, codes::DNS_FAILED, "DNS lookup found no address (does this node have internet?)")),
            Ok(Ok(Err(e))) => return Err(refusal(502, codes::DNS_FAILED, format!("DNS lookup failed (does this node have internet?): {e}"))),
            Ok(Err(_)) => return Err(refusal(502, codes::DNS_FAILED, "DNS lookup failed")),
            Err(_) => return Err(refusal(504, codes::DNS_FAILED, format!("DNS lookup took over {} s (does this node have internet?)", RESOLVE_TIMEOUT.as_secs()))),
        };
        for ip in &addrs {
            if let Err(what) = self.vet(*ip, t.port, &own) {
                return Err(private(*ip, what));
            }
        }
        Ok(addrs.into_iter().map(|ip| SocketAddr::new(ip, t.port)).collect())
    }
}

// ── the decision log ────────────────────────────────────────────────

/// One line of `egress.jsonl`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    /// Unix seconds.
    pub t: i64,
    /// `open` (a tunnel started), `close` (it ended: its bytes),
    /// `refused` (the policy), `failed` (upstream: DNS, connect), or
    /// `dropped` (lines the cap left out).
    pub ev: String,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addr: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// Bytes from the host to the job (`close`).
    #[serde(default, rename = "in", skip_serializing_if = "is_zero")]
    pub bytes_in: u64,
    /// Bytes from the job to the host (`close`).
    #[serde(default, rename = "out", skip_serializing_if = "is_zero")]
    pub bytes_out: u64,
    /// `dropped`: how many.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub n: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

impl Decision {
    pub fn new(ev: &str, host: &str, port: u16) -> Self {
        Self { t: crate::util::now(), ev: ev.into(), host: host.into(), port, addr: None, code: None, why: None, bytes_in: 0, bytes_out: 0, n: 0 }
    }
}

/// Where decisions go: `egress.jsonl`, append only, capped at `cap`
/// bytes; past it lines are counted, and [`Log::finish`] writes one
/// `dropped` line.
pub struct Log {
    inner: Mutex<LogInner>,
    cap: u64,
}

struct LogInner {
    file: Option<std::fs::File>,
    written: u64,
    dropped: u64,
}

impl Log {
    /// Append to `path` (created 0600 when missing).
    pub fn open(path: &std::path::Path, cap: u64) -> Result<Self, String> {
        use std::os::unix::fs::OpenOptionsExt;
        let f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let written = f.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Self { inner: Mutex::new(LogInner { file: Some(f), written, dropped: 0 }), cap })
    }

    /// A log that keeps nothing (tests of the policy alone).
    pub fn none() -> Self {
        Self { inner: Mutex::new(LogInner { file: None, written: 0, dropped: 0 }), cap: 0 }
    }

    pub fn write(&self, d: &Decision) {
        use std::io::Write;
        let mut line = serde_json::to_vec(d).unwrap_or_default();
        line.push(b'\n');
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if g.file.is_none() {
            return;
        }
        if g.written + line.len() as u64 > self.cap {
            g.dropped += 1;
            return;
        }
        let ok = g.file.as_mut().is_some_and(|f| f.write_all(&line).is_ok());
        if ok {
            g.written += line.len() as u64;
        } else {
            g.dropped += 1;
        }
    }

    /// The `dropped` line, if any were (past the cap: it is the one line
    /// allowed over it).
    pub fn finish(&self) {
        use std::io::Write;
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if g.dropped == 0 {
            return;
        }
        let d = Decision { n: g.dropped, ..Decision::new("dropped", "", 0) };
        let mut line = serde_json::to_vec(&d).unwrap_or_default();
        line.push(b'\n');
        if let Some(f) = g.file.as_mut() {
            let _ = f.write_all(&line);
        }
        g.dropped = 0;
    }
}

// ── the server ──────────────────────────────────────────────────────

/// The body a refused job sees (one line).
pub fn refusal_body(t: Option<&Target>, r: &Refusal) -> String {
    let what = t.map(Target::display).unwrap_or_else(|| "this request".into());
    match (r.code, r.addr) {
        (codes::PRIVATE, Some(a)) if t.is_some_and(|t| t.ip.is_none()) => {
            format!("k2-node no-lan: {what} resolves to {a} ({}); this server's jobs on this node reach the internet only", r.why)
        }
        (codes::PRIVATE, _) => format!("k2-node no-lan: {what} is refused ({}); this server's jobs on this node reach the internet only", r.why),
        (codes::PLAIN_HTTP, _) => format!("k2-node no-lan: {} isn't carried (plain HTTP); use https", t.map(|t| plain_url(&t.host, t.port)).unwrap_or_else(|| "http://".into())),
        _ => format!("k2-node no-lan: {what}: {}", r.why),
    }
}

/// `http://host/` (`http://host:port/` off port 80).
pub fn plain_url(host: &str, port: u16) -> String {
    let h = if host.contains(':') { format!("[{host}]") } else { host.to_string() };
    if port == 80 {
        format!("http://{h}/")
    } else {
        format!("http://{h}:{port}/")
    }
}

fn status_text(s: u16) -> &'static str {
    match s {
        400 => "400 Bad Request",
        403 => "403 Forbidden",
        429 => "429 Too Many Requests",
        502 => "502 Bad Gateway",
        504 => "504 Gateway Timeout",
        _ => "500 Internal Server Error",
    }
}

/// The whole refusal answer.
pub fn refusal_bytes(t: Option<&Target>, r: &Refusal) -> Vec<u8> {
    let body = format!("{}\n", refusal_body(t, r));
    format!(
        "HTTP/1.1 {}\r\nX-K2-Refused: {}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        status_text(r.status),
        r.code,
        body.len()
    )
    .into_bytes()
}

/// Read up to the end of the request head: (head, bytes after it).
async fn read_head(c: &mut TcpStream, limit: Duration) -> Result<(String, Vec<u8>), Rejected> {
    let mut buf = Vec::with_capacity(512);
    let mut chunk = [0u8; 2048];
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8(buf[..i].to_vec()).map_err(|_| bad("the request head isn't text"))?;
            return Ok((head, buf[i + 4..].to_vec()));
        }
        if buf.len() > HEAD_MAX {
            return Err(bad(&format!("request heads are at most {} KiB", HEAD_MAX / 1024)));
        }
        match tokio::time::timeout_at(deadline, c.read(&mut chunk)).await {
            Ok(Ok(0)) => return Err(bad("closed before the request head ended")),
            Ok(Ok(n)) => buf.extend_from_slice(&chunk[..n]),
            Ok(Err(_)) => return Err(bad("read error")),
            Err(_) => return Err(bad(&format!("no request head within {} s", limit.as_secs()))),
        }
    }
}

/// Dial the vetted addresses in order.
async fn connect_any(addrs: &[SocketAddr]) -> Result<TcpStream, Refusal> {
    let mut last = refusal(502, codes::CONNECT_FAILED, "no address");
    for a in addrs {
        match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(a)).await {
            Ok(Ok(s)) => {
                let _ = s.set_nodelay(true);
                return Ok(s);
            }
            Ok(Err(e)) => last = Refusal { status: 502, code: codes::CONNECT_FAILED, addr: Some(a.ip()), why: format!("couldn't connect ({})", e.kind()) },
            Err(_) => last = Refusal { status: 504, code: codes::CONNECT_TIMEOUT, addr: Some(a.ip()), why: format!("no answer within {} s", CONNECT_TIMEOUT.as_secs()) },
        }
    }
    Err(last)
}

/// A stream that counts the bytes read and stamps the last activity.
struct Tracked<S> {
    inner: S,
    read: Arc<AtomicU64>,
    last: Arc<AtomicU64>,
    epoch: Instant,
}

impl<S: AsyncRead + Unpin> AsyncRead for Tracked<S> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let r = Pin::new(&mut self.inner).poll_read(cx, buf);
        let n = buf.filled().len() - before;
        if n > 0 {
            self.read.fetch_add(n as u64, Ordering::Relaxed);
            self.last.store(self.epoch.elapsed().as_millis() as u64, Ordering::Relaxed);
        }
        r
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Tracked<S> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, b: &[u8]) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, b)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// Splice until both sides close, the tunnel idles past `idle`, or the
/// proxy stops. Returns (bytes job → host, bytes host → job).
async fn splice(job: TcpStream, up: TcpStream, idle: Duration, mut stop: watch::Receiver<bool>) -> (u64, u64) {
    let epoch = Instant::now();
    let last = Arc::new(AtomicU64::new(0));
    let (from_job, from_host) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)));
    let mut a = Tracked { inner: job, read: from_job.clone(), last: last.clone(), epoch };
    let mut b = Tracked { inner: up, read: from_host.clone(), last: last.clone(), epoch };
    let idle_watch = async {
        loop {
            tokio::time::sleep((idle / 4).max(Duration::from_millis(20))).await;
            let quiet = (epoch.elapsed().as_millis() as u64).saturating_sub(last.load(Ordering::Relaxed));
            if quiet >= idle.as_millis() as u64 {
                return;
            }
        }
    };
    tokio::select! {
        _ = tokio::io::copy_bidirectional(&mut a, &mut b) => {}
        _ = idle_watch => {}
        _ = stop.wait_for(|s| *s) => {}
    }
    (from_job.load(Ordering::Relaxed), from_host.load(Ordering::Relaxed))
}

struct Active(Arc<AtomicUsize>);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Serve `listener` until `stop` turns true; then wait (up to 5 s) for
/// the open tunnels to write their `close` lines.
pub async fn serve(listener: TcpListener, policy: Arc<Policy>, log: Arc<Log>, mut stop: watch::Receiver<bool>) {
    let active = Arc::new(AtomicUsize::new(0));
    loop {
        let accepted = tokio::select! {
            a = listener.accept() => a,
            _ = stop.wait_for(|s| *s) => break,
        };
        let Ok((c, _)) = accepted else {
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        };
        let prev = active.fetch_add(1, Ordering::SeqCst);
        let guard = Active(active.clone());
        let (policy, log, stop) = (policy.clone(), log.clone(), stop.clone());
        tokio::spawn(async move {
            let _g = guard;
            handle(c, prev >= MAX_TUNNELS, &policy, &log, stop).await;
        });
    }
    let t0 = Instant::now();
    while active.load(Ordering::SeqCst) > 0 && t0.elapsed() < Duration::from_secs(5) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn refused_line(t: Option<&Target>, r: &Refusal) -> Decision {
    let (host, port) = t.map(|t| (t.host.as_str(), t.port)).unwrap_or(("", 0));
    let ev = if r.status >= 500 { "failed" } else { "refused" };
    Decision { addr: r.addr.map(|a| a.to_string()), code: Some(r.code.to_string()), why: Some(r.why.clone()), ..Decision::new(ev, host, port) }
}

async fn refuse(c: &mut TcpStream, t: Option<&Target>, r: &Refusal, log: &Log) {
    log.write(&refused_line(t, r));
    let _ = c.write_all(&refusal_bytes(t, r)).await;
    let _ = c.shutdown().await;
    // Read what the client still sends (briefly): closing with unread bytes
    // would reset the connection and could lose the answer.
    let mut sink = [0u8; 4096];
    let until = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut left: usize = 64 * 1024;
    while left > 0 {
        match tokio::time::timeout_at(until, c.read(&mut sink)).await {
            Ok(Ok(n)) if n > 0 => left = left.saturating_sub(n),
            _ => break,
        }
    }
}

async fn handle(mut c: TcpStream, over: bool, policy: &Policy, log: &Log, stop: watch::Receiver<bool>) {
    if over {
        let r = refusal(429, codes::TOO_MANY, format!("at most {MAX_TUNNELS} tunnels at once per job"));
        return refuse(&mut c, None, &r, log).await;
    }
    let (head, rest) = match read_head(&mut c, policy.head_timeout).await {
        Ok(h) => h,
        Err(r) => return refuse(&mut c, r.target.as_ref(), &r.refusal, log).await,
    };
    let t = match parse_head(&head) {
        Ok(t) => t,
        Err(r) => return refuse(&mut c, r.target.as_ref(), &r.refusal, log).await,
    };
    let addrs = match policy.decide(&t).await {
        Ok(a) => a,
        Err(r) => return refuse(&mut c, Some(&t), &r, log).await,
    };
    let mut up = match connect_any(&addrs).await {
        Ok(s) => s,
        Err(r) => return refuse(&mut c, Some(&t), &r, log).await,
    };
    let addr = up.peer_addr().ok().map(|a| a.ip().to_string());
    log.write(&Decision { addr: addr.clone(), ..Decision::new("open", &t.host, t.port) });
    if c.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await.is_err() {
        log.write(&Decision { addr, ..Decision::new("close", &t.host, t.port) });
        return;
    }
    // Bytes the client sent right behind its CONNECT head.
    let lead = rest.len() as u64;
    if lead > 0 && up.write_all(&rest).await.is_err() {
        log.write(&Decision { addr, ..Decision::new("close", &t.host, t.port) });
        return;
    }
    let (out, inn) = splice(c, up, policy.idle, stop).await;
    log.write(&Decision { addr, bytes_in: inn, bytes_out: out + lead, ..Decision::new("close", &t.host, t.port) });
}

#[cfg(test)]
mod tests;
