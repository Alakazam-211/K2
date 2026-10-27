//! App phone pairing, headless (prd-app-phone-pairing-v1 §11): the real
//! dispatcher in process on a temp `$HOME`, driven over loopback.
//!
//! - `scripts/app-pairing-smoke.sh` (curl, as a web App and a phone app
//!   call it) runs against this daemon: start → status → the App host's
//!   link files through its front port (the App down) → check → redeem →
//!   refresh → devices → revoke.
//! - Only a guest's own pass starts (the owner token is `guest_required`);
//!   the answers carrying a code or a pass are never cached.
//! - Web access off: the phone's check and redeem still work through the
//!   edge's markers (V20).
//! - Bad codes are throttled per client on pairing's own buckets, and never
//!   lock out the person's App login (R4).
//! - The box host keeps Companion's AASA and has no `/.well-known/k2-pair`.

#[path = "support/app_fixture.rs"]
mod app_fixture;

use std::io::{Read, Write};
use std::time::Duration;

use app_fixture::*;
use k2_core::skin::{self, app_pairing as store};
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
    fn code(&self) -> String {
        self.json()["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {}", self.body)).to_string()
    }
}

fn raw(port: u16, method: &str, path: &str, headers: &[String], body: Option<&str>) -> Raw {
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
    for h in headers {
        req.push_str(h);
        req.push_str("\r\n");
    }
    match body {
        Some(b) => req.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}", b.len())),
        None if method == "POST" => req.push_str("Content-Length: 0\r\n\r\n"),
        None => req.push_str("\r\n"),
    }
    s.write_all(req.as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    let text = String::from_utf8_lossy(&out).to_string();
    let status = text.lines().next().and_then(|l| l.split_whitespace().nth(1)).and_then(|x| x.parse().ok()).unwrap_or_else(|| panic!("no status: {text:?}"));
    let (head, body) = text.split_once("\r\n\r\n").map(|(h, b)| (h.to_string(), b.to_string())).unwrap_or_default();
    Raw { status, head, body }
}

async fn call(port: u16, method: &'static str, path: String, headers: Vec<String>, body: Option<String>) -> Raw {
    tokio::task::spawn_blocking(move || raw(port, method, &path, &headers, body.as_deref())).await.unwrap()
}

/// A runtime App `pj<tag>` (tunnelled, box `box`) in workspace `pw<tag>`,
/// guest `dana<tag>` (password `s3cret-horse`) with its grant, pairing on
/// with one iOS app and one Android app. The daemon DB is one per test
/// process, so every name carries the test's tag. Returns (service id,
/// App id, username).
fn seed(env: &Env, tag: &str) -> (String, String, String) {
    let cfg = k2_core::tunnel::config::TunnelConfig { subdomain: "box".into(), ..Default::default() };
    k2_core::tunnel::config::save(&cfg).expect("tunnel config");
    let ws = seed_ws(env, &format!("pw{tag}"));
    let app_id = format!("pj{tag}");
    let user = format!("dana{tag}");
    let svc = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let s = k2_core::published_services::insert(&conn, &ws.id, &app_id, "node s.js", "/tmp", 4390, "tunnel", "running", "cmd", "").expect("svc");
        k2_core::deploy::store::insert_app(
            &conn,
            &k2_core::deploy::store::NewApp {
                app_id: &app_id,
                service_id: &s.id,
                seed_id: None,
                owner_workspace: &ws.id,
                isolation: "process",
                memory_limit_bytes: None,
                front_port: 0,
                created_by: "test",
            },
        )
        .expect("runtime app");
        s.id
    };
    skin::add_principal_for_app(&user, None, &svc, None, "test").expect("guest");
    skin::set_principal_password(&user, Some("s3cret-horse")).expect("password");
    let input = store::ConfigInput {
        ios: vec!["ABCDE12345.com.jobs.app".into()],
        android: vec![store::AndroidApp {
            package: "com.jobs.app".into(),
            sha256: vec!["AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89".into()],
        }],
        ..Default::default()
    };
    store::set_config(&svc, &input, "owner").expect("pairing on");
    k2_daemon::app_pairing::refresh_links(&svc);
    (svc, app_id, user)
}

async fn web_pass(port: u16, svc: &str, user: &str) -> String {
    let r = call(port, "POST", "/cli/skin/login".into(), vec![], Some(json!({"username": user, "password": "s3cret-horse", "app": svc}).to_string())).await;
    assert_eq!(r.status, 200, "login: {}", r.body);
    r.json()["token"].as_str().expect("token").to_string()
}

