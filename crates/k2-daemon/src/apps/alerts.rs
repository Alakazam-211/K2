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

static SINK: RwLock<Option<Sink>> = RwLock::new(None);

/// Replace the delivery (tests). `None` restores the real one.
pub fn set_sink(s: Option<Sink>) {
    *SINK.write().unwrap_or_else(|e| e.into_inner()) = s;
}

/// Leave the workspace's agent an inbox note, **no wake** (its App access
/// changed, prd-agent-app-permissions-v1 §5). With a test sink installed
/// the note goes there as `"[note] <title>\n\n<text>"`.
pub fn note(workspace_id: &str, title: &str, text: &str) {
    if let Some(s) = SINK.read().unwrap_or_else(|e| e.into_inner()).as_ref() {
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
    if let Some(s) = SINK.read().unwrap_or_else(|e| e.into_inner()).as_ref() {
        s(workspace_id, text);
        return;
    }
    let ws = workspace_id.to_string();
    let text = text.to_string();
    let _ = std::thread::Builder::new().name("app-alert".into()).spawn(move || {
        let r = crate::workspace_msg::deliver_live(&ws, &text, "k2-apps", "", true, Duration::from_secs(60));
        k2_core::log_debug!("[apps] alert to {ws}: {}", serde_json::to_string(&r).unwrap_or_default());
    });
}
