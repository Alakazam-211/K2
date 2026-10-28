//! `POST /cli/agent/harness` (0.46.6). Who may move a pinned chat, what an
//! agent gets instead, idempotence, and — with `exec cat` shims, never a
//! real CLI — a person's move end to end: the row moves, heartbeats on the
//! pinned chat then fire into the new session, the old chat is still in
//! Chat history, and a second call changes nothing.

use super::*;
use crate::sidecar_routes::{AgentCaller, Caller};
use k2_core::db::schema::{AgentHeartbeat, WorkspaceSession};
use serde_json::{json, Value};

fn agent_of(project_id: &str) -> Caller {
    Caller::Agent(AgentCaller { project_id: project_id.to_string(), map_key: None, args: Vec::new(), sandboxed: false })
}

/// A registered workspace on disk under the temp HOME: (id, path, handle).
fn workspace(home: &crate::test_support::TempHome, tag: &str) -> (String, String, String) {
    let handle = format!("{tag}{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let dir = home.path().join(&handle);
    std::fs::create_dir_all(&dir).expect("workspace dir");
    let path = dir.to_string_lossy().to_string();
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO projects (id, name, path, handle, agent_mode, agent_enabled) VALUES (?1, ?2, ?3, ?2, 'manager', 1)",
        rusqlite::params![id, handle, path],
    )
    .expect("insert project");
    (id, path, handle)
}

/// Pin the workspace's chat to `harness` / `session`.
fn pin(project_id: &str, harness: &str, session: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    WorkspaceSession::upsert(&conn, &format!("ws-{}", uuid::Uuid::new_v4()), project_id, None, Some(session), harness, "user", "running")
        .expect("pin");
}

fn pinned(project_id: &str) -> (String, Option<String>) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row = WorkspaceSession::get(&conn, project_id).expect("read").expect("a pinned row");
    (row.harness, row.session_id)
}

fn call(caller: &Caller, body: Value) -> (String, Value) {
    let r = handle(body.to_string().as_bytes(), caller, "test");
    (r.status.to_string(), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{e}: {}", r.body)))
}

fn live(project_id: &str) -> Option<std::sync::Arc<k2_core::terminal::DaemonPtySession>> {
    crate::v2_session_map::lookup_by_agent_name(&crate::canonical_session::canonical_key_for(project_id))
}

/// `exec cat` shims for the harness CLIs; the daemon resolves agent
/// programs only inside this dir while it is set.
fn shims(home: &crate::test_support::TempHome) -> k2_core::test_env::EnvVar {
    shims_for(home, &["claude", "grok", "codex", "cat"])
}

/// Only `bins` are "installed": every other agent CLI is missing.
fn shims_for(home: &crate::test_support::TempHome, bins: &[&str]) -> k2_core::test_env::EnvVar {
    let dir = home.path().join("agent-shims");
    std::fs::create_dir_all(&dir).expect("shim dir");
    for bin in bins {
        k2_core::test_env::write_executable(&dir.join(bin), "#!/bin/sh\nexec cat\n");
    }
    k2_core::test_env::EnvVar::set("K2_TEST_AGENT_SHIM_DIR", &dir)
}

/// 0.46.6: a move never leaves the pinned chat empty. The target CLI is
/// checked before anything is touched; missing → 409
/// harness_not_installed with the install hint, and the old chat keeps
/// running on the same session. Dry run and the older swap-canonical
/// door answer the same.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_missing_target_harness_is_refused_and_the_old_chat_keeps_running() {
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let _shims = shims_for(&home, &["grok"]); // claude is not installed
    let (id, path, handle) = workspace(&home, "nocli");
    pin(&id, "grok", &uuid::Uuid::new_v4().to_string());
    let started = crate::pinned_chat::ensure_pinned_chat(&path, false, false).expect("grok pinned chat starts");
    let before = live(&id).expect("live grok chat");
    assert!(before.is_child_alive());
    let row_before = pinned(&id);

    for body in [
        json!({ "workspace": handle, "harness": "claude" }),
        json!({ "workspace": handle, "harness": "claude", "dryRun": true }),
    ] {
        let (s, v) = call(&Caller::Owner, body.clone());
        assert_eq!(s, "409 Conflict", "{body}: {v}");
        assert_eq!(v["error"]["code"], crate::pinned_chat::HARNESS_NOT_INSTALLED, "{v}");
        assert_eq!(v["error"]["harness"], "claude", "{v}");
        let hint = v["error"]["hint"].as_str().expect("hint");
        assert!(hint.contains("'claude' is not installed"), "the install hint: {hint}");
        assert!(hint.contains("not touched"), "{hint}");
        assert_eq!(v["switched"], false, "{v}");
    }
    let swap = crate::pinned_chat::handle_swap_canonical(
        json!({ "project": path, "provider": "claude" }).to_string().as_bytes(),
        &Caller::Owner,
        "test",
    );
    assert_eq!(swap.status, "409 Conflict", "{}", swap.body);
    assert!(swap.body.contains(crate::pinned_chat::HARNESS_NOT_INSTALLED), "{}", swap.body);

    // The old chat is untouched: same live session, same row, no handoff.
    let after = live(&id).expect("the grok chat is still live");
    assert_eq!(after.session_id, before.session_id, "never respawned");
    assert!(after.is_child_alive(), "never killed");
    assert_eq!(after.session_id.to_string(), started.session_id);
    assert_eq!(pinned(&id), row_before, "the pinned row did not move");
    assert!(!std::path::Path::new(&path).join(".k2").join("handoffs").exists(), "no handoff written");

    crate::v2_session_map::unregister(&crate::canonical_session::canonical_key_for(&id));
}

