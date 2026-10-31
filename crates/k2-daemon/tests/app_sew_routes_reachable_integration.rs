//! The App's own chat routes reach their handlers through the REAL
//! dispatcher (headless, a temp `$HOME`, over loopback):
//!
//! - `GET /cli/sew/app/member`, `POST /cli/sew/app/whois` (chat.member,
//!   chat.whois) and `POST /cli/sew/app/channels` (chat.createChannel)
//!   answer with their own handlers' refusals, on the main port with the
//!   App's remote pass and inside the wall on its link.
//!
//! Until 0.46.7 these three were matched inside the dispatcher's
//! `/cli/skin` branch, which a `/cli/sew/...` path never enters: every call
//! fell through to the `/cli/` catch-all, whose token check refused an
//! App's pass with a bare 403 `forbidden`. These tests fail on that base.
//!
//! ISOLATION: a temp `$HOME`, the in-memory test DB. No Sew, no real
//! `~/.k2`, no running daemon.

#[path = "support/app_fixture.rs"]
mod app_fixture;

use app_fixture::*;
use k2_daemon::apps::gateway::{self, LinkSpec};
use serde_json::json;

const MEMBER: &str = "/cli/sew/app/member";
const WHOIS: &str = "/cli/sew/app/whois";
const CHANNELS: &str = "/cli/sew/app/channels";

/// One request with `Authorization: Bearer` (how an App calls its link).
fn bearer_req(port: u16, method: &str, path: &str, pass: &str, body: Option<&str>) -> Resp {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(std::time::Duration::from_secs(60))).unwrap();
    let host = "127.0.0.1:8730";
    let raw = match body {
        Some(b) => format!(
            "{method} {path} HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {pass}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {pass}\r\nConnection: close\r\n\r\n"),
    };
    s.write_all(raw.as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    let text = String::from_utf8_lossy(&out).to_string();
    let status = text.lines().next().and_then(|l| l.split_whitespace().nth(1)).and_then(|x| x.parse().ok()).unwrap_or_else(|| panic!("no status: {text:?}"));
    Resp { status, body: text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default() }
}

async fn bearer(port: u16, method: &'static str, path: String, pass: String, body: Option<serde_json::Value>) -> Resp {
    tokio::task::spawn_blocking(move || bearer_req(port, method, &path, &pass, body.map(|b| b.to_string()).as_deref())).await.unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_apps_chat_map_and_channel_routes_reach_their_handlers() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let own = seed_ws(&env, &format!("chatws{tag}"));
    let svc = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::published_services::insert(&conn, &own.id, &format!("chatapp{tag}"), "node s.mjs", &own.path.to_string_lossy(), 47_121, "local", "stopped", "cmd", "")
            .expect("service")
            .id
    };
    // A pass without chat:read or chat:channels: each handler's own cap
    // refusal proves the call reached it.
    let remote = k2_core::skin::link::mint_remote_pass(&svc, "chatapp", &["thread:read".to_string()], &[own.id.clone()], 3600).unwrap().raw;

    let r = get(port, MEMBER, &remote, "username=dana").await;
    assert_eq!((r.status, r.code()), (403, "cap_not_granted".to_string()), "member: {}", r.body);
    assert_eq!(r.json()["error"]["cap"], json!("chat:read"), "{}", r.body);
    let r = post(port, WHOIS, &remote, json!({ "memberIds": ["7"] })).await;
    assert_eq!((r.status, r.code()), (403, "cap_not_granted".to_string()), "whois: {}", r.body);
    let r = get(port, WHOIS, &remote, "").await;
    // A POST route: refused for GET (the route policy answers before the handler).
    assert_eq!(r.status, 405, "whois is a POST: {}", r.body);
    let r = post(port, CHANNELS, &remote, json!({ "slug": "desk", "name": "Desk" })).await;
    assert_eq!((r.status, r.code()), (403, "cap_not_granted".to_string()), "channels: {}", r.body);
    assert_eq!(r.json()["error"]["cap"], json!("chat:channels"), "{}", r.body);
    let r = get(port, CHANNELS, &remote, "").await;
    // A POST route: refused for GET (the route policy answers before the handler).
    assert_eq!(r.status, 405, "channels is a POST: {}", r.body);

    // The owner token is never these routes' credential: the routes' own
    // answer, not the catch-all's.
    for (path, is_post) in [(MEMBER, false), (WHOIS, true), (CHANNELS, true)] {
        let r = if is_post { post(port, path, OWNER, json!({})).await } else { get(port, path, OWNER, "username=dana").await };
        assert_eq!((r.status, r.code()), (403, "app_pass_required".to_string()), "{path} with the owner token: {}", r.body);
    }

    // Inside the wall: the App's link pass on its link.
    let spec = LinkSpec {
        app_id: format!("chatapp{tag}"),
        service_id: svc.clone(),
        workspace_id: own.id.clone(),
        port: 0,
        caps: vec!["thread:read".into()],
        rooms: vec![own.id.clone()],
    };
    let pass = gateway::open(spec, None).expect("open link");
    let lport = gateway::port_of(&format!("chatapp{tag}")).expect("listening");
    let r = bearer(lport, "GET", format!("{MEMBER}?username=dana"), pass.clone(), None).await;
    assert_eq!((r.status, r.code()), (403, "cap_not_granted".to_string()), "member on the link: {}", r.body);
    let r = bearer(lport, "POST", WHOIS.into(), pass.clone(), Some(json!({ "memberIds": ["7"] }))).await;
    assert_eq!((r.status, r.code()), (403, "cap_not_granted".to_string()), "whois on the link: {}", r.body);
    let r = bearer(lport, "POST", CHANNELS.into(), pass.clone(), Some(json!({ "slug": "desk", "name": "Desk" }))).await;
    assert_eq!((r.status, r.code()), (403, "cap_not_granted".to_string()), "channels on the link: {}", r.body);
    gateway::close(&format!("chatapp{tag}"), None, true);
    drop(env);
}
