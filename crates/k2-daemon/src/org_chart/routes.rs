//! `/cli/organization/*` (prd-org-chart-v1 §6.2): one handler for Loopback
//! TCP and the cell socket. The JSON shapes are `k2_core::org_chart::wire`.
//!
//! | Route | Method | Who |
//! |---|---|---|
//! | `list`, `show`, `chart`, `bulletin` (`peek=1` doesn't mark), `bulletin/status` | GET | anyone on the server |
//! | `mine` | GET | the calling agent |
//! | `updates` | GET | that org's head; Owner/Admin people |
//! | `bulletin/read` | POST | the calling agent (its own marker, from its passport) |
//! | `bulletin/set` | POST | that org's head (no switch); Owner/Admin people (audited) |
//! | `ask` | POST | a head, to its own members only |
//! | `update` | POST | a member, to its own head only |
//! | `reach` | POST | the calling agent |
//! | `create`, `rename`, `purpose`, `delete`, `member/*`, `head`, `line/*` | POST | Owner/Admin people; an agent with "Manage the org chart" |
//! | `layout/move`, `layout/tidy` | POST | Owner/Admin people only (the canvas is a person's) |
//!
//! Every route answers 409 `org_chart_off` while the Org Chart is off.
//! Every write records who did it, adds an `org_chart.*` audit line, bumps
//! the revision and emits `org_chart_changed`.

use std::collections::HashMap;

use k2_core::org_chart::store::{self, Anchor, LineKind, NodeKind, NodeRef, Placed};
use k2_core::org_chart::{self as oc, day, text, wire, OrgError};
use rusqlite::Connection;
use serde_json::{json, Map, Value};

use super::caller::{refusal, OrgCaller};
use crate::cli_response::CliResponse;

pub const PREFIX: &str = "/cli/organization/";

pub const GET_ROUTES: &[&str] = &[
    "/cli/organization/list",
    "/cli/organization/show",
    "/cli/organization/mine",
    "/cli/organization/chart",
    "/cli/organization/bulletin",
    "/cli/organization/bulletin/status",
    "/cli/organization/updates",
];

pub const POST_ROUTES: &[&str] = &[
    "/cli/organization/bulletin/read",
    "/cli/organization/bulletin/set",
    "/cli/organization/ask",
    "/cli/organization/update",
    "/cli/organization/reach",
    "/cli/organization/create",
    "/cli/organization/rename",
    "/cli/organization/purpose",
    "/cli/organization/delete",
    "/cli/organization/member/add",
    "/cli/organization/member/move",
    "/cli/organization/member/remove",
    "/cli/organization/head",
    "/cli/organization/line/add",
    "/cli/organization/line/remove",
    "/cli/organization/layout/move",
    "/cli/organization/layout/tidy",
];

/// The routes that need "Manage the org chart" for an agent.
const MANAGE_ROUTES: &[&str] = &[
    "/cli/organization/create",
    "/cli/organization/rename",
    "/cli/organization/purpose",
    "/cli/organization/delete",
    "/cli/organization/member/add",
    "/cli/organization/member/move",
    "/cli/organization/member/remove",
    "/cli/organization/head",
    "/cli/organization/line/add",
    "/cli/organization/line/remove",
];

/// Request bodies are small (5 lines of 160, a 400-char purpose).
pub const MAX_BODY: usize = 32 * 1024;

pub fn is_route(path: &str) -> bool {
    GET_ROUTES.contains(&path) || POST_ROUTES.contains(&path)
}

pub fn is_post_route(path: &str) -> bool {
    POST_ROUTES.contains(&path)
}

// ── responses ──────────────────────────────────────────────────────

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

pub(crate) fn fail(e: &OrgError) -> CliResponse {
    refusal(status_line(e.status), e.code, &e.hint)
}

fn ok(v: Value) -> CliResponse {
    CliResponse::ok_json(v.to_string())
}

fn usage(hint: &str) -> CliResponse {
    refusal("400 Bad Request", "usage", hint)
}

pub fn too_large() -> CliResponse {
    refusal("413 Payload Too Large", "body_too_large", "request bodies here are at most 32 KiB")
}

/// The answer for an agent without "Manage the org chart" (a wall row
/// turns it into the `Ask with: k2 permission request organization.manage`
/// line while permission cards are on).
pub const MANAGE_OFF_CODE: &str = "org_manage_off";

fn manage_off(ws: &str) -> CliResponse {
    refusal(
        "403 Forbidden",
        MANAGE_OFF_CODE,
        &format!(
            "Managing the Org Chart needs the Owner to turn on 'Manage the org chart' for {ws} (Settings → Workspaces → {ws} → Agent). Reading, your own bulletin as a head, update, ask and reach need no switch."
        ),
    )
}

// ── the request ────────────────────────────────────────────────────

