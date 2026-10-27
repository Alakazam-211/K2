//! S31 K23: the Sew delivery hub's injector types into the agent's own PTY
//! through `k2 msg`'s throat (`via = chat`) and records NO Chatter line
//! (chat text never goes on disk). The same throat with `via = msg` does
//! record one, so the check isn't vacuous.
//!
//! Headless: in-memory DB, temp `$HOME`, `claude` is a `cat` shim under
//! `K2_TEST_AGENT_SHIM_DIR`; no real CLI, no real `~/.claude`, no Sew.

#![cfg(unix)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};

use k2_daemon::sew_chat::hub::{Injector, LiveInjector, FROM, VIA};
use k2_daemon::spawn::{spawn_agent_session_v2_blocking, SpawnWorkspaceSessionRequest};
use k2_daemon::workspace_msg;
use rusqlite::params;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

struct TestEnv {
    home: PathBuf,
    prev_home: Option<std::ffi::OsString>,
    prev_shim: Option<std::ffi::OsString>,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        for (key, s) in k2_daemon::v2_session_map::snapshot() {
            if s.cwd.as_ref().is_some_and(|p| p.starts_with(&self.home)) {
                k2_daemon::v2_session_map::unregister(&key);
                s.kill();
            }
        }
        match self.prev_home.take() {
            Some(p) => std::env::set_var("HOME", p),
            None => std::env::remove_var("HOME"),
        }
        match self.prev_shim.take() {
            Some(p) => std::env::set_var("K2_TEST_AGENT_SHIM_DIR", p),
            None => std::env::remove_var("K2_TEST_AGENT_SHIM_DIR"),
        }
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn setup() -> TestEnv {
    let guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    // Short: per-cell socket paths must fit SUN_LEN.
    let home = PathBuf::from(format!("/tmp/k2sh-{:x}", nanos % 0xffff_ffff));
    let shim_dir = home.join("shim");
    std::fs::create_dir_all(&shim_dir).expect("shim dir");
    let shim = shim_dir.join("claude");
    std::fs::write(&shim, "#!/bin/sh\nexec /bin/cat\n").expect("claude shim");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).expect("chmod shim");
    let env = TestEnv {
        prev_home: std::env::var_os("HOME"),
        prev_shim: std::env::var_os("K2_TEST_AGENT_SHIM_DIR"),
        home: home.clone(),
        _guard: guard,
    };
    std::env::set_var("HOME", &home);
    std::env::set_var("K2_TEST_AGENT_SHIM_DIR", &shim_dir);
    k2_core::db::init_for_tests();
    env
}

fn seed_ws(path: PathBuf, handle: &str) -> String {
    std::fs::create_dir_all(&path).expect("workspace dir");
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
        params![id, handle, path.to_string_lossy()],
    )
    .expect("seed project");
    conn.execute(
        "INSERT INTO workspaces (id, project_id, name) VALUES (?1, ?2, 'main')",
        params![uuid::Uuid::new_v4().to_string(), id],
    )
    .expect("seed workspaces row");
    id
}

/// The workspace's pinned Chat, live (the canonical session the hub types into).
fn pin_and_spawn(id: &str, path: &std::path::Path) {
    let conv = uuid::Uuid::new_v4().to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO workspace_sessions (id, project_id, session_id, harness, owner, status, created_at) \
             VALUES (?1, ?2, ?3, 'claude', 'user', 'running', unixepoch())",
            params![uuid::Uuid::new_v4().to_string(), id, conv],
        )
        .expect("pin chat");
    }
    spawn_agent_session_v2_blocking(SpawnWorkspaceSessionRequest {
        agent_name: id.to_string(),
        project_id: Some(id.to_string()),
        cwd: path.to_string_lossy().into_owned(),
        command: Some("claude".to_string()),
        args: Some(vec!["--resume".to_string(), conv]),
        cols: 120,
        rows: 30,
        canonical_key: Some(id.to_string()),
        env: HashMap::new(),
        env_owner_set: false,
        launch_prompt: None,
        label: None,
    })
    .unwrap_or_else(|e| panic!("spawn {id}: {e}"));
}

fn wait_grid(key: &str, marker: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if k2_daemon::v2_session_map::lookup_by_agent_name(key)
            .is_some_and(|s| s.visible_text_rows().iter().any(|r| r.contains(marker)))
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let rows = k2_daemon::v2_session_map::lookup_by_agent_name(key).map(|s| s.visible_text_rows()).unwrap_or_default();
    panic!("{marker:?} never reached {key}; grid={rows:?}");
}

fn chatter_has(marker: &str) -> bool {
    k2_core::overlay::read_chatterlog(0)
        .expect("read the chatter log")
        .iter()
        .any(|i| serde_json::to_string(i).expect("chatter item json").contains(marker))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_chat_delivery_types_into_the_agents_pty_and_records_no_chatter() {
    let env = setup();
    let path = env.home.join("chathub");
    let id = seed_ws(path.clone(), "chathub");
    pin_and_spawn(&id, &path);
    let token = path.to_string_lossy().into_owned();

    assert_eq!(VIA, "chat");
    let marker = format!("hub-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let out = LiveInjector.inject(&token, &format!("Sew chat — 1 new: {marker}"), false);
    assert!(out.delivered, "inject: {:?}", out.reason);
    wait_grid(&id, &marker);
    assert!(!chatter_has(&marker), "via=chat must not record a Chatter line (no chat text on disk)");

    // The same throat with via=msg records one: the check above is real.
    let marker2 = format!("msg-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let r = workspace_msg::deliver_live_with_via(&token, &marker2, FROM, "", false, Duration::from_millis(500), "msg");
    assert!(r.success, "msg inject: {:?} {:?}", r.reason, r.hint);
    wait_grid(&id, &marker2);
    assert!(chatter_has(&marker2), "via=msg records Chatter");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_asleep_agent_is_not_woken_without_a_wake() {
    let env = setup();
    let path = env.home.join("chatsleep");
    let id = seed_ws(path.clone(), "chatsleep");
    // A pinned Chat with no live PTY: dormant.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO workspace_sessions (id, project_id, session_id, harness, owner, status, created_at) \
             VALUES (?1, ?2, ?3, 'claude', 'user', 'running', unixepoch())",
            params![uuid::Uuid::new_v4().to_string(), id, uuid::Uuid::new_v4().to_string()],
        )
        .expect("pin chat");
    }
    let out = LiveInjector.inject(&path.to_string_lossy(), "Sew chat — a follow push", false);
    assert!(!out.delivered);
    assert_eq!(out.reason.as_deref(), Some("dormant_no_wake"));
    assert!(k2_daemon::v2_session_map::lookup_by_agent_name(&id).is_none(), "nothing was spawned");
}
