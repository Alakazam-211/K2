//! The send API through every door against a fake Apple and Google: the
//! App from the credential, the cross-App matrix, targets, topics, budgets,
//! status counts, and the owner/agent rules. Nothing reaches a vendor: the
//! global sender's transport is faked per App.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use k2_core::connect_users::Role;
use k2_core::skin::{self, app_push as store};
use rusqlite::params;
use serde_json::{json, Value as J};

use super::super::sender::{install_test_transport, HttpReq, HttpResp, Transport, TransportErr};
use super::super::test_keys::*;
use super::super::{guest_routes, owner_routes, sendlog, worker};
use super::*;
use crate::compute_routes::Who;

/// Records every request; 200 for everything (an OAuth token for the
/// token URL).
#[derive(Default)]
struct Fake {
    seen: Mutex<Vec<(String, Vec<u8>)>>,
}

impl Transport for Fake {
    fn post(&self, r: &HttpReq) -> Result<HttpResp, TransportErr> {
        self.seen.lock().expect("lock").push((r.url.clone(), r.body.clone()));
        let body = if r.url.ends_with("/token") {
            json!({"access_token": "ya29.test", "expires_in": 3600}).to_string()
        } else {
            "{}".to_string()
        };
        Ok(HttpResp { status: 200, retry_after: None, body: body.into_bytes() })
    }
}

impl Fake {
    fn sends(&self) -> Vec<(String, J)> {
        self.seen
            .lock()
            .expect("lock")
            .iter()
            .filter(|(u, _)| !u.ends_with("/token"))
            .map(|(u, b)| (u.clone(), serde_json::from_slice(b).unwrap_or(J::Null)))
            .collect()
    }
}

struct Fx {
    _db: k2_core::db::TestDbGuard,
    _home: crate::test_support::TempHome,
    ws: String,
    room2: String,
    app: String,
    other_app: String,
    fake: Arc<Fake>,
    other_fake: Arc<Fake>,
}

fn principal(username: &str) -> String {
    skin::list_principals().expect("principals").into_iter().find(|p| p.username == username).expect("principal").id
}

fn fx(tag: &str) -> Fx {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    crate::vault::ensure_test_vault();
    crate::apps::alerts::set_sink(Some(Box::new(|_: &str, _: &str| {})));
    let (ws, other_ws, room2) = (format!("ws-{tag}"), format!("ot-{tag}"), format!("r2-{tag}"));
    let (app, other_app) = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        for r in [&ws, &other_ws, &room2] {
            let path = home.path().join(r.as_str());
            std::fs::create_dir_all(&path).expect("dir");
            conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", params![r, path.to_string_lossy()]).expect("project");
        }
        let a = k2_core::published_services::insert(&conn, &ws, "jobs", "node s.js", "/tmp", 4340, "local", "running", "cmd", "").expect("svc");
        let o = k2_core::published_services::insert(&conn, &other_ws, "crm", "node s.js", "/tmp", 4341, "local", "running", "cmd", "").expect("svc");
        (a.id, o.id)
    };
    let fake = Arc::new(Fake::default());
    let other_fake = Arc::new(Fake::default());
    install_test_transport(&app, fake.clone());
    install_test_transport(&other_app, other_fake.clone());
    for a in [&app, &other_app] {
        store::set_switches(a, store::Switches { owner_on: Some(true), ..Default::default() }, "owner").expect("on");
        let k = super::super::creds::validate_apns(EC_P256_P8, "ABC123DEFG", "TEAM123456", "com.example.app").expect("apns");
        super::super::creds::store_apns(a, &k, "owner").expect("store");
    }
    // dana and eli are this App's guests (room ws); fay is the other App's.
    let mut policy = skin::RoomPolicy::new();
    policy.insert(ws.clone(), vec!["thread:read".into()]);
    skin::create_role_for_app(&format!("r-{tag}"), &policy, Some(&app)).expect("role");
    for u in ["dana", "eli"] {
        skin::add_principal_for_app(u, None, &app, Some(&format!("r-{tag}")), "test").expect("guest");
    }
    skin::add_principal_for_app("fay", None, &other_app, None, "test").expect("fay");
    Fx { _db: db, _home: home, ws, room2, app, other_app, fake, other_fake }
}

