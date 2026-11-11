//! Right-sizing (prd-compute-blacksmith-class-v1 §4.4.2, §4.4.3, slice R1):
//! what a command really used, from its ended runs' `summary_json`
//! (`resources.peak_mem_bytes`, `cpu_ms`) and its plan's limits. No table
//! of its own: `k2 compute rightsize` and `--mem auto` both read
//! `compute_jobs`.
//!
//! **The key** is the estimate's: this workspace and the exact argv; or a
//! `--tag` (a CI check's jobs carry `ci:<check>`, so `--check C` is the tag
//! `ci:C`). Per workspace only: no other workspace's or tenant's runs are
//! ever read (§4.4.5).
//!
//! **Memory**: suggest the p95 of the finished runs' peaks × 1.25, rounded
//! up to a whole GiB, and never below 1.5 × the peak of the last run the
//! node stopped at its memory limit (`failed`/`mem_cap`), so one kill never
//! repeats. A `mem_cap` run with no reported peak counts at its limit (it
//! was stopped there).
//!
//! **CPUs**: the average cores used (CPU time ÷ run time) against what was
//! asked. Under half: a smaller `--cpus` costs less. At the limit in most
//! runs: it was held there, so a bigger `--cpus` may finish sooner.

use rusqlite::{params, Connection};
use serde::Serialize;

/// `--mem auto` reads at most this many recent runs (§4.4.3).
pub const AUTO_RUNS: i64 = 20;
/// `rightsize` reads at most this many runs in its window.
pub const MAX_RUNS: i64 = 500;
/// The suggestion's headroom over the p95 peak.
pub const HEADROOM: f64 = 1.25;
/// After a `mem_cap` end, at least this × its peak.
pub const MEM_CAP_FLOOR: f64 = 1.5;
/// A run used its whole `--cpus` when it averaged at least this share.
pub const PINNED_SHARE: f64 = 0.9;

const GIB: u64 = 1 << 30;

/// Which runs: the same command, or the same tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    Argv(Vec<String>),
    Tag(String),
}

impl Key {
    /// A `[checks.<name>]`'s jobs (`ci:<name>`, `ci::job_body`).
    pub fn check(name: &str) -> Key {
        Key::Tag(format!("ci:{name}"))
    }

    /// `{"kind": "argv"|"tag"|"check", "value": …}` for answers.
    pub fn json(&self) -> serde_json::Value {
        match self {
            Key::Argv(a) => serde_json::json!({"kind": "argv", "value": a}),
            Key::Tag(t) => match t.strip_prefix("ci:") {
                Some(c) => serde_json::json!({"kind": "check", "value": c, "tag": t}),
                None => serde_json::json!({"kind": "tag", "value": t}),
            },
        }
    }
}

/// One ended run, as sizing reads it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Run {
    pub job_id: String,
    /// `done` (it ran and exited, any code) or `failed` (`mem_cap` only).
    pub state: String,
    pub reason: Option<String>,
    pub secs: Option<i64>,
    pub peak_mem: Option<u64>,
    /// The `--mem` it ran under (the plan's limit).
    pub mem_limit: Option<u64>,
    pub cpu_ms: Option<u64>,
    /// Its `--cpus` (`None`: none asked; it ran on the default share).
    pub cpus_asked: Option<f64>,
    pub ended_at: i64,
}

impl Run {
    pub fn is_done(&self) -> bool {
        self.state == "done"
    }
    pub fn is_mem_cap(&self) -> bool {
        self.state == "failed" && self.reason.as_deref() == Some("mem_cap")
    }
    /// Average cores used, when the node reported CPU time.
    pub fn cores_used(&self) -> Option<f64> {
        match (self.cpu_ms, self.secs) {
            (Some(ms), Some(s)) if s > 0 => Some(ms as f64 / 1000.0 / s as f64),
            _ => None,
        }
    }
}

