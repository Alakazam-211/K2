//! Agent identity surface (0.45.5): an App guest names "their" agent.
//!
//! A room is a workspace is an agent. Settings → Workspaces → Agent →
//! Identity has long let the owner set the agent's picture, color, name and
//! handle; these two routes give an App the same fields, jailed to a room:
//!
//! ```text
//! GET  /cli/agent/identity?workspace=<handle>        the room is enough
//! POST /cli/agent/identity {workspace, displayName?, color?, image?}
//!                                                    agent:identity
//! POST /cli/agent/handle   {workspace, handle}       agent:handle
//! ```
//!
//! - **Two caps.** `agent:identity` (name, color, picture) and
//!   `agent:handle` (the address). Neither implies the other and neither is
//!   on by default. Room first (403 `skin_room`), then the cap in that room
//!   (403 `missing capability <cap>`), the same order as every app door.
//! - **Same writes as the owner.** The name goes through
//!   [`crate::workspace_routes::apply_agent_display_name`] and the handle
//!   through [`crate::workspace_routes::apply_workspace_handle`], the exact
//!   paths `set-agent-display-name` and `set-handle` use, so addresses stay
//!   consistent (the old handle is kept as an alias, as for an owner rename).
//! - **Checks in the daemon.** Name: the display-name rules (≤ 64 chars, no
//!   `/`, `:` or control characters). Color: `#rgb` / `#rrggbb`. Picture:
//!   PNG/JPEG/WebP data URL, checked against its bytes, decoded with limits
//!   and re-encoded (`k2_core::workspace::agent_look`). SVG is refused. A
//!   body is validated whole before anything is written.
//! - **Owner and Connect logins** may use the same routes (`workspace=` is
//!   then a handle, uuid, name or path). That is what `k2 workspace
//!   set-agent-identity` calls.
//! - **Audit.** Every change is one `~/.k2/auth-audit.jsonl` line
//!   (`agent.identity.set` / `agent.handle.set`): who (`app:<user>`,
//!   `owner-token`, `user:<name>`), which agent, old → new. A picture is
//!   logged as a short fingerprint, never its bytes.
//! - **Events.** One `agent_identity_changed` per request (the shared
//!   event every client patches from; `fields` names what was written),
//!   plus `SyncProjects`. App sockets get it as the payload-free
//!   `rooms_changed` (`activity_events_ws`).
//! - **Rate limit (apps and agents).** [`WRITES_PER_PASS_MAX`] writes per
//!   pass per room per [`WRITES_PER_PASS_WINDOW`]; an agent's budget is
//!   per workspace (every session of it shares one). 429 with `Retry-After`.
//! - **An agent's own look (0.46.1).** `k2 agent identity` from an agent's
//!   session sends its passport. [`caller_from_passport`] admits it on
//!   `/cli/agent/identity` only (never `/cli/agent/handle`), for **its own
//!   workspace** (no `workspace` = itself; any other target, real or not,
//!   is one 403 `not_your_agent`, no oracle) and for **color and picture
//!   only**: a name (or handle) is 403 `agent_cannot_rename`. Same checks,
//!   same writes, same `agent_identity_changed` event; audit actor
//!   `agent:<workspace>/<session>`. Sandboxed (microVM) sessions, API cells,
//!   passports this daemon did not mint, and passports that arrive from off
//!   this machine (LAN, tunnel, App link, a local proxy) are refused (403
//!   `remote_passport`; the cell socket is local by nature). A K2 shell tab's
//!   passport is a person (noun tiers NT5): owner rules, actor
//!   `shell:<workspace>/<session>`.

use std::collections::{HashMap, VecDeque};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde_json::{json, Value};

use k2_core::skin::{SkinPass, CAP_AGENT_HANDLE, CAP_AGENT_IDENTITY};
use k2_core::workspace::agent_look::{self, LookError};

