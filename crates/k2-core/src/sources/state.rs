//! Source state and the pure poll maths (contract §3.3, PRD §5.5): per-item
//! soft/hard states [GV55, GV56], the source-level reading, incidents,
//! staleness and gaps.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::extract::{self, Projection};
use super::rules;
use super::spec::{FetchTargets, Json, Keep, OnTargetError, SourceSpec};

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
    /// `expect.status` didn't list the status (0.46.1).
    UnexpectedStatus,
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
        Reason::UnexpectedStatus,
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
            Reason::UnexpectedStatus => "unexpected_status",
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
            Reason::UnexpectedStatus => "The site answered with a status this source doesn't expect.",
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
/// fan-out has one per target key. Targets left out of `outcomes` weren't
/// fetched this cycle (budget) and keep their state.
pub struct PollInput<'a> {
    pub spec: &'a SourceSpec,
    pub outcomes: Vec<(Option<String>, FetchOutcome)>,
    /// Wall seconds.
    pub now: i64,
    /// Never true here: simulation is a view overlay.
    pub simulated: bool,
    /// The stored projection (a `304` keeps it; a failed target keeps its
    /// last good fields).
    pub prev_projection: Option<&'a Projection>,
}

/// `{ok, warn, crit, unknown}` item counts (`source_state.counts_json`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub ok: u32,
    pub warn: u32,
    pub crit: u32,
    pub unknown: u32,
}

impl Counts {
    pub fn add(&mut self, h: Health) {
        match h {
            Health::Ok => self.ok += 1,
            Health::Warn => self.warn += 1,
            Health::Crit => self.crit += 1,
            Health::Unknown => self.unknown += 1,
        }
    }
}

/// Persisted state, mirrors `source_state` (one row per source). The
/// stored projection (`projection_json`) travels as [`Projection`] next to
/// it, never inside it.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceState {
    pub source_id: String,
    /// The card's health: the worst of the source-level reading and the
    /// items' hard states, unknown counted apart.
    pub health: Health,
    pub phase: Phase,
    pub reason: Option<Reason>,
    pub http_status: Option<u16>,
    /// Source-level soft reading (single-url fetch, field/aggregate rules).
    pub soft_health: Option<Health>,
    pub soft_count: u32,
    /// Source-level hard reading; `None` until first rated.
    pub self_health: Option<Health>,
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
            self_health: None,
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

    /// Something is waiting on a soft re-check (source level or any item):
    /// a reading that hasn't gone hard yet.
    pub fn has_soft(&self) -> bool {
        self.soft_health.is_some() || self.items.values().any(|i| i.soft_health.is_some())
    }

    /// The first poll after a gap is computed against a gap (triggers
    /// suppress with `gap`).
    pub fn in_gap(&self) -> bool {
        self.phase == Phase::Paused && self.gap_kind.is_some()
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

impl ItemState {
    pub fn new(key: &str, is_target: bool) -> ItemState {
        ItemState {
            item_key: key.to_string(),
            is_target,
            origin: None,
            health: Health::Unknown,
            soft_health: None,
            soft_count: 0,
            reason: None,
            http_status: None,
            hard_since: None,
            recovered_at: None,
            updated_at: None,
            checked_at: None,
            etag: None,
            last_modified: None,
            incident_id: None,
        }
    }
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
    /// What to store now; `None` keeps the stored projection (a failed
    /// single-url fetch).
    pub projection: Option<Projection>,
    pub transitions: Vec<Transition>,
    /// Trend points `(series, value)`.
    pub points: Vec<(String, f64)>,
}

/// Series a source keeps history for (0.46.1): its `extract.trend` first,
/// then every count and aggregate, up to `MAX_HISTORY_SERIES`. Never for
/// `keep: "states"`.
pub fn history_series(spec: &SourceSpec) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let names = spec.extract.trend.iter().chain(spec.extract.counts.keys()).chain(spec.extract.aggregates.keys());
    for n in names {
        if out.len() >= super::MAX_HISTORY_SERIES {
            break;
        }
        if !out.contains(n) {
            out.push(n.clone());
        }
    }
    out
}

