//! The Org Chart's daemon half (prd-org-chart-v1; the store and rules are
//! `k2_core::org_chart`):
//!
//! | Piece | File |
//! |---|---|
//! | who is calling (owner, login, shell tab, agent passport) | [`caller`] |
//! | `/cli/organization/*` (one handler for TCP and the cell socket) | [`routes`] |
//! | the night pass on the heartbeat monitor's tick | [`night`] |
//! | the quiet feed field on agents' responses | [`feed`] |
//! | "connect me to that head": card, ticket, and closing the ask | [`reach`] |
//!
//! Every org wake (night run, last call, a member's update at night, a
//! head's ask, a reach outcome) goes through [`wake`]: the hourly wake
//! budget first (`k2_core::wake_budget::has_room`), then the shared agent
//! wake path (`agent_wake::deliver`, `via = "org"`), then one
//! `org_chart_wakes` row so the budget counts it.

pub mod caller;
pub mod feed;
pub mod night;
pub mod reach;
pub mod routes;

#[cfg(test)]
mod tests;

use std::cell::RefCell;
use std::rc::Rc;

use k2_core::org_chart::store;

/// Delivers one line to an agent (injectable for tests).
pub trait Waker {
    /// `Ok(())` = delivered (live, or after waking it).
    fn wake(&self, project_id: &str, from: &str, text: &str) -> Result<(), String>;
}

/// The real one: `agent_wake::deliver`, on the agent's lane.
pub struct RealWaker;

impl Waker for RealWaker {
    fn wake(&self, project_id: &str, from: &str, text: &str) -> Result<(), String> {
        let path = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::db::schema::Project::get(&conn, project_id).map(|p| p.path).map_err(|_| "workspace gone".to_string())?
        };
        let r = crate::agent_wake::deliver(crate::agent_wake::Wake {
            workspace: &path,
            agent: project_id,
            text,
            from,
            via: "org",
            wake_timeout: crate::workspace_msg::DEFAULT_WAKE_TIMEOUT,
        });
        if r.success {
            Ok(())
        } else {
            Err(r.reason.map(|c| c.to_string()).unwrap_or_else(|| "delivery failed".to_string()))
        }
    }
}

thread_local! {
    static TEST_WAKER: RefCell<Option<Rc<dyn Waker>>> = const { RefCell::new(None) };
}

/// Run `f` with `w` as this thread's waker (tests).
#[cfg(test)]
pub(crate) fn with_waker<R>(w: Rc<dyn Waker>, f: impl FnOnce() -> R) -> R {
    let prev = TEST_WAKER.with(|c| c.replace(Some(w)));
    let out = f();
    TEST_WAKER.with(|c| *c.borrow_mut() = prev);
    out
}

/// The waker in use: a test's, else [`RealWaker`].
pub(crate) fn waker() -> Rc<dyn Waker> {
    TEST_WAKER.with(|c| c.borrow().clone()).unwrap_or_else(|| Rc::new(RealWaker))
}

/// What happened to one org wake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeOutcome {
    Delivered,
    /// The agent's hourly wake budget is spent (`agent_cap`).
    Capped,
    Failed(String),
}

impl WakeOutcome {
    pub fn word(&self) -> String {
        match self {
            WakeOutcome::Delivered => "delivered".into(),
            WakeOutcome::Capped => k2_core::wake_budget::DECISION_AGENT_CAP.into(),
            WakeOutcome::Failed(e) => format!("failed: {e}"),
        }
    }
}

/// One org wake: budget, deliver, count. `kind` is a `store::WAKE_*`.
pub fn wake(w: &dyn Waker, project_id: &str, kind: &str, from: &str, text: &str, now: i64) -> WakeOutcome {
    let room = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        // A query error refuses the wake rather than spending past the cap.
        k2_core::wake_budget::has_room(&conn, project_id, now).unwrap_or(false)
    };
    if !room {
        return WakeOutcome::Capped;
    }
    match w.wake(project_id, from, text) {
        Ok(()) => {
            let db = k2_core::db::shared();
            let conn = db.lock();
            if let Err(e) = store::record_wake(&conn, project_id, kind, now) {
                k2_core::log_debug!("[org-chart] count wake: {e}");
            }
            WakeOutcome::Delivered
        }
        Err(e) => WakeOutcome::Failed(e),
    }
}

/// The name K2 shows for an agent (its handle), or its id when it's gone.
pub(crate) fn caller_handle(project_id: &str) -> String {
    k2_core::workspace_session_handles::workspace_address_name_shared(project_id).unwrap_or_else(|_| project_id.to_string())
}

/// Tell every client to redraw.
pub(crate) fn changed(revision: i64) {
    let _ = crate::session_events::emit(crate::session_events::SessionEvent::OrgChartChanged { revision });
}
