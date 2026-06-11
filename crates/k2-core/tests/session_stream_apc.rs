//! C5 APC-extractor tests.
//!
//! Covers:
//!   - All 8 verbs from PRD §"APC side-channel namespace" produce the
//!     correct ApcEvent variant
//!   - Bad JSON / unknown verb / non-`k2so:` / non-UTF-8 land in the
//!     `drops` bucket with the right reason, never in `chunks`
//!   - State persists across feed() calls — an APC split mid-sequence
//!     across two reads resolves to one event
//!   - `LineMux` wires the extractor in at the correct stream
//!     position, preserving Text/APC/Text ordering

use k2_core::awareness::{
    AgentAddress, PresenceState, ReservationAction, SignalKind, TaskPhase,
};
use k2_core::session::{Frame, SemanticKind};
use k2_core::term::apc::{ApcChunk, ApcDrop, ApcEvent, ApcExtractor};
use k2_core::term::LineMux;
use serde_json::json;

// ─────────────────────────────────────────────────────────────────────
// ApcExtractor unit tests
// ─────────────────────────────────────────────────────────────────────

fn apc(content: &str) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(0x1B); // ESC
    out.push(b'_');
    out.extend_from_slice(content.as_bytes());
    out.push(0x07); // BEL
    out
}

fn apc_st_terminated(content: &str) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(0x1B);
    out.push(b'_');
    out.extend_from_slice(content.as_bytes());
    out.push(0x1B); // ESC
    out.push(b'\\'); // \ → ST
    out
}

fn first_event(output_chunks: Vec<ApcChunk>) -> ApcEvent {
    for chunk in output_chunks {
        if let ApcChunk::Event(e) = chunk {
            return e;
        }
    }
    panic!("no event in chunks");
}

#[test]
fn verb_msg_produces_signal_msg() {
    let mut ext = ApcExtractor::new();
    let out = ext.feed(&apc(
        r#"k2so:msg {"to":"rust-eng","text":"hello"}"#,
    ));
    assert!(out.drops.is_empty());
    let event = first_event(out.chunks);
    match event {
        ApcEvent::Signal(signal) => {
            assert!(
                matches!(signal.to, AgentAddress::Agent { ref name, .. } if name == "rust-eng")
            );
            assert!(
                matches!(signal.kind, SignalKind::Msg { ref text } if text == "hello")
            );
        }
        _ => panic!("expected Signal"),
    }
}

