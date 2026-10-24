//! RS-A15 `guests:rooms` (room-scope amendment 2026-10-18, 0.46.1): an App
//! sets and reads **its own guests'** rooms without the owner token.
//!
//! One rule set, three doors (each door only decides who the caller is and
//! which rooms it may hand out, the [`Ceiling`]):
//! - the App transport: `GET|POST /cli/skin/guests/rooms` with the App's own
//!   pass (its gateway-link pass, or its remote pass), only when the owner
//!   granted this App `guests-rooms` ([`grant`]); ceiling = the grant's rooms
//!   plus the App's own room;
//! - the Garden bridge: `guests.rooms` / `guests.setRooms` through
//!   `/cli/zen/call` on this computer's daemon, as the owner (any local
//!   App), every write behind a K2 confirm sheet outside the frame;
//! - the CLI: `k2 app guest rooms <app> <user> [set …]`, the owner (any
//!   room) or an agent of the App's workspace within the grant.
//!
//! The rules ([`read`], [`set`]), whoever calls:
//! - only a guest of **this App** (a principal holding a `kind=app` grant on
//!   it). Anyone else, a Connect user's name, a guest of another App and a
//!   name nobody has are the same [`Refusal::NotFound`] (no oracle);
//! - only rooms inside the ceiling ([`Refusal::OutsideApp`]); a handle
//!   that doesn't resolve is the same refusal (no workspace oracle);
//! - never above the guest's role: a role-less guest gets Thread-only caps
//!   in each room (its default role), and a guest whose App grant carries a
//!   role keeps that role's rooms ([`Refusal::AboveRole`] for a room the role
//!   doesn't cover, [`Refusal::HasRole`] for a narrower set);
//! - idempotent: the same set (any order) is a no-op, nothing is written;
//! - every change writes one hash-chained `deploy_audit` row
//!   (`guest_rooms`: who, which App, before, after);
//! - at most [`RATE_PER_MIN`] calls a minute per App ([`Refusal::RateLimited`]).
//!
//! Two reads beside them (prd-app-caps-0464-v1 AC1/AC2, catalog v22,
//! 0.46.4): [`app_rooms`] (`app.rooms`: the App's own room, its grant and
//! the ceiling [`set`] enforces, no grant needed to read it) and [`list`]
//! (`guests.list`: this App's own guests a page at a time, the same "guest
//! of this App" rule as [`read`], one page = one call against the rate).
//!
//! The per-App grant lives in skin.db (`app_owner_grants`, created at open:
//! additive, no numbered step, so an older K2 ignores it).

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::json;

use super::{
    app_grant, normalize_room_ids, normalize_username, now_secs, parse_rooms_json, principal_by_username,
    role_by_id_or_name, rooms_from_json, rooms_json, with_conn, HOST_APP_ID,
};

/// The catalog cap of both verbs (an owner grant per App, never a pass cap).
pub const CAP: &str = "guests:rooms";
/// The grant kind `k2 app grant <app> --kind guests-rooms` writes.
pub const GRANT_KIND: &str = "guests-rooms";
/// Calls a minute per App (reads and writes, every door).
pub const RATE_PER_MIN: usize = 60;
const RATE_WINDOW: Duration = Duration::from_secs(60);

/// `deploy_audit` events.
pub const AUDIT_CHANGE: &str = "guest_rooms";
pub const AUDIT_GRANT: &str = "guest_rooms_grant";
pub const AUDIT_REVOKE: &str = "guest_rooms_revoke";

/// skin.db DDL (run by the idempotent open block).
pub(super) fn ensure_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS app_owner_grants (
            service_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            rooms TEXT NOT NULL DEFAULT '[]',
            granted_by TEXT NOT NULL,
            granted_at INTEGER NOT NULL,
            PRIMARY KEY (service_id, kind)
         );",
    )
    .map_err(|e| format!("skin.db app_owner_grants: {e}"))
}

// ── the owner's per-App grant ────────────────────────────────────────

/// One App's `guests-rooms` grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppOwnerGrant {
    pub service_id: String,
    pub kind: String,
    /// Extra rooms (project ids) the App may hand its guests, beyond its own.
    pub rooms: Vec<String>,
    pub granted_by: String,
    pub granted_at: i64,
}

