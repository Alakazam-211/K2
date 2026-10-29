//! k2-ptyd P76 (PRD prd-k2-ptyd-v1 §7.5, vs-live V3): the `survive` flag
//! is set at the daemon's spawn sites and backend selection happens inside
//! `DaemonPtySession::spawn`. A canonical pinned chat and a heartbeat
//! session, both spawned through `spawn.rs`
//! (`spawn_agent_session_v2_blocking`), land on ptyd with the default
//! switch (on since 0.46.5: no `K2_PTYD`, no `terminal.ptyd`) — and stay
//! in-process (ptyd never launched) when the owner sets `terminal.ptyd =
//! off`.
//!
//! Needs the `k2-ptyd` binary: run as
//! `cargo build -p k2-ptyd --bin k2-ptyd && cargo test -p k2-daemon --test ptyd_p76_integration`
//! or set `K2_PTYD_BIN`. A missing binary fails loudly. The test removes
//! every `K2_*` / `K2SO_*` var and uses a temp `HOME`; it never touches the
//! real `~/.k2` or a running daemon's ptyd.
// k2-ptyd is Unix-only until its Windows port (0.47: ConPTY, named pipe, Job Objects).
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use k2_core::db::init_for_tests;
use k2_daemon::canonical_session::ensure_canonical_session;
use k2_daemon::v2_session_map;

fn ptyd_bin() -> PathBuf {
    if let Some(p) = std::env::var_os("K2_PTYD_BIN") {
        return PathBuf::from(p);
    }
    // target/<profile>/deps/ptyd_p76_integration-<hash> → target/<profile>/k2-ptyd
    let exe = std::env::current_exe().expect("current_exe");
    exe.parent().and_then(|d| d.parent()).map(|d| d.join("k2-ptyd")).expect("target dir")
}

fn parent_pid(pid: i32) -> i32 {
    let out = std::process::Command::new("ps").args(["-o", "ppid=", "-p", &pid.to_string()]).output().expect("ps");
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or_else(|_| panic!("ppid of {pid}: {:?}", out))
}

fn register_project(workspace_id: &str, root: &Path) -> PathBuf {
    let project = root.join(workspace_id);
    std::fs::create_dir_all(&project).unwrap();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT OR REPLACE INTO projects (id, path, name, agent_mode, agent_enabled) VALUES (?1, ?2, ?3, 'custom', 1)",
        rusqlite::params![workspace_id, project.to_string_lossy().as_ref(), workspace_id],
    )
    .unwrap();
    project
}

/// AGENT.md whose launch profile runs `cat` (no real agent CLI).
fn write_agent_md(project: &Path, name: &str) {
    let dir = project.join(".k2so/agent");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("AGENT.md"), format!("---\nname: {name}\ntype: custom\nlaunch:\n  command: cat\n---\n# {name}\n")).unwrap();
}

/// Heartbeat lane key the wake path registers under (0.37.8).
fn hb_key(workspace_id: &str, hb: &str) -> String {
    format!("{workspace_id}:hb:{hb}")
}

