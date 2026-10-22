//! Garden access (prd-garden-access-v1 A2 + A3) through the real gates:
//! `zen_routes::handle`, `zen_access_routes::handle`, the widget door and
//! the agent data door. Temp HOME, scoped DB, the switch on for the test
//! and back off after (the env lock TempHome holds keeps it to this test).
//! Fail loudly.

use std::collections::HashMap;

use serde_json::{json, Value as J};

use k2_core::garden_data::authors::{self, CallerKind, StampSource};
use k2_core::garden_data::MadeBy;
use k2_core::zen::garden_access::{self as ga, Actor, Change, Via};
use k2_core::zen::{ZenFile, ZenFiles};

use crate::sidecar_routes::{AgentCaller, Caller};

/// One page placing custom widget `board` as `b1`.
const PAGE: &str = "schema = 1\n\n[layout]\nkind = \"columns\"\n[[layout.column]]\nsize = 100\nmin-width = 200\n\n[[widget]]\nid = \"b1\"\nkind = \"custom\"\nwidget = \"board\"\ncolumn = 0\n";

/// Turns the switch on; off again on drop (even on a panic).
struct SwitchOn;

impl SwitchOn {
    fn new() -> Self {
        ga::set_switch(true);
        SwitchOn
    }
}

impl Drop for SwitchOn {
    fn drop(&mut self) {
        ga::set_switch(false);
    }
}

struct Env {
    _switch: Option<SwitchOn>,
    _db: k2_core::db::TestDbGuard,
    _home: crate::test_support::TempHome,
    f: ZenFiles,
    /// Garden 1 (places `board` as `b1`).
    g1: String,
}

fn manifest(caps: &[&str]) -> String {
    let caps: Vec<String> = caps.iter().map(|c| format!("\"{c}\"")).collect();
    format!("{{\n  \"schema\": 1,\n  \"name\": \"Board\",\n  \"entry\": \"index.html\",\n  \"caps\": [{}]\n}}\n", caps.join(", "))
}

/// Zen set up, Garden 1 placing `board` (with `caps`), workspaces `w-ed`,
/// `w-other`, `w-third` with handles, and the switch on when `on`.
fn env_with(on: bool, caps: &[&str]) -> Env {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        for (id, handle) in [("w-ed", "editor"), ("w-other", "other"), ("w-third", "third")] {
            let dir = home.path().join(format!("ga-{handle}"));
            std::fs::create_dir_all(&dir).expect("workspace dir");
            conn.execute(
                "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
                rusqlite::params![id, handle, dir.to_string_lossy().to_string()],
            )
            .expect("workspace row");
        }
    }
    let f = ZenFiles::local();
    f.setup().expect("zen setup");
    f.new_widget("board", Some("hello")).expect("widget folder");
    std::fs::write(f.widget_dir("board").join("manifest.json"), manifest(caps)).expect("manifest");
    let g1 = f.gardens()[0].id.clone();
    std::fs::write(f.path_of(&ZenFile::Garden(g1.clone())), PAGE).expect("garden page");
    crate::zen_routes::refresh_and_emit().expect("refresh");
    Env { _switch: on.then(SwitchOn::new), _db: db, _home: home, f, g1 }
}

fn env() -> Env {
    env_with(true, &["store:read", "store:write", "thread:post", "files:read"])
}

/// A live agent passport (its session is in the session map).
fn agent(id: &str) -> Caller {
    Caller::Agent(AgentCaller { project_id: id.into(), map_key: Some(format!("{id}-pinned")), args: vec![], sandboxed: false })
}

fn list(garden: &str, project: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    assert!(ga::add(&conn, garden, project, &Change::new(Via::Settings, &Actor::Owner, 1)).expect("add"), "newly listed");
}

fn zen(path: &str, owner: bool, caller: &Caller, body: J) -> (String, J) {
    let r = crate::zen_routes::handle(path, owner, caller, "test", &HashMap::new(), body.to_string().as_bytes());
    let v: J = serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{path}: not JSON ({e}): {}", r.body));
    (r.status.to_string(), v)
}

fn access(path: &str, is_post: bool, owner: bool, caller: &Caller, q: &[(&str, &str)], body: J) -> (String, J) {
    let p: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let raw = if is_post { body.to_string().into_bytes() } else { Vec::new() };
    let r = crate::zen_access_routes::handle(path, is_post, owner, caller, "test", &p, &raw);
    let v: J = serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{path}: not JSON ({e}): {}", r.body));
    (r.status.to_string(), v)
}

fn events(garden: &str) -> Vec<ga::Event> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    ga::events_for_garden(&conn, garden, &[], 500, None).expect("events")
}

fn widget_events(widget: &str) -> Vec<ga::Event> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    ga::events_for_widget(&conn, widget, 500, None).expect("widget events")
}

fn page(accent: &str) -> String {
    format!("{PAGE}\n[colors.light]\naccent = \"{accent}\"\n")
}

fn disk(f: &ZenFiles, id: &str) -> String {
    std::fs::read_to_string(f.path_of(&ZenFile::Garden(id.into()))).expect("garden file")
}

fn maker(f: &ZenFiles, widget: &str) -> MadeBy {
    let hash = f.widget_status(widget).hash().to_string();
    assert_eq!(hash.len(), 64, "{widget} bundles clean");
    let db = k2_core::db::shared();
    let conn = db.lock();
    authors::maker_of(&conn, widget, &hash).expect("maker")
}

fn needed(v: &J, garden: &str) {
    assert_eq!(v["error"], "garden_access_needed", "{v}");
    assert_eq!(v["garden"], garden, "{v}");
    assert!(v.get("card").is_some(), "the refusal names the card (or null): {v}");
}

// ── off = 0.46.3 ─────────────────────────────────────────────────────

