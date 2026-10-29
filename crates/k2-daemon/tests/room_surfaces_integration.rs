//! Room-scope R9/R16 (0.46.1), headless: the real dispatcher in process on
//! a temp `$HOME`, driven over loopback, on the daemon port and an App's
//! own gateway link.
//!
//! - `GET /cli/skin/members` (room.members): a guest login with
//!   `room:members` reads its room from grants; without the cap it's
//!   `cap_not_granted`; a room it doesn't hold is `skin_room`; on the link
//!   the App pass reads its own App's guests; hidden names are null for
//!   every App caller while `k2 app members` (the owner) sees them; the
//!   change arrives as `room_members_changed` on the room socket.
//! - `GET /cli/skin/self|usage` (app.self, app.usage): only the App's own
//!   pass on its own link; another App's id answers exactly what a missing
//!   App answers (404, same bytes); a guest of the App gets `not_exposed`;
//!   usage counts the App's own audited calls with catalog verb names.
//! - Both doors: the browser helper forwards members, never self/usage.

#[path = "support/app_fixture.rs"]
mod app_fixture;

use std::time::Duration;

use app_fixture::*;
use futures_util::StreamExt;
use k2_daemon::apps::gateway::{self, LinkSpec};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

async fn owner_post(port: u16, path: &str, body: Value) -> Value {
    let r = post(port, path, OWNER, body).await;
    assert_eq!(r.status, 200, "{path}: {}", r.body);
    r.json()
}

/// A box-login guest whose role holds `caps` in `ws`; returns its pass.
async fn guest(port: u16, username: &str, ws: &Ws, caps: &[&str]) -> String {
    owner_post(port, "/cli/skin/users", json!({ "username": username })).await;
    k2_daemon::test_harness::guest_with_own_password(&username, "s3cret-horse");
    let role = format!("r-{username}");
    owner_post(port, "/cli/skin/roles", json!({ "name": role })).await;
    owner_post(port, "/cli/skin/roles/room", json!({ "name": role, "handle": ws.handle, "caps": caps })).await;
    owner_post(port, "/cli/skin/roles/assign", json!({ "username": username, "role": role })).await;
    let body = json!({ "username": username, "password": "s3cret-horse" }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body))).await.unwrap();
    assert_eq!(r.status, 200, "login {username}: {}", r.body);
    r.json()["token"].as_str().expect("token").to_string()
}

/// One request on an App's link with `pass` as Bearer.
fn on_link(port: u16, method: &str, pq: &str, pass: &str, body: Option<&str>) -> Resp {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect link");
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
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|x| x.parse().ok())
        .unwrap_or_else(|| panic!("no status for {method} {pq}: {text:?}"));
    Resp { status, body: text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default() }
}

async fn link(port: u16, method: &'static str, pq: String, pass: String, body: Option<String>) -> Resp {
    tokio::task::spawn_blocking(move || on_link(port, method, &pq, &pass, body.as_deref())).await.unwrap()
}

fn names(v: &Value) -> Vec<Value> {
    v["members"].as_array().unwrap_or_else(|| panic!("members in {v}")).iter().map(|m| m["username"].clone()).collect()
}

