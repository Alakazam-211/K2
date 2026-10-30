//! App-governed Spaces S1 (0.46.7, `prd-app-governed-spaces-k2-v1` §2):
//! an App's **chat key**.
//!
//! The chat key is the owner's grant to one App: "K2 may sign chat passes
//! for this App's users, up to this ceiling". It holds **no key material**:
//! K2 signs every pass with its own ES256 keyring in the vault (one per
//! box, `engine:chat-pass`), and Sew trusts K2's JWKS. Nothing about the
//! key ever reaches the App, its secrets dir or its environment. (Not the
//! App-scoped `k2sk_` key of `app_grace::keys`: a different thing.)
//!
//! The record (skin.db `app_chat_keys`, idempotent DDL at open, no numbered
//! step, so an older K2 ignores it):
//!
//! | Column | Meaning |
//! |---|---|
//! | `service_id` | the App (never `host`, never the home client) |
//! | `enabled` | 0/1: a revoke keeps the row, off |
//! | `caps` | JSON list, subset of the chat caps (default: the App's approved manifest chat caps) |
//! | `max_level` | `member` \| `moderator` (default) \| `admin` |
//! | `uploads` | 0/1, default 1 |
//! | `max_members` | people this App may give chat through the key (default 500, at most 10,000) |
//! | `granted_by`, `granted_at`, `revoked_at` | audit |
//!
//! What a change does, in one skin.db transaction:
//! - [`set`] writes the record and merges `max_level` and `uploads` into the
//!   App's Sew settings row (`tool_sew_settings`, subject `app:<id>`); the
//!   sweep sends them (`POST /mgmt/v1/apps/{app_id}/settings`, Sew I7) once
//!   Sew reports `app_governed_spaces`.
//! - [`revoke`] switches the key off at once (no more passes through it)
//!   and queues `POST /mgmt/v1/sessions/revoke {app_id}` (Sew I8), which
//!   ends every Sew session minted for this App. Never on a rotation: K2's
//!   signing keys rotate without ending sessions.
//!
//! People: for a keyed App, a K2 App user needs no chat grant of their own.
//! On their first pass K2 writes one ([`marker`] as `created_by`), within
//! `max_members` ([`members_made`]); see
//! [`crate::tool_hook::keyed_chat_gate`]. A grant made by a key counts for
//! a pass only while that key is on ([`grant_counts_in`]).
//!
//! Who may change a key (owner in person, Admins, the App's own agent for
//! narrowing) is the daemon's route; this module only stores.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::json;

use super::{now_secs, with_conn, CHAT_CAPS, HOST_APP_ID};

/// The grant kind `k2 app grant <app> --kind chat-key` names.
pub const KIND: &str = "chat-key";
/// `grants.created_by` of a chat grant a key made: `chat-key:<service id>`.
pub const CREATED_BY_PREFIX: &str = "chat-key:";
/// People a key may give chat when the owner names no ceiling.
pub const DEFAULT_MAX_MEMBERS: u32 = 500;
/// The largest ceiling.
pub const MAX_MAX_MEMBERS: u32 = 10_000;

/// `deploy_audit` events.
pub const AUDIT_SET: &str = "chat_key_set";
pub const AUDIT_REVOKE: &str = "chat_key_revoke";

/// The highest level the App may give a person in its governed Spaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Member,
    Moderator,
    Admin,
}

impl Level {
    pub const DEFAULT: Level = Level::Moderator;

    pub fn as_str(self) -> &'static str {
        match self {
            Level::Member => "member",
            Level::Moderator => "moderator",
            Level::Admin => "admin",
        }
    }

    pub fn parse(raw: &str) -> Option<Level> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "member" => Some(Level::Member),
            "moderator" => Some(Level::Moderator),
            "admin" => Some(Level::Admin),
            _ => None,
        }
    }
}

