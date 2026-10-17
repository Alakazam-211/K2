//! `/cli/sources/*` HTTP routes (prd-garden-v3-external-data-cards-v1;
//! contract-sources-v1 §4).
//!
//! Who may do what (contract §4.1):
//! - `role_gate` (route_policy floors) stops Connect logins below the row.
//! - [`who_from_tcp`] / [`who_from_cell`] classify the caller with the
//!   compute classifier (`compute_routes::Who`): app passes, API keys and
//!   stream tokens are 403 `apps_not_allowed`; no valid credential is 401
//!   `invalid_token`.
//! - Both transports refuse a GET to a POST-only path with 405 before the
//!   handler runs ([`is_post_route`]).
//! - The per-cell socket serves EVERY `/cli/sources/*` path for the bound
//!   session (agent or K2 shell tab); handlers refuse what agents can't do
//!   with 403 `person_only`, never 404 [GV52]. `shell` follows the agent
//!   rules.
//! - "Manage Gardens and data sources" (`gardens_manage`, 0181): an agent
//!   whose workspace has it may also add, pause and resume sources (and
//!   approve, bind, pin: `sources::approve_routes`) with no person in the
//!   loop. Each act writes a source_audit row with its session and an
//!   Access Audit line ([`crate::gardens_manage`]).
//!
//! The handlers are blocking (DB); the dispatcher runs [`handle`] on a
//! blocking thread. B1 owns this file and its source handlers; the
//! trigger, approval and Try/simulate paths delegate to B2's modules under
//! `crate::sources`.

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::log_debug;
use k2_core::sources::spec::{SpecError, SpecWarning, Validated};
use k2_core::sources::view::{self, SourceResult, Viewer};
use k2_core::sources::{
    history, lint, presets, store, AgentAccess, FetchTargets, Keep, SourceRow, SourceState, SourceStatus, Viewers,
};
use rusqlite::Connection;

use crate::cli_response::CliResponse;
use crate::compute_routes::Who;
use crate::sources::{approve_routes, tinker_routes, trigger_routes, SourceErr};

/// Largest request body (256 KiB).
pub const MAX_BODY: usize = 256 * 1024;

/// Every sources route.
pub const ROUTES: &[&str] = &[
    "/cli/sources/add",
    "/cli/sources/approve",
    "/cli/sources/batch-approve",
    "/cli/sources/changes",
    "/cli/sources/edit",
    "/cli/sources/export",
    "/cli/sources/history",
    "/cli/sources/list",
    "/cli/sources/mute",
    "/cli/sources/pause",
    "/cli/sources/pending",
    "/cli/sources/pin",
    "/cli/sources/propose",
    "/cli/sources/read",
    "/cli/sources/refresh",
    "/cli/sources/remove",
    "/cli/sources/resume",
    "/cli/sources/secret",
    "/cli/sources/show",
    "/cli/sources/simulate",
    "/cli/sources/status",
    "/cli/sources/triggers/add",
    "/cli/sources/triggers/approve",
    "/cli/sources/triggers/edit",
    "/cli/sources/triggers/list",
    "/cli/sources/triggers/log",
    "/cli/sources/triggers/pause",
    "/cli/sources/triggers/propose",
    "/cli/sources/triggers/remove",
    "/cli/sources/triggers/resume",
    "/cli/sources/triggers/show",
    "/cli/sources/triggers/test",
    "/cli/sources/try",
    "/cli/sources/unmute",
    "/cli/sources/unpin",
];

/// POST-only rows (405 on GET).
pub const POST_ROUTES: &[&str] = &[
    "/cli/sources/add",
    "/cli/sources/approve",
    "/cli/sources/batch-approve",
    "/cli/sources/edit",
    "/cli/sources/mute",
    "/cli/sources/pause",
    "/cli/sources/pin",
    "/cli/sources/propose",
    "/cli/sources/refresh",
    "/cli/sources/remove",
    "/cli/sources/resume",
    "/cli/sources/secret",
    "/cli/sources/simulate",
    "/cli/sources/triggers/add",
    "/cli/sources/triggers/approve",
    "/cli/sources/triggers/edit",
    "/cli/sources/triggers/pause",
    "/cli/sources/triggers/propose",
    "/cli/sources/triggers/remove",
    "/cli/sources/triggers/resume",
    "/cli/sources/triggers/test",
    "/cli/sources/try",
    "/cli/sources/unmute",
    "/cli/sources/unpin",
];

pub fn is_route(p: &str) -> bool {
    ROUTES.contains(&p)
}

pub fn is_post_route(p: &str) -> bool {
    POST_ROUTES.contains(&p)
}

/// Classify a TCP caller (the dispatcher's effective auth query).
pub fn who_from_tcp(query: &str, bearer: Option<&str>, owner_token: &str) -> Who {
    crate::compute_routes::who_from_tcp(query, bearer, owner_token)
}

