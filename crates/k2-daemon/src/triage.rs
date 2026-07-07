//! Triage + scheduler-fire for the daemon.
//!
//! Two endpoints live here, with DIFFERENT semantics that the
//! pre-Phase-4 Tauri listener also served distinctly:
//!
//! - **`/cli/agents/triage`** → `handle_triage` — read-only
//!   plain-text summary of what's pending in every agent's
//!   inbox, for the user's `k2so agents triage` verb. Matches
//!   the byte-for-byte shape Tauri's agent_hooks returned in
//!   0.33.0. No side effects.
//!
//! - **`/cli/scheduler-tick`** → `handle_scheduler_fire` — the
//!   destructive heartbeat path: runs `scheduler_tick` + the
//!   multi-heartbeat tick, composes each agent's wake prompt,
//!   and spawns a PTY per agent via `spawn_wake_via_session_stream`
//!   (when the project has `use_session_stream='on'`) or the
//!   legacy `spawn_wake_headless` (otherwise). Called by
//!   `~/.k2so/heartbeat.sh` on launchd's schedule.
//!
//! Phase 4 H7 retired Tauri's HTTP listener and pointed every
//! /cli/* route at the daemon. An earlier iteration of this file
//! put the destructive path at `/cli/agents/triage` — which
//! silently changed user-facing `k2so agents triage` from
//! "show me what's pending" to "launch everything that's pending"
//! AND left `/cli/scheduler-tick` returning a bare decision list,
//! which caused `heartbeat.sh` to silently no-op every tick. This
//! module is the post-H7 correction that restores the 0.33.0
//! Tauri route layout.
//!
//! Helpers here are `pub` so integration tests can invoke each
//! handler directly without the HTTP layer.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Semaphore;
use tokio::task::{spawn_blocking, JoinSet};

use k2_core::heartbeats as heartbeat;
use k2_core::workspace::agent_identity::agent_type_for;
use k2_core::workspace::{scheduler, triage as triage_summary, wake_prompts as wake};
use k2_core::db::shared as shared_db;

/// Cap on parallel heartbeat spawns per scheduler tick. Six is a
/// pragmatic middle ground: each spawn allocates ~100MB for a Claude
/// Code child, so 6× peaks at ~600MB which most laptops absorb
/// without paging. Higher caps risk thundering-herd on slow disks
/// when many schedules align (e.g. 9 AM daily).
const MAX_PARALLEL_HEARTBEAT_SPAWNS: usize = 6;

/// Default per-spawn deadline in seconds. Today's PTY allocate +
/// Claude Code boot measures 1-3s; 30s catches truly hung forks
/// while leaving generous headroom. Per-row override comes in P5.5
/// (the `workspace_heartbeats.active_deadline_secs` column already
/// exists from P5.1; this constant is the fallback).
const DEFAULT_SPAWN_DEADLINE_SECS: u64 = 30;

/// Extra time (beyond the spawn deadline) the lease watchdog waits for
/// a timed-out spawn to finish on its own before force-releasing the
/// in-flight lease it is still holding. See `run_candidates_bounded`.
const DEFAULT_LEASE_WATCHDOG_GRACE_SECS: u64 = 90;

