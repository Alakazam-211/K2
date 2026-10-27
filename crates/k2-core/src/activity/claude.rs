//! Claude hook envelopes → lead state and roster (§6.1, DA20–DA25).
//!
//! Only owner envelopes, and envelopes from a verified OS-process
//! descendant of the owner (DA14-4a, `from_descendant`), get here (DA14
//! ran in the ingest). Child events (`agent_id` set) touch the roster only
//! (DA20). Custom-program envelopes (`k2 hooks emit`, GL45) use the same
//! Claude event names and rules. Cursor and Gemini envelopes, and legacy
//! `/hook/complete` posts, map through the old three buckets
//! ([`apply_bucket`]).
//!
//! **Descendants (DA14-4a).** A descendant is merged into the pane's
//! turn: its tools, subagents and roster count, and it drives the lead
//! like the owner does, because for a Claude Code background job (the
//! pane's `claude` hands the conversation to a `bg-spare` process) it IS
//! the agent doing the work. Two limits keep a nested run (a `claude -p`
//! started by a Bash tool) from ending the pane's turn:
//! - it is never a session boundary: its `SessionStart` / `SessionEnd` /
//!   `PostCompact` are evidence only;
//! - while another process has a tool call open (the Bash running it), its
//!   `UserPromptSubmit` is evidence only, and so is any process's `Stop` /
//!   `StopFailure` / idle `Notification` (the owner's too: a front
//!   `claude` stopping while its background job is mid-tool). The last
//!   process to finish ends the turn ([`nested_in_open_tool`]). A tool
//!   whose Post never comes is cleared by the next owner prompt, the
//!   Claude transcript `end_turn` cross-check, or a process exit.
//!
//! Tool calls and children carry their process ([`ends::sender_key`]):
//! `None` for the owner, so an owner-only pane is unchanged.

use crate::agent_hooks::envelope::{HookEnvelope, HookSource};
use crate::agent_hooks::map_event_type;

