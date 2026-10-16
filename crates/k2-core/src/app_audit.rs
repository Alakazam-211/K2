//! The per-call App audit (`prd-k2-app-runtime-v1` §5.5, S19; K16).
//!
//! Every App → gateway call writes one row: time, App, Seed, actor,
//! on-behalf-of, verb (method + route), room, result, bytes in/out and
//! latency. It lives in its own file, `~/.k2/apps/app-audit.db`, with its
//! own schema version (like `vault.db` and `skin.db`), so a busy App never
//! bloats `k2so.db`.
//!
//! **Hash-chained per day** (UTC): each row's
//! `hash = sha256(prev_hash || canonical row)`; the first row of a day
//! chains onto that day's genesis, `sha256("k2-app-audit-v1:" || day)`.
//! A row edited, removed or re-ordered breaks [`verify_day`] at that row,
//! and whole days can be rolled up and dropped ([`prune`]) without
//! breaking the days that remain.
//!
//! Retention: rows are kept [`KEEP_CALL_DAYS`] days, then rolled into
//! per-day, per-verb summaries kept [`KEEP_SUMMARY_DAYS`] days.

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};

/// `PRAGMA user_version` of a current file.
pub const SCHEMA_VERSION: i64 = 1;
/// Call rows are kept this many days (then summarised).
pub const KEEP_CALL_DAYS: i64 = 30;
/// Daily summaries are kept this many days.
pub const KEEP_SUMMARY_DAYS: i64 = 365;

/// `~/.k2/apps/app-audit.db`.
pub fn db_path() -> PathBuf {
    crate::paths::k2_home().join("apps").join("app-audit.db")
}

const SCHEMA_V1: &str = "
CREATE TABLE IF NOT EXISTS calls (
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  day TEXT NOT NULL,
  at_ms INTEGER NOT NULL,
  app_id TEXT NOT NULL,
  seed TEXT,
  actor TEXT NOT NULL,
  on_behalf_of TEXT,
  verb TEXT NOT NULL,
  room TEXT,
  result INTEGER NOT NULL,
  code TEXT,
  bytes_in INTEGER NOT NULL,
  bytes_out INTEGER NOT NULL,
  latency_ms INTEGER NOT NULL,
  prev_hash TEXT NOT NULL,
  hash TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS calls_app_at ON calls (app_id, at_ms);
CREATE INDEX IF NOT EXISTS calls_day_seq ON calls (day, seq);
CREATE TABLE IF NOT EXISTS daily (
  app_id TEXT NOT NULL,
  day TEXT NOT NULL,
  verb TEXT NOT NULL,
  calls INTEGER NOT NULL,
  denied INTEGER NOT NULL,
  bytes_in INTEGER NOT NULL,
  bytes_out INTEGER NOT NULL,
  last_hash TEXT,
  PRIMARY KEY (app_id, day, verb)
);
";

/// Open (and create or upgrade) the audit file at `path`. The file is
/// owner-only (0600) on Unix.
pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let conn = Connection::open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")?;
    migrate(&conn)?;
    Ok(conn)
}

/// Bring a connection's schema to [`SCHEMA_VERSION`]. A file from a newer
/// K2 (higher version) is refused rather than written with an old shape.
pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if v > SCHEMA_VERSION {
        return Err(rusqlite::Error::InvalidParameterName(format!(
            "app-audit.db is schema {v}; this K2 knows {SCHEMA_VERSION} (update K2)"
        )));
    }
    if v < 1 {
        conn.execute_batch(SCHEMA_V1)?;
        conn.execute_batch("PRAGMA user_version = 1")?;
    }
    Ok(())
}

/// One call, as the gateway saw it.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Call {
    pub at_ms: i64,
    pub app_id: String,
    pub seed: Option<String>,
    /// `app` | `guest:<principal>` | `agent:<ws>` | `remote`.
    pub actor: String,
    pub on_behalf_of: Option<String>,
    /// `METHOD /path` (no query string: it may carry a pass).
    pub verb: String,
    pub room: Option<String>,
    /// HTTP status (0 = the route answered in a way the gateway can't see,
    /// such as a file stream).
    pub result: u16,
    /// The refusal code, when there is one.
    pub code: Option<String>,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub latency_ms: u64,
}

/// One stored row.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub seq: i64,
    pub day: String,
    #[serde(flatten)]
    pub call: Call,
    pub prev_hash: String,
    pub hash: String,
}

