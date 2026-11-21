//! Build statements, the server side (prd-compute-build-attestations-v1
//! slice S3): the real dispatcher in-process (temp `$HOME`, loopback, the
//! vault on a file key there) and fake node sessions
//! (`compute_ws::test_attach` / `test_frame`).
//! - A plan asks for a statement (`provenance: 1`) only of a node offering
//!   `provenance-v1`; a protocol-10 node without it, and a protocol-9 node,
//!   get a plan with no `provenance` key at all.
//! - The statement that comes with the receipt is checked with the pinned
//!   node key and against the receipt: a node's lie about a file, a broken
//!   signature, one for a plan that didn't ask, and one whose plain env
//!   names a vault secret (BA-39) are refused and the job is marked "no
//!   statement" with the reason; the real one is kept and served
//!   (`/cli/compute/jobs/provenance`) as its `.intoto.jsonl` line.
//! - `/cli/compute/verify` checks a job's statement, a posted one, a file's
//!   digest, and the owner against a roster held here; a pinned owner key
//!   refuses the owner-server `builder.id` form.

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::path::PathBuf;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_core::compute::proto::admin::ApprovalSigner;
use k2_core::compute::proto::ci::{self, Isolation, KeyStatus, Signed, TaintBasis, Wall};
use k2_core::compute::proto::crypto::{self, SigningKey};
use k2_core::compute::proto::frames::{
    Availability, Caps, Control, Frame, Free, JobState, Offer, Plan, Receipt, ReceiptFrame, ReceiptLog, SignedReceipt, RECEIPT_V1,
};
use k2_core::compute::proto::node_roster::{NodeRoster, RosterEntry};
use k2_core::compute::proto::provenance::{self as prov, Dsse, NodeFacts, Owner, Statement};
use k2_core::compute::proto::results::ArtifactInfo;
use k2_core::compute::proto::share::Network;
use k2_core::compute::{store, trust};
use k2_daemon::compute_ws;
use k2_daemon::test_harness;
use k2_vault::{Actor, Kind, Namespace, PutRequest, Scope, SecretBytes, SecretStore};
use rusqlite::params;
use serde_json::{json, Value};

