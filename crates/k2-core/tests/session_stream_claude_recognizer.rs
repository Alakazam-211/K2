//! C6 Claude Code recognizer tests.
//!
//! Covers the box-drawn-panel extraction logic against synthetic
//! inputs shaped like Claude Code's real output. Recorded `.bin`
//! fixtures from an actual `claude` session are a follow-up task
//! — the synthetic corpus here proves the recognizer's state
//! machine and the recognizer-attached LineMux integration path.

use k2_core::session::{Frame, SemanticKind};
use k2_core::term::{ClaudeCodeRecognizer, LineMux};

fn mux_with_claude_recognizer() -> LineMux {
    LineMux::new().with_recognizer(Box::new(ClaudeCodeRecognizer::new()))
}

fn semantic_frames(frames: &[Frame]) -> Vec<(&SemanticKind, &serde_json::Value)> {
    frames
        .iter()
        .filter_map(|f| match f {
            Frame::SemanticEvent { kind, payload } => Some((kind, payload)),
            _ => None,
        })
        .collect()
}

// ─────────────────────────────────────────────────────────────────────
// Happy path — the three title mappings we care about
// ─────────────────────────────────────────────────────────────────────

#[test]
fn tool_call_box_emits_tool_call_semantic_event() {
    let mut mux = mux_with_claude_recognizer();
    let input = concat!(
        "╭──────── Tool Call: Bash ────────╮\n",
        "│ $ ls                             │\n",
        "╰──────────────────────────────────╯\n"
    );
    let frames = mux.feed(input.as_bytes());
    let semantic = semantic_frames(&frames);
    assert_eq!(semantic.len(), 1);
    assert_eq!(semantic[0].0, &SemanticKind::ToolCall);
    // Body contains the content line.
    let body = semantic[0].1.get("body").and_then(|v| v.as_str()).unwrap();
    assert!(body.contains("$ ls"), "body was: {body}");
    // Title preserved for the renderer to use.
    assert_eq!(
        semantic[0].1.get("title").and_then(|v| v.as_str()),
        Some("Tool Call: Bash")
    );
}

#[test]
fn tool_result_box_emits_tool_result_semantic_event() {
    let mut mux = mux_with_claude_recognizer();
    let input = concat!(
        "╭─ Tool Result ─╮\n",
        "│ total 0        │\n",
        "│ drwxr-xr-x .   │\n",
        "╰────────────────╯\n"
    );
    let frames = mux.feed(input.as_bytes());
    let semantic = semantic_frames(&frames);
    assert_eq!(semantic.len(), 1);
    assert_eq!(semantic[0].0, &SemanticKind::ToolResult);
    // Multi-line body joined with newlines.
    let body = semantic[0].1.get("body").and_then(|v| v.as_str()).unwrap();
    assert!(body.contains("total 0"), "body was: {body}");
    assert!(body.contains("drwxr-xr-x ."), "body was: {body}");
    // `lines` array preserves per-line structure for UIs that want it.
    let lines = semantic[0].1.get("lines").and_then(|v| v.as_array()).unwrap();
    assert_eq!(lines.len(), 2);
}

#[test]
fn plan_box_emits_plan_semantic_event() {
    let mut mux = mux_with_claude_recognizer();
    let input = concat!(
        "╭─ Plan ─╮\n",
        "│ 1. Do X │\n",
        "│ 2. Do Y │\n",
        "╰─────────╯\n"
    );
    let frames = mux.feed(input.as_bytes());
    let semantic = semantic_frames(&frames);
    assert_eq!(semantic.len(), 1);
    assert_eq!(semantic[0].0, &SemanticKind::Plan);
}

#[test]
fn unknown_titled_box_falls_back_to_message() {
    let mut mux = mux_with_claude_recognizer();
    let input = concat!(
        "╭─ Free-form panel ─╮\n",
        "│ greetings          │\n",
        "╰────────────────────╯\n"
    );
    let frames = mux.feed(input.as_bytes());
    let semantic = semantic_frames(&frames);
    assert_eq!(semantic.len(), 1);
    assert_eq!(semantic[0].0, &SemanticKind::Message);
}

// ─────────────────────────────────────────────────────────────────────
// Graceful degradation
// ─────────────────────────────────────────────────────────────────────

