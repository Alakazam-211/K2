//! prd-daemon-activity-and-thread-working-v1 S2: the activity store and
//! the `workspace_sessions.status` session lock (DA30, DA32 / A14).
//!
//! - T-S2f: boot releases every stale `running` / `permission` lock and
//!   every `active_terminal_id` before the first heartbeat tick.
//! - T-S2g: a missed `Stop` followed by the `idle_prompt` Notification
//!   releases the lock, and `is_agent_locked` returns false.
//! - T-S2h: the legacy scheduler tick with inbox items and a live,
//!   unconfirmed session does not fire again (no second wake inject); the
//!   same tick fires once a turn end released the lock.
//! - T-S2i: a pinned `agent-chat:<pid>` row is `sleeping` after its PTY
//!   exits; the liveness sweep releases a session whose PTY died.
//!
//! No agent CLI runs: the PTYs are `cat`, the hook envelopes synthetic.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};

use k2_core::activity::Evidence;
use k2_core::agent_hooks::envelope::{self, HookEnvelope, HookHeaders, HookSource};
use k2_core::db::schema::WorkspaceSession;
use k2_core::terminal::{DaemonPtyConfig, DaemonPtySession};
use k2_daemon::activity_store::{self, SessionFacts};
use k2_daemon::v2_session_map;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    let g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    k2_core::test_isolation::assert_no_prod_env();
    g
}

fn tmp_dir(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "k2-act-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock").as_nanos()
    ));
    std::fs::create_dir_all(&p).expect("tmp dir");
    p
}

/// A workspace with a primary agent (`ROLE.md` name `scout`) and the
/// legacy scheduler armed (`heartbeat_mode = heartbeat`).
fn seed_project(id: &str, path: &Path) {
    let agent = path.join(".k2/agent");
    std::fs::create_dir_all(&agent).expect("agent dir");
    std::fs::write(agent.join("ROLE.md"), "---\nname: scout\ntype: custom\n---\n# scout\n").expect("ROLE.md");
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT OR REPLACE INTO projects (id, path, name, color, agent_mode, pinned, tab_order, heartbeat_mode) \
         VALUES (?1, ?2, 'activity-test', '#123456', 'manager', 0, 0, 'heartbeat')",
        rusqlite::params![id, path.to_string_lossy().as_ref()],
    )
    .expect("insert project");
}

fn session_row(project_id: &str, terminal_id: &str, active: Option<&str>, status: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    WorkspaceSession::upsert(&conn, &uuid::Uuid::new_v4().to_string(), project_id, Some(terminal_id), None, "claude", "system", status)
        .expect("upsert workspace_sessions");
    conn.execute(
        "UPDATE workspace_sessions SET active_terminal_id = ?1 WHERE project_id = ?2",
        rusqlite::params![active, project_id],
    )
    .expect("stamp active_terminal_id");
}

fn status_of(project_id: &str) -> (String, Option<String>) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row = WorkspaceSession::get(&conn, project_id).expect("query").expect("workspace_sessions row");
    (row.status, row.active_terminal_id)
}

fn hook(pane: &str, body: &str) -> HookEnvelope {
    envelope::parse(
        &HookHeaders {
            pane: pane.to_string(),
            agent_pid: Some(4242),
            source: HookSource::Claude,
            hook_version: Some(2),
            cli_version: Some("2.1.292".into()),
            truncated: false,
            event_hint: None,
        },
        body.as_bytes(),
    )
    .expect("synthetic hook parses")
}

fn apply(sid: &str, body: &str, at: i64) {
    activity_store::apply_at(sid, Evidence::Hook(&hook(sid, body)), at);
}

fn launchable(path: &Path) -> Vec<String> {
    k2_core::workspace::scheduler::k2so_agents_scheduler_tick(path.to_string_lossy().into_owned())
        .expect("scheduler tick")
}

