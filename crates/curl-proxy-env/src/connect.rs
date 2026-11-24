//! Opening the tunnel: the TCP connection to the proxy, TLS to an
//! `https://` proxy, then the proxy's own handshake (HTTP CONNECT, SOCKS5
//! with RFC 1928 and RFC 1929, SOCKS4 and SOCKS4a).

use crate::error::Error;
use crate::stream::ProxyStream;
use crate::url::{Host, ProxyUrl, Scheme};
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

/// curl's connect timeout when none is given: 300 seconds.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(300);

/// The most of a CONNECT answer's head read (curl's limit is 100 KiB).
const MAX_HEAD: usize = 100 * 1024;

/// How to connect ([`connect`]).
#[derive(Debug, Clone, Default)]
pub struct Options {
    user_agent: Option<String>,
    deadline: Option<Instant>,
    #[cfg(feature = "rustls")]
    tls: Option<std::sync::Arc<rustls::ClientConfig>>,
}

impl Options {
    /// No `User-Agent`, curl's 300 second limit, no TLS config.
    pub fn new() -> Options {
        Options::default()
    }

    /// The `User-Agent` line of a CONNECT (curl sends `curl/<version>`).
    /// None by default: no such line.
    pub fn user_agent(mut self, ua: impl Into<String>) -> Options {
        self.user_agent = Some(ua.into());
        self
    }

    /// When the whole of [`connect`] must be done: the TCP connection,
    /// TLS to an `https://` proxy and the proxy's handshake. Past it:
    /// [`Error::TimedOut`].
    pub fn deadline(mut self, at: Instant) -> Options {
        self.deadline = Some(at);
        self
    }

    /// [`Options::deadline`], this long from now.
    pub fn timeout(self, d: Duration) -> Options {
        self.deadline(Instant::now() + d)
    }

    /// The TLS config for an `https://` proxy: its roots check the
    /// proxy's certificate, for the proxy's own name. ALPN is set to
    /// `http/1.1` for the proxy whatever the config says. Without one,
    /// an `https://` proxy is [`Error::TlsNotConfigured`].
    #[cfg(feature = "rustls")]
    pub fn proxy_tls(mut self, config: std::sync::Arc<rustls::ClientConfig>) -> Options {
        self.tls = Some(config);
        self
    }
}

/// A connection to `host:port` (a URL's host: a name, four numbers, or
/// an IPv6 address with or without brackets), through `proxy` when there
/// is one (from [`crate::proxy_for`]), as curl makes it:
///
/// - no proxy: TCP to the target;
/// - `http://`: TCP to the proxy, then `CONNECT host:port HTTP/1.1` with
///   `Host`, `Proxy-Authorization: Basic` (when the URL has a user part),
///   `User-Agent` (when [`Options::user_agent`] gives one) and
///   `Proxy-Connection: Keep-Alive`; any 2xx opens the tunnel;
/// - `https://`: the same, inside TLS to the proxy, whose certificate is
///   checked for the proxy's name ([`Options::proxy_tls`]);
/// - `socks5://`: the target's name resolved here and its first address
///   sent; `socks5h://`: the name sent for the proxy to resolve (an
///   address as an address). Methods offered: no authentication, and
///   user and password (RFC 1929) when the URL has a user part;
/// - `socks4://`: the target's first IPv4 address, resolved here;
///   `socks4a://`: the name, for the proxy to resolve. The URL's user is
///   the user id; a password is not used.
///
/// The proxy's credentials go to the proxy alone, in its handshake;
/// nothing of the caller's request is sent until the tunnel is open. A
/// proxy that fails is an error, never a direct connection.
pub fn connect(
    proxy: Option<&ProxyUrl>,
    host: &str,
    port: u16,
    opts: &Options,
) -> Result<ProxyStream, Error> {
    let deadline = opts
        .deadline
        .unwrap_or_else(|| Instant::now() + DEFAULT_CONNECT_TIMEOUT);
    let target = Host::parse(host);
    let Some(p) = proxy else {
        let addrs = addresses(&target, port).map_err(Error::ResolveHost)?;
        return tcp(&addrs, deadline).map(ProxyStream::Tcp);
    };
    #[cfg(feature = "rustls")]
    let tls = match (p.scheme(), &opts.tls) {
        (Scheme::Https, None) => return Err(Error::TlsNotConfigured),
        (_, cfg) => cfg.clone(),
    };
    #[cfg(not(feature = "rustls"))]
    if p.scheme() == Scheme::Https {
        return Err(Error::TlsNotConfigured);
    }
    // SOCKS4 and SOCKS5 take an address: the target's name is resolved
    // before the proxy is reached.
    let target = match p.scheme() {
        Scheme::Socks4 => Host::V4(first_v4(&target, port)?),
        Scheme::Socks5 => resolved(&target, port)?,
        // A name too long for SOCKS5's one length byte is resolved here.
        Scheme::Socks5h if matches!(&target, Host::Name(n) if n.len() > 255) => {
            resolved(&target, port)?
        }
        _ => target,
    };
    let addrs = addresses(p.host(), p.port()).map_err(Error::ResolveProxy)?;
    let s = tcp(&addrs, deadline)?;
    let ua = opts.user_agent.as_deref();
    match p.scheme() {
        Scheme::Http => {
            let mut s = ProxyStream::Tcp(s);
            http_connect(&mut s, p, &target, port, ua, deadline)?;
            Ok(s)
        }
        #[cfg(feature = "rustls")]
        Scheme::Https => {
            let cfg = tls.ok_or(Error::TlsNotConfigured)?;
            let mut s = tls_to_proxy(s, p.host(), &cfg, deadline)?;
            http_connect(&mut s, p, &target, port, ua, deadline)?;
            Ok(s)
        }
        #[cfg(not(feature = "rustls"))]
        Scheme::Https => Err(Error::TlsNotConfigured),
        Scheme::Socks5 | Scheme::Socks5h => {
            let mut s = ProxyStream::Tcp(s);
            socks5(&mut s, p, &target, port, deadline)?;
            Ok(s)
        }
        Scheme::Socks4 | Scheme::Socks4a => {
            let mut s = ProxyStream::Tcp(s);
            socks4(&mut s, p, &target, port, deadline)?;
            Ok(s)
        }
    }
}