/// One App's chat key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatKey {
    pub service_id: String,
    pub kind: &'static str,
    pub enabled: bool,
    pub caps: Vec<String>,
    pub max_level: Level,
    pub uploads: bool,
    pub max_members: u32,
    pub granted_by: String,
    pub granted_at: i64,
    pub revoked_at: Option<i64>,
}

/// What a set asks for (the route fills the defaults first).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Want {
    pub caps: Vec<String>,
    pub max_level: Level,
    pub uploads: bool,
    pub max_members: u32,
}

/// skin.db DDL (run by the idempotent open block).
pub(super) fn ensure_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS app_chat_keys (
            service_id  TEXT PRIMARY KEY NOT NULL,
            enabled     INTEGER NOT NULL,
            caps        TEXT NOT NULL,
            max_level   TEXT NOT NULL,
            uploads     INTEGER NOT NULL,
            max_members INTEGER NOT NULL,
            granted_by  TEXT NOT NULL,
            granted_at  INTEGER NOT NULL,
            revoked_at  INTEGER
         );",
    )
    .map_err(|e| format!("skin.db app_chat_keys: {e}"))
}

/// The `created_by` of a chat grant this App's key made.
pub fn marker(service_id: &str) -> String {
    format!("{CREATED_BY_PREFIX}{}", service_id.trim())
}

/// Chat caps in their canonical order, de-duplicated. `Err` names the
/// first one that isn't a chat cap.
pub fn normalize_caps(raw: &[String]) -> Result<Vec<String>, String> {
    if let Some(bad) = raw.iter().map(|c| c.trim()).find(|c| !CHAT_CAPS.contains(c)) {
        return Err(format!("'{bad}' is not a chat cap (caps: {})", CHAT_CAPS.join(" ")));
    }
    Ok(CHAT_CAPS.iter().filter(|c| raw.iter().any(|r| r.trim() == **c)).map(|c| c.to_string()).collect())
}

/// Check a wanted key: at least one chat cap, a ceiling in range.
pub fn validate(w: &Want) -> Result<(), String> {
    let caps = normalize_caps(&w.caps)?;
    if caps.is_empty() {
        return Err(format!("a chat key needs at least one chat cap ({})", CHAT_CAPS.join(" ")));
    }
    if !(1..=MAX_MAX_MEMBERS).contains(&w.max_members) {
        return Err(format!("max is 1..={MAX_MAX_MEMBERS}"));
    }
    Ok(())
}

/// Does `w` give the App more than `before` does? Turning a key on (or
/// back on) always does; otherwise any cap it lacked, a higher level, a
/// higher ceiling, or uploads switched on.
pub fn widens(before: Option<&ChatKey>, w: &Want) -> bool {
    let Some(b) = before.filter(|b| b.enabled) else { return true };
    w.caps.iter().any(|c| !b.caps.contains(c)) || w.max_level > b.max_level || w.max_members > b.max_members || (w.uploads && !b.uploads)
}

fn key_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ChatKey> {
    let caps: String = r.get(2)?;
    let level: String = r.get(3)?;
    Ok(ChatKey {
        service_id: r.get(0)?,
        kind: KIND,
        enabled: r.get::<_, i64>(1)? != 0,
        caps: serde_json::from_str::<Vec<String>>(&caps).unwrap_or_default(),
        // An unknown stored level reads as the lowest (never wider).
        max_level: Level::parse(&level).unwrap_or(Level::Member),
        uploads: r.get::<_, i64>(4)? != 0,
        max_members: r.get::<_, i64>(5)?.clamp(0, MAX_MAX_MEMBERS as i64) as u32,
        granted_by: r.get(6)?,
        granted_at: r.get(7)?,
        revoked_at: r.get(8)?,
    })
}

/// The App's key (on or off), on an open skin.db connection.
pub fn get_in(conn: &Connection, service_id: &str) -> Result<Option<ChatKey>, String> {
    conn.query_row(
        "SELECT service_id, enabled, caps, max_level, uploads, max_members, granted_by, granted_at, revoked_at
         FROM app_chat_keys WHERE service_id = ?1",
        params![service_id.trim()],
        key_row,
    )
    .optional()
    .map_err(|e| format!("chat key lookup: {e}"))
}

