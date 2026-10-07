//! The Thread working strip, daemon side
//! (prd-daemon-activity-and-thread-working-v1 S6: TW1–TW9, A22–A26).
//!
//! A **Thread turn** starts on a human Thread inject (the compose bar, an
//! app guest's post, a card answer or dismissal) and follows the v2
//! session the inject was delivered to until the agent is done with it.
//! While it runs, every Member-floor view of that Thread gets ephemeral
//! `activity` overlay frames (§7.6): a pulse, the time since the user's
//! message, the current step ("Running `cargo test`", a thinking pulse),
//! a tool tally, and subagent / background counts. Nothing here is
//! stored: no redb item, no Thread seq, no catalog row (TW5).
//!
//! - **Start (TW1, A23).** `turnId` is the triggering Thread doc's id,
//!   `startedAt` the daemon's ms receipt time of the post. The turn is
//!   `delivering` until `deliver_live_with_via` returns, then it follows
//!   `target_session_id` (the v2 PTY id = the activity row key). A failed
//!   delivery ends it `delivery_failed`.
//! - **Binding (TW2, A22).** Thread delivers mid-turn, and Claude records
//!   that input as a `queued_command` attachment, not a prompt, so most
//!   mid-turn injects never fire `UserPromptSubmit`. A turn binds to the
//!   lead's turn on the first of: an owner `UserPromptSubmit` whose prompt
//!   carries `[thread:<addr>]` (owner envelopes only, so a nested
//!   `claude -p` never binds, DA14-5), or a transcript `queued_command` /
//!   user record carrying the stamp after `startedAt`
//!   ([`TranscriptSignal::thread_addr`]). An **unbound** turn ends `done`
//!   (`unbound_turn_end`) when the session's lead goes idle more than 2 s
//!   after `startedAt`, after a 1.5 s grace in which a stamped prompt
//!   (the queued input submitted as the next prompt) still binds it.
//! - **End (TW3, TW4).** The first of: a Thread reply from the agent
//!   (`reply`); the bound turn's lead idle with display `idle` or
//!   `monitoring` (`done`); the lead's own end reason (`interrupted`,
//!   `failed`, an exit = `session_gone`); the row removed
//!   (`session_gone`); a newer turn on the same conversation
//!   (`superseded`); no evidence for 2× `STALE_AFTER` (`stale`, A24).
//!   After a reply, if the lead is still working 3 s later the strip comes
//!   back with the same `startedAt` (Q7). A turn that ends with no reply
//!   leaves no trace (Q8): its end frame, then nothing.
//! - **After a reply, children (Rosson 2026-10-07).** A reply never hides
//!   live work: if the session still has live children (subagents, and
//!   background shells or monitors) when the agent replies, or when a
//!   replied turn's lead goes idle, the turn stays open in phase
//!   `children` instead of ending. Its frames carry the counts, the tally
//!   so far and the clock from the user's message; state `working` while
//!   a subagent runs, `monitoring` when only background tasks remain; no
//!   Stop (Esc reaches only the lead). It ends `reply` (the end frame,
//!   then nothing, Q8) when the children are all done; a new compose
//!   supersedes it; the lead taking up work again (a task notification)
//!   brings the normal strip back through the Q7 rule.
//! - **Frames (TW5, A25).** On the overlay socket's second broadcast
//!   ([`overlay_ws::publish_activity`]), at most 2 per second per turn
//!   with the last state always sent; an end goes out at once. No 1 Hz
//!   ticks: clients count from `startedAt` / `phaseSince`, corrected by
//!   `serverNow`.
//! - **Phases (TW6).** `delivering` → `working` → `tool` (a lead
//!   `PreToolUse` until its Post; a transcript tool call for hookless
//!   Codex / Grok) → `thinking` (lead working, no tool, 1.5 s since the
//!   last prompt or tool result; or a transcript reasoning record) →
//!   `waiting` (row waiting; `waitingOn` names a permission prompt or a
//!   question) → `stale` (row unverifiable, A24). Thinking is
//!   a timer; no thinking text exists here.
//! - **Catch-up (TW9).** `since_seq` never replays an ephemeral frame, so
//!   `GET /cli/thread/activity?addr=` returns [`current_turn`]: the live
//!   turn's latest frame, or `null`.
//!
//! App guests never see these frames: `overlay_ws::skin_may_see_frame`
//! passes only `collection == "thread"` (AP1), and the catch-up route is
//! refused for app passes.
//!
//! Inputs: owner hook envelopes ([`on_envelope`], called by the activity
//! store after the row has applied the envelope, so the row read here
//! already reflects it), transcript signals
//! ([`crate::activity_transcript::subscribe`], sent after the row applied
//! them), row changes ([`crate::activity_store::subscribe`], a trigger to
//! re-read the row), the overlay routes (start, delivered, reply), and a
//! timer for the deadlines (frame gap, thinking, Q7, grace, stale).
//!
//! The tracker itself ([`Tracker`]) is pure and clock-injected; rows are
//! read through a [`RowSnap`] lookup.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::{json, Value};

use k2_core::activity::row::{ChildState, STALE_AFTER_MS};
use k2_core::activity::{ChildKind, Display, LeadState, Reason, Row, TranscriptSignal};
use k2_core::agent_hooks::envelope::HookEnvelope;
use k2_core::log_debug;

use crate::overlay_ws::{self, OverlayFrame};

/// TW5 / A25: at most 2 frames per second per turn.
pub const FRAME_GAP_MS: i64 = 500;

/// TW6: no tool and no evidence for this long after a prompt or a tool
/// result → `thinking`.
pub const THINKING_AFTER_MS: i64 = 1_500;

/// Q7: after an early reply, the strip comes back if the lead is still
/// working this much later.
pub const RESUME_AFTER_REPLY_MS: i64 = 3_000;

/// A22: an unbound turn ends only on a lead idle this long after
/// `startedAt` (the inject reached the PTY and the agent went idle since).
pub const UNBOUND_MIN_MS: i64 = 2_000;

/// An unbound turn's end waits this long for a stamped prompt: input that
/// arrived too late in a turn to be folded into it is submitted as the
/// next prompt right after the Stop, and binds the turn then.
pub const UNBOUND_GRACE_MS: i64 = 1_500;

/// A24: with no evidence for this long, the turn ends `stale` and the
/// catch-up returns `null`.
pub const STALE_END_MS: i64 = 2 * STALE_AFTER_MS;

/// The session row as the tracker reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowSnap {
    pub lead: LeadState,
    pub lead_since: i64,
    /// Why the lead last went idle.
    pub lead_end_reason: Option<Reason>,
    pub display: Display,
    /// The lead's current (or last) turn start.
    pub turn_started_at: Option<i64>,
    pub evidence_at: Option<i64>,
    pub stale_since: Option<i64>,
    /// The row waits on a question (AskUserQuestion), not a permission
    /// prompt: the strip names which.
    pub waiting_question: bool,
    /// TW7: running or waiting subagents.
    pub subagents: usize,
    /// Subagents that finished after their lead turn and whose
    /// notification the lead hasn't taken yet. Their `SubagentStop` counts
    /// them in `subagentsDone`, so the strip doesn't call them running,
    /// but they still hold a replied turn open.
    pub owed_subagents: usize,
    /// TW7: running background shells and monitors.
    pub background: usize,
}

impl RowSnap {
    /// Subagents or background tasks are still live.
    fn has_children(&self) -> bool {
        self.subagents > 0 || self.owed_subagents > 0 || self.background > 0
    }
}

impl RowSnap {
    pub fn of(row: &Row) -> Self {
        let (subagents, background) = row.thread_counts();
        let owed_subagents = row
            .children
            .values()
            .filter(|c| c.kind == ChildKind::Subagent && c.state == ChildState::Owed)
            .count();
        Self {
            lead: row.lead.state,
            lead_since: row.lead.since,
            lead_end_reason: row.lead.end_reason,
            display: row.display,
            turn_started_at: row.turn_started_at,
            evidence_at: row.evidence_at,
            stale_since: row.stale_since,
            waiting_question: row.reason == Reason::WaitingQuestion,
            subagents: subagents - owed_subagents,
            owed_subagents,
            background,
        }
    }
}

/// How a turn ended (§7.6 `end.reason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    Reply,
    Done,
    Interrupted,
    Failed,
    SessionGone,
    DeliveryFailed,
    Superseded,
    Stale,
}

impl EndReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reply => "reply",
            Self::Done => "done",
            Self::Interrupted => "interrupted",
            Self::Failed => "failed",
            Self::SessionGone => "session_gone",
            Self::DeliveryFailed => "delivery_failed",
            Self::Superseded => "superseded",
            Self::Stale => "stale",
        }
    }

    /// The frame's coarse `state` once the turn has ended: a clean end
    /// leaves the agent `idle` (or `monitoring` its background work);
    /// anything else `stopped`.
    fn final_state(self, display: Option<Display>) -> &'static str {
        match self {
            Self::Reply | Self::Done if display == Some(Display::Monitoring) => "monitoring",
            Self::Reply | Self::Done => "idle",
            _ => "stopped",
        }
    }
}

/// What kind of stamped record bound the turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindKind {
    /// An owner `UserPromptSubmit` (the row already applied it).
    Prompt,
    /// A transcript user record (a new prompt).
    TurnStart,
    /// A transcript `queued_command`: input folded into the running turn.
    Queued,
}

