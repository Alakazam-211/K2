//! prd-agent-app-permissions-v1 slices D (delete guard, hand over / park /
//! unpark / release, the orphan sweep), F (512 MiB default, switch-1
//! default limits, admission-lite) and G (the box view, clash refusals that
//! name the agent and never a path). In process, on a scoped DB.

use std::collections::HashMap;

use k2_core::app_grace::config::{Op, PortSpec};
use k2_core::connect_users::Role;
use k2_core::deploy::store;
use rusqlite::params;
use serde_json::{json, Value as J};

use super::routes::Who;

const M: i64 = 1 << 20;
const G: i64 = 1 << 30;

struct Fx {
    _db: k2_core::db::TestDbGuard,
    _home: crate::test_support::TempHome,
    a: String,
    b: String,
    /// Two Apps of `a`, one of `b`.
    a1: String,
    a2: String,
    b1: String,
}

fn ws_path(ws: &str) -> String {
    format!("/tmp/k2-own-test/{ws}")
}

fn add_ws(ws: &str, name: &str) {
    let h = k2_core::db::shared();
    let conn = h.lock();
    // The handle differs from the id, so a leaked id is caught.
    conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?4)", params![ws, name, ws_path(ws), format!("{ws}-h")])
        .unwrap();
}

/// An App row with its published service row (as `install` makes them).
fn add_app(app: &str, ws: &str, memory: Option<i64>, tunnel: bool) {
    let h = k2_core::db::shared();
    let conn = h.lock();
    let svc = k2_core::published_services::insert(
        &conn,
        ws,
        app,
        k2_core::published_services::CMD_APP_SENTINEL,
        "/tmp/k2-own-test/apps",
        0,
        if tunnel { k2_core::published_services::EXPOSE_TUNNEL } else { k2_core::published_services::EXPOSE_LOCAL },
        k2_core::published_services::DESIRED_STOPPED,
        k2_core::published_services::KIND_CMD,
        "",
    )
    .unwrap();
    store::insert_app(
        &conn,
        &store::NewApp {
            app_id: app,
            service_id: &svc.id,
            seed_id: None,
            owner_workspace: ws,
            isolation: "process",
            memory_limit_bytes: memory,
            front_port: 0,
            created_by: "owner",
        },
    )
    .unwrap();
    store::set_state(&conn, app, store::STATE_STOPPED, None).unwrap();
}

fn fx(tag: &str) -> Fx {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    k2_core::deploy::set_enabled(true);
    let (a, b) = (format!("own-a-{tag}"), format!("own-b-{tag}"));
    add_ws(&a, &format!("Sales {tag}"));
    add_ws(&b, &format!("Ops {tag}"));
    let (a1, a2, b1) = (format!("oa1-{tag}"), format!("oa2-{tag}"), format!("ob1-{tag}"));
    add_app(&a1, &a, Some(256 * M), false);
    add_app(&a2, &a, Some(256 * M), false);
    add_app(&b1, &b, Some(512 * M), false);
    super::alerts::set_sink(Some(Box::new(|_: &str, _: &str| {})));
    Fx { _db: db, _home: home, a, b, a1, a2, b1 }
}

fn agent(ws: &str) -> Who {
    Who::Agent { project_id: ws.into(), session_id: format!("sess-{ws}") }
}

fn admin() -> Who {
    Who::Login { username: "ada".into(), role: Role::Admin }
}

fn switch(ws: &str, on: bool) {
    k2_core::workspace::settings::set_agents_can_manage_skin(&ws_path(ws), on).unwrap();
}

