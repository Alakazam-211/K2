//! A job's earlier attempts stay readable: when K2 retries or moves a job
//! on its own, `k2 compute get` says per attempt where it ran, when, how it
//! ended and why (in plain words), and attempt N's log and `--artifact`
//! files stay readable as attempt N (`logs --attempt N`, `fetch --attempt
//! N`). An attempt whose node was offline when it ended says its files
//! weren't collected, and picks them up if the node reports them later.
//! A grant change never stops or re-sends a job that already started.
//!
//! The real dispatcher in-process (temp `$HOME`, loopback) and fake node
//! sessions (`compute_ws::test_attach` / `test_frame`).

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::path::PathBuf;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_core::compute::proto::frames::{Availability, Caps, Control, Frame, Free, JobState, LogChunk, LogStream, Offer, StateUpdate};
use k2_core::compute::proto::results::{ArtifactInfo, Outputs};
use k2_core::compute::proto::share::{QueuePos, Role, ServerStatus, ShareLimits, ShareView, Trust, FEATURE_QUEUE, FEATURE_SERVERS};
use k2_core::compute::store;
use k2_core::session::SessionId;
use k2_daemon::compute_ws::{self, TestConn};
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;
use rusqlite::params;
use serde_json::{json, Value};

static TEST_LOCK: StdMutex<()> = StdMutex::new(());
const OWNER: &str = "owner-token-compute-attempts-v1";
const GB: u64 = 1 << 30;

struct Env {
    home: PathBuf,
    prev_home: Option<std::ffi::OsString>,
    _guard: k2_core::test_env::SerialGuard,
}

impl Drop for Env {
    fn drop(&mut self) {
        match self.prev_home.take() {
            Some(p) => std::env::set_var("HOME", p),
            None => std::env::remove_var("HOME"),
        }
        std::env::remove_var("K2_COMPUTE");
        k2_core::compute::retry::set_tuning_for_tests(Default::default());
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn setup() -> Env {
    let guard = k2_core::test_env::serial(&TEST_LOCK);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let home = k2_core::test_env::short_tmp(format!("k2cat-{:x}", nanos % 0xffff_ffff));
    std::fs::create_dir_all(&home).unwrap();
    let home = dunce::canonicalize(&home).unwrap();
    let env = Env { prev_home: std::env::var_os("HOME"), home: home.clone(), _guard: guard };
    std::env::set_var("HOME", &home);
    std::env::set_var("K2_COMPUTE", "1");
    // The ticker sleeps (every pass is called by hand); retries start at
    // once; a node offline at all counts as lost.
    k2_core::compute::retry::set_tuning_for_tests(k2_core::compute::retry::Tuning {
        tick_millis: 3_600_000,
        retry_base_secs: 0,
        node_lost_secs: 0,
        ..Default::default()
    });
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
    let status = text.lines().next().and_then(|l| l.split_whitespace().nth(1)).and_then(|x| x.parse().ok()).unwrap_or_else(|| panic!("no status: {text:?}"));
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    Resp { status, body }
}

fn enc(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

async fn post(port: u16, path: &str, token: &str, body: Value) -> Resp {
    let pq = format!("{path}?token={}", enc(token));
    tokio::task::spawn_blocking(move || req_blocking(port, "POST", &pq, Some(body.to_string()))).await.unwrap()
}

async fn get(port: u16, path: &str, token: &str, query: &str) -> Resp {
    let pq = format!("{path}?token={}&{query}", enc(token));
    tokio::task::spawn_blocking(move || req_blocking(port, "GET", &pq, None)).await.unwrap()
}

fn j(r: &Resp) -> Value {
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("JSON ({e}): {:?}", r.body))
}

struct Ws {
    id: String,
}

fn seed_ws(env: &Env, handle: &str) -> Ws {
    let path = env.home.join(format!("ws-{handle}"));
    std::fs::create_dir_all(&path).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)", params![id, handle, path.to_string_lossy()]).unwrap();
    conn.execute("UPDATE projects SET agents_can_use_compute = 1 WHERE id = ?1", params![id]).unwrap();
    Ws { id }
}

fn passport(ws: &Ws) -> String {
    let sid = SessionId::new();
    k2_daemon::session_token::mint_session_token(
        &sid,
        &sid.to_string(),
        HookPrincipal { workspace_uuid: ws.id.clone(), agent_address: "agent".into() },
        CredMode::ApiKey,
        Provider::Anthropic,
    )
}

fn active_node(id: &str, name: &str, ws: &Ws) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let labels = std::collections::BTreeMap::from([("os".to_string(), "linux".to_string())]);
    store::insert_pending_node(&conn, id, name, &format!("{id:0>64}"), "pem", &labels, "owner", "123456", 1).expect("node");
    store::confirm_node(&conn, id, "owner", 2).expect("confirm");
    let limits = store::GrantLimits { max_parallel: Some(2), ..Default::default() };
    store::set_grant(&conn, id, &ws.id, &limits, "owner", 3).expect("grant");
}

