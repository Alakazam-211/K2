//! `k2 ci` (0.46.10, prd-ci-on-compute-for-clients-v1 §4.7, vs-live V32
//! slice C6a): the store and the pure rules. The daemon owns CI
//! (CLAUDE.md daemon-first): the watcher is a daemon loop and a run is
//! daemon state, never a `k2 job` (`k2_daemon::ci_engine` drives it,
//! `k2_daemon::ci_routes` serves it).
//!
//! - **Config:** `.k2/ci.toml` ([`CiConfig`], frozen in `k2-node-proto`),
//!   read **at the commit** a run checks (`git cat-file blob
//!   <sha>:.k2/ci.toml`). A commit without one is `skipped`.
//! - **Watch** (`ci_workspaces`): the remote and branch patterns the daemon
//!   polls (`git ls-remote`), how often, the tier (`ci`, or `normal` for a
//!   workspace whose grants lack `--ci`), and how many runs may be in
//!   flight. `ci_seen` keeps each tip to one run.
//! - **Runs** (`ci_runs`, `ci_checks`): one compute job per
//!   `[checks.<name>]`, through the normal job path with today's
//!   placement, each `--tier ci --supersede ci/<branch>/<check>` on a
//!   watched branch. A run is `running` until every job ended, then
//!   `passed`, `failed` (a check failed) or `error` (infra: not submitted,
//!   superseded, cancelled, lost, the deadline).
//! - **Statuses:** `pending` when a check is submitted, then `success` /
//!   `failure` / `error`, as `<context>/<check>` on GitHub, with the token
//!   named in `[statuses]` read from the workspace's vault (the daemon
//!   makes the call; the token never reaches a job). Each check remembers
//!   the last state posted, so a failed post is retried on the next tick
//!   (at most [`MAX_STATUS_TRIES`] times per state).
//! - **Freeze** (`k2 ci freeze --until`): a flag 0.46.11's rebalancer reads
//!   (no seat moves during a client's release freeze). Nothing in 0.46.10
//!   acts on it.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::compute::proto::ci_config::{CheckJob, CiConfig, StatusesConfig, WatchConfig, CI_CONFIG_PATH, CI_CONFIG_VERSION};

/// Runs in flight per workspace unless the watch says otherwise.
pub const DEFAULT_MAX_INFLIGHT: u32 = 2;
/// The most a watch may allow.
pub const MAX_INFLIGHT_CAP: u32 = 8;
/// A run whose jobs are still not done this long after it started ends
/// `error` ("still … at the deadline"); compute's own queue timeouts
/// normally end every job well before.
pub const RUN_DEADLINE_SECS: i64 = 8 * 3600;
/// Posting one state for one check is tried at most this many times.
pub const MAX_STATUS_TRIES: i64 = 5;
/// GitHub keeps 140 characters of a status description.
pub const STATUS_DESC_MAX: usize = 140;
/// At most this many checks per run.
pub const MAX_CHECKS: usize = 32;
/// The default branch pattern when nothing names one.
pub const DEFAULT_BRANCH: &str = "main";
/// The default remote.
pub const DEFAULT_REMOTE: &str = "origin";

/// Run states.
pub mod run_state {
    pub const RUNNING: &str = "running";
    pub const PASSED: &str = "passed";
    pub const FAILED: &str = "failed";
    pub const ERROR: &str = "error";
    /// No `.k2/ci.toml` (or no checks) at the commit: nothing ran.
    pub const SKIPPED: &str = "skipped";
}

/// Check states. `pending`/`running` post as GitHub's `pending`.
pub mod check_state {
    pub const PENDING: &str = "pending";
    pub const RUNNING: &str = "running";
    pub const SUCCESS: &str = "success";
    pub const FAILURE: &str = "failure";
    pub const ERROR: &str = "error";
}

/// Refusal codes (`/cli/ci/*`).
pub mod codes {
    /// No `.k2/ci.toml` in the workspace (or at the commit asked for).
    pub const NO_CONFIG: &str = "ci_no_config";
    /// `.k2/ci.toml` doesn't parse.
    pub const BAD_CONFIG: &str = "ci_config_invalid";
    /// `k2 ci init` would overwrite an existing file.
    pub const CONFIG_EXISTS: &str = "ci_config_exists";
    /// The workspace folder isn't a git repo.
    pub const NOT_A_REPO: &str = "ci_not_a_repo";
    /// The commit isn't in the repo and couldn't be fetched.
    pub const NO_COMMIT: &str = "ci_commit_not_found";
    /// No run matches.
    pub const NO_RUN: &str = "ci_run_not_found";
    /// `--check` names a check the config doesn't have.
    pub const NO_CHECK: &str = "ci_check_not_found";
    /// `k2 ci watch on`: the remote didn't answer `git ls-remote`.
    pub const REMOTE_UNREACHABLE: &str = "ci_remote_unreachable";
    /// A watch value out of range.
    pub const BAD_WATCH: &str = "ci_watch_invalid";
}

// ── the workspace row (watch + freeze) ───────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceRow {
    pub workspace_id: String,
    pub watch_on: bool,
    pub remote: String,
    pub branches: Vec<String>,
    pub poll_secs: u32,
    /// `ci` or `normal`.
    pub tier: String,
    pub max_inflight: u32,
    pub watch_by: String,
    pub watch_at: i64,
    pub last_poll_at: Option<i64>,
    pub last_error: Option<String>,
    pub freeze_until: Option<i64>,
    pub freeze_why: Option<String>,
    pub freeze_by: Option<String>,
    pub freeze_at: Option<i64>,
}

const WS_COLS: &str = "workspace_id, watch_on, remote, branches_json, poll_secs, tier, max_inflight, watch_by, watch_at, \
    last_poll_at, last_error, freeze_until, freeze_why, freeze_by, freeze_at";

