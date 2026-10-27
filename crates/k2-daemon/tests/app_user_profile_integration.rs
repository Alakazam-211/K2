//! App user profile (prd-app-user-profile-v1, 0.46.5), headless: the real
//! dispatcher in process on a temp `$HOME`, driven over loopback on the
//! daemon's main port with `Authorization: Bearer` (how a phone app calls;
//! the browser helper and the App's link carry the same pass).
//!
//! - P2: profile.get / update / setImage / clearImage: only a person's own
//!   session pass; this App's role only; the picture only when asked; names
//!   cleaned; per-App settings (`edit_off` writes nothing); an agent's login
//!   reads only.
//! - P5: sessions: this App's only, opaque ids, revoke (another App's id is
//!   404), sign out others (the caller stays), the owner's view and revoke.
//! - P6: profile settings: the owner; an agent without the App-user switch
//!   is refused.
//! - P0: wrong current passwords 429 after 5 (never the login lock);
//!   `stay: true`; the 256-byte cap; a used owner reset clears the lock
//!   and voids an App's leftover reset link; the reset door's own budget.

#![cfg(unix)]

#[path = "support/app_fixture.rs"]
mod app_fixture;

use std::io::{Read, Write};
use std::time::Duration;

use app_fixture::*;
use serde_json::{json, Value};

struct Reply {
    status: u16,
    head: String,
    body: String,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("JSON ({e}): {:?}", self.body))
    }
    fn code(&self) -> String {
        self.json()["error"]["code"].as_str().map(str::to_string).unwrap_or_default()
    }
}

