//! Compute auto-placement P1 (prd-compute-auto-placement-v1 §10): the real
//! dispatcher in-process (temp `$HOME`, loopback, no client attached) and
//! fake node sessions (`compute_ws::test_attach` / `test_frame`) whose
//! offers say exactly what each node has free.
//!
//! Covers: no node named = auto (the route and `--any` the same), a named
//! node pinned; never crossing grants, for the owner too (D1); the agent
//! passport never sees another workspace or an ungranted node in the
//! record (S5); `--timeout` over a grant skips that node (F10); a node too
//! small for `--mem` is skipped and counted; `no_node_fits` with the
//! counts; a burst spreads; warm vs idle per D2 with the commit resolved at
//! submit (CP-A9); `--plan` and `get` carry the record; `--shards` without
//! `--any`; the `computePlacement` v1 switch; a shared node whose other
//! server holds its memory is not "now" (CP-A6, two servers); and a move
//! off a node that went offline rewrites the record.

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_core::compute::proto::frames::{Availability, Caps, Control, Frame, Free, Offer, SlotInfo};
use k2_core::compute::proto::share::{Role, ServerStatus, ShareLimits, ShareView, Trust, FEATURE_QUEUE};
use k2_core::compute::store;
use k2_core::session::SessionId;
use k2_daemon::compute_ws::{self, TestConn};
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;
use rusqlite::params;
use serde_json::{json, Value};

