//! App handover (prd-app-handoff-v1 rev 2; `k2 app handover`): the plan,
//! the owner's card and its apply, offers, guests, secrets, undo. In
//! process, on a scoped DB, a temp HOME (skin.db) and the test vault.

use std::collections::HashMap;

use k2_core::app_grace::config::{AttachedSubdomain, TakenLabel};
use k2_core::connect_users::Role;
use k2_core::deploy::store;
use k2_core::skin::handover as sh;
use k2_vault::{Actor, Consumer, Delivery, GrantRequest, Kind, Namespace, PutRequest, Scope, SecretBytes, SecretStore, Selector};
use rusqlite::params;
use serde_json::{json, Value as J};

use super::routes::Who;

pub(super) struct Fx {
    pub _db: k2_core::db::TestDbGuard,
    pub _home: crate::test_support::TempHome,
    pub a: String,
    pub b: String,
    pub c: String,
    /// A's App under test, and A's other App.
    pub app: String,
    pub other: String,
    pub svc: String,
    pub other_svc: String,
}

fn ws_path(ws: &str) -> String {
    format!("/tmp/k2-hov-test/{ws}")
}

pub(super) fn handle_of(ws: &str) -> String {
    format!("{ws}-h")
}

fn add_ws(ws: &str, name: &str) {
    let h = k2_core::db::shared();
    let conn = h.lock();
    conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?4)", params![ws, name, ws_path(ws), handle_of(ws)]).unwrap();
}

/// An App row with its published service row (as `install` makes them).
/// Returns the service id.
fn add_app(app: &str, ws: &str, tunnel: bool) -> String {
    let h = k2_core::db::shared();
    let conn = h.lock();
    let svc = k2_core::published_services::insert(
        &conn,
        ws,
        app,
        k2_core::published_services::CMD_APP_SENTINEL,
        "/tmp/k2-hov-test/apps",
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
            memory_limit_bytes: Some(256 << 20),
            front_port: 0,
            created_by: "owner",
        },
    )
    .unwrap();
    store::set_state(&conn, app, store::STATE_STOPPED, None).unwrap();
    svc.id
}

/// A plain App of `a` and `b`, no guests or secrets (most tests add what
/// they need).
pub(super) fn fx(tag: &str) -> Fx {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    k2_core::deploy::set_enabled(true);
    let _ = crate::vault::ensure_test_vault();
    let (a, b, c) = (format!("hov-a-{tag}"), format!("hov-b-{tag}"), format!("hov-c-{tag}"));
    add_ws(&a, &format!("Sales {tag}"));
    add_ws(&b, &format!("Ops {tag}"));
    add_ws(&c, &format!("Third {tag}"));
    let (app, other) = (format!("hv-{tag}"), format!("hvo-{tag}"));
    let svc = add_app(&app, &a, true);
    let other_svc = add_app(&other, &a, false);
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        k2_core::db::schema::SubdomainWorkspace::self_stamp(&conn, &app, &a, None).unwrap();
    }
    super::alerts::set_sink(Some(Box::new(|_: &str, _: &str| {})));
    Fx { _db: db, _home: home, a, b, c, app, other, svc, other_svc }
}

pub(super) fn agent(ws: &str) -> Who {
    Who::Agent { project_id: ws.into(), session_id: format!("sess-{ws}") }
}

/// The held-request routes are served beside the App routes (the grace
/// door), so they dispatch on their own here.
fn route(path: &str, who: &Who, body: &[u8], q: &HashMap<String, String>) -> crate::cli_response::CliResponse {
    if super::approvals::ROUTES.contains(&path) {
        return super::routes::respond(super::approvals::handle(path, who, body, q));
    }
    super::routes::handle(path, who.clone(), body, q)
}

