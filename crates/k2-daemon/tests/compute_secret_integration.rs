//! Compute secret-env, the controller side (prd-compute-secret-env-v1
//! §10 "Controller, headless"): the real dispatcher in-process (temp
//! `$HOME`, loopback, the vault on a file key there) and a fake node
//! session (`compute_ws::test_attach` / `test_frame`).
//!
//! Covers: submit with `--secret` (grant check with the owner's command,
//! feature check, env-name rules, `--plan` names and coverage, `--any`
//! skipping nodes a grant doesn't cover, `*` covering only owned nodes);
//! the plan and every stored byte carry names only; `secret_need` →
//! `secret_env` that opens with the fake node's one-time key; refusals
//! (stale attempt, already running, revoked grant); a retry releases again
//! with a second audit row; the controller belt redacts a leaked chunk and
//! records `secret_redaction_miss`; agents can't grant to `compute:`.
//! Fake secrets only; every check fails loudly.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::path::{Path, PathBuf};
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_core::compute::proto::frames::{Availability, Caps, Control, Frame, Free, JobState, Offer, SecretNeed, StateUpdate};
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
const OWNER: &str = "owner-token-compute-secret-v1";
/// The fake secret. Long enough for the redactor (≥ 8 bytes).
const VALUE: &str = "k2test-secret-7Hq2LmZ9xW4vR8nP";
const ENV: &str = "BUILD_GMAIL_CLIENT_SECRET";

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
        k2_core::compute::retry::set_tuning_for_tests(Default::default());
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn setup() -> Env {
    let guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let home = PathBuf::from(format!("/tmp/k2csec-{:x}", nanos % 0xffff_ffff));
    std::fs::create_dir_all(&home).unwrap();
    let home = std::fs::canonicalize(&home).unwrap();
    let env = Env { prev_home: std::env::var_os("HOME"), home: home.clone(), _guard: guard };
    std::env::set_var("HOME", &home);
    std::env::set_var("K2_COMPUTE", "1");
    // The ticker sleeps; retries have no backoff here.
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

fn active_node(id: &str, name: &str, ws: &Ws) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let labels = std::collections::BTreeMap::from([("os".to_string(), "linux".to_string())]);
    store::insert_pending_node(&conn, id, name, &node_fp(id), "pem", &labels, "owner", "123456", 1).expect("node");
    store::confirm_node(&conn, id, "owner", 2).expect("confirm");
    store::set_grant(&conn, id, &ws.id, &store::GrantLimits::default(), "owner", 3).expect("grant");
}

fn offer(features: &[&str], share: Option<ShareView>) -> Offer {
    Offer {
        share,
        revision: 1,
        control: Control::Active,
        control_by: None,
        control_at: None,
        availability: Availability { ok: true, reasons: vec![] },
        caps: Caps { cpu_millis: 8000, mem_bytes: 32 << 30, disk_budget_bytes: 100 << 30, hard: true },
        free: Free { cpu_millis: 8000, mem_bytes: 16 << 30, disk_free_bytes: 500 << 30 },
        slots: vec![],
        running: 0,
        parallel_max: 4,
        refusal: None,
        labels: [("os".to_string(), "linux".to_string())].into(),
        tools: Default::default(),
        protocol: 4,
        boot_id: "b".into(),
        ledger_id: "l".into(),
        node_version: "0.46.4".into(),
        features: features.iter().map(|f| f.to_string()).collect(),
    }
}

fn tenant_view() -> ShareView {
    ShareView {
        role: Role::Tenant,
        slot: 1,
        shared: true,
        servers: 2,
        owner_label: Some("other.k2.dev".into()),
        state: ServerStatus::Active,
        limits: ShareLimits::new_server(Trust::Own, false),
        running: 0,
        queued: 0,
        cpu_secs_today: 0,
        expires_in_secs: None,
        skew_secs: None,
        quotes: vec![],
    }
}

fn row(job: &str) -> store::JobRow {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::job_by_id(&conn, job).expect("job row")
}

