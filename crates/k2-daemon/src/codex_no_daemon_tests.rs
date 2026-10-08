//! Every daemon launch path that starts Codex execs it with `--no-daemon`
//! when the installed Codex supports it, so its tool commands inherit THIS
//! session's K2 passport instead of the shared app-server's (another
//! session's, or a dead one). See `k2_core::terminal::codex_no_daemon`.
//!
//! The "codex" here is a shell stub: `--help` prints a help text (with or
//! without `--no-daemon`); any other call records its argv and the
//! `K2_HOOK_SOCK` it was given, then `exec cat`. Never the real Codex.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use k2_core::session::SessionId;

use crate::spawn::{spawn_agent_session_v2_blocking, SpawnWorkspaceSessionRequest};
use crate::v2_spawn::{handle_v2_refresh, handle_v2_spawn, spawn_session, SpawnRequest};

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

/// `$HOME` beside the test binary (outside the OS temp dir, so the spawn
/// guard accepts an absolute stub path). The `EnvVar` guard holds the ONE
/// shared env lock and restores `$HOME` on drop (after the dir is removed).
struct StubHome {
    home: PathBuf,
    _home_var: k2_core::test_env::EnvVar,
}

impl StubHome {
    fn new() -> Self {
        let home = crate::test_support::stub_home_dir("codex-nd");
        std::fs::create_dir_all(home.join(".k2")).expect("create stub HOME");
        let home_var = k2_core::test_env::EnvVar::set("HOME", &home);
        assert!(
            !k2_core::terminal::agent_spawn_guard::GuardEnv::from_process().home_is_temp(),
            "stub HOME {} is under the OS temp dir; build with a target dir outside $TMPDIR",
            home.display()
        );
        Self {
            home,
            _home_var: home_var,
        }
    }
}