pub(super) fn call(path: &str, who: &Who, body: J) -> (u16, J) {
    let r = route(path, who, body.to_string().as_bytes(), &HashMap::new());
    let n: u16 = r.status.split_whitespace().next().unwrap().parse().unwrap();
    (n, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{path}: not JSON ({e}): {}", r.body)))
}

pub(super) fn get(path: &str, who: &Who, q: &[(&str, &str)]) -> (u16, J) {
    let p: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let r = route(path, who, b"", &p);
    let n: u16 = r.status.split_whitespace().next().unwrap().parse().unwrap();
    (n, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{path}: not JSON ({e}): {}", r.body)))
}

pub(super) fn code(v: &J) -> &str {
    v["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {v}"))
}

pub(super) fn plan(f: &Fx, who: &Who) -> (u16, J) {
    get("/cli/app/handover/plan", who, &[("app", &f.app), ("to", &f.b)])
}

/// Labels: the App's own (stamped by `fx`), a taken one and a domain one.
pub(super) fn add_labels(f: &Fx, taken: &str, sub: &str) {
    let h = k2_core::db::shared();
    let conn = h.lock();
    for l in [taken, sub] {
        k2_core::db::schema::SubdomainWorkspace::self_stamp(&conn, l, &f.a, None).unwrap();
    }
    let mut cfg = k2_core::app_grace::config::load(&conn, &f.app).unwrap();
    cfg.labels.push(TakenLabel { label: taken.into(), service_id: "svc-legacy".into(), legacy_port: 4100, taken_at: 1, taken_by: "agent:a".into() });
    cfg.subdomains.push(AttachedSubdomain { label: sub.into(), added_at: 1, added_by: "agent:a".into(), previous_target: None });
    conn.execute("UPDATE apps SET config_json = ?2 WHERE app_id = ?1", params![f.app, serde_json::to_string(&cfg).unwrap()]).unwrap();
}

/// A guest with a grant on `svc` in `rooms`.
pub(super) fn guest(name: &str, svc: &str, rooms: &[&str]) -> String {
    let p = k2_core::skin::add_principal(name).unwrap();
    k2_core::skin::create_grant(&k2_core::skin::SkinGrantWrite {
        subject_kind: "principal".into(),
        subject_id: p.id.clone(),
        kind: "app".into(),
        target_id: svc.into(),
        role: None,
        scope: Some(serde_json::to_string(rooms).unwrap()),
        created_by: Some("owner-token".into()),
    })
    .unwrap();
    p.id
}

pub(super) fn grant_more(pid: &str, svc: &str, rooms: &[&str]) {
    k2_core::skin::create_grant(&k2_core::skin::SkinGrantWrite {
        subject_kind: "principal".into(),
        subject_id: pid.into(),
        kind: "app".into(),
        target_id: svc.into(),
        role: None,
        scope: Some(serde_json::to_string(rooms).unwrap()),
        created_by: Some("owner-token".into()),
    })
    .unwrap();
}

/// A workspace secret of `ws`.
pub(super) fn put_ws_secret(ws: &str, name: &str, value: &str) -> Selector {
    let v = crate::vault::ensure_test_vault();
    let mut r = PutRequest::new(Namespace::User, Scope::workspace(ws), name, Kind::ApiKey);
    r.default_delivery = Some(Delivery::Env);
    r.delivery_ceiling = Some(Delivery::Env);
    v.put(r, SecretBytes::from_slice(value.as_bytes()), &Actor::Owner).expect("put workspace secret");
    Selector::user(Scope::workspace(ws), name)
}

pub(super) fn grant_to_service(sel: &Selector, svc: &str, env: &str) {
    let v = crate::vault::ensure_test_vault();
    v.grant(
        GrantRequest {
            selector: sel.clone(),
            consumer: Consumer::published_service(svc.to_string()),
            delivery: Delivery::Env,
            env_name: Some(env.into()),
            file_name: None,
            hosts_narrowed: None,
            expires_at: None,
            version: None,
        },
        &Actor::Owner,
    )
    .expect("grant to the service");
}

/// A live Seed whose `.env.k2` holds `lines`. Returns the Seed hash.
pub(super) fn live_seed_with_env(app: &str, lines: &[(&str, String)]) -> String {
    let hash: String = {
        use sha2::{Digest, Sha256};
        Sha256::digest(format!("seed-{app}").as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
    };
    let dir = k2_core::deploy::seed::seed_path(&hash);
    std::fs::create_dir_all(&dir).unwrap();
    let body: String = lines.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    std::fs::write(dir.join(".env.k2"), body).unwrap();
    let h = k2_core::db::shared();
    let conn = h.lock();
    conn.execute(
        "INSERT OR IGNORE INTO seeds (hash, seed_id, version, host_mode, manifest_json, created_with, report_json, signer, size_bytes, created_at, last_used_at) \
         VALUES (?1, ?2, '1.0.0', 'cmd', ?3, 'test', '{}', 'owner', 1, 1, 1)",
        params![hash, app, json!({ "id": app, "version": "1.0.0" }).to_string()],
    )
    .unwrap();
    conn.execute("UPDATE apps SET live_seed = ?2 WHERE app_id = ?1", params![app, hash]).unwrap();
    hash
}

/// A's database, active, on major 17.
pub(super) fn add_db(ws: &str, name: &str) {
    let h = k2_core::db::shared();
    let conn = h.lock();
    conn.execute(
        "INSERT INTO sql_databases (id, project_id, name, status, created_at, cluster_major) VALUES (?1, ?2, ?3, 'active', 1, 17)",
        params![format!("db-{name}"), ws, name],
    )
    .unwrap();
}

fn class_of(v: &J, user: &str) -> String {
    v["guests"]["list"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["username"] == user)
        .unwrap_or_else(|| panic!("no guest {user} in {v}"))["class"]
        .as_str()
        .unwrap()
        .to_string()
}

// ── S0: the plan ─────────────────────────────────────────────────────

/// T0.1: every row of §4.2, each with its case.
#[test]
fn the_plan_lists_every_row_with_its_case() {
    let f = fx("t01");
    add_labels(&f, "agent-t01", "dannon-t01");
    let (o1, o2) = (guest("own1-t01", &f.svc, &[&f.a]), guest("own2-t01", &f.svc, &[&f.a]));
    let added = guest("added-t01", &f.svc, &[&f.a]);
    k2_core::skin::creators::stamp(&added, &f.a).unwrap();
    k2_core::skin::creators::tie(&added, &f.a, "agent:test", k2_core::skin::ties::REASON_CREATED).unwrap();
    let shared = guest("shared-t01", &f.svc, &[&f.a]);
    k2_core::skin::creators::stamp(&shared, &f.a).unwrap();
    k2_core::skin::creators::tie(&shared, &f.a, "agent:test", k2_core::skin::ties::REASON_CREATED).unwrap();
    grant_more(&shared, &f.other_svc, &[&f.a]);
    let stripe = put_ws_secret(&f.a, "STRIPE_KEY_T01", "sk_test_t01");
    grant_to_service(&stripe, &f.svc, "STRIPE_KEY");
    put_ws_secret(&f.a, "DATABASE_URL", "postgres://u:p@127.0.0.1:5432/ws_t01");
    live_seed_with_env(&f.app, &[("DATABASE_URL", format!("k2v://ws/{}/DATABASE_URL", handle_of(&f.a)))]);
    add_db(&f.a, "ws_t01");

    let (st, v) = plan(&f, &Who::Owner);
    assert_eq!(st, 200, "{v}");
    // Labels: all three, with their kinds, held by A.
    let labels: Vec<(String, String, String)> = v["labels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| (l["label"].as_str().unwrap().into(), l["kind"].as_str().unwrap().into(), l["holder"].as_str().unwrap_or("").into()))
        .collect();
    assert_eq!(
        labels,
        vec![
            (f.app.clone(), "app".into(), f.a.clone()),
            ("agent-t01".into(), "taken".into(), f.a.clone()),
            ("dannon-t01".into(), "subdomain".into(), f.a.clone())
        ],
        "{v}"
    );
    // Guests: two owner's, one A-added (tied), one shared.
    assert_eq!(v["guests"]["total"], 4, "{v}");
    assert_eq!(class_of(&v, "own1-t01"), sh::CLASS_OWNER);
    assert_eq!(class_of(&v, "own2-t01"), sh::CLASS_OWNER);
    assert_eq!(class_of(&v, "added-t01"), sh::CLASS_GIVER_TIED);
    assert_eq!(class_of(&v, "shared-t01"), sh::CLASS_SHARED);
    assert_eq!(v["rooms"]["grants"], 4, "every grant on the App names room A: {v}");
    let _ = (o1, o2);
    // Secrets: the granted one and the reference, both A's, both move.
    let secrets: Vec<(String, String, String)> = v["secrets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["name"].as_str().unwrap().into(), s["via"].as_str().unwrap().into(), s["case"].as_str().unwrap().into()))
        .collect();
    assert_eq!(
        secrets,
        vec![("DATABASE_URL".into(), "ref".into(), "move".into()), ("STRIPE_KEY_T01".into(), "grant".into(), "move".into())],
        "{v}"
    );
    // The database: A's, the App its sole user.
    assert_eq!(v["database"]["case"], "sole", "{v}");
    assert_eq!(v["database"]["name"], "ws_t01", "{v}");
    assert_eq!(v["database"]["source"], "ref", "{v}");
    // B: switch 1 off (the card turns it on).
    assert_eq!(v["toSettings"]["build"], false, "{v}");
    assert!(v["lines"].as_array().unwrap().iter().any(|l| l.as_str().unwrap().contains("Turns on \"Build and run Apps\"")), "{v}");
    assert_eq!(v["to"]["handle"], handle_of(&f.b), "{v}");
    let h = v["planHash"].as_str().unwrap();
    assert_eq!(h.len(), 64, "sha256 hex: {h}");
}

/// T0.2: same inputs, same hash; a new guest changes it.
#[test]
fn the_plan_hash_is_stable_and_a_new_guest_changes_it() {
    let f = fx("t02");
    guest("g1-t02", &f.svc, &[&f.a]);
    let (_, v1) = plan(&f, &Who::Owner);
    let (_, v2) = plan(&f, &Who::Owner);
    assert_eq!(v1["planHash"], v2["planHash"], "same inputs, same hash");
    guest("g2-t02", &f.svc, &[&f.a]);
    let (_, v3) = plan(&f, &Who::Owner);
    assert_ne!(v1["planHash"], v3["planHash"], "a new guest changes the hash");
    // The options are part of what the card shows.
    let (_, v4) = get("/cli/app/handover/plan", &Who::Owner, &[("app", &f.app), ("to", &f.b), ("db", "share")]);
    assert_ne!(v3["planHash"], v4["planHash"], "an option changes the hash");
    let (st, v) = get("/cli/app/handover/plan", &Who::Owner, &[("app", &f.app), ("to", &f.b), ("db", "bogus")]);
    assert_eq!((st, code(&v)), (400, "bad_request"), "{v}");
}

/// T0.3: a Member login is refused; a foreign passport gets app_not_found.
#[test]
fn a_member_is_refused_and_a_foreign_agent_sees_no_app() {
    let f = fx("t03");
    let (st, v) = plan(&f, &Who::Login { username: "mo".into(), role: Role::Member });
    assert_eq!((st, code(&v)), (403, "owner_only"), "{v}");
    let (st, v) = plan(&f, &agent(&f.c));
    assert_eq!((st, code(&v)), (404, "app_not_found"), "no existence leak: {v}");
    let (st, v) = plan(&f, &agent(&f.b));
    assert_eq!((st, code(&v)), (404, "app_not_found"), "B doesn't see A's App before an offer: {v}");
    // The App's own agent, an Admin login and the owner see it.
    for who in [agent(&f.a), Who::Login { username: "ad".into(), role: Role::Admin }, Who::Owner] {
        let (st, v) = plan(&f, &who);
        assert_eq!(st, 200, "{who:?}: {v}");
    }
    let (st, v) = get("/cli/app/handover/plan", &Who::Owner, &[("app", &f.app), ("to", &f.a)]);
    assert_eq!((st, code(&v)), (409, "handover_same_workspace"), "{v}");
    let (st, v) = get("/cli/app/handover/plan", &Who::Owner, &[("app", &f.app), ("to", "no-such-ws")]);
    assert_eq!(st, 404, "{v}");
}

// ── S1: the owner's card and its apply ───────────────────────────────

pub(super) fn approve(req: &str) -> (u16, J) {
    let tok = super::egress_routes::test_seams::vault_handle().confirm.mint(super::approvals::CONFIRM_ACTION, Some(req), "owner", "local-auth");
    call("/cli/app/approve", &Who::Owner, json!({ "request": req, "confirmToken": tok }))
}

pub(super) fn ask(f: &Fx) -> (u16, J) {
    call("/cli/app/handover", &Who::Owner, json!({ "app": f.app, "to": f.b }))
}

pub(super) fn req_id(v: &J) -> String {
    v["request"]["id"].as_str().unwrap_or_else(|| panic!("no request: {v}")).to_string()
}

pub(super) fn holder(label: &str) -> Option<String> {
    let h = k2_core::db::shared();
    let conn = h.lock();
    k2_core::db::schema::SubdomainWorkspace::owner_of(&conn, label).unwrap()
}

pub(super) fn app_row(app: &str) -> store::AppRow {
    let h = k2_core::db::shared();
    let conn = h.lock();
    store::get_app(&conn, app).unwrap().unwrap()
}

pub(super) fn handover_row(id: &str) -> k2_core::deploy::handover::Handover {
    let h = k2_core::db::shared();
    let conn = h.lock();
    k2_core::deploy::handover::get(&conn, id).unwrap().unwrap()
}

pub(super) fn notes_for(ws: &str) -> Vec<String> {
    super::ownership::NOTES.lock().unwrap().iter().filter(|(w, _)| w == ws).map(|(_, t)| t.clone()).collect()
}

/// T1.1 + T1.7: every label moves; the engine is untouched.
#[test]
fn every_label_moves_and_nothing_restarts() {
    let f = fx("t11");
    add_labels(&f, "agent-t11", "dannon-t11");
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute(
            "UPDATE apps SET front_port = 41234, live_deployment = 'dep-t11', folder_path = '/tmp/k2-hov-test/src' WHERE app_id = ?1",
            params![f.app],
        )
        .unwrap();
    }
    let before = app_row(&f.app);
    let (st, v) = ask(&f);
    assert_eq!((st, v["status"].as_str()), (202, Some("held")), "{v}");
    assert_eq!(holder("agent-t11").as_deref(), Some(f.a.as_str()), "nothing moves before the approval");
    let (st, v) = approve(&req_id(&v));
    assert_eq!((st, v["status"].as_str()), (200, Some("approved")), "{v}");
    for l in [f.app.as_str(), "agent-t11", "dannon-t11"] {
        assert_eq!(holder(l).as_deref(), Some(f.b.as_str()), "{l} is B's");
    }
    let after = app_row(&f.app);
    assert_eq!(after.owner_workspace, f.b);
    assert_eq!(
        (after.front_port, after.live_deployment.as_deref(), after.state.as_str()),
        (before.front_port, before.live_deployment.as_deref(), before.state.as_str())
    );
    assert_eq!(after.folder_path, None, "A's folder is no longer the App's source (A24)");
    // Taken labels are final: no way back to A's legacy service (Q5).
    let cfg = super::labels::load_cfg(&f.app).unwrap();
    assert_eq!(cfg.labels.len(), 1);
    assert_eq!(cfg.labels[0].service_id, "", "{cfg:?}");
    let svc_ws: String = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.query_row("SELECT project_id FROM published_services WHERE id = ?1", params![f.svc], |r| r.get(0)).unwrap()
    };
    assert_eq!(svc_ws, f.b);
    // B maintains it; A no longer sees it.
    let (st, _) = get("/cli/app/inspect", &agent(&f.b), &[("app", &f.app)]);
    assert_eq!(st, 200);
    let (st, _) = get("/cli/app/inspect", &agent(&f.a), &[("app", &f.app)]);
    assert_eq!(st, 404);
    assert!(
        {
            let h = k2_core::db::shared();
            let conn = h.lock();
            k2_core::deploy::audit::list_for(&conn, k2_core::deploy::audit::SUBJECT_APP, &f.app, 50).unwrap().iter().any(|r| r.event == "handover")
        },
        "audited"
    );
}

/// T1.3: an untied guest's live session keeps resolving and talks to B.
#[test]
fn a_live_guest_session_keeps_working_and_reaches_b() {
    let f = fx("t13");
    let pid = guest("ann-t13", &f.svc, &[&f.a]);
    let s = k2_core::skin::create_app_session_token("ann-t13", &f.svc).unwrap();
    let pass = k2_core::skin::resolve_skin_token(&s.raw).expect("live");
    assert_eq!(pass.room_policy.keys().cloned().collect::<Vec<_>>(), vec![f.a.clone()]);
    let (_, v) = ask(&f);
    let (st, v) = approve(&req_id(&v));
    assert_eq!(st, 200, "{v}");
    let pass = k2_core::skin::resolve_skin_token(&s.raw).expect("the session survives the handover");
    assert_eq!(pass.room_policy.keys().cloned().collect::<Vec<_>>(), vec![f.b.clone()], "it talks to B now");
    assert_eq!(pass.rooms, vec![f.b.clone()]);
    // A new sign-in lands in B's room too.
    let s2 = k2_core::skin::create_app_session_token("ann-t13", &f.svc).unwrap();
    assert_eq!(s2.meta.rooms, vec![f.b.clone()]);
    // Room A on the guest's other App is A's: untouched.
    grant_more(&pid, &f.other_svc, &[&f.a]);
    let s3 = k2_core::skin::create_app_session_token("ann-t13", &f.other_svc).unwrap();
    assert_eq!(s3.meta.rooms, vec![f.a.clone()], "A's other App still reaches A");
}

/// T1.4: A's waiting request for the App ends; A hears.
#[test]
fn a_waiting_request_of_a_is_withdrawn_and_a_hears() {
    let f = fx("t14");
    let rid = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        let n = k2_core::app_grace::requests::NewRequest {
            kind: k2_core::app_grace::requests::KIND_CONFIG,
            app_id: Some(&f.app),
            service_id: Some(&f.svc),
            app_label: &f.app,
            workspace_id: &f.a,
            body: json!({ "ops": [] }),
            diff: json!({ "lines": ["memory 1 GiB"] }),
            summary: "memory 1 GiB".into(),
            reasons: vec![],
            reason: None,
            deployment_id: None,
            requested_by: "agent:a",
            requester_workspace: Some(&f.a),
            requester_session: None,
        };
        k2_core::app_grace::requests::create(&conn, &n, k2_core::deploy::now()).unwrap().id
    };
    super::ownership::NOTES.lock().unwrap().retain(|(w, _)| w != &f.a && w != &f.b);
    let (_, v) = ask(&f);
    assert_eq!(v["plan"]["requests"].as_array().unwrap().len(), 1, "the card lists it: {v}");
    let (st, v) = approve(&req_id(&v));
    assert_eq!(st, 200, "{v}");
    let r = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        k2_core::app_grace::requests::get(&conn, &rid).unwrap().unwrap()
    };
    assert_eq!((r.status.as_str(), r.decision_reason.as_deref()), ("withdrawn", Some("handed_over")));
    let a_notes = notes_for(&f.a);
    assert_eq!(a_notes.len(), 1, "{a_notes:?}");
    assert!(a_notes[0].contains("You no longer manage it"), "{a_notes:?}");
    assert_eq!(notes_for(&f.b).len(), 1);
}