static TEST_LOCK: StdMutex<()> = StdMutex::new(());
const OWNER: &str = "owner-token-compute-place-v1";
const GB: u64 = 1 << 30;

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
        k2_core::compute::set_placement_policy(k2_core::compute::PlacementPolicy::V2);
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn setup() -> Env {
    let guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let home = PathBuf::from(format!("/tmp/k2cpl-{:x}", nanos % 0xffff_ffff));
    std::fs::create_dir_all(&home).unwrap();
    let home = dunce::canonicalize(&home).unwrap();
    let env = Env { prev_home: std::env::var_os("HOME"), home: home.clone(), _guard: guard };
    std::env::set_var("HOME", &home);
    std::env::set_var("K2_COMPUTE", "1");
    // The ticker sleeps: every sweep here is called by hand. A node gone
    // is replaceable at once (the move test).
    k2_core::compute::retry::set_tuning_for_tests(k2_core::compute::retry::Tuning {
        tick_millis: 3_600_000,
        any_hold_secs: 0,
        replace_after_secs: 0,
        ..Default::default()
    });
    k2_core::compute::set_placement_policy(k2_core::compute::PlacementPolicy::V2);
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

fn git(dir: &Path, args: &[&str]) {
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
}

/// A workspace (a git repo, compute on for its agent).
fn seed_ws(env: &Env, handle: &str) -> Ws {
    let path = env.home.join(format!("ws-{handle}"));
    std::fs::create_dir_all(&path).unwrap();
    git(&path, &["init", "-q"]);
    std::fs::write(path.join("a.txt"), format!("hello from {handle}\n")).unwrap();
    git(&path, &["add", "."]);
    git(&path, &["commit", "-q", "-m", "one"]);
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

/// An active (confirmed) node row.
fn active_node(id: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let labels = std::collections::BTreeMap::from([("os".to_string(), "linux".to_string())]);
    store::insert_pending_node(&conn, id, id, &format!("{id:0>64}"), "pem", &labels, "owner", "123456", 1).expect("node");
    store::confirm_node(&conn, id, "owner", 2).expect("confirm");
}

fn grant(node: &str, ws: &Ws, max_job_secs: Option<i64>, max_parallel: i64) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let limits = store::GrantLimits { max_job_secs, max_parallel: Some(max_parallel), ..Default::default() };
    store::set_grant(&conn, node, &ws.id, &limits, "owner", 3).expect("grant");
}

/// A node's offer: `free_gb` of `caps_gb` memory free, `running` of 4.
fn offer(rev: u64, free_gb: u64, caps_gb: u64, running: u32) -> Offer {
    Offer {
        share: None,
        revision: rev,
        control: Control::Active,
        control_by: None,
        control_at: None,
        availability: Availability { ok: true, reasons: vec![] },
        caps: Caps { cpu_millis: 8000, mem_bytes: caps_gb * GB, disk_budget_bytes: 100 * GB, hard: true, gpus: 0 },
        free: Free { cpu_millis: 8000, mem_bytes: free_gb * GB, disk_free_bytes: 500 * GB, gpus: 0 },
        slots: vec![],
        running,
        parallel_max: 4,
        refusal: None,
        labels: [("os".to_string(), "linux".to_string())].into(),
        tools: [("git".to_string(), "2.47".to_string())].into(),
        protocol: 6,
        boot_id: "b".into(),
        ledger_id: "l".into(),
        node_version: "0.46.6".into(),
        features: vec!["artifacts".into(), "stall".into(), k2_core::compute::proto::cache::FEATURE_JOB_CACHE.into()], host: None, pkg: None,
    }
}

/// GPU jobs (0.46.11): a node offering `gpu` to this server, its one AMD
/// GPU open to it (`jobs`) or not.
fn gpu_offer(rev: u64, jobs: bool) -> Offer {
    let mut o = offer(rev, 30, 32, 0);
    o.protocol = 9;
    o.features.push(k2_core::compute::proto::gpu::FEATURE_GPU.into());
    o.caps.gpus = 1;
    o.free.gpus = 1;
    o.host = Some(k2_core::compute::proto::host::HostFacts {
        os: "linux".into(),
        arch: "x86_64".into(),
        distro: "arch".into(),
        distro_version: "rolling".into(),
        distro_name: "Arch Linux".into(),
        gpu: Some("amd".into()),
        gpus: vec![k2_core::compute::proto::gpu::GpuFact { vendor: "amd".into(), pci: "0000:c4:00.0".into(), model: "AMD Test APU".into(), render: "renderD128".into(), jobs, ..Default::default() }],
        ..Default::default()
    });
    o
}

/// This server as a tenant on a node another server owns (two servers).
fn shared_view() -> ShareView {
    ShareView {
        role: Role::Tenant,
        slot: 1,
        shared: true,
        servers: 2,
        owner_label: Some("other.k2.dev".to_string()),
        state: ServerStatus::Active,
        limits: ShareLimits::new_server(Trust::Own, false),
        running: 0,
        queued: 0,
        cpu_secs_today: 0,
        expires_in_secs: None,
        skew_secs: None,
        disk_used_bytes: None,
        quotes: vec![],
        priority: None,
    }
}

fn assigns(c: &mut TestConn) -> Vec<String> {
    c.frames()
        .into_iter()
        .filter_map(|f| match f {
            Frame::Assign(a) => Some(a.fence.job_id),
            _ => None,
        })
        .collect()
}

async fn run(port: u16, token: &str, ws: &Ws, body: Value) -> Resp {
    let mut b = body;
    b["argv"] = json!(["true"]);
    if b.get("noSource").is_none() {
        b["noSource"] = json!(true);
    }
    b["workspace"] = json!(ws.id);
    b["clientJobId"] = json!(uuid::Uuid::new_v4().simple().to_string());
    if b.get("memBytes").is_none() {
        b["memBytes"] = json!(2 * GB);
    }
    post(port, "/cli/compute/run", token, b).await
}

fn node_of(r: &Resp) -> String {
    let v = j(r);
    v["job"]["nodeId"].as_str().unwrap_or_else(|| panic!("no job: {v}")).to_string()
}

fn row(job: &str) -> store::JobRow {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::job_by_id(&conn, job).expect("job row")
}

async fn set_policy(port: u16, p: &str) {
    let r = post(port, "/cli/settings/update", OWNER, json!({"computePlacement": p})).await;
    assert_eq!(r.status, 200, "{}", r.body);
}

// ── the tests ────────────────────────────────────────────────────────

/// Auto by default, grants never crossed, the record everywhere.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn auto_placement_uses_granted_nodes_and_says_why() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let me = seed_ws(&env, "me");
    let other = seed_ws(&env, "otherws");
    for n in ["pa", "pb", "px", "psmall"] {
        active_node(n);
    }
    grant("pa", &me, None, 2);
    grant("pb", &me, None, 2);
    grant("psmall", &me, None, 2);
    // `px` is the biggest, idle node, granted only to the other workspace.
    grant("px", &other, None, 2);
    let mut a = compute_ws::test_attach("pa", offer(1, 30, 32, 0));
    let mut b = compute_ws::test_attach("pb", offer(1, 30, 32, 0));
    let mut x = compute_ws::test_attach("px", offer(1, 120, 128, 0));
    let _s = compute_ws::test_attach("psmall", offer(1, 4, 4, 0));

    // 1. No node named, the owner: auto, never the ungranted px (D1).
    let r = run(d.port, OWNER, &me, json!({})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = j(&r);
    let first = node_of(&r);
    assert!(["pa", "pb", "psmall"].contains(&first.as_str()), "a granted node: {v}");
    let p = &v["placement"];
    assert_eq!((p["mode"].as_str(), p["policy"].as_str(), p["code"].as_str(), p["startsNow"].as_bool()), (Some("auto"), Some("v2"), Some("starts_now_idle"), Some(true)), "{v}");
    assert_eq!(p["node"], first.as_str());
    assert_eq!(p["considered"], 3, "{p}");
    assert!(p["why"].as_str().unwrap().starts_with("starts now · idle · "), "{p}");
    assert_eq!(p["fits"], "3 nodes fit", "{p}");
    assert_eq!(v["job"]["placement"], *p, "the job summary carries it");
    assert!(x.frames().is_empty(), "px never got a job");
    let job1 = v["job"]["id"].as_str().unwrap().to_string();
    assert_eq!(row(&job1).ctl.placement.as_ref().map(|p| p.mode.as_str()), Some("auto"), "stored on the job");
    let ev: String = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row("SELECT detail_json FROM compute_events WHERE kind = 'job_submit' AND job_id = ?1", params![job1], |r| r.get(0)).unwrap()
    };
    assert!(ev.contains("\"placement\""), "the job_submit event carries it: {ev}");

    // 2. A burst spreads: the next two go to the two other granted nodes
    //    (the first is no longer idle).
    let mut seen = vec![first.clone()];
    for _ in 0..2 {
        let r = run(d.port, OWNER, &me, json!({"memBytes": GB})).await;
        assert_eq!(r.status, 200, "{}", r.body);
        seen.push(node_of(&r));
    }
    let mut uniq = seen.clone();
    uniq.sort();
    uniq.dedup();
    assert_eq!(uniq, vec!["pa", "pb", "psmall"], "one job per idle node before any takes a second: {seen:?}");

    // 3. `--any` is the same as naming none.
    let r = run(d.port, OWNER, &me, json!({"any": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(j(&r)["placement"]["mode"], "auto");
    assert!(x.frames().is_empty());

    // 4. A named node pins it; the owner may still name an ungranted one.
    let r = run(d.port, OWNER, &me, json!({"node": "px"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(node_of(&r), "px");
    let p = &j(&r)["placement"];
    assert_eq!((p["mode"].as_str(), p["code"].as_str()), (Some("pinned"), Some("pinned")), "{p}");
    assert_eq!(assigns(&mut x).len(), 1);

    // 5. --mem 8G: psmall (4 GB caps) is skipped and counted.
    let r = run(d.port, OWNER, &me, json!({"memBytes": 8 * GB})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let p = j(&r)["placement"].clone();
    assert_ne!(node_of(&r), "psmall");
    assert_eq!(p["skipped"]["too_small"], 1, "{p}");
    assert!(p["fits"].as_str().unwrap().ends_with("1 skipped (1 too small for --mem 8G)"), "{p}");

    // 6. Nothing fits: no_node_fits with the counts, never px.
    let r = run(d.port, OWNER, &me, json!({"memBytes": 100 * GB})).await;
    assert_eq!((r.status, code(&r)), (404, "no_node_fits".to_string()), "{}", r.body);
    let hint = j(&r)["error"]["hint"].as_str().unwrap().to_string();
    assert!(hint.contains("3 too small for --mem 100G") && !hint.contains("px"), "{hint}");

    // 7. A workspace with no grant at all: the teaching refusal, the owner too.
    let none = seed_ws(&env, "nogrants");
    let r = run(d.port, OWNER, &none, json!({})).await;
    assert_eq!((r.status, code(&r)), (404, "no_node_fits".to_string()), "{}", r.body);
    assert!(j(&r)["error"]["hint"].as_str().unwrap().contains("no compute node is granted to workspace nogrants"), "{}", r.body);

    // 8. The agent passport (S5): its record names only its granted nodes,
    //    no other workspace's id or handle, in the run, get and --plan.
    let agent = passport(&me);
    let r = run(d.port, &agent, &me, json!({})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let job = j(&r)["job"]["id"].as_str().unwrap().to_string();
    let g = get(d.port, "/cli/compute/jobs/get", &agent, &format!("job={job}")).await;
    assert_eq!(g.status, 200, "{}", g.body);
    assert_eq!(j(&g)["placement"]["mode"], "auto");
    let pl = post(d.port, "/cli/compute/run", &agent, json!({"argv": ["true"], "noSource": true, "memBytes": GB, "plan": true})).await;
    assert_eq!(pl.status, 200, "{}", pl.body);
    assert_eq!(j(&pl)["placement"]["mode"], "auto", "--plan shows the placement");
    for text in [j(&r)["placement"].to_string(), j(&g)["placement"].to_string(), j(&pl)["placement"].to_string()] {
        for secret in ["px", other.id.as_str(), "otherws"] {
            assert!(!text.contains(secret), "{secret} leaked into {text}");
        }
    }
    let r = run(d.port, &agent, &me, json!({"node": "px"})).await;
    assert_eq!((r.status, code(&r)), (403, "not_granted".to_string()), "an agent can't pin an ungranted node");

    // 9. --shards without --any: one shard per node; a node is refused.
    let r = post(
        d.port,
        "/cli/compute/run",
        OWNER,
        json!({"workspace": me.id, "argv": ["sh", "-c", "echo {shard}/{shards}"], "noSource": true, "memBytes": GB, "shards": 2, "clientJobId": "client-place-shards-0001"}),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    let shards = j(&r)["shards"].as_array().unwrap().clone();
    assert_eq!(shards.len(), 2);
    assert_ne!(shards[0]["node"], shards[1]["node"], "{shards:?}");
    let r = post(
        d.port,
        "/cli/compute/run",
        OWNER,
        json!({"workspace": me.id, "node": "pa", "argv": ["sh", "-c", "echo {shard}"], "noSource": true, "shards": 2, "clientJobId": "client-place-shards-0002"}),
    )
    .await;
    assert_eq!((r.status, code(&r)), (400, "bad_request".to_string()), "{}", r.body);
    let _ = (assigns(&mut a), assigns(&mut b));
    for n in ["pa", "pb", "px", "psmall"] {
        compute_ws::test_detach(n);
    }
}

/// F10: --timeout above a grant's max skips that node instead of failing
/// after the pick.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_timeout_over_a_grant_skips_that_node() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let me = seed_ws(&env, "tmo");
    active_node("t-short");
    active_node("t-long");
    grant("t-short", &me, Some(3600), 2);
    grant("t-long", &me, Some(4 * 3600), 2);
    // t-short is idle and bigger: it would win on load.
    let _a = compute_ws::test_attach("t-short", offer(1, 60, 64, 0));
    let _b = compute_ws::test_attach("t-long", offer(1, 10, 32, 2));
    let r = run(d.port, OWNER, &me, json!({"timeoutSecs": 3 * 3600})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(node_of(&r), "t-long", "{}", r.body);
    assert_eq!(j(&r)["placement"]["skipped"]["timeout_over_grant"], 1);
    let r = run(d.port, OWNER, &me, json!({"timeoutSecs": 5 * 3600})).await;
    assert_eq!((r.status, code(&r)), (404, "no_node_fits".to_string()), "{}", r.body);
    assert!(j(&r)["error"]["hint"].as_str().unwrap().contains("2 with a grant shorter than --timeout 5h00m"), "{}", r.body);
    // --cache with no node named and no earlier job with it (CE-6): it
    // starts empty there, and the record and a note say so.
    let r = run(d.port, OWNER, &me, json!({"cache": "cp24"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = j(&r);
    assert!(v["placement"]["why"].as_str().unwrap().ends_with("· cache cp24 starts empty here (no earlier job of this workspace used it)"), "{v}");
    assert!(v["warnings"].as_array().unwrap().iter().any(|w| w.as_str().unwrap() == format!("--cache cp24: no earlier job of this workspace used it, so it starts empty on {}", node_of(&r))), "{v}");
    // --os spellings fold (the nodes say `linux`): `Linux` fits; `darwin`
    // (= macos) skips both with the os/arch count.
    let r = run(d.port, OWNER, &me, json!({"os": "Linux"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = run(d.port, OWNER, &me, json!({"os": "darwin"})).await;
    assert_eq!((r.status, code(&r)), (404, "no_node_fits".to_string()), "{}", r.body);
    assert!(j(&r)["error"]["hint"].as_str().unwrap().contains("2 of another --os/--arch/--label"), "{}", r.body);
    // A named node keeps today's refusal (timeout_over_grant, 400).
    let r = run(d.port, OWNER, &me, json!({"node": "t-short", "timeoutSecs": 3 * 3600})).await;
    assert_eq!((r.status, code(&r)), (400, "timeout_over_grant".to_string()), "{}", r.body);
    compute_ws::test_detach("t-short");
    compute_ws::test_detach("t-long");
}

/// GPU jobs (0.46.11): `--gpu` places only on a node that runs GPU jobs
/// for this server with a grant that says `--gpu`; the agent can't set
/// that grant itself (`gpu_needs_grant`, its card); no such node is
/// `no_gpu_node` naming both steps; a pinned node without a GPU for us is
/// refused, never run on the CPU; a job without `--gpu` places as before.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gpu_jobs_place_only_on_granted_gpu_nodes() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let me = seed_ws(&env, "gpus");
    let agent = passport(&me);
    let gpu = json!({"vendor": "any", "count": 1});
    active_node("g-plain");
    active_node("g-gpu");
    grant("g-plain", &me, None, 2);
    grant("g-gpu", &me, None, 2);
    // g-plain is idle and bigger: without --gpu it wins on load.
    let _a = compute_ws::test_attach("g-plain", offer(1, 60, 64, 0));
    let _b = compute_ws::test_attach("g-gpu", gpu_offer(1, true));
    // The grant lacks --gpu: refused, naming the node (the card's param).
    let r = run(d.port, &agent, &me, json!({"gpu": gpu})).await;
    assert_eq!((r.status, code(&r)), (403, "gpu_needs_grant".to_string()), "{}", r.body);
    assert_eq!(j(&r)["error"]["node"], "g-gpu", "{}", r.body);
    assert!(j(&r)["error"]["hint"].as_str().unwrap().contains("k2 compute grant g-gpu --workspace gpus --gpu"), "{}", r.body);
    // The agent can't give itself the right.
    let r = post(d.port, "/cli/compute/grants/set", &agent, json!({"node": "g-gpu", "workspace": me.id, "allowGpu": true})).await;
    assert_eq!(r.status, 403, "{}", r.body);
    // The owner grants it.
    let r = post(d.port, "/cli/compute/grants/set", OWNER, json!({"node": "g-gpu", "workspace": me.id, "allowGpu": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(j(&r)["grant"]["allowGpu"], true, "{}", r.body);
    // Placed on the GPU node only, with the ask in its plan.
    let r = run(d.port, &agent, &me, json!({"gpu": gpu})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(node_of(&r), "g-gpu", "{}", r.body);
    assert_eq!(j(&r)["placement"]["skipped"]["no_gpu"], 1, "{}", r.body);
    let job = j(&r)["job"]["id"].as_str().unwrap().to_string();
    let plan = row(&job).plan().expect("a plan");
    assert_eq!(plan.gpu, Some(k2_core::compute::proto::gpu::GpuAsk::one(k2_core::compute::proto::gpu::GpuVendor::Any)));
    // --plan shows the GPU it would get.
    let r = run(d.port, &agent, &me, json!({"gpu": gpu, "plan": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = j(&r);
    assert_eq!((v["gpu"]["ask"]["vendor"].as_str(), v["gpu"]["gpus"][0]["pci"].as_str()), (Some("any"), Some("0000:c4:00.0")), "{v}");
    // No --gpu: unchanged (the bigger idle node).
    let r = run(d.port, &agent, &me, json!({})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(node_of(&r), "g-plain", "{}", r.body);
    assert!(row(&j(&r)["job"]["id"].as_str().unwrap().to_string()).plan().expect("a plan").gpu.is_none());
    // A node named without a GPU for us: refused (never on the CPU).
    let r = run(d.port, &agent, &me, json!({"node": "g-plain", "gpu": gpu})).await;
    assert_eq!((r.status, code(&r)), (404, "no_gpu_node".to_string()), "{}", r.body);
    // Another vendor than the node has: no node.
    let r = run(d.port, &agent, &me, json!({"gpu": {"vendor": "intel", "count": 1}})).await;
    assert_eq!((r.status, code(&r)), (404, "no_gpu_node".to_string()), "{}", r.body);
    // The node's owner turned GPU jobs off for us: no node, both steps named.
    compute_ws::test_frame("g-gpu", Frame::Offer(gpu_offer(2, false)));
    let r = run(d.port, &agent, &me, json!({"gpu": gpu})).await;
    assert_eq!((r.status, code(&r)), (404, "no_gpu_node".to_string()), "{}", r.body);
    let hint = j(&r)["error"]["hint"].as_str().unwrap().to_string();
    assert!(hint.contains("sudo k2-node admin gpu on") && hint.contains("--workspace gpus --gpu"), "{hint}");
    // NVIDIA and two GPUs are refused at the door (exit 2).
    for bad in [json!({"vendor": "nvidia", "count": 1}), json!({"vendor": "any", "count": 2}), json!({"vendor": "AMD"})] {
        let r = run(d.port, &agent, &me, json!({"gpu": bad})).await;
        assert_eq!(r.status, 400, "{bad}: {}", r.body);
    }
    compute_ws::test_detach("g-plain");
    compute_ws::test_detach("g-gpu");
}

/// D2 at the route (CP-A9: the commit is resolved at submit): a warm,
/// lightly loaded node beats a cold idle one; a warm, heavily loaded one
/// loses to it. The v1 switch restores the old order (warm first).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn warm_vs_idle_follows_d2_and_v1_restores_the_old_order() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let me = seed_ws(&env, "warm");
    active_node("w-warm");
    active_node("w-cold");
    grant("w-warm", &me, None, 4);
    grant("w-cold", &me, None, 4);
    // The project key and commit this workspace's runs build (a --plan
    // pinned to a node; nothing stored).
    let mut probe = compute_ws::test_attach("w-cold", offer(1, 30, 32, 0));
    let r = run(d.port, OWNER, &me, json!({"noSource": false, "node": "w-cold"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let job = j(&r)["job"]["id"].as_str().unwrap().to_string();
    let plan = row(&job).plan().expect("plan");
    let src = plan.src.expect("source");
    let _ = assigns(&mut probe);
    // Settle that job so w-cold is idle again.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::set_job_state(&conn, &job, k2_core::compute::proto::frames::JobState::Cancelled, Some("cancelled"), None, k2_core::compute::now()).unwrap();
    }
    let warm_offer = |rev: u64, free_gb: u64, running: u32| {
        let mut o = offer(rev, free_gb, 32, running);
        o.slots = vec![SlotInfo { project_key: src.project_key.clone(), slot: 0, busy: false, last_sha: Some(src.commit.clone()) }];
        o
    };
    compute_ws::test_frame("w-cold", Frame::Offer(offer(2, 30, 32, 0)));
    // Warm and light (1 of 4 running, 28 GB free) vs cold and idle: warm.
    let _w = compute_ws::test_attach("w-warm", warm_offer(1, 28, 1));
    let r = run(d.port, OWNER, &me, json!({"noSource": false})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let p = j(&r)["placement"].clone();
    assert_eq!((node_of(&r).as_str(), p["code"].as_str()), ("w-warm", Some("starts_now_warm")), "{p}");
    assert!(p["why"].as_str().unwrap().contains("warm slot (this commit)"), "{p}");
    // Warm and heavy (3 GB free): the cold idle node wins.
    compute_ws::test_frame("w-warm", Frame::Offer(warm_offer(2, 3, 1)));
    let r = run(d.port, OWNER, &me, json!({"noSource": false})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(node_of(&r), "w-cold", "{}", r.body);
    // v1: warm first, whatever the load.
    compute_ws::test_frame("w-cold", Frame::Offer(offer(3, 30, 32, 0)));
    set_policy(d.port, "v1").await;
    let s = get(d.port, "/cli/settings/get", OWNER, "").await;
    assert_eq!(j(&s)["computePlacement"], "v1");
    let r = run(d.port, OWNER, &me, json!({"noSource": false, "memBytes": GB})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!((node_of(&r).as_str(), j(&r)["placement"]["policy"].as_str()), ("w-warm", Some("v1")), "{}", r.body);
    let bad = post(d.port, "/cli/settings/update", OWNER, json!({"computePlacement": "v3"})).await;
    assert_eq!(bad.status, 400, "{}", bad.body);
    set_policy(d.port, "v2").await;
    assert_eq!(k2_core::compute::placement_policy(), k2_core::compute::PlacementPolicy::V2);
    compute_ws::test_detach("w-warm");
    compute_ws::test_detach("w-cold");
}

/// Two servers on one node (CP-A6): the shared mini's other server holds
/// 6 of its 8 GB, so it is not "now" for a 4 GB job even with no quote,
/// and the job goes to the solo node; with room it is. Then a node that
/// drops offline: the queued auto job moves and the record says so.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_shared_nodes_other_server_counts_and_a_move_rewrites_the_record() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let me = seed_ws(&env, "two");
    active_node("s-mini");
    active_node("s-solo");
    grant("s-mini", &me, None, 2);
    grant("s-solo", &me, None, 2);
    // The mini: 8 GB, the other server's jobs hold 6 of it (node-wide free
    // 2 GB); this server's share has nothing running there.
    let mut mini_offer = offer(1, 2, 8, 1);
    mini_offer.features.push(FEATURE_QUEUE.to_string());
    mini_offer.share = Some(shared_view());
    let mut mini = compute_ws::test_attach("s-mini", mini_offer.clone());
    let mut solo = compute_ws::test_attach("s-solo", offer(1, 20, 32, 0));
    let r = run(d.port, OWNER, &me, json!({"memBytes": 4 * GB})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(node_of(&r), "s-solo", "the mini's free memory is node-wide: {}", r.body);
    assert_eq!(j(&r)["placement"]["startsNow"], true);
    assert!(assigns(&mut mini).is_empty());
    assert_eq!(assigns(&mut solo).len(), 1);
    // The other server's jobs end (8 GB free on the mini) and the solo
    // node takes one job at a time (full with ours): the mini starts it,
    // into its queue at once.
    let mut roomy = mini_offer.clone();
    roomy.revision = 2;
    roomy.free.mem_bytes = 8 * GB;
    roomy.running = 0;
    compute_ws::test_frame("s-mini", Frame::Offer(roomy.clone()));
    let mut one_slot = offer(2, 20, 32, 1);
    one_slot.parallel_max = 1;
    compute_ws::test_frame("s-solo", Frame::Offer(one_slot));
    let r = run(d.port, OWNER, &me, json!({"memBytes": 4 * GB})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(node_of(&r), "s-mini", "{}", r.body);
    assert_eq!(j(&r)["placement"]["code"], "starts_now_idle", "{}", r.body);
    assert_eq!(assigns(&mut mini).len(), 1);

    // A move: the mini drops off; an auto job waits on the (full) solo
    // node beside a pinned one. Then the solo node goes offline and the
    // mini is back with room: the auto job moves, the pinned one stays.
    compute_ws::test_detach("s-mini");
    let r = run(d.port, OWNER, &me, json!({"memBytes": GB, "node": "s-solo"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let pinned = j(&r)["job"]["id"].as_str().unwrap().to_string();
    let r = run(d.port, OWNER, &me, json!({"memBytes": GB})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let auto = j(&r)["job"]["id"].as_str().unwrap().to_string();
    let first = j(&r)["placement"].clone();
    // P2 (0.46.7): a solo node's wait is estimated: the pinned job ahead
    // and the running one, 15 min each with no history.
    assert_eq!(
        (node_of(&r).as_str(), first["code"].as_str(), first["startsNow"].as_bool(), first["etaSecs"].as_u64()),
        ("s-solo", Some("queued_est_wait"), Some(false), Some(1800)),
        "{first}"
    );
    assert!(first["why"].as_str().unwrap().starts_with("no node free now; shortest wait ~30 min (estimate)"), "{first}");
    assert_eq!((row(&auto).state.as_str(), row(&pinned).state.as_str()), ("queued", "queued"));
    compute_ws::test_detach("s-solo");
    let mut back = roomy.clone();
    back.revision = 3;
    let _mini2 = compute_ws::test_attach("s-mini", back);
    k2_daemon::compute_place::sweep(k2_core::compute::now());
    let moved = row(&auto);
    assert_eq!(moved.node_id, "s-mini", "{moved:?}");
    let p = moved.ctl.placement.expect("record");
    assert_eq!((p.code.as_str(), p.node.as_str(), p.mode.as_str()), ("moved_node_offline", "s-mini", "auto"));
    assert_eq!(u64::from(p.considered), first["considered"].as_u64().unwrap(), "a move keeps `considered`");
    assert_eq!(row(&pinned).node_id, "s-solo", "a pinned job never moves");
    let g = get(d.port, "/cli/compute/jobs/get", OWNER, &format!("job={auto}")).await;
    assert_eq!(j(&g)["placement"]["code"], "moved_node_offline", "{}", g.body);
    compute_ws::test_detach("s-mini");
}

// ── compute easy mode, Lane A (prd-compute-easy-mode-0467) ───────────

/// A node with `needs`, and the tools named (`rust-toolchains`, `cargo-nextest`, `mise:<tool>`).
fn tooled(rev: u64, free_gb: u64, tools: &[(&str, &str)]) -> Offer {
    let mut o = offer(rev, free_gb, 32, 0);
    o.features.push("needs".into());
    for (k, v) in tools {
        o.tools.insert(k.to_string(), v.to_string());
    }
    o
}

const RUST_READY: &[(&str, &str)] = &[("rust-toolchains", "stable-x86_64-unknown-linux-gnu"), ("cargo-nextest", "0.9.100")];

fn job_count() -> i64 {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT COUNT(*) FROM compute_jobs", [], |r| r.get(0)).unwrap()
}

/// CE-11 prefixes, CE-17 --now, CE-13 --exclusive, CE-1/CE-2 tools in
/// --plan.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn prefixes_now_exclusive_and_tools() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let me = seed_ws(&env, "easy");
    let other = seed_ws(&env, "easyother");
    for n in ["k2n12-vin", "k2n1-m4", "k2n10-vin", "k2n99-x"] {
        active_node(n);
    }
    for n in ["k2n12-vin", "k2n1-m4", "k2n10-vin"] {
        grant(n, &me, None, 2);
    }
    grant("k2n99-x", &other, None, 2);
    // All three of mine have 1 GB free: a 2 GB job starts nowhere now.
    let _a = compute_ws::test_attach("k2n12-vin", tooled(1, 1, RUST_READY));
    let _b = compute_ws::test_attach("k2n1-m4", tooled(1, 1, &[]));
    let _c = compute_ws::test_attach("k2n10-vin", tooled(1, 1, &[]));
    let _x = compute_ws::test_attach("k2n99-x", tooled(1, 30, &[]));
    let agent = passport(&me);

    // CE-11: a unique prefix finds the node; an ambiguous one lists them;
    // an agent's prefix never reaches an ungranted node; owner verbs need
    // the full name.
    let r = run(d.port, &agent, &me, json!({"node": "k2n12", "plan": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(j(&r)["node"], "k2n12-vin");
    let r = run(d.port, &agent, &me, json!({"node": "k2n1", "plan": true})).await;
    assert_eq!((r.status, code(&r)), (400, "node_ambiguous".to_string()), "{}", r.body);
    let hint = j(&r)["error"]["hint"].as_str().unwrap().to_string();
    assert!(hint.contains("k2n1-m4") && hint.contains("k2n10-vin") && !hint.contains("k2n99"), "{hint}");
    let r = run(d.port, &agent, &me, json!({"node": "k2n9", "plan": true})).await;
    assert_eq!((r.status, code(&r)), (404, "node_not_found".to_string()), "an ungranted node never matches: {}", r.body);
    let r = post(d.port, "/cli/compute/nodes/remove", OWNER, json!({"node": "k2n12"})).await;
    assert_eq!((r.status, code(&r)), (404, "node_not_found".to_string()), "{}", r.body);

    // CE-1/CE-2: --plan's needs on the picked node, the tools line, the
    // ranking (top 3, never an ungranted node).
    let r = run(d.port, &agent, &me, json!({"node": "k2n12-vin", "needs": ["rust@stable", "cmake@3"], "plan": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let p = j(&r);
    assert_eq!(p["needs"], json!([{"need": "rust@stable", "state": "ready", "have": "stable-x86_64-unknown-linux-gnu"}, {"need": "cmake@3", "state": "install", "have": null}]));
    assert_eq!(p["toolsLine"], "installs cmake@3 first (~minutes)");
    let rk = p["ranking"].as_array().unwrap();
    assert_eq!(rk.len(), 3, "{p}");
    assert!(rk.iter().all(|x| x["node"] != "k2n99-x"), "{p}");

    // CE-17: nothing starts now → not_now (exit 3), nothing stored.
    let before = job_count();
    let r = run(d.port, &agent, &me, json!({"now": true})).await;
    assert_eq!((r.status, code(&r)), (409, "not_now".to_string()), "{}", r.body);
    assert!(j(&r)["error"]["hint"].as_str().unwrap().contains("the shortest wait is "), "{}", r.body);
    let r = run(d.port, &agent, &me, json!({"now": true, "node": "k2n12"})).await;
    assert_eq!((r.status, code(&r)), (409, "not_now".to_string()), "a named node must start it now: {}", r.body);
    assert_eq!(job_count(), before, "--now stored nothing");
    let r = run(d.port, &agent, &me, json!({"now": true, "tier": "batch"})).await;
    assert_eq!(r.status, 400, "{}", r.body);
    compute_ws::test_frame("k2n1-m4", Frame::Offer(tooled(2, 30, &[])));
    let r = run(d.port, &agent, &me, json!({"now": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(node_of(&r), "k2n1-m4");

    // CE-13: --exclusive needs --why (exit 2) and the grant's CI right
    // (exit 3 + the compute.ci card); the owner may.
    let r = run(d.port, &agent, &me, json!({"exclusive": true, "node": "k2n1-m4"})).await;
    assert_eq!((r.status, code(&r)), (400, "exclusive_needs_why".to_string()), "{}", r.body);
    let r = run(d.port, &agent, &me, json!({"exclusive": true, "why": "a benchmark", "node": "k2n1-m4"})).await;
    assert_eq!((r.status, code(&r)), (403, "exclusive_needs_grant".to_string()), "{}", r.body);
    let r = run(d.port, &agent, &me, json!({"exclusive": true, "why": "a benchmark"})).await;
    assert_eq!((r.status, code(&r)), (403, "exclusive_needs_grant".to_string()), "auto too: {}", r.body);
    let r = run(d.port, OWNER, &me, json!({"exclusive": true, "why": "a benchmark", "node": "k2n10-vin"})).await;
    assert_eq!(r.status, 200, "the owner may: {}", r.body);
    assert_eq!(row(j(&r)["job"]["id"].as_str().unwrap()).ctl.why.as_deref(), Some("a benchmark"));
    for n in ["k2n12-vin", "k2n1-m4", "k2n10-vin", "k2n99-x"] {
        compute_ws::test_detach(n);
    }
}

/// CE-3 (with CE-15, D2): .k2/compute.toml defaults and presets, read by
/// the daemon from the workspace folder; flags win; a bad file refuses
/// with its line; the effective settings ride on the job.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn presets_fill_the_run_and_ride_on_the_job() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let me = seed_ws(&env, "presets");
    let dir = env.home.join("ws-presets");
    std::fs::create_dir_all(dir.join(".k2")).unwrap();
    std::fs::write(
        dir.join(".k2/compute.toml"),
        "[defaults]\nos = \"linux\"\nmem = \"8G\"\nneeds = [\"rust@stable\", \"cmake@3\"]\n\n[presets.daemon]\nmem = \"7G\"\nneeds = [\"rust@stable\", \"cmake@3\", \"sys:pkg-config\"]\nenv = { CARGO_BUILD_JOBS = \"2\" }\n",
    )
    .unwrap();
    active_node("pr-a");
    grant("pr-a", &me, None, 2);
    let _a = compute_ws::test_attach("pr-a", tooled(1, 30, RUST_READY));
    let plan_body = |extra: Value| {
        let mut b = json!({"workspace": me.id, "argv": ["true"], "noSource": true, "plan": true});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        b
    };
    // Acceptance 1: defaults, each from: defaults.
    let r = post(d.port, "/cli/compute/run", OWNER, plan_body(json!({}))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let e = j(&r)["effective"].clone();
    assert_eq!(e["source"], ".k2/compute.toml");
    assert_eq!(e["os"], json!({"value": "linux", "from": "defaults"}));
    assert_eq!((e["mem"]["value"].as_str(), e["mem"]["from"].as_str()), (Some("8G"), Some("defaults")));
    assert_eq!(e["needs"], json!({"value": ["rust@stable", "cmake@3"], "from": "defaults"}));
    // CE-15: no --cpus on an 8-CPU node with 4 slots: hints = 2.
    assert_eq!(e["env"]["cpuHints"]["CARGO_BUILD_JOBS"], "2", "{e}");
    // Acceptance 2: --preset daemon --mem 24G.
    let r = post(d.port, "/cli/compute/run", OWNER, plan_body(json!({"preset": "daemon", "memBytes": 24 * GB}))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let e = j(&r)["effective"].clone();
    assert_eq!((e["mem"]["from"].as_str(), e["needs"]["from"].as_str(), e["preset"].as_str()), (Some("flag"), Some("preset"), Some("daemon")));
    assert_eq!(e["env"]["value"], json!(["CARGO_BUILD_JOBS"]));
    assert!(e["env"]["cpuHints"].get("CARGO_BUILD_JOBS").is_none(), "the preset's own CARGO_BUILD_JOBS wins: {e}");
    // An unknown preset lists the ones there are.
    let r = post(d.port, "/cli/compute/run", OWNER, plan_body(json!({"preset": "mac"}))).await;
    assert_eq!((r.status, code(&r)), (400, "preset_not_found".to_string()), "{}", r.body);
    assert!(j(&r)["error"]["hint"].as_str().unwrap().contains("presets daemon"), "{}", r.body);
    // Acceptance 5: a real run keeps the same object on the job.
    let r = run(d.port, OWNER, &me, json!({"preset": "daemon", "tag": "fuzz:p3"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let job = row(j(&r)["job"]["id"].as_str().unwrap());
    let eff = job.ctl.effective.clone().expect("effective on the job");
    assert_eq!((eff["preset"].as_str(), eff["mem"]["from"].as_str()), (Some("daemon"), Some("flag")));
    assert_eq!(job.ctl.tag.as_deref(), Some("fuzz:p3"));
    let plan_v = job.plan().expect("plan");
    assert_eq!(plan_v.needs, vec!["rust@stable", "cmake@3", "sys:pkg-config"], "the file's needs are explicit needs");
    assert_eq!(plan_v.env.get("CARGO_BUILD_JOBS").map(String::as_str), Some("2"));
    let r = run(d.port, OWNER, &me, json!({"tag": "bad tag"})).await;
    assert_eq!((r.status, code(&r)), (400, "tag_invalid".to_string()), "{}", r.body);
    // Acceptance 3: a bad file names its line; nothing is queued.
    std::fs::write(dir.join(".k2/compute.toml"), "[defaults]\nos = \"linux\"\nmem = \"lots\"\n").unwrap();
    let before = job_count();
    let r = run(d.port, OWNER, &me, json!({})).await;
    assert_eq!((r.status, code(&r)), (400, "compute_toml_invalid".to_string()), "{}", r.body);
    assert!(j(&r)["error"]["hint"].as_str().unwrap().starts_with(".k2/compute.toml line 3: [defaults] mem"), "{}", r.body);
    assert_eq!(job_count(), before);
    compute_ws::test_detach("pr-a");
}

/// CE-6: --cache prefers the node that ran this workspace's last job with
/// it; busy for long → elsewhere and the record says where it was; busy
/// for under 15 min → it waits there; --same-node-as pins (another
/// workspace's job: 404).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cache_affinity_and_same_node_as() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let me = seed_ws(&env, "aff");
    let other = seed_ws(&env, "affother");
    for n in ["ca", "cb", "cz"] {
        active_node(n);
    }
    grant("ca", &me, None, 2);
    grant("cb", &me, None, 2);
    grant("cz", &other, None, 2);
    let _a = compute_ws::test_attach("ca", offer(1, 30, 32, 0));
    let _b = compute_ws::test_attach("cb", offer(1, 20, 32, 0));
    let _z = compute_ws::test_attach("cz", offer(1, 20, 32, 0));
    let started = |node: &str, est: i64| {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE compute_jobs SET started_at = ?2, est_secs = ?3 WHERE node_id = ?1", params![node, k2_core::compute::now(), est]).unwrap();
    };
    // Job A ran on cb with cache fz.
    let r = run(d.port, OWNER, &me, json!({"node": "cb", "cache": "fz"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let job_a = j(&r)["job"]["id"].as_str().unwrap().to_string();
    started("cb", 3600);
    // Acceptance 1: B with --cache fz, no node: cb (it starts now there),
    // though ca is roomier.
    let r = run(d.port, OWNER, &me, json!({"cache": "fz"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(node_of(&r), "cb");
    assert_eq!(j(&r)["placement"]["code"], "starts_now_cache", "{}", r.body);
    started("cb", 3600);
    // Acceptance 2: cb is busy (1 GB free) for about an hour: B2 goes to
    // ca, and the record names cb and the wait.
    compute_ws::test_frame("cb", Frame::Offer(offer(2, 1, 32, 0)));
    let r = run(d.port, OWNER, &me, json!({"cache": "fz"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(node_of(&r), "ca");
    let why = j(&r)["placement"]["why"].as_str().unwrap().to_string();
    assert!(why.contains("cache fz isn't on this node (it was on cb, busy ~"), "{why}");
    // The wait there is short now: B3 waits on cb for its cache.
    started("cb", 300);
    let r = run(d.port, OWNER, &me, json!({"cache": "fz"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(node_of(&r), "cb");
    assert_eq!(j(&r)["placement"]["code"], "queued_for_cache", "{}", r.body);
    // Acceptance 3: --same-node-as A pins to cb; another workspace's job
    // is not found.
    let r = run(d.port, OWNER, &me, json!({"sameNodeAs": &job_a[..8]})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(node_of(&r), "cb");
    let v = j(&r);
    assert_eq!(v["placement"]["mode"], "pinned");
    assert!(v["placement"]["why"].as_str().unwrap().starts_with("same node as job "), "{v}");
    assert!(row(v["job"]["id"].as_str().unwrap()).ctl.any.is_none(), "a --same-node-as job never moves");
    let r = run(d.port, OWNER, &other, json!({"node": "cz"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let theirs = j(&r)["job"]["id"].as_str().unwrap().to_string();
    let agent = passport(&me);
    let r = run(d.port, &agent, &me, json!({"sameNodeAs": theirs})).await;
    assert_eq!((r.status, code(&r)), (404, "job_not_found".to_string()), "{}", r.body);
    assert!(!r.body.contains("cz"), "no leak: {}", r.body);
    for n in ["ca", "cb", "cz"] {
        compute_ws::test_detach(n);
    }
}

/// CE-18: prewarm sends a Batch job only where a need isn't ready; a
/// second prewarm (tools now there) sends nothing; POST only.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn prewarm_only_where_tools_are_missing() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let me = seed_ws(&env, "warmup");
    for n in ["pw1", "pw2", "pw3"] {
        active_node(n);
        grant(n, &me, None, 2);
    }
    let _1 = compute_ws::test_attach("pw1", tooled(1, 30, RUST_READY));
    let _2 = compute_ws::test_attach("pw2", tooled(1, 30, &[]));
    let _3 = compute_ws::test_attach("pw3", tooled(1, 30, &[]));
    let body = json!({"workspace": me.id, "needs": ["rust@stable", "cargo-nextest"]});
    let r = get(d.port, "/cli/compute/prewarm", OWNER, &format!("workspace={}", me.id)).await;
    assert_eq!(r.status, 405, "POST only: {}", r.body);
    let mut p = body.clone();
    p["plan"] = json!(true);
    let r = post(d.port, "/cli/compute/prewarm", OWNER, p).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = j(&r);
    assert_eq!(v["jobs"], json!([]), "--plan submits nothing");
    assert_eq!((v["nodes"][0]["node"].as_str(), v["nodes"][0]["state"].as_str()), (Some("pw1"), Some("ready")));
    assert_eq!(v["nodes"][0]["note"], "no tool-cache cap on this node (older k2-node)");
    assert_eq!(v["nodes"][1]["state"], "would_install");
    let before = job_count();
    let r = post(d.port, "/cli/compute/prewarm", &passport(&me), body.clone()).await;
    assert_eq!(r.status, 200, "an agent verb: {}", r.body);
    let v = j(&r);
    assert_eq!(v["jobs"].as_array().unwrap().len(), 2, "{v}");
    assert_eq!(job_count(), before + 2);
    let states: Vec<&str> = v["nodes"].as_array().unwrap().iter().map(|n| n["state"].as_str().unwrap()).collect();
    assert_eq!(states, vec!["ready", "installing", "installing"]);
    let job = row(v["jobs"][0].as_str().unwrap());
    assert_eq!((job.tier.as_str(), job.plan().unwrap().argv), ("batch", vec!["sh".to_string(), "-c".into(), "true".into()]));
    // The tools arrive: a second prewarm sends nothing.
    compute_ws::test_frame("pw2", Frame::Offer(tooled(2, 30, RUST_READY)));
    compute_ws::test_frame("pw3", Frame::Offer(tooled(2, 30, RUST_READY)));
    let r = post(d.port, "/cli/compute/prewarm", OWNER, body).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(j(&r)["jobs"], json!([]), "{}", r.body);
    for n in ["pw1", "pw2", "pw3"] {
        compute_ws::test_detach(n);
    }
}

/// K2 Engine 10-29: in a workspace whose root isn't a repo, a worktree
/// under a nested clone (k2-mpc-engine/.claude/worktrees/x) is found from
/// the caller's folder (its git common dir) without --repo; and --plan
/// checks the source exactly as run does (not_a_repo there too).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn plan_checks_the_source_like_run_and_finds_a_nested_worktree() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let me = seed_ws(&env, "engine");
    let dir = env.home.join("ws-engine");
    std::fs::remove_dir_all(dir.join(".git")).unwrap();
    let clone = dir.join("k2-mpc-engine");
    std::fs::create_dir_all(&clone).unwrap();
    git(&clone, &["init", "-q"]);
    std::fs::write(clone.join("m.rs"), "x\n").unwrap();
    git(&clone, &["add", "."]);
    git(&clone, &["commit", "-q", "-m", "one"]);
    let wt = clone.join(".claude/worktrees/x");
    git(&clone, &["worktree", "add", "-q", "-b", "x", wt.to_str().unwrap()]);
    std::fs::write(wt.join("m.rs"), "y\n").unwrap();
    git(&wt, &["commit", "-q", "-am", "two"]);
    let out = Command::new("git").arg("-C").arg(&wt).args(["rev-parse", "HEAD"]).output().unwrap();
    let wt_head = String::from_utf8(out.stdout).unwrap().trim().to_string();
    active_node("eng-a");
    grant("eng-a", &me, None, 2);
    let _a = compute_ws::test_attach("eng-a", offer(1, 30, 32, 0));
    let body = |extra: Value| {
        let mut b = json!({"workspace": me.id, "argv": ["true"], "memBytes": GB, "plan": true});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        b
    };
    // No --repo, no caller folder: --plan refuses as run does.
    let r = post(d.port, "/cli/compute/run", OWNER, body(json!({}))).await;
    assert_eq!((r.status, code(&r)), (400, "not_a_repo".to_string()), "{}", r.body);
    let mut real = body(json!({}));
    real.as_object_mut().unwrap().remove("plan");
    real["clientJobId"] = json!(uuid::Uuid::new_v4().simple().to_string());
    let r = post(d.port, "/cli/compute/run", OWNER, real).await;
    assert_eq!((r.status, code(&r)), (400, "not_a_repo".to_string()), "run refuses the same: {}", r.body);
    // From the worktree under the nested clone: found, its own HEAD.
    let r = post(d.port, "/cli/compute/run", OWNER, body(json!({"repoDir": wt.to_str().unwrap()}))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let src = j(&r)["source"].clone();
    assert_eq!((src["repo"].as_str(), src["checkout"].as_str(), src["commit"].as_str()), (Some("k2-mpc-engine"), Some("worktree"), Some(wt_head.as_str())), "{src}");
    // A bad --sha is refused by --plan too.
    let r = post(d.port, "/cli/compute/run", OWNER, body(json!({"repoDir": wt.to_str().unwrap(), "sha": "no-such-rev"}))).await;
    assert_eq!(r.status, 400, "{}", r.body);
    compute_ws::test_detach("eng-a");
}

/// CE-12 (CE-A9, prd-compute-easy-mode-0467): a job with no session (the
/// owner token, a script) that times out in the queue still reaches
/// someone: its workspace's inbox, with what it waited on and the retry
/// line. Before 0.46.7 such a timeout was silent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_queue_timeout_without_a_session_reaches_the_inbox() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let me = seed_ws(&env, "qt");
    // Never attached: offline, so the pinned job waits.
    active_node("qt-node");
    grant("qt-node", &me, None, 1);
    let r = run(d.port, OWNER, &me, json!({"node": "qt-node", "queueTimeoutSecs": 60})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let job = j(&r)["job"]["id"].as_str().expect("job id").to_string();
    assert!(row(&job).session_id.is_none(), "an owner-token submit has no session");
    k2_daemon::compute_place::sweep(k2_core::compute::now() + 120);
    let ended = row(&job);
    assert_eq!((ended.state.as_str(), ended.reason.as_deref()), ("failed", Some("queue_timeout")), "{ended:?}");
    let ws_path = env.home.join("ws-qt");
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let items = loop {
        let items = k2_core::inbox::list_all(&ws_path);
        if !items.is_empty() {
            break items;
        }
        assert!(std::time::Instant::now() < deadline, "no inbox message for the queue timeout");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert_eq!(items.len(), 1, "one message: {items:?}");
    let body = k2_core::inbox::read_by_id(&ws_path, &items[0].id).expect("read the inbox item");
    let short = &job[..8];
    assert!(body.contains(&format!("[compute] job {short} on qt-node: failed (queue_timeout): not started within")), "{body}");
    assert!(body.contains(&format!("k2 compute retry {short}")) && body.contains("--queue-timeout"), "{body}");
}

// ── a workspace's node list (k2 compute allow-nodes, 0.46.8) ─────────

const ALLOW: &str = "/cli/compute/allow-nodes";
const ALLOW_SET: &str = "/cli/compute/allow-nodes/set";

/// The node names a list view (`allow-nodes`, `allow-nodes/set`) shows.
fn listed(v: &Value) -> Vec<String> {
    v["nodes"].as_array().unwrap_or_else(|| panic!("no nodes: {v}")).iter().map(|n| n["name"].as_str().unwrap_or("?").to_string()).collect()
}

/// Set, show and clear; an agent narrows its own list (a first list
/// counts) and is refused adding a node or clearing, with the card values
/// in the refusal; the owner does both.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_node_list_is_set_shown_and_cleared_and_an_agent_only_narrows_it() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let me = seed_ws(&env, "lst");
    let other = seed_ws(&env, "lstother");
    for n in ["la", "lb", "lc"] {
        active_node(n);
        grant(n, &me, None, 2);
    }
    let agent = passport(&me);
    let wq = format!("workspace={}", me.id);

    // No list.
    let r = get(d.port, ALLOW, OWNER, &wq).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!((j(&r)["list"].as_bool(), listed(&j(&r)).len()), (Some(false), 0), "{}", r.body);
    let r = get(d.port, ALLOW, &agent, "").await;
    assert_eq!((r.status, j(&r)["list"].as_bool()), (200, Some(false)), "{}", r.body);

    // The agent sets a first list: that narrows (no list = every node).
    let r = post(d.port, ALLOW_SET, &agent, json!({"nodes": "la,lb"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!((j(&r)["change"].as_str(), j(&r)["list"].as_bool()), (Some("narrow"), Some(true)), "{}", r.body);
    assert_eq!(listed(&j(&r)), ["la", "lb"]);
    let r = get(d.port, ALLOW, &agent, "").await;
    assert_eq!(listed(&j(&r)), ["la", "lb"], "{}", r.body);
    assert_eq!(j(&r)["nodes"][0]["granted"], true, "{}", r.body);

    // Adding a node, a swap that adds one, or clearing: a person only.
    for body in [json!({"add": "lc"}), json!({"nodes": ["la", "lc"]})] {
        let r = post(d.port, ALLOW_SET, &agent, body.clone()).await;
        assert_eq!((r.status, code(&r)), (403, "allow_nodes_needs_person".to_string()), "{body}: {}", r.body);
        assert_eq!(j(&r)["error"]["nodes"], "lc", "the card value: {}", r.body);
        assert!(j(&r)["error"].get("clear").is_none(), "{}", r.body);
    }
    let r = post(d.port, ALLOW_SET, &agent, json!({"clear": true})).await;
    assert_eq!((r.status, code(&r)), (403, "allow_nodes_needs_person".to_string()), "{}", r.body);
    assert_eq!(j(&r)["error"]["clear"], true, "{}", r.body);
    assert!(j(&r)["error"].get("nodes").is_none(), "{}", r.body);
    let r = get(d.port, ALLOW, OWNER, &wq).await;
    assert_eq!(listed(&j(&r)), ["la", "lb"], "nothing changed: {}", r.body);

    // Narrowing further is the agent's own; never to nothing.
    let r = post(d.port, ALLOW_SET, &agent, json!({"remove": ["lb"]})).await;
    assert_eq!((r.status, j(&r)["change"].as_str()), (200, Some("narrow")), "{}", r.body);
    assert_eq!(listed(&j(&r)), ["la"]);
    let r = post(d.port, ALLOW_SET, &agent, json!({"remove": "la"})).await;
    assert_eq!((r.status, code(&r)), (400, "bad_request".to_string()), "{}", r.body);
    let r = post(d.port, ALLOW_SET, &agent, json!({"nodes": "la"})).await;
    assert_eq!((r.status, j(&r)["change"].as_str()), (200, Some("same")), "{}", r.body);
    // Never another workspace's list.
    let r = post(d.port, ALLOW_SET, &agent, json!({"workspace": other.id, "nodes": "la"})).await;
    assert_eq!((r.status, code(&r)), (403, "forbidden".to_string()), "{}", r.body);

    // The owner widens (the card's own body) and clears.
    let r = post(d.port, ALLOW_SET, OWNER, json!({"workspace": me.id, "add": ["lb"]})).await;
    assert_eq!((r.status, j(&r)["change"].as_str()), (200, Some("widen")), "{}", r.body);
    assert_eq!(listed(&j(&r)), ["la", "lb"]);
    let r = post(d.port, ALLOW_SET, OWNER, json!({"workspace": me.id, "clear": true})).await;
    assert_eq!((r.status, j(&r)["change"].as_str(), j(&r)["list"].as_bool()), (200, Some("widen"), Some(false)), "{}", r.body);
    let r = get(d.port, ALLOW, OWNER, &wq).await;
    assert_eq!(j(&r)["list"], false, "{}", r.body);
    let events: i64 = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row("SELECT COUNT(*) FROM compute_events WHERE kind = 'allow_nodes' AND workspace_id = ?1", params![me.id], |r| r.get(0)).unwrap()
    };
    assert_eq!(events, 4, "every change is audited (agent set, agent remove, owner add, owner clear)");

    // Bad requests: two asks at once, an unknown node, a GET on the setter.
    let r = post(d.port, ALLOW_SET, OWNER, json!({"workspace": me.id, "nodes": "la", "clear": true})).await;
    assert_eq!((r.status, code(&r)), (400, "bad_request".to_string()), "{}", r.body);
    let r = post(d.port, ALLOW_SET, OWNER, json!({"workspace": me.id, "nodes": "nope"})).await;
    assert_eq!((r.status, code(&r)), (404, "node_not_found".to_string()), "{}", r.body);
    let r = get(d.port, ALLOW_SET, OWNER, &wq).await;
    assert_eq!(r.status, 405, "a mutating route answers GET with 405: {}", r.body);
}

/// With a list, a named node off it is refused (the owner too, every way a
/// job ships source), auto and `--any` only see the list, and the counts
/// say so; without one, everything behaves as before.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_node_list_bounds_named_auto_sharded_and_prewarm_jobs() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let me = seed_ws(&env, "bnd");
    for n in ["ba", "bb", "bc"] {
        active_node(n);
        grant(n, &me, None, 4);
    }
    let _a = compute_ws::test_attach("ba", offer(1, 30, 32, 0));
    let _b = compute_ws::test_attach("bb", offer(1, 30, 32, 0));
    // The biggest, idlest node: the one auto would like best.
    let mut c = compute_ws::test_attach("bc", offer(1, 120, 128, 0));
    let agent = passport(&me);

    // No list: as today. A named bc runs there; the view has no list.
    let r = run(d.port, OWNER, &me, json!({"node": "bc"})).await;
    assert_eq!((r.status, node_of(&r).as_str()), (200, "bc"), "{}", r.body);
    let earlier = j(&r)["job"]["id"].as_str().unwrap().to_string();
    assert_eq!(assigns(&mut c).len(), 1);
    let r = get(d.port, "/cli/compute/nodes", OWNER, &format!("workspace={}", me.id)).await;
    assert!(j(&r)["allowNodes"].is_null(), "{}", r.body);
    assert!(j(&r)["nodes"].as_array().unwrap().iter().all(|n| n.get("allowed").is_none()), "{}", r.body);

    let r = post(d.port, ALLOW_SET, OWNER, json!({"workspace": me.id, "nodes": ["ba", "bb"]})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = get(d.port, "/cli/compute/nodes", OWNER, &format!("workspace={}", me.id)).await;
    assert_eq!(j(&r)["allowNodes"], json!(["ba", "bb"]), "{}", r.body);
    let bc_row = j(&r)["nodes"].as_array().unwrap().iter().find(|n| n["name"] == "bc").cloned().expect("bc listed");
    assert_eq!(bc_row["allowed"], false, "{bc_row}");

    // Named bc: refused for the owner and the agent, whatever ships.
    for (who, body) in [
        (OWNER, json!({"node": "bc"})),
        (agent.as_str(), json!({"node": "bc"})),
        (OWNER, json!({"node": "bc", "plan": true})),
        (OWNER, json!({"node": "bc", "now": true})),
        (OWNER, json!({"node": "bc", "noSource": false, "dirty": true})),
        (OWNER, json!({"node": "bc", "noSource": false, "sha": "HEAD"})),
        (OWNER, json!({"node": "bc", "noSource": false, "with": [{"repo": "sibling"}]})),
        (OWNER, json!({"sameNodeAs": earlier})),
    ] {
        let r = run(d.port, who, &me, body.clone()).await;
        assert_eq!((r.status, code(&r)), (403, "node_not_allowed".to_string()), "{body}: {}", r.body);
        let e = &j(&r)["error"];
        assert_eq!((e["node"].as_str(), e["nodes"].as_str()), (Some("bc"), Some("bc")), "{body}: {e}");
        assert_eq!(e["allowed"], json!(["ba", "bb"]), "{e}");
        assert!(e["hint"].as_str().unwrap().contains("Its list: ba, bb."), "{e}");
    }
    assert!(assigns(&mut c).is_empty(), "bc got nothing after the list");

    // Auto and --any: only ba and bb; bc is counted as off the list.
    for body in [json!({}), json!({"any": true}), json!({"memBytes": 4 * GB}), json!({})] {
        let r = run(d.port, &agent, &me, body.clone()).await;
        assert_eq!(r.status, 200, "{}", r.body);
        assert!(["ba", "bb"].contains(&node_of(&r).as_str()), "{body}: {}", r.body);
        assert_eq!(j(&r)["placement"]["skipped"]["not_allowed"], 1, "{}", r.body);
    }
    // Too big for the list's nodes, though bc fits: no_node_fits, naming the list.
    let r = run(d.port, OWNER, &me, json!({"memBytes": 100 * GB})).await;
    assert_eq!((r.status, code(&r)), (404, "no_node_fits".to_string()), "{}", r.body);
    let hint = j(&r)["error"]["hint"].as_str().unwrap().to_string();
    assert!(hint.contains("1 off this workspace's node list") && hint.contains("go only to its node list: ba, bb"), "{hint}");

    // Shards: each on a node of the list.
    let r = post(
        d.port,
        "/cli/compute/run",
        OWNER,
        json!({"workspace": me.id, "argv": ["sh", "-c", "echo {shard}/{shards}"], "noSource": true, "memBytes": GB, "shards": 3, "clientJobId": "client-list-shards-0001"}),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    for s in j(&r)["shards"].as_array().unwrap() {
        assert!(["ba", "bb"].contains(&s["node"].as_str().unwrap_or("?")), "{s}");
    }
    // Prewarm naming bc.
    let r = post(d.port, "/cli/compute/prewarm", OWNER, json!({"workspace": me.id, "needs": ["rust@stable"], "nodes": ["bc"]})).await;
    assert_eq!((r.status, code(&r)), (403, "node_not_allowed".to_string()), "{}", r.body);
    assert!(assigns(&mut c).is_empty(), "bc still got nothing");
    for n in ["ba", "bb", "bc"] {
        compute_ws::test_detach(n);
    }
}

/// A queued job never moves to a node off the list; it moves to one on
/// it. Narrowing fails the jobs still queued on a node that left it, a
/// kick fails one the list left behind, and a retry there is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_node_list_holds_on_moves_retries_and_narrowing() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let me = seed_ws(&env, "mv");
    for n in ["fa", "fb", "fc"] {
        active_node(n);
        grant(n, &me, None, 2);
    }
    let _a = compute_ws::test_attach("fa", offer(1, 30, 32, 0));
    let _b = compute_ws::test_attach("fb", offer(1, 30, 32, 0));
    let mut c = compute_ws::test_attach("fc", offer(1, 120, 128, 0));
    let r = post(d.port, ALLOW_SET, OWNER, json!({"workspace": me.id, "nodes": ["fa", "fb"]})).await;
    assert_eq!(r.status, 200, "{}", r.body);

    // fb offline, fa full (one slot, held by a pinned job): an auto job
    // waits on fa.
    compute_ws::test_detach("fb");
    let mut one = offer(2, 30, 32, 0);
    one.parallel_max = 1;
    compute_ws::test_frame("fa", Frame::Offer(one));
    let r = run(d.port, OWNER, &me, json!({"node": "fa", "memBytes": GB})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let holder = j(&r)["job"]["id"].as_str().unwrap().to_string();
    assert_eq!(row(&holder).state, "assigned", "it holds fa's one slot");
    let mut full = offer(3, 30, 32, 1);
    full.parallel_max = 1;
    compute_ws::test_frame("fa", Frame::Offer(full));
    let r = run(d.port, OWNER, &me, json!({"memBytes": GB})).await;
    assert_eq!((r.status, node_of(&r).as_str()), (200, "fa"), "{}", r.body);
    let auto = j(&r)["job"]["id"].as_str().unwrap().to_string();
    assert_eq!(row(&auto).state, "queued");

    // fa goes offline; only fc (off the list) has room: it stays put.
    compute_ws::test_detach("fa");
    compute_ws::test_frame("fc", Frame::Offer(offer(2, 120, 128, 0)));
    k2_daemon::compute_place::sweep(k2_core::compute::now());
    assert_eq!(row(&auto).node_id, "fa", "never moved off the list");
    assert!(assigns(&mut c).is_empty(), "fc never got it");

    // fb is back with room: the job moves there.
    let mut b = compute_ws::test_attach("fb", offer(3, 30, 32, 0));
    k2_daemon::compute_place::sweep(k2_core::compute::now());
    let moved = row(&auto);
    assert_eq!(moved.node_id, "fb", "{moved:?}");
    assert!(assigns(&mut b).contains(&auto), "fb runs it");

    // A pinned job waits on the offline fa; the owner narrows the list to
    // fb: the job ends failed/node_not_allowed, and a retry is refused.
    let r = run(d.port, OWNER, &me, json!({"node": "fa"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let pinned = j(&r)["job"]["id"].as_str().unwrap().to_string();
    let r = post(d.port, ALLOW_SET, OWNER, json!({"workspace": me.id, "remove": ["fa"]})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(j(&r)["failedQueued"].as_array().unwrap().contains(&json!(pinned)), "{}", r.body);
    let ended = row(&pinned);
    assert_eq!((ended.state.as_str(), ended.reason.as_deref()), ("failed", Some("node_not_allowed")), "{ended:?}");
    let r = post(d.port, "/cli/compute/jobs/retry", OWNER, json!({"job": pinned})).await;
    assert_eq!((r.status, code(&r)), (403, "node_not_allowed".to_string()), "{}", r.body);

    // A job queued on a full fb; the list then changes in the store alone
    // (no route sweep): the next kick fails it instead of sending it.
    let mut fb_full = offer(4, 30, 32, 1);
    fb_full.parallel_max = 1;
    compute_ws::test_frame("fb", Frame::Offer(fb_full));
    let r = run(d.port, OWNER, &me, json!({"node": "fb"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let left = j(&r)["job"]["id"].as_str().unwrap().to_string();
    assert_eq!(row(&left).state, "queued");
    let _ = assigns(&mut b);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::compute::allow::set(&conn, &me.id, Some(&["fc".to_string()]), "owner", 1).expect("list");
    }
    compute_ws::test_frame("fb", Frame::Offer(offer(5, 30, 32, 0)));
    let ended = row(&left);
    assert_eq!((ended.state.as_str(), ended.reason.as_deref()), ("failed", Some("node_not_allowed")), "{ended:?}");
    assert!(!assigns(&mut b).contains(&left), "never sent to fb");
    for n in ["fb", "fc"] {
        compute_ws::test_detach(n);
    }
}

// ── k2 compute move and --avoid (0.46.8) ─────────────────────────────

const MOVE: &str = "/cli/compute/jobs/move";

/// A node with one slot, held by a running job (as far as its offer says).
fn one_slot_full(rev: u64) -> Offer {
    let mut o = offer(rev, 30, 32, 1);
    o.parallel_max = 1;
    o
}

/// Fill a one-slot node's slot with a pinned job of `ws` (this server's
/// scheduler counts its own jobs, not the offer's `running`): the node's
/// offer at `rev` has the slot free, at `rev + 1` full. The holder's id.
async fn hold_one_slot(port: u16, node: &str, ws: &Ws, rev: u64) -> String {
    let mut free = offer(rev, 30, 32, 0);
    free.parallel_max = 1;
    compute_ws::test_frame(node, Frame::Offer(free));
    let r = run(port, OWNER, ws, json!({"node": node, "memBytes": GB, "cpus": 1})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let id = id_of(&r);
    assert_eq!(row(&id).state, "assigned", "the holder took {node}'s slot");
    compute_ws::test_frame(node, Frame::Offer(one_slot_full(rev + 1)));
    id
}

fn id_of(r: &Resp) -> String {
    j(r)["job"]["id"].as_str().unwrap_or_else(|| panic!("no job id: {}", r.body)).to_string()
}

fn pinned_in(jobs: &Value, id: &str) -> bool {
    let row = jobs["jobs"].as_array().unwrap().iter().find(|x| x["id"] == id).unwrap_or_else(|| panic!("{id} not listed: {jobs}"));
    row["pinned"].as_bool().unwrap_or_else(|| panic!("no pinned field: {row}"))
}

/// A job pinned to a full node stays put until released; `move --auto`
/// makes it auto (keeping its node's OS), records it, and the next sweep
/// moves it to the node that can start it now. Views mark pinned vs auto;
/// the submit hint names the free node; a started job is never moved.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pinned_job_released_to_auto_moves_on_the_next_sweep() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let me = seed_ws(&env, "rel");
    let other = seed_ws(&env, "relother");
    for n in ["ra", "rb"] {
        active_node(n);
        grant(n, &me, None, 2);
    }
    let mut a = compute_ws::test_attach("ra", one_slot_full(1));
    let mut b = compute_ws::test_attach("rb", offer(1, 30, 32, 0));
    let agent = passport(&me);
    hold_one_slot(d.port, "ra", &me, 2).await;

    // No --cpus on a node with 4 slots: a note says what it reserves (8
    // CPUs ÷ 4); with --cpus, or on ra (one job at a time), none.
    let cpu_notes = |r: &Resp| -> Vec<String> {
        j(r)["warnings"].as_array().expect("warnings").iter().map(|w| w.as_str().unwrap().to_string()).filter(|w| w.starts_with("no --cpus")).collect()
    };
    let r = run(d.port, &agent, &me, json!({"node": "rb", "plan": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(
        cpu_notes(&r),
        vec!["no --cpus: this job reserves 2 of the 8 CPUs on rb (8 CPUs ÷ 4 job slots) while it runs, used or not; a job waits for unreserved CPUs even when they sit idle. Pass --cpus N with what it really uses so more jobs fit there".to_string()]
    );
    let r = run(d.port, &agent, &me, json!({"node": "rb", "plan": true, "cpus": 2})).await;
    assert!(cpu_notes(&r).is_empty(), "{}", r.body);
    let r = run(d.port, &agent, &me, json!({"node": "ra", "plan": true})).await;
    assert!(cpu_notes(&r).is_empty(), "one slot: the whole node is the slot: {}", r.body);

    // Pinned to the full ra: queued there, with a hint naming rb.
    let r = run(d.port, &agent, &me, json!({"node": "ra", "memBytes": GB})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let job = id_of(&r);
    let short = &job[..8];
    assert_eq!(row(&job).state, "queued");
    let warns: Vec<String> = j(&r)["warnings"].as_array().unwrap().iter().map(|w| w.as_str().unwrap().to_string()).collect();
    let hint = warns.iter().find(|w| w.starts_with("hint:")).unwrap_or_else(|| panic!("no hint: {warns:?}"));
    assert!(hint.contains("ra can't start this job now") && hint.contains("but rb could"), "{hint}");
    assert!(hint.contains(&format!("k2 compute move {short} --auto")), "{hint}");
    let g = get(d.port, "/cli/compute/jobs", &agent, "state=queued").await;
    assert_eq!(g.status, 200, "{}", g.body);
    assert!(pinned_in(&j(&g), &job), "jobs marks it pinned");
    let st = get(d.port, "/cli/compute/nodes", &agent, "detail=1").await;
    let ra = j(&st)["nodes"].as_array().unwrap().iter().find(|n| n["name"] == "ra").cloned().unwrap();
    assert_eq!(ra["status"]["queued"][0]["pinned"], true, "status marks it pinned: {ra}");

    // The sweeper never moves a pinned job.
    k2_daemon::compute_place::sweep(k2_core::compute::now());
    assert_eq!(row(&job).node_id, "ra", "pinned stays");
    assert!(assigns(&mut b).is_empty());

    // Who: another workspace's agent never sees it; GET is 405.
    let r = post(d.port, MOVE, &passport(&other), json!({"job": job, "auto": true})).await;
    assert_eq!((r.status, code(&r)), (404, "job_not_found".to_string()), "{}", r.body);
    let r = get(d.port, MOVE, &agent, &format!("job={job}&auto=true")).await;
    assert_eq!(r.status, 405, "{}", r.body);

    // Released: auto with its node's OS, a released_auto record, an event;
    // it still waits on ra until the sweep.
    let r = post(d.port, MOVE, &agent, json!({"job": job, "auto": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = j(&r);
    assert_eq!((v["mode"].as_str(), v["node"].as_str(), v["moved"].as_bool()), (Some("auto"), Some("ra"), Some(false)), "{v}");
    assert_eq!(v["filter"]["os"], "linux", "keeps its node's OS: {v}");
    let released = row(&job);
    assert_eq!(released.node_id, "ra");
    assert_eq!(released.ctl.any.as_ref().and_then(|f| f.os.as_deref()), Some("linux"));
    let p = released.ctl.placement.clone().expect("a placement record");
    assert_eq!((p.mode.as_str(), p.code.as_str()), ("auto", "released_auto"), "{p:?}");
    let events: i64 = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row("SELECT COUNT(*) FROM compute_events WHERE kind = 'job_release' AND job_id = ?1", params![job], |r| r.get(0)).unwrap()
    };
    assert_eq!(events, 1, "the release is audited");
    let g = get(d.port, "/cli/compute/jobs", &agent, "state=queued").await;
    assert!(!pinned_in(&j(&g), &job), "jobs marks it auto now");

    // A second release with nothing new: already_auto (exit 3).
    let r = post(d.port, MOVE, &agent, json!({"job": job, "auto": true})).await;
    assert_eq!((r.status, code(&r)), (409, "already_auto".to_string()), "{}", r.body);

    // The next sweep moves it to rb, which starts it.
    k2_daemon::compute_place::sweep(k2_core::compute::now());
    let moved = row(&job);
    assert_eq!(moved.node_id, "rb", "{moved:?}");
    assert_eq!(moved.ctl.placement.as_ref().map(|p| p.code.as_str()), Some("moved_node_busy"));
    assert!(assigns(&mut b).contains(&job), "rb runs it");

    // Started: never moved.
    let r = post(d.port, MOVE, &agent, json!({"job": job, "to": "ra"})).await;
    assert_eq!((r.status, code(&r)), (409, "job_started".to_string()), "{}", r.body);

    // No hint when no other node could start it now.
    compute_ws::test_frame("rb", Frame::Offer(one_slot_full(2)));
    let r = run(d.port, &agent, &me, json!({"node": "ra", "memBytes": GB})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(j(&r)["warnings"].as_array().unwrap().iter().all(|w| !w.as_str().unwrap().starts_with("hint:")), "{}", r.body);
    let _ = assigns(&mut a);
    for n in ["ra", "rb"] {
        compute_ws::test_detach(n);
    }
}

/// `move --to` pins a queued job to another node with the submit checks;
/// the refusals are named; `--all-mine --node N --auto` releases a batch.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn move_to_pins_and_refuses_by_name_and_all_mine_releases_a_batch() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let me = seed_ws(&env, "mvto");
    let other = seed_ws(&env, "mvtoother");
    for n in ["ta", "tb", "tsmall", "tx"] {
        active_node(n);
    }
    for n in ["ta", "tb", "tsmall"] {
        grant(n, &me, None, 2);
    }
    grant("tx", &other, None, 2);
    let _a = compute_ws::test_attach("ta", one_slot_full(1));
    let mut b = compute_ws::test_attach("tb", one_slot_full(1));
    let _s = compute_ws::test_attach("tsmall", offer(1, 4, 4, 0));
    let _x = compute_ws::test_attach("tx", offer(1, 30, 32, 0));
    let agent = passport(&me);
    let ta_holder = hold_one_slot(d.port, "ta", &me, 2).await;
    hold_one_slot(d.port, "tb", &me, 2).await;
    let r = run(d.port, &agent, &me, json!({"node": "ta", "memBytes": 8 * GB})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let job = id_of(&r);

    // Usage: say where; --avoid only with --auto (exit 2).
    let r = post(d.port, MOVE, &agent, json!({"job": job})).await;
    assert_eq!((r.status, code(&r)), (400, "bad_request".to_string()), "{}", r.body);
    let r = post(d.port, MOVE, &agent, json!({"job": job, "auto": true, "to": "tb"})).await;
    assert_eq!((r.status, code(&r)), (400, "bad_request".to_string()), "{}", r.body);
    let r = post(d.port, MOVE, &agent, json!({"job": job, "to": "tb", "avoid": ["ta"]})).await;
    assert_eq!((r.status, code(&r)), (400, "bad_request".to_string()), "{}", r.body);
    // Refusals, each named; the job stays where it is.
    let r = post(d.port, MOVE, &agent, json!({"job": job, "to": "ta"})).await;
    assert_eq!((r.status, code(&r)), (409, "already_on_node".to_string()), "{}", r.body);
    let r = post(d.port, MOVE, &agent, json!({"job": job, "to": "tx"})).await;
    assert_eq!((r.status, code(&r)), (403, "not_granted".to_string()), "{}", r.body);
    let r = post(d.port, MOVE, &agent, json!({"job": job, "to": "tsmall"})).await;
    assert_eq!((r.status, code(&r)), (409, "node_cant_take".to_string()), "{}", r.body);
    assert!(j(&r)["error"]["hint"].as_str().unwrap().contains("too small for --mem 8G"), "{}", r.body);
    assert_eq!(row(&job).node_id, "ta", "nothing moved");

    // --to tb: moved and pinned there, recorded; the sweeper leaves it.
    let r = post(d.port, MOVE, &agent, json!({"job": job, "to": "tb"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!((j(&r)["mode"].as_str(), j(&r)["node"].as_str(), j(&r)["moved"].as_bool()), (Some("pinned"), Some("tb"), Some(true)), "{}", r.body);
    let m = row(&job);
    assert_eq!((m.node_id.as_str(), m.ctl.any.is_none()), ("tb", true), "{m:?}");
    assert_eq!(m.ctl.placement.as_ref().map(|p| (p.mode.as_str(), p.code.as_str())), Some(("pinned", "moved_to")));
    assert_eq!(m.plan().unwrap().node_id, "tb", "the plan was rebuilt for tb");
    compute_ws::test_frame("ta", Frame::Offer(offer(10, 30, 32, 0)));
    k2_daemon::compute_place::sweep(k2_core::compute::now());
    assert_eq!(row(&job).node_id, "tb", "pinned again: never moved by the sweeper");
    let _ = assigns(&mut b);
    compute_ws::test_frame("ta", Frame::Offer(one_slot_full(11)));
    assert_eq!(row(&ta_holder).state, "assigned", "ta stays held");

    // --all-mine --node tb --auto: every queued job this session sent there.
    let mut sent = vec![job.clone()];
    for _ in 0..2 {
        let r = run(d.port, &agent, &me, json!({"node": "tb", "memBytes": GB})).await;
        assert_eq!(r.status, 200, "{}", r.body);
        sent.push(id_of(&r));
    }
    let r = run(d.port, OWNER, &me, json!({"node": "tb", "memBytes": GB})).await;
    let owners = id_of(&r);
    let r = post(d.port, MOVE, &agent, json!({"filter": {"scope": "session", "node": "tb"}, "auto": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = j(&r);
    assert_eq!((v["matched"].as_u64(), v["count"].as_u64()), (Some(3), Some(3)), "{v}");
    for id in &sent {
        assert!(row(id).ctl.any.is_some(), "{id} released");
    }
    assert!(row(&owners).ctl.any.is_none(), "another session's job (the owner's) is left pinned");
    for n in ["ta", "tb", "tsmall", "tx"] {
        compute_ws::test_detach(n);
    }
}

/// `--avoid`: submit never picks the node, the sweeper never moves a job
/// there, a named node with --avoid is exit 2, an avoid list that leaves
/// nothing is exit 3; a release that avoids its own node moves it at once
/// or changes nothing; a release of an auto job keeps its record (CE-6).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn avoid_keeps_jobs_off_a_node_at_submit_on_moves_and_on_release() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let me = seed_ws(&env, "avd");
    for n in ["q2n11-vin", "q2n12-vin", "q2n13-vin"] {
        active_node(n);
        grant(n, &me, None, 2);
    }
    let mut eleven = compute_ws::test_attach("q2n11-vin", offer(1, 30, 32, 0));
    let _twelve = compute_ws::test_attach("q2n12-vin", one_slot_full(1));
    let _thirteen = compute_ws::test_attach("q2n13-vin", one_slot_full(1));
    let agent = passport(&me);
    hold_one_slot(d.port, "q2n12-vin", &me, 2).await;
    hold_one_slot(d.port, "q2n13-vin", &me, 2).await;

    // Refusals first: nothing stored.
    let before = job_count();
    let r = run(d.port, &agent, &me, json!({"node": "q2n12-vin", "avoid": ["q2n11"]})).await;
    assert_eq!((r.status, code(&r)), (400, "avoid_with_pinned_node".to_string()), "{}", r.body);
    let r = run(d.port, &agent, &me, json!({"avoid": ["q2n1"]})).await;
    assert_eq!((r.status, code(&r)), (400, "node_ambiguous".to_string()), "same resolution as a named node: {}", r.body);
    let r = run(d.port, &agent, &me, json!({"avoid": ["q2n11", "q2n12", "q2n13"]})).await;
    assert_eq!((r.status, code(&r)), (409, "avoid_leaves_no_node".to_string()), "{}", r.body);
    assert_eq!(job_count(), before, "nothing queued");

    // Auto with q2n11 (the only free node) avoided: queued on a busy one.
    let r = run(d.port, &agent, &me, json!({"avoid": ["q2n11"], "memBytes": GB})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let job = id_of(&r);
    assert_ne!(node_of(&r), "q2n11-vin");
    let held = row(&job);
    assert_eq!(held.ctl.any.as_ref().map(|f| f.avoid.clone()), Some(vec!["q2n11-vin".to_string()]), "stored by id");
    assert_eq!(held.ctl.placement.as_ref().map(|p| p.skipped.get("avoided").copied()), Some(Some(1)), "the record counts it");
    // The sweeper never moves it to the free, avoided q2n11.
    k2_daemon::compute_place::sweep(k2_core::compute::now());
    assert_eq!(row(&job).node_id, held.node_id, "never to the avoided node");
    assert!(assigns(&mut eleven).is_empty());
    // Releasing it again with one more avoid keeps its placement record.
    let third = if held.node_id == "q2n12-vin" { "q2n13-vin" } else { "q2n12-vin" };
    let r = post(d.port, MOVE, &agent, json!({"job": job, "auto": true, "avoid": [third]})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let again = row(&job);
    assert_eq!(again.ctl.any.as_ref().map(|f| f.avoid.len()), Some(2));
    assert_eq!(again.ctl.placement, held.ctl.placement, "an auto job's record (and a cache wait) is kept");

    // A pinned job released with --avoid of its own node moves now.
    let r = run(d.port, &agent, &me, json!({"node": "q2n12-vin", "memBytes": GB})).await;
    let pinned = id_of(&r);
    let r = post(d.port, MOVE, &agent, json!({"job": pinned, "auto": true, "avoid": ["q2n12"]})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!((j(&r)["moved"].as_bool(), j(&r)["node"].as_str()), (Some(true), Some("q2n11-vin")), "{}", r.body);
    assert_eq!(row(&pinned).node_id, "q2n11-vin");
    assert!(assigns(&mut eleven).contains(&pinned));

    // Same, but every other node is offline: no_other_node, nothing changed.
    compute_ws::test_detach("q2n11-vin");
    compute_ws::test_detach("q2n13-vin");
    let r = run(d.port, &agent, &me, json!({"node": "q2n12-vin", "memBytes": GB})).await;
    let stuck = id_of(&r);
    let r = post(d.port, MOVE, &agent, json!({"job": stuck, "auto": true, "avoid": ["q2n12-vin"]})).await;
    assert_eq!((r.status, code(&r)), (409, "no_other_node".to_string()), "{}", r.body);
    let s = row(&stuck);
    assert_eq!((s.node_id.as_str(), s.ctl.any.is_none()), ("q2n12-vin", true), "{s:?}");
    compute_ws::test_detach("q2n12-vin");
}
