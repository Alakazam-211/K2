//! A WAKEUP.md that is missing or can't be read never turns a heartbeat
//! off. Driven through the real tick (`k2_agents_heartbeat_tick`) against
//! the shared test DB.
//!
//! From a field report: a host problem made every workspace folder
//! unreadable for about 76 minutes. Each heartbeat that came due in that
//! window was turned off for good at its first fire, silently, and stayed
//! off for hours after the files were back.

use super::*;
use crate::db::schema::AgentHeartbeat;

const WAKEUP_REL: &str = ".k2/heartbeats/hb/WAKEUP.md";
const BODY: &str = "---\nname: hb\n---\nCheck the inbox.\n";

struct Ws {
    dir: std::path::PathBuf,
    path: String,
    project_id: String,
}

impl Ws {
    fn wakeup(&self) -> std::path::PathBuf {
        self.dir.join(WAKEUP_REL)
    }

    /// Where the folder goes while it is "moved away".
    fn away(&self) -> std::path::PathBuf {
        self.dir.with_extension("away")
    }
}

/// A workspace with an agent on disk, one minutely heartbeat `hb` that
/// last fired 90 s ago (so it is due now), and a WAKEUP.md with a body.
fn seed(label: &str) -> Ws {
    crate::db::init_for_tests();
    let dir = std::env::temp_dir().join(format!(
        "k2-hb-wwait-{label}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let agent_dir = dir.join(".k2/agent");
    std::fs::create_dir_all(&agent_dir).expect("create agent dir");
    std::fs::write(agent_dir.join("AGENT.md"), "---\nname: manager\ntype: manager\n---\n# manager\n")
        .expect("write AGENT.md");
    let wakeup = dir.join(WAKEUP_REL);
    std::fs::create_dir_all(wakeup.parent().expect("wakeup parent")).expect("create wakeup dir");
    std::fs::write(&wakeup, BODY).expect("write WAKEUP.md");

    let path = dir.to_string_lossy().into_owned();
    let project_id = uuid::Uuid::new_v4().to_string();
    let db = crate::db::shared();
    let conn = db.lock();
    // agent_enabled = 1: the agent name still resolves (from the DB) while
    // the folder is gone, as it does on a configured workspace.
    conn.execute(
        "INSERT INTO projects (id, name, path, agent_mode, agent_enabled) \
         VALUES (?1, 'hb-wwait', ?2, 'manager', 1)",
        rusqlite::params![project_id, path],
    )
    .expect("insert project");
    AgentHeartbeat::insert(
        &conn,
        &uuid::Uuid::new_v4().to_string(),
        &project_id,
        "hb",
        "hourly",
        r#"{"every_seconds":60}"#,
        WAKEUP_REL,
        true,
    )
    .expect("insert heartbeat");
    let last = chrono::Utc::now() - chrono::Duration::seconds(90);
    conn.execute(
        "UPDATE workspace_heartbeats SET last_fired = ?1 WHERE project_id = ?2 AND name = 'hb'",
        rusqlite::params![last.to_rfc3339(), project_id],
    )
    .expect("stamp last_fired");
    Ws { dir, path, project_id }
}

fn row(ws: &Ws) -> AgentHeartbeat {
    let db = crate::db::shared();
    let conn = db.lock();
    AgentHeartbeat::get_by_name(&conn, &ws.project_id, "hb")
        .expect("query heartbeat")
        .expect("heartbeat row exists")
}

fn listed(ws: &Ws) -> AgentHeartbeat {
    crate::heartbeats::k2_heartbeat_list(ws.path.clone())
        .expect("list")
        .into_iter()
        .find(|r| r.name == "hb")
        .expect("row listed")
}

fn tick(ws: &Ws) -> Vec<String> {
    k2_agents_heartbeat_tick(&ws.path).into_iter().map(|c| c.name).collect()
}

fn audit_count(ws: &Ws, decision: &str) -> i64 {
    let db = crate::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT COUNT(*) FROM heartbeat_fires WHERE project_id = ?1 AND schedule_name = 'hb' AND decision = ?2",
        rusqlite::params![ws.project_id, decision],
        |r| r.get(0),
    )
    .expect("count audit rows")
}

/// Every inbox item's body text, oldest first.
fn inbox_texts(ws: &Ws) -> Vec<String> {
    let mut items = crate::inbox::list_root(&ws.dir);
    items.sort_by(|a, b| a.filename.cmp(&b.filename));
    items
        .iter()
        .map(|i| {
            let p = crate::inbox::inbox_root(&ws.dir).join(&i.filename);
            std::fs::read_to_string(&p).expect("read inbox item")
        })
        .collect()
}

/// Move the schedule's reference back a little, so the latest due slot is
/// a different one: the next tick is the next scheduled fire.
fn next_slot(ws: &Ws) {
    let db = crate::db::shared();
    let conn = db.lock();
    let hb = AgentHeartbeat::get_by_name(&conn, &ws.project_id, "hb").unwrap().unwrap();
    let last = chrono::DateTime::parse_from_rfc3339(hb.last_fired.as_deref().expect("last_fired"))
        .expect("parse last_fired");
    let earlier = last - chrono::Duration::seconds(13);
    conn.execute(
        "UPDATE workspace_heartbeats SET last_fired = ?1 WHERE project_id = ?2 AND name = 'hb'",
        rusqlite::params![earlier.to_rfc3339(), ws.project_id],
    )
    .unwrap();
}

fn assert_on(ws: &Ws, when: &str) {
    let hb = row(ws);
    assert!(hb.enabled, "{when}: the heartbeat must stay on");
    assert_eq!(hb.disabled_reason, None, "{when}: no disabled_reason");
    assert_eq!(hb.consecutive_failures, 0, "{when}: a missing file is not a failure");
}

/// The field case: the whole workspace folder went away for a while (a
/// read failure for every file in it), then came back. The fire that came
/// due in between is skipped, the heartbeat stays on, and the next tick
/// after the folder is back fires.
#[test]
fn folder_gone_for_a_fire_skips_and_the_next_readable_fire_runs() {
    let ws = seed("gone");
    std::fs::rename(&ws.dir, ws.away()).expect("move the folder away");
    let during = tick(&ws);
    assert!(
        !ws.dir.exists(),
        "K2 must not recreate a workspace folder that went away"
    );
    std::fs::rename(ws.away(), &ws.dir).expect("put the folder back");

    assert_eq!(during, Vec::<String>::new(), "nothing fires while the file can't be read");
    assert_on(&ws, "after a fire with the folder gone");
    assert_eq!(audit_count(&ws, "wakeup_file_missing"), 0, "the old auto-disable row is gone");

    assert_eq!(tick(&ws), vec!["hb".to_string()], "the next readable fire runs");
    assert_on(&ws, "after the folder is back");
}

/// NotFound on one or two fires (the folder is there), then the file is
/// back: never disabled, one notice, one line when it is back, and the
/// heartbeat fires again.
#[test]
fn missing_for_two_fires_then_back_never_disables() {
    let ws = seed("two");
    std::fs::remove_file(ws.wakeup()).expect("delete WAKEUP.md");

    assert_eq!(tick(&ws), Vec::<String>::new(), "first fire skipped");
    assert_on(&ws, "after one missed fire");
    next_slot(&ws);
    assert_eq!(tick(&ws), Vec::<String>::new(), "second fire skipped");
    assert_on(&ws, "after two missed fires");
    assert_eq!(audit_count(&ws, "skipped_wakeup_waiting"), 2, "one row per skipped fire");

    let notices = inbox_texts(&ws);
    assert_eq!(notices.len(), 1, "one notice for the outage: {notices:?}");
    assert!(notices[0].contains("WAKEUP.md not found"), "{}", notices[0]);
    assert!(notices[0].contains("keeps retrying"), "{}", notices[0]);

    std::fs::write(ws.wakeup(), BODY).expect("restore WAKEUP.md");
    assert_eq!(tick(&ws), vec!["hb".to_string()], "fires as soon as the file is back");
    assert_on(&ws, "after the file is back");
    let notices = inbox_texts(&ws);
    assert_eq!(notices.len(), 2, "one notice + one back line: {notices:?}");
    assert!(notices[1].contains("is back"), "{}", notices[1]);
    assert_eq!(audit_count(&ws, "wakeup_back"), 1);
}

/// A long outage (17 scheduled fires) never disables: one notice, the
/// list says what it is waiting for, one line when it is back, and fires
/// resume.
#[test]
fn long_outage_one_notice_one_recovery_and_fires_resume() {
    let ws = seed("long");
    std::fs::remove_file(ws.wakeup()).expect("delete WAKEUP.md");

    for i in 0..17 {
        if i > 0 {
            next_slot(&ws);
        }
        assert_eq!(tick(&ws), Vec::<String>::new(), "fire {i} skipped");
        assert_on(&ws, &format!("after skipped fire {i}"));
    }
    assert_eq!(audit_count(&ws, "skipped_wakeup_waiting"), 17);
    assert_eq!(inbox_texts(&ws).len(), 1, "at most one notice per outage");

    let hb = listed(&ws);
    assert_eq!(hb.wait_reason.as_deref(), Some("wakeup_waiting"));
    let detail = hb.wait_detail.as_deref().expect("wait detail");
    assert!(detail.contains("17 fires skipped"), "{detail}");
    assert!(detail.contains("since "), "{detail}");

    std::fs::write(ws.wakeup(), BODY).expect("restore WAKEUP.md");
    assert_eq!(tick(&ws), vec!["hb".to_string()], "fires resume");
    let notices = inbox_texts(&ws);
    assert_eq!(notices.len(), 2, "one notice + one back line: {notices:?}");
    assert!(notices[1].contains("17"), "the back line counts the skipped fires: {}", notices[1]);
    assert_eq!(audit_count(&ws, "wakeup_back"), 1);
    assert_ne!(listed(&ws).wait_reason.as_deref(), Some("wakeup_waiting"));
}