/// T1.5: the owner token alone holds; a reused or foreign confirm is
/// refused; a fresh one applies. A passport never approves.
#[test]
fn the_card_needs_a_fresh_confirm_and_never_a_passport() {
    let f = fx("t15");
    let (st, v) = ask(&f);
    assert_eq!((st, v["status"].as_str()), (202, Some("held")), "{v}");
    let req = req_id(&v);
    let hid = v["handover"]["id"].as_str().unwrap().to_string();
    assert_eq!(handover_row(&hid).state, "held");
    // Asking again answers the same card.
    let (st, v2) = ask(&f);
    assert_eq!((st, req_id(&v2), v2["already"].as_bool()), (202, req.clone(), Some(true)), "{v2}");
    let (st, v) = call("/cli/app/approve", &Who::Owner, json!({ "request": req }));
    assert_eq!((st, code(&v)), (403, "vault_confirm_required"), "{v}");
    for who in [agent(&f.a), agent(&f.b), Who::Login { username: "ad".into(), role: Role::Admin }] {
        let tok = super::egress_routes::test_seams::vault_handle().confirm.mint(super::approvals::CONFIRM_ACTION, Some(&req), "owner", "local-auth");
        let (st, v) = call("/cli/app/approve", &who, json!({ "request": req, "confirmToken": tok }));
        assert_eq!((st, code(&v)), (403, "owner_only"), "{who:?}: {v}");
    }
    let tok = super::egress_routes::test_seams::vault_handle().confirm.mint(super::approvals::CONFIRM_ACTION, Some("w_other"), "owner", "local-auth");
    let (st, v) = call("/cli/app/approve", &Who::Owner, json!({ "request": req, "confirmToken": tok }));
    assert_eq!((st, code(&v)), (403, "vault_confirm_required"), "a confirm for another request: {v}");
    assert_eq!(app_row(&f.app).owner_workspace, f.a, "nothing moved");
    let tok = super::egress_routes::test_seams::vault_handle().confirm.mint(super::approvals::CONFIRM_ACTION, Some(&req), "owner", "local-auth");
    let (st, v) = call("/cli/app/approve", &Who::Owner, json!({ "request": req, "confirmToken": tok }));
    assert_eq!(st, 200, "{v}");
    let (st, v) = call("/cli/app/approve", &Who::Owner, json!({ "request": req, "confirmToken": tok }));
    assert_eq!((st, code(&v)), (403, "vault_confirm_required"), "a confirm works once: {v}");
    let h = handover_row(&hid);
    assert_eq!((h.state.as_str(), h.steps_done), ("applied", 7), "{h:?}");
    assert!(h.window_until.unwrap() > k2_core::deploy::now() + 71 * 3600, "a 72 h window");
    assert!(h.journal.is_some());
    // B's switch 1 was off: the approval turned it on.
    assert!(super::access::level(&f.b) >= super::access::Level::Build);
    // Only the owner asks for the card.
    let (st, v) = call("/cli/app/handover", &Who::Login { username: "ad".into(), role: Role::Admin }, json!({ "app": f.other, "to": f.b }));
    assert_eq!((st, code(&v)), (403, "owner_only"), "{v}");
}

/// T1.6: an App with a guest A tied is refused, naming it; nothing moves.
#[test]
fn a_tied_guest_stops_the_handover_and_nothing_changes() {
    if super::handover_plan::TIES_MOVE {
        return;
    }
    let f = fx("t16");
    let pid = guest("tied-t16", &f.svc, &[&f.a]);
    k2_core::skin::creators::stamp(&pid, &f.a).unwrap();
    k2_core::skin::creators::tie(&pid, &f.a, "agent:test", k2_core::skin::ties::REASON_CREATED).unwrap();
    let (st, v) = ask(&f);
    assert_eq!((st, code(&v)), (409, "handover_needs_tie_move"), "{v}");
    assert_eq!(v["error"]["blockers"][0]["names"], json!(["tied-t16"]), "{v}");
    assert_eq!(app_row(&f.app).owner_workspace, f.a);
    assert_eq!(holder(&f.app).as_deref(), Some(f.a.as_str()));
    let s = k2_core::skin::create_app_session_token("tied-t16", &f.svc).unwrap();
    assert_eq!(s.meta.rooms, vec![f.a.clone()], "rooms untouched");
}

/// T2.5 (owner path): the plan changed after the card → stale, nothing
/// moves.
#[test]
fn a_guest_added_after_the_card_makes_it_stale() {
    let f = fx("t25o");
    let (_, v) = ask(&f);
    let req = req_id(&v);
    let hid = v["handover"]["id"].as_str().unwrap().to_string();
    guest("late-t25o", &f.svc, &[&f.a]);
    let (st, v) = approve(&req);
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["status"], "failed", "{v}");
    assert_eq!(v["error"]["code"], "handover_stale", "{v}");
    assert_eq!(handover_row(&hid).state, "stale");
    assert_eq!(app_row(&f.app).owner_workspace, f.a, "nothing moved");
    // A new card for the new plan goes through.
    let (st, v) = ask(&f);
    assert_eq!(st, 202, "{v}");
    let (st, v) = approve(&req_id(&v));
    assert_eq!((st, v["status"].as_str()), (200, Some("approved")), "{v}");
}

/// The card is denied: the handover ends `denied`; a new one may start.
#[test]
fn a_denied_card_ends_the_handover() {
    let f = fx("deny");
    let (_, v) = ask(&f);
    let hid = v["handover"]["id"].as_str().unwrap().to_string();
    let (st, d) = call("/cli/app/deny", &Who::Login { username: "ad".into(), role: Role::Admin }, json!({ "request": req_id(&v) }));
    assert_eq!(st, 200, "{d}");
    assert_eq!(handover_row(&hid).state, "denied");
    let (st, v) = ask(&f);
    assert_eq!(st, 202, "{v}");
}

/// A write on an App whose handover is applying waits.
#[test]
fn a_write_during_the_apply_is_refused() {
    let f = fx("busy");
    assert!(super::handover::refuse_if_busy("/cli/app/config", json!({ "app": f.app }).to_string().as_bytes()).is_none(), "nothing applying");
    {
        let _b = super::handover::test_busy(&f.app);
        let (st, v) = call("/cli/app/stop", &Who::Owner, json!({ "app": f.app }));
        assert_eq!((st, code(&v)), (409, "handover_in_progress"), "{v}");
        let (st, v) = call("/cli/app/stop", &Who::Owner, json!({ "app": f.other }));
        assert_eq!(st, 200, "another App is fine: {v}");
    }
    let (st, v) = call("/cli/app/stop", &Who::Owner, json!({ "app": f.app }));
    assert_eq!(st, 200, "{v}");
}

