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
//! line; stdout carries framed replies and events, and JPEG frames come on
//! their own pipe, mapped to the child's fd 3 (`--frames-fd 3`, protocol 3,
//! fix 3), so a reply never waits behind a 300–600 KB frame. A request with
//! `"nr": true` (a touch's down and moves) gets no reply; a failure comes
//! back as an `input_error` event instead (see the Swift file's header). No socket, so nothing else on the machine can
//! reach it, and no token to leak. It exits when its stdin closes or its
//! device stops. At start it checks every private class and symbol it uses
//! and refuses with `unsupported_xcode` (naming the Xcode) if one is gone.
//!
//! **Protocol 4** (fix 1, `k2_simvideo::link`): the helper encodes H.264
//! itself (VideoToolbox, straight from the framebuffer) and sends units on
//! fd 3. Its `ready` says which protocols it `accepts`; the gate picks the
//! newest both speak ([`gate`]), 3 when `K2_SIM_HELPER_ENCODE=0`. Then the
//! request says which: `frames` still gets protocol 3's JPEG frames (into a
//! [`FrameSlot`]), `layers` gets units (into a [`UnitQueue`]). One pipe
//! carries both across a switch: a unit's first byte is its codec (1-3), a
//! protocol 3 frame's is its width's high byte (0).

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::io::{BufWriter, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use k2_simvideo::link::{self, Codec, Encoder, LayerSpec, Request, Unit, UnitHeader};
use serde_json::{json, Value as J};
use sha2::{Digest, Sha256};

use super::simctl::{Runner, SimError, SimResult};

/// The helper's Swift source, compiled on first use.
pub const SOURCE: &str = include_str!("helper/k2-simhelper.swift");
/// The newest wire protocol the daemon speaks; the helper reports its own
/// in `ready`/`hello` (`version`, and `accepts` from 4).
/// 2: frames carry the helper's capture and emit times (M0).
/// 3: frames on fd 3 (`--frames-fd`), and `nr` requests answered only by
/// an `input_error` event when they fail (fix 3).
/// 4: H.264 units from the helper's own VideoToolbox encode (`layers`,
/// `keyframe`, `rate`; fix 1, `k2_simvideo::link`).
pub const PROTOCOL_VERSION: u32 = link::PROTOCOL_VERSION;
/// The oldest the daemon still accepts (JPEG frames on fd 3).
pub const OLDEST_PROTOCOL: u32 = link::OLDEST_PROTOCOL;
/// No message is larger than this (a 4K-ish JPEG is far below).
pub const MAX_MESSAGE: usize = 64 * 1024 * 1024;
/// How many of the helper's stderr lines are kept.
const STDERR_TAIL: usize = 20;

/// One JPEG frame as the helper sent it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperFrame {
    pub seq: u64,
    pub width: u32,
    pub height: u32,
    /// The IOSurface callback the frame answers, on the helper's clock
    /// (ns of uptime; [`HelperClock`] maps it to the daemon's).
    pub captured_ns: u64,
    /// The helper finished encoding it (same clock).
    pub emitted_ns: u64,
    /// The daemon read it off the pipe.
    pub received_at: Instant,
    pub jpeg: Vec<u8>,
}

/// The helper's clock (`hello.clock_ns`, ns of uptime) against the
/// daemon's, taken once when the hello reply arrives. Off by at most the
/// reply's trip through the pipe (well under a millisecond).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HelperClock {
    pub helper_ns: u64,
    pub at: Instant,
}

impl HelperClock {
    /// A helper time on the daemon's clock. Never after `limit` (a frame
    /// can't have been captured after the daemon read it).
    pub fn to_instant(&self, ns: u64, limit: Instant) -> Instant {
        let t = if ns >= self.helper_ns {
            self.at.checked_add(Duration::from_nanos(ns - self.helper_ns))
        } else {
            self.at.checked_sub(Duration::from_nanos(self.helper_ns - ns))
        };
        t.unwrap_or(limit).min(limit)
    }
}

/// The newest protocol the daemon asks a helper for: [`PROTOCOL_VERSION`],
/// or 3 with `K2_SIM_HELPER_ENCODE=0` (the helper then streams protocol 3's
/// JPEG frames, as before fix 1). Read once.
pub fn newest_protocol() -> u32 {
    static NEWEST: OnceLock<u32> = OnceLock::new();
    *NEWEST.get_or_init(|| newest_for(std::env::var("K2_SIM_HELPER_ENCODE").ok().as_deref()))
}

fn newest_for(encode: Option<&str>) -> u32 {
    if encode.map(str::trim) == Some("0") {
        OLDEST_PROTOCOL
    } else {
        PROTOCOL_VERSION
    }
}

/// The version gate on a helper's `ready`: the newest protocol both speak,
/// up to `newest`, or `helper_version`.
pub fn gate(ready: &J, newest: u32) -> SimResult<u32> {
    let theirs = link::spoken(ready);
    link::negotiate(OLDEST_PROTOCOL, newest, &theirs)
        .ok_or_else(|| SimError::new("helper_version", format!("the helper speaks protocol {theirs:?}, K2 wants {OLDEST_PROTOCOL} to {newest}")))
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

/// One message off the helper's stdout or frames pipe.
#[derive(Debug, PartialEq)]
pub enum Message {
    Json(J),
    /// Protocol 3: a JPEG frame (`frames`).
    Frame(HelperFrame),
    /// Protocol 4: a unit (`layers`).
    Unit(ReceivedUnit),
}

/// One protocol 4 unit as the daemon read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivedUnit {
    pub unit: Unit,
    /// The daemon read it off the pipe.
    pub received_at: Instant,
}