fn register(user: &str, app: &str, device: &str, token_byte: u8) {
    let s = skin::create_app_session_token(user, app).expect("session");
    let reg = store::parse_registration(&json!({"deviceId": device, "platform": "apns", "pushToken": format!("{token_byte:02x}").repeat(32)})).expect("reg");
    store::register(app, &principal(user), &s.meta.id, &reg).expect("register");
}

fn owner_caller() -> Caller {
    Caller { actor: "owner".into(), kind: payload::KIND_APP }
}

fn note(extra: J) -> Notification {
    let mut v = json!({"title": "Your job was approved", "body": "Tap to see"});
    for (k, x) in extra.as_object().expect("object") {
        v[k] = x.clone();
    }
    payload::parse(&v).expect("valid")
}

fn wait(app: &str) {
    assert!(worker::wait_app(app, Duration::from_secs(20)), "the worker finished {app}'s sends");
}

#[test]
fn a_send_reaches_this_apps_guests_and_counts_the_rest() {
    let f = fx("reach");
    register("dana", &f.app, "d1", 0xd1);
    register("dana", &f.app, "d2", 0xd2);
    register("fay", &f.other_app, "f1", 0xf1);
    let to = To::Guests(vec!["dana".into(), "eli".into(), "fay".into(), "nobody".into(), principal("dana")]);
    let v = send(&f.app, &owner_caller(), &to, note(json!({"topic": "jobs"}))).expect("queued");
    assert_eq!(v["queued"], 2, "{v}");
    assert_eq!(v["skipped"]["not_a_guest"], 2, "fay (another App's) and nobody: one count: {v}");
    assert_eq!(v["skipped"]["no_devices"], 1, "eli: {v}");
    let id = v["sendId"].as_str().expect("sendId").to_string();
    wait(&f.app);
    let r = sendlog::get(&f.app, &id).expect("record");
    assert_eq!((r.queued, r.sent, r.dead, r.failed), (2, 2, 0, 0));
    let sends = f.fake.sends();
    assert_eq!(sends.len(), 2);
    assert!(f.other_fake.sends().is_empty(), "App A never reaches App B's guest");
    let body = &sends[0].1;
    assert_eq!(body["k2App"], f.app.as_str());
    assert_eq!(body["k2Send"], id.as_str());
    assert_eq!(body["k2Topic"], "jobs");
    let raw = sends.iter().map(|(_, b)| b.to_string()).collect::<String>();
    assert!(!raw.contains("dana") && !raw.contains(&principal("dana")), "who never rides a push");
    let who = body["k2Who"].as_str().expect("who");
    assert_eq!(who, super::super::who(&f.app, &principal("dana")).expect("who"));
}

