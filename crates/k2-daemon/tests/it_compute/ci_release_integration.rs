//! `k2 ci release` + `k2 ci evidence` (0.46.11, prd-ci-on-compute-for-
//! clients-v1 C5 + C6b), end to end on the controller: the real dispatcher
//! in-process (temp `$HOME`, loopback), a real git repo with `[release]` in
//! its `.k2/ci.toml`, a client seat on another owner's lane with that
//! owner's signed node roster (an Ed25519 test key), three fake protocol-9
//! node sessions (`compute_ws::test_attach`) that get the jobs, sign v2
//! receipts with their own keys and send their self-statements.
//!
//! Covers: placement refusals from the owner's roster (`seat_degraded`
//! with each node's reason; `no_independent_nodes` naming the host),
//! `--plan` (L1 and V1 on different hosts, L3 on the third node, nothing
//! submitted), the set submitted as one unit (each job pinned by `--only`
//! to its node, receipt v2, `BUILD_COMMIT` / `BUILD_TREE`, `--tier ci`),
//! evidence refused before the set passed, the reaper's verdict, then the
//! evidence bundle v2 (it checks; receipts by label and role, the
//! statements, the roster and seat, the owner attestations the roster names
//! (a note while one is missing), the pairing pins, the commit object)
//! and the dual-build comparison.

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_core::compute::proto::admin::ApprovalSigner;
use k2_core::compute::proto::ci::{self, Isolation, KeyStatus, RunFacts, SeatState, Signed, Taint, TaintBasis, Wall};
use k2_core::compute::proto::crypto::{self, SigningKey};
use k2_core::compute::proto::evidence::{EvidenceBundle, Role};
use k2_core::compute::proto::frames::{
    Availability, Caps, Control, Frame, Free, JobState, Offer, Plan, Receipt, ReceiptFrame, ReceiptLog, SignedReceipt, SrcRan, RECEIPT_V2,
};
use k2_core::compute::proto::node_roster::{NodeRoster, OwnerAttestation, RosterEntry, SeatStatement};
use k2_core::compute::proto::results::ArtifactInfo;
use k2_core::compute::proto::statement::{KeyExposure, NodeStatement};
use k2_core::compute::{store, trust};
use k2_daemon::compute_ws;
use k2_daemon::test_harness;
use rusqlite::params;
use serde_json::{json, Value};

static TEST_LOCK: StdMutex<()> = StdMutex::new(());
const OWNER: &str = "owner-token-ci-release";
const NODE_OWNER: &str = "owner.example";
const LANE: &str = "ci-standard";
const SEAT: &str = "seat-1";

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
    let home = k2_core::test_env::short_tmp(format!("k2rel-{:x}", nanos % 0xffff_ffff));
    std::fs::create_dir_all(&home).unwrap();
    let home = dunce::canonicalize(&home).unwrap();
    let env = Env { prev_home: std::env::var_os("HOME"), home: home.clone(), _guard: guard };
    std::env::set_var("HOME", &home);
    std::env::set_var("K2_COMPUTE", "1");
    std::env::set_var("K2_CI", "1");
    k2_core::compute::retry::set_tuning_for_tests(k2_core::compute::retry::Tuning { tick_millis: 3_600_000, any_hold_secs: 0, retry_base_secs: 0, ..Default::default() });
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

async fn post(port: u16, path: &str, body: Value) -> Resp {
    let pq = format!("{path}?token={}", enc(OWNER));
    tokio::task::spawn_blocking(move || req_blocking(port, "POST", &pq, Some(body.to_string()))).await.unwrap()
}

fn j(r: &Resp) -> Value {
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("JSON ({e}): {:?}", r.body))
}

fn code(r: &Resp) -> String {
    j(r)["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {}", r.body)).to_string()
}

