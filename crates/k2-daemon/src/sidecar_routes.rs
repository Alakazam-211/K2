//! `k2 sidecar` v1 routes (`.k2/prds/prd-k2-sidecar-cli-v1.md`).
//!
//! - `POST /cli/sidecar/new`   — open (or resume) a named sidecar.
//! - `GET  /cli/sidecar/list`  — the workspace's sidecars, live and asleep.
//! - `POST /cli/sidecar/stop`  — stop the process, keep everything else.
//! - `POST /cli/agent-access/set` — the "Allow hiring and managing agents"
//!   switch (Owner/Admin only; not an agent verb).
//!
//! The daemon owns the whole job (SC2). A sidecar is an ordinary
//! `tab-<pane group>` harness session spawned in-process through
//! [`crate::spawn::spawn_agent_session_v2_blocking`] (SC29), never through
//! `/cli/sessions/v2/spawn`. The TCP dispatcher and the per-cell socket
//! both build a [`Caller`] and call [`handle`].
//!
//! The per-harness identity and resume rules live in `k2_core::sidecar`
//! (the module doc has the Big-7 table). This file adds the parts that
//! need the live session map: who is calling, the agent gate (§7.3), the
//! cap, stop, the adoption watch for codex/hermes, and the relaunch plan
//! restart recovery and the `k2 msg` wake share (SC48).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use k2_core::log_debug;
use k2_core::sidecar::{self as core, SidecarError};
use parking_lot::Mutex;

use crate::cli_response::CliResponse;
use crate::session_token::ValidatedHook;
use crate::v2_session_map;

/// Who is calling a sidecar route (§7.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caller {
    /// The owner daemon token (a terminal outside K2, a script).
    Owner,
    /// A Connect login. Its role already cleared the route floor
    /// (`route_policy::role_gate`): Admin+ for new/stop/access.
    Login { username: String },
    /// A passport from a plain shell tab (a human typing in K2): treated
    /// as the owner (noun tiers NT5).
    Shell { project_id: String },
    /// A passport from a harness session (an agent).
    Agent(AgentCaller),
    /// App pass, API key, API cell passport (`wrong_credential`).
    WrongCredential,
    /// No valid credential.
    Invalid,
}

/// The validated session behind an agent passport (SC36).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCaller {
    pub project_id: String,
    /// `v2_session_map` key of the calling session (`None` when the
    /// session is no longer live).
    pub map_key: Option<String>,
    pub args: Vec<String>,
    pub sandboxed: bool,
}

impl Caller {
    pub(crate) fn audit_user(&self) -> String {
        match self {
            Caller::Owner => "owner".to_string(),
            Caller::Login { username } => format!("login:{username}"),
            Caller::Shell { project_id } => format!("shell:{}", ws_handle(project_id)),
            Caller::Agent(a) => format!("agent:{}", ws_handle(&a.project_id)),
            Caller::WrongCredential => "wrong-credential".to_string(),
            Caller::Invalid => "-".to_string(),
        }
    }

    /// Human form for `createdBy` / the pointer line.
    fn display(&self) -> String {
        match self {
            Caller::Owner => "owner".to_string(),
            Caller::Login { username } => username.clone(),
            Caller::Shell { project_id } => format!("{} (shell)", ws_handle(project_id)),
            Caller::Agent(a) => format!("{} (agent)", ws_handle(&a.project_id)),
            _ => "-".to_string(),
        }
    }

    fn passport_project(&self) -> Option<&str> {
        match self {
            Caller::Shell { project_id } => Some(project_id),
            Caller::Agent(a) => Some(&a.project_id),
            _ => None,
        }
    }
}

/// Human form of a stored `created_by` label.
pub fn created_by_display(raw: &str) -> String {
    if let Some(ws) = raw.strip_prefix("agent:") {
        format!("{ws} (agent)")
    } else if let Some(ws) = raw.strip_prefix("shell:") {
        format!("{ws} (shell)")
    } else if let Some(user) = raw.strip_prefix("login:") {
        user.to_string()
    } else {
        raw.to_string()
    }
}

fn ws_handle(project_id: &str) -> String {
    k2_core::workspace_session_handles::workspace_address_name_shared(project_id)
        .unwrap_or_else(|_| project_id.to_string())
}

/// Classify a validated passport by the session it belongs to (SC36):
/// `api-*` → wrong credential; a harness program → agent; else shell.
pub fn caller_from_hook(v: &ValidatedHook) -> Caller {
    caller_from_session(&v.session_id, &v.principal)
}

/// [`caller_from_hook`] for the per-cell socket, which holds the bound
/// session id and principal.
pub fn caller_from_session(
    session_id: &str,
    principal: &crate::session_token::HookPrincipal,
) -> Caller {
    let v = principal;
    let project_id = v.workspace_uuid.clone();
    let live = v2_session_map::snapshot()
        .into_iter()
        .find(|(_, s)| s.session_id.to_string() == session_id);
    match live {
        Some((key, s)) => {
            if k2_core::workspace_session_handles::is_api_agent_name(&key) {
                return Caller::WrongCredential;
            }
            if k2_core::workspace_session_handles::is_harness_command(s.program.as_deref()) {
                Caller::Agent(AgentCaller {
                    project_id,
                    map_key: Some(key),
                    args: s.args.clone(),
                    sandboxed: matches!(s.sandbox, k2_core::terminal::SandboxSpec::Microvm),
                })
            } else {
                Caller::Shell { project_id }
            }
        }
        None => {
            if v.agent_address.starts_with("api-") {
                return Caller::WrongCredential;
            }
            // A passport whose session is gone: fail closed as an agent
            // that is not the canonical session.
            Caller::Agent(AgentCaller {
                project_id,
                map_key: None,
                args: Vec::new(),
                sandboxed: false,
            })
        }
    }
}

