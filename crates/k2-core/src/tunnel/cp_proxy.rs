//! The `k2` CLI's door to the K2 Connect control plane (0.45.7).
//!
//! `k2 publish status` and `k2 publish subdomain list|create|point|rm`
//! used to read the tunnel bearer token out of `~/.k2/tunnel.json` and
//! call `connect.k2.dev` themselves. Since 0.45.4 a desktop keeps that
//! token in the K2 vault and `tunnel.json`'s `token` is empty, so every
//! one of those verbs failed with "no K2 Connect tunnel token".
//!
//! The daemon already resolves the token ([`config::load`], vault-aware)
//! for the connector, the cert broker and the subdomain refresh loop. This
//! module lets the CLI ask the daemon to make the control-plane call for
//! it: the CLI names a method + endpoint (+ JSON body), the daemon checks
//! it against a short allowlist, adds `Authorization: Bearer <token>` and
//! hands back the control plane's HTTP status and body verbatim. The token
//! never leaves the daemon.
//!
//! Allowlist (every control-plane call `cli/k2` makes):
//!
//! | method   | endpoint              | CLI verb                              |
//! |----------|-----------------------|---------------------------------------|
//! | `GET`    | `/subdomains`         | publish status, subdomain list        |
//! | `POST`   | `/subdomains`         | subdomain create, wiki publish        |
//! | `PUT`    | `/subdomains/<label>` | subdomain point, wiki publish         |
//! | `DELETE` | `/subdomains/<label>` | subdomain rm                          |

use std::time::Duration;

use super::config;
use super::subdomains::control_plane_base;

/// Per-request timeout. Matches the CLI's old `curl --max-time 30`.
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// Largest JSON body the CLI may forward. The real bodies are
/// `{"label","target"}` / `{"target"}`, well under 1 KiB.
pub const MAX_BODY_BYTES: usize = 16 * 1024;

/// Why a proxied request was not sent (or got no answer). Each maps to a
/// distinct daemon response so the CLI can keep its old messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyError {
    /// Method/endpoint/body not on the allowlist. Nothing was sent.
    NotAllowed(String),
    /// Air-gap is on. Nothing was sent.
    Airgap(String),
    /// `tunnel.json` could not be read or parsed. Nothing was sent.
    Config(String),
    /// The vault holds the token but could not be read right now (locked,
    /// unavailable). Not the same as "unpaired"; nothing was sent.
    TokenUnreadable(String),
    /// No tunnel token at all (this device is not paired). Nothing was sent.
    NoToken,
    /// The control plane could not be reached / did not answer.
    Unreachable(String),
}

/// The control plane's answer, passed through verbatim (any status).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyResponse {
    pub status: u16,
    pub body: String,
}

/// One path segment the CLI may name after `/subdomains/`: the
/// (url-encoded) label. Never empty, never `.`/`..`, never a `/`, `?` or
/// `#`, and no percent-encoded `/`, `\` or `.` sneaking one in.
fn label_segment_ok(seg: &str) -> bool {
    if seg.is_empty() || seg.len() > 253 || seg == "." || seg == ".." {
        return false;
    }
    if !seg
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'%'))
    {
        return false;
    }
    // A `%` must start a well-formed escape that is not `/`, `\` or `.`.
    let bytes = seg.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let Some(hex) = seg.get(i + 1..i + 3) else { return false };
            let Ok(v) = u8::from_str_radix(hex, 16) else { return false };
            if matches!(v, b'/' | b'\\' | b'.') || v < 0x20 {
                return false;
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    true
}