impl ReceivedUnit {
    /// When the helper captured and emitted it, on the daemon's clock
    /// (never after the read); without a clock, the read time.
    pub fn times(&self, clock: Option<HelperClock>) -> (Instant, Option<Instant>) {
        match clock {
            Some(c) => (c.to_instant(self.unit.header.captured_ns, self.received_at), Some(c.to_instant(self.unit.header.emitted_ns, self.received_at))),
            None => (self.received_at, None),
        }
    }
}

/// An `F` message's header: width, height, seq, capture ns, emit ns.
const FRAME_HEADER: usize = 32;

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
    if body[0] == link::KIND_UNIT && body.get(1).is_some_and(|&codec| codec != 0) {
        // Protocol 4: link.rs's unit header (codec byte 1-3). A protocol 3
        // frame starts with its width's high byte, 0.
        let header = UnitHeader::decode(&body[1..])?;
        body.drain(..1 + link::UNIT_HEADER_LEN);
        return Ok(Some(Message::Unit(ReceivedUnit { unit: Unit { header, data: body }, received_at: Instant::now() })));
    }
    let (kind, payload) = (body[0], &body[1..]);
    match kind {
        b'J' => serde_json::from_slice(payload)
            .map(|v| Some(Message::Json(v)))
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("helper JSON: {e}"))),
        b'F' if payload.len() >= FRAME_HEADER => Ok(Some(Message::Frame(HelperFrame {
            width: u32::from_be_bytes(payload[0..4].try_into().unwrap()),
            height: u32::from_be_bytes(payload[4..8].try_into().unwrap()),
            seq: u64::from_be_bytes(payload[8..16].try_into().unwrap()),
            captured_ns: u64::from_be_bytes(payload[16..24].try_into().unwrap()),
            emitted_ns: u64::from_be_bytes(payload[24..32].try_into().unwrap()),
            received_at: Instant::now(),
            jpeg: payload[FRAME_HEADER..].to_vec(),
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

/// Protocol 4 units off the frames pipe, in order and bounded. Units are
/// never dropped silently: when the queue is full, the unit's layer drops
/// everything until its next key unit that fits (each counted in
/// [`UnitQueue::dropped`]) and wants a keyframe ([`UnitQueue::take_key_wanted`]),
/// so a viewer never decodes a delta whose reference it didn't get.
pub struct UnitQueue {
    inner: Mutex<UnitQueueInner>,
    cv: Condvar,
    closed: AtomicBool,
    max_units: usize,
    max_bytes: usize,
}

#[derive(Default)]
struct UnitQueueInner {
    queue: VecDeque<ReceivedUnit>,
    bytes: usize,
    /// Layers dropping until their next key unit.
    skipping: BTreeSet<u8>,
    /// Layers that need a keyframe to resume, until taken.
    key_wanted: BTreeSet<u8>,
    dropped: u64,
}

impl Default for UnitQueue {
    fn default() -> Self {
        Self::new(UnitQueue::MAX_UNITS, UnitQueue::MAX_BYTES)
    }
}

impl UnitQueue {
    /// About 4 s at 60 fps.
    pub const MAX_UNITS: usize = 240;
    pub const MAX_BYTES: usize = 32 * 1024 * 1024;

    pub fn new(max_units: usize, max_bytes: usize) -> Self {
        Self { inner: Mutex::default(), cv: Condvar::new(), closed: AtomicBool::new(false), max_units, max_bytes }
    }

    pub fn push(&self, u: ReceivedUnit) {
        let mut g = self.inner.lock().unwrap();
        let layer = u.unit.header.layer;
        let fits = g.queue.len() < self.max_units && g.bytes + u.unit.data.len() <= self.max_bytes;
        if g.skipping.contains(&layer) {
            if !(u.unit.header.key && fits) {
                g.dropped += 1;
                if u.unit.header.key {
                    g.key_wanted.insert(layer); // the keyframe asked for didn't fit either
                }
                return;
            }
            g.skipping.remove(&layer);
        } else if !fits {
            g.dropped += 1;
            g.skipping.insert(layer);
            g.key_wanted.insert(layer);
            return;
        }
        g.bytes += u.unit.data.len();
        g.queue.push_back(u);
        drop(g);
        self.cv.notify_all();
    }

    /// Takes the oldest unit, waiting up to `wait`. `Ok(None)` = nothing
    /// yet; `Err(())` = the helper is gone (and the queue drained).
    pub fn take(&self, wait: Duration) -> Result<Option<ReceivedUnit>, ()> {
        let deadline = Instant::now() + wait;
        let mut g = self.inner.lock().unwrap();
        loop {
            if let Some(u) = g.queue.pop_front() {
                g.bytes -= u.unit.data.len();
                return Ok(Some(u));
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

    /// Units dropped on overflow, all time.
    pub fn dropped(&self) -> u64 {
        self.inner.lock().unwrap().dropped
    }

    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The layers that overflowed (or lost the keyframe that would end it)
    /// since the last call: each resumes only on a keyframe.
    pub fn take_key_wanted(&self) -> Vec<u8> {
        std::mem::take(&mut self.inner.lock().unwrap().key_wanted).into_iter().collect()
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.cv.notify_all();
    }
}

/// What the gate kept from the helper's `ready` (and `hello` replies).
#[derive(Debug, Clone, Default)]
struct HelloInfo {
    encoder: Option<Encoder>,
    codecs: Vec<Codec>,
    clock: Option<HelperClock>,
}

impl HelloInfo {
    /// The protocol 4 fields, read leniently (a protocol 3 helper has none).
    fn note(&mut self, v: &J) {
        if let Some(e) = v.get("encoder").and_then(|e| serde_json::from_value(e.clone()).ok()) {
            self.encoder = Some(e);
        }
        if let Some(c) = v.get("codecs").and_then(J::as_array) {
            self.codecs = c.iter().filter_map(|c| serde_json::from_value(c.clone()).ok()).collect();
        }
    }
}

/// Shared between the reader thread and callers.
#[derive(Default)]
struct Inbox {
    pending: Mutex<HashMap<u64, mpsc::Sender<J>>>,
    frames: Mutex<Option<Arc<FrameSlot>>>,
    units: Arc<UnitQueue>,
    ready: Mutex<Option<mpsc::Sender<SimResult<J>>>>,
    /// The newest protocol to accept (`None`: [`PROTOCOL_VERSION`]).
    cap: Option<u32>,
    /// What the gate chose (0 before `ready`).
    protocol: AtomicU32,
    hello: Mutex<HelloInfo>,
    alive: AtomicBool,
    gone_reason: Mutex<Option<String>>,
    /// The newest `input_error` (a fire-and-forget input the helper
    /// refused), until a gesture's `up` takes it.
    input_error: Mutex<Option<SimError>>,
}

impl Inbox {
    fn dispatch(&self, m: Message) {
        match m {
            Message::Frame(f) => {
                if let Some(slot) = self.frames.lock().unwrap().as_ref() {
                    slot.put(f);
                }
            }
            Message::Unit(u) => self.units.push(u),
            Message::Json(v) => {
                if let Some(id) = v.get("id").and_then(J::as_u64) {
                    if let Some(tx) = self.pending.lock().unwrap().remove(&id) {
                        let _ = tx.send(v);
                    }
                    return;
                }
                match v.get("event").and_then(J::as_str) {
                    Some("ready") => {
                        let r = gate(&v, self.cap.unwrap_or(PROTOCOL_VERSION)).map(|p| {
                            self.protocol.store(p, Ordering::SeqCst);
                            let mut h = self.hello.lock().unwrap();
                            h.note(&v);
                            h.clock = v.get("clock_ns").and_then(J::as_u64).map(|helper_ns| HelperClock { helper_ns, at: Instant::now() });
                            v.clone()
                        });
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
                    Some("input_error") => {
                        let code = helper_code(v.get("code").and_then(J::as_str).unwrap_or("helper_error"));
                        let op = v.get("op").and_then(J::as_str).unwrap_or("input");
                        let msg = v.get("error").and_then(J::as_str).unwrap_or("the helper refused");
                        *self.input_error.lock().unwrap() = Some(SimError::new(code, format!("{op}: {msg}")));
                    }
                    Some("device_gone") => {
                        self.alive.store(false, Ordering::SeqCst);
                        *self.gone_reason.lock().unwrap() = Some(format!("the device is {}", v.get("state").and_then(J::as_str).unwrap_or("stopped")));
                        if let Some(slot) = self.frames.lock().unwrap().as_ref() {
                            slot.close();
                        }
                        self.units.close();
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
        self.units.close();
        if let Some(tx) = self.ready.lock().unwrap().take() {
            let _ = tx.send(Err(SimError::new("helper_exited", "the Simulator helper exited before it was ready")));
        }
    }
}

/// What the lane needs from a helper; the seam unit tests fake.
pub trait HelperIo: Send + Sync {
    fn call(&self, op: &str, args: J, timeout: Duration) -> SimResult<J>;
    /// Send without waiting for a reply (fix 3: a touch's down and moves).
    /// A failure surfaces later through [`HelperIo::take_input_error`].
    /// Default: a waited `call` with the reply dropped.
    fn notify(&self, op: &str, args: J) -> SimResult<()> {
        self.call(op, args, Duration::from_secs(20)).map(|_| ())
    }
    /// The newest fire-and-forget failure since the last take.
    fn take_input_error(&self) -> Option<SimError> {
        None
    }
    /// Where frames go; `None` stops forwarding.
    fn frame_slot(&self, slot: Option<Arc<FrameSlot>>);
    fn alive(&self) -> bool;
    /// The protocol the gate chose: 3 = JPEG frames only, 4 = units too.
    fn protocol(&self) -> u32 {
        OLDEST_PROTOCOL
    }
    /// The helper's H.264 encoder, as it checked it (`hello.encoder`).
    fn encoder(&self) -> Encoder {
        Encoder::Unknown
    }
    /// What `layers` can encode, best first (`hello.codecs`; none before 4).
    fn codecs(&self) -> Vec<Codec> {
        vec![]
    }
    /// Protocol 4: the whole layer list (`nr`).
    fn set_layers(&self, _layers: &[LayerSpec]) -> SimResult<()> {
        Err(not_p4())
    }
    /// Protocol 4: a keyframe on `layer`, on the first frame captured at or
    /// after `since_ns` (the helper's clock; `None` = the next).
    fn request_keyframe(&self, _layer: u8, _since_ns: Option<u64>) -> SimResult<()> {
        Err(not_p4())
    }
    /// Protocol 4: new rate targets for a layer, no keyframe.
    fn set_rate(&self, _layer: u8, _fps: u32, _bitrate: u32) -> SimResult<()> {
        Err(not_p4())
    }
    /// Protocol 4: the units the helper sent.
    fn units(&self) -> Option<Arc<UnitQueue>> {
        None
    }
}

fn not_p4() -> SimError {
    SimError::new("helper_version", "this Simulator helper speaks protocol 3: JPEG frames only, no layers")
}

/// One `nr` request line (fix 3): no id, so no reply on success.
fn nr_line(op: &str, mut args: J) -> String {
    if !args.is_object() {
        args = json!({});
    }
    args["op"] = json!(op);
    args["nr"] = json!(true);
    format!("{args}\n")
}

/// A running helper for one device.
pub struct HelperProc {
    child: Mutex<Child>,
    stdin: Mutex<BufWriter<ChildStdin>>,
    inbox: Arc<Inbox>,
    next_id: AtomicU64,
    stderr_tail: Arc<Mutex<Vec<String>>>,
}

/// The child's end of the frames pipe goes to fd 3 (runs between fork and
/// exec: only async-signal-safe calls). `dup2` clears close-on-exec on the
/// new fd; when the pipe already is fd 3, clear it by hand.
fn frames_to_fd3(fd: RawFd) -> std::io::Result<()> {
    // SAFETY: plain fcntl / dup2 on fds this process owns, in the child.
    unsafe {
        if fd == 3 {
            let flags = libc::fcntl(3, libc::F_GETFD);
            if flags < 0 || libc::fcntl(3, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                return Err(std::io::Error::last_os_error());
            }
        } else if libc::dup2(fd, 3) < 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Give `cmd`'s child the write end of a new pipe as fd 3; returns the
/// read end (the caller drops its copy of the write end after spawning).
pub fn frames_pipe(cmd: &mut std::process::Command) -> std::io::Result<(std::io::PipeReader, std::io::PipeWriter)> {
    let (rx, tx) = std::io::pipe()?;
    let fd = tx.as_raw_fd();
    // SAFETY: the hook only calls `frames_to_fd3` (fcntl / dup2).
    unsafe {
        cmd.pre_exec(move || frames_to_fd3(fd));
    }
    Ok((rx, tx))
}

/// Read frames off the frames pipe into the inbox until it closes. The
/// helper's life is judged by stdout; this just stops.
fn read_frames(mut r: impl Read, ib: &Inbox) {
    loop {
        match read_message(&mut r) {
            Ok(Some(m @ (Message::Frame(_) | Message::Unit(_)))) => ib.dispatch(m),
            Ok(Some(Message::Json(_))) => {} // never sent there
            Ok(None) | Err(_) => return,
        }
    }
}

impl HelperProc {
    pub fn spawn(bin: &Path, set: &Path, udid: &str) -> SimResult<Self> {
        let mut cmd = crate::bg_proc::command(bin);
        cmd.arg("--set").arg(set).arg("--udid").arg(udid).arg("--frames-fd").arg("3").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let (frames_rx, frames_tx) = frames_pipe(&mut cmd).map_err(|e| SimError::new("helper_spawn_failed", format!("frames pipe: {e}")))?;
        let spawned = cmd.spawn();
        drop(frames_tx); // the child holds the only write end now
        let mut child = spawned.map_err(|e| SimError::new("helper_spawn_failed", format!("cannot start {}: {e}", bin.display())))?;
        let inbox = Arc::new(Inbox { cap: Some(newest_protocol()), ..Inbox::default() });
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

        let ib = inbox.clone();
        std::thread::Builder::new()
            .name(format!("k2-simframes-{}", &udid[..8.min(udid.len())]))
            .spawn(move || read_frames(frames_rx, &ib))
            .map_err(|e| SimError::new("helper_spawn_failed", format!("frames thread: {e}")))?;

        let stderr_tail = Arc::new(Mutex::new(Vec::new()));
        let stderr = child.stderr.take().expect("piped stderr");
        let tail = stderr_tail.clone();
        let _ = std::thread::Builder::new().name("k2-simhelper-err".into()).spawn(move || {
            // Line by line, so the tail (the encoder it chose, …) is there while it runs.
            use std::io::BufRead;
            for line in std::io::BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                let mut t = tail.lock().unwrap();
                if t.len() == STDERR_TAIL {
                    t.remove(0);
                }
                t.push(line);
            }
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
            if op == "hello" {
                self.inbox.hello.lock().unwrap().note(&reply);
            }
            Ok(reply)
        } else {
            let code = helper_code(reply.get("code").and_then(J::as_str).unwrap_or("helper_error"));
            Err(SimError::new(code, reply.get("error").and_then(J::as_str).unwrap_or("the helper refused").to_string()))
        }
    }

    /// Writes one `nr` request and returns once it is written: no id, no
    /// reply (fix 3). A refusal arrives as an `input_error` event.
    pub fn notify(&self, op: &str, args: J) -> SimResult<()> {
        if !self.alive() {
            let why = self.inbox.gone_reason.lock().unwrap().clone().unwrap_or_else(|| "it exited".into());
            return Err(SimError::new("helper_gone", format!("the Simulator helper stopped ({why}); run k2 sim boot")));
        }
        let line = nr_line(op, args);
        let mut w = self.stdin.lock().unwrap();
        w.write_all(line.as_bytes()).and_then(|_| w.flush()).map_err(|e| SimError::new("helper_gone", format!("cannot reach the Simulator helper: {e}")))
    }

    /// The helper's newest stderr lines, oldest first.
    pub fn stderr_tail(&self) -> Vec<String> {
        self.stderr_tail.lock().unwrap().clone()
    }

    /// The protocol the gate chose at `ready`.
    pub fn protocol(&self) -> u32 {
        self.inbox.protocol.load(Ordering::SeqCst)
    }

    /// The helper's clock against ours, taken when `ready` arrived.
    pub fn clock(&self) -> Option<HelperClock> {
        self.inbox.hello.lock().unwrap().clock
    }

    /// A protocol 4 request, `nr` (a refusal comes back as `input_error`).
    pub fn request(&self, r: &Request) -> SimResult<()> {
        if self.protocol() < 4 {
            return Err(not_p4());
        }
        let v = serde_json::to_value(r).map_err(|e| SimError::new("bad_request", format!("request JSON: {e}")))?;
        let op = v.get("op").and_then(J::as_str).unwrap_or_default().to_string();
        self.notify(&op, v)
    }

    /// The next unit, waiting up to `wait`. A layer the queue had to drop
    /// gets a keyframe request first, so it resumes. `Err(())` = gone.
    pub fn next_unit(&self, wait: Duration) -> Result<Option<ReceivedUnit>, ()> {
        for layer in self.inbox.units.take_key_wanted() {
            let _ = self.request(&Request::Keyframe { layer, since_ns: None });
        }
        self.inbox.units.take(wait)
    }

    /// Units dropped because the queue was full (see [`UnitQueue`]).
    pub fn units_dropped(&self) -> u64 {
        self.inbox.units.dropped()
    }
}

impl HelperIo for HelperProc {
    fn call(&self, op: &str, args: J, timeout: Duration) -> SimResult<J> {
        HelperProc::call(self, op, args, timeout)
    }
    fn notify(&self, op: &str, args: J) -> SimResult<()> {
        HelperProc::notify(self, op, args)
    }
    fn take_input_error(&self) -> Option<SimError> {
        self.inbox.input_error.lock().unwrap().take()
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
    fn protocol(&self) -> u32 {
        HelperProc::protocol(self)
    }
    fn encoder(&self) -> Encoder {
        self.inbox.hello.lock().unwrap().encoder.unwrap_or(Encoder::Unknown)
    }
    fn codecs(&self) -> Vec<Codec> {
        self.inbox.hello.lock().unwrap().codecs.clone()
    }
    fn set_layers(&self, layers: &[LayerSpec]) -> SimResult<()> {
        self.request(&Request::Layers { layers: layers.to_vec() })
    }
    fn request_keyframe(&self, layer: u8, since_ns: Option<u64>) -> SimResult<()> {
        self.request(&Request::Keyframe { layer, since_ns })
    }
    fn set_rate(&self, layer: u8, fps: u32, bitrate: u32) -> SimResult<()> {
        self.request(&Request::Rate { layer, fps, bitrate })
    }
    fn units(&self) -> Option<Arc<UnitQueue>> {
        (self.protocol() >= 4).then(|| self.inbox.units.clone())
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
        HelperFrame { seq, width: 1, height: 1, captured_ns: 0, emitted_ns: 0, received_at: Instant::now(), jpeg: vec![] }
    }

    #[test]
    fn messages_decode_json_frames_and_a_clean_end() {
        let mut frame = Vec::new();
        frame.extend_from_slice(&402u32.to_be_bytes());
        frame.extend_from_slice(&874u32.to_be_bytes());
        frame.extend_from_slice(&7u64.to_be_bytes());
        frame.extend_from_slice(&1_000u64.to_be_bytes());
        frame.extend_from_slice(&9_000u64.to_be_bytes());
        frame.extend_from_slice(b"\xff\xd8jpeg");
        let mut bytes = msg(b'J', br#"{"id":3,"ok":true}"#);
        bytes.extend(msg(b'F', &frame));
        let mut c = Cursor::new(bytes);
        assert_eq!(read_message(&mut c).unwrap(), Some(Message::Json(json!({"id":3,"ok":true}))));
        let before = Instant::now();
        let Some(Message::Frame(f)) = read_message(&mut c).unwrap() else { panic!("a frame") };
        assert_eq!((f.seq, f.width, f.height, f.captured_ns, f.emitted_ns), (7, 402, 874, 1_000, 9_000));
        assert_eq!(f.jpeg, b"\xff\xd8jpeg".to_vec());
        assert!(f.received_at >= before, "stamped when read");
        assert_eq!(read_message(&mut c).unwrap(), None);
    }

    #[test]
    fn garbage_is_an_error_not_a_huge_allocation() {
        assert!(read_message(&mut Cursor::new(vec![0xff, 0xff, 0xff, 0xff, b'J'])).is_err());
        assert!(read_message(&mut Cursor::new(msg(b'Z', b"x"))).is_err());
        assert!(read_message(&mut Cursor::new(msg(b'F', b"short"))).is_err());
        assert!(read_message(&mut Cursor::new(msg(b'F', &[0u8; 16]))).is_err(), "a protocol 1 frame header is too short");
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

    #[test]
    fn the_helper_clock_maps_onto_the_daemons_and_never_past_the_read() {
        let at = Instant::now();
        let c = HelperClock { helper_ns: 5_000_000_000, at };
        let limit = at + Duration::from_secs(1);
        assert_eq!(c.to_instant(5_000_000_000, limit), at);
        assert_eq!(c.to_instant(5_020_000_000, limit), at + Duration::from_millis(20));
        if let Some(earlier) = at.checked_sub(Duration::from_millis(7)) {
            assert_eq!(c.to_instant(4_993_000_000, limit), earlier);
        }
        assert_eq!(c.to_instant(9_000_000_000, limit), limit, "clamped to the read time");
    }

    #[test]
    fn an_input_error_is_kept_until_taken_and_frames_on_their_own_pipe_skip_json() {
        let ib = Inbox::default();
        ib.dispatch(Message::Json(json!({"event": "input_error", "op": "touch", "code": "no_hid_client", "error": "no HID client"})));
        let e = ib.input_error.lock().unwrap().take().expect("kept");
        assert_eq!(e.code, "unsupported_xcode");
        assert!(e.message.contains("touch") && e.message.contains("no HID client"), "{}", e.message);
        assert!(ib.input_error.lock().unwrap().is_none(), "taken once");
        // The frames pipe: frames land in the slot; JSON there is ignored
        // (replies only ever come on stdout).
        let slot = Arc::new(FrameSlot::default());
        *ib.frames.lock().unwrap() = Some(slot.clone());
        let (tx, rx) = mpsc::channel();
        ib.pending.lock().unwrap().insert(1, tx);
        let mut bytes = msg(b'J', br#"{"id":1,"ok":true}"#);
        bytes.extend(f_msg(4, 2, 1, b"\xff\xd8"));
        read_frames(Cursor::new(bytes), &ib);
        assert_eq!(slot.take(Duration::ZERO).unwrap().unwrap().seq, 4);
        assert!(rx.try_recv().is_err(), "a reply on the frames pipe is not a reply");
    }

    fn f_msg(seq: u64, w: u32, h: u32, jpeg: &[u8]) -> Vec<u8> {
        let mut p = Vec::new();
        p.extend_from_slice(&w.to_be_bytes());
        p.extend_from_slice(&h.to_be_bytes());
        p.extend_from_slice(&seq.to_be_bytes());
        p.extend_from_slice(&11u64.to_be_bytes());
        p.extend_from_slice(&22u64.to_be_bytes());
        p.extend_from_slice(jpeg);
        msg(b'F', &p)
    }

    fn sh_bytes(b: &[u8]) -> String {
        b.iter().map(|x| format!("\\{x:03o}")).collect()
    }

    /// Fix 3 end to end, with a shell script standing in for the helper:
    /// spawn passes `--frames-fd 3` and maps the frames pipe there; a frame
    /// written to fd 3 lands in the slot while JSON (ready, input_error)
    /// comes on stdout; `notify` writes `nr: true` with no id, waits for
    /// nothing, and leaves no pending reply.
    #[test]
    fn frames_come_on_fd_3_and_notify_is_fire_and_forget() {
        let dir = std::env::temp_dir().join(format!("k2-simhelper-fd3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("stdin.log");
        let ready = msg(b'J', br#"{"event":"ready","version":3}"#);
        let refused = msg(b'J', br#"{"event":"input_error","op":"touch","code":"no_hid_client","error":"no HID client"}"#);
        let script = format!(
            "#!/bin/sh\ncase \" $* \" in *\" --frames-fd 3 \"*) ;; *) echo no-frames-fd >&2; exit 9 ;; esac\nprintf '{}'\nIFS= read -r line\nprintf '%s\\n' \"$line\" >> '{}'\nprintf '{}' >&3\nprintf '{}'\ncat >> '{}'\n",
            sh_bytes(&ready),
            log.display(),
            sh_bytes(&f_msg(5, 2, 1, b"\xff\xd8jp")),
            sh_bytes(&refused),
            log.display()
        );
        let bin = dir.join("fake-helper.sh");
        std::fs::write(&bin, script).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();

        let h = HelperProc::spawn(&bin, Path::new("/no/set"), "UDID-TEST-0001").expect("spawn the stand-in");
        let slot = Arc::new(FrameSlot::default());
        h.frame_slot(Some(slot.clone()));
        let t0 = Instant::now();
        h.notify("touch", json!({"down": true, "x": 1.5, "y": 2.0})).expect("notify");
        assert!(t0.elapsed() < Duration::from_secs(1), "notify waits for nothing");
        assert!(h.inbox.pending.lock().unwrap().is_empty(), "no pending id");
        let f = slot.take(Duration::from_secs(5)).expect("helper alive").expect("a frame off fd 3");
        assert_eq!((f.seq, f.width, f.height, f.captured_ns, f.emitted_ns), (5, 2, 1, 11, 22));
        assert_eq!(f.jpeg, b"\xff\xd8jp".to_vec());
        let mut refused = None;
        for _ in 0..250 {
            refused = h.take_input_error();
            if refused.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(refused.expect("the input_error event").code, "unsupported_xcode");
        assert!(h.take_input_error().is_none());
        let first = std::fs::read_to_string(&log).unwrap();
        let line: J = serde_json::from_str(first.lines().next().expect("a line")).expect("JSON");
        assert_eq!((line["op"].as_str(), line["nr"].as_bool(), line["x"].as_f64()), (Some("touch"), Some(true), Some(1.5)));
        assert!(line.get("id").is_none(), "{line}");
        drop(h);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn unit(layer: u8, seq: u64, key: bool, data: &[u8]) -> ReceivedUnit {
        let header = UnitHeader { codec: Codec::H264, key, source_start: false, discardable: false, layer, source: 0, width: 1206, height: 2622, seq, captured_ns: 100 + seq, emitted_ns: 200 + seq };
        ReceivedUnit { unit: Unit { header, data: data.to_vec() }, received_at: Instant::now() }
    }

    fn unit_msg(u: &ReceivedUnit) -> Vec<u8> {
        let mut v = Vec::new();
        link::write_unit(&mut v, &u.unit.header, &u.unit.data).unwrap();
        v
    }

    fn ready_gate(ready: J, cap: Option<u32>) -> (SimResult<J>, Inbox) {
        let ib = Inbox { cap, ..Inbox::default() };
        let (tx, rx) = mpsc::channel();
        *ib.ready.lock().unwrap() = Some(tx);
        ib.dispatch(Message::Json(ready));
        (rx.recv().unwrap(), ib)
    }

    #[test]
    fn the_gate_takes_3_or_4_and_the_env_caps_it_at_3() {
        // A protocol 3 helper (no accepts): 3, and no protocol 4 fields.
        let (r, ib) = ready_gate(json!({"event": "ready", "version": 3, "xcode": "26.6"}), None);
        assert!(r.is_ok());
        assert_eq!(ib.protocol.load(Ordering::SeqCst), 3);
        assert_eq!(ib.hello.lock().unwrap().encoder, None);
        // A 3 + 4 helper: 4, with its encoder, codecs and clock.
        let ready = json!({"event": "ready", "version": 4, "accepts": [3, 4], "codecs": ["h264", "jpeg"], "encoder": "hardware", "clock_ns": 5_000});
        let (r, ib) = ready_gate(ready.clone(), None);
        assert_eq!(r.unwrap()["encoder"], "hardware");
        assert_eq!(ib.protocol.load(Ordering::SeqCst), 4);
        let h = ib.hello.lock().unwrap().clone();
        assert_eq!((h.encoder, h.codecs), (Some(Encoder::Hardware), vec![Codec::H264, Codec::Jpeg]));
        assert_eq!(h.clock.unwrap().helper_ns, 5_000);
        // K2_SIM_HELPER_ENCODE=0: the same helper is held to 3.
        assert_eq!(newest_for(Some("0")), 3);
        assert_eq!(newest_for(Some(" 0 ")), 3);
        assert_eq!(newest_for(Some("1")), 4);
        assert_eq!(newest_for(None), PROTOCOL_VERSION);
        let (r, ib) = ready_gate(ready, Some(newest_for(Some("0"))));
        assert!(r.is_ok());
        assert_eq!(ib.protocol.load(Ordering::SeqCst), 3);
        // A helper that speaks only 5: refused, naming the versions.
        let (r, ib) = ready_gate(json!({"event": "ready", "version": 5, "accepts": [5]}), None);
        let e = r.unwrap_err();
        assert_eq!(e.code, "helper_version");
        assert!(e.message.contains("[5]") && e.message.contains("3 to 4"), "{}", e.message);
        assert_eq!(ib.protocol.load(Ordering::SeqCst), 0);
        assert_eq!(gate(&json!({"version": 5, "accepts": [4, 5]}), 4).unwrap(), 4, "a newer helper steps down");
    }

    #[test]
    fn units_and_protocol_3_frames_share_the_frames_pipe() {
        let ib = Inbox::default();
        let slot = Arc::new(FrameSlot::default());
        *ib.frames.lock().unwrap() = Some(slot.clone());
        let key = ReceivedUnit { unit: Unit { header: UnitHeader { source_start: true, ..unit(0, 1, true, b"").unit.header }, data: b"\0\0\0\x01\x67sps\0\0\0\x01\x68pps\0\0\0\x01\x65idr".to_vec() }, received_at: Instant::now() };
        let mut bytes = unit_msg(&key);
        bytes.extend(f_msg(7, 402, 874, b"\xff\xd8"));   // a `frames` frame after a switch back
        bytes.extend(unit_msg(&unit(0, 2, false, b"\0\0\0\x01\x41p")));
        let jpeg = ReceivedUnit { unit: Unit { header: UnitHeader { codec: Codec::Jpeg, layer: 1, ..unit(1, 1, true, b"").unit.header }, data: b"\xff\xd8j".to_vec() }, received_at: Instant::now() };
        bytes.extend(unit_msg(&jpeg));
        let before = Instant::now();
        read_frames(Cursor::new(bytes), &ib);
        assert_eq!(slot.take(Duration::ZERO).unwrap().unwrap().seq, 7);
        let first = ib.units.take(Duration::ZERO).unwrap().unwrap();
        assert_eq!(first.unit, key.unit, "header and Annex B bytes as sent");
        assert!(first.received_at >= before);
        let second = ib.units.take(Duration::ZERO).unwrap().unwrap();
        assert_eq!((second.unit.header.seq, second.unit.header.key), (2, false));
        assert_eq!(ib.units.take(Duration::ZERO).unwrap().unwrap().unit.header.codec, Codec::Jpeg);
        assert_eq!(ib.units.take(Duration::ZERO).unwrap(), None);
        ib.closed();
        assert!(ib.units.take(Duration::ZERO).is_err(), "a closed helper ends the units");
        // A bad unit header (unknown codec byte) breaks the stream, as a torn frame does.
        let mut bad = unit_msg(&unit(0, 3, false, b"x"));
        bad[5] = 9;
        assert!(read_message(&mut Cursor::new(bad)).is_err());
    }

    #[test]
    fn unit_times_map_onto_the_daemons_clock() {
        let u = unit(0, 1, true, b"");
        let c = HelperClock { helper_ns: 100, at: u.received_at - Duration::from_millis(5) };
        let (cap, emit) = u.times(Some(c));
        assert_eq!(cap, c.at + Duration::from_nanos(1));
        assert_eq!(emit, Some(c.at + Duration::from_nanos(101)));
        assert_eq!(u.times(None), (u.received_at, None));
    }

    #[test]
    fn an_overflow_drops_a_layer_to_its_next_key_and_counts_it() {
        let q = UnitQueue::new(3, 1 << 20);
        q.push(unit(0, 1, true, b"k"));
        q.push(unit(0, 2, false, b"d"));
        q.push(unit(1, 1, true, b"k"));
        q.push(unit(0, 3, false, b"d")); // full: layer 0 drops from here
        assert_eq!((q.dropped(), q.len()), (1, 3));
        assert_eq!(q.take_key_wanted(), vec![0]);
        assert!(q.take_key_wanted().is_empty(), "taken once");
        assert_eq!(q.take(Duration::ZERO).unwrap().unwrap().unit.header.seq, 1);
        q.push(unit(0, 4, false, b"d")); // room now, but a delta after the gap: still dropped
        assert_eq!(q.dropped(), 2);
        q.push(unit(1, 2, false, b"d")); // the other layer is unaffected
        q.push(unit(0, 5, true, b"k")); // full again: the key is dropped too, and wanted again
        assert_eq!(q.dropped(), 3);
        assert_eq!(q.take_key_wanted(), vec![0]);
        let seqs: Vec<(u8, u64)> = std::iter::from_fn(|| q.take(Duration::ZERO).unwrap()).map(|u| (u.unit.header.layer, u.unit.header.seq)).collect();
        assert_eq!(seqs, vec![(0, 2), (1, 1), (1, 2)]);
        q.push(unit(0, 6, false, b"d"));
        q.push(unit(0, 7, true, b"k")); // the key that fits ends the drop
        q.push(unit(0, 8, false, b"d"));
        let seqs: Vec<u64> = std::iter::from_fn(|| q.take(Duration::ZERO).unwrap()).map(|u| u.unit.header.seq).collect();
        assert_eq!(seqs, vec![7, 8]);
        assert_eq!(q.dropped(), 4);
        // The byte bound counts too.
        let q = UnitQueue::new(100, 4);
        q.push(unit(2, 1, true, b"kkk"));
        q.push(unit(2, 2, false, b"dd"));
        assert_eq!((q.dropped(), q.len()), (1, 1));
    }

    #[test]
    fn protocol_4_requests_have_stable_lines() {
        let line = |r: &Request| -> J {
            let v = serde_json::to_value(r).unwrap();
            let op = v["op"].as_str().unwrap().to_string();
            let l = nr_line(&op, v);
            assert!(l.ends_with('\n') && !l[..l.len() - 1].contains('\n'), "one line: {l:?}");
            serde_json::from_str(&l).unwrap()
        };
        let layers = Request::Layers {
            layers: vec![
                LayerSpec { id: 0, codec: Codec::H264, max_width: 0, fps: 60, bitrate: 12_000_000, quality: None, on: true },
                LayerSpec { id: 1, codec: Codec::Jpeg, max_width: 402, fps: 15, bitrate: 0, quality: Some(70), on: false },
            ],
        };
        assert_eq!(
            line(&layers),
            json!({"op": "layers", "nr": true, "layers": [
                {"id": 0, "codec": "h264", "max_width": 0, "fps": 60, "bitrate": 12_000_000, "on": true},
                {"id": 1, "codec": "jpeg", "max_width": 402, "fps": 15, "bitrate": 0, "quality": 70, "on": false},
            ]})
        );
        assert_eq!(line(&Request::Keyframe { layer: 0, since_ns: Some(9) }), json!({"op": "keyframe", "nr": true, "layer": 0, "since_ns": 9}));
        assert_eq!(line(&Request::Keyframe { layer: 2, since_ns: None }), json!({"op": "keyframe", "nr": true, "layer": 2}));
        assert_eq!(line(&Request::Rate { layer: 0, fps: 30, bitrate: 2_000_000 }), json!({"op": "rate", "nr": true, "layer": 0, "fps": 30, "bitrate": 2_000_000}));
    }

    /// Protocol 4 end to end, with a shell script standing in for the
    /// helper: the gate picks 4 from `accepts`, the hello fields are kept,
    /// `set_layers` writes one `nr` line, a unit written to fd 3 comes out of
    /// `next_unit`, and a protocol 3 helper refuses the protocol 4 calls.
    #[test]
    fn a_protocol_4_helper_sends_units_on_fd_3() {
        let dir = std::env::temp_dir().join(format!("k2-simhelper-p4-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("stdin.log");
        let ready = msg(b'J', br#"{"event":"ready","version":4,"accepts":[3,4],"codecs":["h264","jpeg"],"encoder":"hardware","clock_ns":1000}"#);
        let u = unit(0, 1, true, b"\0\0\0\x01\x65");
        let script = format!(
            "#!/bin/sh\nprintf '{}'\nIFS= read -r line\nprintf '%s\\n' \"$line\" >> '{}'\nprintf '{}' >&3\ncat >> '{}'\n",
            sh_bytes(&ready),
            log.display(),
            sh_bytes(&unit_msg(&u)),
            log.display()
        );
        let bin = dir.join("fake-helper-p4.sh");
        std::fs::write(&bin, script).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();

        let h = HelperProc::spawn(&bin, Path::new("/no/set"), "UDID-TEST-0004").expect("spawn the stand-in");
        assert_eq!(h.protocol(), newest_protocol(), "4 unless K2_SIM_HELPER_ENCODE=0 is set for the test run");
        if h.protocol() < 4 {
            drop(h);
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        assert_eq!((HelperIo::encoder(&h), HelperIo::codecs(&h)), (Encoder::Hardware, vec![Codec::H264, Codec::Jpeg]));
        assert_eq!(h.clock().unwrap().helper_ns, 1000);
        assert!(HelperIo::units(&h).is_some());
        let spec = LayerSpec { id: 0, codec: Codec::H264, max_width: 0, fps: 60, bitrate: 8_000_000, quality: None, on: true };
        h.set_layers(&[spec]).expect("layers");
        let got = h.next_unit(Duration::from_secs(5)).expect("helper alive").expect("a unit off fd 3");
        assert_eq!(got.unit, u.unit);
        assert_eq!(h.units_dropped(), 0);
        let first = std::fs::read_to_string(&log).unwrap();
        let line: J = serde_json::from_str(first.lines().next().expect("a line")).expect("JSON");
        assert_eq!(line, json!({"op": "layers", "nr": true, "layers": [{"id": 0, "codec": "h264", "max_width": 0, "fps": 60, "bitrate": 8_000_000, "on": true}]}));
        drop(h);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_protocol_3_helper_refuses_protocol_4_calls() {
        struct Old;
        impl HelperIo for Old {
            fn call(&self, _op: &str, _args: J, _t: Duration) -> SimResult<J> {
                Ok(json!({}))
            }
            fn frame_slot(&self, _s: Option<Arc<FrameSlot>>) {}
            fn alive(&self) -> bool {
                true
            }
        }
        assert_eq!(Old.protocol(), 3);
        assert_eq!(Old.set_layers(&[]).unwrap_err().code, "helper_version");
        assert_eq!(Old.request_keyframe(0, None).unwrap_err().code, "helper_version");
        assert_eq!(Old.set_rate(0, 30, 1).unwrap_err().code, "helper_version");
        assert!(Old.units().is_none() && Old.codecs().is_empty() && Old.encoder() == Encoder::Unknown);
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
