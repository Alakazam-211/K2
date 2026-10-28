//! `heartbeat.pause-other` / `heartbeat.resume-other` end to end: the
//! manager agent is refused at the route and asks; a Member can't answer;
//! an Admin taps Allow and the card runs the route as that Admin (the other
//! workspace's heartbeat pauses, audited with the Admin as the actor, the
//! agent hears it once); asking again is `already_done`; resume undoes it.

use k2_core::connect_users::Role;
use k2_core::permission::catalog::{Floor, Grant, Proof, Undo};
use serde_json::json;

use super::super::held::tests::{answer_as, ask, ask_ok, ask_refused, below_floor, card_of, last_turn, login, seed, view, Ws};
use super::*;
use crate::sidecar_routes::{AgentCaller, Caller};

/// Another workspace on disk with one enabled heartbeat `name`.
fn other_with(home: &crate::test_support::TempHome, tag: &str, name: &str) -> (String, String, String) {
    let handle = format!("{tag}{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let dir = home.path().join(&handle);
    let rel = format!(".k2/heartbeats/{name}/WAKEUP.md");
    std::fs::create_dir_all(dir.join(format!(".k2/heartbeats/{name}"))).unwrap();
    std::fs::write(dir.join(&rel), "Triage the queue.\n").unwrap();
    let path = dir.to_string_lossy().to_string();
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)", rusqlite::params![id, handle, path])
        .expect("insert project");
    k2_core::db::schema::AgentHeartbeat::insert(&conn, &uuid::Uuid::new_v4().to_string(), &id, name, "hourly", "{}", &rel, true)
        .expect("insert heartbeat");
    (id, path, handle)
}

fn enabled(id: &str, name: &str) -> bool {
    crate::heartbeat_other::enabled_of(id, name).expect("the heartbeat exists")
}

#[test]
fn both_actions_are_registered_at_the_admin_floor_with_their_walls() {
    for id in [PAUSE_ACTION, RESUME_ACTION] {
        let r = super::super::find(id).expect("registered");
        k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!((r.spec.floor, r.spec.proof, r.spec.grant), (Floor::Admin, Proof::Click, Grant::OneTime), "{id}");
        assert!(matches!(r.spec.undo, Undo::None), "{id}");
        let keys: Vec<&str> = r.spec.params.iter().map(|p| p.key).collect();
        assert_eq!(keys, ["target", "heartbeat"], "{id}");
        let walls: Vec<(&str, &str)> = r
            .spec
            .walls
            .iter()
            .map(|w| match w.matches {
                k2_core::permission::catalog::WallMatch::Code { code, .. } => (w.route_prefix, code),
                other => panic!("{id}: {other:?}"),
            })
            .collect();
        assert_eq!(walls, [(ROUTE, OTHER_WORKSPACE)], "{id}");
    }
    // The route itself has a POST row (the card's floor is stricter: a
    // Member's own-workspace toggles are unchanged, another workspace is
    // Admin and up at the route too).
    assert!(crate::routes::route_policy::lookup(ROUTE).and_then(|x| x.post).is_some(), "{ROUTE} has a POST row");
}

