//! `k2 ci` engine (0.46.10, prd-ci-on-compute-for-clients-v1 §4.7, vs-live
//! V32 slice C6a): the daemon loop that watches a repo, runs each new tip's
//! checks on K2 Compute and posts commit statuses. Headless: it runs on a
//! daemon with no client attached (CLAUDE.md daemon-first), and it is the
//! daemon-owned replacement for `scripts/ci/k2-ci-poll.sh` + `run-checks.sh`
//! + `post-status.sh` as a `k2 job`.
//!
//! One tick ([`tick`], every [`TICK`] while compute is on):
//! 1. **reap** every run in flight: each check's compute job is read; once
//!    every job ended the run gets its verdict (`k2_core::ci::verdict`, the
//!    port of `run-checks.sh`'s reap) and its final statuses;
//! 2. **retry** statuses that didn't post (runs ended in the last day);
//! 3. **poll** each due watch: `git ls-remote <remote> --heads`, then the
//!    first tips not seen yet (main first, then `prefix/*` by name) are
//!    marked seen and started, while fewer than the watch's cap are in
//!    flight. A tip over the cap waits for a later tick.
//!
//! Starting a run ([`launch`]): the commit is fetched when it isn't in the
//! workspace's repo, `.k2/ci.toml` is read **at that commit**, and each
//! `[checks.<name>]` becomes one compute job through the normal job path
//! (`compute_routes::submit_for_ci`: today's placement, the workspace's
//! own grants, `--tier ci`, `--supersede ci/<branch>/<check>` on a watched
//! branch). A run is daemon state (`ci_runs`, `ci_checks`), never a
//! `k2 job`.
//!
//! Statuses go to GitHub with the token `[statuses] token_secret` names,
//! read from **the workspace's vault** as that workspace with a fetch use
//! bound to the API's origin (so the secret must allow `api.github.com`,
//! which `k2 vault set … --preset github` does). The token is used here and
//! dropped; it never reaches a job, a log, k2.db or an answer.
//!
//! Every outside effect goes through [`Ops`], so the rules are tested with
//! a fake compute and a fake forge ([`test_set_ops`]).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use k2_core::ci::{self as ci, check_state, codes, run_state, CheckRow, JobFacts, RunRow, StatusTarget};
use k2_core::compute::proto::ci_config::{CheckJob, CiConfig, CI_CONFIG_PATH};
use k2_core::log_debug;

use crate::compute_routes::Ws;

/// How often the loop runs.
pub const TICK: Duration = Duration::from_secs(15);
/// Longest a `git ls-remote` / `git fetch` may take.
const GIT_TIMEOUT: Duration = Duration::from_secs(120);
/// Largest `.k2/ci.toml` read.
const MAX_CONFIG_BYTES: usize = 64 * 1024;
/// Ended runs whose statuses are still retried.
const RETRY_WINDOW_SECS: i64 = 86_400;

// ── the outside world ────────────────────────────────────────────────

/// Everything the engine does outside k2.db: git in the workspace's repo,
/// compute jobs, the forge. [`Real`] in the daemon; a fake in tests.
pub trait Ops: Send + Sync {
    /// The repo top level for a workspace folder.
    fn repo_root(&self, ws_path: &Path) -> Result<PathBuf, String>;
    /// `git ls-remote --heads <remote>` output.
    fn ls_remote(&self, root: &Path, remote: &str) -> Result<String, String>;
    fn has_commit(&self, root: &Path, sha: &str) -> bool;
    /// `git fetch <remote> <what>` (a sha or `refs/heads/<branch>`), no ref
    /// written.
    fn fetch(&self, root: &Path, remote: &str, what: &str) -> Result<(), String>;
    /// A revision → its full commit id.
    fn resolve(&self, checkout: &Path, rev: &str) -> Result<String, String>;
    /// Is `name` a local branch?
    fn is_branch(&self, root: &Path, name: &str) -> bool;
    /// `.k2/ci.toml` at `sha` (`None`: the commit has none).
    fn read_config(&self, root: &Path, sha: &str) -> Result<Option<String>, String>;
    /// `owner/repo` when the remote is a github.com repo.
    fn remote_slug(&self, root: &Path, remote: &str) -> Option<String>;
    /// Submit one compute job → its id, or `(code, hint)`.
    fn submit(&self, ws_id: &str, run_id: &str, body: &serde_json::Value) -> Result<String, (String, String)>;
    /// What a compute job says now.
    fn job(&self, job_id: &str) -> Option<JobFacts>;
    /// One commit status. `Err` is one line for the check's row.
    fn post_status(&self, ws_id: &str, target: &StatusTarget, sha: &str, context: &str, state: &str, description: &str) -> Result<(), String>;
}

static OPS: Mutex<Option<Arc<dyn Ops>>> = Mutex::new(None);

pub(crate) fn ops() -> Arc<dyn Ops> {
    OPS.lock().unwrap_or_else(|p| p.into_inner()).clone().unwrap_or_else(|| Arc::new(Real))
}

/// Test seam: run the engine against fakes (`None`: the real world again).
#[doc(hidden)]
pub fn test_set_ops(o: Option<Arc<dyn Ops>>) {
    *OPS.lock().unwrap_or_else(|p| p.into_inner()) = o;
}

static STATUSES_API: Mutex<Option<String>> = Mutex::new(None);

fn statuses_api() -> String {
    STATUSES_API.lock().unwrap_or_else(|p| p.into_inner()).clone().unwrap_or_else(|| "https://api.github.com".into())
}

/// Test seam: post statuses to a fake GitHub (`None`: GitHub again).
#[doc(hidden)]
pub fn test_set_statuses_api(base: Option<String>) {
    *STATUSES_API.lock().unwrap_or_else(|p| p.into_inner()) = base;
}

/// One reap or launch at a time: two never post the same run's statuses
/// or decide its verdict together. Never held across `git ls-remote`.
static RUN_LOCK: Mutex<()> = Mutex::new(());

fn now() -> i64 {
    k2_core::compute::now()
}

fn db() -> Arc<parking_lot::ReentrantMutex<rusqlite::Connection>> {
    k2_core::db::shared()
}

