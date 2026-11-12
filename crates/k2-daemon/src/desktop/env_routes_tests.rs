//! `POST /cli/desktop/env` (prd-linux-virtual-desktop-v1 L8, §7's "fails
//! first" list): headless, a fake manager (fake runner and clock), real
//! store. The two-desktops input check against a real Xvfb is in
//! `virtual_desktop_tests.rs` (`xvfb::two_agents_desktops_keep_to_themselves`).

use std::collections::HashMap;
use std::sync::Arc;

use k2_core::connect_users::{self, Role};
use k2_core::desktop::audit;
use k2_core::desktop::virtual_desktop as vd;
use k2_core::desktop::{store, MachineState};
use k2_core::desktop::wire::Display;
use serde_json::{json, Value};

use super::*;
use crate::compute_routes::Who;
use crate::desktop::deskd::{self, Deskd};
use crate::desktop::routes::{handle, platform_for_test, PlatformGuard, Req};
use crate::desktop::virtual_desktop::tests::Rig;
use crate::desktop::virtual_desktop::{self as vdm, TestManager};

struct FakeScreen;

impl Deskd for FakeScreen {
    fn backend(&self) -> &'static str {
        "linux"
    }
    fn state(&self) -> MachineState {
        MachineState::Ready
    }
    fn enable(&self) -> Result<(), String> {
        Ok(())
    }
    fn disable(&self) -> Result<(), String> {
        Ok(())
    }
    fn displays(&self) -> Vec<Display> {
        Vec::new()
    }
}

/// A Linux server with no screen, the virtual switch on, workspaces `w1`
/// ("scientist", switch on), `w2` ("atlas", switch on) and `w3` ("quill",
/// switch off).
struct E {
    _home: k2_core::test_env::TempHome,
    _db: k2_core::db::TestDbGuard,
    _p: PlatformGuard,
    rig: Rig,
    _m: TestManager,
    _screens: Vec<deskd::TestDesktopDeskd>,
    _addr: AddressGuard,
}

fn env() -> E {
    let home = k2_core::test_env::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    let p = platform_for_test(true, false);
    let rig = Rig::new(true, true);
    super::super::virtual_routes::install_hooks(&rig.m);
    let m = vdm::set_for_test(rig.m.clone());
    let screens = ["virtual", "virtual:w1", "virtual:w2", "virtual:w3"].iter().map(|id| deskd::set_for_test_desktop(id, Arc::new(FakeScreen))).collect();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("INSERT INTO projects (id, name, path) VALUES ('w1', 'scientist', '/tmp/w1'), ('w2', 'atlas', '/tmp/w2'), ('w3', 'quill', '/tmp/w3')", [])
            .unwrap();
        store::set_agents_can_use_desktop(&conn, "w1", true).unwrap();
        store::set_agents_can_use_desktop(&conn, "w2", true).unwrap();
    }
    store::enable(vd::VIRTUAL, true, "owner").unwrap();
    // Session addresses: s-main is scientist's main session, s-side its
    // sidecar "helper"; s-atlas is atlas's.
    let addr = addresses_for_test(HashMap::from([
        ("s-main".to_string(), "scientist".to_string()),
        ("s-side".to_string(), "scientist/helper".to_string()),
        ("s-atlas".to_string(), "atlas".to_string()),
    ]));
    E { _home: home, _db: db, _p: p, rig, _m: m, _screens: screens, _addr: addr }
}

fn agent(ws: &str, session: &str) -> Who {
    Who::Agent { project_id: ws.into(), session_id: session.into() }
}

