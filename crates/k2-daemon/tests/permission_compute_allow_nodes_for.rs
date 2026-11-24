//! A managing agent asks to change ANOTHER workspace's compute node list
//! on a card (0.46.18, Rosson 2026-11-23: "This should be a card you can
//! present. Go ahead and build it.").
//!
//! On a real in-process daemon, with "Permission cards (Beta)" on, an
//! agent passport on the TCP port:
//! - the agent that hired a workspace naming it (`k2 compute allow-nodes
//!   --add N --workspace WS`, and `--remove`, `--clear`) is refused
//!   `allow_nodes_for_needs_person`, and the refusal ends with `Ask with:
//!   k2 permission request compute.allow-nodes-for add=N target=WS` (or
//!   `remove=N`, `clear=true`); nothing changes;
//! - a node not granted to that workspace is `allow_nodes_not_granted`
//!   (not requestable), before any card;
//! - an agent that doesn't manage it gets `forbidden` (not requestable) on
//!   the route and `allow_nodes_not_manager` (409) at the ask;
//! - a Member and the agent itself can't answer the card; an Admin's Deny
//!   changes nothing; an Admin's Allow makes exactly the card's change to
//!   that workspace's list, and the `allow_nodes` event names the Admin and
//!   the agent that asked; the Access Audit has the answer line.
//!
//! Its own binary: it flips the process-wide compute switch (`K2_COMPUTE`).
//! No vault, keychain, real `~/.k2` or running daemon is touched.

use std::collections::BTreeMap;
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

const OWNER: &str = "owner-token-permission-compute-allow-nodes-for";
const SET: &str = "/cli/compute/allow-nodes/set";
const ACTION: &str = "compute.allow-nodes-for";

struct Resp {
    status: u16,
    body: String,
}

impl Resp {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("not JSON ({e}): {} {}", self.status, self.body))
    }
    fn code(&self) -> String {
        self.json()["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {} {}", self.status, self.body)).to_string()
    }
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

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

async fn post(port: u16, path: &str, token: &str, body: Value) -> Resp {
    let pq = format!("{path}?token={}", enc(token));
    let body = body.to_string();
    tokio::task::spawn_blocking(move || tcp_blocking(port, "POST", &pq, Some(body))).await.unwrap()
}

struct Home {
    home: PathBuf,
    prev: Option<std::ffi::OsString>,
}
impl Drop for Home {
    fn drop(&mut self) {
        k2_core::permission::gate::set_server_switch(false);
        match self.prev.take() {
            Some(p) => std::env::set_var("HOME", p),
            None => std::env::remove_var("HOME"),
        }
        std::env::remove_var("K2_COMPUTE");
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

struct Ws {
    id: String,
    handle: String,
}

fn workspace(home: &std::path::Path, handle: &str) -> Ws {
    let path = home.join("ws").join(handle);
    std::fs::create_dir_all(&path).unwrap();
    let path = path.to_string_lossy().to_string();
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)", rusqlite::params![id, handle, path]).unwrap();
    // A pinned chat: the card goes on its Thread.
    let conv = uuid::Uuid::new_v4().to_string();
    WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &id, None, Some(&conv), "claude", "system", "running").unwrap();
    k2_core::compute::store::set_agents_can_use_compute(&conn, &id, true).expect("Allow compute nodes");
    Ws { id, handle: handle.to_string() }
}

/// The hire ledger: `boss`'s agent hired `ws` (as `k2 agent hire` records it).
fn hired(boss: &Ws, ws: &Ws) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let inst = format!("inst-{}", uuid::Uuid::new_v4().simple());
    let by = k2_core::hire::HireCaller::Agent { workspace_id: boss.id.clone() };
    k2_core::hire::ledger::insert_applying(&conn, &inst, &by, None, "h", None, None, 1).expect("instance");
    k2_core::hire::ledger::add_member(&conn, &inst, &ws.id, "lead", &ws.handle, "/w", true).expect("member");
}