fn ws_of(r: &rusqlite::Row) -> rusqlite::Result<WorkspaceRow> {
    let branches: String = r.get(3)?;
    Ok(WorkspaceRow {
        workspace_id: r.get(0)?,
        watch_on: r.get::<_, i64>(1)? != 0,
        remote: r.get(2)?,
        branches: serde_json::from_str(&branches).unwrap_or_default(),
        poll_secs: r.get::<_, i64>(4)?.clamp(0, u32::MAX as i64) as u32,
        tier: r.get(5)?,
        max_inflight: r.get::<_, i64>(6)?.clamp(0, u32::MAX as i64) as u32,
        watch_by: r.get(7)?,
        watch_at: r.get(8)?,
        last_poll_at: r.get(9)?,
        last_error: r.get(10)?,
        freeze_until: r.get(11)?,
        freeze_why: r.get(12)?,
        freeze_by: r.get(13)?,
        freeze_at: r.get(14)?,
    })
}

pub fn workspace(conn: &Connection, ws: &str) -> Option<WorkspaceRow> {
    conn.query_row(&format!("SELECT {WS_COLS} FROM ci_workspaces WHERE workspace_id = ?1"), params![ws], ws_of).optional().ok().flatten()
}

/// Every workspace whose watch is on.
pub fn watched(conn: &Connection) -> Vec<WorkspaceRow> {
    let Ok(mut st) = conn.prepare(&format!("SELECT {WS_COLS} FROM ci_workspaces WHERE watch_on = 1 ORDER BY workspace_id")) else { return Vec::new() };
    st.query_map([], ws_of).map(|rows| rows.filter_map(Result::ok).collect()).unwrap_or_default()
}

/// What `k2 ci watch on` sets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Watch {
    pub remote: String,
    pub branches: Vec<String>,
    pub poll_secs: u32,
    pub tier: String,
    pub max_inflight: u32,
}

impl Watch {
    /// The checks a watch must pass: the same rules as `[watch]` in
    /// `.k2/ci.toml` ([`CiConfig::check`]), plus the tier and the cap.
    pub fn check(&self) -> Result<(), String> {
        let as_cfg = CiConfig {
            version: CI_CONFIG_VERSION,
            watch: Some(WatchConfig { remote: self.remote.clone(), branches: self.branches.clone(), poll_secs: self.poll_secs }),
            statuses: None,
            checks: Default::default(),
            release: None,
        };
        as_cfg.check()?;
        if !valid_remote(&self.remote) {
            return Err(format!("watch.remote: {:?} is not a git remote name", self.remote));
        }
        if self.tier != "ci" && self.tier != "normal" {
            return Err("watch: --tier is ci or standard".into());
        }
        if !(1..=MAX_INFLIGHT_CAP).contains(&self.max_inflight) {
            return Err(format!("watch: --max-inflight is 1-{MAX_INFLIGHT_CAP}"));
        }
        Ok(())
    }
}

