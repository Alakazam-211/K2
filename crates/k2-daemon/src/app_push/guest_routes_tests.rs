//! The guest's push routes in process: guest passes only, the App from the
//! pass, push_off / not_for_host / channel_unavailable, the register rate,
//! the reply shape (no token), and the no-oracle unregister.

use std::collections::HashMap;

use k2_core::skin::{self, app_push as store, SkinPass};
use rusqlite::params;
use serde_json::{json, Value as J};

use super::*;

struct Fx {
    _db: k2_core::db::TestDbGuard,
    _home: crate::test_support::TempHome,
    app: String,
    other_app: String,
}

const APNS: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90";

fn fx(tag: &str) -> Fx {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    let (ws, other) = (format!("ws-{tag}"), format!("ot-{tag}"));
    let (app, other_app) = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        for r in [&ws, &other] {
            conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", params![r, format!("/tmp/apg-{r}")])
                .expect("project");
        }
        let a = k2_core::published_services::insert(&conn, &ws, "jobs", "node s.js", "/tmp", 4310, "local", "running", "cmd", "").expect("svc");
        let o = k2_core::published_services::insert(&conn, &other, "crm", "node s.js", "/tmp", 4311, "local", "running", "cmd", "").expect("svc");
        (a.id, o.id)
    };
    skin::add_principal_for_app("dana", None, &app, None, "test").expect("dana");
    skin::add_principal_for_app("eli", None, &other_app, None, "test").expect("eli");
    Fx { _db: db, _home: home, app, other_app }
}

fn pass_for(user: &str, app: &str) -> SkinPass {
    let s = skin::create_app_session_token(user, app).expect("session");
    skin::resolve_skin_token(&s.raw).expect("live pass")
}

fn turn_on(app: &str) {
    store::set_switches(app, store::Switches { owner_on: Some(true), ..Default::default() }, "owner").expect("on");
}

fn status(r: &crate::cli_response::CliResponse) -> u16 {
    r.status.split_whitespace().next().expect("status").parse().expect("number")
}

fn call(path: &str, pass: &SkinPass, is_post: bool, body: J, q: &[(&str, &str)]) -> (u16, J, String) {
    let params: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let r = handle_pass(path, pass, None, is_post, body.to_string().as_bytes(), &params);
    let v: J = serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("json body ({e}): {}", r.body));
    (status(&r), v, r.body)
}

fn code(v: &J) -> &str {
    v["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {v}"))
}

fn reg_body(device: &str) -> J {
    json!({"deviceId": device, "platform": "apns", "pushToken": APNS, "environment": "sandbox"})
}

#[test]
fn register_answers_the_device_never_the_token() {
    let f = fx("ok");
    let p = pass_for("dana", &f.app);
    let (st, v, _) = call(REGISTER, &p, true, reg_body("iphone-1"), &[]);
    assert_eq!((st, code(&v)), (403, "push_off"), "off until the owner turns it on: {v}");
    turn_on(&f.app);
    let (st, v, raw) = call(REGISTER, &p, true, reg_body("iphone-1"), &[]);
    assert_eq!(st, 200, "{v}");
    assert!(!raw.contains(APNS), "the token never comes back: {raw}");
    assert_eq!(v["ok"], true);
    assert_eq!(v["deviceId"], "iphone-1");
    assert_eq!((v["platform"].as_str(), v["environment"].as_str(), v["channel"].as_str()), (Some("apns"), Some("sandbox"), Some("app")));
    assert_eq!(v["previews"], J::Null);
    assert_eq!(v["configured"], json!({"apns": false, "fcm": false}));
    let who = v["who"].as_str().expect("who");
    assert_eq!(who.len(), 32, "{who}");
    assert!(who.chars().all(|c| c.is_ascii_hexdigit()));
    let keys: Vec<&String> = v.as_object().expect("object").keys().collect();
    assert_eq!(
        keys.iter().map(|k| k.as_str()).collect::<Vec<_>>(),
        ["channel", "configured", "deviceId", "environment", "id", "lastSeenAt", "mutedTopics", "ok", "platform", "previews", "who"],
        "exactly the CP9 fields"
    );
    // The App comes from the pass: a body naming another App is ignored.
    let mut b = reg_body("iphone-2");
    b["app"] = json!(f.other_app);
    let (st, v, _) = call(REGISTER, &p, true, b, &[]);
    assert_eq!((st, code(&v)), (400, "usage"), "unknown fields are refused, never read: {v}");
    assert_eq!(store::devices_for_app(&f.other_app, None).expect("list").len(), 0);
    // The device read and prefs answer the same shape.
    let (st, d, _) = call(DEVICE, &p, false, json!({}), &[("deviceId", "iphone-1")]);
    assert_eq!(st, 200, "{d}");
    assert_eq!(d["who"], who);
    let (st, d, _) = call(PREFS, &p, true, json!({"deviceId": "iphone-1", "previews": false, "mutedTopics": ["promo"]}), &[]);
    assert_eq!(st, 200, "{d}");
    assert_eq!((d["previews"].clone(), d["mutedTopics"].clone()), (json!(false), json!(["promo"])));
    // Another App's guest gets another who.
    turn_on(&f.other_app);
    let pe = pass_for("eli", &f.other_app);
    let (_, e, _) = call(REGISTER, &pe, true, reg_body("iphone-9"), &[]);
    assert_ne!(e["who"], who);
}

