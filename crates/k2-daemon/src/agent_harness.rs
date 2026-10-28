//! `POST /cli/agent/harness` — `k2 agent harness <workspace> <harness>`
//! (0.46.6, Rosson): move a workspace's pinned chat to another harness.
//!
//! The pinned chat (the `workspace_sessions` row heartbeats, Threads and
//! messages target) keeps the harness it started with; changing the
//! workspace's default agent only changes new chats. This route:
//!
//! - ends the live pinned chat and starts a new chat of `harness` with the
//!   continue seed (`/cli/chat/continue-seed`'s text, the same as Chat
//!   history's "Continue in a new chat…") plus optional notes;
//! - repoints `workspace_sessions` (harness + session id) to the new chat,
//!   so heartbeats on the pinned chat, Threads and `k2 msg` follow it;
//! - leaves the old chat on disk, so it stays in Chat history;
//! - is idempotent: already on `harness` answers `alreadyOn` and changes
//!   nothing.
//!
//! The swap itself is [`crate::pinned_chat::run_swap`] (also behind the
//! older `POST /cli/workspace/swap-canonical`).
//!
//! **Who.** A person: the owner token, an Owner or Admin login, or a
//! passport from a plain shell tab (a person typing in K2). An agent never
//! switches, because it would restart its own session: for its own
//! workspace it gets `403 harness_needs_person` (with permission cards on,
//! the refusal teaches `k2 permission request agent.harness-switch
//! harness=<h>`; the person taps Allow and the card runs this route as
//! them, `permission::actions::harness`); for another workspace,
//! `403 other_workspace`. A Member login gets `403 admin_only`. Every
//! switch and refusal leaves an auth-audit line (`harness-switch`).

use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::sidecar_routes::Caller;

/// The route.
pub const ROUTE: &str = "/cli/agent/harness";

/// Body cap (notes ride along, capped at 32,000 characters in k2-core).
pub const MAX_BODY: usize = crate::pinned_chat::SWAP_CANONICAL_MAX_BODY;

/// An agent asked to move its own pinned chat.
pub const NEEDS_PERSON: &str = "harness_needs_person";

/// The permission-card action an agent asks with.
pub const CARD_ACTION: &str = "agent.harness-switch";

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Req {
    /// Handle, name, path or id.
    #[serde(default, alias = "project")]
    workspace: String,
    /// `claude`, `codex`, `grok`, … (a binary or preset command works too).
    #[serde(default, alias = "provider")]
    harness: String,
    /// `recent` (default) or `full`: how much of the old chat the seed carries.
    #[serde(default)]
    mode: String,
    #[serde(default)]
    notes: String,
    #[serde(default)]
    dry_run: bool,
}

/// A resolved workspace.
pub(crate) struct Target {
    pub id: String,
    pub path: String,
    pub handle: String,
}

fn refused(status: &'static str, code: &str, hint: impl Into<String>) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: json!({ "error": { "code": code, "hint": hint.into() }, "switched": false }).to_string(),
    }
}

fn handle_of(project_id: &str) -> String {
    k2_core::workspace_session_handles::workspace_address_name_shared(project_id).unwrap_or_else(|_| project_id.to_string())
}

/// The refusal an agent gets for its own pinned chat. `harness` is
/// normalized; the walls map reads it from `/error/harness`.
pub(crate) fn needs_person(project_id: &str, harness: &str) -> CliResponse {
    let ws = handle_of(project_id);
    let hint = format!(
        "Moving the pinned chat restarts your own session, so a person does it. Ask your person to run \
         `k2 agent harness {ws} {harness}` from a terminal outside K2, or to pick \
         \"Move pinned chat to {harness}\" in the pinned chat's menu. After the move you continue in a \
         new {harness} chat that starts with a summary of this one; this chat stays in Chat history."
    );
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: json!({
            "error": { "code": NEEDS_PERSON, "hint": hint, "workspace": ws, "harness": harness },
            "switched": false,
            "swapped": false,
        })
        .to_string(),
    }
}

