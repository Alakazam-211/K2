//! `state/queue.sqlite` (prd-compute-shared-queue-v1 §11.1): what the node
//! queue remembers across restarts. Node-only state (0700 dir), never sent
//! anywhere as is.
//!
//! - `usage(slot, decayed_cpu_ms, updated_at)`: decayed CPU use per server,
//!   millicore-seconds (24 h half-life, applied on read);
//! - `usage_day(slot, day, cpu_secs, wall_secs, jobs)`: plain daily totals
//!   (never billed);
//! - `durations(slot, cmd_hash, n, p50, p90, samples)`: run times of the
//!   same command per server (ETAs), the last [`MAX_SAMPLES`];
//! - `asks(slot, cpu, mem, last_at)`: each server's recent job sizes (its
//!   four quotes);
//! - `meta(k, v)`: the rotation (`last_served`, `last_served_ws:<slot>`,
//!   `last_served_submitter:<slot>:<ws>`, `last_served_line`; 0.46.5's
//!   `last_served_session:` keys are read once, then replaced).

use std::collections::BTreeMap;
use std::path::Path;

use k2_node_proto::queue::Rotation;
use k2_node_proto::tiers::Line;
use k2_node_proto::share::{Slot, MAX_QUOTES};
use rusqlite::{params, Connection, OptionalExtension};

/// Samples kept per command.
pub const MAX_SAMPLES: usize = 20;

pub struct QueueStore {
    conn: Connection,
}

fn sqlerr(e: rusqlite::Error) -> String {
    format!("queue store: {e}")
}

/// The UTC day of a unix time, `YYYY-MM-DD`.
pub fn day_of(t: i64) -> String {
    let days = t.div_euclid(86400);
    // Civil from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{:02}-{:02}", if m <= 2 { y + 1 } else { y }, m, d)
}

/// p50 and p90 of some samples (nearest rank).
pub fn percentiles(samples: &[u64]) -> Option<(u64, u64)> {
    if samples.is_empty() {
        return None;
    }
    let mut s = samples.to_vec();
    s.sort_unstable();
    let rank = |p: usize| s[((p * s.len()).div_ceil(100)).saturating_sub(1).min(s.len() - 1)];
    Some((rank(50), rank(90)))
}

impl QueueStore {
    pub fn open(path: &Path) -> Result<Self, String> {
        Self::init(Connection::open(path).map_err(sqlerr)?)
    }

    pub fn open_in_memory() -> Result<Self, String> {
        Self::init(Connection::open_in_memory().map_err(sqlerr)?)
    }

