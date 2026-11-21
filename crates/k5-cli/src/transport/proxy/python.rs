//! The person's proxy as the shell's embedded Python reads it (K2's
//! ruling for the urllib-shaped families): `_K2_PY_PROXY_RULE` installs a
//! `urllib.request.ProxyHandler()` with the environment's settings, and
//! skips it for a URL `k2_direct` calls this computer's. For an `https://`
//! daemon on another machine, urllib then tunnels through the proxy with
//! `http.client`'s CONNECT and checks the daemon's certificate inside it.
//! The CONNECT itself goes through the shared path
//! ([`super::tunnel_head`]); this module is only how Python reads the
//! settings and writes its CONNECT head, which differ from curl's
//! ([`super::decide`]):
//!
//! - **The variables** (`getproxies_environment`): every variable whose
//!   name ends in `_proxy` in any case, keyed by the name before it in
//!   lower case. One with a value counts; then a name ending in `_proxy`
//!   in lower case wins over the others, and one of those that is empty
//!   takes the key away. An https request reads the key `https` alone:
//!   `all_proxy` is never read (urllib has no handler for it).
//! - **no_proxy** (`proxy_bypass_environment`, key `no`): `*` skips every
//!   proxy; else each comma-separated token, blanks around it stripped and
//!   its leading dots dropped, in lower case, matches the request's host
//!   (`host:port`, in lower case) or the host without its port, exactly
//!   or as a suffix after a dot. No address ranges: an IP is a token like
//!   any other.
//! - **The proxy URL** (`_parse_proxy`): `http://[user[:password]@]host[:port][/...]`
//!   or `[user[:password]@]host[:port]` with no scheme. The port is 443
//!   when none is given (urllib's https connection's own). A user and a
//!   password, both non-empty, percent-decoded, are sent as
//!   `Proxy-Authorization: Basic`, in the CONNECT head only; a user alone
//!   sends nothing.
//! - **The CONNECT head** (`http.client`'s `_tunnel`, Python 3.12 and
//!   later): `CONNECT <host>:<port> HTTP/1.1`, `Proxy-Authorization` when
//!   given, then `Host: <host>:<port>`. No `User-Agent`, and no other
//!   header of the request: the token rides inside the tunnel only.
//! - **The answer:** 200 opens the tunnel, any other status fails it.
//!
//! What the binary doesn't read as Python does is [`Route::Shell`]: the
//! caller sends nothing ([`super::super::urllib`]), never a direct or
//! plain request in its place. That is a proxy URL with another scheme
//! (`https://`, `socks5://`: Python sends a plain CONNECT to that host
//! too), a byte the binary doesn't take in it, a port it doesn't read, and
//! a variable that is not UTF-8.

use super::{Credentials, NotText, Proxy, Route};

/// urllib's https connection's default port, for a proxy URL with none.
pub const DEFAULT_PORT: u16 = 443;

/// What `getproxies_environment()` hands an https request: the `https`
/// key and the `no` key, and whether the dict held anything at all
/// (macOS reads its system setting when it holds nothing: the named
/// difference macos-system-proxy).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Env {
    pub https: Option<Result<String, NotText>>,
    pub no: Option<Result<String, NotText>>,
    pub any: bool,
}

