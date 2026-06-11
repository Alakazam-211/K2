//! Session Stream watchdog — idle-session detection + escalation.
//!
//! G1 of Phase 3.2. This module is the *decision primitive*: given
//! how long a session has been idle plus which escalation stages
//! have already fired, it returns the next stage (if any) to fire.
//! Host code (the daemon's `watchdog.rs`) is responsible for
//! running the timer loop, calling `evaluate()` per session, and
//! executing the chosen `Escalation` — emitting a SemanticEvent
//! frame, writing Ctrl-C to the PTY, or killing the child.
//!
//! Split rationale (Pi-mono style):
//!   - **Pure part (here).** `Escalation`, `WatchdogConfig`,
//!     `EscalationState`, `evaluate()`. No I/O, no async, no time
//!     source. Deterministic for the caller-supplied clock.
//!   - **Host part (daemon).** Clock source, session enumeration,
//!     PTY write, child kill, SemanticEvent emission, env-var
//!     config overrides. Lives in `k2so-daemon/src/watchdog.rs`.
//!
//! Why that split: every side effect is host-specific. The
//! decision logic is a 20-line state machine that benefits from
//! exhaustive boundary tests more than any mock framework would
//! help. Keep the math here, the actions over there.
//!
//! **Grace period.** The host is expected to skip sessions whose
//! `created_at` is younger than `WatchdogConfig::spawn_grace` —
//! some CLI harnesses take 3-5 seconds to print their first
//! prompt, and a too-eager watchdog would SIGKILL them during
//! startup. The grace check lives in the host loop, not
//! `evaluate()`, because `SessionEntry` borrowing crosses the
//! pure/host line here.

use std::time::Duration;

/// Per-session bookkeeping the host maintains so the watchdog
/// doesn't re-fire the same escalation stage every poll tick.
/// Created with `Default::default()` when a session first
/// appears; persisted for the session's lifetime in the host's
/// watchdog map.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EscalationState {
    /// True once the host has logged the idle warning. Cleared
    /// never — one warning per session lifetime.
    pub warned: bool,
    /// True once Ctrl-C bytes (0x03) have been written to the
    /// session's PTY. Cleared never — a single Ctrl-C is plenty;
    /// if the target ignores it, escalation moves to kill.
    pub ctrl_c_sent: bool,
    /// True once the child process has been killed. Terminal —
    /// once here, nothing further the watchdog does matters.
    pub killed: bool,
}

/// What the watchdog has decided to do next for a given session.
/// `None` means "keep watching, nothing to do this tick."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Escalation {
    /// Session is fine. No action.
    None,
    /// Session has been idle past `warn_after`. Host logs a
    /// warning + emits a `SemanticEvent::Custom {
    /// kind: "watchdog.idle_warning" }` frame. Does NOT touch the
    /// PTY — warning is observation-only.
    Warn,
    /// Session has been idle past `ctrl_c_after`. Host writes a
    /// single 0x03 byte to the PTY (same as typing Ctrl-C) and
    /// emits a `SemanticEvent::Custom { kind:
    /// "watchdog.ctrl_c_sent" }` frame. Most interactive CLI
    /// harnesses (bash, claude, codex, vim) treat this as
    /// "cancel current operation / return to prompt."
    CtrlC,
    /// Session has been idle past `kill_after`. Host calls
    /// `SessionStreamSession::kill()` and emits a
    /// `SemanticEvent::Custom { kind: "watchdog.killed" }` frame.
    /// Terminal — after this the reader thread sees EOF and the
    /// session drops out of the registry.
    Kill,
}

/// Tunable thresholds for the watchdog's escalation ladder.
///
/// Every threshold is `Option<Duration>` — `None` disables that
/// stage entirely. A fully-`None` config is a valid "watchdog
/// off" mode that makes the host loop a no-op (useful for tests
/// and for projects whose users don't want automated kill).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchdogConfig {
    /// Idle threshold at which the host logs a warning + emits a
    /// `watchdog.idle_warning` SemanticEvent frame. Observation-
    /// only; no PTY action.
    pub warn_after: Option<Duration>,
    /// Idle threshold at which the host writes 0x03 to the PTY.
    pub ctrl_c_after: Option<Duration>,
    /// Idle threshold at which the host kills the child.
    pub kill_after: Option<Duration>,
    /// Grace period from `SessionEntry::created_at` during which
    /// the watchdog treats the session as "still booting" and
    /// escalation is frozen at `Escalation::None`. Covers harness
    /// startup latency.
    pub spawn_grace: Duration,
    /// How often the host loop should tick. Not consumed by
    /// `evaluate()` (that's pure); carried here so host code can
    /// read config from a single source. Default matches the PRD
    /// 5-second suggestion.
    pub poll_interval: Duration,
}

