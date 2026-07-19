//! Remote Session Layer 0 + Stage 2 grant routes.
//!
//! Hard wall: `remote_sessions_enabled` defaults OFF. Drive attempts while
//! OFF always 403 with `REMOTE_SESSIONS_DISABLED`, persist a denial event,
//! and broadcast [`SessionEvent::RemoteSessionAccessDenied`] for owner
//! visibility.
//!
//! Stage 2: owner mints shell grants (`k2rs_…`), lists/revokes them, and
//! `shell/spawn` accepts a valid grant token after Layer 0. No PTY yet —
//! valid grant returns 200 with `ready:false`.

use crate::cli_response::CliResponse;
use crate::session_events::{self, SessionEvent};

/// Stable 403 body for Layer 0 / grant denials.
fn denied(code: &str, hint: &str) -> CliResponse {
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": {
                "code": code,
                "hint": hint,
            },
        })
        .to_string(),
    }
}

fn internal(err: impl std::fmt::Display) -> CliResponse {
    CliResponse::internal_error(err)
}

/// Persist denial + broadcast for owner visibility. Best-effort on emit.
fn audit_denial(principal_label: &str, code: &str, reason: &str) {
    let payload = serde_json::json!({ "reason": reason }).to_string();
    if let Err(e) = k2_core::remote_sessions::record_denial(
        principal_label,
        code,
        Some(&payload),
    ) {
        k2_core::log_debug!("[remote_session] record_denial failed: {e}");
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let _ = session_events::emit(SessionEvent::RemoteSessionAccessDenied {
        principal_label: principal_label.to_string(),
        reason: reason.to_string(),
        code: code.to_string(),
        ts,
    });
}

/// `GET /cli/remote-session/status`
pub fn handle_status() -> CliResponse {
    let enabled = k2_core::remote_sessions::is_enabled();
    let recent_denials = match k2_core::remote_sessions::list_recent_denials(20) {
        Ok(v) => v,
        Err(e) => return internal(e),
    };
    let active_grants = match k2_core::remote_sessions::active_grant_count() {
        Ok(n) => n,
        Err(e) => return internal(e),
    };
    // Stage 2: no live remote sessions yet (PTY lands in Stage 3).
    let active_sessions: Vec<serde_json::Value> = Vec::new();
    match serde_json::to_string(&serde_json::json!({
        "ok": true,
        "enabled": enabled,
        "activeSessions": active_sessions,
        "activeGrants": active_grants,
        "recentDenials": recent_denials,
    })) {
        Ok(body) => CliResponse::ok_json(body),
        Err(e) => internal(format!("serialize status: {e}")),
    }
}

/// `POST /cli/remote-session/enable` — owner-or-admin surface.
pub fn handle_enable() -> CliResponse {
    if let Err(e) = k2_core::remote_sessions::set_enabled(true) {
        return internal(e);
    }
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "enabled": true,
        })
        .to_string(),
    )
}

/// `POST /cli/remote-session/disable` — owner-or-admin surface.
/// Stage 2: no live sessions to kill (`killedSessions: 0`).
pub fn handle_disable() -> CliResponse {
    if let Err(e) = k2_core::remote_sessions::set_enabled(false) {
        return internal(e);
    }
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "enabled": false,
            "killedSessions": 0,
        })
        .to_string(),
    )
}

/// `POST /cli/remote-session/grant` — mint a shell grant (owner-or-admin).
///
/// Body: `{"scope":"shell","ttlSeconds":1800,"label":"optional"}`
/// Mint is allowed while Layer 0 is OFF; use is blocked until enable.
pub fn handle_grant_create(body: &[u8], issued_by: &str) -> CliResponse {
    let v: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) if body.iter().all(|b| b.is_ascii_whitespace()) => serde_json::json!({}),
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };

    let scope = v
        .get("scope")
        .and_then(|x| x.as_str())
        .unwrap_or(k2_core::remote_sessions::SCOPE_SHELL);
    if scope == "runbook" {
        return CliResponse::bad_request("scope 'runbook' is not_implemented (Stage 2 is shell-only)");
    }
    if scope != k2_core::remote_sessions::SCOPE_SHELL {
        return CliResponse::bad_request(format!(
            "unsupported scope {scope:?}; only 'shell' is implemented"
        ));
    }

    let ttl = v
        .get("ttlSeconds")
        .or_else(|| v.get("ttl_seconds"))
        .and_then(|x| x.as_i64())
        .unwrap_or(k2_core::remote_sessions::DEFAULT_TTL_SECS);

    let label = v.get("label").and_then(|x| x.as_str());

    let (grant, token) = match k2_core::remote_sessions::create_grant(
        scope,
        ttl,
        label,
        Some(issued_by),
    ) {
        Ok(pair) => pair,
        Err(e) => return CliResponse::bad_request(e),
    };

    match serde_json::to_string(&serde_json::json!({
        "ok": true,
        "grant": grant,
        "token": token,
    })) {
        Ok(body) => CliResponse::ok_json(body),
        Err(e) => internal(format!("serialize grant create: {e}")),
    }
}