fn bearer(t: &str) -> String {
    format!("Authorization: Bearer {t}")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_curl_smoke_pairs_a_phone_headless() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let (svc, app_id, user) = seed(&env, "smoke");
    let pass = web_pass(d.port, &svc, &user).await;
    // The App is down: its front port still serves the link files (V5).
    let front = k2_daemon::apps::router::ensure_front(&tokio::runtime::Handle::current(), &app_id, 0, true).expect("front port");
    let script = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts/app-pairing-smoke.sh");
    let out = tokio::task::spawn_blocking({
        let (svc, pass, host) = (svc.clone(), pass.clone(), format!("{app_id}.box.k2.dev"));
        move || {
            std::process::Command::new("bash")
                .arg(&script)
                .env("K2_URL", format!("http://127.0.0.1:{}", d.port))
                .env("K2_APP", &svc)
                .env("K2_PASS", &pass)
                .env("K2_FRONT", format!("http://127.0.0.1:{front}"))
                .env("K2_APP_HOST", &host)
                .output()
                .expect("run the smoke script")
        }
    })
    .await
    .unwrap();
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "the curl smoke failed:\n{text}");
    assert!(text.contains("PASS:"), "{text}");
    assert!(!text.contains(&pass), "the script never prints a pass");
    k2_daemon::apps::router::close_front(&app_id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_a_guest_starts_web_off_never_walls_the_phone_and_bad_codes_are_throttled_on_their_own() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let (svc, _, user) = seed(&env, "gate");
    let pass = web_pass(d.port, &svc, &user).await;
    // The owner token is not a person's web sign-in.
    let r = call(d.port, "POST", format!("/cli/skin/pair/start?token={OWNER}"), vec![], Some("{}".into())).await;
    assert_eq!((r.status, r.code()), (403, "guest_required".to_string()), "{}", r.body);
    let r = call(d.port, "POST", "/cli/skin/pair/start".into(), vec![], Some("{}".into())).await;
    assert_eq!((r.status, r.code()), (401, "pass_required".to_string()), "{}", r.body);
    // Web access off, the phone's first contact through the edge.
    let r = call(
        d.port,
        "POST",
        format!("/cli/settings/update?token={OWNER}"),
        vec![],
        Some(r#"{"webClientEnabled":false}"#.into()),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    let started = call(d.port, "POST", "/cli/skin/pair/start".into(), vec![bearer(&pass)], Some("{}".into())).await;
    assert_eq!(started.status, 200, "{}", started.body);
    assert!(started.head.to_ascii_lowercase().contains("cache-control: no-store"), "{}", started.head);
    let code = started.json()["code"].as_str().expect("code").to_string();
    let edge = vec!["X-K2-Client: web".to_string(), "X-Forwarded-Host: box.app.k2.dev".to_string()];
    let r = call(d.port, "POST", "/cli/skin/pair/check".into(), edge.clone(), Some(json!({"code": code, "app": svc}).to_string())).await;
    assert_eq!(r.status, 200, "check through the edge with web access off: {}", r.body);
    let r = call(
        d.port,
        "POST",
        "/cli/skin/pair".into(),
        edge,
        Some(json!({"code": code, "app": svc, "device": {"id": "edge-1", "name": "Phone"}}).to_string()),
    )
    .await;
    assert_eq!(r.status, 200, "redeem through the edge with web access off: {}", r.body);
    assert!(r.head.to_ascii_lowercase().contains("cache-control: no-store"), "{}", r.head);
    // Bad codes from one client: pairing's own bucket answers 429 after its
    // limit, and the person's App login is untouched.
    let client = vec!["X-Forwarded-For: 198.51.100.7".to_string()];
    let mut statuses = Vec::new();
    for _ in 0..12 {
        let r = call(d.port, "POST", "/cli/skin/pair/check".into(), client.clone(), Some(json!({"code": "K2A-00000-00000-00000-00000", "app": svc}).to_string())).await;
        statuses.push(r.status);
    }
    assert!(statuses[..10].iter().all(|s| *s == 401), "{statuses:?}");
    assert!(statuses[10..].iter().all(|s| *s == 429), "{statuses:?}");
    let r = call(d.port, "POST", "/cli/skin/login".into(), client, Some(json!({"username": user, "password": "s3cret-horse", "app": svc}).to_string())).await;
    assert_eq!(r.status, 200, "bad codes never lock out the App login: {}", r.body);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_box_host_keeps_companions_files_and_has_no_pair_path() {
    let _env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let r = call(d.port, "GET", "/.well-known/apple-app-site-association".into(), vec![], None).await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains(k2_daemon::pair_page::COMPANION_APP_ID) && !r.body.contains("k2-pair"), "{}", r.body);
    let r = call(d.port, "GET", "/.well-known/k2-pair".into(), vec![], None).await;
    assert_eq!(r.status, 404, "the dispatcher has no App pairing page on the box host: {}", r.body);
}
