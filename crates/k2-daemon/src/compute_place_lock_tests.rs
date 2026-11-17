//! Placement's database reads stay cheap however long the job history
//! grows (2026-11-17: a busy controller's `compute_jobs` reached 33k rows
//! and 1.2 GB, and every placement report scanned all of it under the one
//! database lock, for up to 143 s on a loaded Mac, while every other route
//! waited). Kept apart so compute_place.rs stays under its line limit.
//!
//! The measure is SQLite's own work (virtual machine steps, through a
//! progress handler on the connection), not wall time: it is the same on
//! a fast and a slow machine, and a full scan of the history is about a
//! hundred times the bound. Each check also runs once with 0312's indexes
//! dropped and must then go over the bound, so the test proves it would
//! have caught the scan.

use super::*;
use rusqlite::ffi;
use std::ffi::{c_int, c_void};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

/// Jobs in the seeded history.
const HISTORY: i64 = 100_000;
/// Progress handler period, in VM steps.
const STEP: c_int = 100;
/// The most VM steps one placement report's reads may take with the
/// history above (a handful of open jobs, a few nodes). A full scan of
/// the history is over 1,000,000.
const MAX_STEPS: u64 = 60_000;
/// The longest one report may hold the database lock here (generous: an
/// indexed report takes about a millisecond; the 0.46.13 scans took
/// seconds on this history).
const MAX_HOLD: std::time::Duration = std::time::Duration::from_millis(250);

const WS: &str = "ws-me";
const NODES: [&str; 4] = ["n1", "n2", "n3", "n4"];

/// 100k finished jobs with a 1 KB plan each, a `--cache` on every tenth,
/// then a few open ones on every node.
fn seed(conn: &rusqlite::Connection) {
    for n in NODES {
        store::insert_pending_node(conn, n, n, &format!("fp-{n}"), "pem", &BTreeMap::new(), "owner", "123456", 1).expect("node");
        store::confirm_node(conn, n, "owner", 2).expect("confirm");
        store::set_grant(conn, n, WS, &store::GrantLimits::default(), "owner", 3).expect("grant");
    }
    conn.execute_batch(&format!(
        "BEGIN;
         WITH RECURSIVE k(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM k WHERE i < {HISTORY})
         INSERT INTO compute_jobs (id, client_job_id, node_id, workspace_id, requested_by, argv_json, env_names_json,
             request_sha256, plan_json, state, plan_digest, created_at, started_at, ended_at)
         SELECT printf('%032x', i), printf('c%d', i), 'n' || (1 + i % 4), CASE WHEN i % 3 = 0 THEN 'ws-other' ELSE '{WS}' END,
             'agent', '[\"bash\"]', '[]', 'sha', json_object('argv', json_array(hex(zeroblob(512))),
             'cache', CASE WHEN i % 10 = 0 THEN 'warm' END),
             CASE WHEN i % 7 = 0 THEN 'failed' ELSE 'done' END, 'sha256:x', 1000 + i, 1000 + i, 1001 + i
         FROM k;
         COMMIT;"
    ))
    .expect("history");
    // The open jobs: two queued and one running per node.
    for (i, n) in NODES.iter().enumerate() {
        for (k, state) in ["queued", "queued", "running"].iter().enumerate() {
            conn.execute(
                "INSERT INTO compute_jobs (id, client_job_id, node_id, workspace_id, requested_by, argv_json, env_names_json, \
                 request_sha256, plan_json, state, plan_digest, created_at, started_at) \
                 VALUES (?1, ?1, ?2, ?3, 'agent', '[]', '[]', 'sha', '{}', ?4, 'sha256:x', ?5, ?6)",
                rusqlite::params![
                    format!("open-{i}-{k}"),
                    n,
                    WS,
                    state,
                    10_000_000 + (i * 10 + k) as i64,
                    (*state == "running").then_some(10_000_000i64)
                ],
            )
            .expect("open job");
        }
    }
}

unsafe extern "C" fn tick(p: *mut c_void) -> c_int {
    // SAFETY: `p` is the counter `steps` keeps alive while this handler is set.
    let n = unsafe { &*(p as *const AtomicU64) };
    n.fetch_add(STEP as u64, AtomicOrdering::Relaxed);
    0
}

/// VM steps `f` makes SQLite take on `conn` (its progress handler, set
/// through the C API: rusqlite's wants `&mut Connection`, and the lock
/// hands out `&Connection`).
fn steps<T>(conn: &rusqlite::Connection, f: impl FnOnce() -> T) -> (u64, T) {
    let n = AtomicU64::new(0);
    // SAFETY: `conn` is the open scoped connection; `n` outlives the
    // handler, which is cleared below before `n` drops.
    unsafe { ffi::sqlite3_progress_handler(conn.handle(), STEP, Some(tick), &n as *const AtomicU64 as *mut c_void) };
    let out = f();
    // SAFETY: as above; clearing takes no pointer.
    unsafe { ffi::sqlite3_progress_handler(conn.handle(), 0, None, std::ptr::null_mut()) };
    (n.load(AtomicOrdering::Relaxed), out)
}

