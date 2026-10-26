//! The App gateway link takes the body encodings the daemon's handlers
//! read (0.46.4 field report: an App moved onto the runtime posted
//! form-encoded to `POST /cli/thread/post` with its guest's pass in
//! `?token=`; the link answered `415 json_required`, the old daemon path
//! took it, the App's UI silently did nothing, and the audit named the
//! caller `app`), headless: the real dispatcher in process on a temp
//! `$HOME`, an App's link opened the way the guarded engine opens one.
//!
//! - a form body reaches the handler on the routes whose handler reads a
//!   form (`k2_core::app_body::FORM_POSTS`): a Thread post lands;
//! - a form body on a JSON-only write is `415 json_required` with a hint
//!   that says the route reads JSON only; another type names both;
//! - a guest's pass in `?token=` is audited as the guest, refusals too;
//! - `k2 app warnings` names the `?token=` use (deprecated) and the
//!   refused writes from the audit.

#![cfg(unix)]

#[path = "support/app_fixture.rs"]
mod app_fixture;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use app_fixture::*;
use k2_core::db::schema::WorkspaceSession;
use k2_daemon::apps::gateway::{self, LinkSpec};
use serde_json::Value;

fn parse(text: &str) -> (u16, String) {
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|x| x.parse().ok())
        .unwrap_or_else(|| panic!("no status: {text:?}"));
    (status, text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default())
}

