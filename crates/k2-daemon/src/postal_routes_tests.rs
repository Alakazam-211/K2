//! `/cli/postal/*` (prd-postal-in-k2-v1 S3 local half): off by default and
//! every route but settings and link-p5 answers `postal_off`; on, a person
//! claims an address, an agent sees only its own workspace and can only
//! tighten, and nobody but the Owner (or an allowed Admin) answers a
//! request. Test DB, temp HOME; nothing talks to the Postal network.

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
fn off_by_default_every_route_answers_off_but_settings_and_link_p5() {
    let _home = crate::test_support::TempHome::new();
    let _off = switch_for_test(false);
    assert!(!k2_core::app_settings::AppSettings::default().postal_enabled, "the switch defaults off");
    let ws = seed_ws("poff");
    for (path, post) in [(STATUS, false), (FRIENDS, false), (CLAIM, true), (INVITE, true), (DECIDE, true), (REMOVE, true), (BLOCK, true)] {
        for who in [Who::Owner, agent(&ws)] {
            let (s, v) = call(path, who, post, json!({ "workspace": ws.path }), &[("workspace", &ws.path)]);
            assert_eq!((s, v["state"].as_str(), v["error"]["code"].as_str()), (409, Some("off"), Some("postal_off")), "{path}: {v}");
        }
    }
    let (s, v) = call(SETTINGS, Who::Owner, false, Value::Null, &[]);
    assert_eq!((s, v["enabled"].as_bool()), (200, Some(false)), "{v}");
    // Mutating routes are POST only, off or on.
    for path in [CLAIM, INVITE, DECIDE, REMOVE, BLOCK, LINK_P5] {
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
    assert!(!k2_core::postal::enabled());
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
    // An agent can't claim, and can't look at another workspace.
    let addr = format!("{}::acme.postal.bot", ws.handle);
    let (s, v) = call(CLAIM, agent(&ws), true, json!({ "address": addr }), &[]);
    assert_eq!((s, v["error"]["code"].as_str()), (403, Some("postal_people_only")), "{v}");
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
    let got = receive_request(&ws.id, &peer, Some("Dana"), Some("SHA256:k1"), Some("pr_9"), Some("hi")).expect("recorded");
    let id = got["friend"]["id"].as_str().expect("id").to_string();
    assert_eq!(got["friend"]["status"], "pending_in");
    let (_, v) = call(FRIENDS, agent(&ws), false, Value::Null, &[]);
    assert_eq!(v["friends"][0]["peer"], "lab::gamma.postal.bot", "{v}");
    // The agent never decides; a Member and an Admin (not allowed) neither.
    let (s, v) = call(DECIDE, agent(&ws), true, json!({ "id": id, "decision": "accept" }), &[]);
    assert_eq!((s, v["error"]["code"].as_str()), (403, Some("postal_people_decide")), "{v}");
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
    assert_eq!(receive_request(&ws.id, &peer, None, None, None, None).unwrap_err(), StoreError::Blocked);
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
