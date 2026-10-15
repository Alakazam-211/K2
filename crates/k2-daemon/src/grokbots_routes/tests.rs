use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::external_agents::grokbot::fixture::{self, FakeGateway, FakeSandData, FAKE_TOKEN};
use k2_core::external_agents::settings::{self, LocalGrokBotsSettings};
use serde_json::{json, Value};

use super::*;
use crate::test_support::TempHome;

const A: &str = "0e5f5de8-7619-4ba4-9753-32c5470b2346";

fn body(r: &CliResponse) -> Value {
    serde_json::from_str(&r.body).expect("json body")
}

fn get(path: &str, who: Who, q: &[(&str, &str)]) -> CliResponse {
    let params: HashMap<String, String> = q
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    handle(path, false, who, b"", &params)
}

fn post(path: &str, who: Who, b: Value) -> CliResponse {
    handle(path, true, who, b.to_string().as_bytes(), &HashMap::new())
}

fn login(role: Role) -> Who {
    Who::Login {
        username: "pat".into(),
        role,
    }
}

fn agent() -> Who {
    Who::Agent {
        project_id: "p1".into(),
        session_id: "s1".into(),
    }
}

fn shell() -> Who {
    Who::Shell {
        project_id: "p1".into(),
        session_id: "s1".into(),
    }
}

struct Env {
    gw: FakeGateway,
    sand: FakeSandData,
    _home: TempHome,
}

fn env() -> Env {
    let gw = FakeGateway::start(json!([fixture::list_item(A, "Cortana")]));
    let home = TempHome::new();
    let sand = FakeSandData::create(home.path(), gw.port);
    sand.add_agent(A, "Cortana", "Chief of staff");
    sand.add_sample_chat(A);
    Env {
        gw,
        sand,
        _home: home,
    }
}

fn turn_on(show: bool, send: bool) {
    settings::save(&LocalGrokBotsSettings { show, send }).expect("save setting");
}

#[test]
fn off_by_default_reads_nothing() {
    let e = env();
    let r = get("/cli/grokbots/list", Who::Owner, &[]);
    assert_eq!(r.status, "200 OK");
    let b = body(&r);
    assert_eq!(b["enabled"], json!(false));
    assert_eq!(b["agents"], json!([]));
    let r = get("/cli/grokbots/status", Who::Owner, &[]);
    assert_eq!(body(&r)["enabled"], json!(false));
    let r = get("/cli/grokbots/thread", Who::Owner, &[("agent", A)]);
    assert_eq!(r.status, "403 Forbidden");
    assert_eq!(body(&r)["error"], json!("grokbots_off"));
    let r = post(
        "/cli/grokbots/send",
        Who::Owner,
        json!({"agent": A, "text": "hi"}),
    );
    assert_eq!(body(&r)["error"], json!("grokbots_off"));
    assert!(
        fixture::opened_under(&e.sand.root).is_empty(),
        "nothing under sand-data opened"
    );
    assert!(e.gw.seen().is_empty(), "the gateway was never called");
    let s = body(&get("/cli/grokbots/settings", Who::Owner, &[]));
    assert_eq!(
        (s["show"].clone(), s["send"].clone(), s["canEdit"].clone()),
        (json!(false), json!(false), json!(true))
    );
}

#[test]
fn detect_previews_path_and_names_for_admins_only() {
    let e = env();
    let r = get("/cli/grokbots/detect", Who::Owner, &[]);
    let b = body(&r);
    assert_eq!(b["installed"], json!(true));
    assert_eq!(b["dataPath"], json!(e.sand.root.display().to_string()));
    assert_eq!(b["agents"][0]["name"], json!("Cortana"));
    assert!(e.gw.seen().is_empty(), "detect uses no token / no network");
    assert_eq!(
        get("/cli/grokbots/detect", login(Role::Member), &[]).status,
        "403 Forbidden"
    );
    assert_eq!(
        get("/cli/grokbots/detect", login(Role::Admin), &[]).status,
        "200 OK"
    );
}

#[test]
fn show_on_lists_with_tag_status_and_reads_thread() {
    let e = env();
    turn_on(true, false);
    let b = body(&get("/cli/grokbots/list", login(Role::Member), &[]));
    assert_eq!(b["enabled"], json!(true));
    assert_eq!(b["sendEnabled"], json!(false));
    assert_eq!(b["status"]["reachable"], json!(true));
    let a = &b["agents"][0];
    assert_eq!(
        (a["id"].clone(), a["kind"].clone(), a["name"].clone()),
        (json!(A), json!("grokbot"), json!("Cortana"))
    );
    let t = body(&get(
        "/cli/grokbots/thread",
        login(Role::Member),
        &[("agent", A), ("limit", "50")],
    ));
    assert_eq!(t["supported"], json!(true));
    assert_eq!(t["messages"][0]["author"], json!("human"));
    assert_eq!(t["messages"][0]["text"], json!("Can you check the build?"));
    let since = t["nextSince"].as_i64().expect("nextSince");
    e.sand.add_entry(
        A,
        json!({"kind":"send-message","message":{"type":"text","content":"later"}}),
    );
    let t2 = body(&get(
        "/cli/grokbots/thread",
        Who::Owner,
        &[("agent", A), ("since", &since.to_string())],
    ));
    assert_eq!(t2["messages"].as_array().expect("arr").len(), 1);
    assert_eq!(t2["messages"][0]["text"], json!("later"));
    let bad = get(
        "/cli/grokbots/thread",
        Who::Owner,
        &[("agent", A), ("since", "x")],
    );
    assert_eq!(bad.status, "400 Bad Request");
}

