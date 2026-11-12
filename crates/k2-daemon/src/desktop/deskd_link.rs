//! The daemon's link to `k2-deskd` ("K2 Screen"), D3.
//!
//! [`DeskdManager`] is the small interface the desktop routes and the
//! stream call: [`enable`](DeskdManager::enable) /
//! [`disable`](DeskdManager::disable), [`state`](DeskdManager::state),
//! [`request_permission`](DeskdManager::request_permission),
//! [`inject`](DeskdManager::inject), [`release_all`](DeskdManager::release_all),
//! [`banner`](DeskdManager::banner), [`displays`](DeskdManager::displays),
//! [`idle`](DeskdManager::idle), [`subscribe`](DeskdManager::subscribe) and
//! [`take_frames`](DeskdManager::take_frames).
//!
//! - **macOS:** `enable` installs the `dev.k2.deskd` LaunchAgent
//!   ([`super::launch_agent`]) and connects to `~/.k2/run/deskd.sock`. deskd
//!   checks the daemon's code signature, sends `hello`, then the frames
//!   pipe's read end over `SCM_RIGHTS`. A standalone macOS daemon (no
//!   `K2 Screen.app` beside it) reports `needs_app` (vs-live R7).
//! - **Linux:** `enable` arms a child: the daemon finds the console
//!   session's `DISPLAY` / `XAUTHORITY` / `WAYLAND_DISPLAY` /
//!   `DBUS_SESSION_BUS_ADDRESS` (`systemctl --user show-environment`, then
//!   `loginctl`), and spawns deskd with only those (plus `PATH`, `HOME`)
//!   over pipes, frames on fd 3. Release Linux daemons embed deskd (like
//!   k2-ptyd) and place it at `~/.k2/bin/deskd/<ver>-<sha12>/k2-deskd`.
//!
//! **Version gate (§3.3).** A hello whose protocol isn't
//! [`PROTOCOL_VERSION`] (or whose role isn't `desktop`) gets one restart
//! (`launchctl kickstart -k`, or a fresh child); a second mismatch is
//! `needs_update`.
//!
//! **Reconnects.** A supervisor thread runs while enabled: when the link
//! drops (deskd crashed; launchd's KeepAlive restarts it), it reconnects.
//! The link closing makes deskd release every held key and hide the
//! banner, so a dead daemon never leaves input stuck.
//!
//! Std threads, no tokio: the stream (D6) reads frames on its own thread.
//! The wire is K2 Render's protocol 4 (`k2-deskd-proto` re-exports
//! `k2_simvideo::link`): the frames pipe carries protocol-4 units for the
//! engine's `LinkSource`, which sends its `layers` / `keyframe` / `rate`
//! through [`DeskdManager::send_link`].

#![cfg_attr(not(unix), allow(unused_imports, dead_code))]

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value as J};

use k2_deskd_proto::msg::{Display, Event, Hello, InputEvent, Op, ReleaseReason, Request, Tcc};
use k2_deskd_proto::wire::{self, Message};
use k2_deskd_proto::{PROTOCOL_VERSION, ROLE};

use super::launch_agent::{Output, Runner, SystemRunner};

include!(concat!(env!("OUT_DIR"), "/deskd_embed.rs"));

/// Debug builds only: run this k2-deskd (Linux) or this `K2 Screen.app`
/// executable (macOS) instead of the shipped one.
pub const ENV_BIN: &str = "K2_DESKD_BIN";
pub const CALL_TIMEOUT: Duration = Duration::from_secs(5);
pub const HELLO_TIMEOUT: Duration = Duration::from_secs(10);

fn debug_env(name: &str) -> Option<String> {
    if !cfg!(debug_assertions) {
        return None;
    }
    std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// What the desktop routes show (§8's states, plus `starting` and `ready`;
/// `ready` means deskd is up, not that anyone is watching).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DeskdState {
    Off,
    Starting,
    NeedsPermission { screen: bool, accessibility: bool },
    Ready { backend: String },
    NeedsUpdate { found: Option<u64> },
    NeedsApp,
    BlockedInLoginItems,
    NoSession,
    NoDisplay,
    Failed { error: String },
}

impl DeskdState {
    pub fn name(&self) -> &'static str {
        match self {
            DeskdState::Off => "off",
            DeskdState::Starting => "starting",
            DeskdState::NeedsPermission { .. } => "needs_permission",
            DeskdState::Ready { .. } => "ready",
            DeskdState::NeedsUpdate { .. } => "needs_update",
            DeskdState::NeedsApp => "needs_app",
            DeskdState::BlockedInLoginItems => "blocked_in_login_items",
            DeskdState::NoSession => "no_session",
            DeskdState::NoDisplay => "no_display",
            DeskdState::Failed { .. } => "failed",
        }
    }

    /// The person-facing line (§8 copy where §8 has one).
    pub fn message(&self) -> String {
        match self {
            DeskdState::Off => "Remote desktop is off on this computer.".into(),
            DeskdState::Starting => "Starting K2 Screen…".into(),
            DeskdState::NeedsPermission { .. } => "Someone at this Mac has to allow K2 Screen once.".into(),
            DeskdState::Ready { .. } => "Ready.".into(),
            DeskdState::NeedsUpdate { .. } => "K2 Screen and K2 don't match; restarting it.".into(),
            DeskdState::NeedsApp => "Install the K2 app on this Mac to use Remote desktop.".into(),
            DeskdState::BlockedInLoginItems => "Turn K2 Screen on in System Settings → General → Login Items.".into(),
            DeskdState::NoSession => "No one is logged in on this computer's screen.".into(),
            DeskdState::NoDisplay => "This computer has no screen K2 can show.".into(),
            DeskdState::Failed { error } => format!("K2 Screen failed: {error}"),
        }
    }

    fn from_hello(h: &Hello) -> DeskdState {
        if !h.tcc.granted() {
            DeskdState::NeedsPermission { screen: h.tcc.screen, accessibility: h.tcc.accessibility }
        } else if h.backend == "none" {
            DeskdState::NoDisplay
        } else {
            DeskdState::Ready { backend: h.backend.clone() }
        }
    }
}

/// The S14-style TCC card copy (§3.2 step 2), for the routes.
pub const TCC_CARD: &str = "K2 Screen needs Screen Recording (for screen sharing in calls and for Remote desktop) and Accessibility (to type and click for you). Someone at this Mac has to allow both once.";

#[derive(Debug, Clone, PartialEq)]
pub enum DeskdEvent {
    State(DeskdState),
    /// The person at the machine is using it (`hold`), or stopped for 3 s.
    LocalInput { hold: bool },
    /// Disconnect on the banner: close every viewer.
    BannerDisconnect,
    InputError { op: String, code: String, error: String },
    /// A stream layer can't run (a codec deskd doesn't make here, or its
    /// encoder failed): the stream's hub should move that layer's viewers
    /// (`SourceError::LayerFailed`).
    LayerFailed { layer: u8, code: String, error: String },
    /// deskd's capture stopped or couldn't start (`no_screen_permission`,
    /// `no_such_display`, `capture_failed`); it retries on its own.
    CaptureError { display: u32, code: String, error: String },
    /// An event this daemon's protocol module doesn't model yet, passed on
    /// whole for the stream (D6): `cursor`, `cursor_pos`,
    /// `displays_changed`, `locked` (deskd adds them with the out-of-band
    /// cursor and the lock check).
    Other(J),
}

