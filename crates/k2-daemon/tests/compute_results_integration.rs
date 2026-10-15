//! K2 compute, results and reliability (research-distributed-compute-gaps
//! G1–G5, G8): the real dispatcher in-process (temp `$HOME`, loopback)
//! with REAL `k2-node` runtimes over loopback WebSockets, plus the real
//! `cli/k2` for the agent-facing commands.
//!
//! - results back: `--artifact` globs, the result envelope on
//!   `get --json` / `run --json`, `fetch`, JUnit failures first in
//!   `logs --failures`, test counts in the wake message;

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
