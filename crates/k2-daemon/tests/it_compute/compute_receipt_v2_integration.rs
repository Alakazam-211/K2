//! Receipt v2 and node self-statements, the controller side
//! (prd-ci-on-compute-for-clients-v1 slice C0, vs-live V4/V7): the real
//! dispatcher in-process (temp `$HOME`, loopback) and fake node sessions
//! (`compute_ws::test_attach` / `test_frame`). Mixed versions:
//! - a protocol-9 server never sends `receipt_v` to a node without
//!   `receipt-v2` (a protocol-8 node): a pinned submit is refused (409), an
//!   auto one never lands there, and a plain job's plan to it has 0.46.9's
//!   bytes (no `receipt_v` key);
//! - a protocol-9 node's statement is kept only when its pinned key signed
//!   it; a v2 receipt verifies only with the statement it names, and
//!   `/cli/compute/jobs/receipt` returns the body, key, plan and statement
//!   for the evidence bundle, while a v1 answer is unchanged.

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::path::PathBuf;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_core::compute::proto::ci::{self, Isolation, RunFacts, Taint, Wall};
use k2_core::compute::proto::crypto::{self, SigningKey};
use k2_core::compute::proto::frames::{
    Availability, Caps, Control, Frame, Free, JobState, Offer, Receipt, ReceiptFrame, ReceiptLog, SignedReceipt, RECEIPT_V1, RECEIPT_V2,
};
use k2_core::compute::proto::statement::{KeyExposure, NodeStatement};
use k2_core::compute::store;
use k2_daemon::compute_ws;
use k2_daemon::test_harness;
use rusqlite::params;
use serde_json::{json, Value};

