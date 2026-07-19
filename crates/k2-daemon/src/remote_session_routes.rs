//! Remote Session Layer 0 — status / enable / disable / shell spawn stub.
//!
//! Hard wall: `remote_sessions_enabled` defaults OFF. Drive attempts while
//! OFF always 403 with `REMOTE_SESSIONS_DISABLED`, persist a denial event,
//! and broadcast [`SessionEvent::RemoteSessionAccessDenied`] for owner
//! visibility. Stage 1 does not open a PTY; when ON without a grant the
//! spawn stub returns `NO_GRANT` so Layer 0 is testable separately from
//! Stage 2 grant minting.

use crate::cli_response::CliResponse;
use crate::session_events::{self, SessionEvent};

/// Stable 403 body for Layer 0 hard-wall denials.
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
    // Stage 1: no live remote sessions yet.
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
/// Stage 1: no live sessions to kill (`killedSessions: 0`).
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

/// `POST /cli/remote-session/shell/spawn` — stub.
///
/// - Layer 0 OFF → 403 `REMOTE_SESSIONS_DISABLED` + denial audit
/// - Layer 0 ON  → 403 `NO_GRANT` (Stage 2 mints grants; no PTY here)
pub fn handle_shell_spawn(principal_label: &str) -> CliResponse {
    let label = if principal_label.trim().is_empty() {
        "unknown"
    } else {
        principal_label.trim()
    };

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

    // Stage 1: grants not minted yet — always NO_GRANT when ON.
    let hint =
        "Remote Sessions are ON but no grant covers this principal. Ask the owner to mint a grant (Stage 2).";
    audit_denial(
        label,
        k2_core::remote_sessions::CODE_NO_GRANT,
        hint,
    );
    denied(k2_core::remote_sessions::CODE_NO_GRANT, hint)
}

/// Resolve a short principal label for audit rows from the request token.
pub fn principal_label_from_query(query: &str, owner_token: &str) -> String {
    let Some(tok) = crate::routes::http::extract_token(query) else {
        return "unknown".to_string();
    };
    if tok.is_empty() {
        return "unknown".to_string();
    }
    if crate::routes::http::ct_eq_token(tok, owner_token) {
        return "owner".to_string();
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
}
