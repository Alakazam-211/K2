//! URL, header and secret-shape checks for source definitions (PRD §5.1,
//! GV50, GV54) and the root-key hint (GV17).
//!
//! Pure: no network, no vault. The daemon runs the same origin rules again
//! at fetch time (`vault_fetch::Allowlist::for_source`), so these checks
//! are the friendly first line, not the only one.

use std::net::IpAddr;

use super::spec::{SpecError, SpecWarning};

/// One parsed fetch URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedUrl {
    pub url: url::Url,
    /// `scheme://host:port`, host lower-case (A-label), IPv6 in brackets.
    pub origin: String,
    /// The host is an IP literal (any form the URL parser accepts,
    /// `2130706433` and `0x7f000001` included).
    pub ip: Option<IpAddr>,
}

fn err(code: &'static str, field: &str, hint: impl Into<String>) -> SpecError {
    SpecError { code, field: field.to_string(), hint: hint.into() }
}

/// IPv4-mapped IPv6 as IPv4.
fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(IpAddr::V6(v6)),
        v4 => v4,
    }
}

/// Parse a fetch URL and derive its origin. Refuses userinfo, schemes
/// other than http(s), and plain `http://` to a host name (`https_only`).
/// An IP literal (any scheme) is a LAN source: `lan_owner_only` unless the
/// caller is the owner [GV54].
pub fn origin_of(raw: &str, field: &str, caller_is_owner: bool) -> Result<ParsedUrl, SpecError> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > 2048 {
        return Err(err("bad_url", field, "a URL is 1-2048 characters"));
    }
    let url = url::Url::parse(raw).map_err(|_| err("bad_url", field, "not a URL"))?;
    let https = match url.scheme() {
        "https" => true,
        "http" => false,
        _ => return Err(err("bad_url", field, "only https:// URLs (http:// only for an exact LAN IP the owner types)")),
    };
    if !url.username().is_empty() || url.password().is_some() {
        return Err(err("userinfo", field, "URLs can't carry a user name or password; put the credential in the vault"));
    }
    if url.fragment().is_some() {
        return Err(err("bad_url", field, "remove the #fragment"));
    }
    let ip = match url.host() {
        Some(url::Host::Ipv4(v4)) => Some(IpAddr::V4(v4)),
        Some(url::Host::Ipv6(v6)) => Some(canonical(IpAddr::V6(v6))),
        Some(url::Host::Domain(_)) => None,
        None => return Err(err("bad_url", field, "the URL has no host")),
    };
    if ip.is_none() && !https {
        return Err(err("https_only", field, "use https:// (http:// is only for an exact LAN IP the owner types)"));
    }
    if ip.is_some() && !caller_is_owner {
        return Err(err(
            "lan_owner_only",
            field,
            "an IP address or http:// source can only be added by the server's owner",
        ));
    }
    let port = url.port_or_known_default().ok_or_else(|| err("bad_url", field, "the URL has no port"))?;
    let host = match ip {
        Some(IpAddr::V6(v6)) => format!("[{v6}]"),
        Some(v4) => v4.to_string(),
        None => url.host_str().unwrap_or_default().trim_end_matches('.').to_ascii_lowercase(),
    };
    if host.is_empty() {
        return Err(err("bad_url", field, "the URL has no host"));
    }
    let origin = format!("{}://{}:{}", if https { "https" } else { "http" }, host, port);
    for (k, _) in url.query_pairs() {
        if secret_query_key(&k) {
            return Err(err(
                "secret_in_query",
                field,
                format!("the query key '{k}' looks like a secret: put the whole URL in the vault and use urlSecret"),
            ));
        }
    }
    Ok(ParsedUrl { url, origin, ip })
}

