//! Source state and the pure poll maths (contract §3.3). C0 ships the
//! types; `apply_poll` / `check_stale` are B1's (soft/hard, incidents,
//! staleness) and are placeholders until then.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::extract::Projection;
use super::spec::{Json, SourceSpec};

/// `Ord` = severity for "worst of"; unknown is counted apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Health {
    Ok,
    Unknown,
    Warn,
    Crit,
}

impl Health {
    pub fn code(self) -> &'static str {
        match self {
            Health::Ok => "ok",
            Health::Unknown => "unknown",
            Health::Warn => "warn",
            Health::Crit => "crit",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Pending,
    Live,
    Stale,
    Paused,
}

impl Phase {
    pub fn code(self) -> &'static str {
        match self {
            Phase::Pending => "pending",
            Phase::Live => "live",
            Phase::Stale => "stale",
            Phase::Paused => "paused",
        }
    }
}

/// Fixed reasons. Never derived from response text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    HttpAuth,
    #[serde(rename = "http_404")]
    Http404,
    #[serde(rename = "http_4xx")]
    Http4xx,
    #[serde(rename = "http_5xx")]
    Http5xx,
    Timeout,
    Connect,
    Dns,
    Tls,
    BlockedAddress,
    Airgap,
    HostSlept,
    DaemonRestarted,
    TooLarge,
    NotJson,
    RateLimited,
    SecretMissing,
    SecretDisabled,
    VaultUnavailable,
    RedirectOtherOrigin,
    Budget,
    PausedByPerson,
}

impl Reason {
    pub const ALL: &'static [Reason] = &[
        Reason::HttpAuth,
        Reason::Http404,
        Reason::Http4xx,
        Reason::Http5xx,
        Reason::Timeout,
        Reason::Connect,
        Reason::Dns,
        Reason::Tls,
        Reason::BlockedAddress,
        Reason::Airgap,
        Reason::HostSlept,
        Reason::DaemonRestarted,
        Reason::TooLarge,
        Reason::NotJson,
        Reason::RateLimited,
        Reason::SecretMissing,
        Reason::SecretDisabled,
        Reason::VaultUnavailable,
        Reason::RedirectOtherOrigin,
        Reason::Budget,
        Reason::PausedByPerson,
    ];

    /// The stored and wire code (`source_state.reason`, `reason` in JSON).
    pub fn code(self) -> &'static str {
        match self {
            Reason::HttpAuth => "http_auth",
            Reason::Http404 => "http_404",
            Reason::Http4xx => "http_4xx",
            Reason::Http5xx => "http_5xx",
            Reason::Timeout => "timeout",
            Reason::Connect => "connect",
            Reason::Dns => "dns",
            Reason::Tls => "tls",
            Reason::BlockedAddress => "blocked_address",
            Reason::Airgap => "airgap",
            Reason::HostSlept => "host_slept",
            Reason::DaemonRestarted => "daemon_restarted",
            Reason::TooLarge => "too_large",
            Reason::NotJson => "not_json",
            Reason::RateLimited => "rate_limited",
            Reason::SecretMissing => "secret_missing",
            Reason::SecretDisabled => "secret_disabled",
            Reason::VaultUnavailable => "vault_unavailable",
            Reason::RedirectOtherOrigin => "redirect_other_origin",
            Reason::Budget => "budget",
            Reason::PausedByPerson => "paused_by_person",
        }
    }

    pub fn from_code(code: &str) -> Option<Reason> {
        Reason::ALL.iter().copied().find(|r| r.code() == code)
    }

    /// One plain sentence for cards, Settings and trigger notes.
    pub fn sentence(self) -> &'static str {
        match self {
            Reason::HttpAuth => "The site refused the credentials (401 or 403).",
            Reason::Http404 => "The address was not found (404).",
            Reason::Http4xx => "The site refused the request (4xx).",
            Reason::Http5xx => "The site had a server error (5xx).",
            Reason::Timeout => "The site did not answer in time.",
            Reason::Connect => "Could not connect to the site.",
            Reason::Dns => "The site's name did not resolve.",
            Reason::Tls => "The site's HTTPS certificate was not accepted.",
            Reason::BlockedAddress => "The address is a private or local one, which sources can't reach.",
            Reason::Airgap => "This server is air-gapped, so it makes no outside checks.",
            Reason::HostSlept => "The computer running this check was asleep.",
            Reason::DaemonRestarted => "K2 restarted on the computer running this check.",
            Reason::TooLarge => "The answer was larger than this source allows.",
            Reason::NotJson => "The answer was not JSON.",
            Reason::RateLimited => "The site asked K2 to slow down.",
            Reason::SecretMissing => "The secret this source uses is missing.",
            Reason::SecretDisabled => "The secret this source uses is disabled or no longer shared with it.",
            Reason::VaultUnavailable => "The vault is locked or unavailable.",
            Reason::RedirectOtherOrigin => "The site redirected to a different address.",
            Reason::Budget => "K2 is holding checks to this site to stay within its rate.",
            Reason::PausedByPerson => "A person paused this source.",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GapKind {
    HostSlept,
    DaemonRestarted,
}

impl GapKind {
    pub fn code(self) -> &'static str {
        match self {
            GapKind::HostSlept => "host_slept",
            GapKind::DaemonRestarted => "daemon_restarted",
        }
    }
}