#[test]
fn the_agent_asks_a_member_cant_answer_and_an_admin_allow_pauses_it_as_the_admin() {
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let name = format!("triage-{}", &uuid::Uuid::new_v4().simple().to_string()[..6]);
    let mgr: Ws = seed("hbcmgr");
    let (comp_id, _comp_path, comp) = other_with(&home, "hbccomp", &name);
    let mgr_path = format!("/tmp/pc-e2-{}", mgr.handle);

    // The agent itself is refused at the route: nothing changes.
    let me = Caller::Agent(AgentCaller { project_id: mgr.id.clone(), map_key: None, args: Vec::new(), sandboxed: false });
    let r = crate::heartbeat_other::handle(Some(&mgr_path), Some(&comp), &name, false, &me, "test");
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    assert!(r.body.contains(OTHER_WORKSPACE), "{}", r.body);
    assert!(enabled(&comp_id, &name));

    // It asks; the card says what happens, in K2's words.
    let v = ask_ok(&mgr, PAUSE_ACTION, json!({ "target": comp, "heartbeat": name }));
    let card = card_of(&v);
    let vw = view(&mgr, &card);
    assert_eq!(vw["whatChanges"], format!("Pause {comp}'s {name} heartbeat."), "{vw}");
    assert!(vw["consequences"].as_str().unwrap_or("").contains("until someone resumes it"), "{vw}");

    // A Member is below the floor; nothing changes.
    let member = login("hbcm", Role::Member);
    below_floor(answer_as(&member, &mgr, &card, json!({ "answer": "Allow" })), "admin");
    assert!(enabled(&comp_id, &name));

    // An Admin taps Allow: the card runs the route as the Admin, once.
    let admin = login("hbca", Role::Admin);
    let (s, a) = answer_as(&admin, &mgr, &card, json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "approved", "{a}");
    assert!(!enabled(&comp_id, &name), "paused");
    let turn = last_turn(&mgr);
    assert!(turn.contains(&format!("{comp}'s {name} heartbeat is paused")), "{turn}");
    let actor: Option<String> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT actor FROM heartbeat_fires WHERE project_id = ?1 AND schedule_name = ?2 AND decision = 'changed' ORDER BY rowid DESC LIMIT 1",
            rusqlite::params![comp_id, name],
            |r| r.get(0),
        )
        .expect("a changed row")
    };
    assert_eq!(actor, Some(format!("user:{admin}")), "the heartbeat's history names the person who allowed it");
    let audit = std::fs::read_to_string(home.path().join(".k2").join("auth-audit.jsonl")).expect("audited");
    assert!(audit.contains(&format!("paused:{comp}/{name}")) && audit.contains(&format!("login:{admin}")), "{audit}");
    assert!(audit.contains("permission-card"), "{audit}");

    // Asking again: already done, no new card.
    let (s, v) = ask(&mgr, PAUSE_ACTION, json!({ "target": comp, "heartbeat": name }));
    assert_eq!((s.as_str(), &v["status"]), ("200 OK", &json!("already_done")), "{v}");

    // Resume is the other card; the owner allows it.
    let v = ask_ok(&mgr, RESUME_ACTION, json!({ "target": comp, "heartbeat": name }));
    let (s, a) = answer_as("owner", &mgr, &card_of(&v), json!({ "answer": "Allow" }));
    assert_eq!((s.as_str(), &a["action"]["result"]["outcome"]), ("200 OK", &json!("approved")), "{a}");
    assert!(enabled(&comp_id, &name), "resumed");
}

#[test]
fn values_that_dont_hold_are_refused_at_the_ask() {
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let name = format!("triage-{}", &uuid::Uuid::new_v4().simple().to_string()[..6]);
    let mgr: Ws = seed("hbvmgr");
    let (_id, _path, comp) = other_with(&home, "hbvcomp", &name);

    let hint = ask_refused(&mgr, PAUSE_ACTION, json!({ "target": comp, "heartbeat": "nope" }));
    assert!(hint.contains(&format!("no heartbeat named 'nope' in workspace {comp}")), "{hint}");
    let hint = ask_refused(&mgr, PAUSE_ACTION, json!({ "target": "no-such-ws-xyz", "heartbeat": name }));
    assert!(hint.contains("workspace not found"), "{hint}");
    let hint = ask_refused(&mgr, PAUSE_ACTION, json!({ "target": mgr.handle, "heartbeat": name }));
    assert!(hint.contains("your own workspace"), "{hint}");
    // Both values are required.
    let (s, v) = ask(&mgr, PAUSE_ACTION, json!({ "target": comp }));
    assert!(s.starts_with("400") && v["error"]["hint"].as_str().unwrap_or("").contains("heartbeat="), "{s}: {v}");
}
