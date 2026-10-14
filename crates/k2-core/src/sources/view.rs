//! The card view (contract §3.3): the one shape for `sources.read`,
//! `k2 sources read --json`, `show`'s `view` and `changes`.

use std::collections::BTreeMap;

use serde::Serialize;

use super::extract::Projection;
use super::spec::{AgentAccess, Json, Keep, SimulateState, SourceKind, SourceStatus};
use super::state::{GapKind, Health, Phase, Reason, SourceState};
pub use super::state::Counts;
use super::store::SourceRow;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Soft {
    pub count: u32,
    pub of: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Gap {
    pub kind: GapKind,
    pub from: i64,
    pub to: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Muted {
    pub until: i64,
    pub by: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Simulated {
    pub state: SimulateState,
    pub until: i64,
    pub by: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceResult {
    pub name: String,
    pub label: String,
    pub kind: SourceKind,
    pub status: SourceStatus,
    pub health: Health,
    pub phase: Phase,
    /// `pending` | `paused` | `stale` | the health code.
    pub display: &'static str,
    pub reason: Option<Reason>,
    pub reason_text: Option<&'static str>,
    pub http_status: Option<u16>,
    pub soft: Soft,
    pub server_now: i64,
    pub updated_at: Option<i64>,
    pub checked_at: Option<i64>,
    pub next_at: Option<i64>,
    pub hard_since: Option<i64>,
    pub recovered_at: Option<i64>,
    pub gap: Option<Gap>,
    pub muted: Option<Muted>,
    pub simulated: Option<Simulated>,
    pub pinned: bool,
    pub keep: Keep,
    /// Values replaced for this caller.
    pub masked: bool,
    pub link: Option<String>,
    /// `{}` for `keep: states`.
    pub fields: BTreeMap<String, Json>,
    /// <= 200.
    pub items: Vec<ItemView>,
    pub counts: Counts,
    pub rev: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemView {
    pub key: String,
    pub health: Health,
    pub reason: Option<Reason>,
    pub since: Option<i64>,
    pub soft: Soft,
    pub fields: BTreeMap<String, Json>,
}

/// Who reads. `Agent` covers agent and shell passports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Viewer {
    Person,
    Agent,
}

/// A masked value: `"•••"` for strings, bools and null; `0` for numbers.
fn mask(v: &Json) -> Json {
    match v {
        Json::Number(_) => Json::from(0),
        Json::Array(a) => Json::Array(a.iter().map(mask).collect()),
        Json::Object(m) => Json::Object(m.iter().map(|(k, v)| (k.clone(), mask(v))).collect()),
        _ => Json::String("•••".into()),
    }
}

fn shown(fields: &BTreeMap<String, Json>, keep: Keep, masked: bool) -> BTreeMap<String, Json> {
    match keep {
        Keep::States => BTreeMap::new(),
        Keep::Values if masked => fields.iter().map(|(k, v)| (k.clone(), mask(v))).collect(),
        Keep::Values => fields.clone(),
    }
}

/// Build the card view. Masking: `Viewer::Agent` on an `agentAccess:
/// shape` source masks every value; `keep: states` shows no values to
/// anyone. Simulation overlays `health`/`phase` and leaves the real values
/// underneath. Mute and simulate windows that ended are not shown.
pub fn build(row: &SourceRow, st: &SourceState, proj: Option<&Projection>, viewer: Viewer, server_now: i64) -> SourceResult {
    let masked = viewer == Viewer::Agent && row.agent_access == AgentAccess::Shape && row.keep == Keep::Values;
    let simulated = match (row.simulate_state, row.simulate_until) {
        (Some(state), Some(until)) if until > server_now => {
            Some(Simulated { state, until, by: row.simulate_by.clone() })
        }
        _ => None,
    };
    let muted = match (row.mute_until, &row.mute_by) {
        (Some(until), Some(by)) if until > server_now => {
            Some(Muted { until, by: by.clone(), reason: row.mute_reason.clone() })
        }
        _ => None,
    };
    let (mut health, mut phase) = (st.health, st.phase);
    if let Some(sim) = &simulated {
        match sim.state {
            SimulateState::Ok => health = Health::Ok,
            SimulateState::Warn => health = Health::Warn,
            SimulateState::Crit => health = Health::Crit,
            SimulateState::Unknown => health = Health::Unknown,
            SimulateState::Stale => phase = Phase::Stale,
            SimulateState::Slow => {}
        }
        if phase == Phase::Pending && sim.state != SimulateState::Stale {
            phase = Phase::Live;
        }
    }
    if row.status == SourceStatus::Paused {
        phase = Phase::Paused;
    }
    let display = match phase {
        Phase::Pending => "pending",
        Phase::Paused => "paused",
        Phase::Stale => "stale",
        Phase::Live => health.code(),
    };
    let soft_of = u32::from(row.fail_after);
    let empty = BTreeMap::new();
    let fields = shown(proj.map(|p| &p.fields).unwrap_or(&empty), row.keep, masked);
    let mut items: Vec<ItemView> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    if let Some(p) = proj {
        for pi in &p.items {
            let it = st.items.get(&pi.key);
            seen.insert(pi.key.clone());
            items.push(ItemView {
                key: pi.key.clone(),
                health: it.map_or(Health::Unknown, |i| i.health),
                reason: it.and_then(|i| i.reason),
                since: it.and_then(|i| i.hard_since),
                soft: Soft { count: it.map_or(0, |i| i.soft_count), of: soft_of },
                fields: shown(&pi.fields, row.keep, masked),
            });
        }
    }
    // Fan-out targets that answered nothing this cycle still show.
    for (key, it) in &st.items {
        if items.len() >= super::MAX_ITEMS {
            break;
        }
        if seen.contains(key) {
            continue;
        }
        items.push(ItemView {
            key: key.clone(),
            health: it.health,
            reason: it.reason,
            since: it.hard_since,
            soft: Soft { count: it.soft_count, of: soft_of },
            fields: BTreeMap::new(),
        });
    }
    items.truncate(super::MAX_ITEMS);
    let gap = match (st.gap_kind, st.gap_from, st.gap_to) {
        (Some(kind), Some(from), Some(to)) => Some(Gap { kind, from, to }),
        _ => None,
    };
    SourceResult {
        name: row.name.clone(),
        label: row.label.clone(),
        kind: row.kind,
        status: row.status,
        health,
        phase,
        display,
        reason: st.reason,
        reason_text: st.reason.map(Reason::sentence),
        http_status: st.http_status,
        soft: Soft { count: st.soft_count, of: soft_of },
        server_now,
        updated_at: st.updated_at,
        checked_at: st.checked_at,
        next_at: st.next_at,
        hard_since: st.hard_since,
        recovered_at: st.recovered_at,
        gap,
        muted,
        simulated,
        pinned: row.pinned,
        keep: row.keep,
        masked,
        link: row.link.clone(),
        fields,
        items,
        counts: st.counts,
        rev: row.rev.max(st.rev),
    }
}
