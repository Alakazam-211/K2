//! Control-plane (web API) DNS client.
//!
//! Auth: tunnel bearer from `~/.k2/tunnel.json` as
//! `Authorization: Bearer k2c_…`. Audit-only header: `X-K2-Agent` (≤120).
//!
//! Base URL: [`dns_api_base`] — `K2_DNS_API_BASE` if set, else
//! [`DEFAULT_DNS_API_BASE`] (`https://k2.dev`). The live routes live on
//! k2-dev-web (`/api/dns/…`); connect.k2.dev is the tunnel/subdomain plane
//! and is a different host.

use std::time::Duration;

/// Default web API host that serves `/api/dns/*` (k2-dev-web).
pub const DEFAULT_DNS_API_BASE: &str = "https://k2.dev";

/// Env override for [`DEFAULT_DNS_API_BASE`] (tests / staging).
pub const DNS_API_BASE_ENV: &str = "K2_DNS_API_BASE";

const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// Max chars of the audit-only `X-K2-Agent` header (control-plane contract).
pub const AGENT_HEADER_MAX: usize = 120;

/// Resolve the DNS API base URL (trailing slash trimmed).
pub fn dns_api_base() -> String {
    let base = match std::env::var(DNS_API_BASE_ENV) {
        Ok(v) if !v.trim().is_empty() => v.trim().to_string(),
        _ => DEFAULT_DNS_API_BASE.to_string(),
    };
    base.trim_end_matches('/').to_string()
}

/// Load the tunnel bearer token from `~/.k2/tunnel.json`.
///
/// The token is already minted as `k2c_<label>_<hex>` by the control plane;
/// we send it as-is (do not re-prefix).
pub fn tunnel_bearer_token() -> Result<String, String> {
    let cfg = k2_core::tunnel::config::load()?;
    let tok = cfg.token.trim().to_string();
    if tok.is_empty() {
        return Err(
            "no tunnel token in ~/.k2/tunnel.json — pair K2 Connect first".to_string(),
        );
    }
    Ok(tok)
}

/// Format the Authorization header value (`Bearer <token>`).
pub fn authorization_header(token: &str) -> String {
    format!("Bearer {}", token.trim())
}

/// Truncate an agent name for the audit header.
pub fn agent_header_value(name: &str) -> String {
    let t = name.trim();
    if t.chars().count() <= AGENT_HEADER_MAX {
        t.to_string()
    } else {
        t.chars().take(AGENT_HEADER_MAX).collect()
    }
}

/// One HTTP response from the DNS API (status + body text).
#[derive(Debug, Clone)]
pub struct DnsHttpResponse {
    pub status: u16,
    pub body: String,
}

/// Injectable HTTP surface so unit tests never dial the live API.
pub trait DnsHttpClient: Send + Sync {
    fn request(
        &self,
        method: &str,
        path: &str,
        token: &str,
        agent: Option<&str>,
        body: Option<&str>,
    ) -> Result<DnsHttpResponse, String>;
}

/// Production client: blocking reqwest against [`dns_api_base`].
#[derive(Debug, Default, Clone, Copy)]
pub struct ReqwestDnsClient;

impl DnsHttpClient for ReqwestDnsClient {
    fn request(
        &self,
        method: &str,
        path: &str,
        token: &str,
        agent: Option<&str>,
        body: Option<&str>,
    ) -> Result<DnsHttpResponse, String> {
        k2_core::airgap::refuse()?;
        let url = format!("{}{}", dns_api_base(), path);
        let client = reqwest::blocking::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .build()
            .map_err(|e| format!("dns http client build failed: {e}"))?;
        let mut builder = match method {
            "GET" => client.get(&url),
            "POST" => client.post(&url),
            "PUT" => client.put(&url),
            "DELETE" => client.delete(&url),
            other => return Err(format!("unsupported DNS proxy method: {other}")),
        };
        builder = builder.header("Authorization", authorization_header(token));
        if let Some(a) = agent.map(str::trim).filter(|s| !s.is_empty()) {
            builder = builder.header("X-K2-Agent", agent_header_value(a));
        }
        if let Some(b) = body {
            builder = builder
                .header("Content-Type", "application/json")
                .body(b.to_string());
        }
        let resp = builder
            .send()
            .map_err(|e| format!("DNS API {method} {path}: {e}"))?;
        let status = resp.status().as_u16();
        let body = resp
            .text()
            .map_err(|e| format!("read DNS API response: {e}"))?;
        Ok(DnsHttpResponse { status, body })
    }
}

