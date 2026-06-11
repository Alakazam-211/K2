//! Per-harness T0.5 recognizers.
//!
//! A recognizer inspects each committed `Line` and optionally emits
//! `SemanticEvent` frames derived from the harness's ANSI-grid
//! output patterns. This is the tier between raw VTE parsing (T0)
//! and structured-mode adapters (T1) in the PRD at
//! `.k2so/prds/session-stream-and-awareness-bus.md`.
//!
//! One file per harness family — keeps the surface contained and
//! easy to iterate on as each harness's output format drifts.
//!
//! **Additive by design.** A recognizer that fails to match its
//! harness's patterns emits nothing; the underlying Text / CursorOp
//! frames still flow. Mobile downgrades to T0 (degraded but
//! legible), never breaks. Recognizers also must not panic on
//! corrupted patterns — all edge cases reset state cleanly.

use crate::session::{Frame, Line};

/// Trait implemented by per-harness recognizers. `LineMux` calls
/// `on_line` once per committed `Line` and extends the outgoing
/// frame buffer with whatever the recognizer returns.
pub trait Recognizer: Send + 'static {
    /// Inspect a newly-committed line; emit zero or more frames.
    fn on_line(&mut self, line: &Line) -> Vec<Frame>;
}

pub mod claude_code;

pub use claude_code::ClaudeCodeRecognizer;
