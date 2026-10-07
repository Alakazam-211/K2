//! The explicit ends (§5.4): every reason a row is held open has its own
//! way to stop holding it.
//!
//! | Held by | Ends on |
//! |---|---|
//! | subagent child | `SubagentStop` / child `StopFailure` / omitted from a lead `Stop` inventory |
//! | background shell, monitor | omitted from a later lead `Stop` inventory, its `<task-notification>`, `TaskStop` |
//! | owed notification | its `<task-notification>` prompt, or [`OWED_LEASE_MS`] after `max(owedAt, leadIdleSince)` |
//! | crons | a later `Stop` with empty `session_crons` |
//! | lead working, Ctrl-C/Esc | [`on_key`] + confirmation (DA26, A16) |
//! | agent process | owner pid gone or reused ([`process_exit`], `agent_exited`) |
//! | PTY | child exit or the liveness sweep ([`process_exit`], `pty_exited`) |

use crate::agent_hooks::envelope::{HookEnvelope, TaskEntry};

use super::claude::{arm_cancel_latch, lead_end, lead_waiting, lead_working};
use super::row::{
    Child, ChildKind, ChildOrigin, ChildState, EvidenceSource, LeadState, Outcome, PendingCancel,
    Reason, Row, TitleSignal, KEY_CONFIRM_WINDOW_MS, KEY_SETTLE_MS, OWED_LEASE_MS, ROSTER_CAP,
};

/// A client keystroke the daemon read on its way to the PTY (DA26).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyInput {
    /// An input that is exactly `\x1b`.
    Esc,
    /// `\x03`.
    CtrlC,
}

impl KeyInput {
    /// Classify one client input frame. Only a lone Esc or Ctrl-C counts;
    /// an escape sequence (`\x1b[A`) or pasted text never does.
    pub fn classify(input: &[u8]) -> Option<Self> {
        match input {
            b"\x1b" => Some(Self::Esc),
            b"\x03" => Some(Self::CtrlC),
            _ => None,
        }
    }
}

// ── Roster ───────────────────────────────────────────────────────────────

/// Insert a child unless the roster is full (DA24: only finished entries
/// are evicted, never running ones). Returns whether it is in the roster.
pub(crate) fn upsert_child(
    row: &mut Row,
    id: &str,
    kind: ChildKind,
    origin: ChildOrigin,
    background: bool,
    now: i64,
) -> bool {
    if let Some(child) = row.children.get_mut(id) {
        if child.state == ChildState::Owed {
            child.state = ChildState::Running;
            child.owed_at = None;
        }
        if child.kind == ChildKind::Unknown || origin == ChildOrigin::Inventory {
            child.kind = kind;
        }
        child.background |= background;
        return true;
    }
    if row.children.len() >= ROSTER_CAP {
        let oldest_owed = row
            .children
            .iter()
            .filter(|(_, c)| c.state == ChildState::Owed)
            .min_by_key(|(_, c)| c.owed_at)
            .map(|(k, _)| k.clone());
        match oldest_owed {
            Some(k) => {
                row.children.remove(&k);
            }
            None => return false,
        }
    }
    row.children.insert(
        id.to_string(),
        Child { kind, state: ChildState::Running, origin, since: now, owed_at: None, background },
    );
    true
}

fn kind_of(raw_type: &str) -> ChildKind {
    match raw_type {
        "shell" | "local_bash" | "bash" => ChildKind::Shell,
        "subagent" | "teammate" | "agent" => ChildKind::Subagent,
        "monitor" => ChildKind::Monitor,
        // `workflow` and anything new fail active (DA24).
        _ => ChildKind::Unknown,
    }
}

/// `Some(true)` running, `Some(false)` finished, `None` unknown (fails
/// active as an `unknown` child).
fn status_running(raw: &str) -> Option<bool> {
    match raw {
        "running" | "pending" | "in_progress" | "starting" | "queued" => Some(true),
        "completed" | "complete" | "done" | "failed" | "error" | "killed" | "stopped"
        | "cancelled" | "canceled" | "timeout" | "timed_out" => Some(false),
        _ => None,
    }
}

/// A lead `PostToolUse` launched a background task.
pub(crate) fn add_launched(row: &mut Row, task: &TaskEntry, now: i64) {
    upsert_child(row, &task.id, kind_of(&task.kind), ChildOrigin::Launch, true, now);
}

