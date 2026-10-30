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

use super::handover_guard as guard;
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
    // 0.46.5: a handover's plan reads the App's secrets (a closed vault
    // refuses it: handover_vault_closed), whatever test ran first.
    let _ = crate::vault::ensure_test_vault();
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

/// A fresh hand-over confirm bound to `bound` (an App id or a release's
/// `release:<ws>`), stamped like the vault stamps `stamp`.
fn confirm(bound: &str, stamp: &str) -> String {
    super::egress_routes::test_seams::vault_handle().confirm.mint(guard::CONFIRM_ACTION, Some(bound), stamp, "local-auth")
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

/// 0.46.5: a handover is a card the owner approves in person (a fresh
/// `app-approve` confirm bound to the request).
fn approve_card(v: &J) -> (u16, J) {
    let req = v["request"]["id"].as_str().unwrap_or_else(|| panic!("no held request: {v}")).to_string();
    let tok = super::egress_routes::test_seams::vault_handle().confirm.mint(super::approvals::CONFIRM_ACTION, Some(&req), "owner", "local-auth");
    let body = json!({ "request": req, "confirmToken": tok }).to_string();
    let r = super::routes::respond(super::approvals::handle("/cli/app/approve", &Who::Owner, body.as_bytes(), &HashMap::new()));
    let n: u16 = r.status.split_whitespace().next().unwrap().parse().unwrap();
    (n, serde_json::from_str(&r.body).unwrap())
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
    // Owner only: an admin and an agent are refused; a Member is
    // read-only on every App route first (Q7, slice C).
    // (0.46.5: the App's own agent offers it instead; handover_tests.)
    let (st, v) = call("/cli/app/handover", &admin(), json!({ "app": f.a1, "to": f.b }));
    assert_eq!((st, code(&v)), (403, "owner_only"), "{v}");
    let (st, v) = call("/cli/app/handover", &Who::Login { username: "mo".into(), role: Role::Member }, json!({ "app": f.a1, "to": f.b }));
    assert_eq!((st, code(&v)), (403, "member_read_only"), "{v}");
    // The label it holds moves with it.
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        k2_core::db::schema::SubdomainWorkspace::self_stamp(&conn, &f.a1, &f.a, None).unwrap();
    }
    // The owner in person: the owner token alone is refused, naming the
    // confirm the CLI and Settings mint.
    let (st, v) = call("/cli/app/handover", &Who::Owner, json!({ "app": f.a1, "to": f.b }));
    assert_eq!((st, v["status"].as_str()), (202, Some("held")), "a card for the owner, nothing moved yet: {v}");
    assert_eq!(row(&f.a1).owner_workspace, f.a, "nothing moves before the approval");
    let (st, v) = approve_card(&v);
    assert_eq!((st, v["status"].as_str()), (200, Some("approved")), "{v}");
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
    assert_eq!(st, 202, "{v}");
    let (st, v) = approve_card(&v);
    assert_eq!((st, v["status"].as_str()), (200, Some("approved")), "{v}");
    let r = row(&f.a2);
    assert_eq!((r.state.as_str(), r.owner_workspace.as_str()), (store::STATE_STOPPED, f.b.as_str()));
    assert!(audit_events(&f.a2).contains(&"unpark".to_string()));

    // `release --apps handover:` is the owner's too, with one confirm
    // bound to the workspace.
    add_app(&format!("oa3-{}", "hand"), &f.a, Some(64 * M), false);
    let (st, v) = call("/cli/app/release", &admin(), json!({ "workspace": f.a, "apps": format!("handover:{}", f.b) }));
    assert_eq!((st, code(&v)), (403, "owner_only"), "{v}");
    let (st, v) = call("/cli/app/release", &Who::Owner, json!({ "workspace": f.a, "apps": format!("handover:{}", f.b) }));
    assert_eq!((st, v["status"].as_str()), (202, Some("held")), "one card per App: {v}");
    let (st, _) = remove_ws(&f.a);
    assert!(st.starts_with("409"), "the removal waits for the cards: {st}");
    for r in v["results"].as_array().unwrap() {
        let (st, a) = approve_card(&json!({ "request": { "id": r["request"] } }));
        assert_eq!(st, 200, "{a}");
    }
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

fn svc_of(app: &str) -> String {
    row(app).service_id
}

/// App-governed Spaces S7 follow-up: `release --apps uninstall` is all or
/// nothing. Apps that would drop people's access are listed in ONE
/// refusal (each with its counts) before any App is uninstalled, even
/// the ones that would drop nothing; `dropAccess` goes through for all.
#[test]
fn release_uninstall_refuses_and_lists_every_app_that_would_drop_access() {
    let f = fx("rdrop");
    k2_core::skin::add_principal_for_app("rdrop-u1", None, &svc_of(&f.a1), None, "owner").unwrap();
    k2_core::skin::add_principal_for_app("rdrop-u2", None, &svc_of(&f.a1), None, "owner").unwrap();
    let (st, v) = call("/cli/app/release", &Who::Owner, json!({ "workspace": f.a, "apps": "uninstall" }));
    assert_eq!((st, code(&v)), (409, super::routes::UNINSTALL_DROPS_ACCESS), "{v}");
    let hint = v["error"]["hint"].as_str().unwrap();
    assert!(hint.starts_with("nothing was released"), "{hint}");
    assert!(hint.contains(&format!("{}: 2 App logins", f.a1)), "{hint}");
    assert!(hint.contains("--drop-access"), "{hint}");
    let listed = v["error"]["apps"].as_array().expect("apps");
    assert_eq!(listed.len(), 1, "only the App that would lose something: {v}");
    assert_eq!((listed[0]["app"].as_str(), listed[0]["wouldDrop"]["logins"].as_u64()), (Some(f.a1.as_str()), Some(2)));
    assert_eq!(v["error"]["released"], json!([]));
    assert!(exists(&f.a1) && exists(&f.a2), "nothing was uninstalled, not even the App with nothing to lose");
    // The workspace can't go while its Apps are there.
    let (st, _) = remove_ws(&f.a);
    assert!(st.starts_with("409"), "{st}");

    let (st, v) = call("/cli/app/release", &Who::Owner, json!({ "workspace": f.a, "apps": "uninstall", "dropAccess": true }));
    assert_eq!(st, 200, "{v}");
    assert!(!exists(&f.a1) && !exists(&f.a2), "uninstalled, both");
    assert!(exists(&f.b1));
    let (st, v) = remove_ws(&f.a);
    assert!(st.starts_with("200"), "{st} {v}");
}

/// `forRemoval` (k2 workspace remove --apps, Settings): a workspace that
/// is a project group's Point of Contact can't be removed, so none of its
/// Apps is released either (parked or uninstalled), whatever dropAccess.
#[test]
fn release_for_removal_releases_nothing_when_the_workspace_cannot_go() {
    let f = fx("rpoc");
    let g = k2_core::project_groups::create_group("rpoc-group").unwrap();
    k2_core::project_groups::add_member(&g.id, &f.a).unwrap();
    k2_core::project_groups::set_poc(&g.id, &f.a).unwrap();
    for way in ["uninstall", "park"] {
        let (st, v) = call("/cli/app/release", &Who::Owner, json!({ "workspace": f.a, "apps": way, "forRemoval": true, "dropAccess": true }));
        assert_eq!((st, code(&v)), (409, "poc_of_projects"), "{way}: {v}");
        assert!(v["error"]["hint"].as_str().unwrap().contains("Point of Contact for: rpoc-group"), "{v}");
        assert!(exists(&f.a1) && exists(&f.a2), "{way}: nothing released");
        assert_ne!(row(&f.a1).state, store::STATE_PARKED, "{way}: nothing parked");
    }
    // Without forRemoval, release alone still works (the CLI's k2 app release).
    let (st, v) = call("/cli/app/release", &Who::Owner, json!({ "workspace": f.a, "apps": "park" }));
    assert_eq!(st, 200, "{v}");
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

/// 0.46.3 interplay (selfmove × DFG N6): `k2 app label take` of a label
/// another workspace holds is refused naming the agent that holds it and
/// its `k2 msg` address, never its folder or id: a published service row
/// of theirs and a bare subdomain stamp alike.
#[test]
fn label_take_of_another_workspaces_label_names_the_holding_agent() {
    let f = fx("ltake");
    switch(&f.a, true);
    let a = agent(&f.a);
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        k2_core::published_services::insert(&conn, &f.b, "oltake-svc", "python3 -m http.server", "/tmp", 8081, "tunnel", "stopped", "cmd", "")
            .unwrap();
        k2_core::db::schema::SubdomainWorkspace::self_stamp(&conn, "oltake-lbl", &f.b, None).unwrap();
    }
    for label in ["oltake-svc", "oltake-lbl"] {
        let (st, v) = crate::label_owner::with_caller(crate::label_owner::Caller::Agent, || {
            call("/cli/app/label/take", &a, json!({ "app": f.a1, "label": label }))
        });
        assert_eq!((st, code(&v)), (403, "label_owned_by_other_workspace"), "{label}: {v}");
        assert_eq!(v["error"]["owner"]["agent"], "Ops ltake", "{label}: {v}");
        assert_eq!(v["error"]["owner"]["kind"], "agent", "{label}: {v}");
        let addr = v["error"]["owner"]["address"].as_str().unwrap_or_else(|| panic!("{label}: no address: {v}"));
        let hint = v["error"]["hint"].as_str().unwrap();
        assert!(hint.contains(&format!("k2 msg {addr}")), "{label}: {v}");
        assert!(hint.contains("maintained by Ops ltake"), "{label}: {v}");
        assert_no_leak(&v, &[&f.a, &f.b]);
    }
}

/// 0.46.3 interplay (selfmove × sew-template × apps-on): an App made by
/// the default install path lands in a container (walled), and its seal
/// key reaches the guarded engine as the file `K2_APP_SEAL_KEY` (a secret
/// the wall carries, never dropped, never an env variable).
#[test]
fn a_default_install_is_walled_and_its_seal_key_reaches_the_wall_as_a_file() {
    let f = fx("sealw");
    crate::vault::ensure_test_vault();
    let _pool = super::admission::pool_for_test(64 * G);
    // Unique App id: the test vault is shared by the test binary.
    let app = format!("osealw-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let (st, v) = call("/cli/app/install", &Who::Owner, json!({ "app": app, "workspace": f.a }));
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["app"]["isolation"], "container", "the default path is walled: {v}");
    let r = row(&app);
    assert_eq!(r.isolation, "container");
    assert!(!super::supervisor::runs_unwalled(&r), "a default App never runs unwalled");
    // What every start does before the engine reads the grants.
    super::seal::ensure_for_start(&r.app_id, &r.service_id);
    let g = crate::vault::publish_env::granted(&r.service_id, &f.a, std::path::Path::new("/nonexistent-sealw-cwd"));
    assert!(g.env.iter().all(|(k, _)| k != super::seal::SEAL_FILE), "never an env variable");
    let (items, dropped) = super::guarded::secret_items(g.env, g.files);
    assert!(dropped.is_empty(), "the wall drops nothing: {dropped:?}");
    let seal: Vec<_> = items.iter().filter(|i| i.file.as_deref() == Some(super::seal::SEAL_FILE)).collect();
    assert_eq!(seal.len(), 1, "exactly one seal key file for the walled App");
    assert!(seal[0].env.is_none(), "a file, not an env variable");
    assert_eq!(seal[0].value_hex.len(), 86, "43 base64url characters, hex-encoded on the wire");
}

// ── the hand-over guard (prd-app-handoff-v1 S1, H19) ─────────────────

/// What a hand-over could change, to prove a refusal changed nothing:
/// (owner workspace, state, service row's workspace, own label's holder).
fn snapshot(app: &str) -> (String, String, String, Option<String>) {
    let r = row(app);
    let svc_ws = service_ws(app);
    let h = k2_core::db::shared();
    let conn = h.lock();
    let label = k2_core::db::schema::SubdomainWorkspace::owner_of(&conn, app).unwrap();
    (r.owner_workspace, r.state, svc_ws, label)
}

fn svc(app: &str) -> String {
    row(app).service_id
}

fn stamp_label(app: &str, ws: &str) {
    let h = k2_core::db::shared();
    let conn = h.lock();
    k2_core::db::schema::SubdomainWorkspace::self_stamp(&conn, app, ws, None).unwrap();
}

/// A fresh App of `ws` (its own label stamped), for one blocker.
fn fresh(tag: &str, ws: &str) -> String {
    let app = format!("og-{tag}");
    add_app(&app, ws, Some(64 * M), false);
    stamp_label(&app, ws);
    app
}

/// Mark `app` running the way the supervisor keeps it: a walled App (the
/// default isolation since 0.46.3). `add_app` makes `process` Apps, and the
/// supervisor's boot (the first App route of a test process starts it, as
/// the daemon's start does) stops an unwalled "running" App while the
/// process engine is off, the default: `UNWALLED_STOPPED`. Its state check
/// then reads `stopped` and the handover goes through.
fn mark_running(app: &str) {
    let h = k2_core::db::shared();
    let conn = h.lock();
    assert_eq!(conn.execute("UPDATE apps SET isolation = 'container' WHERE app_id = ?1", params![app]).unwrap(), 1, "{app}");
    store::set_state(&conn, app, store::STATE_RUNNING, None).unwrap();
    let r = store::get_app(&conn, app).unwrap().unwrap_or_else(|| panic!("no App {app}"));
    assert!(!super::supervisor::runs_unwalled(&r), "{app}: walled");
}

/// The hand-over with a valid confirm is refused `409 handoff_needs_v1`
/// naming `want`, the hint gives the 0.46.5 line and the manual path, and
/// nothing changed (the confirm wasn't spent: it still works for this App).
fn assert_refused(app: &str, to: &str, want: &str) -> J {
    let before = snapshot(app);
    let tok = confirm(app, "owner");
    let (st, v) = call("/cli/app/handover", &Who::Owner, json!({ "app": app, "to": to, "confirmToken": tok }));
    assert_eq!((st, code(&v)), (409, guard::NEEDS_V1), "{app}: {v}");
    let codes: Vec<&str> = v["error"]["blockers"].as_array().unwrap_or_else(|| panic!("no blockers: {v}")).iter().map(|b| b["code"].as_str().unwrap()).collect();
    assert!(codes.contains(&want), "{app}: want {want} in {codes:?}: {v}");
    let hint = v["error"]["hint"].as_str().unwrap();
    for needle in ["The full App hand-off arrives in 0.46.5; until then", "new id", "k2 app data put", "label return", "label take", "domain"] {
        assert!(hint.contains(needle), "{app}: the hint lacks {needle:?}: {hint}");
    }
    assert_eq!(v["error"]["manual"].as_array().map(Vec::len), Some(4), "{v}");
    assert_eq!(snapshot(app), before, "{app}: a refusal changes nothing");
    // The blockers come before the confirm: the token is still good.
    let vault = super::egress_routes::test_seams::vault_handle();
    let stamp = "owner".to_string();
    vault.confirm.take(&tok, guard::CONFIRM_ACTION, Some(app), &stamp).expect("a refused move never spends the confirm");
    v
}

/// The trivial case still moves (owner + confirm), and the Owner login
/// passes its own confirm; the owner token alone, a confirm bound to
/// another App, a spent confirm and anyone but the owner are refused.
#[test]
fn handover_needs_the_owner_in_person() {
    // 0.46.4's guard is the K2_APP_HANDOVER=legacy path from 0.46.5 on
    // (the full handover: handover_tests).
    super::handover::set_test_mode(Some(super::handover::Mode::Legacy));
    let f = fx("person");
    stamp_label(&f.a1, &f.a);
    // An owner-added guest with no tie doesn't block.
    k2_core::skin::add_principal_for_app("person-guest", None, &svc(&f.a1), None, "owner").unwrap();
    for who in [admin(), agent(&f.a), Who::Shell { project_id: f.a.clone(), session_id: "sh".into() }] {
        let (st, v) = call("/cli/app/handover", &who, json!({ "app": f.a1, "to": f.b, "confirmToken": confirm(&f.a1, "owner") }));
        assert_eq!((st, code(&v)), (403, "owner_only"), "{who:?}: {v}");
    }
    let (st, v) = call("/cli/app/handover", &Who::Owner, json!({ "app": f.a1, "to": f.b }));
    assert_eq!((st, code(&v)), (403, "vault_confirm_required"), "the owner token alone: {v}");
    let (st, v) = call("/cli/app/handover", &Who::Owner, json!({ "app": f.a1, "to": f.b, "confirmToken": confirm(&f.a2, "owner") }));
    assert_eq!((st, code(&v)), (403, "vault_confirm_required"), "bound to another App: {v}");
    let (st, v) = call("/cli/app/handover", &Who::Owner, json!({ "app": f.a1, "to": f.b, "confirmToken": confirm(&f.a1, "connect:rosson") }));
    assert_eq!((st, code(&v)), (403, "vault_confirm_required"), "another person's confirm: {v}");
    let (st, v) = call("/cli/app/handover", &Who::Owner, json!({ "app": f.a1, "to": f.b, "confirmToken": "k2vc_made_up" }));
    assert_eq!((st, code(&v)), (403, "vault_confirm_required"), "{v}");
    assert_eq!(snapshot(&f.a1).0, f.a, "nothing moved yet");

    let tok = confirm(&f.a1, "owner");
    let (st, v) = call("/cli/app/handover", &Who::Owner, json!({ "app": f.a1, "to": f.b, "confirmToken": tok }));
    assert_eq!(st, 200, "{v}");
    assert_eq!(snapshot(&f.a1), (f.b.clone(), store::STATE_STOPPED.to_string(), f.b.clone(), Some(f.b.clone())));
    // Back again with the same token: a confirm works once.
    let (st, v) = call("/cli/app/handover", &Who::Owner, json!({ "app": f.a1, "to": f.a, "confirmToken": tok }));
    assert_eq!((st, code(&v)), (403, "vault_confirm_required"), "{v}");
    // An Owner login with its own stamp.
    let rosson = Who::Login { username: "rosson".into(), role: Role::Owner };
    let (st, v) = call("/cli/app/handover", &rosson, json!({ "app": f.a1, "to": f.a, "confirmToken": confirm(&f.a1, "connect:rosson") }));
    assert_eq!(st, 200, "{v}");
    assert_eq!(row(&f.a1).owner_workspace, f.a);
}

/// Each blocker refuses on its own, names itself, and changes nothing.
#[test]
fn handover_refuses_each_blocker_by_name() {
    // 0.46.4's guard is the K2_APP_HANDOVER=legacy path from 0.46.5 on
    // (the full handover: handover_tests).
    super::handover::set_test_mode(Some(super::handover::Mode::Legacy));
    let f = fx("block");

    // A running App: its link keeps the old room until it restarts.
    let running = fresh("run", &f.a);
    mark_running(&running);
    assert_refused(&running, &f.b, "handoff_app_running");

    // Guests tied to the old workspace: added by its agent (creator
    // stamp), re-keyed by it (a tie), or marked as its agent login.
    let tied = fresh("tied", &f.a);
    let g = k2_core::skin::add_principal_for_app("tied-added", None, &svc(&tied), None, "agent").unwrap();
    k2_core::skin::creators::stamp(&g.id, &f.a).unwrap();
    k2_core::skin::creators::tie(&g.id, &f.a, "agent:test", k2_core::skin::ties::REASON_CREATED).unwrap();
    let v = assert_refused(&tied, &f.b, "handoff_needs_tie_move");
    let b = v["error"]["blockers"].as_array().unwrap().iter().find(|b| b["code"] == "handoff_needs_tie_move").unwrap().clone();
    assert_eq!(b["items"], json!(["tied-added"]), "names the guest: {v}");
    let rekeyed = fresh("rekey", &f.a);
    let g = k2_core::skin::add_principal_for_app("tied-rekeyed", None, &svc(&rekeyed), None, "owner").unwrap();
    k2_core::skin::creators::tie(&g.id, &f.a, "agent:rekey", k2_core::skin::ties::REASON_PASSWORD).unwrap();
    assert_refused(&rekeyed, &f.b, "handoff_needs_tie_move");
    // Once the owner takes the guest over, it no longer blocks.
    k2_core::skin::creators::clear_agent_tie(&g.id, "owner-token").unwrap();
    let (st, v) = call("/cli/app/handover", &Who::Owner, json!({ "app": rekeyed, "to": f.b, "confirmToken": confirm(&rekeyed, "owner") }));
    assert_eq!(st, 200, "a claimed guest is the owner's: {v}");
    let marked = fresh("mark", &f.a);
    let g = k2_core::skin::add_principal_for_app("agent-login", None, &svc(&marked), None, "owner").unwrap();
    {
        // The owner's `k2 app user agent` mark (skin.db, no API without a route).
        let db = rusqlite::Connection::open(k2_core::paths::k2_home().join("skin.db")).unwrap();
        assert_eq!(db.execute("UPDATE principals SET agent_workspace = ?2 WHERE id = ?1", params![g.id, f.a]).unwrap(), 1);
    }
    assert_refused(&marked, &f.b, "handoff_needs_tie_move");

    // Labels and domains beyond its own label.
    let labelled = fresh("lab", &f.a);
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        let cfg = json!({
            "labels": [{ "label": "shop", "serviceId": "legacy-svc", "legacyPort": 3000, "takenAt": 1, "takenBy": "agent" }],
            "subdomains": [{ "label": "api", "addedAt": 1, "addedBy": "agent" }],
        });
        conn.execute("UPDATE apps SET config_json = ?2 WHERE app_id = ?1", params![labelled, cfg.to_string()]).unwrap();
    }
    let v = assert_refused(&labelled, &f.b, "handoff_needs_label_move");
    let b = v["error"]["blockers"].as_array().unwrap().iter().find(|b| b["code"] == "handoff_needs_label_move").unwrap().clone();
    assert_eq!(b["items"], json!(["shop (taken)", "api (attached)"]), "{v}");
    let domained = fresh("dom", &f.a);
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute("INSERT INTO domain_bindings (apex) VALUES ('blockdom.example')", []).unwrap();
        conn.execute(
            "INSERT INTO domain_names (hostname, apex, role) VALUES ('shop.blockdom.example', 'blockdom.example', ?1)",
            params![format!("app:{domained}")],
        )
        .unwrap();
    }
    assert_refused(&domained, &f.b, "handoff_needs_label_move");

    // A secret the old workspace owns, granted to the App.
    let secret = fresh("sec", &f.a);
    {
        use k2_vault::SecretStore;
        let vault = super::egress_routes::test_seams::vault_handle();
        let name = format!("HO_WS_{}", uuid::Uuid::new_v4().simple().to_string()[..8].to_uppercase());
        let mut put = k2_vault::PutRequest::new(k2_vault::Namespace::User, k2_vault::Scope::workspace(f.a.clone()), &name, k2_vault::Kind::Generic);
        put.origin = "test".into();
        put.default_delivery = Some(k2_vault::Delivery::File);
        put.delivery_ceiling = Some(k2_vault::Delivery::Env);
        vault.put(put, k2_vault::SecretBytes::from_slice(b"v"), &k2_vault::Actor::Owner).unwrap();
        vault
            .grant(
                k2_vault::GrantRequest {
                    selector: k2_vault::Selector::user(k2_vault::Scope::workspace(f.a.clone()), &name),
                    consumer: k2_vault::Consumer::published_service(svc(&secret)),
                    delivery: k2_vault::Delivery::File,
                    env_name: None,
                    file_name: None,
                    hosts_narrowed: None,
                    expires_at: None,
                    version: None,
                },
                &k2_vault::Actor::Owner,
            )
            .unwrap();
        let v = assert_refused(&secret, &f.b, "handoff_needs_secret_move");
        let b = v["error"]["blockers"].as_array().unwrap().iter().find(|b| b["code"] == "handoff_needs_secret_move").unwrap().clone();
        assert_eq!(b["items"], json!([format!("{name} (granted)")]), "{v}");
    }

    // A `.env.k2` reference in its live Seed that resolves only through
    // the old workspace (a server secret granted to that workspace).
    let reffed = fresh("ref", &f.a);
    {
        use k2_vault::SecretStore;
        let vault = super::egress_routes::test_seams::vault_handle();
        let name = format!("HO_SRV_{}", uuid::Uuid::new_v4().simple().to_string()[..8].to_uppercase());
        let mut put = k2_vault::PutRequest::new(k2_vault::Namespace::User, k2_vault::Scope::server(), &name, k2_vault::Kind::Generic);
        put.origin = "test".into();
        put.default_delivery = Some(k2_vault::Delivery::Env);
        put.delivery_ceiling = Some(k2_vault::Delivery::Env);
        vault.put(put, k2_vault::SecretBytes::from_slice(b"v"), &k2_vault::Actor::Owner).unwrap();
        vault
            .grant(
                k2_vault::GrantRequest {
                    selector: k2_vault::Selector::user(k2_vault::Scope::server(), &name),
                    consumer: k2_vault::Consumer::workspace(f.a.clone()),
                    delivery: k2_vault::Delivery::Env,
                    env_name: None,
                    file_name: None,
                    hosts_narrowed: None,
                    expires_at: None,
                    version: None,
                },
                &k2_vault::Actor::Owner,
            )
            .unwrap();
        let seed = format!("seed-ho-{}", uuid::Uuid::new_v4().simple());
        let dir = k2_core::deploy::seed::seed_path(&seed);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".env.k2"), format!("API_TOKEN=k2v://server/{name}\nPLAIN=1\n")).unwrap();
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute(
            "INSERT INTO seeds (hash, seed_id, host_mode, manifest_json, created_with, report_json, signer, size_bytes, created_at, last_used_at) \
             VALUES (?1, ?2, 'cmd', '{}', '{}', '{\"ok\":true}', 'owner', 1, 1, 1)",
            params![seed, reffed],
        )
        .unwrap();
        conn.execute("UPDATE apps SET live_seed = ?2 WHERE app_id = ?1", params![reffed, seed]).unwrap();
    }
    let v = assert_refused(&reffed, &f.b, "handoff_needs_secret_move");
    let b = v["error"]["blockers"].as_array().unwrap().iter().find(|b| b["code"] == "handoff_needs_secret_move").unwrap().clone();
    assert_eq!(b["items"], json!(["API_TOKEN (.env.k2)"]), "{v}");

    // A live App key (its rooms name the old workspace); a revoked one
    // doesn't block.
    let keyed = fresh("key", &f.a);
    let (kid, _raw) = k2_core::api_keys::create_app_key("ci", &svc(&keyed), "{}").unwrap();
    assert_refused(&keyed, &f.b, "handoff_needs_key_move");
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute("UPDATE api_keys SET revoked_at = 1 WHERE id = ?1", params![kid]).unwrap();
    }
    let (st, v) = call("/cli/app/handover", &Who::Owner, json!({ "app": keyed, "to": f.b, "confirmToken": confirm(&keyed, "owner") }));
    assert_eq!(st, 200, "a revoked key carries nothing: {v}");

    // Push set up for it.
    let pushed = fresh("push", &f.a);
    k2_core::skin::app_push::set_switches(&svc(&pushed), k2_core::skin::app_push::Switches { owner_on: Some(true), ..Default::default() }, "owner").unwrap();
    assert_refused(&pushed, &f.b, "handoff_needs_push_move");

    // Every blocker at once is listed together.
    let all = fresh("all", &f.a);
    k2_core::skin::app_push::set_switches(&svc(&all), k2_core::skin::app_push::Switches { previews: Some(false), ..Default::default() }, "owner").unwrap();
    k2_core::api_keys::create_app_key("ci", &svc(&all), "{}").unwrap();
    mark_running(&all);
    let v = assert_refused(&all, &f.b, "handoff_needs_push_move");
    let codes: Vec<&str> = v["error"]["blockers"].as_array().unwrap().iter().map(|b| b["code"].as_str().unwrap()).collect();
    assert_eq!(codes, vec!["handoff_app_running", "handoff_needs_key_move", "handoff_needs_push_move"], "{v}");
}

