//! Mid-session relay-death watchdog — the PURE half.
//!
//! The supervise loop's failover is EXIT-based: frpc self-exits on a
//! failed LOGIN (`loginFailExit` defaults true), so a dead relay at dial
//! time surfaces as a fast child exit and
//! [`RelaySelector::on_failure`](super::failover::RelaySelector::on_failure)
//! fires. But when a relay dies MID-SESSION (after a successful login),
//! frpc does NOT exit — it reconnect-retries internally forever, logging
//! `connect to server error` / heartbeat-timeout lines while the tunnel
//! is actually down. This module classifies those log lines and decides
//! when the session is DEAD, so the connector can kill the stuck child
//! and let the existing exit-based failover path rotate relays.
//!
//! Deliberately free of I/O, threads, and clocks (time is injected via
//! `Instant`) — every rule is unit-tested in isolation, mirroring
//! [`failover`](super::failover).
//!
//! Policy ([`DisconnectTracker`]):
//!   * Each frpc log line is classified: disconnect signal
//!     ([`frpc_line_signals_disconnect`]), recovery signal
//!     ([`frpc_line_signals_recovery`]), or neither (ignored).
//!   * [`DEFAULT_DISCONNECT_THRESHOLD`] disconnect signals within a
//!     rolling [`DEFAULT_DISCONNECT_WINDOW`], with no intervening
//!     recovery line, declare the session dead.
//!   * A recovery line RESETS the streak — frpc's internal reconnect
//!     succeeded, so a transient blip never cycles the child.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Disconnect-signal lines within the rolling window before the session
/// is declared dead. Three matches the relay-rotation failure threshold's
/// "not a blip" philosophy ([`super::failover::DEFAULT_FAILURE_THRESHOLD`]).
pub const DEFAULT_DISCONNECT_THRESHOLD: usize = 3;

/// Rolling window the disconnect signals must fall inside. frpc's
/// reconnect loop retries every few seconds, so a genuinely dead relay
/// produces the threshold well inside 30 s; sparse one-off errors from a
/// healthy-but-lossy link never accumulate.
pub const DEFAULT_DISCONNECT_WINDOW: Duration = Duration::from_secs(30);

/// True when an frpc (frp v0.61) log line signals a MID-SESSION
/// connection failure — the lines frpc emits while internally
/// reconnect-retrying against a dead relay instead of exiting.
///
/// Matched substrings are frp's own message text (case-sensitive, as frp
/// logs them); timestamps/level/file prefixes are irrelevant to the
/// match. `heartbeat` is matched as `heartbeat timeout` and `reconnect`
/// as `reconnect to server` so that a proxy/subdomain NAME containing
/// those words can never false-positive a healthy line.
pub fn frpc_line_signals_disconnect(line: &str) -> bool {
    const SIGNALS: [&str; 5] = [
        // service.go — the reconnect loop's dial failures.
        "connect to server error",
        // control.go — established control connection dropped.
        "read error",
        // control.go — server stopped answering pings.
        "heartbeat timeout",
        // service.go — a reconnect attempt's login was rejected/failed.
        "login to server failed",
        // service.go — "try to reconnect to server..." announcement.
        "reconnect to server",
    ];
    SIGNALS.iter().any(|s| line.contains(s))
}

/// True when an frpc log line signals the session RECOVERED — frp's
/// internal reconnect worked, so any disconnect streak must reset.
pub fn frpc_line_signals_recovery(line: &str) -> bool {
    const SIGNALS: [&str; 2] = [
        // service.go — (re-)login accepted by frps.
        "login to server success",
        // control.go — proxy re-registered and serving again.
        "start proxy success",
    ];
    SIGNALS.iter().any(|s| line.contains(s))
}

/// Pure mid-session death detector. Feed every frpc log line through
/// [`observe`](DisconnectTracker::observe); it returns `true` the moment
/// the disconnect policy (threshold within rolling window, no intervening
/// recovery) declares the session dead. Time is INJECTED so tests are
/// deterministic — the tracker never reads a clock itself.
#[derive(Debug)]
pub struct DisconnectTracker {
    /// Disconnect signals within `window` that declare death.
    threshold: usize,
    /// Rolling window the signals must fall inside.
    window: Duration,
    /// Timestamps of disconnect signals still inside the window,
    /// oldest first. Cleared by any recovery line.
    hits: VecDeque<Instant>,
}

impl DisconnectTracker {
    /// Tracker with the default policy
    /// ([`DEFAULT_DISCONNECT_THRESHOLD`] / [`DEFAULT_DISCONNECT_WINDOW`]).
    pub fn new() -> Self {
        Self::with_policy(DEFAULT_DISCONNECT_THRESHOLD, DEFAULT_DISCONNECT_WINDOW)
    }