impl Default for WatchdogConfig {
    /// Conservative defaults for production. Interactive CLI
    /// sessions rarely idle past 10 minutes in real use; 30 min
    /// before a Ctrl-C and 60 min before a kill gives slow-but-
    /// live workflows (a dev reading output, a harness thinking)
    /// comfortable headroom without letting genuine wedges stay
    /// stuck indefinitely.
    fn default() -> Self {
        Self {
            warn_after: Some(Duration::from_secs(10 * 60)),
            ctrl_c_after: Some(Duration::from_secs(30 * 60)),
            kill_after: Some(Duration::from_secs(60 * 60)),
            spawn_grace: Duration::from_secs(10),
            poll_interval: Duration::from_secs(5),
        }
    }
}

impl WatchdogConfig {
    /// All-`None` thresholds; watchdog becomes a no-op. Used by
    /// tests that register fake sessions and don't want a
    /// real-daemon watchdog pulling the rug.
    pub fn disabled() -> Self {
        Self {
            warn_after: None,
            ctrl_c_after: None,
            kill_after: None,
            spawn_grace: Duration::ZERO,
            poll_interval: Duration::from_secs(1),
        }
    }

    /// True if at least one escalation stage is enabled. The host
    /// loop checks this at startup; `false` means "don't bother
    /// running a watchdog tick."
    pub fn any_stage_enabled(&self) -> bool {
        self.warn_after.is_some()
            || self.ctrl_c_after.is_some()
            || self.kill_after.is_some()
    }
}

/// Pure decision function: given current idle duration + which
/// stages already fired + the config, return the next escalation
/// to run. Checked in order of severity (kill > ctrl_c > warn) so
/// if the watchdog missed several ticks (daemon restart, system
/// suspend) and the session is past multiple thresholds, we
/// catch up to the highest-severity stage immediately rather
/// than laddering through warn → ctrl_c → kill over three ticks.
///
/// The stateful trio (`warned`/`ctrl_c_sent`/`killed`) prevents
/// re-firing: once `warned = true`, this fn never returns `Warn`
/// again for that session.
///
/// Callers are responsible for marking the returned stage on the
/// `EscalationState` — this fn takes `&EscalationState` to stay
/// pure-readable; `execute + mark` is the host's job.
pub fn evaluate(
    idle: Duration,
    state: &EscalationState,
    config: &WatchdogConfig,
) -> Escalation {
    // Kill: highest severity. If the session has already been
    // killed we're done — return None.
    if !state.killed {
        if let Some(threshold) = config.kill_after {
            if idle >= threshold {
                return Escalation::Kill;
            }
        }
    }
    // Ctrl-C: second tier. Skip if already sent OR if we've
    // already escalated to kill (can happen after a restart).
    if !state.ctrl_c_sent && !state.killed {
        if let Some(threshold) = config.ctrl_c_after {
            if idle >= threshold {
                return Escalation::CtrlC;
            }
        }
    }
    // Warn: informational. Skip if already warned OR if we're
    // already past a later stage.
    if !state.warned && !state.ctrl_c_sent && !state.killed {
        if let Some(threshold) = config.warn_after {
            if idle >= threshold {
                return Escalation::Warn;
            }
        }
    }
    Escalation::None
}

