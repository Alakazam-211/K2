//! CPU and memory trends for box usage (prd-app-resources-disk-guard-v1
//! Slice B, research B1; 0.46.4; migration 0217 `app_resource_samples`).
//!
//! The daemon's sampler (`k2_daemon::apps::trends`) writes one **minute**
//! row per subject (the box, each running App, each running `--cmd`
//! service) every 60 s; [`roll_up`] turns each finished hour into one
//! **hour** row (the average, and the highest minute's memory); [`prune`]
//! keeps 24 hours of minutes and 30 days of hours. So the table stays
//! bounded: at most 1440 + 720 rows per subject, and a removed App's rows
//! age out on their own.
//!
//! Unknown is `None` (NULL), never 0. CPU is **cores busy** on average
//! (`2.0` = two whole cores), so the box row and an App row read the same
//! way. Memory budgets are only read here (`mem_budget_bytes`, the seam for
//! per-App RAM budgets); nothing in this module enforces anything.

use std::collections::BTreeMap;

use rusqlite::{params, Connection};
use serde::Serialize;

/// One sample a minute.
pub const RES_MINUTE: i64 = 60;
/// One row an hour (rolled up from the minutes).
pub const RES_HOUR: i64 = 3600;
/// How long minute rows are kept.
pub const KEEP_MINUTES_SECS: i64 = 86_400;
/// How long hour rows are kept.
pub const KEEP_HOURS_SECS: i64 = 30 * 86_400;
/// The id of the box's own row.
pub const BOX_ID: &str = "box";

/// What a row describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Box,
    App,
    Service,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Box => "box",
            Kind::App => "app",
            Kind::Service => "service",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        match s {
            "box" => Some(Kind::Box),
            "app" => Some(Kind::App),
            "service" => Some(Kind::Service),
            _ => None,
        }
    }
}

/// One minute's numbers for one subject.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sample {
    pub id: String,
    pub cpu_cores: Option<f64>,
    pub mem_bytes: Option<u64>,
    pub mem_budget_bytes: Option<u64>,
    /// Box rows only.
    pub mem_available_bytes: Option<u64>,
    /// Box rows only.
    pub load5: Option<f64>,
}

/// One point of a series (camelCase on the wire).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Point {
    pub at: i64,
    pub cpu_cores: Option<f64>,
    pub mem_bytes: Option<u64>,
    /// Hour rows: the highest minute. Minute rows: the same as `memBytes`.
    pub mem_peak_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mem_budget_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mem_available_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load5: Option<f64>,
}

fn i64_of(v: Option<u64>) -> Option<i64> {
    v.and_then(|b| i64::try_from(b).ok())
}

fn u64_of(v: Option<i64>) -> Option<u64> {
    v.and_then(|b| u64::try_from(b).ok())
}

/// Write this minute's samples (`at` is floored to the minute; a second
/// write in the same minute replaces the first).
pub fn insert_minute(conn: &Connection, at: i64, rows: &[(Kind, Sample)]) -> rusqlite::Result<usize> {
    let minute = at - at.rem_euclid(RES_MINUTE);
    let mut st = conn.prepare(
        "INSERT OR REPLACE INTO app_resource_samples \
           (subject_kind, subject_id, res, at, cpu_cores, mem_bytes, mem_peak_bytes, mem_budget_bytes, mem_available_bytes, load5) \
         VALUES (?1, ?2, 60, ?3, ?4, ?5, ?5, ?6, ?7, ?8)",
    )?;
    let mut n = 0;
    for (k, s) in rows {
        if s.id.is_empty() || s.id.len() > 200 {
            continue;
        }
        n += st.execute(params![
            k.as_str(),
            s.id,
            minute,
            s.cpu_cores.filter(|c| c.is_finite() && *c >= 0.0),
            i64_of(s.mem_bytes),
            i64_of(s.mem_budget_bytes),
            i64_of(s.mem_available_bytes),
            s.load5.filter(|l| l.is_finite()),
        ])?;
    }
    Ok(n)
}

/// Turn every finished hour of minute rows into one hour row (average CPU,
/// average and peak memory, the last budget). An hour already rolled up is
/// left as it is. Returns the rows written.
pub fn roll_up(conn: &Connection, now: i64) -> rusqlite::Result<usize> {
    let this_hour = now - now.rem_euclid(RES_HOUR);
    conn.execute(
        "INSERT OR IGNORE INTO app_resource_samples \
           (subject_kind, subject_id, res, at, cpu_cores, mem_bytes, mem_peak_bytes, mem_budget_bytes, mem_available_bytes, load5) \
         SELECT subject_kind, subject_id, 3600, (at / 3600) * 3600, AVG(cpu_cores), CAST(AVG(mem_bytes) AS INTEGER), \
                MAX(COALESCE(mem_peak_bytes, mem_bytes)), MAX(mem_budget_bytes), CAST(AVG(mem_available_bytes) AS INTEGER), AVG(load5) \
         FROM app_resource_samples WHERE res = 60 AND at < ?1 \
         GROUP BY subject_kind, subject_id, at / 3600",
        params![this_hour],
    )
}

