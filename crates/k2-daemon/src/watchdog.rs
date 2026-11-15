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
//! The pure decision logic lives in k2-core so it can be tested
//! without any tokio runtime. See the module-level docs there for
//! the escalation ladder.
//!
//! **Config discovery.** The watchdog reads tunables from
//! environment variables at boot so production deployments + tests
//! can override defaults without code changes:
//!
//! | Var                               | Default |
//! |-----------------------------------|---------|
//! | `K5_WATCHDOG_DISABLED=1`          | off     | skip all stages |
//! | `K5_WATCHDOG_WARN_SECS=<n>`       | 600     | idle→warning   |
//! | `K5_WATCHDOG_CTRL_C_SECS=<n>`     | 1800    | idle→Ctrl-C    |
//! | `K5_WATCHDOG_KILL_SECS=<n>`       | 3600    | idle→SIGKILL   |
//! | `K5_WATCHDOG_SPAWN_GRACE_SECS=<n>` | 10     | boot grace     |
//! | `K5_WATCHDOG_POLL_SECS=<n>`       | 5       | tick interval  |
//!
//! Setting any *_SECS to `0` disables that stage (maps to `None`).
//!
//! Each is read under its `K5_` name first, then under the old
//! `K2SO_` name (and `K2_WATCHDOG_DISABLED`, which exists too). The first
//! name that is set wins, whatever its value.
//!
//! **State lifecycle.** Per-session escalation state lives inside
//! the watchdog loop itself, keyed by `SessionId`. When a session
//! drops out of `session_map` (reader thread joined, session
//! dropped) the watchdog prunes its state on the next tick so
//! stale IDs don't accumulate.

use k2_core::env_names::first_set;
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

/// The names one watchdog setting is read under, newest first.
const DISABLED_NAMES: &[&str] = &["K5_WATCHDOG_DISABLED", "K2_WATCHDOG_DISABLED", "K2SO_WATCHDOG_DISABLED"];
const WARN_NAMES: &[&str] = &["K5_WATCHDOG_WARN_SECS", "K2SO_WATCHDOG_WARN_SECS"];
const CTRL_C_NAMES: &[&str] = &["K5_WATCHDOG_CTRL_C_SECS", "K2SO_WATCHDOG_CTRL_C_SECS"];
const KILL_NAMES: &[&str] = &["K5_WATCHDOG_KILL_SECS", "K2SO_WATCHDOG_KILL_SECS"];
const SPAWN_GRACE_NAMES: &[&str] = &["K5_WATCHDOG_SPAWN_GRACE_SECS", "K2SO_WATCHDOG_SPAWN_GRACE_SECS"];
const POLL_NAMES: &[&str] = &["K5_WATCHDOG_POLL_SECS", "K2SO_WATCHDOG_POLL_SECS"];

