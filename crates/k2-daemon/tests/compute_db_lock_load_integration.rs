//! Compute bookkeeping never jams the main database lock (0.46.17 follow-up).
//!
//! On the owner's Mac (the compute controller) 0.46.17's `[db] waited` lines
//! went from 2–7k an hour to 265k an hour: short-id job lookups scanned the
//! jobs table under the lock, every `get?wait=` / `logs?wait=` long-poll
//! re-read under it on every compute change (all of them woken at once),
//! and the queue decision of a queued job ran under it. Every UI route
//! waited behind that.
//!
//! This test is that load, headless: the real dispatcher in process on a
//! temp `$HOME`, the main database the file the daemon opens (WAL, read
//! pool), a jobs table with 40k rows of history, 50 `jobs/get?wait=25`
//! long-polls (30 on running jobs, 20 on queued ones, most by the 8-char id
//! the CLI prints), 20 `jobs/logs?wait=25` followers, and two fake nodes
//! sending log chunks, state frames, offers and job ends. Meanwhile it times
//! the GETs an agent switch makes (`thread?limit=25`, `thread/activity`,
//! `sessions/list-for-workspace`, `workspace/agent-display-name`) and counts
//! the main lock's waits.
//!
//! Target: those GETs under 50 ms at p95 under that load. The numbers are
//! printed (`LOAD:` lines) for the before/after comparison.
//!
//! Its own test binary: the file database is published process-wide
//! (`init_database`), so the compute threads read the same database.

#[path = "support/app_fixture.rs"]
mod app_fixture;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use app_fixture::*;
use k2_core::compute::proto::frames::{Availability, Caps, Control, Frame, Free, JobState, LogChunk, LogStream, Offer, StateUpdate};
use k2_core::compute::store;
use k2_daemon::compute_ws;
use k2_daemon::test_harness;
use rusqlite::params;
use serde_json::json;

const GB: u64 = 1 << 30;
const NODE_A: &str = "lockload-a";
const NODE_B: &str = "lockload-b";
const HISTORY: usize = 40_000;
const RUNNING: usize = 40;
const CHURN: usize = 60;
const QUEUED: usize = 20;
const LOAD_FOR: Duration = Duration::from_secs(20);

fn active_node(id: &str, name: &str, ws_id: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let labels = std::collections::BTreeMap::from([("os".to_string(), "linux".to_string())]);
    store::insert_pending_node(&conn, id, name, &format!("{id:0>64}"), "pem", &labels, "owner", "123456", 1).expect("node");
    store::confirm_node(&conn, id, "owner", 2).expect("confirm");
    let limits = store::GrantLimits { max_parallel: Some(500), ..Default::default() };
    store::set_grant(&conn, id, ws_id, &limits, "owner", 3).expect("grant");
}

/// A node's offer; `full`: nothing free, every slot running (its queued
/// jobs stay queued here and get a position).
fn offer(revision: u64, full: bool) -> Offer {
    Offer {
        share: None,
        revision,
        control: Control::Active,
        control_by: None,
        control_at: None,
        availability: Availability { ok: true, reasons: vec![] },
        caps: Caps { cpu_millis: 64_000, mem_bytes: 256 * GB, disk_budget_bytes: 100 * GB, hard: true, gpus: 0 },
        free: if full {
            Free { cpu_millis: 0, mem_bytes: 0, disk_free_bytes: 500 * GB, gpus: 0 }
        } else {
            Free { cpu_millis: 64_000, mem_bytes: 250 * GB, disk_free_bytes: 500 * GB, gpus: 0 }
        },
        slots: vec![],
        running: if full { 4 } else { 0 },
        parallel_max: if full { 4 } else { 500 },
        refusal: None,
        labels: [("os".to_string(), "linux".to_string())].into(),
        tools: [("git".to_string(), "2.47".to_string())].into(),
        protocol: 6,
        boot_id: "b".into(),
        ledger_id: "l".into(),
        node_version: "0.46.6".into(),
        features: ["artifacts", "stall"].iter().map(|f| f.to_string()).collect(),
        host: None,
        pkg: None,
        sim: None,
    }
}

fn state(job: &str, st: JobState) -> Frame {
    Frame::State(StateUpdate {
        queue: None,
        job_id: job.into(),
        generation: 1,
        state: st,
        reason: None,
        detail: None,
        at: k2_core::compute::now(),
        exit: None,
        src: None,
        log_seq: 0,
        outputs: None,
    })
}