// ── the loop ─────────────────────────────────────────────────────────

/// The CI loop (spawned after the readiness gate, like `k2 job`'s). It
/// does nothing while compute is off.
pub fn spawn_loop() -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(5)).await;
        let mut t = tokio::time::interval(TICK);
        t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            t.tick().await;
            if !k2_core::compute::enabled() {
                continue;
            }
            if let Err(e) = tokio::task::spawn_blocking(|| tick(ops().as_ref(), now())).await {
                log_debug!("[ci] tick panicked: {e}");
            }
        }
    })
}

/// One tick with the real world (tests drive the loop by hand).
#[doc(hidden)]
pub fn tick_now() {
    tick(ops().as_ref(), now());
}

/// One pass: reap, retry statuses, poll due watches.
pub fn tick(o: &dyn Ops, now: i64) {
    let running = {
        let db = db();
        let conn = db.lock();
        ci::running_runs(&conn)
    };
    for r in running {
        reap(o, &r.id, now);
    }
    for r in runs_owing_statuses(now) {
        let _g = RUN_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        sync_statuses(o, &r);
    }
    let watched = {
        let db = db();
        let conn = db.lock();
        ci::watched(&conn)
    };
    for w in watched.into_iter().filter(|w| ci::poll_due(w, now)) {
        poll(o, &w, now);
    }
}

/// Ended runs (last day) with a status still to post.
fn runs_owing_statuses(now: i64) -> Vec<RunRow> {
    let db = db();
    let conn = db.lock();
    let ids: Vec<String> = conn
        .prepare("SELECT id FROM ci_runs WHERE state != 'running' AND statuses_json IS NOT NULL AND ended_at >= ?1 ORDER BY ended_at LIMIT 200")
        .and_then(|mut st| st.query_map([now - RETRY_WINDOW_SECS], |r| r.get::<_, String>(0)).map(|rows| rows.filter_map(Result::ok).collect()))
        .unwrap_or_default();
    ids.iter()
        .filter_map(|id| ci::run_by_id(&conn, id))
        .filter(|r| ci::checks_of(&conn, &r.id).iter().any(owes_status))
        .collect()
}

fn owes_status(c: &CheckRow) -> bool {
    c.posted.as_deref() != Some(ci::github_state(&c.state)) && c.post_tries < ci::MAX_STATUS_TRIES
}

// ── watch ────────────────────────────────────────────────────────────

/// One poll of one watch: new tips → runs, up to the cap in flight.
pub fn poll(o: &dyn Ops, w: &ci::WorkspaceRow, now: i64) {
    let Some(ws) = crate::compute_routes::ws_by_id(&w.workspace_id) else {
        let db = db();
        let conn = db.lock();
        ci::set_polled(&conn, &w.workspace_id, now, Some("the workspace is no longer registered"));
        return;
    };
    // A watch an agent (or a K2 tab) turned on follows the workspace's
    // "Allow compute nodes" switch: off = no new runs until it's back on.
    if is_passport_label(&w.watch_by) {
        let db = db();
        let conn = db.lock();
        if !k2_core::compute::store::agents_can_use_compute(&conn, &ws.id) {
            ci::set_polled(&conn, &ws.id, now, Some("the workspace's \"Allow compute nodes\" switch is off; an agent's watch waits for it"));
            return;
        }
    }
    let tips = match o.repo_root(Path::new(&ws.path)).and_then(|root| o.ls_remote(&root, &w.remote).map(|out| (root, out))) {
        Ok((root, out)) => (root, ci::tips_from_ls_remote(&out, &w.branches)),
        Err(e) => {
            let db = db();
            let conn = db.lock();
            ci::set_polled(&conn, &ws.id, now, Some(&one_line(&e)));
            return;
        }
    };
    {
        let db = db();
        let conn = db.lock();
        ci::set_polled(&conn, &ws.id, now, None);
    }
    let (root, tips) = tips;
    for (sha, branch) in tips {
        let (seen, inflight) = {
            let db = db();
            let conn = db.lock();
            (ci::is_seen(&conn, &ws.id, &sha), ci::inflight_count(&conn, &ws.id))
        };
        if seen {
            continue;
        }
        if inflight >= w.max_inflight {
            log_debug!("[ci] {}: {inflight} run(s) in flight (max {}); {branch} @ {} waits", ws.handle, w.max_inflight, &sha[..12]);
            break;
        }
        {
            let db = db();
            let conn = db.lock();
            ci::mark_seen(&conn, &ws.id, &sha, &branch, now);
        }
        let start = Start { sha: &sha, git_ref: Some(&branch), trigger: "watch", by: "watch", tier: &w.tier, remote: &w.remote, only: &[] };
        match prepare(o, &ws, &root, &start) {
            Ok(p) => {
                launch(o, &ws, &start, p, now);
            }
            Err(problem) => {
                record_unrun(&ws, &start, &problem, now);
            }
        }
    }
}

/// `agent:<ws>` / `shell:<ws>` ([`crate::compute_routes::Who::label`]): set
/// through a passport, not by a person outside K2.
fn is_passport_label(by: &str) -> bool {
    by.starts_with("agent:") || by.starts_with("shell:")
}

/// The current watched tips (for `k2 ci watch on`): `(sha, branch)`.
pub(crate) fn current_tips(ws: &Ws, remote: &str, branches: &[String]) -> Result<Vec<(String, String)>, String> {
    let o = ops();
    let root = o.repo_root(Path::new(&ws.path))?;
    let out = o.ls_remote(&root, remote)?;
    Ok(ci::tips_from_ls_remote(&out, branches))
}

// ── starting a run ───────────────────────────────────────────────────

/// What a run is asked to check.
pub struct Start<'a> {
    pub sha: &'a str,
    pub git_ref: Option<&'a str>,
    /// `watch` or `manual`.
    pub trigger: &'a str,
    pub by: &'a str,
    /// `ci` or `normal`.
    pub tier: &'a str,
    pub remote: &'a str,
    /// Only these checks (empty: all).
    pub only: &'a [String],
}

