//! Job stats series (prd-compute-blacksmith-class-v1 §4.5.1, slice O2;
//! migration 0296 `compute_job_stats`).
//!
//! A node offering `job-stats` (protocol 10) sends a job's series with its
//! end (`Outputs.stats`): one point every 5 s or more (CPU over the
//! interval, memory, cumulative IO, the node's load), at most 720 points and
//! 64 KB, plus the cgroup's throttle counters and OOM kills. It is kept here,
//! one row per attempt, not in `compute_jobs.summary_json` (which stays
//! small). Rows go after [`KEEP_SECS`]. `k2 compute stats <job>` reads it
//! through [`view`].

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

use super::proto::gens::JobStats;

/// How long a series is kept.
pub const KEEP_SECS: i64 = 30 * 86_400;

/// The CFS period a throttle count is in (systemd's `CPUQuotaPeriodSec`
/// default and the node's `cpu.max`): each throttled period held the job
/// for up to this long.
pub const CFS_PERIOD_MS: u64 = 100;

/// At or above this share of its run held at its CPU limit, `stats` says to
/// ask for more.
pub const HELD_HINT_PCT: u64 = 10;

/// One stored series.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stored {
    pub generation: i64,
    pub node_id: String,
    pub stats: JobStats,
    pub created_at: i64,
}

/// Keep `s` for attempt `generation` of `job_id` (replacing that attempt's
/// earlier one), and drop series older than [`KEEP_SECS`].
pub fn put(conn: &Connection, job_id: &str, generation: i64, workspace_id: &str, node_id: &str, s: &JobStats, now: i64) -> Result<(), String> {
    let s = s.clone().downsampled();
    let text = serde_json::to_string(&s).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT OR REPLACE INTO compute_job_stats (job_id, generation, workspace_id, node_id, every_secs, points, stats_json, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![job_id, generation, workspace_id, node_id, i64::from(s.every_secs), s.points.len() as i64, text, now],
    )
    .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM compute_job_stats WHERE created_at < ?1", params![now - KEEP_SECS]).map_err(|e| e.to_string())?;
    Ok(())
}

/// The latest attempt's series of `job_id`, if any.
pub fn get(conn: &Connection, job_id: &str) -> Option<Stored> {
    conn.query_row(
        "SELECT generation, node_id, stats_json, created_at FROM compute_job_stats WHERE job_id = ?1 ORDER BY generation DESC LIMIT 1",
        params![job_id],
        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?)),
    )
    .optional()
    .ok()
    .flatten()
    .and_then(|(generation, node_id, text, created_at)| serde_json::from_str(&text).ok().map(|stats| Stored { generation, node_id, stats, created_at }))
}

/// Whole CPUs without a dot (`4`, `0.5`).
fn cpus(m: u64) -> String {
    if m % 1000 == 0 {
        (m / 1000).to_string()
    } else {
        format!("{}", m as f64 / 1000.0)
    }
}

/// What `k2 compute stats` shows of a series: the points, the peaks, IO
/// totals, how long the job was held at its CPU limit (with what to ask
/// for), OOM kills, and what isn't measured. `cpu_millis` / `burst_millis`:
/// its `--cpus N[+M]`; `wall_secs`: how long it ran.
pub fn view(s: &JobStats, cpu_millis: Option<u64>, burst_millis: Option<u64>, wall_secs: Option<i64>) -> Value {
    let points: Vec<Value> = s
        .points
        .iter()
        .map(|p| json!({"t": p.t, "cpuMilli": p.cpu_milli, "memBytes": p.mem_bytes, "ioReadBytes": p.io_read, "ioWriteBytes": p.io_write, "load1Milli": p.load1_milli}))
        .collect();
    let peak_cpu = s.points.iter().map(|p| p.cpu_milli).max();
    let peak_mem = s.points.iter().map(|p| p.mem_bytes).max();
    let peak_load = s.points.iter().map(|p| p.load1_milli).max();
    // `io.stat` counts are cumulative: the last point is the total.
    let io = s.points.iter().rev().find_map(|p| Some((p.io_read?, p.io_write?)));
    let throttle = s.nr_throttled.map(|n| {
        let held_ms = n.saturating_mul(CFS_PERIOD_MS);
        let pct = wall_secs.filter(|w| *w > 0).map(|w| (held_ms / 10 / w as u64).min(100));
        let ceiling = burst_millis.or(cpu_millis);
        let hint = match (pct, cpu_millis, burst_millis) {
            (Some(p), Some(n), None) if p >= HELD_HINT_PCT => {
                Some(format!("held to its --cpus {} {p}% of the time: try --cpus {}+{}", cpus(n), cpus(n), cpus(n.saturating_mul(2))))
            }
            (Some(p), Some(n), Some(m)) if p >= HELD_HINT_PCT => Some(format!(
                "held at its ceiling of {} CPUs {p}% of the time: a bigger M (--cpus {}+{}) may finish sooner where the node has the cores",
                cpus(m),
                cpus(n),
                cpus(m.saturating_mul(2))
            )),
            _ => None,
        };
        json!({"periods": n, "usec": s.throttled_usec, "heldPct": pct, "ceilingCpus": ceiling.map(|c| c as f64 / 1000.0), "hint": hint})
    });
    let mut notes = vec!["no per-job network numbers (a cgroup doesn't give them cheaply)".to_string()];
    if io.is_none() && !s.points.is_empty() {
        notes.push("no IO numbers: the job had no cgroup (macOS, or Linux without delegation)".into());
    }
    if s.nr_throttled.is_none() {
        notes.push("no throttle counters: no CPU limit there (macOS), or no cgroup".into());
    }
    json!({
        "everySecs": s.every_secs,
        "points": points,
        "peaks": {
            "cpus": peak_cpu.map(|c| f64::from(c) / 1000.0),
            "memBytes": peak_mem,
            "load1": peak_load.map(|l| f64::from(l) / 1000.0),
        },
        "io": io.map(|(r, w)| json!({"readBytes": r, "writeBytes": w})),
        "throttle": throttle,
        "oomEvents": s.oom_events,
        "notes": notes,
    })
}

