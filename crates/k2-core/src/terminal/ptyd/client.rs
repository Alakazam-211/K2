//! The daemon's connection to k2-ptyd (PRD §7.5 `ptyd_client`).
//!
//! One process-global [`Ptyd`] per K2 home. On first use it finds the
//! `k2-ptyd` binary, places a versioned copy under `<home>/bin/ptyd/`
//! (P3), launches it through the double-fork launcher if no ptyd is
//! listening (P6), and connects as the **owner** with `takeover:true`.
//! A reader thread routes responses to waiting callers and `exited` /
//! `overrun` events to the session that registered for them.
//!
//! S7 (PRD §7.10, P65): on its first owner connection the daemon may ask
//! an older ptyd to exec itself into the current binary ([`decide_upgrade`]
//! in `super`). The owner connection then drops and the reader thread
//! reconnects IN PLACE (same [`Owner`], same boot id), so attached sessions
//! keep their handle, reattach at their byte offset and hear the exits that
//! happened while no owner was connected.

use std::collections::{HashMap, HashSet};
use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use k2_ptyd::auth;
use k2_ptyd::client::{ConnectError, Conn, DataStream};
use k2_ptyd::paths::Paths;
use k2_ptyd::proto::{self, Frame, Reply, Request, Role};

/// What a session hears from ptyd besides its byte stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    Exited { code: Option<i32>, signal: Option<i32> },
    Overrun { lost_bytes: u64 },
    /// The owner connection to ptyd is gone (ptyd died or evicted us).
    OwnerLost,
}

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_WAIT: Duration = Duration::from_secs(5);
/// `upgrade` runs ptyd's 10 s-bounded selftest before it answers.
const UPGRADE_TIMEOUT: Duration = Duration::from_secs(20);
/// How long the reader keeps reconnecting after an upgrade drop.
const RECONNECT_WAIT: Duration = Duration::from_secs(20);

/// What [`Owner::wait_reconnected`] saw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reconnect {
    /// The owner reconnected (generation `g`): reattach at your offset.
    Changed(u64),
    /// No reconnect happened (no upgrade, or it was refused).
    Unchanged,
    /// ptyd did not come back.
    Dead,
}

/// A request waiting for its response. `wants_fd`: the reply carries an
/// fd (SCM_RIGHTS) that the reader thread hands over with it.
struct Pending {
    tx: mpsc::SyncSender<(Reply, Option<OwnedFd>)>,
    wants_fd: bool,
}

/// The live owner connection.
pub struct Owner {
    writer: Mutex<UnixStream>,
    next_id: AtomicU64,
    pending: Mutex<HashMap<u64, Pending>>,
    watchers: Mutex<HashMap<String, mpsc::Sender<SessionEvent>>>,
    dead: AtomicBool,
    key: [u8; auth::KEY_LEN],
    sock: PathBuf,
    client_version: String,
    /// The version ptyd reported when this owner first connected (see
    /// [`Owner::version`] for the current one after an upgrade).
    pub ptyd_version: String,
    /// Kept identical across a self-upgrade (§7.10).
    pub boot_id: String,
    /// The running ptyd's protocol range (its challenge).
    pub proto_range: (u32, u32),
    version: Mutex<String>,
    /// Bumped on every in-place reconnect.
    generation: AtomicU64,
    /// An upgrade was requested: a dropped connection is reconnected.
    reconnecting: AtomicBool,
}

impl Owner {
    fn connect(paths: &Paths, client_version: &str) -> Result<Arc<Owner>, String> {
        let key = auth::read_key(&paths.key).map_err(|e| format!("key:{}", e.kind()))?;
        let sock = paths.client_sock();
        let mut conn = Conn::connect(&sock).map_err(|e| format!("connect:{}", e.kind()))?;
        conn.set_timeout(Some(REQUEST_TIMEOUT)).map_err(|e| format!("timeout:{}", e.kind()))?;
        let hello = conn.handshake(&key, client_version, Role::Owner, true).map_err(|e| format!("handshake:{e}"))?;
        conn.set_timeout(None).map_err(|e| format!("timeout:{}", e.kind()))?;
        let writer = conn.stream.try_clone().map_err(|e| format!("clone:{}", e.kind()))?;
        let owner = Arc::new(Owner {
            writer: Mutex::new(writer),
            next_id: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            watchers: Mutex::new(HashMap::new()),
            dead: AtomicBool::new(false),
            key,
            sock,
            client_version: client_version.to_string(),
            version: Mutex::new(hello.ptyd_version.clone()),
            ptyd_version: hello.ptyd_version,
            boot_id: hello.boot_id,
            proto_range: (hello.proto_min, hello.proto_max),
            generation: AtomicU64::new(0),
            reconnecting: AtomicBool::new(false),
        });
        let weak = Arc::downgrade(&owner);
        std::thread::Builder::new()
            .name("k2-ptyd-owner".into())
            .spawn(move || reader_loop(conn, weak))
            .map_err(|e| format!("thread:{}", e.kind()))?;
        Ok(owner)
    }

