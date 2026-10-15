//! K2 compute, results and reliability (research-distributed-compute-gaps
//! G1–G5, G8): the real dispatcher in-process (temp `$HOME`, loopback)
//! with REAL `k2-node` runtimes over loopback WebSockets, plus the real
//! `cli/k2` for the agent-facing commands.
//!
//! - results back: `--artifact` globs, the result envelope on
//!   `get --json` / `run --json`, `fetch`, JUnit failures first in
//!   `logs --failures`, test counts in the wake message;
//! - never stuck: queue timeouts, `--any` late binding (moves off a busy
//!   node), lost nodes, infra-only retries on another node (the CLI
//!   follows the new attempt), a returning lost node is told to stop,
//!   a node with infra failures in a row is set aside;
//! - stall timeout: a silent job ends `timeout/stalled`, never retried;
//! - split runs (compute B): `k2 compute test --shards N` puts one nextest
//!   partition per node, merges the JUnit, gives one exit code, one wake
//!   message; logs/fetch/retry/cancel work on the run.

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
use rusqlite::params;
use serde_json::{json, Value};

static TEST_LOCK: StdMutex<()> = StdMutex::new(());
const OWNER: &str = "owner-token-compute-results";

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
    let home = PathBuf::from(format!("/tmp/k2cr-{:x}", nanos % 0xffff_ffff));
    std::fs::create_dir_all(&home).unwrap();
    let home = std::fs::canonicalize(&home).unwrap();
    let env = Env { prev_home: std::env::var_os("HOME"), home: home.clone(), _guard: guard };
    std::env::set_var("HOME", &home);
    std::env::set_var("K2_COMPUTE", "1");
    env
}

// ── HTTP over loopback ───────────────────────────────────────────────

struct Resp {
    status: u16,
    body: String,
}