use crate::cli_response::CliResponse;
use crate::fs_routes::SkinWorkspace;

pub const IDENTITY_PATH: &str = "/cli/agent/identity";
pub const HANDLE_PATH: &str = "/cli/agent/handle";

/// The whole POST body, in bytes: a 2 MiB picture as base64 plus JSON.
pub const MAX_BODY: usize = agent_look::MAX_INPUT_IMAGE_BYTES / 3 * 4 + 64 * 1024;

/// Longest handle this route sets (after slugging). The owner's
/// `set-handle` has no cap of its own; an address this long is already
/// unwieldy in `name::host`.
pub const MAX_HANDLE_CHARS: usize = 64;

/// App writes: at most this many per pass per room per window.
pub const WRITES_PER_PASS_MAX: usize = 20;
pub const WRITES_PER_PASS_WINDOW: Duration = Duration::from_secs(600);

/// True for the two doors.
pub fn is_door(path: &str) -> bool {
    path == IDENTITY_PATH || path == HANDLE_PATH
}

/// Who is calling. Built by the dispatcher after auth.
#[derive(Debug, Clone)]
pub enum Caller {
    /// An App pass (`k2skn_`): jailed to its rooms, caps per room.
    App(SkinPass),
    /// The owner token or a Connect login (any role at or above the route
    /// floor), or a K2 shell tab's passport. `actor` is the audit name;
    /// `home` is the shell tab's workspace, used when none is named.
    Owner { actor: String, home: Option<String> },
    /// An agent's own session passport: its own workspace, color and
    /// picture only (see the module docs).
    Agent { project_id: String, session_id: String },
}

impl Caller {
    fn actor(&self) -> String {
        match self {
            Caller::App(pass) => crate::routes::http::app_actor_label(pass),
            Caller::Owner { actor, .. } => actor.clone(),
            Caller::Agent { project_id, session_id } => {
                format!("agent:{}/{session_id}", workspace_label(project_id))
            }
        }
    }

    fn client(&self) -> &'static str {
        match self {
            Caller::App(_) => "app",
            Caller::Owner { .. } => "cli",
            Caller::Agent { .. } => "agent",
        }
    }
}

/// The workspace's handle, else its name, else its id (audit labels).
fn workspace_label(project_id: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::db::schema::Project::get(&conn, project_id)
        .ok()
        .map(|p| {
            let h = p.handle.trim().to_string();
            if h.is_empty() {
                p.name.trim().to_string()
            } else {
                h
            }
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| project_id.to_string())
}

// ── Agent passports ────────────────────────────────────────────────────

fn forbidden(code: &str, message: &str) -> CliResponse {
    json_response("403 Forbidden", json!({ "error": code, "message": message }))
}

/// What an agent sees when it names another agent (or one that does not
/// exist: same answer, no oracle).
fn not_your_agent() -> CliResponse {
    forbidden(
        "not_your_agent",
        "an agent can change only its own picture and color; leave out the agent name",
    )
}

/// What an agent sees when it tries to change its name or handle.
pub fn agent_cannot_rename() -> CliResponse {
    forbidden(
        "agent_cannot_rename",
        "agents can't rename themselves; ask your owner (k2 agent identity --name, from their terminal or Settings)",
    )
}

/// A passport that did not arrive from this machine (LAN, tunnel, App
/// link): never an agent of this server acting on itself.
pub fn remote_passport() -> CliResponse {
    forbidden(
        "remote_passport",
        "an agent passport works only from its own session on this server",
    )
}

/// Classify a passport this daemon validated (TCP bearer or the cell
/// socket) for the identity door.
pub fn caller_from_passport(
    session_id: &str,
    principal: &crate::session_token::HookPrincipal,
) -> Result<Caller, CliResponse> {
    classify_passport(
        crate::sidecar_routes::caller_from_session(session_id, principal),
        session_id,
    )
}

fn classify_passport(c: crate::sidecar_routes::Caller, session_id: &str) -> Result<Caller, CliResponse> {
    use crate::sidecar_routes::Caller as S;
    match c {
        S::Agent(a) if a.sandboxed => Err(forbidden(
            "sandboxed",
            "a sandboxed agent can't change its picture or color; ask your owner",
        )),
        S::Agent(a) => Ok(Caller::Agent {
            project_id: a.project_id,
            session_id: session_id.to_string(),
        }),
        // A K2 shell tab is a person typing in K2 (NT5).
        S::Shell { project_id } => Ok(Caller::Owner {
            actor: format!("shell:{}/{session_id}", workspace_label(&project_id)),
            home: Some(project_id),
        }),
        S::Owner | S::Login { .. } | S::WrongCredential | S::Invalid => Err(forbidden(
            "wrong_credential",
            "this credential can't change an agent's look",
        )),
    }
}

// ── Responses ──────────────────────────────────────────────────────────

fn json_response(status: &'static str, body: Value) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: body.to_string(),
    }
}