#[test]
fn an_agent_is_held_not_switched() {
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let (id, path, handle) = workspace(&home, "hold");
    let old = uuid::Uuid::new_v4().to_string();
    pin(&id, "grok", &old);

    // By path, by handle, any spelling of the harness: always refused.
    for ws in [path.clone(), handle.clone()] {
        let (s, v) = call(&agent_of(&id), json!({ "workspace": ws, "harness": "Claude" }));
        assert_eq!(s, "403 Forbidden", "{v}");
        assert_eq!(v["error"]["code"], NEEDS_PERSON, "{v}");
        assert_eq!(v["error"]["harness"], "claude", "the walls read the normalized harness: {v}");
        assert_eq!(v["error"]["workspace"], handle.as_str(), "{v}");
        assert_eq!(v["switched"], false, "{v}");
        let hint = v["error"]["hint"].as_str().expect("hint");
        assert!(hint.contains(&format!("k2 agent harness {handle} claude")), "the hint names the person's command: {hint}");
    }
    // Nothing moved, nothing spawned, no handoff written.
    assert_eq!(pinned(&id), ("grok".to_string(), Some(old)));
    assert!(live(&id).is_none(), "a refused switch never spawns");
    assert!(!std::path::Path::new(&path).join(".k2").join("handoffs").exists(), "no handoff for a refused switch");
    let audit = std::fs::read_to_string(home.path().join(".k2").join("auth-audit.jsonl")).expect("the refusal is audited");
    assert!(audit.contains("\"event\":\"harness-switch\"") && audit.contains("refused:needs_person"), "{audit}");
}

#[test]
fn an_agent_cannot_move_another_workspace() {
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let (me, _, _) = workspace(&home, "me");
    let (other, other_path, _) = workspace(&home, "other");
    pin(&other, "grok", &uuid::Uuid::new_v4().to_string());
    let (s, v) = call(&agent_of(&me), json!({ "workspace": other_path, "harness": "claude" }));
    assert_eq!(s, "403 Forbidden", "{v}");
    assert_eq!(v["error"]["code"], "other_workspace", "{v}");
    assert_eq!(pinned(&other).0, "grok");
    assert!(live(&other).is_none());
}