/// `GET /cli/remote-session/grants` — list grants (no secrets/hashes).
pub fn handle_grants_list() -> CliResponse {
    let grants = match k2_core::remote_sessions::list_grants() {
        Ok(v) => v,
        Err(e) => return internal(e),
    };
    match serde_json::to_string(&serde_json::json!({
        "ok": true,
        "grants": grants,
    })) {
        Ok(body) => CliResponse::ok_json(body),
        Err(e) => internal(format!("serialize grants list: {e}")),
    }
}

/// `POST /cli/remote-session/revoke` — revoke by id (owner-or-admin).
/// Body: `{"id":"rs_…"}`
pub fn handle_revoke(body: &[u8]) -> CliResponse {
    let v: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let id = v
        .get("id")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let Some(id) = id else {
        return CliResponse::bad_request("missing 'id'");
    };
    match k2_core::remote_sessions::revoke_grant(id) {
        Ok(grant) => match serde_json::to_string(&serde_json::json!({
            "ok": true,
            "grant": grant,
        })) {
            Ok(body) => CliResponse::ok_json(body),
            Err(e) => internal(format!("serialize revoke: {e}")),
        },
        Err(e) if e.contains("not found") => CliResponse::bad_request(e),
        Err(e) => internal(e),
    }
}

/// `POST /cli/remote-session/shell/spawn` — Stage 2 stub.
///
/// Check order:
/// 1. Layer 0 OFF → 403 REMOTE_SESSIONS_DISABLED + denial
/// 2. Resolve grant from request token (if k2rs_…)
/// 3. Grant expired/revoked/missing → 403 + denial
/// 4. Valid grant → 200 `{ok:true, grantId, ready:false, hint:"… Stage 3"}`
///
/// Owner/connect tokens without a grant → NO_GRANT (unchanged from Stage 1).
pub fn handle_shell_spawn(principal_label: &str, presented_token: Option<&str>) -> CliResponse {
    let label = if principal_label.trim().is_empty() {
        "unknown"
    } else {
        principal_label.trim()
    };

    // 1) Layer 0 first — always.
    if !k2_core::remote_sessions::is_enabled() {
        let hint = "Remote Sessions are OFF on this device. Ask the owner to run: k2 remote-session enable";
        audit_denial(
            label,
            k2_core::remote_sessions::CODE_REMOTE_SESSIONS_DISABLED,
            hint,
        );
        return denied(
            k2_core::remote_sessions::CODE_REMOTE_SESSIONS_DISABLED,
            hint,
        );
    }

    // 2–4) Grant path when a k2rs_ token is presented; otherwise NO_GRANT.
    let Some(tok) = presented_token.map(str::trim).filter(|t| !t.is_empty()) else {
        return no_grant(label);
    };

    if k2_core::remote_sessions::is_grant_token(tok) {
        match k2_core::remote_sessions::validate_grant_token(tok) {
            Ok(grant) => {
                return CliResponse::ok_json(
                    serde_json::json!({
                        "ok": true,
                        "grantId": grant.id,
                        "ready": false,
                        "hint": "shell PTY lands in Stage 3",
                    })
                    .to_string(),
                );
            }
            Err(e) => {
                let code = e.code();
                let hint = e.hint();
                audit_denial(label, code, hint);
                return denied(code, hint);
            }
        }
    }

    // Owner / connect-user token while ON without a grant credential.
    no_grant(label)
}

fn no_grant(label: &str) -> CliResponse {
    let hint =
        "Remote Sessions are ON but no grant covers this principal. Ask the owner to mint a grant.";
    audit_denial(label, k2_core::remote_sessions::CODE_NO_GRANT, hint);
    denied(k2_core::remote_sessions::CODE_NO_GRANT, hint)
}

