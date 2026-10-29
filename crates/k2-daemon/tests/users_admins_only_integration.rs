//! R1 (prd-agent-app-users-v1 S1, Rosson 2026-10-24): `/cli/users*` — the
//! K2 server and admin portal logins — is for admins only. No agent
//! credential reaches it, whatever switch is on. Through the REAL
//! dispatcher on a temp `$HOME`:
//!
//! - T1.2: the arm list is read from `dispatcher.rs` itself; an arm this
//!   file has no case for fails the test.
//! - T1.1: every arm, with an agent passport (`?token=` and Bearer, the
//!   workspace's "manage Apps" switch on), a K2 shell tab's passport, a
//!   `k2skn_` login session, a `k2skn_` platform pass, an App's link pass,
//!   an App key (`k2sk_`), a Connect Member login and a Sew chat pass: 403
//!   every time, and the Connect user store is byte-for-byte unchanged
//!   afterwards.
//!
//! Fail loudly: every assertion names the arm and the credential.

#[path = "support/app_fixture.rs"]
mod app_fixture;
#[path = "support/sew_ready.rs"]
mod sew_ready;

use std::io::{Read, Write};
use std::time::Duration;

use app_fixture::*;
use serde_json::json;

const DISPATCHER_SRC: &str = include_str!("../src/routes/dispatcher.rs");

/// Every arm this test knows, with the methods it is called by.
const KNOWN_ARMS: &[(&str, &[&str])] = &[
    ("/cli/users", &["GET"]),
    ("/cli/users/add", &["POST"]),
    ("/cli/users/remove", &["POST"]),
    ("/cli/users/set-password", &["POST"]),
    ("/cli/users/unlock", &["POST"]),
    ("/cli/users/set-disabled", &["POST"]),
    ("/cli/users/set-role", &["POST"]),
    ("/cli/users/set-can-clone", &["POST"]),
    ("/cli/users/policy", &["GET", "POST"]),
    ("/cli/users/audit", &["GET"]),
];

/// The quoted `/cli/users…` arm paths in the dispatcher source.
fn dispatcher_arms() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let needle = "\"/cli/users";
    let mut rest = DISPATCHER_SRC;
    while let Some(i) = rest.find(needle) {
        let after = &rest[i + 1..];
        let end = after.find('"').expect("closing quote");
        let path = &after[..end];
        let tail = &after[end + 1..];
        // An arm is a match pattern: `"/cli/users…" =>` (maybe with a guard).
        let is_arm = tail.trim_start().starts_with("=>") || tail.trim_start().starts_with("if ") || tail.trim_start().starts_with('|');
        if is_arm && !out.iter().any(|p| p == path) {
            out.push(path.to_string());
        }
        rest = &after[end + 1..];
    }
    out
}

fn raw(port: u16, method: &str, pq: &str, bearer: Option<&str>, body: Option<&str>) -> (u16, String) {
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let mut head = format!("{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
    if let Some(b) = bearer {
        head.push_str(&format!("Authorization: Bearer {b}\r\n"));
    }
    match body {
        Some(b) => head.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}", b.len())),
        None if method == "POST" => head.push_str("Content-Length: 0\r\n\r\n"),
        None => head.push_str("\r\n"),
    }
    s.write_all(head.as_bytes()).unwrap();
    let mut out = Vec::new();
    match s.read_to_end(&mut out) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
        Err(e) => panic!("{method} {pq}: {e}"),
    }
    let text = String::from_utf8_lossy(&out).to_string();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|x| x.parse().ok())
        .unwrap_or_else(|| panic!("{method} {pq}: no status in {text:?}"));
    (status, text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default())
}

/// A body each POST arm would act on if it were let in.
fn body_for(path: &str) -> String {
    match path {
        "/cli/users/add" => json!({ "username": "intruder", "password": "Intruder-pass-123" }),
        "/cli/users/policy" => json!({ "minLength": 4 }),
        "/cli/users/set-role" => json!({ "username": "admin1", "role": "owner" }),
        "/cli/users/set-disabled" => json!({ "username": "admin1", "disabled": true }),
        "/cli/users/set-can-clone" => json!({ "username": "admin1", "canClone": true }),
        _ => json!({ "username": "admin1", "password": "Changed-pass-123" }),
    }
    .to_string()
}

