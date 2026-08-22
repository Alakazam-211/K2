//! Spawn-backed integration tests for the daemon-side Active reaper
//! (task #672, PRD `.k2so/prds/daemon-canonical-active.md`).
//!
//! These exercise the REAL `active_reaper::reconcile_pass` (via the
//! `ReaperTestDriver`) against REAL `DaemonPtySession` PTYs registered
//! in the REAL `v2_session_map`, with a real SQLite DB. The grace is
//! shrunk to a few ms so the arm → fire transition is observable
//! without a 15s wait.
//!
//! Coverage (need-clock Active reap):
//!   * an aged live canonical IS unregistered after grace; pin is not;
//!     in-window is not;
//!   * heartbeat fire bumps last_interaction_at; after window+grace
//!     with no further fire, PTY gone;
//!   * deliver_live inject into a live PTY bumps last_interaction_at;
//!   * API-only workspace: reaper does not unregister api-*;
//!   * extra tab-* in an aged workspace is not closed;
//!   * dedicated `{pid}:hb:*` IS closed on the same age-out as canonical;
//!   * dismiss of a live heartbeat-enabled workspace still reaps;
//!   * GET /cli/projects/active and ActiveChanged carry the same ids
//!     (window/pin, not live PTYs);
//!   * POST activate after dismiss un-suppresses without waiting a tick.

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
    spawn_named_pty(project_id, cwd)
}

fn spawn_named_pty(agent_name: &str, cwd: &PathBuf) -> Arc<DaemonPtySession> {
    let cfg = DaemonPtyConfig {
        cols: 80,
        rows: 24,
        cwd: Some(cwd.clone()),
        program: Some("cat".to_string()),
        ..DaemonPtyConfig::default()
    };
    let session = DaemonPtySession::spawn(cfg).expect("spawn cat PTY");
    v2_session_map::register(agent_name.to_string(), Arc::clone(&session));
    session
}

fn is_live(agent_name: &str) -> bool {
    v2_session_map::lookup_by_agent_name(agent_name).is_some()
}

fn last_interaction_secs(project_id: &str) -> Option<i64> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT last_interaction_at FROM projects WHERE id = ?1",
        rusqlite::params![project_id],
        |r| r.get::<_, Option<i64>>(0),
    )
    .expect("read last_interaction_at")
}

fn set_manually_active(project_id: &str, pinned: bool) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "UPDATE projects SET manually_active = ?1 WHERE id = ?2",
        rusqlite::params![if pinned { 1i64 } else { 0i64 }, project_id],
    )
    .expect("set manually_active");
}

fn insert_heartbeat(project_id: &str, name: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::db::schema::AgentHeartbeat::insert(
        &conn,
        &format!("hb-{name}-{project_id}"),
        project_id,
        name,
        "daily",
        "{}",
        "WAKEUP.md",
        true,
    )
    .expect("insert heartbeat");
}

fn get_active_ids() -> Vec<String> {
    let resp = k2_daemon::db_routes::handle_projects_active();
    assert_eq!(
        resp.status, "200 OK",
        "GET /cli/projects/active failed: {}",
        resp.body
    );
    let v: serde_json::Value =
        serde_json::from_str(&resp.body).expect("parse GET /cli/projects/active body");
    let arr = v
        .get("projectIds")
        .expect("projectIds field")
        .as_array()
        .expect("projectIds array");
    arr.iter()
        .map(|x| x.as_str().expect("project id string").to_string())
        .collect()
}