impl Drop for StubHome {
    fn drop(&mut self) {
        // Runs before the fields drop, so the lock is still held here.
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

struct ReapAgent(String);
impl Drop for ReapAgent {
    fn drop(&mut self) {
        if let Some(session) = crate::v2_session_map::unregister(&self.0) {
            session.kill();
        }
    }
}

/// Write `<dir>/codex`. `supports` = its `--help` lists `--no-daemon`
/// (Codex 0.155+); otherwise it looks like Codex 0.154.
fn write_codex_stub(supports: bool) -> PathBuf {
    // OS temp dir, not beside the test binary: provider detection takes the
    // basename of the first whitespace token, so the stub path must have no
    // spaces (the checkout path may).
    let dir = std::env::temp_dir().join(format!(
            "k2-codex-nd-stub-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::SeqCst)
        ));
    std::fs::create_dir_all(&dir).expect("stub dir");
    let help = if supports {
        "      --no-alt-screen\\n          Disable alternate screen mode\\n      --no-daemon\\n          Run without the shared background server, even if it is already running\\n"
    } else {
        "      --no-alt-screen\\n          Disable alternate screen mode\\n"
    };
    let script = format!(
        "#!/bin/sh\n\
         if [ \"$1\" = \"--help\" ]; then printf '{help}'; exit 0; fi\n\
         d=\"$(dirname \"$0\")\"\n\
         {{ for a in \"$@\"; do printf '%s\\n' \"$a\"; done; printf 'SOCK=%s\\n' \"$K2_HOOK_SOCK\"; printf -- '---\\n'; }} >> \"$d/calls.log\"\n\
         exec cat\n"
    );
    let path = dir.join("codex");
    std::fs::write(&path, script).expect("stub script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    path
}

/// One recorded launch: argv + the `K2_HOOK_SOCK` it saw.
#[derive(Debug)]
struct Call {
    argv: Vec<String>,
    sock: String,
}

fn read_calls(stub: &Path) -> Vec<Call> {
    let log = stub.parent().unwrap().join("calls.log");
    let raw = std::fs::read_to_string(&log).unwrap_or_default();
    let mut out = Vec::new();
    let mut argv = Vec::new();
    let mut sock = String::new();
    for line in raw.lines() {
        if line == "---" {
            out.push(Call {
                argv: std::mem::take(&mut argv),
                sock: std::mem::take(&mut sock),
            });
        } else if let Some(s) = line.strip_prefix("SOCK=") {
            sock = s.to_string();
        } else {
            argv.push(line.to_string());
        }
    }
    out
}

/// Wait (≤ 10 s) for the stub's `n`th launch to be recorded.
fn wait_call(stub: &Path, n: usize) -> Call {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let mut calls = read_calls(stub);
        if calls.len() >= n {
            return calls.swap_remove(n - 1);
        }
        assert!(
            Instant::now() < deadline,
            "codex stub {} never launched (calls so far: {:?})",
            stub.display(),
            calls
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

fn seed_project(pid: &str, cwd: &str) {
    std::fs::create_dir_all(cwd).expect("workspace dir");
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT OR REPLACE INTO projects (id, path, name) VALUES (?1, ?2, ?3)",
        rusqlite::params![pid, cwd, "codex-no-daemon"],
    )
    .expect("seed project");
}

fn seed_tab_row(pid: &str, cwd: &str, agent: &str, command: &str, sid: &str, args: &[&str]) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::db::schema::WorkspaceTabSession::upsert(
        &conn,
        &k2_core::db::schema::WorkspaceTabSession {
            project_id: pid.to_string(),
            pane_group_id: agent.strip_prefix("tab-").unwrap_or(agent).to_string(),
            agent_name: agent.to_string(),
            session_id: Some(sid.to_string()),
            command: Some(command.to_string()),
            args_json: Some(serde_json::to_string(&args).expect("args json")),
            cwd: Some(cwd.to_string()),
            last_seen_at: 0,
            pinned_cols: None,
            pinned_rows: None,
            pinned_set_by: None,
        },
    )
    .expect("seed workspace_tab_sessions row");
}

fn workspace(label: &str) -> (String, String) {
    let n = NEXT_ID.fetch_add(1, Ordering::SeqCst);
    let pid = format!("codex-nd-{label}-{n}");
    // Under the test's StubHome (taken first in every test), so the dir is
    // unique per run and removed with it; it used to be a fixed name next
    // to the test binary that every run re-used and none removed.
    let home = std::env::var_os("HOME").expect("StubHome sets HOME");
    let cwd = std::path::PathBuf::from(home)
        .join(format!("ws-{pid}"))
        .to_string_lossy()
        .into_owned();
    seed_project(&pid, &cwd);
    (pid, cwd)
}

fn ws_req(
    agent: &str,
    pid: &str,
    cwd: &str,
    stub: &str,
    args: &[&str],
    canonical_key: Option<String>,
    launch_prompt: Option<&str>,
) -> SpawnWorkspaceSessionRequest {
    SpawnWorkspaceSessionRequest {
        agent_name: agent.to_string(),
        project_id: Some(pid.to_string()),
        cwd: cwd.to_string(),
        command: Some(stub.to_string()),
        args: Some(s(args)),
        cols: 120,
        rows: 38,
        canonical_key,
        env: Default::default(),
        launch_prompt: launch_prompt.map(str::to_string),
        label: None,
    }
}

/// Extra harness tab (`tab-*` via v2/spawn), fresh: root `--no-daemon`;
/// the session's durable args stay identity-only.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn extra_tab_fresh_codex_execs_with_no_daemon() {
    k2_core::db::init_for_tests();
    let _home = StubHome::new();
    let (pid, cwd) = workspace("tab");
    let agent = format!("tab-{pid}");
    let _reap = ReapAgent(agent.clone());
    let stub = write_codex_stub(true);
    let r = handle_v2_spawn(
        serde_json::json!({
            "agent_name": agent,
            "cwd": cwd,
            "command": stub.to_string_lossy(),
            "args": ["--yolo"],
        })
        .to_string()
        .as_bytes(),
    );
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let call = wait_call(&stub, 1);
    assert_eq!(call.argv, s(&["--no-daemon", "--yolo"]));
    // The tool commands of THIS session see THIS session's socket.
    let live = crate::v2_session_map::lookup_by_agent_name(&agent).expect("live tab");
    assert_eq!(
        call.sock,
        crate::cell_uds::cell_socket_path(&live.session_id).to_string_lossy(),
    );
    assert_eq!(live.args, s(&["--yolo"]), "durable args must stay clean");
    let _ = std::fs::remove_dir_all(stub.parent().unwrap());
}

/// Pinned chat (canonical `<project_id>` lane), resume: root `--no-daemon`,
/// `resume <id>` kept adjacent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pinned_chat_resume_codex_execs_with_no_daemon() {
    k2_core::db::init_for_tests();
    let _home = StubHome::new();
    let (pid, cwd) = workspace("pinned");
    let _reap = ReapAgent(pid.clone());
    let stub = write_codex_stub(true);
    let sid = "0199aaaa-1111-7000-8000-000000000001";
    let out = spawn_agent_session_v2_blocking(ws_req(
        "pinned-agent",
        &pid,
        &cwd,
        &stub.to_string_lossy(),
        &["--yolo", "resume", sid],
        None,
        None,
    ))
    .expect("pinned spawn");
    let call = wait_call(&stub, 1);
    assert_eq!(call.argv, s(&["--no-daemon", "--yolo", "resume", sid]));
    assert_eq!(
        call.sock,
        crate::cell_uds::cell_socket_path(&out.session_id).to_string_lossy(),
    );
    let live = crate::v2_session_map::lookup_by_agent_name(&pid).expect("pinned live");
    assert_eq!(live.args, s(&["--yolo", "resume", sid]));
    let _ = std::fs::remove_dir_all(stub.parent().unwrap());
}

/// Heartbeat resume fire (`<project_id>:hb:<name>` lane).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn heartbeat_resume_fire_codex_execs_with_no_daemon() {
    k2_core::db::init_for_tests();
    let _home = StubHome::new();
    let (pid, cwd) = workspace("hb");
    let key = format!("{pid}:hb:nightly");
    let _reap = ReapAgent(key.clone());
    let stub = write_codex_stub(true);
    let sid = "0199aaaa-2222-7000-8000-000000000002";
    spawn_agent_session_v2_blocking(ws_req(
        "hb-agent",
        &pid,
        &cwd,
        &stub.to_string_lossy(),
        &["--yolo", "resume", sid],
        Some(key.clone()),
        None,
    ))
    .expect("heartbeat spawn");
    let call = wait_call(&stub, 1);
    assert_eq!(call.argv, s(&["--no-daemon", "--yolo", "resume", sid]));
    let _ = std::fs::remove_dir_all(stub.parent().unwrap());
}

