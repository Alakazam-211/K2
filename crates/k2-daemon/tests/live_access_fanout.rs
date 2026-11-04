//! Big fan-out, for the night run (`prd-live-access-changes-v1` S5, §14 tests 20-23).
//! Reports numbers; gates nothing by day, so every test here is `#[ignore]`d:
//!
//! ```text
//! ulimit -n 40000; K2_FANOUT_N=5000 cargo test -p k2-daemon --release \
//!   --test live_access_fanout -- --ignored --nocapture --test-threads=1
//! ```
//!
//! Each `[fanout]` line is a result. An assert is only what must hold at any size:
//! nobody who wasn't changed hears anything, and everyone who was hears.

#[path = "support/app_fixture.rs"]
mod app_fixture;
#[path = "support/live_sockets.rs"]
mod live_sockets;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use app_fixture::*;
use futures_util::StreamExt;
use k2_core::skin;
use live_sockets::*;
use serde_json::json;
use tokio_tungstenite::tungstenite::Message;

fn n() -> usize {
    std::env::var("K2_FANOUT_N").ok().and_then(|v| v.parse().ok()).unwrap_or(5_000)
}

fn rss_mb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("VmRSS:")).and_then(|l| l.split_whitespace().nth(1)).and_then(|k| k.parse::<u64>().ok()))
        .map_or(0, |kb| kb / 1024)
}

/// `count` people on one role in `ws`, each with one session.
fn people(ws: &Ws, role: &str, prefix: &str, count: usize) -> Vec<String> {
    let mut policy = skin::RoomPolicy::new();
    policy.insert(ws.id.clone(), vec!["thread:read".to_string(), "files:read".to_string()]);
    skin::create_role_for_app(role, &policy, None).expect("role");
    let t0 = Instant::now();
    let passes: Vec<String> = (0..count)
        .map(|i| {
            let user = format!("{prefix}{i}");
            skin::add_principal(&user).expect("person");
            skin::assign_role(&user, role).expect("assign");
            skin::create_session_token(&user).expect("session").1
        })
        .collect();
    eprintln!("[fanout] setup: {count} people in {:?}", t0.elapsed());
    passes
}

/// Opens a socket per pass; a task per socket counts its text frames and when each
/// arrived, and whether it ended.
struct Listeners {
    frames: Arc<AtomicU64>,
    closes: Arc<AtomicU64>,
    arrivals: Arc<std::sync::Mutex<Vec<Duration>>>,
    close_times: Arc<std::sync::Mutex<Vec<(Duration, u16)>>>,
    t0: Arc<std::sync::Mutex<Instant>>,
}

async fn listen(port: u16, paths: Vec<String>) -> Listeners {
    let l = Listeners {
        frames: Arc::new(AtomicU64::new(0)),
        closes: Arc::new(AtomicU64::new(0)),
        arrivals: Arc::new(std::sync::Mutex::new(Vec::new())),
        close_times: Arc::new(std::sync::Mutex::new(Vec::new())),
        t0: Arc::new(std::sync::Mutex::new(Instant::now())),
    };
    let t0 = Instant::now();
    let total = paths.len();
    // Not all at once: this is a test of the fan-out, not of the accept queue.
    for chunk in paths.chunks(200) {
        let mut opening = Vec::new();
        for pq in chunk {
            let url = format!("ws://127.0.0.1:{port}{pq}");
            opening.push(tokio::spawn(async move { tokio_tungstenite::connect_async(url).await.expect("open").0 }));
        }
        for o in opening {
            let mut s = o.await.expect("join");
            let (f, c, a, ct, t) = (l.frames.clone(), l.closes.clone(), l.arrivals.clone(), l.close_times.clone(), l.t0.clone());
            tokio::spawn(async move {
                while let Some(m) = s.next().await {
                    match m {
                        Ok(Message::Text(_)) => {
                            f.fetch_add(1, Ordering::SeqCst);
                            let since = t.lock().expect("t0").elapsed();
                            a.lock().expect("arrivals").push(since);
                        }
                        Ok(Message::Close(frame)) => {
                            c.fetch_add(1, Ordering::SeqCst);
                            let since = t.lock().expect("t0").elapsed();
                            ct.lock().expect("closes").push((since, frame.map_or(0, |x| u16::from(x.code))));
                            break;
                        }
                        Ok(_) => {}
                        Err(_) => break,
                    }
                }
            });
        }
    }
    eprintln!("[fanout] opened {total} sockets in {:?}; rss {} MB", t0.elapsed(), rss_mb());
    l
}

