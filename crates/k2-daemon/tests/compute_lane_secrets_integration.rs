//! Lane secrets, the controller side (0.46.10, prd-ci-on-compute-for-
//! clients-v1 §4.4, vs-live V25/V26; slice C3b): the real dispatcher
//! in-process (temp `$HOME`, loopback, the vault on a file key there),
//! fake node sessions (`compute_ws::test_attach` / `test_frame`) and a
//! fixed seat book (`lane_secrets::StaticSeats`; the daemon's own book,
//! `TrustSeats`, is covered by the two-server test in `compute_trust`).
//! Consent is the pinned owner's (`compute_owner_trust`, through
//! `/cli/compute/owners/consent`), bound to the owner's roster key.
//!
//! Covers: a lane grant (`compute:<ws>/lane:<owner>/<lane>`) sealed to a
//! seat node and opened with that node's one-time key; refused without
//! the owner's consent (at submit and again at release), on a non-Linux
//! node, without a per-job uid or `--exclusive`; consent only from a
//! person with a fresh confirm; per-node grants still work (owned node,
//! and a `trust own` tenant on no seat); and a protocol-8 node on a seat
//! never gets a lane secret. Fake secrets only; every check fails loudly.

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use k2_core::compute::lane_secrets::{self as lanes, NodeStanding, SeatView, StaticSeats};
use k2_core::compute::proto::admin::ApprovalSigner;
use k2_core::compute::proto::ci::{self, Isolation};
use k2_core::compute::proto::frames::{Availability, Caps, Control, Frame, Free, Offer, SecretNeed, FEATURE_JOB_WALL};
use k2_core::compute::proto::secret::{self as psecret, codes as scodes, NodeKey};
use k2_core::compute::proto::share::{Role, ServerStatus, ShareLimits, ShareView, Trust, FEATURE_QUEUE, FEATURE_SERVERS};
use k2_core::compute::store;
use k2_core::session::SessionId;
use k2_daemon::compute_ws;
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;
use k2_vault::{Actor, Consumer, Delivery, GrantRequest, Kind, Namespace, PutRequest, Scope, SecretBytes, SecretStore, Selector};
use rusqlite::params;
use serde_json::{json, Value};

static TEST_LOCK: StdMutex<()> = StdMutex::new(());
const OWNER: &str = "owner-token-lane-secret-v1";
const VALUE: &str = "k2test-secret-LANE-4Rt8pW2zQ6yK1nB9";
const ENV: &str = "BUILD_GMAIL_CLIENT_ID";
const NODE_OWNER: &str = "owner.k2.dev";
const SEAT: &str = "seat-1";
const LANE: &str = "ci-standard";

