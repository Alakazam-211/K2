//! Claude hook envelopes → lead state and roster (§6.1, DA20–DA25).
//!
//! Only owner envelopes get here (DA14 ran in the ingest). Child events
//! (`agent_id` set) touch the roster only (DA20). Cursor and Gemini
//! envelopes, and legacy `/hook/complete` posts, map through the old
//! three buckets ([`apply_bucket`]).

use crate::agent_hooks::envelope::{HookEnvelope, HookSource};
use crate::agent_hooks::map_event_type;

use super::ends;
use super::row::{
    ChildKind, ChildOrigin, ChildState, EvidenceSource, LeadState, Latch, Outcome, Reason, Row,
    TurnEnded, Waiting, CANCEL_LATCH_MS, IN_FLIGHT_CAP,
};

/// The tool whose `PreToolUse` means the lead is asking the user.
const ASK_TOOL: &str = "AskUserQuestion";

/// Apply one owner envelope.
pub(crate) fn apply_hook(row: &mut Row, env: &HookEnvelope, now: i64) {
    row.note_evidence(EvidenceSource::Hook, now);
    row.had_hook = true;
    if env.source != HookSource::Claude {
        // Cursor / Gemini: today's three buckets (their own event names).
        if let Some(bucket) = map_event_type(&env.event) {
            apply_bucket_inner(row, bucket, now);
        }
        return;
    }
    if let Some(agent_id) = env.agent_id.as_deref() {
        apply_child(row, env, agent_id, now);
        return;
    }
    let tool = env.tool_name.as_deref().unwrap_or("");
    match env.event.as_str() {
        "SessionStart" => {
            if matches!(env.source_field.as_deref(), Some("startup" | "resume" | "clear")) {
                session_boundary(row, Reason::SessionBoundary, now);
            }
        }
        "UserPromptSubmit" => {
            for id in &env.task_notification_ids {
                if row.children.remove(id).is_some() {
                    row.end_reason = Some(Reason::ChildDone);
                }
            }
            row.latch = None;
            row.pending_cancel = None;
            row.in_flight.clear();
            row.lead.prompt_id = env.prompt_id.clone();
            row.turn_started_at = Some(now);
            lead_working(row, now);
        }
        "PreToolUse" => {
            if latched(row, env, now) {
                return;
            }
            if let Some(id) = env.tool_use_id.as_deref() {
                remember_in_flight(row, id, tool);
            }
            if row.lead.state == LeadState::Waiting {
                // A17: never clears waiting; a late async PreToolUse binds
                // the pending `waitingFor` instead.
                if let Some(w) = row.lead.waiting.as_mut() {
                    if w.waiting_for.is_none() && w.tool_name.as_deref() == Some(tool) {
                        w.waiting_for = env.tool_use_id.clone();
                    }
                }
                return;
            }
            if tool == ASK_TOOL {
                lead_waiting(row, true, env.tool_use_id.clone(), Some(tool.to_string()), now);
            } else {
                lead_working(row, now);
            }
        }
        "PostToolUse" | "PostToolUseFailure" => {
            let id = env.tool_use_id.as_deref();
            if let Some(id) = id {
                row.in_flight.retain(|(t, _)| t != id);
            }
            if let Some(task) = env.launched_task.as_ref() {
                ends::add_launched(row, task, now);
            }
            if let Some(stopped) = env.stopped_task_id.as_deref() {
                // Claude sends no notification for a stopped task.
                if row.children.remove(stopped).is_some() {
                    row.end_reason = Some(Reason::ChildDone);
                }
            }
            if env.event == "PostToolUseFailure" && env.is_interrupt == Some(true) {
                if row.lead.state != LeadState::Idle {
                    ends::commit_cancel(row, Reason::Interrupted, now);
                }
                return;
            }
            if latched(row, env, now) {
                return;
            }
            if row.lead.state == LeadState::Waiting {
                let waited = row.lead.waiting.as_ref().and_then(|w| w.waiting_for.as_deref());
                if waited.is_some() && waited == id {
                    lead_working(row, now);
                }
                return;
            }
            lead_working(row, now);
        }
        "PermissionRequest" => {
            if latched(row, env, now) {
                return;
            }
            // DA23: the latest same-tool PreToolUse with no Post yet.
            let waiting_for = row
                .in_flight
                .iter()
                .rev()
                .find(|(_, name)| name == tool)
                .map(|(id, _)| id.clone());
            lead_waiting(row, false, waiting_for, Some(tool.to_string()), now);
        }
        "PermissionDenied" => {
            let id = env.tool_use_id.as_deref();
            if let Some(id) = id {
                row.in_flight.retain(|(t, _)| t != id);
            }
            if row.lead.state == LeadState::Waiting && !latched(row, env, now) {
                let waited = row.lead.waiting.as_ref().and_then(|w| w.waiting_for.as_deref());
                if waited.is_none() || waited == id {
                    lead_working(row, now);
                }
            }
        }
        "Notification" => match env.notification_type.as_deref() {
            Some("idle_prompt") if row.lead.state == LeadState::Working => {
                lead_end(row, Outcome::Success, Reason::IdlePrompt, now);
                arm_turn_latch(row);
            }
            // A17: about 6 s after a request nobody answered. Confirms
            // waiting when the PermissionRequest itself was lost.
            Some("permission_prompt")
                if row.lead.state == LeadState::Working && !row.in_flight.is_empty() =>
            {
                let (id, name) = row.in_flight.last().cloned().unwrap_or_default();
                lead_waiting(row, false, Some(id), Some(name), now);
            }
            _ => {}
        },
        "Stop" => {
            // Subagents still running outlived this turn: their end is
            // owed a notification, not final.
            for child in row.children.values_mut() {
                if child.kind == ChildKind::Subagent {
                    child.background = true;
                }
            }
            lead_end(row, Outcome::Success, Reason::TurnDone, now);
            arm_turn_latch(row);
            ends::reconcile_inventory(row, env, true, now);
        }
        "StopFailure" => {
            lead_end(row, Outcome::Failure, Reason::TurnFailed, now);
            arm_turn_latch(row);
        }
        "PostCompact" => {
            if env.source_field.as_deref() == Some("manual") {
                row.children.retain(|_, c| c.origin == ChildOrigin::Hook);
                lead_end(row, Outcome::Boundary, Reason::Compacted, now);
            }
        }
        "SessionEnd" => {
            let reason = match env.source_field.as_deref() {
                Some("clear" | "resume") => Reason::SessionBoundary,
                _ => Reason::AgentExited,
            };
            session_boundary(row, reason, now);
        }
        _ => {}
    }
}

