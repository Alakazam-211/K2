//! E3 tests for `Delivery` enum + `awareness::routing` resolver +
//! `awareness::roster` query.
//!
//! Routing is a pure function — no I/O, no singletons, no locks.
//! Tests are straightforward value assertions. Roster tests use
//! tempdirs for the filesystem side; liveness tests use the
//! session::registry singleton so there's a `TEST_LOCK` for those.

use std::path::PathBuf;
use std::sync::Mutex;

use k2_core::awareness::{
    self, AgentAddress, AgentInfo, AgentSignal, Delivery, DeliveryPlan,
    Priority, RosterFilter, RosterState, SignalKind, TargetState, WorkspaceId,
};
use k2_core::session::{registry, SessionId};

static ROSTER_TEST_LOCK: Mutex<()> = Mutex::new(());

fn workspace() -> WorkspaceId {
    WorkspaceId("k2so".into())
}

fn signal_to_bar(delivery: Delivery) -> AgentSignal {
    AgentSignal::new(
        AgentAddress::Agent {
            workspace: workspace(),
            name: "foo".into(),
        },
        AgentAddress::Agent {
            workspace: workspace(),
            name: "bar".into(),
        },
        SignalKind::Msg {
            text: "hello".into(),
        },
    )
    .with_delivery(delivery)
}

fn signal_broadcast() -> AgentSignal {
    AgentSignal::new(
        AgentAddress::Agent {
            workspace: workspace(),
            name: "foo".into(),
        },
        AgentAddress::Broadcast,
        SignalKind::Status {
            text: "heartbeat".into(),
        },
    )
}

// ─────────────────────────────────────────────────────────────────────
// Delivery enum
// ─────────────────────────────────────────────────────────────────────

#[test]
fn delivery_defaults_to_live() {
    assert_eq!(Delivery::default(), Delivery::Live);
}

#[test]
fn delivery_json_round_trip_live() {
    let json = serde_json::to_string(&Delivery::Live).unwrap();
    assert_eq!(json, "\"live\"");
    let decoded: Delivery = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, Delivery::Live);
}

#[test]
fn delivery_json_round_trip_inbox() {
    let json = serde_json::to_string(&Delivery::Inbox).unwrap();
    assert_eq!(json, "\"inbox\"");
    let decoded: Delivery = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, Delivery::Inbox);
}

#[test]
fn agent_signal_new_defaults_to_live_delivery() {
    let s = signal_to_bar(Delivery::default());
    assert_eq!(s.delivery, Delivery::Live);
}

#[test]
fn agent_signal_with_delivery_builder_overrides() {
    let s = signal_to_bar(Delivery::Inbox);
    assert_eq!(s.delivery, Delivery::Inbox);
}

#[test]
fn agent_signal_backwards_compat_decodes_without_delivery_field() {
    // Simulate a pre-Phase-3 signal on the wire (no `delivery` key
    // in the JSON) — serde(default) should populate Live.
    let json = r#"{
        "id": "00000000-0000-0000-0000-000000000001",
        "from": { "scope": "broadcast" },
        "to": { "scope": "broadcast" },
        "kind": { "kind": "status", "data": { "text": "legacy" } },
        "priority": "normal",
        "at": "2020-01-01T00:00:00Z"
    }"#;
    let decoded: AgentSignal = serde_json::from_str(json).unwrap();
    assert_eq!(decoded.delivery, Delivery::Live);
}

#[test]
fn agent_signal_round_trip_preserves_delivery_field() {
    let original = signal_to_bar(Delivery::Inbox);
    let encoded = serde_json::to_string(&original).unwrap();
    let decoded: AgentSignal = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded.delivery, Delivery::Inbox);
    assert_eq!(decoded, original);
}

// ─────────────────────────────────────────────────────────────────────
// Routing resolver — the four-cell matrix
// ─────────────────────────────────────────────────────────────────────

#[test]
fn live_to_live_target_injects_to_pty() {
    let plan =
        awareness::resolve(&signal_to_bar(Delivery::Live), TargetState::Live);
    assert_eq!(plan.target_agent.as_deref(), Some("bar"));
    assert!(plan.inject_to_pty, "Live+Live must inject");
    assert!(!plan.wake_target, "Live+Live must not wake");
    assert!(!plan.write_to_inbox, "Live+Live must not write inbox");
    assert!(plan.publish_to_bus);
    assert!(plan.write_activity_feed);
}

#[test]
fn live_to_offline_target_wakes_and_queues() {
    let plan = awareness::resolve(
        &signal_to_bar(Delivery::Live),
        TargetState::Offline,
    );
    assert_eq!(plan.target_agent.as_deref(), Some("bar"));
    assert!(!plan.inject_to_pty, "Live+Offline must not inject immediately");
    assert!(plan.wake_target, "Live+Offline must wake");
    assert!(
        !plan.write_to_inbox,
        "Live+Offline uses pending-queue, NOT inbox"
    );
    assert!(plan.publish_to_bus);
    assert!(plan.write_activity_feed);
}

#[test]
fn inbox_to_live_target_writes_inbox_no_inject() {
    let plan = awareness::resolve(
        &signal_to_bar(Delivery::Inbox),
        TargetState::Live,
    );
    assert_eq!(plan.target_agent.as_deref(), Some("bar"));
    assert!(!plan.inject_to_pty, "Inbox never injects");
    assert!(!plan.wake_target, "Inbox never wakes");
    assert!(plan.write_to_inbox, "Inbox always writes inbox");
    assert!(plan.publish_to_bus);
    assert!(plan.write_activity_feed);
}

