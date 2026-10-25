//! Which workspace's agent added a guest (security review, 0.46.4). The
//! owner's guests carry no stamp. An agent with the skin-manage switch may
//! act on a guest with no rooms, role or App yet only when its own
//! workspace's agent added it; an unstamped guest stays the owner's. Copy
//! of box logins onto an App leaves out guests another workspace's agent
//! added. The table is additive in skin.db (created at open, no numbered
//! step), and a guest's row goes with it.

use rusqlite::{params, Connection, OptionalExtension};

use super::{now_secs, with_conn};

/// skin.db DDL (run by the idempotent open block). `principal_agent_ties`
/// and `principal_person_claims` are additive too (an older K2 ignores
/// them).
pub(super) fn ensure_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS principal_agent_creators (
            principal_id TEXT PRIMARY KEY REFERENCES principals(id) ON DELETE CASCADE,
            workspace_id TEXT NOT NULL,
            created_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS principal_agent_ties (
            principal_id TEXT PRIMARY KEY REFERENCES principals(id) ON DELETE CASCADE,
            tied_to TEXT NOT NULL,
            tied_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS principal_person_claims (
            principal_id TEXT PRIMARY KEY REFERENCES principals(id) ON DELETE CASCADE,
            claimed_by TEXT NOT NULL,
            claimed_at INTEGER NOT NULL
         );",
    )
    .map_err(|e| format!("skin.db principal_agent_creators: {e}"))
}

// -- agent ties (0.46.4 security review B1, round 2) ------------------
//
// A guest is "tied" while something other than a person could know its
// password: the agent that added it (its creator stamp, above), an agent
// that set its password or email, or a `/v1` App key that created it or
// minted its reset token ([`tie`], in `principal_agent_ties`). A tied guest
// is not "a person" for person-only actions (answering a card). Only the
// owner lifts a tie ([`clear_agent_tie`]); a later re-key ties it again.
// The creator stamps stay a pure record of who added the guest: a tie never
// writes them.

fn claimed(conn: &Connection, principal_id: &str) -> Result<bool, String> {
    conn.query_row("SELECT 1 FROM principal_person_claims WHERE principal_id = ?1", params![principal_id], |_| Ok(()))
        .optional()
        .map(|o| o.is_some())
        .map_err(|e| format!("person claim lookup: {e}"))
}

/// The guest's tie: the FIRST workspace (or `app-key:<service id>`) it was
/// tied to, else the workspace whose agent added it; `None` for the
/// owner's untouched guest, or one the owner has taken over since its last
/// re-key.
pub fn agent_tie(principal_id: &str) -> Result<Option<String>, String> {
    with_conn(|conn| {
        if claimed(conn, principal_id)? {
            return Ok(None);
        }
        let tied: Option<String> = conn
            .query_row("SELECT tied_to FROM principal_agent_ties WHERE principal_id = ?1", params![principal_id], |r| r.get(0))
            .optional()
            .map_err(|e| format!("agent tie lookup: {e}"))?;
        match tied {
            Some(t) => Ok(Some(t)),
            None => workspace_of_conn(conn, principal_id),
        }
    })
}

/// What an App key's tie names instead of a workspace id.
pub fn app_key_marker(service_id: &str) -> String {
    format!("app-key:{}", service_id.trim())
}

/// Tie a guest whose password an agent or App key could now know (an agent
/// set its password or email: `tied_to` = that agent's workspace id; a
/// `/v1` App key created it or minted its reset token: [`app_key_marker`]).
/// The first tie is kept (a later one never overwrites it), and any person
/// claim is dropped: the guest is tied again. One transaction.
pub fn tie(principal_id: &str, tied_to: &str) -> Result<(), String> {
    let (pid, to) = (principal_id.trim().to_string(), tied_to.trim().to_string());
    if pid.is_empty() || to.is_empty() {
        return Err("a tie needs a guest and who it is tied to".into());
    }
    with_conn(|conn| {
        let tx = conn.unchecked_transaction().map_err(|e| format!("agent tie: {e}"))?;
        tx.execute(
            "INSERT INTO principal_agent_ties (principal_id, tied_to, tied_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(principal_id) DO NOTHING",
            params![pid, to, now_secs()],
        )
        .map_err(|e| format!("agent tie: {e}"))?;
        tx.execute("DELETE FROM principal_person_claims WHERE principal_id = ?1", params![pid])
            .map_err(|e| format!("agent tie: drop the person claim: {e}"))?;
        tx.commit().map_err(|e| format!("agent tie commit: {e}"))
    })
}

/// Who may lift a tie: ONLY the owner token (`owner-token`) or an Owner
/// Connect login (`user:<name>` whose role is Owner). Never the guest
/// itself (a reset link can reach the agent: its mailbox, its own App's
/// set-password page), never an Admin or Member login, an App key, a Garden
/// widget or an agent.
fn person_may_claim(by: &str) -> bool {
    let by = by.trim();
    by == "owner-token"
        || by.strip_prefix("user:").is_some_and(|n| crate::connect_users::role_for_user(n) == Some(crate::connect_users::Role::Owner))
}