/// deskd events the stream relays without this module modelling them.
pub const RELAYED_EVENTS: &[&str] = &["cursor", "cursor_pos", "displays_changed", "locked"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeskdError {
    pub code: String,
    pub message: String,
}

impl DeskdError {
    fn new(code: &str, message: impl Into<String>) -> Self {
        DeskdError { code: code.into(), message: message.into() }
    }
}

impl std::fmt::Display for DeskdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

/// One link as a transport opened it, `hello` already read.
pub struct Opened {
    pub hello: J,
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
    pub frames: Option<File>,
    /// Kept for the link's life (a Linux child, killed on drop).
    pub guard: Option<Box<dyn Send>>,
    /// Hangs the link up (a socket's `shutdown`), so deskd sees it close
    /// even while the reader thread still holds the stream.
    pub close: Option<Box<dyn Fn() + Send + Sync>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    /// Not up yet (socket missing or refusing): retry soon.
    NotYet(String),
    NoSession,
    NoDisplay,
    NeedsApp,
    Blocked,
    /// deskd closed before hello: its peer check refused this daemon.
    Refused(String),
    Failed(String),
}

impl OpenError {
    fn state(&self) -> DeskdState {
        match self {
            OpenError::NotYet(_) => DeskdState::Starting,
            OpenError::NoSession => DeskdState::NoSession,
            OpenError::NoDisplay => DeskdState::NoDisplay,
            OpenError::NeedsApp => DeskdState::NeedsApp,
            OpenError::Blocked => DeskdState::BlockedInLoginItems,
            OpenError::Refused(m) | OpenError::Failed(m) => DeskdState::Failed { error: m.clone() },
        }
    }

    fn retry_after(&self) -> Duration {
        match self {
            OpenError::NotYet(_) => Duration::from_millis(500),
            OpenError::NeedsApp => Duration::from_secs(60),
            _ => Duration::from_secs(10),
        }
    }
}

/// How deskd is started and reached on this OS.
pub trait Transport: Send + Sync {
    /// Arm it (install the LaunchAgent; nothing on Linux).
    fn start(&self) -> Result<(), OpenError>;
    fn open(&self) -> Result<Opened, OpenError>;
    /// After a version mismatch: make the next `open` reach a fresh deskd.
    fn restart(&self) -> Result<(), OpenError>;
    /// Disarm (boot the LaunchAgent out).
    fn stop(&self) -> Result<(), OpenError>;
}

struct Link {
    writer: Mutex<Box<dyn Write + Send>>,
    pending: Mutex<HashMap<u64, mpsc::Sender<J>>>,
    next_id: AtomicU64,
    alive: AtomicBool,
    hello: Hello,
    /// When the hello was read (the stream's clock map, with its
    /// `clock_ns`).
    hello_at: Instant,
    guard: Mutex<Option<Box<dyn Send>>>,
    close: Option<Box<dyn Fn() + Send + Sync>>,
}

impl Link {
    /// Hangs up: deskd releases held input and hides the banner when it
    /// sees the link close; a Linux child is stopped.
    fn close(&self) {
        self.alive.store(false, Ordering::SeqCst);
        if let Some(c) = &self.close {
            c();
        }
        drop(self.guard.lock().unwrap().take());
    }

    fn send(&self, req: &Request) -> io::Result<()> {
        let mut w = self.writer.lock().unwrap();
        wire::write_json(&mut *w, req)
    }

    fn send_json(&self, v: &J) -> io::Result<()> {
        let mut w = self.writer.lock().unwrap();
        wire::write_json(&mut *w, v)
    }
}

struct Inner {
    enabled: bool,
    /// Bumped by enable/disable; a supervisor whose generation is stale
    /// stops.
    gen: u64,
    state: DeskdState,
    link: Option<Arc<Link>>,
    frames: Option<File>,
    mismatches: u32,
}

pub struct DeskdManager {
    transport: Arc<dyn Transport>,
    inner: Mutex<Inner>,
    changed: Condvar,
    subs: Mutex<Vec<mpsc::Sender<DeskdEvent>>>,
}

impl DeskdManager {
    pub fn new(transport: Arc<dyn Transport>) -> Arc<Self> {
        Arc::new(DeskdManager {
            transport,
            inner: Mutex::new(Inner { enabled: false, gen: 0, state: DeskdState::Off, link: None, frames: None, mismatches: 0 }),
            changed: Condvar::new(),
            subs: Mutex::new(Vec::new()),
        })
    }

    pub fn state(&self) -> DeskdState {
        self.inner.lock().unwrap().state.clone()
    }

    pub fn enabled(&self) -> bool {
        self.inner.lock().unwrap().enabled
    }

    /// deskd's hello on the live link.
    pub fn hello(&self) -> Option<Hello> {
        self.inner.lock().unwrap().link.as_ref().map(|l| l.hello.clone())
    }

    pub fn subscribe(&self) -> mpsc::Receiver<DeskdEvent> {
        let (tx, rx) = mpsc::channel();
        self.subs.lock().unwrap().push(tx);
        rx
    }

    fn broadcast(&self, e: DeskdEvent) {
        self.subs.lock().unwrap().retain(|s| s.send(e.clone()).is_ok());
    }

    fn set_state_locked(&self, g: &mut Inner, s: DeskdState) {
        if g.state != s {
            g.state = s.clone();
            self.changed.notify_all();
            self.broadcast(DeskdEvent::State(s));
        }
    }

    fn set_state(&self, s: DeskdState) {
        let mut g = self.inner.lock().unwrap();
        self.set_state_locked(&mut g, s);
    }

    /// Turns deskd on and waits up to `wait` for it to settle (anything
    /// but `starting`). Idempotent: a second enable keeps the running link.
    pub fn enable(self: &Arc<Self>, wait: Duration) -> DeskdState {
        let gen = {
            let mut g = self.inner.lock().unwrap();
            if g.enabled {
                drop(g);
                return self.wait_settled(wait);
            }
            g.enabled = true;
            g.gen += 1;
            g.mismatches = 0;
            self.set_state_locked(&mut g, DeskdState::Starting);
            g.gen
        };
        let me = Arc::clone(self);
        std::thread::Builder::new().name("k2-deskd-supervisor".into()).spawn(move || me.supervise(gen)).expect("spawn deskd supervisor");
        self.wait_settled(wait)
    }

    /// Waits until the state isn't `starting`, up to `wait`.
    pub fn wait_settled(&self, wait: Duration) -> DeskdState {
        let deadline = Instant::now() + wait;
        let mut g = self.inner.lock().unwrap();
        while g.state == DeskdState::Starting {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            g = self.changed.wait_timeout(g, left).unwrap().0;
        }
        g.state.clone()
    }

    /// Turns deskd off: closes the link (deskd releases held input and
    /// hides the banner), stops it, and drops the frames pipe.
    pub fn disable(&self) -> Result<(), DeskdError> {
        let link = {
            let mut g = self.inner.lock().unwrap();
            g.enabled = false;
            g.gen += 1;
            g.frames = None;
            self.set_state_locked(&mut g, DeskdState::Off);
            g.link.take()
        };
        if let Some(l) = link {
            l.close();
        }
        self.transport.stop().map_err(|e| DeskdError::new("deskd_stop_failed", e.state().message()))
    }

    fn live_gen(&self, gen: u64) -> bool {
        let g = self.inner.lock().unwrap();
        g.enabled && g.gen == gen
    }

    fn supervise(self: Arc<Self>, gen: u64) {
        let mut started = false;
        while self.live_gen(gen) {
            let has_link = self.inner.lock().unwrap().link.as_ref().is_some_and(|l| l.alive.load(Ordering::SeqCst));
            if has_link {
                std::thread::sleep(Duration::from_millis(250));
                continue;
            }
            if !started {
                match self.transport.start() {
                    Ok(()) => started = true,
                    Err(e) => {
                        self.set_state(e.state());
                        self.nap(gen, e.retry_after());
                        continue;
                    }
                }
            }
            match self.transport.open() {
                Ok(o) => {
                    if !self.live_gen(gen) {
                        return; // disabled while opening: drop it
                    }
                    if let Some(wait) = self.accept(o, gen) {
                        self.nap(gen, wait);
                    }
                }
                Err(e) => {
                    self.set_state(e.state());
                    self.nap(gen, e.retry_after());
                }
            }
        }
    }

    /// Sleeps up to `d`, waking early on disable.
    fn nap(&self, gen: u64, d: Duration) {
        let deadline = Instant::now() + d;
        let mut g = self.inner.lock().unwrap();
        while g.enabled && g.gen == gen {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return;
            }
            g = self.changed.wait_timeout(g, left).unwrap().0;
        }
    }

    /// Takes an opened link. Returns how long to wait before the next try
    /// when it was refused (version mismatch).
    fn accept(self: &Arc<Self>, o: Opened, gen: u64) -> Option<Duration> {
        let version = o.hello.get("version").and_then(J::as_u64);
        let role = o.hello.get("role").and_then(J::as_str).map(str::to_string);
        let hello: Option<Hello> = match serde_json::from_value::<Event>(o.hello.clone()) {
            Ok(Event::Hello(h)) => Some(h),
            _ => None,
        };
        let matches = version == Some(PROTOCOL_VERSION as u64) && role.as_deref() == Some(ROLE) && hello.is_some();
        if !matches {
            drop(o);
            let n = {
                let mut g = self.inner.lock().unwrap();
                g.mismatches += 1;
                g.mismatches
            };
            if n == 1 {
                let _ = self.transport.restart();
                return Some(Duration::from_secs(1));
            }
            self.set_state(DeskdState::NeedsUpdate { found: version });
            return Some(Duration::from_secs(30));
        }
        let hello = hello.expect("checked");
        let hello_at = Instant::now();
        let link = Arc::new(Link {
            hello_at,
            writer: Mutex::new(o.writer),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            alive: AtomicBool::new(true),
            hello: hello.clone(),
            guard: Mutex::new(o.guard),
            close: o.close,
        });
        {
            let mut g = self.inner.lock().unwrap();
            if !(g.enabled && g.gen == gen) {
                drop(g);
                link.close();
                return None;
            }
            g.mismatches = 0;
            g.link = Some(Arc::clone(&link));
            g.frames = o.frames;
            self.set_state_locked(&mut g, DeskdState::from_hello(&hello));
        }
        let me = Arc::clone(self);
        let mut reader = o.reader;
        let l2 = Arc::clone(&link);
        std::thread::Builder::new()
            .name("k2-deskd-link".into())
            .spawn(move || {
                loop {
                    if !l2.alive.load(Ordering::SeqCst) {
                        break;
                    }
                    match wire::read_message(&mut reader) {
                        Ok(Some(Message::Json(v))) => me.on_message(&l2, v),
                        Ok(Some(Message::Unit(_))) | Ok(None) | Err(_) => break,
                    }
                }
                me.link_ended(&l2);
            })
            .expect("spawn deskd link reader");
        None
    }

    fn on_message(&self, link: &Arc<Link>, v: J) {
        if let Some(id) = v.get("id").and_then(J::as_u64) {
            if let Some(tx) = link.pending.lock().unwrap().remove(&id) {
                let _ = tx.send(v);
            }
            return;
        }
        let relayed = v.get("event").and_then(J::as_str).is_some_and(|e| RELAYED_EVENTS.contains(&e));
        if relayed {
            self.broadcast(DeskdEvent::Other(v));
            return;
        }
        let Ok(ev) = serde_json::from_value::<Event>(v) else { return };
        match ev {
            Event::TccChanged { tcc } => {
                let mut g = self.inner.lock().unwrap();
                let backend = g.link.as_ref().map(|l| l.hello.backend.clone()).unwrap_or_default();
                let mut h = link.hello.clone();
                h.tcc = tcc;
                h.backend = backend;
                self.set_state_locked(&mut g, DeskdState::from_hello(&h));
            }
            Event::LocalInput { hold } => self.broadcast(DeskdEvent::LocalInput { hold }),
            Event::BannerDisconnect => self.broadcast(DeskdEvent::BannerDisconnect),
            Event::InputError { op, code, error } => self.broadcast(DeskdEvent::InputError { op, code, error }),
            Event::Fatal { code, error } => self.set_state(DeskdState::Failed { error: format!("{code}: {error}") }),
            Event::LayerFailed { layer, code, error } => self.broadcast(DeskdEvent::LayerFailed { layer, code, error }),
            Event::CaptureError { display, code, error } => self.broadcast(DeskdEvent::CaptureError { display, code, error }),
            Event::Hello(_) => {}
        }
    }

    fn link_ended(&self, link: &Arc<Link>) {
        link.close();
        link.pending.lock().unwrap().clear();
        let mut g = self.inner.lock().unwrap();
        if g.link.as_ref().is_some_and(|l| Arc::ptr_eq(l, link)) {
            g.link = None;
            g.frames = None;
            if g.enabled {
                self.set_state_locked(&mut g, DeskdState::Starting);
            }
        }
        self.changed.notify_all();
    }

    fn link(&self) -> Result<Arc<Link>, DeskdError> {
        let g = self.inner.lock().unwrap();
        if !g.enabled {
            return Err(DeskdError::new("desktop_off", DeskdState::Off.message()));
        }
        g.link.clone().filter(|l| l.alive.load(Ordering::SeqCst)).ok_or_else(|| DeskdError::new("deskd_not_ready", g.state.message()))
    }

    /// One request and its reply. A reply with `ok: false` is an error with
    /// deskd's code.
    pub fn call(&self, op: Op, timeout: Duration) -> Result<J, DeskdError> {
        let link = self.link()?;
        let id = link.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel();
        link.pending.lock().unwrap().insert(id, tx);
        if let Err(e) = link.send(&Request { id: Some(id), nr: false, op }) {
            link.pending.lock().unwrap().remove(&id);
            return Err(DeskdError::new("deskd_gone", format!("cannot reach K2 Screen: {e}")));
        }
        match rx.recv_timeout(timeout) {
            Ok(v) if v.get("ok") == Some(&J::Bool(true)) => Ok(v),
            Ok(v) => Err(DeskdError::new(v.get("code").and_then(J::as_str).unwrap_or("deskd_error"), v.get("error").and_then(J::as_str).unwrap_or("K2 Screen refused"))),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                link.pending.lock().unwrap().remove(&id);
                Err(DeskdError::new("deskd_timeout", format!("K2 Screen didn't answer within {} s", timeout.as_secs())))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(DeskdError::new("deskd_gone", "K2 Screen stopped")),
        }
    }

    /// deskd's hello on the live link, and when it was read.
    pub fn hello_at(&self) -> Option<(Hello, Instant)> {
        self.inner.lock().unwrap().link.as_ref().map(|l| (l.hello.clone(), l.hello_at))
    }

    /// [`Self::call`] for a request this module doesn't model (`op` is a
    /// JSON object with its `op`; the id is added here): the stream's
    /// `clipboard_get` and `clipboard_set`.
    pub fn call_json(&self, mut op: J, timeout: Duration) -> Result<J, DeskdError> {
        let link = self.link()?;
        let id = link.next_id.fetch_add(1, Ordering::SeqCst);
        let Some(o) = op.as_object_mut() else { return Err(DeskdError::new("bad_request", "a request is a JSON object")) };
        o.insert("id".into(), J::from(id));
        o.remove("nr");
        let (tx, rx) = mpsc::channel();
        link.pending.lock().unwrap().insert(id, tx);
        if let Err(e) = link.send_json(&op) {
            link.pending.lock().unwrap().remove(&id);
            return Err(DeskdError::new("deskd_gone", format!("cannot reach K2 Screen: {e}")));
        }
        match rx.recv_timeout(timeout) {
            Ok(v) if v.get("ok") == Some(&J::Bool(true)) => Ok(v),
            Ok(v) => Err(DeskdError::new(v.get("code").and_then(J::as_str).unwrap_or("deskd_error"), v.get("error").and_then(J::as_str).unwrap_or("K2 Screen refused"))),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                link.pending.lock().unwrap().remove(&id);
                Err(DeskdError::new("deskd_timeout", format!("K2 Screen didn't answer within {} s", timeout.as_secs())))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(DeskdError::new("deskd_gone", "K2 Screen stopped")),
        }
    }

    fn send_nr(&self, op: Op) -> Result<(), DeskdError> {
        let link = self.link()?;
        link.send(&Request { id: None, nr: true, op }).map_err(|e| DeskdError::new("deskd_gone", format!("cannot reach K2 Screen: {e}")))
    }

    /// Fire-and-forget input; a failure comes back as
    /// [`DeskdEvent::InputError`]. Input during a local hold is dropped by
    /// deskd (not queued); [`DeskdEvent::LocalInput`] says when.
    pub fn inject(&self, ev: InputEvent) -> Result<(), DeskdError> {
        self.send_nr(Op::Inject { ev })
    }

    /// Lets go of every held key and button (vs-live R3: the daemon's
    /// second line when a controller's socket closes, on hand-back, lease
    /// loss, view-only and display switch). Returns how many were released.
    pub fn release_all(&self, reason: ReleaseReason) -> Result<u64, DeskdError> {
        self.call(Op::ReleaseAll { reason }, CALL_TIMEOUT).map(|v| v.get("released").and_then(J::as_u64).unwrap_or(0))
    }

    /// Shows ("Rosson is viewing this screen from MacBook over K2
    /// Connect") or hides (`None`) the in-use banner.
    pub fn banner(&self, text: Option<String>) -> Result<(), DeskdError> {
        self.call(Op::Banner { text }, CALL_TIMEOUT).map(|_| ())
    }

    /// The TCC card's button: fire the prompts and open the panes on the
    /// Mac. Returns the grants right after (usually still missing).
    pub fn request_permission(&self) -> Result<Tcc, DeskdError> {
        let v = self.call(Op::TccRequest, CALL_TIMEOUT)?;
        serde_json::from_value(v["tcc"].clone()).map_err(|e| DeskdError::new("deskd_bad_reply", e.to_string()))
    }

    pub fn displays(&self) -> Result<Vec<Display>, DeskdError> {
        let v = self.call(Op::Displays, CALL_TIMEOUT)?;
        serde_json::from_value(v["displays"].clone()).map_err(|e| DeskdError::new("deskd_bad_reply", e.to_string()))
    }

    /// `{hid_idle_ms, local_idle_ms, hold}`.
    pub fn idle(&self) -> Result<J, DeskdError> {
        self.call(Op::Idle, CALL_TIMEOUT).map(|mut v| {
            if let Some(o) = v.as_object_mut() {
                o.remove("id");
                o.remove("ok");
            }
            v
        })
    }

    /// Protocol 4's requests on the control link, fire-and-forget: what
    /// the stream engine's `LinkSource` sends through its `SendRequest`
    /// (`layers`, `keyframe`, `rate`). A failure deskd sees comes back as
    /// [`DeskdEvent::LayerFailed`] / [`DeskdEvent::InputError`].
    pub fn send_link(&self, r: &k2_deskd_proto::link::Request) -> io::Result<()> {
        use k2_deskd_proto::link::Request as L;
        let op = match r.clone() {
            L::Layers { layers } => Op::Layers { layers },
            L::Keyframe { layer, since_ns } => Op::Keyframe { layer, since_ns },
            L::Rate { layer, fps, bitrate } => Op::Rate { layer, fps, bitrate },
        };
        self.send_nr(op).map_err(|e| io::Error::other(e.to_string()))
    }

    /// Moves deskd's one capture to `display` (v1 rule: every viewer of
    /// the desktop follows). Each layer's next unit is a key with `source
    /// start` from the new display. Waits for the capture to reopen.
    pub fn select_display(&self, display: u32) -> Result<u32, DeskdError> {
        let v = self.call(Op::Display { display }, Duration::from_secs(12))?;
        v.get("display").and_then(J::as_u64).map(|d| d as u32).ok_or_else(|| DeskdError::new("deskd_bad_reply", "no display in the reply"))
    }

    /// The frames pipe of the current link (once per link). End of stream
    /// = the link ended.
    pub fn take_frames(&self) -> Option<File> {
        self.inner.lock().unwrap().frames.take()
    }

    /// `{enabled, state, message, backend, tcc, version}` for `status`.
    pub fn status_json(&self) -> J {
        let g = self.inner.lock().unwrap();
        let hello = g.link.as_ref().map(|l| &l.hello);
        json!({
            "enabled": g.enabled,
            "state": g.state.name(),
            "message": g.state.message(),
            "backend": hello.map(|h| h.backend.clone()),
            "tcc": hello.map(|h| h.tcc),
            "deskd": hello.map(|h| json!({"version": h.version, "build": h.build, "pid": h.pid})),
        })
    }
}