fn grant_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<AppOwnerGrant> {
    Ok(AppOwnerGrant {
        service_id: r.get(0)?,
        kind: r.get(1)?,
        rooms: rooms_from_json(&r.get::<_, String>(2)?),
        granted_by: r.get(3)?,
        granted_at: r.get(4)?,
    })
}

fn grant_in(conn: &Connection, service_id: &str) -> Result<Option<AppOwnerGrant>, String> {
    conn.query_row(
        "SELECT service_id, kind, rooms, granted_by, granted_at FROM app_owner_grants WHERE service_id = ?1 AND kind = ?2",
        params![service_id, GRANT_KIND],
        grant_row,
    )
    .optional()
    .map_err(|e| format!("app grant lookup: {e}"))
}

/// The App's `guests-rooms` grant, if the owner gave one.
pub fn get_grant(service_id: &str) -> Result<Option<AppOwnerGrant>, String> {
    with_conn(|conn| grant_in(conn, service_id.trim()))
}

/// Give (or re-give, replacing its rooms) the App `guests-rooms`. `rooms`
/// are project ids the caller resolved. Returns `(before, after)`.
pub fn grant(service_id: &str, rooms: &[String], by: &str) -> Result<(Option<AppOwnerGrant>, AppOwnerGrant), String> {
    let svc = service_id.trim();
    if svc.is_empty() || svc == HOST_APP_ID {
        return Err("name a published App (host is the box login, not an App)".into());
    }
    let rooms = normalize_room_ids(rooms);
    let r = with_conn(|conn| {
        let before = grant_in(conn, svc)?;
        conn.execute(
            "INSERT INTO app_owner_grants (service_id, kind, rooms, granted_by, granted_at) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(service_id, kind) DO UPDATE SET rooms = excluded.rooms, granted_by = excluded.granted_by,
               granted_at = excluded.granted_at",
            params![svc, GRANT_KIND, rooms_json(&rooms), by, now_secs()],
        )
        .map_err(|e| format!("app grant write: {e}"))?;
        let after = grant_in(conn, svc)?.ok_or_else(|| "app grant vanished".to_string())?;
        Ok((before, after))
    })?;
    audit(
        svc,
        AUDIT_GRANT,
        by,
        json!({ "kind": GRANT_KIND, "before": r.0.as_ref().map(|g| &g.rooms), "after": r.1.rooms }),
    );
    Ok(r)
}

/// Take the App's `guests-rooms` grant away. Returns what it was.
pub fn revoke(service_id: &str, by: &str) -> Result<Option<AppOwnerGrant>, String> {
    let svc = service_id.trim();
    let before = with_conn(|conn| {
        let before = grant_in(conn, svc)?;
        conn.execute("DELETE FROM app_owner_grants WHERE service_id = ?1 AND kind = ?2", params![svc, GRANT_KIND])
            .map_err(|e| format!("app grant delete: {e}"))?;
        Ok(before)
    })?;
    if let Some(g) = &before {
        audit(svc, AUDIT_REVOKE, by, json!({ "kind": GRANT_KIND, "before": g.rooms }));
    }
    Ok(before)
}

/// The rooms an App may hand its guests under its grant: the grant's rooms
/// plus the App's own room (its published service's workspace). `None`
/// when the owner hasn't granted it.
pub fn app_ceiling(service_id: &str) -> Result<Option<Vec<String>>, String> {
    let Some(g) = get_grant(service_id)? else { return Ok(None) };
    let own = {
        let db = crate::db::shared();
        let conn = db.lock();
        crate::published_services::get_by_id(&conn, service_id).map_err(|e| format!("app lookup: {e}"))?.map(|r| r.project_id)
    };
    let mut rooms = g.rooms;
    if let Some(own) = own {
        rooms.push(own);
    }
    Ok(Some(normalize_room_ids(&rooms)))
}

// ── the rules ────────────────────────────────────────────────────────

/// The rooms a caller may hand out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ceiling {
    /// The owner's rights (the Garden, the owner's CLI): any room on this box.
    AnyRoom,
    /// These project ids only.
    Rooms(Vec<String>),
}

