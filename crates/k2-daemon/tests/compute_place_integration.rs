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
        caps: Caps { cpu_millis: 8000, mem_bytes: caps_gb * GB, disk_budget_bytes: 100 * GB, hard: true },
        free: Free { cpu_millis: 8000, mem_bytes: free_gb * GB, disk_free_bytes: 500 * GB },
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
        features: vec!["artifacts".into(), "stall".into(), k2_core::compute::proto::cache::FEATURE_JOB_CACHE.into()],
    }
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
    assert_eq!((node_of(&r).as_str(), first["code"].as_str(), first["startsNow"].as_bool()), ("s-solo", Some("queued_least_busy"), Some(false)), "{first}");
    assert!(first["why"].as_str().unwrap().starts_with("no node free now; least busy"), "{first}");
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