/// The manager for this daemon (macOS LaunchAgent or Linux child).
pub fn manager() -> &'static Arc<DeskdManager> {
    static M: OnceLock<Arc<DeskdManager>> = OnceLock::new();
    M.get_or_init(|| DeskdManager::new(default_transport()))
}

/// A virtual desktop's own K2 Screen (LD-22): a child over pipes, given
/// that desktop's `DISPLAY` and cookie while it is awake (no session
/// otherwise, so it waits).
#[cfg(target_os = "linux")]
pub fn manager_for_virtual(id: &str) -> Arc<DeskdManager> {
    let transport: Arc<dyn Transport> = match linux_binary() {
        Ok(bin) => {
            let id = id.to_string();
            Arc::new(ChildTransport {
                bin,
                env: Box::new(move || {
                    super::virtual_desktop::manager().screen_env(&id).map(|vars| SessionEnv { vars: vars.into_iter().collect() })
                }),
            })
        }
        Err(e) => Arc::new(Unavailable(OpenError::Failed(e))),
    };
    DeskdManager::new(transport)
}

fn home() -> PathBuf {
    k2_core::paths::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(unix)]
fn uid() -> u32 {
    // SAFETY: plain call.
    unsafe { libc::getuid() }
}

fn default_transport() -> Arc<dyn Transport> {
    #[cfg(target_os = "macos")]
    {
        let exe = debug_env(ENV_BIN).map(PathBuf::from).or_else(|| std::env::current_exe().ok().and_then(|d| super::launch_agent::screen_exe_for(&d)));
        match exe.filter(|p| p.is_file()) {
            Some(program) => Arc::new(MacTransport {
                agent: super::launch_agent::Agent { home: home(), uid: uid(), program, runner: Arc::new(SystemRunner) },
                socket: home().join(".k2").join("run").join(k2_deskd_proto::SOCKET_NAME),
            }),
            None => Arc::new(Unavailable(OpenError::NeedsApp)),
        }
    }
    #[cfg(target_os = "linux")]
    {
        match linux_binary() {
            Ok(bin) => {
                let (u, h) = (uid(), home());
                Arc::new(ChildTransport { bin, env: Box::new(move || discover_session_env(&SystemRunner, u, &h, &|p| p.exists())) })
            }
            Err(e) => Arc::new(Unavailable(OpenError::Failed(e))),
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Arc::new(Unavailable(OpenError::Failed("Remote desktop runs on macOS and Linux in v1".into())))
    }
}

/// A transport that can't start (no K2 Screen.app, no binary, Windows).
pub struct Unavailable(pub OpenError);

impl Transport for Unavailable {
    fn start(&self) -> Result<(), OpenError> {
        Err(self.0.clone())
    }
    fn open(&self) -> Result<Opened, OpenError> {
        Err(self.0.clone())
    }
    fn restart(&self) -> Result<(), OpenError> {
        Ok(())
    }
    fn stop(&self) -> Result<(), OpenError> {
        Ok(())
    }
}

// ── macOS: LaunchAgent + socket ──────────────────────────────────────

#[cfg(unix)]
pub struct MacTransport {
    pub agent: super::launch_agent::Agent,
    pub socket: PathBuf,
}

/// Connects to deskd's socket and reads `hello` and the frames fd.
#[cfg(unix)]
pub fn open_socket(socket: &Path) -> Result<Opened, OpenError> {
    use std::os::unix::net::UnixStream;
    let s = match UnixStream::connect(socket) {
        Ok(s) => s,
        Err(e) if matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused) => return Err(OpenError::NotYet(e.to_string())),
        Err(e) => return Err(OpenError::Failed(format!("{}: {e}", socket.display()))),
    };
    s.set_read_timeout(Some(HELLO_TIMEOUT)).map_err(|e| OpenError::Failed(e.to_string()))?;
    let mut r = &s;
    let hello = match wire::read_message(&mut r) {
        Ok(Some(Message::Json(v))) if v.get("event").and_then(J::as_str) == Some("hello") => v,
        Ok(None) => {
            return Err(OpenError::Refused(
                "K2 Screen closed the link before hello: this daemon's code signature doesn't satisfy its check (a daemon not signed by K2?)".into(),
            ))
        }
        Ok(Some(_)) => return Err(OpenError::Failed("K2 Screen's first message wasn't hello".into())),
        Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => return Err(OpenError::NotYet("no hello yet".into())),
        Err(e) => return Err(OpenError::Failed(format!("reading hello: {e}"))),
    };
    let (marker, fd) = k2_deskd_proto::fdpass::recv_fd(&s).map_err(|e| OpenError::Failed(format!("frames fd: {e}")))?;
    if marker != k2_deskd_proto::fdpass::FRAMES_FD_MARKER {
        return Err(OpenError::Failed(format!("frames fd came with marker {marker:#x}")));
    }
    s.set_read_timeout(None).map_err(|e| OpenError::Failed(e.to_string()))?;
    let w = s.try_clone().map_err(|e| OpenError::Failed(e.to_string()))?;
    let c = s.try_clone().map_err(|e| OpenError::Failed(e.to_string()))?;
    Ok(Opened {
        hello,
        reader: Box::new(s),
        writer: Box::new(w),
        frames: Some(File::from(fd)),
        guard: None,
        close: Some(Box::new(move || {
            let _ = c.shutdown(std::net::Shutdown::Both);
        })),
    })
}

