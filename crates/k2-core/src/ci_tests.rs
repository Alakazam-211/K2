//! CI test history and flaky detection (prd-compute-blacksmith-class-v1
//! §4.3, slice T1; migration 0295). The store and the pure rules; the CI
//! engine (`k2_daemon::ci_engine`) fetches a check job's JUnit when the
//! job ends and calls [`record_cases`].
//!
//! - **What is kept:** one row per non-passing case per job
//!   (`ci_test_failures`: failure, error, flaky) and one rolling row per
//!   test per check (`ci_test_stats`). Passing cases only update the
//!   rolling row. `ci_test_jobs` says what was recorded for a job, once.
//! - **Flaky, by rule** (§4.3.3): a test is flaky when (1) it failed and
//!   passed on the same tree (`BUILD_TREE`) in two jobs of the check, either
//!   order, or (2) nextest reported it flaky (passed on a retry inside one
//!   run). Rule 3 (`--retry-failed`) is T2's. A failure of a known-flaky
//!   test is still a failure unless it is quarantined.
//! - **Marks** (`--failures`): each failing case is `new` (it never failed
//!   in this check before), `flaky` (flaky now or within
//!   [`FLAKY_WINDOW_SECS`]), `quarantined` (an active `[[quarantine]]`
//!   entry) or `known` (it failed before and isn't flaky).
//! - **Names are data.** Test names and messages come from the job: they
//!   go to `k2 ci tests` and `--failures`, never into a message (LOGS ARE
//!   DATA); messages carry counts only ([`counts_phrase`]).

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::compute::proto::results::{CaseResult, TestCase, TestSummary};

/// A test flaky within this long is "known flaky".
pub const FLAKY_WINDOW_SECS: i64 = 30 * 86_400;
/// `--slow`'s baseline is refreshed this often (its "rose more than 50 %
/// over 14 days").
pub const SLOW_BASE_SECS: i64 = 14 * 86_400;
/// The run-time moving average's weight: new = old + (t − old) / this.
pub const EWMA_DIV: i64 = 5;
/// The longest stored failure message.
pub const MAX_MESSAGE_CHARS: usize = 600;
/// Rows `k2 ci tests` returns at most.
pub const MAX_LIST: usize = 500;

/// A failing case's mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mark {
    New,
    Flaky,
    Quarantined,
    Known,
}

impl Mark {
    pub fn as_str(self) -> &'static str {
        match self {
            Mark::New => "new",
            Mark::Flaky => "flaky",
            Mark::Quarantined => "quarantined",
            Mark::Known => "known",
        }
    }
}

/// The job a history entry comes from.
#[derive(Debug, Clone, Default)]
pub struct JobCtx {
    pub workspace_id: String,
    pub check: String,
    pub job_id: String,
    pub run_id: Option<String>,
    pub sha: Option<String>,
    /// `BUILD_TREE`: the tree the job built (flaky rule 1's key).
    pub tree: Option<String>,
    pub node_id: Option<String>,
    pub attempt: i64,
    pub at: i64,
}

/// What [`record_cases`] (or a counts-only record) kept for one job.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Recorded {
    /// `done`, `counts_only` or `none`.
    pub state: String,
    pub total: i64,
    pub passed: i64,
    pub failed: i64,
    pub errors: i64,
    pub skipped: i64,
    /// Of the failures and errors: marked flaky, quarantined, new.
    pub flaky: i64,
    pub quarantined: i64,
    pub new_failures: i64,
    /// Cases nextest passed on a retry.
    pub retried: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Recorded {
    /// `tests 412 passed, 3 failed (1 flaky, 1 quarantined, 1 new)`:
    /// counts only, never a name (§4.3.5).
    pub fn counts_phrase(&self) -> String {
        counts_phrase(self.passed, self.failed + self.errors, self.flaky, self.quarantined, self.new_failures)
    }
}

