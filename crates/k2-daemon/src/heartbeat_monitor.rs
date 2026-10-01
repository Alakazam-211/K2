//! Heartbeat scheduler loop — the daemon fires heartbeats on its own.
//!
//! Heartbeat S2 (`prd-heartbeat-firing-v1.md`, HB12–HB17): until S2 the
//! ONLY autonomous heartbeat driver was the OS scheduler
//! (`dev.k2.heartbeat` launchd agent / crontab line). On 2026-09-30 a
//! test run left that job loaded from a deleted temp folder and nothing
//! fired for ~8h; on a stock Arch box there is no `crontab` at all.
//! The daemon now ticks itself, on every OS, headless or not:
//!
//! 1. **Boot scan.** After boot settles, one due scan runs at once, so
//!    a slot missed while the daemon was down catches up (once, within
//!    12 h — D7) without waiting for anything.
//! 2. **60 s tick (HB12).** The same due scan an OS tick drives
//!    (`triage::handle_scheduler_fire`), every 60 s. Each pass runs in
//!    its own task (HB17): a panic is logged, the loop carries on, and a
//!    dead loop shows as a stale `last_daemon_tick_at`. Passes never
//!    overlap; per-project single-flight (HB13) and the lease's due
//!    re-check (HB14) make a leftover OS tick harmless.
//! 3. **Wake (W7).** Every 5 s the loop compares wall-clock time with
//!    monotonic time. A gap over 2 minutes means the machine slept. The
//!    daemon then takes a short keep-awake hold (a scheduled wake may be
//!    a short dark wake), runs the due scan at once, and re-plans the
//!    next wake.
//! 4. **Old OS job.** The launchd job / crontab line is retired (HB15,
//!    W1): checked at boot and every 10 minutes, and removed when found.
//!    `WakeSystem` was never a launchd key; waking is the daemon's job
//!    now (`crate::power`).
//!
//! Escape hatch: `K2_HEARTBEAT_NO_SELF_HEAL=1` skips every OS side
//! effect of this loop (removing the old job, scheduling wake events) —
//! the headless e2e harness and scratch-HOME daemons set it. The due
//! scan itself always runs.

use std::time::{Duration, Instant};

use k2_core::log_debug;

/// How often the loop wakes up. Also the resolution of sleep/wake
/// detection, so a dark wake is noticed within seconds.
const POLL_INTERVAL: Duration = Duration::from_secs(5);

/// HB12 — the daemon's own due-scan cadence.
const TICK_INTERVAL: Duration = Duration::from_secs(60);

/// Wall-vs-monotonic divergence that counts as a sleep. 2 minutes:
/// big enough to never trip on scheduler jitter, small enough that a
/// laptop-lid sleep of any consequence triggers an immediate scan.
const WALL_JUMP_THRESHOLD_SECS: i64 = 120;

/// How often to look for (and remove) the old OS tick job.
const TRANSPORT_CHECK_INTERVAL: Duration = Duration::from_secs(600);

/// Keep-awake hold taken on wake, before the scan: long enough for the
/// scan to start launches, which then hold their own lease-time guard.
const WAKE_HOLD: Duration = Duration::from_secs(120);