/// One fetch's result as the core sees it. The daemon builds it AFTER
/// redaction.
#[derive(Debug, Clone, PartialEq)]
pub enum FetchOutcome {
    Ok {
        status: u16,
        body: Json,
        not_modified: bool,
        etag: Option<String>,
        last_modified: Option<String>,
        elapsed_ms: u32,
    },
    Err {
        reason: Reason,
        http_status: Option<u16>,
        retry_after_secs: Option<u32>,
    },
}

/// One poll cycle: a single-url source has one outcome with key None;
/// fan-out has one per target key.
pub struct PollInput<'a> {
    pub spec: &'a SourceSpec,
    pub outcomes: Vec<(Option<String>, FetchOutcome)>,
    /// Wall seconds.
    pub now: i64,
    /// Never true here: simulation is a view overlay.
    pub simulated: bool,
}

/// `{ok, warn, crit, unknown}` item counts (`source_state.counts_json`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub ok: u32,
    pub warn: u32,
    pub crit: u32,
    pub unknown: u32,
}

/// Persisted state, mirrors `source_state` (one row per source). The
/// stored projection (`projection_json`) travels as [`Projection`] next to
/// it, never inside it.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceState {
    pub source_id: String,
    pub health: Health,
    pub phase: Phase,
    pub reason: Option<Reason>,
    pub http_status: Option<u16>,
    pub soft_health: Option<Health>,
    /// Source level (single-url and fields/aggregate rules).
    pub soft_count: u32,
    /// Consecutive cycles with no success on any target.
    pub fetch_failures: u32,
    pub backoff_k: u32,
    pub hard_since: Option<i64>,
    pub recovered_at: Option<i64>,
    /// Last success on any target.
    pub updated_at: Option<i64>,
    /// Last attempt.
    pub checked_at: Option<i64>,
    /// Display only; the scheduler uses `Instant`.
    pub next_at: Option<i64>,
    pub gap_kind: Option<GapKind>,
    pub gap_from: Option<i64>,
    pub gap_to: Option<i64>,
    /// Open source-level incident.
    pub incident_id: Option<String>,
    pub counts: Counts,
    /// Single-url sources only.
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    /// Parked after a vault denial [GV68].
    pub vault_denied: bool,
    pub rev: i64,
    /// `source_items` rows by `item_key`.
    pub items: BTreeMap<String, ItemState>,
}

impl SourceState {
    /// The row a new source starts with: unknown, pending.
    pub fn new(source_id: &str) -> SourceState {
        SourceState {
            source_id: source_id.to_string(),
            health: Health::Unknown,
            phase: Phase::Pending,
            reason: None,
            http_status: None,
            soft_health: None,
            soft_count: 0,
            fetch_failures: 0,
            backoff_k: 0,
            hard_since: None,
            recovered_at: None,
            updated_at: None,
            checked_at: None,
            next_at: None,
            gap_kind: None,
            gap_from: None,
            gap_to: None,
            incident_id: None,
            counts: Counts::default(),
            etag: None,
            last_modified: None,
            vault_denied: false,
            rev: 0,
            items: BTreeMap::new(),
        }
    }
}