/// Why a commit can't be run: a refusal for `k2 ci run`, a `skipped` or
/// `error` row for the watcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub code: &'static str,
    pub status: u16,
    pub hint: String,
}

fn problem(code: &'static str, status: u16, hint: impl Into<String>) -> Problem {
    Problem { code, status, hint: hint.into() }
}

/// What [`launch`] needs: the checks at the commit and where statuses go.
pub struct Prepared {
    pub checks: Vec<(String, CheckJob)>,
    pub statuses: Option<StatusTarget>,
    /// Said on the run (e.g. why there are no statuses).
    pub note: Option<String>,
}

/// The commit here (fetched when missing), `.k2/ci.toml` at it, the checks
/// asked for, and the statuses target.
pub(crate) fn prepare(o: &dyn Ops, _ws: &Ws, root: &Path, s: &Start<'_>) -> Result<Prepared, Problem> {
    if !ci::is_sha(s.sha) {
        return Err(problem(codes::NO_COMMIT, 400, format!("{} is not a full commit id", s.sha)));
    }
    if !o.has_commit(root, s.sha) {
        // The branch first (any server allows it), then the commit itself.
        let by_branch = s.git_ref.map(|b| o.fetch(root, s.remote, &format!("refs/heads/{b}")));
        if !o.has_commit(root, s.sha) {
            let direct = o.fetch(root, s.remote, s.sha);
            if !o.has_commit(root, s.sha) {
                let why = direct.err().or(by_branch.and_then(Result::err)).unwrap_or_else(|| "the remote doesn't have it".into());
                return Err(problem(codes::NO_COMMIT, 404, format!("commit {} isn't in the workspace's repo and couldn't be fetched from {}: {}", &s.sha[..12], s.remote, one_line(&why))));
            }
        }
    }
    let text = o
        .read_config(root, s.sha)
        .map_err(|e| problem(codes::BAD_CONFIG, 400, format!("{CI_CONFIG_PATH} at {}: {}", &s.sha[..12], one_line(&e))))?
        .ok_or_else(|| {
            problem(
                codes::NO_CONFIG,
                404,
                format!("commit {} has no {CI_CONFIG_PATH}: write one (k2 ci init), commit it, then run again (k2 study ci)", &s.sha[..12]),
            )
        })?;
    let cfg = CiConfig::parse(&text).map_err(|e| problem(codes::BAD_CONFIG, 400, format!("at {}: {e}", &s.sha[..12])))?;
    let picked = ci::pick_checks(&cfg, s.only).map_err(|e| problem(codes::NO_CHECK, 404, e))?;
    if picked.is_empty() {
        return Err(problem(codes::NO_CONFIG, 404, format!("{CI_CONFIG_PATH} at {} has no [checks.<name>]: add one (k2 study ci)", &s.sha[..12])));
    }
    let checks = picked.into_iter().map(|(n, c)| (n.clone(), c.clone())).collect();
    let slug = cfg.statuses.as_ref().and_then(|_| o.remote_slug(root, s.remote));
    let (statuses, note) = match ci::status_target(cfg.statuses.as_ref(), slug.as_deref()) {
        Ok(t) => (t, None),
        Err(e) => (None, Some(e)),
    };
    Ok(Prepared { checks, statuses, note })
}

/// A watched tip that can't run: a `skipped` run (no config) or an `error`
/// one, so `k2 ci status` shows why nothing ran for it.
fn record_unrun(ws: &Ws, s: &Start<'_>, p: &Problem, now: i64) {
    let state = if p.code == codes::NO_CONFIG { run_state::SKIPPED } else { run_state::ERROR };
    let id = new_run_id();
    let db = db();
    let conn = db.lock();
    let _ = ci::insert_run(
        &conn,
        &ci::NewRun { id: &id, workspace_id: &ws.id, sha: s.sha, git_ref: s.git_ref, trigger: s.trigger, state, detail: Some(&format!("{}: {}", p.code, p.hint)), created_by: s.by, tier: s.tier, statuses: None, at: now },
    );
}

pub fn new_run_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// Store the run, submit each check's job, post `pending` (or `error` for
/// a check that couldn't be submitted). A run whose every check failed to
/// submit ends at once.
pub(crate) fn launch(o: &dyn Ops, ws: &Ws, s: &Start<'_>, p: Prepared, now: i64) -> RunRow {
    let _g = RUN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let id = new_run_id();
    let run = {
        let db = db();
        let conn = db.lock();
        let r = ci::insert_run(
            &conn,
            &ci::NewRun {
                id: &id,
                workspace_id: &ws.id,
                sha: s.sha,
                git_ref: s.git_ref,
                trigger: s.trigger,
                state: run_state::RUNNING,
                detail: p.note.as_deref(),
                created_by: s.by,
                tier: s.tier,
                statuses: p.statuses.as_ref(),
                at: now,
            },
        );
        match r {
            Ok(r) => {
                for (name, _) in &p.checks {
                    let _ = ci::insert_check(&conn, &id, name, check_state::PENDING, "submitting");
                }
                r
            }
            Err(e) => {
                log_debug!("[ci] {}: storing a run failed: {e}", ws.handle);
                return RunRow {
                    id,
                    workspace_id: ws.id.clone(),
                    sha: s.sha.to_string(),
                    git_ref: s.git_ref.map(str::to_string),
                    trigger: s.trigger.to_string(),
                    state: run_state::ERROR.to_string(),
                    detail: Some(e),
                    created_by: s.by.to_string(),
                    tier: s.tier.to_string(),
                    statuses: None,
                    created_at: now,
                    ended_at: Some(now),
                };
            }
        }
    };
    for (i, (name, c)) in p.checks.iter().enumerate() {
        let body = ci::job_body(&id, i, name, c, s.sha, s.git_ref, s.tier);
        let (job, state, desc) = match o.submit(&ws.id, &id, &body) {
            Ok(job) => {
                let short = &job[..job.len().min(8)];
                (Some(job.clone()), check_state::PENDING, format!("job {short} queued ({} tier)", if s.tier == "ci" { "CI" } else { "Standard" }))
            }
            Err((code, hint)) => (None, check_state::ERROR, format!("not submitted: {code}: {hint}")),
        };
        let db = db();
        let conn = db.lock();
        ci::set_check(&conn, &id, name, job.as_deref(), state, &desc);
    }
    sync_statuses(o, &run);
    let checks = {
        let db = db();
        let conn = db.lock();
        ci::checks_of(&conn, &id)
    };
    if checks.iter().all(CheckRow::is_final) {
        finish(o, &run, &checks, now);
    }
    let db = db();
    let conn = db.lock();
    ci::run_by_id(&conn, &id).unwrap_or(run)
}

