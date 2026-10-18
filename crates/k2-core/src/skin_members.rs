//! The room access list (room-scope R9, 0.46.1): who holds a grant on a
//! room, and the owner's per-room "hide names" switch (GA-Q40).
//!
//! **Built from grants, not presence (GA-Q25).** A member is a guest (or an
//! agent login, `k2 app user agent`) with an **enabled** `kind=app` grant
//! whose login policy holds the room: the grant's role's `room_policy`, or
//! a role-less grant's `scope` rooms (Thread-only), exactly what a login
//! through that grant would snapshot ([`super::grant_login_policy`]). A
//! disabled grant is no grant (settings-access B3). Connect users have no
//! rooms (runtime AR29); callers show them as one line, "admins of this
//! box" ([`ADMINS_LINE`]).
//!
//! **Hiding names.** The owner can hide guests' names per room. Hiding
//! applies to the App transport only (guests and Apps); the owner (the
//! Garden bridge, the CLI) always sees every name. The switch lives in
//! `skin.db` (`room_privacy`, created on first use like the rest of
//! `skin.db`'s additive schema; not a daemon migration).
//!
//! **Changes.** Every grant, role or guest change that can move a room's
//! list calls [`changed`]; the daemon registers a listener
//! ([`set_listener`]) that emits `room_members_changed` (k2-core has no
//! event bus).

use std::sync::OnceLock;

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use super::{grant_login_policy, map_grant_row, now_secs, with_conn, GRANT_SELECT};

/// The line that stands for every Connect login (they have no rooms).
pub const ADMINS_LINE: &str = "admins of this box";

/// One member of a room.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomMember {
    pub principal_id: String,
    pub username: String,
    /// The guest's full name when set, else the username (never empty).
    pub display_name: String,
    /// `guest` (a person) or `agent` (a login marked `k2 app user agent`).
    pub kind: &'static str,
    /// The role on the grant, or `None` (a role-less grant: Thread-only).
    pub role: Option<String>,
    /// The grant's App: `host` (the box login) or a published service id.
    pub app: String,
    /// What the member may do in this room.
    pub caps: Vec<String>,
}

const PRIVACY_DDL: &str = "CREATE TABLE IF NOT EXISTS room_privacy (
    room TEXT PRIMARY KEY,
    hide_names INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL
);";

fn ensure_privacy(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(PRIVACY_DDL).map_err(|e| format!("skin room privacy schema: {e}"))
}

/// Members of `room` (a `projects.id`), by username. `app` = only grants
/// on that App (`host` or a service id); `None` = every App (the owner's
/// view).
pub fn room_members(room: &str, app: Option<&str>) -> Result<Vec<RoomMember>, String> {
    let room = room.trim();
    if room.is_empty() {
        return Err("missing room".to_string());
    }
    let app = app.map(str::trim).filter(|a| !a.is_empty());
    with_conn(|conn| {
        let mut st = conn
            .prepare(&format!(
                "{GRANT_SELECT} WHERE subject_kind = 'principal' AND kind = 'app' AND enabled != 0 \
                 AND (?1 IS NULL OR target_id = ?1) ORDER BY created_at, id"
            ))
            .map_err(|e| format!("skin room members: {e}"))?;
        let grants = st
            .query_map(params![app], map_grant_row)
            .map_err(|e| format!("skin room members: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("skin room members row: {e}"))?;
        let mut out = Vec::new();
        for g in grants {
            let (policy, role) = match grant_login_policy(conn, &g) {
                Ok(p) => p,
                // A grant naming a role that is gone logs nobody in.
                Err(_) => continue,
            };
            let Some(caps) = policy.get(room) else { continue };
            let Some((username, full_name, agent_ws)) = conn
                .query_row(
                    "SELECT username, full_name, agent_workspace FROM principals WHERE id = ?1",
                    params![g.subject_id],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?)),
                )
                .optional()
                .map_err(|e| format!("skin room member: {e}"))?
            else {
                continue;
            };
            let display_name = full_name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| username.clone());
            let kind = if agent_ws.as_deref().is_some_and(|w| !w.trim().is_empty()) { "agent" } else { "guest" };
            out.push(RoomMember {
                principal_id: g.subject_id.clone(),
                username,
                display_name,
                kind,
                role,
                app: g.target_id.clone(),
                caps: caps.clone(),
            });
        }
        out.sort_by(|a, b| a.username.cmp(&b.username).then_with(|| a.app.cmp(&b.app)));
        Ok(out)
    })
}

