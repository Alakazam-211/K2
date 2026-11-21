//! `focus-groups.create`, `.rename` and `.assign` end to end: the agent is
//! refused at the route and asks; a Member can't answer; the owner or an
//! Admin taps Allow and the card makes exactly the change it names, as
//! them; asking again is `already_done`; the agent's own request is still
//! refused after an approval; an assign card never moves another workspace.

use k2_core::connect_users::Role;
use k2_core::permission::catalog::{Floor, Grant, ParamKind, Proof, Undo, WallMatch};
use serde_json::{json, Value};

use super::super::held::tests::{answer_as, ask, ask_ok, ask_refused, below_floor, card_of, last_turn, login, seed, view};
use super::*;
use crate::sidecar_routes::{AgentCaller, Caller};

fn groups() -> Vec<(String, String)> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::db::schema::FocusGroup::list(&conn).expect("list").into_iter().map(|g| (g.id, g.name)).collect()
}

fn named(name: &str) -> Option<String> {
    groups().into_iter().find(|(_, n)| n == name).map(|(id, _)| id)
}

fn group_of(project_id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT focus_group_id FROM projects WHERE id = ?1", rusqlite::params![project_id], |r| r.get(0)).expect("row")
}

fn as_agent(project_id: &str, path: &str, body: Value) -> (String, Value) {
    let me = Caller::Agent(AgentCaller { project_id: project_id.to_string(), map_key: None, args: Vec::new(), sandboxed: false });
    let r = crate::focus_group_routes::handle_as(path, body.to_string().as_bytes(), &me);
    (r.status.to_string(), serde_json::from_str(&r.body).expect("json"))
}

fn allow(author: &str, ws: &super::super::held::tests::Ws, card: &str) {
    let (s, a) = answer_as(author, ws, card, json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "approved", "{a}");
}