fn login(user: &str, role: Role) -> String {
    k2_core::connect_users::add_user(user, "password-for-pcanf-int-9").expect("add_user");
    if role != Role::Member {
        k2_core::connect_users::set_role(user, role).expect("set_role");
    }
    k2_core::connect_users::create_session(user)
}

fn passport(ws: &Ws, pane: &str) -> String {
    k2_daemon::session_token::mint_session_token(
        &SessionId::new(),
        pane,
        HookPrincipal { workspace_uuid: ws.id.clone(), agent_address: ws.handle.clone() },
        CredMode::ApiKey,
        Provider::Anthropic,
    )
}

/// An active node named `name`, granted to each of `to`. Returns its id.
fn node(name: &str, to: &[&Ws]) -> String {
    let id = format!("node-pcanf-{name}");
    let db = k2_core::db::shared();
    let conn = db.lock();
    let labels = BTreeMap::from([("os".to_string(), "linux".to_string()), ("arch".to_string(), "x86_64".to_string())]);
    k2_core::compute::store::insert_pending_node(&conn, &id, name, &k2_core::compute::proto::crypto::sha256_hex(id.as_bytes()), "pem", &labels, "owner", "123456", 5)
        .expect("enroll");
    k2_core::compute::store::confirm_node(&conn, &id, "owner", 6).expect("confirm");
    for ws in to {
        k2_core::compute::store::set_grant(&conn, &id, &ws.id, &Default::default(), "owner", 7).expect("grant");
    }
    id
}

/// The workspace's list (node ids, sorted), or `None` for no list.
fn list_of(ws: &Ws) -> Option<Vec<String>> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::compute::allow::get(&conn, &ws.id).expect("read the list").map(|e| {
        let mut n = e.nodes;
        n.sort();
        n
    })
}

fn sorted(ids: &[&str]) -> Option<Vec<String>> {
    let mut v: Vec<String> = ids.iter().map(|s| s.to_string()).collect();
    v.sort();
    Some(v)
}

fn state_of(id: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT state FROM permission_requests WHERE id = ?1", [id], |r| r.get(0))
        .unwrap_or_else(|e| panic!("request {id}: {e}"))
}

fn asks(ws: &Ws) -> i64 {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT COUNT(*) FROM permission_requests WHERE workspace_id = ?1", [&ws.id], |r| r.get(0))
        .expect("count permission requests")
}

