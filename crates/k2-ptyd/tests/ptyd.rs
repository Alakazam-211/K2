//! k2-ptyd integration tests (PRD §12 "ptyd" 1–7). Each test runs the real
//! binary against its own temp K2 home. Nothing here touches `~/.k2`.

#![cfg(unix)]

use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use k2_ptyd::client::{Client, ConnectError, Conn};
use k2_ptyd::paths::Paths;
use k2_ptyd::proto::{Frame, Reply, Request, Role, SessionState, Sig, SigTarget};

const PTYD: &str = env!("CARGO_BIN_EXE_k2-ptyd");
const AGENT: &str = env!("CARGO_BIN_EXE_k2-ptyd-test-agent");

static N: AtomicUsize = AtomicUsize::new(0);

/// A temp K2 home. Short (`/tmp/...`) unless `long`.
struct Home {
    root: PathBuf,
    home: PathBuf,
}

impl Home {
    fn new(long: bool) -> Home {
        let n = N.fetch_add(1, Ordering::SeqCst);
        let root = if long {
            std::env::temp_dir().join(format!("k2-ptyd-it-{}-{}-{}", std::process::id(), n, "x".repeat(70)))
        } else {
            PathBuf::from(format!("/tmp/k2pt-{}-{}", std::process::id(), n))
        };
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join(".k2");
        std::fs::create_dir_all(&home).unwrap();
        Home { root, home }
    }
    fn paths(&self) -> Paths {
        Paths::resolve(&self.home, Path::new("/tmp"))
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn base_cmd(home: &Home) -> Command {
    let mut c = Command::new(PTYD);
    c.env_clear()
        .env("K2_HOME", &home.home)
        .env("HOME", &home.root)
        .env("PATH", "/usr/bin:/bin")
        .env("TMPDIR", "/tmp");
    c
}

/// `k2-ptyd --serve` in the foreground, ready once the key exists and the
/// socket accepts.
struct Ptyd {
    child: Child,
}

impl Ptyd {
    fn start(home: &Home, extra: &[(&str, &str)]) -> Ptyd {
        let mut c = base_cmd(home);
        c.arg("--serve").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
        for (k, v) in extra {
            c.env(k, v);
        }
        let child = c.spawn().expect("spawn k2-ptyd");
        wait_listening(&home.paths());
        Ptyd { child }
    }
    fn pid(&self) -> i32 {
        self.child.id() as i32
    }
    fn wait_exit(&mut self, within: Duration) -> std::process::ExitStatus {
        let t = Instant::now();
        loop {
            if let Some(st) = self.child.try_wait().unwrap() {
                return st;
            }
            assert!(t.elapsed() < within, "k2-ptyd did not exit within {within:?}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Ptyd {
    fn drop(&mut self) {
        if self.child.try_wait().unwrap().is_none() {
            // SAFETY: killing our own test child.
            unsafe { libc::kill(self.child.id() as i32, libc::SIGKILL) };
            let _ = self.child.wait();
        }
    }
}

fn wait_listening(p: &Paths) {
    let t = Instant::now();
    loop {
        if p.key.exists() && std::os::unix::net::UnixStream::connect(p.client_sock()).is_ok() {
            return;
        }
        assert!(t.elapsed() < Duration::from_secs(10), "ptyd never listened at {:?}", p.client_sock());
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn owner(home: &Home) -> Client {
    Client::connect_owner(&home.paths(), "test-0", true).expect("owner connect")
}

fn agent_env(home: &Home) -> Vec<(String, String)> {
    vec![
        ("PATH".into(), "/usr/bin:/bin".into()),
        ("HOME".into(), home.root.to_string_lossy().into_owned()),
        ("TERM".into(), "xterm-256color".into()),
    ]
}

fn spawn_agent(c: &mut Client, home: &Home, sid: &str, args: &[&str], ring: Option<u64>) -> i32 {
    let mut argv = vec![AGENT.to_string()];
    argv.extend(args.iter().map(|s| s.to_string()));
    match c
        .request(Request::Spawn {
            sid: sid.into(),
            argv,
            env: agent_env(home),
            cwd: Some(home.root.to_string_lossy().into_owned()),
            cols: 100,
            rows: 30,
            ring_bytes: ring,
            meta: serde_json::json!({"label": "test"}),
        })
        .unwrap()
    {
        Reply::Spawned { pid, .. } => pid,
        other => panic!("spawn failed: {other:?}"),
    }
}

fn wait_exited(c: &mut Client, sid: &str, within: Duration) -> (Option<i32>, Option<i32>) {
    c.conn.set_timeout(Some(within)).unwrap();
    loop {
        match c.next_event().expect("event before timeout") {
            Reply::Exited { sid: s, code, signal, .. } if s == sid => {
                c.conn.set_timeout(Some(Duration::from_secs(10))).unwrap();
                return (code, signal);
            }
            _ => continue,
        }
    }
}

/// Read a data stream until `needle` shows up (or panic after `within`).
fn read_until(r: &mut impl Read, needle: &str, within: Duration) -> String {
    let t = Instant::now();
    let mut acc = Vec::new();
    let mut buf = [0u8; 4096];
    while t.elapsed() < within {
        match r.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                acc.extend_from_slice(&buf[..n]);
                if String::from_utf8_lossy(&acc).contains(needle) {
                    return String::from_utf8_lossy(&acc).into_owned();
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => continue,
            Err(e) => panic!("read: {e}"),
        }
    }
    panic!("never saw {needle:?}; got {:?}", String::from_utf8_lossy(&acc));
}

fn alive(pid: i32) -> bool {
    // SAFETY: probing a pid.
    unsafe { libc::kill(pid, 0) == 0 }
}

fn wait_gone(pid: i32, within: Duration) {
    let t = Instant::now();
    while alive(pid) {
        assert!(t.elapsed() < within, "pid {pid} still alive after {within:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

// ------------------------------------------------------------------ 1

#[test]
fn handshake_refusals_owner_lease_and_takeover() {
    let home = Home::new(false);
    let _p = Ptyd::start(&home, &[]);
    let paths = home.paths();

    // bad_mac: a wrong key.
    let mut c = Conn::connect(&paths.client_sock()).unwrap();
    match c.handshake(&[9u8; 32], "x", Role::Owner, false) {
        Err(ConnectError::Refused { code, .. }) => assert_eq!(code, "bad_mac"),
        other => panic!("{other:?}"),
    }

    // proto: a hello for a protocol ptyd doesn't speak.
    let key = k2_ptyd::auth::read_key(&paths.key).unwrap();
    let mut c = Conn::connect(&paths.client_sock()).unwrap();
    let Reply::Challenge { nonce, .. } = c.recv().unwrap().body else { panic!() };
    let nonce = k2_ptyd::auth::unhex(&nonce).unwrap();
    c.send(&Frame::new(
        None,
        Request::Hello { proto: 99, client: "t".into(), client_version: "v".into(), mac: k2_ptyd::auth::mac(&key, &nonce, "v"), takeover: false, role: Role::Owner },
    ))
    .unwrap();
    match c.recv().unwrap().body {
        Reply::Refused { code, .. } => assert_eq!(code, "proto"),
        other => panic!("{other:?}"),
    }

    // One owner; busy without takeover; takeover evicts.
    let mut first = Client::connect_owner(&paths, "a", false).unwrap();
    match Client::connect_owner(&paths, "b", false) {
        Err(ConnectError::Refused { code, .. }) => assert_eq!(code, "busy"),
        Err(e) => panic!("{e}"),
        Ok(_) => panic!("second owner without takeover must be refused"),
    }
    let mut second = Client::connect_owner(&paths, "c", true).unwrap();
    assert_eq!(first.next_event().unwrap(), Reply::Evicted);
    assert!(first.next_event().is_err(), "evicted owner is disconnected");
    assert!(matches!(second.request(Request::Ping).unwrap(), Reply::Pong { .. }));

    // Unknown `t` → error unknown_message, connection stays up.
    second.conn.send(&serde_json::json!({"id": 77, "t": "frobnicate"})).unwrap();
    let f = second.conn.recv().unwrap();
    assert_eq!(f.id, Some(77));
    assert_eq!(f.body, Reply::Error { code: "unknown_message".into(), detail: String::new() });
    assert!(matches!(second.request(Request::Ping).unwrap(), Reply::Pong { .. }));
}

#[test]
fn oversize_frame_closes_the_connection() {
    let home = Home::new(false);
    let _p = Ptyd::start(&home, &[]);
    let mut c = Conn::connect(&home.paths().client_sock()).unwrap();
    c.set_timeout(Some(Duration::from_secs(5))).unwrap();
    let _ = c.recv().unwrap(); // challenge
    c.stream.write_all(&(2u32 << 20).to_be_bytes()).unwrap();
    let mut b = [0u8; 16];
    assert_eq!(c.stream.read(&mut b).unwrap(), 0, "ptyd closes on frame_too_large");
}

// ------------------------------------------------------------------ 2

#[test]
fn spawned_child_has_its_own_session_ctty_clean_fds_default_signals_and_exact_env() {
    let home = Home::new(false);
    let _p = Ptyd::start(&home, &[]);
    let mut c = owner(&home);
    let out = home.root.join("probe.json");
    let pid = spawn_agent(&mut c, &home, "probe", &["probe", out.to_str().unwrap()], None);
    let mut s = c.attach("probe", 0).unwrap();
    read_until(&mut s, "PROBE_DONE", Duration::from_secs(10));
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(v["pid"], pid);
    assert_eq!(v["sid"], pid, "setsid: session leader");
    assert_eq!(v["pgid"], pid, "own process group");
    assert_eq!(v["tpgid"], pid, "slave is the controlling tty, child in the foreground");
    assert_eq!(v["isatty"], true);
    assert_eq!(v["fds"], serde_json::json!([0, 1, 2]), "no fd but the slave on 0/1/2");
    assert_eq!(v["ignored"], serde_json::json!([]), "ptyd ignores SIGPIPE/SIGHUP; the child must not inherit that");
    assert_eq!(v["handled"], serde_json::json!([]));
    assert_eq!(v["blocked"], serde_json::json!([]));
    assert_eq!(v["iutf8"], true);
    let cwd = std::fs::canonicalize(&home.root).unwrap();
    assert_eq!(std::fs::canonicalize(v["cwd"].as_str().unwrap()).unwrap(), cwd);
    let mut want = agent_env(&home);
    want.sort();
    let got: Vec<(String, String)> = serde_json::from_value(v["env"].clone()).unwrap();
    assert_eq!(got, want, "the child env is exactly the sent env; nothing from ptyd leaks");
    // ptyd itself keeps its own sockets; none reached the child (checked above).
    assert!(alive(pid));
}

// ------------------------------------------------------------------ 3

fn pattern_at(offset: u64, len: usize) -> Vec<u8> {
    (0..len).map(|i| b'a' + ((offset as usize + i) % 26) as u8).collect()
}

#[test]
fn masters_are_always_read_and_attach_is_byte_exact_from_any_offset() {
    let home = Home::new(false);
    let _p = Ptyd::start(&home, &[]);
    let mut c = owner(&home);
    let t = Instant::now();
    spawn_agent(&mut c, &home, "burst", &["burst", "50"], None);
    let (code, signal) = wait_exited(&mut c, "burst", Duration::from_secs(10));
    assert_eq!((code, signal), (Some(0), None));
    assert!(t.elapsed() < Duration::from_secs(5), "50 MiB with no reader took {:?} (P21: < 5 s)", t.elapsed());

    let Reply::Sessions { sessions } = c.request(Request::List).unwrap() else { panic!() };
    let info = sessions.iter().find(|s| s.sid == "burst").unwrap();
    assert_eq!(info.ring_end, 50 << 20);
    assert_eq!(info.ring_end - info.ring_start, 4 << 20, "default 4 MiB ring");
    assert!(matches!(info.state, SessionState::Exited { code: Some(0), .. }));

    for from in [0, info.ring_start + 12_345, info.ring_end - 10] {
        let s = c.attach("burst", from).unwrap();
        let start = from.max(info.ring_start);
        assert_eq!((s.start, s.end), (start, info.ring_end));
        let mut s = s;
        let mut got = Vec::new();
        s.read_to_end(&mut got).unwrap(); // exited session: EOF after the backlog
        assert_eq!(got.len() as u64, info.ring_end - start, "no duplication, no loss");
        assert!(got == pattern_at(start, got.len()), "byte-exact from offset {start}");
    }
}

#[test]
fn ring_wrap_past_an_unread_stream_is_an_overrun() {
    let home = Home::new(false);
    let _p = Ptyd::start(&home, &[]);
    let mut c = owner(&home);
    // Tiny ring, big burst, and a stream nobody reads: the socket buffer
    // fills, the ring wraps past the stream's unsent cursor.
    spawn_agent(&mut c, &home, "ov2", &["burst", "64"], Some(256 << 10));
    let mut s2 = c.attach("ov2", 0).unwrap();
    // Read nothing until the overrun event arrives.
    c.conn.set_timeout(Some(Duration::from_secs(15))).unwrap();
    let lost = loop {
        match c.next_event().unwrap() {
            Reply::Overrun { sid, lost_bytes } if sid == "ov2" => break lost_bytes,
            _ => continue,
        }
    };
    assert!(lost > 0);
    let mut sink = Vec::new();
    s2.read_to_end(&mut sink).unwrap(); // stream was closed by ptyd
    // Re-attach recovers from ring_start (once the burst is over, so the
    // ring stops moving between `list` and `attach`).
    let t = Instant::now();
    let info = loop {
        let Reply::Sessions { sessions } = c.request(Request::List).unwrap() else { panic!() };
        let info = sessions.into_iter().find(|x| x.sid == "ov2").unwrap();
        if matches!(info.state, SessionState::Exited { .. }) {
            break info;
        }
        assert!(t.elapsed() < Duration::from_secs(30), "burst never finished");
        std::thread::sleep(Duration::from_millis(50));
    };
    let r = c.attach("ov2", 0).unwrap();
    assert_eq!(r.start, info.ring_start);
}

// ------------------------------------------------------------------ input

#[test]
fn input_reaches_the_master_in_order_and_a_second_attach_closes_the_first() {
    let home = Home::new(false);
    let _p = Ptyd::start(&home, &[]);
    let mut c = owner(&home);
    spawn_agent(&mut c, &home, "echo", &["echo"], None);
    let mut a = c.attach("echo", 0).unwrap();
    read_until(&mut a, "READY", Duration::from_secs(10));
    a.write_all(b"one\rtwo\rthree\r").unwrap();
    let seen = read_until(&mut a, "got:three", Duration::from_secs(10));
    let (i1, i2, i3) = (seen.find("got:one").unwrap(), seen.find("got:two").unwrap(), seen.find("got:three").unwrap());
    assert!(i1 < i2 && i2 < i3, "in order: {seen:?}");
    let mut b = c.attach("echo", u64::MAX).unwrap();
    let mut rest = Vec::new();
    a.read_to_end(&mut rest).unwrap(); // first stream closed by the second attach
    b.write_all(b"four\r").unwrap();
    read_until(&mut b, "got:four", Duration::from_secs(10));
}

// ------------------------------------------------------------------ 4

#[test]
fn kill_escalates_hup_then_kill_and_returns_after_the_reap() {
    let home = Home::new(false);
    let _p = Ptyd::start(&home, &[]);
    let mut c = owner(&home);
    let pid = spawn_agent(&mut c, &home, "stubborn", &["hup-ignore"], None);
    let mut s = c.attach("stubborn", 0).unwrap();
    read_until(&mut s, "READY", Duration::from_secs(10));
    let t = Instant::now();
    assert_eq!(c.request(Request::Kill { sid: "stubborn".into(), grace_ms: Some(300) }).unwrap(), Reply::Ok);
    assert!(t.elapsed() >= Duration::from_millis(250), "waited out the grace for a HUP-ignoring child");
    assert!(!alive(pid), "reaped before the reply");
    let (_, signal) = wait_exited(&mut c, "stubborn", Duration::from_secs(5));
    assert_eq!(signal, Some(libc::SIGKILL));
    // Idempotent.
    assert_eq!(c.request(Request::Kill { sid: "stubborn".into(), grace_ms: None }).unwrap(), Reply::Ok);

    let pid = spawn_agent(&mut c, &home, "polite", &["sleep"], None);
    let mut s = c.attach("polite", 0).unwrap();
    read_until(&mut s, "READY", Duration::from_secs(10));
    assert_eq!(c.request(Request::Kill { sid: "polite".into(), grace_ms: Some(5000) }).unwrap(), Reply::Ok);
    assert!(!alive(pid));
    let (_, signal) = wait_exited(&mut c, "polite", Duration::from_secs(5));
    assert_eq!(signal, Some(libc::SIGHUP));
}

#[test]
fn exit_codes_signals_and_ack_exit() {
    let home = Home::new(false);
    let _p = Ptyd::start(&home, &[]);
    let mut c = owner(&home);
    spawn_agent(&mut c, &home, "e7", &["exit", "7"], None);
    assert_eq!(wait_exited(&mut c, "e7", Duration::from_secs(10)), (Some(7), None));
    let pid = spawn_agent(&mut c, &home, "t", &["sleep"], None);
    let mut s = c.attach("t", 0).unwrap();
    read_until(&mut s, "READY", Duration::from_secs(10));
    assert_eq!(c.request(Request::Signal { sid: "t".into(), sig: Sig::Term, target: SigTarget::Group }).unwrap(), Reply::Ok);
    assert_eq!(wait_exited(&mut c, "t", Duration::from_secs(10)), (None, Some(libc::SIGTERM)));
    assert!(!alive(pid));
    // Stream of an exited session ends with EOF after the backlog.
    let mut rest = Vec::new();
    s.read_to_end(&mut rest).unwrap();
    assert_eq!(c.request(Request::AckExit { sid: "e7".into() }).unwrap(), Reply::Ok);
    assert_eq!(c.request(Request::AckExit { sid: "e7".into() }).unwrap(), Reply::Error { code: "unknown_session".into(), detail: String::new() });
    // dup sid refused, including against an exited-unacked record.
    let r = c
        .request(Request::Spawn { sid: "t".into(), argv: vec![AGENT.into(), "sleep".into()], env: agent_env(&home), cwd: None, cols: 80, rows: 24, ring_bytes: None, meta: serde_json::Value::Null })
        .unwrap();
    assert_eq!(r, Reply::Error { code: "dup_sid".into(), detail: String::new() });
}

#[test]
fn resize_and_foreground_pgrp() {
    let home = Home::new(false);
    let _p = Ptyd::start(&home, &[]);
    let mut c = owner(&home);
    let pid = spawn_agent(&mut c, &home, "r", &["sleep"], None);
    let mut s = c.attach("r", 0).unwrap();
    read_until(&mut s, "READY", Duration::from_secs(10));
    assert_eq!(c.request(Request::Resize { sid: "r".into(), cols: 132, rows: 43 }).unwrap(), Reply::Ok);
    let Reply::Sessions { sessions } = c.request(Request::List).unwrap() else { panic!() };
    assert_eq!((sessions[0].cols, sessions[0].rows), (132, 43));
    assert_eq!(c.request(Request::FgPgrp { sid: "r".into() }).unwrap(), Reply::Pgrp { pgid: Some(pid) });
}

// ------------------------------------------------------------------ 5

#[test]
fn launcher_detaches_reaps_and_idle_exit_leaves_no_zombie() {
    let home = Home::new(false);
    let mut c = base_cmd(&home);
    c.arg("--launch").env("K2_PTYD_TEST_IDLE_SECS", "2");
    let st = c.status().unwrap(); // reaps the launcher
    assert!(st.success(), "launcher exit {st:?}");
    let paths = home.paths();
    wait_listening(&paths);
    let ptyd_pid: i32 = std::fs::read_to_string(&paths.lock).unwrap().trim().parse().unwrap();
    assert!(alive(ptyd_pid), "ptyd runs");
    // P6: ptyd is never the caller's child. Observed through our OWN
    // process table: waitpid on a pid that is not our child is ECHILD.
    // (No `ps`/`/proc` of a foreign process: K2 compute's job wall runs
    // with ProtectProc=invisible, and ptyd is non-dumpable.)
    let mut status = 0;
    // SAFETY: WNOHANG probe on a pid; never reaps anything we didn't start.
    let r = unsafe { libc::waitpid(ptyd_pid, &mut status, libc::WNOHANG) };
    assert_eq!(
        (r, std::io::Error::last_os_error().raw_os_error()),
        (-1, Some(libc::ECHILD)),
        "ptyd is never the caller's child (P6)"
    );
    // A second launch while one runs exits 3.
    let st = base_cmd(&home).arg("--launch").status().unwrap();
    assert_eq!(st.code(), Some(3));
    // Idle: no sessions, no owner → exits; socket and key removed.
    wait_gone(ptyd_pid, Duration::from_secs(10));
    assert!(!paths.sock.exists() && !paths.key.exists());
}

#[test]
fn second_serve_exits_3_naming_the_pid() {
    let home = Home::new(false);
    let p = Ptyd::start(&home, &[]);
    let out = base_cmd(&home).arg("--serve").output().unwrap();
    assert_eq!(out.status.code(), Some(3));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains(&p.pid().to_string()), "names the running pid: {err}");
}

#[test]
fn long_home_uses_the_short_socket_fallback() {
    let home = Home::new(true);
    let paths = Paths::resolve(&home.home, Path::new("/tmp"));
    assert!(paths.uses_fallback(), "test home must be long enough: {:?}", home.home);
    let _p = Ptyd::start(&home, &[]);
    assert_eq!(std::fs::read_to_string(&paths.endpoint_file).unwrap(), paths.sock.to_string_lossy());
    let mut c = owner(&home);
    assert!(matches!(c.request(Request::Ping).unwrap(), Reply::Pong { .. }));
}

#[test]
fn shutdown_semantics_and_sigterm_end_sessions() {
    let home = Home::new(false);
    let mut p = Ptyd::start(&home, &[]);
    let mut c = owner(&home);
    let pid = spawn_agent(&mut c, &home, "a", &["hup-ignore"], None);
    let mut s = c.attach("a", 0).unwrap();
    read_until(&mut s, "READY", Duration::from_secs(10));
    assert_eq!(c.request(Request::Shutdown { end_sessions: false }).unwrap(), Reply::Error { code: "sessions_running".into(), detail: "1".into() });
    assert_eq!(c.request(Request::Shutdown { end_sessions: true }).unwrap(), Reply::Ok);
    assert!(!alive(pid), "HUP-ignoring agent SIGKILLed after the 2 s grace");
    assert!(p.wait_exit(Duration::from_secs(5)).success());
    assert!(!home.paths().sock.exists());

    let home = Home::new(false);
    let mut p = Ptyd::start(&home, &[]);
    let mut c = owner(&home);
    let pid = spawn_agent(&mut c, &home, "b", &["sleep"], None);
    let mut s = c.attach("b", 0).unwrap();
    read_until(&mut s, "READY", Duration::from_secs(10));
    // SAFETY: signalling our test child.
    unsafe { libc::kill(p.pid(), libc::SIGTERM) };
    assert!(p.wait_exit(Duration::from_secs(5)).success());
    wait_gone(pid, Duration::from_secs(2));
}

#[test]
fn orphan_timeout_ends_sessions_after_the_minimum_120s() {
    let home = Home::new(false);
    // A smaller request is clamped up to the 120 s minimum.
    let mut p = Ptyd::start(&home, &[("K2_PTYD_ORPHAN_SECS", "5")]);
    let pid = {
        let mut c = owner(&home);
        let pid = spawn_agent(&mut c, &home, "o", &["sleep"], None);
        let mut s = c.attach("o", 0).unwrap();
        read_until(&mut s, "READY", Duration::from_secs(10));
        pid
    }; // owner gone
    std::thread::sleep(Duration::from_secs(60));
    assert!(alive(pid), "not before half the minimum");
    // An owner arriving past half the timeout is warned.
    {
        std::thread::sleep(Duration::from_secs(1));
        let mut c = owner(&home);
        match c.next_event().unwrap() {
            Reply::OrphanWarning { seconds_left } => assert!(seconds_left <= 60, "{seconds_left}"),
            other => panic!("expected orphan_warning, got {other:?}"),
        }
    } // this owner leaves: the clock restarts now
    std::thread::sleep(Duration::from_secs(110));
    assert!(alive(pid), "an owner attach reset the clock");
    std::thread::sleep(Duration::from_secs(15));
    wait_gone(pid, Duration::from_secs(5));
    assert!(p.wait_exit(Duration::from_secs(5)).success());
}

// ------------------------------------------------------------------ fds

#[test]
fn held_listeners_queue_connections_and_fds_round_trip() {
    let home = Home::new(false);
    let _p = Ptyd::start(&home, &[]);
    let mut c = owner(&home);
    let path = home.root.join("cell.sock");
    let r = c.request(Request::HoldUnixListener { name: "cell:a".into(), path: path.to_string_lossy().into(), mode: 0o600, owner_uid: None }).unwrap();
    assert_eq!(r, Reply::Ok);
    let fd = c.take_fd().expect("listener fd rides on the reply");
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }
    // A client connects while nobody accepts: it queues in the backlog.
    let mut early = std::os::unix::net::UnixStream::connect(&path).unwrap();
    early.write_all(b"queued").unwrap();
    drop(fd); // the daemon "restarts": its copy is gone; ptyd still holds one
    // Take it again (idempotent hold also returns the same socket).
    assert_eq!(c.request(Request::TakeFd { name: "cell:a".into() }).unwrap(), Reply::Ok);
    let fd = c.take_fd().unwrap();
    let l = std::os::unix::net::UnixListener::from(fd);
    let (mut acc, _) = l.accept().unwrap();
    let mut b = [0u8; 6];
    acc.read_exact(&mut b).unwrap();
    assert_eq!(&b, b"queued", "the connection made while no daemon held the socket was served");
    assert_eq!(c.request(Request::ReleaseFd { name: "cell:a".into() }).unwrap(), Reply::Ok);
    assert!(!path.exists(), "release unlinks the path");
    assert_eq!(c.request(Request::TakeFd { name: "cell:a".into() }).unwrap(), Reply::Error { code: "unknown".into(), detail: String::new() });

    // hold_fd / take_fd with a TCP listener (the main listener, P50).
    let tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = tcp.local_addr().unwrap();
    assert_eq!(c.request_with_fd(Request::HoldFd { name: "main".into() }, &tcp).unwrap(), Reply::Ok);
    drop(tcp);
    assert_eq!(c.request(Request::TakeFd { name: "main".into() }).unwrap(), Reply::Ok);
    let back = std::net::TcpListener::from(c.take_fd().unwrap());
    assert_eq!(back.local_addr().unwrap(), addr);
    assert!(back.as_raw_fd() > 2);
}

// ------------------------------------------------------------------ 7

#[test]
fn no_secret_reaches_the_log_or_any_ptyd_file() {
    let home = Home::new(false);
    let canary = "k2-canary-7f3a9d";
    let _p = Ptyd::start(&home, &[]);
    let mut c = owner(&home);
    let mut env = agent_env(&home);
    env.push(("K2_HOOK_TOKEN".into(), canary.into()));
    let r = c
        .request(Request::Spawn {
            sid: "sec".into(),
            argv: vec![AGENT.into(), "exit".into(), "0".into()],
            env,
            cwd: None,
            cols: 80,
            rows: 24,
            ring_bytes: None,
            meta: serde_json::json!({"label": "x"}),
        })
        .unwrap();
    assert!(matches!(r, Reply::Spawned { .. }), "{r:?}");
    wait_exited(&mut c, "sec", Duration::from_secs(10));
    let mut files = Vec::new();
    walk(&home.home, &mut files);
    assert!(files.iter().any(|f| f.ends_with("ptyd.log")), "the log exists: {files:?}");
    for f in files {
        let bytes = std::fs::read(&f).unwrap_or_default();
        assert!(!String::from_utf8_lossy(&bytes).contains(canary), "canary leaked into {}", f.display());
    }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        let ft = e.file_type().unwrap();
        if ft.is_dir() {
            walk(&p, out);
        } else if ft.is_file() {
            out.push(p);
        }
    }
}

#[test]
fn launch_env_is_the_allowlist_only() {
    let home = Home::new(false);
    std::env::set_var("K2_HOOK_TOKEN", "leak-me-not");
    std::env::set_var("NOTIFY_SOCKET", "/run/leak");
    let r = k2_ptyd::launch::spawn_detached(Path::new(PTYD), &home.home, &[("K2_PTYD_ORPHAN_SECS", "900")]);
    std::env::remove_var("K2_HOOK_TOKEN");
    std::env::remove_var("NOTIFY_SOCKET");
    r.unwrap();
    let paths = home.paths();
    wait_listening(&paths);
    let pid: i32 = std::fs::read_to_string(&paths.lock).unwrap().trim().parse().unwrap();
    let mut c = owner(&home);
    check_live_env(&mut c, pid, &home.home);
    assert_eq!(c.request(Request::Shutdown { end_sessions: true }).unwrap(), Reply::Ok);
    wait_gone(pid, Duration::from_secs(5));
}

/// The launch env, derived from the allowlist itself: strict, and
/// independent of whatever the test runner's own environment carries.
#[test]
fn launch_env_is_exactly_the_allowlisted_subset() {
    use std::ffi::OsString;
    let parent: Vec<(OsString, OsString)> = vec![
        ("PATH".into(), "/usr/bin".into()),
        ("LC_ALL".into(), "C.UTF-8".into()),
        ("K2_HOME".into(), "/parent/home/must/not/win".into()),
        ("K2_HOOK_TOKEN".into(), "leak".into()),
        ("NOTIFY_SOCKET".into(), "/run/leak".into()),
        ("LISTEN_FDS".into(), "3".into()),
        ("K2_COMPUTE_JOB".into(), "node-noise".into()),
        ("XDG_CONFIG_HOME".into(), "/x".into()),
    ];
    let env = k2_ptyd::launch::launch_env(parent, Path::new("/h/.k2"), &[("K2_PTYD_ORPHAN_SECS", "900"), ("K2_HOOK_SOCK", "nope")]);
    let want: Vec<(OsString, OsString)> = vec![
        ("K2_HOME".into(), "/h/.k2".into()),
        ("K2_PTYD_ORPHAN_SECS".into(), "900".into()),
        ("LC_ALL".into(), "C.UTF-8".into()),
        ("PATH".into(), "/usr/bin".into()),
    ];
    assert_eq!(env, want);
    for (k, _) in &env {
        assert!(k2_ptyd::env_allowed(k.to_str().unwrap()), "{k:?} is not on the allowlist");
    }
}

/// Live check of a launched ptyd's environment and dumpability, asked of
/// ptyd itself over its control socket (`ping` → `env_keys`, `dumpable`):
/// it works under K2 compute's `ProtectProc=invisible`, where another
/// process's `/proc` is hidden. Plus, on macOS (debug builds have no
/// PT_DENY_ATTACH), the values through `ps eww`.
fn check_live_env(c: &mut Client, pid: i32, home: &Path) {
    let Reply::Pong { env_keys, dumpable, .. } = c.request(Request::Ping).unwrap() else { panic!("ping") };
    assert!(!env_keys.is_empty(), "ptyd reports its env names");
    for k in &env_keys {
        assert!(k2_ptyd::env_allowed(k), "{k} leaked into ptyd's env (P5)");
    }
    assert!(env_keys.iter().any(|k| k == "K2_HOME"), "K2_HOME present: {env_keys:?}");
    if cfg!(target_os = "linux") {
        assert_eq!(dumpable, Some(false), "ptyd must be non-dumpable (S-3/P71)");
    } else {
        let out = Command::new("ps").args(["eww", "-o", "command=", "-p", &pid.to_string()]).output().unwrap();
        let env = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(env.contains(&format!("K2_HOME={}", home.display())), "sanity: env readable: {env}");
        assert!(!env.contains("leak-me-not") && !env.contains("NOTIFY_SOCKET"), "{env}");
    }
}

// ------------------------------------------------------------------ 6 (S7)

/// `seq=<n> ` tokens of a stream, in order.
fn seq_numbers(bytes: &[u8]) -> Vec<u64> {
    String::from_utf8_lossy(bytes).split_whitespace().filter_map(|t| t.strip_prefix("seq=")?.parse().ok()).collect()
}

fn assert_consecutive(seqs: &[u64], what: &str) {
    assert!(seqs.len() > 10, "{what}: too few seq lines: {seqs:?}");
    let bad: Vec<_> = seqs.windows(2).filter(|w| w[1] != w[0] + 1).collect();
    assert!(bad.is_empty(), "{what}: seq not consecutive at {bad:?}");
}

/// Read a data stream until `acc` holds at least `n` more bytes.
fn read_more(r: &mut impl Read, acc: &mut Vec<u8>, n: usize, within: Duration) {
    let want = acc.len() + n;
    let t = Instant::now();
    let mut buf = [0u8; 4096];
    while acc.len() < want {
        assert!(t.elapsed() < within, "stream stalled at {} bytes (wanted {want})", acc.len());
        match r.read(&mut buf) {
            Ok(0) => panic!("stream closed at {} bytes (wanted {want})", acc.len()),
            Ok(k) => acc.extend_from_slice(&buf[..k]),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => continue,
            Err(e) => panic!("read: {e}"),
        }
    }
}

fn session<'a>(list: &'a [k2_ptyd::proto::SessionInfo], sid: &str) -> &'a k2_ptyd::proto::SessionInfo {
    list.iter().find(|s| s.sid == sid).unwrap_or_else(|| panic!("no session {sid} in {list:?}"))
}

fn list(c: &mut Client) -> Vec<k2_ptyd::proto::SessionInfo> {
    match c.request(Request::List).unwrap() {
        Reply::Sessions { sessions } => sessions,
        other => panic!("list: {other:?}"),
    }
}

fn sh(c: &mut Client, home: &Home, sid: &str, script: &str) -> i32 {
    match c
        .request(Request::Spawn {
            sid: sid.into(),
            argv: vec!["/bin/sh".into(), "-c".into(), script.into()],
            env: agent_env(home),
            cwd: None,
            cols: 80,
            rows: 24,
            ring_bytes: None,
            meta: serde_json::json!({"label": sid}),
        })
        .unwrap()
    {
        Reply::Spawned { pid, .. } => pid,
        other => panic!("spawn {sid}: {other:?}"),
    }
}

/// The owner connection ends right after `ok` (the exec closes it).
fn assert_dropped(c: &mut Client) {
    // macOS refuses setsockopt on a socket the peer already closed (EINVAL):
    // that is the drop this asserts.
    match c.conn.set_timeout(Some(Duration::from_secs(10))) {
        Ok(()) => {}
        Err(e) if e.raw_os_error() == Some(libc::EINVAL) => {}
        Err(e) => panic!("set_timeout: {e}"),
    }
    loop {
        match c.next_event() {
            Ok(Reply::OrphanWarning { .. }) | Ok(Reply::Exited { .. }) => continue,
            Ok(other) => panic!("expected the connection to drop after the upgrade, got {other:?}"),
            Err(_) => return,
        }
    }
}

fn probe_fds_after(c: &mut Client, home: &Home, sid: &str) -> serde_json::Value {
    let out = home.root.join(format!("{sid}.json"));
    spawn_agent(c, home, sid, &["probe", out.to_str().unwrap()], None);
    let mut pr = c.attach(sid, 0).unwrap();
    read_until(&mut pr, "PROBE_DONE", Duration::from_secs(10));
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    v["fds"].clone()
}

/// P63 (§7.10): `upgrade` execs the new image in place. Same ptyd pid, same
/// children, same boot id and key; rings carry over byte-exact (a stream
/// reattached at its offset continues with no gap and no duplicate); held
/// fds are still held; an exited-unacked record keeps its code; a child
/// that exits after the upgrade gets its exit code delivered; the new image
/// is non-dumpable again and leaks no inherited fd to a new child.
#[test]
fn upgrade_execs_in_place_keeping_pid_children_rings_and_fds() {
    let home = Home::new(false);
    let mut p = Ptyd::start(&home, &[("K2_PTYD_TEST_VERSION", "0.0.1")]);
    let ptyd_pid = p.pid();
    let mut c = owner(&home);
    assert_eq!(c.hello.ptyd_version, "0.0.1", "the test knob makes this ptyd look old");
    let boot = c.hello.boot_id.clone();
    let key_before = std::fs::read(&home.paths().key).unwrap();

    // An exited-unacked record with a known ring.
    spawn_agent(&mut c, &home, "burst", &["burst", "5"], None);
    assert_eq!(wait_exited(&mut c, "burst", Duration::from_secs(10)), (Some(0), None));
    // A live stream, read up to an exact offset.
    let seq_pid = spawn_agent(&mut c, &home, "seq", &["seq"], None);
    let mut s = c.attach("seq", 0).unwrap();
    assert_eq!(s.start, 0);
    let mut before = Vec::new();
    read_more(&mut s, &mut before, 400, Duration::from_secs(10));
    // Children that exit after the upgrade: on input, and on their own.
    let sh42 = sh(&mut c, &home, "sh42", "read x; exit 42");
    let sh9 = sh(&mut c, &home, "sh9", "sleep 2; exit 9");
    // Held fds: a unix listener with a queued connection, and a TCP listener.
    let path = home.root.join("cell.sock");
    assert_eq!(
        c.request(Request::HoldUnixListener { name: "cell:a".into(), path: path.to_string_lossy().into(), mode: 0o600, owner_uid: None }).unwrap(),
        Reply::Ok
    );
    drop(c.take_fd().expect("listener fd"));
    let mut early = std::os::unix::net::UnixStream::connect(&path).unwrap();
    early.write_all(b"queued").unwrap();
    let tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let tcp_addr = tcp.local_addr().unwrap();
    assert_eq!(c.request_with_fd(Request::HoldFd { name: "main".into() }, &tcp).unwrap(), Reply::Ok);
    drop(tcp);
    let pids_before: Vec<(String, i32)> = list(&mut c).into_iter().map(|s| (s.sid, s.pid)).collect();
    let burst_before = session(&list(&mut c), "burst").clone();

    assert_eq!(c.request(upgrade_req(PTYD)).unwrap(), Reply::Ok);
    assert_dropped(&mut c);
    let mut rest = Vec::new();
    s.read_to_end(&mut rest).unwrap(); // the stream closed too
    before.extend_from_slice(&rest);
    let offset = before.len() as u64;

    // Same process: our Child handle still runs, the lock names it.
    assert!(p.child.try_wait().unwrap().is_none(), "ptyd exited during the upgrade");
    wait_listening(&home.paths());
    let mut c = owner(&home);
    assert_eq!(c.hello.ptyd_version, k2_ptyd::proto::PTYD_VERSION, "the new image runs (the exec env drops the test knob)");
    assert_eq!(c.hello.boot_id, boot, "the boot id is kept across the upgrade");
    assert_eq!(std::fs::read(&home.paths().key).unwrap(), key_before, "the key file is unchanged");
    assert_eq!(std::fs::read_to_string(&home.paths().lock).unwrap().trim(), ptyd_pid.to_string());
    let after_list = list(&mut c);
    let pids_after: Vec<(String, i32)> = after_list.iter().map(|s| (s.sid.clone(), s.pid)).collect();
    assert_eq!(pids_after, pids_before, "same sessions, same child pids");
    assert!(alive(seq_pid) && alive(sh42), "children kept running");
    let burst = session(&after_list, "burst");
    assert_eq!((burst.ring_start, burst.ring_end, &burst.state), (burst_before.ring_start, burst_before.ring_end, &burst_before.state));

    // Reattach at the exact offset: no gap, no duplicate.
    let mut s = c.attach("seq", offset).unwrap();
    assert_eq!(s.start, offset, "the ring still covers the offset");
    let mut after = Vec::new();
    read_more(&mut s, &mut after, 400, Duration::from_secs(10));
    let mut all = before.clone();
    all.extend_from_slice(&after);
    assert_consecutive(&seq_numbers(&all), "seq across the upgrade");
    // And the ring from 0 is byte-identical to what the stream delivered.
    let mut again = c.attach("seq", 0).unwrap();
    let mut from0 = Vec::new();
    read_more(&mut again, &mut from0, all.len(), Duration::from_secs(10));
    assert!(from0[..all.len()] == all[..], "ring bytes differ from the live stream across the upgrade");
    // The exited record's ring.
    let mut b = c.attach("burst", 0).unwrap();
    let mut got = Vec::new();
    b.read_to_end(&mut got).unwrap();
    assert_eq!(got.len() as u64, burst.ring_end - burst.ring_start);
    assert!(got == pattern_at(burst.ring_start, got.len()), "exited ring byte-exact");

    // Held fds survived.
    assert_eq!(c.request(Request::TakeFd { name: "cell:a".into() }).unwrap(), Reply::Ok);
    let l = std::os::unix::net::UnixListener::from(c.take_fd().unwrap());
    let (mut acc, _) = l.accept().unwrap();
    let mut q = [0u8; 6];
    acc.read_exact(&mut q).unwrap();
    assert_eq!(&q, b"queued");
    assert_eq!(c.request(Request::TakeFd { name: "main".into() }).unwrap(), Reply::Ok);
    assert_eq!(std::net::TcpListener::from(c.take_fd().unwrap()).local_addr().unwrap(), tcp_addr);

    // Exit codes after the upgrade (waitpid still works: same parent).
    let mut w = c.attach("sh42", 0).unwrap();
    w.write_all(b"go\n").unwrap();
    assert_eq!(wait_exited(&mut c, "sh42", Duration::from_secs(10)), (Some(42), None));
    let t = Instant::now();
    loop {
        let l = list(&mut c);
        if let SessionState::Exited { code, signal, .. } = &session(&l, "sh9").state {
            assert_eq!((*code, *signal), (Some(9), None));
            break;
        }
        assert!(t.elapsed() < Duration::from_secs(10), "sh9 never exited");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!alive(sh9) && !alive(sh42), "reaped, no zombies");

    // The new image is hardened again and its env is the allowlist (P5).
    let Reply::Pong { dumpable, env_keys, .. } = c.request(Request::Ping).unwrap() else { panic!("ping") };
    if cfg!(target_os = "linux") {
        assert_eq!(dumpable, Some(false), "exec resets dumpability; the new image must clear it again");
    }
    for k in &env_keys {
        assert!(k2_ptyd::env_allowed(k), "{k} in the upgraded ptyd's env");
    }
    // S-2: a child spawned by the new image sees no inherited ptyd fd.
    assert_eq!(probe_fds_after(&mut c, &home, "probe"), serde_json::json!([0, 1, 2]), "an fd the upgrade inherited leaked into a new child");

    let log = std::fs::read_to_string(home.home.join("logs/ptyd.log")).unwrap();
    assert!(log.contains(&format!("upgrade: v{0} -> v{0}", k2_ptyd::proto::PTYD_VERSION)), "{log}");
    assert!(log.contains("resumed v"), "{log}");
    assert_eq!(c.request(Request::Shutdown { end_sessions: true }).unwrap(), Reply::Ok);
    assert!(p.wait_exit(Duration::from_secs(10)).success());
    assert!(!alive(seq_pid));
}

/// An `upgrade` request carrying the file's sha256 (F6), or a hash that
/// can't match when the file can't be read.
fn upgrade_req(exe: &str) -> Request {
    let sha256 = std::fs::read(exe).map(|b| k2_ptyd::auth::sha256_hex(&b)).unwrap_or_else(|_| "0".repeat(64));
    Request::Upgrade { exe: exe.to_string(), sha256: Some(sha256) }
}

/// Security review F6: the exe is checked as opened — with the expected
/// sha256 (required), ours, not group/other-writable, not a symlink —
/// before any selftest or exec; every refusal leaves the old image serving.
#[test]
fn f6_upgrade_refuses_an_exe_that_is_not_the_verified_file() {
    use std::os::unix::fs::PermissionsExt;
    let home = Home::new(false);
    let _p = Ptyd::start(&home, &[("K2_PTYD_TEST_VERSION", "0.0.1")]);
    let mut c = owner(&home);
    c.conn.set_timeout(Some(Duration::from_secs(20))).unwrap();
    let good = script(&home, "good.sh", "echo ok 9.9.9 1");
    let writable = script(&home, "gw.sh", "echo ok 9.9.9 1");
    std::fs::set_permissions(&writable, std::fs::Permissions::from_mode(0o775)).unwrap();
    let link = home.root.join("link.sh");
    std::os::unix::fs::symlink(&good, &link).unwrap();
    let link = link.to_string_lossy().into_owned();
    let cases: Vec<(&str, Request, &str)> = vec![
        ("no hash", Request::Upgrade { exe: good.clone(), sha256: None }, "sha256 required"),
        ("wrong hash", Request::Upgrade { exe: good.clone(), sha256: Some("ab".repeat(32)) }, "sha256 mismatch"),
        ("group-writable", upgrade_req(&writable), "group/other-writable"),
        ("symlink", upgrade_req(&link), "exe: open"),
    ];
    for (what, req, want) in cases {
        match c.request(req).unwrap() {
            Reply::Error { code, detail } => {
                assert_eq!(code, "upgrade_failed", "{what}");
                assert!(detail.contains(want), "{what}: {detail}");
            }
            other => panic!("{what}: expected upgrade_failed, got {other:?}"),
        }
        assert!(matches!(c.request(Request::Ping).unwrap(), Reply::Pong { .. }), "{what}: still serving");
    }
    let log = std::fs::read_to_string(home.home.join("logs/ptyd.log")).unwrap();
    assert!(log.contains("upgrade_failed: exe: sha256 mismatch"), "{log}");
}

fn script(home: &Home, name: &str, body: &str) -> String {
    use std::os::unix::fs::PermissionsExt;
    let p = home.root.join(name);
    std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p.to_string_lossy().into_owned()
}

/// P64: a selftest that fails, lies, hangs, asks for a downgrade or can't
/// run (missing or relative path) is refused with `upgrade_failed`; the old
/// image keeps serving on the same connection and the stream keeps flowing.
#[test]
fn a_bad_upgrade_is_refused_and_the_old_image_keeps_serving() {
    let home = Home::new(false);
    let _p = Ptyd::start(&home, &[("K2_PTYD_TEST_VERSION", "0.0.1")]);
    let mut c = owner(&home);
    let seq_pid = spawn_agent(&mut c, &home, "seq", &["seq"], None);
    let mut s = c.attach("seq", 0).unwrap();
    let mut seen = Vec::new();
    read_more(&mut s, &mut seen, 100, Duration::from_secs(10));
    c.conn.set_timeout(Some(Duration::from_secs(20))).unwrap();
    let cases = [
        ("relative", "k2-ptyd".to_string(), "absolute"),
        ("missing", "/nonexistent/k2-ptyd".to_string(), "exe: open"),
        ("exit 1", script(&home, "fails.sh", "exit 1"), "exited"),
        ("wrong format", script(&home, "fmt.sh", "echo ok 9.9.9 2"), "unexpected answer"),
        ("downgrade", script(&home, "old.sh", "echo ok 0.0.0 1"), "downgrade"),
        ("hangs", script(&home, "hang.sh", "sleep 30"), "timed out after 10 s"),
    ];
    for (what, exe, want) in cases {
        let t = Instant::now();
        match c.request(upgrade_req(&exe)).unwrap() {
            Reply::Error { code, detail } => {
                assert_eq!(code, "upgrade_failed", "{what}");
                assert!(detail.contains(want), "{what}: {detail}");
            }
            other => panic!("{what}: expected upgrade_failed, got {other:?}"),
        }
        if what == "hangs" {
            assert!(t.elapsed() >= Duration::from_secs(9), "the selftest bound is 10 s");
        }
        assert!(matches!(c.request(Request::Ping).unwrap(), Reply::Pong { .. }), "{what}: the owner connection survives");
        read_more(&mut s, &mut seen, 50, Duration::from_secs(10));
    }
    assert_consecutive(&seq_numbers(&seen), "seq through refused upgrades");
    assert!(alive(seq_pid));
    drop(c);
    let again = Client::connect_owner(&home.paths(), "x", true).expect("reconnect");
    assert_eq!(again.hello.ptyd_version, "0.0.1", "still the old image");
}

/// P64: an `execve` that fails after `ok` restores CLOEXEC and keeps the
/// old image serving: the daemon reconnects (same boot id, old version),
/// reattaches at its offset and nothing is lost.
#[test]
fn a_failed_exec_keeps_the_old_image_serving() {
    let home = Home::new(false);
    let _p = Ptyd::start(&home, &[("K2_PTYD_TEST_VERSION", "0.0.1"), ("K2_PTYD_TEST_FAIL_EXEC", "1")]);
    let mut c = owner(&home);
    let boot = c.hello.boot_id.clone();
    let seq_pid = spawn_agent(&mut c, &home, "seq", &["seq"], None);
    let mut s = c.attach("seq", 0).unwrap();
    let mut seen = Vec::new();
    read_more(&mut s, &mut seen, 200, Duration::from_secs(10));
    assert_eq!(c.request(upgrade_req(PTYD)).unwrap(), Reply::Ok);
    assert_dropped(&mut c);
    let mut rest = Vec::new();
    s.read_to_end(&mut rest).unwrap();
    seen.extend_from_slice(&rest);
    let mut c = owner(&home);
    assert_eq!((c.hello.ptyd_version.as_str(), c.hello.boot_id.as_str()), ("0.0.1", boot.as_str()), "the old image, same instance");
    let mut s = c.attach("seq", seen.len() as u64).unwrap();
    read_more(&mut s, &mut seen, 200, Duration::from_secs(10));
    assert_consecutive(&seq_numbers(&seen), "seq across a failed exec");
    assert!(alive(seq_pid));
    // S-2: the fds whose CLOEXEC the failed attempt cleared are CLOEXEC again.
    assert_eq!(probe_fds_after(&mut c, &home, "probe"), serde_json::json!([0, 1, 2]));
    let log = std::fs::read_to_string(home.home.join("logs/ptyd.log")).unwrap();
    assert!(log.contains("upgrade_failed: exec"), "{log}");
}

/// A private scratch dir for the F3 socket tests (short: sun_path).
fn f3_dir(tag: &str) -> PathBuf {
    let d = PathBuf::from(format!("/tmp/k2f3-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn set_mode(p: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
}

/// Security review F3: the client refuses a server that runs as another
/// uid (the real peer-credential path; an expected uid other than ours
/// stands in for "the listener belongs to someone else").
#[test]
fn f3_client_refuses_a_server_of_another_uid() {
    let base = f3_dir("uid");
    let dir = base.join("sock");
    std::fs::create_dir(&dir).unwrap();
    set_mode(&dir, 0o700);
    let sock = dir.join("ctl.sock");
    let _l = std::os::unix::net::UnixListener::bind(&sock).unwrap();
    // SAFETY: no preconditions.
    let me = unsafe { libc::geteuid() };
    assert!(Conn::connect_expecting(&sock, me).is_ok(), "our own server is accepted");
    let err = Conn::connect_expecting(&sock, me.wrapping_add(1)).err().expect("another uid must be refused");
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied, "{err}");
    assert!(err.to_string().contains("not our k2-ptyd"), "{err}");
    let _ = std::fs::remove_dir_all(&base);
}

/// Security review F3: the client refuses a socket whose directory isn't a
/// private (0700, ours) real directory — the squatted `/tmp` fallback case.
#[test]
fn f3_client_refuses_a_socket_in_a_shared_or_symlinked_dir() {
    let base = f3_dir("dir");
    let dir = base.join("sock");
    std::fs::create_dir(&dir).unwrap();
    let sock = dir.join("ctl.sock");
    let _l = std::os::unix::net::UnixListener::bind(&sock).unwrap();
    set_mode(&dir, 0o755);
    let err = Conn::connect(&sock).err().expect("a group/other-readable dir must be refused");
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied, "{err}");
    set_mode(&dir, 0o700);
    assert!(Conn::connect(&sock).is_ok(), "the private dir is accepted");
    // The same socket reached through a symlinked directory.
    let link = base.join("link");
    std::os::unix::fs::symlink(&dir, &link).unwrap();
    let err = Conn::connect(&link.join("ctl.sock")).err().expect("a symlinked dir must be refused");
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied, "{err}");
    let _ = std::fs::remove_dir_all(&base);
}