/// Query keys that carry credentials.
pub fn secret_query_key(k: &str) -> bool {
    let k = k.to_ascii_lowercase().replace('-', "_");
    matches!(
        k.as_str(),
        "token" | "key" | "apikey" | "api_key" | "access_token" | "secret" | "sig" | "signature" | "password" | "auth"
            | "client_secret" | "private_token"
    ) || k.ends_with("_token")
        || k.ends_with("_secret")
        || k.ends_with("_key")
        || k.ends_with("password")
}

/// A path segment of >= 24 base64url characters looks like a token
/// (Slack, Discord, Gatus badges) [GV50].
pub fn path_token_warning(u: &url::Url, field: &str) -> Option<SpecWarning> {
    let suspicious = u.path_segments().into_iter().flatten().any(|seg| {
        seg.len() >= 24
            && seg.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            && seg.bytes().any(|b| b.is_ascii_digit())
            && seg.bytes().any(|b| b.is_ascii_alphabetic())
    });
    suspicious.then(|| SpecWarning {
        code: "token_in_path",
        field: field.to_string(),
        hint: "part of this path looks like a token; if it is one, store the whole URL in the vault and use urlSecret"
            .into(),
    })
}

/// Headers K2 sets itself (routing, framing, conditional requests).
const ROUTING_HEADERS: &[&str] = &[
    "host",
    "content-length",
    "transfer-encoding",
    "connection",
    "keep-alive",
    "te",
    "trailer",
    "upgrade",
    "expect",
    "accept-encoding",
    "if-none-match",
    "if-modified-since",
    "forwarded",
    "x-forwarded-for",
    "x-forwarded-host",
    "x-forwarded-proto",
    "x-real-ip",
];

fn secret_header(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    matches!(n.as_str(), "authorization" | "proxy-authorization" | "cookie" | "x-api-key" | "api-key" | "apikey")
        || n.starts_with("x-auth-")
        || n.ends_with("-token")
        || n.ends_with("-key")
        || n.ends_with("-secret")
        || n.ends_with("-signature")
        || n.ends_with("-password")
}

/// One spec header: RFC 9110 name, no line breaks, not secret-shaped
/// (`secret_in_header`), not a header K2 owns (`forbidden_header`).
pub fn check_header(name: &str, value: &str, field: &str) -> Result<(), SpecError> {
    if name.is_empty()
        || name.len() > 100
        || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
    {
        return Err(err("forbidden_header", field, format!("'{name}' isn't a header name")));
    }
    if secret_header(name) {
        return Err(err(
            "secret_in_header",
            field,
            format!("{name} carries a credential: put it in the vault and pick it as the source's secret"),
        ));
    }
    let lower = name.to_ascii_lowercase();
    if ROUTING_HEADERS.contains(&lower.as_str()) || lower.starts_with("proxy-") {
        return Err(err("forbidden_header", field, format!("{name} is set by K2; remove it")));
    }
    if value.len() > 1024 || value.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0) {
        return Err(err("forbidden_header", field, format!("{name} has a line break or is too long")));
    }
    Ok(())
}

/// A JSON Pointer (RFC 6901): empty, or starting with `/`.
pub fn check_pointer(p: &str, field: &str) -> Result<(), SpecError> {
    if p.len() > 300 || !(p.is_empty() || p.starts_with('/')) {
        return Err(err("bad_pointer", field, format!("'{p}' isn't a JSON Pointer (it starts with /, like /data/0/id)")));
    }
    // `~` must be followed by 0 or 1.
    let b = p.as_bytes();
    for (i, c) in b.iter().enumerate() {
        if *c == b'~' && !matches!(b.get(i + 1), Some(b'0') | Some(b'1')) {
            return Err(err("bad_pointer", field, format!("'{p}': ~ must be ~0 or ~1")));
        }
    }
    Ok(())
}

