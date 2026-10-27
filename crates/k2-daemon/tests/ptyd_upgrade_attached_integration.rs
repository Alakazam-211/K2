//! k2-ptyd S7 (PRD prd-k2-ptyd-v1 §7.10, P63): ptyd upgrades itself in
//! place WHILE this daemon process has a session attached. The owner
//! connection and the session's data stream drop at the exec; the owner
//! reconnects in place (same `Owner`, same boot id, generation + 1), the
//! session's IO loop reattaches at its exact byte offset, and the Term
//! shows every `seq` line once — no loss, no duplicate. Control requests
//! (resize) and input go through the reconnected owner afterwards, and
//! nothing unsolicited reached the agent's stdin (P75).
//!
//! Needs the binaries: `cargo build -p k2-ptyd --bins` (k2-ptyd and
//! k2-ptyd-test-agent next to this test's target dir), or set
//! `K2_PTYD_BIN` / `K2_PTYD_TEST_AGENT`. Missing binaries fail loudly.
//! Uses a temp `HOME`; never touches the real `~/.k2` or a running daemon.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use k2_core::terminal::daemon_pty::{DaemonPtyConfig, DaemonPtySession};
use k2_core::terminal::ptyd::client;

fn bin(env: &str, name: &str) -> PathBuf {
    let p = std::env::var_os(env).map(PathBuf::from).unwrap_or_else(|| {
        let exe = std::env::current_exe().expect("current_exe");
        exe.parent().and_then(|d| d.parent()).map(|d| d.join(name)).expect("target dir")
    });
    assert!(p.is_file(), "{name} not found at {} — run `cargo build -p k2-ptyd --bins` first or set {env}", p.display());
    p
}

fn alive(pid: i32) -> bool {
    // SAFETY: existence probe only.
    unsafe { libc::kill(pid, 0) == 0 }
}

