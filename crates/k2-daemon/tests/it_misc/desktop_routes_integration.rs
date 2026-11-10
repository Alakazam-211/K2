//! Remote desktop routes on the wire (prd-remote-desktop-tab-v1 D2),
//! headless: a real daemon on a port, temp HOME, no deskd (the routes run
//! on the `NotBuilt` backend, which says why there's no picture).
//!
//! 1. Every POST route answers GET with 405, for every caller, before auth.
//! 2. People only: an agent passport, an App pass, an API key get 403
//!    `desktop_people_only` on every route; no token is the plain 403.
//! 3. Route floors: an Admin login gets 403 on enable (the switch is the
//!    Owner's); a Voice & Chat only login is refused even status.
//! 4. A Member's status shows only itself; an Admin opens without a grant.
//! 5. The switch is not in AppSettings: an Admin's settings write of a
//!    look-alike key leaves it off.
//! 6. `/boot-status` lists `remote-desktop-v1`.
//! 7. The owner token walks every route (the headless curl pass), and each
//!    write leaves an access-log line.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use k2_core::connect_users::{self, Role};
use k2_core::session::SessionId;
use k2_daemon::session_token::{self, HookPrincipal};
use k2_daemon::test_harness;
use serde_json::{json, Value};

const OWNER: &str = "desktop-routes-owner-token";

struct Resp {
    status: u16,
    body: Value,
    raw: String,
}

/// One request. `remote` adds `X-Forwarded-For`, so a loopback test
/// connection is classified as a LAN caller (not this machine).
fn http(port: u16, method: &str, path: &str, body: Option<&str>, remote: bool) -> Resp {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(30))).expect("timeout");
    let fwd = if remote { "X-Forwarded-For: 203.0.113.9\r\n" } else { "" };
    let req = match body {
        Some(b) => format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n{fwd}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n{fwd}\r\n"),
    };
    s.write_all(req.as_bytes()).expect("write");
    let mut raw = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let text = String::from_utf8_lossy(&raw).to_string();
        if let Some((head, rest)) = text.split_once("\r\n\r\n") {
            let len = head
                .lines()
                .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok()));
            if len.is_some_and(|n| rest.len() >= n) {
                break;
            }
        }
        match s.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&chunk[..n]),
            Err(e) => panic!("read {method} {path}: {e}"),
        }
    }
    let text = String::from_utf8_lossy(&raw).to_string();
    let (head, rest) = text.split_once("\r\n\r\n").unwrap_or_else(|| panic!("no HTTP response to {method} {path}: {text:?}"));
    let status: u16 = head.split_whitespace().nth(1).and_then(|c| c.parse().ok()).expect("status code");
    let body = serde_json::from_str(rest).unwrap_or(Value::Null);
    Resp { status, body, raw: rest.to_string() }
}

fn q(path: &str, tok: &str) -> String {
    format!("{path}?token={tok}")
}

fn login(name: &str, role: Role) -> String {
    connect_users::add_user(name, "password123").unwrap_or_else(|e| panic!("add_user {name}: {e}"));
    if role != Role::Member {
        connect_users::set_role(name, role).unwrap_or_else(|e| panic!("set_role {name}: {e}"));
    }
    connect_users::create_session(name)
}

const POST_ROUTES: &[&str] = &[
    "/cli/desktop/disable",
    "/cli/desktop/enable",
    "/cli/desktop/grant",
    "/cli/desktop/kick",
    "/cli/desktop/open",
    "/cli/desktop/policy",
    "/cli/desktop/revoke",
    "/cli/desktop/ticket",
];
const GET_ROUTES: &[&str] = &[
    "/cli/desktop/audit",
    "/cli/desktop/displays",
    "/cli/desktop/grants",
    "/cli/desktop/status",
    "/cli/desktop/viewers",
];

fn send(port: u16, path: &str, tok: &str, body: &str, remote: bool) -> Resp {
    if POST_ROUTES.contains(&path) {
        http(port, "POST", &q(path, tok), Some(body), remote)
    } else {
        http(port, "GET", &q(path, tok), None, remote)
    }
}