// ── reaping ──────────────────────────────────────────────────────────

/// Look at a running run's jobs once; once every job ended, its verdict
/// and final statuses.
pub fn reap(o: &dyn Ops, run_id: &str, now: i64) {
    let _g = RUN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (run, checks) = {
        let db = db();
        let conn = db.lock();
        let Some(r) = ci::run_by_id(&conn, run_id) else { return };
        let c = ci::checks_of(&conn, run_id);
        (r, c)
    };
    if !run.is_running() {
        return;
    }
    let late = now >= run.created_at + ci::RUN_DEADLINE_SECS;
    for c in checks.iter().filter(|c| !c.is_final()) {
        let (state, desc) = match c.job_id.as_deref() {
            None => (check_state::ERROR, "never submitted".to_string()),
            Some(job) => match o.job(job) {
                Some(f) => {
                    let (s, d) = ci::verdict(&f, late);
                    (s, d)
                }
                None if late => (check_state::ERROR, format!("job {} has no record here", &job[..job.len().min(8)])),
                None => continue,
            },
        };
        let db = db();
        let conn = db.lock();
        ci::set_check(&conn, run_id, &c.name, None, state, &desc);
        if ci::is_final_check(state) {
            // The final state gets its own tries at posting.
            ci::set_check_posted(&conn, run_id, &c.name, None, 0, c.post_error.as_deref());
        }
    }
    let checks = {
        let db = db();
        let conn = db.lock();
        ci::checks_of(&conn, run_id)
    };
    if checks.iter().all(CheckRow::is_final) {
        finish(o, &run, &checks, now);
    } else {
        sync_statuses(o, &run);
    }
}

fn finish(o: &dyn Ops, run: &RunRow, checks: &[CheckRow], now: i64) {
    let state = ci::run_verdict(checks.iter().map(|c| c.state.as_str()));
    let count = |s: &str| checks.iter().filter(|c| c.state == s).count();
    let mut detail = format!("{} passed, {} failed, {} error", count(check_state::SUCCESS), count(check_state::FAILURE), count(check_state::ERROR));
    if let Some(note) = &run.detail {
        if !note.is_empty() {
            detail = format!("{detail}; {note}");
        }
    }
    {
        let db = db();
        let conn = db.lock();
        ci::set_run_state(&conn, &run.id, state, Some(&detail), Some(now));
    }
    sync_statuses(o, run);
}

// ── statuses ─────────────────────────────────────────────────────────

/// Post each check's state where GitHub doesn't have it yet. A failure is
/// kept on the check (`k2 ci status` shows it) and tried again next tick.
fn sync_statuses(o: &dyn Ops, run: &RunRow) {
    let Some(target) = run.statuses.as_ref() else { return };
    let checks = {
        let db = db();
        let conn = db.lock();
        ci::checks_of(&conn, &run.id)
    };
    for c in checks.iter().filter(|c| owes_status(c)) {
        let want = ci::github_state(&c.state);
        let context = format!("{}/{}", target.context, c.name);
        let r = o.post_status(&run.workspace_id, target, &run.sha, &context, want, &ci::trim_desc(&c.description));
        let db = db();
        let conn = db.lock();
        match r {
            Ok(()) => ci::set_check_posted(&conn, &run.id, &c.name, Some(want), 0, None),
            Err(e) => ci::set_check_posted(&conn, &run.id, &c.name, None, c.post_tries + 1, Some(&one_line(&e))),
        }
    }
}

fn one_line(s: &str) -> String {
    let t: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() > 300 {
        let mut o: String = t.chars().take(299).collect();
        o.push('…');
        o
    } else {
        t
    }
}

// ── the real world ───────────────────────────────────────────────────

/// Git in the workspace's repo ([`k2_core::git_safe`]: no hooks, no
/// fsmonitor, no prompts), compute through `compute_routes`, GitHub
/// through `compute::git::Github`, the token from the vault.
pub struct Real;

