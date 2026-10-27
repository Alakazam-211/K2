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
/// Tied to the giving workspace only by an App key (every tie reason is
/// `app_key`): A's runtime could know its password. Re-tied and retired
/// with the giver-tied (S3); listed apart on the card.
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
    /// The workspace the guest's tie holds it to (`principal_agent_ties`,
    /// the 0.46.4 ceiling), `None` for a person.
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

/// The guest's tie on an open connection (the one tie store, skin.db step
/// 6: an owner reset or claim drops the row, so no row = a person).
pub(crate) fn tie_conn(conn: &Connection, pid: &str) -> Result<Option<super::ties::Tie>, String> {
    super::ties::get_conn(conn, pid)
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
            match tie.as_ref() {
                None => CLASS_OWNER,
                Some(t) if t.workspace_id == from && !from.is_empty() => {
                    if on_others {
                        CLASS_SHARED
                    } else if !t.reasons.is_empty() && t.reasons.iter().all(|r| r == super::ties::REASON_APP_KEY) {
                        CLASS_APP_KEY_TIED
                    } else {
                        CLASS_GIVER_TIED
                    }
                }
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
            tie: tie.map(|t| t.workspace_id),
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

// ── S3: ties and credentials (§4.4) ─────────────────────────────────
//
// An agent that set a guest's password must not keep control. So the
// guests the giving workspace could know the password of (`giver_tied`,
// and this App key's `app_key_tied`) are re-tied to the taker (their tie
// row and creator stamp say B) and their password is **retired**: the hash
// goes to the journal and is cleared, unused reset links are voided. Live
// sessions are kept: a session token is the guest's own bearer. When it
// ends the guest sets a new password (B's `k2 app user reset`, or the
// owner's). Shared guests (also on A's other Apps) stay A's.
//
// Shape (0.46.5 port): the 0.46.4 tie store is the ceiling
// (`principal_agent_ties.workspace_id`, `host_ok`, `reasons`): a tied
// guest signs in only to Apps of its tie's workspace. So the re-tie is
// load-bearing (without it every giver-tied guest is refused on B's App),
// and the ceiling's `app_inside` consults [`tie_inside`] so a tied guest's
// request between the daemon-DB commit and this transaction is never
// refused. A shared guest the owner takes over is an owner claim
// (`ties::owner_claim`'s steps, in this transaction).

/// skin.db DDL (run by the idempotent open block): the undo journal of a
/// handover, one row per guest it touched. Additive (an older K2 ignores
/// it: it only matters to undo). Password hashes are deleted when the undo
/// window ends.
pub(super) fn ensure_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS app_handover_journal (
            handover_id   TEXT NOT NULL,
            principal_id  TEXT NOT NULL REFERENCES principals(id) ON DELETE CASCADE,
            class         TEXT NOT NULL,
            tie_before    TEXT, -- the tie row as JSON (principal_agent_ties, step 6)
            stamp_before  TEXT,
            password_hash TEXT,
            claimed       INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (handover_id, principal_id)
         );",
    )
    .map_err(|e| format!("skin.db app_handover_journal: {e}"))
}

fn handing() -> &'static std::sync::Mutex<std::collections::HashMap<String, (String, String)>> {
    static H: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, (String, String)>>> = std::sync::OnceLock::new();
    H.get_or_init(Default::default)
}

/// While a handover of `service_id` runs from `from` to `to`, a tie to
/// either counts as inside this App (the gap between the daemon-DB commit
/// and the skin transaction, §4.4). Returns a guard that ends it.
pub fn begin_handing_over(service_id: &str, from: &str, to: &str) -> HandingOver {
    handing().lock().unwrap_or_else(|e| e.into_inner()).insert(service_id.to_string(), (from.to_string(), to.to_string()));
    HandingOver(service_id.to_string())
}

pub struct HandingOver(String);

impl Drop for HandingOver {
    fn drop(&mut self) {
        handing().lock().unwrap_or_else(|e| e.into_inner()).remove(&self.0);
    }
}

/// `(from, to)` while `service_id` is being handed over.
pub fn handing_over(service_id: &str) -> Option<(String, String)> {
    handing().lock().unwrap_or_else(|e| e.into_inner()).get(service_id).cloned()
}

/// Is a tie to `tie_ws` inside the App `service_id` of workspace
/// `app_ws`? Equal, or the two sides of a handover of it running now.
pub fn tie_inside(tie_ws: &str, service_id: &str, app_ws: &str) -> bool {
    if tie_ws == app_ws {
        return true;
    }
    handing_over(service_id).is_some_and(|(a, b)| (tie_ws == a || tie_ws == b) && (app_ws == a || app_ws == b))
}

/// What [`apply_guests`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GuestReport {
    pub rooms: RoomReport,
    pub retied: usize,
    pub retired: usize,
    pub reset_links_voided: usize,
    pub shared_kept: usize,
    pub shared_reset: usize,
}

