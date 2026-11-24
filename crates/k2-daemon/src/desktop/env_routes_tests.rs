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
    _db: k2_core::db::TestDbGuard,
    _p: PlatformGuard,
    rig: Rig,
    _m: TestManager,
    _screens: Vec<deskd::TestDesktopDeskd>,
    _addr: AddressGuard,
    /// Last: fields drop in order, and the env lock must outlive the
    /// process-wide guards above it, or the next test (waiting on the lock)
    /// installs its own and then has them put back from under it.
    _home: k2_core::test_env::TempHome,
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
    let folder = k2_core::paths::k2_home_in(&home).join("desktop").join("virtual-w1");
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
    let hint = r["error"]["hint"].as_str().unwrap();
    assert!(hint.contains("scientist"), "{r}");
    // In plain words (L9: the box said "until 1794491163 (Unix time)" and
    // "alpha (alpha)"): a clock time and how long.
    assert!(hint.contains("(in 30 min)"), "{hint}");
    assert!(!hint.contains("Unix time") && !hint.contains(&until.to_string()), "{hint}");
    assert!(!hint.contains("scientist (scientist)"), "{hint}");
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

// ── 0.46.18: a terminal the daemon spawned for an agent ──────────────

/// A real passport in the process registry (this test's temp home).
fn passport(workspace: &str, agent_address: &str) -> String {
    let sid = k2_core::session::SessionId::new();
    crate::session_token::mint_session_token(
        &sid,
        &sid.to_string(),
        crate::session_token::HookPrincipal { workspace_uuid: workspace.into(), agent_address: agent_address.into() },
        crate::session_token::CredMode::ApiKey,
        crate::session_token::Provider::Anthropic,
    );
    sid.to_string()
}

fn shell(ws: &str, session: &str) -> Who {
    Who::Shell { project_id: ws.into(), session_id: session.into() }
}

/// Every session-name prefix K2 generates, with a name of that shape and
/// whether it names an agent. A new kind of session goes here on purpose.
const GENERATED_PREFIXES: &[(&str, &str, bool)] = &[
    // A person's tab, and a sidecar's (`k2 sidecar new`, a harness: those
    // pass as Who::Agent, never through the name).
    ("tab-", "tab-7d1f", false),
    // `k2 terminal spawn-background` with no --agent.
    ("terminal-", "terminal-0a1b2c3d", false),
    // `k2 remote-session shell`.
    ("remote-shell-", "remote-shell-g1-ab12cd34", false),
    // `/v1` host sessions (`api-<principal>-<uuid>`).
    ("api-", "api-key-1-5e0c1a9b-2f3d-4e5f-8a7b-6c5d4e3f2a1b", false),
];

/// The daemon's and k2-core's source (no test files or in-file test
/// modules: they name made-up sessions), and whether the file spawns
/// sessions (builds a spawn request or registers one).
fn spawn_sources() -> Vec<(String, String, bool)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let markers = ["SpawnWorkspaceSessionRequest {", "v2_session_map::register(", "\"agent_name\":"];
    let mut out = Vec::new();
    let mut dirs = vec![root.join("src"), root.join("../k2-core/src")];
    while let Some(d) = dirs.pop() {
        for e in std::fs::read_dir(&d).unwrap_or_else(|e| panic!("{}: {e}", d.display())) {
            let p = e.unwrap().path();
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            if p.is_dir() {
                if name != "tests" {
                    dirs.push(p);
                }
                continue;
            }
            if !name.ends_with(".rs") || name.ends_with("_tests.rs") || name == "tests.rs" {
                continue;
            }
            let mut src = std::fs::read_to_string(&p).unwrap();
            // Cut an in-file test module (it names made-up sessions).
            if let Some(i) = src.find("#[cfg(test)]\nmod ").or_else(|| src.find("#[cfg(test)]\npub(crate) mod ")) {
                src.truncate(i);
            }
            let spawns = markers.iter().any(|m| src.contains(m));
            out.push((p.display().to_string(), src, spawns));
        }
    }
    out
}

/// The literal head of a `format!("…{` that starts at `at` (whitespace,
/// then the string), when it has a generated name's shape: `word-…-`.
fn name_head(at: &str) -> Option<String> {
    let lit = at.trim_start().strip_prefix('"')?;
    let head = &lit[..lit.find(['{', '"'])?];
    let shape = head.ends_with('-')
        && head.starts_with(|c: char| c.is_ascii_lowercase())
        && head.trim_end_matches('-').split('-').all(|w| !w.is_empty() && w.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()));
    shape.then(|| head.to_string())
}

/// Generated session-name prefixes in one file: in a file that spawns
/// sessions, every `format!("word-…{`; anywhere, one assigned to a session
/// name (`agent_name`, `session_key`, `map_key` `=` or `:` `format!(…)`).
fn generated_prefixes(src: &str, spawns: bool) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = src;
    while let Some(i) = rest.find("format!(") {
        let before = rest[..i].trim_end();
        rest = &rest[i + "format!(".len()..];
        let assigned = ["agent_name", "session_key", "map_key"]
            .iter()
            .any(|k| before.strip_suffix(['=', ':']).is_some_and(|b| b.trim_end().ends_with(k)));
        if spawns || assigned {
            found.extend(name_head(rest));
        }
    }
    found
}

