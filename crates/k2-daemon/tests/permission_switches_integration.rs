//! Permission cards lane E1 (prd-agent-settings-cards-v1 rev 3.1 §6.4,
//! §13.2, §13.3 "Parity"): on a real daemon with both switches on, every
//! E1 switch row through **both doors**, over HTTP:
//!
//! - the card door: the agent asks (`POST /cli/permission/request` with its
//!   passport), a person answers on the Thread (`POST /cli/thread/answer`
//!   with their login);
//! - the Settings door: the same person posts the Settings route itself.
//!
//! For each row: a Member login is refused at both doors (and an Admin on
//! the Owner rows); the allowed person applies it at both doors, and the
//! stored state is the same. Then `garden.keep-mine` from the phone (an
//! Owner login) and the widened Settings claim.
//!
//! Its own binary: it flips the process-wide switch (lane B's rule).

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::path::PathBuf;
use std::time::Duration;

use k2_core::connect_users::Role;
use k2_core::db::schema::WorkspaceSession;
use k2_core::session::SessionId;
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;
use serde_json::{json, Value};

const OWNER: &str = "owner-token-permission-e1";

struct Resp {
    status: u16,
    body: String,
}

fn parse(text: &str) -> Resp {
    let line = text.lines().next().unwrap_or_else(|| panic!("no status line: {text:?}"));
    let status = line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or_else(|| panic!("bad status: {line}"));
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    Resp { status, body }
}

fn tcp_blocking(port: u16, method: &str, pq: &str, body: Option<String>) -> Resp {
    let mut s = StdTcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let raw = match body {
        Some(b) => format!(
            "{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!("{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"),
    };
    s.write_all(raw.as_bytes()).unwrap();
    let mut out = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match s.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => out.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => break,
            Err(e) => panic!("{method} {pq}: {e}"),
        }
    }
    parse(&String::from_utf8_lossy(&out))
}

/// `path` may carry its own query; the token is appended.
async fn post(port: u16, path: &str, token: &str, body: Option<Value>) -> Resp {
    let sep = if path.contains('?') { '&' } else { '?' };
    let pq = format!("{path}{sep}token={token}");
    let body = body.map(|b| b.to_string());
    tokio::task::spawn_blocking(move || tcp_blocking(port, "POST", &pq, body)).await.unwrap()
}

fn json_of(r: &Resp) -> Value {
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("not JSON ({e}): {} {}", r.status, r.body))
}

struct Home {
    home: PathBuf,
    prev: Option<std::ffi::OsString>,
}
impl Drop for Home {
    fn drop(&mut self) {
        match self.prev.take() {
            Some(p) => std::env::set_var("HOME", p),
            None => std::env::remove_var("HOME"),
        }
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

struct Ws {
    id: String,
    handle: String,
    path: String,
}

fn workspace(home: &std::path::Path, handle: &str, pinned: bool) -> Ws {
    let path = home.join("ws").join(handle);
    std::fs::create_dir_all(&path).unwrap();
    let path = path.to_string_lossy().to_string();
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)", rusqlite::params![id, handle, path]).unwrap();
    if pinned {
        let conv = uuid::Uuid::new_v4().to_string();
        WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &id, None, Some(&conv), "claude", "system", "running").unwrap();
    }
    Ws { id, handle: handle.to_string(), path }
}

fn login(user: &str, role: Role) -> String {
    k2_core::connect_users::add_user(user, "password-for-e1-int-9").expect("add_user");
    if role != Role::Member {
        k2_core::connect_users::set_role(user, role).expect("set_role");
    }
    k2_core::connect_users::create_session(user)
}

/// One E1 row: its catalog id, request values, who may answer, the
/// Settings route (path with query, body), and how to read its state.
struct Row {
    action: &'static str,
    params: Value,
    owner_only: bool,
    settings: fn(&Ws) -> (String, Option<Value>),
    state: fn(&Ws) -> bool,
}

fn access(toggle: &'static str) -> impl Fn(&Ws) -> (String, Option<Value>) {
    move |w| ("/cli/agent-access/set".to_string(), Some(json!({ "toggle": toggle, "workspace": w.id, "value": true })))
}

macro_rules! access_row {
    ($action:literal, $toggle:literal, $owner:expr, $state:expr) => {
        Row {
            action: $action,
            params: json!({}),
            owner_only: $owner,
            settings: |w| access($toggle)(w),
            state: $state,
        }
    };
}

