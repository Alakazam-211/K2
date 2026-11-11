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
//! Each test gets a temp `$HOME` and this thread's own fresh DB
//! (`db::scoped_for_test`), so "no session ever on this server" is true.

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
