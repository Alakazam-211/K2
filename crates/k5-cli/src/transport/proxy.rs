//! The person's proxy, for an `https://` request to another machine
//! (the shared transport's rule): what curl 8.5 does with the
//! proxy variables, for the one kind of target that may use them.
//!
//! Which requests: ONLY a [`super::wire::Target::Tls`] to a host that is
//! not this computer, as the shell's `_k2_direct_url` rule says
//! (loopback and plain http never use a proxy). The decision is made in
//! [`super::remote`], which is the only reader of the process
//! environment for it ([`from_process_env`]); a daemon target
//! ([`super::wire::Target::Tcp`] or [`super::wire::Target::Unix`]) has
//! no place to hold a proxy at all.
//!
//! curl's rule lives in the `proxy-env` crate (`crates/proxy-env`,
//! checked against curl 8.5.0 run beside it): which variable names the
//! proxy, `no_proxy`, the proxy URL (`http://`, `https://`, `socks4://`,
//! `socks4a://`, `socks5://`, `socks5h://`, IPv6 hosts, a percent-decoded
//! user part), and the tunnel through it. [`decide`] asks it for a host;
//! [`through`] opens the tunnel and words a failure in curl's exit codes
//! ([`Failure`]), as `_k2_curl` sees them:
//!
//! - a proxy setting curl refuses (another scheme, a URL it can't read)
//!   is carried to the connect and fails there, with curl's code (7 or
//!   5) and nothing sent: never a direct request in its place;
//! - a refused CONNECT is curl 8.5's 56 ([`Failure::Tunnel`]), one try:
//!   the named difference curl-version-proxy-exit
//!   (`scripts/cli-parity.sh`) covers newer curl's 7;
//! - a SOCKS handshake that fails is curl's 97, a proxy that refuses the
//!   connection 7 (tried again as any refused connect is), one whose name
//!   doesn't resolve 5, one that doesn't answer in time 28;
//! - an `https://` proxy's certificate is checked with the system's
//!   roots, as curl's built-in bundle: the CA variables are the target's
//!   alone ([`super::tls::proxy_shared`]). 60 when it fails, 35 for any
//!   other handshake failure.
//!
//! A proxy URL's user and password are the person's own credential for
//! their proxy: sent only to it, in its handshake (the CONNECT head, or
//! SOCKS5's user and password), never printed. The token rides inside
//! the tunnel only.
//!
//! The Python rule ([`python`], [`Proxy`], [`Route`], [`tunnel_head`]) is
//! another reader of the same variables: the shell's embedded Python
//! reads them as urllib does.

use super::wire::{curl, Failure};
use std::ffi::OsString;
use std::fmt;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

/// curl 5: the proxy's name doesn't resolve, or its URL can't be read.
pub const COULDNT_RESOLVE_PROXY: i32 = proxy_env::exit::COULDNT_RESOLVE_PROXY;
/// curl 97: the SOCKS handshake failed.
pub const PROXY_HANDSHAKE: i32 = proxy_env::exit::PROXY;
/// The most of a CONNECT answer's head read (curl's limit is 100 KiB).
const MAX_HEAD: usize = 100 * 1024;

/// The proxy's user and password as the shell's Python reads them,
/// percent-decoded. Printed and logged as `<redacted>` only; the one place
/// they are written out is the CONNECT head's `Proxy-Authorization` line
/// ([`python::connect_head`]).
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    user: Vec<u8>,
    pass: Vec<u8>,
}

impl Credentials {
    /// `Basic <base64 of user:password>`.
    fn basic(&self) -> String {
        let mut raw = self.user.clone();
        raw.push(b':');
        raw.extend_from_slice(&self.pass);
        format!("Basic {}", base64(&raw))
    }
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Credentials(<redacted>)")
    }
}

impl fmt::Display for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// An `http://` proxy, as the shell's Python reads one ([`python`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proxy {
    pub host: String,
    pub port: u16,
    pub auth: Option<Credentials>,
}

/// How a request a Python block sends goes ([`python::decide`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// No proxy.
    Direct,
    /// Through this proxy's CONNECT tunnel.
    Via(Proxy),
    /// A setting the binary doesn't read as the shell's Python does: the
    /// caller sends nothing and names it.
    Shell,
}

