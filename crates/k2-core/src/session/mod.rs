//! Session primitive — the atom of the 0.34.0 Session Stream.
//!
//! This is primitive A from the PRD at
//! `.k2so/prds/session-stream-and-awareness-bus.md`. A session here
//! is a typed event stream owned by the daemon with many producers
//! and many consumers — the unit that lets multiple devices view the
//! same session at their own width without fighting over a shared
//! grid. The harness-neutral `Frame` and width-free `Line` types
//! are the vocabulary every consumer speaks.
//!
//! Phase 1 (commits C1-C6) defined the types. Phase 2 (commits
//! D1-D7) landed the runtime plumbing: per-session broadcast
//! channel + replay ring (`entry` / `registry`), dual-emit PTY
//! reader (`crate::terminal::session_stream_pty`), daemon WS
//! subscribe endpoint (`crates/k2so-daemon/src/sessions_ws.rs`),
//! and a smoke-test consumer (`crates/k2so-core/examples/
//! session_stream_subscribe.rs`). Phase 3 adds the Awareness Bus
//! routing + archive NDJSON writer.

pub mod archive;
pub mod bytes_ring;
pub mod entry;
pub mod frame;
pub mod line;
pub mod registry;
pub mod types;
pub mod watchdog;

pub use archive::{
    parse_rotation_index, rotated_uncompressed_segments, spawn as spawn_archive,
    HARD_LIMIT_BYTES, ROTATE_BYTES, WARN_BYTES,
};
pub use bytes_ring::{BytesRing, RingChunk, BYTES_RING_CAP};
pub use entry::{SessionEntry, BROADCAST_CAP, BYTES_BROADCAST_CAP, REPLAY_CAP};
pub use frame::{
    CursorOp, CursorShape, EraseMode, Frame, ModeKind, SemanticKind, Style,
};
pub use line::{Line, SeqnoGen, SequenceNo};
pub use types::{HarnessKind, Session, SessionId};
pub use watchdog::{
    evaluate as watchdog_evaluate, mark_fired as watchdog_mark_fired, Escalation,
    EscalationState, WatchdogConfig,
};