fn log(job: &str, seq: u64) -> Frame {
    let text = format!("line {seq}: compiling k2-daemon v0.46.17 (crates/k2-daemon)\n");
    Frame::Log(LogChunk { job_id: job.into(), generation: 1, seq, stream: LogStream::Out, data: k2_core::compute::proto::crypto::b64(text.as_bytes()) })
}

fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// Copies of the template job: `(id, node, state)` each.
fn clone_jobs(template: &str, rows: &[(String, &str, &str)]) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute_batch("BEGIN IMMEDIATE").expect("begin");
    conn.execute_batch("CREATE TEMP TABLE lockload_tpl AS SELECT * FROM compute_jobs WHERE 0").expect("template table");
    assert_eq!(conn.execute("INSERT INTO lockload_tpl SELECT * FROM compute_jobs WHERE id = ?1", params![template]).expect("template copy"), 1, "the template job");
    let now = k2_core::compute::now();
    {
        let mut set = conn
            .prepare(
                "UPDATE lockload_tpl SET id = ?1, client_job_id = ?2, node_id = ?3, state = ?4, reason = NULL, detail = NULL, \
                 attempt = 1, generation = 1, created_at = ?5, started_at = CASE WHEN ?4 = 'queued' THEN NULL ELSE ?5 END, \
                 ended_at = CASE WHEN ?4 IN ('done', 'failed') THEN ?5 + 30 ELSE NULL END, \
                 plan_json = json_set(plan_json, '$.job_id', ?1, '$.node_id', ?3)",
            )
            .expect("prepare update");
        let mut put = conn.prepare("INSERT INTO compute_jobs SELECT * FROM lockload_tpl").expect("prepare insert");
        for (i, (id, node, st)) in rows.iter().enumerate() {
            let created = if *st == "done" || *st == "failed" { now - 86_400 * 30 + i as i64 } else { now - 60 + i as i64 % 50 };
            set.execute(params![id, format!("lockload-{id}"), node, st, created]).expect("set row");
            put.execute([]).expect("insert row");
        }
    }
    conn.execute_batch("DROP TABLE lockload_tpl; COMMIT").expect("commit");
}

fn pct(sorted: &[Duration], p: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let i = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[i.min(sorted.len() - 1)]
}

