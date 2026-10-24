//! Adoption state after a daemon restart (PRD prd-k2-ptyd-v1 S3, §7.9).
//!
//! **B0** ([`scan`]) runs early in boot, right after the DB opens and
//! before the first boot sweep: it connects to a ptyd that is already
//! running (never launches one), takes ownership (`takeover:true`), lists
//! its sessions and keeps the result here. Every boot sweep that used to
//! assume "no terminal survives a restart" asks this module which session
//! ids are still running ([`running_sids`], [`running`]) and leaves them
//! alone.
//!
//! **B1** (the daemon's `ptyd_adopt`) then rebuilds each running session
//! ([`crate::terminal::daemon_pty::DaemonPtySession::adopt`]), registers
//! it, and calls [`mark_settled`] for it, adopted or not.
//!
//! No ptyd → empty set → today's boot, unchanged.
//!
//! This module also keeps the conversations held by adopted sessions, for
//! the one `--resume` chokepoint (P33) in `DaemonPtySession::spawn`.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use k2_ptyd::proto::{Reply, Request, SessionInfo, SessionState};

use super::client::{self, Owner};

/// What B0 found.
#[derive(Debug, Default, Clone)]
pub struct Scan {
    /// Running sessions, each with the meta its spawn recorded.
    pub running: Vec<SessionInfo>,
    /// Sessions that exited while no daemon was attached.
    pub exited: Vec<SessionInfo>,
    /// Why there was nothing to adopt (`no_ptyd`, a connect error…).
    pub skipped: Option<String>,
}

