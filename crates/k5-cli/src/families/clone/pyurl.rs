//! Where the clone engine's Python sends a request: `urllib.request.
//! Request(base + route + query)` and the `http.client` connection
//! urllib opens for it, read the way Python reads the URL. Both sides go
//! through it: this server's `BASE_URL` (`http://HOST:PORT`, or
//! `https://<name>:PORT` for `K2_HOST=https://<name>`, an IPv6 address
//! as the shell writes it) and the remote server's URL as typed (a query,
//! a fragment, a `%` escape, a port that is not a number).
//!
//! What Python does, step by step:
//!
//! - the fragment is cut at the LAST `#` (`_splittag`), route and query
//!   with it when they come after one;
//! - the scheme is the text before the first `:` (`_splittype`), lower
//!   case; `https` is TLS, anything else plain;
//! - the host is what follows `//` up to the first `/`, `?` or `#`
//!   (`_splithost`), `%` escapes decoded; no `//`, or nothing there, is
//!   `URLError('no host given')`;
//! - the request line's path is the rest, with a `/` put in front when it
//!   doesn't start with one (`/` when there is none);
//! - the `Host` header is the decoded host as written, port and all;
//! - `http.client` takes the port after the last `:` when it comes after
//!   the last `]`, read by `int()` (a sign, `_` between digits); empty is
//!   the scheme's own; anything else is `InvalidURL('nonnumeric port:
//!   ...')`, raised out of `urlopen` (a traceback). Brackets around the
//!   host are dropped for the connect and the certificate's name.
//!
//! What the binary refuses instead (K2's ruling R10, a clear usage line,
//! nothing sent; the shell's Python would try): a byte outside printable
//! ASCII or a blank anywhere in the URL or the decoded host, an empty
//! host before a port, and a port `int()` reads that no socket takes
//! (above 65535, or below 0).

/// Where one request goes, as `http.client` connects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// `https`.
    pub tls: bool,
    /// The host to connect to and name in TLS: brackets off.
    pub host: String,
    pub port: u16,
    /// urllib's `Host` header: the decoded host as written.
    pub host_header: String,
    /// The request line's path, query and all.
    pub selector: String,
}

/// Why a URL gets no connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Miss {
    /// `URLError('no host given')`: the engine's `daemon_unreachable`.
    NoHost,
    /// An exception out of `urlopen`, as `Class: message`.
    Raised(String),
    /// A form the binary refuses (R10).
    Unsendable,
}

fn printable(s: &str) -> bool {
    s.bytes().all(|b| (0x21..0x7f).contains(&b))
}

