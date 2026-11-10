//! Recording in Apps (Sew 0.9.0, K2 PRD prd-k2-bobbin-in-apps-v1 §8 K-c),
//! the App routes in process: each method on its own path (405 the other
//! way), a Sew without `app_recording` refused before anything, an agent
//! narrows its own App but never turns `required` / `server_read` on (the
//! owner only) or widens (a person), the keep days never above the org's.

use std::collections::HashMap;

use k2_core::connect_users::Role;
use rusqlite::params;
use serde_json::{json, Value as J};

use super::*;

struct Fx {
    _home: crate::test_support::TempHome,
    _db: k2_core::db::TestDbGuard,
    ws: String,
    app: String,
}

const ALL: [&str; 4] = ["app_recording", "voice_record_cap", "recording_required", "mgmt_recording_reads"];

fn fx(tag: &str, flags: &[&str]) -> Fx {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    let ws = format!("ws-rec-{tag}");
    let app = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", params![ws, format!("/tmp/{ws}")]).unwrap();
        k2_core::published_services::insert(&conn, &ws, "jobs", "node s.js", "/tmp", 4710, "local", "running", "cmd", "").unwrap().id
    };
    k2_core::tool_hook::note_features_for_tests(flags).unwrap();
    Fx { _home: home, _db: db, ws, app }
}

fn agent(ws: &str) -> Who {
    Who::Agent { project_id: ws.into(), session_id: format!("sess-{ws}") }
}

/// Switches 1 and 2 ("…and manage them end to end") for `ws`'s agent.
fn full_switches(ws: &str) {
    let h = k2_core::db::shared();
    let conn = h.lock();
    conn.execute("UPDATE projects SET agents_can_manage_skin = 1, agents_can_manage_apps_full = 1 WHERE id = ?1", params![ws]).expect("switches");
}

fn call(path: &str, who: &Who, post: bool, body: J, q: &[(&str, &str)]) -> (u16, J) {
    let p: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let r = crate::apps::guest_rooms::handle(path, who, post, body.to_string().as_bytes(), &p);
    let st = r.status.split_whitespace().next().expect("status").parse().expect("number");
    (st, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("json ({e}): {}", r.body)))
}

fn code(v: &J) -> &str {
    v["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {v}"))
}

/// §8 K-c: `GET /cli/app/recording` with a POST → 405, and the set with a
/// GET → 405 (B3: each method on its own path).
#[test]
fn each_method_has_its_own_path() {
    let f = fx("405", &ALL);
    let (s, v) = call(READ, &Who::Owner, true, json!({ "app": f.app }), &[]);
    assert_eq!((s, code(&v)), (405, "method_not_allowed"), "{v}");
    let (s, v) = call(SET, &Who::Owner, false, json!({}), &[("app", &f.app)]);
    assert_eq!((s, code(&v)), (405, "method_not_allowed"), "{v}");
    let (s, v) = call(READ, &Who::Owner, false, json!({}), &[("app", &f.app)]);
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["flags"]["appRecording"], json!(true));
    assert_eq!(v["recording"], json!({}));
    assert!(crate::apps::guest_rooms::POST_ROUTES.contains(&SET) && !crate::apps::guest_rooms::POST_ROUTES.contains(&READ));
    assert_eq!(crate::routes::route_policy::lookup(SET).and_then(|r| r.post), Some(crate::routes::route_policy::Floor::Admin));
    assert_eq!(crate::routes::route_policy::lookup(READ).and_then(|r| r.get), Some(crate::routes::route_policy::Floor::Admin));
}

/// §7: on Sew 0.8.x (no `app_recording`) nothing is stored.
#[test]
fn a_sew_without_app_recording_takes_nothing() {
    let f = fx("old", &["app_governed_spaces"]);
    let (s, v) = call(SET, &Who::Owner, true, json!({ "app": f.app, "startLevel": "none" }), &[]);
    assert_eq!((s, code(&v)), (409, "sew_feature_missing"), "{v}");
    assert_eq!(v["error"]["feature"], "app_recording");
    let (_, v) = call(READ, &Who::Owner, false, json!({}), &[("app", &f.app)]);
    assert_eq!(v["recording"], json!({}), "nothing stored: {v}");
}

