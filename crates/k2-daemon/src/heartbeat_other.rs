//! Pause or resume ANOTHER workspace's heartbeat (0.46.6): `k2 heartbeat
//! disable|enable <name> --workspace <ws>` (and `schedule disable|enable`).
//! One door, `POST /cli/heartbeat/enable` with `workspace=`, the same model
//! as `k2 agent harness` ([`crate::agent_harness`]).
//!
//! **Who.** A person: the owner token, an Owner or Admin login, or a
//! passport from a plain K2 shell tab (a person typing in K2). A Member
//! login gets `403 admin_only`, however the workspace is named
//! (`workspace=`, `<ws>/<name>` or `project=`: a login has no workspace of
//! its own). An agent passport keeps its own workspace's heartbeats as
//! before; naming another workspace (any of the three ways) gets
//! `403 other_workspace` with the workspace and the heartbeat in the
//! refusal. With permission cards on, that refusal teaches
//! `k2 permission request heartbeat.pause-other target=<ws> heartbeat=<n>`
//! (or `heartbeat.resume-other`); the person taps Allow and the card runs
//! [`handle`] as them, once ([`crate::permission::actions::heartbeats`]).
//!
//! **Errors say where.** A heartbeat missing from the named workspace is
//! `no_such_heartbeat` naming that workspace; one missing from the
//! caller's own workspace names the workspaces that do have it, so the
//! real reason ("it's in another workspace") is never hidden. Heartbeat
//! names have no `/`, so `<workspace>/<name>` is read as both.
//!
//! **Audit.** Every cross-workspace pause or resume, and every refusal of
//! one, leaves an auth-audit line (`heartbeat-other`: actor, target
//! workspace, heartbeat, outcome). The heartbeat's own history gets the
//! usual `changed` row with the actor.

use std::collections::HashMap;

use serde_json::json;

use crate::cli_response::CliResponse;
use crate::sidecar_routes::Caller;

/// The route (pause and resume are `enabled=false|true`).
pub const ROUTE: &str = "/cli/heartbeat/enable";

/// An agent named another workspace's heartbeat.
pub const OTHER_WORKSPACE: &str = "other_workspace";

/// The permission-card actions an agent asks with.
pub const PAUSE_ACTION: &str = "heartbeat.pause-other";
pub const RESUME_ACTION: &str = "heartbeat.resume-other";

/// The auth-audit event.
pub const AUDIT_EVENT: &str = "heartbeat-other";

