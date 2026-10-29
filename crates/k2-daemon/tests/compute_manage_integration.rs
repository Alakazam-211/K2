//! "Allow managing compute nodes" (`compute_manage`, migration 0194;
//! Rosson 2026-10-18) over real HTTP: the real dispatcher in-process (temp
//! `$HOME`, loopback listener + the harness's tunnel-ingress listener).
//!
//! - Switch off: every managed verb answers an agent passport `owner_only`,
//!   byte-for-byte the same refusal as `node remove` (always the owner's).
//! - Only the owner sets the switch: an agent passport and an Admin login
//!   are refused, and nothing changes.
//! - Switch on: the agent runs node add (enroll code), confirm, rename,
//!   pause/drain/resume, route add/rm, grant/revoke for another workspace
//!   and retire; each writes an Access Audit line naming the workspace,
//!   the session and the act; add and confirm drop an owner inbox note.
//! - Still the owner's with the switch on: node remove, the preview flag
//!   (`/cli/settings/update {computePreview}`) and the switch itself.
//! - The same passport from the Connect tunnel, or through a loopback
//!   proxy (X-Forwarded-For), never manages.
//!
//! No vault, keychain, real node or `~/.k2` is touched (temp `$HOME`).

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::path::PathBuf;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_core::session::SessionId;
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;
use rusqlite::params;
use serde_json::{json, Value};

static TEST_LOCK: StdMutex<()> = StdMutex::new(());
const OWNER: &str = "owner-token-compute-manage-v1";

struct Env {
    home: PathBuf,
    prev_home: Option<std::ffi::OsString>,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl Drop for Env {
    fn drop(&mut self) {
        match self.prev_home.take() {
            Some(p) => std::env::set_var("HOME", p),
            None => std::env::remove_var("HOME"),
        }
        std::env::remove_var("K2_COMPUTE");
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn setup() -> Env {
    let guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let home = k2_core::test_env::short_tmp(format!("k2cm-{:x}", nanos % 0xffff_ffff));
    std::fs::create_dir_all(&home).unwrap();
    let home = dunce::canonicalize(&home).unwrap();
    let env = Env { prev_home: std::env::var_os("HOME"), home: home.clone(), _guard: guard };
    std::env::set_var("HOME", &home);
    env
}

// ── HTTP (blocking; called through spawn_blocking) ───────────────────

struct Resp {
    status: u16,
    body: String,
}

fn req_blocking(port: u16, method: &str, pq: &str, body: Option<String>, extra_header: Option<&str>) -> Resp {
    let mut s = StdTcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let extra = extra_header.map(|h| format!("{h}\r\n")).unwrap_or_default();
    let raw = match body {
        Some(b) => format!(
            "{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n{extra}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!("{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n{extra}\r\n"),
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
    let text = String::from_utf8_lossy(&out).to_string();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|x| x.parse().ok())
        .unwrap_or_else(|| panic!("no status: {text:?}"));
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    Resp { status, body }
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

async fn post_via(port: u16, path: &str, token: &str, body: Value, extra_header: Option<&'static str>) -> Resp {
    let pq = format!("{path}?token={}", enc(token));
    tokio::task::spawn_blocking(move || req_blocking(port, "POST", &pq, Some(body.to_string()), extra_header)).await.unwrap()
}

async fn post(port: u16, path: &str, token: &str, body: Value) -> Resp {
    post_via(port, path, token, body, None).await
}

async fn get(port: u16, path: &str, token: &str, query: &str) -> Resp {
    let pq = format!("{path}?token={}&{query}", enc(token));
    tokio::task::spawn_blocking(move || req_blocking(port, "GET", &pq, None, None)).await.unwrap()
}

fn j(r: &Resp) -> Value {
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("JSON ({e}): {:?}", r.body))
}

fn code(r: &Resp) -> String {
    j(r)["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {}", r.body)).to_string()
}

// ── fixtures ─────────────────────────────────────────────────────────

struct Ws {
    id: String,
    path: PathBuf,
}

fn seed_ws(env: &Env, handle: &str) -> Ws {
    let path = env.home.join(format!("ws-{handle}"));
    std::fs::create_dir_all(&path).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)", params![id, handle, path.to_string_lossy()])
        .unwrap();
    Ws { id, path }
}

fn passport(ws: &Ws) -> (String, String) {
    let sid = SessionId::new();
    let tok = k2_daemon::session_token::mint_session_token(
        &sid,
        &sid.to_string(),
        HookPrincipal { workspace_uuid: ws.id.clone(), agent_address: "agent".into() },
        CredMode::ApiKey,
        Provider::Anthropic,
    );
    (tok, sid.to_string())
}

fn pending_node(id: &str, name: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::compute::store::insert_pending_node(&conn, id, name, &format!("{id:0>64}"), "pem", &Default::default(), "owner", "123456", 1)
        .expect("pending node");
}

fn active_node(id: &str, name: &str) {
    pending_node(id, name);
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::compute::store::confirm_node(&conn, id, "owner", 2).expect("confirm");
}

fn node_by_name(name: &str) -> Option<k2_core::compute::store::NodeRow> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::compute::store::find_node(&conn, name).filter(|n| n.state != "revoked")
}

fn node_by_id(id: &str) -> k2_core::compute::store::NodeRow {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::compute::store::node_by_id(&conn, id).unwrap_or_else(|| panic!("no node row {id}"))
}

fn grant_of(ws: &str, node: &str) -> Option<k2_core::compute::store::GrantRow> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::compute::store::grant(&conn, node, ws)
}

/// "Alerts go to" is server-wide and the DB is shared by every test in
/// this binary: each test sets the value it relies on.
fn set_alerts_to(ws: Option<&str>) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::compute::admin::set_setting(&conn, k2_core::compute::admin::ALERTS_TO, ws, "owner", 1).unwrap();
}