#[test]
fn only_a_guest_session_pass_registers() {
    let f = fx("who");
    turn_on(&f.app);
    // A platform pass (session = 0), like an App's own link pass.
    let mut app_pass = pass_for("dana", &f.app);
    app_pass.session = false;
    app_pass.principal_id = None;
    for path in [REGISTER, UNREGISTER, PREFS] {
        let (st, v, _) = call(path, &app_pass, true, reg_body("x"), &[]);
        assert_eq!((st, code(&v)), (403, "guest_required"), "{path}: {v}");
    }
    let (st, v, _) = call(DEVICE, &app_pass, false, json!({}), &[("deviceId", "x")]);
    assert_eq!((st, code(&v)), (403, "guest_required"), "{v}");
    assert_eq!(status(&not_a_pass()), 403);
    assert_eq!(status(&pass_required()), 401);
    // The box login's session: never push.
    let mut host = pass_for("dana", &f.app);
    host.app = skin::HOST_APP_ID.to_string();
    let (st, v, _) = call(REGISTER, &host, true, reg_body("x"), &[]);
    assert_eq!((st, code(&v)), (400, "not_for_host"), "{v}");
    // Wrong methods.
    let r = handle_pass(REGISTER, &pass_for("dana", &f.app), None, false, b"", &HashMap::new());
    assert_eq!(status(&r), 405);
    let r = handle_pass(DEVICE, &pass_for("dana", &f.app), None, true, b"{}", &HashMap::new());
    assert_eq!(status(&r), 405);
}

#[test]
fn companion_channel_and_bad_bodies_are_refused_before_anything_is_stored() {
    let f = fx("bad");
    turn_on(&f.app);
    let p = pass_for("dana", &f.app);
    let mut b = reg_body("x");
    b["channel"] = json!("k2-companion");
    let (st, v, _) = call(REGISTER, &p, true, b, &[]);
    assert_eq!((st, code(&v)), (400, "channel_unavailable"), "{v}");
    let (st, v, raw) = call(REGISTER, &p, true, json!({"deviceId": "x", "platform": "apns", "pushToken": "not-hex-but-long-0123456789"}), &[]);
    assert_eq!((st, code(&v)), (400, "usage"), "{v}");
    assert!(!raw.contains("not-hex-but-long"), "never echoes the token: {raw}");
    let r = handle_pass(REGISTER, &p, None, true, b"[1,2]", &HashMap::new());
    assert_eq!(status(&r), 400);
    assert_eq!(store::devices_for_app(&f.app, None).expect("list").len(), 0);
}

