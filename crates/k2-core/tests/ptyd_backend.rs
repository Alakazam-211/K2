//! `DaemonPtySession` on the k2-ptyd backend (PRD prd-k2-ptyd-v1 S2).
//!
//! Needs the `k2-ptyd` binary: run as
//! `cargo build -p k2-ptyd --bin k2-ptyd && cargo test -p k2-core --test ptyd_backend`
//! or set `K2_PTYD_BIN`. A missing
//! binary fails loudly. This binary sets `K2_PTYD=1` and a temp `HOME` for
//! itself only; it never touches the real `~/.k2`.

#![cfg(unix)]

use std::path::PathBuf;
use std::sync::{Mutex, Once};
use std::time::{Duration, Instant};

use k2_core::terminal::daemon_pty::{AlacEvent, DaemonPtyConfig, DaemonPtySession};

static SETUP: Once = Once::new();
static SERIAL: Mutex<()> = Mutex::new(());

fn ptyd_bin() -> PathBuf {
    if let Some(p) = std::env::var_os("K2_PTYD_BIN") {
        return PathBuf::from(p);
    }
    let exe = std::env::current_exe().unwrap();
    // target/<profile>/deps/ptyd_backend-<hash> → target/<profile>/k2-ptyd
    exe.parent().and_then(|d| d.parent()).map(|d| d.join("k2-ptyd")).unwrap()
}

fn setup() -> std::sync::MutexGuard<'static, ()> {
    let g = match SERIAL.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    SETUP.call_once(|| {
        let bin = ptyd_bin();
        assert!(bin.is_file(), "k2-ptyd binary not found at {} — run `cargo build -p k2-ptyd --bin k2-ptyd` first or set K2_PTYD_BIN", bin.display());
        let home = PathBuf::from(format!("/tmp/k2pb-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join(".k2")).unwrap();
        for k in std::env::vars().map(|(k, _)| k).filter(|k| k.starts_with("K2_") || k.starts_with("K2SO_")).collect::<Vec<_>>() {
            std::env::remove_var(k);
        }
        // This test process is not the daemon's systemd unit. Under a CI
        // runner service (GitHub Actions: INVOCATION_ID set, the runner's
        // unit is KillMode=control-group) `current_killmode()` would read the
        // RUNNER's unit and rightly keep survive sessions in-process
        // (`killmode_control-group`). That gate is unit-tested in
        // terminal::ptyd (select_backend); here it must see "not under
        // systemd", as on a dev machine, so the ptyd backend is what's tested.
        std::env::remove_var("INVOCATION_ID");
        std::env::set_var("HOME", &home);
        std::env::set_var("K2_PTYD", "1");
        std::env::set_var("K2_PTYD_BIN", &bin);
        k2_core::terminal::ptyd::client::set_home_for_tests(&home.join(".k2"));
    });
    g
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap())
}

fn cfg(script: &str) -> DaemonPtyConfig {
    DaemonPtyConfig {
        program: Some("/bin/sh".into()),
        args: vec!["-c".into(), script.into()],
        cwd: Some(home()),
        survive: true,
        ..DaemonPtyConfig::default()
    }
}