/// Classify a TCP caller. `query` is the dispatcher's effective auth
/// query (Bearer / cookie already folded into `token=`).
pub fn caller_from_tcp(path: &str, query: &str, bearer: Option<&str>, owner_token: &str) -> Caller {
    let presented = crate::routes::http::extract_token(query)
        .filter(|s| !s.is_empty())
        .or_else(|| bearer.map(str::trim).filter(|s| !s.is_empty()))
        .unwrap_or("");
    if presented.is_empty() {
        return Caller::Invalid;
    }
    if k2_core::skin::is_skin_token(presented) || presented.starts_with("k2sk_") {
        return Caller::WrongCredential;
    }
    if crate::routes::http::ct_eq_token(presented, owner_token) {
        return Caller::Owner;
    }
    if let Some(username) = k2_core::connect_users::validate_session(presented) {
        return Caller::Login { username };
    }
    if let Some(v) = crate::session_token::require_hook(presented, path) {
        return caller_from_hook(&v);
    }
    if crate::session_token::validate_hook(presented).is_some() {
        // A real passport on a route that is not an agent verb
        // (`/cli/agent-access/set`).
        return Caller::WrongCredential;
    }
    Caller::Invalid
}

// ── Responses ─────────────────────────────────────────────────────────

fn status_line(code: u16) -> &'static str {
    match code {
        200 => "200 OK",
        400 => "400 Bad Request",
        403 => "403 Forbidden",
        404 => "404 Not Found",
        405 => "405 Method Not Allowed",
        409 => "409 Conflict",
        413 => "413 Payload Too Large",
        _ => "500 Internal Server Error",
    }
}

fn refusal(e: &SidecarError) -> CliResponse {
    CliResponse {
        status: status_line(e.status),
        content_type: "application/json",
        body: serde_json::json!({ "error": { "code": e.code, "hint": e.hint } }).to_string(),
    }
}

fn err(code: &'static str, status: u16, hint: impl Into<String>) -> SidecarError {
    SidecarError::new(code, status, hint)
}

/// 413 for an over-cap body; the caller closes the socket (SC42).
pub fn too_large(path: &str) -> CliResponse {
    let limit = if path == "/cli/sidecar/new" {
        core::NEW_BODY_MAX_BYTES
    } else {
        core::SMALL_BODY_MAX_BYTES
    };
    refusal(&err(
        "body_too_large",
        413,
        format!("request bodies here are at most {} KiB", limit / 1024),
    ))
}

/// Body cap for a sidecar route.
pub fn body_cap(path: &str) -> usize {
    if path == "/cli/sidecar/new" {
        core::NEW_BODY_MAX_BYTES
    } else {
        core::SMALL_BODY_MAX_BYTES
    }
}

/// Is this one of the four routes?
pub fn is_route(path: &str) -> bool {
    matches!(
        path,
        "/cli/sidecar/new" | "/cli/sidecar/list" | "/cli/sidecar/stop" | "/cli/agent-access/set"
    )
}

/// POST-only routes (`require_post` → 405).
pub fn is_post_route(path: &str) -> bool {
    matches!(
        path,
        "/cli/sidecar/new" | "/cli/sidecar/stop" | "/cli/agent-access/set"
    )
}

// ── Audit (§7.6, SC46) ───────────────────────────────────────────────

fn audit(event: &str, caller: &Caller, outcome: &str, ingress: &str) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        event,
        &caller.audit_user(),
        outcome.to_string(),
        ingress,
        "-",
        "cli",
    ));
}

// ── Entry ─────────────────────────────────────────────────────────────

/// Route a sidecar request. Blocking (spawns, DB, file IO): call from
/// `spawn_blocking`. `params` are the query params (list reads
/// `workspace` from there).
pub fn handle(
    path: &str,
    caller: Caller,
    body: &[u8],
    params: &HashMap<String, String>,
    ingress: &str,
) -> CliResponse {
    match path {
        "/cli/sidecar/new" => {
            let (resp, event, outcome) = match handle_new(&caller, body) {
                Ok((v, resumed, outcome)) => (
                    CliResponse::ok_json(v.to_string()),
                    if resumed {
                        "sidecar-resume"
                    } else {
                        "sidecar-new"
                    },
                    outcome,
                ),
                Err(e) => (refusal(&e), "sidecar-new", format!("refused:{}", e.code)),
            };
            audit(event, &caller, &outcome, ingress);
            resp
        }
        "/cli/sidecar/stop" => {
            let (resp, outcome) = match handle_stop(&caller, body) {
                Ok((v, outcome)) => (CliResponse::ok_json(v.to_string()), outcome),
                Err(e) => (refusal(&e), format!("refused:{}", e.code)),
            };
            audit("sidecar-stop", &caller, &outcome, ingress);
            resp
        }
        "/cli/sidecar/list" => match handle_list(&caller, params) {
            Ok(v) => CliResponse::ok_json(v.to_string()),
            Err(e) => refusal(&e),
        },
        "/cli/agent-access/set" => {
            let (resp, outcome) = match handle_access(&caller, body) {
                Ok((v, outcome)) => (CliResponse::ok_json(v.to_string()), outcome),
                Err(e) => (refusal(&e), format!("refused:{}", e.code)),
            };
            audit("agent-access-set", &caller, &outcome, ingress);
            resp
        }
        _ => CliResponse::not_found(),
    }
}

// ── Shared resolution ────────────────────────────────────────────────

struct Workspace {
    id: String,
    path: String,
    handle: String,
}

fn workspace_by_id(id: &str) -> Option<Workspace> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let p = k2_core::db::schema::Project::get(&conn, id).ok()?;
    let handle = k2_core::workspace_session_handles::workspace_address_name(&conn, &p.id)
        .unwrap_or_else(|_| p.id.clone());
    Some(Workspace {
        id: p.id,
        path: p.path,
        handle,
    })
}

fn workspace_by_token(token: &str) -> Option<Workspace> {
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    if let Some(w) = workspace_by_id(token) {
        return Some(w);
    }
    // A path may reach us un-canonicalized (`/var/…` vs the stored
    // `/private/var/…` on macOS): try the canonical form too.
    let path = crate::workspace_msg::resolve_workspace(token).or_else(|| {
        token
            .starts_with('/')
            .then(|| std::fs::canonicalize(token).ok())
            .flatten()
            .and_then(|c| crate::workspace_msg::resolve_workspace(&c.to_string_lossy()))
    })?;
    let id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::workspace::agent_identity::resolve_project_id(&conn, &path)?
    };
    workspace_by_id(&id)
}