/// The addresses of a host: an address as it is, a name looked up.
fn addresses(h: &Host, port: u16) -> io::Result<Vec<SocketAddr>> {
    let addrs: Vec<SocketAddr> = match h {
        Host::V4(a) => vec![SocketAddr::from((*a, port))],
        Host::V6(a) => vec![SocketAddr::from((*a, port))],
        Host::Name(n) => (n.as_str(), port).to_socket_addrs()?.collect(),
    };
    if addrs.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{h} has no address"),
        ));
    }
    Ok(addrs)
}

/// The target's first address, as a host (a name resolved here).
fn resolved(target: &Host, port: u16) -> Result<Host, Error> {
    let a = addresses(target, port).map_err(Error::ResolveHost)?[0];
    Ok(match a.ip() {
        IpAddr::V4(v4) => Host::V4(v4),
        IpAddr::V6(v6) => Host::V6(v6),
    })
}

/// The target's first IPv4 address (SOCKS4 has no other kind).
fn first_v4(target: &Host, port: u16) -> Result<Ipv4Addr, Error> {
    if let Host::V6(_) = target {
        return Err(Error::Socks("SOCKS4 can't reach an IPv6 address".into()));
    }
    addresses(target, port)
        .map_err(Error::ResolveHost)?
        .into_iter()
        .find_map(|a| match a.ip() {
            IpAddr::V4(v4) => Some(v4),
            IpAddr::V6(_) => None,
        })
        .ok_or_else(|| {
            Error::ResolveHost(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{target} has no IPv4 address"),
            ))
        })
}

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

/// TCP to the first address that accepts, before `deadline`.
fn tcp(addrs: &[SocketAddr], deadline: Instant) -> Result<TcpStream, Error> {
    let mut last = io::Error::new(io::ErrorKind::NotFound, "no address");
    for addr in addrs {
        let budget = remaining(deadline).ok_or(Error::TimedOut)?;
        match TcpStream::connect_timeout(addr, budget) {
            Ok(s) => return Ok(s),
            Err(e) if is_timeout(&e) => return Err(Error::TimedOut),
            Err(e) => last = e,
        }
    }
    Err(Error::Connect(last))
}

/// The time left, on the TCP connection's reads and writes.
fn arm(s: &TcpStream, deadline: Instant) -> Result<(), Error> {
    let d = remaining(deadline).ok_or(Error::TimedOut)?;
    s.set_read_timeout(Some(d))
        .and_then(|()| s.set_write_timeout(Some(d)))
        .map_err(Error::Connect)
}

/// Which handshake failed, for its error.
#[derive(Clone, Copy)]
enum Stage {
    Http,
    Socks,
}

impl Stage {
    fn fail(self, why: String) -> Error {
        match self {
            Stage::Http => Error::HttpProxy(why),
            Stage::Socks => Error::Socks(why),
        }
    }
}

fn send(s: &mut ProxyStream, bytes: &[u8], deadline: Instant, st: Stage) -> Result<(), Error> {
    arm(s.tcp(), deadline)?;
    s.write_all(bytes).and_then(|()| s.flush()).map_err(|e| {
        if is_timeout(&e) {
            Error::TimedOut
        } else {
            st.fail(format!("send: {e}"))
        }
    })
}