/// The JSON body as a map; query/form params fill keys it lacks.
fn body_map(body: &[u8], params: &HashMap<String, String>) -> Map<String, Value> {
    let mut m: Map<String, Value> = serde_json::from_slice::<Value>(body).ok().and_then(|v| v.as_object().cloned()).unwrap_or_default();
    for (k, v) in params {
        if k == "token" || k == "project" || k.starts_with("principal") || k == crate::caller_workspace::PRINCIPAL_BOUND_KEY {
            continue;
        }
        m.entry(k.clone()).or_insert_with(|| Value::String(v.clone()));
    }
    m
}

fn s<'a>(m: &'a Map<String, Value>, k: &str) -> Option<&'a str> {
    m.get(k).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty())
}

fn truthy(m: &Map<String, Value>, k: &str) -> bool {
    match m.get(k) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(v)) => matches!(v.as_str(), "1" | "true" | "yes" | "on"),
        Some(Value::Number(n)) => n.as_i64() == Some(1),
        _ => false,
    }
}

fn need<'a>(m: &'a Map<String, Value>, k: &str, what: &str) -> Result<&'a str, CliResponse> {
    s(m, k).ok_or_else(|| usage(&format!("name {what}: {k}=…")))
}

/// An agent on this server by handle, name, path or id.
pub(crate) fn resolve_agent(token: &str) -> Result<String, CliResponse> {
    let t = token.trim().strip_prefix("agent:").unwrap_or(token.trim());
    let not_found = || {
        fail(&OrgError::not_found("agent", t, "k2 organization list (or k2 workspace list) shows the agents on this server."))
    };
    if t.is_empty() {
        return Err(usage("name the agent"));
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    if store::agent_exists(&conn, t).unwrap_or(false) {
        return Ok(t.to_string());
    }
    drop(conn);
    let path = crate::workspace_msg::resolve_workspace(t).ok_or_else(not_found)?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT id FROM projects WHERE path = ?1", rusqlite::params![path], |r| r.get::<_, String>(0))
        .map_err(|_| not_found())
}

/// A line end: `org:<name>` / `agent:<name>`, else an org name or id, else
/// an agent. A name that is both an org and an agent must say which.
fn resolve_end(token: &str) -> Result<NodeRef, CliResponse> {
    let t = token.trim();
    if let Some(o) = t.strip_prefix("org:") {
        let db = k2_core::db::shared();
        let conn = db.lock();
        return store::find_org(&conn, o).map(|o| NodeRef::org(&o.id)).map_err(|e| fail(&e));
    }
    if t.starts_with("agent:") {
        return resolve_agent(t).map(|id| NodeRef::agent(&id));
    }
    let org = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::lookup_org(&conn, t).map_err(|e| fail(&e))?
    };
    let agent = resolve_agent(t).ok();
    // An org "Billing" and its head "billing": the exact spelling decides.
    if let (Some(o), Some(a)) = (&org, &agent) {
        let org_exact = o.name == t;
        let agent_exact = super::caller_handle(a) == t;
        if org_exact != agent_exact {
            return Ok(if org_exact { NodeRef::org(&o.id) } else { NodeRef::agent(a) });
        }
    }
    match (org, agent) {
        (Some(o), Some(_)) => Err(refusal(
            "400 Bad Request",
            "ambiguous_name",
            &format!("'{t}' is both an org and an agent. Say which: org:{} or agent:{t}.", o.name),
        )),
        (Some(o), None) => Ok(NodeRef::org(&o.id)),
        (None, Some(a)) => Ok(NodeRef::agent(&a)),
        (None, None) => Err(fail(&OrgError::not_found("org or agent", t, "k2 organization list shows the orgs and their agents."))),
    }
}

fn find_org(token: &str) -> Result<store::Org, CliResponse> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::find_org(&conn, token).map_err(|e| fail(&e))
}

/// The caller's own org (members: the org it's in; `--org` picks one when
/// the cap ever allows several).
fn own_org(project_id: &str, named: Option<&str>) -> Result<Option<store::Org>, CliResponse> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mine = store::orgs_of(&conn, project_id).map_err(|e| fail(&e))?;
    Ok(match named {
        Some(n) => mine.into_iter().find(|o| o.id == n || o.name.eq_ignore_ascii_case(n)),
        None => mine.into_iter().next(),
    })
}

fn audit(caller: &OrgCaller, event: &str, detail: String) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        &format!("org_chart.{event}"),
        caller.label(),
        detail,
        "-",
        "-",
        "cli",
    ));
}

/// After a write: audit, redraw everywhere, answer with the revision.
fn wrote(caller: &OrgCaller, event: &str, detail: String, mut v: Value) -> CliResponse {
    audit(caller, event, detail);
    let rev = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::revision(&conn).unwrap_or(0)
    };
    super::changed(rev);
    v["ok"] = json!(true);
    v["revision"] = json!(rev);
    ok(v)
}

