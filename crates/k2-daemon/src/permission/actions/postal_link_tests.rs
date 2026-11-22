//! Postal's rule: **if an invite can be sent, it can be accepted.**
//! The owner's decide token (the approval token) is what confirms a pair;
//! these pin that no path leaves this server holding a friendship it can't
//! finish, against a stub plane with the network marked connected:
//! - an invite or a claim with no token here, and a mint that fails for
//!   good (401), is refused up front with what is missing and why: no card,
//!   no row (an agent's ask after that waits, never calling the plane);
//! - with the mint answering, the token is got first and the invite goes on;
//! - the send loop keeps an allowed invite here until the token is here;
//! - a request that arrives while this server can't accept puts its card up
//!   saying so, with the fix (Accept links first; Link now in Settings);
//! - a person's Accept with no token gets it and confirms in that same
//!   action, with the token it just got;
//! - no card, answer, audit row or owner notice holds the token or offers a
//!   page on the Postal website.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use k2_core::connect_users::Role;
use k2_core::postal::sdk::plane::Request;
use k2_core::postal::store;
use serde_json::{json, Value};

use super::super::held::tests::{answer_as, card_of, login, seed, Ws};
use super::*;
use crate::postal_net::decide_token;
use crate::postal_net::plane::{Answer, Plane, Transport};

const LABEL: &str = "initech";
const TOKEN: &str = "k2c_pd_0123456789abcdef0123456789abcdef";
const PEER: &str = "ops::globex.postal.bot";
const PAIR: &str = "01JDQ8Z6X9K2M3N4P5R6S7T8V9";

/// The plane, answered from a script per path (the last answer repeats);
/// `/session` and `/me` answer by default. Every call is kept with its
/// bearer, in the test's memory only.
#[derive(Default)]
struct Stub {
    answers: Mutex<HashMap<String, Vec<(u16, Value)>>>,
    calls: Mutex<Vec<(String, String)>>,
}

impl Stub {
    fn script(&self, path: &str, answers: Vec<(u16, Value)>) {
        self.answers.lock().unwrap().insert(path.to_string(), answers);
    }
    fn paths(&self) -> Vec<String> {
        self.calls.lock().unwrap().iter().map(|c| c.0.clone()).collect()
    }
    fn bearer_of(&self, path: &str) -> Option<String> {
        self.calls.lock().unwrap().iter().find(|c| c.0 == path).map(|c| c.1.clone())
    }
}

impl Transport for Arc<Stub> {
    fn call(&self, bearer: &str, req: &Request) -> Result<Answer, String> {
        let path = req.path.strip_prefix("/api/postal/v1").expect("a v1 path").split('?').next().unwrap_or("").to_string();
        self.calls.lock().unwrap().push((path.clone(), bearer.to_string()));
        let mut a = self.answers.lock().unwrap();
        let (s, v) = match a.get_mut(path.as_str()) {
            Some(list) if list.len() > 1 => list.remove(0),
            Some(list) => list[0].clone(),
            None => match path.as_str() {
                "/session" => (200, json!({ "token": "pst_0123456789abcdef0123456789abcdef", "label": LABEL, "expires_at": "x" })),
                "/me" => (200, json!({ "ok": true })),
                _ => (404, json!({ "error": { "code": "not_found", "message": "" } })),
            },
        };
        Ok((s, v.to_string().into_bytes()))
    }
    fn post_open(&self, _path: &str, _body: &Value) -> Result<Answer, String> {
        Err("not in this test".into())
    }
}

fn minted() -> (u16, Value) {
    (200, json!({ "token": TOKEN, "scope": "postal-decide", "label": LABEL, "minted_at": "2026-11-21T00:00:00Z" }))
}

fn unauthorized() -> (u16, Value) {
    (401, json!({ "error": { "code": "unauthorized", "message": "see https://postal.bot/help" } }))
}

/// The network marked connected for the test, and not after.
struct Connected;

impl Drop for Connected {
    fn drop(&mut self) {
        k2_core::postal::gate::set_network_available(false);
    }
}