#[test]
fn topics_rooms_and_the_owner_switches_shape_a_send() {
    let f = fx("shape");
    register("dana", &f.app, "d1", 0xd1);
    register("eli", &f.app, "e1", 0xe1);
    store::set_prefs(&f.app, &principal("eli"), "e1", None, Some(&["promo".to_string()])).expect("prefs").expect("row");
    let v = send(&f.app, &owner_caller(), &To::Room(f.ws.clone()), note(json!({"topic": "promo"}))).expect("room send");
    assert_eq!((v["queued"].clone(), v["skipped"]["muted_topic"].clone()), (json!(1), json!(1)), "{v}");
    wait(&f.app);
    // Another room (not this App's) is outside_app_rooms, as is a room that doesn't exist.
    for room in [f.room2.clone(), "no-such-room".to_string()] {
        let e = send(&f.app, &owner_caller(), &To::Room(room.clone()), note(json!({}))).expect_err("outside");
        assert_eq!((e.status, e.code), (403, "outside_app_rooms"), "{room}");
    }
    // The owner's off wins.
    store::set_switches(&f.app, store::Switches { enabled: Some(false), ..Default::default() }, "owner").expect("off");
    let e = send(&f.app, &owner_caller(), &To::Guest("dana".into()), note(json!({}))).expect_err("off");
    assert_eq!((e.status, e.code), (403, "push_off"));
    store::set_switches(&f.app, store::Switches { enabled: Some(true), ..Default::default() }, "owner").expect("on");
    // No keys at all: push_not_configured.
    super::super::creds::remove(&f.app, "apns", "owner").expect("remove");
    let e = send(&f.app, &owner_caller(), &To::Guest("dana".into()), note(json!({}))).expect_err("no keys");
    assert_eq!((e.status, e.code), (409, "push_not_configured"));
    // Air-gap.
    let k = super::super::creds::validate_apns(EC_P256_P8, "ABC123DEFG", "TEAM123456", "com.example.app").expect("apns");
    super::super::creds::store_apns(&f.app, &k, "owner").expect("store");
    crate::push_routes::set_test_airgap(true);
    let e = send(&f.app, &owner_caller(), &To::Guest("dana".into()), note(json!({}))).expect_err("airgap");
    crate::push_routes::set_test_airgap(false);
    assert_eq!((e.status, e.code), (503, "airgap"));
    // An FCM-only device on an APNs-only App: platform_not_configured.
    let s = skin::create_app_session_token("dana", &f.app).expect("session");
    let reg = store::parse_registration(&json!({"deviceId": "a1", "platform": "fcm", "pushToken": "fcm-token-0123456789-abcdef"})).expect("reg");
    store::register(&f.app, &principal("dana"), &s.meta.id, &reg).expect("register");
    let v = send(&f.app, &owner_caller(), &To::Guest("dana".into()), note(json!({}))).expect("queued");
    assert_eq!((v["queued"].clone(), v["skipped"]["platform_not_configured"].clone()), (json!(1), json!(1)), "{v}");
    wait(&f.app);
}

#[test]
fn per_guest_budgets_skip_and_recipient_ceilings_refuse() {
    let f = fx("budget");
    register("dana", &f.app, "d1", 0xd1);
    for i in 0..ALERTS_PER_GUEST_HOUR {
        let v = send(&f.app, &owner_caller(), &To::Guest("dana".into()), note(json!({}))).expect("queued");
        assert_eq!(v["queued"], 1, "send {i}: {v}");
    }
    let v = send(&f.app, &owner_caller(), &To::Guest("dana".into()), note(json!({}))).expect("answered");
    assert_eq!((v["queued"].clone(), v["skipped"]["rate_limited"].clone()), (json!(0), json!(1)), "the 31st alert: {v}");
    let v = send(&f.app, &owner_caller(), &To::Guest("dana".into()), payload::parse(&json!({"silent": true})).expect("silent")).expect("silent");
    assert_eq!(v["queued"], 1, "silent pushes have their own budget: {v}");
    wait(&f.app);
    let too_many: Vec<J> = (0..=MAX_RECIPIENTS).map(|i| json!(format!("g{i}"))).collect();
    let e = parse_to(&json!({"to": {"guests": too_many}})).expect_err("too many");
    assert_eq!((e.status, e.code), (413, "too_many_recipients"));
    assert_eq!(e.extra["count"], MAX_RECIPIENTS + 1);
    for bad in [json!({}), json!({"to": {}}), json!({"to": {"guest": "a", "room": "b"}}), json!({"to": {"guests": []}}), json!({"to": {"guests": [1]}})] {
        assert_eq!(parse_to(&bad).expect_err("bad to").code, "usage", "{bad}");
    }
}

fn link_pass(f: &Fx, caps: &[&str]) -> skin::SkinPass {
    let caps: Vec<String> = caps.iter().map(|c| c.to_string()).collect();
    let m = skin::link::mint_link_pass(&f.app, "jobs", "link-test", &caps, std::slice::from_ref(&f.ws)).expect("link pass");
    skin::link::resolve_link_pass(&m.raw, "link-test", &f.app).expect("resolves on its link")
}