#[test]
fn thirty_registers_per_pass_an_hour() {
    let f = fx("rate");
    turn_on(&f.app);
    let p = pass_for("dana", &f.app);
    for i in 0..REGISTERS_PER_HOUR {
        let (st, v, _) = call(REGISTER, &p, true, reg_body(&format!("d{}", i % 3)), &[]);
        assert_eq!(st, 200, "register {i}: {v}");
    }
    let (st, v, _) = call(REGISTER, &p, true, reg_body("d0"), &[]);
    assert_eq!((st, code(&v)), (429, "rate_limited"), "{v}");
    let wait = v["error"]["retryAfterMs"].as_u64().expect("retryAfterMs");
    assert!(wait > 0 && wait <= 3_600_000, "{wait}");
    assert_eq!(v["error"]["limit"], REGISTERS_PER_HOUR);
    // A new pass (a new sign-in) has its own window.
    let (st, v, _) = call(REGISTER, &pass_for("dana", &f.app), true, reg_body("d0"), &[]);
    assert_eq!(st, 200, "{v}");
}

#[test]
fn unregister_is_one_answer_for_a_miss_and_someone_elses_device() {
    let f = fx("unreg");
    turn_on(&f.app);
    skin::add_principal_for_app("fay", None, &f.app, None, "test").expect("fay");
    let pd = pass_for("dana", &f.app);
    let pf = pass_for("fay", &f.app);
    let (st, _, _) = call(REGISTER, &pd, true, reg_body("dana-phone"), &[]);
    assert_eq!(st, 200);
    let (st, v, _) = call(UNREGISTER, &pf, true, json!({"deviceId": "dana-phone"}), &[]);
    assert_eq!((st, v.clone()), (200, json!({"ok": true, "removed": false})), "{v}");
    let (st, v, _) = call(UNREGISTER, &pf, true, json!({"deviceId": "never-was"}), &[]);
    assert_eq!((st, v.clone()), (200, json!({"ok": true, "removed": false})), "{v}");
    let (st, v, _) = call(DEVICE, &pf, false, json!({}), &[("deviceId", "dana-phone")]);
    assert_eq!((st, code(&v)), (404, "not_found"), "fay can't read dana's device: {v}");
    let (st, v, _) = call(UNREGISTER, &pd, true, json!({"deviceId": "dana-phone"}), &[]);
    assert_eq!((st, v.clone()), (200, json!({"ok": true, "removed": true})), "{v}");
    let (st, v, _) = call(UNREGISTER, &pd, true, json!({}), &[]);
    assert_eq!((st, code(&v)), (400, "usage"), "{v}");
    // Unregister works with push off (a guest can always leave).
    let (st, _, _) = call(REGISTER, &pd, true, reg_body("dana-phone"), &[]);
    assert_eq!(st, 200);
    store::set_switches(&f.app, store::Switches { enabled: Some(false), ..Default::default() }, "owner").expect("off");
    let (st, v, _) = call(UNREGISTER, &pd, true, json!({"deviceId": "dana-phone"}), &[]);
    assert_eq!((st, v["removed"].clone()), (200, json!(true)), "{v}");
    let (st, v, _) = call(REGISTER, &pd, true, reg_body("dana-phone"), &[]);
    assert_eq!((st, code(&v)), (403, "push_off"), "the owner's off wins over owner_on: {v}");
}

#[test]
fn the_app_ceiling_answers_push_devices_full() {
    let f = fx("full");
    turn_on(&f.app);
    store::set_max_devices(&f.app, Some(1), "owner").expect("ceiling");
    let p = pass_for("dana", &f.app);
    let (st, _, _) = call(REGISTER, &p, true, reg_body("a"), &[]);
    assert_eq!(st, 200);
    let mut b = reg_body("b");
    b["pushToken"] = json!("ffeeddccbbaa99887766554433221100ffeeddccbbaa99887766554433221100");
    let (st, v, _) = call(REGISTER, &p, true, b, &[]);
    assert_eq!((st, code(&v)), (409, "push_devices_full"), "{v}");
    assert_eq!(v["error"]["limit"], 1);
}
