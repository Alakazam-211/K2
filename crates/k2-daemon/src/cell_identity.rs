//! Spawn-time cell identity env (K2_CELL / K2_SIDECAR_NAME / K2_PRIMARY /
//! K2_SESSION_ID). Identity is env + `k2 whoami` — never a spawn prompt.
//!
//! `K2_SESSION_ID` prefers the **provider conversation id** (claude
//! `--resume` uuid / `workspace_tab_sessions.session_id`). The daemon
//! PTY id stays what `sessions live` / inject use.

use std::collections::HashMap;

use k2_core::session::SessionId;
use k2_core::workspace_session_handles::{
    conversation_key_for, ensure_sidecar_handle, format_address, handle_for_session,
    is_canonical_agent_name, is_sidecar_harness, workspace_address_name,
};

use crate::cli_response::CliResponse;
use crate::session_lookup;

/// Kind of cell we are about to spawn. Blank shells / file tabs are
/// `None` — do not set identity env.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CellKind {
    Canonical,
    Sidecar,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellIdentity {
    pub cell: CellKind,
    pub sidecar_name: Option<String>,
    pub primary: String,
    /// Provider conversation id when known; else daemon PTY id.
    pub session_id: String,
}

/// Apply identity env. Sidecar spawn MUST set `K2_CELL=sidecar`.
/// Canonical sets `K2_CELL=canonical` and does **not** set
/// `K2_SIDECAR_NAME`. Never injects PTY bytes.
pub fn apply_cell_identity_env(env: &mut HashMap<String, String>, identity: &CellIdentity) {
    match identity.cell {
        CellKind::Canonical => {
            env.insert("K2_CELL".to_string(), "canonical".to_string());
            env.remove("K2_SIDECAR_NAME");
        }
        CellKind::Sidecar => {
            env.insert("K2_CELL".to_string(), "sidecar".to_string());
            if let Some(name) = identity.sidecar_name.as_deref() {
                env.insert("K2_SIDECAR_NAME".to_string(), name.to_string());
            }
        }
    }
    env.insert("K2_PRIMARY".to_string(), identity.primary.clone());
    // Never clobber a host-session / API-curated K2_SESSION_ID (D16).
    if !env.contains_key("K2_API_CELL") {
        env.insert("K2_SESSION_ID".to_string(), identity.session_id.clone());
    }
}

/// Classify this spawn and (for sidecars) allocate/reuse a handle.
///
/// - Canonical / pinned: `agent_name == project_id`
/// - Sidecar: extra `tab-*` **harness** session
/// - `/v1` `api-*` host-sessions and blank shells: `None` (omit overlay)
pub fn resolve_spawn_identity(
    project_id: &str,
    agent_name: &str,
    command: Option<&str>,
    args: &[String],
    daemon_session_id: &str,
    pane_or_tab_key: &str,
) -> Option<CellIdentity> {
    if project_id.trim().is_empty() {
        return None;
    }
    if k2_core::workspace_session_handles::is_api_agent_name(agent_name) {
        return None;
    }
    let provider_sid = k2_core::workspace::provider_resume::session_id_from_spawn_argv(
        command.unwrap_or(""),
        args,
    );
    let session_id = provider_sid
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| daemon_session_id.to_string());

    let primary = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        workspace_address_name(&conn, project_id).ok()
    }?;

    if is_canonical_agent_name(agent_name, project_id) {
        return Some(CellIdentity {
            cell: CellKind::Canonical,
            sidecar_name: None,
            primary,
            session_id,
        });
    }

    if is_sidecar_harness(agent_name, project_id, command) {
        let key = conversation_key_for(provider_sid.as_deref(), pane_or_tab_key);
        let sidecar_name = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            ensure_sidecar_handle(
                &conn,
                project_id,
                agent_name,
                command,
                provider_sid.as_deref(),
                &key,
            )
            .ok()
            .flatten()
        };
        return Some(CellIdentity {
            cell: CellKind::Sidecar,
            sidecar_name,
            primary,
            session_id,
        });
    }

    None
}

