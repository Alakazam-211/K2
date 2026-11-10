//! The wire contract (PRD §4, §6b, §7.1): what this box sends, to whom,
//! and how each answer is read. Public on purpose: people can read exactly
//! what their box sends.
//!
//! 1. **Box token** (PF11–PF12): `POST <connect>/api/identity/assert` with
//!    the tunnel bearer and `X-K2-Subdomain` (push S0 auth), body
//!    `{aud}`; answer `{token, expiresAt}`, a 5-minute ES256 JWT naming
//!    this box's label. The tunnel token itself never goes to akzm.
//! 2. **Submission** (§7.1): `POST <intake>/v1/submissions` with
//!    `Authorization: K2-Box <jwt>`, `Idempotency-Key: <clientId>` and the
//!    JSON body from [`submission_body`]. Answers: `201 {id}`, `200 {id,
//!    duplicate: true}`, `400 {code}`, `401 {code}`, `403 box_blocked`,
//!    `413`, `429 {retryAfterSecs, scope}`, `503 intake_paused
//!    {retryAfterSecs}`.
//!
//! Redirects are never followed (an intake must not steer the box to
//! another host). Before the intake or the relay route exists, both answer
//! 404 or fail to resolve: that is [`Outcome::Retry`] with
//! `service_unreachable` / `relay_not_ready`, never a refusal, so nothing
//! written is lost.

use std::time::Duration;

use serde_json::{json, Value as J};

use super::diagnostics::Section;
use super::store::Row;

pub const SUBMISSIONS_PATH: &str = "/v1/submissions";
pub const ASSERT_PATH: &str = "/api/identity/assert";
const TIMEOUT: Duration = Duration::from_secs(20);

/// Retryable-failure codes kept in `last_error` (`<code>: <detail>`).
pub const E_UNREACHABLE: &str = "service_unreachable";
pub const E_RELAY_NOT_READY: &str = "relay_not_ready";
pub const E_RELAY_REFUSED: &str = "relay_refused";
pub const E_RATE_LIMITED: &str = "rate_limited";
pub const E_PAUSED: &str = "intake_paused";
pub const E_TOKEN: &str = "token_rejected";

/// The submission's JSON body: the row's fields, its stored versions
/// (`meta_json`) and, only when the person consented, the diagnostics
/// exactly as they were previewed.
pub fn submission_body(row: &Row) -> J {
    let mut body = json!({
        "clientId": row.id,
        "category": row.category,
        "text": row.body,
        "senderKind": row.sender_kind,
        "client": row.client,
    });
    let obj = body.as_object_mut().expect("object");
    if let Some(e) = row.contact_email.as_deref().filter(|e| !e.is_empty()) {
        obj.insert("contactEmail".into(), json!(e));
    }
    if let Ok(J::Object(meta)) = serde_json::from_str::<J>(&row.meta_json) {
        for k in ["k2Version", "daemonVersion", "installKind", "os", "osVersion", "arch", "locale"] {
            if let Some(v) = meta.get(k).filter(|v| v.is_string()) {
                obj.insert(k.into(), v.clone());
            }
        }
    }
    if let Some(sections) = row
        .diagnostics_json
        .as_deref()
        .and_then(|d| serde_json::from_str::<Vec<Section>>(d).ok())
        .filter(|s| !s.is_empty())
    {
        let sections: Vec<J> = sections.iter().map(|s| json!({"id": s.id, "title": s.title, "text": s.text})).collect();
        obj.insert("diagnostics".into(), json!({ "sections": sections }));
    }
    body
}

/// How one submission attempt ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// `201 {id}` or `200 {id, duplicate: true}`.
    Sent { remote_id: String, duplicate: bool },
    /// Try again later. `after_secs` is the intake's `retryAfterSecs`;
    /// `reauth` = drop the box token and fetch a new one.
    Retry { code: String, detail: String, after_secs: Option<u64>, reauth: bool },
    /// The intake understood and rejected it: never retried.
    Refused { code: String, detail: String },
}