fn store_snapshot() -> (Vec<u8>, String) {
    let bytes = std::fs::read(k2_core::connect_users::store_path()).expect("connect-users store exists");
    let users = serde_json::to_string(&k2_core::connect_users::list_users().expect("list users")).unwrap();
    (bytes, users)
}

#[test]
fn t1_2_every_users_arm_in_the_dispatcher_has_a_case() {
    let arms = dispatcher_arms();
    assert!(arms.len() >= KNOWN_ARMS.len(), "found only {arms:?}");
    for a in &arms {
        assert!(
            KNOWN_ARMS.iter().any(|(k, _)| k == a),
            "dispatcher arm {a} has no case in KNOWN_ARMS: add it (and prove no agent reaches it)"
        );
    }
    for (k, _) in KNOWN_ARMS {
        assert!(arms.iter().any(|a| a == k), "KNOWN_ARMS lists {k}, which the dispatcher no longer has: {arms:?}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t1_1_no_agent_credential_reaches_any_users_arm() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let ws = seed_ws(&env, &format!("crm{tag}"));
    allow_manage(&ws, true);
    let agent = passport(&ws);

    // A Connect admin to aim at, so a write that got in would show.
    k2_core::connect_users::add_user("admin1", "Admin-pass-1234").expect("add admin");
    k2_core::connect_users::set_role("admin1", k2_core::connect_users::Role::Admin).expect("admin role");

    // A guest login session, a platform pass and an App key.
    let svc = {
        let dir = ws.path.join("app");
        std::fs::create_dir_all(&dir).unwrap();
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::published_services::insert(&conn, &ws.id, "app", "node s.mjs", &dir.to_string_lossy(), 47_101, "local", "stopped", "cmd", "")
            .expect("service")
            .id
    };
    let guest = format!("g{tag}");
    let r = post(port, "/cli/skin/users", OWNER, json!({ "username": guest })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    k2_daemon::test_harness::guest_with_own_password(&guest, "s3cret-horse");
    let login = {
        let b = json!({ "username": guest, "password": "s3cret-horse" }).to_string();
        tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(b))).await.unwrap()
    };
    assert_eq!(login.status, 200, "{}", login.body);
    let session = login.json()["token"].as_str().expect("session").to_string();
    let r = post(port, "/cli/skin-tokens", OWNER, json!({ "name": format!("p{tag}"), "caps": ["thread:read"], "rooms": [ws.handle] })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let platform = r.json()["token"].as_str().expect("pass").to_string();
    let (_, app_key) = k2_core::api_keys::create_app_key("t1", &svc, r#"{"scopes":["users","rooms"]}"#).expect("app key");
    assert!(app_key.starts_with("k2sk_"));
    assert!(session.starts_with("k2skn_") && platform.starts_with("k2skn_"));

    // A K2 shell tab's passport: a live login-shell PTY (a human typing
    // inside K2, `Who::Shell`).
    let shell = {
        let cfg = k2_core::terminal::DaemonPtyConfig { cols: 80, rows: 24, cwd: Some(ws.path.clone()), program: None, ..Default::default() };
        let pty = k2_core::terminal::DaemonPtySession::spawn(cfg).expect("spawn a login shell");
        k2_daemon::v2_session_map::register(format!("shell-{tag}"), std::sync::Arc::clone(&pty));
        k2_daemon::session_token::mint_session_token(
            &pty.session_id,
            &pty.session_id.to_string(),
            k2_daemon::session_token::HookPrincipal { workspace_uuid: ws.id.clone(), agent_address: format!("shell-{tag}") },
            k2_daemon::session_token::CredMode::ApiKey,
            k2_daemon::session_token::Provider::Anthropic,
        )
    };
    // An App's link pass (valid only on its own link, never here).
    let link = k2_core::skin::link::mint_link_pass(&svc, "app", &format!("gw:{}", 47_102), &["thread:read".to_string()], std::slice::from_ref(&ws.id))
        .expect("link pass")
        .raw;
    // A Connect Member login.
    k2_core::connect_users::add_user("member1", "Member-pass-1234").expect("add member");
    let member = k2_core::connect_users::create_session("member1");
    // A Sew chat pass minted for a guest with a chat grant.
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    let mut row = k2_daemon::sew::store::Row::new("0.1.2", 0);
    row.status = "running".into();
    row.install_id = Some("123456789".into());
    k2_daemon::sew::store::save(&row).expect("sew row");
    // Sew answers its health probe ready (the 0.3.1 mint gate).
    sew_ready::serve();
    let chat_guest = format!("cg{tag}");
    let cg = k2_core::skin::add_principal(&chat_guest).expect("chat guest");
    k2_core::skin::insert_chat_grant_for_tests(&cg.id).expect("chat grant");
    let (_, cg_session) = k2_core::skin::create_session_token(&chat_guest).expect("chat guest login");
    let chat_pass = {
        let (pq, tok) = ("/cli/sew/pass".to_string(), cg_session.clone());
        let (status, body) = tokio::task::spawn_blocking(move || raw(port, "POST", &pq, Some(&tok), Some("{}"))).await.unwrap();
        assert_eq!(status, 200, "a chat pass to try: {body}");
        serde_json::from_str::<serde_json::Value>(&body).unwrap()["pass"].as_str().expect("pass").to_string()
    };

    let before = store_snapshot();
    let creds: Vec<(&str, String, bool)> = vec![
        ("agent passport ?token=", agent.clone(), false),
        ("agent passport Bearer (manage Apps on)", agent.clone(), true),
        ("K2 shell tab passport", shell.clone(), false),
        ("k2skn_ login session", session.clone(), false),
        ("k2skn_ platform pass", platform.clone(), false),
        ("App link pass", link.clone(), true),
        ("App key k2sk_", app_key.clone(), true),
        ("Connect Member login", member.clone(), false),
        ("Sew chat pass", chat_pass.clone(), true),
    ];
    let mut checked = 0usize;
    for (path, methods) in KNOWN_ARMS {
        for method in *methods {
            for (what, cred, as_bearer) in &creds {
                let pq = if *as_bearer { path.to_string() } else { format!("{path}?token={}", enc(cred)) };
                let body = (*method == "POST").then(|| body_for(path));
                let (method_s, pq_s, cred_s, bearer) = (method.to_string(), pq.clone(), cred.clone(), *as_bearer);
                let (status, resp) = tokio::task::spawn_blocking(move || {
                    raw(port, &method_s, &pq_s, bearer.then_some(cred_s.as_str()), body.as_deref())
                })
                .await
                .unwrap();
                // The one read a Connect login (a person, not an agent) has:
                // the password policy, so a login screen can show its rules.
                if *what == "Connect Member login" && *method == "GET" && *path == "/cli/users/policy" {
                    assert_eq!(status, 200, "{resp}");
                    assert!(resp.contains("minLength") && !resp.contains("\"users\""), "{resp}");
                    checked += 1;
                    continue;
                }
                assert_eq!(status, 403, "{method} {path} with {what}: must be 403, got {status} {resp}");
                assert!(!resp.contains("\"success\":true") && !resp.contains("\"users\""), "{method} {path} with {what}: {resp}");
                checked += 1;
            }
        }
    }
    assert_eq!(checked, 11 * creds.len(), "every arm × method × credential ran");
    assert_eq!(store_snapshot(), before, "the Connect user store is unchanged");
    assert!(!k2_core::connect_users::verify("intruder", "Intruder-pass-123"), "no user was added");
    assert!(k2_core::connect_users::verify("admin1", "Admin-pass-1234"), "the admin's password is unchanged");

    // The control: the owner token does reach it.
    let r = get(port, "/cli/users", OWNER, "").await;
    assert_eq!(r.status, 200, "{}", r.body);
}