fn link_call(path: &str, pass: &skin::SkinPass, link_app: Option<&str>, is_post: bool, body: J, q: &[(&str, &str)]) -> (u16, J) {
    let params: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let r = guest_routes::handle_pass(path, pass, link_app, is_post, body.to_string().as_bytes(), &params);
    (r.status.split_whitespace().next().expect("status").parse().expect("number"), serde_json::from_str(&r.body).expect("json"))
}

#[test]
fn the_link_sends_only_with_its_own_pass_and_push_send() {
    let f = fx("link");
    register("dana", &f.app, "d1", 0xd1);
    let body = json!({"to": {"guest": "dana"}, "title": "Hi", "body": "There"});
    let with = link_pass(&f, &["push:send", "thread:read"]);
    let (st, v) = link_call(guest_routes::SEND, &with, Some(f.app.as_str()), true, body.clone(), &[]);
    assert_eq!(st, 202, "{v}");
    let id = v["sendId"].as_str().expect("id").to_string();
    wait(&f.app);
    let (st, v) = link_call(guest_routes::STATUS, &with, Some(f.app.as_str()), false, json!({}), &[("sendId", &id)]);
    assert_eq!((st, v["sent"].clone(), v["queued"].clone()), (200, json!(1), json!(1)), "{v}");
    let (st, v) = link_call(guest_routes::DEVICES, &with, Some(f.app.as_str()), false, json!({}), &[]);
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["guests"], json!([{"guest": principal("dana"), "devices": 1, "platforms": ["apns"]}]));
    // Without the cap.
    let without = link_pass(&f, &["thread:read"]);
    let (st, v) = link_call(guest_routes::SEND, &without, Some(f.app.as_str()), true, body.clone(), &[]);
    assert_eq!((st, v["error"]["code"].clone()), (403, json!("cap_not_granted")), "{v}");
    // Off its link, or on another App's link.
    let (st, v) = link_call(guest_routes::SEND, &with, None, true, body.clone(), &[]);
    assert_eq!((st, v["error"]["code"].clone()), (403, json!("app_pass_required")), "{v}");
    let (st, v) = link_call(guest_routes::SEND, &with, Some(f.other_app.as_str()), true, body.clone(), &[]);
    assert_eq!((st, v["error"]["code"].clone()), (403, json!("app_pass_required")), "{v}");
    // A guest's session on the link never sends.
    let s = skin::create_app_session_token("dana", &f.app).expect("session");
    let guest = skin::resolve_skin_token(&s.raw).expect("pass");
    let (st, v) = link_call(guest_routes::SEND, &guest, Some(f.app.as_str()), true, body.clone(), &[]);
    assert_eq!((st, v["error"]["code"].clone()), (403, json!("app_pass_required")), "{v}");
    // A body naming another App is refused, never read.
    let mut b = body.clone();
    b["app"] = json!(f.other_app);
    let (st, v) = link_call(guest_routes::SEND, &with, Some(f.app.as_str()), true, b, &[]);
    assert_eq!((st, v["error"]["code"].clone()), (400, json!("usage")), "{v}");
    // Another App's send id isn't found.
    let (st, _) = link_call(guest_routes::STATUS, &with, Some(f.app.as_str()), false, json!({}), &[("sendId", "snd_nope")]);
    assert_eq!(st, 404);
}

