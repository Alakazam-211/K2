//! Compute package requests end to end (0.46.9,
//! prd-compute-self-serve-environments-v1 §6): the REAL dispatcher
//! in-process as the controller, the REAL `k2-node` runtime as the node,
//! and the node's root executor in-process with a scripted package manager
//! (`pkgsys::fake::FakeBackend`; nothing real is installed).
//!
//! Covers: the node reports its package view (protocol 7); an agent files a
//! request and gets the node's dry run; a passport can't decide; the owner
//! approves, root installs through the guards and the agent's row reads
//! installed; the dry-run guard refuses an install that would remove a
//! package (nothing runs); a job whose package the policy doesn't cover is
//! refused at submit with a filed request; widening the policy is held,
//! narrowing applies; the audit trail on both ends.
// k2-node is Unix-only; host facts and packages are Linux's.
#![cfg(target_os = "linux")]

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
use k2_node::pkgsys::{fake::FakeBackend, Backend, HostCtx};
use k2_node_proto::pkgmap::Mgr;
use rusqlite::params;
use serde_json::{json, Value};

static TEST_LOCK: StdMutex<()> = StdMutex::new(());
const OWNER: &str = "owner-token-pkg-e2e";

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
    let home = k2_core::test_env::short_tmp(format!("k2pkg-{:x}", nanos % 0xffff_ffff));
    std::fs::create_dir_all(&home).unwrap();
    let home = dunce::canonicalize(&home).unwrap();
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
    conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)", params![id, handle, format!("/tmp/k2pkg-ws-{id}")]).unwrap();
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

fn ubuntu() -> HostCtx {
    HostCtx { mgr: Mgr::Apt, distro: "ubuntu".into(), version: "24.04".into(), like: vec!["debian".into()] }
}