fn bad(code: &str, message: impl Into<String>) -> CliResponse {
    json_response(
        "400 Bad Request",
        json!({ "error": code, "message": message.into() }),
    )
}

fn method_not_allowed() -> CliResponse {
    json_response(
        "405 Method Not Allowed",
        json!({ "error": "method not allowed for this route" }),
    )
}

fn rate_limited(retry_after_secs: u64) -> CliResponse {
    json_response(
        "429 Too Many Requests",
        json!({ "error": "rate_limited", "retryAfterSecs": retry_after_secs }),
    )
}

fn look_error(e: &LookError) -> CliResponse {
    let field = match e {
        LookError::BadColor(_) => "color",
        _ => "image",
    };
    json_response(
        "400 Bad Request",
        json!({ "error": e.code(), "field": field, "message": e.to_string() }),
    )
}

/// The `Retry-After` line for a 429 from these doors. Empty otherwise.
pub fn extra_headers(resp: &CliResponse) -> String {
    if !resp.status.starts_with("429") {
        return String::new();
    }
    serde_json::from_str::<Value>(&resp.body)
        .ok()
        .and_then(|v| v.get("retryAfterSecs").and_then(Value::as_u64))
        .map(|s| format!("Retry-After: {s}\r\n"))
        .unwrap_or_default()
}

// ── Rate limit ─────────────────────────────────────────────────────────

fn write_log() -> &'static Mutex<HashMap<(String, String), VecDeque<Instant>>> {
    static LOG: OnceLock<Mutex<HashMap<(String, String), VecDeque<Instant>>>> = OnceLock::new();
    LOG.get_or_init(|| Mutex::new(HashMap::new()))
}

/// `Ok` and the write is counted, or `Err(retry after secs)`.
fn take_write_slot(pass_id: &str, project_id: &str) -> Result<(), u64> {
    let now = Instant::now();
    let mut log = write_log().lock();
    let q = log
        .entry((pass_id.to_string(), project_id.to_string()))
        .or_default();
    while q
        .front()
        .is_some_and(|t| now.duration_since(*t) >= WRITES_PER_PASS_WINDOW)
    {
        q.pop_front();
    }
    if q.len() >= WRITES_PER_PASS_MAX {
        let oldest = *q.front().expect("non-empty when full");
        let wait = WRITES_PER_PASS_WINDOW.saturating_sub(now.duration_since(oldest));
        return Err(wait.as_secs().max(1));
    }
    q.push_back(now);
    Ok(())
}

// ── Room resolution ────────────────────────────────────────────────────

