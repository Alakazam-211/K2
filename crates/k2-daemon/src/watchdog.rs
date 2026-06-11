//! Host half of the harness watchdog.
//!
//! G1 of Phase 3.2. This module runs the watchdog tokio loop that
//! iterates every live session in the daemon's `session_map`,
//! asks the pure `k2_core::session::watchdog::evaluate()` whether
//! any escalation is due, and — if so — executes the host-specific
//! side effect: logging, writing Ctrl-C to the PTY, killing the
//! child, and emitting a `SemanticEvent::Custom` frame so audit
//! (activity_feed via bus subscribers) records the escalation.
//!
//! The pure decision logic lives in k2so-core so it can be tested
//! without any tokio runtime. See the module-level docs there for
//! the escalation ladder.
//!
//! **Config discovery.** The watchdog reads tunables from
//! environment variables at boot so production deployments + tests
//! can override defaults without code changes:
//!
//! | Var                               | Default |
//! |-----------------------------------|---------|
//! | `K2SO_WATCHDOG_DISABLED=1`        | off     | skip all stages |
//! | `K2SO_WATCHDOG_WARN_SECS=<n>`     | 600     | idle→warning   |
//! | `K2SO_WATCHDOG_CTRL_C_SECS=<n>`   | 1800    | idle→Ctrl-C    |
//! | `K2SO_WATCHDOG_KILL_SECS=<n>`     | 3600    | idle→SIGKILL   |
//! | `K2SO_WATCHDOG_SPAWN_GRACE_SECS=<n>` | 10   | boot grace     |
//! | `K2SO_WATCHDOG_POLL_SECS=<n>`     | 5       | tick interval  |
//!
//! Setting any *_SECS to `0` disables that stage (maps to `None`).
//!
//! **State lifecycle.** Per-session escalation state lives inside
//! the watchdog loop itself, keyed by `SessionId`. When a session
//! drops out of `session_map` (reader thread joined, session
//! dropped) the watchdog prunes its state on the next tick so
//! stale IDs don't accumulate.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::task::JoinHandle;

use k2_core::log_debug;
use k2_core::session::{
    self, watchdog as core_watchdog, Escalation, EscalationState, Frame,
    SemanticKind, SessionEntry, SessionId, WatchdogConfig,
};

use crate::session_lookup::{self, LiveSession};
use crate::v2_session_map;

/// Spawn the watchdog as a background tokio task. Returns the
/// JoinHandle so the daemon can abort it on graceful shutdown (not
/// currently wired; the task exits naturally when the runtime is
/// dropped).
pub fn spawn(config: WatchdogConfig) -> JoinHandle<()> {
    tokio::spawn(async move { run(config).await })
}

/// Load config from environment variables, falling back to
/// `WatchdogConfig::default()` for anything not set. If
/// `K2SO_WATCHDOG_DISABLED=1` the returned config has every
/// threshold cleared (watchdog becomes a no-op).
pub fn config_from_env() -> WatchdogConfig {
    if std::env::var("K2_WATCHDOG_DISABLED").or_else(|_| std::env::var("K2SO_WATCHDOG_DISABLED")).as_deref() == Ok("1") {
        log_debug!("[daemon/watchdog] disabled via K2SO_WATCHDOG_DISABLED=1");
        return WatchdogConfig::disabled();
    }
    let defaults = WatchdogConfig::default();

    fn opt_duration(var: &str, fallback: Option<Duration>) -> Option<Duration> {
        match std::env::var(var) {
            Ok(v) => match v.parse::<u64>() {
                Ok(0) => None,
                Ok(secs) => Some(Duration::from_secs(secs)),
                Err(_) => fallback,
            },
            Err(_) => fallback,
        }
    }
    fn duration(var: &str, fallback: Duration) -> Duration {
        std::env::var(var)
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(Duration::from_secs)
            .unwrap_or(fallback)
    }

    WatchdogConfig {
        warn_after: opt_duration("K2SO_WATCHDOG_WARN_SECS", defaults.warn_after),
        ctrl_c_after: opt_duration(
            "K2SO_WATCHDOG_CTRL_C_SECS",
            defaults.ctrl_c_after,
        ),
        kill_after: opt_duration("K2SO_WATCHDOG_KILL_SECS", defaults.kill_after),
        spawn_grace: duration("K2SO_WATCHDOG_SPAWN_GRACE_SECS", defaults.spawn_grace),
        poll_interval: duration("K2SO_WATCHDOG_POLL_SECS", defaults.poll_interval),
    }
}