#[test]
fn inbox_to_offline_target_same_as_inbox_to_live() {
    let live = awareness::resolve(
        &signal_to_bar(Delivery::Inbox),
        TargetState::Live,
    );
    let offline = awareness::resolve(
        &signal_to_bar(Delivery::Inbox),
        TargetState::Offline,
    );
    assert_eq!(
        live, offline,
        "Inbox is state-insensitive — sender intent is the only input"
    );
}

#[test]
fn audit_channels_fire_in_every_matrix_cell() {
    for delivery in [Delivery::Live, Delivery::Inbox] {
        for state in [TargetState::Live, TargetState::Offline] {
            let plan =
                awareness::resolve(&signal_to_bar(delivery), state);
            assert!(
                plan.publish_to_bus,
                "bus broadcast must fire for {delivery:?} × {state:?}"
            );
            assert!(
                plan.write_activity_feed,
                "activity_feed must fire for {delivery:?} × {state:?}"
            );
        }
    }
}

#[test]
fn broadcast_address_returns_target_agent_none() {
    let plan = awareness::resolve(&signal_broadcast(), TargetState::Live);
    assert!(
        plan.target_agent.is_none(),
        "Broadcast fans out in caller, not resolver"
    );
    assert!(plan.publish_to_bus);
    assert!(plan.write_activity_feed);
    assert!(!plan.inject_to_pty);
    assert!(!plan.write_to_inbox);
}

#[test]
fn resolve_for_agent_ignores_signal_to_field() {
    // The per-agent resolver takes the target name explicitly, so
    // `signal.to` doesn't matter. Useful for fanout from
    // Broadcast/Workspace addresses.
    let plan = awareness::resolve_for_agent(
        &signal_broadcast(),
        Some("specific-target".into()),
        TargetState::Live,
    );
    assert_eq!(plan.target_agent.as_deref(), Some("specific-target"));
    // signal_broadcast() has default Live delivery, so Live+Live path.
    assert!(plan.inject_to_pty);
}

// ─────────────────────────────────────────────────────────────────────
// Roster — connections-backed (0.39.0 rewrite)
// ─────────────────────────────────────────────────────────────────────
//
// Pre-0.39 the roster was filesystem-backed (it walked
// `.k2so/agents/<name>/`). 0.39.0's workspace==agent model moved the
// data source to `crate::connections::list_peers` — a workspace's
// "team" is the set of other connected workspaces, not subdirectories
// in its own tree. The integration-level filesystem tests that lived
// here were superseded by the in-module tests in
// `crates/k2so-core/src/awareness/roster.rs`, which can seed the
// shared in-memory DB directly without needing the daemon to be up.
//
// `agent_info_json_serializes_cleanly` survives because it's a pure
// type-level round-trip — but the `skill_summary` field is now
// `Option<String>` (always `None` for peers until cross-workspace
// skill reads land; see roster.rs module head for the follow-up).

#[test]
fn agent_info_json_serializes_cleanly() {
    let info = AgentInfo {
        name: "alice".into(),
        workspace: Some(WorkspaceId("k2so".into())),
        state: RosterState::Live,
        skill_summary: Some("does things".into()),
    };
    let json = serde_json::to_string(&info).unwrap();
    assert!(json.contains(r#""state":"live""#), "{json}");
    let decoded: AgentInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(info, decoded);
}

#[test]
fn signal_with_priority_and_delivery_together() {
    // Regression: priority and delivery are both optional additive
    // fields; setting both shouldn't interfere.
    let sig = AgentSignal::new(
        AgentAddress::Broadcast,
        AgentAddress::Broadcast,
        SignalKind::Msg {
            text: "test".into(),
        },
    );
    let sig = AgentSignal {
        priority: Priority::Urgent,
        delivery: Delivery::Inbox,
        ..sig
    };
    let encoded = serde_json::to_string(&sig).unwrap();
    let decoded: AgentSignal = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded.priority, Priority::Urgent);
    assert_eq!(decoded.delivery, Delivery::Inbox);
}

#[test]
fn resolve_matrix_as_table() {
    // Compact assertion of every cell in one place — doubles as
    // living documentation of the routing rules.
    let cases: Vec<(Delivery, TargetState, fn(&DeliveryPlan) -> bool)> = vec![
        (Delivery::Live, TargetState::Live, |p| {
            p.inject_to_pty && !p.wake_target && !p.write_to_inbox
        }),
        (Delivery::Live, TargetState::Offline, |p| {
            !p.inject_to_pty && p.wake_target && !p.write_to_inbox
        }),
        (Delivery::Inbox, TargetState::Live, |p| {
            !p.inject_to_pty && !p.wake_target && p.write_to_inbox
        }),
        (Delivery::Inbox, TargetState::Offline, |p| {
            !p.inject_to_pty && !p.wake_target && p.write_to_inbox
        }),
    ];
    for (delivery, state, check) in cases {
        let plan = awareness::resolve(&signal_to_bar(delivery), state);
        assert!(
            check(&plan),
            "matrix cell {:?} × {:?} wrong: {:?}",
            delivery,
            state,
            plan
        );
    }
}