/// Resolves in a loop while `stop` is false: what another request waits for.
fn other_requests(pass: String, stop: Arc<AtomicBool>) -> std::thread::JoinHandle<Vec<Duration>> {
    std::thread::spawn(move || {
        let mut out = Vec::new();
        while !stop.load(Ordering::SeqCst) {
            let t = Instant::now();
            assert!(skin::resolve_skin_token(&pass).is_some(), "the bystander stays live");
            out.push(t.elapsed());
            std::thread::sleep(Duration::from_millis(5));
        }
        out
    })
}

fn pct(mut v: Vec<Duration>, p: f64) -> Duration {
    if v.is_empty() {
        return Duration::ZERO;
    }
    v.sort();
    v[((v.len() - 1) as f64 * p) as usize]
}

/// Test 20: one role edit reaches everyone once, paced; room sockets close first.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "the night run"]
async fn one_role_edit_reaches_everyone() {
    let count = n();
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let ws = seed_ws(&env, "fanout");
    let passes = people(&ws, "fanrole", "fanuser", count);
    let room_sockets = (count / 10).min(500);
    let me_paths: Vec<String> = passes.iter().map(|p| format!("/cli/skin/me/events?scope=me&token={}", enc(p))).collect();
    let room_paths: Vec<String> = passes.iter().take(room_sockets).map(|p| format!("/cli/fs/events?workspace={}&token={}", ws.handle, enc(p))).collect();
    let me = listen(port, me_paths).await;
    let rooms = listen(port, room_paths).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    eprintln!("[fanout] hub holds {} passes; rss {} MB", k2_daemon::access_live::open_passes(), rss_mb());
    let stop = Arc::new(AtomicBool::new(false));
    let bystander = other_requests(passes[0].clone(), stop.clone());

    // Keep the caps, drop files:read: fs sockets lose their cap; everyone's answer moves.
    let mut policy = skin::RoomPolicy::new();
    policy.insert(ws.id.clone(), vec!["thread:read".to_string()]);
    let start = Instant::now();
    *me.t0.lock().expect("t0") = start;
    *rooms.t0.lock().expect("t0") = start;
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": "fanrole", "handle": ws.handle, "caps": ["thread:read"] })).await;
    eprintln!("[fanout] the role edit's own request took {:?}", start.elapsed());

    let deadline = start + Duration::from_secs(120);
    while me.frames.load(Ordering::SeqCst) < count as u64 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let everyone_in = start.elapsed();
    tokio::time::sleep(Duration::from_secs(3)).await;
    stop.store(true, Ordering::SeqCst);
    let waits = bystander.join().expect("bystander");

    let frames = me.frames.load(Ordering::SeqCst);
    let mut arrivals = me.arrivals.lock().expect("arrivals").clone();
    arrivals.sort();
    let by_sec = |secs: u64| arrivals.iter().filter(|a| a.as_secs() == secs).count();
    let worst_second = (0..=arrivals.last().map_or(0, |a| a.as_secs())).map(by_sec).max().unwrap_or(0);
    let closes = rooms.close_times.lock().expect("closes").clone();
    let last_close = closes.iter().map(|(t, _)| *t).max().unwrap_or_default();
    let codes_ok = closes.iter().filter(|(_, c)| *c == 4013).count();
    eprintln!("[fanout] test20 n={count}: {frames} frames, all in within {everyone_in:?} (target 30 s); first {:?}, last {:?}; worst second {worst_second} frames (limit 200 + a tick)", arrivals.first(), arrivals.last());
    eprintln!("[fanout] test20 room sockets: {} of {room_sockets} closed 4013, last at {last_close:?} (target 1 s); the first frame came at {:?}", codes_ok, arrivals.first());
    eprintln!("[fanout] test20 another request's resolve during it: p50 {:?}, p99 {:?}, max {:?} ({} samples); rss {} MB", pct(waits.clone(), 0.5), pct(waits.clone(), 0.99), pct(waits.clone(), 1.0), waits.len(), rss_mb());
    eprintln!("[fanout] test20 walks so far: {:?}", k2_daemon::access_live::walk_stats());
    assert_eq!(frames, count as u64, "exactly one frame each");
    assert_eq!(codes_ok, room_sockets, "every room socket closed 4013");
}