fn state(job: &str, generation: u32, st: JobState, reason: Option<&str>) -> Frame {
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
        outputs: None,
    })
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
    let mut r = PutRequest::new(Namespace::User, Scope::workspace(&ws.id), "GMAIL_CLIENT_SECRET", Kind::ApiKey);
    r.default_delivery = Some(Delivery::Fetch);
    r.delivery_ceiling = Some(Delivery::Env);
    r.hosts = vec!["oauth2.googleapis.com".into()];
    vault.put(r, SecretBytes::from_slice(VALUE.as_bytes()), &Actor::Owner).expect("put").meta.id
}

fn owner_grant(secret_id: &str, consumer: Consumer, env: Option<&str>) -> String {
    let vault = k2_daemon::vault::get().expect("vault open");
    vault
        .grant(
            GrantRequest {
                selector: Selector::Id(secret_id.into()),
                consumer,
                delivery: Delivery::Env,
                env_name: env.map(str::to_string),
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

/// Every frame this server sent the fake node, as wire JSON.
fn frames_text(frames: &[Frame]) -> String {
    frames.iter().map(|f| serde_json::to_string(f).unwrap()).collect::<Vec<_>>().join("\n")
}

/// Fail if the value is in any file under `root` (k2.db and its WAL, the
/// compute dir, logs, blobs; the vault holds it only encrypted).
fn assert_no_value_on_disk(root: &Path) {
    let mut stack = vec![root.to_path_buf()];
    let mut scanned = 0;
    while let Some(p) = stack.pop() {
        let m = std::fs::symlink_metadata(&p).unwrap();
        if m.is_dir() {
            for e in std::fs::read_dir(&p).unwrap() {
                stack.push(e.unwrap().path());
            }
        } else if m.is_file() {
            let bytes = std::fs::read(&p).unwrap();
            scanned += 1;
            assert!(!bytes.windows(VALUE.len()).any(|w| w == VALUE.as_bytes()), "the secret value is on disk in {}", p.display());
        }
    }
    assert!(scanned > 3, "scanned only {scanned} files under {}", root.display());
}

fn need(job: &str, r: &store::JobRow, key: &NodeKey) -> Frame {
    Frame::SecretNeed(SecretNeed {
        job_id: job.into(),
        generation: r.generation as u32,
        attempt: r.attempt as u32,
        plan_digest: r.plan_digest.clone(),
        epk: key.public_b64(),
    })
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

fn answer(conn: &mut compute_ws::TestConn, job: &str, all: &mut Vec<Frame>) -> Answer {
    assert!(compute_ws::test_wait_releases(Duration::from_secs(20)), "release thread never finished");
    let got = conn.frames();
    all.extend(got.iter().cloned());
    let mut answers: Vec<Answer> = got
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn secret_env_release_end_to_end() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    let ws = seed_ws(&env, "rel");
    active_node("sec-1", "k2n4", &ws);
    active_node("old-1", "k2n9", &ws);
    let secret_id = put_secret(&ws);
    let mut node = compute_ws::test_attach("sec-1", offer(&["artifacts", "stall", psecret::FEATURE_SECRET_ENV], None));
    let mut old = compute_ws::test_attach("old-1", offer(&["artifacts", "stall"], None));
    let mut sent: Vec<Frame> = Vec::new();
    let agent = passport(&ws);

    // Body checks (exit 2).
    // Env names are checked before coverage (no grant yet).
    let r = run(d.port, OWNER, json!({"node": "k2n4", "workspace": ws.id, "secrets": [{"name": "GMAIL_CLIENT_SECRET", "env": ENV}], "env": {ENV: "x"}})).await;
    assert_eq!((r.status, code(&r).as_str()), (400, "env_secret_collision"), "{}", r.body);
    let r = run(d.port, OWNER, json!({"node": "k2n4", "workspace": ws.id, "secrets": [{"name": "GMAIL_CLIENT_SECRET", "env": "K2_X"}]})).await;
    assert_eq!((r.status, code(&r).as_str()), (400, "env_refused"), "{}", r.body);
    let r = run(d.port, OWNER, json!({"node": "k2n4", "workspace": ws.id, "secrets": [{"name": "GMAIL_CLIENT_SECRET", "env": "LD_PRELOAD"}]})).await;
    assert_eq!((r.status, code(&r).as_str()), (400, "env_refused"), "{}", r.body);
    let many: Vec<Value> = (0..17).map(|i| json!({"name": format!("S{i}")})).collect();
    let r = run(d.port, OWNER, json!({"node": "k2n4", "workspace": ws.id, "secrets": many})).await;
    assert_eq!((r.status, code(&r).as_str()), (400, scodes::TOO_LARGE), "{}", r.body);
    let r = run(d.port, OWNER, json!({"node": "k2n4", "workspace": ws.id, "secrets": [{"name": "NOPE_SECRET"}]})).await;
    assert_eq!((r.status, code(&r).as_str()), (404, "secret_not_found"), "{}", r.body);

    // Not granted: exit 3 with the owner's exact command.
    let r = run(d.port, &agent, json!({"node": "k2n4", "secrets": [{"name": "GMAIL_CLIENT_SECRET", "env": ENV}]})).await;
    assert_eq!((r.status, code(&r).as_str()), (403, scodes::NOT_GRANTED), "{}", r.body);
    let hint = j(&r)["error"]["hint"].as_str().unwrap().to_string();
    assert!(hint.contains("k2 vault grant GMAIL_CLIENT_SECRET --scope ws:rel --to compute:rel/k2n4 --as env"), "{hint}");

    // An agent can't grant to compute: (people only, SE15), through the route.
    let r = post(d.port, "/cli/vault/grant", &agent, json!({"id": secret_id, "to": "compute:rel/k2n4", "as": "env"})).await;
    assert_eq!((r.status, code(&r).as_str()), (403, "vault_owner_confirm_required"), "{}", r.body);

    // The owner grants it to that node.
    let grant_id = owner_grant(&secret_id, Consumer::compute(&ws.id, "sec-1"), Some(ENV));

    // A node without the feature: refused (exit 3), never assigned.
    let r = run(d.port, &agent, json!({"node": "k2n9", "secrets": [{"name": "GMAIL_CLIENT_SECRET"}]})).await;
    assert_eq!((r.status, code(&r).as_str()), (409, "node_upgrade_required"), "{}", r.body);
    assert!(old.frames().is_empty());

    // --plan: names and coverage, no value.
    let r = run(d.port, &agent, json!({"node": "k2n4", "plan": true, "secrets": [{"name": "GMAIL_CLIENT_SECRET"}]})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let p = j(&r);
    assert_eq!(p["secrets"], json!([{"name": "GMAIL_CLIENT_SECRET", "secret": "GMAIL_CLIENT_SECRET", "env": ENV, "covered": true}]), "{p}");

    // Submit. The plan carries the env name only.
    let r = run(d.port, &agent, json!({"node": "k2n4", "retries": 3, "secrets": [{"name": "GMAIL_CLIENT_SECRET"}]})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let job = j(&r)["job"]["id"].as_str().unwrap().to_string();
    assert_eq!(j(&r)["job"]["secrets"], json!([ENV]));
    let f = node.frames();
    sent.extend(f.iter().cloned());
    let plan = f
        .iter()
        .find_map(|f| match f {
            Frame::Assign(a) if a.fence.job_id == job => Some(a.plan.clone()),
            _ => None,
        })
        .expect("assigned");
    assert_eq!(plan.secret_env, vec![ENV.to_string()]);
    let r1 = row(&job);
    assert_eq!(r1.plan_digest, plan.digest().unwrap());
    assert_eq!(r1.ctl.secrets.len(), 1);
    assert_eq!((r1.ctl.secrets[0].secret_id.as_str(), r1.ctl.secrets[0].env.as_str()), (secret_id.as_str(), ENV));
    assert_eq!(events("job_submit", &job).len(), 1);
    assert!(events("job_submit", &job)[0].contains(ENV));

    // A stale attempt: refused.
    let key = NodeKey::generate().unwrap();
    let mut bad = r1.clone();
    bad.generation += 1;
    compute_ws::test_frame("sec-1", need(&job, &bad, &key));
    assert!(matches!(answer(&mut node, &job, &mut sent), Answer::Refused(c) if c == scodes::ATTEMPT_STALE));

    // The release: opens with the node's one-time key, nothing else does.
    let key = NodeKey::generate().unwrap();
    compute_ws::test_frame("sec-1", need(&job, &r1, &key));
    let Answer::Env(e) = answer(&mut node, &job, &mut sent) else { panic!("refused") };
    let plain = key.open(&e.epk, &e.ct, &binding("sec-1", &r1)).expect("opens");
    assert_eq!(plain.0, vec![(ENV.to_string(), VALUE.to_string())]);
    assert!(NodeKey::generate().unwrap().open(&e.epk, &e.ct, &binding("sec-1", &r1)).is_err(), "another key can't open it");
    assert_eq!(events("secret_release", &job).len(), 1);
    assert!(compute_ws::test_wait_releases(Duration::from_secs(5)));

    // The belt: a leaked chunk is stored redacted, once flagged.
    let leak = format!("token={VALUE}\n");
    compute_ws::test_frame(
        "sec-1",
        Frame::Log(k2_core::compute::proto::frames::LogChunk {
            job_id: job.clone(),
            generation: r1.generation as u32,
            seq: 1,
            stream: k2_core::compute::proto::frames::LogStream::Out,
            data: k2_core::compute::proto::crypto::b64(leak.as_bytes()),
        }),
    );
    let (recs, _) = k2_core::compute::logs::read_from(&job, r1.generation, 0, 1 << 20);
    let stored: Vec<u8> = recs.iter().flat_map(|r| r.data.clone()).collect();
    assert_eq!(String::from_utf8(stored).unwrap(), format!("token=[k2v:{ENV}]\n"));
    assert_eq!(events(scodes::REDACTION_MISS, &job).len(), 1);

    // Running: no more releases for this attempt.
    compute_ws::test_frame("sec-1", state(&job, r1.generation as u32, JobState::Running, None));
    let key = NodeKey::generate().unwrap();
    compute_ws::test_frame("sec-1", need(&job, &row(&job), &key));
    assert!(matches!(answer(&mut node, &job, &mut sent), Answer::Refused(c) if c == scodes::ALREADY_RUNNING));

    // The attempt is interrupted: a retry is a new attempt that asks again,
    // and gets its own release and its own audit row (SE27).
    compute_ws::test_frame("sec-1", state(&job, r1.generation as u32, JobState::Interrupted, Some("node_restart")));
    assert!(!k2_daemon::compute_secrets::belt_armed(&job, r1.generation as u32), "the belt drops with the attempt");
    compute_ws::kick("sec-1");
    let r2 = row(&job);
    assert!(r2.attempt > r1.attempt, "retried: {} → {}", r1.attempt, r2.attempt);
    sent.extend(node.frames());
    let key = NodeKey::generate().unwrap();
    compute_ws::test_frame("sec-1", need(&job, &r2, &key));
    let Answer::Env(e2) = answer(&mut node, &job, &mut sent) else { panic!("the retry was refused") };
    assert_eq!(key.open(&e2.epk, &e2.ct, &binding("sec-1", &r2)).expect("opens").0[0].1, VALUE);
    let vault = k2_daemon::vault::get().unwrap();
    let audit = vault.audit(&k2_vault::AuditQuery { secret_id: Some(secret_id.clone()), ..Default::default() }, &Actor::Owner).unwrap();
    let releases: Vec<_> = audit.iter().filter(|a| a.actor == "compute:".to_string() + &ws.id + "/sec-1" && a.outcome == "ok").collect();
    assert_eq!(releases.len(), 2, "one audit row per release: {audit:?}");

    // Revoked: the next attempt is refused; the revoke names the running job.
    compute_ws::test_frame("sec-1", state(&job, r2.generation as u32, JobState::Running, None));
    let r = post(d.port, "/cli/vault/revoke", OWNER, json!({"grantId": grant_id})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let still = j(&r)["stillRunning"].as_array().cloned().unwrap_or_default();
    assert_eq!(still.len(), 1, "{}", r.body);
    assert_eq!((still[0]["job"].as_str(), still[0]["env"].as_str()), (Some(job.as_str()), Some(ENV)));
    compute_ws::test_frame("sec-1", state(&job, r2.generation as u32, JobState::Interrupted, Some("node_restart")));
    compute_ws::kick("sec-1");
    let r3 = row(&job);
    assert!(r3.attempt > r2.attempt);
    sent.extend(node.frames());
    let key = NodeKey::generate().unwrap();
    compute_ws::test_frame("sec-1", need(&job, &r3, &key));
    assert!(matches!(answer(&mut node, &job, &mut sent), Answer::Refused(c) if c == scodes::NOT_GRANTED));
    assert!(!events("secret_refused", &job).is_empty());

    // get / receipt-style JSON: names only.
    let r = get(d.port, "/cli/compute/jobs/get", OWNER, &format!("job={job}")).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(j(&r)["secrets"], json!([ENV]));
    assert_eq!(j(&r)["result"]["secrets"], json!([ENV]));

    // Nothing this server sent the node, nor any file under $HOME, holds the value.
    let wire = frames_text(&sent);
    assert!(wire.contains("secret_env") && !wire.contains(VALUE), "the wire carried the value");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
    }
    assert_no_value_on_disk(&env.home);
    compute_ws::test_detach("sec-1");
    compute_ws::test_detach("old-1");
}

/// `--any` (SE6): only nodes offering `secret-env` that a grant covers;
/// `compute:<ws>/*` covers only nodes this server owns (Q2).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn any_places_only_on_covered_nodes() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    let ws = seed_ws(&env, "anyw");
    active_node("own-1", "own-node", &ws);
    active_node("ten-1", "tenant-node", &ws);
    let secret_id = put_secret(&ws);
    let feats = [psecret::FEATURE_SECRET_ENV, "artifacts", "stall", FEATURE_QUEUE, FEATURE_SERVERS];
    let mut ten = compute_ws::test_attach("ten-1", offer(&feats, Some(tenant_view())));
    let mut own = compute_ws::test_attach("own-1", offer(&[psecret::FEATURE_SECRET_ENV, "artifacts", "stall"], None));
    {
        let db = k2_core::db::shared();
        assert!(store::node_by_id(&db.lock(), "ten-1").unwrap().is_tenant());
    }
    owner_grant(&secret_id, Consumer::compute(&ws.id, "*"), None);
    let agent = passport(&ws);
    for _ in 0..3 {
        let r = run(d.port, &agent, json!({"any": true, "secrets": [{"name": "GMAIL_CLIENT_SECRET"}]})).await;
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(j(&r)["job"]["node"], "own-node", "{}", r.body);
    }
    assert!(ten.frames().iter().all(|f| !matches!(f, Frame::Assign(_))), "the tenant node never gets the job");
    assert!(own.frames().iter().any(|f| matches!(f, Frame::Assign(_))));
    // Named, on the tenant node: `*` doesn't cover it.
    let r = run(d.port, &agent, json!({"node": "tenant-node", "secrets": [{"name": "GMAIL_CLIENT_SECRET"}]})).await;
    assert_eq!((r.status, code(&r).as_str()), (403, scodes::NOT_GRANTED), "{}", r.body);
    compute_ws::test_detach("own-1");
    compute_ws::test_detach("ten-1");
}
