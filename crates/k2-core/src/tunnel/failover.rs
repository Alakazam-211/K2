//! Relay failover policy — the PURE half of multi-relay edge resilience.
//!
//! frp's `frpc` takes ONE `serverAddr` and cannot fail over natively, so
//! the daemon owns the policy: which relay of the ordered fallback list
//! ([`TunnelConfig::relay_list`](super::config::TunnelConfig::relay_list))
//! frpc should be dialing *right now*. This module is deliberately free
//! of I/O, clocks, and process handles — the connector's supervise loop
//! feeds it failure/success observations and acts on the returned
//! [`RelaySwitch`]; every rule here is unit-tested in isolation.
//!
//! Policy:
//!   * **Failover** — [`RelaySelector::on_failure`] counts CONSECUTIVE
//!     failures; at [`DEFAULT_FAILURE_THRESHOLD`] it advances to the next
//!     relay (wrapping) and resets the counter. A single relay never
//!     switches — the selector is then a no-op shell around it.
//!   * **No thrash** — one success ([`RelaySelector::on_success`]) resets
//!     the failure counter, so intermittent blips never accumulate into a
//!     spurious switch.
//!   * **Fail-back** — index 0 is the PREFERRED relay. While running on a
//!     fallback, once successes span [`DEFAULT_FAILBACK_AFTER`] of stable
//!     uptime the selector returns to index 0 (optimistic re-probe: if
//!     the primary is still dead, the normal failover path rotates away
//!     again after the threshold).

use std::time::{Duration, Instant};

use super::config::RelayEndpoint;

/// Consecutive dial failures on the current relay before advancing to the
/// next one.
pub const DEFAULT_FAILURE_THRESHOLD: u32 = 3;

/// How long a fallback relay must stay healthy before we optimistically
/// return to the preferred relay (index 0). Long enough that a flapping
/// primary can't drag the tunnel into a switch storm; short enough that a
/// recovered primary is re-adopted within minutes.
pub const DEFAULT_FAILBACK_AFTER: Duration = Duration::from_secs(300);

/// A decided relay change. `after_failures` is the consecutive-failure
/// count that drove a failover, and `0` for a fail-back (which is driven
/// by stability, not failure).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelaySwitch {
    pub from: RelayEndpoint,
    pub to: RelayEndpoint,
    pub after_failures: u32,
}

impl RelaySwitch {
    /// True when this switch is the stability-driven return to the
    /// preferred relay rather than a failure-driven rotation.
    pub fn is_failback(&self) -> bool {
        self.after_failures == 0
    }
}

/// Pure relay-selection state machine. See the module docs for the
/// policy. Time is INJECTED (`now: Instant` on [`on_success`]) so tests
/// are deterministic — the selector never reads a clock itself.
///
/// [`on_success`]: RelaySelector::on_success
#[derive(Debug)]
pub struct RelaySelector {
    /// Ordered fallback list; index 0 = preferred. Never empty.
    relays: Vec<RelayEndpoint>,
    /// Index of the relay frpc should currently dial.
    current: usize,
    /// Consecutive failures observed on `current` since the last success
    /// or switch.
    failures: u32,
    /// Failures needed to advance ([`DEFAULT_FAILURE_THRESHOLD`]).
    failure_threshold: u32,
    /// Stable-uptime span on a fallback before failing back to index 0.
    failback_after: Duration,
    /// While on a fallback relay: when the current unbroken healthy
    /// streak began. Cleared by any failure and while on the primary.
    healthy_since: Option<Instant>,
}

impl RelaySelector {
    /// Build a selector over the ordered `relays` list with the default
    /// thresholds. Panics on an empty list — the config layer guarantees
    /// at least one entry ([`relay_list`]), so an empty list here is a
    /// caller bug, not a runtime condition to limp through.
    ///
    /// [`relay_list`]: super::config::TunnelConfig::relay_list
    pub fn new(relays: Vec<RelayEndpoint>) -> Self {
        Self::with_policy(relays, DEFAULT_FAILURE_THRESHOLD, DEFAULT_FAILBACK_AFTER)
    }

    /// [`new`](RelaySelector::new) with explicit thresholds (tests / future
    /// config knobs). `failure_threshold` is clamped to ≥1 — a threshold of
    /// 0 would mean "switch before the first attempt", which is nonsense.
    pub fn with_policy(
        relays: Vec<RelayEndpoint>,
        failure_threshold: u32,
        failback_after: Duration,
    ) -> Self {
        assert!(
            !relays.is_empty(),
            "RelaySelector requires at least one relay (relay_list() never returns empty)"
        );
        Self {
            relays,
            current: 0,
            failures: 0,
            failure_threshold: failure_threshold.max(1),
            failback_after,
            healthy_since: None,
        }
    }

