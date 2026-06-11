//! Line-oriented terminal producer.
//!
//! Sibling to `terminal/` (alacritty-backed single-grid path). The
//! `term/` tree holds the WezTerm-style line-mux pipeline that emits
//! client-agnostic `Line` and `Frame` events — the Producer A of the
//! Session Stream PRD.
//!
//! Originally gated behind `feature = "session_stream"`; the flag was
//! retired in 0.39.0e and these modules are now always compiled in.

pub mod apc;
pub mod line_mux;
pub mod recognizers;

pub use line_mux::LineMux;
pub use recognizers::{ClaudeCodeRecognizer, Recognizer};
