//! A compute node's "pinned jobs only" mode as a permission card (0.46.18,
//! Rosson: an agent presents a one-tap card to turn it on or off).
//!
//! On a real in-process daemon, with "Permission cards (Beta)" on for the
//! agent, an agent passport on the TCP port:
//! - `k2 compute node pinned-only <node> on|off` is refused `owner_only`
//!   (with "Let this agent manage compute nodes" on too) and ends with
//!   `Ask with: k2 permission request compute.node.pinned-only node=<node>
//!   on=<true|false>`, never compute.manage; nothing changes;
//! - the printed request makes a card; a Member and the agent can't answer
//!   it; an Admin's Deny changes nothing; an Admin's Allow flips the node's
//!   `pinnedOnly` as that Admin, with a compute event and an Access Audit
//!   line naming who asked and `via=permission-card`; asked again, it is
//!   already done; the owner's Allow turns it back off.
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

const OWNER: &str = "owner-token-permission-compute-pinned-only";
const ROUTE: &str = "/cli/compute/nodes/pinned-only";

fn ask_line(node: &str, on: &str) -> String {
    format!("k2 permission request compute.node.pinned-only node={node} on={on} --why \"<what you need it for>\"")
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
    Ws { id, handle: handle.to_string() }
}

fn login(user: &str, role: Role) -> String {
    k2_core::connect_users::add_user(user, "password-for-pcp-int-9").expect("add_user");
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

/// An active node named `name`. Returns its id.
fn node(name: &str) -> String {
    let id = format!("node-pcp-{name}");
    let db = k2_core::db::shared();
    let conn = db.lock();
    let labels = BTreeMap::from([("os".to_string(), "linux".to_string()), ("arch".to_string(), "x86_64".to_string())]);
    k2_core::compute::store::insert_pending_node(&conn, &id, name, &k2_core::compute::proto::crypto::sha256_hex(id.as_bytes()), "pem", &labels, "owner", "123456", 5)
        .expect("enroll");
    k2_core::compute::store::confirm_node(&conn, &id, "owner", 6).expect("confirm");
    id
}

fn pinned(node_id: &str) -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::compute::pinned_only::is_on(&conn, node_id)
}

/// The node's `pinned_only` compute events: `(actor, detail)`.
fn events(node_id: &str) -> Vec<(String, Value)> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut st = conn
        .prepare("SELECT actor, detail_json FROM compute_events WHERE kind = 'pinned_only' AND node_id = ?1 ORDER BY id")
        .expect("prepare");
    let rows: Vec<(String, String)> =
        st.query_map([node_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).expect("query").map(|x| x.expect("row")).collect();
    rows.into_iter().map(|(a, d)| (a, serde_json::from_str(&d).expect("detail json"))).collect()
}

/// The card's Access Audit lines.
fn audit() -> Vec<Value> {
    k2_core::auth_audit::read_all().expect("audit").into_iter().filter(|e| e["event"] == "compute-node-pinned-only").collect()
}

