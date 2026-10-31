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

    crate::sim_routes::clear_lanes_for_tests();
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

// ── licences: every one up front, a failed setup, retry ─────────────

/// A fresh sim home and a fake Android lane that still needs setup, with
/// these licences known up front and these only the install finds.
fn lane_needing_setup(tag: &str, known: &[(&str, &str)], hidden: &[(&str, &str)]) -> (std::path::PathBuf, Arc<k2_core::sim::fake::FakeLane>, String) {
    let tmp = std::env::temp_dir().join(format!("k2-sim-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("ws")).expect("ws");
    k2_core::sim::set_base_for_tests(tmp.join("home"));
    k2_core::sim::limits::save_settings(&k2_core::sim::limits::SimSettings {
        max_devices: Some(2),
        min_free_ram_bytes: Some(0),
        ..Default::default()
    })
    .expect("limits");
    k2_core::sim::clear_lanes();
    let fake = Arc::new(k2_core::sim::fake::FakeLane::new(LaneKind::Android));
    fake.ready.store(false, std::sync::atomic::Ordering::SeqCst);
    *fake.licences.lock() = known.iter().map(|(i, t)| (i.to_string(), t.to_string())).collect();
    *fake.hidden_licences.lock() = hidden.iter().map(|(i, t)| (i.to_string(), t.to_string())).collect();
    k2_core::sim::register_lane(fake.clone());
    manager::manager().refresh_probes();
    let ws = insert_project(&format!("sim-{tag}"), &tmp.join("ws"));
    (tmp, fake, ws)
}

fn android_lane(st: &Value) -> Value {
    st["lanes"]
        .as_array()
        .expect("lanes")
        .iter()
        .find(|l| l["lane"] == json!("android"))
        .cloned()
        .unwrap_or_else(|| panic!("no android lane in {st}"))
}

fn licence_ids(lane: &Value) -> Vec<String> {
    lane["needs"]
        .as_array()
        .map(|a| a.iter().filter(|n| n["kind"] == json!("licence")).map(|n| n["id"].as_str().expect("id").to_string()).collect())
        .unwrap_or_default()
}

fn licence_sha(id: &str) -> String {
    let r = get(LICENCE, Caller::Owner, &[("lane", "android"), ("id", id)]);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    body(&r)["sha1"].as_str().expect("sha1").to_string()
}

/// Wait for the owner's setup to stop running; the lane as status shows it.
fn wait_setup(ws: &str) -> Value {
    let t0 = std::time::Instant::now();
    loop {
        let st = body(&get(STATUS, Caller::Owner, &[("workspace", ws)]));
        let lane = android_lane(&st);
        let installing = lane["detail"].as_str().is_some_and(|d| d.starts_with("installing"));
        if !installing {
            return lane;
        }
        assert!(t0.elapsed() < std::time::Duration::from_secs(20), "setup still running after 20 s: {lane}");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn recorded() -> Vec<(String, String, String)> {
    k2_core::sim::licence::load().into_iter().map(|a| (a.id, a.sha1, a.by)).collect()
}

fn installs(fake: &k2_core::sim::fake::FakeLane) -> u64 {
    fake.installs.load(std::sync::atomic::Ordering::SeqCst)
}

/// Two licences: the status lists both before any download, one setup
/// call accepts both (all or nothing), and setup runs to ready.
#[test]
fn one_acceptance_covers_every_licence_the_setup_needs() {
    let _serial = sim_test_lock();
    let (tmp, fake, ws) =
        lane_needing_setup("two-licences", &[("fake-license", "Fake SDK licence"), ("fake-arm-license", "Fake ARM licence")], &[]);

    let lane = android_lane(&body(&get(STATUS, Caller::Owner, &[("workspace", &ws), ("refresh", "1")])));
    assert_eq!(licence_ids(&lane), vec!["fake-license", "fake-arm-license"], "both, before anything downloads: {lane}");
    let (a, b) = (licence_sha("fake-license"), licence_sha("fake-arm-license"));

    // One of the two: refused naming the other; nothing recorded or downloaded.
    let r = post(SETUP, Caller::Owner, json!({ "lane": "android", "accept": [{ "id": "fake-license", "sha1": a }] }));
    assert_eq!(code(&r), "sim_needs_setup", "{}", r.body);
    assert_eq!(body(&r)["exit"], json!(3));
    assert!(body(&r)["message"].as_str().expect("message").contains("Fake ARM licence"), "names the missing one: {}", r.body);
    assert_eq!(recorded(), vec![], "all or nothing");
    assert_eq!(installs(&fake), 0, "no download before every licence is accepted");

    // A stale text for one of them: refused, nothing recorded.
    let r = post(
        SETUP,
        Caller::Owner,
        json!({ "lane": "android", "accept": [{ "id": "fake-license", "sha1": a }, { "id": "fake-arm-license", "sha1": "0000" }] }),
    );
    assert_eq!(code(&r), "sim_licence_changed", "{}", r.body);
    assert_eq!(recorded(), vec![]);

    // Both: one call, one setup, ready.
    let r = post(
        SETUP,
        Caller::Owner,
        json!({ "lane": "android", "accept": [{ "id": "fake-license", "sha1": a }, { "id": "fake-arm-license", "sha1": b }] }),
    );
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let lane = wait_setup(&ws);
    assert_eq!(lane["ready"], json!(true), "{lane}");
    let mut got = recorded();
    got.sort();
    assert_eq!(
        got,
        vec![
            ("fake-arm-license".to_string(), b.clone(), "owner".to_string()),
            ("fake-license".to_string(), a.clone(), "owner".to_string())
        ]
    );
    assert_eq!(installs(&fake), 1);
    let r = post(BOOT, Caller::Owner, json!({ "workspace": ws, "noTab": true }));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let _ = post(STOP, Caller::Owner, json!({ "workspace": ws }));

    crate::sim_routes::clear_lanes_for_tests();
    let _ = std::fs::remove_dir_all(&tmp);
}

/// The owner's card lists every pending licence with its text, and its one
/// Accept records them all.
#[test]
fn the_licence_card_shows_every_licence_and_one_accept_covers_them() {
    let _serial = sim_test_lock();
    let (tmp, _fake, ws) = lane_needing_setup("card", &[("fake-license", "Fake SDK licence"), ("fake-arm-license", "Fake ARM licence")], &[]);
    let (a, b) = (licence_sha("fake-license"), licence_sha("fake-arm-license"));

    let card = licence_card(LaneKind::Android, "sim-card").expect("a card");
    for (id, title, sha) in [("fake-license", "Fake SDK licence", &a), ("fake-arm-license", "Fake ARM licence", &b)] {
        assert!(card.prompt.contains(title), "{title} on the card: {}", card.prompt);
        assert!(card.prompt.contains(&sha[..12]), "{id}'s sha1 on the card");
        assert!(card.prompt.contains(&format!("the terms of {id}")), "{id}'s text on the card");
    }
    assert_eq!(card.params["licences"], json!([{ "id": "fake-license", "sha1": a }, { "id": "fake-arm-license", "sha1": b }]));

    // Not the owner: refused, nothing recorded.
    assert!(run_card(&card.params, CARD_ACCEPT, "someone-who-is-not-owner").is_err());
    assert_eq!(recorded(), vec![]);
    // The owner's one tap: both recorded, setup runs.
    let out = run_card(&card.params, CARD_ACCEPT, "owner").expect("accepted");
    assert_eq!(out["ok"], json!(true), "{out}");
    let lane = wait_setup(&ws);
    assert_eq!(lane["ready"], json!(true), "{lane}");
    assert_eq!(recorded().len(), 2);

    crate::sim_routes::clear_lanes_for_tests();
    let _ = std::fs::remove_dir_all(&tmp);
}

/// The owner's stuck box: the first licence was accepted, the setup then
/// failed on a licence nobody was asked for. The next status names that
/// licence and the failure, the next Start asks for it (409 with the
/// need), and accepting it retries the setup to ready.
#[test]
fn a_setup_that_failed_on_a_licence_asks_for_it_and_accepting_retries() {
    let _serial = sim_test_lock();
    let (tmp, fake, ws) =
        lane_needing_setup("failed-licence", &[("fake-license", "Fake SDK licence")], &[("fake-arm-license", "Fake ARM licence")]);
    let a = licence_sha("fake-license");
    let r = post(SETUP, Caller::Owner, json!({ "lane": "android", "acceptLicence": "fake-license", "licenceSha1": a }));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let lane = wait_setup(&ws);
    assert_eq!(lane["ready"], json!(false), "{lane}");
    assert_eq!(licence_ids(&lane), vec!["fake-arm-license"], "the licence the setup failed on: {lane}");
    assert!(lane["detail"].as_str().expect("detail").contains("the last setup failed"), "{lane}");
    assert!(lane["setup"]["error"].as_str().expect("setup.error").contains("fake-arm-license"), "{lane}");

    // The next Start: 409 with that licence as the need (the card's input).
    let r = post(BOOT, Caller::Owner, json!({ "workspace": ws, "noTab": true }));
    assert_eq!(code(&r), "sim_needs_setup", "{}", r.body);
    assert!(
        body(&r)["needs"].as_array().expect("needs").iter().any(|n| n["kind"] == json!("licence") && n["id"] == json!("fake-arm-license")),
        "{}",
        r.body
    );
    let card = licence_card(LaneKind::Android, "sim-failed-licence").expect("a card for the missing one");
    assert_eq!(card.params["licences"].as_array().map(|v| v.len()), Some(1), "only the missing one: {}", card.params);

    // Accept it: the setup runs again and finishes.
    let b = licence_sha("fake-arm-license");
    let r = post(SETUP, Caller::Owner, json!({ "lane": "android", "accept": [{ "id": "fake-arm-license", "sha1": b }] }));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let lane = wait_setup(&ws);
    assert_eq!(lane["ready"], json!(true), "{lane}");
    assert!(lane.get("setup").is_none(), "no failure left: {lane}");
    assert_eq!(installs(&fake), 2);
    let r = post(BOOT, Caller::Owner, json!({ "workspace": ws, "noTab": true }));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let _ = post(STOP, Caller::Owner, json!({ "workspace": ws }));

    crate::sim_routes::clear_lanes_for_tests();
    let _ = std::fs::remove_dir_all(&tmp);
}

/// A setup that failed for another reason (a broken download): status
/// says so in `setup.error`, and Retry setup (`POST setup` with no
/// licence) runs it again.
#[test]
fn a_failed_setup_is_reported_and_retry_runs_it_again() {
    let _serial = sim_test_lock();
    let (tmp, fake, ws) = lane_needing_setup("retry", &[("fake-license", "Fake SDK licence")], &[]);
    fake.fail_install.store(true, std::sync::atomic::Ordering::SeqCst);
    let a = licence_sha("fake-license");
    let r = post(SETUP, Caller::Owner, json!({ "lane": "android", "accept": [{ "id": "fake-license", "sha1": a }] }));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let lane = wait_setup(&ws);
    assert_eq!(lane["ready"], json!(false), "{lane}");
    assert_eq!(licence_ids(&lane), Vec::<String>::new(), "no licence is missing: {lane}");
    assert!(lane["setup"]["error"].as_str().expect("setup.error").contains("connection reset"), "{lane}");
    assert_eq!(lane["setup"]["running"], json!(false));

    fake.fail_install.store(false, std::sync::atomic::Ordering::SeqCst);
    let r = post(SETUP, Caller::Owner, json!({ "lane": "android" }));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let lane = wait_setup(&ws);
    assert_eq!(lane["ready"], json!(true), "{lane}");
    assert_eq!(installs(&fake), 2);

    crate::sim_routes::clear_lanes_for_tests();
    let _ = std::fs::remove_dir_all(&tmp);
}

/// Accepting stays the box owner's, by person: agents and non-owners are
/// refused on every form (route list, legacy fields, the card), nothing is
/// recorded; status tells the tab who may set up.
#[test]
fn only_the_box_owner_accepts_licences_and_status_says_who_can() {
    let _serial = sim_test_lock();
    let (tmp, fake, ws) =
        lane_needing_setup("owner-only", &[("fake-license", "Fake SDK licence"), ("fake-arm-license", "Fake ARM licence")], &[]);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::sim::store::set_agents_can_use_sim(&conn, &ws, true).expect("switch on");
    }
    let (a, b) = (licence_sha("fake-license"), licence_sha("fake-arm-license"));
    let both = json!({ "lane": "android", "accept": [{ "id": "fake-license", "sha1": a }, { "id": "fake-arm-license", "sha1": b }] });

    let r = post(SETUP, agent(&ws), both.clone());
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    let r = post(SETUP, Caller::Shell { project_id: ws.clone() }, both.clone());
    assert_eq!(code(&r), "owner_only", "{}", r.body);
    let params = json!({ "lane": "android", "licences": [{ "id": "fake-license", "sha1": a }, { "id": "fake-arm-license", "sha1": b }] });
    assert!(run_card(&params, CARD_ACCEPT, "agent:one").is_err());
    assert!(run_card(&json!({ "lane": "android", "licence": "fake-license", "sha1": a }), CARD_ACCEPT, "nobody").is_err());
    assert_eq!(recorded(), vec![], "nothing recorded");
    assert_eq!(installs(&fake), 0);

    assert_eq!(body(&get(STATUS, Caller::Owner, &[("workspace", &ws)]))["canSetup"], json!(true));
    assert_eq!(body(&get(STATUS, Caller::Shell { project_id: ws.clone() }, &[("workspace", &ws)]))["canSetup"], json!(false));
    assert_eq!(body(&get(STATUS, agent(&ws), &[]))["canSetup"], json!(false));

    crate::sim_routes::clear_lanes_for_tests();
    let _ = std::fs::remove_dir_all(&tmp);
}
