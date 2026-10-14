//! Trigger model and evaluation (contract §3.5, §9). Owner: B2. C0 ships
//! the types and the signature; `evaluate` ignores everything until B2
//! lands the rules.

use serde::{Deserialize, Serialize};

use super::state::{SourceState, Transition};
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

/// First match wins (contract §9 "evaluate order").
///
/// C0 skeleton: `Ignore` for everything (B2 replaces it).
pub fn evaluate(t: &TriggerSpec, tr: &Transition, cx: &FireContext) -> FireDecision {
    let _ = (t, tr, cx);
    FireDecision::Ignore
}
