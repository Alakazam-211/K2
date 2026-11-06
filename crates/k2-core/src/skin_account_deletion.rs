//! App account deletion (0.46.9, feature `app-account-deletion-v1`).
//!
//! App Store rule 5.1.1(v): an App that lets people make an account must
//! let them delete it from inside the App. Google Play also wants a web
//! link for people who no longer have the App. A person signed in to an
//! App asks with their own session pass (`profile.deleteAccount`, the
//! daemon's `app_account_deletion`); this module is the store and the
//! steps.
//!
//! Each App picks one of two modes (`k2 app set <app> --account-deletion`):
//! - **request** (the default): K2 records the request, signs the person
//!   out of this App everywhere (browsers, devices, push, pairing codes)
//!   and turns their access to this App off. The owner completes it, or
//!   the grace period does (30 days by default, 1 to 90).
//! - **immediate**: the deletion runs at once.
//!
//! What a deletion removes depends on the person's **scope**:
//! - `account`: this App is all they use on this server (their only
//!   grants are this App's own). The login itself goes, with everything
//!   that hangs on it (sessions, devices, push, pairings, profile,
//!   picture, email, read state, wallet, chat sessions).
//! - `app`: the same login also opens another App or the server itself.
//!   Only this App's access goes (its grants, its sessions, devices,
//!   push and pairings); the login stays for the rest.
//!
//! Tables (skin.db, created at open, no numbered step):
//! `app_account_deletion_settings` (one row per App that changed the
//! defaults) and `app_account_deletions` (one row per pending request;
//! the row goes when the request completes or is cancelled, and the
//! App's History keeps the record). A request row has no foreign key to
//! `principals`: it outlives an owner removing the person by other means,
//! and the sweep then just drops it.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use super::{now_secs, with_conn, HOST_APP_ID};

pub const MODE_REQUEST: &str = "request";
pub const MODE_IMMEDIATE: &str = "immediate";
/// The default mode (Rosson to confirm).
pub const DEFAULT_MODE: &str = MODE_REQUEST;
/// Days a request waits before K2 completes it on its own.
pub const DEFAULT_GRACE_DAYS: i64 = 30;
pub const MIN_GRACE_DAYS: i64 = 1;
pub const MAX_GRACE_DAYS: i64 = 90;
/// A request id's prefix (not a secret).
pub const ID_PREFIX: &str = "adr_";

/// What a deletion removes or keeps: stable item names an App may show
/// (or translate).
pub mod item {
    pub const LOGIN: &str = "login";
    pub const APP_ACCESS: &str = "app_access";
    pub const SESSIONS: &str = "sessions";
    pub const DEVICES: &str = "devices";
    pub const PUSH_DEVICES: &str = "push_devices";
    pub const PAIRINGS: &str = "pairings";
    pub const PROFILE: &str = "profile";
    pub const EMAIL: &str = "email";
    pub const READ_STATE: &str = "read_state";
    pub const WALLET: &str = "wallet";
    pub const CHAT_SESSIONS: &str = "chat_sessions";
    pub const CLIENT_LOGS: &str = "client_logs";
    pub const THREAD_POSTS: &str = "thread_posts";
    pub const ROOM_FILES: &str = "room_files";
    pub const CHAT_MESSAGES: &str = "chat_messages";
    pub const APP_DATA: &str = "app_data";
    pub const HISTORY: &str = "history";
}

/// One App's settings (the defaults when nobody set any).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub mode: String,
    pub grace_days: i64,
    /// `false`: nobody changed this App's settings (the defaults apply).
    pub set: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { mode: DEFAULT_MODE.to_string(), grace_days: DEFAULT_GRACE_DAYS, set: false }
    }
}

/// One pending request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub id: String,
    pub app: String,
    pub principal_id: String,
    pub username: String,
    pub requested_at: i64,
    pub due_at: i64,
    /// `app` (the person, from inside the App).
    pub via: String,
    /// The grants K2 turned off at the request (turned back on by a cancel).
    #[serde(skip)]
    pub grant_ids: Vec<String>,
}

/// skin.db DDL (run by the idempotent open block).
pub(super) fn ensure_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS app_account_deletion_settings (
            app_id TEXT PRIMARY KEY,
            mode TEXT NOT NULL CHECK (mode IN ('request', 'immediate')),
            grace_days INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            updated_by TEXT
         );
         CREATE TABLE IF NOT EXISTS app_account_deletions (
            id TEXT PRIMARY KEY,
            app_id TEXT NOT NULL,
            principal_id TEXT NOT NULL,
            username TEXT NOT NULL,
            requested_at INTEGER NOT NULL,
            due_at INTEGER NOT NULL,
            via TEXT NOT NULL,
            grant_ids TEXT NOT NULL DEFAULT '[]',
            UNIQUE (app_id, principal_id)
         );
         CREATE INDEX IF NOT EXISTS app_account_deletions_due ON app_account_deletions(due_at);",
    )
    .map_err(|e| format!("skin.db account deletion: {e}"))
}

