//! Trigger model and evaluation (contract §3.5, §9). Owner: B2. Pure: the
//! daemon's wake pipeline (`k2_daemon::sources::wake`) gathers the counts
//! into a [`FireContext`] and acts on the [`FireDecision`].

use serde::{Deserialize, Serialize};

use super::spec::SourceStatus;
use super::state::{Phase, SourceState, Transition, TransitionKind};
use super::store::SourceRow;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TriggerOn {
    WentCrit,
    WentWarn,
    Recovered,
    StaleFor { secs: u32 },
    ItemWentCrit,
}

impl TriggerOn {
    /// `source_triggers.on_kind`.
    pub fn code(&self) -> &'static str {
        match self {
            TriggerOn::WentCrit => "went_crit",
            TriggerOn::WentWarn => "went_warn",
            TriggerOn::Recovered => "recovered",
            TriggerOn::StaleFor { .. } => "stale_for",
            TriggerOn::ItemWentCrit => "item_went_crit",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TriggerStatus {
    Proposed,
    Active,
    Paused,
    Undeliverable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TriggerSpec {
    pub id: String,
    pub source_id: String,
    pub on: TriggerOn,
    pub target_project_id: String,
    pub note: Option<String>,
    /// 1800.
    pub cooldown_secs: u32,
    /// 12.
    pub daily_cap: u16,
    pub status: TriggerStatus,
    pub made_by: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FireDecision {
    Fire { incident_id: String, item_key: Option<String> },
    Suppress { reason: SuppressReason, incident_id: String, item_key: Option<String> },
    /// Not this trigger's kind, or a soft transition.
    Ignore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuppressReason {
    Cooldown,
    DailyCap,
    AgentCap,
    Muted,
    Simulated,
    Paused,
    Gap,
    Deduped,
    Draining,
}

impl SuppressReason {
    /// `source_trigger_fires.suppress_reason`.
    pub fn code(self) -> &'static str {
        match self {
            SuppressReason::Cooldown => "cooldown",
            SuppressReason::DailyCap => "daily_cap",
            SuppressReason::AgentCap => "agent_cap",
            SuppressReason::Muted => "muted",
            SuppressReason::Simulated => "simulated",
            SuppressReason::Paused => "paused",
            SuppressReason::Gap => "gap",
            SuppressReason::Deduped => "deduped",
            SuppressReason::Draining => "draining",
        }
    }
}

pub struct FireContext<'a> {
    pub source: &'a SourceRow,
    pub state: &'a SourceState,
    pub now: i64,
    pub last_fire_at: Option<i64>,
    pub fires_today: u16,
    pub agent_fires_last_hour: u16,
    pub already_fired_incident: bool,
    pub in_gap: bool,
    pub draining: bool,
}

impl TriggerOn {
    /// `source_triggers.stale_for_secs`.
    pub fn stale_for_secs(&self) -> Option<u32> {
        match self {
            TriggerOn::StaleFor { secs } => Some(*secs),
            _ => None,
        }
    }

    /// From the stored columns (`on_kind`, `stale_for_secs`).
    pub fn from_parts(kind: &str, stale_for_secs: Option<i64>) -> Option<TriggerOn> {
        Some(match kind {
            "went_crit" => TriggerOn::WentCrit,
            "went_warn" => TriggerOn::WentWarn,
            "recovered" => TriggerOn::Recovered,
            "item_went_crit" => TriggerOn::ItemWentCrit,
            "stale_for" => TriggerOn::StaleFor { secs: u32::try_from(stale_for_secs?).ok()? },
            _ => return None,
        })
    }
}

impl TriggerStatus {
    pub fn code(self) -> &'static str {
        match self {
            TriggerStatus::Proposed => "proposed",
            TriggerStatus::Active => "active",
            TriggerStatus::Paused => "paused",
            TriggerStatus::Undeliverable => "undeliverable",
        }
    }

    pub fn from_code(s: &str) -> Option<TriggerStatus> {
        Some(match s {
            "proposed" => TriggerStatus::Proposed,
            "active" => TriggerStatus::Active,
            "paused" => TriggerStatus::Paused,
            "undeliverable" => TriggerStatus::Undeliverable,
            _ => return None,
        })
    }
}

/// Default cooldown (30 min) and its range (5 min – 1 day).
pub const DEFAULT_COOLDOWN_SECS: u32 = 1800;
pub const MIN_COOLDOWN_SECS: u32 = 300;
pub const MAX_COOLDOWN_SECS: u32 = 86_400;
/// Default daily cap and its range.
pub const DEFAULT_DAILY_CAP: u16 = 12;
pub const MAX_DAILY_CAP: u16 = 48;
/// `stale_for` range: 5 min – 7 days.
pub const MIN_STALE_FOR_SECS: u32 = 300;
pub const MAX_STALE_FOR_SECS: u32 = 604_800;
/// Owner's note, characters.
pub const MAX_TRIGGER_NOTE: usize = 500;

/// The stale window: `max(3 × every, every + 120)` seconds after the last
/// success (PRD §5.5; B1's `check_stale` uses the same rule).
pub fn stale_window_secs(every_secs: u32) -> i64 {
    let e = i64::from(every_secs);
    (3 * e).max(e + 120)
}

/// When the source went stale (wall seconds), or None while it isn't.
/// A gap pauses the source (phase `paused`), so gaps never count.
pub fn stale_since(row: &SourceRow, st: &SourceState) -> Option<i64> {
    if st.phase != Phase::Stale {
        return None;
    }
    st.updated_at.map(|u| u + stale_window_secs(row.every_secs))
}

/// The incident a `stale_for` fire belongs to. One stale episode = one
/// incident: the last success doesn't move until the source answers again.
pub fn stale_incident_id(source_id: &str, st: &SourceState) -> String {
    format!("stale:{source_id}:{}", st.updated_at.unwrap_or(0))
}

/// Does this trigger act on this transition (kind and scope only)?
pub fn matches(on: &TriggerOn, tr: &Transition) -> bool {
    match on {
        TriggerOn::WentCrit => tr.scope_item.is_none() && tr.kind == TransitionKind::WentCrit,
        TriggerOn::WentWarn => tr.scope_item.is_none() && tr.kind == TransitionKind::WentWarn,
        TriggerOn::Recovered => tr.scope_item.is_none() && tr.kind == TransitionKind::Recovered,
        TriggerOn::ItemWentCrit => tr.scope_item.is_some() && tr.kind == TransitionKind::WentCrit,
        TriggerOn::StaleFor { .. } => tr.scope_item.is_none() && tr.kind == TransitionKind::WentStale,
    }
}

/// First match wins (contract §9 "evaluate order"):
/// 1. not active, a soft transition or another kind → `Ignore` (a
///    `stale_for` trigger also waits until the source has been stale that
///    long); 2. draining; 3. gap; 4. paused, simulated, muted; 5. already
///    fired for this incident; 6. cooldown; 7. daily cap; 8. 30 an hour per
///    agent; 9. fire.
pub fn evaluate(t: &TriggerSpec, tr: &Transition, cx: &FireContext) -> FireDecision {
    if t.status != TriggerStatus::Active || !tr.hard || !matches(&t.on, tr) {
        return FireDecision::Ignore;
    }
    if let TriggerOn::StaleFor { secs } = t.on {
        match stale_since(cx.source, cx.state) {
            Some(since) if cx.now - since >= i64::from(secs) => {}
            _ => return FireDecision::Ignore,
        }
    }
    let incident_id = match (&t.on, &tr.incident_id) {
        (TriggerOn::StaleFor { .. }, None) => stale_incident_id(&cx.source.id, cx.state),
        (_, Some(id)) => id.clone(),
        // A hard transition with no incident is a state-maths slip; key the
        // fire on the moment so it still dedupes and is still logged.
        (_, None) => format!("{}:{}:{}", tr.kind.code(), tr.scope_item.as_deref().unwrap_or(""), cx.now),
    };
    let item_key = tr.scope_item.clone();
    let suppress =
        |reason| FireDecision::Suppress { reason, incident_id: incident_id.clone(), item_key: item_key.clone() };
    if cx.draining {
        return suppress(SuppressReason::Draining);
    }
    if cx.in_gap {
        return suppress(SuppressReason::Gap);
    }
    if let Some(r) = window_suppression(cx.source, cx.now) {
        return suppress(r);
    }
    if cx.already_fired_incident {
        return suppress(SuppressReason::Deduped);
    }
    if cx.last_fire_at.is_some_and(|last| cx.now - last < i64::from(t.cooldown_secs)) {
        return suppress(SuppressReason::Cooldown);
    }
    if cx.fires_today >= t.daily_cap {
        return suppress(SuppressReason::DailyCap);
    }
    if cx.agent_fires_last_hour >= super::AGENT_WAKES_PER_HOUR {
        return suppress(SuppressReason::AgentCap);
    }
    FireDecision::Fire { incident_id, item_key }
}

/// Paused, simulated or muted right now (step 4 of [`evaluate`]).
pub fn window_suppression(source: &SourceRow, now: i64) -> Option<SuppressReason> {
    if source.status == SourceStatus::Paused {
        return Some(SuppressReason::Paused);
    }
    if source.simulate_state.is_some() && source.simulate_until.is_some_and(|u| u > now) {
        return Some(SuppressReason::Simulated);
    }
    if source.mute_until.is_some_and(|u| u > now) {
        return Some(SuppressReason::Muted);
    }
    None
}

/// Start of the UTC day containing `now`.
pub fn utc_day_start(now: i64) -> i64 {
    now - now.rem_euclid(86_400)
}

/// The open incidents a source has now, as hard transitions a trigger can
/// judge: one per crit/warn item with an incident, one for the source, and
/// a stale one. Used when a mute or simulate ends (contract §9): an
/// incident that is open now and never fired gets one fire then.
pub fn open_incidents(st: &SourceState) -> Vec<Transition> {
    use super::state::Health;
    let mut out = Vec::new();
    let went = |h: Health| match h {
        Health::Crit => Some(TransitionKind::WentCrit),
        Health::Warn => Some(TransitionKind::WentWarn),
        _ => None,
    };
    if let (Some(kind), Some(id)) = (went(st.health), &st.incident_id) {
        out.push(Transition {
            scope_item: None,
            from: Health::Ok,
            to: st.health,
            kind,
            incident_id: Some(id.clone()),
            reason: st.reason,
            hard: true,
        });
    }
    for it in st.items.values() {
        if let (Some(kind), Some(id)) = (went(it.health), &it.incident_id) {
            out.push(Transition {
                scope_item: Some(it.item_key.clone()),
                from: Health::Ok,
                to: it.health,
                kind,
                incident_id: Some(id.clone()),
                reason: it.reason,
                hard: true,
            });
        }
    }
    if st.phase == Phase::Stale {
        out.push(Transition {
            scope_item: None,
            from: st.health,
            to: st.health,
            kind: TransitionKind::WentStale,
            incident_id: None,
            reason: st.reason,
            hard: true,
        });
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::sources::spec::{AgentAccess, Keep, OnTargetError, SimulateState, SourceKind, Viewers};
    use crate::sources::state::{Health, ItemState};

    pub(crate) fn row() -> SourceRow {
        SourceRow {
            id: "s1".into(),
            name: "fleet".into(),
            label: "Fleet boxes".into(),
            kind: SourceKind::HttpJson,
            preset: None,
            spec_json: "{}".into(),
            spec_hash: "a".repeat(64),
            origins: vec!["https://acme.k2.dev:443".into()],
            lan: false,
            secret_name: None,
            url_secret_name: None,
            secret_bound: false,
            every_secs: 60,
            timeout_secs: 5,
            max_bytes: 1 << 20,
            fail_after: 3,
            on_target_error: OnTargetError::Crit,
            link: None,
            viewers: Viewers::Member,
            agent_access: AgentAccess::Values,
            keep: Keep::Values,
            pinned: false,
            pinned_by: None,
            pinned_at: None,
            status: SourceStatus::Active,
            pause_reason: None,
            made_by: "owner".into(),
            made_by_project: None,
            approved_by: None,
            approved_at: None,
            approved_spec_hash: None,
            simulate_state: None,
            simulate_until: None,
            simulate_by: None,
            mute_until: None,
            mute_by: None,
            mute_reason: None,
            rev: 1,
            created_at: 0,
            updated_at: 0,
            removed_at: None,
        }
    }

    fn trig(on: TriggerOn) -> TriggerSpec {
        TriggerSpec {
            id: "t1".into(),
            source_id: "s1".into(),
            on,
            target_project_id: "p1".into(),
            note: None,
            cooldown_secs: 1800,
            daily_cap: 12,
            status: TriggerStatus::Active,
            made_by: "owner".into(),
        }
    }

    fn tr(kind: TransitionKind, item: Option<&str>, hard: bool) -> Transition {
        Transition {
            scope_item: item.map(str::to_string),
            from: Health::Ok,
            to: Health::Crit,
            kind,
            incident_id: Some("inc-1".into()),
            reason: None,
            hard,
        }
    }

    fn cx<'a>(source: &'a SourceRow, state: &'a SourceState) -> FireContext<'a> {
        FireContext {
            source,
            state,
            now: 1_760_000_000,
            last_fire_at: None,
            fires_today: 0,
            agent_fires_last_hour: 0,
            already_fired_incident: false,
            in_gap: false,
            draining: false,
        }
    }

    fn reason(d: FireDecision) -> Option<SuppressReason> {
        match d {
            FireDecision::Suppress { reason, .. } => Some(reason),
            other => panic!("expected a suppression, got {other:?}"),
        }
    }

    #[test]
    fn fires_on_its_own_hard_kind_only() {
        let (r, st) = (row(), SourceState::new("s1"));
        let c = cx(&r, &st);
        let t = trig(TriggerOn::ItemWentCrit);
        assert_eq!(
            evaluate(&t, &tr(TransitionKind::WentCrit, Some("ohana"), true), &c),
            FireDecision::Fire { incident_id: "inc-1".into(), item_key: Some("ohana".into()) }
        );
        // Soft blips, source-level crit and other kinds are ignored.
        assert_eq!(evaluate(&t, &tr(TransitionKind::WentCrit, Some("ohana"), false), &c), FireDecision::Ignore);
        assert_eq!(evaluate(&t, &tr(TransitionKind::WentCrit, None, true), &c), FireDecision::Ignore);
        assert_eq!(evaluate(&t, &tr(TransitionKind::WentWarn, Some("ohana"), true), &c), FireDecision::Ignore);
        let src = trig(TriggerOn::WentCrit);
        assert!(matches!(evaluate(&src, &tr(TransitionKind::WentCrit, None, true), &c), FireDecision::Fire { .. }));
        assert_eq!(evaluate(&src, &tr(TransitionKind::WentCrit, Some("x"), true), &c), FireDecision::Ignore);
        let rec = trig(TriggerOn::Recovered);
        assert!(matches!(evaluate(&rec, &tr(TransitionKind::Recovered, None, true), &c), FireDecision::Fire { .. }));
        let warn = trig(TriggerOn::WentWarn);
        assert!(matches!(evaluate(&warn, &tr(TransitionKind::WentWarn, None, true), &c), FireDecision::Fire { .. }));
        assert_eq!(evaluate(&warn, &tr(TransitionKind::WentCrit, None, true), &c), FireDecision::Ignore);
    }

    #[test]
    fn proposed_and_paused_triggers_never_fire() {
        let (r, st) = (row(), SourceState::new("s1"));
        let c = cx(&r, &st);
        for status in [TriggerStatus::Proposed, TriggerStatus::Paused, TriggerStatus::Undeliverable] {
            let mut t = trig(TriggerOn::WentCrit);
            t.status = status;
            assert_eq!(evaluate(&t, &tr(TransitionKind::WentCrit, None, true), &c), FireDecision::Ignore, "{status:?}");
        }
    }

    #[test]
    fn suppressions_come_in_contract_order() {
        let t = trig(TriggerOn::WentCrit);
        let w = tr(TransitionKind::WentCrit, None, true);
        let st = SourceState::new("s1");
        let mut r = row();
        r.status = SourceStatus::Paused;
        r.simulate_state = Some(SimulateState::Crit);
        r.simulate_until = Some(1_760_000_100);
        r.mute_until = Some(1_760_000_100);
        r.mute_by = Some("owner".into());
        let mut c = cx(&r, &st);
        c.draining = true;
        c.in_gap = true;
        c.already_fired_incident = true;
        c.last_fire_at = Some(c.now - 10);
        c.fires_today = 12;
        c.agent_fires_last_hour = 30;
        assert_eq!(reason(evaluate(&t, &w, &c)), Some(SuppressReason::Draining));
        c.draining = false;
        assert_eq!(reason(evaluate(&t, &w, &c)), Some(SuppressReason::Gap));
        c.in_gap = false;
        assert_eq!(reason(evaluate(&t, &w, &c)), Some(SuppressReason::Paused));
        let mut r2 = r.clone();
        r2.status = SourceStatus::Active;
        let mut c = FireContext { source: &r2, ..c };
        assert_eq!(reason(evaluate(&t, &w, &c)), Some(SuppressReason::Simulated));
        let mut r3 = r2.clone();
        r3.simulate_state = None;
        r3.simulate_until = None;
        c.source = &r3;
        assert_eq!(reason(evaluate(&t, &w, &c)), Some(SuppressReason::Muted));
        let mut r4 = r3.clone();
        r4.mute_until = Some(c.now - 1); // a mute that ended doesn't count
        c.source = &r4;
        assert_eq!(reason(evaluate(&t, &w, &c)), Some(SuppressReason::Deduped));
        c.already_fired_incident = false;
        assert_eq!(reason(evaluate(&t, &w, &c)), Some(SuppressReason::Cooldown));
        c.last_fire_at = Some(c.now - 1800);
        assert_eq!(reason(evaluate(&t, &w, &c)), Some(SuppressReason::DailyCap));
        c.fires_today = 11;
        assert_eq!(reason(evaluate(&t, &w, &c)), Some(SuppressReason::AgentCap));
        c.agent_fires_last_hour = 29;
        assert!(matches!(evaluate(&t, &w, &c), FireDecision::Fire { .. }));
    }

    #[test]
    fn stale_for_waits_for_its_duration_and_keys_on_the_episode() {
        let r = row();
        let mut st = SourceState::new("s1");
        st.phase = Phase::Stale;
        st.updated_at = Some(1_760_000_000 - 1000);
        // every 60 → window max(180, 180) = 180; stale since now - 820.
        let t = trig(TriggerOn::StaleFor { secs: 900 });
        let mut w = tr(TransitionKind::WentStale, None, true);
        w.incident_id = None;
        let mut c = cx(&r, &st);
        assert_eq!(evaluate(&t, &w, &c), FireDecision::Ignore, "only 820 s stale");
        c.now += 80;
        assert_eq!(
            evaluate(&t, &w, &c),
            FireDecision::Fire { incident_id: format!("stale:s1:{}", 1_760_000_000 - 1000), item_key: None }
        );
        // Paused (a gap) is never stale.
        let mut paused = st.clone();
        paused.phase = Phase::Paused;
        let c2 = FireContext { state: &paused, ..c };
        assert_eq!(evaluate(&t, &w, &c2), FireDecision::Ignore);
    }

    #[test]
    fn open_incidents_lists_items_source_and_stale() {
        let mut st = SourceState::new("s1");
        st.health = Health::Crit;
        st.incident_id = Some("src-inc".into());
        st.items.insert(
            "ohana".into(),
            ItemState {
                item_key: "ohana".into(),
                is_target: true,
                origin: None,
                health: Health::Crit,
                soft_health: None,
                soft_count: 0,
                reason: None,
                http_status: None,
                hard_since: Some(1),
                recovered_at: None,
                updated_at: None,
                checked_at: None,
                etag: None,
                last_modified: None,
                incident_id: Some("item-inc".into()),
            },
        );
        let open = open_incidents(&st);
        assert_eq!(open.len(), 2);
        assert!(open.iter().all(|t| t.hard));
        assert!(open.iter().any(|t| t.scope_item.as_deref() == Some("ohana") && t.incident_id.as_deref() == Some("item-inc")));
        st.phase = Phase::Stale;
        assert_eq!(open_incidents(&st).len(), 3);
    }

    #[test]
    fn on_parts_round_trip_and_day_start() {
        for on in [
            TriggerOn::WentCrit,
            TriggerOn::WentWarn,
            TriggerOn::Recovered,
            TriggerOn::ItemWentCrit,
            TriggerOn::StaleFor { secs: 1800 },
        ] {
            let back = TriggerOn::from_parts(on.code(), on.stale_for_secs().map(i64::from));
            assert_eq!(back, Some(on));
        }
        assert_eq!(TriggerOn::from_parts("stale_for", None), None);
        assert_eq!(utc_day_start(86_400 * 3 + 5), 86_400 * 3);
        assert_eq!(serde_json::to_value(TriggerOn::StaleFor { secs: 60 }).unwrap(), serde_json::json!({"kind": "stale_for", "secs": 60}));
    }
}