/// The message phrase for test counts (§4.3.5). The breakdown names only
/// the marks that are non-zero.
pub fn counts_phrase(passed: i64, failed: i64, flaky: i64, quarantined: i64, new: i64) -> String {
    let mut s = format!("tests {passed} passed, {failed} failed");
    let parts: Vec<String> = [(flaky, "flaky"), (quarantined, "quarantined"), (new, "new")]
        .iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, w)| format!("{n} {w}"))
        .collect();
    if failed > 0 && !parts.is_empty() {
        s.push_str(&format!(" ({})", parts.join(", ")));
    }
    s
}

/// The job's record, if it has one.
pub fn recorded(conn: &Connection, job_id: &str) -> Option<Recorded> {
    conn.query_row(
        "SELECT state, total, passed, failed, errors, skipped, flaky, quarantined, new_failures, retried, note FROM ci_test_jobs WHERE job_id = ?1",
        params![job_id],
        |r| {
            Ok(Recorded {
                state: r.get(0)?,
                total: r.get(1)?,
                passed: r.get(2)?,
                failed: r.get(3)?,
                errors: r.get(4)?,
                skipped: r.get(5)?,
                flaky: r.get(6)?,
                quarantined: r.get(7)?,
                new_failures: r.get(8)?,
                retried: r.get(9)?,
                note: r.get(10)?,
            })
        },
    )
    .optional()
    .ok()
    .flatten()
}

/// prd-compute-blacksmith-class-v1 O1 (§4.5.4): every recorded job of one
/// CI run summed, for the `[ci]` run message (counts only). `None` when no
/// check of the run recorded any test.
pub fn run_totals(conn: &Connection, run_id: &str) -> Option<Recorded> {
    conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(total),0), COALESCE(SUM(passed),0), COALESCE(SUM(failed),0), COALESCE(SUM(errors),0), \
         COALESCE(SUM(skipped),0), COALESCE(SUM(flaky),0), COALESCE(SUM(quarantined),0), COALESCE(SUM(new_failures),0), \
         COALESCE(SUM(retried),0) FROM ci_test_jobs WHERE run_id = ?1",
        params![run_id],
        |r| {
            Ok((
                r.get::<_, i64>(0)?,
                Recorded {
                    state: "done".into(),
                    total: r.get(1)?,
                    passed: r.get(2)?,
                    failed: r.get(3)?,
                    errors: r.get(4)?,
                    skipped: r.get(5)?,
                    flaky: r.get(6)?,
                    quarantined: r.get(7)?,
                    new_failures: r.get(8)?,
                    retried: r.get(9)?,
                    note: None,
                },
            ))
        },
    )
    .ok()
    .filter(|(jobs, r)| *jobs > 0 && r.total > 0)
    .map(|(_, r)| r)
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut o: String = s.chars().take(max.saturating_sub(1)).collect();
        o.push('…');
        o
    }
}

fn insert_job(conn: &Connection, c: &JobCtx, r: &Recorded) -> Result<(), String> {
    conn.execute(
        "INSERT INTO ci_test_jobs (job_id, workspace_id, check_name, run_id, sha, tree, state, total, passed, failed, errors, skipped, flaky, quarantined, new_failures, retried, note, at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
        params![
            c.job_id,
            c.workspace_id,
            c.check,
            c.run_id,
            c.sha,
            c.tree,
            r.state,
            r.total,
            r.passed,
            r.failed,
            r.errors,
            r.skipped,
            r.flaky,
            r.quarantined,
            r.new_failures,
            r.retried,
            r.note,
            c.at
        ],
    )
    .map_err(|e| format!("ci_test_jobs: {e}"))?;
    Ok(())
}

/// Record a job with no cases: `none` (no JUnit) or `counts_only` (over the
/// cap, a fetch or parse failed: the node's own counts are kept). Once per
/// job; a second call returns the first record.
pub fn record_counts_only(conn: &Connection, c: &JobCtx, summary: Option<&TestSummary>, note: &str) -> Result<Recorded, String> {
    if let Some(r) = recorded(conn, &c.job_id) {
        return Ok(r);
    }
    let r = match summary {
        Some(t) => Recorded {
            state: "counts_only".into(),
            total: t.total as i64,
            passed: t.passed as i64,
            failed: t.failed as i64,
            errors: t.errors as i64,
            skipped: t.skipped as i64,
            note: Some(clip(note, 300)),
            ..Default::default()
        },
        None => Recorded { state: "none".into(), note: Some(clip(note, 300)), ..Default::default() },
    };
    insert_job(conn, c, &r)?;
    Ok(r)
}