/// Sidecar / `k2 msg` wake with a fire-once launch prompt: the flag rides
/// the exec argv next to the prompt; durable args stay identity-only.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wake_with_launch_prompt_codex_execs_with_no_daemon() {
    k2_core::db::init_for_tests();
    let _home = StubHome::new();
    let (pid, cwd) = workspace("wake");
    let _reap = ReapAgent(pid.clone());
    let stub = write_codex_stub(true);
    let sid = "0199aaaa-3333-7000-8000-000000000003";
    let out = spawn_agent_session_v2_blocking(ws_req(
        "wake-agent",
        &pid,
        &cwd,
        &stub.to_string_lossy(),
        &["--yolo", "resume", sid],
        None,
        Some("read .k2/sidecars/x/BRIEF.md"),
    ))
    .expect("wake spawn");
    assert!(out.launch_prompt_attached, "codex takes a launch prompt");
    let call = wait_call(&stub, 1);
    assert_eq!(&call.argv[..4], &s(&["--no-daemon", "--yolo", "resume", sid])[..]);
    assert_eq!(
        call.argv.last().map(String::as_str),
        Some("read .k2/sidecars/x/BRIEF.md")
    );
    let live = crate::v2_session_map::lookup_by_agent_name(&pid).expect("wake live");
    assert!(
        !live.args.iter().any(|a| a == "--no-daemon"),
        "durable args must stay clean: {:?}",
        live.args
    );
    let _ = std::fs::remove_dir_all(stub.parent().unwrap());
}