#[test]
fn the_app_key_door_needs_scope_push_and_its_own_app() {
    let f = fx("key");
    register("dana", &f.app, "d1", 0xd1);
    let mint = |app: &str, scopes: &[&str]| {
        let scopes: Vec<String> = scopes.iter().map(|s| s.to_string()).collect();
        let g = k2_core::app_grace::keys::grant_for_app(app, &scopes, &[], &[]).expect("grant");
        k2_core::api_keys::create_app_key("test", app, &g.to_json()).expect("key").1
    };
    let push_key = mint(&f.app, &["push"]);
    let users_key = mint(&f.app, &["users"]);
    let other_key = mint(&f.other_app, &["push"]);
    let body = json!({"to": {"guest": "dana"}, "title": "Hi", "body": "There"}).to_string();
    let path = format!("/v1/apps/{}/push/send", f.app);
    let r = crate::apps::app_key_routes::handle(&path, true, Some(&push_key), body.as_bytes(), &HashMap::new());
    assert_eq!(r.status, "202 Accepted", "{}", r.body);
    wait(&f.app);
    for k in [&users_key, &other_key] {
        let r = crate::apps::app_key_routes::handle(&path, true, Some(k), body.as_bytes(), &HashMap::new());
        assert_eq!(r.status, "404 Not Found", "the uniform answer: {}", r.body);
    }
    let r = crate::apps::app_key_routes::handle(&path, true, None, body.as_bytes(), &HashMap::new());
    assert_eq!(r.status, "404 Not Found");
    let id: J = serde_json::from_str(&crate::apps::app_key_routes::handle(&path, true, Some(&push_key), body.as_bytes(), &HashMap::new()).body).expect("json");
    let mut q = HashMap::new();
    q.insert("sendId".to_string(), id["sendId"].as_str().expect("id").to_string());
    let r = crate::apps::app_key_routes::handle(&format!("/v1/apps/{}/push/status", f.app), false, Some(&push_key), b"", &q);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    wait(&f.app);
    assert!(crate::apps::app_key_routes::post_allowed(&path));
}

fn owner_post(path: &str, who: &Who, body: J) -> (u16, J) {
    let r = owner_routes::handle(path, who.clone(), body.to_string().as_bytes(), &HashMap::new());
    (r.status.split_whitespace().next().expect("status").parse().expect("number"), serde_json::from_str(&r.body).expect("json"))
}

fn switches(ws: &str, build: bool, full: bool) {
    let h = k2_core::db::shared();
    let conn = h.lock();
    conn.execute("UPDATE projects SET agents_can_manage_skin = ?2, agents_can_manage_apps_full = ?3 WHERE id = ?1", params![ws, build as i64, full as i64])
        .expect("switches");
}

