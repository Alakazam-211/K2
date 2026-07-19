//! Remote Session Layer 0 + Stage 2 grants + Stage 3 shell PTY.
//!
//! Hard wall: `remote_sessions_enabled` defaults OFF. Drive attempts while
//! OFF always 403 with `REMOTE_SESSIONS_DISABLED`, persist a denial event,
//! and broadcast [`SessionEvent::RemoteSessionAccessDenied`] for owner
//! visibility.
//!
//! Stage 2: owner mints shell grants (`k2rs_…`), lists/revokes them.
//! Stage 3: `shell/spawn` opens a real daemon-user login shell PTY under
//! the grant, binds `sessionId → grantId`, and write/read on that session
//! require a matching live grant (or owner ops token).

use crate::cli_response::CliResponse;
use crate::remote_session_sessions;
use crate::session_events::{self, SessionEvent};
use crate::spawn::{spawn_agent_session_v2_blocking, SpawnWorkspaceSessionRequest};

/// Stable 403 body for Layer 0 / grant denials.
pub fn denied(code: &str, hint: &str) -> CliResponse {
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
pub fn audit_denial(principal_label: &str, code: &str, reason: &str) {
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
    let active_sessions: Vec<serde_json::Value> = remote_session_sessions::list_bindings()
        .into_iter()
        .map(|(session_id, grant_id)| {
            serde_json::json!({
                "sessionId": session_id,
                "grantId": grant_id,
            })
        })
        .collect();
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
/// Kills every grant-bound remote shell and returns the count.
pub fn handle_disable() -> CliResponse {
    let killed = remote_session_sessions::kill_all_remote_sessions();
    if let Err(e) = k2_core::remote_sessions::set_enabled(false) {
        return internal(e);
    }
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "enabled": false,
            "killedSessions": killed,
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
/// Body: `{"id":"rs_…"}`. Kills every PTY bound to the grant.
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
        Ok(grant) => {
            let killed = remote_session_sessions::kill_sessions_for_grant(&grant.id);
            match serde_json::to_string(&serde_json::json!({
                "ok": true,
                "grant": grant,
                "killedSessions": killed,
            })) {
                Ok(body) => CliResponse::ok_json(body),
                Err(e) => internal(format!("serialize revoke: {e}")),
            }
        }
        Err(e) if e.contains("not found") => CliResponse::bad_request(e),
        Err(e) => internal(e),
    }
}

/// Default cwd for remote shells: daemon process HOME (soft default only).
fn default_remote_shell_cwd() -> String {
    if let Some(h) = dirs::home_dir() {
        return h.to_string_lossy().into_owned();
    }
    std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string())
}

/// Resolve optional cwd from spawn body (or HOME). Soft default only —
/// no path-jail enforcement in Stage 3.
fn resolve_spawn_cwd(body: &[u8]) -> String {
    if body.is_empty() || body.iter().all(|b| b.is_ascii_whitespace()) {
        return default_remote_shell_cwd();
    }
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) else {
        return default_remote_shell_cwd();
    };
    v.get("cwd")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(default_remote_shell_cwd)
}

/// `POST /cli/remote-session/shell/spawn` — real daemon-user PTY.
///
/// Check order:
/// 1. Layer 0 OFF → 403 REMOTE_SESSIONS_DISABLED + denial
/// 2. Resolve grant from request token (if k2rs_…)
/// 3. Grant expired/revoked/missing → 403 + denial
/// 4. Valid grant → spawn login shell, bind session↔grant, 200
///    `{ok:true, grantId, sessionId, ready:true}`
///
/// Owner/connect tokens without a grant → NO_GRANT (unchanged from Stage 1).
pub fn handle_shell_spawn(
    principal_label: &str,
    presented_token: Option<&str>,
    body: &[u8],
) -> CliResponse {
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
                return spawn_remote_shell(&grant.id, label, body);
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

/// Open a daemon-user login shell under a synthetic agent key and bind
/// the resulting SessionId to `grant_id`.
fn spawn_remote_shell(grant_id: &str, principal_label: &str, body: &[u8]) -> CliResponse {
    let cwd = resolve_spawn_cwd(body);
    // Soft default: ensure HOME (or ~/.k2/remote-shell) exists so spawn
    // does not fail on a missing directory in a temp-HOME test harness.
    if let Err(e) = std::fs::create_dir_all(&cwd) {
        k2_core::log_debug!("[remote_session] ensure cwd {cwd}: {e}");
    }

    // Unique synthetic name so concurrent shells under the same grant
    // never collide on the v2 map idempotency key.
    let short = uuid::Uuid::new_v4().to_string();
    let short8 = &short[..8];
    let grant_short = grant_id
        .strip_prefix(k2_core::remote_sessions::GRANT_ID_PREFIX)
        .unwrap_or(grant_id);
    let grant_short = if grant_short.len() >= 8 {
        &grant_short[..8]
    } else {
        grant_short
    };
    let agent_name = format!("remote-shell-{grant_short}-{short8}");
    // Unique canonical key — no project_id, so no workspace pin reuse.
    let canonical_key = agent_name.clone();

    let outcome = match spawn_agent_session_v2_blocking(SpawnWorkspaceSessionRequest {
        agent_name: agent_name.clone(),
        project_id: None,
        cwd: cwd.clone(),
        // None → daemon_pty opens the user's login shell ($SHELL).
        command: None,
        args: None,
        cols: 80,
        rows: 24,
        canonical_key: Some(canonical_key),
        env: Default::default(),
    }) {
        Ok(o) => o,
        Err(e) => {
            return CliResponse::bad_request(format!("remote shell spawn failed: {e}"));
        }
    };

    let session_id = outcome.session_id.to_string();
    remote_session_sessions::bind_session(&session_id, grant_id);

    let payload = serde_json::json!({
        "sessionId": session_id,
        "cwd": cwd,
        "agentName": agent_name,
    })
    .to_string();
    if let Err(e) = k2_core::remote_sessions::record_event(
        Some(grant_id),
        principal_label,
        k2_core::remote_sessions::KIND_SPAWN,
        None,
        Some(&payload),
    ) {
        k2_core::log_debug!("[remote_session] spawn event failed: {e}");
    }

    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "grantId": grant_id,
            "sessionId": session_id,
            "ready": true,
        })
        .to_string(),
    )
}