#[cfg(unix)]
fn agent_err(e: super::launch_agent::AgentError) -> OpenError {
    use super::launch_agent::AgentError;
    match e {
        AgentError::NoSession(_) => OpenError::NoSession,
        AgentError::BlockedInLoginItems => OpenError::Blocked,
        AgentError::Failed(m) => OpenError::Failed(m),
    }
}

#[cfg(unix)]
impl Transport for MacTransport {
    fn start(&self) -> Result<(), OpenError> {
        self.agent.install().map_err(agent_err)
    }
    fn open(&self) -> Result<Opened, OpenError> {
        open_socket(&self.socket)
    }
    fn restart(&self) -> Result<(), OpenError> {
        self.agent.kickstart().map_err(agent_err)
    }
    fn stop(&self) -> Result<(), OpenError> {
        self.agent.bootout().map_err(agent_err)
    }
}

// ── Linux: a child over pipes ────────────────────────────────────────

/// The session variables deskd gets (and nothing else but PATH and HOME).
pub const SESSION_KEYS: [&str; 6] = ["DISPLAY", "XAUTHORITY", "WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS", "XDG_RUNTIME_DIR", "XDG_SESSION_TYPE"];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionEnv {
    pub vars: BTreeMap<String, String>,
}

impl SessionEnv {
    pub fn get(&self, k: &str) -> Option<&str> {
        self.vars.get(k).map(String::as_str).filter(|v| !v.is_empty())
    }
    pub fn has_display(&self) -> bool {
        self.get("DISPLAY").is_some() || self.get("WAYLAND_DISPLAY").is_some()
    }
}

fn unquote(v: &str) -> String {
    let v = v.trim();
    if let Some(inner) = v.strip_prefix("$'").and_then(|s| s.strip_suffix('\'')) {
        return inner.replace("\\'", "'").replace("\\\\", "\\");
    }
    if v.len() >= 2 && ((v.starts_with('\'') && v.ends_with('\'')) || (v.starts_with('"') && v.ends_with('"'))) {
        return v[1..v.len() - 1].to_string();
    }
    v.to_string()
}

/// `KEY=VALUE` lines (`systemctl --user show-environment`), session keys only.
pub fn parse_env_lines(s: &str) -> SessionEnv {
    let mut vars = BTreeMap::new();
    for line in s.lines() {
        if let Some((k, v)) = line.split_once('=') {
            if SESSION_KEYS.contains(&k.trim()) {
                vars.insert(k.trim().to_string(), unquote(v));
            }
        }
    }
    SessionEnv { vars }
}

/// `loginctl list-sessions --no-legend`: `(session id, uid)` per row (the
/// first two columns in every systemd version).
pub fn parse_session_list(s: &str) -> Vec<(String, u32)> {
    s.lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let id = f.next()?.to_string();
            let uid = f.next()?.parse().ok()?;
            Some((id, uid))
        })
        .collect()
}

/// `loginctl show-session <id> -p …` output.
pub fn parse_props(s: &str) -> HashMap<String, String> {
    s.lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect()
}

/// A local, active, graphical user session (not SSH, not the greeter).
pub fn usable_session(p: &HashMap<String, String>) -> bool {
    let is = |k: &str, v: &str| p.get(k).map(String::as_str) == Some(v);
    let ty = p.get("Type").map(String::as_str).unwrap_or("");
    is("Active", "yes") && !is("Remote", "yes") && p.get("Class").map(String::as_str).unwrap_or("user") == "user" && (ty == "x11" || ty == "wayland")
}

