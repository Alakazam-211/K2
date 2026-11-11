//! The first pinned chat on a brand-new server (Rosson, 2026-11-10): "When
//! you have no sessions (ever) and you make your first agent, the pinned
//! chat tab doesn't ask you what LLM you want to try and launch a session
//! for."
//!
//! A new install's first agent is stamped with the global default agent
//! (`claude`, `projects_ops::stamp_default_agent`), and the Chat tab's
//! `ensure-pinned-chat` resolved that straight to `claude
//! --dangerously-skip-permissions --session-id <new>`: a silent Claude
//! spawn when Claude is installed, `spawn failed: … not found` when it
//! isn't. The pinned chat must ask first.
//!
//! Each test gets a temp `$HOME`. The ones that start nothing also get this
//! thread's own fresh DB (`db::scoped_for_test`), so "no session ever on
//! this server" is true; the ones that spawn an `exec cat` shim stay on the
//! shared test DB (the spawn reads it from other threads) and set up only
//! what they need to be deterministic there.

use super::*;
use serde_json::Value;

/// `exec cat` shims for `bins` only: every other agent CLI is missing.
fn installed(home: &k2_core::test_env::TempHome, bins: &[&str]) -> k2_core::test_env::EnvVar {
    let dir = home.path().join("agent-shims");
    std::fs::create_dir_all(&dir).expect("shim dir");
    for bin in bins {
        k2_core::test_env::write_executable(&dir.join(bin), "#!/bin/sh\nexec cat\n");
    }
    k2_core::test_env::EnvVar::set(k2_core::terminal::agent_spawn_guard::SHIM_DIR_ENV, &dir)
}

/// The person's first agent, made the way Add Workspace makes it (the
/// global default agent is stamped on the new row): (id, path).
fn first_agent(home: &k2_core::test_env::TempHome) -> (String, String) {
    let dir = home.path().join(format!("first-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]));
    std::fs::create_dir_all(&dir).expect("workspace dir");
    let path = dir.to_string_lossy().to_string();
    let project = k2_core::projects_ops::projects_create("First", &path, None).expect("create the first agent");
    (project.id, path)
}

fn ensure(path: &str) -> (String, Value) {
    let r = handle_ensure_pinned_chat(serde_json::json!({ "project": path }).to_string().as_bytes());
    let v = serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{e}: {}", r.body));
    (r.status.to_string(), v)
}

fn live(project_id: &str) -> Option<std::sync::Arc<k2_core::terminal::DaemonPtySession>> {
    crate::v2_session_map::lookup_by_agent_name(&canonical_key_for(project_id))
}

fn pinned_row(project_id: &str) -> Option<k2_core::db::schema::WorkspaceSession> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::db::schema::WorkspaceSession::get(&conn, project_id).expect("read workspace_sessions")
}

/// The harness entries of a choice answer, by harness.
fn choices(v: &Value) -> std::collections::BTreeMap<String, Value> {
    v["error"]["harnesses"]
        .as_array()
        .unwrap_or_else(|| panic!("a harnesses list: {v}"))
        .iter()
        .map(|c| (c["harness"].as_str().expect("harness").to_string(), c.clone()))
        .collect()
}

/// Claude installed, nothing chosen, no session ever: the Chat tab must
/// ask, not start Claude on its own.
#[test]
fn a_new_servers_first_pinned_chat_asks_which_llm_instead_of_starting_claude() {
    let home = k2_core::test_env::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    let _bins = installed(&home, &["claude", "codex"]);
    let (id, path) = first_agent(&home);

    let (status, v) = ensure(&path);
    let spawned = live(&id);
    if spawned.is_some() {
        crate::v2_session_map::unregister(&canonical_key_for(&id));
    }
    assert_eq!(status, "409 Conflict", "the first chat started without asking: {v}");
    assert_eq!(v["error"]["code"], "harness_choice_needed", "{v}");
    assert!(spawned.is_none(), "nothing runs until the person picks: {v}");
    assert!(pinned_row(&id).is_none(), "no harness or session is pinned for them: {v}");

    let c = choices(&v);
    for h in ["claude", "codex", "grok"] {
        assert!(c.contains_key(h), "{h} is offered: {v}");
    }
    assert_eq!(c["claude"]["installed"], true, "{v}");
    assert_eq!(c["codex"]["installed"], true, "{v}");
    assert_eq!(c["grok"]["installed"], false, "{v}");
    assert_eq!(c["claude"]["signedIn"], false, "no K2 sign-in yet: {v}");
    assert_eq!(c["claude"]["signIn"], true, "{v}");
}

/// Nothing installed (a fresh Linux box): the same question, not
/// `spawn failed: … claude: not found on the child PATH`.
#[test]
fn a_new_server_with_no_llm_cli_asks_instead_of_failing_the_spawn() {
    let home = k2_core::test_env::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    let _bins = installed(&home, &[]);
    let (id, path) = first_agent(&home);

    let (status, v) = ensure(&path);
    assert_eq!(status, "409 Conflict", "{v}");
    assert_eq!(v["error"]["code"], "harness_choice_needed", "{v}");
    assert!(live(&id).is_none(), "{v}");
    let c = choices(&v);
    for h in ["claude", "codex", "grok"] {
        assert_eq!(c[h]["installed"], false, "{h}: {v}");
        assert_eq!(c[h]["install"], true, "K2 can install {h}: {v}");
    }
}

fn ensure_with(path: &str, harness: &str) -> (String, Value) {
    let body = serde_json::json!({ "project": path, "harness": harness }).to_string();
    let r = handle_ensure_pinned_chat(body.as_bytes());
    let v = serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{e}: {}", r.body));
    (r.status.to_string(), v)
}

fn default_agent(project_id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT default_agent FROM projects WHERE id = ?1", [project_id], |r| r.get(0))
        .expect("read default_agent")
}

fn preset_id_of(label: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT id FROM agent_presets WHERE label = ?1 AND is_built_in = 1", [label], |r| r.get(0))
        .expect("built-in preset")
}

/// Another workspace's pinned chat: the server has had a session.
fn other_agent_chatted() {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let other = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO projects (id, name, path) VALUES (?1, 'Other', ?2)",
        rusqlite::params![other, format!("/tmp/k2-other-{other}")],
    )
    .expect("other project");
    k2_core::db::schema::WorkspaceSession::upsert(
        &conn,
        &format!("ws-{other}"),
        &other,
        None,
        Some(&uuid::Uuid::new_v4().to_string()),
        "claude",
        "user",
        "sleeping",
    )
    .expect("other pinned chat");
}