/// One request on a fresh link connection: `auth` is `Bearer` (header) or
/// `Query` (`?token=`); `ct` the Content-Type of `body`.
fn on_link(port: u16, path: &str, cred: &str, in_query: bool, ct: &str, body: &str) -> (u16, String) {
    let (pq, auth) = if in_query {
        let sep = if path.contains('?') { '&' } else { '?' };
        (format!("{path}{sep}token={}", enc(cred)), String::new())
    } else {
        (path.to_string(), format!("Authorization: Bearer {cred}\r\n"))
    };
    let req = format!(
        "POST {pq} HTTP/1.1\r\nHost: 127.0.0.1:8730\r\n{auth}Content-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    s.write_all(req.as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    parse(&String::from_utf8_lossy(&out))
}

/// [`on_link`] with extra header lines (each ends in `\r\n`), the App
/// pass as Bearer.
fn on_link_with(port: u16, path: &str, cred: &str, ct: &str, body: &str, extra: &str) -> (u16, String) {
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:8730\r\nAuthorization: Bearer {cred}\r\nContent-Type: {ct}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    s.write_all(req.as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    parse(&String::from_utf8_lossy(&out))
}

fn get_on_link(port: u16, pq: &str, cred: &str) -> (u16, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    s.write_all(format!("GET {pq} HTTP/1.1\r\nHost: 127.0.0.1:8730\r\nAuthorization: Bearer {cred}\r\nConnection: close\r\n\r\n").as_bytes())
        .unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    parse(&String::from_utf8_lossy(&out))
}

fn err(body: &str) -> Value {
    let v: Value = serde_json::from_str(body).unwrap_or_else(|e| panic!("JSON ({e}): {body}"));
    v["error"].clone()
}

fn published(ws: &Ws, name: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let dir = ws.path.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    k2_core::published_services::insert(&conn, &ws.id, name, "node server.mjs", &dir.to_string_lossy(), 47_021, "local", "stopped", "cmd", "")
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn form_bodies_reach_the_handlers_that_read_them_and_query_passes_are_the_guest() {
    let env = setup(&[]);
    k2_daemon::apps::alerts::set_sink(Some(Box::new(|_: &str, _: &str| {})));
    let d = k2_daemon::test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "formws");
    pin_chat(&ws);
    let svc = published(&ws, "formapp");
    install_row("formapp", &svc, &ws, &["thread:read", "thread:post"]);
    // The App's own public origin (a custom hostname bound to it): what its
    // K2_APP_ORIGINS carries.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::domains::upsert_binding(&conn, "example.com", None, false).expect("apex");
        k2_core::domains::upsert_name(&conn, "formapp.example.com", "example.com", "app:formapp").expect("name");
    }
    k2_core::skin::add_principal_for_app("gina", None, &svc, None, "owner").unwrap();
    let guest = k2_core::skin::create_app_session_token("gina", &svc).unwrap();
    let pass = gateway::open(
        LinkSpec {
            app_id: "formapp".into(),
            service_id: svc.clone(),
            workspace_id: ws.id.clone(),
            port: 0,
            caps: vec!["thread:read".into(), "thread:post".into()],
            rooms: vec![ws.id.clone()],
        },
        None,
    )
    .unwrap();
    let port = gateway::port_of("formapp").unwrap();

    const FORM: &str = "application/x-www-form-urlencoded";
    let handle = ws.handle.clone();
    let (p, g) = (pass.clone(), guest.raw.clone());
    let (app_form, guest_form, guest_json_only, plain, read) = tokio::task::spawn_blocking(move || {
        let form = format!("addr={}&text=hello+from+a+form", enc(&handle));
        // The App's own pass in ?token=, a form post: the handler reads it.
        let app_form = on_link(port, "/cli/thread/post", &p, true, &format!("{FORM}; charset=UTF-8"), &form);
        // The guest's session in ?token=, a form post: past the gate (the
        // guest holds no room here, so the handler answers, not the gate).
        let guest_form = on_link(port, "/cli/thread/post", &g, true, FORM, &form);
        // A form on a JSON-only write: 415, the hint says JSON only.
        let guest_json_only = on_link(port, "/cli/feedback/create", &g, true, FORM, "addr=x&title=y");
        // Neither JSON nor a form: 415, the hint names both.
        let plain = on_link(port, "/cli/thread/post", &p, false, "text/plain", "hi");
        let read = get_on_link(port, &format!("/cli/thread?addr={}", enc(&handle)), &p);
        // Security review: a reverse proxy forwarding the App's own page's
        // form (Origin = its own origin) is allowed; another site's page
        // (cross-site, or a foreign Origin) is refused.
        let fwd = on_link_with(port, "/cli/thread/post", &p, FORM, &form, "Origin: https://formapp.example.com\r\nSec-Fetch-Site: same-origin\r\n");
        assert_eq!(fwd.0, 200, "a forwarded same-origin form: {}", fwd.1);
        for extra in ["Origin: https://formapp.example.com\r\nSec-Fetch-Site: cross-site\r\n", "Origin: https://evil.example\r\n"] {
            let r = on_link_with(port, "/cli/thread/post", &p, FORM, &form, extra);
            assert_eq!((r.0, err(&r.1)["code"].as_str().map(str::to_string)), (403, Some("browser_form_refused".to_string())), "{extra}: {}", r.1);
        }
        (app_form, guest_form, guest_json_only, plain, read)
    })
    .await
    .unwrap();

    assert_eq!(app_form.0, 200, "a form Thread post on the link: {}", app_form.1);
    assert_eq!(read.0, 200, "{}", read.1);
    assert!(read.1.contains("hello from a form"), "the form's text landed: {}", read.1);
    assert_ne!(guest_form.0, 415, "a guest's form post reaches the handler: {}", guest_form.1);
    assert_ne!(err(&guest_form.1)["code"], "json_required", "{}", guest_form.1);
    assert_eq!((guest_json_only.0, err(&guest_json_only.1)["code"].as_str()), (415, Some("json_required")), "{}", guest_json_only.1);
    let hint = err(&guest_json_only.1)["hint"].as_str().unwrap().to_string();
    assert!(hint.starts_with("/cli/feedback/create reads JSON only"), "{hint}");
    assert_eq!((plain.0, err(&plain.1)["code"].as_str()), (415, Some("json_required")), "{}", plain.1);
    assert!(err(&plain.1)["hint"].as_str().unwrap().contains("application/x-www-form-urlencoded"), "{}", plain.1);

    // The audit: the guest's ?token= calls are the guest, the refusal too.
    let mut rows: Vec<k2_core::app_audit::Row> = Vec::new();
    for _ in 0..50 {
        k2_daemon::apps::audit::flush();
        let c = k2_core::app_audit::open(&k2_core::app_audit::db_path()).unwrap();
        rows = k2_core::app_audit::list(&c, &k2_core::app_audit::Query { app_id: "formapp", limit: 100, ..Default::default() }).unwrap();
        if rows.len() >= 5 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let refused = rows
        .iter()
        .find(|r| r.call.verb == "POST /cli/feedback/create")
        .unwrap_or_else(|| panic!("no feedback/create row: {:?}", rows.iter().map(|r| &r.call.verb).collect::<Vec<_>>()));
    assert_eq!(refused.call.code.as_deref(), Some("json_required"), "{:?}", refused.call);
    assert!(refused.call.actor.starts_with("guest:"), "a guest's ?token= refusal is the guest: {:?}", refused.call);
    let guest_posts: Vec<&str> = rows
        .iter()
        .filter(|r| r.call.verb == "POST /cli/thread/post")
        .map(|r| r.call.actor.as_str())
        .collect();
    assert!(guest_posts.iter().any(|a| a.starts_with("guest:")), "{guest_posts:?}");
    assert!(guest_posts.iter().any(|a| *a == "app"), "{guest_posts:?}");
    for r in &rows {
        assert!(!r.call.verb.contains("token="), "the audit never stores the query: {}", r.call.verb);
    }

    // `k2 app warnings`: the ?token= use (deprecated) and the refused write.
    let q = gateway::query_pass_use("formapp").expect("noted");
    assert_eq!(q.count, 3, "three ?token= calls: {q:?}");
    let r = get(d.port, "/cli/app/warnings", OWNER, "app=formapp").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let w = r.json();
    let by_code = |c: &str| -> Vec<Value> { w["warnings"].as_array().unwrap().iter().filter(|x| x["code"] == c).cloned().collect() };
    let qp = by_code("pass_in_query");
    assert_eq!(qp.len(), 1, "{w}");
    assert_eq!(qp[0]["severity"], "check", "{w}");
    assert!(qp[0]["fix"].as_str().unwrap().contains("Authorization: Bearer"), "{w}");
    let refused = by_code("body_refused");
    let verbs: Vec<&str> = refused.iter().map(|x| x["message"].as_str().unwrap()).collect();
    assert!(
        refused.iter().any(|x| x["severity"] == "breaks" && x["message"].as_str().unwrap().contains("POST /cli/feedback/create")),
        "{verbs:?}"
    );
    assert!(
        refused.iter().any(|x| x["severity"] == "check" && x["message"].as_str().unwrap().contains("POST /cli/thread/post")),
        "the text/plain refusal on a form route is a check: {verbs:?}"
    );

    gateway::close("formapp", None, true);
    k2_daemon::apps::alerts::set_sink(None);
}
