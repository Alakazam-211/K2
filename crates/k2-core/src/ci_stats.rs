//! `k2 ci stats` (prd-compute-blacksmith-class-v1 §4.5.2, slice O1): per
//! check, over a window of a workspace's CI runs, the p50/p95 of its run
//! time and of its queue wait, its failure rate, its flaky rate (§4.3), its
//! CPU-hours, and its cache hit rate (§4.1, once nodes fork generations).
//! No table of its own: `ci_runs`, `ci_checks`, `compute_jobs` and the test
//! history (`ci_test_jobs`). Release runs are left out.

use std::collections::BTreeMap;

use rusqlite::{params, Connection};
use serde::Serialize;

/// Largest window.
pub const MAX_DAYS: i64 = 90;
pub const DEFAULT_DAYS: i64 = 14;
/// At most this many check rows are read per call.
pub const MAX_ROWS: i64 = 20_000;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckStats {
    pub check: String,
    /// Ended checks in the window (success, failure, error).
    pub runs: i64,
    pub passed: i64,
    pub failed: i64,
    pub errors: i64,
    /// failed ÷ (passed + failed): a check that never ran its job (error)
    /// counts apart.
    pub failure_rate: Option<f64>,
    pub p50_secs: Option<i64>,
    pub p95_secs: Option<i64>,
    pub p50_wait_secs: Option<i64>,
    pub p95_wait_secs: Option<i64>,
    /// Jobs whose test record had a flaky case (or a nextest retry) ÷ jobs
    /// with a test record.
    pub flaky_rate: Option<f64>,
    pub test_jobs: i64,
    /// CPU time of its jobs, in hours (as the nodes reported it).
    pub cpu_hours: f64,
    /// Jobs that started from a cache generation ÷ jobs that reported one
    /// (`None`: no node reported generations yet).
    pub cache_hit_rate: Option<f64>,
    pub cache_jobs: i64,
}

fn pct(sorted: &[i64], p: usize) -> Option<i64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = (p * sorted.len()).div_ceil(100).max(1);
    sorted.get(rank - 1).copied()
}

#[derive(Default)]
struct Acc {
    s: CheckStats,
    durs: Vec<i64>,
    waits: Vec<i64>,
    flaky_jobs: i64,
    cache_hits: i64,
}