/// The lead turn a Thread turn is bound to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Bound {
    /// The lead turn's `turnStartedAt` at bind time.
    since_turn: Option<i64>,
    /// Bound to the NEXT lead turn: a stamped prompt was seen while the
    /// lead was still idle on the previous one.
    strictly_newer: bool,
}

impl Bound {
    fn of(kind: BindKind, snap: &RowSnap) -> Self {
        let idle = snap.lead == LeadState::Idle;
        Self {
            since_turn: snap.turn_started_at,
            strictly_newer: idle && kind != BindKind::Queued,
        }
    }

    /// The row's lead is on the bound turn or a later one.
    fn reached(&self, snap: &RowSnap) -> bool {
        match (self.since_turn, snap.turn_started_at) {
            (None, Some(_)) => true,
            (None, None) => !self.strictly_newer,
            (Some(_), None) => false,
            (Some(t), Some(s)) if self.strictly_newer => s > t,
            (Some(t), Some(s)) => s >= t,
        }
    }

    fn rank(&self) -> (Option<i64>, bool) {
        (self.since_turn, self.strictly_newer)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Delivering,
    Live,
    /// The agent replied in Thread at `at` (Q7 checks 3 s later).
    AfterReply { at: i64 },
    /// Replied, and children are still live: the strip shows them until
    /// they drain. `at` (the reply, or the lead's idle after it) is what
    /// Q7 counts from.
    Children { at: i64 },
}

/// Tools the turn has called, by the kind its line names.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Tally {
    read: u32,
    search: u32,
    cmd: u32,
    edit: u32,
}

impl Tally {
    /// Count one tool call by its TW8 line (hook and transcript lines
    /// share the vocabulary: Codex / Grok tools map to Claude's).
    fn count(&mut self, line: &str) {
        if line.starts_with("Running ") {
            self.cmd += 1;
        } else if line.starts_with("Reading ") {
            self.read += 1;
        } else if line.starts_with("Searching ") || line.starts_with("Fetching ") {
            self.search += 1;
        } else if line.starts_with("Editing ") {
            self.edit += 1;
        }
    }

    fn to_json(self) -> Value {
        json!({ "read": self.read, "search": self.search, "cmd": self.cmd, "edit": self.edit })
    }
}

/// One session's current step, from its owner hooks (or, for a session
/// with no hooks, its transcript).
#[derive(Debug, Clone, Default)]
struct Steps {
    /// The lead tool in flight: (`tool_use_id`, TW8 line, since).
    tool: Option<(Option<String>, String, i64)>,
    /// The last prompt or tool result: thinking starts 1.5 s after it.
    last_step_at: Option<i64>,
    /// A transcript reasoning record (Codex, Grok) while no tool runs.
    thinking_since: Option<i64>,
    /// Owner hooks reach this session; its transcript steps are ignored.
    hooked: bool,
}

#[derive(Debug, Clone)]
struct Turn {
    id: String,
    conversation_id: String,
    addr: String,
    started_at: i64,
    stage: Stage,
    /// The agent has replied in Thread during this turn.
    replied: bool,
    session: Option<String>,
    bound: Option<Bound>,
    /// Stamped records seen while the turn was still delivering.
    pending_binds: Vec<(String, BindKind)>,
    unbound_end_at: Option<i64>,
    tally: Tally,
    subagents_done: HashSet<String>,
    phase: &'static str,
    phase_since: i64,
    rev: u64,
    last_sent_at: Option<i64>,
    last_body: Option<Value>,
    flush_at: Option<i64>,
    /// The earliest time this turn has something to re-check.
    wake_at: Option<i64>,
}

impl Turn {
    fn new(conversation_id: &str, addr: &str, id: &str, started_at: i64) -> Self {
        Self {
            id: id.to_string(),
            conversation_id: conversation_id.to_string(),
            addr: addr.to_string(),
            started_at,
            stage: Stage::Delivering,
            replied: false,
            session: None,
            bound: None,
            pending_binds: Vec::new(),
            unbound_end_at: None,
            tally: Tally::default(),
            subagents_done: HashSet::new(),
            phase: "delivering",
            phase_since: started_at,
            rev: 0,
            last_sent_at: None,
            last_body: None,
            flush_at: None,
            wake_at: None,
        }
    }

    fn bind(&mut self, kind: BindKind, snap: &RowSnap) {
        let b = Bound::of(kind, snap);
        if self.bound.map_or(true, |old| b.rank() > old.rank()) {
            self.bound = Some(b);
        }
        self.unbound_end_at = None;
    }

    fn wake(&mut self, at: i64) {
        self.wake_at = Some(self.wake_at.map_or(at, |w| w.min(at)));
    }
}

/// One frame to publish.
#[derive(Debug, Clone, PartialEq)]
pub struct Out {
    pub conversation_id: String,
    pub turn_id: String,
    /// The §7.6 `activity` object.
    pub body: Value,
}

/// The row lookup the tracker reads through.
pub type Snaps<'a> = &'a dyn Fn(&str) -> Option<RowSnap>;

/// Every open Thread turn (one per conversation, TW3) and every
/// session's current step.
#[derive(Debug, Default)]
pub struct Tracker {
    turns: HashMap<String, Turn>,
    steps: HashMap<String, Steps>,
}

impl Tracker {
    /// TW1: a human Thread inject is about to be delivered.
    pub fn start(&mut self, conversation_id: &str, addr: &str, turn_id: &str, now: i64) -> Vec<Out> {
        let mut out = Vec::new();
        if let Some(mut old) = self.turns.remove(conversation_id) {
            // TW3: a newer turn on the conversation ends the open one. A
            // turn already ended by a reply just goes.
            if !matches!(old.stage, Stage::AfterReply { .. }) {
                out.push(self.end(&mut old, EndReason::Superseded, None, None, now));
            }
        }
        let mut turn = Turn::new(conversation_id, addr, turn_id, now);
        let body = self.body(&mut turn, None, now);
        send(&mut turn, body, now, &mut out);
        self.turns.insert(conversation_id.to_string(), turn);
        out
    }

    /// TW1: `deliver_live_with_via` returned. `target` is the v2 session
    /// the inject reached, or `None` when delivery failed.
    pub fn delivered(&mut self, conversation_id: &str, turn_id: &str, target: Option<&str>, snaps: Snaps, now: i64) -> Vec<Out> {
        let mut out = Vec::new();
        let Some(turn) = self.turns.get_mut(conversation_id).filter(|t| t.id == turn_id) else {
            return out;
        };
        let Some(sid) = target else {
            let mut turn = self.turns.remove(conversation_id).expect("present");
            out.push(self.end(&mut turn, EndReason::DeliveryFailed, None, None, now));
            return out;
        };
        turn.session = Some(sid.to_string());
        if turn.stage == Stage::Delivering {
            turn.stage = Stage::Live;
        }
        let pending = std::mem::take(&mut turn.pending_binds);
        if let Some(snap) = snaps(sid) {
            for (psid, kind) in pending {
                if psid == sid {
                    turn.bind(kind, &snap);
                }
            }
        }
        self.eval(conversation_id, snaps, now, &mut out);
        out
    }

    /// The addr's pinned conversation changed while the turn was being
    /// delivered (a wake re-pinned the Chat): the turn follows it.
    pub fn moved(&mut self, from: &str, turn_id: &str, to: &str, now: i64) -> Vec<Out> {
        let mut out = Vec::new();
        let Some(mut turn) = self.turns.remove(from) else { return out };
        if turn.id != turn_id {
            self.turns.insert(from.to_string(), turn);
            return out;
        }
        if let Some(mut old) = self.turns.remove(to) {
            if !matches!(old.stage, Stage::AfterReply { .. }) {
                out.push(self.end(&mut old, EndReason::Superseded, None, None, now));
            }
        }
        turn.conversation_id = to.to_string();
        self.turns.insert(to.to_string(), turn);
        out
    }

    /// The whole Thread moved to a new key (Codex/Hermes adoption: pane
    /// key → provider id): its turn, whatever its id, goes with it.
    pub fn conversation_moved(&mut self, from: &str, to: &str, now: i64) -> Vec<Out> {
        let turn_id = match self.turns.get(from) {
            Some(t) => t.id.clone(),
            None => return Vec::new(),
        };
        self.moved(from, &turn_id, to, now)
    }

    /// TW4 (a): the agent posted into the conversation's Thread.
    pub fn reply(&mut self, conversation_id: &str, snaps: Snaps, now: i64) -> Vec<Out> {
        let mut out = Vec::new();
        let Some(mut turn) = self.turns.remove(conversation_id) else { return out };
        turn.replied = true;
        let snap = turn.session.as_deref().and_then(snaps);
        match turn.stage {
            Stage::Delivering | Stage::Live if snap.as_ref().is_some_and(RowSnap::has_children) => {
                // A reply never hides live work: the strip stays on the
                // children until they drain.
                turn.stage = Stage::Children { at: now };
                let body = self.body(&mut turn, snap.as_ref(), now);
                send(&mut turn, body, now, &mut out);
                turn.wake_at = None;
                turn.wake(now + RESUME_AFTER_REPLY_MS);
                self.turns.insert(conversation_id.to_string(), turn);
                return out;
            }
            Stage::Delivering | Stage::Live => {
                out.push(self.end(&mut turn, EndReason::Reply, None, snap.as_ref(), now));
            }
            Stage::Children { .. } => {
                // Another reply: Q7 counts from it; the children stay.
                turn.stage = Stage::Children { at: now };
                self.turns.insert(conversation_id.to_string(), turn);
                self.eval(conversation_id, snaps, now, &mut out);
                return out;
            }
            Stage::AfterReply { .. } => {}
        }
        turn.stage = Stage::AfterReply { at: now };
        turn.wake_at = None;
        turn.wake(now + RESUME_AFTER_REPLY_MS);
        self.turns.insert(conversation_id.to_string(), turn);
        out
    }