#[test]
fn with_the_switch_off_the_new_routes_are_unknown_and_the_old_paths_run() {
    let e = env_with(false, &["store:read"]);
    assert!(!ga::enabled());
    for p in crate::zen_access_routes::ROUTES {
        assert!(!crate::zen_access_routes::route_live(p), "{p} has no live row while off");
        assert!(crate::routes::route_policy::live_row(p).is_none(), "{p}: no row while off");
    }
    let (s, v) = access(crate::zen_access_routes::ACCESS_ROUTE, false, true, &Caller::Owner, &[("garden", &e.g1)], json!({}));
    assert_eq!((s.as_str(), v["error"].as_str()), ("404 Not Found", Some("unknown zen route")), "{v}");
    // garden/new takes today's body only.
    let (s, v) = zen("/cli/zen/garden/new", true, &Caller::Owner, json!({"name": "X", "page": "schema = 1\n"}));
    assert_eq!((s.as_str(), v["error"].as_str()), ("400 Bad Request", Some("bad_request")), "{v}");
    // An agent without "Manage Gardens" still proposes, the claim still works.
    let (s, v) = zen("/cli/zen/garden/put", false, &agent("w-ed"), json!({"garden": e.g1, "text": page("#123456")}));
    assert_eq!((s.as_str(), v["proposed"].as_bool()), ("200 OK", Some(true)), "{v}");
    let (s, v) = zen("/cli/zen/widget/claim", true, &Caller::Owner, json!({"widget": "board"}));
    assert_eq!(s, "200 OK", "{v}");
    // Nothing was written to the access tables.
    assert!(events(&e.g1).is_empty(), "no History while off");
}

#[test]
fn with_the_switch_on_the_routes_have_rows() {
    let _e = env();
    for p in crate::zen_access_routes::ROUTES {
        assert!(crate::routes::route_policy::live_row(p).is_some(), "{p} is live with the switch on");
    }
}

// ── G10–G12: writes ──────────────────────────────────────────────────

#[test]
fn an_unlisted_agent_is_refused_and_a_listed_one_writes_and_logs_once() {
    let e = env();
    let before = disk(&e.f, &e.g1);
    let (s, v) = zen("/cli/zen/garden/put", false, &agent("w-ed"), json!({"garden": e.g1, "text": page("#123456")}));
    assert_eq!(s, "403 Forbidden", "{v}");
    needed(&v, &e.g1);
    assert_eq!(disk(&e.f, &e.g1), before, "nothing written, nothing proposed");
    let db_props = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::zen::agent_edit::list(&conn, true).expect("proposals")
    };
    assert!(db_props.is_empty(), "no proposal with the switch on: {db_props:?}");
    list(&e.g1, "w-ed");
    let n0 = events(&e.g1).len();
    let (s, v) = zen("/cli/zen/garden/put", false, &agent("w-ed"), json!({"garden": e.g1, "text": page("#123456")}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(disk(&e.f, &e.g1), page("#123456"));
    let ev = events(&e.g1);
    assert_eq!(ev.len(), n0 + 1, "one History line: {ev:?}");
    assert_eq!((ev[0].kind.as_str(), ev[0].actor.as_str()), ("changed", "agent:w-ed"));
    // Each Garden write route: refused for an unlisted agent, allowed for a listed one.
    for (path, body) in [
        ("/cli/zen/garden/rename", json!({"garden": e.g1, "name": "Renamed"})),
        ("/cli/zen/garden/reorder", json!({"garden": e.g1, "to": 1})),
        ("/cli/zen/theme/set", json!({"garden": e.g1, "name": k2_core::zen::DEFAULT_THEME})),
        ("/cli/zen/garden/template", json!({"garden": e.g1, "template": "blank", "force": true})),
    ] {
        let (s, v) = zen(path, false, &agent("w-other"), body.clone());
        assert_eq!(s, "403 Forbidden", "{path}: {v}");
        needed(&v, &e.g1);
        let (s, v) = zen(path, false, &agent("w-ed"), body);
        assert_eq!(s, "200 OK", "{path}: {v}");
    }
    let kinds: Vec<&str> = events(&e.g1).iter().map(|x| x.kind.as_str()).collect();
    // reset {garden}: refused unlisted; listed gets past the gate (the
    // handler may still say there's no snapshot to restore).
    let (s, v) = zen("/cli/zen/reset", false, &agent("w-other"), json!({"garden": e.g1}));
    assert_eq!(s, "403 Forbidden", "{v}");
    needed(&v, &e.g1);
    let (s, v) = zen("/cli/zen/reset", false, &agent("w-ed"), json!({"garden": e.g1}));
    assert_ne!(s, "403 Forbidden", "{v}");
    for k in ["renamed", "theme", "template"] {
        assert!(kinds.contains(&k), "{k} logged: {kinds:?}");
    }
    // The person always edits, and is logged as the person.
    let (s, v) = zen("/cli/zen/garden/put", true, &Caller::Owner, json!({"garden": e.g1, "text": page("#333366")}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(events(&e.g1)[0].actor, "owner");
}

#[test]
fn a_locked_garden_is_423_before_the_list() {
    let e = env();
    list(&e.g1, "w-ed");
    e.f.set_garden_lock(&e.g1, true).expect("lock");
    let (s, v) = zen("/cli/zen/garden/put", false, &agent("w-ed"), json!({"garden": e.g1, "text": page("#123456")}));
    assert_eq!((s.as_str(), v["error"].as_str()), ("423 Locked", Some("garden_locked")), "{v}");
    let (s, v) = zen("/cli/zen/garden/rename", false, &agent("w-other"), json!({"garden": e.g1, "name": "x"}));
    assert_eq!((s.as_str(), v["error"].as_str()), ("423 Locked", Some("garden_locked")), "{v}");
}

#[test]
fn the_computer_wide_look_needs_an_agent_on_at_least_one_list() {
    let e = env();
    for (path, body) in [
        ("/cli/zen/theme/set", json!({"name": k2_core::zen::DEFAULT_THEME})),
        ("/cli/zen/theme/next", json!({})),
        ("/cli/zen/reset", json!({})),
        ("/cli/zen/reset", json!({"theme": k2_core::zen::DEFAULT_THEME})),
    ] {
        let (s, v) = zen(path, false, &agent("w-ed"), body);
        assert_eq!(s, "403 Forbidden", "{path}: {v}");
        assert_eq!(v["error"], "garden_access_needed", "{v}");
        assert!(v["garden"].is_null(), "garden: null for the computer-wide look: {v}");
    }
    list(&e.g1, "w-ed");
    let (s, v) = zen("/cli/zen/theme/set", false, &agent("w-ed"), json!({"name": k2_core::zen::DEFAULT_THEME}));
    assert_eq!(s, "200 OK", "{v}");
    let look = events(ga::COMPUTER_WIDE);
    assert!(look.iter().any(|x| x.kind.as_str() == "theme" && x.actor == "agent:w-ed"), "{look:?}");
}

#[test]
fn only_the_person_deletes_and_an_agent_asks_with_a_card() {
    let e = env();
    let g2 = e.f.new_garden("Second", None, None, None).expect("second garden").id;
    // Not listed, not the maker: it must ask for access first.
    let (s, v) = zen("/cli/zen/garden/delete", false, &agent("w-other"), json!({"garden": g2}));
    assert_eq!(s, "403 Forbidden", "{v}");
    needed(&v, &g2);
    // Listed: the person deletes; K2 asks them.
    list(&g2, "w-ed");
    let (s, v) = zen("/cli/zen/garden/delete", false, &agent("w-ed"), json!({"garden": g2}));
    assert_eq!((s.as_str(), v["error"].as_str()), ("403 Forbidden", Some("garden_delete_person")), "{v}");
    assert!(v.get("card").is_some(), "{v}");
    assert_ne!(v["error"], "person_only", "never the gardens.manage wall's code");
    assert!(e.f.find_garden(&g2).is_some(), "still there");
    // The maker may ask too.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ga::record_made(&conn, &g2, "w-third", 1).expect("made");
    }
    let (_, v) = zen("/cli/zen/garden/delete", false, &agent("w-third"), json!({"garden": g2}));
    assert_eq!(v["error"], "garden_delete_person", "{v}");
    // The person deletes.
    let (s, v) = zen("/cli/zen/garden/delete", true, &Caller::Owner, json!({"garden": g2}));
    assert_eq!(s, "200 OK", "{v}");
    assert!(e.f.find_garden(&g2).is_none());
}

#[test]
fn a_widget_write_needs_every_placing_garden_and_a_draft_is_open() {
    let e = env();
    let g2 = e.f.new_garden("Second", None, None, None).expect("second").id;
    std::fs::write(e.f.path_of(&ZenFile::Garden(g2.clone())), PAGE).expect("second page");
    crate::zen_routes::refresh_and_emit().expect("refresh");
    list(&e.g1, "w-ed");
    let body = json!({"widget": "board", "file": "index.html", "text": "<!doctype html><p>v2</p>"});
    let (s, v) = zen("/cli/zen/widget/put", false, &agent("w-ed"), body.clone());
    assert_eq!(s, "403 Forbidden", "{v}");
    needed(&v, &g2);
    // reset {widget} is a widget write too.
    let (s, v) = zen("/cli/zen/reset", false, &agent("w-ed"), json!({"widget": "board"}));
    assert_eq!(s, "403 Forbidden", "{v}");
    needed(&v, &g2);
    list(&g2, "w-ed");
    let (s, v) = zen("/cli/zen/widget/put", false, &agent("w-ed"), body);
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v["keepAsMine"], false, "Keep as mine is retired: {v}");
    assert_eq!(maker(&e.f, "board"), MadeBy::Agent { project_id: "w-ed".into() }, "the writer stamps");
    let wev = widget_events("board");
    assert_eq!(wev.iter().filter(|x| x.kind.as_str() == "widget_changed").count(), 1, "{wev:?}");
    assert!(wev[0].garden_id.is_none(), "a widget write is one widget row: {wev:?}");
    // The Garden's History shows the placed widget's line.
    let db_ev = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ga::events_for_garden(&conn, &e.g1, &["board"], 50, None).expect("garden events")
    };
    assert!(db_ev.iter().any(|x| x.kind.as_str() == "widget_changed"), "{db_ev:?}");
    // A draft placed nowhere: any local agent writes it.
    e.f.new_widget("draft", Some("hello")).expect("draft");
    let (s, v) = zen(
        "/cli/zen/widget/put",
        false,
        &agent("w-other"),
        json!({"widget": "draft", "file": "index.html", "text": "<!doctype html><p>d</p>"}),
    );
    assert_eq!(s, "200 OK", "{v}");
}