fn spawn_cat(cwd: &Path, key: &str) -> Arc<DaemonPtySession> {
    let cfg = DaemonPtyConfig {
        cols: 80,
        rows: 24,
        cwd: Some(cwd.to_path_buf()),
        program: Some("cat".to_string()),
        ..DaemonPtyConfig::default()
    };
    let session = DaemonPtySession::spawn(cfg).expect("spawn cat PTY");
    v2_session_map::register(key.to_string(), Arc::clone(&session));
    session
}

/// T-S2f.
#[test]
fn boot_releases_stale_locks_and_active_terminals() {
    let _g = lock();
    let _db = k2_core::db::init_for_tests();
    let a = format!("act-boot-a-{}", uuid::Uuid::new_v4());
    let b = format!("act-boot-b-{}", uuid::Uuid::new_v4());
    let c = format!("act-boot-c-{}", uuid::Uuid::new_v4());
    for (id, status) in [(&a, "running"), (&b, "permission"), (&c, "sleeping")] {
        let dir = tmp_dir("boot");
        seed_project(id, &dir);
        session_row(id, "agent-chat:x", Some("dead-pty"), status);
    }
    let released = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        activity_store::boot_reset_status(&conn)
    };
    assert!(released >= 2, "released {released}");
    for id in [&a, &b, &c] {
        assert_eq!(status_of(id), ("sleeping".to_string(), None), "{id}");
    }
}