fn workspace_token(params: &HashMap<String, String>, body: Option<&Value>) -> String {
    let from_body = body
        .and_then(|b| b.get("workspace"))
        .and_then(Value::as_str)
        .map(str::to_string);
    from_body
        .or_else(|| params.get("workspace").cloned())
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// App: the room on the pass (403 `skin_room` otherwise, no oracle).
/// Owner: a handle or uuid, else a name or path.
/// Agent: its own workspace; no token means itself, any other target
/// (real or not) is 403 `not_your_agent`.
fn resolve(caller: &Caller, token: &str) -> Result<SkinWorkspace, CliResponse> {
    if let Caller::Agent { project_id, .. } = caller {
        let own = workspace_by_id(project_id).ok_or_else(not_your_agent)?;
        if token.is_empty() {
            return Ok(own);
        }
        return match resolve_any(token) {
            Ok(ws) if ws.project_id == own.project_id => Ok(own),
            _ => Err(not_your_agent()),
        };
    }
    if token.is_empty() {
        // A K2 shell tab with no agent named: its own workspace.
        if let Caller::Owner { home: Some(home), .. } = caller {
            return workspace_by_id(home).ok_or_else(|| {
                json_response(
                    "404 Not Found",
                    json!({ "error": "workspace_not_found", "message": "this tab's workspace is gone" }),
                )
            });
        }
        return Err(bad("usage", "missing workspace (the agent's handle)"));
    }
    match caller {
        Caller::App(pass) => crate::fs_routes::resolve_skin_workspace(pass, token),
        Caller::Owner { .. } | Caller::Agent { .. } => resolve_any(token),
    }
}

fn workspace_by_id(project_id: &str) -> Option<SkinWorkspace> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT id, path, COALESCE(handle, '') FROM projects WHERE id = ?1",
        rusqlite::params![project_id],
        |r| {
            Ok(SkinWorkspace {
                project_id: r.get(0)?,
                path: r.get(1)?,
                handle: r.get(2)?,
            })
        },
    )
    .ok()
}

/// Owner resolution: a handle or uuid, else a name or path.
fn resolve_any(token: &str) -> Result<SkinWorkspace, CliResponse> {
    if let Ok(ws) = crate::fs_routes::resolve_owner_workspace(token) {
        return Ok(ws);
    }
    let not_found = || {
        json_response(
            "404 Not Found",
            json!({ "error": "workspace_not_found", "message": format!("no workspace {token:?}") }),
        )
    };
    let path = crate::workspace_msg::resolve_workspace(token).ok_or_else(not_found)?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT id, path, COALESCE(handle, '') FROM projects WHERE path = ?1",
        rusqlite::params![path],
        |r| {
            Ok(SkinWorkspace {
                project_id: r.get(0)?,
                path: r.get(1)?,
                handle: r.get(2)?,
            })
        },
    )
    .map_err(|_| not_found())
}

fn need_cap(caller: &Caller, ws: &SkinWorkspace, cap: &str) -> Result<(), CliResponse> {
    match caller {
        Caller::App(pass) if !pass.has_cap_in_room(&ws.project_id, cap) => {
            Err(crate::skin_routes::missing_cap_response(cap))
        }
        _ => Ok(()),
    }
}

// ── The view ───────────────────────────────────────────────────────────

/// What both doors answer: the agent as the caller may see it.
fn view(caller: &Caller, project_id: &str) -> Value {
    let (path, handle, color, icon): (String, String, String, Option<String>) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT path, COALESCE(handle, ''), color, icon_url FROM projects WHERE id = ?1",
            rusqlite::params![project_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap_or_default()
    };
    let display_name = k2_core::workspace::display::agent_display_name(&path);
    let (image, can_identity, can_handle) = match caller {
        Caller::App(pass) => (
            agent_look::guest_image_view(icon.as_deref()),
            pass.has_cap_in_room(project_id, CAP_AGENT_IDENTITY),
            pass.has_cap_in_room(project_id, CAP_AGENT_HANDLE),
        ),
        Caller::Owner { .. } => (icon, true, true),
        // Its own look, as stored; never its handle.
        Caller::Agent { .. } => (icon, true, false),
    };
    json!({
        "workspace": handle,
        "handle": handle,
        "displayName": display_name,
        "color": agent_look::normalize_color(&color).ok(),
        "image": image,
        "canEditIdentity": can_identity,
        "canEditHandle": can_handle,
    })
}

