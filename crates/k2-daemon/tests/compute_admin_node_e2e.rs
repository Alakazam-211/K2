//! Node admin across both builders' code (prd-k2-node-admin-v1 §16): the
//! REAL dispatcher in-process (temp `$HOME`, loopback, compute on) as the
//! controller, the REAL `k2-node` runtime as the courier, and the node
//! side's root executor in-process (same uid; it writes only temp files).
//! The vault is a temp one with a fake key store, installed into the
//! daemon: no keychain, no Touch ID (the confirm token is minted on that
//! vault, as `/cli/vault/confirm/finish` would after a person confirmed).
//!
//! Covers: the controller pins its approval key on the node (trust on
//! first use); policy show (node-answered read); an agent's narrowing goes
//! through at once; an agent's widening is held, the owner approves with a
//! confirm, and the executor accepts the owner signature and writes the
//! policy; a two-step plan approved once runs on the node; the agent hears
//! back; a passport can't approve.
// pending k2-node Windows port: these tests drive the real k2-node runtime
// in-process, and k2-node is Unix-only until its Windows port (Phase 3).
#![cfg(unix)]

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use k2_core::session::SessionId;
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;
use k2_node::admin::config::{self as acfg, AdminFile};
use k2_node::admin::exec::{ExecPaths, Executor, Platform, SystemPlatform};
use k2_node::node::{Node, NodeOptions};
use k2_node::paths::Layout;
use rusqlite::params;
use serde_json::{json, Value};

static TEST_LOCK: StdMutex<()> = StdMutex::new(());
const OWNER: &str = "owner-token-node-admin-e2e";

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
    let home = PathBuf::from(format!("/tmp/k2nae-{:x}", nanos % 0xffff_ffff));
    std::fs::create_dir_all(&home).unwrap();
    let home = std::fs::canonicalize(&home).unwrap();
    let env = Env { prev_home: std::env::var_os("HOME"), home: home.clone(), _guard: guard };
    std::env::set_var("HOME", &home);
    env
}

struct Resp {
    status: u16,
    body: String,
}