/// The key's ended runs (`done`, and `failed`/`mem_cap`), newest first,
/// ended at or after `since`, at most `limit`. A split run's parent row
/// (it has no resources of its own) is left out; its shards count.
pub fn runs(conn: &Connection, workspace_id: &str, key: &Key, since: i64, limit: i64) -> Vec<Run> {
    let (argv_json, tag) = match key {
        Key::Argv(a) => (serde_json::to_string(a).ok(), None),
        Key::Tag(t) => (None, Some(t.as_str())),
    };
    if argv_json.is_none() && tag.is_none() {
        return Vec::new();
    }
    let sql = "SELECT id, state, reason, started_at, ended_at, json_extract(plan_json, '$.limits.mem_bytes'), \
               json_extract(plan_json, '$.limits.cpu_millis'), summary_json FROM compute_jobs \
               WHERE workspace_id = ?1 AND (?2 IS NULL OR argv_json = ?2) \
               AND (?3 IS NULL OR json_extract(plan_json, '$.k2ctl.tag') = ?3) \
               AND (state = 'done' OR (state = 'failed' AND reason = 'mem_cap')) \
               AND ended_at IS NOT NULL AND ended_at >= ?4 \
               AND json_array_length(COALESCE(json_extract(plan_json, '$.k2ctl.shards'), '[]')) = 0 \
               ORDER BY ended_at DESC, rowid DESC LIMIT ?5";
    let Ok(mut st) = conn.prepare(sql) else { return Vec::new() };
    st.query_map(params![workspace_id, argv_json, tag, since, limit.clamp(1, MAX_RUNS)], |r| {
        let started: Option<i64> = r.get(3)?;
        let ended: i64 = r.get(4)?;
        let summary: Option<String> = r.get(7)?;
        let res = summary
            .as_deref()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
            .and_then(|v| v.get("resources").cloned())
            .unwrap_or(serde_json::Value::Null);
        Ok(Run {
            job_id: r.get(0)?,
            state: r.get(1)?,
            reason: r.get(2)?,
            secs: started.map(|s| (ended - s).max(0)),
            peak_mem: res.get("peak_mem_bytes").and_then(serde_json::Value::as_u64),
            mem_limit: r.get::<_, Option<i64>>(5)?.and_then(|m| u64::try_from(m).ok()),
            cpu_ms: res.get("cpu_ms").and_then(serde_json::Value::as_u64),
            cpus_asked: r.get::<_, Option<i64>>(6)?.filter(|m| *m > 0).map(|m| m as f64 / 1000.0),
            ended_at: ended,
        })
    })
    .map(|rows| rows.filter_map(Result::ok).collect())
    .unwrap_or_default()
}

/// Up to a whole GiB (at least 1).
pub fn round_up_gib(bytes: u64) -> u64 {
    bytes.div_ceil(GIB).max(1) * GIB
}

/// `8G`, `15.2G`.
pub fn gib_words(bytes: u64) -> String {
    if bytes % GIB == 0 {
        format!("{}G", bytes / GIB)
    } else {
        format!("{:.1}G", bytes as f64 / GIB as f64)
    }
}

/// Nearest-rank percentile of `sorted` (ascending), `p` in 0..=100.
fn pct(sorted: &[u64], p: u64) -> Option<u64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = (p as usize * sorted.len()).div_ceil(100).max(1);
    sorted.get(rank - 1).copied()
}

fn pct_f(sorted: &[f64], p: u64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = (p as usize * sorted.len()).div_ceil(100).max(1);
    sorted.get(rank - 1).copied()
}

/// Memory facts of a set of runs.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemFacts {
    /// Finished runs with a reported peak.
    pub samples: usize,
    pub p50_bytes: Option<u64>,
    pub p95_bytes: Option<u64>,
    pub max_bytes: Option<u64>,
    /// Runs the node stopped at their memory limit.
    pub mem_cap_ends: usize,
    /// The newest such run's peak (its limit when no peak was reported).
    pub last_mem_cap_peak_bytes: Option<u64>,
    /// 1.5 × that peak, rounded up.
    pub floor_bytes: Option<u64>,
    /// The suggestion (`None`: no finished run and no `mem_cap` end).
    pub suggest_bytes: Option<u64>,
    /// The `--mem` of the newest run.
    pub asked_bytes: Option<u64>,
}