/// Spawn deadline with a test/tuning env override
/// (`K2_HEARTBEAT_SPAWN_DEADLINE_SECS`). The override exists so the
/// lease-watchdog integration test can exercise the timeout path in
/// seconds instead of minutes; production leaves it unset.
fn spawn_deadline_secs() -> u64 {
    std::env::var("K2_HEARTBEAT_SPAWN_DEADLINE_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_SPAWN_DEADLINE_SECS)
}

/// Watchdog grace with a test/tuning env override
/// (`K2_HEARTBEAT_LEASE_WATCHDOG_SECS`).
fn lease_watchdog_grace_secs() -> u64 {
    std::env::var("K2_HEARTBEAT_LEASE_WATCHDOG_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_LEASE_WATCHDOG_GRACE_SECS)
}

/// Handler for `/cli/agents/triage` — read-only summary. Matches
/// pre-Phase-4 Tauri's `k2so_agents_triage_summary` shape. Used
/// by the user-facing `k2so agents triage` verb. Never spawns.
pub fn handle_triage(project_path: &str) -> String {
    triage_summary::triage_summary(project_path)
        .unwrap_or_else(|e| format!("Triage error: {}", e))
}

/// A tick gap larger than this (seconds) is audit-worthy: at the 60s
/// default cadence, 5 missing ticks means the machine slept, the
/// daemon was down, or the launchd transport is dead. 5 minutes also
/// tolerates users who set the wake interval to a few minutes.
const TICK_GAP_AUDIT_SECS: i64 = 300;

/// Stamp `scheduler_meta.last_tick_at = now` and return the gap since
/// the previous tick when it exceeds [`TICK_GAP_AUDIT_SECS`]. The
/// persisted stamp is what makes "the scheduler is not ticking"
/// observable at all (misfire study fragility #12 — the transport can
/// die silently for weeks); the returned gap feeds `tick_gap` audit
/// rows so downtime shows up in the fire history next to the catch-up
/// fires it caused.
fn note_tick_and_detect_gap() -> Option<i64> {
    let db = shared_db();
    let conn = db.lock();
    let now = chrono::Utc::now();
    let prev = k2_core::db::schema::SchedulerMeta::get(
        &conn,
        k2_core::db::schema::SchedulerMeta::LAST_TICK_AT,
    )
    .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
    .map(|t| t.with_timezone(&chrono::Utc));
    if let Err(e) = k2_core::db::schema::SchedulerMeta::set(
        &conn,
        k2_core::db::schema::SchedulerMeta::LAST_TICK_AT,
        &now.to_rfc3339(),
    ) {
        k2_core::log_debug!("[daemon/scheduler-fire] WARN: persist last_tick_at: {e}");
    }
    let gap = (now - prev?).num_seconds();
    (gap > TICK_GAP_AUDIT_SECS).then_some(gap)
}

/// Write a `tick_gap` audit row for one project. Reason spells out the
/// three possible causes so "why didn't it run?" is answerable from
/// the fire history alone.
fn write_tick_gap_audit(project_path: &str, gap_secs: i64) {
    let db = shared_db();
    let conn = db.lock();
    let Some(project_id) =
        k2_core::workspace::agent_identity::resolve_project_id(&conn, project_path)
    else {
        return;
    };
    let _ = k2_core::db::schema::HeartbeatFire::insert_with_schedule(
        &conn,
        &project_id,
        None,
        None,
        "tick",
        "tick_gap",
        Some(&format!(
            "no scheduler tick for {gap_secs}s — machine asleep, daemon down, \
             or heartbeat transport dead. Missed occurrences catch up now."
        )),
        None,
        None,
        None,
    );
}

/// Handler for `/cli/heartbeat/active-projects` — newline-delimited
/// list of project paths with at least one enabled, non-archived
/// `workspace_heartbeats` row. Replaces the static
/// `~/.k2so/heartbeat-projects.txt` file (which went stale because
/// nothing kept it in sync with workspace creation/deletion).
///
/// heartbeat.sh calls this once per cron tick and iterates the
/// response. Plain text (not JSON) so bash can `while read` without
/// a JSON parser dependency.
///
/// Order: alphabetical by path, for deterministic test output.
pub fn handle_active_projects() -> String {
    let db = shared_db();
    let conn = db.lock();
    let mut stmt = match conn.prepare(
        "SELECT DISTINCT p.path FROM workspace_heartbeats h \
         JOIN projects p ON h.project_id = p.id \
         WHERE h.enabled = 1 AND h.archived_at IS NULL \
         ORDER BY p.path",
    ) {
        Ok(s) => s,
        Err(_) => return String::new(),
    };
    let rows = match stmt.query_map([], |row| row.get::<_, String>(0)) {
        Ok(r) => r,
        Err(_) => return String::new(),
    };
    let paths: Vec<String> = rows.filter_map(|r| r.ok()).collect();
    drop(stmt);
    drop(conn);

    // heartbeat.sh calls this once per cron tick, BEFORE the per-project
    // scheduler-tick calls — the natural once-per-tick spot to stamp
    // `last_tick_at` and detect gaps. A detected gap writes one
    // `tick_gap` row per active project so every affected workspace's
    // history explains its upcoming catch-up fires.
    if let Some(gap) = note_tick_and_detect_gap() {
        k2_core::log_debug!(
            "[daemon/scheduler-fire] tick gap detected: {gap}s since previous tick"
        );
        for p in &paths {
            write_tick_gap_audit(p, gap);
        }
    }

    paths.join("\n")
}

/// Handler for `/cli/scheduler-tick` — the destructive heartbeat
/// path. Returns `{"count":N, "launched":[…], "heartbeats":[…]}`.
///
/// Side-effects in order:
///   1. `scheduler_tick` → list of launchable agent names.
///   2. For each, compose the wake prompt per agent type. Agents
///      whose type has no shipped template (e.g. `agent-template`)
///      never wake autonomously — skipped.
///   3. Spawn via Session Stream or legacy PTY based on
///      `use_session_stream`.
///   4. Multi-heartbeat tick — each candidate carries its own
///      wakeup path; lock-gated per agent.
///   5. `stamp_heartbeat_fired` on the ones that actually spawned.
///   6. Return the count for `heartbeat.sh` to parse.
pub fn handle_scheduler_fire(project_path: &str) -> String {
    // Per-project ticks that DIDN'T come through active-projects
    // (manual "tick now", boot overdue scan, direct curl) still stamp
    // the tick and surface a gap for this project. When heartbeat.sh
    // drove the tick, active-projects stamped seconds ago → no-op here.
    if let Some(gap) = note_tick_and_detect_gap() {
        write_tick_gap_audit(project_path, gap);
    }

    // Belt-and-suspenders for the lease watchdog: sweep any in-flight
    // lease older than 5 minutes at the top of every tick (one cheap
    // conditional UPDATE), so a wedged row recovers within a tick even
    // if the watchdog task itself died with a daemon restart. The boot
    // sweep (main.rs) remains as the third layer.
    {
        let db = shared_db();
        let conn = db.lock();
        match k2_core::db::schema::AgentHeartbeat::sweep_stale_leases(&conn, 300) {
            Ok(0) => {}
            Ok(n) => k2_core::log_debug!(
                "[daemon/scheduler-fire] tick sweep released {} stale lease(s)",
                n
            ),
            Err(e) => k2_core::log_debug!(
                "[daemon/scheduler-fire] WARN: tick lease sweep: {e}"
            ),
        }
    }

    let launchable = match scheduler::k2so_agents_scheduler_tick(project_path.to_string()) {
        Ok(v) => v,
        Err(e) => {
            return serde_json::json!({
                "error": e,
                "count": 0,
                "launched": [],
                "heartbeats": [],
            })
            .to_string()
        }
    };

    // 0.37.0: every daemon-driven heartbeat fire goes through v2.
    // The `use_session_stream` branching (legacy → Kessel-T0 if 'on'
    // else in-process Alacritty Legacy if 'off') is retired —
    // `wake_headless::spawn_wake_headless` always spawns a
    // DaemonPtySession via spawn_agent_session_v2_blocking. The
    // `projects.use_session_stream` column becomes vestigial; safe
    // to drop in a follow-up SQL cleanup.
    let mut launched: Vec<String> = Vec::new();
    for agent_name in &launchable {
        // Dispatch by agent type, not by name: manager-mode primaries
        // use the workspace wake composer (heartbeat row at the
        // workspace level); other agents use per-agent WAKEUP.md.
        let prompt = if agent_type_for(project_path, agent_name) == "manager" {
            wake::compose_wake_prompt_for_workspace(project_path)
        } else {
            match wake::compose_wake_prompt_for_agent(project_path, agent_name) {
                Some(p) => p,
                None => continue, // agent-template: never wakes autonomously
            }
        };
        // Workspace-manager / non-multi-heartbeat path — no specific heartbeat row.
        let result = crate::wake_headless::spawn_wake_headless(
            agent_name,
            project_path,
            &prompt,
            None,
        );
        match result {
            Ok(_tid) => launched.push(agent_name.clone()),
            Err(e) => {
                k2_core::log_debug!(
                    "[daemon/scheduler-fire] spawn failed for {}: {}",
                    agent_name,
                    e
                );
            }
        }
    }

    let candidates = heartbeat::k2so_agents_heartbeat_tick(project_path);
    // Cron tick uses the same smart_launch decision tree the Launch
    // button + `k2so heartbeat launch` CLI use. The decision tree
    // (fresh / inject-into-live / resume-and-fire) handles
    // per-heartbeat session resume + audit row writing.
    //
    // Spawning is bounded-concurrent (P5.4): up to
    // MAX_PARALLEL_HEARTBEAT_SPAWNS run in parallel, the rest queue.
    // Each call gets DEFAULT_SPAWN_DEADLINE_SECS to complete before
    // tokio::time::timeout fires — protects the tick from a hung
    // PTY allocate. The CAS in smart_launch (P5.2) ensures two
    // overlapping ticks don't double-fire the same heartbeat.
    let hb_fired = run_candidates_bounded(project_path, candidates);

    let mut all = launched.clone();
    all.extend(hb_fired.clone());
    serde_json::json!({
        "count": all.len(),
        "launched": all,
        "heartbeats": hb_fired,
    })
    .to_string()
}

/// Bounded-concurrent fan-out of `smart_launch` over the heartbeat
/// candidates. Returns the names that fired successfully.
fn run_candidates_bounded(
    project_path: &str,
    candidates: Vec<heartbeat::HeartbeatFireCandidate>,
) -> Vec<String> {
    run_candidates_bounded_with(project_path, candidates, |pp, cand| {
        crate::heartbeat_launch::smart_launch_with_origin(
            pp,
            &cand.name,
            cand.catchup_of.as_deref(),
        )
    })
}

/// [`run_candidates_bounded`] with the launcher injected — production
/// passes `smart_launch_with_origin`; the lease-watchdog integration
/// test passes a deliberately-hanging closure so the timeout path is
/// exercisable without a real hung PTY allocate.
///
/// Uses `tokio::task::block_in_place` + `Handle::current().block_on`
/// to step into async land from a sync HTTP handler (the daemon
/// runtime is multi-thread, so block_in_place is safe). Inside, a
/// `Semaphore` caps parallelism, `JoinSet` collects the futures,
/// and `tokio::time::timeout` enforces a per-spawn deadline so a
/// hung PTY allocate can't wedge the tick.
///
/// Each launch call is sync (PTY spawn, file I/O, DB writes) so we
/// wrap it in `spawn_blocking` — that runs it on tokio's blocking
/// pool without freezing the multi-thread workers.
///
/// **Timeout path (wedge fix, misfire study §1.6.2):** a spawn that
/// outlives the deadline keeps running on the blocking pool AND keeps
/// holding the row's in-flight lease — pre-overhaul that wedged the
/// heartbeat until the next daemon restart's boot sweep (the "P5.5
/// watchdog" this comment used to promise). Now the timeout arm arms
/// that watchdog: wait one more grace period for the spawn to finish
/// on its own (it then handles its own lease/audit); if it's still
/// hung, force-release the lease age-conditionally and record the
/// attempt as a failure (backoff + auto-disable accounting included).
pub fn run_candidates_bounded_with<F>(
    project_path: &str,
    candidates: Vec<heartbeat::HeartbeatFireCandidate>,
    launch: F,
) -> Vec<String>
where
    F: Fn(&str, &heartbeat::HeartbeatFireCandidate) -> serde_json::Value
        + Send
        + Sync
        + Clone
        + 'static,
{
    if candidates.is_empty() {
        return Vec::new();
    }

    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async move {
            let sem = Arc::new(Semaphore::new(MAX_PARALLEL_HEARTBEAT_SPAWNS));
            let project_path = Arc::new(project_path.to_string());
            let deadline = Duration::from_secs(spawn_deadline_secs());

            let mut set: JoinSet<Option<String>> = JoinSet::new();
            for cand in candidates {
                let permit = sem.clone().acquire_owned().await.expect("semaphore closed");
                let project_path = project_path.clone();
                let launch = launch.clone();
                set.spawn(async move {
                    let cand_for_log = cand.name.clone();
                    let started = std::time::Instant::now();
                    let launch_project = project_path.clone();
                    let mut handle = spawn_blocking(move || launch(&launch_project, &cand));
                    // `&mut handle` so the JoinHandle survives a timeout
                    // and can be handed to the watchdog below.
                    let result = match tokio::time::timeout(deadline, &mut handle).await {
                        Ok(Ok(v)) => v,
                        Ok(Err(e)) => {
                            k2_core::log_debug!(
                                "[daemon/scheduler-fire] hb {} join error: {}",
                                cand_for_log, e
                            );
                            drop(permit);
                            return None;
                        }
                        Err(_) => {
                            k2_core::log_debug!(
                                "[daemon/scheduler-fire] hb {} timed out after {}s — lease watchdog armed",
                                cand_for_log, deadline.as_secs()
                            );
                            spawn_lease_watchdog(
                                project_path.to_string(),
                                cand_for_log,
                                handle,
                                started,
                            );
                            drop(permit);
                            return None;
                        }
                    };
                    drop(permit);
                    if result.get("success").and_then(|v| v.as_bool()).unwrap_or(false) {
                        Some(cand_for_log)
                    } else {
                        let decision = result.get("decision").and_then(|v| v.as_str()).unwrap_or("?");
                        let reason = result.get("reason").and_then(|v| v.as_str()).unwrap_or("");
                        k2_core::log_debug!(
                            "[daemon/scheduler-fire] hb {} decision={} reason={}",
                            cand_for_log, decision, reason
                        );
                        None
                    }
                });
            }

            let mut fired = Vec::new();
            while let Some(res) = set.join_next().await {
                if let Ok(Some(name)) = res {
                    fired.push(name);
                }
            }
            fired
        })
    })
}