/// Apply the outcome of a just-executed escalation to the state.
/// Host calls this after `evaluate()` returns a stage and after
/// the action has been performed (so failed actions don't mark
/// the stage as fired — the next tick will retry).
pub fn mark_fired(state: &mut EscalationState, stage: Escalation) {
    match stage {
        Escalation::None => {}
        Escalation::Warn => state.warned = true,
        Escalation::CtrlC => state.ctrl_c_sent = true,
        Escalation::Kill => state.killed = true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> WatchdogConfig {
        WatchdogConfig {
            warn_after: Some(Duration::from_secs(10)),
            ctrl_c_after: Some(Duration::from_secs(30)),
            kill_after: Some(Duration::from_secs(60)),
            spawn_grace: Duration::from_secs(2),
            poll_interval: Duration::from_secs(1),
        }
    }

    fn fresh() -> EscalationState {
        EscalationState::default()
    }

    // ── Normal cascade: warn → ctrl_c → kill ──────────────────────

    #[test]
    fn idle_below_warn_returns_none() {
        let r = evaluate(Duration::from_secs(9), &fresh(), &cfg());
        assert_eq!(r, Escalation::None);
    }

    #[test]
    fn idle_at_warn_threshold_returns_warn() {
        let r = evaluate(Duration::from_secs(10), &fresh(), &cfg());
        assert_eq!(r, Escalation::Warn);
    }

    #[test]
    fn idle_past_warn_returns_warn_until_marked() {
        let r = evaluate(Duration::from_secs(15), &fresh(), &cfg());
        assert_eq!(r, Escalation::Warn);
    }

    #[test]
    fn idle_at_ctrl_c_threshold_returns_ctrl_c() {
        let r = evaluate(Duration::from_secs(30), &fresh(), &cfg());
        // Between warn (already past) and ctrl_c — the "catch up"
        // rule picks the higher severity ctrl_c directly, skipping
        // the redundant warn step.
        assert_eq!(r, Escalation::CtrlC);
    }

    #[test]
    fn idle_past_ctrl_c_but_warn_fired_returns_ctrl_c() {
        let mut s = fresh();
        s.warned = true;
        let r = evaluate(Duration::from_secs(35), &s, &cfg());
        assert_eq!(r, Escalation::CtrlC);
    }

    #[test]
    fn idle_at_kill_threshold_returns_kill() {
        let r = evaluate(Duration::from_secs(60), &fresh(), &cfg());
        assert_eq!(r, Escalation::Kill);
    }

    #[test]
    fn idle_past_kill_returns_kill_until_marked() {
        let mut s = fresh();
        s.warned = true;
        s.ctrl_c_sent = true;
        let r = evaluate(Duration::from_secs(65), &s, &cfg());
        assert_eq!(r, Escalation::Kill);
    }

    // ── Already-fired suppression ─────────────────────────────────

    #[test]
    fn warn_not_re_fired_after_marked() {
        let mut s = fresh();
        s.warned = true;
        let r = evaluate(Duration::from_secs(15), &s, &cfg());
        assert_eq!(r, Escalation::None);
    }

    #[test]
    fn ctrl_c_not_re_fired_after_marked() {
        let mut s = fresh();
        s.warned = true;
        s.ctrl_c_sent = true;
        let r = evaluate(Duration::from_secs(35), &s, &cfg());
        assert_eq!(r, Escalation::None);
    }

    #[test]
    fn killed_state_suppresses_everything() {
        let mut s = fresh();
        s.killed = true;
        let r = evaluate(Duration::from_secs(999), &s, &cfg());
        assert_eq!(r, Escalation::None);
    }

    // ── Disabled-stage short-circuit ──────────────────────────────

    #[test]
    fn no_warn_threshold_skips_warn() {
        let mut c = cfg();
        c.warn_after = None;
        let r = evaluate(Duration::from_secs(15), &fresh(), &c);
        assert_eq!(r, Escalation::None);
    }

    #[test]
    fn no_ctrl_c_threshold_warns_but_never_ctrl_cs() {
        let mut c = cfg();
        c.ctrl_c_after = None;
        assert_eq!(
            evaluate(Duration::from_secs(15), &fresh(), &c),
            Escalation::Warn
        );
        let mut s = fresh();
        s.warned = true;
        // Past ctrl_c threshold, but stage is disabled.
        assert_eq!(
            evaluate(Duration::from_secs(40), &s, &c),
            Escalation::None
        );
    }

    #[test]
    fn fully_disabled_config_never_escalates() {
        let c = WatchdogConfig::disabled();
        assert!(!c.any_stage_enabled());
        assert_eq!(
            evaluate(Duration::from_secs(999_999), &fresh(), &c),
            Escalation::None
        );
    }

    // ── Catch-up after missed ticks (daemon restart mid-crisis) ───

    #[test]
    fn catch_up_skips_lower_stages() {
        // Fresh state, session is already past kill threshold (e.g.
        // daemon restarted and found a stuck session). We should
        // jump straight to Kill rather than warn → ctrl_c → kill
        // over three ticks.
        let r = evaluate(Duration::from_secs(120), &fresh(), &cfg());
        assert_eq!(r, Escalation::Kill);
    }

    // ── mark_fired is a pure state mutator ────────────────────────

    #[test]
    fn mark_fired_sets_correct_field() {
        let mut s = fresh();
        mark_fired(&mut s, Escalation::Warn);
        assert!(s.warned);
        assert!(!s.ctrl_c_sent);
        assert!(!s.killed);

        mark_fired(&mut s, Escalation::CtrlC);
        assert!(s.ctrl_c_sent);
        assert!(!s.killed);

        mark_fired(&mut s, Escalation::Kill);
        assert!(s.killed);
    }

    #[test]
    fn mark_fired_none_is_noop() {
        let mut s = fresh();
        mark_fired(&mut s, Escalation::None);
        assert_eq!(s, fresh());
    }

    // ── Defaults ──────────────────────────────────────────────────

    #[test]
    fn default_config_has_all_stages_enabled() {
        let c = WatchdogConfig::default();
        assert!(c.warn_after.is_some());
        assert!(c.ctrl_c_after.is_some());
        assert!(c.kill_after.is_some());
        assert!(c.any_stage_enabled());
    }

    #[test]
    fn default_thresholds_monotonic() {
        // warn < ctrl_c < kill — otherwise the escalation ladder
        // would short-circuit on a later stage before the earlier
        // one fires.
        let c = WatchdogConfig::default();
        let w = c.warn_after.unwrap();
        let x = c.ctrl_c_after.unwrap();
        let k = c.kill_after.unwrap();
        assert!(w < x);
        assert!(x < k);
    }
}
