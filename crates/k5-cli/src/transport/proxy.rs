//! The person's proxy, for an `https://` request to another machine
//! (K2's ruling on the shared transport): what curl 8.5 does with the
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
//! What curl does, and this does too:
//!
//! - `no_proxy`, else `NO_PROXY`: a lone `*` turns every proxy off; a
//!   name token matches the host exactly or at a dot boundary, ignoring
//!   case, a leading or trailing dot on the token and a trailing dot on
//!   the host; for an IPv4 host, a token is an address or a CIDR range.
//! - The proxy: `https_proxy`, else `HTTPS_PROXY`, for an https target;
//!   `http_proxy` only (never `HTTP_PROXY`) for an http one; then
//!   `all_proxy`, else `ALL_PROXY`. An empty value counts as unset.
//! - The proxy URL: `http://` or no scheme, `[user[:password]@]host[:port]`,
//!   port 1080 when none is given; the user and password are
//!   percent-decoded and sent as `Proxy-Authorization: Basic`, in the
//!   CONNECT head only.
//! - `CONNECT <host>:<port> HTTP/1.1`, then `Host`, `Proxy-Authorization`,
//!   `User-Agent` and `Proxy-Connection: Keep-Alive`, in that order. No
//!   other header of the request goes to the proxy (curl's default
//!   `CURLHEADER_SEPARATE`): the token rides inside the tunnel only.
//! - A 2xx opens the tunnel. Anything else, or a proxy that closes or
//!   answers with something that is not HTTP, is curl's 56; nothing of the
//!   request was sent. A proxy that refuses the connection is curl's 7
//!   (tried again as any refused connect is), one whose name doesn't
//!   resolve curl's 5, one that doesn't answer in time curl's 28.
//!
//! What the binary leaves to the shell ([`Route::Shell`]), where curl
//! versions differ or the binary has no client: an `https://`, `socks*`
//! or other scheme's proxy, an IPv6 proxy, a scheme-less proxy curl would
//! guess another scheme for (`ftp.`, `dict.`, `ldap.`, `imap.`, `smtp.`,
//! `pop3.`), a proxy URL with a byte or a part the binary doesn't read
//! (a query, a second `@`, a bad escape, a control byte), a scheme-less
//! proxy with a user part, a `no_proxy` with blank-separated tokens (curl
//! 8.5 reads on past a blank, 8.11 stops there), a variable that is not
//! UTF-8, and a target name with a capital letter or a trailing dot.

use super::wire::{curl, Failure};
use std::fmt;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, TcpStream};
use std::time::{Duration, Instant};

/// curl's default proxy port.
pub const DEFAULT_PORT: u16 = 1080;
/// curl 5: the proxy's name doesn't resolve.
pub const COULDNT_RESOLVE_PROXY: i32 = 5;
/// The most of a CONNECT answer's head read (curl's limit is 100 KiB).
const MAX_HEAD: usize = 100 * 1024;

/// The proxy's user and password, percent-decoded. Printed and logged as
/// `<redacted>` only; the one place they are written out is the CONNECT
/// head's `Proxy-Authorization` line ([`connect_head`]).
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

/// An `http://` proxy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proxy {
    pub host: String,
    pub port: u16,
    pub auth: Option<Credentials>,
}

/// How a request to a host goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// No proxy.
    Direct,
    /// Through this proxy's CONNECT tunnel.
    Via(Proxy),
    /// A setting the binary doesn't read as every curl does: the shell
    /// answers the command.
    Shell,
}

/// A variable that is set but is not UTF-8.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotText;

/// One variable's value: `Ok(None)` unset or empty.
pub type Lookup = Result<Option<String>, NotText>;

/// The process environment, as curl reads it (empty is unset). Only
/// [`super::remote`] calls it: the daemon's paths never read a proxy.
pub(in crate::transport) fn from_process_env(name: &str) -> Lookup {
    match std::env::var_os(name) {
        None => Ok(None),
        Some(v) if v.is_empty() => Ok(None),
        Some(v) => v.into_string().map(Some).map_err(|_| NotText),
    }
}

