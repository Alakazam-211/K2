//! Member grants (`desktop_grants`, migration 0281).
//!
//! Owners and Admins open the desktop without a grant (Rosson 2026-11-10).
//! A Member opens only with a grant, and a Member grant is `view` only.
//! A Voice & Chat only login never opens it.
//!
//! **Keyed by the login's stable id** (vs-live R18), never its name: a
//! login removed and added again under the same name is a new id and
//! inherits nothing. A grant also ends when its login is disabled or its
//! role changes (`role_at_grant`): every read checks the login's current
//! facts and deletes a grant that no longer matches, so a change made
//! anywhere (Settings, `k2 users`, a direct store edit) ends it. No TTL: a
//! grant lasts until revoked.

use rusqlite::{params, Connection};
use serde::Serialize;

use super::wire::Mode;
use crate::connect_users::Role;

/// What the desktop needs to know about a login.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserFacts {
    pub id: String,
    pub username: String,
    pub role: Role,
    pub disabled: bool,
}

/// One grant (the owner's `grants` view; never a secret).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Grant {
    #[serde(skip)]
    pub desktop: String,
    pub user: String,
    pub user_id: String,
    #[serde(skip)]
    pub role_at_grant: Role,
    pub mode: Mode,
    pub granted_by: String,
    /// Unix seconds.
    pub granted_at: i64,
}

/// Why a grant was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrantError {
    NoSuchUser(String),
    Disabled(String),
    /// Owners and Admins open the desktop without one.
    NotNeeded(Role),
    /// A Member grant is view only.
    MemberViewOnly,
    /// Voice & Chat only logins never open the desktop.
    VoiceChatNever,
    Store(String),
}

impl GrantError {
    pub fn code(&self) -> &'static str {
        match self {
            GrantError::NoSuchUser(_) => "no_such_user",
            GrantError::Disabled(_) => "user_disabled",
            GrantError::NotNeeded(_) => "grant_not_needed",
            GrantError::MemberViewOnly => "member_view_only",
            GrantError::VoiceChatNever => "voice_chat_never",
            GrantError::Store(_) => "store_error",
        }
    }

    pub fn status(&self) -> u16 {
        match self {
            GrantError::NoSuchUser(_) => 404,
            GrantError::Store(_) => 500,
            _ => 400,
        }
    }

    pub fn hint(&self) -> String {
        match self {
            GrantError::NoSuchUser(u) => format!("there's no login named '{u}' on this server (k2 users list)"),
            GrantError::Disabled(u) => format!("the login '{u}' is disabled; enable it first"),
            GrantError::NotNeeded(r) => format!(
                "{} logins open Remote desktop without a grant; grants are for Members",
                r.label()
            ),
            GrantError::MemberViewOnly => "a Member grant is view only (--view); Members never drive the desktop".into(),
            GrantError::VoiceChatNever => "a Voice & Chat only login can never open Remote desktop".into(),
            GrantError::Store(e) => e.clone(),
        }
    }
}

fn is_live(g: &Grant, facts: Option<&UserFacts>) -> bool {
    match facts {
        Some(f) => f.id == g.user_id && !f.disabled && f.role == g.role_at_grant && f.role == Role::Member,
        None => false,
    }
}

fn read_all(conn: &Connection, desktop: &str) -> Result<Vec<Grant>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT user_id, username, role_at_grant, mode, granted_by, granted_at FROM desktop_grants \
             WHERE desktop_id = ?1 ORDER BY granted_at, username",
        )
        .map_err(|e| format!("desktop_grants read: {e}"))?;
    let rows = stmt
        .query_map(params![desktop], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?, r.get::<_, i64>(5)?))
        })
        .map_err(|e| format!("desktop_grants read: {e}"))?;
    let mut out = Vec::new();
    for row in rows {
        let (user_id, username, role, mode, by, at) = row.map_err(|e| format!("desktop_grants row: {e}"))?;
        // A row this build can't read is not a grant (fails closed).
        let (Some(role_at_grant), Some(mode)) = (Role::from_wire(&role), Mode::parse(&mode)) else {
            continue;
        };
        out.push(Grant { desktop: desktop.to_string(), user: username, user_id, role_at_grant, mode, granted_by: by, granted_at: at });
    }
    Ok(out)
}

