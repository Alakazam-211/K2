//! Room-scope surfaces for 0.46.1 (`prd-room-scope-surfaces-v1` §3.2,
//! Rosson's two-per-release rule): **R9 `room.members`** and **R16
//! `app.self` / `app.usage`** (+ the Garden's `widget.self`, G2). One
//! reveal = both surfaces: each is one catalog row with the App transport
//! (the gateway) and the Garden transport (`/cli/zen/call`, this
//! computer's daemon, as the owner), plus `k2 app` read verbs for agents.
//!
//! ```text
//! App transport (a k2skn_ pass; catalog rows, generated app_surface.rs)
//!   GET /cli/skin/members ?workspace=&app=   room.members  cap room:members, both doors
//!   GET /cli/skin/self    ?app=              app.self      no cap, the link, the App's own pass
//!   GET /cli/skin/usage   ?app=&hours=       app.usage     no cap, the link, the App's own pass
//! Garden transport (POST /cli/zen/call {verb, args}, owner rights)
//!   room.members({workspace, app?})  app.self({app})  app.usage({app, hours?})  widget.self()
//! Owner and agents (k2 app members|self|usage|room-names)
//!   GET  /cli/app/members    ?workspace=&app=   owner/admin: any room; an agent: its own room
//!   GET  /cli/app/self       ?app=              an agent: its own workspace's Apps
//!   GET  /cli/app/usage      ?app=&hours=
//!   POST /cli/app/room-names {workspace, hide}  the owner only
//! ```
//!
//! **Room members** come from grants (enabled only) and their login
//! policy, never presence (GA-Q25; `k2_core::skin::members`). An App caller
//! sees its own App's grants: a link pass its App's, a guest session its
//! login's App's (`host` for the box login), a platform pass the box
//! login's. Connect logins are the one line "admins of this box" (AR29).
//! When the owner hides names in a room (GA-Q40), every App caller gets
//! `username`/`displayName` null; the owner (Garden, CLI) sees them all.
//!
//! **No oracle.** On the App transport, naming another App (`app=`) answers
//! exactly what an App that doesn't exist answers: 404 `not_found`. A room
//! the pass doesn't hold is the uniform 403 `skin_room`.
//!
//! **Events.** `room_members_changed` (k2-core's members listener, every
//! grant/role/guest write and the hide switch) and `app_deployed` (the
//! supervisor, when a deployment goes live) ride the session bus; the App
//! activity socket maps them per room (`activity_events_ws`).

use std::collections::HashMap;

use k2_core::deploy::store::{self, AppRow};
use k2_core::skin::SkinPass;
use serde_json::{json, Value as J};

use super::routes::{agent_ws, db_err, json_body, need_credential, no, respond, s, ws_handle, Who, R};
use crate::cli_response::CliResponse;
use crate::garden_data::refused;
use k2_core::garden_data::DataError;

/// `room.members` on the App transport.
pub const MEMBERS_ROUTE: &str = "/cli/skin/members";
/// `app.self` on the App transport (the link only).
pub const SELF_ROUTE: &str = "/cli/skin/self";
/// `app.usage` on the App transport (the link only).
pub const USAGE_ROUTE: &str = "/cli/skin/usage";
/// The App-transport routes (a pass reads them; the dispatcher's skin arm).
pub const PASS_ROUTES: &[&str] = &[MEMBERS_ROUTE, SELF_ROUTE, USAGE_ROUTE];

/// The owner/agent read verbs and the owner's switch.
pub const ROUTES: &[&str] = &["/cli/app/members", "/cli/app/room-names", "/cli/app/self", "/cli/app/usage"];
/// POST-only among [`ROUTES`].
pub const POST_ROUTES: &[&str] = &["/cli/app/room-names"];

/// The `/boot-status` keys these surfaces bring.
pub const FEATURE_ROOM_MEMBERS: &str = "room-members-v1";
pub const FEATURE_APP_SELF: &str = "app-self-v1";

/// The `app.usage` window: default and largest, in hours (the audit keeps
/// 30 days of call rows).
pub const DEFAULT_HOURS: i64 = 24;
pub const MAX_HOURS: i64 = 720;

pub fn is_route(p: &str) -> bool {
    ROUTES.contains(&p)
}

pub fn is_post_route(p: &str) -> bool {
    POST_ROUTES.contains(&p)
}

// ── views (one shape for every transport) ────────────────────────────

