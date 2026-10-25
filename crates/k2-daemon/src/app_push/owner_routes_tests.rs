//! `/cli/app/push/*` in process: who may do what, never a token in an
//! answer.

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::skin::{self, app_push as store};
use rusqlite::params;
use serde_json::{json, Value as J};

use super::*;

pub(super) struct Fx {
    pub _db: k2_core::db::TestDbGuard,
    pub _home: crate::test_support::TempHome,
    pub ws: String,
    pub other_ws: String,
    pub app: String,
    pub other_app: String,
    pub dana: String,
}

pub(super) const APNS: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90";

pub(super) fn fx(tag: &str) -> Fx {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    let (ws, other_ws) = (format!("ws-{tag}"), format!("ot-{tag}"));
    let (app, other_app) = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        for r in [&ws, &other_ws] {
            let path = home.path().join(r.as_str());
            std::fs::create_dir_all(&path).expect("project dir");
            conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", params![r, path.to_string_lossy()])
                .expect("project");
        }
        let a = k2_core::published_services::insert(&conn, &ws, "jobs", "node s.js", "/tmp", 4320, "local", "running", "cmd", "").expect("svc");
        let o = k2_core::published_services::insert(&conn, &other_ws, "crm", "node s.js", "/tmp", 4321, "local", "running", "cmd", "").expect("svc");
        (a.id, o.id)
    };
    let dana = skin::add_principal_for_app("dana", None, &app, None, "test").expect("dana").id;
    Fx { _db: db, _home: home, ws, other_ws, app, other_app, dana }
}

pub(super) fn agent(ws: &str) -> Who {
    Who::Agent { project_id: ws.into(), session_id: format!("sess-{ws}") }
}

pub(super) fn login(role: Role) -> Who {
    Who::Login { username: "ada".into(), role }
}

pub(super) fn post(path: &str, who: &Who, body: J) -> (u16, J, String) {
    let r = handle(path, who.clone(), body.to_string().as_bytes(), &HashMap::new());
    let st = r.status.split_whitespace().next().expect("status").parse().expect("number");
    (st, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("json ({e}): {}", r.body)), r.body)
}

pub(super) fn get(path: &str, who: &Who, q: &[(&str, &str)]) -> (u16, J, String) {
    let p: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let r = handle(path, who.clone(), b"", &p);
    let st = r.status.split_whitespace().next().expect("status").parse().expect("number");
    (st, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("json ({e}): {}", r.body)), r.body)
}

pub(super) fn code(v: &J) -> &str {
    v["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {v}"))
}

pub(super) fn register_dana(f: &Fx, device: &str) -> store::PushDevice {
    let s = skin::create_app_session_token("dana", &f.app).expect("session");
    let reg = store::parse_registration(&json!({"deviceId": device, "platform": "apns", "pushToken": APNS})).expect("reg");
    store::register(&f.app, &f.dana, &s.meta.id, &reg).expect("register")
}

#[test]
fn devices_are_admins_and_never_carry_a_token() {
    let f = fx("dev");
    let d = register_dana(&f, "iphone-1");
    for who in [Who::Owner, login(Role::Owner), login(Role::Admin)] {
        let (st, v, raw) = get(DEVICES, &who, &[("app", &f.app)]);
        assert_eq!(st, 200, "{who:?}: {v}");
        assert_eq!(v["count"], 1);
        assert_eq!(v["devices"][0]["id"], d.id.as_str());
        assert_eq!(v["devices"][0]["username"], "dana");
        assert!(!raw.contains(APNS), "never a token: {raw}");
    }
    let (st, v, _) = get(DEVICES, &Who::Owner, &[("app", &f.app), ("guest", "dana")]);
    assert_eq!((st, v["count"].clone()), (200, json!(1)), "{v}");
    let (st, v, _) = get(DEVICES, &Who::Owner, &[("app", &f.app), ("guest", "nobody")]);
    assert_eq!((st, v["count"].clone()), (200, json!(0)), "{v}");
    let (st, v, _) = get(DEVICES, &agent(&f.ws), &[("app", &f.app)]);
    assert_eq!((st, code(&v)), (403, "admin_only"), "the App's agent reads counts, not devices: {v}");
    let (st, v, _) = get(DEVICES, &agent(&f.other_ws), &[("app", &f.app)]);
    assert_eq!(st, 404, "another workspace's agent can't see the App: {v}");
    for who in [Who::Wrong, Who::Invalid] {
        let (st, v, _) = get(DEVICES, &who, &[("app", &f.app)]);
        assert_eq!(st, 403, "{who:?}: {v}");
    }
    let (st, v, _) = get(DEVICES, &Who::Owner, &[("app", "host")]);
    assert_eq!((st, code(&v)), (400, "not_for_host"), "{v}");
}