/// The old workspace's Postgres database (its default here) blocks: the
/// App would quietly resolve to the new workspace's database instead.
#[test]
fn handover_refuses_an_app_on_the_old_workspaces_database() {
    // 0.46.4's guard is the K2_APP_HANDOVER=legacy path from 0.46.5 on
    // (the full handover: handover_tests).
    super::handover::set_test_mode(Some(super::handover::Mode::Legacy));
    let f = fx("pgdb");
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute(
            "INSERT INTO sql_databases (id, project_id, name, status, created_at) VALUES ('ho-d1', ?1, 'ws_sales_db', 'active', 1)",
            params![f.a],
        )
        .unwrap();
    }
    let v = assert_refused(&f.a1, &f.b, "handoff_needs_db_move");
    let b = v["error"]["blockers"].as_array().unwrap().iter().find(|b| b["code"] == "handoff_needs_db_move").unwrap().clone();
    assert_eq!(b["items"], json!(["ws_sales_db"]), "{v}");
    // B has no database, so B's App has none to leave behind: it moves.
    let (st, v) = call("/cli/app/handover", &Who::Owner, json!({ "app": f.b1, "to": f.a, "confirmToken": confirm(&f.b1, "owner") }));
    assert_eq!(st, 200, "an App with no database moves: {v}");
    assert_eq!(row(&f.b1).owner_workspace, f.a);
}