fn call(path: &str, who: &Who, body: J) -> (u16, J) {
    let r = super::routes::handle(path, who.clone(), body.to_string().as_bytes(), &HashMap::new());
    let n: u16 = r.status.split_whitespace().next().unwrap().parse().unwrap();
    (n, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{path}: not JSON ({e}): {}", r.body)))
}

fn get(path: &str, who: &Who, q: &[(&str, &str)]) -> (u16, J) {
    let p: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let r = super::routes::handle(path, who.clone(), b"", &p);
    let n: u16 = r.status.split_whitespace().next().unwrap().parse().unwrap();
    (n, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{path}: not JSON ({e}): {}", r.body)))
}

fn code(v: &J) -> &str {
    v["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {v}"))
}

fn row(app: &str) -> store::AppRow {
    let h = k2_core::db::shared();
    let conn = h.lock();
    store::get_app(&conn, app).unwrap().unwrap_or_else(|| panic!("no App {app}"))
}

fn exists(app: &str) -> bool {
    let h = k2_core::db::shared();
    let conn = h.lock();
    store::get_app(&conn, app).unwrap().is_some()
}

fn service_ws(app: &str) -> String {
    let h = k2_core::db::shared();
    let conn = h.lock();
    conn.query_row("SELECT project_id FROM published_services WHERE name = ?1", params![app], |r| r.get(0)).unwrap()
}

fn notes_for(ws: &str) -> Vec<String> {
    super::ownership::NOTES.lock().unwrap().iter().filter(|(w, _)| w == ws).map(|(_, t)| t.clone()).collect()
}

fn audit_events(app: &str) -> Vec<String> {
    let h = k2_core::db::shared();
    let conn = h.lock();
    k2_core::deploy::audit::list_for(&conn, k2_core::deploy::audit::SUBJECT_APP, app, 50)
        .unwrap()
        .into_iter()
        .map(|r| r.event)
        .collect()
}

fn remove_ws(ws: &str) -> (String, J) {
    let mut p = HashMap::new();
    p.insert("path".to_string(), ws_path(ws));
    let r = crate::workspace_routes::dispatch("/cli/workspace/remove", &p).expect("the route answers");
    (r.status.to_string(), serde_json::from_str(&r.body).unwrap_or(J::Null))
}

fn ws_exists(ws: &str) -> bool {
    let h = k2_core::db::shared();
    let conn = h.lock();
    conn.query_row("SELECT 1 FROM projects WHERE id = ?1", params![ws], |_| Ok(())).is_ok()
}

/// No folder path, workspace id, log, env or guest ever reaches an agent.
fn assert_no_leak(v: &J, ws_ids: &[&str]) {
    let text = v.to_string();
    assert!(!text.contains("/tmp/k2-own-test"), "a folder path leaked: {text}");
    for id in ws_ids {
        assert!(!text.contains(&format!("\"{id}\"")), "a workspace id leaked ({id}): {text}");
    }
    for k in ["\"folder\"", "\"logs\"", "\"env\"", "\"guests\"", "\"framed\"", "\"caps\""] {
        assert!(!text.contains(k), "{k} leaked: {text}");
    }
}

// ── D: the delete guard and the ways out ─────────────────────────────

#[test]
fn removing_a_workspace_with_apps_is_refused_with_the_apps_and_three_ways_out() {
    let f = fx("guard");
    let (st, v) = remove_ws(&f.a);
    assert_eq!(st, "409 Conflict", "{v}");
    assert_eq!(v["error"]["code"], "workspace_has_apps", "{v}");
    let apps: Vec<&str> = v["error"]["apps"].as_array().unwrap().iter().map(|a| a["app"].as_str().unwrap()).collect();
    assert_eq!(apps, vec![f.a1.as_str(), f.a2.as_str()], "{v}");
    let ways: Vec<&str> = v["error"]["ways"].as_array().unwrap().iter().map(|w| w["way"].as_str().unwrap()).collect();
    assert_eq!(ways, vec!["handover", "uninstall", "park"]);
    assert!(ws_exists(&f.a), "the workspace stays");
    assert_eq!(row(&f.a1).owner_workspace, f.a, "its Apps stay its");

    // The UI path refuses the same way, and k2_core is the floor under both.
    let r = crate::db_routes::handle_projects_delete(json!({ "id": f.a }).to_string().as_bytes());
    assert!(r.status.starts_with("409"), "{}", r.body);
    assert!(r.body.contains("workspace_has_apps"), "{}", r.body);
    let e = k2_core::workspace::lifecycle::remove_workspace_db_only(&ws_path(&f.a)).expect_err("the core floor refuses");
    assert!(e.starts_with("workspace_has_apps"), "{e}");
    let e = k2_core::projects_ops::projects_delete(&f.a).expect_err("the core floor refuses");
    assert!(e.starts_with("workspace_has_apps"), "{e}");
    assert!(ws_exists(&f.a));

    // A workspace without Apps goes as before.
    let lone = format!("own-lone-{}", "guard");
    add_ws(&lone, "Lone");
    let (st, v) = remove_ws(&lone);
    assert!(st.starts_with("200"), "{st} {v}");
    assert!(!ws_exists(&lone));
}

#[test]
fn park_stops_and_holds_the_app_for_the_owner_then_the_workspace_can_go() {
    let f = fx("park");
    // Agents never park (a person's way out); an admin may (it lowers).
    let (st, v) = call("/cli/app/park", &agent(&f.a), json!({ "app": f.a1 }));
    assert_eq!((st, code(&v)), (403, "owner_only"), "{v}");
    let (st, v) = call("/cli/app/park", &admin(), json!({ "app": f.a1 }));
    assert_eq!(st, 200, "{v}");
    let r = row(&f.a1);
    assert_eq!((r.state.as_str(), r.owner_workspace.as_str()), (store::STATE_PARKED, ""));
    assert_eq!(service_ws(&f.a1), "", "its service row is no workspace's");
    assert!(audit_events(&f.a1).contains(&"park".to_string()));
    assert_eq!(notes_for(&f.a).len(), 1, "the agent hears once (an inbox note, not a wake)");
    // The agent no longer sees it as its own; it can't start it.
    let (st, v) = get("/cli/app/inspect", &agent(&f.a), &[("app", &f.a1)]);
    assert_eq!((st, code(&v)), (404, "app_not_found"), "{v}");
    // Even the owner can't start a parked App: unpark it first.
    let (st, v) = call("/cli/app/start", &Who::Owner, json!({ "app": f.a1 }));
    assert_eq!((st, code(&v)), (409, "app_parked"), "{v}");
    let (st, v) = call("/cli/app/stop", &Who::Owner, json!({ "app": f.a1 }));
    assert_eq!(st, 200, "{v}");
    assert_eq!(row(&f.a1).state, store::STATE_PARKED, "stop leaves it parked");

    // Release parks the rest; then the removal goes through.
    let (st, v) = call("/cli/app/release", &admin(), json!({ "workspace": f.a, "apps": "park" }));
    assert_eq!(st, 200, "{v}");
    assert_eq!(row(&f.a2).state, store::STATE_PARKED);
    let (st, v) = remove_ws(&f.a);
    assert!(st.starts_with("200"), "{st} {v}");
    assert!(!ws_exists(&f.a));
    assert!(exists(&f.a1) && exists(&f.a2), "parking never deletes");
}

#[test]
fn the_owner_hands_an_app_over_and_unparks_a_parked_one() {
    let f = fx("hand");
    // Owner only: an admin, an agent and a member are refused.
    for who in [admin(), agent(&f.a), Who::Login { username: "mo".into(), role: Role::Member }] {
        let (st, v) = call("/cli/app/handover", &who, json!({ "app": f.a1, "to": f.b }));
        assert_eq!((st, code(&v)), (403, "owner_only"), "{who:?}: {v}");
    }
    // The label it holds moves with it.
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        k2_core::db::schema::SubdomainWorkspace::self_stamp(&conn, &f.a1, &f.a, None).unwrap();
    }
    let (st, v) = call("/cli/app/handover", &Who::Owner, json!({ "app": f.a1, "to": f.b }));
    assert_eq!(st, 200, "{v}");
    assert_eq!(row(&f.a1).owner_workspace, f.b);
    assert_eq!(service_ws(&f.a1), f.b);
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        assert_eq!(k2_core::db::schema::SubdomainWorkspace::owner_of(&conn, &f.a1).unwrap().as_deref(), Some(f.b.as_str()));
    }
    assert!(audit_events(&f.a1).contains(&"handover".to_string()));
    assert_eq!(notes_for(&f.b).len(), 1, "the new agent hears");
    assert_eq!(notes_for(&f.a).len(), 1, "the old agent hears");
    let (st, _) = get("/cli/app/inspect", &agent(&f.b), &[("app", &f.a1)]);
    assert_eq!(st, 200, "the new agent maintains it");
    let (st, _) = get("/cli/app/inspect", &agent(&f.a), &[("app", &f.a1)]);
    assert_eq!(st, 404, "the old one doesn't");

    // Unpark is for parked Apps; a parked one comes back stopped.
    let (st, v) = call("/cli/app/unpark", &Who::Owner, json!({ "app": f.a2, "to": f.b }));
    assert_eq!((st, code(&v)), (409, "not_parked"), "{v}");
    let (st, v) = call("/cli/app/park", &Who::Owner, json!({ "app": f.a2 }));
    assert_eq!(st, 200, "{v}");
    let (st, v) = call("/cli/app/unpark", &admin(), json!({ "app": f.a2, "to": f.b }));
    assert_eq!((st, code(&v)), (403, "owner_only"), "{v}");
    let (st, v) = call("/cli/app/unpark", &Who::Owner, json!({ "app": f.a2, "to": f.b }));
    assert_eq!(st, 200, "{v}");
    let r = row(&f.a2);
    assert_eq!((r.state.as_str(), r.owner_workspace.as_str()), (store::STATE_STOPPED, f.b.as_str()));
    assert!(audit_events(&f.a2).contains(&"unpark".to_string()));

    // `release --apps handover:` is the owner's too.
    add_app(&format!("oa3-{}", "hand"), &f.a, Some(64 * M), false);
    let (st, v) = call("/cli/app/release", &admin(), json!({ "workspace": f.a, "apps": format!("handover:{}", f.b) }));
    assert_eq!((st, code(&v)), (403, "owner_only"), "{v}");
    let (st, v) = call("/cli/app/release", &Who::Owner, json!({ "workspace": f.a, "apps": format!("handover:{}", f.b) }));
    assert_eq!(st, 200, "{v}");
    let (st, v) = remove_ws(&f.a);
    assert!(st.starts_with("200"), "{st} {v}");
}

