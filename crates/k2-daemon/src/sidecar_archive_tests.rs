//! `POST /cli/sidecar/archive`: who may, what's refused, and what stays. No
//! process runs here: rows are seeded, and a stopped sidecar is one with no
//! live session (a live one is covered by `tests/cli/sidecar_cli.sh`).

use super::*;

struct Seeded {
    id: String,
    handle: String,
}

fn workspace(tag: &str) -> Seeded {
    let handle = format!("{tag}{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO projects (id, name, path, handle, agents_can_manage_agents) VALUES (?1, ?2, ?3, ?2, 1)",
        rusqlite::params![id, handle, format!("/tmp/sc-arc-{handle}")],
    )
    .expect("seed project");
    Seeded { id, handle }
}

/// A stopped sidecar's tab row, named `name`.
fn stopped(ws: &Seeded, name: &str, created_by: &str) -> String {
    let sid = uuid::Uuid::new_v4().to_string();
    let pg = format!("pg-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO workspace_tab_sessions (project_id, pane_group_id, agent_name, session_id, command, args_json, cwd, last_seen_at, created_by) \
         VALUES (?1, ?2, ?3, ?4, 'claude', '[]', '/tmp', 1, ?5)",
        rusqlite::params![ws.id, pg, format!("tab-{pg}"), sid, created_by],
    )
    .expect("seed row");
    core::bind_name(&conn, &ws.id, "claude", &sid, name, name).expect("bind name");
    sid
}

fn listed(ws: &Seeded) -> Vec<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    core::list_rows(&conn, &ws.id).expect("list").into_iter().filter_map(|r| r.handle).collect()
}

fn main_agent(ws: &Seeded) -> Caller {
    Caller::Agent(AgentCaller { project_id: ws.id.clone(), map_key: Some(ws.id.clone()), args: Vec::new(), sandboxed: false })
}

fn archive(caller: &Caller, ws: &Seeded, body: serde_json::Value) -> Result<serde_json::Value, SidecarError> {
    let mut b = body;
    b["workspace"] = serde_json::json!(ws.handle);
    handle_archive(caller, b.to_string().as_bytes()).map(|(v, _)| v)
}

#[test]
fn archive_takes_a_stopped_sidecar_off_the_list_and_keeps_its_name_with_the_conversation() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let ws = workspace("arca");
    let own = format!("agent:{}", ws.handle);
    let sid = stopped(&ws, "helper", &own);
    stopped(&ws, "other", &own);

    let v = archive(&main_agent(&ws), &ws, serde_json::json!({ "name": "helper" })).expect("archived");
    assert_eq!(v["archived"], true, "{v}");
    assert_eq!(v["address"], format!("{}/helper", ws.handle), "{v}");
    assert_eq!(listed(&ws), ["other"]);

    // The name stays with the conversation: `k2 sidecar new` resumes it.
    let db = k2_core::db::shared();
    let conn = db.lock();
    assert_eq!(k2_core::workspace_session_handles::resolve_handle(&conn, &ws.id, "helper").expect("named"), sid);
}

#[test]
fn all_stopped_takes_what_it_may_and_says_why_it_left_the_rest() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let ws = workspace("arcb");
    let own = format!("agent:{}", ws.handle);
    stopped(&ws, "mine-one", &own);
    stopped(&ws, "mine-two", &own);
    stopped(&ws, "the-owners", "owner");

    let v = archive(&main_agent(&ws), &ws, serde_json::json!({ "allStopped": true })).expect("archived");
    assert_eq!(v["count"], 2, "{v}");
    let skipped = v["skipped"].as_array().expect("skipped");
    assert_eq!(skipped.len(), 1, "{v}");
    assert_eq!(skipped[0]["address"], format!("{}/the-owners", ws.handle), "{v}");
    assert_eq!(skipped[0]["code"], "forbidden", "{v}");
    assert_eq!(listed(&ws), ["the-owners"]);

    // A person takes it.
    let v = archive(&Caller::Owner, &ws, serde_json::json!({ "allStopped": true })).expect("archived");
    assert_eq!(v["count"], 1, "{v}");
    assert!(listed(&ws).is_empty());
}

#[test]
fn archive_refuses_what_isnt_its_to_clear_and_leaves_it() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let ws = workspace("arcc");
    let other = workspace("arcd");
    let own = format!("agent:{}", ws.handle);
    stopped(&ws, "helper", &own);
    stopped(&ws, "the-owners", "owner");
    stopped(&other, "theirs", &format!("agent:{}", other.handle));

    let code = |r: Result<serde_json::Value, SidecarError>| r.map(|v| v.to_string()).map_err(|e| e.code).expect_err("refused");
    let agent = main_agent(&ws);

    // Name one, or all.
    assert_eq!(code(archive(&agent, &ws, serde_json::json!({}))), "bad_request");
    assert_eq!(code(archive(&agent, &ws, serde_json::json!({ "name": "helper", "allStopped": true }))), "bad_request");
    assert_eq!(code(archive(&agent, &ws, serde_json::json!({ "name": "nobody" }))), "no_such_sidecar");
    assert_eq!(code(archive(&agent, &ws, serde_json::json!({ "name": ws.handle }))), "forbidden", "the main session");

    // Sidecars an agent didn't open are its person's.
    assert_eq!(code(archive(&agent, &ws, serde_json::json!({ "name": "the-owners" }))), "forbidden");

    // A sidecar can't, and neither can another workspace's agent.
    let sidecar = Caller::Agent(AgentCaller { project_id: ws.id.clone(), map_key: Some("tab-1".into()), args: Vec::new(), sandboxed: false });
    assert_eq!(code(archive(&sidecar, &ws, serde_json::json!({ "name": "helper" }))), "forbidden");
    let stranger = main_agent(&other);
    assert_eq!(code(archive(&stranger, &ws, serde_json::json!({ "name": "helper" }))), "forbidden");

    // Off the switch, an agent is gated.
    k2_core::workspace::settings::set_agents_can_manage_agents(&ws.id, false).expect("off");
    assert_eq!(code(archive(&agent, &ws, serde_json::json!({ "name": "helper" }))), "gated");
    k2_core::workspace::settings::set_agents_can_manage_agents(&ws.id, true).expect("on");

    // A worktree sidecar is for retire, until it's retired.
    let wt = stopped(&ws, "worktree-one", &own);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "UPDATE workspace_tab_sessions SET worktree_branch = 'k2/worktree-one', worktree_dir = '/tmp/wt' WHERE session_id = ?1",
            rusqlite::params![wt],
        )
        .expect("worktree");
    }
    let e = archive(&Caller::Owner, &ws, serde_json::json!({ "name": "worktree-one" })).map(|_| ()).expect_err("refused");
    assert_eq!(e.code, "worktree_sidecar");
    assert!(e.hint.contains("k2 sidecar retire"), "{}", e.hint);

    let mut left = listed(&ws);
    left.sort();
    assert_eq!(left, ["helper", "the-owners", "worktree-one"], "nothing went");
    assert_eq!(listed(&other), ["theirs"]);
}
