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
    let home = k2_core::test_env::short_tmp(format!("k2cs-{:x}", nanos % 0xffff_ffff));
    std::fs::create_dir_all(&home).unwrap();
    let home = dunce::canonicalize(&home).unwrap();
    let env = Env { prev_home: std::env::var_os("HOME"), home: home.clone(), _guard: guard };
    std::env::set_var("HOME", &home);
    std::env::set_var("K2_COMPUTE", "1");
    // The ticker sleeps: every pass in these tests is called by hand.
    k2_core::compute::retry::set_tuning_for_tests(k2_core::compute::retry::Tuning { tick_millis: 3_600_000, ..Default::default() });
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
        caps: Caps { cpu_millis: 8000, mem_bytes: 32 << 30, disk_budget_bytes: 100 << 30, hard: true, gpus: 0 },
        free: Free { cpu_millis: 8000, mem_bytes: 16 << 30, disk_free_bytes: 500 << 30, gpus: 0 },
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
        features: features.iter().map(|f| f.to_string()).collect(), host: None, pkg: None, sim: None,
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
    // 0.46.6: jobs say their --mem (else a mem_defaulted note joins the
    // notices these tests count).
    if b.get("memBytes").is_none() {
        b["memBytes"] = json!(1u64 << 30);
    }
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
    active_node("shr-1", "rig2-mac", "linux", &ws);
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
    let n = j(&r)["nodes"].as_array().unwrap().iter().find(|n| n["name"] == "rig2-mac").cloned().unwrap();
    assert_eq!(n["share"]["role"], "tenant");
    assert_eq!(n["share"]["ownerLabel"], "rosson.k2.dev");
    assert_eq!(n["share"]["servers"], 2);
    assert!(n["share"]["line"].as_str().unwrap().starts_with("running 0/1 · queued 0/10"), "{}", n["share"]);

    // Submit-time share checks (exit 3).
    let r = run(d.port, OWNER, json!({"node": "rig2-mac", "workspace": ws.id, "cpus": 5})).await;
    assert_eq!((r.status, code(&r).as_str()), (409, "exceeds_share_caps"), "{}", r.body);
    let r = run(d.port, OWNER, json!({"node": "rig2-mac", "workspace": ws.id, "gate": true})).await;
    assert_eq!((r.status, code(&r).as_str()), (409, "gate_not_allowed"), "{}", r.body);
    let r = run(d.port, OWNER, json!({"node": "rig2-mac", "workspace": ws.id, "exclusive": true, "why": "needs the whole node"})).await;
    assert_eq!((r.status, code(&r).as_str()), (409, "exclusive_not_allowed"), "{}", r.body);
    assert!(assigns(&node.frames()).is_empty());

    // --plan shows the share and the quote.
    let r = post(d.port, "/cli/compute/run", OWNER, json!({"node": "rig2-mac", "workspace": ws.id, "argv": ["true"], "noSource": true, "plan": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let p = j(&r);
    assert_eq!((p["shared"].as_bool(), p["owner"].as_str(), p["class"].as_str()), (Some(true), Some("rosson.k2.dev"), Some("interactive")));
    assert_eq!(p["yourShare"]["role"], "tenant");
    assert_eq!(p["quote"]["startsNow"], true, "running 0/1, nothing queued, no quote yet");

    let r = run(d.port, OWNER, json!({"node": "rig2-mac", "workspace": ws.id, "detach": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let job = j(&r)["job"]["id"].as_str().unwrap().to_string();
    let a = assigns(&node.frames());
    assert_eq!(a.len(), 1, "sent into the node's queue at once");
    // Job tiers (0.46.5, G4/T11): --detach no longer lowers a job; the
    // class follows the tier (Normal = interactive). This node doesn't
    // offer `tiers`, so the plan carries no tier (its digest stays old).
    assert_eq!(a[0].2.class, Some(JobClass::Interactive), "--detach is plain Normal now");
    assert_eq!(a[0].2.tier, None, "no tier to a node without the tiers feature");
    assert_eq!(a[0].2.ws_max_running, Some(1), "the workspace cap travels in the plan");
    assert_eq!(row(&job).class, "interactive");
    assert_eq!(row(&job).tier, "normal");

    // The node queues it: shown as queued at the node's place.
    compute_ws::test_frame("shr-1", state(&job, 1, JobState::Queued, Some("node_queued"), Some(QueuePos { pos: 2, of: None, eta_secs: Some(360), eta_p90_secs: Some(600), blocked_by: Some("fair_share_wait".into()) })));
    let rj = row(&job);
    assert_eq!((rj.state.as_str(), rj.node_queued, rj.node_pos), ("assigned", true, Some(2)));
    let r = get(d.port, "/cli/compute/jobs/get", OWNER, &format!("job={job}")).await;
    let g = j(&r);
    assert_eq!((g["state"].as_str(), g["nodeQueued"].as_bool()), (Some("queued"), Some(true)), "{g}");
    let msg = g["queue"]["message"].as_str().unwrap();
    assert_eq!(msg, "queued on rig2-mac, position 2, ETA ~6m — fair share: another server's jobs run first");
    // `status` / `nodes` count it as queued, never running (0.46.6: a
    // node-queued row stays `assigned`, and `status` showed "run 6/3" on a
    // node running 2).
    let r = get(d.port, "/cli/compute/nodes", OWNER, "detail=1").await;
    let me = j(&r)["nodes"].as_array().unwrap().iter().find(|n| n["name"] == "rig2-mac").unwrap().clone();
    assert_eq!((me["running"].as_u64(), me["queued"].as_u64()), (Some(0), Some(1)), "{me}");
    assert_eq!(me["status"]["running"].as_array().map(Vec::len), Some(0), "{me}");
    let q = me["status"]["queued"].as_array().unwrap();
    assert_eq!(q.len(), 1, "{me}");
    assert_eq!(q[0]["state"], "queued", "{me}");

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
    let r = run(d.port, OWNER, json!({"node": "rig2-mac", "workspace": ws.id})).await;
    let job2 = j(&r)["job"]["id"].as_str().unwrap().to_string();
    assert_eq!(assigns(&node.frames()).len(), 1);
    compute_ws::test_frame("shr-1", state(&job2, 1, JobState::Queued, Some("node_queued"), Some(QueuePos { pos: 1, of: None, eta_secs: None, eta_p90_secs: None, blocked_by: None })));
    let r = post(d.port, "/cli/compute/jobs/cancel", OWNER, json!({"job": job2, "ifState": "queued"})).await;
    assert_eq!(j(&r)["state"], "cancel_requested", "{}", r.body);
    compute_ws::test_frame("shr-1", state(&job2, 1, JobState::Cancelled, Some("cancelled"), None));
    let rj = row(&job2);
    assert_eq!((rj.state.as_str(), rj.generation), ("cancelled", 1), "a cancel the user asked for ends it");

    // The node ends a queued job itself when the share is revoked: a
    // result, said plainly, never retried.
    let r = run(d.port, OWNER, json!({"node": "rig2-mac", "workspace": ws.id})).await;
    let job3 = j(&r)["job"]["id"].as_str().unwrap().to_string();
    assert_eq!(assigns(&node.frames()).len(), 1);
    compute_ws::test_frame("shr-1", state(&job3, 1, JobState::Queued, Some("node_queued"), None));
    compute_ws::test_frame("shr-1", state(&job3, 1, JobState::Cancelled, Some("share_revoked"), None));
    let rj = row(&job3);
    assert_eq!((rj.state.as_str(), rj.reason.as_deref(), rj.generation), ("cancelled", Some("share_revoked"), 1));
    assert!(rj.detail.as_deref().is_some_and(|d| d.contains("removed this server")), "{rj:?}");

    // Clock skew over 120 s shows in `nodes`; under it, nothing.
    let mut skewed = view(Role::Tenant, 0, 0);
    skewed.skew_secs = Some(300);
    compute_ws::test_frame("shr-1", Frame::Offer(offer(&["artifacts", "stall", FEATURE_QUEUE, FEATURE_SERVERS], 0, Some(skewed.clone()))));
    let skew = |v: &Value| v["nodes"].as_array().unwrap().iter().find(|n| n["name"] == "rig2-mac").unwrap()["share"]["skewSecs"].clone();
    let r = get(d.port, "/cli/compute/nodes", OWNER, "").await;
    assert_eq!(skew(&j(&r)), json!(300));
    skewed.skew_secs = Some(-60);
    compute_ws::test_frame("shr-1", Frame::Offer(offer(&["artifacts", "stall", FEATURE_QUEUE, FEATURE_SERVERS], 0, Some(skewed))));
    let r = get(d.port, "/cli/compute/nodes", OWNER, "").await;
    assert!(skew(&j(&r)).is_null(), "under 120 s isn't shown");
    compute_ws::test_detach("shr-1");
}

/// prd-compute-job-tiers-v2 L5 + L6 (T23–T26, the 10-27 night): jobs
/// waiting in a shared node's queue carry their workspace's cap from when
/// they were sent. Raising the grant re-sends them (withdraw, then the
/// same node again with the grant's cap, oldest first): a new generation,
/// no move, no V71 withdraw, no retry. The wait says why, "N of M", and
/// that jobs behind it may start first.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raising_a_grant_resends_node_queued_jobs_with_the_new_cap() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "capper");
    active_node("cap-1", "rig9-vps", "linux", &ws);
    let mut node = compute_ws::test_attach("cap-1", offer(&["artifacts", "stall", FEATURE_QUEUE, FEATURE_SERVERS], 1, Some(view(Role::Tenant, 1, 0))));
    let r = run(d.port, OWNER, json!({"node": "rig9-vps", "workspace": ws.id, "detach": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let old = j(&r)["job"]["id"].as_str().unwrap().to_string();
    let r = run(d.port, OWNER, json!({"node": "rig9-vps", "workspace": ws.id, "detach": true})).await;
    let older2 = j(&r)["job"]["id"].as_str().unwrap().to_string();
    let a = assigns(&node.frames());
    assert_eq!(a.len(), 2);
    assert!(a.iter().all(|x| x.2.ws_max_running == Some(1)), "cap 1 from the grant at send");
    let wait = QueuePos { pos: 1, of: Some(3), eta_secs: None, eta_p90_secs: None, blocked_by: Some("workspace_running_full".into()) };
    compute_ws::test_frame("cap-1", state(&old, 1, JobState::Queued, Some("node_queued"), Some(wait.clone())));
    compute_ws::test_frame("cap-1", state(&older2, 1, JobState::Queued, Some("node_queued"), Some(QueuePos { pos: 2, ..wait })));
    // L6: the wait in words, its place, and that it lets others pass.
    let r = get(d.port, "/cli/compute/jobs/get", OWNER, &format!("job={old}")).await;
    let q = j(&r)["queue"].clone();
    assert_eq!((q["position"].as_i64(), q["of"].as_i64(), q["waitReason"].as_str(), q["holdsQueue"].as_bool()), (Some(1), Some(3), Some("workspace_running_full"), Some(false)), "{q}");
    let line = q["line"].as_str().unwrap();
    assert!(line.starts_with("queued on rig9-vps: 1 of 3 — your workspace runs "), "{line}");
    assert!(line.ends_with("(its grant); jobs behind it may start first"), "{line}");

    // Raise the grant 1 → 3: both are asked back, oldest first. (Both were
    // submitted in the same second: make `old` the older one for certain.)
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE compute_jobs SET created_at = created_at - 10 WHERE id = ?1", params![old]).expect("age old");
    }
    let r = post(d.port, "/cli/compute/grants/set", OWNER, json!({"node": "rig9-vps", "workspace": ws.id, "maxParallel": 3})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(j(&r)["resent"], 2, "{}", r.body);
    assert_eq!(cancels(&node.frames()), vec![(old.clone(), 1, CANCEL_WITHDRAW.to_string()), (older2.clone(), 1, CANCEL_WITHDRAW.to_string())]);
    assert_eq!(row(&old).reason.as_deref(), Some("withdraw_resend"));
    // The same grant again re-sends nothing.
    let r = post(d.port, "/cli/compute/grants/set", OWNER, json!({"node": "rig9-vps", "workspace": ws.id, "maxParallel": 3})).await;
    assert_eq!(j(&r)["resent"], 0, "{}", r.body);
    // The node gives each back: sent again to the SAME node with cap 3.
    compute_ws::test_frame("cap-1", state(&old, 1, JobState::Cancelled, Some("withdrawn"), None));
    compute_ws::test_frame("cap-1", state(&older2, 1, JobState::Cancelled, Some("withdrawn"), None));
    let again = assigns(&node.frames());
    assert_eq!(again.iter().map(|x| (x.0.clone(), x.1)).collect::<Vec<_>>(), vec![(old.clone(), 2), (older2.clone(), 2)], "oldest first, new generation");
    assert!(again.iter().all(|x| x.2.ws_max_running == Some(3)), "the fresh cap");
    let rj = row(&old);
    assert_eq!((rj.node_id.as_str(), rj.generation, rj.attempt, rj.withdraws, rj.ctl.retries_used), ("cap-1", 2, 1, 0, 0), "no move, no V71 count, no retry: {rj:?}");
    assert!(!rj.is_terminal());
    assert_eq!((events("resend", &old), events("withdraw", &old)), (1, 0));

    // Retier (L2, T29) of a job in the node's queue: the same re-send. To
    // Batch on a node without `tiers2` (T28): it then waits on this server
    // until the node is idle (`batch_idle_only`), never sent as `batch`.
    compute_ws::test_frame("cap-1", state(&old, 2, JobState::Queued, Some("node_queued"), Some(QueuePos { pos: 1, of: Some(2), eta_secs: None, eta_p90_secs: None, blocked_by: None })));
    let r = post(d.port, "/cli/compute/jobs/tier", OWNER, json!({"job": old, "tier": "batch"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(j(&r)["resent"], true, "{}", r.body);
    assert_eq!(cancels(&node.frames()), vec![(old.clone(), 2, CANCEL_WITHDRAW.to_string())]);
    compute_ws::test_frame("cap-1", state(&old, 2, JobState::Cancelled, Some("withdrawn"), None));
    assert!(assigns(&node.frames()).is_empty(), "the node runs work: Batch waits on this server");
    let rj = row(&old);
    assert_eq!((rj.state.as_str(), rj.tier.as_str(), rj.class.as_str(), rj.reason.as_deref()), ("queued", "batch", "batch", Some("batch_idle_only")), "{rj:?}");
    compute_ws::test_detach("cap-1");
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
    // Sent while A looked free; A had filled first and queues it.
    let job = queued_any_on(d.port, &ws, "mv-a", &mut a).await;
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

// ── an ask the node never answers (0.46.17) ──────────────────────────

/// This machine has been awake long before `now` (the sweeper's deadlines
/// count from a wake; the ticker notes it, these tests call it).
fn awake_long_before(now: i64) {
    k2_daemon::compute_place::note_tick(now - 100_000);
    k2_daemon::compute_place::note_tick(now - 50_000);
}

/// Back to a machine that woke now (as the lost-node test leaves it).
fn awake_now() {
    k2_daemon::compute_place::note_tick(k2_core::compute::now());
}

const SUPERSEDED: &str = "superseded";

/// The bite (jobs stuck `assigned` / `withdraw_requested` while their node
/// stays connected, 2026-11-21): the node never answers the mover's
/// withdraw. It is asked once more at 60 s; at 180 s K2 settles it itself:
/// the old generation is fenced, the node is told `superseded`, the job
/// goes to the node that can start it, with no retry used. The node sent
/// nothing since the ask, so its session is closed. A late answer and a
/// late start of the old generation change nothing (the start gets a
/// stop): the job never runs twice. Before 0.46.17 the job stayed on A,
/// `withdraw_requested`, for good.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn withdraw_never_answered_is_settled_and_fenced() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "unanswered");
    active_node("ua-a", "ua-a", "linux", &ws);
    active_node("ua-b", "ua-b", "linux", &ws);
    let mut a = compute_ws::test_attach("ua-a", offer(&["artifacts", "stall", FEATURE_QUEUE], 1, Some(view(Role::Tenant, 1, 0))));
    let job = queued_any_on(d.port, &ws, "ua-a", &mut a).await;
    let mut b = compute_ws::test_attach("ua-b", solo(0, 2));
    // The ask goes out after A's last frame (A says nothing more).
    let t0 = k2_core::compute::now() + 2;
    awake_long_before(t0);
    k2_daemon::compute_place::sweep(t0);
    assert_eq!(cancels(&a.frames()), vec![(job.clone(), 1, CANCEL_WITHDRAW.to_string())], "withdraw first");
    let ask = row(&job).ctl.node_ask.expect("the ask is stamped");
    assert_eq!((ask.marker.as_str(), ask.generation, ask.at, ask.resent), ("withdraw_requested", 1, t0, false));
    // Not answered. Within a minute: nothing more.
    k2_daemon::compute_place::sweep(t0 + 30);
    assert!(cancels(&a.frames()).is_empty(), "no second withdraw while the first is young");
    // After a minute: asked once more, and only once.
    k2_daemon::compute_place::sweep(t0 + 61);
    assert_eq!(cancels(&a.frames()), vec![(job.clone(), 1, CANCEL_WITHDRAW.to_string())], "asked once more");
    assert_eq!(events("withdraw_asked_again", &job), 1);
    k2_daemon::compute_place::sweep(t0 + 120);
    assert!(cancels(&a.frames()).is_empty(), "only once more");
    let rj = row(&job);
    assert_eq!((rj.node_id.as_str(), rj.state.as_str(), rj.reason.as_deref(), rj.generation), ("ua-a", "assigned", Some("withdraw_requested"), 1), "still waiting: {rj:?}");
    let r = get(d.port, "/cli/compute/jobs/get", OWNER, &format!("job={job}")).await;
    let note = j(&r)["move"]["note"].as_str().map(str::to_string).unwrap_or_else(|| panic!("a move note: {}", r.body));
    assert!(note.contains("K2 takes it back itself at 3 min"), "{note}");
    // After three minutes: K2 settles it.
    k2_daemon::compute_place::sweep(t0 + 181);
    let rj = row(&job);
    assert_eq!((rj.node_id.as_str(), rj.generation, rj.attempt, rj.ctl.retries_used), ("ua-b", 2, 1, 0), "settled to B under generation 2: {rj:?}");
    assert!(!rj.is_terminal(), "{rj:?}");
    assert!(rj.ctl.failed_attempts.is_empty(), "no retry, no failed attempt");
    assert_eq!(rj.ctl.node_ask, None, "the ask ended with its generation");
    assert_eq!(cancels(&a.frames()), vec![(job.clone(), 1, SUPERSEDED.to_string())], "A is told not to run generation 1");
    assert_eq!(assigns(&b.frames()).iter().map(|x| (x.0.clone(), x.1)).collect::<Vec<_>>(), vec![(job.clone(), 2)]);
    assert_eq!(events("withdraw_unanswered", &job), 1);
    assert_eq!(events("withdraw", &job), 0, "never a confirmed withdraw");
    let rec = rj.ctl.attempts.last().expect("the attempt record").clone();
    assert_eq!(
        (rec.generation, rec.next.as_str(), rec.reason.as_deref(), rec.asked.as_deref()),
        (1, "withdraw_unanswered", Some("withdraw_unanswered"), Some("withdraw_requested")),
        "the outcome, not the pending marker: {rec:?}"
    );
    assert!(k2_daemon::compute_place::set_aside_until("ua-a", t0 + 181).is_none(), "no health strike");
    assert!(!compute_ws::is_online("ua-a"), "A sent nothing since the ask: its session is closed");
    // The follower's words for generation 1: a move, never a failure.
    let r = get(d.port, "/cli/compute/jobs/logs", OWNER, &format!("job={job}&generation=1")).await;
    let l = j(&r);
    let first = &l["attempts"][0];
    assert_eq!((first["generation"].as_i64(), first["nextCode"].as_str()), (Some(1), Some("withdraw_unanswered")), "{l}");
    let moved = first["moved"].as_str().unwrap_or_else(|| panic!("a move: {l}"));
    assert_eq!(moved, "ua-a didn't answer a request to give it back within 3 min; K2 took it back (ua-a is told not to run it)");
    let next = first["next"].as_str().unwrap();
    assert!(next.ends_with("placed it again on ua-b (no retry used)") && !next.contains("infrastructure"), "{next}");

    // A comes back. Its late answer for generation 1 changes nothing.
    let mut a = compute_ws::test_attach("ua-a", offer(&["artifacts", "stall", FEATURE_QUEUE], 0, Some(view(Role::Tenant, 0, 0))));
    compute_ws::test_frame("ua-a", state(&job, 1, JobState::Cancelled, Some("withdrawn"), None));
    let rj = row(&job);
    assert_eq!((rj.node_id.as_str(), rj.generation, rj.state.as_str()), ("ua-b", 2, "assigned"));
    // A late start of generation 1 gets a stop; the job stays B's.
    compute_ws::test_frame("ua-a", state(&job, 1, JobState::Running, None, None));
    assert_eq!(cancels(&a.frames()), vec![(job.clone(), 1, SUPERSEDED.to_string())], "a stale start is stopped at once");
    let rj = row(&job);
    assert_eq!((rj.node_id.as_str(), rj.generation, rj.state.as_str(), rj.started_at), ("ua-b", 2, "assigned", None), "never running on A: {rj:?}");
    compute_ws::test_frame("ua-b", state(&job, 2, JobState::Running, None, None));
    assert_eq!(row(&job).state, "running");
    compute_ws::test_frame("ua-b", state(&job, 2, JobState::Done, None, None));
    let rj = row(&job);
    assert_eq!((rj.state.as_str(), rj.node_id.as_str(), rj.generation), ("done", "ua-b", 2));
    assert_eq!(events("job_start", &job), 1, "it ran once");
    compute_ws::test_detach("ua-a");
    compute_ws::test_detach("ua-b");
    awake_now();
}

/// Never twice, on a live session: the node keeps sending (its answer to
/// the ask alone is lost), so its session stays open after K2 settles the
/// ask. When it then starts the old generation, the live `running` gets a
/// `superseded` stop at once (not only on a reattach's snapshot), and the
/// job's row stays with the new generation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_late_start_of_a_settled_attempt_is_stopped_and_never_runs_twice() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "latestart");
    active_node("ls-a", "ls-a", "linux", &ws);
    active_node("ls-b", "ls-b", "linux", &ws);
    let mut a = compute_ws::test_attach("ls-a", offer(&["artifacts", "stall", FEATURE_QUEUE], 1, Some(view(Role::Tenant, 1, 0))));
    let job = queued_any_on(d.port, &ws, "ls-a", &mut a).await;
    let mut b = compute_ws::test_attach("ls-b", solo(0, 2));
    let t0 = k2_core::compute::now();
    awake_long_before(t0);
    k2_daemon::compute_place::sweep(t0);
    assert_eq!(cancels(&a.frames()), vec![(job.clone(), 1, CANCEL_WITHDRAW.to_string())]);
    // A still talks (a queue position), but its answer never comes.
    compute_ws::test_frame("ls-a", state(&job, 1, JobState::Queued, Some("node_queued"), Some(QueuePos { pos: 1, of: Some(2), eta_secs: None, eta_p90_secs: None, blocked_by: None })));
    assert_eq!(row(&job).reason.as_deref(), Some("withdraw_requested"), "a position keeps the ask");
    k2_daemon::compute_place::sweep(t0 + 61);
    assert_eq!(cancels(&a.frames()).len(), 1, "asked once more");
    k2_daemon::compute_place::sweep(t0 + 181);
    let rj = row(&job);
    assert_eq!((rj.node_id.as_str(), rj.generation, rj.ctl.retries_used), ("ls-b", 2, 0), "{rj:?}");
    assert_eq!(cancels(&a.frames()), vec![(job.clone(), 1, SUPERSEDED.to_string())]);
    assert!(compute_ws::is_online("ls-a"), "A kept talking: its session stays");
    assert_eq!(assigns(&b.frames()).len(), 1);
    // A had started generation 1 meanwhile (its `running` was lost too)
    // and says so now: told to stop, at once.
    compute_ws::test_frame("ls-a", state(&job, 1, JobState::Running, None, None));
    assert_eq!(cancels(&a.frames()), vec![(job.clone(), 1, SUPERSEDED.to_string())], "a live stale start gets a stop");
    let rj = row(&job);
    assert_eq!((rj.node_id.as_str(), rj.generation, rj.state.as_str(), rj.started_at), ("ls-b", 2, "assigned", None), "{rj:?}");
    // Its stop lands: nothing changes here either.
    compute_ws::test_frame("ls-a", state(&job, 1, JobState::Cancelled, Some(SUPERSEDED), None));
    assert_eq!((row(&job).node_id.as_str(), row(&job).generation, row(&job).state.as_str()), ("ls-b", 2, "assigned"));
    assert_eq!(events("job_start", &job), 0, "generation 1 never counted as the job's start");
    compute_ws::test_detach("ls-a");
    compute_ws::test_detach("ls-b");
    awake_now();
}

/// An answer that comes between the second ask and the deadline is the
/// plain move it always was: one generation step, no `superseded`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_answer_before_the_deadline_is_a_plain_move() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "lateok");
    active_node("lo-a", "lo-a", "linux", &ws);
    active_node("lo-b", "lo-b", "linux", &ws);
    let mut a = compute_ws::test_attach("lo-a", offer(&["artifacts", "stall", FEATURE_QUEUE], 1, Some(view(Role::Tenant, 1, 0))));
    let job = queued_any_on(d.port, &ws, "lo-a", &mut a).await;
    let mut b = compute_ws::test_attach("lo-b", solo(0, 2));
    let t0 = k2_core::compute::now() + 2;
    awake_long_before(t0);
    k2_daemon::compute_place::sweep(t0);
    k2_daemon::compute_place::sweep(t0 + 61);
    assert_eq!(cancels(&a.frames()).len(), 2, "the ask and the second ask");
    compute_ws::test_frame("lo-a", state(&job, 1, JobState::Cancelled, Some("withdrawn"), None));
    let rj = row(&job);
    assert_eq!((rj.node_id.as_str(), rj.generation, rj.ctl.retries_used), ("lo-b", 2, 0), "{rj:?}");
    let rec = rj.ctl.attempts.last().expect("record").clone();
    assert_eq!((rec.next.as_str(), rec.reason.as_deref(), rec.asked.as_deref()), ("withdrawn", Some("withdrawn"), Some("withdraw_requested")));
    k2_daemon::compute_place::sweep(t0 + 181);
    assert!(cancels(&a.frames()).is_empty(), "no superseded: nothing to settle");
    assert_eq!(row(&job).generation, 2, "one step, no double");
    assert_eq!(assigns(&b.frames()).len(), 1);
    assert_eq!((events("withdraw", &job), events("withdraw_unanswered", &job)), (1, 0));
    assert!(compute_ws::is_online("lo-a"));
    compute_ws::test_detach("lo-a");
    compute_ws::test_detach("lo-b");
    awake_now();
}

/// A pending ask never blocks `--queue-timeout`: the queue time runs out
/// while the mover's withdraw waits; the ask becomes the timeout's, and
/// when the node never answers it the job ends `failed/queue_timeout`
/// and the node is told `superseded`. Before 0.46.17 the pending marker
/// skipped the timeout check, for good.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pending_withdraw_does_not_block_the_queue_timeout() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "qtimeout");
    active_node("qt-a", "qt-a", "linux", &ws);
    active_node("qt-b", "qt-b", "linux", &ws);
    let mut a = compute_ws::test_attach("qt-a", offer(&["artifacts", "stall", FEATURE_QUEUE], 1, Some(view(Role::Tenant, 1, 0))));
    let job = queued_any_on(d.port, &ws, "qt-a", &mut a).await;
    let t0 = k2_core::compute::now() + 2;
    awake_long_before(t0);
    // Its queue time (10 min) runs out 10 s after the mover asks.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let mut ctl = store::job_by_id(&conn, &job).expect("job").ctl;
        ctl.queue_timeout_secs = Some(600);
        ctl.queued_at = t0 - 590;
        store::set_job_ctl(&conn, &job, &ctl).expect("ctl");
    }
    let _b = compute_ws::test_attach("qt-b", solo(0, 2));
    k2_daemon::compute_place::sweep(t0);
    assert_eq!(cancels(&a.frames()), vec![(job.clone(), 1, CANCEL_WITHDRAW.to_string())]);
    assert_eq!(row(&job).reason.as_deref(), Some("withdraw_requested"));
    // B fills; no answer from A. The queue time runs out: the ask is the timeout's now.
    compute_ws::test_frame("qt-b", Frame::Offer(solo(2, 2)));
    k2_daemon::compute_place::sweep(t0 + 11);
    let rj = row(&job);
    assert_eq!((rj.state.as_str(), rj.reason.as_deref()), ("assigned", Some("withdraw_timeout")), "{rj:?}");
    assert_eq!(rj.ctl.node_ask.as_ref().map(|a| (a.marker.as_str(), a.at)), Some(("withdraw_timeout", t0)), "the ask keeps its send time");
    assert_eq!(cancels(&a.frames()), vec![(job.clone(), 1, CANCEL_WITHDRAW.to_string())]);
    // Never answered: failed/queue_timeout at the deadline, A told to drop it.
    k2_daemon::compute_place::sweep(t0 + 181);
    let rj = row(&job);
    assert_eq!((rj.state.as_str(), rj.reason.as_deref(), rj.node_queued, rj.generation), ("failed", Some("queue_timeout"), false, 1), "{rj:?}");
    assert!(rj.detail.as_deref().is_some_and(|d| d.contains("didn't answer")), "{rj:?}");
    let c = cancels(&a.frames());
    assert_eq!(c.last(), Some(&(job.clone(), 1, SUPERSEDED.to_string())), "{c:?}");
    assert_eq!(events("job_queue_timeout", &job), 1);
    compute_ws::test_detach("qt-a");
    compute_ws::test_detach("qt-b");
    awake_now();
}

/// A person's no-force cancel is never settled for them (it could be a
/// job they still want if it started): after the deadline it says how to
/// end it. A re-send (a raised grant) the node never answers goes back to
/// the same node, the stop of the old generation first, then the new one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unanswered_cancel_says_force_and_a_resend_goes_back_in_order() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "cancelresend");
    active_node("cr-1", "cr-1", "linux", &ws);
    let mut n = compute_ws::test_attach("cr-1", offer(&["artifacts", "stall", FEATURE_QUEUE, FEATURE_SERVERS], 1, Some(view(Role::Tenant, 1, 0))));
    let mut jobs = Vec::new();
    for _ in 0..2 {
        let r = run(d.port, OWNER, json!({"node": "cr-1", "workspace": ws.id, "detach": true})).await;
        assert_eq!(r.status, 200, "{}", r.body);
        jobs.push(j(&r)["job"]["id"].as_str().unwrap().to_string());
    }
    assert_eq!(assigns(&n.frames()).len(), 2);
    for (i, id) in jobs.iter().enumerate() {
        compute_ws::test_frame("cr-1", state(id, 1, JobState::Queued, Some("node_queued"), Some(QueuePos { pos: i as u32 + 1, of: Some(3), eta_secs: None, eta_p90_secs: None, blocked_by: None })));
    }
    let (cancelled, resent) = (jobs[0].clone(), jobs[1].clone());
    let r = post(d.port, "/cli/compute/jobs/cancel", OWNER, json!({"job": cancelled, "ifState": "queued"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(d.port, "/cli/compute/grants/set", OWNER, json!({"node": "cr-1", "workspace": ws.id, "maxParallel": 3})).await;
    assert_eq!(j(&r)["resent"], 1, "{}", r.body);
    assert_eq!(
        cancels(&n.frames()),
        vec![(cancelled.clone(), 1, CANCEL_IF_QUEUED.to_string()), (resent.clone(), 1, CANCEL_WITHDRAW.to_string())]
    );
    let t0 = k2_core::compute::now();
    awake_long_before(t0);
    // The node keeps talking (an offer), but answers neither.
    compute_ws::test_frame("cr-1", Frame::Offer(fresh(offer(&["artifacts", "stall", FEATURE_QUEUE, FEATURE_SERVERS], 1, Some(view(Role::Tenant, 1, 2))))));
    let _ = n.frames();
    k2_daemon::compute_place::sweep(t0 + 61);
    let mut again = cancels(&n.frames());
    again.sort();
    let mut want = vec![(cancelled.clone(), 1, CANCEL_IF_QUEUED.to_string()), (resent.clone(), 1, CANCEL_WITHDRAW.to_string())];
    want.sort();
    assert_eq!(again, want, "each asked once more, as it was asked");
    k2_daemon::compute_place::sweep(t0 + 181);
    // The cancel: not settled, it says how to end it.
    let rc = row(&cancelled);
    assert_eq!((rc.state.as_str(), rc.reason.as_deref(), rc.generation), ("assigned", Some("cancel_if_queued"), 1), "{rc:?}");
    let short = &cancelled[..8];
    assert_eq!(rc.detail.as_deref(), Some(format!("cr-1 hasn't answered the cancel in 3 min; k2 compute cancel {short} --force stops it").as_str()));
    // The re-send: back to the same node, the stop of generation 1 first.
    let order: Vec<(String, String, u32)> = n
        .frames()
        .into_iter()
        .filter_map(|f| match f {
            Frame::Cancel(c) => Some(("cancel".to_string(), c.job_id, c.generation)),
            Frame::Assign(a) => Some(("assign".to_string(), a.fence.job_id, a.fence.generation)),
            _ => None,
        })
        .collect();
    assert_eq!(order, vec![("cancel".to_string(), resent.clone(), 1), ("assign".to_string(), resent.clone(), 2)], "never a cancel of the cancel-asked job; the re-send in order");
    let rr = row(&resent);
    assert_eq!((rr.node_id.as_str(), rr.generation, rr.ctl.retries_used), ("cr-1", 2, 0), "{rr:?}");
    assert_eq!(rr.ctl.attempts.last().map(|r| (r.next.as_str(), r.asked.as_deref())), Some(("withdraw_unanswered", Some("withdraw_resend"))));
    assert!(compute_ws::is_online("cr-1"), "the node kept talking");
    compute_ws::test_detach("cr-1");
    awake_now();
}

/// `k2 compute move --to` on a job in a node's own queue, and the node
/// never answers: at the deadline K2 takes it back itself and pins it to
/// the node asked, as a given-back pull would be; the first node is told
/// `superseded`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unanswered_pull_is_settled_to_the_node_asked() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "pullquiet");
    active_node("pq-a", "pq-a", "linux", &ws);
    active_node("pq-b", "pq-b", "linux", &ws);
    let mut a = compute_ws::test_attach("pq-a", offer(&["artifacts", "stall", FEATURE_QUEUE], 1, Some(view(Role::Tenant, 1, 0))));
    let mut b = compute_ws::test_attach("pq-b", solo(0, 2));
    let r = run(d.port, OWNER, json!({"node": "pq-a", "workspace": ws.id})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let job = j(&r)["job"]["id"].as_str().unwrap().to_string();
    let sent = assigns(&a.frames());
    assert_eq!(sent.iter().map(|x| x.0.clone()).collect::<Vec<_>>(), vec![job.clone()], "pinned: into A's queue at once");
    compute_ws::test_frame("pq-a", state(&job, sent[0].1, JobState::Queued, Some("node_queued"), None));
    let r = post(d.port, "/cli/compute/jobs/move", OWNER, json!({"job": job, "to": "pq-b"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(cancels(&a.frames()), vec![(job.clone(), 1, CANCEL_WITHDRAW.to_string())]);
    assert_eq!(row(&job).reason.as_deref(), Some("pull_requested"));
    let t0 = k2_core::compute::now();
    awake_long_before(t0);
    k2_daemon::compute_place::sweep(t0 + 61);
    assert_eq!(cancels(&a.frames()), vec![(job.clone(), 1, CANCEL_WITHDRAW.to_string())], "asked once more");
    assert!(assigns(&b.frames()).is_empty(), "nothing moves before the deadline");
    k2_daemon::compute_place::sweep(t0 + 181);
    let rj = row(&job);
    assert_eq!((rj.node_id.as_str(), rj.generation, rj.ctl.retries_used, rj.ctl.any.is_none()), ("pq-b", 2, 0, true), "pinned to B: {rj:?}");
    assert!(rj.ctl.pull.is_none(), "the pull is done: {rj:?}");
    assert_eq!(cancels(&a.frames()), vec![(job.clone(), 1, SUPERSEDED.to_string())]);
    assert_eq!(assigns(&b.frames()).iter().map(|x| (x.0.clone(), x.1)).collect::<Vec<_>>(), vec![(job.clone(), 2)]);
    assert_eq!((events("withdraw_unanswered", &job), events("pull", &job)), (1, 1));
    assert_eq!(rj.ctl.attempts.last().map(|r| (r.next.as_str(), r.asked.as_deref())), Some(("withdraw_unanswered", Some("pull_requested"))));
    compute_ws::test_detach("pq-a");
    compute_ws::test_detach("pq-b");
    awake_now();
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
            tier: None,
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
    // The route asks, and so may the compute ticker (an add is open, so it
    // mirrors the list too; on macOS it raced the route in r8): every
    // `servers.list` gets the same pinned answer until the route replies.
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop_seen = stop.clone();
    let answerer = tokio::task::spawn_blocking(move || {
        let mut answered = std::collections::HashSet::new();
        while !stop_seen.load(std::sync::atomic::Ordering::SeqCst) {
            let cmds: Vec<String> = n
                .frames()
                .into_iter()
                .filter_map(|f| match f {
                    Frame::Admin(a) => Some(a),
                    _ => None,
                })
                .map(|a| {
                    let body = k2_core::compute::proto::crypto::unb64(&a.cmd.body_b64).unwrap();
                    let c: k2_core::compute::proto::admin::Command = serde_json::from_slice(&body).unwrap();
                    assert_eq!(c.verb, "servers.list");
                    c.cmd_id
                })
                .collect();
            for cmd_id in cmds.into_iter().filter(|c| answered.insert(c.clone())) {
                // The pinned `servers.list` answer (share::ServersList).
                use k2_core::compute::proto::share::{ServerListing, ServersList};
                let entry = |slot: u8, label: &str, state: ServerStatus, trust: Trust, limits: ShareLimits| ServerListing {
                    slot,
                    label: label.into(),
                    state,
                    trust,
                    fp: None,
                    fp_prefix: format!("{slot:0>16}"),
                    limits,
                    expires_at: None,
                    expires_in_secs: None,
                    added_at: 1,
                    added_by: "local:rosson".into(),
                    running: 0,
                    queued: 0,
                    cpu_secs_today: 0,
                    sas: (state == ServerStatus::Pending).then(|| "482913".to_string()),
                    refusal: (state == ServerStatus::Refused).then(|| "server_too_old".to_string()),
                    online: slot == 0,
                    priority: None,
                };
                let result = serde_json::to_value(ServersList {
                    max_servers: 3,
                    solo: false,
                    servers: vec![
                        entry(0, "rosson.k2.dev", ServerStatus::Active, Trust::Own, ShareLimits::owner(2, 8, 16, 100, k2_core::compute::proto::share::Network::On)),
                        entry(1, "baden.k2.dev", ServerStatus::Pending, Trust::Team, ShareLimits::new_server(Trust::Team, false)),
                        entry(2, "old.k2.dev", ServerStatus::Refused, Trust::Own, ShareLimits::new_server(Trust::Own, false)),
                    ],
                })
                .unwrap();
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
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        n
    });
    let r = pending.await.unwrap();
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let n = answerer.await.unwrap();
    assert_eq!(r.status, 200, "{}", r.body);
    let s = j(&r);
    let list = s["servers"].as_array().unwrap();
    assert_eq!(list.len(), 3, "{s}");
    assert_eq!((list[2]["state"].as_str(), list[2]["refusal"].as_str()), (Some("refused"), Some("server_too_old")));
    assert_eq!((list[1]["state"].as_str(), list[1]["sas"].as_str()), (Some("pending"), Some("482 913")));
    assert_eq!((list[0]["weight"].as_u64(), list[1]["weight"].as_u64(), list[0]["sas"].is_null(), list[1]["refusal"].is_null()), (Some(1), Some(1), true, true), "--json carries weight, SAS and refusal");
    assert_eq!(s["requests"].as_array().unwrap().len(), 1, "the held add shows with the list");
    assert_eq!((s["maxServers"].as_u64(), s["solo"].as_bool()), (Some(3), Some(false)), "{s}");
    // Without asking the node again, the last list's facts still show (a
    // refresh by the ticker counts as much as the route's own).
    let r = get(d.port, "/cli/compute/nodes/servers", OWNER, "node=own-1").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let again = j(&r);
    assert_eq!((again["maxServers"].as_u64(), again["solo"].as_bool()), (Some(3), Some(false)), "{again}");
    {
        let db = k2_core::db::shared();
        assert_eq!(store::node_servers(&db.lock(), "own-1").len(), 3);
    }
    drop(n);
    for id in ["own-1", "own-old", "own-mac"] {
        compute_ws::test_detach(id);
    }
}

/// prd-macos-tenant-compute-v1 §6.3 (MT34–MT36): after an owner change.
/// Owner → tenant follows the node: the mirrored server list goes, open
/// admin requests fail `not_node_owner`. Tenant → owner never follows the
/// node's offer (share view or owner-only features): it's a claim, taken
/// over only by the owner in person (`take-owner`, unit-tested with a
/// confirm in `compute_admin::tests`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_role_follows_the_node_after_an_owner_change() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "flip");
    active_node("flip-1", "flip-1", "linux", &ws);
    let admin_feats = [FEATURE_QUEUE, FEATURE_SERVERS, "admin", "health", "self-update"];
    let _n = compute_ws::test_attach("flip-1", offer(&admin_feats, 0, Some(view(Role::Owner, 0, 0))));
    let role = || store::node_by_id(&k2_core::db::shared().lock(), "flip-1").unwrap().role;
    let flips = || -> i64 {
        k2_core::db::shared().lock().query_row("SELECT COUNT(*) FROM compute_events WHERE kind = 'role_change' AND node_id = 'flip-1'", [], |r| r.get(0)).unwrap()
    };
    assert_eq!(role(), "owner");
    let req_id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let row = k2_core::compute::share::ServerRow {
            slot: 1,
            label: "zoe.k2.dev".into(),
            fp: Some("ab".repeat(32)),
            state: "active".into(),
            trust: "own".into(),
            limits: ShareLimits::new_server(Trust::Own, false),
            usage: Some(json!({"online": true})),
            expires_at: None,
            added_at: Some(1),
            added_by: Some("local:test".into()),
        };
        store::replace_node_servers(&conn, "flip-1", &[row], k2_core::compute::now()).unwrap();
        let body = json!({"verb": "unit.restart", "args": {"unit": "k2-node"}});
        k2_core::compute::admin::insert_request(
            &conn,
            &k2_core::compute::admin::NewRequest {
                node_id: "flip-1",
                kind: "command",
                class: k2_core::compute::proto::admin::Class::Privileged,
                body: &body,
                summary: "restart k2-node on flip-1 (as root)",
                reason: None,
                proposed_by: "owner",
                proposer_workspace: None,
                proposer_session: None,
                now: k2_core::compute::now(),
            },
        )
        .unwrap()
        .id
    };

    // The node now belongs to another server: this one is a tenant.
    compute_ws::test_frame("flip-1", Frame::Offer(offer(&[FEATURE_QUEUE, FEATURE_SERVERS], 0, Some(view(Role::Tenant, 0, 0)))));
    assert_eq!(role(), "tenant");
    assert_eq!(flips(), 1);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        assert!(store::node_servers(&conn, "flip-1").is_empty(), "a tenant keeps no server list");
        let r = k2_core::compute::admin::request_by_id(&conn, &req_id).unwrap();
        assert_eq!(r.state, "failed");
        assert_eq!(r.result.as_ref().and_then(|v| v["code"].as_str()), Some("not_node_owner"), "{:?}", r.result);
    }
    // The same tenant view again: no second flip.
    compute_ws::test_frame("flip-1", Frame::Offer(offer(&[FEATURE_QUEUE, FEATURE_SERVERS], 0, Some(view(Role::Tenant, 0, 0)))));
    assert_eq!(flips(), 1);

    // The node now claims this server owns it (security review, cross-
    // branch): by owner-only features with no view (the old MT36 path),
    // then by a share view saying owner, solo or shared (MT34). Never
    // taken at its word: it stays a tenant, no flip, no secrets; the
    // person gets one note to take it over in person.
    let claims = || -> i64 {
        k2_core::db::shared().lock().query_row("SELECT COUNT(*) FROM compute_events WHERE kind = 'owner_claim' AND node_id = 'flip-1'", [], |r| r.get(0)).unwrap()
    };
    let mut solo = view(Role::Owner, 0, 0);
    solo.shared = false;
    solo.servers = 1;
    for o in [offer(&admin_feats, 0, None), offer(&admin_feats, 0, Some(view(Role::Owner, 0, 0))), offer(&admin_feats, 0, Some(solo))] {
        compute_ws::test_frame("flip-1", Frame::Offer(o));
        let n = store::node_by_id(&k2_core::db::shared().lock(), "flip-1").unwrap();
        assert_eq!(n.role, "tenant", "an offer never lifts a tenant");
        assert!(!n.owned_for_secrets());
        assert!(n.owner_claim(), "only a claim");
    }
    assert_eq!(flips(), 1, "no flip to owner");
    assert_eq!(claims(), 1, "one note for the claim");
    let r = get(d.port, "/cli/compute/nodes/info", OWNER, "node=flip-1").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(j(&r)["share"]["ownerClaim"], true, "{}", r.body);
    // Taking it over needs the owner in person: an agent never can, and
    // the owner needs a fresh confirm.
    let r = post(d.port, "/cli/compute/nodes/take-owner", &passport(&ws), json!({"node": "flip-1"})).await;
    assert_eq!(r.status, 403, "{}", r.body);
    let r = post(d.port, "/cli/compute/nodes/take-owner", OWNER, json!({"node": "flip-1"})).await;
    assert_eq!((r.status, code(&r).as_str()), (403, "vault_confirm_required"), "{}", r.body);
    assert_eq!(role(), "tenant");
    compute_ws::test_detach("flip-1");
}

/// Job tiers on a shared node (prd-compute-job-tiers-v1 S2: T7, T9, T11):
/// a node offering `tiers` gets `Plan.tier` with the class beside it; a
/// guest's Priority the node doesn't count runs as Normal with one note
/// (`tierAsked` kept); once the node says it counts, it stays Priority;
/// CI goes in as `gate` only where the share has gate.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tiers_on_a_shared_node() {
    use k2_core::compute::proto::tiers::{Tier, FEATURE_TIERS};
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "tiered");
    active_node("shr-t", "rig3-mac", "linux", &ws);
    let feats = ["artifacts", "stall", FEATURE_QUEUE, FEATURE_SERVERS, FEATURE_TIERS];
    let mut v = view(Role::Tenant, 0, 0);
    v.limits.max_queued = 30;
    v.priority = Some(false);
    let mut node = compute_ws::test_attach("shr-t", offer(&feats, 0, Some(v.clone())));

    // A guest share where Priority doesn't count: Normal, with a note.
    let r = run(d.port, OWNER, json!({"node": "rig3-mac", "workspace": ws.id, "tier": "priority", "why": "the release gate"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let out = j(&r);
    assert_eq!((out["job"]["tier"].as_str(), out["job"]["tierAsked"].as_str()), (Some("normal"), Some("priority")), "{out}");
    assert_eq!(out["notices"].as_array().map(Vec::len), Some(1), "{out}");
    assert_eq!(out["notices"][0]["code"], "priority_downgraded", "{out}");
    let a = assigns(&node.frames());
    let p = &a.last().expect("sent into the node's queue").2;
    assert_eq!((p.tier, p.class), (Some(Tier::Normal), Some(JobClass::Interactive)), "the node gets the tier it counts");

    // CI on a share without gate: refused, as --gate was.
    let r = run(d.port, OWNER, json!({"node": "rig3-mac", "workspace": ws.id, "tier": "ci"})).await;
    assert_eq!((r.status, code(&r).as_str()), (409, "gate_not_allowed"), "{}", r.body);

    // The node's owner lets this server's Priority count, and gives it gate.
    v.priority = Some(true);
    v.limits.gate = true;
    v.limits.top_class = JobClass::Gate;
    compute_ws::test_frame("shr-t", Frame::Offer(offer(&feats, 0, Some(v.clone()))));
    let r = run(d.port, OWNER, json!({"node": "rig3-mac", "workspace": ws.id, "tier": "priority", "why": "the release gate"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let out = j(&r);
    assert_eq!((out["job"]["tier"].as_str(), out["job"]["tierAsked"].as_str()), (Some("priority"), None), "{out}");
    assert!(out["notices"].as_array().is_some_and(|n| n.iter().all(|x| x["code"] != "priority_downgraded")), "{out}");
    let r = run(d.port, OWNER, json!({"node": "rig3-mac", "workspace": ws.id, "gate": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(j(&r)["job"]["tier"], "ci", "--gate is --tier ci");
    let a = assigns(&node.frames());
    let tiers: Vec<(Option<Tier>, Option<JobClass>)> = a.iter().map(|x| (x.2.tier, x.2.class)).collect();
    assert_eq!(tiers, vec![(Some(Tier::Priority), Some(JobClass::Interactive)), (Some(Tier::Ci), Some(JobClass::Gate))], "Priority is interactive for old peers; CI is gate");
    drop(env);
}

// ── the `--any` mover keeps looking (bounces, holds, order) ──────────

/// Offer revisions for the tests' fresh offers: a newer offer is the
/// node's word on every ask sent before it (CP-A7's pending asks end).
static REV: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(100);

/// `o` as the node's newest offer.
fn fresh(mut o: Offer) -> Offer {
    o.revision = REV.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    o
}

/// A node-queued `--any` job and its node's "you're queued" answer. Since
/// 0.46.17 an auto job goes to a shared node only when it starts there
/// now, so this is the race: the node looked free (an idle offer), the job
/// went, and the node had filled first (it queues it); then the node's
/// offer is busy again (this server's one running place there taken), as
/// every caller attached it.
async fn queued_any_on(port: u16, ws: &Ws, node: &str, a: &mut TestConn) -> String {
    let busy = offer(&["artifacts", "stall", FEATURE_QUEUE], 1, Some(view(Role::Tenant, 1, 0)));
    compute_ws::test_frame(node, Frame::Offer(fresh(offer(&["artifacts", "stall", FEATURE_QUEUE], 0, Some(view(Role::Tenant, 0, 0))))));
    let r = run(port, OWNER, json!({"any": true, "workspace": ws.id})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let job = j(&r)["job"]["id"].as_str().unwrap().to_string();
    assert_eq!(row(&job).node_id, node);
    let sent = assigns(&a.frames());
    assert_eq!(sent.len(), 1, "{node} looked free: sent at once");
    compute_ws::test_frame(node, state(&job, sent[0].1, JobState::Queued, Some("node_queued"), Some(QueuePos { pos: 2, of: None, eta_secs: Some(900), eta_p90_secs: None, blocked_by: None })));
    compute_ws::test_frame(node, Frame::Offer(fresh(busy)));
    assert!(assigns(&a.frames()).is_empty(), "nothing else sent");
    job
}

/// A solo node with `slots` slots, `running` busy (full when equal).
fn solo(running: u32, slots: u32) -> Offer {
    let mut o = v0462(offer(&["artifacts", "stall"], running, None));
    o.parallel_max = slots;
    if running >= slots {
        o = full(o);
    }
    o
}

/// The 34-minute wait, and 0.46.17's central hold: a job whose withdraw
/// bounced (the node it was meant for filled before the answer came) used
/// to go back into its old node's queue and wait out a back-off there. Now
/// it is held on this server (never sent back into the busy node's queue)
/// and the sweeper sends it to the first node that frees, with no back-off.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_bounced_job_is_held_here_and_goes_to_the_first_node_that_frees() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "bouncer");
    active_node("bn-a", "bn-a", "linux", &ws);
    active_node("bn-b", "bn-b", "linux", &ws);
    let mut a = compute_ws::test_attach("bn-a", offer(&["artifacts", "stall", FEATURE_QUEUE], 1, Some(view(Role::Tenant, 1, 0))));
    let job = queued_any_on(d.port, &ws, "bn-a", &mut a).await;
    let mut b = compute_ws::test_attach("bn-b", solo(0, 2));
    let t0 = k2_core::compute::now();
    k2_daemon::compute_place::sweep(t0);
    assert_eq!(cancels(&a.frames()), vec![(job.clone(), 1, CANCEL_WITHDRAW.to_string())], "withdraw for B");
    // B fills before A's answer comes back.
    compute_ws::test_frame("bn-b", Frame::Offer(solo(2, 2)));
    compute_ws::test_frame("bn-a", state(&job, 1, JobState::Cancelled, Some("withdrawn"), None));
    let rj = row(&job);
    assert_eq!((rj.node_id.as_str(), rj.state.as_str(), rj.generation, rj.reason.as_deref()), ("bn-a", "queued", 2, Some("any_hold")), "held here: {rj:?}");
    assert!(assigns(&a.frames()).is_empty(), "never sent back into A's queue");
    assert!(assigns(&b.frames()).is_empty());
    assert_eq!(events("withdraw_bounced", &job), 1);
    // It says it is held, and which nodes it fits.
    let r = get(d.port, "/cli/compute/jobs/get", OWNER, &format!("job={job}")).await;
    let g = j(&r);
    assert_eq!(g["wait"]["reason"], "held", "{g}");
    assert_eq!(g["queue"]["line"], "held: waiting for a node with a free slot (fits: bn-a, bn-b)", "{g}");
    assert_eq!(g["queue"]["fits"], json!(["bn-a", "bn-b"]), "{g}");
    // Nothing frees: it stays held through a sweep.
    k2_daemon::compute_place::sweep(t0 + 1);
    assert_eq!(row(&job).node_id, "bn-a");
    assert!(cancels(&a.frames()).is_empty() && assigns(&a.frames()).is_empty());
    // B frees: the next sweep sends it there, no back-off to wait out.
    compute_ws::test_frame("bn-b", Frame::Offer(solo(0, 2)));
    k2_daemon::compute_place::sweep(t0 + 2);
    let rj = row(&job);
    assert_eq!((rj.node_id.as_str(), rj.withdraws), ("bn-b", 0), "moved to B: {rj:?}");
    assert_eq!(assigns(&b.frames()).iter().map(|x| x.0.clone()).collect::<Vec<_>>(), vec![job.clone()]);
    let p = rj.ctl.placement.expect("placement");
    assert!(p.why.starts_with("moved from bn-a: bn-b can start it now"), "{}", p.why);
    // The event stream says it in words.
    let r = get(d.port, "/cli/compute/jobs", OWNER, "events=1&last=50").await;
    let ev = j(&r);
    let moved: Vec<&Value> = ev["events"].as_array().unwrap().iter().filter(|e| e["event"] == "moved" && e["job"] == job.as_str()).collect();
    assert_eq!(moved.len(), 1, "{ev}");
    assert_eq!(moved[0]["detail"], "moved from bn-a to bn-b: bn-b can start it now", "{ev}");
    compute_ws::test_detach("bn-a");
    compute_ws::test_detach("bn-b");
}

/// While a node is asked to give a job back for B, B's place is held: a
/// new `--any` job doesn't take it (or queue behind it there), and the
/// moving job lands on B when the answer comes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_target_is_held_while_the_withdraw_travels() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "holder");
    active_node("hd-a", "hd-a", "linux", &ws);
    active_node("hd-b", "hd-b", "linux", &ws);
    let mut a = compute_ws::test_attach("hd-a", offer(&["artifacts", "stall", FEATURE_QUEUE], 1, Some(view(Role::Tenant, 1, 0))));
    let first = queued_any_on(d.port, &ws, "hd-a", &mut a).await;
    let mut b = compute_ws::test_attach("hd-b", solo(0, 1));
    k2_daemon::compute_place::sweep(k2_core::compute::now());
    assert_eq!(cancels(&a.frames()).len(), 1, "withdraw asked for B");
    // A new job meanwhile: not onto the held B.
    let r = run(d.port, OWNER, json!({"any": true, "workspace": ws.id})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let second = j(&r)["job"]["id"].as_str().unwrap().to_string();
    assert_eq!(row(&second).node_id, "hd-a", "B is held for the moving job");
    assert!(assigns(&b.frames()).is_empty());
    compute_ws::test_frame("hd-a", state(&first, 1, JobState::Cancelled, Some("withdrawn"), None));
    assert_eq!(row(&first).node_id, "hd-b", "the moving job gets B");
    assert_eq!(assigns(&b.frames()).iter().map(|x| x.0.clone()).collect::<Vec<_>>(), vec![first.clone()]);
    compute_ws::test_detach("hd-a");
    compute_ws::test_detach("hd-b");
}

/// Order and pins: while the older job of a workspace backs off after a
/// bounce, its newer job on the same node doesn't move past it; a job
/// named on a node never moves.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_newer_job_never_moves_past_an_older_one_and_a_pinned_job_stays() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "orderly");
    active_node("or-a", "or-a", "linux", &ws);
    active_node("or-b", "or-b", "linux", &ws);
    {
        // Room for three of the workspace's jobs on A (0.46.17: an auto job
        // never goes in behind its own workspace's at the grant's cap).
        let db = k2_core::db::shared();
        let three = store::GrantLimits { max_parallel: Some(3), ..Default::default() };
        store::set_grant(&db.lock(), "or-a", &ws.id, &three, "owner", 4).expect("grant a");
    }
    let mut a = compute_ws::test_attach("or-a", offer(&["artifacts", "stall", FEATURE_QUEUE], 1, Some(view(Role::Tenant, 1, 0))));
    let older = queued_any_on(d.port, &ws, "or-a", &mut a).await;
    let newer = queued_any_on(d.port, &ws, "or-a", &mut a).await;
    let r = run(d.port, OWNER, json!({"node": "or-a", "workspace": ws.id})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let pinned = j(&r)["job"]["id"].as_str().unwrap().to_string();
    let sent = assigns(&a.frames());
    compute_ws::test_frame("or-a", state(&pinned, sent[0].1, JobState::Queued, Some("node_queued"), None));
    let now = k2_core::compute::now();
    // The older job bounced 10 s ago.
    {
        let db = k2_core::db::shared();
        store::note_withdraw(&db.lock(), &older, now - 10);
    }
    let _b = compute_ws::test_attach("or-b", solo(0, 1));
    k2_daemon::compute_place::sweep(now);
    assert!(cancels(&a.frames()).is_empty(), "the newer job waits for the older one's turn; the pinned one never moves");
    k2_daemon::compute_place::sweep(now + 60);
    assert_eq!(cancels(&a.frames()), vec![(older.clone(), 1, CANCEL_WITHDRAW.to_string())], "the older job first; B (one slot) is held for it");
    assert_eq!((row(&newer).node_id.as_str(), row(&pinned).node_id.as_str()), ("or-a", "or-a"));
    let r = get(d.port, "/cli/compute/jobs/get", OWNER, &format!("job={pinned}")).await;
    assert_eq!(j(&r)["move"]["auto"], false, "{}", r.body);
    compute_ws::test_detach("or-a");
    compute_ws::test_detach("or-b");
}

/// The shared-node mover never asks for a withdraw to bring a job to a
/// pinned-only node, however free it is; turned off, it does.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_mover_never_withdraws_a_job_for_a_pinned_only_node() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "pinonly");
    active_node("po-a", "po-a", "linux", &ws);
    active_node("po-b", "po-b", "linux", &ws);
    let mut a = compute_ws::test_attach("po-a", offer(&["artifacts", "stall", FEATURE_QUEUE], 1, Some(view(Role::Tenant, 1, 0))));
    let job = queued_any_on(d.port, &ws, "po-a", &mut a).await;
    let set = |on: bool| {
        let db = k2_core::db::shared();
        k2_core::compute::pinned_only::set(&db.lock(), "po-b", on, "owner", k2_core::compute::now()).expect("pinned-only");
    };
    set(true);
    let _b = compute_ws::test_attach("po-b", solo(0, 2));
    k2_daemon::compute_place::sweep(k2_core::compute::now());
    assert!(cancels(&a.frames()).is_empty(), "no withdraw for a pinned-only node");
    assert_eq!(row(&job).withdraws, 0);
    set(false);
    k2_daemon::compute_place::sweep(k2_core::compute::now());
    assert_eq!(cancels(&a.frames()), vec![(job.clone(), 1, CANCEL_WITHDRAW.to_string())], "off again: the mover asks");
    compute_ws::test_detach("po-a");
    compute_ws::test_detach("po-b");
}

// ── why a job waits while another node looks free ────────────────────

/// A job in node A's queue at its workspace's grant cap there
/// (`workspace_running_full`) moves to node B, granted, shared and free:
/// the mover asks A for it back and sends it to B.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_job_at_its_grant_cap_moves_to_a_free_granted_node() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "capped");
    active_node("gc-a", "gc-a", "linux", &ws);
    active_node("gc-b", "gc-b", "linux", &ws);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let one = store::GrantLimits { max_parallel: Some(1), ..Default::default() };
        store::set_grant(&conn, "gc-a", &ws.id, &one, "owner", 4).expect("grant a");
        store::set_grant(&conn, "gc-b", &ws.id, &one, "owner", 4).expect("grant b");
    }
    let mut a = compute_ws::test_attach("gc-a", offer(&["artifacts", "stall", FEATURE_QUEUE], 1, Some(view(Role::Tenant, 1, 0))));
    let job = queued_any_on(d.port, &ws, "gc-a", &mut a).await;
    let held = QueuePos { pos: 1, of: Some(5), eta_secs: None, eta_p90_secs: None, blocked_by: Some("workspace_running_full".into()) };
    compute_ws::test_frame("gc-a", state(&job, 1, JobState::Queued, Some("node_queued"), Some(held)));
    // B: shared with other servers, one of its two slots busy, no quote yet.
    let mut b = compute_ws::test_attach("gc-b", offer(&["artifacts", "stall", FEATURE_QUEUE], 1, Some(view(Role::Tenant, 0, 0))));
    k2_daemon::compute_place::sweep(k2_core::compute::now());
    assert_eq!(cancels(&a.frames()), vec![(job.clone(), 1, CANCEL_WITHDRAW.to_string())], "A is asked to give it back for B");
    compute_ws::test_frame("gc-a", state(&job, 1, JobState::Cancelled, Some("withdrawn"), None));
    let rj = row(&job);
    assert_eq!(rj.node_id, "gc-b", "{rj:?}");
    assert_eq!(assigns(&b.frames()).len(), 1, "sent to B");
    let p = rj.ctl.placement.expect("placement");
    assert!(p.why.starts_with("moved from gc-a: gc-b can start it now"), "{}", p.why);
    compute_ws::test_detach("gc-a");
    compute_ws::test_detach("gc-b");
}

/// A shared node whose slots all run other servers' jobs can't start the
/// job, though this server runs nothing there and its CPUs and memory look
/// free (no quote for this size yet): the mover leaves the job where it
/// is, and `get` says which node and why. When a slot frees, it moves.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_mover_skips_a_node_full_of_other_servers_jobs_and_says_why() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "fullnode");
    active_node("fl-a", "fl-a", "linux", &ws);
    active_node("fl-b", "fl-b", "linux", &ws);
    let mut a = compute_ws::test_attach("fl-a", offer(&["artifacts", "stall", FEATURE_QUEUE], 1, Some(view(Role::Tenant, 1, 0))));
    let job = queued_any_on(d.port, &ws, "fl-a", &mut a).await;
    let held = QueuePos { pos: 1, of: Some(5), eta_secs: None, eta_p90_secs: None, blocked_by: Some("workspace_running_full".into()) };
    compute_ws::test_frame("fl-a", state(&job, 1, JobState::Queued, Some("node_queued"), Some(held)));
    // B runs 2 of its 2 slots, both another server's; this server's share
    // there runs nothing.
    let mut b = compute_ws::test_attach("fl-b", offer(&["artifacts", "stall", FEATURE_QUEUE], 2, Some(view(Role::Tenant, 0, 0))));
    let now = k2_core::compute::now();
    k2_daemon::compute_place::sweep(now);
    assert!(cancels(&a.frames()).is_empty(), "no withdraw for a node with no free slot");
    assert_eq!(row(&job).withdraws, 0);
    // The job says why B doesn't take it.
    let r = get(d.port, "/cli/compute/jobs/get", OWNER, &format!("job={job}")).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let mv = &j(&r)["move"];
    let not_now = mv["notNow"].as_array().unwrap_or_else(|| panic!("no notNow: {mv}"));
    assert_eq!(not_now.len(), 1, "{mv}");
    assert_eq!((not_now[0]["node"].as_str(), not_now[0]["code"].as_str()), (Some("fl-b"), Some("node_full")), "{mv}");
    let note = mv["note"].as_str().expect("note");
    // One line, in fl-b's own numbers; other servers' jobs only counted.
    let line = "no other node can start it now (it asks 1G): fl-b (full: 2 of 2 jobs run; other servers' jobs)";
    assert!(note.contains(line), "{note}");
    assert_eq!(mv["elsewhere"].as_str(), Some(line), "{mv}");
    assert_eq!(j(&r)["wait"]["elsewhere"].as_str(), Some(line), "{}", r.body);
    // A slot frees on B: the mover asks for it back now.
    compute_ws::test_frame("fl-b", Frame::Offer(offer(&["artifacts", "stall", FEATURE_QUEUE], 1, Some(view(Role::Tenant, 0, 0)))));
    k2_daemon::compute_place::sweep(now + 1);
    assert_eq!(cancels(&a.frames()), vec![(job.clone(), 1, CANCEL_WITHDRAW.to_string())], "B has a slot now");
    compute_ws::test_frame("fl-a", state(&job, 1, JobState::Cancelled, Some("withdrawn"), None));
    assert_eq!(row(&job).node_id, "fl-b");
    assert_eq!(assigns(&b.frames()).len(), 1);
    compute_ws::test_detach("fl-a");
    compute_ws::test_detach("fl-b");
}

/// The central hold: an auto job waits on this server for a node that can
/// start it now. A shared node with every slot taken by other servers'
/// jobs can't, though this server's share there is empty and no quote
/// says otherwise: the job waits here (`any_hold`, so the mover may still
/// take it elsewhere) and goes in once a slot frees.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_auto_job_holds_on_this_server_while_the_shared_node_is_full() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "anyhold");
    active_node("ah-a", "ah-a", "linux", &ws);
    let mut a = compute_ws::test_attach("ah-a", offer(&["artifacts", "stall", FEATURE_QUEUE], 2, Some(view(Role::Tenant, 0, 0))));
    let r = run(d.port, OWNER, json!({"any": true, "workspace": ws.id})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let job = j(&r)["job"]["id"].as_str().unwrap().to_string();
    assert!(assigns(&a.frames()).is_empty(), "2 of 2 slots run other servers' jobs: not into its queue yet");
    let rj = row(&job);
    assert_eq!((rj.node_id.as_str(), rj.state.as_str(), rj.reason.as_deref()), ("ah-a", "queued", Some("any_hold")), "{rj:?}");
    compute_ws::test_frame("ah-a", Frame::Offer(offer(&["artifacts", "stall", FEATURE_QUEUE], 1, Some(view(Role::Tenant, 0, 0)))));
    assert_eq!(assigns(&a.frames()).iter().map(|x| x.0.clone()).collect::<Vec<_>>(), vec![job.clone()], "a slot is free: sent");
    compute_ws::test_detach("ah-a");
}

/// 0.46.17, the central hold: no auto job goes into a busy shared node's
/// queue to wait behind its limit, whatever its tier (Priority and
/// Standard alike); a job named on the node goes in at once, as before.
/// Once one slot frees, the Priority job goes and the Standard one stays
/// held here, saying so.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_auto_job_holds_on_this_server_until_a_node_can_start_it() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "prihold");
    active_node("ph-a", "ph-a", "linux", &ws);
    {
        let db = k2_core::db::shared();
        let three = store::GrantLimits { max_parallel: Some(3), ..Default::default() };
        store::set_grant(&db.lock(), "ph-a", &ws.id, &three, "owner", 4).expect("grant");
    }
    let mut a = compute_ws::test_attach("ph-a", offer(&["artifacts", "stall", FEATURE_QUEUE], 2, Some(view(Role::Tenant, 0, 0))));
    let r = run(d.port, OWNER, json!({"any": true, "workspace": ws.id, "tier": "priority", "why": "the release gate"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let pri = j(&r)["job"]["id"].as_str().unwrap().to_string();
    let r = run(d.port, OWNER, json!({"any": true, "workspace": ws.id})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let std_job = j(&r)["job"]["id"].as_str().unwrap().to_string();
    assert!(assigns(&a.frames()).is_empty(), "2 of 2 slots taken: neither goes into the node's queue");
    for id in [&pri, &std_job] {
        let rj = row(id);
        assert_eq!((rj.state.as_str(), rj.reason.as_deref(), rj.node_queued), ("queued", Some("any_hold"), false), "{rj:?}");
    }
    // A job named on the node goes into its queue at once (unchanged).
    let r = run(d.port, OWNER, json!({"node": "ph-a", "workspace": ws.id})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let pinned = j(&r)["job"]["id"].as_str().unwrap().to_string();
    let sent = assigns(&a.frames());
    assert_eq!(sent.iter().map(|x| x.0.clone()).collect::<Vec<_>>(), vec![pinned.clone()], "pinned: into the queue");
    compute_ws::test_frame("ph-a", state(&pinned, sent[0].1, JobState::Queued, Some("node_queued"), None));
    // While this server's own job still waits in the node's queue, the
    // node starts nothing new of ours: both stay here.
    compute_ws::test_frame("ph-a", Frame::Offer(fresh(offer(&["artifacts", "stall", FEATURE_QUEUE], 1, Some(view(Role::Tenant, 0, 1))))));
    assert!(assigns(&a.frames()).is_empty(), "behind this server's own queued job: still held");
    // One slot free and nothing of this server waits there: Priority goes,
    // Standard stays held (one slot, one job).
    compute_ws::test_frame("ph-a", Frame::Offer(fresh(offer(&["artifacts", "stall", FEATURE_QUEUE], 1, Some(view(Role::Tenant, 0, 0))))));
    assert_eq!(assigns(&a.frames()).iter().map(|x| x.0.clone()).collect::<Vec<_>>(), vec![pri.clone()], "a slot is free: Priority sent");
    assert_eq!((row(&std_job).state.as_str(), row(&std_job).reason.as_deref()), ("queued", Some("any_hold")));
    let r = get(d.port, "/cli/compute/jobs", OWNER, "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let list = j(&r);
    let held = list["jobs"].as_array().unwrap().iter().find(|x| x["id"] == std_job.as_str()).unwrap_or_else(|| panic!("{list}"));
    assert_eq!(held["wait"]["reason"], "held", "{held}");
    assert_eq!(held["wait"]["words"], "held: waiting for a node with a free slot (fits: ph-a)", "{held}");
    assert_eq!(held["queue"]["where"], "server", "{held}");
    compute_ws::test_detach("ph-a");
}

/// 11-19: a job was placed "starts now" on a shared node its owner was
/// draining for a k2-node update, and sat in its queue over 3 hours. A
/// draining node takes no auto job (and never claims "starts now"),
/// whatever its quote says; an auto job already in its queue leaves for a
/// node where it can wait its turn; a job named on it stays here, saying
/// the node is draining, and goes in once the node is active again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_draining_node_takes_no_auto_job_and_its_queued_auto_jobs_leave() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "drainer");
    active_node("dr-a", "dr-a", "linux", &ws);
    active_node("dr-b", "dr-b", "linux", &ws);
    // A: shared, our share's running slot taken: an auto job waits in its queue.
    let busy = || offer(&["artifacts", "stall", FEATURE_QUEUE], 1, Some(view(Role::Tenant, 1, 0)));
    let mut a = compute_ws::test_attach("dr-a", busy());
    let queued = queued_any_on(d.port, &ws, "dr-a", &mut a).await;
    // B: shared and busy the same way (a place to wait, not to start now).
    let mut b = compute_ws::test_attach("dr-b", busy());
    k2_daemon::compute_place::sweep(k2_core::compute::now());
    assert!(cancels(&a.frames()).is_empty(), "both only busy: the job keeps its place on A");

    // A's owner drains it. Its share is empty and its quote says a 1G job
    // starts now: the quote doesn't know about the drain.
    let mut drained = offer(&["artifacts", "stall", FEATURE_QUEUE], 0, Some(view(Role::Tenant, 0, 0)));
    drained.revision = 2;
    drained.control = Control::Draining;
    drained.control_by = Some("k2-web-test".into());
    drained.share.as_mut().unwrap().quotes =
        vec![k2_core::compute::proto::share::Quote { cpu_millis: 0, mem_bytes: 1 << 30, starts_now: true, eta_secs: None, eta_p90_secs: None, blocked_by: None }];
    compute_ws::test_frame("dr-a", Frame::Offer(drained.clone()));

    // A new auto job: never A, never "starts now".
    let r = run(d.port, OWNER, json!({"any": true, "workspace": ws.id})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let fresh = j(&r);
    assert_eq!(fresh["job"]["nodeId"], "dr-b", "{fresh}");
    assert_eq!(fresh["placement"]["startsNow"], false, "{fresh}");
    let code = fresh["placement"]["code"].as_str().unwrap();
    assert!(!code.starts_with("starts_now"), "{fresh}");
    assert!(!fresh["placement"]["why"].as_str().unwrap().contains("starts now"), "{fresh}");

    // A job named on A: placed there, held here, saying why.
    let r = run(d.port, OWNER, json!({"node": "dr-a", "workspace": ws.id})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let pinned = j(&r)["job"]["id"].as_str().unwrap().to_string();
    let rp = row(&pinned);
    assert_eq!((rp.node_id.as_str(), rp.state.as_str(), rp.reason.as_deref()), ("dr-a", "queued", Some("local_draining")), "{rp:?}");
    assert_eq!(rp.detail.as_deref(), Some("the machine's owner is draining this node (by k2-web-test)"), "{rp:?}");
    assert!(assigns(&a.frames()).is_empty(), "nothing goes into a draining node's queue");

    // The sweep asks A to give the queued auto job back, though B can
    // only queue it: A starts nothing until its drain ends.
    k2_daemon::compute_place::sweep(k2_core::compute::now());
    assert_eq!(cancels(&a.frames()), vec![(queued.clone(), 1, CANCEL_WITHDRAW.to_string())], "withdrawn from the draining node");
    compute_ws::test_frame("dr-a", state(&queued, 1, JobState::Cancelled, Some("withdrawn"), None));
    let rq = row(&queued);
    assert_eq!((rq.node_id.as_str(), rq.generation), ("dr-b", 2), "moved to B: {rq:?}");
    let p = rq.ctl.placement.clone().expect("placement");
    assert_eq!(p.code, "moved_node_draining", "{p:?}");
    assert!(p.why.starts_with("moved from dr-a: "), "{}", p.why);
    assert!(assigns(&a.frames()).is_empty(), "never sent back to A");
    // B is busy: held here for it (0.46.17), not in B's queue.
    assert!(!assigns(&b.frames()).iter().any(|x| x.0 == queued), "not into busy B's queue");
    assert_eq!((rq.state.as_str(), rq.reason.as_deref()), ("queued", Some("any_hold")), "held here: {rq:?}");

    // The pinned job stays, still saying why, through another sweep.
    k2_daemon::compute_place::sweep(k2_core::compute::now());
    let rp = row(&pinned);
    assert_eq!((rp.node_id.as_str(), rp.reason.as_deref()), ("dr-a", Some("local_draining")), "{rp:?}");
    assert!(cancels(&a.frames()).is_empty());
    let g = get(d.port, "/cli/compute/jobs/get", OWNER, &format!("job={pinned}")).await;
    assert!(g.body.contains("draining this node"), "{}", g.body);

    // The drain ends: the pinned job goes into A's queue.
    let mut active = drained;
    active.revision = 3;
    active.control = Control::Active;
    active.control_by = None;
    compute_ws::test_frame("dr-a", Frame::Offer(active));
    assert_eq!(assigns(&a.frames()).iter().map(|x| x.0.clone()).collect::<Vec<_>>(), vec![pinned.clone()], "active again: sent");
    compute_ws::test_detach("dr-a");
    compute_ws::test_detach("dr-b");
}

// ── the central hold and pulling back (0.46.17) ──────────────────────

/// 11-20: auto jobs sat in one busy node's own queue while other nodes
/// went idle. Now an auto job no node can start is held on this server
/// (in neither busy node's queue), and when one of them frees it goes
/// there, never to the one still busy.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_held_job_goes_to_the_node_that_frees_never_into_the_busy_ones_queue() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "central");
    active_node("ch-a", "node-a", "linux", &ws);
    active_node("ch-b", "node-b", "linux", &ws);
    // Both shared and full: every slot runs another server's job.
    let full_node = || fresh(offer(&["artifacts", "stall", FEATURE_QUEUE], 2, Some(view(Role::Tenant, 0, 0))));
    let mut a = compute_ws::test_attach("ch-a", full_node());
    let mut b = compute_ws::test_attach("ch-b", full_node());
    let r = run(d.port, OWNER, json!({"any": true, "workspace": ws.id})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let job = j(&r)["job"]["id"].as_str().unwrap().to_string();
    let first = row(&job).node_id;
    assert!(assigns(&a.frames()).is_empty() && assigns(&b.frames()).is_empty(), "into neither node's queue");
    let rj = row(&job);
    assert_eq!((rj.state.as_str(), rj.reason.as_deref(), rj.node_queued), ("queued", Some("any_hold"), false), "held here: {rj:?}");
    let g = j(&get(d.port, "/cli/compute/jobs/get", OWNER, &format!("job={job}")).await);
    assert_eq!(g["wait"]["reason"], "held", "{g}");
    assert_eq!(g["queue"]["line"], "held: waiting for a node with a free slot (fits: node-a, node-b)", "{g}");
    // Sweeps while both stay full: it stays held, nothing is sent.
    k2_daemon::compute_place::sweep(k2_core::compute::now());
    assert!(assigns(&a.frames()).is_empty() && assigns(&b.frames()).is_empty());
    // The node it was NOT first placed on frees a slot: it goes there.
    let (idle_id, mut idle, mut busy) = if first == "ch-a" { ("ch-b", b, a) } else { ("ch-a", a, b) };
    compute_ws::test_frame(idle_id, Frame::Offer(fresh(offer(&["artifacts", "stall", FEATURE_QUEUE], 1, Some(view(Role::Tenant, 0, 0))))));
    k2_daemon::compute_place::sweep(k2_core::compute::now());
    let rj = row(&job);
    assert_eq!((rj.node_id.as_str(), rj.state.as_str()), (idle_id, "assigned"), "sent to the node that freed: {rj:?}");
    assert_eq!(assigns(&idle.frames()).iter().map(|x| x.0.clone()).collect::<Vec<_>>(), vec![job.clone()]);
    assert!(assigns(&busy.frames()).is_empty(), "never into the busy node's queue");
    compute_ws::test_detach("ch-a");
    compute_ws::test_detach("ch-b");
}

/// `k2 compute move` on a job in a shared node's own queue pulls it back
/// (it used to be refused `in_node_queue`): `--auto` asks the node to give
/// it back (the protocol-3 withdraw), and once it is back it is held here,
/// not sent into the still-busy node's queue again; `--to` pins a second
/// one to the named node once it is back. A second ask while the answer
/// travels is `in_node_queue`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn move_pulls_a_job_back_from_a_shared_nodes_queue() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "puller");
    active_node("pb-a", "node-a", "linux", &ws);
    active_node("pb-b", "node-b", "linux", &ws);
    {
        let db = k2_core::db::shared();
        let three = store::GrantLimits { max_parallel: Some(3), ..Default::default() };
        store::set_grant(&db.lock(), "pb-a", &ws.id, &three, "owner", 4).expect("grant a");
    }
    let mut a = compute_ws::test_attach("pb-a", offer(&["artifacts", "stall", FEATURE_QUEUE], 1, Some(view(Role::Tenant, 1, 0))));
    let job = queued_any_on(d.port, &ws, "pb-a", &mut a).await;
    assert!(row(&job).node_queued);
    // B: shared and full, so nothing can start it now.
    let mut b = compute_ws::test_attach("pb-b", fresh(offer(&["artifacts", "stall", FEATURE_QUEUE], 2, Some(view(Role::Tenant, 0, 0)))));

    // move --auto on an auto job in A's queue: pulled back, not already_auto.
    let r = post(d.port, "/cli/compute/jobs/move", OWNER, json!({"job": job, "auto": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let mv = j(&r);
    assert_eq!((mv["pulling"].as_bool(), mv["moved"].as_bool(), mv["mode"].as_str()), (Some(true), Some(false), Some("auto")), "{mv}");
    assert!(mv["note"].as_str().unwrap().starts_with("asked node-a to give it back from its queue"), "{mv}");
    assert_eq!(cancels(&a.frames()), vec![(job.clone(), 1, CANCEL_WITHDRAW.to_string())], "the node is asked to give it back");
    let rj = row(&job);
    assert_eq!((rj.state.as_str(), rj.reason.as_deref()), ("assigned", Some("pull_requested")), "{rj:?}");
    // While the answer travels: get says so, and a second ask waits.
    let g = j(&get(d.port, "/cli/compute/jobs/get", OWNER, &format!("job={job}")).await);
    assert_eq!(g["move"]["moving"], true, "{g}");
    assert!(g["move"]["note"].as_str().unwrap().starts_with("pulling back: node-a was asked to give it back"), "{g}");
    let r = post(d.port, "/cli/compute/jobs/move", OWNER, json!({"job": job, "to": "node-b"})).await;
    assert_eq!((r.status, code(&r)), (409, "in_node_queue".to_string()), "{}", r.body);
    // The sweeper doesn't ask again meanwhile.
    k2_daemon::compute_place::sweep(k2_core::compute::now());
    assert!(cancels(&a.frames()).is_empty());
    // The node gives it back: a new generation, no retry used, held here.
    compute_ws::test_frame("pb-a", state(&job, 1, JobState::Cancelled, Some("withdrawn"), None));
    let rj = row(&job);
    assert_eq!((rj.state.as_str(), rj.generation, rj.ctl.retries_used, rj.reason.as_deref()), ("queued", 2, 0, Some("any_hold")), "{rj:?}");
    assert!(rj.ctl.pull.is_none(), "the ask is done: {rj:?}");
    assert!(assigns(&a.frames()).is_empty(), "not sent back into busy A's queue");
    assert!(assigns(&b.frames()).is_empty(), "nor into busy B's");
    assert_eq!(events("pull", &job), 1);

    // A job pinned to A (it went into A's queue at once), then move --to B.
    let r = run(d.port, OWNER, json!({"node": "node-a", "workspace": ws.id})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let pinned = j(&r)["job"]["id"].as_str().unwrap().to_string();
    let sent = assigns(&a.frames());
    assert_eq!(sent.iter().map(|x| x.0.clone()).collect::<Vec<_>>(), vec![pinned.clone()], "pinned: into A's queue at once, as before");
    compute_ws::test_frame("pb-a", state(&pinned, sent[0].1, JobState::Queued, Some("node_queued"), None));
    let r = post(d.port, "/cli/compute/jobs/move", OWNER, json!({"job": pinned, "to": "node-b"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let mv = j(&r);
    assert_eq!((mv["pulling"].as_bool(), mv["mode"].as_str(), mv["node"].as_str()), (Some(true), Some("pinned"), Some("node-b")), "{mv}");
    assert_eq!(cancels(&a.frames()), vec![(pinned.clone(), sent[0].1, CANCEL_WITHDRAW.to_string())]);
    compute_ws::test_frame("pb-a", state(&pinned, sent[0].1, JobState::Cancelled, Some("withdrawn"), None));
    let rp = row(&pinned);
    assert_eq!((rp.node_id.as_str(), rp.ctl.any.is_none()), ("pb-b", true), "pinned to B: {rp:?}");
    assert_eq!(rp.ctl.placement.as_ref().map(|p| p.code.as_str()), Some("moved_to"), "{rp:?}");
    // Pinned to B: it goes into B's queue at once, busy or not (pinned jobs keep today's rule).
    assert_eq!(assigns(&b.frames()).iter().map(|x| x.0.clone()).collect::<Vec<_>>(), vec![pinned.clone()]);

    // A job that started before the node got the ask stays where it runs.
    let r = run(d.port, OWNER, json!({"node": "node-a", "workspace": ws.id})).await;
    let raced = j(&r)["job"]["id"].as_str().unwrap().to_string();
    let sent = assigns(&a.frames());
    compute_ws::test_frame("pb-a", state(&raced, sent[0].1, JobState::Queued, Some("node_queued"), None));
    let r = post(d.port, "/cli/compute/jobs/move", OWNER, json!({"job": raced, "auto": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    compute_ws::test_frame("pb-a", state(&raced, sent[0].1, JobState::Running, Some("job_started"), None));
    let rr = row(&raced);
    assert_eq!((rr.node_id.as_str(), rr.state.as_str(), rr.generation), ("pb-a", "running", i64::from(sent[0].1)), "{rr:?}");
    compute_ws::test_detach("pb-a");
    compute_ws::test_detach("pb-b");
}
