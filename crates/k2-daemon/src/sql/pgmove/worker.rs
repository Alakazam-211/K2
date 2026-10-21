//! One move at a time, the failure latch and the boot-recovery gate (§6
//! "Routes", §13 row 8, the hostmail-upgrade shape, V8; security review #2
//! F6).
//!
//! - The latch is persisted: it is the newest `sql_moves` row in state
//!   `failed` whose `last_error` doesn't start with [`ACK_PREFIX`];
//!   [`load_latch`] reads it at boot and [`acknowledge`] writes the prefix.
//! - While boot recovery runs ([`recovering`]), no new move, rollback or
//!   forget-old starts. Recovery is only on when the last daemon left a
//!   move open ([`set_pending`]); it waits for Postgres (the route answers
//!   `postgres_stopped` meanwhile, never "recovering"), and once Postgres
//!   runs it has [`RECOVERY_TIMEOUT_SECS`] to finish before
//!   [`expire_recovery`] ends it and latches a failure for the owner
//!   (smoke r6: a stopped sidecar kept every move "recovering" forever).

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Mutex;

use super::{MoveProgress, MoveState};

/// What `--acknowledge-failed` writes in front of a failed row's error.
pub const ACK_PREFIX: &str = "acknowledged: ";

#[derive(Debug, Default)]
struct Worker {
    busy: Option<String>,
    failed: Option<String>,
}

static W: Mutex<Worker> = Mutex::new(Worker { busy: None, failed: None });
static RECOVERING: AtomicBool = AtomicBool::new(false);
/// When boot recovery started its work (Postgres up); 0 while it waits.
static RECOVERY_STARTED: AtomicI64 = AtomicI64::new(0);
/// The databases with a move the last daemon left open.
static PENDING: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// How long boot recovery may work once Postgres runs before it is ended
/// and latched as a failure (the owner looks; the open rows stay, and the
/// next boot tries them again).
pub const RECOVERY_TIMEOUT_SECS: i64 = 30 * 60;

fn lock() -> std::sync::MutexGuard<'static, Worker> {
    W.lock().unwrap_or_else(|p| p.into_inner())
}

/// Held while a move (or rollback, or forget-old) runs.
#[derive(Debug)]
pub struct Busy(());

impl Drop for Busy {
    fn drop(&mut self) {
        lock().busy = None;
    }
}

pub fn recovering() -> bool {
    RECOVERING.load(Ordering::SeqCst)
}

pub fn set_recovering(on: bool) {
    RECOVERING.store(on, Ordering::SeqCst);
    if !on {
        RECOVERY_STARTED.store(0, Ordering::SeqCst);
        pending_lock().clear();
    }
}

fn pending_lock() -> std::sync::MutexGuard<'static, Vec<String>> {
    PENDING.lock().unwrap_or_else(|p| p.into_inner())
}

/// The databases boot recovery has to finish or undo.
pub fn set_pending(dbs: Vec<String>) {
    *pending_lock() = dbs;
}

pub fn pending() -> Vec<String> {
    pending_lock().clone()
}

/// Postgres is up: recovery's work (and its clock) starts now.
pub fn recovery_started(now: i64) {
    RECOVERY_STARTED.store(now.max(1), Ordering::SeqCst);
}

/// When recovery started working; `None` while it waits for Postgres (or
/// isn't on).
pub fn recovery_running_since() -> Option<i64> {
    let at = RECOVERY_STARTED.load(Ordering::SeqCst);
    (recovering() && at > 0).then_some(at)
}

/// Ends a recovery that has worked past [`RECOVERY_TIMEOUT_SECS`]: moves
/// then answer `failed_latched` (the owner looks and acknowledges)
/// instead of "recovering". The latch text, when it ended one.
pub fn expire_recovery(now: i64) -> Option<String> {
    let since = recovery_running_since()?;
    if now - since < RECOVERY_TIMEOUT_SECS {
        return None;
    }
    let dbs = pending().join(", ");
    let msg = format!(
        "boot recovery of {} did not finish within {} minutes; see the daemon log ([db] upgrade) and `k2 db upgrade <db> --check`, \
         then `k2 db upgrade --acknowledge-failed` (restarting the daemon tries the recovery again)",
        if dbs.is_empty() { "a move" } else { &dbs },
        RECOVERY_TIMEOUT_SECS / 60
    );
    recovery_failed(&msg);
    Some(msg)
}

