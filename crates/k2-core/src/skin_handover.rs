//! The skin.db half of an App handover (prd-app-handoff-v1 rev 2 §4.3,
//! §4.4): which of the App's guests move how, and the room rewrite.
//!
//! A room is a workspace's `projects.id`. The App's guests reach the agent
//! of each room on their pass. A handover moves the App's own room from A
//! to B **for this App only**, in every place skin.db stores it:
//!
//! - the App's roles (`roles.app_id` = the service): `rooms`, `room_policy`;
//! - its grants (`grants.kind = 'app'`, `target_id` = the service): `scope`;
//! - its live sessions and platform tokens (`tokens.app_service_id`, never
//!   a link pass: those are re-minted): `rooms`, `room_policy`;
//! - its guests-rooms grant (`app_owner_grants.rooms`);
//! - `principals.default_rooms`, only for guests whose every App grant is
//!   on this App;
//! - its owner-made test guests (`app_test_guests.workspace_id`).
//!
//! Room A on other Apps' grants, and box-wide roles (no `app_id`) other
//! Apps share, are untouched: those are A's (the plan lists the roles).
//! Every rewrite is idempotent ("already B" is a no-op), so the apply rolls
//! forward after a crash, and undo is the same rewrite B → A.
//!
//! Lock order: callers gather everything they need from the daemon DB
//! first; nothing here touches it.

use std::collections::BTreeSet;

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use super::{parse_rooms_json, parse_room_policy_json, room_policy_json, rooms_json, with_conn, RoomPolicy, HOST_APP_ID};

/// The owner's guest (no tie, no agent stamp, or the owner took it over).
pub const CLASS_OWNER: &str = "owner";
/// Tied to (or added by) the giving workspace, with grants only on this
/// App among A's Apps: re-tied to B and its known password retired (S3).
pub const CLASS_GIVER_TIED: &str = "giver_tied";
/// Tied by one of this App's keys (`app-key:<svc>`): A's runtime could
/// know its password; retired with the giver-tied (S3).
pub const CLASS_APP_KEY_TIED: &str = "app_key_tied";
/// Tied to A and also on A's other Apps: stays A's (§4.4).
pub const CLASS_SHARED: &str = "shared";
/// A login the owner marked as the giving workspace's agent
/// (`principals.agent_workspace` = A): its grant on this App is disabled.
pub const CLASS_AGENT_LOGIN: &str = "agent_login";
/// Tied to a third workspace (or another App's key): untouched.
pub const CLASS_OTHER_TIED: &str = "other_tied";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoverGuest {
    pub principal_id: String,
    pub username: String,
    pub class: &'static str,
    /// The tie (`workspace id` or `app-key:<svc>`), `None` for a person.
    pub tie: Option<String>,
    pub has_password: bool,
    pub grant_enabled: bool,
    /// An owner-made (`true`) or agent-made (`false`) test guest of this App.
    pub test_guest: Option<bool>,
}

/// Every principal with a grant on `service_id`, classified for a handover
/// from workspace `from`. `other_services_of_from` = the service ids of
/// A's other Apps (read from the daemon DB by the caller first).
pub fn guests(service_id: &str, from: &str, other_services_of_from: &[String]) -> Result<Vec<HandoverGuest>, String> {
    with_conn(|conn| guests_conn(conn, service_id, from, other_services_of_from))
}

fn claimed(conn: &Connection, pid: &str) -> Result<bool, String> {
    conn.query_row("SELECT 1 FROM principal_person_claims WHERE principal_id = ?1", params![pid], |_| Ok(()))
        .optional()
        .map(|o| o.is_some())
        .map_err(|e| format!("person claim lookup: {e}"))
}

/// [`super::creators::agent_tie`] on an open connection.
pub(crate) fn tie_conn(conn: &Connection, pid: &str) -> Result<Option<String>, String> {
    if claimed(conn, pid)? {
        return Ok(None);
    }
    let tied: Option<String> = conn
        .query_row("SELECT tied_to FROM principal_agent_ties WHERE principal_id = ?1", params![pid], |r| r.get(0))
        .optional()
        .map_err(|e| format!("agent tie lookup: {e}"))?;
    match tied {
        Some(t) => Ok(Some(t)),
        None => super::creators::workspace_of_conn(conn, pid),
    }
}

