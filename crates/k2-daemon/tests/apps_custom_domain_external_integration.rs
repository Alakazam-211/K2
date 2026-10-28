//! An App behind the owner's own web server (0.46.6 parity with 0.46.5),
//! headless through the REAL dispatcher on a temp `$HOME`:
//!
//! - a custom name bound to the App without `--external` waits for DNS and
//!   is no origin (CD-A4): the gateway's form check refuses its Origin;
//! - `k2 app domain add <app> <name> --external` records it `external`:
//!   the form check takes it at once, and `K2_APP_ORIGINS` carries it from
//!   the App's next start;
//! - `k2 app domain external <app> <name> off` hands it back to K2's flow
//!   (no origin until live);
//! - an agent without "Build and run Apps" gets a held request (nothing
//!   changes), another workspace's agent never sees the App.
//!
//! Air-gap is on: K2 makes no DNS check from here (a name handed back to
//! K2's flow would otherwise be checked against public DNS at once).

#[path = "support/app_fixture.rs"]
mod app_fixture;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use app_fixture::*;
use k2_core::db::schema::WorkspaceSession;
use k2_daemon::apps::gateway::{self, LinkSpec};
use serde_json::{json, Value};

const HOST: &str = "shop2.example.com";
const HOST3: &str = "shop3.example.com";

fn parse(text: &str) -> (u16, String) {
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|x| x.parse().ok())
        .unwrap_or_else(|| panic!("no status: {text:?}"));
    (status, text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default())
}

/// A browser form the App's own page forwards (the App pass as Bearer,
/// `Origin: https://<host>`, same-origin), on a fresh link connection.
fn same_origin_form(port: u16, cred: &str, host: &str, body: &str) -> (u16, String) {
    let req = format!(
        "POST /cli/thread/post HTTP/1.1\r\nHost: 127.0.0.1:8730\r\nAuthorization: Bearer {cred}\r\n\
         Content-Type: application/x-www-form-urlencoded\r\nOrigin: https://{host}\r\nSec-Fetch-Site: same-origin\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    s.write_all(req.as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    parse(&String::from_utf8_lossy(&out))
}

fn err_code(body: &str) -> String {
    let v: Value = serde_json::from_str(body).unwrap_or_else(|e| panic!("JSON ({e}): {body}"));
    v["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {body}")).to_string()
}

fn published(ws: &Ws, name: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let dir = ws.path.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    k2_core::published_services::insert(&conn, &ws.id, name, "node server.mjs", &dir.to_string_lossy(), 47_031, "local", "stopped", "cmd", "")
        .expect("insert service")
        .id
}

fn install_row(app: &str, svc: &str, ws: &Ws, approved: &[&str]) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::deploy::store::insert_app(
        &conn,
        &k2_core::deploy::store::NewApp {
            app_id: app,
            service_id: svc,
            seed_id: None,
            owner_workspace: &ws.id,
            isolation: "container",
            memory_limit_bytes: None,
            front_port: 0,
            created_by: "owner",
        },
    )
    .unwrap();
    conn.execute(
        "UPDATE apps SET approved_caps_json = ?2 WHERE app_id = ?1",
        rusqlite::params![app, serde_json::to_string(approved).unwrap()],
    )
    .unwrap();
}

fn pin_chat(ws: &Ws) {
    let conv = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &ws.id, None, Some(&conv), "claude", "system", "running").expect("pin");
}

fn attach_example_com() {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::domains::upsert_binding(&conn, "example.com", None, false).expect("apex");
}

