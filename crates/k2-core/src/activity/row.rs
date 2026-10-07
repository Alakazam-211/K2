//! The activity row (DA19, §7.2), its vocabulary (DA31), and the one
//! entry point every input goes through ([`Row::apply`] / [`Row::tick`]).

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::{claude, ends, fold, lock, Evidence};

/// DA29 / Q3: a non-idle row with no evidence for this long shows
/// `unverifiable` (PTY alive). Never `idle`, never done.
pub const STALE_AFTER_MS: i64 = 30 * 60 * 1000;

/// §5.4: an owed task notification holds the row this long after
/// `max(owedAt, leadIdleSince)` (Claude's own `idle_prompt` threshold).
pub const OWED_LEASE_MS: i64 = 60_000;

/// DA25 / DA26: a cancel latch blocks late tool events this long.
pub const CANCEL_LATCH_MS: i64 = 15_000;

/// DA26: Ctrl-C (no transcript) and Esc-on-waiting settle window.
pub const KEY_SETTLE_MS: i64 = 500;

/// A16: a keystroke cancel must be confirmed within this window.
pub const KEY_CONFIRM_WINDOW_MS: i64 = 3_000;

/// DA27: Claude's transcript `end_turn` ends the lead only after this long
/// with no hook (the Stop hook normally lands first).
pub const TRANSCRIPT_END_GRACE_MS: i64 = 5_000;

/// DA24: the roster never grows past this.
pub const ROSTER_CAP: usize = 64;

/// Lead tools remembered in flight (for `waitingFor` binding).
pub(crate) const IN_FLIGHT_CAP: usize = 32;

/// The lead agent's own state (DA19). Only lead envelopes write it (DA20).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeadState {
    Idle,
    Working,
    Waiting,
}

impl LeadState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Waiting => "waiting",
        }
    }
}

/// How the lead's last turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    None,
    Success,
    Failure,
    Cancelled,
    Boundary,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Cancelled => "cancelled",
            Self::Boundary => "boundary",
        }
    }
}

/// A roster entry's kind (DA24).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildKind {
    Subagent,
    Shell,
    Monitor,
    Cron,
    /// An inventory entry with an unknown type or status: fails active.
    Unknown,
}

/// A roster entry's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildState {
    Running,
    Waiting,
    /// Finished, but its `<task-notification>` hasn't reached the lead.
    Owed,
}

/// Where a roster entry came from: decides which ends apply to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildOrigin {
    /// `SubagentStart` / an envelope carrying its `agent_id`.
    Hook,
    /// A `background_tasks` inventory entry.
    Inventory,
    /// A background launch read from a lead `PostToolUse`.
    Launch,
    /// Non-empty `session_crons`.
    Cron,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Child {
    pub kind: ChildKind,
    pub state: ChildState,
    pub origin: ChildOrigin,
    pub since: i64,
    pub owed_at: Option<i64>,
    /// Outlived a lead turn or launched in the background: its end is
    /// owed a notification rather than final.
    pub background: bool,
}

/// What the row folds to (DA21 + DA29).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Display {
    Working,
    Monitoring,
    Waiting,
    Idle,
    Unverifiable,
}

impl Display {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Monitoring => "monitoring",
            Self::Waiting => "waiting",
            Self::Idle => "idle",
            Self::Unverifiable => "unverifiable",
        }
    }

    pub fn is_live(self) -> bool {
        matches!(self, Self::Working | Self::Monitoring | Self::Waiting)
    }
}

/// Where the last evidence came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceSource {
    Hook,
    Transcript,
    Title,
    /// The interrupt-marker grid scan (A13).
    Screen,
    Process,
}

impl EvidenceSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hook => "hook",
            Self::Transcript => "transcript",
            Self::Title => "title",
            Self::Screen => "screen",
            Self::Process => "process",
        }
    }
}

/// The title observer's three words (DA28).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleSignal {
    Working,
    Idle,
    Permission,
}

macro_rules! reasons {
    ($($variant:ident => $s:literal,)*) => {
        /// DA31: the fixed reason vocabulary. A new value needs a PRD
        /// amendment; `fixtures/reasons.json` is the renderer's copy list.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Reason { $($variant,)* }

        impl Reason {
            pub const ALL: &'static [Reason] = &[$(Reason::$variant,)*];

            pub fn as_str(self) -> &'static str {
                match self { $(Reason::$variant => $s,)* }
            }
        }
    };
}

