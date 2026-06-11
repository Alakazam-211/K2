//! Awareness Bus primitive — cross-agent signals.
//!
//! Primitive B from `.k2so/prds/session-stream-and-awareness-bus.md`.
//! Phase 1 C3 shipped the types; Phase 3 E1-E8 layers the runtime
//! (bus singleton, filesystem inbox, routing, egress composer,
//! daemon endpoints, CLI verbs).
//!
//! The wire model: agents emit `AgentSignal`s through one of three
//! ingress paths (APC escape in a session, `k2so msg` CLI, extension-
//! pack in-process emit). Each signal carries a `Delivery` field the
//! sender picks — `Live` (real-time peer-to-peer, 1-on-1 semantics)
//! or `Inbox` (intentional async, email semantics). `SignalKind` is
//! deliberately small — five variants plus a `Custom` escape hatch —
//! to keep the agent-facing vocabulary tight.

pub mod budget;
pub mod bus;
pub mod egress;
pub mod inbox;
pub mod ingress;
pub mod roster;
pub mod routing;

pub use budget::{check_and_increment as budget_check, BudgetCheck, BudgetDecision, CLI_SENDER_NAME};
pub use bus::{publish, subscribe, subscriber_count, BUS_CAP};
pub use egress::{
    deliver, deliver_to_agent, execute, set_inject_provider,
    set_wake_provider, DeliveryReport, InjectProvider, WakeProvider,
};
pub use ingress::{from_cli, from_session, inbox_root, set_inbox_root};
pub use roster::{AgentInfo, RosterFilter, RosterState};
pub use routing::{resolve, resolve_for_agent, DeliveryPlan, TargetState};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::session::SessionId;

/// Opaque signal identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SignalId(pub Uuid);

impl SignalId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for SignalId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for SignalId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Workspace identifier. Phase 1 wraps `String`; Phase 3 may swap for
/// a typed reference into the `projects` table once routing needs it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorkspaceId(pub String);

/// Where a signal is addressed. Three shapes: a specific agent in a
/// specific workspace, an entire workspace's inbox (no specific
/// agent), or a broadcast visible to all locally-known agents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum AgentAddress {
    Agent {
        workspace: WorkspaceId,
        name: String,
    },
    Workspace {
        workspace: WorkspaceId,
    },
    Broadcast,
}

/// Routing priority. Budget-aware — Phase 3's per-coordination-level
/// message budgets (Pi-Messenger: none/minimal/moderate/chatty =
/// 0/2/5/10 emissions per agent per session) will shed low-priority
/// signals first when an agent overruns its budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    Low,
    #[default]
    Normal,
    High,
    Urgent,
}

/// The locked SignalKind vocabulary — five variants + `Custom`. Matches
/// the Pi-Messenger coordination vocabulary; `#[non_exhaustive]` so
/// future additions are non-breaking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum SignalKind {
    /// Chat-style message between agents.
    Msg { text: String },
    /// Status-line update from the sending agent.
    Status { text: String },
    /// File-reservation claim or release.
    Reservation {
        paths: Vec<String>,
        action: ReservationAction,
    },
    /// Presence state change.
    Presence { state: PresenceState },
    /// Task lifecycle transition. `task_ref` optionally names the
    /// markdown file / DB row being transitioned.
    TaskLifecycle {
        phase: TaskPhase,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        task_ref: Option<String>,
    },
    /// Escape hatch for harness-specific signals. `kind` names the
    /// semantic; `payload` carries arbitrary JSON.
    Custom {
        kind: String,
        payload: serde_json::Value,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum ReservationAction {
    Claim,
    Release,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum PresenceState {
    Active,
    Idle,
    Away,
    Stuck,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum TaskPhase {
    Started,
    Done,
    Blocked,
}

/// Per-signal delivery mode. The sender picks based on intent —
/// see the Phase 3 plan's egress matrix for full semantics.
///
/// - `Live` is the "1-on-1 conversation" path: target sees it
///   immediately via PTY-inject (if running) or wake+inject (if
///   offline). Use when the message is time-sensitive and the
///   sender wants the target to react now.
/// - `Inbox` is the "email / notice" path: atomic-rename file
///   lands in the target's inbox; no PTY-inject, no wake. Target
///   reads on their own schedule. Use for FYIs, status updates,
///   things that can wait.
///
/// Default is `Live`. Callers who want the async/notice path must
/// opt in explicitly — a deliberately stronger default than
/// "whichever is easier" so peer-to-peer stays the norm.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize,
)]
#[non_exhaustive]
#[serde(rename_all = "lowercase")]
pub enum Delivery {
    #[default]
    Live,
    Inbox,
}

/// A cross-agent signal. Phase 1 POD — Phase 3 E3 adds the
/// `delivery` field so senders can pick Live (1-on-1) vs Inbox
/// (email) semantics. `#[serde(default)]` on delivery makes the
/// addition backwards-compatible: old JSON without the field
/// decodes with `delivery: Live` (the default).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentSignal {
    pub id: SignalId,
    /// Session the sender was inside when emitting. Optional because
    /// `k2so msg` CLI invocations aren't attached to a live session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionId>,
    pub from: AgentAddress,
    pub to: AgentAddress,
    pub kind: SignalKind,
    #[serde(default)]
    pub priority: Priority,
    /// Delivery mode. `Live` (default) = real-time peer-to-peer.
    /// `Inbox` = intentional async / notice. See `Delivery` docs.
    #[serde(default)]
    pub delivery: Delivery,
    /// For reply chains. `None` for first emission of a conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<SignalId>,
    pub at: DateTime<Utc>,
}

impl AgentSignal {
    /// Fresh signal with default Live delivery, random id, `at = now`.
    pub fn new(from: AgentAddress, to: AgentAddress, kind: SignalKind) -> Self {
        Self {
            id: SignalId::new(),
            session: None,
            from,
            to,
            kind,
            priority: Priority::default(),
            delivery: Delivery::default(),
            reply_to: None,
            at: Utc::now(),
        }
    }

    /// Builder-style opt-in for Inbox delivery. Most callers should
    /// reach for this only when they've specifically decided that
    /// the message can wait — use of the default `Live` stays the
    /// norm for 1-on-1 agent conversations.
    pub fn with_delivery(mut self, delivery: Delivery) -> Self {
        self.delivery = delivery;
        self
    }
}