/// GET /cli/whoami — fail-loud identity for the calling cell.
///
/// Prefer scoped principal + live/durable session map. Query/env
/// (`session`, `cell`, `sidecar_name`, `primary`) are TCP fallbacks.
/// Missing identity → teaching error (run inside a workspace session).
pub fn handle_whoami(params: &HashMap<String, String>) -> CliResponse {
    match resolve_whoami(params) {
        Ok(info) => CliResponse::ok_json(
            serde_json::to_string(&info).unwrap_or_else(|_| "{}".into()),
        ),
        Err(hint) => CliResponse {
            status: "400 Bad Request",
            content_type: "application/json",
            body: serde_json::json!({
                "error": "cannot determine identity",
                "hint": hint,
            })
            .to_string(),
        },
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WhoamiInfo {
    pub workspace: String,
    pub role: String,
    pub address: String,
    pub primary: String,
    pub session: String,
}

fn param<'a>(params: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    params
        .get(key)
        .map(String::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn resolve_whoami(params: &HashMap<String, String>) -> Result<WhoamiInfo, String> {
    let principal = crate::caller_workspace::request_principal();
    let project_id = principal
        .as_ref()
        .map(|p| p.workspace_uuid.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| param(params, "project_id").map(str::to_string))
        .or_else(|| {
            param(params, "project")
                .or_else(|| param(params, "project_path"))
                .and_then(|q| {
                    let db = k2_core::db::shared();
                    let conn = db.lock();
                    crate::workspace_msg::resolve_workspace(q).and_then(|path| {
                        k2_core::workspace::agent_identity::resolve_project_id(&conn, &path)
                    })
                })
        });

    let cell_sid = param(params, "cell_session_id")
        .or_else(|| param(params, "session"))
        .map(str::to_string);

    let env_cell = param(params, "cell").or_else(|| param(params, "k2_cell"));
    let env_sidecar = param(params, "sidecar_name").or_else(|| param(params, "k2_sidecar_name"));
    let env_primary = param(params, "primary").or_else(|| param(params, "k2_primary"));

    let Some(project_id) = project_id else {
        // TCP without a cell: last-ditch env-only if all identity vars present.
        if let (Some(cell), Some(primary), Some(session)) = (env_cell, env_primary, cell_sid.as_deref())
        {
            let role = if cell == "sidecar" { "sidecar" } else { "canonical" };
            let address = if role == "sidecar" {
                format_address(primary, env_sidecar)
            } else {
                primary.to_string()
            };
            return Ok(WhoamiInfo {
                workspace: primary.to_string(),
                role: role.to_string(),
                address,
                primary: primary.to_string(),
                session: session.to_string(),
            });
        }
        return Err(
            "run this inside a workspace session (`k2 whoami` needs a scoped cell or K2_SESSION_ID)"
                .to_string(),
        );
    };

    let primary = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        workspace_address_name(&conn, &project_id)
    }
    .or_else(|_| env_primary.map(str::to_string).ok_or_else(|| "no workspace name".to_string()))?;

    // Live map keyed by daemon PTY id (scoped token session).
    let live = cell_sid.as_deref().and_then(|s| {
        SessionId::parse(s).and_then(|id| session_lookup::lookup_by_session_id(&id))
    });
    let live_agent = live.as_ref().and_then(|_| {
        session_lookup::snapshot_all()
            .into_iter()
            .find(|(_, s)| {
                cell_sid
                    .as_deref()
                    .and_then(SessionId::parse)
                    .map(|id| s.session_id() == id)
                    .unwrap_or(false)
            })
            .map(|(name, _)| name)
    });

    let tab = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if let Some(sid) = cell_sid.as_deref() {
            if let Some(row) = k2_core::db::schema::WorkspaceTabSession::get_by_session_id(&conn, sid)
                .ok()
                .flatten()
            {
                Some(row)
            } else if let Some(name) = live_agent.as_deref() {
                k2_core::db::schema::WorkspaceTabSession::get_by_agent_name(
                    &conn, &project_id, name,
                )
                .ok()
                .flatten()
            } else {
                None
            }
        } else if let Some(name) = live_agent.as_deref() {
            k2_core::db::schema::WorkspaceTabSession::get_by_agent_name(&conn, &project_id, name)
                .ok()
                .flatten()
        } else {
            None
        }
    };

    let agent_name = live_agent
        .or_else(|| tab.as_ref().map(|t| t.agent_name.clone()))
        .unwrap_or_default();
    let command = live
        .as_ref()
        .and_then(|l| l.command())
        .or_else(|| tab.as_ref().and_then(|t| t.command.clone()));
    let provider_sid = tab
        .as_ref()
        .and_then(|t| t.session_id.clone())
        .filter(|s| !s.is_empty());

    let is_canonical = is_canonical_agent_name(&agent_name, &project_id)
        || agent_name.is_empty() && env_cell != Some("sidecar");
    let is_sidecar = is_sidecar_harness(&agent_name, &project_id, command.as_deref())
        || env_cell == Some("sidecar");

    let (role, sidecar_handle) = if is_sidecar {
        let key = conversation_key_for(
            provider_sid.as_deref(),
            tab.as_ref()
                .map(|t| t.pane_group_id.as_str())
                .unwrap_or(cell_sid.as_deref().unwrap_or("")),
        );
        let handle = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            handle_for_session(&conn, &project_id, &key, provider_sid.as_deref()).ok()
        }
        .or_else(|| env_sidecar.map(str::to_string));
        ("sidecar", handle)
    } else if is_canonical || env_cell == Some("canonical") {
        ("canonical", None)
    } else {
        return Err(
            "run this inside a workspace session — could not classify this cell as canonical or sidecar"
                .to_string(),
        );
    };

    let session = provider_sid
        .or(cell_sid)
        .ok_or_else(|| "no session id (provider conversation id or daemon PTY id)".to_string())?;

    Ok(WhoamiInfo {
        workspace: primary.clone(),
        role: role.to_string(),
        address: format_address(&primary, sidecar_handle.as_deref()),
        primary,
        session,
    })
}