/// `urllib.parse.unquote(s)`: each `%XX` as its byte, the bytes read as
/// UTF-8 (U+FFFD for what isn't), anything else as it is.
fn unquote(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = &s[i + 1..i + 3];
            if hex.bytes().all(|c| c.is_ascii_hexdigit()) {
                out.push(u8::from_str_radix(hex, 16).unwrap_or(0));
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Python's `int(text)` for ASCII text with no blanks: a sign, then
/// digits with single `_` between them. `None`: `ValueError`. A value
/// past `i64` reads as `i64::MAX` (no socket takes it either way).
fn py_int(text: &str) -> Option<i64> {
    let (neg, d) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    if d.is_empty()
        || d.starts_with('_')
        || d.ends_with('_')
        || d.contains("__")
        || !d.bytes().all(|b| b.is_ascii_digit() || b == b'_')
    {
        return None;
    }
    let digits: String = d.chars().filter(|c| *c != '_').collect();
    let v = digits.parse::<i64>().unwrap_or(i64::MAX);
    Some(if neg { -v } else { v })
}

/// `url` as urllib and `http.client` send to it.
pub fn target(url: &str) -> Result<Target, Miss> {
    if !printable(url) {
        return Err(Miss::Unsendable);
    }
    // `_splittag`: the fragment after the last `#`.
    let url = url.rsplit_once('#').map_or(url, |(u, _)| u);
    // `_splittype`: `([^/:]+):(.*)`.
    let (scheme, rest) = match url.find([':', '/']) {
        Some(i) if i > 0 && url.as_bytes()[i] == b':' => {
            (url[..i].to_ascii_lowercase(), &url[i + 1..])
        }
        _ => return Err(Miss::Raised(format!("ValueError: unknown url type: '{url}'"))),
    };
    // `_splithost`: `//([^/#?]*)(.*)`.
    let Some(after) = rest.strip_prefix("//") else {
        return Err(Miss::NoHost);
    };
    let end = after.find(['/', '#', '?']).unwrap_or(after.len());
    let (host_port, path) = after.split_at(end);
    let selector = if path.is_empty() {
        "/".to_string()
    } else if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    let host_header = unquote(host_port);
    if host_header.is_empty() {
        return Err(Miss::NoHost);
    }
    if !printable(&host_header) {
        return Err(Miss::Unsendable);
    }
    let tls = scheme == "https";
    let default = if tls { 443 } else { 80 };
    // `http.client`'s `_get_hostport`.
    let mut host = host_header.as_str();
    let mut port = default;
    let colon = host.rfind(':');
    let bracket = host.rfind(']');
    if let Some(i) = colon.filter(|i| bracket.is_none_or(|j| *i > j)) {
        let text = &host[i + 1..];
        if !text.is_empty() {
            match py_int(text) {
                Some(n) => port = u16::try_from(n).map_err(|_| Miss::Unsendable)?,
                None => {
                    return Err(Miss::Raised(format!(
                        "http.client.InvalidURL: nonnumeric port: '{text}'"
                    )))
                }
            }
        }
        host = &host[..i];
    }
    if let Some(inner) = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        host = inner;
    }
    if host.is_empty() {
        return Err(Miss::Unsendable);
    }
    Ok(Target {
        tls,
        host: host.to_string(),
        port,
        host_header: host_header.clone(),
        selector,
    })
}

/// Would any request to `base` be refused by the binary (R10)? Checked
/// once, before anything is sent: a route the engine adds never makes a
/// sendable base unsendable.
pub fn unsendable(base: &str) -> bool {
    matches!(target(&format!("{base}/cli")), Err(Miss::Unsendable))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(url: &str) -> (bool, &'static str, u16, String, String) {
        let x = target(url).unwrap_or_else(|m| panic!("{url}: {m:?}"));
        let host: &'static str = Box::leak(x.host.into_boxed_str());
        (x.tls, host, x.port, x.host_header, x.selector)
    }

    #[test]
    fn as_urllib_reads_it() {
        assert_eq!(
            t("https://h.example/cli/x?a=1"),
            (true, "h.example", 443, "h.example".into(), "/cli/x?a=1".into())
        );
        assert_eq!(
            t("http://127.0.0.1:04100/cli/x"),
            (false, "127.0.0.1", 4100, "127.0.0.1:04100".into(), "/cli/x".into())
        );
        // A query in the base: the route rides in it.
        assert_eq!(
            t("https://h.example?x=1/cli/vault/recipient").4,
            "/?x=1/cli/vault/recipient"
        );
        // A fragment: cut at the last `#`, route and all.
        assert_eq!(t("https://h.example/#a/cli/x?q=1").4, "/");
        assert_eq!(t("https://h.example/a#b/c#d").4, "/a#b/c");
        // `%` escapes: decoded in the host and the Host header only.
        assert_eq!(
            t("https://h%2Eexample:8443/p%41/cli"),
            (true, "h.example", 8443, "h.example:8443".into(), "/p%41/cli".into())
        );
        // Ports as `int()` reads them; an empty one is the scheme's.
        assert_eq!(t("https://h.example:+44_3/cli").2, 443);
        assert_eq!(t("https://h.example:/cli").2, 443);
        assert_eq!(t("http://h.example:/cli").3, "h.example:");
        // IPv6, bracketed or as the shell writes K2_HOST=::1.
        assert_eq!(t("http://[::1]:9/cli").1, "::1");
        assert_eq!(
            t("http://::1:4100/cli"),
            (false, "::1", 4100, "::1:4100".into(), "/cli".into())
        );
        assert_eq!(t("https://[fd00::1]/cli").2, 443);
    }

    #[test]
    fn what_python_raises_and_what_the_binary_refuses() {
        assert_eq!(
            target("https://h.example:abc/cli"),
            Err(Miss::Raised(
                "http.client.InvalidURL: nonnumeric port: 'abc'".into()
            ))
        );
        assert_eq!(
            target("https://h.example:4_/cli"),
            Err(Miss::Raised(
                "http.client.InvalidURL: nonnumeric port: '4_'".into()
            ))
        );
        assert_eq!(target("https:/cli/x"), Err(Miss::NoHost));
        assert_eq!(target("https:///cli/x"), Err(Miss::NoHost));
        assert_eq!(target("https://h.example:70000/cli"), Err(Miss::Unsendable));
        assert_eq!(target("https://h.example:-1/cli"), Err(Miss::Unsendable));
        assert_eq!(target("https://h example/cli"), Err(Miss::Unsendable));
        assert_eq!(target("https://h%20x/cli"), Err(Miss::Unsendable));
        assert_eq!(target("https://h\u{e9}.example/cli"), Err(Miss::Unsendable));
        assert!(unsendable("https://h.example:99999"));
        assert!(!unsendable("https://h.example:abc"));
        assert!(!unsendable("https://h.example/?x#y"));
    }
}