fn run_ok(r: &dyn Runner, prog: &str, args: &[&str]) -> Option<Output> {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    r.run(prog, &args).ok().filter(|o| o.status == 0)
}

/// Finds the console session's environment (§3.2 Linux): the systemd user
/// manager's environment first (GNOME, KDE and most session scripts import
/// DISPLAY / WAYLAND_DISPLAY there), then `loginctl`'s active graphical
/// session with the usual runtime paths. `exists` is `Path::exists` (a
/// parameter for tests).
pub fn discover_session_env(r: &dyn Runner, uid: u32, home: &Path, exists: &dyn Fn(&Path) -> bool) -> Option<SessionEnv> {
    let runtime = PathBuf::from(format!("/run/user/{uid}"));
    let fill = |mut env: SessionEnv| {
        let bus = runtime.join("bus");
        if env.get("DBUS_SESSION_BUS_ADDRESS").is_none() && exists(&bus) {
            env.vars.insert("DBUS_SESSION_BUS_ADDRESS".into(), format!("unix:path={}", bus.display()));
        }
        if env.get("XDG_RUNTIME_DIR").is_none() && exists(&runtime) {
            env.vars.insert("XDG_RUNTIME_DIR".into(), runtime.display().to_string());
        }
        env
    };
    if let Some(o) = run_ok(r, "systemctl", &["--user", "show-environment"]) {
        let env = parse_env_lines(&o.stdout);
        if env.has_display() {
            return Some(fill(env));
        }
    }
    let list = run_ok(r, "loginctl", &["list-sessions", "--no-legend"])?;
    for (id, u) in parse_session_list(&list.stdout) {
        if u != uid {
            continue;
        }
        let Some(o) = run_ok(r, "loginctl", &["show-session", &id, "-p", "Type", "-p", "Display", "-p", "Active", "-p", "Remote", "-p", "Class"]) else { continue };
        let p = parse_props(&o.stdout);
        if !usable_session(&p) {
            continue;
        }
        let mut env = SessionEnv::default();
        let ty = p.get("Type").cloned().unwrap_or_default();
        env.vars.insert("XDG_SESSION_TYPE".into(), ty.clone());
        if let Some(d) = p.get("Display").filter(|d| !d.is_empty()) {
            env.vars.insert("DISPLAY".into(), d.clone());
        }
        if ty == "wayland" && exists(&runtime.join("wayland-0")) {
            env.vars.insert("WAYLAND_DISPLAY".into(), "wayland-0".into());
        }
        let xauth = home.join(".Xauthority");
        if env.get("DISPLAY").is_some() && exists(&xauth) {
            env.vars.insert("XAUTHORITY".into(), xauth.display().to_string());
        }
        if env.has_display() {
            return Some(fill(env));
        }
    }
    None
}