/// A git remote name K2 passes to `git ls-remote` / `git fetch`: never an
/// option, a URL or a path (the watch names a configured remote).
pub fn valid_remote(r: &str) -> bool {
    !r.is_empty()
        && r.len() <= 64
        && !r.starts_with('-')
        && !r.starts_with('.')
        && r.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

pub fn set_watch(conn: &Connection, ws: &str, w: &Watch, by: &str, at: i64) -> Result<(), String> {
    w.check()?;
    let branches = serde_json::to_string(&w.branches).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO ci_workspaces (workspace_id, watch_on, remote, branches_json, poll_secs, tier, max_inflight, watch_by, watch_at) \
         VALUES (?1, 1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
         ON CONFLICT(workspace_id) DO UPDATE SET watch_on = 1, remote = ?2, branches_json = ?3, poll_secs = ?4, tier = ?5, \
         max_inflight = ?6, watch_by = ?7, watch_at = ?8, last_poll_at = NULL, last_error = NULL",
        params![ws, w.remote, branches, w.poll_secs, w.tier, w.max_inflight, by, at],
    )
    .map_err(|e| format!("ci watch: {e}"))?;
    Ok(())
}

/// Turn the watch off (the row and its settings stay). `false` when it
/// was not on.
pub fn watch_off(conn: &Connection, ws: &str, by: &str, at: i64) -> Result<bool, String> {
    let n = conn
        .execute("UPDATE ci_workspaces SET watch_on = 0, watch_by = ?2, watch_at = ?3 WHERE workspace_id = ?1 AND watch_on = 1", params![ws, by, at])
        .map_err(|e| format!("ci watch: {e}"))?;
    Ok(n > 0)
}

pub fn set_polled(conn: &Connection, ws: &str, at: i64, error: Option<&str>) {
    let _ = conn.execute("UPDATE ci_workspaces SET last_poll_at = ?2, last_error = ?3 WHERE workspace_id = ?1", params![ws, at, error]);
}

/// Is this watch due a poll at `now`?
pub fn poll_due(w: &WorkspaceRow, now: i64) -> bool {
    w.watch_on && w.last_poll_at.is_none_or(|t| now >= t + i64::from(w.poll_secs.max(30)))
}

/// `k2 ci freeze --until <t>` (`None` clears it).
pub fn set_freeze(conn: &Connection, ws: &str, until: Option<i64>, why: Option<&str>, by: &str, at: i64) -> Result<(), String> {
    conn.execute(
        "INSERT INTO ci_workspaces (workspace_id, freeze_until, freeze_why, freeze_by, freeze_at) VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(workspace_id) DO UPDATE SET freeze_until = ?2, freeze_why = ?3, freeze_by = ?4, freeze_at = ?5",
        params![ws, until, why, by, at],
    )
    .map_err(|e| format!("ci freeze: {e}"))?;
    Ok(())
}

/// The freeze in force at `now`, if any (`until`, `why`).
pub fn frozen(w: &WorkspaceRow, now: i64) -> Option<(i64, Option<String>)> {
    w.freeze_until.filter(|u| *u > now).map(|u| (u, w.freeze_why.clone()))
}

// ── seen tips ────────────────────────────────────────────────────────

pub fn is_seen(conn: &Connection, ws: &str, sha: &str) -> bool {
    conn.query_row("SELECT 1 FROM ci_seen WHERE workspace_id = ?1 AND sha = ?2", params![ws, sha], |_| Ok(())).optional().ok().flatten().is_some()
}

pub fn mark_seen(conn: &Connection, ws: &str, sha: &str, git_ref: &str, at: i64) {
    let _ = conn.execute("INSERT OR IGNORE INTO ci_seen (workspace_id, sha, ref, seen_at) VALUES (?1, ?2, ?3, ?4)", params![ws, sha, git_ref, at]);
}

// ── runs and checks ──────────────────────────────────────────────────

/// Where a run's commit statuses go. Names only, never a token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusTarget {
    /// `owner/repo` on GitHub.
    pub slug: String,
    /// `k2-ci`: statuses are `<context>/<check>`.
    pub context: String,
    /// The vault name (`K2_CI_STATUSES_TOKEN`) in the workspace's vault.
    pub token_secret: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRow {
    pub id: String,
    pub workspace_id: String,
    pub sha: String,
    #[serde(rename = "ref")]
    pub git_ref: Option<String>,
    pub trigger: String,
    pub state: String,
    pub detail: Option<String>,
    pub created_by: String,
    pub tier: String,
    pub statuses: Option<StatusTarget>,
    pub created_at: i64,
    pub ended_at: Option<i64>,
    /// `checks` (a `[checks.*]` run) or `release` (`k2 ci release`,
    /// `crate::ci_release`; migration 0287).
    pub kind: String,
}

impl RunRow {
    pub fn is_running(&self) -> bool {
        self.state == run_state::RUNNING
    }
    /// The run id as people type it (8 hex).
    pub fn short(&self) -> &str {
        &self.id[..self.id.len().min(8)]
    }
}

const RUN_COLS: &str = "id, workspace_id, sha, ref, trigger, state, detail, created_by, tier, statuses_json, created_at, ended_at, kind";

fn run_of(r: &rusqlite::Row) -> rusqlite::Result<RunRow> {
    let statuses: Option<String> = r.get(9)?;
    Ok(RunRow {
        id: r.get(0)?,
        workspace_id: r.get(1)?,
        sha: r.get(2)?,
        git_ref: r.get(3)?,
        trigger: r.get(4)?,
        state: r.get(5)?,
        detail: r.get(6)?,
        created_by: r.get(7)?,
        tier: r.get(8)?,
        statuses: statuses.and_then(|s| serde_json::from_str(&s).ok()),
        created_at: r.get(10)?,
        ended_at: r.get(11)?,
        kind: r.get(12)?,
    })
}

/// A new run's facts.
#[derive(Debug, Clone)]
pub struct NewRun<'a> {
    pub id: &'a str,
    pub workspace_id: &'a str,
    pub sha: &'a str,
    pub git_ref: Option<&'a str>,
    /// `watch` or `manual`.
    pub trigger: &'a str,
    pub state: &'a str,
    pub detail: Option<&'a str>,
    pub created_by: &'a str,
    pub tier: &'a str,
    pub statuses: Option<&'a StatusTarget>,
    pub at: i64,
}

pub fn insert_run(conn: &Connection, n: &NewRun<'_>) -> Result<RunRow, String> {
    let statuses = n.statuses.map(serde_json::to_string).transpose().map_err(|e| e.to_string())?;
    let ended = (n.state != run_state::RUNNING).then_some(n.at);
    conn.execute(
        "INSERT INTO ci_runs (id, workspace_id, sha, ref, trigger, state, detail, created_by, tier, statuses_json, created_at, ended_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![n.id, n.workspace_id, n.sha, n.git_ref, n.trigger, n.state, n.detail, n.created_by, n.tier, statuses, n.at, ended],
    )
    .map_err(|e| format!("ci run: {e}"))?;
    run_by_id(conn, n.id).ok_or_else(|| "ci run: not stored".to_string())
}

pub fn run_by_id(conn: &Connection, id: &str) -> Option<RunRow> {
    conn.query_row(&format!("SELECT {RUN_COLS} FROM ci_runs WHERE id = ?1"), params![id], run_of).optional().ok().flatten()
}

/// A workspace's runs, newest first.
pub fn runs_of(conn: &Connection, ws: &str, limit: usize) -> Vec<RunRow> {
    let Ok(mut st) = conn.prepare(&format!("SELECT {RUN_COLS} FROM ci_runs WHERE workspace_id = ?1 ORDER BY created_at DESC, rowid DESC LIMIT ?2")) else {
        return Vec::new();
    };
    st.query_map(params![ws, limit as i64], run_of).map(|rows| rows.filter_map(Result::ok).collect()).unwrap_or_default()
}

/// Every run still in flight (any workspace), oldest first.
pub fn running_runs(conn: &Connection) -> Vec<RunRow> {
    let Ok(mut st) = conn.prepare(&format!("SELECT {RUN_COLS} FROM ci_runs WHERE state = 'running' ORDER BY created_at, rowid")) else { return Vec::new() };
    st.query_map([], run_of).map(|rows| rows.filter_map(Result::ok).collect()).unwrap_or_default()
}

pub fn inflight_count(conn: &Connection, ws: &str) -> u32 {
    // Release runs (`k2 ci release`) never hold a watched tip back.
    conn.query_row("SELECT COUNT(*) FROM ci_runs WHERE workspace_id = ?1 AND state = 'running' AND kind = 'checks'", params![ws], |r| r.get::<_, i64>(0))
        .map(|n| n.clamp(0, u32::MAX as i64) as u32)
        .unwrap_or(0)
}

