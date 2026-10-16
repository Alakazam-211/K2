//! App runtime 0.46.1 door, headless (`prd-k2-app-runtime-v1` §5.3–5.5,
//! S19): the real dispatcher in-process on a temp `$HOME`, an App's link
//! opened the way the guarded engine opens one, then driven over loopback.
//!
//! - the link pass carries the manifest narrowed to the approved set;
//! - quotas: the rate bucket (`429 rate_limited` + `retryAfterMs`), body
//!   size (`413 too_large` + `limit`), with the owner's per-App override;
//! - every request, admitted or refused, is a row in `app-audit.db`,
//!   hash-chained, readable with `GET /cli/app/audit` (own workspace only);
//! - alerts: `pass_wrong_link` and quota hits reach the App's workspace;
//! - suspend: every pass revoked in one step; a connection still open gets
//!   `401` on its very next request; deploys frozen; only the owner resumes.

#![cfg(unix)]

#[path = "support/app_fixture.rs"]
mod app_fixture;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use app_fixture::*;
use k2_daemon::apps::gateway::{self, LinkSpec};
use serde_json::{json, Value};

fn parse(text: &str) -> (u16, String) {
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|x| x.parse().ok())
        .unwrap_or_else(|| panic!("no status: {text:?}"));
    (status, text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default())
}

/// One request on an open connection; reads exactly one response.
fn send(s: &mut TcpStream, req: &str) -> (u16, String) {
    s.write_all(req.as_bytes()).unwrap();
    let mut got = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = s.read(&mut buf).unwrap_or(0);
        if n == 0 {
            break;
        }
        got.extend_from_slice(&buf[..n]);
        let t = String::from_utf8_lossy(&got).to_string();
        if let Some((head, body)) = t.split_once("\r\n\r\n") {
            let len: usize = head
                .lines()
                .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap()))
                .unwrap_or(0);
            if body.len() >= len {
                return parse(&t);
            }
        }
    }
    parse(&String::from_utf8_lossy(&got))
}

fn conn(port: u16) -> TcpStream {
    let s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    s
}

fn me(pass: &str) -> String {
    format!("GET /gw/me HTTP/1.1\r\nHost: 127.0.0.1:8730\r\nAuthorization: Bearer {pass}\r\n\r\n")
}

fn err(body: &str) -> Value {
    let v: Value = serde_json::from_str(body).unwrap_or_else(|e| panic!("JSON ({e}): {body}"));
    v["error"].clone()
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f).await.unwrap()
}