fn org_json(conn: &Connection, org: &store::Org) -> Value {
    wire::org_wire(conn, org).map(|w| serde_json::to_value(w).unwrap_or(Value::Null)).unwrap_or(Value::Null)
}

fn org_json_by_id(id: &str) -> Value {
    let db = k2_core::db::shared();
    let conn = db.lock();
    match store::get_org(&conn, id) {
        Ok(Some(o)) => org_json(&conn, &o),
        _ => Value::Null,
    }
}

// ── the door ───────────────────────────────────────────────────────

pub fn handle(method: &str, path: &str, params: &HashMap<String, String>, body: &[u8], caller: &OrgCaller) -> CliResponse {
    let is_post = method.eq_ignore_ascii_case("POST");
    if !is_route(path) {
        return CliResponse::not_found();
    }
    if is_post_route(path) != is_post {
        return refusal("405 Method Not Allowed", "method_not_allowed", if is_post { "use GET" } else { "POST required" });
    }
    if body.len() > MAX_BODY {
        return too_large();
    }
    if !oc::enabled() {
        return fail(&OrgError::off());
    }
    let m = body_map(body, params);
    if MANAGE_ROUTES.contains(&path) {
        if let OrgCaller::Agent { project_id, .. } = caller {
            let on = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                store::agents_can_manage(&conn, project_id)
            };
            if !on {
                return manage_off(&super::caller_handle(project_id));
            }
        } else if !caller.is_admin() {
            return refusal("403 Forbidden", "role_required", "Changing the Org Chart needs an Owner or Admin login.");
        }
    }
    let now = oc::now();
    let r = match path {
        "/cli/organization/list" => list(),
        "/cli/organization/show" => show(&m),
        "/cli/organization/mine" => mine(caller),
        "/cli/organization/chart" => chart(caller),
        "/cli/organization/bulletin" => bulletin(caller, &m, now),
        "/cli/organization/bulletin/status" => bulletin_status(caller, now),
        "/cli/organization/bulletin/read" => bulletin_read(caller, now),
        "/cli/organization/bulletin/set" => bulletin_set(caller, &m, now.timestamp()),
        "/cli/organization/updates" => updates(caller, &m, now),
        "/cli/organization/ask" => ask(caller, &m, now),
        "/cli/organization/update" => update(caller, &m, now),
        "/cli/organization/reach" => match caller {
            OrgCaller::Agent { project_id, session_id, .. } => super::reach::reach(project_id, session_id, &m, now.timestamp()),
            OrgCaller::Person { .. } => Err(refusal(
                "403 Forbidden",
                "agent_only",
                "reach is an agent's ask to be connected to a head; a person connects agents in Settings → Connections (or k2 connections add).",
            )),
        },
        "/cli/organization/create" => create(caller, &m, now.timestamp()),
        "/cli/organization/rename" => rename(caller, &m, now.timestamp()),
        "/cli/organization/purpose" => purpose(caller, &m, now.timestamp()),
        "/cli/organization/delete" => delete(caller, &m, now.timestamp()),
        "/cli/organization/member/add" => member_add(caller, &m, now.timestamp()),
        "/cli/organization/member/move" => member_move(caller, &m, now.timestamp()),
        "/cli/organization/member/remove" => member_remove(caller, &m, now.timestamp()),
        "/cli/organization/head" => head(caller, &m, now.timestamp()),
        "/cli/organization/line/add" => line_add(caller, &m, now.timestamp()),
        "/cli/organization/line/remove" => line_remove(caller, &m),
        "/cli/organization/layout/move" | "/cli/organization/layout/tidy" => layout(path, caller, &m, now.timestamp()),
        _ => Err(CliResponse::not_found()),
    };
    r.unwrap_or_else(|e| e)
}

type R = Result<CliResponse, CliResponse>;

macro_rules! db {
    ($conn:ident, $body:expr) => {{
        let __db = k2_core::db::shared();
        let $conn = __db.lock();
        $body
    }};
}

fn or_fail<T>(r: k2_core::org_chart::Result<T>) -> Result<T, CliResponse> {
    r.map_err(|e| fail(&e))
}

// ── reads ──────────────────────────────────────────────────────────

fn list() -> R {
    let orgs = or_fail(db!(conn, wire::org_list(&conn)))?;
    Ok(ok(json!({ "ok": true, "orgs": orgs })))
}

fn show(m: &Map<String, Value>) -> R {
    let token = need(m, "org", "the org")?;
    db!(conn, {
        let org = or_fail(store::find_org(&conn, token))?;
        let mut v = org_json(&conn, &org);
        let lines: Vec<Value> = or_fail(store::lines(&conn))?
            .iter()
            .filter(|l| {
                let touches = |n: &NodeRef| match n.kind {
                    NodeKind::Org => n.id == org.id,
                    NodeKind::Agent => store::is_member(&conn, &org.id, &n.id).unwrap_or(false),
                };
                touches(&l.a) || touches(&l.b)
            })
            .map(|l| serde_json::to_value(wire::line_wire(&conn, l)).unwrap_or(Value::Null))
            .collect();
        v["lines"] = json!(lines);
        Ok(ok(json!({ "ok": true, "org": v })))
    })
}

