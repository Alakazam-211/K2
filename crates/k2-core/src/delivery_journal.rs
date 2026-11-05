//! The delivery lanes' journal (migration 0267, 0.46.9).
//!
//! A Thread or project chat post answers once it is stored; the wake +
//! inject runs later on the agent's delivery lane (`k2-daemon`
//! `delivery_queue`, in memory). Each queued delivery is written here
//! first, so a daemon restart doesn't drop it: boot reads [`pending`] and
//! queues again what never started.
//!
//! - [`record`] when the delivery is queued, [`mark_started`] when its lane
//!   picks it up, [`finish`] when it is done (delivered or not: the
//!   outcome is in the Thread turn and the log, not here).
//! - A row with `started_at` set died mid-delivery: it may already be in
//!   the agent's terminal, so the daemon never types it in again.
//! - Rows are small (one line of text and its sender), and a row lives
//!   only while its delivery waits or runs.

use rusqlite::{params, Connection, OptionalExtension};

/// A Thread delivery (`overlay_routes`).
pub const KIND_THREAD: &str = "thread";
/// A project chat line to the point of contact (`project_group_routes`).
pub const KIND_PROJECT: &str = "project";

/// One journal row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalRow {
    pub id: String,
    pub lane: String,
    pub kind: String,
    pub body: String,
    pub queued_at: i64,
    pub started_at: Option<i64>,
}

/// Write a queued delivery. `id` is the caller's (a fresh uuid).
pub fn record(conn: &Connection, id: &str, lane: &str, kind: &str, body: &str, queued_at: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO delivery_journal (id, lane, kind, body, queued_at, started_at) VALUES (?1, ?2, ?3, ?4, ?5, NULL)",
        params![id, lane, kind, body, queued_at],
    )?;
    Ok(())
}

/// The lane picked the delivery up. False when the row is gone.
pub fn mark_started(conn: &Connection, id: &str, at: i64) -> rusqlite::Result<bool> {
    let n = conn.execute("UPDATE delivery_journal SET started_at = ?2 WHERE id = ?1", params![id, at])?;
    Ok(n == 1)
}

/// The delivery is done: its row goes. False when it was already gone.
pub fn finish(conn: &Connection, id: &str) -> rusqlite::Result<bool> {
    let n = conn.execute("DELETE FROM delivery_journal WHERE id = ?1", params![id])?;
    Ok(n == 1)
}

/// One row by id (tests, status).
pub fn get(conn: &Connection, id: &str) -> rusqlite::Result<Option<JournalRow>> {
    conn.query_row(
        "SELECT id, lane, kind, body, queued_at, started_at FROM delivery_journal WHERE id = ?1",
        params![id],
        row_of,
    )
    .optional()
}

/// Every row, oldest first (queue order: `queued_at`, then insert order).
pub fn pending(conn: &Connection) -> rusqlite::Result<Vec<JournalRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, lane, kind, body, queued_at, started_at FROM delivery_journal ORDER BY queued_at ASC, rowid ASC",
    )?;
    let rows = stmt.query_map([], row_of)?;
    rows.collect()
}

fn row_of(r: &rusqlite::Row<'_>) -> rusqlite::Result<JournalRow> {
    Ok(JournalRow {
        id: r.get(0)?,
        lane: r.get(1)?,
        kind: r.get(2)?,
        body: r.get(3)?,
        queued_at: r.get(4)?,
        started_at: r.get(5)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().expect("memory db");
        crate::db::run_migrations(&conn).expect("migrations");
        conn
    }

    #[test]
    fn a_row_lives_from_record_to_finish() {
        let conn = db();
        record(&conn, "j1", "ws:p1", KIND_THREAD, "{\"a\":1}", 100).expect("record");
        let row = get(&conn, "j1").expect("get").expect("the row is there");
        assert_eq!(row.lane, "ws:p1");
        assert_eq!(row.kind, KIND_THREAD);
        assert_eq!(row.body, "{\"a\":1}");
        assert_eq!(row.started_at, None, "queued, not started");

        assert!(mark_started(&conn, "j1", 150).expect("start"));
        assert_eq!(get(&conn, "j1").expect("get").expect("row").started_at, Some(150));

        assert!(finish(&conn, "j1").expect("finish"));
        assert_eq!(get(&conn, "j1").expect("get"), None, "a finished delivery leaves no row");
        assert!(!finish(&conn, "j1").expect("finish twice"), "a second finish finds nothing");
        assert!(!mark_started(&conn, "j1", 200).expect("start a gone row"), "a gone row can't start");
    }

    #[test]
    fn pending_is_in_queue_order() {
        let conn = db();
        record(&conn, "late", "ws:p1", KIND_THREAD, "{}", 300).expect("record");
        record(&conn, "first", "ws:p1", KIND_PROJECT, "{}", 100).expect("record");
        record(&conn, "second", "ws:p2", KIND_THREAD, "{}", 100).expect("record");
        let ids: Vec<String> = pending(&conn).expect("pending").into_iter().map(|r| r.id).collect();
        assert_eq!(ids, vec!["first", "second", "late"], "queued_at, then insert order");
    }

    #[test]
    fn an_unknown_kind_is_refused() {
        let conn = db();
        let err = record(&conn, "x", "ws:p1", "mail", "{}", 1);
        assert!(err.is_err(), "kind is thread or project only");
    }
}