/// The target workspace. A passport's workspace always comes from the
/// passport; naming a different one is `forbidden` (§5.2 step 1). Owners
/// and logins must name one.
fn target_workspace(caller: &Caller, requested: Option<&str>) -> Result<Workspace, SidecarError> {
    let requested = requested.map(str::trim).filter(|s| !s.is_empty());
    match caller {
        Caller::Agent(_) | Caller::Shell { .. } => {
            let own = caller.passport_project().unwrap_or_default();
            let ws = workspace_by_id(own).ok_or_else(|| {
                err(
                    "workspace_not_found",
                    404,
                    "your workspace is not registered",
                )
            })?;
            if let Some(req) = requested {
                let other = workspace_by_token(req);
                let same = other.as_ref().is_some_and(|o| o.id == ws.id);
                if !same && matches!(caller, Caller::Agent(_)) {
                    return Err(err(
                        "forbidden",
                        403,
                        "an agent may act only on its own workspace",
                    ));
                }
                if !same {
                    return other.ok_or_else(|| {
                        err("workspace_not_found", 404, format!("no workspace '{req}'"))
                    });
                }
            }
            Ok(ws)
        }
        _ => {
            let req = requested
                .ok_or_else(|| err("workspace_not_found", 404, "pass --workspace <workspace>"))?;
            workspace_by_token(req)
                .ok_or_else(|| err("workspace_not_found", 404, format!("no workspace '{req}'")))
        }
    }
}

fn require_credential(caller: &Caller) -> Result<(), SidecarError> {
    match caller {
        Caller::WrongCredential => Err(err(
            "wrong_credential",
            403,
            "app passes, API keys and API sessions cannot manage sidecars",
        )),
        Caller::Invalid => Err(err("forbidden", 403, "invalid or missing token")),
        _ => Ok(()),
    }
}

const GATED_HINT: &str = "Ask your human to turn on 'Allow hiring and managing agents' in Settings → Workspaces / Agents → {ws} → Agent (or run k2 sidecar access on --workspace {ws} as the owner).";

/// The interim agent gate (§7.3 rules 1–3). Rule 5 (no more power) is
/// checked once the preset is known.
fn agent_gate(caller: &Caller, ws: &Workspace) -> Result<(), SidecarError> {
    let Caller::Agent(a) = caller else {
        return Ok(());
    };
    if !k2_core::workspace::settings::agents_can_manage_agents(&a.project_id) {
        return Err(err("gated", 403, GATED_HINT.replace("{ws}", &ws.handle)));
    }
    if a.project_id != ws.id {
        return Err(err(
            "forbidden",
            403,
            "an agent may act only on its own workspace",
        ));
    }
    if a.map_key.as_deref() != Some(a.project_id.as_str()) {
        return Err(err(
            "forbidden",
            403,
            "only the workspace's main session may open or stop sidecars; sidecars cannot",
        ));
    }
    if a.sandboxed {
        return Err(err(
            "forbidden",
            403,
            "a sandboxed session may not open host sidecars",
        ));
    }
    Ok(())
}

static WORKSPACE_LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();

fn workspace_lock(project_id: &str) -> Arc<Mutex<()>> {
    let map = WORKSPACE_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    map.lock()
        .entry(format!("sidecar:{project_id}"))
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

/// A live `tab-*` map entry.
fn live_session(agent_name: &str) -> Option<Arc<k2_core::terminal::DaemonPtySession>> {
    v2_session_map::lookup_by_agent_name(agent_name).filter(|s| s.is_child_alive())
}

/// Live sidecars in a workspace (app-made included) — the cap count.
fn live_sidecar_count(ws: &Workspace) -> usize {
    let snapshot = v2_session_map::snapshot();
    let db = k2_core::db::shared();
    let conn = db.lock();
    snapshot
        .into_iter()
        .filter(|(key, s)| {
            k2_core::workspace_session_handles::is_tab_agent_name(key)
                && s.is_child_alive()
                && s.program
                    .as_deref()
                    .and_then(core::supported_provider)
                    .is_some()
                && (k2_core::db::schema::WorkspaceTabSession::get_by_agent_name(&conn, &ws.id, key)
                    .ok()
                    .flatten()
                    .is_some()
                    || s.cwd
                        .as_ref()
                        .map(|p| p.to_string_lossy() == ws.path)
                        .unwrap_or(false))
        })
        .count()
}

/// Parse a JSON body (`{}` for an empty one).
fn json_body(body: &[u8]) -> Result<serde_json::Value, SidecarError> {
    let text = std::str::from_utf8(body)
        .map_err(|_| err("brief_not_text", 400, "the request is not UTF-8 text"))?;
    if text.trim().is_empty() {
        return Ok(serde_json::json!({}));
    }
    serde_json::from_str(text)
        .map_err(|e| err("bad_request", 400, format!("request body is not JSON: {e}")))
}

fn str_field<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(|x| x.as_str())
}

/// The durable tab row for a conversation key in a workspace.
fn tab_row_for_key(
    project_id: &str,
    key: &str,
) -> Option<k2_core::db::schema::WorkspaceTabSession> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let by_sid = k2_core::db::schema::WorkspaceTabSession::list_by_project(&conn, project_id)
        .ok()?
        .into_iter()
        .find(|r| {
            r.agent_name.starts_with("tab-")
                && (r.session_id.as_deref().map(str::trim) == Some(key) || r.pane_group_id == key)
        });
    by_sid
}

// ── new (§5.2) ───────────────────────────────────────────────────────

enum Plan {
    New,
    /// Asleep: the tab row is kept.
    Asleep(k2_core::db::schema::WorkspaceTabSession),
    /// Closed: no tab row; `key` is the conversation id.
    Closed {
        key: String,
    },
}