fn audit(caller: &Caller, outcome: &str, ingress: &str) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        "harness-switch",
        &caller.audit_user(),
        outcome.to_string(),
        ingress,
        "-",
        "cli",
    ));
}

/// Resolve a workspace token (handle, name, path or id).
pub(crate) fn resolve(token: &str) -> Result<Target, CliResponse> {
    use crate::workspace_msg::WorkspaceResolve;
    let path = match crate::workspace_msg::resolve_workspace_detailed(token.trim()) {
        WorkspaceResolve::Found(p) => p,
        WorkspaceResolve::Ambiguous { handles } => {
            return Err(refused(
                "400 Bad Request",
                "ambiguous_workspace",
                format!("'{token}' matches more than one workspace ({}); name it by handle or path", handles.join(", ")),
            ))
        }
        WorkspaceResolve::Retired { handle } => {
            return Err(refused(
                "404 Not Found",
                "not_found",
                format!("workspace not found: {token}. {}", k2_core::workspace::handle::retired_handle_message(&handle)),
            ))
        }
        WorkspaceResolve::Miss => {
            return Err(refused("404 Not Found", "not_found", format!("workspace not found: {token} (k2 agent list)")))
        }
    };
    let id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row("SELECT id FROM projects WHERE path = ?1", rusqlite::params![path], |r| r.get::<_, String>(0)).ok()
    };
    let Some(id) = id else {
        return Err(refused("404 Not Found", "not_found", format!("workspace not found: {token}")));
    };
    let handle = handle_of(&id);
    Ok(Target { id, path, handle })
}

/// The harness the pinned chat runs on: the saved row's, else (never
/// started) the workspace default agent's.
pub(crate) fn current_harness(project_id: &str, project_path: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    if let Ok(Some(row)) = k2_core::db::schema::WorkspaceSession::get(&conn, project_id) {
        if !row.harness.trim().is_empty() {
            return row.harness;
        }
    }
    let cmd = k2_core::workspace::agent_resolve::resolve_agent_command(&conn, project_path);
    k2_core::workspace::provider_resume::provider_resume_for_command(&cmd.command)
        .map(|a| a.provider.to_string())
        .unwrap_or(cmd.command)
}

/// Active heartbeats that resume their own session instead of the pinned
/// chat: they don't follow a switch (`k2 heartbeat use-pinned-session on`).
fn own_session_heartbeats(project_id: &str) -> Vec<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::db::schema::AgentHeartbeat::list_active(&conn, project_id)
        .unwrap_or_default()
        .into_iter()
        .filter(|h| !h.use_workspace_session)
        .map(|h| h.name)
        .collect()
}

/// Is `session_id` one of the workspace's listed chats (Chat history)?
fn listed_in_history(project_path: &str, session_id: &str) -> Option<bool> {
    if session_id.is_empty() {
        return None;
    }
    let rows = k2_core::chat_history::list_all_sessions(Some(project_path)).ok()?;
    Some(rows.iter().any(|r| r.session_id == session_id))
}

