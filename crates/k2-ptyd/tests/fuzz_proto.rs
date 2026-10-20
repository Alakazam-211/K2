//! Protocol fuzzing (PRD prd-k2-ptyd-v1 R1: "fuzz the frame parser").
//!
//! Dependency-free (P1: ptyd's surface stays tiny, dev-deps included): a
//! seeded SplitMix64 generator drives
//! 1. the incremental `FrameDecoder` against a reference decoder, over
//!    random streams cut at random points;
//! 2. `Frame<Request>` / `Frame<Reply>` parsing and `read_frame` on random
//!    and mutated bytes (must never panic; valid frames round-trip);
//! 3. a **live** `k2-ptyd`: unauthenticated and data connections sending
//!    garbage, and an authenticated owner sending garbage frames plus
//!    well-typed requests with hostile field values. ptyd must stay up and
//!    answer `ping` after every batch.
//!
//! Bounded for the normal suite. Longer runs:
//! `K2_PTYD_FUZZ_SECS=600 cargo test -p k2-ptyd --test fuzz_proto -- --test-threads 1`.
//! `K2_PTYD_FUZZ_SEED=<n>` replays a run; every failure prints its seed.

#![cfg(unix)]

use std::io::{Cursor, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use k2_ptyd::client::{Client, Conn};
use k2_ptyd::paths::Paths;
use k2_ptyd::proto::{self, Frame, FrameDecoder, FrameError, Reply, Request, Role, Sig, SigTarget, MAX_FRAME};

const PTYD: &str = env!("CARGO_BIN_EXE_k2-ptyd");
const AGENT: &str = env!("CARGO_BIN_EXE_k2-ptyd-test-agent");

// ------------------------------------------------------------------ rng

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn chance(&mut self, pct: u64) -> bool {
        self.next() % 100 < pct
    }
    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
    fn bytes_upto(&mut self, max: usize) -> Vec<u8> {
        let n = self.below(max + 1);
        self.bytes(n)
    }
    /// Edge-heavy integers.
    fn u64_edge(&mut self) -> u64 {
        match self.below(6) {
            0 => 0,
            1 => u64::MAX,
            2 => u64::MAX - self.next() % 1000,
            3 => self.next() % 100,
            4 => 1 << self.below(64),
            _ => self.next(),
        }
    }
    fn u16_edge(&mut self) -> u16 {
        match self.below(4) {
            0 => 0,
            1 => u16::MAX,
            2 => 1,
            _ => self.next() as u16,
        }
    }
    fn string(&mut self, max: usize) -> String {
        const PIECES: &[&str] = &["a", "Z", "0", "-", "/", "..", "\0", "\n", "\"", "\\", "é", "漢", "🙂", "\u{202e}", " ", "%s", "$(x)"];
        let n = self.below(max + 1);
        (0..n).map(|_| PIECES[self.below(PIECES.len())]).collect()
    }
}

fn seed() -> u64 {
    match std::env::var("K2_PTYD_FUZZ_SEED") {
        Ok(s) => s.trim().parse().expect("K2_PTYD_FUZZ_SEED must be a u64"),
        Err(_) => std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64,
    }
}

/// Run budget: `K2_PTYD_FUZZ_SECS` (wall clock) or `default_cases`.
struct Budget {
    until: Option<Instant>,
    cases: usize,
    done: usize,
}

impl Budget {
    fn new(default_cases: usize) -> Budget {
        let until = std::env::var("K2_PTYD_FUZZ_SECS")
            .ok()
            .map(|s| Instant::now() + Duration::from_secs(s.trim().parse().expect("K2_PTYD_FUZZ_SECS must be seconds")));
        Budget { until, cases: default_cases, done: 0 }
    }
    fn more(&mut self) -> bool {
        self.done += 1;
        match self.until {
            Some(t) => Instant::now() < t,
            None => self.done <= self.cases,
        }
    }
}

// ------------------------------------------------------ valid generators

