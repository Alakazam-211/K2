//! App mobile access (app-mobile-access-v1, 0.46.3), headless: the real
//! dispatcher in process on a temp `$HOME`, driven over loopback on the
//! daemon's main port (where native mobile Apps call).
//!
//! - A′ / R2: on every `/cli` route the daemon classifies that isn't a
//!   catalog `app` row, a live pass gets no 2xx and exactly today's bytes
//!   (the fence on vs `K2_APP_PASS_GATE=off`); the never list stays
//!   refused; catch-up and attachments (now rows) still work.
//! - C / R3: `me.get` lists the person's agents with identity, caps, `can`
//!   and folders; a room held by someone else answers byte-for-byte like a
//!   room that doesn't exist, on every room-taking verb.
//! - E: device sign-in (refresh rotation, reuse detection, own devices,
//!   revoke, logout); a login without a device is today's.
//! - F: discovery is per App, off by default, and a ghost: off (or another
//!   App, or no App) answers exactly like a route that doesn't exist; on,
//!   only that App's name, kind and sign-in; the owner or the App's own
//!   agent with switch 2 turns it on (audited, with an inbox note).
//! - C templates: suggestions are listed, never created on their own, and
//!   one becomes a role only when asked.

#![cfg(unix)]

#[path = "support/app_fixture.rs"]
mod app_fixture;

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use app_fixture::*;
use serde_json::{json, Value};

async fn owner_post(port: u16, path: &str, body: Value) -> Value {
    let r = post(port, path, OWNER, body).await;
    assert_eq!(r.status, 200, "{path}: {}", r.body);
    r.json()
}

async fn login(port: u16, body: Value) -> Resp {
    let b = body.to_string();
    tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(b))).await.unwrap()
}

/// A box-login guest whose role holds `caps` in `ws`; returns its pass.
async fn guest(port: u16, username: &str, ws: &Ws, caps: &[&str]) -> String {
    owner_post(port, "/cli/skin/users", json!({ "username": username })).await;
    owner_post(port, "/cli/skin/users/password", json!({ "username": username, "password": "s3cret-horse" })).await;
    let role = format!("r-{username}");
    owner_post(port, "/cli/skin/roles", json!({ "name": role })).await;
    owner_post(port, "/cli/skin/roles/room", json!({ "name": role, "handle": ws.handle, "caps": caps })).await;
    owner_post(port, "/cli/skin/roles/assign", json!({ "username": username, "role": role })).await;
    let r = login(port, json!({ "username": username, "password": "s3cret-horse" })).await;
    assert_eq!(r.status, 200, "login {username}: {}", r.body);
    r.json()["token"].as_str().expect("token").to_string()
}