#[test]
fn send_needs_its_own_switch_and_an_admin() {
    // Its own agent id: the send limiter is process-wide.
    const A: &str = "c0ffee00-0000-4000-8000-0000000000c1";
    let e = env();
    e.sand.add_agent(A, "Solo", "x");
    e.gw.set(|s| s.agents = json!([fixture::list_item(A, "Solo")]));
    turn_on(true, false);
    let r = post(
        "/cli/grokbots/send",
        Who::Owner,
        json!({"agent": A, "text": "hi"}),
    );
    assert_eq!(r.status, "403 Forbidden");
    assert_eq!(body(&r)["error"], json!("grokbots_send_off"));
    turn_on(false, true);
    assert_eq!(
        body(&post(
            "/cli/grokbots/send",
            Who::Owner,
            json!({"agent": A, "text": "hi"})
        ))["error"],
        json!("grokbots_off")
    );
    turn_on(true, true);
    let r = post(
        "/cli/grokbots/send",
        login(Role::Member),
        json!({"agent": A, "text": "hi"}),
    );
    assert_eq!(body(&r)["error"], json!("grokbots_admin_only"));
    assert!(e.gw.seen().iter().all(|r| r.path != "/api/sendPrompt"));
    let r = post(
        "/cli/grokbots/send",
        Who::Owner,
        json!({"agent": A, "text": "hello"}),
    );
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let nonce = body(&r)["clientNonce"].as_str().expect("nonce").to_string();
    let sent: Vec<_> =
        e.gw.seen()
            .into_iter()
            .filter(|r| r.path == "/api/sendPrompt")
            .collect();
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0].body,
        json!({"prompt":"hello","agentId":A,"clientNonce":nonce})
    );
    assert_eq!(
        sent[0].authorization.as_deref(),
        Some(format!("Bearer {FAKE_TOKEN}").as_str())
    );
    // rate limit: an immediate second send to the same agent is refused
    let r = post(
        "/cli/grokbots/send",
        Who::Owner,
        json!({"agent": A, "text": "again"}),
    );
    assert_eq!(r.status, "429 Too Many Requests");
    assert!(body(&r)["retryAfterMs"].as_i64().expect("retry") > 0);
}

#[test]
fn agents_app_guests_and_keys_are_refused_everywhere() {
    let e = env();
    turn_on(true, true);
    for who in [agent(), shell(), Who::Wrong, Who::Invalid] {
        for path in GET_ROUTES {
            let r = get(path, who.clone(), &[("agent", A)]);
            assert_eq!(r.status, "403 Forbidden", "{path} {who:?}");
            assert_eq!(body(&r)["error"], json!("grokbots_humans_only"));
        }
        for path in POST_ROUTES {
            let r = post(
                path,
                who.clone(),
                json!({"agent": A, "text": "x", "show": true}),
            );
            assert_eq!(
                body(&r)["error"],
                json!("grokbots_humans_only"),
                "{path} {who:?}"
            );
        }
    }
    assert!(
        e.gw.seen().iter().all(|r| r.path != "/api/sendPrompt"),
        "no send happened"
    );
}

#[test]
fn settings_write_is_admin_only_and_round_trips() {
    let _e = env();
    let r = post(
        "/cli/grokbots/settings",
        login(Role::Member),
        json!({"show": true}),
    );
    assert_eq!(body(&r)["error"], json!("grokbots_admin_only"));
    assert!(!settings::load().show);
    let r = post(
        "/cli/grokbots/settings",
        login(Role::Admin),
        json!({"show": true}),
    );
    assert_eq!(body(&r)["show"], json!(true));
    assert_eq!(body(&r)["send"], json!(false));
    let r = post("/cli/grokbots/settings", Who::Owner, json!({"send": true}));
    assert_eq!(
        (body(&r)["show"].clone(), body(&r)["send"].clone()),
        (json!(true), json!(true))
    );
    let bad = post(
        "/cli/grokbots/settings",
        Who::Owner,
        json!({"show": true, "token": "x"}),
    );
    assert_eq!(bad.status, "400 Bad Request");
    let s = body(&get("/cli/grokbots/settings", login(Role::Member), &[]));
    assert_eq!(s["canEdit"], json!(false));
}

#[test]
fn the_token_never_appears_in_any_response() {
    let _e = env();
    turn_on(true, true);
    let mut bodies = Vec::new();
    for path in GET_ROUTES {
        bodies.push(get(path, Who::Owner, &[("agent", A)]).body);
    }
    bodies.push(
        post(
            "/cli/grokbots/send",
            Who::Owner,
            json!({"agent": A, "text": "x"}),
        )
        .body,
    );
    bodies.push(
        post(
            "/cli/grokbots/send",
            Who::Owner,
            json!({"agent": "ffffffff-0000-4000-8000-00000000000f", "text": "x"}),
        )
        .body,
    );
    for b in bodies {
        assert!(!b.contains(FAKE_TOKEN), "token leaked: {b}");
    }
}

#[test]
fn unknown_layout_is_a_soft_200() {
    let e = env();
    turn_on(true, false);
    std::fs::write(e.sand.agent_dir(A).join("store.db"), b"not sqlite").expect("w");
    let _ = std::fs::remove_file(e.sand.agent_dir(A).join("store.db-wal"));
    let _ = std::fs::remove_file(e.sand.agent_dir(A).join("store.db-shm"));
    let r = get("/cli/grokbots/thread", Who::Owner, &[("agent", A)]);
    assert_eq!(r.status, "200 OK");
    assert_eq!(body(&r)["supported"], json!(false));
    // and the agent still lists
    assert_eq!(
        body(&get("/cli/grokbots/list", Who::Owner, &[]))["agents"][0]["id"],
        json!(A)
    );
}
