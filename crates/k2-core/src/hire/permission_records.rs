//! What a template hire left for later, held by the daemon (migration 0330;
//! prd-agent-templates-v1 T3, after the security review).
//!
//! - **card:** a suggested switch the hirer couldn't give, asked of a
//!   person on a permission card. `pending` while the new agent has no
//!   Thread (filed when it first starts), then `filed` with the request id,
//!   or `refused` when the card couldn't be filed. A re-apply never asks a
//!   switch with a row here again.
//! - **declined:** a suggestion the person left off at the hire. A
//!   re-apply leaves it off until a person names it again (`--allow`),
//!   and only one who could grant it with a role at least the decliner's
//!   ([`RANK_OWNER`] … [`RANK_AGENT`]): an Owner's decline needs an Owner.
//!
//! A read that fails is an error, never an empty list: an empty list
//! would drop held declines and let a re-run grant them.
//!
//! The new agent can write its own folder, `template.lock` included, so
//! the daemon decides from these rows only; the lock's copy is for people
//! to read.

use rusqlite::{params, Connection, OptionalExtension};

/// A card still to file at the agent's first start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingCard {
    pub id: i64,
    pub action: String,
    /// The template's display name (the card's words).
    pub template: String,
    /// The hirer's audit actor (`owner-token`, `user:<n>`, `agent:<ws>`,
    /// `api-key:<id>`).
    pub hirer: String,
}

/// Who declined, as a rank: a decline is lifted only by a role at least
/// this high.
pub const RANK_OWNER: i64 = 3;
pub const RANK_ADMIN: i64 = 2;
pub const RANK_MEMBER: i64 = 1;
/// An agent or an API key.
pub const RANK_AGENT: i64 = 0;

/// The rank's word, for refusals.
pub fn rank_word(rank: i64) -> &'static str {
    match rank {
        r if r >= RANK_OWNER => "an owner",
        RANK_ADMIN => "an admin",
        RANK_MEMBER => "a member",
        _ => "an agent",
    }
}

fn store_err(e: rusqlite::Error) -> String {
    format!("hire_permission_records: {e}")
}