fn session_for(key: &str) -> std::sync::Arc<k2_core::terminal::daemon_pty::DaemonPtySession> {
    let t = Instant::now();
    loop {
        if let Some(s) = v2_session_map::lookup_by_agent_name(key) {
            return s;
        }
        assert!(t.elapsed() < Duration::from_secs(10), "no live session registered under {key}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// One sequential test so the switch-off half provably runs before ptyd
/// was ever launched in this process. 0.46.5: the switch is ON by default
/// (no `K2_PTYD`, no `terminal.ptyd`); only the owner's `off` keeps new
/// sessions in-process.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn canonical_chat_and_heartbeat_spawns_land_on_ptyd_unless_the_owner_turned_it_off() {
    let bin = ptyd_bin();
    assert!(
        bin.is_file(),
        "k2-ptyd binary not found at {} — run `cargo build -p k2-ptyd --bin k2-ptyd` first or set K2_PTYD_BIN",
        bin.display()
    );
    for k in std::env::vars().map(|(k, _)| k).filter(|k| k.starts_with("K2_") || k.starts_with("K2SO_")).collect::<Vec<_>>() {
        std::env::remove_var(k);
    }
    // Short path: the ptyd socket lives under HOME (sun_path is 104 bytes on macOS).
    let home = k2_core::test_env::short_tmp(format!("k2p76-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(home.join(".k2")).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("K2_PTYD_BIN", &bin);
    k2_core::terminal::ptyd::client::set_home_for_tests(&home.join(".k2"));
    // Heartbeat wakes spawn `cat` instead of the workspace's agent CLI.
    std::env::set_var("K2SO_WAKE_HEADLESS_TEST_COMMAND", "cat");

    init_for_tests();
    v2_session_map::clear_for_tests();
    let ptyd = k2_core::terminal::ptyd::client::global();

    // ── the owner turned it off (terminal.ptyd = off): both lanes
    // in-process, ptyd untouched ──
    assert!(std::env::var_os("K2_PTYD").is_none());
    k2_core::app_settings::update(serde_json::json!({"terminal": {"ptyd": "off"}})).expect("terminal.ptyd = off");
    assert_eq!(k2_core::terminal::ptyd::Switch::resolve(), k2_core::terminal::ptyd::Switch::Off);
    let off = register_project("p76-off", &home);
    write_agent_md(&off, "offbot");
    let off_path = off.to_string_lossy().into_owned();
    let chat = ensure_canonical_session(&off_path).expect("canonical chat (switch off)");
    assert!(!chat.reused);
    let s = session_for("p76-off");
    assert_eq!(s.backend_name(), "in-process", "switch off: canonical chat stays in-process");
    let pid = s.child_pid().expect("pid");
    assert_eq!(parent_pid(pid) as u32, std::process::id(), "switch off: the daemon is the chat's parent");
    k2_daemon::wake_headless::spawn_wake_headless("offbot", &off_path, "hb prompt", Some("p76hb")).expect("heartbeat spawn (switch off)");
    let h = session_for(&hb_key("p76-off", "p76hb"));
    assert_eq!(h.backend_name(), "in-process", "switch off: heartbeat stays in-process");
    assert!(ptyd.ptyd_pid().is_none(), "switch off: ptyd was never launched");
    assert!(!ptyd.paths().home.join("bin").join("ptyd").exists(), "switch off: no versioned ptyd copy was placed");
    assert_eq!(k2_core::terminal::ptyd::status_line(), "off (switch_off)");
    s.kill();
    h.kill();

    // ── the default (setting unset, no K2_PTYD): the same two spawn.rs
    // lanes land on ptyd ──
    k2_core::app_settings::update(serde_json::json!({"terminal": {"ptyd": ""}})).expect("clear terminal.ptyd");
    assert!(k2_core::app_settings::load().terminal.ptyd.is_empty(), "the setting is unset again");
    assert!(std::env::var_os("K2_PTYD").is_none());
    assert_eq!(k2_core::terminal::ptyd::Switch::resolve(), k2_core::terminal::ptyd::Switch::On, "on by default");
    let on = register_project("p76-on", &home);
    write_agent_md(&on, "onbot");
    let on_path = on.to_string_lossy().into_owned();

    let chat = ensure_canonical_session(&on_path).expect("canonical chat (switch on)");
    assert!(!chat.reused);
    let s = session_for("p76-on");
    assert_eq!(s.backend_name(), "ptyd", "P76: canonical pinned chat lands on ptyd");
    let ptyd_pid = ptyd.ptyd_pid().expect("ptyd launched");
    assert_ne!(ptyd_pid as u32, std::process::id());
    assert_eq!(parent_pid(s.child_pid().expect("pid")), ptyd_pid, "the chat's parent is ptyd, not the daemon");

    k2_daemon::wake_headless::spawn_wake_headless("onbot", &on_path, "hb prompt", Some("p76hb")).expect("heartbeat spawn (switch on)");
    let h = session_for(&hb_key("p76-on", "p76hb"));
    assert_eq!(h.backend_name(), "ptyd", "P76: heartbeat session lands on ptyd");
    assert_eq!(parent_pid(h.child_pid().expect("pid")), ptyd_pid, "the heartbeat's parent is ptyd");
    assert_eq!(k2_core::terminal::ptyd::status_line(), "on");

    // Both live in ptyd's own session table.
    let owner = ptyd.owner().expect("owner");
    let listed = match owner.request(k2_ptyd::proto::Request::List).expect("list") {
        k2_ptyd::proto::Reply::Sessions { sessions } => sessions,
        other => panic!("{other:?}"),
    };
    for sess in [&s, &h] {
        let pid = sess.child_pid().unwrap();
        assert!(listed.iter().any(|x| x.pid == pid), "ptyd does not list pid {pid}: {listed:?}");
    }

    s.kill();
    h.kill();
    assert_eq!(
        owner.request(k2_ptyd::proto::Request::Shutdown { end_sessions: true }).expect("shutdown"),
        k2_ptyd::proto::Reply::Ok
    );
    let _ = std::fs::remove_dir_all(&home);
}
