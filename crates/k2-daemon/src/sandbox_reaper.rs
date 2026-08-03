//! Work-completion-aware reaper for API host-sessions / sandbox cells.
//!
//! ## Model (product lock 2026-08-03 — Scout / Julie / Rosson)
//!
//! Persistent-interview cells must survive user think-time and long mid-write
//! turns. A spawn-time spend-cap (`timeout_secs` as hard wall) is incompatible
//! with that shape.
//!
//! - **Working** — inject / register / non-final `k2 respond` → **never**
//!   auto-reaped (no silence reap, no `timeout_secs` wall from spawn).
//!   Continuous productive work may run past 300s+.
//! - **Grace** — after `k2 respond --final`, short window
//!   ([`FINAL_GRACE_SECS`] = 10s) then reap (work completed).
//! - **New activity** (inject / live-resume / non-final respond) cancels Grace
//!   and re-enters Working (resets the completion path).
//! - **`timeout_secs`** — still accepted on spawn (JWT lifetime clamp, client
//!   poll budgets); it does **not** kill a Working cell.
//! - **Spend control** — integrator **kill** + capability non-remint / caps,
//!   not mid-write wall.
//!
//! Activity stamped by spawn / message-live / resume inject (re-enters Working).

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use k2_core::log_debug;
use k2_core::session::SessionId;
use tokio::task::JoinHandle;

/// Fallback idle timeout when a request doesn't set `timeout_secs`.
/// (JWT / client budget default; not a Working hard wall.)
pub const DEFAULT_TIMEOUT_SECS: u64 = 180;
const MIN_TIMEOUT_SECS: u64 = 30;
const MAX_TIMEOUT_SECS: u64 = 86_400;
const TICK_SECS: u64 = 15;
/// Grace after `--final` before the cell may be reaped as idle-complete.
pub const FINAL_GRACE_SECS: u64 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Agent is generating / mid-turn. Never auto-reaped.
    Working,
    /// After final respond; reap once grace elapses.
    Grace,
}

struct Entry {
    last_activity: Instant,
    /// Spawn/register instant (observability; not a kill clock).
    #[allow(dead_code)]
    registered_at: Instant,
    /// Requested `timeout_secs` (JWT/client budget; not a Working kill).
    #[allow(dead_code)]
    timeout: Duration,
    phase: Phase,
    /// When phase == Grace, reap after this instant (unless re-Working).
    grace_until: Option<Instant>,
}

static REG: LazyLock<Mutex<HashMap<SessionId, Entry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn normalize_timeout(requested: Option<u64>) -> u64 {
    requested
        .filter(|&s| s > 0)
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
        .clamp(MIN_TIMEOUT_SECS, MAX_TIMEOUT_SECS)
}

/// Register or re-arm. Spawn / dead-resume start **Working** (agent about to run).
pub fn register(id: SessionId, timeout_secs: u64) {
    let secs = timeout_secs.clamp(MIN_TIMEOUT_SECS, MAX_TIMEOUT_SECS);
    let now = Instant::now();
    if let Ok(mut m) = REG.lock() {
        m.insert(
            id,
            Entry {
                last_activity: now,
                registered_at: now,
                timeout: Duration::from_secs(secs),
                phase: Phase::Working,
                grace_until: None,
            },
        );
    }
}

/// Inject / live-resume: stamp activity and mark **Working** (cancels grace).
pub fn stamp(id: &SessionId) {
    mark_working(id);
}

/// Explicit Working (same as stamp; named for call sites).
pub fn mark_working(id: &SessionId) {
    if let Ok(mut m) = REG.lock() {
        if let Some(e) = m.get_mut(id) {
            e.last_activity = Instant::now();
            e.phase = Phase::Working;
            e.grace_until = None;
        }
    }
}

/// Non-final `k2 respond` — stay Working, refresh activity.
pub fn on_respond(id: &SessionId, final_: bool) {
    if final_ {
        on_respond_final(id);
    } else {
        mark_working(id);
    }
}

/// `k2 respond --final` → enter Grace; may reap after FINAL_GRACE_SECS.
pub fn on_respond_final(id: &SessionId) {
    if let Ok(mut m) = REG.lock() {
        if let Some(e) = m.get_mut(id) {
            let now = Instant::now();
            e.last_activity = now;
            e.phase = Phase::Grace;
            e.grace_until = Some(now + Duration::from_secs(FINAL_GRACE_SECS));
        }
    }
}

#[allow(dead_code)]
pub fn registered(id: &SessionId) -> bool {
    REG.lock().map(|m| m.contains_key(id)).unwrap_or(false)
}