fn delete(conn: &Connection, desktop: &str, user_id: &str) -> Result<bool, String> {
    conn.execute("DELETE FROM desktop_grants WHERE desktop_id = ?1 AND user_id = ?2", params![desktop, user_id])
        .map(|n| n > 0)
        .map_err(|e| format!("desktop_grants delete: {e}"))
}

/// May `facts`'s login get a `mode` grant? (Checked again by
/// [`grant_with`]; the route checks first so its audit line is written
/// before anything changes.)
pub fn check(facts: &UserFacts, mode: Mode) -> Result<(), GrantError> {
    if facts.id.is_empty() {
        return Err(GrantError::NoSuchUser(facts.username.clone()));
    }
    if facts.disabled {
        return Err(GrantError::Disabled(facts.username.clone()));
    }
    match facts.role {
        Role::Owner | Role::Admin => Err(GrantError::NotNeeded(facts.role)),
        Role::VoiceChat => Err(GrantError::VoiceChatNever),
        Role::Member if mode == Mode::Control => Err(GrantError::MemberViewOnly),
        Role::Member => Ok(()),
    }
}

/// Grant `facts`'s login `mode` on `desktop` (replacing any grant it had).
pub fn grant_with(conn: &Connection, desktop: &str, facts: &UserFacts, mode: Mode, by: &str) -> Result<Grant, GrantError> {
    check(facts, mode)?;
    let at = super::now_secs();
    conn.execute(
        "INSERT INTO desktop_grants (desktop_id, user_id, username, role_at_grant, mode, granted_by, granted_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
         ON CONFLICT(desktop_id, user_id) DO UPDATE SET username = excluded.username, role_at_grant = excluded.role_at_grant, \
         mode = excluded.mode, granted_by = excluded.granted_by, granted_at = excluded.granted_at",
        params![desktop, facts.id, facts.username, facts.role.as_wire(), mode.as_str(), by, at],
    )
    .map_err(|e| GrantError::Store(format!("desktop_grants write: {e}")))?;
    Ok(Grant {
        desktop: desktop.to_string(),
        user: facts.username.clone(),
        user_id: facts.id.clone(),
        role_at_grant: facts.role,
        mode,
        granted_by: by.to_string(),
        granted_at: at,
    })
}

/// Revoke `user_id`'s grant. Returns whether one was there.
pub fn revoke_with(conn: &Connection, desktop: &str, user_id: &str) -> Result<bool, String> {
    delete(conn, desktop, user_id)
}

/// The live grants (stale ones, whose login went, was disabled or changed
/// role, are deleted on the way). `lookup` resolves a login id.
pub fn list_with(conn: &Connection, desktop: &str, lookup: &dyn Fn(&str) -> Option<UserFacts>) -> Result<Vec<Grant>, String> {
    let mut out = Vec::new();
    for g in read_all(conn, desktop)? {
        if is_live(&g, lookup(&g.user_id).as_ref()) {
            out.push(g);
        } else {
            delete(conn, desktop, &g.user_id)?;
        }
    }
    Ok(out)
}

/// `facts`'s live grant on `desktop`, if any (a stale one is deleted).
pub fn live_grant_with(conn: &Connection, desktop: &str, facts: &UserFacts) -> Result<Option<Grant>, String> {
    let Some(g) = read_all(conn, desktop)?.into_iter().find(|g| g.user_id == facts.id) else {
        return Ok(None);
    };
    if is_live(&g, Some(facts)) {
        Ok(Some(g))
    } else {
        delete(conn, desktop, &g.user_id)?;
        Ok(None)
    }
}

// ── the process-wide DB and login store ─────────────────────────────