    /// [`new`](DisconnectTracker::new) with explicit policy (tests /
    /// future config knobs). `threshold` is clamped to ≥1 — a threshold
    /// of 0 would mean "dead before the first error", which is nonsense.
    pub fn with_policy(threshold: usize, window: Duration) -> Self {
        Self {
            threshold: threshold.max(1),
            window,
            hits: VecDeque::new(),
        }
    }

    /// The configured disconnect threshold (for the watchdog's log line).
    pub fn threshold(&self) -> usize {
        self.threshold
    }

    /// The configured rolling window (for the watchdog's log line).
    pub fn window(&self) -> Duration {
        self.window
    }

    /// Feed one frpc log line observed at `now`. Returns `true` when this
    /// line completes the death policy: it is the `threshold`-th
    /// disconnect signal within the rolling `window` with no recovery
    /// line in between. Recovery lines reset the streak; unclassified
    /// lines (normal traffic/startup chatter) are ignored entirely.
    pub fn observe(&mut self, line: &str, now: Instant) -> bool {
        // Recovery wins: frp's reconnect succeeded, the streak is over.
        if frpc_line_signals_recovery(line) {
            self.hits.clear();
            return false;
        }
        if !frpc_line_signals_disconnect(line) {
            return false;
        }
        self.hits.push_back(now);
        // Drop signals that have aged out of the rolling window (a signal
        // exactly `window` old still counts — the window is inclusive).
        while let Some(&oldest) = self.hits.front() {
            if now.duration_since(oldest) > self.window {
                self.hits.pop_front();
            } else {
                break;
            }
        }
        self.hits.len() >= self.threshold
    }
}