#[test]
fn the_three_cards_are_one_time_clicks_at_the_admin_floor_with_their_walls() {
    let _db = k2_core::db::scoped_for_test();
    for (id, keys, route) in [
        (CREATE_ACTION, vec!["name"], CREATE),
        (RENAME_ACTION, vec!["group", "to"], UPDATE),
        (ASSIGN_ACTION, vec!["target", "group"], ASSIGN),
    ] {
        let r = super::super::find(id).expect("registered");
        k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!((r.spec.floor, r.spec.proof, r.spec.grant), (Floor::Admin, Proof::Click, Grant::OneTime), "{id}");
        assert!(matches!(r.spec.undo, Undo::None), "{id}");
        assert_eq!(r.spec.params.iter().map(|p| p.key).collect::<Vec<_>>(), keys, "{id}");
        let walls: Vec<(&str, WallMatch)> = r.spec.walls.iter().map(|w| (w.route_prefix, w.matches)).collect();
        assert_eq!(walls, [(route, WallMatch::Code { status: 403, code: NEEDS_PERSON })], "{id}");
        // The route's own floor admits every person (a Member login too);
        // the card asks the owner or an admin.
        use crate::routes::route_policy::Floor as RouteFloor;
        assert_eq!(crate::routes::route_policy::lookup(route).and_then(|x| x.post), Some(RouteFloor::Member), "{route}");
    }
    let assign = super::super::find(ASSIGN_ACTION).expect("assign");
    assert_eq!(assign.spec.params[0].kind, ParamKind::Workspace, "the workspace is checked to exist");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_a_member_cant_answer_the_owner_allow_makes_it_once() {
    let _db = k2_core::db::scoped_for_test();
    k2_core::db::init_for_tests();
    let mgr = seed("fgcmgr");
    let name = format!("Client {}", &uuid::Uuid::new_v4().simple().to_string()[..6]);

    // Refused at the route: nothing made.
    let (s, v) = as_agent(&mgr.id, CREATE, json!({ "name": name }));
    assert_eq!((s.as_str(), v["error"]["code"].as_str()), ("403 Forbidden", Some(NEEDS_PERSON)), "{v}");
    assert_eq!(named(&name), None);

    let v = ask_ok(&mgr, CREATE_ACTION, json!({ "name": name }));
    let card = card_of(&v);
    let vw = view(&mgr, &card);
    assert!(vw["whatChanges"].as_str().unwrap_or("").starts_with(&format!("Create the focus group {name}")), "{vw}");

    let member = login("fgcm", Role::Member);
    below_floor(answer_as(&member, &mgr, &card, json!({ "answer": "Allow" })), "admin");
    assert_eq!(named(&name), None, "a Member's answer made nothing");

    allow("owner", &mgr, &card);
    assert!(named(&name).is_some(), "the owner's Allow made it");
    assert!(last_turn(&mgr).contains("approved by owner"), "the agent hears it");

    // Asking again: already done. The agent's own request: still refused.
    let (s, v) = ask(&mgr, CREATE_ACTION, json!({ "name": name.to_uppercase() }));
    assert_eq!((s.as_str(), &v["status"]), ("200 OK", &json!("already_done")), "{v}");
    let (s, _) = as_agent(&mgr.id, CREATE, json!({ "name": "Another" }));
    assert_eq!(s, "403 Forbidden", "an approval is not a standing grant");
    assert_eq!(groups().iter().filter(|(_, n)| n.eq_ignore_ascii_case(&name)).count(), 1, "made once");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rename_an_admin_allow_renames_the_named_group_only() {
    let _db = k2_core::db::scoped_for_test();
    k2_core::db::init_for_tests();
    let mgr = seed("fgrmgr");
    let tag = &uuid::Uuid::new_v4().simple().to_string()[..6];
    let infra = k2_core::db_ops::focus_groups_create(&format!("Infra {tag}"), None).expect("infra");
    let other = k2_core::db_ops::focus_groups_create(&format!("Sales {tag}"), None).expect("sales");
    let to = format!("Ops {tag}");

    let (s, v) = as_agent(&mgr.id, UPDATE, json!({ "id": infra.id, "name": to }));
    assert_eq!((s.as_str(), v["error"]["group"].as_str(), v["error"]["to"].as_str()), ("403 Forbidden", Some(infra.name.as_str()), Some(to.as_str())), "{v}");

    let v = ask_ok(&mgr, RENAME_ACTION, json!({ "group": infra.name, "to": to }));
    let card = card_of(&v);
    let admin = login("fgra", Role::Admin);
    allow(&admin, &mgr, &card);
    assert_eq!(named(&to).as_deref(), Some(infra.id.as_str()), "renamed, same id");
    assert_eq!(named(&other.name).as_deref(), Some(other.id.as_str()), "the other group is untouched");

    let (s, v) = ask(&mgr, RENAME_ACTION, json!({ "group": to, "to": to }));
    assert_eq!((s.as_str(), &v["status"]), ("200 OK", &json!("already_done")), "{v}");
    let hint = ask_refused(&mgr, RENAME_ACTION, json!({ "group": format!("Nope {tag}"), "to": "x" }));
    assert!(hint.contains("no focus group matches"), "{hint}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn assign_moves_only_the_named_workspace_and_none_takes_it_out() {
    let _db = k2_core::db::scoped_for_test();
    k2_core::db::init_for_tests();
    let mgr = seed("fgamgr");
    let a = seed("fgaa");
    let b = seed("fgab");
    let g = k2_core::db_ops::focus_groups_create(&format!("Team {}", &uuid::Uuid::new_v4().simple().to_string()[..6]), None).expect("g");

    let (s, v) = as_agent(&mgr.id, ASSIGN, json!({ "projectId": a.id, "focusGroupId": g.id }));
    assert_eq!((s.as_str(), v["error"]["target"].as_str(), v["error"]["group"].as_str()), ("403 Forbidden", Some(a.handle.as_str()), Some(g.name.as_str())), "{v}");
    assert_eq!(group_of(&a.id), None);

    let v = ask_ok(&mgr, ASSIGN_ACTION, json!({ "target": a.handle, "group": g.name }));
    let card = card_of(&v);
    allow("owner", &mgr, &card);
    assert_eq!(group_of(&a.id).as_deref(), Some(g.id.as_str()), "A is in the group");
    assert_eq!(group_of(&b.id), None, "B never moves on A's card");

    // The agent still can't move B itself.
    let (s, _) = as_agent(&mgr.id, ASSIGN, json!({ "projectId": b.id, "focusGroupId": g.id }));
    assert_eq!(s, "403 Forbidden");
    assert_eq!(group_of(&b.id), None);

    let (s, v) = ask(&mgr, ASSIGN_ACTION, json!({ "target": a.handle, "group": g.id }));
    assert_eq!((s.as_str(), &v["status"]), ("200 OK", &json!("already_done")), "{v}");

    let v = ask_ok(&mgr, ASSIGN_ACTION, json!({ "target": a.handle, "group": "none" }));
    allow("owner", &mgr, &card_of(&v));
    assert_eq!(group_of(&a.id), None, "none takes it out");

    let hint = ask_refused(&mgr, ASSIGN_ACTION, json!({ "target": "no-such-ws-fg", "group": "none" }));
    assert!(hint.contains("workspace not found"), "{hint}");
}