/// Convenience: production client + token load + request.
pub fn proxy_request(
    method: &str,
    path: &str,
    agent: Option<&str>,
    body: Option<&str>,
) -> Result<DnsHttpResponse, String> {
    let token = tunnel_bearer_token()?;
    ReqwestDnsClient.request(method, path, &token, agent, body)
}

// ── Response mapping (status → CliResponse-friendly) ─────────────────

/// Map a control-plane HTTP status + body into a daemon CLI-facing status
/// line and JSON body. Surfaces 429 as a clean retry message.
pub fn map_proxy_response(resp: &DnsHttpResponse) -> (&'static str, String) {
    match resp.status {
        200 | 201 => {
            let status = if resp.status == 201 {
                "201 Created"
            } else {
                "200 OK"
            };
            // Pass through JSON as-is when valid; wrap plain text.
            if resp.body.trim_start().starts_with('{') || resp.body.trim_start().starts_with('[') {
                (status, resp.body.clone())
            } else {
                (
                    status,
                    serde_json::json!({ "ok": true, "body": resp.body }).to_string(),
                )
            }
        }
        401 => (
            "401 Unauthorized",
            serde_json::json!({
                "ok": false,
                "error": {
                    "code": "unauthorized",
                    "hint": "tunnel token rejected by DNS API — re-pair K2 Connect"
                }
            })
            .to_string(),
        ),
        402 => (
            "402 Payment Required",
            serde_json::json!({
                "ok": false,
                "error": {
                    "code": "entitlement_required",
                    "hint": parse_error_hint(&resp.body)
                        .unwrap_or_else(|| "Managed DNS requires a paid K2 plan".to_string())
                }
            })
            .to_string(),
        ),
        403 => (
            "403 Forbidden",
            serde_json::json!({
                "ok": false,
                "error": {
                    "code": "forbidden",
                    "hint": parse_error_hint(&resp.body)
                        .unwrap_or_else(|| "DNS API refused this action".to_string())
                }
            })
            .to_string(),
        ),
        404 => (
            "404 Not Found",
            serde_json::json!({
                "ok": false,
                "error": {
                    "code": "not_found",
                    "hint": parse_error_hint(&resp.body)
                        .unwrap_or_else(|| "zone or record not found".to_string())
                }
            })
            .to_string(),
        ),
        409 => (
            "409 Conflict",
            serde_json::json!({
                "ok": false,
                "error": {
                    "code": "conflict",
                    "hint": parse_error_hint(&resp.body)
                        .unwrap_or_else(|| "DNS conflict".to_string())
                }
            })
            .to_string(),
        ),
        422 => (
            "422 Unprocessable Entity",
            serde_json::json!({
                "ok": false,
                "error": {
                    "code": "validation",
                    "hint": parse_error_hint(&resp.body)
                        .unwrap_or_else(|| "invalid DNS record".to_string())
                }
            })
            .to_string(),
        ),
        429 => (
            "429 Too Many Requests",
            serde_json::json!({
                "ok": false,
                "error": {
                    "code": "rate_limited",
                    "hint": "DNS change rate limit reached — retry in a few minutes"
                }
            })
            .to_string(),
        ),
        s if (500..600).contains(&s) => (
            "502 Bad Gateway",
            serde_json::json!({
                "ok": false,
                "error": {
                    "code": "upstream",
                    "hint": format!("DNS API error (HTTP {s})")
                }
            })
            .to_string(),
        ),
        s => (
            "502 Bad Gateway",
            serde_json::json!({
                "ok": false,
                "error": {
                    "code": "upstream",
                    "hint": format!("unexpected DNS API status {s}")
                }
            })
            .to_string(),
        ),
    }
}