/// Run a hardened git with a deadline; stdout on success.
fn run_git(mut c: std::process::Command, what: &str) -> Result<String, String> {
    use std::io::Read;
    c.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    let mut child = c.spawn().map_err(|e| format!("{what}: can't run git: {e}"))?;
    let mut out = child.stdout.take();
    let mut err = child.stderr.take();
    let t_out = std::thread::spawn(move || {
        let mut b = Vec::new();
        if let Some(o) = out.as_mut() {
            let _ = o.read_to_end(&mut b);
        }
        b
    });
    let t_err = std::thread::spawn(move || {
        let mut b = Vec::new();
        if let Some(e) = err.as_mut() {
            let _ = e.read_to_end(&mut b);
        }
        b
    });
    let t0 = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if t0.elapsed() > GIT_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{what}: git took over {}s", GIT_TIMEOUT.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(format!("{what}: {e}")),
        }
    };
    let stdout = t_out.join().unwrap_or_default();
    let stderr = t_err.join().unwrap_or_default();
    if !status.success() {
        return Err(format!("{what}: {}", String::from_utf8_lossy(&stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&stdout).into_owned())
}

fn git(root: &Path) -> std::process::Command {
    k2_core::git_safe::git(root)
}

impl Ops for Real {
    fn repo_root(&self, ws_path: &Path) -> Result<PathBuf, String> {
        k2_core::compute::sync::repo_root(ws_path)
    }

    fn ls_remote(&self, root: &Path, remote: &str) -> Result<String, String> {
        if !ci::valid_remote(remote) {
            return Err(format!("'{remote}' is not a git remote name"));
        }
        let mut c = git(root);
        c.args(["ls-remote", "--heads", remote]);
        run_git(c, &format!("git ls-remote {remote}"))
    }

    fn has_commit(&self, root: &Path, sha: &str) -> bool {
        if !ci::is_sha(sha) {
            return false;
        }
        let mut c = git(root);
        c.args(["cat-file", "-e", &format!("{sha}^{{commit}}")]);
        run_git(c, "look for the commit").is_ok()
    }

    fn fetch(&self, root: &Path, remote: &str, what: &str) -> Result<(), String> {
        let ok_what = ci::is_sha(what) || what.strip_prefix("refs/heads/").is_some_and(|b| !b.is_empty() && !b.contains(char::is_whitespace) && !b.contains(".."));
        if !ci::valid_remote(remote) || !ok_what {
            return Err(format!("won't fetch '{what}' from '{remote}'"));
        }
        let mut c = git(root);
        c.args(["fetch", "--quiet", "--no-tags", "--no-write-fetch-head", remote, what]);
        run_git(c, &format!("git fetch {remote}")).map(|_| ())
    }

    fn resolve(&self, checkout: &Path, rev: &str) -> Result<String, String> {
        k2_core::compute::sync::resolve_commit(checkout, rev)
    }

    fn is_branch(&self, root: &Path, name: &str) -> bool {
        if name.is_empty() || name.starts_with('-') || name.contains(char::is_whitespace) || name.contains("..") {
            return false;
        }
        let mut c = git(root);
        c.args(["show-ref", "--verify", "--quiet", &format!("refs/heads/{name}")]);
        run_git(c, "look for the branch").is_ok()
    }

    fn read_config(&self, root: &Path, sha: &str) -> Result<Option<String>, String> {
        if !ci::is_sha(sha) {
            return Err("not a commit id".into());
        }
        let spec = format!("{sha}:{CI_CONFIG_PATH}");
        let mut probe = git(root);
        probe.args(["cat-file", "-e", &spec]);
        if run_git(probe, "look for the config").is_err() {
            return Ok(None);
        }
        let mut c = git(root);
        c.args(["cat-file", "blob", &spec]);
        let text = run_git(c, "read the config")?;
        if text.len() > MAX_CONFIG_BYTES {
            return Err(format!("over {} KiB", MAX_CONFIG_BYTES / 1024));
        }
        Ok(Some(text))
    }

    fn remote_slug(&self, root: &Path, remote: &str) -> Option<String> {
        if !ci::valid_remote(remote) {
            return None;
        }
        // The URL as configured (not after `url.<x>.insteadOf`): that names
        // the forge repo the statuses belong to.
        let mut c = git(root);
        c.args(["config", "--get", &format!("remote.{remote}.url")]);
        let url = run_git(c, "read the remote").ok()?;
        let key = k2_core::compute::proto::gitcred::repo_of_remote(&k2_core::compute::sync::fetchable_remote(url.trim())?)?;
        let (owner, repo) = k2_core::compute::proto::gitcred::owner_repo(&key)?;
        Some(format!("{owner}/{repo}"))
    }

    fn submit(&self, ws_id: &str, run_id: &str, body: &serde_json::Value) -> Result<String, (String, String)> {
        match crate::compute_routes::submit_for_ci(ws_id, run_id, body) {
            Ok(v) => v["job"]["id"].as_str().map(str::to_string).ok_or_else(|| ("internal".to_string(), "compute answered without a job id".to_string())),
            Err(r) => Err((r.code.to_string(), r.hint)),
        }
    }

    fn job(&self, job_id: &str) -> Option<JobFacts> {
        let db = db();
        let conn = db.lock();
        let j = k2_core::compute::store::job_by_id(&conn, job_id)?;
        let node = k2_core::compute::store::node_by_id(&conn, &j.node_id).map(|n| n.name).unwrap_or_else(|| "?".into());
        let secs = match (j.started_at, j.ended_at) {
            (Some(s), Some(e)) => e - s,
            (Some(s), None) => now() - s,
            _ => 0,
        };
        // A node-queued job is `assigned` in the row; every view says queued.
        let state = if j.node_queued && j.state == "assigned" { "queued".to_string() } else { j.state.clone() };
        Some(JobFacts { state, exit: j.exit_code, reason: j.reason.clone(), node, short: j.id[..j.id.len().min(8)].to_string(), secs })
    }

    fn post_status(&self, ws_id: &str, target: &StatusTarget, sha: &str, context: &str, state: &str, description: &str) -> Result<(), String> {
        use k2_vault::hosts::Origin;
        use k2_vault::{Requester, Scope, SecretStore, Selector, Use};
        let base = statuses_api();
        let (_, origin) = Origin::parse_url(&format!("{base}/")).map_err(|e| format!("statuses API {base}: {}", e.code))?;
        let vault = crate::vault::get().map_err(|e| format!("the vault isn't open ({}); statuses wait for it", e.code))?;
        let who = Requester::Session { workspace_id: ws_id.to_string(), session_id: None };
        let name = target.token_secret.as_str();
        let host = origin.host_str();
        let resolved = vault.resolve(&Selector::user(Scope::workspace(ws_id), name), Use::Fetch(&origin), &who).or_else(|e| {
            if e.status == 404 {
                vault.resolve(&Selector::user(Scope::server(), name), Use::Fetch(&origin), &who)
            } else {
                Err(e)
            }
        });
        let r = resolved.map_err(|e| match e.code {
            "no_allowed_hosts" | "host_not_allowed" => {
                format!("{name} may not be sent to {host}: a person runs k2 vault hosts {name} --add {host} (or sets it with --preset github)")
            }
            _ if e.status == 404 => format!("no secret {name} in this workspace's vault: a person stores it (k2 study ci, STATUSES)"),
            c => format!("the vault refused {name} ({c}): {}", e.hint),
        })?;
        let token = r.value.expose_str().map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| format!("{name} is empty or not text"))?;
        let (owner, repo) = target.slug.split_once('/').ok_or_else(|| format!("'{}' is not owner/repo", target.slug))?;
        let gh = k2_core::compute::git::Github::new(&base)?;
        gh.create_status(token, owner, repo, sha, state, context, description).map_err(|(_, m)| m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, HashMap};

    /// A fake compute + forge + git. Jobs are submitted into a map the test
    /// moves along; statuses are recorded.
    #[derive(Default)]
    struct Fake {
        tips: Mutex<String>,
        configs: Mutex<HashMap<String, String>>,
        slug: Mutex<Option<String>>,
        missing: Mutex<Vec<String>>,
        fetched: Mutex<Vec<String>>,
        refuse: Mutex<HashMap<String, (String, String)>>,
        jobs: Mutex<BTreeMap<String, JobFacts>>,
        bodies: Mutex<Vec<serde_json::Value>>,
        posts: Mutex<Vec<(String, String, String, String)>>,
        post_fails: Mutex<u32>,
        ls_fails: Mutex<Option<String>>,
    }

    impl Fake {
        fn finish(&self, check: &str, exit: i64) {
            let mut jobs = self.jobs.lock().unwrap();
            let bodies = self.bodies.lock().unwrap();
            for b in bodies.iter().filter(|b| b["tag"] == format!("ci:{check}")) {
                let id = b["clientJobId"].as_str().unwrap();
                if let Some(j) = jobs.get_mut(id) {
                    j.state = "done".into();
                    j.exit = Some(exit);
                }
            }
        }
        fn posts(&self) -> Vec<(String, String, String, String)> {
            self.posts.lock().unwrap().clone()
        }
    }

    impl Ops for Fake {
        fn repo_root(&self, p: &Path) -> Result<PathBuf, String> {
            Ok(p.to_path_buf())
        }
        fn ls_remote(&self, _: &Path, remote: &str) -> Result<String, String> {
            assert_eq!(remote, "origin");
            match self.ls_fails.lock().unwrap().clone() {
                Some(e) => Err(e),
                None => Ok(self.tips.lock().unwrap().clone()),
            }
        }
        fn has_commit(&self, _: &Path, sha: &str) -> bool {
            !self.missing.lock().unwrap().iter().any(|m| m == sha)
        }
        fn fetch(&self, _: &Path, _: &str, what: &str) -> Result<(), String> {
            self.fetched.lock().unwrap().push(what.to_string());
            self.missing.lock().unwrap().retain(|m| !(what.ends_with("/main") || m == what));
            Ok(())
        }
        fn resolve(&self, _: &Path, rev: &str) -> Result<String, String> {
            Ok(if ci::is_sha(rev) { rev.to_string() } else { "f".repeat(40) })
        }
        fn is_branch(&self, _: &Path, name: &str) -> bool {
            name == "main"
        }
        fn read_config(&self, _: &Path, sha: &str) -> Result<Option<String>, String> {
            Ok(self.configs.lock().unwrap().get(sha).cloned())
        }
        fn remote_slug(&self, _: &Path, _: &str) -> Option<String> {
            self.slug.lock().unwrap().clone()
        }
        fn submit(&self, _ws: &str, _run: &str, body: &serde_json::Value) -> Result<String, (String, String)> {
            self.bodies.lock().unwrap().push(body.clone());
            let tag = body["tag"].as_str().unwrap_or_default().to_string();
            if let Some(r) = self.refuse.lock().unwrap().get(&tag) {
                return Err(r.clone());
            }
            let id = body["clientJobId"].as_str().unwrap().to_string();
            self.jobs.lock().unwrap().insert(id.clone(), JobFacts { state: "queued".into(), exit: None, reason: None, node: "k2n7".into(), short: id[..8].to_string(), secs: 60 });
            Ok(id)
        }
        fn job(&self, id: &str) -> Option<JobFacts> {
            self.jobs.lock().unwrap().get(id).cloned()
        }
        fn post_status(&self, _ws: &str, t: &StatusTarget, sha: &str, context: &str, state: &str, desc: &str) -> Result<(), String> {
            let mut f = self.post_fails.lock().unwrap();
            if *f > 0 {
                *f -= 1;
                return Err("GitHub answered 502".into());
            }
            self.posts.lock().unwrap().push((format!("{}@{}", t.slug, &sha[..7]), context.to_string(), state.to_string(), desc.to_string()));
            Ok(())
        }
    }

    const CFG: &str = "version = 1\n[watch]\nremote = \"origin\"\nbranches = [\"main\", \"integ/*\"]\n[statuses]\ntoken_secret = \"K2_CI_STATUSES_TOKEN\"\n[checks.unit]\nargv = [\"cargo\", \"test\"]\nmem_gb = 8\n[checks.lint]\nargv = [\"cargo\", \"clippy\"]\n";

    fn sha(c: char) -> String {
        c.to_string().repeat(40)
    }

    fn seed_ws() -> Ws {
        let id = uuid::Uuid::new_v4().to_string();
        let path = std::env::temp_dir().join(format!("k2ci-{id}"));
        {
            let db = db();
            let conn = db.lock();
            conn.execute("INSERT INTO projects (id, name, path) VALUES (?1, 'ciws', ?2)", rusqlite::params![id, path.to_string_lossy()]).unwrap();
        }
        crate::compute_routes::ws_by_id(&id).expect("ws")
    }

    fn watch(ws: &Ws, max: u32) {
        let db = db();
        let conn = db.lock();
        let w = ci::Watch { remote: "origin".into(), branches: vec!["main".into(), "integ/*".into()], poll_secs: 60, tier: "ci".into(), max_inflight: max };
        ci::set_watch(&conn, &ws.id, &w, "owner", 1).unwrap();
    }

    fn runs(ws: &Ws) -> Vec<RunRow> {
        let db = db();
        let conn = db.lock();
        let mut r = ci::runs_of(&conn, &ws.id, 50);
        r.reverse();
        r
    }

    fn checks(run: &str) -> Vec<CheckRow> {
        let db = db();
        let conn = db.lock();
        ci::checks_of(&conn, run)
    }

    #[test]
    fn watch_runs_new_tips_reaps_and_posts_statuses() {
        let _db = k2_core::db::scoped_for_test();
        let f = Fake::default();
        let ws = seed_ws();
        watch(&ws, 2);
        let (a, b) = (sha('a'), sha('b'));
        *f.tips.lock().unwrap() = format!("{a}\trefs/heads/main\n{b}\trefs/heads/integ/x\n{b}\trefs/heads/other\n");
        f.configs.lock().unwrap().insert(a.clone(), CFG.into());
        f.configs.lock().unwrap().insert(b.clone(), CFG.into());
        *f.slug.lock().unwrap() = Some("acme/app".into());

        tick(&f, 100);
        let rs = runs(&ws);
        assert_eq!(rs.len(), 2, "both new tips run");
        assert_eq!((rs[0].sha.as_str(), rs[0].git_ref.as_deref(), rs[0].trigger.as_str()), (a.as_str(), Some("main"), "watch"));
        assert_eq!(rs[1].git_ref.as_deref(), Some("integ/x"));
        assert!(rs.iter().all(RunRow::is_running));
        // One job per check, on the normal path's terms.
        let bodies = f.bodies.lock().unwrap().clone();
        assert_eq!(bodies.len(), 4);
        let unit_main = bodies.iter().find(|b| b["sha"] == a.as_str() && b["tag"] == "ci:unit").unwrap();
        assert_eq!(unit_main["tier"], "ci");
        assert_eq!(unit_main["supersede"], "ci/main/unit");
        assert_eq!(unit_main["memBytes"], 8u64 << 30);
        assert_eq!(unit_main["detach"], true);
        // pending for each check, as <context>/<check>.
        let posts = f.posts();
        assert_eq!(posts.len(), 4);
        assert!(posts.iter().all(|p| p.2 == "pending"), "{posts:?}");
        assert!(posts.iter().any(|p| p.0 == format!("acme/app@{}", &a[..7]) && p.1 == "k2-ci/unit"));

        // Nothing new: no new run, no repeated pending.
        tick(&f, 200);
        assert_eq!(runs(&ws).len(), 2);
        assert_eq!(f.posts().len(), 4, "a pending check isn't posted twice");

        // main's unit passes, lint fails; integ/x both pass.
        f.finish("unit", 0);
        f.jobs.lock().unwrap().values_mut().filter(|j| j.state == "queued").for_each(|j| j.state = "running".into());
        tick(&f, 300);
        assert!(runs(&ws).iter().all(RunRow::is_running), "lint still runs everywhere");
        let mut jobs = f.jobs.lock().unwrap();
        let bodies = f.bodies.lock().unwrap();
        for bdy in bodies.iter() {
            let id = bdy["clientJobId"].as_str().unwrap();
            if bdy["tag"] == "ci:lint" {
                let j = jobs.get_mut(id).unwrap();
                j.state = "done".into();
                j.exit = Some(if bdy["sha"] == a.as_str() { 1 } else { 0 });
            }
        }
        drop((jobs, bodies));
        tick(&f, 400);
        let rs = runs(&ws);
        assert_eq!((rs[0].state.as_str(), rs[1].state.as_str()), (run_state::FAILED, run_state::PASSED));
        assert_eq!(rs[0].ended_at, Some(400));
        assert!(rs[0].detail.as_deref().unwrap().starts_with("1 passed, 1 failed, 0 error"));
        let lint = checks(&rs[0].id).into_iter().find(|c| c.name == "lint").unwrap();
        assert_eq!((lint.state.as_str(), lint.posted.as_deref()), (check_state::FAILURE, Some("failure")));
        assert!(lint.description.contains("exit 1"), "{}", lint.description);
        let finals: Vec<_> = f.posts().into_iter().skip(4).collect();
        assert_eq!(finals.len(), 4, "{finals:?}");
        assert!(finals.iter().any(|p| p.1 == "k2-ci/lint" && p.2 == "failure" && p.0.ends_with(&a[..7])));
        assert_eq!(finals.iter().filter(|p| p.2 == "success").count(), 3);
    }

    #[test]
    fn the_cap_holds_tips_until_a_run_ends() {
        let _db = k2_core::db::scoped_for_test();
        let f = Fake::default();
        let ws = seed_ws();
        watch(&ws, 1);
        let (a, b) = (sha('a'), sha('b'));
        *f.tips.lock().unwrap() = format!("{a}\trefs/heads/main\n{b}\trefs/heads/integ/x\n");
        f.configs.lock().unwrap().insert(a.clone(), "version = 1\n[checks.unit]\nargv = [\"true\"]\n".into());
        f.configs.lock().unwrap().insert(b.clone(), "version = 1\n[checks.unit]\nargv = [\"true\"]\n".into());
        tick(&f, 100);
        assert_eq!(runs(&ws).len(), 1, "max 1 in flight");
        {
            let db = db();
            let conn = db.lock();
            assert!(!ci::is_seen(&conn, &ws.id, &b), "a waiting tip isn't marked seen");
        }
        tick(&f, 130);
        assert_eq!(runs(&ws).len(), 1, "not due yet, and still full");
        f.finish("unit", 0);
        tick(&f, 200);
        let rs = runs(&ws);
        assert_eq!(rs.len(), 2, "the run ended, so the waiting tip starts");
        assert_eq!(rs[0].state, run_state::PASSED);
        assert_eq!(rs[1].sha, b);
        assert!(rs.iter().all(|r| r.statuses.is_none()), "no [statuses]: nothing posted");
        assert!(f.posts().is_empty());
    }

    #[test]
    fn unrunnable_tips_are_recorded_and_refusals_end_the_run() {
        let _db = k2_core::db::scoped_for_test();
        let f = Fake::default();
        let ws = seed_ws();
        watch(&ws, 4);
        let (a, b, c) = (sha('a'), sha('b'), sha('c'));
        *f.tips.lock().unwrap() = format!("{a}\trefs/heads/main\n{b}\trefs/heads/integ/b\n{c}\trefs/heads/integ/c\n");
        // a: no config. b: a bad one. c: its check can't be submitted.
        f.configs.lock().unwrap().insert(b.clone(), "version = 1\nbogus = 1\n".into());
        f.configs.lock().unwrap().insert(c.clone(), CFG.into());
        *f.slug.lock().unwrap() = Some("acme/app".into());
        f.refuse.lock().unwrap().insert("ci:unit".into(), ("ci_not_allowed".into(), "the grant lacks --ci".into()));
        tick(&f, 100);
        let rs = runs(&ws);
        assert_eq!(rs.len(), 3);
        assert_eq!(rs[0].state, run_state::SKIPPED);
        assert!(rs[0].detail.as_deref().unwrap().contains("ci_no_config"), "{:?}", rs[0].detail);
        assert_eq!(rs[1].state, run_state::ERROR);
        assert!(rs[1].detail.as_deref().unwrap().contains("ci_config_invalid"));
        assert!(rs[2].is_running(), "lint was submitted");
        let unit = checks(&rs[2].id).into_iter().find(|c| c.name == "unit").unwrap();
        assert_eq!(unit.state, check_state::ERROR);
        assert!(unit.description.contains("not submitted: ci_not_allowed: the grant lacks --ci"), "{}", unit.description);
        assert!(f.posts().iter().any(|p| p.1 == "k2-ci/unit" && p.2 == "error"), "a refused check posts error at once");
        // Every check refused: the run ends at once.
        f.refuse.lock().unwrap().insert("ci:lint".into(), ("not_granted".into(), "no grant".into()));
        let d = sha('d');
        f.configs.lock().unwrap().insert(d.clone(), CFG.into());
        *f.tips.lock().unwrap() = format!("{d}\trefs/heads/main\n");
        tick(&f, 200);
        let last = runs(&ws).pop().unwrap();
        assert_eq!((last.sha.as_str(), last.state.as_str()), (d.as_str(), run_state::ERROR));
    }

    #[test]
    fn a_missing_commit_is_fetched_and_ls_remote_failures_are_kept() {
        let _db = k2_core::db::scoped_for_test();
        let f = Fake::default();
        let ws = seed_ws();
        watch(&ws, 2);
        *f.ls_fails.lock().unwrap() = Some("fatal: could not read from remote repository".into());
        tick(&f, 100);
        let row = {
            let db = db();
            let conn = db.lock();
            ci::workspace(&conn, &ws.id).unwrap()
        };
        assert_eq!(row.last_error.as_deref(), Some("fatal: could not read from remote repository"));
        assert!(runs(&ws).is_empty());
        *f.ls_fails.lock().unwrap() = None;
        let a = sha('a');
        f.missing.lock().unwrap().push(a.clone());
        *f.tips.lock().unwrap() = format!("{a}\trefs/heads/main\n");
        f.configs.lock().unwrap().insert(a.clone(), "version = 1\n[checks.unit]\nargv = [\"true\"]\n".into());
        tick(&f, 200);
        assert_eq!(f.fetched.lock().unwrap().clone(), vec!["refs/heads/main".to_string()]);
        assert!(runs(&ws)[0].is_running());
        let row = {
            let db = db();
            let conn = db.lock();
            ci::workspace(&conn, &ws.id).unwrap()
        };
        assert_eq!((row.last_poll_at, row.last_error), (Some(200), None));
    }

    #[test]
    fn supersede_deadline_and_status_retries() {
        let _db = k2_core::db::scoped_for_test();
        let f = Fake::default();
        let ws = seed_ws();
        let a = sha('a');
        f.configs.lock().unwrap().insert(a.clone(), CFG.into());
        *f.slug.lock().unwrap() = Some("acme/app".into());
        *f.post_fails.lock().unwrap() = 1;
        let root = PathBuf::from(&ws.path);
        let s = Start { sha: &a, git_ref: Some("main"), trigger: "manual", by: "owner", tier: "ci", remote: "origin", only: &[] };
        let p = prepare(&f, &ws, &root, &s).unwrap();
        let run = launch(&f, &ws, &s, p, 100);
        let unposted: Vec<CheckRow> = checks(&run.id).into_iter().filter(|c| c.posted.is_none()).collect();
        assert_eq!(unposted.len(), 1, "one pending post failed");
        assert_eq!((unposted[0].post_tries, unposted[0].post_error.as_deref()), (1, Some("GitHub answered 502")));
        tick(&f, 110);
        assert!(checks(&run.id).iter().all(|c| c.posted.as_deref() == Some("pending")), "retried on the next tick");
        // A newer push replaced lint; unit is still queued at the deadline.
        {
            let mut jobs = f.jobs.lock().unwrap();
            for (id, j) in jobs.iter_mut() {
                if id.ends_with("-00") {
                    j.state = "cancelled".into();
                    j.reason = Some("replaced".into());
                }
            }
        }
        tick(&f, 120);
        assert!(runs(&ws)[0].is_running());
        tick(&f, 100 + ci::RUN_DEADLINE_SECS);
        let r = runs(&ws).pop().unwrap();
        assert_eq!(r.state, run_state::ERROR);
        let cs = checks(&r.id);
        let lint = cs.iter().find(|c| c.name == "lint").unwrap();
        assert!(lint.description.contains("superseded by a newer push"), "{}", lint.description);
        let unit = cs.iter().find(|c| c.name == "unit").unwrap();
        assert!(unit.description.contains("still queued at the deadline"), "{}", unit.description);
        assert!(cs.iter().all(|c| c.posted.as_deref() == Some("error")));
        // --check picks; an unknown one names what there is.
        let only = vec!["lint".to_string()];
        let p = prepare(&f, &ws, &root, &Start { only: &only, ..s }).unwrap();
        assert_eq!(p.checks.len(), 1);
        let bad = vec!["nope".to_string()];
        let e = prepare(&f, &ws, &root, &Start { only: &bad, ..s }).err().unwrap();
        assert_eq!(e.code, codes::NO_CHECK);
        assert!(e.hint.contains("lint, unit"), "{}", e.hint);
        // [statuses] with a remote that isn't GitHub: the run says so.
        *f.slug.lock().unwrap() = None;
        let p = prepare(&f, &ws, &root, &s).unwrap();
        assert!(p.statuses.is_none());
        assert!(p.note.unwrap().contains("isn't a github.com repo"));
    }
}
