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
use k2_core::workspace::{scheduler, settings, triage as triage_summary, wake_prompts as wake};
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

/// Handler for `/cli/agents/triage` — read-only summary. Matches
/// pre-Phase-4 Tauri's `k2so_agents_triage_summary` shape. Used
/// by the user-facing `k2so agents triage` verb. Never spawns.
pub fn handle_triage(project_path: &str) -> String {
    triage_summary::triage_summary(project_path)
        .unwrap_or_else(|e| format!("Triage error: {}", e))
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
///
/// Uses `tokio::task::block_in_place` + `Handle::current().block_on`
/// to step into async land from a sync HTTP handler (the daemon
/// runtime is multi-thread, so block_in_place is safe). Inside, a
/// `Semaphore` caps parallelism, `JoinSet` collects the futures,
/// and `tokio::time::timeout` enforces a per-spawn deadline so a
/// hung PTY allocate can't wedge the tick.
///
/// Each smart_launch call is sync (PTY spawn, file I/O, DB writes)
/// so we wrap it in `spawn_blocking` — that runs it on tokio's
/// blocking pool without freezing the multi-thread workers.
fn run_candidates_bounded(
    project_path: &str,
    candidates: Vec<heartbeat::HeartbeatFireCandidate>,
) -> Vec<String> {
    if candidates.is_empty() {
        return Vec::new();
    }

    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async move {
            let sem = Arc::new(Semaphore::new(MAX_PARALLEL_HEARTBEAT_SPAWNS));
            let project_path = Arc::new(project_path.to_string());
            let deadline = Duration::from_secs(DEFAULT_SPAWN_DEADLINE_SECS);

            let mut set: JoinSet<Option<String>> = JoinSet::new();
            for cand in candidates {
                let permit = sem.clone().acquire_owned().await.expect("semaphore closed");
                let project_path = project_path.clone();
                set.spawn(async move {
                    let cand_name = cand.name.clone();
                    let cand_for_log = cand.name.clone();
                    let outcome_fut = spawn_blocking(move || {
                        crate::heartbeat_launch::smart_launch(&project_path, &cand_name)
                    });
                    let result = match tokio::time::timeout(deadline, outcome_fut).await {
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
                            // Timeout. The lease will be cleared by the
                            // boot-time sweep next time we restart, or by
                            // an explicit release in P5.5's watchdog.
                            k2_core::log_debug!(
                                "[daemon/scheduler-fire] hb {} timed out after {}s",
                                cand_for_log, DEFAULT_SPAWN_DEADLINE_SECS
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

// `dispatch_wake` retired in 0.37.0 — every fire goes through
// `wake_headless::spawn_wake_headless` (v2). The legacy two-branch
// dispatch (Kessel-T0 if use_stream='on' else in-process Alacritty
// Legacy) has been removed.