pub fn apply_spawn_identity(
    env: &mut HashMap<String, String>,
    project_id: &str,
    agent_name: &str,
    command: Option<&str>,
    args: &[String],
    daemon_session_id: &str,
    pane_or_tab_key: &str,
) {
    if env.get("K2_API_CELL").map(|v| !v.is_empty()).unwrap_or(false)
        || k2_core::workspace_session_handles::is_api_agent_name(agent_name)
    {
        return;
    }
    if let Some(id) = resolve_spawn_identity(
        project_id,
        agent_name,
        command,
        args,
        daemon_session_id,
        pane_or_tab_key,
    ) {
        apply_cell_identity_env(env, &id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_env_sets_all_four_and_canonical_omits_sidecar_name() {
        let mut env = HashMap::new();
        apply_cell_identity_env(
            &mut env,
            &CellIdentity {
                cell: CellKind::Sidecar,
                sidecar_name: Some("reviewer".into()),
                primary: "sales".into(),
                session_id: "sid-1".into(),
            },
        );
        assert_eq!(env.get("K2_CELL").map(String::as_str), Some("sidecar"));
        assert_eq!(
            env.get("K2_SIDECAR_NAME").map(String::as_str),
            Some("reviewer")
        );
        assert_eq!(env.get("K2_PRIMARY").map(String::as_str), Some("sales"));
        assert_eq!(env.get("K2_SESSION_ID").map(String::as_str), Some("sid-1"));

        let mut env2 = HashMap::new();
        env2.insert("K2_SIDECAR_NAME".into(), "stale".into());
        apply_cell_identity_env(
            &mut env2,
            &CellIdentity {
                cell: CellKind::Canonical,
                sidecar_name: None,
                primary: "sales".into(),
                session_id: "sid-2".into(),
            },
        );
        assert_eq!(env2.get("K2_CELL").map(String::as_str), Some("canonical"));
        assert!(
            !env2.contains_key("K2_SIDECAR_NAME"),
            "canonical must not set K2_SIDECAR_NAME"
        );
        assert_eq!(env2.get("K2_PRIMARY").map(String::as_str), Some("sales"));
    }

    #[test]
    fn api_cell_skips_identity_overlay_and_keeps_session_id() {
        k2_core::db::init_for_tests();
        let got = resolve_spawn_identity(
            "proj-id",
            "api-principal-uuid",
            Some("claude"),
            &[],
            "daemon-sid",
            "api-principal-uuid",
        );
        assert!(got.is_none(), "api-* must not become a sidecar");

        let mut env = HashMap::new();
        env.insert("K2_API_CELL".into(), "1".into());
        env.insert("K2_SESSION_ID".into(), "api-session-jwt-sub".into());
        apply_spawn_identity(
            &mut env,
            "proj-id",
            "api-principal-uuid",
            Some("claude"),
            &[],
            "daemon-sid",
            "api-principal-uuid",
        );
        assert_eq!(
            env.get("K2_SESSION_ID").map(String::as_str),
            Some("api-session-jwt-sub"),
            "must not overwrite host-session K2_SESSION_ID"
        );
        assert!(!env.contains_key("K2_CELL"));
    }

    #[test]
    fn extra_tab_shell_gets_no_identity() {
        k2_core::db::init_for_tests();
        let got = resolve_spawn_identity(
            "proj-id",
            "tab-abc",
            Some("zsh"),
            &[],
            "daemon-sid",
            "abc",
        );
        assert!(got.is_none(), "blank shell must not get K2_CELL=sidecar");
    }

    #[test]
    fn whoami_env_fallback_shapes_canonical_and_sidecar() {
        k2_core::db::init_for_tests();
        let mut p = HashMap::new();
        p.insert("cell".into(), "canonical".into());
        p.insert("primary".into(), "sales".into());
        p.insert("session".into(), "sess-canon".into());
        let info = resolve_whoami(&p).expect("canonical env");
        assert_eq!(info.role, "canonical");
        assert_eq!(info.address, "sales");
        assert_eq!(info.primary, "sales");
        assert_eq!(info.workspace, "sales");
        assert_eq!(info.session, "sess-canon");

        let mut p2 = HashMap::new();
        p2.insert("cell".into(), "sidecar".into());
        p2.insert("primary".into(), "sales".into());
        p2.insert("sidecar_name".into(), "reviewer".into());
        p2.insert("session".into(), "sess-side".into());
        let info2 = resolve_whoami(&p2).expect("sidecar env");
        assert_eq!(info2.role, "sidecar");
        assert_eq!(info2.address, "sales/reviewer");
        assert_eq!(info2.primary, "sales");
        assert_eq!(info2.session, "sess-side");
    }
}