#[test]
fn owner_and_agent_rules_for_send_test_set_and_log() {
    let f = fx("owner");
    register("dana", &f.app, "d1", 0xd1);
    let agent = Who::Agent { project_id: f.ws.clone(), session_id: "s".into() };
    let send_body = json!({"app": f.app, "to": {"guest": "dana"}, "title": "Hi", "body": "There"});
    switches(&f.ws, true, false);
    let (st, v) = owner_post(owner_routes::SEND, &agent, send_body.clone());
    assert_eq!(st, 403, "switch 1 only: {v}");
    switches(&f.ws, true, true);
    let (st, v) = owner_post(owner_routes::SEND, &agent, send_body.clone());
    assert_eq!(st, 202, "switch 2: {v}");
    let (st, v) = owner_post(owner_routes::TEST, &Who::Login { username: "ada".into(), role: Role::Admin }, json!({"app": f.app, "to": {"guest": "dana"}}));
    assert_eq!(st, 202, "an Admin tests: {v}");
    let (st, v) = owner_post(owner_routes::SEND, &Who::Login { username: "m".into(), role: Role::Member }, send_body.clone());
    assert_eq!(st, 403, "a Member never sends: {v}");
    wait(&f.app);
    let bodies = f.fake.sends();
    assert!(bodies.iter().any(|(_, b)| b["aps"]["alert"]["title"] == owner_routes::TEST_TEXT), "the test's fixed text");
    // set: the agent may lower and set replies at switch 2, never turn on.
    let (st, v) = owner_post(owner_routes::SET, &agent, json!({"app": f.app, "enabled": true}));
    assert_eq!((st, v["error"]["code"].clone()), (403, json!("owner_only")), "{v}");
    let (st, v) = owner_post(owner_routes::SET, &agent, json!({"app": f.app, "agentReplies": true, "previews": false}));
    assert_eq!(st, 200, "{v}");
    assert_eq!((v["agentReplies"].clone(), v["previews"].clone()), (json!(true), json!(false)));
    let (st, v) = owner_post(owner_routes::SET, &agent, json!({"app": f.app, "enabled": false}));
    assert_eq!((st, v["state"].clone(), v["off"].clone()), (200, json!("off"), json!("owner_off")), "{v}");
    let (st, v) = owner_post(owner_routes::SET, &Who::Login { username: "ada".into(), role: Role::Admin }, json!({"app": f.app, "enabled": true}));
    assert_eq!(st, 403, "an Admin can't turn it on: {v}");
    let shell = Who::Shell { project_id: f.ws.clone(), session_id: "s".into() };
    let (st, v) = owner_post(owner_routes::SET, &shell, json!({"app": f.app, "enabled": true}));
    assert_eq!((st, v["error"]["code"].clone()), (403, json!("owner_only")), "a shell tab can't pass the confirm: {v}");
    // The owner token alone is not enough to turn it on (review A-L2/B-M2).
    let (st, v) = owner_post(owner_routes::SET, &Who::Owner, json!({"app": f.app, "enabled": true}));
    assert_eq!((st, v["error"]["code"].clone(), v["error"]["serviceId"].clone()), (403, json!("vault_confirm_required"), json!(f.app)), "{v}");
    assert_eq!(v["error"]["action"], super::super::CONFIRM_ON);
    let vault = crate::vault::ensure_test_vault();
    let keys_tok = vault.confirm.mint(super::super::creds::CONFIRM_ACTION, Some(&f.app), "owner", "local-auth");
    let (st, v) = owner_post(owner_routes::SET, &Who::Owner, json!({"app": f.app, "enabled": true, "confirmToken": keys_tok}));
    assert_eq!(st, 403, "a keys confirm doesn't turn push on: {v}");
    let other_tok = vault.confirm.mint(super::super::CONFIRM_ON, Some(&f.other_app), "owner", "local-auth");
    let (st, v) = owner_post(owner_routes::SET, &Who::Owner, json!({"app": f.app, "enabled": true, "confirmToken": other_tok}));
    assert_eq!(st, 403, "another App's confirm doesn't either: {v}");
    let tok = vault.confirm.mint(super::super::CONFIRM_ON, Some(&f.app), "owner", "local-auth");
    let (st, v) = owner_post(owner_routes::SET, &Who::Owner, json!({"app": f.app, "enabled": true, "confirmToken": tok}));
    assert_eq!((st, v["state"].clone()), (200, json!("on")), "{v}");
    // Lowering the ceiling needs no confirm (an Admin may); raising does.
    let admin = Who::Login { username: "ada".into(), role: Role::Admin };
    let (st, v) = owner_post(owner_routes::SET, &admin, json!({"app": f.app, "maxDevices": 50}));
    assert_eq!((st, v["maxDevices"].clone()), (200, json!(50)), "{v}");
    let (st, v) = owner_post(owner_routes::SET, &admin, json!({"app": f.app, "maxDevices": 60}));
    assert_eq!((st, v["error"]["code"].clone()), (403, json!("owner_only")), "{v}");
    let (st, v) = owner_post(owner_routes::SET, &Who::Owner, json!({"app": f.app, "maxDevices": 60}));
    assert_eq!((st, v["error"]["code"].clone()), (403, json!("vault_confirm_required")), "{v}");
    let tok = vault.confirm.mint(super::super::CONFIRM_ON, Some(&f.app), "owner", "local-auth");
    let (st, v) = owner_post(owner_routes::SET, &Who::Owner, json!({"app": f.app, "maxDevices": 60, "confirmToken": tok}));
    assert_eq!((st, v["maxDevices"].clone()), (200, json!(60)), "{v}");
    // The confirm sheet names the App from the daemon's record (review A-L1).
    let reason = super::super::confirm_reason(super::super::CONFIRM_ON, Some(&f.app)).expect("an App push action").expect("a known App");
    assert!(reason.contains("jobs") && reason.contains(&f.app), "{reason}");
    let keys = super::super::confirm_reason(super::super::creds::CONFIRM_ACTION, Some(&f.app)).expect("action").expect("App");
    assert!(keys.contains("push keys") && keys.contains(&f.app), "{keys}");
    assert!(super::super::confirm_reason(super::super::CONFIRM_ON, Some("no-such-app")).expect("action").is_err());
    assert!(super::super::confirm_reason(super::super::CONFIRM_ON, None).expect("action").is_err());
    assert!(super::super::confirm_reason("app-key-mint", Some(&f.app)).is_none(), "other actions keep their own text");
    // Another workspace's agent can't name the App at all.
    let stranger = Who::Agent { project_id: "nope".into(), session_id: "s".into() };
    let (st, _) = owner_post(owner_routes::SEND, &stranger, send_body);
    assert_eq!(st, 404);
    // The log: counts only.
    let r = owner_routes::handle(owner_routes::LOG, agent.clone(), b"", &[("app".to_string(), f.app.clone())].into_iter().collect());
    assert!(r.status.starts_with("200"), "{}", r.body);
    let v: J = serde_json::from_str(&r.body).expect("json");
    assert_eq!(v["sends"].as_array().expect("sends").len(), 2);
    assert!(!r.body.contains("There") && !r.body.contains("Hi\""), "no text in the log: {}", r.body);
}

