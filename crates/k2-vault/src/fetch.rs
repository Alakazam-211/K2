//! Fetch request shaping and policy (no I/O; the daemon's egress client
//! makes the call). §9.2 as amended by VS3–VS6:
//!
//! - The URL's origin must match one of the secret's allowed hosts
//!   (narrowed by the grant).
//! - The value goes only where the secret's sealed `inject` says.
//! - The agent can't set `Host`, framing or hop-by-hop headers; header
//!   names are RFC 9110 tokens; values have no CR/LF/NUL.
//! - Redirects are followed only to the same origin, never https→http.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;

use crate::error::{Result, VaultError};
use crate::hosts::{Origin, OriginPattern};
use crate::inject::{is_forbidden_header, validate_header_name, Dst};
use crate::secret::SecretBytes;

pub const MAX_INLINE_BODY: usize = 64 * 1024;
pub const DEFAULT_MAX_RESPONSE: usize = 10 * 1024 * 1024;
pub const MAX_RESPONSE: usize = 100 * 1024 * 1024;
pub const MAX_REDIRECTS: usize = 5;
pub const BODY_MARKER: &str = "{{secret}}";

pub const METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

/// Response headers returned without `--include-headers`.
pub const RESPONSE_HEADER_ALLOWLIST: &[&str] = &[
    "content-type",
    "content-length",
    "date",
    "etag",
    "last-modified",
    "location",
    "retry-after",
    "x-request-id",
    "request-id",
    "x-ratelimit-limit",
    "x-ratelimit-remaining",
    "x-ratelimit-reset",
    "ratelimit-limit",
    "ratelimit-remaining",
    "ratelimit-reset",
];

pub fn validate_method(m: &str) -> Result<String> {
    let up = m.trim().to_ascii_uppercase();
    if METHODS.contains(&up.as_str()) {
        Ok(up)
    } else {
        Err(VaultError::bad_request("bad_method", format!("method {m} isn't supported")))
    }
}

/// Agent-supplied request headers.
pub fn validate_agent_header(name: &str, value: &str) -> Result<()> {
    validate_header_name(name)?;
    if is_forbidden_header(name) {
        return Err(VaultError::bad_request(
            "header_not_allowed",
            format!("{name} is set by K2 from the URL; remove it"),
        ));
    }
    if value.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0) {
        return Err(VaultError::bad_request("header_not_allowed", format!("{name} has a line break")));
    }
    if value.len() > 8 * 1024 {
        return Err(VaultError::bad_request("header_not_allowed", format!("{name} is too long")));
    }
    Ok(())
}

/// The URL's origin must match a pattern in `allowed`.
pub fn check_host(origin: &Origin, allowed: &[OriginPattern]) -> Result<()> {
    if allowed.is_empty() {
        return Err(VaultError::forbidden(
            "no_allowed_hosts",
            "this secret isn't bound to any site; add one in Settings → Vault",
        ));
    }
    if allowed.iter().any(|p| p.matches(origin)) {
        Ok(())
    } else {
        Err(VaultError::forbidden(
            "host_not_allowed",
            format!("this secret may only be sent to its allowed sites, not {}", origin.render()),
        ))
    }
}

/// Follow a redirect only to the same origin, never https→http.
pub fn redirect_allowed(from: &Origin, to: &Origin) -> bool {
    from == to && !(from.https && !to.https)
}

/// What injecting one secret changes in a request.
pub enum InjectEdit {
    Header { name: String, value: SecretBytes },
    Query { param: String, value: SecretBytes },
    Body,
}

fn apply_fmt(fmt: &str, value: &[u8]) -> SecretBytes {
    let (a, b) = fmt.split_once("{}").expect("fmt validated to hold {}");
    let mut v = Vec::with_capacity(a.len() + value.len() + b.len());
    v.extend_from_slice(a.as_bytes());
    v.extend_from_slice(value);
    v.extend_from_slice(b.as_bytes());
    SecretBytes::new(v)
}

/// Build the edit for one secret. A value with CR/LF/NUL can't go into a
/// header (VS5).
pub fn inject_edit(dst: &Dst, fmt: &str, value: &SecretBytes) -> Result<InjectEdit> {
    let header_safe = !value.expose().iter().any(|&b| b == b'\r' || b == b'\n' || b == 0);
    match dst {
        Dst::Header(name) => {
            if !header_safe {
                return Err(VaultError::bad_request("value_not_header_safe", "this value has a line break"));
            }
            Ok(InjectEdit::Header { name: name.clone(), value: apply_fmt(fmt, value.expose()) })
        }
        Dst::Basic => {
            if !header_safe {
                return Err(VaultError::bad_request("value_not_header_safe", "this value has a line break"));
            }
            let creds = apply_fmt(fmt, value.expose());
            let enc = STANDARD.encode(creds.expose());
            Ok(InjectEdit::Header {
                name: "Authorization".into(),
                value: SecretBytes::from_string(format!("Basic {enc}")),
            })
        }
        Dst::Query(param) => Ok(InjectEdit::Query { param: param.clone(), value: apply_fmt(fmt, value.expose()) }),
        Dst::Body => Ok(InjectEdit::Body),
    }
}

