//! Idle-reaper for API-created sandbox cells (replaces the `sleep 86400`
//! observability hack). Each cell registers a per-session idle TIMEOUT — the
//! API `timeout_secs` (default [`DEFAULT_TIMEOUT_SECS`]) — and the reaper tears
//! the cell down once it's been idle longer than that timeout.
//!
//! "Idle" = time since the last activity, where activity is: the spawn itself,
//! and any message delivered into the cell (message-live / resume). Killing the
//! session's PTY (`DaemonPtySession::kill`) fires the existing ChildExit path,
//! which runs the full sandbox cleanup (nft egress + per-session uid eviction) —
//! so the reaper never has to know about the jail internals.
//!
//! The timeout is the caller's knob (per `POST` request), NOT a magic number:
//! the sandbox is a job the caller launches, so the caller says how long it may
//! live. `register` clamps to a sane range; the reaper tick is env-overridable.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use k2_core::log_debug;
use k2_core::session::SessionId;
use tokio::task::JoinHandle;

/// Fallback idle timeout when a request doesn't set `timeout_secs`.
pub const DEFAULT_TIMEOUT_SECS: u64 = 180;
/// Clamp bounds so a caller can't pin a cell forever or reap it instantly.
const MIN_TIMEOUT_SECS: u64 = 30;
const MAX_TIMEOUT_SECS: u64 = 86_400;
/// Default reaper cadence (override with `K2_SANDBOX_REAPER_TICK_SECS`).
const TICK_SECS: u64 = 15;

struct Entry {
    last_activity: Instant,
    timeout: Duration,
}

static REG: LazyLock<Mutex<HashMap<SessionId, Entry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Normalize a requested timeout: clamp to `[MIN, MAX]`, default when 0/unset.
pub fn normalize_timeout(requested: Option<u64>) -> u64 {
    requested
        .filter(|&s| s > 0)
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
        .clamp(MIN_TIMEOUT_SECS, MAX_TIMEOUT_SECS)
}

/// Register (or re-arm) a sandbox cell with its idle timeout. Idempotent —
/// a resume of the same id resets the clock + timeout.
pub fn register(id: SessionId, timeout_secs: u64) {
    let secs = timeout_secs.clamp(MIN_TIMEOUT_SECS, MAX_TIMEOUT_SECS);
    if let Ok(mut m) = REG.lock() {
        m.insert(
            id,
            Entry {
                last_activity: Instant::now(),
                timeout: Duration::from_secs(secs),
            },
        );
    }
}

/// Reset the idle clock — activity delivered into the cell (message / resume).
pub fn stamp(id: &SessionId) {
    if let Ok(mut m) = REG.lock() {
        if let Some(e) = m.get_mut(id) {
            e.last_activity = Instant::now();
        }
    }
}

/// Is `id` currently tracked by the reaper? Observability accessor (the
/// host-sessions/sandbox integration suites assert a spawned session was
/// armed) — never consulted by the reap loop itself.
pub fn registered(id: &SessionId) -> bool {
    REG.lock().map(|m| m.contains_key(id)).unwrap_or(false)
}

/// Drop tracking (after teardown, so we never double-reap).
pub fn unregister(id: &SessionId) {
    if let Ok(mut m) = REG.lock() {
        m.remove(id);
    }
}

fn tick() -> Duration {
    let secs = std::env::var("K2_SANDBOX_REAPER_TICK_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|&s| s > 0)
        .unwrap_or(TICK_SECS);
    Duration::from_secs(secs)
}

/// Spawn the idle-reaper as a background tokio task. Mirrors
/// `session_reaper::spawn`'s shape.
pub fn spawn() -> JoinHandle<()> {
    tokio::spawn(async move { run().await })
}

async fn run() {
    let t = tick();
    log_debug!("[sandbox-reaper] started — tick={}s", t.as_secs());
    loop {
        tokio::time::sleep(t).await;
        // Snapshot the idle-expired ids under the lock, then act without it.
        let expired: Vec<SessionId> = {
            let Ok(m) = REG.lock() else { continue };
            let now = Instant::now();
            m.iter()
                .filter(|(_, e)| now.duration_since(e.last_activity) > e.timeout)
                .map(|(id, _)| *id)
                .collect()
        };
        for id in expired {
            if let Some(sess) = crate::v2_session_map::lookup_by_session_id(&id) {
                // kill() → ChildExit → nft/uid cleanup + v2_session_map unregister.
                sess.kill();
                log_debug!("[sandbox-reaper] reaped idle sandbox cell session={}", id);
            }
            unregister(&id);
        }
    }
}
