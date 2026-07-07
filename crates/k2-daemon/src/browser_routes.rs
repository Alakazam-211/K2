//! 0.40.34 — `POST /cli/browser/open-url`: surface a URL in the
//! CONNECTED K2 app instead of opening a browser on the daemon's box.
//!
//! The staged `~/.k2/bin/k2-open` shim (see `k2_core::open_shim`)
//! intercepts `xdg-open <url>` / `$BROWSER` inside K2 terminal sessions
//! and POSTs the URL here. The handler validates it (http/https only,
//! ≤2048 chars) and broadcasts an APP-LEVEL
//! [`SessionEvent::OpenUrl`](crate::session_events::SessionEvent) on the
//! `/cli/sessions/events` bus — which crosses the K2 Connect tunnel, so
//! a REMOTE viewer of a headless server gets the URL too. The daemon
//! itself never shells out to `open`/`xdg-open` on this route.
//!
//! Reachable over TWO channels:
//!   - loopback TCP (`routes::dispatcher`): owner-token / connect-user
//!     session gate (`token_ok`, standard `?token=` query), POST-only.
//!   - the per-cell UDS (`cell_server`): scoped hook token — the verb is
//!     on the `session_token::is_agent_verb` allowlist so a sandboxed /
//!     scoped session's shim can use `K2_HOOK_SOCK` + `K2_HOOK_TOKEN`.
//!
//! Body shapes accepted (the shim sends the form one via curl
//! `--data-urlencode`): JSON `{"url": "..."}`, form `url=...`, or a
//! `url` query param.

use std::collections::HashMap;

use crate::cli_response::CliResponse;
use crate::session_events::{self, SessionEvent};

/// Hard cap on an accepted URL. Browsers/servers commonly cap around
/// 2k; anything longer is refused outright (400) rather than truncated.
pub const MAX_URL_LEN: usize = 2048;

/// Validate a raw url string for the open-url surface. PURE.
///
/// Accepts ONLY absolute `http://` / `https://` URLs (case-insensitive
/// scheme) with a non-empty remainder, at most [`MAX_URL_LEN`] chars,
/// containing no whitespace or control characters. Everything else —
/// `file://`, `javascript:`, bare paths, empty — is an `Err` (→ 400):
/// this event drives a browser tab in every connected client, so the
/// scheme allowlist is deliberately closed.
pub fn validate_open_url(raw: &str) -> Result<String, String> {
    let url = raw.trim();
    if url.is_empty() {
        return Err("missing url".to_string());
    }
    if url.len() > MAX_URL_LEN {
        return Err(format!("url too long (max {MAX_URL_LEN} chars)"));
    }
    if url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("url must not contain whitespace or control characters".to_string());
    }
    let lower = url.to_ascii_lowercase();
    let rest = lower
        .strip_prefix("http://")
        .or_else(|| lower.strip_prefix("https://"));
    match rest {
        Some(r) if !r.is_empty() => Ok(url.to_string()),
        _ => Err("only http:// and https:// urls can be opened".to_string()),
    }
}

/// Pull the raw url out of a request: JSON body `{"url": ...}` first,
/// then a form body `url=...`, then the merged query/form `params`.
fn extract_url(params: &HashMap<String, String>, body: &[u8]) -> Option<String> {
    let first_non_ws = body.iter().find(|b| !b.is_ascii_whitespace());
    if first_non_ws == Some(&b'{') {
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) {
            if let Some(u) = v.get("url").and_then(|u| u.as_str()) {
                return Some(u.to_string());
            }
        }
    } else if !body.is_empty() {
        let form = crate::routes::http::parse_form_body(body);
        if let Some(u) = form.get("url") {
            return Some(u.clone());
        }
    }
    params.get("url").cloned()
}

/// `POST /cli/browser/open-url` — validate + broadcast. Method gating
/// (POST-only) and auth are upstream (dispatcher arm / cell server);
/// `source` is stamped by the CALLING channel, never the request
/// (`"shim"` for both transports today).
pub fn handle_open_url(
    params: &HashMap<String, String>,
    body: &[u8],
    source: &str,
) -> CliResponse {
    let raw = extract_url(params, body).unwrap_or_default();
    let url = match validate_open_url(&raw) {
        Ok(u) => u,
        Err(e) => return CliResponse::bad_request(e),
    };
    // Best-effort broadcast (the bus convention): zero subscribers is
    // Err in tokio's broadcast — report it as subscribers=0, still 200
    // (the request was valid; there's just nobody watching right now).
    let subscribers = session_events::emit(SessionEvent::OpenUrl {
        url,
        source: source.to_string(),
    })
    .unwrap_or(0);
    CliResponse::ok_json(
        serde_json::json!({ "success": true, "subscribers": subscribers }).to_string(),
    )
}

