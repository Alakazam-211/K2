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
