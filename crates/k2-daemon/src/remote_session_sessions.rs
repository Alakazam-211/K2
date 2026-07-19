//! In-process map: remote-session PTY `session_id` ↔ grant id.
//!
//! Stage 3 binds every successful `shell/spawn` so write/read can
//! require a matching live grant (or owner break-glass). Revoke and
//! Layer 0 disable kill all bound PTYs via the same v2 unregister path
//! used when a UI tab closes.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use k2_core::session::SessionId;

/// session_id (uuid string) → grant_id, and reverse index grant_id → sessions.
struct Map {
    session_to_grant: HashMap<String, String>,
    grant_to_sessions: HashMap<String, HashSet<String>>,
}

impl Map {
    fn new() -> Self {
        Self {
            session_to_grant: HashMap::new(),
            grant_to_sessions: HashMap::new(),
        }
    }
}

static MAP: OnceLock<Mutex<Map>> = OnceLock::new();

fn map() -> &'static Mutex<Map> {
    MAP.get_or_init(|| Mutex::new(Map::new()))
}

/// Bind a live PTY session id to the grant that minted it.
pub fn bind_session(session_id: &str, grant_id: &str) {
    let session_id = session_id.trim();
    let grant_id = grant_id.trim();
    if session_id.is_empty() || grant_id.is_empty() {
        return;
    }
    let mut m = map().lock().unwrap_or_else(|e| e.into_inner());
    // If rebinding, drop from prior grant's set first.
    if let Some(prev) = m.session_to_grant.insert(session_id.to_string(), grant_id.to_string()) {
        if prev != grant_id {
            if let Some(set) = m.grant_to_sessions.get_mut(&prev) {
                set.remove(session_id);
                if set.is_empty() {
                    m.grant_to_sessions.remove(&prev);
                }
            }
        }
    }
    m.grant_to_sessions
        .entry(grant_id.to_string())
        .or_default()
        .insert(session_id.to_string());
}

/// Drop a single session binding (does not kill the PTY).
pub fn unbind_session(session_id: &str) {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return;
    }
    let mut m = map().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(grant_id) = m.session_to_grant.remove(session_id) {
        if let Some(set) = m.grant_to_sessions.get_mut(&grant_id) {
            set.remove(session_id);
            if set.is_empty() {
                m.grant_to_sessions.remove(&grant_id);
            }
        }
    }
}

/// All session ids currently bound to `grant_id`.
pub fn sessions_for_grant(grant_id: &str) -> Vec<String> {
    let m = map().lock().unwrap_or_else(|e| e.into_inner());
    m.grant_to_sessions
        .get(grant_id)
        .map(|s| s.iter().cloned().collect())
        .unwrap_or_default()
}

/// Grant id bound to this session, if any.
pub fn grant_for_session(session_id: &str) -> Option<String> {
    let m = map().lock().unwrap_or_else(|e| e.into_inner());
    m.session_to_grant.get(session_id).cloned()
}

/// True when `session_id` is a remote-session-bound PTY.
#[allow(dead_code)] // public map API for Stage 3+ callers / diagnostics
pub fn is_remote_session(session_id: &str) -> bool {
    grant_for_session(session_id).is_some()
}

/// Snapshot of all live remote bindings as `(session_id, grant_id)`.
pub fn list_bindings() -> Vec<(String, String)> {
    let m = map().lock().unwrap_or_else(|e| e.into_inner());
    m.session_to_grant
        .iter()
        .map(|(s, g)| (s.clone(), g.clone()))
        .collect()
}

/// Close PTYs bound to `grant_id` via the v2 unregister path (same as
/// UI tab kill). Returns how many sessions were unbound (and attempted
/// kill), regardless of whether the PTY was already gone.
pub fn kill_sessions_for_grant(grant_id: &str) -> usize {
    let ids = sessions_for_grant(grant_id);
    let mut n = 0usize;
    for sid in ids {
        if kill_one(&sid) {
            n += 1;
        } else {
            // Map may still hold a stale binding — drop it.
            unbind_session(&sid);
            n += 1;
        }
    }
    n
}

/// Kill every remote-bound session (Layer 0 disable). Returns count.
pub fn kill_all_remote_sessions() -> usize {
    let ids: Vec<String> = {
        let m = map().lock().unwrap_or_else(|e| e.into_inner());
        m.session_to_grant.keys().cloned().collect()
    };
    let mut n = 0usize;
    for sid in ids {
        if kill_one(&sid) {
            n += 1;
        } else {
            unbind_session(&sid);
            n += 1;
        }
    }
    n
}

/// Unregister the v2 map entry for `session_id` (kills PTY) and unbind.
/// Returns true when a live map entry was found and removed.
fn kill_one(session_id: &str) -> bool {
    let Some(parsed) = SessionId::parse(session_id) else {
        unbind_session(session_id);
        return false;
    };
    // Find the agent/canonical key for this SessionId, then unregister
    // (unregister force-kills the child — same chokepoint as UI close).
    let key = crate::v2_session_map::snapshot()
        .into_iter()
        .find(|(_, s)| s.session_id == parsed)
        .map(|(k, _)| k);
    let found = if let Some(key) = key {
        crate::v2_session_map::unregister(&key);
        true
    } else {
        false
    };
    unbind_session(session_id);
    found
}

/// Test helper — clear all bindings (does not kill PTYs).
#[cfg(test)]
pub fn clear_for_tests() {
    let mut m = map().lock().unwrap_or_else(|e| e.into_inner());
    m.session_to_grant.clear();
    m.grant_to_sessions.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bind_unbind_and_indexes() {
        clear_for_tests();
        bind_session("s1", "g1");
        bind_session("s2", "g1");
        bind_session("s3", "g2");
        assert_eq!(grant_for_session("s1").as_deref(), Some("g1"));
        assert!(is_remote_session("s1"));
        assert!(!is_remote_session("nope"));
        let mut for_g1 = sessions_for_grant("g1");
        for_g1.sort();
        assert_eq!(for_g1, vec!["s1".to_string(), "s2".to_string()]);
        unbind_session("s1");
        assert!(!is_remote_session("s1"));
        assert_eq!(sessions_for_grant("g1"), vec!["s2".to_string()]);
        // kill with no live PTY still unbinds and counts.
        let n = kill_sessions_for_grant("g1");
        assert_eq!(n, 1);
        assert!(!is_remote_session("s2"));
        let n2 = kill_all_remote_sessions();
        assert_eq!(n2, 1); // s3
        assert!(list_bindings().is_empty());
    }
}
