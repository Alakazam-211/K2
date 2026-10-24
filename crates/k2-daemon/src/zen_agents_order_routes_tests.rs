//! The Agents widget's order and pins through `zen_routes::handle` (the real
//! gate), under a temp HOME and a scoped DB: agents read, only the owner
//! token sets. Fail loudly.

use std::collections::HashMap;

use serde_json::{json, Value as J};

use crate::sidecar_routes::{AgentCaller, Caller};
use k2_core::zen::ZenFiles;

struct Env {
    _home: crate::test_support::TempHome,
    _db: k2_core::db::TestDbGuard,
    f: ZenFiles,
}

fn env() -> Env {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    let f = ZenFiles::local();
    f.setup().expect("zen setup");
    Env { _home: home, _db: db, f }
}

fn agent() -> Caller {
    Caller::Agent(AgentCaller { project_id: "w-reader".into(), map_key: None, args: vec![], sandboxed: false })
}

fn post(owner: bool, caller: &Caller, body: &[u8]) -> (String, J) {
    let r = crate::zen_routes::handle(super::SET_ROUTE, owner, caller, "test", &HashMap::new(), body);
    let v: J = serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("set: not JSON ({e}): {}", r.body));
    (r.status.to_string(), v)
}

fn set(owner: bool, caller: &Caller, body: J) -> (String, J) {
    post(owner, caller, body.to_string().as_bytes())
}

fn get(owner: bool, caller: &Caller, q: &[(&str, &str)]) -> (String, J) {
    let p: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let r = crate::zen_routes::handle(super::ROUTE, owner, caller, "test", &p, b"");
    let v: J = serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("get: not JSON ({e}): {}", r.body));
    (r.status.to_string(), v)
}

fn garden1(f: &ZenFiles) -> String {
    f.find_garden("Garden 1").unwrap_or_else(|| panic!("no Garden 1: {:?}", f.gardens())).1.id
}

fn stored(f: &ZenFiles) -> String {
    std::fs::read_to_string(f.agents_order_path()).unwrap_or_default()
}

fn audited(event: &str) -> usize {
    k2_core::auth_audit::tail(1000).expect("audit tail").iter().filter(|e| e["event"] == event).count()
}

#[test]
fn the_routes_are_zen_rows_with_their_own_body_cap() {
    assert!(crate::zen_routes::POST_ROUTES.contains(&super::SET_ROUTE), "set is a POST row (GET is 405)");
    assert!(crate::zen_routes::GET_ROUTES.contains(&super::ROUTE), "the read is a GET row");
    assert!(!crate::zen_routes::POST_ROUTES.contains(&super::ROUTE));
    assert_eq!(crate::zen_routes::max_body(super::SET_ROUTE), super::MAX_BODY);
    assert_eq!(super::MAX_BODY, 160 * 1024);
    assert_eq!(crate::zen_routes::too_large_for(super::SET_ROUTE).status, "413 Payload Too Large");
}