/// The shell's `_k2_direct_url` for `https://<host>`: this computer
/// (`localhost`, a name under `.localhost`, `127.*`, `[::1]`), by the
/// text alone. One trailing dot is dropped, case ignored.
pub fn loopback(host: &str) -> bool {
    if host == "[::1]" {
        return true;
    }
    let h = host.strip_suffix('.').unwrap_or(host).to_ascii_lowercase();
    h == "localhost" || h.ends_with(".localhost") || h.starts_with("127.")
}

/// The shell's `_remote_url_is_loopback URL`, which is `_k2_direct_url
/// "https://${URL#*://}"`: the host part read by the text as the shell
/// cuts it (to the first `/`, then the first `?`, after the last `@`;
/// `[::1]` as it is, else to the first `:`), then [`loopback`].
pub fn url_is_loopback(url: &str) -> bool {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let h = rest.split('/').next().unwrap_or("");
    let h = h.split('?').next().unwrap_or("");
    let h = h.rsplit_once('@').map_or(h, |(_, h)| h);
    if h == "[::1]" || h.starts_with("[::1]:") {
        return true;
    }
    loopback(h.split(':').next().unwrap_or(""))
}

/// The route for a request to `host` (a plain name or an IPv4 address,
/// as [`super::remote::base`] reads it), `https` or not.
pub fn decide(host: &str, https: bool, env: impl Fn(&str) -> Lookup) -> Route {
    // Loopback and plain http never use a proxy (`_k2_direct_url`).
    if !https || loopback(host) {
        return Route::Direct;
    }
    let raw = match proxy_setting(https, &env) {
        Ok(Some(raw)) => raw,
        Ok(None) => return Route::Direct,
        Err(NotText) => return Route::Shell,
    };
    match no_proxy_setting(&env) {
        Err(NotText) => return Route::Shell,
        Ok(Some(list)) => match no_proxy(host, &list) {
            NoProxy::Skip => return Route::Direct,
            NoProxy::Shell => return Route::Shell,
            NoProxy::Use => {}
        },
        Ok(None) => {}
    }
    if host.ends_with('.') || host.bytes().any(|b| b.is_ascii_uppercase()) {
        return Route::Shell;
    }
    match parse_proxy(&raw) {
        Some(p) => Route::Via(p),
        None => Route::Shell,
    }
}

fn first(names: &[&str], env: &impl Fn(&str) -> Lookup) -> Lookup {
    for n in names {
        if let Some(v) = env(n)? {
            return Ok(Some(v));
        }
    }
    Ok(None)
}

/// curl's `detect_proxy`: the scheme's own variable, then `all_proxy`
/// and `ALL_PROXY`. `HTTP_PROXY` is never read.
pub fn proxy_setting(https: bool, env: &impl Fn(&str) -> Lookup) -> Lookup {
    let own: &[&str] = if https {
        &["https_proxy", "HTTPS_PROXY"]
    } else {
        &["http_proxy"]
    };
    match first(own, env)? {
        Some(v) => Ok(Some(v)),
        None => first(&["all_proxy", "ALL_PROXY"], env),
    }
}

/// `no_proxy`, else `NO_PROXY`.
pub fn no_proxy_setting(env: &impl Fn(&str) -> Lookup) -> Lookup {
    first(&["no_proxy", "NO_PROXY"], env)
}

/// What `no_proxy` says for a host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoProxy {
    /// The host is listed (or `*`): no proxy.
    Skip,
    /// Not listed.
    Use,
    /// Blank-separated tokens: curl versions read them differently.
    Shell,
}

/// curl 8.5's `Curl_check_noproxy` for a plain name or an IPv4 address.
pub fn no_proxy(host: &str, list: &str) -> NoProxy {
    if list == "*" {
        return NoProxy::Skip;
    }
    if list.bytes().any(|b| b == b' ' || b == b'\t') {
        return NoProxy::Shell;
    }
    let ip: Option<Ipv4Addr> = host.parse().ok();
    let name = if ip.is_none() {
        host.strip_suffix('.').unwrap_or(host)
    } else {
        host
    };
    for token in list.split(',').filter(|t| !t.is_empty()) {
        let hit = match ip {
            Some(ip) => cidr4(ip, token),
            None => name_match(name, token),
        };
        if hit {
            return NoProxy::Skip;
        }
    }
    NoProxy::Use
}