/// Why a call was refused. The doors map each to a status and catalog code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Not a guest of this App (or no such name): `404 not_found`.
    NotFound,
    /// A missing or malformed argument: `400 usage`.
    Usage(String),
    /// Rooms outside the ceiling (or unknown handles): `403 outside_app_rooms`.
    OutsideApp(Vec<String>),
    /// Rooms the guest's role doesn't cover: `403 above_role`.
    AboveRole(Vec<String>),
    /// The guest's App grant carries a role; a narrower set needs a
    /// narrower role: `409 has_role`.
    HasRole(String),
    /// Over [`RATE_PER_MIN`]: `429 rate_limited`.
    RateLimited { retry_after_ms: u64 },
    /// Storage: `500 failed`.
    Store(String),
}

impl Refusal {
    pub fn status(&self) -> u16 {
        match self {
            Refusal::NotFound => 404,
            Refusal::Usage(_) => 400,
            Refusal::OutsideApp(_) | Refusal::AboveRole(_) => 403,
            Refusal::HasRole(_) => 409,
            Refusal::RateLimited { .. } => 429,
            Refusal::Store(_) => 500,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Refusal::NotFound => "not_found",
            Refusal::Usage(_) => "usage",
            Refusal::OutsideApp(_) => "outside_app_rooms",
            Refusal::AboveRole(_) => "above_role",
            Refusal::HasRole(_) => "has_role",
            Refusal::RateLimited { .. } => "rate_limited",
            Refusal::Store(_) => "failed",
        }
    }

    pub fn hint(&self) -> String {
        match self {
            Refusal::NotFound => "no such guest of this App".into(),
            Refusal::Usage(m) | Refusal::Store(m) => m.clone(),
            Refusal::OutsideApp(r) => format!(
                "not a room this App may hand out: {} (the owner widens it: k2 app grant <app> --kind guests-rooms --room <handle>)",
                r.join(", ")
            ),
            Refusal::AboveRole(r) => format!("the guest's role doesn't cover: {} (rooms never add caps beyond the role)", r.join(", ")),
            Refusal::HasRole(role) => format!(
                "the guest has the role '{role}' on this App, so its rooms are the role's; give a narrower role instead (k2 app grant)"
            ),
            Refusal::RateLimited { retry_after_ms } => {
                format!("at most {RATE_PER_MIN} guest-rooms calls a minute per App; retry after {retry_after_ms} ms")
            }
        }
    }
}

impl From<String> for Refusal {
    fn from(e: String) -> Self {
        Refusal::Store(e)
    }
}

/// One guest's rooms on one App (the catalog's `AppGuestRooms`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GuestRooms {
    pub ok: bool,
    pub app: String,
    pub username: String,
    /// Project ids.
    pub rooms: Vec<String>,
    pub room_handles: Vec<String>,
    /// The role on this App's grant (`None` = Thread-only rooms).
    pub role: Option<String>,
    pub enabled: bool,
    /// `true` only when a set wrote something.
    pub changed: bool,
}

/// What a set did (or would do): the audit's before and after.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetOutcome {
    pub view: GuestRooms,
    pub before: Vec<String>,
}

/// The guest as this App sees it: principal id, role name, rooms, enabled.
struct Guest {
    principal_id: String,
    username: String,
    role: Option<(String, Vec<String>)>,
    rooms: Vec<String>,
    enabled: bool,
}

fn guest_of(app: &str, username: &str) -> Result<Guest, Refusal> {
    let app = app.trim();
    if app.is_empty() || app == HOST_APP_ID {
        return Err(Refusal::NotFound);
    }
    let Ok(username) = normalize_username(username) else { return Err(Refusal::NotFound) };
    // A Connect login (the owner included) is never an App's guest, even
    // when a guest shares the name.
    if crate::connect_users::role_for_user(&username).is_some() {
        return Err(Refusal::NotFound);
    }
    with_conn(|conn| {
        let Some(p) = principal_by_username(conn, &username)? else { return Ok(None) };
        let Some(g) = app_grant(conn, &p.id, app)? else { return Ok(None) };
        let role = match g.role_id.as_deref() {
            Some(rid) => match role_by_id_or_name(conn, rid)? {
                Some(r) => Some((r.name, r.room_policy.keys().cloned().collect::<Vec<_>>())),
                None => return Err(format!("the guest's role '{rid}' is gone")),
            },
            None => None,
        };
        let rooms = match &role {
            Some((_, rooms)) => rooms.clone(),
            None => parse_rooms_json(g.scope.as_deref()),
        };
        Ok(Some(Guest { principal_id: p.id, username: p.username, role, rooms, enabled: g.enabled }))
    })?
    .ok_or(Refusal::NotFound)
}