/// A run as people name it, newest match first: its id (6+ hex of it), a
/// commit (7+ hex of it), or a branch.
pub fn find_run(conn: &Connection, ws: &str, token: &str) -> Option<RunRow> {
    let t = token.trim();
    if t.is_empty() {
        return None;
    }
    let runs = runs_of(conn, ws, 500);
    let hex = t.len() >= 6 && t.bytes().all(|b| b.is_ascii_hexdigit());
    if hex {
        let lower = t.to_ascii_lowercase();
        if let Some(r) = runs.iter().find(|r| r.id.starts_with(&lower)) {
            return Some(r.clone());
        }
        if lower.len() >= 7 {
            if let Some(r) = runs.iter().find(|r| r.sha.starts_with(&lower)) {
                return Some(r.clone());
            }
        }
    }
    runs.into_iter().find(|r| r.git_ref.as_deref() == Some(t))
}

pub fn set_run_state(conn: &Connection, id: &str, state: &str, detail: Option<&str>, ended_at: Option<i64>) {
    let _ = conn.execute("UPDATE ci_runs SET state = ?2, detail = ?3, ended_at = ?4 WHERE id = ?1", params![id, state, detail, ended_at]);
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckRow {
    pub run_id: String,
    pub name: String,
    pub job_id: Option<String>,
    pub state: String,
    pub description: String,
    /// The GitHub state last posted for it (`pending`, `success`, …).
    pub posted: Option<String>,
    pub post_tries: i64,
    pub post_error: Option<String>,
}

impl CheckRow {
    pub fn is_final(&self) -> bool {
        is_final_check(&self.state)
    }
}

pub fn is_final_check(state: &str) -> bool {
    matches!(state, check_state::SUCCESS | check_state::FAILURE | check_state::ERROR)
}

const CHECK_COLS: &str = "run_id, name, job_id, state, description, posted, post_tries, post_error";

fn check_of(r: &rusqlite::Row) -> rusqlite::Result<CheckRow> {
    Ok(CheckRow {
        run_id: r.get(0)?,
        name: r.get(1)?,
        job_id: r.get(2)?,
        state: r.get(3)?,
        description: r.get(4)?,
        posted: r.get(5)?,
        post_tries: r.get(6)?,
        post_error: r.get(7)?,
    })
}

pub fn insert_check(conn: &Connection, run: &str, name: &str, state: &str, description: &str) -> Result<(), String> {
    conn.execute(
        "INSERT INTO ci_checks (run_id, name, state, description) VALUES (?1, ?2, ?3, ?4)",
        params![run, name, state, trim_desc(description)],
    )
    .map_err(|e| format!("ci check: {e}"))?;
    Ok(())
}

pub fn checks_of(conn: &Connection, run: &str) -> Vec<CheckRow> {
    let Ok(mut st) = conn.prepare(&format!("SELECT {CHECK_COLS} FROM ci_checks WHERE run_id = ?1 ORDER BY name")) else { return Vec::new() };
    st.query_map(params![run], check_of).map(|rows| rows.filter_map(Result::ok).collect()).unwrap_or_default()
}

/// A check's job, state and description.
pub fn set_check(conn: &Connection, run: &str, name: &str, job: Option<&str>, state: &str, description: &str) {
    let _ = conn.execute(
        "UPDATE ci_checks SET job_id = COALESCE(?3, job_id), state = ?4, description = ?5 WHERE run_id = ?1 AND name = ?2",
        params![run, name, job, state, trim_desc(description)],
    );
}

/// After a post: `posted` is the state GitHub now has (a success), else
/// unchanged; `tries` counts attempts at the state being posted.
pub fn set_check_posted(conn: &Connection, run: &str, name: &str, posted: Option<&str>, tries: i64, error: Option<&str>) {
    let _ = conn.execute(
        "UPDATE ci_checks SET posted = COALESCE(?3, posted), post_tries = ?4, post_error = ?5 WHERE run_id = ?1 AND name = ?2",
        params![run, name, posted, tries, error],
    );
}

// ── the rules ────────────────────────────────────────────────────────

/// Does `branch` match one of the watch's patterns (a name, or
/// `prefix/*`: anything under `prefix/`)?
pub fn branch_matches(patterns: &[String], branch: &str) -> bool {
    patterns.iter().any(|p| match p.strip_suffix("/*") {
        Some(prefix) => branch.len() > prefix.len() + 1 && branch.starts_with(prefix) && branch.as_bytes()[prefix.len()] == b'/',
        None => p == branch,
    })
}

/// The watched tips in `git ls-remote <remote> refs/heads/*` output, as
/// `(sha, branch)`: branches named exactly first (in the watch's order),
/// then the `prefix/*` matches by name. Malformed lines are skipped.
pub fn tips_from_ls_remote(out: &str, patterns: &[String]) -> Vec<(String, String)> {
    let mut exact: Vec<(usize, String, String)> = Vec::new();
    let mut globbed: Vec<(String, String)> = Vec::new();
    for line in out.lines() {
        let mut parts = line.split('\t');
        let (Some(sha), Some(r), None) = (parts.next(), parts.next(), parts.next()) else { continue };
        if !is_sha(sha) {
            continue;
        }
        let Some(branch) = r.strip_prefix("refs/heads/") else { continue };
        if let Some(i) = patterns.iter().position(|p| p == branch) {
            exact.push((i, sha.to_string(), branch.to_string()));
        } else if branch_matches(patterns, branch) {
            globbed.push((sha.to_string(), branch.to_string()));
        }
    }
    exact.sort();
    globbed.sort_by(|a, b| a.1.cmp(&b.1));
    exact.into_iter().map(|(_, s, b)| (s, b)).chain(globbed).collect()
}

/// 40 lowercase hex (a full sha-1 commit id).
pub fn is_sha(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A description GitHub keeps whole (140 characters, one line).
pub fn trim_desc(s: &str) -> String {
    let one: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    if one.chars().count() <= STATUS_DESC_MAX {
        return one;
    }
    let mut out: String = one.chars().take(STATUS_DESC_MAX - 1).collect();
    out.push('…');
    out
}

/// The GitHub state for a check state.
pub fn github_state(check: &str) -> &'static str {
    match check {
        check_state::SUCCESS => "success",
        check_state::FAILURE => "failure",
        check_state::ERROR => "error",
        _ => "pending",
    }
}

/// What a check's compute job says, as the reaper reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobFacts {
    pub state: String,
    pub exit: Option<i64>,
    pub reason: Option<String>,
    pub node: String,
    /// The job id as people type it.
    pub short: String,
    pub secs: i64,
}

