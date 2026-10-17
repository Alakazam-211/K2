//! Garden/widget puts and the proposal queue through `zen_routes::handle`
//! (the real gate), under a temp HOME and a scoped DB. Fail loudly.

use std::collections::HashMap;

use serde_json::{json, Value as J};

use crate::sidecar_routes::{AgentCaller, Caller};
use k2_core::zen::{ZenFile, ZenFiles};

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

/// A workspace row with the switch on or off.
fn workspace(id: &str, manage: bool) -> Caller {
    let h = k2_core::db::shared();
    let c = h.lock();
    c.execute(
        "INSERT INTO projects (id, name, path, handle, agents_can_manage_gardens) VALUES (?1, ?1, ?2, ?1, ?3)",
        rusqlite::params![id, format!("/tmp/zen-agent/{id}"), i64::from(manage)],
    )
    .expect("insert workspace");
    Caller::Agent(AgentCaller { project_id: id.into(), map_key: None, args: vec![], sandboxed: false })
}

fn call(path: &str, owner: bool, caller: &Caller, body: J) -> (String, J) {
    let r = crate::zen_routes::handle(path, owner, caller, "test", &HashMap::new(), body.to_string().as_bytes());
    let v: J = serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{path}: not JSON ({e}): {}", r.body));
    (r.status.to_string(), v)
}