pub(crate) fn guests_conn(conn: &Connection, service_id: &str, from: &str, other_services_of_from: &[String]) -> Result<Vec<HandoverGuest>, String> {
    let rows: Vec<(String, String, Option<String>, Option<String>, bool)> = {
        let mut st = conn
            .prepare(
                "SELECT p.id, p.username, p.password_hash, p.agent_workspace, g.enabled FROM grants g JOIN principals p ON p.id = g.subject_id \
                 WHERE g.subject_kind = 'principal' AND g.kind = 'app' AND g.target_id = ?1 ORDER BY p.username",
            )
            .map_err(|e| format!("handover guests: {e}"))?;
        let it = st
            .query_map(params![service_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, i64>(4)? != 0))
            })
            .map_err(|e| format!("handover guests: {e}"))?;
        it.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| format!("handover guests: {e}"))?
    };
    let key_marker = super::creators::app_key_marker(service_id);
    let mut out = Vec::with_capacity(rows.len());
    for (pid, username, hash, agent_ws, enabled) in rows {
        let tie = tie_conn(conn, &pid)?;
        let on_others = if other_services_of_from.is_empty() {
            false
        } else {
            let mut st = conn
                .prepare("SELECT target_id FROM grants WHERE subject_kind = 'principal' AND subject_id = ?1 AND kind = 'app' AND enabled != 0")
                .map_err(|e| format!("handover guests: {e}"))?;
            let targets: Vec<String> = st
                .query_map(params![pid], |r| r.get(0))
                .map_err(|e| format!("handover guests: {e}"))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| format!("handover guests: {e}"))?;
            targets.iter().any(|t| other_services_of_from.contains(t))
        };
        let class = if agent_ws.as_deref().map(str::trim) == Some(from) && !from.is_empty() {
            CLASS_AGENT_LOGIN
        } else {
            match tie.as_deref() {
                None => CLASS_OWNER,
                Some(t) if t == from && !from.is_empty() => {
                    if on_others {
                        CLASS_SHARED
                    } else {
                        CLASS_GIVER_TIED
                    }
                }
                Some(t) if t == key_marker => CLASS_APP_KEY_TIED,
                Some(_) => CLASS_OTHER_TIED,
            }
        };
        let test_guest: Option<bool> = conn
            .query_row("SELECT created_by FROM app_test_guests WHERE principal_id = ?1 AND app_id = ?2", params![pid, service_id], |r| {
                r.get::<_, String>(0)
            })
            .optional()
            .map_err(|e| format!("handover test guest: {e}"))?
            .map(|by| !by.starts_with("agent:"));
        out.push(HandoverGuest {
            principal_id: pid,
            username,
            class,
            tie,
            has_password: hash.is_some_and(|h| !h.trim().is_empty()),
            grant_enabled: enabled,
            test_guest,
        });
    }
    Ok(out)
}

/// Where the App's room A is stored today (the plan's counts).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomFootprint {
    /// The App's own roles naming room A.
    pub roles: usize,
    /// Box-wide roles (no `app_id`) this App's grants use that name room
    /// A: untouched (other Apps share them), listed on the card.
    pub shared_roles: Vec<String>,
    pub grants: usize,
    pub sessions: usize,
    pub guest_rooms_grant: bool,
    pub default_rooms: usize,
    pub test_guests: usize,
}

pub fn room_footprint(service_id: &str, from: &str) -> Result<RoomFootprint, String> {
    with_conn(|conn| footprint_conn(conn, service_id, from))
}

fn names(list: &[String], room: &str) -> bool {
    list.iter().any(|r| r == room)
}

