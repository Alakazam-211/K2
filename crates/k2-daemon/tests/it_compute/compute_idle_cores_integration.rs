//! prd-compute-blacksmith-class-v1 R2 (§4.4.4, §10, BK-13): `--cpus N+M`
//! on the controller, end to end: the real dispatcher in-process (temp
//! `$HOME`, loopback), compute and the CI preview on, and two fake node
//! sessions (`compute_ws::test_attach`): one offering `cpu-burst`
//! (protocol 10), one older (protocol 9, no feature).
//!
//! Covers: the body rules (M needs N and must be above it); the plan to a
//! `cpu-burst` node carries `cpus_burst` with the guarantee still in
//! `limits.cpu_millis` (what placement and the node's admission count);
//! an older node gets today's plan (no `cpus_burst` key) and the run's
//! note says it runs held to N; `effective.cpus` says `N+M`; it is dark
//! while the CI preview is off, and a plain `--cpus` is untouched.

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::path::PathBuf;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_core::compute::proto::frames::{Assign, Availability, Caps, Control, ExitInfo, Frame, Free, JobState, Offer, StateUpdate};
use k2_core::compute::store;
use k2_daemon::compute_ws;
use k2_daemon::test_harness;
use rusqlite::params;
use serde_json::{json, Value};

static TEST_LOCK: StdMutex<()> = StdMutex::new(());
const OWNER: &str = "owner-token-idle-cores-v1";
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
    let home = k2_core::test_env::short_tmp(format!("k2ic-{:x}", nanos % 0xffff_ffff));
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

async fn post(port: u16, path: &str, body: Value) -> Resp {
    let pq = format!("{path}?token={OWNER}");
    tokio::task::spawn_blocking(move || req_blocking(port, "POST", &pq, Some(body.to_string()))).await.unwrap()
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

fn active_node(id: &str, name: &str, ws_id: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let labels = std::collections::BTreeMap::from([("os".to_string(), "linux".to_string())]);
    store::insert_pending_node(&conn, id, name, &format!("{id:0>64}"), "pem", &labels, "owner", "123456", 1).expect("node");
    store::confirm_node(&conn, id, "owner", 2).expect("confirm");
    store::set_grant(&conn, id, ws_id, &store::GrantLimits::default(), "owner", 3).expect("grant");
}

fn offer(protocol: u32, features: &[&str]) -> Offer {
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
        protocol,
        boot_id: "b".into(),
        ledger_id: "l".into(),
        node_version: "0.46.13".into(),
        features: features.iter().map(|s| s.to_string()).collect(),
        host: None,
        pkg: None,
        sim: None,
    }
}

async fn submit(port: u16, ws: &str, extra: Value) -> Resp {
    let mut body = json!({"workspace": ws, "argv": ["true"], "noSource": true, "memBytes": GB, "clientJobId": uuid::Uuid::new_v4().simple().to_string()});
    for (k, v) in extra.as_object().unwrap() {
        body[k] = v.clone();
    }
    post(port, "/cli/compute/run", body).await
}

fn assigned(frames: &[Frame], job: &str) -> Option<Assign> {
    frames.iter().find_map(|f| match f {
        Frame::Assign(a) if a.fence.job_id == job => Some(a.clone()),
        _ => None,
    })
}