/// `max(3 × every, every + 120)`.
pub fn stale_window(spec: &SourceSpec) -> i64 {
    let every = i64::from(spec.every_secs);
    (3 * every).max(every + 120)
}

fn new_incident() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn kind_for(from: Health, to: Health) -> TransitionKind {
    match to {
        Health::Crit => TransitionKind::WentCrit,
        Health::Warn => TransitionKind::WentWarn,
        Health::Unknown => TransitionKind::WentUnknown,
        Health::Ok if matches!(from, Health::Warn | Health::Crit) => TransitionKind::Recovered,
        Health::Ok => TransitionKind::WentOk,
    }
}

/// One soft/hard step (Checkmk style, GV56; 0.46.1 thresholds). `ok`
/// applies at once and ends the streak. Any other reading adds one to the
/// streak of checks in a row that weren't ok (`count`), and goes hard when
/// the streak reaches its threshold: crit at `crit_after`, warn (also for a
/// crit reading that hasn't reached `crit_after` yet) at `warn_after`,
/// unknown at `warn_after`. Until then the hard state stays and `soft`
/// holds the waiting reading. Fetch failures use `failAfter` for both;
/// rule readings use their rules' `warnAfter`/`critAfter` (default 1, at
/// once). Returns the hard change.
fn step(hard: Health, soft: &mut Option<Health>, count: &mut u32, r: &Reading) -> Option<Health> {
    if r.health == Health::Ok {
        *soft = None;
        *count = 0;
        return (hard != Health::Ok).then_some(Health::Ok);
    }
    *count = count.saturating_add(1);
    if r.health == hard {
        *soft = None;
        return None;
    }
    let reached = match r.health {
        Health::Crit if *count >= r.crit_after => Some(Health::Crit),
        Health::Crit | Health::Warn if *count >= r.warn_after => Some(Health::Warn),
        Health::Unknown if *count >= r.warn_after => Some(Health::Unknown),
        _ => None,
    };
    let new_hard = reached.unwrap_or(hard);
    *soft = (new_hard != r.health).then_some(r.health);
    (new_hard != hard).then_some(new_hard)
}

/// The threshold the view shows next to a waiting reading (`soft.of`).
/// `fetch_failed` also covers a source without per-rule thresholds, whose
/// rule readings wait `failAfter` too.
pub fn soft_of(soft: Option<Health>, rated: Option<rules::Rated>, fetch_failed: bool, fail_after: u32) -> u32 {
    if fetch_failed {
        return fail_after;
    }
    match (soft, rated) {
        (Some(Health::Crit), Some(r)) => r.crit_after,
        (Some(Health::Warn), Some(r)) => r.warn_after,
        (Some(Health::Unknown), _) => fail_after,
        _ => 1,
    }
}

/// Open, continue or close an incident for a hard change; returns the id
/// the transition carries.
fn incident(slot: &mut Option<String>, to: Health) -> Option<String> {
    match to {
        Health::Warn | Health::Crit => {
            if slot.is_none() {
                *slot = Some(new_incident());
            }
            slot.clone()
        }
        Health::Ok => slot.take(),
        // No data isn't recovery: the incident stays open.
        Health::Unknown => slot.clone(),
    }
}

fn health_of(ote: OnTargetError) -> Health {
    match ote {
        OnTargetError::Crit => Health::Crit,
        OnTargetError::Unknown => Health::Unknown,
    }
}

/// A reading: health plus the fixed reason and status that explain it,
/// and the checks in a row it needs before it goes hard.
#[derive(Debug, Clone, Copy)]
struct Reading {
    health: Health,
    reason: Option<Reason>,
    http_status: Option<u16>,
    warn_after: u32,
    crit_after: u32,
}

impl Reading {
    /// A fetch failure: hard after `failAfter`.
    fn failed(health: Health, reason: Reason, http_status: Option<u16>, fail_after: u32) -> Reading {
        Reading { health, reason: Some(reason), http_status, warn_after: fail_after, crit_after: fail_after }
    }