fn offer(share: Option<ShareView>) -> Offer {
    Offer {
        share,
        revision: 1,
        control: Control::Active,
        control_by: None,
        control_at: None,
        availability: Availability { ok: true, reasons: vec![] },
        caps: Caps { cpu_millis: 8000, mem_bytes: 32 * GB, disk_budget_bytes: 100 * GB, hard: true, gpus: 0 },
        free: Free { cpu_millis: 8000, mem_bytes: 30 * GB, disk_free_bytes: 500 * GB, gpus: 0 },
        slots: vec![],
        running: 0,
        parallel_max: 4,
        refusal: None,
        labels: [("os".to_string(), "linux".to_string())].into(),
        tools: [("git".to_string(), "2.47".to_string())].into(),
        protocol: 6,
        boot_id: "b".into(),
        ledger_id: "l".into(),
        node_version: "0.46.6".into(),
        features: ["artifacts", "stall", FEATURE_QUEUE, FEATURE_SERVERS].iter().map(|f| f.to_string()).collect(),
        host: None,
        pkg: None,
        sim: None,
    }
}

fn tenant_view() -> ShareView {
    ShareView {
        role: Role::Tenant,
        slot: 1,
        shared: true,
        servers: 2,
        owner_label: Some("other.example.test".into()),
        state: ServerStatus::Active,
        limits: ShareLimits::new_server(Trust::Own, false),
        running: 0,
        queued: 0,
        cpu_secs_today: 0,
        expires_in_secs: None,
        skew_secs: None,
        disk_used_bytes: None,
        quotes: vec![],
        priority: None,
    }
}

fn row(job: &str) -> store::JobRow {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::job_by_id(&conn, job).expect("job row")
}

fn state(job: &str, generation: u32, st: JobState, reason: Option<&str>, outputs: Option<Outputs>) -> Frame {
    Frame::State(StateUpdate {
        queue: None,
        job_id: job.into(),
        generation,
        state: st,
        reason: reason.map(str::to_string),
        detail: None,
        at: k2_core::compute::now(),
        exit: None,
        src: None,
        log_seq: 0,
        outputs,
    })
}

fn queued_on_node(job: &str, generation: u32) -> Frame {
    Frame::State(StateUpdate {
        queue: Some(QueuePos { pos: 1, of: Some(1), eta_secs: None, eta_p90_secs: None, blocked_by: None }),
        job_id: job.into(),
        generation,
        state: JobState::Queued,
        reason: Some("node_queued".into()),
        detail: None,
        at: k2_core::compute::now(),
        exit: None,
        src: None,
        log_seq: 0,
        outputs: None,
    })
}

fn log(job: &str, generation: u32, seq: u64, text: &str) -> Frame {
    Frame::Log(LogChunk { job_id: job.into(), generation, seq, stream: LogStream::Out, data: k2_core::compute::proto::crypto::b64(text.as_bytes()) })
}

fn verdict_outputs() -> Outputs {
    Outputs { artifacts: vec![ArtifactInfo { name: "verdict.txt".into(), sha256: "ab".repeat(32), bytes: 42 }], ..Default::default() }
}

fn cancels(frames: &[Frame], job: &str) -> Vec<(u32, String)> {
    frames
        .iter()
        .filter_map(|f| match f {
            Frame::Cancel(c) if c.job_id == job => Some((c.generation, c.reason.clone())),
            _ => None,
        })
        .collect()
}