async fn run(config: WatchdogConfig) {
    if !config.any_stage_enabled() {
        log_debug!(
            "[daemon/watchdog] every stage disabled — idle loop (set K2SO_WATCHDOG_*_SECS to enable)"
        );
        return;
    }
    log_debug!(
        "[daemon/watchdog] started; warn={:?} ctrl_c={:?} kill={:?} grace={:?} poll={:?}",
        config.warn_after,
        config.ctrl_c_after,
        config.kill_after,
        config.spawn_grace,
        config.poll_interval,
    );

    let mut states: HashMap<SessionId, EscalationState> = HashMap::new();
    let mut interval = tokio::time::interval(config.poll_interval);
    // On first tick we don't want to skip; subsequent ticks should
    // not pile up if the loop body takes longer than poll_interval.
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        interval.tick().await;
        tick(&config, &mut states);
    }
}

/// One watchdog pass. Public so the integration test can drive it
/// deterministically without waiting on real tokio timer ticks.
pub fn tick(
    config: &WatchdogConfig,
    states: &mut HashMap<SessionId, EscalationState>,
) {
    let now = Instant::now();
    let sessions = session_lookup::snapshot_all();

    // Prune state entries for sessions that have exited since the
    // last tick so the map doesn't accumulate dead ids indefinitely.
    let live_ids: HashSet<SessionId> =
        sessions.iter().map(|(_, s)| s.session_id()).collect();
    states.retain(|id, _| live_ids.contains(id));

    for (agent, session) in sessions {
        let session_id = session.session_id();
        let Some(entry) = session::registry::lookup(&session_id) else {
            // The map has this session but registry doesn't.
            // Two reasons we land here:
            //   - Legacy session is being torn down between lookups
            //     (next tick won't see it).
            //   - V2 session: alacritty_v2 doesn't register in
            //     `k2_core::session::registry` at all yet, so the
            //     watchdog has no idle/created_at to consult.
            //     Skipping leaves v2 sessions un-watchdogged for
            //     now — wiring registry-backed activity tracking for
            //     v2 is a follow-up to A9.
            continue;
        };

        // Spawn grace: freeze escalation while the harness is still
        // starting up. Some CLI harnesses (claude especially) take
        // 3-5s before emitting the first frame.
        let age = now.saturating_duration_since(entry.created_at());
        if age < config.spawn_grace {
            continue;
        }

        let idle = entry.idle_for(now);
        let state = states.entry(session_id).or_default();
        let decision = core_watchdog::evaluate(idle, state, config);
        if decision == Escalation::None {
            continue;
        }

        match execute(decision, &agent, &session, &entry, idle) {
            Ok(()) => core_watchdog::mark_fired(state, decision),
            Err(e) => {
                log_debug!(
                    "[daemon/watchdog] execute {:?} for agent={agent} failed: {e} (will retry next tick)",
                    decision
                );
                // Deliberately do NOT mark; next tick retries.
            }
        }
    }
}

