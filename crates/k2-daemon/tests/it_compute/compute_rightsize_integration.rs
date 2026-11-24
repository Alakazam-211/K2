//! prd-compute-blacksmith-class-v1 R1 (§4.4.2–§4.4.3, §10): `k2 compute
//! rightsize` and `--mem auto[:G]`, end to end on the controller: the real
//! dispatcher in-process (temp `$HOME`, loopback), compute and the CI
//! preview on, one fake node (32G cap) granted to an agent's workspace,
//! and history written as `compute_jobs` rows (what a node's results
//! leave).
//!
//! Covers: no history and no fallback is `mem_auto_no_history` (400 →
//! exit 2); a fallback is used until there is history; the plan carries a
//! plain number and `effective.mem` says where it came from; after a
//! `mem_cap` at a 16G peak the next auto value is at least 24G; a value over
//! the largest node's cap is capped there; a size and auto together are
//! refused; `rightsize` reads only this workspace's runs of the same key;
//! both are dark while the CI preview is off.

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::path::PathBuf;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_core::compute::proto::frames::{Availability, Caps, Control, Free, Offer};
use k2_core::compute::store;
use k2_core::session::SessionId;
use k2_daemon::compute_ws;
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;
use rusqlite::params;
use serde_json::{json, Value};

static TEST_LOCK: StdMutex<()> = StdMutex::new(());
const OWNER: &str = "owner-token-rightsize-v1";
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
    let home = k2_core::test_env::short_tmp(format!("k2rs-{:x}", nanos % 0xffff_ffff));
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
        caps: Caps { cpu_millis: 8000, mem_bytes: 32 * GB, disk_budget_bytes: 100 * GB, hard: true, gpus: 0 },
        free: Free { cpu_millis: 8000, mem_bytes: 32 * GB, disk_free_bytes: 500 * GB, gpus: 0 },
        slots: vec![],
        running: 0,
        parallel_max: 4,
        refusal: None,
        labels: [("os".to_string(), "linux".to_string())].into(),
        tools: [("git".to_string(), "2.43.0".to_string())].into(),
        protocol: 8,
        boot_id: "b".into(),
        ledger_id: "l".into(),
        node_version: "0.46.9".into(),
        features: vec!["artifacts".into(), "stall".into()],
        host: None,
        pkg: None,
        sim: None,
    }
}

/// One ended run of `argv` in `ws` (what a node's result leaves): `peak` and
/// CPU time in `summary_json`, `--mem`/`--cpus` in the plan's limits.
#[allow(clippy::too_many_arguments)]
fn history(id: &str, ws: &str, argv: &[&str], tag: Option<&str>, state: &str, reason: Option<&str>, peak: Option<u64>, ended: i64) {
    let mut plan = json!({"limits": {"max_secs": 3600, "cpu_millis": 4000, "mem_bytes": 16 * GB, "disk_bytes": 0, "log_cap_bytes": 0}});
    if let Some(t) = tag {
        plan["k2ctl"] = json!({"tag": t});
    }
    let summary = peak.map(|p| json!({"artifacts": [], "resources": {"peak_mem_bytes": p, "mem_source": "cgroup", "cpu_ms": 300_000, "cpu_source": "cgroup"}}).to_string());
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO compute_jobs (id, client_job_id, node_id, workspace_id, requested_by, argv_json, env_names_json, request_sha256, plan_json, \
         state, reason, plan_digest, created_at, started_at, ended_at, summary_json) VALUES (?1, ?1, 'rs-1', ?2, 'owner', ?3, '[]', '', ?4, ?5, ?6, '', ?7, ?7, ?8, ?9)",
        params![id, ws, serde_json::to_string(argv).unwrap(), plan.to_string(), state, reason, ended - 100, ended, summary],
    )
    .unwrap();
}