#[test]
fn identity_shell_tabs_count_and_dead_or_sandboxed_sessions_do_not() {
    let e = env();
    list(&e.g1, "w-ed");
    let shell = Caller::Shell { project_id: "w-ed".into() };
    let (s, v) = zen("/cli/zen/garden/rename", false, &shell, json!({"garden": e.g1, "name": "By shell"}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(events(&e.g1)[0].actor, "shell:w-ed");
    let dead = Caller::Agent(AgentCaller { project_id: "w-ed".into(), map_key: None, args: vec![], sandboxed: false });
    let (s, v) = zen("/cli/zen/garden/rename", false, &dead, json!({"garden": e.g1, "name": "x"}));
    assert_eq!((s.as_str(), v["error"].as_str()), ("403 Forbidden", Some("session_gone")), "{v}");
    let boxed = Caller::Agent(AgentCaller { project_id: "w-ed".into(), map_key: Some("k".into()), args: vec![], sandboxed: true });
    let (s, v) = zen("/cli/zen/garden/rename", false, &boxed, json!({"garden": e.g1, "name": "x"}));
    assert_eq!((s.as_str(), v["error"].as_str()), ("403 Forbidden", Some("zen_local_only")), "{v}");
    let login = Caller::Login { username: "member".into() };
    let (s, v) = zen("/cli/zen/garden/rename", false, &login, json!({"garden": e.g1, "name": "x"}));
    assert_eq!((s.as_str(), v["error"].as_str()), ("403 Forbidden", Some("zen_local_only")), "{v}");
    // The resolver itself.
    use crate::zen_access_routes::{garden_access_may, May};
    assert_eq!(garden_access_may(&Caller::Owner, &e.g1), May::Edit);
    assert_eq!(garden_access_may(&agent("w-ed"), &e.g1), May::Edit);
    assert_eq!(garden_access_may(&agent("w-other"), &e.g1), May::Ask);
    assert_eq!(garden_access_may(&dead, &e.g1), May::Refused("session_gone"));
    assert_eq!(garden_access_may(&boxed, &e.g1), May::Refused("zen_local_only"));
}

#[test]
fn proposals_and_the_claim_are_retired_and_preview_opens() {
    let e = env();
    let (s, v) = zen("/cli/zen/widget/claim", true, &Caller::Owner, json!({"widget": "board"}));
    assert_eq!((s.as_str(), v["error"].as_str()), ("410 Gone", Some("retired")), "{v}");
    let (s, v) = zen("/cli/zen/proposals/approve", false, &agent("w-ed"), json!({"id": "p-1"}));
    assert_eq!((s.as_str(), v["error"].as_str()), ("410 Gone", Some("retired")), "{v}");
    // An unlisted agent previews (Always only on K2's preview Garden).
    let files = json!([
        {"name": "manifest.json", "text": manifest(&["store:read"])},
        {"name": "index.html", "text": "<!doctype html><p>p</p>"},
    ]);
    let (s, v) = zen("/cli/zen/widget/preview", false, &agent("w-other"), json!({"widget": "pv", "files": files}));
    assert_eq!(s, "200 OK", "{v}");
    let folder = v["preview"].as_str().expect("preview folder").to_string();
    let pg = v["garden"]["id"].as_str().expect("preview garden").to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    let hash = e.f.widget_status(&folder).hash().to_string();
    assert!(!k2_core::garden_data::signer_gate::passes(&conn, &pg, &folder, &hash).expect("gate"), "an agent's preview runs Always only");
}

#[test]
fn a_persons_approve_of_a_pending_proposal_lists_the_proposer_and_stamps_the_person() {
    let e = env();
    // Made while the switch was off.
    ga::set_switch(false);
    let (_, v) = zen("/cli/zen/garden/put", false, &agent("w-ed"), json!({"garden": e.g1, "text": page("#0f0f0f")}));
    let id = v["proposal"]["id"].as_str().expect("proposal").to_string();
    ga::set_switch(true);
    let (s, v) = zen("/cli/zen/proposals/approve", true, &Caller::Owner, json!({"id": id}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v["access"]["listed"], json!([e.g1]), "{v}");
    let db = k2_core::db::shared();
    let conn = db.lock();
    assert!(ga::is_listed(&conn, &e.g1, "w-ed").expect("listed"));
}

// ── G9: sign and vouch ───────────────────────────────────────────────

#[test]
fn only_the_person_or_a_listed_agent_vouches_over_another_signer() {
    let e = env();
    // An unlisted agent signs first: it stamps, but gives no sight.
    let (s, v) = zen("/cli/zen/widget/sign", false, &agent("w-other"), json!({"widget": "board"}));
    assert_eq!((s.as_str(), v["stamped"].as_bool(), v["vouched"].as_bool()), ("200 OK", Some(true), Some(false)), "{v}");
    let hash = e.f.widget_status("board").hash().to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        assert!(!k2_core::garden_data::signer_gate::passes(&conn, &e.g1, "board", &hash).expect("gate"));
    }
    // Another unlisted agent can't take it over.
    let (_, v) = zen("/cli/zen/widget/sign", false, &agent("w-third"), json!({"widget": "board"}));
    assert_eq!(v["stamped"], false, "{v}");
    assert_eq!(maker(&e.f, "board"), MadeBy::Agent { project_id: "w-other".into() });
    // A listed agent vouches.
    list(&e.g1, "w-ed");
    let (_, v) = zen("/cli/zen/widget/sign", false, &agent("w-ed"), json!({"widget": "board"}));
    assert_eq!((v["stamped"].as_bool(), v["vouched"].as_bool()), (Some(true), Some(true)), "{v}");
    assert_eq!(maker(&e.f, "board"), MadeBy::Agent { project_id: "w-ed".into() });
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        assert!(k2_core::garden_data::signer_gate::passes(&conn, &e.g1, "board", &hash).expect("gate"));
    }
    // The person vouches, from the History modal of a Garden.
    let (_, v) = zen("/cli/zen/widget/sign", true, &Caller::Owner, json!({"widget": "board", "garden": e.g1}));
    assert_eq!(v["stamped"], true, "{v}");
    assert_eq!(maker(&e.f, "board"), MadeBy::Owner);
    let signed: Vec<_> = widget_events("board").into_iter().filter(|x| x.kind.as_str() == "signed").collect();
    assert_eq!(signed.len(), 3, "{signed:?}");
    assert_eq!(signed[0].garden_id.as_deref(), Some(e.g1.as_str()), "the modal's Garden is on the line");
}

// ── G13: making a Garden ─────────────────────────────────────────────

fn new_widget_files(text: &str) -> J {
    json!([
        {"name": "manifest.json", "text": manifest(&["store:read", "store:write", "files:read"])},
        {"name": "index.html", "text": text},
    ])
}

fn tally_page(widget: &str) -> String {
    format!(
        "schema = 1\n\n[layout]\nkind = \"columns\"\n[[layout.column]]\nsize = 100\nmin-width = 200\n\n[[widget]]\nid = \"t1\"\nkind = \"custom\"\nwidget = \"{widget}\"\ncolumn = 0\n"
    )
}

#[test]
fn an_agent_makes_a_garden_with_page_and_widgets_and_is_not_listed() {
    let e = env();
    let body = json!({
        "name": "Sales board",
        "page": tally_page("tally"),
        "widgets": [{"widget": "tally", "files": new_widget_files("<!doctype html><p>t</p>")}],
    });
    let (s, v) = zen("/cli/zen/garden/new", false, &agent("w-ed"), body);
    assert_eq!(s, "200 OK", "{v}");
    assert!(v.get("card").is_some(), "the card in the same answer: {v}");
    assert_eq!(v["made"], true);
    let g = v["garden"]["id"].as_str().expect("garden id").to_string();
    assert_eq!(disk(&e.f, &g), tally_page("tally"));
    assert_eq!(maker(&e.f, "tally"), MadeBy::Agent { project_id: "w-ed".into() }, "the maker stamps its widgets");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        assert!(!ga::is_listed(&conn, &g, "w-ed").expect("listed?"), "making never lists");
        assert_eq!(ga::made(&conn, &g).expect("made").expect("made row").made_by_project, "w-ed");
        let hash = e.f.widget_status("tally").hash().to_string();
        assert!(!k2_core::garden_data::signer_gate::passes(&conn, &g, "tally", &hash).expect("gate"), "Always only until Allow");
    }
    assert!(events(&g).iter().any(|x| x.kind.as_str() == "made" && x.actor == "agent:w-ed"));
    // Its next edit needs the card.
    let (s, v) = zen("/cli/zen/garden/rename", false, &agent("w-ed"), json!({"garden": g, "name": "Sales"}));
    assert_eq!(s, "403 Forbidden", "{v}");
    needed(&v, &g);
    // After Allow: it edits, and its widgets pass with no re-sign.
    list(&g, "w-ed");
    let (s, v) = zen("/cli/zen/garden/rename", false, &agent("w-ed"), json!({"garden": g, "name": "Sales"}));
    assert_eq!(s, "200 OK", "{v}");
    let db = k2_core::db::shared();
    let conn = db.lock();
    let hash = e.f.widget_status("tally").hash().to_string();
    assert!(k2_core::garden_data::signer_gate::passes(&conn, &g, "tally", &hash).expect("gate"));
}