/// The owner took a tied guest over: from now on it counts as a person
/// ([`agent_tie`] is `None`) until an agent or App key re-keys it. `by`
/// must be the owner token or an Owner login; anything else is refused and
/// nothing is written. Idempotent.
pub fn clear_agent_tie(principal_id: &str, by: &str) -> Result<(), String> {
    let (pid, by) = (principal_id.trim().to_string(), by.trim().to_string());
    if pid.is_empty() || !person_may_claim(&by) {
        return Err("only the owner (the owner token or an Owner login) clears a guest's agent tie".into());
    }
    with_conn(|conn| {
        conn.execute(
            "INSERT INTO principal_person_claims (principal_id, claimed_by, claimed_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(principal_id) DO UPDATE SET claimed_by = excluded.claimed_by, claimed_at = excluded.claimed_at",
            params![pid, by, now_secs()],
        )
        .map_err(|e| format!("person claim: {e}"))?;
        Ok(())
    })
}

#[cfg(test)]
mod tie_tests {
    use super::*;

    #[test]
    fn a_tied_guest_stays_tied_until_the_owner_claims_it() {
        let _home = crate::test_env::TempHome::new();
        let owners = super::super::add_principal("owners-guest").unwrap();
        let bots = super::super::add_principal("bots-guest").unwrap();
        stamp(&bots.id, "ws-agent").unwrap();
        assert_eq!(agent_tie(&owners.id).unwrap(), None, "the owner's guest is a person");
        assert_eq!(agent_tie(&bots.id).unwrap().as_deref(), Some("ws-agent"), "an agent-added guest is tied");
        // Only the owner token or an Owner login: never the guest itself, an
        // Admin or Member login, an App key, a Garden or an agent.
        let self_claim = format!("self:{}", bots.id);
        for by in ["agent:bot", "app-key:k1", "garden:board", "user:no-such-login", self_claim.as_str(), "", "owner"] {
            assert!(clear_agent_tie(&bots.id, by).is_err(), "{by:?} may not claim");
        }
        assert_eq!(agent_tie(&bots.id).unwrap().as_deref(), Some("ws-agent"));
        clear_agent_tie(&bots.id, "owner-token").unwrap();
        assert_eq!(agent_tie(&bots.id).unwrap(), None, "the owner took it over");
        // The creator stamp stays a record of who added the guest.
        assert_eq!(workspace_of(&bots.id).unwrap().as_deref(), Some("ws-agent"));
        // A re-key ties it again; the first tie is kept.
        tie(&bots.id, "ws-other").unwrap();
        assert_eq!(agent_tie(&bots.id).unwrap().as_deref(), Some("ws-other"), "re-keyed: tied again");
        tie(&bots.id, "ws-third").unwrap();
        assert_eq!(agent_tie(&bots.id).unwrap().as_deref(), Some("ws-other"), "a second tie never overwrites the first");
        // An App key's tie never touches the creator stamps.
        tie(&owners.id, &app_key_marker("svc-1")).unwrap();
        assert_eq!(agent_tie(&owners.id).unwrap().as_deref(), Some("app-key:svc-1"));
        assert_eq!(workspace_of(&owners.id).unwrap(), None, "no creator stamp for an App key's guest");
        assert_eq!(workspace_of(&bots.id).unwrap().as_deref(), Some("ws-agent"), "the agent's stamp is untouched");
    }
}

/// Stamp `principal_id` as added by the agent of `workspace_id`.
pub fn stamp(principal_id: &str, workspace_id: &str) -> Result<(), String> {
    let (pid, ws) = (principal_id.trim().to_string(), workspace_id.trim().to_string());
    if pid.is_empty() || ws.is_empty() {
        return Err("a creator stamp needs a guest and a workspace".into());
    }
    with_conn(|conn| {
        conn.execute(
            "INSERT INTO principal_agent_creators (principal_id, workspace_id, created_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(principal_id) DO UPDATE SET workspace_id = excluded.workspace_id",
            params![pid, ws, now_secs()],
        )
        .map_err(|e| format!("creator stamp: {e}"))?;
        Ok(())
    })
}

/// The workspace whose agent added `principal_id` (`None`: the owner's).
pub(super) fn workspace_of_conn(conn: &Connection, principal_id: &str) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT workspace_id FROM principal_agent_creators WHERE principal_id = ?1",
        params![principal_id],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .map_err(|e| format!("creator stamp lookup: {e}"))
}

/// [`workspace_of_conn`] on its own connection.
pub fn workspace_of(principal_id: &str) -> Result<Option<String>, String> {
    with_conn(|conn| workspace_of_conn(conn, principal_id))
}
