//! 0.46.18: K2 tells an agent session from a person's own shell tab by how
//! K2 started it, never by the program it runs.
//!
//! Before, a passport was classed by its session's program: a known LLM CLI
//! was an agent, anything else a person's shell tab, treated as the owner.
//! An agent whose launch said `bash` (an AGENT.md `launch:` block), `npx`
//! or a wrapper script was then a shell tab and skipped the agent gates.
//!
//! Headless: the real dispatcher in process on a temp `$HOME`, real PTYs
//! (`bash`, and `cat` behind an `npx` / wrapper name), no real agent CLI,
//! no real `~/.k2`, no keychain. Each test checks the class
//! (`caller_from_session`), an agent gate (`k2 sidecar stop`: `gated` while
//! "Let this agent manage its sidecars" is off; a person's shell tab skips
//! it) and a person-only gate (`k2 sessions model`: every passport is
//! refused, a shell tab's too).

use std::collections::HashMap;
use std::sync::Arc;

use k2_core::session::SessionId;
use k2_core::terminal::{DaemonPtyConfig, DaemonPtySession};
use k2_daemon::session_token::HookPrincipal;
use k2_daemon::sidecar_routes::{caller_from_session, Caller};
use k2_daemon::spawn::{spawn_agent_session_v2_blocking, SpawnWorkspaceSessionRequest};
use serde_json::{json, Value};

use crate::support::app_fixture::*;

/// Ends the sessions a test registered, passing or panicking.
struct Live(Vec<String>);

impl Drop for Live {
    fn drop(&mut self) {
        for key in &self.0 {
            if let Some(s) = k2_daemon::v2_session_map::unregister(key) {
                s.kill();
            }
        }
    }
}

fn class_of(ws: &Ws, key: &str, sid: &SessionId) -> Caller {
    let principal = HookPrincipal { workspace_uuid: ws.id.clone(), agent_address: key.to_string() };
    caller_from_session(&sid.to_string(), &principal)
}

/// A program that stays up in a PTY under the name `name`, outside the
/// workspace folder.
fn program(env: &Env, name: &str) -> String {
    let dir = env.path().join("bin");
    std::fs::create_dir_all(&dir).expect("bin dir");
    let path = dir.join(name);
    k2_core::test_env::write_executable(&path, "#!/bin/sh\nexec /bin/cat\n");
    path.to_string_lossy().into_owned()
}

/// A session K2 starts for an agent through the daemon's spawn helper (the
/// pinned chat, a heartbeat, a sidecar, `k2 terminal spawn --agent`).
fn agent_spawn(ws: &Ws, key: &str, command: &str, args: &[&str]) -> SessionId {
    spawn_agent_session_v2_blocking(SpawnWorkspaceSessionRequest {
        agent_name: key.to_string(),
        project_id: Some(ws.id.clone()),
        cwd: ws.path.to_string_lossy().into_owned(),
        command: Some(command.to_string()),
        args: Some(args.iter().map(|s| s.to_string()).collect()),
        cols: 80,
        rows: 24,
        canonical_key: Some(key.to_string()),
        ..Default::default()
    })
    .unwrap_or_else(|e| panic!("spawn {key} ({command}): {e}"))
    .session_id
}

/// A spawn a person's K2 client asks for: the owner token on
/// `/cli/sessions/v2/spawn`.
async fn person_spawn(port: u16, ws: &Ws, key: &str, command: Option<&str>, args: &[&str]) -> SessionId {
    let body = json!({
        "agent_name": key,
        "cwd": ws.path.to_string_lossy(),
        "command": command,
        "args": if args.is_empty() { Value::Null } else { json!(args) },
        "cols": 80,
        "rows": 24,
    });
    let r = post(port, "/cli/sessions/v2/spawn", OWNER, body).await;
    assert_eq!(r.status, 200, "spawn {key}: {}", r.body);
    let raw = r.json()["sessionId"].as_str().map(str::to_string).unwrap_or_else(|| panic!("no sessionId: {}", r.body));
    SessionId::parse(&raw).unwrap_or_else(|| panic!("bad sessionId {raw}"))
}

fn status_and_code(r: k2_daemon::cli_response::CliResponse) -> (String, String) {
    let v: Value = serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("JSON ({e}): {}", r.body));
    let code = v["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {}", r.body)).to_string();
    (r.status.to_string(), code)
}

/// The sidecar agent gate (`k2 sidecar stop` of a sidecar that isn't there).
fn sidecar_stop(caller: Caller, ws: &Ws) -> (String, String) {
    let body = json!({ "workspace": ws.handle, "name": "nobody-here" }).to_string();
    status_and_code(k2_daemon::sidecar_routes::handle("/cli/sidecar/stop", caller, body.as_bytes(), &HashMap::new(), "cli"))
}