/// Load config from environment variables, falling back to
/// `WatchdogConfig::default()` for anything not set. If the disabled
/// switch is `1` the returned config has every threshold cleared
/// (watchdog becomes a no-op).
pub fn config_from_env() -> WatchdogConfig {
    if let Some((name, v)) = first_set(DISABLED_NAMES) {
        if v == "1" {
            log_debug!("[daemon/watchdog] disabled via {name}=1");
            return WatchdogConfig::disabled();
        }
    }
    let defaults = WatchdogConfig::default();

    fn opt_duration(names: &[&'static str], fallback: Option<Duration>) -> Option<Duration> {
        match first_set(names) {
            Some((_, v)) => match v.parse::<u64>() {
                Ok(0) => None,
                Ok(secs) => Some(Duration::from_secs(secs)),
                Err(_) => fallback,
            },
            None => fallback,
        }
    }
    fn duration(names: &[&'static str], fallback: Duration) -> Duration {
        first_set(names)
            .and_then(|(_, v)| v.parse::<u64>().ok())
            .map(Duration::from_secs)
            .unwrap_or(fallback)
    }

    WatchdogConfig {
        warn_after: opt_duration(WARN_NAMES, defaults.warn_after),
        ctrl_c_after: opt_duration(CTRL_C_NAMES, defaults.ctrl_c_after),
        kill_after: opt_duration(KILL_NAMES, defaults.kill_after),
        spawn_grace: duration(SPAWN_GRACE_NAMES, defaults.spawn_grace),
        poll_interval: duration(POLL_NAMES, defaults.poll_interval),
    }
}

async fn run(config: WatchdogConfig) {
    if !config.any_stage_enabled() {
        log_debug!(
            "[daemon/watchdog] every stage disabled — idle loop (set K5_WATCHDOG_*_SECS to enable)"
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
            //   - V2 session: Kessel doesn't register in
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
    use k2_core::test_env::EnvVar;

    // Env-var tests mutate process-wide state, so they hold the ONE shared
    // env lock (through the `EnvVar` guards) for their whole duration, and
    // parallel execution never sees torn config (e.g. test A sets
    // DISABLED=1; test B reads env mid-mutate and mistakenly concludes
    // defaults are None). The lock never poisons, and every guard restores
    // the previous value on drop, even on panic.

    /// Clear every watchdog env var the tests touch, defensively — even
    /// other tests in the same process don't pollute our reads. Called at
    /// the start of every env-touching test; bind the returned guards for
    /// the test's duration.
    #[must_use = "the vars are restored when the guards drop"]
    fn clear_watchdog_env() -> Vec<EnvVar> {
        [DISABLED_NAMES, WARN_NAMES, CTRL_C_NAMES, KILL_NAMES, SPAWN_GRACE_NAMES, POLL_NAMES]
            .into_iter()
            .flatten()
            .copied()
            .map(EnvVar::remove)
            .collect()
    }

    /// Every setting has a `K5_` name, read first, and keeps its old one.
    #[test]
    fn every_setting_is_read_under_k5_first_and_the_old_name_last() {
        for names in [DISABLED_NAMES, WARN_NAMES, CTRL_C_NAMES, KILL_NAMES, SPAWN_GRACE_NAMES, POLL_NAMES] {
            let new = names.first().expect("a name");
            let old = names.last().expect("a name");
            assert!(new.starts_with("K5_WATCHDOG_"), "{new}");
            assert!(old.starts_with("K2SO_WATCHDOG_"), "{old}");
            assert_eq!(new.trim_start_matches("K5_"), old.trim_start_matches("K2SO_"), "the two names differ only in the prefix");
        }
    }

    #[test]
    fn the_k5_names_set_each_value() {
        let _env = clear_watchdog_env();
        let _a = EnvVar::set("K5_WATCHDOG_WARN_SECS", "11");
        let _b = EnvVar::set("K5_WATCHDOG_CTRL_C_SECS", "22");
        let _c = EnvVar::set("K5_WATCHDOG_KILL_SECS", "33");
        let _d = EnvVar::set("K5_WATCHDOG_SPAWN_GRACE_SECS", "4");
        let _e = EnvVar::set("K5_WATCHDOG_POLL_SECS", "2");
        let c = config_from_env();
        assert_eq!(c.warn_after, Some(Duration::from_secs(11)));
        assert_eq!(c.ctrl_c_after, Some(Duration::from_secs(22)));
        assert_eq!(c.kill_after, Some(Duration::from_secs(33)));
        assert_eq!(c.spawn_grace, Duration::from_secs(4));
        assert_eq!(c.poll_interval, Duration::from_secs(2));
    }

    #[test]
    fn the_old_names_still_set_each_value() {
        let _env = clear_watchdog_env();
        let _a = EnvVar::set("K2SO_WATCHDOG_WARN_SECS", "11");
        let _b = EnvVar::set("K2SO_WATCHDOG_CTRL_C_SECS", "22");
        let _c = EnvVar::set("K2SO_WATCHDOG_KILL_SECS", "33");
        let _d = EnvVar::set("K2SO_WATCHDOG_SPAWN_GRACE_SECS", "4");
        let _e = EnvVar::set("K2SO_WATCHDOG_POLL_SECS", "2");
        let c = config_from_env();
        assert_eq!(c.warn_after, Some(Duration::from_secs(11)));
        assert_eq!(c.ctrl_c_after, Some(Duration::from_secs(22)));
        assert_eq!(c.kill_after, Some(Duration::from_secs(33)));
        assert_eq!(c.spawn_grace, Duration::from_secs(4));
        assert_eq!(c.poll_interval, Duration::from_secs(2));
    }

    #[test]
    fn the_k5_name_wins_when_both_are_set() {
        let _env = clear_watchdog_env();
        let _old = EnvVar::set("K2SO_WATCHDOG_WARN_SECS", "11");
        let _new = EnvVar::set("K5_WATCHDOG_WARN_SECS", "99");
        // Also when the new name switches the stage off and the old one has a number.
        let _old_kill = EnvVar::set("K2SO_WATCHDOG_KILL_SECS", "33");
        let _new_kill = EnvVar::set("K5_WATCHDOG_KILL_SECS", "0");
        let c = config_from_env();
        assert_eq!(c.warn_after, Some(Duration::from_secs(99)));
        assert!(c.kill_after.is_none());
    }

    #[test]
    fn the_disabled_switch_is_read_under_all_three_names() {
        for name in DISABLED_NAMES {
            let _env = clear_watchdog_env();
            let _on = EnvVar::set(name, "1");
            assert!(!config_from_env().any_stage_enabled(), "{name}=1 turns the watchdog off");
        }
        // A newer name that is set and is not 1 wins over an older 1.
        let _env = clear_watchdog_env();
        let _old = EnvVar::set("K2SO_WATCHDOG_DISABLED", "1");
        let _new = EnvVar::set("K5_WATCHDOG_DISABLED", "0");
        assert!(config_from_env().any_stage_enabled());
    }

    #[test]
    fn config_from_env_defaults_with_no_vars() {
        let _env = clear_watchdog_env();
        let c = config_from_env();
        assert_eq!(c, WatchdogConfig::default());
    }

    #[test]
    fn config_from_env_disabled_flag_disables_everything() {
        let _env = clear_watchdog_env();
        let disabled = EnvVar::set("K2SO_WATCHDOG_DISABLED", "1");
        let c = config_from_env();
        drop(disabled);
        assert!(!c.any_stage_enabled());
    }

    #[test]
    fn config_from_env_zero_secs_disables_individual_stage() {
        let _env = clear_watchdog_env();
        let warn = EnvVar::set("K2SO_WATCHDOG_WARN_SECS", "0");
        let c = config_from_env();
        drop(warn);
        assert!(c.warn_after.is_none());
    }
}