/// K2_APP_HANDOVER=off refuses; legacy keeps 0.46.3's direct move.
#[test]
fn the_switch_turns_the_handover_off_or_back_to_legacy() {
    let f = fx("mode");
    super::handover::set_test_mode(Some(super::handover::Mode::Off));
    let (st, v) = ask(&f);
    assert_eq!((st, code(&v)), (403, "handover_off"), "{v}");
    let (st, v) = plan(&f, &Who::Owner);
    assert_eq!((st, code(&v)), (403, "handover_off"), "{v}");
    super::handover::set_test_mode(Some(super::handover::Mode::Legacy));
    let (st, v) = plan(&f, &Who::Owner);
    assert_eq!(st, 404, "{v}");
    let (st, v) = ask(&f);
    super::handover::set_test_mode(None);
    assert_eq!((st, code(&v)), (403, "vault_confirm_required"), "legacy: 0.46.4's guarded move, the owner in person: {v}");
    assert_eq!(v["error"]["action"], super::handover_guard::CONFIRM_ACTION, "{v}");
    assert_eq!(app_row(&f.app).owner_workspace, f.a);
}

// ── S2: A offers, B accepts, the owner's card ────────────────────────

pub(super) fn switch1(ws: &str, on: bool) {
    k2_core::workspace::settings::set_agents_can_manage_skin(&ws_path(ws), on).unwrap();
}

pub(super) fn offer(f: &Fx) -> (u16, J) {
    call("/cli/app/handover", &agent(&f.a), json!({ "app": f.app, "to": handle_of(&f.b), "brief": "# Jobs\nthe cron runs at 3am" }))
}

/// T2.1: A offers (`offered`); B accepts and the card is held with its
/// lines; the owner approves.
#[test]
fn a_offers_b_accepts_and_the_owner_approves() {
    let f = fx("t21");
    // Without switch 1 an agent can't offer.
    let (st, v) = offer(&f);
    assert_eq!((st, code(&v)), (403, "apps_manage_off"), "{v}");
    switch1(&f.a, true);
    let (st, v) = offer(&f);
    assert_eq!((st, v["handover"]["state"].as_str()), (202, Some("offered")), "{v}");
    let hid = v["handover"]["id"].as_str().unwrap().to_string();
    assert!(v["request"].is_null(), "no card before B accepts: {v}");
    assert_eq!(notes_for(&f.b).len(), 1, "B hears about the offer");
    assert!(notes_for(&f.b)[0].contains("the cron runs at 3am"), "the brief rides along");
    // Only B (or the owner) accepts; a third agent sees nothing.
    let (st, v) = call("/cli/app/handover/accept", &agent(&f.c), json!({ "app": f.app }));
    assert_eq!((st, code(&v)), (404, "app_not_found"), "{v}");
    let (st, v) = call("/cli/app/handover/accept", &agent(&f.a), json!({ "app": f.app }));
    assert_eq!((st, code(&v)), (403, "owner_only"), "A can't accept its own offer: {v}");
    let (st, v) = call("/cli/app/handover/accept", &agent(&f.b), json!({ "app": f.app }));
    assert_eq!((st, v["status"].as_str()), (202, Some("held")), "{v}");
    assert_eq!(handover_row(&hid).state, "held");
    let lines = v["request"]["diff"]["lines"].as_array().unwrap();
    assert!(lines.iter().any(|l| l.as_str().unwrap().starts_with("Hand over")), "{v}");
    // The brief: A and B read it; a third agent doesn't.
    let (st, v2) = get("/cli/app/handover/brief", &agent(&f.b), &[("app", &f.app)]);
    assert_eq!((st, v2["brief"].as_str()), (200, Some("# Jobs\nthe cron runs at 3am")), "{v2}");
    let (st, _) = get("/cli/app/handover/brief", &agent(&f.c), &[("app", &f.app)]);
    assert_eq!(st, 404);
    let (st, s) = get("/cli/app/handover/status", &agent(&f.b), &[]);
    assert_eq!((st, s["handovers"].as_array().unwrap().len()), (200, 1), "{s}");
    let (st, s) = get("/cli/app/handover/status", &agent(&f.c), &[]);
    assert_eq!((st, s["handovers"].as_array().unwrap().len()), (200, 0), "{s}");
    let (st, v) = approve(&req_id(&v));
    assert_eq!((st, v["status"].as_str()), (200, Some("approved")), "{v}");
    assert_eq!(app_row(&f.app).owner_workspace, f.b);
    assert_eq!(handover_row(&hid).state, "applied");
}

/// T2.2: neither A's nor B's passport approves its card.
#[test]
fn neither_agent_approves_the_card() {
    let f = fx("t22");
    switch1(&f.a, true);
    offer(&f);
    let (_, v) = call("/cli/app/handover/accept", &agent(&f.b), json!({ "app": f.app }));
    let req = req_id(&v);
    for who in [agent(&f.a), agent(&f.b)] {
        let tok = super::egress_routes::test_seams::vault_handle().confirm.mint(super::approvals::CONFIRM_ACTION, Some(&req), "owner", "local-auth");
        let (st, v) = call("/cli/app/approve", &who, json!({ "request": req, "confirmToken": tok }));
        assert_eq!((st, code(&v)), (403, "owner_only"), "{who:?}: {v}");
    }
    assert_eq!(app_row(&f.app).owner_workspace, f.a);
}

/// T2.3: B reads the App only while it accepted; after a cancel, no more.
#[test]
fn b_shadows_only_while_it_accepted() {
    let f = fx("t23");
    switch1(&f.a, true);
    let (st, _) = get("/cli/app/inspect", &agent(&f.b), &[("app", &f.app)]);
    assert_eq!(st, 404, "before the offer");
    offer(&f);
    let (st, _) = get("/cli/app/inspect", &agent(&f.b), &[("app", &f.app)]);
    assert_eq!(st, 404, "offered, not accepted yet");
    call("/cli/app/handover/accept", &agent(&f.b), json!({ "app": f.app }));
    for path in ["/cli/app/inspect", "/cli/app/history"] {
        let (st, v) = get(path, &agent(&f.b), &[("app", &f.app)]);
        assert_eq!(st, 200, "{path}: B shadows: {v}");
    }
    // Logs: B reaches the App (no deployment yet is the App's own answer).
    let (st, v) = get("/cli/app/logs", &agent(&f.b), &[("app", &f.app)]);
    assert_eq!((st, code(&v)), (404, "not_found"), "B sees the App, it has no deployment: {v}");
    // Reads only: B changes nothing.
    switch1(&f.b, true);
    let (st, v) = call("/cli/app/stop", &agent(&f.b), json!({ "app": f.app }));
    assert_eq!((st, code(&v)), (404, "app_not_found"), "{v}");
    // A cancels: B no longer sees it, and the card is withdrawn.
    let (st, v) = call("/cli/app/handover/cancel", &agent(&f.a), json!({ "app": f.app }));
    assert_eq!((st, v["handover"]["state"].as_str()), (200, Some("cancelled")), "{v}");
    let (st, _) = get("/cli/app/inspect", &agent(&f.b), &[("app", &f.app)]);
    assert_eq!(st, 404, "after the cancel");
    let (st, a) = get("/cli/app/approvals", &Who::Owner, &[("state", "pending")]);
    assert_eq!(st, 200);
    assert!(a["requests"].as_array().unwrap().iter().all(|r| r["appLabel"] != f.app.as_str()), "the card went with it: {a}");
}

/// T2.4: one open handover per App; B can decline.
#[test]
fn one_open_handover_per_app_and_b_may_decline() {
    let f = fx("t24");
    switch1(&f.a, true);
    let (st, _) = offer(&f);
    assert_eq!(st, 202);
    let (st, v) = call("/cli/app/handover", &agent(&f.a), json!({ "app": f.app, "to": f.c }));
    assert_eq!((st, code(&v)), (409, "handover_open"), "{v}");
    let (st, v) = ask(&f);
    assert_eq!((st, code(&v)), (409, "handover_open"), "the owner too: an offer is open: {v}");
    let (st, v) = call("/cli/app/handover/decline", &agent(&f.b), json!({ "app": f.app, "reason": "too busy" }));
    assert_eq!((st, v["handover"]["state"].as_str()), (200, Some("declined")), "{v}");
    assert!(notes_for(&f.a).iter().any(|n| n.contains("too busy")), "A hears why");
    let (st, v) = call("/cli/app/handover", &agent(&f.a), json!({ "app": f.app, "to": f.c }));
    assert_eq!(st, 202, "a new offer may start: {v}");
}

/// T2.5: a guest added after B accepted → the card is stale at approval.
#[test]
fn an_offer_whose_plan_changed_is_stale_at_approval() {
    let f = fx("t25");
    switch1(&f.a, true);
    offer(&f);
    let (_, v) = call("/cli/app/handover/accept", &agent(&f.b), json!({ "app": f.app }));
    guest("late-t25", &f.svc, &[&f.a]);
    let (st, v) = approve(&req_id(&v));
    assert_eq!((st, v["error"]["code"].as_str()), (200, Some("handover_stale")), "{v}");
    assert_eq!(app_row(&f.app).owner_workspace, f.a);
}

// ── S3: guests, ties and credentials ─────────────────────────────────

fn tie_of(pid: &str) -> Option<String> {
    k2_core::skin::creators::agent_tie(pid).unwrap()
}

