//! `POST /cli/agent/refresh`: who may, what an agent gets instead, and, with
//! `exec cat` shims (never a real CLI), a refresh end to end: the pin moves,
//! the Thread follows it, the note reaches the handoff, and a second refresh
//! while the first's new chat has no id is refused.

use super::*;
use crate::sidecar_routes::{AgentCaller, Caller};
use k2_core::connect_users::Role;
use k2_core::db::schema::WorkspaceSession;
use serde_json::{json, Value};

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

fn pin(id: &str, harness: &str, session: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    WorkspaceSession::upsert(&conn, &format!("ws-{session}"), id, None, Some(session), harness, "system", "running").expect("pin");
}

fn pinned(id: &str) -> (Option<String>, String) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row = WorkspaceSession::get(&conn, id).expect("read").expect("row");
    (row.session_id, row.harness)
}

/// The workspace's main session (its map key is the project id).
fn main_session(id: &str) -> Caller {
    Caller::Agent(AgentCaller { project_id: id.to_string(), map_key: Some(id.to_string()), args: Vec::new(), sandboxed: false })
}

fn call(caller: &Caller, body: Value) -> (String, Value) {
    let r = handle(body.to_string().as_bytes(), caller, "test");
    (r.status.to_string(), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{e}: {}", r.body)))
}

fn shims(home: &crate::test_support::TempHome, clis: &[&str]) -> k2_core::test_env::EnvVar {
    let dir = home.path().join("agent-shims");
    std::fs::create_dir_all(&dir).expect("shim dir");
    for cli in clis {
        k2_core::test_env::write_executable(&dir.join(cli), "#!/bin/sh\nexec cat\n");
    }
    k2_core::test_env::EnvVar::set("K2_TEST_AGENT_SHIM_DIR", &dir)
}

fn post_thread(conv: &str, id: &str, text: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::overlay::post_thread(&conn, conv, id, "owner", "desk", text, "compose").expect("post");
}

fn thread_texts(conv: &str) -> Vec<String> {
    let page = k2_core::overlay::read_thread_page(conv, None, None, 50).expect("read");
    page.items.iter().map(|i| serde_json::to_value(&i.doc).expect("doc")["body"].as_str().unwrap_or_default().to_string()).collect()
}

fn end_chat(id: &str) {
    crate::v2_session_map::unregister(&crate::canonical_session::canonical_key_for(id));
}

#[test]
fn an_agent_is_refused_unless_its_own_main_session_has_the_switch_and_nothing_moves() {
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let (id, path, handle) = workspace(&home, "rfa");
    let (other_id, _, other_handle) = workspace(&home, "rfb");
    let old = uuid::Uuid::new_v4().to_string();
    pin(&id, "grok", &old);

    // Off: asks a person, and with cards off K2 keeps no note.
    let (s, v) = call(&main_session(&id), json!({ "notes": "next: finish" }));
    assert_eq!(s, "403 Forbidden", "{v}");
    assert_eq!(v["error"]["code"], NEEDS_PERSON, "{v}");
    assert!(v["error"]["notes"].is_null(), "no card to carry it: {v}");
    let hint = v["error"]["hint"].as_str().expect("hint");
    assert!(hint.contains(&format!("k2 agent refresh {handle} --notes-file")), "{hint}");
    assert!(hint.contains("Your note was not kept"), "{hint}");
    assert!(hint.contains("Can refresh its own pinned chat"), "{hint}");
    assert!(!std::path::Path::new(&path).join(".k2").join("handoffs").exists(), "nothing written");

    // On, the rule still holds for everything else.
    k2_core::workspace::settings::set_agents_can_refresh_chat(&id, true).expect("on");
    let sidecar = Caller::Agent(AgentCaller { project_id: id.clone(), map_key: Some("tab-1".into()), args: Vec::new(), sandboxed: false });
    let (s, v) = call(&sidecar, json!({}));
    assert_eq!((s.as_str(), &v["error"]["code"]), ("403 Forbidden", &json!(MAIN_ONLY)), "{v}");
    assert!(v["error"]["hint"].as_str().expect("hint").contains("under a new name"), "{v}");
    let sandboxed = Caller::Agent(AgentCaller { project_id: id.clone(), map_key: Some(id.clone()), args: Vec::new(), sandboxed: true });
    assert_eq!(call(&sandboxed, json!({})).1["error"]["code"], MAIN_ONLY);
    let (s, v) = call(&main_session(&id), json!({ "workspace": other_handle }));
    assert_eq!((s.as_str(), &v["error"]["code"]), ("403 Forbidden", &json!("other_workspace")), "{v}");

    // A switch on another workspace is not this one's.
    k2_core::workspace::settings::set_agents_can_refresh_chat(&id, false).expect("off");
    k2_core::workspace::settings::set_agents_can_refresh_chat(&other_id, true).expect("other on");
    assert_eq!(call(&main_session(&id), json!({})).1["error"]["code"], NEEDS_PERSON);

    assert_eq!(pinned(&id), (Some(old), "grok".to_string()), "the pin never moved");
    assert!(crate::v2_session_map::lookup_by_agent_name(&crate::canonical_session::canonical_key_for(&id)).is_none(), "nothing spawned");
}

#[test]
fn bad_calls_are_refused_before_anything_is_touched() {
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let (id, _, handle) = workspace(&home, "rfc");
    let old = uuid::Uuid::new_v4().to_string();
    pin(&id, "grok", &old);

    assert_eq!(call(&Caller::Owner, json!({})).1["error"]["code"], "usage", "a person names the workspace");
    assert_eq!(call(&Caller::Owner, json!({ "workspace": handle, "mode": "all" })).1["error"]["code"], "usage");
    let long = "x".repeat(k2_core::workspace::canonical_swap::NOTES_CHAR_CAP + 1);
    assert_eq!(call(&Caller::Owner, json!({ "workspace": handle, "notes": long })).1["error"]["code"], "notes_too_long");
    assert_eq!(call(&Caller::Owner, json!({ "workspace": "no-such-ws-xyz" })).0, "404 Not Found");
    assert_eq!(call(&Caller::WrongCredential, json!({})).1["error"]["code"], "wrong_credential");
    assert_eq!(call(&Caller::Invalid, json!({})).1["error"]["code"], "invalid_token");
    let member = format!("rfm{}", &uuid::Uuid::new_v4().simple().to_string()[..6]);
    k2_core::connect_users::add_user(&member, "password123").expect("add");
    let (s, v) = call(&Caller::Login { username: member }, json!({ "workspace": handle }));
    assert_eq!((s.as_str(), &v["error"]["code"]), ("403 Forbidden", &json!("admin_only")), "{v}");
    let admin = format!("rfa{}", &uuid::Uuid::new_v4().simple().to_string()[..6]);
    k2_core::connect_users::add_user(&admin, "password123").expect("add");
    k2_core::connect_users::set_role(&admin, Role::Admin).expect("role");
    // The harness's CLI isn't here: refused, untouched.
    let (s, v) = call(&Caller::Login { username: admin }, json!({ "workspace": handle }));
    assert_eq!((s.as_str(), &v["error"]["code"]), ("409 Conflict", &json!(crate::pinned_chat::HARNESS_NOT_INSTALLED)), "{v}");
    assert_eq!(pinned(&id), (Some(old), "grok".to_string()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_premint_refresh_moves_the_pin_and_the_thread_and_hands_over_the_note() {
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let _shim = shims(&home, &["grok"]);
    let (id, path, handle) = workspace(&home, "rfd");
    let old = uuid::Uuid::new_v4().to_string();
    pin(&id, "grok", &old);
    post_thread(&old, &id, "before the refresh");

    // A dry run changes nothing, and with the switch off says a person is
    // needed without keeping a note or handing out a line to ask with.
    let (s, v) = call(&main_session(&id), json!({ "dryRun": true, "notes": "abc" }));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!((&v["dryRun"], &v["refreshed"], &v["noteChars"], &v["needsPerson"]), (&json!(true), &json!(false), &json!(3), &json!(true)), "{v}");
    assert!(!std::path::Path::new(&path).join(".k2").join("handoffs").exists(), "no note kept");
    k2_core::workspace::settings::set_agents_can_refresh_chat(&id, true).expect("on");
    let (s, v) = call(&main_session(&id), json!({ "dryRun": true }));
    assert_eq!((s.as_str(), &v["needsPerson"]), ("200 OK", &json!(false)), "{v}");
    assert_eq!(pinned(&id).0.as_deref(), Some(old.as_str()));

    let (s, v) = call(&main_session(&id), json!({ "notes": "NEXT-CHAT-NOTE" }));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v["refreshed"], true, "{v}");
    assert_eq!(v["harness"], "grok", "{v}");
    assert_eq!(v["fromSessionId"], old.as_str(), "{v}");

    // The identity moved to a new provider id on the same harness.
    let (new, harness) = pinned(&id);
    let new = new.expect("grok premints");
    assert_ne!(new, old);
    assert_eq!(harness, "grok");
    assert_eq!(v["claudeSessionId"], new.as_str(), "{v}");
    assert!(crate::v2_session_map::lookup_by_agent_name(&crate::canonical_session::canonical_key_for(&id)).is_some(), "the new chat is live");

    // The Thread came along: a person's history, on the new key, at the address.
    assert_eq!(thread_texts(&new), ["before the refresh"]);
    assert_eq!(crate::overlay_routes::pinned_chat_of(&id).as_deref(), Some(new.as_str()));
    assert_eq!(crate::overlay_routes::conversation_for_addr(&handle).as_deref(), Some(new.as_str()));
    assert!(k2_core::workspace::resume_chat::held_thread(&id).is_none(), "nothing waits");

    // The note is in the handoff, which is private to the workspace.
    let handoff = v["handoffPath"].as_str().expect("handoff");
    let text = std::fs::read_to_string(handoff).expect("handoff file");
    assert!(handoff.starts_with(&path), "{handoff}");
    assert!(text.contains("NEXT-CHAT-NOTE"), "{text}");
    assert!(text.contains(&old), "it names the old chat: {text}");

    end_chat(&id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_self_minting_refresh_holds_the_thread_and_a_second_one_waits() {
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let _shim = shims(&home, &["codex"]);
    let (id, _, handle) = workspace(&home, "rfe");
    let old = uuid::Uuid::new_v4().to_string();
    pin(&id, "codex", &old);
    post_thread(&old, &id, "before the refresh");
    k2_core::workspace::settings::set_agents_can_refresh_chat(&id, true).expect("on");

    let (s, v) = call(&main_session(&id), json!({}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v["pendingSessionDiscovery"], true, "{v}");

    // Codex names its own chat later: no id yet, and the address still works.
    assert_eq!(pinned(&id).0, None);
    assert_eq!(k2_core::workspace::resume_chat::held_thread(&id).as_deref(), Some(old.as_str()));
    assert_eq!(crate::overlay_routes::conversation_for_addr(&handle).as_deref(), Some(old.as_str()));
    assert_eq!(thread_texts(&old), ["before the refresh"]);

    // A second refresh now would have nothing to summarize: refused, untouched.
    let live = crate::v2_session_map::lookup_by_agent_name(&crate::canonical_session::canonical_key_for(&id)).expect("live");
    let (s, v) = call(&main_session(&id), json!({}));
    assert_eq!((s.as_str(), &v["error"]["code"]), ("409 Conflict", &json!(SETTLING)), "{v}");
    let after = crate::v2_session_map::lookup_by_agent_name(&crate::canonical_session::canonical_key_for(&id)).expect("live");
    assert_eq!(after.session_id, live.session_id, "the new chat wasn't restarted");

    end_chat(&id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn swap_canonical_on_the_same_harness_is_a_refresh_for_both_kinds_of_caller() {
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let _shim = shims(&home, &["grok", "claude"]);
    let (id, path, _) = workspace(&home, "rff");
    let old = uuid::Uuid::new_v4().to_string();
    pin(&id, "grok", &old);
    post_thread(&old, &id, "before");
    let swap = |caller: &Caller, provider: &str| {
        let r = crate::pinned_chat::handle_swap_canonical(json!({ "project": path, "provider": provider }).to_string().as_bytes(), caller, "test");
        (r.status.to_string(), serde_json::from_str::<Value>(&r.body).expect("json"))
    };

    // An agent: the same harness asks for the refresh, another for the move.
    let (s, v) = swap(&main_session(&id), "grok");
    assert_eq!((s.as_str(), &v["error"]["code"]), ("403 Forbidden", &json!(NEEDS_PERSON)), "{v}");
    let (s, v) = swap(&main_session(&id), "claude");
    assert_eq!((s.as_str(), &v["error"]["code"]), ("403 Forbidden", &json!(crate::agent_harness::NEEDS_PERSON)), "{v}");

    // The switch lets its own main session through, on the same harness only.
    k2_core::workspace::settings::set_agents_can_refresh_chat(&id, true).expect("on");
    let (s, v) = swap(&main_session(&id), "claude");
    assert_eq!((s.as_str(), &v["error"]["code"]), ("403 Forbidden", &json!(crate::agent_harness::NEEDS_PERSON)), "{v}");
    let (s, v) = swap(&main_session(&id), "grok");
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!((&v["swapped"], &v["refreshed"]), (&json!(true), &json!(true)), "{v}");
    let first = pinned(&id).0.expect("premint");
    assert_eq!(thread_texts(&first), ["before"], "the Thread came along");

    // A person's swap on the same harness keeps the Thread too; a move doesn't.
    let (s, v) = swap(&Caller::Owner, "grok");
    assert_eq!((s.as_str(), &v["refreshed"]), ("200 OK", &json!(true)), "{v}");
    let second = pinned(&id).0.expect("premint");
    assert_ne!(second, first);
    assert_eq!(thread_texts(&second), ["before"]);
    let (s, v) = swap(&Caller::Owner, "claude");
    assert_eq!((s.as_str(), &v["refreshed"]), ("200 OK", &json!(false)), "{v}");
    assert_eq!(thread_texts(&second), ["before"], "another harness starts its own Thread");

    end_chat(&id);
}