/// A legacy `/hook/complete` bucket (DA13) from a live pane.
pub(crate) fn apply_bucket(row: &mut Row, bucket: &str, source: EvidenceSource, now: i64) {
    row.note_evidence(source, now);
    if source == EvidenceSource::Hook {
        row.had_hook = true;
    }
    apply_bucket_inner(row, bucket, now);
}

fn apply_bucket_inner(row: &mut Row, bucket: &str, now: i64) {
    match bucket {
        "start" => lead_working(row, now),
        "stop" => {
            if row.lead.state != LeadState::Idle {
                lead_end(row, Outcome::Success, Reason::TurnDone, now);
            }
        }
        "permission" => lead_waiting(row, false, None, None, now),
        _ => {}
    }
}

/// A subagent's own envelope (DA20, DA23, DA24).
fn apply_child(row: &mut Row, env: &HookEnvelope, agent_id: &str, now: i64) {
    match env.event.as_str() {
        "SubagentStop" => {
            if let Some(child) = row.children.get_mut(agent_id) {
                if child.background {
                    child.state = ChildState::Owed;
                    child.owed_at = Some(now);
                } else {
                    row.children.remove(agent_id);
                    row.end_reason = Some(Reason::ChildDone);
                }
            }
            // A subagent's inventory may not list the lead's own tasks, so
            // it only adds (see `reconcile_inventory`).
            ends::reconcile_inventory(row, env, false, now);
            return;
        }
        "StopFailure" => {
            if row.children.remove(agent_id).is_some() {
                row.end_reason = Some(Reason::ChildDone);
            }
            return;
        }
        _ => {}
    }
    let lead_idle = row.lead.state == LeadState::Idle;
    if !row.children.contains_key(agent_id) && !ends::upsert_child(row, agent_id, ChildKind::Subagent, ChildOrigin::Hook, lead_idle, now) {
        return;
    }
    let tool = env.tool_name.as_deref().unwrap_or("");
    let Some(child) = row.children.get_mut(agent_id) else { return };
    match env.event.as_str() {
        "PermissionRequest" => child.state = ChildState::Waiting,
        "PreToolUse" if tool == ASK_TOOL => child.state = ChildState::Waiting,
        // The child's wait ends on its next completed tool event; the
        // lead was never touched, so an idle lead stays idle (DA23).
        "PostToolUse" | "PostToolUseFailure" | "PermissionDenied" => child.state = ChildState::Running,
        _ => {
            if child.state == ChildState::Owed {
                child.state = ChildState::Running;
                child.owed_at = None;
            }
        }
    }
}