#[test]
fn release_uninstall_removes_the_apps_then_the_workspace() {
    let f = fx("unin");
    let (st, v) = call("/cli/app/release", &agent(&f.a), json!({ "workspace": f.a, "apps": "uninstall" }));
    assert_eq!((st, code(&v)), (403, "owner_only"), "an agent never releases: {v}");
    let (st, v) = call("/cli/app/release", &Who::Owner, json!({ "workspace": f.a, "apps": "bogus" }));
    assert_eq!((st, code(&v)), (400, "bad_request"), "{v}");
    let (st, v) = call("/cli/app/release", &Who::Owner, json!({ "workspace": f.a, "apps": "uninstall" }));
    assert_eq!(st, 200, "{v}");
    assert!(!exists(&f.a1) && !exists(&f.a2), "uninstalled");
    assert!(exists(&f.b1), "another workspace's App is untouched");
    let (st, v) = remove_ws(&f.a);
    assert!(st.starts_with("200"), "{st} {v}");
}

#[test]
fn the_boot_sweep_parks_orphans_and_tells_the_owner_once() {
    let f = fx("sweep");
    let gone = "own-gone-sweep";
    let (o1, o2) = ("orph1-sweep", "orph2-sweep");
    add_app(o1, gone, Some(128 * M), false);
    add_app(o2, gone, None, false);
    let parked = super::ownership::sweep_orphans();
    assert_eq!(parked, vec![o1.to_string(), o2.to_string()]);
    for o in [o1, o2] {
        let r = row(o);
        assert_eq!((r.state.as_str(), r.owner_workspace.as_str()), (store::STATE_PARKED, ""), "{o}");
        assert!(r.state_reason.as_deref().unwrap_or("").contains("workspace was removed"), "{r:?}");
    }
    assert!(exists(&f.a1) && row(&f.a1).owner_workspace == f.a, "a live workspace's App is untouched");
    let notes = || -> Vec<(String, String)> {
        let h = k2_core::db::shared();
        let conn = h.lock();
        let mut st = conn.prepare("SELECT title, COALESCE(body, '') FROM feedback WHERE kind = 'fyi' AND agent_name = 'k2-apps'").unwrap();
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap()
    };
    let n = notes();
    assert_eq!(n.len(), 1, "one note: {n:?}");
    assert!(n[0].0.contains("2 Apps parked"), "{n:?}");
    assert!(n[0].1.contains(o1) && n[0].1.contains(o2), "it lists them: {n:?}");
    // A second boot finds nothing new and says nothing.
    assert!(super::ownership::sweep_orphans().is_empty());
    assert_eq!(notes().len(), 1);
}

