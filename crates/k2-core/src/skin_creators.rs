//! Which workspace's agent added a guest (security review, 0.46.4). The
//! owner's guests carry no stamp. An agent with the skin-manage switch may
//! act on a guest with no rooms, role or App yet only when its own
//! workspace's agent added it; an unstamped guest stays the owner's. Copy
//! of box logins onto an App leaves out guests another workspace's agent
//! added. The table is additive in skin.db (created at open, no numbered
//! step), and a guest's row goes with it.

use rusqlite::{params, Connection, OptionalExtension};

use super::{now_secs, with_conn};

/// skin.db DDL (run by the idempotent open block).
pub(super) fn ensure_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS principal_agent_creators (
            principal_id TEXT PRIMARY KEY REFERENCES principals(id) ON DELETE CASCADE,
            workspace_id TEXT NOT NULL,
            created_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS principal_person_claims (
            principal_id TEXT PRIMARY KEY REFERENCES principals(id) ON DELETE CASCADE,
            claimed_by TEXT NOT NULL,
            claimed_at INTEGER NOT NULL
         );",
    )
    .map_err(|e| format!("skin.db principal_agent_creators: {e}"))
}

/// The agent tie of a guest (0.46.4 security review B1): the workspace
/// whose agent added `principal_id`, while no person has taken the guest
/// over ([`clear_agent_tie`]). An agent could add a guest, set its
/// password and sign in as it, so a tied guest is not "a person" for
/// person-only actions (answering a card). `None`: the owner's guest, or
/// one a person claimed.
pub fn agent_tie(principal_id: &str) -> Result<Option<String>, String> {
    with_conn(|conn| {
        let Some(ws) = workspace_of_conn(conn, principal_id)? else { return Ok(None) };
        let claimed: Option<i64> = conn
            .query_row(
                "SELECT claimed_at FROM principal_person_claims WHERE principal_id = ?1",
                params![principal_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| format!("person claim lookup: {e}"))?;
        Ok(claimed.is_none().then_some(ws))
    })
}

/// A person took an agent-added guest over (reset its password, or claimed
/// it): from now on it counts as a person ([`agent_tie`] is `None`). `by`
/// names the person (`owner-token`, `user:<name>`). Never call it for an
/// agent actor. Idempotent.
pub fn clear_agent_tie(principal_id: &str, by: &str) -> Result<(), String> {
    let (pid, by) = (principal_id.trim().to_string(), by.trim().to_string());
    if pid.is_empty() || by.is_empty() || by.starts_with("agent:") {
        return Err("only a person clears a guest's agent tie".into());
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
    fn an_agent_added_guest_is_tied_until_a_person_claims_it() {
        let _home = crate::test_env::TempHome::new();
        let owners = super::super::add_principal("owners-guest").unwrap();
        let bots = super::super::add_principal("bots-guest").unwrap();
        stamp(&bots.id, "ws-agent").unwrap();
        assert_eq!(agent_tie(&owners.id).unwrap(), None, "the owner's guest is a person");
        assert_eq!(agent_tie(&bots.id).unwrap().as_deref(), Some("ws-agent"), "an agent-added guest is tied");
        assert!(clear_agent_tie(&bots.id, "agent:bot").is_err(), "an agent never clears its own tie");
        assert_eq!(agent_tie(&bots.id).unwrap().as_deref(), Some("ws-agent"));
        clear_agent_tie(&bots.id, "owner-token").unwrap();
        assert_eq!(agent_tie(&bots.id).unwrap(), None, "a person took it over");
        // The creator stamp itself stays (the agent's management rights are
        // another question).
        assert_eq!(workspace_of(&bots.id).unwrap().as_deref(), Some("ws-agent"));
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