fn handle_new(
    caller: &Caller,
    body: &[u8],
) -> Result<(serde_json::Value, bool, String), SidecarError> {
    require_credential(caller)?;
    let v = json_body(body)?;
    let ws = target_workspace(caller, str_field(&v, "workspace"))?;
    agent_gate(caller, &ws)?;

    let name = str_field(&v, "name").unwrap_or("").trim().to_string();
    let harness = str_field(&v, "harness").unwrap_or("").trim().to_string();
    let model = str_field(&v, "model")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let brief: Option<String> = match v.get("brief") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(_) => return Err(err("brief_not_text", 400, "brief must be a string")),
    };
    if let Some(b) = brief.as_deref() {
        core::check_brief(b)?;
    }
    let brief = brief.filter(|b| !b.trim().is_empty());

    let lock = workspace_lock(&ws.id);
    let _guard = lock.lock();

    let preset = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        core::resolve_preset(&conn, &harness)?
    };
    if let Caller::Agent(a) = caller {
        // §7.3 rule 5: every preset's declared skip-approval flags count.
        let all_danger: Vec<String> = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::db::schema::AgentPreset::list(&conn)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|p| p.danger_flags)
                .filter_map(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
                .flatten()
                .collect()
        };
        if core::carries_skip_approvals(&preset.args, &all_danger)
            && !core::carries_skip_approvals(&a.args, &all_danger)
        {
            return Err(err(
                "forbidden",
                403,
                format!(
                    "preset '{}' skips approvals and your session does not; an agent may not open a sidecar with more power than itself",
                    preset.label
                ),
            ));
        }
    }

    let slug = core::validate_name(&name, &ws.handle)?;
    let address = k2_core::workspace_session_handles::format_address(&ws.handle, Some(&slug));

    // Collision (§5.2 step 4).
    let resolved = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::workspace_session_handles::resolve_handle(&conn, &ws.id, &slug)
    };
    let plan = match resolved {
        Err(e) if e.contains("more than one") => {
            return Err(err(
                "name_ambiguous",
                409,
                format!("'{slug}' matches more than one chat; rename one in Chats"),
            ))
        }
        Err(_) => {
            // Q7 / TR11: a retired name stays reserved to the chat that had
            // it. Refuse instead of making a stranger, which would also
            // overwrite the renamed sidecar's `.k2/sidecars/<name>/BRIEF.md`
            // (its row still points there).
            let owner = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                k2_core::workspace_session_handles::alias_owner(&conn, &ws.id, &slug)
                    .map(|o| o.map(|key| {
                        let ordinal = k2_core::workspace_session_handles::get(&conn, &ws.id, &key)
                            .ok()
                            .flatten()
                            .map(|r| r.ordinal);
                        let current = k2_core::workspace_session_handles::current_handle_for(&conn, &ws.id, &key)
                            .ok()
                            .flatten();
                        (ordinal, current)
                    }))
            };
            match owner {
                Ok(Some((ordinal, current))) => {
                    let was = match ordinal {
                        Some(n) => format!("{}/{n}", ws.handle),
                        None => "another sidecar".to_string(),
                    };
                    let now = current.clone().unwrap_or_else(|| slug.clone());
                    return Err(err(
                        "name_reserved",
                        409,
                        format!(
                            "{slug} was {was}'s name (now {}/{now}); pick another name, or resume it with `k2 sidecar new {now}`",
                            ws.handle
                        ),
                    ));
                }
                Ok(None) => Plan::New,
                Err(e) => return Err(err("internal", 500, e)),
            }
        }
        Ok(key) => {
            let canonical =
                k2_core::workspace_session_handles::conversation_is_canonical_shared(&ws.id, &key);
            if canonical {
                return Err(err(
                    "name_taken",
                    409,
                    format!("'{slug}' is the workspace's main chat"),
                ));
            }
            match tab_row_for_key(&ws.id, &key) {
                Some(row) => {
                    if live_session(&row.agent_name).is_some() {
                        return Err(err(
                            "sidecar_live",
                            409,
                            format!("{address} is already running; message it with: k2 msg {address} \"…\""),
                        ));
                    }
                    let row_provider = row
                        .command
                        .as_deref()
                        .and_then(core::supported_provider)
                        .map(|p| p.provider);
                    if row_provider != Some(preset.provider) {
                        return Err(err(
                            "name_taken",
                            409,
                            format!(
                                "'{slug}' belongs to a {} chat; pick another name",
                                row_provider.unwrap_or("non-sidecar")
                            ),
                        ));
                    }
                    Plan::Asleep(row)
                }
                None => {
                    let named = {
                        let db = k2_core::db::shared();
                        let conn = db.lock();
                        core::named_provider(&conn, &key)
                    };
                    match named.as_deref() {
                        Some(p) if p == preset.provider => {
                            // A codex / hermes sidecar closed before its id was
                            // discovered left its name on the pane key: there is
                            // no conversation to resume, so open it fresh.
                            let never_adopted = core::identity_style(p)
                                == Some(core::IdentityStyle::Discover)
                                && !core::existing_conversation_ids(p, &ws.path).contains(&key);
                            if never_adopted {
                                Plan::New
                            } else {
                                Plan::Closed { key }
                            }
                        }
                        Some(p) => {
                            return Err(err(
                                "name_taken",
                                409,
                                format!("'{slug}' belongs to a {p} chat; pick another name"),
                            ))
                        }
                        None => {
                            return Err(err(
                                "name_taken",
                                409,
                                format!("'{slug}' is already used in this workspace"),
                            ))
                        }
                    }
                }
            }
        }
    };

    // Cap (§8.1).
    let cap = core::cap();
    if live_sidecar_count(&ws) >= cap {
        return Err(err(
            "sidecar_cap",
            409,
            format!(
                "{} already has {cap} live sidecars; stop one first (k2 sidecar stop <name>)",
                ws.handle
            ),
        ));
    }

    // Installed (§5.2 step 6).
    if core::resolve_binary(&preset.program).is_none() {
        return Err(err(
            "harness_not_installed",
            409,
            core::install_hint(&preset.program),
        ));
    }

    let started_by = caller.display();
    let created_by_raw = caller.audit_user();
    let resumed = !matches!(plan, Plan::New);

    // Ids + argv.
    let (pg, mut args, cid): (String, Vec<String>, Option<String>) = match &plan {
        Plan::New => {
            let pg = uuid::Uuid::new_v4().to_string();
            let (args, cid) = core::fresh_identity_args(
                preset.provider,
                &preset.program,
                &preset.args,
                &ws.path,
                &preset.env,
            )
            .map_err(|e| err("harness_not_installed", 409, e))?;
            (pg, args, cid)
        }
        Plan::Asleep(row) => {
            let saved: Vec<String> = row
                .args_json
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_else(|| preset.args.clone());
            let sid = row
                .session_id
                .clone()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            let sid = match sid {
                Some(s) => Some(s),
                None => try_adopt_now(
                    &ws,
                    &row.pane_group_id,
                    preset.provider,
                    &address,
                    brief.is_none(),
                ),
            };
            match sid {
                Some(sid) => (
                    row.pane_group_id.clone(),
                    core::resume_argv(&preset.program, &saved, &sid, &ws.path),
                    Some(sid),
                ),
                None => (
                    row.pane_group_id.clone(),
                    core::strip_session_identity(&preset.program, &saved),
                    None,
                ),
            }
        }
        Plan::Closed { key } => {
            let pg = uuid::Uuid::new_v4().to_string();
            (
                pg,
                core::resume_argv(&preset.program, &preset.args, key, &ws.path),
                Some(key.clone()),
            )
        }
    };
    let program = match &plan {
        Plan::Asleep(row) => row
            .command
            .clone()
            .unwrap_or_else(|| preset.program.clone()),
        _ => preset.program.clone(),
    };
    if let Some(m) = model.as_deref() {
        let (ws_default, force) = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::workspace::model_splice::load_workspace_model(&conn, &ws.path)
        };
        args = k2_core::workspace::model_splice::decide_and_apply(
            &program,
            &args,
            Some(m),
            ws_default.as_deref(),
            resumed,
            force,
        )
        .args;
    }

    // Name binding (SC32): new sidecars only; a resume keeps its name.
    if matches!(plan, Plan::New) {
        let key = cid.clone().unwrap_or_else(|| pg.clone());
        let db = k2_core::db::shared();
        let conn = db.lock();
        // A stale pane-keyed name for a never-adopted, closed sidecar.
        if let Ok(stale) = k2_core::workspace_session_handles::resolve_handle(&conn, &ws.id, &slug)
        {
            if stale != key && tab_row_for_key(&ws.id, &stale).is_none() {
                let _ = conn.execute(
                    "DELETE FROM chat_session_names WHERE session_id = ?1",
                    rusqlite::params![stale],
                );
            }
        }
        core::bind_name(&conn, &ws.id, preset.provider, &key, &slug, &name)?;
    }
    let display_name = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let key = cid.clone().unwrap_or_else(|| pg.clone());
        k2_core::workspace_session_handles::custom_name_for_session_id(&conn, &key)
            .ok()
            .flatten()
            .unwrap_or_else(|| name.clone())
    };

    // Brief (§6.4).
    let brief_info = match brief.as_deref() {
        Some(text) => Some(
            core::write_brief(std::path::Path::new(&ws.path), &slug, text)
                .map_err(|e| err("bad_request", 400, e))?,
        ),
        None => None,
    };
    let launch_prompt = match (&brief_info, resumed, cid.is_some()) {
        (Some(b), false, _) => Some(core::pointer_message(
            &address,
            &ws.handle,
            &started_by,
            &b.rel_path,
        )),
        (Some(b), true, true) => Some(core::brief_changed_message(&address, &b.rel_path)),
        // A resume that could not find the old conversation starts fresh.
        (Some(b), true, false) => Some(core::restarted_message(&address, &ws.handle, &b.rel_path)),
        (None, true, false) => {
            existing_brief(&ws, &pg).map(|rel| core::restarted_message(&address, &ws.handle, &rel))
        }
        (None, _, _) => None,
    };

    // Discover harnesses: ids that exist before the spawn never count.
    let discover = cid.is_none()
        && core::identity_style(preset.provider) == Some(core::IdentityStyle::Discover);
    let exclude = if discover {
        core::existing_conversation_ids(preset.provider, &ws.path)
    } else {
        HashSet::new()
    };
    let since = chrono::Utc::now().timestamp();

    let agent_name = format!("tab-{pg}");
    crate::v2_spawn::clear_closed_tab(&agent_name);
    let outcome =
        crate::spawn::spawn_agent_session_v2_blocking(crate::spawn::SpawnWorkspaceSessionRequest {
            agent_name: agent_name.clone(),
            project_id: Some(ws.id.clone()),
            cwd: ws.path.clone(),
            command: Some(program.clone()),
            args: Some(args.clone()),
            cols: 120,
            rows: 38,
            canonical_key: Some(agent_name.clone()),
            env: preset.env.clone(),
            launch_prompt: launch_prompt.clone(),
            label: Some(display_name.clone()),
        })
        .map_err(|e| err("spawn_failed", 500, e))?;

    // Tab-row sidecar columns (SC38).
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let created = match &plan {
            Plan::Asleep(_) => None,
            _ => Some(created_by_raw.as_str()),
        };
        core::stamp_meta(
            &conn,
            &ws.id,
            &pg,
            created,
            brief_info.as_ref().map(|b| b.rel_path.as_str()),
            discover.then_some(since),
        )
        .map_err(|e| err("spawn_failed", 500, e))?;
    }
    if discover {
        spawn_adoption_watch(AdoptWatch {
            project_id: ws.id.clone(),
            project_path: ws.path.clone(),
            pane_group_id: pg.clone(),
            provider: preset.provider,
            address: address.clone(),
            since,
            exclude,
            has_brief: launch_prompt.is_some(),
        });
    }

    // A harness without a first-turn argument (hermes) gets the pointer
    // pasted once its TUI is ready.
    let delivered = match (&launch_prompt, outcome.launch_prompt_attached) {
        (None, _) => "none",
        (Some(_), true) => "launch_param",
        (Some(p), false) => {
            let sid = outcome.session_id;
            let payload = p.clone();
            let profile = k2_core::workspace::provider_resume::injection_profile_for_provider(
                preset.provider,
            );
            std::thread::spawn(move || {
                let ok = crate::workspace_msg::inject_raw_into_session_with_profile(
                    &sid,
                    &payload,
                    Duration::from_secs(60),
                    &profile,
                );
                log_debug!("[sidecar] post-spawn pointer inject session={sid} delivered={ok}");
            });
            "inject"
        }
    };

    // Saved layout (SC34) + tab title, every client.
    write_layout_and_title(&ws, &pg, &display_name, &program, cid.as_deref());

    let (brief_json, brief_audit) = match &brief_info {
        Some(b) => (
            serde_json::json!({ "path": b.rel_path, "bytes": b.bytes, "delivered": delivered }),
            format!(" brief_bytes={} brief_sha={}", b.bytes, b.sha12),
        ),
        None => (serde_json::Value::Null, String::new()),
    };
    let created_by_out = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        core::meta(&conn, &ws.id, &pg).and_then(|m| m.created_by)
    };
    log_debug!(
        "[sidecar] {} {address} pg={pg} harness={} session={}{}",
        if resumed { "resumed" } else { "opened" },
        preset.provider,
        outcome.session_id,
        brief_audit
    );
    let body = serde_json::json!({
        "ok": true,
        "address": address,
        "workspace": ws.handle,
        "handle": slug,
        "name": display_name,
        "harness": preset.provider,
        "preset": preset.label,
        "model": model,
        "sessionId": outcome.session_id.to_string(),
        "conversationId": cid,
        "paneGroupId": pg,
        "brief": brief_json,
        "resumed": resumed,
        "createdBy": created_by_out.as_deref().map(created_by_display),
    });
    Ok((body, resumed, format!("ok {address}{brief_audit}")))
}