/// Spawn the loop. Called once from `async_main` after the boot
/// readiness gate opens (the scan drives the same handlers HTTP ticks
/// do, so the daemon must be fully migrated first).
pub fn spawn() -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(3)).await;
        ensure_transport("boot").await;
        crate::power::boot().await;
        let mut pass = Some(spawn_pass("boot overdue scan"));

        let mut last_mono = Instant::now();
        let mut last_wall = chrono::Utc::now();
        let mut last_tick = Instant::now();
        let mut last_transport_check = Instant::now();
        loop {
            tokio::time::sleep(POLL_INTERVAL).await;

            // Sleep/wake: on suspend the monotonic clock (and this task)
            // pauses with the machine, so wall delta >> mono delta.
            let mono_delta = last_mono.elapsed().as_secs() as i64;
            let wall_delta = (chrono::Utc::now() - last_wall).num_seconds();
            last_mono = Instant::now();
            last_wall = chrono::Utc::now();
            let woke = wall_delta - mono_delta > WALL_JUMP_THRESHOLD_SECS;

            if woke {
                log_debug!(
                    "[daemon/heartbeat-monitor] woke: wall {}s vs monotonic {}s — due scan now",
                    wall_delta,
                    mono_delta
                );
                crate::power::hold_for("heartbeat wake scan", WAKE_HOLD);
            }

            let tick_due = woke || last_tick.elapsed() >= TICK_INTERVAL;
            if tick_due {
                last_tick = Instant::now();
                pass = Some(match pass.take() {
                    // HB17: never overlap passes. A pass still running
                    // (slow spawns) keeps its slot; the next tick retries.
                    Some(running) if !running.is_finished() => {
                        log_debug!("[daemon/heartbeat-monitor] previous pass still running — tick skipped");
                        running
                    }
                    Some(done) => {
                        if let Err(e) = done.await {
                            log_debug!("[daemon/heartbeat-monitor] previous pass panicked: {e}");
                        }
                        spawn_pass(if woke { "wake" } else { "daemon tick" })
                    }
                    None => spawn_pass(if woke { "wake" } else { "daemon tick" }),
                });
            }

            if woke || last_transport_check.elapsed() >= TRANSPORT_CHECK_INTERVAL {
                last_transport_check = Instant::now();
                ensure_transport(if woke { "wake" } else { "periodic" }).await;
            }
        }
    })
}

/// HB17 — one due scan in its own task, then a wake re-plan. A panic
/// inside surfaces as the handle's `JoinError` and is logged by the loop.
fn spawn_pass(reason: &'static str) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        run_due_scan(reason).await;
        crate::power::replan_wake(reason).await;
    })
}

/// Run the full due-evaluation over every project with enabled
/// heartbeats — the same `handle_scheduler_fire` an external tick
/// drives, so gap detection, catch-up, windows, and backoff all apply
/// identically. Public for the integration tests.
pub async fn run_due_scan(reason: &str) {
    // HB9: stamps `last_daemon_tick_at`, never the OS key — a daemon
    // restart must not make a dead OS job look alive.
    let paths: Vec<String> =
        match tokio::task::spawn_blocking(crate::triage::daemon_scan_project_paths).await {
            Ok(p) => p,
            Err(e) => {
                log_debug!("[daemon/heartbeat-monitor] project list join error: {e}");
                return;
            }
        };
    if paths.is_empty() {
        return;
    }
    log_debug!(
        "[daemon/heartbeat-monitor] {} — evaluating {} project(s)",
        reason,
        paths.len()
    );
    for p in paths {
        // handle_scheduler_fire is sync (block_in_place inside) —
        // run it on the blocking pool so this task never starves the
        // runtime workers.
        let result = tokio::task::spawn_blocking(move || {
            crate::triage::handle_scheduler_fire(&p)
        })
        .await;
        if let Err(e) = result {
            log_debug!("[daemon/heartbeat-monitor] scan join error: {e}");
        }
    }
}

/// HB15 / W1 — find the old OS tick job (launchd `dev.k2.heartbeat` /
/// the `k2so-agent-heartbeat` crontab line) and remove it. The daemon
/// ticks itself now; the job adds nothing. The k2-core guard (HB6)
/// still refuses for a scratch-HOME daemon.
async fn ensure_transport(context: &'static str) {
    if std::env::var("K2_HEARTBEAT_NO_SELF_HEAL").map(|v| v == "1").unwrap_or(false) {
        log_debug!(
            "[daemon/heartbeat-monitor] transport check SKIPPED ({context}) — K2_HEARTBEAT_NO_SELF_HEAL=1"
        );
        return;
    }
    // launchctl / crontab calls — keep them off the async workers.
    let joined = tokio::task::spawn_blocking(move || {
        k2_core::heartbeats::install::self_check_and_repair(context)
    })
    .await;
    match joined {
        Ok((report, None)) => {
            log_debug!(
                "[daemon/heartbeat-monitor] transport {:?} ({context}): {}",
                report.state,
                report.detail
            );
        }
        Ok((_report, Some(rec))) => log_debug!(
            "[daemon/heartbeat-monitor] transport {:?} ({context}) → {}: {}",
            rec.before,
            rec.action,
            rec.detail
        ),
        Err(e) => log_debug!("[daemon/heartbeat-monitor] transport check join error: {e}"),
    }
}