/// T3.1: a guest A added (and set the password of) stays signed in, is
/// re-tied to B, and A's known password stops working.
#[test]
fn a_guest_a_set_up_is_retied_and_its_password_retired() {
    let f = fx("t31");
    let pid = guest("bea-t31", &f.svc, &[&f.a]);
    k2_core::skin::creators::stamp(&pid, &f.a).unwrap();
    k2_core::skin::creators::tie(&pid, &f.a, "agent:t31", k2_core::skin::ties::REASON_PASSWORD).unwrap();
    k2_core::skin::set_principal_password("bea-t31", Some("a-knows-this-1")).unwrap();
    let s = k2_core::skin::create_app_session_token("bea-t31", &f.svc).unwrap();
    let (_, v) = ask(&f);
    assert!(
        v["plan"]["lines"].as_array().unwrap().iter().any(|l| l.as_str().unwrap().contains("passwords were set by")),
        "the card says so: {v}"
    );
    let hid = v["handover"]["id"].as_str().unwrap().to_string();
    let (st, v) = approve(&req_id(&v));
    assert_eq!((st, v["status"].as_str()), (200, Some("approved")), "{v}");
    let pass = k2_core::skin::resolve_skin_token(&s.raw).expect("the live session stays");
    assert_eq!(pass.room_policy.keys().cloned().collect::<Vec<_>>(), vec![f.b.clone()]);
    assert!(!k2_core::skin::verify_principal_password("bea-t31", "a-knows-this-1"), "A's known password no longer signs in");
    assert_eq!(tie_of(&pid).as_deref(), Some(f.b.as_str()), "re-tied to B");
    assert_eq!(k2_core::skin::creators::workspace_of(&pid).unwrap().as_deref(), Some(f.b.as_str()), "stamped B");
    let j = sh::journal(&hid).unwrap();
    assert_eq!(j.len(), 1, "{j:?}");
    let tie_before: serde_json::Value = serde_json::from_str(j[0].tie_before.as_deref().expect("the journal keeps the tie row")).expect("the tie row is JSON");
    assert_eq!(tie_before["workspaceId"], json!(f.a), "the whole tie row, as it was: {tie_before}");
    assert_eq!(tie_before["reasons"], json!(["password"]), "{tie_before}");
    assert_eq!(j[0].stamp_before.as_deref(), Some(f.a.as_str()));
    assert!(j[0].password_hash.as_deref().is_some_and(|h| !h.is_empty()), "the journal keeps the hash for undo");
    // B manages it now: a new password from B (or the owner) signs in.
    k2_core::skin::set_principal_password("bea-t31", Some("new-from-b-2")).unwrap();
    assert!(k2_core::skin::verify_principal_password("bea-t31", "new-from-b-2"));
}

/// T3.2: a shared guest stays A's; the owner may take it over instead.
#[test]
fn a_shared_guest_stays_a_s_unless_the_owner_resets_it() {
    let f = fx("t32");
    let pid = guest("sam-t32", &f.svc, &[&f.a]);
    k2_core::skin::creators::stamp(&pid, &f.a).unwrap();
    k2_core::skin::creators::tie(&pid, &f.a, "agent:test", k2_core::skin::ties::REASON_CREATED).unwrap();
    grant_more(&pid, &f.other_svc, &[&f.a]);
    k2_core::skin::set_principal_password("sam-t32", Some("shared-pw-1")).unwrap();
    let (st, v) = ask(&f);
    assert_eq!(st, 202, "a shared guest doesn't block: {v}");
    assert!(
        v["plan"]["lines"].as_array().unwrap().iter().any(|l| l.as_str().unwrap().contains("sam-t32") && l.as_str().unwrap().contains("stay")),
        "the card lists them: {v}"
    );
    let (st, _) = approve(&req_id(&v));
    assert_eq!(st, 200);
    assert_eq!(tie_of(&pid).as_deref(), Some(f.a.as_str()), "the shared guest stays A's");
    assert!(k2_core::skin::verify_principal_password("sam-t32", "shared-pw-1"), "nothing of theirs changed");
    // The 0.46.4 ceiling: a guest tied to A signs in only to A's Apps, so
    // it loses this one (the card said so) and keeps A's other App.
    assert!(k2_core::skin::create_app_session_token("sam-t32", &f.svc).is_err(), "outside its tie's ceiling now");
    assert!(k2_core::skin::create_app_session_token("sam-t32", &f.other_svc).is_ok(), "A's other App stays");
}

/// T3.2 (b): with "take them over", the tie goes, the password A knew is
/// retired, and the guest keeps the App.
#[test]
fn the_owner_may_take_a_shared_guest_over() {
    let g = fx("t32b");
    let pid = guest("tia-t32", &g.svc, &[&g.a]);
    k2_core::skin::creators::stamp(&pid, &g.a).unwrap();
    k2_core::skin::creators::tie(&pid, &g.a, "agent:test", k2_core::skin::ties::REASON_CREATED).unwrap();
    grant_more(&pid, &g.other_svc, &[&g.a]);
    k2_core::skin::set_principal_password("tia-t32", Some("shared-pw-2")).unwrap();
    let before = k2_core::skin::create_app_session_token("tia-t32", &g.other_svc).unwrap();
    let (st, v) = call("/cli/app/handover", &Who::Owner, json!({ "app": g.app, "to": g.b, "resetShared": true }));
    assert_eq!(st, 202, "{v}");
    let (st, _) = approve(&req_id(&v));
    assert_eq!(st, 200);
    assert_eq!(tie_of(&pid), None, "the owner took them over");
    assert!(!k2_core::skin::verify_principal_password("tia-t32", "shared-pw-2"), "A's known password is retired");
    assert!(k2_core::skin::resolve_skin_token(&before.raw).is_none(), "an owner take-over ends their sessions (A could hold one)");
    let s = k2_core::skin::create_app_session_token("tia-t32", &g.svc).unwrap();
    assert_eq!(s.meta.rooms, vec![g.b.clone()], "they keep the App, now with B");
}

/// T3.3: A's own agent login loses this App.
#[test]
fn a_s_agent_login_loses_the_app() {
    let f = fx("t33");
    guest("bot-t33", &f.svc, &[&f.a]);
    k2_core::skin::set_principal_agent("bot-t33", Some(&handle_of(&f.a))).unwrap();
    let s = k2_core::skin::create_app_session_token("bot-t33", &f.svc).unwrap();
    let (_, v) = ask(&f);
    assert!(v["plan"]["lines"].as_array().unwrap().iter().any(|l| l.as_str().unwrap().contains("bot-t33")), "{v}");
    let (st, _) = approve(&req_id(&v));
    assert_eq!(st, 200);
    assert!(k2_core::skin::resolve_skin_token(&s.raw).is_none(), "its session on the App ended");
    assert!(k2_core::skin::create_app_session_token("bot-t33", &f.svc).is_err(), "its grant is off");
}

/// The gap between the two databases: a tie to A or B counts as inside
/// the App while its handover runs, and only then.
#[test]
fn ties_to_either_side_are_inside_only_during_the_handover() {
    assert!(!sh::tie_inside("ws-a", "svc-gap", "ws-b"));
    {
        let _g = sh::begin_handing_over("svc-gap", "ws-a", "ws-b");
        assert!(sh::tie_inside("ws-a", "svc-gap", "ws-b"));
        assert!(sh::tie_inside("ws-b", "svc-gap", "ws-a"));
        assert!(!sh::tie_inside("ws-c", "svc-gap", "ws-b"), "a third workspace never");
        assert!(!sh::tie_inside("ws-a", "svc-other", "ws-b"), "another App never");
    }
    assert!(!sh::tie_inside("ws-a", "svc-gap", "ws-b"), "over when the guard drops");
}

/// T3.4 against the 0.46.4 ceiling itself (`skin::ties::clamp`, what every
/// sign-in and live session goes through): after the daemon-DB commit the
/// App's workspace is B while the guest is still tied to A. Without the
/// handover marker that is a refusal (the test bites); with it, the
/// guest keeps its room until the skin transaction re-ties it.
#[test]
fn the_ceiling_lets_a_tied_guest_through_the_commit_gap_only() {
    use k2_core::skin::ties;
    let tie = ties::Tie {
        principal_id: "p-gap".into(),
        workspace_id: "ws-a".into(),
        host_ok: false,
        reasons: vec![ties::REASON_CREATED.into()],
        set_by: "agent:a".into(),
        set_at: 1,
    };
    let mut policy = k2_core::skin::RoomPolicy::new();
    policy.insert("ws-a".into(), vec!["thread:read".into()]);
    assert_eq!(ties::clamp(Some(&tie), "svc-gap2", Some("ws-b"), policy.clone()), Err(ties::Denied), "no marker: refused in the gap");
    {
        let _g = sh::begin_handing_over("svc-gap2", "ws-a", "ws-b");
        assert_eq!(ties::clamp(Some(&tie), "svc-gap2", Some("ws-b"), policy.clone()), Ok(policy.clone()), "inside while it hands over");
        assert_eq!(ties::clamp(Some(&tie), "svc-else", Some("ws-b"), policy.clone()), Err(ties::Denied), "only this App");
    }
    assert_eq!(ties::clamp(Some(&tie), "svc-gap2", Some("ws-b"), policy), Err(ties::Denied), "over when the guard drops");
}

// ── S4: secrets, the database, the re-key ────────────────────────────

fn meta_of(sel: &Selector) -> Option<k2_vault::SecretMeta> {
    crate::vault::ensure_test_vault().meta(sel, &Actor::Owner).ok()
}

fn env_of(g: &crate::vault::publish_env::Granted, key: &str) -> Option<String> {
    g.env.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
}

/// T4.1: an A-scope secret granted only to the App moves to B (same id);
/// the App still gets it.
#[test]
fn a_secret_only_the_app_uses_moves_to_b() {
    let f = fx("t41");
    let sel = put_ws_secret(&f.a, "MAILGUN_T41", "mg-value-t41");
    grant_to_service(&sel, &f.svc, "MAILGUN");
    let id = meta_of(&sel).unwrap().id;
    let (_, v) = ask(&f);
    let s = v["plan"]["secrets"].as_array().unwrap();
    assert_eq!((s.len(), s[0]["case"].as_str()), (1, Some("move")), "{v}");
    let (st, v) = approve(&req_id(&v));
    assert_eq!((st, v["status"].as_str()), (200, Some("approved")), "{v}");
    let m = meta_of(&Selector::Id(id.clone())).expect("same secret");
    assert_eq!(m.scope, Scope::workspace(&f.b), "re-scoped to B");
    assert!(meta_of(&sel).is_none(), "A no longer has it");
    let g = crate::vault::publish_env::granted(&f.svc, &f.b, std::path::Path::new("/nonexistent-hov-cwd"));
    assert_eq!(env_of(&g, "MAILGUN").as_deref(), Some("mg-value-t41"), "the App's next start still gets it");
}

