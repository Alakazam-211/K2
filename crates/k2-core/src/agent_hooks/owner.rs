//! The owner check: only the pane's own agent may change its activity row
//! (prd-daemon-activity-and-thread-working-v1 DA14 + A5/A6, DA15).
//!
//! Pure and clock-free: the process table is injected
//! ([`ProcessTable`]), so the rules are tested without real processes.
//! The live table is [`crate::proc_ancestry::LiveProcessTable`].
//!
//! The rules, per envelope:
//! 1. **Live pane.** `X-K2-Pane` must be a live v2 session, else
//!    [`Verdict::UnknownPane`].
//! 2. **Ancestry.** Walking up from `X-K2-Agent-Pid` (at most
//!    [`MAX_ANCESTRY_DEPTH`] levels) must reach the session's PTY child,
//!    else [`Verdict::Foreign`]. This is what drops stale env inherited by
//!    a detached process (the Codex shared app-server case, DA15): its
//!    chain ends at init, never at the PTY child.
//! 3. **Claim.** With no owner yet: (a) a sender that IS the PTY child
//!    owns the pane (the normal case: the PTY child is the CLI);
//!    (b) otherwise the first passing envelope without an `agent_id`
//!    claims, unless its ancestry holds a pid that already sent hooks for
//!    this pane (a nested `claude -p`). `session_id` equality is a logged
//!    cross-check, never a gate.
//! 4. **After the claim** only the owner `(pid, start_time)` is accepted.
//!    A `SessionStart` with `source` `clear|resume|fork` from the owner
//!    updates the conversation id.
//! 5. **Foreign** envelopes are counted and change nothing.
//! 6. **Owner exit.** When the owner pid is gone or its start time
//!    changed (pid reuse), the claim is released (`agent_exited`).
//!
//! On Windows there is no pid to walk (Git Bash `$PPID` is an MSYS pid),
//! so [`OwnerCheckMode::Conversation`] skips step 2 and claims by
//! conversation id instead (A6, Q16). `k2 hooks status` labels it.

use serde::Serialize;

/// Deepest parent walk from the hook sender (DA14-2).
pub const MAX_ANCESTRY_DEPTH: usize = 16;

/// Most distinct hook senders remembered per pane.
const KNOWN_SENDERS_CAP: usize = 32;

/// One process, as the owner check sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcInfo {
    pub pid: i32,
    pub ppid: i32,
    /// Opaque, monotonic per boot: tells a reused pid apart.
    pub start_time: u64,
}

/// Process lookup. `None` when the pid is not alive.
pub trait ProcessTable {
    fn info(&self, pid: i32) -> Option<ProcInfo>;
}

/// `pid`'s chain upward, starting with `pid` itself, at most `max`
/// entries. Stops at a missing parent, pid ≤ 1, or a self-parent loop.
pub fn ancestry(table: &dyn ProcessTable, start: ProcInfo, max: usize) -> Vec<ProcInfo> {
    let mut chain = vec![start];
    let mut cur = start;
    while chain.len() < max && cur.ppid > 1 && cur.ppid != cur.pid {
        let Some(parent) = table.info(cur.ppid) else { break };
        if chain.iter().any(|p| p.pid == parent.pid) {
            break;
        }
        chain.push(parent);
        cur = parent;
    }
    chain
}

/// Which owner check this platform can run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerCheckMode {
    /// macOS + Linux: process ancestry (DA14).
    Ancestry,
    /// Windows: conversation id only (A6, Q16).
    Conversation,
}

impl OwnerCheckMode {
    pub const fn for_this_platform() -> Self {
        if cfg!(windows) {
            Self::Conversation
        } else {
            Self::Ancestry
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ancestry => "ancestry",
            Self::Conversation => "conversation",
        }
    }
}

/// A process identity that survives pid reuse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcKey {
    pub pid: i32,
    pub start_time: u64,
}

/// How a claim was made (shown in `k2 hooks status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimVia {
    /// (3a) the sender is the PTY child.
    PtyChild,
    /// (3b) first passing sender with no hook-sending ancestor.
    FirstSender,
    /// Windows: conversation id.
    Conversation,
}

/// The pane's current owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnerClaim {
    /// `None` in conversation mode.
    pub proc: Option<ProcKey>,
    pub claimed_at_ms: i64,
    pub via: ClaimVia,
}