// ── Entry ──────────────────────────────────────────────────────────────

/// Dispatcher entry for both doors (auth already resolved into `caller`).
pub fn handle(
    method: &str,
    path: &str,
    params: &HashMap<String, String>,
    body: &[u8],
    caller: Caller,
) -> CliResponse {
    let is_post = method.eq_ignore_ascii_case("POST");
    match (path, is_post) {
        (IDENTITY_PATH, false) => {
            let token = workspace_token(params, None);
            match resolve(&caller, &token) {
                Ok(ws) => CliResponse::ok_json(view(&caller, &ws.project_id).to_string()),
                Err(r) => r,
            }
        }
        (IDENTITY_PATH, true) => post_identity(params, body, &caller),
        (HANDLE_PATH, true) => post_handle(params, body, &caller),
        (HANDLE_PATH, false) => method_not_allowed(),
        _ => json_response("404 Not Found", json!({ "error": "not found" })),
    }
}

fn parse_body(body: &[u8]) -> Result<Value, CliResponse> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Err(bad("usage", "empty body; send JSON"));
    }
    let v: Value = serde_json::from_slice(body)
        .map_err(|e| bad("usage", format!("invalid JSON body: {e}")))?;
    if !v.is_object() {
        return Err(bad("usage", "body must be a JSON object"));
    }
    Ok(v)
}

/// A string field: absent → `None`; a string → `Some(Some(s))`; JSON
/// `null` → `Some(None)` (only where clearing is allowed); anything else
/// is a usage error.
fn opt_string(v: &Value, key: &str, nullable: bool) -> Result<Option<Option<String>>, CliResponse> {
    match v.get(key) {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(Some(s.clone()))),
        Some(Value::Null) if nullable => Ok(Some(None)),
        Some(_) => Err(bad(
            "usage",
            if nullable {
                format!("{key} must be a string or null")
            } else {
                format!("{key} must be a string")
            },
        )),
    }
}