/// The stored brief path for a pane group, when the file still exists.
fn existing_brief(ws: &Workspace, pg: &str) -> Option<String> {
    let rel = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        core::meta(&conn, &ws.id, pg).and_then(|m| m.brief_path)
    }?;
    std::path::Path::new(&ws.path)
        .join(&rel)
        .is_file()
        .then_some(rel)
}

fn write_layout_and_title(ws: &Workspace, pg: &str, title: &str, program: &str, cid: Option<&str>) {
    match core::ensure_layout_tab(&ws.id, pg, title, Some(program), cid) {
        Ok(core::LayoutWrite::Wrote {
            workspace_id,
            revision,
        }) => {
            let _ =
                crate::session_events::emit(crate::session_events::SessionEvent::TabOrderChanged {
                    workspace_path: ws.path.clone(),
                    project: ws.id.clone(),
                    workspace: workspace_id,
                    revision,
                });
        }
        Ok(_) => {}
        Err(e) => log_debug!("[sidecar] layout write failed pg={pg}: {e}"),
    }
    let tab_id = core::adopted_tab_id(pg);
    match k2_core::db_ops::tab_title_set(&ws.id, &tab_id, title, true) {
        Ok(()) => {
            let _ =
                crate::session_events::emit(crate::session_events::SessionEvent::TabTitleChanged {
                    workspace_path: ws.path.clone(),
                    project: ws.id.clone(),
                    tab_id,
                    title: title.to_string(),
                    locked: true,
                });
        }
        Err(e) => log_debug!("[sidecar] tab title write failed pg={pg}: {e}"),
    }
}