/// Per-pane owner state. Lives in the daemon, one per live v2 session.
#[derive(Debug, Clone, Default)]
pub struct PaneOwner {
    pub claim: Option<OwnerClaim>,
    /// The conversation the owner is on (`session_id`), once known.
    pub conversation_id: Option<String>,
    known_senders: Vec<ProcKey>,
}

/// Facts about the pane's live session, read by the caller.
#[derive(Debug, Clone, Default)]
pub struct PaneFacts {
    /// The PTY child pid (`DaemonPtySession::child_pid`).
    pub child_pid: Option<i32>,
    /// A conversation id the daemon already knows (premint / resume argv).
    pub known_conversation_id: Option<String>,
}

/// The envelope fields the owner check reads.
#[derive(Debug, Clone, Copy)]
pub struct Sender<'a> {
    pub agent_pid: Option<i32>,
    pub session_id: Option<&'a str>,
    pub agent_id: Option<&'a str>,
    pub event: &'a str,
    /// `SessionStart.source` (and friends).
    pub source_field: Option<&'a str>,
}

impl<'a> Sender<'a> {
    pub fn of(env: &'a super::envelope::HookEnvelope) -> Self {
        Self {
            agent_pid: env.agent_pid,
            session_id: env.session_id.as_deref(),
            agent_id: env.agent_id.as_deref(),
            event: env.event.as_str(),
            source_field: env.source_field.as_deref(),
        }
    }

    fn is_conversation_switch(&self) -> bool {
        self.event == "SessionStart"
            && matches!(self.source_field, Some("clear" | "resume" | "fork"))
    }
}

/// Why an envelope was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ForeignReason {
    /// No `X-K2-Agent-Pid` (an old hook command) in ancestry mode.
    NoPid,
    /// The sender pid is not alive.
    SenderGone,
    /// The chain never reached the PTY child (stale env, DA15).
    NotDescendant,
    /// The pane has an owner and this is another process (nested CLI).
    NotOwner,
    /// No owner yet, and an ancestor already sent hooks for this pane.
    NestedUnderSender,
    /// No owner yet, and this is a child (`agent_id`) event.
    UnclaimedChild,
    /// Conversation mode: a different conversation.
    ConversationMismatch,
}

/// The decision for one envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// From the pane's owner: it may change the row.
    Owner,
    Foreign(ForeignReason),
    UnknownPane,
}

/// What [`check`] decided, plus the side effects the store needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckOutcome {
    pub verdict: Verdict,
    /// This envelope made the claim.
    pub claimed_now: bool,
    /// A previous owner was released first (`agent_exited`).
    pub released_previous: bool,
    /// The owner moved to a new conversation (`SessionStart` clear/resume/fork,
    /// or the first claim learning it).
    pub conversation_changed: Option<String>,
    /// The envelope's `session_id` disagrees with the daemon's known
    /// conversation id (logged, never a gate).
    pub session_id_mismatch: bool,
}

impl CheckOutcome {
    fn new(verdict: Verdict) -> Self {
        Self {
            verdict,
            claimed_now: false,
            released_previous: false,
            conversation_changed: None,
            session_id_mismatch: false,
        }
    }
}

impl PaneOwner {
    /// Release the claim when its process is gone or was replaced
    /// (DA14-6). Returns true when it released. Conversation-mode claims
    /// carry no pid and are released only by the session going away.
    pub fn release_if_dead(&mut self, procs: &dyn ProcessTable) -> bool {
        let Some(key) = self.claim.as_ref().and_then(|c| c.proc) else {
            return false;
        };
        match procs.info(key.pid) {
            Some(info) if info.start_time == key.start_time => false,
            _ => {
                self.claim = None;
                true
            }
        }
    }

    fn remember_sender(&mut self, key: ProcKey) {
        if self.known_senders.contains(&key) {
            return;
        }
        if self.known_senders.len() >= KNOWN_SENDERS_CAP {
            self.known_senders.remove(0);
        }
        self.known_senders.push(key);
    }

    fn take_claim(&mut self, proc: Option<ProcKey>, via: ClaimVia, now_ms: i64) {
        self.claim = Some(OwnerClaim { proc, claimed_at_ms: now_ms, via });
    }

    /// Owner accepted: learn or switch the conversation id.
    fn note_conversation(&mut self, sender: &Sender<'_>, out: &mut CheckOutcome) {
        let Some(sid) = sender.session_id else { return };
        let switch = sender.is_conversation_switch();
        if self.conversation_id.is_none() || (switch && self.conversation_id.as_deref() != Some(sid)) {
            self.conversation_id = Some(sid.to_string());
            out.conversation_changed = Some(sid.to_string());
        }
    }
}