/// The proxy curl would use for a request ([`decide`]), or the setting
/// curl refuses, which fails the request at its connect
/// ([`through`]).
pub type Via = Result<proxy_env::ProxyUrl, proxy_env::ConfigError>;

/// A variable that is set but is not UTF-8.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotText;

/// The process environment, as curl reads it. Only [`super::remote`]
/// calls it: the daemon's paths never read a proxy.
pub(in crate::transport) fn from_process_env(name: &str) -> Option<OsString> {
    std::env::var_os(name)
}

/// The shell's `_k2_direct_url` for `https://<host>`, step for step:
/// `host` is the URL's text after the scheme, cut at `/`, `?` and the
/// last `@` (a caller may pass less: a name, or the text before its first
/// `:`). This computer when it is `[::1]` or starts `[::1]:`; else, cut
/// at its first `:`, one trailing dot off and lower-cased, `localhost` or
/// a name ending `.localhost`; else what `_k2_url_is_literal_loopback`
/// says for `https://<that>` (it cuts at `#` too: `localhost`, or an
/// address in 127.0.0.0/8 written as four numbers). So `127.example` is
/// a name, and `::1`, `::ffff:127.0.0.1` and `[::ffff:127.0.0.1]` are not
/// this computer: they may use the proxy, as in the shell.
pub fn loopback(host: &str) -> bool {
    if host == "[::1]" || host.starts_with("[::1]:") {
        return true;
    }
    let h = host.split(':').next().unwrap_or("");
    let h = h.strip_suffix('.').unwrap_or(h).to_ascii_lowercase();
    if h == "localhost" || h.ends_with(".localhost") {
        return true;
    }
    crate::prelude::plain_http::url_is_literal_loopback(&format!("https://{h}"))
}

/// A URL's host, cut as the shell cuts it in `_k2_direct_url`: after the
/// scheme, to the first `/`, then the first `?`, after the last `@`; a
/// bracketed IPv6 address to its `]`, else to the first `:`.
pub fn url_host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let h = rest.split('/').next().unwrap_or("");
    let h = h.split('?').next().unwrap_or("");
    let h = h.rsplit_once('@').map_or(h, |(_, h)| h);
    if h.starts_with('[') {
        return h.find(']').map_or(h, |i| &h[..=i]);
    }
    h.split(':').next().unwrap_or("")
}

/// The proxy for a request to `host` (as [`super::remote::base`] reads
/// it), `https` or not: none for plain http and for this computer
/// (`_k2_direct_url`, [`loopback`]); else curl's rule
/// ([`proxy_env::proxy_for`]) over `env`.
pub fn decide(host: &str, https: bool, env: impl Fn(&str) -> Option<OsString>) -> Option<Via> {
    if !https || loopback(host) {
        return None;
    }
    proxy_env::proxy_for("https", host, env).transpose()
}

/// The tunnel through `via` to `host:port` (the TCP connect, TLS to an
/// `https://` proxy, its handshake), before `limit`, as curl counts the
/// tunnel in its connect time. A failure is curl's code for it, and
/// nothing of the request went out.
pub fn through(
    via: &Via,
    host: &str,
    port: u16,
    user_agent: &str,
    limit: Instant,
) -> Result<proxy_env::ProxyStream, Failure> {
    let p = match via {
        Ok(p) => p,
        Err(e) => return Err(failure(proxy_env::Error::Config(e.clone()))),
    };
    let mut opts = proxy_env::Options::new()
        .user_agent(user_agent)
        .deadline(limit);
    if p.scheme() == proxy_env::Scheme::Https {
        opts = opts.proxy_tls(super::tls::proxy_shared()?);
    }
    proxy_env::connect(Some(p), host, port, &opts).map_err(failure)
}

/// The crate's error as the transport's [`Failure`], by curl's code: a
/// refused connect stays [`Failure::Connect`] (so it is tried again as
/// `_k2_curl` tries curl's 7), a timeout [`Failure::ConnectTimedOut`], a
/// failed CONNECT [`Failure::Tunnel`]; every other code is
/// [`Failure::Broken`] with nothing sent ([`super::tls::before_request`]).
fn failure(e: proxy_env::Error) -> Failure {
    use proxy_env::Error as E;
    match e {
        E::Connect(io) => Failure::Connect(io),
        E::TimedOut => Failure::ConnectTimedOut,
        E::TunnelRefused(_) | E::HttpProxy(_) => Failure::Tunnel(e.to_string()),
        other => {
            let code = other.curl_exit_code();
            if code == curl::COULDNT_CONNECT {
                Failure::Connect(io::Error::other(other.to_string()))
            } else {
                Failure::Broken {
                    curl: code,
                    why: other.to_string(),
                }
            }
        }
    }
}