/// T-S2g.
#[test]
fn idle_prompt_after_a_missed_stop_releases_the_lock() {
    let _g = lock();
    let _db = k2_core::db::init_for_tests();
    activity_store::clear_for_tests();
    let pid = format!("act-g-{}", uuid::Uuid::new_v4());
    let dir = tmp_dir("g");
    seed_project(&pid, &dir);
    let sid = uuid::Uuid::new_v4().to_string();
    session_row(&pid, &sid, Some(&sid), "running");
    activity_store::register(SessionFacts {
        session_id: sid.clone(),
        agent_name: pid.clone(),
        cwd: Some(dir.to_string_lossy().into_owned()),
        program: Some("claude".into()),
    });
    let path = dir.to_string_lossy().into_owned();
    assert!(k2_core::workspace::scheduler::is_agent_locked(&path, "scout"));

    apply(&sid, r#"{"hook_event_name":"UserPromptSubmit","prompt_id":"p1"}"#, 1_000);
    apply(&sid, r#"{"hook_event_name":"PreToolUse","prompt_id":"p1","tool_name":"Bash","tool_use_id":"t1","tool_input":{}}"#, 1_100);
    apply(&sid, r#"{"hook_event_name":"PostToolUse","prompt_id":"p1","tool_name":"Bash","tool_use_id":"t1","tool_input":{}}"#, 1_200);
    // The Stop is lost. The turn stays working and the lock holds.
    activity_store::tick_all(50_000);
    assert_eq!(status_of(&pid).0, "running");
    assert!(k2_core::workspace::scheduler::is_agent_locked(&path, "scout"));

    apply(&sid, r#"{"hook_event_name":"Notification","notification_type":"idle_prompt"}"#, 61_200);
    let row = activity_store::row_json(&sid).expect("row");
    assert_eq!(row["display"], "idle");
    assert_eq!(row["reason"], "idle_prompt");
    assert_eq!(status_of(&pid).0, "sleeping");
    assert!(!k2_core::workspace::scheduler::is_agent_locked(&path, "scout"));
    activity_store::unregister(&sid);
}

/// T-S2h.
#[test]
fn legacy_scheduler_never_re_wakes_a_live_unconfirmed_session() {
    let _g = lock();
    let _db = k2_core::db::init_for_tests();
    activity_store::clear_for_tests();
    let pid = format!("act-h-{}", uuid::Uuid::new_v4());
    let dir = tmp_dir("h");
    seed_project(&pid, &dir);
    k2_core::inbox::compose(&dir, "Synthetic task", "Body.", None, None, None).expect("inbox item");
    // The canonical session was claimed (`k2so_agents_lock` → running)
    // and is live, but no evidence has arrived (a daemon restart, or a
    // CLI that hasn't spoken yet).
    let sid = uuid::Uuid::new_v4().to_string();
    session_row(&pid, &sid, Some(&sid), "running");
    activity_store::register(SessionFacts {
        session_id: sid.clone(),
        agent_name: pid.clone(),
        cwd: Some(dir.to_string_lossy().into_owned()),
        program: Some("claude".into()),
    });
    // Time passes: every timer fires, a key is pressed. Unconfirmed rows
    // never write, so the lock holds and no tick fires a second wake.
    for t in [1_000, 120_000, 3 * 3600 * 1000] {
        activity_store::tick_all(t);
        activity_store::apply_at(&sid, Evidence::Key(k2_core::activity::KeyInput::CtrlC), t);
        assert_eq!(status_of(&pid).0, "running", "t={t}");
        assert!(launchable(&dir).is_empty(), "the scheduler must skip a locked live session (t={t})");
    }
    assert_eq!(activity_store::row_json(&sid).expect("row")["reason"], "unconfirmed");

    // Control: a confirmed turn end releases the lock, and the same tick
    // now fires.
    apply(&sid, r#"{"hook_event_name":"UserPromptSubmit","prompt_id":"p1"}"#, 4 * 3600 * 1000);
    assert_eq!(status_of(&pid).0, "running");
    apply(&sid, r#"{"hook_event_name":"Stop","prompt_id":"p1","background_tasks":[],"session_crons":[]}"#, 4 * 3600 * 1000 + 500);
    assert_eq!(status_of(&pid).0, "sleeping");
    assert_eq!(launchable(&dir), vec!["scout".to_string()]);
    activity_store::unregister(&sid);
}

/// T-S2i (A14 d): the pinned chat's row is keyed `agent-chat:<pid>` with
/// the live PTY in `active_terminal_id`; it must not stay `running`
/// after the PTY goes away. And the 10 s liveness sweep releases a
/// session whose PTY died without an unregister yet.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pinned_agent_chat_row_sleeps_after_its_pty_exits() {
    let _g = lock();
    let _db = k2_core::db::init_for_tests();
    activity_store::clear_for_tests();
    v2_session_map::clear_for_tests();
    let pid = format!("act-i-{}", uuid::Uuid::new_v4());
    let dir = tmp_dir("i");
    seed_project(&pid, &dir);

    let pinned = spawn_cat(&dir, &pid);
    let sid = pinned.session_id.to_string();
    session_row(&pid, &format!("agent-chat:{pid}"), Some(&sid), "running");
    assert_eq!(
        activity_store::row_json(&sid).expect("register created the row")["reason"],
        "unconfirmed"
    );
    pinned.kill();
    drop(v2_session_map::unregister(&pid));
    assert_eq!(status_of(&pid), ("sleeping".to_string(), None));
    assert!(activity_store::row_json(&sid).is_none(), "unregister removes the row");

    // The sweep: a confirmed working session whose PTY died.
    let pid2 = format!("act-i2-{}", uuid::Uuid::new_v4());
    let dir2 = tmp_dir("i2");
    seed_project(&pid2, &dir2);
    let tab = spawn_cat(&dir2, "tab-act-sweep");
    let sid2 = tab.session_id.to_string();
    session_row(&pid2, &sid2, None, "running");
    let now = chrono::Utc::now().timestamp_millis();
    apply(&sid2, r#"{"hook_event_name":"UserPromptSubmit","prompt_id":"p1"}"#, now);
    tab.kill();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while tab.is_child_alive() {
        assert!(std::time::Instant::now() < deadline, "cat never exited");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    activity_store::sweep_liveness();
    let row = activity_store::row_json(&sid2).expect("row");
    assert_eq!(row["reason"], "pty_exited");
    assert_eq!(status_of(&pid2).0, "sleeping");
    drop(v2_session_map::unregister("tab-act-sweep"));
    v2_session_map::clear_for_tests();
}