fn mine(caller: &OrgCaller) -> R {
    let OrgCaller::Agent { project_id, .. } = caller else {
        return Err(refusal("403 Forbidden", "agent_only", "mine is an agent's own org; a person reads k2 organization list."));
    };
    db!(conn, {
        let orgs = or_fail(store::orgs_of(&conn, project_id))?;
        let Some(org) = orgs.first() else {
            return Ok(ok(json!({
                "ok": true, "org": null, "isHead": false, "head": null, "connectedToHead": false,
                "next": "You're in no org. k2 organization bulletin shows every org and its head."
            })));
        };
        let head = org.head.clone();
        drop(conn);
        let connected = head.as_deref().is_some_and(|h| h == project_id || k2_core::connections::are_local_peers(project_id, h));
        let head_ref = head.as_deref().map(|h| json!({ "id": h, "handle": super::caller_handle(h) }));
        let is_head = head.as_deref() == Some(project_id.as_str());
        Ok(ok(json!({
            "ok": true,
            "org": org_json_by_id(&org.id),
            "isHead": is_head,
            "head": head_ref,
            "connectedToHead": connected,
        })))
    })
}

fn chart(caller: &OrgCaller) -> R {
    let can_manage = match caller {
        OrgCaller::Person { admin, .. } => *admin,
        OrgCaller::Agent { project_id, .. } => db!(conn, store::agents_can_manage(&conn, project_id)),
    };
    let c = or_fail(db!(conn, wire::chart(&conn, can_manage, caller.is_admin())))?;
    Ok(CliResponse::ok_json(serde_json::to_string(&c).unwrap_or_default()))
}

/// Today's day key for the read marker.
pub(crate) fn today(now: chrono::DateTime<chrono::Utc>) -> String {
    let cfg = oc::config();
    day::day_key(day::read_day(cfg.zone(), &cfg.night, now))
}

fn bulletin(caller: &OrgCaller, m: &Map<String, Value>, now: chrono::DateTime<chrono::Utc>) -> R {
    let cfg = oc::config();
    let (day_key, text, orgs) = or_fail(db!(conn, wire::server_bulletin(&conn, &cfg, now)))?;
    let mark = !truthy(m, "peek");
    let mut marked = false;
    if let (true, OrgCaller::Agent { project_id, agent_address, .. }) = (mark, caller) {
        or_fail(db!(conn, store::mark_read(&conn, project_id, agent_address, &day_key, now.timestamp())))?;
        marked = true;
    }
    let out = wire::ServerBulletin { ok: true, day: day_key, zone: day::zone_name(cfg.zone()), text, orgs, marked_read: marked };
    Ok(CliResponse::ok_json(serde_json::to_string(&out).unwrap_or_default()))
}

fn bulletin_status(caller: &OrgCaller, now: chrono::DateTime<chrono::Utc>) -> R {
    let day_key = today(now);
    let unread = match caller {
        OrgCaller::Agent { project_id, agent_address, .. } => {
            !or_fail(db!(conn, store::has_read(&conn, project_id, agent_address, &day_key)))?
        }
        OrgCaller::Person { .. } => false,
    };
    Ok(ok(json!({ "ok": true, "day": day_key, "unread": unread })))
}

fn bulletin_read(caller: &OrgCaller, now: chrono::DateTime<chrono::Utc>) -> R {
    let OrgCaller::Agent { project_id, agent_address, .. } = caller else {
        return Err(refusal("403 Forbidden", "agent_only", "The read marker is an agent's; people just read k2 organization bulletin."));
    };
    let day_key = today(now);
    or_fail(db!(conn, store::mark_read(&conn, project_id, agent_address, &day_key, now.timestamp())))?;
    Ok(ok(json!({ "ok": true, "day": day_key, "read": true })))
}

fn bulletin_set(caller: &OrgCaller, m: &Map<String, Value>, now: i64) -> R {
    let parts: Vec<String> = match m.get("lines") {
        Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
        Some(Value::String(t)) => vec![t.clone()],
        _ => s(m, "text").map(|t| vec![t.to_string()]).unwrap_or_default(),
    };
    let org = match (s(m, "org"), caller) {
        (Some(t), _) => find_org(t)?,
        (None, OrgCaller::Agent { project_id, .. }) => {
            let headed = or_fail(db!(conn, store::headed_by(&conn, project_id)))?;
            headed.into_iter().next().ok_or_else(|| {
                refusal(
                    "403 Forbidden",
                    "not_org_head",
                    "You aren't an org's head, so you have no bulletin to write. k2 organization mine shows your org and its head.",
                )
            })?
        }
        (None, OrgCaller::Person { .. }) => return Err(usage("name the org: --org <org>")),
    };
    let b = or_fail(db!(conn, store::set_bulletin(&conn, &org.id, &caller.actor(), &parts, now)))?;
    let n = b.lines.len();
    Ok(wrote(
        caller,
        "bulletin",
        format!("{} {n} line{}", org.name, if n == 1 { "" } else { "s" }),
        json!({ "org": { "id": org.id, "name": org.name }, "lines": b.lines }),
    ))
}