impl Default for DisconnectTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- classifier: positive lines (real frp v0.61 shapes) ----

    #[test]
    fn classifier_matches_midsession_failure_lines() {
        let dead = [
            "2026/07/11 10:00:00 [W] [service.go:132] connect to server error: dial tcp 178.156.232.105:7000: i/o timeout",
            "2026/07/11 10:00:01 [E] [control.go:171] read error: EOF",
            "2026/07/11 10:00:02 [W] [control.go:158] heartbeat timeout",
            "2026/07/11 10:00:03 [W] [service.go:141] login to server failed: dial tcp 178.156.232.105:7000: connect: connection refused",
            "2026/07/11 10:00:04 [I] [service.go:334] try to reconnect to server...",
        ];
        for line in dead {
            assert!(
                frpc_line_signals_disconnect(line),
                "must classify as disconnect: {line}"
            );
            assert!(
                !frpc_line_signals_recovery(line),
                "a failure line must never read as recovery: {line}"
            );
        }
    }

    // ---- classifier: normal lines must NOT match ----

    #[test]
    fn classifier_ignores_healthy_and_traffic_lines() {
        let healthy = [
            "2026/07/11 10:00:00 [I] [service.go:299] login to server success, get run id [abc123], server udp port [0]",
            "2026/07/11 10:00:01 [I] [control.go:181] [k2so-rosson] start proxy success",
            "2026/07/11 10:00:02 [I] [proxy_manager.go:150] proxy added: [k2so-rosson]",
            "2026/07/11 10:00:03 [I] [proxy.go:204] [k2so-rosson] get a new work connection: [203.0.113.9:51442]",
            "2026/07/11 10:00:04 [I] [service.go:250] start frpc service for config file [/Users/x/.k2/frpc.toml]",
            "",
        ];
        for line in healthy {
            assert!(
                !frpc_line_signals_disconnect(line),
                "must NOT classify as disconnect: {line:?}"
            );
        }
    }

    #[test]
    fn classifier_recovery_lines_are_exactly_the_two_success_shapes() {
        assert!(frpc_line_signals_recovery(
            "[I] [service.go:299] login to server success, get run id [abc]"
        ));
        assert!(frpc_line_signals_recovery(
            "[I] [control.go:181] [k2so-rosson] start proxy success"
        ));
        // Adjacent-but-different lines must not read as recovery.
        assert!(!frpc_line_signals_recovery(
            "[I] [proxy_manager.go:150] proxy added: [k2so-rosson]"
        ));
        assert!(!frpc_line_signals_recovery(""));
    }

    #[test]
    fn classifier_bare_keywords_in_names_do_not_false_positive() {
        // A subdomain/proxy name containing the scary words must not
        // trip the disconnect classifier on an otherwise healthy line.
        assert!(!frpc_line_signals_disconnect(
            "[I] [control.go:181] [k2so-heartbeat] start proxy success"
        ));
        assert!(!frpc_line_signals_disconnect(
            "[I] [proxy.go:204] [k2so-reconnect] get a new work connection: [1.2.3.4:5]"
        ));
    }

    // ---- DisconnectTracker policy ----

    const ERR: &str = "[W] [service.go:132] connect to server error: dial tcp: i/o timeout";
    const OK_LOGIN: &str = "[I] [service.go:299] login to server success, get run id [x]";
    const OK_PROXY: &str = "[I] [control.go:181] [k2so-rosson] start proxy success";
    const NOISE: &str = "[I] [proxy.go:204] [k2so-rosson] get a new work connection: [1.2.3.4:5]";

    #[test]
    fn streak_of_three_errors_in_window_declares_dead_exactly_at_threshold() {
        let mut t = DisconnectTracker::new();
        let t0 = Instant::now();
        assert!(!t.observe(ERR, t0), "1/3 must not declare dead");
        assert!(!t.observe(ERR, t0 + Duration::from_secs(5)), "2/3 must not");
        assert!(
            t.observe(ERR, t0 + Duration::from_secs(10)),
            "exactly the 3rd error inside the window declares dead"
        );
    }

    #[test]
    fn success_line_resets_the_streak() {
        let mut t = DisconnectTracker::new();
        let t0 = Instant::now();
        assert!(!t.observe(ERR, t0));
        assert!(!t.observe(ERR, t0 + Duration::from_secs(1)));
        // frp's internal reconnect recovered — do NOT fail over on a blip.
        assert!(!t.observe(OK_LOGIN, t0 + Duration::from_secs(2)));
        // The streak restarted: two more errors are 2/3, not 4.
        assert!(!t.observe(ERR, t0 + Duration::from_secs(3)));
        assert!(!t.observe(ERR, t0 + Duration::from_secs(4)));
        // ...and only a third post-recovery error declares dead.
        assert!(t.observe(ERR, t0 + Duration::from_secs(5)));
    }

    #[test]
    fn proxy_success_also_resets_the_streak() {
        let mut t = DisconnectTracker::new();
        let t0 = Instant::now();
        assert!(!t.observe(ERR, t0));
        assert!(!t.observe(ERR, t0 + Duration::from_secs(1)));
        assert!(!t.observe(OK_PROXY, t0 + Duration::from_secs(2)));
        assert!(
            !t.observe(ERR, t0 + Duration::from_secs(3)),
            "streak restarted at 1/3 after start-proxy-success"
        );
    }

    #[test]
    fn sparse_errors_over_a_long_window_never_declare_dead() {
        // One error every 20 s with a 30 s window: at most 2 signals ever
        // share a window, so a lossy-but-alive link never cycles frpc.
        let mut t = DisconnectTracker::new();
        let t0 = Instant::now();
        for i in 0..20u64 {
            assert!(
                !t.observe(ERR, t0 + Duration::from_secs(i * 20)),
                "sparse error #{i} must not accumulate to dead"
            );
        }
    }

    #[test]
    fn error_exactly_window_old_still_counts_inclusive_boundary() {
        let mut t = DisconnectTracker::new();
        let t0 = Instant::now();
        assert!(!t.observe(ERR, t0));
        assert!(!t.observe(ERR, t0 + Duration::from_secs(15)));
        // 3rd signal exactly DEFAULT_DISCONNECT_WINDOW after the 1st: the
        // 1st is exactly window-old (inclusive) → 3 in window → dead.
        assert!(t.observe(ERR, t0 + DEFAULT_DISCONNECT_WINDOW));
    }

    #[test]
    fn error_just_past_window_ages_out() {
        let mut t = DisconnectTracker::new();
        let t0 = Instant::now();
        assert!(!t.observe(ERR, t0));
        assert!(!t.observe(ERR, t0 + Duration::from_secs(15)));
        // 3rd signal past the window from the 1st: the 1st aged out, so
        // only 2 remain in-window — still alive.
        assert!(!t.observe(
            ERR,
            t0 + DEFAULT_DISCONNECT_WINDOW + Duration::from_secs(1)
        ));
        // But one more inside the window tips it (15s, W+1, W+2 all fit).
        assert!(t.observe(
            ERR,
            t0 + DEFAULT_DISCONNECT_WINDOW + Duration::from_secs(2)
        ));
    }

    #[test]
    fn unclassified_lines_are_ignored_not_resets() {
        // Normal traffic chatter between errors neither counts as a
        // disconnect NOR resets the streak (only recovery lines reset).
        let mut t = DisconnectTracker::new();
        let t0 = Instant::now();
        assert!(!t.observe(ERR, t0));
        assert!(!t.observe(NOISE, t0 + Duration::from_secs(1)));
        assert!(!t.observe(ERR, t0 + Duration::from_secs(2)));
        assert!(!t.observe(NOISE, t0 + Duration::from_secs(3)));
        assert!(
            t.observe(ERR, t0 + Duration::from_secs(4)),
            "3 errors in window declare dead despite interleaved noise"
        );
    }

    #[test]
    fn custom_policy_is_honored_and_zero_threshold_clamps_to_one() {
        let mut t = DisconnectTracker::with_policy(1, Duration::from_secs(5));
        assert!(t.observe(ERR, Instant::now()), "threshold 1 → first error is dead");

        // Threshold 0 clamps to 1 (never "dead before the first error").
        let mut t = DisconnectTracker::with_policy(0, Duration::from_secs(5));
        assert_eq!(t.threshold(), 1);
        assert!(t.observe(ERR, Instant::now()));
    }
}