/// Unpark checks the workspace the App was parked from; release checks
/// every App before any moves and refuses the lot.
#[test]
fn unpark_and_release_use_the_same_guard() {
    // 0.46.4's guard is the K2_APP_HANDOVER=legacy path from 0.46.5 on
    // (the full handover: handover_tests).
    super::handover::set_test_mode(Some(super::handover::Mode::Legacy));
    let f = fx("unrel");
    stamp_label(&f.a1, &f.a);
    stamp_label(&f.a2, &f.a);
    let g = k2_core::skin::add_principal_for_app("parked-tied", None, &svc(&f.a2), None, "agent").unwrap();
    k2_core::skin::creators::stamp(&g.id, &f.a).unwrap();
    k2_core::skin::creators::tie(&g.id, &f.a, "agent:test", k2_core::skin::ties::REASON_CREATED).unwrap();

    // Release to B: a2 is unsafe, so neither moves, and no confirm is asked.
    let (st, v) = call("/cli/app/release", &Who::Owner, json!({ "workspace": f.a, "apps": format!("handover:{}", f.b) }));
    assert_eq!((st, code(&v)), (409, guard::NEEDS_V1), "{v}");
    let results = v["error"]["results"].as_array().unwrap();
    assert_eq!(results.len(), 1, "only the unsafe App is listed: {v}");
    assert_eq!(results[0]["app"], json!(f.a2));
    assert!(v["error"]["hint"].as_str().unwrap().contains("0.46.5"), "{v}");
    assert_eq!(row(&f.a1).owner_workspace, f.a, "the safe App didn't move either");
    assert_eq!(row(&f.a2).owner_workspace, f.a);

    // Parked, it still remembers A: unpark to B is refused the same way.
    let (st, v) = call("/cli/app/park", &Who::Owner, json!({ "app": f.a2 }));
    assert_eq!(st, 200, "{v}");
    let (st, v) = call("/cli/app/unpark", &Who::Owner, json!({ "app": f.a2, "to": f.b, "confirmToken": confirm(&f.a2, "owner") }));
    assert_eq!((st, code(&v)), (409, guard::NEEDS_V1), "{v}");
    assert!(v["error"]["blockers"].as_array().unwrap().iter().any(|b| b["code"] == "handoff_needs_tie_move"), "{v}");
    assert_eq!(row(&f.a2).state, store::STATE_PARKED, "still parked");
}
