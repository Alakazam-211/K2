//! `/cli/sim/*` gates (vs-live "Tests" 1, 10): 405s, App passes, other
//! workspaces, the agent switch, people-only routes, the owner-only
//! licence, plus one end-to-end pass on a fake lane.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};

use super::*;
use crate::sidecar_routes::AgentCaller;

fn body(r: &CliResponse) -> Value {
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("not JSON ({e}): {}", r.body))
}

fn code(r: &CliResponse) -> String {
    body(r)["code"].as_str().map(str::to_string).unwrap_or_else(|| panic!("no code in {}", r.body))
}

fn agent(project_id: &str) -> Caller {
    Caller::Agent(AgentCaller { project_id: project_id.to_string(), map_key: None, args: vec![], sandboxed: false })
}

fn insert_project(name: &str, path: &std::path::Path) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)", rusqlite::params![id, name, path.display().to_string()])
        .expect("insert project row");
    id
}

fn post(path: &str, caller: Caller, b: Value) -> CliResponse {
    handle(path, caller, true, b.to_string().as_bytes(), &HashMap::new())
}

fn get(path: &str, caller: Caller, q: &[(&str, &str)]) -> CliResponse {
    let params: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    handle(path, caller, false, b"", &params)
}

#[test]
fn every_post_route_answers_405_to_a_get_and_reads_405_to_a_post() {
    for p in POST_ROUTES {
        let r = handle(p, Caller::Owner, false, b"", &HashMap::new());
        assert_eq!(r.status, "405 Method Not Allowed", "{p}");
    }
    for p in READ_ROUTES {
        let r = handle(p, Caller::Owner, true, b"{}", &HashMap::new());
        assert_eq!(r.status, "405 Method Not Allowed", "{p}");
    }
}

#[test]
fn app_passes_and_bad_tokens_are_refused_everywhere() {
    for p in READ_ROUTES.iter().chain(POST_ROUTES).chain(BOTH_ROUTES) {
        let is_post = POST_ROUTES.contains(p);
        let r = handle(p, Caller::WrongCredential, is_post, b"{}", &HashMap::new());
        assert_eq!(r.status, "403 Forbidden", "{p}: {}", r.body);
        assert_eq!(code(&r), "wrong_credential", "{p}");
        let r = handle(p, Caller::Invalid, is_post, b"{}", &HashMap::new());
        assert_eq!(r.status, "403 Forbidden", "{p}");
    }
}

#[test]
fn agent_verbs_are_exactly_the_agent_routes() {
    for p in AGENT_ROUTES {
        assert!(crate::session_token::is_agent_verb(p), "{p} should be an agent verb");
    }
    for p in [HOLD, SETUP, SETTINGS, LICENCE, STREAM] {
        assert!(!crate::session_token::is_agent_verb(p), "{p} is for people only");
    }
    for p in READ_ROUTES.iter().chain(POST_ROUTES).chain(BOTH_ROUTES).chain(std::iter::once(&STREAM)) {
        assert!(crate::routes::route_policy::lookup(p).is_some(), "{p} has no route_policy row");
    }
    assert_eq!(
        crate::routes::route_policy::lookup(SETUP).and_then(|r| r.post),
        Some(crate::routes::route_policy::Floor::Owner),
        "only the owner accepts a licence"
    );
}