/// The whole raw response (status line, headers, body) of one request.
fn raw(port: u16, method: &str, pq: &str, extra: &str) -> String {
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    let req = format!("{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n{extra}\r\n");
    s.write_all(req.as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    String::from_utf8_lossy(&out).to_string()
}

async fn raw_async(port: u16, method: &'static str, pq: String, extra: String) -> String {
    tokio::task::spawn_blocking(move || raw(port, method, &pq, &extra)).await.unwrap()
}

/// GET or POST with the pass as `Authorization: Bearer` (how a phone calls).
async fn bearer(port: u16, method: &'static str, path: &str, pass: &str) -> Resp {
    let (pq, pass) = (path.to_string(), pass.to_string());
    let text = tokio::task::spawn_blocking(move || {
        let extra = if method == "POST" {
            format!("Authorization: Bearer {pass}\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{{}}")
        } else {
            format!("Authorization: Bearer {pass}\r\n")
        };
        // `raw` adds the final CRLF; a POST already carries its body.
        if method == "POST" {
            let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
            s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
            let req = format!("POST {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n{extra}");
            s.write_all(req.as_bytes()).unwrap();
            let mut out = Vec::new();
            let _ = s.read_to_end(&mut out);
            String::from_utf8_lossy(&out).to_string()
        } else {
            raw(port, "GET", &pq, &extra)
        }
    })
    .await
    .unwrap();
    let status = text.lines().next().and_then(|l| l.split_whitespace().nth(1)).and_then(|x| x.parse().ok()).unwrap_or(0);
    Resp { status, body: text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default() }
}

fn code(r: &Resp) -> String {
    serde_json::from_str::<Value>(&r.body).ok().and_then(|v| v["error"]["code"].as_str().map(str::to_string)).unwrap_or_default()
}

// ── A′ / R2 ──────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_catalog_is_the_wall_for_passes_on_the_main_port() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let sales = seed_ws(&env, &format!("sales{tag}"));
    let pass = guest(port, &format!("ada{tag}"), &sales, &["thread:read", "thread:post", "files:read"]).await;
    let off = || k2_core::test_env::EnvVar::set(k2_daemon::routes::pass_gate::SWITCH_ENV, "off");

    // Every classified route that isn't a catalog row: a live pass gets
    // nothing (never a 2xx), and exactly today's bytes (the fence off).
    let mut walked = 0usize;
    for r in k2_daemon::routes::route_policy::ROUTES {
        let method: &'static str = if r.get.is_some() { "GET" } else { "POST" };
        if k2_daemon::routes::pass_gate::admits(method, r.path) {
            continue;
        }
        let on = bearer(port, method, r.path, &pass).await;
        let today = {
            let _off = off();
            bearer(port, method, r.path, &pass).await
        };
        assert_eq!((on.status, &on.body), (today.status, &today.body), "{method} {}: the fence changed today's answer", r.path);
        if on.status < 400 {
            // Only a Public route answers, and the pass bought nothing: the
            // same answer as no credential at all (e.g. auth/logout's no-op).
            let anon = tokio::task::spawn_blocking({
                let pq = r.path.to_string();
                move || req_blocking(port, method, &pq, (method == "POST").then(|| "{}".to_string()))
            })
            .await
            .unwrap();
            assert_eq!((on.status, &on.body), (anon.status, &anon.body), "{method} {}: a pass reached it", r.path);
        }
        walked += 1;
    }
    assert!(walked > 500, "walked only {walked} routes");

    // R2 never list (outside the /cli table too): refused, today's bytes.
    for (m, p) in [
        ("GET", "/events"),
        ("GET", "/v1/sandboxes"),
        ("GET", "/cli/sessions/events"),
        ("GET", "/cli/sessions/grid"),
        ("GET", "/cli/presence/summary"),
        ("GET", "/cli/llm/accounts/list"),
        ("GET", "/cli/me/homes"),
        ("POST", "/cli/daemon/update/start"),
        ("GET", "/cli/settings/get"),
    ] {
        let r = bearer(port, if m == "GET" { "GET" } else { "POST" }, p, &pass).await;
        assert!(matches!(r.status, 401 | 403 | 404 | 405), "{m} {p}: {} {}", r.status, r.body);
    }
    let r = bearer(port, "GET", "/cli/sessions/grid", &pass).await;
    assert_eq!(r.body, r#"{"error":"skin tokens cannot use the terminal"}"#, "the teaching 403 stays");
    // A socket upgrade to the server-wide bus is refused before the upgrade.
    let text = raw_async(
        port,
        "GET",
        "/cli/sessions/events".into(),
        format!("Authorization: Bearer {pass}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n"),
    )
    .await;
    assert!(text.starts_with("HTTP/1.1 403"), "{text}");

    // The two arms that took passes before they had rows still work.
    let r = post(port, "/cli/thread/catchup", &pass, json!({ "threads": [{ "addr": sales.handle, "newest": true }] })).await;
    assert_eq!(r.status, 200, "catch-up: {}", r.body);
    let r = bearer(port, "GET", &format!("/cli/thread/attachment?addr={}&id=nope&n=0&mode=meta", sales.handle), &pass).await;
    assert_ne!(r.status, 401, "attachments still resolve the pass: {}", r.body);
    let r = bearer(port, "GET", "/cli/skin/me", &pass).await;
    assert_eq!(r.status, 200, "{}", r.body);
    // A Thread POST with no row keeps its own refusal.
    let r = post(port, "/cli/thread/ask", &pass, json!({})).await;
    assert_eq!((r.status, r.body.as_str()), (403, r#"{"error":"skin tokens cannot use this room"}"#));

    // The owner token is untouched by the fence.
    let r = get(port, "/cli/projects/list", OWNER, "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    drop(d);
}

// ── C / R3 ───────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn me_lists_my_agents_and_other_rooms_do_not_exist() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let sales = seed_ws(&env, &format!("sales{tag}"));
    let ops = seed_ws(&env, &format!("ops{tag}"));
    let ghost = format!("nosuch{tag}");
    let caps = [
        "thread:read",
        "thread:post",
        "files:read",
        "tickets:read",
        "activity:read",
        "room:members",
        "heartbeats:read",
        "agent:identity",
        "store:read",
    ];
    let pass = guest(port, &format!("ada{tag}"), &sales, &caps).await;
    // Someone else holds ops (it exists and is granted, just not to ada).
    let _other = guest(port, &format!("bo{tag}"), &ops, &caps).await;

    let me = get(port, "/cli/skin/me", &pass, "").await;
    assert_eq!(me.status, 200, "{}", me.body);
    let me = me.json();
    let agents = me["agents"].as_array().expect("agents");
    assert_eq!(agents.len(), 1, "{me}");
    let a = &agents[0];
    assert_eq!(a["handle"], sales.handle.as_str());
    assert_eq!(a["room"], sales.id.as_str());
    assert!(a["displayName"].is_string() && a.get("color").is_some() && a.get("image").is_some(), "{a}");
    assert_eq!(a["can"]["chat"], true);
    assert_eq!(a["can"]["send"], true);
    assert_eq!(a["can"]["files"], "read");
    assert_eq!(a["can"]["tickets"], "read");
    assert_eq!(a["can"]["members"], true);
    assert_eq!(me["rooms"].as_array().map(|r| r.len()), Some(1), "rooms stays for older Apps");
    for k in ["serverId", "publicHost", "instanceId"] {
        assert!(me.get(k).is_some(), "me has {k}: {me}");
    }
    assert!(!me.to_string().contains(&ops.id) && !me.to_string().contains(&ops.handle), "never names a room it doesn't hold");

    // R3: on every room-taking verb, ops (granted elsewhere) and a room that
    // doesn't exist answer the same bytes; the granted room answers.
    let probes: Vec<(&str, Box<dyn Fn(&str) -> String>)> = vec![
        ("/cli/thread", Box::new(|h: &str| format!("addr={}", enc(h)))),
        ("/cli/fs/read-dir", Box::new(|h: &str| format!("workspace={}&path=.", enc(h)))),
        ("/cli/skin/members", Box::new(|h: &str| format!("workspace={}", enc(h)))),
        ("/cli/feedback/list", Box::new(|h: &str| format!("project={}", enc(h)))),
        ("/cli/activity/snapshot", Box::new(|h: &str| format!("workspace={}", enc(h)))),
        ("/cli/heartbeat/list", Box::new(|h: &str| format!("workspace={}", enc(h)))),
        ("/cli/agent/identity", Box::new(|h: &str| format!("workspace={}", enc(h)))),
        ("/cli/store/list", Box::new(|h: &str| format!("workspace={}", enc(h)))),
    ];
    for (route, q) in &probes {
        let mine = get(port, route, &pass, &q(&sales.handle)).await;
        let theirs = get(port, route, &pass, &q(&ops.handle)).await;
        let none = get(port, route, &pass, &q(&ghost)).await;
        assert_eq!((theirs.status, theirs.body.clone()), (none.status, none.body.clone()), "{route}: a room held elsewhere is an oracle");
        assert_eq!(theirs.status, 403, "{route}: {}", theirs.body);
        assert_ne!((mine.status, mine.body.clone()), (theirs.status, theirs.body.clone()), "{route}: the probe must reach the granted room ({})", mine.body);
    }
    // The catch-up entry for a room not held looks like one that doesn't exist.
    let r1 = post(port, "/cli/thread/catchup", &pass, json!({ "threads": [{ "addr": ops.handle, "newest": true }] })).await;
    let r2 = post(port, "/cli/thread/catchup", &pass, json!({ "threads": [{ "addr": ghost, "newest": true }] })).await;
    let strip = |r: &Resp, h: &str| r.body.replace(h, "ADDR");
    assert_eq!((r1.status, strip(&r1, &ops.handle)), (r2.status, strip(&r2, &ghost)), "catch-up: {} vs {}", r1.body, r2.body);
    drop(d);
}

// ── E ────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phones_stay_signed_in_with_rotating_refresh() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let sales = seed_ws(&env, &format!("sales{tag}"));
    let user = format!("dana{tag}");
    let plain = guest(port, &user, &sales, &["thread:read"]).await;

    // Today's login: no device, no refresh.
    let me = get(port, "/cli/skin/devices", &plain, "").await;
    assert_eq!(me.status, 200, "{}", me.body);
    assert_eq!(me.json()["devices"], json!([]));

    // A bad device shape is a 400 before any password work.
    let r = login(port, json!({ "username": user, "password": "s3cret-horse", "device": { "id": "has space" } })).await;
    assert_eq!(r.status, 400, "{}", r.body);

    let r = login(port, json!({ "username": user, "password": "s3cret-horse", "device": { "id": "iphone-15", "name": "Dana's iPhone" } })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let first = r.json();
    let refresh1 = first["refresh"].as_str().expect("refresh").to_string();
    let pass1 = first["token"].as_str().expect("token").to_string();
    assert!(refresh1.starts_with("k2skr_"));
    assert_eq!(first["device"], json!({ "id": "iphone-15", "name": "Dana's iPhone" }));
    assert!(first["refreshExpiresAt"].as_i64().unwrap() > first["passExpiresAt"].as_i64().unwrap() - 1);
    assert_eq!(first["roomHandles"], json!([sales.handle]));

    let devs = get(port, "/cli/skin/devices", &pass1, "").await.json();
    assert_eq!(devs["devices"].as_array().map(|d| d.len()), Some(1));
    assert_eq!(devs["devices"][0]["current"], true);
    assert_eq!(devs["devices"][0]["name"], "Dana's iPhone");

    let r = post(port, "/cli/skin/refresh", "", json!({ "refresh": refresh1 })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let second = r.json();
    let refresh2 = second["refresh"].as_str().unwrap().to_string();
    let pass2 = second["token"].as_str().unwrap().to_string();
    assert_ne!(refresh2, refresh1);
    assert_eq!(get(port, "/cli/skin/me", &pass1, "").await.status, 401, "the old pass ended");
    assert_eq!(get(port, "/cli/skin/me", &pass2, "").await.status, 200);

    // Reuse: the spent token ends the device's sign-in, pass and all.
    let r = post(port, "/cli/skin/refresh", "", json!({ "refresh": refresh1 })).await;
    assert_eq!((r.status, code(&r)), (401, "refresh_reused".to_string()), "{}", r.body);
    assert_eq!(get(port, "/cli/skin/me", &pass2, "").await.status, 401);
    let r = post(port, "/cli/skin/refresh", "", json!({ "refresh": refresh2 })).await;
    assert_eq!((r.status, code(&r)), (401, "refresh_invalid".to_string()), "{}", r.body);

    // Revoke own device; then logout ends a device sign-in too.
    let r = login(port, json!({ "username": user, "password": "s3cret-horse", "device": { "id": "ipad" } })).await.json();
    let (p, rf) = (r["token"].as_str().unwrap().to_string(), r["refresh"].as_str().unwrap().to_string());
    let id = get(port, "/cli/skin/devices", &p, "").await.json()["devices"][0]["id"].as_str().unwrap().to_string();
    let r = post(port, "/cli/skin/devices/revoke", &plain, json!({ "id": "nope" })).await;
    assert_eq!((r.status, code(&r)), (404, "not_found".to_string()));
    let r = post(port, "/cli/skin/devices/revoke", &p, json!({ "id": id })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(post(port, "/cli/skin/refresh", "", json!({ "refresh": rf })).await.status, 401);

    let r = login(port, json!({ "username": user, "password": "s3cret-horse", "device": { "id": "pixel" } })).await.json();
    let (p, rf) = (r["token"].as_str().unwrap().to_string(), r["refresh"].as_str().unwrap().to_string());
    assert_eq!(post(port, "/cli/skin/logout", &p, json!({})).await.status, 200);
    assert_eq!(post(port, "/cli/skin/refresh", "", json!({ "refresh": rf })).await.status, 401, "logout ends refresh");

    // The owner sets Stay signed in; 0 = today's login even with a device.
    owner_post(port, "/cli/skin/access/settings", json!({ "app": "host", "staySignedInDays": 0 })).await;
    let r = login(port, json!({ "username": user, "password": "s3cret-horse", "device": { "id": "pixel" } })).await.json();
    assert!(r.get("refresh").is_none(), "off: {r}");
    let r = post(port, "/cli/skin/access/settings", OWNER, json!({ "app": "host", "staySignedInDays": 91 })).await;
    assert_eq!(r.status, 400, "{}", r.body);
    drop(d);
}

// ── F ────────────────────────────────────────────────────────────────

fn service(ws: &Ws, name: &str, port: u16) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let dir = ws.path.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    k2_core::published_services::insert(&conn, &ws.id, name, "node server.mjs", &dir.to_string_lossy(), port, "local", "stopped", "cmd", "")
        .expect("insert service")
        .id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn discovery_is_per_app_and_a_ghost_when_off() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let shop = seed_ws(&env, &format!("shop{tag}"));
    let crm = seed_ws(&env, &format!("crm{tag}"));
    let a = service(&shop, "jobs", 47_201);
    let b = service(&crm, "crm", 47_202);
    let notes: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let sink = notes.clone();
    k2_daemon::apps::alerts::set_sink(Some(Box::new(move |ws: &str, text: &str| sink.lock().unwrap().push((ws.to_string(), text.to_string())))));

    // A /cli/skin route that doesn't exist (the family a prober compares).
    let nothing = raw_async(port, "GET", format!("/cli/skin/no-such-route-{tag}"), String::new()).await;
    assert!(nothing.starts_with("HTTP/1.1 404"), "{nothing}");
    let ghost = |pq: String| raw_async(port, "GET", pq, String::new());
    // Off by default: exactly a route that doesn't exist (status, headers, body).
    assert_eq!(ghost(format!("/cli/skin/app-info?app={a}")).await, nothing);
    assert_eq!(ghost("/cli/skin/app-info".to_string()).await, nothing);
    assert_eq!(ghost("/cli/skin/app-info?app=host".to_string()).await, nothing);

    // The owner lists one App.
    let r = owner_post(port, "/cli/skin/access/listed", json!({ "app": a, "listed": true })).await;
    assert_eq!(r["changed"], true);
    let r = get(port, "/cli/skin/app-info", "", &format!("app={a}")).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(keys, ["app", "kind", "name", "signIn"], "only the App itself: {v}");
    assert_eq!((v["app"].as_str(), v["name"].as_str(), v["kind"].as_str()), (Some(a.as_str()), Some("jobs"), Some("cmd")));
    let sign: Vec<&str> = v["signIn"].as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(sign, ["methods", "staysSignedIn", "url"]);
    for leak in ["serverId", "version", "features", "catalog", "crm", &crm.handle, &shop.handle, &b] {
        assert!(!r.body.contains(leak), "app-info leaks {leak}: {}", r.body);
    }
    // The other App stays invisible.
    assert_eq!(ghost(format!("/cli/skin/app-info?app={b}")).await, nothing);
    // Audited, and the App's agent gets an inbox note.
    let audit = std::fs::read_to_string(k2_core::auth_audit::path()).unwrap_or_default();
    assert!(audit.contains("app_discovery") && audit.contains(&a), "{audit}");
    assert!(notes.lock().unwrap().iter().any(|(ws, t)| ws == &shop.id && t.contains("listed for sign-in discovery")), "{:?}", notes.lock().unwrap());

    // Who may list: the App's own agent with switch 2, nobody else.
    let crm_agent = passport(&crm);
    let r = post(port, "/cli/skin/access/listed", &crm_agent, json!({ "app": b, "listed": true })).await;
    assert_eq!(r.status, 403, "switch 2 off: {}", r.body);
    allow_manage(&crm, true);
    k2_core::workspace::settings::set_agents_can_manage_apps_full(&crm.id, true).unwrap();
    let r = post(port, "/cli/skin/access/listed", &crm_agent, json!({ "app": a, "listed": false })).await;
    assert_eq!(r.status, 403, "another App's agent: {}", r.body);
    let r = post(port, "/cli/skin/access/listed", &crm_agent, json!({ "app": b, "listed": true })).await;
    assert_eq!(r.status, 200, "its own App with switch 2: {}", r.body);
    assert_eq!(get(port, "/cli/skin/app-info", "", &format!("app={b}")).await.status, 200);
    // And off again: a ghost again.
    owner_post(port, "/cli/skin/access/listed", json!({ "app": a, "listed": false })).await;
    assert_eq!(ghost(format!("/cli/skin/app-info?app={a}")).await, nothing);
    k2_daemon::apps::alerts::set_sink(None);
    drop(d);
}

// ── C: templates and the owner's grid ────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn role_templates_and_the_people_grid() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let shop = seed_ws(&env, &format!("shop{tag}"));
    let app = service(&shop, "jobs", 47_301);

    let roles_before = get(port, "/cli/skin/roles", OWNER, "").await.json();
    assert!(roles_before["roles"].as_array().unwrap().iter().all(|r| r["appId"] != app.as_str()), "a new App has no roles");
    let t = get(port, "/cli/skin/roles/suggest", OWNER, "").await;
    assert_eq!(t.status, 200, "{}", t.body);
    let ids: Vec<String> = t.json()["templates"].as_array().unwrap().iter().map(|x| x["id"].as_str().unwrap().to_string()).collect();
    assert_eq!(ids, ["owner", "manager", "staff", "field", "client", "vendor", "auditor", "app"]);
    let r = owner_post(port, "/cli/skin/roles/from-template", json!({ "template": "client", "app": app })).await;
    let role = r["role"]["name"].as_str().unwrap().to_string();
    assert_eq!(role, "jobs-client");
    assert_eq!(r["role"]["roomHandles"], json!([shop.handle]), "defaults to the App's own room");

    owner_post(port, "/cli/skin/users", json!({ "username": format!("cleo{tag}"), "app": app, "role": role })).await;
    let g = get(port, "/cli/skin/access", OWNER, &format!("app={app}")).await;
    assert_eq!(g.status, 200, "{}", g.body);
    let g = g.json();
    assert_eq!(g["people"][0]["username"], format!("cleo{tag}"));
    assert_eq!(g["people"][0]["cells"][0]["handle"], shop.handle.as_str());
    assert_eq!(g["people"][0]["cells"][0]["folders"]["read"], json!(["Shared"]));
    assert_eq!(g["settings"]["staySignedInDays"], 30);
    assert_eq!(g["settings"]["listed"], false);

    // A pass never reads the owner's grid or makes roles (the gate).
    let pass = guest(port, &format!("ada{tag}"), &shop, &["thread:read"]).await;
    for (m, p) in [("GET", "/cli/skin/access"), ("GET", "/cli/skin/roles/suggest"), ("POST", "/cli/skin/roles/from-template")] {
        let r = bearer(port, if m == "GET" { "GET" } else { "POST" }, p, &pass).await;
        assert_eq!(r.status, 403, "{p}: {}", r.body);
    }
    drop(d);
}