    /// An owner hook envelope, after its row applied it.
    pub fn envelope(&mut self, env: &HookEnvelope, at: i64, snaps: Snaps, now: i64) -> Vec<Out> {
        let sid = env.pane.as_str();
        if let Some(agent) = env.agent_id.as_deref() {
            // DA20: a child's envelope is roster only (the row counts it).
            if env.event == "SubagentStop" {
                for turn in self.turns.values_mut().filter(|t| t.session.as_deref() == Some(sid)) {
                    turn.subagents_done.insert(agent.to_string());
                }
            }
            return self.eval_session(sid, snaps, now);
        }
        let steps = self.steps.entry(sid.to_string()).or_default();
        steps.hooked = true;
        let mut counted: Option<String> = None;
        match env.event.as_str() {
            "UserPromptSubmit" => {
                steps.tool = None;
                steps.thinking_since = None;
                steps.last_step_at = Some(at);
            }
            "PreToolUse" => {
                let line = env
                    .tool_line
                    .clone()
                    .or_else(|| env.tool_name.as_ref().map(|n| format!("Using {n}")))
                    .unwrap_or_else(|| "Using a tool".to_string());
                counted = Some(line.clone());
                steps.tool = Some((env.tool_use_id.clone(), line, at));
                steps.thinking_since = None;
            }
            "PostToolUse" | "PostToolUseFailure" => {
                let ends_it = match (&steps.tool, &env.tool_use_id) {
                    (Some((Some(running), _, _)), Some(done)) => running == done,
                    _ => true,
                };
                if ends_it {
                    steps.tool = None;
                }
                steps.last_step_at = Some(at);
            }
            "Stop" | "StopFailure" | "SessionStart" | "SessionEnd" | "PostCompact" => {
                steps.tool = None;
                steps.thinking_since = None;
                steps.last_step_at = None;
            }
            _ => {}
        }
        if let Some(line) = counted {
            self.count_tool(sid, &line);
        }
        if env.event == "UserPromptSubmit" {
            if let Some(addr) = env.prompt_thread_addr.as_deref() {
                self.bind(sid, addr, BindKind::Prompt, at, snaps);
            }
        }
        self.eval_session(sid, snaps, now)
    }

    /// A transcript signal, after its row applied it.
    pub fn transcript(&mut self, sid: &str, signal: &TranscriptSignal, at: i64, snaps: Snaps, now: i64) -> Vec<Out> {
        if let Some(addr) = signal.thread_addr() {
            let kind = match signal {
                TranscriptSignal::Queued { .. } => BindKind::Queued,
                _ => BindKind::TurnStart,
            };
            self.bind(sid, addr, kind, at, snaps);
        }
        let steps = self.steps.entry(sid.to_string()).or_default();
        let mut counted: Option<String> = None;
        if !steps.hooked {
            match signal {
                TranscriptSignal::Tool { line } => {
                    counted = Some(line.clone());
                    steps.tool = Some((None, line.clone(), at));
                    steps.thinking_since = None;
                }
                TranscriptSignal::ToolDone => {
                    steps.tool = None;
                    steps.thinking_since = None;
                    steps.last_step_at = Some(at);
                }
                TranscriptSignal::Thinking => {
                    steps.thinking_since.get_or_insert(at);
                }
                TranscriptSignal::TurnStart { .. } => {
                    steps.tool = None;
                    steps.thinking_since = None;
                    steps.last_step_at = Some(at);
                }
                TranscriptSignal::Queued { .. } => {}
                TranscriptSignal::TurnEnd { .. } | TranscriptSignal::Interrupt => {
                    steps.tool = None;
                    steps.thinking_since = None;
                    steps.last_step_at = None;
                }
            }
        }
        if let Some(line) = counted {
            self.count_tool(sid, &line);
        }
        self.eval_session(sid, snaps, now)
    }

    /// The session's row changed (or went away): re-check its turns.
    pub fn row_changed(&mut self, sid: &str, snaps: Snaps, now: i64) -> Vec<Out> {
        if snaps(sid).is_none() {
            self.steps.remove(sid);
        }
        self.eval_session(sid, snaps, now)
    }

    /// Re-check every turn (the timer, or a lagged input bus).
    pub fn tick(&mut self, snaps: Snaps, now: i64) -> Vec<Out> {
        let mut out = Vec::new();
        let convs: Vec<String> = self.turns.keys().cloned().collect();
        for conv in convs {
            self.eval(&conv, snaps, now, &mut out);
        }
        out
    }

    /// The earliest time [`Tracker::tick`] has something to do.
    pub fn next_deadline(&self) -> Option<i64> {
        self.turns.values().filter_map(|t| t.wake_at).min()
    }

    /// TW9: the conversation's live turn as a frame body (fresh), or
    /// `None` when there is none (ended, or hidden after a reply). A
    /// replied turn still showing its children is live.
    pub fn current(&mut self, conversation_id: &str, snaps: Snaps, now: i64) -> Option<Value> {
        let mut out = Vec::new();
        // An overdue deadline (a stale end, a resume) is settled first.
        self.eval(conversation_id, snaps, now, &mut out);
        let mut turn = self.turns.remove(conversation_id)?;
        let live = !matches!(turn.stage, Stage::AfterReply { .. });
        let snap = turn.session.as_deref().and_then(snaps);
        let body = live.then(|| self.body(&mut turn, snap.as_ref(), now));
        let rev = turn.rev;
        self.turns.insert(conversation_id.to_string(), turn);
        let mut body = body?;
        body["serverNow"] = json!(now);
        body["rev"] = json!(rev);
        Some(body)
    }

    fn count_tool(&mut self, sid: &str, line: &str) {
        for turn in self.turns.values_mut().filter(|t| t.session.as_deref() == Some(sid)) {
            turn.tally.count(line);
        }
    }

    /// TW2 / A22: a stamped record for `addr` from session `sid`.
    fn bind(&mut self, sid: &str, addr: &str, kind: BindKind, at: i64, snaps: Snaps) {
        for turn in self.turns.values_mut() {
            if turn.addr != addr || at < turn.started_at {
                continue;
            }
            match turn.stage {
                Stage::Delivering => {
                    if !turn.pending_binds.iter().any(|(s, k)| s == sid && *k == kind) {
                        turn.pending_binds.push((sid.to_string(), kind));
                    }
                }
                Stage::Live | Stage::AfterReply { .. } | Stage::Children { .. } => {
                    if turn.session.as_deref() != Some(sid) {
                        continue;
                    }
                    if let Some(snap) = snaps(sid) {
                        turn.bind(kind, &snap);
                    }
                }
            }
        }
    }

    fn eval_session(&mut self, sid: &str, snaps: Snaps, now: i64) -> Vec<Out> {
        let mut out = Vec::new();
        let convs: Vec<String> = self
            .turns
            .values()
            .filter(|t| t.session.as_deref() == Some(sid))
            .map(|t| t.conversation_id.clone())
            .collect();
        for conv in convs {
            self.eval(&conv, snaps, now, &mut out);
        }
        out
    }

    /// Re-check one turn against its row: end it, resume it (Q7), or
    /// send its new state.
    fn eval(&mut self, conversation_id: &str, snaps: Snaps, now: i64, out: &mut Vec<Out>) {
        let Some(mut turn) = self.turns.remove(conversation_id) else { return };
        turn.wake_at = None;
        let keep = match turn.stage {
            Stage::Delivering => {
                let body = self.body(&mut turn, None, now);
                maybe_send(&mut turn, body, now, out);
                true
            }
            Stage::AfterReply { at } => self.eval_after_reply(&mut turn, at, snaps, now, out),
            Stage::Children { at } => self.eval_children(&mut turn, at, snaps, now, out),
            Stage::Live => self.eval_live(&mut turn, snaps, now, out),
        };
        if keep {
            self.turns.insert(conversation_id.to_string(), turn);
        }
    }

    /// Q7: 3 s after a reply the strip comes back if the lead is still
    /// working; otherwise the turn is over (its end frame already went).
    fn eval_after_reply(&mut self, turn: &mut Turn, at: i64, snaps: Snaps, now: i64, out: &mut Vec<Out>) -> bool {
        let due = at + RESUME_AFTER_REPLY_MS;
        if now < due {
            turn.wake(due);
            return true;
        }
        let snap = turn.session.as_deref().and_then(snaps);
        let Some(snap) = snap else { return false };
        if snap.lead == LeadState::Working {
            turn.stage = Stage::Live;
            turn.unbound_end_at = None;
            let body = self.body(turn, Some(&snap), now);
            send(turn, body, now, out);
            return self.eval_live(turn, snaps, now, out);
        }
        if snap.has_children() {
            // Work the agent started after its reply is still running.
            turn.stage = Stage::Children { at };
            let body = self.body(turn, Some(&snap), now);
            send(turn, body, now, out);
            return self.eval_children(turn, at, snaps, now, out);
        }
        false
    }