/// Rosson's answer 3, PRD §8 K-c: `required` and `server_read` go on for
/// the owner only. An agent (its own App, both switches on) gets
/// `owner_only` naming the card; an Admin login gets `owner_only`; the owner
/// without a confirm `vault_confirm_required`. Nothing is stored for any.
#[test]
fn only_the_owner_turns_required_or_server_read_on() {
    let f = fx("own", &ALL);
    full_switches(&f.ws);
    for body in [json!({ "app": f.app, "serverRead": true }), json!({ "app": f.app, "required": true })] {
        let (s, v) = call(SET, &agent(&f.ws), true, body.clone(), &[]);
        assert_eq!((s, code(&v)), (403, "owner_only"), "{v}");
        assert!(v["error"]["hint"].as_str().unwrap().contains("k2 permission request apps.recording app=jobs"), "{v}");
        let (s, v) = call(SET, &Who::Login { username: "ada".into(), role: Role::Admin }, true, body.clone(), &[]);
        assert_eq!((s, code(&v)), (403, "owner_only"), "an Admin never: {v}");
        let (s, v) = call(SET, &Who::Owner, true, body, &[]);
        assert_eq!((s, code(&v)), (403, "vault_confirm_required"), "{v}");
    }
    let (_, v) = call(READ, &Who::Owner, false, json!({}), &[("app", &f.app)]);
    assert_eq!(v["recording"], json!({}), "nothing stored: {v}");
}

/// The App's own agent narrows (start level none, required and server_read
/// off, fewer keep days) with no confirm; raising the start level is a
/// person's (`person_only`); keep days above the org's are refused (B23).
#[test]
fn the_apps_agent_narrows_but_never_widens() {
    let f = fx("narrow", &ALL);
    let a = agent(&f.ws);
    let (s, v) = call(SET, &a, true, json!({ "app": f.app, "startLevel": "member" }), &[]);
    assert_eq!((s, code(&v)), (403, "owner_only"), "without the switches the agent can't touch it: {v}");
    full_switches(&f.ws);
    let (s, v) = call(SET, &a, true, json!({ "app": f.app, "startLevel": "member" }), &[]);
    assert_eq!((s, code(&v)), (403, "person_only"), "{v}");
    let (s, v) = call(SET, &a, true, json!({ "app": f.app, "startLevel": "none", "required": false, "serverRead": false }), &[]);
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["recording"], json!({ "start_level": "none", "required": false, "server_read": false }));
    assert_eq!(v["delivered"], json!(false), "Sew hasn't confirmed it yet");
    k2_core::tool_hook::recording_set_policy("recording.audio_max_days", &json!(30), "owner").unwrap();
    let (s, v) = call(SET, &a, true, json!({ "app": f.app, "audioMaxDays": 31 }), &[]);
    assert_eq!((s, code(&v)), (400, "above_org_audio_max_days"), "{v}");
    let (s, v) = call(SET, &a, true, json!({ "app": f.app, "audioMaxDays": 7 }), &[]);
    assert_eq!(s, 200, "fewer days than the org's is a narrowing: {v}");
    let (s, v) = call(SET, &a, true, json!({ "app": f.app, "audioMaxDays": 20 }), &[]);
    assert_eq!((s, code(&v)), (403, "person_only"), "raising it again is a person's: {v}");
    let (s, v) = call(SET, &a, true, json!({ "app": f.app, "startLevel": "everyone" }), &[]);
    assert_eq!((s, code(&v)), (400, "bad_request"), "{v}");
}

/// B22: the home client takes no recording settings.
#[test]
fn the_home_client_takes_none() {
    let f = fx("home", &ALL);
    k2_core::tool_hook::set_home_client(Some(&f.app), "owner").unwrap();
    let (s, v) = call(SET, &Who::Owner, true, json!({ "app": f.app, "startLevel": "none" }), &[]);
    assert_eq!((s, code(&v)), (409, "home_client_cannot_govern"), "{v}");
}

#[test]
fn classify_reads_widening_from_the_settings_in_force() {
    let none = Map::new();
    let c = |v: J| Change::parse(&v).unwrap();
    assert_eq!(classify(&none, &c(json!({"startLevel": "none"})), None), Class::default());
    assert!(classify(&none, &c(json!({"startLevel": "moderator"})), None).widens);
    assert_eq!(classify(&none, &c(json!({"serverRead": true, "required": true})), None).owner_on, vec!["required", "server_read"]);
    let mut on = Map::new();
    on.insert("server_read".into(), json!(true));
    assert!(classify(&on, &c(json!({"serverRead": true})), None).owner_on.is_empty(), "already on");
    assert!(classify(&none, &c(json!({"audioMaxDays": 10})), Some(30)) == Class::default(), "fewer than the org's");
    assert!(classify(&none, &c(json!({"audioMaxDays": 10})), None).widens, "nothing known: a person decides");
    let mut ten = Map::new();
    ten.insert("audio_max_days".into(), json!(10));
    assert!(classify(&ten, &c(json!({"audioMaxDays": null})), Some(30)).widens, "back to the org's 30 from 10");
    assert!(Change::parse(&json!({"app": "x"})).is_err(), "a change must be named");
}