#[test]
fn bad_requests_credentials_and_member_logins_are_refused_before_anything_moves() {
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    // The Admin dry run below preflights claude: it must be installed.
    let _shims = shims(&home);
    let (id, path, _) = workspace(&home, "bad");
    pin(&id, "grok", &uuid::Uuid::new_v4().to_string());

    for caller in [Caller::WrongCredential, Caller::Invalid] {
        let (s, v) = call(&caller, json!({ "workspace": path, "harness": "claude" }));
        assert_eq!(s, "403 Forbidden", "{caller:?}: {v}");
    }
    let (s, v) = call(&Caller::Owner, json!({ "workspace": path }));
    assert_eq!((s.as_str(), &v["error"]["code"]), ("400 Bad Request", &json!("usage")), "{v}");
    let (s, v) = call(&Caller::Owner, json!({ "workspace": path, "harness": "aider" }));
    assert_eq!((s.as_str(), &v["error"]["code"]), ("400 Bad Request", &json!("unknown_harness")), "{v}");
    let (s, v) = call(&Caller::Owner, json!({ "workspace": path, "harness": "claude", "mode": "everything" }));
    assert_eq!((s.as_str(), &v["error"]["code"]), ("400 Bad Request", &json!("usage")), "{v}");
    let (s, v) = call(&Caller::Owner, json!({ "workspace": "no-such-workspace-xyz", "harness": "claude" }));
    assert_eq!((s.as_str(), &v["error"]["code"]), ("404 Not Found", &json!("not_found")), "{v}");

    let member = format!("hm{}", &uuid::Uuid::new_v4().simple().to_string()[..6]);
    k2_core::connect_users::add_user(&member, "password123").expect("add member");
    let (s, v) = call(&Caller::Login { username: member }, json!({ "workspace": path, "harness": "claude" }));
    assert_eq!((s.as_str(), &v["error"]["code"]), ("403 Forbidden", &json!("admin_only")), "{v}");

    // An Admin passes the gate (a dry run changes nothing).
    let admin = format!("ha{}", &uuid::Uuid::new_v4().simple().to_string()[..6]);
    k2_core::connect_users::add_user(&admin, "password123").expect("add admin");
    k2_core::connect_users::set_role(&admin, k2_core::connect_users::Role::Admin).expect("admin");
    let (s, v) = call(&Caller::Login { username: admin }, json!({ "workspace": path, "harness": "claude", "dryRun": true }));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!((v["dryRun"].clone(), v["fromHarness"].clone(), v["harness"].clone()), (json!(true), json!("grok"), json!("claude")), "{v}");

    assert_eq!(pinned(&id).0, "grok", "nothing above moved the pinned chat");
    assert!(live(&id).is_none());
}

#[test]
fn already_on_the_harness_is_a_no_op() {
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let (id, path, handle) = workspace(&home, "same");
    let sid = uuid::Uuid::new_v4().to_string();
    pin(&id, "claude", &sid);
    let (s, v) = call(&Caller::Owner, json!({ "workspace": handle, "harness": "claude --dangerously-skip-permissions" }));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!((v["switched"].clone(), v["alreadyOn"].clone()), (json!(false), json!(true)), "{v}");
    assert_eq!(pinned(&id), ("claude".to_string(), Some(sid)));
    assert!(live(&id).is_none(), "already on it: nothing spawned");
    assert!(!std::path::Path::new(&path).join(".k2").join("handoffs").exists());
}

