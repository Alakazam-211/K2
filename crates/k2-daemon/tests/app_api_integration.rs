//! The App API, headless (runtime S23; room-scope R1, R5, R6): the real
//! dispatcher in-process on a temp `$HOME`, driven over loopback.
//!
//! - Both doors serve exactly the catalog's `app` rows: every link pair
//!   passes the link's gate (never `not_on_link`), the helper forwards its
//!   own list, and a route with no app row is refused at both (R1, §5.3).
//! - `GET /cli/skin/me` (R6): a guest's login, an App's link pass and a
//!   platform pass each read their own kind, rooms with caps, and this
//!   server's features; the owner token and a dead pass are refused.
//! - `GET /cli/thread/latest` for passes (R5): its own rooms preview, a
//!   room it doesn't hold is the uniform `skin_room` item, a room without
//!   `thread:read` is `cap_not_granted`, on the main port and on a link.
//! - `POST /cli/skin/login` through an App's link (must-not-break: a BFF
//!   logs its guests in from inside the wall).

#![cfg(unix)]

#[path = "support/app_fixture.rs"]
mod app_fixture;

use app_fixture::*;
use k2_core::app_surface;
use k2_core::contract::{catalog, Door, Exposure, HttpMethod};
use k2_core::db::schema::WorkspaceSession;
use k2_daemon::apps::gateway::{self, LinkSpec};
use serde_json::{json, Value};

fn route_of(m: HttpMethod) -> &'static str {
    match m {
        HttpMethod::Get => "GET",
        HttpMethod::Post => "POST",
    }
}

#[test]
fn both_doors_serve_exactly_the_catalogs_app_rows() {
    let c = catalog();
    let rows = c.verbs_exposed_to(Exposure::App);
    assert_eq!(rows.len(), 122, "app rows (117 routes + 5 sockets; catalog v26 (0.46.5) email.send, email.status, email.log; the 0.46.5 activity SDK thread.activityRooms; catalog v25 (0.46.5) profile.get, profile.update, profile.setImage, profile.clearImage, profile.sessions, profile.revokeSession, profile.signOutOthers; catalog v24 (0.46.5) pairing.start, pairing.status, pairing.cancel, pairing.check, pairing.redeem; catalog v23 (0.46.4) push.register, push.unregister, push.device, push.prefs, push.send, push.status, push.devices; catalog v22 (0.46.4) app.rooms, guests.list, files.downloads, files.download; the counts-only ear 0.46.3 thread.activity; RS-A15 added guests.rooms / guests.setRooms, room-scope 0.46.1 room.members, app.self and app.usage, R3 files.stat, Thread parity 0.46.3 thread.attachmentGet, thread.catchup, thread.setRead, thread.unread, files.resolveLinks, guest read state thread.mute, App project chat 0.46.3 project.list/read/post/markRead and the project.events socket, app-mobile-access-v1 guests.logout/refresh/devices/revokeDevice, app.info, password.forgot/reset, app-room-stream 0.46.3 the thread.stream socket, App users' wallets 0.46.3 the nine wallet.* helper rows)");
    for v in &rows {
        if let Some(h) = &v.bindings.http {
            let m = route_of(h.method);
            let link = gateway::route_allowed(m, &h.route, "", false);
            let helper = k2_daemon::skin_gateway::allowlisted_http(m, &h.route);
            match h.door {
                None => assert!(link && helper, "{}: {m} {} on both doors", v.verb, h.route),
                Some(Door::Helper) => assert!(!link && helper, "{}: helper only", v.verb),
                Some(Door::Link) => assert!(link && !helper, "{}: link only", v.verb),
            }
            if h.method == HttpMethod::Get {
                let head = h.head != Some(false);
                assert_eq!(gateway::route_allowed("HEAD", &h.route, "", false), head && h.door != Some(Door::Helper), "{}: HEAD", v.verb);
                assert_eq!(k2_daemon::skin_gateway::allowlisted_http("HEAD", &h.route), head && h.door != Some(Door::Link), "{}: HEAD", v.verb);
            }
        }
        if let Some(s) = &v.bindings.socket {
            let param = s.param.as_deref().expect("app sockets name their param");
            assert!(gateway::route_allowed("GET", &s.path, &format!("{param}=x"), true), "{}", v.verb);
            assert!(!gateway::route_allowed("GET", &s.path, "", true), "{}: the param is required", v.verb);
            assert!(k2_daemon::skin_gateway::allowlisted_ws(&s.path), "{}", v.verb);
        }
    }
    // Default deny: no app row, no door.
    for (m, p) in [
        ("POST", "/cli/thread/ask"),
        ("POST", "/cli/thread/secret"),
        ("GET", "/cli/chatter"),
        ("POST", "/cli/skin/users/rooms"),
        ("GET", "/cli/presence/summary"),
        ("POST", "/cli/fs/delete"),
        ("GET", "/cli/fs/search-tree"),
        ("POST", "/cli/store/put"),
        ("GET", "/cli/app/list"),
        ("GET", "/v1/apps/x/users"),
    ] {
        assert!(!gateway::route_allowed(m, p, "", false), "{m} {p} must not be on the link");
        assert!(!k2_daemon::skin_gateway::allowlisted_http(m, p), "{m} {p} must not be on the helper");
        assert!(app_surface::verb_for(m, p).is_none(), "{m} {p}");
    }
    assert!(!gateway::route_allowed("GET", "/cli/sessions/events", "x=1", true));
}