/// Drop minute rows older than a day and hour rows older than 30 days.
pub fn prune(conn: &Connection, now: i64) -> rusqlite::Result<usize> {
    let a = conn.execute("DELETE FROM app_resource_samples WHERE res = 60 AND at < ?1", params![now - KEEP_MINUTES_SECS])?;
    let b = conn.execute("DELETE FROM app_resource_samples WHERE res = 3600 AND at < ?1", params![now - KEEP_HOURS_SECS])?;
    Ok(a + b)
}

/// The resolution a window is read at: minutes up to a day, hours past it.
pub fn res_for_window(window_secs: i64) -> i64 {
    if window_secs <= KEEP_MINUTES_SECS {
        RES_MINUTE
    } else {
        RES_HOUR
    }
}

/// Every series at `res` since `since`, by (kind, id), oldest point first.
/// `only` narrows to one subject.
pub fn series(
    conn: &Connection,
    res: i64,
    since: i64,
    only: Option<(Kind, &str)>,
) -> rusqlite::Result<BTreeMap<(Kind, String), Vec<Point>>> {
    let mut out: BTreeMap<(Kind, String), Vec<Point>> = BTreeMap::new();
    let mut st = conn.prepare(
        "SELECT subject_kind, subject_id, at, cpu_cores, mem_bytes, mem_peak_bytes, mem_budget_bytes, mem_available_bytes, load5 \
         FROM app_resource_samples WHERE res = ?1 AND at >= ?2 \
           AND (?3 IS NULL OR (subject_kind = ?3 AND subject_id = ?4)) \
         ORDER BY subject_kind, subject_id, at",
    )?;
    let (ok, oid) = match only {
        Some((k, id)) => (Some(k.as_str()), Some(id)),
        None => (None, None),
    };
    let rows = st.query_map(params![res, since, ok, oid], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            Point {
                at: r.get(2)?,
                cpu_cores: r.get(3)?,
                mem_bytes: u64_of(r.get(4)?),
                mem_peak_bytes: u64_of(r.get(5)?),
                mem_budget_bytes: u64_of(r.get(6)?),
                mem_available_bytes: u64_of(r.get(7)?),
                load5: r.get(8)?,
            },
        ))
    })?;
    for r in rows {
        let (k, id, p) = r?;
        let Some(k) = Kind::parse(&k) else { continue };
        out.entry((k, id)).or_default().push(p);
    }
    Ok(out)
}

/// Cores busy between two cumulative CPU readings (`usec` of CPU time over
/// `wall_usec` of wall time). `None` when the counter went backwards (a
/// restarted App) or no time passed.
pub fn cores_between(prev_usec: u64, now_usec: u64, wall_usec: u64) -> Option<f64> {
    if wall_usec == 0 || now_usec < prev_usec {
        return None;
    }
    Some(round3((now_usec - prev_usec) as f64 / wall_usec as f64))
}

/// The box's busy and total jiffies from `/proc/stat`'s first line (busy =
/// total − idle − iowait).
pub fn parse_proc_stat_cpu(stat: &str) -> Option<(u64, u64)> {
    let line = stat.lines().find(|l| l.starts_with("cpu "))?;
    let v: Vec<u64> = line.split_whitespace().skip(1).filter_map(|n| n.parse().ok()).collect();
    if v.len() < 4 {
        return None;
    }
    // user nice system idle iowait irq softirq steal (guest is inside user).
    let total: u64 = v.iter().take(8).sum();
    let idle = v[3] + v.get(4).copied().unwrap_or(0);
    Some((total.saturating_sub(idle), total))
}

/// Cores busy on the whole box between two `/proc/stat` readings.
pub fn box_cores_between(prev: (u64, u64), now: (u64, u64), cores: usize) -> Option<f64> {
    let (db, dt) = (now.0.checked_sub(prev.0)?, now.1.checked_sub(prev.1)?);
    if dt == 0 {
        return None;
    }
    Some(round3(db as f64 / dt as f64 * cores as f64))
}

fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(include_str!("../drizzle_sql/0217_app_resource_samples.sql").replace("--> statement-breakpoint", "").as_str())
            .unwrap();
        c
    }

    fn app(id: &str, cpu: f64, mem: u64) -> (Kind, Sample) {
        (Kind::App, Sample { id: id.into(), cpu_cores: Some(cpu), mem_bytes: Some(mem), mem_budget_bytes: Some(4096), ..Sample::default() })
    }

    #[test]
    fn minutes_roll_up_into_hours_and_age_out() {
        let c = db();
        let h0 = 1_800_000_000 - 1_800_000_000 % 3600;
        // Three minutes in hour 0, one in hour 1 (still running).
        for (i, (cpu, mem)) in [(0.5, 100u64), (1.5, 300), (1.0, 200)].iter().enumerate() {
            insert_minute(&c, h0 + 60 * i as i64 + 7, &[app("jobs", *cpu, *mem)]).unwrap();
        }
        insert_minute(&c, h0 + 3600 + 5, &[app("jobs", 2.0, 900)]).unwrap();
        // A second write in the same minute replaces the first.
        insert_minute(&c, h0 + 3600 + 30, &[app("jobs", 3.0, 1000)]).unwrap();
        let m = series(&c, RES_MINUTE, 0, Some((Kind::App, "jobs"))).unwrap();
        let pts = &m[&(Kind::App, "jobs".to_string())];
        assert_eq!(pts.len(), 4, "{pts:?}");
        assert_eq!(pts[0].at, h0, "floored to the minute");
        assert_eq!(pts[3].cpu_cores, Some(3.0));

        assert_eq!(roll_up(&c, h0 + 3600 + 120).unwrap(), 1, "only the finished hour");
        assert_eq!(roll_up(&c, h0 + 3600 + 180).unwrap(), 0, "an hour rolls up once");
        let h = series(&c, RES_HOUR, 0, None).unwrap();
        let hour = &h[&(Kind::App, "jobs".to_string())];
        assert_eq!(hour.len(), 1);
        assert_eq!(hour[0].at, h0);
        assert_eq!(hour[0].cpu_cores, Some(1.0), "the average");
        assert_eq!(hour[0].mem_bytes, Some(200), "the average");
        assert_eq!(hour[0].mem_peak_bytes, Some(300), "the highest minute");
        assert_eq!(hour[0].mem_budget_bytes, Some(4096));

        // A day later the minutes go; the hour stays 30 days.
        let later = h0 + KEEP_MINUTES_SECS + 7200;
        prune(&c, later).unwrap();
        assert!(series(&c, RES_MINUTE, 0, None).unwrap().is_empty(), "minutes kept a day only");
        assert_eq!(series(&c, RES_HOUR, 0, None).unwrap().len(), 1);
        prune(&c, h0 + KEEP_HOURS_SECS + 1).unwrap();
        assert!(series(&c, RES_HOUR, 0, None).unwrap().is_empty(), "hours kept 30 days only");
    }

    #[test]
    fn unknown_stays_null_and_bad_numbers_are_dropped() {
        let c = db();
        let rows = [
            (Kind::Box, Sample { id: BOX_ID.into(), cpu_cores: None, mem_bytes: Some(5), mem_available_bytes: Some(7), load5: Some(1.25), ..Sample::default() }),
            (Kind::Service, Sample { id: "ws-1/site".into(), cpu_cores: Some(f64::NAN), mem_bytes: None, ..Sample::default() }),
            (Kind::App, Sample { id: String::new(), ..Sample::default() }),
        ];
        assert_eq!(insert_minute(&c, 120, &rows).unwrap(), 2, "an empty id is never written");
        let s = series(&c, RES_MINUTE, 0, None).unwrap();
        let b = &s[&(Kind::Box, BOX_ID.to_string())][0];
        assert_eq!((b.cpu_cores, b.mem_available_bytes, b.load5), (None, Some(7), Some(1.25)));
        let v = &s[&(Kind::Service, "ws-1/site".to_string())][0];
        assert_eq!((v.cpu_cores, v.mem_bytes), (None, None), "NaN and unknown are NULL, never 0");
        let j = serde_json::to_value(v).unwrap();
        assert!(j["cpuCores"].is_null() && j["memBytes"].is_null(), "{j}");
    }

    #[test]
    fn cpu_maths() {
        assert_eq!(cores_between(1_000_000, 1_500_000, 1_000_000), Some(0.5));
        assert_eq!(cores_between(1_000_000, 3_000_000, 1_000_000), Some(2.0));
        assert_eq!(cores_between(5, 1, 1_000_000), None, "a restarted App's counter goes back");
        assert_eq!(cores_between(1, 2, 0), None);
        let a = parse_proc_stat_cpu("cpu  100 0 100 700 100 0 0 0 0 0\ncpu0 1 2 3 4\n").unwrap();
        assert_eq!(a, (200, 1000));
        let b = parse_proc_stat_cpu("cpu  300 0 300 1000 200 0 0 0 0 0\n").unwrap();
        assert_eq!(box_cores_between(a, b, 8), Some(4.0), "400 busy of 800 jiffies on 8 cores: half of 8");
        assert_eq!(box_cores_between(b, a, 8), None);
        assert_eq!(parse_proc_stat_cpu("intr 1 2\n"), None);
        assert_eq!(res_for_window(3600), RES_MINUTE);
        assert_eq!(res_for_window(7 * 86_400), RES_HOUR);
    }
}
