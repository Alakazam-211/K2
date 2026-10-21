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

// ── the widget dev loop (0.46.1 S3) ─────────────────────────────────────

const PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUg==";

fn folder(id: &str, body: &str) -> J {
    json!([
        {"name": "manifest.json", "text": format!("{{\"schema\": 1, \"id\": \"{id}\", \"name\": \"Board\", \"caps\": [\"agents:read\"]}}")},
        {"name": "index.html", "text": format!("<!doctype html><p>{body}</p><script src=\"app.js\"></script>")},
        {"name": "app.js", "text": "console.log(1)\n"},
        {"name": "logo.png", "base64": PNG_B64},
    ])
}

fn file(f: &ZenFiles, w: &str, name: &str) -> Option<String> {
    std::fs::read_to_string(f.widget_dir(w).join(name)).ok()
}

#[test]
fn a_folder_put_writes_whole_with_the_switch_and_refuses_a_broken_folder() {
    let e = env();
    let mgr = workspace("w-fmgr", true);
    let (s, v) = call("/cli/zen/widget/folder/put", false, &mgr, json!({"widget": "board", "files": folder("board", "v1")}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v["written"], json!(["app.js", "index.html", "logo.png", "manifest.json"]), "{v}");
    assert_eq!(v["validate"]["ok"], true, "{v}");
    assert!(file(&e.f, "board", "index.html").is_some_and(|t| t.contains("v1")));
    assert!(has_audit("zen-widget-folder-put", "widgets/board"), "{:?}", audit_events());
    // Broken: refused, nothing written.
    let mut broken = folder("board", "v2");
    broken[0]["text"] = json!("{ not json");
    let (s, v) = call("/cli/zen/widget/folder/put", false, &mgr, json!({"widget": "board", "files": broken}));
    assert_eq!(s, "400 Bad Request", "{v}");
    assert!(v["message"].as_str().is_some_and(|m| m.contains("nothing was written")), "{v}");
    assert!(file(&e.f, "board", "index.html").is_some_and(|t| t.contains("v1")), "a bad folder never lands");
    // Built-ins are never written.
    let (s, v) = call("/cli/zen/widget/folder/put", false, &mgr, json!({"widget": "k2:diary", "files": folder("diary", "x")}));
    assert_eq!(s, "400 Bad Request", "{v}");
}

/// The folder with `app.js` changed too (two code files differ from v1).
fn folder_two_changes(id: &str, body: &str) -> J {
    let mut f = folder(id, body);
    f[2]["text"] = json!("console.log(2)\n");
    f
}

#[test]
fn without_the_switch_a_folder_put_is_one_proposal_approved_as_one() {
    let e = env();
    let plain = workspace("w-fone", false);
    let (s, v) = call("/cli/zen/widget/folder/put", true, &Caller::Owner, json!({"widget": "board", "files": folder("board", "v1")}));
    assert_eq!(s, "200 OK", "{v}");
    let (s, v) = call("/cli/zen/widget/folder/put", false, &plain, json!({"widget": "board", "files": folder_two_changes("board", "v2"), "note": "bigger"}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v["proposed"], true, "{v}");
    let props = v["proposals"].as_array().expect("proposals").clone();
    assert_eq!(props.len(), 1, "two changed files are ONE proposal: {v}");
    let p = &v["proposal"];
    assert_eq!(p, &props[0], "{v}");
    assert_eq!((p["kind"].as_str(), p["target"].as_str()), (Some("folder"), Some("board")), "{p}");
    assert!(p["file"].is_null(), "a folder proposal names no file: {p}");
    assert_eq!(p["files"], json!(["app.js", "index.html"]), "{p}");
    assert_eq!(p["unchanged"], 2, "manifest.json and logo.png are kept as they are: {p}");
    assert_eq!(p["note"], "bigger", "{p}");
    assert!(file(&e.f, "board", "index.html").is_some_and(|t| t.contains("v1")), "a proposal writes nothing");
    assert_eq!(file(&e.f, "board", "app.js").as_deref(), Some("console.log(1)\n"));
    let id = p["id"].as_str().expect("id").to_string();
    // Listed once; show carries every changed file and the disk now.
    let (_, l) = get("/cli/zen/proposals", &plain, &[]);
    let rows: Vec<&J> = l["proposals"].as_array().expect("list").iter().filter(|r| r["target"] == "board").collect();
    assert_eq!(rows.len(), 1, "{l}");
    assert_eq!(rows[0]["kind"], "folder", "{l}");
    let (s, sh) = get("/cli/zen/proposals/show", &plain, &[("id", &id)]);
    assert_eq!(s, "200 OK", "{sh}");
    let content = sh["proposal"]["content"].as_array().expect("content is the changed files").clone();
    let names: Vec<&str> = content.iter().map(|c| c["name"].as_str().expect("name")).collect();
    assert_eq!(names, vec!["app.js", "index.html"], "{sh}");
    assert_eq!(content[0]["text"], "console.log(2)\n", "{sh}");
    assert_eq!(sh["proposal"]["current"]["app.js"], "console.log(1)\n", "{sh}");
    assert_eq!((sh["proposal"]["stale"].as_bool(), sh["proposal"]["staleFiles"].clone()), (Some(false), json!([])), "{sh}");
    assert_eq!(sh["proposal"]["path"], "widgets/board/ (app.js, index.html)", "{sh}");
    // The person approves: both files land at once.
    let (s, v) = call("/cli/zen/proposals/approve", true, &Caller::Owner, json!({"id": id}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v["applied"]["written"], json!(["app.js", "index.html"]), "{v}");
    assert!(file(&e.f, "board", "index.html").is_some_and(|t| t.contains("v2")));
    assert_eq!(file(&e.f, "board", "app.js").as_deref(), Some("console.log(2)\n"));
    assert!(has_audit("zen-proposal-approve", &id), "{:?}", audit_events());
    let (s, v) = call("/cli/zen/proposals/approve", true, &Caller::Owner, json!({"id": id}));
    assert_eq!((s.as_str(), v["error"].as_str()), ("409 Conflict", Some("not_pending")), "{v}");
}

#[test]
fn a_folder_proposal_whose_files_moved_writes_nothing() {
    let e = env();
    let plain = workspace("w-fstale", false);
    let mgr = workspace("w-fstale-mgr", true);
    call("/cli/zen/widget/folder/put", true, &Caller::Owner, json!({"widget": "board", "files": folder("board", "v1")}));
    // A kept file (not one it changes) moves after the proposal: stale too.
    let (_, v) = call("/cli/zen/widget/folder/put", false, &plain, json!({"widget": "board", "files": folder("board", "v2")}));
    let id = v["proposal"]["id"].as_str().expect("id").to_string();
    assert_eq!(v["proposal"]["files"], json!(["index.html"]), "{v}");
    let mut moved = folder("board", "v1");
    moved[2]["text"] = json!("console.log('person')\n");
    let (s, v) = call("/cli/zen/widget/folder/put", true, &Caller::Owner, json!({"widget": "board", "files": moved}));
    assert_eq!(s, "200 OK", "{v}");
    let (_, sh) = get("/cli/zen/proposals/show", &plain, &[("id", &id)]);
    assert_eq!((sh["proposal"]["stale"].as_bool(), sh["proposal"]["staleFiles"].clone()), (Some(true), json!(["app.js"])), "{sh}");
    let (s, v) = call("/cli/zen/proposals/approve", false, &mgr, json!({"id": id}));
    assert_eq!((s.as_str(), v["error"].as_str()), ("409 Conflict", Some("proposal_stale")), "{v}");
    assert!(v["message"].as_str().is_some_and(|m| m.contains("app.js") && m.contains("nothing was written")), "{v}");
    assert!(file(&e.f, "board", "index.html").is_some_and(|t| t.contains("v1")), "a stale folder proposal never lands any file");
    assert_eq!(file(&e.f, "board", "app.js").as_deref(), Some("console.log('person')\n"));
    let (s, v) = call("/cli/zen/proposals/reject", false, &plain, json!({"id": id}));
    assert_eq!((s.as_str(), v["status"].as_str()), ("200 OK", Some("withdrawn")), "{v}");
}

#[test]
fn a_new_widget_folder_is_one_proposal_and_lands_whole() {
    let e = env();
    let plain = workspace("w-fnew", false);
    let mut code_only = folder("fresh", "first");
    code_only.as_array_mut().expect("files").truncate(3);
    let (s, v) = call("/cli/zen/widget/folder/put", false, &plain, json!({"widget": "fresh", "files": code_only}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v["proposal"]["files"], json!(["app.js", "index.html", "manifest.json"]), "{v}");
    assert_eq!(v["proposal"]["unchanged"], 0, "{v}");
    assert!(!e.f.widget_dir("fresh").exists(), "a proposal makes no folder");
    let id = v["proposal"]["id"].as_str().expect("id").to_string();
    let (s, v) = call("/cli/zen/proposals/approve", true, &Caller::Owner, json!({"id": id}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v["applied"]["validate"]["ok"], true, "{v}");
    for name in ["app.js", "index.html", "manifest.json"] {
        assert!(file(&e.f, "fresh", name).is_some(), "{name} landed");
    }
}

#[test]
fn without_the_switch_a_folder_put_proposes_only_what_changed() {
    let e = env();
    let plain = workspace("w-fplain", false);
    let (s, v) = call("/cli/zen/widget/folder/put", true, &Caller::Owner, json!({"widget": "board", "files": folder("board", "v1")}));
    assert_eq!(s, "200 OK", "{v}");
    let (s, v) = call("/cli/zen/widget/folder/put", false, &plain, json!({"widget": "board", "files": folder("board", "v2"), "note": "bigger"}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v["proposed"], true, "{v}");
    let props = v["proposals"].as_array().expect("proposals").clone();
    assert_eq!(props.len(), 1, "one proposal: {v}");
    assert_eq!(props[0]["files"], json!(["index.html"]), "only index.html changed: {v}");
    assert_eq!(props[0]["note"], "bigger");
    assert!(file(&e.f, "board", "index.html").is_some_and(|t| t.contains("v1")), "a proposal writes nothing");
    // Nothing changed: no proposals.
    let (s, v) = call("/cli/zen/widget/folder/put", false, &plain, json!({"widget": "board", "files": folder("board", "v1")}));
    assert_eq!((s.as_str(), v["proposed"].as_bool()), ("200 OK", Some(false)), "{v}");
    // An image change needs a writer; prune is refused.
    let mut asset = folder("board", "v1");
    asset[3]["base64"] = json!("iVBORw0KGgoAAAANSUhEUgAA");
    let (s, v) = call("/cli/zen/widget/folder/put", false, &plain, json!({"widget": "board", "files": asset}));
    assert_eq!(s, "400 Bad Request", "{v}");
    assert!(v["message"].as_str().is_some_and(|m| m.contains("image or font")), "{v}");
    let (s, v) = call("/cli/zen/widget/folder/put", false, &plain, json!({"widget": "board", "files": folder("board", "v3"), "prune": true}));
    assert_eq!(s, "400 Bad Request", "{v}");
    // The person approves the proposal: it lands.
    let id = props[0]["id"].as_str().expect("id");
    let (s, v) = call("/cli/zen/proposals/approve", true, &Caller::Owner, json!({"id": id}));
    assert_eq!(s, "200 OK", "{v}");
    assert!(file(&e.f, "board", "index.html").is_some_and(|t| t.contains("v2")));
}

#[test]
fn a_folder_check_writes_nothing_for_any_local_agent_and_never_pends() {
    let e = env();
    let plain = workspace("w-fcheck", false);
    let (s, v) = call("/cli/zen/widget/folder/check", false, &plain, json!({"files": folder("board", "v1"), "fallback": "staged-dir"}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!((v["ok"].as_bool(), v["status"].as_str(), v["widget"].as_str()), (Some(true), Some("ok"), Some("board")), "{v}");
    assert!(!e.f.widget_dir("board").exists(), "check writes nothing");
    let mut bad = folder("board", "v1");
    bad[1] = json!({"name": "index.html", "text": "<p onclick=\"x()\">inline</p>"});
    let (s, v) = call("/cli/zen/widget/folder/check", false, &plain, json!({"files": bad}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!((v["ok"].as_bool(), v["status"].as_str()), (Some(false), Some("failed")), "{v}");
    assert!(v["errors"].as_array().is_some_and(|e| !e.is_empty()), "{v}");
    let (s, v) = call("/cli/zen/widget/folder/check", false, &plain, json!({"files": [{"name": "../x.js", "text": ""}]}));
    assert_eq!(s, "400 Bad Request", "{v}");
    // A Connect login is still turned away.
    let login = Caller::Login { username: "ada".into() };
    let (s, v) = call("/cli/zen/widget/folder/check", false, &login, json!({"files": folder("board", "v1")}));
    assert_eq!((s.as_str(), v["error"].as_str()), ("403 Forbidden", Some("zen_local_only")), "{v}");
}

#[test]
fn a_garden_check_reports_errors_and_writes_nothing() {
    let e = env();
    let plain = workspace("w-gcheck", false);
    let g = garden1(&e.f);
    let before = disk(&e.f, &g);
    let (s, v) = call("/cli/zen/garden/check", false, &plain, json!({"garden": g, "text": "schema = 1\nnot_a_key = 3\n"}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!((v["ok"].as_bool(), v["status"].as_str()), (Some(false), Some("failed")), "{v}");
    assert!(v["errors"][0]["message"].as_str().is_some_and(|m| m.contains("not_a_key")), "{v}");
    let (s, v) = call("/cli/zen/garden/check", false, &plain, json!({"garden": g, "text": page("#445566")}));
    assert_eq!((s.as_str(), v["ok"].as_bool()), ("200 OK", Some(true)), "{v}");
    assert_eq!(disk(&e.f, &g), before, "check never writes");
}

#[test]
fn only_a_writer_previews_and_the_preview_never_touches_the_persons_gardens() {
    let e = env();
    let plain = workspace("w-pplain", false);
    let mgr = workspace("w-pmgr", true);
    let (s, v) = call("/cli/zen/widget/preview", false, &plain, json!({"files": folder("board", "v1")}));
    assert_eq!((s.as_str(), v["error"].as_str()), ("403 Forbidden", Some("person_only")), "{v}");
    let before: Vec<(String, Option<String>)> = e.f.gardens().into_iter().map(|g| (g.id.clone(), disk(&e.f, &g.id))).collect();
    let (s, v) = call("/cli/zen/widget/preview", false, &mgr, json!({"files": folder("board", "v1"), "config": {"sources": "fleet"}}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v["preview"], "preview-board", "{v}");
    let gid = v["garden"]["id"].as_str().expect("garden").to_string();
    for (id, text) in &before {
        assert_ne!(id, &gid);
        assert_eq!(&disk(&e.f, id), text, "{id} untouched");
    }
    assert!(disk(&e.f, &gid).is_some_and(|t| t.contains("widget = \"preview-board\"") && t.contains("sources = \"fleet\"")));
    assert!(has_audit("zen-widget-preview", "preview-board"), "{:?}", audit_events());
    let (s, v) = call("/cli/zen/widget/preview", false, &mgr, json!({"clear": true}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v["removed"], json!(["preview-board"]), "{v}");
    assert!(e.f.find_garden("Widget preview").is_none());
}

#[test]
fn folder_routes_take_a_whole_folder_body_and_the_rest_keep_320_kb() {
    assert_eq!(crate::zen_routes::max_body("/cli/zen/widget/folder/put"), k2_core::zen::widget_folder::MAX_FOLDER_BODY);
    assert_eq!(crate::zen_routes::max_body("/cli/zen/widget/preview"), k2_core::zen::widget_folder::MAX_FOLDER_BODY);
    assert_eq!(crate::zen_routes::max_body("/cli/zen/widget/put"), crate::zen_routes::MAX_BODY);
    assert_eq!(crate::zen_routes::too_large_for("/cli/zen/widget/folder/check").status, "413 Payload Too Large");
    for p in crate::zen_agent_routes::FOLDER_ROUTES {
        assert!(crate::zen_routes::POST_ROUTES.contains(p), "{p} is a POST row");
    }
}

// ── "Keep as mine" (0.46.3) ─────────────────────────────────────────────

fn live_hash(f: &ZenFiles, w: &str) -> String {
    f.widget_status(w).bundle.map(|b| b.hash).unwrap_or_else(|| panic!("{w} has no good version"))
}

fn claim_live(f: &ZenFiles, w: &str, now: i64) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::garden_data::authors::claim(&conn, w, &live_hash(f, w), now).expect("claim");
}

fn keep_notes(project: &str, widget: &str) -> usize {
    k2_core::feedback::list_for_project(project, &k2_core::feedback::ListFilter::Open)
        .expect("tickets")
        .iter()
        .filter(|i| i.title == crate::zen_agent_routes::keep_title(widget) && i.kind == "fyi")
        .count()
}

#[test]
fn an_agent_changing_the_persons_widget_sends_one_keep_as_mine_note() {
    let e = env();
    let mgr = workspace("w-keep-mgr", true);
    let (s, v) = call("/cli/zen/widget/folder/put", true, &Caller::Owner, json!({"widget": "board", "files": folder("board", "v1")}));
    assert_eq!(s, "200 OK", "{v}");
    claim_live(&e.f, "board", 1);
    // The person's own write never asks.
    let (_, v) = call("/cli/zen/widget/folder/put", true, &Caller::Owner, json!({"widget": "board", "files": folder("board", "v1b")}));
    assert!(v.get("keepAsMine").is_none(), "{v}");
    claim_live(&e.f, "board", 2);
    let (s, v) = call("/cli/zen/widget/put", false, &mgr, json!({"widget": "board", "file": "app.js", "text": "console.log('agent')\n"}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v["keepAsMine"], true, "{v}");
    assert_eq!(keep_notes("w-keep-mgr", "board"), 1);
    // The next agent write: the version before it was already not the person's.
    let (_, v) = call("/cli/zen/widget/folder/put", false, &mgr, json!({"widget": "board", "files": folder_two_changes("board", "v3")}));
    assert_eq!(v["keepAsMine"], false, "{v}");
    assert_eq!(keep_notes("w-keep-mgr", "board"), 1);
    // Kept again, then changed again while the first note is open: still one note.
    claim_live(&e.f, "board", 3);
    let (_, v) = call("/cli/zen/widget/put", false, &mgr, json!({"widget": "board", "file": "app.js", "text": "console.log('agent 2')\n"}));
    assert_eq!(v["keepAsMine"], true, "{v}");
    assert_eq!(keep_notes("w-keep-mgr", "board"), 1, "one open note per widget");
}

#[test]
fn approving_a_proposal_on_the_persons_widget_asks_keep_as_mine_too() {
    let e = env();
    let plain = workspace("w-keep-plain", false);
    call("/cli/zen/widget/folder/put", true, &Caller::Owner, json!({"widget": "board", "files": folder("board", "v1")}));
    claim_live(&e.f, "board", 1);
    let (_, v) = call("/cli/zen/widget/folder/put", false, &plain, json!({"widget": "board", "files": folder("board", "v2")}));
    let id = v["proposal"]["id"].as_str().expect("id").to_string();
    let (s, v) = call("/cli/zen/proposals/approve", true, &Caller::Owner, json!({"id": id}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!((v["keepAsMine"].as_bool(), v["target"].as_str()), (Some(true), Some("board")), "{v}");
    // No Thread with the proposing agent here (no pinned Chat): the note.
    assert_eq!(v["keepCard"], json!({"note": true}), "{v}");
    assert_eq!(keep_notes("w-keep-plain", "board"), 1, "no Thread for a card: one inbox note");
    // A widget that was never the person's: nothing to keep.
    let mgr = workspace("w-keep-mgr2", true);
    let (_, v) = call("/cli/zen/widget/folder/put", false, &mgr, json!({"widget": "other", "files": folder("other", "o1")}));
    let (_, v2) = call("/cli/zen/widget/folder/put", false, &mgr, json!({"widget": "other", "files": folder("other", "o2")}));
    assert_eq!((v["keepAsMine"].as_bool(), v2["keepAsMine"].as_bool()), (Some(false), Some(false)), "{v} {v2}");
    assert_eq!(keep_notes("w-keep-mgr2", "other"), 0);
}

/// Who may tap a Keep-as-mine card (the settings-cards review, 2026-10-20):
/// the agent it's about is refused, so is another agent, and so is a
/// Member login; an Owner login (phone, web) succeeds, at the role floor
/// of the Settings control. Every refusal leaves the card waiting and the
/// widget unkept. Other choice cards are untouched (not tested here).
#[test]
fn a_keep_card_is_answered_by_an_owner_or_admin_person_only() {
    let e = env();
    let mgr = workspace("w-keepw-mgr", true);
    let _other = workspace("w-keepw-other", false);
    let conv = pin_chat("w-keepw-mgr");
    call("/cli/zen/widget/folder/put", true, &Caller::Owner, json!({"widget": "board", "files": folder("board", "v1")}));
    claim_live(&e.f, "board", 1);
    let (_, v) = call("/cli/zen/widget/put", false, &mgr, json!({"widget": "board", "file": "app.js", "text": "console.log('agent')\n"}));
    let id = v["keepCard"]["card"].as_str().expect("a card").to_string();
    let live = live_hash(&e.f, "board");
    let unkept = || {
        let h = k2_core::db::shared();
        let c = h.lock();
        k2_core::garden_data::authors::maker_of(&c, "board", &live).expect("maker") != k2_core::garden_data::MadeBy::Owner
    };
    let body = json!({"addr": "w-keepw-mgr", "id": id, "answer": "Keep as mine"}).to_string();
    let passport = |ws: &str| -> HashMap<String, String> {
        [
            (crate::caller_workspace::PRINCIPAL_BOUND_KEY.to_string(), "1".to_string()),
            ("project_id".to_string(), ws.to_string()),
            ("from".to_string(), ws.to_string()),
        ]
        .into()
    };
    let refused = |r: &crate::cli_response::CliResponse, who: &str| {
        assert_eq!(r.status, "403 Forbidden", "{who}: {}", r.body);
        let v: J = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["error"]["code"], "action_card_person_only", "{who}: {v}");
    };
    // The agent the card is about.
    refused(&crate::overlay_routes::dispatch_post("/cli/thread/answer", &passport("w-keepw-mgr"), body.as_bytes()), "its own agent");
    // Another agent on this server.
    refused(&crate::overlay_routes::dispatch_post("/cli/thread/answer", &passport("w-keepw-other"), body.as_bytes()), "another agent");
    // A Member login.
    k2_core::connect_users::add_user("mel", "password-for-tests-1").expect("add member");
    k2_core::connect_users::set_role("mel", k2_core::connect_users::Role::Member).expect("member");
    refused(&crate::overlay_routes::dispatch_post_as("/cli/thread/answer", &HashMap::new(), body.as_bytes(), "mel"), "a member");
    assert_eq!(card(&conv, &id).status, "pending", "refusals leave the card waiting");
    assert!(unkept(), "nothing kept by a refused tap");
    // An Owner login (the phone, the web).
    k2_core::connect_users::add_user("olga", "password-for-tests-2").expect("add owner");
    k2_core::connect_users::set_role("olga", k2_core::connect_users::Role::Owner).expect("owner");
    let r = crate::overlay_routes::dispatch_post_as("/cli/thread/answer", &HashMap::new(), body.as_bytes(), "olga");
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert!(!unkept(), "the owner login kept it");
    let c = card(&conv, &id);
    assert_eq!((c.status.as_str(), c.answer.as_deref()), ("answered", Some("Keep as mine")));
    // Single use.
    let r = crate::overlay_routes::dispatch_post_as("/cli/thread/answer", &HashMap::new(), body.as_bytes(), "olga");
    assert_eq!(r.status, "409 Conflict", "{}", r.body);
}

/// Pin a Chat for a workspace: its agent has a Thread the person reads.
fn pin_chat(project_id: &str) -> String {
    let conv = uuid::Uuid::new_v4().to_string();
    let h = k2_core::db::shared();
    let c = h.lock();
    k2_core::db::schema::WorkspaceSession::upsert(&c, &format!("ws-{conv}"), project_id, None, Some(&conv), "claude", "system", "running")
        .expect("pin chat");
    conv
}

fn card(conv: &str, id: &str) -> k2_core::overlay::ChoiceBody {
    k2_core::overlay::get_thread_doc(conv, id).expect("doc").expect("card on the Thread").choice.expect("a choice card")
}

/// Rosson 2026-10-20: with a Thread, the person gets a CARD there instead of
/// the inbox note: "<agent> changed <widget> in <garden>. Keep it as yours
/// so Garden sync never replaces it?" with Keep as mine / Let sync update
/// it. A newer change replaces the waiting card (one per widget). The
/// person's approval of an agent's proposal gets the card on the Thread with
/// the agent that proposed it. One tap by the person keeps it.
#[test]
fn with_a_thread_the_person_gets_a_keep_as_mine_card_not_a_note() {
    let e = env();
    let mgr = workspace("w-keepc-mgr", true);
    let conv = pin_chat("w-keepc-mgr");
    call("/cli/zen/widget/folder/put", true, &Caller::Owner, json!({"widget": "board", "files": folder("board", "v1")}));
    claim_live(&e.f, "board", 1);
    let (s, v) = call("/cli/zen/widget/put", false, &mgr, json!({"widget": "board", "file": "app.js", "text": "console.log('agent')\n"}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v["keepAsMine"], true, "{v}");
    let first = v["keepCard"]["card"].as_str().expect("a card id").to_string();
    assert_eq!(v["keepCard"]["addr"], "w-keepc-mgr", "{v}");
    assert_eq!(keep_notes("w-keepc-mgr", "board"), 0, "the card replaces the inbox note");
    let c = card(&conv, &first);
    assert_eq!(
        c.prompt,
        "w-keepc-mgr changed board in your widgets. Keep it as yours so Garden sync never replaces it?",
        "the card's words"
    );
    assert_eq!(c.options.iter().map(|o| o.label.as_str()).collect::<Vec<_>>(), vec!["Keep as mine", "Let sync update it"]);
    let action = c.action.expect("an action card");
    assert_eq!((action.id.as_str(), action.params["widget"].as_str()), (crate::action_cards::KEEP_WIDGET, Some("board")));
    let hash = live_hash(&e.f, "board");
    assert_eq!(action.params["hash"], hash.as_str(), "the card keeps the version it describes");

    // Kept, changed again: the waiting card is replaced by one for the new version.
    claim_live(&e.f, "board", 2);
    let (_, v) = call("/cli/zen/widget/put", false, &mgr, json!({"widget": "board", "file": "app.js", "text": "console.log('agent 2')\n"}));
    let second = v["keepCard"]["card"].as_str().expect("a card id").to_string();
    assert_ne!(first, second);
    assert_eq!(card(&conv, &first).status, "voided", "one waiting card per widget");
    assert_eq!(card(&conv, &second).status, "pending");

    // The agent can't tap it; the person can, and the widget is theirs again.
    let mut passport = HashMap::new();
    passport.insert(crate::caller_workspace::PRINCIPAL_BOUND_KEY.to_string(), "1".to_string());
    passport.insert("project_id".to_string(), "w-keepc-mgr".to_string());
    passport.insert("from".to_string(), "w-keepc-mgr".to_string());
    let body = json!({"addr": "w-keepc-mgr", "id": second, "answer": "Keep as mine"}).to_string();
    let r = crate::overlay_routes::dispatch_post("/cli/thread/answer", &passport, body.as_bytes());
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    assert!(r.body.contains("action_card_person_only"), "{}", r.body);
    let live = live_hash(&e.f, "board");
    {
        let h = k2_core::db::shared();
        let c = h.lock();
        assert_eq!(k2_core::garden_data::authors::maker_of(&c, "board", &live).expect("maker"), k2_core::garden_data::MadeBy::Unclaimed);
    }
    let r = crate::overlay_routes::dispatch_post("/cli/thread/answer", &HashMap::new(), body.as_bytes());
    assert_eq!(r.status, "200 OK", "{}", r.body);
    {
        let h = k2_core::db::shared();
        let c = h.lock();
        assert_eq!(k2_core::garden_data::authors::maker_of(&c, "board", &live).expect("maker"), k2_core::garden_data::MadeBy::Owner, "one tap keeps it");
    }
    assert_eq!(keep_notes("w-keepc-mgr", "board"), 0);

    // The person approving an agent's proposal: the card is on the Thread
    // with the agent that proposed it.
    let plain = workspace("w-keepc-plain", false);
    let plain_conv = pin_chat("w-keepc-plain");
    let (_, v) = call("/cli/zen/widget/folder/put", false, &plain, json!({"widget": "board", "files": folder("board", "v9")}));
    let pid = v["proposal"]["id"].as_str().expect("proposal id").to_string();
    let (s, v) = call("/cli/zen/proposals/approve", true, &Caller::Owner, json!({"id": pid}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v["keepCard"]["addr"], "w-keepc-plain", "{v}");
    let on_plain = v["keepCard"]["card"].as_str().expect("card").to_string();
    assert_eq!(card(&plain_conv, &on_plain).status, "pending");
    assert_eq!(keep_notes("w-keepc-plain", "board"), 0);
}

// ── validate reflects reality (0.46.3) ─────────────────────────────────

#[test]
fn clearing_runtime_errors_is_a_post_any_local_agent_may_make_and_it_is_audited() {
    let e = env();
    let plain = workspace("w-clear", false);
    e.f.new_widget("board", Some("hello")).expect("new widget");
    let hash = e.f.validate_widget("board").expect("validate")["runtime"]["hash"].as_str().expect("hash").to_string();
    e.f.record_widget_run("board", &hash, "error", Some("boom"), None).expect("error");
    // Unplaced: the error is a warning and the agent may clear it.
    let (s, v) = call("/cli/zen/widget/errors/clear", false, &plain, json!({"widget": "board"}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!((v["cleared"].as_u64(), v["left"].as_u64()), (Some(1), Some(0)), "{v}");
    assert!(has_audit("zen.widget.errors_cleared", "widgets/board: cleared 1"), "{:?}", audit_events());
    // The person may too, and `all` clears everything.
    e.f.record_widget_run("board", &hash, "error", Some("again"), None).expect("error");
    let (s, v) = call("/cli/zen/widget/errors/clear", true, &Caller::Owner, json!({"widget": "board", "all": true}));
    assert_eq!((s.as_str(), v["cleared"].as_u64()), ("200 OK", Some(1)), "{v}");
    // A Connect login is still turned away; a bad body is 400; a GET is 405.
    let login = Caller::Login { username: "ada".into() };
    let (s, v) = call("/cli/zen/widget/errors/clear", false, &login, json!({"widget": "board"}));
    assert_eq!((s.as_str(), v["error"].as_str()), ("403 Forbidden", Some("zen_local_only")), "{v}");
    let (s, v) = call("/cli/zen/widget/errors/clear", true, &Caller::Owner, json!({"widget": "board", "hash": "x"}));
    assert_eq!(s, "400 Bad Request", "{v}");
    assert!(crate::zen_routes::POST_ROUTES.contains(&"/cli/zen/widget/errors/clear"));
    assert!(crate::routes::route_policy::get_refused("/cli/zen/widget/errors/clear", ""), "GET on the clear row is 405");
    assert!(!crate::routes::route_policy::get_refused("/cli/zen/widget/errors", "widget=board"), "reading errors stays a GET");
}

#[test]
fn validate_all_reports_each_widget_and_an_unplaced_ones_errors_never_fail() {
    let e = env();
    e.f.new_widget("fleet-ops-alerts", Some("hello")).expect("new widget");
    let hash = e.f.validate_widget("fleet-ops-alerts").expect("validate")["runtime"]["hash"].as_str().expect("hash").to_string();
    e.f.record_widget_run("fleet-ops-alerts", &hash, "error", Some("TypeError: x is null"), None).expect("error");
    let r = crate::zen_routes::handle("/cli/zen/validate", true, &Caller::Owner, "test", &HashMap::new(), b"");
    let v: J = serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("validate: not JSON ({e}): {}", r.body));
    assert_eq!(r.status, "200 OK", "{v}");
    assert_eq!(v["ok"], true, "an unplaced widget's runtime errors never fail validate: {v}");
    assert_eq!(v["staleRuntime"], 1, "{v}");
    let row = v["widgets"].as_array().expect("widgets").iter().find(|r| r["widget"] == "fleet-ops-alerts").expect("row").clone();
    assert_eq!((row["status"].as_str(), row["placed"].as_bool()), (Some("unplaced"), Some(false)), "{v}");
}

// ── headless shots (0.46.3) ──────────────────────────────────────────────

/// K2's own `k2:fleet-grid@2` card files, staged as a folder.
fn fleet_grid_folder() -> J {
    let w = k2_core::zen::builtin_widgets::BUILTIN_WIDGETS
        .iter()
        .find(|w| w.name == "fleet-grid" && w.version == 2)
        .expect("k2:fleet-grid@2");
    J::Array(w.files.iter().map(|(n, t)| json!({"name": n, "text": t})).collect())
}

#[test]
fn any_local_agent_gets_a_shot_page_and_nothing_is_written() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    let f = ZenFiles::local();
    assert!(!f.root().exists(), "Zen isn't set up in this test");
    let plain = workspace("w-shot", false);
    let (s, v) = call(
        "/cli/zen/widget/shot",
        false,
        &plain,
        json!({"files": fleet_grid_folder(), "fallback": "fleet-grid", "scheme": "dark", "config": {"source": "fleet"}, "width": 500, "height": 300}),
    );
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!((v["ok"].as_bool(), v["widget"].as_str(), v["scheme"].as_str()), (Some(true), Some("fleet-grid"), Some("dark")), "{v}");
    assert_eq!((v["width"].as_u64(), v["height"].as_u64()), (Some(500), Some(300)));
    assert!(v["frame"].as_str().is_some_and(|t| t.contains("color-scheme:dark!important")), "{v}");
    assert!(v["host"].as_str().is_some_and(|t| t.contains("__K2_SHOT_FRAME__") && t.contains("width:500px")), "{v}");
    assert!(!f.root().exists(), "a shot never makes or writes the Zen folder");
}

#[test]
fn a_shot_refuses_a_bad_scheme_reports_a_broken_folder_and_turns_away_logins() {
    let e = env();
    let plain = workspace("w-shot2", false);
    let before: Vec<(String, Option<String>)> = e.f.gardens().into_iter().map(|g| (g.id.clone(), disk(&e.f, &g.id))).collect();
    let (s, v) = call("/cli/zen/widget/shot", false, &plain, json!({"files": fleet_grid_folder(), "fallback": "x", "scheme": "dim"}));
    assert_eq!((s.as_str(), v["error"].as_str()), ("400 Bad Request", Some("bad_request")), "{v}");
    let mut broken = fleet_grid_folder();
    let i = broken.as_array().expect("files").iter().position(|f| f["name"] == "manifest.json").expect("manifest");
    broken[i]["text"] = json!("{ not json");
    let (s, v) = call("/cli/zen/widget/shot", false, &plain, json!({"files": broken, "fallback": "x"}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!((v["ok"].as_bool(), v["status"].as_str()), (Some(false), Some("failed")), "{v}");
    let (s, v) = call("/cli/zen/widget/shot", false, &plain, json!({"files": fleet_grid_folder(), "fallback": "x", "theme": "nope"}));
    assert_eq!(s, "400 Bad Request", "{v}");
    let login = Caller::Login { username: "ada".into() };
    let (s, v) = call("/cli/zen/widget/shot", false, &login, json!({"files": fleet_grid_folder(), "fallback": "x"}));
    assert_eq!((s.as_str(), v["error"].as_str()), ("403 Forbidden", Some("zen_local_only")), "{v}");
    let after: Vec<(String, Option<String>)> = e.f.gardens().into_iter().map(|g| (g.id.clone(), disk(&e.f, &g.id))).collect();
    assert_eq!(before, after, "a shot never touches a Garden");
    assert!(e.f.find_garden("Widget preview").is_none(), "nor makes the preview Garden");
}

#[test]
fn the_shot_route_is_a_post_folder_row() {
    assert!(crate::zen_agent_routes::FOLDER_ROUTES.contains(&"/cli/zen/widget/shot"));
    assert!(crate::zen_routes::POST_ROUTES.contains(&"/cli/zen/widget/shot"));
    assert_eq!(crate::zen_routes::max_body("/cli/zen/widget/shot"), k2_core::zen::widget_folder::MAX_FOLDER_BODY);
}