/// Test 21: an edit that changes nobody's answer: no frames, and what the walk cost.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "the night run"]
async fn an_edit_of_someone_else_sends_nothing() {
    let count = n();
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let ws = seed_ws(&env, "fanother");
    let passes = people(&ws, "fanotherrole", "fanother", count);
    let me = listen(port, passes.iter().map(|p| format!("/cli/skin/me/events?scope=me&token={}", enc(p))).collect()).await;
    let _lonely = people(&ws, "lonelyrole", "lonely", 1);
    tokio::time::sleep(Duration::from_secs(2)).await;
    let before = k2_daemon::access_live::walk_stats();
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": "lonelyrole", "handle": ws.handle, "caps": ["thread:read"] })).await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    let after = k2_daemon::access_live::walk_stats();
    let frames = me.frames.load(Ordering::SeqCst);
    eprintln!("[fanout] test21 n={count}: {frames} frames; walks {before:?} -> {after:?}");
    assert_eq!(frames, 0, "nobody's answer changed");
}

/// Test 22: a write loop: memory stays flat, walks at most 4 a second.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "the night run"]
async fn a_write_loop_leaves_memory_flat() {
    let count = n().min(1_000);
    let secs: u64 = std::env::var("K2_FANOUT_LOOP_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(600);
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let ws = seed_ws(&env, "fanloop");
    let passes = people(&ws, "fanlooprole", "fanloop", count);
    let me = listen(port, passes.iter().map(|p| format!("/cli/skin/me/events?scope=me&token={}", enc(p))).collect()).await;
    let bystanders = people(&ws, "fanloopidle", "fanloopidle", 1);
    tokio::time::sleep(Duration::from_secs(2)).await;
    let start_rss = rss_mb();
    let before = k2_daemon::access_live::walk_stats();
    let t0 = Instant::now();
    let mut writes = 0u64;
    while t0.elapsed() < Duration::from_secs(secs) {
        // A write nobody's answer rests on, 100 a second.
        skin::set_principal_full_name("fanloopidle0", Some(&format!("n{writes}"))).expect("write");
        writes += 1;
        tokio::time::sleep(Duration::from_millis(10)).await;
        if writes % 6000 == 0 {
            eprintln!("[fanout] test22 {:?}: {writes} writes, rss {} MB", t0.elapsed(), rss_mb());
        }
    }
    let after = k2_daemon::access_live::walk_stats();
    let walks = after.walks - before.walks;
    let rate = walks as f64 / t0.elapsed().as_secs_f64();
    eprintln!("[fanout] test22 n={count} {secs}s: {writes} writes, {walks} walks ({rate:.2}/s, target ≤ 4), rss {start_rss} -> {} MB, frames {}", rss_mb(), me.frames.load(Ordering::SeqCst));
    assert_eq!(me.frames.load(Ordering::SeqCst), 0, "nobody's access changed");
    assert!(rate <= 4.0, "walks at most 4 a second: {rate}");
    let _ = bystanders;
}

/// Test 23: on an App's link at default quotas, every guest re-reading on its frame
/// stays under the App's rate: no rate_limited.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "the night run"]
async fn an_apps_link_stays_under_its_rate() {
    use k2_daemon::apps::gateway::{self, LinkSpec};
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let ws = seed_ws(&env, "fanlink");
    let svc = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let svc = k2_core::published_services::insert(&conn, &ws.id, "fanlink", k2_core::published_services::CMD_APP_SENTINEL, &ws.path.to_string_lossy(), 0, "local", "stopped", "cmd", "").expect("service").id;
        k2_core::deploy::store::insert_app(
            &conn,
            &k2_core::deploy::store::NewApp { app_id: "fanlink", service_id: &svc, seed_id: None, owner_workspace: &ws.id, isolation: "process", memory_limit_bytes: None, front_port: 0, created_by: "owner" },
        )
        .expect("insert app");
        svc
    };
    let q = k2_daemon::apps::quotas::quotas_for("fanlink");
    // Leave room for the me.get calls on the same link.
    let guests = (q.sockets as usize).saturating_sub(4);
    let mut policy = skin::RoomPolicy::new();
    policy.insert(ws.id.clone(), vec!["thread:read".to_string()]);
    skin::create_role_for_app("fanlinkrole", &policy, None).expect("role");
    let passes: Vec<String> = (0..guests)
        .map(|i| {
            let user = format!("fanlink{i}");
            skin::add_principal(&user).expect("person");
            let pid = skin::host_login_detail(&user).expect("guest").principal_id;
            skin::create_grant(&skin::SkinGrantWrite { subject_kind: "principal".into(), subject_id: pid, kind: "app".into(), target_id: svc.clone(), role: Some("fanlinkrole".into()), scope: None, created_by: Some("owner".into()) }).expect("grant");
            skin::create_app_session_token(&user, &svc).expect("session").raw
        })
        .collect();
    let spec = LinkSpec { app_id: "fanlink".into(), service_id: svc.clone(), workspace_id: ws.id.clone(), port: 0, caps: vec!["thread:read".into()], rooms: vec![ws.id.clone()] };
    gateway::open(spec, None).expect("link");
    let link = gateway::port_of("fanlink").expect("listening");

    let limited = Arc::new(AtomicU64::new(0));
    let reads = Arc::new(AtomicU64::new(0));
    let frames = Arc::new(AtomicU64::new(0));
    let t0 = Arc::new(std::sync::Mutex::new(Instant::now()));
    for pass in passes {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut req = format!("ws://127.0.0.1:{link}/cli/skin/me/events?scope=me").into_client_request().expect("request");
        req.headers_mut().insert("Authorization", format!("Bearer {pass}").parse().expect("header"));
        let (mut s, _) = tokio_tungstenite::connect_async(req).await.expect("open on the link");
        let (l, r, f) = (limited.clone(), reads.clone(), frames.clone());
        tokio::spawn(async move {
            while let Some(Ok(m)) = s.next().await {
                if let Message::Text(_) = m {
                    f.fetch_add(1, Ordering::SeqCst);
                    let got = get(link, "/cli/skin/me", &pass, "x=1").await;
                    r.fetch_add(1, Ordering::SeqCst);
                    if got.status == 429 {
                        l.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }
        });
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
    *t0.lock().expect("t0") = Instant::now();
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": "fanlinkrole", "handle": ws.handle, "caps": ["thread:read", "files:read"] })).await;
    let start = Instant::now();
    while frames.load(Ordering::SeqCst) < guests as u64 && start.elapsed() < Duration::from_secs(60) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let all_in = start.elapsed();
    tokio::time::sleep(Duration::from_secs(3)).await;
    eprintln!(
        "[fanout] test23 {guests} guests on a link (rps {} sockets {}): {} frames within {all_in:?}, {} re-reads, {} rate_limited",
        q.rps,
        q.sockets,
        frames.load(Ordering::SeqCst),
        reads.load(Ordering::SeqCst),
        limited.load(Ordering::SeqCst)
    );
    assert_eq!(frames.load(Ordering::SeqCst), guests as u64, "everyone hears");
    assert_eq!(limited.load(Ordering::SeqCst), 0, "no rate_limited");
    gateway::close("fanlink", None, true);
}

/// scope=guests: one socket, every guest of one App, a role edit naming them all.
/// Paced at half the App's request rate, so this reports how long a big change takes
/// through a link at default quotas.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "the night run"]
async fn one_socket_for_every_guest_of_an_app() {
    use k2_daemon::apps::gateway::{self, LinkSpec};
    let count: usize = std::env::var("K2_FANOUT_GUESTS").ok().and_then(|v| v.parse().ok()).unwrap_or(500);
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let ws = seed_ws(&env, "fanfeed");
    let svc = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let svc = k2_core::published_services::insert(&conn, &ws.id, "fanfeed", k2_core::published_services::CMD_APP_SENTINEL, &ws.path.to_string_lossy(), 0, "local", "stopped", "cmd", "").expect("service").id;
        k2_core::deploy::store::insert_app(
            &conn,
            &k2_core::deploy::store::NewApp { app_id: "fanfeed", service_id: &svc, seed_id: None, owner_workspace: &ws.id, isolation: "process", memory_limit_bytes: None, front_port: 0, created_by: "owner" },
        )
        .expect("insert app");
        svc
    };
    let mut policy = skin::RoomPolicy::new();
    policy.insert(ws.id.clone(), vec!["thread:read".to_string()]);
    skin::create_role_for_app("fanfeedrole", &policy, None).expect("role");
    for i in 0..count {
        let user = format!("fanfeed{i}");
        skin::add_principal(&user).expect("person");
        let pid = skin::host_login_detail(&user).expect("guest").principal_id;
        skin::create_grant(&skin::SkinGrantWrite { subject_kind: "principal".into(), subject_id: pid, kind: "app".into(), target_id: svc.clone(), role: Some("fanfeedrole".into()), scope: None, created_by: Some("owner".into()) }).expect("grant");
    }
    let spec = LinkSpec { app_id: "fanfeed".into(), service_id: svc.clone(), workspace_id: ws.id.clone(), port: 0, caps: vec!["thread:read".into()], rooms: vec![ws.id.clone()] };
    let own = gateway::open(spec, None).expect("link");
    let link = gateway::port_of("fanfeed").expect("listening");
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut req = format!("ws://127.0.0.1:{link}/cli/skin/me/events?scope=guests").into_client_request().expect("request");
    req.headers_mut().insert("Authorization", format!("Bearer {own}").parse().expect("header"));
    let t0 = Instant::now();
    let (mut s, _) = tokio_tungstenite::connect_async(req).await.expect("open the feed");
    eprintln!("[fanout] feed n={count}: opened in {:?}", t0.elapsed());
    tokio::time::sleep(Duration::from_secs(1)).await;
    let stop = Arc::new(AtomicBool::new(false));
    let start = Instant::now();
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": "fanfeedrole", "handle": ws.handle, "caps": ["thread:read", "files:read"] })).await;
    let mut seen = std::collections::HashSet::new();
    let deadline = start + Duration::from_secs(count as u64 / 10 + 30);
    while seen.len() < count && Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(5), s.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => {
                let v: serde_json::Value = serde_json::from_str(&t).expect("json");
                seen.insert(v["principalId"].as_str().expect("names a guest").to_string());
            }
            Ok(Some(Ok(_))) | Err(_) => {}
            other => panic!("the feed ended: {other:?}"),
        }
    }
    stop.store(true, Ordering::SeqCst);
    eprintln!("[fanout] feed n={count}: {} guests named within {:?} (rate limit is half the App's rps); walks {:?}", seen.len(), start.elapsed(), k2_daemon::access_live::walk_stats());
    assert_eq!(seen.len(), count, "everyone named");
    gateway::close("fanfeed", None, true);
}
