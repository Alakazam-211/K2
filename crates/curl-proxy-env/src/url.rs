//! A proxy URL, read as the curl command reads one:
//! `[scheme://][user[:password]@]host[:port][/path][?query][#fragment]`.

use crate::error::ConfigError;
use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};

/// The kind of proxy, from the URL's scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Scheme {
    /// `http://`, or no scheme: an HTTP proxy, a CONNECT tunnel.
    Http,
    /// `https://`: TLS to the proxy, then a CONNECT tunnel inside it.
    Https,
    /// `socks4://`: SOCKS4; the target's name is resolved here.
    Socks4,
    /// `socks4a://`: SOCKS4a; the proxy resolves the target's name.
    Socks4a,
    /// `socks5://`: SOCKS5; the target's name is resolved here.
    Socks5,
    /// `socks5h://`: SOCKS5; the proxy resolves the target's name.
    Socks5h,
}

impl Scheme {
    /// The scheme as written in a URL, lower case.
    pub fn as_str(self) -> &'static str {
        match self {
            Scheme::Http => "http",
            Scheme::Https => "https",
            Scheme::Socks4 => "socks4",
            Scheme::Socks4a => "socks4a",
            Scheme::Socks5 => "socks5",
            Scheme::Socks5h => "socks5h",
        }
    }

    /// The port when the URL names none: 443 for `https://`, else 1080.
    pub fn default_port(self) -> u16 {
        match self {
            Scheme::Https => 443,
            _ => 1080,
        }
    }

    fn from_name(name: &str) -> Option<Scheme> {
        Some(match name {
            "http" => Scheme::Http,
            "https" => Scheme::Https,
            "socks4" => Scheme::Socks4,
            "socks4a" => Scheme::Socks4a,
            "socks5" => Scheme::Socks5,
            "socks5h" => Scheme::Socks5h,
            _ => return None,
        })
    }
}

impl fmt::Display for Scheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A host: a name, an IPv4 address written as four numbers, or an IPv6
/// address (in brackets in a URL).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Host {
    /// A name, as written.
    Name(String),
    /// An IPv4 address.
    V4(Ipv4Addr),
    /// An IPv6 address.
    V6(Ipv6Addr),
}

impl Host {
    /// A URL's host: `[v6]` or a bare IPv6 address, four numbers, or a
    /// name (anything else, as written).
    pub fn parse(text: &str) -> Host {
        let unbracketed = text
            .strip_prefix('[')
            .and_then(|t| t.strip_suffix(']'))
            .unwrap_or(text);
        if let Ok(v6) = unbracketed.parse::<Ipv6Addr>() {
            return Host::V6(v6);
        }
        if let Ok(v4) = text.parse::<Ipv4Addr>() {
            return Host::V4(v4);
        }
        Host::Name(text.to_string())
    }
}

impl fmt::Display for Host {
    /// As in a URL's authority: an IPv6 address in brackets.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Host::Name(n) => f.write_str(n),
            Host::V4(a) => write!(f, "{a}"),
            Host::V6(a) => write!(f, "[{a}]"),
        }
    }
}

/// The proxy's user and password, percent-decoded. Printed as
/// `<redacted>` only: they go to the proxy alone, in its handshake.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Credentials {
    user: Vec<u8>,
    password: Vec<u8>,
}

impl Credentials {
    /// A user and password, as bytes.
    pub fn new(user: impl Into<Vec<u8>>, password: impl Into<Vec<u8>>) -> Credentials {
        Credentials {
            user: user.into(),
            password: password.into(),
        }
    }

    /// The user, decoded.
    pub fn user(&self) -> &[u8] {
        &self.user
    }

    /// The password, decoded; empty when the URL gave none.
    pub fn password(&self) -> &[u8] {
        &self.password
    }

    /// `Basic <base64 of user:password>`, for `Proxy-Authorization`.
    pub(crate) fn basic(&self) -> String {
        let mut raw = self.user.clone();
        raw.push(b':');
        raw.extend_from_slice(&self.password);
        format!("Basic {}", base64(&raw))
    }
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Credentials(<redacted>)")
    }
}