#[test]
fn removing_a_device_is_admins_and_per_app() {
    let f = fx("rm");
    let d = register_dana(&f, "iphone-1");
    for who in [agent(&f.ws), login(Role::Member)] {
        let (st, v, _) = post(DEVICES_REMOVE, &who, json!({"app": f.app, "id": d.id}));
        assert_eq!(st, 403, "{who:?}: {v}");
    }
    let (st, v, _) = post(DEVICES_REMOVE, &Who::Owner, json!({"app": f.other_app, "id": d.id}));
    assert_eq!((st, code(&v)), (404, "not_found"), "another App's id: {v}");
    let (st, v, _) = post(DEVICES_REMOVE, &login(Role::Admin), json!({"app": f.app, "id": d.id}));
    assert_eq!((st, v.clone()), (200, json!({"ok": true, "removed": true})), "{v}");
    assert!(store::devices_for_app(&f.app, None).expect("list").is_empty());
}

pub(super) fn switches(ws: &str, build: bool, full: bool) {
    let h = k2_core::db::shared();
    let conn = h.lock();
    conn.execute(
        "UPDATE projects SET agents_can_manage_skin = ?2, agents_can_manage_apps_full = ?3 WHERE id = ?1",
        params![ws, build as i64, full as i64],
    )
    .expect("switches");
}

fn apns_body(app: &str) -> J {
    json!({"app": app, "apns": {"p8": super::super::test_keys::EC_P256_P8, "keyId": "ABC123DEFG", "teamId": "TEAM123456", "bundleId": "com.example.app"}})
}

#[test]
fn only_the_owner_in_person_with_a_fresh_confirm_sets_keys() {
    let f = fx("keys");
    let vault = crate::vault::ensure_test_vault();
    switches(&f.ws, true, true);
    // An agent (even at switch 2), an Admin and a shell tab: owner_only.
    for who in [agent(&f.ws), login(Role::Admin), Who::Shell { project_id: f.ws.clone(), session_id: "s".into() }] {
        let (st, v, _) = post(CREDENTIALS, &who, apns_body(&f.app));
        assert_eq!((st, code(&v)), (403, "owner_only"), "{who:?}: {v}");
    }
    // The owner token without a confirm: asked for one, bound to this App.
    let (st, v, raw) = post(CREDENTIALS, &Who::Owner, apns_body(&f.app));
    assert_eq!((st, code(&v)), (403, "vault_confirm_required"), "{v}");
    assert_eq!(v["error"]["serviceId"], f.app.as_str());
    assert!(!raw.contains("PRIVATE KEY"), "{raw}");
    // A bad key is refused before the confirm is spent.
    let tok = vault.confirm.mint(super::super::creds::CONFIRM_ACTION, Some(&f.app), "owner", "local-auth");
    let mut bad = apns_body(&f.app);
    bad["apns"]["keyId"] = json!("short");
    bad["confirmToken"] = json!(tok);
    let (st, v, _) = post(CREDENTIALS, &Who::Owner, bad);
    assert_eq!((st, code(&v)), (400, "usage"), "{v}");
    // A confirm minted for another App doesn't open this one.
    let other = vault.confirm.mint(super::super::creds::CONFIRM_ACTION, Some(&f.other_app), "owner", "local-auth");
    let mut b = apns_body(&f.app);
    b["confirmToken"] = json!(other);
    let (st, v, _) = post(CREDENTIALS, &Who::Owner, b);
    assert_eq!((st, code(&v)), (403, "vault_confirm_required"), "{v}");
    // The unspent confirm from above works once.
    let mut b = apns_body(&f.app);
    b["confirmToken"] = json!(tok);
    let (st, v, raw) = post(CREDENTIALS, &Who::Owner, b.clone());
    assert_eq!(st, 200, "{v}");
    assert!(!raw.contains("PRIVATE KEY") && !raw.contains("MIG"), "never the key: {raw}");
    assert_eq!(v["apns"]["keyId"], "ABC123DEFG");
    assert_eq!(v["apns"]["bundleId"], "com.example.app");
    assert_eq!(v["fcm"], J::Null);
    let (st, v, _) = post(CREDENTIALS, &Who::Owner, b);
    assert_eq!((st, code(&v)), (403, "vault_confirm_required"), "a confirm works once: {v}");
    assert!(super::super::creds::load_apns(&f.app).is_ok());
    // Nothing to set is usage.
    let (st, v, _) = post(CREDENTIALS, &Who::Owner, json!({"app": f.app}));
    assert_eq!((st, code(&v)), (400, "usage"), "{v}");
}

