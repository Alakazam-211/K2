//! A heartbeat auto-disabled after repeated failures (0.46.18): the real
//! failure policy with the live Thread and bus posts exactly one system
//! row on the agent's Thread, with its button, and one
//! `heartbeat_turned_off` event.

use super::*;
use crate::heartbeat_launch::{record_fire_failure_in, MAX_CONSECUTIVE_FAILURES};
use crate::heartbeat_turned_off::LiveEffects;
use crate::session_events::SessionEvent;
use k2_core::db::schema::AgentHeartbeat;

fn notices(handle: &str) -> Vec<serde_json::Value> {
    let snap = json_body(&dispatch("/cli/thread", &params_of(&[("addr", handle)])).expect("GET thread"));
    snap["items"].as_array().expect("items").iter().filter(|i| i["doc"].get("systemNotice").is_some()).cloned().collect()
}

fn morning(project_id: &str) -> AgentHeartbeat {
    let db = k2_core::db::shared();
    let conn = db.lock();
    AgentHeartbeat::get_by_name(&conn, project_id, "morning").expect("query").expect("row")
}

#[test]
fn an_auto_disable_is_one_system_row_and_one_event() {
    let _home_lock = crate::test_support::lock_home();
    let handle = format!("hbto{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let (project_id, _) = seed(&handle);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        AgentHeartbeat::insert(
            &conn,
            &uuid::Uuid::new_v4().to_string(),
            &project_id,
            "morning",
            "daily",
            r#"{"time":"09:00"}"#,
            ".k2/heartbeats/morning/WAKEUP.md",
            true,
        )
        .expect("insert heartbeat");
    }
    let mut rx = crate::session_events::subscribe();
    let now = chrono::Utc::now().timestamp();
    for i in 1..=MAX_CONSECUTIVE_FAILURES {
        let hb = morning(&project_id);
        record_fire_failure_in(&LiveEffects, &project_id, &handle, &hb, "resume spawn failed: exit status 1", now + i);
    }
    assert!(!morning(&project_id).enabled, "turned off");

    let rows = notices(&handle);
    assert_eq!(rows.len(), 1, "exactly one notice: {rows:?}");
    let doc = &rows[0]["doc"];
    assert_eq!(doc["author"], serde_json::json!({"kind": "system", "login": "k2"}), "{doc}");
    assert_eq!(
        doc["body"],
        format!("{handle}'s heartbeat 'morning' was turned off after 5 failed runs (resume spawn failed: exit status 1).")
    );
    assert_eq!(
        doc["systemNotice"],
        serde_json::json!({
            "kind": "action",
            "code": "heartbeat.auto_disabled",
            "actions": [{
                "id": "heartbeat_enable",
                "label": "Turn it back on",
                "route": "/cli/heartbeat/enable",
                "body": {"project": handle, "name": "morning", "enabled": true}
            }]
        }),
        "{doc}"
    );

    let mut mine = Vec::new();
    while let Ok(e) = rx.try_recv() {
        if let SessionEvent::HeartbeatTurnedOff { ref workspace_id, .. } = e {
            if *workspace_id == project_id {
                mine.push(serde_json::to_value(&e).expect("json"));
            }
        }
    }
    assert_eq!(mine.len(), 1, "exactly one event: {mine:?}");
    let ev = &mine[0];
    assert_eq!(ev["kind"], "heartbeat_turned_off");
    assert_eq!(ev["state"], "disabled");
    assert_eq!(ev["address"], handle.as_str());
    assert_eq!(ev["name"], "morning");
    assert_eq!(ev["failures"], 5);
    assert_eq!(ev["pausedUntil"], serde_json::Value::Null);
    assert_eq!(ev["noticeId"], rows[0]["id"]);
}