/// The k2-deskd binary to run on Linux: `K2_DESKD_BIN` (debug builds), the
/// embedded copy (release), or the one beside the daemon (dev).
pub fn linux_binary() -> Result<PathBuf, String> {
    if let Some(p) = debug_env(ENV_BIN) {
        let p = PathBuf::from(p);
        return if p.is_file() { Ok(p) } else { Err(format!("{ENV_BIN} names no file")) };
    }
    if let Some((bytes, sha)) = DESKD_EMBED {
        if sha256_hex(bytes) != sha {
            return Err("the embedded k2-deskd fails its hash".into());
        }
        static PLACED: OnceLock<Result<PathBuf, String>> = OnceLock::new();
        return PLACED.get_or_init(|| placed_copy(&home().join(".k2"), bytes, sha).map_err(|e| format!("placing k2-deskd: {e}"))).clone();
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let sib = exe.parent().map(|d| d.join(k2_deskd_proto::BINARY_NAME)).ok_or("no exe folder")?;
    if sib.is_file() {
        Ok(sib)
    } else {
        Err("no k2-deskd beside the daemon".into())
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// `<k2 home>/bin/deskd/<ver>-<sha12>/k2-deskd` (0700), reused on a hash
/// match, never overwritten in place.
pub fn placed_copy(k2_home: &Path, bytes: &[u8], sha: &str) -> io::Result<PathBuf> {
    let dir = k2_home.join("bin").join("deskd").join(format!("{}-{}", k2_deskd_proto::VERSION, &sha[..12.min(sha.len())]));
    let target = dir.join(k2_deskd_proto::BINARY_NAME);
    if let Ok(existing) = std::fs::read(&target) {
        if sha256_hex(&existing) == sha {
            return Ok(target);
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
        let tmp = dir.join(format!(".k2-deskd.{}.tmp", std::process::id()));
        {
            let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o700).open(&tmp)?;
            f.write_all(bytes)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, &target)?;
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(&dir)?;
        std::fs::write(&target, bytes)?;
    }
    Ok(target)
}

/// Kills and reaps the child when the link goes.
#[cfg(unix)]
struct ChildGuard(std::process::Child);

#[cfg(unix)]
impl Drop for ChildGuard {
    fn drop(&mut self) {
        // SIGTERM first: deskd releases held input on it.
        // SAFETY: signalling our own child.
        unsafe { libc::kill(self.0.id() as i32, libc::SIGTERM) };
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.0.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(unix)]
pub struct ChildTransport {
    pub bin: PathBuf,
    pub env: Box<dyn Fn() -> Option<SessionEnv> + Send + Sync>,
}

#[cfg(unix)]
fn frames_to_fd3(fd: std::os::fd::RawFd) -> io::Result<()> {
    // SAFETY: fcntl / dup2 in the child, on fds it owns.
    unsafe {
        if fd == 3 {
            let flags = libc::fcntl(3, libc::F_GETFD);
            if flags < 0 || libc::fcntl(3, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                return Err(io::Error::last_os_error());
            }
        } else if libc::dup2(fd, 3) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(unix)]
impl Transport for ChildTransport {
    fn start(&self) -> Result<(), OpenError> {
        if self.bin.is_file() {
            Ok(())
        } else {
            Err(OpenError::Failed(format!("no k2-deskd at {}", self.bin.display())))
        }
    }

    fn open(&self) -> Result<Opened, OpenError> {
        use std::os::fd::AsRawFd;
        use std::os::unix::process::CommandExt;
        use std::process::Stdio;
        let env = (self.env)().ok_or(OpenError::NoSession)?;
        if !env.has_display() {
            return Err(OpenError::NoDisplay);
        }
        let mut cmd = k2_core::bg_proc::command(&self.bin);
        // Only the session's variables: none of the daemon's own (no K2_*
        // token, no secrets) reach the helper.
        cmd.env_clear().env("PATH", "/usr/local/bin:/usr/bin:/bin").env("HOME", home());
        for (k, v) in &env.vars {
            cmd.env(k, v);
        }
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit());
        let (frames_rx, frames_tx) = std::io::pipe().map_err(|e| OpenError::Failed(format!("frames pipe: {e}")))?;
        let fd = frames_tx.as_raw_fd();
        // SAFETY: the hook only calls fcntl / dup2.
        unsafe {
            cmd.pre_exec(move || frames_to_fd3(fd));
        }
        let spawned = cmd.spawn();
        drop(frames_tx);
        let mut child = spawned.map_err(|e| OpenError::Failed(format!("cannot start {}: {e}", self.bin.display())))?;
        let stdin = child.stdin.take().expect("piped stdin");
        let mut stdout = child.stdout.take().expect("piped stdout");
        let guard = ChildGuard(child);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let r = wire::read_message(&mut stdout);
            let _ = tx.send((r, stdout));
        });
        let (r, stdout) = match rx.recv_timeout(HELLO_TIMEOUT) {
            Ok(x) => x,
            Err(_) => return Err(OpenError::Failed(format!("k2-deskd sent no hello within {} s", HELLO_TIMEOUT.as_secs()))),
        };
        let hello = match r {
            Ok(Some(Message::Json(v))) if v.get("event").and_then(J::as_str) == Some("hello") => v,
            Ok(None) => return Err(OpenError::Failed("k2-deskd exited before hello".into())),
            Ok(Some(_)) => return Err(OpenError::Failed("k2-deskd's first message wasn't hello".into())),
            Err(e) => return Err(OpenError::Failed(format!("reading hello: {e}"))),
        };
        Ok(Opened { hello, reader: Box::new(stdout), writer: Box::new(stdin), frames: Some(File::from(std::os::fd::OwnedFd::from(frames_rx))), guard: Some(Box::new(guard)), close: None })
    }

    fn restart(&self) -> Result<(), OpenError> {
        Ok(()) // the old child died with its link; the next open spawns afresh
    }

    fn stop(&self) -> Result<(), OpenError> {
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use k2_deskd_proto::msg::{Button, FramesVia};
    use k2_deskd_proto::wire::{Codec, UnitHeader};
    use std::collections::VecDeque;
    use std::os::unix::net::UnixStream;
    use std::sync::atomic::AtomicU32;

    const SETTLE: Duration = Duration::from_secs(5);

    fn hello(version: u32, tcc: Tcc, backend: &str) -> J {
        serde_json::to_value(Event::Hello(Hello {
            version,
            build: "0.1.0".into(),
            role: "desktop".into(),
            os: "macos".into(),
            backend: backend.into(),
            tcc,
            pid: 4242,
            frames: FramesVia::Fd,
            accepts: vec![version],
            codecs: vec![Codec::H264],
            encoder: k2_deskd_proto::link::Encoder::Hardware,
            clock_ns: 1_000,
        }))
        .unwrap()
    }

    fn granted() -> Tcc {
        Tcc { required: true, screen: true, accessibility: true }
    }

    /// A transport whose deskd is a thread on a socket pair: answers ping,
    /// release_all (2), displays, tcc_request, banner and idle.
    #[derive(Default)]
    struct Fake {
        hellos: Mutex<VecDeque<J>>,
        start_err: Mutex<Option<OpenError>>,
        opens: AtomicU32,
        restarts: AtomicU32,
        stops: AtomicU32,
        /// deskd's end of each link (to push events, or close it).
        peers: Mutex<Vec<UnixStream>>,
        frame_tx: Mutex<Vec<std::io::PipeWriter>>,
        seen: Arc<Mutex<Vec<J>>>,
    }

    impl Fake {
        fn with(hellos: Vec<J>) -> Arc<Fake> {
            let f = Fake::default();
            *f.hellos.lock().unwrap() = hellos.into();
            Arc::new(f)
        }
        fn push_event(&self, v: J) {
            let mut peers = self.peers.lock().unwrap();
            let s = peers.last_mut().expect("a link");
            wire::write_json(s, &v).unwrap();
        }
        fn close_link(&self) {
            for s in self.peers.lock().unwrap().drain(..) {
                let _ = s.shutdown(std::net::Shutdown::Both);
            }
        }
    }

    impl Transport for Fake {
        fn start(&self) -> Result<(), OpenError> {
            match self.start_err.lock().unwrap().clone() {
                Some(e) => Err(e),
                None => Ok(()),
            }
        }
        fn open(&self) -> Result<Opened, OpenError> {
            self.opens.fetch_add(1, Ordering::SeqCst);
            let h = self.hellos.lock().unwrap().pop_front().unwrap_or_else(|| hello(PROTOCOL_VERSION, granted(), "macos"));
            let (daemon, mut deskd) = UnixStream::pair().unwrap();
            self.peers.lock().unwrap().push(deskd.try_clone().unwrap());
            let seen = Arc::clone(&self.seen);
            std::thread::spawn(move || {
                while let Ok(Some(Message::Json(v))) = wire::read_message(&mut deskd) {
                    seen.lock().unwrap().push(v.clone());
                    let Some(id) = v.get("id").and_then(J::as_u64) else { continue };
                    let reply = match v["op"].as_str().unwrap_or("") {
                        "release_all" => json!({"id": id, "ok": true, "released": 2}),
                        "displays" => json!({"id": id, "ok": true, "displays": [{"id": 1, "name": "Built-in display", "x": 0.0, "y": 0.0, "width_pt": 1512.0, "height_pt": 982.0, "width_px": 3024, "height_px": 1964, "scale": 2.0, "main": true}]}),
                        "tcc_request" => json!({"id": id, "ok": true, "tcc": {"required": true, "screen": false, "accessibility": true}}),
                        "idle" => json!({"id": id, "ok": true, "hid_idle_ms": 10, "local_idle_ms": null, "hold": false}),
                        "nope" => json!({"id": id, "ok": false, "code": "bad_request", "error": "no"}),
                        _ => json!({"id": id, "ok": true}),
                    };
                    if wire::write_json(&mut deskd, &reply).is_err() {
                        break;
                    }
                }
            });
            let (rx, tx) = std::io::pipe().unwrap();
            self.frame_tx.lock().unwrap().push(tx);
            let c = daemon.try_clone().unwrap();
            Ok(Opened {
                hello: h,
                reader: Box::new(daemon.try_clone().unwrap()),
                writer: Box::new(daemon),
                frames: Some(File::from(std::os::fd::OwnedFd::from(rx))),
                guard: None,
                close: Some(Box::new(move || {
                    let _ = c.shutdown(std::net::Shutdown::Both);
                })),
            })
        }
        fn restart(&self) -> Result<(), OpenError> {
            self.restarts.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn stop(&self) -> Result<(), OpenError> {
            self.stops.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn wait_for(m: &DeskdManager, pred: impl Fn(&DeskdState) -> bool) -> DeskdState {
        let deadline = Instant::now() + SETTLE;
        loop {
            let s = m.state();
            if pred(&s) || Instant::now() > deadline {
                return s;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn enable_reaches_ready_and_calls_round_trip() {
        let fake = Fake::with(vec![]);
        let m = DeskdManager::new(fake.clone());
        assert_eq!(m.state(), DeskdState::Off);
        assert_eq!(m.enable(SETTLE), DeskdState::Ready { backend: "macos".into() });
        assert_eq!(m.release_all(ReleaseReason::HandBack).unwrap(), 2);
        assert_eq!(m.displays().unwrap()[0].scale, 2.0);
        assert_eq!(m.idle().unwrap(), json!({"hid_idle_ms": 10, "local_idle_ms": null, "hold": false}));
        m.call(Op::Ping, CALL_TIMEOUT).unwrap();
        assert_eq!(m.status_json()["state"], "ready");
        assert_eq!(m.status_json()["deskd"]["pid"], 4242);
        m.disable().unwrap();
    }

    #[test]
    fn inject_is_fire_and_forget_with_the_viewer_shape() {
        let fake = Fake::with(vec![]);
        let m = DeskdManager::new(fake.clone());
        m.enable(SETTLE);
        m.inject(InputEvent::Button { button: Button::Left, down: true }).unwrap();
        m.call(Op::Ping, CALL_TIMEOUT).unwrap(); // ordered after the inject
        let seen = fake.seen.lock().unwrap().clone();
        assert_eq!(seen[0], json!({"nr": true, "op": "inject", "ev": {"t": "button", "button": "left", "down": true}}));
        m.disable().unwrap();
    }

    #[test]
    fn missing_tcc_is_needs_permission_until_deskd_says_otherwise() {
        let fake = Fake::with(vec![hello(PROTOCOL_VERSION, Tcc { required: true, screen: false, accessibility: false }, "macos")]);
        let m = DeskdManager::new(fake.clone());
        assert_eq!(m.enable(SETTLE), DeskdState::NeedsPermission { screen: false, accessibility: false });
        assert_eq!(m.request_permission().unwrap(), Tcc { required: true, screen: false, accessibility: true });
        fake.push_event(json!({"event": "tcc_changed", "tcc": {"required": true, "screen": true, "accessibility": true}}));
        assert_eq!(wait_for(&m, |s| s.name() == "ready"), DeskdState::Ready { backend: "macos".into() });
        m.disable().unwrap();
    }

    #[test]
    fn one_mismatch_restarts_deskd_and_then_it_is_ready() {
        let fake = Fake::with(vec![hello(3, granted(), "macos")]);
        let m = DeskdManager::new(fake.clone());
        m.enable(Duration::from_millis(1));
        assert_eq!(wait_for(&m, |s| s.name() == "ready").name(), "ready");
        assert_eq!(fake.restarts.load(Ordering::SeqCst), 1);
        m.disable().unwrap();
    }

    #[test]
    fn a_second_mismatch_is_needs_update() {
        let fake = Fake::with(vec![hello(3, granted(), "macos"), hello(3, granted(), "macos")]);
        let m = DeskdManager::new(fake.clone());
        m.enable(Duration::from_millis(1));
        assert_eq!(wait_for(&m, |s| s.name() == "needs_update"), DeskdState::NeedsUpdate { found: Some(3) });
        assert_eq!(fake.restarts.load(Ordering::SeqCst), 1);
        m.disable().unwrap();
    }

    #[test]
    fn a_wrong_role_counts_as_a_mismatch() {
        let mut h = hello(PROTOCOL_VERSION, granted(), "macos");
        h["role"] = json!("sim");
        let fake = Fake::with(vec![h.clone(), h]);
        let m = DeskdManager::new(fake.clone());
        m.enable(Duration::from_millis(1));
        assert_eq!(wait_for(&m, |s| s.name() == "needs_update").name(), "needs_update");
        m.disable().unwrap();
    }

    #[test]
    fn a_dropped_link_reconnects() {
        let fake = Fake::with(vec![]);
        let m = DeskdManager::new(fake.clone());
        m.enable(SETTLE);
        let events = m.subscribe();
        fake.close_link();
        let deadline = Instant::now() + SETTLE;
        let mut saw_starting = false;
        let mut back = false;
        while Instant::now() < deadline {
            match events.recv_timeout(Duration::from_millis(100)) {
                Ok(DeskdEvent::State(DeskdState::Starting)) => saw_starting = true,
                Ok(DeskdEvent::State(DeskdState::Ready { .. })) if saw_starting => {
                    back = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(saw_starting && back, "starting={saw_starting} back={back}");
        assert!(fake.opens.load(Ordering::SeqCst) >= 2);
        m.call(Op::Ping, CALL_TIMEOUT).unwrap();
        m.disable().unwrap();
    }

    #[test]
    fn disable_closes_the_link_and_stops_deskd() {
        let fake = Fake::with(vec![]);
        let m = DeskdManager::new(fake.clone());
        m.enable(SETTLE);
        let mut peer = fake.peers.lock().unwrap()[0].try_clone().unwrap();
        m.disable().unwrap();
        // deskd's end sees the link close (so it releases and hides).
        peer.set_read_timeout(Some(SETTLE)).unwrap();
        assert!(wire::read_message(&mut peer).unwrap().is_none());
        assert_eq!(m.state(), DeskdState::Off);
        assert_eq!(fake.stops.load(Ordering::SeqCst), 1);
        assert_eq!(m.call(Op::Ping, CALL_TIMEOUT).unwrap_err().code, "desktop_off");
        assert!(m.take_frames().is_none());
    }

    #[test]
    fn deskd_events_reach_subscribers() {
        let fake = Fake::with(vec![]);
        let m = DeskdManager::new(fake.clone());
        m.enable(SETTLE);
        let rx = m.subscribe();
        fake.push_event(json!({"event": "local_input", "hold": true}));
        fake.push_event(json!({"event": "banner_disconnect"}));
        fake.push_event(json!({"event": "input_error", "op": "inject", "code": "no_accessibility", "error": "x"}));
        let got: Vec<DeskdEvent> = (0..3).map(|_| rx.recv_timeout(SETTLE).unwrap()).collect();
        assert_eq!(got[0], DeskdEvent::LocalInput { hold: true });
        assert_eq!(got[1], DeskdEvent::BannerDisconnect);
        assert_eq!(got[2], DeskdEvent::InputError { op: "inject".into(), code: "no_accessibility".into(), error: "x".into() });
        fake.push_event(json!({"event": "layer_failed", "layer": 2, "code": "unsupported", "error": "no JPEG"}));
        fake.push_event(json!({"event": "capture_error", "display": 1, "code": "no_screen_permission", "error": "no"}));
        assert_eq!(rx.recv_timeout(SETTLE).unwrap(), DeskdEvent::LayerFailed { layer: 2, code: "unsupported".into(), error: "no JPEG".into() });
        assert_eq!(rx.recv_timeout(SETTLE).unwrap(), DeskdEvent::CaptureError { display: 1, code: "no_screen_permission".into(), error: "no".into() });
        m.disable().unwrap();
    }

    /// What `LinkSource` sends through `send_link` reaches deskd as the
    /// engine wrote it, plus `nr` (no reply on success).
    #[test]
    fn send_link_writes_protocol_4s_requests_fire_and_forget() {
        use k2_deskd_proto::link::{LayerSpec, Request as L};
        let fake = Fake::with(vec![]);
        let m = DeskdManager::new(fake.clone());
        m.enable(SETTLE);
        let layer = LayerSpec { id: 1, codec: Codec::H264, max_width: 1920, fps: 30, bitrate: 4_000_000, quality: None, on: true };
        m.send_link(&L::Layers { layers: vec![layer] }).unwrap();
        m.send_link(&L::Keyframe { layer: 1, since_ns: Some(9) }).unwrap();
        m.send_link(&L::Rate { layer: 1, fps: 30, bitrate: 2_000_000 }).unwrap();
        assert_eq!(m.select_display(1), Err(DeskdError::new("deskd_bad_reply", "no display in the reply")), "the fake answers {{}}");
        let seen = fake.seen.lock().unwrap().clone();
        for (i, r) in [L::Layers { layers: vec![layer] }, L::Keyframe { layer: 1, since_ns: Some(9) }, L::Rate { layer: 1, fps: 30, bitrate: 2_000_000 }].iter().enumerate() {
            let mut want = serde_json::to_value(r).unwrap();
            want["nr"] = json!(true);
            assert_eq!(seen[i], want);
        }
        assert_eq!((seen[3]["op"].as_str(), seen[3]["display"].as_u64()), (Some("display"), Some(1)));
        // The hello's protocol-4 fields, for LinkClock and negotiation.
        let f = m.hello().unwrap().link_fields();
        assert_eq!((f.version, f.accepts, f.codecs, f.clock_ns), (PROTOCOL_VERSION, vec![PROTOCOL_VERSION], vec![Codec::H264], 1_000));
        m.disable().unwrap();
    }

    #[test]
    fn an_ok_false_reply_is_an_error_with_deskds_code() {
        let fake = Fake::with(vec![]);
        let m = DeskdManager::new(fake.clone());
        m.enable(SETTLE);
        let link = m.link().unwrap();
        let id = link.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel();
        link.pending.lock().unwrap().insert(id, tx);
        wire::write_json(&mut *link.writer.lock().unwrap(), &json!({"id": id, "op": "nope"})).unwrap();
        assert_eq!(rx.recv_timeout(SETTLE).unwrap()["code"], "bad_request");
        m.disable().unwrap();
    }

    #[test]
    fn frames_come_through_the_link_pipe_once() {
        let fake = Fake::with(vec![]);
        let m = DeskdManager::new(fake.clone());
        m.enable(SETTLE);
        let mut f = m.take_frames().expect("frames pipe");
        assert!(m.take_frames().is_none());
        let h = UnitHeader { codec: Codec::H264, key: true, source_start: true, discardable: false, layer: 0, source: 1, width: 4, height: 4, seq: 1, captured_ns: 1, emitted_ns: 2 };
        wire::write_unit(&mut fake.frame_tx.lock().unwrap()[0], &h, b"au").unwrap();
        match wire::read_message(&mut f).unwrap().unwrap() {
            Message::Unit(fr) => assert_eq!(fr.header, h),
            Message::Json(_) => panic!("json on the frames pipe"),
        }
        m.disable().unwrap();
    }

    #[test]
    fn start_errors_become_states() {
        for (err, name) in [(OpenError::NoSession, "no_session"), (OpenError::Blocked, "blocked_in_login_items"), (OpenError::NeedsApp, "needs_app")] {
            let fake = Fake::with(vec![]);
            *fake.start_err.lock().unwrap() = Some(err);
            let m = DeskdManager::new(fake.clone());
            assert_eq!(m.enable(SETTLE).name(), name);
            m.disable().unwrap();
        }
        let m = DeskdManager::new(Arc::new(Unavailable(OpenError::NeedsApp)));
        assert_eq!(m.enable(SETTLE), DeskdState::NeedsApp);
        assert_eq!(m.inject(InputEvent::Text { text: "x".into() }).unwrap_err().code, "deskd_not_ready");
        m.disable().unwrap();
    }

    #[test]
    fn backend_none_is_no_display() {
        let fake = Fake::with(vec![hello(PROTOCOL_VERSION, Tcc::NOT_REQUIRED, "none")]);
        let m = DeskdManager::new(fake);
        assert_eq!(m.enable(SETTLE), DeskdState::NoDisplay);
        m.disable().unwrap();
    }

    // ── the macOS socket path, against a fake deskd on a real socket ──

    fn tmp(name: &str) -> PathBuf {
        // Short: macOS socket paths are capped at 104 bytes.
        let d = std::env::temp_dir().join(format!("kdl{}{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn open_socket_reads_hello_and_the_frames_fd() {
        use std::os::fd::AsRawFd;
        let dir = tmp("o");
        let path = dir.join("deskd.sock");
        let l = std::os::unix::net::UnixListener::bind(&path).unwrap();
        std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            wire::write_json(&mut s, &hello(PROTOCOL_VERSION, granted(), "macos")).unwrap();
            let (rx, mut tx) = std::io::pipe().unwrap();
            k2_deskd_proto::fdpass::send_fd(&s, rx.as_raw_fd(), k2_deskd_proto::fdpass::FRAMES_FD_MARKER).unwrap();
            drop(rx);
            let h = UnitHeader { codec: Codec::Vp8, key: true, source_start: true, discardable: false, layer: 0, source: 0, width: 2, height: 2, seq: 9, captured_ns: 0, emitted_ns: 0 };
            wire::write_unit(&mut tx, &h, b"vp8").unwrap();
            if let Ok(Some(Message::Json(v))) = wire::read_message(&mut s) {
                wire::write_json(&mut s, &json!({"id": v["id"], "ok": true})).unwrap();
            }
        });
        let mut o = open_socket(&path).unwrap();
        assert_eq!(o.hello["event"], "hello");
        let mut f = o.frames.take().unwrap();
        match wire::read_message(&mut f).unwrap().unwrap() {
            Message::Unit(fr) => assert_eq!((fr.header.seq, fr.data.as_slice()), (9, &b"vp8"[..])),
            Message::Json(_) => panic!("json on the frames pipe"),
        }
        wire::write_json(&mut o.writer, &json!({"id": 1, "op": "ping"})).unwrap();
        match wire::read_message(&mut o.reader).unwrap().unwrap() {
            Message::Json(v) => assert_eq!(v, json!({"id": 1, "ok": true})),
            Message::Unit(_) => panic!("frame on the control link"),
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn open_socket_with_no_deskd_is_not_yet_and_a_refusal_is_refused() {
        let dir = tmp("r");
        let path = dir.join("deskd.sock");
        assert!(matches!(open_socket(&path), Err(OpenError::NotYet(_))));
        let l = std::os::unix::net::UnixListener::bind(&path).unwrap();
        std::thread::spawn(move || {
            // deskd's peer check failed: closed with no reply.
            let (s, _) = l.accept().unwrap();
            drop(s);
        });
        assert!(matches!(open_socket(&path), Err(OpenError::Refused(_))));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn mac_transport_drives_launchctl_through_the_agent() {
        use super::super::launch_agent::{tests::FakeLaunchctl, Agent};
        let dir = tmp("m");
        let fake = Arc::new(FakeLaunchctl::default());
        let t = MacTransport { agent: Agent { home: dir.clone(), uid: 501, program: PathBuf::from("/x/k2-deskd"), runner: fake.clone() }, socket: dir.join("none.sock") };
        t.start().unwrap();
        assert!(t.agent.loaded());
        t.restart().unwrap();
        t.stop().unwrap();
        assert!(!t.agent.loaded());
        let cmds: Vec<String> = fake.calls.lock().unwrap().iter().map(|c| c[0].clone()).collect();
        for want in ["bootstrap", "kickstart", "bootout"] {
            assert!(cmds.iter().any(|c| c == want), "{want} missing from {cmds:?}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // ── the Linux child path, with a shell script as deskd ──

    #[test]
    fn child_transport_passes_only_session_env_and_fd3() {
        let dir = tmp("c");
        let hello_file = dir.join("hello.bin");
        let mut hb = Vec::new();
        let mut h = hello(PROTOCOL_VERSION, Tcc::NOT_REQUIRED, "x11");
        h["os"] = json!("linux");
        h["frames"] = json!("fd3");
        wire::write_json(&mut hb, &h).unwrap();
        std::fs::write(&hello_file, &hb).unwrap();
        let frame_file = dir.join("frame.bin");
        let mut fb = Vec::new();
        let fh = UnitHeader { codec: Codec::Vp8, key: true, source_start: true, discardable: false, layer: 0, source: 0, width: 8, height: 8, seq: 3, captured_ns: 0, emitted_ns: 0 };
        wire::write_unit(&mut fb, &fh, b"key").unwrap();
        std::fs::write(&frame_file, &fb).unwrap();
        let env_file = dir.join("env.txt");
        let script = dir.join("k2-deskd");
        std::fs::write(&script, format!("#!/bin/sh\nenv > '{}'\ncat '{}' >&3\ncat '{}'\ncat > /dev/null\n", env_file.display(), frame_file.display(), hello_file.display())).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut env = SessionEnv::default();
        env.vars.insert("DISPLAY".into(), ":99".into());
        env.vars.insert("XAUTHORITY".into(), "/tmp/xa".into());
        let t = ChildTransport { bin: script, env: Box::new(move || Some(env.clone())) };
        t.start().unwrap();
        let mut o = t.open().unwrap();
        assert_eq!(o.hello["frames"], "fd3");
        match wire::read_message(o.frames.as_mut().unwrap()).unwrap().unwrap() {
            Message::Unit(f) => assert_eq!(f.header, fh),
            Message::Json(_) => panic!("json on fd 3"),
        }
        let envs = std::fs::read_to_string(&env_file).unwrap();
        assert!(envs.lines().any(|l| l == "DISPLAY=:99"), "{envs}");
        assert!(envs.lines().any(|l| l == "XAUTHORITY=/tmp/xa"), "{envs}");
        assert!(!envs.lines().any(|l| l.starts_with("K2_")), "a K2_* variable reached deskd: {envs}");
        drop(o); // kills the child
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn child_transport_without_a_session_is_no_session() {
        let t = ChildTransport { bin: PathBuf::from("/bin/sh"), env: Box::new(|| None) };
        assert!(matches!(t.open(), Err(OpenError::NoSession)));
        let t = ChildTransport { bin: PathBuf::from("/bin/sh"), env: Box::new(|| Some(SessionEnv::default())) };
        assert!(matches!(t.open(), Err(OpenError::NoDisplay)));
    }

    // ── session env discovery ──

    struct Script(HashMap<String, Output>);
    impl Runner for Script {
        fn run(&self, program: &str, args: &[String]) -> io::Result<Output> {
            let key = format!("{program} {}", args.join(" "));
            self.0.get(&key).cloned().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, key))
        }
    }
    fn out(s: &str) -> Output {
        Output { status: 0, stdout: s.into(), stderr: String::new() }
    }
    const PROPS: &str = "-p Type -p Display -p Active -p Remote -p Class";

    #[test]
    fn systemd_user_environment_wins_and_gets_the_bus_filled_in() {
        let mut m = HashMap::new();
        m.insert("systemctl --user show-environment".to_string(), out("HOME=/home/r\nDISPLAY=:0\nXAUTHORITY=$'/run/user/1000/gdm/Xauthority'\nK2_TOKEN=secret\n"));
        let env = discover_session_env(&Script(m), 1000, Path::new("/home/r"), &|p| p == Path::new("/run/user/1000/bus") || p == Path::new("/run/user/1000")).unwrap();
        assert_eq!(env.get("DISPLAY"), Some(":0"));
        assert_eq!(env.get("XAUTHORITY"), Some("/run/user/1000/gdm/Xauthority"));
        assert_eq!(env.get("DBUS_SESSION_BUS_ADDRESS"), Some("unix:path=/run/user/1000/bus"));
        assert_eq!(env.get("XDG_RUNTIME_DIR"), Some("/run/user/1000"));
        assert!(!env.vars.contains_key("K2_TOKEN") && !env.vars.contains_key("HOME"));
    }

    #[test]
    fn loginctl_finds_the_active_local_graphical_session() {
        let mut m = HashMap::new();
        m.insert("systemctl --user show-environment".to_string(), out("HOME=/home/r\n"));
        m.insert("loginctl list-sessions --no-legend".to_string(), out("  3 1001 other seat0 tty2\n  5 1000 r - pts/1\n  2 1000 r seat0 tty2\n"));
        m.insert(format!("loginctl show-session 5 {PROPS}"), out("Type=tty\nDisplay=\nActive=yes\nRemote=yes\nClass=user\n"));
        m.insert(format!("loginctl show-session 2 {PROPS}"), out("Type=x11\nDisplay=:1\nActive=yes\nRemote=no\nClass=user\n"));
        let env = discover_session_env(&Script(m), 1000, Path::new("/home/r"), &|p| p == Path::new("/home/r/.Xauthority")).unwrap();
        assert_eq!(env.get("DISPLAY"), Some(":1"));
        assert_eq!(env.get("XDG_SESSION_TYPE"), Some("x11"));
        assert_eq!(env.get("XAUTHORITY"), Some("/home/r/.Xauthority"));
    }

    #[test]
    fn a_wayland_session_gets_wayland_display() {
        let mut m = HashMap::new();
        m.insert("loginctl list-sessions --no-legend".to_string(), out("2 1000 r seat0 tty2\n"));
        m.insert(format!("loginctl show-session 2 {PROPS}"), out("Type=wayland\nDisplay=\nActive=yes\nRemote=no\nClass=user\n"));
        let env = discover_session_env(&Script(m), 1000, Path::new("/home/r"), &|p| p == Path::new("/run/user/1000/wayland-0")).unwrap();
        assert_eq!(env.get("WAYLAND_DISPLAY"), Some("wayland-0"));
        assert_eq!(env.get("DISPLAY"), None);
    }

    #[test]
    fn no_graphical_session_is_none() {
        let mut m = HashMap::new();
        m.insert("loginctl list-sessions --no-legend".to_string(), out("7 1000 r - pts/0\n"));
        m.insert(format!("loginctl show-session 7 {PROPS}"), out("Type=tty\nActive=yes\nRemote=yes\nClass=user\n"));
        assert_eq!(discover_session_env(&Script(m), 1000, Path::new("/home/r"), &|_| false), None);
        assert_eq!(discover_session_env(&Script(HashMap::new()), 1000, Path::new("/home/r"), &|_| false), None);
    }

    #[test]
    fn greeter_sessions_are_not_used() {
        let mut p = HashMap::new();
        for (k, v) in [("Type", "x11"), ("Active", "yes"), ("Remote", "no"), ("Class", "greeter")] {
            p.insert(k.to_string(), v.to_string());
        }
        assert!(!usable_session(&p));
        p.insert("Class".into(), "user".into());
        assert!(usable_session(&p));
    }

    #[test]
    fn placed_copy_is_reused_on_a_match_and_replaced_on_a_mismatch() {
        let dir = tmp("p");
        let bytes = vec![7u8; 5000];
        let sha = sha256_hex(&bytes);
        let p = placed_copy(&dir, &bytes, &sha).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), bytes);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o700);
        assert!(p.to_string_lossy().contains(&format!("/bin/deskd/{}-{}/k2-deskd", k2_deskd_proto::VERSION, &sha[..12])));
        std::fs::write(&p, b"tampered").unwrap();
        let p2 = placed_copy(&dir, &bytes, &sha).unwrap();
        assert_eq!(std::fs::read(&p2).unwrap(), bytes);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn state_names_and_messages() {
        assert_eq!(DeskdState::NeedsPermission { screen: false, accessibility: true }.name(), "needs_permission");
        assert_eq!(serde_json::to_value(DeskdState::Ready { backend: "x11".into() }).unwrap(), json!({"state": "ready", "backend": "x11"}));
        assert_eq!(DeskdState::NeedsApp.message(), "Install the K2 app on this Mac to use Remote desktop.");
    }
}