/// T4.2: the Seed's `k2v://ws/A/X` resolves through the alias for this
/// service only.
#[test]
fn the_seed_reference_resolves_for_this_service_only() {
    let f = fx("t42");
    put_ws_secret(&f.a, "API_TOKEN_T42", "tok-t42");
    let seed = live_seed_with_env(&f.app, &[("API_TOKEN", format!("k2v://ws/{}/API_TOKEN_T42", handle_of(&f.a)))]);
    let cwd = k2_core::deploy::seed::seed_path(&seed);
    let (_, v) = ask(&f);
    let (st, v) = approve(&req_id(&v));
    assert_eq!(st, 200, "{v}");
    let g = crate::vault::publish_env::granted(&f.svc, &f.b, &cwd);
    assert_eq!(env_of(&g, "API_TOKEN").as_deref(), Some("tok-t42"), "the alias resolves it in B: {:?}", g.refused);
    // Another service of B reading the same .env.k2 is refused, as before.
    let g2 = crate::vault::publish_env::granted("svc-other-of-b", &f.b, &cwd);
    assert_eq!(env_of(&g2, "API_TOKEN"), None);
    assert!(g2.refused.iter().any(|r| r.contains("not_this_workspace")), "{:?}", g2.refused);
}

/// T4.3: a secret A uses elsewhere is copied; A keeps the original; the
/// card says to rotate it.
#[test]
fn a_secret_a_shares_is_copied_and_flagged() {
    let f = fx("t43");
    let sel = put_ws_secret(&f.a, "SHARED_T43", "shared-t43");
    grant_to_service(&sel, &f.svc, "SHARED");
    grant_to_service(&sel, &f.other_svc, "SHARED");
    let orig = meta_of(&sel).unwrap().id;
    let (_, v) = ask(&f);
    assert_eq!(v["plan"]["secrets"][0]["case"], "copy", "{v}");
    assert!(v["plan"]["lines"].as_array().unwrap().iter().any(|l| l.as_str().unwrap().contains("rotate them: SHARED_T43")), "{v}");
    let (st, v) = approve(&req_id(&v));
    assert_eq!(st, 200, "{v}");
    assert_eq!(meta_of(&sel).unwrap().id, orig, "A keeps its own");
    let copy = meta_of(&Selector::user(Scope::workspace(&f.b), "SHARED_T43")).expect("a copy in B");
    assert_ne!(copy.id, orig);
    let vault = crate::vault::ensure_test_vault();
    let mine: Vec<String> = vault.grants_to(&Consumer::published_service(f.svc.clone())).unwrap().into_iter().map(|(g, _)| g.secret_id).collect();
    assert_eq!(mine, vec![copy.id.clone()], "the App's grant moved to the copy");
    let theirs: Vec<String> = vault.grants_to(&Consumer::published_service(f.other_svc.clone())).unwrap().into_iter().map(|(g, _)| g.secret_id).collect();
    assert_eq!(theirs, vec![orig], "A's other App keeps the original");
}

/// Q8: a name B already has → the App's value as NAME__<app>.
#[test]
fn a_name_b_already_has_is_kept_under_the_app_s_name() {
    let f = fx("q8");
    let sel = put_ws_secret(&f.a, "STRIPE_Q8", "a-value");
    grant_to_service(&sel, &f.svc, "STRIPE");
    put_ws_secret(&f.b, "STRIPE_Q8", "b-value");
    let (_, v) = ask(&f);
    assert_eq!(v["plan"]["secrets"][0]["collision"], true, "{v}");
    let (st, _) = approve(&req_id(&v));
    assert_eq!(st, 200);
    let renamed = meta_of(&Selector::user(Scope::workspace(&f.b), format!("STRIPE_Q8__{}", f.app))).expect("renamed in B");
    assert_eq!(renamed.scope, Scope::workspace(&f.b));
    let g = crate::vault::publish_env::granted(&f.svc, &f.b, std::path::Path::new("/nonexistent-hov-cwd"));
    assert_eq!(env_of(&g, "STRIPE").as_deref(), Some("a-value"), "the App keeps its own value");
}

/// T4.4: the App's sole database moves to B; its role passwords change;
/// the App's DSN secret holds the new password.
#[test]
fn a_sole_database_moves_and_its_passwords_change() {
    let f = fx("t44");
    let vault = crate::vault::ensure_test_vault();
    let store = crate::vault::refstore::VaultRefStore::new(vault.clone(), "postgres", "db");
    let (aref, mref) = (store.store_new("t44", "agent", "old-agent-pw").unwrap(), store.store_new("t44", "migrator", "old-mig-pw").unwrap());
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute(
            "INSERT INTO sql_databases (id, project_id, name, status, created_at, agent_secret_ref, migrator_secret_ref) VALUES ('db-t44', ?1, 'ws_t44', 'active', 1, ?2, ?3)",
            params![f.a, aref, mref],
        )
        .unwrap();
    }
    put_ws_secret(&f.a, "DATABASE_URL", "postgres://ws_t44_agent:old-agent-pw@127.0.0.1:5432/ws_t44");
    live_seed_with_env(&f.app, &[("DATABASE_URL", format!("k2v://ws/{}/DATABASE_URL", handle_of(&f.a)))]);
    let (_, v) = ask(&f);
    assert_eq!(v["plan"]["database"]["case"], "sole", "{v}");
    assert_eq!(v["plan"]["rekey"]["database"], true, "{v}");
    let fake: &'static crate::sql::sysops::FakeSystemOps = Box::leak(Box::new(crate::sql::sysops::FakeSystemOps::baked()));
    fake.pg.lock().unwrap().roles.extend(["ws_t44_agent".to_string(), "ws_t44_migrator".to_string()]);
    let (st, v) = crate::sql::routes::with_test_ops(fake, || approve(&req_id(&v)));
    assert_eq!((st, v["status"].as_str()), (200, Some("approved")), "{v}");
    let owner: String = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.query_row("SELECT project_id FROM sql_databases WHERE id = 'db-t44'", [], |r| r.get(0)).unwrap()
    };
    assert_eq!(owner, f.b, "the database is B's");
    let altered: Vec<String> = fake.pg.lock().unwrap().role_sql.iter().filter(|s| s.to_ascii_uppercase().starts_with("ALTER ROLE")).cloned().collect();
    assert_eq!(altered.len(), 2, "both roles get new passwords: {altered:?}");
    let new_pw = store.resolve(&aref).unwrap().unwrap();
    assert_ne!(new_pw, "old-agent-pw", "the agent password changed");
    let g = crate::vault::publish_env::granted(&f.svc, &f.b, &k2_core::deploy::seed::seed_path(&app_row(&f.app).live_seed.unwrap()));
    let dsn = env_of(&g, "DATABASE_URL").expect("the App's DATABASE_URL resolves in B");
    assert!(dsn.contains(&new_pw) && !dsn.contains("old-agent-pw"), "the DSN carries the new password: {dsn}");
}

/// T4.5: a database A's other App uses too is refused unless the owner
/// shares it; sharing gives B a grant and the card says what A keeps.
#[test]
fn a_shared_database_needs_the_owner_s_share() {
    let f = fx("t45");
    add_db(&f.a, "ws_t45");
    put_ws_secret(&f.a, "DATABASE_URL", "postgres://u:p@127.0.0.1:5432/ws_t45");
    let r = format!("k2v://ws/{}/DATABASE_URL", handle_of(&f.a));
    live_seed_with_env(&f.app, &[("DATABASE_URL", r.clone())]);
    live_seed_with_env(&f.other, &[("DATABASE_URL", r)]);
    let (st, v) = ask(&f);
    assert_eq!((st, code(&v)), (409, "handover_db_shared"), "{v}");
    let (st, v) = call("/cli/app/handover", &Who::Owner, json!({ "app": f.app, "to": f.b, "db": "share" }));
    assert_eq!(st, 202, "{v}");
    assert!(v["plan"]["lines"].as_array().unwrap().iter().any(|l| l.as_str().unwrap().contains("can still read")), "{v}");
    let (st, _) = approve(&req_id(&v));
    assert_eq!(st, 200);
    let (level, manage): (String, i64) = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.query_row("SELECT level, can_manage FROM sql_grants WHERE database_id = 'db-ws_t45' AND project_id = ?1", params![f.b], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
    };
    assert_eq!((level.as_str(), manage), ("write", 1));
}

/// T4.6 (keys): a vault-delivered App key gets a successor; the old one
/// stops working (the App isn't running: at once).
#[test]
fn the_app_s_vault_key_is_rekeyed() {
    let f = fx("t46");
    let grant = k2_core::app_grace::keys::AppKeyGrant { scopes: vec!["users".into()], rooms: vec![f.a.clone()], roles: vec![], vault_secret: Some("K2_APP_KEY.t46".into()), minted_by_agent: None };
    let (old, raw) = k2_core::api_keys::create_app_key("jobs key", &f.svc, &grant.to_json()).unwrap();
    assert!(k2_core::api_keys::resolve_app_key(&raw).is_some());
    let (_, v) = ask(&f);
    assert_eq!(v["plan"]["rekey"]["mode"], "next_start", "{v}");
    let (st, v) = approve(&req_id(&v));
    assert_eq!(st, 200, "{v}");
    assert!(k2_core::api_keys::resolve_app_key(&raw).is_none(), "the key A could read is revoked");
    let live: Vec<_> = k2_core::api_keys::list_app_keys(Some(&f.svc)).unwrap().into_iter().filter(|k| k.revoked_at.is_none()).collect();
    assert_eq!(live.len(), 1, "one successor");
    assert_ne!(live[0].id, old);
    let g = k2_core::app_grace::keys::AppKeyGrant::from_stored(live[0].grant_json.as_deref());
    assert_eq!(g.rooms, vec![f.b.clone()], "its room ceiling names B");
    let h = handover_row(v["result"]["handover"]["id"].as_str().unwrap());
    assert_eq!(h.rekey_state.as_deref(), Some("done"));
}