fn view(app: &str, g: &Guest, rooms: Vec<String>, changed: bool) -> GuestRooms {
    GuestRooms {
        ok: true,
        app: app.trim().to_string(),
        username: g.username.clone(),
        room_handles: super::handles_for_project_ids(&rooms),
        rooms,
        role: g.role.as_ref().map(|(n, _)| n.clone()),
        enabled: g.enabled,
        changed,
    }
}

fn rate_buckets() -> &'static Mutex<HashMap<String, VecDeque<Instant>>> {
    static B: OnceLock<Mutex<HashMap<String, VecDeque<Instant>>>> = OnceLock::new();
    B.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Count one call for `app`; refuse past [`RATE_PER_MIN`] in the last minute.
pub fn rate_take(app: &str) -> Result<(), Refusal> {
    let now = Instant::now();
    let mut all = rate_buckets().lock().unwrap_or_else(|p| p.into_inner());
    let q = all.entry(app.trim().to_string()).or_default();
    while q.front().is_some_and(|t| now.duration_since(*t) >= RATE_WINDOW) {
        q.pop_front();
    }
    if q.len() >= RATE_PER_MIN {
        let oldest = *q.front().expect("a full window has a front");
        let wait = RATE_WINDOW.saturating_sub(now.duration_since(oldest));
        return Err(Refusal::RateLimited { retry_after_ms: wait.as_millis().max(1) as u64 });
    }
    q.push_back(now);
    Ok(())
}

/// Resolve room handles or ids one by one; the ones that don't resolve
/// come back separately (the caller refuses them as outside the ceiling).
pub fn resolve_rooms(tokens: &[String]) -> (Vec<String>, Vec<String>) {
    let (mut ids, mut unknown) = (Vec::new(), Vec::new());
    for t in tokens.iter().map(|t| t.trim()).filter(|t| !t.is_empty()) {
        match super::resolve_room_tokens(&[t.to_string()]) {
            Ok(mut v) if !v.is_empty() => {
                let id = v.remove(0);
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
            _ => unknown.push(t.to_string()),
        }
    }
    (ids, unknown)
}

/// `guests.rooms`: one guest of `app`, its rooms. Counts against the rate.
pub fn read(app: &str, username: &str) -> Result<GuestRooms, Refusal> {
    rate_take(app)?;
    let g = guest_of(app, username)?;
    let rooms = g.rooms.clone();
    Ok(view(app, &g, rooms, false))
}

// ── app.rooms / guests.list (prd-app-caps-0464-v1 AC1/AC2) ───────────

/// `guests.list`: default and largest page.
pub const LIST_DEFAULT: usize = 100;
pub const LIST_MAX: usize = 500;

/// One room as the catalog's `AppRoomRef` / `AppOwnRoom`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomRef {
    pub room: String,
    pub handle: String,
    pub display_name: String,
}

/// `app.rooms` (the catalog's `AppRooms`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppRooms {
    pub ok: bool,
    pub app: String,
    /// The App's own workspace room; `None` when it no longer resolves.
    pub own_room: Option<RoomRef>,
    pub granted: bool,
    /// Seconds since the epoch; `None` without the grant.
    pub granted_at: Option<i64>,
    /// The rooms the grant adds (live ones only); `[]` without the grant.
    pub grant_rooms: Vec<RoomRef>,
    /// [`app_ceiling`], live rooms only; `[]` without the grant.
    pub ceiling: Vec<String>,
}

fn room_refs(ids: &[String]) -> Vec<RoomRef> {
    super::live_agents(ids)
        .into_iter()
        .map(|a| RoomRef { room: a.project_id, handle: a.handle, display_name: a.display_name })
        .collect()
}

