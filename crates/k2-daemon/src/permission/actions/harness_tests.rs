//! `agent.harness-switch` end to end: the agent is refused at the route
//! and asks; a Member can't answer; the owner taps Allow and the card runs
//! the route as the owner (the pinned chat moves, the agent hears it once);
//! asking again is `already_done`. Agent CLIs are `exec cat` shims.

use k2_core::connect_users::Role;
use k2_core::db::schema::WorkspaceSession;
use k2_core::permission::catalog::{Floor, Grant, Proof, Undo};
use serde_json::{json, Value};

use super::super::held::tests::{answer_as, ask, ask_ok, below_floor, card_of, last_turn, login, view, Ws};
use crate::sidecar_routes::{AgentCaller, Caller};

/// A workspace with a real folder, its chat pinned to grok.
fn seed_dir(home: &crate::test_support::TempHome, tag: &str) -> Ws {
    let handle = format!("{tag}{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let dir = home.path().join(&handle);
    std::fs::create_dir_all(&dir).expect("workspace dir");
    let id = uuid::Uuid::new_v4().to_string();
    let conv = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
        rusqlite::params![id, handle, dir.to_string_lossy()],
    )
    .expect("seed project");
    WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &id, None, Some(&conv), "grok", "system", "running").expect("pin");
    Ws { id, handle, conv }
}

fn harness_of(ws: &Ws) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    WorkspaceSession::get(&conn, &ws.id).expect("read").expect("row").harness
}

fn live(ws: &Ws) -> bool {
    crate::v2_session_map::lookup_by_agent_name(&crate::canonical_session::canonical_key_for(&ws.id)).is_some()
}

#[test]
fn the_action_is_registered_with_the_route_floor_and_its_walls() {
    let r = super::super::find(crate::agent_harness::CARD_ACTION).expect("registered");
    k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((r.spec.floor, r.spec.proof, r.spec.grant), (Floor::Admin, Proof::Click, Grant::OneTime));
    assert!(matches!(r.spec.undo, Undo::None));
    assert_eq!(r.spec.params[0].choices, &k2_core::workspace::canonical_swap::KNOWN_HARNESSES);
    use crate::routes::route_policy::Floor as RouteFloor;
    assert_eq!(
        crate::routes::route_policy::lookup(crate::agent_harness::ROUTE).and_then(|x| x.post),
        Some(RouteFloor::Admin),
        "the card's floor is the route's floor"
    );
    let prefixes: Vec<&str> = r.spec.walls.iter().map(|w| w.route_prefix).collect();
    assert_eq!(prefixes, [crate::agent_harness::ROUTE, "/cli/workspace/swap-canonical"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agent_asks_a_member_cant_answer_and_the_owner_allow_moves_the_pinned_chat() {
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let shim_dir = home.path().join("agent-shims");
    std::fs::create_dir_all(&shim_dir).unwrap();
    k2_core::test_env::write_executable(&shim_dir.join("claude"), "#!/bin/sh\nexec cat\n");
    let _shim = k2_core::test_env::EnvVar::set("K2_TEST_AGENT_SHIM_DIR", &shim_dir);
    let ws = seed_dir(&home, "hsw");

    // The agent itself is refused at the route: held, not switched.
    let me = Caller::Agent(AgentCaller { project_id: ws.id.clone(), map_key: None, args: Vec::new(), sandboxed: false });
    let r = crate::agent_harness::handle(json!({ "workspace": ws.handle, "harness": "claude" }).to_string().as_bytes(), &me, "test");
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    assert!(r.body.contains(crate::agent_harness::NEEDS_PERSON), "{}", r.body);
    assert_eq!(harness_of(&ws), "grok");
    assert!(!live(&ws));

    // It asks; the card says what happens.
    let v = ask_ok(&ws, "agent.harness-switch", json!({ "harness": "claude" }));
    let card = card_of(&v);
    let vw = view(&ws, &card);
    assert_eq!(vw["setting"], "Pinned chat on claude", "{vw}");
    assert!(vw["whatChanges"].as_str().unwrap_or("").contains("pinned chat to claude"), "{vw}");
    assert!(vw["consequences"].as_str().unwrap_or("").contains("The old chat stays in Chat history"), "{vw}");

    // A Member is below the floor; nothing moves.
    let member = login("hswm", Role::Member);
    below_floor(answer_as(&member, &ws, &card, json!({ "answer": "Allow" })), "admin");
    assert_eq!(harness_of(&ws), "grok");
    assert!(!live(&ws));

    // The owner taps Allow: the card runs the route as the owner.
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "approved", "{a}");
    assert_eq!(harness_of(&ws), "claude", "the pinned row moved");
    assert!(live(&ws), "the new pinned chat is live");
    let turn = last_turn(&ws);
    assert!(turn.contains("approved by owner") && turn.contains("runs on claude now"), "{turn}");

    // Asking again: already done, no new card.
    let (s, v): (String, Value) = ask(&ws, "agent.harness-switch", json!({ "harness": "claude" }));
    assert_eq!((s.as_str(), &v["status"]), ("200 OK", &json!("already_done")), "{v}");
    // A harness K2 doesn't know is not a choice.
    let (s, v) = ask(&ws, "agent.harness-switch", json!({ "harness": "aider" }));
    assert!(s.starts_with("400"), "{s}: {v}");

    crate::v2_session_map::unregister(&crate::canonical_session::canonical_key_for(&ws.id));
}