    /// The relay frpc should currently be dialing.
    pub fn current(&self) -> &RelayEndpoint {
        &self.relays[self.current]
    }

    /// True when there is only one relay — no failover possible, and the
    /// connector keeps its pre-failover single-relay code path.
    pub fn is_solo(&self) -> bool {
        self.relays.len() == 1
    }

    /// Consecutive failures observed on the current relay (diagnostics).
    pub fn consecutive_failures(&self) -> u32 {
        self.failures
    }

    /// Record one dial/connection failure on the current relay. Returns
    /// `Some(switch)` when the consecutive-failure threshold is reached
    /// and there is another relay to rotate to (wrapping past the end);
    /// `None` otherwise. A switch resets the failure counter for the new
    /// relay. A single-relay selector NEVER switches.
    pub fn on_failure(&mut self) -> Option<RelaySwitch> {
        // Any failure breaks a healthy streak — fail-back must be earned
        // by UNBROKEN stability on the fallback.
        self.healthy_since = None;
        self.failures += 1;
        if self.relays.len() == 1 || self.failures < self.failure_threshold {
            return None;
        }
        let from = self.relays[self.current].clone();
        let after_failures = self.failures;
        self.current = (self.current + 1) % self.relays.len();
        self.failures = 0;
        Some(RelaySwitch {
            from,
            to: self.relays[self.current].clone(),
            after_failures,
        })
    }