struct State {
    scan: Scan,
    /// Running sids B1 has not settled yet (adopted, retried, or ended).
    pending: HashSet<String>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
static HELD_CONVERSATIONS: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

/// B0: connect to an existing ptyd (never launch), take ownership, list.
/// Idempotent: the first call's result is kept for the process.
pub fn scan() -> Scan {
    if let Some(s) = lock(&STATE).as_ref() {
        return s.scan.clone();
    }
    let scan = scan_with(client::global());
    let pending = scan.running.iter().map(|s| s.sid.clone()).collect();
    *lock(&STATE) = Some(State { scan: scan.clone(), pending });
    scan
}

/// B0 against an explicit handle (tests).
pub fn scan_with(ptyd: &client::Ptyd) -> Scan {
    let owner = match connect_with_retry(ptyd) {
        Ok(o) => o,
        Err(e) => return Scan { skipped: Some(e), ..Scan::default() },
    };
    list(&owner)
}

/// A ptyd that was just started (or is mid-upgrade) can refuse for a
/// moment; a missing socket never retries.
fn connect_with_retry(ptyd: &client::Ptyd) -> Result<Arc<Owner>, String> {
    let mut last = String::new();
    for attempt in 0..5 {
        match ptyd.connect_existing() {
            Ok(o) => return Ok(o),
            Err(e) if e == "no_ptyd" => return Err(e),
            Err(e) => last = e,
        }
        std::thread::sleep(std::time::Duration::from_millis(100 * (attempt + 1)));
    }
    Err(last)
}

/// `list` → running/exited.
pub fn list(owner: &Owner) -> Scan {
    match owner.request(Request::List) {
        Ok(Reply::Sessions { sessions }) => {
            let (running, exited) = sessions.into_iter().partition(|s| matches!(s.state, SessionState::Running));
            Scan { running, exited, skipped: None }
        }
        Ok(other) => Scan { skipped: Some(format!("list: {other:?}")), ..Scan::default() },
        Err(e) => Scan { skipped: Some(format!("list: {e}")), ..Scan::default() },
    }
}

/// Session ids B0 found running. Empty before B0 and with no ptyd.
pub fn running_sids() -> HashSet<String> {
    lock(&STATE).as_ref().map(|s| s.scan.running.iter().map(|i| i.sid.clone()).collect()).unwrap_or_default()
}

/// The running sessions B0 found (with meta).
pub fn running() -> Vec<SessionInfo> {
    lock(&STATE).as_ref().map(|s| s.scan.running.clone()).unwrap_or_default()
}

/// Sessions that exited while the daemon was down.
pub fn exited() -> Vec<SessionInfo> {
    lock(&STATE).as_ref().map(|s| s.scan.exited.clone()).unwrap_or_default()
}

/// Running sids B1 has not settled yet (a session still waiting for its
/// adoption counts as live for every sweep).
pub fn pending_sids() -> HashSet<String> {
    lock(&STATE).as_ref().map(|s| s.pending.clone()).unwrap_or_default()
}

/// B1 is done with `sid` (adopted, or ended because it couldn't be).
pub fn mark_settled(sid: &str) {
    if let Some(s) = lock(&STATE).as_mut() {
        s.pending.remove(sid);
    }
}

/// Tests: forget the B0 result.
pub fn reset_for_tests() {
    *lock(&STATE) = None;
    *lock(&HELD_CONVERSATIONS) = None;
}

/// Tests: install a B0 result without a ptyd.
pub fn set_scan_for_tests(scan: Scan) {
    let pending = scan.running.iter().map(|s| s.sid.clone()).collect();
    *lock(&STATE) = Some(State { scan, pending });
}

// ── P33: conversations held by adopted sessions ─────────────────────────

/// Record that the adopted session `sid` runs `conversation`.
pub fn hold_conversation(conversation: &str, sid: &str) {
    if conversation.trim().is_empty() {
        return;
    }
    lock(&HELD_CONVERSATIONS).get_or_insert_with(HashMap::new).insert(conversation.to_string(), sid.to_string());
}

/// The session `sid` ended: its conversation is free again.
pub fn release_conversations_of(sid: &str) {
    if let Some(m) = lock(&HELD_CONVERSATIONS).as_mut() {
        m.retain(|_, v| v != sid);
    }
}

/// The adopted session holding `conversation`, if any.
pub fn conversation_holder(conversation: &str) -> Option<String> {
    lock(&HELD_CONVERSATIONS).as_ref().and_then(|m| m.get(conversation).cloned())
}

/// P33: refuse a spawn that would `--resume` (or pre-mint) a conversation
/// an adopted session is still running. `Err(holder_sid)` on a duplicate.
/// A `--fork-session` spawn makes a new conversation and is never a
/// duplicate.
pub fn check_resume(program: Option<&str>, args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--fork-session") {
        return Ok(());
    }
    let Some(conv) = crate::workspace::provider_resume::session_id_from_spawn_argv(program.unwrap_or(""), args) else {
        return Ok(());
    };
    match conversation_holder(&conv) {
        Some(holder) => Err(holder),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn resume_of_a_held_conversation_is_refused_and_freed_on_exit() {
        let _g = crate::test_env::serial(&TEST_LOCK);
        reset_for_tests();
        let conv = "0b4d1f7e-4f8f-4c66-9d11-3a3e8f6a7c21";
        hold_conversation(conv, "sid-a");
        let resume = vec!["--resume".to_string(), conv.to_string()];
        assert_eq!(check_resume(Some("claude"), &resume), Err("sid-a".to_string()));
        let fork = vec!["--resume".to_string(), conv.to_string(), "--fork-session".to_string()];
        assert_eq!(check_resume(Some("claude"), &fork), Ok(()), "a fork is a new conversation");
        assert_eq!(check_resume(Some("claude"), &["--resume".into(), "other".into()]), Ok(()));
        release_conversations_of("sid-a");
        assert_eq!(check_resume(Some("claude"), &resume), Ok(()));
        reset_for_tests();
    }

    #[test]
    fn pending_set_tracks_b1() {
        let _g = crate::test_env::serial(&TEST_LOCK);
        reset_for_tests();
        let info = |sid: &str| SessionInfo {
            sid: sid.into(),
            pid: 1,
            state: SessionState::Running,
            cols: 80,
            rows: 24,
            ring_start: 0,
            ring_end: 0,
            started_at: 0,
            meta: serde_json::Value::Null,
        };
        set_scan_for_tests(Scan { running: vec![info("a"), info("b")], exited: vec![], skipped: None });
        assert_eq!(running_sids().len(), 2);
        mark_settled("a");
        assert_eq!(pending_sids(), ["b".to_string()].into_iter().collect());
        assert_eq!(running_sids().len(), 2, "running stays what B0 saw");
        reset_for_tests();
        assert!(running_sids().is_empty());
    }
}