fn post(who: &Who, path: &str, body: Value) -> (u16, Value) {
    let raw = body.to_string();
    let params: HashMap<String, String> = HashMap::new();
    let req = Req { path, is_post: true, body: raw.as_bytes(), params: &params, ingress: "cell", ip: "-", token: "" };
    let r = handle(who, &req);
    let code: u16 = r.status.split_whitespace().next().unwrap().parse().unwrap();
    (code, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{path}: {e}: {}", r.body)))
}

fn env_call(who: &Who, body: Value) -> (u16, Value) {
    post(who, "/cli/desktop/env", body)
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or_else(|| panic!("no error code in {v}"))
}

fn audit_lines() -> Vec<Value> {
    audit::read(None, 1000).unwrap()
}

#[test]
fn an_agent_is_refused_with_its_switch_off_and_allowed_with_it_on() {
    let e = env();
    let (s, r) = env_call(&agent("w3", "s-quill"), json!({}));
    assert_eq!((s, code(&r)), (403, "desktop_switch_off"), "{r}");
    assert!(r["error"]["hint"].as_str().unwrap().contains("Allow a virtual desktop"), "{r}");
    assert!(e.rig.runs("systemd-run").is_empty(), "nothing started");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::set_agents_can_use_desktop(&conn, "w3", true).unwrap();
    }
    let (s, r) = env_call(&agent("w3", "s-quill"), json!({}));
    assert_eq!((s, r["state"].as_str()), (200, Some("ready")), "{r}");
    assert_eq!(r["desktop"], "virtual:w3");
}

#[test]
fn every_other_desktop_route_still_refuses_an_agent() {
    let _e = env();
    let a = agent("w1", "s-main");
    for path in super::super::routes::ROUTES.iter().filter(|p| **p != ROUTE) {
        let is_post = super::super::routes::is_post_route(path);
        let params: HashMap<String, String> = HashMap::new();
        let req = Req { path, is_post, body: b"{}", params: &params, ingress: "cell", ip: "-", token: "" };
        let r = handle(&a, &req);
        let v: Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!((r.status, code(&v)), ("403 Forbidden", "desktop_people_only"), "{path}: {v}");
    }
}

#[test]
fn people_and_other_credentials_are_refused_and_get_is_405() {
    let _e = env();
    connect_users::add_user("rosson", "password123").unwrap();
    connect_users::set_role("rosson", Role::Owner).unwrap();
    for who in [Who::Owner, Who::Login { username: "rosson".into(), role: Role::Owner }, Who::Shell { project_id: "w1".into(), session_id: "s-shell".into() }] {
        let (s, r) = env_call(&who, json!({}));
        assert_eq!((s, code(&r)), (403, "desktop_env_agents_only"), "{who:?}: {r}");
    }
    let (s, r) = env_call(&Who::Wrong, json!({}));
    assert_eq!((s, code(&r)), (403, "desktop_env_agents_only"), "{r}");
    let params: HashMap<String, String> = HashMap::new();
    let req = Req { path: ROUTE, is_post: false, body: b"", params: &params, ingress: "cell", ip: "-", token: "" };
    assert_eq!(handle(&agent("w1", "s-main"), &req).status, "405 Method Not Allowed");
}

#[test]
fn the_call_starts_a_closed_desktop_at_1280x720_and_answers_what_to_use() {
    let e = env();
    let (s, r) = env_call(&agent("w1", "s-main"), json!({}));
    assert_eq!(s, 200, "{r}");
    assert_eq!((r["desktop"].as_str(), r["state"].as_str()), (Some("virtual:w1"), Some("ready")), "{r}");
    let x = e.rig.runs("systemd-run").into_iter().find(|l| l.contains("-x.service")).expect("Xvfb");
    assert!(x.contains("-screen 0 1280x720x24"), "{x}");
    let display = r["display"].as_str().expect("a display");
    assert!(display.starts_with(':'), "{r}");
    let home = &e.rig.home;
    let folder = home.join(".k2/desktop/virtual-w1");
    assert_eq!(r["xauthority"], json!(folder.join("Xauthority").display().to_string()));
    assert_eq!(r["profile"], json!(folder.join("profile").display().to_string()));
    assert!(folder.join("profile").is_dir(), "the profile folder exists");
    assert_eq!(r["env"]["DISPLAY"], json!(display));
    assert_eq!(r["env"]["XAUTHORITY"], r["xauthority"]);
    assert_eq!(r["env"]["K2_DESKTOP_PROFILE"], r["profile"]);
    assert_eq!(r["hold"]["by"], "scientist", "{r}");
    assert_eq!(r["hold"]["until"], json!(e.rig.m.now() + vd::HOLD_FOR));
    assert_eq!(r["label"], vd::SEPARATE_LABEL);
    // Audit: the agent started it and holds it.
    let lines = audit_lines();
    let has = |ev: &str, outcome: &str| lines.iter().any(|l| l["event"] == ev && l["outcome"] == outcome && l["who"] == "agent:scientist" && l["desktop"] == "virtual:w1");
    assert!(has("desktop_start", "agent"), "{lines:?}");
    assert!(has("agent_hold", "taken"), "{lines:?}");
}

