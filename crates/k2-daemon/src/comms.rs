//! C2 (0.40.45) — connection-required local agent comms.
//!
//! Binary edge model (Option A) for **agent-initiated** same-daemon
//! cross-workspace traffic:
//!
//! - Same workspace → always allowed
//! - Peers via `workspace_relations` / [`k2_core::connections::list_peers`]
//!   → allowed
//! - Otherwise → 403 `not_connected` teaching (CLI exit 3)
//! - Owner / owner-or-admin human path (no scoped principal) → bypass
//!   (Phase 2 residual: ambient owner token still skips the peer check)
//!
//! Caller identity is the stamped [`HookPrincipal::workspace_uuid`] —
//! never free-text `--from` / client-supplied identity.

use crate::cli_response::CliResponse;
use crate::session_token::HookPrincipal;

/// Wire code for the local peer gate. CLI maps this (and `gated`) to
/// exit 3 with teaching stderr — same family as mail/dns denials.
pub const NOT_CONNECTED: &str = "not_connected";

/// Teaching error from [`require_local_peer`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TeachingError {
    /// Caller is not connected to the target workspace.
    NotConnected { hint: String },
}

impl TeachingError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotConnected { .. } => NOT_CONNECTED,
        }
    }

    pub fn hint(&self) -> &str {
        match self {
            Self::NotConnected { hint } => hint,
        }
    }

    pub fn into_response(self) -> CliResponse {
        CliResponse {
            status: "403 Forbidden",
            content_type: "application/json",
            body: serde_json::json!({
                "ok": false,
                "error": { "code": self.code(), "hint": self.hint() },
            })
            .to_string(),
        }
    }
}

/// Canonical teaching hint for a denied local peer edge.
pub fn not_connected_hint(target_display: &str) -> String {
    let name = target_display.trim();
    let shown = if name.is_empty() { "<workspace>" } else { name };
    format!(
        "'{shown}' is not a local connection — add it with \
         `k2 connections add {shown}` (or ask your human: Settings → \
         Connections). Same-workspace is always allowed."
    )
}

/// Pure peer gate: same id OK; peers via `workspace_relations` OK;
/// else [`TeachingError::NotConnected`].
///
/// `target_display` is only used in the teaching hint (name preferred).
pub fn require_local_peer(
    caller_project_id: &str,
    target_project_id: &str,
    target_display: &str,
) -> Result<(), TeachingError> {
    if k2_core::connections::are_local_peers(caller_project_id, target_project_id) {
        return Ok(());
    }
    Err(TeachingError::NotConnected {
        hint: not_connected_hint(target_display),
    })
}

/// Route-level gate for agent-initiated cross-workspace ops.
///
/// * `principal == None` → **owner / ambient path: bypass** (document as
///   residual until Phase 2 deprecates ambient owner tokens on agent-looking
///   paths).
/// * `principal == Some` → enforce [`require_local_peer`] against
///   `principal.workspace_uuid` and the resolved target project id.
///
/// Returns `Ok(())` when allowed, or a ready-to-send 403 `CliResponse`.
pub fn require_local_peer_for_principal(
    principal: Option<&HookPrincipal>,
    target_project_id: &str,
    target_display: &str,
) -> Result<(), CliResponse> {
    let Some(p) = principal else {
        // Owner / owner-or-admin / unscoped ambient path — bypass.
        return Ok(());
    };
    let caller = p.workspace_uuid.trim();
    if caller.is_empty() {
        return Err(CliResponse {
            status: "403 Forbidden",
            content_type: "application/json",
            body: serde_json::json!({
                "ok": false,
                "error": {
                    "code": "forbidden",
                    "hint": "scoped session principal has no resolvable workspace",
                },
            })
            .to_string(),
        });
    }
    require_local_peer(caller, target_project_id, target_display).map_err(TeachingError::into_response)
}

