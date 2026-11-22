//! Adding a node to an agent's own compute node list as a permission card
//! (0.46.15, Rosson 2026-11-16: "Add the permission card(s) for that").
//!
//! On a real in-process daemon, with "Permission cards (Beta)" on for the
//! agent, an agent passport on the TCP port:
//! - `k2 compute allow-nodes --add N` (and the comma form) is refused
//!   `allow_nodes_needs_person` and ends with `Ask with: k2 permission
//!   request compute.allow-nodes nodes=N[,…]`; nothing changes;
//! - a node that isn't granted to its workspace is refused
//!   `allow_nodes_not_granted` (not requestable: a person grants it first),
//!   and an unknown node `node_not_found`, before any card;
//! - another workspace's list is `forbidden`, not requestable, and names
//!   who can (that workspace's own agent, or a person);
//! - `--clear` (every granted node at once) stays a person's: refused, the
//!   refusal says it is bigger than adding a node;
//! - narrowing its own list still needs nobody;
//! - the request can't name a workspace; a Member and the agent can't
//!   answer the card; an Admin's Deny changes nothing; an Admin's Allow adds
//!   exactly the card's nodes to that workspace's list and nothing else.
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

const OWNER: &str = "owner-token-permission-compute-allow-nodes";
const SET: &str = "/cli/compute/allow-nodes/set";

fn ask_line(nodes: &str) -> String {
    format!("k2 permission request compute.allow-nodes nodes={nodes} --why \"<what you need it for>\"")
}

/// The same line after the command's name (the permission object's `tail`).
fn ask_tail(nodes: &str) -> String {
    format!("permission request compute.allow-nodes nodes={nodes} --why \"<what you need it for>\"")
}

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

async fn call(port: u16, method: &str, path: &str, token: &str, body: Option<Value>) -> Resp {
    let sep = if path.contains('?') { '&' } else { '?' };
    let pq = format!("{path}{sep}token={}", enc(token));
    let method = method.to_string();
    let body = body.map(|b| b.to_string());
    tokio::task::spawn_blocking(move || tcp_blocking(port, &method, &pq, body)).await.unwrap()
}