/// The pick: Codex. It is saved as the agent's default and the chat starts
/// on it; the next visit finds the same chat, with no question.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn picking_a_harness_saves_it_and_starts_the_chat_on_it() {
    let home = k2_core::test_env::TempHome::new();
    k2_core::db::init_for_tests();
    let _bins = installed(&home, &["claude", "codex"]);
    let (id, path) = first_agent(&home);

    let (status, v) = ensure_with(&path, "Codex");
    assert_eq!(status, "200 OK", "{v}");
    assert_eq!(v["provider"], "codex", "{v}");
    assert_eq!(v["command"], "codex", "{v}");
    assert_eq!(default_agent(&id).as_deref(), Some(preset_id_of("Codex").as_str()), "the pick is the agent's default");
    let first = live(&id).expect("the codex chat runs");

    let (status, v) = ensure(&path);
    assert_eq!(status, "200 OK", "a later visit starts nothing new and asks nothing: {v}");
    assert_eq!(v["reused"], true, "{v}");
    assert_eq!(v["sessionId"], first.session_id.to_string(), "{v}");

    crate::v2_session_map::unregister(&canonical_key_for(&id));
}

/// A pick that isn't installed is refused before anything is saved.
#[test]
fn picking_a_harness_that_is_not_installed_saves_and_starts_nothing() {
    let home = k2_core::test_env::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    let _bins = installed(&home, &["claude"]);
    let (id, path) = first_agent(&home);
    let before = default_agent(&id);

    let (status, v) = ensure_with(&path, "grok");
    assert_eq!(status, "409 Conflict", "{v}");
    assert_eq!(v["error"]["code"], HARNESS_NOT_INSTALLED, "{v}");
    assert_eq!(v["error"]["harness"], "grok", "{v}");
    assert!(live(&id).is_none(), "{v}");
    assert!(pinned_row(&id).is_none(), "{v}");
    assert_eq!(default_agent(&id), before, "nothing saved");

    // Still asks afterwards.
    let (status, v) = ensure(&path);
    assert_eq!(status, "409 Conflict", "{v}");
    assert_eq!(v["error"]["code"], crate::pinned_chat_choice::CHOICE_NEEDED, "{v}");
}

#[test]
fn harness_is_refused_with_fresh_provider_or_explicit_selection() {
    for body in [
        serde_json::json!({ "project": "/tmp/x", "harness": "codex", "freshProvider": "claude" }),
        serde_json::json!({ "project": "/tmp/x", "harness": "codex", "explicitSelection": true }),
    ] {
        let r = handle_ensure_pinned_chat(body.to_string().as_bytes());
        assert_eq!(r.status, "400 Bad Request", "{body}: {}", r.body);
        assert!(r.body.contains("harness cannot be combined"), "{}", r.body);
    }
}