/// The App's key, only while it is on.
pub fn enabled_in(conn: &Connection, service_id: &str) -> Result<Option<ChatKey>, String> {
    Ok(get_in(conn, service_id)?.filter(|k| k.enabled))
}

/// The App's key (on or off).
pub fn get(service_id: &str) -> Result<Option<ChatKey>, String> {
    with_conn(|conn| get_in(conn, service_id))
}

/// The App's key, only while it is on (a keyed App).
pub fn enabled(service_id: &str) -> Result<Option<ChatKey>, String> {
    with_conn(|conn| enabled_in(conn, service_id))
}

/// People who hold an enabled chat grant this App's key made.
pub fn members_made(conn: &Connection, service_id: &str) -> Result<u32, String> {
    conn.query_row(
        "SELECT COUNT(*) FROM grants WHERE kind = 'chat' AND subject_kind = 'principal' AND enabled = 1 AND created_by = ?1",
        params![marker(service_id)],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n.clamp(0, u32::MAX as i64) as u32)
    .map_err(|e| format!("chat key members: {e}"))
}

/// Does a chat grant with this `created_by` count for a pass? A grant a
/// chat key made counts only while that key is on: revoking the key ends
/// passes through it everywhere (its people had no chat before it).
pub fn grant_counts_in(conn: &Connection, created_by: Option<&str>) -> Result<bool, String> {
    match created_by.and_then(|c| c.strip_prefix(CREATED_BY_PREFIX)) {
        None => Ok(true),
        Some(svc) => Ok(enabled_in(conn, svc)?.is_some()),
    }
}

/// Does the principal hold an enabled chat grant that counts for a pass
/// ([`grant_counts_in`])? The chat pass's per-person check (S28 §5), read
/// live on every mint.
pub fn principal_has_counting_chat_grant(principal_id: &str) -> Result<bool, String> {
    with_conn(|conn| principal_has_counting_chat_grant_in(conn, principal_id))
}

