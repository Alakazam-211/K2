//! Must-change-password at next sign-in for App users (0.46.6), headless:
//! the real dispatcher in process on a temp `$HOME`, over loopback.
//!
//! - The owner's typed password and K2's generated one turn the flag on
//!   (the users list and the set's reply say so); the person's own
//!   password turns it off.
//! - With the flag on, `/cli/skin/login` gives no pass and no session on
//!   any path: JSON (403 `password_change_required` + change token), a
//!   device sign-in (the same, no refresh token, no device row) and the
//!   form (303 to K2's set-password page, token in the fragment). No
//!   `Set-Cookie` anywhere.
//! - The change token: only the newest works, only on the reset route,
//!   never as a pass, never the temporary password, once.
//! - Then a normal sign-in.
//!
//! Never the real `~/.k2`, never the keychain.

#[path = "support/app_fixture.rs"]
mod app_fixture;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use app_fixture::*;
use serde_json::{json, Value};

struct Raw {
    status: u16,
    head: String,
    body: String,
}

impl Raw {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("JSON ({e}): {:?}", self.body))
    }
    fn header(&self, name: &str) -> Option<String> {
        self.head.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim().eq_ignore_ascii_case(name).then(|| v.trim().to_string())
        })
    }
}

/// One request with the response head kept (`ct`: its Content-Type).
fn raw(port: u16, method: &str, pq: &str, ct: &str, body: &str, extra: &str) -> Raw {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    let req = format!(
        "{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n{extra}Content-Type: {ct}\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).expect("read");
    let text = String::from_utf8_lossy(&out).to_string();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or_else(|| panic!("no head: {text:?}"));
    let status = head.split_whitespace().nth(1).and_then(|x| x.parse().ok()).unwrap_or_else(|| panic!("no status: {head}"));
    Raw { status, head: head.to_string(), body: body.to_string() }
}

async fn login_json(port: u16, body: Value) -> Raw {
    let b = body.to_string();
    tokio::task::spawn_blocking(move || raw(port, "POST", "/cli/skin/login", "application/json", &b, "")).await.unwrap()
}

async fn login_form(port: u16, form: String) -> Raw {
    tokio::task::spawn_blocking(move || raw(port, "POST", "/cli/skin/login", "application/x-www-form-urlencoded", &form, ""))
        .await
        .unwrap()
}

async fn reset(port: u16, token: &str, password: &str) -> Raw {
    let b = json!({ "token": token, "password": password }).to_string();
    tokio::task::spawn_blocking(move || raw(port, "POST", "/cli/skin/password/reset", "application/json", &b, "")).await.unwrap()
}

async fn owner_post(port: u16, path: &str, body: Value) -> Value {
    let r = post(port, path, OWNER, body).await;
    assert_eq!(r.status, 200, "{path}: {}", r.body);
    r.json()
}

async fn user_row(port: u16, username: &str) -> Value {
    let r = get(port, "/cli/skin/users", OWNER, "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    r.json()["users"].as_array().expect("users").iter().find(|u| u["username"] == username).cloned().unwrap_or_else(|| panic!("no {username}"))
}

fn app_row(ws: &Ws, name: &str) -> String {
    let dir = ws.path.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::published_services::insert(&conn, &ws.id, name, "node s.mjs", &dir.to_string_lossy(), 47_801, "local", "stopped", "cmd", "")
        .expect("insert service")
        .id
}

fn device_rows(username: &str) -> i64 {
    let db = rusqlite::Connection::open(k2_core::paths::k2_home().join("skin.db")).expect("skin.db");
    db.query_row(
        "SELECT COUNT(*) FROM device_logins d JOIN principals p ON p.id = d.principal_id WHERE p.username = ?1",
        [username],
        |r| r.get(0),
    )
    .unwrap_or_else(|e| panic!("device rows: {e}"))
}

/// The must-change answer on the JSON paths: 403, the code, a change
/// token, no pass, no cookie.
fn assert_must_change(r: &Raw, what: &str) -> String {
    assert_eq!(r.status, 403, "{what}: {}", r.body);
    assert!(r.header("set-cookie").is_none(), "{what}: no cookie: {}", r.head);
    assert!(!r.body.contains("k2skn_") && !r.body.contains("k2skr_"), "{what}: no pass or refresh: {}", r.body);
    let v = r.json();
    assert_eq!(v["error"]["code"], "password_change_required", "{what}: {v}");
    assert!(v["error"]["hint"].as_str().expect("hint").contains("@k2/server"), "{what}: the hint tells an older App to update: {v}");
    assert_eq!((v["ok"].clone(), v["mustChangePassword"].clone()), (json!(false), json!(true)), "{what}: {v}");
    assert!(v.get("token").is_none() && v.get("refresh").is_none(), "{what}: {v}");
    assert_eq!(v["changePath"], "/cli/skin/password/set", "{what}: {v}");
    assert!(v["changeExpiresAt"].as_i64().is_some(), "{what}: {v}");
    let tok = v["changeToken"].as_str().expect("changeToken").to_string();
    assert!(tok.starts_with("k2chg_") && tok.len() == 6 + 64, "{what}: {tok}");
    tok
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_owner_set_password_gets_no_pass_on_any_path_until_the_person_changes_it() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let ws = seed_ws(&env, &format!("mc{tag}"));
    let app = app_row(&ws, &format!("shop{tag}"));
    // Stay signed in on, so a device sign-in would mint a refresh token.
    k2_core::skin::app_login::set_settings(&app, None, Some(30), "owner-token").expect("stay signed in");
    let role = format!("r{tag}");
    owner_post(port, "/cli/skin/roles", json!({ "name": role, "roomAccess": [{ "handle": ws.handle, "caps": ["thread:read"] }] })).await;
    let dana = format!("dana{tag}");
    let temp = "owner-temp-1";
    let v = owner_post(port, "/cli/skin/users", json!({ "username": dana, "app": app, "role": role, "password": temp })).await;
    assert_eq!(v["passwordMustChange"], json!(true), "the owner's typed password: {v}");
    assert_eq!(user_row(port, &dana).await["passwordMustChange"], json!(true));

    // JSON.
    let t_json = assert_must_change(&login_json(port, json!({ "username": dana, "password": temp, "app": app })).await, "json");
    // By email too? (no email here) A wrong password is still the plain 401.
    let r = login_json(port, json!({ "username": dana, "password": "wrong-pass-1", "app": app })).await;
    assert_eq!(r.status, 401, "{}", r.body);
    assert!(!r.body.contains("k2chg_"), "a wrong password never gets a change token: {}", r.body);
    // Another App this person holds no grant for: the plain 401 too.
    let r = login_json(port, json!({ "username": dana, "password": temp, "app": format!("nope{tag}") })).await;
    assert_eq!(r.status, 401, "{}", r.body);
    assert!(!r.body.contains("k2chg_"), "{}", r.body);

    // A device sign-in: the same answer, no refresh token, no device row.
    let r = login_json(port, json!({ "username": dana, "password": temp, "app": app, "device": { "id": "phone-1", "name": "Phone" } })).await;
    let t_dev = assert_must_change(&r, "device");
    assert_eq!(device_rows(&dana), 0, "no device sign-in");

    // The form: 303 to K2's set-password page, the token in the fragment.
    let r = login_form(port, format!("username={dana}&password={temp}&app={app}&next=/home")).await;
    assert_eq!(r.status, 303, "{}", r.body);
    assert!(r.header("set-cookie").is_none(), "no cookie: {}", r.head);
    let loc = r.header("location").expect("Location");
    let t_form = loc.strip_prefix("/cli/skin/password/set#").unwrap_or_else(|| panic!("K2's page: {loc}")).to_string();
    assert!(t_form.starts_with("k2chg_"), "{loc}");
    assert!(!r.body.contains("k2skn_"), "{}", r.body);

    // With a front door, the answer carries K2's page with the token.
    let door = format!("https://door{tag}.example.test");
    owner_post(port, "/cli/skin/front-door", json!({ "mode": "direct", "url": door, "apply": false })).await;
    let r = login_json(port, json!({ "username": dana, "password": temp, "app": app })).await;
    let t_last = assert_must_change(&r, "json with a front door");
    assert_eq!(r.json()["changeUrl"], json!(format!("{door}/cli/skin/password/set#{t_last}")), "{}", r.body);

    // Only the newest token works; the change token is no pass.
    for old in [&t_json, &t_dev, &t_form] {
        let r = reset(port, old, "danas-own-pw-1").await;
        assert_eq!(r.status, 401, "an older change token is void: {}", r.body);
    }
    let r = get(port, "/cli/skin/profile", &t_last, "").await;
    assert_eq!((r.status, r.code()), (403, "guest_required".to_string()), "not a person's pass: {}", r.body);
    assert!(!r.body.contains(&dana), "{}", r.body);
    let r = login_json(port, json!({ "username": dana, "password": t_last, "app": app })).await;
    assert_eq!(r.status, 401, "not a password: {}", r.body);

    // Never the temporary password; then the person's own, once.
    let r = reset(port, &t_last, temp).await;
    assert_eq!(r.status, 400, "{}", r.body);
    assert_eq!(r.json()["error"]["code"], "password_unchanged", "{}", r.body);
    let r = reset(port, &t_last, "danas-own-pw-1").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json(), json!({ "ok": true }), "no pass from a change: {}", r.body);
    let r = reset(port, &t_last, "danas-own-pw-2").await;
    assert_eq!(r.status, 401, "one use: {}", r.body);
    assert_eq!(user_row(port, &dana).await["passwordMustChange"], json!(false));

    // A normal sign-in now, on every path.
    let r = login_json(port, json!({ "username": dana, "password": temp, "app": app })).await;
    assert_eq!(r.status, 401, "the temporary password is gone: {}", r.body);
    let r = login_json(port, json!({ "username": dana, "password": "danas-own-pw-1", "app": app })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.json()["token"].as_str().is_some_and(|t| t.starts_with("k2skn_")), "{}", r.body);
    assert!(r.header("set-cookie").is_some(), "{}", r.head);
    let r = login_json(port, json!({ "username": dana, "password": "danas-own-pw-1", "app": app, "device": { "id": "phone-1", "name": "Phone" } })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.json()["refresh"].as_str().is_some(), "{}", r.body);
    let r = login_form(port, format!("username={dana}&password=danas-own-pw-1&app={app}&next=/home")).await;
    assert_eq!(r.status, 302, "{}", r.body);
    assert!(r.header("set-cookie").is_some(), "{}", r.head);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn generated_and_owner_typed_passwords_set_the_flag_and_the_persons_own_clears_it() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let ws = seed_ws(&env, &format!("mg{tag}"));
    let role = format!("r{tag}");
    owner_post(port, "/cli/skin/roles", json!({ "name": role, "roomAccess": [{ "handle": ws.handle, "caps": ["thread:read"] }] })).await;
    let erin = format!("erin{tag}");
    owner_post(port, "/cli/skin/users", json!({ "username": erin })).await;
    owner_post(port, "/cli/skin/roles/assign", json!({ "username": erin, "role": role })).await;

    // K2 generates it: on. The box login (no app) gets the same answer.
    let v = owner_post(port, "/cli/skin/users/password", json!({ "username": erin, "generate": true })).await;
    assert_eq!(v["passwordMustChange"], json!(true), "{v}");
    let generated = v["generatedPassword"].as_str().expect("generated").to_string();
    let tok = assert_must_change(&login_json(port, json!({ "username": erin, "password": generated })).await, "generated, box login");

    // The person's own password (a change, here the token): off.
    let r = reset(port, &tok, "erins-own-pw-1").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(user_row(port, &erin).await["passwordMustChange"], json!(false));
    let r = login_json(port, json!({ "username": erin, "password": "erins-own-pw-1" })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let pass = r.json()["token"].as_str().expect("pass").to_string();

    // password.change (the person, signed in) stays off.
    let r = post(port, "/cli/skin/password/change", &pass, json!({ "oldPassword": "erins-own-pw-1", "password": "erins-own-pw-2" })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(user_row(port, &erin).await["passwordMustChange"], json!(false));
    let r = login_json(port, json!({ "username": erin, "password": "erins-own-pw-2" })).await;
    assert_eq!(r.status, 200, "{}", r.body);

    // The owner types one: on again. Clearing the password: off.
    let v = owner_post(port, "/cli/skin/users/password", json!({ "username": erin, "password": "owner-typed-2" })).await;
    assert_eq!(v["passwordMustChange"], json!(true), "{v}");
    assert_must_change(&login_json(port, json!({ "username": erin, "password": "owner-typed-2" })).await, "owner typed");
    let v = owner_post(port, "/cli/skin/users/password", json!({ "username": erin })).await;
    assert_eq!((v["hasPassword"].clone(), v["passwordMustChange"].clone()), (json!(false), json!(false)), "{v}");

    // An owner reset link the person uses: off.
    owner_post(port, "/cli/skin/users/password", json!({ "username": erin, "password": "owner-typed-3" })).await;
    let r = post(port, "/cli/skin/users/reset-link", OWNER, json!({ "username": erin, "code": true })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    let link = v["code"]
        .as_str()
        .map(str::to_string)
        .or_else(|| v["link"].as_str().and_then(|l| l.split('#').nth(1)).map(str::to_string))
        .expect("a code");
    let r = reset(port, &link, "erins-own-pw-3").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(user_row(port, &erin).await["passwordMustChange"], json!(false));
    let r = login_json(port, json!({ "username": erin, "password": "erins-own-pw-3" })).await;
    assert_eq!(r.status, 200, "{}", r.body);
}
