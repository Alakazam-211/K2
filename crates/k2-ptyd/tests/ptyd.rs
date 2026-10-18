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
    let ppid = parent_of(ptyd_pid);
    assert_ne!(ppid, std::process::id() as i32, "ptyd is never the caller's child (P6)");
    // A second launch while one runs exits 3.
    let st = base_cmd(&home).arg("--launch").status().unwrap();
    assert_eq!(st.code(), Some(3));
    // Idle: no sessions, no owner → exits; socket and key removed.
    wait_gone(ptyd_pid, Duration::from_secs(10));
    assert!(!paths.sock.exists() && !paths.key.exists());
}

fn parent_of(pid: i32) -> i32 {
    let out = Command::new("ps").args(["-o", "ppid=", "-p", &pid.to_string()]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
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
    let env = process_env(pid);
    assert!(env.contains("K2_HOME="), "sanity: env readable: {env}");
    assert!(!env.contains("leak-me-not") && !env.contains("NOTIFY_SOCKET"), "{env}");
    let mut c = owner(&home);
    assert_eq!(c.request(Request::Shutdown { end_sessions: true }).unwrap(), Reply::Ok);
    wait_gone(pid, Duration::from_secs(5));
}

fn process_env(pid: i32) -> String {
    if cfg!(target_os = "linux") {
        String::from_utf8_lossy(&std::fs::read(format!("/proc/{pid}/environ")).unwrap()).replace('\0', " ")
    } else {
        let out = Command::new("ps").args(["eww", "-o", "command=", "-p", &pid.to_string()]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
}