/// Check `method endpoint` against the allowlist (see the module docs).
/// `Err` carries a short reason for the 400.
pub fn check_allowed(method: &str, endpoint: &str, body: Option<&str>) -> Result<(), String> {
    let refuse = || {
        Err(format!(
            "not an allowed K2 Connect request: {method} {endpoint} (allowed: GET/POST \
             /subdomains, PUT/DELETE /subdomains/<label>)"
        ))
    };
    if endpoint.contains(['?', '#']) || endpoint.contains("//") {
        return refuse();
    }
    let ok = match (method, endpoint) {
        ("GET" | "POST", "/subdomains") => true,
        ("PUT" | "DELETE", ep) => ep
            .strip_prefix("/subdomains/")
            .is_some_and(label_segment_ok),
        _ => false,
    };
    if !ok {
        return refuse();
    }
    match (method, body) {
        ("GET" | "DELETE", Some(b)) if !b.trim().is_empty() => {
            Err(format!("{method} {endpoint} takes no body"))
        }
        (_, Some(b)) if b.len() > MAX_BODY_BYTES => {
            Err(format!("body too large ({} bytes, max {MAX_BODY_BYTES})", b.len()))
        }
        (_, Some(b)) if !b.trim().is_empty() => serde_json::from_str::<serde_json::Value>(b)
            .map(|_| ())
            .map_err(|e| format!("body must be JSON: {e}")),
        _ => Ok(()),
    }
}