    /// A rule reading. Unknown (no rule matched, or a value went missing)
    /// waits `failAfter`. With per-rule thresholds on (`thresholds`), warn
    /// and crit use the rules' own `warnAfter`/`critAfter` (default 1), so
    /// `failAfter` never delays them; off (the pre-0.46.1 default), every
    /// non-ok rule reading waits `failAfter`, as it always did.
    fn rated(r: rules::Rated, http_status: Option<u16>, fail_after: u32, thresholds: bool) -> Reading {
        let (w, c) = if r.health == Health::Unknown || !thresholds {
            (fail_after, fail_after)
        } else {
            (r.warn_after, r.crit_after)
        };
        Reading { health: r.health, reason: None, http_status, warn_after: w, crit_after: c }
    }
}

/// Pure. Applies one cycle, returns the new state, the projection to
/// store, the transitions and trend points.
///
/// - A fan-out target that fails makes its own item red (or unknown with
///   `onTargetError: unknown`) and leaves the source live [GV55]; the
///   source counts as failing only when every target fails.
/// - Fetch failures go hard after `failAfter` (§5.5). Rule readings wait
///   `failAfter` too, unless the source opts in to per-rule thresholds
///   (any rule sets `warnAfter`/`critAfter`, 0.46.1): then each rule's own
///   thresholds, default at once. Only
///   hard changes make health transitions, and they open/continue/close
///   incidents.
/// - `keep: "states"` returns a keys-only projection and no points.
pub fn apply_poll(prev: &SourceState, input: PollInput) -> PollApplied {
    let spec = input.spec;
    let now = input.now;
    let fail_after = u32::from(spec.fail_after.max(1));
    let thresholds = spec.uses_rule_thresholds();
    let mut st = prev.clone();
    let mut transitions = Vec::new();
    if input.outcomes.is_empty() {
        return PollApplied { state: st, projection: None, transitions, points: Vec::new() };
    }
    st.checked_at = Some(now);
    let any_success = input.outcomes.iter().any(|(_, o)| matches!(o, FetchOutcome::Ok { .. }));
    let empty = Projection::default();
    let prev_proj = input.prev_projection.unwrap_or(&empty);

    let mut item_readings: BTreeMap<String, Reading> = BTreeMap::new();
    let mut self_reading: Option<Reading> = None;
    let projection: Option<Projection>;

    match &spec.fetch {
        FetchTargets::Targets(targets) => {
            let outcomes: BTreeMap<&str, &FetchOutcome> =
                input.outcomes.iter().filter_map(|(k, o)| k.as_deref().map(|k| (k, o))).collect();
            // Fresh bodies make fresh fields; 304s, failures and targets not
            // fetched this cycle keep their last good fields.
            let fresh: Vec<(String, Option<&Json>)> = targets
                .iter()
                .filter_map(|t| match outcomes.get(t.key.as_str()) {
                    Some(FetchOutcome::Ok { body, not_modified: false, .. }) => Some((t.key.clone(), Some(body))),
                    _ => None,
                })
                .collect();
            let fresh_p = extract::extract_fanout(&spec.extract, &fresh, now);
            let mut p = Projection::default();
            for t in targets.iter().take(super::MAX_ITEMS) {
                let fields = fresh_p
                    .item(&t.key)
                    .or_else(|| prev_proj.item(&t.key))
                    .map(|i| i.fields.clone())
                    .unwrap_or_default();
                p.items.push(extract::ProjectedItem { key: t.key.clone(), fields, list: None });
            }
            extract::add_aggregates(&spec.extract, &mut p, now);
            let rated = rules::rate_items(spec, &p, now);
            for t in targets.iter().take(super::MAX_ITEMS) {
                let item = st.items.entry(t.key.clone()).or_insert_with(|| ItemState::new(&t.key, true));
                item.is_target = true;
                if item.origin.is_none() {
                    item.origin = super::lint::origin_of(&t.url, "", true).ok().map(|p| p.origin);
                }
                match outcomes.get(t.key.as_str()) {
                    Some(FetchOutcome::Ok { status, etag, last_modified, not_modified, .. }) => {
                        item.updated_at = Some(now);
                        item.checked_at = Some(now);
                        if !*not_modified {
                            item.etag = etag.clone();
                            item.last_modified = last_modified.clone();
                        }
                        let h = rated.get(&t.key).copied().unwrap_or(rules::Rated::now(Health::Ok));
                        item_readings.insert(t.key.clone(), Reading::rated(h, Some(*status), fail_after, thresholds));
                    }
                    Some(FetchOutcome::Err { reason, http_status, .. }) => {
                        item.checked_at = Some(now);
                        item_readings.insert(
                            t.key.clone(),
                            Reading::failed(health_of(spec.on_target_error), *reason, *http_status, fail_after),
                        );
                    }
                    None => {}
                }
            }
            let keys: Vec<&str> = targets.iter().map(|t| t.key.as_str()).collect();
            st.items.retain(|k, _| keys.contains(&k.as_str()));
            if any_success {
                let h = rules::rate_source(spec, &p, now).unwrap_or(rules::Rated::now(Health::Ok));
                self_reading = Some(Reading::rated(h, None, fail_after, thresholds));
            }
            projection = Some(p);
        }
        FetchTargets::Url(_) | FetchTargets::UrlSecret { .. } => {
            let (_, outcome) = &input.outcomes[0];
            match outcome {
                FetchOutcome::Ok { status, body, not_modified, etag, last_modified, .. } => {
                    let p = if *not_modified {
                        prev_proj.clone()
                    } else {
                        st.etag = etag.clone();
                        st.last_modified = last_modified.clone();
                        extract::extract_single(&spec.extract, body, now)
                    };
                    for (k, h) in rules::rate_items(spec, &p, now) {
                        item_readings.insert(k, Reading::rated(h, None, fail_after, thresholds));
                    }
                    let keys: Vec<String> = p.items.iter().map(|i| i.key.clone()).collect();
                    st.items.retain(|k, _| keys.contains(k));
                    for k in &keys {
                        let item = st.items.entry(k.clone()).or_insert_with(|| ItemState::new(k, false));
                        item.updated_at = Some(now);
                        item.checked_at = Some(now);
                    }
                    let h = rules::rate_source(spec, &p, now).unwrap_or(rules::Rated::now(Health::Ok));
                    self_reading = Some(Reading::rated(h, Some(*status), fail_after, thresholds));
                    projection = Some(p);
                }
                FetchOutcome::Err { reason, http_status, .. } => {
                    self_reading = Some(Reading::failed(health_of(spec.on_target_error), *reason, *http_status, fail_after));
                    projection = None;
                }
            }
        }
    }

    // Items: soft/hard per item.
    for (key, r) in &item_readings {
        let Some(item) = st.items.get_mut(key) else { continue };
        let before = item.health;
        if let Some(to) = step(before, &mut item.soft_health, &mut item.soft_count, r) {
            let incident_id = incident(&mut item.incident_id, to);
            item.health = to;
            item.hard_since = (to != Health::Ok).then_some(now);
            if to == Health::Ok && matches!(before, Health::Warn | Health::Crit) {
                item.recovered_at = Some(now);
            }
            transitions.push(Transition {
                scope_item: Some(key.clone()),
                from: before,
                to,
                kind: kind_for(before, to),
                incident_id,
                reason: r.reason,
                hard: true,
            });
        }
        item.reason = if r.health == Health::Ok { None } else { r.reason };
        item.http_status = r.http_status;
    }

    // Source level: its own reading.
    if let Some(r) = self_reading {
        let hard = st.self_health.unwrap_or(Health::Unknown);
        if let Some(to) = step(hard, &mut st.soft_health, &mut st.soft_count, &r) {
            st.self_health = Some(to);
        } else if st.self_health.is_none() && r.health == Health::Unknown {
            st.self_health = Some(Health::Unknown);
        }
        st.http_status = r.http_status;
    }

    // The card's health and its transition.
    let item_hards = st.items.values().map(|i| i.health);
    let combined = rules::worst(st.self_health.into_iter().chain(item_hards), Health::Unknown);
    let mut counts = Counts::default();
    for i in st.items.values() {
        counts.add(i.health);
    }
    st.counts = counts;
    let self_failing_reason = match self_reading {
        Some(r) if r.health != Health::Ok => r.reason,
        _ => None,
    };
    let worst_item_reason = st
        .items
        .values()
        .filter(|i| i.health == combined && matches!(combined, Health::Warn | Health::Crit))
        .find_map(|i| i.reason);
    st.reason = self_failing_reason.or(worst_item_reason);
    if combined != prev.health {
        let incident_id = incident(&mut st.incident_id, combined);
        st.hard_since = (combined != Health::Ok).then_some(now);
        if combined == Health::Ok && matches!(prev.health, Health::Warn | Health::Crit) {
            st.recovered_at = Some(now);
        }
        transitions.push(Transition {
            scope_item: None,
            from: prev.health,
            to: combined,
            kind: kind_for(prev.health, combined),
            incident_id,
            reason: st.reason,
            hard: true,
        });
        st.health = combined;
    }

    // Phase and failure counters.
    if any_success {
        if prev.phase != Phase::Live {
            transitions.push(Transition {
                scope_item: None,
                from: st.health,
                to: st.health,
                kind: TransitionKind::WentLive,
                incident_id: None,
                reason: None,
                hard: false,
            });
        }
        st.phase = Phase::Live;
        st.updated_at = Some(now);
        st.fetch_failures = 0;
        st.backoff_k = 0;
        st.vault_denied = false;
    } else {
        st.fetch_failures += 1;
        st.backoff_k = st.fetch_failures.saturating_sub(fail_after);
        let stale = match st.updated_at {
            Some(u) => now - u >= stale_window(spec),
            None => st.fetch_failures >= fail_after,
        };
        if stale && st.phase != Phase::Stale {
            transitions.push(Transition {
                scope_item: None,
                from: st.health,
                to: st.health,
                kind: TransitionKind::WentStale,
                incident_id: Some(stale_incident(&st.source_id, st.updated_at)),
                reason: st.reason,
                hard: true,
            });
            st.phase = Phase::Stale;
        } else if st.phase == Phase::Paused {
            // The first attempt after a gap ends the gap display.
            st.phase = if st.updated_at.is_some() { Phase::Live } else { Phase::Pending };
        }
    }
    if st.phase != Phase::Paused {
        st.gap_kind = None;
        st.gap_from = None;
        st.gap_to = None;
    }

    // History points (never for keep: states).
    let mut points = Vec::new();
    if spec.keep == Keep::Values && any_success {
        if let Some(p) = &projection {
            for t in &history_series(spec) {
                let v = match p.fields.get(t) {
                    Some(Json::Number(n)) => n.as_f64(),
                    Some(Json::Bool(b)) => Some(if *b { 1.0 } else { 0.0 }),
                    _ => None,
                };
                if let Some(v) = v {
                    points.push((t.clone(), v));
                }
            }
        }
    }
    let projection = match (projection, spec.keep) {
        (Some(p), Keep::States) => Some(p.keys_only()),
        (p, _) => p,
    };
    PollApplied { state: st, projection, transitions, points }
}