/// A text sparkline (at most `width` columns, each the highest value in its
/// slice of the series).
pub fn spark(vals: &[u64], width: usize) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    if vals.is_empty() {
        return String::new();
    }
    let cols: Vec<u64> = if vals.len() > width {
        let step = vals.len() as f64 / width as f64;
        (0..width)
            .map(|i| {
                let a = (i as f64 * step) as usize;
                let b = (((i + 1) as f64 * step) as usize).max(a + 1).min(vals.len());
                vals[a..b].iter().copied().max().unwrap_or(0)
            })
            .collect()
    } else {
        vals.to_vec()
    };
    let top = cols.iter().copied().max().unwrap_or(0).max(1) as f64;
    cols.iter().map(|x| BARS[((*x as f64 * 7.0 / top + 0.5) as usize).min(7)]).collect()
}

/// `k2 compute stats`'s plain answer, from the route's JSON (the CLI only
/// prints these lines).
pub fn render(v: &Value) -> Vec<String> {
    let g = |b: &Value| match b.as_f64() {
        None => "?".to_string(),
        Some(b) => {
            let x = b / (1u64 << 30) as f64;
            if x < 10.0 {
                format!("{x:.1}G")
            } else {
                format!("{}G", x.round() as i64)
            }
        }
    };
    let num = |x: &Value| x.as_f64().map(|f| f.to_string()).unwrap_or_else(|| "?".into());
    let dur = |s: &Value| match s.as_i64() {
        None => "?".to_string(),
        Some(s) if s >= 3600 => format!("{}h{:02}m", s / 3600, (s % 3600) / 60),
        Some(s) if s >= 60 => format!("{}m{:02}s", s / 60, s % 60),
        Some(s) => format!("{s}s"),
    };
    let s = |x: &Value| x.as_str().map(str::to_string).unwrap_or_else(|| "?".into());
    let mut out = vec![format!("stats · job {} · {} · {} · ran {} · attempt {}", s(&v["short"]), s(&v["state"]), s(&v["node"]), dur(&v["wallSecs"]), v["generation"])];
    let st = &v["stats"];
    if st.is_null() {
        out.push(format!("  no series: {}", s(&v["why"])));
        return out;
    }
    if !v["statsGeneration"].is_null() && v["statsGeneration"] != v["generation"] {
        out.push(format!("  (the series of attempt {}: a later attempt sent none)", v["statsGeneration"]));
    }
    let pts = st["points"].as_array().cloned().unwrap_or_default();
    let col = |k: &str| pts.iter().map(|p| p[k].as_u64().unwrap_or(0)).collect::<Vec<u64>>();
    out.push(format!("  every {}s, {} points", st["everySecs"], pts.len()));
    let c = &v["cpus"];
    let asked = match (c["guaranteed"].as_f64(), c["burst"].as_f64()) {
        (Some(n), Some(m)) => format!("of --cpus {n}+{m}"),
        (Some(n), None) => format!("of --cpus {n}"),
        _ => "(no --cpus)".into(),
    };
    let pk = &st["peaks"];
    out.push(format!("  cpu     peak {:<6} {asked}  {}", num(&pk["cpus"]), spark(&col("cpuMilli"), 40)));
    let lim = if v["memLimitBytes"].is_null() { String::new() } else { format!("of {}", g(&v["memLimitBytes"])) };
    out.push(format!("  memory  peak {:<6} {lim}  {}", g(&pk["memBytes"]), spark(&col("memBytes"), 40)));
    if st["io"].is_object() {
        // Per interval: the cumulative counters' growth.
        let mut prev = 0u64;
        let moved: Vec<u64> = pts
            .iter()
            .map(|p| {
                let now = p["ioReadBytes"].as_u64().unwrap_or(0) + p["ioWriteBytes"].as_u64().unwrap_or(0);
                let d = now.saturating_sub(prev);
                prev = now;
                d
            })
            .collect();
        out.push(format!("  io      read {} · written {}  {}", g(&st["io"]["readBytes"]), g(&st["io"]["writeBytes"]), spark(&moved, 40)));
    }
    out.push(format!("  load    peak {:<6} {}", num(&pk["load1"]), spark(&col("load1Milli"), 40)));
    let t = &st["throttle"];
    if !t["heldPct"].is_null() {
        out.push(format!("  held at its CPU limit about {}% of the run ({} throttled periods)", t["heldPct"], t["periods"]));
    }
    if let Some(h) = t["hint"].as_str() {
        out.push(format!("  {h}"));
    }
    if !st["oomEvents"].is_null() {
        out.push(format!("  oom kills: {}", st["oomEvents"]));
    }
    for n in st["notes"].as_array().into_iter().flatten() {
        out.push(format!("  ({})", s(n)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::proto::gens::StatPoint;

    /// The plain answer `k2 compute stats` prints (the daemon writes it).
    #[test]
    fn render_lines() {
        let s = JobStats {
            every_secs: 5,
            points: (1..=72u32).map(|i| StatPoint { t: i * 5, cpu_milli: 3900, mem_bytes: u64::from(i) * (1 << 27), io_read: Some(u64::from(i) << 24), io_write: Some((u64::from(i) * 3) << 24), load1_milli: 6000 }).collect(),
            nr_throttled: Some(1200),
            throttled_usec: Some(110_000_000),
            oom_events: Some(0),
        };
        let v = json!({"short": "ab12cd34", "state": "done", "node": "rig7", "generation": 1, "wallSecs": 360,
            "cpus": {"guaranteed": 4.0, "burst": null}, "memLimitBytes": 16u64 << 30, "statsGeneration": 1,
            "stats": view(&s, Some(4000), None, Some(360))});
        let t = render(&v);
        assert_eq!(t[0], "stats · job ab12cd34 · done · rig7 · ran 6m00s · attempt 1");
        assert_eq!(t[1], "  every 5s, 72 points");
        assert!(t[2].starts_with("  cpu     peak 3.9    of --cpus 4  ████"), "{}", t[2]);
        assert!(t[3].starts_with("  memory  peak 9.0G   of 16G  ▁"), "{}", t[3]);
        assert!(t[3].ends_with('█') && t[3].chars().filter(|c| "▁▂▃▄▅▆▇█".contains(*c)).count() == 40, "{}", t[3]);
        assert!(t[4].starts_with("  io      read 1.1G · written 3.4G  "), "{}", t[4]);
        assert!(t[5].starts_with("  load    peak 6      "), "{}", t[5]);
        assert_eq!(t[6], "  held at its CPU limit about 33% of the run (1200 throttled periods)");
        assert_eq!(t[7], "  held to its --cpus 4 33% of the time: try --cpus 4+8");
        assert_eq!(t[8], "  oom kills: 0");
        assert_eq!(t[9], "  (no per-job network numbers (a cgroup doesn't give them cheaply))");
        // No series: the reason; a burst job's header.
        let none = json!({"short": "old00000", "state": "done", "node": "rig4", "generation": 1, "wallSecs": 30, "stats": null, "why": "no series for it"});
        assert_eq!(render(&none), vec!["stats · job old00000 · done · rig4 · ran 30s · attempt 1", "  no series: no series for it"]);
        let burst = json!({"short": "b", "state": "done", "node": "n", "generation": 2, "wallSecs": 4000, "statsGeneration": 1, "cpus": {"guaranteed": 4.0, "burst": 8.0}, "stats": view(&s, Some(4000), Some(8000), None)});
        let t = render(&burst);
        assert_eq!(t[0], "stats · job b · done · n · ran 1h06m · attempt 2");
        assert_eq!(t[1], "  (the series of attempt 1: a later attempt sent none)");
        assert!(t[3].contains("of --cpus 4+8"), "{}", t[3]);
        assert_eq!(spark(&[0, 7, 14], 40), "▁▅█");
        assert_eq!(spark(&[], 40), "");
    }

    fn series(n: u32, cpu: u32) -> JobStats {
        JobStats {
            every_secs: 5,
            points: (1..=n).map(|i| StatPoint { t: i * 5, cpu_milli: cpu + i, mem_bytes: u64::from(i) << 20, io_read: Some(u64::from(i) << 10), io_write: Some(u64::from(i) << 11), load1_milli: 1000 * i }).collect(),
            nr_throttled: Some(1200),
            throttled_usec: Some(90_000_000),
            oom_events: Some(0),
        }
    }

    /// O2: a series is kept per attempt, the latest attempt reads back, and
    /// rows older than 30 days go when a new one is written.
    #[test]
    fn put_and_get_keep_the_latest_attempt_for_thirty_days() {
        let db = crate::db::shared();
        let conn = db.lock();
        let now = 1_900_000_000;
        put(&conn, "js-a", 1, "w1", "n1", &series(3, 1000), now - KEEP_SECS - 10).unwrap();
        assert_eq!(get(&conn, "js-a").map(|s| s.generation), Some(1));
        put(&conn, "js-b", 1, "w1", "n1", &series(3, 1000), now).unwrap();
        assert_eq!(get(&conn, "js-a"), None, "older than 30 days: gone");
        put(&conn, "js-b", 2, "w1", "n2", &series(4, 2000), now + 1).unwrap();
        let s = get(&conn, "js-b").expect("stored");
        assert_eq!((s.generation, s.node_id.as_str(), s.stats.points.len()), (2, "n2", 4));
        put(&conn, "js-b", 2, "w1", "n2", &series(5, 2000), now + 2).unwrap();
        assert_eq!(get(&conn, "js-b").unwrap().stats.points.len(), 5, "the same attempt again replaces it");
        let rows: i64 = conn.query_row("SELECT COUNT(*) FROM compute_job_stats WHERE job_id = 'js-b'", [], |r| r.get(0)).unwrap();
        assert_eq!(rows, 2);
        assert_eq!(get(&conn, "js-none"), None);
    }

    /// O2 (§4.5.1): peaks, IO totals, held time and its hint ("held to its
    /// --cpus 34% of the time: try --cpus 4+8"), OOM kills, notes.
    #[test]
    fn the_view_says_how_long_it_was_held_and_what_to_ask() {
        // 1200 throttled periods × 100 ms = 120 s held in a 360 s run: 33%.
        let v = view(&series(3, 3900), Some(4000), None, Some(360));
        assert_eq!(v["peaks"]["cpus"], json!(3.903));
        assert_eq!(v["peaks"]["memBytes"], json!(3u64 << 20));
        assert_eq!(v["io"], json!({"readBytes": 3u64 << 10, "writeBytes": 3u64 << 11}));
        assert_eq!(v["throttle"]["heldPct"], json!(33));
        assert_eq!(v["throttle"]["hint"], json!("held to its --cpus 4 33% of the time: try --cpus 4+8"));
        assert_eq!((v["oomEvents"].as_u64(), v["everySecs"].as_u64(), v["points"].as_array().map(Vec::len)), (Some(0), Some(5), Some(3)));
        assert!(v["notes"][0].as_str().unwrap().contains("network"));
        // Already bursting: the ceiling is M.
        let b = view(&series(3, 7900), Some(4000), Some(8000), Some(360));
        assert!(b["throttle"]["hint"].as_str().unwrap().starts_with("held at its ceiling of 8 CPUs 33%"), "{b}");
        assert_eq!(b["throttle"]["ceilingCpus"], json!(8.0));
        // Rarely held: no hint.
        let quiet = JobStats { nr_throttled: Some(10), ..series(3, 1000) };
        assert_eq!(view(&quiet, Some(4000), None, Some(360))["throttle"]["hint"], Value::Null);
        // macOS: no IO, no counters, and said.
        let mac = JobStats { points: series(2, 500).points.into_iter().map(|p| StatPoint { io_read: None, io_write: None, ..p }).collect(), nr_throttled: None, throttled_usec: None, oom_events: None, every_secs: 5 };
        let m = view(&mac, None, None, Some(10));
        assert_eq!((m["io"].clone(), m["throttle"].clone(), m["oomEvents"].clone()), (Value::Null, Value::Null, Value::Null));
        assert_eq!(m["notes"].as_array().unwrap().len(), 3);
    }
}