/// A card's link: a plain http(s) URL with no query, no userinfo.
pub fn check_link(link: &str) -> Result<(), SpecError> {
    let bad = |h: &str| err("bad_link", "link", h.to_string());
    if link.len() > 500 {
        return Err(bad("a link is at most 500 characters"));
    }
    let u = url::Url::parse(link).map_err(|_| bad("not a URL"))?;
    if !matches!(u.scheme(), "https" | "http") {
        return Err(bad("links are http(s) only"));
    }
    if u.query().is_some() || u.fragment().is_some() || !u.username().is_empty() || u.password().is_some() {
        return Err(bad("a link has no query, fragment or user name"));
    }
    Ok(())
}

/// The URL without its query (for notes, logs and agent views).
pub fn without_query(raw: &str) -> String {
    match url::Url::parse(raw) {
        Ok(mut u) => {
            u.set_query(None);
            u.set_fragment(None);
            u.to_string()
        }
        Err(_) => "(invalid url)".into(),
    }
}

/// The URL with every query value replaced by `•••` (agents' view, GV50).
pub fn mask_query(raw: &str) -> String {
    match url::Url::parse(raw) {
        Ok(mut u) => {
            let keys: Vec<String> = u.query_pairs().map(|(k, _)| k.into_owned()).collect();
            if keys.is_empty() {
                return u.to_string();
            }
            u.set_query(None);
            {
                let mut q = u.query_pairs_mut();
                for k in &keys {
                    q.append_pair(k, "•••");
                }
            }
            u.to_string()
        }
        Err(_) => "(invalid url)".into(),
    }
}

/// A root-level key that should be a narrower one (GV17). Reads the value
/// inside the daemon only; returns fixed text.
pub fn root_key_hint(value: &str) -> Option<(&'static str, &'static str)> {
    let v = value.trim();
    let v = v.strip_prefix("Bearer ").unwrap_or(v);
    if v.starts_with("sk_live_") || v.starts_with("sk_test_") {
        return Some((
            "stripe_secret",
            "This is a full Stripe secret key. Make a restricted key with read access to what this card needs (rk_live_...).",
        ));
    }
    if v.starts_with("sb_secret_") || jwt_role(v).as_deref() == Some("service_role") {
        return Some(("supabase_service", "This key bypasses row security. Use a read-only role or view."));
    }
    if v.len() == 37 && v.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Some(("cloudflare_global", "This looks like a Cloudflare Global API Key. Use a scoped API token."));
    }
    if v.starts_with("ghp_") {
        return Some(("github_classic", "This is a classic GitHub token. Use a fine-grained, read-only token."));
    }
    None
}

