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
