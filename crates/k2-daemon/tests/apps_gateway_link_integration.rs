//! App runtime S14 — the gateway link, headless (`prd-k2-app-runtime-v1`
//! §5.1–5.2, §11.4 E11): the real dispatcher in-process on a temp `$HOME`.
//!
//! A link is opened the way the guarded engine opens one (no container
//! here: the wall itself is the Linux e2e), then driven over loopback:
//! - its own pass works on its own link, on App-facing routes only;
//! - no pass, the owner token, a platform token, a guest pass or another
//!   App's pass are refused with stable codes (E11: a pass works only on
//!   its own listener), on every request of a keep-alive connection;
//! - the link pass is refused on the daemon's main port;
//! - Host, cookies and non-JSON writes are refused before any route;
//! - rotation keeps the old pass valid; stop revokes everything and closes
//!   the listener;
//! - `GET /cli/app/link` / `POST /cli/app/link/rotate` (`k2 app link`) with
//!   passport scoping and the manage-Apps switch.

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

/// One raw request on a fresh connection; (status, body).
fn raw(port: u16, request: &str) -> (u16, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    s.write_all(request.as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    parse(&String::from_utf8_lossy(&out))
}

fn parse(text: &str) -> (u16, String) {
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|x| x.parse().ok())
        .unwrap_or_else(|| panic!("no status: {text:?}"));
    (status, text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default())
}

fn get_on(port: u16, path: &str, auth: Option<&str>) -> (u16, String) {
    let a = auth.map(|t| format!("Authorization: Bearer {t}\r\n")).unwrap_or_default();
    raw(port, &format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:8730\r\n{a}Connection: close\r\n\r\n"))
}

fn code(body: &str) -> String {
    let v: Value = serde_json::from_str(body).unwrap_or_else(|e| panic!("JSON ({e}): {body}"));
    v["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {body}")).to_string()
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f).await.unwrap()
}

