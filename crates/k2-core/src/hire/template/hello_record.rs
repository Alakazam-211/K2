//! The hello a template hire wrote, held by the daemon (migration 0331;
//! agent templates T9).
//!
//! A template hire's first create renders the template's hello into the
//! workspace's `.k2/agent/template.lock` as `hello_pending`, and records
//! its sha256 here, keyed by the workspace id. The agent can write its own
//! folder, its lock included, and anyone can put a lock in a folder before
//! it is added, so the lock alone never says the hello is K2's: the first
//! start delivers the lock's hello as the owner only when its sha256
//! matches this row ([`take`] removes the row in the same step, so it goes
//! once).

use rusqlite::{params, Connection};

fn store_err(e: rusqlite::Error) -> String {
    format!("template_hello_records: {e}")
}

/// The sha256 (hex) a hello is recorded and matched by.
pub fn sha(hello: &str) -> String {
    super::format::sha256_text(hello)
}

/// Record the hello a hire just wrote into `ws`'s lock (replacing any
/// earlier record for that workspace).
pub fn record(conn: &Connection, ws: &str, hello: &str, now: i64) -> Result<(), String> {
    conn.execute(
        "INSERT INTO template_hello_records (workspace_id, sha256, created_at) VALUES (?1, ?2, ?3) \
         ON CONFLICT (workspace_id) DO UPDATE SET sha256 = excluded.sha256, created_at = excluded.created_at",
        params![ws, sha(hello), now],
    )
    .map(|_| ())
    .map_err(store_err)
}

/// Take the record for `ws` when it is for exactly this hello. True once:
/// the row is removed. False when there is no row, or it is for another
/// text (the row then stays as it is).
pub fn take(conn: &Connection, ws: &str, hello: &str) -> Result<bool, String> {
    conn.execute("DELETE FROM template_hello_records WHERE workspace_id = ?1 AND sha256 = ?2", params![ws, sha(hello)])
        .map(|n| n == 1)
        .map_err(store_err)
}

/// A record is held for `ws` (tests and reports).
pub fn is_recorded(conn: &Connection, ws: &str) -> Result<bool, String> {
    conn.query_row("SELECT COUNT(*) FROM template_hello_records WHERE workspace_id = ?1", params![ws], |r| r.get::<_, i64>(0))
        .map(|n| n > 0)
        .map_err(store_err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hello_record_is_taken_once_and_only_for_its_text() {
        let _db = crate::db::scoped_for_test();
        let db = crate::db::shared();
        let conn = db.lock();
        record(&conn, "hw1", "Say hello.", 1).unwrap();
        assert!(!take(&conn, "hw1", "Say hello and more.").unwrap(), "another text");
        assert!(!take(&conn, "hw2", "Say hello.").unwrap(), "another workspace");
        assert!(is_recorded(&conn, "hw1").unwrap(), "a mismatch leaves the record");
        assert!(take(&conn, "hw1", "Say hello.").unwrap());
        assert!(!take(&conn, "hw1", "Say hello.").unwrap(), "once");
        assert!(!is_recorded(&conn, "hw1").unwrap());
    }

    #[test]
    fn a_failed_hello_record_read_is_an_error() {
        let conn = Connection::open_in_memory().unwrap();
        assert!(take(&conn, "w", "x").is_err(), "no table: an error, never a match");
    }
}