/// The skin transaction of a handover (step 3): rooms (§4.3), ties and
/// credentials (§4.4), A's agent logins (P9), the journal. One
/// transaction; idempotent (a guest already re-tied is no longer
/// `giver_tied`). `reset_shared`: the owner chose to take the shared
/// guests over (their tie is dropped and their known password retired, so
/// they keep the App and set a new password).
pub fn apply_guests(service_id: &str, from: &str, to: &str, others_of_from: &[String], handover_id: &str, reset_shared: bool) -> Result<GuestReport, String> {
    with_conn(|conn| {
        let tx = conn.unchecked_transaction().map_err(|e| format!("handover guests: {e}"))?;
        let mut rep = GuestReport::default();
        let guests = guests_conn(&tx, service_id, from, others_of_from)?;
        for g in &guests {
            match g.class {
                CLASS_GIVER_TIED => {
                    journal_conn(&tx, handover_id, g, false)?;
                    retie_conn(&tx, &g.principal_id, from, to)?;
                    rep.retied += 1;
                    rep.reset_links_voided += retire_conn(&tx, &g.principal_id)?;
                    rep.retired += usize::from(g.has_password);
                }
                CLASS_APP_KEY_TIED => {
                    // The App key's tie names A's workspace (the App's at
                    // the time): the key is B's now, and so is the tie.
                    journal_conn(&tx, handover_id, g, false)?;
                    retie_conn(&tx, &g.principal_id, from, to)?;
                    rep.retied += 1;
                    rep.reset_links_voided += retire_conn(&tx, &g.principal_id)?;
                    rep.retired += usize::from(g.has_password);
                }
                CLASS_SHARED if reset_shared => {
                    // The owner takes the shared guest over: an owner claim
                    // (0.46.4, `ties::owner_claim`) in this transaction. The
                    // tie goes, the password A may know is wiped, every
                    // session ends (A could hold one it signed in with that
                    // password) and the links A could see are voided. They
                    // keep their grants and sign in again with the owner's
                    // new password or reset link.
                    journal_conn(&tx, handover_id, g, true)?;
                    super::ties::release_conn(&tx, &g.principal_id)?;
                    rep.reset_links_voided += retire_conn(&tx, &g.principal_id)?;
                    super::revoke_sessions_for_principal(&tx, &g.principal_id)?;
                    rep.shared_reset += 1;
                }
                CLASS_SHARED => rep.shared_kept += 1,
                _ => {}
            }
        }
        rep.rooms = move_rooms_conn(&tx, service_id, from, to, true)?;
        tx.commit().map_err(|e| format!("handover guests commit: {e}"))?;
        Ok(rep)
    })
}

/// Keep the before-values of one guest (first write wins: a roll-forward
/// never overwrites what the first run saw).
fn journal_conn(conn: &Connection, handover_id: &str, g: &HandoverGuest, claimed: bool) -> Result<(), String> {
    let tie: Option<String> = super::ties::get_conn(conn, &g.principal_id)?
        .map(|t| serde_json::to_string(&t).map_err(|e| format!("handover journal: {e}")))
        .transpose()?;
    let stamp = super::creators::workspace_of_conn(conn, &g.principal_id)?;
    let hash: Option<String> = conn
        .query_row("SELECT password_hash FROM principals WHERE id = ?1", params![g.principal_id], |r| r.get(0))
        .optional()
        .map_err(|e| format!("handover journal: {e}"))?
        .flatten();
    conn.execute(
        "INSERT OR IGNORE INTO app_handover_journal (handover_id, principal_id, class, tie_before, stamp_before, password_hash, claimed) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![handover_id, g.principal_id, g.class, tie, stamp, hash, i64::from(claimed)],
    )
    .map_err(|e| format!("handover journal: {e}"))?;
    Ok(())
}

/// The tie (its ceiling) and the creator stamp say `to` instead of
/// `from`. `host_ok`, the reasons and who set it are kept (the journal has
/// the whole row).
fn retie_conn(conn: &Connection, pid: &str, from: &str, to: &str) -> Result<(), String> {
    conn.execute("UPDATE principal_agent_ties SET workspace_id = ?3 WHERE principal_id = ?1 AND workspace_id = ?2", params![pid, from, to])
        .map_err(|e| format!("handover re-tie: {e}"))?;
    conn.execute("UPDATE principal_agent_creators SET workspace_id = ?3 WHERE principal_id = ?1 AND workspace_id = ?2", params![pid, from, to])
        .map_err(|e| format!("handover re-tie: {e}"))?;
    Ok(())
}