    fn init(conn: Connection) -> Result<Self, String> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             CREATE TABLE IF NOT EXISTS usage (slot INTEGER PRIMARY KEY, decayed_cpu_ms INTEGER NOT NULL, updated_at INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS usage_day (slot INTEGER NOT NULL, day TEXT NOT NULL, cpu_secs INTEGER NOT NULL DEFAULT 0,
               wall_secs INTEGER NOT NULL DEFAULT 0, jobs INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (slot, day));
             CREATE TABLE IF NOT EXISTS durations (slot INTEGER NOT NULL, cmd_hash TEXT NOT NULL, n INTEGER NOT NULL,
               p50 INTEGER NOT NULL, p90 INTEGER NOT NULL, samples TEXT NOT NULL, PRIMARY KEY (slot, cmd_hash));
             CREATE TABLE IF NOT EXISTS asks (slot INTEGER NOT NULL, cpu INTEGER NOT NULL, mem INTEGER NOT NULL,
               last_at INTEGER NOT NULL, PRIMARY KEY (slot, cpu, mem));
             CREATE TABLE IF NOT EXISTS meta (k TEXT PRIMARY KEY, v TEXT NOT NULL);",
        )
        .map_err(sqlerr)?;
        Ok(Self { conn })
    }

    /// `servers.owner` (prd-macos-tenant-compute-v1 §6.2 step 4): slots `a`
    /// and `b` trade their usage, daily totals, run times and asks; the
    /// rotation marks start over. Exactly once per `id` (recorded in the
    /// same transaction): `Ok(false)` when it already ran.
    pub fn swap_slots(&mut self, a: Slot, b: Slot, id: &str) -> Result<bool, String> {
        const PARK: i64 = 100;
        let tx = self.conn.transaction().map_err(sqlerr)?;
        let done: Option<String> = tx.query_row("SELECT v FROM meta WHERE k = 'owner_change'", [], |r| r.get(0)).optional().map_err(sqlerr)?;
        if done.as_deref() == Some(id) {
            return Ok(false);
        }
        for t in ["usage", "usage_day", "durations", "asks"] {
            tx.execute(&format!("UPDATE {t} SET slot = ?1 WHERE slot = ?2"), params![PARK, a]).map_err(sqlerr)?;
            tx.execute(&format!("UPDATE {t} SET slot = ?1 WHERE slot = ?2"), params![a, b]).map_err(sqlerr)?;
            tx.execute(&format!("UPDATE {t} SET slot = ?1 WHERE slot = ?2"), params![b, PARK]).map_err(sqlerr)?;
        }
        tx.execute("DELETE FROM meta WHERE k LIKE 'last_served%'", []).map_err(sqlerr)?;
        tx.execute("INSERT INTO meta (k, v) VALUES ('owner_change', ?1) ON CONFLICT(k) DO UPDATE SET v = ?1", params![id]).map_err(sqlerr)?;
        tx.commit().map_err(sqlerr)?;
        Ok(true)
    }

    /// Every server's decayed usage `(value, updated_at)`.
    pub fn usage(&self) -> Result<BTreeMap<Slot, (u64, i64)>, String> {
        let mut st = self.conn.prepare("SELECT slot, decayed_cpu_ms, updated_at FROM usage").map_err(sqlerr)?;
        let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)? as Slot, (r.get::<_, i64>(1)? as u64, r.get::<_, i64>(2)?)))).map_err(sqlerr)?;
        rows.collect::<Result<_, _>>().map_err(sqlerr)
    }

    pub fn set_usage(&self, slot: Slot, decayed_cpu_ms: u64, at: i64) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO usage (slot, decayed_cpu_ms, updated_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(slot) DO UPDATE SET decayed_cpu_ms = ?2, updated_at = ?3",
                params![slot, decayed_cpu_ms as i64, at],
            )
            .map_err(sqlerr)?;
        Ok(())
    }

    /// Add to a server's daily totals.
    pub fn add_day(&self, slot: Slot, day: &str, cpu_secs: u64, wall_secs: u64, jobs: u64) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO usage_day (slot, day, cpu_secs, wall_secs, jobs) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(slot, day) DO UPDATE SET cpu_secs = cpu_secs + ?3, wall_secs = wall_secs + ?4, jobs = jobs + ?5",
                params![slot, day, cpu_secs as i64, wall_secs as i64, jobs as i64],
            )
            .map_err(sqlerr)?;
        Ok(())
    }

    /// CPU-seconds a server used on `day`.
    pub fn day_cpu_secs(&self, slot: Slot, day: &str) -> Result<u64, String> {
        self.conn
            .query_row("SELECT cpu_secs FROM usage_day WHERE slot = ?1 AND day = ?2", params![slot, day], |r| r.get::<_, i64>(0))
            .optional()
            .map(|v| v.unwrap_or(0) as u64)
            .map_err(sqlerr)
    }

    /// Record one run time of `cmd_hash` for `slot`.
    pub fn add_duration(&self, slot: Slot, cmd_hash: &str, secs: u64) -> Result<(), String> {
        let prev: Option<String> = self
            .conn
            .query_row("SELECT samples FROM durations WHERE slot = ?1 AND cmd_hash = ?2", params![slot, cmd_hash], |r| r.get(0))
            .optional()
            .map_err(sqlerr)?;
        let mut samples: Vec<u64> = prev.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        samples.push(secs);
        if samples.len() > MAX_SAMPLES {
            samples.drain(..samples.len() - MAX_SAMPLES);
        }
        let (p50, p90) = percentiles(&samples).expect("non-empty");
        self.conn
            .execute(
                "INSERT INTO durations (slot, cmd_hash, n, p50, p90, samples) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(slot, cmd_hash) DO UPDATE SET n = ?3, p50 = ?4, p90 = ?5, samples = ?6",
                params![slot, cmd_hash, samples.len() as i64, p50 as i64, p90 as i64, serde_json::to_string(&samples).unwrap_or_default()],
            )
            .map_err(sqlerr)?;
        Ok(())
    }

    /// `(p50, p90)` run time of `cmd_hash` for `slot`, if known.
    pub fn duration(&self, slot: Slot, cmd_hash: &str) -> Result<Option<(u64, u64)>, String> {
        self.conn
            .query_row("SELECT p50, p90 FROM durations WHERE slot = ?1 AND cmd_hash = ?2", params![slot, cmd_hash], |r| {
                Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64))
            })
            .optional()
            .map_err(sqlerr)
    }

    /// Note a job size `slot` asked for; keeps its last [`MAX_QUOTES`]
    /// distinct sizes.
    pub fn add_ask(&self, slot: Slot, cpu: u64, mem: u64, at: i64) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO asks (slot, cpu, mem, last_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(slot, cpu, mem) DO UPDATE SET last_at = ?4",
                params![slot, cpu as i64, mem as i64, at],
            )
            .map_err(sqlerr)?;
        self.conn
            .execute(
                "DELETE FROM asks WHERE slot = ?1 AND rowid NOT IN
                   (SELECT rowid FROM asks WHERE slot = ?1 ORDER BY last_at DESC, rowid DESC LIMIT ?2)",
                params![slot, MAX_QUOTES as i64],
            )
            .map_err(sqlerr)?;
        Ok(())
    }

    /// `slot`'s last distinct sizes `(cpu, mem)`, newest first.
    pub fn asks(&self, slot: Slot) -> Result<Vec<(u64, u64)>, String> {
        let mut st = self.conn.prepare("SELECT cpu, mem FROM asks WHERE slot = ?1 ORDER BY last_at DESC, rowid DESC").map_err(sqlerr)?;
        let rows = st.query_map(params![slot], |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64))).map_err(sqlerr)?;
        rows.collect::<Result<_, _>>().map_err(sqlerr)
    }

    /// Forget a server (its slot was wiped).
    pub fn forget(&self, slot: Slot) -> Result<(), String> {
        for t in ["usage", "usage_day", "durations", "asks"] {
            self.conn.execute(&format!("DELETE FROM {t} WHERE slot = ?1"), params![slot]).map_err(sqlerr)?;
        }
        let mut rot = self.rotation()?;
        rot.last_ws.remove(&slot);
        rot.last_submitter.retain(|(s, _), _| *s != slot);
        if rot.last_slot == Some(slot) {
            rot.last_slot = None;
        }
        self.set_rotation(&rot)
    }

    pub fn rotation(&self) -> Result<Rotation, String> {
        let mut st = self.conn.prepare("SELECT k, v FROM meta WHERE k LIKE 'last_served%'").map_err(sqlerr)?;
        let rows: Vec<(String, String)> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(sqlerr)?.collect::<Result<_, _>>().map_err(sqlerr)?;
        let mut rot = Rotation::default();
        for (k, v) in rows {
            if k == "last_served" {
                rot.last_slot = v.parse().ok();
            } else if let Some(slot) = k.strip_prefix("last_served_ws:").and_then(|s| s.parse::<Slot>().ok()) {
                rot.last_ws.insert(slot, v);
            } else if k == "last_served_line" {
                rot.last_line = Line::parse(&v);
            } else if let Some(rest) = k.strip_prefix("last_served_submitter:").or_else(|| k.strip_prefix("last_served_session:")) {
                // A 0.46.5 node's session keys still seed the turns once
                // (the session was the turn key then); the submitter key
                // wins when both exist.
                if let Some((slot, ws)) = rest.split_once(':').and_then(|(s, w)| s.parse::<Slot>().ok().map(|s| (s, w.to_string()))) {
                    if k.starts_with("last_served_submitter:") || !rot.last_submitter.contains_key(&(slot, ws.clone())) {
                        rot.last_submitter.insert((slot, ws), v);
                    }
                }
            }
        }
        Ok(rot)
    }

    pub fn set_rotation(&self, rot: &Rotation) -> Result<(), String> {
        let tx = self.conn.unchecked_transaction().map_err(sqlerr)?;
        tx.execute("DELETE FROM meta WHERE k LIKE 'last_served%'", []).map_err(sqlerr)?;
        let put = |k: String, v: &str| tx.execute("INSERT INTO meta (k, v) VALUES (?1, ?2)", params![k, v]).map(|_| ()).map_err(sqlerr);
        if let Some(s) = rot.last_slot {
            put("last_served".into(), &s.to_string())?;
        }
        for (slot, ws) in &rot.last_ws {
            put(format!("last_served_ws:{slot}"), ws)?;
        }
        for ((slot, ws), s) in &rot.last_submitter {
            put(format!("last_served_submitter:{slot}:{ws}"), s)?;
        }
        if let Some(l) = rot.last_line {
            put("last_served_line".into(), l.as_str())?;
        }
        tx.commit().map_err(sqlerr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `servers.owner`: slots 0 and 2 trade every row, other slots stay,
    /// and a second run with the same id changes nothing.
    #[test]
    fn swap_slots_trades_rows_once() {
        let mut s = QueueStore::open_in_memory().unwrap();
        s.set_usage(0, 1000, 10).unwrap();
        s.set_usage(2, 2000, 20).unwrap();
        s.set_usage(1, 3000, 30).unwrap();
        s.add_day(0, "2026-10-24", 60, 70, 1).unwrap();
        s.add_duration(2, "h", 42).unwrap();
        assert!(s.swap_slots(0, 2, "c1").unwrap());
        let u = s.usage().unwrap();
        assert_eq!((u[&0], u[&2], u[&1]), ((2000, 20), (1000, 10), (3000, 30)));
        assert_eq!((s.day_cpu_secs(2, "2026-10-24").unwrap(), s.day_cpu_secs(0, "2026-10-24").unwrap()), (60, 0));
        assert_eq!(s.duration(0, "h").unwrap(), Some((42, 42)));
        assert_eq!(s.duration(2, "h").unwrap(), None);
        assert!(!s.swap_slots(0, 2, "c1").unwrap(), "exactly once per id");
        assert_eq!(s.usage().unwrap()[&0], (2000, 20), "the repeat swapped nothing back");
        assert!(s.swap_slots(0, 2, "c2").unwrap(), "a later change runs");
        assert_eq!(s.usage().unwrap()[&0], (1000, 10));
    }

    #[test]
    fn days_are_utc_dates() {
        assert_eq!(day_of(0), "1970-01-01");
        assert_eq!(day_of(1_760_000_000), "2025-10-09");
        assert_eq!(day_of(951_782_400), "2000-02-29");
        assert_eq!(day_of(-1), "1969-12-31");
    }

    #[test]
    fn percentiles_are_nearest_rank() {
        assert_eq!(percentiles(&[]), None);
        assert_eq!(percentiles(&[7]), Some((7, 7)));
        assert_eq!(percentiles(&[10, 20, 30, 40, 50, 60, 70, 80, 90, 100]), Some((50, 90)));
    }

    #[test]
    fn everything_survives_a_reopen() {
        let d = crate::util::temp_dir("queue-store");
        let p = d.join("queue.sqlite");
        {
            let s = QueueStore::open(&p).unwrap();
            s.set_usage(1, 5000, 100).unwrap();
            s.add_day(1, "2026-10-20", 30, 60, 1).unwrap();
            s.add_day(1, "2026-10-20", 10, 20, 1).unwrap();
            for secs in [100, 200, 300] {
                s.add_duration(1, "abc", secs).unwrap();
            }
            for (i, (c, m)) in [(1, 1), (2, 2), (1, 1), (3, 3), (4, 4), (5, 5)].iter().enumerate() {
                s.add_ask(1, *c, *m, i as i64).unwrap();
            }
            let mut rot = Rotation { last_slot: Some(1), ..Default::default() };
            rot.last_ws.insert(1, "ws-a".into());
            rot.last_submitter.insert((1, "ws:a".into()), "ws/s1/builder-a".into());
            rot.last_line = Some(Line::Priority);
            s.set_rotation(&rot).unwrap();
        }
        let s = QueueStore::open(&p).unwrap();
        assert_eq!(s.usage().unwrap().get(&1), Some(&(5000, 100)));
        assert_eq!(s.day_cpu_secs(1, "2026-10-20").unwrap(), 40);
        assert_eq!(s.day_cpu_secs(1, "2026-10-21").unwrap(), 0);
        assert_eq!(s.duration(1, "abc").unwrap(), Some((200, 300)));
        assert_eq!(s.duration(2, "abc").unwrap(), None, "per server");
        assert_eq!(s.asks(1).unwrap(), vec![(5, 5), (4, 4), (3, 3), (1, 1)], "last four distinct sizes");
        let rot = s.rotation().unwrap();
        assert_eq!(rot.last_slot, Some(1));
        assert_eq!(rot.last_ws.get(&1).map(String::as_str), Some("ws-a"));
        assert_eq!(rot.last_submitter.get(&(1, "ws:a".to_string())).map(String::as_str), Some("ws/s1/builder-a"), "a ':' in a workspace id survives");
        assert_eq!(rot.last_line, Some(Line::Priority), "the alternation survives a restart");
        s.forget(1).unwrap();
        assert!(s.usage().unwrap().is_empty() && s.asks(1).unwrap().is_empty());
        assert_eq!(s.rotation().unwrap(), Rotation { last_line: Some(Line::Priority), ..Default::default() }, "a forgotten server leaves only the node-wide line alternation");
    }

    #[test]
    fn durations_keep_the_last_samples() {
        let s = QueueStore::open_in_memory().unwrap();
        for i in 0..30u64 {
            s.add_duration(0, "h", i).unwrap();
        }
        let (p50, p90) = s.duration(0, "h").unwrap().unwrap();
        assert_eq!((p50, p90), (19, 27), "only the last 20 (10..=29) count");
    }
}