#[test]
fn garden_new_is_atomic_refuses_existing_widgets_and_has_a_daily_budget() {
    let e = env();
    let n = e.f.gardens().len();
    // An existing widget name: 409, nothing made.
    let body = json!({"name": "Dup", "widgets": [{"widget": "board", "files": new_widget_files("<p>x</p>")}]});
    let (s, v) = zen("/cli/zen/garden/new", false, &agent("w-ed"), body);
    assert_eq!((s.as_str(), v["error"].as_str()), ("409 Conflict", Some("widget_exists")), "{v}");
    // A bad page: nothing lands, the new widget folder goes too.
    let body = json!({
        "name": "Broken",
        "page": "schema = 1\nnot_a_key = 3\n",
        "widgets": [{"widget": "fresh", "files": new_widget_files("<!doctype html><p>f</p>")}],
    });
    let (s, v) = zen("/cli/zen/garden/new", false, &agent("w-ed"), body);
    assert_eq!(s, "400 Bad Request", "{v}");
    assert_eq!(e.f.gardens().len(), n, "no Garden left behind");
    assert!(!e.f.widget_status("fresh").exists, "no widget left behind");
    // Five a day per agent; the sixth is 429 with the words.
    for i in 0..5 {
        let (s, v) = zen("/cli/zen/garden/new", false, &agent("w-other"), json!({"name": format!("G{i}")}));
        assert_eq!(s, "200 OK", "{i}: {v}");
    }
    let (s, v) = zen("/cli/zen/garden/new", false, &agent("w-other"), json!({"name": "G6"}));
    assert_eq!((s.as_str(), v["error"].as_str()), ("429 Too Many Requests", Some("garden_budget")), "{v}");
    assert!(v["message"].as_str().is_some_and(|m| m.contains("5 Gardens today")), "{v}");
    // The person has no budget and no card.
    let (s, v) = zen("/cli/zen/garden/new", true, &Caller::Owner, json!({"name": "Mine"}));
    assert_eq!((s.as_str(), v["made"].as_bool()), ("200 OK", Some(false)), "{v}");
    assert!(v["card"].is_null(), "{v}");
}