/// Make the control-plane request with the daemon-held tunnel token.
///
/// Order: allowlist → air-gap → load the config (vault-aware) → refuse an
/// unreadable or missing token → send. Nothing goes on the wire unless
/// every check passes; an unreadable vault is never an empty-token call.
/// The token is never logged, and is scrubbed from the returned body
/// should the control plane ever echo it.
pub fn forward(method: &str, endpoint: &str, body: Option<&str>) -> Result<ProxyResponse, ProxyError> {
    check_allowed(method, endpoint, body).map_err(ProxyError::NotAllowed)?;
    crate::airgap::refuse().map_err(ProxyError::Airgap)?;
    let cfg = config::load().map_err(ProxyError::Config)?;
    if let Some(e) = cfg.token_read_error.as_deref() {
        return Err(ProxyError::TokenUnreadable(e.to_string()));
    }
    let token = cfg.token.trim();
    if token.is_empty() {
        return Err(ProxyError::NoToken);
    }

    let url = format!("{}{endpoint}", control_plane_base());
    let client = reqwest::blocking::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .connect_timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| ProxyError::Unreachable(format!("http client build failed: {e}")))?;
    let mut req = match method {
        "GET" => client.get(&url),
        "POST" => client.post(&url),
        "PUT" => client.put(&url),
        "DELETE" => client.delete(&url),
        // check_allowed admitted only the four above.
        other => return Err(ProxyError::NotAllowed(format!("unsupported method {other}"))),
    };
    req = req.header("Authorization", format!("Bearer {token}"));
    if let Some(b) = body.filter(|b| !b.trim().is_empty()) {
        req = req
            .header("Content-Type", "application/json")
            .body(b.to_string());
    }
    // reqwest's error text names the URL, never the headers.
    let resp = req
        .send()
        .map_err(|e| ProxyError::Unreachable(format!("{method} {url}: {e}")))?;
    let status = resp.status().as_u16();
    let text = resp
        .text()
        .map_err(|e| ProxyError::Unreachable(format!("read {endpoint} response: {e}")))?;
    Ok(ProxyResponse { status, body: text.replace(token, "[redacted]") })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnel::config::{MemTokenVault, TestTokenVault, TunnelConfig};
    use crate::tunnel::subdomains::CONTROL_PLANE_BASE_ENV;
    use crate::tunnel::test_support::with_temp_home;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    /// A one-shot fake control plane: records each raw request and answers
    /// `status body`. Returns its base URL and the request receiver.
    fn fake_cp(status: &'static str, body: &'static str) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind fake control plane");
        let port = listener.local_addr().expect("addr").port();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok((mut sock, _)) = listener.accept() {
                sock.set_read_timeout(Some(Duration::from_secs(5))).expect("timeout");
                let mut raw = Vec::new();
                let mut buf = [0u8; 4096];
                // Read headers, then Content-Length bytes of body.
                loop {
                    let n = sock.read(&mut buf).expect("read request");
                    if n == 0 {
                        break;
                    }
                    raw.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&raw).into_owned();
                    if let Some((head, rest)) = text.split_once("\r\n\r\n") {
                        let clen = head
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().expect("clen"))
                            })
                            .unwrap_or(0);
                        if rest.len() >= clen {
                            break;
                        }
                    }
                }
                tx.send(String::from_utf8_lossy(&raw).into_owned()).expect("send");
                let resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                sock.write_all(resp.as_bytes()).expect("write response");
            }
        });
        (format!("http://127.0.0.1:{port}"), rx)
    }

    fn seed_vaulted(vault: &MemTokenVault, token: &str) {
        config::save(&TunnelConfig {
            token: token.to_string(),
            subdomain: "rosson".to_string(),
            ..Default::default()
        })
        .expect("seed");
        assert_eq!(vault.stored().as_deref(), Some(token), "setup: token in the vault");
        let raw = std::fs::read_to_string(config::config_path()).expect("read tunnel.json");
        assert!(!raw.contains(token), "setup: tunnel.json keeps no token: {raw}");
    }

    #[test]
    fn allowlist_admits_exactly_the_cli_calls() {
        for (m, ep, body) in [
            ("GET", "/subdomains", None),
            ("POST", "/subdomains", Some(r#"{"label":"staging","target":"localhost:3000"}"#)),
            ("PUT", "/subdomains/staging", Some(r#"{"target":"localhost:3001"}"#)),
            ("DELETE", "/subdomains/staging", None),
            ("PUT", "/subdomains/a%20b", Some("{}")),
        ] {
            check_allowed(m, ep, body).unwrap_or_else(|e| panic!("{m} {ep} must pass: {e}"));
        }
        for (m, ep, body) in [
            ("GET", "/api/dns/ptr", None),
            ("POST", "/api/dns/ptr", None),
            ("GET", "/subdomains/staging", None),
            ("POST", "/subdomains/staging", None),
            ("PUT", "/subdomains", None),
            ("DELETE", "/subdomains", None),
            ("PATCH", "/subdomains", None),
            ("get", "/subdomains", None),
            ("GET", "/subdomains?x=1", None),
            ("GET", "/subdomains#x", None),
            ("GET", "//evil.example/subdomains", None),
            ("GET", "subdomains", None),
            ("GET", "http://evil.example/subdomains", None),
            ("PUT", "/subdomains/", None),
            ("PUT", "/subdomains/..", None),
            ("PUT", "/subdomains/a/b", None),
            ("PUT", "/subdomains/a%2Fb", None),
            ("PUT", "/subdomains/a%2e%2e", None),
            ("PUT", "/subdomains/a%zz", None),
            ("PUT", "/subdomains/a%2", None),
            ("DELETE", "/subdomains/a b", None),
        ] {
            let e = check_allowed(m, ep, body).expect_err(&format!("{m} {ep} must be refused"));
            assert!(e.contains("not an allowed K2 Connect request"), "{m} {ep}: {e}");
        }
        assert!(check_allowed("GET", "/subdomains", Some("{}")).is_err(), "GET takes no body");
        assert!(check_allowed("POST", "/subdomains", Some("not json")).is_err(), "body must be JSON");
        let big = format!("{{\"x\":\"{}\"}}", "a".repeat(MAX_BODY_BYTES));
        assert!(check_allowed("POST", "/subdomains", Some(&big)).is_err(), "body cap");
    }

    #[test]
    fn forwards_with_the_vault_token_and_returns_status_and_body() {
        with_temp_home(|| {
            let vault = MemTokenVault::holding(None);
            let _v = TestTokenVault::install(vault.clone());
            seed_vaulted(&vault, "tok_from_vault_7f3");
            let (base, rx) = fake_cp("201 Created", r#"{"host":"staging.rosson.k2.dev","status":"active"}"#);
            let _cp = crate::test_env::EnvVar::set(CONTROL_PLANE_BASE_ENV, &base);
            let _ag = crate::test_env::EnvVar::remove("K2_AIRGAP");

            let body = r#"{"label":"staging","target":"localhost:3000"}"#;
            let r = forward("POST", "/subdomains", Some(body)).expect("forward");
            assert_eq!(r.status, 201);
            assert_eq!(r.body, r#"{"host":"staging.rosson.k2.dev","status":"active"}"#);

            let req = rx.recv_timeout(Duration::from_secs(5)).expect("control plane saw the request");
            assert!(req.starts_with("POST /subdomains "), "{req}");
            assert!(
                req.to_ascii_lowercase().contains("authorization: bearer tok_from_vault_7f3"),
                "must carry the vault token:\n{req}"
            );
            assert!(req.ends_with(body), "body forwarded verbatim:\n{req}");
        });
    }

    #[test]
    fn control_plane_errors_pass_through_and_an_echoed_token_is_scrubbed() {
        with_temp_home(|| {
            let vault = MemTokenVault::holding(None);
            let _v = TestTokenVault::install(vault.clone());
            seed_vaulted(&vault, "tok_echo_42");
            let (base, rx) = fake_cp("401 Unauthorized", r#"{"error":"invalid_token","detail":"tok_echo_42"}"#);
            let _cp = crate::test_env::EnvVar::set(CONTROL_PLANE_BASE_ENV, &base);
            let _ag = crate::test_env::EnvVar::remove("K2_AIRGAP");

            let r = forward("DELETE", "/subdomains/staging", None).expect("forward");
            assert_eq!(r.status, 401);
            assert_eq!(r.body, r#"{"error":"invalid_token","detail":"[redacted]"}"#);
            let req = rx.recv_timeout(Duration::from_secs(5)).expect("request");
            assert!(req.starts_with("DELETE /subdomains/staging "), "{req}");
        });
    }

    #[test]
    fn an_unreadable_vault_is_a_soft_error_and_sends_nothing() {
        with_temp_home(|| {
            let vault = MemTokenVault::holding(None);
            let _v = TestTokenVault::install(vault.clone());
            seed_vaulted(&vault, "tok_locked");
            vault.set_fail_reads(true);
            let (base, rx) = fake_cp("200 OK", "{}");
            let _cp = crate::test_env::EnvVar::set(CONTROL_PLANE_BASE_ENV, &base);
            let _ag = crate::test_env::EnvVar::remove("K2_AIRGAP");

            let e = forward("GET", "/subdomains", None).expect_err("must refuse");
            assert!(matches!(e, ProxyError::TokenUnreadable(ref m) if m.contains("read failed")), "{e:?}");
            assert!(
                rx.recv_timeout(Duration::from_millis(300)).is_err(),
                "no request may reach the control plane with an unreadable token"
            );
            assert_eq!(vault.stored().as_deref(), Some("tok_locked"), "token kept");
        });
    }

    #[test]
    fn no_token_and_refused_endpoints_send_nothing() {
        with_temp_home(|| {
            let vault = MemTokenVault::holding(None);
            let _v = TestTokenVault::install(vault.clone());
            let (base, rx) = fake_cp("200 OK", "{}");
            let _cp = crate::test_env::EnvVar::set(CONTROL_PLANE_BASE_ENV, &base);
            let _ag = crate::test_env::EnvVar::remove("K2_AIRGAP");

            assert_eq!(forward("GET", "/subdomains", None), Err(ProxyError::NoToken));

            seed_vaulted(&vault, "tok_x");
            let e = forward("GET", "/api/account", None).expect_err("refused");
            assert!(matches!(e, ProxyError::NotAllowed(_)), "{e:?}");
            assert!(rx.recv_timeout(Duration::from_millis(300)).is_err(), "nothing sent");
        });
    }

    #[test]
    fn air_gap_refuses_before_reading_the_token() {
        with_temp_home(|| {
            let vault = MemTokenVault::holding(None);
            let _v = TestTokenVault::install(vault.clone());
            seed_vaulted(&vault, "tok_airgap");
            let (base, rx) = fake_cp("200 OK", "{}");
            let _cp = crate::test_env::EnvVar::set(CONTROL_PLANE_BASE_ENV, &base);
            let _ag = crate::test_env::EnvVar::set("K2_AIRGAP", "1");

            let e = forward("GET", "/subdomains", None).expect_err("air-gap refuses");
            assert!(matches!(e, ProxyError::Airgap(ref m) if m.contains("K2_AIRGAP")), "{e:?}");
            assert!(rx.recv_timeout(Duration::from_millis(300)).is_err(), "nothing sent");
        });
    }
}