// ── F: the default, the envelope, admission ──────────────────────────

#[test]
fn an_install_with_no_memory_gets_512_mib() {
    let f = fx("dflt");
    let _pool = super::admission::pool_for_test(64 * G);
    let app = "odflt-app";
    let (st, v) = call("/cli/app/install", &Who::Owner, json!({ "app": app, "workspace": f.a }));
    assert_eq!(st, 200, "{v}");
    assert_eq!(row(app).memory_limit_bytes, Some(512 * M));
    // An owner who says `none` means it (L1); an agent can't (below).
    let (st, v) = call("/cli/app/install", &Who::Owner, json!({ "app": "odflt-none", "workspace": f.a, "memory": "none" }));
    assert_eq!(st, 200, "{v}");
    assert_eq!(row("odflt-none").memory_limit_bytes, None);
}

#[test]
fn agents_past_the_default_limits_are_held_for_the_owner() {
    let f = fx("lim");
    let _pool = super::admission::pool_for_test(64 * G);
    switch(&f.a, true);
    let a = agent(&f.a);
    // App 3 of 3 fits; memory up to 2 GiB applies.
    let (st, v) = call("/cli/app/install", &a, json!({ "app": "olim-three", "memory": "2G" }));
    assert_eq!(st, 200, "{v}");
    // App 4 is held, not refused, and nothing is created.
    let (st, v) = call("/cli/app/install", &a, json!({ "app": "olim-four" }));
    assert_eq!((st, v["status"].as_str()), (202, Some("held")), "{v}");
    assert!(v["limits"][0].as_str().unwrap().contains("default limit is 3 Apps"), "{v}");
    assert!(!exists("olim-four"));

    // Config: a raise inside the envelope applies; past it, held.
    let (st, v) = call("/cli/app/config", &a, json!({ "app": f.a1, "memory": "1G" }));
    assert_eq!((st, v["status"].as_str()), (200, None), "{v}");
    assert_eq!(row(&f.a1).memory_limit_bytes, Some(G));
    let (st, v) = call("/cli/app/config", &a, json!({ "app": f.a1, "memory": "3G" }));
    assert_eq!((st, v["status"].as_str()), (202, Some("held")), "{v}");
    assert!(v["limits"][0].as_str().unwrap().contains("up to 2 GiB"), "{v}");
    assert_eq!(row(&f.a1).memory_limit_bytes, Some(G), "unchanged until the owner approves");
    let (st, v) = call("/cli/app/config", &a, json!({ "app": f.a1, "memory": "none" }));
    assert_eq!((st, v["status"].as_str()), (202, Some("held")), "every agent App keeps a limit: {v}");
    let (st, v) = call("/cli/app/config", &a, json!({ "app": f.a1, "cpuWeight": 400 }));
    assert_eq!((st, v["status"].as_str()), (202, Some("held")), "{v}");
    // Disk: setting a budget where there was none narrows (applies); a
    // raise past 10 GiB across the agent's Apps is held.
    let (st, v) = call("/cli/app/config", &a, json!({ "app": f.a1, "disk": "4G" }));
    assert_eq!((st, v["status"].as_str()), (200, None), "{v}");
    let (st, v) = call("/cli/app/config", &a, json!({ "app": f.a2, "disk": "4G" }));
    assert_eq!((st, v["status"].as_str()), (200, None), "{v}");
    let (st, v) = call("/cli/app/config", &a, json!({ "app": f.a1, "disk": "5G" }));
    assert_eq!((st, v["status"].as_str()), (200, None), "9 GiB in all fits: {v}");
    let (st, v) = call("/cli/app/config", &a, json!({ "app": f.a1, "disk": "7G" }));
    assert_eq!((st, v["status"].as_str()), (202, Some("held")), "11 GiB across the agent's Apps: {v}");
    assert!(v["limits"][0].as_str().unwrap().contains("10 GiB"), "{v}");

    // The owner isn't bound by an agent's envelope.
    let (st, v) = call("/cli/app/config", &Who::Owner, json!({ "app": f.a1, "memory": "3G" }));
    assert_eq!((st, v["status"].as_str()), (200, None), "{v}");

    // One public hostname per agent: a second tunnel install is held.
    let b = agent(&f.b);
    switch(&f.b, true);
    add_app("olim-pub", &f.b, Some(64 * M), true);
    let (st, v) = call("/cli/app/install", &b, json!({ "app": "olim-pub2", "tunnel": true }));
    assert_eq!((st, v["status"].as_str()), (202, Some("held")), "{v}");
    assert!(v["limits"].to_string().contains("public hostname"), "{v}");
}

