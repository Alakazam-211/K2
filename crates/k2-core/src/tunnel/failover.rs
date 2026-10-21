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
//!
//! **0.46.3: DNS-aware failover ([`FailoverMode::Dns`], the default).**
//! `*.k2.dev` only follows a box to the standby once the DNS watcher has
//! seen the primary fail health checks for 2 min, and the relays do not
//! forward to each other. So moving to a relay the public name does not
//! point at makes the box unreachable (2026-10-21: a 2-min refusal on the
//! primary's :7000 left failed-over boxes dark for 10–17 min). In DNS mode
//! a failure past the threshold only moves to a non-primary relay when
//!   * k2.dev's relay list says that relay is the active one
//!     ([`DnsHint::Active`], `relays[i].active` in `GET /api/tunnel/relays`), or
//!   * the list does not say ([`DnsHint::Unknown`]: k2.dev unreachable,
//!     an old server, no relay or two relays marked active) and the primary
//!     has been down without a break for [`DEFAULT_DNS_MIN_OUTAGE`] (150 s,
//!     longer than the 2-min DNS flip).
//! Otherwise it holds and keeps redialing the primary. On a fallback it
//! returns after [`FailbackPolicy::DNS_AWARE`] (2 healthy probes ~30 s
//! apart, then 0–30 s fleet jitter), unless k2.dev says the name points at
//! the fallback. A failed probe still resets the streak (no bouncing onto a
//! dead primary). [`FailoverMode::Legacy`] restores 0.46.2 exactly: the
//! connector then calls the very same [`RelaySelector::on_failure`] /
//! [`RelaySelector::on_primary_probe`] / [`RelaySelector::poll_failback`]
//! with [`FailbackPolicy::DEFAULT`] and never asks k2.dev.

use std::time::{Duration, Instant};

use serde::Serialize;

use super::config::RelayEndpoint;

/// Consecutive dial failures on the current relay before advancing to the
/// next one.
pub const DEFAULT_FAILURE_THRESHOLD: u32 = 3;

/// 0.46.3 DNS mode: how long the primary must have been down without a
/// break before a move to a standby the relay list does not vouch for.
/// Longer than the k2.dev DNS watcher's 2-min flip.
pub const DEFAULT_DNS_MIN_OUTAGE: Duration = Duration::from_secs(150);

/// Which failover rules the supervisor runs (0.46.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FailoverMode {
    /// Move only when the `*.k2.dev` name can follow (the default).
    #[default]
    Dns,
    /// The 0.46.2 rules, exactly: 3 failed dials move to the next relay;
    /// fail-back after 3 healthy probes ~5 min apart.
    Legacy,
}

impl FailoverMode {
    /// Lenient parse of the `tunnel.json` `failover` value / the
    /// `K2_TUNNEL_FAILOVER` env var. `None` = not recognised.
    pub fn parse(raw: &str) -> Option<FailoverMode> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "legacy" | "0.46.2" | "old" => Some(FailoverMode::Legacy),
            "dns" | "dns-aware" | "default" | "new" => Some(FailoverMode::Dns),
            _ => None,
        }
    }

    /// Wire / status name.
    pub fn as_str(self) -> &'static str {
        match self {
            FailoverMode::Dns => "dns",
            FailoverMode::Legacy => "legacy",
        }
    }
}

/// What k2.dev's relay list says about where `*.k2.dev` points right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DnsHint {
    /// The wildcard points at this relay (exactly one relay was marked
    /// `active: true` in a recent fetch).
    Active(RelayEndpoint),
    /// Not known: k2.dev unreachable or stale, an old server without the
    /// field, no relay or several relays marked active.
    Unknown,
}

/// Why the selector stayed put after a failure past the threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hold {
    /// k2.dev says the name points at the relay in use.
    NameHere,
    /// k2.dev did not say; the outage is shorter than the minimum.
    WaitingForOutage,
}