/// A check's state and description from its job (`late`: the run is past
/// its deadline). The port of `scripts/ci/run-checks.sh`'s reap.
pub fn verdict(j: &JobFacts, late: bool) -> (&'static str, String) {
    let mins = (j.secs.max(0) + 59) / 60;
    let mins = mins.max(1);
    match j.state.as_str() {
        "done" if j.exit == Some(0) => (check_state::SUCCESS, format!("job {} on {}, {mins}m", j.short, j.node)),
        "done" => (
            check_state::FAILURE,
            format!("job {} on {}: exit {} after {mins}m (k2 compute logs {} --failures)", j.short, j.node, j.exit.map(|e| e.to_string()).unwrap_or_else(|| "?".into()), j.short),
        ),
        "queued" | "assigned" | "preparing" | "running" | "finishing" => {
            if late {
                (check_state::ERROR, format!("job {} on {} still {} at the deadline", j.short, j.node, j.state))
            } else {
                (check_state::RUNNING, format!("job {} on {} {}", j.short, j.node, j.state))
            }
        }
        _ if j.reason.as_deref().is_some_and(|r| r.contains("replaced") || r.contains("supersed")) => {
            (check_state::ERROR, format!("job {} superseded by a newer push", j.short))
        }
        s => (check_state::ERROR, format!("job {} on {}: {s} ({})", j.short, j.node, j.reason.as_deref().unwrap_or("no reason"))),
    }
}

/// A finished run's state from its checks' states.
pub fn run_verdict<'a>(states: impl IntoIterator<Item = &'a str>) -> &'static str {
    let mut any_failure = false;
    let mut any_error = false;
    let mut any = false;
    for s in states {
        any = true;
        match s {
            check_state::SUCCESS => {}
            check_state::FAILURE => any_failure = true,
            _ => any_error = true,
        }
    }
    if !any {
        run_state::SKIPPED
    } else if any_failure {
        run_state::FAILED
    } else if any_error {
        run_state::ERROR
    } else {
        run_state::PASSED
    }
}

/// The compute job id a check asks for (`clientJobId`, idempotent per
/// run and check).
pub fn client_job_id(run_id: &str, index: usize) -> String {
    format!("ci-{run_id}-{index:02}")
}

/// The `/cli/compute/run` body for one check: the normal job path, at the
/// commit, detached, `--tier ci` (or `normal`), and on a watched branch
/// `--supersede ci/<branch>/<check>` so a newer push replaces this push's
/// jobs that haven't started.
pub fn job_body(run_id: &str, index: usize, name: &str, c: &CheckJob, sha: &str, git_ref: Option<&str>, tier: &str) -> serde_json::Value {
    let mut v = serde_json::json!({
        "argv": c.argv,
        "sha": sha,
        "clientJobId": client_job_id(run_id, index),
        "tier": tier,
        "detach": true,
        "submitter": "ci",
    });
    if !c.needs.is_empty() {
        v["needs"] = serde_json::json!(c.needs);
    }
    if let Some(os) = &c.os {
        v["os"] = serde_json::json!(os);
    }
    if !c.artifacts.is_empty() {
        v["artifacts"] = serde_json::json!(c.artifacts);
    }
    if let Some(t) = c.timeout_secs {
        v["timeoutSecs"] = serde_json::json!(t);
    }
    if let Some(cpus) = c.cpus {
        v["cpus"] = serde_json::json!(cpus);
    }
    if let Some(gb) = c.mem_gb {
        v["memBytes"] = serde_json::json!(u64::from(gb) << 30);
    }
    let tag = format!("ci:{name}");
    if tag.len() <= 64 {
        v["tag"] = serde_json::json!(tag);
    }
    if let Some(r) = git_ref {
        let key = format!("ci/{r}/{name}");
        if crate::compute::proto::tiers::valid_supersede(&key) {
            v["supersede"] = serde_json::json!(key);
        }
    }
    v
}

/// The checks a run takes from `cfg`: all of them, or the `only` named
/// (unknown names are an error naming the ones there are).
pub fn pick_checks<'a>(cfg: &'a CiConfig, only: &[String]) -> Result<Vec<(&'a String, &'a CheckJob)>, String> {
    for n in only {
        if !cfg.checks.contains_key(n) {
            let have: Vec<&str> = cfg.checks.keys().map(String::as_str).collect();
            return Err(format!("no check '{n}' in {CI_CONFIG_PATH} (it has: {})", if have.is_empty() { "none".to_string() } else { have.join(", ") }));
        }
    }
    let picked: Vec<(&String, &CheckJob)> = cfg.checks.iter().filter(|(n, _)| only.is_empty() || only.contains(n)).collect();
    if picked.len() > MAX_CHECKS {
        return Err(format!("{CI_CONFIG_PATH}: at most {MAX_CHECKS} checks"));
    }
    Ok(picked)
}

/// Where a run's statuses go: the config's `[statuses]` and the GitHub
/// repo of the remote. `Err` says why there are none although the config
/// asks (a remote that isn't on GitHub).
pub fn status_target(s: Option<&StatusesConfig>, slug: Option<&str>) -> Result<Option<StatusTarget>, String> {
    let Some(s) = s else { return Ok(None) };
    match slug {
        Some(slug) => Ok(Some(StatusTarget { slug: slug.to_string(), context: s.context.clone(), token_secret: s.token_secret.clone() })),
        None => Err("[statuses] is set, but the remote isn't a github.com repo: no commit statuses for this run".into()),
    }
}