/// Mirrors `source_items`: fan-out targets (`is_target`) and pointer items.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemState {
    pub item_key: String,
    pub is_target: bool,
    /// A fan-out target's origin.
    pub origin: Option<String>,
    pub health: Health,
    pub soft_health: Option<Health>,
    pub soft_count: u32,
    pub reason: Option<Reason>,
    pub http_status: Option<u16>,
    pub hard_since: Option<i64>,
    pub recovered_at: Option<i64>,
    pub updated_at: Option<i64>,
    pub checked_at: Option<i64>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub incident_id: Option<String>,
}

/// `source_events.kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionKind {
    WentOk,
    WentWarn,
    WentCrit,
    WentUnknown,
    Recovered,
    WentStale,
    WentLive,
    Paused,
    Resumed,
    Gap,
}

impl TransitionKind {
    pub fn code(self) -> &'static str {
        match self {
            TransitionKind::WentOk => "went_ok",
            TransitionKind::WentWarn => "went_warn",
            TransitionKind::WentCrit => "went_crit",
            TransitionKind::WentUnknown => "went_unknown",
            TransitionKind::Recovered => "recovered",
            TransitionKind::WentStale => "went_stale",
            TransitionKind::WentLive => "went_live",
            TransitionKind::Paused => "paused",
            TransitionKind::Resumed => "resumed",
            TransitionKind::Gap => "gap",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Transition {
    /// None = source level.
    pub scope_item: Option<String>,
    pub from: Health,
    pub to: Health,
    pub kind: TransitionKind,
    /// Opened, continued or closed.
    pub incident_id: Option<String>,
    pub reason: Option<Reason>,
    /// Only hard transitions are ever passed to triggers.
    pub hard: bool,
}

/// What [`apply_poll`] returns.
#[derive(Debug, Clone)]
pub struct PollApplied {
    pub state: SourceState,
    pub projection: Option<Projection>,
    pub transitions: Vec<Transition>,
    /// Trend points `(series, value)`.
    pub points: Vec<(String, f64)>,
}

/// Pure. Applies one cycle, returns the new state, the projection to
/// store, the transitions and trend points.
///
/// C0 skeleton: returns the previous state unchanged with no transitions
/// (B1 replaces it).
pub fn apply_poll(prev: &SourceState, input: PollInput) -> PollApplied {
    let _ = input;
    PollApplied { state: prev.clone(), projection: None, transitions: Vec::new(), points: Vec::new() }
}

/// Pure. A detected gap (sleep or restart): sets phase Paused + gap
/// fields; no transitions that triggers act on.
pub fn apply_gap(prev: &SourceState, kind: GapKind, from: i64, to: i64) -> SourceState {
    let mut s = prev.clone();
    s.phase = Phase::Paused;
    s.gap_kind = Some(kind);
    s.gap_from = Some(from);
    s.gap_to = Some(to);
    s
}

/// Pure. Time-only staleness check, run every scheduler tick:
/// `max(3*every, every+120)` since `updated_at`.
///
/// C0 skeleton: never stale (B1 replaces it).
pub fn check_stale(prev: &SourceState, spec: &SourceSpec, now: i64) -> Option<(SourceState, Transition)> {
    let _ = (prev, spec, now);
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reason_codes_match_serde_and_round_trip() {
        for r in Reason::ALL {
            assert_eq!(serde_json::to_value(r).unwrap(), serde_json::json!(r.code()), "{r:?}");
            assert_eq!(Reason::from_code(r.code()), Some(*r));
            assert!(!r.sentence().is_empty());
        }
        assert_eq!(Reason::Http404.code(), "http_404");
    }

    #[test]
    fn health_orders_by_severity() {
        assert!(Health::Crit > Health::Warn && Health::Warn > Health::Unknown && Health::Unknown > Health::Ok);
        let worst = [Health::Ok, Health::Warn, Health::Unknown].into_iter().max();
        assert_eq!(worst, Some(Health::Warn));
    }

    #[test]
    fn a_gap_pauses_and_records_its_window() {
        let s = apply_gap(&SourceState::new("s1"), GapKind::HostSlept, 100, 500);
        assert_eq!(s.phase, Phase::Paused);
        assert_eq!((s.gap_kind, s.gap_from, s.gap_to), (Some(GapKind::HostSlept), Some(100), Some(500)));
    }
}