fn warnings(r: &Resp) -> Vec<String> {
    j(r)["warnings"].as_array().map(|a| a.iter().filter_map(|w| w.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cpus_n_plus_m_reaches_only_burst_nodes_and_admission_counts_n() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "icapp");
    active_node("ic-new", "rig7", &ws);
    active_node("ic-old", "rig4", &ws);
    let burst = k2_core::compute::proto::gens::FEATURE_CPU_BURST;
    let mut new = compute_ws::test_attach("ic-new", offer(10, &["artifacts", "stall", burst]));
    let mut old = compute_ws::test_attach("ic-old", offer(9, &["artifacts", "stall"]));

    // Body rules (BK-13): M needs N, and is above it.
    let r = submit(d.port, &ws, json!({"node": "rig7", "cpusBurst": 8})).await;
    assert_eq!((r.status, code(&r).as_str()), (400, "bad_request"), "{}", r.body);
    assert!(r.body.contains("needs N"), "{}", r.body);
    let r = submit(d.port, &ws, json!({"node": "rig7", "cpus": 4, "cpusBurst": 4})).await;
    assert_eq!((r.status, code(&r).as_str()), (400, "bad_request"), "{}", r.body);
    assert!(new.frames().is_empty() && old.frames().is_empty(), "nothing sent for a refused body");

    // To the cpu-burst node: the burst rides the plan, the guarantee stays
    // the limit placement and admission count (§10 R2).
    let r = submit(d.port, &ws, json!({"node": "rig7", "cpus": 2, "cpusBurst": 8})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(!warnings(&r).iter().any(|w| w.contains("idle cores")), "{}", r.body);
    let id = j(&r)["job"]["id"].as_str().unwrap().to_string();
    let a = assigned(&new.frames(), &id).expect("assigned to the cpu-burst node");
    assert_eq!((a.plan.limits.cpu_millis, a.plan.cpus_burst), (Some(2000), Some(8000)));
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::job_by_id(&conn, &id).expect("row")
    };
    assert_eq!(row.ctl.cpus_burst, Some(8000));
    // `get --json` shows it as result.effective.cpus.
    assert_eq!(row.ctl.effective.as_ref().expect("effective")["cpus"]["value"], "2+8", "{:?}", row.ctl.effective);
    // --plan says the same before anything is stored.
    let r = submit(d.port, &ws, json!({"node": "rig7", "cpus": 2, "cpusBurst": 8, "plan": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(j(&r)["effective"]["cpus"]["value"], "2+8", "{}", r.body);
    assert_eq!(row.plan_digest, a.plan.digest().unwrap(), "the burst is in the digest the node checks");

    // To the older node: today's plan (no key at all), held to 2, said.
    let r = submit(d.port, &ws, json!({"node": "rig4", "cpus": 2, "cpusBurst": 8})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(warnings(&r).iter().any(|w| w.contains("--cpus 2+8: rig4 doesn't take idle cores") && w.contains("held to 2")), "{}", r.body);
    let id = j(&r)["job"]["id"].as_str().unwrap().to_string();
    let a = assigned(&old.frames(), &id).expect("assigned to the older node");
    assert_eq!((a.plan.limits.cpu_millis, a.plan.cpus_burst), (Some(2000), None));
    assert!(!serde_json::to_string(&a.plan).unwrap().contains("cpus_burst"), "an older node's plan bytes");

    // A plain --cpus never carries a burst. (The burst job ends first: the
    // workspace's grant runs one job at a time on a node.)
    for st in [JobState::Running, JobState::Done] {
        compute_ws::test_frame(
            "ic-new",
            Frame::State(StateUpdate {
                queue: None,
                job_id: row.id.clone(),
                generation: row.generation as u32,
                state: st,
                reason: None,
                detail: None,
                at: k2_core::compute::now(),
                exit: (st == JobState::Done).then_some(ExitInfo { code: Some(0), signal: None }),
                src: None,
                log_seq: 0,
                outputs: None,
            }),
        );
    }
    let r = submit(d.port, &ws, json!({"node": "rig7", "cpus": 4})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let id = j(&r)["job"]["id"].as_str().unwrap().to_string();
    assert_eq!(assigned(&new.frames(), &id).expect("assigned").plan.cpus_burst, None);

    // Dark while the CI preview is off; a plain --cpus is untouched.
    std::env::set_var("K2_CI", "off");
    let r = submit(d.port, &ws, json!({"node": "rig7", "cpus": 2, "cpusBurst": 8})).await;
    assert_eq!((r.status, code(&r).as_str()), (409, "ci_preview_off"), "{}", r.body);
    let r = submit(d.port, &ws, json!({"node": "rig7", "cpus": 2})).await;
    assert_eq!(r.status, 200, "{}", r.body);
}
