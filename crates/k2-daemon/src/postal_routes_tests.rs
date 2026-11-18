//! `/cli/postal/*` (prd-postal-in-k2-v1 S3 local half): off by default and
//! every route but settings, enable and link-p5 answers `postal_off` (409
//! to a person, 403 to an agent); on, a person claims an address, an agent
//! sees only its own workspace, asks on cards and tightens itself, and
//! nobody but the Owner (or an allowed Admin) answers a request, with its
//! pairing code when it carries one. Test DB, temp HOME; nothing talks to
//! the Postal network. These workspaces have no pinned Thread, so an
//! agent's ask answers `card_not_presented` here; the cards themselves are
//! `permission::actions::postal`'s tests.

use super::*;
use serde_json::json;

fn seed_ws(tag: &str) -> Ws {
    k2_core::db::init_for_tests();
    let id = uuid::Uuid::new_v4().to_string();
    let handle = format!("{tag}{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let path = format!("/tmp/k2-postal-{handle}");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)", rusqlite::params![id, handle, path])
            .expect("seed project");
    }
    ws_of_id(&id).expect("ws")
}

fn call(path: &str, who: Who, post: bool, body: Value, q: &[(&str, &str)]) -> (u16, Value) {
    let params: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let b = if post { body.to_string().into_bytes() } else { Vec::new() };
    let r = handle(path, who, post, &b, &params);
    let code: u16 = r.status.split_whitespace().next().and_then(|s| s.parse().ok()).expect("status");
    (code, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("json {e}: {}", r.body)))
}

fn agent(ws: &Ws) -> Who {
    Who::Agent { project_id: ws.id.clone(), session_id: "sess-postal".into() }
}

fn login(role: Role) -> Who {
    Who::Login { username: format!("p{}", &uuid::Uuid::new_v4().simple().to_string()[..6]), role }
}

#[test]
fn off_by_default_every_route_answers_off_but_settings_enable_and_link_p5() {
    let _home = crate::test_support::TempHome::new();
    let _off = switch_for_test(false);
    assert!(!k2_core::app_settings::AppSettings::default().postal_enabled, "the switch defaults off");
    let ws = seed_ws("poff");
    for (path, post) in [(STATUS, false), (FRIENDS, false), (CLAIM, true), (INVITE, true), (DECIDE, true), (REMOVE, true), (BLOCK, true)] {
        // A person: a 409 that says how to turn it on. An agent: a refusal
        // (403) whose hint names the card it can ask for.
        for (who, want) in [(Who::Owner, 409), (agent(&ws), 403)] {
            let (s, v) = call(path, who, post, json!({ "workspace": ws.path }), &[("workspace", &ws.path)]);
            assert_eq!((s, v["state"].as_str(), v["error"]["code"].as_str()), (want, Some("off"), Some("postal_off")), "{path}: {v}");
            if want == 403 {
                assert!(v["error"]["hint"].as_str().unwrap().contains("postal enable puts that card"), "{v}");
            }
        }
    }
    let (s, v) = call(SETTINGS, Who::Owner, false, Value::Null, &[]);
    assert_eq!((s, v["enabled"].as_bool()), (200, Some(false)), "{v}");
    // Mutating routes are POST only, off or on.
    for path in [CLAIM, INVITE, DECIDE, REMOVE, BLOCK, LINK_P5, ENABLE] {
        let (s, _) = call(path, Who::Owner, false, Value::Null, &[("workspace", &ws.path)]);
        assert_eq!(s, 405, "{path}");
    }
    let (s, _) = call(STATUS, Who::Owner, true, json!({}), &[]);
    assert_eq!(s, 405);
}