fn arb_request(r: &mut Rng, sids: &[String]) -> Request {
    let sid = |r: &mut Rng| if !sids.is_empty() && r.chance(50) { sids[r.below(sids.len())].clone() } else { r.string(12) };
    match r.below(15) {
        0 => Request::List,
        1 => Request::Ping,
        2 => Request::UpdateMeta { sid: sid(r), meta: arb_json(r, 3) },
        3 => Request::Resize { sid: sid(r), cols: r.u16_edge(), rows: r.u16_edge() },
        4 => Request::Signal {
            sid: sid(r),
            sig: [Sig::Hup, Sig::Int, Sig::Term, Sig::Kill][r.below(4)],
            target: [SigTarget::Group, SigTarget::Leader, SigTarget::Foreground][r.below(3)],
        },
        5 => Request::FgPgrp { sid: sid(r) },
        6 => Request::Kill { sid: sid(r), grace_ms: if r.chance(30) { None } else { Some(r.u64_edge()) } },
        7 => Request::AckExit { sid: sid(r) },
        8 => Request::TakeFd { name: r.string(8) },
        9 => Request::ReleaseFd { name: r.string(8) },
        10 => Request::HoldFd { name: r.string(8) },
        11 => Request::Attach { sid: sid(r), from: r.u64_edge() },
        12 => Request::Hello {
            proto: r.next() as u32,
            client: r.string(8),
            client_version: r.string(8),
            mac: r.string(64),
            takeover: r.chance(50),
            role: if r.chance(50) { Role::Owner } else { Role::Data },
        },
        // Spawn only ever runs the test agent or a path that cannot exist.
        _ => Request::Spawn {
            sid: r.string(12),
            argv: if r.chance(50) { vec![AGENT.into(), "sleep".into()] } else if r.chance(50) { vec!["/nonexistent/k2-fuzz".into()] } else { vec![] },
            env: (0..r.below(4)).map(|_| (r.string(6), r.string(6))).collect(),
            cwd: if r.chance(50) { None } else { Some(format!("/nonexistent/{}", r.string(6))) },
            cols: r.u16_edge(),
            rows: r.u16_edge(),
            ring_bytes: if r.chance(50) { None } else { Some(r.u64_edge()) },
            meta: arb_json(r, 2),
        },
    }
}

fn arb_json(r: &mut Rng, depth: usize) -> serde_json::Value {
    use serde_json::Value;
    match if depth == 0 { r.below(4) } else { r.below(6) } {
        0 => Value::Null,
        1 => Value::Bool(r.chance(50)),
        2 => serde_json::json!(r.next() as i64),
        3 => {
            let max = if r.chance(5) { 70_000 } else { 16 };
            Value::String(r.string(max))
        }
        4 => Value::Array((0..r.below(4)).map(|_| arb_json(r, depth - 1)).collect()),
        _ => Value::Object((0..r.below(4)).map(|_| (r.string(6), arb_json(r, depth - 1))).collect()),
    }
}