fn parse_error_hint(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    v.get("error")
        .and_then(|e| e.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            v.get("error")
                .and_then(|e| e.get("hint"))
                .and_then(|h| h.as_str())
                .map(|s| s.to_string())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex, OnceLock};

    /// Serialize env mutations (K2_DNS_API_BASE / HOME) across dns proxy tests.
    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn authorization_header_uses_token_as_is() {
        assert_eq!(
            authorization_header("k2c_demo_abc123"),
            "Bearer k2c_demo_abc123"
        );
        assert_eq!(authorization_header("  k2c_x  "), "Bearer k2c_x");
    }

    #[test]
    fn agent_header_truncates_at_120() {
        let short = "agent-a";
        assert_eq!(agent_header_value(short), "agent-a");
        let long: String = (0..200).map(|_| 'x').collect();
        assert_eq!(agent_header_value(&long).chars().count(), 120);
    }

    #[test]
    fn map_proxy_response_surfaces_429_cleanly() {
        let (status, body) = map_proxy_response(&DnsHttpResponse {
            status: 429,
            body: r#"{"error":"rate limited"}"#.to_string(),
        });
        assert_eq!(status, "429 Too Many Requests");
        assert!(body.contains("rate_limited"), "{body}");
        assert!(body.contains("retry"), "{body}");
    }

    #[test]
    fn dns_api_base_default_and_override() {
        let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var_os(DNS_API_BASE_ENV);
        std::env::remove_var(DNS_API_BASE_ENV);
        assert_eq!(dns_api_base(), DEFAULT_DNS_API_BASE);
        std::env::set_var(DNS_API_BASE_ENV, "http://127.0.0.1:9/");
        assert_eq!(dns_api_base(), "http://127.0.0.1:9");
        match prev {
            Some(p) => std::env::set_var(DNS_API_BASE_ENV, p),
            None => std::env::remove_var(DNS_API_BASE_ENV),
        }
    }

    /// Manual mock HTTP: proxy builds the correct Authorization header.
    #[test]
    fn proxy_builds_correct_authorization_header_from_fake_token() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind mock");
        let port = listener.local_addr().unwrap().port();
        let seen: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
        let seen_c = Arc::clone(&seen);
        std::thread::spawn(move || {
            if let Ok((mut sock, _)) = listener.accept() {
                let mut buf = [0u8; 8192];
                let n = sock.read(&mut buf).unwrap_or(0);
                *seen_c.lock().unwrap() = String::from_utf8_lossy(&buf[..n]).into_owned();
                let body = r#"{"zones":[],"capability":{"allowed":true}}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes());
            }
        });

        let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var_os(DNS_API_BASE_ENV);
        std::env::set_var(DNS_API_BASE_ENV, format!("http://127.0.0.1:{port}"));

        let fake_token = "k2c_testlabel_deadbeefcafebabe";
        let resp = ReqwestDnsClient
            .request(
                "GET",
                "/api/dns/zones",
                fake_token,
                Some("agent-alpha-with-a-very-long-name-that-should-be-truncated-past-one-hundred-twenty-characters-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"),
                None,
            )
            .expect("mock fetch");

        match prev {
            Some(p) => std::env::set_var(DNS_API_BASE_ENV, p),
            None => std::env::remove_var(DNS_API_BASE_ENV),
        }

        assert_eq!(resp.status, 200);
        let req = seen.lock().unwrap().clone();
        let lower = req.to_ascii_lowercase();
        assert!(
            lower.contains(&format!("authorization: bearer {fake_token}")),
            "must send Bearer token as-is:\n{req}"
        );
        assert!(
            lower.contains("x-k2-agent:"),
            "must send audit agent header:\n{req}"
        );
        if let Some(line) = req
            .lines()
            .find(|l| l.to_ascii_lowercase().starts_with("x-k2-agent:"))
        {
            let val = line.split_once(':').map(|(_, v)| v.trim()).unwrap_or("");
            assert!(val.chars().count() <= 120, "agent header too long: {val}");
        }
        assert!(req.starts_with("GET /api/dns/zones"), "path:\n{req}");
    }

    #[test]
    fn tunnel_bearer_token_empty_is_error() {
        let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        let tmp = std::env::temp_dir().join(format!(
            "k2-dns-proxy-tok-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::create_dir_all(&tmp);
        let prev_home = std::env::var_os("HOME");
        std::env::set_var("HOME", &tmp);
        let err = tunnel_bearer_token().expect_err("empty token");
        assert!(err.contains("tunnel token"), "{err}");
        match prev_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