async fn submit(port: u16, token: &str, ws: &Ws, body: Value) -> String {
    let mut b = body;
    b["argv"] = json!(["sh", "-c", "long-soak"]);
    b["noSource"] = json!(true);
    b["workspace"] = json!(ws.id);
    b["memBytes"] = json!(2 * GB);
    b["clientJobId"] = json!(uuid::Uuid::new_v4().simple().to_string());
    let r = post(port, "/cli/compute/run", token, b).await;
    assert_eq!(r.status, 200, "{}", r.body);
    j(&r)["job"]["id"].as_str().unwrap_or_else(|| panic!("no job id: {}", r.body)).to_string()
}

fn text_of(chunks: &Value) -> String {
    let mut out = Vec::new();
    for c in chunks.as_array().unwrap_or_else(|| panic!("chunks: {chunks}")) {
        out.extend(k2_core::compute::proto::crypto::unb64(c["data"].as_str().unwrap()).unwrap());
    }
    String::from_utf8(out).unwrap()
}

fn attempts(v: &Value) -> Vec<Value> {
    v["attempts"].as_array().unwrap_or_else(|| panic!("no attempts list: {v}")).clone()
}

/// A long run on node A is stopped by A's update and retried on node B:
/// `get` names attempt 1 (node, times, `interrupted/node_update` in plain
/// words, retried automatically), its log and its artifacts stay readable
/// as attempt 1, and `logs` without `--attempt` reads attempt 2 and lists
/// attempt 1.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_retried_job_keeps_attempt_ones_log_and_artifacts_and_says_why() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "soaker");
    active_node("att-a", "nodea-vin", &ws);
    active_node("att-b", "nodeb-vin", &ws);
    let mut conns: Vec<(&str, TestConn)> = vec![("att-a", compute_ws::test_attach("att-a", offer(None))), ("att-b", compute_ws::test_attach("att-b", offer(None)))];
    let agent = passport(&ws);
    let job = submit(d.port, &agent, &ws, json!({"retries": 2, "artifacts": ["verdict.txt"]})).await;
    let first = row(&job);
    let (n1, n2) = if first.node_id == "att-a" { ("att-a", "att-b") } else { ("att-b", "att-a") };
    let name1 = if n1 == "att-a" { "nodea-vin" } else { "nodeb-vin" };
    assert_eq!((first.attempt, first.generation), (1, 1));
    compute_ws::test_frame(n1, state(&job, 1, JobState::Running, None, None));
    compute_ws::test_frame(n1, log(&job, 1, 1, "verdict: suite one passed\n"));
    compute_ws::test_frame(n1, log(&job, 1, 2, "verdict: suite two passed\n"));
    compute_ws::test_frame(n1, state(&job, 1, JobState::Interrupted, Some("node_update"), Some(verdict_outputs())));
    let now = row(&job);
    assert_eq!((now.attempt, now.generation, now.node_id.as_str()), (2, 2, n2), "retried on the other node: {now:?}");

    // get: every attempt, the ended one in plain words.
    let r = get(d.port, "/cli/compute/jobs/get", OWNER, &format!("job={job}")).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = j(&r);
    let list = attempts(&v);
    assert_eq!(list.len(), 2, "{list:?}");
    let one = &list[0];
    assert_eq!((one["attempt"].as_i64(), one["generation"].as_i64(), one["node"].as_str()), (Some(1), Some(1), Some(name1)), "{one}");
    assert_eq!((one["state"].as_str(), one["reason"].as_str(), one["current"].as_bool()), (Some("interrupted"), Some("node_update"), Some(false)), "{one}");
    assert!(one["startedAt"].as_i64().is_some() && one["endedAt"].as_i64().is_some(), "start and end: {one}");
    let why = one["why"].as_str().unwrap_or_else(|| panic!("no why: {one}"));
    assert!(why.contains("updat"), "the end in plain words names the node update: {why}");
    assert!(one["next"].as_str().unwrap_or_default().contains("retried automatically"), "{one}");
    assert_eq!(one["log"]["available"].as_bool(), Some(true), "{one}");
    assert!(one["log"]["bytes"].as_u64().unwrap_or(0) > 0, "{one}");
    assert_eq!(one["artifacts"]["status"].as_str(), Some("kept"), "{one}");
    assert_eq!(one["artifacts"]["files"][0]["name"].as_str(), Some("verdict.txt"), "{one}");
    let two = &list[1];
    assert_eq!((two["attempt"].as_i64(), two["generation"].as_i64(), two["current"].as_bool()), (Some(2), Some(2), Some(true)), "{two}");
    assert_eq!(v["result"]["attempts"], v["attempts"], "get --json carries the same list");

    // logs: the latest attempt by default, attempt 1 on ask.
    let r = get(d.port, "/cli/compute/jobs/logs", OWNER, &format!("job={job}&cursor=0")).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let latest = j(&r);
    assert_eq!((latest["attempt"].as_i64(), latest["generation"].as_i64()), (Some(2), Some(2)), "{latest}");
    assert_eq!(text_of(&latest["chunks"]), "", "attempt 2 printed nothing yet");
    assert_eq!(attempts(&latest).len(), 2, "logs lists the attempts too");
    let r = get(d.port, "/cli/compute/jobs/logs", OWNER, &format!("job={job}&attempt=1&cursor=0")).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let old = j(&r);
    assert_eq!((old["attempt"].as_i64(), old["generation"].as_i64(), old["end"].as_bool()), (Some(1), Some(1), Some(true)), "{old}");
    assert_eq!(text_of(&old["chunks"]), "verdict: suite one passed\nverdict: suite two passed\n");
    let r = get(d.port, "/cli/compute/jobs/logs", OWNER, &format!("job={job}&attempt=9")).await;
    assert_eq!((r.status, j(&r)["error"]["code"].as_str()), (404, Some("attempt_not_found")), "{}", r.body);

    // Attempt 1's artifacts: listed as attempt 1, pulled from node A's copy
    // of generation 1.
    let r = get(d.port, "/cli/compute/jobs/artifact", OWNER, &format!("job={job}&attempt=1")).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!((j(&r)["attempt"].as_i64(), j(&r)["artifacts"][0]["name"].as_str()), (Some(1), Some("verdict.txt")), "{}", r.body);
    let r = get(d.port, "/cli/compute/jobs/artifact", OWNER, &format!("job={job}")).await;
    assert_eq!(j(&r)["artifacts"], json!([]), "attempt 2 kept nothing yet: {}", r.body);
    let first_conn = &mut conns.iter_mut().find(|(n, _)| *n == n1).expect("node 1's session").1;
    let _ = first_conn.frames();
    let r = get(d.port, "/cli/compute/jobs/artifact", OWNER, &format!("job={job}&attempt=1&name=verdict.txt&wait=1")).await;
    assert_eq!((r.status, j(&r)["pending"].as_bool()), (200, Some(true)), "{}", r.body);
    let gets: Vec<(String, u32, String)> = first_conn
        .frames()
        .into_iter()
        .filter_map(|f| match f {
            Frame::ArtifactGet(g) => Some((g.job_id, g.generation, g.name)),
            _ => None,
        })
        .collect();
    assert_eq!(gets, vec![(job.clone(), 1, "verdict.txt".to_string())], "the pull asks attempt 1's node for generation 1");
    for n in ["att-a", "att-b"] {
        compute_ws::test_detach(n);
    }
}

