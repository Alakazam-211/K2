//! `k2 sessions main` and `k2 sessions main set`: an agent reads, a person
//! chooses, and a chat K2 can't find on disk is refused rather than pinned.

use crate::sidecar_routes::{AgentCaller, Caller};
use crate::workspace_sessions_routes::{handle, is_post_route, is_route, MAIN, MAIN_SET};
use k2_core::db::schema::WorkspaceSession;
use serde_json::{json, Value};
use std::collections::HashMap;

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

fn write_transcript(home: &std::path::Path, project: &str, sid: &str) -> std::path::PathBuf {
    let dir = home.join(".claude/projects").join(k2_core::chat_history::claude_project_hash(project));
    std::fs::create_dir_all(&dir).expect("transcript dir");
    let file = dir.join(format!("{sid}.jsonl"));
    std::fs::write(
        &file,
        json!({"type":"user","cwd":project,"message":{"role":"user","content":"the old chat"}}).to_string() + "\n",
    )
    .expect("transcript");
    file
}

fn pin(id: &str, session: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO workspace_sessions (id, project_id, session_id, harness, owner, status, created_at) \
         VALUES (?1, ?2, ?3, 'claude', 'user', 'running', unixepoch()) \
         ON CONFLICT(project_id) DO UPDATE SET session_id = ?3",
        rusqlite::params![uuid::Uuid::new_v4().to_string(), id, session],
    )
    .expect("pin");
}

fn pinned(id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    WorkspaceSession::get(&conn, id).expect("read").and_then(|r| r.session_id)
}