#[test]
fn push_send_is_in_no_core_so_a_deploy_adding_it_is_held() {
    assert_eq!(k2_core::workspace::settings::app_core_of_cap(skin::CAP_PUSH_SEND), None);
}

/// Review B-L1: a queued send is checked again right before it goes out.
#[test]
fn a_queued_send_is_dropped_when_the_device_guest_or_app_changed() {
    let f = fx("recheck");
    register("dana", &f.app, "d1", 0xd1);
    let target = |user: &str| {
        let d = store::devices_for_app(&f.app, Some(&principal(user))).expect("list").remove(0);
        Target::from(&d)
    };
    let t = target("dana");
    assert!(worker::deliverable(&t));
    let mut wrong = t.clone();
    wrong.token = "ee".repeat(32);
    assert!(!worker::deliverable(&wrong), "the row now holds another token");
    // The App's push turned off after the send was queued.
    store::set_switches(&f.app, store::Switches { enabled: Some(false), ..Default::default() }, "owner").expect("off");
    assert!(!worker::deliverable(&t), "push off");
    store::set_switches(&f.app, store::Switches { enabled: Some(true), ..Default::default() }, "owner").expect("on");
    assert!(worker::deliverable(&t));
    // The approval withdrawn (no push:send, no owner on): queued sends stop
    // (review L1).
    store::set_switches(&f.app, store::Switches { owner_on: Some(false), ..Default::default() }, "owner").expect("unapprove");
    assert!(!worker::deliverable(&t), "push:send no longer approved");
    store::set_switches(&f.app, store::Switches { owner_on: Some(true), ..Default::default() }, "owner").expect("approve");
    assert!(worker::deliverable(&t));
    // The guest's grant disabled (their sessions end with it).
    let g = skin::list_grants().expect("grants").into_iter().find(|g| g.subject_id == principal("dana") && g.target_id == f.app).expect("grant");
    skin::set_grant_enabled(&g.id, false).expect("disable");
    assert!(!worker::deliverable(&t), "grant disabled");
    // A send queued for a device that changed answers dropped, not sent.
    skin::set_grant_enabled(&g.id, true).expect("enable");
    register("dana", &f.app, "d1", 0xd1);
    let t = target("dana");
    skin::revoke_sessions_for_app(&f.app).expect("sign everyone out");
    assert!(!worker::deliverable(&t), "signed out");
}

/// Review B-L5: the App's device budget is taken before any guest's
/// bucket, so a send the box refuses never spends a guest's allowance.
#[test]
fn the_app_budget_refuses_before_a_guest_bucket_is_spent() {
    let f = fx("order");
    register("dana", &f.app, "d1", 0xd1);
    device_sends().take_n(&f.app, DEVICE_SENDS_PER_HOUR, DEVICE_SENDS_PER_HOUR).expect("fill the App's device budget");
    let e = send(&f.app, &owner_caller(), &To::Guest("dana".into()), note(json!({}))).expect_err("over the App's budget");
    assert_eq!((e.status, e.code), (429, "rate_limited"));
    let key = format!("{}\0{}\0alert", f.app, principal("dana"));
    per_guest().take_n(&key, ALERTS_PER_GUEST_HOUR, ALERTS_PER_GUEST_HOUR).expect("dana's whole hourly allowance is still there");
}