struct Stats {
    runs: i64,
    fails: i64,
    flakes: i64,
    ewma_ms: Option<i64>,
    base_ms: Option<i64>,
    base_at: Option<i64>,
    last_flake_at: Option<i64>,
    last_pass_tree: Option<String>,
}

/// Record every case of one check job (§4.3.2) and apply the flaky rules
/// (§4.3.3). `quarantined(test_id)` says whether an active quarantine
/// entry covers a test. Once per job: a second call returns the first
/// record and changes nothing.
pub fn record_cases(conn: &Connection, c: &JobCtx, cases: &[TestCase], quarantined: &dyn Fn(&str) -> bool) -> Result<Recorded, String> {
    if let Some(r) = recorded(conn, &c.job_id) {
        return Ok(r);
    }
    let tx = conn.unchecked_transaction().map_err(|e| format!("ci tests: {e}"))?;
    // Rule 1, earlier failure → this pass: the failures on this tree in
    // the check's other jobs (a small set).
    let mut failed_on_tree = std::collections::HashSet::new();
    if let Some(tree) = &c.tree {
        let mut st = tx
            .prepare_cached(
                "SELECT DISTINCT test_id FROM ci_test_failures WHERE workspace_id = ?1 AND check_name = ?2 AND tree = ?3 AND kind IN ('failure','error') AND job_id != ?4",
            )
            .map_err(|e| format!("ci tests: {e}"))?;
        let rows = st.query_map(params![c.workspace_id, c.check, tree, c.job_id], |r| r.get::<_, String>(0)).map_err(|e| format!("ci tests: {e}"))?;
        for id in rows {
            failed_on_tree.insert(id.map_err(|e| format!("ci tests: {e}"))?);
        }
    }
    let mut out = Recorded { state: "done".into(), ..Default::default() };
    for case in cases {
        let id = case.test_id();
        out.total += 1;
        match case.result {
            CaseResult::Passed | CaseResult::Flaky => out.passed += 1,
            CaseResult::Failed => out.failed += 1,
            CaseResult::Error => out.errors += 1,
            CaseResult::Skipped => {
                out.skipped += 1;
                continue;
            }
        }
        let s = tx
            .query_row(
                "SELECT runs, fails, flakes, ewma_ms, base_ms, base_at, last_flake_at, last_pass_tree FROM ci_test_stats WHERE workspace_id = ?1 AND check_name = ?2 AND test_id = ?3",
                params![c.workspace_id, c.check, id],
                |r| {
                    Ok(Stats {
                        runs: r.get(0)?,
                        fails: r.get(1)?,
                        flakes: r.get(2)?,
                        ewma_ms: r.get(3)?,
                        base_ms: r.get(4)?,
                        base_at: r.get(5)?,
                        last_flake_at: r.get(6)?,
                        last_pass_tree: r.get(7)?,
                    })
                },
            )
            .optional()
            .map_err(|e| format!("ci tests: {e}"))?;
        let is_fail = case.result.is_failure();
        let passed = matches!(case.result, CaseResult::Passed | CaseResult::Flaky);
        let same_tree_pass = is_fail && c.tree.is_some() && s.as_ref().and_then(|s| s.last_pass_tree.as_deref()) == c.tree.as_deref();
        let flaky_now = case.result == CaseResult::Flaky || (passed && failed_on_tree.contains(&id)) || same_tree_pass;
        let known_flaky = s.as_ref().is_some_and(|s| s.flakes > 0 && s.last_flake_at.is_some_and(|t| c.at - t <= FLAKY_WINDOW_SECS));
        if case.result == CaseResult::Flaky {
            out.retried += 1;
        }
        // Stats: runs, fails, flakes, the time average, the last trees.
        let (runs, fails, flakes) = s.as_ref().map(|s| (s.runs, s.fails, s.flakes)).unwrap_or((0, 0, 0));
        let old = s.as_ref().and_then(|s| s.ewma_ms);
        let ewma = match (old, case.time_ms) {
            (Some(o), Some(t)) => Some(o + (t as i64 - o) / EWMA_DIV),
            (None, Some(t)) => Some(t as i64),
            (o, None) => o,
        };
        let (base_ms, base_at) = match s.as_ref().and_then(|s| s.base_at) {
            Some(at) if c.at - at < SLOW_BASE_SECS => (s.as_ref().and_then(|s| s.base_ms), Some(at)),
            _ => (old.or(ewma), Some(c.at)),
        };
        tx.execute(
            "INSERT INTO ci_test_stats (workspace_id, check_name, test_id, classname, name, runs, fails, flakes, ewma_ms, base_ms, base_at, first_seen, last_seen, last_fail_at, last_flake_at, last_flake_tree, last_pass_tree)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12, ?13, ?14, ?15, ?16)
             ON CONFLICT (workspace_id, check_name, test_id) DO UPDATE SET
               classname = excluded.classname, name = excluded.name, runs = excluded.runs, fails = excluded.fails,
               flakes = excluded.flakes, ewma_ms = excluded.ewma_ms, base_ms = excluded.base_ms, base_at = excluded.base_at,
               last_seen = excluded.last_seen,
               last_fail_at = COALESCE(excluded.last_fail_at, ci_test_stats.last_fail_at),
               last_flake_at = COALESCE(excluded.last_flake_at, ci_test_stats.last_flake_at),
               last_flake_tree = COALESCE(excluded.last_flake_tree, ci_test_stats.last_flake_tree),
               last_pass_tree = COALESCE(excluded.last_pass_tree, ci_test_stats.last_pass_tree)",
            params![
                c.workspace_id,
                c.check,
                id,
                case.classname,
                case.name,
                runs + 1,
                fails + i64::from(is_fail),
                flakes + i64::from(flaky_now),
                ewma,
                base_ms,
                base_at,
                c.at,
                is_fail.then_some(c.at),
                flaky_now.then_some(c.at),
                if flaky_now { c.tree.clone() } else { None },
                if passed { c.tree.clone() } else { None },
            ],
        )
        .map_err(|e| format!("ci_test_stats: {e}"))?;
        // A row per non-passing case: failures and errors with their mark,
        // and each flaky pass (the flip rule 1 or nextest saw).
        let (kind, mark) = if is_fail {
            let mark = if quarantined(&id) {
                Mark::Quarantined
            } else if flaky_now || known_flaky {
                Mark::Flaky
            } else if fails == 0 {
                Mark::New
            } else {
                Mark::Known
            };
            match mark {
                Mark::Quarantined => out.quarantined += 1,
                Mark::Flaky => out.flaky += 1,
                Mark::New => out.new_failures += 1,
                Mark::Known => {}
            }
            (if case.result == CaseResult::Error { "error" } else { "failure" }, mark)
        } else if flaky_now {
            ("flaky", Mark::Flaky)
        } else {
            continue;
        };
        tx.execute(
            "INSERT INTO ci_test_failures (workspace_id, check_name, test_id, classname, name, kind, mark, job_id, run_id, sha, tree, node_id, attempt, message, at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                c.workspace_id,
                c.check,
                id,
                case.classname,
                case.name,
                kind,
                mark.as_str(),
                c.job_id,
                c.run_id,
                c.sha,
                c.tree,
                c.node_id,
                c.attempt,
                case.message.as_deref().map(|m| clip(m, MAX_MESSAGE_CHARS)),
                c.at
            ],
        )
        .map_err(|e| format!("ci_test_failures: {e}"))?;
    }
    insert_job(&tx, c, &out)?;
    tx.commit().map_err(|e| format!("ci tests: {e}"))?;
    Ok(out)
}