/// A registered workspace with a pinned Chat (so its handle is a Thread
/// address).
fn pinned_ws(env: &Env, handle: &str) -> Ws {
    let ws = seed_ws(env, handle);
    let conv = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &ws.id, None, Some(&conv), "claude", "system", "running").expect("pin");
    ws
}

async fn owner_post(port: u16, path: &str, body: Value) {
    let r = post(port, path, OWNER, body).await;
    assert_eq!(r.status, 200, "{path}: {}", r.body);
}

/// A guest login whose role holds `caps` in each room; returns its pass.
async fn guest(port: u16, username: &str, rooms: &[(&Ws, &[&str])]) -> String {
    owner_post(port, "/cli/skin/users", json!({ "username": username })).await;
    owner_post(port, "/cli/skin/users/password", json!({ "username": username, "password": "s3cret-horse" })).await;
    let role = format!("r-{username}");
    owner_post(port, "/cli/skin/roles", json!({ "name": role })).await;
    for (ws, caps) in rooms {
        owner_post(port, "/cli/skin/roles/room", json!({ "name": role, "handle": ws.handle, "caps": caps })).await;
    }
    owner_post(port, "/cli/skin/roles/assign", json!({ "username": username, "role": role })).await;
    let body = json!({ "username": username, "password": "s3cret-horse" }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body))).await.unwrap();
    assert_eq!(r.status, 200, "login {username}: {}", r.body);
    let tok = r.json()["token"].as_str().expect("token").to_string();
    assert!(tok.starts_with("k2skn_"), "{tok}");
    tok
}