/// A published service and its App row in `ws` (the link's App).
fn app_in(ws: &Ws, app_id: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let dir = ws.path.join(app_id);
    std::fs::create_dir_all(&dir).unwrap();
    let svc = k2_core::published_services::insert(&conn, &ws.id, app_id, "node server.mjs", &dir.to_string_lossy(), 47_100, "local", "stopped", "cmd", "")
        .expect("insert service")
        .id;
    k2_core::deploy::store::insert_app(
        &conn,
        &k2_core::deploy::store::NewApp {
            app_id,
            service_id: &svc,
            seed_id: None,
            owner_workspace: &ws.id,
            isolation: "container",
            memory_limit_bytes: None,
            front_port: 0,
            created_by: "owner",
        },
    )
    .expect("insert app");
    svc
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn room_members_and_app_self_usage_on_both_doors() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let sales = seed_ws(&env, &format!("sales{tag}"));
    let ops = seed_ws(&env, &format!("ops{tag}"));

    // ── room.members on the daemon port (a guest's login) ──────────
    let ada = format!("ada{tag}");
    let bo = format!("bo{tag}");
    let staff = guest(port, &ada, &sales, &["thread:read", "room:members"]).await;
    let plain = guest(port, &bo, &sales, &["thread:read"]).await;
    let r = get(port, "/cli/skin/members", &staff, &format!("workspace={}", sales.handle)).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert_eq!(names(&v), [json!(ada), json!(bo)], "{v}");
    assert_eq!((v["admins"].as_str(), v["namesHidden"].as_bool()), (Some("admins of this box"), Some(false)));
    assert!(!r.body.contains(&staff), "the pass is never echoed");
    let r = get(port, "/cli/skin/members", &plain, &format!("workspace={}", sales.handle)).await;
    assert_eq!((r.status, r.code()), (403, "cap_not_granted".to_string()), "{}", r.body);
    let r = get(port, "/cli/skin/members", &staff, &format!("workspace={}", ops.handle)).await;
    assert_eq!((r.status, r.code()), (403, "skin_room".to_string()), "{}", r.body);
    let r = get(port, "/cli/skin/members", OWNER, &format!("workspace={}", sales.handle)).await;
    assert_eq!((r.status, r.code()), (403, "skin_token".to_string()), "the owner uses k2 app members: {}", r.body);
    let r = post(port, "/cli/skin/members", &staff, json!({})).await;
    assert_eq!(r.status, 405, "GET only: {}", r.body);
    // Not on a link: app.self is the App's own pass only.
    let r = get(port, "/cli/skin/self", &staff, "").await;
    assert_eq!((r.status, r.code()), (403, "not_exposed".to_string()), "{}", r.body);

    // The helper forwards members, never self/usage (link-only rows).
    assert!(k2_daemon::skin_gateway::allowlisted_http("GET", "/cli/skin/members"));
    assert!(!k2_daemon::skin_gateway::allowlisted_http("GET", "/cli/skin/self"));
    assert!(!k2_daemon::skin_gateway::allowlisted_http("GET", "/cli/skin/usage"));
    assert!(gateway::route_allowed("GET", "/cli/skin/self", "", false));

    // ── the room socket hears the owner's hide switch ───────────────
    let url = format!("ws://127.0.0.1:{port}/cli/activity/events?workspace={}&token={staff}", sales.handle);
    let (mut sock, _) = tokio_tungstenite::connect_async(&url).await.expect("room:members opens the room socket");
    let owner_hide = owner_post(port, "/cli/app/room-names", json!({ "workspace": sales.handle, "hide": true })).await;
    assert_eq!(owner_hide["namesHidden"], true);
    let frame = loop {
        let m = tokio::time::timeout(Duration::from_secs(10), sock.next())
            .await
            .expect("a frame in time")
            .expect("socket open")
            .expect("frame ok");
        if let Message::Text(t) = m {
            let f: Value = serde_json::from_str(&t).expect("JSON frame");
            if f["kind"] == "room_members_changed" {
                break f;
            }
        }
    };
    assert_eq!(frame, json!({ "kind": "room_members_changed", "workspace": sales.handle }));
    let v = get(port, "/cli/skin/members", &staff, &format!("workspace={}", sales.handle)).await.json();
    assert_eq!(names(&v), [Value::Null, Value::Null], "hidden from the guest: {v}");
    let r = get(port, "/cli/app/members", OWNER, &format!("workspace={}", sales.handle)).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(names(&r.json()), [json!(ada), json!(bo)], "the owner sees every name");
    let r = post(port, "/cli/app/room-names", &passport(&sales), json!({ "workspace": sales.handle, "hide": false })).await;
    assert_eq!(r.status, 403, "an agent never flips it: {}", r.body);
    owner_post(port, "/cli/app/room-names", json!({ "workspace": sales.handle, "hide": false })).await;
    // An agent reads its own room through its passport.
    let r = get(port, "/cli/app/members", &passport(&sales), "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["room"], sales.id.as_str());

    // ── the link: app.self / app.usage, the App's own guests ─────────
    let jobs = format!("jobs{tag}");
    let other = format!("other{tag}");
    let svc = app_in(&sales, &jobs);
    app_in(&ops, &other);
    let lina = format!("lina{tag}");
    k2_core::skin::add_principal_for_app(&lina, None, &svc, None, "owner").expect("an App guest");
    k2_core::skin::set_app_grant_rooms(&lina, &svc, &[sales.id.clone()], false, "owner").expect("lina's room");
    k2_core::skin::set_principal_password(&lina, Some("s3cret-horse")).unwrap();
    // room:members is a cap like any other: the link pass carries it only
    // once the owner approved it for this App (§5.3 grants formula).
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_daemon::apps::grants::record(&conn, &jobs, &["thread:read".to_string(), "room:members".to_string()]).expect("approved set");
    }
    let pass = gateway::open(
        LinkSpec {
            app_id: jobs.clone(),
            service_id: svc.clone(),
            workspace_id: sales.id.clone(),
            port: 0,
            caps: vec!["thread:read".into(), "room:members".into()],
            rooms: vec![sales.id.clone()],
        },
        None,
    )
    .expect("open link");
    let lport = gateway::port_of(&jobs).expect("listening");

    let r = link(lport, "GET", "/cli/skin/self".into(), pass.clone(), None).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert_eq!((v["app"].as_str(), v["workspace"].as_str(), v["health"].as_str()), (Some(jobs.as_str()), Some(sales.handle.as_str()), Some("stopped")));
    assert_eq!(v["liveSeed"], Value::Null);
    // Another App's data is exactly "no such App" (no oracle).
    let mine = link(lport, "GET", format!("/cli/skin/self?app={jobs}"), pass.clone(), None).await;
    assert_eq!(mine.status, 200, "naming itself: {}", mine.body);
    let theirs = link(lport, "GET", format!("/cli/skin/self?app={other}"), pass.clone(), None).await;
    let ghost = link(lport, "GET", "/cli/skin/self?app=ghost-app".into(), pass.clone(), None).await;
    assert_eq!((theirs.status, ghost.status), (404, 404));
    assert_eq!(theirs.body, ghost.body, "another App's answer is byte-identical to no such App");
    let theirs = link(lport, "GET", format!("/cli/skin/usage?app={other}"), pass.clone(), None).await;
    assert_eq!((theirs.status, theirs.body.clone()), (404, ghost.body.clone()));
    // room.members on the link: the App's own guests only.
    let r = link(lport, "GET", format!("/cli/skin/members?workspace={}", sales.handle), pass.clone(), None).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert_eq!(names(&v), [json!(lina)], "the box login's guests are another App's: {v}");
    assert_eq!(v["members"][0]["app"], jobs.as_str());
    let r = link(lport, "GET", format!("/cli/skin/members?workspace={}&app=host", sales.handle), pass.clone(), None).await;
    assert_eq!((r.status, r.body.clone()), (404, ghost.body.clone()), "naming the box login is another App");
    // A guest of this App, through the link: never the App's own data.
    let login = link(
        lport,
        "POST",
        "/cli/skin/login".into(),
        pass.clone(),
        Some(json!({ "username": lina, "password": "s3cret-horse", "app": svc }).to_string()),
    )
    .await;
    assert_eq!(login.status, 200, "{}", login.body);
    let lina_pass = login.json()["token"].as_str().unwrap().to_string();
    for p in ["/cli/skin/self", "/cli/skin/usage"] {
        let r = link(lport, "GET", p.into(), lina_pass.clone(), None).await;
        assert_eq!((r.status, r.code()), (403, "not_exposed".to_string()), "{p}: {}", r.body);
    }
    // app.usage: its own audited calls, catalog verb names, after the
    // writer catches up (the link records each call when it closes).
    let want_me_calls = 2;
    for _ in 0..want_me_calls {
        assert_eq!(link(lport, "GET", "/cli/skin/me".into(), pass.clone(), None).await.status, 200);
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let usage = loop {
        let r = link(lport, "GET", "/cli/skin/usage?hours=1".into(), pass.clone(), None).await;
        assert_eq!(r.status, 200, "{}", r.body);
        let v = r.json();
        let me = v["verbs"].as_array().unwrap().iter().find(|x| x["verb"] == "me.get").cloned();
        if me.as_ref().is_some_and(|m| m["calls"].as_i64() >= Some(want_me_calls)) {
            break v;
        }
        assert!(std::time::Instant::now() < deadline, "the audit never showed {want_me_calls} me.get calls: {v}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert_eq!(usage["hours"], 1);
    assert_eq!(usage["quotas"]["rps"], 50);
    let routes: Vec<&str> = usage["verbs"].as_array().unwrap().iter().filter_map(|x| x["route"].as_str()).collect();
    assert!(routes.contains(&"GET /cli/skin/self"), "{routes:?}");
    assert!(usage["totals"]["denied"].as_i64().unwrap() >= 3, "the refused calls count too: {usage}");
    let r = link(lport, "GET", "/cli/skin/usage?hours=0".into(), pass.clone(), None).await;
    assert_eq!((r.status, r.code()), (400, "usage".to_string()));
    // The owner and the App's own agent read the same through k2 app.
    let r = get(port, "/cli/app/usage", OWNER, &format!("app={jobs}&hours=1")).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = get(port, "/cli/app/self", &passport(&ops), &format!("app={jobs}")).await;
    assert_eq!(r.status, 404, "another workspace's agent: {}", r.body);
    let r = get(port, "/cli/app/self", &passport(&sales), &format!("app={jobs}")).await;
    assert_eq!(r.status, 200, "its own agent: {}", r.body);

    gateway::close(&jobs, None, true);
    drop(env);
}