    pub fn is_dead(&self) -> bool {
        self.dead.load(Ordering::Acquire)
    }

    /// The running ptyd's version (changes after an in-place upgrade).
    pub fn version(&self) -> String {
        lock(&self.version).clone()
    }

    /// Bumped on every in-place reconnect (an upgrade).
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// An upgrade is in flight, or one completed since generation `since`:
    /// a data stream that just hit EOF should wait and reattach.
    pub fn upgrade_in_flight(&self, since: u64) -> bool {
        self.reconnecting.load(Ordering::Acquire) || self.generation() != since
    }

    /// Wait (bounded) until the owner reconnected past generation `since`,
    /// gave up, or no reconnect is pending any more.
    pub fn wait_reconnected(&self, since: u64, within: Duration) -> Reconnect {
        let t = Instant::now();
        loop {
            if self.is_dead() {
                return Reconnect::Dead;
            }
            let g = self.generation();
            if g != since {
                return Reconnect::Changed(g);
            }
            if !self.reconnecting.load(Ordering::Acquire) || t.elapsed() >= within {
                return Reconnect::Unchanged;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// §7.10: ask ptyd to exec `exe` in place. `Ok(new_version)` once the
    /// owner reconnected to the new image (same boot id). `Err` when ptyd
    /// refused (`upgrade_failed`/`unsupported`: this connection is
    /// untouched), when its exec failed (it reconnected on the old image),
    /// or when it did not come back (this owner is then dead).
    pub fn upgrade(&self, exe: &Path) -> Result<String, String> {
        let since = self.generation();
        let from = self.version();
        // Security review F6: ptyd execs only a file with this hash.
        let sha256 = std::fs::read(exe).map(|b| auth::sha256_hex(&b)).map_err(|e| format!("read {}: {e}", exe.display()))?;
        self.reconnecting.store(true, Ordering::Release);
        let r = self.request_timeout(Request::Upgrade { exe: exe.to_string_lossy().into_owned(), sha256: Some(sha256) }, UPGRADE_TIMEOUT);
        match r {
            Ok(Reply::Ok) => {}
            Ok(Reply::Error { code, detail }) => {
                self.reconnecting.store(false, Ordering::Release);
                return Err(format!("{code}: {detail}"));
            }
            Ok(other) => {
                self.reconnecting.store(false, Ordering::Release);
                return Err(format!("unexpected {other:?}"));
            }
            // The connection dropped before the answer: ptyd may still be
            // exec'ing (the reader is reconnecting) — wait and see.
            Err(_) if self.is_dead() || self.generation() != since || self.reconnecting.load(Ordering::Acquire) => {}
            Err(e) => {
                self.reconnecting.store(false, Ordering::Release);
                return Err(format!("no answer: {e}"));
            }
        }
        match self.wait_reconnected(since, RECONNECT_WAIT + Duration::from_secs(5)) {
            Reconnect::Changed(_) => {
                let now = self.version();
                if now == from {
                    Err(format!("exec failed: ptyd is still v{from}"))
                } else {
                    Ok(now)
                }
            }
            Reconnect::Dead => Err("ptyd did not come back after the upgrade".into()),
            Reconnect::Unchanged => {
                self.reconnecting.store(false, Ordering::Release);
                Err("ptyd answered ok but the connection never dropped".into())
            }
        }
    }

    /// Reader thread: reconnect after an upgrade drop. Retries until
    /// [`RECONNECT_WAIT`]; a refused connect (no listener: ptyd is gone), a
    /// bad mac or another boot id (another ptyd instance) is final.
    fn reconnect(&self) -> Result<Conn, String> {
        let t = Instant::now();
        loop {
            match self.try_reconnect() {
                Ok(c) => return Ok(c),
                Err((fatal, e)) if fatal || t.elapsed() >= RECONNECT_WAIT => {
                    self.reconnecting.store(false, Ordering::Release);
                    return Err(e);
                }
                Err(_) => std::thread::sleep(Duration::from_millis(25)),
            }
        }
    }

    fn try_reconnect(&self) -> Result<Conn, (bool, String)> {
        let mut conn = Conn::connect(&self.sock).map_err(|e| {
            let fatal = matches!(e.kind(), io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound);
            (fatal, format!("connect:{}", e.kind()))
        })?;
        conn.set_timeout(Some(REQUEST_TIMEOUT)).map_err(|e| (false, format!("timeout:{}", e.kind())))?;
        let hello = conn.handshake(&self.key, &self.client_version, Role::Owner, true).map_err(|e| match e {
            ConnectError::Refused { code, detail } => (code == "bad_mac" || code == "proto", format!("refused:{code} {detail}")),
            other => (false, format!("handshake:{other}")),
        })?;
        if hello.boot_id != self.boot_id {
            return Err((true, "a different ptyd instance answered".into()));
        }
        // Exits that happened while no owner was connected were never sent
        // as events: learn them from `list` (id 0 is never a request id).
        conn.send(&Frame::new(Some(0), Request::List)).map_err(|e| (false, format!("list:{}", e.kind())))?;
        let mut notified = HashSet::new();
        let sessions = loop {
            let f = conn.recv().map_err(|e| (false, format!("list:{}", e.kind())))?;
            match (f.id, f.body) {
                (Some(0), Reply::Sessions { sessions }) => break sessions,
                (Some(0), other) => return Err((false, format!("list: {other:?}"))),
                (None, ev) => {
                    if let Reply::Exited { sid, .. } = &ev {
                        notified.insert(sid.clone());
                    }
                    self.on_event(ev);
                }
                (Some(_), _) => {}
            }
        };
        conn.set_timeout(None).map_err(|e| (false, format!("timeout:{}", e.kind())))?;
        let writer = conn.stream.try_clone().map_err(|e| (false, format!("clone:{}", e.kind())))?;
        *lock(&self.writer) = writer;
        *lock(&self.version) = hello.ptyd_version;
        for (sid, w) in lock(&self.watchers).iter() {
            if notified.contains(sid) {
                continue;
            }
            match sessions.iter().find(|s| &s.sid == sid).map(|s| &s.state) {
                Some(k2_ptyd::proto::SessionState::Exited { code, signal, .. }) => {
                    let _ = w.send(SessionEvent::Exited { code: *code, signal: *signal });
                }
                Some(k2_ptyd::proto::SessionState::Running) => {}
                None => {
                    let _ = w.send(SessionEvent::OwnerLost);
                }
            }
        }
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.reconnecting.store(false, Ordering::Release);
        Ok(conn)
    }

    /// Route an event to its session. `false`: stop reading (evicted).
    fn on_event(&self, body: Reply) -> bool {
        match body {
            Reply::Exited { sid, code, signal, .. } => {
                if let Some(w) = lock(&self.watchers).get(&sid) {
                    let _ = w.send(SessionEvent::Exited { code, signal });
                }
            }
            Reply::Overrun { sid, lost_bytes } => {
                crate::log_debug!("[ptyd] overrun session={} lost={}", sid, lost_bytes);
                if let Some(w) = lock(&self.watchers).get(&sid) {
                    let _ = w.send(SessionEvent::Overrun { lost_bytes });
                }
            }
            Reply::Evicted => {
                crate::log_debug!("[ptyd] evicted by a newer owner");
                self.mark_dead();
                return false;
            }
            Reply::OrphanWarning { seconds_left } => {
                crate::log_debug!("[ptyd] orphan warning: {}s left", seconds_left);
            }
            _ => {}
        }
        true
    }

    /// Send a request and wait for its response.
    pub fn request(&self, body: Request) -> io::Result<Reply> {
        self.request_inner(body, None, false).map(|(r, _)| r)
    }

    /// [`Self::request`] with its own response timeout.
    pub fn request_timeout(&self, body: Request, timeout: Duration) -> io::Result<Reply> {
        self.request_inner_t(body, None, false, timeout).map(|(r, _)| r)
    }

    /// Send a request whose reply carries an fd (`take_fd`,
    /// `hold_unix_listener`). `Ok((reply, None))` when ptyd answered
    /// without one (an error reply).
    pub fn request_fd(&self, body: Request) -> io::Result<(Reply, Option<OwnedFd>)> {
        self.request_inner(body, None, true)
    }

    /// Send a request with an fd attached (`hold_fd`).
    pub fn request_with_fd(&self, body: Request, fd: &impl AsRawFd) -> io::Result<Reply> {
        self.request_inner(body, Some(fd.as_raw_fd()), false).map(|(r, _)| r)
    }

    fn request_inner(&self, body: Request, pass: Option<std::os::fd::RawFd>, wants_fd: bool) -> io::Result<(Reply, Option<OwnedFd>)> {
        self.request_inner_t(body, pass, wants_fd, REQUEST_TIMEOUT)
    }

    fn request_inner_t(
        &self,
        body: Request,
        pass: Option<std::os::fd::RawFd>,
        wants_fd: bool,
        timeout: Duration,
    ) -> io::Result<(Reply, Option<OwnedFd>)> {
        if self.is_dead() {
            return Err(io::Error::new(io::ErrorKind::NotConnected, "ptyd owner connection lost"));
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::sync_channel(1);
        lock(&self.pending).insert(id, Pending { tx, wants_fd });
        let bytes = proto::encode(&Frame::new(Some(id), body))?;
        {
            use std::io::Write;
            let mut w = lock(&self.writer);
            let sent = match pass {
                Some(fd) => k2_ptyd::sys::send_with_fd(w.as_raw_fd(), &bytes, Some(fd)).and_then(|n| w.write_all(&bytes[n..])),
                None => w.write_all(&bytes),
            };
            if let Err(e) = sent {
                lock(&self.pending).remove(&id);
                return Err(e);
            }
        }
        match rx.recv_timeout(timeout) {
            Ok(r) => Ok(r),
            Err(_) => {
                lock(&self.pending).remove(&id);
                Err(io::Error::new(io::ErrorKind::TimedOut, "ptyd did not answer"))
            }
        }
    }

    /// `hold_unix_listener`: ptyd binds (or re-hands) a listening Unix
    /// socket that outlives the daemon (P44). Returns our dup of it.
    pub fn hold_unix_listener(&self, name: &str, path: &Path, mode: u32) -> io::Result<std::os::unix::net::UnixListener> {
        let req = Request::HoldUnixListener { name: name.into(), path: path.to_string_lossy().into_owned(), mode, owner_uid: None };
        match self.request_fd(req)? {
            (Reply::Ok, Some(fd)) => Ok(std::os::unix::net::UnixListener::from(fd)),
            (Reply::Ok, None) => Err(io::Error::other("hold_unix_listener: no fd in the reply")),
            (Reply::Error { code, detail }, _) => Err(io::Error::other(format!("hold_unix_listener: {code} {detail}"))),
            (other, _) => Err(io::Error::other(format!("hold_unix_listener: unexpected {other:?}"))),
        }
    }

    /// `take_fd`: a dup of a held fd, `Ok(None)` when ptyd holds nothing
    /// under `name`.
    pub fn take_fd(&self, name: &str) -> io::Result<Option<OwnedFd>> {
        match self.request_fd(Request::TakeFd { name: name.into() })? {
            (Reply::Ok, Some(fd)) => Ok(Some(fd)),
            (Reply::Ok, None) => Err(io::Error::other("take_fd: no fd in the reply")),
            (Reply::Error { code, .. }, _) if code == "unknown" => Ok(None),
            (Reply::Error { code, detail }, _) => Err(io::Error::other(format!("take_fd: {code} {detail}"))),
            (other, _) => Err(io::Error::other(format!("take_fd: unexpected {other:?}"))),
        }
    }

    /// `hold_fd`: ptyd keeps a copy of `fd` under `name` (replacing any).
    pub fn hold_fd(&self, name: &str, fd: &impl AsRawFd) -> io::Result<()> {
        match self.request_with_fd(Request::HoldFd { name: name.into() }, fd)? {
            Reply::Ok => Ok(()),
            other => Err(io::Error::other(format!("hold_fd: {other:?}"))),
        }
    }

    /// `release_fd`: ptyd closes its copy (and unlinks a held listener's
    /// path). Fire-and-forget: an unknown name is fine.
    pub fn release_fd(&self, name: &str) {
        self.send_nowait(Request::ReleaseFd { name: name.into() });
    }

    /// Fire-and-forget request (the response is discarded by the reader).
    /// For updates nobody waits on, e.g. `update_meta` (P28).
    pub fn send_nowait(&self, body: Request) {
        if self.is_dead() {
            return;
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        if let Ok(bytes) = proto::encode(&Frame::new(Some(id), body)) {
            use std::io::Write;
            let _ = lock(&self.writer).write_all(&bytes);
        }
    }

    /// Register for a session's events. Call before `spawn` so an instant
    /// exit can't be missed.
    pub fn watch(&self, sid: &str) -> mpsc::Receiver<SessionEvent> {
        let (tx, rx) = mpsc::channel();
        lock(&self.watchers).insert(sid.to_string(), tx);
        rx
    }

    pub fn unwatch(&self, sid: &str) {
        lock(&self.watchers).remove(sid);
    }

    /// Open a data stream for `sid` from `from`.
    pub fn attach(&self, sid: &str, from: u64) -> io::Result<DataStream> {
        let mut conn = Conn::connect(&self.sock)?;
        conn.set_timeout(Some(REQUEST_TIMEOUT))?;
        conn.handshake(&self.key, &self.client_version, Role::Data, false).map_err(io::Error::other)?;
        conn.send(&Frame::new(Some(1), Request::Attach { sid: sid.into(), from }))?;
        match conn.recv()?.body {
            Reply::Attached { start, end } => {
                let pending = conn.take_rest();
                k2_ptyd::client::clear_timeout_after_attach(&conn)?;
                Ok(DataStream::from_parts(conn.stream, start, end, pending))
            }
            Reply::Error { code, .. } => Err(io::Error::other(format!("attach refused: {code}"))),
            other => Err(io::Error::other(format!("expected attached, got {other:?}"))),
        }
    }

    fn mark_dead(&self) {
        if self.dead.swap(true, Ordering::AcqRel) {
            return;
        }
        lock(&self.pending).clear(); // waiters see a disconnected channel
        for (_, w) in lock(&self.watchers).drain() {
            let _ = w.send(SessionEvent::OwnerLost);
        }
    }
}

fn reader_loop(mut conn: Conn, owner: std::sync::Weak<Owner>) {
    loop {
        let frame = conn.recv();
        let Some(o) = owner.upgrade() else { return };
        let frame = match frame {
            Ok(f) => f,
            Err(_) => {
                if o.reconnecting.load(Ordering::Acquire) {
                    // §7.10: ptyd is exec'ing its new image. Its listener
                    // never closed: reconnect in place. Requests in flight
                    // on the old connection will never be answered.
                    lock(&o.pending).clear();
                    match o.reconnect() {
                        Ok(c) => {
                            crate::log_debug!("[ptyd] owner reconnected after an upgrade (ptyd v{})", o.version());
                            conn = c;
                            continue;
                        }
                        Err(e) => crate::log_debug!("[ptyd] reconnect after an upgrade failed: {}", e),
                    }
                }
                crate::log_debug!("[ptyd] owner connection closed");
                o.mark_dead();
                return;
            }
        };
        match frame.id {
            Some(id) => {
                if let Some(p) = lock(&o.pending).remove(&id) {
                    // An fd rides the same sendmsg as its reply, so it is
                    // queued by the time the reply decodes.
                    let fd = if p.wants_fd && matches!(frame.body, Reply::Ok) { conn.take_fd() } else { None };
                    let _ = p.tx.send((frame.body, fd));
                }
            }
            None => {
                if !o.on_event(frame.body) {
                    return;
                }
            }
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

/// Per-K2-home ptyd handle.
pub struct Ptyd {
    paths: Paths,
    owner: Mutex<Option<Arc<Owner>>>,
    /// The leftover sweep runs once, on this process's first owner
    /// connection.
    swept: AtomicBool,
    /// P65: the self-upgrade decision runs once, on this process's first
    /// owner connection (B0 for a restarted daemon).
    upgrade_checked: AtomicBool,
}

/// Kill (and ack) every session in `sids`. For sessions that can't be
/// adopted (S3: meta without a session key, e.g. spawned by 0.46.3, which
/// had no adoption) and for an explicit end. Returns how many ran.
pub fn end_sessions(owner: &Owner, sids: &[String]) -> usize {
    let mut ended = 0;
    for sid in sids {
        if matches!(owner.request(Request::Kill { sid: sid.clone(), grace_ms: Some(100) }), Ok(Reply::Ok)) {
            ended += 1;
        }
        let _ = owner.request(Request::AckExit { sid: sid.clone() });
    }
    ended
}

static HOME_OVERRIDE: OnceLock<PathBuf> = OnceLock::new();
static GLOBAL: OnceLock<Ptyd> = OnceLock::new();

/// Tests: point the global handle at a temp K2 home. Must run before the
/// first [`global`] call in the process.
pub fn set_home_for_tests(home: &Path) {
    let _ = HOME_OVERRIDE.set(home.to_path_buf());
}

/// The process-global handle for this daemon's K2 home.
pub fn global() -> &'static Ptyd {
    GLOBAL.get_or_init(|| {
        let home = HOME_OVERRIDE.get().cloned().unwrap_or_else(crate::paths::k2_home);
        Ptyd::for_home(&home)
    })
}

/// The K2 version the daemon presents in `hello` (bound into the mac).
pub fn client_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

impl Ptyd {
    /// A handle for an explicit K2 home (the global one uses the daemon's).
    pub fn for_home(home: &Path) -> Ptyd {
        Ptyd { paths: Paths::from_home(home), owner: Mutex::new(None), swept: AtomicBool::new(false), upgrade_checked: AtomicBool::new(false) }
    }

    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    /// A live owner connection, connecting (and launching ptyd) if needed.
    pub fn owner(&self) -> Result<Arc<Owner>, String> {
        self.owner_inner(true)
    }

    /// B0 (PRD §7.9): connect to a ptyd that is ALREADY running, never
    /// launch one. `Err("no_ptyd")` when no control socket exists (the
    /// common case: switch off, or nothing survived) — that check is a
    /// single `stat` and logs nothing.
    pub fn connect_existing(&self) -> Result<Arc<Owner>, String> {
        if !self.paths.client_sock().exists() {
            return Err("no_ptyd".into());
        }
        self.owner_inner(false)
    }

    /// The current owner connection, if one is live (never connects).
    pub fn current_owner(&self) -> Option<Arc<Owner>> {
        lock(&self.owner).as_ref().filter(|o| !o.is_dead()).cloned()
    }

    fn owner_inner(&self, may_launch: bool) -> Result<Arc<Owner>, String> {
        let mut g = lock(&self.owner);
        if let Some(o) = g.as_ref() {
            if !o.is_dead() {
                return Ok(Arc::clone(o));
            }
        }
        let o = match Owner::connect(&self.paths, client_version()) {
            Ok(o) => o,
            Err(first) if !may_launch => return Err(first),
            Err(first) => {
                let exe = run_location(&self.paths.home)?;
                k2_ptyd::launch::spawn_detached(&exe, &self.paths.home, &[]).map_err(|e| {
                    crate::log_debug!("[ptyd] launch failed: {} (connect said {})", e, first);
                    "launch_failed".to_string()
                })?;
                let t = Instant::now();
                loop {
                    match Owner::connect(&self.paths, client_version()) {
                        Ok(o) => break o,
                        Err(e) if t.elapsed() > CONNECT_WAIT => return Err(e),
                        Err(_) => std::thread::sleep(Duration::from_millis(25)),
                    }
                }
            }
        };
        crate::log_debug!("[ptyd] owner connected (ptyd v{}, boot {})", o.ptyd_version, &o.boot_id[..8.min(o.boot_id.len())]);
        if !self.upgrade_checked.swap(true, Ordering::AcqRel) {
            self.maybe_upgrade(&o);
            if o.is_dead() {
                return Err("ptyd_lost_in_upgrade".into());
            }
        }
        if !self.swept.swap(true, Ordering::AcqRel) {
            // Sessions a previous daemon left in ptyd are NOT touched here:
            // boot's B0/B1 (`terminal::ptyd::adopt`) lists and adopts them.
            // P3: prune old versioned copies, keeping the current one and
            // the running ptyd's image.
            let mut keep = Vec::new();
            if let Ok(cur) = run_location(&self.paths.home) {
                keep.push(cur);
            }
            if let Some(exe) = self.ptyd_pid().and_then(exe_of) {
                keep.push(exe);
            }
            if !keep.is_empty() {
                let pruned = prune_versioned(&self.paths.home, &keep);
                if pruned > 0 {
                    crate::log_debug!("[ptyd] pruned {} old versioned cop{}", pruned, if pruned == 1 { "y" } else { "ies" });
                }
            }
        }
        *g = Some(Arc::clone(&o));
        Ok(o)
    }

    /// `Ok(())` when ptyd can host sessions now (connects/launches).
    /// P65 (§7.10): upgrade the running ptyd in place when the decision
    /// says so. Never fails the connection: a refused or failed upgrade
    /// keeps the running ptyd (logged).
    fn maybe_upgrade(&self, o: &Owner) {
        let from = o.version();
        let mut exe = None;
        let decision = super::decide_upgrade(&from, o.proto_range, (proto::PROTO_MIN, proto::PROTO_MAX), &super::min_ptyd_version(), || {
            let e = run_location(&self.paths.home).ok()?;
            let v = probe_version(&e);
            exe = Some(e);
            v
        });
        match decision {
            super::UpgradeDecision::Keep(why) => {
                if why != "current" {
                    crate::log_debug!("[ptyd] keeping k2-ptyd v{} ({})", from, why);
                }
            }
            super::UpgradeDecision::Upgrade { reason, to } => {
                let Some(exe) = exe else { return };
                crate::log_debug!("[ptyd] upgrading k2-ptyd in place: v{} -> v{} ({})", from, to, reason);
                match o.upgrade(&exe) {
                    Ok(now) => crate::log_debug!(
                        "[ptyd] upgraded k2-ptyd in place: v{} -> v{} (pid {}, boot {} kept)",
                        from,
                        now,
                        self.ptyd_pid().map(|p| p.to_string()).unwrap_or_else(|| "?".into()),
                        &o.boot_id[..8.min(o.boot_id.len())]
                    ),
                    Err(e) => crate::log_debug!("[ptyd] k2-ptyd upgrade not done ({}); running v{}", e, o.version()),
                }
            }
        }
    }

    /// §7.10: upgrade the running ptyd to `exe` now, regardless of P65
    /// (tests, diagnostics). `Ok(new_version)`.
    pub fn upgrade_now(&self, exe: &Path) -> Result<String, String> {
        let o = self.current_owner().ok_or("no_owner")?;
        o.upgrade(exe)
    }

    pub fn is_available(&self) -> Result<(), String> {
        self.owner().map(|_| ())
    }

    /// ptyd's pid from its lock file (P74's vault ancestry check, later).
    pub fn ptyd_pid(&self) -> Option<i32> {
        std::fs::read_to_string(&self.paths.lock).ok()?.trim().parse().ok()
    }
}

/// The version a k2-ptyd binary reports (`<exe> --version` →
/// `k2-ptyd <version> proto <n>`), bounded to 5 s. `None` if it doesn't
/// answer like a k2-ptyd.
pub fn probe_version(exe: &Path) -> Option<String> {
    use std::io::Read;
    let mut child = std::process::Command::new(exe)
        .arg("--version")
        .env_clear()
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let t = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) if st.success() => break,
            Ok(Some(_)) | Err(_) => return None,
            Ok(None) if t.elapsed() > Duration::from_secs(5) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    parse_version_line(&out)
}

/// `k2-ptyd 0.2.0 proto 1` → `0.2.0`.
pub fn parse_version_line(out: &str) -> Option<String> {
    let mut w = out.split_whitespace();
    match (w.next(), w.next()) {
        (Some("k2-ptyd"), Some(v)) if proto::parse_version(v).is_some() => Some(v.to_string()),
        _ => None,
    }
}

/// File name of the ptyd binary beside the daemon. Pinned against
/// `scripts/macos-daemon-helper-app.sh` (`K2_DAEMON_HELPER_PTYD`) and the
/// Linux packages by the k2-daemon layout tests.
pub const PTYD_BINARY_NAME: &str = "k2-ptyd";

/// The shipped binary: `K2_PTYD_BIN` (dev/tests) or `k2-ptyd` beside the
/// daemon executable (the macOS bundle nests it beside `k2-daemon` in
/// `K2 Daemon.app/Contents/MacOS`; the Linux packages install it beside
/// `/usr/bin/k2-daemon`). Linux release daemons also embed it (P84).
/// `K2_PTYD_BIN` — dev/test builds only (security review F6: in a release
/// build an env var must not be able to pick the binary that becomes the
/// parent of every agent).
fn ptyd_bin_env() -> Option<std::ffi::OsString> {
    if !cfg!(debug_assertions) {
        return None;
    }
    std::env::var_os("K2_PTYD_BIN").filter(|p| !p.is_empty())
}

pub fn shipped_binary() -> Result<PathBuf, String> {
    if let Some(p) = ptyd_bin_env() {
        let p = PathBuf::from(p);
        return if p.is_file() { Ok(p) } else { Err("bin_env_missing".into()) };
    }
    let exe = std::env::current_exe().map_err(|_| "no_current_exe".to_string())?;
    let sib = exe.parent().map(|d| d.join(PTYD_BINARY_NAME)).ok_or("no_exe_dir")?;
    if sib.is_file() {
        Ok(sib)
    } else {
        Err("no_binary".into())
    }
}

static EMBEDDED: OnceLock<(&'static [u8], &'static str)> = OnceLock::new();

/// P84: the daemon registers the k2-ptyd bytes its build embedded (Linux
/// release builds) with their sha256.
pub fn set_embedded(bytes: &'static [u8], sha256: &'static str) {
    let _ = EMBEDDED.set((bytes, sha256));
}

/// P3: run from `<home>/bin/ptyd/<ptyd_version>-<sha256[0..12]>/k2-ptyd`
/// (0700), reused on a hash match. S0 showed the location doesn't affect
/// macOS TCC, and nothing ever overwrites this copy in place.
///
/// Source order: `K2_PTYD_BIN` (dev/tests), then the embedded copy (P84:
/// authoritative when present, hash-checked), then the binary beside the
/// daemon (macOS bundle, packages).
pub fn run_location(home: &Path) -> Result<PathBuf, String> {
    let from_env = ptyd_bin_env().is_some();
    place_binary(home, from_env, EMBEDDED.get().copied())
}

/// Log the k2-ptyd source once per process (which copy runs: P84 support).
fn note_source(what: &str) {
    static NOTED: AtomicBool = AtomicBool::new(false);
    if !NOTED.swap(true, Ordering::AcqRel) {
        crate::log_debug!("[ptyd] k2-ptyd source: {what}");
    }
}

fn place_binary(home: &Path, from_env: bool, embedded: Option<(&[u8], &str)>) -> Result<PathBuf, String> {
    if !from_env {
        if let Some((bytes, sha)) = embedded {
            if auth::sha256_hex(bytes) != sha {
                return Err("embed_hash_mismatch".into());
            }
            note_source(&format!("embedded in the daemon (sha256 {})", &sha[..12.min(sha.len())]));
            return versioned_copy(home, bytes, sha).map_err(|e| format!("copy_bin:{}", e.kind()));
        }
    }
    let src = shipped_binary()?;
    note_source(&src.display().to_string());
    let bytes = std::fs::read(&src).map_err(|e| format!("read_bin:{}", e.kind()))?;
    let hash = auth::sha256_hex(&bytes);
    versioned_copy(home, &bytes, &hash).map_err(|e| format!("copy_bin:{}", e.kind()))
}

/// P3: delete versioned copies under `<home>/bin/ptyd/` except the dirs of
/// the paths in `keep` (the current copy and the running ptyd's image).
/// Returns how many were removed.
pub fn prune_versioned(home: &Path, keep: &[PathBuf]) -> usize {
    let root = home.join("bin").join("ptyd");
    let keep_dirs: Vec<PathBuf> = keep
        .iter()
        .filter_map(|p| p.parent().map(|d| dunce::canonicalize(&d).unwrap_or_else(|_| d.to_path_buf())))
        .collect();
    let Ok(rd) = std::fs::read_dir(&root) else { return 0 };
    let mut removed = 0;
    for e in rd.flatten() {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        let canon = dunce::canonicalize(&p).unwrap_or_else(|_| p.clone());
        if keep_dirs.contains(&canon) {
            continue;
        }
        if std::fs::remove_dir_all(&p).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// The executable path of a running process (the live ptyd's image).
pub fn exe_of(pid: i32) -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_link(format!("/proc/{pid}/exe")).ok()
    }
    #[cfg(target_os = "macos")]
    {
        let mut buf = vec![0u8; 4096];
        // SAFETY: proc_pidpath writes at most buf.len() bytes.
        let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr() as *mut libc::c_void, buf.len() as u32) };
        if n <= 0 {
            return None;
        }
        buf.truncate(n as usize);
        Some(PathBuf::from(String::from_utf8_lossy(&buf).into_owned()))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = pid;
        None
    }
}

pub fn versioned_copy(home: &Path, bytes: &[u8], hash: &str) -> io::Result<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let dir = home.join("bin").join("ptyd").join(format!("{}-{}", proto::PTYD_VERSION, &hash[..12]));
    let target = dir.join("k2-ptyd");
    if let Ok(existing) = std::fs::read(&target) {
        if auth::sha256_hex(&existing) == hash {
            return Ok(target);
        }
    }
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
    let tmp = dir.join(format!(".k2-ptyd.{}.tmp", std::process::id()));
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o700).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, &target)?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versioned_copy_is_reused_on_a_hash_match_and_replaced_otherwise() {
        use std::os::unix::fs::PermissionsExt;
        let home = std::env::temp_dir().join(format!("k2-ptyd-vc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let a = b"binary-a".to_vec();
        let ha = auth::sha256_hex(&a);
        let p = versioned_copy(&home, &a, &ha).unwrap();
        assert!(p.to_string_lossy().contains(&format!("{}-{}", proto::PTYD_VERSION, &ha[..12])));
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o700);
        let mtime = std::fs::metadata(&p).unwrap().modified().unwrap();
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(versioned_copy(&home, &a, &ha).unwrap(), p);
        assert_eq!(std::fs::metadata(&p).unwrap().modified().unwrap(), mtime, "reused, not rewritten");
        std::fs::write(&p, b"tampered").unwrap();
        versioned_copy(&home, &a, &ha).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), a, "a hash mismatch is rewritten");
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn version_line_parses_only_a_k2_ptyd_answer() {
        assert_eq!(parse_version_line("k2-ptyd 0.2.0 proto 1\n").as_deref(), Some("0.2.0"));
        assert_eq!(parse_version_line("k2-ptyd 0.1.0 proto 1").as_deref(), Some("0.1.0"));
        assert_eq!(parse_version_line("bash 5.2.0"), None);
        assert_eq!(parse_version_line("k2-ptyd nope"), None);
        assert_eq!(parse_version_line(""), None);
    }

    #[test]
    fn prune_keeps_current_and_running_copies_only() {
        let home = std::env::temp_dir().join(format!("k2-ptyd-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let a = versioned_copy(&home, b"a-image", &auth::sha256_hex(b"a-image")).unwrap();
        let b = versioned_copy(&home, b"b-image", &auth::sha256_hex(b"b-image")).unwrap();
        let c = versioned_copy(&home, b"c-image", &auth::sha256_hex(b"c-image")).unwrap();
        assert_eq!(prune_versioned(&home, &[a.clone(), c.clone()]), 1);
        assert!(a.exists() && c.exists() && !b.exists());
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn exe_of_reports_our_own_image() {
        // SAFETY: getpid.
        let me = exe_of(unsafe { libc::getpid() }).expect("own exe");
        assert_eq!(dunce::canonicalize(&me).unwrap(), dunce::canonicalize(&std::env::current_exe().unwrap()).unwrap());
    }

    #[test]
    fn embedded_binary_is_placed_verified_and_authoritative() {
        let home = std::env::temp_dir().join(format!("k2-ptyd-embed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let bytes: &[u8] = b"embedded-ptyd-image";
        let sha = auth::sha256_hex(bytes);
        let p = place_binary(&home, false, Some((bytes, sha.as_str()))).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), bytes);
        assert_eq!(
            place_binary(&home, false, Some((bytes, "0000000000000000000000000000000000000000000000000000000000000000"))).unwrap_err(),
            "embed_hash_mismatch"
        );
        std::fs::remove_dir_all(&home).unwrap();
    }
}