/// Run the owner check for one envelope against `state` (DA14).
pub fn check(
    state: &mut PaneOwner,
    pane: Option<&PaneFacts>,
    sender: &Sender<'_>,
    procs: &dyn ProcessTable,
    mode: OwnerCheckMode,
    now_ms: i64,
) -> CheckOutcome {
    let Some(pane) = pane else {
        return CheckOutcome::new(Verdict::UnknownPane);
    };
    let mismatch = matches!(
        (pane.known_conversation_id.as_deref(), sender.session_id),
        (Some(known), Some(got)) if known != got && !sender.is_conversation_switch()
    );
    let mut out = match mode {
        OwnerCheckMode::Ancestry => check_ancestry(state, pane, sender, procs, now_ms),
        OwnerCheckMode::Conversation => check_conversation(state, pane, sender, now_ms),
    };
    out.session_id_mismatch = mismatch;
    out
}

fn check_ancestry(
    state: &mut PaneOwner,
    pane: &PaneFacts,
    sender: &Sender<'_>,
    procs: &dyn ProcessTable,
    now_ms: i64,
) -> CheckOutcome {
    // (6) first, so a dead owner never blocks its successor.
    let released = state.release_if_dead(procs);
    let out = |verdict| {
        let mut o = CheckOutcome::new(verdict);
        o.released_previous = released;
        o
    };
    let Some(pid) = sender.agent_pid else {
        return out(Verdict::Foreign(ForeignReason::NoPid));
    };
    let Some(info) = procs.info(pid) else {
        return out(Verdict::Foreign(ForeignReason::SenderGone));
    };
    let Some(child) = pane.child_pid else {
        return out(Verdict::Foreign(ForeignReason::NotDescendant));
    };
    // (2) the chain must reach the PTY child.
    let chain = ancestry(procs, info, MAX_ANCESTRY_DEPTH);
    if !chain.iter().any(|p| p.pid == child) {
        return out(Verdict::Foreign(ForeignReason::NotDescendant));
    }
    let key = ProcKey { pid, start_time: info.start_time };
    let nested = chain[1..].iter().any(|p| {
        state
            .known_senders
            .contains(&ProcKey { pid: p.pid, start_time: p.start_time })
    });
    state.remember_sender(key);

    // (4) an owner exists: only it is accepted.
    if let Some(claim) = &state.claim {
        if claim.proc == Some(key) {
            let mut o = out(Verdict::Owner);
            state.note_conversation(sender, &mut o);
            return o;
        }
        return out(Verdict::Foreign(ForeignReason::NotOwner));
    }

    // (3) no owner yet.
    let via = if pid == child {
        ClaimVia::PtyChild
    } else if sender.agent_id.is_some() {
        return out(Verdict::Foreign(ForeignReason::UnclaimedChild));
    } else if nested {
        return out(Verdict::Foreign(ForeignReason::NestedUnderSender));
    } else {
        ClaimVia::FirstSender
    };
    state.take_claim(Some(key), via, now_ms);
    let mut o = out(Verdict::Owner);
    o.claimed_now = true;
    state.note_conversation(sender, &mut o);
    o
}

