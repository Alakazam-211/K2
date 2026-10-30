//! Job caches (`k2 compute run --cache NAME`, protocol 6) against a
//! loopback fake controller: a `--dev` node in temp dirs, no real users.
// pending k2-node Windows port: k2-node (the compute node runtime) is Unix-only
// until its Windows port (Phase 3: Job Objects instead of uids and cgroups).
#![cfg(unix)]

mod common;

use std::time::Duration;

use common::*;
use k2_node_proto::cache::{self as pcache, CacheGet};
use k2_node_proto::frames::{self, Frame, JobState, LogStream};

/// A job that counts its runs in `$K2_JOB_CACHE/runs` and prints the count.
fn counter(job: &str, cache: &str) -> frames::Plan {
    let mut p = plan(job, &["/bin/sh", "-c", "n=$(cat \"$K2_JOB_CACHE/runs\" 2>/dev/null || echo 0); n=$((n+1)); echo $n > \"$K2_JOB_CACHE/runs\"; echo runs=$n"]);
    p.cache = Some(cache.into());
    p
}

/// Runs the job to its receipt, then waits until the node has given its
/// cache back. The node reports the end first and gives the cache back
/// last (it measures it once the job's unit is gone, `run.rs`), so the
/// next job, or a test that edits the cache's meta, would otherwise race
/// that hand-back (seen on a Mac mini: the next run found the cache still
/// held, and the late meta write undid the test's edit).
async fn run(c: &mut Conn, p: frames::Plan) -> (Vec<Frame>, String, String) {
    let job = p.job_id.clone();
    let cache = p.cache.clone().map(|name| (p.workspace_id.clone(), name));
    c.send(Frame::Assign(assign(p, 1)));
    let fr = run_to_receipt(c, &job).await;
    if let Some((ws, name)) = cache {
        wait_cache_given_back(c, &job, &ws, &name).await;
    }
    let logs = logs_of(&fr, &job);
    (fr.clone(), text(&logs, LogStream::Out), text(&logs, LogStream::Sys))
}

