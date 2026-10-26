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

pub(super) fn call(path: &str, who: &Who, body: J) -> (u16, J) {
    let r = super::routes::handle(path, who.clone(), body.to_string().as_bytes(), &HashMap::new());
    let n: u16 = r.status.split_whitespace().next().unwrap().parse().unwrap();
    (n, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{path}: not JSON ({e}): {}", r.body)))
}

pub(super) fn get(path: &str, who: &Who, q: &[(&str, &str)]) -> (u16, J) {
    let p: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let r = super::routes::handle(path, who.clone(), b"", &p);
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
    let shared = guest("shared-t01", &f.svc, &[&f.a]);
    k2_core::skin::creators::stamp(&shared, &f.a).unwrap();
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
