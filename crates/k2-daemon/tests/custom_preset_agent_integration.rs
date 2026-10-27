//! GL44 / GL45 (prd-gardens-local-services-v1 Appendix A, asks A and B):
//! a custom-preset agent (a no-model program, appa's `board-keeper`) gets
//! a pinned Thread conversation and a supported lifecycle hook.
//!
//! - `k2 thread <custom-agent>` resolves for reading and posting, before
//!   and after the first launch, and keeps one id across launches.
//! - A Thread post reaches the program's stdin as
//!   `[from …] [thread:<addr>] <text>` (a recorder program logs stdin).
//! - A workspace that moved Claude → custom does not reuse the stale
//!   Claude id.
//! - `k2 hooks emit` (the real `cli/k2`) run by the program inside its PTY
//!   is accepted on the session's own passport and cell socket, claims as
//!   the PTY child even from a grandchild shell; the same passport from a
//!   process outside the PTY is not counted (exit 1).
//!
//! Headless: in-memory DB, a short temp `$HOME`, the agent is a `/bin/sh`
//! shim under `K2_TEST_AGENT_SHIM_DIR`; no LLM CLI is ever started.

#![cfg(unix)]

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};

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

fn k2_cli() -> PathBuf {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../cli/k2");
    assert!(p.is_file(), "cli/k2 missing at {}", p.display());
    dunce::canonicalize(&p).expect("canonical cli path")
}

/// `board-keeper`: emits its lifecycle through the real CLI (once
/// directly, once from a grandchild `sh -c`), then logs every stdin line.
fn keeper_script(home: &Path) -> String {
    let cli = k2_cli();
    format!(
        "#!/bin/sh\n\
         K2CLI='{cli}'\n\
         \"$K2CLI\" hooks emit SessionStart --json '{{\"source\":\"startup\"}}' > '{home}/emit1.out' 2>&1\n\
         echo \"rc=$?\" >> '{home}/emit1.out'\n\
         /bin/sh -c \"'$K2CLI' hooks emit UserPromptSubmit\" > '{home}/emit2.out' 2>&1\n\
         echo \"rc=$?\" >> '{home}/emit2.out'\n\
         while IFS= read -r line; do printf '%s\\n' \"$line\" >> '{home}/keeper.log'; done\n",
        cli = cli.display(),
        home = home.display(),
    )
}

fn setup() -> TestEnv {
    let guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    // Short: per-cell socket paths must fit SUN_LEN.
    let home = PathBuf::from(format!("/tmp/k2cp-{:x}", nanos % 0xffff_ffff));
    let shim_dir = home.join("shim");
    std::fs::create_dir_all(home.join(".k2")).expect("k2 home");
    std::fs::create_dir_all(&shim_dir).expect("shim dir");
    let shim = shim_dir.join("board-keeper");
    std::fs::write(&shim, keeper_script(&home)).expect("keeper shim");
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).expect("chmod shim");
    let env = TestEnv {
        prev_home: std::env::var_os("HOME"),
        prev_shim: std::env::var_os("K2_TEST_AGENT_SHIM_DIR"),
        home: home.clone(),
        _guard: guard,
    };
    std::env::set_var("HOME", &home);
    std::env::set_var("K2_TEST_AGENT_SHIM_DIR", &shim_dir);
    k2_core::test_isolation::assert_isolated_from_prod();
    k2_core::db::init_for_tests();
    env
}

struct Ws {
    id: String,
    path: PathBuf,
}

/// A workspace whose default agent is the `board-keeper` custom preset.
fn seed_custom_ws(home: &Path, handle: &str) -> Ws {
    let path = home.join(handle);
    std::fs::create_dir_all(&path).expect("workspace dir");
    let id = uuid::Uuid::new_v4().to_string();
    let preset = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO agent_presets (id, label, command, icon, enabled, sort_order, is_built_in) \
         VALUES (?1, ?2, 'board-keeper', '', 1, 990, 0)",
        params![preset, format!("keeper-{preset}")],
    )
    .expect("seed preset");
    conn.execute(
        "INSERT INTO projects (id, name, path, handle, default_agent) VALUES (?1, ?2, ?3, ?2, ?4)",
        params![id, handle, path.to_string_lossy(), preset],
    )
    .expect("seed project");
    conn.execute(
        "INSERT INTO workspaces (id, project_id, name) VALUES (?1, ?2, 'main')",
        params![uuid::Uuid::new_v4().to_string(), id],
    )
    .expect("seed workspaces row");
    Ws { id, path }
}