fn no_grant(label: &str) -> CliResponse {
    let hint =
        "Remote Sessions are ON but no grant covers this principal. Ask the owner to mint a grant.";
    audit_denial(label, k2_core::remote_sessions::CODE_NO_GRANT, hint);
    denied(k2_core::remote_sessions::CODE_NO_GRANT, hint)
}

/// Gate for `/cli/terminal/write` and `/cli/terminal/read` on
/// grant-bound remote shells.
///
/// - Non-remote sessions + non-grant token → `None` (normal path).
/// - Non-remote sessions + k2rs_ grant token → 403 NO_GRANT.
/// - Remote session + Layer 0 OFF → 403 REMOTE_SESSIONS_DISABLED
/// - Remote session + owner token → allow (ops break-glass)
/// - Remote session + matching live grant token → allow
/// - Remote session + connect-user / wrong/expired/revoked → 403 + audit
///
/// `owner_token` is the daemon owner credential (dispatcher `state.token`).
/// Returns `Some(403 response)` to short-circuit, `None` to allow.
pub fn gate_remote_session_io(
    session_id: &str,
    presented_token: Option<&str>,
    owner_token: &str,
) -> Option<CliResponse> {
    let label = match presented_token {
        Some(t) if !t.is_empty() => principal_label_from_token(t, owner_token),
        _ => "unknown".to_string(),
    };
    let tok = presented_token.map(str::trim).filter(|t| !t.is_empty());
    let is_grant = tok.is_some_and(k2_core::remote_sessions::is_grant_token);
    let bound_grant_id = remote_session_sessions::grant_for_session(session_id);

    // Layer 0 first whenever this looks like remote-session I/O
    // (bound PTY and/or grant credential), including post-disable
    // unbind so write still reports REMOTE_SESSIONS_DISABLED.
    if (bound_grant_id.is_some() || is_grant) && !k2_core::remote_sessions::is_enabled() {
        let hint = "Remote Sessions are OFF on this device. Ask the owner to run: k2 remote-session enable";
        audit_denial(
            &label,
            k2_core::remote_sessions::CODE_REMOTE_SESSIONS_DISABLED,
            hint,
        );
        return Some(denied(
            k2_core::remote_sessions::CODE_REMOTE_SESSIONS_DISABLED,
            hint,
        ));
    }

    let Some(bound_grant_id) = bound_grant_id else {
        // Not a remote-bound PTY: grant tokens must not freely drive
        // arbitrary owner terminals. Prefer revoked/expired codes when
        // the credential itself is dead (post-revoke kill unbinds first).
        if is_grant {
            if let Some(tok) = tok {
                if let Err(e) = k2_core::remote_sessions::validate_grant_token(tok) {
                    let code = e.code();
                    let hint = e.hint();
                    audit_denial(&label, code, hint);
                    return Some(denied(code, hint));
                }
            }
            let hint = "Grant token is not bound to this session.";
            audit_denial(&label, k2_core::remote_sessions::CODE_NO_GRANT, hint);
            return Some(denied(k2_core::remote_sessions::CODE_NO_GRANT, hint));
        }
        return None;
    };

    let Some(tok) = tok else {
        let hint = "Remote shell requires a matching grant token or owner token.";
        audit_denial(&label, k2_core::remote_sessions::CODE_NO_GRANT, hint);
        return Some(denied(k2_core::remote_sessions::CODE_NO_GRANT, hint));
    };

    // Owner break-glass: may read/write remote shells for ops.
    // Prefer explicit owner_token compare; also accept hook_config token
    // when the production daemon set it at boot.
    if !owner_token.is_empty() && crate::routes::http::ct_eq_token(tok, owner_token) {
        return None;
    }
    let hook_tok = k2_core::hook_config::get_token();
    if !hook_tok.is_empty() && crate::routes::http::ct_eq_token(tok, hook_tok) {
        return None;
    }

    if k2_core::remote_sessions::is_grant_token(tok) {
        match k2_core::remote_sessions::validate_grant_token(tok) {
            Ok(g) if g.id == bound_grant_id => return None,
            Ok(_other) => {
                let hint = "Grant token does not match the session's bound grant.";
                audit_denial(&label, k2_core::remote_sessions::CODE_NO_GRANT, hint);
                return Some(denied(k2_core::remote_sessions::CODE_NO_GRANT, hint));
            }
            Err(e) => {
                let code = e.code();
                let hint = e.hint();
                audit_denial(&label, code, hint);
                return Some(denied(code, hint));
            }
        }
    }

    // Connect-user (or any non-owner non-grant): denied on remote shells.
    // Dispatcher already required token_ok for non-grant tokens, so a
    // connect session reaches here with a valid session — still no I/O
    // without a matching grant.
    let hint = "Remote shell requires a matching grant token or owner token.";
    audit_denial(&label, k2_core::remote_sessions::CODE_NO_GRANT, hint);
    Some(denied(k2_core::remote_sessions::CODE_NO_GRANT, hint))
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
    use std::sync::Mutex;

    /// Serialize tests that flip Layer 0 / mint grants so parallel
    /// lib tests cannot race `is_enabled` into an accidental real PTY
    /// spawn (which needs a Tokio runtime).
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

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
        let _g = lock();
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
    fn grant_create_and_layer0_gate_before_spawn() {
        let _g = lock();
        // Mint while OFF is allowed. Do NOT call real spawn with a valid
        // grant while ON here — that needs a Tokio runtime (integration
        // tests cover the happy path).
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
        let spawn_off = handle_shell_spawn("grant:t", Some(&token), b"{}");
        assert_eq!(spawn_off.status, "403 Forbidden");
        let off_v: serde_json::Value = serde_json::from_str(&spawn_off.body).unwrap();
        assert_eq!(off_v["error"]["code"], "REMOTE_SESSIONS_DISABLED");

        // Owner token without grant → NO_GRANT once ON (no PTY spawn).
        k2_core::remote_sessions::set_enabled(true).unwrap();
        let owner_spawn = handle_shell_spawn("owner", Some("not-a-grant-token"), b"{}");
        assert_eq!(owner_spawn.status, "403 Forbidden");
        let ov: serde_json::Value = serde_json::from_str(&owner_spawn.body).unwrap();
        assert_eq!(ov["error"]["code"], "NO_GRANT");

        // Cleanup switch so other unit tests see default-ish state.
        let _ = k2_core::remote_sessions::set_enabled(false);
        let _ = k2_core::remote_sessions::revoke_grant(&grant_id);
    }

    #[test]
    fn gate_non_remote_session_is_passthrough() {
        let _g = lock();
        remote_session_sessions::clear_for_tests();
        assert!(gate_remote_session_io("not-bound-uuid", Some("tok"), "owner").is_none());
    }

    #[test]
    fn gate_remote_requires_matching_grant_or_owner() {
        let _g = lock();
        remote_session_sessions::clear_for_tests();
        let _ = k2_core::remote_sessions::set_enabled(true);
        let (g, raw) = k2_core::remote_sessions::create_grant(
            k2_core::remote_sessions::SCOPE_SHELL,
            1800,
            Some("gate-t"),
            Some("owner"),
        )
        .unwrap();
        remote_session_sessions::bind_session("sess-gate-1", &g.id);

        // Matching grant → allow.
        assert!(
            gate_remote_session_io("sess-gate-1", Some(&raw), "owner-secret").is_none(),
            "matching grant must pass"
        );
        // Owner → allow.
        assert!(
            gate_remote_session_io("sess-gate-1", Some("owner-secret"), "owner-secret").is_none(),
            "owner must pass"
        );
        // Wrong grant token (mint another).
        let (_g2, raw2) = k2_core::remote_sessions::create_grant(
            k2_core::remote_sessions::SCOPE_SHELL,
            1800,
            None,
            None,
        )
        .unwrap();
        let deny = gate_remote_session_io("sess-gate-1", Some(&raw2), "owner-secret")
            .expect("wrong grant must deny");
        assert_eq!(deny.status, "403 Forbidden");
        let dv: serde_json::Value = serde_json::from_str(&deny.body).unwrap();
        assert_eq!(dv["error"]["code"], "NO_GRANT");

        // Layer 0 off → REMOTE_SESSIONS_DISABLED.
        let _ = k2_core::remote_sessions::set_enabled(false);
        let off = gate_remote_session_io("sess-gate-1", Some(&raw), "owner-secret")
            .expect("disabled must deny");
        let ov: serde_json::Value = serde_json::from_str(&off.body).unwrap();
        assert_eq!(ov["error"]["code"], "REMOTE_SESSIONS_DISABLED");

        // Cleanup.
        remote_session_sessions::unbind_session("sess-gate-1");
        let _ = k2_core::remote_sessions::revoke_grant(&g.id);
        let _ = k2_core::remote_sessions::set_enabled(false);
    }
}