#[test]
fn removing_keys_needs_the_same_confirm_and_status_shows_metadata_only() {
    let f = fx("rmkeys");
    let vault = crate::vault::ensure_test_vault();
    let mut b = apns_body(&f.app);
    b["confirmToken"] = json!(vault.confirm.mint(super::super::creds::CONFIRM_ACTION, Some(&f.app), "owner", "local-auth"));
    let (st, v, _) = post(CREDENTIALS, &Who::Owner, b);
    assert_eq!(st, 200, "{v}");
    // Status: the owner, an Admin, and the App's own agent at switch 1.
    switches(&f.ws, false, false);
    let (st, v, _) = get(STATUS, &agent(&f.ws), &[("app", &f.app)]);
    assert_eq!(st, 403, "switch 1 off: {v}");
    switches(&f.ws, true, false);
    for who in [Who::Owner, login(Role::Admin), agent(&f.ws)] {
        let (st, v, raw) = get(STATUS, &who, &[("app", &f.app)]);
        assert_eq!(st, 200, "{who:?}: {v}");
        assert_eq!(v["state"], "off");
        assert_eq!(v["off"], "not_approved");
        assert_eq!(v["apns"]["teamId"], "TEAM123456");
        assert!(!raw.contains("PRIVATE KEY"), "{raw}");
    }
    let (st, v, _) = get(STATUS, &agent(&f.other_ws), &[("app", &f.app)]);
    assert_eq!(st, 404, "{v}");
    // Remove: agent refused, owner needs a confirm.
    switches(&f.ws, true, true);
    let (st, v, _) = post(CREDENTIALS_REMOVE, &agent(&f.ws), json!({"app": f.app, "platform": "apns"}));
    assert_eq!((st, code(&v)), (403, "owner_only"), "{v}");
    let (st, v, _) = post(CREDENTIALS_REMOVE, &Who::Owner, json!({"app": f.app, "platform": "apns"}));
    assert_eq!((st, code(&v)), (403, "vault_confirm_required"), "{v}");
    let (st, v, _) = post(CREDENTIALS_REMOVE, &Who::Owner, json!({"app": f.app, "platform": "web"}));
    assert_eq!((st, code(&v)), (400, "usage"), "{v}");
    let tok = vault.confirm.mint(super::super::creds::CONFIRM_ACTION, Some(&f.app), "owner", "local-auth");
    let (st, v, _) = post(CREDENTIALS_REMOVE, &Who::Owner, json!({"app": f.app, "platform": "all", "confirmToken": tok}));
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["removed"], json!(["apns"]));
    assert_eq!(v["apns"], J::Null);
    assert_eq!(super::super::creds::load_apns(&f.app), Err(super::super::creds::KeyError::NotConfigured));
}