fn footprint_conn(conn: &Connection, svc: &str, from: &str) -> Result<RoomFootprint, String> {
    let mut fp = RoomFootprint::default();
    if from.is_empty() {
        return Ok(fp);
    }
    for (_, rooms, policy) in roles_of(conn, svc)? {
        if names(&rooms, from) || policy.contains_key(from) {
            fp.roles += 1;
        }
    }
    for (_, scope) in grants_of(conn, svc)? {
        if names(&parse_rooms_json(scope.as_deref()), from) {
            fp.grants += 1;
        }
    }
    for (_, rooms, policy, _) in tokens_of(conn, svc)? {
        if names(&rooms, from) || policy.as_ref().is_some_and(|p| p.contains_key(from)) {
            fp.sessions += 1;
        }
    }
    if let Some(rooms) = guest_rooms_of(conn, svc)? {
        fp.guest_rooms_grant = names(&rooms, from);
    }
    fp.default_rooms = sole_app_principals(conn, svc)?.into_iter().filter(|(_, d)| names(d, from)).count();
    fp.test_guests = conn
        .query_row(
            "SELECT COUNT(*) FROM app_test_guests WHERE app_id = ?1 AND workspace_id = ?2 AND created_by NOT LIKE 'agent:%'",
            params![svc, from],
            |r| r.get::<_, i64>(0),
        )
        .map_err(|e| format!("handover test guests: {e}"))? as usize;
    let mut st = conn
        .prepare(
            "SELECT DISTINCT r.name, r.rooms, COALESCE(r.room_policy, '{}') FROM grants g JOIN roles r ON r.id = g.role_id \
             WHERE g.kind = 'app' AND g.target_id = ?1 AND (r.app_id IS NULL OR r.app_id != ?1)",
        )
        .map_err(|e| format!("handover shared roles: {e}"))?;
    let shared: Vec<(String, String, String)> = st
        .query_map(params![svc], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .map_err(|e| format!("handover shared roles: {e}"))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| format!("handover shared roles: {e}"))?;
    for (name, rooms, policy) in shared {
        let p = parse_room_policy_json(Some(&policy))?.unwrap_or_default();
        if names(&parse_rooms_json(Some(&rooms)), from) || p.contains_key(from) {
            fp.shared_roles.push(name);
        }
    }
    fp.shared_roles.sort();
    Ok(fp)
}

fn roles_of(conn: &Connection, svc: &str) -> Result<Vec<(String, Vec<String>, RoomPolicy)>, String> {
    let mut st = conn
        .prepare("SELECT id, rooms, COALESCE(room_policy, '{}') FROM roles WHERE app_id = ?1")
        .map_err(|e| format!("handover roles: {e}"))?;
    let rows: Vec<(String, String, String)> = st
        .query_map(params![svc], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .map_err(|e| format!("handover roles: {e}"))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| format!("handover roles: {e}"))?;
    rows.into_iter()
        .map(|(id, rooms, policy)| Ok((id, parse_rooms_json(Some(&rooms)), parse_room_policy_json(Some(&policy))?.unwrap_or_default())))
        .collect()
}

fn grants_of(conn: &Connection, svc: &str) -> Result<Vec<(String, Option<String>)>, String> {
    let mut st = conn.prepare("SELECT id, scope FROM grants WHERE kind = 'app' AND target_id = ?1").map_err(|e| format!("handover grants: {e}"))?;
    let out = st
        .query_map(params![svc], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| format!("handover grants: {e}"))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| format!("handover grants: {e}"));
    out
}

/// The App's live non-link tokens: (id, rooms, room_policy, session).
fn tokens_of(conn: &Connection, svc: &str) -> Result<Vec<(String, Vec<String>, Option<RoomPolicy>, bool)>, String> {
    let mut st = conn
        .prepare("SELECT id, rooms, room_policy, session FROM tokens WHERE app_service_id = ?1 AND link_id IS NULL AND revoked_at IS NULL")
        .map_err(|e| format!("handover tokens: {e}"))?;
    let rows: Vec<(String, String, Option<String>, i64)> = st
        .query_map(params![svc], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .map_err(|e| format!("handover tokens: {e}"))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| format!("handover tokens: {e}"))?;
    rows.into_iter()
        .map(|(id, rooms, policy, session)| Ok((id, parse_rooms_json(Some(&rooms)), parse_room_policy_json(policy.as_deref())?, session != 0)))
        .collect()
}