// ── 0.46.5 review (review-app-handoff-0465-v1): 0.46.4's App grants ──

/// R1: a key A's agent minted itself (0.46.4 `mintedByAgent`: its users
/// and rooms scopes follow that agent's "Let this agent manage its Apps'
/// users") answers to B's switch after the handover, never A's; undo puts
/// A back.
#[test]
fn an_agent_minted_key_follows_b_s_users_switch() {
    let f = fx("r1");
    let grant = k2_core::app_grace::keys::AppKeyGrant {
        scopes: vec!["users".into()],
        rooms: vec![f.a.clone()],
        roles: vec![],
        vault_secret: Some("K2_APP_KEY.r1".into()),
        minted_by_agent: Some(f.a.clone()),
    };
    let (_old, raw) = k2_core::api_keys::create_app_key("agent key", &f.svc, &grant.to_json()).unwrap();
    let (_, v) = ask(&f);
    let (st, v) = approve(&req_id(&v));
    assert_eq!(st, 200, "{v}");
    assert!(k2_core::api_keys::resolve_app_key(&raw).is_none(), "the key A's runtime could read is revoked");
    let live: Vec<_> = k2_core::api_keys::list_app_keys(Some(&f.svc)).unwrap().into_iter().filter(|k| k.revoked_at.is_none()).collect();
    assert_eq!(live.len(), 1, "one successor");
    let g = k2_core::app_grace::keys::AppKeyGrant::from_stored(live[0].grant_json.as_deref());
    assert_eq!(g.minted_by_agent.as_deref(), Some(f.b.as_str()), "B's switch governs it now");
    assert_eq!(g.rooms, vec![f.b.clone()]);
    let (st, v) = call("/cli/app/handover/undo", &Who::Owner, json!({ "app": f.app }));
    assert_eq!(st, 202, "{v}");
    let (st, v) = approve(&req_id(&v));
    assert_eq!(st, 200, "{v}");
    let live: Vec<_> = k2_core::api_keys::list_app_keys(Some(&f.svc)).unwrap().into_iter().filter(|k| k.revoked_at.is_none()).collect();
    assert_eq!(live.len(), 1, "one key again after the undo's re-key");
    let g = k2_core::app_grace::keys::AppKeyGrant::from_stored(live[0].grant_json.as_deref());
    assert_eq!(g.minted_by_agent.as_deref(), Some(f.a.as_str()), "undo: A's switch again");
    assert_eq!(g.rooms, vec![f.a.clone()]);
}

/// R2: a test guest A's agent made (0.46.4) follows its re-tie: B's agent
/// manages it, A's doesn't.
#[test]
fn an_agent_made_test_guest_follows_the_app() {
    let f = fx("r2");
    let pid = guest("tg-r2", &f.svc, &[&f.a]);
    k2_core::skin::creators::stamp(&pid, &f.a).unwrap();
    k2_core::skin::creators::tie(&pid, &f.a, "agent:test", k2_core::skin::ties::REASON_CREATED).unwrap();
    k2_core::skin::test_guests::record(&pid, &f.svc, &f.a, &format!("agent:{}", handle_of(&f.a)), k2_core::deploy::now() + 3600).unwrap();
    let (_, v) = ask(&f);
    let (st, v) = approve(&req_id(&v));
    assert_eq!(st, 200, "{v}");
    let t = k2_core::skin::test_guests::get(&pid).unwrap().expect("still a test guest");
    assert_eq!(t.workspace_id, f.b, "the row follows the maintaining agent");
    let a_actor = format!("agent:{}", handle_of(&f.a));
    let b_actor = format!("agent:{}", handle_of(&f.b));
    assert!(crate::skin_routes::agent_test_guest_refused(&a_actor, "tg-r2").is_some(), "A no longer manages it");
    assert!(crate::skin_routes::agent_test_guest_refused(&b_actor, "tg-r2").is_none(), "B does");
}

/// R3: the card says what B's 0.46.4 App settings mean for the App: the
/// users switch a handover never turns on, the cores B lacks, and the caps
/// that reach the maintaining agent itself (hashed: they reach B).
#[test]
fn the_card_names_b_s_users_switch_cores_and_agent_caps() {
    let f = fx("r3");
    guest("gus-r3", &f.svc, &[&f.a]);
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        super::grants::record(&conn, &f.app, &["thread:read".to_string(), "files:agentconfig".to_string()]).unwrap();
    }
    let (st, v) = plan(&f, &Who::Owner);
    assert_eq!(st, 200, "{v}");
    let lines: Vec<String> = v["lines"].as_array().unwrap().iter().map(|l| l.as_str().unwrap().to_string()).collect();
    assert!(lines.iter().any(|l| l.contains("Let this agent manage its Apps' users") && l.contains("never")), "{lines:?}");
    assert!(lines.iter().any(|l| l.contains("files:agentconfig") && l.contains(&handle_of(&f.b))), "{lines:?}");
    assert_eq!(v["toSettings"]["appUsers"], json!(false));
    assert_eq!(v["toSettings"]["agentCaps"], json!(["files:agentconfig"]));
    assert!(v["toSettings"]["coresMissing"].as_array().is_some_and(|c| !c.is_empty()), "{v}");
    // The handover never raises B's users switch.
    let (_, v) = ask(&f);
    let (st, _) = approve(&req_id(&v));
    assert_eq!(st, 200);
    assert!(!k2_core::workspace::settings::agents_can_manage_app_users(&f.b), "still off for B");
}

/// R4: the App's audit keeps the whole trail: offered, accepted, the card,
/// the commit (who approved), in order.
#[test]
fn the_app_s_audit_keeps_the_whole_trail() {
    let f = fx("r4");
    switch1(&f.a, true);
    let (st, v) = call("/cli/app/handover", &agent(&f.a), json!({ "app": f.app, "to": f.b }));
    assert_eq!((st, v["handover"]["state"].as_str()), (202, Some("offered")), "{v}");
    let (st, v) = call("/cli/app/handover/accept", &agent(&f.b), json!({ "app": f.app }));
    assert_eq!(st, 202, "{v}");
    let (st, _) = approve(&req_id(&v));
    assert_eq!(st, 200);
    let events: Vec<(String, String)> = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        let mut rows = k2_core::deploy::audit::list_for(&conn, k2_core::deploy::audit::SUBJECT_APP, &f.app, 100).unwrap();
        rows.sort_by_key(|r| r.seq);
        rows.into_iter().filter(|r| r.event.starts_with("handover")).map(|r| (r.event, r.actor)).collect()
    };
    let names: Vec<&str> = events.iter().map(|(e, _)| e.as_str()).collect();
    assert_eq!(names, vec!["handover_offered", "handover_accepted", "handover"], "{events:?}");
    assert_eq!(events[0].1, format!("agent:{}", handle_of(&f.a)), "who offered");
    assert_eq!(events[1].1, format!("agent:{}", handle_of(&f.b)), "who accepted");
    assert_eq!(events[2].1, "owner", "who approved");
}

// ── S5: the undo window, recovery ───────────────────────────────────

/// T5.1: undo puts rooms, ties, labels and secrets back; a retired
/// password comes back only for a guest who didn't set a new one.
#[test]
fn undo_puts_everything_back_with_compare_and_set_passwords() {
    let f = fx("t51");
    add_labels(&f, "agent-t51", "dannon-t51");
    let kept = guest("kay-t51", &f.svc, &[&f.a]);
    k2_core::skin::creators::stamp(&kept, &f.a).unwrap();
    k2_core::skin::creators::tie(&kept, &f.a, "agent:test", k2_core::skin::ties::REASON_CREATED).unwrap();
    k2_core::skin::set_principal_password("kay-t51", Some("a-set-kay-1")).unwrap();
    let changed = guest("cal-t51", &f.svc, &[&f.a]);
    k2_core::skin::creators::stamp(&changed, &f.a).unwrap();
    k2_core::skin::creators::tie(&changed, &f.a, "agent:test", k2_core::skin::ties::REASON_CREATED).unwrap();
    k2_core::skin::set_principal_password("cal-t51", Some("a-set-cal-1")).unwrap();
    let sel = put_ws_secret(&f.a, "TOKEN_T51", "tok-t51");
    grant_to_service(&sel, &f.svc, "TOKEN");
    let id = meta_of(&sel).unwrap().id;
    let (_, v) = ask(&f);
    let (st, v) = approve(&req_id(&v));
    assert_eq!(st, 200, "{v}");
    let hid = v["result"]["handover"]["id"].as_str().unwrap().to_string();
    // Meanwhile cal sets a new password (B's reset link).
    k2_core::skin::set_principal_password("cal-t51", Some("cal-own-new-2")).unwrap();
    // An agent can't undo; the owner files the undo card.
    let (st, v) = call("/cli/app/handover/undo", &agent(&f.b), json!({ "app": f.app }));
    assert_eq!((st, code(&v)), (403, "owner_only"), "{v}");
    let (st, v) = call("/cli/app/handover/undo", &Who::Owner, json!({ "app": f.app }));
    assert_eq!((st, v["status"].as_str()), (202, Some("held")), "{v}");
    let (st, v) = approve(&req_id(&v));
    assert_eq!((st, v["status"].as_str()), (200, Some("approved")), "{v}");
    assert_eq!(handover_row(&hid).state, "undone");
    assert_eq!(app_row(&f.app).owner_workspace, f.a);
    for l in [f.app.as_str(), "agent-t51", "dannon-t51"] {
        assert_eq!(holder(l).as_deref(), Some(f.a.as_str()), "{l} is A's again");
    }
    assert_eq!(super::labels::load_cfg(&f.app).unwrap().labels[0].service_id, "svc-legacy", "the taken label points back");
    assert_eq!(k2_core::skin::creators::agent_tie(&kept).unwrap().as_deref(), Some(f.a.as_str()), "re-tied to A");
    assert!(k2_core::skin::verify_principal_password("kay-t51", "a-set-kay-1"), "kay's hash came back (unchanged since)");
    assert!(k2_core::skin::verify_principal_password("cal-t51", "cal-own-new-2"), "cal keeps the password cal set (CAS)");
    assert!(!k2_core::skin::verify_principal_password("cal-t51", "a-set-cal-1"));
    let s = k2_core::skin::create_app_session_token("kay-t51", &f.svc).unwrap();
    assert_eq!(s.meta.rooms, vec![f.a.clone()], "rooms back to A");
    assert_eq!(meta_of(&Selector::Id(id)).unwrap().scope, Scope::workspace(&f.a), "the secret is A's again");
    let aliases = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        k2_core::deploy::handover::aliases_of(&conn, &f.svc).unwrap()
    };
    assert!(aliases.is_empty(), "{aliases:?}");
    // No second undo.
    let (st, v) = call("/cli/app/handover/undo", &Who::Owner, json!({ "app": f.app }));
    assert_eq!((st, code(&v)), (409, "handover_not_in_window"), "{v}");
}