thread_local! {
    /// The dispatcher's ingress label for the current request (audit only).
    static INGRESS: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Run `f` with `ingress` as the request's ingress label (audit only).
pub fn with_ingress<R>(ingress: &str, f: impl FnOnce() -> R) -> R {
    struct ClearOnDrop;
    impl Drop for ClearOnDrop {
        fn drop(&mut self) {
            INGRESS.with(|c| *c.borrow_mut() = None);
        }
    }
    let _guard = ClearOnDrop;
    INGRESS.with(|c| *c.borrow_mut() = Some(ingress.to_string()));
    f()
}

fn ingress_now(params: &HashMap<String, String>) -> String {
    INGRESS.with(|c| c.borrow().clone()).unwrap_or_else(|| {
        if params.contains_key(crate::caller_workspace::CELL_SESSION_KEY) {
            "cell".to_string()
        } else {
            "-".to_string()
        }
    })
}

/// A resolved workspace.
#[derive(Debug, Clone)]
pub(crate) struct Target {
    pub id: String,
    pub path: String,
    pub handle: String,
}

fn handle_of(project_id: &str) -> String {
    k2_core::workspace_session_handles::workspace_address_name_shared(project_id).unwrap_or_else(|_| project_id.to_string())
}

fn project_id_of(path: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::workspace::agent_identity::resolve_project_id(&conn, path)
}

/// Resolve a workspace token (handle, name, path or id). The errors are
/// the heartbeat family's own (`workspace not found: …`, 400).
pub(crate) fn resolve(token: &str) -> Result<Target, CliResponse> {
    let path = crate::misc_routes::resolve_heartbeat_workspace(token)?;
    let Some(id) = project_id_of(&path) else {
        return Err(CliResponse::bad_request(format!("workspace not found: {token}")));
    };
    let handle = handle_of(&id);
    Ok(Target { id, path, handle })
}

/// The caller of a param-dispatched heartbeat route. A passport (stamped
/// `principal_bound`) is classified by its session (`cell_session_id`,
/// stamped by the cell socket and the TCP heartbeat arm): a plain shell tab
/// is a person, a harness session (or a session that is gone) an agent.
/// Otherwise the request actor: `user:<name>` is a Connect login; the owner
/// token or an in-daemon call is the owner.
pub(crate) fn caller_from_params(params: &HashMap<String, String>) -> Caller {
    if let Some(p) = crate::caller_workspace::principal_from_params(params) {
        let sid = params.get(crate::caller_workspace::CELL_SESSION_KEY).map(String::as_str).unwrap_or("");
        return match crate::sidecar_routes::caller_from_session(sid, &p) {
            c @ (Caller::Shell { .. } | Caller::Agent(_)) => c,
            // An API cell or anything else that isn't a person: fail closed
            // as an agent of its own workspace.
            _ => Caller::Agent(crate::sidecar_routes::AgentCaller {
                project_id: p.workspace_uuid,
                map_key: None,
                args: Vec::new(),
                sandboxed: false,
            }),
        };
    }
    match crate::heartbeat_routes::request_actor() {
        Some(a) => match a.strip_prefix("user:") {
            Some(user) => Caller::Login { username: user.to_string() },
            None => Caller::Owner,
        },
        None => Caller::Owner,
    }
}

/// The label the heartbeat's history shows for this caller.
fn actor_label(caller: &Caller) -> String {
    match caller {
        Caller::Owner => crate::heartbeat_routes::request_actor()
            .filter(|a| !a.starts_with("agent:"))
            .unwrap_or_else(|| "owner-token".to_string()),
        Caller::Login { username } => format!("user:{username}"),
        Caller::Shell { project_id } => format!("shell:{}", handle_of(project_id)),
        Caller::Agent(a) => format!("agent:{}", handle_of(&a.project_id)),
        Caller::WrongCredential | Caller::Invalid => "-".to_string(),
    }
}

fn verb(enabled: bool) -> &'static str {
    if enabled {
        "enable"
    } else {
        "disable"
    }
}

fn audit(caller: &Caller, outcome: &str, target: &Target, name: &str, ingress: &str) {
    k2_core::auth_audit::record(
        &k2_core::auth_audit::AuditEvent::new(
            AUDIT_EVENT,
            &caller.audit_user(),
            format!("{outcome}:{}/{name}", target.handle),
            ingress,
            "-",
            "cli",
        )
        .with_actor(&actor_label(caller)),
    );
}

fn json_resp(status: &'static str, v: serde_json::Value) -> CliResponse {
    CliResponse { status, content_type: "application/json", body: v.to_string() }
}

/// `no_such_heartbeat` (404), naming the workspace that was looked in.
fn no_such(target: &Target, name: &str, hint: String) -> CliResponse {
    json_resp(
        "404 Not Found",
        json!({
            "error": k2_core::heartbeats::NO_SUCH_HEARTBEAT,
            "message": format!("no heartbeat named '{name}' in workspace {}{hint}", target.handle),
            "workspace": target.handle,
            "heartbeat": name,
        }),
    )
}

/// The other workspaces with a heartbeat of this name (handles, sorted).
fn others_with(name: &str, except_id: &str) -> Vec<String> {
    let ids: Vec<String> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let Ok(mut stmt) = conn.prepare(
            "SELECT DISTINCT project_id FROM workspace_heartbeats WHERE name = ?1 AND project_id != ?2",
        ) else {
            return Vec::new();
        };
        let rows = stmt.query_map(rusqlite::params![name, except_id], |r| r.get::<_, String>(0));
        match rows {
            Ok(r) => r.filter_map(Result::ok).collect(),
            Err(_) => return Vec::new(),
        }
    };
    let mut out: Vec<String> = ids.iter().map(|id| handle_of(id)).collect();
    out.sort();
    out
}

/// The heartbeat's enabled flag, or `None` when there is none by that name.
pub(crate) fn enabled_of(project_id: &str, name: &str) -> Option<bool> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::db::schema::AgentHeartbeat::get_by_name(&conn, project_id, name).ok().flatten().map(|h| h.enabled)
}