fn post_identity(params: &HashMap<String, String>, body: &[u8], caller: &Caller) -> CliResponse {
    let v = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let token = workspace_token(params, Some(&v));
    let ws = match resolve(caller, &token) {
        Ok(ws) => ws,
        Err(r) => return r,
    };
    if let Err(r) = need_cap(caller, &ws, CAP_AGENT_IDENTITY) {
        return r;
    }
    // An agent changes its color and picture, never its name or handle.
    if matches!(caller, Caller::Agent { .. })
        && ["displayName", "display_name", "name", "handle"]
            .iter()
            .any(|k| v.get(*k).is_some())
    {
        return agent_cannot_rename();
    }
    // Accept the snake_case spelling too (the CLI's other routes use it).
    let name_key = if v.get("displayName").is_some() { "displayName" } else { "display_name" };
    let name = match opt_string(&v, name_key, false) {
        Ok(n) => n.flatten(),
        Err(r) => return r,
    };
    let color = match opt_string(&v, "color", false) {
        Ok(c) => c.flatten(),
        Err(r) => return r,
    };
    let image = match opt_string(&v, "image", true) {
        Ok(i) => i,
        Err(r) => return r,
    };
    if name.is_none() && color.is_none() && image.is_none() {
        return bad(
            "nothing_to_change",
            "send at least one of displayName, color or image",
        );
    }

    // Validate everything before writing anything.
    let name = match name {
        Some(n) => {
            let n = n.trim().to_string();
            if let Err(e) = k2_core::workspace::display::validate_display_name(&n) {
                return json_response(
                    "400 Bad Request",
                    json!({ "error": "bad_name", "field": "displayName", "message": e }),
                );
            }
            if k2_core::workspace::display::is_persona_file_name(&n) {
                return json_response(
                    "400 Bad Request",
                    json!({
                        "error": "bad_name",
                        "field": "displayName",
                        "message": k2_core::workspace::display::PERSONA_FILE_NAME_HINT,
                    }),
                );
            }
            Some(n)
        }
        None => None,
    };
    let color = match color.map(|c| agent_look::normalize_color(&c)) {
        Some(Ok(c)) => Some(c),
        Some(Err(e)) => return look_error(&e),
        None => None,
    };
    // `Some(None)` clears the picture; `""` clears it too.
    let image: Option<Option<String>> = match image {
        Some(Some(s)) if s.trim().is_empty() => Some(None),
        Some(Some(s)) => match agent_look::normalize_image_data_url(&s) {
            Ok(url) => Some(Some(url)),
            Err(e) => return look_error(&e),
        },
        Some(None) => Some(None),
        None => None,
    };

    let budget = match caller {
        Caller::App(pass) => Some(pass.id.clone()),
        Caller::Agent { project_id, .. } => Some(format!("agent:{project_id}")),
        Caller::Owner { .. } => None,
    };
    if let Some(key) = budget {
        if let Err(secs) = take_write_slot(&key, &ws.project_id) {
            return rate_limited(secs);
        }
    }

    let old_name = k2_core::workspace::display::agent_display_name(&ws.path);
    let (old_color, old_icon) = agent_look::stored_look(&ws.project_id).unwrap_or_default();
    let mut changed: Vec<&str> = Vec::new();
    let mut audit: Vec<String> = Vec::new();

    // The event's `fields` words for what was written.
    let mut fields: Vec<&str> = Vec::new();
    if let Some(ref n) = name {
        if *n != old_name {
            if let Err(e) = crate::workspace_routes::write_agent_display_name(&ws.path, n) {
                return bad("bad_name", e);
            }
            changed.push("displayName");
            fields.extend(["displayName", "name"]);
            audit.push(format!("displayName={old_name:?}->{n:?}"));
        }
    }
    if let Some(ref c) = color {
        if *c != old_color.to_ascii_lowercase() {
            if let Err(e) = agent_look::set_color(&ws.project_id, c) {
                return CliResponse::internal_error(e);
            }
            changed.push("color");
            fields.push("color");
            audit.push(format!("color={old_color}->{c}"));
        }
    }
    if let Some(ref img) = image {
        if img.as_deref() != old_icon.as_deref() {
            if let Err(e) = agent_look::set_image(&ws.project_id, img.as_deref()) {
                return CliResponse::internal_error(e);
            }
            changed.push("image");
            fields.push("image");
            audit.push(format!(
                "image={}->{}",
                agent_look::image_fingerprint(old_icon.as_deref()),
                agent_look::image_fingerprint(img.as_deref())
            ));
        }
    }
    // One event for the whole request, naming every field written.
    crate::workspace_routes::emit_identity_refresh(
        crate::session_events::AgentIdentityTarget::Id(&ws.project_id),
        &fields,
    );
    if !audit.is_empty() {
        record_audit("agent.identity.set", caller, &ws, &audit.join(" "));
    }

    let mut out = view(caller, &ws.project_id);
    out["ok"] = json!(true);
    out["changed"] = json!(changed);
    CliResponse::ok_json(out.to_string())
}