/// The node goes offline mid-run: attempt 1 ends `interrupted/node_lost`,
/// is retried elsewhere, and says its `--artifact` files weren't
/// collected. When the node comes back and reports that attempt's end with
/// its files, attempt 1 lists them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lost_nodes_attempt_says_its_artifacts_were_not_collected() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "lostws");
    active_node("lst-a", "losta-vin", &ws);
    active_node("lst-b", "lostb-vin", &ws);
    let _a = compute_ws::test_attach("lst-a", offer(None));
    let _b = compute_ws::test_attach("lst-b", offer(None));
    let agent = passport(&ws);
    let job = submit(d.port, &agent, &ws, json!({"retries": 1, "artifacts": ["verdict.txt"]})).await;
    let n1 = row(&job).node_id;
    compute_ws::test_frame(&n1, state(&job, 1, JobState::Running, None, None));
    compute_ws::test_frame(&n1, log(&job, 1, 1, "verdict: half way\n"));
    compute_ws::test_detach(&n1);
    k2_daemon::compute_place::sweep(k2_core::compute::now() + 5);
    let now = row(&job);
    assert_eq!(now.attempt, 2, "retried after the node was lost: {now:?}");
    let r = get(d.port, "/cli/compute/jobs/get", OWNER, &format!("job={job}")).await;
    let list = attempts(&j(&r));
    let one = &list[0];
    assert_eq!((one["state"].as_str(), one["reason"].as_str()), (Some("interrupted"), Some("node_lost")), "{one}");
    assert!(one["why"].as_str().unwrap_or_default().contains("offline"), "{one}");
    assert_eq!(one["artifacts"]["status"].as_str(), Some("not_collected"), "{one}");
    assert!(one["artifacts"]["note"].as_str().unwrap_or_default().contains("offline"), "{one}");
    assert_eq!(one["log"]["available"].as_bool(), Some(true), "what reached this server stays: {one}");

    // The node comes back and reports attempt 1's end (told to stop it),
    // with its files: attempt 1 lists them now.
    let _a2 = compute_ws::test_attach(&n1, offer(None));
    compute_ws::test_frame(&n1, state(&job, 1, JobState::Cancelled, Some("superseded"), Some(verdict_outputs())));
    let r = get(d.port, "/cli/compute/jobs/get", OWNER, &format!("job={job}")).await;
    let one = attempts(&j(&r))[0].clone();
    assert_eq!(one["artifacts"]["status"].as_str(), Some("kept"), "{one}");
    assert_eq!((one["state"].as_str(), one["reason"].as_str()), (Some("interrupted"), Some("node_lost")), "the end this server recorded stays: {one}");
    assert_eq!(row(&job).attempt, 2, "a late report never touches the current attempt");
    for n in ["lst-a", "lst-b"] {
        compute_ws::test_detach(n);
    }
}

