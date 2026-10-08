//! The per-session activity row and its state machine
//! (prd-daemon-activity-and-thread-working-v1 S2: DA19–DA26, DA29–DA32).
//!
//! Pure and clock-injected: every entry point takes `now` (unix ms, the
//! daemon's receipt clock), nothing here reads the time, the disk, a
//! process, or the database. The daemon's `activity_store` owns one
//! [`Row`] per live v2 session and performs the side effects a
//! [`Change`] asks for (the `workspace_sessions.status` lock release,
//! the compat lifecycle emits, the Active touch).
//!
//! - [`row`]: the row, its vocabulary, and its JSON (§7.2).
//! - [`claude`]: Claude hook envelopes → lead state and roster (§6.1).
//! - [`fold`]: lead + roster → display and reason, then decay (§6.2).
//! - [`ends`]: the explicit ends (§5.4): inventory reconcile, the owed
//!   lease, interrupt inference, process exits.
//! - [`lock`]: when the row may touch the session lock (DA32).
//! - [`transcript`]: transcript records → evidence (S3: DA27, A16, A19).
//! - [`screen`]: the interrupt-marker scan for titleless CLIs (S3: A13).
//! - [`tools`]: which tool calls are shell commands (the per-turn
//!   `tools` / `commands` counts, [`Row::turn_counts`]).
//!
//! Silence is never "done" (DA3): no timer moves a row to `idle`. Time
//! only decays a row to `unverifiable` ([`row::STALE_AFTER_MS`]) or ends
//! a lease that was taken on evidence (the owed notification, the
//! Ctrl-C settle).
//!
//! Seams for later slices: [`Evidence::Transcript`] (S3 feeds it from the
//! transcript follower; [`Evidence::TranscriptInterrupt`] and
//! [`Evidence::TranscriptTurnEnd`] are the same ends without a record),
//! [`Row::set_transcript_resolvable`] (S3), [`Change`] and
//! [`Row::to_json`] (S4's `activity_changed` and snapshot), and
//! [`Change::turn_ended`] (S6's Thread turn tracker).

pub mod claude;
pub mod ends;
pub mod fold;
pub mod lock;
pub mod row;
pub mod screen;
pub mod tools;
pub mod transcript;

pub use ends::KeyInput;
pub use row::{
    Change, ChildKind, Display, EvidenceSource, LeadState, Outcome, Reason, Row, RowFacts,
    TitleSignal, TurnCounts, TurnEnded,
};
pub use transcript::{TranscriptReader, TranscriptSignal};

use crate::agent_hooks::envelope::HookEnvelope;

/// One piece of evidence (or one input) for a row.
#[derive(Debug, Clone, Copy)]
pub enum Evidence<'a> {
    /// An owner envelope from `POST /hook/event` (DA14 already passed).
    Hook(&'a HookEnvelope),
    /// A legacy `GET /hook/complete` bucket (`start` | `stop` |
    /// `permission`) from a live pane (DA13). No payload, no pid.
    LegacyHook(&'a str),
    /// The PTY title/bell observer (DA28). Counts only while the row has
    /// had no hook and no transcript evidence since registration.
    Title(TitleSignal),
    /// A client keystroke read in the daemon (DA26). Not evidence of the
    /// agent; it can only open an inference window.
    Key(KeyInput),
    /// S3 seam: a Claude transcript user record starting
    /// `[Request interrupted by user` (A16).
    TranscriptInterrupt,
    /// S3 seam: a transcript turn end (Claude `end_turn` with no hook for
    /// 5 s, Codex `task_complete`, Grok final assistant record).
    TranscriptTurnEnd,
    /// One record from the session's transcript (DA27). Drives the lead
    /// both ways only while the row has had no hook since registration;
    /// with hooks it is evidence, a cancel, or Claude's 5 s `end_turn`
    /// cross-check.
    Transcript(&'a TranscriptSignal),
    /// The screen marker scan (A13): `true` the harness's interrupt
    /// marker is on screen, `false` it has been gone 3 s. Counts only
    /// while the row has had no hook and no transcript evidence.
    Screen(bool),
    /// The pane's owner process exited or its pid was reused (DA14-6).
    OwnerReleased,
    /// The PTY child exited (`ChildExit`, or the 10 s liveness sweep).
    PtyExited,
}

#[cfg(test)]
mod replay;
