//! Garden data, daemon half (prd-gardens-local-services-v1 1A;
//! contract-garden-data-v1). Owner: B1.
//!
//! - [`store`]: the process-wide `SqliteStore` under `~/.k2/stores/`.
//! - [`start`]: the minute ticker (idle close, audit counts, budgets) and
//!   the hourly reaper / orphan pass.
//! - [`notify_changed`]: after every committed write to a `data:` space,
//!   emit `garden_data_changed {space, rev}` (coalesced per space, 100 ms,
//!   trailing edge always sent) and wake `changes?wait=` waiters.
//! - [`response`]: `DataError` → the `{"error": {...}}` envelope.
//!
//! Handlers live beside this file, one per owner (contract §3.1). The route
//! table and caller classifier are `crate::zen_data_routes`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use k2_core::garden_data::sqlite::SqliteStore;
use k2_core::garden_data::{DataError, StoreBackend};

use crate::cli_response::CliResponse;

pub mod agent_routes;
pub mod audit;
pub mod budget;
pub mod call;
pub mod export_routes;
pub mod grant_routes;
pub mod tx_routes;

/// `~/.k2/stores/` (0700): `gardens.sqlite` and `.snapshots/`.
pub fn stores_dir() -> PathBuf {
    k2_core::paths::k2_home().join("stores")
}

/// The process-wide Garden data store (opened lazily).
#[cfg(not(test))]
pub fn store() -> &'static SqliteStore {
    static STORE: OnceLock<SqliteStore> = OnceLock::new();
    STORE.get_or_init(|| SqliteStore::new(&stores_dir()))
}

/// Tests: one store per `$HOME` (each test runs under its own temp home).
#[cfg(test)]
pub fn store() -> &'static SqliteStore {
    static STORES: OnceLock<Mutex<HashMap<PathBuf, &'static SqliteStore>>> = OnceLock::new();
    let dir = stores_dir();
    let mut m = STORES.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap_or_else(|e| e.into_inner());
    m.entry(dir.clone()).or_insert_with(|| Box::leak(Box::new(SqliteStore::new(&dir))))
}

/// Seconds since the Unix epoch.
pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Start the minute ticker and the hourly reaper / orphan pass.
pub fn start() {
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
    }
    let spawned = std::thread::Builder::new().name("garden-data-ticker".into()).spawn(|| {
        let mut minutes = 0u64;
        loop {
            std::thread::sleep(Duration::from_secs(60));
            store().close_if_idle();
            audit::flush();
            budget::prune();
            if minutes.is_multiple_of(60) {
                hourly();
            }
            minutes += 1;
        }
    });
    if let Err(e) = spawned {
        k2_core::log_debug!("[garden-data] ticker didn't start: {e}");
    }
}

/// One reaper + orphan pass. Errors are logged (the next hour retries).
pub fn hourly() {
    if let Err(e) = store().reap(now()) {
        k2_core::log_debug!("[garden-data] reaper: {e}");
    }
    mark_orphans();
}

/// Mark spaces whose widget folder is gone (and clear the mark when it's
/// back). Skipped when Zen isn't set up on this computer.
pub fn mark_orphans() {
    let files = k2_core::zen::ZenFiles::local();
    if !files.is_set_up() {
        return;
    }
    let live = files.widget_names();
    if let Err(e) = store().mark_orphans(&live, now()) {
        k2_core::log_debug!("[garden-data] orphan pass: {e}");
    }
}

// ── change push and long-poll waiters ─────────────────────────────────

/// Per `data:` space: the rev of the last committed write this daemon saw,
/// and a generation counter that goes up on every notify. Waiters compare
/// generations, not revs, so a deleted and re-created space (its rev starts
/// over) still wakes them correctly.
struct Revs {
    latest: Mutex<HashMap<String, (i64, u64)>>,
    moved: Condvar,
}

fn revs() -> &'static Revs {
    static R: OnceLock<Revs> = OnceLock::new();
    R.get_or_init(|| Revs { latest: Mutex::new(HashMap::new()), moved: Condvar::new() })
}

/// Per space: when the last frame went out, and whether a trailing frame
/// is already scheduled.
fn emits() -> &'static Mutex<HashMap<String, (Instant, bool)>> {
    static E: OnceLock<Mutex<HashMap<String, (Instant, bool)>>> = OnceLock::new();
    E.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Frames per space are at most one per this.
pub const COALESCE: Duration = Duration::from_millis(100);

fn emit_frame(space: &str) {
    let rev = revs().latest.lock().unwrap_or_else(|e| e.into_inner()).get(space).map_or(0, |(r, _)| *r);
    let _ = crate::session_events::emit(crate::session_events::SessionEvent::GardenDataChanged {
        space: space.to_string(),
        rev,
    });
}