// ── list (§5.3) ───────────────────────────────────────────────────────

fn handle_list(
    caller: &Caller,
    params: &HashMap<String, String>,
) -> Result<serde_json::Value, SidecarError> {
    require_credential(caller)?;
    let ws = target_workspace(caller, params.get("workspace").map(String::as_str))?;
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        core::list_rows(&conn, &ws.id).map_err(|e| err("bad_request", 500, e))?
    };
    let mut live = 0usize;
    let sidecars: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|r| {
            let session = live_session(&r.agent_name);
            if session.is_some() {
                live += 1;
            }
            let handle = r.handle.clone();
            serde_json::json!({
                "address": handle.as_deref().map(|h| k2_core::workspace_session_handles::format_address(&ws.handle, Some(h))),
                "handle": handle,
                "name": r.name,
                "harness": r.provider,
                "state": if session.is_some() { "live" } else { "asleep" },
                "attached": session.as_ref().is_some_and(|s| s.subscriber_count() > 0),
                "sessionId": session.as_ref().map(|s| s.session_id.to_string()),
                "conversationId": r.conversation_id,
                "paneGroupId": r.pane_group_id,
                "createdBy": r.created_by.as_deref().map(created_by_display),
                "brief": r.brief_path,
                "lastSeenAt": r.last_seen_at,
            })
        })
        .collect();
    Ok(serde_json::json!({
        "ok": true,
        "workspace": ws.handle,
        "cap": core::cap(),
        "live": live,
        "agentsCanManage": k2_core::workspace::settings::agents_can_manage_agents(&ws.id),
        "sidecars": sidecars,
    }))
}

// ── stop (§5.4) ───────────────────────────────────────────────────────

fn handle_stop(caller: &Caller, body: &[u8]) -> Result<(serde_json::Value, String), SidecarError> {
    require_credential(caller)?;
    let v = json_body(body)?;
    let ws = target_workspace(caller, str_field(&v, "workspace"))?;
    agent_gate(caller, &ws)?;
    let force = v.get("force").and_then(|f| f.as_bool()).unwrap_or(false);
    if force && matches!(caller, Caller::Agent(_)) {
        return Err(err("forbidden", 403, "agents may not use --force"));
    }
    let raw = str_field(&v, "name").unwrap_or("").trim().to_string();
    if raw.is_empty() {
        return Err(err("no_such_sidecar", 404, "name the sidecar to stop"));
    }
    let token = k2_core::workspace_session_handles::slugify_custom_name(&raw)
        .unwrap_or_else(|_| raw.to_lowercase());

    let lock = workspace_lock(&ws.id);
    let _guard = lock.lock();

    let key = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::workspace_session_handles::resolve_handle(&conn, &ws.id, &token)
    }
    .map_err(|_| {
        err(
            "no_such_sidecar",
            404,
            format!("no sidecar '{raw}' in {}", ws.handle),
        )
    })?;
    let row = tab_row_for_key(&ws.id, &key)
        .filter(|r| {
            r.command
                .as_deref()
                .and_then(core::supported_provider)
                .is_some()
        })
        .ok_or_else(|| {
            err(
                "no_such_sidecar",
                404,
                format!("no sidecar '{raw}' in {}", ws.handle),
            )
        })?;
    let handle = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        core::handle_read_only(&conn, &ws.id, &key).unwrap_or(token.clone())
    };
    let address = k2_core::workspace_session_handles::format_address(&ws.handle, Some(&handle));

    let meta = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        core::meta(&conn, &ws.id, &row.pane_group_id).unwrap_or_default()
    };
    if let Caller::Agent(_) = caller {
        let own = format!("agent:{}", ws.handle);
        if meta.created_by.as_deref() != Some(own.as_str()) {
            return Err(err(
                "forbidden",
                403,
                "an agent may stop only sidecars an agent of this workspace opened; ask your human",
            ));
        }
    }

    let Some(session) = live_session(&row.agent_name) else {
        return Ok((
            serde_json::json!({ "ok": true, "address": address, "stopped": false, "state": "asleep" }),
            format!("ok {address} already-asleep"),
        ));
    };
    if session.subscriber_count() > 0 && !force {
        return Err(err(
            "sidecar_attached",
            409,
            format!("someone is viewing {address}; close its tab or pass --force"),
        ));
    }

    // A codex/hermes sidecar that still waits for its id: look once more
    // before the process goes away.
    if row
        .session_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .is_none()
        && meta.adopt_since.is_some()
    {
        if let Some(provider) = row
            .command
            .as_deref()
            .and_then(core::supported_provider)
            .map(|p| p.provider)
        {
            let _ = try_adopt_now(
                &ws,
                &row.pane_group_id,
                provider,
                &address,
                meta.brief_path.is_none(),
            );
        }
    }

    crate::v2_spawn::record_closed_tab(&row.agent_name);
    v2_session_map::unregister(&row.agent_name);
    session.kill();

    match core::remove_layout_tab(&ws.id, &row.pane_group_id) {
        Ok(core::LayoutWrite::Wrote {
            workspace_id,
            revision,
        }) => {
            let _ =
                crate::session_events::emit(crate::session_events::SessionEvent::TabOrderChanged {
                    workspace_path: ws.path.clone(),
                    project: ws.id.clone(),
                    workspace: workspace_id,
                    revision,
                });
        }
        Ok(_) => {}
        Err(e) => log_debug!(
            "[sidecar] layout remove failed pg={}: {e}",
            row.pane_group_id
        ),
    }
    Ok((
        serde_json::json!({ "ok": true, "address": address, "stopped": true, "state": "asleep" }),
        format!("ok {address}"),
    ))
}