reasons! {
    TurnRunning => "turn_running",
    ToolRunning => "tool_running",
    Thinking => "thinking",
    WaitingPermission => "waiting_permission",
    WaitingQuestion => "waiting_question",
    SubagentsRunning => "subagents_running",
    BackgroundShell => "background_shell",
    MonitorRunning => "monitor_running",
    CronsScheduled => "crons_scheduled",
    OwedNotification => "owed_notification",
    TurnDone => "turn_done",
    TurnFailed => "turn_failed",
    Interrupted => "interrupted",
    PromptDismissed => "prompt_dismissed",
    PermissionDone => "permission_done",
    Compacted => "compacted",
    SessionBoundary => "session_boundary",
    IdlePrompt => "idle_prompt",
    TranscriptTurnEnd => "transcript_turn_end",
    AgentExited => "agent_exited",
    PtyExited => "pty_exited",
    OwedExpired => "owed_expired",
    CronsCleared => "crons_cleared",
    ChildDone => "child_done",
    StaleNoEvidence => "stale_no_evidence",
    Unconfirmed => "unconfirmed",
    Stale => "stale",
    UnboundTurnEnd => "unbound_turn_end",
}

/// What the lead is waiting on (DA23).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Waiting {
    pub question: bool,
    /// The waited call's `tool_use_id`. `None` until a late async
    /// `PreToolUse` with the same tool name binds it (A17).
    pub waiting_for: Option<String>,
    pub tool_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lead {
    pub state: LeadState,
    pub outcome: Outcome,
    pub since: i64,
    pub prompt_id: Option<String>,
    pub waiting: Option<Waiting>,
    /// Why the lead last went idle (drives the lock release, DA32).
    pub end_reason: Option<Reason>,
}

/// DA25: late lead envelopes for a finished prompt are evidence only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Latch {
    pub prompt_id: Option<String>,
    /// A cancel latch lets later events through after this (15 s).
    pub until: Option<i64>,
}

/// A keystroke inference window (DA26, A16).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingCancel {
    pub dismiss: bool,
    pub evidence_seq: u64,
    pub settle_at: Option<i64>,
    pub expires_at: i64,
}

/// A turn end, for toasts (S5) and the Thread tracker (S6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TurnEnded {
    pub outcome: Outcome,
    pub reason: Reason,
    pub at: i64,
}

/// Facts the daemon knows about a session when it registers.
#[derive(Debug, Clone, Default)]
pub struct RowFacts {
    pub session_id: String,
    pub agent_name: String,
    pub project_id: Option<String>,
    pub workspace_path: Option<String>,
    /// `claude` | `codex` | `grok` | … | `shell` (from the spawn command).
    pub harness: String,
}

/// What one input did, for the store's side effects.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Change {
    /// Something a client draws changed (display, reason, lead, children
    /// counts, staleness). `rev` was bumped.
    pub changed: bool,
    pub turn_ended: Option<TurnEnded>,
    /// DA32: write this to `workspace_sessions.status` (lock release or
    /// re-claim on evidence). Never on an unconfirmed row.
    pub status_write: Option<&'static str>,
    /// RL5: the legacy lifecycle bucket to emit (`start` | `stop` |
    /// `permission`), derived from the display.
    pub compat: Option<&'static str>,
    /// The lead went to `working` on confirmed evidence (Q14: touch
    /// Active, debounced by the store).
    pub lead_started_working: bool,
}

/// Roster counts (§7.2 `children`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChildCounts {
    pub subagents: usize,
    pub shells: usize,
    pub monitors: usize,
    pub crons: usize,
    pub unknown: usize,
    pub owed: usize,
    pub waiting: usize,
}