/// Every login's facts (ids backfilled by `connect_users::user_uuid`).
pub fn all_user_facts() -> Result<Vec<UserFacts>, String> {
    Ok(crate::connect_users::admin_space_facts()?
        .into_iter()
        .map(|(id, username, role, disabled)| UserFacts { id, username, role, disabled })
        .collect())
}

/// The facts of the login named `username` (its id backfilled first).
pub fn user_facts(username: &str) -> Result<Option<UserFacts>, String> {
    let Some(id) = crate::connect_users::user_uuid(username)? else {
        return Ok(None);
    };
    Ok(all_user_facts()?.into_iter().find(|f| f.id == id))
}

fn facts_by_id(id: &str) -> Option<UserFacts> {
    all_user_facts().ok()?.into_iter().find(|f| f.id == id)
}

/// [`grant_with`] for the login named `username`.
pub fn grant(desktop: &str, username: &str, mode: Mode, by: &str) -> Result<Grant, GrantError> {
    let facts = user_facts(username)
        .map_err(GrantError::Store)?
        .ok_or_else(|| GrantError::NoSuchUser(username.trim().to_string()))?;
    let db = crate::db::shared();
    let conn = db.lock();
    grant_with(&conn, desktop, &facts, mode, by)
}

/// Revoke the grant of the login named `username`, or of a login id that
/// no longer has a name (a removed account's leftover row).
pub fn revoke(desktop: &str, username_or_id: &str) -> Result<Option<String>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let key = username_or_id.trim();
    let id = match user_facts(key) {
        Ok(Some(f)) => f.id,
        _ => match read_all(&conn, desktop)?.into_iter().find(|g| g.user_id == key || g.user == key) {
            Some(g) => g.user_id,
            None => return Ok(None),
        },
    };
    Ok(revoke_with(&conn, desktop, &id)?.then_some(id))
}

/// [`list_with`] on the shared DB and the login store.
pub fn list(desktop: &str) -> Result<Vec<Grant>, String> {
    let facts = all_user_facts()?;
    let db = crate::db::shared();
    let conn = db.lock();
    list_with(&conn, desktop, &|id: &str| facts.iter().find(|f| f.id == id).cloned())
}

/// [`live_grant_with`] for a login, by name.
pub fn live_grant(desktop: &str, username: &str) -> Result<Option<Grant>, String> {
    let Some(facts) = user_facts(username)? else { return Ok(None) };
    let db = crate::db::shared();
    let conn = db.lock();
    live_grant_with(&conn, desktop, &facts)
}