/// The newest `allow_nodes` event of `ws`: (actor, detail).
fn last_event(ws: &Ws) -> (String, Value) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let (actor, detail): (String, String) = conn
        .query_row(
            "SELECT actor, detail_json FROM compute_events WHERE kind = 'allow_nodes' AND workspace_id = ?1 ORDER BY id DESC LIMIT 1",
            [&ws.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("an allow_nodes event");
    (actor, serde_json::from_str(&detail).expect("detail JSON"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_managing_agent_asks_to_change_another_workspaces_node_list() {
    for (k, _) in std::env::vars() {
        if k.starts_with("K2_") || k.starts_with("K2SO_") {
            std::env::remove_var(k);
        }
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let home = k2_core::test_env::short_tmp(format!("k2pcanf{:x}", nanos % 0xffff_ffff));
    std::fs::create_dir_all(&home).unwrap();
    let home = dunce::canonicalize(&home).unwrap();
    let _home = Home { prev: std::env::var_os("HOME"), home: home.clone() };
    std::env::set_var("HOME", &home);
    std::env::set_var("K2_COMPUTE", "1");

    let d = test_harness::start(OWNER).await;
    let port = d.port;
    let admin = login("pcanfadmin", Role::Admin);
    let member = login("pcanfmember", Role::Member);
    let boss = workspace(&home, "pcanf-boss");
    let eng = workspace(&home, "pcanf-engine");
    let stranger = workspace(&home, "pcanf-stranger");
    hired(&boss, &eng);
    let la = node("pcanf-la", &[&eng]);
    let lb = node("pcanf-lb", &[&eng]);
    let _lc = node("pcanf-lc", &[&boss]);
    let pb = passport(&boss, "pane-pcanf-boss");
    let ps = passport(&stranger, "pane-pcanf-stranger");

    let r = post(port, SET, OWNER, json!({ "workspace": eng.id, "nodes": "pcanf-la" })).await;
    assert_eq!(r.status, 200, "{}", r.body);

    // Cards on: the server's switch and each asking agent's own.
    let r = post(port, "/cli/settings/update", OWNER, json!({ "permissionCards": true })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    for ws in [&boss, &stranger] {
        let r = post(port, "/cli/agent-access/set", OWNER, json!({ "toggle": "permission_cards", "workspace": ws.id, "value": true })).await;
        assert_eq!(r.status, 200, "{}", r.body);
    }

    // ── the refusals ────────────────────────────────────────────────────
    // The hiring agent names eng's list: each change ends with its card.
    for (body, key, value) in [
        (json!({ "workspace": eng.id, "add": "pcanf-lb" }), "add", "pcanf-lb"),
        (json!({ "workspace": eng.handle, "remove": "pcanf-la" }), "remove", "pcanf-la"),
        (json!({ "workspace": eng.handle, "clear": true }), "clear", "true"),
    ] {
        let r = post(port, SET, &pb, body.clone()).await;
        assert_eq!((r.status, r.code()), (403, "allow_nodes_for_needs_person".to_string()), "{body}: {}", r.body);
        let v = r.json();
        assert_eq!((&v["error"]["workspace"], &v["error"][key]), (&json!(eng.handle), &json!(value)), "{body}: {v}");
        let hint = v["error"]["hint"].as_str().unwrap_or("");
        assert!(hint.contains("pcanf-boss manages pcanf-engine (it hired pcanf-engine)"), "{body}: {hint}");
        // The values print in key order: the change, then the workspace.
        let tail = format!("permission request {ACTION} {key}={value} target={} --why \"<what you need it for>\"", eng.handle);
        let command = format!("k2 {tail}");
        let mut params = serde_json::Map::new();
        params.insert("target".into(), json!(eng.handle));
        params.insert(key.into(), json!(value));
        assert_eq!(
            v["permission"],
            // The rebrand's `tail` (the Ask line after the command's name),
            // beside `command`, on every requestable refusal (0.46.18).
            json!({ "action": ACTION, "params": params, "approvers": "owner or admin", "command": command, "tail": tail }),
            "{body}: {v}"
        );
    }
    // A node eng has no grant on: refused before any card, not requestable.
    let r = post(port, SET, &pb, json!({ "workspace": eng.id, "add": "pcanf-lc" })).await;
    assert_eq!((r.status, r.code()), (403, "allow_nodes_not_granted".to_string()), "{}", r.body);
    assert!(r.json()["error"]["hint"].as_str().unwrap_or("").contains("pcanf-lc is not granted to workspace pcanf-engine"), "{}", r.body);
    assert_eq!(r.json()["permission"]["requestable"], false, "{}", r.body);
    // An agent that doesn't manage eng: forbidden, not requestable.
    let r = post(port, SET, &ps, json!({ "workspace": eng.id, "add": "pcanf-lb" })).await;
    assert_eq!((r.status, r.code()), (403, "forbidden".to_string()), "{}", r.body);
    assert!(r.json()["error"]["hint"].as_str().unwrap_or("").contains("compute.allow-nodes-for"), "{}", r.body);
    assert_eq!(r.json()["permission"]["requestable"], false, "{}", r.body);
    assert_eq!(list_of(&eng), sorted(&[&la]), "a refusal changes nothing");

    // ── the request ─────────────────────────────────────────────────────
    let ask = |who: String, params: Value| async move {
        post(port, "/cli/permission/request", &who, json!({ "action": ACTION, "params": params, "why": "the new nodes are faster" })).await
    };
    let r = ask(ps.clone(), json!({ "target": eng.handle, "add": "pcanf-lb" })).await;
    assert_eq!((r.status, r.code()), (409, "allow_nodes_not_manager".to_string()), "{}", r.body);
    let r = ask(pb.clone(), json!({ "target": eng.handle, "add": "pcanf-lc" })).await;
    assert_eq!((r.status, r.code()), (400, "bad_params".to_string()), "an ungranted node: {}", r.body);
    assert_eq!(asks(&stranger) + asks(&boss), 0, "no request was stored for a refused ask");

    let answer = |who: String, card: String, decision: &'static str| {
        let body = json!({ "addr": boss.handle, "id": card, "answer": decision, "reason": "not now" });
        async move { post(port, "/cli/thread/answer", &who, body).await }
    };
    let card_and_id = |r: &Resp| {
        let v = r.json();
        assert_eq!(v["status"], "pending", "{v}");
        (v["card"].as_str().unwrap_or_else(|| panic!("no card: {v}")).to_string(), v["id"].as_str().unwrap_or_else(|| panic!("no id: {v}")).to_string())
    };

    // Deny (a clear): nothing changes.
    let r = ask(pb.clone(), json!({ "target": eng.handle, "clear": "true" })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let (card, id) = card_and_id(&r);
    let r = answer(member.clone(), card.clone(), "Allow").await;
    assert_eq!((r.status, r.code()), (403, "action_card_person_only".to_string()), "{}", r.body);
    let r = answer(pb.clone(), card.clone(), "Allow").await;
    assert_eq!(r.status, 403, "the agent answers its own card: {}", r.body);
    let r = answer(admin.clone(), card, "Deny").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(state_of(&id), "denied");
    assert_eq!(list_of(&eng), sorted(&[&la]), "deny changes nothing");

    // Allow (an add): exactly lb joins eng's list, as the Admin, and the
    // event says whose card it was.
    let r = ask(pb.clone(), json!({ "target": eng.handle, "add": "pcanf-lb" })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let (card, id) = card_and_id(&r);
    let r = answer(admin.clone(), card, "Allow").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["action"]["result"]["outcome"], "approved", "{}", r.body);
    assert_eq!(state_of(&id), "approved");
    assert_eq!(list_of(&eng), sorted(&[&la, &lb]), "exactly the card's node was added");
    assert_eq!(list_of(&boss), None, "the asker's own list is untouched");
    let (actor, detail) = last_event(&eng);
    assert_eq!(actor, "login:pcanfadmin", "{detail}");
    assert_eq!((&detail["askedBy"], &detail["via"]), (&json!("agent:pcanf-boss"), &json!("permission-card")), "{detail}");
    assert_eq!(detail["to"], json!(["pcanf-la", "pcanf-lb"]), "{detail}");
    let audit = std::fs::read_to_string(k2_core::auth_audit::path()).expect("the Access Audit");
    assert!(audit.lines().any(|l| l.contains(&id) && l.contains(ACTION) && l.contains("login:pcanfadmin")), "{audit}");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        assert!(k2_core::compute::store::grant(&conn, &lb, &boss.id).is_none(), "no grant is added");
    }

    // Allow (a remove): la leaves eng's list.
    let r = ask(pb.clone(), json!({ "target": eng.handle, "remove": "pcanf-la" })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let (card, id) = card_and_id(&r);
    let r = answer(admin.clone(), card, "Allow").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(state_of(&id), "approved");
    assert_eq!(list_of(&eng), sorted(&[&lb]));
    let (_, detail) = last_event(&eng);
    assert_eq!(detail["to"], json!(["pcanf-lb"]), "{detail}");

    drop(d);
}