/// One sequential pass: the lane registry and the manager are process-wide.
#[test]
fn sim_routes_end_to_end_on_a_fake_lane() {
    let _serial = sim_test_lock();
    let tmp = std::env::temp_dir().join(format!("k2-sim-routes-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("mine")).expect("ws");
    std::fs::create_dir_all(tmp.join("other")).expect("ws");
    k2_core::sim::set_base_for_tests(tmp.join("home"));
    k2_core::sim::limits::save_settings(&k2_core::sim::limits::SimSettings {
        max_devices: Some(2),
        min_free_ram_bytes: Some(0),
        ..Default::default()
    })
    .expect("limits");
    let fake = Arc::new(k2_core::sim::fake::FakeLane::new(LaneKind::Android));
    k2_core::sim::register_lane(fake.clone());
    manager::manager().refresh_probes();
    let mine = insert_project("sim-mine", &tmp.join("mine"));
    let other = insert_project("sim-other", &tmp.join("other"));

    // The switch is off: status works (so the agent can see why), verbs
    // exit 3 with the Settings row.
    let r = get(STATUS, agent(&mine), &[]);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(body(&r)["agentsAllowed"], json!(false));
    assert_eq!(body(&r)["canControl"], json!(false), "agents drive through verbs, not the tab");
    // B's CLI sends the folder path as `workspace`.
    let folder = tmp.join("mine").display().to_string();
    let r = get(STATUS, Caller::Owner, &[("workspace", &folder)]);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(body(&r)["canControl"], json!(true));
    let r = post(BOOT, agent(&mine), json!({}));
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    assert_eq!(code(&r), "sim_switch_off");
    assert_eq!(body(&r)["exit"], json!(3));
    assert_eq!(body(&r)["error"]["code"], json!("sim_switch_off"), "the permission wall matches error.code");
    assert!(body(&r)["message"].as_str().expect("message").contains("Allow mobile devices"));
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::sim::store::set_agents_can_use_sim(&conn, &mine, true).expect("switch on");
    }

    // An agent may not name another workspace.
    let r = post(BOOT, agent(&mine), json!({ "workspace": other }));
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    let r = get(TREE, agent(&mine), &[("workspace", &other)]);
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);

    // People-only routes refuse agents.
    for (p, b) in [(HOLD, json!({ "action": "take-over" })), (SETUP, json!({ "lane": "android" })), (SETTINGS, json!({}))] {
        let r = post(p, agent(&mine), b);
        assert_eq!(r.status, "403 Forbidden", "{p}: {}", r.body);
    }

    // Owner-only licence setup: an Admin-less caller gets owner_only (exit 3).
    let r = post(SETUP, Caller::Shell { project_id: mine.clone() }, json!({ "lane": "android" }));
    assert_eq!(code(&r), "owner_only", "{}", r.body);
    assert_eq!(body(&r)["exit"], json!(3));

    // Boot → tree → act → stop, as the agent on its own workspace.
    let r = post(BOOT, agent(&mine), json!({ "noTab": true }));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v = body(&r);
    assert_eq!(v["state"], json!("running"));
    assert_eq!(v["lane"], json!("android"));
    let r = post(ACT, agent(&mine), json!({ "action": { "verb": "tap", "element": "e3" } }));
    assert_eq!(code(&r), "sim_stale_element", "no snapshot yet: {}", r.body);
    let r = get(TREE, agent(&mine), &[]);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let t = body(&r);
    let save = t["snapshot"]["elements"]
        .as_array()
        .expect("elements")
        .iter()
        .find(|e| e["label"] == json!("Save"))
        .and_then(|e| e["id"].as_str())
        .expect("Save element")
        .to_string();
    let r = post(ACT, agent(&mine), json!({ "action": { "verb": "tap", "element": save } }));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert!(body(&r)["line"].as_str().expect("line").contains("text \"Saved\""), "{}", r.body);

    // A person takes over; the agent is held (exit 2).
    let r = post(HOLD, Caller::Owner, json!({ "workspace": mine, "action": "take-over" }));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(body(&r)["hold"]["kind"], json!("takeover"));
    let r = post(ACT, agent(&mine), json!({ "action": { "verb": "button", "button": "home" } }));
    assert_eq!(code(&r), "sim_held", "{}", r.body);
    assert_eq!(body(&r)["exit"], json!(2));
    let r = post(HOLD, Caller::Owner, json!({ "workspace": mine, "action": "hand-back" }));
    assert_eq!(r.status, "200 OK", "{}", r.body);

    // `type --secret`: people type their own; an agent's unknown secret is
    // the vault's refusal, never a typed value.
    let r = post(ACT, Caller::Owner, json!({ "workspace": mine, "action": { "verb": "type", "secret": "pw" } }));
    assert_eq!(code(&r), "sim_invalid", "{}", r.body);
    let typed_before = fake.actions.lock().iter().filter(|a| a.starts_with("type")).count();
    let r = post(ACT, agent(&mine), json!({ "action": { "verb": "type", "secret": "no-such-secret" } }));
    assert_ne!(r.status, "200 OK", "{}", r.body);
    assert!(body(&r)["message"].as_str().expect("message").starts_with("vault: "), "{}", r.body);
    assert_eq!(fake.actions.lock().iter().filter(|a| a.starts_with("type")).count(), typed_before, "nothing was typed");

    // Settings: a person reads; the owner writes; bad values are refused.
    let r = get(SETTINGS, Caller::Owner, &[]);
    assert_eq!(body(&r)["effective"]["maxDevices"], json!(2), "{}", r.body);
    let r = post(SETTINGS, Caller::Owner, json!({ "maxDevices": 0 }));
    assert_eq!(r.status, "400 Bad Request", "{}", r.body);

    let r = post(STOP, agent(&mine), json!({}));
    assert_eq!(body(&r)["state"], json!("stopped"), "{}", r.body);
    let r = post(ERASE, Caller::Owner, json!({ "workspace": mine }));
    assert_eq!(body(&r)["erased"], json!(true), "{}", r.body);

    // The card: only the box owner's answer counts.
    let params = json!({ "lane": "android", "licence": "fake-license", "sha1": "x" });
    assert!(run_card(&params, CARD_ACCEPT, "someone-who-is-not-owner").is_err());
    assert_eq!(run_card(&params, CARD_NOT_NOW, "anyone").expect("not now")["ok"], json!(true));

    k2_core::sim::clear_lanes();
    let _ = std::fs::remove_dir_all(&tmp);
}

