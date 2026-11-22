//! An agent's own compute grant limits as a permission card (0.46.17,
//! Rosson 2026-11-19: "Is this not a card you can present?").
//!
//! On a real in-process daemon, with "Permission cards (Beta)" on for the
//! agent, an agent passport on the TCP port:
//! - `k2 compute grant <node> --max-time 4h` (its own workspace named, or
//!   none) is refused `owner_only` and ends with `Ask with: k2 permission
//!   request compute.grant max-time=4h node=<node>`; every limit it typed is
//!   on the line; nothing changes;
//! - another workspace's grant, and a grant with `--ci` or `--gpu`, never
//!   teach this card;
//! - the request can't name a workspace, `ci=` or `gpu=`; a node the
//!   workspace holds no grant on (with "Let this agent use compute nodes"
//!   off) and an unknown node are refused before any card;
//! - a Member and the agent can't answer the card; an Admin's Deny changes
//!   nothing; an Admin's Allow sets exactly the asked limits on that
//!   workspace's grant there, keeps its other limits, its CI right and its
//!   per-submitter cap, and touches no other grant; asked again, it is
//!   already done.
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

const OWNER: &str = "owner-token-permission-compute-grant";
const SET: &str = "/cli/compute/grants/set";

fn ask_line(values: &str) -> String {
    format!("k2 permission request compute.grant {values} --why \"<what you need it for>\"")
}

/// The same line after the command's name (the permission object's `tail`).
fn ask_tail(values: &str) -> String {
    format!("permission request compute.grant {values} --why \"<what you need it for>\"")
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
    k2_core::compute::store::set_agents_can_use_compute(&conn, &id, true).expect("Allow compute nodes");
    Ws { id, handle: handle.to_string() }
}

fn set_switch(ws: &Ws, on: bool) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::compute::store::set_agents_can_use_compute(&conn, &ws.id, on).expect("the compute switch");
}