fn check_conversation(
    state: &mut PaneOwner,
    pane: &PaneFacts,
    sender: &Sender<'_>,
    now_ms: i64,
) -> CheckOutcome {
    let foreign = || CheckOutcome::new(Verdict::Foreign(ForeignReason::ConversationMismatch));
    if state.claim.is_some() {
        if sender.session_id.is_some() && sender.session_id == state.conversation_id.as_deref() {
            return CheckOutcome::new(Verdict::Owner);
        }
        // A `/clear` or resume moves the owner to a new conversation; a
        // nested `claude -p` starts with `source: startup`, never these.
        if sender.agent_id.is_none() && sender.is_conversation_switch() {
            let mut o = CheckOutcome::new(Verdict::Owner);
            state.note_conversation(sender, &mut o);
            return o;
        }
        return foreign();
    }
    if sender.agent_id.is_some() {
        return CheckOutcome::new(Verdict::Foreign(ForeignReason::UnclaimedChild));
    }
    if let (Some(known), Some(got)) = (pane.known_conversation_id.as_deref(), sender.session_id) {
        if known != got {
            return foreign();
        }
    }
    state.take_claim(None, ClaimVia::Conversation, now_ms);
    let mut o = CheckOutcome::new(Verdict::Owner);
    o.claimed_now = true;
    state.note_conversation(sender, &mut o);
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    /// An injected process table: pid → (ppid, start_time).
    #[derive(Default)]
    struct FakeProcs(RefCell<HashMap<i32, ProcInfo>>);

    impl FakeProcs {
        fn add(&self, pid: i32, ppid: i32, start: u64) -> &Self {
            self.0.borrow_mut().insert(pid, ProcInfo { pid, ppid, start_time: start });
            self
        }
        fn kill(&self, pid: i32) {
            self.0.borrow_mut().remove(&pid);
        }
    }

    impl ProcessTable for FakeProcs {
        fn info(&self, pid: i32) -> Option<ProcInfo> {
            self.0.borrow().get(&pid).copied()
        }
    }

    fn sender<'a>(pid: i32, event: &'a str, sid: Option<&'a str>) -> Sender<'a> {
        Sender { agent_pid: Some(pid), session_id: sid, agent_id: None, event, source_field: None }
    }

    fn pane(child: i32) -> PaneFacts {
        PaneFacts { child_pid: Some(child), known_conversation_id: None }
    }

    const A: OwnerCheckMode = OwnerCheckMode::Ancestry;

    /// The daemon (10) spawned the PTY child = the agent CLI (100). The
    /// agent's Bash tool runs `sh` (200), which runs a nested `claude -p`
    /// (300) on another conversation.
    fn agent_tree() -> FakeProcs {
        let p = FakeProcs::default();
        p.add(10, 1, 1).add(100, 10, 2).add(200, 100, 3).add(300, 200, 4);
        p
    }

    #[test]
    fn t_s1c_nested_claude_p_is_foreign() {
        let procs = agent_tree();
        let mut st = PaneOwner::default();
        let out = check(&mut st, Some(&pane(100)), &sender(100, "SessionStart", Some("conv-a")), &procs, A, 1);
        assert_eq!(out.verdict, Verdict::Owner);
        assert!(out.claimed_now);
        assert_eq!(st.claim.as_ref().map(|c| c.via), Some(ClaimVia::PtyChild));
        assert_eq!(out.conversation_changed.as_deref(), Some("conv-a"));

        // Grandchild of the PTY child, other session_id: foreign.
        let out = check(&mut st, Some(&pane(100)), &sender(300, "UserPromptSubmit", Some("conv-n")), &procs, A, 2);
        assert_eq!(out.verdict, Verdict::Foreign(ForeignReason::NotOwner));
        assert_eq!(st.conversation_id.as_deref(), Some("conv-a"), "a foreign envelope changes nothing");
    }

    #[test]
    fn t_s1c_dead_pane_is_unknown() {
        let procs = agent_tree();
        let mut st = PaneOwner::default();
        let out = check(&mut st, None, &sender(100, "Stop", None), &procs, A, 1);
        assert_eq!(out.verdict, Verdict::UnknownPane);
        assert!(st.claim.is_none());
    }

    #[test]
    fn t_s1c_owner_after_clear_is_accepted_and_conversation_moves() {
        let procs = agent_tree();
        let mut st = PaneOwner::default();
        check(&mut st, Some(&pane(100)), &sender(100, "UserPromptSubmit", Some("conv-a")), &procs, A, 1);
        let mut s = sender(100, "SessionStart", Some("conv-b"));
        s.source_field = Some("clear");
        let out = check(&mut st, Some(&pane(100)), &s, &procs, A, 2);
        assert_eq!(out.verdict, Verdict::Owner);
        assert_eq!(out.conversation_changed.as_deref(), Some("conv-b"));
        assert_eq!(st.conversation_id.as_deref(), Some("conv-b"));

        // A plain event with a different session_id never moves it.
        let out = check(&mut st, Some(&pane(100)), &sender(100, "Stop", Some("conv-z")), &procs, A, 3);
        assert_eq!(out.verdict, Verdict::Owner);
        assert_eq!(out.conversation_changed, None);
        assert_eq!(st.conversation_id.as_deref(), Some("conv-b"));
    }

    #[test]
    fn t_s1c_pid_reuse_releases_the_claim() {
        // A shell tab: PTY child is a shell (100); the user ran `claude` (200).
        let procs = FakeProcs::default();
        procs.add(10, 1, 1).add(100, 10, 2).add(200, 100, 3);
        let mut st = PaneOwner::default();
        let out = check(&mut st, Some(&pane(100)), &sender(200, "UserPromptSubmit", None), &procs, A, 1);
        assert!(out.claimed_now);

        // 200 exits; a new process reuses pid 200 with a new start time.
        procs.kill(200);
        procs.add(200, 100, 9);
        let out = check(&mut st, Some(&pane(100)), &sender(200, "SessionStart", None), &procs, A, 2);
        assert!(out.released_previous, "pid reuse must release the old claim");
        assert!(out.claimed_now, "the new process may claim");
        assert_eq!(st.claim.as_ref().and_then(|c| c.proc).map(|k| k.start_time), Some(9));
    }

    #[test]
    fn owner_exit_releases_without_an_envelope() {
        let procs = agent_tree();
        let mut st = PaneOwner::default();
        check(&mut st, Some(&pane(100)), &sender(100, "UserPromptSubmit", None), &procs, A, 1);
        assert!(!st.release_if_dead(&procs));
        procs.kill(100);
        assert!(st.release_if_dead(&procs));
        assert!(st.claim.is_none());
    }

    #[test]
    fn t_s1c_plus_shell_tab_claude_claims_and_its_nested_run_is_foreign() {
        // PTY child = shell (100); `claude` (200, grandchild of nothing but
        // the shell) → its Bash tool sh (300) → nested `claude -p` (400).
        let procs = FakeProcs::default();
        procs.add(10, 1, 1).add(100, 10, 2).add(200, 100, 3).add(300, 200, 4).add(400, 300, 5);
        let mut st = PaneOwner::default();
        let out = check(&mut st, Some(&pane(100)), &sender(200, "SessionStart", Some("c1")), &procs, A, 1);
        assert_eq!(out.verdict, Verdict::Owner);
        assert_eq!(st.claim.as_ref().map(|c| c.via), Some(ClaimVia::FirstSender));

        let out = check(&mut st, Some(&pane(100)), &sender(400, "UserPromptSubmit", Some("c2")), &procs, A, 2);
        assert_eq!(out.verdict, Verdict::Foreign(ForeignReason::NotOwner));
    }

    #[test]
    fn nested_sender_cannot_claim_after_the_owner_left() {
        // The owner (200) and its Bash shell (300) both sent hooks. 200
        // exits; 300 is reparented under the PTY child (100) and its
        // nested run (400) keeps going.
        let procs = FakeProcs::default();
        procs.add(10, 1, 1).add(100, 10, 2).add(200, 100, 3).add(300, 200, 4).add(400, 300, 5);
        let mut st = PaneOwner::default();
        check(&mut st, Some(&pane(100)), &sender(200, "UserPromptSubmit", None), &procs, A, 1);
        check(&mut st, Some(&pane(100)), &sender(300, "UserPromptSubmit", None), &procs, A, 2);
        procs.kill(200);
        procs.add(300, 100, 4);
        let out = check(&mut st, Some(&pane(100)), &sender(400, "UserPromptSubmit", None), &procs, A, 3);
        assert!(out.released_previous);
        assert_eq!(out.verdict, Verdict::Foreign(ForeignReason::NestedUnderSender));
        assert!(st.claim.is_none());
    }

    #[test]
    fn t_s1d_codex_shared_app_server_stale_env_is_foreign() {
        // A live session A (PTY child 100). A detached `codex app-server`
        // (500, ppid 1) inherited A's env and starts a hooked CLI (600).
        let procs = agent_tree();
        procs.add(500, 1, 7).add(600, 500, 8);
        let mut st = PaneOwner::default();
        let out = check(&mut st, Some(&pane(100)), &sender(600, "UserPromptSubmit", Some("x")), &procs, A, 1);
        assert_eq!(out.verdict, Verdict::Foreign(ForeignReason::NotDescendant));
        assert!(st.claim.is_none(), "the row is unchanged: no claim");
        // Even after a real owner claims, the stale sender stays foreign.
        check(&mut st, Some(&pane(100)), &sender(100, "SessionStart", Some("a")), &procs, A, 2);
        let out = check(&mut st, Some(&pane(100)), &sender(600, "Stop", Some("x")), &procs, A, 3);
        assert_eq!(out.verdict, Verdict::Foreign(ForeignReason::NotDescendant));
    }

    #[test]
    fn missing_pid_and_dead_sender_are_foreign() {
        let procs = agent_tree();
        let mut st = PaneOwner::default();
        let mut s = sender(100, "Stop", None);
        s.agent_pid = None;
        assert_eq!(
            check(&mut st, Some(&pane(100)), &s, &procs, A, 1).verdict,
            Verdict::Foreign(ForeignReason::NoPid)
        );
        assert_eq!(
            check(&mut st, Some(&pane(100)), &sender(999, "Stop", None), &procs, A, 1).verdict,
            Verdict::Foreign(ForeignReason::SenderGone)
        );
    }

    #[test]
    fn child_event_never_claims_unless_from_the_pty_child() {
        let procs = FakeProcs::default();
        procs.add(10, 1, 1).add(100, 10, 2).add(200, 100, 3);
        let mut st = PaneOwner::default();
        let mut s = sender(200, "PostToolUse", None);
        s.agent_id = Some("sub-1");
        assert_eq!(
            check(&mut st, Some(&pane(100)), &s, &procs, A, 1).verdict,
            Verdict::Foreign(ForeignReason::UnclaimedChild)
        );
        // From the PTY child itself (subagents run in the agent process).
        let mut s = sender(100, "SubagentStart", None);
        s.agent_id = Some("sub-1");
        assert_eq!(check(&mut st, Some(&pane(100)), &s, &procs, A, 2).verdict, Verdict::Owner);
    }

    #[test]
    fn known_conversation_mismatch_is_logged_not_gated() {
        let procs = agent_tree();
        let mut st = PaneOwner::default();
        let facts = PaneFacts { child_pid: Some(100), known_conversation_id: Some("premint".into()) };
        let out = check(&mut st, Some(&facts), &sender(100, "UserPromptSubmit", Some("other")), &procs, A, 1);
        assert_eq!(out.verdict, Verdict::Owner);
        assert!(out.session_id_mismatch);
    }

    #[test]
    fn ancestry_walk_is_bounded_and_loop_safe() {
        let procs = FakeProcs::default();
        for pid in 2..40 {
            procs.add(pid, pid - 1, pid as u64);
        }
        let start = procs.info(39).expect("start");
        assert_eq!(ancestry(&procs, start, MAX_ANCESTRY_DEPTH).len(), MAX_ANCESTRY_DEPTH);
        let looped = FakeProcs::default();
        looped.add(5, 6, 1).add(6, 5, 1);
        let start = looped.info(5).expect("start");
        assert_eq!(ancestry(&looped, start, MAX_ANCESTRY_DEPTH).len(), 2);
    }

    #[test]
    fn conversation_mode_claims_by_session_id() {
        let procs = FakeProcs::default();
        let mut st = PaneOwner::default();
        let facts = PaneFacts { child_pid: None, known_conversation_id: Some("conv-a".into()) };
        let c = OwnerCheckMode::Conversation;
        // A sender on another conversation can't claim a premint pane.
        let out = check(&mut st, Some(&facts), &sender(0, "UserPromptSubmit", Some("conv-x")), &procs, c, 1);
        assert_eq!(out.verdict, Verdict::Foreign(ForeignReason::ConversationMismatch));
        let out = check(&mut st, Some(&facts), &sender(0, "UserPromptSubmit", Some("conv-a")), &procs, c, 2);
        assert!(out.claimed_now);
        assert_eq!(st.claim.as_ref().map(|c| c.via), Some(ClaimVia::Conversation));
        let mut clear = sender(0, "SessionStart", Some("conv-b"));
        clear.source_field = Some("clear");
        assert_eq!(check(&mut st, Some(&facts), &clear, &procs, c, 3).verdict, Verdict::Owner);
        assert_eq!(st.conversation_id.as_deref(), Some("conv-b"));
        let nested = sender(0, "SessionStart", Some("conv-n"));
        assert_eq!(
            check(&mut st, Some(&facts), &nested, &procs, c, 4).verdict,
            Verdict::Foreign(ForeignReason::ConversationMismatch)
        );
    }

    /// A6: the platform pick is pinned at compile time.
    #[test]
    fn platform_mode_is_pinned() {
        #[cfg(windows)]
        assert_eq!(OwnerCheckMode::for_this_platform(), OwnerCheckMode::Conversation);
        #[cfg(not(windows))]
        assert_eq!(OwnerCheckMode::for_this_platform(), OwnerCheckMode::Ancestry);
    }
}