/// The label an App grant's target shows as: `host`, the App id of the
/// published service, else the service's name (an App from before the
/// runtime), else the id itself.
fn app_label(target: &str, cache: &mut HashMap<String, String>) -> String {
    if target == k2_core::skin::HOST_APP_ID {
        return target.to_string();
    }
    if let Some(l) = cache.get(target) {
        return l.clone();
    }
    let label = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match store::get_app_by_service(&conn, target).ok().flatten() {
            Some(a) => a.app_id,
            None => k2_core::published_services::get_by_id(&conn, target)
                .ok()
                .flatten()
                .map(|p| p.name)
                .unwrap_or_else(|| target.to_string()),
        }
    };
    cache.insert(target.to_string(), label.clone());
    label
}

/// The grant target (`host` or a published service id) an App id names:
/// `host`, or the App row's service. `None` = no such App.
fn target_for_app_id(app: &str) -> Option<String> {
    if app == k2_core::skin::HOST_APP_ID {
        return Some(app.to_string());
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::get_app(&conn, &app.to_ascii_lowercase()).ok().flatten().map(|a| a.service_id)
}

/// `room.members` for one room. `target` = only that App's grants (`None`
/// = every App, the owner's view); `hide` = null the names (App callers in
/// a room whose names the owner hides).
pub fn members_view(project_id: &str, handle: &str, target: Option<&str>, hide: bool) -> Result<J, String> {
    let hidden = k2_core::skin::members::names_hidden(project_id)?;
    let rows = k2_core::skin::members::room_members(project_id, target)?;
    let mut cache = HashMap::new();
    // agent-app-users (security review item 11): the owner's and the
    // agent's views say who an agent set up and whose extra access waits;
    // an App's own view never does.
    let tied = if hide { HashMap::new() } else { crate::skin_routes::tie_display()? };
    let members: Vec<J> = rows
        .iter()
        .map(|m| {
            let (username, display) = if hide && hidden {
                (J::Null, J::Null)
            } else {
                (J::String(m.username.clone()), J::String(m.display_name.clone()))
            };
            let mut row = json!({
                "username": username,
                "displayName": display,
                "kind": m.kind,
                "role": m.role,
                "app": app_label(&m.app, &mut cache),
                "caps": m.caps,
            });
            if !hide {
                let t = tied.get(&m.principal_id);
                row["pendingReset"] = json!(t.is_some_and(|t| t.pending));
                row["tiedTo"] = json!(t.map(|t| t.handle.clone()));
            }
            row
        })
        .collect();
    Ok(json!({
        "workspace": handle,
        "room": project_id,
        "namesHidden": hidden,
        "admins": k2_core::skin::members::ADMINS_LINE,
        "count": members.len(),
        "members": members,
    }))
}

/// An App's health, in the words of `app.self`.
fn health(app: &AppRow) -> &'static str {
    if super::suspend::is_suspended(&app.app_id) {
        return "suspended";
    }
    match super::supervisor::service_status(&app.service_id) {
        Some("running") => "running",
        Some("crashlooping") => "crashlooping",
        Some("exited") => "exited",
        Some("starting") => "starting",
        _ => "stopped",
    }
}

/// `app.self` for one App.
pub fn self_view(app: &AppRow) -> J {
    let deployed_at = app.live_deployment.as_deref().and_then(|d| {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::get_deployment(&conn, d).ok().flatten().and_then(|r| r.live_at)
    });
    json!({
        "app": app.app_id,
        "workspace": ws_handle(&app.owner_workspace),
        "liveSeed": app.live_seed,
        "health": health(app),
        "state": app.state,
        "deployment": app.live_deployment,
        "deployedAt": deployed_at,
    })
}

/// `app.usage` for one App over the last `hours`.
pub fn usage_view(app: &AppRow, hours: i64) -> Result<J, String> {
    let since = chrono::Utc::now().timestamp_millis() - hours * 3_600_000;
    super::audit::flush();
    let audit = k2_core::app_audit::db_path();
    let conn = k2_core::app_audit::open(&audit).map_err(|e| format!("app-audit.db: {e}"))?;
    let (calls, denied, bytes_in, bytes_out) =
        k2_core::app_audit::totals(&conn, &app.app_id, since).map_err(|e| format!("app-audit.db: {e}"))?;
    let verbs: Vec<J> = k2_core::app_audit::verb_counts(&conn, &app.app_id, since)
        .map_err(|e| format!("app-audit.db: {e}"))?
        .into_iter()
        .map(|v| {
            let (method, path) = v.verb.split_once(' ').unwrap_or(("", v.verb.as_str()));
            let verb = k2_core::app_surface::verb_for(method, path).or_else(|| k2_core::app_surface::verb_for("WS", path));
            json!({ "verb": verb, "route": v.verb, "calls": v.calls, "denied": v.denied })
        })
        .collect();
    Ok(json!({
        "app": app.app_id,
        "hours": hours,
        "quotas": super::quotas::quotas_for(&app.app_id),
        "live": {
            "connections": super::gateway::connections(&app.app_id),
            "subscriptions": super::quotas::live_subscriptions(&app.app_id),
        },
        "totals": { "calls": calls, "denied": denied, "bytesIn": bytes_in, "bytesOut": bytes_out },
        "verbs": verbs,
    }))
}