fn state_of(host: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::domains::app_domain(&conn, host).expect("read").unwrap_or_else(|| panic!("{host} is not bound")).state
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_external_name_is_an_origin_for_the_gateways_form_check_and_off_hands_it_back() {
    let env = setup(&[("K2_AIRGAP", "1")]);
    k2_daemon::apps::alerts::set_sink(Some(Box::new(|_: &str, _: &str| {})));
    let d = k2_daemon::test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "shopws");
    pin_chat(&ws);
    let svc = published(&ws, "shopapp");
    install_row("shopapp", &svc, &ws, &["thread:read", "thread:post"]);
    attach_example_com();
    let pass = gateway::open(
        LinkSpec {
            app_id: "shopapp".into(),
            service_id: svc.clone(),
            workspace_id: ws.id.clone(),
            port: 0,
            caps: vec!["thread:read".into(), "thread:post".into()],
            rooms: vec![ws.id.clone()],
        },
        None,
    )
    .unwrap();
    let port = gateway::port_of("shopapp").unwrap();
    let form = format!("addr={}&text=hello+from+shop2", enc(&ws.handle));
    let try_form = |p: String, f: String| tokio::task::spawn_blocking(move || same_origin_form(port, &p, HOST, &f));

    // Bound without --external: waiting_dns, no origin (CD-A4).
    let r = post(d.port, "/cli/app/domain/add", OWNER, json!({ "app": "shopapp", "label": HOST })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert_eq!(v["custom"]["state"], "waiting_dns", "{v}");
    assert_eq!(v["custom"]["notServedYet"], true, "{v}");
    assert_eq!(
        v["custom"]["ownServer"],
        format!("If your own web server serves this name, run: k2 app domain add shopapp {HOST} --external"),
        "this box has no K2 front: {v}"
    );
    let (st, body) = try_form(pass.clone(), form.clone()).await.unwrap();
    assert_eq!((st, err_code(&body)), (403, "browser_form_refused".to_string()), "not an origin yet: {body}");

    // The App's own agent without "Build and run Apps": a held request,
    // nothing changes (the CLI exits 2, "held for the owner").
    allow_build_only(&ws, false);
    let agent = passport(&ws);
    let r = post(d.port, "/cli/app/domain/add", &agent, json!({ "app": "shopapp", "label": HOST, "external": true })).await;
    assert_eq!(r.status, 202, "{}", r.body);
    let held = r.json();
    let summary = held["request"]["summary"].as_str().unwrap_or_else(|| panic!("no request.summary: {held}"));
    assert_eq!(summary, format!("k2 app domain add shopapp {HOST} --external"), "{held}");
    assert_eq!(state_of(HOST), "waiting_dns", "a held request changes nothing");

    // Another workspace's agent (with its switch on) never sees the App
    // (404: the CLI exits 3).
    let other = seed_ws(&env, "otherws");
    allow_build_only(&other, true);
    let r = post(d.port, "/cli/app/domain/external", &passport(&other), json!({ "app": "shopapp", "host": HOST, "external": true })).await;
    assert_eq!((r.status, r.code()), (404, "app_not_found".to_string()), "{}", r.body);
    assert_eq!(state_of(HOST), "waiting_dns");

    // --external from the owner: external, an origin at once.
    let r = post(d.port, "/cli/app/domain/add", OWNER, json!({ "app": "shopapp", "label": HOST, "external": true })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert_eq!((v["custom"]["state"].as_str(), v["external"].as_bool()), (Some("external"), Some(true)), "{v}");
    assert!(v["note"].as_str().expect("note").contains("k2 app restart shopapp"), "{v}");
    assert_eq!(
        k2_daemon::apps::origins::for_service(&svc, "shopapp"),
        vec![format!("https://{HOST}")],
        "what K2_APP_ORIGINS carries at the next start"
    );
    let (st, body) = try_form(pass.clone(), form.clone()).await.unwrap();
    assert_eq!(st, 200, "a same-origin form from the external name: {body}");
    let list = get(d.port, "/cli/app/domains", OWNER, "app=shopapp").await;
    assert_eq!(list.status, 200, "{}", list.body);
    let c = list.json()["custom"][0].clone();
    assert_eq!((c["hostname"].as_str(), c["state"].as_str(), c["external"].as_bool()), (Some(HOST), Some("external"), Some(true)), "{c}");

    // Off: back to K2's flow, no origin until live.
    let r = post(d.port, "/cli/app/domain/external", OWNER, json!({ "app": "shopapp", "host": HOST, "external": false })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["custom"]["state"], "waiting_dns", "{}", r.body);
    assert!(k2_daemon::apps::origins::for_service(&svc, "shopapp").is_empty());
    let (st, body) = try_form(pass.clone(), form.clone()).await.unwrap();
    assert_eq!((st, err_code(&body)), (403, "browser_form_refused".to_string()), "handed back: {body}");

    // A GET on the POST-only route: 405.
    let r = get(d.port, "/cli/app/domain/external", OWNER, "app=shopapp").await;
    assert_eq!(r.status, 405, "{}", r.body);

    gateway::close("shopapp", None, true);
    k2_daemon::apps::alerts::set_sink(None);
}

async fn restart(port: u16, app: &str) {
    let r = post(port, "/cli/app/restart", OWNER, json!({"app": app, "wait": 60})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["deployment"]["result"], "live", "{}", r.body);
}

fn env_val(front: u16, name: &str) -> Option<String> {
    match http_get(front, &format!("/envval/{name}")).unwrap_or_else(|| panic!("the App on {front} didn't answer")) {
        (200, b) => Some(b),
        (404, _) => None,
        other => panic!("/envval/{name}: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_external_name_reaches_k2_app_origins_at_the_next_start() {
    require_python3();
    let env = setup(&[("K2_AIRGAP", "1")]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "originws");
    let r = post(d.port, "/cli/app/install", OWNER, json!({"app": "shopnext", "workspace": ws.id, "engine": "process"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let front = r.json()["frontPort"].as_u64().expect("frontPort") as u16;
    let v1 = app_folder(&env.path().join("v1"), "shopnext", "1.0.0", "v1");
    let r = post(d.port, "/cli/app/deploy", OWNER, json!({"app": "shopnext", "folder": v1, "wait": 60})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["deployment"]["result"], "live", "{}", r.body);
    assert_eq!(env_val(front, "K2_APP_ORIGINS"), None, "local-only, no domain: no origin");
    attach_example_com();

    // Without --external: bound, not served, still no origin after a start.
    let r = post(d.port, "/cli/app/domain/add", OWNER, json!({ "app": "shopnext", "label": HOST3 })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    restart(d.port, "shopnext").await;
    assert_eq!(env_val(front, "K2_APP_ORIGINS"), None, "a waiting name is no origin (CD-A4)");

    // --external: in K2_APP_ORIGINS from the next start.
    let r = post(d.port, "/cli/app/domain/add", OWNER, json!({ "app": "shopnext", "label": HOST3, "external": true })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(env_val(front, "K2_APP_ORIGINS"), None, "a running process keeps its start values");
    restart(d.port, "shopnext").await;
    let origins = env_val(front, "K2_APP_ORIGINS").expect("K2_APP_ORIGINS is set");
    assert!(origins.split(',').any(|o| o == format!("https://{HOST3}")), "{origins}");

    // Off: gone again at the next start.
    let r = post(d.port, "/cli/app/domain/external", OWNER, json!({ "app": "shopnext", "host": HOST3, "external": false })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    restart(d.port, "shopnext").await;
    assert_eq!(env_val(front, "K2_APP_ORIGINS"), None, "handed back to K2's flow: no origin until live");

    let r = post(d.port, "/cli/app/stop", OWNER, json!({"app": "shopnext"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
}