/// The refusal an agent gets for another workspace's heartbeat. The walls
/// read `error.workspace` and `error.heartbeat`; the route's `enabled=`
/// picks pause or resume.
fn other_workspace(own_id: &str, target: &Target, name: &str, enabled: bool) -> CliResponse {
    let what = if enabled { "resume" } else { "pause" };
    let hint = format!(
        "'{name}' is a heartbeat of workspace {ws}, not of yours ({own}): an agent can {what} only its own \
         workspace's heartbeats. A person runs `k2 heartbeat {verb} {name} --workspace {ws}` (the owner, an \
         Admin, or someone in a K2 shell tab), or {what}s it in the app.",
        ws = target.handle,
        own = handle_of(own_id),
        verb = verb(enabled),
    );
    json_resp(
        "403 Forbidden",
        json!({
            "error": {
                "code": OTHER_WORKSPACE,
                "hint": hint,
                "workspace": target.handle,
                "heartbeat": name,
                "enabled": enabled,
            }
        }),
    )
}

/// `POST /cli/heartbeat/enable` from the heartbeat dispatch. `params` are
/// the request's (`name`, `enabled`, optional `workspace`) and `project`
/// is `project=`: for a passport, stamped to its own workspace (its own
/// claim kept under [`crate::caller_workspace::CLIENT_PROJECT_KEY`]); for
/// the owner and logins, the client's own value.
///
/// The workspace a toggle changes can be named three ways, all equal:
/// `workspace=`, `<ws>/<name>`, or `project=`. Only a passport has a
/// workspace of its own (the agent's, or the shell tab's): naming that one
/// is the plain toggle as before. Anything else is another workspace:
/// people at the Admin floor, an agent asks, audited. The owner and logins
/// have no workspace of their own, so every toggle of theirs is one: a
/// Member login gets `admin_only` however the workspace is named (0.46.6).
pub fn route(params: &HashMap<String, String>, project: &str) -> CliResponse {
    let raw_name = params.get("name").map(|s| s.trim()).unwrap_or("");
    let enabled = params.get("enabled").map(|v| v == "true" || v == "1").unwrap_or(true);
    let mut workspace = params.get("workspace").map(|s| s.trim()).filter(|s| !s.is_empty());
    let mut name = raw_name;
    // `<workspace>/<name>`: a heartbeat name has no '/'.
    if let Some((ws, n)) = raw_name.rsplit_once('/') {
        if workspace.is_none() && !ws.trim().is_empty() {
            workspace = Some(ws.trim());
        }
        name = n.trim();
    }
    if name.is_empty() {
        return CliResponse::bad_request("Missing 'name' parameter");
    }
    let caller = caller_from_params(params);
    let ingress = ingress_now(params);
    let passport = matches!(caller, Caller::Agent(_) | Caller::Shell { .. });
    if !passport {
        // The owner and logins: `project=` names the workspace, like
        // `workspace=` (which wins when both are sent).
        let named = workspace.unwrap_or(project);
        return handle(None, Some(named), name, enabled, &caller, &ingress);
    }
    // A passport's own `project=` claim, when it names another registered
    // workspace, is that workspace (an unregistered folder, e.g. a
    // subfolder of its own, is ignored as before).
    let claim = params
        .get(crate::caller_workspace::CLIENT_PROJECT_KEY)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .filter(|s| matches!(crate::workspace_msg::resolve_workspace_detailed(s), crate::workspace_msg::WorkspaceResolve::Found(_)));
    handle(Some(project), workspace.or(claim), name, enabled, &caller, &ingress)
}

