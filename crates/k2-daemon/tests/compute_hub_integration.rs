//! The compute hub, hub side (`prd-compute-shared-queue-v1` S1): the real
//! dispatcher in-process (temp `$HOME`, loopback) with the REAL `k2-node`,
//! and two fake tenant servers that only hold a P-256 key, pinned in the
//! federation store the way pairing v2 leaves it.
//!
//! Covers (every check fails loudly):
//! - tenants: add (owner only; `team` → `wall_required`; a v1-paired or
//!   unconfirmed peer refused), grant per node, list;
//! - signed requests over HTTP: no header (401), a replayed request
//!   (`hub_replay`), a stale epoch after suspend (`tenant_epoch_stale`),
//!   suspend (`tenant_suspended`) and revoke (`tenant_revoked`);
//! - a tenant's job runs on the real node (`noSource`), its logs come back,
//!   its log file lives under the tenant's folder (V32);
//! - scoping: the other tenant can't get, read logs of, cancel or list it;
//!   the hub owner sees it in full with the tenant label; a local passport
//!   sees it redacted in `status` (V12);
//! - classes: `gate` only for a tenant whose top class allows it;
//! - tenant `max_queued` → `queue_full`;
//! - revoke stops the tenant's running job (`cancel_requested`) and takes
//!   `compute` off the peer;
//! - the no-hub path: a local passport's run is tenant `local`, class
//!   `interactive`, exactly as before.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};

use k2_core::session::SessionId;
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;
use k2_node::node::{Node, NodeOptions};
use k2_node::paths::Layout;
use k2_node_proto::crypto::SigningKey;
use rusqlite::params;
use serde_json::{json, Value};

static TEST_LOCK: StdMutex<()> = StdMutex::new(());
const OWNER: &str = "owner-token-compute-hub";

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
        std::env::remove_var("K2_FEDERATION");
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn setup() -> Env {
    let guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let home = PathBuf::from(format!("/tmp/k2hub-{:x}", nanos % 0xffff_ffff));
    std::fs::create_dir_all(&home).unwrap();
    let home = std::fs::canonicalize(&home).unwrap();
    let env = Env { prev_home: std::env::var_os("HOME"), home: home.clone(), _guard: guard };
    std::env::set_var("HOME", &home);
    std::env::set_var("K2_COMPUTE", "1");
    std::env::set_var("K2_FEDERATION", "1");
    env
}

/// The test binary shares one database across its tests: start each with
/// no hub configured on this server.
fn no_hub() {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("UPDATE compute_hubs SET state = 'off'", []).unwrap();
}

// ── HTTP ─────────────────────────────────────────────────────────────

struct Resp {
    status: u16,
    body: String,
}

fn raw_blocking(port: u16, raw: Vec<u8>) -> Resp {
    let mut s = StdTcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    // The blob route answers a refused upload without reading its body and
    // closes: a writer still sending sees a broken pipe, then reads the answer.
    match s.write_all(&raw) {
        Ok(()) => {}
        Err(e) if matches!(e.kind(), std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset) => {}
        Err(e) => panic!("write: {e}"),
    }
    let mut out = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match s.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => out.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => break,
            Err(e) => panic!("read: {e}"),
        }
    }
    let text = String::from_utf8_lossy(&out).to_string();
    let status = text.lines().next().and_then(|l| l.split_whitespace().nth(1)).and_then(|x| x.parse().ok()).unwrap_or_else(|| panic!("no status: {text:?}"));
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    Resp { status, body }
}

async fn raw(port: u16, bytes: Vec<u8>) -> Resp {
    tokio::task::spawn_blocking(move || raw_blocking(port, bytes)).await.unwrap()
}

fn request_bytes(method: &str, pq: &str, headers: &[(String, String)], body: &[u8]) -> Vec<u8> {
    let mut h = format!("{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n", body.len());
    for (k, v) in headers {
        h.push_str(&format!("{k}: {v}\r\n"));
    }
    h.push_str("\r\n");
    let mut out = h.into_bytes();
    out.extend_from_slice(body);
    out
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

async fn post(port: u16, path: &str, token: &str, body: Value) -> Resp {
    raw(port, request_bytes("POST", &format!("{path}?token={}", enc(token)), &[], body.to_string().as_bytes())).await
}

async fn get(port: u16, path: &str, token: &str, query: &str) -> Resp {
    let pq = format!("{path}?token={}&{query}", enc(token));
    raw(port, format!("GET {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").into_bytes()).await
}

fn j(r: &Resp) -> Value {
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("JSON ({e}): {:?}", r.body))
}

fn code(r: &Resp) -> String {
    j(r)["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {}", r.body)).to_string()
}

/// A fake tenant server: its key and the epoch it last heard.
struct Tenant {
    key: SigningKey,
    epoch: i64,
}

impl Tenant {
    fn signed(&self, path: &str, body: &Value) -> Vec<u8> {
        let bytes = body.to_string().into_bytes();
        let sha = k2_node_proto::crypto::sha256_hex(&bytes);
        let now = chrono::Utc::now().timestamp();
        let a = k2_core::compute::hub::sign_request(&self.key, "POST", path, "", &sha, self.epoch, now).unwrap();
        request_bytes("POST", path, &[(k2_core::compute::hub::HEADER.to_string(), a.to_header())], &bytes)
    }
    async fn call(&self, port: u16, path: &str, body: Value) -> Resp {
        raw(port, self.signed(path, &body)).await
    }
}