/// Boot recovery could not finish (a panic, or [`expire_recovery`]): end
/// the gate and latch the failure, so moves stop saying "try again in a
/// minute" and wait for the owner instead.
pub fn recovery_failed(why: &str) {
    lock().failed = Some(why.to_string());
    set_recovering(false);
}

/// The route's 409 code for a refusal from [`begin`].
pub fn refusal_code(e: &str) -> &'static str {
    if e.starts_with("busy") {
        "upgrade_in_progress"
    } else if e.starts_with("recovering") {
        "recovering"
    } else {
        "failed_latched"
    }
}

fn take(db: &str, during_recovery: bool) -> Result<Busy, String> {
    let mut w = lock();
    if !during_recovery && recovering() {
        return Err("recovering: K2 is finishing or undoing a move the last daemon was in".into());
    }
    if let Some(f) = &w.failed {
        if !during_recovery {
            return Err(format!(
                "failed_latched: an earlier move failed and needs the owner ({f}); `k2 db upgrade --acknowledge-failed` after fixing it"
            ));
        }
    }
    if let Some(b) = &w.busy {
        return Err(format!("busy: a move of {b} is running"));
    }
    w.busy = Some(db.to_string());
    Ok(Busy(()))
}

/// Start work on `db`: refused while another runs, while boot recovery
/// runs, or while a failure is latched.
pub fn begin(db: &str) -> Result<Busy, String> {
    take(db, false)
}

/// Boot recovery's own claim (it runs past the latch: an interrupted move
/// must be finished or undone either way).
pub fn begin_recovery(db: &str) -> Result<Busy, String> {
    take(db, true)
}

pub fn busy() -> Option<String> {
    lock().busy.clone()
}

/// Record a finished move's outcome (a `failed` one latches).
pub fn settle(p: &MoveProgress) {
    if p.state == MoveState::Failed {
        lock().failed = Some(format!("{}: {}", p.db, p.last_error.clone().unwrap_or_default()));
    }
}

pub fn failed() -> Option<String> {
    lock().failed.clone()
}

/// Boot: the newest unacknowledged failed row latches again.
pub fn load_latch(conn: &rusqlite::Connection) {
    let row: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT db, last_error FROM sql_moves WHERE state = 'failed' \
             AND (last_error IS NULL OR substr(last_error, 1, ?1) <> ?2) ORDER BY started_at DESC LIMIT 1",
            rusqlite::params![ACK_PREFIX.len() as i64, ACK_PREFIX],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    if let Some((db, e)) = row {
        lock().failed = Some(format!("{db}: {}", e.unwrap_or_default()));
    }
}

/// Owner only (the gate checks): clear the latch, in memory and on every
/// failed row. `true` when one was set.
pub fn acknowledge() -> bool {
    let was = lock().failed.take().is_some();
    let k2db = k2_core::db::shared();
    let conn = k2db.lock();
    let n = conn
        .execute(
            "UPDATE sql_moves SET last_error = ?1 || coalesce(last_error, '') WHERE state = 'failed' \
             AND (last_error IS NULL OR substr(last_error, 1, ?2) <> ?1)",
            rusqlite::params![ACK_PREFIX, ACK_PREFIX.len() as i64],
        )
        .unwrap_or(0);
    was || n > 0
}