/// [`MemFacts`] of `runs` (newest first).
pub fn mem_facts(runs: &[Run]) -> MemFacts {
    let mut peaks: Vec<u64> = runs.iter().filter(|r| r.is_done()).filter_map(|r| r.peak_mem).collect();
    peaks.sort_unstable();
    let caps: Vec<&Run> = runs.iter().filter(|r| r.is_mem_cap()).collect();
    let last_cap = caps.first().and_then(|r| r.peak_mem.or(r.mem_limit));
    let floor = last_cap.map(|p| round_up_gib((p as f64 * MEM_CAP_FLOOR).ceil() as u64));
    let p95 = pct(&peaks, 95);
    let from_peaks = p95.map(|p| round_up_gib((p as f64 * HEADROOM).ceil() as u64));
    let suggest = match (from_peaks, floor) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    };
    MemFacts {
        samples: peaks.len(),
        p50_bytes: pct(&peaks, 50),
        p95_bytes: p95,
        max_bytes: peaks.last().copied(),
        mem_cap_ends: caps.len(),
        last_mem_cap_peak_bytes: last_cap,
        floor_bytes: floor,
        suggest_bytes: suggest,
        asked_bytes: runs.first().and_then(|r| r.mem_limit),
    }
}

/// `--mem auto[:G]` (§4.4.3): the number a submit uses, and its words for
/// `effective.mem.from`. `None`: no history and no fallback
/// (`mem_auto_no_history`). With no finished run, the fallback, raised to
/// the `mem_cap` floor when there is one.
pub fn auto_mem(runs: &[Run], fallback: Option<u64>) -> Option<(u64, String)> {
    let f = mem_facts(runs);
    let floor_words = |fl: Option<u64>| match (f.last_mem_cap_peak_bytes, fl) {
        (Some(p), Some(_)) => format!("; at least 1.5 × the {} peak of a mem_cap end", gib_words(p)),
        _ => String::new(),
    };
    if let Some(p95) = f.p95_bytes {
        let v = f.suggest_bytes.unwrap_or_else(|| round_up_gib(p95));
        let floored = f.floor_bytes.is_some_and(|fl| fl >= v);
        return Some((
            v,
            format!(
                "auto (p95 {} of {} run{}{})",
                gib_words(p95),
                f.samples,
                if f.samples == 1 { "" } else { "s" },
                if floored { floor_words(f.floor_bytes) } else { String::new() }
            ),
        ));
    }
    match (fallback, f.floor_bytes) {
        (Some(fb), Some(fl)) if fl > fb => Some((fl, format!("auto (no finished run yet{})", floor_words(Some(fl))))),
        (Some(fb), _) => Some((fb, format!("auto (no history yet: the fallback {})", gib_words(fb)))),
        (None, Some(fl)) => Some((fl, format!("auto (no finished run yet{})", floor_words(Some(fl))))),
        (None, None) => None,
    }
}

/// CPU facts of a set of runs.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CpuFacts {
    /// Finished runs with CPU time and a duration.
    pub samples: usize,
    pub avg_used: Option<f64>,
    pub p95_used: Option<f64>,
    /// The `--cpus` of the newest run (`None`: none asked).
    pub asked: Option<f64>,
    /// Runs that averaged at least 90 % of their `--cpus`.
    pub pinned_runs: usize,
    /// A `--cpus` to try, and why (`cost`, `speed` or `none-asked`).
    pub suggest: Option<f64>,
    pub why: Option<String>,
}