/// What `k2 ci init` writes: a starter `.k2/ci.toml`.
#[derive(Debug, Clone, Default)]
pub struct Starter {
    pub remote: Option<String>,
    pub branches: Vec<String>,
    /// `[checks.<name>]` (default `test`).
    pub check: Option<String>,
    /// The check's command; empty = a placeholder that fails and says to
    /// edit the file.
    pub argv: Vec<String>,
    pub needs: Vec<String>,
    pub os: Option<String>,
    pub mem_gb: Option<u32>,
    pub cpus: Option<u32>,
    pub timeout_secs: Option<u64>,
    /// `[statuses] token_secret` (none = no statuses section).
    pub statuses_token: Option<String>,
}

/// The placeholder command a fresh `.k2/ci.toml` runs until edited.
pub const PLACEHOLDER: &str = "echo 'edit .k2/ci.toml: put your test command in [checks.test] argv' >&2; exit 1";

impl Starter {
    pub fn config(&self) -> Result<CiConfig, String> {
        let name = self.check.clone().unwrap_or_else(|| "test".to_string());
        let argv = if self.argv.is_empty() { vec!["sh".to_string(), "-c".to_string(), PLACEHOLDER.to_string()] } else { self.argv.clone() };
        let mut checks = std::collections::BTreeMap::new();
        checks.insert(
            name,
            CheckJob {
                argv,
                needs: self.needs.clone(),
                os: self.os.clone(),
                artifacts: Vec::new(),
                timeout_secs: self.timeout_secs,
                cpus: self.cpus,
                mem_gb: self.mem_gb,
            },
        );
        let cfg = CiConfig {
            version: CI_CONFIG_VERSION,
            watch: Some(WatchConfig {
                remote: self.remote.clone().unwrap_or_else(|| DEFAULT_REMOTE.to_string()),
                branches: if self.branches.is_empty() { vec![DEFAULT_BRANCH.to_string()] } else { self.branches.clone() },
                poll_secs: 60,
            }),
            statuses: self.statuses_token.as_ref().map(|t| StatusesConfig {
                token_secret: t.clone(),
                env_name: crate::compute::proto::ci_config::DEFAULT_STATUSES_ENV.to_string(),
                context: "k2-ci".to_string(),
            }),
            checks,
            release: None,
        };
        cfg.check()?;
        Ok(cfg)
    }

    /// The file's text: a short header, then the TOML (it parses back to
    /// [`Starter::config`]).
    pub fn render(&self) -> Result<String, String> {
        let cfg = self.config()?;
        let body = cfg.to_toml()?;
        let text = format!(
            "# K2 CI for this repo (k2 study ci). Commit this file: K2 reads it at\n\
             # each commit it checks. Every [checks.<name>] runs as one K2 Compute\n\
             # job on the commit; `k2 ci run` checks HEAD now, `k2 ci watch on`\n\
             # checks each push to [watch] branches.\n{body}"
        );
        // Never write what K2 itself can't read back.
        CiConfig::parse(&text)?;
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(include_str!("../drizzle_sql/0273_ci.sql")).unwrap();
        c.execute_batch("CREATE TABLE app_update_releases (id TEXT PRIMARY KEY);").unwrap();
        for stmt in include_str!("../drizzle_sql/0287_ci_release.sql").split("--> statement-breakpoint") {
            c.execute_batch(stmt).unwrap();
        }
        c
    }