/// A committed write to `space` reached `rev`: wake waiters and push one
/// coalesced `garden_data_changed`. Never call it for `save:` spaces.
pub fn notify_changed(space: &str, rev: i64) {
    if !space.starts_with("data:") {
        return;
    }
    {
        let r = revs();
        let mut latest = r.latest.lock().unwrap_or_else(|e| e.into_inner());
        let e = latest.entry(space.to_string()).or_insert((0, 0));
        *e = (rev, e.1 + 1);
        r.moved.notify_all();
    }
    let now = Instant::now();
    let mut em = emits().lock().unwrap_or_else(|e| e.into_inner());
    match em.get_mut(space) {
        Some((_, true)) => {} // a trailing frame is coming and will carry the newest rev
        Some((last, sched)) if now.duration_since(*last) < COALESCE => {
            *sched = true;
            let wait = COALESCE - now.duration_since(*last);
            let space = space.to_string();
            std::thread::spawn(move || {
                std::thread::sleep(wait);
                {
                    let mut em = emits().lock().unwrap_or_else(|e| e.into_inner());
                    em.insert(space.clone(), (Instant::now(), false));
                }
                emit_frame(&space);
            });
        }
        _ => {
            em.insert(space.to_string(), (now, false));
            drop(em);
            emit_frame(space);
        }
    }
}

/// The space's notify generation now. Read it BEFORE reading the store, then
/// [`wait_for_change`] with it, so a write in between is never missed.
pub fn generation(space: &str) -> u64 {
    revs().latest.lock().unwrap_or_else(|e| e.into_inner()).get(space).map_or(0, |(_, g)| *g)
}

/// Block up to `wait_secs` until `space` is notified after generation
/// `seen` (the `changes?wait=` long-poll). Answers true when it was.
pub fn wait_for_change(space: &str, seen: u64, wait_secs: u32) -> bool {
    if wait_secs == 0 {
        return false;
    }
    let r = revs();
    let deadline = Instant::now() + Duration::from_secs(u64::from(wait_secs));
    let mut latest = r.latest.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if latest.get(space).is_some_and(|(_, g)| *g != seen) {
            return true;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return false;
        }
        latest = r.moved.wait_timeout(latest, left).unwrap_or_else(|e| e.into_inner()).0;
    }
}

// ── responses ─────────────────────────────────────────────────────────

pub(crate) fn status_line(code: u16) -> &'static str {
    match code {
        200 => "200 OK",
        400 => "400 Bad Request",
        401 => "401 Unauthorized",
        403 => "403 Forbidden",
        404 => "404 Not Found",
        405 => "405 Method Not Allowed",
        409 => "409 Conflict",
        410 => "410 Gone",
        413 => "413 Payload Too Large",
        423 => "423 Locked",
        429 => "429 Too Many Requests",
        501 => "501 Not Implemented",
        503 => "503 Service Unavailable",
        507 => "507 Insufficient Storage",
        _ => "500 Internal Server Error",
    }
}

/// The error envelope for `e` (contract §4.6).
pub fn response(e: &DataError) -> CliResponse {
    CliResponse { status: status_line(e.status()), content_type: "application/json", body: e.envelope().to_string() }
}

/// A fixed refusal (`apps_not_allowed`, `zen_local_only`, `owner_only`, …).
pub fn refused(code: &'static str, status: u16, hint: impl Into<String>) -> DataError {
    DataError::Refused { code, status, hint: hint.into() }
}

/// 200 with a JSON body.
pub fn ok(v: serde_json::Value) -> CliResponse {
    CliResponse::ok_json(v.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waiters_wake_on_a_newer_rev_and_time_out_otherwise() {
        let space = format!("data:wait-{}", uuid::Uuid::new_v4().simple());
        let g0 = generation(&space);
        assert!(!wait_for_change(&space, g0, 0));
        let s2 = space.clone();
        let t = std::thread::spawn(move || wait_for_change(&s2, g0, 5));
        std::thread::sleep(Duration::from_millis(50));
        notify_changed(&space, 3);
        assert!(t.join().unwrap(), "woken by the notify");
        assert!(wait_for_change(&space, g0, 1), "already notified since g0");
        let g1 = generation(&space);
        let started = Instant::now();
        assert!(!wait_for_change(&space, g1, 1), "nothing since g1");
        assert!(started.elapsed() >= Duration::from_millis(900));
        // A re-created space starts its rev over and still wakes waiters.
        let s3 = space.clone();
        let t = std::thread::spawn(move || wait_for_change(&s3, g1, 5));
        std::thread::sleep(Duration::from_millis(50));
        notify_changed(&space, 1);
        assert!(t.join().unwrap());
        // save: spaces never push.
        let gs = generation("save:x");
        notify_changed("save:x", 9);
        assert_eq!(generation("save:x"), gs);
    }
}
