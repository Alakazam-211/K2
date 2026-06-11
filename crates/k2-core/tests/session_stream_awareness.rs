//! C3 awareness-type tests: `AgentSignal`, `SignalKind`, `AgentAddress`,
//! `Priority`, `PresenceState`, `TaskPhase`, `ReservationAction`.
//!
//! Types-only. Routing and egress are Phase 3.

use chrono::Utc;
use k2_core::awareness::{
    AgentAddress, AgentSignal, PresenceState, Priority, ReservationAction,
    SignalId, SignalKind, TaskPhase, WorkspaceId,
};
use k2_core::session::SessionId;
use serde_json::json;

fn some_workspace() -> WorkspaceId {
    WorkspaceId("alakazam-labs/k2so".into())
}

fn some_agent_address() -> AgentAddress {
    AgentAddress::Agent {
        workspace: some_workspace(),
        name: "rust-eng".into(),
    }
}

#[test]
fn signal_kind_all_variants_round_trip() {
    let variants = [
        SignalKind::Msg {
            text: "hello".into(),
        },
        SignalKind::Status {
            text: "scanning repo".into(),
        },
        SignalKind::Reservation {
            paths: vec!["src/lib.rs".into(), "Cargo.toml".into()],
            action: ReservationAction::Claim,
        },
        SignalKind::Reservation {
            paths: vec!["src/main.rs".into()],
            action: ReservationAction::Release,
        },
        SignalKind::Presence {
            state: PresenceState::Active,
        },
        SignalKind::Presence {
            state: PresenceState::Stuck,
        },
        SignalKind::TaskLifecycle {
            phase: TaskPhase::Started,
            task_ref: Some(".k2so/inbox/foo.md".into()),
        },
        SignalKind::TaskLifecycle {
            phase: TaskPhase::Done,
            task_ref: None,
        },
        SignalKind::Custom {
            kind: "my-harness:tool-aborted".into(),
            payload: json!({ "reason": "user-cancelled", "tool_id": "t_123" }),
        },
    ];
    for variant in variants {
        let encoded = serde_json::to_string(&variant).unwrap();
        let decoded: SignalKind = serde_json::from_str(&encoded).unwrap();
        assert_eq!(variant, decoded, "round-trip failed for {encoded}");
    }
}

#[test]
fn agent_address_all_shapes_round_trip() {
    let shapes = [
        AgentAddress::Agent {
            workspace: some_workspace(),
            name: "pod-leader".into(),
        },
        AgentAddress::Workspace {
            workspace: some_workspace(),
        },
        AgentAddress::Broadcast,
    ];
    for shape in shapes {
        let encoded = serde_json::to_string(&shape).unwrap();
        let decoded: AgentAddress = serde_json::from_str(&encoded).unwrap();
        assert_eq!(shape, decoded);
    }
}

#[test]
fn priority_defaults_to_normal_and_round_trips() {
    assert_eq!(Priority::default(), Priority::Normal);
    for p in [Priority::Low, Priority::Normal, Priority::High, Priority::Urgent] {
        let encoded = serde_json::to_string(&p).unwrap();
        let decoded: Priority = serde_json::from_str(&encoded).unwrap();
        assert_eq!(p, decoded);
    }
}

#[test]
fn agent_signal_constructor_defaults() {
    let signal = AgentSignal::new(
        some_agent_address(),
        AgentAddress::Broadcast,
        SignalKind::Status {
            text: "alive".into(),
        },
    );
    assert_eq!(signal.priority, Priority::Normal);
    assert!(signal.session.is_none());
    assert!(signal.reply_to.is_none());
    // `at` should be recent — within 5 seconds of now.
    let delta = Utc::now().signed_duration_since(signal.at);
    assert!(delta.num_seconds().abs() < 5);
}

#[test]
fn agent_signal_round_trips_with_all_optional_fields() {
    let signal = AgentSignal {
        id: SignalId::new(),
        session: Some(SessionId::new()),
        from: some_agent_address(),
        to: AgentAddress::Broadcast,
        kind: SignalKind::Msg {
            text: "coordination check".into(),
        },
        priority: Priority::High,
        delivery: k2_core::awareness::Delivery::default(),
        reply_to: Some(SignalId::new()),
        at: Utc::now(),
    };
    let encoded = serde_json::to_string(&signal).unwrap();
    let decoded: AgentSignal = serde_json::from_str(&encoded).unwrap();
    assert_eq!(signal, decoded);
}

#[test]
fn agent_signal_round_trips_with_none_optional_fields() {
    let signal = AgentSignal::new(
        some_agent_address(),
        AgentAddress::Workspace {
            workspace: some_workspace(),
        },
        SignalKind::Presence {
            state: PresenceState::Idle,
        },
    );
    let encoded = serde_json::to_string(&signal).unwrap();
    // `session` and `reply_to` are None and should be skipped in the wire
    // format — keeps payloads compact. `priority` is `Normal` (default)
    // but is still serialized because serde's skip-default only applies
    // when explicitly opted into with `skip_serializing_if`.
    assert!(!encoded.contains("\"session\""));
    assert!(!encoded.contains("\"reply_to\""));
    let decoded: AgentSignal = serde_json::from_str(&encoded).unwrap();
    assert_eq!(signal, decoded);
}