/// Resolve a workspace token (name | path | uuid) to `(path, project_id)`,
/// or `None` if unregistered.
pub fn resolve_target_project(token: &str) -> Option<(String, String)> {
    // `sales/reviewer` is a sidecar address — peer-gate on the workspace.
    let token = k2_core::workspace_session_handles::split_workspace_handle(token)
        .map(|(ws, _)| ws)
        .unwrap_or(token);
    let path = crate::workspace_msg::resolve_workspace(token)?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let id = k2_core::workspace::agent_identity::resolve_project_id(&conn, &path)?;
    Some((path, id))
}

/// Gate helper used by route handlers: principal + target token.
///
/// When the target token does not resolve, returns `None` so the caller
/// can fall through to its existing not-found path (do not leak
/// "not connected" for unknown workspaces).
pub fn gate_cross_workspace(
    principal: Option<&HookPrincipal>,
    target_token: &str,
) -> Result<(), Option<CliResponse>> {
    let Some((_path, target_id)) = resolve_target_project(target_token) else {
        return Err(None);
    };
    // Prefer a human-readable name in the hint when available.
    let display = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT name FROM projects WHERE id = ?1",
            rusqlite::params![target_id],
            |r| r.get::<_, String>(0),
        )
        .unwrap_or_else(|_| target_token.trim().to_string())
    };
    require_local_peer_for_principal(principal, &target_id, &display).map_err(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn seed_project(name: &str) -> (String, String) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let path = format!("/tmp/comms-peer-{}-{}", name, Uuid::new_v4());
        let id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, name, path],
        )
        .expect("seed project");
        (path, id)
    }

    fn link(a: &str, b: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let rid = Uuid::new_v4().to_string();
        k2_core::db::schema::WorkspaceRelation::create(&conn, &rid, a, b, "collaborator")
            .expect("link peers");
    }

    #[test]
    fn require_local_peer_same_workspace_ok() {
        let (_p, id) = seed_project("same-ws");
        require_local_peer(&id, &id, "same-ws").expect("same workspace always allowed");
    }

    #[test]
    fn require_local_peer_not_peers_denied() {
        let (_pa, a) = seed_project("alpha");
        let (_pb, b) = seed_project("beta");
        let err = require_local_peer(&a, &b, "beta").expect_err("strangers denied");
        assert_eq!(err.code(), NOT_CONNECTED);
        assert!(
            err.hint().contains("k2 connections add"),
            "hint should teach add: {}",
            err.hint()
        );
        assert!(
            err.hint().contains("beta") || err.hint().contains("Settings"),
            "hint should name target or Settings: {}",
            err.hint()
        );
    }

    #[test]
    fn require_local_peer_peers_ok() {
        let (_pa, a) = seed_project("peer-a");
        let (_pb, b) = seed_project("peer-b");
        link(&a, &b);
        require_local_peer(&a, &b, "peer-b").expect("linked peers allowed");
        // Bidirectional awareness — reverse direction also ok.
        require_local_peer(&b, &a, "peer-a").expect("reverse also allowed");
    }

    #[test]
    fn principal_none_bypasses_owner_path() {
        let (_p, id) = seed_project("owner-bypass");
        // No principal → owner ambient residual (Phase 2).
        assert!(
            require_local_peer_for_principal(None, &id, "owner-bypass").is_ok(),
            "owner path bypasses peer check"
        );
    }

    #[test]
    fn principal_enforced_when_not_peers() {
        let (_pa, a) = seed_project("scoped-a");
        let (_pb, b) = seed_project("scoped-b");
        let principal = HookPrincipal {
            workspace_uuid: a,
            agent_address: "scoped-a".into(),
        };
        let err = match require_local_peer_for_principal(Some(&principal), &b, "scoped-b") {
            Ok(()) => panic!("scoped non-peer must be denied"),
            Err(e) => e,
        };
        assert_eq!(err.status, "403 Forbidden");
        assert!(
            err.body.contains(NOT_CONNECTED),
            "body should carry not_connected: {}",
            err.body
        );
    }

    #[test]
    fn teaching_response_shape_is_mail_dns_compatible() {
        let err = TeachingError::NotConnected {
            hint: not_connected_hint("other"),
        };
        let resp = err.into_response();
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("json");
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"]["code"], NOT_CONNECTED);
        assert!(v["error"]["hint"].as_str().unwrap().contains("connections add"));
    }
}
