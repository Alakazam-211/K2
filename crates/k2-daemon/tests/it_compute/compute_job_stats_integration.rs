//! prd-compute-blacksmith-class-v1 O2 (§4.5.1, §6): job stats series on
//! the controller, end to end: the real dispatcher in-process (temp
//! `$HOME`, loopback), compute and the CI preview on, a fake node session
//! (`compute_ws::test_attach` / `test_frame`) offering `job-stats`.
//!
//! Covers: a job's end carrying `Outputs.stats` lands in
//! `compute_job_stats` (migration 0296), not in `summary_json`;
//! `GET /cli/compute/stats` shows the series, peaks, IO totals, the held
//! time with "try --cpus N+M", and OOM kills; a job with no series (an
//! older node, or not ended) says why; an agent reads only its own
//! workspace's jobs; dark while the CI preview is off.

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::path::PathBuf;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_core::compute::proto::frames::{Availability, Caps, Control, ExitInfo, Frame, Free, JobState, Offer, StateUpdate};
use k2_core::compute::proto::gens::{JobStats, StatPoint};
use k2_core::compute::proto::results::Outputs;
use k2_core::compute::store;
use k2_core::session::SessionId;
use k2_daemon::compute_ws;
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;
use rusqlite::params;
use serde_json::{json, Value};

static TEST_LOCK: StdMutex<()> = StdMutex::new(());
const OWNER: &str = "owner-token-job-stats-v1";
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
        std::env::remove_var("K2_CI");
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn setup() -> Env {
    let guard = k2_core::test_env::serial(&TEST_LOCK);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let home = k2_core::test_env::short_tmp(format!("k2js-{:x}", nanos % 0xffff_ffff));
    std::fs::create_dir_all(&home).unwrap();
    let home = dunce::canonicalize(&home).unwrap();
    let env = Env { prev_home: std::env::var_os("HOME"), home: home.clone(), _guard: guard };
    std::env::set_var("HOME", &home);
    std::env::set_var("K2_COMPUTE", "1");
    std::env::set_var("K2_CI", "1");
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
        Some(b) => format!("{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}", b.len()),
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

fn code(r: &Resp) -> String {
    j(r)["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {}", r.body)).to_string()
}

fn seed_ws(env: &Env, handle: &str) -> String {
    let path = env.home.join(format!("ws-{handle}"));
    std::fs::create_dir_all(&path).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)", params![id, handle, path.to_string_lossy()]).unwrap();
    conn.execute("UPDATE projects SET agents_can_use_compute = 1 WHERE id = ?1", params![id]).unwrap();
    id
}

fn passport(ws_id: &str) -> String {
    let sid = SessionId::new();
    k2_daemon::session_token::mint_session_token(&sid, &sid.to_string(), HookPrincipal { workspace_uuid: ws_id.to_string(), agent_address: "agent".into() }, CredMode::ApiKey, Provider::Anthropic)
}

fn active_node(id: &str, name: &str, ws_id: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let labels = std::collections::BTreeMap::from([("os".to_string(), "linux".to_string())]);
    store::insert_pending_node(&conn, id, name, &format!("{id:0>64}"), "pem", &labels, "owner", "123456", 1).expect("node");
    store::confirm_node(&conn, id, "owner", 2).expect("confirm");
    store::set_grant(&conn, id, ws_id, &store::GrantLimits::default(), "owner", 3).expect("grant");
}

fn offer() -> Offer {
    Offer {
        share: None,
        revision: 1,
        control: Control::Active,
        control_by: None,
        control_at: None,
        availability: Availability { ok: true, reasons: vec![] },
        caps: Caps { cpu_millis: 16_000, mem_bytes: 32 * GB, disk_budget_bytes: 100 * GB, hard: true, gpus: 0 },
        free: Free { cpu_millis: 16_000, mem_bytes: 32 * GB, disk_free_bytes: 500 * GB, gpus: 0 },
        slots: vec![],
        running: 0,
        parallel_max: 4,
        refusal: None,
        labels: [("os".to_string(), "linux".to_string())].into(),
        tools: [("git".to_string(), "2.43.0".to_string())].into(),
        protocol: 10,
        boot_id: "b".into(),
        ledger_id: "l".into(),
        node_version: "0.46.13".into(),
        features: ["artifacts", "stall", k2_core::compute::proto::gens::FEATURE_JOB_STATS].iter().map(|s| s.to_string()).collect(),
        host: None,
        pkg: None,
        sim: None,
    }
}

fn row(job: &str) -> store::JobRow {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::job_by_id(&conn, job).expect("job row")
}

fn state(job: &str, generation: u32, st: JobState, at: i64, outputs: Option<Outputs>) -> Frame {
    Frame::State(StateUpdate {
        queue: None,
        job_id: job.into(),
        generation,
        state: st,
        reason: None,
        detail: None,
        at,
        exit: (st == JobState::Done).then_some(ExitInfo { code: Some(0), signal: None }),
        src: None,
        log_seq: 0,
        outputs,
    })
}

/// 72 points (6 minutes) at 3.9 of 4 CPUs, 1200 throttled periods (120 s
/// held), 1G read and 2G written by the end, no OOM kill.
fn series() -> JobStats {
    JobStats {
        every_secs: 5,
        points: (1..=72u32)
            .map(|i| StatPoint { t: i * 5, cpu_milli: 3900, mem_bytes: u64::from(i) * (GB / 8), io_read: Some(u64::from(i) * GB / 72), io_write: Some(u64::from(i) * 2 * GB / 72), load1_milli: 6000 })
            .collect(),
        nr_throttled: Some(1200),
        throttled_usec: Some(110_000_000),
        oom_events: Some(0),
    }
}