/// `hours` from a query string or args: absent = the default; else 1..=720.
fn parse_hours(raw: Option<&str>) -> Result<i64, String> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(DEFAULT_HOURS),
        Some(t) => match t.parse::<i64>() {
            Ok(h) if (1..=MAX_HOURS).contains(&h) => Ok(h),
            _ => Err(format!("hours is a whole number from 1 to {MAX_HOURS}, not '{t}'")),
        },
    }
}

fn get_app(app_id: &str) -> Option<AppRow> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::get_app(&conn, app_id).ok().flatten()
}

// ── the App transport ────────────────────────────────────────────────

fn refusal(status: &'static str, code: &str, hint: &str) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: json!({ "ok": false, "error": { "code": code, "hint": hint } }).to_string(),
    }
}

/// The one answer for "no such App" and "another App's": the same bytes,
/// so an App can't tell which (no oracle).
pub fn not_found() -> CliResponse {
    refusal("404 Not Found", "not_found", "no such App")
}

fn usage(hint: &str) -> CliResponse {
    refusal("400 Bad Request", "usage", hint)
}

/// The caller's App on the App transport: `(app id, grant target)`. A link
/// pass is the link's App; a guest session is its login's App (`host` for
/// the box login); a platform pass is the box login's.
fn caller_app(pass: &SkinPass, link: Option<&super::gateway::LinkCtx>) -> (String, String) {
    if let Some(l) = link {
        if !pass.session {
            return (l.app_id.clone(), l.service_id.clone());
        }
    }
    if pass.session && !pass.app.trim().is_empty() {
        let target = pass.app.clone();
        let mut cache = HashMap::new();
        return (app_label(&target, &mut cache), target);
    }
    let host = k2_core::skin::HOST_APP_ID.to_string();
    (host.clone(), host)
}

/// A non-pass credential on a pass route.
pub fn forbidden(path: &str) -> CliResponse {
    refusal("403 Forbidden", "skin_token", &format!("GET {path} is for a live app or guest pass (k2skn_)"))
}

/// `GET /cli/skin/members|self|usage` for a resolved pass (blocking).
///
/// `link` is the gateway link the request came in on, read by the caller
/// on the request's task (the link context is task-local, so it never
/// reaches a blocking worker on its own).
pub fn handle_pass(path: &str, pass: &SkinPass, params: &HashMap<String, String>, link: Option<&super::gateway::LinkCtx>) -> CliResponse {
    let param = |k: &str| params.get(k).map(|v| v.trim()).filter(|v| !v.is_empty());
    match path {
        MEMBERS_ROUTE => {
            let Some(token) = param("workspace") else { return usage("name the room: workspace=<handle>") };
            let room = match crate::fs_routes::resolve_skin_workspace(pass, token) {
                Ok(r) => r,
                Err(r) => return r,
            };
            if !pass.has_cap_in_room(&room.project_id, k2_core::skin::CAP_ROOM_MEMBERS) {
                return CliResponse {
                    status: "403 Forbidden",
                    content_type: "application/json",
                    body: json!({
                        "ok": false,
                        "error": { "code": "cap_not_granted", "cap": k2_core::skin::CAP_ROOM_MEMBERS, "room": room.handle, "hint": "this pass has no room:members in that room" },
                    })
                    .to_string(),
                };
            }
            let (own_id, target) = caller_app(pass, link);
            if param("app").is_some_and(|a| !a.eq_ignore_ascii_case(&own_id)) {
                return not_found();
            }
            let handle = if room.handle.is_empty() { token.to_string() } else { room.handle.clone() };
            match members_view(&room.project_id, &handle, Some(&target), true) {
                Ok(v) => CliResponse::ok_json(v.to_string()),
                Err(e) => CliResponse::internal_error(e),
            }
        }
        SELF_ROUTE | USAGE_ROUTE => {
            // The App's own pass on its own link; a guest's session (even
            // one of this App) never reads the App's own data.
            let Some(link) = link.filter(|_| !pass.session) else {
                return refusal(
                    "403 Forbidden",
                    "not_exposed",
                    "app.self and app.usage answer the App's own pass on its gateway link (asApp), never a guest",
                );
            };
            if param("app").is_some_and(|a| !a.eq_ignore_ascii_case(&link.app_id)) {
                return not_found();
            }
            let Some(app) = get_app(&link.app_id) else { return not_found() };
            if path == SELF_ROUTE {
                return CliResponse::ok_json(self_view(&app).to_string());
            }
            let hours = match parse_hours(param("hours")) {
                Ok(h) => h,
                Err(e) => return usage(&e),
            };
            match usage_view(&app, hours) {
                Ok(v) => CliResponse::ok_json(v.to_string()),
                Err(e) => CliResponse::internal_error(e),
            }
        }
        _ => CliResponse::not_found(),
    }
}

