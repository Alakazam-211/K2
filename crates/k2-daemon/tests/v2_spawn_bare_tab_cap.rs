//! 2026-07-02 PTY-leak breaker — pins the `/cli/sessions/v2/spawn`
//! never-attached bare-shell cap (defense in depth for the split-pane
//! restore re-mint loop, client fix b339c70).
//!
//! The leak's exact shape: a looping client minted a fresh `tab-<uuid>`
//! agent_name every layout-echo cycle and POSTed spawn with NO command;
//! each request became a bare login/zsh session nothing ever attached
//! to, until the box exhausted `kern.tty.ptmx_max`. The daemon-side cap
//! refuses to HOLD more than N live never-attached bare-shell `tab-*`
//! sessions per cwd:
//!
//!   - the (cap+1)-th bare `tab-*` spawn for one cwd is a 429, not a PTY;
//!   - spawns WITH a command are exempt (recovery/claude/heartbeat);
//!   - a session a client ever streamed (`ever_attached`) stops
//!     counting, so real once-viewed tabs never trip the cap;
//!   - a DIFFERENT cwd has its own budget.
//!
//! Self-contained: bare spawns run the real default shell; the cap is
//! shrunk to 3 via `K2_V2_BARE_TAB_CAP` so the test spawns a handful of
//! PTYs, all reaped in cleanup.

#![cfg(unix)]

use std::sync::Mutex as StdMutex;

use k2_core::db::init_for_tests;
use k2_daemon::v2_session_map;
use k2_daemon::v2_spawn::handle_v2_spawn;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn temp_cwd(tag: &str) -> String {
    let dir = std::env::temp_dir().join(format!(
        "k2so-bare-tab-cap-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir.to_string_lossy().into_owned()
}

/// POST a spawn for `agent_name` in `cwd`. `command: None` is the
/// leak shape (bare shell); `Some` is the exempt shape.
fn spawn(agent_name: &str, cwd: &str, command: Option<&str>) -> (String, String) {
    let body = serde_json::json!({
        "agent_name": agent_name,
        "cwd": cwd,
        "command": command,
        "cols": 40,
        "rows": 12,
    });
    let res = handle_v2_spawn(body.to_string().as_bytes());
    (res.status.to_string(), res.body)
}

/// Kill + drop every session this test registered. `clear_for_tests`
/// only empties the map, so kill first or the shells outlive the test.
fn reap(agents: &[String]) {
    for a in agents {
        if let Some(s) = v2_session_map::unregister(a) {
            s.kill();
        }
    }
    v2_session_map::clear_for_tests();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bare_tab_cap_refuses_spawn_number_cap_plus_one() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();
    std::env::set_var("K2_V2_BARE_TAB_CAP", "3");

    let cwd = temp_cwd("refuse");
    let mut spawned: Vec<String> = Vec::new();

    // Fill the budget: 3 bare tab shells all spawn fine.
    for i in 0..3 {
        let agent = format!("tab-cap-fill-{i}");
        let (status, body) = spawn(&agent, &cwd, None);
        assert_eq!(
            status, "200 OK",
            "bare spawn #{i} within the cap must succeed, got {status}: {body}"
        );
        spawned.push(agent);
    }

    // The 4th bare tab for the SAME cwd is the leak shape — refused, and
    // crucially NO session is registered (no PTY held).
    let (status, body) = spawn("tab-cap-overflow", &cwd, None);
    assert_eq!(
        status, "429 Too Many Requests",
        "bare spawn over the cap must be refused, got {status}: {body}"
    );
    assert!(
        body.contains("bare_tab_cap"),
        "refusal body must carry the machine-readable code, got: {body}"
    );
    assert!(
        v2_session_map::lookup_by_agent_name("tab-cap-overflow").is_none(),
        "a refused spawn must not register a session"
    );

    // A spawn WITH a command is exempt even at cap (recovery/claude/
    // heartbeat shapes must keep working). `cat` = cheap long-lived child.
    let (status, body) = spawn("tab-cap-with-cmd", &cwd, Some("cat"));
    assert_eq!(
        status, "200 OK",
        "command-carrying spawn must be exempt from the cap, got {status}: {body}"
    );
    spawned.push("tab-cap-with-cmd".to_string());

    // A DIFFERENT cwd has its own budget.
    let other_cwd = temp_cwd("other");
    let (status, body) = spawn("tab-cap-other-ws", &other_cwd, None);
    assert_eq!(
        status, "200 OK",
        "the cap is per-cwd; another workspace must spawn fine, got {status}: {body}"
    );
    spawned.push("tab-cap-other-ws".to_string());

    std::env::remove_var("K2_V2_BARE_TAB_CAP");
    reap(&spawned);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attached_sessions_stop_counting_against_the_cap() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();
    std::env::set_var("K2_V2_BARE_TAB_CAP", "2");

    let cwd = temp_cwd("attach");
    let mut spawned: Vec<String> = Vec::new();

    for i in 0..2 {
        let agent = format!("tab-att-{i}");
        let (status, body) = spawn(&agent, &cwd, None);
        assert_eq!(status, "200 OK", "fill spawn #{i} failed: {body}");
        spawned.push(agent);
    }
    let (status, _) = spawn("tab-att-blocked", &cwd, None);
    assert_eq!(status, "429 Too Many Requests", "cap must be hit at 2");

    // A client attaches to one of the shells (grid-WS sets the latch);
    // that session is a REAL tab now and stops counting, so the budget
    // frees one slot.
    let watched = v2_session_map::lookup_by_agent_name("tab-att-0")
        .expect("filled session must be registered");
    watched
        .ever_attached
        .store(true, std::sync::atomic::Ordering::Relaxed);

    let (status, body) = spawn("tab-att-after-watch", &cwd, None);
    assert_eq!(
        status, "200 OK",
        "an attached session must free its cap slot, got {status}: {body}"
    );
    spawned.push("tab-att-after-watch".to_string());

    std::env::remove_var("K2_V2_BARE_TAB_CAP");
    reap(&spawned);
}
