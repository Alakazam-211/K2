//! Postal's cards end to end (prd-postal-in-k2-v1 S3, S0; Rosson
//! 2026-11-18): turning Postal on, an agent's address, its invite and an
//! incoming request each wait on a card on the agent's pinned Thread; only
//! a person at the floor answers; Allow and Deny run the route's own rules
//! as that person; deciding in the tab closes the card; incoming requests
//! batch, except one with a pairing code, and never wake the agent; the p5
//! link mints a key that can only message its workspace and hands it to p5
//! on stdin. Test DB, temp HOME; no agent is woken (turns go to `inject`'s
//! sink); p5 is a script.

use k2_core::connect_users::Role;
use k2_core::permission::catalog::{Floor, Grant, Proof, WallCoverage};
use serde_json::{json, Value};

use super::super::held::tests::{answer_as, below_floor, card_of, last_turn, login, row, seed, view, Ws};
use super::*;

fn pws(ws: &Ws) -> crate::postal_routes::Ws {
    crate::postal_routes::ws_of_token(&ws.id).expect("postal ws")
}

fn claim(ws: &Ws) -> String {
    let addr = format!("{}::initech.postal.bot", ws.handle);
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
fn the_cards_are_registered_k2_presented_clicks_with_their_walls() {
    let _db = k2_core::db::scoped_for_test();
    for (id, floor, grant, coverage) in [
        (INVITE, Floor::Admin, Grant::OneTime, WallCoverage::Held),
        (PAIR_ACCEPT, Floor::Admin, Grant::OneTime, WallCoverage::Held),
        (LINK_P5, Floor::Owner, Grant::OneTime, WallCoverage::Held),
        (CLAIM, Floor::Admin, Grant::OneTime, WallCoverage::Held),
        (ENABLE, Floor::Owner, Grant::Standing, WallCoverage::Code),
    ] {
        let r = super::super::find(id).expect("registered");
        k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!((r.spec.floor, r.spec.proof, r.spec.grant, r.spec.coverage), (floor, Proof::Click, grant, coverage), "{id}");
        assert!(crate::permission::walls::V1_ACTIONS.contains(&id), "{id}: a v1 action (the Ask-with lines name it)");
        assert!(crate::permission::present::find_not_cardable(id).is_none(), "{id}");
    }
    for p in [
        crate::postal_routes::INVITE,
        crate::postal_routes::DECIDE,
        crate::postal_routes::LINK_P5,
        crate::postal_routes::CLAIM,
        crate::postal_routes::ENABLE,
    ] {
        assert!(crate::routes::route_policy::lookup(p).and_then(|x| x.post).is_some(), "{p} has a POST row");
        assert!(crate::session_token::is_agent_verb(p), "{p}: an agent may ask");
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
    let turn = last_turn(&ws);
    // Not connected to the Postal network in this test: recorded, and it
    // says so (never "sent").
    assert!(turn.contains(&format!("Friend request to {peer} recorded")) && turn.contains("as soon as this server is connected"), "{turn}");
    assert!(vw["consequences"].as_str().unwrap().contains("as soon as this server is connected"), "the card never says it was sent: {vw}");
    assert_eq!(f.pair_id, None, "not on the Postal network yet: the loop sends it once connected");
}

fn no_postal_turn(ws: &Ws) {
    let turns = crate::permission::inject::sent_to(&ws.handle);
    assert!(!turns.iter().any(|t| t.contains("Postal friend")), "a request (or its answer) never wakes the agent: {turns:?}");
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
    let got = crate::postal_routes::receive_request(&ws.id, &peer, Some("Dana"), Some("SHA256:abc"), Some("pr_1"), Some("let's pair"), None)
        .expect("recorded");
    let card = got["card"]["card"].as_str().expect("K2 put a card up").to_string();
    let friend_id = got["friend"]["id"].as_str().unwrap().to_string();
    let req = row(got["card"]["request"].as_str().unwrap());
    assert_eq!((req.action.as_str(), req.held_ref.as_deref(), req.session_id.as_deref()), (PAIR_ACCEPT, Some(friend_id.as_str()), Some(K2_SESSION)));
    let vw = view(&ws, &card);
    let what = vw["whatChanges"].as_str().unwrap();
    assert!(what.contains("Let lab::gamma.postal.bot (its owner: Dana) be friends with"), "{what}");
    let effect = vw["consequences"].as_str().unwrap();
    assert!(effect.contains("SHA256:abc") && effect.ends_with("Their note: \u{201c}let's pair\u{201d}"), "the note goes last, in quotes, with no stray stop: {effect}");
    // The same request again: the same card.
    let again = crate::postal_routes::receive_request(&ws.id, &peer, None, None, Some("pr_1"), None, None).unwrap();
    assert_eq!(
        (again["card"]["card"].as_str(), again["card"]["waiting"].as_bool()),
        (Some(card.as_str()), Some(true)),
        "a known request puts up no second card: {again}"
    );
    // The agent's own `k2 postal accept <id>`: that same card, not a decision.
    let (s, v) = post(crate::postal_routes::DECIDE, agent_who(&ws), json!({ "id": friend_id, "decision": "accept" }));
    assert_eq!((s.as_str(), v["asked"].as_bool(), v["card"].as_str()), ("200 OK", Some(true), Some(card.as_str())), "{v}");
    assert_eq!(friend_status(&ws, "lab::gamma.postal.bot").as_deref(), Some("pending_in"));
    // An Admin may not answer until the owner allows Admins; the owner may.
    let admin = login("ppa", Role::Admin);
    let (s, a) = answer_as(&admin, &ws, &card, json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "failed", "{a}");
    assert_eq!(friend_status(&ws, "lab::gamma.postal.bot").as_deref(), Some("pending_in"));
    no_postal_turn(&ws);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn incoming_requests_batch_and_each_is_answered_on_its_own() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let _on = crate::postal_routes::switch_for_test(true);
    let ws = seed("pbat");
    claim(&ws);
    let mut reqs = Vec::new();
    for p in ["lab2::gamma.postal.bot", "lab3::gamma.postal.bot", "lab4::gamma.postal.bot"] {
        let got = crate::postal_routes::receive_request(&ws.id, &k2_core::postal::parse_address(p).unwrap(), None, None, None, None, None).unwrap();
        reqs.push((p, got["card"]["card"].as_str().unwrap().to_string(), got["card"]["request"].as_str().unwrap().to_string(), got["friend"]["id"].as_str().unwrap().to_string()));
    }
    // Three requests in the batch window: one card.
    assert!(reqs.iter().all(|r| r.1 == reqs[0].1), "batched: {reqs:?}");
    let card = reqs[0].1.clone();
    assert_eq!(view(&ws, &card)["items"].as_array().map(Vec::len), Some(3));
    // The Owner allows the first, denies the second; the third is decided in the tab.
    let (s, a) = answer_as("owner", &ws, &card, json!({ "item": reqs[0].2, "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(friend_status(&ws, reqs[0].0).as_deref(), Some("accepted"), "{a}");
    let (s, a) = answer_as("owner", &ws, &card, json!({ "item": reqs[1].2, "answer": "Deny" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(friend_status(&ws, reqs[1].0).as_deref(), Some("rejected"));
    let (s, v) = post(crate::postal_routes::DECIDE, Who::Owner, json!({ "workspace": ws.id, "id": reqs[2].3, "decision": "accept" }));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(row(&reqs[2].2).state.as_str(), "approved", "deciding in the tab closes its card item");
    no_postal_turn(&ws);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_with_a_pairing_code_is_alone_never_shows_the_code_and_allow_needs_the_code() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let _on = crate::postal_routes::switch_for_test(true);
    let ws = seed("pcode");
    claim(&ws);
    let plain = crate::postal_routes::receive_request(&ws.id, &k2_core::postal::parse_address("a1::gamma.postal.bot").unwrap(), None, None, None, None, None).unwrap();
    let got = crate::postal_routes::receive_request(
        &ws.id,
        &k2_core::postal::parse_address("lab::gamma.postal.bot").unwrap(),
        Some("Dana"),
        Some("SHA256:abc"),
        Some("pr_c"),
        None,
        Some("482913"),
    )
    .unwrap();
    let card = got["card"]["card"].as_str().unwrap().to_string();
    assert_ne!(Some(card.as_str()), plain["card"]["card"].as_str(), "a request with a code never joins a batch");
    let vw = view(&ws, &card);
    assert!(!vw.to_string().contains("482913") && !vw.to_string().contains("482 913"), "the code is never on the card: {vw}");
    // The headline says what accepting does; the code step is the card's
    // own button. The words name who to compare codes with.
    assert!(!vw["whatChanges"].as_str().unwrap().contains("Allow opens"), "{vw}");
    assert!(vw["consequences"].as_str().unwrap().contains("First you and Dana compare 6-digit codes in K2\u{2019}s own form"), "{vw}");
    // Allow without the code (K2's form on the card types it first): refused
    // before anything is decided, and the card stays open.
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow" }));
    assert!(s.starts_with("409"), "{s} {a}");
    assert_eq!(a["error"]["code"], "postal_code_needed", "{a}");
    assert_eq!(friend_status(&ws, "lab::gamma.postal.bot").as_deref(), Some("pending_in"), "nothing pretended");
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Deny" }));
    assert_eq!(s, "200 OK", "the card was still open: {a}");
    assert_eq!(friend_status(&ws, "lab::gamma.postal.bot").as_deref(), Some("rejected"));
    no_postal_turn(&ws);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agent_asks_for_its_address_and_an_admin_gives_it() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let _on = crate::postal_routes::switch_for_test(true);
    let ws = seed("pclm");
    let addr = format!("{}::initech.postal.bot", ws.handle);
    let (s, v) = post(crate::postal_routes::CLAIM, agent_who(&ws), json!({ "address": addr.to_uppercase() }));
    assert_eq!((s.as_str(), v["asked"].as_bool(), v["action"].as_str()), ("200 OK", Some(true), Some(CLAIM)), "{v}");
    let card = card_of(&v);
    assert!(k2_core::db::read(|c| k2_core::postal::store::handle_for_workspace(c, &ws.id)).is_none(), "nothing claimed before a person allows it");
    let vw = view(&ws, &card);
    assert!(vw["whatChanges"].as_str().unwrap().contains(&format!("Give {} the Postal address {addr}", ws.handle)), "{vw}");
    below_floor(answer_as(&login("pcm", Role::Member), &ws, &card, json!({ "answer": "Allow" })), "admin");
    let (s, a) = answer_as(&login("pca", Role::Admin), &ws, &card, json!({ "answer": "Allow" }));
    assert_eq!((s.as_str(), a["action"]["result"]["outcome"].as_str()), ("200 OK", Some("approved")), "{a}");
    let h = k2_core::db::read(|c| k2_core::postal::store::handle_for_workspace(c, &ws.id)).expect("claimed");
    assert_eq!(h.address, addr);
    assert!(last_turn(&ws).contains(&format!("has the Postal address {addr}")), "{}", last_turn(&ws));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agent_asks_the_owner_to_turn_postal_on() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let _off = crate::postal_routes::switch_for_test(false);
    let ws = seed("pena");
    // Any Postal route refuses the agent and names the card; enable asks.
    let (s, v) = post(crate::postal_routes::INVITE, agent_who(&ws), json!({ "to": "ops::beta.postal.bot", "why": "x" }));
    assert_eq!((s.as_str(), v["error"]["code"].as_str()), ("403 Forbidden", Some("postal_off")), "{v}");
    let (s, v) = post(crate::postal_routes::ENABLE, agent_who(&ws), json!({ "why": "to pair with the build agent" }));
    assert_eq!((s.as_str(), v["asked"].as_bool()), ("200 OK", Some(true)), "{v}");
    let card = card_of(&v);
    let vw = view(&ws, &card);
    assert_eq!(vw["setting"], "Postal (Beta)", "{vw}");
    assert!(vw["consequences"].as_str().unwrap().contains("still waits on a person's card"), "{vw}");
    below_floor(answer_as(&login("pea", Role::Admin), &ws, &card, json!({ "answer": "Allow" })), "owner");
    assert!(!k2_core::postal::enabled());
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow" }));
    assert_eq!((s.as_str(), a["action"]["result"]["outcome"].as_str()), ("200 OK", Some("approved")), "{a}");
    assert!(k2_core::postal::enabled() && k2_core::app_settings::load().postal_enabled, "on, and kept");
    assert!(last_turn(&ws).contains("approved by"), "{}", last_turn(&ws));
    // On already: asking again needs no card.
    let (s, v) = post(crate::postal_routes::ENABLE, agent_who(&ws), json!({}));
    assert_eq!((s.as_str(), v["enabled"].as_bool(), v.get("asked")), ("200 OK", Some(true), None), "{v}");
    k2_core::app_settings::update(json!({ "postalEnabled": false })).expect("reset");
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

/// Both boxes get the card (the SDK raises the request on the side that
/// asked too): this box's allowed, sent request (`pending_out` with a pair
/// id) becomes a request with a pairing code on a card here as well, never
/// showing the code; and the person's code through K2's form makes Allow
/// "already done" rather than stale.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_asking_box_gets_the_code_card_too() {
    use k2_core::postal::gate::{FriendRequest, HumanGate};
    use k2_core::postal::sdk::pairing::{Fingerprint, Side};
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let _on = crate::postal_routes::switch_for_test(true);
    let ws = seed("pfrom");
    let me = claim(&ws);
    let peer = "ops::beta.postal.bot";
    let pair = "01JD8X4Q2W3E4R5T6Y7Z8A9B0C";
    let fid = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let h = k2_core::postal::store::handle_for_workspace(&conn, &ws.id).unwrap();
        let f = k2_core::postal::store::record_outgoing(&conn, &h, &k2_core::postal::parse_address(peer).unwrap(), Some("pair up"), "owner").unwrap();
        k2_core::postal::store::set_pair_id(&conn, &f.id, pair).unwrap();
        f.id
    };
    crate::postal_routes::K2Gate.friend_request(&FriendRequest {
        pair_id: pair.into(),
        local_handle: me,
        peer: peer.into(),
        peer_owner_name: None,
        side: Side::From,
        show_code: "246810".into(),
        peer_fingerprint: Fingerprint::from_bytes([9; 32]),
    });
    let f = k2_core::db::read(|c| k2_core::postal::store::friend(c, &fid)).unwrap();
    assert_eq!((f.status.as_str(), f.pair_id.as_deref()), ("pending_in", Some(pair)), "the same row, now a person's to answer");
    assert_eq!(gate::code(&fid).as_deref(), Some("246810"));
    let open: Vec<_> = crate::permission::store::open(Some(&ws.id)).unwrap().into_iter().filter(|r| r.action == PAIR_ACCEPT).collect();
    assert_eq!(open.len(), 1, "a card on the asking box too");
    assert_eq!(open[0].params["pairing"], "code");
    let card = open[0].card_id.clone().expect("card");
    let vw = view(&ws, &card);
    assert!(!vw.to_string().contains("246810") && !vw.to_string().contains("246 810"), "never the code: {vw}");
    // The person's code went through K2's form (the decide route signed and
    // sent it): the row waits for the other side, decided by that person.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::postal::store::confirmed_waiting_peer(&conn, &fid, "owner").unwrap();
    }
    gate::clear_code(&fid);
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "already_done", "not stale: the card's words don't move with the row: {a}");
}
