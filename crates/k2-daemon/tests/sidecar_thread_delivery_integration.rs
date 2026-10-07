//! Thread / `k2 msg` delivery to a sidecar address (`ws/1`) lands in THAT
//! sidecar's PTY, never in another workspace's live session that holds the
//! same provider conversation id.
//!
//! Field case (0.45.0 smoke, z3mbpZ, 2026-10-07): a Cortana tab continued
//! the nested ProposalWriter workspace's pinned Chat
//! (`claude --fork-session --resume <id>`). Registration stamped the source
//! id on the Cortana tab row, so its handle `cortanax/1` keys on the
//! ProposalWriter conversation. The project-blind live lookup then sent the
//! Thread message to ProposalWriter's pinned Chat (it answered "you sent it
//! on the cortanax thread"), not the Cortana sidecar in split view.
//!
//! Headless: in-memory DB, temp `$HOME`, `claude` is a `cat` shim under
//! `K2_TEST_AGENT_SHIM_DIR`; no real CLI and no real `~/.claude`.

#![cfg(unix)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};

use k2_core::session::SessionId;
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
    let home = PathBuf::from(format!("/tmp/k2st-{:x}", nanos % 0xffff_ffff));
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

struct Ws {
    id: String,
    path: PathBuf,
}

fn seed_ws(path: PathBuf, handle: &str) -> Ws {
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
    Ws { id, path }
}

fn spawn(ws: &Ws, key: &str, args: &[&str]) -> SessionId {
    spawn_agent_session_v2_blocking(SpawnWorkspaceSessionRequest {
        agent_name: key.to_string(),
        project_id: Some(ws.id.clone()),
        cwd: ws.path.to_string_lossy().into_owned(),
        command: Some("claude".to_string()),
        args: Some(args.iter().map(|s| s.to_string()).collect()),
        cols: 120,
        rows: 30,
        canonical_key: Some(key.to_string()),
        env: HashMap::new(),
        launch_prompt: None,
        label: None,
    })
    .unwrap_or_else(|e| panic!("spawn {key}: {e}"))
    .session_id
}

fn stop(key: &str) {
    let s = k2_daemon::v2_session_map::unregister(key)
        .unwrap_or_else(|| panic!("{key} was not live"));
    s.kill();
}

/// The other workspace's pinned Chat, live on `conv`.
fn pin_and_spawn_chat(ws: &Ws, conv: &str) -> SessionId {
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO workspace_sessions (id, project_id, session_id, harness, owner, status, created_at) \
             VALUES (?1, ?2, ?3, 'claude', 'user', 'running', unixepoch())",
            params![uuid::Uuid::new_v4().to_string(), ws.id, conv],
        )
        .expect("pin chat");
    }
    spawn(ws, &ws.id, &["--resume", conv])
}

/// Stamp the provider id on the sidecar's tab row and key its handle on it
/// (what registration does for a `--resume <conv>` tab).
fn stamp_sidecar(ws: &Ws, pane: &str, conv: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::db::schema::WorkspaceTabSession::stamp_session_id(&conn, &ws.id, pane, conv)
        .expect("stamp tab session id");
    k2_core::workspace_session_handles::ensure_sidecar_handle(
        &conn,
        &ws.id,
        &format!("tab-{pane}"),
        Some("claude"),
        Some(conv),
        pane,
    )
    .expect("sidecar handle")
    .expect("a tab-* claude session is a sidecar")
}

fn grid_has(key: &str, marker: &str) -> bool {
    k2_daemon::v2_session_map::lookup_by_agent_name(key)
        .is_some_and(|s| s.visible_text_rows().iter().any(|r| r.contains(marker)))
}

