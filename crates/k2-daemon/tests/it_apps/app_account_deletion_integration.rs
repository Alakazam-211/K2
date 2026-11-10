//! App account deletion and client logs (0.46.9), headless: the real
//! dispatcher in process on a temp `$HOME`, driven over loopback on the
//! daemon's main port with `Authorization: Bearer` (how a phone app calls;
//! the browser helper and the App's link carry the same pass).
//!
//! - `profile.deleteAccount`: a person's own session pass only; the recent
//!   sign-in rule (the pass's age, a phone's device sign-in age, the
//!   password and its lock); request mode (signed out, access off, the
//!   owner's list, cancel, complete, the agent switch, the grace sweep);
//!   immediate mode; scope `account` vs `app`; never the box login; what
//!   goes and what stays; History lines and the agent's note.
//! - `diagnostics.send`: auth, caps, redaction, rate limits, the reader's
//!   framed view, who may read, retention settings, and a deletion
//!   taking the person's bundles with it.

use crate::support::app_fixture;

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use app_fixture::*;
use serde_json::{json, Value};

struct Reply {
    status: u16,
    body: String,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("JSON ({e}): {:?}", self.body))
    }
    fn code(&self) -> String {
        self.json()["error"]["code"].as_str().map(str::to_string).unwrap_or_else(|| panic!("no error code: {}", self.body))
    }
}

/// One request with the pass as `Authorization: Bearer` (or none). A raw
/// body string, so a test can send one that is too large.
async fn call_raw(port: u16, method: &'static str, pq: &str, pass: Option<&str>, body: Option<String>) -> Reply {
    let (pq, pass) = (pq.to_string(), pass.map(str::to_string));
    tokio::task::spawn_blocking(move || {
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
        s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
        let auth = pass.map(|p| format!("Authorization: Bearer {p}\r\n")).unwrap_or_default();
        let req = match body {
            Some(b) => format!(
                "{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
                b.len()
            ),
            None if method == "POST" => format!("{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n{auth}Content-Length: 0\r\n\r\n"),
            None => format!("{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n{auth}\r\n"),
        };
        // A refused large body may close before we finish writing.
        let _ = s.write_all(req.as_bytes());
        let mut out = Vec::new();
        let _ = s.read_to_end(&mut out);
        let text = String::from_utf8_lossy(&out).to_string();
        let status = text.lines().next().and_then(|l| l.split_whitespace().nth(1)).and_then(|x| x.parse().ok()).unwrap_or(0);
        let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
        Reply { status, body }
    })
    .await
    .unwrap()
}

async fn call(port: u16, method: &'static str, pq: &str, pass: Option<&str>, body: Option<Value>) -> Reply {
    call_raw(port, method, pq, pass, body.map(|b| b.to_string())).await
}

async fn owner_post(port: u16, path: &str, body: Value) -> Value {
    let r = post(port, path, OWNER, body).await;
    assert_eq!(r.status, 200, "{path}: {}", r.body);
    r.json()
}

fn service(ws: &Ws, name: &str, port: u16) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let dir = ws.path.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    k2_core::published_services::insert(&conn, &ws.id, name, "node server.mjs", &dir.to_string_lossy(), port, "local", "stopped", "cmd", "")
        .expect("insert service")
        .id
}

async fn login(port: u16, body: Value) -> Reply {
    call(port, "POST", "/cli/skin/login", None, Some(body)).await
}

async fn login_pass(port: u16, username: &str, app: &str) -> String {
    let r = login(port, json!({ "username": username, "password": "s3cret-horse", "app": app })).await;
    assert_eq!(r.status, 200, "login {username} on {app}: {}", r.body);
    r.json()["token"].as_str().expect("token").to_string()
}

/// An App guest of `app` (role from the client template), password set.
async fn app_guest(port: u16, username: &str, app: &str) {
    let r = owner_post(port, "/cli/skin/roles/from-template", json!({ "template": "client", "app": app, "name": format!("{username}-role") })).await;
    let role = r["role"]["name"].as_str().unwrap().to_string();
    owner_post(port, "/cli/skin/users", json!({ "username": username, "app": app, "role": role, "email": format!("{username}@example.test") })).await;
    k2_daemon::test_harness::guest_with_own_password(username, "s3cret-horse");
}