/// An unknown platform and an iPhone on a server with no iOS lane each get
/// their own code and a plain message, never a bare 400 (the Mobile tab
/// only offers the lanes `status` reports; this is the server's half).
#[test]
fn an_unknown_or_missing_lane_is_named_not_a_bare_400() {
    let _serial = sim_test_lock();
    let tmp = std::env::temp_dir().join(format!("k2-sim-lanes-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("ws")).expect("ws");
    k2_core::sim::set_base_for_tests(tmp.join("home"));
    k2_core::sim::clear_lanes();
    k2_core::sim::register_lane(Arc::new(k2_core::sim::fake::FakeLane::new(LaneKind::Android)));
    manager::manager().refresh_probes();
    let ws = insert_project("sim-lanes", &tmp.join("ws"));

    // A Linux box: Android only. The status lists no iOS lane...
    let st = body(&get(STATUS, Caller::Owner, &[("workspace", &ws)]));
    let lanes: Vec<&str> = st["lanes"].as_array().expect("lanes").iter().map(|l| l["lane"].as_str().expect("lane")).collect();
    assert_eq!(lanes, vec!["android"], "{st}");
    // ...and asking for an iPhone (by platform or by lane) says what it needs.
    for platform in ["ios", "ios-node"] {
        let r = get(DEVICES, Caller::Owner, &[("workspace", &ws), ("platform", platform)]);
        assert_eq!(r.status, "409 Conflict", "{platform}: {}", r.body);
        assert_eq!(code(&r), "sim_lane_unavailable", "{platform}");
        assert_eq!(body(&r)["message"], json!("iPhone needs a Mac (this computer or a Mac compute node)"), "{platform}");
        let r = post(BOOT, Caller::Owner, json!({ "workspace": ws, "platform": platform }));
        assert_eq!((r.status, code(&r).as_str()), ("409 Conflict", "sim_lane_unavailable"), "{platform}: {}", r.body);
    }
    // A name that is no lane at all: its own code, naming the choices.
    let r = get(DEVICES, Caller::Owner, &[("workspace", &ws), ("platform", "windows-phone")]);
    assert_eq!(r.status, "400 Bad Request", "{}", r.body);
    assert_eq!(code(&r), "sim_lane_unknown");
    assert!(body(&r)["message"].as_str().expect("message").contains("use android or ios"), "{}", r.body);
    // Android still answers.
    let r = get(DEVICES, Caller::Owner, &[("workspace", &ws), ("platform", "android")]);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(body(&r)["lane"], json!("android"));

    k2_core::sim::clear_lanes();
    let _ = std::fs::remove_dir_all(&tmp);
}