fn get(path: &str, caller: &Caller, q: &[(&str, &str)]) -> (String, J) {
    let p: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let r = crate::zen_routes::handle(path, false, caller, "test", &p, b"");
    (r.status.to_string(), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{path}: {e}: {}", r.body)))
}

fn garden1(f: &ZenFiles) -> String {
    f.find_garden("Garden 1").unwrap_or_else(|| panic!("no Garden 1: {:?}", f.gardens())).1.id
}

fn page(accent: &str) -> String {
    format!("schema = 1\ntemplate = \"k2.texting@1\"\n\n[colors.light]\naccent = \"{accent}\"\n")
}

fn disk(f: &ZenFiles, id: &str) -> Option<String> {
    std::fs::read_to_string(f.path_of(&ZenFile::Garden(id.into()))).ok()
}

fn audit_events() -> Vec<J> {
    k2_core::auth_audit::tail(1000).expect("audit tail")
}

fn has_audit(event: &str, target: &str) -> bool {
    audit_events()
        .iter()
        .any(|e| e["event"] == event && e["outcome"].as_str().is_some_and(|o| o.contains(target)))
}

#[test]
fn an_agent_with_the_switch_writes_a_garden_page_and_it_is_audited() {
    let e = env();
    let agent = workspace("w-mgr", true);
    let g = garden1(&e.f);
    let (s, v) = call("/cli/zen/garden/put", false, &agent, json!({"garden": g, "text": page("#123456")}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v["changed"], true, "{v}");
    assert_eq!(disk(&e.f, &g).as_deref(), Some(page("#123456").as_str()));
    assert!(has_audit("zen-garden-put", &g), "audit: {:?}", audit_events());
    // A page with errors is refused and nothing is written.
    let (s, v) = call("/cli/zen/garden/put", false, &agent, json!({"garden": g, "text": "schema = 1\nnot_a_key = 3\n"}));
    assert_eq!(s, "400 Bad Request", "{v}");
    assert!(v["message"].as_str().is_some_and(|m| m.contains("nothing was written")), "{v}");
    assert_eq!(disk(&e.f, &g).as_deref(), Some(page("#123456").as_str()), "a bad page never lands");
}

#[test]
fn without_the_switch_a_garden_put_becomes_a_proposal_a_person_approves() {
    let e = env();
    let agent = workspace("w-plain", false);
    let g = garden1(&e.f);
    let before = disk(&e.f, &g);
    let (s, v) = call("/cli/zen/garden/put", false, &agent, json!({"garden": g, "text": page("#2a4d8f"), "note": "brighter"}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v["proposed"], true, "{v}");
    assert_eq!(disk(&e.f, &g), before, "a proposal writes nothing");
    let id = v["proposal"]["id"].as_str().expect("proposal id").to_string();
    // It is listed, and show carries the content.
    let (s, l) = get("/cli/zen/proposals", &agent, &[]);
    assert_eq!(s, "200 OK", "{l}");
    assert!(l["proposals"].as_array().is_some_and(|a| a.iter().any(|p| p["id"] == id.as_str())), "{l}");
    let (_, sh) = get("/cli/zen/proposals/show", &agent, &[("id", &id)]);
    assert_eq!(sh["proposal"]["content"], page("#2a4d8f"), "{sh}");
    assert_eq!(sh["proposal"]["stale"], false, "{sh}");
    // The agent without the switch can't approve its own proposal.
    let (s, v) = call("/cli/zen/proposals/approve", false, &agent, json!({"id": id}));
    assert_eq!((s.as_str(), v["error"].as_str()), ("403 Forbidden", Some("person_only")), "{v}");
    // The person approves: the page lands.
    let (s, v) = call("/cli/zen/proposals/approve", true, &Caller::Owner, json!({"id": id}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(disk(&e.f, &g).as_deref(), Some(page("#2a4d8f").as_str()));
    let (s, v) = call("/cli/zen/proposals/approve", true, &Caller::Owner, json!({"id": id}));
    assert_eq!((s.as_str(), v["error"].as_str()), ("409 Conflict", Some("not_pending")), "{v}");
}

#[test]
fn a_stale_proposal_is_refused_and_an_agent_withdraws_only_its_own() {
    let e = env();
    let a = workspace("w-a", false);
    let b = workspace("w-b", false);
    let g = garden1(&e.f);
    let (_, v) = call("/cli/zen/garden/put", false, &a, json!({"garden": g, "text": page("#111111")}));
    let id = v["proposal"]["id"].as_str().expect("id").to_string();
    // The person changes the page after the proposal was made.
    call("/cli/zen/garden/put", true, &Caller::Owner, json!({"garden": g, "text": page("#222222")}));
    let (s, v) = call("/cli/zen/proposals/approve", true, &Caller::Owner, json!({"id": id}));
    assert_eq!((s.as_str(), v["error"].as_str()), ("409 Conflict", Some("proposal_stale")), "{v}");
    assert_eq!(disk(&e.f, &g).as_deref(), Some(page("#222222").as_str()), "a stale proposal never overwrites");
    let (s, v) = call("/cli/zen/proposals/reject", false, &b, json!({"id": id}));
    assert_eq!((s.as_str(), v["error"].as_str()), ("403 Forbidden", Some("not_own")), "{v}");
    let (s, v) = call("/cli/zen/proposals/reject", false, &a, json!({"id": id}));
    assert_eq!((s.as_str(), v["status"].as_str()), ("200 OK", Some("withdrawn")), "{v}");
}

#[test]
fn widget_put_writes_with_the_switch_proposes_without_and_never_touches_built_ins() {
    let e = env();
    let mgr = workspace("w-wmgr", true);
    let plain = workspace("w-wplain", false);
    e.f.new_widget("board", None).expect("new widget");
    let html = "<!doctype html><p>board v2</p>";
    let (s, v) = call("/cli/zen/widget/put", false, &mgr, json!({"widget": "board", "file": "index.html", "text": html}));
    assert_eq!(s, "200 OK", "{v}");
    let on_disk = std::fs::read_to_string(e.f.widget_dir("board").join("index.html")).expect("index.html");
    assert_eq!(on_disk, html);
    assert!(has_audit("zen-widget-put", "widgets/board/index.html"), "{:?}", audit_events());
    // Without the switch: a proposal, nothing written.
    let (s, v) = call("/cli/zen/widget/put", false, &plain, json!({"widget": "board", "file": "index.html", "text": "<p>v3</p>"}));
    assert_eq!((s.as_str(), v["proposed"].as_bool()), ("200 OK", Some(true)), "{v}");
    assert_eq!(std::fs::read_to_string(e.f.widget_dir("board").join("index.html")).expect("read"), html);
    // An agent with the switch may approve another agent's proposal.
    let id = v["proposal"]["id"].as_str().expect("id").to_string();
    let (s, v) = call("/cli/zen/proposals/approve", false, &mgr, json!({"id": id}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(std::fs::read_to_string(e.f.widget_dir("board").join("index.html")).expect("read"), "<p>v3</p>");
    assert!(has_audit("zen-proposal-approve", &id));
    // K2's built-in widgets and non-code files are never written.
    for (w, file) in [("k2:diary", "index.html"), ("board", "../escape.js"), ("board", ".env"), ("board", "pic.png")] {
        let (s, v) = call("/cli/zen/widget/put", false, &mgr, json!({"widget": w, "file": file, "text": "x"}));
        assert_eq!(s, "400 Bad Request", "{w}/{file}: {v}");
    }
}

#[test]
fn a_locked_garden_refuses_puts_and_proposals_and_logins_are_local_only() {
    let e = env();
    let mgr = workspace("w-lock", true);
    let g = garden1(&e.f);
    e.f.set_garden_lock(&g, true).expect("lock");
    for who in [mgr.clone(), workspace("w-lock2", false)] {
        let (s, v) = call("/cli/zen/garden/put", false, &who, json!({"garden": g, "text": page("#333333")}));
        assert_eq!((s.as_str(), v["error"].as_str()), ("423 Locked", Some("garden_locked")), "{who:?}: {v}");
    }
    let login = Caller::Login { username: "ada".into() };
    let (s, v) = call("/cli/zen/garden/put", false, &login, json!({"garden": g, "text": page("#333333")}));
    assert_eq!((s.as_str(), v["error"].as_str()), ("403 Forbidden", Some("zen_local_only")), "{v}");
}

#[test]
fn only_an_agent_with_the_switch_makes_a_garden() {
    let _e = env();
    let plain = workspace("w-new-plain", false);
    let (s, v) = call("/cli/zen/garden/new", false, &plain, json!({"name": "Fleet Ops"}));
    assert_eq!((s.as_str(), v["error"].as_str()), ("403 Forbidden", Some("zen_local_only")), "{v}");
    let mgr = workspace("w-new-mgr", true);
    let (s, v) = call("/cli/zen/garden/new", false, &mgr, json!({"name": "Fleet Ops"}));
    assert_eq!(s, "200 OK", "{v}");
    assert!(has_audit("zen-garden-new", "ok target="), "{:?}", audit_events());
}