/// One live v2 session's activity (DA19).
#[derive(Debug, Clone)]
pub struct Row {
    pub facts: RowFacts,
    pub lead: Lead,
    pub children: BTreeMap<String, Child>,
    /// Lead tools seen `PreToolUse` without a Post yet: (tool_use_id, tool_name).
    pub(crate) in_flight: Vec<(String, String)>,
    pub(crate) latch: Option<Latch>,
    pub(crate) pending_cancel: Option<PendingCancel>,
    pub turn_started_at: Option<i64>,
    pub lead_idle_since: Option<i64>,
    pub evidence_at: Option<i64>,
    pub evidence_source: Option<EvidenceSource>,
    /// Bumped on every piece of evidence (the Ctrl-C settle baseline).
    pub(crate) evidence_seq: u64,
    pub(crate) had_hook: bool,
    pub(crate) had_transcript: bool,
    /// S3 sets this once a transcript path resolves (A16 fallback rule).
    pub(crate) transcript_resolvable: bool,
    /// The last hook evidence (DA27: a transcript `end_turn` older than
    /// this is stale).
    pub(crate) last_hook_at: Option<i64>,
    /// A Claude transcript `end_turn` waiting out its 5 s grace:
    /// (due, armed at). A hook after `armed at` cancels it.
    pub(crate) pending_transcript_end: Option<(i64, i64)>,
    /// DA30: false until the first evidence after registration.
    pub confirmed: bool,
    /// The lead was confirmed working in this registration (DA32).
    pub confirmed_working: bool,
    /// The most recent end (lead or child), the idle display's reason.
    pub end_reason: Option<Reason>,
    pub display: Display,
    pub reason: Reason,
    pub stale_since: Option<i64>,
    pub rev: u64,
    pub registered_at: i64,
    pub(crate) last_status_write: Option<&'static str>,
    pub(crate) last_compat: Option<&'static str>,
    pub(crate) pending_turn_end: Option<TurnEnded>,
}

/// A comparable picture of what a client draws.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Visible {
    display: Display,
    reason: Reason,
    lead: (LeadState, Outcome),
    counts: ChildCounts,
    stale_since: Option<i64>,
    confirmed: bool,
}

impl Row {
    /// A freshly registered session: idle, unconfirmed (DA30).
    pub fn new(facts: RowFacts, now: i64) -> Self {
        Self {
            facts,
            lead: Lead {
                state: LeadState::Idle,
                outcome: Outcome::None,
                since: now,
                prompt_id: None,
                waiting: None,
                end_reason: None,
            },
            children: BTreeMap::new(),
            in_flight: Vec::new(),
            latch: None,
            pending_cancel: None,
            turn_started_at: None,
            lead_idle_since: Some(now),
            evidence_at: None,
            evidence_source: None,
            evidence_seq: 0,
            had_hook: false,
            had_transcript: false,
            transcript_resolvable: false,
            last_hook_at: None,
            pending_transcript_end: None,
            confirmed: false,
            confirmed_working: false,
            end_reason: None,
            display: Display::Idle,
            reason: Reason::Unconfirmed,
            stale_since: None,
            rev: 0,
            registered_at: now,
            last_status_write: None,
            last_compat: None,
            pending_turn_end: None,
        }
    }

    /// Apply one input at `now` and re-fold.
    pub fn apply(&mut self, ev: Evidence<'_>, now: i64) -> Change {
        let before = self.visible();
        let lead_before = self.lead.state;
        // Timers due before this input fire first, in order.
        ends::run_timers(self, now);
        match ev {
            Evidence::Hook(env) => claude::apply_hook(self, env, now),
            Evidence::LegacyHook(bucket) => claude::apply_bucket(self, bucket, EvidenceSource::Hook, now),
            Evidence::Title(signal) => ends::apply_title(self, signal, now),
            Evidence::Key(key) => ends::on_key(self, key, now),
            Evidence::TranscriptInterrupt => ends::transcript_interrupt(self, now),
            Evidence::TranscriptTurnEnd => ends::transcript_turn_end(self, now),
            Evidence::Transcript(signal) => ends::apply_transcript(self, signal, now),
            Evidence::Screen(present) => ends::apply_screen(self, present, now),
            Evidence::OwnerReleased => ends::process_exit(self, Reason::AgentExited, now),
            Evidence::PtyExited => ends::process_exit(self, Reason::PtyExited, now),
        }
        self.finish(before, lead_before, now)
    }

    /// Fire due timers (owed lease, key settle, decay) at `now`.
    pub fn tick(&mut self, now: i64) -> Change {
        let before = self.visible();
        let lead_before = self.lead.state;
        ends::run_timers(self, now);
        self.finish(before, lead_before, now)
    }

