//! CI monitors (prd-compute-blacksmith-class-v1 §4.5.4, slice O1; migration
//! 0297). How this server wants to hear about a workspace's CI: set by CLI
//! (`k2 ci monitor add`), never by `.k2/ci.toml`. The CI engine evaluates a
//! workspace's monitors once per ended run ([`evaluate`]) and sends what
//! fired.
//!
//! Kinds:
//! - `fail-streak --branch B [--count 3]`: the branch's last N ended runs
//!   all failed (or errored).
//! - `slow --check C [--over 1.5x]`: the p50 of C's last 5 runs is over
//!   that many times its 14-day p50 (with at least 10 runs in 14 days).
//! - `queue-wait [--over 10m]`: a check job of the run waited longer than
//!   that to start.
//! - `new-flaky [--check C]`: a test turned flaky in this run (its first
//!   flaky record is in this run).
//!
//! A message is facts only: counts, check names (from the committed file),
//! the branch, the run id. Never a test name (§4.3.5): names stay behind
//! `k2 ci tests`. A monitor fires at most once per cooldown (default 6h)
//! and never while muted.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::ci::RunRow;
use crate::brand::CLI_NAME;

/// At most this many monitors per workspace.
pub const MAX_PER_WORKSPACE: usize = 20;
/// Default cooldown between two messages of one monitor.
pub const DEFAULT_COOLDOWN_SECS: i64 = 6 * 3600;
pub const MIN_COOLDOWN_SECS: i64 = 300;
pub const MAX_COOLDOWN_SECS: i64 = 7 * 86_400;
/// Longest mute.
pub const MAX_MUTE_SECS: i64 = 30 * 86_400;
/// fail-streak: default and largest `--count`.
pub const DEFAULT_STREAK: i64 = 3;
pub const MAX_STREAK: i64 = 20;
/// slow: default `--over` (1.5x) and its range, in thousandths.
pub const DEFAULT_OVER_MILLI: i64 = 1500;
pub const MIN_OVER_MILLI: i64 = 1100;
pub const MAX_OVER_MILLI: i64 = 10_000;
/// slow: the recent window and the baseline's.
pub const SLOW_RECENT: usize = 5;
pub const SLOW_BASE_MIN: usize = 10;
pub const SLOW_BASE_SECS: i64 = 14 * 86_400;
/// queue-wait: default `--over` (10 min) and its range.
pub const DEFAULT_WAIT_SECS: i64 = 600;
pub const MIN_WAIT_SECS: i64 = 60;
pub const MAX_WAIT_SECS: i64 = 86_400;