/// Small grace so arm → fire is fast. 50ms is well above scheduler
/// jitter yet keeps the test sub-second.
const GRACE_MS: u64 = 50;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn aged_live_pty_is_not_reaped_by_window_alone() {
    // Need clock: an aged live canonical IS unregistered after grace.
    // Pin still spares. In-window still spares. Live PTY alone does not.
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();

    let (aged_pid, aged_path) = setup_project("aged-live");
    set_last_interaction(&aged_pid, Some(now_secs() - 100 * 3600));
    let aged_session = spawn_chat_pty(&aged_pid, &aged_path);
    assert!(is_live(&aged_pid), "PTY should be live after spawn");

    let (pin_pid, pin_path) = setup_project("aged-pinned");
    set_last_interaction(&pin_pid, Some(now_secs() - 100 * 3600));
    set_manually_active(&pin_pid, true);
    let pin_session = spawn_chat_pty(&pin_pid, &pin_path);

    let (win_pid, win_path) = setup_project("in-window");
    set_last_interaction(&win_pid, Some(now_secs() - 3600));
    let win_session = spawn_chat_pty(&win_pid, &win_path);

    let mut reaper = ReaperTestDriver::with_grace_ms(GRACE_MS);

    reaper.pass().await;
    assert!(
        reaper.is_armed(&aged_pid),
        "aged live canonical must arm age-out grace"
    );
    assert!(
        !reaper.is_armed(&pin_pid),
        "pin (manually_active) must spare age-out"
    );
    assert!(
        !reaper.is_armed(&win_pid),
        "in-window chat must not arm"
    );
    assert!(is_live(&aged_pid), "still within grace");
    assert!(is_live(&pin_pid));
    assert!(is_live(&win_pid));

    tokio::time::sleep(Duration::from_millis(GRACE_MS + 30)).await;
    reaper.pass().await;
    assert!(
        !is_live(&aged_pid),
        "aged live canonical must be unregistered after grace"
    );
    assert!(
        is_live(&pin_pid),
        "pinned aged live canonical must not be reaped"
    );
    assert!(
        is_live(&win_pid),
        "in-window live canonical must not be reaped"
    );

    drop(aged_session);
    drop(pin_session);
    drop(win_session);
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
async fn heartbeat_fire_bumps_need_clock_then_ages_out() {
    // Heartbeat fire is a need (bumps last_interaction_at). Enablement
    // is not a spare. After window+grace with no further fire, PTY gone.
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();

    let (pid, path) = setup_project("hbfire");
    insert_heartbeat(&pid, "daily-brief");
    set_last_interaction(&pid, Some(now_secs() - 100 * 3600));
    let session = spawn_chat_pty(&pid, &path);

    let before = last_interaction_secs(&pid);
    assert!(
        before.is_some_and(|s| now_secs() - s >= 100 * 3600 - 5),
        "pre-fire last_interaction_at must be aged, got {before:?}"
    );

    active_reaper::note_workspace_need(&pid);
    let after = last_interaction_secs(&pid).expect("fire must stamp last_interaction_at");
    assert!(
        now_secs() - after <= 2,
        "heartbeat fire must bump last_interaction_at to now, got {after} now={}",
        now_secs()
    );

    set_last_interaction(&pid, Some(now_secs() - 100 * 3600));
    let mut reaper = ReaperTestDriver::with_grace_ms(GRACE_MS);
    reaper.pass().await;
    assert!(
        reaper.is_armed(&pid),
        "enabled heartbeat must not spare age-out after the need clock expires"
    );
    tokio::time::sleep(Duration::from_millis(GRACE_MS + 30)).await;
    reaper.pass().await;
    assert!(
        !is_live(&pid),
        "after window+grace with no further fire, canonical PTY must be gone"
    );

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deliver_live_inject_bumps_last_interaction_at() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();

    let (pid, path) = setup_project("inject-bump");
    set_last_interaction(&pid, Some(now_secs() - 100 * 3600));
    let session = spawn_chat_pty(&pid, &path);
    let sid = session.session_id.to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO workspace_sessions (id, project_id, session_id, harness, owner, status, created_at, active_terminal_id) \
             VALUES (?1, ?2, ?3, 'claude', 'user', 'running', unixepoch(), ?3)",
            rusqlite::params![format!("ws-{pid}"), pid, sid],
        )
        .expect("stamp workspace_sessions.active_terminal_id");
    }

    let before = last_interaction_secs(&pid).expect("seeded last_interaction_at");
    let path_str = path.to_string_lossy().into_owned();
    let r = k2_daemon::workspace_msg::deliver_live(
        &path_str,
        "hello need clock",
        "tester",
        "",
        false,
        std::time::Duration::from_secs(2),
    );
    assert!(
        r.success,
        "inject into live PTY must succeed, reason={:?} hint={:?}",
        r.reason, r.hint
    );
    let after = last_interaction_secs(&pid).expect("inject must stamp last_interaction_at");
    assert!(
        after > before,
        "deliver_live inject must bump last_interaction_at (before={before} after={after})"
    );

    drop(session);
    v2_session_map::clear_for_tests();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reaper_does_not_unregister_api_only_workspace() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();

    let (pid, path) = setup_project("api-only");
    set_last_interaction(&pid, Some(now_secs() - 100 * 3600));
    let api_name = format!("api-principal-{pid}");
    let session = spawn_named_pty(&api_name, &path);
    assert!(is_live(&api_name));

    let mut reaper = ReaperTestDriver::with_grace_ms(GRACE_MS);
    reaper.pass().await;
    assert!(
        !reaper.is_armed(&pid),
        "api-* only workspace must not be a reap candidate"
    );
    tokio::time::sleep(Duration::from_millis(GRACE_MS + 30)).await;
    reaper.pass().await;
    assert!(
        is_live(&api_name),
        "reaper must not unregister api-* sessions"
    );

    drop(session);
    v2_session_map::clear_for_tests();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn extra_tab_in_aged_workspace_is_not_closed() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();

    let (pid, path) = setup_project("extra-tab");
    set_last_interaction(&pid, Some(now_secs() - 100 * 3600));
    let canonical = spawn_chat_pty(&pid, &path);
    let tab_name = format!("tab-{pid}");
    let tab = spawn_named_pty(&tab_name, &path);

    let mut reaper = ReaperTestDriver::with_grace_ms(GRACE_MS);
    reaper.pass().await;
    assert!(reaper.is_armed(&pid), "aged canonical must arm");
    tokio::time::sleep(Duration::from_millis(GRACE_MS + 30)).await;
    reaper.pass().await;
    assert!(!is_live(&pid), "aged canonical must close");
    assert!(
        is_live(&tab_name),
        "extra tab-* in an aged workspace must not be closed"
    );

    drop(canonical);
    drop(tab);
    v2_session_map::clear_for_tests();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dedicated_heartbeat_pty_closes_on_same_age_out_as_canonical() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();

    let (pid, path) = setup_project("hb-pty");
    set_last_interaction(&pid, Some(now_secs() - 100 * 3600));
    let canonical = spawn_chat_pty(&pid, &path);
    let hb_name = format!("{pid}:hb:daily-brief");
    let hb = spawn_named_pty(&hb_name, &path);

    let mut reaper = ReaperTestDriver::with_grace_ms(GRACE_MS);
    reaper.pass().await;
    assert!(reaper.is_armed(&pid), "aged workspace must arm");
    tokio::time::sleep(Duration::from_millis(GRACE_MS + 30)).await;
    reaper.pass().await;
    assert!(!is_live(&pid), "canonical must close on age-out");
    assert!(
        !is_live(&hb_name),
        "dedicated {{pid}}:hb:* must close on the same age-out as canonical"
    );

    drop(canonical);
    drop(hb);
    v2_session_map::clear_for_tests();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dismiss_of_heartbeat_enabled_live_workspace_still_reaps() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();

    let (pid, path) = setup_project("hb-dismiss");
    set_last_interaction(&pid, Some(now_secs() - 3600));
    insert_heartbeat(&pid, "still-reaps");
    let session = spawn_chat_pty(&pid, &path);

    let mut reaper = ReaperTestDriver::with_grace_ms(GRACE_MS);
    active_reaper::arm_dismiss_grace(&pid);
    reaper.pass().await;
    assert!(
        reaper.is_armed(&pid),
        "dismiss must arm even when heartbeat is enabled"
    );
    tokio::time::sleep(Duration::from_millis(GRACE_MS + 30)).await;
    reaper.pass().await;
    assert!(
        !is_live(&pid),
        "dismiss of a live heartbeat-enabled workspace must still reap after grace"
    );

    drop(session);
    v2_session_map::clear_for_tests();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_active_and_active_changed_match_window_pin_only() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();

    let (win_pid, win_path) = setup_project("active-win");
    set_last_interaction(&win_pid, Some(now_secs() - 3600));
    let win_session = spawn_chat_pty(&win_pid, &win_path);

    let (pin_pid, pin_path) = setup_project("active-pin");
    set_last_interaction(&pin_pid, Some(now_secs() - 100 * 3600));
    set_manually_active(&pin_pid, true);
    let pin_session = spawn_chat_pty(&pin_pid, &pin_path);

    let (live_pid, live_path) = setup_project("active-live-only");
    set_last_interaction(&live_pid, None);
    let live_session = spawn_chat_pty(&live_pid, &live_path);

    let mut rx = k2_daemon::session_events::subscribe();
    active_reaper::recompute_and_broadcast_active();

    let mut broadcast_ids: Option<Vec<String>> = None;
    for _ in 0..32 {
        match rx.try_recv() {
            Ok(k2_daemon::session_events::SessionEvent::ActiveChanged {
                active_project_ids,
                ..
            }) => {
                broadcast_ids = Some(active_project_ids);
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    let mut broadcast_ids = broadcast_ids.expect("ActiveChanged must be emitted");
    let mut get_ids = get_active_ids();
    broadcast_ids.sort();
    get_ids.sort();
    assert_eq!(
        get_ids, broadcast_ids,
        "GET /cli/projects/active and ActiveChanged must carry the same ids"
    );
    assert!(
        get_ids.contains(&win_pid),
        "in-window workspace must be Active"
    );
    assert!(
        get_ids.contains(&pin_pid),
        "pinned workspace must be Active"
    );
    assert!(
        !get_ids.contains(&live_pid),
        "live PTY alone must not join Active (window/pin only)"
    );

    drop(win_session);
    drop(pin_session);
    drop(live_session);
    v2_session_map::clear_for_tests();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn post_activate_after_dismiss_unsuppresses_immediately() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();

    let (pid, path) = setup_project("activate-after-dismiss");
    set_last_interaction(&pid, Some(now_secs() - 3600));
    let session = spawn_chat_pty(&pid, &path);

    let body = serde_json::json!({ "projectId": pid }).to_string().into_bytes();
    let dismiss = k2_daemon::db_routes::handle_projects_dismiss(&body);
    assert_eq!(
        dismiss.status, "200 OK",
        "dismiss failed: {}",
        dismiss.body
    );
    assert!(
        !get_active_ids().contains(&pid),
        "dismissed workspace must leave GET /cli/projects/active immediately"
    );

    let activate = k2_daemon::db_routes::handle_projects_activate(&body);
    assert_eq!(
        activate.status, "200 OK",
        "activate failed: {}",
        activate.body
    );
    assert!(
        get_active_ids().contains(&pid),
        "POST activate after dismiss must un-suppress immediately"
    );

    drop(session);
    v2_session_map::clear_for_tests();
}