fn login(user: &str, role: Role) -> String {
    k2_core::connect_users::add_user(user, "password-for-pcg-int-9").expect("add_user");
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

/// An active node named `name`, granted to each of `to` with default
/// limits. Returns its id.
fn node(name: &str, to: &[&Ws]) -> String {
    let id = format!("node-pcg-{name}");
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

/// A grant's (max secs, parallel, queued, disk GB, CI, per-submitter cap),
/// or `None` for no grant.
fn grant_of(node: &str, ws: &Ws) -> Option<(i64, i64, i64, i64, bool, i64)> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::compute::store::grant(&conn, node, &ws.id)
        .map(|g| (g.max_job_secs, g.max_parallel, g.max_queued, g.max_disk_gb, g.allow_ci, g.submitter_max_running))
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
async fn an_agent_asks_for_its_own_grant_limits_and_only_those_change() {
    for (k, _) in std::env::vars() {
        if k.starts_with("K2_") || k.starts_with("K2SO_") {
            std::env::remove_var(k);
        }
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let home = k2_core::test_env::short_tmp(format!("k2pcg{:x}", nanos % 0xffff_ffff));
    std::fs::create_dir_all(&home).unwrap();
    let home = dunce::canonicalize(&home).unwrap();
    let _home = Home { prev: std::env::var_os("HOME"), home: home.clone() };
    std::env::set_var("HOME", &home);
    std::env::set_var("K2_COMPUTE", "1");

    let d = test_harness::start(OWNER).await;
    let port = d.port;
    let admin = login("pcgadmin", Role::Admin);
    let member = login("pcgmember", Role::Member);
    let a = workspace(&home, "pcg-builder");
    let b = workspace(&home, "pcg-tester");
    let n1 = node("pcg-n1", &[&a, &b]);
    let n2 = node("pcg-n2", &[&b]);
    let pa = passport(&a, "pane-pcg-a");

    // A's grant on n1, as the owner set it: 2 h, one at a time, 10 waiting,
    // 60 GB, CI allowed, at most 2 running per submitter.
    let r = post(
        port,
        SET,
        OWNER,
        json!({ "node": "pcg-n1", "workspace": a.id, "maxJobSecs": 7200, "maxParallel": 1, "maxQueued": 10, "maxDiskGb": 60, "allowCi": true, "submitterMaxRunning": 2 }),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    let before = Some((7200, 1, 10, 60, true, 2));
    assert_eq!(grant_of(&n1, &a), before);
    let b_before = grant_of(&n1, &b);

    // Cards on: the server's switch and A's own.
    let r = post(port, "/cli/settings/update", OWNER, json!({ "permissionCards": true })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(port, "/cli/agent-access/set", OWNER, json!({ "toggle": "permission_cards", "workspace": a.id, "value": true })).await;
    assert_eq!(r.status, 200, "{}", r.body);

    // ── the refusals ────────────────────────────────────────────────────
    // What the CLI sends for `k2 compute grant pcg-n1 --max-time 4h`, with
    // A named by its handle or its id, or not named at all.
    for body in [
        json!({ "node": "pcg-n1", "workspace": a.handle, "maxJobSecs": 14400 }),
        json!({ "node": "pcg-n1", "workspace": a.id, "maxJobSecs": 14400 }),
        json!({ "node": "pcg-n1", "maxJobSecs": 14400 }),
    ] {
        let r = post(port, SET, &pa, body.clone()).await;
        assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "{body}: {}", r.body);
        assert_eq!(
            r.json()["permission"],
            json!({
                "action": "compute.grant",
                "params": { "node": "pcg-n1", "max-time": "4h" },
                "approvers": "owner or admin",
                "command": ask_line("max-time=4h node=pcg-n1"),
                "tail": ask_tail("max-time=4h node=pcg-n1"),
            }),
            "{body}: {}",
            r.body
        );
    }
    // Every limit it typed is on the line, max-time as a duration.
    let r = post(port, SET, &pa, json!({ "node": "pcg-n1", "workspace": a.handle, "maxJobSecs": 5400, "maxParallel": 2, "maxQueued": 20, "maxDiskGb": 120 })).await;
    assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "{}", r.body);
    assert_eq!(r.json()["permission"]["command"], ask_line("max-disk=120 max-parallel=2 max-queued=20 max-time=1h30m node=pcg-n1"), "{}", r.body);
    // Never this card: another workspace's grant, or one with --ci or --gpu.
    for body in [
        json!({ "node": "pcg-n1", "workspace": b.handle, "maxJobSecs": 14400 }),
        json!({ "node": "pcg-n1", "workspace": a.handle, "maxJobSecs": 14400, "allowCi": true }),
        json!({ "node": "pcg-n1", "maxJobSecs": 14400, "allowGpu": true }),
        json!({ "node": "pcg-n1", "maxJobSecs": 14400, "allowCi": false }),
    ] {
        let r = post(port, SET, &pa, body.clone()).await;
        assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "{body}: {}", r.body);
        assert_ne!(r.json()["permission"]["action"], "compute.grant", "{body}: {}", r.body);
    }
    assert_eq!(grant_of(&n1, &a), before, "a refusal changes nothing");

    // ── the request ─────────────────────────────────────────────────────
    let request = |params: Value| {
        let body = json!({ "action": "compute.grant", "params": params, "why": "the soak runs 3 hours" });
        let pa = pa.clone();
        async move { post(port, "/cli/permission/request", &pa, body).await }
    };
    // It can never name a workspace, CI or GPU.
    for params in [
        json!({ "node": "pcg-n1", "max-time": "4h", "workspace": b.id }),
        json!({ "node": "pcg-n1", "max-time": "4h", "workspace": a.id }),
        json!({ "node": "pcg-n1", "max-time": "4h", "ci": "true" }),
        json!({ "node": "pcg-n1", "max-time": "4h", "gpu": "true" }),
    ] {
        let r = request(params.clone()).await;
        assert_eq!((r.status, r.code()), (400, "bad_params".to_string()), "{params}: {}", r.body);
    }
    // n2 isn't granted to A: with "Let this agent use compute nodes" off,
    // refused before a card (and it names the card to ask first).
    set_switch(&a, false);
    let r = request(json!({ "node": "pcg-n2", "max-time": "4h" })).await;
    assert_eq!((r.status, r.code()), (400, "bad_params".to_string()), "{}", r.body);
    let hint = r.json()["error"]["hint"].as_str().unwrap_or_default().to_string();
    assert!(hint.contains("has no grant on 'pcg-n2'") && hint.contains("compute.use"), "{hint}");
    set_switch(&a, true);
    // Unknown node, no limit, a bad value: refused before a card.
    for params in [
        json!({ "node": "no-such-node", "max-time": "4h" }),
        json!({ "node": "pcg-n1" }),
        json!({ "node": "pcg-n1", "max-time": "4 hours" }),
        json!({ "node": "pcg-n1", "max-parallel": "65" }),
    ] {
        let r = request(params.clone()).await;
        assert_eq!((r.status, r.code()), (400, "bad_params".to_string()), "{params}: {}", r.body);
    }
    assert_eq!(asks(&a), 0, "no request was stored for a refused ask");

    // The printed command, as the CLI sends it: a card. (This one is
    // denied below; a denial blocks the same values for a day, so the one
    // allowed after it asks for 4h.)
    let r = request(json!({ "node": "pcg-n1", "max-time": "3h" })).await;
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
    assert_eq!(grant_of(&n1, &a), before, "a refused answer applies nothing");

    // Deny: nothing changes.
    let r = answer(admin.clone(), card.clone(), "Deny").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(state_of(&denied_id), "denied", "{}", r.body);
    assert_eq!(grant_of(&n1, &a), before, "deny changes nothing");

    // Allow: exactly the asked limit; A's other limits, its CI right and
    // its per-submitter cap stay; B's grant and A's (missing) one on n2 too.
    let r = request(json!({ "node": "pcg-n1", "max-time": "4h" })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let card = r.json()["card"].as_str().unwrap_or_else(|| panic!("no card: {}", r.body)).to_string();
    let allowed_id = r.json()["id"].as_str().unwrap_or_else(|| panic!("no id: {}", r.body)).to_string();
    let r = answer(admin.clone(), card, "Allow").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["action"]["result"]["outcome"], "approved", "{}", r.body);
    assert_eq!(state_of(&allowed_id), "approved", "{}", r.body);
    assert_eq!(grant_of(&n1, &a), Some((14_400, 1, 10, 60, true, 2)), "exactly max-time changed");
    assert_eq!(grant_of(&n1, &b), b_before, "another workspace's grant is untouched");
    assert_eq!(grant_of(&n2, &a), None, "no grant is added elsewhere");

    // Asked again: already done, no card.
    let r = request(json!({ "node": "pcg-n1", "max-time": "4h" })).await;
    assert_eq!((r.status, r.json()["status"].as_str()), (200, Some("already_done")), "{}", r.body);

    // Two limits at once, max-parallel among them: both set, the rest kept.
    let r = request(json!({ "node": "pcg-n1", "max-parallel": "2", "max-queued": "20" })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let card = r.json()["card"].as_str().unwrap_or_else(|| panic!("no card: {}", r.body)).to_string();
    let r = answer(admin.clone(), card, "Allow").await;
    assert_eq!((r.status, r.json()["action"]["result"]["outcome"].as_str()), (200, Some("approved")), "{}", r.body);
    assert_eq!(grant_of(&n1, &a), Some((14_400, 2, 20, 60, true, 2)), "exactly max-parallel and max-queued changed");

    drop(d);
}