fn updates(caller: &OrgCaller, m: &Map<String, Value>, now: chrono::DateTime<chrono::Utc>) -> R {
    let org = match (s(m, "org"), caller) {
        (Some(t), _) => find_org(t)?,
        (None, OrgCaller::Agent { project_id, .. }) => or_fail(db!(conn, store::headed_by(&conn, project_id)))?
            .into_iter()
            .next()
            .ok_or_else(|| refusal("403 Forbidden", "not_org_head", "updates are for an org's head; you aren't one (k2 organization mine)."))?,
        (None, OrgCaller::Person { .. }) => return Err(usage("name the org: --org <org>")),
    };
    if let OrgCaller::Agent { project_id, .. } = caller {
        if org.head.as_deref() != Some(project_id.as_str()) {
            return Err(refusal("403 Forbidden", "not_org_head", &format!("Only {}'s head reads its members' updates.", org.name)));
        }
    } else if !caller.is_admin() {
        return Err(refusal("403 Forbidden", "role_required", "Reading an org's updates needs an Owner or Admin login."));
    }
    // Default: since the last night began (yesterday 22:00 at the latest).
    let cfg = oc::config();
    let since = match s(m, "since").and_then(|v| v.parse::<i64>().ok()) {
        Some(t) => t,
        None => {
            let local = day::local(cfg.zone(), now);
            let night = day::current_night_at(&cfg.night, local).unwrap_or_else(|| local.date() - chrono::Duration::days(1));
            let base = day::night_base(night);
            now.timestamp() - (local - base).num_seconds().max(0)
        }
    };
    let rows = or_fail(db!(conn, store::updates(&conn, &org.id, since, 200)))?;
    let list: Vec<Value> = rows
        .iter()
        .map(|u| {
            json!({
                "id": u.id, "from": { "id": u.from, "handle": super::caller_handle(&u.from) },
                "body": u.body, "night": u.night, "woke": u.woke, "createdAt": u.created_at
            })
        })
        .collect();
    Ok(ok(json!({ "ok": true, "org": { "id": org.id, "name": org.name }, "since": since, "updates": list })))
}

/// Is `now` inside tonight's run window (run time → day start)? Returns the
/// night key.
pub(crate) fn in_run_window(now: chrono::DateTime<chrono::Utc>) -> Option<String> {
    let cfg = oc::config();
    let local = day::local(cfg.zone(), now);
    let night = day::current_night_at(&cfg.night, local)?;
    (local >= day::on_night(night, cfg.night.run_at)).then(|| day::day_key(night))
}