fn manage_on(ws: &str) -> bool {
    k2_core::workspace::settings::agents_can_manage_compute(ws)
}

/// This test's Access Audit lines for `user` with a `compute-` event.
fn compute_audit(user: &str) -> Vec<(String, String)> {
    k2_core::auth_audit::tail(500)
        .expect("read the access audit")
        .into_iter()
        .filter(|v| v["user"] == user && v["event"].as_str().is_some_and(|e| e.starts_with("compute-")))
        .map(|v| (v["event"].as_str().unwrap().to_string(), v["outcome"].as_str().unwrap().to_string()))
        .collect()
}

/// Inbox notes (whole files) in a workspace's inbox root.
fn inbox_titles(ws: &Ws) -> Vec<String> {
    let dir = k2_core::inbox::inbox_root(&ws.path);
    let Ok(rd) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("md") {
            continue;
        }
        let text = std::fs::read_to_string(&p).unwrap();
        out.push(text);
    }
    out
}

const MANAGED: &[&str] = &[
    "/cli/compute/nodes/enroll-code",
    "/cli/compute/nodes/confirm",
    "/cli/compute/nodes/rename",
    "/cli/compute/nodes/pause",
    "/cli/compute/nodes/retire",
    "/cli/compute/nodes/routes",
    "/cli/compute/grants/set",
    "/cli/compute/grants/revoke",
];