/// The UTC day (`YYYY-MM-DD`) of a time in milliseconds.
pub fn day_of(at_ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(at_ms)
        .unwrap_or_default()
        .format("%Y-%m-%d")
        .to_string()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A day's first `prev_hash`.
pub fn genesis(day: &str) -> String {
    hex(&Sha256::digest(format!("k2-app-audit-v1:{day}").as_bytes()))
}

fn canonical(day: &str, c: &Call) -> String {
    serde_json::json!([
        day,
        c.at_ms,
        c.app_id,
        c.seed,
        c.actor,
        c.on_behalf_of,
        c.verb,
        c.room,
        c.result,
        c.code,
        c.bytes_in,
        c.bytes_out,
        c.latency_ms
    ])
    .to_string()
}

fn chain(prev: &str, canon: &str) -> String {
    let mut h = Sha256::new();
    h.update(prev.as_bytes());
    h.update(canon.as_bytes());
    hex(&h.finalize())
}

/// Append calls in one transaction; returns the last hash.
pub fn append(conn: &Connection, calls: &[Call]) -> rusqlite::Result<Option<String>> {
    if calls.is_empty() {
        return Ok(None);
    }
    let tx = conn.unchecked_transaction()?;
    let mut last = None;
    for c in calls {
        let day = day_of(c.at_ms);
        let prev: String = tx
            .query_row("SELECT hash FROM calls WHERE day = ?1 ORDER BY seq DESC LIMIT 1", params![day], |r| r.get(0))
            .optional()?
            .unwrap_or_else(|| genesis(&day));
        let hash = chain(&prev, &canonical(&day, c));
        tx.execute(
            "INSERT INTO calls (day, at_ms, app_id, seed, actor, on_behalf_of, verb, room, result, code, bytes_in, bytes_out, \
             latency_ms, prev_hash, hash) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                day,
                c.at_ms,
                c.app_id,
                c.seed,
                c.actor,
                c.on_behalf_of,
                c.verb,
                c.room,
                c.result as i64,
                c.code,
                c.bytes_in as i64,
                c.bytes_out as i64,
                c.latency_ms as i64,
                prev,
                hash
            ],
        )?;
        last = Some(hash);
    }
    tx.commit()?;
    Ok(last)
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    Ok(Row {
        seq: r.get(0)?,
        day: r.get(1)?,
        call: Call {
            at_ms: r.get(2)?,
            app_id: r.get(3)?,
            seed: r.get(4)?,
            actor: r.get(5)?,
            on_behalf_of: r.get(6)?,
            verb: r.get(7)?,
            room: r.get(8)?,
            result: r.get::<_, i64>(9)?.clamp(0, u16::MAX as i64) as u16,
            code: r.get(10)?,
            bytes_in: r.get::<_, i64>(11)?.max(0) as u64,
            bytes_out: r.get::<_, i64>(12)?.max(0) as u64,
            latency_ms: r.get::<_, i64>(13)?.max(0) as u64,
        },
        prev_hash: r.get(14)?,
        hash: r.get(15)?,
    })
}

const COLS: &str = "seq, day, at_ms, app_id, seed, actor, on_behalf_of, verb, room, result, code, bytes_in, bytes_out, latency_ms, prev_hash, hash";

/// Which rows [`list`] returns.
#[derive(Debug, Clone, Default)]
pub struct Query<'a> {
    pub app_id: &'a str,
    /// Only rows at or after this time (ms).
    pub since_ms: Option<i64>,
    /// Only rows whose verb contains this (`thread`, `POST /cli/fs`).
    pub verb: Option<&'a str>,
    /// Only refused calls (status ≥ 400).
    pub denied_only: bool,
    pub limit: i64,
}

/// Rows, newest first.
pub fn list(conn: &Connection, q: &Query<'_>) -> rusqlite::Result<Vec<Row>> {
    let mut st = conn.prepare(&format!(
        "SELECT {COLS} FROM calls WHERE app_id = ?1 AND (?2 IS NULL OR at_ms >= ?2) \
         AND (?3 IS NULL OR instr(verb, ?3) > 0) AND (?4 = 0 OR result >= 400) ORDER BY seq DESC LIMIT ?5"
    ))?;
    let rows = st.query_map(params![q.app_id, q.since_ms, q.verb, q.denied_only as i64, q.limit.clamp(1, 5000)], row)?;
    rows.collect()
}

/// Per-App totals since a time: (calls, denied, bytes in, bytes out).
pub fn totals(conn: &Connection, app_id: &str, since_ms: i64) -> rusqlite::Result<(i64, i64, i64, i64)> {
    conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(result >= 400), 0), COALESCE(SUM(bytes_in), 0), COALESCE(SUM(bytes_out), 0) \
         FROM calls WHERE app_id = ?1 AND at_ms >= ?2",
        params![app_id, since_ms],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )
}

