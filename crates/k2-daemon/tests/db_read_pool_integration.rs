//! The main database's read pool and the catch-all off the async workers
//! (speed pass P1), headless: the real dispatcher in process on a temp
//! `$HOME`, the main database opened as the daemon opens it
//! (`k2_core::db::init_database`: a WAL file with its read pool).
//!
//! - A writer that holds the main lock for 2 s no longer holds up the pooled
//!   GET routes; a route still on the main connection does wait (the hold is
//!   real).
//! - `K2_DB_READ_POOL=0` puts the pooled routes back on the main connection:
//!   they wait again.
//! - With twice the workers' worth of catch-all requests parked on a held
//!   lock, an event socket still answers a ping.
//!
//! ISOLATION: the database handle is process-wide and set once, so every
//! test here needs a process of its own. Run with cargo-nextest, or one test
//! per `cargo test … -- --exact <name>` call.

#[path = "support/app_fixture.rs"]
mod app_fixture;

use std::time::{Duration, Instant};

use app_fixture::*;
use futures_util::{SinkExt, StreamExt};
use rusqlite::params;
use tokio_tungstenite::tungstenite::Message;

fn open_file_db() -> std::sync::Arc<k2_core::db::DbLock> {
    let db = k2_core::db::init_database().expect("init_database under the temp HOME");
    let mode: String = db.lock().query_row("PRAGMA journal_mode", [], |r| r.get(0)).expect("journal_mode");
    assert_eq!(mode.to_ascii_lowercase(), "wal", "the main database is the WAL file");
    db
}

fn seed_project(env: &Env, handle: &str) -> (String, String) {
    let id = uuid::Uuid::new_v4().to_string();
    let path = env.path().join(format!("ws-{handle}"));
    std::fs::create_dir_all(&path).expect("workspace dir");
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
        params![id, handle, path.to_string_lossy()],
    )
    .expect("project");
    let ws_id = uuid::Uuid::new_v4().to_string();
    k2_core::db::schema::Workspace::create(&conn, &ws_id, &id, None, "branch", Some("main"), "main", 0, None).expect("workspace");
    conn.execute(
        "INSERT INTO workspace_layouts (id, project_id, workspace_id, layout_json, updated_at) VALUES (?1, ?2, ?3, '{}', unixepoch())",
        params![format!("{id}:{ws_id}"), id, ws_id],
    )
    .expect("layout");
    (id, path.to_string_lossy().into_owned())
}

/// Hold the main lock on its own thread for `hold`, inside an open write
/// (as a slow writer does). Returns once the lock is held.
fn hold_main_lock(hold: Duration) -> std::thread::JoinHandle<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    let t = std::thread::spawn(move || {
        let db = k2_core::db::shared();
        let g = db.lock();
        g.execute_batch("BEGIN IMMEDIATE; UPDATE projects SET name = name;").expect("open write");
        tx.send(()).expect("held");
        std::thread::sleep(hold);
        g.execute_batch("COMMIT").expect("commit");
    });
    rx.recv().expect("lock held");
    t
}

fn timed_get(port: u16, pq: &str) -> (u16, Duration) {
    let t0 = Instant::now();
    let r = req_blocking(port, "GET", pq, None);
    (r.status, t0.elapsed())
}