    /// Record a healthy observation at `now`. Always resets the failure
    /// counter (the "don't thrash" rule: blips never accumulate). While
    /// on a fallback relay, successes start/extend a healthy streak; once
    /// the streak spans `failback_after`, returns `Some(switch)` back to
    /// the preferred relay (index 0). On the primary this never switches.
    pub fn on_success(&mut self, now: Instant) -> Option<RelaySwitch> {
        self.failures = 0;
        if self.current == 0 {
            self.healthy_since = None;
            return None;
        }
        let since = *self.healthy_since.get_or_insert(now);
        if now.duration_since(since) < self.failback_after {
            return None;
        }
        let from = self.relays[self.current].clone();
        self.current = 0;
        self.healthy_since = None;
        Some(RelaySwitch {
            from,
            to: self.relays[0].clone(),
            after_failures: 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ep(host: &str) -> RelayEndpoint {
        RelayEndpoint {
            host: host.to_string(),
            port: 7000,
        }
    }

    fn pair() -> Vec<RelayEndpoint> {
        vec![ep("primary"), ep("secondary")]
    }

    #[test]
    fn single_relay_never_switches_no_matter_how_many_failures() {
        let mut sel = RelaySelector::new(vec![ep("only")]);
        assert!(sel.is_solo());
        for i in 1..=20 {
            assert_eq!(
                sel.on_failure(),
                None,
                "solo selector must never switch (failure #{i})"
            );
            assert_eq!(sel.current(), &ep("only"));
        }
        assert_eq!(sel.consecutive_failures(), 20, "failures still counted");
    }

    #[test]
    fn two_relays_switch_after_exactly_three_consecutive_failures() {
        let mut sel = RelaySelector::new(pair());
        assert_eq!(sel.current(), &ep("primary"));
        assert_eq!(sel.on_failure(), None, "failure 1/3 must not switch");
        assert_eq!(sel.on_failure(), None, "failure 2/3 must not switch");
        let sw = sel.on_failure().expect("failure 3/3 must switch");
        assert_eq!(
            sw,
            RelaySwitch {
                from: ep("primary"),
                to: ep("secondary"),
                after_failures: 3,
            }
        );
        assert!(!sw.is_failback());
        assert_eq!(sel.current(), &ep("secondary"));
        assert_eq!(
            sel.consecutive_failures(),
            0,
            "switch must reset the counter for the new relay"
        );
    }

    #[test]
    fn one_success_resets_the_failure_counter_no_thrash() {
        // 2 failures + success + 2 failures — never 3 CONSECUTIVE, so a
        // blip-y but working relay is never abandoned.
        let mut sel = RelaySelector::new(pair());
        assert_eq!(sel.on_failure(), None);
        assert_eq!(sel.on_failure(), None);
        assert_eq!(sel.on_success(Instant::now()), None);
        assert_eq!(sel.consecutive_failures(), 0, "success must reset the counter");
        assert_eq!(sel.on_failure(), None, "counter restarted: 1/3");
        assert_eq!(sel.on_failure(), None, "counter restarted: 2/3");
        assert_eq!(sel.current(), &ep("primary"), "still on primary");
        // ...and only a third consecutive failure finally rotates.
        assert!(sel.on_failure().is_some());
    }

    #[test]
    fn wrap_around_from_last_relay_back_to_first() {
        let mut sel = RelaySelector::new(pair());
        for _ in 0..3 {
            let _ = sel.on_failure(); // primary -> secondary
        }
        assert_eq!(sel.current(), &ep("secondary"));
        for i in 1..=2 {
            assert_eq!(sel.on_failure(), None, "failure {i}/3 on secondary");
        }
        let sw = sel.on_failure().expect("3rd failure on secondary wraps");
        assert_eq!(sw.from, ep("secondary"));
        assert_eq!(sw.to, ep("primary"), "must wrap past the end to index 0");
        assert_eq!(sel.current(), &ep("primary"));
    }

    #[test]
    fn three_relays_rotate_in_declared_order() {
        let mut sel = RelaySelector::new(vec![ep("a"), ep("b"), ep("c")]);
        let fail_over = |sel: &mut RelaySelector| {
            let mut sw = None;
            for _ in 0..3 {
                sw = sel.on_failure();
            }
            sw.expect("3 consecutive failures must switch")
        };
        assert_eq!(fail_over(&mut sel).to, ep("b"));
        assert_eq!(fail_over(&mut sel).to, ep("c"));
        assert_eq!(fail_over(&mut sel).to, ep("a"), "wraps a→b→c→a");
    }

    #[test]
    fn fails_back_to_primary_after_stable_period_on_fallback() {
        let mut sel = RelaySelector::new(pair());
        for _ in 0..3 {
            let _ = sel.on_failure(); // move to secondary
        }
        let t0 = Instant::now();
        assert_eq!(
            sel.on_success(t0),
            None,
            "first success only STARTS the streak"
        );
        assert_eq!(
            sel.on_success(t0 + DEFAULT_FAILBACK_AFTER / 2),
            None,
            "mid-streak success must not fail back early"
        );
        let sw = sel
            .on_success(t0 + DEFAULT_FAILBACK_AFTER)
            .expect("stable period elapsed -> fail back");
        assert_eq!(
            sw,
            RelaySwitch {
                from: ep("secondary"),
                to: ep("primary"),
                after_failures: 0,
            }
        );
        assert!(sw.is_failback());
        assert_eq!(sel.current(), &ep("primary"));
    }

    #[test]
    fn failure_breaks_the_healthy_streak_so_failback_restarts() {
        let mut sel = RelaySelector::new(pair());
        for _ in 0..3 {
            let _ = sel.on_failure(); // move to secondary
        }
        let t0 = Instant::now();
        assert_eq!(sel.on_success(t0), None);
        // A blip on the fallback resets the streak (and doesn't switch —
        // 1 failure < threshold).
        assert_eq!(sel.on_failure(), None);
        // A full period after t0 is NOT enough anymore: the streak
        // restarted at the post-blip success.
        assert_eq!(
            sel.on_success(t0 + DEFAULT_FAILBACK_AFTER),
            None,
            "streak was broken — this success restarts it"
        );
        let sw = sel
            .on_success(t0 + DEFAULT_FAILBACK_AFTER * 2)
            .expect("unbroken period after the restart -> fail back");
        assert_eq!(sw.to, ep("primary"));
    }

    #[test]
    fn success_on_primary_never_switches() {
        let mut sel = RelaySelector::new(pair());
        let t0 = Instant::now();
        assert_eq!(sel.on_success(t0), None);
        assert_eq!(
            sel.on_success(t0 + DEFAULT_FAILBACK_AFTER * 10),
            None,
            "already on the preferred relay — nothing to fail back to"
        );
        assert_eq!(sel.current(), &ep("primary"));
    }

    #[test]
    fn custom_threshold_is_honored_and_zero_clamps_to_one() {
        let mut sel = RelaySelector::with_policy(pair(), 1, DEFAULT_FAILBACK_AFTER);
        let sw = sel.on_failure().expect("threshold 1 switches immediately");
        assert_eq!(sw.after_failures, 1);

        // Threshold 0 clamps to 1 (never "switch before trying").
        let mut sel = RelaySelector::with_policy(pair(), 0, DEFAULT_FAILBACK_AFTER);
        assert!(sel.on_failure().is_some(), "clamped threshold = 1");
    }

    #[test]
    #[should_panic(expected = "at least one relay")]
    fn empty_relay_list_is_a_caller_bug() {
        let _ = RelaySelector::new(Vec::new());
    }
}
