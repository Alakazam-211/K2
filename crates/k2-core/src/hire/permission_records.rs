//! What a template hire left for later, held by the daemon (migration 0330;
//! prd-agent-templates-v1 T3, after the security review).
//!
//! - **card:** a suggested switch the hirer couldn't give, asked of a
//!   person on a permission card. `pending` while the new agent has no
//!   Thread (filed when it first starts), then `filed` with the request id,
//!   or `refused` when the card couldn't be filed. A re-apply never asks a
//!   switch with a row here again.
//! - **declined:** a suggestion the person left off at the hire. A
//!   re-apply leaves it off until a person names it again (`--allow`).
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

fn list(conn: &Connection, ws: &str, kind: &str) -> Vec<String> {
    let Ok(mut st) = conn.prepare("SELECT action FROM hire_permission_records WHERE workspace_id = ?1 AND kind = ?2 ORDER BY id") else {
        return Vec::new();
    };
    st.query_map(params![ws, kind], |r| r.get::<_, String>(0)).map(|rows| rows.flatten().collect()).unwrap_or_default()
}

/// Switches already asked on a card for this workspace (any state).
pub fn asked(conn: &Connection, ws: &str) -> Vec<String> {
    list(conn, ws, "card")
}

/// Suggestions the person declined at a hire of this workspace.
pub fn declined(conn: &Connection, ws: &str) -> Vec<String> {
    list(conn, ws, "declined")
}

/// Record a decline (a second one is a no-op).
pub fn record_declined(conn: &Connection, ws: &str, action: &str, template: &str, hirer: &str, now: i64) -> Result<(), String> {
    conn.execute(
        "INSERT OR IGNORE INTO hire_permission_records (workspace_id, action, kind, state, template, hirer, created_at) \
         VALUES (?1, ?2, 'declined', 'declined', ?3, ?4, ?5)",
        params![ws, action, template, hirer, now],
    )
    .map(|_| ())
    .map_err(|e| format!("hire_permission_records: {e}"))
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
    conn.execute(
        "INSERT OR IGNORE INTO hire_permission_records (workspace_id, action, kind, state, template, hirer, created_at, done_at, card) \
         VALUES (?1, ?2, 'card', ?3, ?4, ?5, ?6, ?7, ?8)",
        params![ws, action, if pending { "pending" } else { "filed" }, template, hirer, now, (!pending).then_some(now), card],
    )
    .map(|n| n > 0)
    .map_err(|e| format!("hire_permission_records: {e}"))
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
        assert_eq!(asked(&conn, "w1"), vec!["compute.use"]);
        let p = pending_cards(&conn, "w1");
        assert_eq!(p.len(), 1);
        assert_eq!((p[0].template.as_str(), p[0].hirer.as_str()), ("Developer", "agent:w0"));
        assert!(claim(&conn, p[0].id, 3));
        assert!(!claim(&conn, p[0].id, 4), "never twice");
        finish(&conn, p[0].id, Some("p_1")).unwrap();
        assert!(pending_cards(&conn, "w1").is_empty());
        assert_eq!(state_of(&conn, "w1", "compute.use", "card").as_deref(), Some("filed"));
        assert!(pending_cards(&conn, "w2").is_empty(), "per workspace");

        record_declined(&conn, "w1", "apps.build", "App owner", "owner-token", 5).unwrap();
        record_declined(&conn, "w1", "apps.build", "App owner", "owner-token", 6).unwrap();
        assert_eq!(declined(&conn, "w1"), vec!["apps.build"]);
        assert!(undecline(&conn, "w1", "apps.build").unwrap());
        assert!(declined(&conn, "w1").is_empty());
    }
}