#[test]
fn verb_status_produces_signal_status() {
    let mut ext = ApcExtractor::new();
    let out = ext.feed(&apc(r#"k2so:status {"text":"scanning"}"#));
    let event = first_event(out.chunks);
    match event {
        ApcEvent::Signal(s) => assert!(
            matches!(s.kind, SignalKind::Status { ref text } if text == "scanning")
        ),
        _ => panic!("expected Signal"),
    }
}

#[test]
fn verb_presence_variants_all_parse() {
    for (state, expected) in [
        ("active", PresenceState::Active),
        ("idle", PresenceState::Idle),
        ("away", PresenceState::Away),
        ("stuck", PresenceState::Stuck),
    ] {
        let mut ext = ApcExtractor::new();
        let payload = format!(r#"k2so:presence {{"state":"{state}"}}"#);
        let out = ext.feed(&apc(&payload));
        let event = first_event(out.chunks);
        match event {
            ApcEvent::Signal(s) => assert!(
                matches!(s.kind, SignalKind::Presence { state } if state == expected)
            ),
            _ => panic!("expected Signal"),
        }
    }
}

#[test]
fn verb_reserve_and_release() {
    let mut ext = ApcExtractor::new();
    let mut out = ext.feed(&apc(
        r#"k2so:reserve {"paths":["src/lib.rs","Cargo.toml"]}"#,
    ));
    out.chunks.extend(
        ext.feed(&apc(r#"k2so:release {"paths":["src/lib.rs"]}"#))
            .chunks,
    );

    let events: Vec<_> = out
        .chunks
        .into_iter()
        .filter_map(|c| match c {
            ApcChunk::Event(e) => Some(e),
            _ => None,
        })
        .collect();
    assert_eq!(events.len(), 2);
    match &events[0] {
        ApcEvent::Signal(s) => match &s.kind {
            SignalKind::Reservation { paths, action } => {
                assert_eq!(paths, &vec!["src/lib.rs".to_string(), "Cargo.toml".to_string()]);
                assert!(matches!(action, ReservationAction::Claim));
            }
            _ => panic!("expected Reservation"),
        },
        _ => panic!("expected Signal"),
    }
    match &events[1] {
        ApcEvent::Signal(s) => match &s.kind {
            SignalKind::Reservation { action, .. } => {
                assert!(matches!(action, ReservationAction::Release));
            }
            _ => panic!("expected Reservation"),
        },
        _ => panic!("expected Signal"),
    }
}

#[test]
fn verb_task_lifecycle_all_phases() {
    for (phase, expected) in [
        ("started", TaskPhase::Started),
        ("done", TaskPhase::Done),
        ("blocked", TaskPhase::Blocked),
    ] {
        let mut ext = ApcExtractor::new();
        let payload = format!(
            r#"k2so:task {{"phase":"{phase}","ref":"work/inbox/foo.md"}}"#
        );
        let out = ext.feed(&apc(&payload));
        let event = first_event(out.chunks);
        match event {
            ApcEvent::Signal(s) => match s.kind {
                SignalKind::TaskLifecycle { phase, task_ref } => {
                    assert_eq!(phase, expected);
                    assert_eq!(task_ref.as_deref(), Some("work/inbox/foo.md"));
                }
                _ => panic!("expected TaskLifecycle"),
            },
            _ => panic!("expected Signal"),
        }
    }
}

#[test]
fn verb_tool_produces_semantic_tool_call() {
    let mut ext = ApcExtractor::new();
    let out = ext.feed(&apc(
        r#"k2so:tool {"name":"bash","id":"t_1","input":{"cmd":"ls"}}"#,
    ));
    let event = first_event(out.chunks);
    match event {
        ApcEvent::Semantic { kind, payload } => {
            assert_eq!(kind, SemanticKind::ToolCall);
            assert_eq!(payload["name"], json!("bash"));
            assert_eq!(payload["id"], json!("t_1"));
        }
        _ => panic!("expected Semantic ToolCall"),
    }
}

#[test]
fn verb_tool_result_produces_semantic_tool_result() {
    let mut ext = ApcExtractor::new();
    let out = ext.feed(&apc(
        r#"k2so:tool-result {"id":"t_1","ok":true,"output":"total 0"}"#,
    ));
    let event = first_event(out.chunks);
    match event {
        ApcEvent::Semantic { kind, payload } => {
            assert_eq!(kind, SemanticKind::ToolResult);
            assert_eq!(payload["id"], json!("t_1"));
            assert_eq!(payload["ok"], json!(true));
        }
        _ => panic!("expected Semantic ToolResult"),
    }
}

#[test]
fn unknown_verb_drops_with_reason() {
    let mut ext = ApcExtractor::new();
    let out = ext.feed(&apc(r#"k2so:totally-made-up {}"#));
    assert!(out.chunks.iter().all(|c| matches!(c, ApcChunk::Bytes(_))));
    assert_eq!(out.drops.len(), 1);
    assert!(matches!(
        &out.drops[0],
        ApcDrop::UnknownVerb(v) if v == "totally-made-up"
    ));
}

#[test]
fn bad_json_drops_with_reason() {
    let mut ext = ApcExtractor::new();
    let out = ext.feed(&apc(r#"k2so:msg {this is not json"#));
    assert!(out.chunks.iter().all(|c| matches!(c, ApcChunk::Bytes(_))));
    assert_eq!(out.drops.len(), 1);
    assert!(matches!(
        &out.drops[0],
        ApcDrop::BadPayload { verb, .. } if verb == "msg"
    ));
}

#[test]
fn non_k2so_namespace_drops_silently() {
    let mut ext = ApcExtractor::new();
    let out = ext.feed(&apc("pi:cursor 1,2"));
    assert!(out.chunks.iter().all(|c| matches!(c, ApcChunk::Bytes(_))));
    assert_eq!(out.drops.len(), 1);
    assert!(matches!(&out.drops[0], ApcDrop::NotOurNamespace));
}

#[test]
fn st_terminator_also_works() {
    let mut ext = ApcExtractor::new();
    let out = ext.feed(&apc_st_terminated(
        r#"k2so:status {"text":"via ST"}"#,
    ));
    let event = first_event(out.chunks);
    assert!(matches!(event, ApcEvent::Signal(_)));
}

#[test]
fn apc_split_across_two_feeds_yields_one_signal() {
    let full = apc(r#"k2so:msg {"to":"rust-eng","text":"split"}"#);
    // Split the bytes roughly in the middle of the payload.
    let midpoint = full.len() / 2;
    let mut ext = ApcExtractor::new();

    let first_half = ext.feed(&full[..midpoint]);
    // No event yet — APC isn't terminated.
    assert!(first_half.chunks.iter().all(|c| matches!(c, ApcChunk::Bytes(_))));
    assert!(first_half.drops.is_empty());

    let second_half = ext.feed(&full[midpoint..]);
    let events: Vec<_> = second_half
        .chunks
        .into_iter()
        .filter_map(|c| match c {
            ApcChunk::Event(e) => Some(e),
            _ => None,
        })
        .collect();
    assert_eq!(events.len(), 1);
    assert!(matches!(events[0], ApcEvent::Signal(_)));
}

#[test]
fn non_apc_escape_passes_through_unchanged() {
    let mut ext = ApcExtractor::new();
    // \x1b[2J is a CSI, not APC — extractor should NOT eat it.
    let out = ext.feed(b"\x1b[2J");
    // Pending ESC + following bytes all come back as passthrough.
    let bytes: Vec<u8> = out
        .chunks
        .into_iter()
        .flat_map(|c| match c {
            ApcChunk::Bytes(b) => b,
            _ => panic!("unexpected event"),
        })
        .collect();
    assert_eq!(bytes, b"\x1b[2J");
    assert!(out.drops.is_empty());
}

#[test]
fn interleaved_text_and_apc_preserves_order() {
    let mut ext = ApcExtractor::new();
    let mut input = b"pre".to_vec();
    input.extend_from_slice(&apc(r#"k2so:status {"text":"mid"}"#));
    input.extend_from_slice(b"post");
    let out = ext.feed(&input);
    assert_eq!(out.chunks.len(), 3);
    assert!(matches!(&out.chunks[0], ApcChunk::Bytes(b) if b == b"pre"));
    assert!(matches!(&out.chunks[1], ApcChunk::Event(ApcEvent::Signal(_))));
    assert!(matches!(&out.chunks[2], ApcChunk::Bytes(b) if b == b"post"));
}

// ─────────────────────────────────────────────────────────────────────
// LineMux integration — the extractor's output becomes Frames.
// ─────────────────────────────────────────────────────────────────────

#[test]
fn linemux_emits_agent_signal_frame_from_apc() {
    let mut mux = LineMux::new();
    let mut input = Vec::new();
    input.extend_from_slice(&apc(r#"k2so:msg {"to":"qa-eng","text":"ping"}"#));
    let frames = mux.feed(&input);
    let signal_frames: Vec<_> = frames
        .iter()
        .filter(|f| matches!(f, Frame::AgentSignal(_)))
        .collect();
    assert_eq!(signal_frames.len(), 1);
}

#[test]
fn linemux_emits_semantic_event_frame_from_tool_apc() {
    let mut mux = LineMux::new();
    let input = apc(r#"k2so:tool {"name":"bash","id":"t_42","input":{"cmd":"ls"}}"#);
    let frames = mux.feed(&input);
    let semantic_frames: Vec<_> = frames
        .iter()
        .filter(|f| matches!(f, Frame::SemanticEvent { kind: SemanticKind::ToolCall, .. }))
        .collect();
    assert_eq!(semantic_frames.len(), 1);
}

#[test]
fn linemux_interleaves_text_apc_text_in_correct_order() {
    let mut mux = LineMux::new();
    let mut input = b"pre".to_vec();
    input.extend_from_slice(&apc(r#"k2so:status {"text":"mid"}"#));
    input.extend_from_slice(b"post\n");
    let frames = mux.feed(&input);

    // Expected frame ordering: Text("pre"), AgentSignal(...), Text("post")
    assert!(frames.len() >= 3);
    match &frames[0] {
        Frame::Text { bytes, .. } => assert_eq!(bytes, b"pre"),
        other => panic!("expected Text frame, got {other:?}"),
    }
    assert!(matches!(&frames[1], Frame::AgentSignal(_)));
    match &frames[2] {
        // Post-Phase-4.5: the trailing `\n` terminator is now
        // preserved in Frame::Text bytes so grid consumers see
        // the line break.
        Frame::Text { bytes, .. } => assert_eq!(bytes, b"post\n"),
        other => panic!("expected Text frame, got {other:?}"),
    }

    // And the scrollback got one complete line "prepost".
    let lines: Vec<_> = mux.lines().collect();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].text, "prepost");
}

#[test]
fn linemux_no_regression_on_pure_text() {
    // Confirm C4's canonical test still holds after APC wiring.
    // Phase 4.5: LineMux now preserves `\n` in Frame::Text bytes so
    // downstream grid consumers know where lines break. Alacritty
    // always saw the delimiter from the PTY stream directly; this
    // brings LineMux's Frame::Text to parity.
    let mut mux = LineMux::new();
    let frames = mux.feed(b"hello\nworld\n");
    let text_frames: Vec<_> = frames
        .iter()
        .filter_map(|f| match f {
            Frame::Text { bytes, .. } => Some(bytes.as_slice()),
            _ => None,
        })
        .collect();
    assert_eq!(text_frames.len(), 2);
    assert_eq!(text_frames[0], b"hello\n");
    assert_eq!(text_frames[1], b"world\n");
}