    /// After a reply, the children's strip: back to the normal strip if
    /// the lead takes up work again (Q7), over when the children drain.
    fn eval_children(&mut self, turn: &mut Turn, at: i64, snaps: Snaps, now: i64, out: &mut Vec<Out>) -> bool {
        let Some(sid) = turn.session.clone() else { return false };
        let Some(snap) = snaps(&sid) else {
            out.push(self.end(turn, EndReason::SessionGone, None, None, now));
            return false;
        };
        let due = at + RESUME_AFTER_REPLY_MS;
        if snap.lead == LeadState::Working {
            if now >= due {
                turn.stage = Stage::Live;
                turn.unbound_end_at = None;
                let body = self.body(turn, Some(&snap), now);
                send(turn, body, now, out);
                return self.eval_live(turn, snaps, now, out);
            }
            turn.wake(due);
        }
        if !snap.has_children() {
            // The children drained: the reply's end, then nothing (Q8).
            // Inside the Q7 window the lead may still resume it.
            out.push(self.end(turn, EndReason::Reply, None, Some(&snap), now));
            if now >= due {
                return false;
            }
            turn.stage = Stage::AfterReply { at };
            turn.wake(due);
            return true;
        }
        // A24, as for a live turn.
        let last = snap.evidence_at.unwrap_or(turn.started_at).max(turn.started_at);
        if now >= last + STALE_END_MS {
            out.push(self.end(turn, EndReason::Stale, None, Some(&snap), now));
            return false;
        }
        turn.wake(last + STALE_END_MS);
        let body = self.body(turn, Some(&snap), now);
        maybe_send(turn, body, now, out);
        true
    }

    fn eval_live(&mut self, turn: &mut Turn, snaps: Snaps, now: i64, out: &mut Vec<Out>) -> bool {
        let Some(sid) = turn.session.clone() else { return true };
        let Some(snap) = snaps(&sid) else {
            // TW4 (d): the row is gone.
            out.push(self.end(turn, EndReason::SessionGone, None, None, now));
            return false;
        };
        let reached = match &turn.bound {
            Some(b) => b.reached(&snap),
            None => snap.lead_since >= turn.started_at + UNBOUND_MIN_MS,
        };
        if snap.lead == LeadState::Idle && reached {
            let explicit = match snap.lead_end_reason {
                Some(Reason::Interrupted | Reason::PromptDismissed) => Some(EndReason::Interrupted),
                Some(Reason::TurnFailed) => Some(EndReason::Failed),
                Some(Reason::AgentExited | Reason::PtyExited) => Some(EndReason::SessionGone),
                _ => None,
            };
            if let Some(reason) = explicit {
                out.push(self.end(turn, reason, None, Some(&snap), now));
                return false;
            }
            if turn.replied && snap.has_children() {
                // Q7 brought the strip back and the lead is done again,
                // but its children aren't: show them, not "done".
                turn.stage = Stage::Children { at: now };
                return self.eval_children(turn, now, snaps, now, out);
            }
            if matches!(snap.display, Display::Idle | Display::Monitoring) {
                if turn.bound.is_some() {
                    out.push(self.end(turn, EndReason::Done, None, Some(&snap), now));
                    return false;
                }
                let due = *turn.unbound_end_at.get_or_insert(now + UNBOUND_GRACE_MS);
                if now >= due {
                    let detail = Some(Reason::UnboundTurnEnd.as_str());
                    out.push(self.end(turn, EndReason::Done, detail, Some(&snap), now));
                    return false;
                }
                turn.wake(due);
            } else {
                turn.unbound_end_at = None;
            }
        } else {
            turn.unbound_end_at = None;
        }
        // A24: no evidence for 2× STALE_AFTER ends it; until then a
        // decayed row shows `stale` and the clock keeps running.
        let last = snap.evidence_at.unwrap_or(turn.started_at).max(turn.started_at);
        if now >= last + STALE_END_MS {
            out.push(self.end(turn, EndReason::Stale, None, Some(&snap), now));
            return false;
        }
        turn.wake(last + STALE_END_MS);
        let body = self.body(turn, Some(&snap), now);
        maybe_send(turn, body, now, out);
        true
    }

    /// The turn's end frame (sent at once, never coalesced).
    fn end(&mut self, turn: &mut Turn, reason: EndReason, detail: Option<&str>, snap: Option<&RowSnap>, now: i64) -> Out {
        let mut body = self.body(turn, snap, now);
        body["state"] = json!(reason.final_state(snap.map(|s| s.display)));
        body["end"] = json!({ "reason": reason.as_str(), "detail": detail, "at": now });
        log_debug!("[thread-activity] turn {} on {} ended: {}", turn.id, turn.conversation_id, reason.as_str());
        let mut out = Vec::new();
        send(turn, body, now, &mut out);
        out.pop().expect("send always emits")
    }

    /// The §7.6 body (without `serverNow` / `rev`). Moves the turn's
    /// phase clock and arms the thinking deadline.
    fn body(&self, turn: &mut Turn, snap: Option<&RowSnap>, now: i64) -> Value {
        let steps = turn.session.as_deref().and_then(|s| self.steps.get(s));
        let (state, phase, natural_since, line) = match (turn.stage, snap) {
            (Stage::Delivering, _) => ("working", "delivering", Some(turn.started_at), None),
            (_, None) => ("working", "working", None, None),
            (_, Some(s)) if s.display == Display::Waiting => ("needs-you", "waiting", Some(s.lead_since), None),
            (_, Some(s)) if s.display == Display::Unverifiable => ("unverifiable", "stale", s.stale_since, None),
            (Stage::Children { .. }, Some(s)) => {
                let state = if s.subagents > 0 { "working" } else { "monitoring" };
                (state, "children", None, None)
            }
            (_, Some(s)) => {
                let working = s.lead == LeadState::Working;
                match steps.filter(|_| working) {
                    Some(Steps { tool: Some((_, line, since)), .. }) => {
                        ("working", "tool", Some(*since), Some(line.clone()))
                    }
                    Some(Steps { thinking_since: Some(t), .. }) => ("working", "thinking", Some(*t), None),
                    Some(Steps { last_step_at: Some(t), .. }) => {
                        if now - *t >= THINKING_AFTER_MS {
                            ("working", "thinking", Some(*t), None)
                        } else {
                            turn.wake(*t + THINKING_AFTER_MS);
                            ("working", "working", None, None)
                        }
                    }
                    _ => ("working", "working", None, None),
                }
            }
        };
        let same_phase = turn.phase == phase
            && (phase != "tool" || turn.last_body.as_ref().and_then(|b| b["line"].as_str()) == line.as_deref());
        turn.phase_since = natural_since.unwrap_or(if same_phase { turn.phase_since } else { now });
        turn.phase = phase;
        json!({
            "turnId": turn.id,
            "state": state,
            "phase": phase,
            "phaseSince": turn.phase_since,
            "line": line,
            "startedAt": turn.started_at,
            "since": turn.started_at,
            "subagents": snap.map_or(0, |s| s.subagents),
            "subagentsDone": turn.subagents_done.len(),
            "background": snap.map_or(0, |s| s.background),
            "tally": turn.tally.to_json(),
            "waitingOn": (phase == "waiting").then(|| match snap {
                Some(s) if s.waiting_question => "question",
                _ => "permission",
            }),
            "end": Value::Null,
        })
    }
}

/// Send now (an end, a resume, a turn's first frame).
fn send(turn: &mut Turn, body: Value, now: i64, out: &mut Vec<Out>) {
    turn.rev += 1;
    let mut wire = body.clone();
    wire["serverNow"] = json!(now);
    wire["rev"] = json!(turn.rev);
    out.push(Out { conversation_id: turn.conversation_id.clone(), turn_id: turn.id.clone(), body: wire });
    turn.last_body = Some(body);
    turn.last_sent_at = Some(now);
    turn.flush_at = None;
}

/// A25: send a changed state at most every [`FRAME_GAP_MS`]; a change
/// inside the gap goes out when it closes (the latest state then).
fn maybe_send(turn: &mut Turn, body: Value, now: i64, out: &mut Vec<Out>) {
    if turn.last_body.as_ref() == Some(&body) {
        turn.flush_at = None;
        return;
    }
    match turn.last_sent_at {
        Some(t) if now - t < FRAME_GAP_MS => {
            let at = t + FRAME_GAP_MS;
            turn.flush_at = Some(at);
            turn.wake(at);
        }
        _ => send(turn, body, now, out),
    }
}

// ── daemon wiring ───────────────────────────────────────────────────

fn tracker() -> &'static Mutex<Tracker> {
    static T: OnceLock<Mutex<Tracker>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(Tracker::default()))
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// The live row for `sid`, as the tracker reads it.
pub fn snap_of(sid: &str) -> Option<RowSnap> {
    crate::activity_store::with_row(sid, RowSnap::of)
}

fn wake_cell() -> &'static (std::sync::Mutex<bool>, std::sync::Condvar) {
    static W: OnceLock<(std::sync::Mutex<bool>, std::sync::Condvar)> = OnceLock::new();
    W.get_or_init(|| (std::sync::Mutex::new(false), std::sync::Condvar::new()))
}

fn wake_timer() {
    let (flag, cv) = wake_cell();
    if let Ok(mut woken) = flag.lock() {
        *woken = true;
        cv.notify_one();
    }
}