/// One request on an App's link: the App pass as Bearer, JSON on POST.
fn on_link(port: u16, method: &str, pq: &str, pass: &str, body: Option<&str>) -> Resp {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect link");
    s.set_read_timeout(Some(std::time::Duration::from_secs(60))).unwrap();
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn r1_r5_r6_me_thread_latest_and_login_on_both_doors() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let sales = pinned_ws(&env, &format!("sales{tag}"));
    let ops = pinned_ws(&env, &format!("ops{tag}"));
    let quiet = pinned_ws(&env, &format!("quiet{tag}"));
    owner_post(port, "/cli/thread/post", json!({ "addr": sales.handle, "text": "Quote is ready" })).await;

    // ── me (R6) ──────────────────────────────────────────────────────
    let dana = guest(port, &format!("dana{tag}"), &[(&sales, &["thread:read", "files:read"]), (&quiet, &["files:read"])]).await;
    let me = get(port, "/cli/skin/me", &dana, "").await;
    assert_eq!(me.status, 200, "{}", me.body);
    let v = me.json();
    assert_eq!(v["ok"], true);
    assert_eq!(v["kind"], "guest");
    assert_eq!(v["username"], format!("dana{tag}"));
    assert_eq!(v["app"], "host");
    assert!(v["principalId"].as_str().is_some_and(|p| !p.is_empty()), "{v}");
    let rooms = v["rooms"].as_array().expect("rooms");
    assert_eq!(rooms.len(), 2, "only the rooms the login holds: {v}");
    let sales_room = rooms.iter().find(|r| r["room"] == sales.id.as_str()).unwrap_or_else(|| panic!("sales in {v}"));
    assert_eq!(sales_room["handle"], sales.handle.as_str());
    let mut caps: Vec<String> = serde_json::from_value(sales_room["caps"].clone()).unwrap();
    caps.sort();
    assert_eq!(caps, ["files:read", "thread:read"]);
    assert!(!rooms.iter().any(|r| r["room"] == ops.id.as_str()), "never a room it doesn't hold");
    let features: Vec<String> = serde_json::from_value(v["features"].clone()).unwrap();
    assert!(features.iter().any(|f| f == "app-api-v1") && features.iter().any(|f| f == "thread-latest"), "{features:?}");
    assert_eq!(v["catalogVersion"], k2_core::contract::catalog().catalog_version);
    assert!(!me.body.contains(&dana), "the pass is never echoed");

    let r = get(port, "/cli/skin/me", OWNER, "").await;
    assert_eq!(r.status, 403, "owner token: {}", r.body);
    assert_eq!(r.code(), "skin_token");
    let r = get(port, "/cli/skin/me", "k2skn_00000000000000000000000000000000000000000", "").await;
    assert_eq!(r.status, 401, "dead pass: {}", r.body);
    let r = post(port, "/cli/skin/me", &dana, json!({})).await;
    assert_eq!(r.status, 405, "GET only: {}", r.body);

    let (_, platform) = k2_core::skin::create_token(&format!("bff{tag}"), Some(&["thread:read".to_string()]), &[ops.id.clone()]).unwrap();
    let v = get(port, "/cli/skin/me", &platform, "").await.json();
    assert_eq!(v["kind"], "token");
    assert_eq!(v["rooms"], json!([{ "room": ops.id, "handle": ops.handle, "caps": ["thread:read"] }]));

    // ── thread/latest for a pass (R5) ───────────────────────────────
    let r = get(port, "/cli/thread/latest", &dana, &format!("addrs={},{},{}", sales.handle, ops.handle, quiet.handle)).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let items = r.json()["items"].as_array().expect("items").clone();
    assert_eq!(items.len(), 3);
    assert_eq!(items[0]["preview"], "Quote is ready", "its own room: {}", r.body);
    assert!(items[0]["conversationId"].is_string());
    assert_eq!(items[1]["error"]["code"], "skin_room", "a room it doesn't hold: {}", r.body);
    assert!(items[1].get("preview").is_none());
    assert_eq!(items[2]["error"]["code"], "cap_not_granted", "a room without thread:read: {}", r.body);
    assert!(items[2].get("preview").is_none());

    // ── the link: an App's own pass ─────────────────────────────────
    // The link's App is a published service, so its own guests can log in
    // through the link (lane 2 binds a link login to the link's App, AR50).
    let svc = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let dir = sales.path.join(format!("jobs{tag}"));
        std::fs::create_dir_all(&dir).unwrap();
        k2_core::published_services::insert(
            &conn,
            &sales.id,
            &format!("jobs{tag}"),
            "node server.mjs",
            &dir.to_string_lossy(),
            47_012,
            "local",
            "stopped",
            "cmd",
            "",
        )
        .expect("insert service")
        .id
    };
    let spec = LinkSpec {
        app_id: format!("jobs{tag}"),
        service_id: svc.clone(),
        workspace_id: sales.id.clone(),
        port: 0,
        caps: vec!["thread:read".into()],
        rooms: vec![sales.id.clone()],
    };
    let pass = gateway::open(spec, None).expect("open link");
    let lport = gateway::port_of(&format!("jobs{tag}")).expect("listening");

    let r = link(lport, "GET", "/cli/skin/me".into(), pass.clone(), None).await;
    assert_eq!(r.status, 200, "me on the link: {}", r.body);
    let v = r.json();
    assert_eq!(v["kind"], "app");
    assert_eq!(v["rooms"], json!([{ "room": sales.id, "handle": sales.handle, "caps": ["thread:read"] }]));

    let r = link(lport, "GET", format!("/cli/thread/latest?addrs={},{}", sales.handle, ops.handle), pass.clone(), None).await;
    assert_eq!(r.status, 200, "thread/latest on the link: {}", r.body);
    let items = r.json()["items"].as_array().expect("items").clone();
    assert_eq!(items[0]["preview"], "Quote is ready");
    assert_eq!(items[1]["error"]["code"], "skin_room");

    // A BFF logs one of its App's guests in from inside the wall
    // (must-not-break). The body names the App; on a link it is the link's
    // App either way (a link logs guests in to its own App only).
    let lina = format!("lina{tag}");
    k2_core::skin::add_principal_for_app(&lina, None, &svc, None, "owner").expect("App guest");
    k2_core::skin::set_principal_password(&lina, Some("s3cret-horse")).expect("App guest password");
    let r = link(
        lport,
        "POST",
        "/cli/skin/login".into(),
        pass.clone(),
        Some(json!({ "username": lina, "password": "s3cret-horse", "app": svc }).to_string()),
    )
    .await;
    assert_eq!(r.status, 200, "login through the link: {}", r.body);
    let guest_pass = r.json()["token"].as_str().expect("token").to_string();
    assert!(guest_pass.starts_with("k2skn_") && guest_pass != dana);
    let r = link(
        lport,
        "POST",
        "/cli/skin/login".into(),
        pass.clone(),
        Some(json!({ "username": lina, "password": "wrong", "app": svc }).to_string()),
    )
    .await;
    assert_eq!(r.status, 401, "a wrong password through the link: {}", r.body);

    // Every pair the link serves passes its gate (rooms and caps decide
    // after it; an empty body is a usage refusal, never not_on_link).
    for (m, p) in app_surface::LINK_HTTP {
        if *m == "POST" && *p == "/cli/skin/login" {
            continue; // above, with real credentials (and the login throttle)
        }
        let body = (*m == "POST").then(|| "{}".to_string());
        let r = link(lport, m, p.to_string(), pass.clone(), body).await;
        if r.status == 404 && !r.body.is_empty() {
            let v: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
            assert_ne!(v["error"]["code"], "not_on_link", "{m} {p} is an app row but the link refused it: {}", r.body);
        }
    }
    // And a route with no app row is refused before any handler.
    let r = link(lport, "GET", "/cli/chatter".into(), pass.clone(), None).await;
    assert_eq!(r.status, 404);
    assert_eq!(r.code(), "not_on_link");

    gateway::close(&format!("jobs{tag}"), None, true);
    drop(env);
}