/// The marks of one job's failing cases, by test id (`--failures`).
pub fn marks_of_job(conn: &Connection, job_id: &str) -> std::collections::BTreeMap<String, String> {
    let mut out = std::collections::BTreeMap::new();
    let Ok(mut st) = conn.prepare_cached("SELECT test_id, mark FROM ci_test_failures WHERE job_id = ?1 AND kind IN ('failure','error')") else {
        return out;
    };
    if let Ok(rows) = st.query_map(params![job_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))) {
        for (id, mark) in rows.flatten() {
            out.insert(id, mark);
        }
    }
    out
}

/// Which list `k2 ci tests` shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    /// Flaky, then failing, then the slowest (a short view of each).
    All,
    Flaky,
    Slow,
    Failing,
    Quarantined,
}

impl Filter {
    pub fn parse(s: &str) -> Option<Filter> {
        Some(match s {
            "" | "all" => Filter::All,
            "flaky" => Filter::Flaky,
            "slow" => Filter::Slow,
            "failing" => Filter::Failing,
            "quarantined" => Filter::Quarantined,
            _ => return None,
        })
    }
}

/// One test in `k2 ci tests`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestRow {
    pub check: String,
    pub test: String,
    /// Why it is listed: `flaky`, `failing`, `slow`, `quarantined`.
    pub why: String,
    pub runs: i64,
    pub fails: i64,
    pub flakes: i64,
    /// In the window: jobs of the check, and this test's flips / failures.
    pub window_jobs: i64,
    pub window_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ewma_ms: Option<i64>,
    /// `--slow`: the time rose more than 50 % over the baseline.
    pub rose: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_fail_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_flake_at: Option<i64>,
    /// The last tree it flipped on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_flake_tree: Option<String>,
    pub last_seen: i64,
}