fn wait_file(p: &PathBuf, within: Duration) -> String {
    let t = Instant::now();
    loop {
        if let Ok(s) = std::fs::read_to_string(p) {
            if !s.is_empty() {
                std::thread::sleep(Duration::from_millis(100));
                return std::fs::read_to_string(p).unwrap();
            }
        }
        assert!(t.elapsed() < within, "{} never written", p.display());
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn parent_pid(pid: i32) -> i32 {
    let out = std::process::Command::new("ps").args(["-o", "ppid=", "-p", &pid.to_string()]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse().expect("ppid")
}

fn alive(pid: i32) -> bool {
    // SAFETY: probe only.
    unsafe { libc::kill(pid, 0) == 0 }
}

fn wait_child_exit(rx: &mut tokio::sync::broadcast::Receiver<AlacEvent>, within: Duration) -> std::process::ExitStatus {
    let t = Instant::now();
    loop {
        match rx.try_recv() {
            Ok(AlacEvent::ChildExit(st)) => return st,
            Ok(_) => {}
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => panic!("event channel: {e:?}"),
        }
        assert!(t.elapsed() < within, "no ChildExit within {within:?}");
    }
}

fn cpu_secs() -> f64 {
    // SAFETY: getrusage fills a struct.
    unsafe {
        let mut ru: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &mut ru);
        ru.ru_utime.tv_sec as f64 + ru.ru_utime.tv_usec as f64 / 1e6 + ru.ru_stime.tv_sec as f64 + ru.ru_stime.tv_usec as f64 / 1e6
    }
}

#[test]
fn survive_session_runs_under_ptyd_and_kill_reaps_through_ptyd() {
    let _g = setup();
    let s = DaemonPtySession::spawn(DaemonPtyConfig { program: Some("cat".into()), args: vec![], ..cfg("") }).expect("spawn");
    assert_eq!(s.backend_name(), "ptyd");
    let pid = s.child_pid().expect("pid");
    let ptyd = k2_core::terminal::ptyd::client::global().ptyd_pid().expect("ptyd pid");
    assert_eq!(parent_pid(pid), ptyd, "the agent's parent is ptyd, not the daemon");
    // SAFETY: getpid.
    assert_ne!(ptyd, unsafe { libc::getpid() });
    assert_eq!(k2_core::terminal::ptyd::status_line(), "on");

    // Input in, Term updated by K2's IO loop.
    s.write(b"hello-ptyd\r".to_vec());
    let t = Instant::now();
    while !s.visible_text_rows().iter().any(|r| r.contains("hello-ptyd")) {
        assert!(t.elapsed() < Duration::from_secs(10), "echo never reached the Term: {:?}", s.visible_text_rows());
        std::thread::sleep(Duration::from_millis(20));
    }

    // P26 twin of kill_terminates_and_reaps_child + kill_is_idempotent:
    // pid gone, no waitpid by us (ptyd is the parent), second kill a no-op.
    s.kill();
    assert!(!alive(pid), "child gone after kill()");
    assert!(!s.is_child_alive());
    // Twin of kill_releases_pty_master_fd_with_arc_retained: with the Arc
    // still held, ptyd has dropped the session (stream closed, exit acked).
    let owner = k2_core::terminal::ptyd::client::global().owner().unwrap();
    match owner.request(k2_ptyd::proto::Request::List).unwrap() {
        k2_ptyd::proto::Reply::Sessions { sessions } => {
            assert!(sessions.iter().all(|x| x.pid != pid), "ptyd still holds the killed session: {sessions:?}")
        }
        other => panic!("{other:?}"),
    }
    let t = Instant::now();
    s.kill();
    assert!(t.elapsed() < Duration::from_millis(50), "second kill is a no-op");
}

#[test]
fn label_and_pin_changes_reach_ptyd_meta_without_secrets() {
    let _g = setup();
    std::env::set_var("K2_TEST_CANARY_SECRET", "canary-9c1e");
    let s = DaemonPtySession::spawn(cfg("sleep 30")).expect("spawn");
    std::env::remove_var("K2_TEST_CANARY_SECRET");
    s.set_label("Renamed".into(), true);
    DaemonPtySession::set_pinned(&s, Some((90, 30)), Some("owner".into()));
    let owner = k2_core::terminal::ptyd::client::global().owner().unwrap();
    let sid = s.session_id.to_string();
    let t = Instant::now();
    let meta = loop {
        let k2_ptyd::proto::Reply::Sessions { sessions } = owner.request(k2_ptyd::proto::Request::List).unwrap() else { panic!() };
        let m = sessions.into_iter().find(|x| x.sid == sid).expect("session listed").meta;
        if m["label"] == "Renamed" && m["pin"]["cols"] == 90 {
            break m;
        }
        assert!(t.elapsed() < Duration::from_secs(5), "meta never updated: {m}");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(meta["label_source"], "Locked");
    assert_eq!(meta["pin"]["set_by"], "owner");
    assert_eq!(meta["session_id"], sid);
    assert!(!meta.to_string().contains("canary-9c1e"), "P72: meta never carries env values");
    s.kill();
}

#[test]
fn exit_status_arrives_as_child_exit() {
    let _g = setup();
    let s = DaemonPtySession::spawn(cfg("sleep 0.5; exit 7")).expect("spawn");
    let mut rx = s.subscribe_events();
    let st = wait_child_exit(&mut rx, Duration::from_secs(10));
    assert_eq!(st.code(), Some(7));
}

#[test]
fn child_env_matches_the_in_process_backend() {
    let _g = setup();
    let mut dumps = Vec::new();
    for survive in [false, true] {
        let out = home().join(format!("env-{survive}"));
        let _ = std::fs::remove_file(&out);
        let s = DaemonPtySession::spawn(DaemonPtyConfig { survive, ..cfg(&format!("env | sort > '{}'; sleep 30", out.display())) }).expect("spawn");
        assert_eq!(s.backend_name(), if survive { "ptyd" } else { "in-process" });
        dumps.push(wait_file(&out, Duration::from_secs(10)));
        s.kill();
    }
    assert_eq!(dumps[0], dumps[1], "P15: the ptyd child sees exactly the in-process child's env");
}

#[test]
fn terminal_query_replies_reach_the_child_live() {
    let _g = setup();
    let out = home().join("cpr");
    let _ = std::fs::remove_file(&out);
    let s = DaemonPtySession::spawn(cfg(&format!(
        "stty raw -echo; printf 'ab\\033[6n'; dd bs=1 count=6 2>/dev/null > '{}'; sleep 30",
        out.display()
    )))
    .expect("spawn");
    assert_eq!(wait_file(&out, Duration::from_secs(10)), "\x1b[1;3R", "the CPR answer was typed into the child");
    s.kill();
}

#[test]
fn resize_reaches_the_child() {
    let _g = setup();
    let out = home().join("size");
    let _ = std::fs::remove_file(&out);
    let s = DaemonPtySession::spawn(cfg(&format!("sleep 1; stty size > '{}'; sleep 30", out.display()))).expect("spawn");
    s.resize(100, 40);
    assert_eq!(wait_file(&out, Duration::from_secs(10)).trim(), "40 100");
    s.kill();
}

/// S3 (PRD §7.7, §7.9, P34, P75): a new daemon ADOPTS the session a
/// previous daemon left in ptyd. B0 lists it without ending it; `adopt`
/// rebuilds the Term from the ring with the same pid; the terminal query
/// in the replayed bytes is NOT answered (zero bytes reach the child);
/// live input works after the switch; `detach` lets go without ending it.
#[test]
fn a_new_daemon_adopts_a_session_left_by_a_previous_daemon() {
    let _g = setup();
    use k2_ptyd::proto::{Reply, Request};
    let home2 = PathBuf::from(format!("/tmp/k2pb2-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home2);
    std::fs::create_dir_all(&home2).unwrap();
    let got = home2.join("stdin-bytes");
    k2_ptyd::launch::spawn_detached(&ptyd_bin(), &home2, &[]).expect("launch ptyd for home2");
    let paths = k2_ptyd::paths::Paths::from_home(&home2);
    let t = Instant::now();
    let mut old = loop {
        match k2_ptyd::client::Client::connect_owner(&paths, "previous-daemon", true) {
            Ok(c) => break c,
            Err(e) => {
                assert!(t.elapsed() < Duration::from_secs(5), "old owner: {e}");
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    };
    let sid = k2_core::session::SessionId::new().to_string();
    // The child prints, asks for the cursor position (a query the previous
    // daemon never answered: it had no Term), then records the first 6
    // bytes it receives on stdin.
    let script = format!("stty raw -echo; printf 'before-restart\\033[6n'; dd bs=1 count=6 of='{}' 2>/dev/null; sleep 60", got.display());
    let pid = match old
        .request(Request::Spawn {
            sid: sid.clone(),
            argv: vec!["/bin/sh".into(), "-c".into(), script],
            env: vec![("PATH".into(), "/usr/bin:/bin".into())],
            cwd: None,
            cols: 80,
            rows: 24,
            ring_bytes: None,
            meta: serde_json::json!({"session_id": sid, "key": "adopt-test", "program": "/bin/sh", "durable_args": ["-c", "x"], "label": "Adopted", "label_source": "Locked"}),
        })
        .unwrap()
    {
        Reply::Spawned { pid, .. } => pid,
        other => panic!("{other:?}"),
    };
    // Wait until the output is in ptyd's ring, then the old daemon exits.
    let t = Instant::now();
    loop {
        let Reply::Sessions { sessions } = old.request(Request::List).unwrap() else { panic!() };
        if sessions.iter().any(|s| s.sid == sid && s.ring_end >= 18) {
            break;
        }
        assert!(t.elapsed() < Duration::from_secs(5), "output never reached the ring");
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(old);
    assert!(alive(pid));

    // B0: list, don't end.
    let fresh = k2_core::terminal::ptyd::client::Ptyd::for_home(&home2);
    let scan = k2_core::terminal::ptyd::adopt::scan_with(&fresh);
    assert_eq!(scan.skipped, None);
    let info = scan.running.iter().find(|s| s.sid == sid).expect("B0 lists the surviving session").clone();
    assert_eq!(info.pid, pid);
    assert_eq!(info.meta["key"], "adopt-test");
    std::thread::sleep(Duration::from_millis(300));
    assert!(alive(pid), "B0 must never end a surviving session");

    // B1: adopt.
    let owner = fresh.current_owner().expect("B0 left an owner connection");
    let s = k2_core::terminal::daemon_pty::DaemonPtySession::adopt(&info, owner, Duration::from_secs(5)).expect("adopt");
    assert_eq!(s.child_pid(), Some(pid), "same agent process");
    assert_eq!(s.session_id.to_string(), sid, "same SessionId");
    assert_eq!(s.label(), "Adopted");
    assert_eq!(s.backend_name(), "ptyd");
    assert!(s.visible_text_rows().iter().any(|r| r.contains("before-restart")), "Term rebuilt by replay: {:?}", s.visible_text_rows());
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(std::fs::read(&got).unwrap_or_default(), Vec::<u8>::new(), "P75: the replayed query was not answered");
    // Live input after the switch reaches the child (and nothing preceded it).
    s.write(b"abcdef".to_vec());
    assert_eq!(wait_file(&got, Duration::from_secs(5)), "abcdef");

    // KeepAgents: detach lets go; kill/Drop leave the agent running.
    assert!(s.detach());
    s.kill();
    drop(s);
    std::thread::sleep(Duration::from_millis(300));
    assert!(alive(pid), "a detached session belongs to the next daemon");

    let owner = fresh.current_owner().expect("owner");
    assert_eq!(owner.request(Request::Shutdown { end_sessions: true }).unwrap(), Reply::Ok);
    let t = Instant::now();
    while alive(pid) {
        assert!(t.elapsed() < Duration::from_secs(5), "shutdown{{end_sessions}} ends it");
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = std::fs::remove_dir_all(&home2);
}

#[test]
fn ptyd_death_ends_sessions_without_spinning_and_ptyd_relaunches() {
    let _g = setup();
    let s = DaemonPtySession::spawn(cfg("sleep 60")).expect("spawn");
    let pid = s.child_pid().unwrap();
    let mut rx = s.subscribe_events();
    let ptyd = k2_core::terminal::ptyd::client::global().ptyd_pid().expect("ptyd pid");
    // SAFETY: killing the test's own ptyd.
    unsafe { libc::kill(ptyd, libc::SIGKILL) };
    let st = wait_child_exit(&mut rx, Duration::from_secs(10));
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(st.signal(), Some(libc::SIGHUP), "ptyd vanished → reported as a hang-up");
    }
    let t = Instant::now();
    while alive(pid) {
        assert!(t.elapsed() < Duration::from_secs(5), "the agent outlived its ptyd (master closed → SIGHUP)");
        std::thread::sleep(Duration::from_millis(20));
    }
    // P77: the closed stream must not spin the IO loop.
    let c0 = cpu_secs();
    std::thread::sleep(Duration::from_secs(1));
    let used = cpu_secs() - c0;
    assert!(used < 0.05, "IO loop spinning after EOF: {used:.3}s CPU in 1s");
    drop(s);
    // The next survive spawn launches a fresh ptyd.
    let s2 = DaemonPtySession::spawn(cfg("sleep 30")).expect("spawn after ptyd death");
    assert_eq!(s2.backend_name(), "ptyd");
    let ptyd2 = k2_core::terminal::ptyd::client::global().ptyd_pid().expect("new ptyd");
    assert_ne!(ptyd2, ptyd);
    s2.kill();
}

/// `k2-ptyd-test-agent` next to `k2-ptyd` (`cargo build -p k2-ptyd --bins`).
fn test_agent_bin() -> PathBuf {
    let bin = ptyd_bin().with_file_name("k2-ptyd-test-agent");
    assert!(bin.is_file(), "k2-ptyd-test-agent not found at {} — run `cargo build -p k2-ptyd --bins` first", bin.display());
    bin
}

/// Removes this test's debug-only knobs again, also on a panic, so no
/// other test in this binary inherits them.
struct KnobsOff(&'static [&'static str]);
impl Drop for KnobsOff {
    fn drop(&mut self) {
        for k in self.0 {
            std::env::remove_var(k);
        }
    }
}

/// Soak 10-30: a program that floods output from its first instant
/// overruns ptyd's ring before the spawn's FIRST attach lands. ptyd then
/// drops the queued `attached` reply and closes the stream; the spawn used
/// to fail on that ("ptyd closed the connection") and kill a live program.
/// It must retry like the IO loop's reattach and keep the program.
///
/// A 256 KiB ring and a 150 ms pause before the first attach (debug-only
/// knobs; the pause stands in for a loaded box) make the ring full and the
/// flood still running when the attach lands. Six sessions at once, each
/// flooding for one second by the clock, then printing `flood-done`. Every
/// spawn must succeed and every program survive; at least one session must
/// have overrun (on a loaded box not all of them do).
#[test]
fn a_flood_during_the_first_attach_never_fails_the_spawn() {
    let _g = setup();
    let agent = test_agent_bin();
    const KNOBS: &[&str] = &["K2_PTYD_TEST_RING_BYTES", "K2_PTYD_TEST_FIRST_ATTACH_DELAY_MS"];
    let off = KnobsOff(KNOBS);
    std::env::set_var("K2_PTYD_TEST_RING_BYTES", (256 * 1024).to_string());
    std::env::set_var("K2_PTYD_TEST_FIRST_ATTACH_DELAY_MS", "150");
    type Spawned = (PathBuf, Result<std::sync::Arc<DaemonPtySession>, std::io::Error>);
    let spawned: Vec<Spawned> = std::thread::scope(|sc| {
        let handles: Vec<_> = (0..6)
            .map(|i| {
                let dir = home().join(format!("first-attach-{i}"));
                let _ = std::fs::remove_dir_all(&dir);
                let agent = agent.clone();
                sc.spawn(move || {
                    // Through /bin/sh: the temp-HOME spawn guard only lets
                    // system binaries start directly.
                    let r = DaemonPtySession::spawn(cfg(&format!("exec '{}' flood-for '{}' 1000", agent.display(), dir.display())));
                    (dir, r)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("spawn thread")).collect()
    });
    drop(off);
    let failed: Vec<String> = spawned.iter().filter_map(|(d, r)| r.as_ref().err().map(|e| format!("{}: {e}", d.display()))).collect();
    assert!(failed.is_empty(), "spawn failed while the program flooded its first attach ({} of {}):\n{}", failed.len(), spawned.len(), failed.join("\n"));
    let sessions: Vec<_> = spawned.into_iter().map(|(d, r)| (d, r.expect("checked above"))).collect();
    let log = home().join(".k2/logs/ptyd.log");
    let mut overran = 0;
    for (dir, s) in &sessions {
        assert_eq!(s.backend_name(), "ptyd");
        let pid: i32 = wait_file(&dir.join("pid"), Duration::from_secs(10)).trim().parse().expect("agent pid");
        assert_eq!(s.child_pid(), Some(pid), "the session holds the flooding program");
        let t = Instant::now();
        while !s.visible_text_rows().iter().any(|r| r.starts_with("flood-done")) {
            assert!(t.elapsed() < Duration::from_secs(60), "the flood's last line never reached the screen: {:?}", s.visible_text_rows());
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(alive(pid), "the flooding program died");
        assert!(s.is_child_alive(), "the session ended during the flood");
        let ptyd_log = std::fs::read_to_string(&log).unwrap_or_else(|e| panic!("{}: {e}", log.display()));
        let sid = s.session_id.to_string();
        if ptyd_log.contains(&format!("overrun sid={sid}")) {
            overran += 1;
        }
        assert!(!ptyd_log.contains(&format!("exited sid={sid}")), "session {sid} ended during the flood");
    }
    // A loaded box floods slower, so not every session overruns; one is
    // enough to prove the point (every spawn above succeeded).
    eprintln!("first-attach flood: {overran} of {} session(s) overran", sessions.len());
    assert!(overran >= 1, "no session overran; the test proves nothing");
    for (_, s) in sessions {
        s.kill();
    }
}
