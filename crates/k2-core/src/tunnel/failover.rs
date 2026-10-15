//! Relay failover policy — the PURE half of multi-relay edge resilience.
//!
//! frp's `frpc` takes ONE `serverAddr` and cannot fail over natively, so
//! the daemon owns the policy: which relay of the ordered fallback list
//! ([`TunnelConfig::relay_list`](super::config::TunnelConfig::relay_list))
//! frpc should be dialing *right now*. This module is deliberately free
//! of I/O, clocks, and process handles — the connector's supervise loop
//! feeds it failure/success/probe observations and acts on the returned
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
//!   * **Fail-back (0.45.7: probe-driven)** — index 0 is the PRIMARY relay.
//!     While running on a fallback the supervisor TCP-probes the primary
//!     every [`FailbackPolicy::probe_every`] (± a spread). After
//!     [`FailbackPolicy::healthy_probes`] healthy probes IN A ROW the
//!     selector schedules one controlled return to the primary, delayed by
//!     a random 0..=[`FailbackPolicy::failback_delay_max`] so a fleet of
//!     boxes doesn't move in the same second. A failed probe resets the
//!     streak and cancels a scheduled return. Nothing fires while the
//!     primary is the relay in use. (Before 0.45.7 the selector failed
//!     back blind after 5 min of fallback uptime: during a primary outage
//!     that bounced boxes back onto the dead primary every few minutes.)

use std::time::{Duration, Instant};

use super::config::RelayEndpoint;

/// Consecutive dial failures on the current relay before advancing to the
/// next one.
pub const DEFAULT_FAILURE_THRESHOLD: u32 = 3;

/// When and how the supervisor returns to the primary relay (index 0)
/// after a failover. See the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FailbackPolicy {
    /// Base gap between two probes of the primary while on a fallback.
    pub probe_every: Duration,
    /// Each gap is `probe_every` ± a random amount up to this.
    pub probe_spread: Duration,
    /// Healthy probes in a row that earn the return to the primary.
    pub healthy_probes: u32,
    /// The return is delayed by a random 0..=this after the last probe.
    pub failback_delay_max: Duration,
}

impl FailbackPolicy {
    /// Production policy: probe every 5 min (± 1 min), 3 healthy probes in
    /// a row, then return within 0–120 s.
    pub const DEFAULT: FailbackPolicy = FailbackPolicy {
        probe_every: Duration::from_secs(300),
        probe_spread: Duration::from_secs(60),
        healthy_probes: 3,
        failback_delay_max: Duration::from_secs(120),
    };
}

impl Default for FailbackPolicy {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// A decided relay change. `after_failures` is the consecutive-failure
/// count that drove a failover, and `0` for a fail-back (which is driven
/// by healthy probes of the primary, not by failure).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelaySwitch {
    pub from: RelayEndpoint,
    pub to: RelayEndpoint,
    pub after_failures: u32,
}

impl RelaySwitch {
    /// True when this switch is the probe-driven return to the primary
    /// relay rather than a failure-driven rotation.
    pub fn is_failback(&self) -> bool {
        self.after_failures == 0
    }
}

/// Pure relay-selection state machine. See the module docs for the
/// policy. Time is INJECTED (`now: Instant`) so tests are deterministic —
/// the selector never reads a clock itself. Randomness (probe spread and
/// failback delay) comes from a small seeded generator; tests seed it or
/// zero the spreads.
#[derive(Debug)]
pub struct RelaySelector {
    /// Ordered fallback list; index 0 = primary. Never empty.
    relays: Vec<RelayEndpoint>,
    /// Index of the relay frpc should currently dial.
    current: usize,
    /// Consecutive failures observed on `current` since the last success
    /// or switch.
    failures: u32,
    /// Failures needed to advance ([`DEFAULT_FAILURE_THRESHOLD`]).
    failure_threshold: u32,
    /// Fail-back timing.
    failback: FailbackPolicy,
    /// While on a fallback: when the next probe of the primary is due.
    next_probe_at: Option<Instant>,
    /// Healthy probes of the primary in a row.
    probe_streak: u32,
    /// The scheduled (randomly delayed) return to the primary, once earned.
    failback_at: Option<Instant>,
    /// xorshift64 state for the spreads (never 0).
    rng: u64,
}