fn ms(d: Duration) -> String {
    format!("{:.1}ms", d.as_secs_f64() * 1000.0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn compute_long_polls_and_job_churn_leave_the_ui_gets_fast() {
    let env = setup(&[("K2_COMPUTE", "1")]);
    // The file database the daemon opens (WAL, read pool), for every thread.
    let db = k2_core::db::init_database().expect("open the home database under the temp HOME");
    assert_eq!(db.readers(), Some(k2_core::db::READERS), "the main database has its read pool");
    // The ticker sleeps: only this test's frames move jobs.
    k2_core::compute::retry::set_tuning_for_tests(k2_core::compute::retry::Tuning { tick_millis: 3_600_000, ..Default::default() });

    let d = test_harness::start(OWNER).await;
    let port = d.port;
    let ws = seed_ws(&env, "lockload");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE projects SET agents_can_use_compute = 1 WHERE id = ?1", params![ws.id]).expect("compute on");
        // A pinned Chat, so the workspace has a Thread to read.
        let conv = uuid::Uuid::new_v4().to_string();
        k2_core::db::schema::WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &ws.id, None, Some(&conv), "claude", "system", "running").expect("pinned chat");
    }
    active_node(NODE_A, "lockload-a-vin", &ws.id);
    active_node(NODE_B, "lockload-b-vin", &ws.id);
    let _a = compute_ws::test_attach(NODE_A, offer(1, false));

    // One real submit for a real plan; every other job is a copy of it.
    let agent = passport(&ws);
    let body = json!({
        "argv": ["sh", "-c", "cargo test"],
        "noSource": true,
        "workspace": ws.id,
        "memBytes": 2 * GB,
        "node": "lockload-a-vin",
        "clientJobId": new_id(),
    });
    let r = post(port, "/cli/compute/run", &agent, body).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let template = r.json()["job"]["id"].as_str().unwrap_or_else(|| panic!("no job id: {}", r.body)).to_string();

    let mut rows: Vec<(String, &str, &str)> = Vec::new();
    for i in 0..HISTORY {
        rows.push((new_id(), NODE_A, if i % 7 == 0 { "failed" } else { "done" }));
    }
    let running: Vec<String> = (0..RUNNING).map(|_| new_id()).collect();
    let churn: Vec<String> = (0..CHURN).map(|_| new_id()).collect();
    let queued: Vec<String> = (0..QUEUED).map(|_| new_id()).collect();
    rows.extend(running.iter().map(|id| (id.clone(), NODE_A, "running")));
    rows.extend(churn.iter().map(|id| (id.clone(), NODE_A, "running")));
    rows.extend(queued.iter().map(|id| (id.clone(), NODE_B, "queued")));
    let t0 = Instant::now();
    clone_jobs(&template, &rows);
    eprintln!("LOAD: seeded {} jobs in {}", rows.len(), ms(t0.elapsed()));
    let _b = compute_ws::test_attach(NODE_B, offer(1, true));

    // The UI GETs an agent switch makes; each answers 200 before the load.
    let tok = enc(OWNER);
    let path = enc(&ws.path.to_string_lossy());
    let ui: Vec<(&str, String)> = vec![
        ("thread", format!("/cli/thread?token={tok}&addr={}&limit=25", ws.handle)),
        ("thread/activity", format!("/cli/thread/activity?token={tok}&addr={}", ws.handle)),
        ("sessions/list-for-workspace", format!("/cli/sessions/list-for-workspace?token={tok}&path={path}")),
        ("agent-display-name", format!("/cli/workspace/agent-display-name?token={tok}&project={path}")),
    ];
    for (name, pq) in &ui {
        let pq = pq.clone();
        let r = tokio::task::spawn_blocking(move || req_blocking(port, "GET", &pq, None)).await.unwrap();
        assert_eq!(r.status, 200, "{name}: {}", r.body);
    }
    // The compute GETs work, by full id and by the short id.
    for id in [&running[0], &queued[0]] {
        for token in [id.clone(), id[..8].to_string()] {
            let pq = format!("/cli/compute/jobs/get?token={tok}&job={token}");
            let r = tokio::task::spawn_blocking(move || req_blocking(port, "GET", &pq, None)).await.unwrap();
            assert_eq!(r.status, 200, "get {token}: {}", r.body);
        }
    }

    let stop = Arc::new(AtomicBool::new(false));
    let served = Arc::new(AtomicU64::new(0));
    let mut clients = Vec::new();
    // 50 `get?wait=25`: 30 on running jobs, 20 on queued ones; two in three
    // by the short id.
    let waited: Vec<String> = running.iter().take(30).chain(queued.iter()).cloned().collect();
    for (i, id) in waited.into_iter().enumerate() {
        let (stop, served, tok) = (stop.clone(), served.clone(), tok.clone());
        clients.push(std::thread::spawn(move || {
            let token = if i % 3 == 0 { id.clone() } else { id[..8].to_string() };
            while !stop.load(Ordering::Relaxed) {
                let r = req_blocking(port, "GET", &format!("/cli/compute/jobs/get?token={tok}&job={token}&wait=25"), None);
                assert_eq!(r.status, 200, "get {token}: {}", r.body);
                served.fetch_add(1, Ordering::Relaxed);
                let st = r.json()["state"].as_str().map(str::to_string).or_else(|| r.json()["job"]["state"].as_str().map(str::to_string));
                if matches!(st.as_deref(), Some("done" | "failed" | "cancelled" | "timeout" | "interrupted" | "unknown")) {
                    break;
                }
            }
        }));
    }
    // 20 log followers.
    for id in running.iter().skip(20).take(20).cloned() {
        let (stop, served, tok) = (stop.clone(), served.clone(), tok.clone());
        clients.push(std::thread::spawn(move || {
            let short = id[..8].to_string();
            let mut cursor = 0u64;
            while !stop.load(Ordering::Relaxed) {
                let r = req_blocking(port, "GET", &format!("/cli/compute/jobs/logs?token={tok}&job={short}&wait=25&cursor={cursor}"), None);
                assert_eq!(r.status, 200, "logs {short}: {}", r.body);
                served.fetch_add(1, Ordering::Relaxed);
                let v = r.json();
                cursor = v["cursor"].as_u64().unwrap_or_else(|| panic!("no cursor: {}", r.body));
                if v["end"].as_bool() == Some(true) {
                    break;
                }
            }
        }));
    }
    // The nodes: ~200 log chunks a second, a state frame every 50 ms, an
    // offer from each every 250 ms, a job end every 300 ms.
    let nodes = {
        let (stop, running, churn) = (stop.clone(), running.clone(), churn.clone());
        std::thread::spawn(move || {
            let mut seq = vec![0u64; running.len()];
            let mut tick = 0u64;
            let mut ended = 0usize;
            let mut rev = 1u64;
            while !stop.load(Ordering::Relaxed) {
                tick += 1;
                let i = (tick as usize * 7) % running.len();
                seq[i] += 1;
                compute_ws::test_frame(NODE_A, log(&running[i], seq[i]));
                if tick.is_multiple_of(10) {
                    compute_ws::test_frame(NODE_A, state(&running[(tick as usize / 10) % running.len()], JobState::Running));
                }
                if tick.is_multiple_of(50) {
                    rev += 1;
                    compute_ws::test_frame(NODE_A, Frame::Offer(offer(rev, false)));
                    compute_ws::test_frame(NODE_B, Frame::Offer(offer(rev, true)));
                }
                if tick.is_multiple_of(60) && ended < churn.len() {
                    compute_ws::test_frame(NODE_A, state(&churn[ended], JobState::Done));
                    ended += 1;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            ended
        })
    };

    // Let the long-polls settle in, then measure.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let before = k2_core::db::lock::wait_stats();
    let served_before = served.load(Ordering::Relaxed);
    let samples = tokio::task::spawn_blocking({
        let ui = ui.clone();
        move || {
            let mut out: Vec<(String, Duration)> = Vec::new();
            let end = Instant::now() + LOAD_FOR;
            while Instant::now() < end {
                for (name, pq) in &ui {
                    let t = Instant::now();
                    let r = req_blocking(port, "GET", pq, None);
                    let took = t.elapsed();
                    assert_eq!(r.status, 200, "{name} under load: {}", r.body);
                    out.push((name.to_string(), took));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            out
        }
    })
    .await
    .unwrap();
    let after = k2_core::db::lock::wait_stats();
    let served_during = served.load(Ordering::Relaxed) - served_before;

    // Wind down: the nodes stop, every job ends, the long-polls return.
    stop.store(true, Ordering::Relaxed);
    let ended = nodes.join().expect("node thread");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE compute_jobs SET state = 'cancelled', ended_at = ?1 WHERE state IN ('running', 'queued', 'assigned')", params![k2_core::compute::now()])
            .expect("end every job");
    }
    compute_ws::notify_change();
    for c in clients {
        c.join().expect("a long-poll client");
    }

    let mut all: Vec<Duration> = samples.iter().map(|(_, d)| *d).collect();
    all.sort();
    for (name, _) in &ui {
        let mut mine: Vec<Duration> = samples.iter().filter(|(n, _)| n.as_str() == *name).map(|(_, d)| *d).collect();
        mine.sort();
        eprintln!(
            "LOAD: {name:<28} n={:<4} p50={} p95={} p99={} max={}",
            mine.len(),
            ms(pct(&mine, 0.5)),
            ms(pct(&mine, 0.95)),
            ms(pct(&mine, 0.99)),
            ms(*mine.last().unwrap_or(&Duration::ZERO))
        );
    }
    let p95 = pct(&all, 0.95);
    let waits = after.waits - before.waits;
    let logged = after.logged - before.logged;
    let waited = after.waited.saturating_sub(before.waited);
    eprintln!(
        "LOAD: all UI GETs n={} p50={} p95={} p99={} max={}",
        all.len(),
        ms(pct(&all, 0.5)),
        ms(p95),
        ms(pct(&all, 0.99)),
        ms(*all.last().unwrap_or(&Duration::ZERO))
    );
    eprintln!(
        "LOAD: db lock over {}s: waits>=1ms {waits}, waits>=100ms {logged}, waited {} total ({} avg); compute long-poll answers {served_during}; jobs ended {ended}",
        LOAD_FOR.as_secs(),
        ms(waited),
        ms(if waits > 0 { waited / waits as u32 } else { Duration::ZERO }),
    );
    assert!(p95 < Duration::from_millis(50), "the UI GETs took {} at p95 under the compute load (target < 50 ms)", ms(p95));
}