/// DA24: reconcile a `background_tasks` + `session_crons` inventory.
///
/// A lead `Stop` inventory is **complete**: an inventory or launch entry,
/// or an owed one, that it doesn't list as running is gone (omission =
/// removal), and a hook-tracked subagent goes too when the inventory
/// lists no running subagent at all. A `SubagentStop` inventory only adds:
/// it may describe the subagent's own tasks, not the lead's.
pub(crate) fn reconcile_inventory(row: &mut Row, env: &HookEnvelope, complete: bool, now: i64) {
    if let Some(tasks) = env.background_tasks.as_ref() {
        let mut running_ids = Vec::new();
        let mut any_subagent = false;
        for task in tasks {
            let (kind, running) = match status_running(&task.status) {
                Some(true) => (kind_of(&task.kind), true),
                Some(false) => (kind_of(&task.kind), false),
                None => (ChildKind::Unknown, true),
            };
            if !running {
                continue;
            }
            any_subagent |= kind == ChildKind::Subagent;
            running_ids.push(task.id.clone());
            upsert_child(row, &task.id, kind, ChildOrigin::Inventory, true, now);
        }
        if complete {
            let before = row.children.len();
            row.children.retain(|id, c| {
                if running_ids.iter().any(|r| r == id) || c.kind == ChildKind::Cron {
                    return true;
                }
                match (c.origin, c.state) {
                    (_, ChildState::Owed) => false,
                    (ChildOrigin::Inventory | ChildOrigin::Launch, _) => false,
                    (ChildOrigin::Hook, _) => c.kind == ChildKind::Subagent && any_subagent,
                    (ChildOrigin::Cron, _) => true,
                }
            });
            if row.children.len() < before {
                row.end_reason = Some(Reason::ChildDone);
            }
        }
    }
    match env.session_crons {
        Some(n) if n > 0 => {
            upsert_child(row, CRON_ID, ChildKind::Cron, ChildOrigin::Cron, true, now);
        }
        Some(_) if complete => {
            if row.children.remove(CRON_ID).is_some() {
                row.end_reason = Some(Reason::CronsCleared);
            }
        }
        _ => {}
    }
}

/// The one roster entry standing for "session crons are scheduled".
const CRON_ID: &str = "session-crons";

// ── Timers ───────────────────────────────────────────────────────────────

/// Fire every timer due at `now`: the key settle and confirm window, and
/// the owed-notification lease.
pub(crate) fn run_timers(row: &mut Row, now: i64) {
    if let Some(pc) = row.pending_cancel.clone() {
        if pc.settle_at.is_some_and(|t| now >= t) {
            let unchanged = row.evidence_seq == pc.evidence_seq;
            let still = if pc.dismiss {
                row.lead.state == LeadState::Waiting
            } else {
                row.lead.state == LeadState::Working
            };
            if unchanged && still {
                let reason = if pc.dismiss { Reason::PromptDismissed } else { Reason::Interrupted };
                commit_cancel(row, reason, pc.settle_at.unwrap_or(now));
            } else if let Some(p) = row.pending_cancel.as_mut() {
                p.settle_at = None;
            }
        }
        if row.pending_cancel.as_ref().is_some_and(|p| now >= p.expires_at) {
            row.pending_cancel = None;
        }
    }
    if row.lead.state == LeadState::Idle {
        let idle_since = row.lead_idle_since.unwrap_or(row.lead.since);
        let expired: Vec<String> = row
            .children
            .iter()
            .filter(|(_, c)| c.state == ChildState::Owed)
            .filter(|(_, c)| now >= c.owed_at.unwrap_or(c.since).max(idle_since) + OWED_LEASE_MS)
            .map(|(k, _)| k.clone())
            .collect();
        if !expired.is_empty() {
            for k in expired {
                row.children.remove(&k);
            }
            row.end_reason = Some(Reason::OwedExpired);
        }
    }
}