fn name_match(name: &str, token: &str) -> bool {
    let token = token.strip_suffix('.').unwrap_or(token);
    let token = token.strip_prefix('.').unwrap_or(token);
    let (n, t) = (name.as_bytes(), token.as_bytes());
    if t.len() == n.len() {
        return n.eq_ignore_ascii_case(t);
    }
    // `token` is empty here only when the token was `.` or `..`, and an
    // empty tail never matches (curl: the byte before it is the name's
    // last, never a dot, as the name's trailing dot is gone).
    t.len() < n.len()
        && !t.is_empty()
        && n[n.len() - t.len() - 1] == b'.'
        && n[n.len() - t.len()..].eq_ignore_ascii_case(t)
}

/// curl's `Curl_cidr4_match`: `token` is an address, or `address/bits`
/// with `bits` read as C's `atoi` (0 or 32: the exact address; above 32:
/// no match).
fn cidr4(ip: Ipv4Addr, token: &str) -> bool {
    let (net, bits) = match token.split_once('/') {
        Some((net, bits)) => (net, atoi(bits)),
        None => (token, 0),
    };
    let Ok(net) = net.parse::<Ipv4Addr>() else {
        return false;
    };
    if !(0..=32).contains(&bits) {
        return false;
    }
    let (a, n) = (u32::from(ip), u32::from(net));
    if bits == 0 || bits == 32 {
        return a == n;
    }
    let mask = u32::MAX << (32 - bits);
    (a ^ n) & mask == 0
}

/// C's `atoi`: leading blanks, a sign, then digits; anything else stops
/// it. Out of range reads as -1 (never a match).
fn atoi(s: &str) -> i64 {
    let s = s.trim_start_matches([' ', '\t', '\n', '\r', '\x0b', '\x0c']);
    let (neg, digits) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let end = digits
        .bytes()
        .position(|b| !b.is_ascii_digit())
        .unwrap_or(digits.len());
    let digits = &digits[..end];
    if digits.is_empty() {
        return 0;
    }
    match digits.parse::<i64>() {
        Ok(v) if v <= i64::from(i32::MAX) => {
            if neg {
                -v
            } else {
                v
            }
        }
        _ => -1,
    }
}

/// The schemes curl guesses for a scheme-less URL by its first label.
const GUESSED: [&str; 6] = ["ftp.", "dict.", "ldap.", "imap.", "smtp.", "pop3."];

/// A proxy URL the binary reads as curl does; `None`: the shell's.
pub fn parse_proxy(raw: &str) -> Option<Proxy> {
    if raw.bytes().any(|b| b <= b' ' || b == 0x7f) {
        return None;
    }
    let (schemed, rest) = match raw.find("://") {
        Some(i) => {
            if !raw[..i].eq_ignore_ascii_case("http") {
                return None;
            }
            (true, &raw[i + 3..])
        }
        None => (false, raw),
    };
    // The authority ends at the path; a query or a fragment is the
    // shell's.
    let end = rest.find('/').unwrap_or(rest.len());
    let (authority, _path) = rest.split_at(end);
    if authority.contains(['?', '#']) {
        return None;
    }
    let (userinfo, hostport) = match authority.split_once('@') {
        Some((u, h)) => {
            if h.contains('@') || !schemed {
                return None;
            }
            (Some(u), h)
        }
        None => (None, authority),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => {
            if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            (h, p.parse::<u16>().ok().filter(|p| *p != 0)?)
        }
        None => (hostport, DEFAULT_PORT),
    };
    let plain = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_');
    if host.is_empty() || !host.bytes().all(plain) {
        return None;
    }
    if !schemed {
        let lower = host.to_ascii_lowercase();
        if GUESSED.iter().any(|g| lower.starts_with(g)) {
            return None;
        }
    }
    let auth = match userinfo {
        None => None,
        Some(u) => {
            let (user, pass) = u.split_once(':').unwrap_or((u, ""));
            Some(Credentials {
                user: decode(user)?,
                pass: decode(pass)?,
            })
        }
    };
    Some(Proxy {
        host: host.to_string(),
        port,
        auth,
    })
}