/// Refusal codes (`/cli/ci/monitor`).
pub mod codes {
    /// A bad name, kind, value or target (exit 2).
    pub const INVALID: &str = "ci_monitor_invalid";
    /// No monitor of that name in this workspace (exit 4).
    pub const NOT_FOUND: &str = "ci_monitor_not_found";
    /// `add` with a name already used: rm it first (exit 3).
    pub const EXISTS: &str = "ci_monitor_exists";
    /// More than [`super::MAX_PER_WORKSPACE`] (exit 3).
    pub const LIMIT: &str = "ci_monitor_limit";
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Kind {
    #[serde(rename = "fail-streak")]
    FailStreak,
    #[serde(rename = "slow")]
    Slow,
    #[serde(rename = "queue-wait")]
    QueueWait,
    #[serde(rename = "new-flaky")]
    NewFlaky,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Kind> {
        match s {
            "fail-streak" => Some(Kind::FailStreak),
            "slow" => Some(Kind::Slow),
            "queue-wait" => Some(Kind::QueueWait),
            "new-flaky" => Some(Kind::NewFlaky),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::FailStreak => "fail-streak",
            Kind::Slow => "slow",
            Kind::QueueWait => "queue-wait",
            Kind::NewFlaky => "new-flaky",
        }
    }
}

/// Where a monitor's message goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "target", rename_all = "lowercase")]
pub enum To {
    /// A workspace id: its live session, else its inbox.
    Workspace(String),
    /// A Thread address (`k2 thread <addr>`).
    Thread(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Monitor {
    pub workspace_id: String,
    pub name: String,
    pub kind: Kind,
    pub check: Option<String>,
    pub branch: Option<String>,
    pub count: Option<i64>,
    pub over_milli: Option<i64>,
    pub over_secs: Option<i64>,
    pub cooldown_secs: i64,
    pub to: To,
    pub muted_until: Option<i64>,
    pub last_fired_at: Option<i64>,
    pub fired: i64,
    pub created_by: String,
    pub created_at: i64,
}

impl Monitor {
    /// One line: what it watches.
    pub fn describe(&self) -> String {
        match self.kind {
            Kind::FailStreak => {
                let n = self.count.unwrap_or(DEFAULT_STREAK);
                format!("{} failed {n} run{} in a row", self.branch.as_deref().unwrap_or("?"), if n == 1 { "" } else { "s" })
            }
            Kind::Slow => format!("check {} over {}x its 14-day p50", self.check.as_deref().unwrap_or("?"), ratio_words(self.over_milli.unwrap_or(DEFAULT_OVER_MILLI))),
            Kind::QueueWait => format!("a check job waits over {} to start", crate::compute::message::duration(self.over_secs.unwrap_or(DEFAULT_WAIT_SECS))),
            Kind::NewFlaky => match &self.check {
                Some(c) => format!("a test turns flaky in check {c}"),
                None => "a test turns flaky in any check".into(),
            },
        }
    }
}

/// `1.5`, `2`.
pub fn ratio_words(milli: i64) -> String {
    let s = format!("{:.2}", milli as f64 / 1000.0);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// `1.5x` / `1.5` / `150%` → thousandths.
pub fn parse_ratio(s: &str) -> Option<i64> {
    let t = s.trim();
    let (num, pct) = match t.strip_suffix('%') {
        Some(n) => (n, true),
        None => (t.strip_suffix('x').unwrap_or(t), false),
    };
    let v: f64 = num.trim().parse().ok().filter(|v: &f64| v.is_finite())?;
    let milli = if pct { v * 10.0 } else { v * 1000.0 };
    Some(milli.round() as i64)
}

/// A monitor name: 1–40 of `a-z 0-9 . _ -`, starting with a letter or digit.
pub fn valid_name(n: &str) -> bool {
    (1..=40).contains(&n.len())
        && n.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
}

const COLS: &str = "workspace_id, name, kind, check_name, branch, count, over_milli, over_secs, cooldown_secs, to_kind, to_target, \
                    muted_until, last_fired_at, fired, created_by, created_at";

fn monitor_of(r: &rusqlite::Row) -> rusqlite::Result<Monitor> {
    let kind: String = r.get(2)?;
    let to_kind: String = r.get(9)?;
    let target: String = r.get(10)?;
    Ok(Monitor {
        workspace_id: r.get(0)?,
        name: r.get(1)?,
        kind: Kind::parse(&kind).unwrap_or(Kind::FailStreak),
        check: r.get(3)?,
        branch: r.get(4)?,
        count: r.get(5)?,
        over_milli: r.get(6)?,
        over_secs: r.get(7)?,
        cooldown_secs: r.get(8)?,
        to: if to_kind == "thread" { To::Thread(target) } else { To::Workspace(target) },
        muted_until: r.get(11)?,
        last_fired_at: r.get(12)?,
        fired: r.get(13)?,
        created_by: r.get(14)?,
        created_at: r.get(15)?,
    })
}

/// A workspace's monitors, by name.
pub fn list(conn: &Connection, ws: &str) -> Vec<Monitor> {
    let Ok(mut st) = conn.prepare(&format!("SELECT {COLS} FROM ci_monitors WHERE workspace_id = ?1 ORDER BY name")) else { return Vec::new() };
    st.query_map(params![ws], monitor_of).map(|rows| rows.filter_map(Result::ok).collect()).unwrap_or_default()
}

pub fn get(conn: &Connection, ws: &str, name: &str) -> Option<Monitor> {
    conn.query_row(&format!("SELECT {COLS} FROM ci_monitors WHERE workspace_id = ?1 AND name = ?2"), params![ws, name], monitor_of).optional().ok().flatten()
}

/// Store a new monitor (the caller checked its values). `Err((code, why))`.
pub fn add(conn: &Connection, m: &Monitor) -> Result<(), (&'static str, String)> {
    if get(conn, &m.workspace_id, &m.name).is_some() {
        return Err((codes::EXISTS, format!("a monitor '{}' exists: {CLI_NAME} ci monitor rm {} first, or pick another name", m.name, m.name)));
    }
    if list(conn, &m.workspace_id).len() >= MAX_PER_WORKSPACE {
        return Err((codes::LIMIT, format!("at most {MAX_PER_WORKSPACE} monitors per workspace: remove one ({CLI_NAME} ci monitors)")));
    }
    let (to_kind, target) = match &m.to {
        To::Workspace(w) => ("workspace", w.as_str()),
        To::Thread(a) => ("thread", a.as_str()),
    };
    conn.execute(
        "INSERT INTO ci_monitors (workspace_id, name, kind, check_name, branch, count, over_milli, over_secs, cooldown_secs, to_kind, to_target, created_by, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![m.workspace_id, m.name, m.kind.as_str(), m.check, m.branch, m.count, m.over_milli, m.over_secs, m.cooldown_secs, to_kind, target, m.created_by, m.created_at],
    )
    .map_err(|e| ("internal", format!("ci monitor: {e}")))?;
    Ok(())
}

/// `true` when it existed.
pub fn remove(conn: &Connection, ws: &str, name: &str) -> bool {
    conn.execute("DELETE FROM ci_monitors WHERE workspace_id = ?1 AND name = ?2", params![ws, name]).unwrap_or(0) > 0
}

/// Mute until `until` (`None`: unmute). `true` when it exists.
pub fn mute(conn: &Connection, ws: &str, name: &str, until: Option<i64>) -> bool {
    conn.execute("UPDATE ci_monitors SET muted_until = ?3 WHERE workspace_id = ?1 AND name = ?2", params![ws, name, until]).unwrap_or(0) > 0
}

/// One monitor that fired for a run: where to send what.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fired {
    pub name: String,
    pub to: To,
    pub text: String,
}

fn p50(v: &mut [i64]) -> Option<i64> {
    if v.is_empty() {
        return None;
    }
    v.sort_unstable();
    Some(v[(v.len() - 1) / 2])
}

/// The fact behind a monitor for the run that just ended, if it holds.
fn fact(conn: &Connection, m: &Monitor, run: &RunRow, now: i64) -> Option<String> {
    let short = run.short();
    match m.kind {
        Kind::FailStreak => {
            let branch = m.branch.as_deref()?;
            if run.git_ref.as_deref() != Some(branch) {
                return None;
            }
            let n = m.count.unwrap_or(DEFAULT_STREAK).clamp(1, MAX_STREAK);
            let states: Vec<String> = conn
                .prepare("SELECT state FROM ci_runs WHERE workspace_id = ?1 AND ref = ?2 AND kind = 'checks' AND state NOT IN ('running','skipped') ORDER BY created_at DESC, rowid DESC LIMIT ?3")
                .and_then(|mut st| st.query_map(params![run.workspace_id, branch, n], |r| r.get(0)).map(|rows| rows.filter_map(Result::ok).collect()))
                .unwrap_or_default();
            (states.len() as i64 == n && states.iter().all(|s| s == "failed" || s == "error"))
                .then(|| format!("{branch} failed {n} run{} in a row (latest: run {short} @ {})", if n == 1 { "" } else { "s" }, &run.sha[..run.sha.len().min(12)]))
        }
        Kind::Slow => {
            let check = m.check.as_deref()?;
            if !crate::ci::checks_of(conn, &run.id).iter().any(|c| c.name == check) {
                return None;
            }
            let rows: Vec<(i64, i64)> = conn
                .prepare(
                    "SELECT j.ended_at - j.started_at, j.ended_at FROM ci_checks c JOIN ci_runs r ON r.id = c.run_id JOIN compute_jobs j ON j.id = c.job_id \
                     WHERE r.workspace_id = ?1 AND r.kind = 'checks' AND c.name = ?2 AND c.state IN ('success','failure') \
                     AND j.started_at IS NOT NULL AND j.ended_at IS NOT NULL AND j.ended_at >= ?3 ORDER BY j.ended_at DESC LIMIT 1000",
                )
                .and_then(|mut st| st.query_map(params![run.workspace_id, check, now - SLOW_BASE_SECS], |r| Ok((r.get(0)?, r.get(1)?))).map(|rows| rows.filter_map(Result::ok).collect()))
                .unwrap_or_default();
            if rows.len() < SLOW_BASE_MIN || rows.len() < SLOW_RECENT {
                return None;
            }
            let mut recent: Vec<i64> = rows.iter().take(SLOW_RECENT).map(|r| r.0).collect();
            let mut base: Vec<i64> = rows.iter().map(|r| r.0).collect();
            let (r50, b50) = (p50(&mut recent)?, p50(&mut base)?);
            let over = m.over_milli.unwrap_or(DEFAULT_OVER_MILLI);
            (b50 > 0 && r50 * 1000 > b50 * over).then(|| {
                format!(
                    "check {check} is slow: p50 of its last {SLOW_RECENT} runs {} vs {} over 14 days (over {}x; run {short})",
                    crate::compute::message::duration(r50),
                    crate::compute::message::duration(b50),
                    ratio_words(over)
                )
            })
        }
        Kind::QueueWait => {
            let over = m.over_secs.unwrap_or(DEFAULT_WAIT_SECS);
            let worst: Option<(String, i64)> = conn
                .prepare(
                    "SELECT c.name, j.started_at - j.created_at FROM ci_checks c JOIN compute_jobs j ON j.id = c.job_id \
                     WHERE c.run_id = ?1 AND j.started_at IS NOT NULL ORDER BY 2 DESC LIMIT 1",
                )
                .and_then(|mut st| st.query_row(params![run.id], |r| Ok((r.get(0)?, r.get(1)?))).optional())
                .ok()
                .flatten();
            worst.filter(|(_, w)| *w > over).map(|(name, w)| {
                format!("check {name} waited {} to start (over {}; run {short})", crate::compute::message::duration(w), crate::compute::message::duration(over))
            })
        }
        Kind::NewFlaky => {
            // Tests whose first flaky record (any job of the check) is in this run.
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(DISTINCT f.check_name || char(31) || f.test_id) FROM ci_test_failures f \
                     WHERE f.run_id = ?1 AND f.workspace_id = ?2 AND (f.kind = 'flaky' OR f.mark = 'flaky') AND (?3 IS NULL OR f.check_name = ?3) \
                     AND NOT EXISTS (SELECT 1 FROM ci_test_failures g WHERE g.workspace_id = f.workspace_id AND g.check_name = f.check_name \
                       AND g.test_id = f.test_id AND (g.kind = 'flaky' OR g.mark = 'flaky') AND (g.run_id IS NOT ?1) AND g.at <= f.at)",
                    params![run.id, run.workspace_id, m.check],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            (n > 0).then(|| {
                format!(
                    "{n} test{} turned flaky{} (run {short}; names: {CLI_NAME} ci tests --flaky)",
                    if n == 1 { "" } else { "s" },
                    m.check.as_deref().map(|c| format!(" in check {c}")).unwrap_or_default()
                )
            })
        }
    }
}

/// The monitors of `run`'s workspace that fire now (not muted, past their
/// cooldown, not already fired for this run), with their messages. Each
/// one that fires is stamped (`last_fired_at`, `last_key`, `fired`).
pub fn evaluate(conn: &Connection, run: &RunRow, now: i64) -> Vec<Fired> {
    let mut out = Vec::new();
    for m in list(conn, &run.workspace_id) {
        if m.muted_until.is_some_and(|u| u > now) {
            continue;
        }
        if m.last_fired_at.is_some_and(|t| now - t < m.cooldown_secs) {
            continue;
        }
        let last_key: Option<String> = conn
            .query_row("SELECT last_key FROM ci_monitors WHERE workspace_id = ?1 AND name = ?2", params![m.workspace_id, m.name], |r| r.get(0))
            .ok()
            .flatten();
        if last_key.as_deref() == Some(run.id.as_str()) {
            continue;
        }
        let Some(f) = fact(conn, &m, run, now) else { continue };
        let _ = conn.execute(
            "UPDATE ci_monitors SET last_fired_at = ?3, last_key = ?4, fired = fired + 1 WHERE workspace_id = ?1 AND name = ?2",
            params![m.workspace_id, m.name, now, run.id],
        );
        out.push(Fired { name: m.name.clone(), to: m.to.clone(), text: format!("[ci] monitor {}: {f} · {CLI_NAME} ci status {}", m.name, run.short()) });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(ws: &str, name: &str, kind: Kind) -> Monitor {
        Monitor {
            workspace_id: ws.into(),
            name: name.into(),
            kind,
            check: None,
            branch: None,
            count: None,
            over_milli: None,
            over_secs: None,
            cooldown_secs: DEFAULT_COOLDOWN_SECS,
            to: To::Workspace(ws.into()),
            muted_until: None,
            last_fired_at: None,
            fired: 0,
            created_by: "owner".into(),
            created_at: 1,
        }
    }

    fn run(conn: &Connection, id: &str, ws: &str, git_ref: &str, state: &str, at: i64) -> RunRow {
        let id = format!("{id:0<32}");
        let sha = "a".repeat(40);
        let r = crate::ci::insert_run(
            conn,
            &crate::ci::NewRun { id: &id, workspace_id: ws, sha: &sha, git_ref: Some(git_ref), trigger: "watch", state, detail: None, created_by: "watch", tier: "ci", statuses: None, at },
        )
        .unwrap();
        crate::ci::set_run_state(conn, &r.id, state, None, Some(at + 10));
        crate::ci::run_by_id(conn, &r.id).unwrap()
    }

    fn job(conn: &Connection, run: &RunRow, check: &str, id: &str, created: i64, started: i64, ended: i64) {
        conn.execute(
            "INSERT INTO compute_jobs (id, client_job_id, node_id, workspace_id, requested_by, argv_json, env_names_json, request_sha256, plan_json, \
             state, plan_digest, created_at, started_at, ended_at) VALUES (?1, ?1, 'n1', ?2, 'owner', '[]', '[]', '', '{}', 'done', '', ?3, ?4, ?5)",
            params![id, run.workspace_id, created, started, ended],
        )
        .unwrap();
        crate::ci::insert_check(conn, &run.id, check, "pending", "").unwrap();
        crate::ci::set_check(conn, &run.id, check, Some(id), "success", "ok");
    }

    #[test]
    fn names_ratios_and_crud() {
        assert!(valid_name("main-red") && !valid_name("Main") && !valid_name("") && !valid_name(&"a".repeat(41)));
        assert_eq!((parse_ratio("1.5x"), parse_ratio("2"), parse_ratio("150%"), parse_ratio("x")), (Some(1500), Some(2000), Some(1500), None));
        assert_eq!(ratio_words(1500), "1.5");
        assert_eq!(ratio_words(2000), "2");
        let _db = crate::db::scoped_for_test();
        let db = crate::db::shared();
        let conn = db.lock();
        let m = Monitor { branch: Some("main".into()), ..monitor("ws", "red", Kind::FailStreak) };
        add(&conn, &m).unwrap();
        assert_eq!(add(&conn, &m).unwrap_err().0, codes::EXISTS);
        let t = Monitor { to: To::Thread("ciapp".into()), ..monitor("ws", "flaky", Kind::NewFlaky) };
        add(&conn, &t).unwrap();
        let l = list(&conn, "ws");
        assert_eq!(l.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), vec!["flaky", "red"]);
        assert_eq!(l[0].to, To::Thread("ciapp".into()));
        assert_eq!(l[1].describe(), "main failed 3 runs in a row");
        assert!(mute(&conn, "ws", "red", Some(99)));
        assert_eq!(get(&conn, "ws", "red").unwrap().muted_until, Some(99));
        assert!(!mute(&conn, "ws", "nope", Some(1)));
        assert!(remove(&conn, "ws", "red") && !remove(&conn, "ws", "red"));
        assert!(list(&conn, "other").is_empty(), "per workspace");
        for i in 0..(MAX_PER_WORKSPACE - 1) {
            add(&conn, &monitor("ws", &format!("m{i}"), Kind::QueueWait)).unwrap();
        }
        assert_eq!(add(&conn, &monitor("ws", "one-more", Kind::QueueWait)).unwrap_err().0, codes::LIMIT);
    }

    /// A branch's third failure in a row fires once; the cooldown holds
    /// the next; a pass breaks the streak; muted never fires.
    #[test]
    fn fail_streak_fires_once_per_cooldown() {
        let _db = crate::db::scoped_for_test();
        let db = crate::db::shared();
        let conn = db.lock();
        add(&conn, &Monitor { branch: Some("main".into()), ..monitor("ws", "red", Kind::FailStreak) }).unwrap();
        let r1 = run(&conn, "r1", "ws", "main", "failed", 100);
        assert!(evaluate(&conn, &r1, 200).is_empty());
        let _r2 = run(&conn, "r2", "ws", "main", "error", 200);
        let other = run(&conn, "rx", "ws", "integ/x", "failed", 250);
        assert!(evaluate(&conn, &other, 260).is_empty(), "another branch");
        let r3 = run(&conn, "r3", "ws", "main", "failed", 300);
        let f = evaluate(&conn, &r3, 400);
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(f[0].to, To::Workspace("ws".into()));
        assert!(f[0].text.starts_with("[ci] monitor red: main failed 3 runs in a row (latest: run r3000000 @ aaaaaaaaaaaa)"), "{}", f[0].text);
        assert!(f[0].text.ends_with("k2 ci status r3000000"));
        assert!(evaluate(&conn, &r3, 401).is_empty(), "the same run fires once");
        let r4 = run(&conn, "r4", "ws", "main", "failed", 500);
        assert!(evaluate(&conn, &r4, 600).is_empty(), "within the cooldown");
        assert!(!evaluate(&conn, &r4, 400 + DEFAULT_COOLDOWN_SECS).is_empty(), "after it");
        let r5 = run(&conn, "r5", "ws", "main", "passed", 700);
        assert!(evaluate(&conn, &r5, 400 + 3 * DEFAULT_COOLDOWN_SECS).is_empty(), "a pass breaks the streak");
        mute(&conn, "ws", "red", Some(i64::MAX));
        let r6 = run(&conn, "r6", "ws", "main", "failed", 800);
        let _ = run(&conn, "r7", "ws", "main", "failed", 900);
        let r8 = run(&conn, "r8", "ws", "main", "failed", 1000);
        let _ = r6;
        assert!(evaluate(&conn, &r8, 400 + 5 * DEFAULT_COOLDOWN_SECS).is_empty(), "muted");
    }

    #[test]
    fn slow_and_queue_wait() {
        let _db = crate::db::scoped_for_test();
        let db = crate::db::shared();
        let conn = db.lock();
        let now = 2_000_000;
        add(&conn, &Monitor { check: Some("unit".into()), ..monitor("ws", "slow", Kind::Slow) }).unwrap();
        add(&conn, &Monitor { over_secs: Some(600), ..monitor("ws", "wait", Kind::QueueWait) }).unwrap();
        // 10 runs at 100 s, then 5 at 200 s: recent p50 200 > 1.5 × 100.
        let mut last = None;
        for i in 0..15 {
            let at = now - 100_000 + i * 1000;
            let r = run(&conn, &format!("s{i:02}"), "ws", "main", "passed", at);
            let secs = if i < 10 { 100 } else { 200 };
            job(&conn, &r, "unit", &format!("j{i:02}"), at, at + 5, at + 5 + secs);
            last = Some(r);
        }
        let last = last.unwrap();
        let f = evaluate(&conn, &last, now);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].text.contains("check unit is slow: p50 of its last 5 runs 3m20s vs 1m40s over 14 days (over 1.5x"), "{}", f[0].text);
        // A job that waited 11 minutes to start.
        let r = run(&conn, "w1", "ws", "main", "passed", now);
        job(&conn, &r, "lint", "jw", now, now + 660, now + 700);
        let f = evaluate(&conn, &r, now + 800);
        let w: Vec<_> = f.iter().filter(|x| x.name == "wait").collect();
        assert_eq!(w.len(), 1, "{f:?}");
        assert!(w[0].text.contains("check lint waited 11m00s to start (over 10m00s"), "{}", w[0].text);
    }