#[test]
fn only_the_owner_turns_it_on_and_the_generic_settings_route_cant() {
    let _home = crate::test_support::TempHome::new();
    let _g = switch_for_test(false);
    for who in [login(Role::Admin), login(Role::Member)] {
        let (s, v) = call(SETTINGS, who, true, json!({ "enabled": true }), &[]);
        assert_eq!((s, v["error"]["code"].as_str()), (403, Some("owner_only")), "{v}");
    }
    let ws = seed_ws("pset");
    let (s, _) = call(SETTINGS, agent(&ws), true, json!({ "enabled": true }), &[]);
    assert_eq!(s, 403);
    // `k2 postal enable`: a login below the Owner is refused; an agent asks
    // on a card (no Thread here: not presented); nothing turns on.
    let (s, v) = call(ENABLE, login(Role::Admin), true, json!({}), &[]);
    assert_eq!((s, v["error"]["code"].as_str()), (403, Some("owner_only")), "{v}");
    let (s, v) = call(ENABLE, agent(&ws), true, json!({}), &[]);
    assert_eq!((s, v["error"]["code"].as_str()), (409, Some("card_not_presented")), "{v}");
    assert!(!k2_core::postal::enabled());
    let (s, v) = call(ENABLE, Who::Owner, true, json!({}), &[]);
    assert_eq!((s, v["enabled"].as_bool()), (200, Some(true)), "{v}");
    let (_, v) = call(SETTINGS, Who::Owner, true, json!({ "enabled": false }), &[]);
    assert_eq!(v["enabled"], false);
    let r = crate::settings_routes::handle_settings_update(br#"{"postalEnabled":true}"#, crate::settings_routes::SettingsActor::Owner);
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    let (s, v) = call(SETTINGS, Who::Owner, true, json!({ "enabled": true, "adminsMayAccept": true }), &[]);
    assert_eq!((s, v["enabled"].as_bool(), v["adminsMayAccept"].as_bool()), (200, Some(true), Some(true)), "{v}");
    assert!(k2_core::postal::enabled() && k2_core::app_settings::load().postal_enabled);
    let (s, v) = call(SETTINGS, Who::Owner, true, json!({ "enabled": "yes" }), &[]);
    assert_eq!(s, 400, "{v}");
    let (_, v) = call(SETTINGS, Who::Owner, true, json!({ "enabled": false }), &[]);
    assert_eq!(v["enabled"], false);
    assert!(!k2_core::postal::enabled());
}

#[test]
fn on_a_person_claims_and_an_agent_sees_only_its_own() {
    let _home = crate::test_support::TempHome::new();
    let _on = switch_for_test(true);
    let ws = seed_ws("pon");
    let other = seed_ws("poth");
    let (s, v) = call(STATUS, agent(&ws), false, Value::Null, &[]);
    assert_eq!((s, v["state"].as_str(), v["handle"].is_null()), (200, Some("on"), true), "{v}");
    assert_eq!(v["plane"]["connected"], false);
    // An agent asks for its address on a card (no Thread here: not
    // presented, nothing claimed), and can't look at another workspace.
    let addr = format!("{}::acme.postal.bot", ws.handle);
    let (s, v) = call(CLAIM, agent(&ws), true, json!({ "address": addr }), &[]);
    assert_eq!((s, v["error"]["code"].as_str()), (409, Some("card_not_presented")), "{v}");
    let (s, v) = call(CLAIM, agent(&ws), true, json!({ "address": "nope" }), &[]);
    assert_eq!((s, v["error"]["code"].as_str()), (400, Some("bad_postal_address")), "{v}");
    assert!(k2_core::db::read(|c| store::handle_for_workspace(c, &ws.id)).is_none());
    let (s, v) = call(STATUS, agent(&ws), false, Value::Null, &[("workspace", &other.path)]);
    assert_eq!((s, v["error"]["code"].as_str()), (403, Some("postal_own_workspace_only")), "{v}");
    let (s, _) = call(CLAIM, login(Role::Member), true, json!({ "workspace": ws.path, "address": addr }), &[]);
    assert_eq!(s, 403);
    let (s, v) = call(CLAIM, Who::Owner, true, json!({ "workspace": ws.path, "address": "nope" }), &[]);
    assert_eq!((s, v["error"]["code"].as_str()), (400, Some("bad_postal_address")));
    // An Admin claims; the agent sees it.
    let (s, v) = call(CLAIM, login(Role::Admin), true, json!({ "workspace": ws.path, "address": addr.to_uppercase() }), &[]);
    assert_eq!((s, v["handle"]["address"].as_str(), v["handle"]["status"].as_str()), (200, Some(addr.as_str()), Some("local")), "{v}");
    let (s, v) = call(CLAIM, Who::Owner, true, json!({ "workspace": other.path, "address": addr }), &[]);
    assert_eq!((s, v["error"]["code"].as_str()), (409, Some("postal_address_taken")), "{v}");
    let (_, v) = call(STATUS, agent(&ws), false, Value::Null, &[]);
    assert_eq!(v["handle"]["address"], addr.as_str());
    assert_eq!(v["counts"], json!({ "accepted": 0, "pendingIn": 0, "pendingOut": 0 }));
    assert_eq!((v["plane"]["connected"].as_bool(), v["plane"]["state"].as_str()), (Some(false), Some("not_available_yet")), "{v}");
    // Its own address again: no card, it has it. Another: refused before a card.
    let (s, v) = call(CLAIM, agent(&ws), true, json!({ "address": addr }), &[]);
    assert_eq!((s, v["already"].as_bool()), (200, Some(true)), "{v}");
    let (s, v) = call(CLAIM, agent(&ws), true, json!({ "address": "x::acme.postal.bot" }), &[]);
    assert_eq!((s, v["error"]["code"].as_str()), (409, Some("postal_has_address")), "{v}");
    let (s, v) = call(CLAIM, agent(&other), true, json!({ "address": addr }), &[]);
    assert_eq!((s, v["error"]["code"].as_str()), (409, Some("postal_address_taken")), "{v}");
}

#[test]
fn incoming_requests_wait_for_the_owner_and_an_agent_only_tightens() {
    let _home = crate::test_support::TempHome::new();
    let _on = switch_for_test(true);
    let ws = seed_ws("pin");
    let addr = format!("{}::acme.postal.bot", ws.handle);
    call(CLAIM, Who::Owner, true, json!({ "workspace": ws.path, "address": addr }), &[]);
    let peer = k2_core::postal::parse_address("lab::gamma.postal.bot").unwrap();
    // The seam the network half calls (no Thread here, so no card goes up).
    let got = receive_request(&ws.id, &peer, Some("Dana"), Some("SHA256:k1"), Some("pr_9"), Some("hi"), None).expect("recorded");
    let id = got["friend"]["id"].as_str().expect("id").to_string();
    assert_eq!(got["friend"]["status"], "pending_in");
    let (_, v) = call(FRIENDS, agent(&ws), false, Value::Null, &[]);
    assert_eq!(v["friends"][0]["peer"], "lab::gamma.postal.bot", "{v}");
    // The agent never accepts: its accept asks for K2's card again (no
    // Thread here: not presented). A Member and an Admin (not allowed)
    // don't decide either.
    let (s, v) = call(DECIDE, agent(&ws), true, json!({ "id": id, "decision": "accept" }), &[]);
    assert_eq!((s, v["error"]["code"].as_str()), (409, Some("card_not_presented")), "{v}");
    assert_eq!(k2_core::db::read(|c| store::friend(c, &id)).unwrap().status, "pending_in");
    for role in [Role::Member, Role::Admin] {
        let (s, v) = call(DECIDE, login(role), true, json!({ "workspace": ws.path, "id": id, "decision": "accept" }), &[]);
        assert_eq!((s, v["error"]["code"].as_str()), (403, Some("postal_owner_decides")), "{v}");
    }
    let (s, _) = call(DECIDE, Who::Owner, true, json!({ "workspace": ws.path, "id": id, "decision": "maybe" }), &[]);
    assert_eq!(s, 400);
    let (s, v) = call(DECIDE, Who::Owner, true, json!({ "workspace": ws.path, "id": id, "decision": "accept" }), &[]);
    assert_eq!((s, v["friend"]["status"].as_str()), (200, Some("accepted")), "{v}");
    // The agent may tighten its own: remove, block.
    let (s, v) = call(BLOCK, agent(&ws), true, json!({ "peer": "lab::gamma.postal.bot" }), &[]);
    assert_eq!((s, v["friend"]["status"].as_str()), (200, Some("blocked")), "{v}");
    assert_eq!(receive_request(&ws.id, &peer, None, None, None, None, None).unwrap_err(), StoreError::Blocked);
    let (s, v) = call(REMOVE, login(Role::Member), true, json!({ "workspace": ws.path, "peer": "lab::gamma.postal.bot" }), &[]);
    assert_eq!((s, v["error"]["code"].as_str()), (403, Some("postal_people_only")));
    let (s, v) = call(REMOVE, agent(&ws), true, json!({ "peer": "lab::gamma.postal.bot" }), &[]);
    assert_eq!((s, v["friend"]["status"].as_str()), (200, Some("revoked")), "{v}");
    // A person's invite from the tab is recorded at once (their click is the consent).
    let (s, v) = call(INVITE, Who::Owner, true, json!({ "workspace": ws.path, "to": "ops::beta.postal.bot" }), &[]);
    assert_eq!((s, v["friend"]["status"].as_str()), (200, Some("pending_out")), "{v}");
    let (s, _) = call(INVITE, login(Role::Member), true, json!({ "workspace": ws.path, "to": "x1::beta.postal.bot" }), &[]);
    assert_eq!(s, 403);
    // An agent's invite needs a why; with one it asks its person (no Thread here: refused, nothing recorded).
    let (s, v) = call(INVITE, agent(&ws), true, json!({ "to": "x2::beta.postal.bot" }), &[]);
    assert_eq!((s, v["error"]["code"].as_str()), (400, Some("usage")), "{v}");
    let (s, v) = call(INVITE, agent(&ws), true, json!({ "to": "x2::beta.postal.bot", "why": "to review PRs" }), &[]);
    assert_eq!((s, v["error"]["code"].as_str()), (409, Some("card_not_presented")), "{v}");
    let (_, v) = call(FRIENDS, agent(&ws), false, Value::Null, &[]);
    assert!(!v["friends"].as_array().unwrap().iter().any(|f| f["peer"] == "x2::beta.postal.bot"), "{v}");
}

#[test]
fn an_agent_turns_a_request_down_itself_and_a_coded_request_waits() {
    let _home = crate::test_support::TempHome::new();
    let _on = switch_for_test(true);
    let ws = seed_ws("pcode");
    call(CLAIM, Who::Owner, true, json!({ "workspace": ws.path, "address": format!("{}::acme.postal.bot", ws.handle) }), &[]);
    // Rejecting is tightening: the agent's own, queued for the SDK.
    let p1 = k2_core::postal::parse_address("spam::gamma.postal.bot").unwrap();
    let id1 = receive_request(&ws.id, &p1, None, None, Some("pr_s"), None, None).unwrap()["friend"]["id"].as_str().unwrap().to_string();
    let (s, v) = call(DECIDE, agent(&ws), true, json!({ "id": id1, "decision": "reject" }), &[]);
    assert_eq!((s, v["friend"]["status"].as_str(), v["friend"]["decidedBy"].as_str()), (200, Some("rejected"), Some(format!("agent:{}", ws.handle).as_str())), "{v}");
    assert!(matches!(k2_core::postal::gate::answers_for(&id1).last(), Some(k2_core::postal::gate::Answer::Reject { .. })));
    let (s, v) = call(DECIDE, agent(&ws), true, json!({ "id": id1, "decision": "reject" }), &[]);
    assert_eq!((s, v["error"]["code"].as_str()), (409, Some("postal_not_pending")), "{v}");
    // Another workspace's request id is not found for this agent.
    let other = seed_ws("pcod2");
    let (s, _) = call(DECIDE, agent(&other), true, json!({ "id": id1, "decision": "reject" }), &[]);
    assert_eq!(s, 404);

    // A request with a pairing code: never in the list (agents read it),
    // and it can't be accepted yet (the SDK checks the other box's code);
    // Reject still works.
    let p2 = k2_core::postal::parse_address("lab::gamma.postal.bot").unwrap();
    let got = receive_request(&ws.id, &p2, Some("Dana"), None, Some("pr_c"), None, Some("123 456")).unwrap();
    let id2 = got["friend"]["id"].as_str().unwrap().to_string();
    let (_, v) = call(FRIENDS, Who::Owner, false, Value::Null, &[("workspace", &ws.path)]);
    let row = v["friends"].as_array().unwrap().iter().find(|f| f["id"] == id2.as_str()).cloned().unwrap();
    assert_eq!(row["codeWaiting"], true, "{row}");
    assert!(!v.to_string().contains("123456") && !v.to_string().contains("123 456"), "the code is never in the list: {v}");
    let (s, v) = call(DECIDE, Who::Owner, true, json!({ "workspace": ws.path, "id": id2, "decision": "accept", "sas": "123456" }), &[]);
    assert_eq!((s, v["error"]["code"].as_str()), (409, Some("postal_code_check_not_available")), "{v}");
    assert!(v["error"]["hint"].as_str().unwrap().contains("later release"), "{v}");
    assert_eq!(k2_core::db::read(|c| store::friend(c, &id2)).unwrap().status, "pending_in", "nothing pretended");
    let (s, v) = call(DECIDE, Who::Owner, true, json!({ "workspace": ws.path, "id": id2, "decision": "reject" }), &[]);
    assert_eq!((s, v["friend"]["status"].as_str()), (200, Some("rejected")), "{v}");
    assert_eq!(k2_core::postal::gate::code(&id2), None, "answered: the code is gone");

    // A person's invite: recorded, never claimed as sent.
    let (s, v) = call(INVITE, Who::Owner, true, json!({ "workspace": ws.path, "to": "ops::beta.postal.bot" }), &[]);
    assert_eq!((s, v["sent"].as_bool(), v["network"].as_str()), (200, Some(false), Some("not_available_yet")), "{v}");
}

#[test]
fn the_sdk_gate_records_and_never_decides() {
    use k2_core::postal::gate::{FriendRequest, HumanGate, RekeyPending};
    use k2_core::postal::sdk::pairing::{Fingerprint, Side};
    let _home = crate::test_support::TempHome::new();
    let _on = switch_for_test(true);
    let ws = seed_ws("pgate");
    let me = format!("{}::acme.postal.bot", ws.handle);
    call(CLAIM, Who::Owner, true, json!({ "workspace": ws.path, "address": me }), &[]);
    let req = FriendRequest {
        pair_id: "pr_g".into(),
        local_handle: me.clone(),
        peer: "lab::gamma.postal.bot".into(),
        peer_owner_name: Some("Dana".into()),
        side: Side::To,
        show_code: "111222".into(),
        peer_fingerprint: Fingerprint::from_bytes([7; 32]),
    };
    K2Gate.friend_request(&req);
    let f = k2_core::db::read(|c| {
        let h = store::handle_for_workspace(c, &ws.id).unwrap();
        store::friend_by_peer(c, &h.id, "lab::gamma.postal.bot").unwrap()
    });
    assert_eq!((f.status.as_str(), f.pair_id.as_deref(), f.owner_name.as_deref()), ("pending_in", Some("pr_g"), Some("Dana")));
    assert_eq!(f.peer_fingerprint.as_deref(), Some("07".repeat(32).as_str()), "the SDK's fingerprint, as hex");
    assert_eq!(k2_core::postal::gate::code(&f.id).as_deref(), Some("111222"), "this box's code, kept for people");
    // Accepting it needs the SDK's handshake: not in this release.
    let (s, v) = call(DECIDE, Who::Owner, true, json!({ "workspace": ws.path, "id": f.id, "decision": "accept" }), &[]);
    assert_eq!((s, v["error"]["code"].as_str()), (409, Some("postal_code_check_not_available")), "{v}");
    // A request for an address nobody here holds is dropped, not recorded.
    K2Gate.friend_request(&FriendRequest { local_handle: "nobody::acme.postal.bot".into(), peer: "x9::gamma.postal.bot".into(), ..req.clone() });
    assert!(k2_core::db::read(|c| {
        let h = store::handle_for_workspace(c, &ws.id).unwrap();
        store::friend_by_peer(c, &h.id, "x9::gamma.postal.bot")
    })
    .is_none());
    // A friend a person accepted; then its key changes: it waits again.
    let p2 = k2_core::postal::parse_address("lab2::gamma.postal.bot").unwrap();
    let id2 = receive_request(&ws.id, &p2, None, None, None, None, None).unwrap()["friend"]["id"].as_str().unwrap().to_string();
    let (s, _) = call(DECIDE, Who::Owner, true, json!({ "workspace": ws.path, "id": id2, "decision": "accept" }), &[]);
    assert_eq!(s, 200);
    K2Gate.rekey(&RekeyPending { local_handle: me, peer: "lab2::gamma.postal.bot".into(), old_fingerprint: "a".into(), new_fingerprint: "b".into() });
    assert_eq!(k2_core::db::read(|c| store::friend(c, &id2)).unwrap().status, "rekey_pending");
}