/// The id a stale episode carries: one per `updated_at`, so a `stale_for`
/// trigger fires once per episode.
pub fn stale_incident(source_id: &str, updated_at: Option<i64>) -> String {
    format!("stale:{source_id}:{}", updated_at.unwrap_or(0))
}

/// When the current stale episode began (`updated_at + window`), if stale.
pub fn stale_since(st: &SourceState, spec: &SourceSpec) -> Option<i64> {
    (st.phase == Phase::Stale).then(|| st.updated_at.map(|u| u + stale_window(spec))).flatten()
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
/// `max(3*every, every+120)` since `updated_at`. Only a live source goes
/// stale here (a paused or gapped one doesn't). The transition is hard and
/// carries the stale episode's id.
pub fn check_stale(prev: &SourceState, spec: &SourceSpec, now: i64) -> Option<(SourceState, Transition)> {
    if prev.phase != Phase::Live {
        return None;
    }
    let u = prev.updated_at?;
    if now - u < stale_window(spec) {
        return None;
    }
    let mut s = prev.clone();
    s.phase = Phase::Stale;
    let t = Transition {
        scope_item: None,
        from: prev.health,
        to: prev.health,
        kind: TransitionKind::WentStale,
        incident_id: Some(stale_incident(&prev.source_id, prev.updated_at)),
        reason: prev.reason,
        hard: true,
    };
    Some((s, t))
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
