//! Heartbeat reliability monitor — boot overdue scan, wall-clock-jump
//! detection, and tick-transport self-heal.
//!
//! The misfire study established that the ONLY autonomous heartbeat
//! driver is the OS scheduler (`dev.k2.heartbeat` launchd agent /
//! crontab entry). Two consequences it documented empirically:
//!
//! 1. **Recovery after downtime waited for the next external tick.**
//!    A daemon that boots after the laptop was dead through a fire
//!    window did nothing until launchd's next 60s tick — and nothing
//!    EVER if the transport was broken. The boot scan below runs the
//!    due-evaluation immediately, so "kill daemon through the window →
//!    restart" fires the catch-up without depending on the plist.
//!
//! 2. **The transport can die silently for WEEKS** (study bonus
//!    finding: this very box's LaunchAgent was missing for ~3 weeks —
//!    `heartbeat.log`'s last tick 2026-06-10 — while every enabled
//!    heartbeat reported enabled=yes). The self-heal below verifies
//!    the transport exists + is loaded whenever enabled heartbeats
//!    exist, and reinstalls it if not.
//!
//! The monitor loop also compares wall-clock against monotonic time:
//! a jump (sleep, suspend, clock change) triggers an immediate
//! due-evaluation instead of waiting for launchd's post-wake tick —
//! and doubles as the tick-loop drift detector the overhaul asked for.
//!
//! Escape hatch: `K2_HEARTBEAT_NO_SELF_HEAL=1` skips the launchctl /
//! crontab side effects (used by the headless e2e harness, whose
//! scratch-HOME daemon must never bootstrap a real `dev.k2.heartbeat`
//! label into the box's launchd).

use std::time::{Duration, Instant};

use k2_core::log_debug;

/// Monitor cadence. Also the resolution of wall-clock-jump detection.
const MONITOR_INTERVAL: Duration = Duration::from_secs(60);

/// Wall-vs-monotonic divergence that counts as a jump. 2 minutes:
/// big enough to never trip on scheduler jitter, small enough that a
/// laptop-lid sleep of any consequence triggers an immediate scan.
const WALL_JUMP_THRESHOLD_SECS: i64 = 120;

/// How often (in monitor iterations) to re-verify the tick transport.
/// 60 iterations ≈ hourly — cheap (`launchctl print`), and an hour is
/// a short outage compared to the weeks-long silent failure it guards
/// against.
const TRANSPORT_CHECK_EVERY_ITERS: u64 = 60;

/// Spawn the monitor. Called once from `async_main` after the boot
/// readiness gate opens (the scan drives the same handlers HTTP ticks
/// do, so the daemon must be fully migrated first).
pub fn spawn() -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // Boot pass: let boot settle, then verify the transport and
        // run the overdue scan — recovery no longer depends on the
        // plist being alive.
        tokio::time::sleep(Duration::from_secs(3)).await;
        ensure_transport("boot");
        run_due_scan("boot overdue scan").await;

        let mut last_mono = Instant::now();
        let mut last_wall = chrono::Utc::now();
        let mut iter: u64 = 0;
        loop {
            tokio::time::sleep(MONITOR_INTERVAL).await;
            iter += 1;

            // Wall-clock jump vs monotonic: on sleep/suspend the
            // monotonic clock (and this task) pauses with the machine,
            // so wall delta >> mono delta on wake.
            let mono_delta = last_mono.elapsed().as_secs() as i64;
            let wall_delta = (chrono::Utc::now() - last_wall).num_seconds();
            last_mono = Instant::now();
            last_wall = chrono::Utc::now();
            if wall_delta - mono_delta > WALL_JUMP_THRESHOLD_SECS {
                log_debug!(
                    "[daemon/heartbeat-monitor] wall-clock jump: wall {}s vs monotonic {}s — running due-evaluation now",
                    wall_delta,
                    mono_delta
                );
                run_due_scan("wall-clock jump").await;
            }

            if iter % TRANSPORT_CHECK_EVERY_ITERS == 0 {
                ensure_transport("periodic");
            }
        }
    })
}

/// Run the full due-evaluation over every project with enabled
/// heartbeats — the same `handle_scheduler_fire` an external tick
/// drives, so gap detection, catch-up coalescing, windows, and
/// backoff all apply identically.
async fn run_due_scan(reason: &str) {
    let paths: Vec<String> = crate::triage::handle_active_projects()
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
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

/// Verify the tick transport (launchd agent / crontab entry) and
/// reinstall it when enabled heartbeats exist but no transport does.
/// Respects an explicit user opt-out: wake-scheduler mode "off" means
/// the user chose no transport — never fight that.
fn ensure_transport(context: &str) {
    if std::env::var("K2_HEARTBEAT_NO_SELF_HEAL").map(|v| v == "1").unwrap_or(false) {
        log_debug!(
            "[daemon/heartbeat-monitor] transport self-heal SKIPPED ({context}) — K2_HEARTBEAT_NO_SELF_HEAL=1"
        );
        return;
    }

    let enabled_count = count_enabled_heartbeats();
    if enabled_count == 0 {
        return; // nothing scheduled — a missing transport is fine
    }

    let ws = k2_core::app_settings::load().wake_scheduler;
    if ws.mode == "off" {
        // Explicit user choice (Settings → Heartbeats → Mode: Off).
        return;
    }

    if k2_core::heartbeats::install::transport_installed() {
        return;
    }

    log_debug!(
        "[daemon/heartbeat-monitor] {enabled_count} enabled heartbeat(s) but the tick \
         transport is missing/unloaded ({context}) — self-healing"
    );
    // Mode "heartbeat" reinstalls with the user's configured interval +
    // wake_system; anything else gets the same 60s default bootstrap
    // `heartbeat add` performs.
    let result = if ws.mode == "heartbeat" {
        k2_core::heartbeats::install::apply_wake_scheduler(
            &ws.mode,
            ws.interval_minutes,
            ws.wake_system,
        )
        .map(Some)
    } else {
        k2_core::heartbeats::install::ensure_cron_installed()
            .map(|changed| changed.then(|| "transport reinstalled (default 60s interval)".to_string()))
    };
    match result {
        Ok(Some(msg)) => log_debug!("[daemon/heartbeat-monitor] self-heal OK: {msg}"),
        Ok(None) => {}
        Err(e) => log_debug!("[daemon/heartbeat-monitor] self-heal FAILED: {e}"),
    }
}

fn count_enabled_heartbeats() -> i64 {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT COUNT(*) FROM workspace_heartbeats \
         WHERE enabled = 1 AND archived_at IS NULL",
        [],
        |r| r.get(0),
    )
    .unwrap_or(0)
}