// ── G14: the list and History routes ─────────────────────────────────

#[test]
fn the_list_routes_answer_the_person_and_tell_an_agent_only_about_itself() {
    use crate::zen_access_routes::{ACCESS_ROUTE, ASK_ROUTE};
    let e = env();
    let (s, v) = access(ACCESS_ROUTE, true, true, &Caller::Owner, &[], json!({"garden": e.g1, "agent": "w-ed", "allow": true}));
    assert_eq!((s.as_str(), v["listed"].as_bool(), v["changed"].as_bool()), ("200 OK", Some(true), Some(true)), "{v}");
    let (s, v) = access(ACCESS_ROUTE, false, true, &Caller::Owner, &[("garden", &e.g1)], json!({}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v["garden"], e.g1.as_str());
    assert_eq!(v["editors"][0]["agent"], "w-ed", "{v}");
    assert_eq!(v["editors"][0]["handle"], "editor", "{v}");
    assert_eq!(v["editors"][0]["addedBy"], "owner", "{v}");
    assert_eq!(v["sees"], "everything");
    // An agent learns only whether it is listed.
    let (_, v) = access(ACCESS_ROUTE, false, false, &agent("w-other"), &[("garden", &e.g1)], json!({}));
    assert_eq!((v["listed"].as_bool(), v.get("editors").is_none()), (Some(false), true), "{v}");
    let (_, v) = access(ACCESS_ROUTE, false, false, &agent("w-ed"), &[("garden", &e.g1)], json!({}));
    assert_eq!(v["listed"], true, "{v}");
    // An agent can't change the list.
    let (s, v) = access(ACCESS_ROUTE, true, false, &agent("w-ed"), &[], json!({"garden": e.g1, "agent": "w-other", "allow": true}));
    assert_eq!((s.as_str(), v["error"].as_str()), ("403 Forbidden", Some("owner_only")), "{v}");
    // Ask: listed → nothing to ask; unlisted → asked through the card.
    let (s, v) = access(ASK_ROUTE, true, false, &agent("w-ed"), &[], json!({"garden": e.g1}));
    assert_eq!((s.as_str(), v["listed"].as_bool()), ("200 OK", Some(true)), "{v}");
    let (s, v) = access(ASK_ROUTE, true, false, &agent("w-other"), &[], json!({"garden": e.g1, "reason": "to fix the board"}));
    assert_eq!((s.as_str(), v["listed"].as_bool()), ("200 OK", Some(false)), "{v}");
    assert!(v.get("card").is_some(), "{v}");
    let (s, v) = access(ASK_ROUTE, true, true, &Caller::Owner, &[], json!({"garden": e.g1}));
    assert_eq!(s, "403 Forbidden", "the person doesn't ask: {v}");
    // Remove: takes effect on the next request.
    let (_, v) = access(ACCESS_ROUTE, true, true, &Caller::Owner, &[], json!({"garden": e.g1, "agent": "editor", "allow": false}));
    assert_eq!((v["listed"].as_bool(), v["changed"].as_bool()), (Some(false), Some(true)), "{v}");
    let (s, _) = zen("/cli/zen/garden/rename", false, &agent("w-ed"), json!({"garden": e.g1, "name": "Gone"}));
    assert_eq!(s, "403 Forbidden");
    let kinds: Vec<&str> = events(&e.g1).iter().map(|x| x.kind.as_str()).collect();
    assert_eq!(kinds.iter().filter(|k| **k == "access_added").count(), 1, "{kinds:?}");
    assert_eq!(kinds.iter().filter(|k| **k == "access_removed").count(), 1, "{kinds:?}");
}

#[test]
fn history_pages_filters_and_answers_only_the_person() {
    use crate::zen_access_routes::{GARDEN_HISTORY_ROUTE, WIDGET_HISTORY_ROUTE};
    let e = env();
    list(&e.g1, "w-ed");
    for i in 0..4 {
        let (s, v) = zen("/cli/zen/garden/rename", false, &agent("w-ed"), json!({"garden": e.g1, "name": format!("N{i}")}));
        assert_eq!(s, "200 OK", "{v}");
    }
    let (s, v) = zen("/cli/zen/widget/sign", true, &Caller::Owner, json!({"widget": "board"}));
    assert_eq!(s, "200 OK", "{v}");
    // Page 1 of 2 renames, then the rest.
    let (s, v) = access(GARDEN_HISTORY_ROUTE, false, true, &Caller::Owner, &[("garden", &e.g1), ("kind", "renamed"), ("limit", "2")], json!({}));
    assert_eq!(s, "200 OK", "{v}");
    let items = v["items"].as_array().expect("items");
    assert_eq!(items.len(), 2, "{v}");
    assert_eq!(items[0]["detail"]["to"], "N3", "newest first: {v}");
    assert_eq!(items[0]["actor"]["kind"], "agent");
    assert_eq!(items[0]["actor"]["handle"], "editor");
    assert_eq!(items[0]["source"], "event");
    let next = v["next"].as_str().expect("next cursor").to_string();
    let (_, v2) = access(GARDEN_HISTORY_ROUTE, false, true, &Caller::Owner, &[("garden", &e.g1), ("kind", "renamed"), ("before", &next)], json!({}));
    assert_eq!(v2["items"].as_array().expect("items").len(), 2, "{v2}");
    assert!(v2["next"].is_null(), "{v2}");
    // The Garden's History includes its placed widget's lines; groups filter.
    let (_, all) = access(GARDEN_HISTORY_ROUTE, false, true, &Caller::Owner, &[("garden", &e.g1), ("kind", "changes")], json!({}));
    let kinds: Vec<&str> = all["items"].as_array().expect("items").iter().filter_map(|i| i["kind"].as_str()).collect();
    assert!(kinds.contains(&"signed") && kinds.contains(&"renamed"), "{kinds:?}");
    assert!(!kinds.contains(&"access_added"), "access isn't a change: {kinds:?}");
    let (_, acc) = access(GARDEN_HISTORY_ROUTE, false, true, &Caller::Owner, &[("garden", &e.g1), ("kind", "access")], json!({}));
    assert_eq!(acc["items"].as_array().expect("items").len(), 1, "{acc}");
    // The widget's History has its own lines only.
    let (_, w) = access(WIDGET_HISTORY_ROUTE, false, true, &Caller::Owner, &[("widget", "board")], json!({}));
    let witems = w["items"].as_array().expect("items");
    let wk: Vec<&str> = witems.iter().filter(|i| i["source"] == "event").filter_map(|i| i["kind"].as_str()).collect();
    assert_eq!(wk, vec!["signed"], "{w}");
    // A6's merge (G29): the setup version no line covers is its own
    // "version saved" line, and nothing else is merged.
    let merged: Vec<(&str, &str)> = witems
        .iter()
        .filter(|i| i["source"] != "event")
        .map(|i| (i["source"].as_str().expect("source"), i["kind"].as_str().expect("kind")))
        .collect();
    assert_eq!(merged, vec![("snapshot", "widget_changed")], "{w}");
    assert!(witems.iter().all(|i| i["widget"] == "board"), "{w}");
    assert_eq!(w["unsigned"], json!([]), "signed by the person: {w}");
    // Agents can't read History; a bad kind is 400.
    for p in [GARDEN_HISTORY_ROUTE, WIDGET_HISTORY_ROUTE] {
        let (s, v) = access(p, false, false, &agent("w-ed"), &[("garden", &e.g1), ("widget", "board")], json!({}));
        assert_eq!((s.as_str(), v["error"].as_str()), ("403 Forbidden", Some("owner_only")), "{p}: {v}");
    }
    let (s, _) = access(GARDEN_HISTORY_ROUTE, false, true, &Caller::Owner, &[("garden", &e.g1), ("kind", "nope")], json!({}));
    assert_eq!(s, "400 Bad Request");
}

#[test]
fn history_lists_unsigned_versions_for_the_modals() {
    use crate::zen_access_routes::GARDEN_HISTORY_ROUTE;
    let e = env();
    let (_, v) = access(GARDEN_HISTORY_ROUTE, false, true, &Caller::Owner, &[("garden", &e.g1)], json!({}));
    assert_eq!(v["unsigned"][0]["widget"], "board", "the hello starter is unsigned: {v}");
    zen("/cli/zen/widget/sign", true, &Caller::Owner, json!({"widget": "board"}));
    let (_, v) = access(GARDEN_HISTORY_ROUTE, false, true, &Caller::Owner, &[("garden", &e.g1)], json!({}));
    assert_eq!(v["unsigned"], json!([]), "{v}");
}

// ── A3: the signer gate on the widget and agent data doors ───────────

const APP: &str = "POST /cli/zen/call HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: tauri://localhost\r\n\r\n";

fn call(e: &Env, verb: &str, args: J) -> (u16, J) {
    let hash = e.f.garden_widget_status(&e.g1, "board").hash().to_string();
    let body = json!({ "garden": e.g1, "placement": "b1", "hash": hash, "verb": verb, "args": args });
    let r = crate::zen_data_routes::handle("/cli/zen/call", crate::zen_data_routes::DataWho::Owner, body.to_string().as_bytes(), &HashMap::new(), APP);
    let code: u16 = r.status.split(' ').next().expect("status").parse().expect("code");
    (code, serde_json::from_str(&r.body).unwrap_or_else(|err| panic!("{verb}: {err}: {}", r.body)))
}

/// The resolved custom placement `id` anywhere in a page answer.
fn find_placement(v: &J, id: &str) -> Option<J> {
    match v {
        J::Object(o) => {
            if o.get("id").and_then(J::as_str) == Some(id) && o.get("kind").and_then(J::as_str) == Some("custom") {
                return Some(v.clone());
            }
            o.values().find_map(|x| find_placement(x, id))
        }
        J::Array(a) => a.iter().find_map(|x| find_placement(x, id)),
        _ => None,
    }
}

fn code(v: &J) -> &str {
    v["error"]["code"].as_str().or_else(|| v["error"].as_str()).unwrap_or("")
}

fn stamp(e: &Env, by: MadeBy) {
    let hash = e.f.widget_status("board").hash().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    let kind = if by == MadeBy::Owner { CallerKind::Owner } else { CallerKind::Agent };
    authors::vouch(&conn, "board", &hash, &by, StampSource::Sign, kind, 2).expect("stamp");
}

#[test]
fn an_unsigned_version_keeps_its_own_store_and_nothing_else() {
    let e = env();
    // Unsigned: its own keyspace works (author_unknown is gone).
    let (c, v) = call(&e, "store.put", json!(["k", 1]));
    assert_eq!(c, 200, "{v}");
    let (c, v) = call(&e, "store.get", json!(["k"]));
    assert_eq!((c, v["result"]["value"].clone()), (200, json!(1)), "{v}");
    // Everything that acts as the person: needs_editor.
    for (verb, args) in [("viewer.get", json!([])), ("files.list", json!([{"workspace": "editor"}]))] {
        let (c, v) = call(&e, verb, args);
        assert_eq!((c, code(&v)), (403, "needs_editor"), "{verb}: {v}");
    }
    // The renderer gets Always-only caps and `signed: false`.
    let page = crate::zen_sync_routes::get(&e.f, Some(&e.g1), None, &k2_core::zen::widget_access::WidgetPauses::new()).expect("page");
    let w = find_placement(&page, "b1").unwrap_or_else(|| panic!("no b1 in {page}"));
    assert_eq!(w["signed"], false, "{w}");
    assert_eq!(w["caps"], json!(["store:read", "store:write"]), "{w}");
    // The Thread door (G15a: the gate, never the agent's list).
    let hash = e.f.garden_widget_status(&e.g1, "board").hash().to_string();
    let r = k2_core::zen::widget_access::check_placement_cap(&e.f, &e.g1, "b1", "board", &hash, "thread:post");
    assert_eq!(r.map_err(|x| x.code), Err("needs_editor"));
    // An unlisted agent's stamp: still needs_editor.
    stamp(&e, MadeBy::Agent { project_id: "w-other".into() });
    let (c, v) = call(&e, "viewer.get", json!([]));
    assert_eq!((c, code(&v)), (403, "needs_editor"), "{v}");
    // Allow it: everything, with no re-sign; Remove: back to Always.
    list(&e.g1, "w-other");
    let (c, v) = call(&e, "viewer.get", json!([]));
    assert_eq!(c, 200, "{v}");
    assert!(k2_core::zen::widget_access::check_placement_cap(&e.f, &e.g1, "b1", "board", &hash, "thread:post").is_ok());
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ga::remove(&conn, &e.g1, "w-other", &Change::new(Via::Settings, &Actor::Owner, 3)).expect("remove");
    }
    let (c, v) = call(&e, "viewer.get", json!([]));
    assert_eq!((c, code(&v)), (403, "needs_editor"), "{v}");
}

