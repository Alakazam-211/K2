//! One node, many servers: the server side (prd-compute-shared-queue-v1
//! S4a, §16 tier 2). The real dispatcher in-process (temp `$HOME`,
//! loopback, no client attached) and a fake node session
//! (`compute_ws::test_attach` / `test_frame`): the frames a protocol-3
//! node would send, without the node crate's shared mode (S1/S2 lanes).
//!
//! Covers: solo nodes keep today's path (no share fields, no new plan
//! fields to an older node, placement still waits for capacity); a shared
//! node shows its share; a named run goes into the node's queue at once
//! and shows the node's position; a cancel without --force asks the node
//! and ends `job_started` when the start won (V57); submit-time share
//! checks (exit 3); `--plan` with the quote; `--any` moves only after a
//! confirmed withdraw, under a new generation, with no retry used (V46,
//! V71); a lost node's queued job is requeued with no retry and no health
//! strike (V56); a tenant node gets no node admin (V52); `k2 compute
//! queue` redaction; the `servers` verbs (held add, one pending at a time,
//! the binding never shown, refusals) and the mirrored list.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::path::PathBuf;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_core::compute::proto::frames::{
    Availability, Caps, Control, Frame, Free, JobState, Offer, StateUpdate, CANCEL_IF_QUEUED, CANCEL_WITHDRAW,
};
use k2_core::compute::proto::share::{
    JobClass, QueueEntry, QueuePos, QueueView, Role, ServerStatus, ShareLimits, ShareView, Trust, FEATURE_QUEUE, FEATURE_SERVERS,
};
use k2_core::compute::store;
use k2_core::session::SessionId;
use k2_daemon::compute_ws::{self, TestConn};
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;
use rusqlite::params;
use serde_json::{json, Value};

static TEST_LOCK: StdMutex<()> = StdMutex::new(());
const OWNER: &str = "owner-token-compute-shared-v1";

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
    let home = PathBuf::from(format!("/tmp/k2cs-{:x}", nanos % 0xffff_ffff));
    std::fs::create_dir_all(&home).unwrap();
    let home = std::fs::canonicalize(&home).unwrap();
    let env = Env { prev_home: std::env::var_os("HOME"), home: home.clone(), _guard: guard };
    std::env::set_var("HOME", &home);
    std::env::set_var("K2_COMPUTE", "1");
    // The ticker sleeps: every pass in these tests is called by hand.
    k2_core::compute::retry::set_tuning_for_tests(k2_core::compute::retry::Tuning { tick_millis: 3_600_000, any_hold_secs: 0, ..Default::default() });
    env
}

// ── HTTP over loopback ───────────────────────────────────────────────

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

fn code(r: &Resp) -> String {
    j(r)["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {}", r.body)).to_string()
}

// ── fixtures ─────────────────────────────────────────────────────────

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

/// An active (confirmed) node row, granted to `ws`.
fn active_node(id: &str, name: &str, os: &str, ws: &Ws) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let labels = std::collections::BTreeMap::from([("os".to_string(), os.to_string())]);
    store::insert_pending_node(&conn, id, name, &format!("{id:0>64}"), "pem", &labels, "owner", "123456", 1).expect("node");
    store::confirm_node(&conn, id, "owner", 2).expect("confirm");
    store::set_grant(&conn, id, &ws.id, &store::GrantLimits::default(), "owner", 3).expect("grant");
}

fn offer(features: &[&str], running: u32, share: Option<ShareView>) -> Offer {
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
        running,
        parallel_max: 2,
        refusal: None,
        labels: [("os".to_string(), "linux".to_string())].into(),
        tools: Default::default(),
        protocol: 3,
        boot_id: "b".into(),
        ledger_id: "l".into(),
        node_version: "0.46.3".into(),
        features: features.iter().map(|f| f.to_string()).collect(),
    }
}

/// A 0.46.2 node: protocol 2 (it still takes jobs: JOBS_MIN_PROTOCOL).
fn v0462(mut o: Offer) -> Offer {
    o.protocol = 2;
    o.node_version = "0.46.2".into();
    o
}

/// The node says it is full (`parallel_full`, node-wide).
fn full(mut o: Offer) -> Offer {
    o.refusal = Some(k2_core::compute::proto::frames::Refusal::new("parallel_full", "2 of 2 running"));
    o
}