#[test]
fn the_call_wakes_an_asleep_desktop_and_renews_the_hold() {
    let e = env();
    let a = agent("w1", "s-main");
    env_call(&a, json!({}));
    let (_, r) = env_call(&a, json!({"release": true}));
    assert_eq!(r["released"], true, "{r}");
    e.rig.advance(vd::SLEEP_AFTER + 1);
    e.rig.m.tick();
    assert_eq!(e.rig.m.state("virtual:w1"), vd::State::Asleep);
    let (s, r) = env_call(&a, json!({}));
    assert_eq!((s, r["state"].as_str()), (200, Some("ready")), "woken: {r}");
    assert!(e.rig.host.lines().iter().any(|l| l.contains("thaw")), "by thawing");
    let first = r["hold"]["until"].as_i64().unwrap();
    e.rig.advance(60);
    let (_, r) = env_call(&a, json!({}));
    assert_eq!(r["hold"]["until"].as_i64().unwrap(), first + 60, "renewed: {r}");
    let lines = audit_lines();
    for (ev, outcome) in [("desktop_wake", "agent"), ("agent_hold", "renewed"), ("agent_hold", "released")] {
        assert!(lines.iter().any(|l| l["event"] == ev && l["outcome"] == outcome && l["who"] == "agent:scientist"), "{ev} {outcome}: {lines:?}");
    }
}

#[test]
fn two_agents_get_two_desktops_with_their_own_folders() {
    let e = env();
    let (_, a) = env_call(&agent("w1", "s-main"), json!({}));
    let (_, b) = env_call(&agent("w2", "s-atlas"), json!({}));
    assert_eq!((a["desktop"].as_str(), b["desktop"].as_str()), (Some("virtual:w1"), Some("virtual:w2")));
    assert_ne!(a["display"], b["display"]);
    assert_ne!(a["xauthority"], b["xauthority"]);
    assert_ne!(a["profile"], b["profile"]);
    // No login file is copied or linked between desktop folders: each
    // profile starts empty, a real folder, and nothing in either folder is
    // a link.
    for r in [&a, &b] {
        let p = std::path::PathBuf::from(r["profile"].as_str().unwrap());
        assert_eq!(std::fs::read_dir(&p).unwrap().count(), 0, "{} starts empty", p.display());
        for f in std::fs::read_dir(p.parent().unwrap()).unwrap() {
            let f = f.unwrap();
            assert!(!f.file_type().unwrap().is_symlink(), "{} is a link", f.path().display());
        }
    }
    let _ = e;
}

#[test]
fn naming_another_agents_desktop_or_the_shared_one_is_refused() {
    let e = env();
    for other in ["virtual:w2", "virtual", "console", "virtual:w3"] {
        let (s, r) = env_call(&agent("w1", "s-main"), json!({"desktop": other}));
        assert_eq!((s, code(&r)), (403, "not_your_desktop"), "{other}: {r}");
    }
    let (s, r) = env_call(&agent("w1", "s-main"), json!({"desktop": "virtual:w1"}));
    assert_eq!((s, r["desktop"].as_str()), (200, Some("virtual:w1")), "its own by name is fine: {r}");
    assert_eq!(e.rig.m.state("virtual:w2"), vd::State::Closed, "nothing started for the other");
}