/// Per check (by name), ended checks of runs created at or after `since`.
/// `check`/`branch` narrow it.
pub fn stats(conn: &Connection, ws: &str, check: Option<&str>, branch: Option<&str>, since: i64) -> Vec<CheckStats> {
    let sql = "SELECT c.name, c.state, j.created_at, j.started_at, j.ended_at, j.summary_json, t.state, t.flaky, t.retried \
               FROM ci_checks c JOIN ci_runs r ON r.id = c.run_id \
               LEFT JOIN compute_jobs j ON j.id = c.job_id \
               LEFT JOIN ci_test_jobs t ON t.job_id = c.job_id \
               WHERE r.workspace_id = ?1 AND r.kind = 'checks' AND r.created_at >= ?2 \
               AND c.state IN ('success','failure','error') \
               AND (?3 IS NULL OR c.name = ?3) AND (?4 IS NULL OR r.ref = ?4) \
               ORDER BY r.created_at DESC LIMIT ?5";
    #[allow(clippy::type_complexity)]
    let rows: Vec<(String, String, Option<i64>, Option<i64>, Option<i64>, Option<String>, Option<String>, Option<i64>, Option<i64>)> = conn
        .prepare(sql)
        .and_then(|mut st| {
            st.query_map(params![ws, since, check, branch, MAX_ROWS], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?))
            })
            .map(|rows| rows.filter_map(Result::ok).collect())
        })
        .unwrap_or_default();
    let mut by: BTreeMap<String, Acc> = BTreeMap::new();
    for (name, state, created, started, ended, summary, tstate, flaky, retried) in rows {
        let a = by.entry(name.clone()).or_default();
        a.s.check = name;
        a.s.runs += 1;
        match state.as_str() {
            "success" => a.s.passed += 1,
            "failure" => a.s.failed += 1,
            _ => a.s.errors += 1,
        }
        if let (Some(s), Some(e)) = (started, ended) {
            a.durs.push((e - s).max(0));
        }
        if let (Some(c), Some(s)) = (created, started) {
            a.waits.push((s - c).max(0));
        }
        let out: Option<serde_json::Value> = summary.as_deref().and_then(|s| serde_json::from_str(s).ok());
        if let Some(ms) = out.as_ref().and_then(|o| o.get("resources")).and_then(|r| r.get("cpu_ms")).and_then(serde_json::Value::as_u64) {
            a.s.cpu_hours += ms as f64 / 3_600_000.0;
        }
        if let Some(c) = out.as_ref().and_then(|o| o.get("cache")).filter(|c| c.is_object()) {
            a.s.cache_jobs += 1;
            if c.get("lineage").is_some_and(|l| l.is_string()) {
                a.cache_hits += 1;
            }
        }
        if tstate.as_deref().is_some_and(|t| t != "none") {
            a.s.test_jobs += 1;
            if flaky.unwrap_or(0) > 0 || retried.unwrap_or(0) > 0 {
                a.flaky_jobs += 1;
            }
        }
    }
    by.into_values()
        .map(|mut a| {
            a.durs.sort_unstable();
            a.waits.sort_unstable();
            let decided = a.s.passed + a.s.failed;
            a.s.failure_rate = (decided > 0).then(|| a.s.failed as f64 / decided as f64);
            a.s.p50_secs = pct(&a.durs, 50);
            a.s.p95_secs = pct(&a.durs, 95);
            a.s.p50_wait_secs = pct(&a.waits, 50);
            a.s.p95_wait_secs = pct(&a.waits, 95);
            a.s.flaky_rate = (a.s.test_jobs > 0).then(|| a.flaky_jobs as f64 / a.s.test_jobs as f64);
            a.s.cache_hit_rate = (a.s.cache_jobs > 0).then(|| a.cache_hits as f64 / a.s.cache_jobs as f64);
            a.s.cpu_hours = (a.s.cpu_hours * 1000.0).round() / 1000.0;
            a.s
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_check_rates_percentiles_and_filters() {
        let _db = crate::db::scoped_for_test();
        let db = crate::db::shared();
        let conn = db.lock();
        let sha = "a".repeat(40);
        let mut n = 0;
        let mut add = |git_ref: &str, check: &str, state: &str, wait: i64, secs: i64, cpu_ms: u64, flaky: i64, cache: Option<&str>, kind: &str| {
            n += 1;
            let run = format!("{n:0>32}");
            let at = 1_000 + n * 100;
            crate::ci::insert_run(
                &conn,
                &crate::ci::NewRun { id: &run, workspace_id: "ws", sha: &sha, git_ref: Some(git_ref), trigger: "watch", state: "passed", detail: None, created_by: "watch", tier: "ci", statuses: None, at },
            )
            .unwrap();
            conn.execute("UPDATE ci_runs SET kind = ?2 WHERE id = ?1", params![run, kind]).unwrap();
            let job = format!("job{n}");
            let mut summary = serde_json::json!({"artifacts": [], "resources": {"cpu_ms": cpu_ms, "mem_source": "x", "cpu_source": "x"}});
            if let Some(c) = cache {
                summary["cache"] = if c == "cold" { serde_json::json!({"fork_ms": 0, "committed": false, "why": "cold"}) } else { serde_json::json!({"lineage": c, "fork_ms": 900, "committed": false, "why": "x"}) };
            }
            conn.execute(
                "INSERT INTO compute_jobs (id, client_job_id, node_id, workspace_id, requested_by, argv_json, env_names_json, request_sha256, plan_json, \
                 state, plan_digest, created_at, started_at, ended_at, summary_json) VALUES (?1, ?1, 'n1', 'ws', 'owner', '[]', '[]', '', '{}', 'done', '', ?2, ?3, ?4, ?5)",
                params![job, at, at + wait, at + wait + secs, summary.to_string()],
            )
            .unwrap();
            crate::ci::insert_check(&conn, &run, check, "pending", "").unwrap();
            crate::ci::set_check(&conn, &run, check, Some(&job), state, "x");
            conn.execute(
                "INSERT INTO ci_test_jobs (job_id, workspace_id, check_name, run_id, state, total, flaky, at) VALUES (?1, 'ws', ?2, ?3, 'done', 10, ?4, ?5)",
                params![job, check, run, flaky, at],
            )
            .unwrap();
        };
        add("main", "unit", "success", 10, 100, 3_600_000, 0, Some("trusted"), "checks");
        add("main", "unit", "failure", 20, 200, 3_600_000, 1, Some("cold"), "checks");
        add("main", "unit", "success", 30, 300, 0, 0, None, "checks");
        add("integ/x", "unit", "success", 40, 400, 0, 0, None, "checks");
        add("main", "lint", "error", 0, 10, 0, 0, None, "checks");
        add("main", "unit", "success", 0, 9999, 0, 0, None, "release");
        let s = stats(&conn, "ws", None, None, 0);
        assert_eq!(s.iter().map(|c| c.check.as_str()).collect::<Vec<_>>(), vec!["lint", "unit"], "by name; release runs left out");
        let u = &s[1];
        assert_eq!((u.runs, u.passed, u.failed, u.errors), (4, 3, 1, 0));
        assert_eq!(u.failure_rate, Some(0.25));
        assert_eq!((u.p50_secs, u.p95_secs, u.p50_wait_secs, u.p95_wait_secs), (Some(200), Some(400), Some(20), Some(40)));
        assert_eq!((u.test_jobs, u.flaky_rate), (4, Some(0.25)));
        assert_eq!(u.cpu_hours, 2.0);
        assert_eq!((u.cache_jobs, u.cache_hit_rate), (2, Some(0.5)));
        let l = &s[0];
        assert_eq!((l.errors, l.failure_rate, l.cache_hit_rate), (1, None, None));
        let main = stats(&conn, "ws", Some("unit"), Some("main"), 0);
        assert_eq!((main.len(), main[0].runs), (1, 3));
        assert!(stats(&conn, "other", None, None, 0).is_empty(), "per workspace");
        assert!(stats(&conn, "ws", None, None, 1_000_000).is_empty(), "the window");
    }
}
