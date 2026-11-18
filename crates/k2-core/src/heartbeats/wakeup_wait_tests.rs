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

/// Every inbox item's text (any order).
fn inbox_texts(ws: &Ws) -> Vec<String> {
    let items = crate::inbox::list_root(&ws.dir);
    items
        .iter()
        .map(|i| {
            let p = crate::inbox::inbox_root(&ws.dir).join(&i.filename);
            std::fs::read_to_string(&p).expect("read inbox item")
        })
        .collect()
}

/// How many of `texts` contain `needle`.
fn with(texts: &[String], needle: &str) -> usize {
    texts.iter().filter(|t| t.contains(needle)).count()
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
    assert_eq!(with(&notices, "WAKEUP.md not found"), 1, "{notices:?}");
    assert_eq!(with(&notices, "keeps retrying"), 1, "{notices:?}");

    std::fs::write(ws.wakeup(), BODY).expect("restore WAKEUP.md");
    assert_eq!(tick(&ws), vec!["hb".to_string()], "fires as soon as the file is back");
    assert_on(&ws, "after the file is back");
    let notices = inbox_texts(&ws);
    assert_eq!(notices.len(), 2, "one notice + one back line: {notices:?}");
    assert_eq!(with(&notices, "is back"), 1, "{notices:?}");
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
    assert_eq!(with(&notices, "17 fires skipped"), 1, "the back line counts the skipped fires: {notices:?}");
    assert_eq!(audit_count(&ws, "wakeup_back"), 1);
    assert_ne!(listed(&ws).wait_reason.as_deref(), Some("wakeup_waiting"));
}

// ── New-API tests (the fault seam and the Thread fallback) ─────────────

use super::wakeup_wait;

/// A transient read error (permission denied, an I/O error) skips the fire
/// and keeps the schedule on. No notice for a short blip; one once it has
/// lasted the grace; the next readable fire runs.
#[test]
fn unreadable_wakeup_skips_keeps_on_and_tells_once_after_the_grace() {
    for (i, kind) in [std::io::ErrorKind::PermissionDenied, std::io::ErrorKind::Other].into_iter().enumerate() {
        let ws = seed(&format!("fault{i}"));
        wakeup_wait::set_read_fault(Some((ws.wakeup(), kind)));
        assert_eq!(tick(&ws), Vec::<String>::new(), "{kind:?}: skipped");
        assert_on(&ws, &format!("{kind:?}: first skip"));
        assert_eq!(inbox_texts(&ws).len(), 0, "{kind:?}: a short blip says nothing");
        let hb = listed(&ws);
        assert_eq!(hb.wait_reason.as_deref(), Some("wakeup_waiting"), "{kind:?}");
        assert!(hb.wait_detail.as_deref().unwrap_or_default().contains("can't be read"), "{kind:?}: {:?}", hb.wait_detail);

        // The outage has lasted past the grace.
        {
            let db = crate::db::shared();
            let conn = db.lock();
            let earlier = (chrono::Local::now()
                - chrono::Duration::seconds(wakeup_wait::UNREADABLE_NOTICE_AFTER_SECS + 60))
            .to_rfc3339();
            conn.execute(
                "UPDATE heartbeat_fires SET fired_at = ?1 WHERE project_id = ?2 AND decision = ?3",
                rusqlite::params![earlier, ws.project_id, wakeup_wait::DECISION_SKIPPED_WAKEUP],
            )
            .unwrap();
        }
        assert_eq!(tick(&ws), Vec::<String>::new(), "{kind:?}: still skipped");
        assert_eq!(tick(&ws), Vec::<String>::new(), "{kind:?}: still skipped");
        assert_on(&ws, &format!("{kind:?}: after the grace"));
        let notices = inbox_texts(&ws);
        assert_eq!(notices.len(), 1, "{kind:?}: one notice: {notices:?}");
        assert_eq!(with(&notices, "can't be read"), 1, "{notices:?}");
        // Three ticks; a minute boundary may pass between them, so count
        // slots, not ticks: every skipped row names a different slot.
        let (rows, slots): (i64, i64) = {
            let db = crate::db::shared();
            let conn = db.lock();
            conn.query_row(
                "SELECT COUNT(*), COUNT(DISTINCT substr(reason, 1, instr(reason, ': '))) FROM heartbeat_fires \
                 WHERE project_id = ?1 AND decision = ?2",
                rusqlite::params![ws.project_id, wakeup_wait::DECISION_SKIPPED_WAKEUP],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap()
        };
        assert_eq!(rows, slots, "{kind:?}: one row per slot, not per tick");
        assert!(rows < 3, "{kind:?}: three ticks wrote {rows} rows");

        wakeup_wait::set_read_fault(None);
        assert_eq!(tick(&ws), vec!["hb".to_string()], "{kind:?}: the next readable fire runs");
        assert_eq!(with(&inbox_texts(&ws), "is back"), 1, "{kind:?}");
        assert_on(&ws, &format!("{kind:?}: back"));
    }
}

static THREAD_NOTES: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());