/// Run the host-specific side effect for a single escalation
/// stage. On success the caller marks the stage as fired; on
/// failure it's left unmarked so the next tick retries.
///
/// Every stage emits a `SemanticEvent::Custom` frame with kind
/// `watchdog.<stage>` so the Awareness Bus (and downstream audit
/// via activity_feed subscribers) records the escalation.
fn execute(
    stage: Escalation,
    agent: &str,
    session: &LiveSession,
    entry: &Arc<SessionEntry>,
    idle: Duration,
) -> std::io::Result<()> {
    let (kind_name, act_log) = match stage {
        Escalation::None => return Ok(()),
        Escalation::Warn => ("watchdog.idle_warning", "idle threshold crossed; warning"),
        Escalation::CtrlC => ("watchdog.ctrl_c_sent", "sending Ctrl-C (0x03) to PTY"),
        Escalation::Kill => ("watchdog.killed", "killing child process"),
    };

    let session_id = session.session_id();
    log_debug!(
        "[daemon/watchdog] agent={agent} session={} idle={}ms stage={kind_name} — {act_log}",
        session_id,
        idle.as_millis(),
    );

    // Side effect per stage. `Warn` is observation-only.
    match stage {
        Escalation::Warn => {}
        Escalation::CtrlC => {
            session.write(&[0x03])?;
        }
        Escalation::Kill => {
            // Force-kill + reap the child directly — do NOT rely on the
            // Arc-drop → channel-close → single-SIGHUP path. Agent CLIs
            // (claude/codex/…) ignore/outlive that SIGHUP, so a
            // watchdog "kill" that only unregistered would leave the
            // runaway child alive (the very thing the watchdog exists to
            // stop). `DaemonPtySession::kill()` does the two-phase
            // killpg+SIGKILL+waitpid reap. `LiveSession` is a thin
            // newtype over `Arc<DaemonPtySession>`, so reach the inner
            // session via `.0`. Kill first (authoritative termination),
            // then unregister to run the DB/active-session cleanup
            // chokepoint (its own kill() call is then an idempotent
            // no-op).
            session.0.kill();
            v2_session_map::unregister(agent);
        }
        Escalation::None => unreachable!(),
    }

    // Emit the SemanticEvent frame last, after the side effect
    // succeeded. Subscribers (including the archive writer) see
    // escalation events in the same Frame stream as normal text.
    //
    // Use `publish_meta` NOT `publish`: watchdog frames are
    // observer-emitted, not harness activity. Bumping
    // `last_frame_at` on our own Warn emit would reset the idle
    // timer and prevent the ctrl_c/kill ladder from ever firing
    // (we'd keep rewarning a session whose idle clock we kept
    // resetting with the warning frames themselves).
    let payload = serde_json::json!({
        "agent": agent,
        "session_id": session_id.to_string(),
        "idle_ms": idle.as_millis() as u64,
        "stage": kind_name,
    });
    entry.publish_meta(Frame::SemanticEvent {
        kind: SemanticKind::Custom {
            kind: kind_name.to_string(),
            payload: payload.clone(),
        },
        payload,
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Env-var tests mutate process-wide state; serialize them so
    /// parallel execution doesn't see torn config
    /// (e.g. test A sets DISABLED=1; test B reads env mid-mutate
    /// and mistakenly concludes defaults are None). Poison-tolerant
    /// so a panicking test doesn't brick the rest of the suite.
    static ENV_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        ENV_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Clear every watchdog env var the tests touch, defensively —
    /// even other test binaries in the same process don't pollute
    /// our reads. Called at the start of every env-touching test.
    fn clear_watchdog_env() {
        for v in [
            "K2SO_WATCHDOG_DISABLED",
            "K2SO_WATCHDOG_WARN_SECS",
            "K2SO_WATCHDOG_CTRL_C_SECS",
            "K2SO_WATCHDOG_KILL_SECS",
            "K2SO_WATCHDOG_SPAWN_GRACE_SECS",
            "K2SO_WATCHDOG_POLL_SECS",
        ] {
            std::env::remove_var(v);
        }
    }

    #[test]
    fn config_from_env_defaults_with_no_vars() {
        let _g = env_lock();
        clear_watchdog_env();
        let c = config_from_env();
        assert_eq!(c, WatchdogConfig::default());
    }

    #[test]
    fn config_from_env_disabled_flag_disables_everything() {
        let _g = env_lock();
        clear_watchdog_env();
        std::env::set_var("K2SO_WATCHDOG_DISABLED", "1");
        let c = config_from_env();
        std::env::remove_var("K2SO_WATCHDOG_DISABLED");
        assert!(!c.any_stage_enabled());
    }

    #[test]
    fn config_from_env_zero_secs_disables_individual_stage() {
        let _g = env_lock();
        clear_watchdog_env();
        std::env::set_var("K2SO_WATCHDOG_WARN_SECS", "0");
        let c = config_from_env();
        std::env::remove_var("K2SO_WATCHDOG_WARN_SECS");
        assert!(c.warn_after.is_none());
    }
}