fn recv(s: &mut ProxyStream, n: usize, deadline: Instant, st: Stage) -> Result<Vec<u8>, Error> {
    let mut buf = vec![0u8; n];
    let mut got = 0;
    while got < n {
        arm(s.tcp(), deadline)?;
        match s.read(&mut buf[got..]) {
            Ok(0) => return Err(st.fail("the proxy closed the connection".into())),
            Ok(k) => got += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) if is_timeout(&e) => return Err(Error::TimedOut),
            Err(e) => return Err(st.fail(format!("read: {e}"))),
        }
    }
    Ok(buf)
}

/// The CONNECT request's head, as curl writes it.
pub(crate) fn connect_head(p: &ProxyUrl, target: &Host, port: u16, ua: Option<&str>) -> Vec<u8> {
    let authority = format!("{target}:{port}");
    let mut h = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n");
    if let Some(c) = p.credentials() {
        h.push_str(&format!("Proxy-Authorization: {}\r\n", c.basic()));
    }
    if let Some(ua) = ua {
        h.push_str(&format!("User-Agent: {ua}\r\n"));
    }
    h.push_str("Proxy-Connection: Keep-Alive\r\n\r\n");
    h.into_bytes()
}

/// CONNECT, and the answer's head read byte by byte, so nothing of what
/// follows (the target's TLS) is taken.
fn http_connect(
    s: &mut ProxyStream,
    p: &ProxyUrl,
    target: &Host,
    port: u16,
    ua: Option<&str>,
    deadline: Instant,
) -> Result<(), Error> {
    send(s, &connect_head(p, target, port, ua), deadline, Stage::Http)?;
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") && !head.ends_with(b"\n\n") {
        if head.len() >= MAX_HEAD {
            return Err(Error::HttpProxy("the CONNECT answer is too large".into()));
        }
        head.push(recv(s, 1, deadline, Stage::Http)?[0]);
    }
    match status(&head) {
        Some(code) if (200..300).contains(&code) => Ok(()),
        Some(code) => Err(Error::TunnelRefused(code)),
        None => Err(Error::HttpProxy("the CONNECT answer is not HTTP".into())),
    }
}

/// The status of `HTTP/1.x NNN ...`.
fn status(head: &[u8]) -> Option<u16> {
    let line = head.split(|b| *b == b'\n').next()?;
    let line = std::str::from_utf8(line).ok()?;
    let rest = line.strip_prefix("HTTP/1.")?;
    let mut parts = rest.splitn(3, ' ');
    let minor = parts.next()?;
    if minor.len() != 1 || !minor.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let code = parts.next()?.trim_end_matches('\r');
    if code.len() != 3 || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    code.parse().ok()
}

#[cfg(feature = "rustls")]
fn tls_to_proxy(
    mut tcp: TcpStream,
    host: &Host,
    cfg: &std::sync::Arc<rustls::ClientConfig>,
    deadline: Instant,
) -> Result<ProxyStream, Error> {
    use rustls::pki_types::ServerName;
    let tls_err = |certificate: bool, e: io::Error| Error::Tls {
        certificate,
        source: e,
    };
    let name: ServerName<'static> = match host {
        Host::Name(n) => ServerName::try_from(n.clone())
            .map_err(|e| tls_err(false, io::Error::new(io::ErrorKind::InvalidInput, e)))?,
        Host::V4(a) => ServerName::IpAddress(IpAddr::V4(*a).into()),
        Host::V6(a) => ServerName::IpAddress(IpAddr::V6(*a).into()),
    };
    let mut cfg = (**cfg).clone();
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    let mut conn = rustls::ClientConnection::new(std::sync::Arc::new(cfg), name)
        .map_err(|e| tls_err(false, io::Error::other(e)))?;
    while conn.is_handshaking() {
        arm(&tcp, deadline)?;
        if let Err(e) = conn.complete_io(&mut tcp) {
            if is_timeout(&e) {
                return Err(Error::TimedOut);
            }
            let certificate = matches!(
                e.get_ref().and_then(|i| i.downcast_ref::<rustls::Error>()),
                Some(rustls::Error::InvalidCertificate(_))
            );
            return Err(tls_err(certificate, e));
        }
    }
    Ok(ProxyStream::Tls(Box::new(rustls::StreamOwned::new(
        conn, tcp,
    ))))
}