/// `app.rooms`: what this App may hand its guests. Rooms that no longer
/// resolve are left out (never an error, never an oracle). `ceiling` is
/// [`app_ceiling`]'s list, so it is the one [`set`] enforces.
pub fn app_rooms(service_id: &str) -> Result<AppRooms, String> {
    let svc = service_id.trim();
    let own = {
        let db = crate::db::shared();
        let conn = db.lock();
        crate::published_services::get_by_id(&conn, svc).map_err(|e| format!("app lookup: {e}"))?.map(|r| r.project_id)
    };
    let own_room = own.as_ref().and_then(|id| room_refs(std::slice::from_ref(id)).into_iter().next());
    let grant = get_grant(svc)?;
    let (granted_at, grant_rooms, ceiling) = match &grant {
        Some(g) => {
            let ceiling: Vec<String> = app_ceiling(svc)?.unwrap_or_default();
            let live: Vec<String> = room_refs(&ceiling).into_iter().map(|r| r.room).collect();
            let extra: Vec<String> = g.rooms.iter().filter(|r| own.as_deref() != Some(r.as_str())).cloned().collect();
            (Some(g.granted_at), room_refs(&extra), ceiling.into_iter().filter(|r| live.contains(r)).collect())
        }
        None => (None, Vec::new(), Vec::new()),
    };
    Ok(AppRooms { ok: true, app: svc.to_string(), own_room, granted: grant.is_some(), granted_at, grant_rooms, ceiling })
}

/// One row of `guests.list` (the catalog's `AppGuestRow`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GuestRow {
    pub username: String,
    /// Project ids on this App only.
    pub rooms: Vec<String>,
    pub room_handles: Vec<String>,
    pub role: Option<String>,
    pub enabled: bool,
    /// A marked agent login (`k2 app user agent`), not a person.
    pub agent: bool,
}

/// One page of `guests.list` (the catalog's `AppGuestList`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GuestPage {
    pub ok: bool,
    pub app: String,
    pub guests: Vec<GuestRow>,
    /// The last username of this page when more may follow; `None` on the
    /// last page.
    pub next_cursor: Option<String>,
}