    #[test]
    fn new_flaky_counts_first_flakes_only_and_never_names_them() {
        let _db = crate::db::scoped_for_test();
        let db = crate::db::shared();
        let conn = db.lock();
        add(&conn, &monitor("ws", "flaky", Kind::NewFlaky)).unwrap();
        let ins = |run: &str, test: &str, at: i64| {
            conn.execute(
                "INSERT INTO ci_test_failures (workspace_id, check_name, test_id, name, kind, mark, job_id, run_id, at) VALUES ('ws', 'unit', ?1, ?1, 'failure', 'flaky', 'j', ?2, ?3)",
                params![test, run, at],
            )
            .unwrap();
        };
        let r1 = run(&conn, "f1", "ws", "main", "failed", 100);
        ins(&r1.id, "old::flake", 100);
        let r2 = run(&conn, "f2", "ws", "main", "failed", 200);
        ins(&r2.id, "old::flake", 200);
        ins(&r2.id, "sync::reconnect @user", 200);
        let f = evaluate(&conn, &r2, 300);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].text.contains("1 test turned flaky (run f2000000; names: k2 ci tests --flaky)"), "{}", f[0].text);
        assert!(!f[0].text.contains("reconnect") && !f[0].text.contains("@user"), "no names: {}", f[0].text);
    }
}