static TEST_LOCK: StdMutex<()> = StdMutex::new(());
const OWNER: &str = "owner-token-compute-receipt-v2";

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
        k2_core::compute::retry::set_tuning_for_tests(Default::default());
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn setup() -> Env {
    let guard = k2_core::test_env::serial(&TEST_LOCK);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let home = k2_core::test_env::short_tmp(format!("k2crv2-{:x}", nanos % 0xffff_ffff));
    std::fs::create_dir_all(&home).unwrap();
    let home = dunce::canonicalize(&home).unwrap();
    let env = Env { prev_home: std::env::var_os("HOME"), home: home.clone(), _guard: guard };
    std::env::set_var("HOME", &home);
    std::env::set_var("K2_COMPUTE", "1");
    // CI on compute is its own preview (0.46.10 ships it dark).
    std::env::set_var("K2_CI", "1");
    k2_core::compute::retry::set_tuning_for_tests(k2_core::compute::retry::Tuning {
        tick_millis: 3_600_000,
        any_hold_secs: 0,
        retry_base_secs: 0,
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

async fn post(port: u16, path: &str, body: Value) -> Resp {
    let pq = format!("{path}?token={}", enc(OWNER));
    tokio::task::spawn_blocking(move || req_blocking(port, "POST", &pq, Some(body.to_string()))).await.unwrap()
}

async fn get(port: u16, path: &str, query: &str) -> Resp {
    let pq = format!("{path}?token={}&{query}", enc(OWNER));
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

/// An active node pinned to `key`, granted to `ws`.
fn active_node(id: &str, name: &str, key: &SigningKey, ws: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let labels = std::collections::BTreeMap::from([("os".to_string(), "linux".to_string())]);
    store::insert_pending_node(&conn, id, name, &key.fingerprint(), &key.spki_pem(), &labels, "owner", "123456", 1).expect("node");
    store::confirm_node(&conn, id, "owner", 2).expect("confirm");
    store::set_grant(&conn, id, ws, &store::GrantLimits::default(), "owner", 3).expect("grant");
}

fn offer(protocol: u32, features: &[&str]) -> Offer {
    Offer {
        share: None,
        revision: 1,
        control: Control::Active,
        control_by: None,
        control_at: None,
        availability: Availability { ok: true, reasons: vec![] },
        caps: Caps { cpu_millis: 8000, mem_bytes: 32 << 30, disk_budget_bytes: 100 << 30, hard: true, gpus: 0 },
        free: Free { cpu_millis: 8000, mem_bytes: 16 << 30, disk_free_bytes: 500 << 30, gpus: 0 },
        slots: vec![],
        running: 0,
        parallel_max: 4,
        refusal: None,
        labels: [("os".to_string(), "linux".to_string())].into(),
        tools: Default::default(),
        protocol,
        boot_id: "b".into(),
        ledger_id: "l".into(),
        node_version: "0.46.10".into(),
        features: features.iter().map(|f| f.to_string()).collect(),
        host: None,
        pkg: None,
    }
}

fn statement(key: &SigningKey) -> NodeStatement {
    NodeStatement {
        v: 1,
        node_fp: key.fingerprint(),
        node_spki: crypto::b64(key.spki_der()),
        key_epoch: 0,
        key_born_at: 1_700_000_000,
        prev_fp: None,
        issued_at: 1_700_000_100,
        wall: Wall::JobUser,
        dev: false,
        taint: Taint::Clean,
        key_exposure: KeyExposure { store: ci::KeyStore::File, owner: "k2node".into(), mode: Some(0o600), job_readable: false },
        k2node_version: "0.46.10".into(),
        k2node_commit: Some("0123456789abcdef0123456789abcdef01234567".into()),
        protocol: 9,
        os: "linux".into(),
        arch: "x86_64".into(),
        distro: Some("ubuntu".into()),
        distro_version: Some("24.04".into()),
        kernel: None,
        ptrace_scope: Some(1),
        host_id: None,
    }
}

fn row(job: &str) -> store::JobRow {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::job_by_id(&conn, job).expect("job row")
}

fn receipt(key: &SigningKey, r: &store::JobRow, run: Option<RunFacts>) -> Receipt {
    Receipt {
        v: if run.is_some() { RECEIPT_V2 } else { RECEIPT_V1 },
        job_id: r.id.clone(),
        attempt: r.attempt as u32,
        generation: r.generation as u32,
        plan_digest: r.plan_digest.clone(),
        node_fp: key.fingerprint(),
        controller_fp: "c".repeat(64),
        workspace_id: r.workspace_id.clone(),
        argv_sha256: "a".repeat(64),
        env_names: vec![],
        src: None,
        tools: Default::default(),
        boot_id: "b".into(),
        started_at: Some(1),
        ended_at: 2,
        state: JobState::Done,
        exit: None,
        log: ReceiptLog { sha256: "s".into(), bytes: 0, truncated: false },
        artifacts: vec![],
        secret_env: vec![],
        tools_used: Default::default(),
        run,
    }
}

fn run_facts(digest: &str) -> RunFacts {
    RunFacts {
        isolation: Isolation::JobUid,
        uid_index: Some(0),
        wall: Wall::JobUser,
        tenant_slot: 1,
        k2node_version: "0.46.10".into(),
        k2node_commit: None,
        key_epoch: 0,
        statement_digest: digest.into(),
        roster_seq_seen: None,
        lane: None,
    }
}

async fn submit(port: u16, ws: &str, extra: Value) -> Resp {
    let mut body = json!({"workspace": ws, "argv": ["true"], "noSource": true, "clientJobId": uuid::Uuid::new_v4().simple().to_string()});
    for (k, v) in extra.as_object().unwrap() {
        body[k] = v.clone();
    }
    post(port, "/cli/compute/run", body).await
}

fn assigned(frames: &[Frame], job: &str) -> Option<k2_core::compute::proto::frames::Assign> {
    frames.iter().find_map(|f| match f {
        Frame::Assign(a) if a.fence.job_id == job => Some(a.clone()),
        _ => None,
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn receipt_v2_mixed_versions_and_the_receipt_route() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "rv2");
    let new_key = SigningKey::generate().unwrap();
    let old_key = SigningKey::generate().unwrap();
    active_node("new-1", "k2n7", &new_key, &ws);
    active_node("old-1", "k2n4", &old_key, &ws);
    let v9 = [ci::FEATURE_NODE_STATEMENT, ci::FEATURE_RECEIPT_V2];
    let mut new = compute_ws::test_attach("new-1", offer(9, &["artifacts", "stall", v9[0], v9[1]]));
    let mut old = compute_ws::test_attach("old-1", offer(8, &["artifacts", "stall"]));

    // Body rules: only 2.
    let r = submit(d.port, &ws, json!({"node": "k2n7", "receiptV": 3})).await;
    assert_eq!((r.status, code(&r).as_str()), (400, "bad_request"), "{}", r.body);
    // A protocol-8 node named for receipt v2: refused, nothing sent to it.
    let r = submit(d.port, &ws, json!({"node": "k2n4", "receiptV": 2})).await;
    assert_eq!((r.status, code(&r).as_str()), (409, "node_upgrade_required"), "{}", r.body);
    assert!(old.frames().is_empty());
    // A plain job to the old node: 0.46.9's plan bytes (no `receipt_v`).
    let r = submit(d.port, &ws, json!({"node": "k2n4"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let plain = j(&r)["job"]["id"].as_str().unwrap().to_string();
    let a = assigned(&old.frames(), &plain).expect("assigned to the old node");
    assert_eq!(a.plan.receipt_v, None);
    assert!(!serde_json::to_string(&a.plan).unwrap().contains("receipt_v"));
    // Auto placement with receipt v2: only the protocol-9 node.
    let r = submit(d.port, &ws, json!({"receiptV": 2})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let job = j(&r)["job"]["id"].as_str().unwrap().to_string();
    assert!(assigned(&old.frames(), &job).is_none(), "never to the protocol-8 node");
    let a = assigned(&new.frames(), &job).expect("assigned to the protocol-9 node");
    assert_eq!(a.plan.receipt_v, Some(RECEIPT_V2));
    let rw = row(&job);
    assert_eq!(rw.plan_digest, a.plan.digest().unwrap(), "receipt_v is in the digest");

    // The node's statement: kept when its pinned key signed it; one signed
    // by another key (claiming to be it) is refused.
    let st = statement(&new_key).sign(&new_key).unwrap();
    let other = SigningKey::generate().unwrap();
    compute_ws::test_frame("new-1", Frame::NodeStatement(statement(&other).sign(&other).unwrap()));
    assert_eq!(compute_ws::node_statement("new-1"), None, "another key's statement isn't this node's");
    compute_ws::test_frame("new-1", Frame::NodeStatement(st.clone()));
    assert_eq!(compute_ws::node_statement("new-1"), Some(st.clone()));

    // A v2 receipt without its statement: stored, not verified.
    let rc = receipt(&new_key, &rw, Some(run_facts(&st.digest())));
    let signed = SignedReceipt::sign(&new_key, &rc).unwrap();
    compute_ws::test_frame("new-1", Frame::Receipt(ReceiptFrame { job_id: job.clone(), generation: rw.generation as u32, receipt: signed.clone(), statement: None }));
    assert_eq!(row(&job).receipt_ok, Some(false), "a v2 receipt needs its statement");
    // With another statement (not the one it names): not verified.
    let st2 = NodeStatement { issued_at: 1_700_000_200, ..statement(&new_key) }.sign(&new_key).unwrap();
    compute_ws::test_frame("new-1", Frame::Receipt(ReceiptFrame { job_id: job.clone(), generation: rw.generation as u32, receipt: signed.clone(), statement: Some(st2) }));
    assert_eq!(row(&job).receipt_ok, Some(false));
    // A v1 receipt for a v2 plan: not verified.
    let v1 = SignedReceipt::sign(&new_key, &receipt(&new_key, &rw, None)).unwrap();
    compute_ws::test_frame("new-1", Frame::Receipt(ReceiptFrame { job_id: job.clone(), generation: rw.generation as u32, receipt: v1, statement: None }));
    assert_eq!(row(&job).receipt_ok, Some(false), "the plan asked for v2");
    // The real one: verified, and the route returns all the bundle needs.
    compute_ws::test_frame("new-1", Frame::Receipt(ReceiptFrame { job_id: job.clone(), generation: rw.generation as u32, receipt: signed.clone(), statement: Some(st.clone()) }));
    assert_eq!(row(&job).receipt_ok, Some(true));
    let r = get(d.port, "/cli/compute/jobs/receipt", &format!("job={job}")).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = j(&r);
    assert_eq!(v["verified"], json!(true));
    assert_eq!(v["v"], json!(2));
    assert_eq!(v["body"].as_str().unwrap(), signed.body, "the exact signed bytes");
    assert_eq!(v["signature"].as_str().unwrap(), signed.sig);
    assert_eq!(v["node_public_key_pem"].as_str().unwrap(), new_key.spki_pem());
    assert_eq!(v["plan"]["receipt_v"], json!(2));
    let back: ci::Signed = serde_json::from_value(v["statement"].clone()).expect("the statement");
    assert_eq!(back, st);
    let parsed: Receipt = serde_json::from_str(v["body"].as_str().unwrap()).unwrap();
    parsed.check_statement(&back).unwrap();

    // The plain v1 job's receipt: verified, and its answer has no v2 keys.
    let prow = row(&plain);
    let pv1 = SignedReceipt::sign(&old_key, &receipt(&old_key, &prow, None)).unwrap();
    compute_ws::test_frame("old-1", Frame::Receipt(ReceiptFrame { job_id: plain.clone(), generation: prow.generation as u32, receipt: pv1.clone(), statement: None }));
    assert_eq!(row(&plain).receipt_ok, Some(true));
    let r = get(d.port, "/cli/compute/jobs/receipt", &format!("job={plain}")).await;
    let v = j(&r);
    assert_eq!(v["verified"], json!(true));
    assert!(v.get("v").is_none() && v.get("statement").is_none(), "{v}");
    assert_eq!(v["body"].as_str().unwrap(), pv1.body);
    // The stored v1 receipt is the signed receipt alone (as 0.46.9 kept it).
    assert_eq!(row(&plain).receipt_json.as_deref().map(|s| serde_json::from_str::<Value>(s).unwrap()), Some(serde_json::to_value(&pv1).unwrap()));
}