/// GET-chain guard (the `feedback_routes`/`push_routes` pattern): a
/// stray GET on the POST-only mutation must 405 loudly, not fall to a
/// confusing 404 (`feedback_post_only_route_guards` house rule).
pub fn dispatch(path: &str, _params: &HashMap<String, String>) -> Option<CliResponse> {
    match path {
        "/cli/browser/open-url" => Some(CliResponse::method_not_allowed()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_params() -> HashMap<String, String> {
        HashMap::new()
    }

    // ── validation ───────────────────────────────────────────────────

    #[test]
    fn validate_accepts_http_and_https_only() {
        assert_eq!(
            validate_open_url("https://example.com/a?b=1#c").unwrap(),
            "https://example.com/a?b=1#c"
        );
        assert_eq!(
            validate_open_url("http://127.0.0.1:8080/x").unwrap(),
            "http://127.0.0.1:8080/x"
        );
        // Scheme match is case-insensitive but the URL is passed through
        // unmodified.
        assert_eq!(
            validate_open_url("HTTPS://Example.com").unwrap(),
            "HTTPS://Example.com"
        );
        // Leading/trailing whitespace is trimmed, not rejected.
        assert_eq!(
            validate_open_url("  https://example.com  ").unwrap(),
            "https://example.com"
        );
    }

    #[test]
    fn validate_rejects_non_http_schemes_and_garbage() {
        for bad in [
            "",
            "   ",
            "ftp://example.com",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "chrome://settings",
            "example.com",         // no scheme
            "/usr/local/thing",    // bare path
            "http://",             // empty remainder
            "https://",            // empty remainder
            "httpsx://example.com",
        ] {
            assert!(
                validate_open_url(bad).is_err(),
                "must reject {bad:?}"
            );
        }
    }

    #[test]
    fn validate_rejects_oversized_and_control_chars() {
        let long = format!("https://example.com/{}", "a".repeat(MAX_URL_LEN));
        assert!(validate_open_url(&long).is_err(), "over the length cap");
        // Exactly at the cap is fine.
        let at_cap = format!(
            "https://e.com/{}",
            "a".repeat(MAX_URL_LEN - "https://e.com/".len())
        );
        assert_eq!(at_cap.len(), MAX_URL_LEN);
        assert!(validate_open_url(&at_cap).is_ok(), "at the cap is allowed");
        assert!(validate_open_url("https://exam ple.com").is_err(), "inner space");
        assert!(validate_open_url("https://ex\nample.com").is_err(), "newline");
        assert!(validate_open_url("https://ex\x07ample.com").is_err(), "control char");
    }

    // ── handler ──────────────────────────────────────────────────────

    #[test]
    fn handler_accepts_form_body_the_shim_sends() {
        // curl --data-urlencode "url=<...>" → urlencoded form body.
        let resp = handle_open_url(
            &no_params(),
            b"url=https%3A%2F%2Fexample.com%2Fdocs%3Fq%3D1",
            "shim",
        );
        assert_eq!(resp.status, "200 OK", "body: {}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["success"], true);
        assert!(v["subscribers"].is_number());
    }

    #[test]
    fn handler_accepts_json_body_and_query_param() {
        let resp = handle_open_url(
            &no_params(),
            br#"{"url":"https://example.com"}"#,
            "shim",
        );
        assert_eq!(resp.status, "200 OK", "body: {}", resp.body);

        let mut params = no_params();
        params.insert("url".to_string(), "http://localhost:3000".to_string());
        let resp = handle_open_url(&params, b"", "terminal-link");
        assert_eq!(resp.status, "200 OK", "body: {}", resp.body);
    }

    #[test]
    fn handler_rejects_invalid_targets_with_400() {
        for body in [
            &br#"{"url":"file:///etc/passwd"}"#[..],
            &br#"{"url":"javascript:alert(1)"}"#[..],
            &b"url=ftp%3A%2F%2Fx"[..],
            &b""[..], // no url anywhere
        ] {
            let resp = handle_open_url(&no_params(), body, "shim");
            assert_eq!(
                resp.status, "400 Bad Request",
                "body {:?} → {}",
                String::from_utf8_lossy(body),
                resp.body
            );
        }
    }

    #[test]
    fn handler_delivers_the_event_to_a_bus_subscriber() {
        // End-to-end through the broadcast bus: what a
        // /cli/sessions/events subscriber (the connected app) receives.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        rt.block_on(async {
            let probe = format!(
                "https://probe.example/{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            );
            let mut rx = crate::session_events::subscribe();
            let body = serde_json::json!({ "url": probe }).to_string();
            let resp = handle_open_url(&no_params(), body.as_bytes(), "shim");
            assert_eq!(resp.status, "200 OK", "body: {}", resp.body);

            // Drain until our probe arrives (global bus — other tests
            // may interleave events).
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
            loop {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    panic!("did not receive probe OpenUrl in time");
                }
                match tokio::time::timeout(remaining, rx.recv()).await {
                    Ok(Ok(SessionEvent::OpenUrl { url, source })) if url == probe => {
                        assert_eq!(source, "shim");
                        break;
                    }
                    Ok(Ok(_)) => continue, // contamination from another test
                    Ok(Err(_)) => panic!("receiver closed"),
                    Err(_) => panic!("timed out waiting for probe"),
                }
            }
        });
    }

    #[test]
    fn get_chain_dispatch_405s_the_post_only_route() {
        // feedback_post_only_route_guards: a stray GET must 405, never
        // silently mutate or 404.
        let resp = dispatch("/cli/browser/open-url", &no_params())
            .expect("route claimed by browser_routes");
        assert_eq!(resp.status, "405 Method Not Allowed");
        assert!(dispatch("/cli/browser/other", &no_params()).is_none());
    }
}