/// Classify a per-cell socket caller (the bound session's passport).
pub fn who_from_cell(session_id: &str, principal: &crate::session_token::HookPrincipal) -> Who {
    crate::compute_routes::who_from_cell(session_id, principal)
}

/// 413 for a body over [`MAX_BODY`].
pub fn too_large() -> CliResponse {
    SourceErr::new("bad_request", 413, format!("request bodies here are at most {} KiB", MAX_BODY / 1024)).response()
}

/// 405 for a GET on a POST-only path.
pub fn method_not_allowed() -> CliResponse {
    SourceErr::new("method_not_allowed", 405, "this route takes POST").response()
}

/// Run one sources route. The transport already refused a GET to a
/// POST-only path and an oversized body.
pub fn handle(path: &str, who: Who, body: &[u8], params: &HashMap<String, String>) -> CliResponse {
    match &who {
        Who::Invalid => return SourceErr::new("invalid_token", 401, "invalid or missing token").response(),
        Who::Wrong => {
            return SourceErr::new(
                "apps_not_allowed",
                403,
                "app passes, API keys and stream tokens can't use data sources",
            )
            .response()
        }
        _ => {}
    }
    let r: Result<serde_json::Value, SourceErr> = match path {
        // B1: sources.
        "/cli/sources/list"
        | "/cli/sources/show"
        | "/cli/sources/read"
        | "/cli/sources/history"
        | "/cli/sources/changes"
        | "/cli/sources/export"
        | "/cli/sources/status"
        | "/cli/sources/propose"
        | "/cli/sources/add"
        | "/cli/sources/edit"
        | "/cli/sources/remove"
        | "/cli/sources/pause"
        | "/cli/sources/resume"
        | "/cli/sources/mute"
        | "/cli/sources/unmute"
        | "/cli/sources/refresh" => b1(path, &who, body, params),
        // B2: Try and simulate.
        "/cli/sources/try" | "/cli/sources/simulate" => tinker_routes::handle(path, &who, body, params),
        // B2: approvals, secret binding, pins.
        "/cli/sources/pending"
        | "/cli/sources/approve"
        | "/cli/sources/batch-approve"
        | "/cli/sources/secret"
        | "/cli/sources/pin"
        | "/cli/sources/unpin" => approve_routes::handle(path, &who, body, params),
        // B2: triggers.
        "/cli/sources/triggers/list"
        | "/cli/sources/triggers/show"
        | "/cli/sources/triggers/log"
        | "/cli/sources/triggers/propose"
        | "/cli/sources/triggers/add"
        | "/cli/sources/triggers/edit"
        | "/cli/sources/triggers/approve"
        | "/cli/sources/triggers/pause"
        | "/cli/sources/triggers/resume"
        | "/cli/sources/triggers/remove"
        | "/cli/sources/triggers/test" => trigger_routes::handle(path, &who, body, params),
        _ => Err(SourceErr::new("not_found", 404, "no such sources route")),
    };
    match r {
        Ok(v) => CliResponse::ok_json(v.to_string()),
        Err(e) => e.response(),
    }
}

/// B1's source routes.
fn b1(path: &str, who: &Who, body: &[u8], params: &HashMap<String, String>) -> Result<serde_json::Value, SourceErr> {
    match path {
        "/cli/sources/list" => list(who),
        "/cli/sources/show" => show(who, params),
        "/cli/sources/read" => read(who, params),
        "/cli/sources/history" => history_route(who, params),
        "/cli/sources/changes" => changes(who, params),
        "/cli/sources/export" => export(who, params),
        "/cli/sources/status" => status_route(who),
        "/cli/sources/propose" => create(who, body, false),
        "/cli/sources/add" => create(who, body, true),
        "/cli/sources/edit" => edit(who, body),
        "/cli/sources/remove" => remove(who, body),
        "/cli/sources/pause" => pause_resume(who, body, true),
        "/cli/sources/resume" => pause_resume(who, body, false),
        "/cli/sources/mute" => mute(who, body),
        "/cli/sources/unmute" => unmute(who, body),
        "/cli/sources/refresh" => refresh(who, body),
        _ => Err(SourceErr::new("not_found", 404, "no such sources route")),
    }
}

// ── shared helpers (B2's handlers may use these too) ─────────────────

type R<T> = Result<T, SourceErr>;
type J = serde_json::Value;

pub(crate) fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// The owner token or a Connect login.
pub(crate) fn is_person(who: &Who) -> bool {
    matches!(who, Who::Owner | Who::Login { .. })
}

/// The project of an agent or K2 shell passport (`shell` follows the
/// agent rules).
pub(crate) fn passport_project(who: &Who) -> Option<&str> {
    match who {
        Who::Agent { project_id, .. } | Who::Shell { project_id, .. } => Some(project_id),
        _ => None,
    }
}