/// A user or password part, percent-decoded. `None`: a byte the binary
/// doesn't take there, a broken escape, or a control byte once decoded.
fn decode(part: &str) -> Option<Vec<u8>> {
    let ok = |b: u8| {
        b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'-' | b'.' | b'_' | b'~' | b'!' | b'$' | b'&' | b'\'' | b'(' | b')' | b'*'
                    | b'+' | b',' | b'=' | b':'
            )
    };
    let b = part.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = part.get(i + 1..i + 3)?;
            let v = u8::from_str_radix(hex, 16).ok()?;
            if !hex.bytes().all(|c| c.is_ascii_hexdigit()) || v < 0x20 || v == 0x7f {
                return None;
            }
            out.push(v);
            i += 3;
        } else if ok(b[i]) {
            out.push(b[i]);
            i += 1;
        } else {
            return None;
        }
    }
    Some(out)
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

/// The CONNECT request's head, as curl writes it.
pub fn connect_head(p: &Proxy, host: &str, port: u16, user_agent: &str) -> Vec<u8> {
    let authority = format!("{host}:{port}");
    let mut h = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n");
    if let Some(c) = &p.auth {
        h.push_str(&format!("Proxy-Authorization: {}\r\n", c.basic()));
    }
    h.push_str(&format!(
        "User-Agent: {user_agent}\r\nProxy-Connection: Keep-Alive\r\n\r\n"
    ));
    h.into_bytes()
}

fn remaining(limit: Instant) -> Option<Duration> {
    limit
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
}

fn tunnel_failed(why: impl Into<String>) -> Failure {
    Failure::Tunnel(why.into())
}

/// The tunnel through `p` to `host:port`: the proxy's TCP connect, the
/// CONNECT head, and its answer's head read byte by byte (nothing of the
/// target's TLS is read). Before `limit`, as curl counts the tunnel in
/// its connect time.
pub fn tunnel(
    p: &Proxy,
    host: &str,
    port: u16,
    user_agent: &str,
    limit: Instant,
    connect: impl FnOnce(&str, u16, Instant) -> Result<TcpStream, Failure>,
) -> Result<TcpStream, Failure> {
    let mut s = match connect(&p.host, p.port, limit) {
        Err(Failure::Broken { curl: c, why }) if c == curl::COULDNT_RESOLVE_HOST => {
            return Err(Failure::Broken {
                curl: COULDNT_RESOLVE_PROXY,
                why,
            })
        }
        r => r?,
    };
    let timeouts = |s: &TcpStream| -> Result<(), Failure> {
        let d = remaining(limit).ok_or(Failure::ConnectTimedOut)?;
        s.set_read_timeout(Some(d))
            .and_then(|()| s.set_write_timeout(Some(d)))
            .map_err(|e| tunnel_failed(format!("proxy: set a timeout: {e}")))
    };
    timeouts(&s)?;
    let head = connect_head(p, host, port, user_agent);
    if let Err(e) = s.write_all(&head).and_then(|()| s.flush()) {
        return Err(if is_timeout(&e) {
            Failure::ConnectTimedOut
        } else {
            tunnel_failed(format!("proxy: send CONNECT: {e}"))
        });
    }
    let mut got = Vec::new();
    let mut byte = [0u8; 1];
    while !got.ends_with(b"\r\n\r\n") && !got.ends_with(b"\n\n") {
        if got.len() >= MAX_HEAD {
            return Err(tunnel_failed("proxy: CONNECT answer too large"));
        }
        timeouts(&s)?;
        match s.read(&mut byte) {
            Ok(0) => return Err(tunnel_failed("proxy: CONNECT aborted")),
            Ok(_) => got.push(byte[0]),
            Err(e) if is_timeout(&e) => return Err(Failure::ConnectTimedOut),
            Err(e) => return Err(tunnel_failed(format!("proxy: read the CONNECT answer: {e}"))),
        }
    }
    match status(&got) {
        Some(code) if (200..300).contains(&code) => Ok(s),
        Some(code) => Err(tunnel_failed(format!(
            "CONNECT tunnel failed, response {code}"
        ))),
        None => Err(tunnel_failed("proxy: the CONNECT answer is not HTTP")),
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

#[cfg(test)]
mod tests;