// ── the Garden transport (POST /cli/zen/call, the owner's rights) ─────

/// The one object argument of a both-transport row (`[]` or `[{…}]`), with
/// only the named keys.
fn garden_args<'a>(verb: &str, args: &'a [J], allowed: &[&str]) -> Result<serde_json::Map<String, J>, DataError> {
    let o = match args {
        [] | [J::Null] => serde_json::Map::new(),
        [J::Object(o)] => o.clone(),
        _ => return Err(DataError::BadRequest(format!("{verb} takes one object argument"))),
    };
    if let Some(k) = o.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(DataError::BadRequest(format!("{verb}: unknown argument '{k}'")));
    }
    for (k, v) in &o {
        if k != "hours" && !v.is_string() {
            return Err(DataError::BadRequest(format!("{verb}: {k} is text")));
        }
    }
    Ok(o)
}

fn arg<'a>(o: &'a serde_json::Map<String, J>, k: &str) -> Option<&'a str> {
    o.get(k).and_then(J::as_str).map(str::trim).filter(|s| !s.is_empty())
}

fn garden_app(verb: &str, o: &serde_json::Map<String, J>) -> Result<AppRow, DataError> {
    let id = arg(o, "app").ok_or_else(|| DataError::BadRequest(format!("{verb} needs {{app}}: the App id")))?;
    get_app(&id.to_ascii_lowercase()).ok_or_else(|| refused("not_found", 404, format!("no App '{id}' on this computer")))
}

/// `room.members`, `app.self`, `app.usage` for a Garden widget. A Garden
/// acts as the owner (Rosson 2026-10-11): every name, any local App.
pub fn garden_call(verb: &str, args: &[J]) -> Result<J, DataError> {
    match verb {
        "room.members" => {
            let o = garden_args(verb, args, &["workspace", "app"])?;
            let token = arg(&o, "workspace").ok_or_else(|| DataError::BadRequest("room.members needs {workspace}".into()))?;
            let room = crate::fs_routes::resolve_owner_workspace(token)
                .map_err(|_| refused("not_bound", 403, format!("'{token}' isn't an agent on this computer")))?;
            let target = match arg(&o, "app") {
                Some(a) => Some(target_for_app_id(a).ok_or_else(|| refused("not_found", 404, format!("no App '{a}' on this computer")))?),
                None => None,
            };
            members_view(&room.project_id, &room.handle, target.as_deref(), false).map_err(DataError::Unavailable)
        }
        "app.self" => {
            let o = garden_args(verb, args, &["app"])?;
            Ok(self_view(&garden_app(verb, &o)?))
        }
        "app.usage" => {
            let o = garden_args(verb, args, &["app", "hours"])?;
            let app = garden_app(verb, &o)?;
            let hours = match o.get("hours") {
                None | Some(J::Null) => DEFAULT_HOURS,
                Some(h) => h
                    .as_i64()
                    .filter(|h| (1..=MAX_HOURS).contains(h))
                    .ok_or_else(|| DataError::BadRequest(format!("app.usage: hours is a whole number from 1 to {MAX_HOURS}")))?,
            };
            usage_view(&app, hours).map_err(DataError::Unavailable)
        }
        _ => Err(refused("unknown_verb", 400, format!("no verb {verb}"))),
    }
}

// ── owner and agents (k2 app members|self|usage|room-names) ──────────

