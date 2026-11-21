use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

// Each test gets its own agent_name so parallel test runs
// don't stomp on each other's map entries.
static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
fn uniq_agent_name() -> String {
    format!("test-v2-{}", NEXT_ID.fetch_add(1, Ordering::SeqCst))
}

/// A temp HOME (held for the test) and a clean folder under it for a
/// spawn of a gated program (`sleep`). The folder-trust gate (SS11)
/// gates every program but a plain login shell, acts whenever HOME is
/// a temp HOME, and never trusts `/tmp` (a parent of one): a `sleep`
/// in `/tmp` passed or got 409 by whether another test held a
/// TempHome at that moment (soak 2026-10-26, daemon shuffle seed 1).
/// Under its own HOME the gate always runs and grants this clean
/// folder.
fn clean_spawn_dir() -> (crate::test_support::TempHome, String) {
    let home = crate::test_support::TempHome::new();
    let ws = home.path().join(format!("ws-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&ws).expect("spawn folder");
    let ws = dunce::canonicalize(&ws).expect("canonical spawn folder").to_string_lossy().into_owned();
    (home, ws)
}

fn sleep_spawn_body(agent: &str, cwd: &str, sandbox: bool) -> Vec<u8> {
    let (cmd, args) = k2_core::test_env::sleep_command(30);
    let mut v = serde_json::json!({ "agent_name": agent, "cwd": cwd, "command": cmd, "args": args });
    if sandbox {
        v["sandbox"] = serde_json::Value::Bool(true);
    }
    v.to_string().into_bytes()
}

#[test]
fn spawn_request_rejects_empty_agent_name() {
    let body = br#"{"agent_name":""}"#;
    let result = handle_v2_spawn(body);
    assert_eq!(result.status, "400 Bad Request");
    assert!(result.body.contains("agent_name required"));
}

// ── Sandbox requests fail closed (0.46.17) ───────────────────────────

#[test]
fn resolve_sandbox_refuses_a_request_on_this_build() {
    use k2_core::terminal::SandboxSpec;
    // Not requested: the plain spawn.
    assert_eq!(resolve_sandbox(false), Some(SandboxSpec::Passthrough));
    // Requested on a build that can't sandbox: no spec at all, so the
    // caller refuses. Never the old silent Passthrough, never Microvm.
    assert!(!can_sandbox(), "test premise: this build cannot sandbox");
    assert_eq!(resolve_sandbox(true), None, "a sandbox request must not resolve to an unsandboxed spec");
}

#[test]
fn sandbox_unavailable_body_is_one_plain_line_with_a_code() {
    let v: serde_json::Value = serde_json::from_str(&sandbox_unavailable_body("--sandbox")).expect("json");
    assert_eq!(
        v["error"],
        serde_json::json!("this computer can't run a sandboxed session. Run it without --sandbox, knowing it runs as you.")
    );
    assert_eq!(v["code"], serde_json::json!("sandbox_unavailable"));
    assert!(!v["error"].as_str().expect("error string").contains('\n'));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_with_sandbox_true_refuses_409_and_spawns_nothing() {
    // `{"sandbox":true}` on a build that can't sandbox: 409 with the plain
    // line, no session registered under the name, no process started.
    // Before 0.46.17 this answered 200 with "sandbox":"passthrough" and a
    // live unsandboxed PTY. Two names: a renderer tab, and an `api-…` cell
    // tab remounting with its command after its daemon lost the sandbox.
    // (async test: a spawn would use `tokio::spawn`.)
    let _ = k2_core::db::init_for_tests();
    let (_home, ws) = clean_spawn_dir();
    let (shell, flag) = if cfg!(windows) { ("cmd", "/C") } else { ("sh", "-c") };
    for agent in [uniq_agent_name(), format!("api-owner-{}", uuid::Uuid::new_v4())] {
        let marker = std::path::Path::new(&ws).join(format!("ran-{agent}"));
        let body = serde_json::json!({
            "agent_name": agent,
            "cwd": ws,
            "command": shell,
            "args": [flag, format!("echo ran > \"{}\"", marker.display())],
            "sandbox": true,
        })
        .to_string();
        let result = handle_v2_spawn(body.as_bytes());
        let leaked = crate::v2_session_map::unregister(&agent);
        if let Some(ref s) = leaked {
            s.kill();
        }
        assert_eq!(result.status, "409 Conflict", "{agent}: body={}", result.body);
        let v: serde_json::Value = serde_json::from_str(&result.body).expect("409 body is JSON");
        assert_eq!(v["code"], serde_json::json!("sandbox_unavailable"), "{agent}: body={}", result.body);
        assert!(
            v["error"].as_str().expect("error line").starts_with("this computer can't run a sandboxed session."),
            "{agent}: body={}",
            result.body
        );
        assert!(leaked.is_none(), "{agent}: a refused sandbox request must register no session");
        // Give a wrongly started child time to write its marker.
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert!(!marker.exists(), "{agent}: a refused sandbox request must run nothing");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sandbox_true_never_attaches_to_a_live_unsandboxed_session() {
    // A live plain session under the name: a later `{"sandbox":true}` for the
    // same name is refused and the live session is left as it was (not
    // reused and labelled, not killed).
    let _ = k2_core::db::init_for_tests();
    let (_home, ws) = clean_spawn_dir();
    let agent = uniq_agent_name();
    let first = handle_v2_spawn(&sleep_spawn_body(&agent, &ws, false));
    assert_eq!(first.status, "200 OK", "body={}", first.body);
    let first_id = serde_json::from_str::<serde_json::Value>(&first.body).expect("json")["sessionId"].clone();
    let again = handle_v2_spawn(&sleep_spawn_body(&agent, &ws, true));
    let live = crate::v2_session_map::unregister(&agent);
    let live_id = live.as_ref().map(|s| s.session_id.to_string());
    if let Some(ref s) = live {
        s.kill();
    }
    assert_eq!(again.status, "409 Conflict", "body={}", again.body);
    assert!(!again.body.contains("\"reused\""), "body={}", again.body);
    assert_eq!(live_id.map(serde_json::Value::String), Some(first_id), "the live session stays as it was");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_with_sandbox_false_echoes_passthrough() {
    // `{"sandbox":false}` asks for the plain spawn and says so truthfully.
    let _ = k2_core::db::init_for_tests();
    let (_home, ws) = clean_spawn_dir();
    let agent = uniq_agent_name();
    let (cmd, args) = k2_core::test_env::sleep_command(30);
    let body = serde_json::json!({ "agent_name": agent, "cwd": ws, "command": cmd, "args": args, "sandbox": false });
    let result = handle_v2_spawn(body.to_string().as_bytes());
    let session = crate::v2_session_map::unregister(&agent);
    if let Some(ref s) = session {
        s.kill();
    }
    assert_eq!(result.status, "200 OK", "body={}", result.body);
    assert!(result.body.contains(r#""sandbox":"passthrough""#), "body={}", result.body);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_without_sandbox_omits_the_field() {
    // Default-path regression guard: when the request omits `sandbox`,
    // the response must NOT carry a `sandbox` field (byte-identical to
    // pre-seam behavior). (async test: child-exit observer uses tokio.)
    let _ = k2_core::db::init_for_tests();
    let (_home, ws) = clean_spawn_dir();
    let agent = uniq_agent_name();
    let body = sleep_spawn_body(&agent, &ws, false);
    let result = handle_v2_spawn(&body);
    let session = crate::v2_session_map::unregister(&agent);
    if let Some(ref s) = session {
        s.kill();
    }
    assert_eq!(result.status, "200 OK", "body={}", result.body);
    assert!(
        !result.body.contains("sandbox"),
        "absent sandbox request must produce no sandbox field; body={}",
        result.body
    );
}

fn passport_for(session: &k2_core::terminal::DaemonPtySession) -> String {
    crate::session_token::mint_session_token(
        &session.session_id,
        "pane",
        crate::session_token::HookPrincipal { workspace_uuid: "ws".into(), agent_address: "a".into() },
        crate::session_token::CredMode::ApiKey,
        crate::session_token::Provider::Anthropic,
    )
}

/// A passport ends with the session, not with a map key. Re-keyed: the
/// same session is still served. Closed, or displaced by a newer session
/// under its key: refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_passport_ends_with_the_session_not_with_its_map_key() {
    let _ = k2_core::db::init_for_tests();
    let (_home, ws) = clean_spawn_dir();
    let spawn = |agent: &str| {
        let result = handle_v2_spawn(&sleep_spawn_body(agent, &ws, false));
        assert_eq!(result.status, "200 OK", "spawn {agent}: {}", result.body);
        crate::v2_session_map::lookup_by_agent_name(agent).expect("registered")
    };

    // Re-keyed (the boot's legacy-key sweep): same session, still served.
    let pid = uuid::Uuid::new_v4().to_string();
    let legacy = format!("{pid}:legacy");
    let session = spawn(&legacy);
    let token = passport_for(&session);
    crate::v2_session_map::migrate_legacy_keys_to_bare_pid();
    let rekeyed = crate::v2_session_map::lookup_by_agent_name(&pid).expect("under the bare key");
    assert_eq!(rekeyed.session_id, session.session_id);
    assert!(rekeyed.is_child_alive(), "a re-key doesn't end the session");
    assert!(crate::session_token::validate_hook(&token).is_some(), "a re-keyed session keeps its passport");

    // Closed: refused, at once.
    crate::v2_session_map::unregister(&pid);
    assert!(!rekeyed.is_child_alive(), "unregister ends the session");
    assert!(crate::session_token::validate_hook(&token).is_none(), "a closed session's passport is refused");

    // Displaced: the old one is refused, the new one served.
    let (name_a, name_b) = (uniq_agent_name(), uniq_agent_name());
    let (a, b) = (spawn(&name_a), spawn(&name_b));
    let (token_a, token_b) = (passport_for(&a), passport_for(&b));
    crate::v2_session_map::register(name_a.clone(), std::sync::Arc::clone(&b));
    assert!(!a.is_child_alive(), "a displaced session is killed");
    assert!(crate::session_token::validate_hook(&token_a).is_none(), "a displaced session's passport is refused");
    assert!(crate::session_token::validate_hook(&token_b).is_some(), "the session that took the key keeps its own");
    crate::v2_session_map::unregister(&name_a);
    crate::v2_session_map::unregister(&name_b);
    b.kill();
}

#[test]
fn spawn_request_rejects_malformed_json() {
    let body = b"not json at all";
    let result = handle_v2_spawn(body);
    assert_eq!(result.status, "400 Bad Request");
    assert!(result.body.contains("parse v2 SpawnRequest"));
}

#[test]
fn close_noop_returns_closed_false() {
    let agent = uniq_agent_name();
    let body = format!(r#"{{"agent_name":"{}"}}"#, agent).into_bytes();
    let result = handle_v2_close(&body);
    assert_eq!(result.status, "200 OK");
    assert!(result.body.contains(r#""closed":false"#));
}

#[test]
fn recovered_launch_skips_api_host_sessions() {
    k2_core::db::init_for_tests();
    assert!(
        recovered_launch(
            "api-ec8ba43a-637c-45c6-806d-9325a7069bef-aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
            "/tmp/sales-interview",
        )
        .is_none(),
        "api-* must not restart-recover after reboot"
    );
}

#[test]
fn recovered_launch_skips_canonical_chat() {
    k2_core::db::init_for_tests();
    let pid = "canonical-dead-until-need";
    let cwd = "/tmp/canonical-dead-until-need";
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT OR REPLACE INTO projects (id, path, name) VALUES (?1, ?2, ?3)",
            rusqlite::params![pid, cwd, "dead"],
        )
        .expect("seed canonical project");
    }
    assert!(
        recovered_launch(pid, cwd).is_none(),
        "canonical chat must stay dead until a need"
    );
}

#[test]
fn recovered_launch_skips_tab_sessions() {
    k2_core::db::init_for_tests();
    assert!(
        recovered_launch("tab-aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee", "/tmp/any",).is_none(),
        "tab-* without a workspace_tab_sessions row must stay a shell"
    );
}

fn seed_project_and_tab_row(
    pid: &str,
    cwd: &str,
    agent_name: &str,
    command: Option<&str>,
    session_id: Option<&str>,
    args: &[&str],
) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT OR REPLACE INTO projects (id, path, name) VALUES (?1, ?2, ?3)",
        rusqlite::params![pid, cwd, "tab-resume"],
    )
    .expect("seed project");
    let pane_group_id = agent_name
        .strip_prefix("tab-")
        .unwrap_or(agent_name)
        .to_string();
    k2_core::db::schema::WorkspaceTabSession::upsert(
        &conn,
        &k2_core::db::schema::WorkspaceTabSession {
            project_id: pid.to_string(),
            pane_group_id,
            agent_name: agent_name.to_string(),
            session_id: session_id.map(str::to_string),
            command: command.map(str::to_string),
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

#[test]
fn recovered_launch_tab_harness_with_session_id_splices_resume() {
    k2_core::db::init_for_tests();
    let pid = "tab-resume-harness-ws";
    let cwd = "/tmp/tab-resume-harness-ws";
    let agent = "tab-aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let sid = "01920000-aaaa-7000-8000-000000000001";
    seed_project_and_tab_row(
        pid,
        cwd,
        agent,
        Some("claude"),
        Some(sid),
        &["--dangerously-skip-permissions"],
    );
    let (cmd, args) = recovered_launch(agent, cwd)
        .expect("tab-* harness + session id must recover, not mint a shell");
    assert_eq!(cmd, "claude", "recovered command must be the harness");
    assert!(
        args.windows(2).any(|w| w[0] == "--resume" && w[1] == sid),
        "recovered argv must splice --resume {sid}, got: {args:?}"
    );
    assert!(
        !args.iter().any(|a| a == "--session-id"),
        "resume splice must not leave a premint --session-id, got: {args:?}"
    );
}

#[test]
fn recovered_launch_tab_shell_or_missing_session_stays_none() {
    k2_core::db::init_for_tests();
    let pid = "tab-resume-shell-ws";
    let cwd = "/tmp/tab-resume-shell-ws";
    let agent = "tab-bbbbbbbb-cccc-dddd-eeee-ffffffffffff";
    seed_project_and_tab_row(pid, cwd, agent, Some("/bin/zsh"), Some("sid-1"), &[]);
    assert!(
        recovered_launch(agent, cwd).is_none(),
        "tab-* shell command must stay a shell even with a session id"
    );

    let agent_no_sid = "tab-cccccccc-dddd-eeee-ffff-000000000000";
    seed_project_and_tab_row(
        pid,
        cwd,
        agent_no_sid,
        Some("claude"),
        None,
        &["--dangerously-skip-permissions"],
    );
    assert!(
        recovered_launch(agent_no_sid, cwd).is_none(),
        "tab-* harness without a provider session id must stay a shell"
    );

    let agent_unknown = "tab-dddddddd-eeee-ffff-0000-111111111111";
    seed_project_and_tab_row(pid, cwd, agent_unknown, Some("make"), Some("sid-2"), &[]);
    assert!(
        recovered_launch(agent_unknown, cwd).is_none(),
        "tab-* unknown command must stay a shell even with a session id"
    );
}

#[test]
fn empty_command_api_spawn_is_conflict_not_bare_shell() {
    k2_core::db::init_for_tests();
    let agent = "api-principal-00000000-0000-0000-0000-000000000001";
    let body =
        format!(r#"{{"agent_name":"{agent}","cwd":"/tmp/sales-interview"}}"#).into_bytes();
    let result = handle_v2_spawn(&body);
    assert_eq!(result.status, "409 Conflict", "body={}", result.body);
    assert!(
        result.body.contains("api_session_not_live"),
        "body={}",
        result.body
    );
    assert!(
        crate::v2_session_map::lookup_by_agent_name(agent).is_none(),
        "must not register a bare api-* shell"
    );
}

/// The one env lock, for a spawn test with no `TempHome` / `StubHome`
/// (those hold it already), under a stub `$HOME` outside the temp roots
/// (so the folder-trust gate stays off for these `/tmp` folders, as it
/// did on the process HOME, and the passport each spawn mints never lands
/// in the real `~/.k2`). Held for the whole test. Another test's
/// `AgentShim` or `EnvVar` sets `K2_TEST_AGENT_SHIM_DIR` for a while,
/// and in that window the spawn guard looks a program up in the shim
/// dirs only: an unlocked `sleep` spawn then failed with 500 "agent shim
/// 'sleep' not found in K2_TEST_AGENT_SHIM_DIR=<another test's home>"
/// (nightly soak, shuffled runs). Under the lock this test sees the
/// process's own environment from start to end.
fn spawn_env_lock() -> k2_core::test_env::StubHome {
    k2_core::test_env::StubHome::new("v2spawn-env")
}

fn seed_project_row(id: &str, path: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT OR REPLACE INTO projects (id, path, name) VALUES (?1, ?2, ?3)",
        rusqlite::params![id, path, id],
    )
    .expect("seed project");
}

fn spawn_sleep(agent: &str, cwd: &str) -> HandlerResult {
    let (cmd, args) = k2_core::test_env::sleep_command(30);
    let body = serde_json::json!({
        "agent_name": agent,
        "cwd": cwd,
        "command": cmd,
        "args": args,
    })
    .to_string()
    .into_bytes();
    handle_v2_spawn(&body)
}

struct SpawnCleanup {
    agent: String,
    root: std::path::PathBuf,
    ids: Vec<String>,
}

impl Drop for SpawnCleanup {
    fn drop(&mut self) {
        if let Some(session) = crate::v2_session_map::unregister(&self.agent) {
            session.kill();
        }
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            for id in &self.ids {
                let _ =
                    conn.execute("DELETE FROM projects WHERE id = ?1", rusqlite::params![id]);
            }
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn session_id_of(body: &str) -> String {
    let json: serde_json::Value = serde_json::from_str(body).expect("spawn json");
    json["sessionId"]
        .as_str()
        .expect("sessionId string")
        .to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nested_workspace_spawn_refuses_other_project() {
    // Declared first so it drops last (after the session is killed).
    let _env = spawn_env_lock();
    k2_core::db::init_for_tests();
    let n = NEXT_ID.fetch_add(1, Ordering::SeqCst);
    let agent = format!("tab-nested-own-{n}");
    let root = k2_core::test_env::stable_temp_dir().join(format!("k2-nested-own-{n}"));
    let parent = root.join("parent");
    let nested = parent.join("nested");
    std::fs::create_dir_all(&nested).expect("create nested cwd");
    let parent_s = parent.to_string_lossy().into_owned();
    let nested_s = nested.to_string_lossy().into_owned();
    let parent_id = format!("proj-parent-{n}");
    let nested_id = format!("proj-nested-{n}");
    seed_project_row(&parent_id, &parent_s);
    seed_project_row(&nested_id, &nested_s);
    let _cleanup = SpawnCleanup {
        agent: agent.clone(),
        root,
        ids: vec![parent_id, nested_id],
    };

    let first = spawn_sleep(&agent, &nested_s);
    assert_eq!(first.status, "200 OK", "body={}", first.body);
    let first_id = session_id_of(&first.body);

    let second = spawn_sleep(&agent, &parent_s);
    assert_eq!(second.status, "409 Conflict", "body={}", second.body);
    assert_eq!(second.body, r#"{"error":"session_owned_elsewhere"}"#);

    let live =
        crate::v2_session_map::lookup_by_agent_name(&agent).expect("session stays registered");
    assert!(live.is_child_alive(), "child must stay alive");
    assert_eq!(live.session_id.to_string(), first_id);
}

/// Home M4: a view-only room's attach never spawns, evicts or resizes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attach_only_never_spawns_and_returns_the_live_session_untouched() {
    // Declared first so it drops last (after the session is killed).
    let _env = spawn_env_lock();
    k2_core::db::init_for_tests();
    let n = NEXT_ID.fetch_add(1, Ordering::SeqCst);
    let agent = format!("tab-attach-only-{n}");
    let root = k2_core::test_env::stable_temp_dir().join(format!("k2-attach-only-{n}"));
    std::fs::create_dir_all(&root).expect("create cwd");
    let cwd = root.to_string_lossy().into_owned();
    let pid = format!("proj-attach-only-{n}");
    seed_project_row(&pid, &cwd);
    let _cleanup = SpawnCleanup {
        agent: agent.clone(),
        root,
        ids: vec![pid],
    };

    // No session yet: 404, and nothing was spawned.
    let (cmd, args) = k2_core::test_env::sleep_command(30);
    let missing = handle_v2_spawn(
        &serde_json::json!({ "agent_name": agent, "cwd": cwd, "attach_only": true, "command": cmd, "args": args })
            .to_string()
            .into_bytes(),
    );
    assert_eq!(missing.status, "404 Not Found", "body={}", missing.body);
    let missing_json: serde_json::Value = serde_json::from_str(&missing.body).expect("404 json");
    assert_eq!(missing_json["error"], "session_not_live");
    assert!(crate::v2_session_map::lookup_by_agent_name(&agent).is_none());

    let first = spawn_sleep(&agent, &cwd);
    assert_eq!(first.status, "200 OK", "body={}", first.body);
    let first_id = session_id_of(&first.body);
    let live = crate::v2_session_map::lookup_by_agent_name(&agent).expect("registered");
    let before = current_dims(&live);

    // A different command and another size: attach-only still neither
    // evicts the live program nor resizes it.
    let attach = handle_v2_spawn(
        &serde_json::json!({
            "agent_name": agent,
            "cwd": cwd,
            "attach_only": true,
            "command": "bash",
            "cols": before.0 + 17,
            "rows": before.1 + 9,
        })
        .to_string()
        .into_bytes(),
    );
    assert_eq!(attach.status, "200 OK", "body={}", attach.body);
    let json: serde_json::Value = serde_json::from_str(&attach.body).expect("attach json");
    assert_eq!(json["reused"], true);
    assert_eq!(json["attachOnly"], true);
    assert_eq!(json["sessionId"].as_str().expect("sessionId"), first_id);
    assert_eq!(json["cols"].as_u64().expect("cols"), u64::from(before.0));
    assert_eq!(json["rows"].as_u64().expect("rows"), u64::from(before.1));
    let after = crate::v2_session_map::lookup_by_agent_name(&agent).expect("still registered");
    assert!(after.is_child_alive(), "the live child must not be evicted");
    assert_eq!(after.session_id.to_string(), first_id);
    assert_eq!(current_dims(&after), before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nested_workspace_spawn_reuses_worktree_on_parent_project() {
    // Declared first so it drops last (after the session is killed).
    let _env = spawn_env_lock();
    k2_core::db::init_for_tests();
    let n = NEXT_ID.fetch_add(1, Ordering::SeqCst);
    let agent = format!("tab-nested-wt-{n}");
    let root = k2_core::test_env::stable_temp_dir().join(format!("k2-nested-wt-{n}"));
    let parent = root.join("parent");
    let worktree = parent.join(".worktrees").join("branch");
    std::fs::create_dir_all(&worktree).expect("create worktree cwd");
    let parent_s = parent.to_string_lossy().into_owned();
    let worktree_s = worktree.to_string_lossy().into_owned();
    let parent_id = format!("proj-wt-parent-{n}");
    seed_project_row(&parent_id, &parent_s);
    let _cleanup = SpawnCleanup {
        agent: agent.clone(),
        root,
        ids: vec![parent_id],
    };

    let first = spawn_sleep(&agent, &worktree_s);
    assert_eq!(first.status, "200 OK", "body={}", first.body);
    let first_id = session_id_of(&first.body);

    let second = spawn_sleep(&agent, &parent_s);
    assert_eq!(second.status, "200 OK", "body={}", second.body);
    let second_json: serde_json::Value =
        serde_json::from_str(&second.body).expect("reuse json");
    assert_eq!(second_json["reused"], true);
    assert_eq!(
        second_json["sessionId"].as_str().expect("sessionId"),
        first_id
    );

    // Empty command matches any live program when the owner is the same.
    let empty = handle_v2_spawn(
        &serde_json::json!({
            "agent_name": agent,
            "cwd": parent_s,
        })
        .to_string()
        .into_bytes(),
    );
    assert_eq!(empty.status, "200 OK", "body={}", empty.body);
    let empty_json: serde_json::Value =
        serde_json::from_str(&empty.body).expect("empty-command json");
    assert_eq!(empty_json["reused"], true);
    assert_eq!(
        empty_json["sessionId"].as_str().expect("sessionId"),
        first_id
    );
    assert!(crate::v2_session_map::lookup_by_agent_name(&agent)
        .expect("session stays registered")
        .is_child_alive());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nested_workspace_spawn_reuses_when_one_side_unregistered() {
    // Declared first so it drops last (after the session is killed).
    let _env = spawn_env_lock();
    k2_core::db::init_for_tests();
    let n = NEXT_ID.fetch_add(1, Ordering::SeqCst);
    let agent = format!("tab-nested-orphan-{n}");
    let root = k2_core::test_env::stable_temp_dir().join(format!("k2-nested-orphan-{n}"));
    let parent = root.join("parent");
    let orphan = root.join("orphan");
    std::fs::create_dir_all(&parent).expect("create parent cwd");
    std::fs::create_dir_all(&orphan).expect("create unregistered cwd");
    let parent_s = parent.to_string_lossy().into_owned();
    let orphan_s = orphan.to_string_lossy().into_owned();
    let parent_id = format!("proj-orphan-parent-{n}");
    seed_project_row(&parent_id, &parent_s);
    let _cleanup = SpawnCleanup {
        agent: agent.clone(),
        root,
        ids: vec![parent_id],
    };

    let first = spawn_sleep(&agent, &orphan_s);
    assert_eq!(first.status, "200 OK", "body={}", first.body);
    let first_id = session_id_of(&first.body);

    let second = spawn_sleep(&agent, &parent_s);
    assert_eq!(second.status, "200 OK", "body={}", second.body);
    let second_json: serde_json::Value =
        serde_json::from_str(&second.body).expect("reuse json");
    assert_eq!(second_json["reused"], true);
    assert_eq!(
        second_json["sessionId"].as_str().expect("sessionId"),
        first_id
    );
}

// GH#22 close-guard decision table (pure logic; no PTY needed).
// The spawn-backed end-to-end variant (real session + real grid-WS
// subscriber drives `subscriber_count`) lives in
// crates/k2-daemon/tests/it_sessions/reaper_close_guard_integration.rs.
#[test]
fn close_allowed_proceeds_when_no_subscribers() {
    // Nobody attached → safe to reap.
    assert!(close_allowed(0, false));
    assert!(close_allowed(0, true));
}

#[test]
fn close_allowed_refuses_attached_without_force() {
    // A client is watching and no force flag → REFUSE.
    assert!(!close_allowed(1, false));
    assert!(!close_allowed(5, false));
}

#[test]
fn close_allowed_force_bypasses_attached_guard() {
    // Deliberate teardown escape hatch overrides the guard.
    assert!(close_allowed(1, true));
    assert!(close_allowed(42, true));
}

// Full spawn-then-lookup + spawn-then-reuse tests live in
// crates/k2so-daemon/tests/ where a running tokio runtime and
// the ability to fork a shell are available. They gate A7's
// parity work.

// ── autoinject_premint_session_id (0.38.8 / Slice 3b) ────────────

fn args_of(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// CLAUDE PIN — the pre-3b behavior byte-for-byte: bare claude gets
/// `--session-id <uuid>` appended; an existing `--session-id` /
/// `--resume` / `-r` suppresses the inject.
#[test]
fn autoinject_claude_pin() {
    let mut args = args_of(&["--dangerously-skip-permissions"]);
    autoinject_premint_session_id(Some("claude"), &mut args, "tab-x", "/w");
    assert_eq!(args.len(), 3, "flag + uuid appended: {args:?}");
    assert_eq!(args[0], "--dangerously-skip-permissions");
    assert_eq!(args[1], "--session-id");
    assert_eq!(args[2].len(), 36, "a v4 uuid value: {args:?}");

    for suppressor in [
        args_of(&["--session-id", "X"]),
        args_of(&["--resume", "X"]),
        args_of(&["-r", "X"]),
    ] {
        let mut args = suppressor.clone();
        autoinject_premint_session_id(Some("claude"), &mut args, "tab-x", "/w");
        assert_eq!(args, suppressor, "existing identity must suppress inject");
    }
}

/// Slice 3b widening: grok (the other premint provider) gets the
/// same continuity mint; its own identity spellings suppress it.
#[test]
fn autoinject_grok_premints_and_respects_grok_identity_flags() {
    let mut args = args_of(&["--always-approve"]);
    autoinject_premint_session_id(Some("grok"), &mut args, "tab-x", "/w");
    assert_eq!(
        args[1], "--session-id",
        "grok premints via --session-id: {args:?}"
    );
    assert_eq!(args.len(), 3);

    for suppressor in [
        args_of(&["--resume", "X"]),
        args_of(&["-s", "X"]),
        args_of(&["-c"]),
        args_of(&["--continue"]),
        args_of(&["--fork-session"]),
    ] {
        let mut args = suppressor.clone();
        autoinject_premint_session_id(Some("grok"), &mut args, "tab-x", "/w");
        assert_eq!(args, suppressor, "grok identity flag must suppress inject");
    }
}

/// Self-minting providers + unknown commands + bare shells are
/// NEVER given invented flags.
#[test]
fn autoinject_leaves_self_minting_and_unknown_commands_untouched() {
    for cmd in [
        Some("pi"),
        Some("codex"),
        Some("gemini"),
        Some("cursor-agent"),
        Some("hermes"),
        Some("bash"),
        None,
    ] {
        let mut args = args_of(&["--whatever"]);
        autoinject_premint_session_id(cmd, &mut args, "tab-x", "/w");
        assert_eq!(
            args,
            args_of(&["--whatever"]),
            "cmd={cmd:?} must be untouched"
        );
    }
}

/// Basename gating: a path-qualified claude premints too (the
/// pre-3b literal `== "claude"` compare missed it — deliberate fix).
#[test]
fn autoinject_matches_path_qualified_claude() {
    let mut args = Vec::new();
    autoinject_premint_session_id(Some("/usr/local/bin/claude"), &mut args, "tab-x", "/w");
    assert_eq!(args.first().map(String::as_str), Some("--session-id"));
}

#[test]
fn conversation_id_from_args_is_premint_not_pty() {
    let premint = "01920000-aaaa-7000-8000-000000000001";
    let args = args_of(&["--dangerously-skip-permissions", "--session-id", premint]);
    assert_eq!(
        conversation_id_from_args(Some("claude"), &args).as_deref(),
        Some(premint)
    );
    assert_eq!(
        conversation_id_from_args(Some("claude"), &args_of(&["--resume", premint])).as_deref(),
        Some(premint)
    );
    assert_eq!(
        conversation_id_from_args(Some("pi"), &args_of(&["--whatever"])),
        None,
        "self-minting providers with no identity argv must not invent an id"
    );
}

fn assert_resumes_same_id(args: &[String], sid: &str) {
    assert!(
        args.windows(2).any(|w| w[1] == sid),
        "argv must resume {sid}, got {args:?}"
    );
    assert!(
        !args.iter().any(|a| a == "--session-id"),
        "refresh must not re-exec --session-id, got {args:?}"
    );
    for arg in args {
        if arg != sid && is_uuid_shape(arg) {
            panic!("refresh minted a new session id {arg} in {args:?}");
        }
    }
}

#[test]
fn recovered_launch_strips_stored_premint_and_resumes_row_id() {
    k2_core::db::init_for_tests();
    let pid = format!("refresh-premint-{}", NEXT_ID.fetch_add(1, Ordering::SeqCst));
    let cwd = format!("/tmp/{pid}");
    let agent = format!("tab-{pid}");
    let sid = "01920000-aaaa-7000-8000-000000000001";
    let premint = "01920000-bbbb-7000-8000-0000000000aa";
    seed_project_and_tab_row(
        &pid,
        &cwd,
        &agent,
        Some("claude"),
        Some(sid),
        &["--dangerously-skip-permissions", "--session-id", premint],
    );
    let (cmd, args) = recovered_launch(&agent, &cwd).expect("row resumes");
    assert_eq!(cmd, "claude");
    assert_resumes_same_id(&args, sid);
    assert!(
        !args.iter().any(|a| a == premint),
        "stored premint must not be re-exec'd, got {args:?}"
    );
}

#[test]
fn recovered_launch_self_minting_harness_resumes_same_id_without_premint() {
    k2_core::db::init_for_tests();
    let cases: &[(&str, &[&str], &str)] = &[
        ("codex", &["--yolo"], "resume"),
        ("pi", &[], "--session"),
        ("gemini", &[], "--resume"),
        ("cursor-agent", &[], "--resume"),
    ];
    for (command, base, flag) in cases {
        let n = NEXT_ID.fetch_add(1, Ordering::SeqCst);
        let pid = format!("refresh-self-{command}-{n}");
        let cwd = format!("/tmp/{pid}");
        let agent = format!("tab-{pid}");
        let sid = format!("01920000-cccc-7000-8000-{n:012}");
        seed_project_and_tab_row(&pid, &cwd, &agent, Some(*command), Some(sid.as_str()), base);
        let (cmd, args) = recovered_launch(&agent, &cwd)
            .unwrap_or_else(|| panic!("{command} live argv with no identity must resume"));
        assert_eq!(cmd, *command);
        assert_resumes_same_id(&args, &sid);
        assert!(
            args.windows(2).any(|w| w[0] == *flag && w[1] == sid),
            "{command} must splice {flag} {sid}, got {args:?}"
        );
    }
}

/// Private `$HOME` for tests that exec a non-system stub.
///
/// The temp-HOME belt (`k2_core::terminal::agent_spawn_guard`,
/// 20dc5d7f) refuses every program outside `/bin` `/usr/bin` … while the
/// PROCESS `$HOME` sits under the OS temp dir — which it does whenever
/// another lib test holds `test_support::TempHome`. These tests exec a
/// `#!/bin/sh exec cat` stub, so they take the ONE shared env lock
/// (through `EnvVar`) and point `$HOME` at a fresh dir next to the test
/// binary (outside the temp roots). They neither race the HOME swappers
/// nor read the real home dir. Removes the dir on drop, then the
/// `EnvVar` guard restores `$HOME` (and releases the lock).
struct StubHome {
    home: std::path::PathBuf,
    _home_var: k2_core::test_env::EnvVar,
}

impl StubHome {
    fn new() -> Self {
        let home = crate::test_support::stub_home_dir("v2spawn");
        std::fs::create_dir_all(k2_core::paths::k2_home_in(&home)).expect("create stub HOME");
        let home_var = k2_core::test_env::EnvVar::set("HOME", &home);
        assert!(
            !k2_core::terminal::agent_spawn_guard::GuardEnv::from_process().home_is_temp(),
            "stub HOME {} is under the OS temp dir; the spawn guard would refuse the stub \
                 (build with a target dir outside $TMPDIR)",
            home.display()
        );
        Self { home, _home_var: home_var }
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

fn write_agent_stub(basename: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "k2-sidecar-refresh-{}-{}-{}",
        basename,
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).expect("stub dir");
    let path = dir.join(basename);
    // Not fs::write + exec: ETXTBSY under parallel forks.
    k2_core::test_env::write_executable(&path, "#!/bin/sh\nexec cat\n");
    path
}

fn json_body(raw: &str) -> serde_json::Value {
    serde_json::from_str(raw).unwrap_or_else(|e| panic!("json {e}: {raw}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refresh_respawns_same_provider_session_and_later_spawn_attaches() {
    k2_core::db::init_for_tests();
    // Declared first so it drops last: the stub is killed (ReapAgent)
    // before $HOME is restored.
    let _home = StubHome::new();
    let n = NEXT_ID.fetch_add(1, Ordering::SeqCst);
    let pid = format!("refresh-live-{n}");
    let cwd = format!("/tmp/{pid}");
    let agent = format!("tab-{pid}");
    let _reap = ReapAgent(agent.clone());
    let stub = write_agent_stub("claude");
    let stub_s = stub.to_string_lossy().into_owned();
    let sid = "01920000-aaaa-7000-8000-000000000099";
    let premint = "01920000-bbbb-7000-8000-0000000000bb";

    // Live cell first. Its register() would overwrite a harness row
    // with command=sleep, so the resumable row is written after.
    let (cmd, args) = k2_core::test_env::sleep_command(30);
    let prior = handle_v2_spawn(
        serde_json::json!({ "agent_name": agent, "cwd": cwd, "command": cmd, "args": args }).to_string().as_bytes(),
    );
    assert_eq!(prior.status, "200 OK", "{}", prior.body);
    let prior_pty = json_body(&prior.body)["sessionId"]
        .as_str()
        .unwrap()
        .to_string();
    seed_project_and_tab_row(
        &pid,
        &cwd,
        &agent,
        Some(stub_s.as_str()),
        Some(sid),
        &["--dangerously-skip-permissions", "--session-id", premint],
    );

    let refreshed = handle_v2_refresh(
        serde_json::json!({ "agent_name": agent, "cwd": cwd })
            .to_string()
            .as_bytes(),
    );
    assert_eq!(refreshed.status, "200 OK", "{}", refreshed.body);
    let refreshed_json = json_body(&refreshed.body);
    assert_eq!(refreshed_json["conversationId"].as_str(), Some(sid));
    assert_eq!(refreshed_json["reused"].as_bool(), Some(false));
    let refreshed_pty = refreshed_json["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_string();
    assert_ne!(
        refreshed_pty, prior_pty,
        "refresh must replace the killed pty"
    );

    let live = crate::v2_session_map::lookup_by_agent_name(&agent).expect("refreshed pty");
    assert_eq!(live.session_id.to_string(), refreshed_pty);
    assert_resumes_same_id(&live.args, sid);
    assert!(live.is_child_alive());

    // Stale ChildExit from the killed sleep must not drop the new pty.
    let started = std::time::Instant::now();
    while started.elapsed() < std::time::Duration::from_millis(800) {
        let still = crate::v2_session_map::lookup_by_agent_name(&agent)
            .expect("stale ChildExit unregistered the refreshed pty");
        assert_eq!(still.session_id.to_string(), refreshed_pty);
        assert!(still.is_child_alive());
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    }

    let row_sid = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::WorkspaceTabSession::get_by_agent_name(&conn, &pid, &agent)
            .expect("row")
            .expect("tab row kept")
            .session_id
    };
    assert_eq!(
        row_sid.as_deref(),
        Some(sid),
        "refresh must not clear_index"
    );

    // Later client spawn posts the old launch argv (no session
    // identity). Attach only — do not replace the live resume.
    let attach = handle_v2_spawn(
        serde_json::json!({
            "agent_name": agent,
            "cwd": cwd,
            "command": stub_s,
            "args": ["--dangerously-skip-permissions"],
        })
        .to_string()
        .as_bytes(),
    );
    assert_eq!(attach.status, "200 OK", "{}", attach.body);
    let attach_json = json_body(&attach.body);
    assert_eq!(
        attach_json["reused"].as_bool(),
        Some(true),
        "old launch argv must attach"
    );
    assert_eq!(
        attach_json["sessionId"].as_str(),
        Some(refreshed_pty.as_str())
    );
    assert_eq!(attach_json["conversationId"].as_str(), Some(sid));
    let still = crate::v2_session_map::lookup_by_agent_name(&agent).expect("still live");
    assert_eq!(still.session_id.to_string(), refreshed_pty);
    assert_resumes_same_id(&still.args, sid);
    let _ = std::fs::remove_dir_all(stub.parent().unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refresh_self_minting_codex_does_not_mint_and_old_argv_attaches() {
    k2_core::db::init_for_tests();
    // Declared first so it drops last: the stub is killed (ReapAgent)
    // before $HOME is restored.
    let _home = StubHome::new();
    let n = NEXT_ID.fetch_add(1, Ordering::SeqCst);
    let pid = format!("refresh-codex-{n}");
    let cwd = format!("/tmp/{pid}");
    let agent = format!("tab-{pid}");
    let _reap = ReapAgent(agent.clone());
    let stub = write_agent_stub("codex");
    let stub_s = stub.to_string_lossy().into_owned();
    let sid = format!("01920000-dddd-7000-8000-{n:012}");
    seed_project_and_tab_row(
        &pid,
        &cwd,
        &agent,
        Some(stub_s.as_str()),
        Some(sid.as_str()),
        &["--yolo"],
    );
    let refreshed = handle_v2_refresh(
        serde_json::json!({ "agent_name": agent, "cwd": cwd })
            .to_string()
            .as_bytes(),
    );
    assert_eq!(refreshed.status, "200 OK", "{}", refreshed.body);
    let refreshed_json = json_body(&refreshed.body);
    assert_eq!(
        refreshed_json["conversationId"].as_str(),
        Some(sid.as_str())
    );
    let pty = refreshed_json["sessionId"].as_str().unwrap().to_string();
    let live = crate::v2_session_map::lookup_by_agent_name(&agent).expect("codex pty");
    assert_resumes_same_id(&live.args, &sid);
    assert!(
        live.args
            .windows(2)
            .any(|w| w[0] == "resume" && w[1] == sid),
        "codex must resume, got {:?}",
        live.args
    );

    let attach = handle_v2_spawn(
        serde_json::json!({
            "agent_name": agent,
            "cwd": cwd,
            "command": stub_s,
            "args": ["--yolo"],
        })
        .to_string()
        .as_bytes(),
    );
    assert_eq!(attach.status, "200 OK", "{}", attach.body);
    let attach_json = json_body(&attach.body);
    assert_eq!(attach_json["reused"].as_bool(), Some(true));
    assert_eq!(attach_json["sessionId"].as_str(), Some(pty.as_str()));
    assert_eq!(attach_json["conversationId"].as_str(), Some(sid.as_str()));
    let still = crate::v2_session_map::lookup_by_agent_name(&agent).expect("attached");
    assert_resumes_same_id(&still.args, &sid);
    let _ = std::fs::remove_dir_all(stub.parent().unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refresh_without_a_resumable_row_does_not_kill_or_mint() {
    k2_core::db::init_for_tests();
    let (_home, ws) = clean_spawn_dir();
    let agent = format!("tab-no-row-{}", NEXT_ID.fetch_add(1, Ordering::SeqCst));
    let _reap = ReapAgent(agent.clone());
    let spawned = handle_v2_spawn(&sleep_spawn_body(&agent, &ws, false));
    assert_eq!(spawned.status, "200 OK", "{}", spawned.body);
    let pty = json_body(&spawned.body)["sessionId"]
        .as_str()
        .unwrap()
        .to_string();
    let failed = handle_v2_refresh(
        serde_json::json!({ "agent_name": agent, "cwd": ws })
            .to_string()
            .as_bytes(),
    );
    assert_eq!(failed.status, "409 Conflict", "{}", failed.body);
    assert!(
        failed.body.contains("no resumable session"),
        "{}",
        failed.body
    );
    let still = crate::v2_session_map::lookup_by_agent_name(&agent).expect("not killed");
    assert_eq!(still.session_id.to_string(), pty);
    assert!(still.is_child_alive());
    assert!(
        !still.args.iter().any(|a| a == "--session-id"),
        "failure must not mint, got {:?}",
        still.args
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refresh_spawn_failure_surfaces_an_error() {
    // Declared first so it drops last (after the session is killed).
    let _env = spawn_env_lock();
    k2_core::db::init_for_tests();
    let n = NEXT_ID.fetch_add(1, Ordering::SeqCst);
    let pid = format!("refresh-missing-{n}");
    let cwd = format!("/tmp/{pid}");
    let agent = format!("tab-{pid}");
    let _reap = ReapAgent(agent.clone());
    let missing = format!("/tmp/{pid}/missing/claude");
    let sid = "01920000-eeee-7000-8000-0000000000ee";
    seed_project_and_tab_row(&pid, &cwd, &agent, Some(missing.as_str()), Some(sid), &[]);
    let failed = handle_v2_refresh(
        serde_json::json!({ "agent_name": agent, "cwd": cwd })
            .to_string()
            .as_bytes(),
    );
    assert!(
        failed.status.starts_with('5'),
        "spawn failure must surface, got {} {}",
        failed.status,
        failed.body
    );
    assert!(failed.body.contains("error"), "{}", failed.body);
    assert!(
        crate::v2_session_map::lookup_by_agent_name(&agent).is_none(),
        "failed refresh must not leave a stand-in pty"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn child_exit_for_a_different_session_does_not_unregister_current() {
    k2_core::db::init_for_tests();
    let (_home, ws) = clean_spawn_dir();
    let agent = uniq_agent_name();
    let _reap = ReapAgent(agent.clone());
    let spawned = handle_v2_spawn(&sleep_spawn_body(&agent, &ws, false));
    assert_eq!(spawned.status, "200 OK", "{}", spawned.body);
    let current = crate::v2_session_map::lookup_by_agent_name(&agent).expect("live");
    let current_id = current.session_id;
    let stale = SessionId::new();
    assert_ne!(stale, current_id);
    assert!(!unregister_if_same_session(&agent, stale));
    let still = crate::v2_session_map::lookup_by_agent_name(&agent).expect("kept");
    assert_eq!(still.session_id, current_id);
    assert!(still.is_child_alive());
    assert!(unregister_if_same_session(&agent, current_id));
    assert!(crate::v2_session_map::lookup_by_agent_name(&agent).is_none());
}

// ── One conversation, one writer (duplicate canonical tab) ───────────

fn assert_duplicate_refused(result: &HandlerResult, agent: &str, conversation: &str, message: &str) {
    assert_eq!(result.status, "409 Conflict", "{}", result.body);
    let body = json_body(&result.body);
    assert_eq!(body["error"].as_str(), Some("conversation_is_canonical"), "{}", result.body);
    assert_eq!(body["message"].as_str(), Some(message), "{}", result.body);
    assert_eq!(body["conversationId"].as_str(), Some(conversation), "{}", result.body);
    assert!(
        crate::v2_session_map::lookup_by_agent_name(agent).is_none(),
        "a refused duplicate must register no {agent} entry"
    );
}

/// Chat history opened the agent's own conversation as an extra tab:
/// a `tab-*` resuming `workspace_sessions.session_id` ran a second
/// claude on the canonical's transcript. Both the saved bad tab row
/// (empty-command recovery) and an explicit `--resume X` are refused;
/// the canonical spawn and a tab on another conversation still work,
/// and a tab duplicating another live tab's conversation is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_refuses_a_tab_that_resumes_the_canonical_conversation() {
    k2_core::db::init_for_tests();
    // Declared first so it drops last: the stubs are killed (ReapAgent)
    // before $HOME is restored.
    let _home = StubHome::new();
    let n = NEXT_ID.fetch_add(1, Ordering::SeqCst);
    let pid = format!("dup-canonical-{n}");
    let cwd = format!("/tmp/{pid}");
    let tab = format!("tab-{pid}");
    let other_tab = format!("tab-{pid}-other");
    let third_tab = format!("tab-{pid}-third");
    let _reap_tab = ReapAgent(tab.clone());
    let _reap_canonical = ReapAgent(pid.clone());
    let _reap_other = ReapAgent(other_tab.clone());
    let _reap_third = ReapAgent(third_tab.clone());
    let stub = write_agent_stub("claude");
    let stub_s = stub.to_string_lossy().into_owned();
    let canonical_sid = uuid::Uuid::new_v4().to_string();
    let other_sid = uuid::Uuid::new_v4().to_string();

    // The saved bad tab: claude, session = the canonical's conversation.
    seed_project_and_tab_row(
        &pid,
        &cwd,
        &tab,
        Some(stub_s.as_str()),
        Some(canonical_sid.as_str()),
        &["--dangerously-skip-permissions"],
    );
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO workspace_sessions (id, project_id, session_id, harness, owner, status, created_at) \
                 VALUES (?1, ?2, ?3, 'claude', 'user', 'sleeping', unixepoch())",
            rusqlite::params![uuid::Uuid::new_v4().to_string(), pid, canonical_sid],
        )
        .expect("insert workspace_sessions");
    }
    let recovered = recovered_launch(&tab, &cwd).expect("the bad tab row recovers a resume");
    assert!(
        recovered.1.windows(2).any(|w| w[0] == "--resume" && w[1] == canonical_sid),
        "precondition: the saved row resumes the canonical id, got {:?}",
        recovered.1
    );

    // Empty-command spawn (the saved tab coming back on visit).
    let empty = handle_v2_spawn(
        serde_json::json!({ "agent_name": tab, "cwd": cwd }).to_string().as_bytes(),
    );
    assert_duplicate_refused(&empty, &tab, &canonical_sid, CONVERSATION_IS_CANONICAL_MESSAGE);

    // Explicit `claude --resume X` (Chat history's resume tab).
    let explicit_body = serde_json::json!({
        "agent_name": tab,
        "cwd": cwd,
        "command": stub_s,
        "args": ["--dangerously-skip-permissions", "--resume", canonical_sid],
    })
    .to_string();
    let explicit = handle_v2_spawn(explicit_body.as_bytes());
    assert_duplicate_refused(&explicit, &tab, &canonical_sid, CONVERSATION_IS_CANONICAL_MESSAGE);

    // Control: the canonical itself still resumes its conversation.
    let canonical = handle_v2_spawn(
        serde_json::json!({
            "agent_name": pid,
            "cwd": cwd,
            "command": stub_s,
            "args": ["--dangerously-skip-permissions", "--resume", canonical_sid],
        })
        .to_string()
        .as_bytes(),
    );
    assert_eq!(canonical.status, "200 OK", "{}", canonical.body);
    assert_eq!(
        json_body(&canonical.body)["conversationId"].as_str(),
        Some(canonical_sid.as_str())
    );
    let live_canonical =
        crate::v2_session_map::lookup_by_agent_name(&pid).expect("canonical registered");
    assert!(live_canonical.is_child_alive());

    // Still refused while the canonical is live.
    let again = handle_v2_spawn(explicit_body.as_bytes());
    assert_duplicate_refused(&again, &tab, &canonical_sid, CONVERSATION_IS_CANONICAL_MESSAGE);

    // Control: a tab resuming a different conversation still works.
    let other = handle_v2_spawn(
        serde_json::json!({
            "agent_name": other_tab,
            "cwd": cwd,
            "command": stub_s,
            "args": ["--dangerously-skip-permissions", "--resume", other_sid],
        })
        .to_string()
        .as_bytes(),
    );
    assert_eq!(other.status, "200 OK", "{}", other.body);
    assert_eq!(
        json_body(&other.body)["conversationId"].as_str(),
        Some(other_sid.as_str())
    );
    let live_other =
        crate::v2_session_map::lookup_by_agent_name(&other_tab).expect("other tab registered");
    assert!(live_other.is_child_alive());

    // A second tab on a conversation another live tab resumes.
    let third = handle_v2_spawn(
        serde_json::json!({
            "agent_name": third_tab,
            "cwd": cwd,
            "command": stub_s,
            "args": ["--dangerously-skip-permissions", "--resume", other_sid],
        })
        .to_string()
        .as_bytes(),
    );
    assert_duplicate_refused(&third, &third_tab, &other_sid, CONVERSATION_ALREADY_OPEN_MESSAGE);
    assert!(
        crate::v2_session_map::lookup_by_agent_name(&other_tab)
            .expect("the first tab stays")
            .is_child_alive(),
        "refusing the duplicate must not touch the live tab"
    );
    let _ = std::fs::remove_dir_all(stub.parent().expect("stub dir"));
}

/// prd-workspace-sessions-cli-v1 WS-O13 / WS-T4: the in-process doors
/// (`k2 sidecar new`, `k2 sessions open`, the `k2 msg` sidecar wake all
/// spawn `tab-*` through `spawn_agent_session_v2_blocking`) ask the same
/// seam as this route: the pinned chat's conversation and one another
/// live runner holds are refused (409 `conversation_live` for the
/// sidecar/open doors), the holder is untouched, and the one roster
/// shows who holds what.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn in_process_tab_spawns_ask_the_one_runner_seam() {
    k2_core::db::init_for_tests();
    let _home = StubHome::new();
    let n = NEXT_ID.fetch_add(1, Ordering::SeqCst);
    let pid = format!("seam-doors-{n}");
    let cwd = format!("/tmp/{pid}");
    let tab_a = format!("tab-{pid}-a");
    let tab_b = format!("tab-{pid}-b");
    let tab_c = format!("tab-{pid}-c");
    let _reap_a = ReapAgent(tab_a.clone());
    let _reap_b = ReapAgent(tab_b.clone());
    let _reap_c = ReapAgent(tab_c.clone());
    let stub = write_agent_stub("claude");
    let stub_s = stub.to_string_lossy().into_owned();
    let canonical_sid = uuid::Uuid::new_v4().to_string();
    let other_sid = uuid::Uuid::new_v4().to_string();
    seed_project_and_tab_row(&pid, &cwd, &tab_a, Some(stub_s.as_str()), None, &[]);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO workspace_sessions (id, project_id, session_id, harness, owner, status, created_at) \
                 VALUES (?1, ?2, ?3, 'claude', 'user', 'sleeping', unixepoch())",
            rusqlite::params![uuid::Uuid::new_v4().to_string(), pid, canonical_sid],
        )
        .expect("insert workspace_sessions");
    }
    let spawn_tab = |key: &str, sid: &str| {
        crate::spawn::spawn_agent_session_v2_blocking(crate::spawn::SpawnWorkspaceSessionRequest {
            agent_name: key.to_string(),
            project_id: Some(pid.clone()),
            cwd: cwd.clone(),
            command: Some(stub_s.clone()),
            args: Some(vec!["--resume".to_string(), sid.to_string()]),
            cols: 80,
            rows: 24,
            canonical_key: Some(key.to_string()),
            ..Default::default()
        })
    };

    // The pinned chat's conversation (asleep): refused, nothing registered.
    let err = spawn_tab(&tab_a, &canonical_sid).expect_err("a tab may not resume the main chat");
    let sentence = crate::conversation_owner::spawn_refusal_in(&err).expect("the seam's refusal");
    assert!(sentence.contains("the workspace's main chat"), "{err}");
    assert!(crate::v2_session_map::lookup_by_agent_name(&tab_a).is_none());
    let mapped = crate::sidecar_routes::spawn_error(&err);
    assert_eq!((mapped.code, mapped.status), ("conversation_live", 409), "{}", mapped.hint);

    // A live tab holds another conversation; a second tab on it is refused.
    let first = spawn_tab(&tab_b, &other_sid).expect("the first runner starts");
    assert!(!first.reused);
    let err = spawn_tab(&tab_c, &other_sid).expect_err("one conversation never runs twice");
    assert!(err.contains(&format!("another live session ({tab_b})")), "{err}");
    assert!(crate::v2_session_map::lookup_by_agent_name(&tab_c).is_none());
    assert!(
        crate::v2_session_map::lookup_by_agent_name(&tab_b).expect("holder stays").is_child_alive(),
        "refusing the duplicate must not touch the holder"
    );
    // Respawning the holder's own key is not a duplicate (reuse).
    let again = spawn_tab(&tab_b, &other_sid).expect("same key reuses");
    assert!(again.reused);

    // The seam's answer for each.
    let owner = crate::conversation_owner::conversation_owner(&other_sid, Some(&pid), None).expect("held");
    assert!(owner.live);
    assert_eq!(owner.agent_name.as_deref(), Some(tab_b.as_str()));
    let main = crate::conversation_owner::conversation_owner(&canonical_sid, Some(&pid), None).expect("main");
    assert_eq!((main.role, main.live), ("main", false));
    assert!(crate::conversation_owner::conversation_owner(&uuid::Uuid::new_v4().to_string(), Some(&pid), None).is_none());

    // The one roster (WS-R4): main asleep, the live tab as a chat (no
    // created_by), `you` marks the caller; asleep only with `all`.
    let roster = crate::workspace_roster::workspace_roster(
        &pid,
        &crate::workspace_roster::RosterOpts { all: true, include_api: false, caller_map_key: Some(tab_b.clone()) },
    )
    .expect("roster");
    let main_row = roster.rows.iter().find(|r| r.role == "main").expect("main row");
    assert_eq!(main_row.state, "asleep");
    assert_eq!(main_row.address.as_deref(), Some(roster.workspace_handle.as_str()));
    let live_tab = roster
        .rows
        .iter()
        .find(|r| r.agent_name.as_deref() == Some(tab_b.as_str()))
        .expect("live tab row");
    assert_eq!(live_tab.role, "chat");
    assert!(live_tab.you);
    assert_ne!(live_tab.state, "asleep");
    assert_eq!(live_tab.conversation_id.as_deref(), Some(other_sid.as_str()));
    assert!(live_tab.address.is_some(), "every live session has an address (ordinal)");
    assert_eq!(roster.you, live_tab.address);
    let live_only = crate::workspace_roster::workspace_roster(&pid, &Default::default()).expect("roster");
    assert!(live_only.rows.iter().all(|r| r.state != "asleep"), "asleep rows only with all");
    assert!(live_only.asleep >= 1, "asleep rows are still counted");
    let _ = std::fs::remove_dir_all(stub.parent().expect("stub dir"));
}

// ── Split-view V23: closed-tab guard ─────────────────────────────────

fn closed_tab_name() -> String {
    format!(
        "tab-closed-guard-{}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::SeqCst)
    )
}

fn unique_cwd(tag: &str) -> String {
    let dir = k2_core::test_env::stable_temp_dir().join(format!(
        "k2-closed-tab-{tag}-{}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).expect("cwd");
    dir.to_string_lossy().into_owned()
}

fn on_closed_list(agent: &str) -> bool {
    CLOSED_TABS.lock().is_closed(agent, std::time::Instant::now())
}

#[test]
fn closed_tab_rule_matrix() {
    let t0 = std::time::Instant::now();
    let mut closed = ClosedTabs::default();
    closed.record("tab-a", t0);
    closed.record("0192aaaa-bbbb-7000-8000-000000000000", t0);

    // Refused: tab-*, empty command (absent or blank), no live child, listed.
    assert!(closed_tab_refuses(&closed, "tab-a", None, false, t0));
    assert!(closed_tab_refuses(&closed, "tab-a", Some(""), false, t0));
    assert!(closed_tab_refuses(&closed, "tab-a", Some("  "), false, t0));
    // A command, a live child, or an unlisted name → proceed.
    assert!(!closed_tab_refuses(&closed, "tab-a", Some("claude"), false, t0));
    assert!(!closed_tab_refuses(&closed, "tab-a", None, true, t0));
    assert!(!closed_tab_refuses(&closed, "tab-b", None, false, t0));
    // Non-`tab-*` names (canonical key) are never refused.
    assert!(!closed_tab_refuses(
        &closed,
        "0192aaaa-bbbb-7000-8000-000000000000",
        None,
        false,
        t0
    ));
    // Expiry: still refused just inside the TTL, not at it.
    let almost = t0 + CLOSED_TAB_TTL - std::time::Duration::from_secs(1);
    assert!(closed_tab_refuses(&closed, "tab-a", None, false, almost));
    assert!(!closed_tab_refuses(&closed, "tab-a", None, false, t0 + CLOSED_TAB_TTL));
    // Clear drops the entry.
    closed.clear("tab-a");
    assert!(!closed_tab_refuses(&closed, "tab-a", None, false, t0));
}

#[test]
fn closed_tab_list_caps_and_evicts_oldest() {
    let t0 = std::time::Instant::now();
    let mut closed = ClosedTabs::default();
    for i in 0..(CLOSED_TAB_CAP + 5) {
        closed.record(
            &format!("tab-cap-{i}"),
            t0 + std::time::Duration::from_millis(i as u64),
        );
    }
    assert_eq!(closed.len(), CLOSED_TAB_CAP);
    let now = t0 + std::time::Duration::from_millis((CLOSED_TAB_CAP + 5) as u64);
    for i in 0..5 {
        assert!(
            !closed.is_closed(&format!("tab-cap-{i}"), now),
            "oldest entry tab-cap-{i} must be evicted"
        );
    }
    assert!(closed.is_closed("tab-cap-5", now));
    assert!(closed.is_closed(&format!("tab-cap-{}", CLOSED_TAB_CAP + 4), now));
}

#[test]
fn closed_tab_record_prunes_expired_entries() {
    let t0 = std::time::Instant::now();
    let mut closed = ClosedTabs::default();
    closed.record("tab-old", t0);
    closed.record("tab-new", t0 + CLOSED_TAB_TTL);
    assert_eq!(closed.len(), 1, "expired entry must be pruned on record");
    assert!(closed.is_closed("tab-new", t0 + CLOSED_TAB_TTL));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn close_with_tab_close_reason_refuses_empty_spawn_until_command_spawn() {
    // Declared first so it drops last (after the session is killed).
    let _env = spawn_env_lock();
    k2_core::db::init_for_tests();
    let agent = closed_tab_name();
    let _reap = ReapAgent(agent.clone());
    let cwd = unique_cwd("refuse");

    let closed = handle_v2_close(
        serde_json::json!({ "agent_name": agent, "force": true, "reason": "tab_close" })
            .to_string()
            .as_bytes(),
    );
    assert_eq!(closed.status, "200 OK", "{}", closed.body);
    assert!(on_closed_list(&agent), "tab_close must record the name");

    let refused = handle_v2_spawn(
        serde_json::json!({ "agent_name": agent, "cwd": cwd })
            .to_string()
            .as_bytes(),
    );
    assert_eq!(refused.status, "409 Conflict", "{}", refused.body);
    let body = json_body(&refused.body);
    assert_eq!(body["error"].as_str(), Some("tab_closed"), "{body}");
    assert_eq!(body["agent_name"].as_str(), Some(agent.as_str()), "{body}");
    assert!(
        crate::v2_session_map::lookup_by_agent_name(&agent).is_none(),
        "a refused spawn must not register a session"
    );

    // A blank command string is still an empty spawn.
    let blank = handle_v2_spawn(
        serde_json::json!({ "agent_name": agent, "cwd": cwd, "command": "" })
            .to_string()
            .as_bytes(),
    );
    assert_eq!(blank.status, "409 Conflict", "{}", blank.body);

    // A spawn that carries a command proceeds and clears the entry.
    let (cmd, args) = k2_core::test_env::sleep_command(30);
    let with_cmd = handle_v2_spawn(
        serde_json::json!({ "agent_name": agent, "cwd": cwd, "command": cmd, "args": args })
            .to_string()
            .as_bytes(),
    );
    assert_eq!(with_cmd.status, "200 OK", "{}", with_cmd.body);
    assert!(!on_closed_list(&agent), "command spawn must clear the entry");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn close_without_reason_does_not_refuse_empty_spawn() {
    // Declared first so it drops last (after the session is killed).
    let _env = spawn_env_lock();
    k2_core::db::init_for_tests();
    let agent = closed_tab_name();
    let _reap = ReapAgent(agent.clone());
    let cwd = unique_cwd("no-reason");

    let closed = handle_v2_close(
        serde_json::json!({ "agent_name": agent, "force": true })
            .to_string()
            .as_bytes(),
    );
    assert_eq!(closed.status, "200 OK", "{}", closed.body);
    assert!(!on_closed_list(&agent), "a close without reason must not record");

    // Another reason string is not tab_close either.
    let other = handle_v2_close(
        serde_json::json!({ "agent_name": agent, "force": true, "reason": "reap" })
            .to_string()
            .as_bytes(),
    );
    assert_eq!(other.status, "200 OK", "{}", other.body);
    assert!(!on_closed_list(&agent), "reason other than tab_close must not record");

    let spawned = handle_v2_spawn(
        serde_json::json!({ "agent_name": agent, "cwd": cwd })
            .to_string()
            .as_bytes(),
    );
    assert_eq!(spawned.status, "200 OK", "{}", spawned.body);
}

#[test]
fn close_with_tab_close_reason_ignores_non_tab_names() {
    let agent = uniq_agent_name();
    assert!(!agent.starts_with("tab-"));
    let closed = handle_v2_close(
        serde_json::json!({ "agent_name": agent, "reason": "tab_close" })
            .to_string()
            .as_bytes(),
    );
    assert_eq!(closed.status, "200 OK", "{}", closed.body);
    assert!(!on_closed_list(&agent), "non-tab names are never recorded");
}

/// Refresh always respawns with the recovered command, so the V23 gate
/// lets it through and clears the entry. Checked at the gate itself: a
/// full refresh spawns the agent stub, which the temp-HOME guard refuses
/// whenever a parallel test has HOME under temp (the same race the
/// existing `refresh_*` PTY tests hit).
#[test]
fn spawn_with_a_command_is_not_refused_and_clears_the_closed_entry() {
    let agent = closed_tab_name();
    let cwd = unique_cwd("command-clears");
    CLOSED_TABS.lock().record(&agent, std::time::Instant::now());

    let empty: SpawnRequest =
        serde_json::from_value(serde_json::json!({ "agent_name": agent, "cwd": cwd }))
            .expect("spawn request");
    let refused = closed_tab_gate(&empty).expect("empty-command spawn of a closed tab is refused");
    assert_eq!(refused.status, "409 Conflict");
    assert_eq!(json_body(&refused.body)["error"].as_str(), Some("tab_closed"));
    assert!(on_closed_list(&agent), "a refusal keeps the entry");

    let with_command: SpawnRequest = serde_json::from_value(serde_json::json!({
        "agent_name": agent,
        "cwd": cwd,
        "command": "claude",
        "args": ["--resume", "01920000-cccc-7000-8000-0000000000cc"],
    }))
    .expect("spawn request");
    assert!(closed_tab_gate(&with_command).is_none(), "a spawn with a command proceeds");
    assert!(!on_closed_list(&agent), "a spawn with a command clears the entry");
    assert!(closed_tab_gate(&empty).is_none(), "after clearing, an empty spawn proceeds too");
    let _ = std::fs::remove_dir_all(&cwd);
}