/// `getproxies_environment()` over the variables in the environment's
/// order (`os.environ`'s), each name and value as bytes.
pub fn env<N: AsRef<[u8]>, V: AsRef<[u8]>>(vars: impl IntoIterator<Item = (N, V)>) -> Env {
    // (key, the name ends in `_proxy` in lower case, the value, set).
    let mut seen: Vec<(String, bool, Result<String, NotText>, bool)> = Vec::new();
    // A name that isn't UTF-8 is never the key `https` or `no` (Python
    // reads it with its bad bytes kept), but it fills the dict.
    let mut other = false;
    for (name, value) in vars {
        let (name, value) = (name.as_ref(), value.as_ref());
        if name.len() <= 5
            || name[name.len() - 6] != b'_'
            || !name[name.len() - 5..].eq_ignore_ascii_case(b"proxy")
        {
            continue;
        }
        let Ok(name) = std::str::from_utf8(name) else {
            other |= !value.is_empty();
            continue;
        };
        let text = std::str::from_utf8(value)
            .map(str::to_string)
            .map_err(|_| NotText);
        seen.push((
            name[..name.len() - 6].to_lowercase(),
            name.ends_with("_proxy"),
            text,
            !value.is_empty(),
        ));
    }
    // The first pass: any spelling with a value; the second: the lower
    // case spellings, an empty one taking the key away. The last wins.
    let mut keys: Vec<(&str, &Result<String, NotText>)> = Vec::new();
    let passes = seen
        .iter()
        .filter(|s| s.3)
        .map(|s| (s, true))
        .chain(seen.iter().filter(|s| s.1).map(|s| (s, s.3)));
    for ((key, _, text, _), keep) in passes {
        keys.retain(|(have, _)| *have != key.as_str());
        if keep {
            keys.push((key.as_str(), text));
        }
    }
    let get = |k: &str| {
        keys.iter()
            .find(|(have, _)| *have == k)
            .map(|(_, v)| (*v).clone())
    };
    Env {
        https: get("https"),
        no: get("no"),
        any: other || !keys.is_empty(),
    }
}

/// Python's `str.strip()` and `str.split()` whitespace.
fn py_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// `urllib.parse._splitport`: the host before a last `:` followed by
/// digits only (or nothing), else the whole.
fn host_only(host: &str) -> &str {
    match host.rsplit_once(':') {
        Some((h, port)) if port.bytes().all(|b| b.is_ascii_digit()) => h,
        _ => host,
    }
}

/// `proxy_bypass_environment(host)` with `no` (the `no` key): whether
/// the request to `host` (urllib's `req.host`: the URL's `host:port`)
/// skips the proxy.
pub fn bypass(host: &str, no: &str) -> bool {
    if no == "*" {
        return true;
    }
    let host = host.to_lowercase();
    let only = host_only(&host);
    for name in no.split(',') {
        let name = name.trim_matches(py_space);
        if name.is_empty() {
            continue;
        }
        let name = name.trim_start_matches('.').to_lowercase();
        if only == name || host == name {
            return true;
        }
        let dotted = format!(".{name}");
        if only.ends_with(&dotted) || host.ends_with(&dotted) {
            return true;
        }
    }
    false
}

/// `urllib.parse.unquote`: each `%XX` as its byte, anything else as it
/// is, then UTF-8 with a bad sequence as U+FFFD.
fn unquote(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let (hi, lo) = (char::from(b[i + 1]), char::from(b[i + 2]));
            if let (Some(hi), Some(lo)) = (hi.to_digit(16), lo.to_digit(16)) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A proxy URL as `ProxyHandler` reads it for an https request; `None`:
/// one the binary doesn't read as Python does.
pub fn parse(raw: &str) -> Option<Proxy> {
    if raw.chars().any(|c| c.is_control() || py_space(c)) {
        return None;
    }
    // `_splittype`: a scheme is the text before the first `:` with no `/`
    // in it; only a rest that starts with `/` makes it a URL.
    let scheme_end = raw.find(':').filter(|&i| i > 0 && !raw[..i].contains('/'));
    let authority = match scheme_end {
        Some(i) if raw[i + 1..].starts_with('/') => {
            if !raw[..i].eq_ignore_ascii_case("http") {
                return None;
            }
            let rest = raw[i + 1..].strip_prefix("//")?;
            // The authority ends at the first `/` after the last part's
            // `@` (Python's `find('@')`: the first `@`), else after `//`.
            let from = rest.find('@').unwrap_or(0);
            let end = rest[from..].find('/').map_or(rest.len(), |j| from + j);
            &rest[..end]
        }
        None if raw.starts_with('/') => return None,
        _ => raw,
    };
    let (userinfo, hostport) = match authority.rsplit_once('@') {
        Some((u, h)) => (Some(u), h),
        None => (None, authority),
    };
    let auth = userinfo.and_then(|u| {
        let (user, pass) = u.split_once(':')?;
        (!user.is_empty() && !pass.is_empty()).then(|| Credentials {
            user: unquote(user).into_bytes(),
            pass: unquote(pass).into_bytes(),
        })
    });
    let hostport = unquote(hostport);
    // `HTTPSConnection(hostport)`'s `_get_hostport`.
    let close = hostport.rfind(']');
    let (host, port) = match hostport.rfind(':') {
        Some(i) if close.is_none_or(|j| i > j) => {
            let digits = &hostport[i + 1..];
            let port = if digits.is_empty() {
                DEFAULT_PORT
            } else {
                if !digits.bytes().all(|b| b.is_ascii_digit()) {
                    return None;
                }
                digits.parse::<u16>().ok().filter(|p| *p != 0)?
            };
            (&hostport[..i], port)
        }
        _ => (hostport.as_str(), DEFAULT_PORT),
    };
    let host = match host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        Some(v6) => {
            v6.parse::<std::net::Ipv6Addr>().ok()?;
            v6
        }
        None => {
            let plain = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_');
            if host.is_empty() || !host.bytes().all(plain) {
                return None;
            }
            host
        }
    };
    Some(Proxy {
        host: host.to_string(),
        port,
        auth,
    })
}

