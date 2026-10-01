//! Heartbeat S2 — missed runs (D7) and "enable / edit waits for the next
//! slot" (D4), driven through the real tick evaluator
//! (`k2so_agents_heartbeat_tick`) against the shared test DB.
//!
//! Kept in its own file so the S5 creation-trap tests in `mod.rs` and
//! these never touch the same lines.

use super::*;
use crate::db::schema::AgentHeartbeat;

const WAKEUP_REL: &str = ".k2/heartbeats/hb/WAKEUP.md";

/// A workspace with a manager agent on disk, one heartbeat row `hb`
/// (hourly, `every_seconds`), and a non-empty WAKEUP.md. Returns
/// `(project_path, project_id)`.
fn seed(label: &str, every_seconds: i64, last_fired: Option<chrono::DateTime<chrono::Utc>>) -> (String, String) {
    crate::db::init_for_tests();
    let dir = std::env::temp_dir().join(format!(
        "k2-hb-missed-{label}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let agent_dir = dir.join(".k2/agent");
    std::fs::create_dir_all(&agent_dir).expect("create agent dir");
    std::fs::write(
        agent_dir.join("AGENT.md"),
        "---\nname: manager\ntype: manager\n---\n# manager\n",
    )
    .expect("write AGENT.md");
    let wakeup = dir.join(WAKEUP_REL);
    std::fs::create_dir_all(wakeup.parent().expect("wakeup parent")).expect("create wakeup dir");
    std::fs::write(&wakeup, "---\nname: hb\n---\nCheck the inbox.\n").expect("write WAKEUP.md");

    let project_path = dir.to_string_lossy().into_owned();
    let project_id = uuid::Uuid::new_v4().to_string();
    let db = crate::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO projects (id, name, path, agent_mode) VALUES (?1, 'hb-missed', ?2, 'manager')",
        rusqlite::params![project_id, project_path],
    )
    .expect("insert project");
    AgentHeartbeat::insert(
        &conn,
        &uuid::Uuid::new_v4().to_string(),
        &project_id,
        "hb",
        "hourly",
        &format!(r#"{{"every_seconds":{every_seconds}}}"#),
        WAKEUP_REL,
        true,
    )
    .expect("insert heartbeat");
    if let Some(t) = last_fired {
        conn.execute(
            "UPDATE workspace_heartbeats SET last_fired = ?1 WHERE project_id = ?2 AND name = 'hb'",
            rusqlite::params![t.to_rfc3339(), project_id],
        )
        .expect("stamp last_fired");
    }
    (project_path, project_id)
}

fn row(project_id: &str) -> AgentHeartbeat {
    let db = crate::db::shared();
    let conn = db.lock();
    AgentHeartbeat::get_by_name(&conn, project_id, "hb")
        .expect("query heartbeat")
        .expect("heartbeat row exists")
}

fn audit_count(project_id: &str, decision: &str) -> i64 {
    let db = crate::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT COUNT(*) FROM heartbeat_fires WHERE project_id = ?1 AND decision = ?2",
        rusqlite::params![project_id, decision],
        |r| r.get(0),
    )
    .expect("count audit rows")
}

#[test]
fn slot_missed_by_more_than_12h_is_skipped_once_and_reanchored() {
    // Daily-cadence interval; the only missed slot is 13h old.
    let day = 86_400;
    let last = chrono::Utc::now() - chrono::Duration::seconds(day + 13 * 3600);
    let (path, pid) = seed("skip", day, Some(last));

    let first = k2so_agents_heartbeat_tick(&path);
    assert!(first.is_empty(), "a 13h-old miss must not fire, got {first:?}");
    assert_eq!(audit_count(&pid, "skipped_missed"), 1);
    let after = row(&pid);
    let anchor = after.schedule_anchor_at.clone().expect("skip must re-anchor the schedule");
    assert_eq!(after.last_fired.as_deref(), Some(last.to_rfc3339().as_str()), "a skip never stamps last_fired");
    let anchor_t = chrono::DateTime::parse_from_rfc3339(&anchor).expect("anchor is RFC3339");
    let age = (chrono::Utc::now() - anchor_t.with_timezone(&chrono::Utc)).num_seconds();
    assert!((0..60).contains(&age), "anchor must be ~now, age {age}s");

    // The next tick sees the next real slot (a day away): no fire, no
    // second skip row.
    let second = k2so_agents_heartbeat_tick(&path);
    assert!(second.is_empty(), "second tick must not fire, got {second:?}");
    assert_eq!(audit_count(&pid, "skipped_missed"), 1, "one skip row per missed slot");
}

#[test]
fn slot_missed_within_12h_fires_one_catch_up_carrying_the_evaluated_last_fired() {
    let day = 86_400;
    let last = chrono::Utc::now() - chrono::Duration::seconds(day + 2 * 3600);
    let (path, pid) = seed("catchup", day, Some(last));

    let cands = k2so_agents_heartbeat_tick(&path);
    assert_eq!(cands.len(), 1, "a 2h-old miss fires one catch-up");
    let c = &cands[0];
    assert!(c.catchup_of.is_some(), "a 2h-late fire is a catch-up");
    assert_eq!(
        c.evaluated_last_fired.as_deref(),
        Some(last.to_rfc3339().as_str()),
        "HB14: the candidate carries the last_fired the evaluator read",
    );
    assert_eq!(audit_count(&pid, "skipped_missed"), 0);
}

#[test]
fn enabling_a_disabled_row_waits_for_its_next_slot() {
    // T-W9: disabled for three days, then enabled → no catch-up fire.
    let hour = 3600;
    let last = chrono::Utc::now() - chrono::Duration::days(3);
    let (path, pid) = seed("enable", hour, Some(last));
    k2so_heartbeat_set_enabled(path.clone(), "hb".into(), false).expect("disable");
    assert!(row(&pid).schedule_anchor_at.is_none(), "disabling does not anchor");
    k2so_heartbeat_set_enabled(path.clone(), "hb".into(), true).expect("enable");
    let anchor = row(&pid).schedule_anchor_at.expect("enable must anchor the schedule");

    let cands = k2so_agents_heartbeat_tick(&path);
    assert!(cands.is_empty(), "enable must not fire a catch-up, got {cands:?}");
    assert_eq!(audit_count(&pid, "skipped_missed"), 0, "enable is not a skipped miss");

    // Re-saving an already-enabled row keeps the anchor.
    k2so_heartbeat_set_enabled(path.clone(), "hb".into(), true).expect("re-enable");
    assert_eq!(row(&pid).schedule_anchor_at.as_deref(), Some(anchor.as_str()));
}

#[test]
fn editing_the_schedule_waits_for_the_next_slot() {
    let hour = 3600;
    let last = chrono::Utc::now() - chrono::Duration::hours(5);
    let (path, pid) = seed("edit", hour, Some(last));
    // Before the edit the row is overdue (catch-up due).
    let before = k2so_agents_heartbeat_tick(&path);
    assert_eq!(before.len(), 1, "precondition: the row is due before the edit");

    k2so_heartbeat_edit(
        path.clone(),
        "hb".into(),
        "hourly".into(),
        r#"{"every_seconds":1800}"#.into(),
    )
    .expect("edit schedule");
    assert!(row(&pid).schedule_anchor_at.is_some(), "edit must anchor the schedule");
    let after = k2so_agents_heartbeat_tick(&path);
    assert!(after.is_empty(), "an edit must not fire a catch-up, got {after:?}");
}

#[test]
fn unarchive_waits_for_the_next_slot() {
    let hour = 3600;
    let last = chrono::Utc::now() - chrono::Duration::hours(6);
    let (path, pid) = seed("unarchive", hour, Some(last));
    k2so_heartbeat_archive(path.clone(), "hb".into()).expect("archive");
    k2so_heartbeat_unarchive(path.clone(), "hb".into()).expect("unarchive");
    assert!(row(&pid).schedule_anchor_at.is_some(), "unarchive must anchor the schedule");
    let cands = k2so_agents_heartbeat_tick(&path);
    assert!(cands.is_empty(), "unarchive must not fire a catch-up, got {cands:?}");
}