#[test]
fn corrupted_top_border_without_close_does_not_panic() {
    let mut mux = mux_with_claude_recognizer();
    // Top border → interior line → SOMETHING OTHER than interior /
    // bottom border → top border again → real close.
    let input = concat!(
        "╭── Tool Call: Bash ──╮\n",
        "│ $ ls                 │\n",
        "random interruption\n",
        "╭─ Tool Result ─╮\n",
        "│ total 0        │\n",
        "╰────────────────╯\n"
    );
    let frames = mux.feed(input.as_bytes());
    let semantic = semantic_frames(&frames);
    // First box is discarded (interrupted); second box emits normally.
    assert_eq!(semantic.len(), 1);
    assert_eq!(semantic[0].0, &SemanticKind::ToolResult);
    // No panic.
}

#[test]
fn non_box_text_produces_no_semantic_events() {
    let mut mux = mux_with_claude_recognizer();
    let frames = mux.feed(b"just a plain line\nanother plain line\n");
    let semantic = semantic_frames(&frames);
    assert!(semantic.is_empty());
    // Text frames still flow.
    let text_frames: Vec<_> = frames
        .iter()
        .filter(|f| matches!(f, Frame::Text { .. }))
        .collect();
    assert_eq!(text_frames.len(), 2);
}

#[test]
fn interior_without_top_border_is_not_consumed_as_box() {
    let mut mux = mux_with_claude_recognizer();
    // A `│...│` line without a top border first. Recognizer stays
    // in Idle, emits nothing, Text frame flows through.
    let frames = mux.feed("│ not in a box │\n".as_bytes());
    assert!(semantic_frames(&frames).is_empty());
}

#[test]
fn bottom_border_without_top_is_ignored() {
    let mut mux = mux_with_claude_recognizer();
    let frames = mux.feed("╰────╯\n".as_bytes());
    assert!(semantic_frames(&frames).is_empty());
}

#[test]
fn two_sequential_boxes_both_emit() {
    let mut mux = mux_with_claude_recognizer();
    let input = concat!(
        "╭─ Tool Call: a ─╮\n",
        "│ first           │\n",
        "╰─────────────────╯\n",
        "╭─ Tool Result ─╮\n",
        "│ ok              │\n",
        "╰─────────────────╯\n"
    );
    let frames = mux.feed(input.as_bytes());
    let semantic = semantic_frames(&frames);
    assert_eq!(semantic.len(), 2);
    assert_eq!(semantic[0].0, &SemanticKind::ToolCall);
    assert_eq!(semantic[1].0, &SemanticKind::ToolResult);
}

// ─────────────────────────────────────────────────────────────────────
// No-recognizer regression
// ─────────────────────────────────────────────────────────────────────

#[test]
fn linemux_without_recognizer_emits_no_semantic_events_for_box_input() {
    let mut mux = LineMux::new(); // no recognizer attached
    let input = concat!(
        "╭─ Tool Call: Bash ─╮\n",
        "│ $ ls               │\n",
        "╰────────────────────╯\n"
    );
    let frames = mux.feed(input.as_bytes());
    let semantic = semantic_frames(&frames);
    assert!(
        semantic.is_empty(),
        "no recognizer attached — no semantic events expected, got {semantic:?}"
    );
    // Three Text frames (one per line) still flow.
    let text_count = frames
        .iter()
        .filter(|f| matches!(f, Frame::Text { .. }))
        .count();
    assert_eq!(text_count, 3);
}

// ─────────────────────────────────────────────────────────────────────
// SGR + recognizer — vte strips color, recognizer sees unstyled text
// ─────────────────────────────────────────────────────────────────────

#[test]
fn recognizer_works_across_sgr_color_sequences() {
    let mut mux = mux_with_claude_recognizer();
    // Box with SGR colour sequences sprinkled in — vte's parser
    // eats the SGR CSIs, leaving plain box chars in `Line.text`.
    let input = concat!(
        "\x1b[36m╭── Tool Call: Bash ──╮\x1b[0m\n",
        "\x1b[36m│\x1b[0m \x1b[1m$ ls\x1b[0m                 \x1b[36m│\x1b[0m\n",
        "\x1b[36m╰──────────────────────╯\x1b[0m\n"
    );
    let frames = mux.feed(input.as_bytes());
    let semantic = semantic_frames(&frames);
    assert_eq!(
        semantic.len(),
        1,
        "recognizer should match despite SGR sequences: got {semantic:?}"
    );
    assert_eq!(semantic[0].0, &SemanticKind::ToolCall);
}