fn round_cpus(c: f64) -> f64 {
    // Whole cores at 1 and above; halves below.
    if c >= 1.0 {
        c.ceil()
    } else {
        ((c * 2.0).ceil() / 2.0).max(0.5)
    }
}

/// [`CpuFacts`] of `runs` (newest first).
pub fn cpu_facts(runs: &[Run]) -> CpuFacts {
    let done: Vec<&Run> = runs.iter().filter(|r| r.is_done()).collect();
    let used: Vec<(f64, Option<f64>)> = done.iter().filter_map(|r| r.cores_used().map(|u| (u, r.cpus_asked))).collect();
    let asked = done.first().and_then(|r| r.cpus_asked);
    if used.is_empty() {
        return CpuFacts { asked, ..Default::default() };
    }
    let avg = used.iter().map(|(u, _)| u).sum::<f64>() / used.len() as f64;
    let mut sorted: Vec<f64> = used.iter().map(|(u, _)| *u).collect();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p95 = pct_f(&sorted, 95);
    let pinned = used.iter().filter(|(u, a)| a.is_some_and(|a| a > 0.0 && *u >= a * PINNED_SHARE)).count();
    let (suggest, why) = match asked {
        Some(a) if avg < a / 2.0 => {
            let s = round_cpus(p95.unwrap_or(avg) * HEADROOM);
            (Some(s).filter(|s| *s < a), Some(format!("cost: it used about {avg:.1} of its {a} cores; --cpus {s} frees the rest for other jobs")))
        }
        Some(a) if pinned * 2 > used.len() => (
            Some((a * 2.0).ceil()),
            Some(format!("speed: it was held at its --cpus {a} in {pinned} of {} runs; a bigger --cpus may finish sooner", used.len())),
        ),
        Some(_) => (None, None),
        None => {
            let s = round_cpus(p95.unwrap_or(avg));
            (Some(s), Some(format!("none-asked: it gave no --cpus and used about {avg:.1} cores; --cpus {s} reserves that")))
        }
    };
    CpuFacts { samples: used.len(), avg_used: Some(avg), p95_used: p95, asked, pinned_runs: pinned, suggest, why }
}

