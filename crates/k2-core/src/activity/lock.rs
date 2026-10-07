//! When a row may touch `workspace_sessions.status` (DA32, A14) and which
//! legacy lifecycle word it stands for (RL5).
//!
//! `status = 'running'` is a "session claimed" lock with about ten
//! writers; the legacy scheduler re-injects its wake prompt into a live
//! PTY when it isn't locked (`wake_headless.rs`). So the store never owns
//! the column. It:
//! - writes `permission` when the row goes to waiting;
//! - writes `running` when the lead is working on confirmed evidence (the
//!   old hook `start` writer, now behind the owner check), including
//!   waiting → working;
//! - writes `sleeping` only on evidence of a turn end after the lead was
//!   confirmed working in this registration, on a process exit, and on
//!   decay to `unverifiable`;
//! - writes nothing at all while the row is unconfirmed (a restarted
//!   daemon must not release a lock it never saw taken).
//!
//! `monitoring` (lead idle, background work) releases the lock, so a
//! heartbeat may fire into it (Q4).

use super::row::{Display, LeadState, Reason, Row};

/// Lead end reasons that prove a turn finished (A14 b).
pub const TURN_END_REASONS: &[Reason] = &[
    Reason::TurnDone,
    Reason::TurnFailed,
    Reason::Interrupted,
    // Esc on a permission prompt cancels the turn just like Ctrl-C.
    Reason::PromptDismissed,
    Reason::IdlePrompt,
    Reason::TranscriptTurnEnd,
    Reason::Compacted,
    Reason::SessionBoundary,
];

/// Process ends release the lock whether or not a turn was seen.
pub const EXIT_REASONS: &[Reason] = &[Reason::AgentExited, Reason::PtyExited];

/// The status this row wants in `workspace_sessions`, or `None` to leave
/// the column alone. The caller writes only when it differs from the
/// last value it wrote.
pub fn status_target(row: &Row) -> Option<&'static str> {
    if !row.confirmed {
        return None;
    }
    match row.display {
        // A working lead keeps its claim while one of its subagents waits.
        Display::Waiting if row.lead.state == LeadState::Working => Some("running"),
        Display::Waiting => Some("permission"),
        Display::Working if row.lead.state == LeadState::Working => Some("running"),
        // Lead idle, subagents still running: the turn isn't over.
        Display::Working => None,
        Display::Unverifiable => Some("sleeping"),
        Display::Idle | Display::Monitoring => {
            let end = row.lead.end_reason?;
            let ended = EXIT_REASONS.contains(&end)
                || (row.confirmed_working && TURN_END_REASONS.contains(&end));
            ended.then_some("sleeping")
        }
    }
}

/// RL5: the legacy `agent_status_changed` / `agent:lifecycle` bucket for
/// this row, derived from the display (never from a raw hook).
pub fn compat_target(row: &Row) -> Option<&'static str> {
    if !row.confirmed {
        return None;
    }
    Some(match row.display {
        Display::Working | Display::Monitoring => "start",
        Display::Waiting => "permission",
        Display::Idle | Display::Unverifiable => "stop",
    })
}