/// [`principal_has_counting_chat_grant`] on an open skin.db connection.
pub fn principal_has_counting_chat_grant_in(conn: &Connection, principal_id: &str) -> Result<bool, String> {
    // The same rows `principal_has_chat_grant` reads (kind chat on the
    // box's one Sew, enabled), each counted by who made it.
    let mut st = conn
        .prepare(
            "SELECT created_by FROM grants WHERE subject_kind = 'principal' AND subject_id = ?1
               AND kind = ?2 AND target_id = ?3 AND enabled != 0",
        )
        .map_err(|e| format!("chat grant lookup: {e}"))?;
    let makers = st
        .query_map(params![principal_id.trim(), super::CHAT_GRANT_KIND, super::CHAT_GRANT_TARGET], |r| r.get::<_, Option<String>>(0))
        .map_err(|e| format!("chat grant lookup: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("chat grant row: {e}"))?;
    for m in makers {
        if grant_counts_in(conn, m.as_deref())? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// May a keyed App's pass give this principal chat with no grant of their
/// own? Only a K2 App user of THAT App (an enabled `kind=app` grant on
/// it), never a login marked as an agent's (an agent is its workspace in
/// chat), never a principal who also holds the box login (`host`): they
/// keep today's rule (the owner gives them chat).
pub fn keyed_eligible(principal_id: &str, service_id: &str) -> Result<bool, String> {
    with_conn(|conn| {
        let pid = principal_id.trim();
        let agent: Option<Option<String>> = conn
            .query_row("SELECT agent_workspace FROM principals WHERE id = ?1", params![pid], |r| r.get(0))
            .optional()
            .map_err(|e| format!("chat key agent mark: {e}"))?;
        match agent {
            None => return Ok(false),
            Some(Some(ws)) if !ws.trim().is_empty() => return Ok(false),
            Some(_) => {}
        }
        // `enabled_only` false: any box-login row, on or off, keeps the
        // person the owner's (as `skin_routes::has_box_login`).
        let login = |app: &str, enabled_only: bool| -> Result<bool, String> {
            conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM grants WHERE subject_kind = 'principal' AND subject_id = ?1 AND kind = 'app'
                   AND target_id = ?2 AND (enabled = 1 OR ?3 = 0))",
                params![pid, app, enabled_only as i64],
                |r| r.get::<_, bool>(0),
            )
            .map_err(|e| format!("chat key login lookup: {e}"))
        };
        Ok(login(service_id.trim(), true)? && !login(HOST_APP_ID, false)?)
    })
}

/// Give (or change, or turn back on) the App's chat key. `w` is checked
/// ([`validate`]). Returns `(before, after)`. The App's Sew settings row
/// gets `max_level` and `uploads` in the same transaction.
pub fn set(service_id: &str, w: &Want, by: &str) -> Result<(Option<ChatKey>, ChatKey), String> {
    let svc = service_id.trim();
    if svc.is_empty() || svc == HOST_APP_ID {
        return Err("name a published App (host is the box login, not an App)".into());
    }
    validate(w)?;
    let caps = normalize_caps(&w.caps)?;
    let now = now_secs();
    let r = with_conn(|conn| {
        let tx = conn.unchecked_transaction().map_err(|e| format!("chat key begin: {e}"))?;
        let before = get_in(&tx, svc)?;
        tx.execute(
            "INSERT INTO app_chat_keys (service_id, enabled, caps, max_level, uploads, max_members, granted_by, granted_at, revoked_at)
             VALUES (?1, 1, ?2, ?3, ?4, ?5, ?6, ?7, NULL)
             ON CONFLICT(service_id) DO UPDATE SET enabled = 1, caps = excluded.caps, max_level = excluded.max_level,
               uploads = excluded.uploads, max_members = excluded.max_members, granted_by = excluded.granted_by,
               granted_at = excluded.granted_at, revoked_at = NULL",
            params![svc, json!(caps).to_string(), w.max_level.as_str(), w.uploads as i64, w.max_members as i64, by, now],
        )
        .map_err(|e| format!("chat key write: {e}"))?;
        crate::tool_hook::home::set_app_chat(&tx, svc, w.max_level.as_str(), w.uploads, by, now)?;
        let after = get_in(&tx, svc)?.ok_or_else(|| "chat key vanished".to_string())?;
        tx.commit().map_err(|e| format!("chat key commit: {e}"))?;
        Ok((before, after))
    })?;
    audit(svc, AUDIT_SET, by, json!({ "before": r.0, "after": r.1 }));
    crate::tool_hook::kick_reconcile();
    Ok(r)
}

/// What [`revoke`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revoked {
    /// The key as it was (`None`: the App never had one).
    pub before: Option<ChatKey>,
    /// Sew was asked to end this App's sessions (I8). `false` when the box's
    /// Sew doesn't report `app_governed_spaces` (it holds no keyed
    /// sessions then) or Sew was never set up here.
    pub sessions_revoke_queued: bool,
}

/// Switch the App's chat key off: no more passes through it at once, and
/// (on a Sew with `app_governed_spaces`) Sew ends every session whose
/// `app` claim is this App. The row stays, off. Idempotent: an App with
/// no key, or a key already off, changes nothing (the I8 call still goes,
/// so a retry after a lost answer is safe).
pub fn revoke(service_id: &str, by: &str) -> Result<Revoked, String> {
    let svc = service_id.trim();
    let now = now_secs();
    let r = with_conn(|conn| {
        let tx = conn.unchecked_transaction().map_err(|e| format!("chat key revoke begin: {e}"))?;
        let before = get_in(&tx, svc)?;
        if before.is_none() {
            return Ok(Revoked { before, sessions_revoke_queued: false });
        }
        tx.execute(
            "UPDATE app_chat_keys SET enabled = 0, revoked_at = COALESCE(CASE WHEN enabled = 1 THEN ?2 END, revoked_at) WHERE service_id = ?1",
            params![svc, now],
        )
        .map_err(|e| format!("chat key revoke: {e}"))?;
        let queued = queue_app_sessions_revoke(&tx, svc)?;
        tx.commit().map_err(|e| format!("chat key revoke commit: {e}"))?;
        Ok(Revoked { before, sessions_revoke_queued: queued })
    })?;
    if let Some(b) = &r.before {
        audit(svc, AUDIT_REVOKE, by, json!({ "before": b, "sessionsRevokeQueued": r.sessions_revoke_queued }));
    }
    Ok(r)
}

/// Queue Sew's I8 (`sessions/revoke {app_id}`) on `conn`'s open
/// transaction, only when the box's Sew reports `app_governed_spaces` (an
/// older Sew refuses the field). `Ok(false)`: not queued.
pub fn queue_app_sessions_revoke(conn: &Connection, service_id: &str) -> Result<bool, String> {
    let flag = crate::tool_hook::features::APP_GOVERNED_SPACES;
    if !crate::tool_hook::features::has(conn, crate::tool_hook::CHAT_KIND, flag)? {
        crate::log_debug!("[skin] chat key {service_id}: Sew lacks {flag}; no app_id session revoke sent");
        return Ok(false);
    }
    let queued = crate::sew_revoke::sink_installed() || crate::tool_hook::outbox::get_target(conn, crate::tool_hook::CHAT_KIND)?.is_some();
    crate::sew_revoke::enqueue(conn, &crate::sew_revoke::SewSessionRevoke::AppId(service_id.trim().to_string()))?;
    Ok(queued)
}

/// The App was removed from the box: its key goes off and Sew ends its
/// sessions (I8), keyed or not, on a Sew that takes `app_id`.
pub fn app_removed(service_id: &str, by: &str) -> Result<(), String> {
    let svc = service_id.trim();
    if svc.is_empty() || svc == HOST_APP_ID {
        return Ok(());
    }
    if get(svc)?.is_some() {
        revoke(svc, by)?;
        return Ok(());
    }
    with_conn(|conn| {
        let tx = conn.unchecked_transaction().map_err(|e| format!("app sessions revoke begin: {e}"))?;
        queue_app_sessions_revoke(&tx, svc)?;
        tx.commit().map_err(|e| format!("app sessions revoke commit: {e}"))
    })
}

fn audit(service_id: &str, event: &str, actor: &str, mut payload: serde_json::Value) {
    payload["service"] = json!(service_id);
    let subject = super::guest_rooms::audit_subject(service_id);
    let db = crate::db::shared();
    let conn = db.lock();
    if let Err(e) = crate::deploy::audit::append(&conn, crate::deploy::audit::SUBJECT_APP, &subject, event, actor, None, &payload) {
        crate::log_debug!("[skin] chat key audit {service_id} {event}: {e}");
    }
}

/// The App's chat-key audit rows, newest first.
pub fn audit_rows(service_id: &str, limit: usize) -> Result<Vec<crate::deploy::audit::AuditRow>, String> {
    let subject = super::guest_rooms::audit_subject(service_id);
    let db = crate::db::shared();
    let conn = db.lock();
    let rows = crate::deploy::audit::list_for(&conn, crate::deploy::audit::SUBJECT_APP, &subject, limit.max(1) * 4)
        .map_err(|e| format!("audit read: {e}"))?;
    Ok(rows
        .into_iter()
        .filter(|r| [AUDIT_SET, AUDIT_REVOKE].contains(&r.event.as_str()))
        .filter(|r| r.payload.get("service").and_then(|s| s.as_str()) == Some(service_id))
        .take(limit)
        .collect())
}

#[cfg(test)]
#[path = "skin_app_chat_key_tests.rs"]
mod tests;
