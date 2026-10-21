//! One move at a time, and the failure latch (§6 "Routes", §13 row 8,
//! the hostmail-upgrade shape, V8). The latch is set from the persisted
//! move rows at boot as well (`MoveState::Failed` not acknowledged).

use std::sync::Mutex;

use super::{MoveProgress, MoveState};

#[derive(Debug, Default)]
struct Worker {
    busy: Option<String>,
    failed: Option<String>,
}

static W: Mutex<Worker> = Mutex::new(Worker { busy: None, failed: None });

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

/// Start work on `db`: refused while another runs or a failure is latched.
pub fn begin(db: &str) -> Result<Busy, String> {
    let mut w = lock();
    if let Some(f) = &w.failed {
        return Err(format!("failed_latched: an earlier move failed and needs the owner ({f}); `k2 db upgrade --acknowledge-failed` after fixing it"));
    }
    if let Some(b) = &w.busy {
        return Err(format!("busy: a move of {b} is running"));
    }
    w.busy = Some(db.to_string());
    Ok(Busy(()))
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

/// Owner only (the gate checks): clear the latch. `true` when one was set.
pub fn acknowledge() -> bool {
    lock().failed.take().is_some()
}

#[cfg(test)]
pub(crate) fn reset_for_tests() {
    *lock() = Worker::default();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_at_a_time_and_the_latch() {
        let _g = crate::sql::sql_server_test_lock();
        reset_for_tests();
        let b = begin("ws_a").unwrap();
        assert_eq!(busy().as_deref(), Some("ws_a"));
        assert!(begin("ws_b").unwrap_err().starts_with("busy: a move of ws_a"));
        drop(b);
        assert!(busy().is_none());
        let mut p: MoveProgress = serde_json::from_value(serde_json::json!({
            "id": "m", "db": "ws_a", "from": 16, "to": 17, "tag": "20261020T101500Z", "actor": "o",
            "state": "failed", "step": "switch", "startedAt": 0, "finishedAt": 1, "lastError": "reversal failed",
            "fromPort": 5432, "toPort": 5433, "targetCreated": true, "quiesced": false, "stoppedApps": [],
            "doorsPaused": false, "repointed": ["vault"], "rolesCreated": [], "dumpBytes": 1, "diff": [], "warnings": [], "check": []
        }))
        .unwrap();
        settle(&p);
        assert!(begin("ws_b").unwrap_err().starts_with("failed_latched"));
        assert!(acknowledge());
        assert!(!acknowledge());
        p.state = MoveState::Aborted;
        settle(&p);
        drop(begin("ws_b").unwrap());
        reset_for_tests();
    }
}
