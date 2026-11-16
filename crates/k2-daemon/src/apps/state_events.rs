//! An App's state changed without anyone asking through a route that tells
//! the clients: a crash, a restart scheduled, the crash-loop breaker,
//! waiting for K2's engine, a stop, a suspend or resume, a park. The
//! owning workspace's clients hear the existing `publish_services_changed
//! { projectId }` (the drawer refetches `GET /cli/app/list?workspace=` on
//! it), so a crashed App never keeps reading "running".
//!
//! Coalesced per App so a crash loop can't flood the bus: the first change
//! arms one flush [`SETTLE`] later, changes before it fold in, and two
//! flushes of one App are at least [`WINDOW`] apart. The last change is
//! always told (the flush runs after it). A flush tells each workspace the
//! App touched once (a park tells the workspace it left; a parked App's
//! `''` is no one's).

use std::collections::{BTreeSet, HashMap};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use k2_core::deploy::store;

/// How long a flush waits for the changes that come with the first one
/// (a stop then a park, a crash then its restart scheduled).
pub const SETTLE: Duration = Duration::from_millis(250);
/// At most one flush per App per window.
pub const WINDOW: Duration = Duration::from_secs(1);

/// One App's coalescing state. Pure (the clock is passed in) so the
/// bound is tested on a manual clock.
#[derive(Debug, Default)]
pub(crate) struct Gate {
    last: Option<Instant>,
    armed: Option<Instant>,
    pending: BTreeSet<String>,
    /// Open [`hold`]s: changes fold in, nothing arms until the last ends.
    held: usize,
}

impl Gate {
    /// A change for workspace `ws` at `now`. `Some(at)`: arm a flush at
    /// `at` (none was armed); `None`: the armed flush carries it.
    pub(crate) fn note(&mut self, ws: &str, now: Instant) -> Option<Instant> {
        let ws = ws.trim();
        if !ws.is_empty() {
            self.pending.insert(ws.to_string());
        }
        self.arm(now)
    }

    /// A [`hold`] began: changes until it ends fold into one flush.
    pub(crate) fn hold(&mut self) {
        self.held += 1;
    }

    /// A hold ended at `now`: `Some(at)` arms the flush it held back.
    pub(crate) fn release(&mut self, now: Instant) -> Option<Instant> {
        self.held = self.held.saturating_sub(1);
        self.arm(now)
    }

    fn arm(&mut self, now: Instant) -> Option<Instant> {
        if self.held > 0 || self.armed.is_some() || self.pending.is_empty() {
            return None;
        }
        let at = match self.last {
            Some(l) => (now + SETTLE).max(l + WINDOW),
            None => now + SETTLE,
        };
        self.armed = Some(at);
        Some(at)
    }

    /// The armed flush ran at `now`: the workspaces to tell, each once.
    /// A flush armed before a [`hold`] that comes due inside it tells
    /// nothing: the hold's release arms it again.
    pub(crate) fn flush(&mut self, now: Instant) -> Vec<String> {
        self.armed = None;
        if self.held > 0 {
            return Vec::new();
        }
        let out: Vec<String> = std::mem::take(&mut self.pending).into_iter().collect();
        if !out.is_empty() {
            self.last = Some(now);
        }
        out
    }
}

fn gates() -> &'static Mutex<HashMap<String, Gate>> {
    static G: OnceLock<Mutex<HashMap<String, Gate>>> = OnceLock::new();
    G.get_or_init(|| Mutex::new(HashMap::new()))
}

/// `app_id`'s state changed: its owning workspace's clients re-read. Call
/// it after the write, outside any DB lock.
pub fn app_changed(app_id: &str) {
    let ws = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        store::get_app(&conn, app_id).ok().flatten().map(|a| a.owner_workspace)
    };
    if let Some(ws) = ws {
        app_changed_in(app_id, &ws);
    }
}

/// [`app_changed`] when the caller knows the workspace to tell (a park:
/// the one the App left, its row already says `''`).
pub fn app_changed_in(app_id: &str, ws: &str) {
    let arm = {
        let mut g = gates().lock().unwrap_or_else(|e| e.into_inner());
        g.entry(app_id.to_string()).or_default().note(ws, Instant::now())
    };
    if let Some(at) = arm {
        flush_at(app_id, at);
    }
}

/// Fold every change to `app_id` until the returned guard drops into one
/// flush: a park (the stop, then the row moving to no one) tells the
/// workspace it left once however long the steps between take; the
/// settle alone can't promise that on a slow run.
#[must_use]
pub fn hold(app_id: &str) -> Held {
    let mut g = gates().lock().unwrap_or_else(|e| e.into_inner());
    g.entry(app_id.to_string()).or_default().hold();
    Held { app_id: app_id.to_string() }
}

/// [`hold`]'s guard: dropping it arms the flush it held back.
pub struct Held {
    app_id: String,
}

impl Drop for Held {
    fn drop(&mut self) {
        let arm = {
            let mut g = gates().lock().unwrap_or_else(|e| e.into_inner());
            g.get_mut(&self.app_id).and_then(|g| g.release(Instant::now()))
        };
        if let Some(at) = arm {
            flush_at(&self.app_id, at);
        }
    }
}

/// Run `app_id`'s armed flush at `at`.
fn flush_at(app_id: &str, at: Instant) {
    let app = app_id.to_string();
    let spawned = std::thread::Builder::new().name("app-state-event".into()).spawn(move || {
        std::thread::sleep(at.saturating_duration_since(Instant::now()));
        flush(&app);
    });
    if spawned.is_err() {
        // No thread to wait on: tell now rather than never.
        flush(app_id);
    }
}

/// Tests: sleep past one armed flush (settle, the window, slack), so every
/// change made before it has been told.
#[cfg(test)]
pub(crate) fn wait_told() {
    std::thread::sleep(SETTLE + WINDOW + Duration::from_millis(400));
}

/// Tests: drain `rx`, counting what workspace `ws`'s clients refetch on:
/// (`publish_services_changed`, `app_deployed`). A lagged bus fails loudly.
#[cfg(test)]
pub(crate) fn drain_refreshes(rx: &mut tokio::sync::broadcast::Receiver<crate::session_events::SessionEvent>, ws: &str) -> (usize, usize) {
    use crate::session_events::SessionEvent;
    use tokio::sync::broadcast::error::TryRecvError;
    let (mut changed, mut deployed) = (0, 0);
    loop {
        match rx.try_recv() {
            Ok(SessionEvent::PublishServicesChanged { project_id }) if project_id == ws => changed += 1,
            Ok(SessionEvent::AppDeployed { project_id, .. }) if project_id == ws => deployed += 1,
            Ok(_) => {}
            Err(TryRecvError::Empty) => return (changed, deployed),
            Err(e) => panic!("the session bus: {e:?}"),
        }
    }
}

fn flush(app_id: &str) {
    let to = {
        let mut g = gates().lock().unwrap_or_else(|e| e.into_inner());
        g.get_mut(app_id).map(|g| g.flush(Instant::now())).unwrap_or_default()
    };
    for ws in to {
        let _ = crate::session_events::emit(crate::session_events::SessionEvent::PublishServicesChanged { project_id: ws });
    }
}
