//! Heartbeat S3 — the daemon owns next-fire (prd-heartbeat-firing-v1.md
//! HB18–HB23). Daemon half of `k2_core::heartbeats::wait`.
//!
//! `wait::refresh_project` is the one writer of `next_fire_at` /
//! `wait_reason`. k2-core runs it after the tick and every CRUD write;
//! this module adds what only the daemon can do:
//!
//! - [`install_listener`] — registers the k2-core change listener, so
//!   every refresh that changed a row emits `heartbeat_roster_changed`
//!   (HB23) whoever called it (CRUD, tick, fire, wait pass).
//! - [`FireOutcomeGuard`] — one line at the top of
//!   `heartbeat_launch::smart_launch_checked`. When the launch returns,
//!   by any path (fired, failure/backoff, auto-disable, empty WAKEUP,
//!   lease held), it re-derives the workspace's wait state. A fire that
//!   moved `last_fired` or `enabled` without moving the stored wait
//!   state (a calendar row fired by hand) still emits.
//! - [`spawn`] — the wait pass: once at boot (fills every row, HB18) and
//!   every 60 s after (the overdue watchdog, HB22). Its own task, so it
//!   runs headless and does not depend on the tick it watches.

use std::time::Duration;

use k2_core::heartbeats::wait;
use k2_core::log_debug;

/// Wait-pass cadence. Matches S2's ticker, so an overdue row is noticed
/// within a minute of crossing the 120 s line.
const WAIT_PASS_INTERVAL: Duration = Duration::from_secs(60);

/// Register the k2-core change listener (idempotent). Called by every
/// entry point here, so library users (tests) get events too.
pub fn install_listener() {
    wait::set_change_listener(emit_roster_changed);
}

fn emit_roster_changed(project_path: &str) {
    let project_id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::workspace::agent_identity::resolve_project_id(&conn, project_path)
            .unwrap_or_default()
    };
    crate::session_events::emit_heartbeat_roster_changed(project_path, &project_id);
}

/// Re-derive one workspace's stored wait state; the listener emits when
/// anything changed (HB23). Errors are logged — the next pass retries.
pub fn refresh_and_emit(project_path: &str) -> bool {
    install_listener();
    match wait::refresh_project(project_path) {
        Ok(changed) => changed,
        Err(e) => {
            log_debug!("[daemon/heartbeat-wait] WARN: refresh {project_path}: {e}");
            false
        }
    }
}

/// One wait pass over every workspace with a non-archived heartbeat.
/// Returns how many workspaces changed.
pub fn run_pass() -> usize {
    wait::projects_with_heartbeats()
        .iter()
        .filter(|p| refresh_and_emit(p))
        .count()
}

/// Spawn the wait pass. Boot pass after a short settle (HB18 fill), then
/// every [`WAIT_PASS_INTERVAL`]. Each pass runs on the blocking pool; a
/// panic in one pass is logged and the loop continues.
pub fn spawn() -> tokio::task::JoinHandle<()> {
    install_listener();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(3)).await;
        loop {
            match tokio::task::spawn_blocking(run_pass).await {
                Ok(0) => {}
                Ok(n) => log_debug!("[daemon/heartbeat-wait] pass updated {n} workspace(s)"),
                Err(e) => log_debug!("[daemon/heartbeat-wait] pass join error: {e}"),
            }
            tokio::time::sleep(WAIT_PASS_INTERVAL).await;
        }
    })
}

/// `(last_fired, enabled)` of one row, or `None` when it does not exist.
fn row_marks(project_path: &str, name: &str) -> Option<(Option<String>, bool)> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let pid = k2_core::workspace::agent_identity::resolve_project_id(&conn, project_path)?;
    k2_core::db::schema::AgentHeartbeat::get_by_name(&conn, &pid, name)
        .ok()
        .flatten()
        .map(|hb| (hb.last_fired, hb.enabled))
}

/// Records a fire's outcome when the launch returns (HB19, HB23). Built
/// at the top of `smart_launch_checked`; its `Drop` does the work so
/// every return path is covered by one line in the fire path.
pub struct FireOutcomeGuard {
    project_path: String,
    name: String,
    before: Option<(Option<String>, bool)>,
}

impl FireOutcomeGuard {
    pub fn new(project_path: &str, name: &str) -> Self {
        install_listener();
        let before = if name.is_empty() { None } else { row_marks(project_path, name) };
        FireOutcomeGuard { project_path: project_path.to_string(), name: name.to_string(), before }
    }
}

impl Drop for FireOutcomeGuard {
    fn drop(&mut self) {
        // Never touch the DB while unwinding a panic (a second panic
        // would abort the daemon).
        if std::thread::panicking() {
            return;
        }
        let after = if self.name.is_empty() { None } else { row_marks(&self.project_path, &self.name) };
        let row_moved = after.is_some() && after != self.before;
        // The listener already emitted when the stored state changed.
        let wait_moved = refresh_and_emit(&self.project_path);
        if row_moved && !wait_moved {
            emit_roster_changed(&self.project_path);
        }
    }
}