fn plan_body(argv: &[&str], extra: Value) -> Value {
    let mut b = json!({"argv": argv, "noSource": true, "plan": true});
    for (k, v) in extra.as_object().unwrap() {
        b[k] = v.clone();
    }
    b
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mem_auto_resolves_from_history_with_the_mem_cap_floor_and_the_node_cap() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "rsapp");
    let other = seed_ws(&env, "rsother");
    active_node("rs-1", "rig7", &ws);
    let _node = compute_ws::test_attach("rs-1", offer());
    let agent = passport(&ws);
    let cmd = ["cargo", "nextest", "run"];
    let now = k2_core::compute::now();

    // No history, no fallback: mem_auto_no_history (exit 2), with the fix.
    let r = post(d.port, "/cli/compute/run", &agent, plan_body(&cmd, json!({"memAuto": true}))).await;
    assert_eq!((r.status, code(&r).as_str()), (400, "mem_auto_no_history"), "{}", r.body);
    assert!(j(&r)["error"]["hint"].as_str().unwrap().contains("--mem auto:8G"), "{}", r.body);
    // A size and auto together: refused.
    let r = post(d.port, "/cli/compute/run", &agent, plan_body(&cmd, json!({"memAuto": true, "memBytes": GB}))).await;
    assert_eq!((r.status, code(&r).as_str()), (400, "bad_request"), "{}", r.body);

    // The fallback until there is history; effective.mem says so.
    let r = post(d.port, "/cli/compute/run", &agent, plan_body(&cmd, json!({"memAuto": true, "memAutoFallbackBytes": 8 * GB}))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let e = j(&r)["effective"]["mem"].clone();
    assert_eq!((e["value"].as_str(), e["bytes"].as_u64()), (Some("8G"), Some(8 * GB)), "{e}");
    assert_eq!(e["from"], "auto (no history yet: the fallback 8G)", "{e}");
    assert!(j(&r)["warnings"].as_array().unwrap().iter().any(|w| w.as_str().unwrap().starts_with("--mem auto: 8G")), "{}", r.body);

    // History: finished runs peaking 8–10G (another workspace's 30G runs
    // and another command's never count).
    history("h1", &ws, &cmd, None, "done", None, Some(8 * GB), now - 3000);
    history("h2", &ws, &cmd, None, "done", None, Some(9 * GB), now - 2000);
    history("h3", &ws, &cmd, None, "done", None, Some(10 * GB), now - 1000);
    history("o1", &other, &cmd, None, "done", None, Some(30 * GB), now - 500);
    history("o2", &ws, &["make"], None, "done", None, Some(30 * GB), now - 500);
    let r = post(d.port, "/cli/compute/run", &agent, plan_body(&cmd, json!({"memAuto": true}))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let e = j(&r)["effective"]["mem"].clone();
    // p95 of 8, 9, 10 = 10G; × 1.25 = 12.5 → 13G.
    assert_eq!((e["value"].as_str(), e["bytes"].as_u64()), (Some("13G"), Some(13 * GB)), "{e}");
    assert_eq!(e["from"], "auto (p95 10G of 3 runs)");

    // §10 R1: after a mem_cap at a 16G peak, the next value is ≥ 24G.
    history("k1", &ws, &cmd, None, "failed", Some("mem_cap"), Some(16 * GB), now - 100);
    let r = post(d.port, "/cli/compute/run", &agent, plan_body(&cmd, json!({"memAuto": true, "memAutoFallbackBytes": 4 * GB}))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let e = j(&r)["effective"]["mem"].clone();
    assert!(e["bytes"].as_u64().unwrap() >= 24 * GB, "{e}");
    assert_eq!(e["value"], "24G");
    assert!(e["from"].as_str().unwrap().contains("1.5 × the 16G peak of a mem_cap end"), "{e}");

    // Over the largest node's cap: capped there (32G), and said.
    history("k2", &ws, &cmd, None, "failed", Some("mem_cap"), Some(30 * GB), now - 50);
    let r = post(d.port, "/cli/compute/run", &agent, plan_body(&cmd, json!({"memAuto": true}))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let e = j(&r)["effective"]["mem"].clone();
    assert_eq!((e["value"].as_str(), e["bytes"].as_u64()), (Some("32G"), Some(32 * GB)), "{e}");
    assert!(e["from"].as_str().unwrap().ends_with("capped at the largest node's 32G per job"), "{e}");

    // A real submit: the stored plan carries the plain number.
    let mut body = plan_body(&cmd, json!({"memAuto": true, "clientJobId": "client-rightsize-auto-0001"}));
    body.as_object_mut().unwrap().remove("plan");
    let r = post(d.port, "/cli/compute/run", &agent, body).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let id = j(&r)["job"]["id"].as_str().unwrap().to_string();
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::job_by_id(&conn, &id).expect("row")
    };
    assert_eq!(row.plan().unwrap().limits.mem_bytes, Some(32 * GB));
    assert!(!row.ctl.mem_defaulted);
    assert_eq!(row.ctl.effective.as_ref().unwrap()["mem"]["value"], "32G");

    // A CI check's history is its tag: ci:<check>.
    history("c1", &ws, &["anything"], Some("ci:unit"), "done", None, Some(2 * GB), now - 10);
    let r = post(d.port, "/cli/compute/run", &agent, plan_body(&["other", "argv"], json!({"memAuto": true, "tag": "ci:unit"}))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(j(&r)["effective"]["mem"]["value"], "3G", "2G × 1.25 = 2.5 → 3G: {}", r.body);

    // rightsize: the same numbers, this workspace only.
    let q = format!("argv={}", enc(&serde_json::to_string(&cmd).unwrap()));
    let r = get(d.port, "/cli/compute/rightsize", &agent, &q).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = j(&r);
    assert_eq!((v["runs"].as_u64(), v["finished"].as_u64(), v["confidence"].as_str()), (Some(5), Some(3), Some("low")), "{v}");
    assert_eq!((v["mem"]["p95Bytes"].as_u64(), v["mem"]["maxBytes"].as_u64(), v["mem"]["memCapEnds"].as_u64()), (Some(10 * GB), Some(10 * GB), Some(2)), "{v}");
    assert_eq!((v["mem"]["lastMemCapPeakBytes"].as_u64(), v["mem"]["suggest"].as_str()), (Some(30 * GB), Some("--mem 45G")), "{v}");
    assert_eq!(v["key"]["kind"], "argv");
    // 300 CPU-seconds over 100 s on --cpus 4: about 3 of 4 cores.
    assert!((v["cpus"]["avgUsed"].as_f64().unwrap() - 3.0).abs() < 1e-9, "{v}");
    let r = get(d.port, "/cli/compute/rightsize", &agent, "check=unit").await;
    assert_eq!((j(&r)["key"]["kind"].as_str(), j(&r)["runs"].as_u64()), (Some("check"), Some(1)), "{}", r.body);
    // Exactly one key; another workspace is refused.
    let r = get(d.port, "/cli/compute/rightsize", &agent, "check=unit&tag=x").await;
    assert_eq!((r.status, code(&r).as_str()), (400, "bad_request"), "{}", r.body);
    let r = get(d.port, "/cli/compute/rightsize", &agent, "").await;
    assert_eq!(r.status, 400, "{}", r.body);
    let r = get(d.port, "/cli/compute/rightsize", &agent, &format!("check=unit&workspace={other}")).await;
    assert_eq!((r.status, code(&r).as_str()), (403, "forbidden"), "{}", r.body);
    // The owner names the workspace.
    let r = get(d.port, "/cli/compute/rightsize", OWNER, &format!("check=unit&workspace={ws}")).await;
    assert_eq!(r.status, 200, "{}", r.body);

    // Dark while the CI preview is off.
    std::env::set_var("K2_CI", "off");
    let r = get(d.port, "/cli/compute/rightsize", &agent, "check=unit").await;
    assert_eq!((r.status, code(&r).as_str()), (409, "ci_preview_off"), "{}", r.body);
    let r = post(d.port, "/cli/compute/run", &agent, plan_body(&cmd, json!({"memAuto": true}))).await;
    assert_eq!((r.status, code(&r).as_str()), (409, "ci_preview_off"), "{}", r.body);
    // A plain run is untouched.
    let r = post(d.port, "/cli/compute/run", &agent, plan_body(&cmd, json!({"memBytes": GB}))).await;
    assert_eq!(r.status, 200, "{}", r.body);
}