/// The earliest due timer (decay is added by [`Row::next_deadline`]).
pub(crate) fn next_timer(row: &Row) -> Option<i64> {
    let mut best: Option<i64> = None;
    let mut take = |t: i64| best = Some(best.map_or(t, |b| b.min(t)));
    if let Some(pc) = row.pending_cancel.as_ref() {
        take(pc.settle_at.unwrap_or(pc.expires_at).min(pc.expires_at));
    }
    if row.lead.state == LeadState::Idle {
        let idle_since = row.lead_idle_since.unwrap_or(row.lead.since);
        for c in row.children.values().filter(|c| c.state == ChildState::Owed) {
            take(c.owed_at.unwrap_or(c.since).max(idle_since) + OWED_LEASE_MS);
        }
    }
    best
}

// ── Interrupts (DA26, A15, A16) ─────────────────────────────────────────

/// A keystroke. Only rows driven by hooks (Claude) infer from keys; a
/// title-only row (Grok, Codex) gets its idle from the title or, in S3,
/// its transcript.
pub(crate) fn on_key(row: &mut Row, key: KeyInput, now: i64) {
    if !row.had_hook {
        return;
    }
    match (row.lead.state, key) {
        (LeadState::Working, _) => {
            // A16: Esc or Ctrl-C opens a 3 s window; the transcript's
            // `[Request interrupted by user` record or
            // `PostToolUseFailure.is_interrupt` commits it. Ctrl-C keeps
            // the 500 ms settle as a fallback while no transcript resolves.
            // A plain Esc with no confirmation changes nothing.
            let settle = key == KeyInput::CtrlC && !row.transcript_resolvable;
            row.pending_cancel = Some(PendingCancel {
                dismiss: false,
                evidence_seq: row.evidence_seq,
                settle_at: settle.then_some(now + KEY_SETTLE_MS),
                expires_at: now + KEY_CONFIRM_WINDOW_MS,
            });
        }
        (LeadState::Waiting, KeyInput::Esc) => {
            row.pending_cancel = Some(PendingCancel {
                dismiss: true,
                evidence_seq: row.evidence_seq,
                settle_at: Some(now + KEY_SETTLE_MS),
                expires_at: now + KEY_SETTLE_MS,
            });
        }
        // Ctrl-C at an idle lead cancels nothing: children keep the row.
        _ => {}
    }
}

/// Commit a cancel: lead idle/cancelled, latch 15 s (DA25).
pub(crate) fn commit_cancel(row: &mut Row, reason: Reason, now: i64) {
    lead_end(row, Outcome::Cancelled, reason, now);
    arm_cancel_latch(row, now);
}

/// S3 seam: the transcript's `[Request interrupted by user` record. It
/// confirms an open key window, and with no keystroke at all it is a
/// cancel on its own (A16).
pub(crate) fn transcript_interrupt(row: &mut Row, now: i64) {
    row.note_evidence(EvidenceSource::Transcript, now);
    row.had_transcript = true;
    if row.lead.state != LeadState::Idle {
        commit_cancel(row, Reason::Interrupted, now);
    }
}

/// S3 seam: a transcript turn end (DA27). Lead idle, reason
/// `transcript_turn_end`.
pub(crate) fn transcript_turn_end(row: &mut Row, now: i64) {
    row.note_evidence(EvidenceSource::Transcript, now);
    row.had_transcript = true;
    if row.lead.state != LeadState::Idle {
        lead_end(row, Outcome::Success, Reason::TranscriptTurnEnd, now);
    }
}

// ── Title, process ───────────────────────────────────────────────────────

/// DA28: the title observer counts only for a session that has had no
/// hook and no transcript evidence since registration.
pub(crate) fn apply_title(row: &mut Row, signal: TitleSignal, now: i64) {
    if row.had_hook || row.had_transcript {
        return;
    }
    row.note_evidence(EvidenceSource::Title, now);
    match signal {
        TitleSignal::Working => lead_working(row, now),
        TitleSignal::Permission => lead_waiting(row, false, None, None, now),
        TitleSignal::Idle => {
            if row.lead.state != LeadState::Idle {
                lead_end(row, Outcome::Success, Reason::TurnDone, now);
            }
        }
    }
}

/// The agent process or the PTY is gone: lead idle, roster wiped.
pub(crate) fn process_exit(row: &mut Row, reason: Reason, now: i64) {
    let already = row.lead.state == LeadState::Idle
        && row.children.is_empty()
        && row.lead.end_reason == Some(reason);
    if already {
        return;
    }
    row.note_evidence(EvidenceSource::Process, now);
    row.children.clear();
    row.latch = None;
    lead_end(row, Outcome::None, reason, now);
}