fn req_blocking(port: u16, method: &str, pq: &str, body: Option<String>) -> Resp {
    let mut s = StdTcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    let raw = match body {
        Some(b) => format!(
            "{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!("{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"),
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

async fn req(port: u16, method: &str, pq: String, body: Option<Value>) -> Resp {
    let m = method.to_string();
    tokio::task::spawn_blocking(move || req_blocking(port, &m, &pq, body.map(|b| b.to_string()))).await.unwrap()
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

async fn post(port: u16, path: &str, token: &str, body: Value) -> Resp {
    req(port, "POST", format!("{path}?token={}", enc(token)), Some(body)).await
}

async fn get(port: u16, path: &str, token: &str, query: &str) -> Resp {
    req(port, "GET", format!("{path}?token={}&{query}", enc(token)), None).await
}

fn j(r: &Resp) -> Value {
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("JSON ({e}): {:?}", r.body))
}

async fn wait_for(what: &str, secs: u64, mut f: impl FnMut() -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send>>) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if f().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for {what}");
}

fn seed_ws(handle: &str) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)", params![id, handle, format!("/tmp/k2nae-ws-{id}")])
        .unwrap();
    id
}

fn passport(ws: &str) -> String {
    let sid = SessionId::new();
    k2_daemon::session_token::mint_session_token(
        &sid,
        &sid.to_string(),
        HookPrincipal { workspace_uuid: ws.to_string(), agent_address: "agent".into() },
        CredMode::ApiKey,
        Provider::Anthropic,
    )
}

/// A temp vault with a fake key store, installed as the daemon's.
fn install_vault(home: &std::path::Path) -> Arc<k2_vault::LocalVault> {
    let ks: Arc<dyn k2_vault::KeyStore> = Arc::new(k2_vault::keystore::fake::FakeKeyStore::new());
    let v = Arc::new(
        k2_vault::LocalVault::open(home.join("test-vault"), ks, k2_vault::OpenOptions { kdf: k2_vault::confirm::KdfCost::Floor }).expect("open test vault"),
    );
    k2_daemon::vault::replace(v.clone());
    v
}

fn policy_file(layout: &Layout) -> String {
    std::fs::read_to_string(layout.policy()).unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn propose_approve_and_the_real_node_runs_it() {
    let env = setup();
    std::env::set_var("K2_COMPUTE", "1");
    let d = test_harness::start(OWNER).await;
    let port = d.port;
    let vault = install_vault(&env.home);

    let ws = seed_ws("it-ops");
    let agent = passport(&ws);
    let r = post(port, "/cli/agent-access/set", OWNER, json!({"workspace": "it-ops", "toggle": "compute_operate", "value": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);

    // Enroll the real node.
    let base = format!("http://127.0.0.1:{port}");
    let r = post(port, "/cli/compute/nodes/enroll-code", OWNER, json!({"name": "m1", "url": base})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let es = k2_node_proto::pairing::EnrollString::parse(j(&r)["enroll"].as_str().unwrap()).expect("enroll string");
    let dir = env.home.join("node-m1");
    let layout = Layout::new(dir.join("home"), dir.join("config"));
    layout.ensure().unwrap();
    std::fs::create_dir_all(&layout.config).unwrap();
    std::fs::write(layout.policy(), "availability = \"always\"\nforeign_locks = []\nwrite_smoke_lock = \"\"\ndisk_floor_gb = 0\nmax_parallel = 2\n").unwrap();
    let key = k2_node::identity::load_or_create_key(&layout.key()).unwrap();
    let mut sock = k2_node::session::connect(&base, k2_node::session::ENROLL_PATH).await.expect("enroll connect");
    let done = k2_node::enroll::enroll_on(&mut sock, &key, &base, &k2_node::enroll::Credential::Code(es), "m1", &Default::default(), k2_node::util::now())
        .await
        .expect("enroll");
    k2_node::identity::write_pin(&layout.pin(), &done.pin).unwrap();
    let r = post(port, "/cli/compute/nodes/confirm", OWNER, json!({"node": "m1", "sas": done.sas})).await;
    assert_eq!(r.status, 200, "{}", r.body);

    // Root's side at the machine: admin.toml (level full, root's own copy of
    // the controller key) and the executor on a local socket.
    let controller = k2_core::compute::controller_key().unwrap();
    acfg::write(
        &layout.config.join(acfg::ADMIN_FILE),
        &AdminFile {
            remote_admin: "full".into(),
            controller_fp: Some(controller.fingerprint()),
            controller_key: Some(k2_node_proto::crypto::b64(controller.spki_der())),
            ..Default::default()
        },
    )
    .unwrap();
    let admin_sock = PathBuf::from(format!("/tmp/k2nae-{}.sock", &k2_node_proto::crypto::random_id().unwrap()[..10]));
    let paths = ExecPaths { config_dir: layout.config.clone(), state_dir: dir.join("admin-state"), home: layout.home.clone(), binary: dir.join("bin/k2-node") };
    k2_node::admin::exec::ensure_state_dir(&paths.state_dir).unwrap();
    let exec = Arc::new(Executor::new(paths, Arc::new(SystemPlatform) as Arc<dyn Platform>));
    let listener = k2_node::admin::server::bind(&admin_sock, None).unwrap();
    let me = unsafe { libc::geteuid() };
    let e2 = exec.clone();
    std::thread::spawn(move || {
        let rule = k2_node::admin::server::PeerRule { allowed_uids: vec![me] };
        let _ = k2_node::admin::server::serve(&e2, &rule, listener, &|| false);
    });

    let mut o = NodeOptions::new(layout.clone());
    o.dev = true;
    o.config_poll = Duration::from_millis(100);
    o.backoff_min = Duration::from_millis(50);
    o.backoff_max = Duration::from_millis(300);
    o.offer_every = Duration::from_secs(1);
    o.keep_awake = false;
    o.use_cgroups = false;
    o.admin_socket = admin_sock.clone();
    o.facts = Some(k2_node::config::MachineFacts { cores: 8, mem_bytes: 32 << 30, disk_total_bytes: 1 << 40, has_battery: false, is_macos: cfg!(target_os = "macos") });
    let node = Node::open(o).await.expect("node open");
    node.spawn_background();
    let runner = tokio::spawn(k2_node::session::run_forever(node.clone()));

    // Online with the admin feature; the controller pins its approval key.
    wait_for("admin offer", 20, || {
        Box::pin(async move {
            let r = get(port, "/cli/compute/nodes/health", OWNER, "node=m1").await;
            r.status == 200 && j(&r)["features"].as_array().is_some_and(|f| f.iter().any(|x| x == "admin"))
        })
    })
    .await;
    let admin_toml = layout.config.join(acfg::ADMIN_FILE);
    wait_for("approval key pinned on the node", 20, || {
        let p = admin_toml.clone();
        Box::pin(async move { std::fs::read_to_string(&p).unwrap_or_default().contains("ed25519:") })
    })
    .await;
    let k = get(port, "/cli/compute/admin/approval-key", OWNER, "").await;
    assert!(std::fs::read_to_string(&admin_toml).unwrap().contains(j(&k)["keyId"].as_str().unwrap()), "the node pinned THIS controller's approval key");

    // Read: the node answers policy.show itself.
    let r = get(port, "/cli/compute/nodes/policy", &agent, "node=m1").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(j(&r)["policy"]["max_parallel"], 2, "{}", r.body);

    // Narrowing: sent at once, applied by the executor.
    let r = post(port, "/cli/compute/nodes/policy/set", &agent, json!({"node": "m1", "set": {"max_parallel": 1}})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(j(&r)["note"], "narrowing applied on the node", "{}", r.body);
    assert!(policy_file(&layout).contains("max_parallel = 1"), "{}", policy_file(&layout));

    // Widening: held. A passport can't approve it.
    let r = post(port, "/cli/compute/nodes/policy/set", &agent, json!({"node": "m1", "set": {"max_parallel": 6}, "reason": "bigger builds"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(j(&r)["held"], true, "{}", r.body);
    let id = j(&r)["request"]["id"].as_str().unwrap().to_string();
    let tok = vault.confirm.mint("node-admin", Some(&id), "owner", "local-auth");
    let r = post(port, "/cli/compute/admin/approve", &agent, json!({"request": id, "confirmToken": tok})).await;
    assert_eq!((r.status, j(&r)["error"]["code"].as_str()), (403, Some("owner_only")), "{}", r.body);
    assert!(policy_file(&layout).contains("max_parallel = 1"), "nothing ran");

    // The owner approves with a fresh confirm for this request.
    let tok = vault.confirm.mint("node-admin", Some(&id), "owner", "local-auth");
    let r = post(port, "/cli/compute/admin/approve", OWNER, json!({"request": id, "confirmToken": tok})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    wait_for("the approved widening to run", 20, || {
        let id = id.clone();
        Box::pin(async move {
            let r = get(port, "/cli/compute/admin/requests", OWNER, "state=all").await;
            j(&r)["requests"].as_array().unwrap().iter().any(|x| x["id"] == id.as_str() && x["state"] == "done")
        })
    })
    .await;
    assert!(policy_file(&layout).contains("max_parallel = 6"), "{}", policy_file(&layout));

    // A plan, approved once, runs step by step on the node.
    let r = post(
        port,
        "/cli/compute/admin/propose",
        &agent,
        json!({"node": "m1", "kind": "plan", "steps": [
            {"verb": "policy.set", "args": {"max_parallel": 8}},
            {"verb": "control.pause", "args": {}}
        ], "reason": "make room, then hold"}),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    let plan_id = j(&r)["request"]["id"].as_str().unwrap().to_string();
    let tok = vault.confirm.mint("node-admin", Some(&plan_id), "owner", "local-auth");
    let r = post(port, "/cli/compute/admin/approve", OWNER, json!({"request": plan_id, "confirmToken": tok})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    wait_for("the plan to finish", 20, || {
        let id = plan_id.clone();
        Box::pin(async move {
            let r = get(port, "/cli/compute/admin/requests", OWNER, "state=all").await;
            j(&r)["requests"].as_array().unwrap().iter().any(|x| x["id"] == id.as_str() && x["state"] == "done")
        })
    })
    .await;
    assert!(policy_file(&layout).contains("max_parallel = 8"), "{}", policy_file(&layout));
    wait_for("the node to pause", 20, || {
        Box::pin(async move {
            let r = get(port, "/cli/compute/nodes", OWNER, "").await;
            j(&r)["nodes"].as_array().unwrap().iter().any(|n| n["name"] == "m1" && n["offer"]["control"] == "paused")
        })
    })
    .await;

    // The executor audited every root action; the controller did too.
    let lines = exec.audit().verify().unwrap();
    assert!(lines.len() >= 4, "{}", lines.len());
    let r = get(port, "/cli/compute/events", OWNER, "limit=200").await;
    let kinds: Vec<String> = j(&r)["events"].as_array().unwrap().iter().filter_map(|e| e["kind"].as_str().map(str::to_string)).collect();
    for k in ["admin_keys_sent", "admin_proposed", "admin_approved", "admin_sent", "admin_result"] {
        assert!(kinds.iter().any(|x| x == k), "missing {k}: {kinds:?}");
    }

    runner.abort();
    let _ = std::fs::remove_file(&admin_sock);
}