fn wait_for_wake(timeout: Duration) {
    let (flag, cv) = wake_cell();
    let Ok(guard) = flag.lock() else { return };
    let Ok((mut guard, _)) = cv.wait_timeout_while(guard, timeout, |woken| !*woken) else { return };
    *guard = false;
}

/// Run one input under the tracker lock and publish what it sends. Frames
/// go out under the lock, so their order is the tracker's order.
fn run(f: impl FnOnce(&mut Tracker, i64) -> Vec<Out>) {
    let now = now_ms();
    {
        let mut t = tracker().lock();
        for o in f(&mut t, now) {
            overlay_ws::publish_activity(OverlayFrame {
                collection: "activity".to_string(),
                seq: 0,
                id: o.turn_id,
                doc: None,
                activity: Some(o.body),
                conversation_id: Some(o.conversation_id),
            });
        }
    }
    wake_timer();
}

/// TW1: a human Thread inject (`turn_id` = its doc id) is about to be
/// delivered to `addr`. `started_at` is the post's ms receipt time (A23).
pub fn start_turn(conversation_id: &str, addr: &str, turn_id: &str, started_at: i64) {
    run(|t, _| t.start(conversation_id, addr, turn_id, started_at));
}

/// TW1: delivery returned; `target` is the v2 session it reached.
/// `now_conversation` is the addr's conversation after delivery (a wake
/// may have re-pinned it).
pub fn delivered(conversation_id: &str, turn_id: &str, target: Option<&str>, now_conversation: Option<&str>) {
    run(|t, now| {
        let mut out = Vec::new();
        let conv = match now_conversation.filter(|c| *c != conversation_id) {
            Some(moved) => {
                out.extend(t.moved(conversation_id, turn_id, moved, now));
                moved
            }
            None => conversation_id,
        };
        out.extend(t.delivered(conv, turn_id, target, &snap_of, now));
        out
    });
}

/// The Thread moved to a new conversation key (overlay move listener).
pub fn conversation_moved(from: &str, to: &str) {
    run(|t, now| t.conversation_moved(from, to, now));
}

/// TW4 (a): the agent posted into this conversation's Thread.
pub fn note_reply(conversation_id: &str) {
    run(|t, now| t.reply(conversation_id, &snap_of, now));
}

/// An owner hook envelope the activity store just applied.
pub fn on_envelope(env: &HookEnvelope, at: i64) {
    run(|t, now| t.envelope(env, at, &snap_of, now));
}

/// TW9: the conversation's live turn (§7.6 `activity` object), or `None`.
pub fn current_turn(conversation_id: &str) -> Option<Value> {
    let now = now_ms();
    tracker().lock().current(conversation_id, &snap_of, now)
}

/// Drop every turn (integration tests share the process).
#[allow(dead_code)] // called via the LIB target by integration tests
pub fn clear_for_tests() {
    *tracker().lock() = Tracker::default();
}