/// `made_by`: `owner` | `login:<u>` | `agent:<projectId>`.
pub(crate) fn maker(who: &Who) -> (String, Option<String>) {
    match who {
        Who::Owner => ("owner".into(), None),
        Who::Login { username, .. } => (format!("login:{username}"), None),
        Who::Agent { project_id, .. } | Who::Shell { project_id, .. } => {
            (format!("agent:{project_id}"), Some(project_id.clone()))
        }
        Who::Wrong | Who::Invalid => ("-".into(), None),
    }
}

/// `(actor, actor_kind)` for `source_audit`.
pub(crate) fn actor(who: &Who) -> (String, &'static str) {
    match who {
        Who::Owner => ("owner".into(), "owner"),
        Who::Login { username, .. } => (format!("login:{username}"), "login"),
        Who::Agent { project_id, .. } => (format!("agent:{project_id}"), "agent"),
        Who::Shell { project_id, .. } => (format!("shell:{project_id}"), "shell"),
        Who::Wrong | Who::Invalid => ("-".into(), "daemon"),
    }
}

pub(crate) fn viewer(who: &Who) -> Viewer {
    if passport_project(who).is_some() {
        Viewer::Agent
    } else {
        Viewer::Person
    }
}

/// The owner token or an Owner-role login.
pub(crate) fn is_owner(who: &Who) -> bool {
    matches!(who, Who::Owner | Who::Login { role: Role::Owner, .. })
}

/// A person, or a passport whose workspace has "Manage Gardens and data
/// sources" (0181); else 403 `person_only`.
pub(crate) fn person_or_manager(who: &Who) -> R<()> {
    approve_routes::util::need_person_or_manager(who)
}

/// The `viewers` floor for reads: a Member login can't read an `admin`
/// source. The owner token and passports skip it (passports are masked).
pub(crate) fn can_read(who: &Who, row: &SourceRow) -> R<()> {
    if let Who::Login { role: Role::Member, .. } = who {
        if row.viewers == Viewers::Admin {
            return Err(SourceErr::new("cap_not_granted", 403, "this source is for admins on this server").with("role", "admin"));
        }
    }
    Ok(())
}

/// A live source by name: 404 `source_not_found`, or 410 `removed` for a
/// tombstoned name.
pub(crate) fn find(conn: &Connection, name: &str) -> R<SourceRow> {
    if let Some(r) = store::get_by_name(conn, name)? {
        return Ok(r);
    }
    let removed: Option<i64> = conn.query_row(
        "SELECT MAX(removed_at) FROM sources WHERE name = ?1 AND status = 'removed'",
        rusqlite::params![name],
        |r| r.get(0),
    )?;
    match removed {
        Some(at) => Err(SourceErr::new("removed", 410, format!("{name} was removed")).with("removedAt", at)),
        None => Err(SourceErr::new("source_not_found", 404, format!("no source named {name}"))),
    }
}

/// Agents change only sources they made.
pub(crate) fn own(who: &Who, row: &SourceRow) -> R<()> {
    match passport_project(who) {
        Some(p) if row.made_by_project.as_deref() != Some(p) => {
            Err(SourceErr::new("not_own", 403, "agents change only the sources they made"))
        }
        _ => Ok(()),
    }
}

/// 423 for an agent changing a pinned source or one a locked Garden on
/// this computer reads [GV16, GV65].
pub(crate) fn not_locked(who: &Who, row: &SourceRow) -> R<()> {
    if passport_project(who).is_none() {
        return Ok(());
    }
    if row.pinned {
        return Err(SourceErr::new("source_pinned", 423, "a person pinned this source; agents can't change it"));
    }
    // B3's set already folds `name::<this host>` into bare names.
    if k2_core::zen::lock::locked_placement_sources().contains(&row.name) {
        return Err(SourceErr::new("garden_locked", 423, "a locked Garden on this computer shows this source"));
    }
    Ok(())
}

fn bad(code: &'static str, hint: impl Into<String>) -> SourceErr {
    SourceErr::new(code, 400, hint)
}

fn spec_err(e: SpecError) -> SourceErr {
    match e.code {
        "lan_owner_only" => SourceErr::new("lan_owner_only", 403, e.hint).with("field", e.field),
        _ => SourceErr::new("bad_spec", 400, e.hint).with("field", e.field).with("specCode", e.code),
    }
}

fn body_json(body: &[u8]) -> R<J> {
    if body.is_empty() {
        return Ok(serde_json::json!({}));
    }
    serde_json::from_slice(body).map_err(|_| bad("bad_request", "the body is JSON"))
}

fn parse<T: serde::de::DeserializeOwned>(body: &[u8], hint: &str) -> R<T> {
    serde_json::from_slice(body).map_err(|_| bad("bad_request", hint.to_string()))
}

fn param<'a>(params: &'a HashMap<String, String>, k: &str) -> R<&'a str> {
    params.get(k).map(String::as_str).filter(|v| !v.is_empty()).ok_or_else(|| bad("bad_request", format!("{k}= is required")))
}