/// Daily summaries for an App, newest first.
pub fn summaries(conn: &Connection, app_id: &str, limit: i64) -> rusqlite::Result<Vec<serde_json::Value>> {
    let mut st = conn.prepare(
        "SELECT day, verb, calls, denied, bytes_in, bytes_out FROM daily WHERE app_id = ?1 ORDER BY day DESC, verb LIMIT ?2",
    )?;
    let rows = st.query_map(params![app_id, limit.clamp(1, 5000)], |r| {
        Ok(serde_json::json!({
            "day": r.get::<_, String>(0)?,
            "verb": r.get::<_, String>(1)?,
            "calls": r.get::<_, i64>(2)?,
            "denied": r.get::<_, i64>(3)?,
            "bytesIn": r.get::<_, i64>(4)?,
            "bytesOut": r.get::<_, i64>(5)?,
        }))
    })?;
    rows.collect()
}

/// Check one day's chain. `Ok(n)` = `n` rows verified; `Err(seq)` = the
/// first row that doesn't chain (edited, removed or re-ordered).
pub fn verify_day(conn: &Connection, day: &str) -> rusqlite::Result<Result<usize, i64>> {
    let mut st = conn.prepare(&format!("SELECT {COLS} FROM calls WHERE day = ?1 ORDER BY seq"))?;
    let rows = st.query_map(params![day], row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    let mut prev = genesis(day);
    for r in &rows {
        if r.prev_hash != prev || chain(&prev, &canonical(day, &r.call)) != r.hash {
            return Ok(Err(r.seq));
        }
        prev = r.hash.clone();
    }
    Ok(Ok(rows.len()))
}

/// The days that still have call rows, oldest first.
pub fn days(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut st = conn.prepare("SELECT DISTINCT day FROM calls ORDER BY day")?;
    let rows = st.query_map([], |r| r.get(0))?;
    rows.collect()
}

/// Roll call rows older than [`KEEP_CALL_DAYS`] into daily summaries (with
/// each day's last hash), delete them, and drop summaries older than
/// [`KEEP_SUMMARY_DAYS`]. Whole days only, so the remaining chains verify.
/// Returns how many call rows were rolled up.
pub fn prune(conn: &Connection, now_ms: i64) -> rusqlite::Result<usize> {
    let cut = day_of(now_ms - KEEP_CALL_DAYS * 86_400_000);
    let cut_summary = day_of(now_ms - KEEP_SUMMARY_DAYS * 86_400_000);
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO daily (app_id, day, verb, calls, denied, bytes_in, bytes_out, last_hash) \
         SELECT app_id, day, verb, COUNT(*), SUM(result >= 400), SUM(bytes_in), SUM(bytes_out), \
                (SELECT c2.hash FROM calls c2 WHERE c2.day = calls.day ORDER BY c2.seq DESC LIMIT 1) \
         FROM calls WHERE day < ?1 GROUP BY app_id, day, verb \
         ON CONFLICT (app_id, day, verb) DO UPDATE SET calls = calls + excluded.calls, denied = denied + excluded.denied, \
           bytes_in = bytes_in + excluded.bytes_in, bytes_out = bytes_out + excluded.bytes_out, last_hash = excluded.last_hash",
        params![cut],
    )?;
    let n = tx.execute("DELETE FROM calls WHERE day < ?1", params![cut])?;
    tx.execute("DELETE FROM daily WHERE day < ?1", params![cut_summary])?;
    tx.commit()?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        migrate(&c).unwrap();
        c
    }

    fn call(at_ms: i64, app: &str, verb: &str, result: u16) -> Call {
        Call {
            at_ms,
            app_id: app.into(),
            seed: Some("seed-1".into()),
            actor: "app".into(),
            verb: verb.into(),
            room: Some("ws-1".into()),
            result,
            code: (result >= 400).then(|| "denied".into()),
            bytes_in: 10,
            bytes_out: 20,
            latency_ms: 3,
            ..Default::default()
        }
    }

    const DAY1: i64 = 1_760_000_000_000; // 2025-10-09
    const DAY: i64 = 86_400_000;

    #[test]
    fn schema_is_versioned_and_a_newer_file_is_refused() {
        let c = mem();
        let v: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        migrate(&c).unwrap();
        c.execute_batch("PRAGMA user_version = 99").unwrap();
        assert!(migrate(&c).is_err(), "a newer schema is refused");
    }

    #[test]
    fn rows_chain_per_day_and_tampering_breaks_the_chain() {
        let c = mem();
        append(&c, &[call(DAY1, "jobs", "GET /cli/thread", 200), call(DAY1 + 5, "other", "POST /cli/fs/write-file", 403)]).unwrap();
        append(&c, &[call(DAY1 + 10, "jobs", "GET /cli/thread", 200)]).unwrap();
        append(&c, &[call(DAY1 + DAY, "jobs", "GET /cli/thread", 200)]).unwrap();
        let d1 = day_of(DAY1);
        let d2 = day_of(DAY1 + DAY);
        assert_eq!(days(&c).unwrap(), vec![d1.clone(), d2.clone()]);
        assert_eq!(verify_day(&c, &d1).unwrap(), Ok(3));
        assert_eq!(verify_day(&c, &d2).unwrap(), Ok(1));
        let first2: String = c.query_row("SELECT prev_hash FROM calls WHERE day = ?1", params![d2], |r| r.get(0)).unwrap();
        assert_eq!(first2, genesis(&d2), "each day starts at its own genesis");

        // Filters.
        let jobs = list(&c, &Query { app_id: "jobs", limit: 50, ..Default::default() }).unwrap();
        assert_eq!(jobs.len(), 3);
        assert!(jobs[0].seq > jobs[1].seq, "newest first");
        let denied = list(&c, &Query { app_id: "other", denied_only: true, limit: 50, ..Default::default() }).unwrap();
        assert_eq!(denied.len(), 1);
        assert_eq!(denied[0].call.code.as_deref(), Some("denied"));
        let fs = list(&c, &Query { app_id: "other", verb: Some("fs/"), limit: 50, ..Default::default() }).unwrap();
        assert_eq!(fs.len(), 1);
        let since = list(&c, &Query { app_id: "jobs", since_ms: Some(DAY1 + 6), limit: 50, ..Default::default() }).unwrap();
        assert_eq!(since.len(), 2);
        assert_eq!(totals(&c, "jobs", 0).unwrap(), (3, 0, 30, 60));

        // Edit a row: the chain breaks at that row.
        let seq: i64 = c.query_row("SELECT seq FROM calls WHERE day = ?1 ORDER BY seq LIMIT 1 OFFSET 1", params![d1], |r| r.get(0)).unwrap();
        c.execute("UPDATE calls SET result = 200 WHERE seq = ?1", params![seq]).unwrap();
        assert_eq!(verify_day(&c, &d1).unwrap(), Err(seq));
        // Remove a row: the next one no longer chains.
        let c2 = mem();
        append(&c2, &[call(DAY1, "jobs", "a", 200), call(DAY1 + 1, "jobs", "b", 200), call(DAY1 + 2, "jobs", "c", 200)]).unwrap();
        let mid: i64 = c2.query_row("SELECT seq FROM calls WHERE verb = 'b'", [], |r| r.get(0)).unwrap();
        c2.execute("DELETE FROM calls WHERE seq = ?1", params![mid]).unwrap();
        assert_eq!(verify_day(&c2, &d1).unwrap(), Err(mid + 1));
    }

    #[test]
    fn prune_rolls_old_days_into_summaries_and_keeps_the_rest_verifiable() {
        let c = mem();
        let now = DAY1 + 40 * DAY;
        append(&c, &[call(DAY1, "jobs", "GET /cli/thread", 200), call(DAY1 + 1, "jobs", "GET /cli/thread", 403)]).unwrap();
        append(&c, &[call(now - DAY, "jobs", "GET /cli/thread", 200)]).unwrap();
        assert_eq!(prune(&c, now).unwrap(), 2);
        let s = summaries(&c, "jobs", 10).unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!((s[0]["calls"].as_i64(), s[0]["denied"].as_i64(), s[0]["bytesIn"].as_i64()), (Some(2), Some(1), Some(20)));
        assert_eq!(days(&c).unwrap(), vec![day_of(now - DAY)]);
        assert_eq!(verify_day(&c, &day_of(now - DAY)).unwrap(), Ok(1));
        // A year later the first day's summary goes too.
        prune(&c, DAY1 + 400 * DAY).unwrap();
        let left = summaries(&c, "jobs", 10).unwrap();
        assert!(!left.iter().any(|s| s["day"] == day_of(DAY1).as_str()), "{left:?}");
        assert!(days(&c).unwrap().is_empty(), "every call row is rolled up after 30 days");
    }

    #[test]
    fn open_creates_an_owner_only_file() {
        let dir = std::env::temp_dir().join(format!("k2-app-audit-{}", uuid::Uuid::new_v4().simple()));
        let p = dir.join("apps").join("app-audit.db");
        let c = open(&p).unwrap();
        append(&c, &[call(DAY1, "jobs", "GET /cli/thread", 200)]).unwrap();
        drop(c);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let c = open(&p).unwrap();
        assert_eq!(list(&c, &Query { app_id: "jobs", limit: 5, ..Default::default() }).unwrap().len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