fn pinned_row(project_id: &str) -> (Option<String>, String) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row = k2_core::db::schema::WorkspaceSession::get(&conn, project_id)
        .expect("read row")
        .expect("a workspace_sessions row");
    (row.session_id, row.harness)
}

fn get_thread(addr: &str) -> serde_json::Value {
    let mut p = HashMap::new();
    p.insert("addr".to_string(), addr.to_string());
    let resp = k2_daemon::overlay_routes::dispatch("/cli/thread", &p).expect("thread route");
    assert!(resp.status.starts_with("200"), "GET thread {addr}: {} {}", resp.status, resp.body);
    serde_json::from_str(&resp.body).unwrap_or_else(|e| panic!("thread JSON ({e}): {}", resp.body))
}

fn wait_file_contains(path: &Path, needle: &str, secs: u64) -> String {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        if text.contains(needle) {
            return text;
        }
        if Instant::now() > deadline {
            panic!("{needle:?} never reached {}; got {text:?}", path.display());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn custom_preset_gets_a_thread_and_reports_its_lifecycle() {
    let env = setup();
    let ws = seed_custom_ws(&env.home, "board-keeper");

    // (1) Before any launch (the field case right after the upgrade): the
    // address resolves and the id is pinned with the custom harness.
    let before = get_thread("board-keeper");
    let pin = before["conversation_id"].as_str().expect("conversation id").to_string();
    assert!(uuid::Uuid::parse_str(&pin).is_ok(), "K2-minted v4 id, got {pin:?}");
    assert_eq!(pinned_row(&ws.id), (Some(pin.clone()), "board-keeper".to_string()));

    // (2) Launch the pinned chat: the program spawns bare (no id on its
    // argv) and the Thread keeps the same conversation.
    let path = ws.path.to_string_lossy().into_owned();
    let out = tokio::task::block_in_place(|| k2_daemon::pinned_chat::ensure_pinned_chat(&path, false, false))
        .expect("ensure pinned chat");
    assert!(out.command.ends_with("board-keeper"), "{}", out.command);
    assert!(out.args.is_empty(), "no session flags for a custom program: {:?}", out.args);
    assert_eq!(out.claude_session_id, pin, "launch keeps the pinned Thread id");
    assert_eq!(out.provider, "board-keeper");

    // (3) GL45: the program's own emits are accepted on its passport,
    // as the PTY child, even from the grandchild shell.
    let e1 = wait_file_contains(&env.home.join("emit1.out"), "rc=", 20);
    assert!(e1.contains("hooks emit: SessionStart accepted") && e1.contains("rc=0"), "{e1}");
    let e2 = wait_file_contains(&env.home.join("emit2.out"), "rc=", 20);
    assert!(e2.contains("hooks emit: UserPromptSubmit accepted") && e2.contains("rc=0"), "{e2}");
    let status = k2_daemon::hook_ingest::status_json();
    let pane = &status["sessions"][&out.session_id];
    assert_eq!(pane["owner"]["via"], "pty_child", "{status}");
    assert_eq!(pane["source"], "custom", "{status}");
    assert_eq!(pane["lastEvent"], "UserPromptSubmit", "{status}");
    assert_eq!(pane["counters"]["accepted"], 2, "{status}");
    assert_eq!(pane["counters"]["foreign"], 0, "{status}");

    // The same passport from a process OUTSIDE the PTY (this test) is not
    // counted, and the CLI says so (exit 1).
    let token = k2_daemon::session_token::mint_session_token(
        &k2_core::session::SessionId::parse(&out.session_id).expect("session id"),
        &out.session_id,
        k2_daemon::session_token::HookPrincipal {
            workspace_uuid: ws.id.clone(),
            agent_address: "board-keeper".to_string(),
        },
        k2_daemon::session_token::CredMode::ApiKey,
        k2_daemon::session_token::Provider::Anthropic,
    );
    let sock = k2_daemon::cell_uds::cell_socket_path(
        &k2_core::session::SessionId::parse(&out.session_id).expect("session id"),
    );
    let outside = std::process::Command::new(k2_cli())
        .args(["hooks", "emit", "Stop"])
        .env("HOME", &env.home)
        .env("K2_HOOK_TOKEN", &token)
        .env("K2_HOOK_SOCK", &sock)
        .env("K2_PANE_ID", &out.session_id)
        .env("K2_PROJECT_PATH", &ws.path)
        .env_remove("K2_PORT")
        .output()
        .expect("run k2 hooks emit");
    let err = String::from_utf8_lossy(&outside.stderr);
    assert_eq!(outside.status.code(), Some(1), "stdout={} stderr={err}", String::from_utf8_lossy(&outside.stdout));
    assert!(err.contains("not counted: foreign"), "{err}");
    let status = k2_daemon::hook_ingest::status_json();
    assert_eq!(status["sessions"][&out.session_id]["counters"]["foreign"], 1, "{status}");
    assert_eq!(status["sessions"][&out.session_id]["lastEvent"], "UserPromptSubmit", "{status}");

    // (4) A Thread post reaches the program's stdin as the envelope, and
    // reads back on the same conversation.
    let marker = format!("save-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let mut p = HashMap::new();
    p.insert("addr".to_string(), "board-keeper".to_string());
    p.insert("text".to_string(), marker.clone());
    p.insert("via".to_string(), "compose".to_string());
    let resp = k2_daemon::overlay_routes::dispatch_post("/cli/thread/post", &p, b"");
    assert!(resp.status.starts_with("200"), "thread post: {} {}", resp.status, resp.body);
    let log = wait_file_contains(&env.home.join("keeper.log"), &marker, 30);
    assert!(
        log.contains(&format!("[thread:board-keeper] {marker}")),
        "the program got the Thread envelope: {log:?}"
    );
    assert!(log.contains("[from "), "{log:?}");
    let after = get_thread("board-keeper");
    assert_eq!(after["conversation_id"], pin.as_str());
    let items = serde_json::to_string(&after["items"]).expect("items");
    assert!(items.contains(&marker), "{items}");

    // (5) A respawn (restart / k2 agent restart) keeps the conversation.
    let again = tokio::task::block_in_place(|| k2_daemon::pinned_chat::ensure_pinned_chat(&path, true, false))
        .expect("force respawn");
    assert_ne!(again.session_id, out.session_id, "a new PTY");
    assert_eq!(again.claude_session_id, pin, "same Thread across restarts");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_to_custom_switch_does_not_reuse_the_stale_claude_id() {
    let env = setup();
    let ws = seed_custom_ws(&env.home, "keeper-switch");
    // The field shape: the Claude conversation (its JSONL still on disk)
    // is the saved id, while the harness already names the custom preset.
    let claude_sid = uuid::Uuid::new_v4().to_string();
    let hash = k2_core::chat_history::claude_project_hash(
        k2_core::chat_history::resolve_root_project_path(&ws.path.to_string_lossy()),
    );
    let dir = env.home.join(".claude").join("projects").join(&hash);
    std::fs::create_dir_all(&dir).expect("claude projects dir");
    std::fs::write(dir.join(format!("{claude_sid}.jsonl")), b"{\"cwd\":\"/x\"}\n").expect("jsonl");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO workspace_sessions (id, project_id, session_id, harness, owner, status, created_at) \
             VALUES (?1, ?2, ?3, 'board-keeper', 'user', 'running', unixepoch())",
            params![uuid::Uuid::new_v4().to_string(), ws.id, claude_sid],
        )
        .expect("stale row");
    }

    let thread = get_thread("keeper-switch");
    let conv = thread["conversation_id"].as_str().expect("conversation id");
    assert_ne!(conv, claude_sid, "Thread must not show the old Claude conversation");
    assert_eq!(pinned_row(&ws.id), (Some(conv.to_string()), "board-keeper".to_string()));
    // The launch uses that same new id.
    let path = ws.path.to_string_lossy().into_owned();
    let out = tokio::task::block_in_place(|| k2_daemon::pinned_chat::ensure_pinned_chat(&path, false, false))
        .expect("ensure pinned chat");
    assert_eq!(out.claude_session_id, conv);
}
