//! Postal's cards end to end (prd-postal-in-k2-v1 S3, S0): an agent's
//! invite and an incoming request each wait on a card on the agent's pinned
//! Thread; only a person at the floor answers; Allow and Deny run the
//! route's own rules as that person; deciding in the tab closes the card;
//! the p5 link mints a key that can only message its workspace and hands it
//! to p5 on stdin. Test DB, temp HOME; no agent is woken (turns go to
//! `inject`'s sink); p5 is a script.

use k2_core::connect_users::Role;
use k2_core::permission::catalog::{Floor, Grant, Proof, WallCoverage};
use serde_json::{json, Value};

use super::super::held::tests::{answer_as, below_floor, card_of, last_turn, login, row, seed, view, Ws};
use super::*;

fn pws(ws: &Ws) -> crate::postal_routes::Ws {
    crate::postal_routes::ws_of_token(&ws.id).expect("postal ws")
}

fn claim(ws: &Ws) -> String {
    let addr = format!("{}::acme.postal.bot", ws.handle);
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::postal::store::claim(&conn, &ws.id, &k2_core::postal::parse_address(&addr).unwrap(), "owner").expect("claim");
    addr
}

fn friend_status(ws: &Ws, peer: &str) -> Option<String> {
    k2_core::db::read(|conn| {
        let h = k2_core::postal::store::handle_for_workspace(conn, &ws.id)?;
        k2_core::postal::store::friend_by_peer(conn, &h.id, peer).map(|f| f.status)
    })
}

fn agent_who(ws: &Ws) -> Who {
    Who::Agent { project_id: ws.id.clone(), session_id: format!("sess-{}", &ws.id[..8]) }
}