/// Is a login id still the same person with a live grant? For the
/// stream's 5 s re-check (D6), which holds the id.
pub fn still_granted(desktop: &str, user_id: &str) -> bool {
    let Some(facts) = facts_by_id(user_id) else { return false };
    let db = crate::db::shared();
    let conn = db.lock();
    matches!(live_grant_with(&conn, desktop, &facts), Ok(Some(_)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop::CONSOLE;

    fn member(id: &str, name: &str) -> UserFacts {
        UserFacts { id: id.into(), username: name.into(), role: Role::Member, disabled: false }
    }

    #[test]
    fn a_member_grant_is_view_only() {
        let g = crate::db::scoped_for_test();
        let conn = g.handle();
        let conn = conn.lock();
        let m = member("u-1", "mel");
        assert_eq!(grant_with(&conn, CONSOLE, &m, Mode::Control, "owner"), Err(GrantError::MemberViewOnly));
        assert_eq!(read_all(&conn, CONSOLE).unwrap(), vec![], "nothing stored");
        let ok = grant_with(&conn, CONSOLE, &m, Mode::View, "owner").expect("view");
        assert_eq!((ok.mode, ok.user.as_str(), ok.user_id.as_str()), (Mode::View, "mel", "u-1"));
        assert_eq!(GrantError::MemberViewOnly.code(), "member_view_only");
    }

    #[test]
    fn owners_admins_and_voice_chat_get_no_grant() {
        let g = crate::db::scoped_for_test();
        let conn = g.handle();
        let conn = conn.lock();
        for (role, want) in [
            (Role::Owner, GrantError::NotNeeded(Role::Owner)),
            (Role::Admin, GrantError::NotNeeded(Role::Admin)),
            (Role::VoiceChat, GrantError::VoiceChatNever),
        ] {
            let f = UserFacts { role, ..member("u-x", "x") };
            assert_eq!(grant_with(&conn, CONSOLE, &f, Mode::View, "owner"), Err(want));
        }
        let off = UserFacts { disabled: true, ..member("u-d", "dis") };
        assert_eq!(grant_with(&conn, CONSOLE, &off, Mode::View, "owner"), Err(GrantError::Disabled("dis".into())));
        assert_eq!(read_all(&conn, CONSOLE).unwrap(), vec![]);
    }

    #[test]
    fn grants_follow_the_id_and_end_on_remove_disable_or_role_change() {
        let g = crate::db::scoped_for_test();
        let conn = g.handle();
        let conn = conn.lock();
        let mel = member("u-mel", "mel");
        let ann = member("u-ann", "ann");
        let bob = member("u-bob", "bob");
        for f in [&mel, &ann, &bob] {
            grant_with(&conn, CONSOLE, f, Mode::View, "owner").expect("grant");
        }
        assert!(live_grant_with(&conn, CONSOLE, &mel).unwrap().is_some());

        // mel removed and added again under the same name: a new id.
        let mel_again = member("u-mel-2", "mel");
        assert_eq!(live_grant_with(&conn, CONSOLE, &mel_again).unwrap(), None, "the name inherits nothing");
        // ann became an Admin (opens without a grant now); bob was disabled.
        let ann_admin = UserFacts { role: Role::Admin, ..ann.clone() };
        let bob_off = UserFacts { disabled: true, ..bob.clone() };
        let world = [mel_again.clone(), ann_admin.clone(), bob_off.clone()];
        let lookup = |id: &str| world.iter().find(|f| f.id == id).cloned();
        assert_eq!(list_with(&conn, CONSOLE, &lookup).unwrap(), vec![], "every grant is stale");
        assert_eq!(read_all(&conn, CONSOLE).unwrap(), vec![], "and deleted");
        // A role round trip doesn't bring it back.
        assert_eq!(live_grant_with(&conn, CONSOLE, &ann).unwrap(), None);
    }

    #[test]
    fn revoke_and_regrant_replace_one_row() {
        let g = crate::db::scoped_for_test();
        let conn = g.handle();
        let conn = conn.lock();
        let mel = member("u-mel", "mel");
        grant_with(&conn, CONSOLE, &mel, Mode::View, "owner").unwrap();
        grant_with(&conn, CONSOLE, &mel, Mode::View, "login:rosson").unwrap();
        let rows = read_all(&conn, CONSOLE).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].granted_by, "login:rosson");
        assert!(revoke_with(&conn, CONSOLE, "u-mel").unwrap());
        assert!(!revoke_with(&conn, CONSOLE, "u-mel").unwrap(), "idempotent");
        assert_eq!(live_grant_with(&conn, CONSOLE, &mel).unwrap(), None);
        // Grants are per desktop.
        grant_with(&conn, "agent:x", &mel, Mode::View, "owner").unwrap();
        assert_eq!(live_grant_with(&conn, CONSOLE, &mel).unwrap(), None);
    }

    #[test]
    fn the_grants_view_carries_no_role_or_desktop_internals() {
        let g = Grant {
            desktop: CONSOLE.into(),
            user: "mel".into(),
            user_id: "u-1".into(),
            role_at_grant: Role::Member,
            mode: Mode::View,
            granted_by: "owner".into(),
            granted_at: 5,
        };
        assert_eq!(
            serde_json::to_value(&g).unwrap(),
            serde_json::json!({"user": "mel", "userId": "u-1", "mode": "view", "grantedBy": "owner", "grantedAt": 5})
        );
    }
}