/// Retire what the giver could know: the password (cleared; live sessions
/// stay), every unused reset link, and every unused invite or App reset
/// link (`app_user_invites`, which an agent or the App could see; the
/// owner's own reset links stay, as an owner claim keeps them). Returns the
/// links voided.
fn retire_conn(conn: &Connection, pid: &str) -> Result<usize, String> {
    conn.execute("UPDATE principals SET password_hash = NULL WHERE id = ?1", params![pid]).map_err(|e| format!("handover retire: {e}"))?;
    let resets = conn
        .execute("UPDATE password_reset_tokens SET used_at = ?2 WHERE principal_id = ?1 AND used_at IS NULL", params![pid, super::now_secs()])
        .map_err(|e| format!("handover retire: {e}"))?;
    let invites = conn
        .execute("DELETE FROM app_user_invites WHERE principal_id = ?1 AND used_at IS NULL AND purpose != 'owner_reset'", params![pid])
        .map_err(|e| format!("handover retire (invites): {e}"))?;
    Ok(resets + invites)
}

/// Put back a tie row the journal kept (undo of an owner take-over). A tie
/// written since (a new tie after an owner reset) is left alone.
fn restore_tie_conn(conn: &Connection, pid: &str, before: &str) -> Result<(), String> {
    let t: serde_json::Value = serde_json::from_str(before).map_err(|e| format!("handover undo (tie): {e}"))?;
    let ws = t["workspaceId"].as_str().ok_or("handover undo (tie): no workspace in the journal")?;
    conn.execute(
        "INSERT OR IGNORE INTO principal_agent_ties (principal_id, workspace_id, host_ok, reasons, set_by, set_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            pid,
            ws,
            i64::from(t["hostOk"].as_bool() == Some(true)),
            t["reasons"].to_string(),
            t["setBy"].as_str().unwrap_or("handover:undo"),
            t["setAt"].as_i64().unwrap_or_else(super::now_secs),
        ],
    )
    .map_err(|e| format!("handover undo (tie): {e}"))?;
    Ok(())
}

/// One journal row (undo, tests).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JournalRow {
    pub principal_id: String,
    pub class: String,
    pub tie_before: Option<String>,
    pub stamp_before: Option<String>,
    #[serde(skip)]
    pub password_hash: Option<String>,
    pub claimed: bool,
}

pub fn journal(handover_id: &str) -> Result<Vec<JournalRow>, String> {
    with_conn(|conn| {
        let mut st = conn
            .prepare("SELECT principal_id, class, tie_before, stamp_before, password_hash, claimed FROM app_handover_journal WHERE handover_id = ?1 ORDER BY principal_id")
            .map_err(|e| format!("handover journal: {e}"))?;
        let out = st
            .query_map(params![handover_id], |r| {
                Ok(JournalRow {
                    principal_id: r.get(0)?,
                    class: r.get(1)?,
                    tie_before: r.get(2)?,
                    stamp_before: r.get(3)?,
                    password_hash: r.get(4)?,
                    claimed: r.get::<_, i64>(5)? != 0,
                })
            })
            .map_err(|e| format!("handover journal: {e}"))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| format!("handover journal: {e}"));
        out
    })
}

/// The undo window ended: the journal's credential material goes (T5.3).
/// Returns the rows cleared.
pub fn forget_journal(handover_id: &str) -> Result<usize, String> {
    with_conn(|conn| {
        conn.execute("DELETE FROM app_handover_journal WHERE handover_id = ?1", params![handover_id]).map_err(|e| format!("handover journal: {e}"))
    })
}

/// What [`undo_guests`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoReport {
    pub rooms: RoomReport,
    pub retied: usize,
    pub passwords_restored: usize,
    /// Guests who set a new password since: theirs is kept (CAS).
    pub passwords_kept: usize,
}

/// Undo a handover's skin half (§4.9): rooms B → A, ties and stamps back,
/// a retired hash restored only where the guest hasn't set a new password
/// since (compare-and-set on NULL). One transaction.
pub fn undo_guests(service_id: &str, from: &str, to: &str, handover_id: &str) -> Result<UndoReport, String> {
    let rows = journal(handover_id)?;
    with_conn(|conn| {
        let tx = conn.unchecked_transaction().map_err(|e| format!("handover undo: {e}"))?;
        let mut rep = UndoReport::default();
        for j in &rows {
            if j.class == CLASS_GIVER_TIED || j.class == CLASS_APP_KEY_TIED {
                retie_conn(&tx, &j.principal_id, to, from)?;
                rep.retied += 1;
            }
            if j.claimed {
                // The shared guest the owner took over is tied to A again,
                // as before (unless a tie was written since).
                if let Some(before) = j.tie_before.as_deref() {
                    restore_tie_conn(&tx, &j.principal_id, before)?;
                }
            }
            if let Some(h) = &j.password_hash {
                let n = tx
                    .execute("UPDATE principals SET password_hash = ?2 WHERE id = ?1 AND password_hash IS NULL", params![j.principal_id, h])
                    .map_err(|e| format!("handover undo: {e}"))?;
                if n == 1 {
                    rep.passwords_restored += 1;
                } else {
                    rep.passwords_kept += 1;
                }
            }
        }
        rep.rooms = move_rooms_conn(&tx, service_id, to, from, false)?;
        tx.commit().map_err(|e| format!("handover undo commit: {e}"))?;
        Ok(rep)
    })
}
