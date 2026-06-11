//! Spawn-backed integration tests for the daemon-side Active reaper
//! (task #672, PRD `.k2so/prds/daemon-canonical-active.md`).
//!
//! These exercise the REAL `active_reaper::reconcile_pass` (via the
//! `ReaperTestDriver`) against REAL `DaemonPtySession` PTYs registered
//! in the REAL `v2_session_map`, with a real SQLite DB. The grace is
//! shrunk to a few ms so the arm → fire transition is observable
//! without a 15s wait.
//!
//! Coverage (PRD §10 daemon integration):
//!   * an aged (non-Active) chat PTY is reaped after the grace;
//!   * an Active (within-window) chat PTY is NOT reaped;
//!   * a heartbeat-warm aged chat PTY is NOT reaped;
//!   * `dismiss` arms the grace → reap (even within-window);
//!   * re-activate within the grace cancels the timer (no reap).

#![cfg(unix)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use k2_core::db::init_for_tests;
use k2_core::terminal::{DaemonPtyConfig, DaemonPtySession};

use k2_daemon::active_reaper::{self, ReaperTestDriver};
use k2_daemon::v2_session_map;

/// Serialize tests — they all touch process globals (DB, the
/// v2_session_map, the reaper's pending-dismiss signal set).
static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Unique temp project path + registered `projects` row. Returns
/// (project_id, project_path).
fn setup_project(tag: &str) -> (String, PathBuf) {
    let project_path = std::env::temp_dir().join(format!(
        "k2so-active-reaper-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&project_path);
    std::fs::create_dir_all(&project_path).unwrap();

    let project_id = format!("reaper-pid-{tag}-{}", std::process::id());
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT OR REPLACE INTO projects (id, path, name, color, agent_mode, pinned, tab_order, manually_active) \
         VALUES (?1, ?2, ?3, '#123456', 'off', 0, 0, 0)",
        rusqlite::params![project_id, project_path.to_string_lossy().as_ref(), "reaper-test"],
    )
    .unwrap();
    (project_id, project_path)
}

/// Set `projects.last_interaction_at` to `secs` (unix seconds). `None`
/// nulls it.
fn set_last_interaction(project_id: &str, secs: Option<i64>) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "UPDATE projects SET last_interaction_at = ?1 WHERE id = ?2",
        rusqlite::params![secs, project_id],
    )
    .unwrap();
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// Spawn a real PTY running `cat` (benign, exits when its master
/// channel closes) in `cwd`, register it in the v2 map under the bare
/// project_id key (the canonical workspace-chat shape), and return the
/// session Arc so the test holds a strong reference.
fn spawn_chat_pty(project_id: &str, cwd: &PathBuf) -> Arc<DaemonPtySession> {
    let cfg = DaemonPtyConfig {
        cols: 80,
        rows: 24,
        cwd: Some(cwd.clone()),
        program: Some("cat".to_string()),
        ..DaemonPtyConfig::default()
    };
    let session = DaemonPtySession::spawn(cfg).expect("spawn cat PTY");
    v2_session_map::register(project_id.to_string(), Arc::clone(&session));
    session
}

fn is_live(project_id: &str) -> bool {
    v2_session_map::lookup_by_agent_name(project_id).is_some()
}