fn call(caller: &Caller, path: &str, handle_name: &str, body: Value) -> (String, Value) {
    let params: HashMap<String, String> = HashMap::from([("workspace".to_string(), handle_name.to_string())]);
    let mut body = body;
    body["workspace"] = json!(handle_name);
    let r = handle(path, clone(caller), body.to_string().as_bytes(), &params, "test");
    (r.status.to_string(), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{e}: {}", r.body)))
}

fn clone(c: &Caller) -> Caller {
    match c {
        Caller::Owner => Caller::Owner,
        Caller::Login { username } => Caller::Login { username: username.clone() },
        Caller::Shell { project_id } => Caller::Shell { project_id: project_id.clone() },
        Caller::Agent(a) => Caller::Agent(AgentCaller {
            project_id: a.project_id.clone(),
            map_key: a.map_key.clone(),
            args: a.args.clone(),
            sandboxed: a.sandboxed,
        }),
        _ => panic!("not used here"),
    }
}

fn main_session(id: &str) -> Caller {
    Caller::Agent(AgentCaller { project_id: id.to_string(), map_key: Some(id.to_string()), args: Vec::new(), sandboxed: false })
}

#[test]
fn the_set_route_is_post_only() {
    assert!(is_route(MAIN) && is_route(MAIN_SET));
    assert!(is_post_route(MAIN_SET), "the dispatcher sends anything else a 405");
    assert!(!is_post_route(MAIN));
}

#[test]
fn an_agent_reads_the_main_chat_but_may_not_choose_it() {
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let (id, path, handle) = workspace(&home, "ma");
    let real = uuid::Uuid::new_v4().to_string();
    write_transcript(home.path(), &path, &real);
    pin(&id, &real);

    let (s, v) = call(&main_session(&id), MAIN, &handle, json!({}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v["main"]["state"], "found", "{v}");
    assert_eq!(v["main"]["conversationId"], json!(real), "{v}");

    let other = uuid::Uuid::new_v4().to_string();
    write_transcript(home.path(), &path, &other);
    let (s, v) = call(&main_session(&id), MAIN_SET, &handle, json!({"id": other}));
    assert_eq!(s, "403 Forbidden", "{v}");
    assert_eq!(v["error"]["code"], "main_chat_needs_person", "{v}");
    assert_eq!(pinned(&id).as_deref(), Some(real.as_str()), "nothing moved");
}

#[test]
fn a_person_chooses_a_chat_on_disk_and_is_refused_a_gone_one() {
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let (id, path, handle) = workspace(&home, "mb");
    let gone = uuid::Uuid::new_v4().to_string();
    pin(&id, &gone);
    // Another chat of this workspace is on disk, so the pin was a real one.
    write_transcript(home.path(), &path, &uuid::Uuid::new_v4().to_string());

    // The pinned chat is gone: K2 says so instead of picking another.
    let (_, v) = call(&Caller::Owner, MAIN, &handle, json!({}));
    assert_eq!(v["main"]["state"], "missing", "{v}");
    assert_eq!(pinned(&id).as_deref(), Some(gone.as_str()), "the record is kept");

    // A chat that is on disk: the owner pins it.
    let real = uuid::Uuid::new_v4().to_string();
    write_transcript(home.path(), &path, &real);
    let (s, v) = call(&Caller::Owner, MAIN_SET, &handle, json!({"id": &real[..8]}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v["main"]["conversationId"], json!(real), "{v}");
    assert_eq!(v["previous"], json!(gone), "{v}");
    assert_eq!(v["restartWith"], json!(format!("k2 agent restart {handle}")), "{v}");
    assert_eq!(pinned(&id).as_deref(), Some(real.as_str()));

    // A shell tab is a person too.
    let again = uuid::Uuid::new_v4().to_string();
    write_transcript(home.path(), &path, &again);
    let (s, v) = call(&Caller::Shell { project_id: id.clone() }, MAIN_SET, &handle, json!({"id": again}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(pinned(&id).as_deref(), Some(again.as_str()));

    // An id nothing knows is refused and nothing moves.
    let (s, v) = call(&Caller::Owner, MAIN_SET, &handle, json!({"id": uuid::Uuid::new_v4().to_string()}));
    assert_eq!(s, "404 Not Found", "{v}");
    assert_eq!(v["error"]["code"], "conversation_not_found", "{v}");
    assert_eq!(pinned(&id).as_deref(), Some(again.as_str()));
}

// ── which Thread the main chat shows after a sidecar becomes the main chat ──

/// A workspace whose main chat `main` has a Thread, and a sidecar
/// `<ws>/reviewer` (conversation `side`) with a Thread of its own.
struct Promote {
    _home: crate::test_support::TempHome,
    id: String,
    path: String,
    handle: String,
    main: String,
    side: String,
}

fn promote_fixture(tag: &str) -> Promote {
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let (id, path, handle) = workspace(&home, tag);
    let main = uuid::Uuid::new_v4().to_string();
    let side = uuid::Uuid::new_v4().to_string();
    write_transcript(home.path(), &path, &main);
    write_transcript(home.path(), &path, &side);
    pin(&id, &main);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::sidecar::bind_name(&conn, &id, "claude", &side, "reviewer", "reviewer").expect("name the sidecar");
        k2_core::overlay::post_thread(&conn, &main, &id, "owner", &handle, "on the main Thread", "compose").expect("post main");
        k2_core::overlay::post_thread(&conn, &side, &id, "owner", &format!("{handle}/reviewer"), "on the sidecar Thread", "compose")
            .expect("post sidecar");
    }
    Promote { _home: home, id, path, handle, main, side }
}

/// The conversation an address reaches now, and its Thread's texts.
fn thread_at(addr: &str) -> (String, Vec<String>) {
    let conv = crate::overlay_routes::conversation_for_addr(addr).unwrap_or_else(|| panic!("{addr} reaches no Thread"));
    (conv.clone(), texts(&conv))
}

fn texts(conv: &str) -> Vec<String> {
    k2_core::overlay::read_thread_page(conv, None, None, 50)
        .expect("read")
        .items
        .into_iter()
        .filter(|i| i.collection == "thread")
        .map(|i| i.doc.body.expect("a text post"))
        .collect()
}

#[test]
fn keep_main_keeps_the_main_thread_and_gives_the_sidecar_thread_an_address() {
    let p = promote_fixture("km");
    let (s, v) = call(&Caller::Owner, MAIN_SET, &p.handle, json!({"id": p.side, "thread": "keep_main"}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(pinned(&p.id).as_deref(), Some(p.side.as_str()));
    assert_eq!(v["thread"]["choice"], "keep_main", "{v}");

    // The workspace's address reads the main chat's Thread, now under the new pin.
    assert_eq!(thread_at(&p.handle), (p.side.clone(), vec!["on the main Thread".to_string()]));
    // The sidecar's own Thread is kept at an address of its own.
    let other = v["thread"]["other"]["address"].as_str().unwrap_or_else(|| panic!("no address for the other Thread: {v}"));
    assert!(other.starts_with(&format!("{}/", p.handle)), "{other}");
    let (key, items) = thread_at(other);
    assert_eq!(items, vec!["on the sidecar Thread".to_string()]);
    assert_eq!(v["thread"]["other"]["conversationId"], json!(key), "{v}");
    assert_ne!(key, p.side);
    assert_ne!(key, p.main);
    // Nothing is left under the old main conversation.
    let db = k2_core::db::shared();
    let conn = db.lock();
    assert_eq!(k2_core::overlay::catalog::get(&conn, &p.main).expect("catalog"), None);
}

#[test]
fn adopt_sidecar_shows_the_sidecar_thread_and_keeps_the_old_main_thread_reachable() {
    let p = promote_fixture("as");
    let (s, v) = call(&Caller::Owner, MAIN_SET, &p.handle, json!({"id": p.side, "thread": "adopt_sidecar"}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(pinned(&p.id).as_deref(), Some(p.side.as_str()));
    assert_eq!(v["thread"]["choice"], "adopt_sidecar", "{v}");

    assert_eq!(thread_at(&p.handle), (p.side.clone(), vec!["on the sidecar Thread".to_string()]));
    // The old main Thread stays with the old main conversation, at an address.
    let other = v["thread"]["other"]["address"].as_str().unwrap_or_else(|| panic!("no address for the old main Thread: {v}"));
    assert_eq!(thread_at(other), (p.main.clone(), vec!["on the main Thread".to_string()]));
    assert_eq!(thread_at(&p.main), (p.main.clone(), vec!["on the main Thread".to_string()]), "by its id too");
    assert_eq!(v["thread"]["other"]["conversationId"], json!(p.main), "{v}");
}

#[test]
fn without_a_thread_choice_only_the_pin_moves() {
    // Older clients send no `thread`: the pin moves and nothing else, as
    // before the choice existed.
    let p = promote_fixture("np");
    let (s, v) = call(&Caller::Owner, MAIN_SET, &p.handle, json!({"id": p.side}));
    assert_eq!(s, "200 OK", "{v}");
    assert!(v.get("thread").is_none(), "{v}");
    assert_eq!(pinned(&p.id).as_deref(), Some(p.side.as_str()));
    assert_eq!(texts(&p.main), vec!["on the main Thread".to_string()]);
    assert_eq!(texts(&p.side), vec!["on the sidecar Thread".to_string()]);
    // So the workspace's address follows the pin to the sidecar's Thread,
    // and the old main Thread has no address of its own.
    assert_eq!(thread_at(&p.handle), (p.side.clone(), vec!["on the sidecar Thread".to_string()]));
    let db = k2_core::db::shared();
    let conn = db.lock();
    assert_eq!(k2_core::workspace_session_handles::current_address_for(&conn, &p.id, &p.main).expect("address"), None);
}

#[test]
fn an_unknown_thread_choice_is_refused_and_nothing_moves() {
    let p = promote_fixture("bt");
    let (s, v) = call(&Caller::Owner, MAIN_SET, &p.handle, json!({"id": p.side, "thread": "merge"}));
    assert_eq!(s, "400 Bad Request", "{v}");
    assert_eq!(v["error"]["code"], "bad_request", "{v}");
    assert_eq!(pinned(&p.id).as_deref(), Some(p.main.as_str()));
    assert_eq!(thread_at(&p.handle), (p.main.clone(), vec!["on the main Thread".to_string()]));
}

#[test]
fn the_chat_menu_pick_takes_the_same_choice() {
    // The desktop chat menu pins through `workspace/set-chat-session`.
    for (choice, main_text, other_text, other_key_is_main) in [
        ("keep_main", "on the main Thread", "on the sidecar Thread", false),
        ("adopt_sidecar", "on the sidecar Thread", "on the main Thread", true),
    ] {
        let p = promote_fixture("cm");
        let params: HashMap<String, String> = HashMap::from([
            ("project".to_string(), p.path.clone()),
            ("session_id".to_string(), p.side.clone()),
            ("provider".to_string(), "claude".to_string()),
            ("thread".to_string(), choice.to_string()),
        ]);
        let r = crate::workspace_routes::dispatch("/cli/workspace/set-chat-session", &params).expect("routed");
        assert_eq!(r.status, "200 OK", "{choice}: {}", r.body);
        let v: Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["thread"]["choice"], choice, "{v}");
        assert_eq!(pinned(&p.id).as_deref(), Some(p.side.as_str()));
        assert_eq!(thread_at(&p.handle), (p.side.clone(), vec![main_text.to_string()]), "{choice}");
        let other = v["thread"]["other"]["address"].as_str().unwrap_or_else(|| panic!("{choice}: {v}"));
        let (key, items) = thread_at(other);
        assert_eq!(items, vec![other_text.to_string()], "{choice}");
        assert_eq!(key == p.main, other_key_is_main, "{choice}");
    }

    // A bad choice is a 400 and changes nothing.
    let p = promote_fixture("cb");
    let params: HashMap<String, String> = HashMap::from([
        ("project".to_string(), p.path.clone()),
        ("session_id".to_string(), p.side.clone()),
        ("thread".to_string(), "both".to_string()),
    ]);
    let r = crate::workspace_routes::dispatch("/cli/workspace/set-chat-session", &params).expect("routed");
    assert_eq!(r.status, "400 Bad Request", "{}", r.body);
    assert_eq!(pinned(&p.id).as_deref(), Some(p.main.as_str()));
}

#[test]
fn keep_main_with_no_thread_on_the_other_chat_just_follows_the_pin() {
    // A past chat with no Thread of its own: keep_main brings the main
    // Thread along, and there is no other Thread to give an address.
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let (id, path, handle) = workspace(&home, "kn");
    let main = uuid::Uuid::new_v4().to_string();
    let past = uuid::Uuid::new_v4().to_string();
    write_transcript(home.path(), &path, &main);
    write_transcript(home.path(), &path, &past);
    pin(&id, &main);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::overlay::post_thread(&conn, &main, &id, "owner", &handle, "on the main Thread", "compose").expect("post");
    }
    let (s, v) = call(&Caller::Owner, MAIN_SET, &handle, json!({"id": past, "thread": "keep_main"}));
    assert_eq!(s, "200 OK", "{v}");
    assert!(v["thread"]["other"].is_null(), "{v}");
    assert_eq!(thread_at(&handle), (past.clone(), vec!["on the main Thread".to_string()]));
}
