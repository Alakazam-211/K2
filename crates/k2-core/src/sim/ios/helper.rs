//! `k2-simhelper`: build it on first use, run one per booted device, talk to
//! it over its pipes.
//!
//! **Build.** The Swift source is embedded in the daemon. The first time a
//! device needs it, the daemon compiles it with the Mac's own Xcode
//! (`xcrun swiftc`, about 3 s) into `~/.k2/sim/ios/bin/k2-simhelper-<key>`.
//! The key hashes the source AND the Xcode version, so a new Xcode gets a
//! fresh build against its own private frameworks. A sidecar file holds the
//! binary's sha256; a binary that no longer matches it is rebuilt. No release
//! pipeline change, and it builds for the Mac's own CPU (no arm64-only
//! limit, M-A16).
//!
//! **Run.** A plain child process (M-A15): stdin takes one JSON request per
//! line; stdout carries framed replies, events and JPEG frames (see the
//! Swift file's header). No socket, so nothing else on the machine can
//! reach it, and no token to leak. It exits when its stdin closes or its
//! device stops. At start it checks every private class and symbol it uses
//! and refuses with `unsupported_xcode` (naming the Xcode) if one is gone.

use std::collections::HashMap;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value as J};
use sha2::{Digest, Sha256};

use super::simctl::{Runner, SimError, SimResult};

/// The helper's Swift source, compiled on first use.
pub const SOURCE: &str = include_str!("helper/k2-simhelper.swift");
/// Bumps with the wire protocol; the helper reports it in `ready`/`hello`.
pub const PROTOCOL_VERSION: u64 = 1;
/// No message is larger than this (a 4K-ish JPEG is far below).
pub const MAX_MESSAGE: usize = 64 * 1024 * 1024;

/// One JPEG frame as the helper sent it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperFrame {
    pub seq: u64,
    pub width: u32,
    pub height: u32,
    pub jpeg: Vec<u8>,
}

pub fn sha256_hex(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

/// `k2-simhelper-<16 hex of sha256(source, xcode version)>`.
pub fn binary_name(xcode: &str) -> String {
    let mut h = Sha256::new();
    h.update(SOURCE.as_bytes());
    h.update([0u8]);
    h.update(xcode.as_bytes());
    let hex: String = h.finalize().iter().map(|x| format!("{x:02x}")).collect();
    format!("k2-simhelper-{}", &hex[..16])
}

/// The helper binary in `dir` for this Xcode, compiled first if it's
/// missing or doesn't match its recorded hash.
pub fn ensure_built(dir: &Path, runner: &dyn Runner, xcode: &str) -> SimResult<PathBuf> {
    use std::os::unix::fs::DirBuilderExt;
    let name = binary_name(xcode);
    let bin = dir.join(&name);
    let marker = dir.join(format!("{name}.sha256"));
    if let (Ok(b), Ok(m)) = (std::fs::read(&bin), std::fs::read_to_string(&marker)) {
        if sha256_hex(&b) == m.trim() {
            return Ok(bin);
        }
    }
    let find: Vec<String> = vec!["--find".into(), "swiftc".into()];
    let found = runner.run("xcrun", &find, Duration::from_secs(30))?;
    if found.status != 0 {
        return Err(SimError::new(
            "no_swiftc",
            "K2 builds its Simulator helper with Xcode's Swift compiler, and `xcrun swiftc` isn't available. Install Xcode (or run `xcode-select --install`), open it once, then try again.",
        ));
    }
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|e| SimError::new("helper_build_failed", format!("cannot create {}: {e}", dir.display())))?;
    let src = dir.join(format!("{name}.swift"));
    std::fs::write(&src, SOURCE).map_err(|e| SimError::new("helper_build_failed", format!("cannot write {}: {e}", src.display())))?;
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let args: Vec<String> = vec!["swiftc".into(), "-O".into(), "-o".into(), tmp.to_string_lossy().into_owned(), src.to_string_lossy().into_owned()];
    let out = runner.run("xcrun", &args, Duration::from_secs(300))?;
    if out.status != 0 {
        let _ = std::fs::remove_file(&tmp);
        let tail: String = out.stderr.lines().filter(|l| l.contains("error")).take(5).collect::<Vec<_>>().join("\n");
        return Err(SimError::new(
            "helper_build_failed",
            format!("K2 couldn't build its Simulator helper with Xcode {xcode} (xcrun swiftc exited {}). Open Xcode once so it finishes installing, then try again.\n{tail}", out.status),
        ));
    }
    let bytes = std::fs::read(&tmp).map_err(|e| SimError::new("helper_build_failed", format!("swiftc wrote no binary: {e}")))?;
    std::fs::rename(&tmp, &bin).map_err(|e| SimError::new("helper_build_failed", format!("cannot place the helper: {e}")))?;
    std::fs::write(&marker, sha256_hex(&bytes)).map_err(|e| SimError::new("helper_build_failed", format!("cannot write {}: {e}", marker.display())))?;
    let _ = std::fs::remove_file(&src);
    Ok(bin)
}