// ── fixtures ─────────────────────────────────────────────────────────

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main"])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

struct Ws {
    id: String,
}

fn seed_ws(env: &Env, handle: &str) -> Ws {
    let path = env.home.join(format!("ws-{handle}"));
    std::fs::create_dir_all(&path).unwrap();
    git(&path, &["init", "-q"]);
    std::fs::write(path.join("a.txt"), format!("hello from {handle}\n")).unwrap();
    git(&path, &["add", "."]);
    git(&path, &["commit", "-q", "-m", "one"]);
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)", params![id, handle, path.to_string_lossy()]).unwrap();
    Ws { id }
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

/// Pin `key` as a peer (Trusted unless `pending`, with this pairing version).
fn pin_peer(label: &str, key: &SigningKey, trusted: bool, pairing_version: u32) -> String {
    let mut p = k2_core::federation::FederationPeer::pin(label, label, key.spki_pem()).unwrap();
    p.pairing_version = pairing_version;
    if trusted {
        p.trust = k2_core::federation::PeerTrust::Trusted;
    }
    let fp = p.fingerprint.clone();
    k2_core::federation::PeerStore::transact(|st| -> Result<((), bool), String> {
        st.upsert(p);
        Ok(((), true))
    })
    .unwrap()
    .unwrap();
    fp
}

fn peer_caps(fp: &str) -> Vec<String> {
    let st = k2_core::federation::PeerStore::load().unwrap();
    st.get(fp).map(|p| p.capabilities.iter().cloned().collect()).unwrap_or_default()
}

struct RunningNode {
    node: std::sync::Arc<Node>,
    runner: tokio::task::JoinHandle<()>,
}

fn node_layout(env: &Env, tag: &str) -> Layout {
    let dir = env.home.join(format!("node-{tag}"));
    let layout = Layout::new(dir.join("home"), dir.join("config"));
    layout.ensure().unwrap();
    std::fs::create_dir_all(&layout.config).unwrap();
    std::fs::write(
        layout.policy(),
        "availability = \"always\"\nforeign_locks = []\nwrite_smoke_lock = \"\"\ndisk_floor_gb = 0\nmax_parallel = 4\n",
    )
    .unwrap();
    layout
}