fn spec(app: &str, svc: &str, ws: &Ws, caps: &[&str]) -> LinkSpec {
    LinkSpec {
        app_id: app.into(),
        service_id: svc.into(),
        workspace_id: ws.id.clone(),
        port: 0,
        caps: caps.iter().map(|c| c.to_string()).collect(),
        rooms: vec![ws.id.clone()],
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s14_the_link_admits_only_its_own_pass_on_app_routes() {
    let env = setup(&[]);
    // The door's alerts (pass_wrong_link below) never wake a real agent.
    k2_daemon::apps::alerts::set_sink(Some(Box::new(|_: &str, _: &str| {})));
    let d = k2_daemon::test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "linkws");
    let other_ws = seed_ws(&env, "otherws");

    let delivered: Arc<Mutex<Vec<String>>> = Arc::default();
    let sink = delivered.clone();
    let deliver: gateway::Deliver = Arc::new(move |p: &str| {
        sink.lock().unwrap().push(p.to_string());
        Ok(())
    });
    let pass = gateway::open(spec("jobs", "svc-jobs-1111", &ws, &["thread:read"]), Some(deliver)).expect("open link");
    let port = gateway::port_of("jobs").expect("listening");
    let other_pass = gateway::open(spec("crm", "svc-crm-2222", &other_ws, &["thread:read"]), None).expect("second link");
    let other_port = gateway::port_of("crm").unwrap();
    assert_ne!(port, other_port);

    // Its own pass: /gw/me names the link, the caps and the one room.
    let (st, body) = blocking({
        let p = pass.clone();
        move || get_on(port, "/gw/me", Some(&p))
    })
    .await;
    assert_eq!(st, 200, "{body}");
    let me: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(me["app"], "jobs");
    assert_eq!(me["appServiceId"], "svc-jobs-1111");
    assert_eq!(me["link"], format!("gw:{port}"));
    assert_eq!(me["caps"], json!(["thread:read"]));
    assert_eq!(me["rooms"], json!([ws.id]));
    assert!(!body.contains(&pass), "the pass is never echoed");

    // An App-facing route runs its real handler with the pass (rooms decide).
    let (st, body) = blocking({
        let p = pass.clone();
        move || get_on(port, "/cli/skin/agents", Some(&p))
    })
    .await;
    assert_eq!(st, 200, "{body}");
    assert!(serde_json::from_str::<Value>(&body).unwrap()["agents"].is_array(), "{body}");
    // ?token= works too (sockets carry it that way).
    let (st, _) = blocking({
        let p = pass.clone();
        move || get_on(port, &format!("/cli/skin/agents?token={p}"), None)
    })
    .await;
    assert_eq!(st, 200);

    // Refusals, each with its code.
    let (_, platform) = k2_core::skin::create_token("dannon-bff", Some(&["thread:read".to_string()]), &[ws.id.clone()]).unwrap();
    let cases: Vec<(String, Option<String>, u16, &str)> = vec![
        ("/gw/me".into(), None, 401, "pass_required"),
        ("/gw/me".into(), Some(OWNER.into()), 401, "wrong_credential"),
        ("/gw/me".into(), Some("k2skn_00000000000000000000000000000000000000000".into()), 401, "pass_invalid"),
        ("/gw/me".into(), Some(other_pass.clone()), 403, "pass_wrong_link"),
        ("/gw/me".into(), Some(platform.clone()), 403, "pass_wrong_link"),
        ("/cli/app/list".into(), Some(pass.clone()), 404, "not_on_link"),
        ("/cli/skin/users".into(), Some(pass.clone()), 404, "not_on_link"),
        ("/cli/sessions/list".into(), Some(pass.clone()), 404, "not_on_link"),
        ("/health".into(), Some(pass.clone()), 404, "not_on_link"),
    ];
    for (path, auth, want_status, want_code) in cases {
        let (st, body) = blocking(move || get_on(port, &path, auth.as_deref())).await;
        assert_eq!(st, want_status, "{want_code}: {body}");
        assert_eq!(code(&body), want_code, "{body}");
    }
    // The other App's link refuses this App's pass the same way.
    let (st, body) = blocking({
        let p = pass.clone();
        move || get_on(other_port, "/gw/me", Some(&p))
    })
    .await;
    assert_eq!((st, code(&body).as_str()), (403, "pass_wrong_link"), "{body}");

    // Host, cookies, non-JSON writes: refused before any route.
    let p = pass.clone();
    let (st, body) = blocking(move || {
        raw(port, &format!("GET /gw/me HTTP/1.1\r\nHost: evil.example\r\nAuthorization: Bearer {p}\r\nConnection: close\r\n\r\n"))
    })
    .await;
    assert_eq!((st, code(&body).as_str()), (421, "host_not_link"), "{body}");
    let p = pass.clone();
    let (st, body) = blocking(move || {
        raw(port, &format!("GET /gw/me HTTP/1.1\r\nHost: 127.0.0.1\r\nCookie: k2_session=x\r\nAuthorization: Bearer {p}\r\nConnection: close\r\n\r\n"))
    })
    .await;
    assert_eq!((st, code(&body).as_str()), (400, "cookies_not_allowed"), "{body}");
    let p = pass.clone();
    let (st, body) = blocking(move || {
        raw(
            port,
            &format!(
                "POST /cli/thread/post HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {p}\r\nContent-Type: text/plain\r\nContent-Length: 2\r\nConnection: close\r\n\r\nhi"
            ),
        )
    })
    .await;
    assert_eq!((st, code(&body).as_str()), (415, "json_required"), "{body}");

    // Keep-alive: the gate runs on every request of a connection, not just
    // the first.
    let p = pass.clone();
    let (a, b) = blocking(move || {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let read_one = |s: &mut TcpStream| {
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
        };
        s.write_all(format!("GET /cli/skin/agents HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {p}\r\n\r\n").as_bytes()).unwrap();
        let a = read_one(&mut s);
        s.write_all(format!("GET /cli/skin/agents HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {OWNER}\r\n\r\n").as_bytes()).unwrap();
        let b = read_one(&mut s);
        (a, b)
    })
    .await;
    assert_eq!(a.0, 200, "{}", a.1);
    assert_eq!((b.0, code(&b.1).as_str()), (401, "wrong_credential"), "the second request on the same socket is gated too");

    // The link pass never works on the daemon's main port (or the tunnel).
    for p in [d.port, d.tunnel_port] {
        let pp = pass.clone();
        let (st, body) = blocking(move || get_on(p, &format!("/cli/skin/agents?token={pp}"), None)).await;
        assert_eq!(st, 401, "port {p}: {body}");
    }
    // …while a platform token still does (control).
    let pl = platform.clone();
    let (st, _) = blocking(move || get_on(d.port, &format!("/cli/skin/agents?token={pl}"), None)).await;
    assert_eq!(st, 200);

    // Rotation: a new pass reaches the App; the old one lives until expiry.
    let exp = gateway::rotate("jobs").expect("rotate");
    let newest = delivered.lock().unwrap().last().cloned().expect("delivered");
    assert_ne!(newest, pass);
    assert!(exp > 0);
    for p in [pass.clone(), newest.clone()] {
        let (st, body) = blocking(move || get_on(port, "/gw/me", Some(&p))).await;
        assert_eq!(st, 200, "{body}");
    }
    let s = gateway::status_json("jobs");
    assert_eq!(s["open"], true);
    assert_eq!(s["rotations"], 1);
    assert_eq!(s["livePasses"], 2);
    assert!(s["refused"].as_u64().unwrap() >= 10, "{s}");
    assert!(s["wrongLink"].as_u64().unwrap() >= 2, "{s}");
    assert!(!s.to_string().contains(&newest), "status never shows a pass");
    // A rotation that can't reach the App is revoked at once.
    assert!(gateway::rotate("crm").is_err(), "the second link has no way to deliver");

    // Stop: every pass revoked, the listener gone.
    assert_eq!(gateway::close("jobs", None, true), 2);
    assert!(gateway::port_of("jobs").is_none());
    let gone = blocking(move || TcpStream::connect_timeout(&format!("127.0.0.1:{port}").parse().unwrap(), Duration::from_millis(500)).is_err()).await;
    assert!(gone, "the listener closed");
    let (st, body) = blocking(move || get_on(other_port, "/gw/me", Some(&newest))).await;
    assert_eq!((st, code(&body).as_str()), (401, "pass_invalid"), "revoked, so unknown everywhere");
    gateway::close("crm", None, true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s14_k2_app_link_routes_scope_and_rotate() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let mine = seed_ws(&env, "linkmine");
    let theirs = seed_ws(&env, "linktheirs");
    let r = post(d.port, "/cli/app/install", OWNER, json!({"app": "linkapp", "workspace": mine.id, "engine": "process"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let svc = r.json()["app"]["serviceId"].as_str().map(str::to_string).unwrap_or_else(|| {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::deploy::store::get_app(&conn, "linkapp").unwrap().unwrap().service_id
    });

    // A process-engine App has no link.
    let r = get(d.port, "/cli/app/link", OWNER, "app=linkapp").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["link"]["open"], false);
    let r = post(d.port, "/cli/app/link/rotate", OWNER, json!({"app": "linkapp"})).await;
    assert_eq!((r.status, r.code().as_str()), (409, "link_unavailable"), "{}", r.body);
    let r = get(d.port, "/cli/app/link/rotate", OWNER, "app=linkapp").await;
    assert_eq!(r.status, 405, "POST only");

    // Open its link (as the guarded engine does) and read it back.
    let delivered: Arc<Mutex<Vec<String>>> = Arc::default();
    let sink = delivered.clone();
    let pass = gateway::open(
        LinkSpec { app_id: "linkapp".into(), service_id: svc.clone(), workspace_id: mine.id.clone(), port: 0, caps: vec!["thread:read".into()], rooms: vec![mine.id.clone()] },
        Some(Arc::new(move |p: &str| {
            sink.lock().unwrap().push(p.to_string());
            Ok(())
        })),
    )
    .unwrap();
    let r = get(d.port, "/cli/app/link", OWNER, "app=linkapp").await;
    let l = r.json()["link"].clone();
    assert_eq!(l["open"], true, "{}", r.body);
    assert_eq!(l["inWallUrl"], "http://127.0.0.1:8730");
    assert_eq!(l["passFile"], "/run/k2/pass");
    assert_eq!(l["caps"], json!(["thread:read"]));
    assert!(!r.body.contains(&pass), "never the pass");
    let r = get(d.port, "/cli/app/inspect", OWNER, "app=linkapp").await;
    assert_eq!(r.json()["app"]["link"]["open"], true);

    // Passports: their own workspace's App only; rotate needs the switch.
    let mine_pass = passport(&mine);
    let theirs_pass = passport(&theirs);
    let r = get(d.port, "/cli/app/link", &theirs_pass, "app=linkapp").await;
    assert_eq!((r.status, r.code().as_str()), (404, "app_not_found"), "{}", r.body);
    let r = get(d.port, "/cli/app/link", &mine_pass, "app=linkapp").await;
    assert_eq!(r.status, 200, "{}", r.body);
    allow_manage(&mine, false);
    let r = post(d.port, "/cli/app/link/rotate", &mine_pass, json!({"app": "linkapp"})).await;
    // S42: without the switch the rotate is held for the owner.
    assert_eq!((r.status, r.json()["status"].as_str(), r.json()["kind"].as_str()), (202, Some("held"), Some("manage")), "{}", r.body);
    allow_manage(&mine, true);
    let r = post(d.port, "/cli/app/link/rotate", &mine_pass, json!({"app": "linkapp"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["link"]["rotations"], 1);
    assert_eq!(delivered.lock().unwrap().len(), 1);
    // Another workspace's agent, even with its own switch on, never finds it.
    allow_manage(&theirs, true);
    let r = post(d.port, "/cli/app/link/rotate", &theirs_pass, json!({"app": "linkapp"})).await;
    assert_eq!((r.status, r.code().as_str()), (404, "app_not_found"), "{}", r.body);
    // An App pass can't manage Apps.
    let r = get(d.port, "/cli/app/link", &pass, "app=linkapp").await;
    assert_eq!(r.status, 403, "{}", r.body);

    // Stop revokes the passes and closes the link.
    let r = post(d.port, "/cli/app/stop", OWNER, json!({"app": "linkapp"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(gateway::port_of("linkapp").is_none());
    assert_eq!(k2_core::skin::link::live_link_passes(&svc).unwrap(), 0);
    let r = get(d.port, "/cli/app/link", OWNER, "app=linkapp").await;
    assert_eq!(r.json()["link"]["open"], false);
}