/// A permission request's stored state (`pr_…`).
fn state_of(id: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT state FROM permission_requests WHERE id = ?1", [id], |r| r.get(0))
        .unwrap_or_else(|e| panic!("request {id}: {e}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_agent_asks_on_a_card_to_switch_a_nodes_pinned_only_mode() {
    for (k, _) in std::env::vars() {
        if k.starts_with("K2_") || k.starts_with("K2SO_") {
            std::env::remove_var(k);
        }
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let home = k2_core::test_env::short_tmp(format!("k2pcp{:x}", nanos % 0xffff_ffff));
    std::fs::create_dir_all(&home).unwrap();
    let home = dunce::canonicalize(&home).unwrap();
    let _home = Home { prev: std::env::var_os("HOME"), home: home.clone() };
    std::env::set_var("HOME", &home);
    std::env::set_var("K2_COMPUTE", "1");

    let d = test_harness::start(OWNER).await;
    let port = d.port;
    let admin_user = "pcpadmin";
    let admin = login(admin_user, Role::Admin);
    let member = login("pcpmember", Role::Member);
    let a = workspace(&home, "pcp-builder");
    let n1 = node("pcp-n1");
    let pa = passport(&a, "pane-pcp-a");

    // Cards on: the server's switch and A's own. "Let this agent manage
    // compute nodes" on as well: the route refuses a manager too, and its
    // card is no answer.
    let r = post(port, "/cli/settings/update", OWNER, json!({ "permissionCards": true })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    for toggle in ["permission_cards", "compute_manage"] {
        let r = post(port, "/cli/agent-access/set", OWNER, json!({ "toggle": toggle, "workspace": a.id, "value": true })).await;
        assert_eq!(r.status, 200, "{toggle}: {}", r.body);
    }

    // ── the refusals: the agent can't flip it itself ────────────────────
    for (on, word) in [(true, "true"), (false, "false")] {
        let r = post(port, ROUTE, &pa, json!({ "node": "pcp-n1", "on": on })).await;
        assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "{}", r.body);
        assert_eq!(
            r.json()["permission"],
            json!({
                "action": "compute.node.pinned-only",
                "params": { "node": "pcp-n1", "on": word },
                "approvers": "owner or admin",
                "command": ask_line("pcp-n1", word),
            }),
            "{}",
            r.body
        );
    }
    // Another node route an agent can't use is not this card.
    let r = post(port, "/cli/compute/nodes/remove", &pa, json!({ "node": "pcp-n1" })).await;
    assert_eq!(r.status, 403, "{}", r.body);
    assert_ne!(r.json()["permission"]["action"], "compute.node.pinned-only", "{}", r.body);
    assert!(!pinned(&n1), "a refusal changes nothing");
    assert!(events(&n1).is_empty());

    // ── the request ─────────────────────────────────────────────────────
    let request = |on: &'static str| {
        let body = json!({ "action": "compute.node.pinned-only", "params": { "node": "pcp-n1", "on": on }, "why": "the Mac node runs only the sandbox tests" });
        let pa = pa.clone();
        async move { post(port, "/cli/permission/request", &pa, body).await }
    };
    let answer = |who: String, card: String, decision: &'static str| {
        let body = json!({ "addr": a.handle, "id": card, "answer": decision, "reason": "not now" });
        async move { post(port, "/cli/thread/answer", &who, body).await }
    };
    // Off already: nothing to ask.
    let r = request("false").await;
    assert_eq!((r.status, r.json()["status"].as_str()), (200, Some("already_done")), "{}", r.body);

    let r = request("true").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert_eq!(v["status"], "pending", "{v}");
    let card = v["card"].as_str().unwrap_or_else(|| panic!("no card: {v}")).to_string();
    let denied_id = v["id"].as_str().unwrap_or_else(|| panic!("no id: {v}")).to_string();
    // A Member can't answer it; the agent itself can't either.
    let r = answer(member.clone(), card.clone(), "Allow").await;
    assert_eq!((r.status, r.code()), (403, "action_card_person_only".to_string()), "{}", r.body);
    let r = answer(pa.clone(), card.clone(), "Allow").await;
    assert_eq!(r.status, 403, "the agent answers its own card: {}", r.body);
    assert!(!pinned(&n1), "a refused answer applies nothing");

    // Deny: nothing changes.
    let r = answer(admin.clone(), card, "Deny").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(state_of(&denied_id), "denied", "{}", r.body);
    assert!(!pinned(&n1), "deny changes nothing");
    assert!(events(&n1).is_empty() && audit().is_empty(), "deny records no change");

    // Allow (an Admin), asked by another agent (a denial blocks the same
    // values for a day for the agent it was denied to): on, as that Admin,
    // audited with who asked.
    let b = workspace(&home, "pcp-tester");
    let r = post(port, "/cli/agent-access/set", OWNER, json!({ "toggle": "permission_cards", "workspace": b.id, "value": true })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let pb = passport(&b, "pane-pcp-b");
    let body = json!({ "action": "compute.node.pinned-only", "params": { "node": "pcp-n1", "on": "true" }, "why": "the Mac node runs only the sandbox tests" });
    let r = post(port, "/cli/permission/request", &pb, body).await;
    assert_eq!((r.status, r.json()["status"].as_str()), (200, Some("pending")), "{}", r.body);
    let card = r.json()["card"].as_str().unwrap_or_else(|| panic!("no card: {}", r.body)).to_string();
    let allowed_id = r.json()["id"].as_str().unwrap_or_else(|| panic!("no id: {}", r.body)).to_string();
    let body = json!({ "addr": b.handle, "id": card, "answer": "Allow" });
    let r = post(port, "/cli/thread/answer", &admin, body).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["action"]["result"]["outcome"], "approved", "{}", r.body);
    assert_eq!(state_of(&allowed_id), "approved", "{}", r.body);
    assert!(pinned(&n1), "allow turned it on");
    let asked_by = format!("agent:{}", b.handle);
    assert_eq!(events(&n1), vec![(format!("login:{admin_user}"), json!({ "on": true, "askedBy": asked_by, "via": "permission-card" }))]);
    let lines = audit();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0]["user"], format!("login:{admin_user}"), "{lines:?}");
    assert_eq!(lines[0]["outcome"], format!("node=pcp-n1 on=true changed=true askedBy={asked_by} via=permission-card"), "{lines:?}");
    // Asked again: already done.
    let body = json!({ "action": "compute.node.pinned-only", "params": { "node": "pcp-n1", "on": "true" }, "why": "again" });
    let r = post(port, "/cli/permission/request", &pb, body).await;
    assert_eq!((r.status, r.json()["status"].as_str()), (200, Some("already_done")), "{}", r.body);

    // The owner's Allow turns it back off, asked by A.
    let r = request("false").await;
    assert_eq!((r.status, r.json()["status"].as_str()), (200, Some("pending")), "{}", r.body);
    let card = r.json()["card"].as_str().unwrap_or_else(|| panic!("no card: {}", r.body)).to_string();
    let r = answer(OWNER.to_string(), card, "Allow").await;
    assert_eq!((r.status, r.json()["action"]["result"]["outcome"].as_str()), (200, Some("approved")), "{}", r.body);
    assert!(!pinned(&n1), "allow turned it off");
    let last = events(&n1).pop().expect("an event");
    assert_eq!((last.0.as_str(), &last.1), ("owner", &json!({ "on": false, "askedBy": format!("agent:{}", a.handle), "via": "permission-card" })));
    let r = post(port, ROUTE, OWNER, json!({ "node": "pcp-n1", "on": false })).await;
    assert_eq!((r.status, r.json()["pinnedOnly"].as_bool(), r.json()["changed"].as_bool()), (200, Some(false), Some(false)), "{}", r.body);

    drop(d);
}