fn post_handle(params: &HashMap<String, String>, body: &[u8], caller: &Caller) -> CliResponse {
    let v = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let token = workspace_token(params, Some(&v));
    let ws = match resolve(caller, &token) {
        Ok(ws) => ws,
        Err(r) => return r,
    };
    if let Err(r) = need_cap(caller, &ws, CAP_AGENT_HANDLE) {
        return r;
    }
    // The dispatcher never admits a passport here; belt.
    if matches!(caller, Caller::Agent { .. }) {
        return agent_cannot_rename();
    }
    let requested = match v.get("handle") {
        Some(Value::String(s)) if !s.trim().is_empty() => s.trim().to_string(),
        _ => return bad("usage", "missing handle (a string)"),
    };
    // The same slug rules `set-handle` applies; checked here first so a
    // refusal never names another workspace to an App.
    let slug = match k2_core::workspace_session_handles::slugify_address_token(&requested) {
        Ok(s) => s,
        Err(e) => {
            return json_response(
                "400 Bad Request",
                json!({ "error": "bad_handle", "field": "handle", "message": e }),
            )
        }
    };
    if slug.len() > MAX_HANDLE_CHARS {
        return json_response(
            "400 Bad Request",
            json!({
                "error": "bad_handle",
                "field": "handle",
                "message": format!("handle must be at most {MAX_HANDLE_CHARS} characters"),
            }),
        );
    }
    let old_handle = ws.handle.clone();
    if slug.eq_ignore_ascii_case(&old_handle) {
        let mut out = view(caller, &ws.project_id);
        out["ok"] = json!(true);
        out["previousHandle"] = json!(old_handle);
        out["changed"] = json!(Vec::<&str>::new());
        return CliResponse::ok_json(out.to_string());
    }
    let (retired, taken) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        (
            k2_core::workspace::handle::is_retired_for_other(&conn, &slug, &ws.project_id),
            k2_core::workspace::handle::handle_collision_owner(&conn, &slug, Some(&ws.project_id)),
        )
    };
    // Handles are never reused (0159): a removed workspace's handle is
    // refused like a live one, without naming whose it was.
    if retired {
        return json_response(
            "409 Conflict",
            json!({
                "error": "handle_retired",
                "field": "handle",
                "message": format!("handle {slug:?} belonged to a removed agent; handles are never reused"),
            }),
        );
    }
    if let Some(other) = taken {
        let message = match caller {
            Caller::App(_) => format!("handle {slug:?} is already used on this server"),
            Caller::Owner { .. } | Caller::Agent { .. } => {
                format!("Handle '{slug}' is already used by workspace '{other}'.")
            }
        };
        return json_response(
            "409 Conflict",
            json!({ "error": "handle_taken", "field": "handle", "message": message }),
        );
    }
    if let Caller::App(pass) = caller {
        if let Err(secs) = take_write_slot(&pass.id, &ws.project_id) {
            return rate_limited(secs);
        }
    }
    let new_handle = match crate::workspace_routes::apply_workspace_handle(&ws.path, &requested) {
        Ok(h) => h,
        Err(e) => {
            let code = if e.contains("already used") { "handle_taken" } else { "bad_handle" };
            let message = match caller {
                Caller::App(_) if code == "handle_taken" => {
                    format!("handle {slug:?} is already used on this server")
                }
                _ => e,
            };
            return json_response(
                if code == "handle_taken" { "409 Conflict" } else { "400 Bad Request" },
                json!({ "error": code, "field": "handle", "message": message }),
            );
        }
    };
    record_audit(
        "agent.handle.set",
        caller,
        &ws,
        &format!("handle={old_handle}->{new_handle}"),
    );
    let mut out = view(caller, &ws.project_id);
    out["ok"] = json!(true);
    out["previousHandle"] = json!(old_handle);
    out["changed"] = json!(["handle"]);
    CliResponse::ok_json(out.to_string())
}