/// Give an existing person `app` too (their own role there).
async fn also_on(port: u16, username: &str, app: &str) {
    let r = owner_post(port, "/cli/skin/roles/from-template", json!({ "template": "client", "app": app, "name": format!("{username}-2") })).await;
    let role = r["role"]["name"].as_str().unwrap().to_string();
    k2_core::skin::create_grant(&k2_core::skin::SkinGrantWrite {
        subject_kind: "principal".into(),
        subject_id: pid_of(username).expect("person"),
        kind: "app".into(),
        target_id: app.to_string(),
        role: Some(role),
        ..Default::default()
    })
    .expect("grant");
}

fn pid_of(username: &str) -> Option<String> {
    k2_core::skin::list_principals().unwrap().into_iter().find(|p| p.username == username).map(|p| p.id)
}

fn token_id(pass: &str) -> String {
    k2_core::skin::resolve_skin_token(pass).expect("a live pass").id
}

fn people_events(app: &str) -> Vec<(String, Value)> {
    let subject = k2_core::skin::guest_rooms::audit_subject(app);
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::deploy::audit::list_people_for(&conn, k2_core::deploy::audit::SUBJECT_APP, &subject, 200)
        .unwrap()
        .into_iter()
        .map(|r| (r.event, r.payload))
        .collect()
}

fn items(v: &Value, key: &str) -> Vec<String> {
    v[key].as_array().unwrap_or_else(|| panic!("{key} is a list: {v}")).iter().map(|x| x.as_str().expect("text").to_string()).collect()
}

fn kept_items(v: &Value) -> Vec<String> {
    v["kept"].as_array().expect("kept").iter().map(|k| k["item"].as_str().expect("item").to_string()).collect()
}

const DELETE: &str = "/cli/skin/account/delete";
const LOGS: &str = "/cli/skin/client-logs";

fn bundle(lines: Vec<String>) -> Value {
    json!({ "appVersion": "1.4.0", "os": "iOS 27.0", "kind": "crash", "message": "panicked at 'index out of bounds'", "lines": lines })
}