#[test]
fn admission_refuses_what_the_box_cant_hold_and_names_the_agents() {
    let f = fx("adm");
    switch(&f.a, true);
    // Pool 1 GiB; Apps hold 256 + 256 + 512 = 1 GiB already.
    super::admission::with_pool(G, || {
        let (st, v) = call("/cli/app/install", &Who::Owner, json!({ "app": "oadm-new", "workspace": f.b, "memory": "128M" }));
        assert_eq!((st, code(&v)), (409, "box_budget_full"), "even the owner: {v}");
        assert!(!exists("oadm-new"));
        let holders = v["error"]["holders"].as_array().unwrap();
        assert_eq!(holders[0]["app"], f.b1, "biggest first: {v}");
        assert_eq!(holders[0]["agent"], "Ops adm", "{v}");
        assert!(holders[0]["address"].as_str().unwrap().starts_with(&f.b), "{v}");
        assert!(v["error"]["hint"].as_str().unwrap().contains("Ops adm"), "{v}");
        assert_no_leak(&v, &[]);

        // An agent's raise that fits its envelope still meets the box.
        let (st, v) = call("/cli/app/config", &agent(&f.a), json!({ "app": f.a1, "memory": "512M" }));
        assert_eq!((st, code(&v)), (409, "box_budget_full"), "{v}");
        assert_eq!(row(&f.a1).memory_limit_bytes, Some(256 * M));
        assert_no_leak(&v, &[&f.a, &f.b]);
        // Lowering always applies.
        let (st, v) = call("/cli/app/config", &agent(&f.a), json!({ "app": f.a1, "memory": "128M" }));
        assert_eq!(st, 200, "{v}");
        // Parked Apps don't count; unparking re-checks.
        let (st, v) = call("/cli/app/park", &Who::Owner, json!({ "app": f.b1 }));
        assert_eq!(st, 200, "{v}");
        let (st, v) = call("/cli/app/install", &Who::Owner, json!({ "app": "oadm-new", "workspace": f.b, "memory": "512M" }));
        assert_eq!(st, 200, "{v}");
        let (st, v) = call("/cli/app/unpark", &Who::Owner, json!({ "app": f.b1, "to": f.b }));
        assert_eq!((st, code(&v)), (409, "box_budget_full"), "{v}");
        assert_eq!(row(&f.b1).state, store::STATE_PARKED);
    });
}