fn wait_grid(key: &str, marker: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if grid_has(key, marker) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let rows = k2_daemon::v2_session_map::lookup_by_agent_name(key)
        .map(|s| s.visible_text_rows())
        .unwrap_or_default();
    panic!("{marker:?} never reached {key}; grid={rows:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sidecar_address_delivers_to_its_own_pty_not_another_workspaces_chat() {
    let env = setup();
    let parent = seed_ws(env.home.join("cortanax"), "cortanax");
    // Nested workspace (ProposalWriter under Cortana): its own project.
    let nested = seed_ws(env.home.join("cortanax").join("writer"), "writer");
    let conv = uuid::Uuid::new_v4().to_string();

    let chat_sid = pin_and_spawn_chat(&nested, &conv);

    // (1) The sidecar's live argv names a different id; only its tab row
    // carries `conv`. The other workspace's Chat is the only argv match, so
    // the project-blind scan picks it every time.
    let pane = uuid::Uuid::new_v4().to_string();
    let tab_key = format!("tab-{pane}");
    let own_sid = spawn(&parent, &tab_key, &[]);
    let handle = stamp_sidecar(&parent, &pane, &conv);
    let addr = format!("cortanax/{handle}");

    let r = workspace_msg::deliver_live_with_via(
        &addr,
        "row-only",
        "owner",
        "",
        true,
        Duration::from_millis(500),
        "compose",
    );
    assert!(r.success, "deliver to {addr}: reason={:?} hint={:?}", r.reason, r.hint);
    assert_eq!(
        r.target_session_id.as_deref(),
        Some(own_sid.to_string().as_str()),
        "{addr} must inject into its own sidecar PTY ({own_sid}), not the nested workspace's pinned Chat ({chat_sid}); branch={:?}",
        r.branch
    );

    // (2) The field shape: the sidecar continued that Chat, so BOTH live
    // argvs reference `conv`.
    stop(&tab_key);
    let own_sid = spawn(&parent, &tab_key, &["--fork-session", "--resume", &conv]);
    for i in 0..8 {
        let r = workspace_msg::deliver_live_with_via(
            &addr,
            &format!("fork-{i}"),
            "owner",
            "",
            true,
            Duration::from_millis(500),
            "compose",
        );
        assert!(r.success, "deliver {i} to {addr}: reason={:?}", r.reason);
        assert_eq!(
            r.target_session_id.as_deref(),
            Some(own_sid.to_string().as_str()),
            "send {i}: {addr} must reach its own sidecar ({own_sid}), not {chat_sid}"
        );
    }

    // (3) The Thread pane's send: `/cli/thread/post` via=compose on the
    // sidecar address types into the sidecar's grid only.
    let marker = format!("thread-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let mut p = HashMap::new();
    p.insert("addr".to_string(), addr.clone());
    p.insert("text".to_string(), marker.clone());
    p.insert("via".to_string(), "compose".to_string());
    let resp = k2_daemon::overlay_routes::dispatch_post("/cli/thread/post", &p, b"");
    assert!(resp.status.starts_with("200"), "thread post: {} {}", resp.status, resp.body);
    wait_grid(&tab_key, &marker);
    assert!(
        !grid_has(&nested.id, &marker),
        "the Thread send for {addr} must not reach the nested workspace's pinned Chat"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dormant_sidecar_wake_uses_its_own_workspaces_tab_row() {
    let env = setup();
    let parent = seed_ws(env.home.join("cortanay"), "cortanay");
    let nested = seed_ws(env.home.join("cortanay").join("writer2"), "writer2");
    let conv = uuid::Uuid::new_v4().to_string();

    // The nested workspace also has a (newer) tab row on `conv`, dormant.
    let nested_pane = uuid::Uuid::new_v4().to_string();
    let nested_key = format!("tab-{nested_pane}");
    let pane = uuid::Uuid::new_v4().to_string();
    let tab_key = format!("tab-{pane}");
    spawn(&parent, &tab_key, &["--fork-session", "--resume", &conv]);
    let handle = stamp_sidecar(&parent, &pane, &conv);
    stop(&tab_key);
    std::thread::sleep(Duration::from_millis(1100));
    spawn(&nested, &nested_key, &["--resume", &conv]);
    stop(&nested_key);
    let addr = format!("cortanay/{handle}");

    let r = workspace_msg::deliver_live_with_via(
        &addr,
        "wake-me",
        "owner",
        "",
        true,
        Duration::from_millis(500),
        "compose",
    );
    assert!(r.success, "wake {addr}: reason={:?} hint={:?}", r.reason, r.hint);
    let woken = k2_daemon::v2_session_map::lookup_by_agent_name(&tab_key)
        .unwrap_or_else(|| panic!("{addr} must wake its own tab {tab_key}"));
    assert_eq!(
        r.target_session_id.as_deref(),
        Some(woken.session_id.to_string().as_str())
    );
    assert!(
        k2_daemon::v2_session_map::lookup_by_agent_name(&nested_key).is_none(),
        "waking {addr} must not respawn the nested workspace's tab"
    );
    assert_eq!(
        woken.cwd.as_deref(),
        Some(parent.path.as_path()),
        "the woken sidecar runs in its own workspace"
    );
}
