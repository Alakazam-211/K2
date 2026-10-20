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

// ── HTTP ─────────────────────────────────────────────────────────────

struct Resp {
    status: u16,
    body: String,
}

fn raw_blocking(port: u16, raw: Vec<u8>) -> Resp {
    let mut s = StdTcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    s.write_all(&raw).unwrap();
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
    assert_eq!(labels, vec!["local", "alpha-srv", "beta-srv"], "{}", r.body);

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