fn app_id(app: &str) -> Result<String, String> {
    let a = super::normalize_app_target(Some(app));
    if a.is_empty() || a == HOST_APP_ID {
        return Err("account deletion is for a published App's own people, never the box login (host)".to_string());
    }
    Ok(a)
}

// ── settings ─────────────────────────────────────────────────────────

/// One App's settings.
pub fn settings(app: &str) -> Result<Settings, String> {
    let app = app_id(app)?;
    with_conn(|conn| {
        let row: Option<(String, i64)> = conn
            .query_row("SELECT mode, grace_days FROM app_account_deletion_settings WHERE app_id = ?1", params![app], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()
            .map_err(|e| format!("account deletion settings: {e}"))?;
        Ok(match row {
            Some((mode, grace_days)) => Settings { mode, grace_days, set: true },
            None => Settings::default(),
        })
    })
}

/// Parse a mode word (`request` / `immediate`).
pub fn parse_mode(raw: &str) -> Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "request" => Some(MODE_REQUEST),
        "immediate" => Some(MODE_IMMEDIATE),
        _ => None,
    }
}

/// Change one App's mode and/or grace period. `None` keeps a field.
pub fn set_settings(app: &str, mode: Option<&str>, grace_days: Option<i64>, actor: &str) -> Result<Settings, String> {
    let app = app_id(app)?;
    let mode = match mode {
        Some(m) => Some(parse_mode(m).ok_or_else(|| format!("mode must be {MODE_REQUEST} or {MODE_IMMEDIATE}"))?),
        None => None,
    };
    if let Some(d) = grace_days {
        if !(MIN_GRACE_DAYS..=MAX_GRACE_DAYS).contains(&d) {
            return Err(format!("the grace period is {MIN_GRACE_DAYS} to {MAX_GRACE_DAYS} days"));
        }
    }
    let cur = settings(&app)?;
    let next_mode = mode.map(str::to_string).unwrap_or(cur.mode);
    let next_days = grace_days.unwrap_or(cur.grace_days);
    with_conn(|conn| {
        conn.execute(
            "INSERT INTO app_account_deletion_settings (app_id, mode, grace_days, updated_at, updated_by)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(app_id) DO UPDATE SET mode = ?2, grace_days = ?3, updated_at = ?4, updated_by = ?5",
            params![app, next_mode, next_days, now_secs(), actor],
        )
        .map_err(|e| format!("account deletion settings write: {e}"))?;
        Ok(())
    })?;
    Ok(Settings { mode: next_mode, grace_days: next_days, set: true })
}

// ── requests ─────────────────────────────────────────────────────────

const REQUEST_SELECT: &str = "SELECT id, app_id, principal_id, username, requested_at, due_at, via, grant_ids FROM app_account_deletions";

fn map_request(r: &rusqlite::Row<'_>) -> rusqlite::Result<Request> {
    let ids: String = r.get(7)?;
    Ok(Request {
        id: r.get(0)?,
        app: r.get(1)?,
        principal_id: r.get(2)?,
        username: r.get(3)?,
        requested_at: r.get(4)?,
        due_at: r.get(5)?,
        via: r.get(6)?,
        grant_ids: serde_json::from_str(&ids).unwrap_or_default(),
    })
}

fn requests_where(sql_where: &str, args: &[&dyn rusqlite::ToSql]) -> Result<Vec<Request>, String> {
    with_conn(|conn| {
        let mut st = conn.prepare(&format!("{REQUEST_SELECT} WHERE {sql_where} ORDER BY due_at, id")).map_err(|e| format!("account deletions: {e}"))?;
        let rows = st.query_map(args, map_request).map_err(|e| format!("account deletions: {e}"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| format!("account deletions: {e}"))
    })
}

/// One App's pending requests, the soonest due first.
pub fn pending(app: &str) -> Result<Vec<Request>, String> {
    let app = app_id(app)?;
    requests_where("app_id = ?1", &[&app])
}

/// Every pending request due at `now` or before.
pub fn due(now: i64) -> Result<Vec<Request>, String> {
    requests_where("due_at <= ?1", &[&now])
}

/// A pending request of `app` by its id (`adr_…`) or the person's username.
pub fn find(app: &str, id_or_username: &str) -> Result<Option<Request>, String> {
    let app = app_id(app)?;
    let key = id_or_username.trim();
    if key.is_empty() {
        return Ok(None);
    }
    let rows = requests_where("app_id = ?1 AND (id = ?2 OR username = ?2 COLLATE NOCASE)", &[&app, &key])?;
    Ok(rows.into_iter().next())
}

/// The pending request of one person in one App.
pub fn pending_for(app: &str, principal_id: &str) -> Result<Option<Request>, String> {
    let app = app_id(app)?;
    Ok(requests_where("app_id = ?1 AND principal_id = ?2", &[&app, &principal_id])?.into_iter().next())
}

fn new_id() -> String {
    format!("{ID_PREFIX}{}", &uuid::Uuid::new_v4().simple().to_string()[..20])
}

/// Drop a request row (completed or cancelled). `false` when it was gone.
pub fn forget(id: &str) -> Result<bool, String> {
    with_conn(|conn| {
        conn.execute("DELETE FROM app_account_deletions WHERE id = ?1", params![id])
            .map(|n| n > 0)
            .map_err(|e| format!("account deletion forget: {e}"))
    })
}

// ── scope ────────────────────────────────────────────────────────────

/// What a deletion of this person in this App reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// This App is all the login opens: the login goes.
    Account,
    /// The login opens something else too: only this App's access goes.
    App,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Account => "account",
            Scope::App => "app",
        }
    }
}