/// Route an owner/agent request (blocking).
pub fn handle(path: &str, who: Who, body: &[u8], params: &HashMap<String, String>) -> CliResponse {
    let p = |k: &str| params.get(k).map(String::as_str);
    let r = match path {
        "/cli/app/members" => members_route(&who, p("workspace"), p("app")),
        "/cli/app/room-names" => room_names_route(&who, body),
        "/cli/app/self" => need_credential(&who).and_then(|_| super::routes::app_for(&who, p("app"))).map(|a| {
            let mut v = self_view(&a);
            v["ok"] = J::Bool(true);
            v
        }),
        "/cli/app/usage" => need_credential(&who).and_then(|_| {
            let a = super::routes::app_for(&who, p("app"))?;
            let hours = parse_hours(p("hours")).map_err(|e| no(k2_core::deploy::codes::BAD_REQUEST, 400, e))?;
            let mut v = usage_view(&a, hours).map_err(|e| no("audit_unavailable", 503, e))?;
            v["ok"] = J::Bool(true);
            Ok(v)
        }),
        _ => Err(no("not_found", 404, "no such route")),
    };
    respond(r)
}

/// Owner/admin: any room (and any App, or every App). An agent: its own
/// workspace's room only. A Connect member login is refused.
fn members_route(who: &Who, workspace: Option<&str>, app: Option<&str>) -> R<J> {
    need_credential(who)?;
    if let Who::Login { .. } = who {
        if !who.is_admin() {
            return Err(no(k2_core::deploy::codes::FORBIDDEN, 403, "room members are for the owner, admins and the room's own agent"));
        }
    }
    let ws = super::routes::target_ws(who, workspace)?;
    let handle = ws_handle(&ws.id);
    let target = match app.map(str::trim).filter(|a| !a.is_empty() && *a != "all") {
        Some(a) => Some(target_for_app_id(a).ok_or_else(|| no(k2_core::deploy::codes::APP_NOT_FOUND, 404, format!("no App '{a}' on this box (k2 app list; host = the box login)")))?),
        None => None,
    };
    let mut v = members_view(&ws.id, &handle, target.as_deref(), false).map_err(db_err)?;
    v["ok"] = J::Bool(true);
    Ok(v)
}

/// POST `/cli/app/room-names {workspace, hide}`: the owner hides (or shows)
/// guests' names in one room's `room.members`. Owner only.
fn room_names_route(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    if agent_ws(who).is_some() || !who.is_owner() {
        return Err(no(k2_core::deploy::codes::OWNER_ONLY, 403, "only the owner hides or shows guests' names in a room"));
    }
    let v = json_body(body)?;
    let ws = super::routes::target_ws(who, s(&v, "workspace"))?;
    let hide = match v.get("hide") {
        Some(J::Bool(b)) => *b,
        _ => return Err(no(k2_core::deploy::codes::BAD_REQUEST, 400, "hide is true or false")),
    };
    let changed = k2_core::skin::members::set_names_hidden(&ws.id, hide).map_err(db_err)?;
    Ok(json!({ "ok": true, "workspace": ws_handle(&ws.id), "room": ws.id, "namesHidden": hide, "changed": changed }))
}

// ── events ───────────────────────────────────────────────────────────

fn emit_members_changed(room: Option<&str>) {
    let _ = crate::session_events::emit(crate::session_events::SessionEvent::RoomMembersChanged { room: room.map(str::to_string) });
}

/// Register k2-core's members listener (idempotent; the daemon's boot and
/// the activity socket both call it, so tests get events too).
pub fn install_listener() {
    k2_core::skin::members::set_listener(emit_members_changed);
}

/// An App's live deployment changed (the supervisor, after the row says so).
/// `kind`: what went live (`store::KIND_*`: `rehost` vs `deploy`, …; R16).
pub fn emit_deployed(app_id: &str, kind: &str) {
    let Some(app) = get_app(app_id) else { return };
    let _ = crate::session_events::emit(deployed_event(app, kind));
}

/// The `app_deployed` session event for `app` (tests read it).
pub fn deployed_event(app: AppRow, kind: &str) -> crate::session_events::SessionEvent {
    crate::session_events::SessionEvent::AppDeployed {
        app_id: app.app_id,
        service_id: app.service_id,
        project_id: app.owner_workspace,
        seed: app.live_seed,
        deployment: app.live_deployment,
        deploy_kind: kind.to_string(),
    }
}

/// Does an `app_deployed` for (`app_id`, `service_id`) reach a socket with
/// `pass`? The owner (no pass) always; the App's own link pass; that App's
/// guests. Never another App's pass or guest.
pub fn deployed_reaches(pass: Option<&SkinPass>, link_app: Option<&str>, app_id: &str, service_id: &str) -> bool {
    match pass {
        None => true,
        Some(p) if p.session => p.app == service_id,
        Some(_) => link_app == Some(app_id),
    }
}