/// A proxy, read from its URL ([`ProxyUrl::parse`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProxyUrl {
    scheme: Scheme,
    host: Host,
    port: u16,
    credentials: Option<Credentials>,
}

impl ProxyUrl {
    /// A proxy from its parts.
    pub fn new(scheme: Scheme, host: Host, port: u16, credentials: Option<Credentials>) -> Self {
        ProxyUrl {
            scheme,
            host,
            port,
            credentials,
        }
    }

    /// The kind of proxy.
    pub fn scheme(&self) -> Scheme {
        self.scheme
    }

    /// The proxy's host.
    pub fn host(&self) -> &Host {
        &self.host
    }

    /// The proxy's port (the scheme's default when the URL names none).
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The user and password from the URL, if it has a user part.
    pub fn credentials(&self) -> Option<&Credentials> {
        self.credentials.as_ref()
    }

    /// Reads a proxy URL, as bytes (an environment value need not be
    /// UTF-8):
    ///
    /// - a blank or control byte anywhere is [`ConfigError::BadSyntax`];
    /// - the scheme, in any case, is `http`, `https`, `socks4`,
    ///   `socks4a`, `socks5` or `socks5h`; any other is
    ///   [`ConfigError::UnsupportedScheme`]. With no scheme it is
    ///   `http`, unless the host starts `ftp.`, `dict.`, `ldap.`,
    ///   `imap.`, `smtp.` or `pop3.` (any case), which curl reads as
    ///   that protocol, so unsupported too;
    /// - the authority ends at the first `/`, `?` or `#`; what follows is
    ///   not used;
    /// - the user part runs to the last `@`; the user ends at its first
    ///   `:`, and both are percent-decoded (a `%` not followed by two hex
    ///   digits is kept as it is);
    /// - the host is a name of letters, digits, `-`, `.` and `_`, four
    ///   numbers, or an IPv6 address in brackets;
    /// - the port is digits, 1 to 65535; none (or an empty one) is the
    ///   scheme's default ([`Scheme::default_port`]).
    pub fn parse(raw: &[u8]) -> Result<ProxyUrl, ConfigError> {
        if raw.iter().any(|&b| b <= b' ' || b == 0x7f) {
            return Err(ConfigError::BadSyntax("a blank or control byte"));
        }
        let (scheme_name, rest) = match find(raw, b"://") {
            Some(i) if is_scheme(&raw[..i]) => (Some(lower(&raw[..i])), &raw[i + 3..]),
            _ => (None, raw),
        };
        let end = rest
            .iter()
            .position(|b| matches!(b, b'/' | b'?' | b'#'))
            .unwrap_or(rest.len());
        let authority = &rest[..end];
        let (userinfo, hostport) = match authority.iter().rposition(|&b| b == b'@') {
            Some(i) => (Some(&authority[..i]), &authority[i + 1..]),
            None => (None, authority),
        };
        // The whole URL is read before its scheme is judged.
        let (host_text, port) = split_port(hostport)?;
        let host = read_host(host_text)?;
        let scheme = match scheme_name {
            Some(name) => match Scheme::from_name(&name) {
                Some(s) => s,
                None => return Err(ConfigError::UnsupportedScheme(name)),
            },
            None => match guessed(host_text) {
                Some(g) => return Err(ConfigError::UnsupportedScheme(g.to_string())),
                None => Scheme::Http,
            },
        };
        let port = port.unwrap_or(scheme.default_port());
        let credentials = userinfo.map(|u| {
            let (user, password) = match u.iter().position(|&b| b == b':') {
                Some(i) => (&u[..i], &u[i + 1..]),
                None => (u, &b""[..]),
            };
            Credentials::new(percent_decode(user), percent_decode(password))
        });
        Ok(ProxyUrl {
            scheme,
            host,
            port,
            credentials,
        })
    }
}