/// Is this grant one of `app`'s own: its `kind=app` grant, or a grant the
/// App itself made (an App-made chat grant names the App in `created_by`).
fn of_app(g: &super::SkinGrant, app: &str) -> bool {
    (g.kind == "app" && g.target_id == app) || g.created_by.as_deref() == Some(super::app_chat::app_marker(app).as_str())
}

/// This person's grants that belong to `app`.
pub fn app_grants(app: &str, principal_id: &str) -> Result<Vec<super::SkinGrant>, String> {
    let app = app_id(app)?;
    Ok(super::list_grants()?
        .into_iter()
        .filter(|g| g.subject_kind == "principal" && g.subject_id == principal_id && of_app(g, &app))
        .collect())
}

/// The scope of deleting `principal_id` in `app`.
pub fn scope(app: &str, principal_id: &str) -> Result<Scope, String> {
    let app = app_id(app)?;
    let all: Vec<super::SkinGrant> =
        super::list_grants()?.into_iter().filter(|g| g.subject_kind == "principal" && g.subject_id == principal_id).collect();
    let only_this = all.iter().all(|g| of_app(g, &app));
    Ok(if only_this { Scope::Account } else { Scope::App })
}

// ── the steps ────────────────────────────────────────────────────────

/// Sign `principal_id` out of `app` everywhere: every browser session, every
/// device sign-in and its push rows (Sew ends the chat sessions minted from
/// them). Returns how many ended.
pub fn sign_out_of_app(app: &str, principal_id: &str) -> Result<usize, String> {
    let app = app_id(app)?;
    with_conn(|conn| {
        let tx = super::immediate_tx(conn)?;
        let n = super::revoke_sessions_for_principal_except(&tx, principal_id, "", Some(&app))?;
        tx.commit().map_err(|e| format!("account deletion sign-out commit: {e}"))?;
        Ok(n)
    })
}

/// Record a request (mode `request`): the row first, then this App's
/// grants turned off, then every session of this App ended. A second ask
/// while one is pending answers the pending one (nothing changes).
/// Returns `(request, created)`.
pub fn request(app: &str, principal_id: &str, username: &str, grace_days: i64, via: &str, actor: &str) -> Result<(Request, bool), String> {
    let app = app_id(app)?;
    if let Some(r) = pending_for(&app, principal_id)? {
        return Ok((r, false));
    }
    let grants = app_grants(&app, principal_id)?;
    let ids: Vec<String> = grants.iter().filter(|g| g.enabled).map(|g| g.id.clone()).collect();
    let now = now_secs();
    let r = Request {
        id: new_id(),
        app: app.clone(),
        principal_id: principal_id.to_string(),
        username: username.to_string(),
        requested_at: now,
        due_at: now + grace_days.clamp(MIN_GRACE_DAYS, MAX_GRACE_DAYS) * 86_400,
        via: via.to_string(),
        grant_ids: ids.clone(),
    };
    let ids_json = serde_json::to_string(&ids).map_err(|e| e.to_string())?;
    with_conn(|conn| {
        conn.execute(
            "INSERT INTO app_account_deletions (id, app_id, principal_id, username, requested_at, due_at, via, grant_ids)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![r.id, r.app, r.principal_id, r.username, r.requested_at, r.due_at, r.via, ids_json],
        )
        .map_err(|e| format!("account deletion request: {e}"))?;
        Ok(())
    })?;
    for id in &ids {
        super::set_grant_enabled_as(id, false, actor)?;
    }
    sign_out_of_app(&app, principal_id)?;
    Ok((r, true))
}