/// Has the owner hidden guests' names in `room`?
pub fn names_hidden(room: &str) -> Result<bool, String> {
    let room = room.trim().to_string();
    with_conn(|conn| {
        ensure_privacy(conn)?;
        let v: Option<i64> = conn
            .query_row("SELECT hide_names FROM room_privacy WHERE room = ?1", params![room], |r| r.get(0))
            .optional()
            .map_err(|e| format!("skin room privacy: {e}"))?;
        Ok(v.unwrap_or(0) != 0)
    })
}

/// Every room whose names are hidden (owner views).
pub fn hidden_rooms() -> Result<Vec<String>, String> {
    with_conn(|conn| {
        ensure_privacy(conn)?;
        let mut st = conn
            .prepare("SELECT room FROM room_privacy WHERE hide_names != 0 ORDER BY room")
            .map_err(|e| format!("skin room privacy: {e}"))?;
        let rows = st.query_map([], |r| r.get(0)).map_err(|e| format!("skin room privacy: {e}"))?;
        rows.collect::<Result<Vec<String>, _>>().map_err(|e| format!("skin room privacy: {e}"))
    })
}

/// Hide (or show) guests' names in `room`. Returns whether it changed.
pub fn set_names_hidden(room: &str, hide: bool) -> Result<bool, String> {
    let room = room.trim().to_string();
    if room.is_empty() {
        return Err("missing room".to_string());
    }
    let was = names_hidden(&room)?;
    with_conn(|conn| {
        ensure_privacy(conn)?;
        conn.execute(
            "INSERT INTO room_privacy (room, hide_names, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(room) DO UPDATE SET hide_names = excluded.hide_names, updated_at = excluded.updated_at",
            params![room, hide as i64, now_secs()],
        )
        .map_err(|e| format!("skin room privacy: {e}"))?;
        Ok(())
    })?;
    if was != hide {
        changed(Some(&room));
    }
    Ok(was != hide)
}

// ── change listener ──────────────────────────────────────────────────

static LISTENER: OnceLock<fn(Option<&str>)> = OnceLock::new();

/// Register the change listener (first call wins; later calls no-op).
pub fn set_listener(f: fn(Option<&str>)) {
    let _ = LISTENER.set(f);
}

/// A room's list may have changed (`None` = any room: a grant, role or
/// guest change).
pub fn changed(room: Option<&str>) {
    if let Some(f) = LISTENER.get() {
        f(room);
    }
}

/// `r`, after telling the listener when it is `Ok` (wraps a skin write).
pub(crate) fn notify_ok<T>(r: Result<T, String>) -> Result<T, String> {
    if r.is_ok() {
        changed(None);
    }
    r
}

#[cfg(test)]
mod tests {
    use super::super::{
        add_principal, add_principal_for_app, assign_role, create_role, list_grants, set_grant_enabled,
        set_principal_default_rooms, set_principal_full_name, with_conn, RoomPolicy, CAP_ROOM_MEMBERS, CAP_THREAD_POST,
        CAP_THREAD_READ, HOST_APP_ID,
    };
    use super::*;
    use std::sync::Mutex;

    fn policy(room: &str, caps: &[&str]) -> RoomPolicy {
        let mut p = RoomPolicy::new();
        p.insert(room.to_string(), caps.iter().map(|c| c.to_string()).collect());
        p
    }

    fn names(m: &[RoomMember]) -> Vec<&str> {
        m.iter().map(|x| x.username.as_str()).collect()
    }