/// JSON-ish garbage: mutations of a valid frame, structural bombs, raw bytes.
fn arb_frame_body(r: &mut Rng) -> Vec<u8> {
    let valid = serde_json::to_vec(&Frame::new(Some(r.next() % 1000), arb_request(r, &[]))).unwrap();
    match r.below(9) {
        0 => r.bytes_upto(256),
        1 => {
            let mut v = valid;
            for _ in 0..1 + r.below(4) {
                let i = r.below(v.len());
                v[i] = r.next() as u8;
            }
            v
        }
        2 => {
            let mut v = valid;
            v.truncate(r.below(v.len()));
            v
        }
        3 => {
            let n = 1 + r.below(5000);
            let open = if r.chance(50) { "[" } else { "{\"a\":" };
            format!("{}{}", open.repeat(n), if r.chance(50) { "1" } else { "" }).into_bytes()
        }
        4 => format!(r#"{{"id":{},"t":"{}"}}"#, ["-1", "1e400", "18446744073709551616", "null", "\"x\"", "0"][r.below(6)], r.string(10)).into_bytes(),
        5 => br#"{"id":1,"t":"resize","sid":"\ud800","cols":-1,"rows":70000}"#.to_vec(),
        6 => format!(r#"{{"t":"spawn","id":2,"sid":"s","argv":{},"env":[["a"]],"cols":1,"rows":1}}"#, ["null", "[1]", "\"x\"", "[]"][r.below(4)]).into_bytes(),
        7 => {
            let mut v = valid;
            let at = r.below(v.len());
            let n = 1 + r.below(16);
            let junk = r.bytes(n);
            v.splice(at..at, junk);
            v
        }
        _ => valid,
    }
}

// ---------------------------------------------------- 1. decoder vs ref

/// Reference: whole-buffer decode. Frames until the first incomplete one,
/// or `Err` at the first oversize header.
fn reference_decode(stream: &[u8]) -> (Vec<Vec<u8>>, bool) {
    let mut out = Vec::new();
    let mut i = 0;
    while stream.len() - i >= 4 {
        let len = u32::from_be_bytes(stream[i..i + 4].try_into().unwrap()) as usize;
        if len > MAX_FRAME {
            return (out, true);
        }
        if stream.len() - i - 4 < len {
            break;
        }
        out.push(stream[i + 4..i + 4 + len].to_vec());
        i += 4 + len;
    }
    (out, false)
}

#[test]
fn fuzz_frame_decoder_matches_reference_under_random_splits() {
    let s = seed();
    let mut r = Rng(s);
    let mut b = Budget::new(4000);
    while b.more() {
        let mut stream = Vec::new();
        for _ in 0..r.below(6) {
            match r.below(10) {
                0 => stream.extend(r.bytes_upto(9)),
                1 => stream.extend(((MAX_FRAME as u32) + 1 + (r.next() as u32 % 1000)).to_be_bytes()),
                2 => stream.extend((MAX_FRAME as u32).to_be_bytes()),
                _ => stream.extend(proto::encode(&Frame::new(Some(r.next()), arb_request(&mut r, &[]))).unwrap_or_default()),
            }
        }
        let (want, want_err) = reference_decode(&stream);
        let mut d = FrameDecoder::default();
        let (mut got, mut got_err, mut i) = (Vec::new(), false, 0);
        'feed: while i < stream.len() {
            let n = 1 + r.below(stream.len() - i);
            d.push(&stream[i..i + n]);
            i += n;
            loop {
                match d.next_frame() {
                    Ok(Some(f)) => got.push(f),
                    Ok(None) => break,
                    Err(FrameError::TooLarge) => {
                        got_err = true;
                        break 'feed;
                    }
                }
            }
        }
        assert_eq!(got_err, want_err, "seed {s} case {}: oversize detection differs", b.done);
        if want_err {
            assert!(got.len() <= want.len() && got[..] == want[..got.len()], "seed {s} case {}: frames before the oversize header differ", b.done);
        } else {
            assert_eq!(got, want, "seed {s} case {}: frames differ", b.done);
        }
        for f in &got {
            assert!(f.len() <= MAX_FRAME);
        }
    }
}

// ------------------------------------------------- 2. parsing never panics

#[test]
fn fuzz_frame_parsing_never_panics_and_valid_frames_round_trip() {
    let s = seed();
    let mut r = Rng(s);
    let mut b = Budget::new(20_000);
    while b.more() {
        let body = arb_frame_body(&mut r);
        let _ = serde_json::from_slice::<Frame<Request>>(&body);
        let _ = serde_json::from_slice::<Frame<Reply>>(&body);
        // read_frame over a length prefix that may lie about the body.
        let mut wire = Vec::new();
        let claimed = match r.below(4) {
            0 => body.len() as u32 + r.below(10) as u32,
            1 => u32::MAX,
            _ => body.len() as u32,
        };
        wire.extend(claimed.to_be_bytes());
        wire.extend(&body);
        let _ = proto::read_frame::<_, Frame<Request>>(&mut Cursor::new(&wire));
        let _ = proto::read_frame::<_, Frame<Reply>>(&mut Cursor::new(&wire));

        // Round trip: encode → decode → equal.
        let req = Frame::new(if r.chance(50) { Some(r.next()) } else { None }, arb_request(&mut r, &[]));
        match proto::encode(&req) {
            Ok(enc) => {
                let back: Frame<Request> = proto::read_frame(&mut Cursor::new(&enc)).unwrap_or_else(|e| panic!("seed {s}: {e} for {req:?}"));
                assert_eq!(back, req, "seed {s}: round trip");
            }
            Err(e) => assert_eq!(e.to_string(), "frame_too_large", "seed {s}"),
        }
    }
}

// ------------------------------------------------------- 3. live ptyd

static N: AtomicUsize = AtomicUsize::new(0);

struct Live {
    root: PathBuf,
    home: PathBuf,
    child: Child,
}

impl Live {
    fn start() -> Live {
        Live::start_with(&[])
    }
    fn start_with(extra: &[(&str, &str)]) -> Live {
        let root = PathBuf::from(format!("/tmp/k2pf-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join(".k2");
        std::fs::create_dir_all(&home).unwrap();
        let mut cmd = Command::new(PTYD);
        cmd.arg("--serve")
            .env_clear()
            .env("K2_HOME", &home)
            .env("HOME", &root)
            .env("PATH", "/usr/bin:/bin")
            .env("TMPDIR", "/tmp")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        for (k, v) in extra {
            cmd.env(k, v);
        }
        let child = cmd.spawn().expect("spawn k2-ptyd");
        let live = Live { root, home, child };
        let t = Instant::now();
        while !(live.paths().key.exists() && UnixStream::connect(live.paths().client_sock()).is_ok()) {
            assert!(t.elapsed() < Duration::from_secs(10), "ptyd never listened");
            std::thread::sleep(Duration::from_millis(20));
        }
        live
    }
    fn paths(&self) -> Paths {
        Paths::resolve(&self.home, Path::new("/tmp"))
    }
    /// Exit status (if ptyd is gone) and the tail of its own log, so a
    /// failure says whether ptyd crashed, exited on purpose, or is wedged.
    fn diag(&mut self) -> String {
        let status = self.child.try_wait().unwrap();
        let log = std::fs::read_to_string(&self.paths().log).unwrap_or_else(|e| format!("<no log: {e}>"));
        let tail: Vec<&str> = log.lines().rev().take(15).collect::<Vec<_>>().into_iter().rev().collect();
        format!("ptyd status: {status:?}; log tail:\n{}", tail.join("\n"))
    }
    fn assert_alive(&mut self, ctx: &str) {
        if self.child.try_wait().unwrap().is_some() {
            panic!("k2-ptyd DIED ({ctx}): {}", self.diag());
        }
    }
    fn connect_raw(&mut self, ctx: &str) -> UnixStream {
        match UnixStream::connect(self.paths().client_sock()) {
            Ok(s) => s,
            Err(e) => panic!("connect failed ({ctx}): {e}; {}", self.diag()),
        }
    }
    fn owner(&self) -> Client {
        let c = Client::connect_owner(&self.paths(), "fuzz", true).expect("owner connect");
        c.conn.set_timeout(Some(Duration::from_secs(10))).unwrap();
        c
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        if self.child.try_wait().unwrap().is_none() {
            // SIGKILL every session first: `hup-ignore` agents would
            // outlive ptyd's closed masters.
            if let Ok(mut c) = Client::connect_owner(&self.paths(), "fuzz-teardown", true) {
                let _ = c.conn.set_timeout(Some(Duration::from_secs(5)));
                if let Ok(Reply::Sessions { sessions }) = c.request(Request::List) {
                    for s in sessions {
                        // SAFETY: our own test agents.
                        unsafe { libc::kill(s.pid, libc::SIGKILL) };
                    }
                }
            }
            // SAFETY: our own test child.
            unsafe { libc::kill(self.child.id() as i32, libc::SIGKILL) };
            let _ = self.child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn ping(c: &mut Client) -> bool {
    matches!(c.request(Request::Ping), Ok(Reply::Pong { .. }))
}

fn framed(body: &[u8]) -> Vec<u8> {
    let mut v = (body.len() as u32).to_be_bytes().to_vec();
    v.extend_from_slice(body);
    v
}

/// Drain whatever ptyd answered on a raw connection (bounded), so its
/// write buffer never blocks the fuzz loop.
fn drain(s: &mut UnixStream) {
    // macOS answers EINVAL here once ptyd has closed the connection (which
    // it may do for garbage); nothing left to drain then. Liveness is
    // checked by the caller after every case.
    if s.set_read_timeout(Some(Duration::from_millis(5))).is_err() {
        return;
    }
    let mut buf = [0u8; 65536];
    for _ in 0..8 {
        match s.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
    }
}

#[test]
fn fuzz_live_ptyd_survives_hostile_unauthenticated_and_data_connections() {
    let s = seed();
    let mut r = Rng(s);
    let mut live = Live::start();
    // An owner stays connected for the whole run, as a running daemon
    // does. Without one, ptyd's P7 idle exit (60 s, no owner + no
    // sessions) ends it cleanly mid-run — what job 661802f5 hit; pinned by
    // `unauthenticated_peers_cannot_keep_ptyd_alive_and_idle_exit_is_clean`.
    let mut keeper = live.owner();
    let mut b = Budget::new(300);
    while b.more() {
        let ctx = format!("seed {s} case {}", b.done);
        let mut sock = live.connect_raw(&ctx);
        sock.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
        // Half the time get past the hello as a real data client first.
        if r.chance(50) {
            let key = match k2_ptyd::auth::read_key(&live.paths().key) {
                Ok(k) => k,
                Err(e) => panic!("read key ({ctx}): {e}; {}", live.diag()),
            };
            let mut conn = match Conn::connect(&live.paths().client_sock()) {
                Ok(c) => c,
                Err(e) => panic!("data connect ({ctx}): {e}; {}", live.diag()),
            };
            conn.set_timeout(Some(Duration::from_secs(5))).unwrap();
            if let Err(e) = conn.handshake(&key, "fuzz", Role::Data, false) {
                panic!("data hello ({ctx}): {e:?}; {}", live.diag());
            }
            sock = conn.stream;
        }
        for _ in 0..1 + r.below(4) {
            let payload = match r.below(4) {
                0 => r.bytes_upto(512),
                1 => ((MAX_FRAME as u32) + 1).to_be_bytes().to_vec(),
                _ => framed(&arb_frame_body(&mut r)),
            };
            if sock.write_all(&payload).is_err() {
                break; // ptyd closed this connection: allowed.
            }
            drain(&mut sock);
        }
        if r.chance(30) {
            let _ = sock.shutdown(std::net::Shutdown::Write);
        }
        drop(sock);
        live.assert_alive(&ctx);
        if b.done % 25 == 0 && !ping(&mut keeper) {
            panic!("{ctx}: the owner's ping went unanswered during the flood; {}", live.diag());
        }
    }
    assert!(ping(&mut keeper), "seed {s}: the owner connection survived the run");
    let mut c = live.owner();
    assert!(ping(&mut c), "seed {s}: ptyd answers a new owner after the run");
}

/// Pinned from the long fuzz run (job 661802f5, 2026-10-20): with no owner
/// and no sessions, a flood of unauthenticated and data-role peers must
/// NOT keep ptyd alive (P7 idle exit still fires), and the exit is clean:
/// status 0, `idle exit` logged, socket and key removed. Minimal input: no
/// owner for longer than the idle timeout; the garbage only proves peers
/// don't reset the timer.
#[test]
fn unauthenticated_peers_cannot_keep_ptyd_alive_and_idle_exit_is_clean() {
    // Clock from before the spawn: ptyd's idle timer starts at its init.
    let t = Instant::now();
    let mut live = Live::start_with(&[("K2_PTYD_TEST_IDLE_SECS", "2")]);
    let mut r = Rng(seed());
    let status = loop {
        if let Some(st) = live.child.try_wait().unwrap() {
            break st;
        }
        assert!(t.elapsed() < Duration::from_secs(15), "ptyd never idle-exited under the flood; {}", live.diag());
        if let Ok(mut s) = UnixStream::connect(live.paths().client_sock()) {
            let body = arb_frame_body(&mut r);
            let _ = s.write_all(&framed(&body));
            drain(&mut s);
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(t.elapsed() >= Duration::from_secs(2), "exited before the idle timeout: {:?}", t.elapsed());
    assert!(status.success(), "idle exit must be clean, got {status:?}; {}", live.diag());
    let log = std::fs::read_to_string(&live.paths().log).expect("ptyd log");
    assert!(log.contains("idle exit"), "exit reason not logged: {}", live.diag());
    assert!(!live.paths().sock.exists(), "socket left behind");
    assert!(!live.paths().key.exists(), "key left behind");
}

#[test]
fn fuzz_live_ptyd_survives_a_hostile_owner() {
    let s = seed();
    let mut r = Rng(s);
    let mut live = Live::start();
    let mut c = live.owner();
    let mut sids: Vec<String> = Vec::new();
    let mut b = Budget::new(400);
    while b.more() {
        let ctx = format!("seed {s} case {}", b.done);
        // Keep a live session around so session-addressed requests hit it.
        if sids.is_empty() || r.chance(5) {
            let sid = format!("live-{}", b.done);
            if let Ok(Reply::Spawned { .. }) = c.request(Request::Spawn {
                sid: sid.clone(),
                // hup-ignore keeps the session running through Kill's
                // SIGHUP, so the grace deadline path runs too.
                argv: vec![AGENT.into(), if r.chance(50) { "hup-ignore" } else { "sleep" }.into()],
                env: vec![("PATH".into(), "/usr/bin:/bin".into())],
                cwd: None,
                cols: 80,
                rows: 24,
                ring_bytes: None,
                meta: serde_json::Value::Null,
            }) {
                sids.push(sid);
            }
        }
        let mut reconnect = false;
        match r.below(3) {
            // Garbage frame on the owner connection.
            0 => {
                let body = arb_frame_body(&mut r);
                if c.conn.stream.write_all(&framed(&body)).is_err() {
                    reconnect = true;
                }
            }
            // An oversize header closes the connection (P19).
            1 if r.chance(10) => {
                let _ = c.conn.stream.write_all(&((MAX_FRAME as u32) + 1).to_be_bytes());
                reconnect = true;
            }
            // A well-typed request with hostile values.
            _ => {
                let req = arb_request(&mut r, &sids);
                let is_kill = matches!(req, Request::Kill { .. });
                c.conn.set_timeout(Some(Duration::from_secs(if is_kill { 1 } else { 10 }))).unwrap();
                match c.request(req) {
                    Ok(_) => {}
                    // Kill with a long grace answers only when the child is
                    // gone; a timeout there is the protocol, not a hang.
                    Err(e) if is_kill && (e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut) => reconnect = true,
                    Err(_) => reconnect = true,
                }
                c.conn.set_timeout(Some(Duration::from_secs(10))).unwrap();
            }
        }
        live.assert_alive(&ctx);
        if reconnect {
            c = live.owner();
        }
        if b.done % 25 == 0 {
            if !ping(&mut c) {
                c = live.owner();
                assert!(ping(&mut c), "{ctx}: ptyd stopped answering ping");
            }
            // Don't let the fuzz pile up hundreds of agents.
            if sids.len() > 8 {
                for sid in sids.drain(..) {
                    let _ = c.request(Request::Signal { sid: sid.clone(), sig: Sig::Kill, target: SigTarget::Leader });
                }
            }
        }
    }
    live.assert_alive(&format!("seed {s} end"));
    let mut c = live.owner();
    assert!(ping(&mut c), "seed {s}: ptyd answers ping after the run");
}

/// Fuzz finding class: `kill.grace_ms` is an untrusted u64 added to an
/// `Instant`. Every edge value must leave ptyd up.
#[test]
fn kill_with_edge_grace_values_keeps_ptyd_up() {
    let mut live = Live::start();
    let mut c = live.owner();
    for (i, grace) in [u64::MAX, u64::MAX / 2, 1 << 63, 1 << 40, 0].into_iter().enumerate() {
        let sid = format!("g{i}");
        match c
            .request(Request::Spawn {
                sid: sid.clone(),
                argv: vec![AGENT.into(), "hup-ignore".into()],
                env: vec![("PATH".into(), "/usr/bin:/bin".into())],
                cwd: None,
                cols: 80,
                rows: 24,
                ring_bytes: None,
                meta: serde_json::Value::Null,
            })
            .expect("spawn")
        {
            Reply::Spawned { .. } => {}
            other => panic!("{other:?}"),
        }
        // The agent prints READY after installing its SIGHUP ignore.
        {
            let mut d = c.attach(&sid, 0).expect("attach");
            d.stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let (mut acc, mut buf) = (Vec::new(), [0u8; 256]);
            while !String::from_utf8_lossy(&acc).contains("READY") {
                let n = d.read(&mut buf).expect("READY before timeout");
                assert!(n > 0, "agent output ended before READY: {:?}", String::from_utf8_lossy(&acc));
                acc.extend_from_slice(&buf[..n]);
            }
        }
        c.conn.set_timeout(Some(Duration::from_millis(500))).unwrap();
        // The reply only comes once the child is gone: at once for grace 0
        // (SIGKILL), never within the timeout for the long graces.
        let r = c.request(Request::Kill { sid: sid.clone(), grace_ms: Some(grace) });
        live.assert_alive(&format!("kill grace_ms={grace}"));
        if grace == 0 {
            assert_eq!(r.expect("kill reply"), Reply::Ok, "grace 0 ends the session now");
            continue;
        }
        let e = r.expect_err("a hup-ignoring agent outlives the reply timeout under a long grace");
        assert!(matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut), "{e}");
        c = live.owner();
        assert!(ping(&mut c), "grace_ms={grace}: ptyd stopped answering");
        assert_eq!(
            c.request(Request::Signal { sid: sid.clone(), sig: Sig::Kill, target: SigTarget::Leader }).expect("signal"),
            Reply::Ok,
            "grace_ms={grace}: the session is still there and running"
        );
    }
}