/// Undo a pending request: this App's grants K2 turned off come back on
/// (the ones that still exist), and the row goes. `None`: no such request.
pub fn cancel(app: &str, id_or_username: &str, actor: &str) -> Result<Option<Request>, String> {
    let Some(r) = find(app, id_or_username)? else { return Ok(None) };
    let live: Vec<String> = super::list_grants()?.into_iter().map(|g| g.id).collect();
    for id in r.grant_ids.iter().filter(|id| live.contains(id)) {
        super::set_grant_enabled_as(id, true, actor)?;
    }
    forget(&r.id)?;
    Ok(Some(r))
}

/// What one deletion did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deleted {
    pub scope: Scope,
    /// `false`: the person was already gone (an owner removed them).
    pub found: bool,
    /// Item names removed now (see [`item`]).
    pub done: Vec<&'static str>,
}

/// Delete `principal_id` from `app` now (immediate mode, a completed
/// request, the grace sweep). Scope `account` removes the login with
/// everything that hangs on it; scope `app` signs them out of this App
/// and removes this App's grants. Any pending request row of theirs in
/// this App goes too. The daemon adds what lives outside skin.db (read
/// state, client logs, the audit and the notes).
pub fn delete_now(app: &str, principal_id: &str, actor: &str) -> Result<Deleted, String> {
    use item::*;
    let app = app_id(app)?;
    let pending = pending_for(&app, principal_id)?;
    let username: Option<String> = with_conn(|conn| {
        conn.query_row("SELECT username FROM principals WHERE id = ?1", params![principal_id], |r| r.get(0))
            .optional()
            .map_err(|e| format!("account deletion person: {e}"))
    })?;
    let Some(username) = username else {
        if let Some(r) = pending {
            forget(&r.id)?;
        }
        return Ok(Deleted { scope: Scope::App, found: false, done: vec![] });
    };
    let scope = scope(&app, principal_id)?;
    let done = match scope {
        Scope::Account => {
            super::remove_principal_as(&username, actor)?;
            vec![LOGIN, APP_ACCESS, SESSIONS, DEVICES, PUSH_DEVICES, PAIRINGS, PROFILE, EMAIL, CHAT_SESSIONS]
        }
        Scope::App => {
            sign_out_of_app(&app, principal_id)?;
            for g in app_grants(&app, principal_id)? {
                super::delete_grant_by_id_with_cascade(&g.id, actor)?;
            }
            vec![APP_ACCESS, SESSIONS, DEVICES, PUSH_DEVICES, PAIRINGS]
        }
    };
    if let Some(r) = pending {
        forget(&r.id)?;
    }
    Ok(Deleted { scope, found: true, done })
}

/// Test seam: make the device sign-in behind session `token_id` look
/// `secs` older (the recent sign-in rule reads it, not the pass).
#[cfg(any(test, feature = "test-util"))]
pub fn age_device_sign_in_for_test(token_id: &str, secs: i64) {
    with_conn(|conn| {
        conn.execute("UPDATE device_logins SET created_at = created_at - ?2 WHERE session_token_id = ?1", params![token_id, secs])
            .map_err(|e| e.to_string())
    })
    .expect("age the device sign-in");
}

/// Test seam: make a pending request due now.
#[cfg(any(test, feature = "test-util"))]
pub fn make_due_for_test(id: &str) {
    with_conn(|conn| {
        conn.execute("UPDATE app_account_deletions SET due_at = ?2 WHERE id = ?1", params![id, now_secs() - 1]).map_err(|e| e.to_string())
    })
    .expect("make the request due");
}

/// What K2 keeps after a deletion, and why (an App shows it as is).
pub fn kept(scope: Scope) -> Vec<(&'static str, &'static str)> {
    use item::*;
    let mut out = vec![
        (THREAD_POSTS, "Messages and files they sent to agents in Threads stay in those conversations: they belong to the App's owner."),
        (ROOM_FILES, "Files they uploaded into a room's folder stay there: they belong to that room."),
        (
            CHAT_MESSAGES,
            "Chat (Sew), if they used it: K2 ends their chat sessions and Sew deactivates them, but Sew keeps their messages, files and reactions (it has no erase yet).",
        ),
        (APP_DATA, "Anything the App keeps in its own database or files: the App deletes that itself (K2 can't see inside it)."),
        (HISTORY, "The App's History keeps one line that this account was deleted, with its username: the owner's record."),
    ];
    if scope == Scope::App {
        out.insert(
            0,
            (LOGIN, "Their login stays, with its name, picture and email, because another App on this server or the server itself also uses it. Only this App's access went."),
        );
    }
    out
}

/// What a pending request still removes when it completes, by scope.
pub fn pending_items(scope: Scope) -> Vec<&'static str> {
    use item::*;
    match scope {
        Scope::Account => vec![LOGIN, APP_ACCESS, PROFILE, EMAIL, READ_STATE, WALLET, CLIENT_LOGS],
        Scope::App => vec![APP_ACCESS, CLIENT_LOGS],
    }
}