fn wait_for<T>(what: &str, within: Duration, mut f: impl FnMut() -> Option<T>) -> T {
    let t = Instant::now();
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(t.elapsed() < within, "timed out after {within:?} waiting for {what}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Every `seq=<n>` on the session's screen + scrollback, in order.
fn seqs(s: &DaemonPtySession) -> Vec<u64> {
    s.scan_rows(5000)
        .iter()
        .flat_map(|r| r.text.split_whitespace().filter_map(|t| t.strip_prefix("seq=")?.parse().ok()).collect::<Vec<u64>>())
        .collect()
}

/// Consecutive, no duplicates, reaching past `past`. Returns the max.
fn assert_no_gap(s: &DaemonPtySession, past: u64, what: &str) -> u64 {
    let max = wait_for(&format!("seq past {past} ({what})"), Duration::from_secs(15), || {
        let v = seqs(s);
        let m = *v.iter().max()?;
        (m > past + 10).then_some(m)
    });
    let v = seqs(s);
    let bad: Vec<_> = v.windows(2).filter(|w| w[1] != w[0] + 1).collect();
    assert!(bad.is_empty(), "{what}: seq lines lost or doubled at {bad:?}");
    assert!(v.first().copied().unwrap_or(u64::MAX) <= past, "{what}: the Term lost the lines before");
    max
}

fn listed_cols(owner: &client::Owner, sid: &str) -> u16 {
    match owner.request(k2_ptyd::proto::Request::List).expect("list") {
        k2_ptyd::proto::Reply::Sessions { sessions } => sessions.iter().find(|s| s.sid == sid).map(|s| s.cols).expect("our session"),
        other => panic!("list: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_attached_session_survives_a_ptyd_upgrade_with_no_lost_or_doubled_bytes() {
    let ptyd_bin = bin("K2_PTYD_BIN", "k2-ptyd");
    let agent = bin("K2_PTYD_TEST_AGENT", "k2-ptyd-test-agent");
    for k in std::env::vars().map(|(k, _)| k).filter(|k| k.starts_with("K2_") || k.starts_with("K2SO_")).collect::<Vec<_>>() {
        std::env::remove_var(k);
    }
    // Not "under systemd" (a compute job may run in a unit whose KillMode
    // would keep the session in-process, P46).
    std::env::remove_var("INVOCATION_ID");
    // Short path: the ptyd socket lives under HOME (sun_path is 104 bytes on macOS).
    let home = PathBuf::from(format!("/tmp/k2upa-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let k2 = home.join(".k2");
    std::fs::create_dir_all(&k2).unwrap();
    std::fs::create_dir_all(home.join("tmp")).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("K2_PTYD_BIN", &ptyd_bin);
    std::env::set_var("K2_PTYD", "1");
    // The first owner connection must NOT upgrade on its own (P65): the
    // minimum is the old version itself. This test upgrades explicitly.
    std::env::set_var("K2_PTYD_TEST_MIN_VERSION", "0.0.1");
    // The spawn guard refuses unknown programs under a temp HOME; this one
    // is the test's own fake agent.
    std::env::set_var("K2_TEST_ALLOW_REAL_AGENT", "1");
    client::set_home_for_tests(&k2);
    k2_core::db::init_for_tests();

    // A ptyd that reports an old version is already running.
    let st = std::process::Command::new(&ptyd_bin)
        .arg("--launch")
        .env_clear()
        .env("K2_HOME", &k2)
        .env("HOME", &home)
        .env("PATH", "/usr/bin:/bin")
        .env("TMPDIR", home.join("tmp"))
        .env("K2_PTYD_TEST_VERSION", "0.0.1")
        .status()
        .expect("launch ptyd");
    assert!(st.success(), "launch: {st}");
    let ptyd = client::global();
    let ptyd_pid = wait_for("ptyd lock", Duration::from_secs(10), || ptyd.ptyd_pid());

    let dir = home.join("agent");
    let cfg = DaemonPtyConfig {
        program: Some(agent.to_string_lossy().into_owned()),
        args: vec!["survive".into(), dir.to_string_lossy().into_owned(), "20".into()],
        cwd: Some(home.clone()),
        cols: 100,
        rows: 30,
        survive: true,
        ..DaemonPtyConfig::default()
    };
    let sid = cfg.session_id.to_string();
    let s: Arc<DaemonPtySession> = DaemonPtySession::spawn(cfg).expect("spawn");
    assert_eq!(s.backend_name(), "ptyd", "the switch is on: the session lives in ptyd");
    let owner = ptyd.current_owner().expect("owner");
    assert_eq!(owner.version(), "0.0.1", "connected to the prelaunched ptyd");
    let (boot, gen0) = (owner.boot_id.clone(), owner.generation());
    let pid = s.child_pid().expect("pid");
    assert_eq!(wait_for("agent pid file", Duration::from_secs(10), || std::fs::read_to_string(dir.join("pid")).ok()?.trim().parse::<i32>().ok()), pid);
    let past = assert_no_gap(&s, 0, "before the upgrade");

    // The binary the daemon would launch (its versioned copy, P3).
    let exe = client::run_location(&k2).expect("run location");
    assert!(Path::new(&exe).is_absolute());
    let now = ptyd.upgrade_now(&exe).expect("upgrade");
    assert_eq!(now, k2_ptyd::proto::PTYD_VERSION, "the new image runs");

    // Same owner object, reconnected; same ptyd instance and process.
    let same = ptyd.current_owner().expect("owner after the upgrade");
    assert!(Arc::ptr_eq(&same, &owner), "the owner reconnected in place");
    assert_eq!((owner.generation(), owner.boot_id.as_str()), (gen0 + 1, boot.as_str()));
    assert_eq!(ptyd.ptyd_pid(), Some(ptyd_pid));
    assert!(alive(ptyd_pid) && alive(pid), "ptyd and the agent kept running");
    assert!(s.is_child_alive(), "the session did not see an exit");
    assert!(!dir.join("sighup").exists(), "the agent got a SIGHUP");

    // The attached session reattached at its offset: no gap, no duplicate.
    let past = assert_no_gap(&s, past, "across the upgrade");
    assert!(std::fs::read(dir.join("stdin")).unwrap_or_default().is_empty(), "unsolicited bytes reached the agent (P75)");
    // Input goes through the reattached stream.
    s.write(b"after-upgrade".to_vec());
    wait_for("input after the upgrade", Duration::from_secs(10), || {
        (std::fs::read(dir.join("stdin")).unwrap_or_default() == b"after-upgrade").then_some(())
    });
    assert_no_gap(&s, past, "after input");
    // Control requests go through the reconnected owner. (Last: a real
    // resize clears the daemon's grid by design, so no gap check after it.)
    s.resize(120, 40);
    wait_for("the resize to reach ptyd", Duration::from_secs(10), || (listed_cols(&owner, &sid) == 120).then_some(()));

    s.kill();
    wait_for("the agent to end", Duration::from_secs(10), || (!alive(pid)).then_some(()));
    assert_eq!(
        owner.request(k2_ptyd::proto::Request::Shutdown { end_sessions: true }).expect("shutdown"),
        k2_ptyd::proto::Reply::Ok
    );
    let _ = std::fs::remove_dir_all(&home);
}