/// Usernames of principals holding a `kind=app` grant on `app`, after
/// `after`, by username (the candidates; [`guest_of`] decides).
fn candidates(app: &str, after: &str, n: usize) -> Result<Vec<String>, String> {
    with_conn(|conn| {
        let mut st = conn
            .prepare(
                "SELECT p.username FROM grants g JOIN principals p ON p.id = g.subject_id
                 WHERE g.subject_kind = 'principal' AND g.kind = 'app' AND g.target_id = ?1 AND p.username > ?2
                 ORDER BY p.username LIMIT ?3",
            )
            .map_err(|e| format!("guest list: {e}"))?;
        let rows = st
            .query_map(params![app, after, n as i64], |r| r.get::<_, String>(0))
            .map_err(|e| format!("guest list: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("guest list: {e}"))
    })
}

/// `guests.list`: this App's own guests by username, `limit` a page
/// (default [`LIST_DEFAULT`], at most [`LIST_MAX`]), after `cursor` (the
/// previous page's last username). The owner, Connect logins (even when a
/// guest shares the name) and other Apps' guests are never listed: every
/// row passes [`guest_of`], the rule [`read`] uses. Counts one call against
/// the rate.
pub fn list(app: &str, cursor: &str, limit: Option<usize>) -> Result<GuestPage, Refusal> {
    let app = app.trim();
    if app.is_empty() || app == HOST_APP_ID {
        return Err(Refusal::NotFound);
    }
    let limit = match limit {
        None => LIST_DEFAULT,
        Some(n) if (1..=LIST_MAX).contains(&n) => n,
        Some(_) => return Err(Refusal::Usage(format!("limit is 1..={LIST_MAX}"))),
    };
    rate_take(app)?;
    let mut guests: Vec<GuestRow> = Vec::new();
    let mut after = cursor.trim().to_string();
    let mut more = false;
    'pages: loop {
        let batch = candidates(app, &after, limit + 1)?;
        let full = batch.len() == limit + 1;
        for u in batch {
            if guests.len() == limit {
                more = true;
                break 'pages;
            }
            after = u.clone();
            let g = match guest_of(app, &u) {
                Ok(g) => g,
                Err(Refusal::NotFound) => continue,
                Err(e) => return Err(e),
            };
            let agent = super::principal_agent_workspace(&g.principal_id)?.is_some();
            guests.push(GuestRow {
                room_handles: super::handles_for_project_ids(&g.rooms),
                rooms: g.rooms,
                role: g.role.map(|(n, _)| n),
                enabled: g.enabled,
                agent,
                username: g.username,
            });
        }
        if !full {
            break;
        }
    }
    let next_cursor = if more { guests.last().map(|g| g.username.clone()) } else { None };
    Ok(GuestPage { ok: true, app: app.to_string(), guests, next_cursor })
}

/// Check a set without writing: the rooms resolved, the outcome it would
/// have. `Ok(None)` = valid and a no-op. (The Garden door shows this change
/// on its confirm sheet before anything is written.)
pub fn plan(app: &str, username: &str, room_tokens: &[String], ceiling: &Ceiling) -> Result<(SetOutcome, Vec<String>), Refusal> {
    let g = guest_of(app, username)?;
    let (ids, unknown) = resolve_rooms(room_tokens);
    let mut outside: Vec<String> = unknown;
    if let Ceiling::Rooms(allowed) = ceiling {
        outside.extend(ids.iter().filter(|id| !allowed.contains(id)).map(|id| handle_or_id(id)));
    }
    if !outside.is_empty() {
        return Err(Refusal::OutsideApp(outside));
    }
    let want: BTreeSet<&String> = ids.iter().collect();
    let have: BTreeSet<&String> = g.rooms.iter().collect();
    if let Some((role, role_rooms)) = &g.role {
        let above: Vec<String> = ids.iter().filter(|id| !role_rooms.contains(id)).map(|id| handle_or_id(id)).collect();
        if !above.is_empty() {
            return Err(Refusal::AboveRole(above));
        }
        if want != have {
            return Err(Refusal::HasRole(role.clone()));
        }
    }
    let before = g.rooms.clone();
    let changed = want != have;
    Ok((SetOutcome { view: view(app, &g, if changed { ids.clone() } else { before.clone() }, changed), before }, ids))
}

fn handle_or_id(id: &str) -> String {
    super::handles_for_project_ids(&[id.to_string()]).into_iter().next().unwrap_or_else(|| id.to_string())
}

/// `guests.setRooms`: set one guest's rooms on `app` to `room_tokens`
/// (handles or ids; `[]` takes every room away), within `ceiling`. Live
/// sessions of that guest on that App follow at once. `actor` and `via`
/// (`app` | `garden` | `cli`) go in the audit row.
pub fn set(app: &str, username: &str, room_tokens: &[String], ceiling: &Ceiling, actor: &str, via: &str) -> Result<SetOutcome, Refusal> {
    rate_take(app)?;
    let (out, ids) = plan(app, username, room_tokens, ceiling)?;
    if !out.view.changed {
        return Ok(out);
    }
    super::set_app_grant_rooms(&out.view.username, app.trim(), &ids, true, actor).map_err(Refusal::Store)?;
    audit(
        app.trim(),
        AUDIT_CHANGE,
        actor,
        json!({ "username": out.view.username, "before": out.before, "after": ids, "via": via }),
    );
    Ok(out)
}

// ── audit ────────────────────────────────────────────────────────────

/// The `deploy_audit` subject id for a service: its runtime App id when it
/// is one, else the service id.
pub fn audit_subject(service_id: &str) -> String {
    let db = crate::db::shared();
    let conn = db.lock();
    crate::deploy::store::get_app_by_service(&conn, service_id)
        .ok()
        .flatten()
        .map(|a| a.app_id)
        .unwrap_or_else(|| service_id.to_string())
}

fn audit(service_id: &str, event: &str, actor: &str, mut payload: serde_json::Value) {
    payload["service"] = json!(service_id);
    let subject = audit_subject(service_id);
    let db = crate::db::shared();
    let conn = db.lock();
    if let Err(e) = crate::deploy::audit::append(&conn, crate::deploy::audit::SUBJECT_APP, &subject, event, actor, None, &payload) {
        crate::log_debug!("[skin] guest rooms audit {service_id} {event}: {e}");
    }
}

/// The App's guest-rooms audit rows, newest first (tests, `k2 app history`).
pub fn audit_rows(service_id: &str, limit: usize) -> Result<Vec<crate::deploy::audit::AuditRow>, String> {
    let subject = audit_subject(service_id);
    let db = crate::db::shared();
    let conn = db.lock();
    let rows = crate::deploy::audit::list_for(&conn, crate::deploy::audit::SUBJECT_APP, &subject, limit.max(1) * 4)
        .map_err(|e| format!("audit read: {e}"))?;
    Ok(rows
        .into_iter()
        .filter(|r| [AUDIT_CHANGE, AUDIT_GRANT, AUDIT_REVOKE].contains(&r.event.as_str()))
        .filter(|r| r.payload.get("service").and_then(|s| s.as_str()) == Some(service_id))
        .take(limit)
        .collect())
}

#[cfg(test)]
#[path = "skin_guest_rooms_tests.rs"]
mod tests;