fn audit(conn: &Connection, who: &Who, action: &str, source_id: Option<&str>, detail: J) {
    let (a, k) = actor(who);
    if let Err(e) = store::audit(conn, now(), &a, k, action, source_id, None, &detail) {
        log_debug!("[sources] audit write failed: {e}");
    }
}

fn changed(rev: i64) {
    crate::sources::notify_changed(rev);
}

/// The card view for `row` as `who` sees it.
fn view_of(conn: &Connection, who: &Who, row: &SourceRow) -> R<SourceResult> {
    let (st, proj) = store::load_state(conn, &row.id)?.unwrap_or_else(|| (SourceState::new(&row.id), None));
    Ok(view::build(row, &st, proj.as_ref(), viewer(who), now()))
}

/// GV47: a `shape` source read with values by a person (or anything
/// holding the owner token) is audited, at most once per actor per hour.
fn audit_unmasked(conn: &Connection, who: &Who, row: &SourceRow, v: &SourceResult) {
    if row.agent_access != AgentAccess::Shape || v.masked || row.keep == Keep::States {
        return;
    }
    let (a, _) = actor(who);
    if store::audited_since(conn, &a, "unmasked_read", &row.id, now() - 3600).unwrap_or(false) {
        return;
    }
    audit(conn, who, "unmasked_read", Some(&row.id), serde_json::json!({"name": row.name}));
}

/// The export shape (secret names only; query values masked for agents).
fn export_of(who: &Who, row: &SourceRow) -> R<J> {
    let spec = store::spec_of(row).map_err(|_| SourceErr::new("db_error", 500, "this source's definition can't be read"))?;
    let mut v = serde_json::to_value(&spec).unwrap_or(J::Null);
    if viewer(who) == Viewer::Agent {
        if let Some(u) = v.pointer_mut("/fetch/url") {
            *u = J::String(lint::mask_query(u.as_str().unwrap_or_default()));
        }
        if let Some(J::Array(ts)) = v.pointer_mut("/fetch/targets") {
            for t in ts {
                if let Some(u) = t.get_mut("url") {
                    *u = J::String(lint::mask_query(u.as_str().unwrap_or_default()));
                }
            }
        }
    }
    Ok(v)
}

fn def_of(who: &Who, row: &SourceRow) -> R<J> {
    let mut v = export_of(who, row)?;
    if let J::Object(m) = &mut v {
        m.insert("status".into(), store::code_of(&row.status).into());
        m.insert("madeBy".into(), row.made_by.clone().into());
        m.insert("approvedBy".into(), row.approved_by.clone().into());
        m.insert("approvedAt".into(), row.approved_at.into());
        m.insert("specHash".into(), row.spec_hash.clone().into());
        m.insert("createdAt".into(), row.created_at.into());
        m.insert("updatedAt".into(), row.updated_at.into());
        m.insert("pinned".into(), row.pinned.into());
        m.insert("secretBound".into(), row.secret_bound.into());
        m.insert("lan".into(), row.lan.into());
        m.insert("origins".into(), row.origins.clone().into());
        if is_person(who) {
            m.insert("id".into(), row.id.clone().into());
        }
    }
    Ok(v)
}

fn warnings_of(row: &SourceRow) -> Vec<SpecWarning> {
    let Ok(spec) = store::spec_of(row) else { return vec![] };
    let urls: Vec<(String, String)> = match &spec.fetch {
        FetchTargets::Url(u) => vec![("fetch.url".into(), u.clone())],
        FetchTargets::Targets(ts) => {
            ts.iter().enumerate().map(|(i, t)| (format!("fetch.targets[{i}].url"), t.url.clone())).collect()
        }
        FetchTargets::UrlSecret { .. } => vec![],
    };
    urls.iter()
        .filter_map(|(f, u)| url::Url::parse(u).ok().and_then(|p| lint::path_token_warning(&p, f)))
        .collect()
}

fn approval_of(v: &Validated) -> &'static str {
    if v.lan {
        "owner_only"
    } else if v.spec.has_secret() {
        "needs_confirm"
    } else {
        "one_click"
    }
}

// ── reads ────────────────────────────────────────────────────────────