fn rows() -> Vec<Row> {
    use k2_core::workspace::settings as s;
    vec![
        access_row!("compute.use", "compute", false, |w| {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::compute::store::agents_can_use_compute(&conn, &w.id)
        }),
        access_row!("compute.operate", "compute_operate", false, |w| {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::compute::admin::agents_can_operate_compute(&conn, &w.id)
        }),
        access_row!("compute.manage", "compute_manage", true, |w| s::agents_can_manage_compute(&w.id)),
        access_row!("agents.manage", "agents_manage", false, |w| s::agents_can_manage_agents(&w.id)),
        access_row!("apps.build", "apps_build", true, |w| s::agents_app_switches(&w.id).0),
        access_row!("apps.manage-full", "apps_manage_full", true, |w| s::agents_app_switches(&w.id).1),
        Row {
            action: "apps.core",
            params: json!({ "core": "db" }),
            owner_only: true,
            settings: |w| ("/cli/agent-access/set".to_string(), Some(json!({ "toggle": "app_core", "workspace": w.id, "core": "db", "value": true }))),
            state: |w| s::agents_app_cores(&w.id).iter().any(|c| c == "db"),
        },
        access_row!("chat.manage", "chat_manage", false, |w| s::agents_can_manage_chat(&w.id)),
        access_row!("vault.manage", "vault_manage", false, |w| s::agents_can_manage_vault(&w.id)),
        access_row!("gardens.manage", "gardens_manage", false, |w| s::agents_can_manage_gardens(&w.id)),
        Row {
            action: "dns.manage",
            params: json!({}),
            owner_only: false,
            settings: |w| (format!("/cli/dns-manage?project={}&enable=1", w.path), None),
            state: |w| s::get_dns_manage_enabled(&w.path),
        },
        Row {
            action: "connections.create",
            params: json!({}),
            owner_only: false,
            settings: |w| (format!("/cli/agents-create-connections?project={}&enable=1", w.path), None),
            state: |w| s::get_agents_can_create_connections(&w.path),
        },
        Row {
            action: "mail.manage",
            params: json!({}),
            owner_only: false,
            settings: |w| ("/cli/mail-manage".to_string(), Some(json!({ "project": w.path, "enable": 1 }))),
            state: |w| s::mail_manage_allowed_for_path(&w.path),
        },
        Row {
            action: "mail.send-mode",
            params: json!({ "mode": "on" }),
            owner_only: false,
            settings: |w| ("/cli/mail/config/set".to_string(), Some(json!({ "workspace": w.path, "agentSend": "on" }))),
            state: |w| s::mail_agent_send_for_path(&w.path) == "on",
        },
        Row {
            action: "db.agent-access",
            params: json!({ "level": "write" }),
            owner_only: false,
            settings: |w| ("/cli/workspace/set".to_string(), Some(json!({ "project": w.path, "fields": { "db_agent_access": "write" } }))),
            state: |w| s::db_agent_access_for_path(&w.path) == "write",
        },
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_e1_switch_applies_the_same_through_the_card_and_settings() {
    for (k, _) in std::env::vars() {
        if k.starts_with("K2_") || k.starts_with("K2SO_") {
            std::env::remove_var(k);
        }
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let home = PathBuf::from(format!("/tmp/k2pe1{:x}", nanos % 0xffff_ffff));
    std::fs::create_dir_all(&home).unwrap();
    let home = dunce::canonicalize(&home).unwrap();
    let _home = Home { prev: std::env::var_os("HOME"), home: home.clone() };
    std::env::set_var("HOME", &home);

    let d = test_harness::start(OWNER).await;
    let port = d.port;
    let owner = login("e1owner", Role::Owner);
    let admin = login("e1admin", Role::Admin);
    let member = login("e1member", Role::Member);

    // Both switches on for the card workspace.
    let r = post(port, "/cli/settings/update", OWNER, Some(json!({ "permissionCards": true }))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let rows = rows();
    let mut done: Vec<&str> = Vec::new();
    for (n, row) in rows.iter().enumerate() {
        // A fresh pair per row: A asks with a card, B is set in Settings.
        let a = workspace(&home, &format!("e1a{n}"), true);
        let b = workspace(&home, &format!("e1b{n}"), false);
        let r = post(port, "/cli/agent-access/set", OWNER, Some(json!({ "toggle": "permission_cards", "workspace": a.id, "value": true }))).await;
        assert_eq!(r.status, 200, "{}", r.body);
        if row.action == "apps.manage-full" {
            // Switch 2 sits under switch 1, on both workspaces.
            for w in [&a, &b] {
                let r = post(port, "/cli/agent-access/set", OWNER, Some(json!({ "toggle": "apps_build", "workspace": w.id, "value": true }))).await;
                assert_eq!(r.status, 200, "{}", r.body);
            }
        }
        let sid = SessionId::new();
        let passport = k2_daemon::session_token::mint_session_token(
            &sid,
            &format!("pane-e1-{n}"),
            HookPrincipal { workspace_uuid: a.id.clone(), agent_address: a.handle.clone() },
            CredMode::ApiKey,
            Provider::Anthropic,
        );
        assert!(!(row.state)(&a) && !(row.state)(&b), "{} starts off", row.action);

        // ── the card door ─────────────────────────────────────────
        let r = post(
            port,
            "/cli/permission/request",
            &passport,
            Some(json!({ "action": row.action, "params": row.params, "why": "Run the suite on the minis" })),
        )
        .await;
        assert_eq!(r.status, 200, "{}: {}", row.action, r.body);
        let v = json_of(&r);
        assert_eq!(v["status"], "pending", "{}: {v}", row.action);
        let card = v["card"].as_str().unwrap_or_else(|| panic!("no card: {v}")).to_string();
        let answer = |who: String, label: &'static str| {
            let body = json!({ "addr": a.handle, "id": card, "answer": label });
            async move { post(port, "/cli/thread/answer", &who, Some(body)).await }
        };
        let r = answer(member.clone(), "Allow").await;
        assert_eq!((r.status, json_of(&r)["error"]["code"].as_str()), (403, Some("action_card_person_only")), "{}: {}", row.action, r.body);
        if row.owner_only {
            let r = answer(admin.clone(), "Allow").await;
            assert_eq!((r.status, json_of(&r)["error"]["required"].as_str()), (403, Some("owner")), "{}: {}", row.action, r.body);
        }
        assert!(!(row.state)(&a), "{}: a refused answer applies nothing", row.action);
        let allowed = if row.owner_only { owner.clone() } else { admin.clone() };
        let r = answer(allowed.clone(), "Allow").await;
        assert_eq!(r.status, 200, "{}: {}", row.action, r.body);
        assert_eq!(json_of(&r)["action"]["result"]["outcome"], "approved", "{}: {}", row.action, r.body);
        assert!((row.state)(&a), "{} is on through the card", row.action);

        // ── the Settings door ─────────────────────────────────────
        let (path, body) = (row.settings)(&b);
        let r = post(port, &path, &member, body.clone()).await;
        assert_eq!(r.status, 403, "{}: a Member is refused in Settings too: {}", row.action, r.body);
        if row.owner_only {
            let r = post(port, &path, &admin, body.clone()).await;
            assert_eq!(r.status, 403, "{}: an Admin can't raise it in Settings either: {}", row.action, r.body);
        }
        assert!(!(row.state)(&b), "{}: nothing applied by a refusal", row.action);
        let r = post(port, &path, &allowed, body).await;
        assert_eq!(r.status, 200, "{}: {}", row.action, r.body);
        assert_eq!(((row.state)(&a), (row.state)(&b)), (true, true), "{}: the same state through both doors", row.action);
        done.push(row.action);
    }
    assert_eq!(done.len(), 15, "{done:?}");

    // ── connections.add: one connection, through both doors ──────────
    let a = workspace(&home, "e1conn-a", true);
    let peer = workspace(&home, "e1conn-peer", false);
    let b = workspace(&home, "e1conn-b", false);
    let r = post(port, "/cli/agent-access/set", OWNER, Some(json!({ "toggle": "permission_cards", "workspace": a.id, "value": true }))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let sid = SessionId::new();
    let passport = k2_daemon::session_token::mint_session_token(
        &sid,
        "pane-e1-conn",
        HookPrincipal { workspace_uuid: a.id.clone(), agent_address: a.handle.clone() },
        CredMode::ApiKey,
        Provider::Anthropic,
    );
    let r = post(port, "/cli/permission/request", &passport, Some(json!({ "action": "connections.add", "params": { "to": peer.handle }, "why": "Talk to the peer" }))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let card = json_of(&r)["card"].as_str().expect("card").to_string();
    let r = post(port, "/cli/thread/answer", &member, Some(json!({ "addr": a.handle, "id": card, "answer": "Allow" }))).await;
    assert_eq!(r.status, 403, "{}", r.body);
    let r = post(port, "/cli/thread/answer", &admin, Some(json!({ "addr": a.handle, "id": card, "answer": "Allow" }))).await;
    assert_eq!((r.status, json_of(&r)["action"]["result"]["outcome"].as_str()), (200, Some("approved")), "{}", r.body);
    let r = post(port, &format!("/cli/connections?project={}&action=add&target={}", b.path, peer.handle), &admin, None).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(k2_core::connections::are_local_peers(&a.id, &peer.id) && k2_core::connections::are_local_peers(&b.id, &peer.id));

    // ── the Garden's owner from the phone ───────────────────────────
    // Keep as mine is retired (0.46.3: Garden access is always on), for
    // every caller; widget resume keeps the Owner-login gate it shared.
    for who in [&admin, &owner] {
        let r = post(port, "/cli/zen/widget/claim", who, Some(json!({ "widget": "board" }))).await;
        assert_eq!((r.status, json_of(&r)["error"].as_str()), (410, Some("retired")), "{}", r.body);
    }
    let r = post(port, "/cli/zen/widget/resume", &admin, Some(json!({ "widget": "board" }))).await;
    assert_eq!((r.status, json_of(&r)["error"].as_str()), (403, Some("owner_only")), "{}", r.body);
    let r = post(port, "/cli/zen/widget/resume", &owner, Some(json!({ "widget": "board" }))).await;
    assert_ne!(json_of(&r)["error"].as_str(), Some("owner_only"), "an Owner login is past the gate: {}", r.body);
    assert_ne!(json_of(&r)["error"].as_str(), Some("zen_local_only"), "{}", r.body);
    let r = post(port, "/cli/zen/garden/lock", &owner, Some(json!({ "garden": "g", "locked": true }))).await;
    assert_eq!((r.status, json_of(&r)["error"].as_str()), (403, Some("owner_only")), "the lock stays owner-token only: {}", r.body);

    let r = post(port, "/cli/settings/update", OWNER, Some(json!({ "permissionCards": false }))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    drop(d);
}