fn ask(caller: &OrgCaller, m: &Map<String, Value>, now: chrono::DateTime<chrono::Utc>) -> R {
    let OrgCaller::Agent { project_id, .. } = caller else {
        return Err(refusal("403 Forbidden", "agent_only", "ask is a head's question to its own members."));
    };
    let q = or_fail(text::capped(s(m, "question").or_else(|| s(m, "text")).unwrap_or(""), text::ASK_MAX_CHARS, "The question", "usage"))?;
    let org = or_fail(db!(conn, store::headed_by(&conn, project_id)))?.into_iter().next().ok_or_else(|| {
        refusal("403 Forbidden", "not_org_head", "Only an org's head asks its members; you aren't one (k2 organization mine).")
    })?;
    let members: Vec<String> = or_fail(db!(conn, store::members(&conn, &org.id)))?.into_iter().filter(|x| x != project_id).collect();
    let targets: Vec<String> = match s(m, "member") {
        Some(t) => {
            let id = resolve_agent(t)?;
            if !members.contains(&id) {
                return Err(refusal(
                    "403 Forbidden",
                    "not_org_member",
                    &format!("{} isn't in {}. You ask only your own org's members.", super::caller_handle(&id), org.name),
                ));
            }
            vec![id]
        }
        None => members,
    };
    if targets.is_empty() {
        return Err(refusal("409 Conflict", "no_members", &format!("{} has no members besides you yet.", org.name)));
    }
    // Caps: 3 asks a night, 10 a day.
    let cfg = oc::config();
    let local = day::local(cfg.zone(), now);
    let ts = now.timestamp();
    let day_start = day::day_start(&cfg.night, day::read_day_at(&cfg.night, local));
    let since_day = ts - (local - day_start).num_seconds().max(0);
    let (n_day, n_night) = db!(conn, {
        let d = or_fail(store::asks_since(&conn, project_id, since_day))?;
        let n = match day::current_night_at(&cfg.night, local) {
            Some(night) => or_fail(store::asks_since(&conn, project_id, ts - (local - day::night_base(night)).num_seconds().max(0)))?,
            None => 0,
        };
        (d, n)
    });
    if n_day >= oc::ASKS_PER_DAY || n_night >= oc::ASKS_PER_NIGHT {
        return Err(refusal(
            "409 Conflict",
            "ask_limit",
            &format!("A head sends at most {} asks a night and {} a day; you've sent yours. Read the answers with k2 organization updates.", oc::ASKS_PER_NIGHT, oc::ASKS_PER_DAY),
        ));
    }
    let me = super::caller_handle(project_id);
    let line = format!("{}'s head ({me}) asks: {q} Answer with: k2 organization update \"<your news>\"", org.name);
    let from = format!("org:{}", org.name);
    let w = super::waker();
    let group = store::new_id("ask");
    let mut asked = Vec::new();
    let mut held = Vec::new();
    for t in &targets {
        let id = or_fail(db!(conn, store::add_ask(&conn, &group, &org.id, project_id, t, &q, ts)))?;
        match super::wake(w.as_ref(), t, store::WAKE_ASK, &from, &line, ts) {
            super::WakeOutcome::Delivered => {
                let _ = db!(conn, store::set_ask_woke(&conn, &id));
                asked.push(super::caller_handle(t));
            }
            other => held.push(json!({ "member": super::caller_handle(t), "why": other.word() })),
        }
    }
    audit(caller, "ask", format!("{} to {} member(s)", org.name, targets.len()));
    Ok(ok(json!({ "ok": true, "org": { "id": org.id, "name": org.name }, "asked": asked, "notDelivered": held })))
}

fn update(caller: &OrgCaller, m: &Map<String, Value>, now: chrono::DateTime<chrono::Utc>) -> R {
    let OrgCaller::Agent { project_id, .. } = caller else {
        return Err(refusal("403 Forbidden", "agent_only", "update is a member's news to its own org's head."));
    };
    let body = or_fail(text::capped(s(m, "text").unwrap_or(""), text::UPDATE_MAX_CHARS, "An update", "usage"))?;
    let org = own_org(project_id, s(m, "org"))?.ok_or_else(|| {
        refusal("403 Forbidden", "not_org_member", "You're in no org, so there's no head to send news to. k2 organization bulletin shows every org.")
    })?;
    let Some(head) = org.head.clone() else {
        return Err(refusal("409 Conflict", "no_head", &format!("{} has no head yet, so the update has nowhere to go.", org.name)));
    };
    if head == *project_id {
        return Err(refusal("400 Bad Request", "own_head", &format!("You are {}'s head: write the bulletin itself (k2 organization bulletin set).", org.name)));
    }
    let ts = now.timestamp();
    // "Bulletins would run at night, not during the day": stored always,
    // it wakes the head only between the run time and the day's start.
    let night = in_run_window(now);
    let u = or_fail(db!(conn, store::add_update(&conn, &org.id, project_id, &body, night.as_deref(), ts)))?;
    let mut woke = false;
    let mut why = Value::Null;
    if night.is_some() {
        let me = super::caller_handle(project_id);
        let line = format!("Update from {me} for {}: {body} (all of them: k2 organization updates)", org.name);
        match super::wake(super::waker().as_ref(), &head, store::WAKE_UPDATE, &format!("org:{}", org.name), &line, ts) {
            super::WakeOutcome::Delivered => {
                let _ = db!(conn, store::set_update_woke(&conn, &u.id));
                woke = true;
            }
            other => why = json!(other.word()),
        }
    }
    Ok(ok(json!({
        "ok": true,
        "org": { "id": org.id, "name": org.name },
        "head": { "id": head, "handle": super::caller_handle(&head) },
        "stored": true,
        "night": night,
        "deliveredNow": woke,
        "notDeliveredWhy": why,
    })))
}

// ── manage ─────────────────────────────────────────────────────────

fn create(caller: &OrgCaller, m: &Map<String, Value>, now: i64) -> R {
    let name = need(m, "name", "the org")?;
    let org = or_fail(db!(conn, store::create_org(&conn, name, s(m, "purpose").unwrap_or(""), caller.label(), now)))?;
    Ok(wrote(caller, "create", org.name.clone(), json!({ "org": org_json_by_id(&org.id) })))
}

fn rename(caller: &OrgCaller, m: &Map<String, Value>, now: i64) -> R {
    let org = find_org(need(m, "org", "the org")?)?;
    let name = need(m, "name", "the new name")?;
    let new = or_fail(db!(conn, store::rename_org(&conn, &org.id, name, now)))?;
    Ok(wrote(caller, "rename", format!("{} -> {}", org.name, new.name), json!({ "org": org_json_by_id(&org.id) })))
}