    #[test]
    fn members_come_from_enabled_grants_and_their_login_policy() {
        let _home = crate::test_env::TempHome::new();
        let room = uuid::Uuid::new_v4().to_string();
        let other = uuid::Uuid::new_v4().to_string();
        for u in ["ada", "bo", "cy"] {
            add_principal(u).unwrap();
        }
        create_role("staff", &policy(&room, &[CAP_THREAD_READ, CAP_ROOM_MEMBERS])).unwrap();
        create_role("elsewhere", &policy(&other, &[CAP_THREAD_READ])).unwrap();
        assign_role("ada", "staff").unwrap();
        assign_role("cy", "elsewhere").unwrap();
        set_principal_default_rooms("bo", &[room.clone()], false).unwrap();
        set_principal_full_name("ada", Some("Ada Lovelace")).unwrap();

        let m = room_members(&room, None).unwrap();
        assert_eq!(names(&m), ["ada", "bo"], "cy's role holds another room only");
        assert_eq!(m[0].display_name, "Ada Lovelace");
        assert_eq!(m[0].role.as_deref(), Some("staff"));
        assert_eq!(m[0].caps, [CAP_THREAD_READ, CAP_ROOM_MEMBERS]);
        assert_eq!((m[0].kind, m[0].app.as_str()), ("guest", HOST_APP_ID));
        assert_eq!(m[1].display_name, "bo", "no full name: the username");
        assert_eq!(m[1].role, None);
        assert_eq!(m[1].caps, [CAP_THREAD_READ, CAP_THREAD_POST], "a role-less grant is Thread-only");

        // A disabled grant is no grant (settings-access B3).
        let bo_id = with_conn(|c| Ok(c.query_row("SELECT id FROM principals WHERE username = 'bo'", [], |r| r.get::<_, String>(0)).unwrap())).unwrap();
        let g = list_grants().unwrap().into_iter().find(|g| g.subject_id == bo_id && g.target_id == HOST_APP_ID).expect("bo's box grant");
        set_grant_enabled(&g.id, false).unwrap();
        assert_eq!(names(&room_members(&room, None).unwrap()), ["ada"]);
        set_grant_enabled(&g.id, true).unwrap();

        // Grants on another App: in the owner's view, not in the box's.
        add_principal_for_app("dee", None, "svc-jobs", Some("staff"), "owner").unwrap();
        assert_eq!(names(&room_members(&room, None).unwrap()), ["ada", "bo", "dee"]);
        assert_eq!(names(&room_members(&room, Some(HOST_APP_ID)).unwrap()), ["ada", "bo"]);
        let jobs = room_members(&room, Some("svc-jobs")).unwrap();
        assert_eq!(names(&jobs), ["dee"]);
        assert_eq!(jobs[0].app, "svc-jobs");

        // An agent login reads as kind agent.
        with_conn(|c| Ok(c.execute("UPDATE principals SET agent_workspace = 'ws-1' WHERE username = 'dee'", []).unwrap())).unwrap();
        assert_eq!(room_members(&room, Some("svc-jobs")).unwrap()[0].kind, "agent");
        assert!(room_members(&uuid::Uuid::new_v4().to_string(), None).unwrap().is_empty());
        assert!(room_members(" ", None).is_err(), "a room is required");
    }

    static SEEN: Mutex<Vec<Option<String>>> = Mutex::new(Vec::new());

    fn record(room: Option<&str>) {
        SEEN.lock().unwrap_or_else(|e| e.into_inner()).push(room.map(str::to_string));
    }

    #[test]
    fn names_hidden_is_per_room_and_changes_tell_the_listener() {
        let _home = crate::test_env::TempHome::new();
        set_listener(record);
        let room = uuid::Uuid::new_v4().to_string();
        let other = uuid::Uuid::new_v4().to_string();
        assert!(!names_hidden(&room).unwrap(), "shown by default");
        assert!(set_names_hidden(&room, true).unwrap(), "a change");
        assert!(!set_names_hidden(&room, true).unwrap(), "the same again is no change");
        assert!(names_hidden(&room).unwrap());
        assert!(!names_hidden(&other).unwrap(), "per room");
        assert_eq!(hidden_rooms().unwrap(), [room.clone()]);
        let seen = SEEN.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(seen.iter().filter(|r| r.as_deref() == Some(room.as_str())).count(), 1, "one event for one change: {seen:?}");
        assert!(set_names_hidden(&room, false).unwrap());
        assert!(!names_hidden(&room).unwrap());
        let before = SEEN.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|r| r.is_none()).count();
        add_principal(&format!("u{}", &other[..6])).unwrap();
        let after = SEEN.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|r| r.is_none()).count();
        assert!(after > before, "a guest change tells the listener (any room)");
    }
}