/// T5.2: the daemon dies right after the commit point: boot finishes the
/// rest before anything else.
#[test]
fn boot_finishes_a_handover_cut_off_after_its_commit() {
    let f = fx("t52");
    guest("gil-t52", &f.svc, &[&f.a]);
    let (_, v) = ask(&f);
    let hid = v["handover"]["id"].as_str().unwrap().to_string();
    super::handover::CRASH_AFTER_COMMIT.with(|c| c.set(true));
    let (st, _) = approve(&req_id(&v));
    super::handover::CRASH_AFTER_COMMIT.with(|c| c.set(false));
    assert_eq!(st, 200);
    let h = handover_row(&hid);
    assert_eq!((h.state.as_str(), h.steps_done), ("applied", 2), "committed, nothing after: {h:?}");
    assert_eq!(app_row(&f.app).owner_workspace, f.b, "the commit point moved the App");
    let s = k2_core::skin::create_app_session_token("gil-t52", &f.svc).unwrap();
    assert_eq!(s.meta.rooms, vec![f.a.clone()], "step 3 hasn't run");
    let lines = super::handover::boot_recover();
    assert!(lines.iter().any(|l| l.contains(&hid)), "{lines:?}");
    assert_eq!(handover_row(&hid).steps_done, 7);
    let s = k2_core::skin::create_app_session_token("gil-t52", &f.svc).unwrap();
    assert_eq!(s.meta.rooms, vec![f.b.clone()], "boot finished it forward");
    assert!(super::handover::boot_recover().iter().all(|l| !l.contains(&hid)), "once");
}

/// T5.3: the window ends: `final`, and the journal's hashes are gone.
#[test]
fn the_window_end_forgets_the_credential_journal() {
    let f = fx("t53");
    let pid = guest("hal-t53", &f.svc, &[&f.a]);
    k2_core::skin::creators::stamp(&pid, &f.a).unwrap();
    k2_core::skin::creators::tie(&pid, &f.a, "agent:test", k2_core::skin::ties::REASON_CREATED).unwrap();
    k2_core::skin::set_principal_password("hal-t53", Some("a-set-hal-1")).unwrap();
    let (_, v) = ask(&f);
    let (_, v) = approve(&req_id(&v));
    let hid = v["result"]["handover"]["id"].as_str().unwrap().to_string();
    assert_eq!(sh::journal(&hid).unwrap().len(), 1);
    super::handover::sweep();
    assert_eq!(handover_row(&hid).state, "applied", "still in its window");
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        k2_core::deploy::handover::set_window(&conn, &hid, Some(k2_core::deploy::now() - 1), k2_core::deploy::now()).unwrap();
    }
    super::handover::sweep();
    assert_eq!(handover_row(&hid).state, "final");
    assert_eq!(sh::journal(&hid).unwrap().len(), 0, "no credential material left");
    let (st, v) = call("/cli/app/handover/undo", &Who::Owner, json!({ "app": f.app }));
    assert_eq!((st, code(&v)), (409, "handover_not_in_window"), "{v}");
}

/// Finish: the owner at any time; B only after it deployed the App.
#[test]
fn the_window_can_be_finished_early() {
    let f = fx("fin");
    let (_, v) = ask(&f);
    let (_, v) = approve(&req_id(&v));
    let hid = v["result"]["handover"]["id"].as_str().unwrap().to_string();
    let (st, v) = call("/cli/app/handover/finish", &agent(&f.b), json!({ "app": f.app }));
    assert_eq!((st, code(&v)), (409, "handover_not_in_window"), "B hasn't deployed yet: {v}");
    let (st, v) = call("/cli/app/handover/finish", &agent(&f.a), json!({ "app": f.app }));
    assert_eq!(st, 404, "A no longer sees the App: {v}");
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute("UPDATE apps SET live_deployment = 'dep-b-fin' WHERE app_id = ?1", params![f.app]).unwrap();
    }
    let (st, v) = call("/cli/app/handover/finish", &agent(&f.b), json!({ "app": f.app }));
    assert_eq!((st, v["handover"]["state"].as_str()), (200, Some("final")), "{v}");
    assert_eq!(handover_row(&hid).state, "final");
}

// ── S6: hire --take-app, the context layer ───────────────────────────

fn hire(who: &Who, body: J) -> (u16, J) {
    let r = crate::hire_routes::handle_cli("/cli/agents/hire", who.clone(), true, body.to_string().as_bytes(), &HashMap::new(), None);
    let n: u16 = r.status.split_whitespace().next().unwrap().parse().unwrap();
    (n, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("hire: not JSON ({e}): {}", r.body)))
}

fn handovers_of(app: &str) -> usize {
    let h = k2_core::db::shared();
    let conn = h.lock();
    k2_core::deploy::handover::list(&conn, None, Some(app), 50).unwrap().len()
}

/// T6.1: hire --take-app from A's passport: a new agent, and the App's
/// handover to it auto-accepted (the card waits for the owner). A failed
/// hire files nothing.
#[test]
fn hire_take_app_offers_the_app_to_the_new_agent_only_after_the_hire() {
    let f = fx("t61");
    switch1(&f.a, true);
    let bad = f._home.path().join("not-a-dir-t61");
    std::fs::write(&bad, b"a file, not a folder").unwrap();
    let (st, v) = hire(&agent(&f.a), json!({ "path": bad.to_string_lossy(), "takeApp": f.app }));
    assert!(st >= 400, "the hire fails: {st} {v}");
    assert_eq!(handovers_of(&f.app), 0, "a failed hire files nothing");
    let dir = f._home.path().join("hired-t61");
    let (st, v) = hire(&agent(&f.a), json!({ "path": dir.to_string_lossy(), "takeApp": f.app, "brief": "# notes for you" }));
    assert_eq!(st, 200, "{v}");
    let new_ws = v["id"].as_str().unwrap().to_string();
    assert_eq!(v["handover"]["state"], "held", "auto-accepted for the agent hired for it: {v}");
    let req = v["handover"]["request"].as_str().expect("the owner's card").to_string();
    let (st, a) = approve(&req);
    assert_eq!(st, 200, "{a}");
    assert_eq!(app_row(&f.app).owner_workspace, new_ws);
}

/// T6.2: "Apps you maintain" lists the App for B and not for A after.
#[test]
fn the_context_layer_follows_the_app() {
    let f = fx("t62");
    let (_, v) = ask(&f);
    let (st, _) = approve(&req_id(&v));
    assert_eq!(st, 200);
    let b = k2_core::workspace::context_layers::render_apps_maintained_body(&ws_path(&f.b));
    let a = k2_core::workspace::context_layers::render_apps_maintained_body(&ws_path(&f.a));
    assert!(b.contains(&format!("| {} |", f.app)), "B lists it:\n{b}");
    assert!(b.contains("Recent handovers") && b.contains(&format!("k2 app handover brief {}", f.app)), "with the handover note:\n{b}");
    assert!(!a.contains(&format!("| {} |", f.app)), "A doesn't:\n{a}");
    assert!(a.contains(&format!("| {} |", f.other)), "A still lists its other App:\n{a}");
}

// ── S7: the source copy (opt-in) ─────────────────────────────────────

/// Copy the source: into B's folder, never `.k2/` or `.env*`;
/// `folder_path` points at the copy.
#[test]
fn copy_source_lands_in_b_without_env_or_k2_files() {
    let f = fx("s7");
    let src = f._home.path().join("a-src-s7");
    std::fs::create_dir_all(src.join("lib")).unwrap();
    std::fs::create_dir_all(src.join(".k2")).unwrap();
    std::fs::write(src.join("lib/app.js"), b"console.log('hi')").unwrap();
    std::fs::write(src.join(".env.k2"), b"TOKEN=k2v://ws/a/TOKEN").unwrap();
    std::fs::write(src.join(".k2/notes.md"), b"private").unwrap();
    let b_dir = f._home.path().join("b-ws-s7");
    std::fs::create_dir_all(&b_dir).unwrap();
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute("UPDATE apps SET folder_path = ?2 WHERE app_id = ?1", params![f.app, src.to_string_lossy()]).unwrap();
        conn.execute("UPDATE projects SET path = ?2 WHERE id = ?1", params![f.b, b_dir.to_string_lossy()]).unwrap();
    }
    let (st, v) = call("/cli/app/handover", &Who::Owner, json!({ "app": f.app, "to": f.b, "copySource": true }));
    assert_eq!(st, 202, "{v}");
    assert!(v["plan"]["lines"].as_array().unwrap().iter().any(|l| l.as_str().unwrap().contains("is copied into")), "{v}");
    let (st, v) = approve(&req_id(&v));
    assert_eq!(st, 200, "{v}");
    let dest = b_dir.join("apps").join(&f.app);
    assert_eq!(std::fs::read(dest.join("lib/app.js")).unwrap(), b"console.log('hi')");
    assert!(!dest.join(".env.k2").exists(), "env files stay behind");
    assert!(!dest.join(".k2").exists(), "K2's folder stays behind");
    assert_eq!(app_row(&f.app).folder_path.as_deref(), Some(dest.to_string_lossy().as_ref()));
}
