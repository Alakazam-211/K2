//! One answer to "is this agent session still alive?" for every sweep that
//! removes per-session resources (the vault's secrets dirs today; the
//! per-session ssh-agent sockets next).
//!
//! A session counts as live when ANY of these holds:
//! - it is registered in `v2_session_map` (every live agent session,
//!   canonical pinned chats included — they register under their bare
//!   project id, which is why they skip the `workspace_tab_sessions`
//!   stamp but are still in the map);
//! - it survived a daemon restart in k2-ptyd and boot's B1 has not
//!   settled it yet (PRD prd-k2-ptyd-v1 §7.9: the B0 set is known right
//!   after the DB opens, the map fills only at B1 — and the vault's first
//!   tick fires as soon as the vault boots, in between);
//! - it is being spawned: its resources were made ([`note_spawning`]) but
//!   it isn't registered yet (a ptyd launch can take seconds);
//! - it is a live terminal of the legacy `TerminalManager`
//!   (`/cli/terminal/create`): those get a passport and vault delivery
//!   too, but never register in `v2_session_map`. Before 0.46.5 the sweep
//!   removed their `$K2_SECRETS_DIR` once the spawn grace ran out, under
//!   the running session (field report 10-24).
//!
//! Resource owners call [`note_spawning`] when they create a session's
//! resource before the session registers, and sweep with [`is_live`] /
//! [`live_session_ids`].

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// How long a session counts as "being spawned" without registering.
pub const SPAWN_GRACE: Duration = Duration::from_secs(120);

static SPAWNING: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);

/// A session's per-session resource was just created and the session is
/// not registered yet: keep it live for [`SPAWN_GRACE`].
pub fn note_spawning(session_id: &str) {
    let mut g = SPAWNING.lock();
    let m = g.get_or_insert_with(HashMap::new);
    let now = Instant::now();
    m.retain(|_, t| now.duration_since(*t) < SPAWN_GRACE);
    m.insert(session_id.to_string(), now);
}

fn spawning_ids() -> Vec<String> {
    let mut g = SPAWNING.lock();
    let Some(m) = g.as_mut() else { return Vec::new() };
    let now = Instant::now();
    m.retain(|_, t| now.duration_since(*t) < SPAWN_GRACE);
    m.keys().cloned().collect()
}

/// Session ids that survived a restart and are still waiting for B1.
fn pending_adoption_ids() -> HashSet<String> {
    #[cfg(unix)]
    {
        k2_core::terminal::ptyd::adopt::pending_sids()
    }
    #[cfg(not(unix))]
    {
        HashSet::new()
    }
}

/// Every live session id (see the module doc). One snapshot; callers
/// sweeping many resources take it once.
pub fn live_session_ids() -> HashSet<String> {
    let mut live: HashSet<String> =
        crate::v2_session_map::snapshot().into_iter().map(|(_, s)| s.session_id.to_string()).collect();
    live.extend(pending_adoption_ids());
    live.extend(spawning_ids());
    live.extend(legacy_terminal_ids());
    live
}

/// Live legacy `TerminalManager` terminals, by the id their spawn used
/// for vault delivery (the parsed session id) and as given.
fn legacy_terminal_ids() -> Vec<String> {
    let ids: Vec<String> = {
        let m = k2_core::terminal::shared();
        let g = m.lock();
        g.list_terminal_ids().into_iter().map(|(id, _)| id).collect()
    };
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(sid) = k2_core::session::SessionId::parse(&id) {
            out.push(sid.to_string());
        }
        out.push(id);
    }
    out
}

/// Is `session_id` live? (One lookup; for a sweep over many ids use
/// [`live_session_ids`].)
pub fn is_live(session_id: &str) -> bool {
    live_session_ids().contains(session_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spawning_session_is_live_until_the_grace_ends() {
        let sid = format!("spawning-{}", std::process::id());
        assert!(!is_live(&sid));
        note_spawning(&sid);
        assert!(is_live(&sid), "a session whose resources exist but that isn't registered yet is live");
        // Expire it by back-dating its stamp past the grace.
        SPAWNING.lock().as_mut().unwrap().insert(sid.clone(), Instant::now() - SPAWN_GRACE - Duration::from_secs(1));
        assert!(!is_live(&sid), "a spawn that never registered stops counting after the grace");
    }

    /// 0.46.5 (field report 10-24): a legacy `/cli/terminal/create`
    /// terminal never registers in `v2_session_map`, so the minute sweep
    /// took its `$K2_SECRETS_DIR` once the spawn grace ran out. A running
    /// legacy terminal is live (under the parsed id its spawn used for
    /// vault delivery, and as given); after its kill it isn't.
    #[cfg(unix)]
    #[test]
    fn a_running_legacy_terminal_is_live() {
        let raw = uuid::Uuid::new_v4().to_string().to_uppercase();
        let parsed = k2_core::session::SessionId::parse(&raw).expect("a uuid").to_string();
        assert!(!is_live(&parsed) && !is_live(&raw));
        let dir = std::env::temp_dir();
        {
            let m = k2_core::terminal::shared();
            let mut m = m.lock();
            m.create(
                raw.clone(),
                dir.to_string_lossy().into_owned(),
                Some("cat".into()),
                None,
                Some(80),
                Some(24),
                std::sync::Arc::new(k2_core::workspace::wake_prompts::NoOpTerminalEventSink),
            )
            .expect("legacy terminal");
        }
        let live = live_session_ids();
        let kill = || {
            let m = k2_core::terminal::shared();
            let mut m = m.lock();
            m.kill(&raw).expect("kill legacy terminal");
        };
        if !(live.contains(&parsed) && live.contains(&raw)) {
            kill();
            panic!("a running legacy terminal must be live under {parsed} and {raw}");
        }
        kill();
        assert!(!is_live(&parsed) && !is_live(&raw), "a killed legacy terminal is not live");
    }

    #[cfg(unix)]
    #[test]
    fn a_session_waiting_for_adoption_is_live() {
        use k2_core::terminal::ptyd::adopt;
        use k2_ptyd::proto::{SessionInfo, SessionState};
        let sid = format!("adopt-{}", std::process::id());
        adopt::set_scan_for_tests(adopt::Scan {
            running: vec![SessionInfo {
                sid: sid.clone(),
                pid: 1,
                state: SessionState::Running,
                cols: 80,
                rows: 24,
                ring_start: 0,
                ring_end: 0,
                started_at: 0,
                meta: serde_json::Value::Null,
            }],
            exited: vec![],
            skipped: None,
        });
        assert!(is_live(&sid), "B0 found it running; B1 hasn't run yet");
        adopt::mark_settled(&sid);
        assert!(!is_live(&sid), "settled and not registered: no longer live");
        adopt::reset_for_tests();
    }
}