#[test]
fn a_signed_agent_widget_acts_as_the_person_in_any_room() {
    let e = env();
    list(&e.g1, "w-ed");
    stamp(&e, MadeBy::Agent { project_id: "w-ed".into() });
    // files.list in another agent's room: not refused for reach (the rows'
    // own handler may answer anything but not_bound / needs_editor).
    let (c, v) = call(&e, "files.list", json!([{"workspace": "third"}]));
    assert!(!matches!(code(&v), "needs_editor" | "not_bound" | "author_unknown"), "{c}: {v}");
    // widget.self: last signer, editors, signed.
    let (c, v) = call(&e, "widget.self", json!([]));
    assert_eq!(c, 200, "{v}");
    assert_eq!(v["result"]["author"]["handle"], "editor", "{v}");
    assert_eq!(v["result"]["editors"], json!(["editor"]), "{v}");
    assert_eq!(v["result"]["signed"], true, "{v}");
    assert_eq!(v["result"]["garden"], e.g1.as_str(), "garden stays the id: {v}");
}

#[test]
fn a_built_in_always_passes_so_use_is_never_gated_by_an_agents_list() {
    let e = env();
    let db = k2_core::db::shared();
    let conn = db.lock();
    assert!(k2_core::garden_data::signer_gate::passes(&conn, &e.g1, "k2:diary@1", &"a".repeat(64)).expect("gate"));
}