fn git(dir: &Path, args: &[&str]) -> String {
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
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

const CI_TOML: &str = r#"version = 1
[release]
project = "acme-desktop"
owner = "owner.example"
lane = "ci-standard"
[release.builds.linux-x86_64]
argv = ["bash", "k2/ci/build-linux.sh"]
needs = []
artifacts = ["k2/out/*"]
env = ["BUILD_COMMIT", "BUILD_TREE"]
label = "L1"
verify_label = "V1"
independent = ["host"]
mem_gb = 1
[release.builds.linux-docs]
argv = ["sh", "-c", "true"]
needs = []
artifacts = ["docs/*"]
label = "L3"
mem_gb = 1
"#;

struct Node {
    id: &'static str,
    name: &'static str,
    key: SigningKey,
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
        k2node_version: "0.46.11".into(),
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

fn offer() -> Offer {
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
        tools: [("git".to_string(), "2.43.0".to_string())].into(),
        protocol: 9,
        boot_id: "b".into(),
        ledger_id: "l".into(),
        node_version: "0.46.11".into(),
        features: vec!["artifacts".into(), "stall".into(), ci::FEATURE_NODE_STATEMENT.into(), ci::FEATURE_RECEIPT_V2.into()],
        host: None,
        pkg: None,
    }
}

fn owner_key() -> ApprovalSigner {
    ApprovalSigner::from_seed(&[7u8; 32]).unwrap()
}

/// The owner's attestation for `n` (signed with the roster key; the same
/// bytes every call).
fn attestation(n: &Node) -> Signed {
    let st = statement(&n.key);
    let d = st.sign(&n.key).unwrap().digest();
    trust::attestation_for(NODE_OWNER, &owner_key().public(), &st, &d, "installed with the job wall", "owner", 1_700_000_200).sign(&owner_key()).unwrap()
}

/// The owner's roster `seq` with each node on `hosts[i]`, vouched for when
/// `vouched[i]` (by attestation for the names in `attested`); chained to
/// `prev`.
fn roster(nodes: &[Node], hosts: &[&str], vouched: &[bool], attested: &[&str], seq: u64, prev: Option<&Signed>) -> Signed {
    let mut entries: Vec<RosterEntry> = nodes
        .iter()
        .zip(hosts)
        .zip(vouched)
        .map(|((n, h), v)| RosterEntry {
            node_fp: n.key.fingerprint(),
            node_spki: crypto::b64(n.key.spki_der()),
            name: n.name.into(),
            key_epoch: 0,
            statement_digest: statement(&n.key).sign(&n.key).unwrap().digest(),
            status: KeyStatus::Active,
            revoked_reason: None,
            valid_from: 0,
            valid_to: None,
            taint_basis: v.then_some(if attested.contains(&n.name) { TaintBasis::OwnerAttested } else { TaintBasis::SelfClean }),
            attestation_digest: (*v && attested.contains(&n.name)).then(|| attestation(n).digest()),
            host: h.to_string(),
            provider: None,
            site: None,
            admin_reach: vec![],
            pod: None,
            point: None,
        })
        .collect();
    entries.sort_by(|a, b| a.node_fp.cmp(&b.node_fp));
    let r = NodeRoster {
        v: 1,
        owner_id: NODE_OWNER.into(),
        owner_server_fp: "9".repeat(64),
        roster_key: owner_key().public().encode(),
        seq,
        issued_at: k2_core::compute::now() - 60 + seq as i64,
        prev_digest: prev.map(Signed::digest),
        entries,
    };
    r.check().expect("a valid roster");
    let signed = Signed::sign_owner(&owner_key(), ci::DOMAIN_NODE_ROSTER, &r).unwrap();
    let db = k2_core::db::shared();
    let conn = db.lock();
    assert!(trust::store_roster(&conn, &signed, &r, "peer", k2_core::compute::now()).unwrap());
    signed
}

fn seed(env: &Env) -> (String, PathBuf, Vec<Node>) {
    let path = env.home.join("ws-acme");
    std::fs::create_dir_all(path.join(".k2")).unwrap();
    std::fs::write(path.join(".k2/ci.toml"), CI_TOML).unwrap();
    std::fs::write(path.join("README"), "acme\n").unwrap();
    git(&path, &["init", "-q"]);
    git(&path, &["add", "."]);
    git(&path, &["commit", "-q", "-m", "one"]);
    let ws = uuid::Uuid::new_v4().to_string();
    let nodes = vec![
        Node { id: "n-7", name: "k2n7", key: SigningKey::generate().unwrap() },
        Node { id: "n-8", name: "k2n8", key: SigningKey::generate().unwrap() },
        Node { id: "n-9", name: "k2n9", key: SigningKey::generate().unwrap() },
    ];
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, 'acme', ?2, 'acme')", params![ws, path.to_string_lossy()]).unwrap();
    conn.execute("UPDATE projects SET agents_can_use_compute = 1 WHERE id = ?1", params![ws]).unwrap();
    let labels = std::collections::BTreeMap::from([("os".to_string(), "linux".to_string())]);
    for n in &nodes {
        store::insert_pending_node(&conn, n.id, n.name, &n.key.fingerprint(), &n.key.spki_pem(), &labels, "owner", "123456", 1).expect("node");
        store::confirm_node(&conn, n.id, "owner", 2).expect("confirm");
        store::set_grant(&conn, n.id, &ws, &store::GrantLimits { allow_ci: Some(true), ..Default::default() }, "owner", 3).expect("grant");
    }
    // This server's seat on the owner's lane, with the owner's statement.
    let names: Vec<String> = {
        let mut v: Vec<String> = nodes.iter().map(|n| n.name.to_string()).collect();
        v.sort();
        v
    };
    let st = SeatStatement {
        v: 1,
        owner_id: NODE_OWNER.into(),
        roster_key: owner_key().public().encode(),
        seat_id: SEAT.into(),
        client_fp: "c".repeat(64),
        client_label: "acme.k2.dev".into(),
        lane: LANE.into(),
        nodes: names.clone(),
        roster_seq: 1,
        state: SeatState::Active,
        issued_at: k2_core::compute::now() - 30,
        pod: None,
        line: None,
    };
    let signed = Signed::sign_owner(&owner_key(), ci::DOMAIN_NODE_SEAT, &st).unwrap();
    let now = k2_core::compute::now();
    trust::put_seat(
        &conn,
        &trust::SeatRow {
            id: SEAT.into(),
            side: "client".into(),
            peer_fp: "9".repeat(64),
            peer_host: NODE_OWNER.into(),
            owner_id: NODE_OWNER.into(),
            owner_key_fp: Some(owner_key().public().fingerprint()),
            client_fp: "c".repeat(64),
            client_label: "acme.k2.dev".into(),
            client_url: String::new(),
            lane: LANE.into(),
            nodes: names,
            state: "active".into(),
            statement: Some(signed),
            roster_seq: Some(1),
            code_binding: None,
            request_id: None,
            batch_id: None,
            note: None,
            created_at: now,
            updated_at: now,
        },
    )
    .expect("seat");
    for n in &nodes {
        trust::set_node_seat(&conn, n.id, Some(SEAT)).expect("node on the seat");
    }
    (ws, path, nodes)
}

