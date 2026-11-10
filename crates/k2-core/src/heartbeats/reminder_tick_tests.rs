//! Reminders through the real tick (`k2so_agents_heartbeat_tick`) against the
//! shared test DB: what the scan does with a due, a far-too-late and a capped
//! reminder.

use std::sync::Mutex;

use super::reminders::{self, NewReminder, SessionChoice};
use super::when::WhenInput;
use super::*;
use crate::db::schema::{AgentHeartbeat, HeartbeatFire};

static NOTES: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

fn capture(path: &str, text: &str) {
    NOTES.lock().expect("notes").push((path.to_string(), text.to_string()));
}

fn notes_for(path: &str) -> Vec<String> {
    NOTES.lock().expect("notes").iter().filter(|(p, _)| p == path).map(|(_, t)| t.clone()).collect()
}

/// A workspace with an agent, and one reminder `r` backdated so it was due
/// `late_secs` ago. Returns `(project_path, project_id)`.
fn seed(label: &str, late_secs: i64) -> (String, String) {
    crate::db::init_for_tests();
    reminders::set_note_sink(capture);
    let dir = std::env::temp_dir().join(format!("k2-rem-tick-{label}-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
    let agent_dir = dir.join(".k2/agent");
    std::fs::create_dir_all(&agent_dir).expect("agent dir");
    std::fs::write(agent_dir.join("AGENT.md"), "---\nname: manager\ntype: manager\n---\n# manager\n").expect("AGENT.md");
    let path = dir.to_string_lossy().into_owned();
    let project_id = uuid::Uuid::new_v4().to_string();
    {
        let db = crate::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, agent_mode) VALUES (?1, 'rem-tick', ?2, 'manager')",
            rusqlite::params![project_id, path],
        )
        .expect("project");
    }
    let now = chrono::Utc::now();
    reminders::add(
        &path,
        NewReminder {
            text: "call Jane back",
            when: WhenInput { in_: Some("1h"), ..Default::default() },
            name: Some("r"),
            file: None,
            session: SessionChoice::Pinned,
            created_by: "user:rosson",
        },
        now,
    )
    .expect("add");
    let db = crate::db::shared();
    let conn = db.lock();
    let fire_at = (now - chrono::Duration::seconds(late_secs)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    conn.execute(
        "UPDATE workspace_heartbeats SET fire_at = ?1 WHERE project_id = ?2 AND name = 'r'",
        rusqlite::params![fire_at, project_id],
    )
    .expect("backdate");
    (path, project_id)
}

fn row(project_id: &str) -> AgentHeartbeat {
    let db = crate::db::shared();
    let conn = db.lock();
    AgentHeartbeat::get_by_name(&conn, project_id, "r").expect("q").expect("row")
}

fn decisions(project_id: &str) -> Vec<String> {
    let db = crate::db::shared();
    let conn = db.lock();
    HeartbeatFire::list_by_schedule_name(&conn, project_id, "r", 50)
        .expect("fires")
        .into_iter()
        .map(|f| f.decision)
        .collect()
}

#[test]
fn a_due_reminder_is_a_candidate_with_no_wakeup_file_and_stays_enabled() {
    let (path, pid) = seed("due", 5);
    let cands = k2so_agents_heartbeat_tick(&path);
    assert_eq!(cands.len(), 1, "the reminder is due");
    assert_eq!(cands[0].name, "r");
    assert_eq!(cands[0].catchup_of, None, "on time");
    let r = row(&pid);
    assert!(r.enabled, "W6: no WAKEUP.md must not auto-disable a reminder");
    assert_eq!(r.disabled_reason, None);
}

#[test]
fn a_late_reminder_is_a_catchup_candidate() {
    let (path, _pid) = seed("late", 13 * 3600);
    let cands = k2so_agents_heartbeat_tick(&path);
    assert_eq!(cands.len(), 1, "W8: 13 h late still fires");
    assert!(cands[0].catchup_of.is_some());
}

#[test]
fn past_24h_it_retires_as_missed_with_one_note_and_never_fires() {
    let (path, pid) = seed("missed", 25 * 3600);
    let cands = k2so_agents_heartbeat_tick(&path);
    assert!(cands.is_empty(), "too late to fire");
    let r = row(&pid);
    assert_eq!(r.retired_reason.as_deref(), Some("missed"));
    assert!(!r.enabled && r.archived_at.is_some());
    assert_eq!(decisions(&pid), vec!["skipped_missed".to_string()]);
    let notes = notes_for(&path);
    assert_eq!(notes.len(), 1, "one note to the owner: {notes:?}");
    assert!(notes[0].contains("call Jane back") && notes[0].contains("didn't fire"), "{notes:?}");

    assert!(k2so_agents_heartbeat_tick(&path).is_empty());
    assert_eq!(decisions(&pid).len(), 1, "no second audit row");
    assert_eq!(notes_for(&path).len(), 1, "no second note");
}

#[test]
fn the_21st_fire_in_an_hour_waits_a_minute_and_is_audited_once() {
    let (path, pid) = seed("cap", 5);
    {
        let db = crate::db::shared();
        let conn = db.lock();
        for i in 0..reminders::MAX_FIRES_PER_HOUR {
            HeartbeatFire::insert_with_schedule(
                &conn, &pid, Some("manager"), Some(&format!("old-{i}")), "once", "fired", None, None, None, None,
            )
            .expect("fire");
        }
    }
    assert!(k2so_agents_heartbeat_tick(&path).is_empty(), "held by the cap");
    let r = row(&pid);
    assert!(r.next_retry_at.is_some(), "the next minute");
    assert!(r.enabled && r.retired_at.is_none(), "still pending");
    assert_eq!(decisions(&pid), vec![reminders::DECISION_DEFERRED_CAP.to_string()]);

    assert!(k2so_agents_heartbeat_tick(&path).is_empty(), "the retry gate holds it");
    assert_eq!(decisions(&pid).len(), 1, "one audit row per wait");
}

#[test]
fn the_agents_wake_budget_holds_a_reminder_too() {
    let (path, pid) = seed("budget", 5);
    {
        let db = crate::db::shared();
        let conn = db.lock();
        // Filled from the Sources table: 30 reminder fires would trip the
        // workspace's own cap (20) first.
        let now = chrono::Utc::now().timestamp();
        for i in 0..crate::wake_budget::AGENT_WAKES_PER_HOUR {
            conn.execute(
                "INSERT INTO source_trigger_fires (id, trigger_id, source_id, target_project_id, incident_id, at, outcome) \
                 VALUES (?1, 't', 's', ?2, ?1, ?3, 'delivered')",
                rusqlite::params![format!("f{i}"), pid, now - 10],
            )
            .expect("source fire");
        }
    }
    assert!(k2so_agents_heartbeat_tick(&path).is_empty(), "the budget is spent");
    assert_eq!(decisions(&pid), vec![crate::wake_budget::DECISION_AGENT_CAP.to_string()]);
}