fn json_code(body: &str) -> Option<String> {
    let v: J = serde_json::from_str(body).ok()?;
    v.get("code")
        .or_else(|| v.get("error"))
        .and_then(|c| c.as_str())
        .map(|s| s.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').take(64).collect())
}

fn retry_after(body: &str, header: Option<&str>) -> Option<u64> {
    let from_body = serde_json::from_str::<J>(body)
        .ok()
        .and_then(|v| v.get("retryAfterSecs").and_then(|n| n.as_u64()));
    from_body
        .or_else(|| header.and_then(|h| h.trim().parse::<u64>().ok()))
        .map(|s| s.min(7 * 86_400))
}

/// Read an intake answer (pure).
pub fn classify(status: u16, body: &str, retry_after_header: Option<&str>) -> Outcome {
    let code = json_code(body);
    match status {
        200 | 201 => {
            let v: Option<J> = serde_json::from_str(body).ok();
            let id = v.as_ref().and_then(|v| v.get("id")).and_then(|i| i.as_str()).filter(|s| !s.is_empty());
            match id {
                Some(id) => Outcome::Sent {
                    remote_id: id.chars().take(128).collect(),
                    duplicate: v.as_ref().and_then(|v| v.get("duplicate")).and_then(|d| d.as_bool()).unwrap_or(false),
                },
                // Not our intake (a captive portal, a placeholder page).
                None => Outcome::Retry {
                    code: E_UNREACHABLE.into(),
                    detail: format!("the feedback service answered {status} without an id"),
                    after_secs: None,
                    reauth: false,
                },
            }
        }
        401 => Outcome::Retry {
            code: E_TOKEN.into(),
            detail: code.unwrap_or_else(|| "bad_assertion".into()),
            after_secs: None,
            reauth: true,
        },
        429 => Outcome::Retry {
            code: E_RATE_LIMITED.into(),
            detail: serde_json::from_str::<J>(body)
                .ok()
                .and_then(|v| v.get("scope").and_then(|s| s.as_str()).map(|s| format!("scope {s}")))
                .unwrap_or_else(|| "too many submissions".into()),
            after_secs: retry_after(body, retry_after_header),
            reauth: false,
        },
        503 if code.as_deref() == Some("intake_paused") => Outcome::Retry {
            code: E_PAUSED.into(),
            detail: "Alakazam paused the feedback intake".into(),
            after_secs: retry_after(body, retry_after_header),
            reauth: false,
        },
        // Not deployed yet, or a proxy in front of nothing: keep it.
        404 | 405 | 502 | 503 | 504 => Outcome::Retry {
            code: E_UNREACHABLE.into(),
            detail: format!("the feedback service answered {status}"),
            after_secs: retry_after(body, retry_after_header),
            reauth: false,
        },
        400..=499 => Outcome::Refused {
            code: code.clone().unwrap_or_else(|| format!("http_{status}")),
            detail: match status {
                403 => "the feedback service refused this server".into(),
                413 => "the feedback was too large for the service".into(),
                _ => format!("the feedback service refused it ({status})"),
            },
        },
        _ => Outcome::Retry {
            code: E_UNREACHABLE.into(),
            detail: format!("the feedback service answered {status}"),
            after_secs: None,
            reauth: false,
        },
    }
}

fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(TIMEOUT)
        .build()
        .map_err(|e| format!("build http client: {e}"))
}

/// One submission attempt.
pub fn post_submission(endpoint: &str, jwt: &str, row: &Row) -> Outcome {
    let url = format!("{}{SUBMISSIONS_PATH}", endpoint.trim_end_matches('/'));
    let http = match client() {
        Ok(c) => c,
        Err(e) => return Outcome::Retry { code: E_UNREACHABLE.into(), detail: e, after_secs: None, reauth: false },
    };
    let body = submission_body(row).to_string();
    match http
        .post(&url)
        .header("Authorization", format!("K2-Box {jwt}"))
        .header("Idempotency-Key", row.id.as_str())
        .header("Content-Type", "application/json")
        .body(body)
        .send()
    {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let ra = resp.headers().get("retry-after").and_then(|v| v.to_str().ok()).map(str::to_string);
            let text = resp.text().unwrap_or_default();
            classify(status, &text, ra.as_deref())
        }
        Err(e) => Outcome::Retry {
            code: E_UNREACHABLE.into(),
            detail: short_transport_error(&e),
            after_secs: None,
            reauth: false,
        },
    }
}

fn short_transport_error(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "the feedback service did not answer in time".into()
    } else if e.is_connect() {
        "can't connect to the feedback service".into()
    } else {
        "can't reach the feedback service".into()
    }
}

/// A box token from the relay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxToken {
    pub token: String,
    /// Milliseconds since the epoch.
    pub expires_at_ms: i64,
}

/// Why no box token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenError {
    /// The relay has no assert route yet (404/405/501).
    NotReady(String),
    /// The relay refused this box's tunnel token (401/403).
    Refused(String),
    /// 429, with the wait.
    RateLimited(Option<u64>),
    /// Transport failure or a 5xx.
    Unreachable(String),
}

