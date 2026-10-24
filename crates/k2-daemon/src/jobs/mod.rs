//! `k2 job` — daemon-run scheduled jobs (prd-vault-gaps-v1 rev 2, item 5).
//!
//! The daemon runs them itself, headless, on Linux and macOS: no OS cron,
//! no session, no LLM. Each run:
//! - gets its own passport (`job-<run id>`, an agent-level principal of the
//!   job's workspace; `session_token::mint_job_passport`), revoked at run
//!   end and at boot;
//! - gets vault env/file delivery at start through the same path sessions
//!   use (`vault::spawn::materialize_job`), for `Requester::Job`: only
//!   grants to `job:<name>` (Q13);
//! - runs argv (no shell) in the workspace folder (or a folder inside it),
//!   in a new process group, with a clean env and a PATH that includes the
//!   `k2` CLI's usual homes; never overlaps itself; has a timeout (SIGTERM,
//!   SIGKILL 10 s later); writes a redacted log capped at 1 MiB.
//!
//! The store and the due evaluator are `k2_core::jobs` (migration 0220).

pub mod routes;
mod runner;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use k2_core::jobs::{self as store, Due, Job};
use k2_core::log_debug;
use parking_lot::Mutex;

pub use runner::start_run;

/// How often the loop looks for due jobs.
const TICK: Duration = Duration::from_secs(15);
/// Runs at once, daemon-wide; a due slot over the cap is `skipped_busy`.
pub const MAX_CONCURRENT_RUNS: usize = 16;

/// Why a live run is being stopped.
pub(crate) const STOP_NONE: u8 = 0;
pub(crate) const STOP_KILL: u8 = 1;

/// A run in flight.
pub(crate) struct Live {
    pub job_id: String,
    pub run_id: String,
    pub workspace_id: String,
    pub name: String,
    pub session_id: String,
    pub stop: AtomicU8,
}

/// The job behind a live passport (for `VaultCaller::requester`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveRef {
    pub workspace_id: String,
    pub job_id: String,
    pub name: String,
    pub run_id: String,
}

/// job id → its live run.
static LIVE: Mutex<Option<HashMap<String, Arc<Live>>>> = Mutex::new(None);

fn with_live<R>(f: impl FnOnce(&mut HashMap<String, Arc<Live>>) -> R) -> R {
    let mut g = LIVE.lock();
    f(g.get_or_insert_with(HashMap::new))
}

/// The live run a job passport belongs to (`job-<run id>`), if any.
pub fn live_run_for_session(sid: &str) -> Option<LiveRef> {
    with_live(|m| {
        m.values().find(|l| l.session_id == sid).map(|l| LiveRef {
            workspace_id: l.workspace_id.clone(),
            job_id: l.job_id.clone(),
            name: l.name.clone(),
            run_id: l.run_id.clone(),
        })
    })
}

/// The running run's id for a job.
pub fn running_run(job_id: &str) -> Option<String> {
    with_live(|m| m.get(job_id).map(|l| l.run_id.clone()))
}

/// `k2 job kill`: stop the job's running run (SIGTERM, SIGKILL after 10 s).
pub fn kill(job_id: &str) -> Option<String> {
    with_live(|m| {
        m.get(job_id).map(|l| {
            l.stop.store(STOP_KILL, Ordering::SeqCst);
            l.run_id.clone()
        })
    })
}

pub(crate) fn live_count() -> usize {
    with_live(|m| m.len())
}

/// Claim the job's single-flight slot. `None` when a run is in flight.
pub(crate) fn claim(live: Live) -> Option<Arc<Live>> {
    with_live(|m| {
        if m.contains_key(&live.job_id) {
            return None;
        }
        let l = Arc::new(live);
        m.insert(l.job_id.clone(), l.clone());
        Some(l)
    })
}

pub(crate) fn release(job_id: &str, run_id: &str) {
    with_live(|m| {
        if m.get(job_id).is_some_and(|l| l.run_id == run_id) {
            m.remove(job_id);
        }
    });
}

/// Boot: runs the last daemon left `running` are `interrupted`, their
/// passports are revoked, and (Linux) their leftover process groups are
/// stopped when they are provably ours.
pub fn boot() {
    let revoked = crate::session_token::revoke_job_passports_at_boot();
    let db = k2_core::db::shared();
    let rows = {
        let conn = db.lock();
        match store::mark_interrupted(&conn, Utc::now()) {
            Ok(r) => r,
            Err(e) => {
                log_debug!("[jobs] boot: {e}");
                return;
            }
        }
    };
    for r in &rows {
        if let Some(pid) = r.pid {
            runner::stop_leftover_group(pid, &r.id);
        }
    }
    if revoked > 0 || !rows.is_empty() {
        log_debug!("[jobs] boot: {} run(s) interrupted, {revoked} passport(s) revoked", rows.len());
    }
}

/// The scheduler loop (spawned after the readiness gate, like heartbeats).
pub fn spawn_loop() -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let mut t = tokio::time::interval(TICK);
        t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            t.tick().await;
            if let Err(e) = tokio::task::spawn_blocking(tick).await {
                log_debug!("[jobs] tick panicked: {e}");
            }
        }
    })
}

/// One due scan.
pub fn tick() {
    let now = Utc::now();
    let jobs: Vec<Job> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match store::paused(&conn) {
            Ok((true, _, _)) => return,
            Ok(_) => {}
            Err(e) => {
                log_debug!("[jobs] tick: {e}");
                return;
            }
        }
        match store::list(&conn, None) {
            Ok(j) => j,
            Err(e) => {
                log_debug!("[jobs] tick: {e}");
                return;
            }
        }
    };
    for job in jobs.into_iter().filter(|j| j.enabled) {
        match store::evaluate(&job.schedule, job.anchor(), now) {
            Due::NotYet { .. } => {}
            Due::Invalid { reason } => log_debug!("[jobs] {} ({}): {reason}", job.name, job.workspace_id),
            Due::SkipMissed { slot } => {
                let db = k2_core::db::shared();
                let conn = db.lock();
                let _ = store::set_anchor(&conn, &job.id, slot);
                let _ = store::insert_run(&conn, &new_run_id(), &job.id, "skipped_missed", "schedule", None, "daemon", now);
            }
            Due::Fire { slot, catchup } => {
                {
                    let db = k2_core::db::shared();
                    let conn = db.lock();
                    if let Err(e) = store::set_anchor(&conn, &job.id, slot) {
                        log_debug!("[jobs] anchor {}: {e}", job.name);
                        continue;
                    }
                }
                let trigger = if catchup { "catchup" } else { "schedule" };
                match start_run(&job, trigger, "daemon") {
                    Ok(_) => {}
                    Err(e) if e.code == "job_running" || e.code == "too_many_runs" => {}
                    Err(e) => log_debug!("[jobs] {} ({}): {e}", job.name, job.workspace_id),
                }
            }
        }
    }
}

pub(crate) fn new_run_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