#[test]
fn the_person_sets_order_and_pins_and_an_agent_reads_them() {
    let e = env();
    let g = garden1(&e.f);
    let (s, v) = set(true, &Caller::Owner, json!({"garden": g, "order": ["b::local", "a::local"]}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v["changed"], true, "{v}");
    assert_eq!(v["garden"]["order"], json!(["b::local", "a::local"]), "{v}");
    assert_eq!(v["garden"]["pins"], J::Null, "pins untouched: {v}");
    let (s, v) = set(true, &Caller::Owner, json!({"garden": "Garden 1", "pins": ["a::local"]}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v["garden"]["order"], json!(["b::local", "a::local"]), "order kept when absent: {v}");
    assert_eq!(v["garden"]["pins"], json!(["a::local"]), "{v}");
    // The same again: nothing changes.
    let (_, v) = set(true, &Caller::Owner, json!({"garden": g, "pins": ["a::local"]}));
    assert_eq!(v["changed"], false, "{v}");

    // An agent's passport reads every Garden, or one.
    for who in [agent(), Caller::Shell { project_id: "w-shell".into() }] {
        let (s, v) = get(false, &who, &[]);
        assert_eq!(s, "200 OK", "{who:?}: {v}");
        let row = v["gardens"]
            .as_array()
            .and_then(|a| a.iter().find(|r| r["id"] == g.as_str()))
            .unwrap_or_else(|| panic!("{who:?}: no Garden 1 row: {v}"));
        assert_eq!(row["order"], json!(["b::local", "a::local"]), "{v}");
        assert_eq!(row["pins"], json!(["a::local"]), "{v}");
        let (s, v) = get(false, &who, &[("garden", "Garden 1")]);
        assert_eq!(s, "200 OK", "{v}");
        assert_eq!(v["gardens"].as_array().map(Vec::len), Some(1), "{v}");
    }
    // The owner token reads it too.
    let (s, _) = get(true, &Caller::Owner, &[]);
    assert_eq!(s, "200 OK");
    // null clears.
    let (s, v) = set(true, &Caller::Owner, json!({"garden": g, "order": null, "pins": null}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!((v["garden"]["order"].clone(), v["garden"]["pins"].clone()), (J::Null, J::Null), "{v}");
}

#[test]
fn only_the_owner_token_sets_it_and_every_refusal_is_audited() {
    let e = env();
    let g = garden1(&e.f);
    set(true, &Caller::Owner, json!({"garden": g, "order": ["keep::local"]}));
    let before = stored(&e.f);
    assert!(before.contains("keep::local"), "{before}");
    let refusals = audited("zen.agents_order.refused");
    let body = json!({"garden": g, "order": ["agent-was-here::local"]});
    let cases: Vec<(&str, bool, Caller)> = vec![
        ("an agent passport", false, agent()),
        ("a K2 terminal passport", false, Caller::Shell { project_id: "w-shell".into() }),
        ("a sandboxed agent", false, Caller::Agent(AgentCaller { project_id: "w-box".into(), map_key: None, args: vec![], sandboxed: true })),
        ("a Connect login", false, Caller::Login { username: "someone".into() }),
        ("an app pass", false, Caller::WrongCredential),
        ("no credential", false, Caller::Invalid),
        // The owner flag with a passport caller is still not the owner token.
        ("a passport beside the owner token", true, agent()),
    ];
    let n = cases.len();
    for (what, owner, who) in cases {
        let (s, v) = set(owner, &who, body.clone());
        assert_eq!(s, "403 Forbidden", "{what}: {v}");
        assert_eq!(v["error"], "owner_only", "{what}: {v}");
        assert!(
            v["message"].as_str().is_some_and(|m| m.contains("k2 zen agents-order")),
            "{what}: the refusal says how to read it: {v}"
        );
    }
    assert_eq!(stored(&e.f), before, "no refused set ever lands");
    assert_eq!(audited("zen.agents_order.refused"), refusals + n, "one audit line per refusal");
}

#[test]
fn reads_are_this_computers_only() {
    let _e = env();
    for (what, who) in [
        ("a Connect login", Caller::Login { username: "someone".into() }),
        ("an app pass", Caller::WrongCredential),
        ("no credential", Caller::Invalid),
        ("a sandboxed agent", Caller::Agent(AgentCaller { project_id: "w-box".into(), map_key: None, args: vec![], sandboxed: true })),
    ] {
        let (s, v) = get(false, &who, &[]);
        assert_eq!(s, "403 Forbidden", "{what}: {v}");
        assert_eq!(v["error"], "zen_local_only", "{what}: {v}");
    }
}

#[test]
fn bad_bodies_and_unknown_gardens_are_refused() {
    let e = env();
    let g = garden1(&e.f);
    let cases: Vec<(&str, &str, &str)> = vec![
        ("", "400 Bad Request", "agents/order/set takes"),
        ("not json", "400 Bad Request", "agents/order/set takes"),
        (r#"{"garden":"Garden 1","order":"a::local"}"#, "400 Bad Request", "agents/order/set takes"),
        (r#"{"garden":"Garden 1","order":[1,2]}"#, "400 Bad Request", "agents/order/set takes"),
        (r#"{"garden":"Garden 1","order":["a::local"],"extra":true}"#, "400 Bad Request", "unknown field"),
        (r#"{"order":["a::local"]}"#, "400 Bad Request", "garden"),
        (r#"{"garden":"Garden 1"}"#, "400 Bad Request", "needs order or pins"),
        (r#"{"garden":"Garden 1","pins":["  "]}"#, "400 Bad Request", "pins[0] is empty"),
        (r#"{"garden":"No Such Garden","order":["a::local"]}"#, "404 Not Found", "No Such Garden"),
    ];
    for (body, want_status, want) in cases {
        let (s, v) = post(true, &Caller::Owner, body.as_bytes());
        assert_eq!(s, want_status, "{body}: {v}");
        assert!(v.to_string().contains(want), "{body}: {v} must mention {want:?}");
    }
    let many: Vec<String> = (0..=200).map(|i| format!("a{i}::local")).collect();
    let (s, v) = set(true, &Caller::Owner, json!({"garden": g, "order": many}));
    assert_eq!(s, "400 Bad Request", "{v}");
    assert!(v["message"].as_str().is_some_and(|m| m.contains("at most 200")), "{v}");
    assert!(stored(&e.f).is_empty(), "nothing was written: {}", stored(&e.f));
    // An unknown Garden on the read is 404 too.
    let (s, v) = get(true, &Caller::Owner, &[("garden", "No Such Garden")]);
    assert_eq!((s.as_str(), v["error"].as_str()), ("404 Not Found", Some("unknown_garden")), "{v}");
}

#[test]
fn deleting_a_garden_through_its_route_drops_its_entry() {
    let e = env();
    let g2 = e.f.find_garden("Garden 2").expect("Garden 2").1.id;
    let (s, v) = set(true, &Caller::Owner, json!({"garden": g2, "pins": []}));
    assert_eq!(s, "200 OK", "{v}");
    assert!(stored(&e.f).contains(&g2));
    let r = crate::zen_routes::handle(
        "/cli/zen/garden/delete",
        true,
        &Caller::Owner,
        "test",
        &HashMap::new(),
        json!({"garden": g2}).to_string().as_bytes(),
    );
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert!(!stored(&e.f).contains(&g2), "the deleted Garden's view is gone: {}", stored(&e.f));
}