async fn submit(port: u16, token: &str, ws: &str, cpus: u64) -> String {
    let body = json!({"workspace": ws, "node": "rig7", "argv": ["cargo", "build"], "noSource": true, "memBytes": 16 * GB, "cpus": cpus, "clientJobId": uuid::Uuid::new_v4().simple().to_string()});
    let r = post(port, "/cli/compute/run", token, body).await;
    assert_eq!(r.status, 200, "{}", r.body);
    j(&r)["job"]["id"].as_str().unwrap().to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_jobs_series_is_kept_apart_and_compute_stats_reads_it() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "jsapp");
    let other = seed_ws(&env, "jsother");
    active_node("js-1", "rig7", &ws);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::set_grant(&conn, "js-1", &other, &store::GrantLimits::default(), "owner", 4).expect("grant");
    }
    let _node = compute_ws::test_attach("js-1", offer());
    let agent = passport(&ws);
    let now = k2_core::compute::now();

    // A job ends with its series: kept in its own table, not the summary.
    let id = submit(d.port, &agent, &ws, 4).await;
    let generation = row(&id).generation as u32;
    compute_ws::test_frame("js-1", state(&id, generation, JobState::Running, now - 360, None));
    let out = Outputs { stats: Some(series()), ..Default::default() };
    compute_ws::test_frame("js-1", state(&id, generation, JobState::Done, now, Some(out)));
    {
        // A 6-minute run (the node's own times need its clock checked by a
        // health frame; this test sets them directly).
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE compute_jobs SET started_at = ?2, ended_at = ?3 WHERE id = ?1", params![id, now - 360, now]).unwrap();
    }
    let r = row(&id);
    assert_eq!(r.state, "done");
    let summary = r.summary_json.expect("a summary");
    assert!(!summary.contains("\"stats\"") && !summary.contains("cpu_milli"), "the series isn't in summary_json: {summary}");
    let kept: (i64, i64) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row("SELECT points, every_secs FROM compute_job_stats WHERE job_id = ?1", params![id], |x| Ok((x.get(0)?, x.get(1)?))).expect("a 0296 row")
    };
    assert_eq!(kept, (72, 5));

    // k2 compute stats: peaks, IO, held time and what to ask for.
    let s = get(d.port, "/cli/compute/stats", &agent, &format!("job={id}")).await;
    assert_eq!(s.status, 200, "{}", s.body);
    let v = j(&s);
    assert_eq!((v["node"].as_str(), v["wallSecs"].as_i64(), v["cpus"]["guaranteed"].as_f64()), (Some("rig7"), Some(360), Some(4.0)), "{v}");
    let st = &v["stats"];
    assert_eq!(st["points"].as_array().map(Vec::len), Some(72));
    assert_eq!((st["peaks"]["cpus"].as_f64(), st["peaks"]["memBytes"].as_u64()), (Some(3.9), Some(9 * GB)));
    assert_eq!(st["io"], json!({"readBytes": GB, "writeBytes": 2 * GB}));
    assert_eq!(st["throttle"]["heldPct"], json!(33));
    assert_eq!(st["throttle"]["hint"], json!("held to its --cpus 4 33% of the time: try --cpus 4+8"));
    assert_eq!(st["oomEvents"], json!(0));
    assert!(st["notes"].as_array().unwrap().iter().any(|n| n.as_str().unwrap().contains("network")), "{st}");

    // No series (an older node, or still running): stats is null, and why.
    let plain = submit(d.port, &agent, &ws, 2).await;
    let s = get(d.port, "/cli/compute/stats", &agent, &format!("job={plain}")).await;
    assert_eq!(s.status, 200, "{}", s.body);
    assert_eq!(j(&s)["stats"], Value::Null);
    assert!(j(&s)["why"].as_str().unwrap().starts_with("it hasn't ended yet"), "{}", s.body);
    let g = row(&plain).generation as u32;
    compute_ws::test_frame("js-1", state(&plain, g, JobState::Running, now - 10, None));
    compute_ws::test_frame("js-1", state(&plain, g, JobState::Done, now, Some(Outputs::default())));
    let s = get(d.port, "/cli/compute/stats", &agent, &format!("job={plain}")).await;
    assert!(j(&s)["why"].as_str().unwrap().contains("older than protocol 10 (k2 compute node update rig7)"), "{}", s.body);

    // Another workspace's agent can't read it; no job named is a usage error.
    let r = get(d.port, "/cli/compute/stats", &passport(&other), &format!("job={id}")).await;
    assert_eq!((r.status, code(&r).as_str()), (404, "job_not_found"), "{}", r.body);
    let r = get(d.port, "/cli/compute/stats", &agent, "").await;
    assert_eq!(r.status, 400, "{}", r.body);
    // The owner reads any job.
    assert_eq!(get(d.port, "/cli/compute/stats", OWNER, &format!("job={id}")).await.status, 200);

    // Dark while the CI preview is off.
    std::env::set_var("K2_CI", "off");
    let r = get(d.port, "/cli/compute/stats", &agent, &format!("job={id}")).await;
    assert_eq!((r.status, code(&r).as_str()), (409, "ci_preview_off"), "{}", r.body);
}