fn list(who: &Who) -> R<J> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut out = Vec::new();
    for row in store::list_live(&conn)? {
        if can_read(who, &row).is_err() {
            continue;
        }
        let view = view_of(&conn, who, &row)?;
        out.push(serde_json::json!({
            "name": row.name,
            "label": row.label,
            "status": store::code_of(&row.status),
            "madeBy": row.made_by,
            "secret": row.secret_name.clone().or(row.url_secret_name.clone()),
            "pinned": row.pinned,
            "keep": store::code_of(&row.keep),
            "viewers": store::code_of(&row.viewers),
            "agentAccess": store::code_of(&row.agent_access),
            "everySecs": row.every_secs,
            "targets": store::target_count(&row),
            "triggers": k2_core::sources::trigger_store::count_for_source(&conn, &row.id)?,
            "view": view,
        }));
    }
    let status_order = |s: &J| match s["status"].as_str() {
        Some("active") => 0,
        Some("proposed") => 1,
        _ => 2,
    };
    out.sort_by_key(|s| (status_order(s), s["name"].as_str().unwrap_or_default().to_string()));
    Ok(serde_json::json!({"sources": out, "serverNow": now(), "rev": store::current_rev(&conn)?}))
}

fn show(who: &Who, params: &HashMap<String, String>) -> R<J> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row = find(&conn, param(params, "name")?)?;
    can_read(who, &row)?;
    let view = view_of(&conn, who, &row)?;
    audit_unmasked(&conn, who, &row, &view);
    Ok(serde_json::json!({"source": def_of(who, &row)?, "view": view, "warnings": warnings_of(&row)}))
}

fn read(who: &Who, params: &HashMap<String, String>) -> R<J> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row = find(&conn, param(params, "name")?)?;
    can_read(who, &row)?;
    let view = view_of(&conn, who, &row)?;
    audit_unmasked(&conn, who, &row, &view);
    Ok(serde_json::to_value(view).unwrap_or(J::Null))
}

fn history_route(who: &Who, params: &HashMap<String, String>) -> R<J> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row = find(&conn, param(params, "name")?)?;
    can_read(who, &row)?;
    if row.keep == Keep::States {
        return Err(SourceErr::new("no_history", 409, "this source keeps states only, so it has no history"));
    }
    let field = param(params, "field")?;
    let spec = store::spec_of(&row).map_err(|_| SourceErr::new("db_error", 500, "this source's definition can't be read"))?;
    if !spec.extract.trend.iter().any(|t| t == field) {
        return Err(bad("bad_request", format!("{field} isn't a trend of this source (extract.trend)")));
    }
    let t = now();
    let num = |k: &str| params.get(k).and_then(|v| v.parse::<i64>().ok());
    let from = num("from").unwrap_or(t - history::RAW_KEEP_SECS);
    let to = num("to").unwrap_or(t);
    let res = history::resolution(from, t);
    let pts = store::points(&conn, &row.id, field, res, from, to)?;
    let pts = history::downsample(&pts, history::MAX_POINTS);
    let points: Vec<J> = pts.iter().map(|(t, v)| serde_json::json!({"t": t, "v": v})).collect();
    // Agents on a shape source see the line's shape, not its values.
    let masked = viewer(who) == Viewer::Agent && row.agent_access == AgentAccess::Shape;
    let points: Vec<J> = if masked {
        points.into_iter().map(|p| serde_json::json!({"t": p["t"], "v": if p["v"].is_null() { J::Null } else { 0.into() }})).collect()
    } else {
        points
    };
    Ok(serde_json::json!({"field": field, "res": res, "points": points}))
}

fn changes(who: &Who, params: &HashMap<String, String>) -> R<J> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let since = params.get("since").and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
    let rev = store::current_rev(&conn)?;
    let full = since <= 0 || since < store::reaped_rev(&conn)?;
    let (rows, removed) = if full {
        (store::list_live(&conn)?, Vec::new())
    } else {
        store::changed_since(&conn, since)?
    };
    let mut changed = Vec::new();
    for row in rows {
        if can_read(who, &row).is_ok() {
            changed.push(view_of(&conn, who, &row)?);
        }
    }
    Ok(serde_json::json!({"rev": rev, "full": full, "changed": changed, "removed": removed}))
}

fn export(who: &Who, params: &HashMap<String, String>) -> R<J> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row = find(&conn, param(params, "name")?)?;
    can_read(who, &row)?;
    export_of(who, &row)
}

fn status_route(_who: &Who) -> R<J> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let rows = store::list_live(&conn)?;
    let active = rows.iter().filter(|r| r.status == SourceStatus::Active).count();
    let targets: usize = rows.iter().filter(|r| r.status == SourceStatus::Active).map(store::target_count).sum();
    let st = crate::sources::engine().status();
    let parked: Vec<J> = st
        .parked
        .iter()
        .map(|(o, p)| serde_json::json!({"origin": o, "untilAt": p.until_at, "reason": p.reason.code()}))
        .collect();
    let gap = st.last_gap.map(|(k, f, t)| serde_json::json!({"kind": k.code(), "from": f, "to": t}));
    Ok(serde_json::json!({
        "sources": rows.len(),
        "active": active,
        "targets": targets,
        "inFlight": st.in_flight,
        "behindSecs": st.behind_secs,
        "parkedOrigins": parked,
        "lastGap": gap,
        "airgap": k2_core::airgap::enabled(),
        "running": st.running,
    }))
}