fn guest_rooms_of(conn: &Connection, svc: &str) -> Result<Option<Vec<String>>, String> {
    conn.query_row(
        "SELECT rooms FROM app_owner_grants WHERE service_id = ?1 AND kind = ?2",
        params![svc, super::guest_rooms::GRANT_KIND],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .map(|o| o.map(|s| parse_rooms_json(Some(&s))))
    .map_err(|e| format!("handover guests-rooms grant: {e}"))
}

/// Principals whose every enabled App grant is on `svc` (and who have no
/// box login): (id, default_rooms).
fn sole_app_principals(conn: &Connection, svc: &str) -> Result<Vec<(String, Vec<String>)>, String> {
    let mut st = conn
        .prepare(
            "SELECT p.id, p.default_rooms FROM principals p WHERE EXISTS (SELECT 1 FROM grants g WHERE g.subject_kind = 'principal' \
             AND g.subject_id = p.id AND g.kind = 'app' AND g.target_id = ?1) AND NOT EXISTS (SELECT 1 FROM grants g \
             WHERE g.subject_kind = 'principal' AND g.subject_id = p.id AND g.kind = 'app' AND g.target_id != ?1)",
        )
        .map_err(|e| format!("handover default rooms: {e}"))?;
    let rows: Vec<(String, String)> = st
        .query_map(params![svc], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| format!("handover default rooms: {e}"))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| format!("handover default rooms: {e}"))?;
    Ok(rows.into_iter().map(|(id, d)| (id, parse_rooms_json(Some(&d)))).collect())
}

/// `from` → `to` in a room list (kept in order, no duplicate).
pub fn swap_room(list: &[String], from: &str, to: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::with_capacity(list.len());
    for r in list {
        let r = if r == from { to.to_string() } else { r.clone() };
        if seen.insert(r.clone()) {
            out.push(r);
        }
    }
    out
}

/// `from` → `to` in a room policy (caps merged when `to` is already there).
pub fn swap_policy(p: &RoomPolicy, from: &str, to: &str) -> RoomPolicy {
    let mut out = p.clone();
    if let Some(caps) = out.remove(from) {
        let entry = out.entry(to.to_string()).or_default();
        for c in caps {
            if !entry.contains(&c) {
                entry.push(c);
            }
        }
    }
    out
}

/// What [`move_rooms`] rewrote.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomReport {
    pub roles: usize,
    pub grants: usize,
    pub sessions: usize,
    pub guest_rooms_grant: bool,
    pub default_rooms: usize,
    pub test_guests: usize,
    pub agent_logins_disabled: usize,
}

/// Move the App's room `from` → `to` everywhere skin.db stores it for
/// this App, and disable the grant on this App of every login the owner
/// marked as `from`'s agent (P9). One transaction; idempotent.
pub fn move_rooms(service_id: &str, from: &str, to: &str) -> Result<RoomReport, String> {
    with_conn(|conn| {
        let tx = conn.unchecked_transaction().map_err(|e| format!("handover rooms: {e}"))?;
        let r = move_rooms_conn(&tx, service_id, from, to, true)?;
        tx.commit().map_err(|e| format!("handover rooms commit: {e}"))?;
        Ok(r)
    })
}

