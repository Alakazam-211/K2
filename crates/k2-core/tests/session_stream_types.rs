//! C2 type-definition tests: Frame / Line / SemanticKind / Session
//! round-trips through serde_json, plus sanity checks on `SeqnoGen`
//! monotonicity and `SessionId` uniqueness.

use k2_core::awareness::{AgentAddress, AgentSignal, SignalKind, WorkspaceId};
use k2_core::session::{
    CursorOp, EraseMode, Frame, HarnessKind, Line, SemanticKind, SeqnoGen,
    Session, SessionId, Style,
};
use serde_json::json;
use std::path::PathBuf;

#[test]
fn semantic_kind_all_variants_round_trip() {
    let variants = [
        SemanticKind::Message,
        SemanticKind::ToolCall,
        SemanticKind::ToolResult,
        SemanticKind::Plan,
        SemanticKind::Compaction,
        SemanticKind::Custom {
            kind: "usage".into(),
            payload: json!({ "tokens": 42 }),
        },
    ];
    for v in variants {
        let encoded = serde_json::to_string(&v).unwrap();
        let decoded: SemanticKind = serde_json::from_str(&encoded).unwrap();
        assert_eq!(v, decoded, "round-trip failed for {encoded}");
    }
}

#[test]
fn frame_variants_round_trip() {
    let cases = [
        Frame::Text {
            bytes: b"hello".to_vec(),
            style: None,
        },
        Frame::Text {
            bytes: b"hi".to_vec(),
            style: Some(Style {
                bold: true,
                ..Default::default()
            }),
        },
        Frame::CursorOp(CursorOp::Goto { row: 5, col: 10 }),
        Frame::CursorOp(CursorOp::ClearScreen),
        Frame::CursorOp(CursorOp::EraseInLine(EraseMode::All)),
        Frame::CursorOp(CursorOp::Up(3)),
        Frame::SemanticEvent {
            kind: SemanticKind::ToolCall,
            payload: json!({ "name": "bash", "input": { "cmd": "ls" } }),
        },
        Frame::RawPtyFrame(vec![0x1b, b'[', b'2', b'J']),
        Frame::AgentSignal(AgentSignal::new(
            AgentAddress::Agent {
                workspace: WorkspaceId("k2so".into()),
                name: "rust-eng".into(),
            },
            AgentAddress::Broadcast,
            SignalKind::Msg {
                text: "session stream frames flowing".into(),
            },
        )),
    ];
    for frame in cases {
        let encoded = serde_json::to_string(&frame).unwrap();
        let decoded: Frame = serde_json::from_str(&encoded)
            .unwrap_or_else(|e| panic!("round-trip failed for {encoded}: {e}"));
        assert_eq!(frame, decoded);
    }
}

#[test]
fn line_append_and_serde() {
    let gen = SeqnoGen::default();
    assert_eq!(gen.current(), 0);
    let first = gen.next();
    let second = gen.next();
    assert_eq!(first, 1);
    assert_eq!(second, 2);
    assert_eq!(gen.current(), 2);

    let mut line = Line::new(first);
    line.push_str("hello");
    line.push_str(", ");
    line.push_str("world");
    assert_eq!(line.text, "hello, world");
    assert_eq!(line.seqno, 1);

    let encoded = serde_json::to_string(&line).unwrap();
    let decoded: Line = serde_json::from_str(&encoded).unwrap();
    assert_eq!(line, decoded);
}

#[test]
fn seqno_gen_is_cheap_to_clone_and_shares_counter() {
    let a = SeqnoGen::default();
    let b = a.clone();
    let _ = a.next();
    let _ = b.next();
    // Both sides bumped the same shared AtomicU64.
    assert_eq!(a.current(), 2);
    assert_eq!(b.current(), 2);
}

#[test]
fn session_id_is_unique_per_call() {
    let a = SessionId::new();
    let b = SessionId::new();
    assert_ne!(a, b);
}

#[test]
fn session_round_trips() {
    let session = Session::new(HarnessKind::ClaudeCode, PathBuf::from("/tmp/x"));
    let encoded = serde_json::to_string(&session).unwrap();
    let decoded: Session = serde_json::from_str(&encoded).unwrap();
    assert_eq!(session.id, decoded.id);
    assert_eq!(session.harness, decoded.harness);
    assert_eq!(session.cwd, decoded.cwd);
}

#[test]
fn harness_kind_round_trips_every_variant() {
    let variants = [
        HarnessKind::ClaudeCode,
        HarnessKind::Codex,
        HarnessKind::Gemini,
        HarnessKind::Aider,
        HarnessKind::Pi,
        HarnessKind::Goose,
        HarnessKind::Other,
    ];
    for variant in variants {
        let encoded = serde_json::to_string(&variant).unwrap();
        let decoded: HarnessKind = serde_json::from_str(&encoded).unwrap();
        assert_eq!(variant, decoded);
    }
}

#[test]
fn semantic_kind_custom_carries_arbitrary_payload() {
    let variant = SemanticKind::Custom {
        kind: "message_stop".into(),
        payload: json!({
            "stop_reason": "end_turn",
            "usage": { "input_tokens": 1024, "output_tokens": 42 }
        }),
    };
    let encoded = serde_json::to_string(&variant).unwrap();
    let decoded: SemanticKind = serde_json::from_str(&encoded).unwrap();
    assert_eq!(variant, decoded);
}
