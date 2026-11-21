//! Where clone sends a token or a password: the shell's `_CLONE_URL_RULE`
//! (`cli/src/clone.sh`), word for word.
//!
//! A token goes to an `https://` server, or over plain http only to a
//! loopback or Tailscale address as written in the URL (127.0.0.0/8,
//! `::1` or the name `localhost`; 100.64.0.0/10 or fd7a:115c:a1e0::/48).
//! A name is never looked up to decide. A password goes over https only.
//! An address with anything before an `@` in its host part is refused
//! whatever its scheme, and the refusal never shows what came before it.
//! The host is read as Python's `urlsplit(url).hostname` reads it (lower
//! case, brackets off).

use k2_client::product_name;
use std::net::{Ipv4Addr, Ipv6Addr};

/// `urlsplit(url)`'s parts the rule reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Split {
    pub scheme: String,
    pub netloc: String,
    pub path: String,
}

impl Split {
    /// `u.hostname`: the netloc less any user part and port, brackets off,
    /// in lower case; `None` when empty.
    pub fn hostname(&self) -> Option<String> {
        let host = self
            .netloc
            .rsplit_once('@')
            .map_or(&*self.netloc, |(_, h)| h);
        let host = if let Some(rest) = host.strip_prefix('[') {
            rest.split_once(']').map_or(rest, |(h, _)| h)
        } else {
            host.split_once(':').map_or(host, |(h, _)| h)
        };
        (!host.is_empty()).then(|| host.to_lowercase())
    }
}

/// Python's `urllib.parse.urlsplit`, for the parts the rule reads. `None`:
/// it raises `ValueError` (a bracket with no partner, or a bracketed host
/// that is not an IPv6 address).
pub fn urlsplit(url: &str) -> Option<Split> {
    let mut rest = url;
    let mut scheme = String::new();
    if let Some((s, after)) = url.split_once(':') {
        let ok = s.bytes().next().is_some_and(|b| b.is_ascii_alphabetic())
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'));
        if ok {
            scheme = s.to_ascii_lowercase();
            rest = after;
        }
    }
    let mut netloc = String::new();
    if let Some(after) = rest.strip_prefix("//") {
        let end = after.find(['/', '?', '#']).unwrap_or(after.len());
        netloc = after[..end].to_string();
        rest = &after[end..];
        let open = netloc.contains('[');
        let close = netloc.contains(']');
        if open != close {
            return None;
        }
        if open {
            let host = netloc.rsplit_once('@').map_or(&*netloc, |(_, h)| h);
            let inner = host
                .strip_prefix('[')
                .and_then(|h| h.split_once(']'))
                .map(|(h, _)| h)?;
            let addr = inner.split_once('%').map_or(inner, |(a, _)| a);
            if !inner.starts_with('v') && addr.parse::<Ipv6Addr>().is_err() {
                return None;
            }
        }
    }
    let path = rest
        .split_once(['?', '#'])
        .map_or(rest, |(p, _)| p)
        .to_string();
    Some(Split {
        scheme,
        netloc,
        path,
    })
}

/// `clone_loopback_literal`: `localhost`, or an address in 127.0.0.0/8
/// or `::1`, as written.
pub fn loopback_literal(host: &str) -> bool {
    if host == "localhost" {
        return true;
    }
    if let Ok(a) = host.parse::<Ipv4Addr>() {
        return a.octets()[0] == 127;
    }
    host.parse::<Ipv6Addr>()
        .is_ok_and(|a| a == Ipv6Addr::LOCALHOST)
}

/// `clone_tailnet_literal`: an address in 100.64.0.0/10 or
/// fd7a:115c:a1e0::/48, as written (never a name).
pub fn tailnet_literal(host: &str) -> bool {
    if let Ok(a) = host.parse::<Ipv4Addr>() {
        let o = a.octets();
        return o[0] == 100 && (64..128).contains(&o[1]);
    }
    host.parse::<Ipv6Addr>().is_ok_and(|a| {
        let s = a.segments();
        s[0] == 0xfd7a && s[1] == 0x115c && s[2] == 0xa1e0
    })
}

/// `clone_plain_http_ok`.
pub fn plain_http_ok(host: &str) -> bool {
    loopback_literal(host) || tailnet_literal(host)
}