/// `SessionStart` startup/resume/clear and `SessionEnd`: the lead goes
/// idle and the roster is wiped.
fn session_boundary(row: &mut Row, reason: Reason, now: i64) {
    let outcome = if reason == Reason::SessionBoundary { Outcome::Boundary } else { Outcome::None };
    row.children.clear();
    row.latch = None;
    row.pending_cancel = None;
    lead_end(row, outcome, reason, now);
}

/// DA25: is this lead envelope for a prompt that already ended?
fn latched(row: &mut Row, env: &HookEnvelope, now: i64) -> bool {
    let Some(latch) = row.latch.as_ref() else { return false };
    if latch.until.is_some_and(|until| now >= until) {
        row.latch = None;
        return false;
    }
    match (latch.prompt_id.as_deref(), env.prompt_id.as_deref()) {
        (Some(a), Some(b)) => a == b,
        // Before the first prompt there is no id to latch on.
        (Some(_), None) => false,
        // A21: no `prompt_id` at Stop: "a Stop was seen and no
        // UserPromptSubmit has arrived since".
        (None, _) => true,
    }
}

pub(crate) fn arm_turn_latch(row: &mut Row) {
    row.latch = Some(Latch { prompt_id: row.lead.prompt_id.clone(), until: None });
}

pub(crate) fn arm_cancel_latch(row: &mut Row, now: i64) {
    row.latch = Some(Latch { prompt_id: row.lead.prompt_id.clone(), until: Some(now + CANCEL_LATCH_MS) });
}

fn remember_in_flight(row: &mut Row, id: &str, tool: &str) {
    if row.in_flight.iter().any(|(t, _)| t == id) {
        return;
    }
    if row.in_flight.len() >= IN_FLIGHT_CAP {
        row.in_flight.remove(0);
    }
    row.in_flight.push((id.to_string(), tool.to_string()));
}

pub(crate) fn lead_working(row: &mut Row, now: i64) {
    if row.lead.state == LeadState::Idle && row.turn_started_at.map_or(true, |t| t < row.lead.since) {
        // A turn whose start we didn't see (title, legacy, a restart).
        row.turn_started_at = Some(now);
    }
    if row.lead.state != LeadState::Working {
        row.lead.state = LeadState::Working;
        row.lead.since = now;
        row.lead.outcome = Outcome::None;
    }
    row.lead.waiting = None;
    row.confirmed_working = true;
    row.lead_idle_since = None;
}

pub(crate) fn lead_waiting(row: &mut Row, question: bool, waiting_for: Option<String>, tool_name: Option<String>, now: i64) {
    if row.lead.state != LeadState::Waiting {
        row.lead.since = now;
    }
    row.lead.state = LeadState::Waiting;
    row.lead.outcome = Outcome::None;
    row.lead.waiting = Some(Waiting { question, waiting_for, tool_name });
    row.lead_idle_since = None;
}

/// The lead goes idle with `outcome`. Records a turn end when a turn was
/// running.
pub(crate) fn lead_end(row: &mut Row, outcome: Outcome, reason: Reason, now: i64) {
    let was_busy = row.lead.state != LeadState::Idle;
    row.lead.state = LeadState::Idle;
    row.lead.outcome = outcome;
    row.lead.since = now;
    row.lead.waiting = None;
    row.lead.end_reason = Some(reason);
    row.end_reason = Some(reason);
    row.in_flight.clear();
    row.pending_cancel = None;
    if was_busy || row.lead_idle_since.is_none() {
        row.lead_idle_since = Some(now);
    }
    if was_busy {
        row.pending_turn_end = Some(TurnEnded { outcome, reason, at: now });
    }
}