    fn pats(p: &[&str]) -> Vec<String> {
        p.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn branch_patterns() {
        let p = pats(&["main", "integ/*"]);
        assert!(branch_matches(&p, "main"));
        assert!(branch_matches(&p, "integ/0.46.10"));
        assert!(branch_matches(&p, "integ/a/b"));
        assert!(!branch_matches(&p, "integ"));
        assert!(!branch_matches(&p, "integ/"));
        assert!(!branch_matches(&p, "integration"));
        assert!(!branch_matches(&p, "mainline"));
        assert!(!branch_matches(&p, "feature/main"));
    }

    #[test]
    fn ls_remote_tips_main_first_then_globs_by_name() {
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        let c = "c".repeat(40);
        let out = format!(
            "{c}\trefs/heads/integ/zz\n{b}\trefs/heads/integ/aa\n{a}\trefs/heads/main\n{a}\tHEAD\nnot a line\n{}\trefs/heads/main\n{c}\trefs/tags/v1\n{c}\trefs/heads/other\n",
            "Z".repeat(40)
        );
        let tips = tips_from_ls_remote(&out, &pats(&["main", "integ/*"]));
        assert_eq!(tips, vec![(a.clone(), "main".to_string()), (b, "integ/aa".to_string()), (c, "integ/zz".to_string())]);
        assert!(tips_from_ls_remote(&out, &pats(&["nope"])).is_empty());
    }

    #[test]
    fn verdicts_port_run_checks() {
        let j = |state: &str, exit: Option<i64>, reason: Option<&str>| JobFacts {
            state: state.into(),
            exit,
            reason: reason.map(str::to_string),
            node: "k2n7".into(),
            short: "abcd1234".into(),
            secs: 61,
        };
        assert_eq!(verdict(&j("done", Some(0), None), false), (check_state::SUCCESS, "job abcd1234 on k2n7, 2m".to_string()));
        let (s, d) = verdict(&j("done", Some(101), None), false);
        assert_eq!(s, check_state::FAILURE);
        assert!(d.contains("exit 101") && d.contains("k2 compute logs abcd1234 --failures"), "{d}");
        assert_eq!(verdict(&j("running", None, None), false).0, check_state::RUNNING);
        let (s, d) = verdict(&j("queued", None, None), true);
        assert_eq!(s, check_state::ERROR);
        assert!(d.contains("still queued at the deadline"), "{d}");
        let (s, d) = verdict(&j("cancelled", None, Some("replaced")), false);
        assert_eq!((s, d.as_str()), (check_state::ERROR, "job abcd1234 superseded by a newer push"));
        let (s, d) = verdict(&j("failed", None, Some("sync_failed")), false);
        assert_eq!(s, check_state::ERROR);
        assert!(d.contains("failed (sync_failed)"), "{d}");
    }

    #[test]
    fn run_verdicts() {
        assert_eq!(run_verdict(["success", "success"]), run_state::PASSED);
        assert_eq!(run_verdict(["success", "failure", "error"]), run_state::FAILED);
        assert_eq!(run_verdict(["success", "error"]), run_state::ERROR);
        assert_eq!(run_verdict(Vec::<&str>::new()), run_state::SKIPPED);
        assert_eq!(github_state("running"), "pending");
        assert_eq!(github_state("pending"), "pending");
        assert_eq!(github_state("failure"), "failure");
    }

    #[test]
    fn descriptions_fit_github() {
        let long = "x".repeat(300);
        let t = trim_desc(&long);
        assert_eq!(t.chars().count(), STATUS_DESC_MAX);
        assert!(t.ends_with('…'));
        assert_eq!(trim_desc("a\nb\tc"), "a b c");
    }

    #[test]
    fn job_bodies() {
        let c = CheckJob {
            argv: vec!["cargo".into(), "test".into()],
            needs: vec!["rust@1.99.1".into()],
            os: Some("linux".into()),
            artifacts: vec![],
            timeout_secs: Some(3600),
            cpus: Some(4),
            mem_gb: Some(8),
        };
        let sha = "a".repeat(40);
        let v = job_body("r1", 0, "unit", &c, &sha, Some("main"), "ci");
        assert_eq!(v["argv"], serde_json::json!(["cargo", "test"]));
        assert_eq!(v["sha"], sha.as_str());
        assert_eq!(v["clientJobId"], "ci-r1-00");
        assert_eq!(v["tier"], "ci");
        assert_eq!(v["detach"], true);
        assert_eq!(v["needs"], serde_json::json!(["rust@1.99.1"]));
        assert_eq!(v["os"], "linux");
        assert_eq!(v["memBytes"], 8u64 << 30);
        assert_eq!(v["cpus"], 4);
        assert_eq!(v["timeoutSecs"], 3600);
        assert_eq!(v["supersede"], "ci/main/unit");
        assert_eq!(v["tag"], "ci:unit");
        assert!(v.get("artifacts").is_none());
        let manual = job_body("r1", 1, "unit", &c, &sha, None, "normal");
        assert!(manual.get("supersede").is_none(), "a run on no branch replaces nothing");
        assert_eq!(manual["tier"], "normal");
        // A client job id compute accepts: 16-64 of A-Z a-z 0-9 - _.
        let id = client_job_id(&uuid::Uuid::new_v4().simple().to_string(), 31);
        assert!((16..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'), "{id}");
    }

    #[test]
    fn pick_checks_names_unknown_ones() {
        let cfg = CiConfig::parse("version = 1\n[checks.unit]\nargv = [\"true\"]\n[checks.lint]\nargv = [\"true\"]\n").unwrap();
        assert_eq!(pick_checks(&cfg, &[]).unwrap().len(), 2);
        assert_eq!(pick_checks(&cfg, &["lint".into()]).unwrap().len(), 1);
        let e = pick_checks(&cfg, &["nope".into()]).unwrap_err();
        assert!(e.contains("no check 'nope'") && e.contains("lint, unit"), "{e}");
    }

    #[test]
    fn status_targets() {
        let s = StatusesConfig { token_secret: "K2_CI_STATUSES_TOKEN".into(), env_name: "CI_STATUSES_TOKEN".into(), context: "k2-ci".into() };
        assert_eq!(status_target(None, Some("a/b")).unwrap(), None);
        let t = status_target(Some(&s), Some("acme/app")).unwrap().unwrap();
        assert_eq!((t.slug.as_str(), t.context.as_str(), t.token_secret.as_str()), ("acme/app", "k2-ci", "K2_CI_STATUSES_TOKEN"));
        assert!(status_target(Some(&s), None).unwrap_err().contains("isn't a github.com repo"));
    }

    #[test]
    fn starter_parses_back_and_fails_until_edited() {
        let text = Starter::default().render().unwrap();
        let cfg = CiConfig::parse(&text).unwrap();
        let w = cfg.watch.as_ref().unwrap();
        assert_eq!((w.remote.as_str(), w.branches.clone(), w.poll_secs), ("origin", vec!["main".to_string()], 60));
        assert_eq!(cfg.checks["test"].argv, vec!["sh".to_string(), "-c".to_string(), PLACEHOLDER.to_string()]);
        assert!(cfg.statuses.is_none());
        let s = Starter {
            check: Some("unit".into()),
            argv: vec!["cargo".into(), "test".into(), "--locked".into()],
            needs: vec!["rust@1.99.1".into()],
            os: Some("linux".into()),
            mem_gb: Some(8),
            branches: vec!["main".into(), "integ/*".into()],
            statuses_token: Some("K2_CI_STATUSES_TOKEN".into()),
            ..Default::default()
        };
        let cfg = CiConfig::parse(&s.render().unwrap()).unwrap();
        assert_eq!(cfg.checks["unit"].mem_gb, Some(8));
        assert_eq!(cfg.statuses.unwrap().env_name, "CI_STATUSES_TOKEN");
        assert!(Starter { check: Some("Bad Name".into()), ..Default::default() }.render().is_err());
        assert!(Starter { os: Some("windows".into()), ..Default::default() }.render().is_err());
    }

    #[test]
    fn watch_rules() {
        let w = |remote: &str, branches: &[&str], poll: u32, tier: &str, max: u32| Watch { remote: remote.into(), branches: pats(branches), poll_secs: poll, tier: tier.into(), max_inflight: max };
        w("origin", &["main"], 60, "ci", 2).check().unwrap();
        w("upstream", &["main", "integ/*"], 30, "normal", 8).check().unwrap();
        assert!(w("--upload-pack=x", &["main"], 60, "ci", 2).check().is_err());
        assert!(w("https://x", &["main"], 60, "ci", 2).check().is_err());
        assert!(w("origin", &[], 60, "ci", 2).check().is_err());
        assert!(w("origin", &["*"], 60, "ci", 2).check().is_err());
        assert!(w("origin", &["main"], 10, "ci", 2).check().is_err());
        assert!(w("origin", &["main"], 60, "batch", 2).check().is_err());
        assert!(w("origin", &["main"], 60, "ci", 0).check().is_err());
        assert!(w("origin", &["main"], 60, "ci", 9).check().is_err());
    }

    #[test]
    fn store_round_trip() {
        let c = conn();
        let ws = "ws-1";
        assert!(workspace(&c, ws).is_none());
        let w = Watch { remote: "origin".into(), branches: pats(&["main"]), poll_secs: 60, tier: "ci".into(), max_inflight: 2 };
        set_watch(&c, ws, &w, "owner", 10).unwrap();
        let row = workspace(&c, ws).unwrap();
        assert!(row.watch_on && row.branches == pats(&["main"]) && row.last_poll_at.is_none());
        assert!(poll_due(&row, 10));
        set_polled(&c, ws, 100, None);
        let row = workspace(&c, ws).unwrap();
        assert!(!poll_due(&row, 159) && poll_due(&row, 160));
        assert_eq!(watched(&c).len(), 1);
        assert!(watch_off(&c, ws, "owner", 200).unwrap());
        assert!(!watch_off(&c, ws, "owner", 201).unwrap(), "already off");
        assert!(watched(&c).is_empty());
        set_freeze(&c, ws, Some(1000), Some("0.46.10 release"), "owner", 300).unwrap();
        let row = workspace(&c, ws).unwrap();
        assert_eq!(frozen(&row, 999), Some((1000, Some("0.46.10 release".to_string()))));
        assert_eq!(frozen(&row, 1000), None);
        assert_eq!(row.branches, pats(&["main"]), "a freeze keeps the watch settings");

        let sha = "a".repeat(40);
        assert!(!is_seen(&c, ws, &sha));
        mark_seen(&c, ws, &sha, "main", 1);
        mark_seen(&c, ws, &sha, "integ/x", 2);
        assert!(is_seen(&c, ws, &sha));

        let target = StatusTarget { slug: "acme/app".into(), context: "k2-ci".into(), token_secret: "T".into() };
        let r = insert_run(
            &c,
            &NewRun { id: "0123456789abcdef", workspace_id: ws, sha: &sha, git_ref: Some("main"), trigger: "watch", state: run_state::RUNNING, detail: None, created_by: "watch", tier: "ci", statuses: Some(&target), at: 5 },
        )
        .unwrap();
        assert_eq!(r.statuses, Some(target));
        assert_eq!(r.ended_at, None);
        assert_eq!(inflight_count(&c, ws), 1);
        insert_check(&c, &r.id, "unit", check_state::PENDING, "submitting").unwrap();
        set_check(&c, &r.id, "unit", Some("job-1"), check_state::RUNNING, "job job-1 queued");
        set_check(&c, &r.id, "unit", None, check_state::SUCCESS, "done");
        let ch = &checks_of(&c, &r.id)[0];
        assert_eq!((ch.job_id.as_deref(), ch.state.as_str(), ch.posted.as_deref()), (Some("job-1"), "success", None));
        set_check_posted(&c, &r.id, "unit", None, 1, Some("GitHub answered 502"));
        set_check_posted(&c, &r.id, "unit", Some("success"), 0, None);
        let ch = &checks_of(&c, &r.id)[0];
        assert_eq!((ch.posted.as_deref(), ch.post_tries, ch.post_error.as_deref()), (Some("success"), 0, None));
        assert_eq!(find_run(&c, ws, "012345").unwrap().id, r.id);
        assert_eq!(find_run(&c, ws, &sha[..7]).unwrap().id, r.id);
        assert_eq!(find_run(&c, ws, "main").unwrap().id, r.id);
        assert!(find_run(&c, ws, "nope").is_none());
        assert!(find_run(&c, "ws-2", "main").is_none(), "runs are per workspace");
        assert_eq!(running_runs(&c).len(), 1);
        set_run_state(&c, &r.id, run_state::PASSED, None, Some(9));
        assert!(running_runs(&c).is_empty());
        assert_eq!(inflight_count(&c, ws), 0);
        let skipped = insert_run(
            &c,
            &NewRun { id: "fedcba9876543210", workspace_id: ws, sha: &sha, git_ref: None, trigger: "manual", state: run_state::SKIPPED, detail: Some("no .k2/ci.toml"), created_by: "owner", tier: "ci", statuses: None, at: 6 },
        )
        .unwrap();
        assert_eq!(skipped.ended_at, Some(6), "a run that never ran ends when it is made");
        assert_eq!(runs_of(&c, ws, 10)[0].id, skipped.id, "newest first");
        assert_eq!(skipped.kind, "checks");
        // A release run in flight holds no watched tip back.
        let rel = insert_run(
            &c,
            &NewRun { id: "1111111111111111", workspace_id: ws, sha: &sha, git_ref: None, trigger: "manual", state: run_state::RUNNING, detail: None, created_by: "owner", tier: "ci", statuses: None, at: 7 },
        )
        .unwrap();
        crate::ci_release::mark_release(&c, &rel.id, "1.0.0", "acme", &sha, "seat").unwrap();
        assert_eq!(run_by_id(&c, &rel.id).unwrap().kind, "release");
        assert_eq!(running_runs(&c).len(), 1, "the reaper reaps release runs too");
        assert_eq!(inflight_count(&c, ws), 0);
    }
}