// ── definitions ──────────────────────────────────────────────────────

/// Expand presets, validate (the owner may add LAN/http [GV54]), and
/// check the per-server caps and the name.
fn prepare(conn: &Connection, who: &Who, body: &J, replacing: Option<&SourceRow>) -> R<Validated> {
    let spec = presets::expand(body).map_err(spec_err)?;
    let v = spec.validate(is_owner(who)).map_err(spec_err)?;
    let (n, targets) = store::live_counts(conn)?;
    let (n, targets) = match replacing {
        Some(r) => (n.saturating_sub(1), targets.saturating_sub(store::target_count(r))),
        None => (n, targets),
    };
    if n + 1 > k2_core::sources::MAX_SOURCES {
        return Err(SourceErr::new("too_many_sources", 409, "a server runs at most 64 sources"));
    }
    let new_targets = match &v.spec.fetch {
        FetchTargets::Targets(t) => t.len(),
        _ => 1,
    };
    if targets + new_targets > k2_core::sources::MAX_TARGETS_PER_SERVER {
        return Err(SourceErr::new("too_many_targets", 409, "a server checks at most 500 targets in all"));
    }
    if replacing.is_none() {
        if store::get_by_name(conn, &v.spec.name)?.is_some() {
            return Err(SourceErr::new("name_taken", 409, format!("a source named {} already exists", v.spec.name)));
        }
        if let Some(at) = store::held_name(conn, &v.spec.name, now())? {
            return Err(SourceErr::new("name_held", 409, "this name was removed recently and is held for 7 days")
                .with("untilAt", at + k2_core::sources::TOMBSTONE_SECS));
        }
    }
    Ok(v)
}