    /// The earliest time [`Row::tick`] has something to do.
    pub fn next_deadline(&self) -> Option<i64> {
        let mut best: Option<i64> = None;
        let mut take = |t: i64| best = Some(best.map_or(t, |b| b.min(t)));
        if let Some(t) = ends::next_timer(self) {
            take(t);
        }
        if self.display != Display::Unverifiable && fold::raw_display(self).is_live() {
            take(self.evidence_at.unwrap_or(self.registered_at) + STALE_AFTER_MS);
        }
        best
    }

    /// S3: a transcript path resolved for this session, so a Ctrl-C waits
    /// for the transcript's interrupt record instead of the 500 ms settle.
    pub fn set_transcript_resolvable(&mut self, yes: bool) {
        self.transcript_resolvable = yes;
    }

    /// A hook has reached this row since registration (hooks work).
    pub fn had_hook(&self) -> bool {
        self.had_hook
    }

    /// Transcript evidence has reached this row since registration.
    pub fn had_transcript(&self) -> bool {
        self.had_transcript
    }

    /// S3: the transcript follower should read this session fast (200 ms):
    /// the lead is mid-turn, a keystroke cancel waits for its record, or a
    /// transcript `end_turn` waits out its grace.
    pub fn transcript_hot(&self) -> bool {
        self.lead.state != LeadState::Idle
            || self.pending_cancel.is_some()
            || self.pending_transcript_end.is_some()
    }