/// A proxy URL's scheme when it is one urllib reads as a URL but the
/// binary doesn't carry (anything but `http`), in lower case:
/// `_splittype`'s scheme, the text before the first `:` with no `/` in
/// it, followed by `/`; one of a URL scheme's own bytes only.
pub fn other_scheme(raw: &str) -> Option<String> {
    let i = raw.find(':').filter(|&i| i > 0 && !raw[..i].contains('/'))?;
    let scheme = raw[..i].to_ascii_lowercase();
    let plain = scheme
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'.' | b'-'));
    (plain && raw[i + 1..].starts_with('/') && scheme != "http").then_some(scheme)
}

/// The line the binary prints, once, when it sends nothing because the
/// person's https proxy has a scheme it doesn't carry (the named
/// difference python-proxy-other-scheme: bash's Python sends a plain
/// CONNECT to that host). Only the scheme is named: the URL may hold the
/// proxy's password. `None` for every other setting.
pub fn unsupported_line(env: &Env) -> Option<String> {
    if matches!(env.no, Some(Err(NotText))) {
        return None;
    }
    let Some(Ok(raw)) = &env.https else {
        return None;
    };
    let scheme = other_scheme(raw)?;
    Some(format!(
        "Error: proxy {scheme}://\u{2026} is not supported: only http:// proxies are"
    ))
}

/// The route for an https request to the daemon whose URL's `host:port`
/// is `netloc` (`k2_direct` already said it is another machine).
pub fn decide(netloc: &str, env: &Env) -> Route {
    let raw = match &env.https {
        None => return Route::Direct,
        Some(Err(NotText)) => return Route::Shell,
        Some(Ok(raw)) => raw,
    };
    match &env.no {
        Some(Err(NotText)) => return Route::Shell,
        Some(Ok(no)) if bypass(netloc, no) => return Route::Direct,
        _ => {}
    }
    parse(raw).map_or(Route::Shell, Route::Via)
}

/// `k2_direct(url)` for `https://<host>`, `host` as [`super::super::remote::base`]
/// reads it (a name or an IPv4 address): `localhost`, a name under
/// `.localhost`, or a loopback address, in lower case less its trailing
/// dots.
pub fn direct(host: &str) -> bool {
    let h = host.to_ascii_lowercase();
    let h = h.trim_end_matches('.');
    h == "localhost"
        || h.ends_with(".localhost")
        || h.parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// `http.client`'s CONNECT head for `host:port` through `p`.
pub fn connect_head(p: &Proxy, host: &str, port: u16) -> Vec<u8> {
    let authority = format!("{host}:{port}");
    let mut h = format!("CONNECT {authority} HTTP/1.1\r\n");
    if let Some(c) = &p.auth {
        h.push_str(&format!("Proxy-Authorization: {}\r\n", c.basic()));
    }
    h.push_str(&format!("Host: {authority}\r\n\r\n"));
    h.into_bytes()
}