fn want_cache() -> Want {
    let mut w = tests::want(1);
    w.ws_id = WS.into();
    w.cache = Some("warm".into());
    w
}

const INDEXES: [&str; 3] = ["compute_jobs_state_created", "compute_jobs_created", "compute_jobs_ws_cache"];

#[test]
fn a_placement_report_reads_only_the_open_jobs_however_long_the_history() {
    let _db = k2_core::db::scoped_for_test();
    let db = k2_core::db::shared();
    let conn = db.lock();
    seed(&conn);
    let w = want_cache();
    let now = compute::now();

    // The whole report: its one snapshot read and the per-node work.
    let t0 = std::time::Instant::now();
    let (report_steps, report) = steps(&conn, || candidates_report_raw(&w, &[], now));
    let held = t0.elapsed();
    assert!(report_steps <= MAX_STEPS, "a placement report took {report_steps} VM steps over a {HISTORY}-job history (bound {MAX_STEPS}): it scans the history");
    assert!(held <= MAX_HOLD, "a placement report held the database lock {held:?} (bound {MAX_HOLD:?})");
    // It read the right things: every node seen, the cache's home found
    // (the newest started `warm` job of this workspace: i = 99,980, on n1).
    assert_eq!(report.visible, NODES.len() as u32, "every granted node is seen: {report:?}");
    let snap = PlaceSnapshot::read(&conn, &w);
    assert_eq!(snap.queued.len(), 2 * NODES.len(), "the open queued jobs");
    assert_eq!(snap.active.len(), NODES.len(), "the running jobs");
    assert_eq!(snap.cache_at.as_deref(), Some("n1"), "the cache's home node");

    // The job list, unfiltered and per workspace / node / state.
    let lists: [store::JobFilter<'_>; 4] = [
        store::JobFilter { limit: 30, ..Default::default() },
        store::JobFilter { workspace_id: Some(WS), limit: 30, ..Default::default() },
        store::JobFilter { node_id: Some("n2"), state: Some("queued"), limit: 30, ..Default::default() },
        store::JobFilter { state: Some("running"), limit: 30, ..Default::default() },
    ];
    for f in &lists {
        let (n, rows) = steps(&conn, || store::list_jobs(&conn, f));
        assert!(!rows.is_empty(), "the list finds jobs: {f:?}");
        assert!(n <= MAX_STEPS, "`jobs` {f:?} took {n} VM steps over a {HISTORY}-job history (bound {MAX_STEPS})");
    }
    let (n, newest) = steps(&conn, || store::list_jobs(&conn, &store::JobFilter { limit: 1, ..Default::default() }));
    assert_eq!(newest.first().map(|j| j.id.as_str()), Some("open-3-2"), "newest first");
    assert!(n <= MAX_STEPS, "{n}");

    // The control: without 0312's indexes the same reads scan the
    // history, and the bound catches it.
    for ix in INDEXES {
        conn.execute_batch(&format!("DROP INDEX {ix}")).expect("drop index");
    }
    let (scan_steps, _) = steps(&conn, || PlaceSnapshot::read(&conn, &w));
    assert!(scan_steps > 10 * MAX_STEPS, "without the indexes a report must scan ({scan_steps} VM steps): the bound would not catch a scan");
    let (scan_list, _) = steps(&conn, || store::list_jobs(&conn, &lists[0]));
    assert!(scan_list > MAX_STEPS, "without the indexes the job list must scan ({scan_list} VM steps)");
}

/// The report's database reads run on one read (never a lock per node):
/// a report over every node takes the database once.
#[test]
fn a_placement_report_reads_the_database_once_for_every_node() {
    let _db = k2_core::db::scoped_for_test();
    let db = k2_core::db::shared();
    {
        let conn = db.lock();
        seed(&conn);
    }
    let before = k2_core::db::read_pool::read_counts_for_test();
    let w = want_cache();
    let report = candidates_report_raw(&w, &[], compute::now());
    let after = k2_core::db::read_pool::read_counts_for_test();
    assert_eq!(report.visible, NODES.len() as u32);
    let reads = (after.pool + after.held_lock + after.no_pool + after.nested) - (before.pool + before.held_lock + before.no_pool + before.nested);
    assert_eq!(reads, 1, "one read for {} nodes, not one per node: {before:?} → {after:?}", NODES.len());
}