/// A grok session on disk for `project_path` (what a real grok writes), so
/// Chat history lists it.
fn grok_session_on_disk(home: &std::path::Path, project_path: &str, id: &str) {
    let dir = home.join(".grok").join("sessions").join("%2Ffixture").join(id);
    std::fs::create_dir_all(&dir).expect("grok session dir");
    std::fs::write(
        dir.join("summary.json"),
        json!({ "info": { "id": id, "cwd": project_path }, "last_active_at": "2026-10-27T10:00:00Z" }).to_string(),
    )
    .expect("summary.json");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_person_switches_and_heartbeats_follow_the_new_chat() {
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let _shims = shims(&home);
    let (id, path, handle) = workspace(&home, "move");
    let old = uuid::Uuid::new_v4().to_string();
    grok_session_on_disk(home.path(), &path, &old);
    pin(&id, "grok", &old);

    // An hourly heartbeat on the pinned chat (the default for new rows).
    let rel = ".k2/heartbeats/hourly/WAKEUP.md";
    std::fs::create_dir_all(std::path::Path::new(&path).join(".k2/heartbeats/hourly")).unwrap();
    std::fs::write(std::path::Path::new(&path).join(rel), "check the queue\n").unwrap();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        AgentHeartbeat::insert(&conn, &uuid::Uuid::new_v4().to_string(), &id, "hourly", "hourly", "{}", rel, true).expect("heartbeat");
        let hb = AgentHeartbeat::get_by_name(&conn, &id, "hourly").unwrap().expect("row");
        assert!(hb.use_workspace_session, "new heartbeats deliver into the pinned chat");
    }
    let before = k2_core::chat_history::list_all_sessions(Some(&path)).expect("chat history");
    assert!(before.iter().any(|r| r.session_id == old && r.provider == "grok"), "the grok chat is listed before: {before:?}");

    let (s, v) = call(&Caller::Owner, json!({ "workspace": handle, "harness": "claude" }));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v["switched"], true, "{v}");
    assert_eq!((v["fromHarness"].clone(), v["harness"].clone()), (json!("grok"), json!("claude")), "{v}");
    assert_eq!(v["fromSessionId"], old.as_str(), "{v}");
    assert_eq!(v["oldChatInHistory"], true, "{v}");
    assert_eq!(v["ownSessionHeartbeats"], json!([]), "{v}");
    let new_session = v["sessionId"].as_str().expect("sessionId").to_string();
    let new_conv = v["claudeSessionId"].as_str().expect("claudeSessionId").to_string();
    assert_ne!(new_conv, old);

    // The pinned row now names the new claude chat.
    assert_eq!(pinned(&id), ("claude".to_string(), Some(new_conv.clone())));
    let running = live(&id).expect("the new pinned chat is live");
    assert_eq!(running.session_id.to_string(), new_session);
    assert!(running.args.iter().any(|a| a == &new_conv), "a fresh claude chat: {:?}", running.args);
    assert!(!running.args.iter().any(|a| a == &old || a == "--resume"), "never resumes the grok chat: {:?}", running.args);

    // The old chat is still in Chat history.
    let after = k2_core::chat_history::list_all_sessions(Some(&path)).expect("chat history");
    assert!(after.iter().any(|r| r.session_id == old && r.provider == "grok"), "the old chat stays listed: {after:?}");

    // The heartbeat fires into the new session.
    let fired = crate::heartbeat_launch::smart_launch(&path, "hourly");
    assert_eq!(fired["success"], true, "{fired}");
    assert!(fired["branch"].as_str().unwrap_or("").starts_with("workspace_session:"), "{fired}");
    assert_eq!(fired["targetSessionId"], new_session.as_str(), "the heartbeat targets the new chat: {fired}");

    // Idempotent: a second call changes nothing.
    let (s, v) = call(&Caller::Owner, json!({ "workspace": path, "harness": "claude" }));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v["alreadyOn"], true, "{v}");
    assert_eq!(live(&id).expect("still live").session_id.to_string(), new_session, "no respawn");
    assert_eq!(pinned(&id).1, Some(new_conv));

    crate::v2_session_map::unregister(&crate::canonical_session::canonical_key_for(&id));
}
