//! The delivery lanes' journal, daemon side (0.46.9, migration 0267).
//!
//! [`crate::delivery_queue`] runs a Thread or project chat delivery after
//! the post has answered, in memory. Each one is written to SQLite
//! (`k2_core::delivery_journal`) when it is queued, marked when its lane
//! starts it and removed when it is done, so a daemon restart doesn't lose
//! it. At boot ([`replay_on_boot`], after the surviving sessions are
//! adopted and the daemon is ready):
//!
//! - a row that never started is queued again on its lane, in the order
//!   it was first queued (a Thread line to `overlay_routes`, a project chat
//!   line to `project_group_routes`);
//! - a row that started but never finished may already be in the agent's
//!   terminal, so it is never typed in again: the Thread gets a notice
//!   that it may not have reached the agent (a project chat line, a log
//!   line);
//! - a row older than [`REPLAY_MAX_AGE_MS`] is not sent so late: the
//!   Thread gets a notice instead.
//!
//! A journal write that fails is logged and the delivery still runs (in
//! memory, as before 0.46.9).

use k2_core::delivery_journal::{self as store, JournalRow, KIND_PROJECT, KIND_THREAD};

/// A queued row older than this at boot is not delivered (1 hour).
pub(crate) const REPLAY_MAX_AGE_MS: i64 = 60 * 60_000;

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

type Db = std::sync::Arc<parking_lot::ReentrantMutex<rusqlite::Connection>>;

/// One delivery's journal row (or none, when the write failed). It keeps
/// the database it was written to: the lane runs on another thread.
#[derive(Clone, Default)]
pub(crate) struct Entry {
    row: Option<(Db, String)>,
}

impl Entry {
    /// A row that is already in the journal (a boot replay).
    pub(crate) fn existing(id: &str) -> Self {
        Self { row: k2_core::db::try_shared().map(|db| (db, id.to_string())) }
    }

    /// The lane picked it up.
    pub(crate) fn started(&self) {
        self.with_row("mark started", |conn, id| store::mark_started(conn, id, now_ms()).map(|_| ()));
    }

    /// It is done (delivered or not): the row goes.
    pub(crate) fn finish(&self) {
        self.with_row("finish", |conn, id| store::finish(conn, id).map(|_| ()));
    }

    fn with_row(&self, what: &str, f: impl FnOnce(&rusqlite::Connection, &str) -> rusqlite::Result<()>) {
        let Some((db, id)) = self.row.as_ref() else { return };
        let conn = db.lock();
        if let Err(e) = f(&conn, id) {
            k2_core::log_debug!("[delivery] journal {what} failed for {id}: {e}");
        }
    }
}

/// Write a queued delivery of `kind` on `lane`. `body` is the job as JSON.
pub(crate) fn record(lane: &str, kind: &str, body: &impl serde::Serialize) -> Entry {
    let body = match serde_json::to_string(body) {
        Ok(b) => b,
        Err(e) => {
            k2_core::log_debug!("[delivery] journal: can't write a {kind} delivery on {lane} ({e}); in memory only");
            return Entry::default();
        }
    };
    let Some(db) = k2_core::db::try_shared() else {
        return Entry::default();
    };
    let id = uuid::Uuid::new_v4().to_string();
    let written = {
        let conn = db.lock();
        store::record(&conn, &id, lane, kind, &body, now_ms())
    };
    match written {
        Ok(()) => Entry { row: Some((db, id)) },
        Err(e) => {
            k2_core::log_debug!("[delivery] journal: can't record a {kind} delivery on {lane} ({e}); in memory only");
            Entry::default()
        }
    }
}

/// What boot did with the journal.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ReplaySummary {
    pub requeued: usize,
    pub interrupted: usize,
    pub too_old: usize,
    pub unreadable: usize,
}

/// Why a Thread row is not delivered at boot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Abandon {
    /// K2 stopped while it was typing it in.
    Interrupted,
    /// Queued too long ago ([`REPLAY_MAX_AGE_MS`]).
    TooOld,
}

/// What a row should become at `now`.
pub(crate) fn plan(row: &JournalRow, now: i64) -> Result<(), Abandon> {
    if row.started_at.is_some() {
        return Err(Abandon::Interrupted);
    }
    if now - row.queued_at > REPLAY_MAX_AGE_MS {
        return Err(Abandon::TooOld);
    }
    Ok(())
}