impl fmt::Display for ProxyUrl {
    /// `scheme://host:port`, never the user part.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}://{}:{}", self.scheme, self.host, self.port)
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// RFC 3986's scheme: a letter, then letters, digits, `+`, `-` and `.`.
fn is_scheme(s: &[u8]) -> bool {
    match s.split_first() {
        Some((first, rest)) => {
            first.is_ascii_alphabetic()
                && rest
                    .iter()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
        }
        None => false,
    }
}

fn lower(s: &[u8]) -> String {
    String::from_utf8_lossy(s).to_ascii_lowercase()
}

/// The protocols curl guesses for a scheme-less URL by its host's first
/// label (the curl manual, URL).
const GUESSED: [(&str, &str); 6] = [
    ("ftp.", "ftp"),
    ("dict.", "dict"),
    ("ldap.", "ldap"),
    ("imap.", "imap"),
    ("smtp.", "smtp"),
    ("pop3.", "pop3"),
];

fn guessed(host: &[u8]) -> Option<&'static str> {
    GUESSED.iter().find_map(|(prefix, scheme)| {
        (host.len() >= prefix.len() && host[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes()))
            .then_some(*scheme)
    })
}

/// The host and the port of `host[:port]` or `[v6][:port]`.
fn split_port(hp: &[u8]) -> Result<(&[u8], Option<u16>), ConfigError> {
    let (host, port) = if hp.first() == Some(&b'[') {
        let close = hp
            .iter()
            .position(|&b| b == b']')
            .ok_or(ConfigError::BadSyntax("an IPv6 address with no ]"))?;
        match &hp[close + 1..] {
            [] => (&hp[..=close], None),
            [b':', port @ ..] => (&hp[..=close], Some(port)),
            _ => return Err(ConfigError::BadSyntax("bytes after an IPv6 address")),
        }
    } else {
        match hp.iter().rposition(|&b| b == b':') {
            Some(i) => (&hp[..i], Some(&hp[i + 1..])),
            None => (hp, None),
        }
    };
    let port = match port {
        None | Some([]) => None,
        Some(digits) => {
            if !digits.iter().all(u8::is_ascii_digit) {
                return Err(ConfigError::BadSyntax("a port that is not a number"));
            }
            // At most 5 digits after any leading zeros.
            let digits = &digits[digits.iter().take_while(|&&b| b == b'0').count()..];
            let n: u32 = if digits.len() > 5 {
                u32::MAX
            } else {
                digits.iter().fold(0, |n, d| n * 10 + u32::from(d - b'0'))
            };
            match u16::try_from(n) {
                Ok(p) if p != 0 => Some(p),
                _ => return Err(ConfigError::BadSyntax("a port out of range")),
            }
        }
    };
    Ok((host, port))
}

fn read_host(h: &[u8]) -> Result<Host, ConfigError> {
    if h.is_empty() {
        return Err(ConfigError::BadSyntax("no host"));
    }
    if let Some(inner) = h.strip_prefix(b"[").and_then(|i| i.strip_suffix(b"]")) {
        let text = std::str::from_utf8(inner)
            .map_err(|_| ConfigError::BadSyntax("an IPv6 address that doesn't parse"))?;
        return text
            .parse::<Ipv6Addr>()
            .map(Host::V6)
            .map_err(|_| ConfigError::BadSyntax("an IPv6 address that doesn't parse"));
    }
    let plain = |b: &u8| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_');
    if !h.iter().all(plain) {
        return Err(ConfigError::BadSyntax("a byte a host name can't hold"));
    }
    // ASCII only, so this is text.
    let text = String::from_utf8_lossy(h).into_owned();
    Ok(match text.parse::<Ipv4Addr>() {
        Ok(v4) => Host::V4(v4),
        Err(_) => Host::Name(text),
    })
}

/// `%XX` to its byte; a `%` that isn't followed by two hex digits stays.
fn percent_decode(s: &[u8]) -> Vec<u8> {
    let hex = |b: u8| (b as char).to_digit(16);
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i] == b'%' && i + 2 < s.len() {
            if let (Some(h), Some(l)) = (hex(s[i + 1]), hex(s[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(s[i]);
        i += 1;
    }
    out
}

/// Standard base64 with padding.
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

#[cfg(test)]
mod tests;