impl TokenError {
    /// As a drain [`Outcome::Retry`].
    pub fn as_retry(&self) -> Outcome {
        let (code, detail, after) = match self {
            TokenError::NotReady(d) => (E_RELAY_NOT_READY, d.clone(), None),
            TokenError::Refused(d) => (E_RELAY_REFUSED, d.clone(), None),
            TokenError::RateLimited(a) => (E_RATE_LIMITED, "the K2 Connect relay asked to wait".to_string(), *a),
            TokenError::Unreachable(d) => (E_RELAY_NOT_READY, d.clone(), None),
        };
        Outcome::Retry { code: code.into(), detail, after_secs: after, reauth: false }
    }
}

/// Parse an `expiresAt` (seconds, milliseconds or RFC 3339), else the
/// JWT's own `exp`, else 4 minutes from `now`.
pub fn token_expiry(expires_at: Option<&J>, jwt: &str, now_ms: i64) -> i64 {
    let from_field = expires_at.and_then(|v| match v {
        J::Number(n) => n.as_i64().map(|n| if n > 100_000_000_000 { n } else { n * 1000 }),
        J::String(s) => chrono::DateTime::parse_from_rfc3339(s).ok().map(|d| d.timestamp_millis()),
        _ => None,
    });
    from_field
        .or_else(|| jwt_exp_ms(jwt))
        .unwrap_or(now_ms + 240_000)
}

fn jwt_exp_ms(jwt: &str) -> Option<i64> {
    use base64::Engine;
    let payload = jwt.split('.').nth(1)?;
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    let v: J = serde_json::from_slice(&raw).ok()?;
    v.get("exp")?.as_i64().map(|s| s * 1000)
}

/// Read a relay answer (pure).
pub fn classify_token(status: u16, body: &str, retry_after_header: Option<&str>, now_ms: i64) -> Result<BoxToken, TokenError> {
    match status {
        200 | 201 => {
            let v: J = serde_json::from_str(body).map_err(|_| TokenError::Unreachable("the relay answered without a token".into()))?;
            let token = v
                .get("token")
                .and_then(|t| t.as_str())
                .filter(|t| t.split('.').count() == 3)
                .ok_or_else(|| TokenError::Unreachable("the relay answered without a token".into()))?
                .to_string();
            let expires_at_ms = token_expiry(v.get("expiresAt"), &token, now_ms);
            Ok(BoxToken { token, expires_at_ms })
        }
        404 | 405 | 501 => Err(TokenError::NotReady(format!(
            "K2 Connect can't vouch for this server yet (the relay answered {status})"
        ))),
        401 | 403 => Err(TokenError::Refused(
            json_code(body).unwrap_or_else(|| format!("the relay refused this server ({status})")),
        )),
        429 => Err(TokenError::RateLimited(retry_after(body, retry_after_header))),
        _ => Err(TokenError::Unreachable(format!("the K2 Connect relay answered {status}"))),
    }
}