fn create(who: &Who, body: &[u8], add: bool) -> R<J> {
    if add {
        person_or_manager(who)?;
    }
    let body = body_json(body)?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let v = prepare(&conn, who, &body, None)?;
    let (made_by, project) = maker(who);
    let active = add && !v.spec.has_secret();
    let status = if active { SourceStatus::Active } else { SourceStatus::Proposed };
    let approval = approval_of(&v);
    let row = store::tx(&conn, |c| {
        let row = store::insert_source(
            c,
            &store::NewSource {
                v: &v,
                status,
                made_by: &made_by,
                made_by_project: project.as_deref(),
                approved_by: active.then_some(made_by.as_str()),
                now: now(),
            },
        )?;
        let (a, k) = actor(who);
        store::audit(
            c,
            now(),
            &a,
            k,
            if add { "add" } else { "propose" },
            Some(&row.id),
            None,
            &approve_routes::util::with_session(
                who,
                serde_json::json!({"name": row.name, "origins": row.origins, "secret": row.secret_name, "lan": row.lan}),
            ),
        )?;
        Ok(row)
    })?;
    changed(row.rev);
    if add {
        crate::gardens_manage::audit_who(who, "sources-add", &row.name, None);
    }
    if active {
        crate::sources::engine().poll_now(&row.id)?;
    } else {
        crate::sources::engine().reload();
    }
    Ok(serde_json::json!({
        "name": row.name,
        "status": store::code_of(&row.status),
        "specHash": row.spec_hash,
        "warnings": v.warnings,
        "approval": approval,
    }))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct EditBody {
    name: String,
    set: serde_json::Map<String, J>,
}

const AGENT_FORBIDDEN_FIELDS: &[&str] = &["viewers", "agentAccess", "keep", "secret", "pinned"];

fn edit(who: &Who, body: &[u8]) -> R<J> {
    let b: EditBody = parse(body, "edit takes {name, set: {...}}")?;
    if b.set.contains_key("name") {
        return Err(bad("bad_request", "a source is never renamed; change its label"));
    }
    let agent = passport_project(who).is_some();
    if agent {
        if let Some(f) = AGENT_FORBIDDEN_FIELDS.iter().find(|f| b.set.contains_key(**f)) {
            return Err(SourceErr::new("field_person_only", 403, format!("only a person sets {f}")).with("field", *f));
        }
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row = find(&conn, &b.name)?;
    own(who, &row)?;
    not_locked(who, &row)?;
    let mut cur = export_of(&Who::Owner, &row)?;
    if let J::Object(m) = &mut cur {
        for (k, v) in b.set {
            m.insert(k, v);
        }
    }
    let v = prepare(&conn, who, &cur, Some(&row))?;
    let fetch_changed = v.origins != row.origins
        || store::spec_of(&row).is_ok_and(|old| old.fetch != v.spec.fetch || old.headers != v.spec.headers || old.extract != v.spec.extract);
    // GV53: an agent's change to what a secret-bound source fetches sends it
    // back for approval and unbinds the secret; a person's change of origin
    // unbinds too (the grant is for the old origin).
    let unbind = row.secret_bound && ((agent && fetch_changed) || v.origins != row.origins);
    let status = if unbind || (row.status == SourceStatus::Proposed) {
        SourceStatus::Proposed
    } else {
        row.status
    };
    if unbind {
        crate::sources::secrets::unbind(&row.id)?;
    }
    let rev = store::tx(&conn, |c| {
        let rev = store::update_spec(c, &row.id, &v, status, unbind, now())?;
        let (a, k) = actor(who);
        store::audit(
            c,
            now(),
            &a,
            k,
            if unbind { "edit_unbound" } else { "edit" },
            Some(&row.id),
            None,
            &serde_json::json!({"name": row.name, "origins": v.origins}),
        )?;
        Ok(rev)
    })?;
    changed(rev);
    if status == SourceStatus::Active {
        crate::sources::engine().poll_now(&row.id)?;
    } else {
        crate::sources::engine().reload();
    }
    Ok(serde_json::json!({
        "name": row.name,
        "status": store::code_of(&status),
        "specHash": v.spec.hash(),
        "warnings": v.warnings,
        "unbound": unbind,
    }))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct NameBody {
    name: String,
}

fn remove(who: &Who, body: &[u8]) -> R<J> {
    let b: NameBody = parse(body, "remove takes {name}")?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row = find(&conn, &b.name)?;
    own(who, &row)?;
    not_locked(who, &row)?;
    if row.secret_bound {
        crate::sources::secrets::unbind(&row.id)?;
    }
    let t = now();
    let (rev, triggers) = store::tx(&conn, |c| {
        let r = store::tombstone(c, &row.id, t)?;
        let (a, k) = actor(who);
        store::audit(c, t, &a, k, "remove", Some(&row.id), None, &serde_json::json!({"name": row.name, "triggersRemoved": r.1}))?;
        Ok(r)
    })?;
    changed(rev);
    crate::sources::engine().reload();
    Ok(serde_json::json!({"removed": row.name, "heldUntilAt": t + k2_core::sources::TOMBSTONE_SECS, "triggersRemoved": triggers}))
}

fn pause_resume(who: &Who, body: &[u8], pause: bool) -> R<J> {
    person_or_manager(who)?;
    let b: NameBody = parse(body, "takes {name}")?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row = find(&conn, &b.name)?;
    // A locked Garden on this computer shows it: agents leave it alone,
    // switch or not (prd-garden-lock-and-sync-v1).
    if passport_project(who).is_some() && k2_core::zen::lock::locked_placement_sources().contains(&row.name) {
        return Err(SourceErr::new("garden_locked", 423, "a locked Garden on this computer shows this source"));
    }
    let status = match (pause, row.status) {
        (true, SourceStatus::Active) => SourceStatus::Paused,
        (false, SourceStatus::Paused) => SourceStatus::Active,
        (_, s) => {
            return Err(SourceErr::new(
                "bad_request",
                409,
                format!("{} is {}", row.name, store::code_of(&s)),
            ))
        }
    };
    let reason = pause.then_some(k2_core::sources::PauseReason::PausedByPerson);
    let rev = store::tx(&conn, |c| {
        let rev = store::set_status(c, &row.id, status, reason, now())?;
        let (a, k) = actor(who);
        store::audit(
            c,
            now(),
            &a,
            k,
            if pause { "pause" } else { "resume" },
            Some(&row.id),
            None,
            &approve_routes::util::with_session(who, serde_json::json!({"name": row.name})),
        )?;
        Ok(rev)
    })?;
    changed(rev);
    drop(conn);
    crate::gardens_manage::audit_who(who, if pause { "sources-pause" } else { "sources-resume" }, &row.name, None);
    if pause {
        crate::sources::engine().reload();
    } else {
        crate::sources::engine().poll_now(&row.id)?;
    }
    Ok(serde_json::json!({"name": row.name, "status": store::code_of(&status)}))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct MuteBody {
    name: String,
    for_secs: u32,
    reason: Option<String>,
}

fn mute(who: &Who, body: &[u8]) -> R<J> {
    let b: MuteBody = parse(body, "mute takes {name, forSecs, reason?}")?;
    let agent = passport_project(who).is_some();
    let reason = b.reason.as_deref().map(str::trim).filter(|r| !r.is_empty());
    if reason.is_some_and(|r| r.chars().count() > 200) {
        return Err(bad("bad_request", "a reason is at most 200 characters"));
    }
    if agent && reason.is_none() {
        return Err(SourceErr::new("reason_required", 400, "agents say why they mute (--reason)"));
    }
    let max = if agent { k2_core::sources::AGENT_MUTE_MAX_SECS } else { k2_core::sources::PERSON_MUTE_MAX_SECS };
    if b.for_secs == 0 || b.for_secs > max {
        return Err(SourceErr::new("too_long", 403, format!("mute for 1 s to {} h", max / 3600)));
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row = find(&conn, &b.name)?;
    let until = now() + i64::from(b.for_secs);
    let (by, _) = actor(who);
    let rev = store::tx(&conn, |c| {
        let rev = store::set_mute(c, &row.id, Some(until), Some(&by), reason, now())?;
        let (a, k) = actor(who);
        store::audit(c, now(), &a, k, "mute", Some(&row.id), None, &serde_json::json!({"name": row.name, "forSecs": b.for_secs}))?;
        Ok(rev)
    })?;
    changed(rev);
    crate::sources::engine().reload();
    Ok(serde_json::json!({"name": row.name, "muted": {"until": until, "by": by, "reason": reason}}))
}

fn unmute(who: &Who, body: &[u8]) -> R<J> {
    let b: NameBody = parse(body, "unmute takes {name}")?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row = find(&conn, &b.name)?;
    let (by, _) = actor(who);
    if passport_project(who).is_some() && row.mute_by.as_deref().is_some_and(|m| m != by) {
        return Err(SourceErr::new("not_own", 403, "agents end only the mutes they set"));
    }
    let rev = store::tx(&conn, |c| {
        let rev = store::set_mute(c, &row.id, None, None, None, now())?;
        let (a, k) = actor(who);
        store::audit(c, now(), &a, k, "unmute", Some(&row.id), None, &serde_json::json!({"name": row.name}))?;
        Ok(rev)
    })?;
    changed(rev);
    crate::sources::engine().reload();
    if let Some(r) = store::get_by_id(&conn, &row.id)? {
        drop(conn);
        crate::sources::wake::handle_unmute(&r, now());
    }
    Ok(serde_json::json!({"name": row.name, "muted": J::Null}))
}

fn refresh(who: &Who, body: &[u8]) -> R<J> {
    let b: NameBody = parse(body, "refresh takes {name}")?;
    let id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let row = find(&conn, &b.name)?;
        can_read(who, &row)?;
        row.id
    };
    let e = crate::sources::engine();
    if let Err(wait) = e.refresh_gate(&id) {
        return Err(SourceErr::new("rate_limited", 429, "refresh at most once every 15 s").with("retryAfterSecs", wait));
    }
    let next = e.poll_now(&id)?;
    Ok(serde_json::json!({"queued": true, "nextAt": next}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(r: &CliResponse) -> &'static str {
        r.status
    }

    #[test]
    fn route_lists_are_sorted_unique_and_post_rows_are_routes() {
        for w in ROUTES.windows(2) {
            assert!(w[0] < w[1], "ROUTES sorted and unique: {} then {}", w[0], w[1]);
        }
        for w in POST_ROUTES.windows(2) {
            assert!(w[0] < w[1], "POST_ROUTES sorted and unique: {} then {}", w[0], w[1]);
        }
        for p in POST_ROUTES {
            assert!(is_route(p), "{p} is a POST row but not a route");
        }
        assert_eq!(ROUTES.len(), 35);
        assert_eq!(POST_ROUTES.len(), 24);
    }

    /// The policy table agrees with these lists: every route is classified,
    /// POST-only rows have no GET form, GET rows take no POST.
    #[test]
    fn every_route_matches_its_policy_row() {
        for p in ROUTES {
            let row = crate::routes::route_policy::lookup(p).unwrap_or_else(|| panic!("{p} has no policy row"));
            if is_post_route(p) {
                assert!(row.get.is_none() && row.post.is_some(), "{p} is POST-only");
            } else {
                assert!(row.get.is_some() && row.post.is_none(), "{p} is GET-only");
            }
        }
    }

    #[test]
    fn handle_refuses_apps_and_invalid_tokens_before_any_route() {
        let params = HashMap::new();
        let r = handle("/cli/sources/list", Who::Wrong, b"", &params);
        assert_eq!(status(&r), "403 Forbidden");
        assert!(r.body.contains("apps_not_allowed"), "{}", r.body);
        let r = handle("/cli/sources/list", Who::Invalid, b"", &params);
        assert_eq!(status(&r), "401 Unauthorized");
        assert!(r.body.contains("invalid_token"), "{}", r.body);
    }

    /// Every route reaches a handler (a stub answers 501 `not_built` until
    /// its builder lands it): never this module's 404, always a JSON body.
    #[test]
    fn every_route_is_wired_to_a_handler() {
        let _home = crate::test_support::TempHome::new();
        let _ = k2_core::db::init_for_tests();
        let params = HashMap::new();
        for p in ROUTES {
            let r = handle(p, Who::Owner, b"{}", &params);
            assert!(!r.body.contains("no such sources route"), "{p} is not wired: {}", r.body);
            let _: serde_json::Value =
                serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{p}: not JSON ({e}): {}", r.body));
        }
        let r = handle("/cli/sources/nope", Who::Owner, b"", &params);
        assert_eq!(status(&r), "404 Not Found");
    }
}

#[cfg(test)]
#[path = "sources_routes_tests.rs"]
mod b1_tests;