use super::{ends, tools};
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
    // A custom program (`k2 hooks emit`, GL45) speaks Claude's event
    // names by contract, so it gets Claude's lead semantics.
    if !matches!(env.source, HookSource::Claude | HookSource::Custom) {
        // Cursor / Gemini: today's three buckets (their own event names).
        if let Some(bucket) = map_event_type(&env.event) {
            // A descendant (DA14-4a) adds work; only the owner's own
            // stop bucket ends the lead.
            if !(env.from_descendant && bucket == "stop") {
                apply_bucket_inner(row, bucket, now);
            }
        }
        // After the bucket, so a turn the bucket started counts this call.
        if let Some(command) = tools::foreign_hook_tool(env.source, &env.event, env.tool_name.as_deref()) {
            row.note_tool(env.tool_use_id.as_deref(), command);
        }
        return;
    }
    if let Some(agent_id) = env.agent_id.as_deref() {
        apply_child(row, env, agent_id, now);
        return;
    }
    let other_tool_open = nested_in_open_tool(row, env);
    match env.event.as_str() {
        // A descendant is never the pane's boundary: the owner's session is.
        "SessionStart" | "SessionEnd" | "PostCompact" if env.from_descendant => return,
        // A descendant's prompt inside another process's tool call is a
        // nested run, not the pane's next turn.
        "UserPromptSubmit" if env.from_descendant && other_tool_open => return,
        // One process stopping while another still has a tool call open
        // (the owner's Bash running a nested `claude -p`, or a background
        // job mid-tool when the pane's front `claude` stops) does not end
        // the pane's turn. The last process to finish ends it.
        "Stop" | "StopFailure" if other_tool_open => return,
        "Notification"
            if other_tool_open
                && (env.from_descendant || env.notification_type.as_deref() == Some("idle_prompt")) =>
        {
            return
        }
        _ => {}
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
            row.start_turn(now);
            lead_working(row, now);
        }
        "PreToolUse" => {
            if latched(row, env, now) {
                return;
            }
            if let Some(id) = env.tool_use_id.as_deref() {
                remember_in_flight(row, id, tool, ends::sender_key(env));
            }
            if row.lead.state == LeadState::Waiting {
                // A17: never clears waiting; a late async PreToolUse binds
                // the pending `waitingFor` instead.
                if let Some(w) = row.lead.waiting.as_mut() {
                    if w.waiting_for.is_none() && w.tool_name.as_deref() == Some(tool) {
                        w.waiting_for = env.tool_use_id.clone();
                    }
                }
                count_tool(row, env, tool, false);
                return;
            }
            if tool == ASK_TOOL {
                lead_waiting(row, true, env.tool_use_id.clone(), Some(tool.to_string()), now);
            } else {
                lead_working(row, now);
            }
            // After `lead_working`: a turn it started counts this call.
            count_tool(row, env, tool, false);
        }
        "PostToolUse" | "PostToolUseFailure" => {
            let id = env.tool_use_id.as_deref();
            if let Some(id) = id {
                row.in_flight.retain(|(t, _, _)| t != id);
            }
            if let Some(task) = env.launched_task.as_ref() {
                ends::add_launched(row, task, ends::sender_key(env), now);
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
            // A Post whose (async) Pre was lost still counts its call;
            // only with an id, so a Pre + Post pair counts once.
            if row.lead.state == LeadState::Waiting {
                let waited = row.lead.waiting.as_ref().and_then(|w| w.waiting_for.as_deref());
                if waited.is_some() && waited == id {
                    lead_working(row, now);
                }
                count_tool(row, env, tool, true);
                return;
            }
            lead_working(row, now);
            count_tool(row, env, tool, true);
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
                .find(|(_, name, _)| name == tool)
                .map(|(id, _, _)| id.clone());
            lead_waiting(row, false, waiting_for, Some(tool.to_string()), now);
        }
        "PermissionDenied" => {
            let id = env.tool_use_id.as_deref();
            if let Some(id) = id {
                row.in_flight.retain(|(t, _, _)| t != id);
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
                let (id, name, _) = row.in_flight.last().cloned().unwrap_or_default();
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
            row.tombstone_child(agent_id, now);
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
            row.tombstone_child(agent_id, now);
            if row.children.remove(agent_id).is_some() {
                row.end_reason = Some(Reason::ChildDone);
            }
            return;
        }
        _ => {}
    }
    // A late async event (`SubagentStart` / `PreToolUse` / `PostToolUse`
    // run `async: true`) from a subagent that already stopped: evidence
    // only, never a running (or un-owed) subagent again.
    if row.child_stopped_recently(agent_id, now) {
        return;
    }
    let lead_idle = row.lead.state == LeadState::Idle;
    let label = env.agent_type.as_deref().and_then(crate::agent_hooks::tool_line::short_label);
    if !row.children.contains_key(agent_id)
        && !ends::upsert_child(row, agent_id, ChildKind::Subagent, ChildOrigin::Hook, lead_idle, ends::sender_key(env), label.clone(), now)
    {
        return;
    }
    if let Some(child) = row.children.get_mut(agent_id) {
        if child.label.is_none() {
            child.label = label;
        }
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

/// Count one lead tool call from a Claude hook (subagent envelopes never
/// get here: [`apply_child`]). `require_id`: a Post event counts only
/// with a `tool_use_id`, the dedupe key its Pre was counted under.
fn count_tool(row: &mut Row, env: &HookEnvelope, tool: &str, require_id: bool) {
    let id = env.tool_use_id.as_deref();
    if tool.is_empty() && id.is_none() {
        return;
    }
    if require_id && id.is_none() {
        return;
    }
    row.note_tool(id, tools::is_command_tool(tool));
}

/// DA14-4a: a descendant's envelope arriving while another process has a
/// lead tool call open is a nested run inside that call (a `claude -p` in
/// a Bash tool): it may add tools and children, never start or end the
/// pane's turn.
fn nested_in_open_tool(row: &Row, env: &HookEnvelope) -> bool {
    let me = ends::sender_key(env);
    row.in_flight.iter().any(|(_, _, sender)| *sender != me)
}

fn remember_in_flight(row: &mut Row, id: &str, tool: &str, sender: Option<i32>) {
    if row.in_flight.iter().any(|(t, _, _)| t == id) {
        return;
    }
    if row.in_flight.len() >= IN_FLIGHT_CAP {
        row.in_flight.remove(0);
    }
    row.in_flight.push((id.to_string(), tool.to_string(), sender));
}

pub(crate) fn lead_working(row: &mut Row, now: i64) {
    if row.lead.state == LeadState::Idle && row.turn_started_at.map_or(true, |t| t < row.lead.since) {
        // A turn whose start we didn't see (title, legacy, a restart).
        row.start_turn(now);
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

/// Phantom children (0.46.4 field bug, Baden / appa: the Thread said the
/// agent was working after its turn ended) and the children's labels.
#[cfg(test)]
mod phantom_tests {
    use super::super::row::{Display, Row, RowFacts, CHILD_TOMBSTONE_MS};
    use super::super::Evidence;
    use crate::agent_hooks::envelope::{self, HookHeaders, HookSource};

    fn row() -> Row {
        Row::new(RowFacts { session_id: "s".into(), harness: "claude".into(), ..Default::default() }, 0)
    }

    fn hook(r: &mut Row, body: &str, at: i64) {
        let env = envelope::parse(
            &HookHeaders {
                pane: "s".into(),
                agent_pid: Some(100),
                source: HookSource::Claude,
                hook_version: Some(2),
                cli_version: Some("2.1.311".into()),
                truncated: false,
                event_hint: None,
            },
            body.as_bytes(),
        )
        .expect("synthetic hook parses");
        r.apply(Evidence::Hook(&env), at);
    }

    /// A foreground subagent, the way Claude Code 2.1.311 reports it.
    /// `SubagentStart` / `PreToolUse` / `PostToolUse` are installed
    /// `async: true`, so the subagent's last ones can land after its
    /// `SubagentStop` and even after the lead's `Stop`; and the
    /// `SubagentStop` inventory, built from the task registry while the
    /// hook runs, still lists the stopping subagent as `running` (every
    /// subagent is registered `isBackgrounded: true`). Each of those used
    /// to put a running subagent back on the roster: the row (and the
    /// Thread turn following it) stayed `working` after the turn ended,
    /// until a later Stop.
    #[test]
    fn a_stopped_subagent_never_comes_back_from_late_events_or_its_own_inventory() {
        let mut r = row();
        hook(&mut r, r#"{"hook_event_name":"UserPromptSubmit","prompt_id":"p1","prompt":"go"}"#, 100);
        hook(&mut r, r#"{"hook_event_name":"PreToolUse","prompt_id":"p1","tool_name":"Agent","tool_use_id":"tu-1","tool_input":{"subagent_type":"Explore"}}"#, 200);
        hook(&mut r, r#"{"hook_event_name":"SubagentStart","agent_id":"a-1","agent_type":"Explore"}"#, 300);
        hook(&mut r, r#"{"hook_event_name":"PreToolUse","agent_id":"a-1","agent_type":"Explore","tool_name":"Read","tool_use_id":"tu-a1","tool_input":{"file_path":"x"}}"#, 400);
        assert_eq!(r.counts().subagents, 1);
        let named = r.thread_children();
        assert_eq!((named[0].kind, named[0].state, named[0].label.as_deref()), ("subagent", "running", Some("Explore")));
        hook(
            &mut r,
            r#"{"hook_event_name":"SubagentStop","agent_id":"a-1","agent_type":"Explore","stop_hook_active":false,
                "background_tasks":[{"id":"a-1","type":"subagent","status":"running","description":"d","agent_type":"Explore"}],
                "session_crons":[]}"#,
            800,
        );
        assert_eq!(r.counts().subagents, 0, "its own inventory never re-adds the stopping subagent: {:?}", r.children);
        hook(&mut r, r#"{"hook_event_name":"PostToolUse","prompt_id":"p1","tool_name":"Agent","tool_use_id":"tu-1","tool_input":{},"tool_response":{}}"#, 900);
        hook(&mut r, r#"{"hook_event_name":"Stop","prompt_id":"p1","stop_hook_active":false,"background_tasks":[],"session_crons":[]}"#, 1_000);
        assert_eq!(r.display, Display::Idle);
        // The subagent's late async hooks land after the lead's Stop.
        hook(&mut r, r#"{"hook_event_name":"PostToolUse","agent_id":"a-1","agent_type":"Explore","tool_name":"Read","tool_use_id":"tu-a1","tool_input":{},"tool_response":{}}"#, 1_200);
        hook(&mut r, r#"{"hook_event_name":"SubagentStart","agent_id":"a-1","agent_type":"Explore"}"#, 1_300);
        assert_eq!(r.counts().subagents, 0, "late events are evidence only: {:?}", r.children);
        assert_eq!(r.display, Display::Idle, "the turn has ended; nothing is still working");
        assert!(r.thread_children().is_empty());
        // A subagent continued after the window shows again.
        hook(&mut r, r#"{"hook_event_name":"SubagentStart","agent_id":"a-1","agent_type":"Explore"}"#, 800 + CHILD_TOMBSTONE_MS);
        assert_eq!(r.counts().subagents, 1);
    }

    /// The Thread strip names each child: a subagent by its type, a
    /// background shell by its redacted command line, an inventory task
    /// by its type.
    #[test]
    fn children_carry_short_labels() {
        let mut r = row();
        hook(&mut r, r#"{"hook_event_name":"UserPromptSubmit","prompt_id":"p1","prompt":"go"}"#, 100);
        hook(
            &mut r,
            r#"{"hook_event_name":"PostToolUse","prompt_id":"p1","tool_name":"Bash","tool_use_id":"tu-1",
                "tool_input":{"command":"npm run dev","run_in_background":true},"tool_response":{"backgroundTaskId":"b-1"}}"#,
            200,
        );
        hook(&mut r, r#"{"hook_event_name":"SubagentStart","agent_id":"a-2","agent_type":"code-reviewer"}"#, 300);
        hook(
            &mut r,
            r#"{"hook_event_name":"Stop","prompt_id":"p1","stop_hook_active":false,
                "background_tasks":[
                    {"id":"b-1","type":"shell","status":"running","command":"npm run dev","description":"d"},
                    {"id":"a-2","type":"subagent","status":"running","agent_type":"code-reviewer","description":"d"},
                    {"id":"m-1","type":"monitor","status":"running","description":"d"}
                ],"session_crons":[]}"#,
            400,
        );
        let kids = r.thread_children();
        let named: Vec<(&str, &str, Option<&str>)> = kids.iter().map(|c| (c.kind, c.state, c.label.as_deref())).collect();
        assert_eq!(
            named,
            vec![
                ("shell", "running", Some("Running `npm run dev`")),
                ("subagent", "running", Some("code-reviewer")),
                ("monitor", "running", Some("monitor")),
            ]
        );
    }
}