struct Kit {
    _home: crate::test_support::TempHome,
    _on: crate::postal_routes::SwitchGuard,
    _connected: Connected,
    stub: Arc<Stub>,
}

fn kit() -> Kit {
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    k2_core::postal::keys::install(Arc::new(k2_core::postal::keys::MemorySecrets::default()));
    let on = crate::postal_routes::switch_for_test(true);
    decide_token::reset_for_test();
    k2_core::postal::gate::set_network_available(true);
    let stub = Arc::new(Stub::default());
    crate::postal_net::install(Arc::new(Plane::new(Box::new(stub.clone()), Box::new(|| Ok(("k2c_connect_test".to_string(), LABEL.to_string()))))));
    Kit { _home: home, _on: on, _connected: Connected, stub }
}

fn address(ws: &Ws) -> String {
    let addr = format!("{}::{LABEL}.postal.bot", ws.handle);
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::claim(&conn, &ws.id, &k2_core::postal::parse_address(&addr).expect("addr"), "owner").expect("claim");
    addr
}

/// The workspace's address, on the network already (its key registered).
fn claimed(ws: &Ws) -> String {
    let addr = address(ws);
    k2_core::postal::keys::identity_or_create(&addr).expect("the handle's key");
    let db = k2_core::db::shared();
    let conn = db.lock();
    let h = store::handle_for_workspace(&conn, &ws.id).expect("handle");
    store::set_handle_status(&conn, &h.id, "claimed", Some("k")).expect("claimed");
    addr
}

fn agent_who(ws: &Ws) -> Who {
    Who::Agent { project_id: ws.id.clone(), session_id: format!("sess-{}", &ws.id[..8]) }
}