/// Start the tracker once per process: the transcript and row consumers
/// and the deadline timer. Plain threads, like the store's.
pub fn spawn() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        use tokio::sync::broadcast::error::RecvError;
        let mut transcripts = crate::activity_transcript::subscribe();
        let t = std::thread::Builder::new().name("thread-activity-transcript".into()).spawn(move || loop {
            match transcripts.blocking_recv() {
                Ok(ev) => run(|t, now| t.transcript(&ev.session_id, &ev.signal, ev.at, &snap_of, now)),
                Err(RecvError::Lagged(n)) => {
                    // A missed stamp leaves a turn unbound: it still ends
                    // when its lead goes idle (A22).
                    log_debug!("[thread-activity] transcript bus lagged {n}");
                    run(|t, now| t.tick(&snap_of, now));
                }
                Err(RecvError::Closed) => return,
            }
        });
        if let Err(e) = t {
            log_debug!("[thread-activity] could not start the transcript consumer: {e}");
        }
        let mut rows = crate::activity_store::subscribe();
        let r = std::thread::Builder::new().name("thread-activity-rows".into()).spawn(move || loop {
            match rows.blocking_recv() {
                Ok(ev) => run(|t, now| t.row_changed(&ev.session_id, &snap_of, now)),
                Err(RecvError::Lagged(n)) => {
                    log_debug!("[thread-activity] row bus lagged {n}; re-checking every turn");
                    run(|t, now| t.tick(&snap_of, now));
                }
                Err(RecvError::Closed) => return,
            }
        });
        if let Err(e) = r {
            log_debug!("[thread-activity] could not start the row consumer: {e}");
        }
        let timer = std::thread::Builder::new().name("thread-activity-timer".into()).spawn(|| loop {
            let now = now_ms();
            let next = tracker().lock().next_deadline();
            let wait = next.map_or(Duration::from_secs(5), |t| Duration::from_millis((t - now).max(1) as u64));
            wait_for_wake(wait);
            if tracker().lock().next_deadline().is_some_and(|t| t <= now_ms()) {
                run(|t, now| t.tick(&snap_of, now));
            }
        });
        if let Err(e) = timer {
            log_debug!("[thread-activity] could not start the timer: {e}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_core::agent_hooks::envelope::{self, HookHeaders, HookSource};
    use std::cell::RefCell;

    const SID: &str = "11111111-1111-4111-8111-111111111111";
    const CONV: &str = "conv-1";
    const ADDR: &str = "sales";

    fn hook(body: &str) -> HookEnvelope {
        envelope::parse(
            &HookHeaders {
                pane: SID.to_string(),
                agent_pid: Some(100),
                source: HookSource::Claude,
                hook_version: Some(2),
                cli_version: None,
                truncated: false,
                event_hint: None,
            },
            body.as_bytes(),
        )
        .expect("parse")
    }

    fn snap(lead: LeadState, display: Display, turn: i64) -> RowSnap {
        RowSnap {
            lead,
            lead_since: turn,
            lead_end_reason: None,
            display,
            turn_started_at: Some(turn),
            evidence_at: Some(turn),
            stale_since: None,
            waiting_question: false,
            subagents: 0,
            owed_subagents: 0,
            background: 0,
        }
    }

    /// A fake row the tests move by hand.
    struct Fake(RefCell<Option<RowSnap>>);

    impl Fake {
        fn new(s: RowSnap) -> Self {
            Self(RefCell::new(Some(s)))
        }
        fn set(&self, s: RowSnap) {
            *self.0.borrow_mut() = Some(s);
        }
        fn gone(&self) {
            *self.0.borrow_mut() = None;
        }
        fn lookup(&self) -> impl Fn(&str) -> Option<RowSnap> + '_ {
            move |sid: &str| if sid == SID { self.0.borrow().clone() } else { None }
        }
    }

    fn only(out: &[Out]) -> &Value {
        assert_eq!(out.len(), 1, "one frame expected: {out:?}");
        &out[0].body
    }

    fn end_of(out: &[Out]) -> (String, Option<String>) {
        let b = &out.last().expect("a frame").body;
        let end = &b["end"];
        assert!(end.is_object(), "an end frame expected: {b}");
        (end["reason"].as_str().expect("reason").to_string(), end["detail"].as_str().map(str::to_string))
    }

    /// Start a turn at 1_000 and deliver it to SID at 1_100.
    fn live(t: &mut Tracker, row: &Fake) {
        let out = t.start(CONV, ADDR, "turn-1", 1_000);
        let b = only(&out);
        assert_eq!(b["phase"], "delivering");
        assert_eq!(b["state"], "working");
        assert_eq!(b["startedAt"], 1_000);
        assert_eq!(b["since"], 1_000);
        assert_eq!(b["end"], Value::Null);
        t.delivered(CONV, "turn-1", Some(SID), &row.lookup(), 1_100);
    }

    /// T-S6a (pure): working → tool → thinking after 1.5 s → reply; a
    /// stamped prompt binds; the tally counts the tool; at most 2 frames/s.
    #[test]
    fn a_compose_turn_runs_tool_thinking_and_ends_on_reply() {
        let mut t = Tracker::default();
        let row = Fake::new(snap(LeadState::Idle, Display::Idle, 500));
        live(&mut t, &row);
        // The prompt carries the stamp: the row is already working on it.
        row.set(snap(LeadState::Working, Display::Working, 1_600));
        let out = t.envelope(
            &hook(r#"{"hook_event_name":"UserPromptSubmit","prompt_id":"p1","prompt":"[from u] [thread:sales] run the tests"}"#),
            1_600,
            &row.lookup(),
            1_600,
        );
        assert_eq!(only(&out)["phase"], "working");
        let out = t.envelope(
            &hook(r#"{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_use_id":"t1","tool_input":{"command":"cargo test"}}"#),
            1_700,
            &row.lookup(),
            1_700,
        );
        assert!(out.is_empty(), "inside the 500 ms gap: coalesced, {out:?}");
        let out = t.tick(&row.lookup(), 2_100);
        let b = only(&out);
        assert_eq!(b["phase"], "tool");
        assert_eq!(b["line"], "Running `cargo test`");
        assert_eq!(b["phaseSince"], 1_700);
        assert_eq!(b["tally"]["cmd"], 1);
        assert_eq!(t.current(CONV, &row.lookup(), 2_200).expect("live turn")["line"], "Running `cargo test`");
        let out = t.envelope(
            &hook(r#"{"hook_event_name":"PostToolUse","tool_name":"Bash","tool_use_id":"t1"}"#),
            3_000,
            &row.lookup(),
            3_000,
        );
        assert_eq!(only(&out)["phase"], "working");
        assert_eq!(t.next_deadline(), Some(4_500), "the thinking deadline is armed");
        let b = only(&t.tick(&row.lookup(), 4_500)).clone();
        assert_eq!(b["phase"], "thinking");
        assert_eq!(b["phaseSince"], 3_000, "thinking counts from the tool result");
        let out = t.reply(CONV, &row.lookup(), 5_000);
        assert_eq!(end_of(&out).0, "reply");
        assert_eq!(only(&out)["state"], "idle");
        assert!(t.current(CONV, &row.lookup(), 5_100).is_none(), "the strip is hidden after a reply");
        // The lead finishes inside the Q7 window: over, with no more frames.
        row.set(snap(LeadState::Idle, Display::Idle, 1_600));
        assert!(t.row_changed(SID, &row.lookup(), 6_000).is_empty());
        assert!(t.tick(&row.lookup(), 8_100).is_empty());
        assert!(t.current(CONV, &row.lookup(), 8_200).is_none());
        assert!(t.turns.is_empty());
    }

    /// Q7: the agent replies early and keeps working: the strip comes back
    /// 3 s later with the same `startedAt`, and ends at the turn's end.
    #[test]
    fn the_strip_comes_back_after_an_early_reply() {
        let mut t = Tracker::default();
        let row = Fake::new(snap(LeadState::Working, Display::Working, 800));
        live(&mut t, &row);
        t.transcript(SID, &TranscriptSignal::Queued { thread_addr: Some(ADDR.into()) }, 1_200, &row.lookup(), 1_200);
        assert_eq!(end_of(&t.reply(CONV, &row.lookup(), 2_000)).0, "reply");
        assert!(t.tick(&row.lookup(), 4_999).is_empty());
        let out = t.tick(&row.lookup(), 5_000);
        let b = only(&out);
        assert_eq!(b["end"], Value::Null, "resumed: {b}");
        assert_eq!(b["startedAt"], 1_000, "same startedAt");
        assert_eq!(t.current(CONV, &row.lookup(), 5_100).expect("live again")["state"], "working");
        // The bound turn (started 800) ends: done.
        row.set(snap(LeadState::Idle, Display::Idle, 800));
        let out = t.row_changed(SID, &row.lookup(), 9_000);
        assert_eq!(end_of(&out), ("done".to_string(), None));
        assert!(t.current(CONV, &row.lookup(), 9_100).is_none());
    }

    /// Q8: a bound turn that ends with no Thread reply sends its end frame
    /// and leaves nothing behind.
    #[test]
    fn a_turn_without_a_reply_leaves_no_trace() {
        let mut t = Tracker::default();
        let row = Fake::new(snap(LeadState::Working, Display::Working, 1_050));
        live(&mut t, &row);
        t.envelope(
            &hook(r#"{"hook_event_name":"UserPromptSubmit","prompt_id":"p","prompt":"[thread:sales] hi"}"#),
            1_050,
            &row.lookup(),
            1_200,
        );
        row.set(RowSnap { lead_since: 1_400, ..snap(LeadState::Idle, Display::Idle, 1_050) });
        let out = t.row_changed(SID, &row.lookup(), 1_400);
        assert_eq!(end_of(&out), ("done".to_string(), None));
        assert_eq!(only(&out)["state"], "idle");
        assert!(t.turns.is_empty());
        assert!(t.tick(&row.lookup(), 10_000).is_empty());
    }

    /// T-S6h: an inject while the lead is mid-tool binds through the
    /// transcript `queued_command` and ends at that turn's Stop, even with
    /// the Stop inside the 2 s unbound floor.
    #[test]
    fn a_queued_inject_ends_at_the_running_turns_stop() {
        let mut t = Tracker::default();
        let row = Fake::new(snap(LeadState::Working, Display::Working, 400));
        live(&mut t, &row);
        t.envelope(
            &hook(r#"{"hook_event_name":"PreToolUse","tool_name":"Read","tool_use_id":"r1","tool_input":{"file_path":"/home/u/ws/src/lib.rs"}}"#),
            1_150,
            &row.lookup(),
            1_150,
        );
        t.transcript(SID, &TranscriptSignal::Queued { thread_addr: Some(ADDR.into()) }, 1_300, &row.lookup(), 1_300);
        row.set(RowSnap { lead_since: 1_500, ..snap(LeadState::Idle, Display::Idle, 400) });
        let out = t.row_changed(SID, &row.lookup(), 1_500);
        assert_eq!(end_of(&out), ("done".to_string(), None));
        assert_eq!(out.last().expect("frame").body["tally"]["read"], 1);
    }

    /// T-S6i: an unbound turn ends `done` / `unbound_turn_end` once the
    /// lead goes idle after `startedAt + 2 s` and the grace passes.
    #[test]
    fn an_unbound_turn_ends_when_the_lead_goes_idle() {
        let mut t = Tracker::default();
        let row = Fake::new(snap(LeadState::Working, Display::Working, 400));
        live(&mut t, &row);
        // Idle inside the 2 s floor: not the inject's turn yet.
        row.set(RowSnap { lead_since: 2_000, ..snap(LeadState::Idle, Display::Idle, 400) });
        assert!(t.row_changed(SID, &row.lookup(), 2_000).iter().all(|o| o.body["end"].is_null()));
        // The agent works again and goes idle after the floor.
        row.set(snap(LeadState::Working, Display::Working, 2_500));
        t.row_changed(SID, &row.lookup(), 2_500);
        row.set(RowSnap { lead_since: 9_000, ..snap(LeadState::Idle, Display::Monitoring, 2_500) });
        assert!(t.row_changed(SID, &row.lookup(), 9_000).iter().all(|o| o.body["end"].is_null()), "grace first");
        assert_eq!(t.next_deadline(), Some(9_000 + UNBOUND_GRACE_MS));
        let out = t.tick(&row.lookup(), 9_000 + UNBOUND_GRACE_MS);
        assert_eq!(end_of(&out), ("done".to_string(), Some("unbound_turn_end".to_string())));
        assert_eq!(only(&out)["state"], "monitoring", "background work keeps running");
    }

    /// The grace lets the queued input, submitted as the next prompt right
    /// after the Stop, bind the turn instead of ending it.
    #[test]
    fn a_stamped_prompt_inside_the_grace_binds_the_turn() {
        let mut t = Tracker::default();
        let row = Fake::new(snap(LeadState::Working, Display::Working, 400));
        live(&mut t, &row);
        row.set(RowSnap { lead_since: 5_000, ..snap(LeadState::Idle, Display::Idle, 400) });
        t.row_changed(SID, &row.lookup(), 5_000);
        row.set(snap(LeadState::Working, Display::Working, 5_200));
        t.envelope(
            &hook(r#"{"hook_event_name":"UserPromptSubmit","prompt_id":"p2","prompt":"[from u] [thread:sales] hi"}"#),
            5_200,
            &row.lookup(),
            5_200,
        );
        let out = t.tick(&row.lookup(), 7_000);
        assert!(out.iter().all(|o| o.body["end"].is_null()), "bound, still working: {out:?}");
        assert!(t.turns.contains_key(CONV));
        row.set(RowSnap { lead_since: 9_000, ..snap(LeadState::Idle, Display::Idle, 5_200) });
        assert_eq!(end_of(&t.row_changed(SID, &row.lookup(), 9_000)), ("done".to_string(), None));
    }

    /// A stamped transcript user record read before its prompt hook binds
    /// the NEXT lead turn, never the finished one.
    #[test]
    fn a_turn_start_seen_on_an_idle_lead_waits_for_the_next_turn() {
        let mut t = Tracker::default();
        let row = Fake::new(snap(LeadState::Idle, Display::Idle, 400));
        live(&mut t, &row);
        t.transcript(SID, &TranscriptSignal::TurnStart { thread_addr: Some(ADDR.into()) }, 1_200, &row.lookup(), 1_200);
        row.set(RowSnap { lead_since: 1_300, ..snap(LeadState::Idle, Display::Idle, 400) });
        assert!(t.row_changed(SID, &row.lookup(), 1_300).iter().all(|o| o.body["end"].is_null()));
        assert!(t.turns.contains_key(CONV));
    }

    /// T-S6c: a failed delivery ends the turn `delivery_failed`.
    #[test]
    fn a_failed_delivery_ends_the_turn() {
        let mut t = Tracker::default();
        let row = Fake::new(snap(LeadState::Idle, Display::Idle, 0));
        t.start(CONV, ADDR, "turn-1", 1_000);
        let out = t.delivered(CONV, "turn-1", None, &row.lookup(), 1_050);
        assert_eq!(end_of(&out).0, "delivery_failed");
        assert_eq!(only(&out)["state"], "stopped");
        assert!(t.turns.is_empty());
    }

    /// A wake that re-pins the Chat moves the turn to the new conversation.
    #[test]
    fn a_turn_follows_a_re_pinned_conversation() {
        let mut t = Tracker::default();
        let row = Fake::new(snap(LeadState::Working, Display::Working, 1_050));
        t.start(CONV, ADDR, "turn-1", 1_000);
        assert!(t.moved(CONV, "turn-1", "conv-2", 1_100).is_empty());
        let out = t.delivered("conv-2", "turn-1", Some(SID), &row.lookup(), 1_600);
        assert_eq!(out[0].conversation_id, "conv-2");
        assert!(t.current(CONV, &row.lookup(), 1_700).is_none());
        assert_eq!(t.current("conv-2", &row.lookup(), 1_700).expect("moved")["turnId"], "turn-1");
        // A stale move for another turn is ignored.
        assert!(t.moved("conv-2", "turn-x", "conv-3", 1_800).is_empty());
        assert!(t.turns.contains_key("conv-2"));
    }

    /// T-S6f: a second compose ends the first turn `superseded`.
    #[test]
    fn a_second_compose_supersedes_the_first() {
        let mut t = Tracker::default();
        let row = Fake::new(snap(LeadState::Working, Display::Working, 900));
        live(&mut t, &row);
        let out = t.start(CONV, ADDR, "turn-2", 2_000);
        assert_eq!(out.len(), 2, "{out:?}");
        assert_eq!(out[0].turn_id, "turn-1");
        assert_eq!(out[0].body["end"]["reason"], "superseded");
        assert_eq!(out[0].body["state"], "stopped");
        assert_eq!(out[1].turn_id, "turn-2");
        assert_eq!(out[1].body["phase"], "delivering");
        // A late delivery report for the old turn is ignored.
        assert!(t.delivered(CONV, "turn-1", Some(SID), &row.lookup(), 2_100).is_empty());
        assert_eq!(t.turns[CONV].id, "turn-2");
    }

    /// TW4 (c), (d): the lead's own end reason ends a bound turn; a
    /// removed row ends it `session_gone`.
    #[test]
    fn interrupts_failures_and_removals_end_the_turn() {
        for (reason, want) in [
            (Reason::Interrupted, "interrupted"),
            (Reason::PromptDismissed, "interrupted"),
            (Reason::TurnFailed, "failed"),
            (Reason::PtyExited, "session_gone"),
        ] {
            let mut t = Tracker::default();
            let row = Fake::new(snap(LeadState::Working, Display::Working, 1_050));
            live(&mut t, &row);
            t.transcript(SID, &TranscriptSignal::Queued { thread_addr: Some(ADDR.into()) }, 1_200, &row.lookup(), 1_200);
            row.set(RowSnap {
                lead_end_reason: Some(reason),
                lead_since: 1_300,
                ..snap(LeadState::Idle, Display::Working, 1_050)
            });
            let out = t.row_changed(SID, &row.lookup(), 1_300);
            assert_eq!(end_of(&out).0, want, "{reason:?}");
            assert_eq!(only(&out)["state"], "stopped");
        }
        let mut t = Tracker::default();
        let row = Fake::new(snap(LeadState::Working, Display::Working, 1_050));
        live(&mut t, &row);
        row.gone();
        assert_eq!(end_of(&t.row_changed(SID, &row.lookup(), 1_300)).0, "session_gone");
    }

    /// TW6 / TW7: waiting and stale phases, subagent and background counts,
    /// subagents done during the turn.
    #[test]
    fn waiting_stale_and_counts() {
        let mut t = Tracker::default();
        let row = Fake::new(snap(LeadState::Working, Display::Working, 900));
        live(&mut t, &row);
        row.set(RowSnap { lead_since: 1_700, ..snap(LeadState::Waiting, Display::Waiting, 900) });
        let b = only(&t.row_changed(SID, &row.lookup(), 1_700)).clone();
        assert_eq!((b["phase"].as_str(), b["state"].as_str()), (Some("waiting"), Some("needs-you")));
        assert_eq!(b["phaseSince"], 1_700);
        assert_eq!(b["waitingOn"], "permission");
        row.set(RowSnap { waiting_question: true, lead_since: 1_800, ..snap(LeadState::Waiting, Display::Waiting, 900) });
        assert_eq!(t.current(CONV, &row.lookup(), 1_800).expect("live")["waitingOn"], "question");
        row.set(RowSnap { subagents: 2, background: 1, ..snap(LeadState::Working, Display::Working, 900) });
        let mut sub = hook(r#"{"hook_event_name":"SubagentStop","agent_id":"a1","agent_type":"general"}"#);
        t.envelope(&sub, 2_300, &row.lookup(), 2_300);
        sub.agent_id = Some("a1".into());
        t.envelope(&sub, 2_301, &row.lookup(), 2_301);
        let b = t.current(CONV, &row.lookup(), 2_400).expect("live");
        assert_eq!((b["subagents"].as_u64(), b["background"].as_u64()), (Some(2), Some(1)));
        assert_eq!(b["subagentsDone"], 1, "one distinct subagent finished: {b}");
        row.set(RowSnap {
            stale_since: Some(900 + STALE_AFTER_MS),
            ..snap(LeadState::Working, Display::Unverifiable, 900)
        });
        let b = t.current(CONV, &row.lookup(), 900 + STALE_AFTER_MS + 5).expect("still open");
        assert_eq!((b["phase"].as_str(), b["state"].as_str()), (Some("stale"), Some("unverifiable")));
        assert_eq!(b["startedAt"], 1_000, "the clock keeps running from the message");
        // A24: 2× STALE_AFTER with no evidence ends it and the catch-up is null.
        assert!(t.current(CONV, &row.lookup(), 1_000 + STALE_END_MS).is_none());
        assert!(t.turns.is_empty());
    }

    /// T-S6g: Codex (no hooks): a transcript tool call sets the line and
    /// counts; `task_complete` with no reply ends the turn `done`.
    #[test]
    fn a_codex_turn_runs_on_its_transcript() {
        let mut t = Tracker::default();
        let row = Fake::new(snap(LeadState::Working, Display::Working, 1_100));
        live(&mut t, &row);
        t.transcript(SID, &TranscriptSignal::Queued { thread_addr: Some(ADDR.into()) }, 1_150, &row.lookup(), 1_150);
        let line = "Running `cargo test`".to_string();
        let out = t.transcript(SID, &TranscriptSignal::Tool { line: line.clone() }, 1_700, &row.lookup(), 1_700);
        let b = only(&out);
        assert_eq!((b["phase"].as_str(), b["line"].as_str()), (Some("tool"), Some(line.as_str())));
        assert_eq!(b["tally"]["cmd"], 1);
        t.transcript(SID, &TranscriptSignal::ToolDone, 2_300, &row.lookup(), 2_300);
        let out = t.transcript(SID, &TranscriptSignal::Thinking, 2_400, &row.lookup(), 2_900);
        assert_eq!(only(&out)["phase"], "thinking");
        assert_eq!(only(&out)["phaseSince"], 2_400);
        row.set(RowSnap { lead_end_reason: Some(Reason::TranscriptTurnEnd), lead_since: 3_000, ..snap(LeadState::Idle, Display::Idle, 1_100) });
        let out = t.transcript(SID, &TranscriptSignal::TurnEnd { record_at: None, cross_check: false }, 3_000, &row.lookup(), 3_000);
        assert_eq!(end_of(&out), ("done".to_string(), None));
    }

    /// With hooks, transcript steps are ignored (the hook is the step).
    #[test]
    fn a_hooked_session_ignores_transcript_steps() {
        let mut t = Tracker::default();
        let row = Fake::new(snap(LeadState::Working, Display::Working, 1_100));
        live(&mut t, &row);
        t.envelope(&hook(r#"{"hook_event_name":"PostToolUse","tool_name":"Bash","tool_use_id":"x"}"#), 1_150, &row.lookup(), 1_150);
        t.transcript(SID, &TranscriptSignal::Tool { line: "Reading `a.rs`".into() }, 1_200, &row.lookup(), 1_800);
        let b = t.current(CONV, &row.lookup(), 1_800).expect("live");
        assert_ne!(b["phase"], "tool", "{b}");
        assert_eq!(b["tally"]["read"], 0);
    }

    /// A stamped record from another session, or for another address,
    /// never binds this turn; a stamp seen while delivering binds on
    /// delivery.
    #[test]
    fn binding_needs_the_same_session_and_address() {
        let mut t = Tracker::default();
        let row = Fake::new(snap(LeadState::Working, Display::Working, 1_050));
        t.start(CONV, ADDR, "turn-1", 1_000);
        t.transcript(SID, &TranscriptSignal::Queued { thread_addr: Some(ADDR.into()) }, 1_020, &row.lookup(), 1_020);
        t.delivered(CONV, "turn-1", Some(SID), &row.lookup(), 1_100);
        assert!(t.turns[CONV].bound.is_some(), "a stamp read while delivering binds on delivery");

        let mut t = Tracker::default();
        live(&mut t, &row);
        t.transcript("other-session", &TranscriptSignal::Queued { thread_addr: Some(ADDR.into()) }, 1_200, &row.lookup(), 1_200);
        t.transcript(SID, &TranscriptSignal::Queued { thread_addr: Some("sales/reviewer".into()) }, 1_200, &row.lookup(), 1_200);
        t.transcript(SID, &TranscriptSignal::Queued { thread_addr: Some(ADDR.into()) }, 900, &row.lookup(), 1_200);
        assert!(t.turns[CONV].bound.is_none(), "wrong session, wrong addr, or older than the turn");
    }

    /// A bound turn whose lead runs with `subagents` / `background` live.
    fn busy(subagents: usize, background: usize) -> RowSnap {
        RowSnap { subagents, background, ..snap(LeadState::Working, Display::Working, 1_050) }
    }

    /// Rosson 2026-10-07: a reply while subagents run keeps the strip on
    /// them (no end, no Stop), and it ends `reply` when they finish.
    #[test]
    fn a_reply_while_subagents_run_keeps_the_strip_until_they_finish() {
        let mut t = Tracker::default();
        let row = Fake::new(busy(2, 1));
        live(&mut t, &row);
        t.transcript(SID, &TranscriptSignal::Queued { thread_addr: Some(ADDR.into()) }, 1_200, &row.lookup(), 1_200);
        t.envelope(
            &hook(r#"{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_use_id":"t1","tool_input":{"command":"cargo test"}}"#),
            1_300,
            &row.lookup(),
            1_900,
        );
        let out = t.reply(CONV, &row.lookup(), 2_000);
        let b = only(&out);
        assert_eq!(b["end"], Value::Null, "the reply does not hide the children: {b}");
        assert_eq!((b["phase"].as_str(), b["state"].as_str()), (Some("children"), Some("working")));
        assert_eq!((b["subagents"].as_u64(), b["background"].as_u64()), (Some(2), Some(1)));
        assert_eq!(b["startedAt"], 1_000, "the clock runs from the user's message");
        assert_eq!(b["tally"]["cmd"], 1, "the tally so far");
        assert_eq!(b["line"], Value::Null);
        // The lead finishes its turn; the children keep the strip.
        row.set(RowSnap { lead_since: 2_100, ..RowSnap { subagents: 2, background: 1, ..snap(LeadState::Idle, Display::Working, 1_050) } });
        assert!(t.row_changed(SID, &row.lookup(), 2_100).iter().all(|o| o.body["end"].is_null()));
        t.envelope(&hook(r#"{"hook_event_name":"SubagentStop","agent_id":"a1","agent_type":"general"}"#), 4_000, &row.lookup(), 4_000);
        row.set(RowSnap { lead_since: 2_100, ..RowSnap { subagents: 1, background: 1, ..snap(LeadState::Idle, Display::Working, 1_050) } });
        t.row_changed(SID, &row.lookup(), 6_000);
        let b = t.current(CONV, &row.lookup(), 6_100).expect("the children keep the turn live");
        assert_eq!((b["phase"].as_str(), b["subagents"].as_u64(), b["subagentsDone"].as_u64()), (Some("children"), Some(1), Some(1)));
        // Only the background task is left: monitoring.
        row.set(RowSnap { lead_since: 2_100, ..RowSnap { background: 1, ..snap(LeadState::Idle, Display::Monitoring, 1_050) } });
        let b = only(&t.row_changed(SID, &row.lookup(), 7_000)).clone();
        assert_eq!((b["phase"].as_str(), b["state"].as_str()), (Some("children"), Some("monitoring")));
        // All done: the reply's end, then nothing.
        row.set(RowSnap { lead_since: 2_100, ..snap(LeadState::Idle, Display::Idle, 1_050) });
        let out = t.row_changed(SID, &row.lookup(), 8_000);
        assert_eq!(end_of(&out), ("reply".to_string(), None));
        assert_eq!(only(&out)["state"], "idle");
        assert!(t.turns.is_empty());
        assert!(t.current(CONV, &row.lookup(), 8_100).is_none());
        assert!(t.tick(&row.lookup(), 20_000).is_empty());
    }

    /// Subagents that finished after the lead's turn but whose notification
    /// the lead hasn't taken yet still hold the strip, as done, not running.
    #[test]
    fn owed_subagents_hold_the_strip_as_done() {
        let mut t = Tracker::default();
        let row = Fake::new(busy(1, 0));
        live(&mut t, &row);
        t.reply(CONV, &row.lookup(), 2_000);
        t.envelope(&hook(r#"{"hook_event_name":"SubagentStop","agent_id":"a1","agent_type":"general"}"#), 2_500, &row.lookup(), 2_500);
        row.set(RowSnap { owed_subagents: 1, lead_since: 2_200, ..snap(LeadState::Idle, Display::Working, 1_050) });
        t.row_changed(SID, &row.lookup(), 2_600);
        let b = t.current(CONV, &row.lookup(), 2_700).expect("held open");
        assert_eq!((b["phase"].as_str(), b["state"].as_str()), (Some("children"), Some("monitoring")), "{b}");
        assert_eq!((b["subagents"].as_u64(), b["subagentsDone"].as_u64()), (Some(0), Some(1)));
        // The lead takes the notification (owed cleared) and works on it.
        row.set(RowSnap { lead_since: 6_000, ..snap(LeadState::Working, Display::Working, 6_000) });
        let b = only(&t.row_changed(SID, &row.lookup(), 6_000)).clone();
        assert_eq!((b["phase"].as_str(), b["end"].is_null()), (Some("working"), true), "{b}");
    }

    /// A reply with only a background task left reads `monitoring`.
    #[test]
    fn a_reply_with_only_a_background_task_is_monitoring() {
        let mut t = Tracker::default();
        let row = Fake::new(busy(0, 1));
        live(&mut t, &row);
        let b = only(&t.reply(CONV, &row.lookup(), 2_000)).clone();
        assert_eq!((b["phase"].as_str(), b["state"].as_str(), b["end"].is_null()), (Some("children"), Some("monitoring"), true));
    }

    /// A new compose supersedes the children's strip.
    #[test]
    fn a_new_compose_supersedes_the_childrens_strip() {
        let mut t = Tracker::default();
        let row = Fake::new(busy(1, 0));
        live(&mut t, &row);
        t.reply(CONV, &row.lookup(), 2_000);
        let out = t.start(CONV, ADDR, "turn-2", 3_000);
        assert_eq!(out.len(), 2, "{out:?}");
        assert_eq!((out[0].turn_id.as_str(), out[0].body["end"]["reason"].as_str()), ("turn-1", Some("superseded")));
        assert_eq!((out[1].turn_id.as_str(), out[1].body["phase"].as_str()), ("turn-2", Some("delivering")));
    }

    /// The lead takes up work again (a task notification): after the Q7
    /// window the normal strip is back; when the lead is done again with a
    /// child still live, the children's strip; then the reply's end.
    #[test]
    fn the_lead_resuming_brings_the_working_strip_back() {
        let mut t = Tracker::default();
        let row = Fake::new(busy(1, 0));
        live(&mut t, &row);
        t.transcript(SID, &TranscriptSignal::Queued { thread_addr: Some(ADDR.into()) }, 1_200, &row.lookup(), 1_200);
        t.reply(CONV, &row.lookup(), 2_000);
        // Still working inside the window: the children's strip stays.
        assert!(t.tick(&row.lookup(), 4_000).iter().all(|o| o.body["phase"] == "children"));
        let out = t.tick(&row.lookup(), 5_000);
        let b = only(&out);
        assert_eq!((b["phase"].as_str(), b["end"].is_null()), (Some("working"), true), "{b}");
        row.set(RowSnap { lead_since: 9_000, ..RowSnap { subagents: 1, ..snap(LeadState::Idle, Display::Working, 1_050) } });
        let b = only(&t.row_changed(SID, &row.lookup(), 9_000)).clone();
        assert_eq!((b["phase"].as_str(), b["end"].is_null()), (Some("children"), true));
        row.set(RowSnap { lead_since: 9_000, ..snap(LeadState::Idle, Display::Idle, 1_050) });
        assert_eq!(end_of(&t.row_changed(SID, &row.lookup(), 20_000)), ("reply".to_string(), None));
        assert!(t.turns.is_empty());
    }

    /// Children that drain inside the Q7 window while the lead still works
    /// end the strip, and Q7 can still bring it back.
    #[test]
    fn children_draining_inside_the_window_leave_q7_in_place() {
        let mut t = Tracker::default();
        let row = Fake::new(busy(1, 0));
        live(&mut t, &row);
        t.reply(CONV, &row.lookup(), 2_000);
        row.set(busy(0, 0));
        assert_eq!(end_of(&t.row_changed(SID, &row.lookup(), 3_000)).0, "reply");
        assert!(t.current(CONV, &row.lookup(), 3_100).is_none(), "hidden after the end");
        let b = only(&t.tick(&row.lookup(), 5_000)).clone();
        assert_eq!((b["phase"].as_str(), b["end"].is_null()), (Some("working"), true), "Q7: {b}");
    }

    /// Work started after the reply (none live at the reply) still shows
    /// once the Q7 window closes with the lead idle.
    #[test]
    fn children_started_after_the_reply_show_after_the_window() {
        let mut t = Tracker::default();
        let row = Fake::new(busy(0, 0));
        live(&mut t, &row);
        assert_eq!(end_of(&t.reply(CONV, &row.lookup(), 2_000)).0, "reply");
        row.set(RowSnap { lead_since: 3_000, ..RowSnap { background: 1, ..snap(LeadState::Idle, Display::Monitoring, 1_050) } });
        assert!(t.row_changed(SID, &row.lookup(), 3_000).is_empty());
        let b = only(&t.tick(&row.lookup(), 5_000)).clone();
        assert_eq!((b["phase"].as_str(), b["state"].as_str()), (Some("children"), Some("monitoring")));
    }

    #[test]
    fn tally_kinds_follow_the_tool_line() {
        let mut tally = Tally::default();
        for line in ["Running `ls`", "Reading `a.rs`", "Searching `fn x`", "Fetching example.com", "Editing `b.rs`", "Using Foo", "Searching the web"] {
            tally.count(line);
        }
        assert_eq!(tally, Tally { read: 1, search: 3, cmd: 1, edit: 1 });
    }
}
