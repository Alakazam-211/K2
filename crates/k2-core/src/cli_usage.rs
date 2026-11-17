//! How the k5 CLI is used, as counted by the daemon (`cli_usage_counts`,
//! migration 0308; R1 slice 10).
//!
//! Three kinds of count, one row per (kind, key):
//!
//! - [`FALLTHROUGH`]: a call the native `k5` binary handed to the shell CLI,
//!   per CLI noun. At most [`MAX_NOUNS`] nouns are kept; past that a new
//!   noun counts as [`OTHER`], so a client can't grow the table without end.
//! - [`WOULD_REFUSE`]: an owner-token call made from inside an agent
//!   session, per workspace id (or [`OTHER`]).
//! - [`OWNER_FROM_DISK`]: an owner-token call that read the token from
//!   disk, per workspace id (or [`OTHER`]).
//!
//! The daemon counts in memory and writes here in batches ([`flush`]), never
//! on a request. Keys only: no arguments, paths, pids or tool names.

use rusqlite::{params, Connection};
use serde::Serialize;

pub const FALLTHROUGH: &str = "fallthrough";
pub const WOULD_REFUSE: &str = "would_refuse";
pub const OWNER_FROM_DISK: &str = "owner_from_disk";
/// The key for a noun past the cap, an invalid noun, or a workspace that
/// doesn't resolve.
pub const OTHER: &str = "other";
/// The most distinct fallthrough nouns kept (not counting [`OTHER`]).
pub const MAX_NOUNS: usize = 256;

/// One stored row, or a batch of counts waiting to be added to one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Row {
    pub kind: String,
    pub key: String,
    pub count: i64,
    pub first_at: i64,
    pub last_at: i64,
}

/// Is `noun` a CLI noun we keep as a key? `^[a-z][a-z0-9-]{0,31}$`.
pub fn valid_noun(noun: &str) -> bool {
    let b = noun.as_bytes();
    !b.is_empty()
        && b.len() <= 32
        && b[0].is_ascii_lowercase()
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// The key a fallthrough is counted under: the noun when valid, else
/// [`OTHER`] (absent, empty, too long, or any other character).
pub fn noun_key(noun: Option<&str>) -> &str {
    match noun {
        Some(n) if valid_noun(n) => n,
        _ => OTHER,
    }
}

/// Add each batch to its row in one transaction: counts summed, `first_at`
/// the earliest, `last_at` the latest. A fallthrough noun that has no row
/// yet while [`MAX_NOUNS`] nouns already do is added to [`OTHER`] instead.
pub fn flush(conn: &Connection, batch: &[Row]) -> rusqlite::Result<()> {
    if batch.is_empty() {
        return Ok(());
    }
    let tx = conn.unchecked_transaction()?;
    for row in batch {
        let key = if row.kind == FALLTHROUGH && row.key != OTHER && !has_row(&tx, FALLTHROUGH, &row.key)? {
            let nouns: i64 = tx.query_row(
                "SELECT COUNT(*) FROM cli_usage_counts WHERE kind = ?1 AND key != ?2",
                params![FALLTHROUGH, OTHER],
                |r| r.get(0),
            )?;
            if nouns as usize >= MAX_NOUNS {
                OTHER
            } else {
                row.key.as_str()
            }
        } else {
            row.key.as_str()
        };
        tx.execute(
            "INSERT INTO cli_usage_counts (kind, key, count, first_at, last_at) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(kind, key) DO UPDATE SET count = count + excluded.count,
               first_at = MIN(first_at, excluded.first_at), last_at = MAX(last_at, excluded.last_at)",
            params![row.kind, key, row.count, row.first_at, row.last_at],
        )?;
    }
    tx.commit()
}

fn has_row(conn: &Connection, kind: &str, key: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT COUNT(*) > 0 FROM cli_usage_counts WHERE kind = ?1 AND key = ?2",
        params![kind, key],
        |r| r.get(0),
    )
}