/// The route's entry, shared with the permission card (which passes the
/// person who tapped Allow as `caller`). `home` is the caller's own
/// workspace path (a passport's; `None` for the owner and logins);
/// `workspace` the named one (`None`: `home`'s own toggle).
pub(crate) fn handle(
    home: Option<&str>,
    workspace: Option<&str>,
    name: &str,
    enabled: bool,
    caller: &Caller,
    ingress: &str,
) -> CliResponse {
    match caller {
        Caller::WrongCredential => {
            return json_resp(
                "403 Forbidden",
                json!({ "error": { "code": "wrong_credential", "hint": "this credential can't change heartbeats" } }),
            )
        }
        Caller::Invalid => {
            return json_resp("403 Forbidden", json!({ "error": { "code": "invalid_token", "hint": "invalid or missing token" } }))
        }
        _ => {}
    }
    let own_id = home.and_then(project_id_of);
    let token = match (workspace, home) {
        (Some(t), _) => t,
        (None, Some(own)) => return own_workspace(own, own_id.as_deref(), name, enabled),
        (None, None) => return CliResponse::bad_request("Missing project (or workspace) parameter"),
    };
    let target = match resolve(token) {
        Ok(t) => t,
        Err(r) => return r,
    };
    if let (Some(own), true) = (home, own_id.as_deref() == Some(target.id.as_str())) {
        // A passport naming its own workspace: exactly as without a name.
        return own_workspace(own, own_id.as_deref(), name, enabled);
    }
    if enabled_of(&target.id, name).is_none() {
        // A person can list that workspace's heartbeats; an agent can't.
        let hint = match caller {
            Caller::Agent(_) => String::new(),
            _ => format!(" (list them: k2 heartbeat schedule list --workspace {})", target.handle),
        };
        return no_such(&target, name, hint);
    }
    match caller {
        Caller::Agent(a) => {
            audit(caller, "refused:other_workspace", &target, name, ingress);
            return other_workspace(&a.project_id, &target, name, enabled);
        }
        Caller::Login { username } => {
            use k2_core::connect_users::Role;
            match k2_core::connect_users::role_for_user(username) {
                Some(Role::Owner) | Some(Role::Admin) => {}
                other => {
                    audit(caller, "refused:admin_only", &target, name, ingress);
                    let role = other.map(|r| r.as_wire()).unwrap_or("unknown");
                    return json_resp(
                        "403 Forbidden",
                        json!({ "error": {
                            "code": "admin_only",
                            "hint": format!(
                                "pausing or resuming another workspace's heartbeat needs the owner or an Admin login; you are signed in as {role}"
                            ),
                            "workspace": target.handle,
                            "heartbeat": name,
                        }}),
                    );
                }
            }
        }
        _ => {}
    }
    let actor = actor_label(caller);
    let result = crate::heartbeat_routes::with_request_actor(Some(actor), || {
        crate::heartbeat_routes::with_change(
            &target.path,
            name,
            if enabled { "enabled" } else { "disabled" },
            k2_core::heartbeats::k2so_heartbeat_set_enabled(target.path.clone(), name.to_string(), enabled)
                .map(|_| r#"{"success":true}"#.to_string()),
        )
    });
    match result {
        Ok(_) => {
            audit(caller, if enabled { "resumed" } else { "paused" }, &target, name, ingress);
            CliResponse::ok_json(
                json!({
                    "success": true,
                    "workspace": target.handle,
                    "heartbeat": name,
                    "enabled": enabled,
                })
                .to_string(),
            )
        }
        Err(msg) if k2_core::heartbeats::is_no_such_heartbeat(&msg) => no_such(&target, name, String::new()),
        Err(msg) => {
            audit(caller, "failed", &target, name, ingress);
            CliResponse::bad_request(msg)
        }
    }
}

/// The caller's own workspace, as before `workspace=` existed; a missing
/// heartbeat names the workspaces that do have one by that name.
fn own_workspace(own: &str, own_id: Option<&str>, name: &str, enabled: bool) -> CliResponse {
    let mut params = HashMap::new();
    params.insert("name".to_string(), name.to_string());
    params.insert("enabled".to_string(), enabled.to_string());
    // The dispatcher's request actor (AH28) stamps the change, as before.
    match crate::heartbeat_routes::dispatch_get(ROUTE, own, &params) {
        Ok(body) => CliResponse::ok_json(body),
        Err(msg) if k2_core::heartbeats::is_no_such_heartbeat(&msg) => {
            let Some(own_id) = own_id else {
                return crate::heartbeat_app_routes::no_such_heartbeat_response(&msg);
            };
            let here = Target { id: own_id.to_string(), path: own.to_string(), handle: handle_of(own_id) };
            let others = others_with(name, own_id);
            let hint = match others.as_slice() {
                [] => String::new(),
                [one] => format!(
                    ". Workspace {one} has one: k2 heartbeat {} {name} --workspace {one}",
                    verb(enabled)
                ),
                many => format!(
                    ". These workspaces have one: {}. Name it: k2 heartbeat {} {name} --workspace <workspace>",
                    many.join(", "),
                    verb(enabled)
                ),
            };
            no_such(&here, name, hint)
        }
        Err(msg) => CliResponse::bad_request(msg),
    }
}

#[cfg(test)]
#[path = "heartbeat_other_tests.rs"]
mod tests;