// ── access (§5.5) ─────────────────────────────────────────────────────

fn handle_access(
    caller: &Caller,
    body: &[u8],
) -> Result<(serde_json::Value, String), SidecarError> {
    match caller {
        Caller::Owner | Caller::Login { .. } => {}
        Caller::Invalid => return Err(err("forbidden", 403, "invalid or missing token")),
        _ => {
            return Err(err(
                "forbidden",
                403,
                "only the owner or an admin login may change agent access",
            ))
        }
    }
    let v = json_body(body)?;
    let toggle = str_field(&v, "toggle").unwrap_or("");
    if toggle != "agents_manage" {
        return Err(err("bad_request", 400, "toggle must be 'agents_manage'"));
    }
    let value = match v.get("value") {
        Some(serde_json::Value::Bool(b)) => *b,
        Some(serde_json::Value::Number(n)) if n.as_i64() == Some(1) => true,
        Some(serde_json::Value::Number(n)) if n.as_i64() == Some(0) => false,
        _ => return Err(err("bad_request", 400, "value must be true or false")),
    };
    let ws = target_workspace(caller, str_field(&v, "workspace"))?;
    k2_core::workspace::settings::set_agents_can_manage_agents(&ws.id, value)
        .map_err(|e| err("workspace_not_found", 404, e))?;
    k2_core::agent_hooks::emit(
        k2_core::agent_hooks::HookEvent::SyncProjects,
        serde_json::Value::Null,
    );
    Ok((
        serde_json::json!({ "ok": true, "workspace": ws.handle, "toggle": "agents_manage", "agentsCanManage": value }),
        format!("ok {} agents_manage={value}", ws.handle),
    ))
}

// ── Discovery: codex / hermes (k2_core::sidecar::discover_conversation) ──

/// No other live, unadopted session of this harness runs in the
/// workspace — the only case an unmarked conversation may be adopted.
fn alone_in_workspace(ws: &Workspace, own_pg: &str, provider: &str) -> bool {
    let own_key = format!("tab-{own_pg}");
    let snapshot = v2_session_map::snapshot();
    let db = k2_core::db::shared();
    let conn = db.lock();
    !snapshot.into_iter().any(|(key, s)| {
        if key == own_key || !s.is_child_alive() {
            return false;
        }
        let same_provider = s
            .program
            .as_deref()
            .and_then(core::supported_provider)
            .is_some_and(|p| p.provider == provider);
        let in_ws = s
            .cwd
            .as_ref()
            .map(|p| p.to_string_lossy() == ws.path)
            .unwrap_or(false);
        if !same_provider || !in_ws {
            return false;
        }
        let known = k2_core::workspace::provider_resume::session_id_from_spawn_argv(
            s.program.as_deref().unwrap_or(""),
            &s.args,
        )
        .is_some()
            || k2_core::db::schema::WorkspaceTabSession::get_by_agent_name(&conn, &ws.id, &key)
                .ok()
                .flatten()
                .and_then(|r| r.session_id)
                .is_some();
        !known
    })
}

/// One discovery attempt for a pane group; adopts and returns the id.
fn try_adopt_now(
    ws: &Workspace,
    pg: &str,
    provider: &str,
    address: &str,
    no_brief: bool,
) -> Option<String> {
    if core::identity_style(provider) != Some(core::IdentityStyle::Discover) {
        return None;
    }
    let since = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        core::meta(&conn, &ws.id, pg).and_then(|m| m.adopt_since)
    }?;
    let claimed = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        core::claimed_conversation_ids(&conn)
    };
    let allow_unmarked = no_brief && alone_in_workspace(ws, pg, provider);
    let cid = core::discover_conversation(
        provider,
        &ws.path,
        address,
        since,
        &HashSet::new(),
        &claimed,
        allow_unmarked,
    )?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    match core::adopt_conversation(&conn, &ws.id, pg, provider, &cid) {
        Ok(()) => {
            log_debug!("[sidecar] adopted {provider} conversation {cid} for {address}");
            Some(cid)
        }
        Err(e) => {
            log_debug!("[sidecar] adopt failed for {address}: {e}");
            None
        }
    }
}

struct AdoptWatch {
    project_id: String,
    project_path: String,
    pane_group_id: String,
    provider: &'static str,
    address: String,
    since: i64,
    exclude: HashSet<String>,
    has_brief: bool,
}