async fn post(port: u16, path: &str, token: &str, body: Value) -> Resp {
    call(port, "POST", path, token, Some(body)).await
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

fn login(user: &str, role: Role) -> String {
    k2_core::connect_users::add_user(user, "password-for-pcan-int-9").expect("add_user");
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
    let id = format!("node-pcan-{name}");
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

/// A permission request's stored state (`pr_…`).
fn state_of(id: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT state FROM permission_requests WHERE id = ?1", [id], |r| r.get(0))
        .unwrap_or_else(|e| panic!("request {id}: {e}"))
}

/// Every permission request this workspace ever made.
fn asks(ws: &Ws) -> i64 {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT COUNT(*) FROM permission_requests WHERE workspace_id = ?1", [&ws.id], |r| r.get(0))
        .expect("count permission requests")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_agent_asks_to_add_nodes_to_its_own_list_and_only_those_are_added() {
    for (k, _) in std::env::vars() {
        if k.starts_with("K2_") || k.starts_with("K2SO_") {
            std::env::remove_var(k);
        }
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let home = k2_core::test_env::short_tmp(format!("k2pcan{:x}", nanos % 0xffff_ffff));
    std::fs::create_dir_all(&home).unwrap();
    let home = dunce::canonicalize(&home).unwrap();
    let _home = Home { prev: std::env::var_os("HOME"), home: home.clone() };
    std::env::set_var("HOME", &home);
    std::env::set_var("K2_COMPUTE", "1");

    let d = test_harness::start(OWNER).await;
    let port = d.port;
    let admin = login("pcanadmin", Role::Admin);
    let member = login("pcanmember", Role::Member);
    let a = workspace(&home, "pcan-engine");
    let b = workspace(&home, "pcan-cores");
    let la = node("pcan-la", &[&a, &b]);
    let lb = node("pcan-lb", &[&a]);
    let lc = node("pcan-lc", &[&a]);
    let _ld = node("pcan-ld", &[&b]);
    let pa = passport(&a, "pane-pcan-a");

    // A has a list of one; B a list of its own.
    let r = post(port, SET, OWNER, json!({ "workspace": a.id, "nodes": "pcan-la" })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(port, SET, OWNER, json!({ "workspace": b.id, "nodes": "pcan-la" })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(list_of(&a), sorted(&[&la]));

    // Cards on: the server's switch and A's own.
    let r = post(port, "/cli/settings/update", OWNER, json!({ "permissionCards": true })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(port, "/cli/agent-access/set", OWNER, json!({ "toggle": "permission_cards", "workspace": a.id, "value": true })).await;
    assert_eq!(r.status, 200, "{}", r.body);

    // ── the refusals ────────────────────────────────────────────────────
    // `--add N` and the comma form (what the CLI sends): refused, and the
    // refusal ends with the exact ask for those nodes.
    for (body, nodes) in [
        (json!({ "add": "pcan-lb" }), "pcan-lb"),
        (json!({ "add": "pcan-lb,pcan-lc" }), "pcan-lb,pcan-lc"),
        (json!({ "nodes": "pcan-la,pcan-lc" }), "pcan-lc"),
    ] {
        let r = post(port, SET, &pa, body.clone()).await;
        assert_eq!((r.status, r.code()), (403, "allow_nodes_needs_person".to_string()), "{body}: {}", r.body);
        let v = r.json();
        assert_eq!(v["error"]["nodes"], nodes, "{body}: {v}");
        assert!(v["error"]["hint"].as_str().unwrap_or("").contains(&format!("k2 permission request compute.allow-nodes nodes={nodes}")), "{body}: {v}");
        assert_eq!(
            v["permission"],
            json!({ "action": "compute.allow-nodes", "params": { "nodes": nodes }, "approvers": "owner or admin", "command": ask_line(nodes), "tail": ask_tail(nodes) }),
            "{body}: {v}"
        );
    }
    assert_eq!(list_of(&a), sorted(&[&la]), "a refusal changes nothing");

    // Not granted to A: refused before any card, and not requestable.
    let r = post(port, SET, &pa, json!({ "add": "pcan-ld" })).await;
    assert_eq!((r.status, r.code()), (403, "allow_nodes_not_granted".to_string()), "{}", r.body);
    let v = r.json();
    assert!(v["error"]["hint"].as_str().unwrap_or("").contains("pcan-ld is not granted to workspace pcan-engine"), "{v}");
    assert_eq!(v["permission"]["requestable"], false, "{v}");
    assert!(v["permission"].get("command").is_none(), "{v}");
    // Unknown: a plain not-found, no card.
    let r = post(port, SET, &pa, json!({ "add": "no-such-node" })).await;
    assert_eq!((r.status, r.code()), (404, "node_not_found".to_string()), "{}", r.body);
    assert!(r.json().get("permission").is_none(), "{}", r.body);

    // Another workspace's list: forbidden, not requestable, and it names
    // who can.
    let r = post(port, SET, &pa, json!({ "workspace": b.id, "add": "pcan-lb" })).await;
    assert_eq!((r.status, r.code()), (403, "forbidden".to_string()), "{}", r.body);
    let v = r.json();
    let hint = v["error"]["hint"].as_str().unwrap_or("");
    assert!(hint.contains("never pcan-cores's") && hint.contains("pcan-cores's own agent asks") && hint.contains("--workspace pcan-cores"), "{v}");
    assert_eq!(v["permission"]["requestable"], false, "{v}");
    assert_eq!(list_of(&b), sorted(&[&la]), "the other list is untouched");

    // `--clear` stays a person's: bigger than adding a node, and says so.
    let r = post(port, SET, &pa, json!({ "clear": true })).await;
    assert_eq!((r.status, r.code()), (403, "allow_nodes_needs_person".to_string()), "{}", r.body);
    let v = r.json();
    assert!(v["error"]["hint"].as_str().unwrap_or("").contains("opens every node granted to it at once"), "{v}");
    assert!(v["error"].get("nodes").is_none(), "{v}");
    assert_ne!(v["permission"]["action"], "compute.allow-nodes", "a clear never asks for the add card: {v}");
    assert_eq!(list_of(&a), sorted(&[&la]), "a refused clear changes nothing");

    // ── the request ─────────────────────────────────────────────────────
    // It can never name a workspace (its own or another).
    for ws in [&b.id, &a.id] {
        let r = post(port, "/cli/permission/request", &pa, json!({ "action": "compute.allow-nodes", "params": { "nodes": "pcan-lb", "workspace": ws }, "why": "new nodes" })).await;
        assert_eq!(r.status, 400, "{}", r.body);
    }
    // An ungranted or unknown node: refused before a card.
    for nodes in ["pcan-lb,pcan-ld", "pcan-lb,no-such-node"] {
        let r = post(port, "/cli/permission/request", &pa, json!({ "action": "compute.allow-nodes", "params": { "nodes": nodes }, "why": "new nodes" })).await;
        assert_eq!((r.status, r.code()), (400, "bad_params".to_string()), "{nodes}: {}", r.body);
    }
    assert_eq!(asks(&a), 0, "no request was stored for a refused ask");

    // The printed command, as the CLI sends it: a card for lb and lc.
    let ask = |nodes: &'static str| {
        let body = json!({ "action": "compute.allow-nodes", "params": { "nodes": nodes }, "why": "the new nodes are faster" });
        let pa = pa.clone();
        async move { post(port, "/cli/permission/request", &pa, body).await }
    };
    let r = ask("pcan-lb,pcan-lc").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert_eq!(v["status"], "pending", "{v}");
    let card = v["card"].as_str().unwrap_or_else(|| panic!("no card: {v}")).to_string();
    let denied_id = v["id"].as_str().unwrap_or_else(|| panic!("no id: {v}")).to_string();
    let answer = |who: String, card: String, decision: &'static str| {
        let body = json!({ "addr": a.handle, "id": card, "answer": decision, "reason": "not now" });
        async move { post(port, "/cli/thread/answer", &who, body).await }
    };
    // A Member can't answer it; the agent itself can't either.
    let r = answer(member.clone(), card.clone(), "Allow").await;
    assert_eq!((r.status, r.code()), (403, "action_card_person_only".to_string()), "{}", r.body);
    let r = answer(pa.clone(), card.clone(), "Allow").await;
    assert_eq!(r.status, 403, "the agent answers its own card: {}", r.body);
    assert_eq!(list_of(&a), sorted(&[&la]), "a refused answer applies nothing");

    // Deny: nothing changes.
    let r = answer(admin.clone(), card.clone(), "Deny").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(state_of(&denied_id), "denied", "{}", r.body);
    assert_eq!(list_of(&a), sorted(&[&la]), "deny changes nothing");

    // Allow: exactly lb joins A's list; lc (not on this card), B's list
    // and every grant stay as they were.
    let r = ask("pcan-lb").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let card = r.json()["card"].as_str().unwrap_or_else(|| panic!("no card: {}", r.body)).to_string();
    let allowed_id = r.json()["id"].as_str().unwrap_or_else(|| panic!("no id: {}", r.body)).to_string();
    let r = answer(admin.clone(), card, "Allow").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["action"]["result"]["outcome"], "approved", "{}", r.body);
    assert_eq!(state_of(&allowed_id), "approved", "{}", r.body);
    assert_eq!(list_of(&a), sorted(&[&la, &lb]), "exactly the card's node was added");
    assert_eq!(list_of(&b), sorted(&[&la]), "another workspace's list is untouched");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        assert!(k2_core::compute::store::grant(&conn, &lc, &a.id).is_some(), "grants are untouched");
        assert!(k2_core::compute::store::grant(&conn, &lb, &b.id).is_none(), "no grant is added");
    }

    // The agent's own add of lb is a no-op now; narrowing needs nobody.
    let r = post(port, SET, &pa, json!({ "add": "pcan-lb" })).await;
    assert_eq!((r.status, r.json()["change"].as_str()), (200, Some("same")), "{}", r.body);
    let r = post(port, SET, &pa, json!({ "remove": "pcan-lb" })).await;
    assert_eq!((r.status, r.json()["change"].as_str()), (200, Some("narrow")), "{}", r.body);
    assert_eq!(list_of(&a), sorted(&[&la]));

    drop(d);
}