#[cfg(test)]
pub(crate) fn reset_for_tests() {
    *lock() = Worker::default();
    set_recovering(false);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed_progress(db: &str) -> MoveProgress {
        serde_json::from_value(serde_json::json!({
            "id": "m", "db": db, "from": 16, "to": 17, "tag": "20261020T101500Z", "actor": "o",
            "state": "failed", "step": "switch", "startedAt": 0, "finishedAt": 1, "lastError": "reversal failed",
            "fromPort": 5432, "toPort": 5433, "targetCreated": true, "quiesced": false, "stoppedApps": [],
            "doorsPaused": false, "repointed": ["vault"], "rolesCreated": [], "dumpBytes": 1, "diff": [], "warnings": [], "check": []
        }))
        .unwrap()
    }

    #[test]
    fn one_at_a_time_the_latch_and_recovery() {
        let _home = crate::test_support::TempHome::new();
        let _g = crate::sql::sql_server_test_lock();
        k2_core::db::init_for_tests();
        reset_for_tests();
        let b = begin("ws_a").unwrap();
        assert_eq!(busy().as_deref(), Some("ws_a"));
        assert!(begin("ws_b").unwrap_err().starts_with("busy: a move of ws_a"));
        drop(b);
        assert!(busy().is_none());
        let mut p = failed_progress("ws_a");
        settle(&p);
        let e = begin("ws_b").unwrap_err();
        assert!(e.starts_with("failed_latched"));
        assert_eq!(refusal_code(&e), "failed_latched");
        assert!(acknowledge());
        assert!(!acknowledge());
        p.state = MoveState::Aborted;
        settle(&p);
        drop(begin("ws_b").unwrap());
        // Boot recovery blocks new work, but not its own.
        set_recovering(true);
        let e = begin("ws_b").unwrap_err();
        assert_eq!(refusal_code(&e), "recovering");
        drop(begin_recovery("ws_b").unwrap());
        set_recovering(false);
        reset_for_tests();
    }

    /// F6: the latch survives a restart (it lives in sql_moves) until the
    /// owner acknowledges it.
    #[test]
    fn the_latch_is_persisted() {
        let _home = crate::test_support::TempHome::new();
        let _g = crate::sql::sql_server_test_lock();
        k2_core::db::init_for_tests();
        reset_for_tests();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO sql_moves (id, database_id, db, from_major, to_major, state, started_at, last_error) \
                 VALUES ('m1', 'd1', 'ws_a', 16, 17, 'failed', 5, 'reversal failed')",
                [],
            )
            .unwrap();
            load_latch(&conn);
        }
        assert!(failed().is_some_and(|f| f.contains("reversal failed")));
        assert!(begin("ws_b").unwrap_err().starts_with("failed_latched"));
        assert!(acknowledge());
        // A restart after the acknowledgement: no latch.
        reset_for_tests();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            load_latch(&conn);
            let e: String = conn.query_row("SELECT last_error FROM sql_moves WHERE id = 'm1'", [], |r| r.get(0)).unwrap();
            assert_eq!(e, "acknowledged: reversal failed");
        }
        assert!(failed().is_none());
        drop(begin("ws_b").unwrap());
        reset_for_tests();
    }

    /// Smoke r6: recovery ends. While it waits for Postgres it has no
    /// clock; once it works it has RECOVERY_TIMEOUT_SECS, then the gate
    /// turns into the owner's failure latch (never "recovering" forever).
    #[test]
    fn a_recovery_past_its_limit_ends_in_the_latch() {
        let _home = crate::test_support::TempHome::new();
        let _g = crate::sql::sql_server_test_lock();
        k2_core::db::init_for_tests();
        reset_for_tests();
        set_pending(vec!["ws_a".into()]);
        set_recovering(true);
        // Waiting for Postgres: no clock, nothing expires.
        assert_eq!(recovery_running_since(), None);
        assert_eq!(expire_recovery(i64::MAX / 2), None);
        assert!(recovering());
        // Postgres up at t=1000: inside the limit it keeps going.
        recovery_started(1000);
        assert_eq!(recovery_running_since(), Some(1000));
        assert_eq!(expire_recovery(1000 + RECOVERY_TIMEOUT_SECS - 1), None);
        assert!(recovering());
        assert_eq!(refusal_code(&begin("ws_b").unwrap_err()), "recovering");
        // Past it: recovery ends, the latch names the database.
        let msg = expire_recovery(1000 + RECOVERY_TIMEOUT_SECS).expect("expired");
        assert!(msg.contains("ws_a") && msg.contains("--acknowledge-failed"), "{msg}");
        assert!(!recovering());
        assert!(pending().is_empty());
        assert_eq!(recovery_running_since(), None);
        let e = begin("ws_b").unwrap_err();
        assert_eq!(refusal_code(&e), "failed_latched", "{e}");
        assert!(e.contains("boot recovery of ws_a"), "{e}");
        // The owner acknowledges: moves may start again.
        acknowledge();
        drop(begin("ws_b").unwrap());
        // A panicked recovery ends the same way.
        set_recovering(true);
        recovery_failed("boot recovery panicked");
        assert!(!recovering());
        assert_eq!(refusal_code(&begin("ws_b").unwrap_err()), "failed_latched");
        reset_for_tests();
    }
}