/// Poll for the conversation a codex / hermes sidecar writes: every 2 s
/// for a minute, every 10 s to 10 min, every 30 s to 30 min. Stops once
/// the row has an id (adopted here or elsewhere) or `adopt_since` is
/// cleared.
fn spawn_adoption_watch(w: AdoptWatch) {
    let _ = std::thread::Builder::new()
        .name("k2-sidecar-adopt".to_string())
        .spawn(move || {
            let ws = Workspace {
                id: w.project_id.clone(),
                path: w.project_path.clone(),
                handle: String::new(),
            };
            let start = std::time::Instant::now();
            loop {
                let elapsed = start.elapsed();
                if elapsed > Duration::from_secs(30 * 60) {
                    log_debug!("[sidecar] adoption watch gave up for {}", w.address);
                    return;
                }
                let step = if elapsed < Duration::from_secs(60) {
                    Duration::from_secs(2)
                } else if elapsed < Duration::from_secs(600) {
                    Duration::from_secs(10)
                } else {
                    Duration::from_secs(30)
                };
                std::thread::sleep(step);
                let (waiting, claimed) = {
                    let db = k2_core::db::shared();
                    let conn = db.lock();
                    let row = k2_core::db::schema::WorkspaceTabSession::get(
                        &conn,
                        &w.project_id,
                        &w.pane_group_id,
                    )
                    .ok()
                    .flatten();
                    let waiting = row.is_some_and(|r| {
                        r.session_id
                            .as_deref()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .is_none()
                    }) && core::meta(&conn, &w.project_id, &w.pane_group_id)
                        .and_then(|m| m.adopt_since)
                        .is_some();
                    (waiting, core::claimed_conversation_ids(&conn))
                };
                if !waiting {
                    return;
                }
                let allow_unmarked =
                    !w.has_brief && alone_in_workspace(&ws, &w.pane_group_id, w.provider);
                if let Some(cid) = core::discover_conversation(
                    w.provider,
                    &w.project_path,
                    &w.address,
                    w.since,
                    &w.exclude,
                    &claimed,
                    allow_unmarked,
                ) {
                    let db = k2_core::db::shared();
                    let conn = db.lock();
                    match core::adopt_conversation(
                        &conn,
                        &w.project_id,
                        &w.pane_group_id,
                        w.provider,
                        &cid,
                    ) {
                        Ok(()) => log_debug!(
                            "[sidecar] adopted {} conversation {cid} for {}",
                            w.provider,
                            w.address
                        ),
                        Err(e) => log_debug!("[sidecar] adopt failed for {}: {e}", w.address),
                    }
                    return;
                }
            }
        });
}

// ── Relaunch plan: restart recovery + msg wake (SC48) ────────────────

/// How to bring a sleeping sidecar back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relaunch {
    pub args: Vec<String>,
    /// Fire-once first turn (only for a fresh restart with a brief).
    pub launch_prompt: Option<String>,
}

/// Build the argv that brings tab row `(project_id, pane_group_id)` back
/// as its own harness. Shared by `v2_spawn::recovered_launch` and the
/// `k2 msg` sidecar wake.
///
/// - Stored conversation id → [`core::resume_argv`] (SC48).
/// - No id, a CLI sidecar of a discover harness (codex / hermes) → one
///   discovery attempt; if found, adopt + resume. Otherwise a fresh
///   session of the same harness that re-reads BRIEF.md, with a new
///   adoption watch.
/// - Anything else without an id → `None` (callers keep today's rule:
///   an app tab with no id comes back as a shell).
pub fn relaunch_plan(
    project_id: &str,
    pane_group_id: &str,
    cwd: &str,
    command: &str,
    saved_args: &[String],
    session_id: Option<&str>,
) -> Option<Relaunch> {
    let sid = session_id.map(str::trim).filter(|s| !s.is_empty());
    if let Some(sid) = sid {
        return Some(Relaunch {
            args: core::resume_argv(command, saved_args, sid, cwd),
            launch_prompt: None,
        });
    }
    let provider = core::supported_provider(command)?.provider;
    if core::identity_style(provider) != Some(core::IdentityStyle::Discover) {
        return None;
    }
    let meta = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        core::meta(&conn, project_id, pane_group_id)
    }?;
    // Only CLI sidecars (created_by set) get the fresh fallback.
    meta.created_by.as_ref()?;
    let ws = workspace_by_id(project_id)?;
    let handle = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        core::handle_read_only(&conn, project_id, pane_group_id)
    };
    let address = k2_core::workspace_session_handles::format_address(&ws.handle, handle.as_deref());
    if let Some(cid) = try_adopt_now(
        &ws,
        pane_group_id,
        provider,
        &address,
        meta.brief_path.is_none(),
    ) {
        return Some(Relaunch {
            args: core::resume_argv(command, saved_args, &cid, cwd),
            launch_prompt: None,
        });
    }
    // Fresh restart: a new conversation of the same harness.
    let since = chrono::Utc::now().timestamp();
    let exclude = core::existing_conversation_ids(provider, &ws.path);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = conn.execute(
            "UPDATE workspace_tab_sessions SET adopt_since = ?3 WHERE project_id = ?1 AND pane_group_id = ?2",
            rusqlite::params![project_id, pane_group_id, since],
        );
    }
    let launch_prompt = existing_brief(&ws, pane_group_id)
        .map(|rel| core::restarted_message(&address, &ws.handle, &rel));
    spawn_adoption_watch(AdoptWatch {
        project_id: project_id.to_string(),
        project_path: ws.path.clone(),
        pane_group_id: pane_group_id.to_string(),
        provider,
        address,
        since,
        exclude,
        has_brief: launch_prompt.is_some(),
    });
    Some(Relaunch {
        args: core::strip_session_identity(command, saved_args),
        launch_prompt,
    })
}

/// The Chats display name of a `tab-*` sidecar (SC30), for its label.
pub fn sidecar_display_name(pane_group_id: &str, session_id: Option<&str>) -> Option<String> {
    let key = k2_core::workspace_session_handles::conversation_key_for(session_id, pane_group_id);
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::workspace_session_handles::custom_name_for_session_id(&conn, &key)
        .ok()
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn created_by_display_forms() {
        assert_eq!(created_by_display("agent:k2"), "k2 (agent)");
        assert_eq!(created_by_display("shell:k2"), "k2 (shell)");
        assert_eq!(created_by_display("login:ana"), "ana");
        assert_eq!(created_by_display("owner"), "owner");
    }

    #[test]
    fn route_shapes() {
        assert!(is_route("/cli/sidecar/new"));
        assert!(is_route("/cli/agent-access/set"));
        assert!(!is_route("/cli/sidecar/other"));
        assert!(is_post_route("/cli/sidecar/stop"));
        assert!(!is_post_route("/cli/sidecar/list"));
        assert_eq!(body_cap("/cli/sidecar/new"), 96 * 1024);
        assert_eq!(body_cap("/cli/sidecar/stop"), 4 * 1024);
    }
}
