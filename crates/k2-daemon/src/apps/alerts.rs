//! Tell an App's agent something happened (§6.4, litmus L3): a deploy that
//! stalled, a crash loop. Through the existing rail: a `k2 msg` wake of the
//! App's owning workspace (`workspace_msg::deliver_live`, AR19). Durable
//! tickets and activity-socket frames for incidents are stage R (S18).
//!
//! Tests install a recording sink ([`set_sink`]), so no test ever wakes a
//! real agent CLI.

use std::sync::RwLock;
use std::time::Duration;

/// `(workspace_id, text)`.
pub type Sink = Box<dyn Fn(&str, &str) + Send + Sync>;

type Shared = std::sync::Arc<dyn Fn(&str, &str) + Send + Sync>;

static SINK: RwLock<Option<Shared>> = RwLock::new(None);

// Under test, the thread that installs a sink also keeps its own copy, read
// first: libtest runs every test on its own thread, so a test's alerts from
// its own thread reach its own sink even when a test running beside it
// installs another (the process-wide slot was last-writer-wins: an audit
// test's alert went to a reach test's sink in the shuffled soak). Alerts
// raised on other threads (a supervisor, a worker) still use the
// process-wide slot, as before.
#[cfg(test)]
thread_local! {
    static THREAD_SINK: std::cell::RefCell<Option<Shared>> = const { std::cell::RefCell::new(None) };
}

/// Replace the delivery (tests). `None` restores the real one.
pub fn set_sink(s: Option<Sink>) {
    let s: Option<Shared> = s.map(Shared::from);
    #[cfg(test)]
    THREAD_SINK.with(|t| *t.borrow_mut() = s.clone());
    *SINK.write().unwrap_or_else(|e| e.into_inner()) = s;
}

/// The installed sink, if any: this thread's own first (tests).
fn sink() -> Option<Shared> {
    #[cfg(test)]
    if let Some(s) = THREAD_SINK.with(|t| t.borrow().clone()) {
        return Some(s);
    }
    SINK.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Leave the workspace's agent an inbox note, **no wake** (its App access
/// changed, prd-agent-app-permissions-v1 §5). With a test sink installed
/// the note goes there as `"[note] <title>\n\n<text>"`.
pub fn note(workspace_id: &str, title: &str, text: &str) {
    if let Some(s) = sink() {
        s(workspace_id, &format!("[note] {title}\n\n{text}"));
        return;
    }
    let path = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::Project::get(&conn, workspace_id).ok().map(|p| p.path)
    };
    let Some(path) = path else { return };
    if let Err(e) = k2_core::inbox::compose(std::path::Path::new(&path), title, text, Some("normal"), Some("apps"), Some("k2-apps")) {
        k2_core::log_debug!("[apps] note to {workspace_id}: {e}");
    }
}

/// Deliver `text` to the App's owning workspace agent. Never blocks the
/// caller: the real delivery (which may wake the agent) runs on a thread.
pub fn notify(workspace_id: &str, text: &str) {
    if let Some(s) = sink() {
        s(workspace_id, text);
        return;
    }
    let ws = workspace_id.to_string();
    let text = text.to_string();
    // In the daemon's runtime: a wake spawns the agent's session (`crate::rt`).
    let spawned = crate::rt::spawn("app-alert", move || {
        let r = crate::workspace_msg::deliver_live(&ws, &text, "k2-apps", "", true, Duration::from_secs(60));
        k2_core::log_debug!("[apps] alert to {ws}: {}", serde_json::to_string(&r).unwrap_or_default());
    });
    if let Err(e) = spawned {
        k2_core::log_debug!("[apps] alert to {workspace_id} not sent: {e}");
    }
}