/// One request with the pass as `Authorization: Bearer` (or none) and any
/// extra header lines.
async fn call(port: u16, method: &'static str, pq: &str, pass: Option<&str>, body: Option<Value>, extra: &str) -> Reply {
    let (pq, pass, extra) = (pq.to_string(), pass.map(str::to_string), extra.to_string());
    tokio::task::spawn_blocking(move || {
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
        s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
        let auth = pass.map(|p| format!("Authorization: Bearer {p}\r\n")).unwrap_or_default();
        let req = match body {
            Some(b) => {
                let b = b.to_string();
                format!(
                    "{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n{auth}{extra}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
                    b.len()
                )
            }
            None if method == "POST" => format!("{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n{auth}{extra}Content-Length: 0\r\n\r\n"),
            None => format!("{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n{auth}{extra}\r\n"),
        };
        s.write_all(req.as_bytes()).unwrap();
        let mut out = Vec::new();
        let _ = s.read_to_end(&mut out);
        let text = String::from_utf8_lossy(&out).to_string();
        let status = text.lines().next().and_then(|l| l.split_whitespace().nth(1)).and_then(|x| x.parse().ok()).unwrap_or(0);
        let (head, body) = text.split_once("\r\n\r\n").map(|(h, b)| (h.to_string(), b.to_string())).unwrap_or_default();
        Reply { status, head, body }
    })
    .await
    .unwrap()
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

const SAFARI: &str = "User-Agent: Mozilla/5.0 (Macintosh; Intel Mac OS X 14_5) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Safari/605.1.15\r\n";

async fn login(port: u16, username: &str, password: &str, app: &str, ua: &str) -> Reply {
    call(port, "POST", "/cli/skin/login", None, Some(json!({ "username": username, "password": password, "app": app })), ua).await
}

async fn login_pass(port: u16, username: &str, app: &str, ua: &str) -> String {
    let r = login(port, username, "s3cret-horse", app, ua).await;
    assert_eq!(r.status, 200, "login {username} on {app}: {}", r.body);
    r.json()["token"].as_str().expect("token").to_string()
}

/// An App guest of `app` (role from the client template), password set.
async fn app_guest(port: u16, username: &str, app: &str) {
    let r = owner_post(port, "/cli/skin/roles/from-template", json!({ "template": "client", "app": app, "name": format!("{username}-role") })).await;
    let role = r["role"]["name"].as_str().unwrap().to_string();
    owner_post(port, "/cli/skin/users", json!({ "username": username, "app": app, "role": role, "email": format!("{username}@example.test") })).await;
    owner_post(port, "/cli/skin/users/password", json!({ "username": username, "password": "s3cret-horse" })).await;
}

fn png(w: u32, h: u32) -> String {
    use base64::Engine as _;
    let img = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([(x % 256) as u8, (y % 256) as u8, 7, 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img).write_to(&mut out, image::ImageFormat::Png).unwrap();
    format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(out.into_inner()))
}

// ── P2 + P5 + P6 ─────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_person_reads_and_edits_their_own_profile_and_sessions_in_one_app() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let shop = seed_ws(&env, &format!("shop{tag}"));
    let crm_ws = seed_ws(&env, &format!("crm{tag}"));
    let jobs = service(&shop, "jobs", 47_401);
    let crm = service(&crm_ws, "crm", 47_402);
    let dana = format!("dana{tag}");
    app_guest(port, &dana, &jobs).await;
    // Dana also holds the other App (its own role there).
    let r = owner_post(port, "/cli/skin/roles/from-template", json!({ "template": "staff", "app": crm, "name": format!("crm-{tag}") })).await;
    let crm_role = r["role"]["name"].as_str().unwrap().to_string();
    let pid = k2_core::skin::list_principals().unwrap().into_iter().find(|p| p.username == dana).unwrap().id;
    k2_core::skin::create_grant(&k2_core::skin::SkinGrantWrite {
        subject_kind: "principal".into(),
        subject_id: pid.clone(),
        kind: "app".into(),
        target_id: crm.clone(),
        role: Some(crm_role),
        scope: None,
        created_by: Some("test".into()),
    })
    .unwrap();

    let a = login_pass(port, &dana, &jobs, SAFARI).await;
    let b = login_pass(port, &dana, &jobs, "").await;
    let c = login_pass(port, &dana, &crm, "").await;

    // Only a person's own session pass.
    let r = call(port, "GET", &format!("/cli/skin/profile?token={OWNER}"), None, None, "").await;
    assert_eq!((r.status, r.code()), (403, "guest_required".to_string()), "the owner token: {}", r.body);
    let r = call(port, "GET", "/cli/skin/profile", None, None, "").await;
    assert_eq!(r.status, 403, "no credential: {}", r.body);

    // profile.get: this App only; no picture unless asked.
    let r = call(port, "GET", "/cli/skin/profile", Some(&a), None, "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let p = r.json();
    assert_eq!(p["username"], dana.as_str());
    assert_eq!(p["app"], jobs.as_str());
    assert_eq!(p["role"], format!("{dana}-role").as_str(), "the role on THIS App's grant");
    assert_eq!(p["email"], format!("{dana}@example.test").as_str());
    assert_eq!(p["counts"]["sessions"], 2, "a and b, never crm's c");
    assert_eq!(p["can"], json!({ "editNames": true, "editImage": true }));
    assert!(p.get("image").is_none(), "no picture unless asked");
    assert!(p["rooms"].as_array().unwrap().iter().all(|x| x["room"] == shop.id.as_str()), "this App's rooms only: {}", p["rooms"]);
    let r = call(port, "GET", "/cli/skin/profile", Some(&c), None, "").await;
    assert_eq!(r.json()["role"], format!("crm-{tag}").as_str(), "each App sees its own role");

    // profile.update: cleaned, marked self-set, the owner sees the mark.
    let r = call(port, "POST", "/cli/skin/profile", Some(&a), Some(json!({ "fullName": "  Dana \u{202E}Ruiz\u{200B} ", "displayName": "Dee" })), "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let p = r.json()["profile"].clone();
    assert_eq!((p["fullName"].as_str(), p["displayName"].as_str(), p["nameSetBySelf"].as_bool()), (Some("Dana Ruiz"), Some("Dee"), Some(true)));
    let roster = get(port, "/cli/skin/users", OWNER, "").await.json();
    let row = roster["users"].as_array().unwrap().iter().find(|u| u["username"] == dana.as_str()).unwrap().clone();
    assert_eq!(row["nameSetBySelf"], true, "U24: the owner's roster marks a self-set name");
    let r = call(port, "POST", "/cli/skin/profile", Some(&a), Some(json!({ "username": "mallory" })), "").await;
    assert_eq!((r.status, r.code()), (400, "usage".to_string()), "the username never changes: {}", r.body);

    // Pictures: stored as K2's JPEG, only when asked, cached by hash.
    let r = call(port, "POST", "/cli/skin/profile/image", Some(&a), Some(json!({ "image": png(600, 400) })), "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let hash = r.json()["imageHash"].as_str().unwrap().to_string();
    assert!(r.json()["image"].as_str().unwrap().starts_with("data:image/jpeg;base64,"));
    let r = call(port, "GET", "/cli/skin/profile?image=1", Some(&b), None, "").await;
    assert!(r.json()["image"].as_str().unwrap().starts_with("data:image/jpeg;base64,"));
    let r = call(port, "GET", &format!("/cli/skin/profile?ifImageHash={hash}"), Some(&b), None, "").await;
    assert!(r.json().get("image").is_none(), "unchanged hash: no picture");
    let r = call(port, "POST", "/cli/skin/profile/image", Some(&a), Some(json!({ "image": "data:image/svg+xml;base64,PHN2Zy8+" })), "").await;
    assert_eq!((r.status, r.code()), (415, "type_refused".to_string()), "{}", r.body);
    let r = call(port, "POST", "/cli/skin/profile/image/clear", Some(&a), None, "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(call(port, "GET", "/cli/skin/profile", Some(&a), None, "").await.json()["imageHash"].is_null());

    // Per-App settings: off writes nothing.
    owner_post(port, "/cli/app/profile-settings", json!({ "app": jobs, "editNames": false })).await;
    let r = call(port, "POST", "/cli/skin/profile", Some(&a), Some(json!({ "fullName": "Someone Else" })), "").await;
    assert_eq!((r.status, r.code()), (403, "edit_off".to_string()), "{}", r.body);
    let p = call(port, "GET", "/cli/skin/profile", Some(&a), None, "").await.json();
    assert_eq!((p["fullName"].as_str(), p["can"]["editNames"].as_bool()), (Some("Dana Ruiz"), Some(false)));
    let r = call(port, "POST", "/cli/skin/profile", Some(&c), Some(json!({ "displayName": "D" })), "").await;
    assert_eq!(r.status, 200, "another App keeps its own setting: {}", r.body);
    // An agent without "manage its Apps' users" can't touch the settings.
    let agent = passport(&shop);
    allow_build_only(&shop, true);
    let r = post(port, "/cli/app/profile-settings", &agent, json!({ "app": jobs, "editImage": false })).await;
    assert_eq!((r.status, r.json()["error"]["code"].as_str()), (403, Some("app_users_off")), "{}", r.body);
    // With "Let this agent manage its Apps' users" (apps.manage-users,
    // 0225): lower only (U16), its own workspace's Apps, never host.
    allow_manage(&shop, true);
    let r = post(port, "/cli/app/profile-settings", &agent, json!({ "app": jobs, "editImage": false })).await;
    assert_eq!((r.status, r.json()["settings"]["editImage"].as_bool()), (200, Some(false)), "an agent may turn it off: {}", r.body);
    let r = post(port, "/cli/app/profile-settings", &agent, json!({ "app": jobs, "editNames": true })).await;
    assert_eq!((r.status, r.json()["error"]["code"].as_str()), (403, Some("owner_only")), "never on: {}", r.body);
    let r = post(port, "/cli/app/profile-settings", &agent, json!({ "app": crm, "editImage": false })).await;
    assert_eq!(r.status, 403, "another workspace's App: {}", r.body);
    let r = post(port, "/cli/app/profile-settings", &agent, json!({ "app": "host", "editImage": false })).await;
    assert_eq!(r.status, 403, "never the box login: {}", r.body);
    owner_post(port, "/cli/app/profile-settings", json!({ "app": jobs, "editImage": true })).await;

    // Sessions: this App's only, opaque ids, the caller first.
    let r = call(port, "GET", "/cli/skin/sessions", Some(&a), None, "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let rows = r.json()["sessions"].as_array().unwrap().clone();
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[0]["current"], true);
    assert_eq!(rows[0]["client"], "Safari on macOS", "a coarse label from the User-Agent");
    assert_eq!(rows[0]["via"], "password");
    for row in &rows {
        let id = row["id"].as_str().unwrap();
        assert!(id.starts_with("ses_") && !id.contains(&a[..12]), "opaque: {id}");
    }
    let crm_id = call(port, "GET", "/cli/skin/sessions", Some(&c), None, "").await.json()["sessions"][0]["id"].as_str().unwrap().to_string();
    let r = call(port, "POST", "/cli/skin/sessions/revoke", Some(&a), Some(json!({ "id": crm_id })), "").await;
    assert_eq!((r.status, r.code()), (404, "not_found".to_string()), "another App's session: {}", r.body);
    let r = call(port, "POST", "/cli/skin/sessions/revoke-others", Some(&a), None, "").await;
    assert_eq!((r.status, r.json()["ended"].as_u64()), (200, Some(1)), "{}", r.body);
    assert_eq!(call(port, "GET", "/cli/skin/profile", Some(&b), None, "").await.status, 401, "b ended");
    assert_eq!(call(port, "GET", "/cli/skin/profile", Some(&a), None, "").await.status, 200, "the caller stays");
    assert_eq!(call(port, "GET", "/cli/skin/profile", Some(&c), None, "").await.status, 200, "another App's session stays");

    // The owner's view and revoke.
    let r = get(port, "/cli/skin/access/profile", OWNER, &format!("app={jobs}&user={dana}")).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert_eq!((v["nameSetBySelf"].as_bool(), v["sessions"].as_array().unwrap().len()), (Some(true), 1));
    let r = owner_post(port, "/cli/skin/access/sessions/revoke", json!({ "app": jobs, "user": dana, "all": true })).await;
    assert_eq!(r["ended"], 1);
    assert_eq!(call(port, "GET", "/cli/skin/profile", Some(&a), None, "").await.status, 401);

    // An agent's own login reads, never writes.
    let bot = format!("bot{tag}");
    app_guest(port, &bot, &jobs).await;
    owner_post(port, "/cli/skin/users/agent", json!({ "username": bot, "workspace": shop.id })).await;
    let bp = login_pass(port, &bot, &jobs, "").await;
    let r = call(port, "GET", "/cli/skin/profile", Some(&bp), None, "").await;
    assert_eq!((r.status, r.json()["agentLogin"].as_bool()), (200, Some(true)), "{}", r.body);
    let r = call(port, "POST", "/cli/skin/profile/image/clear", Some(&bp), None, "").await;
    assert_eq!((r.status, r.code()), (403, "agent_login".to_string()), "{}", r.body);
    drop(d);
}

// ── P0 ───────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wrong_current_passwords_throttle_and_stay_keeps_the_caller() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let shop = seed_ws(&env, &format!("pw{tag}"));
    let jobs = service(&shop, "jobs", 47_411);
    let ema = format!("ema{tag}");
    app_guest(port, &ema, &jobs).await;
    let a = login_pass(port, &ema, &jobs, "").await;
    let b = login_pass(port, &ema, &jobs, "").await;

    for i in 0..5 {
        let r = call(port, "POST", "/cli/skin/password/change", Some(&a), Some(json!({ "oldPassword": "nope-nope", "password": "next-pass-1" })), "").await;
        assert_eq!(r.status, 401, "wrong {i}: {}", r.body);
        assert_eq!(r.body.trim(), r#"{"error":"invalid password"}"#, "today's bytes");
    }
    let r = call(port, "POST", "/cli/skin/password/change", Some(&a), Some(json!({ "oldPassword": "s3cret-horse", "password": "next-pass-1" })), "").await;
    assert_eq!((r.status, r.code()), (429, "rate_limited".to_string()), "the 6th try, even right: {}", r.body);
    assert!(r.head.to_ascii_lowercase().contains("retry-after:"), "{}", r.head);
    assert!(r.json()["error"]["retryAfterSecs"].as_i64().unwrap() > 0);
    assert_eq!(login(port, &ema, "s3cret-horse", &jobs, "").await.status, 200, "never the login lock");
    let pid = k2_core::skin::list_principals().unwrap().into_iter().find(|p| p.username == ema).unwrap().id;
    k2_core::skin::guard::expire_current_password_window(&pid);

    let long = "x".repeat(257);
    let r = call(port, "POST", "/cli/skin/password/change", Some(&a), Some(json!({ "oldPassword": "s3cret-horse", "password": long })), "").await;
    assert_eq!(r.status, 400, "257 bytes: {}", r.body);

    let r = call(port, "POST", "/cli/skin/password/change", Some(&a), Some(json!({ "oldPassword": "s3cret-horse", "password": "next-pass-1", "stay": true })), "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json(), json!({ "ok": true, "stayed": true }));
    assert_eq!(call(port, "GET", "/cli/skin/profile", Some(&a), None, "").await.status, 200, "the caller stays");
    assert_eq!(call(port, "GET", "/cli/skin/profile", Some(&b), None, "").await.status, 401, "the others end");

    // No stay: today's bytes, the caller ends too.
    let r = call(port, "POST", "/cli/skin/password/change", Some(&a), Some(json!({ "oldPassword": "next-pass-1", "password": "next-pass-2" })), "").await;
    assert_eq!((r.status, r.body.trim()), (200, r#"{"ok":true}"#));
    assert_eq!(call(port, "GET", "/cli/skin/profile", Some(&a), None, "").await.status, 401);
    drop(d);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_used_owner_reset_unlocks_and_leaves_no_app_link_behind() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let shop = seed_ws(&env, &format!("rs{tag}"));
    let jobs = service(&shop, "jobs", 47_421);
    let fay = format!("fay{tag}");
    app_guest(port, &fay, &jobs).await;
    let pid = k2_core::skin::list_principals().unwrap().into_iter().find(|p| p.username == fay).unwrap().id;

    // An App's reset link that its page may have seen.
    let app_link = k2_core::skin::invites::mint(&pid, &jobs, k2_core::skin::invites::PURPOSE_RESET, 3600, None, false, None, "pass:test").unwrap();
    k2_core::skin::invites::activate(&app_link.id).unwrap();
    // Locked out.
    for _ in 0..3 {
        let _ = login(port, &fay, "wrong-wrong-1", &jobs, "").await;
    }
    assert_eq!(login(port, &fay, "s3cret-horse", &jobs, "").await.status, 423, "locked");

    let r = owner_post(port, "/cli/skin/users/reset-link", json!({ "username": fay, "app": jobs, "code": true })).await;
    let token = r["code"].as_str().or(r["link"].as_str().and_then(|l| l.split('#').nth(1))).expect("the owner's link").to_string();
    // While the owner's reset is live an App's reset mints nothing.
    let err = k2_core::skin::invites::mint(&pid, &jobs, k2_core::skin::invites::PURPOSE_RESET, 3600, None, false, None, "pass:test").unwrap_err();
    assert_eq!(err, k2_core::skin::invites::MintError::OwnerResetPending);

    let r = call(port, "POST", "/cli/skin/password/reset", None, Some(json!({ "token": token, "password": "owners-reset-1" })), "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(login(port, &fay, "owners-reset-1", &jobs, "").await.status, 200, "U7: the used reset cleared the lock");
    let r = call(port, "POST", "/cli/skin/password/reset", None, Some(json!({ "token": app_link.token, "password": "app-takeover-1" })), "").await;
    assert_eq!(r.status, 401, "U6: the App's leftover link died with the owner's reset: {}", r.body);
    drop(d);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_public_reset_door_has_its_own_per_client_budget() {
    let _env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    // A local proxy names the client (loopback + X-Forwarded-For).
    let xff = "X-Forwarded-For: 198.51.100.77\r\n";
    for i in 0..k2_daemon::login_throttle::RESET_LIMIT {
        let r = call(port, "POST", "/cli/skin/password/reset", None, Some(json!({ "token": format!("k2inv_junk{i}"), "password": "whatever-pw-1" })), xff).await;
        assert_eq!(r.status, 401, "{i}: {}", r.body);
    }
    let r = call(port, "POST", "/cli/skin/password/reset", None, Some(json!({ "token": "k2inv_junk", "password": "whatever-pw-1" })), xff).await;
    assert_eq!(r.status, 429, "over the reset door's budget: {}", r.body);
    // Logins from that client are another door.
    let r = call(port, "POST", "/cli/skin/login", None, Some(json!({ "username": "nobody", "password": "x" })), xff).await;
    assert_eq!(r.status, 401, "{}", r.body);
    // Another client is fine.
    let r = call(port, "POST", "/cli/skin/password/reset", None, Some(json!({ "token": "k2inv_junk", "password": "whatever-pw-1" })), "X-Forwarded-For: 198.51.100.78\r\n").await;
    assert_eq!(r.status, 401, "{}", r.body);
    drop(d);
}

// ── the App's link (a BFF acting for the person, AR50) ───────────────

/// One request on an App's gateway link with `pass` as Bearer.
async fn on_link(lport: u16, method: &'static str, pq: &str, pass: &str, body: Option<Value>) -> Reply {
    let (pq, pass, body) = (pq.to_string(), pass.to_string(), body.map(|b| b.to_string()));
    tokio::task::spawn_blocking(move || {
        let mut s = std::net::TcpStream::connect(("127.0.0.1", lport)).expect("connect link");
        s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
        let raw = match body {
            Some(b) => format!(
                "{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1:8730\r\nAuthorization: Bearer {pass}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
                b.len()
            ),
            None => format!("{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1:8730\r\nAuthorization: Bearer {pass}\r\nConnection: close\r\n\r\n"),
        };
        s.write_all(raw.as_bytes()).unwrap();
        let mut out = Vec::new();
        let _ = s.read_to_end(&mut out);
        let text = String::from_utf8_lossy(&out).to_string();
        let status = text.lines().next().and_then(|l| l.split_whitespace().nth(1)).and_then(|x| x.parse().ok()).unwrap_or(0);
        let (head, body) = text.split_once("\r\n\r\n").map(|(h, b)| (h.to_string(), b.to_string())).unwrap_or_default();
        Reply { status, head, body }
    })
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_bff_changes_the_password_and_reads_the_profile_on_the_link() {
    use k2_daemon::apps::gateway::{self, LinkSpec};
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let shop = seed_ws(&env, &format!("lk{tag}"));
    let other_ws = seed_ws(&env, &format!("lo{tag}"));
    let jobs = service(&shop, "jobs", 47_431);
    let crm = service(&other_ws, "crm", 47_432);
    let gus = format!("gus{tag}");
    app_guest(port, &gus, &jobs).await;
    let ivy = format!("ivy{tag}");
    app_guest(port, &ivy, &crm).await;
    let pass = login_pass(port, &gus, &jobs, "").await;
    let elsewhere = login_pass(port, &ivy, &crm, "").await;
    let app_id = format!("jobs{tag}");
    let spec = LinkSpec {
        app_id: app_id.clone(),
        service_id: jobs.clone(),
        workspace_id: shop.id.clone(),
        port: 0,
        caps: vec!["thread:read".into()],
        rooms: vec![shop.id.clone()],
    };
    let app_pass = gateway::open(spec, None).expect("open link");
    let lport = gateway::port_of(&app_id).expect("listening");

    // profile.get on the link: the person's own session (AR50), never the
    // App's own pass, never another App's session.
    let r = on_link(lport, "GET", "/cli/skin/profile", &pass, None).await;
    assert_eq!((r.status, r.json()["username"].as_str()), (200, Some(gus.as_str())), "{}", r.body);
    let r = on_link(lport, "GET", "/cli/skin/profile", &app_pass, None).await;
    assert_eq!((r.status, r.code()), (403, "guest_required".to_string()), "the App's own pass: {}", r.body);
    let r = on_link(lport, "GET", "/cli/skin/profile", &elsewhere, None).await;
    assert_ne!(r.status, 200, "another App's session on this link: {}", r.body);

    // password.change on the link (G12, a BFF for the person): the U5
    // throttle holds on this door too.
    for i in 0..5 {
        let r = on_link(lport, "POST", "/cli/skin/password/change", &pass, Some(json!({ "oldPassword": "nope-nope", "password": "link-pass-1" }))).await;
        assert_eq!(r.status, 401, "wrong {i} on the link: {}", r.body);
    }
    let r = on_link(lport, "POST", "/cli/skin/password/change", &pass, Some(json!({ "oldPassword": "s3cret-horse", "password": "link-pass-1" }))).await;
    assert_eq!((r.status, r.code()), (429, "rate_limited".to_string()), "the link door counts too: {}", r.body);
    let pid = k2_core::skin::list_principals().unwrap().into_iter().find(|p| p.username == gus).unwrap().id;
    k2_core::skin::guard::expire_current_password_window(&pid);
    let r = on_link(lport, "POST", "/cli/skin/password/change", &pass, Some(json!({ "oldPassword": "s3cret-horse", "password": "link-pass-1", "stay": true }))).await;
    assert_eq!((r.status, r.json()), (200, json!({ "ok": true, "stayed": true })), "{}", r.body);
    assert_eq!(login(port, &gus, "s3cret-horse", &jobs, "").await.status, 401, "the old password fails");
    assert_eq!(login(port, &gus, "link-pass-1", &jobs, "").await.status, 200, "the new one works");
    assert_eq!(on_link(lport, "GET", "/cli/skin/profile", &pass, None).await.status, 200, "the BFF's pass stayed");
    // Without stay, today's behaviour on the link too: the pass ends.
    let r = on_link(lport, "POST", "/cli/skin/password/change", &pass, Some(json!({ "oldPassword": "link-pass-1", "password": "link-pass-2" }))).await;
    assert_eq!((r.status, r.body.trim()), (200, r#"{"ok":true}"#));
    assert_ne!(on_link(lport, "GET", "/cli/skin/profile", &pass, None).await.status, 200);
    gateway::close(&app_id, None, true);
    drop(d);
}