fn record_thread_note(project_path: &str, code: &str, text: &str) -> Result<(), String> {
    let want = if text.contains("is back to normal") { wakeup_wait::NOTE_BACK } else { wakeup_wait::NOTE_WAITING };
    assert_eq!(code, want, "the Thread note's notice code: {text}");
    THREAD_NOTES.lock().expect("notes lock").push((project_path.to_string(), text.to_string()));
    Ok(())
}

fn thread_notes(ws: &Ws) -> Vec<String> {
    THREAD_NOTES
        .lock()
        .expect("notes lock")
        .iter()
        .filter(|(p, _)| *p == ws.path)
        .map(|(_, t)| t.clone())
        .collect()
}

/// The field case end to end: the workspace folder is moved away for a
/// long time. The heartbeat stays on, the one notice goes to the Thread
/// (the inbox lives in the folder that is gone, and K2 must not recreate
/// it), the "back" line follows it there, and fires resume.
#[test]
fn folder_moved_away_long_outage_tells_the_thread_once_and_resumes() {
    wakeup_wait::set_thread_sink(record_thread_note);
    let ws = seed("moved");
    std::fs::rename(&ws.dir, ws.away()).expect("move the folder away");
    for i in 0..5 {
        if i > 0 {
            next_slot(&ws);
        }
        if i == 2 {
            // Past the grace for a read error.
            let db = crate::db::shared();
            let conn = db.lock();
            let earlier = (chrono::Local::now() - chrono::Duration::minutes(76)).to_rfc3339();
            conn.execute(
                "UPDATE heartbeat_fires SET fired_at = ?1 WHERE project_id = ?2 AND decision = ?3",
                rusqlite::params![earlier, ws.project_id, wakeup_wait::DECISION_SKIPPED_WAKEUP],
            )
            .unwrap();
        }
        assert_eq!(tick(&ws), Vec::<String>::new(), "fire {i} skipped");
        assert_on(&ws, &format!("fire {i}"));
    }
    assert!(!ws.dir.exists(), "K2 must not recreate the folder (no inbox written there)");
    let notes = thread_notes(&ws);
    assert_eq!(notes.len(), 1, "one notice per outage: {notes:?}");
    assert_eq!(with(&notes, "workspace folder not found"), 1, "{notes:?}");
    assert_eq!(audit_count(&ws, wakeup_wait::DECISION_WAKEUP_NOTICE), 1);

    std::fs::rename(ws.away(), &ws.dir).expect("put the folder back");
    assert_eq!(tick(&ws), vec!["hb".to_string()], "fires resume");
    let notes = thread_notes(&ws);
    assert_eq!(notes.len(), 2, "one notice + one back line: {notes:?}");
    assert_eq!(with(&notes, "5 fires skipped"), 1, "{notes:?}");
    assert_on(&ws, "back");
}