fn purpose(caller: &OrgCaller, m: &Map<String, Value>, now: i64) -> R {
    let org = find_org(need(m, "org", "the org")?)?;
    let p = m.get("purpose").and_then(Value::as_str).unwrap_or("");
    or_fail(db!(conn, store::set_purpose(&conn, &org.id, p, now)))?;
    Ok(wrote(caller, "purpose", org.name.clone(), json!({ "org": org_json_by_id(&org.id) })))
}

fn delete(caller: &OrgCaller, m: &Map<String, Value>, now: i64) -> R {
    let org = find_org(need(m, "org", "the org")?)?;
    or_fail(db!(conn, store::delete_org(&conn, &org.id, now)))?;
    Ok(wrote(caller, "delete", org.name.clone(), json!({ "deleted": { "id": org.id, "name": org.name } })))
}

fn member_add(caller: &OrgCaller, m: &Map<String, Value>, now: i64) -> R {
    let org = find_org(need(m, "org", "the org")?)?;
    let agent = resolve_agent(need(m, "agent", "the agent")?)?;
    let changed = or_fail(db!(conn, store::add_member(&conn, &org.id, &agent, caller.label(), now)))?;
    Ok(wrote(
        caller,
        "member_add",
        format!("{} + {}", org.name, super::caller_handle(&agent)),
        json!({ "changed": changed, "org": org_json_by_id(&org.id) }),
    ))
}

fn member_move(caller: &OrgCaller, m: &Map<String, Value>, now: i64) -> R {
    let org = find_org(need(m, "org", "the org")?)?;
    let agent = resolve_agent(need(m, "agent", "the agent")?)?;
    let left = or_fail(db!(conn, store::move_member(&conn, &agent, &org.id, caller.label(), now)))?;
    let from: Vec<String> = left.iter().map(|o| o.name.clone()).collect();
    Ok(wrote(
        caller,
        "member_move",
        format!("{} -> {} (from {})", super::caller_handle(&agent), org.name, from.join(",")),
        json!({ "from": from, "org": org_json_by_id(&org.id) }),
    ))
}

fn member_remove(caller: &OrgCaller, m: &Map<String, Value>, now: i64) -> R {
    let org = find_org(need(m, "org", "the org")?)?;
    let agent = resolve_agent(need(m, "agent", "the agent")?)?;
    let changed = or_fail(db!(conn, store::remove_member(&conn, &org.id, &agent, now)))?;
    Ok(wrote(
        caller,
        "member_remove",
        format!("{} - {}", org.name, super::caller_handle(&agent)),
        json!({ "changed": changed, "org": org_json_by_id(&org.id) }),
    ))
}

fn head(caller: &OrgCaller, m: &Map<String, Value>, now: i64) -> R {
    let org = find_org(need(m, "org", "the org")?)?;
    let agent = match s(m, "agent") {
        Some("none") => None,
        Some(t) => Some(resolve_agent(t)?),
        None => return Err(usage("name the head: agent=<agent> (or agent=none to clear it)")),
    };
    or_fail(db!(conn, store::set_head(&conn, &org.id, agent.as_deref(), now)))?;
    let who = agent.as_deref().map(super::caller_handle).unwrap_or_else(|| "none".into());
    Ok(wrote(caller, "head", format!("{} head {who}", org.name), json!({ "org": org_json_by_id(&org.id) })))
}

/// A line end: `{kind, id}` (the canvas) or a name (the CLI).
fn end_of(m: &Map<String, Value>, k: &str, what: &str) -> Result<NodeRef, CliResponse> {
    if let Some(Value::Object(o)) = m.get(k) {
        let kind = o.get("kind").and_then(Value::as_str).and_then(NodeKind::parse).ok_or_else(|| usage("a line end's kind is agent or org"))?;
        let id = o.get("id").and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty()).ok_or_else(|| usage("a line end needs an id"))?;
        return Ok(NodeRef { kind, id: id.to_string() });
    }
    resolve_end(need(m, k, what)?)
}

