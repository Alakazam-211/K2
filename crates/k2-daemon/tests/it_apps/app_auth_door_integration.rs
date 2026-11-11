//! The sign-in door, headless (prd-app-hosted-sign-in-v1 + vs-live, 0.46.9):
//! the real dispatcher in process on a temp `$HOME`, the real App path
//! (`apps::router`) in front of a recording fake App, driven over loopback.
//!
//! - Every door verb: success, wrong password, the shared lockout, PKCE
//!   mismatch, code reuse, expiry, another App's key, a password changed
//!   under a code, must-change, an invite, the owner's reset refused.
//! - The exchange on both doors: the App key (`/v1/apps/<id>/auth/exchange`)
//!   and the App's gateway link (`/cli/skin/auth/exchange`).
//! - The App never gets a byte of a door request on any covered host: its
//!   front port, the k2.dev label path (`Mode::Tunnel`, a Guarded App's
//!   row) and a custom name (`Mode::Door`), first request and mid
//!   keep-alive. A raw App's label has no door; the switches turn it off.
//! - Origins, Content-Type, the per-visitor attempt bucket, a suspended
//!   App, and one audit row per door request (never the body).

use crate::support::app_fixture;

use std::io::{Read, Write};
use std::net::{IpAddr, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use app_fixture::*;
use base64::Engine as _;
use k2_core::skin::{self, auth_door as door_store};
use k2_daemon::apps::{auth_door, gateway, router};
use k2_daemon::client_ip::Mode;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const PW: &str = "first-pass-11";

// ── fixtures ─────────────────────────────────────────────────────────

struct Seeded {
    ws: Ws,
    /// App A: the door's App (a Guarded App's row).
    app: String,
    svc: String,
    /// App B: another App on the box.
    other_app: String,
    other_svc: String,
    user: String,
}

fn runtime_app(ws: &Ws, app_id: &str, isolation: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let s = k2_core::published_services::insert(&conn, &ws.id, app_id, "node s.js", "/tmp", 4390, "tunnel", "running", "cmd", "").expect("svc");
    k2_core::deploy::store::insert_app(
        &conn,
        &k2_core::deploy::store::NewApp {
            app_id,
            service_id: &s.id,
            seed_id: None,
            owner_workspace: &ws.id,
            isolation,
            memory_limit_bytes: None,
            front_port: 0,
            created_by: "test",
        },
    )
    .expect("runtime app");
    s.id
}

fn seed(env: &Env, tag: &str) -> Seeded {
    let ws = seed_ws(env, &format!("aw{tag}"));
    let app = format!("ad{tag}");
    let other_app = format!("ao{tag}");
    let svc = runtime_app(&ws, &app, "container");
    let other_svc = runtime_app(&ws, &other_app, "process");
    let user = format!("dana{tag}");
    skin::add_principal_for_app(&user, None, &svc, None, "test").expect("guest");
    skin::set_principal_password(&user, Some(PW)).expect("password");
    // The switch reads are cached per process: start from this HOME's.
    auth_door::invalidate();
    Seeded { ws, app, svc, other_app, other_svc, user }
}

fn door_on() {
    door_store::set_box_on(true, "test").expect("switch");
    auth_door::invalidate();
}

/// A fake App on loopback that records every byte and answers `200 app`
/// to each request head (keep-alive).
fn recording_app() -> (u16, Arc<Mutex<Vec<u8>>>) {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let seen: Arc<Mutex<Vec<u8>>> = Arc::default();
    let seen2 = seen.clone();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { return };
            let seen = seen2.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                let mut pending: Vec<u8> = Vec::new();
                loop {
                    let n = match s.read(&mut buf) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => n,
                    };
                    seen.lock().unwrap().extend_from_slice(&buf[..n]);
                    pending.extend_from_slice(&buf[..n]);
                    while let Some(i) = pending.windows(4).position(|w| w == b"\r\n\r\n") {
                        pending.drain(..i + 4);
                        if s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\napp").is_err() {
                            return;
                        }
                    }
                }
            });
        }
    });
    (port, seen)
}

fn seen_text(seen: &Arc<Mutex<Vec<u8>>>) -> String {
    String::from_utf8_lossy(&seen.lock().unwrap()).into_owned()
}

/// A listener that serves each connection the way the App TLS door
/// (custom names) or the E2E listener (k2.dev labels) does after TLS.
async fn app_path(app_id: &str, mode: Mode) -> u16 {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let id = app_id.to_string();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else { return };
            let id = id.clone();
            tokio::spawn(async move { router::serve_app_conn_with(&mut s, &id, mode).await });
        }
    });
    port
}

struct R {
    status: u16,
    head: String,
    body: String,
}

impl R {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("JSON ({e}): {:?} {}", self.body, self.head))
    }
    fn code(&self) -> String {
        self.json()["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {}", self.body)).to_string()
    }
}

