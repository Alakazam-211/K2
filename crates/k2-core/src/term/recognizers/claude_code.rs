//! Claude Code T0.5 recognizer.
//!
//! Claude Code (TS and the claw-code Rust port) renders tool calls,
//! tool results, and plan blocks as bordered panels in its TUI:
//!
//! ```text
//! ╭───────────── Tool Call: Bash ─────────────╮
//! │ $ ls                                       │
//! ╰────────────────────────────────────────────╯
//! ```
//!
//! At mobile width, these borders break catastrophically under naive
//! reflow (the baked-width problem). This recognizer lifts the
//! panels out as structured `SemanticEvent` frames so mobile can
//! re-render them natively at phone width.
//!
//! Recognized titles (case-insensitive, prefix match):
//!   - "Tool Call..."   → `SemanticKind::ToolCall`
//!   - "Tool Result..." → `SemanticKind::ToolResult`
//!   - "Plan"           → `SemanticKind::Plan`
//!   - other            → `SemanticKind::Message`
//!
//! Additive: non-box lines pass through with no semantic frame;
//! corrupted boxes reset state instead of panicking.

use serde_json::json;

use super::Recognizer;
use crate::session::{Frame, Line, SemanticKind};

pub struct ClaudeCodeRecognizer {
    state: State,
    title: String,
    body: Vec<String>,
}

enum State {
    Idle,
    InBox,
}

impl Default for ClaudeCodeRecognizer {
    fn default() -> Self {
        Self {
            state: State::Idle,
            title: String::new(),
            body: Vec::new(),
        }
    }
}

impl ClaudeCodeRecognizer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Abandon any in-progress box. Called when something doesn't
    /// match our expectations — better to lose one panel than to
    /// emit garbage semantic events.
    fn reset(&mut self) {
        self.state = State::Idle;
        self.title.clear();
        self.body.clear();
    }
}

impl Recognizer for ClaudeCodeRecognizer {
    fn on_line(&mut self, line: &Line) -> Vec<Frame> {
        let text = line.text.as_str();
        match self.state {
            State::Idle => {
                if let Some(title) = parse_top_border(text) {
                    self.title = title;
                    self.body.clear();
                    self.state = State::InBox;
                }
                Vec::new()
            }
            State::InBox => {
                if is_bottom_border(text) {
                    let frames = emit_semantic(&self.title, &self.body);
                    self.reset();
                    frames
                } else if let Some(content) = parse_box_interior(text) {
                    self.body.push(content);
                    Vec::new()
                } else {
                    // Interrupted box — Claude cleared the screen or
                    // emitted unexpected output mid-panel. Drop the
                    // partial without panicking.
                    self.reset();
                    Vec::new()
                }
            }
        }
    }
}

/// Match `╭─...─ <title> ─...─╮`. Returns the title if the line is
/// recognizably a top border. Empty title is allowed for untitled
/// panels.
fn parse_top_border(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if !trimmed.starts_with('╭') || !trimmed.ends_with('╮') {
        return None;
    }
    let inner = trimmed.trim_start_matches('╭').trim_end_matches('╮');
    let title = inner
        .trim_matches(|c: char| c == '─' || c.is_whitespace())
        .to_string();
    Some(title)
}

/// Match `╰─...─╯` — a bottom border with only box-drawing runs
/// between the corners.
fn is_bottom_border(text: &str) -> bool {
    let trimmed = text.trim();
    if !trimmed.starts_with('╰') || !trimmed.ends_with('╯') {
        return false;
    }
    trimmed
        .chars()
        .all(|c| c == '╰' || c == '╯' || c == '─' || c.is_whitespace())
}

/// Match `│ <content> │` and return the content (trimmed).
fn parse_box_interior(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if !trimmed.starts_with('│') || !trimmed.ends_with('│') {
        return None;
    }
    let inner = trimmed.trim_start_matches('│').trim_end_matches('│');
    Some(inner.trim().to_string())
}

fn emit_semantic(title: &str, body: &[String]) -> Vec<Frame> {
    let title_lower = title.to_lowercase();
    let kind = if title_lower.starts_with("tool call") {
        SemanticKind::ToolCall
    } else if title_lower.starts_with("tool result") {
        SemanticKind::ToolResult
    } else if title_lower == "plan" || title_lower.starts_with("plan:") {
        SemanticKind::Plan
    } else {
        SemanticKind::Message
    };
    vec![Frame::SemanticEvent {
        kind,
        payload: json!({
            "title": title,
            "body": body.join("\n"),
            "lines": body,
        }),
    }]
}