/// One message off the helper's stdout.
#[derive(Debug, PartialEq)]
pub enum Message {
    Json(J),
    Frame(HelperFrame),
}

/// Reads one framed message; `Ok(None)` at a clean end of stream.
pub fn read_message(r: &mut impl Read) -> std::io::Result<Option<Message>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let n = u32::from_be_bytes(len) as usize;
    if n == 0 || n > MAX_MESSAGE {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("helper message of {n} bytes")));
    }
    let mut body = vec![0u8; n];
    r.read_exact(&mut body)?;
    let (kind, payload) = (body[0], &body[1..]);
    match kind {
        b'J' => serde_json::from_slice(payload)
            .map(|v| Some(Message::Json(v)))
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("helper JSON: {e}"))),
        b'F' if payload.len() >= 16 => Ok(Some(Message::Frame(HelperFrame {
            width: u32::from_be_bytes(payload[0..4].try_into().unwrap()),
            height: u32::from_be_bytes(payload[4..8].try_into().unwrap()),
            seq: u64::from_be_bytes(payload[8..16].try_into().unwrap()),
            jpeg: payload[16..].to_vec(),
        }))),
        k => Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("helper message kind {k:#x}"))),
    }
}

/// The newest frame, overwritten in place: a slow puller always gets the
/// latest screen and never stalls the helper's reader.
#[derive(Default)]
pub struct FrameSlot {
    newest: Mutex<Option<HelperFrame>>,
    cv: Condvar,
    closed: AtomicBool,
}

impl FrameSlot {
    pub fn put(&self, f: HelperFrame) {
        *self.newest.lock().unwrap() = Some(f);
        self.cv.notify_all();
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.cv.notify_all();
    }