static TEST_LOCK: StdMutex<()> = StdMutex::new(());
const OWNER: &str = "owner-token-compute-provenance";
const FILE_SHA: &str = "4444444444444444444444444444444444444444444444444444444444444444";

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
    let home = k2_core::test_env::short_tmp(format!("k2prov-{:x}", nanos % 0xffff_ffff));
    std::fs::create_dir_all(&home).unwrap();
    let home = dunce::canonicalize(&home).unwrap();
    let env = Env { prev_home: std::env::var_os("HOME"), home: home.clone(), _guard: guard };
    std::env::set_var("HOME", &home);
    std::env::set_var("K2_COMPUTE", "1");
    // Build statements ride the CI preview.
    std::env::set_var("K2_CI", "1");
    k2_core::compute::retry::set_tuning_for_tests(k2_core::compute::retry::Tuning {
        tick_millis: 3_600_000,
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

fn hint(r: &Resp) -> String {
    j(r)["error"]["hint"].as_str().unwrap_or_else(|| panic!("no error.hint: {}", r.body)).to_string()
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
        node_version: "0.46.13".into(),
        features: features.iter().map(|f| f.to_string()).collect(),
        host: None,
        pkg: None,
        sim: None,
    }
}

fn owner_key() -> ApprovalSigner {
    ApprovalSigner::from_seed(&[9u8; 32]).unwrap()
}

/// The owner's roster #1, listing `key` as node `name`, active; stored
/// here as received from the owner.
fn store_roster(name: &str, key: &SigningKey) {
    let r = NodeRoster {
        v: 1,
        owner_id: "owner.example.test".into(),
        owner_server_fp: "9".repeat(64),
        roster_key: owner_key().public().encode(),
        seq: 1,
        issued_at: k2_core::compute::now() - 60,
        prev_digest: None,
        entries: vec![RosterEntry {
            node_fp: key.fingerprint(),
            node_spki: crypto::b64(key.spki_der()),
            name: name.into(),
            key_epoch: 0,
            statement_digest: format!("sha256:{}", "7".repeat(64)),
            status: KeyStatus::Active,
            revoked_reason: None,
            valid_from: 0,
            valid_to: None,
            taint_basis: Some(TaintBasis::SelfClean),
            attestation_digest: None,
            host: "host-a".into(),
            provider: None,
            site: None,
            admin_reach: vec![],
            pod: None,
            point: None,
        }],
    };
    r.check().expect("a valid roster");
    let signed = Signed::sign_owner(&owner_key(), ci::DOMAIN_NODE_ROSTER, &r).unwrap();
    let db = k2_core::db::shared();
    let conn = db.lock();
    assert!(trust::store_roster(&conn, &signed, &r, "peer", k2_core::compute::now()).unwrap());
}

fn row(job: &str) -> store::JobRow {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::job_by_id(&conn, job).expect("job row")
}

async fn submit(port: u16, ws: &str, extra: Value) -> String {
    let mut body = json!({"workspace": ws, "argv": ["sh", "build.sh"], "noSource": true, "artifacts": ["dist/*"], "clientJobId": uuid::Uuid::new_v4().simple().to_string()});
    for (k, v) in extra.as_object().unwrap() {
        body[k] = v.clone();
    }
    let r = post(port, "/cli/compute/run", body).await;
    assert_eq!(r.status, 200, "{}", r.body);
    j(&r)["job"]["id"].as_str().unwrap().to_string()
}

/// The plan the node got for `job`; the job then ends done (so the next
/// job to the same node isn't held behind it).
fn assigned(frames: &[Frame], job: &str) -> Option<Plan> {
    let plan = frames.iter().find_map(|f| match f {
        Frame::Assign(a) if a.fence.job_id == job => Some(a.plan.clone()),
        _ => None,
    })?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let now = k2_core::compute::now();
    store::set_job_state(&conn, job, JobState::Running, None, None, now - 90).unwrap();
    store::set_job_exit(&conn, job, Some(0), None);
    store::set_job_state(&conn, job, JobState::Done, None, None, now).unwrap();
    Some(plan)
}

/// The node's signed receipt for the job as it ran: one file, `dist/acme`.
fn receipt_for(key: &SigningKey, r: &store::JobRow, plan: &Plan) -> SignedReceipt {
    let rc = Receipt {
        v: RECEIPT_V1,
        job_id: r.id.clone(),
        attempt: r.attempt as u32,
        generation: r.generation as u32,
        plan_digest: r.plan_digest.clone(),
        node_fp: key.fingerprint(),
        controller_fp: "c".repeat(64),
        workspace_id: r.workspace_id.clone(),
        argv_sha256: k2_core::compute::proto::frames::argv_sha256(&plan.argv),
        env_names: plan.env.keys().cloned().collect(),
        src: None,
        tools: Default::default(),
        boot_id: "b".into(),
        started_at: Some(1_800_000_000),
        ended_at: 1_800_000_090,
        state: JobState::Done,
        exit: None,
        log: ReceiptLog { sha256: "3".repeat(64), bytes: 10, truncated: false },
        artifacts: vec![ArtifactInfo { name: "dist/acme".into(), sha256: FILE_SHA.into(), bytes: 10 }],
        secret_env: plan.secret_env.clone(),
        tools_used: Default::default(),
        run: None,
        kind: None,
        cache: None,
    };
    SignedReceipt::sign(key, &rc).unwrap()
}

fn facts(owner: Owner) -> NodeFacts {
    NodeFacts {
        owner,
        network: Network::On,
        isolation: Isolation::JobUid,
        wall: Wall::JobUser,
        tenant_slot: 1,
        lane: None,
        k2node_version: "0.46.13".into(),
        k2node_commit: None,
        protocol: 10,
        key_epoch: 0,
        statement_digest: None,
        node_name: Some("k2n7".into()),
    }
}

fn statement(sr: &SignedReceipt, plan: &Plan, owner: Owner) -> Statement {
    Statement::for_receipt(sr, plan, &facts(owner)).unwrap().expect("a file: a statement")
}

fn send(node: &str, r: &store::JobRow, sr: &SignedReceipt, env: Option<Dsse>) {
    compute_ws::test_frame(node, Frame::Receipt(ReceiptFrame { job_id: r.id.clone(), generation: r.generation as u32, receipt: sr.clone(), statement: None, provenance: env }));
}

fn stored(job: &str) -> Value {
    serde_json::from_str(row(job).receipt_json.as_deref().expect("a stored receipt")).unwrap()
}

fn put_secret(ws: &str, name: &str) {
    let vault = k2_daemon::vault::get().expect("vault open");
    let mut r = PutRequest::new(Namespace::User, Scope::workspace(ws), name, Kind::ApiKey);
    r.hosts = vec!["example.test".into()];
    vault.put(r, SecretBytes::from_slice(b"k2test-provenance-secret"), &Actor::Owner).expect("put");
}

async fn verify(port: u16, body: Value) -> Value {
    let r = post(port, "/cli/compute/verify", body).await;
    assert_eq!(r.status, 200, "{}", r.body);
    j(&r)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_statement_is_asked_checked_kept_served_and_verified() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    let ws = seed_ws(&env, "prov");
    let key = SigningKey::generate().unwrap();
    let mid_key = SigningKey::generate().unwrap();
    let old_key = SigningKey::generate().unwrap();
    active_node("new-1", "k2n7", &key, &ws);
    active_node("mid-1", "k2n9", &mid_key, &ws);
    active_node("old-1", "k2n4", &old_key, &ws);
    let mut new = compute_ws::test_attach("new-1", offer(10, &["artifacts", "stall", prov::FEATURE_PROVENANCE_V1]));
    let mut mid = compute_ws::test_attach("mid-1", offer(10, &["artifacts", "stall"]));
    let mut old = compute_ws::test_attach("old-1", offer(9, &["artifacts", "stall"]));

    // ── asking ──────────────────────────────────────────────────────────
    // A protocol-10 node without the feature, and a protocol-9 node: no
    // `provenance` key in the plan at all (their digests as before).
    let on_mid = submit(d.port, &ws, json!({"node": "k2n9"})).await;
    let p = assigned(&mid.frames(), &on_mid).expect("assigned to k2n9");
    assert_eq!(p.provenance, None);
    assert!(!serde_json::to_string(&p).unwrap().contains("provenance"), "no key: {p:?}");
    let on_old = submit(d.port, &ws, json!({"node": "k2n4"})).await;
    let p = assigned(&old.frames(), &on_old).expect("assigned to k2n4");
    assert!(!serde_json::to_string(&p).unwrap().contains("provenance"), "no key: {p:?}");
    // The node offering it: asked, and the digest covers it.
    let job = submit(d.port, &ws, json!({"node": "k2n7"})).await;
    let plan = assigned(&new.frames(), &job).expect("assigned to k2n7");
    assert_eq!(plan.provenance, Some(prov::PROVENANCE_V1));
    let rw = row(&job);
    assert_eq!(rw.plan_digest, plan.digest().unwrap());

    // ── receiving ───────────────────────────────────────────────────────
    let sr = receipt_for(&key, &rw, &plan);
    let st = statement(&sr, &plan, Owner::RosterKey(owner_key().public().fingerprint()));
    let good = key.sign_statement(&st).unwrap();
    // A node that lies about a file's digest (signed with its own key):
    // the receipt stands, the statement is refused, the job says why.
    let mut lie = st.clone();
    lie.subject[0].digest.insert("sha256".into(), "5".repeat(64));
    send("new-1", &rw, &sr, Some(key.sign_statement(&lie).unwrap()));
    assert_eq!(row(&job).receipt_ok, Some(true), "the receipt itself verifies");
    let s = stored(&job);
    assert!(s.get("provenance").is_none(), "{s}");
    assert!(s["provenance_refused"].as_str().unwrap().contains("subjects"), "{s}");
    let r = get(d.port, "/cli/compute/jobs/provenance", &format!("job={job}")).await;
    assert_eq!((r.status, code(&r).as_str()), (409, "no_statement"), "{}", r.body);
    assert!(hint(&r).contains("refused") && hint(&r).contains("subjects"), "{}", r.body);
    let refused_events: i64 = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row("SELECT COUNT(*) FROM compute_events WHERE kind = 'provenance_refused' AND job_id = ?1", params![job], |r| r.get(0)).unwrap()
    };
    assert_eq!(refused_events, 1, "the refusal is an event too");
    // A broken signature: refused.
    let mut broken = good.clone();
    broken.signatures[0].sig = good.signatures[0].sig.chars().rev().collect();
    send("new-1", &rw, &sr, Some(broken.clone()));
    assert!(stored(&job)["provenance_refused"].as_str().unwrap().contains("signature"), "{}", stored(&job));
    // Another key's signature over the true statement: refused.
    let stranger = SigningKey::generate().unwrap();
    send("new-1", &rw, &sr, Some(stranger.sign_statement(&st).unwrap()));
    assert!(stored(&job)["provenance_refused"].as_str().unwrap().contains("keyid"), "{}", stored(&job));
    // The real one: kept beside the receipt, served as its line.
    send("new-1", &rw, &sr, Some(good.clone()));
    let s = stored(&job);
    assert!(s.get("provenance_refused").is_none(), "{s}");
    assert_eq!(serde_json::from_value::<Dsse>(s["provenance"].clone()).unwrap(), good);
    assert_eq!(serde_json::from_value::<SignedReceipt>(s.clone()).unwrap(), sr, "a plain receipt reader still reads it");
    let r = get(d.port, "/cli/compute/jobs/provenance", &format!("job={job}")).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = j(&r);
    assert_eq!(v["line"].as_str().unwrap(), good.line().unwrap(), "the .intoto.jsonl line, byte for byte");
    assert_eq!(serde_json::from_value::<Dsse>(v["envelope"].clone()).unwrap(), good);
    assert_eq!((v["node"].as_str(), v["keyid"].as_str()), (Some("k2n7"), Some(key.fingerprint().as_str())));
    assert_eq!(v["subjects"], json!([{"name": "dist/acme", "sha256": FILE_SHA}]));
    assert_eq!(v["trust"]["nodeKeys"][key.fingerprint()]["pem"].as_str().unwrap(), key.spki_pem(), "the node's key for a stock DSSE tool");
    assert_eq!(v["trust"]["owners"], json!([]), "no roster from that owner held yet");
    // The receipt route answers as before.
    let r = get(d.port, "/cli/compute/jobs/receipt", &format!("job={job}")).await;
    assert_eq!((r.status, j(&r)["verified"].clone()), (200, json!(true)), "{}", r.body);
    assert_eq!(j(&r)["body"].as_str().unwrap(), sr.body);

    // ── verify ──────────────────────────────────────────────────────────
    let v = verify(d.port, json!({"job": job})).await;
    assert_eq!(v["verified"], json!(true), "{v}");
    let checks: Vec<String> = v["checks"].as_array().unwrap().iter().map(|c| c["check"].as_str().unwrap().to_string()).collect();
    assert_eq!(checks, ["node key", "receipt", "statement", "plain env", "owner"], "{v}");
    let owner_line = v["checks"][4]["detail"].as_str().unwrap();
    assert!(owner_line.starts_with("not checked") && owner_line.contains("holds no roster"), "{owner_line}");
    assert!(v["notChecked"].as_str().unwrap().contains("honest"), "{v}");
    // A file: one of its subjects, or not.
    let v = verify(d.port, json!({"job": job, "sha256": FILE_SHA, "file": "acme"})).await;
    assert_eq!(v["verified"], json!(true), "{v}");
    assert!(v["checks"][4]["detail"].as_str().unwrap().contains("subject 1 of 1: dist/acme"), "{v}");
    let v = verify(d.port, json!({"job": job, "sha256": "6".repeat(64), "file": "other"})).await;
    assert_eq!(v["verified"], json!(false), "{v}");
    assert!(v["failed"].as_str().unwrap().contains("none of the statement's 1 subject"), "{v}");
    // A posted statement (a .intoto.jsonl line): the same checks.
    let v = verify(d.port, json!({"envelope": good})).await;
    assert_eq!(v["verified"], json!(true), "{v}");
    let v = verify(d.port, json!({"envelope": broken})).await;
    assert_eq!(v["verified"], json!(false), "{v}");
    assert!(v["failed"].as_str().unwrap().contains("signature"), "{v}");
    let mut stray = good.clone();
    stray.signatures[0].keyid = stranger.fingerprint();
    let v = verify(d.port, json!({"envelope": stray})).await;
    assert!(v["failed"].as_str().unwrap().contains("the key this server pinned for k2n7"), "{v}");
    let r = post(d.port, "/cli/compute/verify", json!({"envelope": {"payload": "x"}})).await;
    assert_eq!((r.status, code(&r).as_str()), (400, "bad_request"), "{}", r.body);
    let r = post(d.port, "/cli/compute/verify", json!({})).await;
    assert_eq!((r.status, code(&r).as_str()), (400, "bad_request"), "{}", r.body);
    let r = get(d.port, "/cli/compute/verify", "job=x").await;
    assert_eq!(r.status, 405, "POST only: {}", r.body);

    // The owner: with the owner's roster held here, its key pinned.
    store_roster("k2n7", &key);
    let owner_fp = owner_key().public().fingerprint();
    let v = verify(d.port, json!({"job": job, "ownerKey": owner_fp})).await;
    assert_eq!(v["verified"], json!(true), "{v}");
    let owner_line = v["checks"][4]["detail"].as_str().unwrap();
    assert!(owner_line.contains("roster #1 from owner.example.test") && owner_line.contains("as active"), "{owner_line}");
    let v = verify(d.port, json!({"job": job, "ownerKey": "f".repeat(64)})).await;
    assert_eq!(v["verified"], json!(false), "{v}");
    assert!(v["failed"].as_str().unwrap().contains("not the pinned owner key"), "{v}");
    let r = get(d.port, "/cli/compute/jobs/provenance", &format!("job={job}")).await;
    assert_eq!(j(&r)["trust"]["owners"], json!([{"id": "owner.example.test", "rosterKeyFp": owner_fp, "rosterSeq": 1}]));
    assert_eq!(j(&r)["trust"]["rosters"].as_array().unwrap().len(), 1);

    // A node that held no roster signs the owner-server form: verified on
    // its own, refused under a pinned owner key.
    let job2 = submit(d.port, &ws, json!({"node": "k2n7"})).await;
    let plan2 = assigned(&new.frames(), &job2).expect("assigned");
    let rw2 = row(&job2);
    let sr2 = receipt_for(&key, &rw2, &plan2);
    send("new-1", &rw2, &sr2, Some(key.sign_statement(&statement(&sr2, &plan2, Owner::Server("9".repeat(64)))).unwrap()));
    let v = verify(d.port, json!({"job": job2})).await;
    assert_eq!(v["verified"], json!(true), "{v}");
    assert!(v["checks"][4]["detail"].as_str().unwrap().contains("held no roster"), "{v}");
    let v = verify(d.port, json!({"job": job2, "ownerKey": owner_fp})).await;
    assert_eq!(v["verified"], json!(false), "{v}");
    assert!(v["failed"].as_str().unwrap().contains("owner-server form"), "{v}");

    // ── no statement, and why ───────────────────────────────────────────
    // The plan didn't ask (k2n9 doesn't offer it).
    let mrw = row(&on_mid);
    let mplan = mrw.plan().unwrap();
    let msr = receipt_for(&mid_key, &mrw, &mplan);
    send("mid-1", &mrw, &msr, None);
    let r = get(d.port, "/cli/compute/jobs/provenance", &format!("job={on_mid}")).await;
    assert_eq!((r.status, code(&r).as_str()), (409, "no_statement"), "{}", r.body);
    assert!(hint(&r).contains("didn't ask"), "{}", r.body);
    // A statement for a plan that didn't ask: refused.
    let mst = statement(&msr, &mplan, Owner::Server("9".repeat(64)));
    send("mid-1", &mrw, &msr, Some(mid_key.sign_statement(&mst).unwrap()));
    assert!(stored(&on_mid)["provenance_refused"].as_str().unwrap().contains("didn't ask"), "{}", stored(&on_mid));
    // Asked, a file kept, and none came.
    let job3 = submit(d.port, &ws, json!({"node": "k2n7"})).await;
    let plan3 = assigned(&new.frames(), &job3).expect("assigned");
    let rw3 = row(&job3);
    send("new-1", &rw3, &receipt_for(&key, &rw3, &plan3), None);
    let r = get(d.port, "/cli/compute/jobs/provenance", &format!("job={job3}")).await;
    assert_eq!((r.status, code(&r).as_str()), (409, "no_statement"), "{}", r.body);
    assert!(hint(&r).contains("the node sent none"), "{}", r.body);

    // ── BA-39: a plain env name that is a vault secret's ────────────────
    put_secret(&ws, "BUILD_API_KEY");
    let job4 = submit(d.port, &ws, json!({"node": "k2n7", "env": {"BUILD_API_KEY": "plain-value"}})).await;
    let plan4 = assigned(&new.frames(), &job4).expect("assigned");
    assert_eq!(plan4.env.get("BUILD_API_KEY").map(String::as_str), Some("plain-value"));
    let rw4 = row(&job4);
    let sr4 = receipt_for(&key, &rw4, &plan4);
    send("new-1", &rw4, &sr4, Some(key.sign_statement(&statement(&sr4, &plan4, Owner::Server("9".repeat(64)))).unwrap()));
    assert!(stored(&job4)["provenance_refused"].as_str().unwrap().contains("plain env BUILD_API_KEY is also a secret's name"), "{}", stored(&job4));
    // A secret named after a statement was kept: not written out.
    let job5 = submit(d.port, &ws, json!({"node": "k2n7", "env": {"LATER_TOKEN": "x"}})).await;
    let plan5 = assigned(&new.frames(), &job5).expect("assigned");
    let rw5 = row(&job5);
    let sr5 = receipt_for(&key, &rw5, &plan5);
    send("new-1", &rw5, &sr5, Some(key.sign_statement(&statement(&sr5, &plan5, Owner::Server("9".repeat(64)))).unwrap()));
    let r = get(d.port, "/cli/compute/jobs/provenance", &format!("job={job5}")).await;
    assert_eq!(r.status, 200, "kept before the secret existed: {}", r.body);
    put_secret(&ws, "LATER_TOKEN");
    let r = get(d.port, "/cli/compute/jobs/provenance", &format!("job={job5}")).await;
    assert_eq!((r.status, code(&r).as_str()), (409, "provenance_secret_in_env"), "{}", r.body);
    assert!(hint(&r).contains("LATER_TOKEN") && hint(&r).contains("--secret"), "{}", r.body);
    let v = verify(d.port, json!({"job": job5})).await;
    assert_eq!(v["verified"], json!(false), "{v}");
    assert!(v["failed"].as_str().unwrap().contains("LATER_TOKEN"), "{v}");
    drop(new);
}