// ── Existing users: exactly as before ─────────────────────────────────

/// A server that has had a pinned chat: a new agent starts on its default
/// as before (no question).
#[test]
fn a_server_that_has_had_a_pinned_chat_never_asks() {
    let home = k2_core::test_env::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    let _bins = installed(&home, &["claude"]);
    let (_, path) = first_agent(&home);
    assert!(crate::pinned_chat_choice::needs_choice(&path).is_some(), "asks before");
    other_agent_chatted();
    assert_eq!(crate::pinned_chat_choice::needs_choice(&path), None);
}

/// Same, end to end: the ensure spawns Claude, as it did before 0.46.11.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_server_that_has_had_a_pinned_chat_starts_the_default_as_before() {
    let home = k2_core::test_env::TempHome::new();
    k2_core::db::init_for_tests();
    let _bins = installed(&home, &["claude"]);
    other_agent_chatted();
    let (id, path) = first_agent(&home);

    let (status, v) = ensure(&path);
    assert_eq!(status, "200 OK", "{v}");
    assert_eq!(v["provider"], "claude", "{v}");
    assert_eq!(v["command"], "claude", "{v}");
    assert_eq!(v["args"][0], "--dangerously-skip-permissions", "{v}");
    assert_eq!(v["args"][1], "--session-id", "{v}");
    assert!(live(&id).is_some(), "{v}");
    crate::v2_session_map::unregister(&canonical_key_for(&id));
}

/// An agent that already has a pinned chat keeps it: no question, and a
/// `harness` on the ensure changes nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agent_with_a_pinned_chat_resumes_it_and_ignores_a_pick() {
    let home = k2_core::test_env::TempHome::new();
    k2_core::db::init_for_tests();
    let _bins = installed(&home, &["claude", "codex"]);
    let (id, path) = first_agent(&home);
    let pin = uuid::Uuid::new_v4().to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::WorkspaceSession::upsert(&conn, &format!("ws-{id}"), &id, None, Some(&pin), "claude", "user", "sleeping")
            .expect("pin");
    }
    k2_core::workspace::resume_chat::note_unstarted_pin(&id, &pin);
    assert_eq!(crate::pinned_chat_choice::needs_choice(&path), None);
    let before = default_agent(&id);

    let (status, v) = ensure_with(&path, "codex");
    assert_eq!(status, "200 OK", "{v}");
    assert_eq!(v["provider"], "claude", "{v}");
    assert_eq!(v["claudeSessionId"], pin.as_str(), "the same pinned chat: {v}");
    assert_eq!(default_agent(&id), before, "the pick is ignored");
    crate::v2_session_map::unregister(&canonical_key_for(&id));
}

/// A new server whose default was set to another installed harness on
/// purpose starts that one, as before.
#[test]
fn a_default_set_to_another_installed_harness_is_not_asked_again() {
    let home = k2_core::test_env::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    let _bins = installed(&home, &["codex"]);
    let (id, path) = first_agent(&home);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE projects SET default_agent = ?1 WHERE id = ?2", rusqlite::params![preset_id_of("Codex"), id])
            .expect("set default");
    }
    assert_eq!(crate::pinned_chat_choice::needs_choice(&path), None);

    // Not installed: it asks, rather than fail the spawn.
    std::fs::remove_file(home.path().join("agent-shims").join("codex")).expect("uninstall codex");
    assert!(crate::pinned_chat_choice::needs_choice(&path).is_some());
}

/// Sign-in state comes from K2's LLM logins; air-gap turns sign-in off.
#[test]
fn choices_say_which_harness_is_signed_in() {
    let home = k2_core::test_env::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    let _bins = installed(&home, &["claude", "codex", "gemini"]);
    let (_, path) = first_agent(&home);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO llm_accounts (id, tool, label, kind, position, state, created_at) \
             VALUES ('acct-codex', 'codex', 'work', 'subscription', 0, 'signed_in', unixepoch())",
            [],
        )
        .expect("a signed-in codex login");
    }
    let c: std::collections::BTreeMap<_, _> = crate::pinned_chat_choice::needs_choice(&path)
        .expect("asks")
        .into_iter()
        .map(|c| (c.harness.clone(), c))
        .collect();
    assert!(c["codex"].signed_in);
    assert!(!c["claude"].signed_in);
    assert!(c.contains_key("gemini"), "an installed harness beyond the three is offered");
    assert!(!c["gemini"].sign_in, "K2 has no Gemini subscription sign-in");
    assert!(!c.contains_key("pi"), "a harness that isn't installed and isn't one of the three is not offered");
    let order: Vec<&str> = c.keys().map(String::as_str).collect();
    assert_eq!(order.len(), 4, "{order:?}");
}