/// Resolve a short principal label for audit rows from the request token.
pub fn principal_label_from_query(query: &str, owner_token: &str) -> String {
    let Some(tok) = crate::routes::http::extract_token(query) else {
        return "unknown".to_string();
    };
    principal_label_from_token(tok, owner_token)
}

/// Resolve a short principal label from an already-extracted token.
pub fn principal_label_from_token(tok: &str, owner_token: &str) -> String {
    if tok.is_empty() {
        return "unknown".to_string();
    }
    if crate::routes::http::ct_eq_token(tok, owner_token) {
        return "owner".to_string();
    }
    if k2_core::remote_sessions::is_grant_token(tok) {
        // Prefer the grant's label when available; fall back to grant id.
        return match k2_core::remote_sessions::validate_grant_token(tok) {
            Ok(g) => g
                .label
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("grant:{}", g.id)),
            Err(_) => "grant".to_string(),
        };
    }
    if let Some(username) = k2_core::connect_users::validate_session(tok) {
        return username;
    }
    "unknown".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denied_shape_has_stable_code_and_ok_false() {
        let r = denied(
            "REMOTE_SESSIONS_DISABLED",
            "Remote Sessions are OFF on this device. Ask the owner to run: k2 remote-session enable",
        );
        assert_eq!(r.status, "403 Forbidden");
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"]["code"], "REMOTE_SESSIONS_DISABLED");
        let hint = v["error"]["hint"].as_str().unwrap_or("");
        assert!(
            hint.contains("Remote Sessions are OFF"),
            "hint={hint}"
        );
    }

    #[test]
    fn grant_create_rejects_runbook_scope() {
        let body = br#"{"scope":"runbook","ttlSeconds":60}"#;
        let r = handle_grant_create(body, "owner");
        assert_eq!(r.status, "400 Bad Request");
        assert!(
            r.body.contains("not_implemented") || r.body.contains("runbook"),
            "body={}",
            r.body
        );
    }

    #[test]
    fn grant_create_and_spawn_happy_path() {
        // Mint while OFF is allowed.
        let _ = k2_core::remote_sessions::set_enabled(false);
        let body = br#"{"scope":"shell","ttlSeconds":1800,"label":"t"}"#;
        let r = handle_grant_create(body, "owner");
        assert_eq!(r.status, "200 OK", "body={}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(v["ok"], true);
        let token = v["token"].as_str().expect("token").to_string();
        assert!(token.starts_with("k2rs_"));
        let grant_id = v["grant"]["id"].as_str().expect("id").to_string();

        // Use blocked while OFF.
        let spawn_off = handle_shell_spawn("grant:t", Some(&token));
        assert_eq!(spawn_off.status, "403 Forbidden");
        let off_v: serde_json::Value = serde_json::from_str(&spawn_off.body).unwrap();
        assert_eq!(off_v["error"]["code"], "REMOTE_SESSIONS_DISABLED");

        // Enable + valid grant → ready:false.
        k2_core::remote_sessions::set_enabled(true).unwrap();
        let spawn_on = handle_shell_spawn("grant:t", Some(&token));
        assert_eq!(spawn_on.status, "200 OK", "body={}", spawn_on.body);
        let on_v: serde_json::Value = serde_json::from_str(&spawn_on.body).unwrap();
        assert_eq!(on_v["ok"], true);
        assert_eq!(on_v["ready"], false);
        assert_eq!(on_v["grantId"], grant_id);
        assert!(
            on_v["hint"]
                .as_str()
                .unwrap_or("")
                .to_ascii_lowercase()
                .contains("stage 3"),
            "hint={}",
            on_v["hint"]
        );

        // Owner token without grant → NO_GRANT.
        let owner_spawn = handle_shell_spawn("owner", Some("not-a-grant-token"));
        assert_eq!(owner_spawn.status, "403 Forbidden");
        let ov: serde_json::Value = serde_json::from_str(&owner_spawn.body).unwrap();
        assert_eq!(ov["error"]["code"], "NO_GRANT");

        // Cleanup switch so other unit tests see default-ish state.
        let _ = k2_core::remote_sessions::set_enabled(false);
        let _ = k2_core::remote_sessions::revoke_grant(&grant_id);
    }
}