/// Watchdog for a spawn that outlived its deadline. Races the still-
/// running spawn against one grace period:
///
/// - Spawn finishes first → it already handled its own lease + audit
///   (success stamps, failure records); just log.
/// - Grace expires first → the spawn is genuinely hung. Force-release
///   the in-flight lease it is holding via the age-conditional
///   `release_lease_if_stale` — the age bound is the attempt's REAL
///   elapsed time minus a second, so a lease re-acquired by any later
///   attempt is never clobbered — and record a failed fire-attempt.
fn spawn_lease_watchdog(
    project_path: String,
    hb_name: String,
    mut handle: tokio::task::JoinHandle<serde_json::Value>,
    started: std::time::Instant,
) {
    tokio::spawn(async move {
        let grace = Duration::from_secs(lease_watchdog_grace_secs());
        tokio::select! {
            res = &mut handle => {
                k2_core::log_debug!(
                    "[daemon/scheduler-fire] hb {} finished late after {}s (result present: {}) — no lease release needed",
                    hb_name,
                    started.elapsed().as_secs(),
                    res.is_ok(),
                );
            }
            _ = tokio::time::sleep(grace) => {
                let min_age = (started.elapsed().as_secs() as i64 - 1).max(1);
                let pp = project_path.clone();
                let name = hb_name.clone();
                let released = spawn_blocking(move || {
                    crate::heartbeat_launch::force_release_hung_lease(&pp, &name, min_age)
                })
                .await
                .unwrap_or(false);
                k2_core::log_debug!(
                    "[daemon/scheduler-fire] hb {} still hung after {}s — watchdog release: {}",
                    hb_name,
                    started.elapsed().as_secs(),
                    released,
                );
            }
        }
    });
}

// `dispatch_wake` retired in 0.37.0 — every fire goes through
// `wake_headless::spawn_wake_headless` (v2). The legacy two-branch
// dispatch (Kessel-T0 if use_stream='on' else in-process Alacritty
// Legacy) has been removed.
