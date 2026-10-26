//! RS-A15 `guests:rooms`, headless (room-scope amendment 2026-10-18): the
//! real dispatcher in process on a temp `$HOME`, over loopback, the way
//! Agent NSI's Jobs BFF calls it to replace `POST /cli/skin/users/rooms`
//! with the owner token.
//!
//! - the owner token and Connect logins never reach `/cli/skin/guests/rooms`;
//! - an App's remote pass (main port) and its link pass (inside the wall)
//!   read and set their own guests' rooms once the owner grants
//!   `guests-rooms` (`POST /cli/app/grant`), and not before;
//! - a guest's own session pass is refused even on its App's link;
//! - an agent sets its own App's guests within the grant over the CLI
//!   routes; it can't grant itself.

#![cfg(unix)]

#[path = "support/app_fixture.rs"]
mod app_fixture;

use app_fixture::*;
use k2_daemon::apps::gateway::{self, LinkSpec};
use serde_json::{json, Value};

const ROUTE: &str = "/cli/skin/guests/rooms";

fn bearer_req(port: u16, method: &str, pq: &str, pass: &str, body: Option<&str>, host: &str) -> Resp {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(std::time::Duration::from_secs(60))).unwrap();
    let raw = match body {
        Some(b) => format!(
            "{method} {pq} HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {pass}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!("{method} {pq} HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {pass}\r\nConnection: close\r\n\r\n"),
    };
    s.write_all(raw.as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    let text = String::from_utf8_lossy(&out).to_string();
    let status = text.lines().next().and_then(|l| l.split_whitespace().nth(1)).and_then(|x| x.parse().ok()).unwrap_or_else(|| panic!("no status: {text:?}"));
    Resp { status, body: text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default() }
}

async fn bearer(port: u16, method: &'static str, pq: String, pass: String, body: Option<Value>, host: &'static str) -> Resp {
    tokio::task::spawn_blocking(move || bearer_req(port, method, &pq, &pass, body.map(|b| b.to_string()).as_deref(), host)).await.unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_sets_its_own_guests_rooms_without_the_owner_token() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let own = seed_ws(&env, &format!("jobsws{tag}"));
    let acme = seed_ws(&env, &format!("acme{tag}"));
    let beta = seed_ws(&env, &format!("beta{tag}"));
    let svc = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::published_services::insert(&conn, &own.id, &format!("jobs{tag}"), "node s.mjs", &own.path.to_string_lossy(), 47_101, "local", "stopped", "cmd", "")
            .expect("service")
            .id
    };
    let dana = format!("dana{tag}");
    k2_core::skin::add_principal_for_app(&dana, None, &svc, None, "owner").unwrap();
    let remote = k2_core::skin::link::mint_remote_pass(&svc, "jobs", &["thread:read".to_string()], &[own.id.clone()], 3600).unwrap().raw;
    let set_body = |rooms: &[&str]| json!({ "username": dana, "rooms": rooms });

    // The owner token is never this route's credential.
    let r = post(port, ROUTE, OWNER, set_body(&[&acme.handle])).await;
    assert_eq!((r.status, r.code()), (403, "app_pass_required".to_string()), "{}", r.body);
    // Nor a Connect login (the route floor admits none).
    k2_core::connect_users::add_user(&format!("ada{tag}"), "correct horse battery 9").unwrap();
    let login = k2_core::connect_users::create_session(&format!("ada{tag}"));
    let r = post(port, ROUTE, &login, set_body(&[&acme.handle])).await;
    assert_eq!(r.status, 403, "a Connect login: {}", r.body);

    // The App's remote pass, before the owner's grant: 403.
    let r = bearer(port, "POST", ROUTE.into(), remote.clone(), Some(set_body(&[&acme.handle])), "127.0.0.1").await;
    assert_eq!((r.status, r.code()), (403, "grant_required".to_string()), "{}", r.body);

    // An agent can't grant its own App; the owner can.
    let agent = passport(&own);
    let r = post(port, "/cli/app/grant", &agent, json!({ "app": svc, "kind": "guests-rooms", "rooms": [acme.handle] })).await;
    assert_eq!((r.status, r.json()["status"].clone()), (202, json!("held")), "another workspace's room: a held request: {}", r.body);
    assert!(k2_core::skin::guest_rooms::get_grant(&svc).unwrap().is_none());
    let r = get(port, "/cli/app/grant", OWNER, "").await;
    assert_eq!(r.status, 405, "POST only: {}", r.body);
    let r = post(port, "/cli/app/grant", OWNER, json!({ "app": svc, "kind": "guests-rooms", "rooms": [acme.handle] })).await;
    assert_eq!(r.status, 200, "{}", r.body);

    // Remote pass, main port: set, read, the same rooms again, outside rooms.
    let r = bearer(port, "POST", ROUTE.into(), remote.clone(), Some(set_body(&[&acme.handle, &own.handle])), "127.0.0.1").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert_eq!((v["changed"].clone(), v["app"].clone()), (json!(true), json!(svc)));
    assert_eq!(v["roomHandles"], json!([acme.handle, own.handle]));
    let r = bearer(port, "GET", format!("{ROUTE}?username={dana}"), remote.clone(), None, "127.0.0.1").await;
    assert_eq!((r.status, r.json()["changed"].clone()), (200, json!(false)), "{}", r.body);
    let r = bearer(port, "POST", ROUTE.into(), remote.clone(), Some(set_body(&[&own.handle, &acme.handle])), "127.0.0.1").await;
    assert_eq!(r.json()["changed"], json!(false), "idempotent: {}", r.body);
    let r = bearer(port, "POST", ROUTE.into(), remote.clone(), Some(set_body(&[&beta.handle])), "127.0.0.1").await;
    assert_eq!((r.status, r.code()), (403, "outside_app_rooms".to_string()), "{}", r.body);
    // ?token= works too (sockets' form; the same pass).
    let r = get(port, ROUTE, &remote, &format!("username={dana}")).await;
    assert_eq!(r.status, 200, "{}", r.body);

    // Inside the wall: the App's link pass on its link.
    let spec = LinkSpec {
        app_id: format!("jobs{tag}"),
        service_id: svc.clone(),
        workspace_id: own.id.clone(),
        port: 0,
        caps: vec!["thread:read".into()],
        rooms: vec![own.id.clone()],
    };
    let pass = gateway::open(spec, None).expect("open link");
    let lport = gateway::port_of(&format!("jobs{tag}")).expect("listening");
    let r = bearer(lport, "POST", ROUTE.into(), pass.clone(), Some(set_body(&[&own.handle])), "127.0.0.1:8730").await;
    assert_eq!(r.status, 200, "on the link: {}", r.body);
    assert_eq!(r.json()["roomHandles"], json!([own.handle]));
    let r = bearer(lport, "GET", format!("{ROUTE}?username={dana}"), pass.clone(), None, "127.0.0.1:8730").await;
    assert_eq!(r.status, 200, "{}", r.body);
    // The App's own guest's session pass: refused, even on its App's link.
    k2_core::skin::set_principal_password(&dana, Some("s3cret-horse")).unwrap();
    let guest = k2_core::skin::create_app_session_token(&dana, &svc).unwrap().raw;
    let r = bearer(lport, "POST", ROUTE.into(), guest.clone(), Some(set_body(&[])), "127.0.0.1:8730").await;
    assert_eq!((r.status, r.code()), (403, "app_pass_required".to_string()), "{}", r.body);
    gateway::close(&format!("jobs{tag}"), None, true);

    // An agent, end to end over the CLI routes, within the grant: with
    // "Build and run Apps" (0.46.4: without it, refused).
    let r = post(port, "/cli/app/guest/rooms", &agent, json!({ "app": svc, "username": dana, "rooms": [acme.handle] })).await;
    assert_eq!((r.status, r.code()), (403, "apps_manage_off".to_string()), "{}", r.body);
    k2_core::workspace::settings::set_agents_can_manage_skin(&own.path.to_string_lossy(), true).unwrap();
    let r = post(port, "/cli/app/guest/rooms", &agent, json!({ "app": svc, "username": dana, "rooms": [acme.handle] })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["beforeHandles"], json!([own.handle]));
    let r = post(port, "/cli/app/guest/rooms", &agent, json!({ "app": svc, "username": dana, "rooms": [beta.handle] })).await;
    assert_eq!((r.status, r.code()), (403, "outside_app_rooms".to_string()), "{}", r.body);
    let r = get(port, "/cli/app/guest/rooms", &agent, &format!("app={svc}&username={dana}")).await;
    assert_eq!(r.status, 200, "{}", r.body);

    // Every change is audited with before and after.
    let rows = k2_core::skin::guest_rooms::audit_rows(&svc, 20).unwrap();
    let changes: Vec<_> = rows.iter().filter(|r| r.event == k2_core::skin::guest_rooms::AUDIT_CHANGE).collect();
    assert_eq!(changes.len(), 3, "remote, link, agent: {rows:?}");
    assert!(changes[1].actor.starts_with("app:jobs"), "{:?}", changes[1].actor);
    assert_eq!(changes[2].actor, format!("app-remote:{svc}"));
    drop(env);
}