/// SOCKS5 (RFC 1928), with user and password (RFC 1929) when the URL
/// has a user part. `target` is an address for `socks5://`, or a name
/// for the proxy to resolve.
fn socks5(
    s: &mut ProxyStream,
    p: &ProxyUrl,
    target: &Host,
    port: u16,
    deadline: Instant,
) -> Result<(), Error> {
    let st = Stage::Socks;
    let creds = p.credentials();
    if let Some(c) = creds {
        if c.user().len() > 255 || c.password().len() > 255 {
            return Err(Error::Socks(
                "a user name or password longer than 255 bytes".into(),
            ));
        }
    }
    let greeting: &[u8] = if creds.is_some() {
        &[5, 2, 0, 2]
    } else {
        &[5, 1, 0]
    };
    send(s, greeting, deadline, st)?;
    let r = recv(s, 2, deadline, st)?;
    if r[0] != 5 {
        return Err(Error::Socks(format!(
            "the proxy answered version {}, not 5",
            r[0]
        )));
    }
    match (r[1], creds) {
        (0, _) => {}
        (2, Some(c)) => {
            let mut auth = vec![1, c.user().len() as u8];
            auth.extend_from_slice(c.user());
            auth.push(c.password().len() as u8);
            auth.extend_from_slice(c.password());
            send(s, &auth, deadline, st)?;
            let r = recv(s, 2, deadline, st)?;
            if r[1] != 0 {
                return Err(Error::Socks(
                    "the proxy refused the user name and password".into(),
                ));
            }
        }
        (0xff, _) => {
            return Err(Error::Socks(
                "the proxy took none of the authentication methods offered".into(),
            ))
        }
        (m, _) => {
            return Err(Error::Socks(format!(
                "the proxy picked method {m}, which was not offered"
            )))
        }
    }
    let mut req = vec![5, 1, 0];
    match target {
        Host::V4(a) => {
            req.push(1);
            req.extend_from_slice(&a.octets());
        }
        Host::V6(a) => {
            req.push(4);
            req.extend_from_slice(&a.octets());
        }
        Host::Name(n) => {
            // `connect` resolves a name longer than 255 bytes itself.
            req.push(3);
            req.push(n.len() as u8);
            req.extend_from_slice(n.as_bytes());
        }
    }
    req.extend_from_slice(&port.to_be_bytes());
    send(s, &req, deadline, st)?;
    let r = recv(s, 4, deadline, st)?;
    if r[0] != 5 {
        return Err(Error::Socks(format!(
            "the proxy answered version {}, not 5",
            r[0]
        )));
    }
    if r[1] != 0 {
        return Err(Error::Socks(format!(
            "the proxy refused the connection: {}",
            socks5_reply(r[1])
        )));
    }
    let len = match r[3] {
        1 => 4,
        4 => 16,
        3 => usize::from(recv(s, 1, deadline, st)?[0]),
        t => return Err(Error::Socks(format!("the proxy answered address type {t}"))),
    };
    recv(s, len + 2, deadline, st)?;
    Ok(())
}

/// RFC 1928's reply codes.
fn socks5_reply(code: u8) -> String {
    match code {
        1 => "general failure".into(),
        2 => "not allowed by the rules".into(),
        3 => "network unreachable".into(),
        4 => "host unreachable".into(),
        5 => "connection refused".into(),
        6 => "TTL expired".into(),
        7 => "command not supported".into(),
        8 => "address type not supported".into(),
        c => format!("code {c}"),
    }
}

/// SOCKS4, or SOCKS4a for a name. The URL's user is the user id.
fn socks4(
    s: &mut ProxyStream,
    p: &ProxyUrl,
    target: &Host,
    port: u16,
    deadline: Instant,
) -> Result<(), Error> {
    let st = Stage::Socks;
    let user = p.credentials().map_or(&b""[..], |c| c.user());
    if user.contains(&0) {
        return Err(Error::Socks("a user id with a NUL byte".into()));
    }
    let (ip, name) = match target {
        Host::V4(a) => (*a, None),
        Host::V6(_) => return Err(Error::Socks("SOCKS4 can't reach an IPv6 address".into())),
        Host::Name(n) => {
            if n.as_bytes().contains(&0) {
                return Err(Error::Socks("a host name with a NUL byte".into()));
            }
            (Ipv4Addr::new(0, 0, 0, 1), Some(n))
        }
    };
    let mut req = vec![4, 1];
    req.extend_from_slice(&port.to_be_bytes());
    req.extend_from_slice(&ip.octets());
    req.extend_from_slice(user);
    req.push(0);
    if let Some(n) = name {
        req.extend_from_slice(n.as_bytes());
        req.push(0);
    }
    send(s, &req, deadline, st)?;
    let r = recv(s, 8, deadline, st)?;
    if r[1] != 90 {
        return Err(Error::Socks(format!(
            "the proxy refused the connection (code {})",
            r[1]
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