/// Boot: queue again what a restart left behind. Once, after `ready`.
pub(crate) fn replay_on_boot() -> ReplaySummary {
    let Some(db) = k2_core::db::try_shared() else {
        return ReplaySummary::default();
    };
    let rows = {
        let conn = db.lock();
        match store::pending(&conn) {
            Ok(rows) => rows,
            Err(e) => {
                k2_core::log_debug!("[delivery] journal: can't read it at boot ({e}); nothing re-sent");
                return ReplaySummary::default();
            }
        }
    };
    replay(rows, now_ms())
}

pub(crate) fn replay(rows: Vec<JournalRow>, now: i64) -> ReplaySummary {
    let mut sum = ReplaySummary::default();
    for row in rows {
        match plan(&row, now) {
            Ok(()) => {
                let queued = match row.kind.as_str() {
                    KIND_THREAD => crate::overlay_routes::requeue_thread_delivery(&row),
                    KIND_PROJECT => crate::project_group_routes::requeue_poc_delivery(&row),
                    other => Err(format!("unknown kind {other}")),
                };
                match queued {
                    Ok(()) => {
                        sum.requeued += 1;
                        k2_core::log_debug!(
                            "[delivery] boot: queued again a {} delivery on {} (queued {} s before the restart)",
                            row.kind,
                            row.lane,
                            (now - row.queued_at) / 1000
                        );
                    }
                    Err(e) => {
                        sum.unreadable += 1;
                        k2_core::log_debug!("[delivery] boot: dropped journal row {} on {}: {e}", row.id, row.lane);
                        Entry::existing(&row.id).finish();
                    }
                }
            }
            Err(why) => {
                match why {
                    Abandon::Interrupted => sum.interrupted += 1,
                    Abandon::TooOld => sum.too_old += 1,
                }
                k2_core::log_debug!(
                    "[delivery] boot: not sending a {} delivery on {} again ({why:?}, queued {} s ago)",
                    row.kind,
                    row.lane,
                    (now - row.queued_at) / 1000
                );
                if row.kind == KIND_THREAD {
                    crate::overlay_routes::abandon_thread_delivery(&row, why);
                }
                Entry::existing(&row.id).finish();
            }
        }
    }
    if sum != ReplaySummary::default() {
        k2_core::log_debug!(
            "[delivery] boot: journal replay: {} queued again, {} cut off mid-delivery, {} too old, {} unreadable",
            sum.requeued,
            sum.interrupted,
            sum.too_old,
            sum.unreadable
        );
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(started: Option<i64>, queued_at: i64) -> JournalRow {
        JournalRow {
            id: "r".into(),
            lane: "ws:p".into(),
            kind: KIND_THREAD.into(),
            body: "{}".into(),
            queued_at,
            started_at: started,
        }
    }

    #[test]
    fn a_row_that_never_started_is_sent_again() {
        assert_eq!(plan(&row(None, 1_000), 2_000), Ok(()));
        assert_eq!(plan(&row(None, 0), REPLAY_MAX_AGE_MS), Ok(()), "exactly an hour is still sent");
    }

    #[test]
    fn a_started_row_is_never_typed_in_twice() {
        assert_eq!(plan(&row(Some(1_500), 1_000), 2_000), Err(Abandon::Interrupted));
    }

    #[test]
    fn a_row_over_an_hour_old_is_not_sent() {
        assert_eq!(plan(&row(None, 0), REPLAY_MAX_AGE_MS + 1), Err(Abandon::TooOld));
    }

    #[test]
    fn an_unknown_body_is_dropped_not_kept_forever() {
        let db = k2_core::db::shared();
        let id = uuid::Uuid::new_v4().to_string();
        {
            let conn = db.lock();
            store::record(&conn, &id, "ws:nope", KIND_PROJECT, "not json", 1_000).expect("record");
        }
        let rows: Vec<JournalRow> = {
            let conn = db.lock();
            store::pending(&conn).expect("pending").into_iter().filter(|r| r.id == id).collect()
        };
        let sum = replay(rows, 2_000);
        assert_eq!(sum.unreadable, 1, "{sum:?}");
        let conn = db.lock();
        assert_eq!(store::get(&conn, &id).expect("get"), None, "the bad row is gone");
    }
}