async fn wait_cache_given_back(c: &mut Conn, job: &str, ws: &str, name: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    for i in 0u32.. {
        let v = view(c, CacheGet { request_id: format!("idle-{job}-{i}"), workspace_id: Some(ws.into()), clear: false, name: Some(name.into()) }).await;
        if v.caches.iter().all(|e| !e.busy) {
            return;
        }
        assert!(tokio::time::Instant::now() < deadline, "job {job}'s cache {name} is still held 20 s after its receipt: {v:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn view(c: &mut Conn, ask: CacheGet) -> pcache::CacheView {
    let id = ask.request_id.clone();
    c.send(Frame::CacheGet(ask));
    let fr = tokio::time::timeout(Duration::from_secs(20), c.until(|f| matches!(f, Frame::CacheView(v) if v.request_id == id))).await.expect("a cache_view");
    match fr.last() {
        Some(Frame::CacheView(v)) => v.clone(),
        _ => unreachable!(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cache_outlives_its_job_per_workspace_and_name() {
    let mut fake = Fake::start().await;
    let n = start_node(&fake, "e2e-cache", "", |_| {}).await;
    let mut c = fake.next_conn().await;
    let offer = c.until(|f| matches!(f, Frame::Offer(_))).await;
    let Some(Frame::Offer(o)) = offer.last() else { unreachable!() };
    assert!(o.features.iter().any(|f| f == pcache::FEATURE_JOB_CACHE), "a protocol-6 server is offered job caches: {:?}", o.features);

    let ws_key = pcache::ws_key(&fake.key.fingerprint(), "ws-1");
    let dir = n.layout.home.join("cache/job-caches").join(&ws_key).join("fuzz");
    let (fr, out, sys) = run(&mut c, counter("c1", "fuzz")).await;
    assert_eq!(states_of(&fr, "c1").last().unwrap().state, JobState::Done, "{sys}");
    assert_eq!(out, "runs=1\n");
    assert!(sys.contains("cache fuzz: new (empty)"), "{sys}");
    assert!(sys.contains(&format!("K2_JOB_CACHE={}", dir.display())), "{sys}");
    let Some(Frame::Receipt(r)) = fr.last() else { unreachable!() };
    let receipt = r.receipt.verify(n.node.key.spki_der()).unwrap();
    assert!(receipt.env_names.iter().any(|e| e == "K2_JOB_CACHE"), "{:?}", receipt.env_names);

    // The next run of the workspace finds it; the job's own dir is gone.
    let (_, out, sys) = run(&mut c, counter("c2", "fuzz")).await;
    assert_eq!(out, "runs=2\n", "{sys}");
    assert!(sys.contains("cache fuzz: kept from an earlier run"), "{sys}");
    assert_eq!(std::fs::read_to_string(dir.join("runs")).unwrap(), "2\n");

    // Another name, and another workspace's same name: separate folders.
    let (_, out, _) = run(&mut c, counter("c3", "corpus")).await;
    assert_eq!(out, "runs=1\n");
    let mut other = counter("c4", "fuzz");
    other.workspace_id = "ws-2".into();
    let (_, out, _) = run(&mut c, other).await;
    assert_eq!(out, "runs=1\n", "workspace ws-2's fuzz isn't ws-1's");

    // A job without --cache gets no K2_JOB_CACHE.
    let (_, out, _) = run(&mut c, plan("c5", &["/bin/sh", "-c", "echo [${K2_JOB_CACHE:-none}]"])).await;
    assert_eq!(out, "[none]\n");

    // `cache_get`: this server's caches, its workspace keys.
    let v = view(&mut c, CacheGet { request_id: "r1".into(), workspace_id: None, clear: false, name: None }).await;
    let mut got: Vec<(String, String)> = v.caches.iter().map(|e| (e.ws_key.clone(), e.name.clone())).collect();
    got.sort();
    let mut want = vec![(ws_key.clone(), "corpus".to_string()), (ws_key.clone(), "fuzz".to_string()), (pcache::ws_key(&fake.key.fingerprint(), "ws-2"), "fuzz".to_string())];
    want.sort();
    assert_eq!(got, want);
    assert!(v.caches.iter().all(|e| e.bytes > 0 && e.used_at > 0 && !e.busy), "{v:?}");
    let one = view(&mut c, CacheGet { request_id: "r2".into(), workspace_id: Some("ws-1".into()), clear: false, name: Some("fuzz".into()) }).await;
    assert_eq!(one.caches.len(), 1);
    // A bad name is refused, never matched as a path.
    let bad = view(&mut c, CacheGet { request_id: "r3".into(), workspace_id: None, clear: true, name: Some("../x".into()) }).await;
    assert_eq!(bad.refused.as_deref(), Some(pcache::codes::CACHE_INVALID));
    assert!(dir.exists());

    // Clear one workspace's `fuzz`: gone, the rest stay; the next run starts over.
    let cl = view(&mut c, CacheGet { request_id: "r4".into(), workspace_id: Some("ws-1".into()), clear: true, name: Some("fuzz".into()) }).await;
    assert_eq!(cl.cleared.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), vec!["fuzz"]);
    assert!(cl.caches.is_empty(), "the view is filtered like the clear: {cl:?}");
    assert!(!dir.exists());
    let left = view(&mut c, CacheGet { request_id: "r5".into(), workspace_id: None, clear: false, name: None }).await;
    assert_eq!(left.caches.len(), 2);
    let (_, out, _) = run(&mut c, counter("c6", "fuzz")).await;
    assert_eq!(out, "runs=1\n");
}

/// The earlier warm-slot hazard: files another uid wrote can't have their
/// times or modes set by the new job user. A cache last used by another
/// uid starts empty, said in the job's log.
#[tokio::test(flavor = "multi_thread")]
async fn a_cache_written_by_another_job_user_starts_empty() {
    let mut fake = Fake::start().await;
    let n = start_node(&fake, "e2e-cache-uid", "", |_| {}).await;
    let mut c = fake.next_conn().await;
    ready(&mut c).await;
    let (_, out, _) = run(&mut c, counter("u1", "fuzz")).await;
    assert_eq!(out, "runs=1\n");
    let ws_dir = n.layout.home.join("cache/job-caches").join(pcache::ws_key(&fake.key.fingerprint(), "ws-1"));
    let meta_path = ws_dir.join(".fuzz.k2meta");
    let mut meta: serde_json::Value = serde_json::from_slice(&std::fs::read(&meta_path).unwrap()).unwrap();
    // SAFETY: geteuid can't fail.
    let me = unsafe { libc::geteuid() };
    assert_eq!(meta["uid"], me);
    meta["uid"] = (me + 4242).into();
    std::fs::write(&meta_path, serde_json::to_vec(&meta).unwrap()).unwrap();
    let (_, out, sys) = run(&mut c, counter("u2", "fuzz")).await;
    assert_eq!(out, "runs=1\n", "started over: {sys}");
    assert!(sys.contains(&format!("written by another job user (uid {}, now {me}); it starts empty", me + 4242)), "{sys}");
    assert!(std::fs::read_dir(&ws_dir).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().starts_with(".evicted-")), "nothing left aside");
}

/// All caches together stay under a quarter of the disk budget: an idle
/// cache over it is evicted once a job using one ends (never the one a
/// job holds while it runs).
#[tokio::test(flavor = "multi_thread")]
async fn caches_over_the_cap_are_evicted_least_recently_used_first() {
    let mut fake = Fake::start().await;
    // A 1 GB budget: a 256 MiB cap.
    let n = start_node(&fake, "e2e-cache-cap", "disk_budget_gb = 1\n", |_| {}).await;
    let mut c = fake.next_conn().await;
    ready(&mut c).await;
    let big = |job: &str, name: &str| {
        let mut p = plan(job, &["/bin/sh", "-c", "dd if=/dev/zero of=\"$K2_JOB_CACHE/blob\" bs=1048576 count=160 2>/dev/null; echo ok"]);
        p.cache = Some(name.into());
        p
    };
    let ws = n.layout.home.join("cache/job-caches").join(pcache::ws_key(&fake.key.fingerprint(), "ws-1"));
    let (_, out, _) = run(&mut c, big("b1", "old")).await;
    assert_eq!(out, "ok\n");
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let (_, out, _) = run(&mut c, big("b2", "new")).await;
    assert_eq!(out, "ok\n");
    // 320 MiB > 256 MiB: the least recently used (`old`) went, `new` stays.
    for _ in 0..100 {
        if !ws.join("old").exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!ws.join("old").exists(), "the older cache is evicted");
    assert!(ws.join("new/blob").exists(), "the newer one stays");
    assert!(!ws.join(".old.k2meta").exists(), "its meta goes with it");
}