/// Handler for `POST /cli/agent/harness` (see the module doc). Body:
/// `{workspace, harness, mode?, notes?, dryRun?}`.
pub fn handle(body: &[u8], caller: &Caller, ingress: &str) -> CliResponse {
    match caller {
        Caller::WrongCredential => {
            return refused(
                "403 Forbidden",
                "wrong_credential",
                "k2 agent harness takes the owner token, a Connect login, or a K2 session passport (not an app pass, API key, or API cell)",
            )
        }
        Caller::Invalid => return refused("403 Forbidden", "invalid_token", "invalid or missing token"),
        _ => {}
    }
    let req: Req = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => {
            return refused("400 Bad Request", "usage", format!("parse agent harness request: {}", e.to_string().replace('"', "'")))
        }
    };
    if req.workspace.trim().is_empty() || req.harness.trim().is_empty() {
        return refused(
            "400 Bad Request",
            "usage",
            "usage: k2 agent harness <workspace> <harness> (claude, codex, grok, gemini, cursor, pi, or hermes)",
        );
    }
    let harness = match k2_core::workspace::canonical_swap::normalize_harness(&req.harness) {
        Ok(h) => h,
        Err(e) => return refused("400 Bad Request", "unknown_harness", e),
    };
    let mode = match req.mode.trim() {
        "" | "recent" => "recent",
        "full" => "full",
        other => return refused("400 Bad Request", "usage", format!("mode must be recent or full (got {other})")),
    };
    let target = match resolve(&req.workspace) {
        Ok(t) => t,
        Err(r) => return r,
    };
    let from = match caller {
        Caller::Agent(agent) => {
            if agent.project_id == target.id {
                audit(caller, "refused:needs_person", ingress);
                return needs_person(&target.id, harness);
            }
            audit(caller, "refused:other_workspace", ingress);
            return refused(
                "403 Forbidden",
                "other_workspace",
                "an agent can't move another workspace's pinned chat; a person runs `k2 agent harness <workspace> <harness>` from a terminal outside K2",
            );
        }
        Caller::Login { username } => {
            use k2_core::connect_users::Role;
            match k2_core::connect_users::role_for_user(username) {
                Some(Role::Owner) | Some(Role::Admin) => username.clone(),
                other => {
                    audit(caller, "refused:admin_only", ingress);
                    let role = other.map(|r| r.as_wire()).unwrap_or("unknown");
                    return refused(
                        "403 Forbidden",
                        "admin_only",
                        format!("moving a pinned chat needs the owner or an Admin login; you are signed in as {role}"),
                    );
                }
            }
        }
        // 0.46.0 stamp rule: the owner's linked username, else `owner`.
        _ => k2_core::person_name::sender_identity_now(k2_core::person_name::Sender::OwnerToken),
    };

    let current = current_harness(&target.id, &target.path);
    if current == harness {
        return CliResponse::ok_json(
            json!({
                "switched": false,
                "alreadyOn": true,
                "workspace": target.handle,
                "path": target.path,
                "harness": harness,
            })
            .to_string(),
        );
    }
    // The target CLI must launch before anything is touched; a dry run
    // answers the same refusal a real run would.
    let launches = match crate::pinned_chat::preflight_harness(&target.path, harness) {
        Ok(found) => found,
        Err(r) => {
            audit(caller, &format!("refused:{}:{harness}", crate::pinned_chat::HARNESS_NOT_INSTALLED), ingress);
            return r;
        }
    };
    let own_session = own_session_heartbeats(&target.id);
    if req.dry_run {
        return CliResponse::ok_json(
            json!({
                "switched": false,
                "dryRun": true,
                "workspace": target.handle,
                "path": target.path,
                "fromHarness": current,
                "harness": harness,
                "launches": launches,
                "ownSessionHeartbeats": own_session,
            })
            .to_string(),
        );
    }
    match crate::pinned_chat::run_swap(&target.path, harness, mode, &req.notes, &from) {
        Err(r) => {
            audit(caller, &format!("failed:{harness}"), ingress);
            r
        }
        Ok(mut v) => {
            audit(caller, &format!("switched:{harness}"), ingress);
            let from_session = v["fromSessionId"].as_str().unwrap_or("").to_string();
            let o = v.as_object_mut().expect("run_swap answers an object");
            o.insert("switched".into(), Value::Bool(true));
            o.insert("workspace".into(), Value::String(target.handle.clone()));
            o.insert("path".into(), Value::String(target.path.clone()));
            o.insert("harness".into(), Value::String(harness.to_string()));
            o.insert("fromHarness".into(), Value::String(current));
            o.insert("oldChatInHistory".into(), json!(listed_in_history(&target.path, &from_session)));
            o.insert("ownSessionHeartbeats".into(), json!(own_session));
            CliResponse::ok_json(v.to_string())
        }
    }
}

#[cfg(test)]
#[path = "agent_harness_tests.rs"]
mod tests;