/// One audit line: who, which agent (uuid + handle at the time), old → new.
fn record_audit(event: &str, caller: &Caller, ws: &SkinWorkspace, detail: &str) {
    let line = format!("ok agent={} handle={} {detail}", ws.project_id, ws.handle);
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        event,
        &caller.actor(),
        line.clone(),
        "-",
        "-",
        caller.client(),
    ));
    k2_core::log_debug!("[agent-identity] {event} by={} {line}", caller.actor());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doors_are_exactly_two() {
        assert!(is_door("/cli/agent/identity"));
        assert!(is_door("/cli/agent/handle"));
        assert!(!is_door("/cli/agent/identity/x"));
        assert!(!is_door("/cli/agent/conf"));
    }

    #[test]
    fn body_cap_fits_the_largest_picture() {
        assert!(MAX_BODY > agent_look::MAX_INPUT_IMAGE_BYTES / 3 * 4);
    }

    #[test]
    fn rate_limit_counts_per_pass_and_room() {
        let pass = format!("pass-{}", uuid::Uuid::new_v4());
        for _ in 0..WRITES_PER_PASS_MAX {
            take_write_slot(&pass, "room-a").expect("under the cap");
        }
        let wait = take_write_slot(&pass, "room-a").expect_err("over the cap");
        assert!(wait >= 1 && wait <= WRITES_PER_PASS_WINDOW.as_secs(), "{wait}");
        take_write_slot(&pass, "room-b").expect("another room has its own budget");
    }

    #[test]
    fn retry_after_only_on_429() {
        assert_eq!(extra_headers(&rate_limited(42)), "Retry-After: 42\r\n");
        assert_eq!(extra_headers(&bad("usage", "x")), "");
    }

    #[test]
    fn passport_classes() {
        use crate::sidecar_routes::{AgentCaller, Caller as S};
        let agent = |sandboxed| {
            S::Agent(AgentCaller {
                project_id: "ws-1".into(),
                map_key: Some("k".into()),
                args: Vec::new(),
                sandboxed,
            })
        };
        let refused = |r: Result<Caller, CliResponse>| match r {
            Err(r) => r,
            Ok(c) => panic!("expected a refusal, got {c:?}"),
        };
        match classify_passport(agent(false), "sid-1") {
            Ok(Caller::Agent { project_id, session_id }) => {
                assert_eq!((project_id.as_str(), session_id.as_str()), ("ws-1", "sid-1"));
            }
            Ok(c) => panic!("a harness passport is an agent: {c:?}"),
            Err(r) => panic!("a harness passport is an agent: {}", r.body),
        }
        let r = refused(classify_passport(agent(true), "sid-1"));
        assert!(r.status.starts_with("403") && r.body.contains("\"sandboxed\""), "{}", r.body);
        for c in [S::WrongCredential, S::Invalid, S::Owner, S::Login { username: "x".into() }] {
            let r = refused(classify_passport(c, "sid-1"));
            assert!(r.status.starts_with("403") && r.body.contains("wrong_credential"), "{}", r.body);
        }
        match classify_passport(S::Shell { project_id: "no-such-ws".into() }, "sid-9") {
            Ok(Caller::Owner { actor, home }) => {
                assert_eq!(actor, "shell:no-such-ws/sid-9");
                assert_eq!(home.as_deref(), Some("no-such-ws"));
            }
            Ok(c) => panic!("a shell tab is a person: {c:?}"),
            Err(r) => panic!("a shell tab is a person: {}", r.body),
        }
    }

    #[test]
    fn agent_refusals_are_403_with_a_hint() {
        let r = agent_cannot_rename();
        assert!(r.status.starts_with("403"), "{}", r.status);
        assert!(r.body.contains("agent_cannot_rename") && r.body.contains("ask your owner"), "{}", r.body);
        let r = not_your_agent();
        assert!(r.status.starts_with("403") && r.body.contains("not_your_agent"), "{}", r.body);
    }

    #[test]
    fn opt_string_shapes() {
        let v = json!({"a": "x", "b": null, "c": 3});
        assert!(matches!(opt_string(&v, "a", false), Ok(Some(Some(ref s))) if s == "x"));
        assert!(matches!(opt_string(&v, "b", true), Ok(Some(None))));
        assert!(opt_string(&v, "b", false).is_err());
        assert!(opt_string(&v, "c", true).is_err());
        assert!(matches!(opt_string(&v, "zz", true), Ok(None)));
    }
}