/// Standard base64 with padding (also the TLS tests' PEM writer).
pub(crate) fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if i <= c.len() {
                out.push(T[((n >> shift) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn remaining(limit: Instant) -> Option<Duration> {
    limit
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
}

fn tunnel_failed(why: impl Into<String>) -> Failure {
    Failure::Tunnel(why.into())
}

/// The CONNECT exchange of the Python rule's clients: the proxy's TCP
/// connect, `head` (the whole CONNECT request, as that client writes
/// it), and the answer's head read byte by byte (nothing of the target's
/// TLS is read). `opens` says which status opens the tunnel (Python's
/// `http.client`: only 200). When
/// it fails, the bytes of the proxy's answer read before it did (none
/// for a connect that failed). Before `limit`.
pub fn tunnel_head(
    p: &Proxy,
    head: &[u8],
    opens: impl Fn(u16) -> bool,
    limit: Instant,
    connect: impl FnOnce(&str, u16, Instant) -> Result<TcpStream, Failure>,
) -> Result<TcpStream, (Failure, Vec<u8>)> {
    let mut s = match connect(&p.host, p.port, limit) {
        Err(Failure::Broken { curl: c, why }) if c == curl::COULDNT_RESOLVE_HOST => {
            return Err((
                Failure::Broken {
                    curl: COULDNT_RESOLVE_PROXY,
                    why,
                },
                Vec::new(),
            ))
        }
        r => r.map_err(|f| (f, Vec::new()))?,
    };
    let timeouts = |s: &TcpStream| -> Result<(), Failure> {
        let d = remaining(limit).ok_or(Failure::ConnectTimedOut)?;
        s.set_read_timeout(Some(d))
            .and_then(|()| s.set_write_timeout(Some(d)))
            .map_err(|e| tunnel_failed(format!("proxy: set a timeout: {e}")))
    };
    timeouts(&s).map_err(|f| (f, Vec::new()))?;
    if let Err(e) = s.write_all(head).and_then(|()| s.flush()) {
        let f = if is_timeout(&e) {
            Failure::ConnectTimedOut
        } else {
            tunnel_failed(format!("proxy: send CONNECT: {e}"))
        };
        return Err((f, Vec::new()));
    }
    let mut got = Vec::new();
    let mut byte = [0u8; 1];
    while !got.ends_with(b"\r\n\r\n") && !got.ends_with(b"\n\n") {
        if got.len() >= MAX_HEAD {
            return Err((tunnel_failed("proxy: CONNECT answer too large"), got));
        }
        if let Err(f) = timeouts(&s) {
            return Err((f, got));
        }
        match s.read(&mut byte) {
            Ok(0) => return Err((tunnel_failed("proxy: CONNECT aborted"), got)),
            Ok(_) => got.push(byte[0]),
            Err(e) if is_timeout(&e) => return Err((Failure::ConnectTimedOut, got)),
            Err(e) => {
                return Err((
                    tunnel_failed(format!("proxy: read the CONNECT answer: {e}")),
                    got,
                ))
            }
        }
    }
    match status(&got) {
        Some(code) if opens(code) => Ok(s),
        Some(code) => Err((
            tunnel_failed(format!("CONNECT tunnel failed, response {code}")),
            got,
        )),
        None => Err((tunnel_failed("proxy: the CONNECT answer is not HTTP"), got)),
    }
}

fn is_timeout(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
    )
}

/// The status of `HTTP/1.x NNN …`.
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

pub mod python;

/// The process environment as Python's `os.environ` lists it, read as
/// the shell's embedded Python reads its proxy ([`python::env`]). Only
/// [`super::remote`] calls it: the daemon's paths never read a proxy.
pub(in crate::transport) fn python_environ() -> python::Env {
    python::env(std::env::vars_os().map(|(n, v)| (n.into_encoded_bytes(), v.into_encoded_bytes())))
}

#[cfg(test)]
mod tests;