impl RelaySelector {
    /// Build a selector over the ordered `relays` list with the default
    /// policy. Panics on an empty list — the config layer guarantees at
    /// least one entry ([`relay_list`]), so an empty list here is a caller
    /// bug, not a runtime condition to limp through.
    ///
    /// [`relay_list`]: super::config::TunnelConfig::relay_list
    pub fn new(relays: Vec<RelayEndpoint>) -> Self {
        Self::with_policy(relays, DEFAULT_FAILURE_THRESHOLD, FailbackPolicy::DEFAULT)
    }

    /// [`new`](RelaySelector::new) with an explicit policy (tests / the
    /// connector's test override). `failure_threshold` is clamped to ≥1 —
    /// a threshold of 0 would mean "switch before the first attempt".
    pub fn with_policy(
        relays: Vec<RelayEndpoint>,
        failure_threshold: u32,
        failback: FailbackPolicy,
    ) -> Self {
        Self::with_policy_seeded(relays, failure_threshold, failback, random_seed())
    }

    /// [`with_policy`](RelaySelector::with_policy) with a fixed RNG seed.
    pub fn with_policy_seeded(
        relays: Vec<RelayEndpoint>,
        failure_threshold: u32,
        failback: FailbackPolicy,
        seed: u64,
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
            failback,
            next_probe_at: None,
            probe_streak: 0,
            failback_at: None,
            rng: if seed == 0 { 0x9E37_79B9_7F4A_7C15 } else { seed },
        }
    }

    /// The relay frpc should currently be dialing.
    pub fn current(&self) -> &RelayEndpoint {
        &self.relays[self.current]
    }

    /// The primary relay (index 0).
    pub fn primary(&self) -> &RelayEndpoint {
        &self.relays[0]
    }

    /// True when the relay in use is the primary.
    pub fn on_primary(&self) -> bool {
        self.current == 0
    }

    /// The ordered list this selector rotates through.
    pub fn relays(&self) -> &[RelayEndpoint] {
        &self.relays
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

    /// Healthy probes of the primary in a row (diagnostics / tests).
    pub fn probe_streak(&self) -> u32 {
        self.probe_streak
    }

    /// When the earned return to the primary is due (diagnostics / tests).
    pub fn failback_due_at(&self) -> Option<Instant> {
        self.failback_at
    }

    /// Adopt a new ordered relay list (the on-disk list changed; applied
    /// at a reconnect, never as a live restart). The relay in use stays in
    /// use when the new list still carries it; otherwise the selector
    /// starts over at the new primary. Failure and probe state reset.
    /// Returns true when the relay to dial changed. Panics on an empty
    /// list, like [`new`](RelaySelector::new).
    pub fn adopt(&mut self, relays: Vec<RelayEndpoint>) -> bool {
        assert!(!relays.is_empty(), "RelaySelector requires at least one relay");
        let before = self.current().clone();
        self.current = relays.iter().position(|r| *r == before).unwrap_or(0);
        self.relays = relays;
        self.failures = 0;
        self.reset_probes();
        *self.current() != before
    }

    fn reset_probes(&mut self) {
        self.next_probe_at = None;
        self.probe_streak = 0;
        self.failback_at = None;
    }

    /// Record one dial/connection failure on the current relay. Returns
    /// `Some(switch)` when the consecutive-failure threshold is reached
    /// and there is another relay to rotate to (wrapping past the end);
    /// `None` otherwise. A switch resets the failure counter for the new
    /// relay. A single-relay selector NEVER switches.
    pub fn on_failure(&mut self) -> Option<RelaySwitch> {
        self.failures += 1;
        if self.relays.len() == 1 || self.failures < self.failure_threshold {
            return None;
        }
        let from = self.relays[self.current].clone();
        let after_failures = self.failures;
        self.current = (self.current + 1) % self.relays.len();
        self.failures = 0;
        // A new relay in use: a probe streak earned from the old position
        // starts over.
        self.reset_probes();
        Some(RelaySwitch {
            from,
            to: self.relays[self.current].clone(),
            after_failures,
        })
    }

    /// Record a healthy observation of the relay in use. Resets the
    /// failure counter (the "don't thrash" rule: blips never accumulate).
    /// Never switches — the return to the primary is probe-driven.
    pub fn on_success(&mut self) {
        self.failures = 0;
    }

    /// Whether a probe of the primary is due at `now`. Always false while
    /// the primary is in use. The first call on a fallback schedules the
    /// first probe one (spread) interval out and returns false.
    pub fn primary_probe_due(&mut self, now: Instant) -> bool {
        if self.on_primary() {
            self.reset_probes();
            return false;
        }
        match self.next_probe_at {
            Some(at) => now >= at,
            None => {
                self.next_probe_at = Some(now + self.probe_gap());
                false
            }
        }
    }

    /// Record the result of one probe of the primary taken at `now`.
    /// Healthy extends the streak; once it reaches
    /// [`FailbackPolicy::healthy_probes`] the return is scheduled (with
    /// its random delay). Unhealthy resets the streak and cancels a
    /// scheduled return. Ignored while the primary is in use.
    pub fn on_primary_probe(&mut self, healthy: bool, now: Instant) {
        if self.on_primary() {
            self.reset_probes();
            return;
        }
        self.next_probe_at = Some(now + self.probe_gap());
        if !healthy {
            self.probe_streak = 0;
            self.failback_at = None;
            return;
        }
        self.probe_streak += 1;
        if self.probe_streak >= self.failback.healthy_probes.max(1) && self.failback_at.is_none() {
            let delay = self.random_upto(self.failback.failback_delay_max);
            self.failback_at = Some(now + delay);
        }
    }

    /// The scheduled return to the primary, once its delay has passed.
    /// `None` on the primary, before the streak is earned, or before the
    /// delay is up.
    pub fn poll_failback(&mut self, now: Instant) -> Option<RelaySwitch> {
        if self.on_primary() {
            self.reset_probes();
            return None;
        }
        match self.failback_at {
            Some(at) if now >= at => {
                let from = self.relays[self.current].clone();
                self.current = 0;
                self.failures = 0;
                self.reset_probes();
                Some(RelaySwitch {
                    from,
                    to: self.relays[0].clone(),
                    after_failures: 0,
                })
            }
            _ => None,
        }
    }

    /// `probe_every` ± a random amount up to `probe_spread`.
    fn probe_gap(&mut self) -> Duration {
        let base = self.failback.probe_every;
        let spread = self.failback.probe_spread.min(base);
        if spread.is_zero() {
            return base;
        }
        let r = self.random_upto(spread * 2);
        base - spread + r
    }

    /// Random duration in `0..=max` (millisecond grain).
    fn random_upto(&mut self, max: Duration) -> Duration {
        let ms = max.as_millis() as u64;
        if ms == 0 {
            return Duration::ZERO;
        }
        // xorshift64
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        Duration::from_millis(x % (ms + 1))
    }
}