/// This server's share on a shared node: a tenant at slot 1 of 2 servers.
fn view(role: Role, running: u32, queued: u32) -> ShareView {
    ShareView {
        role,
        slot: if role == Role::Owner { 0 } else { 1 },
        shared: true,
        servers: 2,
        owner_label: (role == Role::Tenant).then(|| "rosson.k2.dev".to_string()),
        state: ServerStatus::Active,
        limits: ShareLimits::new_server(Trust::Own, false),
        running,
        queued,
        cpu_secs_today: 3600,
        expires_in_secs: Some(7 * 86_400),
        quotes: vec![],
    }
}

fn row(job: &str) -> store::JobRow {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::job_by_id(&conn, job).expect("job row")
}

fn state(job: &str, generation: u32, st: JobState, reason: Option<&str>, queue: Option<QueuePos>) -> Frame {
    Frame::State(StateUpdate {
        queue,
        job_id: job.into(),
        generation,
        state: st,
        reason: reason.map(str::to_string),
        detail: None,
        at: k2_core::compute::now(),
        exit: (st == JobState::Done).then(|| k2_core::compute::proto::frames::ExitInfo { code: Some(0), signal: None }),
        src: None,
        log_seq: 0,
        outputs: None,
    })
}

fn assigns(frames: &[Frame]) -> Vec<(String, u32, k2_core::compute::proto::frames::Plan)> {
    frames
        .iter()
        .filter_map(|f| match f {
            Frame::Assign(a) => Some((a.fence.job_id.clone(), a.fence.generation, a.plan.clone())),
            _ => None,
        })
        .collect()
}

fn cancels(frames: &[Frame]) -> Vec<(String, u32, String)> {
    frames
        .iter()
        .filter_map(|f| match f {
            Frame::Cancel(c) => Some((c.job_id.clone(), c.generation, c.reason.clone())),
            _ => None,
        })
        .collect()
}

async fn run(port: u16, token: &str, body: Value) -> Resp {
    let mut b = body;
    b["argv"] = json!(["true"]);
    b["noSource"] = json!(true);
    b["clientJobId"] = json!(uuid::Uuid::new_v4().simple().to_string());
    post(port, "/cli/compute/run", token, b).await
}

fn events(kind: &str, job: &str) -> usize {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT COUNT(*) FROM compute_events WHERE kind = ?1 AND job_id = ?2", params![kind, job], |r| r.get::<_, i64>(0)).unwrap() as usize
}

// ── the tests ────────────────────────────────────────────────────────