/// `low` (< 5 finished runs), `medium` (5–19), `high` (20+).
pub fn confidence(samples: usize) -> &'static str {
    match samples {
        0..=4 => "low",
        5..=19 => "medium",
        _ => "high",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn done(peak_g: f64, secs: i64, cpu_s: u64, cpus: Option<f64>, at: i64) -> Run {
        Run {
            job_id: format!("j{at}"),
            state: "done".into(),
            peak_mem: Some((peak_g * GIB as f64) as u64),
            mem_limit: Some(24 * GIB),
            secs: Some(secs),
            cpu_ms: Some(cpu_s * 1000),
            cpus_asked: cpus,
            ended_at: at,
            ..Default::default()
        }
    }

    fn mem_cap(peak: Option<u64>, limit: u64, at: i64) -> Run {
        Run { job_id: format!("c{at}"), state: "failed".into(), reason: Some("mem_cap".into()), peak_mem: peak, mem_limit: Some(limit), ended_at: at, ..Default::default() }
    }

    #[test]
    fn mem_suggests_p95_with_headroom_rounded_up() {
        // 20 runs, peaks 1..=20 GiB: p95 = 19G; × 1.25 = 23.75 → 24G.
        let runs: Vec<Run> = (1..=20).rev().map(|g| done(g as f64, 60, 60, Some(4.0), g)).collect();
        let f = mem_facts(&runs);
        assert_eq!((f.samples, f.p95_bytes, f.max_bytes, f.p50_bytes), (20, Some(19 * GIB), Some(20 * GIB), Some(10 * GIB)));
        assert_eq!(f.suggest_bytes, Some(24 * GIB));
        assert_eq!(f.asked_bytes, Some(24 * GIB));
        let (v, from) = auto_mem(&runs, None).unwrap();
        assert_eq!(v, 24 * GIB);
        assert_eq!(from, "auto (p95 19G of 20 runs)");
        assert_eq!(confidence(f.samples), "high");
    }

    /// §10 R1: after a `mem_cap` at a 16G peak, the next auto value is
    /// at least 24G, even when the finished runs say less.
    #[test]
    fn a_mem_cap_end_floors_the_next_value_at_one_and_a_half_times_its_peak() {
        let runs = vec![mem_cap(Some(16 * GIB), 16 * GIB, 30), done(9.0, 60, 60, None, 20), done(10.0, 60, 60, None, 10)];
        let f = mem_facts(&runs);
        assert_eq!((f.mem_cap_ends, f.last_mem_cap_peak_bytes, f.floor_bytes), (1, Some(16 * GIB), Some(24 * GIB)));
        let (v, from) = auto_mem(&runs, None).unwrap();
        assert!(v >= 24 * GIB, "{v}");
        assert_eq!(v, 24 * GIB);
        assert!(from.contains("at least 1.5 × the 16G peak of a mem_cap end"), "{from}");
        // A kill with no reported peak counts at its limit.
        let (v, _) = auto_mem(&[mem_cap(None, 7 * GIB, 5)], None).unwrap();
        assert_eq!(v, 11 * GIB, "1.5 × 7G = 10.5G → 11G");
    }

    /// §10 R1: no history and no fallback is `mem_auto_no_history`; with a
    /// fallback, the fallback.
    #[test]
    fn no_history_needs_a_fallback() {
        assert_eq!(auto_mem(&[], None), None);
        assert_eq!(auto_mem(&[], Some(8 * GIB)), Some((8 * GIB, "auto (no history yet: the fallback 8G)".to_string())));
        // A finished run without a peak (an old node) is no history either.
        let mut r = done(1.0, 60, 60, None, 1);
        r.peak_mem = None;
        assert_eq!(auto_mem(std::slice::from_ref(&r), None), None);
        // A mem_cap end beats a smaller fallback.
        let (v, _) = auto_mem(&[mem_cap(Some(16 * GIB), 16 * GIB, 1)], Some(8 * GIB)).unwrap();
        assert_eq!(v, 24 * GIB);
    }

    #[test]
    fn cpus_say_cost_speed_or_what_it_used() {
        // Asked 8, used ~2: cost.
        let runs: Vec<Run> = (0..6).map(|i| done(1.0, 100, 200, Some(8.0), i)).collect();
        let c = cpu_facts(&runs);
        assert_eq!((c.samples, c.asked, c.pinned_runs), (6, Some(8.0), 0));
        assert!((c.avg_used.unwrap() - 2.0).abs() < 1e-9);
        assert_eq!(c.suggest, Some(3.0), "2 × 1.25 = 2.5 → 3");
        assert!(c.why.as_deref().unwrap().starts_with("cost:"), "{c:?}");
        // Asked 4, used ~4 every run: speed.
        let runs: Vec<Run> = (0..6).map(|i| done(1.0, 100, 395, Some(4.0), i)).collect();
        let c = cpu_facts(&runs);
        assert_eq!((c.pinned_runs, c.suggest), (6, Some(8.0)));
        assert!(c.why.as_deref().unwrap().starts_with("speed:"), "{c:?}");
        // None asked.
        let runs: Vec<Run> = (0..3).map(|i| done(1.0, 100, 150, None, i)).collect();
        let c = cpu_facts(&runs);
        assert_eq!(c.suggest, Some(2.0));
        assert!(c.why.as_deref().unwrap().starts_with("none-asked:"), "{c:?}");
        // Asked 4, used ~3: fine as it is.
        let runs: Vec<Run> = (0..3).map(|i| done(1.0, 100, 300, Some(4.0), i)).collect();
        assert_eq!(cpu_facts(&runs).suggest, None);
        assert_eq!(confidence(3), "low");
        assert_eq!(confidence(5), "medium");
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_job(conn: &Connection, id: &str, ws: &str, argv: &[&str], tag: Option<&str>, state: &str, reason: Option<&str>, ended: i64, summary: Option<&str>, shards: bool) {
        let mut plan = serde_json::json!({"limits": {"max_secs": 3600, "cpu_millis": 4000, "mem_bytes": 8u64 << 30, "disk_bytes": 0, "log_cap_bytes": 0}});
        let mut ctl = serde_json::json!({});
        if let Some(t) = tag {
            ctl["tag"] = t.into();
        }
        if shards {
            ctl["shards"] = serde_json::json!(["a", "b"]);
        }
        plan["k2ctl"] = ctl;
        conn.execute(
            "INSERT INTO compute_jobs (id, client_job_id, node_id, workspace_id, requested_by, argv_json, env_names_json, request_sha256, plan_json, \
             state, reason, plan_digest, created_at, started_at, ended_at, summary_json) VALUES (?1, ?1, 'n1', ?2, 'owner', ?3, '[]', '', ?4, ?5, ?6, '', ?7, ?7, ?8, ?9)",
            params![id, ws, serde_json::to_string(argv).unwrap(), plan.to_string(), state, reason, ended - 100, ended, summary],
        )
        .unwrap();
    }

    #[test]
    fn runs_reads_one_workspace_and_key_newest_first() {
        let _db = crate::db::scoped_for_test();
        let db = crate::db::shared();
        let conn = db.lock();
        let res = |g: u64| format!(r#"{{"artifacts":[],"resources":{{"peak_mem_bytes":{},"mem_source":"cgroup","cpu_ms":250000,"cpu_source":"cgroup"}}}}"#, g << 30);
        insert_job(&conn, "a1", "ws", &["cargo", "test"], None, "done", None, 1000, Some(&res(5)), false);
        insert_job(&conn, "a2", "ws", &["cargo", "test"], None, "done", None, 2000, Some(&res(6)), false);
        insert_job(&conn, "a3", "ws", &["cargo", "test"], None, "failed", Some("mem_cap"), 3000, None, false);
        // Not counted: another state, another workspace, another argv, a split run's parent, too old.
        insert_job(&conn, "x1", "ws", &["cargo", "test"], None, "failed", Some("spawn"), 2500, None, false);
        insert_job(&conn, "x2", "other", &["cargo", "test"], None, "done", None, 2600, Some(&res(40)), false);
        insert_job(&conn, "x3", "ws", &["cargo", "build"], None, "done", None, 2700, Some(&res(40)), false);
        insert_job(&conn, "x4", "ws", &["cargo", "test"], None, "done", None, 2800, None, true);
        insert_job(&conn, "x5", "ws", &["cargo", "test"], None, "done", None, 10, Some(&res(40)), false);
        let key = Key::Argv(vec!["cargo".into(), "test".into()]);
        let r = runs(&conn, "ws", &key, 100, 50);
        assert_eq!(r.iter().map(|r| r.job_id.as_str()).collect::<Vec<_>>(), vec!["a3", "a2", "a1"]);
        assert_eq!((r[1].peak_mem, r[1].cpu_ms, r[1].secs, r[1].cpus_asked, r[1].mem_limit), (Some(6 << 30), Some(250_000), Some(100), Some(4.0), Some(8 << 30)));
        assert!(r[0].is_mem_cap());
        // By tag: a check's jobs.
        insert_job(&conn, "c1", "ws", &["make", "ci"], Some("ci:unit"), "done", None, 1500, Some(&res(3)), false);
        let r = runs(&conn, "ws", &Key::check("unit"), 0, 50);
        assert_eq!(r.iter().map(|r| r.job_id.as_str()).collect::<Vec<_>>(), vec!["c1"]);
        assert_eq!(Key::check("unit").json()["kind"], "check");
        // The limit holds.
        assert_eq!(runs(&conn, "ws", &key, 0, 1).len(), 1);
    }
}