/// Test/observability: is the cell in Working phase?
#[cfg(test)]
pub fn is_working(id: &SessionId) -> bool {
    REG.lock()
        .ok()
        .and_then(|m| m.get(id).map(|e| e.phase == Phase::Working))
        .unwrap_or(false)
}

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

fn should_reap(e: &Entry, now: Instant) -> bool {
    // Working is never auto-reaped (no short hard wall mid-write; no pure
    // idle kill while a turn is open). Grace only.
    match e.phase {
        Phase::Working => false,
        Phase::Grace => e.grace_until.map(|u| now >= u).unwrap_or(false),
    }
}

pub fn spawn() -> JoinHandle<()> {
    tokio::spawn(async move { run().await })
}

async fn run() {
    let t = tick();
    log_debug!(
        "[sandbox-reaper] started — tick={}s (work-completion gate)",
        t.as_secs()
    );
    loop {
        tokio::time::sleep(t).await;
        // Scout 0.40.78: drop map entries whose child is already dead so
        // host-sessions list cannot report phantom live:true (restart /
        // KillMode / missed ChildExit). Map is empty right after boot; this
        // catches mid-lifetime zombies.
        crate::v2_session_map::reconcile_dead_children();
        let expired: Vec<SessionId> = {
            let Ok(m) = REG.lock() else { continue };
            let now = Instant::now();
            m.iter()
                .filter(|(_, e)| should_reap(e, now))
                .map(|(id, _)| *id)
                .collect()
        };
        for id in expired {
            if let Some(sess) = crate::v2_session_map::lookup_by_session_id(&id) {
                sess.kill();
                log_debug!("[sandbox-reaper] reaped sandbox/host cell session={}", id);
            }
            unregister(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn working_survives_idle_window() {
        let id = SessionId::new();
        register(id, 30);
        assert!(is_working(&id));
        let now = Instant::now();
        let entry = Entry {
            last_activity: now - Duration::from_secs(120),
            registered_at: now,
            timeout: Duration::from_secs(180),
            phase: Phase::Working,
            grace_until: None,
        };
        assert!(!should_reap(&entry, now), "Working must not idle-reap");
        unregister(&id);
    }

    #[test]
    fn grace_reaps_after_deadline() {
        let now = Instant::now();
        let entry = Entry {
            last_activity: now,
            registered_at: now,
            timeout: Duration::from_secs(180),
            phase: Phase::Grace,
            grace_until: Some(now - Duration::from_secs(1)),
        };
        assert!(should_reap(&entry, now));
    }

    #[test]
    fn working_survives_past_timeout_secs_wall() {
        // Scout E-1: continuous work at timeout_secs=300 must not die
        // solely because registered_at + wall elapsed (plan b95c7409).
        let now = Instant::now();
        let entry = Entry {
            last_activity: now,
            registered_at: now - Duration::from_secs(400),
            timeout: Duration::from_secs(300),
            phase: Phase::Working,
            grace_until: None,
        };
        assert!(
            !should_reap(&entry, now),
            "Working must not be reaped by timeout_secs hard wall"
        );
    }

    #[test]
    fn working_survives_long_mid_write_silence() {
        // E-1 shape: inject once, then write continuously for > timeout without
        // new API activity stamps — still Working → still alive.
        let now = Instant::now();
        let entry = Entry {
            last_activity: now - Duration::from_secs(400),
            registered_at: now - Duration::from_secs(400),
            timeout: Duration::from_secs(300),
            phase: Phase::Working,
            grace_until: None,
        };
        assert!(
            !should_reap(&entry, now),
            "mid-write silence must not kill Working"
        );
    }

    #[test]
    fn grace_reaps_even_if_wall_not_reached() {
        let now = Instant::now();
        let entry = Entry {
            last_activity: now,
            registered_at: now,
            timeout: Duration::from_secs(86_400),
            phase: Phase::Grace,
            grace_until: Some(now - Duration::from_secs(1)),
        };
        assert!(should_reap(&entry, now), "Grace after --final still reaps");
    }

    #[test]
    fn final_enters_grace() {
        let id = SessionId::new();
        register(id, 180);
        on_respond_final(&id);
        let e = REG.lock().unwrap();
        let ent = e.get(&id).unwrap();
        assert_eq!(ent.phase, Phase::Grace);
        assert!(ent.grace_until.is_some());
        drop(e);
        unregister(&id);
    }

    #[test]
    fn inject_cancels_grace() {
        let id = SessionId::new();
        register(id, 180);
        on_respond_final(&id);
        stamp(&id);
        assert!(is_working(&id));
        unregister(&id);
    }

    #[test]
    fn non_final_respond_stays_working() {
        let id = SessionId::new();
        register(id, 180);
        on_respond_final(&id);
        on_respond(&id, false);
        assert!(is_working(&id));
        unregister(&id);
    }
}