/// The selector's verdict on one failure ([`RelaySelector::on_failure_at`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailureDecision {
    /// Below the threshold (or a single relay): keep redialing.
    Count,
    /// Past the threshold, but the name cannot follow a move: keep
    /// redialing the relay in use. The text says why (for the log).
    Hold(String),
    /// Move. The text says why (for the log).
    Switch(RelaySwitch, String),
}

/// The failover part of `/cli/tunnel/status` (`failover`), 0.46.3.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FailoverStatus {
    /// `dns` (0.46.3 default) or `legacy` (0.46.2 rules).
    pub mode: String,
    /// `ready` | `waiting_for_dns` | `on_fallback` | `returning` | `legacy`.
    pub state: String,
    /// One plain sentence for `k2 tunnel status`.
    pub detail: String,
}

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

    /// 0.46.3 DNS mode: probe every 30 s (± 5 s), 2 healthy probes in a
    /// row, then return within 0–30 s — back on the primary about 1–1.5
    /// min after it answers again.
    pub const DNS_AWARE: FailbackPolicy = FailbackPolicy {
        probe_every: Duration::from_secs(30),
        probe_spread: Duration::from_secs(5),
        healthy_probes: 2,
        failback_delay_max: Duration::from_secs(30),
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
    /// 0.46.3: which rules run. Constructors start in
    /// [`FailoverMode::Legacy`]; [`RelaySelector::with_dns_gate`] opts in.
    mode: FailoverMode,
    /// DNS mode: outage needed before a move k2.dev did not vouch for.
    dns_min_outage: Duration,
    /// DNS mode: when the current failure streak began.
    down_since: Option<Instant>,
    /// DNS mode: why the last failure past the threshold stayed put.
    hold: Option<Hold>,
    /// DNS mode: the last probe round found k2.dev saying the name points
    /// at the fallback in use, so the return to the primary is held.
    fallback_name_here: bool,
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
            mode: FailoverMode::Legacy,
            dns_min_outage: DEFAULT_DNS_MIN_OUTAGE,
            down_since: None,
            hold: None,
            fallback_name_here: false,
        }
    }

    /// Switch this selector to the 0.46.3 DNS-aware rules with the given
    /// minimum outage (see the module docs).
    pub fn with_dns_gate(mut self, min_outage: Duration) -> Self {
        self.mode = FailoverMode::Dns;
        self.dns_min_outage = min_outage;
        self
    }

    /// The rules this selector runs.
    pub fn mode(&self) -> FailoverMode {
        self.mode
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
        self.down_since = None;
        self.hold = None;
        self.reset_probes();
        *self.current() != before
    }

    fn reset_probes(&mut self) {
        self.next_probe_at = None;
        self.probe_streak = 0;
        self.failback_at = None;
        self.fallback_name_here = false;
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
        self.down_since = None;
        self.hold = None;
    }

    /// 0.46.3: record one failure at `now` under the selector's mode.
    ///
    /// * [`FailoverMode::Legacy`] — exactly [`on_failure`](Self::on_failure)
    ///   (`now` and `hint` are ignored).
    /// * [`FailoverMode::Dns`] — below the threshold nothing happens. Past
    ///   it: when `hint` names a listed relay, move there (or hold when it
    ///   is the relay in use); when it does not, a move onto the primary is
    ///   always allowed, a move onto another relay only once the failure
    ///   streak has lasted the minimum outage.
    pub fn on_failure_at(&mut self, now: Instant, hint: &DnsHint) -> FailureDecision {
        if self.mode == FailoverMode::Legacy {
            return match self.on_failure() {
                Some(sw) => {
                    let why = format!("after {} failures (0.46.2 rules)", sw.after_failures);
                    FailureDecision::Switch(sw, why)
                }
                None => FailureDecision::Count,
            };
        }
        self.failures += 1;
        let since = *self.down_since.get_or_insert(now);
        if self.relays.len() == 1 || self.failures < self.failure_threshold {
            return FailureDecision::Count;
        }
        let n = self.relays.len();
        let down_for = now.saturating_duration_since(since);
        let named = match hint {
            DnsHint::Active(r) => self.relays.iter().position(|x| x == r),
            DnsHint::Unknown => None,
        };
        let (target, why) = match named {
            Some(i) if i == self.current => {
                self.hold = Some(Hold::NameHere);
                let why = if self.current == 0 {
                    "staying on primary, standby not active per k2.dev".to_string()
                } else {
                    format!(
                        "staying on {}, k2.dev says the name points here",
                        self.relays[self.current]
                    )
                };
                return FailureDecision::Hold(why);
            }
            Some(i) => (i, format!("{} is the active relay per k2.dev", self.relays[i])),
            None => {
                let next = (self.current + 1) % n;
                if next == 0 {
                    (0, "back to the primary (the fallback is failing)".to_string())
                } else if down_for >= self.dns_min_outage {
                    (
                        next,
                        format!(
                            "primary down for {} s and k2.dev did not say where the name points",
                            down_for.as_secs()
                        ),
                    )
                } else {
                    self.hold = Some(Hold::WaitingForOutage);
                    return FailureDecision::Hold(format!(
                        "staying on {}, k2.dev did not say where the name points; \
                         moving only after {} s down ({} s so far)",
                        self.relays[self.current],
                        self.dns_min_outage.as_secs(),
                        down_for.as_secs()
                    ));
                }
            }
        };
        let from = self.relays[self.current].clone();
        let after_failures = self.failures;
        self.current = target;
        self.failures = 0;
        self.down_since = None;
        self.hold = None;
        self.reset_probes();
        FailureDecision::Switch(
            RelaySwitch {
                from,
                to: self.relays[self.current].clone(),
                after_failures,
            },
            why,
        )
    }

    /// 0.46.3: record one probe of the primary under the selector's mode.
    /// Legacy is exactly [`on_primary_probe`](Self::on_primary_probe). DNS
    /// mode does the same, except that while k2.dev says the name points
    /// at the fallback in use the streak is reset and no return is earned
    /// (going back would take the box off the public name).
    pub fn on_primary_probe_at(&mut self, healthy: bool, now: Instant, hint: &DnsHint) {
        if self.mode == FailoverMode::Dns && !self.on_primary() && self.names_current(hint) {
            self.next_probe_at = Some(now + self.probe_gap());
            self.probe_streak = 0;
            self.failback_at = None;
            self.fallback_name_here = true;
            return;
        }
        self.fallback_name_here = false;
        self.on_primary_probe(healthy, now);
    }

    /// 0.46.3: [`poll_failback`](Self::poll_failback) under the mode. DNS
    /// mode cancels an earned return when k2.dev now says the name points
    /// at the fallback in use.
    pub fn poll_failback_at(&mut self, now: Instant, hint: &DnsHint) -> Option<RelaySwitch> {
        if self.mode == FailoverMode::Dns && !self.on_primary() && self.names_current(hint) {
            self.probe_streak = 0;
            self.failback_at = None;
            self.fallback_name_here = true;
            return None;
        }
        self.poll_failback(now)
    }

    fn names_current(&self, hint: &DnsHint) -> bool {
        matches!(hint, DnsHint::Active(r) if *r == self.relays[self.current])
    }

    /// The failover part of the tunnel status. `None` for a single relay
    /// (nowhere to move).
    pub fn failover_status(&self) -> Option<FailoverStatus> {
        if self.is_solo() {
            return None;
        }
        let mk = |state: &str, detail: String| {
            Some(FailoverStatus {
                mode: self.mode.as_str().to_string(),
                state: state.to_string(),
                detail,
            })
        };
        if self.mode == FailoverMode::Legacy {
            return mk(
                "legacy",
                "0.46.2 rules: moves after 3 failed dials (set by tunnel.json \"failover\" or K2_TUNNEL_FAILOVER)"
                    .to_string(),
            );
        }
        if self.on_primary() {
            return match self.hold {
                Some(Hold::NameHere) => mk(
                    "waiting_for_dns",
                    "waiting for DNS (standby inactive per k2.dev); retrying the primary"
                        .to_string(),
                ),
                Some(Hold::WaitingForOutage) => mk(
                    "waiting_for_dns",
                    format!(
                        "waiting for DNS (k2.dev did not say where the name points); \
                         retrying the primary, moving after {} s down",
                        self.dns_min_outage.as_secs()
                    ),
                ),
                None => mk(
                    "ready",
                    "on the primary; moves to a standby only when the k2.dev name can follow"
                        .to_string(),
                ),
            };
        }
        if self.failback_at.is_some() {
            return mk(
                "returning",
                "returning to the primary (it passed its health probes)".to_string(),
            );
        }
        if self.fallback_name_here || self.hold == Some(Hold::NameHere) {
            return mk(
                "on_fallback",
                "on a standby; k2.dev says the name points here, so it stays".to_string(),
            );
        }
        mk(
            "on_fallback",
            format!(
                "on a standby; returns after {} healthy probes of the primary",
                self.failback.healthy_probes.max(1)
            ),
        )
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

    // ── 0.46.3 DNS-aware failover ─────────────────────────────────────
    //
    // Fake clock: every test drives an explicit `Instant` timeline; the
    // selector never reads a clock. Fake relay list: the `DnsHint` passed
    // in is exactly what the connector derives from k2.dev's list.

    fn dns_selector() -> RelaySelector {
        RelaySelector::with_policy_seeded(pair(), DEFAULT_FAILURE_THRESHOLD, DNS_EXACT, 11)
            .with_dns_gate(DEFAULT_DNS_MIN_OUTAGE)
    }

    /// DNS-mode fail-back with no probe spread: probe every 30 s, 2
    /// healthy probes, then a 0–30 s delay.
    const DNS_EXACT: FailbackPolicy = FailbackPolicy {
        probe_every: Duration::from_secs(30),
        probe_spread: Duration::ZERO,
        healthy_probes: 2,
        failback_delay_max: Duration::from_secs(30),
    };

    fn active(host: &str) -> DnsHint {
        DnsHint::Active(ep(host))
    }

    /// Feed failures every `gap` from `t0` for `span` with `hint`; return
    /// the first switch (and when it happened), or `None`.
    fn fail_for(
        sel: &mut RelaySelector,
        t0: Instant,
        span: Duration,
        gap: Duration,
        hint: &DnsHint,
    ) -> Option<(Duration, RelaySwitch, String)> {
        let mut at = Duration::ZERO;
        while at <= span {
            if let FailureDecision::Switch(sw, why) = sel.on_failure_at(t0 + at, hint) {
                return Some((at, sw, why));
            }
            at += gap;
        }
        None
    }

    #[test]
    fn dns_two_minute_blip_with_standby_inactive_never_switches() {
        // 2026-10-21: the primary refused :7000 for ~2 min while k2.dev's
        // name stayed on it. k2.dev says the primary is active → stay.
        let mut sel = dns_selector();
        let t0 = Instant::now();
        let got = fail_for(
            &mut sel,
            t0,
            Duration::from_secs(130),
            Duration::from_secs(2),
            &active("primary"),
        );
        assert_eq!(got, None, "must keep redialing the primary through the blip");
        assert_eq!(sel.current(), &ep("primary"));
        let st = sel.failover_status().expect("two relays report failover");
        assert_eq!(st.state, "waiting_for_dns");
        assert!(st.detail.contains("standby inactive per k2.dev"), "{}", st.detail);
        // Way past 150 s too: k2.dev vouches the name is on the primary.
        let got = fail_for(
            &mut sel,
            t0 + Duration::from_secs(130),
            Duration::from_secs(600),
            Duration::from_secs(30),
            &active("primary"),
        );
        assert_eq!(got, None, "k2.dev says the name is on the primary: never move");
        // The primary answers again: back to ready.
        sel.on_success();
        assert_eq!(sel.failover_status().expect("status").state, "ready");
    }

    #[test]
    fn dns_two_minute_blip_with_k2dev_unknown_never_switches() {
        // Today's k2.dev (no `active` field) during the same 130 s blip.
        let mut sel = dns_selector();
        let t0 = Instant::now();
        let got = fail_for(
            &mut sel,
            t0,
            Duration::from_secs(130),
            Duration::from_secs(1),
            &DnsHint::Unknown,
        );
        assert_eq!(got, None, "a 130 s outage is under the 150 s minimum");
        assert_eq!(sel.current(), &ep("primary"));
        let st = sel.failover_status().expect("status");
        assert_eq!(st.state, "waiting_for_dns");
        assert!(st.detail.contains("did not say"), "{}", st.detail);
    }

    #[test]
    fn dns_standby_active_switches_right_at_the_threshold() {
        let mut sel = dns_selector();
        let t0 = Instant::now();
        assert_eq!(sel.on_failure_at(t0, &active("secondary")), FailureDecision::Count);
        assert_eq!(
            sel.on_failure_at(t0 + Duration::from_secs(1), &active("secondary")),
            FailureDecision::Count
        );
        match sel.on_failure_at(t0 + Duration::from_secs(2), &active("secondary")) {
            FailureDecision::Switch(sw, why) => {
                assert_eq!(
                    sw,
                    RelaySwitch { from: ep("primary"), to: ep("secondary"), after_failures: 3 }
                );
                assert!(why.contains("active relay per k2.dev"), "{why}");
            }
            other => panic!("3rd failure with the standby active must switch, got {other:?}"),
        }
        assert_eq!(sel.current(), &ep("secondary"));
        assert_eq!(sel.consecutive_failures(), 0);
    }

    #[test]
    fn dns_standby_turning_active_mid_outage_switches_on_the_next_failure() {
        let mut sel = dns_selector();
        let t0 = Instant::now();
        let got = fail_for(
            &mut sel,
            t0,
            Duration::from_secs(60),
            Duration::from_secs(5),
            &active("primary"),
        );
        assert_eq!(got, None);
        // The DNS watcher flipped the name at ~2 min.
        match sel.on_failure_at(t0 + Duration::from_secs(125), &active("secondary")) {
            FailureDecision::Switch(sw, _) => assert_eq!(sw.to, ep("secondary")),
            other => panic!("must follow the name to the standby, got {other:?}"),
        }
    }

    #[test]
    fn dns_k2dev_unreachable_and_primary_down_150s_switches() {
        let mut sel = dns_selector();
        let t0 = Instant::now();
        let (at, sw, why) = fail_for(
            &mut sel,
            t0,
            Duration::from_secs(400),
            Duration::from_secs(5),
            &DnsHint::Unknown,
        )
        .expect("150 s down with k2.dev silent must move to the standby");
        assert_eq!(at, Duration::from_secs(150), "moves at the first failure at/after 150 s");
        assert_eq!(sw.to, ep("secondary"));
        assert!(why.contains("primary down for 150 s"), "{why}");
    }

    #[test]
    fn dns_a_success_restarts_the_150s_clock() {
        // "Continuously unreachable": a working run in between breaks it.
        let mut sel = dns_selector();
        let t0 = Instant::now();
        assert_eq!(
            fail_for(&mut sel, t0, Duration::from_secs(140), Duration::from_secs(5), &DnsHint::Unknown),
            None
        );
        sel.on_success();
        let t1 = t0 + Duration::from_secs(145);
        assert_eq!(
            fail_for(&mut sel, t1, Duration::from_secs(140), Duration::from_secs(5), &DnsHint::Unknown),
            None,
            "the clock restarted at the success"
        );
        assert_eq!(sel.current(), &ep("primary"));
    }

    #[test]
    fn dns_hint_naming_an_unlisted_relay_counts_as_unknown() {
        let mut sel = dns_selector();
        let t0 = Instant::now();
        let hint = active("somewhere-else");
        assert_eq!(
            fail_for(&mut sel, t0, Duration::from_secs(140), Duration::from_secs(5), &hint),
            None,
            "an unlisted active relay is never a target, and does not skip the 150 s rule"
        );
        let (_, sw, _) = fail_for(
            &mut sel,
            t0 + Duration::from_secs(145),
            Duration::from_secs(30),
            Duration::from_secs(5),
            &hint,
        )
        .expect("the 150 s rule still applies");
        assert_eq!(sw.to, ep("secondary"), "never the unlisted relay");
    }

    #[test]
    fn dns_fallback_failing_wraps_back_to_the_primary_without_waiting() {
        let mut sel = dns_selector();
        let t0 = Instant::now();
        fail_for(&mut sel, t0, Duration::from_secs(10), Duration::from_secs(1), &active("secondary"))
            .expect("to the standby");
        // The standby dies too and k2.dev is silent: the primary is where
        // the name lives by default — no 150 s wait for that move.
        let t1 = t0 + Duration::from_secs(20);
        let (at, sw, _) =
            fail_for(&mut sel, t1, Duration::from_secs(10), Duration::from_secs(1), &DnsHint::Unknown)
                .expect("wrap to the primary");
        assert_eq!(at, Duration::from_secs(2), "after the plain 3-failure threshold");
        assert_eq!(sw.to, ep("primary"));
    }

    #[test]
    fn dns_failing_fallback_holds_while_k2dev_says_the_name_is_there() {
        let mut sel = dns_selector();
        let t0 = Instant::now();
        fail_for(&mut sel, t0, Duration::from_secs(10), Duration::from_secs(1), &active("secondary"))
            .expect("to the standby");
        let got = fail_for(
            &mut sel,
            t0 + Duration::from_secs(20),
            Duration::from_secs(300),
            Duration::from_secs(10),
            &active("secondary"),
        );
        assert_eq!(got, None, "the name is on the standby: keep redialing it");
        assert_eq!(sel.failover_status().expect("status").state, "on_fallback");
    }

    /// Fail over (standby active), then return once the primary is back.
    fn dns_failback_run(hint_after: &DnsHint) -> Duration {
        let mut sel = dns_selector();
        let t0 = Instant::now();
        fail_for(&mut sel, t0, Duration::from_secs(10), Duration::from_secs(1), &active("secondary"))
            .expect("to the standby");
        // From here the primary is healthy again; `hint_after` is what
        // k2.dev says (or Unknown when it can't be fetched).
        let ts = t0 + Duration::from_secs(10);
        assert!(!sel.primary_probe_due(ts), "first call schedules the first probe");
        let mut t = ts;
        loop {
            t += Duration::from_secs(1);
            assert!(
                t - ts <= Duration::from_secs(120),
                "no fail-back within 2 min (state {:?})",
                sel.failover_status()
            );
            if sel.primary_probe_due(t) {
                sel.on_primary_probe_at(true, t, hint_after);
            }
            if let Some(sw) = sel.poll_failback_at(t, hint_after) {
                assert!(sw.is_failback());
                assert_eq!(sw.to, ep("primary"));
                assert_eq!(sel.current(), &ep("primary"));
                return t - ts;
            }
        }
    }

    #[test]
    fn dns_fails_back_within_about_a_minute_when_the_primary_is_active_again() {
        let took = dns_failback_run(&active("primary"));
        assert!(
            took >= Duration::from_secs(60) && took <= Duration::from_secs(90),
            "2 healthy probes 30 s apart + 0–30 s jitter = 60–90 s, took {took:?}"
        );
    }

    #[test]
    fn dns_fails_back_within_about_a_minute_when_k2dev_cannot_be_fetched() {
        let took = dns_failback_run(&DnsHint::Unknown);
        assert!(
            took >= Duration::from_secs(60) && took <= Duration::from_secs(90),
            "healthy probes alone earn the return when the list is unknown, took {took:?}"
        );
    }

    #[test]
    fn dns_production_failback_policy_stays_within_one_and_a_half_minutes() {
        // Worst case with the real spreads: 2 gaps of 35 s + 30 s jitter.
        let p = FailbackPolicy::DNS_AWARE;
        let worst = (p.probe_every + p.probe_spread) * p.healthy_probes + p.failback_delay_max;
        assert_eq!(worst, Duration::from_secs(100));
        let best = (p.probe_every - p.probe_spread) * p.healthy_probes;
        assert_eq!(best, Duration::from_secs(50));
        assert!(p.failback_delay_max <= Duration::from_secs(30), "fleet jitter capped at 30 s");
    }

    #[test]
    fn dns_no_failback_while_k2dev_says_the_name_is_on_the_fallback() {
        let mut sel = dns_selector();
        let t0 = Instant::now();
        fail_for(&mut sel, t0, Duration::from_secs(10), Duration::from_secs(1), &active("secondary"))
            .expect("to the standby");
        let ts = t0 + Duration::from_secs(10);
        assert!(!sel.primary_probe_due(ts));
        for s in 1..=600u64 {
            let t = ts + Duration::from_secs(s);
            if sel.primary_probe_due(t) {
                sel.on_primary_probe_at(true, t, &active("secondary"));
            }
            assert_eq!(
                sel.poll_failback_at(t, &active("secondary")),
                None,
                "going back would take the box off the public name"
            );
        }
        assert_eq!(sel.probe_streak(), 0);
        let st = sel.failover_status().expect("status");
        assert!(st.detail.contains("points here"), "{}", st.detail);
    }

    #[test]
    fn dns_an_earned_return_is_cancelled_when_the_name_moves_to_the_fallback() {
        let mut sel = dns_selector();
        let t0 = Instant::now();
        fail_for(&mut sel, t0, Duration::from_secs(10), Duration::from_secs(1), &active("secondary"))
            .expect("to the standby");
        let ts = t0 + Duration::from_secs(10);
        assert!(!sel.primary_probe_due(ts));
        probe(&mut sel, ts + Duration::from_secs(30), true);
        probe(&mut sel, ts + Duration::from_secs(60), true);
        assert!(sel.failback_due_at().is_some(), "return earned");
        assert_eq!(sel.failover_status().expect("status").state, "returning");
        assert_eq!(
            sel.poll_failback_at(ts + Duration::from_secs(200), &active("secondary")),
            None,
            "k2.dev now says the name is on the fallback: cancel"
        );
        assert_eq!(sel.failback_due_at(), None);
        assert_eq!(sel.current(), &ep("secondary"));
    }

    #[test]
    fn dns_bounce_guard_a_dead_primary_is_never_returned_to() {
        // The 0.45.7 guard holds in DNS mode: a probe that fails resets
        // the streak and cancels a scheduled return, whatever k2.dev says.
        let mut sel = dns_selector();
        let t0 = Instant::now();
        let (_, sw, _) = fail_for(
            &mut sel,
            t0,
            Duration::from_secs(400),
            Duration::from_secs(5),
            &DnsHint::Unknown,
        )
        .expect("150 s rule");
        assert_eq!(sw.to, ep("secondary"));
        let ts = t0 + Duration::from_secs(200);
        assert!(!sel.primary_probe_due(ts));
        let step = Duration::from_secs(30);
        // Healthy, dead, healthy, dead … never 2 healthy in a row.
        for i in 1..=20u32 {
            let t = ts + step * i;
            assert!(sel.primary_probe_due(t));
            sel.on_primary_probe_at(i % 2 == 1, t, &active("primary"));
            assert_eq!(sel.poll_failback_at(t, &active("primary")), None, "probe #{i}");
        }
        // An earned return is cancelled by a failed probe before it is due.
        let t = ts + step * 21;
        assert!(sel.primary_probe_due(t));
        sel.on_primary_probe_at(true, t, &active("primary"));
        let t = ts + step * 22;
        assert!(sel.primary_probe_due(t));
        sel.on_primary_probe_at(true, t, &active("primary"));
        let due = sel.failback_due_at().expect("earned");
        sel.on_primary_probe_at(false, t, &active("primary"));
        assert_eq!(sel.failback_due_at(), None, "a failed probe cancels the return");
        assert_eq!(sel.poll_failback_at(due + Duration::from_secs(1), &active("primary")), None);
        assert_eq!(sel.current(), &ep("secondary"));
    }

    #[test]
    fn dns_solo_relay_never_switches_and_reports_no_failover() {
        let mut sel = RelaySelector::new(vec![ep("only")]).with_dns_gate(DEFAULT_DNS_MIN_OUTAGE);
        let t0 = Instant::now();
        for i in 0..400u64 {
            assert_eq!(
                sel.on_failure_at(t0 + Duration::from_secs(i), &DnsHint::Unknown),
                FailureDecision::Count
            );
        }
        assert_eq!(sel.failover_status(), None);
    }

    #[test]
    fn legacy_mode_reproduces_0462_exactly() {
        // The switch back to 0.46.2: the *_at entry points in Legacy mode
        // must make the very same decisions as the 0.46.2 methods, at the
        // very same times, whatever hint they are handed — compared step
        // by step against a twin selector driven through the old API.
        let hints = [DnsHint::Unknown, active("primary"), active("secondary")];
        for (h, hint) in hints.iter().enumerate() {
            let mut new = RelaySelector::with_policy_seeded(pair(), 3, FailbackPolicy::DEFAULT, 77);
            let mut old = RelaySelector::with_policy_seeded(pair(), 3, FailbackPolicy::DEFAULT, 77);
            assert_eq!(new.mode(), FailoverMode::Legacy, "constructors start in legacy");
            let t0 = Instant::now();
            let mut switches = 0;
            for s in 0..4_000u64 {
                let t = t0 + Duration::from_secs(s);
                // A failure every 7 s for the first 30 s, then the primary
                // answers probes, with a few dead probes mixed in.
                if s < 30 && s % 7 == 0 {
                    let a = new.on_failure_at(t, hint);
                    let b = old.on_failure();
                    match (&a, &b) {
                        (FailureDecision::Count, None) => {}
                        (FailureDecision::Switch(sw, why), Some(sw2)) => {
                            assert_eq!(sw, sw2);
                            assert!(why.contains("0.46.2"), "{why}");
                            switches += 1;
                        }
                        _ => panic!("hint #{h} at {s}s: legacy {a:?} vs 0.46.2 {b:?}"),
                    }
                }
                let due = new.primary_probe_due(t);
                assert_eq!(due, old.primary_probe_due(t), "due at {s}s");
                if due {
                    // Dead probes first (they must reset the streak), then
                    // healthy ones earn the return.
                    let healthy = s > 1_200;
                    new.on_primary_probe_at(healthy, t, hint);
                    old.on_primary_probe(healthy, t);
                }
                assert_eq!(new.poll_failback_at(t, hint), old.poll_failback(t), "fail-back at {s}s");
                assert_eq!(new.current(), old.current(), "relay at {s}s");
                assert_eq!(new.probe_streak(), old.probe_streak());
                assert_eq!(new.failback_due_at(), old.failback_due_at());
            }
            assert_eq!(switches, 1, "one failover after 3 failures, as in 0.46.2");
            assert_eq!(new.current(), &ep("primary"), "probe-driven fail-back happened");
            assert_eq!(new.failover_status().expect("status").state, "legacy");
        }
        // The 0.46.2 timing constants themselves are untouched.
        assert_eq!(DEFAULT_FAILURE_THRESHOLD, 3);
        assert_eq!(
            FailbackPolicy::DEFAULT,
            FailbackPolicy {
                probe_every: Duration::from_secs(300),
                probe_spread: Duration::from_secs(60),
                healthy_probes: 3,
                failback_delay_max: Duration::from_secs(120),
            }
        );
    }

    #[test]
    fn failover_mode_parses_leniently() {
        assert_eq!(FailoverMode::parse("legacy"), Some(FailoverMode::Legacy));
        assert_eq!(FailoverMode::parse(" Legacy "), Some(FailoverMode::Legacy));
        assert_eq!(FailoverMode::parse("0.46.2"), Some(FailoverMode::Legacy));
        assert_eq!(FailoverMode::parse("dns"), Some(FailoverMode::Dns));
        assert_eq!(FailoverMode::parse("DNS-aware"), Some(FailoverMode::Dns));
        assert_eq!(FailoverMode::parse("bogus"), None);
        assert_eq!(FailoverMode::default(), FailoverMode::Dns, "new behaviour is the default");
    }

    #[test]
    #[should_panic(expected = "at least one relay")]
    fn empty_relay_list_is_a_caller_bug() {
        let _ = RelaySelector::new(Vec::new());
    }
}