// ── G: the box view, clashes ─────────────────────────────────────────

#[test]
fn every_agent_sees_the_box_with_maintainers_and_never_the_data() {
    let f = fx("box");
    super::routes::apply_config_ops(&f.b1, &[Op::PortAdd(PortSpec { port: 9443, proto: "tcp".into() })], "owner").unwrap();
    let (st, v) = get("/cli/app/list", &agent(&f.a), &[("box", "1")]);
    assert_eq!(st, 200, "{v}");
    let apps = v["apps"].as_array().unwrap();
    assert_eq!(apps.len(), 3, "all three, not just its own: {v}");
    let other = apps.iter().find(|x| x["appId"] == f.b1).expect("b's App is listed");
    assert_eq!(other["maintainer"]["agent"], "Ops box");
    assert!(other["maintainer"]["address"].as_str().unwrap().starts_with(&f.b));
    assert_eq!(other["memory"]["limitBytes"], 512 * M);
    assert_eq!(other["ports"][0]["port"], 9443);
    assert_eq!(other["switches"]["buildAndRun"], false);
    assert!(v["memory"]["pool"]["poolBytes"].is_number(), "{v}");
    assert_no_leak(&v, &[&f.a, &f.b]);
    // The plain list stays the agent's own.
    let (_, v) = get("/cli/app/list", &agent(&f.a), &[]);
    assert_eq!(v["apps"].as_array().unwrap().len(), 2, "{v}");
}