fn audit_events() -> Vec<String> {
    k2_core::desktop::audit::read(None, 1000)
        .expect("audit")
        .iter()
        .map(|l| l["event"].as_str().expect("event").to_string())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn desktop_routes_on_the_wire() {
    let _home = k2_core::test_env::TempHome::new();
    k2_core::db::init_for_tests();
    let d = test_harness::start(OWNER).await;
    let port = d.port;

    let admin = login("dt-admin", Role::Admin);
    let member = login("dt-member", Role::Member);
    let voice = login("dt-voice", Role::VoiceChat);
    let passport = session_token::mint_session_token(
        &SessionId::new(),
        "pane-desktop",
        HookPrincipal { workspace_uuid: "ws-desktop".into(), agent_address: "desktop-agent".into() },
        session_token::CredMode::ApiKey,
        session_token::Provider::Anthropic,
    );
    let app_pass = "k2skn_0123456789abcdef0123456789abcdef";
    let api_key = "k2sk_0123456789abcdef0123456789abcdef";

    // 1. GET on every POST route: 405, whoever asks.
    for path in POST_ROUTES {
        for tok in [OWNER, admin.as_str(), member.as_str(), passport.as_str(), app_pass, ""] {
            let r = http(port, "GET", &q(path, tok), None, true);
            assert_eq!(r.status, 405, "GET {path} as {tok:.12}: {}", r.raw);
        }
    }
    // A POST to a read route is not allowed either.
    for path in GET_ROUTES {
        let r = http(port, "POST", &q(path, OWNER), Some("{}"), false);
        assert_eq!(r.status, 405, "POST {path}: {}", r.raw);
    }

    // 2. People only.
    for path in POST_ROUTES.iter().chain(GET_ROUTES) {
        for (what, tok) in [("agent passport", passport.as_str()), ("App pass", app_pass), ("API key", api_key)] {
            let r = send(port, path, tok, "{}", false);
            assert_eq!(r.status, 403, "{what} {path}: {}", r.raw);
            assert_eq!(r.body["error"]["code"], "desktop_people_only", "{what} {path}: {}", r.raw);
        }
        let r = send(port, path, "", "{}", false);
        assert_eq!(r.status, 403, "no token {path}: {}", r.raw);
        assert_ne!(r.body["error"]["code"], "desktop_people_only", "no token is not told about people: {}", r.raw);
    }
    assert!(audit_events().is_empty(), "refusals write nothing");

    // 3. Floors: the switch is the Owner's, never an Admin's.
    let r = send(port, "/cli/desktop/enable", &admin, "{}", true);
    assert_eq!(r.status, 403, "{}", r.raw);
    assert_eq!((r.body["error"].as_str(), r.body["required"].as_str()), (Some("role_required"), Some("owner")), "{}", r.raw);
    for path in ["/cli/desktop/grants", "/cli/desktop/audit", "/cli/desktop/viewers", "/cli/desktop/policy"] {
        let r = send(port, path, &admin, r#"{"hideApps":[]}"#, true);
        assert_eq!(r.status, 403, "Admin {path}: {}", r.raw);
    }
    let r = send(port, "/cli/desktop/status", &voice, "", true);
    assert_eq!(r.status, 403, "Voice & Chat only never reaches the desktop: {}", r.raw);
    let r = send(port, "/cli/desktop/status", OWNER, "", false);
    assert_eq!(r.body["enabled"], false, "{}", r.raw);

    // 5. The switch isn't in AppSettings.
    let r = http(port, "POST", &q("/cli/settings/update", &admin), Some(r#"{"remoteDesktopEnabled":true,"desktopEnabled":true}"#), true);
    assert!(r.status == 200 || r.status == 400 || r.status == 403, "{}", r.raw);
    let r = send(port, "/cli/desktop/status", OWNER, "", false);
    assert_eq!(r.body["enabled"], false, "a settings write never turns it on: {}", r.raw);

    // 6. boot-status.
    let r = http(port, "GET", &q("/boot-status", OWNER), None, false);
    assert_eq!(r.status, 200, "{}", r.raw);
    let features: Vec<&str> = r.body["features"].as_array().expect("features").iter().filter_map(Value::as_str).collect();
    assert!(features.contains(&"remote-desktop-v1"), "{features:?}");

    // 7. The owner token walks every route.
    let r = send(port, "/cli/desktop/enable", OWNER, "{}", false);
    assert_eq!(r.status, 200, "{}", r.raw);
    assert_eq!(r.body["enabled"], true);
    assert!(r.body["card"].as_str().is_some_and(|c| c.contains("passwords")), "{}", r.raw);
    let r = send(port, "/cli/desktop/status", OWNER, "", false);
    assert_eq!(r.status, 200, "{}", r.raw);
    assert_eq!((r.body["enabled"].as_bool(), r.body["state"].as_str()), (Some(true), Some("unavailable")), "no deskd yet: {}", r.raw);
    assert_eq!(r.body["you"]["mayManage"], true, "{}", r.raw);
    assert_eq!(r.body["you"]["mayView"], false, "the owner token never watches: {}", r.raw);
    let r = send(port, "/cli/desktop/displays", OWNER, "", false);
    assert_eq!((r.status, r.body["displays"].clone()), (200, json!([])), "{}", r.raw);
    let r = send(port, "/cli/desktop/policy", OWNER, r#"{"hideApps":["com.apple.Passwords"]}"#, false);
    assert_eq!((r.status, r.body["hideApps"].clone()), (200, json!(["com.apple.Passwords"])), "{}", r.raw);
    let r = send(port, "/cli/desktop/grant", OWNER, r#"{"user":"dt-member","mode":"control"}"#, false);
    assert_eq!((r.status, r.body["error"]["code"].as_str()), (400, Some("member_view_only")), "{}", r.raw);
    let r = send(port, "/cli/desktop/grant", OWNER, r#"{"user":"dt-member"}"#, false);
    assert_eq!(r.status, 200, "{}", r.raw);
    let r = send(port, "/cli/desktop/grants", OWNER, "", false);
    assert_eq!(r.body["grants"][0]["user"], "dt-member", "{}", r.raw);
    assert_eq!(r.body["grants"][0]["mode"], "view", "{}", r.raw);

    // 4. A Member with a grant watches (from off the machine) and sees
    // only itself; an Admin opens without one.
    let r = send(port, "/cli/desktop/status", &member, "", true);
    assert_eq!(r.status, 200, "{}", r.raw);
    assert_eq!(r.body["you"], json!({"role": "member", "mayView": true, "mayControl": false, "mayManage": false, "sameMachine": false}), "{}", r.raw);
    assert!(r.body.get("grants").is_none() && r.body.get("viewers").is_none(), "{}", r.raw);
    let r = send(port, "/cli/desktop/ticket", &member, "{}", true);
    assert_eq!(r.status, 200, "{}", r.raw);
    assert!(r.body["ticket"].as_str().is_some_and(|t| t.starts_with("k2dt_")), "{}", r.raw);
    let r = send(port, "/cli/desktop/status", &admin, "", true);
    assert_eq!((r.body["you"]["mayView"].as_bool(), r.body["you"]["mayControl"].as_bool()), (Some(true), Some(true)), "{}", r.raw);
    // The same Member on this machine: no watching.
    let r = send(port, "/cli/desktop/ticket", &member, "{}", false);
    assert_eq!(r.body["error"]["code"], "same_machine", "{}", r.raw);

    let r = send(port, "/cli/desktop/viewers", OWNER, "", false);
    assert_eq!((r.status, r.body["viewers"].clone()), (200, json!([])), "{}", r.raw);
    let r = send(port, "/cli/desktop/kick", OWNER, r#"{"user":"dt-member"}"#, false);
    assert_eq!((r.status, r.body["kicked"].as_u64()), (200, Some(0)), "{}", r.raw);
    let r = send(port, "/cli/desktop/open", OWNER, "{}", false);
    assert_eq!((r.status, r.body["for"].as_str()), (200, Some("owner")), "{}", r.raw);
    let r = send(port, "/cli/desktop/revoke", OWNER, r#"{"user":"dt-member"}"#, false);
    assert_eq!((r.status, r.body["revoked"].as_bool()), (200, Some(true)), "{}", r.raw);
    let r = send(port, "/cli/desktop/disable", OWNER, "{}", false);
    assert_eq!((r.status, r.body["enabled"].as_bool()), (200, Some(false)), "{}", r.raw);
    let r = http(port, "GET", &(q("/cli/desktop/audit", OWNER) + "&limit=50"), None, false);
    assert_eq!(r.status, 200, "{}", r.raw);
    let events: Vec<&str> = r.body["lines"].as_array().expect("lines").iter().filter_map(|l| l["event"].as_str()).collect();
    assert_eq!(events, vec!["disable", "revoke", "open", "kick", "ticket", "grant", "policy", "enable"], "{}", r.raw);
    let all = r.raw.clone();
    assert!(!all.contains(OWNER) && !all.contains(&member), "the access log never holds a token");
}