/// Every row, by kind then key.
pub fn list(conn: &Connection) -> rusqlite::Result<Vec<Row>> {
    let mut st = conn.prepare("SELECT kind, key, count, first_at, last_at FROM cli_usage_counts ORDER BY kind, key")?;
    let rows = st.query_map([], |r| {
        Ok(Row { kind: r.get(0)?, key: r.get(1)?, count: r.get(2)?, first_at: r.get(3)?, last_at: r.get(4)? })
    })?;
    rows.collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(include_str!("../drizzle_sql/0308_cli_usage_counts.sql")).unwrap();
        c
    }

    fn row(kind: &str, key: &str, count: i64, first_at: i64, last_at: i64) -> Row {
        Row { kind: kind.into(), key: key.into(), count, first_at, last_at }
    }

    #[test]
    fn nouns_are_validated_exactly() {
        for ok in ["a", "msg", "heartbeat", "k2-cli", "a0", &format!("a{}", "b".repeat(31))] {
            assert!(valid_noun(ok), "{ok}");
            assert_eq!(noun_key(Some(ok)), ok);
        }
        for bad in ["", "Msg", "0msg", "-msg", "msg_x", "msg x", "msg/x", "émail", &format!("a{}", "b".repeat(32))] {
            assert!(!valid_noun(bad), "{bad:?}");
            assert_eq!(noun_key(Some(bad)), OTHER, "{bad:?}");
        }
        assert_eq!(noun_key(None), OTHER);
    }

    #[test]
    fn flush_adds_counts_and_keeps_the_earliest_and_latest_times() {
        let c = conn();
        flush(&c, &[row(FALLTHROUGH, "msg", 2, 100, 150), row(WOULD_REFUSE, "ws-1", 1, 120, 120)]).unwrap();
        flush(&c, &[row(FALLTHROUGH, "msg", 3, 90, 140), row(OWNER_FROM_DISK, OTHER, 4, 200, 260)]).unwrap();
        flush(&c, &[]).unwrap();
        assert_eq!(
            list(&c).unwrap(),
            vec![
                row(FALLTHROUGH, "msg", 5, 90, 150),
                row(OWNER_FROM_DISK, OTHER, 4, 200, 260),
                row(WOULD_REFUSE, "ws-1", 1, 120, 120),
            ]
        );
    }

    #[test]
    fn past_the_noun_cap_a_new_noun_counts_as_other_and_a_known_one_still_counts() {
        let c = conn();
        let first: Vec<Row> = (0..MAX_NOUNS).map(|i| row(FALLTHROUGH, &format!("n{i}"), 1, 10, 10)).collect();
        flush(&c, &first).unwrap();
        flush(&c, &[row(FALLTHROUGH, "late", 2, 20, 20), row(FALLTHROUGH, "n7", 1, 30, 30)]).unwrap();
        let rows = list(&c).unwrap();
        let nouns: Vec<&Row> = rows.iter().filter(|r| r.kind == FALLTHROUGH && r.key != OTHER).collect();
        assert_eq!(nouns.len(), MAX_NOUNS, "the cap holds");
        assert!(!rows.iter().any(|r| r.key == "late"), "a noun past the cap is not kept");
        assert_eq!(rows.iter().find(|r| r.key == OTHER).cloned(), Some(row(FALLTHROUGH, OTHER, 2, 20, 20)));
        assert_eq!(rows.iter().find(|r| r.key == "n7").map(|r| r.count), Some(2), "a kept noun keeps counting");
        // The cap is per kind: workspace keys are not nouns.
        flush(&c, &[row(WOULD_REFUSE, "ws-9", 1, 40, 40)]).unwrap();
        assert!(list(&c).unwrap().iter().any(|r| r.kind == WOULD_REFUSE && r.key == "ws-9"));
    }

    #[test]
    fn the_table_refuses_unknown_kinds_and_empty_or_long_keys() {
        let c = conn();
        assert!(flush(&c, &[row("tool", "x", 1, 1, 1)]).is_err(), "unknown kind");
        assert!(flush(&c, &[row(WOULD_REFUSE, "", 1, 1, 1)]).is_err(), "empty key");
        assert!(flush(&c, &[row(WOULD_REFUSE, &"x".repeat(65), 1, 1, 1)]).is_err(), "key over 64");
        assert!(list(&c).unwrap().is_empty(), "a failed batch writes nothing");
    }
}