fn assigned(frames: &[Frame], job: &str) -> Option<Plan> {
    frames.iter().find_map(|f| match f {
        Frame::Assign(a) if a.fence.job_id == job => Some(a.plan.clone()),
        _ => None,
    })
}

fn row(job: &str) -> store::JobRow {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::job_by_id(&conn, job).expect("job row")
}

/// The node ran the job: its row ends done/exit 0, and it sends its v2
/// receipt (with `files`) and the statement the receipt names.
fn node_ran(n: &Node, job: &str, commit: &str, tree: &str, files: &[(&str, &str)]) {
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let now = k2_core::compute::now();
        store::set_job_state(&conn, job, JobState::Running, None, None, now - 90).unwrap();
        store::set_job_exit(&conn, job, Some(0), None);
        store::set_job_state(&conn, job, JobState::Done, None, None, now).unwrap();
    }
    let r = row(job);
    let st = statement(&n.key).sign(&n.key).unwrap();
    let rc = Receipt {
        v: RECEIPT_V2,
        job_id: r.id.clone(),
        attempt: r.attempt as u32,
        generation: r.generation as u32,
        plan_digest: r.plan_digest.clone(),
        node_fp: n.key.fingerprint(),
        controller_fp: "c".repeat(64),
        workspace_id: r.workspace_id.clone(),
        argv_sha256: "a".repeat(64),
        env_names: vec!["BUILD_COMMIT".into(), "BUILD_TREE".into()],
        src: Some(SrcRan { commit: commit.into(), tree: tree.into(), dirty_sha256: None, slot: None, warmth: Some("private".into()) }),
        tools: Default::default(),
        boot_id: "b".into(),
        started_at: Some(k2_core::compute::now() - 90),
        ended_at: k2_core::compute::now(),
        state: JobState::Done,
        exit: None,
        log: ReceiptLog { sha256: "s".into(), bytes: 0, truncated: false },
        artifacts: files.iter().map(|(name, sha)| ArtifactInfo { name: name.to_string(), sha256: sha.to_string(), bytes: 10 }).collect(),
        secret_env: vec![],
        tools_used: Default::default(),
        run: Some(RunFacts {
            isolation: Isolation::TenantUid,
            uid_index: None,
            wall: Wall::JobUser,
            tenant_slot: 1,
            k2node_version: "0.46.11".into(),
            k2node_commit: None,
            key_epoch: 0,
            statement_digest: st.digest(),
            roster_seq_seen: None,
            lane: Some(LANE.into()),
        }),
    };
    let signed = SignedReceipt::sign(&n.key, &rc).unwrap();
    compute_ws::test_frame(n.id, Frame::Receipt(ReceiptFrame { job_id: job.into(), generation: r.generation as u32, receipt: signed, statement: Some(st) }));
    assert_eq!(row(job).receipt_ok, Some(true), "the receipt verifies with the pinned key");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_release_set_is_placed_on_the_seat_built_and_bundled_as_evidence() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let (ws, path, nodes) = seed(&env);
    let sha = git(&path, &["rev-parse", "HEAD"]);
    let tree = git(&path, &["rev-parse", "HEAD^{tree}"]);
    let mut conns: Vec<compute_ws::TestConn> = nodes.iter().map(|n| compute_ws::test_attach(n.id, offer())).collect();
    for n in &nodes {
        compute_ws::test_frame(n.id, Frame::NodeStatement(statement(&n.key).sign(&n.key).unwrap()));
    }
    let release = |plan: bool| json!({"workspace": ws, "version": "1.2.0", "rev": "HEAD", "plan": plan});

    // No roster yet: every node is out, by name.
    let r = post(d.port, "/cli/ci/release", release(true)).await;
    assert_eq!((r.status, code(&r).as_str()), (409, "seat_degraded"), "{}", r.body);
    assert!(r.body.contains("isn't in the owner's newest node roster"), "{}", r.body);
    // Roster 1: only k2n7 vouched for. Degraded, each one's reason named.
    let r1 = roster(&nodes, &["h1", "h1", "h2"], &[true, false, false], &[], 1, None);
    let r = post(d.port, "/cli/ci/release", release(true)).await;
    assert_eq!((r.status, code(&r).as_str()), (409, "seat_degraded"), "{}", r.body);
    assert!(r.body.contains("k2n8: the owner vouches for nothing") && r.body.contains("1 can"), "{}", r.body);
    // Roster 2: all vouched for, all on one host: no independent pair.
    let r2 = roster(&nodes, &["h1", "h1", "h1"], &[true, true, true], &[], 2, Some(&r1));
    let r = post(d.port, "/cli/ci/release", release(true)).await;
    assert_eq!((r.status, code(&r).as_str()), (409, "no_independent_nodes"), "{}", r.body);
    assert!(r.body.contains("both on host h1"), "{}", r.body);
    // Roster 3: k2n9 on another host, vouched for by the owner's
    // attestation. --plan places the set, sends nothing.
    let _r3 = roster(&nodes, &["h1", "h1", "h2"], &[true, true, true], &["k2n9"], 3, Some(&r2));
    let r = post(d.port, "/cli/ci/release", release(true)).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let placed = j(&r)["placed"].as_array().unwrap().clone();
    let at = |label: &str| placed.iter().find(|p| p["label"] == label).unwrap_or_else(|| panic!("no {label}: {placed:?}")).clone();
    let (l1, v1, l3) = (at("L1"), at("V1"), at("L3"));
    let on_h2 = |p: &Value| p["node"] == "k2n9";
    assert!(on_h2(&l1) ^ on_h2(&v1), "exactly one of L1/V1 on the other host: {placed:?}");
    assert_eq!((l1["role"].as_str(), v1["role"].as_str(), l3["role"].as_str()), (Some("build"), Some("verify"), Some("build")));
    assert!(l3["node"] != l1["node"] && l3["node"] != v1["node"], "L3 on the third node: {placed:?}");
    for c in conns.iter_mut() {
        assert!(c.frames().iter().all(|f| !matches!(f, Frame::Assign(_))), "--plan submits nothing");
    }
    // Unknown builds and versions are refused by name.
    let r = post(d.port, "/cli/ci/release", json!({"workspace": ws, "version": "1.2.0", "builds": ["nope"]})).await;
    assert_eq!((r.status, code(&r).as_str()), (404, "ci_build_not_found"), "{}", r.body);
    let r = post(d.port, "/cli/ci/release", json!({"workspace": ws, "version": "v1.2"})).await;
    assert_eq!((r.status, code(&r).as_str()), (400, "ci_version_invalid"), "{}", r.body);

    // The real thing: three jobs, each on its planned node.
    let r = post(d.port, "/cli/ci/release", release(false)).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let run = j(&r)["run"].clone();
    assert_eq!((run["kind"].as_str(), run["state"].as_str()), (Some("release"), Some("running")), "{run}");
    assert_eq!(run["release"]["version"], "1.2.0");
    let run_id = run["id"].as_str().unwrap().to_string();
    let jobs: Vec<(String, String, String)> = run["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["name"].as_str().unwrap().to_string(), c["job"].as_str().expect("submitted").to_string(), c["nodeId"].as_str().unwrap().to_string()))
        .collect();
    assert_eq!(jobs.len(), 3);
    let frames: Vec<Vec<Frame>> = conns.iter_mut().map(|c| c.frames()).collect();
    for (label, job, node_id) in &jobs {
        let i = nodes.iter().position(|n| n.id == node_id).unwrap();
        let plan = assigned(&frames[i], job).unwrap_or_else(|| panic!("{label} assigned to {node_id}"));
        assert_eq!(plan.receipt_v, Some(RECEIPT_V2), "{label}");
        assert_eq!(plan.src.as_ref().map(|s| s.commit.as_str()), Some(sha.as_str()));
        if label != "L3" {
            assert_eq!(plan.env.get("BUILD_COMMIT"), Some(&sha));
            assert_eq!(plan.env.get("BUILD_TREE"), Some(&tree));
            assert_eq!(plan.artifacts, vec!["k2/out/*".to_string()]);
        }
        let jr = row(job);
        assert_eq!(jr.tier().as_str(), "ci", "{label}");
        assert_eq!(jr.ctl.any.as_ref().map(|a| a.only.clone()), Some(vec![node_id.clone()]), "{label} pinned to its node by --only");
    }
    // Evidence before the set passed: refused.
    let r = post(d.port, "/cli/ci/evidence", json!({"workspace": ws, "run": run_id})).await;
    assert_eq!((r.status, code(&r).as_str()), (409, "ci_release_not_done"), "{}", r.body);

    // The nodes build: L1 and V1 agree on the binary, differ on the bundle.
    for (label, job, node_id) in &jobs {
        let n = nodes.iter().find(|n| n.id == node_id).unwrap();
        let (one, two, three, four) = ("1".repeat(64), "2".repeat(64), "3".repeat(64), "4".repeat(64));
        let files: Vec<(&str, &str)> = match label.as_str() {
            "L1" => vec![("k2/out/acme", &one), ("k2/out/acme.AppImage", &two)],
            "V1" => vec![("k2/out/acme", &one), ("k2/out/acme.AppImage", &three)],
            _ => vec![("docs/index.html", &four)],
        };
        node_ran(n, job, &sha, &tree, &files);
    }
    tokio::task::spawn_blocking(k2_daemon::ci_engine::tick_now).await.unwrap();
    // The roster vouches for k2n9 by attestation, but no attestation
    // document is held here yet: the bundle has none, and a note says so.
    let r = post(d.port, "/cli/ci/evidence", json!({"workspace": ws, "run": &run_id[..8]})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = j(&r);
    assert_eq!(v["bundle"]["attestations"], json!([]), "{}", r.body);
    let note = v["notes"][0].as_str().unwrap_or_else(|| panic!("a note: {}", r.body));
    assert!(note.contains("k2n9") && note.contains("owner_attested"), "{note}");
    // The attestation as it arrives with the seat offer (compute_owner_
    // attestations; the trust tests cover receive_attestations itself).
    let k9 = nodes.iter().find(|n| n.name == "k2n9").unwrap();
    let att = attestation(k9);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO compute_owner_attestations (digest, key_fp, node_fp, body, sig, received_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![att.digest(), owner_key().public().fingerprint(), k9.key.fingerprint(), att.body, att.sig, k2_core::compute::now()],
        )
        .unwrap();
    }
    let r = post(d.port, "/cli/ci/evidence", json!({"workspace": ws, "run": &run_id[..8]})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = j(&r);
    assert_eq!(v["notes"], json!([]), "every attestation the roster names is held: {}", r.body);
    assert_eq!(v["file"], "evidence-acme-desktop-1.2.0.json");
    let bundle: EvidenceBundle = serde_json::from_value(v["bundle"].clone()).expect("a bundle v2");
    bundle.check().expect("it checks");
    assert_eq!((bundle.project.as_str(), bundle.version.as_deref(), bundle.run_id.as_str()), ("acme-desktop", Some("1.2.0"), run_id.as_str()));
    assert_eq!((bundle.source.commit.as_str(), bundle.source.tree.as_str()), (sha.as_str(), tree.as_str()));
    assert!(bundle.source.commit_object.is_some(), "the commit object rides along");
    assert_eq!(bundle.receipts.keys().cloned().collect::<Vec<_>>(), vec!["L1".to_string(), "L3".to_string(), "V1".to_string()]);
    assert_eq!((bundle.receipts["L1"].role, bundle.receipts["V1"].role), (Role::Build, Role::Verify));
    assert_eq!(bundle.receipts["V1"].build, "linux-x86_64");
    for (label, er) in &bundle.receipts {
        assert_eq!(er.plan_digest().unwrap(), serde_json::from_str::<Receipt>(&er.body).unwrap().plan_digest, "{label}: the plan hashes to its receipt");
    }
    assert_eq!(bundle.statements.len(), 3);
    assert_eq!(bundle.attestations.len(), 1, "the one attestation the roster names");
    assert_eq!(bundle.attestations[0].digest(), att.digest());
    let a = OwnerAttestation::open(&bundle.attestations[0], &owner_key().public()).expect("opens with the roster key");
    assert_eq!((a.node_fp.as_str(), a.owner_id.as_str()), (k9.key.fingerprint().as_str(), NODE_OWNER));
    assert_eq!(bundle.rosters.len(), 1);
    let held: NodeRoster = bundle.rosters[0].open_owner(&owner_key().public(), ci::DOMAIN_NODE_ROSTER).expect("the owner's roster");
    assert_eq!(held.seq, 3, "the newest roster held");
    let seat: SeatStatement = bundle.seats[0].open_owner(&owner_key().public(), ci::DOMAIN_NODE_SEAT).expect("the seat statement");
    assert_eq!(seat.seat_id, SEAT);
    assert_eq!(bundle.pairing_pins.len(), 3);
    for n in &nodes {
        assert_eq!(bundle.pairing_pins[&n.key.fingerprint()], crypto::b64(n.key.spki_der()));
    }
    let dual = &v["dual"]["linux-x86_64"]["compare"];
    assert_eq!(dual["matched"], json!(["k2/out/acme"]));
    assert_eq!(dual["differ"], json!(["k2/out/acme.AppImage"]));
    assert_eq!(v["jobs"]["L3"]["artifacts"][0]["name"], "docs/index.html");
    // A check run is not a release run.
    let r = post(d.port, "/cli/ci/evidence", json!({"workspace": ws, "run": "ffffffff"})).await;
    assert_eq!((r.status, code(&r).as_str()), (404, "ci_run_not_found"), "{}", r.body);
}