#[test]
fn the_agent_data_door_is_full_for_an_editor_of_every_placing_garden() {
    use crate::zen_data_routes::DataWho;
    let e = env();
    let get = |who: DataWho| {
        let p: HashMap<String, String> = [("widget", "board"), ("key", "k")].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        let r = crate::zen_data_routes::handle("/cli/zen/data/get", who, b"", &p, "");
        let c: u16 = r.status.split(' ').next().expect("status").parse().expect("code");
        (c, serde_json::from_str::<J>(&r.body).unwrap_or_else(|err| panic!("{err}: {}", r.body)))
    };
    let (c, v) = get(DataWho::Agent { project_id: "w-ed".into() });
    assert_eq!((c, code(&v)), (403, "not_granted"), "{v}");
    list(&e.g1, "w-ed");
    let (c, v) = get(DataWho::Agent { project_id: "w-ed".into() });
    assert_eq!(c, 200, "{v}");
    // A second placing Garden it isn't on: back to its grants.
    let g2 = e.f.new_garden("Second", None, None, None).expect("second").id;
    std::fs::write(e.f.path_of(&ZenFile::Garden(g2)), PAGE).expect("second page");
    crate::zen_routes::refresh_and_emit().expect("refresh");
    let (c, v) = get(DataWho::Agent { project_id: "w-ed".into() });
    assert_eq!((c, code(&v)), (403, "not_granted"), "{v}");
}

// ── Deleted Gardens, restore, View this version, list decorations ────

