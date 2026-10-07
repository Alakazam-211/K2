//! prd-daemon-activity-and-thread-working-v1 DA32: the activity store
//! releases the session lock on turn-end evidence, so the legacy
//! `.k2/agent/work/.lock` file must not keep holding it.
//!
//! `k2so_agents_lock` writes BOTH the `workspace_sessions` row (`running`)
//! and the `.lock` file; only `k2so_agents_unlock` removes the file. Before
//! this fix `is_agent_locked` fell back to the file even when the row said
//! `sleeping`, so a workspace that was ever locked that way stayed locked
//! after the store released it: heartbeats skipped it forever.
//!
//! - A workspace with a `workspace_sessions` row: the row decides. A turn
//!   end releases the lock even though the file is still on disk.
//! - A workspace with no row (an unregistered scratch path): the file
//!   still decides, as before.
//!
//! No agent CLI runs: the hook envelopes are synthetic.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::Mutex as StdMutex;

use k2_core::activity::Evidence;
use k2_core::agent_hooks::envelope::{self, HookHeaders, HookSource};
use k2_core::workspace::scheduler::{agent_work_dir, is_agent_locked};
use k2_core::workspace::session::{k2so_agents_lock, k2so_agents_unlock};
use k2_daemon::activity_store::{self, SessionFacts};

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    let g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    k2_core::test_isolation::assert_no_prod_env();
    g
}

fn tmp_dir(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "k2-legacy-lock-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock").as_nanos()
    ));
    std::fs::create_dir_all(&p).expect("tmp dir");
    p
}

fn seed_project(id: &str, path: &Path) {
    let agent = path.join(".k2/agent");
    std::fs::create_dir_all(&agent).expect("agent dir");
    std::fs::write(agent.join("ROLE.md"), "---\nname: scout\ntype: custom\n---\n# scout\n").expect("ROLE.md");
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT OR REPLACE INTO projects (id, path, name, color, agent_mode, pinned, tab_order, heartbeat_mode) \
         VALUES (?1, ?2, 'legacy-lock-test', '#123456', 'manager', 0, 0, 'heartbeat')",
        rusqlite::params![id, path.to_string_lossy().as_ref()],
    )
    .expect("insert project");
}

fn apply(sid: &str, body: &str, at: i64) {
    let env = envelope::parse(
        &HookHeaders {
            pane: sid.to_string(),
            agent_pid: Some(4242),
            source: HookSource::Claude,
            hook_version: Some(2),
            cli_version: Some("2.1.292".into()),
            truncated: false,
            event_hint: None,
        },
        body.as_bytes(),
    )
    .expect("synthetic hook parses");
    activity_store::apply_at(sid, Evidence::Hook(&env), at);
}

#[test]
fn a_released_row_unlocks_even_with_the_legacy_lock_file_on_disk() {
    let _g = lock();
    let _db = k2_core::db::init_for_tests();
    activity_store::clear_for_tests();
    let pid = format!("legacy-lock-{}", uuid::Uuid::new_v4());
    let dir = tmp_dir("row");
    seed_project(&pid, &dir);
    let path = dir.to_string_lossy().into_owned();
    let sid = uuid::Uuid::new_v4().to_string();

    // The canonical-session claim: row `running` + the `.lock` file.
    k2so_agents_lock(path.clone(), "scout".into(), Some(sid.clone()), None).expect("lock");
    let file = agent_work_dir(&path, "scout", "").join(".lock");
    assert!(file.exists(), "k2so_agents_lock writes the legacy file");
    assert!(is_agent_locked(&path, "scout"));

    activity_store::register(SessionFacts {
        session_id: sid.clone(),
        agent_name: pid.clone(),
        cwd: Some(path.clone()),
        program: Some("claude".into()),
    });
    apply(&sid, r#"{"hook_event_name":"UserPromptSubmit","prompt_id":"p1"}"#, 1_000);
    assert!(is_agent_locked(&path, "scout"), "a working turn holds the lock");
    apply(&sid, r#"{"hook_event_name":"Stop","prompt_id":"p1","background_tasks":[],"session_crons":[]}"#, 2_000);

    assert!(file.exists(), "the store never touches the legacy file");
    assert!(
        !is_agent_locked(&path, "scout"),
        "the released row decides; the stale .lock file must not hold the lock"
    );
    // A new confirmed turn re-claims through the row.
    apply(&sid, r#"{"hook_event_name":"UserPromptSubmit","prompt_id":"p2"}"#, 3_000);
    assert!(is_agent_locked(&path, "scout"));
    activity_store::unregister(&sid);
    k2so_agents_unlock(path.clone(), "scout".into()).expect("unlock");
    assert!(!file.exists());
}

#[test]
fn a_workspace_without_a_session_row_still_reads_the_legacy_file() {
    let _g = lock();
    let _db = k2_core::db::init_for_tests();
    // Not a registered project: no `workspace_sessions` row can exist.
    let dir = tmp_dir("file-only");
    let path = dir.to_string_lossy().into_owned();
    assert!(!is_agent_locked(&path, "scout"));
    let file = agent_work_dir(&path, "scout", "").join(".lock");
    std::fs::create_dir_all(file.parent().expect("parent")).expect("work dir");
    std::fs::write(&file, "now").expect("write .lock");
    assert!(is_agent_locked(&path, "scout"), "pre-migration workspaces keep the file lock");
}