/// Solo nodes behave exactly as today (§1 goal 4, V66): no share fields in
/// `nodes`/job JSON, no protocol 3 plan fields to an older node, and a
/// full node still keeps the job on this server.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn solo_nodes_keep_todays_path() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "solo");
    active_node("solo-old", "solo-old", "linux", &ws);
    active_node("solo-new", "solo-new", "linux", &ws);
    // A 0.46.2 node (no `queue`), idle.
    let mut old = compute_ws::test_attach("solo-old", v0462(offer(&["artifacts", "stall"], 0, None)));
    let r = run(d.port, OWNER, json!({"node": "solo-old", "workspace": ws.id})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = j(&r);
    let job = v["job"]["id"].as_str().unwrap().to_string();
    for k in ["class", "nodeQueued", "queue"] {
        assert!(v["job"].get(k).is_none(), "solo job JSON grew {k}: {}", v["job"]);
    }
    let a = assigns(&old.frames());
    assert_eq!(a.len(), 1, "assigned at once on an idle node");
    let p = &a[0].2;
    assert_eq!((p.class, &p.session_id, p.est_secs, p.ws_max_running), (None, &None, None, None), "no protocol 3 fields to an older node");
    assert_eq!(row(&job).plan_digest, p.digest().unwrap(), "the digest an older node checks");
    // A 0.46.3 node that nobody shares, full: the job waits here, as today.
    let mut new = compute_ws::test_attach("solo-new", full(offer(&["artifacts", "stall", FEATURE_QUEUE], 2, None)));
    let r = run(d.port, OWNER, json!({"node": "solo-new", "workspace": ws.id})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let queued = j(&r)["job"]["id"].as_str().unwrap().to_string();
    assert!(assigns(&new.frames()).is_empty(), "a full solo node gets no assign");
    let q = row(&queued);
    assert_eq!((q.state.as_str(), q.node_queued), ("queued", false));
    let r = get(d.port, "/cli/compute/nodes", OWNER, "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let all = j(&r);
    let solo: Vec<&Value> = all["nodes"].as_array().unwrap().iter().filter(|n| n["name"].as_str().is_some_and(|x| x.starts_with("solo-"))).collect();
    assert_eq!(solo.len(), 2);
    for n in solo {
        assert!(n.get("share").is_none() && n.get("role").is_none(), "solo node JSON unchanged: {n}");
    }
    // `queue` on a solo node: this server's own jobs there.
    let r = get(d.port, "/cli/compute/queue", OWNER, "node=solo-new").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let qv = j(&r);
    assert_eq!((qv["from"].as_str(), qv["shared"].as_bool()), (Some("server"), Some(false)));
    assert_eq!(qv["entries"].as_array().unwrap().len(), 1);
    // `servers` on a solo node: just this server.
    let r = get(d.port, "/cli/compute/nodes/servers", OWNER, "node=solo-new").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(j(&r)["servers"][0]["solo"], true);
    compute_ws::test_detach("solo-old");
    compute_ws::test_detach("solo-new");
}

/// §6 named runs, §4.2 what you see, V57 the no-force cancel.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_named_run_goes_into_a_shared_nodes_queue() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "tenant");
    active_node("shr-1", "k2n2-m1", "linux", &ws);
    // Busy as far as the node's own slots go: on a shared node the job is
    // sent anyway, and the node decides.
    let mut node = compute_ws::test_attach("shr-1", full(offer(&["artifacts", "stall", FEATURE_QUEUE, FEATURE_SERVERS], 2, Some(view(Role::Tenant, 0, 0)))));
    {
        let db = k2_core::db::shared();
        let n = store::node_by_id(&db.lock(), "shr-1").unwrap();
        assert!(n.is_tenant() && n.shared, "the offer's share is mirrored");
        assert_eq!(n.owner_label.as_deref(), Some("rosson.k2.dev"));
    }
    let r = get(d.port, "/cli/compute/nodes", OWNER, "").await;
    let n = j(&r)["nodes"].as_array().unwrap().iter().find(|n| n["name"] == "k2n2-m1").cloned().unwrap();
    assert_eq!(n["share"]["role"], "tenant");
    assert_eq!(n["share"]["ownerLabel"], "rosson.k2.dev");
    assert_eq!(n["share"]["servers"], 2);
    assert!(n["share"]["line"].as_str().unwrap().starts_with("running 0/1 · queued 0/10"), "{}", n["share"]);

    // Submit-time share checks (exit 3).
    let r = run(d.port, OWNER, json!({"node": "k2n2-m1", "workspace": ws.id, "cpus": 5})).await;
    assert_eq!((r.status, code(&r).as_str()), (409, "exceeds_share_caps"), "{}", r.body);
    let r = run(d.port, OWNER, json!({"node": "k2n2-m1", "workspace": ws.id, "gate": true})).await;
    assert_eq!((r.status, code(&r).as_str()), (409, "gate_not_allowed"), "{}", r.body);
    let r = run(d.port, OWNER, json!({"node": "k2n2-m1", "workspace": ws.id, "exclusive": true})).await;
    assert_eq!((r.status, code(&r).as_str()), (409, "exclusive_not_allowed"), "{}", r.body);
    assert!(assigns(&node.frames()).is_empty());

    // --plan shows the share and the quote.
    let r = post(d.port, "/cli/compute/run", OWNER, json!({"node": "k2n2-m1", "workspace": ws.id, "argv": ["true"], "noSource": true, "plan": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let p = j(&r);
    assert_eq!((p["shared"].as_bool(), p["owner"].as_str(), p["class"].as_str()), (Some(true), Some("rosson.k2.dev"), Some("interactive")));
    assert_eq!(p["yourShare"]["role"], "tenant");
    assert_eq!(p["quote"]["startsNow"], true, "running 0/1, nothing queued, no quote yet");

    let r = run(d.port, OWNER, json!({"node": "k2n2-m1", "workspace": ws.id, "detach": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let job = j(&r)["job"]["id"].as_str().unwrap().to_string();
    let a = assigns(&node.frames());
    assert_eq!(a.len(), 1, "sent into the node's queue at once");
    assert_eq!(a[0].2.class, Some(JobClass::Batch), "--detach is batch");
    assert_eq!(a[0].2.ws_max_running, Some(1), "the workspace cap travels in the plan");
    assert_eq!(row(&job).class, "batch");

    // The node queues it: shown as queued at the node's place.
    compute_ws::test_frame("shr-1", state(&job, 1, JobState::Queued, Some("node_queued"), Some(QueuePos { pos: 2, eta_secs: Some(360), eta_p90_secs: Some(600), blocked_by: Some("fair_share_wait".into()) })));
    let rj = row(&job);
    assert_eq!((rj.state.as_str(), rj.node_queued, rj.node_pos), ("assigned", true, Some(2)));
    let r = get(d.port, "/cli/compute/jobs/get", OWNER, &format!("job={job}")).await;
    let g = j(&r);
    assert_eq!((g["state"].as_str(), g["nodeQueued"].as_bool()), (Some("queued"), Some(true)), "{g}");
    let msg = g["queue"]["message"].as_str().unwrap();
    assert_eq!(msg, "queued on k2n2-m1, position 2, ETA ~6m (fair share: another server's jobs run first)");

    // Cancel without --force: asks the node; the start wins → job_started.
    let r = post(d.port, "/cli/compute/jobs/cancel", OWNER, json!({"job": job, "ifState": "queued"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(j(&r)["state"], "cancel_requested");
    assert_eq!(cancels(&node.frames()), vec![(job.clone(), 1, CANCEL_IF_QUEUED.to_string())]);
    compute_ws::test_frame("shr-1", state(&job, 1, JobState::Running, None, None));
    let rj = row(&job);
    assert_eq!((rj.state.as_str(), rj.reason.as_deref(), rj.node_queued), ("running", Some("job_started"), false));
    assert_eq!(events("job_cancel_lost", &job), 1);
    compute_ws::test_frame("shr-1", state(&job, 1, JobState::Done, None, None));
    assert_eq!(row(&job).state, "done");

    // A no-force cancel the node answers before the start: cancelled.
    let r = run(d.port, OWNER, json!({"node": "k2n2-m1", "workspace": ws.id})).await;
    let job2 = j(&r)["job"]["id"].as_str().unwrap().to_string();
    assert_eq!(assigns(&node.frames()).len(), 1);
    compute_ws::test_frame("shr-1", state(&job2, 1, JobState::Queued, Some("node_queued"), Some(QueuePos { pos: 1, eta_secs: None, eta_p90_secs: None, blocked_by: None })));
    let r = post(d.port, "/cli/compute/jobs/cancel", OWNER, json!({"job": job2, "ifState": "queued"})).await;
    assert_eq!(j(&r)["state"], "cancel_requested", "{}", r.body);
    compute_ws::test_frame("shr-1", state(&job2, 1, JobState::Cancelled, Some("cancelled"), None));
    let rj = row(&job2);
    assert_eq!((rj.state.as_str(), rj.generation), ("cancelled", 1), "a cancel the user asked for ends it");
    compute_ws::test_detach("shr-1");
}

/// V46 + V71: `--any` leaves a shared node only after the node confirms
/// the withdraw; it comes back under a new generation, with no retry.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn any_moves_only_after_a_confirmed_withdraw_under_a_new_generation() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "mover");
    active_node("mv-a", "mv-a", "linux", &ws);
    active_node("mv-b", "mv-b", "linux", &ws);
    // A: shared, our share's running slot taken (no quote: not "now").
    let mut a = compute_ws::test_attach("mv-a", offer(&["artifacts", "stall", FEATURE_QUEUE], 1, Some(view(Role::Tenant, 1, 0))));
    let r = run(d.port, OWNER, json!({"any": true, "workspace": ws.id})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let job = j(&r)["job"]["id"].as_str().unwrap().to_string();
    assert_eq!(row(&job).node_id, "mv-a");
    assert_eq!(assigns(&a.frames()).len(), 1, "any_hold_secs = 0: into A's queue at once");
    compute_ws::test_frame("mv-a", state(&job, 1, JobState::Queued, Some("node_queued"), Some(QueuePos { pos: 3, eta_secs: Some(900), eta_p90_secs: None, blocked_by: None })));
    // B comes online, idle (a solo 0.46.2 node).
    let mut b = compute_ws::test_attach("mv-b", v0462(offer(&["artifacts", "stall"], 0, None)));
    assert!(assigns(&b.frames()).is_empty(), "nothing moves before a withdraw");
    k2_daemon::compute_place::sweep(k2_core::compute::now());
    assert_eq!(cancels(&a.frames()), vec![(job.clone(), 1, CANCEL_WITHDRAW.to_string())], "withdraw first");
    let rj = row(&job);
    assert_eq!((rj.node_id.as_str(), rj.reason.as_deref(), rj.withdraws), ("mv-a", Some("withdraw_requested"), 1));
    // No second withdraw while one is in flight.
    k2_daemon::compute_place::sweep(k2_core::compute::now());
    assert!(cancels(&a.frames()).is_empty());
    // The node confirms: a new generation, moved to B, never "cancelled".
    compute_ws::test_frame("mv-a", state(&job, 1, JobState::Cancelled, Some("withdrawn"), None));
    let rj = row(&job);
    assert_eq!((rj.node_id.as_str(), rj.generation, rj.attempt, rj.ctl.retries_used), ("mv-b", 2, 1, 0), "{rj:?}");
    assert!(!rj.is_terminal());
    let to_b = assigns(&b.frames());
    assert_eq!(to_b.len(), 1);
    assert_eq!((to_b[0].0.as_str(), to_b[0].1), (job.as_str(), 2));
    assert_eq!(to_b[0].2.class, None, "B offers no queue: no protocol 3 fields");
    assert_eq!(events("withdraw", &job), 1);
    // A late snapshot of the old generation changes nothing.
    compute_ws::test_frame("mv-a", state(&job, 1, JobState::Cancelled, Some("withdrawn"), None));
    assert_eq!(row(&job).generation, 2);
    compute_ws::test_detach("mv-a");
    compute_ws::test_detach("mv-b");
}

/// V56: a node lost while holding a queued job: a new generation, no retry
/// used, no health strike. V57/V47 answer-owed cancels end there too.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lost_nodes_queued_jobs_are_requeued_without_a_retry_or_a_strike() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "lost");
    active_node("lost-1", "lost-1", "linux", &ws);
    let mut n = compute_ws::test_attach("lost-1", offer(&[FEATURE_QUEUE], 2, Some(view(Role::Owner, 0, 0))));
    let mut jobs = Vec::new();
    for _ in 0..3 {
        let r = run(d.port, OWNER, json!({"node": "lost-1", "workspace": ws.id, "retries": 0, "queueTimeoutSecs": 0})).await;
        assert_eq!(r.status, 200, "{}", r.body);
        jobs.push(j(&r)["job"]["id"].as_str().unwrap().to_string());
    }
    assert_eq!(assigns(&n.frames()).len(), 3);
    for id in &jobs {
        compute_ws::test_frame("lost-1", state(id, 1, JobState::Queued, Some("node_queued"), None));
    }
    // One of them had a no-force cancel waiting for the node's answer.
    let r = post(d.port, "/cli/compute/jobs/cancel", OWNER, json!({"job": jobs[2], "ifState": "queued"})).await;
    assert_eq!(j(&r)["state"], "cancel_requested");
    compute_ws::test_detach("lost-1");
    // Far past node_lost_secs (the ticker noted this machine awake).
    k2_daemon::compute_place::note_tick(k2_core::compute::now());
    k2_daemon::compute_place::sweep(k2_core::compute::now() + 10_000);
    for id in &jobs[..2] {
        let rj = row(id);
        assert_eq!((rj.state.as_str(), rj.generation, rj.attempt, rj.ctl.retries_used, rj.node_queued), ("queued", 2, 1, 0, false), "{id}: {rj:?}");
        assert!(rj.ctl.failed_attempts.is_empty(), "no failed attempt recorded");
        assert_eq!(events("job_requeue", id), 1);
        assert_eq!(events("job_lost", id), 0, "never interrupted/node_lost");
    }
    assert_eq!(row(&jobs[2]).state, "cancelled", "the owed cancel ends it: the node can't start it any more (V47)");
    assert!(k2_daemon::compute_place::set_aside_until("lost-1", k2_core::compute::now()).is_none(), "no health strike");
    // Back online: the requeued jobs go again under generation 2.
    let mut n = compute_ws::test_attach("lost-1", offer(&[FEATURE_QUEUE], 0, Some(view(Role::Owner, 0, 0))));
    let again = assigns(&n.frames());
    assert_eq!(again.iter().map(|a| a.1).collect::<Vec<_>>(), vec![2, 2]);
    compute_ws::test_detach("lost-1");
}

/// V52: a node another server owns gets no node admin from here; its
/// admin frames are dropped; its servers list is this server's share.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tenant_node_gets_no_node_admin() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "tadmin");
    active_node("ten-1", "ten-1", "linux", &ws);
    let _n = compute_ws::test_attach("ten-1", offer(&[FEATURE_QUEUE, "admin", "health", "self-update"], 0, Some(view(Role::Tenant, 0, 0))));
    let r = post(d.port, "/cli/compute/nodes/policy/set", OWNER, json!({"node": "ten-1", "set": {"mem_cap_gb": 4}})).await;
    assert_eq!((r.status, code(&r).as_str()), (403, "not_node_owner"), "{}", r.body);
    let r = post(d.port, "/cli/compute/nodes/servers/pause", OWNER, json!({"node": "ten-1", "server": "x.k2.dev"})).await;
    assert_eq!((r.status, code(&r).as_str()), (403, "not_node_owner"), "{}", r.body);
    let r = get(d.port, "/cli/compute/nodes/servers", OWNER, "node=ten-1").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!((j(&r)["role"].as_str(), j(&r)["owner"].as_str()), (Some("tenant"), Some("rosson.k2.dev")));
    let h = k2_core::compute::proto::health::Health::default();
    compute_ws::test_frame("ten-1", Frame::Health(h));
    let db = k2_core::db::shared();
    let conn = db.lock();
    assert!(k2_core::compute::admin::health(&conn, "ten-1").is_none(), "a tenant's health frame is never stored");
    let dropped: i64 = conn.query_row("SELECT COUNT(*) FROM compute_events WHERE kind = 'tenant_admin_frame_dropped' AND node_id = 'ten-1'", [], |r| r.get(0)).unwrap();
    assert_eq!(dropped, 1);
    drop(conn);
    compute_ws::test_detach("ten-1");
}

/// §7 "What each viewer sees": other servers' entries stay redacted; this
/// server's own carry their short id and command; another workspace's
/// passport sees ours only by size.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn queue_shows_other_servers_redacted() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "qview");
    let other = seed_ws(&env, "qother");
    active_node("q-1", "q-1", "linux", &ws);
    {
        let db = k2_core::db::shared();
        store::set_grant(&db.lock(), "q-1", &other.id, &store::GrantLimits::default(), "owner", 3).unwrap();
    }
    let mut n = compute_ws::test_attach("q-1", offer(&[FEATURE_QUEUE], 0, Some(view(Role::Tenant, 0, 0))));
    let r = run(d.port, OWNER, json!({"node": "q-1", "workspace": ws.id})).await;
    let job = j(&r)["job"]["id"].as_str().unwrap().to_string();
    let _ = n.frames();
    let answer = |n: &mut TestConn, job: &str| {
        let rid = loop {
            if let Some(Frame::QueueGet(g)) = n.frames().into_iter().find(|f| matches!(f, Frame::QueueGet(_))) {
                break g.request_id;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let other = QueueEntry {
            pos: 1,
            mine: false,
            class: JobClass::Gate,
            state: JobState::Running,
            cpu_millis: 8000,
            mem_bytes: 0,
            eta_secs: Some(360),
            eta_p90_secs: None,
            blocked_by: None,
            job_id: None,
            generation: None,
            workspace_id: None,
            session_id: None,
            server_label: None,
        };
        let mine = QueueEntry { pos: 2, mine: true, class: JobClass::Interactive, state: JobState::Queued, cpu_millis: 4000, job_id: Some(job.to_string()), generation: Some(1), ..other.clone() };
        compute_ws::test_frame("q-1", Frame::QueueView(QueueView { request_id: rid, at: 1, entries: vec![other, mine], totals: vec![] }));
    };
    let pending = tokio::spawn(get(d.port, "/cli/compute/queue", OWNER, "node=q-1"));
    let job2 = job.clone();
    let mut n = tokio::task::spawn_blocking(move || {
        answer(&mut n, &job2);
        n
    })
    .await
    .unwrap();
    let r = pending.await.unwrap();
    assert_eq!(r.status, 200, "{}", r.body);
    let q = j(&r);
    assert_eq!(q["from"], "node");
    let e = q["entries"].as_array().unwrap();
    assert_eq!((e[0]["mine"].as_bool(), e[0]["short"].is_null(), e[0]["server"].is_null()), (Some(false), true, true), "{}", e[0]);
    assert_eq!(e[1]["short"].as_str(), Some(&job[..8]));
    assert_eq!(e[1]["argv"], "true");
    // Another workspace's agent: our job by size only.
    let tok = passport(&other);
    let port = d.port;
    let pending = tokio::spawn(async move { get(port, "/cli/compute/queue", &tok, "node=q-1").await });
    let job3 = job.clone();
    tokio::task::spawn_blocking(move || answer(&mut n, &job3)).await.unwrap();
    let r = pending.await.unwrap();
    assert_eq!(r.status, 200, "{}", r.body);
    let e1 = j(&r)["entries"][1].clone();
    assert!(e1["short"].is_null() && e1["argv"].is_null() && e1["otherWorkspace"] == true, "{e1}");
    compute_ws::test_detach("q-1");
}

/// §4.1, §9: `servers add` is held for the owner (never sent without an
/// approval), one at a time; the code's binding never shows; nodes that
/// can't take a second server say why; revoke is the owner's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn servers_add_is_held_one_at_a_time_and_hides_the_binding() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "srv");
    active_node("own-1", "own-1", "linux", &ws);
    active_node("own-old", "own-old", "linux", &ws);
    active_node("own-mac", "own-mac", "macos", &ws);
    let feats = ["admin", "health", "job-wall", FEATURE_SERVERS, FEATURE_QUEUE, "tenant-wall"];
    let mut n = compute_ws::test_attach("own-1", offer(&feats, 0, None));
    let _old = compute_ws::test_attach("own-old", offer(&["admin", "job-wall"], 0, None));
    let _mac = compute_ws::test_attach("own-mac", offer(&["admin", "job-wall", FEATURE_SERVERS], 0, None));
    let code_fp = "ABCDE-FGHJK.0123456789abcdef";
    let add = |node: &str, trust: &str| json!({"node": node, "url": "https://baden.k2.dev", "code": code_fp, "trust": trust});
    let r = post(d.port, "/cli/compute/nodes/servers/add", OWNER, add("own-old", "own")).await;
    assert_eq!((r.status, code(&r).as_str()), (409, "node_too_old"), "{}", r.body);
    let r = post(d.port, "/cli/compute/nodes/servers/add", OWNER, add("own-mac", "team")).await;
    assert_eq!((r.status, code(&r).as_str()), (409, "team_needs_tenant_wall"), "{}", r.body);
    let r = post(d.port, "/cli/compute/nodes/servers/add", OWNER, json!({"node": "own-1", "url": "https://b.k2.dev", "code": "nope"})).await;
    assert_eq!(r.status, 400, "{}", r.body);
    let r = post(d.port, "/cli/compute/nodes/servers/add", OWNER, add("own-1", "team")).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = j(&r);
    assert_eq!(v["held"], true, "always held for the owner's approval");
    assert!(v["warning"].as_str().unwrap().contains("Only add people you trust"));
    assert!(!r.body.contains(&k2_core::compute::proto::pairing::code_binding("ABCDEFGHJK")), "the binding never shows");
    assert!(r.body.contains("[redacted]"));
    let req = v["request"]["id"].as_str().unwrap().to_string();
    assert!(n.frames().iter().all(|f| !matches!(f, Frame::Admin(_))), "nothing is sent before the approval");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let rr = k2_core::compute::admin::request_by_id(&conn, &req).unwrap();
        assert_eq!(rr.class, "privileged");
        assert_eq!(rr.expires_at - rr.proposed_at, 600, "the binding lives 10 minutes");
        assert_eq!(rr.body["args"]["for_secs"], 7 * 86_400, "team: 7 days by default");
    }
    let r = post(d.port, "/cli/compute/nodes/servers/add", OWNER, add("own-1", "own")).await;
    assert_eq!((r.status, code(&r).as_str()), (409, "server_add_pending"), "{}", r.body);
    let r = get(d.port, "/cli/compute/admin/requests", OWNER, "state=open").await;
    assert!(!r.body.contains(&k2_core::compute::proto::pairing::code_binding("ABCDEFGHJK")), "listings hide it too");
    // Revoke: the owner only, never a passport.
    let r = post(d.port, "/cli/compute/nodes/servers/revoke", &passport(&ws), json!({"node": "own-1", "server": "baden.k2.dev"})).await;
    assert_eq!((r.status, code(&r).as_str()), (403, "owner_only"), "{}", r.body);

    // The mirror: `servers` asks the node (servers.list) and keeps the list.
    let pending = tokio::spawn(get(d.port, "/cli/compute/nodes/servers", OWNER, "node=own-1&refresh=1"));
    let n = tokio::task::spawn_blocking(move || {
        let cmd_id = loop {
            let f = n.frames().into_iter().find_map(|f| match f {
                Frame::Admin(a) => Some(a),
                _ => None,
            });
            if let Some(a) = f {
                let body = k2_core::compute::proto::crypto::unb64(&a.cmd.body_b64).unwrap();
                let c: k2_core::compute::proto::admin::Command = serde_json::from_slice(&body).unwrap();
                assert_eq!(c.verb, "servers.list");
                break c.cmd_id;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let owner = serde_json::to_value(ShareLimits::owner(2, 8, 16, 100, k2_core::compute::proto::share::Network::On)).unwrap();
        let mut tenant = serde_json::to_value(ShareLimits::new_server(Trust::Team, false)).unwrap();
        tenant["slot"] = json!(1);
        tenant["label"] = json!("baden.k2.dev");
        tenant["state"] = json!("pending");
        tenant["trust"] = json!("team");
        tenant["fp_prefix"] = json!("0123456789abcdef");
        tenant["sas"] = json!("482913");
        let old = json!({"slot": 2, "label": "old.k2.dev", "state": "refused", "trust": "own", "fp_prefix": "fedcba9876543210", "refusal": "server_too_old", "limits": ShareLimits::new_server(Trust::Own, false)});
        let result = json!({"servers": [{"slot": 0, "label": "rosson.k2.dev", "state": "active", "trust": "own", "limits": owner}, tenant, old]});
        compute_ws::test_frame(
            "own-1",
            Frame::AdminResult(k2_core::compute::proto::admin::AdminResult {
                cmd_id,
                state: "done".into(),
                code: None,
                message: None,
                exit: None,
                stdout_tail: String::new(),
                stderr_tail: String::new(),
                audit_seq: None,
                digest: String::new(),
                result,
            }),
        );
        n
    })
    .await
    .unwrap();
    let r = pending.await.unwrap();
    assert_eq!(r.status, 200, "{}", r.body);
    let s = j(&r);
    let list = s["servers"].as_array().unwrap();
    assert_eq!(list.len(), 3, "{s}");
    assert_eq!((list[2]["state"].as_str(), list[2]["refusal"].as_str()), (Some("refused"), Some("server_too_old")));
    assert_eq!((list[1]["state"].as_str(), list[1]["sas"].as_str()), (Some("pending"), Some("482 913")));
    assert_eq!((list[0]["weight"].as_u64(), list[1]["weight"].as_u64(), list[0]["sas"].is_null(), list[1]["refusal"].is_null()), (Some(1), Some(1), true, true), "--json carries weight, SAS and refusal");
    assert_eq!(s["requests"].as_array().unwrap().len(), 1, "the held add shows with the list");
    {
        let db = k2_core::db::shared();
        assert_eq!(store::node_servers(&db.lock(), "own-1").len(), 3);
    }
    drop(n);
    for id in ["own-1", "own-old", "own-mac"] {
        compute_ws::test_detach(id);
    }
}