#[test]
fn a_deleted_garden_lists_restores_with_its_editors_and_logs() {
    use crate::zen_access_routes::{DELETED_ROUTE, RESTORE_ROUTE};
    let e = env();
    let g2 = e.f.new_garden("Second", None, None, None).expect("second").id;
    std::fs::write(e.f.path_of(&ZenFile::Garden(g2.clone())), page("#123123")).expect("page");
    list(&g2, "w-ed");
    let (s, v) = zen("/cli/zen/garden/delete", true, &Caller::Owner, json!({"garden": g2}));
    assert_eq!(s, "200 OK", "{v}");
    let (s, v) = access(DELETED_ROUTE, false, true, &Caller::Owner, &[], json!({}));
    assert_eq!(s, "200 OK", "{v}");
    let row = v["gardens"].as_array().and_then(|a| a.iter().find(|g| g["id"] == g2.as_str()).cloned()).unwrap_or_else(|| panic!("{v}"));
    assert_eq!(row["name"], "Second");
    assert!(row["deletedAt"].as_i64().is_some(), "{row}");
    // Agents can't see or restore; a GET on restore is 405.
    let (s, _) = access(DELETED_ROUTE, false, false, &agent("w-ed"), &[], json!({}));
    assert_eq!(s, "403 Forbidden");
    let (s, _) = access(RESTORE_ROUTE, true, false, &agent("w-ed"), &[], json!({"garden": g2}));
    assert_eq!(s, "403 Forbidden");
    let (s, _) = access(RESTORE_ROUTE, false, true, &Caller::Owner, &[], json!({}));
    assert_eq!(s, "405 Method Not Allowed");
    // The person restores: page, list rows and a History line come back.
    let (s, v) = access(RESTORE_ROUTE, true, true, &Caller::Owner, &[], json!({"garden": g2}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v["garden"]["id"], g2.as_str());
    assert_eq!(disk(&e.f, &g2), page("#123123"));
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        assert!(ga::is_listed(&conn, &g2, "w-ed").expect("listed"), "editors come back from access.json");
    }
    assert!(events(&g2).iter().any(|x| x.kind.as_str() == "restored"), "{:?}", events(&g2));
    let (_, v) = access(DELETED_ROUTE, false, true, &Caller::Owner, &[], json!({}));
    assert!(v["gardens"].as_array().is_some_and(|a| a.iter().all(|g| g["id"] != g2.as_str())), "no longer deleted: {v}");
    let (s, v) = access(RESTORE_ROUTE, true, true, &Caller::Owner, &[], json!({"garden": g2}));
    assert_eq!((s.as_str(), v["error"].as_str()), ("409 Conflict", Some("garden_exists")), "{v}");
}

#[test]
fn view_this_version_reads_a_kept_page_and_widget() {
    use crate::zen_access_routes::SNAPSHOT_ROUTE;
    let e = env();
    // A person's put keeps the old page as a snapshot.
    let (s, v) = zen("/cli/zen/garden/put", true, &Caller::Owner, json!({"garden": e.g1, "text": page("#456456")}));
    assert_eq!(s, "200 OK", "{v}");
    let snap = v["snapshot"].as_str().expect("snapshot name").to_string();
    let file = format!("gardens/{}.toml", e.g1);
    let (s, v) = access(SNAPSHOT_ROUTE, false, true, &Caller::Owner, &[("file", &file), ("name", &snap)], json!({}));
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v["text"], PAGE, "{v}");
    let (s, _) = access(SNAPSHOT_ROUTE, false, false, &agent("w-ed"), &[("file", &file), ("name", &snap)], json!({}));
    assert_eq!(s, "403 Forbidden");
    let (s, _) = access(SNAPSHOT_ROUTE, false, true, &Caller::Owner, &[("file", &file), ("name", "nope.toml")], json!({}));
    assert_eq!(s, "404 Not Found");
    // A widget's code snapshot.
    let (s, v) = zen("/cli/zen/widget/put", true, &Caller::Owner, json!({"widget": "board", "file": "index.html", "text": "<!doctype html><p>2</p>"}));
    assert_eq!(s, "200 OK", "{v}");
    let snaps = e.f.widget_snapshots("board");
    if let Some(first) = snaps.first() {
        let (s, v) = access(SNAPSHOT_ROUTE, false, true, &Caller::Owner, &[("file", "widgets/board"), ("name", &first.name)], json!({}));
        assert_eq!(s, "200 OK", "{v}");
        assert!(v["files"].as_array().is_some_and(|f| f.iter().any(|x| x["path"] == "manifest.json")), "{v}");
    } else {
        panic!("the put kept no widget snapshot");
    }
}

#[test]
fn garden_and_widget_rows_carry_editors_and_signers_with_the_switch_on() {
    let e = env();
    list(&e.g1, "w-ed");
    let (s, v) = zen("/cli/zen/gardens", true, &Caller::Owner, json!({}));
    assert_eq!(s, "200 OK", "{v}");
    let row = v["gardens"].as_array().and_then(|a| a.iter().find(|g| g["id"] == e.g1.as_str()).cloned()).expect("row");
    assert_eq!(row["editors"], 1, "{row}");
    assert!(row["madeBy"].is_null(), "{row}");
    zen("/cli/zen/widget/sign", false, &agent("w-ed"), json!({"widget": "board"}));
    let (s, v) = zen("/cli/zen/widgets", true, &Caller::Owner, json!({}));
    assert_eq!(s, "200 OK", "{v}");
    let w = v["widgets"].as_array().and_then(|a| a.iter().find(|w| w["name"] == "board").cloned()).expect("board");
    assert_eq!(w["signedBy"]["kind"], "agent", "{w}");
    assert_eq!(w["signedBy"]["handle"], "editor", "{w}");
    assert!(w["changedAt"].as_i64().is_some(), "{w}");
    assert_eq!(w["placements"][0]["signed"], true, "{w}");
    // Off: the rows are as at 0.46.3.
    ga::set_switch(false);
    let (_, v) = zen("/cli/zen/gardens", true, &Caller::Owner, json!({}));
    assert!(v["gardens"][0].get("editors").is_none(), "{v}");
    let (_, v) = zen("/cli/zen/widgets", true, &Caller::Owner, json!({}));
    assert!(v["widgets"][0].get("signedBy").is_none(), "{v}");
}