async fn row_state(port: u16, id: &str) -> String {
    let r = get(port, "/cli/compute/nodes/packages/requests", OWNER, "state=all").await;
    assert_eq!(r.status, 200, "{}", r.body);
    j(&r)["requests"].as_array().unwrap().iter().find(|x| x["batchId"] == id || x["id"] == id).map(|x| x["state"].as_str().unwrap_or("").to_string()).unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn request_card_approve_and_the_node_installs() {
    let env = setup();
    std::env::set_var("K2_COMPUTE", "1");
    let d = test_harness::start(OWNER).await;
    let port = d.port;

    let ws = seed_ws("tests");
    let agent = passport(&ws);

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

    // The scripted package manager both the node and root see.
    let fake = Arc::new(FakeBackend::new(Mgr::Apt));
    fake.add("libgtk-3-dev", &["libglib2.0-dev", "libpango1.0-dev"], 4000);
    fake.add("libglib2.0-dev", &[], 3000);
    fake.add("libpango1.0-dev", &[], 2000);
    fake.add("libbad-dev", &[], 10);
    fake.state.lock().unwrap().available.get_mut("libbad-dev").unwrap().removes = vec!["libold1".into()];
    fake.add("libpq-dev", &[], 100);
    let backend: Arc<dyn Backend> = fake.clone();

    // Root's side: admin.toml (full) and the executor with the fake.
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
    let admin_sock = k2_core::test_env::short_tmp(format!("k2pkg-{}.sock", &k2_node_proto::crypto::random_id().unwrap()[..10]));
    let state_dir = dir.join("admin-state");
    let paths = ExecPaths { config_dir: layout.config.clone(), state_dir: state_dir.clone(), home: layout.home.clone(), binary: dir.join("bin/k2-node") };
    k2_node::admin::exec::ensure_state_dir(&paths.state_dir).unwrap();
    let exec = Arc::new(Executor::new(paths, Arc::new(SystemPlatform) as Arc<dyn Platform>));
    exec.set_pkg_backend(ubuntu(), backend.clone());
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
    o.facts = Some(k2_node::config::MachineFacts { cores: 8, mem_bytes: 32 << 30, disk_total_bytes: 1 << 40, has_battery: false, is_macos: false });
    o.pkg_backend = Some(backend.clone());
    o.pkg_host = Some(ubuntu());
    let node = Node::open(o).await.expect("node open");
    node.spawn_background();
    let runner = tokio::spawn(k2_node::session::run_forever(node.clone()));

    // Protocol 7: host facts and the package view reach the controller.
    wait_for("a protocol 7 offer with packages", 30, || {
        Box::pin(async move {
            let r = get(port, "/cli/compute/nodes", OWNER, "").await;
            r.status == 200
                && j(&r)["nodes"].as_array().unwrap().iter().any(|n| {
                    n["name"] == "m1" && n["host"]["pkgManager"] == "apt" && n["offer"]["features"].as_array().is_some_and(|f| f.iter().any(|x| x == "pkg-requests"))
                })
        })
    })
    .await;
    let r = get(port, "/cli/compute/nodes", OWNER, "").await;
    let n = j(&r)["nodes"].as_array().unwrap().iter().find(|n| n["name"] == "m1").unwrap().clone();
    assert!(n["platform"].as_str().unwrap().ends_with("-x86_64") || n["platform"].as_str().unwrap().ends_with("-aarch64"), "{n}");
    assert!(n["host"]["line"].as_str().unwrap().contains(" · apt · "), "{n}");
    // No approval key is needed here: a package approval is an Operate
    // command a person decided (no owner signature), and a widened policy
    // is only held, never sent, in this test.

    // The owner turns on compute for the agent's workspace and grants it
    // the node.
    let r = post(port, "/cli/compute/agents/set", OWNER, json!({"workspace": "tests", "value": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(port, "/cli/compute/grants/set", OWNER, json!({"node": "m1", "workspace": "tests", "maxJobSecs": 7200, "maxDiskGb": 20, "maxParallel": 2, "maxQueued": 10})).await;
    assert_eq!(r.status, 200, "{}", r.body);

    // The agent reads the node's packages: today's rule (list).
    let r = get(port, "/cli/compute/nodes/packages", &agent, "node=m1").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!((j(&r)["packages"]["level"].as_str(), j(&r)["takesRequests"].as_bool()), (Some("list"), Some(true)), "{}", r.body);

    // The agent asks; the node's dry run is on the request.
    let r = post(port, "/cli/compute/nodes/packages/request", &agent, json!({"nodes": ["m1"], "names": ["libgtk-3-dev"], "why": "tauri tests"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = j(&r);
    assert_eq!(v["pkgHeld"], true, "{v}");
    assert!(v.get("held").is_none(), "never the node admin card's field: {v}");
    let batch = v["request"]["id"].as_str().unwrap().to_string();
    assert!(batch.starts_with("cpr_"), "{batch}");
    let row = &v["request"]["rows"][0];
    assert_eq!(row["dry"]["installs"].as_array().map(Vec::len), Some(3), "the dry run with dependencies: {row}");
    assert_eq!(row["dry"]["size_mb"], 9, "{row}");
    assert!(v["request"]["summary"].as_str().unwrap().contains("Install 1 package on m1"), "{v}");
    assert!(fake.state.lock().unwrap().installs.is_empty(), "nothing installed before a person decides");

    // A passport can't decide, not even its own ask.
    let r = post(port, "/cli/compute/nodes/packages/decide", &agent, json!({"request": batch, "approve": true})).await;
    assert_eq!((r.status, j(&r)["error"]["code"].as_str()), (403, Some("owner_only")), "{}", r.body);

    // The owner approves (as the card's Allow does) and adds it to the list.
    let r = post(port, "/cli/compute/nodes/packages/decide", OWNER, json!({"request": batch, "approve": true, "add": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    wait_for("the install to finish", 30, || {
        let b = batch.clone();
        Box::pin(async move { row_state(port, &b).await == "installed" })
    })
    .await;
    {
        let st = fake.state.lock().unwrap();
        assert_eq!(st.installs, vec![vec!["libgtk-3-dev".to_string()]], "one install of the asked name");
        for p in ["libgtk-3-dev", "libglib2.0-dev", "libpango1.0-dev"] {
            assert!(st.installed.contains(p), "{p} installed");
        }
    }
    let approved = std::fs::read_to_string(layout.config.join("k2-node-packages.toml")).unwrap();
    assert!(approved.contains("\"libgtk-3-dev\""), "Allow and add puts it on the list: {approved}");
    let log = std::fs::read_to_string(state_dir.join("packages.log")).unwrap();
    assert!(log.contains("\"result\":\"installed\"") && log.contains("owner approval"), "root's install log: {log}");

    // The dry-run guard: an install that removes a package never runs.
    let r = post(port, "/cli/compute/nodes/packages/request", &agent, json!({"nodes": ["m1"], "names": ["libbad-dev"], "why": "x"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let bad = j(&r)["request"]["id"].as_str().unwrap().to_string();
    assert!(j(&r)["request"]["rows"][0]["dry"]["refusal"].as_str().unwrap_or("").starts_with("package_removes"), "the card warns: {}", r.body);
    let r = post(port, "/cli/compute/nodes/packages/decide", OWNER, json!({"request": bad, "approve": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    wait_for("root to refuse it", 30, || {
        let b = bad.clone();
        Box::pin(async move { row_state(port, &b).await == "failed" })
    })
    .await;
    let rr = get(port, "/cli/compute/nodes/packages/requests", OWNER, "state=failed").await;
    assert!(j(&rr)["requests"][0]["note"].as_str().unwrap().starts_with("package_removes"), "{}", rr.body);
    assert_eq!(fake.state.lock().unwrap().installs.len(), 1, "nothing ran for the refused install");

    // A job whose package this server's policy doesn't cover: refused at
    // submit, with the request filed for the owner.
    let run = |needs: Value, plan: bool| {
        json!({"node": "m1", "argv": ["true"], "needs": needs, "noSource": true, "plan": plan, "clientJobId": format!("pkgjob-{}", uuid::Uuid::new_v4().simple())})
    };
    let r = post(port, "/cli/compute/run", &agent, run(json!(["sys:libpq-dev"]), false)).await;
    assert_eq!((r.status, j(&r)["error"]["code"].as_str()), (403, Some("package_not_approved")), "{}", r.body);
    let filed = j(&r)["error"]["request"].as_str().unwrap().to_string();
    assert!(filed.starts_with("cpr_"), "{}", r.body);
    assert_eq!(row_state(port, &filed).await, "pending");
    // Installed now: the same check passes.
    let r = post(port, "/cli/compute/run", &agent, run(json!(["sys:libgtk-3-dev"]), true)).await;
    assert_eq!(r.status, 200, "an installed package needs nothing: {}", r.body);
    // Never satisfiable: a native name for another manager.
    let r = post(port, "/cli/compute/run", &agent, run(json!(["sys:pacman:protobuf"]), false)).await;
    assert_eq!((r.status, j(&r)["error"]["code"].as_str()), (409, Some("no_package_mapping")), "{}", r.body);
    assert!(j(&r)["error"]["hint"].as_str().unwrap().contains("It can't be requested"), "{}", r.body);

    // Policy: widening is the owner's (held); at `official` the job's
    // package is covered.
    let r = post(port, "/cli/compute/nodes/packages/policy", OWNER, json!({"node": "m1", "server": "own", "level": "official", "reason": "own jobs"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(j(&r)["held"], true, "a widening is held for the owner's approval: {}", r.body);
    let pol = layout.config.join("k2-node-package-policy.toml");
    assert!(!std::fs::read_to_string(&pol).unwrap_or_default().contains("official"), "nothing written before the approval");
    std::fs::write(&pol, "[servers.own]\nlevel = \"official\"\nmax_install_mb = 2048\n").unwrap();
    node.refresh_pkg_cache();
    node.notify_offers();
    wait_for("the node to report official", 20, || {
        let a = agent.clone();
        Box::pin(async move { j(&get(port, "/cli/compute/nodes/packages", &a, "node=m1").await)["packages"]["level"] == "official" })
    })
    .await;
    let r = post(port, "/cli/compute/run", &agent, run(json!(["sys:libpq-dev"]), true)).await;
    assert_eq!(r.status, 200, "official covers it: {}", r.body);
    let r = post(port, "/cli/compute/run", &agent, run(json!(["sys:openssh-server"]), true)).await;
    assert_eq!((r.status, j(&r)["error"]["code"].as_str()), (409, Some("no_package_mapping")), "denied at every level: {}", r.body);
    // Narrowing applies at once.
    let r = post(port, "/cli/compute/nodes/packages/policy", OWNER, json!({"node": "m1", "server": "own", "level": "list"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(j(&r)["applied"], true, "{}", r.body);
    assert!(std::fs::read_to_string(&pol).unwrap().contains("level = \"list\""), "{}", std::fs::read_to_string(&pol).unwrap());

    // The audit trail on the controller.
    let r = get(port, "/cli/compute/events", OWNER, "limit=200").await;
    let kinds: Vec<String> = j(&r)["events"].as_array().unwrap().iter().filter_map(|e| e["kind"].as_str().map(str::to_string)).collect();
    for k in ["pkg_requested", "pkg_approved", "pkg_installed", "pkg_failed"] {
        assert!(kinds.iter().any(|x| x == k), "missing {k}: {kinds:?}");
    }

    runner.abort();
    let _ = std::fs::remove_file(&admin_sock);
}