/// A seed from the OS RNG (the clock when that is unavailable).
fn random_seed() -> u64 {
    let mut b = [0u8; 8];
    if getrandom::getrandom(&mut b).is_ok() {
        let v = u64::from_le_bytes(b);
        if v != 0 {
            return v;
        }
    }
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64 | 1)
        .unwrap_or(0x9E37_79B9_7F4A_7C15)
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

    /// No spreads: probe every 300 s exactly, return right after the 3rd
    /// healthy probe. Makes the timing assertions exact.
    const EXACT: FailbackPolicy = FailbackPolicy {
        probe_every: Duration::from_secs(300),
        probe_spread: Duration::ZERO,
        healthy_probes: 3,
        failback_delay_max: Duration::ZERO,
    };

    fn on_secondary(policy: FailbackPolicy) -> RelaySelector {
        let mut sel =
            RelaySelector::with_policy_seeded(pair(), DEFAULT_FAILURE_THRESHOLD, policy, 42);
        for _ in 0..3 {
            let _ = sel.on_failure();
        }
        assert_eq!(sel.current(), &ep("secondary"), "setup: must be on the fallback");
        sel
    }

    /// Drive one probe at `t` (asserting it is due) with `healthy`.
    fn probe(sel: &mut RelaySelector, t: Instant, healthy: bool) {
        assert!(sel.primary_probe_due(t), "a probe must be due at the scheduled time");
        sel.on_primary_probe(healthy, t);
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
        sel.on_success();
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
    fn fails_back_after_three_healthy_probes_of_the_primary() {
        let mut sel = on_secondary(EXACT);
        let t0 = Instant::now();
        assert!(
            !sel.primary_probe_due(t0),
            "the first call on a fallback only schedules the first probe"
        );
        assert!(
            !sel.primary_probe_due(t0 + Duration::from_secs(299)),
            "no probe before the interval"
        );
        let step = Duration::from_secs(300);
        probe(&mut sel, t0 + step, true);
        assert_eq!(sel.poll_failback(t0 + step), None, "1 healthy probe is not enough");
        probe(&mut sel, t0 + step * 2, true);
        assert_eq!(sel.poll_failback(t0 + step * 2), None, "2 healthy probes are not enough");
        probe(&mut sel, t0 + step * 3, true);
        assert_eq!(sel.probe_streak(), 3);
        let sw = sel
            .poll_failback(t0 + step * 3)
            .expect("3 healthy probes in a row earn the return to the primary");
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
        assert_eq!(sel.probe_streak(), 0, "probe state resets on the primary");
    }

    #[test]
    fn a_failed_probe_resets_the_streak() {
        let mut sel = on_secondary(EXACT);
        let t0 = Instant::now();
        assert!(!sel.primary_probe_due(t0));
        let step = Duration::from_secs(300);
        probe(&mut sel, t0 + step, true);
        probe(&mut sel, t0 + step * 2, true);
        probe(&mut sel, t0 + step * 3, false);
        assert_eq!(sel.probe_streak(), 0, "a failed probe must reset the streak");
        assert_eq!(sel.poll_failback(t0 + step * 3), None);
        // Two more healthy probes: still only 2 in a row — no return.
        probe(&mut sel, t0 + step * 4, true);
        probe(&mut sel, t0 + step * 5, true);
        assert_eq!(
            sel.poll_failback(t0 + step * 5),
            None,
            "the streak restarted after the failed probe: 2 of 3"
        );
        probe(&mut sel, t0 + step * 6, true);
        assert!(sel.poll_failback(t0 + step * 6).is_some(), "3 in a row again → return");
    }

    #[test]
    fn a_failed_probe_cancels_a_scheduled_return() {
        // A long random delay: the return is earned but not yet due when
        // the primary fails again.
        let policy = FailbackPolicy {
            failback_delay_max: Duration::from_secs(120),
            ..EXACT
        };
        let mut sel = on_secondary(policy);
        let t0 = Instant::now();
        assert!(!sel.primary_probe_due(t0));
        let step = Duration::from_secs(300);
        for i in 1..=3 {
            probe(&mut sel, t0 + step * i, true);
        }
        let due = sel.failback_due_at().expect("streak earned → return scheduled");
        assert!(
            due <= t0 + step * 3 + Duration::from_secs(120),
            "the random delay is capped at failback_delay_max"
        );
        sel.on_primary_probe(false, t0 + step * 3);
        assert_eq!(sel.failback_due_at(), None, "a failed probe cancels the return");
        assert_eq!(sel.poll_failback(t0 + step * 10), None);
        assert_eq!(sel.current(), &ep("secondary"));
    }

    #[test]
    fn no_failback_while_the_primary_is_in_use() {
        let mut sel = RelaySelector::with_policy_seeded(pair(), 3, EXACT, 7);
        let t0 = Instant::now();
        for i in 0..10u32 {
            let t = t0 + Duration::from_secs(300) * i;
            assert!(!sel.primary_probe_due(t), "never probe the primary while on it");
            sel.on_primary_probe(true, t); // a stray result is ignored
            assert_eq!(sel.poll_failback(t), None, "nothing to fail back to");
        }
        assert_eq!(sel.probe_streak(), 0);
        assert_eq!(sel.current(), &ep("primary"));
        // Long healthy uptime on the primary never switches either.
        sel.on_success();
        assert_eq!(sel.current(), &ep("primary"));
    }

    #[test]
    fn success_on_a_fallback_never_fails_back_by_itself() {
        // 0.45.7: no blind fail-back on fallback uptime any more.
        let mut sel = on_secondary(EXACT);
        for _ in 0..100 {
            sel.on_success();
        }
        assert_eq!(sel.current(), &ep("secondary"));
        assert_eq!(sel.poll_failback(Instant::now() + Duration::from_secs(86_400)), None);
    }

    #[test]
    fn a_failover_mid_streak_restarts_the_probe_streak() {
        // Three relays: on b, two healthy probes of a, then b dies → c.
        let mut sel = RelaySelector::with_policy_seeded(
            vec![ep("a"), ep("b"), ep("c")],
            3,
            EXACT,
            9,
        );
        for _ in 0..3 {
            let _ = sel.on_failure();
        }
        assert_eq!(sel.current(), &ep("b"));
        let t0 = Instant::now();
        assert!(!sel.primary_probe_due(t0));
        let step = Duration::from_secs(300);
        probe(&mut sel, t0 + step, true);
        probe(&mut sel, t0 + step * 2, true);
        for _ in 0..3 {
            let _ = sel.on_failure();
        }
        assert_eq!(sel.current(), &ep("c"));
        assert_eq!(sel.probe_streak(), 0, "a rotation restarts the streak");
        assert!(
            !sel.primary_probe_due(t0 + step * 2),
            "the first call after the rotation only schedules"
        );
    }

    #[test]
    fn probe_gap_stays_inside_the_spread() {
        let policy = FailbackPolicy {
            probe_spread: Duration::from_secs(60),
            ..EXACT
        };
        let mut sel = on_secondary(policy);
        let t0 = Instant::now();
        for _ in 0..50 {
            sel.next_probe_at = None;
            assert!(!sel.primary_probe_due(t0));
            let at = sel.next_probe_at.expect("scheduled");
            let gap = at - t0;
            assert!(
                gap >= Duration::from_secs(240) && gap <= Duration::from_secs(360),
                "gap {gap:?} must be 300 s ± 60 s"
            );
        }
    }

    #[test]
    fn adopt_keeps_the_relay_in_use_when_still_listed() {
        let mut sel = on_secondary(EXACT);
        let changed = sel.adopt(vec![ep("primary"), ep("third"), ep("secondary")]);
        assert!(!changed, "the relay in use is still listed → keep dialing it");
        assert_eq!(sel.current(), &ep("secondary"));
        assert_eq!(sel.primary(), &ep("primary"));
        assert_eq!(sel.relays().len(), 3);
    }

    #[test]
    fn adopt_moves_to_the_new_primary_when_the_relay_in_use_is_gone() {
        let mut sel = RelaySelector::new(vec![ep("old")]);
        let changed = sel.adopt(vec![ep("new-a"), ep("new-b")]);
        assert!(changed);
        assert_eq!(sel.current(), &ep("new-a"));
        assert!(!sel.is_solo());
        assert_eq!(sel.consecutive_failures(), 0);
    }

    #[test]
    fn custom_threshold_is_honored_and_zero_clamps_to_one() {
        let mut sel = RelaySelector::with_policy(pair(), 1, FailbackPolicy::DEFAULT);
        let sw = sel.on_failure().expect("threshold 1 switches immediately");
        assert_eq!(sw.after_failures, 1);

        // Threshold 0 clamps to 1 (never "switch before trying").
        let mut sel = RelaySelector::with_policy(pair(), 0, FailbackPolicy::DEFAULT);
        assert!(sel.on_failure().is_some(), "clamped threshold = 1");
    }

    #[test]
    #[should_panic(expected = "at least one relay")]
    fn empty_relay_list_is_a_caller_bug() {
        let _ = RelaySelector::new(Vec::new());
    }
}