/// Review C1: "names an agent" is a deny-list of name shapes, so it fails
/// open. Every prefix the spawn code generates must be classified in
/// [`GENERATED_PREFIXES`] (a new session kind fails here until it is), and
/// each classified non-agent shape is refused.
#[test]
fn every_session_name_prefix_k2_generates_is_classified() {
    let sources = spawn_sources();
    assert!(sources.iter().filter(|s| s.2).count() > 5, "the scan found the spawn code");
    let mut seen = std::collections::BTreeSet::new();
    for (path, src, spawns) in &sources {
        for p in generated_prefixes(src, *spawns) {
            assert!(
                GENERATED_PREFIXES.iter().any(|(k, _, _)| p.starts_with(k)),
                "{path} generates session names starting {p:?}: classify it in GENERATED_PREFIXES (an agent's or not) and in names_an_agent"
            );
            seen.insert(p);
        }
    }
    // The scan still finds the kinds we know (it would go quiet if the
    // spawn code moved).
    for want in ["tab-", "terminal-", "remote-shell-", "api-"] {
        assert!(seen.iter().any(|p| p.starts_with(want)), "{want} not found by the scan: {seen:?}");
    }
    for &(prefix, example, agent) in GENERATED_PREFIXES {
        assert!(example.starts_with(prefix));
        assert_eq!(names_an_agent(example, "w1"), agent, "{example}");
        let p = crate::session_token::HookPrincipal { workspace_uuid: "w1".into(), agent_address: example.to_string() };
        assert_eq!(passport_agent("w1", Some(&p), Some(example)).is_some(), agent, "{example}");
    }
}

/// The rule, pure: the passport's own workspace and agent, and the name its
/// session was spawned under, never the program it runs.
#[test]
fn a_passport_names_an_agent_only_for_the_session_spawned_as_that_agent() {
    use crate::session_token::HookPrincipal;
    let p = |ws: &str, a: &str| HookPrincipal { workspace_uuid: ws.into(), agent_address: a.into() };
    // `k2 terminal spawn --agent builder`: spawned as builder, passport for builder.
    assert_eq!(passport_agent("w1", Some(&p("w1", "builder")), Some("builder")).as_deref(), Some("builder"));
    // The workspace's canonical slot (its own id).
    let ws = "0b5f6f2e-6c1d-4c38-9a8e-3f1d2b7c9e01";
    assert_eq!(passport_agent(ws, Some(&p(ws, ws)), Some(ws)).as_deref(), Some(ws));
    for (why, principal, spawned) in [
        ("a person's shell tab (v2 spawn: the passport names the workspace)", p("w1", "scientist"), "tab-7d1f"),
        ("a person's tab, even if the passport said tab-*", p("w1", "tab-7d1f"), "tab-7d1f"),
        ("a terminal spawned with no agent", p("w1", "terminal-0a1b2c3d"), "terminal-0a1b2c3d"),
        ("a remote-session shell", p("w1", "remote-shell-g1-ab12"), "remote-shell-g1-ab12"),
        ("a /v1 host session", p("w1", "api-x-1"), "api-x-1"),
        ("a renderer terminal's bare id", p("w1", "5e0c1a9b-2f3d-4e5f-8a7b-6c5d4e3f2a1b"), "5e0c1a9b-2f3d-4e5f-8a7b-6c5d4e3f2a1b"),
        ("another workspace's passport", p("w2", "builder"), "builder"),
        ("a passport for another agent than its session", p("w1", "builder"), "atlas"),
        ("an empty name", p("w1", ""), ""),
    ] {
        assert_eq!(passport_agent("w1", Some(&principal), Some(spawned)), None, "{why}");
    }
    assert_eq!(passport_agent("w1", None, Some("builder")), None, "no live passport");
    assert_eq!(passport_agent("w1", Some(&p("w1", "builder")), None), None, "no live session");
    // An agent may be called terminal-<anything but 8 hex>.
    assert!(names_an_agent("terminal-runner", "w1"));
}

/// 0.46.18 field report: `k2 desktop env` from a shell the daemon spawned
/// for an agent (`k2 terminal spawn --agent builder --command bash`) was
/// refused as a person's (`desktop_env_agents_only`). It is that agent now;
/// a person's own shell tab is still refused.
#[test]
fn a_shell_spawned_for_an_agent_gets_its_desktop_and_a_persons_shell_tab_is_refused() {
    let _e = env();
    let built = passport("w1", "builder");
    let tab = passport("w1", "scientist");
    let other = passport("w2", "builder");
    let gone = k2_core::session::SessionId::new().to_string();
    let _spawns = spawns_for_test(HashMap::from([
        (built.clone(), "builder".to_string()),
        (tab.clone(), "tab-7d1f".to_string()),
        (other.clone(), "builder".to_string()),
        (gone.clone(), "builder".to_string()),
    ]));
    for (why, who) in [
        ("a person's own shell tab", shell("w1", &tab)),
        ("another workspace's passport", shell("w1", &other)),
        ("no live passport", shell("w1", &gone)),
    ] {
        let (s, r) = env_call(&who, json!({}));
        assert_eq!((s, code(&r)), (403, "desktop_env_agents_only"), "{why}: {r}");
    }
    let (s, r) = env_call(&shell("w1", &built), json!({}));
    assert_eq!((s, r["state"].as_str()), (200, Some("ready")), "{r}");
    assert_eq!(r["desktop"], "virtual:w1", "its own workspace's desktop: {r}");
    // Still its own desktop only, and the same switch applies.
    let (s, r) = env_call(&shell("w1", &built), json!({"desktop": "virtual:w2"}));
    assert_eq!((s, code(&r)), (403, "not_your_desktop"), "{r}");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::set_agents_can_use_desktop(&conn, "w1", false).unwrap();
    }
    let (s, r) = env_call(&shell("w1", &built), json!({}));
    assert_eq!((s, code(&r)), (403, "desktop_switch_off"), "{r}");
}