#[test]
fn a_sidecar_shares_its_parents_desktop_one_holder_at_a_time() {
    let e = env();
    let main = agent("w1", "s-main");
    let side = agent("w1", "s-side");
    let (_, r) = env_call(&main, json!({}));
    let until = r["hold"]["until"].as_i64().unwrap();
    // While the main session holds it, the sidecar is told who and until when.
    let (s, r) = env_call(&side, json!({}));
    assert_eq!((s, code(&r)), (403, "desktop_in_use"), "{r}");
    assert_eq!(r["holder"], json!({"by": "scientist", "name": "scientist", "until": until}), "{r}");
    assert!(r["error"]["hint"].as_str().unwrap().contains("scientist"), "{r}");
    assert!(audit_lines().iter().any(|l| l["event"] == "agent_hold" && l["outcome"] == "refused_in_use" && l["who"] == "agent:scientist/helper"));
    // A release lets it in, on the same desktop.
    let (_, r) = env_call(&main, json!({"release": true}));
    assert_eq!(r["released"], true);
    let (s, r) = env_call(&side, json!({}));
    assert_eq!((s, r["desktop"].as_str(), r["hold"]["by"].as_str()), (200, Some("virtual:w1"), Some("scientist/helper")), "{r}");
    // And an expired hold lets the main session back in.
    let (s, _) = env_call(&main, json!({}));
    assert_eq!(s, 403, "the sidecar holds it now");
    e.rig.advance(vd::HOLD_FOR + 1);
    let (s, r) = env_call(&main, json!({}));
    assert_eq!((s, r["hold"]["by"].as_str()), (200, Some("scientist")), "{r}");
    // Releasing someone else's hold drops nothing.
    let (_, r) = env_call(&side, json!({"release": true}));
    assert_eq!(r["released"], false, "{r}");
}

#[test]
fn the_owners_switch_and_the_platform_come_first() {
    let _e = env();
    store::enable(vd::VIRTUAL, false, "owner").unwrap();
    let (s, r) = env_call(&agent("w1", "s-main"), json!({}));
    assert_eq!((s, code(&r)), (403, "desktop_off"), "{r}");
    let _mac = platform_for_test(false, true);
    let (s, r) = env_call(&agent("w1", "s-main"), json!({}));
    assert_eq!((s, code(&r)), (409, "linux_only"), "{r}");
}

#[test]
fn a_release_works_with_the_switch_off() {
    let _e = env();
    let a = agent("w1", "s-main");
    env_call(&a, json!({}));
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::set_agents_can_use_desktop(&conn, "w1", false).unwrap();
    }
    let (s, r) = env_call(&a, json!({"release": true}));
    assert_eq!((s, r["released"].as_bool()), (200, Some(true)), "{r}");
}

#[test]
fn status_names_the_agent_holding_it() {
    let _e = env();
    env_call(&agent("w1", "s-side"), json!({}));
    let params = HashMap::from([("desktop".to_string(), "virtual:w1".to_string())]);
    let req = Req { path: "/cli/desktop/status", is_post: false, body: b"", params: &params, ingress: "loopback", ip: "-", token: "" };
    let r = handle(&Who::Owner, &req);
    let v: Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(v["virtual"]["hold"]["name"], "scientist (helper)", "{v}");
}

#[test]
fn a_waiting_desktop_says_so_with_no_display() {
    let e = env();
    e.rig.host.set_facts(4, 3 << 30, 2); // one awake at a time
    let owner_start = |id: &str| {
        let body = json!({"desktop": id}).to_string();
        let params: HashMap<String, String> = HashMap::new();
        let req = Req { path: "/cli/desktop/start", is_post: true, body: body.as_bytes(), params: &params, ingress: "loopback", ip: "-", token: "" };
        handle(&Who::Owner, &req)
    };
    assert_eq!(owner_start("virtual").status, "200 OK");
    let (s, r) = env_call(&agent("w1", "s-main"), json!({}));
    assert_eq!((s, r["state"].as_str()), (200, Some("waiting")), "{r}");
    assert_eq!(r["display"], Value::Null);
    assert_eq!(r["env"], json!({}));
    assert!(r["message"].as_str().unwrap().contains("Waiting"), "{r}");
}
