//! Who may change focus groups: a person's changes go through; an agent's
//! passport is refused `focus_group_needs_person` with the values the card
//! needs, and nothing changes; a bad credential keeps today's answer. The
//! list rows (`/cli/agent/list`, `k2 workspace list --json`) name each
//! workspace's group.

use serde_json::{json, Value};

use super::*;
use crate::sidecar_routes::{AgentCaller, Caller};

fn agent(project_id: &str) -> Caller {
    Caller::Agent(AgentCaller { project_id: project_id.to_string(), map_key: None, args: Vec::new(), sandboxed: false })
}

fn seed_workspace(tag: &str) -> (String, String) {
    let handle = format!("{tag}{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
        rusqlite::params![id, handle, format!("/tmp/fg-{handle}")],
    )
    .expect("seed project");
    (id, handle)
}

fn groups() -> Vec<(String, String)> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::db::schema::FocusGroup::list(&conn).expect("list").into_iter().map(|g| (g.id, g.name)).collect()
}

fn group_of(project_id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT focus_group_id FROM projects WHERE id = ?1", rusqlite::params![project_id], |r| r.get(0)).expect("row")
}

fn body(r: &CliResponse) -> Value {
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{e}: {}", r.body))
}

fn post(path: &str, b: Value, caller: &Caller) -> CliResponse {
    handle_as(path, b.to_string().as_bytes(), caller)
}

#[test]
fn a_person_creates_renames_and_assigns() {
    let _db = k2_core::db::scoped_for_test();
    let (ws, _) = seed_workspace("fgp");
    for caller in [Caller::Owner, Caller::Login { username: "gina".into() }, Caller::Shell { project_id: ws.clone() }] {
        let name = format!("Grp-{}", &uuid::Uuid::new_v4().simple().to_string()[..6]);
        let r = post(CREATE, json!({ "name": name }), &caller);
        assert_eq!(r.status, "200 OK", "{caller:?}: {}", r.body);
        let id = body(&r)["id"].as_str().expect("created id").to_string();
        assert!(groups().contains(&(id.clone(), name.clone())), "{caller:?}: the group exists");

        let to = format!("{name}-2");
        let r = post(UPDATE, json!({ "id": id, "name": to }), &caller);
        assert_eq!(r.status, "200 OK", "{caller:?}: {}", r.body);
        assert!(groups().contains(&(id.clone(), to.clone())), "{caller:?}: renamed");

        let r = post(ASSIGN, json!({ "projectId": ws, "focusGroupId": id }), &caller);
        assert_eq!(r.status, "200 OK", "{caller:?}: {}", r.body);
        assert_eq!(group_of(&ws).as_deref(), Some(id.as_str()), "{caller:?}: assigned");
        let r = post(ASSIGN, json!({ "projectId": ws, "focusGroupId": null }), &caller);
        assert_eq!(r.status, "200 OK", "{caller:?}: {}", r.body);
        assert_eq!(group_of(&ws), None, "{caller:?}: out of every group");
    }
}

