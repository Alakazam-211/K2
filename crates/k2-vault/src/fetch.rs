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
use crate::inject::{is_forbidden_header, validate_header_name, Dst, Enc, Sign};
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

/// The header edits a signer makes for one request (PRD vault-gaps item 2).
/// Pure: `url` is the exact text sent (query included), `body` the exact
/// bytes (empty when none), `now` Unix seconds. Every returned value is
/// sensitive. Errors never quote the value.
pub fn sign_edit(sign: &Sign, value: &SecretBytes, method: &str, url: &str, body: &[u8], now: i64) -> Result<Vec<(String, SecretBytes)>> {
    use hmac::{Hmac, Mac};
    use sha1::{Digest, Sha1};
    let method = method.to_ascii_uppercase();
    let enc = |e: Enc, tag: &[u8]| -> String {
        match e {
            Enc::Hex => crate::crypto::hex(tag),
            Enc::Base64 => STANDARD.encode(tag),
        }
    };
    let hmac256 = |msg: &[u8]| -> Vec<u8> {
        let mut m = <Hmac<sha2::Sha256> as Mac>::new_from_slice(value.expose()).expect("HMAC takes any key length");
        m.update(msg);
        m.finalize().into_bytes().to_vec()
    };
    match sign {
        Sign::Ovh => {
            let body = std::str::from_utf8(body).map_err(|_| VaultError::bad_request("body_not_text", "OVH signs text bodies only"))?;
            let k = Sign::ovh_keys(value.expose())?;
            let ts = now.to_string();
            let mut h = Sha1::new();
            for (i, part) in [k.application_secret.as_str(), k.consumer_key.as_str(), method.as_str(), url, body, ts.as_str()].iter().enumerate() {
                if i > 0 {
                    h.update(b"+");
                }
                h.update(part.as_bytes());
            }
            let sig = format!("$1${}", crate::crypto::hex(&h.finalize()));
            Ok(vec![
                ("X-Ovh-Application".into(), SecretBytes::from_string(k.application_key)),
                ("X-Ovh-Consumer".into(), SecretBytes::from_string(k.consumer_key)),
                ("X-Ovh-Timestamp".into(), SecretBytes::from_string(ts)),
                ("X-Ovh-Signature".into(), SecretBytes::from_string(sig)),
            ])
        }
        Sign::HmacSha256 { header, encoding, prefix } => {
            let v = format!("{prefix}{}", enc(*encoding, &hmac256(body)));
            Ok(vec![(header.clone(), SecretBytes::from_string(v))])
        }
        Sign::HmacSha256Ts { header, ts_header, template, value_fmt, encoding } => {
            let ts = now.to_string();
            // Fill the template structurally: a body holding "{ts}" stays
            // as sent.
            let mut msg = Vec::with_capacity(template.len() + body.len() + ts.len());
            let mut rest = template.as_str();
            while !rest.is_empty() {
                if let Some(r) = rest.strip_prefix("{ts}") {
                    msg.extend_from_slice(ts.as_bytes());
                    rest = r;
                } else if let Some(r) = rest.strip_prefix("{body}") {
                    msg.extend_from_slice(body);
                    rest = r;
                } else {
                    let c = rest.chars().next().expect("non-empty");
                    msg.extend_from_slice(&rest.as_bytes()[..c.len_utf8()]);
                    rest = &rest[c.len_utf8()..];
                }
            }
            let sig = enc(*encoding, &hmac256(&msg));
            let v = value_fmt.replacen("{sig}", &sig, 1).replacen("{ts}", &ts, 1);
            let mut out = vec![(header.clone(), SecretBytes::from_string(v))];
            if let Some(t) = ts_header {
                out.push((t.clone(), SecretBytes::from_string(ts)));
            }
            Ok(out)
        }
    }
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

    fn ovh_value() -> SecretBytes {
        // python-ovh's test keys (tests/test_client.py: MockApplicationKey,
        // MockApplicationSecret, MockConsumerKey).
        SecretBytes::from_slice(
            br#"{"applicationKey":"TDPKJdwZwAQPwKX2","applicationSecret":"9ufkBmLaTQ9nz5yMUlg79taH0GNnzDjk","consumerKey":"5mBuy6SUQcRw2ZUxg0cG68BoDKpED4KY"}"#,
        )
    }

    fn get<'a>(edits: &'a [(String, SecretBytes)], h: &str) -> &'a str {
        edits.iter().find(|(n, _)| n == h).unwrap_or_else(|| panic!("no {h}")).1.expose_str().expect("text")
    }

    #[test]
    fn ovh_matches_python_ovh_known_answers() {
        // Copied byte for byte from python-ovh tests/test_client.py
        // (test_call_signature: MockTime 1457018875, /1.0/auth).
        let url = "https://eu.api.ovh.com/1.0/auth";
        let body = br#"{"a":"b","c":"d"}"#;
        for (m, b, sig) in [
            ("GET", &b""[..], "$1$e9556054b6309771395efa467c22e627407461ad"),
            ("POST", &body[..], "$1$ec2fb5c7a81f64723c77d2e5b609ae6f58a84fc1"),
            ("PUT", &body[..], "$1$8a75a9e7c8e7296c9dbeda6a2a735eb6bd58ec4b"),
            ("DELETE", &b""[..], "$1$a1eecd00b3b02b6cf5708b84b9ff42059a950d85"),
        ] {
            let e = sign_edit(&Sign::Ovh, &ovh_value(), m, url, b, 1457018875).expect("signs");
            assert_eq!(get(&e, "X-Ovh-Signature"), sig, "{m}");
            assert_eq!(get(&e, "X-Ovh-Application"), "TDPKJdwZwAQPwKX2");
            assert_eq!(get(&e, "X-Ovh-Consumer"), "5mBuy6SUQcRw2ZUxg0cG68BoDKpED4KY");
            assert_eq!(get(&e, "X-Ovh-Timestamp"), "1457018875");
            assert_eq!(e.len(), 4);
        }
    }

    #[test]
    fn ovh_signs_the_exact_url_with_its_query_and_lower_case_method() {
        // sha1("secret+consumer+GET+<url with query>++1700000000"), from Python.
        let url = "https://eu.api.ovh.com/1.0/me/api/credential?status=validated&x=1";
        let e = sign_edit(&Sign::Ovh, &ovh_value(), "get", url, b"", 1700000000).expect("signs");
        assert_eq!(get(&e, "X-Ovh-Signature"), "$1$dd82b6fc1c94a59490c6849b9f8cb15a3c0debb4");
        assert_eq!(sign_edit(&Sign::Ovh, &ovh_value(), "POST", url, &[0xff, 0xfe], 1).err().expect("binary").code, "body_not_text");
        let no_ck = SecretBytes::from_slice(br#"{"applicationKey":"a","applicationSecret":"b"}"#);
        let e = sign_edit(&Sign::Ovh, &no_ck, "GET", url, b"", 1).err().expect("bad value");
        assert_eq!(e.code, "bad_value");
        assert!(!e.hint.contains("\"a\"") && !e.hint.contains(":\"b\""), "never quotes the value: {}", e.hint);
        assert_eq!(Sign::Ovh.check_value(b"not json").expect_err("json").code, "bad_value");
        assert_eq!(Sign::Ovh.check_value(br#"{"applicationKey":"a","applicationSecret":"b","consumerKey":"c\nd"}"#).expect_err("lf").code, "bad_value");
        Sign::Ovh.check_value(ovh_value().expose()).expect("good value");
    }

    #[test]
    fn hmac_sha256_known_answers() {
        // RFC 4231 test case 2.
        let key = SecretBytes::from_slice(b"Jefe");
        let body = b"what do ya want for nothing?";
        let hex = Sign::HmacSha256 { header: "X-Hub-Signature-256".into(), encoding: Enc::Hex, prefix: "sha256=".into() };
        let e = sign_edit(&hex, &key, "POST", "https://hooks.example.com/x", body, 0).expect("signs");
        assert_eq!(get(&e, "X-Hub-Signature-256"), "sha256=5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
        let b64 = Sign::HmacSha256 { header: "X-Signature".into(), encoding: Enc::Base64, prefix: String::new() };
        let e = sign_edit(&b64, &key, "POST", "https://hooks.example.com/x", body, 0).expect("signs");
        assert_eq!(get(&e, "X-Signature"), "W9zBRr9gdU5qBCQmCJV1x1oAPwidJzmDnexYuWTsOEM=");
        // Empty body signs "".
        let k2 = SecretBytes::from_slice(b"k2test-hook-key");
        let e = sign_edit(&hex, &k2, "GET", "https://hooks.example.com/x", b"", 0).expect("signs");
        assert_eq!(get(&e, "X-Hub-Signature-256"), "sha256=89a6139a1204d4ca0bc8bc4f5a4374061051f933eab287ec473b094563b194f1");
    }

    #[test]
    fn hmac_sha256_ts_known_answers() {
        // Vectors computed independently with Python's hmac module.
        let key = SecretBytes::from_slice(b"k2test-hook-key");
        let body = br#"{"event":"ping","n":1}"#;
        let u = "https://ingest.example.com/v1/ops";
        let ops = Sign::HmacSha256Ts {
            header: "X-K2-Ops-Signature".into(),
            ts_header: Some("X-K2-Ops-Ts".into()),
            template: "{ts}.{body}".into(),
            value_fmt: "sha256={sig}".into(),
            encoding: Enc::Hex,
        };
        let e = sign_edit(&ops, &key, "POST", u, body, 1700000000).expect("signs");
        assert_eq!(get(&e, "X-K2-Ops-Signature"), "sha256=65bf63d738d8650faf8c091e212f59503494ad29bd5c4baea4898992926aadfb");
        assert_eq!(get(&e, "X-K2-Ops-Ts"), "1700000000");
        let slack = Sign::HmacSha256Ts {
            header: "X-Slack-Signature".into(),
            ts_header: Some("X-Slack-Request-Timestamp".into()),
            template: "v0:{ts}:{body}".into(),
            value_fmt: "v0={sig}".into(),
            encoding: Enc::Hex,
        };
        let e = sign_edit(&slack, &key, "POST", u, body, 1700000000).expect("signs");
        assert_eq!(get(&e, "X-Slack-Signature"), "v0=d063c50dabd347ce1abb351172cc7d1482e4770750a239f31262dc44c917c207");
        assert_eq!(get(&e, "X-Slack-Request-Timestamp"), "1700000000");
        let stripe = Sign::HmacSha256Ts {
            header: "Stripe-Signature".into(),
            ts_header: None,
            template: "{ts}.{body}".into(),
            value_fmt: "t={ts},v1={sig}".into(),
            encoding: Enc::Hex,
        };
        let e = sign_edit(&stripe, &key, "POST", u, body, 1700000000).expect("signs");
        assert_eq!(e.len(), 1);
        assert_eq!(get(&e, "Stripe-Signature"), "t=1700000000,v1=65bf63d738d8650faf8c091e212f59503494ad29bd5c4baea4898992926aadfb");
        // A body that contains "{ts}" is signed as sent.
        let e = sign_edit(&ops, &key, "POST", u, b"a{ts}b", 1700000000).expect("signs");
        assert_eq!(get(&e, "X-K2-Ops-Signature"), "sha256=ad235bca3029175ce8c8f22bb7cd96e584d8751f91839b39009fd8ed648481cb");
    }

    #[test]
    fn upstream_errors_never_carry_the_url() {
        let origin = o("https://api.stripe.com/v1/x?key=k2test_k1");
        let e = upstream_error("timeout", &origin);
        assert!(!e.hint.contains("k2test_k1"));
        assert!(!e.hint.contains("/v1/x"));
    }
}