    /// Takes the newest frame, waiting up to `wait`. `Ok(None)` = nothing
    /// new; `Err(())` = the helper is gone.
    pub fn take(&self, wait: Duration) -> Result<Option<HelperFrame>, ()> {
        let deadline = Instant::now() + wait;
        let mut g = self.newest.lock().unwrap();
        loop {
            if let Some(f) = g.take() {
                return Ok(Some(f));
            }
            if self.closed.load(Ordering::SeqCst) {
                return Err(());
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            g = self.cv.wait_timeout(g, left).unwrap().0;
        }
    }
}

/// Shared between the reader thread and callers.
#[derive(Default)]
struct Inbox {
    pending: Mutex<HashMap<u64, mpsc::Sender<J>>>,
    frames: Mutex<Option<Arc<FrameSlot>>>,
    ready: Mutex<Option<mpsc::Sender<SimResult<J>>>>,
    alive: AtomicBool,
    gone_reason: Mutex<Option<String>>,
}

impl Inbox {
    fn dispatch(&self, m: Message) {
        match m {
            Message::Frame(f) => {
                if let Some(slot) = self.frames.lock().unwrap().as_ref() {
                    slot.put(f);
                }
            }
            Message::Json(v) => {
                if let Some(id) = v.get("id").and_then(J::as_u64) {
                    if let Some(tx) = self.pending.lock().unwrap().remove(&id) {
                        let _ = tx.send(v);
                    }
                    return;
                }
                match v.get("event").and_then(J::as_str) {
                    Some("ready") => {
                        let ok = v.get("version").and_then(J::as_u64) == Some(PROTOCOL_VERSION);
                        let r = if ok { Ok(v.clone()) } else { Err(SimError::new("helper_version", format!("helper speaks {:?}, K2 wants {PROTOCOL_VERSION}", v.get("version")))) };
                        if let Some(tx) = self.ready.lock().unwrap().take() {
                            let _ = tx.send(r);
                        }
                    }
                    Some("fatal") => {
                        let code = match v.get("code").and_then(J::as_str) {
                            Some("unsupported_xcode") => "unsupported_xcode",
                            Some("no_device") => "no_device",
                            _ => "helper_fatal",
                        };
                        let e = SimError::new(code, v.get("error").and_then(J::as_str).unwrap_or("helper failed").to_string());
                        *self.gone_reason.lock().unwrap() = Some(e.message.clone());
                        if let Some(tx) = self.ready.lock().unwrap().take() {
                            let _ = tx.send(Err(e));
                        }
                    }
                    Some("device_gone") => {
                        self.alive.store(false, Ordering::SeqCst);
                        *self.gone_reason.lock().unwrap() = Some(format!("the device is {}", v.get("state").and_then(J::as_str).unwrap_or("stopped")));
                        if let Some(slot) = self.frames.lock().unwrap().as_ref() {
                            slot.close();
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    fn closed(&self) {
        self.alive.store(false, Ordering::SeqCst);
        // Dropping the senders wakes every waiting caller with an error.
        self.pending.lock().unwrap().clear();
        if let Some(slot) = self.frames.lock().unwrap().take() {
            slot.close();
        }
        if let Some(tx) = self.ready.lock().unwrap().take() {
            let _ = tx.send(Err(SimError::new("helper_exited", "the Simulator helper exited before it was ready")));
        }
    }
}

/// What the lane needs from a helper; the seam unit tests fake.
pub trait HelperIo: Send + Sync {
    fn call(&self, op: &str, args: J, timeout: Duration) -> SimResult<J>;
    /// Where frames go; `None` stops forwarding.
    fn frame_slot(&self, slot: Option<Arc<FrameSlot>>);
    fn alive(&self) -> bool;
}

/// A running helper for one device.
pub struct HelperProc {
    child: Mutex<Child>,
    stdin: Mutex<BufWriter<ChildStdin>>,
    inbox: Arc<Inbox>,
    next_id: AtomicU64,
    stderr_tail: Arc<Mutex<Vec<String>>>,
}

impl HelperProc {
    pub fn spawn(bin: &Path, set: &Path, udid: &str) -> SimResult<Self> {
        let mut child = Command::new(bin)
            .arg("--set")
            .arg(set)
            .arg("--udid")
            .arg(udid)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| SimError::new("helper_spawn_failed", format!("cannot start {}: {e}", bin.display())))?;
        let inbox = Arc::new(Inbox::default());
        inbox.alive.store(true, Ordering::SeqCst);
        let (ready_tx, ready_rx) = mpsc::channel();
        *inbox.ready.lock().unwrap() = Some(ready_tx);

        let mut stdout = child.stdout.take().expect("piped stdout");
        let ib = inbox.clone();
        std::thread::Builder::new()
            .name(format!("k2-simhelper-{}", &udid[..8.min(udid.len())]))
            .spawn(move || {
                loop {
                    match read_message(&mut stdout) {
                        Ok(Some(m)) => ib.dispatch(m),
                        Ok(None) => break,
                        Err(e) => {
                            *ib.gone_reason.lock().unwrap() = Some(format!("helper stream broke: {e}"));
                            break;
                        }
                    }
                }
                ib.closed();
            })
            .map_err(|e| SimError::new("helper_spawn_failed", format!("reader thread: {e}")))?;

        let stderr_tail = Arc::new(Mutex::new(Vec::new()));
        let mut stderr = child.stderr.take().expect("piped stderr");
        let tail = stderr_tail.clone();
        let _ = std::thread::Builder::new().name("k2-simhelper-err".into()).spawn(move || {
            let mut buf = String::new();
            let _ = stderr.read_to_string(&mut buf);
            tail.lock().unwrap().extend(buf.lines().rev().take(20).map(str::to_string));
        });

        let stdin = child.stdin.take().expect("piped stdin");
        let me = Self { child: Mutex::new(child), stdin: Mutex::new(BufWriter::new(stdin)), inbox, next_id: AtomicU64::new(1), stderr_tail };
        match ready_rx.recv_timeout(Duration::from_secs(30)) {
            Ok(Ok(_)) => Ok(me),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(SimError::new("helper_timeout", "the Simulator helper didn't start within 30 s")),
        }
    }

    pub fn alive(&self) -> bool {
        self.inbox.alive.load(Ordering::SeqCst)
    }

    /// Sends one request and waits for its reply. A reply with `ok:false`
    /// becomes a `SimError` with the helper's code.
    pub fn call(&self, op: &str, mut args: J, timeout: Duration) -> SimResult<J> {
        if !self.alive() {
            let why = self.inbox.gone_reason.lock().unwrap().clone().unwrap_or_else(|| "it exited".into());
            return Err(SimError::new("helper_gone", format!("the Simulator helper stopped ({why}); run k2 sim boot")));
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel();
        self.inbox.pending.lock().unwrap().insert(id, tx);
        if !args.is_object() {
            args = json!({});
        }
        args["id"] = json!(id);
        args["op"] = json!(op);
        let line = format!("{args}\n");
        {
            let mut w = self.stdin.lock().unwrap();
            if let Err(e) = w.write_all(line.as_bytes()).and_then(|_| w.flush()) {
                self.inbox.pending.lock().unwrap().remove(&id);
                return Err(SimError::new("helper_gone", format!("cannot reach the Simulator helper: {e}")));
            }
        }
        let reply = match rx.recv_timeout(timeout) {
            Ok(v) => v,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.inbox.pending.lock().unwrap().remove(&id);
                return Err(SimError::new("helper_timeout", format!("the Simulator helper took longer than {}s on {op}", timeout.as_secs())));
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let tail = self.stderr_tail.lock().unwrap().join(" | ");
                return Err(SimError::new("helper_gone", format!("the Simulator helper exited during {op}. {tail}")));
            }
        };
        if reply.get("ok").and_then(J::as_bool) == Some(true) {
            Ok(reply)
        } else {
            let code = helper_code(reply.get("code").and_then(J::as_str).unwrap_or("helper_error"));
            Err(SimError::new(code, reply.get("error").and_then(J::as_str).unwrap_or("the helper refused").to_string()))
        }
    }
}

impl HelperIo for HelperProc {
    fn call(&self, op: &str, args: J, timeout: Duration) -> SimResult<J> {
        HelperProc::call(self, op, args, timeout)
    }
    fn frame_slot(&self, slot: Option<Arc<FrameSlot>>) {
        let mut g = self.inbox.frames.lock().unwrap();
        if let Some(old) = g.take() {
            old.close();
        }
        *g = slot;
    }
    fn alive(&self) -> bool {
        HelperProc::alive(self)
    }
}

/// Maps the helper's free-form codes onto `&'static str` codes.
fn helper_code(c: &str) -> &'static str {
    match c {
        "unmappable" => "unmappable",
        "no_element" => "stale_element",
        "no_frontmost_app" => "no_app",
        "bad_button" | "bad_request" => "bad_request",
        "no_frame" => "no_frame",
        "no_device" => "no_device",
        "no_indigo" | "no_hid_client" | "no_ax" | "no_coresimulator" | "unsupported_xcode" => "unsupported_xcode",
        _ => "helper_error",
    }
}

impl Drop for HelperProc {
    fn drop(&mut self) {
        if let Ok(mut w) = self.stdin.lock() {
            let _ = w.write_all(b"{\"op\":\"quit\",\"id\":0}\n").and_then(|_| w.flush());
        }
        if let Ok(mut c) = self.child.lock() {
            for _ in 0..25 {
                if matches!(c.try_wait(), Ok(Some(_))) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::simctl::{FakeRunner, Output};
    use super::*;
    use std::io::Cursor;

    fn msg(kind: u8, payload: &[u8]) -> Vec<u8> {
        let mut v = ((payload.len() + 1) as u32).to_be_bytes().to_vec();
        v.push(kind);
        v.extend_from_slice(payload);
        v
    }

    fn hf(seq: u64) -> HelperFrame {
        HelperFrame { seq, width: 1, height: 1, jpeg: vec![] }
    }

    #[test]
    fn messages_decode_json_frames_and_a_clean_end() {
        let mut frame = Vec::new();
        frame.extend_from_slice(&402u32.to_be_bytes());
        frame.extend_from_slice(&874u32.to_be_bytes());
        frame.extend_from_slice(&7u64.to_be_bytes());
        frame.extend_from_slice(b"\xff\xd8jpeg");
        let mut bytes = msg(b'J', br#"{"id":3,"ok":true}"#);
        bytes.extend(msg(b'F', &frame));
        let mut c = Cursor::new(bytes);
        assert_eq!(read_message(&mut c).unwrap(), Some(Message::Json(json!({"id":3,"ok":true}))));
        assert_eq!(read_message(&mut c).unwrap(), Some(Message::Frame(HelperFrame { seq: 7, width: 402, height: 874, jpeg: b"\xff\xd8jpeg".to_vec() })));
        assert_eq!(read_message(&mut c).unwrap(), None);
    }

    #[test]
    fn garbage_is_an_error_not_a_huge_allocation() {
        assert!(read_message(&mut Cursor::new(vec![0xff, 0xff, 0xff, 0xff, b'J'])).is_err());
        assert!(read_message(&mut Cursor::new(msg(b'Z', b"x"))).is_err());
        assert!(read_message(&mut Cursor::new(msg(b'F', b"short"))).is_err());
        assert!(read_message(&mut Cursor::new(vec![0, 0, 0, 0])).is_err());
    }

    #[test]
    fn replies_route_by_id_and_a_slow_puller_gets_the_newest_frame() {
        let ib = Inbox::default();
        let (tx, rx) = mpsc::channel();
        ib.pending.lock().unwrap().insert(5, tx);
        ib.dispatch(Message::Json(json!({"id": 5, "ok": true, "x": 1})));
        assert_eq!(rx.recv().unwrap()["x"], 1);
        let slot = Arc::new(FrameSlot::default());
        *ib.frames.lock().unwrap() = Some(slot.clone());
        for seq in 0..10 {
            ib.dispatch(Message::Frame(hf(seq)));
        }
        assert_eq!(slot.take(Duration::ZERO).unwrap().unwrap().seq, 9, "newest wins");
        assert_eq!(slot.take(Duration::from_millis(30)).unwrap(), None, "still screen: nothing new");
        ib.closed();
        assert!(slot.take(Duration::from_millis(30)).is_err(), "a closed helper ends the source");
    }

    #[test]
    fn a_waiting_puller_wakes_on_a_new_frame() {
        let slot = Arc::new(FrameSlot::default());
        let s2 = slot.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            s2.put(hf(1));
        });
        let start = Instant::now();
        assert_eq!(slot.take(Duration::from_secs(5)).unwrap().unwrap().seq, 1);
        assert!(start.elapsed() < Duration::from_secs(2));
        t.join().unwrap();
    }

    #[test]
    fn ready_checks_the_protocol_and_unsupported_xcode_is_named() {
        let ib = Inbox::default();
        let (tx, rx) = mpsc::channel();
        *ib.ready.lock().unwrap() = Some(tx);
        ib.dispatch(Message::Json(json!({"event": "ready", "version": 99})));
        assert_eq!(rx.recv().unwrap().unwrap_err().code, "helper_version");

        let (tx, rx) = mpsc::channel();
        *ib.ready.lock().unwrap() = Some(tx);
        ib.dispatch(Message::Json(json!({"event": "fatal", "code": "unsupported_xcode", "error": "Xcode 28.0 (1A1) lacks what K2's Simulator helper needs: AXPTranslator"})));
        let e = rx.recv().unwrap().unwrap_err();
        assert_eq!(e.code, "unsupported_xcode");
        assert!(e.message.contains("28.0") && e.message.contains("AXPTranslator"));
    }

    #[test]
    fn device_gone_marks_the_helper_dead_and_ends_frames() {
        let ib = Inbox::default();
        ib.alive.store(true, Ordering::SeqCst);
        let slot = Arc::new(FrameSlot::default());
        *ib.frames.lock().unwrap() = Some(slot.clone());
        ib.dispatch(Message::Json(json!({"event": "device_gone", "state": "Shutdown"})));
        assert!(!ib.alive.load(Ordering::SeqCst));
        assert!(ib.gone_reason.lock().unwrap().as_deref().unwrap().contains("Shutdown"));
        assert!(slot.take(Duration::ZERO).is_err());
    }

    fn swiftc_fake(ok: bool) -> FakeRunner {
        FakeRunner {
            calls: Mutex::new(vec![]),
            answer: Box::new(move |_, a| {
                if a[0] == "--find" {
                    return Ok(Output { status: if ok { 0 } else { 1 }, stdout: "/x/swiftc\n".into(), stderr: String::new() });
                }
                let o = a.iter().position(|x| x == "-o").unwrap();
                std::fs::write(&a[o + 1], b"fake-binary").unwrap();
                Ok(Output { status: 0, stdout: String::new(), stderr: String::new() })
            }),
        }
    }

    fn compiles(f: &FakeRunner) -> usize {
        f.calls.lock().unwrap().iter().filter(|(_, a)| a[0] == "swiftc").count()
    }

    #[test]
    fn the_build_cache_is_keyed_by_xcode_reused_and_rebuilt_when_tampered() {
        let dir = std::env::temp_dir().join(format!("k2-simhelper-build-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let fake = swiftc_fake(true);
        let a = ensure_built(&dir, &fake, "26.5 (17F42)").unwrap();
        assert_eq!(ensure_built(&dir, &fake, "26.5 (17F42)").unwrap(), a);
        assert_eq!(compiles(&fake), 1, "a matching binary is reused");
        let b = ensure_built(&dir, &fake, "26.6 (17F113)").unwrap();
        assert_ne!(a, b, "another Xcode gets its own build");
        assert_eq!(compiles(&fake), 2);
        std::fs::write(&a, b"tampered").unwrap();
        ensure_built(&dir, &fake, "26.5 (17F42)").unwrap();
        assert_eq!(compiles(&fake), 3, "a binary that no longer matches is rebuilt");
        assert_eq!(std::fs::read(&a).unwrap(), b"fake-binary");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_swiftc_says_how_to_get_it() {
        let dir = std::env::temp_dir().join(format!("k2-simhelper-noswift-{}", std::process::id()));
        let e = ensure_built(&dir, &swiftc_fake(false), "26.5").unwrap_err();
        assert_eq!(e.code, "no_swiftc");
        assert!(e.message.contains("xcode-select --install"), "{}", e.message);
    }

    #[test]
    fn a_failed_build_says_what_to_do() {
        let dir = std::env::temp_dir().join(format!("k2-simhelper-fail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let fake = FakeRunner {
            calls: Mutex::new(vec![]),
            answer: Box::new(|_, a| {
                if a[0] == "--find" {
                    return Ok(Output { status: 0, stdout: String::new(), stderr: String::new() });
                }
                Ok(Output { status: 1, stdout: String::new(), stderr: "x.swift:1: error: no such module 'AppKit'".into() })
            }),
        };
        let e = ensure_built(&dir, &fake, "26.5").unwrap_err();
        assert_eq!(e.code, "helper_build_failed");
        assert!(e.message.contains("Open Xcode once") && e.message.contains("no such module"), "{}", e.message);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
