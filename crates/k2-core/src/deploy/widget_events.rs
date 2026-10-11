//! Garden widget versions in the deploy audit (S9, AR23).
//!
//! Widget versions, last-good and rollback already live in
//! `zen::widget_store` (`.history/widgets/<name>/`). Gardens never run on
//! an engine; this only writes `deploy_audit` rows
//! (`subject_kind = garden_widget`) from widget_store events, so one
//! hash-chained history covers Apps and widgets. No store move.
//!
//! `widget_store` calls [`emit`]; it is a no-op until the daemon installs
//! the sink ([`install_db_sink`]), so library tests on temp Zen folders
//! never write to a DB.

use std::sync::RwLock;

use rusqlite::Connection;
use serde_json::json;

use super::audit;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WidgetEvent {
    /// A folder bundled clean with new content: its new last-good version.
    LastGood { widget: String, hash: String },
    /// `k2 zen reset --widget`: code restored from a snapshot.
    Reset { widget: String, restored: String, kept: Option<String> },
    /// `k2 zen widget new`: a widget folder created from a starter.
    Created { widget: String, from: String },
}

impl WidgetEvent {
    pub fn widget(&self) -> &str {
        match self {
            WidgetEvent::LastGood { widget, .. } | WidgetEvent::Reset { widget, .. } | WidgetEvent::Created { widget, .. } => widget,
        }
    }

    fn event(&self) -> &'static str {
        match self {
            WidgetEvent::LastGood { .. } => "last_good",
            WidgetEvent::Reset { .. } => "reset",
            WidgetEvent::Created { .. } => "created",
        }
    }

    /// The watcher sees a last-good change (`system`); reset and new come
    /// from the desktop owner's own commands.
    fn actor(&self) -> &'static str {
        match self {
            WidgetEvent::LastGood { .. } => "system",
            _ => "owner",
        }
    }

    fn payload(&self) -> serde_json::Value {
        match self {
            WidgetEvent::LastGood { hash, .. } => json!({ "hash": hash }),
            WidgetEvent::Reset { restored, kept, .. } => json!({ "restored": restored, "kept": kept }),
            WidgetEvent::Created { from, .. } => json!({ "from": from }),
        }
    }
}

/// Write one event as a `deploy_audit` row.
pub fn record(conn: &Connection, e: &WidgetEvent) -> rusqlite::Result<String> {
    audit::append(conn, audit::SUBJECT_GARDEN_WIDGET, e.widget(), e.event(), e.actor(), None, &e.payload())
}

pub type Sink = Box<dyn Fn(&WidgetEvent) + Send + Sync>;

static SINK: RwLock<Option<Sink>> = RwLock::new(None);

pub fn set_sink(s: Option<Sink>) {
    *SINK.write().unwrap_or_else(|e| e.into_inner()) = s;
}

/// Called by `zen::widget_store`. No-op without a sink.
pub fn emit(e: WidgetEvent) {
    if let Some(s) = SINK.read().unwrap_or_else(|p| p.into_inner()).as_ref() {
        s(&e);
    }
}

/// The daemon's sink: each event becomes a row on the shared DB.
pub fn install_db_sink() {
    set_sink(Some(Box::new(|e| {
        let db = crate::db::shared();
        let conn = db.lock();
        if let Err(err) = record(&conn, e) {
            crate::log_debug!("[deploy] widget audit {}: {err}", e.widget());
        }
    })));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widget_events_are_chained_audit_rows() {
        let c = super::super::store::tests::conn();
        record(&c, &WidgetEvent::Created { widget: "arcade".into(), from: "hello".into() }).unwrap();
        record(&c, &WidgetEvent::LastGood { widget: "arcade".into(), hash: "h1".into() }).unwrap();
        record(&c, &WidgetEvent::Reset { widget: "arcade".into(), restored: "s1".into(), kept: None }).unwrap();
        let rows = audit::list_for(&c, audit::SUBJECT_GARDEN_WIDGET, "arcade", 10).unwrap();
        let events: Vec<&str> = rows.iter().map(|r| r.event.as_str()).collect();
        assert_eq!(events, vec!["reset", "last_good", "created"]);
        assert_eq!(rows[1].actor, "system");
        assert_eq!(rows[1].payload["hash"], "h1");
        assert_eq!(audit::verify(&c).unwrap(), Ok(3));
    }
}