/// [`move_rooms`] inside the caller's transaction. `cut_agent_logins`:
/// the forward move disables `from`'s agent logins on this App; undo
/// (`false`) leaves the grants as they are (the owner re-enables a login
/// deliberately).
pub(crate) fn move_rooms_conn(conn: &Connection, svc: &str, from: &str, to: &str, cut_agent_logins: bool) -> Result<RoomReport, String> {
    let mut rep = RoomReport::default();
    if from.is_empty() || to.is_empty() || from == to {
        return Ok(rep);
    }
    let err = |e: rusqlite::Error| format!("handover rooms: {e}");
    for (id, rooms, policy) in roles_of(conn, svc)? {
        if !names(&rooms, from) && !policy.contains_key(from) {
            continue;
        }
        conn.execute(
            "UPDATE roles SET rooms = ?2, room_policy = ?3 WHERE id = ?1",
            params![id, rooms_json(&swap_room(&rooms, from, to)), room_policy_json(&swap_policy(&policy, from, to))],
        )
        .map_err(err)?;
        rep.roles += 1;
    }
    for (id, scope) in grants_of(conn, svc)? {
        let rooms = parse_rooms_json(scope.as_deref());
        if !names(&rooms, from) {
            continue;
        }
        conn.execute("UPDATE grants SET scope = ?2 WHERE id = ?1", params![id, rooms_json(&swap_room(&rooms, from, to))]).map_err(err)?;
        rep.grants += 1;
    }
    // Live sessions are rewritten in place (as role edits do): a guest
    // signed in before the switch talks to `to` from the switch on.
    for (id, rooms, policy, _) in tokens_of(conn, svc)? {
        let in_policy = policy.as_ref().is_some_and(|p| p.contains_key(from));
        if !names(&rooms, from) && !in_policy {
            continue;
        }
        let new_policy = policy.map(|p| room_policy_json(&swap_policy(&p, from, to)));
        conn.execute(
            "UPDATE tokens SET rooms = ?2, room_policy = COALESCE(?3, room_policy) WHERE id = ?1",
            params![id, rooms_json(&swap_room(&rooms, from, to)), new_policy],
        )
        .map_err(err)?;
        rep.sessions += 1;
    }
    if let Some(rooms) = guest_rooms_of(conn, svc)? {
        if names(&rooms, from) {
            conn.execute(
                "UPDATE app_owner_grants SET rooms = ?3 WHERE service_id = ?1 AND kind = ?2",
                params![svc, super::guest_rooms::GRANT_KIND, rooms_json(&swap_room(&rooms, from, to))],
            )
            .map_err(err)?;
            rep.guest_rooms_grant = true;
        }
    }
    for (id, d) in sole_app_principals(conn, svc)? {
        if names(&d, from) {
            conn.execute("UPDATE principals SET default_rooms = ?2 WHERE id = ?1", params![id, rooms_json(&swap_room(&d, from, to))]).map_err(err)?;
            rep.default_rooms += 1;
        }
    }
    rep.test_guests = conn
        .execute(
            "UPDATE app_test_guests SET workspace_id = ?3 WHERE app_id = ?1 AND workspace_id = ?2 AND created_by NOT LIKE 'agent:%'",
            params![svc, from, to],
        )
        .map_err(err)?;
    if cut_agent_logins {
        rep.agent_logins_disabled = conn
            .execute(
                "UPDATE grants SET enabled = 0 WHERE kind = 'app' AND target_id = ?1 AND enabled != 0 AND subject_kind = 'principal' \
                 AND subject_id IN (SELECT id FROM principals WHERE trim(COALESCE(agent_workspace, '')) = ?2)",
                params![svc, from],
            )
            .map_err(err)?;
        // Their live sessions on this App end with the grant.
        conn.execute(
            "UPDATE tokens SET revoked_at = ?3 WHERE app_service_id = ?1 AND session = 1 AND revoked_at IS NULL \
             AND principal_id IN (SELECT id FROM principals WHERE trim(COALESCE(agent_workspace, '')) = ?2)",
            params![svc, from, super::now_secs()],
        )
        .map_err(err)?;
    }
    Ok(rep)
}

/// The host-login app id never moves (the box login is no App's).
pub fn is_host(service_id: &str) -> bool {
    service_id.trim() == HOST_APP_ID
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swaps_keep_order_merge_and_never_duplicate() {
        let l = vec!["a".to_string(), "c".to_string(), "b".to_string()];
        assert_eq!(swap_room(&l, "a", "b"), vec!["b".to_string(), "c".to_string()]);
        assert_eq!(swap_room(&l, "x", "y"), l, "a room not named is a no-op");
        let mut p = RoomPolicy::new();
        p.insert("a".into(), vec!["thread".into()]);
        p.insert("b".into(), vec!["files:read".into()]);
        let s = swap_policy(&p, "a", "b");
        assert_eq!(s.len(), 1);
        assert_eq!(s["b"], vec!["files:read".to_string(), "thread".to_string()]);
        assert_eq!(swap_policy(&s, "a", "b"), s, "idempotent");
    }
}