// ── request mode ─────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_person_asks_to_delete_their_account_and_the_owner_or_agent_completes_it() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let shop = seed_ws(&env, &format!("shop{tag}"));
    let other = seed_ws(&env, &format!("other{tag}"));
    let jobs = service(&shop, "jobs", 47_501);
    let notes: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let sink = notes.clone();
    k2_daemon::apps::alerts::set_sink(Some(Box::new(move |ws: &str, text: &str| sink.lock().unwrap().push((ws.to_string(), text.to_string())))));

    // The defaults, as the owner sees them.
    let s = get(port, "/cli/app/settings", OWNER, &format!("app={jobs}")).await;
    assert_eq!(s.status, 200, "{}", s.body);
    let s = s.json();
    assert_eq!(s["accountDeletion"], json!({ "mode": "request", "graceDays": 30, "set": false }));
    assert_eq!(s["clientLogs"], json!({ "keepDays": 14, "set": false }));

    let dana = format!("dana{tag}");
    app_guest(port, &dana, &jobs).await;
    let pass = login_pass(port, &dana, &jobs).await;

    // Who may call: a person's own session pass, by POST.
    let r = call(port, "POST", DELETE, None, Some(json!({}))).await;
    assert_eq!((r.status, r.code()), (401, "pass_required".to_string()), "{}", r.body);
    let r = call(port, "POST", DELETE, Some(OWNER), Some(json!({}))).await;
    assert_eq!((r.status, r.code()), (403, "guest_required".to_string()), "{}", r.body);
    let r = call(port, "GET", DELETE, Some(&pass), None).await;
    assert_eq!(r.status, 405, "{}", r.body);
    let r = call(port, "POST", DELETE, Some(&pass), Some(json!({ "username": "someone-else" }))).await;
    assert_eq!((r.status, r.code()), (400, "usage".to_string()), "the person comes from the pass: {}", r.body);

    // An old sign-in needs the password; a wrong one is bad_login.
    k2_core::skin::app_pairing::age_session_for_test(&token_id(&pass), k2_core::skin::app_pairing::FRESH_SIGNIN_SECS + 60);
    let r = call(port, "POST", DELETE, Some(&pass), Some(json!({}))).await;
    assert_eq!((r.status, r.code()), (401, "reauth_required".to_string()), "{}", r.body);
    let r = call(port, "POST", DELETE, Some(&pass), Some(json!({ "password": "wrong-horse" }))).await;
    assert_eq!((r.status, r.code()), (401, "bad_login".to_string()), "{}", r.body);
    assert!(pid_of(&dana).is_some(), "a refusal deletes nothing");

    let r = call(port, "POST", DELETE, Some(&pass), Some(json!({ "password": "s3cret-horse" }))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert_eq!((v["mode"].as_str(), v["state"].as_str(), v["scope"].as_str()), (Some("request"), Some("requested"), Some("account")), "{v}");
    let request_id = v["requestId"].as_str().expect("requestId").to_string();
    assert!(request_id.starts_with("adr_"), "{request_id}");
    let due = v["dueAt"].as_i64().expect("dueAt") - v["requestedAt"].as_i64().expect("requestedAt");
    assert_eq!(due, 30 * 86_400, "the default grace period");
    assert_eq!(items(&v, "done"), ["sessions", "devices", "push_devices", "pairings"]);
    let pending = items(&v, "pending");
    for item in ["login", "app_access", "profile", "email", "client_logs"] {
        assert!(pending.contains(&item.to_string()), "{item} goes at completion: {pending:?}");
    }
    let kept = kept_items(&v);
    for item in ["thread_posts", "room_files", "chat_messages", "app_data", "history"] {
        assert!(kept.contains(&item.to_string()), "{item} is kept: {kept:?}");
    }
    assert!(!kept.contains(&"login".to_string()), "scope account: the login isn't kept");
    assert!(v["kept"][0]["why"].as_str().is_some_and(|w| w.len() > 20), "each kept item says why");

    // Signed out everywhere, and the login no longer opens the App.
    assert_eq!(call(port, "GET", "/cli/skin/me", Some(&pass), None).await.status, 401, "the pass ended");
    let r = login(port, json!({ "username": dana, "password": "s3cret-horse", "app": jobs })).await;
    assert_ne!(r.status, 200, "access is off while the request waits: {}", r.body);
    assert!(pid_of(&dana).is_some(), "the login itself waits for completion");

    // The owner sees it; History and the agent's note say so.
    let l = get(port, "/cli/app/account-deletion", OWNER, &format!("app={jobs}")).await;
    assert_eq!(l.status, 200, "{}", l.body);
    let l = l.json();
    assert_eq!(l["pending"].as_array().map(Vec::len), Some(1), "{l}");
    assert_eq!(l["pending"][0]["username"].as_str(), Some(dana.as_str()));
    assert_eq!(l["pending"][0]["scope"].as_str(), Some("account"));
    assert!(people_events(&jobs).iter().any(|(e, p)| e == "app_user_deletion_requested" && p["requestId"] == json!(request_id)), "History");
    assert!(
        notes.lock().unwrap().iter().any(|(ws, t)| ws == &shop.id && t.contains("asked to delete their account") && t.contains(&dana)),
        "the App's agent is told: {:?}",
        notes.lock().unwrap()
    );

    // Cancel by username: access comes back.
    let r = owner_post(port, "/cli/app/account-deletion/cancel", json!({ "app": jobs, "request": dana })).await;
    assert_eq!(r["restored"], 1, "{r}");
    assert!(people_events(&jobs).iter().any(|(e, _)| e == "app_user_deletion_cancelled"));
    let pass = login_pass(port, &dana, &jobs).await;

    // A fresh sign-in needs no password. Ask again.
    let r = call(port, "POST", DELETE, Some(&pass), Some(json!({}))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let request_id = r.json()["requestId"].as_str().unwrap().to_string();

    // Completing: another workspace's agent never; this App's agent needs
    // "Let this agent manage its Apps' users".
    let r = post(port, "/cli/app/account-deletion/complete", &passport(&other), json!({ "app": jobs, "request": request_id })).await;
    assert_eq!((r.status, r.code()), (403, "outside_workspace".to_string()), "{}", r.body);
    allow_build_only(&shop, true);
    let r = post(port, "/cli/app/account-deletion/complete", &passport(&shop), json!({ "app": jobs, "request": request_id })).await;
    assert_eq!(r.status, 403, "the users switch is off: {}", r.body);
    allow_manage(&shop, true);
    let r = post(port, "/cli/app/account-deletion/complete", &passport(&shop), json!({ "app": jobs, "request": request_id })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert_eq!((v["state"].as_str(), v["scope"].as_str(), v["requestId"].as_str()), (Some("deleted"), Some("account"), Some(request_id.as_str())));
    let done = items(&v, "done");
    for item in ["login", "app_access", "sessions", "profile", "email", "read_state", "wallet", "client_logs"] {
        assert!(done.contains(&item.to_string()), "{item} went: {done:?}");
    }
    assert!(pid_of(&dana).is_none(), "the login is gone");
    let l = get(port, "/cli/app/account-deletion", OWNER, &format!("app={jobs}")).await.json();
    assert_eq!(l["pending"], json!([]), "nothing pending");
    assert!(people_events(&jobs).iter().any(|(e, p)| e == "app_user_deleted" && p["username"] == json!(dana)), "History keeps the line");
    let r = post(port, "/cli/app/account-deletion/complete", OWNER, json!({ "app": jobs, "request": request_id })).await;
    assert_eq!((r.status, r.code()), (404, "not_found".to_string()), "{}", r.body);

    k2_daemon::apps::alerts::set_sink(None);
    drop(d);
    drop(env);
}

// ── immediate mode, scope app, the grace sweep, phones, the box login ─

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn immediate_mode_scope_app_the_grace_sweep_and_a_phones_sign_in() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let shop = seed_ws(&env, &format!("shop{tag}"));
    let jobs = service(&shop, "jobs", 47_511);
    let crm = service(&shop, "crm", 47_512);

    // Settings: checked, then immediate.
    for (body, why) in [
        (json!({ "app": jobs, "accountDeletion": "later" }), "a mode that isn't one"),
        (json!({ "app": jobs, "deletionGraceDays": 0 }), "grace below 1"),
        (json!({ "app": jobs, "deletionGraceDays": 91 }), "grace above 90"),
        (json!({ "app": jobs, "clientLogsDays": 100 }), "retention above 90"),
        (json!({ "app": jobs }), "nothing to set"),
        (json!({ "app": "host", "accountDeletion": "immediate" }), "never the box login"),
    ] {
        let r = post(port, "/cli/app/settings", OWNER, body).await;
        assert!(r.status == 400 || r.status == 403, "{why}: {}", r.body);
    }
    let s = owner_post(port, "/cli/app/settings", json!({ "app": jobs, "accountDeletion": "immediate", "deletionGraceDays": "7d" })).await;
    assert_eq!(s["accountDeletion"], json!({ "mode": "immediate", "graceDays": 7, "set": true }));
    assert!(people_events(&jobs).iter().any(|(e, _)| e == "app_user_deletion_settings"));

    // Scope app: Eli also uses crm, so only jobs' access goes.
    let eli = format!("eli{tag}");
    app_guest(port, &eli, &jobs).await;
    also_on(port, &eli, &crm).await;
    let pass = login_pass(port, &eli, &jobs).await;
    let r = call(port, "POST", DELETE, Some(&pass), Some(json!({}))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert_eq!((v["mode"].as_str(), v["state"].as_str(), v["scope"].as_str()), (Some("immediate"), Some("deleted"), Some("app")), "{v}");
    assert_eq!(kept_items(&v)[0], "login", "the login stays, first in the list");
    assert!(!items(&v, "done").contains(&"login".to_string()));
    assert!(pid_of(&eli).is_some(), "the login stays");
    login_pass(port, &eli, &crm).await;
    let r = login(port, json!({ "username": eli, "password": "s3cret-horse", "app": jobs })).await;
    assert_ne!(r.status, 200, "jobs is gone for Eli: {}", r.body);

    // Scope account, with client logs that go too.
    let fay = format!("fay{tag}");
    app_guest(port, &fay, &jobs).await;
    let pass = login_pass(port, &fay, &jobs).await;
    let r = call(port, "POST", LOGS, Some(&pass), Some(bundle(vec!["boot".into()]))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let listed = get(port, "/cli/app/client-logs", OWNER, &format!("app={jobs}&user={fay}")).await.json();
    assert_eq!(listed["count"], 1, "{listed}");
    let r = call(port, "POST", DELETE, Some(&pass), Some(json!({}))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["scope"], "account");
    assert!(pid_of(&fay).is_none());
    let listed = get(port, "/cli/app/client-logs", OWNER, &format!("app={jobs}&user={fay}")).await.json();
    assert_eq!(listed["count"], 0, "their client logs went: {listed}");

    // A phone: its pass is fresh after a refresh, but its sign-in is what
    // counts.
    let hal = format!("hal{tag}");
    app_guest(port, &hal, &jobs).await;
    let r = login(port, json!({ "username": hal, "password": "s3cret-horse", "app": jobs, "device": { "id": "phone-1", "name": "Phone" } })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let phone = r.json()["token"].as_str().unwrap().to_string();
    k2_core::skin::account_deletion::age_device_sign_in_for_test(&token_id(&phone), k2_core::skin::app_pairing::FRESH_SIGNIN_SECS + 60);
    let r = call(port, "POST", DELETE, Some(&phone), Some(json!({}))).await;
    assert_eq!((r.status, r.code()), (401, "reauth_required".to_string()), "{}", r.body);
    let r = call(port, "POST", DELETE, Some(&phone), Some(json!({ "password": "s3cret-horse" }))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(pid_of(&hal).is_none());

    // The grace sweep completes a request whose time came.
    owner_post(port, "/cli/app/settings", json!({ "app": jobs, "accountDeletion": "request" })).await;
    let gus = format!("gus{tag}");
    app_guest(port, &gus, &jobs).await;
    let pass = login_pass(port, &gus, &jobs).await;
    let r = call(port, "POST", DELETE, Some(&pass), Some(json!({}))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert_eq!(v["dueAt"].as_i64().unwrap() - v["requestedAt"].as_i64().unwrap(), 7 * 86_400, "the App's grace period");
    assert_eq!(tokio::task::spawn_blocking(k2_daemon::app_account_deletion::sweep).await.unwrap(), 0, "not due yet");
    assert!(pid_of(&gus).is_some());
    k2_core::skin::account_deletion::make_due_for_test(v["requestId"].as_str().unwrap());
    assert_eq!(tokio::task::spawn_blocking(k2_daemon::app_account_deletion::sweep).await.unwrap(), 1);
    assert!(pid_of(&gus).is_none(), "the grace period completed it");
    assert!(people_events(&jobs).iter().any(|(e, p)| e == "app_user_deleted" && p["username"] == json!(gus) && p["requestId"].is_string()));

    // Never the box login.
    let ivy = format!("ivy{tag}");
    owner_post(port, "/cli/skin/users", json!({ "username": ivy })).await;
    k2_daemon::test_harness::guest_with_own_password(&ivy, "s3cret-horse");
    let r = login(port, json!({ "username": ivy, "password": "s3cret-horse" })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let host_pass = r.json()["token"].as_str().unwrap().to_string();
    let r = call(port, "POST", DELETE, Some(&host_pass), Some(json!({}))).await;
    assert_eq!((r.status, r.code()), (403, "not_for_host".to_string()), "{}", r.body);
    assert!(pid_of(&ivy).is_some());

    drop(d);
    drop(env);
}

// ── client logs ──────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn client_logs_are_capped_redacted_rate_limited_and_read_by_the_apps_own_people() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let shop = seed_ws(&env, &format!("shop{tag}"));
    let other = seed_ws(&env, &format!("other{tag}"));
    let jobs = service(&shop, "jobs", 47_521);
    k2_daemon::app_client_logs::reset_limits_for_tests();
    let kim = format!("kim{tag}");
    app_guest(port, &kim, &jobs).await;
    let pass = login_pass(port, &kim, &jobs).await;

    // Auth and shape.
    let r = call(port, "POST", LOGS, None, Some(bundle(vec![]))).await;
    assert_eq!((r.status, r.code()), (401, "pass_required".to_string()), "{}", r.body);
    let r = call(port, "POST", LOGS, Some(OWNER), Some(bundle(vec![]))).await;
    assert_eq!((r.status, r.code()), (403, "guest_required".to_string()), "{}", r.body);
    assert_eq!(call(port, "GET", LOGS, Some(&pass), None).await.status, 405);
    for (body, why) in [
        (json!({ "os": "iOS", "message": "m" }), "no appVersion"),
        (json!({ "appVersion": "1", "os": "iOS" }), "no message or lines"),
        (json!({ "appVersion": "1", "os": "iOS", "message": "m", "kind": "trace" }), "an unknown kind"),
        (json!({ "appVersion": "1", "os": "iOS", "message": "m", "token": "x" }), "an unknown field"),
        (json!({ "appVersion": "1", "os": "iOS", "lines": "one line" }), "lines isn't a list"),
    ] {
        let r = call(port, "POST", LOGS, Some(&pass), Some(body)).await;
        assert_eq!((r.status, r.code()), (400, "usage".to_string()), "{why}: {}", r.body);
    }
    let huge = json!({ "appVersion": "1", "os": "iOS", "lines": vec!["x".repeat(900); 200] }).to_string();
    assert!(huge.len() > 128 * 1024);
    let r = call_raw(port, "POST", LOGS, Some(&pass), Some(huge)).await;
    assert_eq!(r.status, 413, "{}", r.body);

    // Redacted on the server; the newest lines kept.
    let secret = "k2skn_0123456789abcdef0123456789abcdef";
    let r = call(
        port,
        "POST",
        LOGS,
        Some(&pass),
        Some(bundle(vec![format!("GET /cli/skin/me Authorization: Bearer {secret}"), "password=hunter2hunter2".into(), "screen opened".into()])),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert!(v["id"].as_str().is_some_and(|i| i.starts_with("cl_")), "{v}");
    assert!(v["redacted"].as_i64().unwrap() >= 2, "{v}");
    assert_eq!((v["lines"].as_i64(), v["truncated"].as_bool()), (Some(3), Some(false)));
    let many: Vec<String> = (0..1_200).map(|i| format!("line {i}")).collect();
    let r = call(port, "POST", LOGS, Some(&pass), Some(bundle(many))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!((r.json()["lines"].as_i64(), r.json()["truncated"].as_bool()), (Some(1_000), Some(true)));

    // The reader: framed, never the secret, filtered by person.
    let l = get(port, "/cli/app/client-logs", OWNER, &format!("app={jobs}")).await;
    assert_eq!(l.status, 200, "{}", l.body);
    let l = l.json();
    assert_eq!(l["count"], 2, "{l}");
    let framed = l["framed"].as_str().unwrap();
    assert!(framed.starts_with("<untrusted-app-output"), "{framed}");
    assert!(framed.contains("screen opened") && framed.contains("line 1199"));
    assert!(!framed.contains(secret) && !framed.contains("hunter2"), "redacted: {framed}");
    assert!(!l.to_string().contains(secret));
    assert_eq!(l["bundles"][0]["username"].as_str(), Some(kim.as_str()));
    assert_eq!(get(port, "/cli/app/client-logs", OWNER, &format!("app={jobs}&user=nobody{tag}")).await.json()["count"], 0);
    assert_eq!(get(port, "/cli/app/client-logs", OWNER, &format!("app={jobs}&kind=log")).await.json()["count"], 0);
    let r = get(port, "/cli/app/client-logs", &passport(&other), &format!("app={jobs}")).await;
    assert_eq!((r.status, r.code()), (403, "outside_workspace".to_string()), "{}", r.body);
    let r = get(port, "/cli/app/client-logs", &passport(&shop), &format!("app={jobs}")).await;
    assert_eq!(r.status, 200, "the App's own agent reads them: {}", r.body);

    // 20 per person an hour (two sent above).
    for i in 0..18 {
        let r = call(port, "POST", LOGS, Some(&pass), Some(bundle(vec![format!("n {i}")]))).await;
        assert_eq!(r.status, 200, "send {i}: {}", r.body);
    }
    let r = call(port, "POST", LOGS, Some(&pass), Some(bundle(vec!["one more".into()]))).await;
    assert_eq!((r.status, r.code()), (429, "rate_limited".to_string()), "{}", r.body);
    assert!(r.json()["error"]["retryAfterSecs"].as_i64().is_some_and(|s| s > 0 && s <= 3600));
    // Another person of the same App still sends.
    let lou = format!("lou{tag}");
    app_guest(port, &lou, &jobs).await;
    let lou_pass = login_pass(port, &lou, &jobs).await;
    assert_eq!(call(port, "POST", LOGS, Some(&lou_pass), Some(bundle(vec!["hi".into()]))).await.status, 200);

    // Retention: the owner sets it; an agent without the users switch can't.
    let r = post(port, "/cli/app/settings", &passport(&shop), json!({ "app": jobs, "clientLogsDays": 7 })).await;
    assert_eq!(r.status, 403, "{}", r.body);
    let s = owner_post(port, "/cli/app/settings", json!({ "app": jobs, "clientLogsDays": 7 })).await;
    assert_eq!(s["clientLogs"], json!({ "keepDays": 7, "set": true }));
    assert_eq!(get(port, "/cli/app/client-logs", OWNER, &format!("app={jobs}")).await.json()["keepDays"], 7);

    drop(d);
    drop(env);
}