fn list(conn: &Connection, sql: &str, ws: &str) -> Result<Vec<String>, String> {
    let mut st = conn.prepare(sql).map_err(store_err)?;
    let rows = st.query_map(params![ws], |r| r.get::<_, String>(0)).map_err(store_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(store_err)
}

/// Switches already asked on a card for this workspace: pending or filed.
/// A card whose filing was refused is not "asked", so a re-run asks again.
pub fn asked(conn: &Connection, ws: &str) -> Result<Vec<String>, String> {
    list(
        conn,
        "SELECT action FROM hire_permission_records WHERE workspace_id = ?1 AND kind = 'card' AND state != 'refused' ORDER BY id",
        ws,
    )
}

/// Suggestions declined at a hire of this workspace.
pub fn declined(conn: &Connection, ws: &str) -> Result<Vec<String>, String> {
    list(conn, "SELECT action FROM hire_permission_records WHERE workspace_id = ?1 AND kind = 'declined' ORDER BY id", ws)
}

/// The rank of whoever declined `action` here (`None`: not declined).
pub fn decline_rank(conn: &Connection, ws: &str, action: &str) -> Result<Option<i64>, String> {
    conn.query_row(
        "SELECT rank FROM hire_permission_records WHERE workspace_id = ?1 AND action = ?2 AND kind = 'declined'",
        params![ws, action],
        |r| r.get(0),
    )
    .optional()
    .map_err(store_err)
}

/// Record a decline with the decliner's rank. A second decline keeps the
/// higher rank (an Owner declining what a Member declined makes it the
/// Owner's).
pub fn record_declined(conn: &Connection, ws: &str, action: &str, template: &str, hirer: &str, rank: i64, now: i64) -> Result<(), String> {
    conn.execute(
        "INSERT INTO hire_permission_records (workspace_id, action, kind, state, template, hirer, created_at, rank) \
         VALUES (?1, ?2, 'declined', 'declined', ?3, ?4, ?5, ?6) \
         ON CONFLICT (workspace_id, action, kind) DO UPDATE SET \
           rank = excluded.rank, hirer = excluded.hirer WHERE excluded.rank > hire_permission_records.rank",
        params![ws, action, template, hirer, now, rank],
    )
    .map(|_| ())
    .map_err(store_err)
}

/// A person named a declined suggestion again: it is decided like any
/// other from here on.
pub fn undecline(conn: &Connection, ws: &str, action: &str) -> Result<bool, String> {
    conn.execute(
        "DELETE FROM hire_permission_records WHERE workspace_id = ?1 AND action = ?2 AND kind = 'declined'",
        params![ws, action],
    )
    .map(|n| n > 0)
    .map_err(|e| format!("hire_permission_records: {e}"))
}

/// Record a card: `pending` (filed at the first start) or `filed` now with
/// its request id. `false` when the switch was asked already.
#[allow(clippy::too_many_arguments)]
pub fn record_card(
    conn: &Connection,
    ws: &str,
    action: &str,
    pending: bool,
    template: &str,
    hirer: &str,
    card: Option<&str>,
    now: i64,
) -> Result<bool, String> {
    // A card whose filing was refused may be asked again: its row is
    // replaced. A pending or filed one is never asked twice.
    conn.execute(
        "INSERT INTO hire_permission_records (workspace_id, action, kind, state, template, hirer, created_at, done_at, card) \
         VALUES (?1, ?2, 'card', ?3, ?4, ?5, ?6, ?7, ?8) \
         ON CONFLICT (workspace_id, action, kind) DO UPDATE SET \
           state = excluded.state, template = excluded.template, hirer = excluded.hirer, \
           created_at = excluded.created_at, done_at = excluded.done_at, card = excluded.card \
         WHERE hire_permission_records.state = 'refused'",
        params![ws, action, if pending { "pending" } else { "filed" }, template, hirer, now, (!pending).then_some(now), card],
    )
    .map(|n| n > 0)
    .map_err(store_err)
}

/// The cards waiting for this workspace's first start, oldest first.
pub fn pending_cards(conn: &Connection, ws: &str) -> Vec<PendingCard> {
    let Ok(mut st) = conn.prepare(
        "SELECT id, action, template, hirer FROM hire_permission_records \
         WHERE workspace_id = ?1 AND kind = 'card' AND state = 'pending' ORDER BY id",
    ) else {
        return Vec::new();
    };
    st.query_map(params![ws], |r| Ok(PendingCard { id: r.get(0)?, action: r.get(1)?, template: r.get(2)?, hirer: r.get(3)? }))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

/// Claim one pending card for filing: only one caller ever wins it (the
/// row leaves `pending` first), so it is filed once.
pub fn claim(conn: &Connection, id: i64, now: i64) -> bool {
    conn.execute(
        "UPDATE hire_permission_records SET state = 'filed', done_at = ?2 WHERE id = ?1 AND state = 'pending'",
        params![id, now],
    )
    .is_ok_and(|n| n == 1)
}

/// After a claimed filing: its request id, or `refused`.
pub fn finish(conn: &Connection, id: i64, card: Option<&str>) -> Result<(), String> {
    let r = match card {
        Some(c) => conn.execute("UPDATE hire_permission_records SET card = ?2 WHERE id = ?1", params![id, c]),
        None => conn.execute("UPDATE hire_permission_records SET state = 'refused' WHERE id = ?1", params![id]),
    };
    r.map(|_| ()).map_err(|e| format!("hire_permission_records: {e}"))
}

/// The state of one record (tests and the report).
pub fn state_of(conn: &Connection, ws: &str, action: &str, kind: &str) -> Option<String> {
    conn.query_row(
        "SELECT state FROM hire_permission_records WHERE workspace_id = ?1 AND action = ?2 AND kind = ?3",
        params![ws, action, kind],
        |r| r.get(0),
    )
    .optional()
    .ok()
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_card_is_claimed_once_and_a_decline_is_undone_only_by_name() {
        let _db = crate::db::scoped_for_test();
        let db = crate::db::shared();
        let conn = db.lock();
        assert!(record_card(&conn, "w1", "compute.use", true, "Developer", "agent:w0", None, 1).unwrap());
        assert!(!record_card(&conn, "w1", "compute.use", true, "Developer", "agent:w0", None, 2).unwrap(), "asked once");
        assert_eq!(asked(&conn, "w1").unwrap(), vec!["compute.use"]);
        let p = pending_cards(&conn, "w1");
        assert_eq!(p.len(), 1);
        assert_eq!((p[0].template.as_str(), p[0].hirer.as_str()), ("Developer", "agent:w0"));
        assert!(claim(&conn, p[0].id, 3));
        assert!(!claim(&conn, p[0].id, 4), "never twice");
        finish(&conn, p[0].id, Some("p_1")).unwrap();
        assert!(pending_cards(&conn, "w1").is_empty());
        assert_eq!(state_of(&conn, "w1", "compute.use", "card").as_deref(), Some("filed"));
        assert!(pending_cards(&conn, "w2").is_empty(), "per workspace");

        record_declined(&conn, "w1", "apps.build", "App owner", "user:m", RANK_MEMBER, 5).unwrap();
        record_declined(&conn, "w1", "apps.build", "App owner", "owner-token", RANK_OWNER, 6).unwrap();
        record_declined(&conn, "w1", "apps.build", "App owner", "user:a", RANK_ADMIN, 7).unwrap();
        assert_eq!(declined(&conn, "w1").unwrap(), vec!["apps.build"]);
        assert_eq!(decline_rank(&conn, "w1", "apps.build").unwrap(), Some(RANK_OWNER), "the highest decliner holds it");
        assert_eq!(decline_rank(&conn, "w1", "vault.manage").unwrap(), None);
        assert!(undecline(&conn, "w1", "apps.build").unwrap());
        assert!(declined(&conn, "w1").unwrap().is_empty());
    }

    /// A card whose filing was refused is not "asked": a re-run records it
    /// again. A filed one is never asked twice.
    #[test]
    fn a_refused_card_may_be_asked_again() {
        let _db = crate::db::scoped_for_test();
        let db = crate::db::shared();
        let conn = db.lock();
        assert!(record_card(&conn, "w3", "vault.manage", true, "Developer", "owner-token", None, 1).unwrap());
        let p = pending_cards(&conn, "w3");
        assert!(claim(&conn, p[0].id, 2));
        finish(&conn, p[0].id, None).unwrap();
        assert_eq!(state_of(&conn, "w3", "vault.manage", "card").as_deref(), Some("refused"));
        assert!(asked(&conn, "w3").unwrap().is_empty(), "a refused filing is not asked");
        assert!(record_card(&conn, "w3", "vault.manage", true, "Developer", "owner-token", None, 3).unwrap(), "asked again");
        assert_eq!(state_of(&conn, "w3", "vault.manage", "card").as_deref(), Some("pending"));
        assert!(!record_card(&conn, "w3", "vault.manage", false, "Developer", "owner-token", Some("p_9"), 4).unwrap(), "pending: never twice");
    }

    /// A read that fails is an error, never an empty list (a dropped
    /// decline would be granted by the next re-run).
    #[test]
    fn a_failed_read_is_an_error() {
        let conn = Connection::open_in_memory().unwrap();
        assert!(declined(&conn, "w").is_err(), "no table: an error");
        assert!(asked(&conn, "w").is_err());
        assert!(decline_rank(&conn, "w", "x").is_err());
    }
}