fn line_add(caller: &OrgCaller, m: &Map<String, Value>, now: i64) -> R {
    let a = end_of(m, "a", "the first card")?;
    let b = end_of(m, "b", "the second card")?;
    // The widget sends both anchors; the CLI sends a kind word.
    let (kind, swap) = match (s(m, "fromAnchor"), s(m, "toAnchor")) {
        (Some(f), Some(t)) => {
            let (f, t) = match (Anchor::parse(f), Anchor::parse(t)) {
                (Some(f), Some(t)) => (f, t),
                _ => return Err(usage("anchors are top, bottom, left or right")),
            };
            or_fail(store::kind_from_anchors(f, t))?
        }
        _ => match s(m, "kind").map(str::to_ascii_lowercase).as_deref() {
            // The stored kinds: `below` (a sits above b, top to bottom) and
            // `beside`. `above` says the same as `below`; `under` is the
            // other way round (a sits under b: `k2 organization link a below b`).
            Some("below") | Some("above") => (LineKind::Below, false),
            Some("under") => (LineKind::Below, true),
            Some("beside") => (LineKind::Beside, false),
            Some("top-to-top") | Some("top") | Some("bottom-to-bottom") => {
                return Err(fail(&OrgError::new(400, "line_top_to_top", store::TOP_TO_TOP_HINT)))
            }
            _ => return Err(usage("say how they sit: kind=below (a over b), under (a under b) or beside")),
        },
    };
    let (upper, lower) = if swap { (b, a) } else { (a, b) };
    let line = or_fail(db!(conn, store::add_line(&conn, kind, upper, lower, caller.label(), now)))?;
    let w = db!(conn, wire::line_wire(&conn, &line));
    Ok(wrote(caller, "line_add", format!("{} {} {}", w.a.name, line.kind.as_str(), w.b.name), json!({ "line": w })))
}

fn line_remove(caller: &OrgCaller, m: &Map<String, Value>) -> R {
    let n = match s(m, "id") {
        Some(id) => or_fail(db!(conn, store::remove_line_id(&conn, id)))?,
        None => {
            let a = resolve_end(need(m, "a", "the first card")?)?;
            let b = resolve_end(need(m, "b", "the second card")?)?;
            or_fail(db!(conn, store::remove_line(&conn, &a, &b)))?
        }
    };
    if n == 0 {
        return Err(refusal("404 Not Found", "no_line", "There's no line between those two cards (k2 organization show <org> lists its lines)."));
    }
    Ok(wrote(caller, "line_remove", format!("{n} line(s)"), json!({ "removed": n })))
}

fn layout(path: &str, caller: &OrgCaller, m: &Map<String, Value>, now: i64) -> R {
    if !caller.is_admin() {
        return Err(refusal(
            "403 Forbidden",
            "people_only",
            "Only a person (an Owner or Admin) arranges the canvas, by dragging on the Org Chart tab. Agents change orgs, members, heads and lines; the chart lays itself out.",
        ));
    }
    let sent_rev = m.get("revision").and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())));
    if path.ends_with("/tidy") {
        if let Some(want) = sent_rev {
            let rev = or_fail(db!(conn, store::revision(&conn)))?;
            if rev != want {
                return Err(refusal(
                    "409 Conflict",
                    "stale_revision",
                    &format!("The chart changed (revision {rev}, you sent {want}). Reload it and try again."),
                ));
            }
        }
        let n = or_fail(db!(conn, store::tidy(&conn)))?;
        return Ok(wrote(caller, "layout_tidy", format!("{n} card(s) back on auto"), json!({ "reset": n })));
    }
    let want: i64 = sent_rev.ok_or_else(|| usage("send the chart's revision with a move: revision=<n>"))?;
    let kind = s(m, "kind").and_then(NodeKind::parse).ok_or_else(|| usage("kind is agent or org"))?;
    let id = need(m, "id", "the card")?.to_string();
    let num = |k: &str| m.get(k).and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())));
    let (x, y) = match (num("x"), num("y")) {
        (Some(x), Some(y)) if x.is_finite() && y.is_finite() => (x, y),
        _ => return Err(usage("x and y are numbers (canvas coordinates)")),
    };
    db!(conn, {
        let rev = or_fail(store::revision(&conn))?;
        if rev != want {
            return Err(refusal(
                "409 Conflict",
                "stale_revision",
                &format!("The chart changed (revision {rev}, you sent {want}). Reload it and drag again."),
            ));
        }
        // As sent: a member's x, y relative to its bucket (the chart's
        // `nodes` give it that way), an org's or a loose agent's on the canvas.
        let (px, py) = (x, y);
        if kind == NodeKind::Agent {
            if !or_fail(store::agent_exists(&conn, &id))? {
                return Err(fail(&OrgError::not_found("agent", &id, "Reload the chart.")));
            }
        } else if or_fail(store::get_org(&conn, &id))?.is_none() {
            return Err(fail(&OrgError::not_found("org", &id, "Reload the chart.")));
        }
        let w = num("w").filter(|v| v.is_finite() && *v > 0.0);
        let h = num("h").filter(|v| v.is_finite() && *v > 0.0);
        or_fail(store::place(&conn, &Placed { node: NodeRef { kind, id: id.clone() }, x: px, y: py, w, h }, now))?;
        Ok(())
    })?;
    Ok(wrote(caller, "layout_move", format!("{} {id}", kind.as_str()), json!({ "moved": { "kind": kind.as_str(), "id": id } })))
}