/// The `role` claim of an unverified JWT, if `v` is one.
fn jwt_role(v: &str) -> Option<String> {
    use base64::Engine as _;
    let mut parts = v.split('.');
    let (_h, p, _s) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(p.trim_end_matches('=')).ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    claims.get("role")?.as_str().map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins_are_normalised_and_lan_needs_the_owner() {
        let p = origin_of("https://API.Example.com/v1/x?limit=5", "url", false).unwrap();
        assert_eq!(p.origin, "https://api.example.com:443");
        assert!(p.ip.is_none());
        assert_eq!(origin_of("http://example.com/", "url", true).unwrap_err().code, "https_only");
        assert_eq!(origin_of("https://u:p@example.com/", "url", true).unwrap_err().code, "userinfo");
        assert_eq!(origin_of("ftp://example.com/", "url", true).unwrap_err().code, "bad_url");
        for ip in [
            "http://169.254.169.254/latest/meta-data/",
            "http://127.0.0.1:9/",
            "https://127.0.0.1/",
            "http://[::1]/",
            "http://0.0.0.0/",
            "http://10.0.0.5/",
            "http://2130706433/",
            "http://0x7f000001/",
            "http://017700000001/",
            "http://[::ffff:127.0.0.1]/",
            "https://8.8.8.8/",
        ] {
            assert_eq!(origin_of(ip, "url", false).unwrap_err().code, "lan_owner_only", "{ip}");
            let p = origin_of(ip, "url", true).unwrap_or_else(|e| panic!("{ip}: {e:?}"));
            assert!(p.ip.is_some(), "{ip}");
        }
        assert_eq!(origin_of("http://2130706433/", "url", true).unwrap().origin, "http://127.0.0.1:80");
        assert_eq!(origin_of("http://[::ffff:127.0.0.1]/", "url", true).unwrap().origin, "http://127.0.0.1:80");
        assert_eq!(origin_of("http://[::1]:8080/", "url", true).unwrap().origin, "http://[::1]:8080");
    }

    #[test]
    fn secret_shaped_queries_headers_and_paths() {
        for q in ["token", "api_key", "apiKey", "access_token", "sig", "password", "client_secret", "x-api-key"] {
            let e = origin_of(&format!("https://x.example/?{q}=abc"), "url", true).unwrap_err();
            assert_eq!(e.code, "secret_in_query", "{q}");
        }
        assert!(origin_of("https://x.example/?teamId=t1&limit=5", "url", false).is_ok());
        for h in ["Authorization", "cookie", "X-API-Key", "api-key", "X-Auth-Email", "X-Vercel-Token", "Stripe-Key"] {
            assert_eq!(check_header(h, "v", "headers[0]").unwrap_err().code, "secret_in_header", "{h}");
        }
        for h in ["Host", "Content-Length", "If-None-Match", "Proxy-Connection", "X-Forwarded-For"] {
            assert_eq!(check_header(h, "v", "headers[0]").unwrap_err().code, "forbidden_header", "{h}");
        }
        assert_eq!(check_header("X-A", "a\r\nb", "h").unwrap_err().code, "forbidden_header");
        check_header("Accept", "application/json", "h").unwrap();
        let u = url::Url::parse("https://hooks.example.com/services/T0000/B0000/abcdefghijklmnopqrstuvwx1234").unwrap();
        assert_eq!(path_token_warning(&u, "url").unwrap().code, "token_in_path");
        let u = url::Url::parse("https://api.github.com/repos/Alakazam-211/K2/releases/latest").unwrap();
        assert!(path_token_warning(&u, "url").is_none());
    }

    #[test]
    fn pointers_links_and_masks() {
        check_pointer("", "p").unwrap();
        check_pointer("/a/0/b~1c", "p").unwrap();
        assert_eq!(check_pointer("a/b", "p").unwrap_err().code, "bad_pointer");
        assert_eq!(check_pointer("/a~2", "p").unwrap_err().code, "bad_pointer");
        check_link("https://k2.dev/admin/fleet").unwrap();
        assert_eq!(check_link("https://k2.dev/?x=1").unwrap_err().code, "bad_link");
        assert_eq!(without_query("https://a.example/p?teamId=x"), "https://a.example/p");
        assert_eq!(mask_query("https://a.example/p?teamId=x&limit=5"), "https://a.example/p?teamId=%E2%80%A2%E2%80%A2%E2%80%A2&limit=%E2%80%A2%E2%80%A2%E2%80%A2");
    }

    #[test]
    fn root_key_hints() {
        assert_eq!(root_key_hint("sk_live_abc").unwrap().0, "stripe_secret");
        assert_eq!(root_key_hint("Bearer sk_test_abc").unwrap().0, "stripe_secret");
        assert!(root_key_hint("rk_live_abc").is_none());
        assert_eq!(root_key_hint("sb_secret_xyz").unwrap().0, "supabase_service");
        use base64::Engine as _;
        let p = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"role":"service_role"}"#);
        assert_eq!(root_key_hint(&format!("eyJhbGciOiJIUzI1NiJ9.{p}.sig")).unwrap().0, "supabase_service");
        assert_eq!(root_key_hint(&"a".repeat(37)).unwrap().0, "cloudflare_global");
        assert_eq!(root_key_hint("ghp_xxx").unwrap().0, "github_classic");
        assert!(root_key_hint("github_pat_xxx").is_none());
    }
}