const POOLED: &[&str] = &[
    "/cli/projects/list",
    "/cli/workspace-layouts/load-all",
    "/cli/projects/active",
    "/cli/thread/unread",
];

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_slow_writer_does_not_hold_up_the_pooled_routes() {
    let env = setup(&[]);
    let db = open_file_db();
    assert_eq!(db.readers(), Some(k2_core::db::READERS), "the daemon's database has its read pool");
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let (pid, _) = seed_project(&env, "poolone");
    seed_project(&env, "pooltwo");
    let tok = enc(OWNER);
    // Warm every route once (the owner is seen, caches fill).
    for r in POOLED {
        let (s, _) = tokio::task::spawn_blocking({
            let pq = format!("{r}?token={tok}");
            move || timed_get(port, &pq)
        })
        .await
        .unwrap();
        assert_eq!(s, 200, "{r}");
    }

    let holder = hold_main_lock(Duration::from_millis(2000));
    let results = tokio::task::spawn_blocking({
        let tok = tok.clone();
        let pid = pid.clone();
        move || {
            let mut out = Vec::new();
            for r in POOLED {
                out.push((r.to_string(), timed_get(port, &format!("{r}?token={tok}"))));
            }
            // A route still on the main connection waits for the writer.
            out.push(("tab-titles (main lock)".into(), timed_get(port, &format!("/cli/workspace/tab-titles?token={tok}&project_id={pid}"))));
            out
        }
    })
    .await
    .unwrap();
    holder.join().expect("holder");

    let (last_name, (last_status, last_took)) = results.last().cloned().expect("results");
    assert_eq!(last_status, 200, "{last_name}");
    assert!(last_took >= Duration::from_millis(500), "{last_name} answered in {last_took:?}: the lock was not really held");
    for (name, (status, took)) in &results[..results.len() - 1] {
        assert_eq!(*status, 200, "{name}");
        assert!(*took < Duration::from_millis(200), "{name} waited {took:?} behind the writer: {results:?}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_the_pool_switched_off_the_routes_wait_for_the_writer() {
    let env = setup(&[("K2_DB_READ_POOL", "0")]);
    let db = open_file_db();
    assert_eq!(db.readers(), None, "K2_DB_READ_POOL=0: no read pool");
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    seed_project(&env, "nopool");
    let tok = enc(OWNER);
    let pq = format!("/cli/projects/list?token={tok}");
    let (s, _) = tokio::task::spawn_blocking({
        let pq = pq.clone();
        move || timed_get(port, &pq)
    })
    .await
    .unwrap();
    assert_eq!(s, 200);
    let holder = hold_main_lock(Duration::from_millis(1500));
    let (s, took) = tokio::task::spawn_blocking(move || timed_get(port, &pq)).await.unwrap();
    holder.join().expect("holder");
    assert_eq!(s, 200);
    assert!(took >= Duration::from_millis(1000), "with no pool the list waits for the writer; it took {took:?}");
}

const WORKERS: usize = 2;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parked_catch_all_requests_do_not_stall_an_event_socket() {
    let env = setup(&[]);
    open_file_db();
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let (pid, path) = seed_project(&env, "pong");
    let tok = enc(OWNER);

    let url = format!("ws://127.0.0.1:{port}/cli/sessions/events?path={}&token={tok}", enc(&path));
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.expect("events socket");
    // The first frame (hello) proves the socket is served.
    let first = tokio::time::timeout(Duration::from_secs(5), ws.next()).await.expect("hello in time");
    assert!(matches!(first, Some(Ok(_))), "hello: {first:?}");

    let holder = hold_main_lock(Duration::from_millis(3000));
    // Twice the workers' worth of catch-all requests that need the lock.
    let parked: Vec<_> = (0..WORKERS * 2)
        .map(|_| {
            let pq = format!("/cli/workspace/tab-titles?token={tok}&project_id={pid}");
            std::thread::spawn(move || timed_get(port, &pq))
        })
        .collect();
    tokio::time::sleep(Duration::from_millis(400)).await;

    let t0 = Instant::now();
    ws.send(Message::Ping(b"speed-pass".to_vec().into())).await.expect("send ping");
    let pong = tokio::time::timeout(Duration::from_millis(500), async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Pong(p))) => return Some(p),
                Some(Ok(_)) => continue,
                _ => return None,
            }
        }
    })
    .await;
    let took = t0.elapsed();
    holder.join().expect("holder");
    for p in parked {
        let (s, _) = p.join().expect("parked request");
        assert_eq!(s, 200, "a parked request finished");
    }
    let pong = pong.unwrap_or_else(|_| panic!("no pong within 500 ms while {} catch-all requests waited", WORKERS * 2));
    assert_eq!(pong.map(|p| p.to_vec()), Some(b"speed-pass".to_vec()), "the pong echoes the ping");
    assert!(took < Duration::from_millis(500), "pong after {took:?}");
}