fn managed_body(path: &str, web: &str) -> Value {
    match path {
        "/cli/compute/nodes/enroll-code" => json!({"name": "off-node"}),
        "/cli/compute/nodes/confirm" => json!({"node": "cm-pend", "sas": "123456"}),
        "/cli/compute/nodes/rename" => json!({"node": "cm-act", "name": "cm-renamed"}),
        "/cli/compute/nodes/pause" => json!({"node": "cm-act", "action": "drain"}),
        "/cli/compute/nodes/retire" => json!({"node": "cm-act"}),
        "/cli/compute/nodes/routes" => json!({"node": "cm-act", "add": "https://route.example.test/k2"}),
        "/cli/compute/grants/set" => json!({"node": "cm-act", "workspace": web}),
        "/cli/compute/grants/revoke" => json!({"node": "cm-act", "workspace": web}),
        other => panic!("no body for {other}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn compute_manage_switch_end_to_end() {
    let env = setup();
    std::env::set_var("K2_COMPUTE", "1");
    let d = test_harness::start(OWNER).await;
    let port = d.port;
    let ops = seed_ws(&env, "cm-ops");
    let web = seed_ws(&env, "cm-web");
    let (agent, sid) = passport(&ops);
    set_alerts_to(None);
    pending_node("cmpend1", "cm-pend");
    active_node("cmact1", "cm-act");

    // ── Switch off (the default): owner_only, byte-for-byte as `remove`.
    assert!(!manage_on(&ops.id), "0194 default is OFF");
    let remove = post(port, "/cli/compute/nodes/remove", &agent, json!({"node": "cm-act"})).await;
    assert_eq!(remove.status, 403, "{}", remove.body);
    assert_eq!(code(&remove), "owner_only");
    for p in MANAGED {
        let r = post(port, p, &agent, managed_body(p, "cm-web")).await;
        assert_eq!(r.status, 403, "{p}: {}", r.body);
        assert_eq!(r.body, remove.body, "{p}: the switch-off refusal is today's owner_only, byte for byte");
    }
    assert_eq!(node_by_id("cmpend1").state, "pending", "nothing changed");
    assert_eq!(node_by_id("cmact1").name, "cm-act");
    assert!(grant_of(&web.id, "cmact1").is_none());
    assert!(compute_audit("agent:cm-ops").is_empty(), "no audit line without the switch");
    // Reads keep the old passport rules: compute_off without "Allow compute nodes".
    let r = get(port, "/cli/compute/nodes", &agent, "").await;
    assert_eq!((r.status, code(&r).as_str()), (403, "compute_off"), "{}", r.body);

    // ── Only the owner sets the switch.
    let r = post(port, "/cli/agent-access/set", &agent, json!({"workspace": ops.id, "toggle": "compute_manage", "value": true})).await;
    assert_eq!(r.status, 403, "an agent never grants itself: {}", r.body);
    let r = post(port, "/cli/agent-access/set", &agent, json!({"workspace": web.id, "toggle": "compute_manage", "value": true})).await;
    assert_eq!(r.status, 403, "nor another workspace: {}", r.body);
    k2_core::connect_users::add_user("cm-admin", "correct-horse-battery-staple-1").expect("add admin");
    k2_core::connect_users::set_role("cm-admin", k2_core::connect_users::Role::Admin).expect("admin role");
    let admin = k2_core::connect_users::create_session("cm-admin");
    let r = post(port, "/cli/agent-access/set", &admin, json!({"workspace": ops.id, "toggle": "compute_manage", "value": true})).await;
    assert_eq!(r.status, 403, "an Admin login can't hand out owner verbs: {}", r.body);
    assert_eq!(code(&r), "owner_only");
    assert!(!manage_on(&ops.id) && !manage_on(&web.id), "refused flips change nothing");
    let r = get(port, "/cli/agent-access/set", OWNER, &format!("workspace={}&toggle=compute_manage&value=true", ops.id)).await;
    assert_eq!(r.status, 405, "{}", r.body);
    let r = post(port, "/cli/agent-access/set", OWNER, json!({"workspace": ops.id, "toggle": "compute_manage", "value": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(j(&r)["agentsCanManageCompute"], true, "{}", r.body);
    assert!(manage_on(&ops.id));
    assert!(!manage_on(&web.id), "only the named workspace");

    // ── Switch on: each managed verb works for the agent.
    let r = post(port, "/cli/compute/nodes/enroll-code", &agent, json!({"name": "cm-new", "url": "https://ctl.example.test"})).await;
    assert_eq!(r.status, 200, "node add: {}", r.body);
    assert_eq!(j(&r)["name"], "cm-new");
    assert!(j(&r)["enroll"].as_str().is_some_and(|e| !e.is_empty()), "{}", r.body);
    let r = post(port, "/cli/compute/nodes/confirm", &agent, json!({"node": "cm-pend", "sas": "999999"})).await;
    assert_eq!((r.status, code(&r).as_str()), (409, "sas_mismatch"), "the SAS check still holds: {}", r.body);
    let r = post(port, "/cli/compute/nodes/confirm", &agent, json!({"node": "cm-pend", "sas": "123456"})).await;
    assert_eq!(r.status, 200, "confirm: {}", r.body);
    assert_eq!(node_by_id("cmpend1").state, "active");

    // It sees every node now (pending ones too), without the SAS.
    let r = get(port, "/cli/compute/nodes", &agent, "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let names: Vec<String> = j(&r)["nodes"].as_array().unwrap().iter().map(|n| n["name"].as_str().unwrap().to_string()).collect();
    assert!(names.contains(&"cm-pend".to_string()) && names.contains(&"cm-act".to_string()), "{names:?}");
    assert!(j(&r)["nodes"].as_array().unwrap().iter().all(|n| n.get("sas").is_none()), "never the SAS: {}", r.body);

    let r = post(port, "/cli/compute/nodes/rename", &agent, json!({"node": "cm-act", "name": "cm-act2"})).await;
    assert_eq!(r.status, 200, "rename: {}", r.body);
    assert_eq!(node_by_id("cmact1").name, "cm-act2");
    for (action, want) in [("pause", Some("paused")), ("drain", Some("draining")), ("resume", None)] {
        let r = post(port, "/cli/compute/nodes/pause", &agent, json!({"node": "cm-act2", "action": action})).await;
        assert_eq!(r.status, 200, "{action}: {}", r.body);
        assert_eq!(node_by_id("cmact1").controller_pause.as_deref(), want, "{action}");
    }
    let r = post(port, "/cli/compute/nodes/routes", &agent, json!({"node": "cm-act2", "add": "https://route.example.test/k2"})).await;
    assert_eq!(r.status, 200, "route add: {}", r.body);
    assert_eq!(node_by_id("cmact1").routes, vec!["https://route.example.test/k2".to_string()]);
    let r = post(port, "/cli/compute/nodes/routes", &agent, json!({"node": "cm-act2", "remove": "https://route.example.test/k2"})).await;
    assert_eq!(r.status, 200, "route rm: {}", r.body);
    assert!(node_by_id("cmact1").routes.is_empty());

    // Grant/revoke for ANOTHER workspace.
    let r = post(port, "/cli/compute/grants/set", &agent, json!({"node": "cm-act2", "workspace": "cm-web", "maxParallel": 2})).await;
    assert_eq!(r.status, 200, "grant: {}", r.body);
    assert_eq!(j(&r)["workspace"], "cm-web");
    assert_eq!(grant_of(&web.id, "cmact1").expect("grant for cm-web").max_parallel, 2);
    let r = get(port, "/cli/compute/grants", &agent, "workspace=cm-web").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(j(&r)["grants"].as_array().unwrap().iter().any(|g| g["workspace"] == "cm-web"), "{}", r.body);
    let r = post(port, "/cli/compute/grants/revoke", &agent, json!({"node": "cm-act2", "workspace": "cm-web"})).await;
    assert_eq!(r.status, 200, "revoke: {}", r.body);
    assert_eq!(j(&r)["removed"], true, "{}", r.body);
    assert!(grant_of(&web.id, "cmact1").is_none());

    // ── Still the owner's with the switch on.
    let r = post(port, "/cli/compute/nodes/remove", &agent, json!({"node": "cm-act2"})).await;
    assert_eq!(r.status, 403, "remove: {}", r.body);
    assert_eq!(r.body, remove.body, "remove stays owner_only");
    assert!(node_by_name("cm-act2").is_some(), "not removed");
    let preview_before = j(&get(port, "/cli/settings/get", OWNER, "").await)["computePreview"].clone();
    let r = post(port, "/cli/settings/update", &agent, json!({"computePreview": preview_before.as_bool() != Some(true)})).await;
    assert_eq!(r.status, 403, "preview on/off: {}", r.body);
    assert_eq!(j(&get(port, "/cli/settings/get", OWNER, "").await)["computePreview"], preview_before, "preview unchanged");
    let r = post(port, "/cli/agent-access/set", &agent, json!({"workspace": ops.id, "toggle": "compute_manage", "value": false})).await;
    assert_eq!(r.status, 403, "the switch itself: {}", r.body);
    let r = post(port, "/cli/agent-access/set", &agent, json!({"workspace": web.id, "toggle": "compute_manage", "value": true})).await;
    assert_eq!(r.status, 403, "never for another workspace: {}", r.body);
    assert!(manage_on(&ops.id) && !manage_on(&web.id));
    let r = post(port, "/cli/compute/agents/set", &agent, json!({"workspace": ops.id, "value": true})).await;
    assert_eq!((r.status, code(&r).as_str()), (403, "owner_only"), "nor Allow compute nodes: {}", r.body);

    // ── Remote: the same passport off the loopback listener never manages.
    let r = post_via(port, "/cli/compute/nodes/enroll-code", &agent, json!({"name": "cm-proxied"}), Some("X-Forwarded-For: 203.0.113.9")).await;
    assert_eq!(r.status, 403, "through a loopback proxy: {}", r.body);
    assert_eq!(code(&r), "owner_only");
    // (The tunnel listener may refuse before the compute route: any
    // refusal will do; the state checks below are the proof.)
    let r = post(d.tunnel_port, "/cli/compute/nodes/enroll-code", &agent, json!({"name": "cm-tunnel"})).await;
    assert!((400..500).contains(&r.status), "through the Connect tunnel: {} {}", r.status, r.body);
    let r = post(d.tunnel_port, "/cli/compute/grants/set", &agent, json!({"node": "cm-act2", "workspace": "cm-web"})).await;
    assert!((400..500).contains(&r.status), "through the Connect tunnel: {} {}", r.status, r.body);
    assert!(grant_of(&web.id, "cmact1").is_none(), "nothing granted from off the box");
    let waiting = j(&get(port, "/cli/compute/nodes", OWNER, "").await)["waitingCodes"].clone();
    let codes: Vec<&str> = waiting.as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap()).collect();
    assert!(codes.contains(&"cm-new"), "the local add minted a code: {codes:?}");
    assert!(!codes.contains(&"cm-proxied") && !codes.contains(&"cm-tunnel"), "no code from off the box: {codes:?}");

    // Retire (idle → removed).
    let r = post(port, "/cli/compute/nodes/retire", &agent, json!({"node": "cm-act2"})).await;
    assert_eq!(r.status, 200, "retire: {}", r.body);
    assert_eq!(j(&r)["state"], "revoked");
    assert!(node_by_name("cm-act2").is_none());

    // ── Access Audit: one line per act, naming the workspace and session.
    let lines = compute_audit("agent:cm-ops");
    let events: Vec<&str> = lines.iter().map(|(e, _)| e.as_str()).collect();
    for want in [
        "compute-node-add",
        "compute-node-confirm",
        "compute-node-rename",
        "compute-node-pause",
        "compute-node-drain",
        "compute-node-resume",
        "compute-node-route-add",
        "compute-node-route-rm",
        "compute-grant",
        "compute-revoke",
        "compute-node-retire",
    ] {
        assert_eq!(events.iter().filter(|e| **e == want).count(), 1, "{want} once: {events:?}");
    }
    assert_eq!(lines.len(), 11, "no line for refusals or remote tries: {events:?}");
    for (event, outcome) in &lines {
        assert!(outcome.starts_with("ok "), "{event}: {outcome}");
        assert!(outcome.contains("workspace=cm-ops"), "{event}: {outcome}");
        assert!(outcome.contains(&format!("session={sid}")), "{event}: {outcome}");
    }
    let add = &lines.iter().find(|(e, _)| e == "compute-node-add").unwrap().1;
    assert!(add.contains("node=cm-new"), "{add}");
    let grant = &lines.iter().find(|(e, _)| e == "compute-grant").unwrap().1;
    assert!(grant.contains("node=cm-act2") && grant.contains("for=cm-web"), "{grant}");

    // ── Owner inbox notes for add and confirm (no alerts workspace set:
    // the agent's own workspace), and none for the other acts.
    let notes = inbox_titles(&ops);
    assert_eq!(notes.len(), 2, "{notes:#?}");
    assert!(notes.iter().any(|t| t.contains("Agent cm-ops added compute node cm-new")), "{notes:#?}");
    assert!(notes.iter().any(|t| t.contains("Agent cm-ops confirmed compute node cm-pend")), "{notes:#?}");
    assert!(notes.iter().all(|t| t.contains(&sid)), "each note names the session: {notes:#?}");
    assert!(inbox_titles(&web).is_empty(), "the other workspace gets nothing");

    // Owner turns it off: back to owner_only, byte for byte.
    let r = post(port, "/cli/agent-access/set", OWNER, json!({"workspace": ops.id, "toggle": "compute_manage", "value": false})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(!manage_on(&ops.id));
    let r = post(port, "/cli/compute/nodes/rename", &agent, json!({"node": "cm-pend", "name": "cm-x"})).await;
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(r.body, remove.body);
    assert_eq!(node_by_id("cmpend1").name, "cm-pend");
}

/// With "alerts go to" set, the owner notice goes there, not to the agent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn owner_notice_follows_alerts_to() {
    let env = setup();
    std::env::set_var("K2_COMPUTE", "1");
    let d = test_harness::start(OWNER).await;
    let ops = seed_ws(&env, "cn-ops");
    let it = seed_ws(&env, "cn-it");
    let (agent, _) = passport(&ops);
    set_alerts_to(Some(&it.id));
    let r = post(d.port, "/cli/agent-access/set", OWNER, json!({"workspace": ops.id, "toggle": "compute_manage", "value": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(d.port, "/cli/compute/nodes/enroll-code", &agent, json!({"name": "cn-new"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let notes = inbox_titles(&it);
    assert_eq!(notes.len(), 1, "{notes:#?}");
    assert!(notes[0].contains("Agent cn-ops added compute node cn-new"), "{notes:#?}");
    assert!(inbox_titles(&ops).is_empty(), "the alerts workspace gets it instead");
    set_alerts_to(None);
}