fn post(path: &str, who: Who, body: Value) -> (String, Value) {
    let r = crate::postal_routes::handle(path, who, true, body.to_string().as_bytes(), &std::collections::HashMap::new());
    (r.status.to_string(), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{e}: {}", r.body)))
}

#[test]
fn the_cards_are_registered_k2_presented_one_time_clicks() {
    let _db = k2_core::db::scoped_for_test();
    for (id, floor) in [(INVITE, Floor::Admin), (PAIR_ACCEPT, Floor::Admin), (LINK_P5, Floor::Owner)] {
        let r = super::super::find(id).expect("registered");
        k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!((r.spec.floor, r.spec.proof, r.spec.grant), (floor, Proof::Click, Grant::OneTime), "{id}");
        assert!(r.spec.walls.is_empty() && r.spec.coverage == WallCoverage::None, "{id}: K2 presents it");
        assert!(crate::permission::present::find_not_cardable(id).is_none(), "{id}");
    }
    for p in [crate::postal_routes::INVITE, crate::postal_routes::DECIDE, crate::postal_routes::LINK_P5] {
        assert!(crate::routes::route_policy::lookup(p).and_then(|x| x.post).is_some(), "{p} has a POST row");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agents_invite_waits_for_a_person_and_allow_records_it() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let _on = crate::postal_routes::switch_for_test(true);
    let ws = seed("pinv");
    claim(&ws);
    let peer = "ops::beta.postal.bot";
    let (s, v) = post(crate::postal_routes::INVITE, agent_who(&ws), json!({ "to": peer, "why": "to review each other's PRs" }));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v["asked"], true, "{v}");
    let card = card_of(&v);
    assert_eq!(friend_status(&ws, peer), None, "nothing is recorded before a person allows it");
    let vw = view(&ws, &card);
    assert_eq!(vw["setting"], format!("Send a Postal friend request for {}", ws.handle), "{vw}");
    assert!(vw["whatChanges"].as_str().unwrap().contains(&format!("Ask {peer} to be friends with {}", ws.handle)), "{vw}");
    // Asking again while it waits is the same card.
    let (_, again) = post(crate::postal_routes::INVITE, agent_who(&ws), json!({ "to": peer, "why": "to review each other's PRs" }));
    assert_eq!((again["card"].as_str(), again["waiting"].as_bool()), (Some(card.as_str()), Some(true)), "{again}");
    // A Member is below the floor; an Admin allows: recorded as them.
    below_floor(answer_as(&login("pim", Role::Member), &ws, &card, json!({ "answer": "Allow" })), "admin");
    assert_eq!(friend_status(&ws, peer), None);
    let admin = login("pia", Role::Admin);
    let (s, a) = answer_as(&admin, &ws, &card, json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "approved", "{a}");
    assert_eq!(friend_status(&ws, peer).as_deref(), Some("pending_out"));
    let f = k2_core::db::read(|conn| {
        let h = k2_core::postal::store::handle_for_workspace(conn, &ws.id).unwrap();
        k2_core::postal::store::friend_by_peer(conn, &h.id, peer).unwrap()
    });
    assert_eq!((f.asked_by.as_deref(), f.note.as_deref()), (Some(format!("login:{admin}").as_str()), Some("to review each other's PRs")));
    assert!(last_turn(&ws).contains(&format!("Friend request to {peer} recorded")), "{}", last_turn(&ws));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_incoming_request_is_k2s_card_owner_allows_deny_rejects() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let _on = crate::postal_routes::switch_for_test(true);
    let ws = seed("ppair");
    claim(&ws);
    let peer = k2_core::postal::parse_address("lab::gamma.postal.bot").unwrap();
    let got = crate::postal_routes::receive_request(&ws.id, &peer, Some("Dana"), Some("SHA256:abc"), Some("pr_1"), Some("let's pair")).expect("recorded");
    let card = got["card"]["card"].as_str().expect("K2 put a card up").to_string();
    let friend_id = got["friend"]["id"].as_str().unwrap().to_string();
    let req = row(got["card"]["request"].as_str().unwrap());
    assert_eq!((req.action.as_str(), req.held_ref.as_deref(), req.session_id.as_deref()), (PAIR_ACCEPT, Some(friend_id.as_str()), Some(K2_SESSION)));
    let vw = view(&ws, &card);
    let what = vw["whatChanges"].as_str().unwrap();
    assert!(what.contains("Let lab::gamma.postal.bot (its owner: Dana) be friends with"), "{what}");
    let effect = vw["consequences"].as_str().unwrap();
    assert!(effect.contains("let's pair") && effect.contains("SHA256:abc"), "{effect}");
    // The same request again: the same card.
    let again = crate::postal_routes::receive_request(&ws.id, &peer, None, None, Some("pr_1"), None).unwrap();
    assert!(again["card"].is_null(), "known requests put up no second card: {again}");
    // An Admin may not answer until the owner allows Admins; the owner may.
    let admin = login("ppa", Role::Admin);
    let (s, a) = answer_as(&admin, &ws, &card, json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "failed", "{a}");
    assert_eq!(friend_status(&ws, "lab::gamma.postal.bot").as_deref(), Some("pending_in"));

    // A second request: the Owner allows it.
    let peer2 = k2_core::postal::parse_address("lab2::gamma.postal.bot").unwrap();
    let got2 = crate::postal_routes::receive_request(&ws.id, &peer2, None, None, None, None).unwrap();
    let card2 = got2["card"]["card"].as_str().unwrap().to_string();
    let (s, a) = answer_as("owner", &ws, &card2, json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(friend_status(&ws, "lab2::gamma.postal.bot").as_deref(), Some("accepted"), "{a}");

    // A third: Deny rejects it, as the person.
    let peer3 = k2_core::postal::parse_address("lab3::gamma.postal.bot").unwrap();
    let got3 = crate::postal_routes::receive_request(&ws.id, &peer3, None, None, None, None).unwrap();
    let card3 = got3["card"]["card"].as_str().unwrap().to_string();
    let (s, a) = answer_as("owner", &ws, &card3, json!({ "answer": "Deny" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(friend_status(&ws, "lab3::gamma.postal.bot").as_deref(), Some("rejected"));

    // A fourth, decided in the tab: its card closes.
    let peer4 = k2_core::postal::parse_address("lab4::gamma.postal.bot").unwrap();
    let got4 = crate::postal_routes::receive_request(&ws.id, &peer4, None, None, None, None).unwrap();
    let card4 = got4["card"]["card"].as_str().unwrap().to_string();
    let id4 = got4["friend"]["id"].as_str().unwrap().to_string();
    let (s, v) = post(crate::postal_routes::DECIDE, Who::Owner, json!({ "workspace": ws.id, "id": id4, "decision": "accept" }));
    assert_eq!(s, "200 OK", "{v}");
    let doc = k2_core::overlay::get_thread_doc(&ws.conv, &card4).expect("read").expect("doc");
    assert!(!doc.is_pending_choice(), "the card closed");
    assert_eq!(row(got4["card"]["request"].as_str().unwrap()).state.as_str(), "approved");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn link_p5_mints_a_key_that_can_only_message_this_workspace_and_hands_it_to_p5() {
    let _db = k2_core::db::scoped_for_test();
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    // Works while Postal is off: it is the interim path.
    let _off = crate::postal_routes::switch_for_test(false);
    let ws = seed("plink");
    let dir = home.path().join("p5-fake");
    std::fs::create_dir_all(&dir).unwrap();
    let got = dir.join("got");
    let p5 = dir.join("p5");
    std::fs::write(&p5, format!("#!/bin/sh\necho \"$@\" > '{0}.args'\ncat > '{0}'\n", got.display())).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p5, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    *P5_FOR_TESTS.lock().unwrap() = Some(p5.clone());
    let (s, v) = post(crate::postal_routes::LINK_P5, agent_who(&ws), json!({}));
    assert_eq!(s, "200 OK", "{v}");
    let card = card_of(&v);
    let vw = view(&ws, &card);
    let effect = vw["consequences"].as_str().unwrap();
    assert!(effect.contains("can only message") && effect.contains("Remote instruction is off"), "{effect}");
    // An Admin is below the Owner's floor.
    below_floor(answer_as(&login("pla", Role::Admin), &ws, &card, json!({ "answer": "Allow" })), "owner");
    assert!(!got.exists(), "nothing handed over yet");
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow" }));
    *P5_FOR_TESTS.lock().unwrap() = None;
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "approved", "{a}");
    let key = std::fs::read_to_string(&got).expect("p5 got the key on stdin");
    let args = std::fs::read_to_string(dir.join("got.args")).unwrap();
    assert_eq!(args.trim(), format!("k2 link {} --path {} --key-stdin", ws.handle, pws(&ws).path));
    let p = k2_core::api_keys::resolve_api_key(key.trim()).expect("a live key");
    assert_eq!(k2_core::api_keys::key_label(&p.id).as_deref(), Some(format!("postal-link:{}", ws.handle).as_str()));
    assert!(p.capabilities.canonical_message && !p.capabilities.host_sessions && !p.capabilities.sandboxes && !p.capabilities.db && !p.capabilities.hire);
    assert!(p.authorizes_workspace(&ws.handle) && !p.authorizes_workspace("someone-else"));
    assert!(k2_core::workspace::settings::remote_instruct_allowed_for_path(&pws(&ws).path), "turned on with the click");
    assert!(crate::misc_routes::api_enabled(), "the /v1 surface is on with the click");
    k2_core::app_settings::set_api_enabled(false);
    assert!(!key.contains(' ') && !last_turn(&ws).contains(key.trim()), "the key is never shown");
}