fn post(path: &str, who: Who, body: Value) -> (String, Value) {
    let r = crate::postal_routes::handle(path, who, true, body.to_string().as_bytes(), &HashMap::new());
    clean(&r.body);
    (r.status.to_string(), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{e}: {}", r.body)))
}

/// Never the token, never a page on the Postal website.
fn clean(text: &str) {
    assert!(!text.contains("k2c_pd_"), "the token is never shown: {text}");
    assert!(!text.contains("http://") && !text.contains("https://") && !text.contains("postal.bot/"), "no web page: {text}");
}

fn friend_of(ws: &Ws, peer: &str) -> Option<store::Friend> {
    k2_core::db::read(|c| {
        let h = store::handle_for_workspace(c, &ws.id)?;
        store::friend_by_peer(c, &h.id, peer)
    })
}

fn stored() -> Option<String> {
    k2_core::postal::keys::decide_token(LABEL).expect("the secrets answer")
}

fn card_text(ws: &Ws, card: &str) -> String {
    let doc = k2_core::overlay::get_thread_doc(&ws.conv, card).expect("doc read").expect("card doc");
    serde_json::to_string(&doc).expect("doc json")
}

fn open_cards(ws: &Ws, action: &str) -> Vec<k2_core::permission::request::PermissionRequest> {
    crate::permission::store::open(Some(ws.id.as_str())).expect("open requests").into_iter().filter(|r| r.action == action).collect()
}

/// Audit rows and owner notices hold no token.
fn records_clean() {
    for a in crate::hire_routes::AUDIT_SINK.lock().unwrap().iter() {
        assert!(!a.2.contains("k2c_pd_"), "{a:?}");
    }
    for n in decide_token::OWNER_NOTICES.lock().unwrap().iter() {
        clean(n);
    }
}

#[test]
fn an_invite_and_a_claim_are_refused_up_front_when_this_server_cant_accept() {
    let _db = k2_core::db::scoped_for_test();
    let k = kit();
    let ws = seed("plnk");
    address(&ws);
    k.stub.script("/decide-token", vec![unauthorized()]);

    // The agent's invite: refused before any card, saying what and why.
    let (s, v) = post(crate::postal_routes::INVITE, agent_who(&ws), json!({ "to": PEER, "why": "to review each other's PRs" }));
    assert_eq!((s.as_str(), v["error"]["code"].as_str()), ("409 Conflict", Some("postal_cant_accept")), "{v}");
    let hint = v["error"]["hint"].as_str().expect("hint");
    assert!(hint.starts_with("a friend request can't go out from this server yet"), "{hint}");
    assert!(hint.contains(&format!("didn't accept this server's {} Connect account", k2_core::product_name!())), "why: {hint}");
    assert!(hint.contains("Link now"), "the fix: {hint}");
    assert!(open_cards(&ws, INVITE).is_empty(), "no card for an invite that couldn't be accepted");
    assert!(friend_of(&ws, PEER).is_none(), "nothing recorded");
    assert_eq!(k.stub.paths(), ["/decide-token"], "it tried to link first");

    // Asking again soon is answered without calling the plane.
    let (s, v) = post(crate::postal_routes::INVITE, agent_who(&ws), json!({ "to": PEER, "why": "to review each other's PRs" }));
    assert_eq!((s.as_str(), v["error"]["code"].as_str()), ("409 Conflict", Some("postal_cant_accept")), "{v}");
    assert_eq!(k.stub.paths().len(), 1, "an agent's ask waits after a failure: {:?}", k.stub.paths());

    // A person's own invite asks again, and is refused the same way.
    let (s, v) = post(crate::postal_routes::INVITE, Who::Owner, json!({ "workspace": ws.id, "to": PEER, "why": "x" }));
    assert_eq!((s.as_str(), v["error"]["code"].as_str()), ("409 Conflict", Some("postal_cant_accept")), "{v}");
    assert!(friend_of(&ws, PEER).is_none());
    assert_eq!(k.stub.paths().len(), 2);

    // A claim too: the agent's (waits) and a person's.
    let ws2 = seed("plnc");
    let addr2 = format!("{}::{LABEL}.postal.bot", ws2.handle);
    let (s, v) = post(crate::postal_routes::CLAIM, agent_who(&ws2), json!({ "address": addr2 }));
    assert_eq!((s.as_str(), v["error"]["code"].as_str()), ("409 Conflict", Some("postal_cant_accept")), "{v}");
    assert!(v["error"]["hint"].as_str().expect("hint").starts_with("this workspace can't take a Postal address yet"), "{v}");
    assert!(open_cards(&ws2, CLAIM).is_empty());
    assert_eq!(k.stub.paths().len(), 2, "the agent's claim waited");
    let (s, v) = post(crate::postal_routes::CLAIM, Who::Owner, json!({ "workspace": ws2.id, "address": addr2 }));
    assert_eq!((s.as_str(), v["error"]["code"].as_str()), ("409 Conflict", Some("postal_cant_accept")), "{v}");
    assert!(k2_core::db::read(|c| store::handle_for_workspace(c, &ws2.id)).is_none(), "no address recorded");
    assert_eq!(stored(), None);
    records_clean();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_invite_gets_the_token_first_then_goes_out() {
    let _db = k2_core::db::scoped_for_test();
    let k = kit();
    let ws = seed("plnm");
    claimed(&ws);
    k.stub.script("/decide-token", vec![minted()]);
    k.stub.script("/pairs", vec![(200, json!({ "id": "pr_link", "status": "pending_out" }))]);

    let (s, v) = post(crate::postal_routes::INVITE, agent_who(&ws), json!({ "to": PEER, "why": "to review each other's PRs" }));
    assert_eq!((s.as_str(), v["asked"].as_bool()), ("200 OK", Some(true)), "{v}");
    assert_eq!(stored().as_deref(), Some(TOKEN), "the token came first");
    assert_eq!(k.stub.paths(), ["/decide-token"]);
    let card = card_of(&v);
    clean(&card_text(&ws, &card));

    let admin = login("plna", Role::Admin);
    let (s, a) = answer_as(&admin, &ws, &card, json!({ "answer": "Allow" }));
    clean(&a.to_string());
    assert_eq!((s.as_str(), a["action"]["result"]["outcome"].as_str()), ("200 OK", Some("approved")), "{a}");
    let f = friend_of(&ws, PEER).expect("recorded");
    assert!(f.pair_id.is_some(), "sent: {f:?}");
    assert_eq!(k.stub.paths().iter().filter(|p| *p == "/decide-token").count(), 1, "no second mint: {:?}", k.stub.paths());
    assert!(k.stub.bearer_of("/pairs").is_some_and(|b| b.starts_with("pst_")), "the box's session sends it");
    records_clean();
}

#[test]
fn the_send_loop_keeps_an_invite_here_until_this_server_can_accept() {
    let _db = k2_core::db::scoped_for_test();
    let k = kit();
    let ws = seed("plns");
    claimed(&ws);
    // Allowed while the network was down: recorded, not sent.
    let id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let h = store::handle_for_workspace(&conn, &ws.id).expect("handle");
        store::record_outgoing(&conn, &h, &k2_core::postal::parse_address(PEER).expect("peer"), Some("hi"), "owner").expect("request").id
    };
    k.stub.script("/decide-token", vec![(500, json!({ "error": { "code": "internal", "message": "" } }))]);
    k.stub.script("/pairs", vec![(200, json!({ "id": "pr_loop", "status": "pending_out" }))]);
    let net = crate::postal_net::net().expect("plane");

    crate::postal_net::pairing::send_unsent(&net).expect("the loop goes on");
    assert_eq!(k.stub.paths(), ["/decide-token"], "no pair request without the token");
    assert!(k2_core::db::read(|c| store::friend(c, &id)).expect("row").pair_id.is_none(), "it waits here");
    crate::postal_net::pairing::send_unsent(&net).expect("the loop goes on");
    assert_eq!(k.stub.paths().len(), 1, "the next pass waits too, without calling the plane");

    // The wait is over and the network answers: the token, then the request.
    decide_token::reset_for_test();
    k.stub.script("/decide-token", vec![minted()]);
    crate::postal_net::pairing::send_unsent(&net).expect("sent");
    let paths = k.stub.paths();
    assert_eq!(paths.first().map(String::as_str), Some("/decide-token"));
    assert_eq!(paths.iter().filter(|p| *p == "/decide-token").count(), 2, "{paths:?}");
    assert_eq!(paths.last().map(String::as_str), Some("/pairs"), "{paths:?}");
    assert!(k2_core::db::read(|c| store::friend(c, &id)).expect("row").pair_id.is_some());
    assert_eq!(stored().as_deref(), Some(TOKEN));
}

#[test]
fn a_request_this_server_cant_accept_says_so_on_its_card_with_the_fix() {
    let _db = k2_core::db::scoped_for_test();
    let k = kit();
    let ws = seed("plnr");
    address(&ws);
    k.stub.script("/decide-token", vec![unauthorized()]);
    let peer = k2_core::postal::parse_address(PEER).expect("peer");
    let got = crate::postal_routes::receive_request(&ws.id, &peer, None, None, Some(PAIR), None, Some("482913")).expect("recorded");
    let card = got["card"]["card"].as_str().unwrap_or_else(|| panic!("the card is up: {got}")).to_string();
    let text = card_text(&ws, &card);
    clean(&text);
    assert!(text.contains("This server can't accept friend requests yet: it doesn't have its Postal approval token"), "{text}");
    assert!(text.contains(&format!("didn't accept this server's {} Connect account", k2_core::product_name!())), "why: {text}");
    assert!(text.contains("Accept gets it first") && text.contains("Link now in Settings → General → Postal"), "the fix: {text}");
    let req = open_cards(&ws, PAIR_ACCEPT).into_iter().next().expect("the request's card");
    assert_eq!(req.params["link"], json!("unauthorized"), "a kind, never free text");
    assert_eq!(k.stub.paths(), ["/decide-token"], "it tried to link as the request arrived");

    // Another request soon after: no new plane call, the same words.
    let other = k2_core::postal::parse_address("lab::globex.postal.bot").expect("peer");
    let got = crate::postal_routes::receive_request(&ws.id, &other, None, None, Some("01JTESTPAIR00000000000002"), None, Some("111222")).expect("recorded");
    let card2 = got["card"]["card"].as_str().expect("card").to_string();
    assert!(card_text(&ws, &card2).contains("This server can't accept friend requests yet"));
    assert_eq!(k.stub.paths().len(), 1, "waits after a failure: {:?}", k.stub.paths());

    // With the token here, a request's card says nothing of the kind.
    k2_core::postal::keys::set_decide_token(LABEL, TOKEN).expect("stored");
    let third = k2_core::postal::parse_address("qa::globex.postal.bot").expect("peer");
    let got = crate::postal_routes::receive_request(&ws.id, &third, None, None, Some("01JTESTPAIR00000000000003"), None, Some("333444")).expect("recorded");
    let card3 = got["card"]["card"].as_str().expect("card").to_string();
    let text3 = card_text(&ws, &card3);
    clean(&text3);
    assert!(!text3.contains("can't accept friend requests yet"), "{text3}");
}

#[test]
fn a_persons_accept_with_no_token_gets_it_and_confirms_in_the_same_action() {
    use k2_core::postal::sdk::identity::Identity;
    use k2_core::postal::sdk::pairing::{Attempts, Initiator, PairContext, Responder};
    let _db = k2_core::db::scoped_for_test();
    let k = kit();
    let ws = seed("plna");
    let me = address(&ws);
    let mine = k2_core::postal::keys::identity_or_create(&me).expect("this handle's key");
    let theirs = Identity::from_seed([7u8; 32]);

    // A finished SAS v2 handshake on both sides (they asked; this box answered).
    let ctx = PairContext { pair_id: PAIR.into(), initiator: PEER.into(), responder: me.clone() };
    let now = crate::postal_net::pairing::now();
    let (init, c) = Initiator::start(ctx.clone(), theirs.public_key(), &mut Attempts::from_log(Vec::new()), now).expect("start");
    let (resp_state, resp) = Responder::respond(ctx, theirs.public_key(), c, mine.public_key(), &mut Attempts::from_log(Vec::new()), now).expect("respond");
    let (reveal, their_hs) = init.finish(resp, now).expect("their handshake");
    let my_hs = resp_state.finish(reveal, now).expect("my handshake");
    let their_code = their_hs.code().as_str().to_string();
    let my_code = my_hs.code().as_str().to_string();
    let id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let h = store::handle_for_workspace(&conn, &ws.id).expect("handle");
        match store::record_incoming(&conn, &h, &k2_core::postal::parse_address(PEER).expect("peer"), None, None, Some(PAIR), None).expect("request") {
            store::Incoming::New(f) | store::Incoming::Known(f) => f.id,
        }
    };
    k2_core::postal::gate::set_code(&id, &my_code);
    crate::postal_net::pairing::hold_handshake_for_test(PAIR, my_hs);

    // No token here: one Accept (the code from their screen) gets it and confirms.
    k.stub.script("/decide-token", vec![minted()]);
    let confirm = format!("/pairs/{PAIR}/confirm");
    k.stub.script(
        &confirm,
        vec![(200, json!({ "id": PAIR, "side": "to", "handle": me, "peer": PEER, "direction": "in", "status": "pending_in", "step": "confirmed", "sas_version": 2 }))],
    );
    assert_eq!(stored(), None);
    let (s, v) = post(crate::postal_routes::DECIDE, Who::Owner, json!({ "workspace": ws.id, "id": id, "decision": "accept", "code": their_code }));
    assert_eq!(s, "200 OK", "one Accept is enough: {v}");
    assert_eq!(v["hint"], json!("your side is confirmed; they become friends when the other side types your code"), "{v}");
    let paths = k.stub.paths();
    assert_eq!(&paths[..2], ["/decide-token".to_string(), confirm.clone()], "the token, then the confirm, in one action: {paths:?}");
    assert_eq!(k.stub.bearer_of(&confirm).as_deref(), Some(TOKEN), "confirmed with the token it just got");
    let f = k2_core::db::read(|c| store::friend(c, &id)).expect("row");
    assert_eq!((f.status.as_str(), f.decided_by.is_some()), ("pending_out", true), "this side confirmed: {f:?}");
    assert_eq!(
        crate::hire_routes::AUDIT_SINK.lock().unwrap().iter().rfind(|a| a.0 == "postal-decide-token").map(|a| a.2.clone()).as_deref(),
        Some("label=initech minted (a friend request accepted)")
    );
    records_clean();
}