#[test]
fn an_agent_is_refused_with_the_values_and_nothing_changes() {
    let _db = k2_core::db::scoped_for_test();
    let (me, _) = seed_workspace("fgme");
    let (other, other_handle) = seed_workspace("fgot");
    let made = k2_core::db_ops::focus_groups_create("Infra", None).expect("a group");
    let before = groups();
    let me = agent(&me);

    let r = post(CREATE, json!({ "name": "Client work" }), &me);
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    let v = body(&r);
    assert_eq!((v["error"]["code"].as_str(), v["error"]["name"].as_str()), (Some(NEEDS_PERSON), Some("Client work")), "{v}");
    assert!(v["error"]["hint"].as_str().unwrap_or("").contains("a person's step"), "{v}");

    let r = post(UPDATE, json!({ "id": made.id, "name": "Ops" }), &me);
    let v = body(&r);
    assert_eq!((r.status, v["error"]["group"].as_str(), v["error"]["to"].as_str()), ("403 Forbidden", Some("Infra"), Some("Ops")), "{v}");

    // A colour change names the group but no new name: no card teaches it.
    let r = post(UPDATE, json!({ "id": made.id, "color": "#ff0000" }), &me);
    let v = body(&r);
    assert_eq!((r.status, v["error"]["code"].as_str()), ("403 Forbidden", Some(NEEDS_PERSON)), "{v}");
    assert!(v["error"].get("to").is_none(), "{v}");

    let r = post(ASSIGN, json!({ "projectId": other, "focusGroupId": made.id }), &me);
    let v = body(&r);
    assert_eq!((v["error"]["target"].as_str(), v["error"]["group"].as_str()), (Some(other_handle.as_str()), Some("Infra")), "{v}");
    let r = post(ASSIGN, json!({ "projectId": other, "focusGroupId": null }), &me);
    assert_eq!(body(&r)["error"]["group"], NONE, "{}", r.body);

    assert_eq!(groups(), before, "an agent's refusal changed no group");
    assert_eq!(group_of(&other), None, "an agent's refusal assigned nothing");
}

#[test]
fn a_bad_credential_keeps_todays_answer() {
    let _db = k2_core::db::scoped_for_test();
    for caller in [Caller::Invalid, Caller::WrongCredential] {
        let r = post(CREATE, json!({ "name": "Nope" }), &caller);
        assert_eq!((r.status, r.body.as_str()), ("403 Forbidden", r#"{"error":"invalid or missing token"}"#), "{caller:?}");
    }
    assert!(groups().iter().all(|(_, n)| n != "Nope"));
}

#[test]
fn only_the_three_changes_are_classified() {
    for p in [CREATE, UPDATE, ASSIGN] {
        assert!(is_change(p), "{p}");
        assert!(crate::session_token::is_agent_verb(p), "{p}: the passport reaches the TCP arm");
    }
    for p in ["/cli/focus-groups/delete", "/cli/focus-groups/reconcile", "/cli/focus-groups/list"] {
        assert!(!is_change(p), "{p}");
        assert!(!crate::session_token::is_agent_verb(p), "{p} stays off the agent verbs");
    }
}

#[test]
fn a_group_is_found_by_id_name_or_a_unique_start() {
    let _db = k2_core::db::scoped_for_test();
    let infra = k2_core::db_ops::focus_groups_create("Infra", None).expect("infra");
    k2_core::db_ops::focus_groups_create("Client A", None).expect("a");
    k2_core::db_ops::focus_groups_create("Client B", None).expect("b");
    assert_eq!(resolve_group(&infra.id).expect("by id").name, "Infra");
    assert_eq!(resolve_group("infra").expect("by name, any case").id, infra.id);
    assert_eq!(resolve_group("Inf").expect("a unique start").id, infra.id);
    assert!(resolve_group("Client").expect_err("two starts").contains("Client A, Client B"));
    assert!(resolve_group("Nope").expect_err("none").contains("no focus group matches 'Nope'"));
}

#[test]
fn the_list_rows_name_each_workspaces_group() {
    let _db = k2_core::db::scoped_for_test();
    let (inside, _) = seed_workspace("fgin");
    let (outside, _) = seed_workspace("fgout");
    let g = k2_core::db_ops::focus_groups_create("Infra", None).expect("a group");
    k2_core::db_ops::focus_groups_assign_project(&inside, Some(&g.id)).expect("assign");
    let r = crate::agents_routes::handle_agent_list();
    let v = body(&r);
    let row = |id: &str| v["agents"].as_array().expect("rows").iter().find(|a| a["id"] == id).cloned().expect("the row");
    assert_eq!(row(&inside)["focusGroup"], "Infra", "{v}");
    let out = row(&outside);
    assert!(out.get("focusGroup").is_some_and(Value::is_null), "in no group: null, never missing: {out}");
}