    /// The last legacy lifecycle word this row emitted (RL5), so a removal
    /// can close out an older client that last heard `start`.
    pub fn compat_heard(&self) -> Option<&'static str> {
        self.last_compat
    }

    pub fn counts(&self) -> ChildCounts {
        let mut c = ChildCounts::default();
        for child in self.children.values() {
            match child.state {
                ChildState::Owed => {
                    c.owed += 1;
                    continue;
                }
                ChildState::Waiting => c.waiting += 1,
                ChildState::Running => {}
            }
            match child.kind {
                ChildKind::Subagent => c.subagents += 1,
                ChildKind::Shell => c.shells += 1,
                ChildKind::Monitor => c.monitors += 1,
                ChildKind::Cron => c.crons += 1,
                ChildKind::Unknown => c.unknown += 1,
            }
        }
        c
    }

    /// TW7, the Thread strip's counts: (subagents still holding the turn,
    /// i.e. running, waiting or owed subagent children; background shells
    /// and monitors still running or waiting). Counts only, no labels.
    pub fn thread_counts(&self) -> (usize, usize) {
        let mut subagents = 0;
        let mut background = 0;
        for child in self.children.values() {
            match child.kind {
                ChildKind::Subagent => subagents += 1,
                ChildKind::Shell | ChildKind::Monitor if child.state != ChildState::Owed => background += 1,
                _ => {}
            }
        }
        (subagents, background)
    }

    fn visible(&self) -> Visible {
        Visible {
            display: self.display,
            reason: self.reason,
            lead: (self.lead.state, self.lead.outcome),
            counts: self.counts(),
            stale_since: self.stale_since,
            confirmed: self.confirmed,
        }
    }

    fn finish(&mut self, before: Visible, lead_before: LeadState, now: i64) -> Change {
        fold::refold(self, now);
        let after = self.visible();
        let changed = after != before;
        if changed {
            self.rev += 1;
        }
        let status_write = lock::status_target(self).filter(|t| self.last_status_write != Some(*t));
        if let Some(t) = status_write {
            self.last_status_write = Some(t);
        }
        let compat = lock::compat_target(self).filter(|w| {
            self.last_compat != Some(*w) && !(self.last_compat.is_none() && *w == "stop")
        });
        if let Some(w) = compat {
            self.last_compat = Some(w);
        }
        Change {
            changed,
            turn_ended: self.pending_turn_end.take(),
            status_write,
            compat,
            lead_started_working: self.confirmed
                && lead_before != LeadState::Working
                && self.lead.state == LeadState::Working,
        }
    }

    /// Evidence arrived (DA29: only receipt of evidence moves `evidenceAt`).
    pub(crate) fn note_evidence(&mut self, source: EvidenceSource, now: i64) {
        if source == EvidenceSource::Hook {
            self.last_hook_at = Some(now);
        }
        self.evidence_at = Some(now);
        self.evidence_source = Some(source);
        self.evidence_seq += 1;
        self.confirmed = true;
    }

    /// The row as §7.2 JSON. No tool lines, tool names, or task names.
    pub fn to_json(&self) -> Value {
        let c = self.counts();
        json!({
            "sessionId": self.facts.session_id,
            "agentName": self.facts.agent_name,
            "projectId": self.facts.project_id,
            "workspacePath": self.facts.workspace_path,
            "harness": self.facts.harness,
            "display": self.display.as_str(),
            "lead": {
                "state": self.lead.state.as_str(),
                "outcome": self.lead.outcome.as_str(),
                "since": self.lead.since,
                "promptId": self.lead.prompt_id,
            },
            "children": {
                "subagents": c.subagents,
                "shells": c.shells,
                "monitors": c.monitors,
                "crons": c.crons,
                "unknown": c.unknown,
                "owed": c.owed,
                "waiting": c.waiting,
            },
            "turnStartedAt": self.turn_started_at,
            "evidenceAt": self.evidence_at,
            "evidenceSource": self.evidence_source.map(EvidenceSource::as_str),
            "reason": self.reason.as_str(),
            "staleSince": self.stale_since,
            "confirmed": self.confirmed,
            "rev": self.rev,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// DA31: the vocabulary is pinned by `fixtures/reasons.json`, the
    /// list the renderer's copy test (S5) reads. Regenerate with
    /// `K2_WRITE_ACTIVITY_FIXTURES=1` after a PRD amendment.
    #[test]
    fn reasons_fixture_matches_the_vocabulary() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/activity/fixtures/reasons.json");
        let want: Vec<&str> = Reason::ALL.iter().map(|r| r.as_str()).collect();
        let text = serde_json::to_string_pretty(&want).expect("serialize") + "\n";
        if std::env::var_os("K2_WRITE_ACTIVITY_FIXTURES").is_some() {
            std::fs::write(&path, &text).expect("write reasons.json");
        }
        let on_disk = std::fs::read_to_string(&path).expect("reasons.json is committed");
        assert_eq!(on_disk, text, "reasons.json is out of date with Reason::ALL");
        // DA31 + A22/A24: 26 base values plus `stale` and `unbound_turn_end`.
        assert_eq!(Reason::ALL.len(), 28);
        let mut uniq = want.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), want.len(), "duplicate reason strings");
    }

    /// TW7: subagents count while running, waiting or owed; background
    /// work (shells, monitors) only while it still runs.
    #[test]
    fn thread_counts_follow_tw7() {
        let mut row = Row::new(RowFacts { session_id: "s".into(), ..Default::default() }, 1_000);
        let child = |kind, state| Child {
            kind,
            state,
            origin: ChildOrigin::Hook,
            since: 1_000,
            owed_at: None,
            background: false,
        };
        row.children.insert("a".into(), child(ChildKind::Subagent, ChildState::Running));
        row.children.insert("b".into(), child(ChildKind::Subagent, ChildState::Owed));
        row.children.insert("c".into(), child(ChildKind::Subagent, ChildState::Waiting));
        row.children.insert("d".into(), child(ChildKind::Shell, ChildState::Running));
        row.children.insert("e".into(), child(ChildKind::Shell, ChildState::Owed));
        row.children.insert("f".into(), child(ChildKind::Monitor, ChildState::Running));
        row.children.insert("g".into(), child(ChildKind::Cron, ChildState::Running));
        assert_eq!(row.thread_counts(), (3, 2));
    }

    #[test]
    fn a_new_row_is_idle_and_unconfirmed() {
        let row = Row::new(RowFacts { session_id: "s".into(), ..Default::default() }, 1_000);
        assert_eq!(row.display, Display::Idle);
        assert_eq!(row.reason, Reason::Unconfirmed);
        assert!(!row.confirmed);
        let j = row.to_json();
        assert_eq!(j["display"], "idle");
        assert_eq!(j["reason"], "unconfirmed");
        assert_eq!(j["confirmed"], false);
        assert_eq!(j["evidenceAt"], Value::Null);
    }
}