/// A guarded App row with an approved set and a quota override.
fn install_row(app: &str, svc: &str, ws: &Ws, approved: &[&str], quota: Value) {
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
        "UPDATE apps SET approved_caps_json = ?2, quota_json = ?3 WHERE app_id = ?1",
        rusqlite::params![app, serde_json::to_string(approved).unwrap(), quota.to_string()],
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn door_quotas_audit_alerts_and_suspend_on_a_live_link() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "doorws");
    let other_ws = seed_ws(&env, "doorother");
    let msgs: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let sink = msgs.clone();
    k2_daemon::apps::alerts::set_sink(Some(Box::new(move |w: &str, t: &str| sink.lock().unwrap().push((w.into(), t.into())))));

    install_row("doorapp", "svc-door-1", &ws, &["thread:read"], json!({"rps": 2, "burst": 3, "bodyBytes": 64}));
    let spec = LinkSpec {
        app_id: "doorapp".into(),
        service_id: "svc-door-1".into(),
        workspace_id: ws.id.clone(),
        port: 0,
        // The engine's manifest ceiling; the approved set narrows it.
        caps: vec!["thread:read".into(), "files:write".into()],
        rooms: vec![ws.id.clone()],
    };
    let pass = gateway::open(spec, Some(Arc::new(|_: &str| Ok(())))).expect("open");
    let port = gateway::port_of("doorapp").unwrap();
    let other_pass = gateway::open(
        LinkSpec {
            app_id: "dooroth".into(),
            service_id: "svc-door-2".into(),
            workspace_id: other_ws.id.clone(),
            port: 0,
            caps: vec!["thread:read".into()],
            rooms: vec![other_ws.id.clone()],
        },
        None,
    )
    .unwrap();

    let p = pass.clone();
    let op = other_pass.clone();
    let (first, burst_over, after_wait, too_big, wrong) = blocking(move || {
        let mut s = conn(port);
        // §5.3: manifest (thread:read, files:write) ∩ approved (thread:read).
        let first = send(&mut s, &me(&p));
        // The bucket holds 3: two more pass, the fourth in the same instant is 429.
        let _ = send(&mut s, &me(&p));
        let _ = send(&mut s, &me(&p));
        let burst_over = send(&mut s, &me(&p));
        std::thread::sleep(Duration::from_millis(1100));
        let after_wait = send(&mut s, &me(&p));
        // A body over the override's 64 bytes.
        let body = format!("{{\"text\":\"{}\"}}", "x".repeat(80));
        let too_big = send(
            &mut conn(port),
            &format!(
                "POST /cli/thread/post HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {p}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            ),
        );
        // Another App's pass on this link.
        let wrong = send(&mut conn(port), &format!("GET /gw/me HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {op}\r\nConnection: close\r\n\r\n"));
        (first, burst_over, after_wait, too_big, wrong)
    })
    .await;
    assert_eq!(first.0, 200, "{}", first.1);
    let v: Value = serde_json::from_str(&first.1).unwrap();
    assert_eq!(v["caps"], json!(["thread:read"]), "files:write is withheld until approved");
    assert_eq!(burst_over.0, 429, "{}", burst_over.1);
    let e = err(&burst_over.1);
    assert_eq!((e["code"].as_str(), e["app"].as_str(), e["limit"].as_u64()), (Some("rate_limited"), Some("doorapp"), Some(2)), "{e}");
    assert!(e["retryAfterMs"].as_u64().unwrap() >= 1, "{e}");
    assert_eq!(after_wait.0, 200, "the bucket refills: {}", after_wait.1);
    assert_eq!(too_big.0, 413, "{}", too_big.1);
    let e = err(&too_big.1);
    assert_eq!((e["code"].as_str(), e["limit"].as_u64()), (Some("too_large"), Some(64)), "{e}");
    assert_eq!((wrong.0, err(&wrong.1)["code"].as_str().map(str::to_string)), (403, Some("pass_wrong_link".to_string())));

    // The per-call audit: every request, admitted or refused, chained. A
    // connection's last row is closed when the daemon sees it end, so wait
    // for the refusals to land (bounded; then the assertions below fail).
    let mut a = Value::Null;
    for _ in 0..50 {
        let r = get(d.port, "/cli/app/audit", OWNER, "app=doorapp&verify=1&limit=50").await;
        assert_eq!(r.status, 200, "{}", r.body);
        a = r.json();
        let rows = a["calls"].as_array().unwrap();
        let n = rows.iter().filter(|c| c["code"].is_string()).count();
        let ok = rows.iter().filter(|c| c["result"] == 200).count();
        if n >= 3 && ok >= 4 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let calls = a["calls"].as_array().unwrap();
    let codes: Vec<&str> = calls.iter().filter_map(|c| c["code"].as_str()).collect();
    for want in ["rate_limited", "too_large", "pass_wrong_link"] {
        assert!(codes.contains(&want), "{want} audited: {codes:?}");
    }
    let ok = calls.iter().filter(|c| c["result"] == 200 && c["verb"] == "GET /gw/me").count();
    assert!(ok >= 4, "admitted calls audited with their result: {}", a);
    for c in calls {
        assert_eq!(c["appId"], "doorapp");
        assert_eq!(c["actor"], "app");
        assert!(!c["verb"].as_str().unwrap().contains("k2skn_"), "never a pass in the audit: {c}");
    }
    assert!(calls.iter().any(|c| c["bytesOut"].as_u64().unwrap() > 0), "bytes out recorded");
    assert!(a["chain"].as_array().unwrap().iter().all(|d| d["ok"] == true), "{}", a["chain"]);
    assert!(a["totals"]["denied"].as_i64().unwrap() >= 3);
    // Another workspace's agent can't read it; this workspace's can.
    let r = get(d.port, "/cli/app/audit", &passport(&other_ws), "app=doorapp").await;
    assert_eq!((r.status, r.code().as_str()), (404, "app_not_found"), "{}", r.body);
    let r = get(d.port, "/cli/app/audit", &passport(&ws), "app=doorapp&denied=1").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.json()["calls"].as_array().unwrap().iter().all(|c| c["result"].as_u64().unwrap() >= 400));

    // Alerts reached the App's workspace (once per kind).
    let said: Vec<String> = msgs.lock().unwrap().iter().filter(|(w, _)| w == &ws.id).map(|(_, t)| t.clone()).collect();
    assert!(said.iter().any(|t| t.contains("pass for another link")), "{said:?}");
    assert!(said.iter().any(|t| t.contains("rate_limited")), "{said:?}");
    assert!(said.iter().any(|t| t.contains("too_large")), "{said:?}");
    assert_eq!(said.iter().filter(|t| t.contains("rate_limited")).count(), 1, "once per 5 minutes: {said:?}");

    // Suspend with a connection still open: its very next request is 401.
    let p = pass.clone();
    let mut held = blocking(move || {
        let mut s = conn(port);
        std::thread::sleep(Duration::from_millis(1100));
        let r = send(&mut s, &me(&p));
        assert_eq!(r.0, 200, "{}", r.1);
        s
    })
    .await;
    let r = post(d.port, "/cli/app/suspend", &passport(&ws), json!({"app": "doorapp", "reason": "door test"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["passesRevoked"], 1);
    let p = pass.clone();
    let (next, s2) = blocking(move || {
        let r = send(&mut held, &me(&p));
        (r, held)
    })
    .await;
    drop(s2);
    assert_eq!(next.0, 401, "{}", next.1);
    assert_eq!(err(&next.1)["code"], "app_suspended");
    assert!(gateway::port_of("doorapp").is_none(), "stop closed the link");
    let r = post(d.port, "/cli/app/deploy", OWNER, json!({"app": "doorapp", "seed": "nope"})).await;
    assert_eq!((r.status, r.code().as_str()), (409, "app_suspended"), "{}", r.body);
    let r = post(d.port, "/cli/app/resume", &passport(&ws), json!({"app": "doorapp"})).await;
    assert_eq!((r.status, r.code().as_str()), (403, "owner_only"), "{}", r.body);
    let r = get(d.port, "/cli/app/resume", OWNER, "app=doorapp").await;
    assert_eq!(r.status, 405, "POST only");
    let r = post(d.port, "/cli/app/resume", OWNER, json!({"app": "doorapp"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["suspension"]["suspended"], false);

    gateway::close("dooroth", None, true);
    k2_daemon::apps::alerts::set_sink(None);
}