fn parse(text: &str) -> R {
    let status = text.lines().next().and_then(|l| l.split_whitespace().nth(1)).and_then(|x| x.parse().ok()).unwrap_or_else(|| panic!("no status: {text:?}"));
    let (head, body) = text.split_once("\r\n\r\n").map(|(h, b)| (h.to_string(), b.to_string())).unwrap_or_default();
    R { status, head, body }
}

fn raw(port: u16, req: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    s.write_all(req.as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    String::from_utf8_lossy(&out).into_owned()
}

fn req_text(method: &str, path: &str, headers: &[&str], body: Option<&str>) -> String {
    let mut r = format!("{method} {path} HTTP/1.1\r\nHost: shop.example.com\r\nConnection: close\r\n");
    for h in headers {
        r.push_str(h);
        r.push_str("\r\n");
    }
    match body {
        Some(b) => r.push_str(&format!("Content-Length: {}\r\n\r\n{b}", b.len())),
        None => r.push_str("\r\n"),
    }
    r
}

async fn door(port: u16, method: &'static str, path: &'static str, headers: Vec<&'static str>, body: Option<Value>) -> R {
    tokio::task::spawn_blocking(move || {
        let b = body.map(|v| v.to_string());
        parse(&raw(port, &req_text(method, path, &headers, b.as_deref())))
    })
    .await
    .unwrap()
}

const JSON_CT: &str = "Content-Type: application/json";

async fn post_door(port: u16, path: &'static str, body: Value) -> R {
    door(port, "POST", path, vec![JSON_CT], Some(body)).await
}

/// A PKCE pair: (verifier, challenge).
fn pkce(seed: &str) -> (String, String) {
    let verifier = format!("{seed:-<43}").chars().take(43).map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { 'x' }).collect::<String>();
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

async fn sign_in(port: u16, user: &str, pw: &str, challenge: &str) -> R {
    post_door(port, "/.k2/auth/sign-in", json!({"username": user, "password": pw, "challenge": challenge})).await
}

async fn exchange_key(d: u16, svc: &str, key: &str, code: &str, verifier: &str) -> Resp {
    post(d, &format!("/v1/apps/{svc}/auth/exchange"), key, json!({"code": code, "verifier": verifier})).await
}

fn key_for(svc: &str, scopes: &str) -> String {
    k2_core::api_keys::create_app_key(&format!("k-{svc}"), svc, &format!(r#"{{"scopes":[{scopes}]}}"#)).expect("app key").1
}

// ── the verbs and the exchange ───────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sign_in_and_exchange_with_every_refusal() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let s = seed(&env, "v1");
    let (app_port, seen) = recording_app();
    router::switch_with(&s.app, app_port, true);
    let front = router::ensure_front(&tokio::runtime::Handle::current(), &s.app, 0, true).expect("front port");

    // Off (the default): today's behaviour, the App answers its own path.
    let r = door(front, "GET", "/.k2/auth/ping", vec![], None).await;
    assert_eq!((r.status, r.body.as_str()), (200, "app"), "off = the App's path");
    assert!(seen_text(&seen).contains("GET /.k2/auth/ping"));
    seen.lock().unwrap().clear();

    door_on();
    let r = door(front, "GET", "/.k2/auth/ping", vec![], None).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json(), json!({"k2": true, "app": s.app, "v": 1, "door": "/.k2/auth"}));
    assert!(r.head.contains("Cache-Control: no-store") && !r.head.to_ascii_lowercase().contains("set-cookie"), "{}", r.head);

    let key = key_for(&s.svc, r#""signin""#);
    let other_key = key_for(&s.other_svc, r#""users""#);

    // Success, then the code once.
    let (v, c) = pkce("verifier-success");
    let r = sign_in(front, &s.user, PW, &c).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let code = r.json()["code"].as_str().expect("code").to_string();
    assert!(code.starts_with("k2lgn_"));
    assert!(!r.body.contains("k2skn_"), "no pass in the browser");
    let x = exchange_key(d.port, &s.svc, &key, &code, &v).await;
    assert_eq!(x.status, 200, "{}", x.body);
    let j = x.json();
    let pass = j["token"].as_str().expect("token");
    assert!(pass.starts_with("k2skn_"));
    assert_eq!((j["username"].as_str(), j["app"].as_str(), j["mustChangePassword"].as_bool(), j["via"].as_str()), (Some(s.user.as_str()), Some(s.svc.as_str()), Some(false), Some("sign-in")));
    assert!(j["authAt"].as_i64().expect("authAt") > 0, "the recent-auth stamp");
    let p = skin::resolve_skin_token(pass).expect("a live pass");
    assert_eq!(p.principal_id.is_some(), true);
    // Reuse.
    let again = exchange_key(d.port, &s.svc, &key, &code, &v).await;
    assert_eq!((again.status, again.code()), (401, "bad_code".to_string()), "one use");

    // PKCE mismatch uses the code up.
    let (v, c) = pkce("verifier-mismatch");
    let code = sign_in(front, &s.user, PW, &c).await.json()["code"].as_str().unwrap().to_string();
    let (wrong, _) = pkce("verifier-somebody-else");
    assert_eq!(exchange_key(d.port, &s.svc, &key, &code, &wrong).await.code(), "bad_code");
    assert_eq!(exchange_key(d.port, &s.svc, &key, &code, &v).await.code(), "bad_code", "a wrong verifier used it up");

    // Another App's key: refused, and the code still works for its App.
    let (v, c) = pkce("verifier-cross-app");
    let code = sign_in(front, &s.user, PW, &c).await.json()["code"].as_str().unwrap().to_string();
    let cross = exchange_key(d.port, &s.other_svc, &other_key, &code, &v).await;
    assert_eq!((cross.status, cross.code()), (401, "bad_code".to_string()), "another App's key");
    // The App key of App B can't even name App A's route.
    assert_eq!(exchange_key(d.port, &s.svc, &other_key, &code, &v).await.status, 404, "a key opens its own App only");
    assert_eq!(exchange_key(d.port, &s.svc, &key, &code, &v).await.status, 200, "not used up by the other App");

    // Expiry.
    let (v, c) = pkce("verifier-expired");
    let code = sign_in(front, &s.user, PW, &c).await.json()["code"].as_str().unwrap().to_string();
    auth_door::age_codes_for_tests(auth_door::CODE_TTL + Duration::from_secs(1));
    assert_eq!(exchange_key(d.port, &s.svc, &key, &code, &v).await.code(), "bad_code", "expired");

    // V12: the password changes between the sign-in and the exchange.
    let (v, c) = pkce("verifier-v12");
    let code = sign_in(front, &s.user, PW, &c).await.json()["code"].as_str().unwrap().to_string();
    skin::set_principal_password(&s.user, Some("second-pass-22")).unwrap();
    assert_eq!(exchange_key(d.port, &s.svc, &key, &code, &v).await.code(), "bad_code", "a code dies with its password");
    skin::set_principal_password(&s.user, Some(PW)).unwrap();

    // Missing or malformed challenge.
    let r = post_door(front, "/.k2/auth/sign-in", json!({"username": s.user, "password": PW})).await;
    assert_eq!((r.status, r.code()), (400, "challenge_required".to_string()));
    let r = post_door(front, "/.k2/auth/sign-in", json!({"username": s.user, "password": PW, "challenge": "plain"})).await;
    assert_eq!(r.code(), "challenge_required");

    // No grant on App B: the same bad_login as a wrong password.
    let (_, c) = pkce("verifier-no-grant");
    let b_front = {
        let (bp, _) = recording_app();
        router::switch_with(&s.other_app, bp, true);
        router::ensure_front(&tokio::runtime::Handle::current(), &s.other_app, 0, true).unwrap()
    };
    let r = sign_in(b_front, &s.user, PW, &c).await;
    assert_eq!((r.status, r.code()), (401, "bad_login".to_string()), "{}", r.body);
    skin::clear_login_lockout(&s.user).unwrap();

    // Wrong password, then the shared 3-in-15 lockout.
    let (_, c) = pkce("verifier-lockout");
    for _ in 0..3 {
        let r = sign_in(front, &s.user, "wrong-pass-00", &c).await;
        assert_eq!((r.status, r.code()), (401, "bad_login".to_string()), "{}", r.body);
    }
    let r = sign_in(front, &s.user, PW, &c).await;
    assert_eq!((r.status, r.code()), (423, "locked".to_string()), "{}", r.body);
    assert!(r.head.contains("Retry-After: "), "{}", r.head);
    assert!(r.json()["error"]["retryAfterSecs"].as_i64().unwrap() > 0);
    // The same lockout as every other sign-in.
    let login = post(d.port, "/cli/skin/login", "", json!({"username": s.user, "password": PW, "app": s.svc})).await;
    assert_eq!(login.status, 423, "{}", login.body);
    skin::clear_login_lockout(&s.user).unwrap();

    // The App never saw any of it.
    let seen = seen_text(&seen);
    assert!(!seen.contains(PW) && !seen.contains("/.k2/auth") && !seen.contains("k2lgn_"), "the App saw: {seen}");

    // One audit row per door request, never the body.
    k2_daemon::apps::audit::flush();
    let a = get(d.port, "/cli/app/audit", OWNER, &format!("app={}&limit=200", s.app)).await;
    assert_eq!(a.status, 200, "{}", a.body);
    let calls = a.json()["calls"].as_array().cloned().unwrap_or_default();
    let door_rows: Vec<&Value> = calls.iter().filter(|c| c["verb"].as_str().is_some_and(|v| v.contains("/.k2/auth/"))).collect();
    assert!(door_rows.len() >= 10, "{}", a.body);
    assert!(door_rows.iter().any(|c| c["verb"] == "POST /.k2/auth/sign-in" && c["result"] == 423 && c["code"] == "locked"), "{}", a.body);
    assert!(door_rows.iter().all(|c| c["onBehalfOf"].as_str().is_some_and(|v| v.starts_with("visitor:"))), "{}", a.body);
    assert!(!a.body.contains(PW) && !a.body.contains("k2lgn_"), "no body in the audit");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn change_set_password_must_change_invites_and_the_link_exchange() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let s = seed(&env, "v2");
    let (app_port, seen) = recording_app();
    router::switch_with(&s.app, app_port, true);
    let front = router::ensure_front(&tokio::runtime::Handle::current(), &s.app, 0, true).expect("front port");
    door_on();
    let key = key_for(&s.svc, r#""users""#);

    // change-password: a grant first; same password refused; then a code.
    let (v, c) = pkce("verifier-change");
    let r = post_door(front, "/.k2/auth/change-password", json!({"username": s.user, "oldPassword": PW, "password": PW, "challenge": c})).await;
    assert_eq!(r.code(), "password_unchanged");
    let r = post_door(front, "/.k2/auth/change-password", json!({"username": s.user, "oldPassword": PW, "password": "new-pass-333", "challenge": c})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let code = r.json()["code"].as_str().unwrap().to_string();
    let x = exchange_key(d.port, &s.svc, &key, &code, &v).await;
    assert_eq!((x.status, x.json()["via"].as_str()), (200, Some("change-password")), "{}", x.body);
    let (_, c2) = pkce("verifier-old-pw");
    assert_eq!(sign_in(front, &s.user, PW, &c2).await.code(), "bad_login", "the old password is gone");
    skin::clear_login_lockout(&s.user).unwrap();
    assert_eq!(sign_in(front, &s.user, "new-pass-333", &c2).await.status, 200);

    // must-change: sign-in answers the change token, set-password ends in a code.
    skin::set_principal_password_tied(&s.user, Some("temp-pass-44"), None, true).unwrap();
    let (v, c) = pkce("verifier-must-change");
    let r = sign_in(front, &s.user, "temp-pass-44", &c).await;
    assert_eq!((r.status, r.code()), (403, "password_change_required".to_string()), "{}", r.body);
    let tok = r.json()["changeToken"].as_str().expect("changeToken").to_string();
    assert!(tok.starts_with("k2chg_") && !r.body.contains("k2skn_"));
    // The exact shape k2 study auth-pages shows (0.46.11): the token at the
    // top level and again in error, with its end in seconds.
    let j = r.json();
    let mut keys: Vec<&str> = j.as_object().expect("object").keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["changeExpiresAt", "changeToken", "error", "mustChangePassword", "ok"], "{j}");
    let mut ekeys: Vec<&str> = j["error"].as_object().expect("error").keys().map(String::as_str).collect();
    ekeys.sort_unstable();
    assert_eq!(ekeys, ["changeExpiresAt", "changeToken", "code", "hint"], "{j}");
    assert_eq!((j["ok"].as_bool(), j["mustChangePassword"].as_bool(), j["error"]["changeToken"].as_str()), (Some(false), Some(true), Some(tok.as_str())), "{j}");
    let exp = j["changeExpiresAt"].as_i64().expect("changeExpiresAt");
    assert_eq!(j["error"]["changeExpiresAt"].as_i64(), Some(exp), "{j}");
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    assert!(exp > now && exp <= now + 15 * 60 + 5, "seconds since the epoch, 15 minutes: {j}");
    let r = post_door(front, "/.k2/auth/set-password", json!({"token": tok, "password": "temp-pass-44", "challenge": c})).await;
    assert_eq!(r.code(), "password_unchanged");
    let r = post_door(front, "/.k2/auth/set-password", json!({"token": tok, "password": "mine-pass-555", "challenge": c})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let code = r.json()["code"].as_str().expect("a code: no second typing").to_string();
    let x = exchange_key(d.port, &s.svc, &key, &code, &v).await;
    assert_eq!((x.status, x.json()["mustChangePassword"].as_bool()), (200, Some(false)), "{}", x.body);

    // An invite for this App: set-password with a challenge gives a code.
    let pid = skin::resolve_skin_token(x.json()["token"].as_str().unwrap()).unwrap().principal_id.unwrap();
    let inv = skin::invites::mint(&pid, &s.svc, skin::invites::PURPOSE_RESET, 3600, None, false, None, "test").unwrap();
    skin::invites::activate(&inv.id).unwrap().expect("started");
    let (v, c) = pkce("verifier-invite");
    let r = post_door(front, "/.k2/auth/set-password", json!({"token": inv.token, "password": "reset-pass-66", "challenge": c})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let code = r.json()["code"].as_str().unwrap().to_string();
    assert_eq!(exchange_key(d.port, &s.svc, &key, &code, &v).await.status, 200);
    // Without a challenge: set, no code.
    let inv = skin::invites::mint(&pid, &s.svc, skin::invites::PURPOSE_RESET, 3600, None, false, None, "test").unwrap();
    skin::invites::activate(&inv.id).unwrap().expect("started");
    let r = post_door(front, "/.k2/auth/set-password", json!({"token": inv.token, "password": "reset-pass-77"})).await;
    assert_eq!(r.json(), json!({"ok": true}));

    // V15: another App's token and the owner's reset never open this door,
    // and nothing is used up.
    let other = skin::invites::mint(&pid, &s.other_svc, skin::invites::PURPOSE_RESET, 3600, None, false, None, "test").unwrap();
    skin::invites::activate(&other.id).unwrap().expect("started");
    let r = post_door(front, "/.k2/auth/set-password", json!({"token": other.token, "password": "x-pass-8888"})).await;
    assert_eq!((r.status, r.code()), (401, "bad_token".to_string()));
    assert!(door_store::token_target(&other.token).unwrap().is_some(), "still live for its own App");
    let own = skin::invites::mint(&pid, &s.svc, skin::invites::PURPOSE_OWNER_RESET, 3600, None, false, None, "owner-token").unwrap();
    skin::invites::activate(&own.id).unwrap().expect("started");
    let r = post_door(front, "/.k2/auth/set-password", json!({"token": own.token, "password": "x-pass-9999"})).await;
    assert_eq!(r.code(), "bad_token", "the owner's reset stays off the door");
    assert!(door_store::token_target(&own.token).unwrap().is_some());

    // The exchange on the App's gateway link, with the App pass.
    let spec = gateway::LinkSpec { app_id: s.app.clone(), service_id: s.svc.clone(), workspace_id: s.ws.id.clone(), port: 0, caps: vec![], rooms: vec![s.ws.id.clone()] };
    let app_pass = gateway::open(spec, Some(Arc::new(|_: &str| Ok(())))).expect("link");
    let link = gateway::port_of(&s.app).unwrap();
    let (v, c) = pkce("verifier-link");
    let code = sign_in(front, &s.user, "reset-pass-77", &c).await.json()["code"].as_str().unwrap().to_string();
    let body = json!({"code": code, "verifier": v}).to_string();
    let auth = format!("Authorization: Bearer {app_pass}");
    let r = tokio::task::spawn_blocking(move || {
        parse(&raw(link, &format!("POST /cli/skin/auth/exchange HTTP/1.1\r\nHost: 127.0.0.1\r\n{auth}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())))
    })
    .await
    .unwrap();
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.json()["token"].as_str().unwrap().starts_with("k2skn_"));
    assert!(!r.head.to_ascii_lowercase().contains("set-cookie"), "no cookie from the exchange");
    // Off a link: no such route.
    let off = post(d.port, "/cli/skin/auth/exchange", "", json!({"code": "k2lgn_x", "verifier": v})).await;
    assert_eq!(off.status, 404, "{}", off.body);

    assert!(!seen_text(&seen).contains("pass-"), "the App saw no password");
}

// ── every covered host, the switches, origins, limits ────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_app_never_sees_a_door_request_on_any_covered_host() {
    let env = setup(&[]);
    let _d = k2_daemon::test_harness::start(OWNER).await;
    let s = seed(&env, "v3");
    let (app_port, seen) = recording_app();
    router::switch_with(&s.app, app_port, true);
    door_on();
    let visitor: IpAddr = "203.0.113.7".parse().unwrap();
    let front = router::ensure_front(&tokio::runtime::Handle::current(), &s.app, 0, true).unwrap();
    let label = app_path(&s.app, Mode::Tunnel(Some(visitor))).await;
    let custom = app_path(&s.app, Mode::Door(Some(visitor))).await;
    let (_, c) = pkce("verifier-hosts");
    let sign = json!({"username": s.user, "password": PW, "challenge": c}).to_string();
    for (name, port) in [("front port", front), ("k2.dev label", label), ("custom name", custom)] {
        // First request on the connection.
        let r = parse(&tokio::task::spawn_blocking({
            let b = sign.clone();
            move || raw(port, &req_text("POST", "/.k2/auth/sign-in", &[JSON_CT], Some(&b)))
        })
        .await
        .unwrap());
        assert_eq!(r.status, 200, "{name}: {}", r.body);
        // Mid keep-alive, behind an App request.
        let b = sign.clone();
        let both = tokio::task::spawn_blocking(move || {
            let mut t = String::from("GET /before HTTP/1.1\r\nHost: shop.example.com\r\n\r\n");
            t.push_str(&format!("POST /.k2/auth/sign-in HTTP/1.1\r\nHost: shop.example.com\r\n{JSON_CT}\r\nContent-Length: {}\r\n\r\n{b}", b.len()));
            raw(port, &t)
        })
        .await
        .unwrap();
        assert!(both.contains("app") && both.contains("k2lgn_"), "{name}: {both}");
        // Misspellings land on the door's 404, never the App.
        let r = door(port, "GET", "/.K2/AUTH/sign-in", vec![], None).await;
        assert_eq!((r.status, r.code()), (404, "not_found".to_string()), "{name}");
    }
    let text = seen_text(&seen);
    assert_eq!(text.matches("GET /before").count(), 3, "{text}");
    assert!(!text.contains(PW) && !text.to_ascii_lowercase().contains("/.k2/auth"), "the App saw: {text}");

    // A raw App's k2.dev label has no door: ping is the App's own answer.
    // A custom name always rewrites, so it keeps the door.
    seen.lock().unwrap().clear();
    router::switch_with(&s.app, app_port, false);
    let r = door(label, "GET", "/.k2/auth/ping", vec![], None).await;
    assert_eq!(r.body, "app", "raw label: no door");
    let r = door(custom, "GET", "/.k2/auth/ping", vec![], None).await;
    assert_eq!(r.json()["k2"], true, "a custom name keeps the door: {}", r.body);
    router::switch_with(&s.app, app_port, true);

    // Off for this App only: the App owns its path again.
    door_store::set_off(&s.svc, true, "test").unwrap();
    auth_door::invalidate();
    let r = door(custom, "GET", "/.k2/auth/ping", vec![], None).await;
    assert_eq!(r.body, "app", "per-App off");
    door_store::set_off(&s.svc, false, "test").unwrap();
    auth_door::invalidate();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn origins_content_type_limits_and_a_suspended_app() {
    let env = setup(&[]);
    let _d = k2_daemon::test_harness::start(OWNER).await;
    let s = seed(&env, "v4");
    let (app_port, _seen) = recording_app();
    router::switch_with(&s.app, app_port, true);
    door_on();
    let custom = app_path(&s.app, Mode::Door(Some("198.51.100.9".parse().unwrap()))).await;
    let (_, c) = pkce("verifier-origins");
    let body = json!({"username": s.user, "password": PW, "challenge": c});

    let r = door(custom, "POST", "/.k2/auth/sign-in", vec![JSON_CT, "Origin: https://evil.example"], Some(body.clone())).await;
    assert_eq!((r.status, r.code()), (403, "bad_origin".to_string()), "a stranger's page");
    let r = door(custom, "POST", "/.k2/auth/sign-in", vec![JSON_CT, "Origin: https://shop.example.com"], Some(body.clone())).await;
    assert_eq!(r.status, 200, "the App's own page: {}", r.body);
    assert!(!r.head.contains("Access-Control-Allow-Origin"), "same host needs no CORS");
    let r = door(custom, "POST", "/.k2/auth/sign-in", vec!["Content-Type: text/plain"], Some(body.clone())).await;
    assert_eq!(r.status, 415, "{}", r.body);
    let r = door(custom, "GET", "/.k2/auth/sign-in", vec![], None).await;
    assert_eq!(r.status, 405);
    // A bundled shell's origin, once listed: CORS and a preflight.
    let r = door(custom, "OPTIONS", "/.k2/auth/sign-in", vec!["Origin: tauri://localhost"], None).await;
    assert_eq!(r.status, 403, "not listed yet");
    door_store::set_origins(&s.svc, &["tauri://localhost".to_string()], "test").unwrap();
    let r = door(custom, "OPTIONS", "/.k2/auth/sign-in", vec!["Origin: tauri://localhost"], None).await;
    assert_eq!(r.status, 204, "{}", r.head);
    assert!(r.head.contains("Access-Control-Allow-Origin: tauri://localhost"), "{}", r.head);
    let r = door(custom, "POST", "/.k2/auth/sign-in", vec![JSON_CT, "Origin: tauri://localhost"], Some(body.clone())).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.head.contains("Access-Control-Allow-Origin: tauri://localhost") && r.head.contains("Vary: Origin"));

    // Every attempt per (App, visitor) counts before any hash: 20, then 429.
    auth_door::reset_limits_for_tests();
    let junk = json!({"username": s.user, "password": PW});
    for i in 0..auth_door::ATTEMPTS_PER_VISITOR {
        let r = door(custom, "POST", "/.k2/auth/sign-in", vec![JSON_CT], Some(junk.clone())).await;
        assert_eq!(r.code(), "challenge_required", "attempt {i}");
    }
    let r = door(custom, "POST", "/.k2/auth/sign-in", vec![JSON_CT], Some(body.clone())).await;
    assert_eq!((r.status, r.code()), (429, "rate_limited".to_string()), "{}", r.body);
    assert!(r.head.contains("Retry-After: "));
    auth_door::reset_limits_for_tests();

    // A suspended App signs nobody in.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE apps SET suspended_at = 1 WHERE app_id = ?1", rusqlite::params![s.app]).unwrap();
    }
    let r = door(custom, "POST", "/.k2/auth/sign-in", vec![JSON_CT], Some(body)).await;
    assert_eq!((r.status, r.code()), (403, "app_suspended".to_string()), "{}", r.body);
    let _ = (&s.other_app, &s.other_svc);
}

// ── on the link: the door's status, a device refresh (0.46.11) ───────

/// Open `app`'s gateway link (any free port); its App pass and port.
fn open_link(ws: &Ws, app: &str, svc: &str) -> (String, u16) {
    let spec = gateway::LinkSpec { app_id: app.to_string(), service_id: svc.to_string(), workspace_id: ws.id.clone(), port: 0, caps: vec![], rooms: vec![ws.id.clone()] };
    let pass = gateway::open(spec, Some(Arc::new(|_: &str| Ok(())))).expect("link");
    (pass, gateway::port_of(app).expect("link port"))
}

/// One request on a gateway link, with `pass` as the Bearer (none when empty).
async fn on_link(link: u16, method: &'static str, path: &'static str, pass: &str, body: Option<Value>) -> R {
    let auth = if pass.is_empty() { String::new() } else { format!("Authorization: Bearer {pass}\r\n") };
    tokio::task::spawn_blocking(move || {
        let mut t = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n{auth}Connection: close\r\n");
        match body.map(|v| v.to_string()) {
            Some(b) => t.push_str(&format!("{JSON_CT}\r\nContent-Length: {}\r\n\r\n{b}", b.len())),
            None => t.push_str("\r\n"),
        }
        parse(&raw(link, &t))
    })
    .await
    .unwrap()
}

const DOOR_STATUS: &str = "/cli/skin/auth/door";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn door_status_on_the_link_answers_for_the_links_own_app_only() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let s = seed(&env, "v7");
    let (app_pass, link) = open_link(&s.ws, &s.app, &s.svc);
    let (other_pass, other_link) = open_link(&s.ws, &s.other_app, &s.other_svc);

    // Off (the default): the box switch is off, so no door anywhere.
    let r = on_link(link, "GET", DOOR_STATUS, &app_pass, None).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert_eq!(
        (v["ok"].as_bool(), v["on"].as_bool(), v["box"].as_bool(), v["appOff"].as_bool(), v["app"].as_str(), v["door"].as_str()),
        (Some(true), Some(false), Some(false), Some(false), Some(s.app.as_str()), Some("/.k2/auth")),
        "{v}"
    );
    let hosts = v["hosts"].as_array().unwrap_or_else(|| panic!("hosts: {v}"));
    assert!(hosts.iter().all(|h| h["door"] == json!(false) && h["host"].is_string()), "{v}");

    // The owner turns the box's door on: on for this App.
    door_on();
    let v = on_link(link, "GET", DOOR_STATUS, &app_pass, None).await.json();
    assert_eq!((v["on"].as_bool(), v["box"].as_bool(), v["appOff"].as_bool()), (Some(true), Some(true), Some(false)), "{v}");

    // One App's own switch shows on its own link only.
    door_store::set_off(&s.other_svc, true, "test").expect("off");
    auth_door::invalidate();
    let o = on_link(other_link, "GET", DOOR_STATUS, &other_pass, None).await.json();
    assert_eq!((o["on"].as_bool(), o["appOff"].as_bool(), o["app"].as_str()), (Some(false), Some(true), Some(s.other_app.as_str())), "{o}");
    let v = on_link(link, "GET", DOOR_STATUS, &app_pass, None).await.json();
    assert_eq!((v["on"].as_bool(), v["appOff"].as_bool(), v["app"].as_str()), (Some(true), Some(false), Some(s.app.as_str())), "{v}");

    // Never another App's state: this App's pass on the other App's link
    // is refused by the gate, and nothing in the request picks the App.
    let r = on_link(other_link, "GET", DOOR_STATUS, &app_pass, None).await;
    assert_eq!((r.status, r.code()), (403, "pass_wrong_link".to_string()), "{}", r.body);
    let r = on_link(link, "GET", "/cli/skin/auth/door?app=aov7", &app_pass, None).await;
    assert_eq!((r.status, r.json()["app"].as_str()), (200, Some(s.app.as_str())), "a query never names another App: {}", r.body);
    // No pass: the link's pass_required.
    let r = on_link(link, "GET", DOOR_STATUS, "", None).await;
    assert_eq!((r.status, r.code()), (401, "pass_required".to_string()), "{}", r.body);

    // A guest's own session of this App: the App's own data is the App pass's.
    let dev = skin::app_login::Device { id: "install-v7".into(), name: "Laptop".into() };
    let ds = skin::app_login::login_device(&s.user, &s.svc, &dev).expect("device sign-in").expect("stay signed in is on by default");
    let r = on_link(link, "GET", DOOR_STATUS, &ds.session.raw, None).await;
    assert_eq!((r.status, r.code()), (403, "not_exposed".to_string()), "{}", r.body);

    // A write is refused: it is a read.
    let r = on_link(link, "POST", DOOR_STATUS, &app_pass, Some(json!({}))).await;
    assert!(r.status == 404 || r.status == 405, "GET only on the link: {} {}", r.status, r.body);

    // Off a link: no such route, even for the owner.
    let r = get(d.port, DOOR_STATUS, OWNER, "").await;
    assert_eq!(r.status, 404, "{}", r.body);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_device_refresh_on_the_link_takes_the_app_pass_and_only_that_apps_tokens() {
    let env = setup(&[]);
    let _d = k2_daemon::test_harness::start(OWNER).await;
    let s = seed(&env, "v8");
    let (app_pass, link) = open_link(&s.ws, &s.app, &s.svc);
    let (other_pass, other_link) = open_link(&s.ws, &s.other_app, &s.other_svc);
    let ada = "adav8";
    skin::add_principal_for_app(ada, None, &s.other_svc, None, "test").expect("guest of the other App");
    skin::set_principal_password(ada, Some(PW)).expect("password");
    let dev = |id: &str| skin::app_login::Device { id: id.into(), name: "Laptop".into() };
    let mine = skin::app_login::login_device(&s.user, &s.svc, &dev("install-a")).expect("sign-in").expect("stay signed in");
    let theirs = skin::app_login::login_device(ada, &s.other_svc, &dev("install-b")).expect("sign-in").expect("stay signed in");

    // The link wants a pass even for this Public route: none is pass_required.
    let r = on_link(link, "POST", "/cli/skin/refresh", "", Some(json!({"refresh": mine.refresh}))).await;
    assert_eq!((r.status, r.code()), (401, "pass_required".to_string()), "{}", r.body);

    // With the App pass and no `app` (the BFF's call): a new pass and the
    // next refresh token, with the documented fields (seconds since the epoch).
    let r = on_link(link, "POST", "/cli/skin/refresh", &app_pass, Some(json!({"refresh": mine.refresh}))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    assert!(v["token"].as_str().is_some_and(|t| t.starts_with("k2skn_") && t != mine.session.raw), "{v}");
    let next = v["refresh"].as_str().unwrap_or_else(|| panic!("refresh: {v}")).to_string();
    assert!(next.starts_with("k2skr_") && next != mine.refresh, "{v}");
    let pass_exp = v["passExpiresAt"].as_i64().unwrap_or_else(|| panic!("passExpiresAt: {v}"));
    let refresh_exp = v["refreshExpiresAt"].as_i64().unwrap_or_else(|| panic!("refreshExpiresAt: {v}"));
    assert!(pass_exp > now && pass_exp < now + 400 * 86_400, "seconds since the epoch: {v}");
    assert!(refresh_exp >= pass_exp, "{v}");
    assert_eq!((v["app"].as_str(), v["device"]["id"].as_str()), (Some(s.svc.as_str()), Some("install-a")), "{v}");

    // Another App's refresh token through this App's link: refused, even
    // when the body names that App; the token itself isn't used up.
    let r = on_link(link, "POST", "/cli/skin/refresh", &app_pass, Some(json!({"refresh": theirs.refresh, "app": s.other_svc}))).await;
    assert_eq!((r.status, r.code()), (401, "refresh_invalid".to_string()), "{}", r.body);
    let r = on_link(link, "POST", "/cli/skin/refresh", &app_pass, Some(json!({"refresh": theirs.refresh}))).await;
    assert_eq!((r.status, r.code()), (401, "refresh_invalid".to_string()), "{}", r.body);
    let r = on_link(other_link, "POST", "/cli/skin/refresh", &other_pass, Some(json!({"refresh": theirs.refresh}))).await;
    assert_eq!((r.status, r.json()["app"].as_str()), (200, Some(s.other_svc.as_str())), "still good on its own App's link: {}", r.body);

    // Reuse ends the device's sign-in (unchanged).
    let r = on_link(link, "POST", "/cli/skin/refresh", &app_pass, Some(json!({"refresh": mine.refresh}))).await;
    assert_eq!((r.status, r.code()), (401, "refresh_reused".to_string()), "{}", r.body);
}