/// A grant change re-sends only jobs still waiting in a shared node's
/// queue (their caps ride with them). A job that already started is never
/// asked back, stopped or given a new generation, even when the node's
/// "started" answer races the re-send. Each re-send shows in `get` as a
/// generation that never started, with the reason in plain words.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_grant_change_never_stops_a_started_job() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "granted");
    active_node("gr-1", "grant-vin", &ws);
    let mut node = compute_ws::test_attach("gr-1", offer(Some(tenant_view())));
    let agent = passport(&ws);
    let running = submit(d.port, &agent, &ws, json!({"node": "grant-vin"})).await;
    let waiting = submit(d.port, &agent, &ws, json!({"node": "grant-vin"})).await;
    let racing = submit(d.port, &agent, &ws, json!({"node": "grant-vin"})).await;
    compute_ws::test_frame("gr-1", queued_on_node(&running, 1));
    compute_ws::test_frame("gr-1", state(&running, 1, JobState::Running, None, None));
    compute_ws::test_frame("gr-1", queued_on_node(&waiting, 1));
    compute_ws::test_frame("gr-1", queued_on_node(&racing, 1));
    let _ = node.frames();

    let r = post(d.port, "/cli/compute/grants/set", OWNER, json!({"node": "grant-vin", "workspace": ws.id, "maxParallel": 3})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(j(&r)["resent"], 2, "only the two waiting jobs: {}", r.body);
    let sent = node.frames();
    assert!(cancels(&sent, &running).is_empty(), "a running job was asked back: {sent:?}");
    // The node had started `racing` already: it answers its real state.
    compute_ws::test_frame("gr-1", state(&racing, 1, JobState::Running, None, None));
    compute_ws::test_frame("gr-1", state(&waiting, 1, JobState::Cancelled, Some("withdrawn"), None));
    for (job, what) in [(&running, "running"), (&racing, "racing")] {
        let rj = row(job);
        assert_eq!((rj.state.as_str(), rj.generation, rj.attempt), ("running", 1, 1), "{what}: {rj:?}");
    }
    let rw = row(&waiting);
    assert_eq!((rw.generation, rw.attempt), (2, 1), "re-sent under a new generation: {rw:?}");
    let r = get(d.port, "/cli/compute/jobs/get", OWNER, &format!("job={waiting}")).await;
    let list = attempts(&j(&r));
    assert_eq!(list.len(), 2, "{list:?}");
    assert_eq!((list[0]["generation"].as_i64(), list[0]["startedAt"].as_i64()), (Some(1), None), "{}", list[0]);
    assert!(list[0]["next"].as_str().unwrap_or_default().contains("new caps"), "{}", list[0]);
    assert_eq!(list[0]["artifacts"]["status"].as_str(), Some("never_started"), "{}", list[0]);
    let r = get(d.port, "/cli/compute/jobs/get", OWNER, &format!("job={running}")).await;
    assert_eq!(attempts(&j(&r)).len(), 1, "a started job has one attempt: {}", r.body);
    compute_ws::test_detach("gr-1");
}