/// Replace the body marker with the formatted value (dst = body only).
pub fn inject_body(body: &[u8], fmt: &str, value: &SecretBytes) -> Result<Vec<u8>> {
    let marker = BODY_MARKER.as_bytes();
    let count = body.windows(marker.len()).filter(|w| *w == marker).count();
    if count == 0 {
        return Err(VaultError::bad_request(
            "body_marker_missing",
            format!("put {BODY_MARKER} in the body where the secret goes"),
        ));
    }
    let v = apply_fmt(fmt, value.expose());
    let mut out = Vec::with_capacity(body.len() + v.len() * count);
    let mut i = 0;
    while i < body.len() {
        if body[i..].starts_with(marker) {
            out.extend_from_slice(v.expose());
            i += marker.len();
        } else {
            out.push(body[i]);
            i += 1;
        }
    }
    Ok(out)
}

/// Fixed error text for upstream failures (VS6): never the URL, query or
/// headers.
pub fn upstream_error(kind: &str, origin: &Origin) -> VaultError {
    VaultError::new("upstream_error", 502, format!("{kind} talking to {}", origin.host_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inject::Inject;

    fn o(s: &str) -> Origin {
        Origin::parse_url(s).expect("url").1
    }

    #[test]
    fn host_policy_table() {
        let allowed = vec![
            OriginPattern::parse("api.stripe.com", false).expect("ok"),
            OriginPattern::parse("http://10.0.0.5:8080", false).expect("ok"),
        ];
        check_host(&o("https://api.stripe.com/v1/charges?x=1"), &allowed).expect("allowed");
        check_host(&o("http://10.0.0.5:8080/x"), &allowed).expect("exact ip");
        for denied in [
            "https://evil.example/",
            "http://api.stripe.com/",
            "https://api.stripe.com:8443/",
            "https://api.stripe.com.evil.example/",
            "http://10.0.0.5:8081/",
            "http://169.254.169.254/latest/meta-data",
        ] {
            assert_eq!(check_host(&o(denied), &allowed).expect_err(denied).code, "host_not_allowed", "{denied}");
        }
        assert_eq!(check_host(&o("https://api.stripe.com/"), &[]).expect_err("none").code, "no_allowed_hosts");
    }

    #[test]
    fn agent_headers() {
        validate_agent_header("Content-Type", "application/json").expect("ok");
        validate_agent_header("Stripe-Version", "2024-06-20").expect("ok");
        for (n, v) in [("Host", "x"), ("Transfer-Encoding", "chunked"), ("Content-Length", "1"), ("Connection", "close"),
                       ("X-A", "a\r\nX-B: b"), ("X A", "v"), ("Accept-Encoding", "gzip"), ("Proxy-Authorization", "x")] {
            assert_eq!(validate_agent_header(n, v).expect_err(n).code, "header_not_allowed", "{n}");
        }
    }

    #[test]
    fn redirects() {
        let a = o("https://api.stripe.com/a");
        assert!(redirect_allowed(&a, &o("https://api.stripe.com/b")));
        assert!(!redirect_allowed(&a, &o("https://files.stripe.com/b")), "sibling subdomain");
        assert!(!redirect_allowed(&a, &o("http://api.stripe.com/b")), "https→http");
        assert!(!redirect_allowed(&a, &o("https://evil.example/")));
    }

    #[test]
    fn inject_edits() {
        let key = SecretBytes::from_slice(b"k2test_k1");
        let i = Inject::bearer();
        match inject_edit(&i.dst, &i.fmt, &key).expect("ok") {
            InjectEdit::Header { name, value } => {
                assert_eq!(name, "Authorization");
                assert_eq!(value.expose(), b"Bearer k2test_k1");
            }
            _ => panic!("header expected"),
        }
        match inject_edit(&Dst::Basic, "{}:", &key).expect("ok") {
            InjectEdit::Header { value, .. } => assert_eq!(value.expose(), b"Basic azJ0ZXN0X2sxOg=="),
            _ => panic!("basic expected"),
        }
        let nl = SecretBytes::from_slice(b"a\nb");
        assert_eq!(inject_edit(&i.dst, &i.fmt, &nl).err().expect("refused").code, "value_not_header_safe");
        let b = inject_body(b"client_secret={{secret}}&x=1", "{}", &key).expect("ok");
        assert_eq!(b, b"client_secret=k2test_k1&x=1");
        assert_eq!(inject_body(b"x=1", "{}", &key).expect_err("no marker").code, "body_marker_missing");
    }

    #[test]
    fn upstream_errors_never_carry_the_url() {
        let origin = o("https://api.stripe.com/v1/x?key=k2test_k1");
        let e = upstream_error("timeout", &origin);
        assert!(!e.hint.contains("k2test_k1"));
        assert!(!e.hint.contains("/v1/x"));
    }
}