#[test]
fn clashes_name_the_owning_agent_and_address_never_the_path() {
    let f = fx("clash");
    switch(&f.a, true);
    let a = agent(&f.a);
    // A port another App opens.
    super::routes::apply_config_ops(&f.b1, &[Op::PortAdd(PortSpec { port: 9555, proto: "tcp".into() })], "owner").unwrap();
    let (st, v) = call("/cli/app/config", &a, json!({ "app": f.a1, "portAdd": "9555" }));
    assert_eq!((st, code(&v)), (409, "port_owned"), "{v}");
    assert_eq!(v["error"]["app"], f.b1);
    assert_eq!(v["error"]["owner"]["agent"], "Ops clash");
    let addr = v["error"]["owner"]["address"].as_str().unwrap();
    assert!(v["error"]["hint"].as_str().unwrap().contains(&format!("k2 msg {addr}")), "{v}");
    assert_no_leak(&v, &[&f.a, &f.b]);

    // A published label another workspace holds.
    let (st, v) = call("/cli/app/install", &a, json!({ "app": f.b1 }));
    assert_eq!((st, code(&v)), (409, "app_exists"), "the App id itself: {v}");
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        k2_core::published_services::insert(&conn, &f.b, "oclash-site", "python3 -m http.server", "/tmp", 8080, "local", "stopped", "cmd", "").unwrap();
        k2_core::db::schema::SubdomainWorkspace::self_stamp(&conn, "oclash-lbl", &f.b, None).unwrap();
    }
    let (st, v) = call("/cli/app/install", &a, json!({ "app": "oclash-site" }));
    assert_eq!((st, code(&v)), (409, "label_taken"), "{v}");
    assert_eq!(v["error"]["owner"]["agent"], "Ops clash", "{v}");
    assert_no_leak(&v, &[&f.a, &f.b]);
    let (st, v) = crate::label_owner::with_caller(crate::label_owner::Caller::Agent, || {
        call("/cli/app/install", &a, json!({ "app": "oclash-lbl" }))
    });
    assert_eq!((st, code(&v)), (409, "label_taken"), "{v}");
    assert_eq!(v["error"]["owner"]["agent"], "Ops clash", "{v}");
    assert!(v["error"]["owner"]["address"].is_string(), "{v}");
    assert_no_leak(&v, &[&f.a, &f.b]);
}