/// `POST <connect>/api/identity/assert` with the tunnel bearer.
pub fn fetch_token(connect_base: &str, tunnel_token: &str, label: &str, aud: &str, now_ms: i64) -> Result<BoxToken, TokenError> {
    let url = format!("{}{ASSERT_PATH}", connect_base.trim_end_matches('/'));
    let http = client().map_err(TokenError::Unreachable)?;
    match http
        .post(&url)
        .header("Authorization", format!("Bearer {tunnel_token}"))
        .header("X-K2-Subdomain", label)
        .header("Content-Type", "application/json")
        .body(json!({ "aud": aud }).to_string())
        .send()
    {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let ra = resp.headers().get("retry-after").and_then(|v| v.to_str().ok()).map(str::to_string);
            let text = resp.text().unwrap_or_default();
            classify_token(status, &text, ra.as_deref(), now_ms)
        }
        Err(e) => Err(TokenError::Unreachable(if e.is_timeout() {
            "the K2 Connect relay did not answer in time".into()
        } else {
            "can't reach the K2 Connect relay".into()
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::product_feedback::store::State;

    fn row() -> Row {
        Row {
            id: "pf_0123456789012345678901".into(),
            created_at: 1,
            queued_at: 1,
            updated_at: 1,
            sender_kind: "agent".into(),
            sender: "agent:ws-secret-id".into(),
            client: "cli".into(),
            category: "bug".into(),
            body: "the tab froze".into(),
            contact_email: None,
            meta_json: r#"{"k2Version":"0.46.11","daemonVersion":"0.46.11","installKind":"app","os":"macos","osVersion":"26.1","arch":"aarch64","locale":"en-US","extra":"never"}"#.into(),
            state: State::Queued,
            attempts: 0,
            next_attempt_at: 1,
            last_error: None,
            remote_id: None,
            sent_at: None,
            diagnostics_json: None,
        }
    }

    #[test]
    fn the_body_carries_exactly_the_contract_fields() {
        let b = submission_body(&row());
        let keys: Vec<&String> = b.as_object().unwrap().keys().collect();
        let mut keys: Vec<&str> = keys.iter().map(|k| k.as_str()).collect();
        keys.sort();
        assert_eq!(
            keys,
            vec!["arch", "category", "client", "clientId", "daemonVersion", "installKind", "k2Version", "locale", "os", "osVersion", "senderKind", "text"]
        );
        assert!(!b.to_string().contains("ws-secret-id"), "the sender stays on the box: {b}");
        let mut r = row();
        r.contact_email = Some("a@b.co".into());
        r.diagnostics_json = Some(r#"[{"id":"versions","title":"Versions","text":"Daemon: 1"}]"#.into());
        let b = submission_body(&r);
        assert_eq!(b["contactEmail"], json!("a@b.co"));
        assert_eq!(b["diagnostics"], json!({"sections": [{"id": "versions", "title": "Versions", "text": "Daemon: 1"}]}));
    }

    #[test]
    fn intake_answers() {
        assert_eq!(classify(201, r#"{"id":"u1"}"#, None), Outcome::Sent { remote_id: "u1".into(), duplicate: false });
        assert_eq!(classify(200, r#"{"id":"u1","duplicate":true}"#, None), Outcome::Sent { remote_id: "u1".into(), duplicate: true });
        assert!(matches!(classify(200, "<html>portal</html>", None), Outcome::Retry { ref code, .. } if code == E_UNREACHABLE));
        assert!(matches!(classify(401, r#"{"code":"replayed"}"#, None), Outcome::Retry { reauth: true, ref detail, .. } if detail == "replayed"));
        assert_eq!(
            classify(429, r#"{"retryAfterSecs":120,"scope":"box"}"#, None),
            Outcome::Retry { code: E_RATE_LIMITED.into(), detail: "scope box".into(), after_secs: Some(120), reauth: false }
        );
        assert!(matches!(classify(429, "", Some("30")), Outcome::Retry { after_secs: Some(30), .. }));
        assert!(matches!(classify(503, r#"{"code":"intake_paused","retryAfterSecs":600}"#, None), Outcome::Retry { ref code, after_secs: Some(600), .. } if code == E_PAUSED));
        for s in [404, 405, 502, 503, 504, 500] {
            assert!(matches!(classify(s, "nope", None), Outcome::Retry { ref code, .. } if code == E_UNREACHABLE), "{s}");
        }
        assert!(matches!(classify(400, r#"{"code":"bad_category"}"#, None), Outcome::Refused { ref code, .. } if code == "bad_category"));
        assert!(matches!(classify(403, r#"{"code":"box_blocked"}"#, None), Outcome::Refused { ref code, .. } if code == "box_blocked"));
        assert!(matches!(classify(413, "", None), Outcome::Refused { ref code, .. } if code == "http_413"));
    }

    #[test]
    fn relay_answers() {
        let now = 1_000_000;
        let t = classify_token(200, r#"{"token":"a.b.c","expiresAt":1700000300}"#, None, now).unwrap();
        assert_eq!(t, BoxToken { token: "a.b.c".into(), expires_at_ms: 1_700_000_300_000 });
        let t = classify_token(200, r#"{"token":"a.b.c","expiresAt":"2026-11-10T00:05:00Z"}"#, None, now).unwrap();
        assert_eq!(t.expires_at_ms, chrono::DateTime::parse_from_rfc3339("2026-11-10T00:05:00Z").unwrap().timestamp_millis());
        assert_eq!(classify_token(200, r#"{"token":"a.b.c"}"#, None, now).unwrap().expires_at_ms, now + 240_000);
        assert!(matches!(classify_token(200, r#"{"token":"opaque"}"#, None, now), Err(TokenError::Unreachable(_))));
        assert!(matches!(classify_token(404, "", None, now), Err(TokenError::NotReady(_))));
        assert!(matches!(classify_token(401, r#"{"code":"bad_token"}"#, None, now), Err(TokenError::Refused(ref c)) if c == "bad_token"));
        assert_eq!(classify_token(429, r#"{"retryAfterSecs":60}"#, None, now), Err(TokenError::RateLimited(Some(60))));
        assert!(matches!(classify_token(502, "", None, now), Err(TokenError::Unreachable(_))));
    }

    #[test]
    fn jwt_exp_is_read_when_the_field_is_missing() {
        use base64::Engine;
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"exp":1800000000}"#);
        let jwt = format!("eyJhbGciOiJFUzI1NiJ9.{payload}.sig");
        assert_eq!(token_expiry(None, &jwt, 0), 1_800_000_000_000);
    }
}