fn req_blocking(port: u16, method: &str, pq: &str, body: Option<String>) -> Resp {
    let mut s = StdTcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(90))).unwrap();
    let raw = match body {
        Some(b) => format!(
            "{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None if method == "POST" => format!("{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"),
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
    let status = text.lines().next().and_then(|l| l.split_whitespace().nth(1)).and_then(|x| x.parse().ok()).unwrap_or_else(|| panic!("no status: {text:?}"));
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

fn code(r: &Resp) -> String {
    j(r)["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {}", r.body)).to_string()
}

async fn job(port: u16, id: &str) -> Value {
    let r = get(port, "/cli/compute/jobs/get", OWNER, &format!("job={id}")).await;
    assert_eq!(r.status, 200, "{}", r.body);
    j(&r)
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

async fn wait_state(port: u16, id: &str, secs: u64, pred: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let v = job(port, id).await;
        if pred(&v) {
            return v;
        }
        assert!(Instant::now() < deadline, "job {id} never got there: {v}");
        tokio::time::sleep(Duration::from_millis(150)).await;
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
    path: PathBuf,
}

/// A workspace repo with `files` committed.
fn seed_ws(env: &Env, handle: &str, files: &[(&str, &str)]) -> Ws {
    let path = env.home.join(format!("ws-{handle}"));
    std::fs::create_dir_all(&path).unwrap();
    git(&path, &["init", "-q"]);
    std::fs::write(path.join("a.txt"), format!("hello from {handle}\n")).unwrap();
    for (name, body) in files {
        let p = path.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }
    git(&path, &["add", "."]);
    git(&path, &["commit", "-q", "-m", "one"]);
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)", params![id, handle, path.to_string_lossy()]).unwrap();
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

struct RunningNode {
    node: std::sync::Arc<Node>,
    runner: tokio::task::JoinHandle<()>,
}

impl Drop for RunningNode {
    fn drop(&mut self) {
        self.runner.abort();
        self.node.stop_all(k2_node_proto::frames::JobState::Cancelled, "test_over");
    }
}

impl RunningNode {
    /// The machine drops off the network: no session, no redial. Its jobs
    /// keep running locally (like a node whose Wi-Fi died).
    fn cut(&mut self, node_id: &str) {
        self.runner.abort();
        k2_daemon::compute_ws::drop_connection(node_id);
    }

    /// Back on the network.
    fn revive(&mut self) {
        self.runner = tokio::spawn(k2_node::session::run_forever(self.node.clone()));
    }
}

fn tuning_for_tests() -> k2_core::compute::retry::Tuning {
    k2_core::compute::retry::Tuning {
        replace_after_secs: 1,
        node_lost_secs: 2,
        retry_base_secs: 0,
        retry_cap_secs: 0,
        quarantine_after: 2,
        quarantine_secs: 300,
        tick_millis: 250,
        ..Default::default()
    }
}

/// Submit as the owner for `ws`; returns the job id.
async fn submit(port: u16, ws: &Ws, n: &mut u32, mut body: Value) -> String {
    *n += 1;
    body["workspace"] = json!(ws.id);
    body["clientJobId"] = json!(format!("client-{}-{:08}", &ws.id[..8], n));
    let r = post(port, "/cli/compute/run", OWNER, body).await;
    assert_eq!(r.status, 200, "{}", r.body);
    j(&r)["job"]["id"].as_str().unwrap().to_string()
}

async fn cancel(port: u16, id: &str) {
    let r = post(port, "/cli/compute/jobs/cancel", OWNER, json!({"job": id})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    wait_state(port, id, 20, |v| v["state"] == "cancelled").await;
}

fn node_layout(env: &Env, tag: &str, parallel: u32) -> Layout {
    let dir = env.home.join(format!("node-{tag}"));
    let layout = Layout::new(dir.join("home"), dir.join("config"));
    layout.ensure().unwrap();
    std::fs::create_dir_all(&layout.config).unwrap();
    std::fs::write(
        layout.policy(),
        format!("availability = \"always\"\nforeign_locks = []\nwrite_smoke_lock = \"\"\ndisk_floor_gb = 0\nmax_parallel = {parallel}\n"),
    )
    .unwrap();
    layout
}

fn node_options(layout: Layout) -> NodeOptions {
    let mut o = NodeOptions::new(layout);
    o.dev = true;
    o.cancel_grace = Duration::from_millis(300);
    o.config_poll = Duration::from_millis(100);
    o.backoff_min = Duration::from_millis(50);
    o.backoff_max = Duration::from_millis(300);
    o.offer_every = Duration::from_secs(1);
    o.keep_awake = false;
    o.use_cgroups = false;
    o.health = false;
    o.facts = Some(k2_node::config::MachineFacts { cores: 8, mem_bytes: 32 << 30, disk_total_bytes: 1 << 40, has_battery: false, is_macos: cfg!(target_os = "macos") });
    o
}

async fn start_node(layout: &Layout) -> RunningNode {
    let node = Node::open(node_options(layout.clone())).await.expect("node open");
    node.spawn_background();
    let runner = tokio::spawn(k2_node::session::run_forever(node.clone()));
    RunningNode { node, runner }
}

async fn node_row(port: u16, name: &str) -> Value {
    let r = get(port, "/cli/compute/nodes", OWNER, "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    j(&r)["nodes"].as_array().unwrap().iter().find(|n| n["name"] == name).cloned().unwrap_or(Value::Null)
}

/// Enroll + confirm a node called `name`, start it, wait for its offer.
async fn online_node(env: &Env, port: u16, name: &str, parallel: u32) -> (RunningNode, String) {
    let base = format!("http://127.0.0.1:{port}");
    let r = post(port, "/cli/compute/nodes/enroll-code", OWNER, json!({"name": name, "url": base})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let es = k2_node_proto::pairing::EnrollString::parse(j(&r)["enroll"].as_str().unwrap()).unwrap();
    let layout = node_layout(env, name, parallel);
    let key = k2_node::identity::load_or_create_key(&layout.key()).unwrap();
    let mut sock = k2_node::session::connect(&base, k2_node::session::ENROLL_PATH).await.unwrap();
    let done = k2_node::enroll::enroll_on(&mut sock, &key, &base, &k2_node::enroll::Credential::Code(es), name, &Default::default(), k2_node::util::now())
        .await
        .unwrap();
    k2_node::identity::write_pin(&layout.pin(), &done.pin).unwrap();
    let r = post(port, "/cli/compute/nodes/confirm", OWNER, json!({"node": name, "sas": done.sas})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let running = start_node(&layout).await;
    let n = name.to_string();
    wait_for("node offer", 30, move || {
        let n = n.clone();
        Box::pin(async move { node_row(port, &n).await["offer"].is_object() })
    })
    .await;
    let id = node_row(port, name).await["id"].as_str().unwrap().to_string();
    (running, id)
}

/// The real `cli/k2` against this daemon (owner token from disk, a clean
/// env so the outer K2 cell never leaks in). Returns (exit, stdout, stderr).
fn k2(env: &Env, port: u16, cwd: &Path, args: &[&str]) -> (i32, String, String) {
    let k2_dir = env.home.join(".k2");
    std::fs::create_dir_all(&k2_dir).unwrap();
    std::fs::write(k2_dir.join("heartbeat.port"), port.to_string()).unwrap();
    std::fs::write(k2_dir.join("heartbeat.token"), OWNER).unwrap();
    let cli = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../cli/k2");
    let out = Command::new("bash")
        .arg(&cli)
        .args(args)
        .env_clear()
        .env("PATH", std::env::var("PATH").expect("PATH"))
        .env("HOME", &env.home)
        .env("TMPDIR", env.home.join("cli-tmp"))
        .env("K2_PROJECT_PATH", cwd)
        .current_dir(cwd)
        .output()
        .expect("run cli/k2");
    std::fs::create_dir_all(env.home.join("cli-tmp")).ok();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

const JUNIT: &str = r#"<testsuites><testsuite name="k2-core"><testcase name="ok_one" classname="k2-core"/><testcase name="bad_one" classname="k2-core"><failure type="test failure">thread bad_one panicked at src/x.rs:1:1:
boom here</failure></testcase></testsuite></testsuites>"#;

/// A committed script that writes JUnit + a binary, then exits 100 (what
/// nextest does when a test fails).
fn results_script() -> String {
    format!("mkdir -p out/bin\ncat > out/junit.xml <<'EOF'\n{JUNIT}\nEOF\nprintf 'BIN' > out/bin/tool.bin\necho running\nexit 100\n")
}

// ── results back (G1, G2) ────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn results_come_back_as_artifacts_tests_and_one_result_object() {
    let env = setup();
    k2_core::compute::retry::set_tuning_for_tests(Default::default());
    let d = test_harness::start(OWNER).await;
    let port = d.port;
    std::fs::create_dir_all(env.home.join("cli-tmp")).unwrap();
    let ws = seed_ws(&env, "res", &[("t.sh", &results_script())]);
    let (_n1, _) = online_node(&env, port, "r1", 2).await;

    // A bad glob is a usage error (exit 2), refused before anything runs.
    let (rc, _, err) = k2(&env, port, &ws.path, &["compute", "run", "r1", "--artifact", "../x", "--", "sh", "t.sh"]);
    assert_eq!(rc, 2, "{err}");
    assert!(err.contains("artifact_glob_escapes"), "{err}");

    // Attached `run --json`: logs on stderr, the result object on stdout,
    // the command's own exit code.
    let (rc, out, err) = k2(&env, port, &ws.path, &["compute", "run", "r1", "--artifact", "out/*.xml", "--artifact", "out/**/*.bin", "--json", "--", "sh", "t.sh"]);
    assert_eq!(rc, 100, "the job's exit code: {err}");
    assert!(err.contains("running"), "logs go to stderr under --json: {err}");
    let r: Value = serde_json::from_str(&out).unwrap_or_else(|e| panic!("stdout is one JSON object ({e}): {out:?}"));
    assert_eq!(r["exit"], 100);
    assert_eq!(r["state"], "done");
    assert_eq!(r["ok"], false);
    assert_eq!(r["node"], "r1");
    assert!(r["durationSecs"].is_i64(), "{r}");
    let names: Vec<&str> = r["artifacts"].as_array().unwrap().iter().map(|a| a["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["out/junit.xml", "out/bin/tool.bin"]);
    assert_eq!(r["artifacts"][1]["sha256"], k2_node_proto::crypto::sha256_hex(b"BIN"));
    assert_eq!((r["tests"]["passed"].as_u64(), r["tests"]["failed"].as_u64()), (Some(1), Some(1)));
    assert_eq!(r["tests"]["failing"][0]["name"], "bad_one");
    let id = r["job"].as_str().unwrap().to_string();
    let short = r["short"].as_str().unwrap().to_string();

    // The receipt lands right after the end state: `get --json` names it.
    let v = wait_state(port, &id, 20, |v| v["result"]["receipt"]["verified"] == true).await;
    assert_eq!(v["result"]["receipt"]["id"].as_str().unwrap().len(), 16);
    let (rc, out, err) = k2(&env, port, &ws.path, &["compute", "get", &short, "--json"]);
    assert_eq!(rc, 0, "{err}");
    let g: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(g["exit"], 100);
    assert_eq!(g["receipt"]["verified"], true);

    // fetch: every kept file, checked against the node's sha256.
    let dest = env.home.join("got");
    let (rc, out, err) = k2(&env, port, &ws.path, &["compute", "fetch", &short, "--out", dest.to_str().unwrap(), "--json"]);
    assert_eq!(rc, 0, "{err}");
    let f: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(f["files"].as_array().unwrap().len(), 2);
    assert_eq!(std::fs::read_to_string(dest.join("out/junit.xml")).unwrap().trim(), JUNIT);
    assert_eq!(std::fs::read(dest.join("out/bin/tool.bin")).unwrap(), b"BIN");
    // One name only, and a name the job never kept.
    let one = env.home.join("one");
    let (rc, _, err) = k2(&env, port, &ws.path, &["compute", "fetch", &short, "out/bin/tool.bin", "--out", one.to_str().unwrap()]);
    assert_eq!(rc, 0, "{err}");
    assert!(one.join("out/bin/tool.bin").is_file() && !one.join("out/junit.xml").exists());
    let (rc, _, err) = k2(&env, port, &ws.path, &["compute", "fetch", &short, "out/nope"]);
    assert_eq!(rc, 3, "{err}");
    assert!(err.contains("artifact_not_found"), "{err}");

    // logs --failures: the failing tests first, from the JUnit.
    let (rc, out, err) = k2(&env, port, &ws.path, &["compute", "logs", &short, "--failures"]);
    assert_eq!(rc, 0, "{err}");
    let first = out.lines().next().unwrap_or_default();
    assert!(first.starts_with("FAILING TESTS: 1 failed, 0 errors, 1 passed"), "{out}");
    assert!(out.contains("k2-core bad_one — thread bad_one panicked at src/x.rs:1:1:"), "{out}");

    // A detached agent run: the wake message carries the counts (never
    // test names), and `run --json --detach` prints the queued result.
    let r = post(port, "/cli/compute/agents/set", OWNER, json!({"workspace": ws.id, "value": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let (agent, sid) = passport(&ws);
    k2_daemon::compute_ws::test_sink_register(&sid);
    let r = post(
        port,
        "/cli/compute/run",
        &agent,
        json!({"node": "r1", "argv": ["sh", "t.sh"], "clientJobId": "client-results-0001", "detach": true, "artifacts": ["out/*.xml"]}),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    let id2 = j(&r)["job"]["id"].as_str().unwrap().to_string();
    let s2 = sid.clone();
    wait_for("wake message", 30, move || {
        let s = s2.clone();
        Box::pin(async move { !k2_daemon::compute_ws::test_sink_peek(&s).is_empty() })
    })
    .await;
    let msgs = k2_daemon::compute_ws::test_sink_take(&sid);
    let short2 = &id2[..8];
    assert!(msgs[0].contains("exit 100") && msgs[0].contains("; tests 1 passed, 1 failed."), "{msgs:?}");
    assert!(msgs[0].contains(&format!("k2 compute logs {short2} --failures · k2 compute get {short2} --json")), "{msgs:?}");
    assert!(!msgs[0].contains("bad_one"), "test names are program output and stay out of the wake line");

    // The artifact route lists, then pulls slices (agent passport).
    let r = get(port, "/cli/compute/jobs/artifact", &agent, &format!("job={id2}")).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(j(&r)["artifacts"][0]["name"], "out/junit.xml");
    let r = get(port, "/cli/compute/jobs/artifact", &agent, &format!("job={id2}&name=out%2Fjunit.xml&offset=5&max=65536")).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = j(&r);
    let data = k2_node_proto::crypto::unb64(v["data"].as_str().unwrap()).unwrap();
    assert_eq!(String::from_utf8(data).unwrap().trim_end(), &JUNIT[5..]);
    assert_eq!(v["eof"], true);
    let r = get(port, "/cli/compute/jobs/artifact", &agent, &format!("job={id2}&name=..%2F..%2Fstate%2Fnode-key.pem")).await;
    assert_eq!(r.status, 404, "{}", r.body);
    assert_eq!(code(&r), "artifact_not_found");
    drop(env);
}

// ── never stuck (G3, G4) ─────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn never_stuck_timeouts_moves_lost_nodes_retries_and_set_aside() {
    let env = setup();
    k2_core::compute::retry::set_tuning_for_tests(tuning_for_tests());
    let d = test_harness::start(OWNER).await;
    let port = d.port;
    std::fs::create_dir_all(env.home.join("cli-tmp")).unwrap();
    let ws = seed_ws(&env, "ns", &[]);
    let (mut na, na_id) = online_node(&env, port, "na", 1).await;
    let (mut nb, nb_id) = online_node(&env, port, "nb", 1).await;
    let mut n = 0u32;

    // 1. A queue timeout fails loudly with the reason it waited.
    let long_a = submit(port, &ws, &mut n, json!({"node": "na", "argv": ["sleep", "30"], "noSource": true, "retries": 0})).await;
    wait_state(port, &long_a, 20, |v| v["state"] == "running").await;
    let qt = submit(port, &ws, &mut n, json!({"node": "na", "argv": ["true"], "noSource": true, "queueTimeoutSecs": 2})).await;
    let v = wait_state(port, &qt, 20, |v| v["state"] == "failed").await;
    assert_eq!(v["reason"], "queue_timeout", "{v}");
    let detail = v["detail"].as_str().unwrap();
    assert!(detail.starts_with("not started within 2s (last wait on na: parallel_full"), "{detail}");
    assert_eq!(v["result"]["ok"], false);
    cancel(port, &long_a).await;

    // 2. A real failure is a result: never retried, whatever --retries says.
    let f = submit(port, &ws, &mut n, json!({"any": true, "argv": ["sh", "-c", "exit 7"], "noSource": true, "retries": 3})).await;
    let v = wait_state(port, &f, 20, |v| v["state"] == "done").await;
    assert_eq!((v["exitCode"].as_i64(), v["attempt"].as_i64()), (Some(7), Some(1)), "{v}");

    // 3. Late binding: an --any job queued behind a busy node moves to the
    //    first node that can start it.
    let la = submit(port, &ws, &mut n, json!({"node": "na", "argv": ["sleep", "30"], "noSource": true, "retries": 0})).await;
    let lb = submit(port, &ws, &mut n, json!({"node": "nb", "argv": ["sleep", "30"], "noSource": true, "retries": 0})).await;
    wait_state(port, &la, 20, |v| v["state"] == "running").await;
    wait_state(port, &lb, 20, |v| v["state"] == "running").await;
    let x = submit(port, &ws, &mut n, json!({"any": true, "argv": ["sh", "-c", "echo moved"], "noSource": true})).await;
    let first = job(port, &x).await;
    assert_eq!(first["state"], "queued", "{first}");
    let (stay, free) = if first["nodeId"] == na_id { (&la, &lb) } else { (&lb, &la) };
    cancel(port, free).await;
    let v = wait_state(port, &x, 20, |v| v["state"] == "done").await;
    assert_ne!(v["nodeId"], first["nodeId"], "it ran on the node that freed up: {v}");
    assert_eq!(v["exitCode"], 0);
    cancel(port, stay).await;

    // 4. The node running an attached --any job drops off the network:
    //    after node_lost_secs the attempt is `interrupted/node_lost`, a
    //    new attempt runs on the other node, and the CLI follows it.
    let cli_env_home = env.home.clone();
    let ws_path = ws.path.clone();
    let k2_dir = env.home.join(".k2");
    std::fs::create_dir_all(&k2_dir).unwrap();
    std::fs::write(k2_dir.join("heartbeat.port"), port.to_string()).unwrap();
    std::fs::write(k2_dir.join("heartbeat.token"), OWNER).unwrap();
    let cli = tokio::task::spawn_blocking(move || {
        let out = Command::new("bash")
            .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../cli/k2"))
            .args(["compute", "run", "--any", "--no-source", "--retries", "1", "--", "sh", "-c", "echo start; sleep 8; echo end"])
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap())
            .env("HOME", &cli_env_home)
            .env("TMPDIR", cli_env_home.join("cli-tmp"))
            .env("K2_PROJECT_PATH", &ws_path)
            .current_dir(&ws_path)
            .output()
            .unwrap();
        (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
    });
    let mut lost_id = String::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    while lost_id.is_empty() {
        assert!(Instant::now() < deadline, "the CLI's job never ran");
        let r = get(port, "/cli/compute/jobs", OWNER, &format!("workspace={}", ws.id)).await;
        for jb in j(&r)["jobs"].as_array().unwrap() {
            if jb["argv"][2].as_str().is_some_and(|a| a.contains("sleep 8")) && jb["state"] == "running" {
                lost_id = jb["id"].as_str().unwrap().to_string();
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let lost_on = job(port, &lost_id).await["nodeId"].as_str().unwrap().to_string();
    let (gone, other_id) = if lost_on == na_id { (&mut na, nb_id.clone()) } else { (&mut nb, na_id.clone()) };
    gone.cut(&lost_on);
    let v = wait_state(port, &lost_id, 30, |v| v["attempt"] == 2 && v["state"] == "running").await;
    assert_eq!(v["nodeId"], other_id, "the retry prefers another node: {v}");
    assert_eq!(v["ctl"]["failedAttempts"][0]["reason"], "node_lost", "{v}");
    // The lost node comes back still running attempt 1: it is told to stop.
    gone.revive();
    let gnode = gone.node.clone();
    wait_for("the stale attempt stops on the returning node", 6, move || {
        let g = gnode.clone();
        Box::pin(async move { g.running_ids().is_empty() })
    })
    .await;
    let (rc, out, err) = cli.await.unwrap();
    assert_eq!(rc, 0, "the second attempt's exit: {err}");
    assert!(err.contains("retrying as attempt 2"), "{err}");
    assert!(out.ends_with("start\nend\n"), "the new attempt's output: {out:?}");
    let v = job(port, &lost_id).await;
    assert_eq!((v["state"].as_str(), v["exitCode"].as_i64(), v["attempt"].as_i64()), (Some("done"), Some(0), Some(2)), "{v}");

    // 5. Infra failures in a row set a node aside; --any skips it.
    for _ in 0..2 {
        let bad = submit(port, &ws, &mut n, json!({"node": "na", "argv": ["/nonexistent/k2-test-binary"], "noSource": true, "retries": 0})).await;
        let v = wait_state(port, &bad, 20, |v| v["state"] == "failed").await;
        assert_eq!(v["reason"], "spawn_failed", "{v}");
    }
    let row = node_row(port, "na").await;
    assert!(row["setAsideUntil"].is_i64(), "{row}");
    for _ in 0..2 {
        let a = submit(port, &ws, &mut n, json!({"any": true, "argv": ["true"], "noSource": true})).await;
        let v = wait_state(port, &a, 20, |v| v["state"] == "done").await;
        assert_eq!(v["nodeId"], nb_id, "a set-aside node takes no --any work: {v}");
    }
    k2_daemon::compute_place::clear_set_aside(&na_id);
    k2_core::compute::retry::set_tuning_for_tests(Default::default());
    drop(na);
    drop(nb);
    drop(env);
}

// ── stall timeout (G4) ───────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn a_stalled_job_ends_as_a_result_and_is_never_retried() {
    let env = setup();
    k2_core::compute::retry::set_tuning_for_tests(tuning_for_tests());
    let d = test_harness::start(OWNER).await;
    let port = d.port;
    std::fs::create_dir_all(env.home.join("cli-tmp")).unwrap();
    let ws = seed_ws(&env, "st", &[]);
    let (_n1, _) = online_node(&env, port, "s1", 1).await;

    // Via the CLI: exit 3 with the code and the detail on stderr.
    let (rc, _, err) = k2(&env, port, &ws.path, &["compute", "run", "s1", "--no-source", "--stall-timeout", "1s", "--retries", "2", "--", "sh", "-c", "echo hi; sleep 30"]);
    assert_eq!(rc, 3, "{err}");
    assert!(err.contains("\"reason\": \"stalled\"") && err.contains("no output for 1s"), "{err}");
    let r = get(port, "/cli/compute/jobs", OWNER, &format!("workspace={}", ws.id)).await;
    let v = j(&r)["jobs"][0].clone();
    assert_eq!((v["state"].as_str(), v["attempt"].as_i64()), (Some("timeout"), Some(1)), "a stall is a result, not retried: {v}");

    // The default applies when the node enforces it; `off` sends none.
    let mut n = 0u32;
    let id = submit(port, &ws, &mut n, json!({"node": "s1", "argv": ["true"], "noSource": true, "timeoutSecs": 7200})).await;
    let plan_json: String = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row("SELECT plan_json FROM compute_jobs WHERE id = ?1", params![id], |r| r.get(0)).unwrap()
    };
    let p: Value = serde_json::from_str(&plan_json).unwrap();
    assert_eq!(p["stall_secs"], 1800, "{p}");
    let id = submit(port, &ws, &mut n, json!({"node": "s1", "argv": ["true"], "noSource": true, "stallSecs": 0})).await;
    let p: Value = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        serde_json::from_str(&conn.query_row("SELECT plan_json FROM compute_jobs WHERE id = ?1", params![id], |r| r.get::<_, String>(0)).unwrap()).unwrap()
    };
    assert!(p.get("stall_secs").is_none(), "{p}");
    k2_core::compute::retry::set_tuning_for_tests(Default::default());
    drop(env);
}

// ── split runs = compute B (G8) ──────────────────────────────────────

/// Stands in for `cargo nextest run` on the node: reads --profile,
/// --partition and --config-file, writes JUnit where nextest would
/// (`$CARGO_TARGET_DIR/nextest/<profile>/junit.xml`). Shard 2 fails.
const FAKE_NEXTEST: &str = r#"profile=default; part=""; cfg=""
while [ $# -gt 0 ]; do
  case "$1" in
    --profile) profile="$2"; shift 2 ;;
    --partition) part="$2"; shift 2 ;;
    --config-file) cfg="$2"; shift 2 ;;
    *) shift ;;
  esac
done
i=${part#hash:}; i=${i%/*}; n=${part#*/}
out="$CARGO_TARGET_DIR/nextest/$profile"; mkdir -p "$out"
if [ "$i" = "2" ]; then fail='<testcase name="t2_bad" classname="k2-core"><failure message="boom in shard 2"/></testcase>'; rc=100; else fail=""; rc=0; fi
printf '<testsuites><testsuite name="k2-core"><testcase name="t%s_a" classname="k2-core"/><testcase name="t%s_b" classname="k2-core"/>%s</testsuite></testsuites>' "$i" "$i" "$fail" > "$out/junit.xml"
grep -q 'path = "junit.xml"' "$cfg" && echo "junit config ok"
env | grep -q '^K2_' && echo "K2 VARIABLE LEAKED"
[ -d "$XDG_RUNTIME_DIR" ] && echo "xdg ok"
echo "shard $i of $n, profile $profile"
exit $rc
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn split_runs_spread_shards_merge_junit_and_give_one_result() {
    let env = setup();
    k2_core::compute::retry::set_tuning_for_tests(tuning_for_tests());
    let d = test_harness::start(OWNER).await;
    let port = d.port;
    std::fs::create_dir_all(env.home.join("cli-tmp")).unwrap();
    let ws = seed_ws(&env, "sp", &[("fake-nextest.sh", FAKE_NEXTEST)]);
    let (_a, a_id) = online_node(&env, port, "sa", 1).await;
    let (_b, b_id) = online_node(&env, port, "sb", 1).await;
    let (_c, c_id) = online_node(&env, port, "sc", 1).await;

    // Usage errors first (exit 2): shards need --any and the placeholders.
    let (rc, _, err) = k2(&env, port, &ws.path, &["compute", "run", "sa", "--shards", "2", "--", "true"]);
    assert_eq!(rc, 2, "{err}");
    let (rc, _, err) = k2(&env, port, &ws.path, &["compute", "run", "--any", "--shards", "2", "--", "true"]);
    assert_eq!(rc, 2, "{err}");
    assert!(err.contains("{shard}"), "{err}");

    // `k2 compute test --shards 3`: one partition per node, one result.
    let (rc, out, err) = k2(&env, port, &ws.path, &["compute", "test", "--shards", "3", "--runner", "sh fake-nextest.sh", "--json"]);
    assert_eq!(rc, 100, "the failing shard's exit code: {err}");
    let r: Value = serde_json::from_str(&out).unwrap_or_else(|e| panic!("one JSON result ({e}): {out:?} / {err}"));
    assert_eq!((r["state"].as_str(), r["exit"].as_i64(), r["ok"].as_bool()), (Some("done"), Some(100), Some(false)), "{r}");
    let shards = r["shards"].as_array().unwrap();
    assert_eq!(shards.len(), 3);
    let mut nodes: Vec<String> = shards.iter().map(|x| x["node"].as_str().unwrap().to_string()).collect();
    nodes.sort();
    assert_eq!(nodes, vec!["sa", "sb", "sc"], "one shard per node");
    assert_eq!(shards[1]["exit"], 100);
    let t = &r["tests"];
    assert_eq!((t["passed"].as_u64(), t["failed"].as_u64(), t["total"].as_u64()), (Some(6), Some(1), Some(7)), "{t}");
    assert_eq!(t["failing"][0]["name"], "t2_bad");
    assert_eq!(t["failing"][0]["shard"], 2);
    assert!(err.contains("shard 2/3 on") && err.contains("3 shards: exit 100"), "{err}");
    let run = r["short"].as_str().unwrap().to_string();

    // logs: the merged failures once, then each shard's log under a header.
    let (rc, out, err) = k2(&env, port, &ws.path, &["compute", "logs", &run]);
    assert_eq!(rc, 0, "{err}");
    for i in 1..=3 {
        assert!(out.contains(&format!("── shard {i}/3 · job ")), "{out}");
        assert!(out.contains(&format!("shard {i} of 3, profile k2")), "{out}");
    }
    assert!(out.contains("junit config ok") && out.contains("xdg ok") && !out.contains("K2 VARIABLE LEAKED"), "{out}");
    let (rc, out, _) = k2(&env, port, &ws.path, &["compute", "logs", &run, "--failures"]);
    assert_eq!(rc, 0);
    assert!(out.starts_with("FAILING TESTS: 1 failed, 0 errors, 6 passed"), "{out}");
    assert!(out.contains("[shard 2] k2-core t2_bad — boom in shard 2"), "{out}");
    assert_eq!(out.matches("FAILING TESTS").count(), 1, "once for the run: {out}");

    // fetch: every shard's JUnit, under shard-<i>/.
    let dest = env.home.join("shards-out");
    let (rc, out, err) = k2(&env, port, &ws.path, &["compute", "fetch", &run, "--out", dest.to_str().unwrap(), "--json"]);
    assert_eq!(rc, 0, "{err}");
    let f: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(f["files"].as_array().unwrap().len(), 3, "{f}");
    let x = std::fs::read_to_string(dest.join("shard-2/target/nextest/k2/junit.xml")).unwrap();
    assert!(x.contains("t2_bad"), "{x}");

    // retry: only the shard that didn't pass runs again; the run reopens.
    let parent = job(port, &run).await;
    let pid = parent["id"].as_str().unwrap().to_string();
    let r = post(port, "/cli/compute/jobs/retry", OWNER, json!({"job": pid})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(j(&r)["shards"], json!([2]));
    let v = wait_state(port, &pid, 30, |v| v["state"] == "done").await;
    let sh = v["result"]["shards"].as_array().unwrap();
    assert_eq!((sh[0]["attempt"].as_i64(), sh[1]["attempt"].as_i64(), sh[2]["attempt"].as_i64()), (Some(1), Some(2), Some(1)), "{v}");

    // A detached agent run: ONE wake message for the whole run.
    let r = post(port, "/cli/compute/agents/set", OWNER, json!({"workspace": ws.id, "value": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let (agent, sid) = passport(&ws);
    k2_daemon::compute_ws::test_sink_register(&sid);
    let r = post(
        port,
        "/cli/compute/run",
        &agent,
        json!({"any": true, "shards": 3, "detach": true, "clientJobId": "client-shards-detached-1",
               "argv": ["sh", "-c", "echo shard {shard} of {shards}"], "noSource": true}),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    let pid = j(&r)["job"]["id"].as_str().unwrap().to_string();
    let p2 = pid.clone();
    wait_for("the run ends", 30, move || {
        let p = p2.clone();
        Box::pin(async move { job(port, &p).await["state"] == "done" })
    })
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let msgs = k2_daemon::compute_ws::test_sink_take(&sid);
    assert_eq!(msgs.len(), 1, "one message for the run, none per shard: {msgs:?}");
    assert!(msgs[0].starts_with(&format!("[compute] job {} (3 shards): exit 0", &pid[..8])), "{msgs:?}");
    let v = job(port, &pid).await;
    for x in v["result"]["shards"].as_array().unwrap() {
        let r = get(port, "/cli/compute/jobs/logs", &agent, &format!("job={}&cursor=0", x["job"].as_str().unwrap())).await;
        let text: String = j(&r)["chunks"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["stream"] == "out")
            .map(|c| String::from_utf8(k2_node_proto::crypto::unb64(c["data"].as_str().unwrap()).unwrap()).unwrap())
            .collect();
        assert_eq!(text, format!("shard {} of 3\n", x["index"]), "placeholders filled per shard");
    }
    let _ = (a_id, b_id, c_id);
    k2_core::compute::retry::set_tuning_for_tests(Default::default());
    drop(env);
}

// ── know before you run: --plan (G5) ─────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn plan_shows_node_queue_warmth_estimate_and_grant_without_running() {
    let env = setup();
    k2_core::compute::retry::set_tuning_for_tests(tuning_for_tests());
    let d = test_harness::start(OWNER).await;
    let port = d.port;
    std::fs::create_dir_all(env.home.join("cli-tmp")).unwrap();
    let ws = seed_ws(&env, "pl", &[]);
    let (_p1, _) = online_node(&env, port, "p1", 1).await;
    let r = post(port, "/cli/compute/agents/set", OWNER, json!({"workspace": ws.id, "value": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let (agent, _) = passport(&ws);

    // History: the same command three times (with source: a warm slot).
    for i in 0..3 {
        let r = post(port, "/cli/compute/run", &agent, json!({"node": "p1", "argv": ["sh", "-c", "sleep 1"], "clientJobId": format!("client-plan-hist-{i:04}")})).await;
        assert_eq!(r.status, 200, "{}", r.body);
        let id = j(&r)["job"]["id"].as_str().unwrap().to_string();
        wait_state(port, &id, 30, |v| v["state"] == "done").await;
    }
    let jobs_now = |port: u16, ws: String| async move {
        let r = get(port, "/cli/compute/jobs", OWNER, &format!("workspace={ws}&limit=100")).await;
        j(&r)["jobs"].as_array().unwrap().len()
    };
    assert_eq!(jobs_now(port, ws.id.clone()).await, 3);

    // Idle node: starts now, warm at this commit, an estimate from those runs.
    let r = post(port, "/cli/compute/run", &agent, json!({"any": true, "argv": ["sh", "-c", "sleep 1"], "plan": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = j(&r);
    assert_eq!((v["plan"].as_bool(), v["node"].as_str(), v["startsNow"].as_bool()), (Some(true), Some("p1"), Some(true)), "{v}");
    assert!(v["warmSlot"].as_str().unwrap().starts_with("warm"), "{v}");
    assert_eq!((v["estimate"]["samples"].as_u64(), v["estimate"]["basis"].as_str()), (Some(3), Some("this command on this node")), "{v}");
    assert!((1..=3).contains(&v["estimate"]["medianSecs"].as_i64().unwrap()), "{v}");
    assert_eq!((v["grant"]["parallel"]["left"].as_i64(), v["grant"]["queued"]["left"].as_i64()), (Some(1), Some(10)), "{v}");
    assert_eq!(v["limits"]["retries"], 1);

    // Busy node: it would wait, and says on what.
    let long = post(port, "/cli/compute/run", &agent, json!({"node": "p1", "argv": ["sleep", "20"], "noSource": true, "clientJobId": "client-plan-long-0001"})).await;
    let long = j(&long)["job"]["id"].as_str().unwrap().to_string();
    wait_state(port, &long, 20, |v| v["state"] == "running").await;
    let r = post(port, "/cli/compute/run", &agent, json!({"node": "p1", "argv": ["true"], "noSource": true, "plan": true})).await;
    let v = j(&r);
    assert_eq!(v["startsNow"], false, "{v}");
    assert_eq!(v["grant"]["parallel"]["left"], 0, "{v}");
    assert!(v["estimate"].is_null(), "no history for this command: {v}");

    // The CLI: JSON on stdout, a summary line on stderr, exit 0; nothing stored.
    let (rc, out, err) = k2(&env, port, &ws.path, &["compute", "run", "p1", "--plan", "--no-source", "--", "sh", "-c", "sleep 1"]);
    assert_eq!(rc, 0, "{err}");
    let p: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(p["plan"], true);
    assert!(err.contains("[k2 compute] plan: waits (") && err.contains("on p1"), "{err}");
    assert_eq!(jobs_now(port, ws.id.clone()).await, 4, "a plan stores nothing");

    // The same refusals as run: another workspace without the switch.
    let other = seed_ws(&env, "pl2", &[]);
    let (agent2, _) = passport(&other);
    let r = post(port, "/cli/compute/run", &agent2, json!({"any": true, "argv": ["true"], "plan": true})).await;
    assert_eq!((r.status, code(&r)), (403, "compute_off".to_string()), "{}", r.body);
    cancel(port, &long).await;
    k2_core::compute::retry::set_tuning_for_tests(Default::default());
    drop(env);
}