/// Small grace so arm → fire is fast. 50ms is well above scheduler
/// jitter yet keeps the test sub-second.
const GRACE_MS: u64 = 50;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn aged_chat_is_reaped_after_grace() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();

    let (pid, path) = setup_project("aged");
    // Aged out: interacted 100h ago, default window is 24h.
    set_last_interaction(&pid, Some(now_secs() - 100 * 3600));
    let _session = spawn_chat_pty(&pid, &path);
    assert!(is_live(&pid), "PTY should be live after spawn");

    let mut reaper = ReaperTestDriver::with_grace_ms(GRACE_MS);

    // Pass 1: aged + not warm → arm grace. Not yet fired.
    reaper.pass().await;
    assert!(reaper.is_armed(&pid), "aged chat should be armed");
    assert!(is_live(&pid), "must not reap before grace elapses");

    // Wait past the grace, then a second pass fires the reap.
    tokio::time::sleep(Duration::from_millis(GRACE_MS + 30)).await;
    reaper.pass().await;
    assert!(
        !is_live(&pid),
        "aged chat must be reaped after the grace elapses"
    );

    v2_session_map::clear_for_tests();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn active_chat_is_not_reaped() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();

    let (pid, path) = setup_project("active");
    // Within window: interacted 1h ago.
    set_last_interaction(&pid, Some(now_secs() - 3600));
    let session = spawn_chat_pty(&pid, &path);
    assert!(is_live(&pid));

    let mut reaper = ReaperTestDriver::with_grace_ms(GRACE_MS);
    reaper.pass().await;
    assert!(
        !reaper.is_armed(&pid),
        "an Active (within-window) chat must never be armed"
    );

    tokio::time::sleep(Duration::from_millis(GRACE_MS + 30)).await;
    reaper.pass().await;
    assert!(is_live(&pid), "an Active chat must never be reaped");

    drop(session);
    v2_session_map::clear_for_tests();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn heartbeat_warm_aged_chat_is_not_reaped() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();

    let (pid, path) = setup_project("hbwarm");
    // Aged out, but a heartbeat keeps it warm.
    set_last_interaction(&pid, Some(now_secs() - 100 * 3600));
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::AgentHeartbeat::insert(
            &conn,
            &format!("hb-{pid}"),
            &pid,
            "default",
            "daily",
            "{}",
            "WAKEUP.md",
            true, // enabled
        )
        .unwrap();
    }
    let session = spawn_chat_pty(&pid, &path);

    let mut reaper = ReaperTestDriver::with_grace_ms(GRACE_MS);
    reaper.pass().await;
    assert!(
        !reaper.is_armed(&pid),
        "heartbeat-warm chat must never be armed"
    );
    tokio::time::sleep(Duration::from_millis(GRACE_MS + 30)).await;
    reaper.pass().await;
    assert!(is_live(&pid), "heartbeat-warm chat must never be reaped");

    drop(session);
    v2_session_map::clear_for_tests();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dismiss_arms_grace_then_reaps_even_within_window() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();

    let (pid, path) = setup_project("dismiss");
    // WITHIN the window (1h ago) — normally Active + unreapable. The
    // explicit dismiss arms the grace anyway.
    set_last_interaction(&pid, Some(now_secs() - 3600));
    let _session = spawn_chat_pty(&pid, &path);

    let mut reaper = ReaperTestDriver::with_grace_ms(GRACE_MS);

    // Sanity: without a dismiss, a within-window chat is NOT armed.
    reaper.pass().await;
    assert!(!reaper.is_armed(&pid), "within-window chat not armed pre-dismiss");
    assert!(is_live(&pid));

    // Dismiss arms the grace NOW.
    active_reaper::arm_dismiss_grace(&pid);
    reaper.pass().await;
    assert!(
        reaper.is_armed(&pid),
        "dismiss must arm the grace even within the window"
    );
    assert!(is_live(&pid), "still within grace");

    tokio::time::sleep(Duration::from_millis(GRACE_MS + 30)).await;
    reaper.pass().await;
    assert!(
        !is_live(&pid),
        "dismissed chat must be reaped after the grace"
    );

    v2_session_map::clear_for_tests();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reactivate_within_grace_cancels_dismiss_reap() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();

    let (pid, path) = setup_project("reactivate");
    // Within window, then dismissed.
    let armed_secs = now_secs() - 3600;
    set_last_interaction(&pid, Some(armed_secs));
    let session = spawn_chat_pty(&pid, &path);

    let mut reaper = ReaperTestDriver::with_grace_ms(GRACE_MS);
    active_reaper::arm_dismiss_grace(&pid);
    reaper.pass().await;
    assert!(reaper.is_armed(&pid), "dismiss armed the grace");

    // Re-activate within the grace: a fresh interaction advances
    // last_interaction_at past the value captured when the timer armed.
    set_last_interaction(&pid, Some(now_secs() + 5));
    reaper.pass().await;
    assert!(
        !reaper.is_armed(&pid),
        "re-activation within the grace must cancel the forced timer"
    );

    // Even after the grace window elapses, the cancelled timer never
    // fires.
    tokio::time::sleep(Duration::from_millis(GRACE_MS + 30)).await;
    reaper.pass().await;
    assert!(
        is_live(&pid),
        "re-activated chat must not be reaped"
    );

    drop(session);
    v2_session_map::clear_for_tests();
}