struct Env {
    home: PathBuf,
    prev_home: Option<std::ffi::OsString>,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl Drop for Env {
    fn drop(&mut self) {
        lanes::set_seat_book(None);
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
    let guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let home = k2_core::test_env::short_tmp(format!("k2lsec-{:x}", nanos % 0xffff_ffff));
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
    assert!(!body.contains(VALUE), "an HTTP answer carries the secret value: {body}");
    Resp { status, body }
}

fn enc(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

async fn post(port: u16, path: &str, token: &str, body: Value) -> Resp {
    let pq = format!("{path}?token={}", enc(token));
    tokio::task::spawn_blocking(move || req_blocking(port, "POST", &pq, Some(body.to_string()))).await.unwrap()
}

async fn get(port: u16, path: &str, token: &str) -> Resp {
    let pq = format!("{path}?token={}", enc(token));
    tokio::task::spawn_blocking(move || req_blocking(port, "GET", &pq, None)).await.unwrap()
}

fn j(r: &Resp) -> Value {
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("JSON ({e}): {:?}", r.body))
}

fn code(r: &Resp) -> String {
    j(r)["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {}", r.body)).to_string()
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

fn node_fp(id: &str) -> String {
    format!("{id:0>64}")
}

fn active_node(id: &str, name: &str, ws: &Ws, os: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let labels = std::collections::BTreeMap::from([("os".to_string(), os.to_string())]);
    store::insert_pending_node(&conn, id, name, &node_fp(id), "pem", &labels, "owner", "123456", 1).expect("node");
    store::confirm_node(&conn, id, "owner", 2).expect("confirm");
    store::set_grant(&conn, id, &ws.id, &store::GrantLimits::default(), "owner", 3).expect("grant");
}

/// Everything a protocol-9 node offers a server on its lane.
const LANE_FEATURES: &[&str] =
    &[psecret::FEATURE_SECRET_ENV, ci::FEATURE_LANE_SECRETS, ci::FEATURE_JOB_UID, FEATURE_JOB_WALL, "artifacts", "stall", FEATURE_QUEUE, FEATURE_SERVERS];

fn offer(features: &[&str], share: Option<ShareView>, protocol: u32, os: &str) -> Offer {
    Offer {
        share,
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
        labels: [("os".to_string(), os.to_string())].into(),
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

fn tenant_view(trust: Trust) -> ShareView {
    // Room for several jobs at once, and `--exclusive` (a lane's budget).
    let mut limits = ShareLimits::new_server(trust, false);
    limits.max_running = 8;
    limits.exclusive = true;
    ShareView {
        role: Role::Tenant,
        slot: 1,
        shared: true,
        servers: 2,
        owner_label: Some(NODE_OWNER.into()),
        state: ServerStatus::Active,
        limits,
        running: 0,
        queued: 0,
        cpu_secs_today: 0,
        expires_in_secs: None,
        skew_secs: None,
        quotes: vec![],
        priority: None,
    }
}

/// The node owner's roster key (a test key), pinned here by "a person".
fn owner_key() -> ApprovalSigner {
    ApprovalSigner::from_seed(&[5u8; 32]).unwrap()
}

fn seat(nodes: &[&str], isolation: Isolation) -> SeatView {
    SeatView {
        seat_id: SEAT.into(),
        owner_id: NODE_OWNER.into(),
        owner_key_fp: owner_key().public().fingerprint(),
        lane: LANE.into(),
        active: true,
        lane_secrets: true,
        isolation: vec![isolation],
        node_ids: nodes.iter().map(|n| n.to_string()).collect(),
    }
}

/// The owner's roster key pinned (as the seat request card does), without
/// the secrets consent.
fn pin_owner() {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::compute::trust::pin_owner(&conn, &owner_key().public(), NODE_OWNER, &"9".repeat(64), "owner", "local-auth", false, 1).expect("pin");
}

fn install(seats: Vec<SeatView>, clean: &[&str]) {
    let standing = clean.iter().map(|n| (n.to_string(), NodeStanding::Clean)).collect();
    lanes::set_seat_book(Some(Arc::new(StaticSeats { seats, standing })));
}

fn row(job: &str) -> store::JobRow {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::job_by_id(&conn, job).expect("job row")
}

fn events(kind: &str, job: &str) -> Vec<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut st = conn.prepare("SELECT detail_json FROM compute_events WHERE kind = ?1 AND job_id = ?2 ORDER BY id").unwrap();
    st.query_map(params![kind, job], |r| r.get::<_, String>(0)).unwrap().map(|x| x.unwrap()).collect()
}

async fn run(port: u16, token: &str, mut body: Value) -> Resp {
    body["argv"] = json!(["true"]);
    body["noSource"] = json!(true);
    body["clientJobId"] = json!(uuid::Uuid::new_v4().simple().to_string());
    post(port, "/cli/compute/run", token, body).await
}

fn put_secret(ws: &Ws) -> String {
    let vault = k2_daemon::vault::get().expect("vault open");
    let mut r = PutRequest::new(Namespace::User, Scope::workspace(&ws.id), "GMAIL_CLIENT_ID", Kind::ApiKey);
    r.default_delivery = Some(Delivery::Fetch);
    r.delivery_ceiling = Some(Delivery::Env);
    r.hosts = vec!["oauth2.googleapis.com".into()];
    vault.put(r, SecretBytes::from_slice(VALUE.as_bytes()), &Actor::Owner).expect("put").meta.id
}

fn owner_grant(secret_id: &str, consumer: Consumer) -> String {
    let vault = k2_daemon::vault::get().expect("vault open");
    vault
        .grant(
            GrantRequest {
                selector: Selector::Id(secret_id.into()),
                consumer,
                delivery: Delivery::Env,
                env_name: Some(ENV.into()),
                file_name: None,
                hosts_narrowed: None,
                expires_at: None,
                version: None,
            },
            &Actor::Owner,
        )
        .expect("owner grants")
        .id
}

fn need(job: &str, r: &store::JobRow, key: &NodeKey) -> Frame {
    Frame::SecretNeed(SecretNeed { job_id: job.into(), generation: r.generation as u32, attempt: r.attempt as u32, plan_digest: r.plan_digest.clone(), epk: key.public_b64() })
}

fn binding(node: &str, r: &store::JobRow) -> psecret::Binding {
    psecret::Binding {
        controller_fp: k2_core::compute::controller_key().expect("controller key").fingerprint(),
        node_fp: node_fp(node),
        job_id: r.id.clone(),
        generation: r.generation as u32,
        attempt: r.attempt as u32,
        plan_digest: r.plan_digest.clone(),
    }
}

enum Answer {
    Env(k2_core::compute::proto::frames::SecretEnv),
    Refused(String),
}

fn answer(conn: &mut compute_ws::TestConn, job: &str) -> Answer {
    assert!(compute_ws::test_wait_releases(Duration::from_secs(20)), "release thread never finished");
    let mut answers: Vec<Answer> = conn
        .frames()
        .into_iter()
        .filter_map(|f| match f {
            Frame::SecretEnv(e) if e.job_id == job => Some(Answer::Env(e)),
            Frame::SecretRefused(r) if r.job_id == job => Some(Answer::Refused(r.code)),
            _ => None,
        })
        .collect();
    assert_eq!(answers.len(), 1, "exactly one answer per secret_need");
    answers.pop().unwrap()
}

/// The assign's plan of `job` among the frames a node got.
fn assigned(frames: &[Frame], job: &str) -> k2_core::compute::proto::frames::Plan {
    frames
        .iter()
        .find_map(|f| match f {
            Frame::Assign(a) if a.fence.job_id == job => Some(a.plan.clone()),
            _ => None,
        })
        .expect("assigned")
}

fn consent_subject() -> String {
    format!("consent:{}", owner_key().public().fingerprint())
}

/// The person's consent, through the route: owner token + a fresh
/// `compute-trust` confirm bound to the owner's pinned roster key.
async fn consent(port: u16) {
    pin_owner();
    let tok = k2_daemon::vault::get().unwrap().confirm.mint(k2_core::compute::trust::CONFIRM_TRUST, Some(&consent_subject()), "owner", "local-auth");
    let r = post(port, "/cli/compute/owners/consent", OWNER, json!({"owner": NODE_OWNER, "on": true, "confirmToken": tok})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!((j(&r)["owner"].as_str(), j(&r)["secretsConsent"].as_bool()), (Some(NODE_OWNER), Some(true)), "{}", r.body);
}

/// Take it back (at once; no confirm).
async fn revoke_consent(port: u16) {
    pin_owner();
    let r = post(port, "/cli/compute/owners/consent", OWNER, json!({"owner": NODE_OWNER, "on": false})).await;
    assert_eq!((r.status, j(&r)["secretsConsent"].as_bool()), (200, Some(false)), "{}", r.body);
}

/// A lane grant reaches every node of the seat, sealed to the asking
/// node's one-time key; the CLI form, the label and the grant warning all
/// name the lane; only a person consents, with a fresh confirm.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lane_grant_is_sealed_to_a_seat_node() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    let ws = seed_ws(&env, "lanews");
    active_node("ln-1", "k2n7", &ws, "linux");
    active_node("ln-2", "k2n8", &ws, "linux");
    let secret_id = put_secret(&ws);
    install(vec![seat(&["ln-1", "ln-2"], Isolation::JobUid)], &["ln-1", "ln-2"]);
    let mut n1 = compute_ws::test_attach("ln-1", offer(LANE_FEATURES, Some(tenant_view(Trust::Team)), 9, "linux"));
    let mut n2 = compute_ws::test_attach("ln-2", offer(LANE_FEATURES, Some(tenant_view(Trust::Team)), 9, "linux"));
    let agent = passport(&ws);

    // The CLI form resolves to this server's seat (V26) and reads back.
    let c = k2_daemon::compute_secrets::consumer(&format!("lanews/lane:{NODE_OWNER}/{LANE}"), None).expect("lane consumer");
    assert_eq!(c, Consumer::compute_lane(&ws.id, SEAT));
    assert_eq!(k2_daemon::compute_secrets::label(&c), format!("compute:lanews/lane:{NODE_OWNER}/{LANE}"));
    assert!(k2_daemon::compute_secrets::consumer(&format!("lanews/lane:{NODE_OWNER}/other"), None).is_err(), "no seat on that lane");
    assert!(k2_daemon::compute_secrets::consumer("lanews/lane:only-owner", None).is_err(), "lane:<owner>/<lane>");

    // Not granted yet: the command names the lane form.
    let r = run(d.port, &agent, json!({"node": "k2n7", "secrets": [{"name": "GMAIL_CLIENT_ID"}]})).await;
    assert_eq!((r.status, code(&r).as_str()), (403, scodes::NOT_GRANTED), "{}", r.body);
    assert!(j(&r)["error"]["hint"].as_str().unwrap().contains(&format!("--to compute:lanews/lane:{NODE_OWNER}/{LANE}")), "{}", r.body);

    // The owner grants to the lane through the route (a person confirms a
    // readable share from the owner token); the answer warns about root.
    let confirm = k2_daemon::vault::get().unwrap().confirm.mint("grant-read", Some(&secret_id), "owner", "local-auth");
    let r = post(
        d.port,
        "/cli/vault/grant",
        OWNER,
        json!({"id": secret_id, "to": format!("compute:lanews/lane:{NODE_OWNER}/{LANE}"), "as": "env", "envName": ENV, "confirmToken": confirm}),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(j(&r)["warning"].as_str().is_some_and(|w| w.contains("root") && w.contains("consent")), "{}", r.body);
    // An agent can't grant to a lane (people only, SE15).
    let r = post(d.port, "/cli/vault/grant", &agent, json!({"id": secret_id, "to": format!("compute:lanews/lane:{NODE_OWNER}/{LANE}"), "as": "env"})).await;
    assert_eq!((r.status, code(&r).as_str()), (403, "vault_owner_confirm_required"), "{}", r.body);

    // Consent: never an agent, never without a fresh confirm bound to the
    // owner's pinned key.
    pin_owner();
    let r = post(d.port, "/cli/compute/owners/consent", &agent, json!({"owner": NODE_OWNER, "on": true})).await;
    assert_eq!((r.status, code(&r).as_str()), (403, "person_only"), "{}", r.body);
    let r = post(d.port, "/cli/compute/owners/consent", OWNER, json!({"owner": NODE_OWNER, "on": true})).await;
    assert_eq!((r.status, code(&r).as_str()), (403, "vault_confirm_required"), "{}", r.body);
    let wrong = k2_daemon::vault::get().unwrap().confirm.mint(k2_core::compute::trust::CONFIRM_TRUST, Some(&format!("consent:{}", "f".repeat(64))), "owner", "local-auth");
    let r = post(d.port, "/cli/compute/owners/consent", OWNER, json!({"owner": NODE_OWNER, "on": true, "confirmToken": wrong})).await;
    assert_eq!((r.status, code(&r).as_str()), (403, "vault_confirm_required"), "a confirm for another owner key: {}", r.body);
    let r = post(d.port, "/cli/compute/owners/consent", OWNER, json!({"owner": NODE_OWNER, "on": true, "preview": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(j(&r)["text"].as_str().unwrap().contains("root"), "{}", r.body);
    consent(d.port).await;
    let r = get(d.port, "/cli/compute/owners", OWNER).await;
    let trusted = j(&r)["trusted"].as_array().cloned().unwrap_or_default();
    assert!(trusted.iter().any(|t| t["ownerId"] == NODE_OWNER && t["secretsConsent"] == true), "{}", r.body);

    // Submit on each seat node; each release opens with that node's key.
    for (nid, name, conn) in [("ln-1", "k2n7", &mut n1), ("ln-2", "k2n8", &mut n2)] {
        let r = run(d.port, &agent, json!({"node": name, "secrets": [{"name": "GMAIL_CLIENT_ID"}]})).await;
        assert_eq!(r.status, 200, "{}", r.body);
        let job = j(&r)["job"]["id"].as_str().unwrap().to_string();
        let plan = assigned(&conn.frames(), &job);
        assert_eq!(plan.secret_env, vec![ENV.to_string()], "names only in the plan");
        let r1 = row(&job);
        let key = NodeKey::generate().unwrap();
        compute_ws::test_frame(nid, need(&job, &r1, &key));
        let Answer::Env(e) = answer(conn, &job) else { panic!("refused on {name}") };
        let plain = key.open(&e.epk, &e.ct, &binding(nid, &r1)).expect("opens with the node's key");
        assert_eq!(plain.0, vec![(ENV.to_string(), VALUE.to_string())]);
        let released = events("secret_release", &job);
        assert_eq!(released.len(), 1);
        assert!(released[0].contains(ENV) && !released[0].contains(VALUE), "{released:?}");
    }
    // One vault audit row per release, naming each node; never the value.
    let audit = k2_daemon::vault::get().unwrap().audit(&k2_vault::AuditQuery { secret_id: Some(secret_id.clone()), ..Default::default() }, &Actor::Owner).unwrap();
    for nid in ["ln-1", "ln-2"] {
        assert_eq!(audit.iter().filter(|a| a.actor == format!("compute:{}/{nid}", ws.id) && a.outcome == "ok").count(), 1, "{audit:?}");
    }
    assert!(!format!("{audit:?}").contains(VALUE));

    // `--any` places only on the seat's nodes.
    let r = run(d.port, &agent, json!({"any": true, "secrets": [{"name": "GMAIL_CLIENT_ID"}]})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(["k2n7", "k2n8"].contains(&j(&r)["job"]["node"].as_str().unwrap()), "{}", r.body);

    // Revoking the consent stops the next release at once.
    let r = run(d.port, &agent, json!({"node": "k2n7", "secrets": [{"name": "GMAIL_CLIENT_ID"}]})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let job = j(&r)["job"]["id"].as_str().unwrap().to_string();
    let _ = n1.frames();
    revoke_consent(d.port).await;
    let r1 = row(&job);
    compute_ws::test_frame("ln-1", need(&job, &r1, &NodeKey::generate().unwrap()));
    assert!(matches!(answer(&mut n1, &job), Answer::Refused(c) if c == ci::codes::SECRET_OWNER_NOT_TRUSTED), "V25 at release");
    compute_ws::test_detach("ln-1");
    compute_ws::test_detach("ln-2");
}

/// Refused without consent, on a non-Linux node, and without isolation;
/// a tainted or unlisted key and a seat that isn't active too.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lane_secrets_are_refused_outside_the_rules() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    let ws = seed_ws(&env, "rulews");
    // Names of their own: the k2.db is shared by the tests of this binary.
    active_node("lx-1", "rule-lx", &ws, "linux");
    active_node("mac-1", "rule-mac", &ws, "macos");
    active_node("nouid-1", "rule-nouid", &ws, "linux");
    let secret_id = put_secret(&ws);
    install(vec![seat(&["lx-1", "mac-1", "nouid-1"], Isolation::JobUid)], &["lx-1", "mac-1", "nouid-1"]);
    let mut lx = compute_ws::test_attach("lx-1", offer(LANE_FEATURES, Some(tenant_view(Trust::Team)), 9, "linux"));
    let _mac = compute_ws::test_attach("mac-1", offer(LANE_FEATURES, Some(tenant_view(Trust::Team)), 9, "macos"));
    let no_uid: Vec<&str> = LANE_FEATURES.iter().copied().filter(|f| *f != ci::FEATURE_JOB_UID).collect();
    let _nouid = compute_ws::test_attach("nouid-1", offer(&no_uid, Some(tenant_view(Trust::Team)), 9, "linux"));
    owner_grant(&secret_id, Consumer::compute_lane(&ws.id, SEAT));
    let agent = passport(&ws);
    let ask = |node: &str| json!({"node": node, "secrets": [{"name": "GMAIL_CLIENT_ID"}]});
    // No consent left from another test (one k2.db per test binary).
    revoke_consent(d.port).await;

    // No consent: refused at submit (exit 3), with the person's command.
    let r = run(d.port, &agent, ask("rule-lx")).await;
    assert_eq!((r.status, code(&r).as_str()), (403, ci::codes::SECRET_OWNER_NOT_TRUSTED), "{}", r.body);
    assert!(j(&r)["error"]["hint"].as_str().unwrap().contains(&format!("k2 compute owners consent {NODE_OWNER} on")), "{}", r.body);
    // Nor placed anywhere with --any.
    let r = run(d.port, &agent, json!({"any": true, "secrets": [{"name": "GMAIL_CLIENT_ID"}]})).await;
    assert_eq!((r.status, code(&r).as_str()), (404, "no_node_fits"), "--any found a node without consent: {}", r.body);
    assert!(j(&r)["error"]["hint"].as_str().unwrap().contains("not covered by the secret's grant"), "{}", r.body);
    assert!(lx.frames().iter().all(|f| !matches!(f, Frame::Assign(_))));

    consent(d.port).await;
    // A Mac node: never (one uid per node).
    let r = run(d.port, &agent, ask("rule-mac")).await;
    assert_eq!((r.status, code(&r).as_str()), (409, scodes::LINUX_ONLY), "{}", r.body);
    // No per-job uid and not exclusive: refused; --exclusive is enough.
    let r = run(d.port, &agent, ask("rule-nouid")).await;
    assert_eq!((r.status, code(&r).as_str()), (409, scodes::NEEDS_ISOLATION), "{}", r.body);
    let r = run(d.port, OWNER, json!({"node": "rule-nouid", "workspace": ws.id, "exclusive": true, "why": "lane secret test", "secrets": [{"name": "GMAIL_CLIENT_ID"}]})).await;
    assert_eq!(r.status, 200, "--exclusive meets a job_uid lane: {}", r.body);
    // An exclusive lane refuses a per-job uid.
    install(vec![seat(&["lx-1", "mac-1", "nouid-1"], Isolation::Exclusive)], &["lx-1", "mac-1", "nouid-1"]);
    let r = run(d.port, &agent, ask("rule-lx")).await;
    assert_eq!((r.status, code(&r).as_str()), (409, scodes::NEEDS_ISOLATION), "{}", r.body);
    // A tainted key, or one not on the owner's roster.
    install(vec![seat(&["lx-1"], Isolation::JobUid)], &[]);
    let r = run(d.port, &agent, ask("rule-lx")).await;
    assert_eq!((r.status, code(&r).as_str()), (403, scodes::UNTRUSTED_SERVER), "{}", r.body);
    lanes::set_seat_book(Some(Arc::new(StaticSeats { seats: vec![seat(&["lx-1"], Isolation::JobUid)], standing: [("lx-1".to_string(), NodeStanding::Tainted)].into() })));
    let r = run(d.port, &agent, ask("rule-lx")).await;
    assert_eq!((r.status, code(&r).as_str()), (403, ci::codes::NODE_KEY_TAINTED), "{}", r.body);
    // A seat that isn't active, or a lane that doesn't allow secrets.
    let mut off = seat(&["lx-1"], Isolation::JobUid);
    off.active = false;
    install(vec![off], &["lx-1"]);
    let r = run(d.port, &agent, ask("rule-lx")).await;
    assert_eq!((r.status, code(&r).as_str()), (403, scodes::UNTRUSTED_SERVER), "{}", r.body);
    let mut closed = seat(&["lx-1"], Isolation::JobUid);
    closed.lane_secrets = false;
    install(vec![closed], &["lx-1"]);
    let r = run(d.port, &agent, ask("rule-lx")).await;
    assert_eq!((r.status, code(&r).as_str()), (403, scodes::UNTRUSTED_SERVER), "{}", r.body);
    // A per-node grant to a team node on a seat goes under the same rules
    // (never around them).
    owner_grant(&secret_id, Consumer::compute(&ws.id, "lx-1"));
    let r = run(d.port, &agent, ask("rule-lx")).await;
    assert_eq!((r.status, code(&r).as_str()), (403, scodes::UNTRUSTED_SERVER), "a node grant doesn't skip the lane: {}", r.body);
    // And with no seat book at all, a lane grant covers nothing.
    lanes::set_seat_book(None);
    let r = run(d.port, &agent, json!({"node": "rule-nouid", "secrets": [{"name": "GMAIL_CLIENT_ID"}]})).await;
    assert_eq!((r.status, code(&r).as_str()), (403, scodes::NOT_GRANTED), "{}", r.body);
    compute_ws::test_detach("lx-1");
    compute_ws::test_detach("mac-1");
    compute_ws::test_detach("nouid-1");
}

/// Per-node grants keep working exactly as before C3b: on a node this
/// server owns, and on a `trust own` tenant node that is on no seat. A
/// protocol-8 node on a seat (mixed versions) never gets a lane secret,
/// while its owned protocol-8 neighbour keeps today's secret env.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn per_node_grants_still_work_and_protocol_8_nodes_never_get_lane_secrets() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    let ws = seed_ws(&env, "mixws");
    active_node("own-1", "own-node", &ws, "linux");
    active_node("trust-1", "own-tenant", &ws, "linux");
    active_node("old-1", "k2n10", &ws, "linux");
    let secret_id = put_secret(&ws);
    // old-1 is on our seat, but runs a protocol-8 k2-node: it offers
    // secret-env only as a `trust own` server would see it, and no
    // protocol-9 feature.
    install(vec![seat(&["old-1"], Isolation::JobUid)], &["old-1"]);
    consent(d.port).await;
    let p8 = [psecret::FEATURE_SECRET_ENV, "artifacts", "stall", FEATURE_QUEUE, FEATURE_SERVERS];
    let mut own = compute_ws::test_attach("own-1", offer(&p8, None, 8, "linux"));
    let mut trust = compute_ws::test_attach("trust-1", offer(&p8, Some(tenant_view(Trust::Own)), 8, "linux"));
    let mut old = compute_ws::test_attach("old-1", offer(&p8, Some(tenant_view(Trust::Own)), 8, "linux"));
    owner_grant(&secret_id, Consumer::compute(&ws.id, "own-1"));
    owner_grant(&secret_id, Consumer::compute(&ws.id, "trust-1"));
    owner_grant(&secret_id, Consumer::compute_lane(&ws.id, SEAT));
    let agent = passport(&ws);

    for (nid, name, conn) in [("own-1", "own-node", &mut own), ("trust-1", "own-tenant", &mut trust)] {
        let r = run(d.port, &agent, json!({"node": name, "secrets": [{"name": "GMAIL_CLIENT_ID"}]})).await;
        assert_eq!(r.status, 200, "{name}: {}", r.body);
        let job = j(&r)["job"]["id"].as_str().unwrap().to_string();
        let plan = assigned(&conn.frames(), &job);
        assert_eq!(plan.receipt_v, None, "a protocol-8 node gets today's plan");
        let r1 = row(&job);
        let key = NodeKey::generate().unwrap();
        compute_ws::test_frame(nid, need(&job, &r1, &key));
        let Answer::Env(e) = answer(conn, &job) else { panic!("{name}: a per-node grant was refused") };
        assert_eq!(key.open(&e.epk, &e.ct, &binding(nid, &r1)).expect("opens").0, vec![(ENV.to_string(), VALUE.to_string())]);
    }

    // The protocol-8 seat node: the lane grant covers it, the gate says no.
    let r = run(d.port, &agent, json!({"node": "k2n10", "secrets": [{"name": "GMAIL_CLIENT_ID"}]})).await;
    assert_eq!((r.status, code(&r).as_str()), (409, scodes::NEEDS_UPDATE), "{}", r.body);
    assert!(old.frames().iter().all(|f| !matches!(f, Frame::Assign(_))), "never assigned");
    // Even a forged secret_need for a job of that node is never answered
    // with a value: the node's own jobs only, and the gate again.
    let r = run(d.port, &agent, json!({"node": "k2n10"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let job = j(&r)["job"]["id"].as_str().unwrap().to_string();
    let _ = old.frames();
    compute_ws::test_frame("old-1", need(&job, &row(&job), &NodeKey::generate().unwrap()));
    assert!(compute_ws::test_wait_releases(Duration::from_secs(20)));
    assert!(!old.frames().iter().any(|f| matches!(f, Frame::SecretEnv(_))), "no value for a job without secrets");
    compute_ws::test_detach("own-1");
    compute_ws::test_detach("trust-1");
    compute_ws::test_detach("old-1");
}