/// `clone_local_warns`: this server's owner token goes over plain http to
/// `host` (`HOST`, as written) and it is not this computer, a Tailscale
/// address or a `*.ts.net` name.
pub fn local_warns(host: &str) -> bool {
    let h = host.to_lowercase();
    let h = h
        .strip_prefix('[')
        .and_then(|x| x.strip_suffix(']'))
        .unwrap_or(&h);
    !(loopback_literal(h) || tailnet_literal(h) || h.ends_with(".ts.net"))
}

/// `clone_url_refusal(url, password)`: `None` when a token (with
/// `password`, a password) may go to `url`, else the line that refuses it.
pub fn url_refusal(url: &str, password: bool) -> Option<String> {
    let u = urlsplit(url);
    let host = u.as_ref().and_then(Split::hostname).unwrap_or_default();
    let after_at = |s: &str| s.rsplit_once('@').map_or(s, |(_, h)| h).to_string();
    let shown = u
        .as_ref()
        .map(|u| after_at(&u.netloc))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| after_at(url));
    let p = product_name!();
    if let Some(u) = &u {
        if u.netloc.contains('@') {
            return Some(format!(
                "refused: the address for {shown} has a name before an @, so {p} won't send your token or password there; give the server's address alone"
            ));
        }
        if u.scheme == "https" {
            return None;
        }
        if u.scheme == "http" && !password && plain_http_ok(&host) {
            return None;
        }
    }
    Some(format!(
        "refused: {shown} isn't https, so {p} won't send your token or password there; use https://{shown}"
    ))
}

/// Where the remote server is: its URL read as urllib reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub tls: bool,
    /// The host to connect to (an IPv6 address without its brackets).
    pub host: String,
    pub port: u16,
    /// The URL's netloc as written: urllib's `Host` header.
    pub netloc: String,
    /// The URL's path, put before each route (`base + route`).
    pub prefix: String,
}

/// `url` (`http://` or `https://`, no user part) as urllib sends to it.
/// `None`: a form the binary doesn't send to itself (a query or fragment,
/// a port that is not a number, a byte outside printable ASCII).
pub fn place(url: &str) -> Option<Place> {
    if !url.bytes().all(|b| (0x21..0x7f).contains(&b)) || url.contains(['?', '#', '%', '\\']) {
        return None;
    }
    let u = urlsplit(url)?;
    let tls = match u.scheme.as_str() {
        "https" => true,
        "http" => false,
        _ => return None,
    };
    if u.netloc.is_empty() || u.netloc.contains('@') {
        return None;
    }
    let (host, port) = if let Some(rest) = u.netloc.strip_prefix('[') {
        let (h, after) = rest.split_once(']')?;
        (h.to_string(), after.strip_prefix(':').unwrap_or(after))
    } else {
        match u.netloc.split_once(':') {
            Some((h, p)) => (h.to_string(), p),
            None => (u.netloc.clone(), ""),
        }
    };
    let port = if port.is_empty() {
        if tls {
            443
        } else {
            80
        }
    } else {
        if !port.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        port.parse().ok()?
    };
    if host.is_empty() {
        return None;
    }
    Some(Place {
        tls,
        host,
        port,
        netloc: u.netloc,
        prefix: u.path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rule() {
        assert_eq!(url_refusal("https://h.example", false), None);
        assert_eq!(url_refusal("http://127.0.0.9:4", false), None);
        assert_eq!(url_refusal("http://LOCALHOST:4", false), None);
        assert_eq!(url_refusal("http://[::1]:4", false), None);
        assert_eq!(url_refusal("http://100.100.1.2:4", false), None);
        assert!(url_refusal("http://127.0.0.1:4", true).is_some());
        assert!(url_refusal("http://x.ts.net", false).is_some());
        let line = url_refusal("http://h.example:8", false).unwrap();
        assert!(line.contains("refused: h.example:8 isn't https"), "{line}");
        let at = url_refusal("https://ann:pw@h.example", false).unwrap();
        assert!(
            at.contains("for h.example has a name") && !at.contains("ann"),
            "{at}"
        );
        assert!(urlsplit("http://[::1").is_none());
        assert!(local_warns("h.example"));
        assert!(!local_warns("[FD7A:115C:A1E0::1]"));
        assert!(!local_warns("x.TS.net"));
    }
}