fn node_options(layout: Layout) -> NodeOptions {
    let mut o = NodeOptions::new(layout);
    o.dev = true;
    o.cancel_grace = Duration::from_millis(500);
    o.config_poll = Duration::from_millis(100);
    o.backoff_min = Duration::from_millis(50);
    o.backoff_max = Duration::from_millis(300);
    o.offer_every = Duration::from_secs(1);
    o.keep_awake = false;
    o.use_cgroups = false;
    o.facts = Some(k2_node::config::MachineFacts { cores: 8, mem_bytes: 32 << 30, disk_total_bytes: 1 << 40, has_battery: false, is_macos: cfg!(target_os = "macos") });
    o
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

async fn node_row(port: u16, name: &str) -> Value {
    let r = get(port, "/cli/compute/nodes", OWNER, "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    j(&r)["nodes"].as_array().unwrap().iter().find(|n| n["name"] == name).cloned().unwrap_or(Value::Null)
}

/// Enroll, confirm and start a real node; wait for its first offer.
async fn online_node(env: &Env, port: u16, name: &str) -> RunningNode {
    let base = format!("http://127.0.0.1:{port}");
    let r = post(port, "/cli/compute/nodes/enroll-code", OWNER, json!({"name": name, "url": base})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let es = k2_node_proto::pairing::EnrollString::parse(j(&r)["enroll"].as_str().unwrap()).unwrap();
    let layout = node_layout(env, name);
    let key = k2_node::identity::load_or_create_key(&layout.key()).unwrap();
    let mut sock = k2_node::session::connect(&base, k2_node::session::ENROLL_PATH).await.unwrap();
    let done = k2_node::enroll::enroll_on(&mut sock, &key, &base, &k2_node::enroll::Credential::Code(es), name, &Default::default(), k2_node::util::now())
        .await
        .unwrap();
    k2_node::identity::write_pin(&layout.pin(), &done.pin).unwrap();
    let r = post(port, "/cli/compute/nodes/confirm", OWNER, json!({"node": name, "sas": done.sas})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let node = Node::open(node_options(layout.clone())).await.expect("node open");
    node.spawn_background();
    let runner = tokio::spawn(k2_node::session::run_forever(node.clone()));
    let nm = name.to_string();
    wait_for("node offer", 30, move || {
        let n = nm.clone();
        Box::pin(async move { node_row(port, &n).await["offer"].is_object() })
    })
    .await;
    RunningNode { node, runner }
}

fn tenant_run(node: &str, ws: &str, session: &str, argv: &[&str], client: &str) -> Value {
    json!({"node": node, "argv": argv, "clientJobId": client, "noSource": true, "remoteWs": ws, "remoteWsLabel": format!("{ws}-label"), "remoteSession": session})
}

/// Follow a tenant job's logs through the hub routes until it ends.
async fn follow_tenant(port: u16, t: &Tenant, ws: &str, job: &str) -> (String, Value) {
    let mut cursor = 0u64;
    let mut out = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        assert!(Instant::now() < deadline, "job {job} never ended");
        let r = t.call(port, "/cli/compute/hub/jobs/logs", json!({"job": job, "cursor": cursor, "wait": 5, "remoteWs": ws})).await;
        assert_eq!(r.status, 200, "{}", r.body);
        let v = j(&r);
        for c in v["chunks"].as_array().unwrap() {
            if c["stream"] == "out" {
                out.extend(k2_node_proto::crypto::unb64(c["data"].as_str().unwrap()).unwrap());
            }
        }
        cursor = v["cursor"].as_u64().unwrap();
        if v["end"] == true {
            return (String::from_utf8_lossy(&out).into(), v["job"].clone());
        }
    }
}

// ── the test ─────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn tenants_signed_requests_and_scoping_on_the_hub() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    no_hub();
    let port = d.port;
    let running = online_node(&env, port, "hn1").await;

    let alpha = Tenant { key: SigningKey::generate().unwrap(), epoch: 0 };
    let beta = Tenant { key: SigningKey::generate().unwrap(), epoch: 0 };
    let old = SigningKey::generate().unwrap();
    let unconfirmed = SigningKey::generate().unwrap();
    let alpha_fp = pin_peer("alpha", &alpha.key, true, 2);
    let beta_fp = pin_peer("beta", &beta.key, true, 2);
    pin_peer("oldpair", &old, true, 1);
    pin_peer("pending", &unconfirmed, false, 2);

    // A paired server that isn't a tenant yet is refused.
    let r = alpha.call(port, "/cli/compute/hub/hello", json!({})).await;
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(code(&r), "peer_not_granted", "no compute cap yet");

    // ── tenants: owner only, own only, v2 only ───────────────────────
    let local_ws = seed_ws(&env, "localws");
    let (agent, _) = passport(&local_ws);
    let r = post(port, "/cli/compute/tenants/add", &agent, json!({"peer": "alpha"})).await;
    assert_eq!(code(&r), "owner_only", "an agent never adds a tenant");
    let r = post(port, "/cli/compute/tenants/add", OWNER, json!({"peer": "alpha", "trust": "team"})).await;
    assert_eq!((r.status, code(&r)), (409, "wall_required".to_string()), "team tenants wait for the wall");
    let r = post(port, "/cli/compute/tenants/add", OWNER, json!({"peer": "oldpair"})).await;
    assert_eq!(code(&r), "pairing_v2_required");
    let r = post(port, "/cli/compute/tenants/add", OWNER, json!({"peer": "pending"})).await;
    assert_eq!(code(&r), "peer_not_granted", "an unconfirmed peer can't be a tenant");
    let r = post(port, "/cli/compute/tenants/add", OWNER, json!({"peer": "alpha", "label": "alpha-srv", "weight": 2, "topClass": "gate"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(j(&r)["tenant"]["weight"], 2);
    assert!(peer_caps(&alpha_fp).contains(&"compute".to_string()), "the peer is granted compute");
    let r = post(port, "/cli/compute/tenants/add", OWNER, json!({"peer": beta_fp, "label": "beta-srv"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(port, "/cli/compute/tenants/add", OWNER, json!({"peer": "alpha"})).await;
    assert_eq!(r.status, 409, "one tenant per server: {}", r.body);
    let r = get(port, "/cli/compute/tenants", OWNER, "").await;
    let labels: Vec<String> = j(&r)["tenants"].as_array().unwrap().iter().map(|t| t["label"].as_str().unwrap().to_string()).collect();
    // The test binary shares one database across its tests: look for ours.
    assert_eq!(labels.first().map(String::as_str), Some("local"), "local first: {}", r.body);
    assert!(labels.contains(&"alpha-srv".to_string()) && labels.contains(&"beta-srv".to_string()), "{}", r.body);

    // ── signed requests ──────────────────────────────────────────────
    // No header: 401 with its own code.
    let r = raw(port, request_bytes("POST", "/cli/compute/hub/jobs", &[], b"{}")).await;
    assert_eq!((r.status, code(&r)), (401, "hub_auth_missing".to_string()));
    // hello with epoch 0 learns the epoch.
    let mut alpha = alpha;
    let r = alpha.call(port, "/cli/compute/hub/hello", json!({})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    alpha.epoch = j(&r)["epoch"].as_i64().unwrap();
    assert_eq!(j(&r)["label"], "alpha-srv");
    let mut beta = beta;
    beta.epoch = j(&beta.call(port, "/cli/compute/hub/hello", json!({})).await)["epoch"].as_i64().unwrap();
    // Epoch 0 on anything but hello is stale.
    let zero = Tenant { key: SigningKey::from_pkcs8_pem(&alpha.key.to_pkcs8_pem().unwrap()).unwrap(), epoch: 0 };
    let r = zero.call(port, "/cli/compute/hub/jobs", json!({})).await;
    assert_eq!((r.status, code(&r)), (409, "tenant_epoch_stale".to_string()));
    // A replay of the exact same signed request is refused.
    let once = alpha.signed("/cli/compute/hub/jobs", &json!({}));
    assert_eq!(raw(port, once.clone()).await.status, 200);
    let r = raw(port, once).await;
    assert_eq!((r.status, code(&r)), (401, "hub_replay".to_string()));
    // A body changed after signing fails the signature.
    let mut forged = alpha.signed("/cli/compute/hub/jobs", &json!({"limit": 1}));
    let n = forged.len();
    forged[n - 2] = b'2';
    let r = raw(port, forged).await;
    assert_eq!((r.status, code(&r)), (401, "hub_bad_signature".to_string()));

    // Nodes: none until the owner grants one; then exactly that node.
    let r = alpha.call(port, "/cli/compute/hub/nodes", json!({})).await;
    assert_eq!(j(&r)["nodes"].as_array().unwrap().len(), 0, "{}", r.body);
    let r = alpha.call(port, "/cli/compute/hub/run", tenant_run("hn1", "ws-1", "sa", &["true"], "tenant-client-000001")).await;
    assert_eq!((r.status, code(&r)), (403, "not_granted".to_string()), "{}", r.body);
    let r = post(port, "/cli/compute/tenants/grant", &agent, json!({"tenant": "alpha-srv", "node": "hn1"})).await;
    assert_eq!(code(&r), "owner_only");
    for t in ["alpha-srv", "beta-srv"] {
        let r = post(port, "/cli/compute/tenants/grant", OWNER, json!({"tenant": t, "node": "hn1", "maxParallel": 2})).await;
        assert_eq!(r.status, 200, "{}", r.body);
    }
    let r = alpha.call(port, "/cli/compute/hub/nodes", json!({})).await;
    let nodes = j(&r)["nodes"].as_array().unwrap().clone();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0]["name"], "hn1");
    assert!(nodes[0].get("fingerprint").is_none(), "no node fingerprints for tenants: {}", nodes[0]);

    // ── a tenant job on the real node ────────────────────────────────
    let r = alpha.call(port, "/cli/compute/hub/run", tenant_run("hn1", "ws-1", "sa", &["sh", "-c", "echo hi-from-alpha"], "tenant-client-000001")).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let a_job = j(&r)["job"]["id"].as_str().unwrap().to_string();
    assert_eq!(j(&r)["job"]["tenantId"].as_str().map(|t| t.starts_with("tn-")), Some(true), "{}", r.body);
    assert_eq!(j(&r)["job"]["workspace"], "alpha-srv/ws-1-label");
    assert_eq!(j(&r)["job"]["class"], "interactive");
    let (out, fin) = follow_tenant(port, &alpha, "ws-1", &a_job).await;
    assert_eq!(out, "hi-from-alpha\n");
    assert_eq!(fin["state"], "done", "{fin}");
    assert_eq!(fin["exitCode"], 0);
    // Its log lives under the tenant's folder (V32).
    let tid = fin["tenantId"].as_str().unwrap().to_string();
    let tenant_log = env.home.join(".k2/compute/tenants").join(&tid).join("jobs").join(format!("{a_job}-g1.log"));
    assert!(tenant_log.is_file(), "missing {}", tenant_log.display());
    assert!(!env.home.join(format!(".k2/compute/jobs/{a_job}-g1.log")).exists(), "not in the shared log folder");
    // Idempotent per tenant + workspace + client id; the other tenant may
    // reuse the same client id and workspace id freely.
    let r = alpha.call(port, "/cli/compute/hub/run", tenant_run("hn1", "ws-1", "sa", &["sh", "-c", "echo hi-from-alpha"], "tenant-client-000001")).await;
    assert_eq!((j(&r)["existing"].as_bool(), j(&r)["job"]["id"].as_str()), (Some(true), Some(a_job.as_str())));

    // ── scoping: beta never reaches alpha's job ──────────────────────
    for (path, body) in [
        ("/cli/compute/hub/jobs/get", json!({"job": a_job, "remoteWs": "ws-1"})),
        ("/cli/compute/hub/jobs/logs", json!({"job": a_job, "remoteWs": "ws-1"})),
        ("/cli/compute/hub/jobs/receipt", json!({"job": a_job})),
        ("/cli/compute/hub/jobs/artifact", json!({"job": a_job})),
        ("/cli/compute/hub/jobs/cancel", json!({"job": a_job})),
        ("/cli/compute/hub/jobs/retry", json!({"job": a_job})),
    ] {
        let r = beta.call(port, path, body).await;
        assert_eq!((r.status, code(&r)), (404, "job_not_found".to_string()), "{path}: {}", r.body);
    }
    let r = beta.call(port, "/cli/compute/hub/jobs", json!({})).await;
    assert_eq!(j(&r)["jobs"].as_array().unwrap().len(), 0, "beta lists none of alpha's jobs: {}", r.body);
    let r = beta.call(port, "/cli/compute/hub/changes", json!({"jobs": [{"id": a_job, "state": "queued"}]})).await;
    assert_eq!(j(&r)["unknown"], json!([a_job]), "{}", r.body);
    // Alpha naming another of its own workspaces doesn't see ws-1's job.
    let r = alpha.call(port, "/cli/compute/hub/jobs/get", json!({"job": a_job, "remoteWs": "ws-2"})).await;
    assert_eq!(r.status, 404, "{}", r.body);
    // Beta's job with the same workspace and client ids is its own.
    let r = beta.call(port, "/cli/compute/hub/run", tenant_run("hn1", "ws-1", "sb", &["sh", "-c", "sleep 30"], "tenant-client-000001")).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let b_long = j(&r)["job"]["id"].as_str().unwrap().to_string();
    assert_ne!(b_long, a_job);

    // The hub owner sees every job, with the tenant's label.
    let r = get(port, "/cli/compute/jobs", OWNER, "").await;
    let jobs = j(&r)["jobs"].as_array().unwrap().clone();
    let seen_a = jobs.iter().find(|x| x["id"] == a_job.as_str()).expect("owner sees alpha's job");
    assert_eq!(seen_a["tenant"], "alpha-srv");
    assert_eq!(seen_a["argv"], json!(["sh", "-c", "echo hi-from-alpha"]));

    // A local passport sees beta's running job only redacted (V12).
    let r = post(port, "/cli/agent-access/set", OWNER, json!({"workspace": local_ws.id, "toggle": "compute", "value": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let bl = b_long.clone();
    wait_for("beta's job running", 30, move || {
        let bl = bl.clone();
        Box::pin(async move { j(&get(port, "/cli/compute/jobs/get", OWNER, &format!("job={bl}")).await)["state"] == "running" })
    })
    .await;
    let r = get(port, "/cli/compute/nodes", &agent, "detail=1").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let running_list = j(&r)["nodes"][0]["status"]["running"].as_array().unwrap().clone();
    let entry = running_list.iter().find(|e| e["tenant"] == "beta-srv").unwrap_or_else(|| panic!("beta's job in status: {}", r.body));
    assert!(entry.get("argv").is_none() && entry.get("short").is_none(), "redacted: {entry}");
    assert_eq!(entry["workspace"], Value::Null);
    // And beta sees its own entry in full in its own status-like reads.
    let r = beta.call(port, "/cli/compute/hub/jobs", json!({"remoteWs": "ws-1"})).await;
    assert_eq!(j(&r)["jobs"][0]["argv"], json!(["sh", "-c", "sleep 30"]));

    // ── classes and quotas ───────────────────────────────────────────
    let mut gate = tenant_run("hn1", "ws-1", "sa", &["true"], "tenant-client-gate01");
    gate["class"] = json!("gate");
    let r = beta.call(port, "/cli/compute/hub/run", gate.clone()).await;
    assert_eq!((r.status, code(&r)), (403, "class_not_allowed".to_string()), "beta's top class is interactive");
    let r = alpha.call(port, "/cli/compute/hub/run", gate).await;
    assert_eq!(r.status, 200, "alpha may use gate: {}", r.body);
    assert_eq!(j(&r)["job"]["class"], "gate");
    let r = post(port, "/cli/compute/run", &agent, json!({"node": "hn1", "argv": ["true"], "clientJobId": "local-client-gate01", "noSource": true, "class": "gate"})).await;
    assert_eq!(code(&r), "class_not_allowed", "local's top class is interactive");
    let r = post(port, "/cli/compute/tenants/set", OWNER, json!({"tenant": "beta-srv", "maxQueued": 0})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = beta.call(port, "/cli/compute/hub/run", tenant_run("hn1", "ws-1", "sb", &["true"], "tenant-client-full01")).await;
    assert_eq!((r.status, code(&r)), (409, "queue_full".to_string()), "{}", r.body);
    let r = post(port, "/cli/compute/tenants/set", OWNER, json!({"tenant": "beta-srv", "maxQueued": null})).await;
    assert_eq!(r.status, 200, "{}", r.body);

    // ── the no-hub path is unchanged ─────────────────────────────────
    let r = post(port, "/cli/compute/run", &agent, json!({"node": "hn1", "argv": ["sh", "-c", "echo local"], "clientJobId": "local-client-000001", "noSource": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let lv = j(&r);
    assert_eq!(lv["job"]["tenantId"], "local");
    assert_eq!(lv["job"]["class"], "interactive");
    assert_eq!(lv["job"]["workspace"], "localws");
    assert!(lv["job"].get("tenant").is_none(), "local jobs carry no tenant label");

    // ── suspend, stale epoch, resume ─────────────────────────────────
    let r = post(port, "/cli/compute/tenants/suspend", OWNER, json!({"tenant": "beta-srv"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = beta.call(port, "/cli/compute/hub/jobs", json!({})).await;
    assert_eq!((r.status, code(&r)), (403, "tenant_suspended".to_string()));
    let r = post(port, "/cli/compute/tenants/resume", OWNER, json!({"tenant": "beta-srv"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = beta.call(port, "/cli/compute/hub/jobs", json!({})).await;
    assert_eq!((r.status, code(&r)), (409, "tenant_epoch_stale".to_string()), "suspend bumped the epoch");
    beta.epoch = j(&beta.call(port, "/cli/compute/hub/hello", json!({})).await)["epoch"].as_i64().unwrap();
    assert_eq!(beta.call(port, "/cli/compute/hub/jobs", json!({})).await.status, 200);

    // ── revoke stops the running job and takes compute away ──────────
    let r = post(port, "/cli/compute/tenants/revoke", OWNER, json!({"tenant": "beta-srv"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(j(&r)["stopped"], 1, "{}", r.body);
    assert!(!peer_caps(&beta_fp).contains(&"compute".to_string()), "compute is off the peer");
    let r = beta.call(port, "/cli/compute/hub/jobs", json!({})).await;
    assert_eq!(r.status, 403, "{}", r.body);
    let bl = b_long.clone();
    wait_for("beta's job stopped", 30, move || {
        let bl = bl.clone();
        Box::pin(async move { j(&get(port, "/cli/compute/jobs/get", OWNER, &format!("job={bl}")).await)["state"] == "cancelled" })
    })
    .await;

    running.runner.abort();
    running.node.stop_all(k2_node_proto::frames::JobState::Cancelled, "test_over");
    drop(env);
}

// ── S2: this server as a tenant of a hub ─────────────────────────────

/// Pin this daemon's own federation identity as a peer at `base` (Trusted,
/// pairing v2): the in-process daemon is both the submitter and the hub,
/// over real HTTP on loopback.
fn pin_self(label: &str, base: &str) -> String {
    let kp = k2_core::federation::identity::load_or_create().unwrap();
    let mut p = k2_core::federation::FederationPeer::pin(label, label, kp.public_key_pem()).unwrap().with_base_url(base);
    p.trust = k2_core::federation::PeerTrust::Trusted;
    let fp = p.fingerprint.clone();
    k2_core::federation::PeerStore::transact(|st| -> Result<((), bool), String> {
        st.upsert(p);
        Ok(((), true))
    })
    .unwrap()
    .unwrap();
    fp
}

fn self_tenant() -> Tenant {
    let kp = k2_core::federation::identity::load_or_create().unwrap();
    Tenant { key: SigningKey::from_pkcs8_der(&kp.serialize_der()).unwrap(), epoch: 0 }
}

/// Follow a job's logs through THIS server's routes until it ends.
async fn follow_local(port: u16, token: &str, job: &str) -> (String, Value) {
    let mut cursor = 0u64;
    let mut out = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        assert!(Instant::now() < deadline, "job {job} never ended");
        let r = get(port, "/cli/compute/jobs/logs", token, &format!("job={job}&cursor={cursor}&wait=5")).await;
        assert_eq!(r.status, 200, "{}", r.body);
        let v = j(&r);
        for c in v["chunks"].as_array().unwrap() {
            if c["stream"] == "out" {
                out.extend(k2_node_proto::crypto::unb64(c["data"].as_str().unwrap()).unwrap());
            }
        }
        cursor = v["cursor"].as_u64().unwrap();
        if v["end"] == true {
            return (String::from_utf8_lossy(&out).into(), v["job"].clone());
        }
    }
}

fn blob_request(t: &Tenant, query: &str, declared_sha: &str, body: &[u8], content_length: usize) -> Vec<u8> {
    let now = chrono::Utc::now().timestamp();
    let a = k2_core::compute::hub::sign_request(&t.key, "POST", "/cli/compute/hub/blob", query, declared_sha, t.epoch, now).unwrap();
    let mut h = format!(
        "POST /cli/compute/hub/blob?{query} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Length: {content_length}\r\n{}: {}\r\n\r\n",
        k2_core::compute::hub::HEADER,
        a.to_header()
    )
    .into_bytes();
    h.extend_from_slice(body);
    h
}

fn db_one(sql: &str, p: &[&dyn rusqlite::ToSql]) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(sql, p, |r| r.get::<_, String>(0)).unwrap_or_else(|e| panic!("{sql}: {e}"))
}

fn count(sql: &str) -> i64 {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(sql, [], |r| r.get(0)).unwrap_or_else(|e| panic!("{sql}: {e}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_server_uses_a_hub_code_goes_up_and_completions_come_home() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    no_hub();
    let port = d.port;
    let running = online_node(&env, port, "sn1").await;
    let base = format!("http://127.0.0.1:{port}");
    pin_self("selfhub", &base);

    // The hub side: this server is its own tenant "me" on sn1.
    let r = post(port, "/cli/compute/tenants/add", OWNER, json!({"peer": "selfhub", "label": "me"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let tid = j(&r)["tenant"]["id"].as_str().unwrap().to_string();
    let r = post(port, "/cli/compute/tenants/grant", OWNER, json!({"tenant": "me", "node": "sn1", "maxParallel": 3})).await;
    assert_eq!(r.status, 200, "{}", r.body);

    // The submitter side: an agent with compute on, before any hub.
    let ws = seed_ws(&env, "subws");
    let ws_path = env.home.join("ws-subws");
    let other = seed_ws(&env, "otherws");
    let (agent, agent_sid) = passport(&ws);
    let (other_agent, _) = passport(&other);
    for w in [&ws, &other] {
        let r = post(port, "/cli/agent-access/set", OWNER, json!({"workspace": w.id, "toggle": "compute", "value": true})).await;
        assert_eq!(r.status, 200, "{}", r.body);
    }
    let r = get(port, "/cli/compute/hubs", OWNER, "").await;
    assert_eq!(j(&r)["hub"], Value::Null, "no hub: {}", r.body);

    // "Use compute hub": the owner's, never an agent's.
    let r = post(port, "/cli/compute/hubs/use", &agent, json!({"peer": "selfhub"})).await;
    assert_eq!(code(&r), "owner_only");
    let r = post(port, "/cli/compute/hubs/use", OWNER, json!({"peer": "selfhub"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(j(&r)["tenant"]["label"], "me");
    let r = get(port, "/cli/compute/hubs", OWNER, "").await;
    assert_eq!((j(&r)["reachable"].as_bool(), j(&r)["hub"]["label"].as_str()), (Some(true), Some("selfhub")), "{}", r.body);

    // An agent's nodes are the hub's.
    let r = get(port, "/cli/compute/nodes", &agent, "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(j(&r)["hub"], "selfhub");
    assert_eq!(j(&r)["nodes"][0]["name"], "sn1");

    // ── a run goes up: prepare, bundle, run ──────────────────────────
    let r = post(port, "/cli/compute/run", &agent, json!({"node": "sn1", "argv": ["sh", "-c", "cat a.txt"], "clientJobId": "sub-client-00000001"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = j(&r);
    let job1 = v["job"]["id"].as_str().unwrap().to_string();
    assert_eq!(v["hub"], "selfhub");
    assert_eq!(v["source"]["checkout"], "workspace", "the local source info: {v}");
    assert_eq!(v["job"]["workspace"], "subws", "this server's own handle");
    let (out, fin) = follow_local(port, &agent, &job1).await;
    assert_eq!(out, "hello from subws\n", "the job ran the workspace's commit: {fin}");
    assert_eq!(fin["state"], "done");
    // On the hub: the tenant's job, its workspace key, and the hub-named
    // project key (V28), never the submitter's.
    assert_eq!(db_one("SELECT tenant_id FROM compute_jobs WHERE id = ?1", &[&job1]), tid);
    assert_eq!(db_one("SELECT workspace_id FROM compute_jobs WHERE id = ?1", &[&job1]), format!("t/{tid}/{}", ws.id));
    let plan: Value = serde_json::from_str(&db_one("SELECT plan_json FROM compute_jobs WHERE id = ?1", &[&job1])).unwrap();
    let pk = plan["src"]["project_key"].as_str().or(plan["src"]["projectKey"].as_str()).unwrap_or_else(|| panic!("no project key: {plan}")).to_string();
    assert_eq!(pk, k2_core::compute::hub::hub_project_key(&tid, &ws.id, None));
    assert_ne!(pk, k2_core::compute::sync::project_key(&ws.id, None), "not the submitter's own key");
    let mirror = env.home.join(".k2/compute/tenants").join(&tid).join("repos").join(format!("{pk}.git"));
    assert!(mirror.join("objects").is_dir(), "the tenant's mirror on the hub: {}", mirror.display());
    // The submitter remembers it as a remote job.
    assert_eq!(db_one("SELECT workspace_id FROM compute_remote_jobs WHERE hub_job_id = ?1", &[&job1]), ws.id);

    // ── a second commit + --dirty: only have..commit goes up ─────────
    std::fs::write(ws_path.join("a.txt"), "edited\n").unwrap();
    git(&ws_path, &["commit", "-q", "-am", "two"]);
    std::fs::write(ws_path.join("new.txt"), "untracked\n").unwrap();
    let r = post(port, "/cli/compute/run", &agent, json!({"node": "sn1", "argv": ["sh", "-c", "cat a.txt new.txt"], "clientJobId": "sub-client-00000002", "dirty": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let job2 = j(&r)["job"]["id"].as_str().unwrap().to_string();
    let (out, _) = follow_local(port, &agent, &job2).await;
    assert_eq!(out, "edited\nuntracked\n");
    let ups = format!("SELECT COUNT(*) FROM compute_events WHERE kind = 'tenant_code' AND detail_json LIKE '%{tid}%'");
    assert_eq!(count(&ups), 2, "one bundle per new commit");
    let r = post(port, "/cli/compute/run", &agent, json!({"node": "sn1", "argv": ["true"], "clientJobId": "sub-client-00000003"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(count(&ups), 2, "a commit the hub has goes up as nothing");

    // ── --plan and a split run go through the hub too ────────────────
    let r = post(port, "/cli/compute/run", &agent, json!({"node": "sn1", "argv": ["true"], "plan": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!((j(&r)["plan"].as_bool(), j(&r)["node"].as_str(), j(&r)["hub"].as_str()), (Some(true), Some("sn1"), Some("selfhub")), "{}", r.body);
    let r = post(port, "/cli/compute/run", &agent, json!({"any": true, "shards": 2, "argv": ["sh", "-c", "echo shard {shard}/{shards}"], "clientJobId": "sub-client-shards01"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let parent = j(&r)["job"]["id"].as_str().unwrap().to_string();
    let kids: Vec<String> = j(&r)["shards"].as_array().unwrap().iter().map(|k| k["job"].as_str().unwrap().to_string()).collect();
    assert_eq!(kids.len(), 2, "{}", r.body);
    for (i, k) in kids.iter().enumerate() {
        let (out, fin) = follow_local(port, &agent, k).await;
        assert_eq!(out, format!("shard {}/2\n", i + 1), "a shard's log reads through this server: {fin}");
    }
    let p = parent.clone();
    let a = agent.clone();
    wait_for("split run done", 30, move || {
        let (a, p) = (a.clone(), p.clone());
        Box::pin(async move { j(&get(port, "/cli/compute/jobs/get", &a, &format!("job={p}")).await)["state"] == "done" })
    })
    .await;

    // ── scoping on the submitter ─────────────────────────────────────
    let r = get(port, "/cli/compute/jobs/get", &other_agent, &format!("job={job1}")).await;
    assert_eq!(r.status, 404, "another workspace's agent can't see it: {}", r.body);
    let r = get(port, "/cli/compute/jobs", &agent, "").await;
    let listed: Vec<String> = j(&r)["jobs"].as_array().unwrap().iter().map(|x| x["id"].as_str().unwrap().to_string()).collect();
    assert!(listed.contains(&job1) && listed.contains(&job2), "{}", r.body);
    let r = get(port, "/cli/compute/jobs", &other_agent, "").await;
    assert_eq!(j(&r)["jobs"].as_array().unwrap().len(), 0, "{}", r.body);

    // ── a detached job's completion comes home, once ─────────────────
    k2_daemon::compute_ws::test_sink_register(&agent_sid);
    let r = post(port, "/cli/compute/run", &agent, json!({"node": "sn1", "argv": ["sh", "-c", "exit 1"], "clientJobId": "sub-client-00000004", "detach": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let det = j(&r)["job"]["id"].as_str().unwrap().to_string();
    let sid = agent_sid.clone();
    wait_for("completion message", 60, move || {
        let s = sid.clone();
        Box::pin(async move { !k2_daemon::compute_ws::test_sink_peek(&s).is_empty() })
    })
    .await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let msgs = k2_daemon::compute_ws::test_sink_take(&agent_sid);
    // The attached `true` run nobody followed gets its one message too
    // (the same rule as this server's own jobs); the detached one: once.
    let mine: Vec<&String> = msgs.iter().filter(|m| m.contains(&det[..8])).collect();
    assert_eq!(mine.len(), 1, "delivered once, locally: {msgs:?}");
    assert!(mine[0].starts_with(&format!("[compute] job {} on sn1: exit 1", &det[..8])), "{msgs:?}");
    let mut ids: Vec<&str> = msgs.iter().map(|m| m.split_whitespace().nth(2).unwrap_or("")).collect();
    ids.sort_unstable();
    let n = ids.len();
    ids.dedup();
    assert_eq!(ids.len(), n, "no job's message twice: {msgs:?}");
    assert_eq!(count(&format!("SELECT COUNT(*) FROM compute_remote_jobs WHERE hub_job_id = '{det}' AND notified_at IS NOT NULL")), 1);

    // ── cancel goes to the hub ───────────────────────────────────────
    let r = post(port, "/cli/compute/run", &agent, json!({"node": "sn1", "argv": ["sh", "-c", "echo started; sleep 30"], "clientJobId": "sub-client-00000005"})).await;
    let long = j(&r)["job"]["id"].as_str().unwrap().to_string();
    let (a, l) = (agent.clone(), long.clone());
    wait_for("long job running", 30, move || {
        let (a, l) = (a.clone(), l.clone());
        Box::pin(async move { j(&get(port, "/cli/compute/jobs/get", &a, &format!("job={l}")).await)["state"] == "running" })
    })
    .await;
    let r = post(port, "/cli/compute/jobs/cancel", &other_agent, json!({"job": long})).await;
    assert_eq!(r.status, 404, "not another workspace's to cancel: {}", r.body);
    let r = post(port, "/cli/compute/jobs/cancel", &agent, json!({"job": &long[..8]})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let (a, l) = (agent.clone(), long.clone());
    wait_for("long job cancelled", 30, move || {
        let (a, l) = (a.clone(), l.clone());
        Box::pin(async move { j(&get(port, "/cli/compute/jobs/get", &a, &format!("job={l}")).await)["state"] == "cancelled" })
    })
    .await;

    // ── the blob route's limits (signed directly, as a tenant) ───────
    let mut me = self_tenant();
    me.epoch = j(&me.call(port, "/cli/compute/hub/hello", json!({})).await)["epoch"].as_i64().unwrap();
    let sha_of = |b: &[u8]| k2_node_proto::crypto::sha256_hex(b);
    // Unsigned.
    let r = raw(port, request_bytes("POST", &format!("/cli/compute/hub/blob?sha256={}&kind=dirty", sha_of(b"x")), &[], b"x")).await;
    assert_eq!((r.status, code(&r)), (401, "hub_auth_missing".to_string()));
    // Over the relay cap on the tunnel door: refused before the body.
    let q = format!("sha256={}&kind=dirty", sha_of(b"big"));
    let r = raw(d.tunnel_port, blob_request(&me, &q, &sha_of(b"big"), b"", 60 << 20)).await;
    assert_eq!((r.status, code(&r)), (413, "blob_too_large".to_string()), "{}", r.body);
    // Over the LAN cap on the direct door.
    let q = format!("sha256={}&kind=dirty", sha_of(b"huge"));
    let r = raw(port, blob_request(&me, &q, &sha_of(b"huge"), b"", (1usize << 30) + 1)).await;
    assert_eq!((r.status, code(&r)), (413, "blob_too_large".to_string()), "{}", r.body);
    // The bytes must be what was named.
    let q = format!("sha256={}&kind=dirty", sha_of(b"named"));
    let r = raw(port, blob_request(&me, &q, &sha_of(b"named"), b"other", 5)).await;
    assert_eq!((r.status, code(&r)), (400, "blob_mismatch".to_string()), "{}", r.body);
    // The tenant's blob quota.
    let r = post(port, "/cli/compute/tenants/set", OWNER, json!({"tenant": "me", "blobMb": 1})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let two_mb = vec![7u8; 2 << 20];
    let q = format!("sha256={}&kind=dirty", sha_of(&two_mb));
    let r = raw(port, blob_request(&me, &q, &sha_of(&two_mb), &two_mb, two_mb.len())).await;
    assert_eq!((r.status, code(&r)), (409, "blob_quota".to_string()), "{}", r.body);
    let r = post(port, "/cli/compute/tenants/set", OWNER, json!({"tenant": "me", "blobMb": null})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    // Within the caps it lands in the tenant's store, file-backed.
    let r = raw(port, blob_request(&me, &q, &sha_of(&two_mb), &two_mb, two_mb.len())).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(env.home.join(".k2/compute/tenants").join(&tid).join("blobs").join(sha_of(&two_mb)).is_file());

    // ── hub off: new runs are local again; remote ids still resolve ──
    let r = post(port, "/cli/compute/hubs/off", OWNER, json!({})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(port, "/cli/compute/run", &agent, json!({"node": "sn1", "argv": ["true"], "clientJobId": "sub-client-00000006", "noSource": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(j(&r)["job"]["tenantId"], "local", "no hub: this server's own controller");
    assert!(j(&r).get("hub").is_none());
    let r = get(port, "/cli/compute/jobs/get", &agent, &format!("job={job1}")).await;
    assert_eq!((r.status, j(&r)["hub"].as_str()), (200, Some("selfhub")), "{}", r.body);

    running.runner.abort();
    running.node.stop_all(k2_node_proto::frames::JobState::Cancelled, "test_over");
    drop(env);
}
