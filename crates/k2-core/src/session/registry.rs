//! `SessionRegistry` — in-process map of `SessionId → SessionEntry`.
//!
//! Mirrors the singleton pattern used across k2so-core
//! (`db::shared()`, `terminal::shared()`, `llm::shared()`). One
//! registry per process — the daemon and Tauri (when running in
//! the same process during dev) share the same map via
//! `registry::shared()`. Cross-process sync is a Phase 4 concern.
//!
//! Producer path (Phase 2 D3): `session_stream_pty` reader thread
//! calls `register(id)` on session spawn, publishes frames via the
//! returned `Arc<SessionEntry>`, then `unregister(id)` on session
//! exit.
//!
//! Consumer path (Phase 2 D4): the daemon's WS subscribe handler
//! looks up by id, snapshots the replay ring, then subscribes to
//! the live broadcast channel.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;

use crate::session::entry::SessionEntry;
use crate::session::SessionId;

type Registry = Arc<Mutex<HashMap<SessionId, Arc<SessionEntry>>>>;

static REGISTRY: OnceLock<Registry> = OnceLock::new();

/// Shared registry. Initialized lazily on first call.
pub fn shared() -> Registry {
    REGISTRY
        .get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
        .clone()
}

/// Register a fresh `SessionEntry` for the given id. Returns an
/// `Arc<SessionEntry>` the producer holds for the session's
/// lifetime. If an entry already exists for `id`, it's replaced —
/// old subscribers get a dropped-sender and exit their loops.
pub fn register(id: SessionId) -> Arc<SessionEntry> {
    let entry = Arc::new(SessionEntry::new());
    shared().lock().insert(id, entry.clone());
    entry
}

/// Register with a custom replay cap. Mostly useful for tests.
pub fn register_with_cap(id: SessionId, replay_cap: usize) -> Arc<SessionEntry> {
    let entry = Arc::new(SessionEntry::with_replay_cap(replay_cap));
    shared().lock().insert(id, entry.clone());
    entry
}

/// Lookup without mutating the registry. Returns `None` if the
/// session isn't registered (crashed, never existed, already
/// unregistered).
pub fn lookup(id: &SessionId) -> Option<Arc<SessionEntry>> {
    shared().lock().get(id).cloned()
}

/// Unregister a session. Holders of the `Arc<SessionEntry>` keep
/// working until they drop it — subscribers get natural
/// `RecvError::Closed` once the last sender is dropped.
pub fn unregister(id: &SessionId) -> Option<Arc<SessionEntry>> {
    shared().lock().remove(id)
}

/// Snapshot of all registered session ids. Ordering is
/// unspecified; call sites that need sorting sort it themselves.
pub fn list_ids() -> Vec<SessionId> {
    shared().lock().keys().copied().collect()
}

/// Number of registered sessions.
pub fn len() -> usize {
    shared().lock().len()
}

/// Test helper: drop every registered entry. Keeps tests that
/// share the global registry from contaminating each other.
#[cfg(any(test, feature = "test-util"))]
pub fn clear_for_tests() {
    shared().lock().clear();
}