/// Sidecar refresh (`v2/refresh` respawns the tab row's conversation).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sidecar_refresh_codex_execs_with_no_daemon() {
    k2_core::db::init_for_tests();
    let _home = StubHome::new();
    let (pid, cwd) = workspace("refresh");
    let agent = format!("tab-{pid}");
    let _reap = ReapAgent(agent.clone());
    let stub = write_codex_stub(true);
    let stub_s = stub.to_string_lossy().into_owned();
    let sid = "0199aaaa-4444-7000-8000-000000000004";
    seed_tab_row(&pid, &cwd, &agent, &stub_s, sid, &["--yolo"]);
    let r = handle_v2_refresh(
        serde_json::json!({ "agent_name": agent, "cwd": cwd })
            .to_string()
            .as_bytes(),
    );
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let call = wait_call(&stub, 1);
    assert_eq!(call.argv.first().map(String::as_str), Some("--no-daemon"));
    assert!(
        call.argv.windows(2).any(|w| w[0] == "resume" && w[1] == sid),
        "codex must resume {sid}: {:?}",
        call.argv
    );
    let _ = std::fs::remove_dir_all(stub.parent().unwrap());
}

/// `/v1` host session (`spawn_session` with fire-once `exec_args`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_session_codex_execs_with_no_daemon() {
    k2_core::db::init_for_tests();
    let _home = StubHome::new();
    let (_pid, cwd) = workspace("host");
    let agent = format!("api-test-{}", uuid::Uuid::new_v4());
    let _reap = ReapAgent(agent.clone());
    let stub = write_codex_stub(true);
    let sid = SessionId::new();
    let r = spawn_session(SpawnRequest {
        agent_name: agent.clone(),
        cwd: cwd.clone(),
        command: Some(stub.to_string_lossy().into_owned()),
        args: Some(s(&["--yolo"])),
        exec_args: Some(s(&["--yolo", "--", "hello from the API"])),
        cols: 80,
        rows: 24,
        env: Some(Default::default()),
        label: None,
        label_locked: None,
        sandbox: None,
        ephemeral_cwd: None,
        principal_key: None,
        quota_workspace: None,
        overlay: None,
        forced_session_id: Some(sid),
        attach_only: false,
    });
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let call = wait_call(&stub, 1);
    assert_eq!(
        call.argv,
        s(&["--no-daemon", "--yolo", "--", "hello from the API"])
    );
    assert_eq!(
        call.sock,
        crate::cell_uds::cell_socket_path(&sid).to_string_lossy(),
    );
    let live = crate::v2_session_map::lookup_by_agent_name(&agent).expect("host live");
    assert_eq!(live.args, s(&["--yolo"]));
    let _ = std::fs::remove_dir_all(stub.parent().unwrap());
}

/// Codex ≤ 0.154 rejects unknown flags: its argv is left exactly as built.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn old_codex_without_the_flag_execs_unchanged() {
    k2_core::db::init_for_tests();
    let _home = StubHome::new();
    let (pid, cwd) = workspace("old");
    let agent = format!("tab-{pid}");
    let _reap = ReapAgent(agent.clone());
    let stub = write_codex_stub(false);
    let r = handle_v2_spawn(
        serde_json::json!({
            "agent_name": agent,
            "cwd": cwd,
            "command": stub.to_string_lossy(),
            "args": ["--yolo", "resume", "0199aaaa-5555-7000-8000-000000000005"],
        })
        .to_string()
        .as_bytes(),
    );
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let call = wait_call(&stub, 1);
    assert_eq!(
        call.argv,
        s(&["--yolo", "resume", "0199aaaa-5555-7000-8000-000000000005"])
    );
    let _ = std::fs::remove_dir_all(stub.parent().unwrap());
}