/// A person-only gate (`k2 sessions model`).
fn model_switch(caller: Caller) -> (String, String) {
    let body = json!({ "session_id": SessionId::new().to_string(), "model": "opus" }).to_string();
    status_and_code(k2_daemon::workspace_sessions_routes::handle(
        k2_daemon::workspace_sessions_routes::MODEL,
        caller,
        body.as_bytes(),
        &HashMap::new(),
        "cli",
    ))
}

fn gated() -> (String, String) {
    ("403 Forbidden".to_string(), "gated".to_string())
}

fn needs_person() -> (String, String) {
    ("403 Forbidden".to_string(), "model_switch_needs_person".to_string())
}

/// The agent's gates hold, the person-only one too.
fn assert_agent(ws: &Ws, key: &str, sid: &SessionId, what: &str) {
    let c = class_of(ws, key, sid);
    assert!(matches!(c, Caller::Agent(_)), "{what}: an agent's session, got {c:?}");
    assert_eq!(sidecar_stop(c.clone(), ws), gated(), "{what}: the agent gate still applies");
    assert_eq!(model_switch(c), needs_person(), "{what}: the person-only gate is unchanged");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_agent_session_is_an_agent_whatever_its_launch_runs() {
    let env = setup(&[]);
    let _d = k2_daemon::test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "originagent");
    let npx = program(&env, "npx");
    let wrapper = program(&env, "run-my-agent.sh");
    let hb = |tag: &str| format!("{}:hb:origin-{tag}", ws.id);
    let terminal = format!("terminal-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let cases: Vec<(String, String, Vec<&str>, &str)> = vec![
        // The finding: an AGENT.md `launch:` block that says `bash`, in
        // the pinned chat's own slot.
        (ws.id.clone(), "bash".to_string(), vec!["--norc", "--noprofile"], "pinned chat launched as bash"),
        (hb("bash"), "bash".to_string(), vec!["--norc", "--noprofile"], "heartbeat launched as bash"),
        (hb("npx"), npx.clone(), vec![], "heartbeat launched through npx"),
        (hb("wrapper"), wrapper.clone(), vec![], "heartbeat launched through a wrapper script"),
        (terminal, "bash".to_string(), vec!["--norc", "--noprofile"], "k2 terminal spawn --agent with bash"),
    ];
    let _live = Live(cases.iter().map(|c| c.0.clone()).collect());
    for (key, command, args, what) in &cases {
        let sid = agent_spawn(&ws, key, command, args);
        assert_agent(&ws, key, &sid, what);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_persons_own_shell_tab_is_a_shell() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "originshell");
    let key = format!("tab-{}", uuid::Uuid::new_v4());
    let _live = Live(vec![key.clone()]);
    let sid = person_spawn(d.port, &ws, &key, None, &[]).await;
    let c = class_of(&ws, &key, &sid);
    assert_eq!(c, Caller::Shell { project_id: ws.id.clone() }, "a person's own shell tab");
    // It skips the agent gate (the person is typing) and reaches the route.
    assert_eq!(sidecar_stop(c.clone(), &ws), ("404 Not Found".to_string(), "no_such_sidecar".to_string()));
    // A person-only gate still refuses its passport.
    assert_eq!(model_switch(c), needs_person());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_person_door_spawn_of_anything_but_a_plain_shell_tab_is_an_agent() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "origindoor");
    let wrapper = program(&env, "npx");
    let tab = format!("tab-{}", uuid::Uuid::new_v4());
    let _live = Live(vec![ws.id.clone(), tab.clone()]);
    // The pinned chat's slot, whatever the client starts there.
    let sid = person_spawn(d.port, &ws, &ws.id, Some("bash"), &["--norc", "--noprofile"]).await;
    assert_agent(&ws, &ws.id, &sid, "the pinned chat's slot running bash");
    // A tab running a launcher, not the plain login shell.
    let sid = person_spawn(d.port, &ws, &tab, Some(&wrapper), &[]).await;
    assert_agent(&ws, &tab, &sid, "a tab running npx");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_with_no_recorded_origin_is_an_agent() {
    let env = setup(&[]);
    let _d = k2_daemon::test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "originnone");
    // A plain login shell under a tab's key that no spawn door recorded.
    let session: Arc<DaemonPtySession> = DaemonPtySession::spawn(DaemonPtyConfig {
        cwd: Some(ws.path.clone()),
        ..Default::default()
    })
    .expect("spawn a plain shell");
    let key = format!("tab-{}", uuid::Uuid::new_v4());
    k2_daemon::v2_session_map::register(key.clone(), Arc::clone(&session));
    let _live = Live(vec![key.clone()]);
    assert_agent(&ws, &key, &session.session_id, "no recorded origin");
}