fn window_jobs(conn: &Connection, ws: &str, check: &str, since: i64) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM ci_test_jobs WHERE workspace_id = ?1 AND check_name = ?2 AND at >= ?3 AND state = 'done'",
        params![ws, check, since],
        |r| r.get(0),
    )
    .unwrap_or(0)
}

/// `k2 ci tests`: the history of one workspace (one check, or all), since
/// `since`. At most `limit` rows per list.
pub fn list(conn: &Connection, ws: &str, check: Option<&str>, filter: Filter, since: i64, limit: usize) -> Result<Vec<TestRow>, String> {
    let limit = limit.clamp(1, MAX_LIST) as i64;
    let mut out = Vec::new();
    let lists: &[Filter] = match filter {
        Filter::All => &[Filter::Flaky, Filter::Failing, Filter::Slow],
        Filter::Flaky => &[Filter::Flaky],
        Filter::Slow => &[Filter::Slow],
        Filter::Failing => &[Filter::Failing],
        Filter::Quarantined => &[Filter::Quarantined],
    };
    let base = "SELECT s.check_name, s.test_id, s.runs, s.fails, s.flakes, s.ewma_ms, s.base_ms, s.last_fail_at, s.last_flake_at, s.last_flake_tree, s.last_seen";
    for f in lists {
        let (sql, why, count_sql) = match f {
            Filter::Flaky => (
                format!("{base}, 0 FROM ci_test_stats s WHERE s.workspace_id = ?1 AND (?2 IS NULL OR s.check_name = ?2) AND s.flakes > 0 AND s.last_flake_at >= ?3 ORDER BY s.last_flake_at DESC LIMIT ?4"),
                "flaky",
                Some("SELECT COUNT(*) FROM ci_test_failures WHERE workspace_id = ?1 AND check_name = ?2 AND test_id = ?3 AND mark = 'flaky' AND at >= ?4"),
            ),
            Filter::Failing => (
                format!("{base}, 0 FROM ci_test_stats s WHERE s.workspace_id = ?1 AND (?2 IS NULL OR s.check_name = ?2) AND s.last_fail_at >= ?3 ORDER BY s.last_fail_at DESC LIMIT ?4"),
                "failing",
                Some("SELECT COUNT(*) FROM ci_test_failures WHERE workspace_id = ?1 AND check_name = ?2 AND test_id = ?3 AND kind IN ('failure','error') AND at >= ?4"),
            ),
            Filter::Slow => (
                format!("{base}, 0 FROM ci_test_stats s WHERE s.workspace_id = ?1 AND (?2 IS NULL OR s.check_name = ?2) AND s.last_seen >= ?3 AND s.ewma_ms IS NOT NULL ORDER BY (s.base_ms > 0 AND s.ewma_ms * 2 > s.base_ms * 3) DESC, s.ewma_ms DESC LIMIT ?4"),
                "slow",
                None,
            ),
            Filter::Quarantined => (
                format!("{base}, 0 FROM ci_test_stats s WHERE s.workspace_id = ?1 AND (?2 IS NULL OR s.check_name = ?2) AND EXISTS (SELECT 1 FROM ci_test_failures q WHERE q.workspace_id = s.workspace_id AND q.check_name = s.check_name AND q.test_id = s.test_id AND q.mark = 'quarantined' AND q.at >= ?3) ORDER BY s.last_fail_at DESC LIMIT ?4"),
                "quarantined",
                Some("SELECT COUNT(*) FROM ci_test_failures WHERE workspace_id = ?1 AND check_name = ?2 AND test_id = ?3 AND mark = 'quarantined' AND at >= ?4"),
            ),
            Filter::All => unreachable!("expanded above"),
        };
        let mut st = conn.prepare(&sql).map_err(|e| format!("ci tests: {e}"))?;
        let rows = st
            .query_map(params![ws, check, since, limit], |r| {
                let ewma: Option<i64> = r.get(5)?;
                let base: Option<i64> = r.get(6)?;
                Ok(TestRow {
                    check: r.get(0)?,
                    test: r.get(1)?,
                    why: why.to_string(),
                    runs: r.get(2)?,
                    fails: r.get(3)?,
                    flakes: r.get(4)?,
                    window_jobs: 0,
                    window_count: 0,
                    ewma_ms: ewma,
                    rose: matches!((ewma, base), (Some(e), Some(b)) if b > 0 && e * 2 > b * 3),
                    last_fail_at: r.get(7)?,
                    last_flake_at: r.get(8)?,
                    last_flake_tree: r.get(9)?,
                    last_seen: r.get(10)?,
                })
            })
            .map_err(|e| format!("ci tests: {e}"))?;
        for row in rows {
            let mut row = row.map_err(|e| format!("ci tests: {e}"))?;
            row.window_jobs = window_jobs(conn, ws, &row.check, since);
            if let Some(q) = count_sql {
                row.window_count = conn.query_row(q, params![ws, row.check, row.test, since], |r| r.get(0)).unwrap_or(0);
            }
            out.push(row);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(include_str!("../drizzle_sql/0295_ci_test_history.sql")).unwrap();
        c
    }

    fn case(name: &str, result: CaseResult, ms: Option<u64>) -> TestCase {
        TestCase { name: name.into(), classname: Some("k2-core".into()), file: None, result, time_ms: ms, retries: 0, message: None }
    }

    fn ctx(job: &str, tree: &str, at: i64) -> JobCtx {
        JobCtx {
            workspace_id: "ws".into(),
            check: "test".into(),
            job_id: job.into(),
            run_id: Some(format!("r-{job}")),
            sha: Some("a".repeat(40)),
            tree: Some(tree.into()),
            node_id: Some("n1".into()),
            attempt: 1,
            at,
        }
    }

    fn no_quarantine(_: &str) -> bool {
        false
    }
    const NOQ: &dyn Fn(&str) -> bool = &no_quarantine;

    /// §10 T1: a case that failed on tree T and passed on tree T is flaky;
    /// on different trees it isn't.
    #[test]
    fn same_tree_fail_then_pass_is_flaky_different_trees_are_not() {
        let c = conn();
        let r = record_cases(&c, &ctx("j1", "T", 100), &[case("a", CaseResult::Failed, Some(10)), case("b", CaseResult::Passed, Some(5))], NOQ).unwrap();
        assert_eq!((r.failed, r.new_failures, r.flaky), (1, 1, 0), "first failure is new");
        let r = record_cases(&c, &ctx("j2", "T", 200), &[case("a", CaseResult::Passed, Some(10)), case("b", CaseResult::Passed, Some(5))], NOQ).unwrap();
        assert_eq!(r.failed, 0);
        let rows = list(&c, "ws", None, Filter::Flaky, 0, 50).unwrap();
        assert_eq!(rows.iter().map(|r| r.test.as_str()).collect::<Vec<_>>(), vec!["k2-core::a"], "{rows:?}");
        assert_eq!(rows[0].last_flake_tree.as_deref(), Some("T"));
        assert_eq!((rows[0].window_count, rows[0].window_jobs), (1, 2), "flaky (1 of the last 2 runs)");

        // Different trees: fail on U, pass on V → not flaky.
        let c = conn();
        record_cases(&c, &ctx("j1", "U", 100), &[case("a", CaseResult::Failed, None)], NOQ).unwrap();
        record_cases(&c, &ctx("j2", "V", 200), &[case("a", CaseResult::Passed, None)], NOQ).unwrap();
        assert!(list(&c, "ws", None, Filter::Flaky, 0, 50).unwrap().is_empty());
        // ... and a later failure of it is `known`, not new.
        let r = record_cases(&c, &ctx("j3", "W", 300), &[case("a", CaseResult::Failed, None)], NOQ).unwrap();
        assert_eq!((r.new_failures, r.flaky), (0, 0));
        assert_eq!(marks_of_job(&c, "j3").get("k2-core::a").map(String::as_str), Some("known"));
    }

    /// Rule 1 the other way round: pass on T, then fail on T → flaky, and
    /// the failure is marked flaky.
    #[test]
    fn same_tree_pass_then_fail_is_flaky() {
        let c = conn();
        record_cases(&c, &ctx("j1", "T", 100), &[case("a", CaseResult::Passed, None)], NOQ).unwrap();
        let r = record_cases(&c, &ctx("j2", "T", 200), &[case("a", CaseResult::Failed, None)], NOQ).unwrap();
        assert_eq!((r.failed, r.flaky, r.new_failures), (1, 1, 0));
        assert_eq!(marks_of_job(&c, "j2").get("k2-core::a").map(String::as_str), Some("flaky"));
        assert_eq!(r.counts_phrase(), "tests 0 passed, 1 failed (1 flaky)");
    }

    /// §10 T1: a nextest JUnit with `flakyFailure` marks the test flaky.
    #[test]
    fn nextest_flaky_case_is_flaky() {
        let c = conn();
        let xml = r#"<testsuites><testsuite name="k2-core"><testcase name="x" classname="k2-core"><flakyFailure message="first try"/></testcase><testcase name="y" classname="k2-core"/></testsuite></testsuites>"#;
        let cases = crate::compute::proto::results::parse_junit_cases(xml, "junit.xml").unwrap();
        let r = record_cases(&c, &ctx("j1", "T", 100), &cases, NOQ).unwrap();
        assert_eq!((r.total, r.passed, r.retried, r.failed), (2, 2, 1, 0));
        let rows = list(&c, "ws", Some("test"), Filter::Flaky, 0, 50).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].test, "k2-core::x");
        // A later failure of the known-flaky test is still a failure, marked flaky.
        let r = record_cases(&c, &ctx("j2", "U", 200), &[case("x", CaseResult::Failed, None)], NOQ).unwrap();
        assert_eq!((r.failed, r.flaky, r.new_failures), (1, 1, 0));
        // Past the window it is no longer known flaky.
        let r = record_cases(&c, &ctx("j3", "V", 200 + FLAKY_WINDOW_SECS + 1), &[case("x", CaseResult::Failed, None)], NOQ).unwrap();
        assert_eq!(r.flaky, 0);
    }

    #[test]
    fn quarantine_marks_and_counts_and_idempotence() {
        let c = conn();
        let q: &dyn Fn(&str) -> bool = &|id| id == "k2-core::q";
        let r = record_cases(&c, &ctx("j1", "T", 100), &[case("q", CaseResult::Failed, None), case("n", CaseResult::Error, None), case("p", CaseResult::Passed, None), case("s", CaseResult::Skipped, None)], q).unwrap();
        assert_eq!((r.total, r.passed, r.failed, r.errors, r.skipped), (4, 1, 1, 1, 1));
        assert_eq!((r.quarantined, r.new_failures), (1, 1));
        assert_eq!(r.counts_phrase(), "tests 1 passed, 2 failed (1 quarantined, 1 new)");
        let marks = marks_of_job(&c, "j1");
        assert_eq!(marks.get("k2-core::q").map(String::as_str), Some("quarantined"));
        assert_eq!(marks.get("k2-core::n").map(String::as_str), Some("new"));
        assert_eq!(list(&c, "ws", None, Filter::Quarantined, 0, 50).unwrap().len(), 1);
        // Recording the same job again changes nothing.
        let again = record_cases(&c, &ctx("j1", "T", 100), &[case("q", CaseResult::Passed, None)], q).unwrap();
        assert_eq!(again, r);
        let n: i64 = c.query_row("SELECT runs FROM ci_test_stats WHERE test_id = 'k2-core::q'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
    }

    /// Counts only: never names (the message rule), and the phrase skips
    /// a breakdown that is all zero.
    #[test]
    fn counts_phrase_never_names_a_test() {
        assert_eq!(counts_phrase(412, 3, 1, 1, 1), "tests 412 passed, 3 failed (1 flaky, 1 quarantined, 1 new)");
        assert_eq!(counts_phrase(5, 0, 0, 0, 0), "tests 5 passed, 0 failed");
        assert_eq!(counts_phrase(5, 2, 0, 0, 0), "tests 5 passed, 2 failed");
        let c = conn();
        let r = record_cases(&c, &ctx("j1", "T", 100), &[case("@user](http://x)", CaseResult::Failed, None)], NOQ).unwrap();
        let p = r.counts_phrase();
        assert!(!p.contains("@user") && !p.contains("http"), "{p}");
    }

    #[test]
    fn slow_tracks_the_average_and_a_rise() {
        let c = conn();
        record_cases(&c, &ctx("j1", "T", 0), &[case("s", CaseResult::Passed, Some(1000)), case("f", CaseResult::Passed, Some(10))], NOQ).unwrap();
        for i in 1..10 {
            record_cases(&c, &ctx(&format!("j{}", i + 1), "T", i * 86_400), &[case("s", CaseResult::Passed, Some(1000))], NOQ).unwrap();
        }
        // After the baseline window the test gets three times slower.
        for i in 0..12 {
            record_cases(&c, &ctx(&format!("k{i}"), "T", SLOW_BASE_SECS + 86_400 + i), &[case("s", CaseResult::Passed, Some(3000))], NOQ).unwrap();
        }
        let rows = list(&c, "ws", None, Filter::Slow, 0, 50).unwrap();
        assert_eq!(rows[0].test, "k2-core::s");
        assert!(rows[0].rose, "{rows:?}");
        assert!(!rows[1].rose);
    }

    #[test]
    fn counts_only_and_none() {
        let c = conn();
        let t = TestSummary { total: 10, passed: 9, failed: 1, ..Default::default() };
        let r = record_counts_only(&c, &ctx("j1", "T", 1), Some(&t), "JUnit over the 20 MB history cap: counts only").unwrap();
        assert_eq!((r.state.as_str(), r.passed, r.failed), ("counts_only", 9, 1));
        let n = record_counts_only(&c, &ctx("j2", "T", 1), None, "the check names no JUnit artifact").unwrap();
        assert_eq!(n.state, "none");
        assert_eq!(recorded(&c, "j1").unwrap(), r);
        assert_eq!(Filter::parse("slow"), Some(Filter::Slow));
        assert_eq!(Filter::parse("nope"), None);
    }
}
