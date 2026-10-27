//! The ptyd server (PRD §7.1–§7.4, §7.11): one thread, one `poll(2)` loop
//! over the control socket, client connections, every PTY master and a
//! signal self-pipe. Masters are ALWAYS read (P21), so an agent never blocks
//! on output while no daemon is attached.

use std::collections::{HashMap, VecDeque};
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::auth;
use crate::paths::Paths;
use crate::proto::{
    self, Frame, FrameDecoder, Reply, Request, Role, SessionInfo, SessionState, Sig, SigTarget, MAX_META,
    PROTO_MAX, PROTO_MIN, PTYD_VERSION,
};
use crate::ring::{clamp_ring, Ring, TOTAL_CAP};
use crate::spawn::{spawn, SpawnSpec};
use crate::sys::{self, cvt, now_secs, set_cloexec, set_nonblocking};

mod upgrade;
pub use upgrade::{resume, selftest_reply, STATE_FORMAT};

/// Runtime knobs. Defaults are the PRD's; tests shorten `idle`.
#[derive(Debug, Clone)]
pub struct Config {
    pub paths: Paths,
    /// §7.11: default 900 s, minimum 120 s.
    pub orphan: Duration,
    /// P7: idle exit after 60 s with nothing to do.
    pub idle: Duration,
    /// P25: exited records live 1 h with no owner.
    pub exited_ttl: Duration,
    /// Launcher pipe: one status byte once listening.
    pub ready_fd: Option<RawFd>,
}

pub const ORPHAN_DEFAULT: Duration = Duration::from_secs(900);
pub const ORPHAN_MIN: Duration = Duration::from_secs(120);
pub const IDLE_DEFAULT: Duration = Duration::from_secs(60);
pub const EXITED_TTL: Duration = Duration::from_secs(3600);
const KILL_GRACE_DEFAULT: u64 = 100;
const SHUTDOWN_GRACE_MS: u64 = 2000;
const CTRL_WBUF_MAX: usize = 16 << 20;
const READ_CHUNK: usize = 64 << 10;
const MAX_READ_PER_WAKE: usize = 1 << 20;

/// Clamp a requested orphan timeout (env `K2_PTYD_ORPHAN_SECS`).
pub fn orphan_from_env(v: Option<&str>) -> Duration {
    match v.and_then(|s| s.trim().parse::<u64>().ok()) {
        Some(n) => Duration::from_secs(n).max(ORPHAN_MIN),
        None => ORPHAN_DEFAULT,
    }
}

/// Exit outcomes of [`serve`].
#[derive(Debug, PartialEq, Eq)]
pub enum Exit {
    /// Normal exit (idle, shutdown, orphan, signal).
    Clean,
    /// Another ptyd owns this K2 home (P4).
    AlreadyRunning(Option<i32>),
}

// ---------------------------------------------------------------- logging

/// Size at which `ptyd.log` rotates to `ptyd.log.1` (one generation kept).
/// Any peer can make ptyd log a line (e.g. `frame_too_large` per bad
/// frame) and ptyd lives as long as the agents, so the log is bounded
/// (fuzz finding, 2026-10-20).
pub const LOG_MAX_BYTES: u64 = 8 << 20;

pub struct Log {
    path: PathBuf,
    f: Option<std::fs::File>,
    written: u64,
    max: u64,
}

impl Log {
    fn open(path: &Path) -> Log {
        Log::open_capped(path, LOG_MAX_BYTES)
    }

    pub fn open_capped(path: &Path, max: u64) -> Log {
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let (f, written) = Log::open_file(path);
        Log { path: path.to_path_buf(), f, written, max }
    }

    fn open_file(path: &Path) -> (Option<std::fs::File>, u64) {
        use std::os::unix::fs::OpenOptionsExt;
        let f = std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(path).ok();
        let len = f.as_ref().and_then(|f| f.metadata().ok()).map(|m| m.len()).unwrap_or(0);
        (f, len)
    }

    /// S-4/P8: callers pass only fixed text, sids, pids and error kinds.
    /// Never PTY bytes, env values or meta.
    pub fn line(&mut self, msg: &str) {
        if self.written >= self.max {
            let mut rotated = self.path.clone().into_os_string();
            rotated.push(".1");
            self.f = None;
            let _ = std::fs::rename(&self.path, &rotated);
            let (f, len) = Log::open_file(&self.path);
            self.f = f;
            self.written = len;
        }
        if let Some(f) = self.f.as_mut() {
            let line = format!("{} k2-ptyd[{}] {}\n", now_secs(), std::process::id(), msg);
            if f.write_all(line.as_bytes()).is_ok() {
                self.written += line.len() as u64;
            }
        }
    }
}

// ---------------------------------------------------------------- state

enum SState {
    Running,
    Exited { code: Option<i32>, signal: Option<i32>, at: u64, when: Instant },
}

struct Session {
    pid: i32,
    master: Option<OwnedFd>,
    ring: Ring,
    state: SState,
    cols: u16,
    rows: u16,
    started_at: u64,
    meta: serde_json::Value,
    input: VecDeque<u8>,
    /// Attached data connection.
    stream: Option<u64>,
    /// Next ring offset to send to `stream`.
    sent: u64,
    kill_deadline: Option<Instant>,
    kill_waiters: Vec<(u64, Option<u64>)>,
}

impl Session {
    fn running(&self) -> bool {
        matches!(self.state, SState::Running)
    }

    fn info(&self, sid: &str) -> SessionInfo {
        SessionInfo {
            sid: sid.to_string(),
            pid: self.pid,
            state: match self.state {
                SState::Running => SessionState::Running,
                SState::Exited { code, signal, at, .. } => SessionState::Exited { code, signal, at },
            },
            cols: self.cols,
            rows: self.rows,
            ring_start: self.ring.start(),
            ring_end: self.ring.end(),
            started_at: self.started_at,
            meta: self.meta.clone(),
        }
    }
}

enum Kind {
    Hello { nonce: [u8; 32] },
    Owner,
    /// Authenticated data connection; `sid` once attached.
    Data { sid: Option<String> },
}

struct Conn {
    sock: OwnedFd,
    kind: Kind,
    dec: FrameDecoder,
    wbuf: VecDeque<u8>,
    /// Absolute stream position (bytes queued so far) at which an fd rides.
    wfds: VecDeque<(u64, OwnedFd)>,
    queued: u64,
    written: u64,
    rfds: VecDeque<OwnedFd>,
    /// Close once `wbuf` drains.
    closing: bool,
}

impl Conn {
    fn new(sock: OwnedFd, kind: Kind) -> Conn {
        Conn {
            sock,
            kind,
            dec: FrameDecoder::default(),
            wbuf: VecDeque::new(),
            wfds: VecDeque::new(),
            queued: 0,
            written: 0,
            rfds: VecDeque::new(),
            closing: false,
        }
    }

    fn queue(&mut self, id: Option<u64>, body: Reply) {
        match proto::encode(&Frame::new(id, body)) {
            Ok(bytes) => {
                self.queued += bytes.len() as u64;
                self.wbuf.extend(bytes);
            }
            Err(_) => self.closing = true,
        }
    }

    fn queue_with_fd(&mut self, id: Option<u64>, body: Reply, fd: OwnedFd) {
        let at = self.queued;
        self.queue(id, body);
        self.wfds.push_back((at, fd));
    }
}

struct Held {
    fd: OwnedFd,
    unlink: Option<PathBuf>,
}

struct Server {
    cfg: Config,
    log: Log,
    key: [u8; auth::KEY_LEN],
    boot_id: String,
    listener: OwnedFd,
    sig_r: OwnedFd,
    conns: HashMap<u64, Conn>,
    next_conn: u64,
    sessions: HashMap<String, Session>,
    owner: Option<u64>,
    owner_since: Option<u64>,
    /// When the owner left (or ptyd started with none).
    ownerless_since: Option<Instant>,
    idle_since: Option<Instant>,
    held: HashMap<String, Held>,
    /// Pending `shutdown{end_sessions:true}`: reply target + SIGKILL deadline.
    shutting_down: Option<(u64, Option<u64>, Instant)>,
    exit_now: bool,
    orphan_warned: bool,
    _lock: std::fs::File,
}

// ---------------------------------------------------------------- signals

static SIG_W: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);

extern "C" fn on_signal(sig: libc::c_int) {
    let fd = SIG_W.load(std::sync::atomic::Ordering::Relaxed);
    if fd >= 0 {
        let b = sig as u8;
        // SAFETY: write(2) is async-signal-safe.
        unsafe { libc::write(fd, &b as *const u8 as *const libc::c_void, 1) };
    }
}

fn install_signals() -> io::Result<OwnedFd> {
    let mut p = [0 as RawFd; 2];
    // SAFETY: pipe fills two fds.
    cvt(unsafe { libc::pipe(p.as_mut_ptr()) })?;
    for fd in p {
        set_cloexec(fd, true)?;
        set_nonblocking(fd, true)?;
    }
    SIG_W.store(p[1], std::sync::atomic::Ordering::Relaxed);
    // SAFETY: installing handlers; on_signal only calls write(2).
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = on_signal as *const () as usize;
        sa.sa_flags = libc::SA_RESTART;
        libc::sigemptyset(&mut sa.sa_mask);
        for s in [libc::SIGCHLD, libc::SIGTERM, libc::SIGINT] {
            cvt(libc::sigaction(s, &sa, std::ptr::null_mut()))?;
        }
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
    }
    // SAFETY: p[0] is ours; p[1] stays open for the process lifetime.
    Ok(unsafe { OwnedFd::from_raw_fd(p[0]) })
}

// ---------------------------------------------------------------- setup

fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    match std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) => return Err(e),
    }
    let md = std::fs::symlink_metadata(dir)?;
    if !md.is_dir() || md.uid() != sys::euid() {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("{} is not our directory", dir.display())));
    }
    if md.mode() & 0o077 != 0 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn take_lock(path: &Path) -> io::Result<Result<std::fs::File, Option<i32>>> {
    use std::io::{Read, Seek};
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).mode(0o600).open(path)?;
    set_cloexec(f.as_raw_fd(), true)?;
    // SAFETY: flock on our own fd.
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let e = io::Error::last_os_error();
        if e.raw_os_error() == Some(libc::EWOULDBLOCK) {
            let mut s = String::new();
            let _ = f.read_to_string(&mut s);
            return Ok(Err(s.trim().parse().ok()));
        }
        return Err(e);
    }
    f.set_len(0)?;
    f.seek(io::SeekFrom::Start(0))?;
    write!(f, "{}", std::process::id())?;
    f.sync_all()?;
    Ok(Ok(f))
}

pub fn listen_unix(path: &Path, mode: u32, backlog: i32) -> io::Result<OwnedFd> {
    use std::os::unix::ffi::OsStrExt;
    let bytes = path.as_os_str().as_bytes();
    // SAFETY: plain socket creation.
    let fd = cvt(unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) })?;
    // SAFETY: fd was just created.
    let sock = unsafe { OwnedFd::from_raw_fd(fd) };
    set_cloexec(fd, true)?;
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if bytes.len() >= addr.sun_path.len() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "socket path too long"));
    }
    addr.sun_family = libc::AF_UNIX as _;
    for (i, b) in bytes.iter().enumerate() {
        addr.sun_path[i] = *b as libc::c_char;
    }
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let len = (std::mem::size_of::<libc::sa_family_t>() + bytes.len() + 1) as libc::socklen_t;
    // SAFETY: addr is a valid sockaddr_un of `len` bytes.
    cvt(unsafe { libc::bind(fd, &addr as *const _ as *const libc::sockaddr, len) })?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    }
    // SAFETY: listen on our bound socket.
    cvt(unsafe { libc::listen(fd, backlog) })?;
    Ok(sock)
}

/// S-3: not dumpable (Linux `PR_SET_DUMPABLE 0`: no `/proc/<pid>/fd`,
/// `mem` or core for other processes of the uid), no debugger attach
/// (macOS `PT_DENY_ATTACH`, release builds). `main` calls it first thing
/// (security review F5: `execve` resets dumpability, so a self-upgraded
/// image must harden before it reads the state fd, key and rings); the
/// server calls it again. Idempotent.
pub fn harden_process() {
    #[cfg(target_os = "linux")]
    // SAFETY: prctl with constant args.
    unsafe {
        libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0);
    }
    #[cfg(all(target_os = "macos", not(debug_assertions)))]
    // SAFETY: PT_DENY_ATTACH takes no address.
    unsafe {
        libc::ptrace(libc::PT_DENY_ATTACH, 0, std::ptr::null_mut(), 0);
    }
}

fn ready(fd: Option<RawFd>, status: u8) {
    if let Some(fd) = fd {
        // SAFETY: one byte to the launcher pipe, then close it.
        unsafe {
            libc::write(fd, &status as *const u8 as *const libc::c_void, 1);
            libc::close(fd);
        }
    }
}

/// Run ptyd until it exits. Blocks.
pub fn serve(cfg: Config) -> io::Result<Exit> {
    let mut log = Log::open(&cfg.paths.log);
    if let Some(run) = cfg.paths.dir.parent() {
        ensure_private_dir(run)?;
    }
    ensure_private_dir(&cfg.paths.dir)?;
    let lock = match take_lock(&cfg.paths.lock)? {
        Ok(f) => f,
        Err(pid) => {
            ready(cfg.ready_fd, b'B');
            return Ok(Exit::AlreadyRunning(pid));
        }
    };
    if cfg.paths.uses_fallback() {
        if let Some(d) = cfg.paths.sock.parent() {
            ensure_private_dir(d)?;
        }
        crate::fsutil::write_private(&cfg.paths.endpoint_file, cfg.paths.sock.to_string_lossy().as_bytes())?;
    } else {
        let _ = std::fs::remove_file(&cfg.paths.endpoint_file);
    }
    let key = auth::create_key(&cfg.paths.key)?;
    let listener = listen_unix(&cfg.paths.sock, 0o600, 64)?;
    set_nonblocking(listener.as_raw_fd(), true)?;
    let sig_r = install_signals()?;
    harden_process();
    log.line(&format!("start v{} proto {}..{} sock_fallback={}", PTYD_VERSION, PROTO_MIN, PROTO_MAX, cfg.paths.uses_fallback()));
    let boot_id = auth::hex(&auth::random_bytes::<16>()?);
    let ready_fd = cfg.ready_fd;
    let mut s = Server {
        cfg,
        log,
        key,
        boot_id,
        listener,
        sig_r,
        conns: HashMap::new(),
        next_conn: 1,
        sessions: HashMap::new(),
        owner: None,
        owner_since: None,
        ownerless_since: Some(Instant::now()),
        idle_since: Some(Instant::now()),
        held: HashMap::new(),
        shutting_down: None,
        exit_now: false,
        orphan_warned: false,
        _lock: lock,
    };
    ready(ready_fd, b'R');
    let r = s.run();
    s.cleanup();
    r.map(|()| Exit::Clean)
}

// ---------------------------------------------------------------- loop

#[derive(Clone, Copy)]
enum Slot {
    Listener,
    Signals,
    Conn(u64),
    Master(usize),
}

impl Server {
    fn run(&mut self) -> io::Result<()> {
        let mut fds: Vec<libc::pollfd> = Vec::new();
        let mut slots: Vec<Slot> = Vec::new();
        let mut sids: Vec<String> = Vec::new();
        while !self.exit_now {
            fds.clear();
            slots.clear();
            sids.clear();
            let add = |fds: &mut Vec<libc::pollfd>, slots: &mut Vec<Slot>, fd: RawFd, ev: i16, slot: Slot| {
                fds.push(libc::pollfd { fd, events: ev, revents: 0 });
                slots.push(slot);
            };
            add(&mut fds, &mut slots, self.listener.as_raw_fd(), libc::POLLIN, Slot::Listener);
            add(&mut fds, &mut slots, self.sig_r.as_raw_fd(), libc::POLLIN, Slot::Signals);
            for (id, c) in &self.conns {
                let mut ev = libc::POLLIN;
                if !c.wbuf.is_empty() || self.stream_pending(*id) {
                    ev |= libc::POLLOUT;
                }
                add(&mut fds, &mut slots, c.sock.as_raw_fd(), ev, Slot::Conn(*id));
            }
            for (sid, sess) in &self.sessions {
                if let Some(m) = &sess.master {
                    let mut ev = libc::POLLIN;
                    if !sess.input.is_empty() {
                        ev |= libc::POLLOUT;
                    }
                    let idx = sids.len();
                    sids.push(sid.clone());
                    add(&mut fds, &mut slots, m.as_raw_fd(), ev, Slot::Master(idx));
                }
            }
            let timeout = self.next_timeout_ms();
            // SAFETY: fds is a valid array of pollfd.
            let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout) };
            if n < 0 {
                let e = io::Error::last_os_error();
                if e.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(e);
            }
            for i in 0..fds.len() {
                let re = fds[i].revents;
                if re == 0 {
                    continue;
                }
                match slots[i] {
                    Slot::Listener => self.accept_all(),
                    Slot::Signals => self.on_signals(),
                    Slot::Conn(id) => {
                        if re & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
                            self.conn_readable(id);
                        }
                        if re & libc::POLLOUT != 0 {
                            self.conn_writable(id);
                        }
                    }
                    Slot::Master(idx) => {
                        let sid = sids[idx].clone();
                        if re & libc::POLLOUT != 0 {
                            self.master_writable(&sid);
                        }
                        if re & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
                            self.master_readable(&sid);
                        }
                    }
                }
            }
            self.flush_streams();
            self.timers();
            self.reap_closed_conns();
        }
        Ok(())
    }

    fn next_timeout_ms(&self) -> i32 {
        let now = Instant::now();
        let mut next = now + Duration::from_secs(1);
        for s in self.sessions.values() {
            if let Some(d) = s.kill_deadline {
                next = next.min(d);
            }
        }
        if let Some((_, _, d)) = self.shutting_down {
            next = next.min(d);
        }
        next.saturating_duration_since(now).as_millis().min(1000) as i32
    }

    fn cleanup(&mut self) {
        for (_, h) in self.held.drain() {
            if let Some(p) = h.unlink {
                let _ = std::fs::remove_file(p);
            }
        }
        let _ = std::fs::remove_file(&self.cfg.paths.sock);
        if self.cfg.paths.uses_fallback() {
            let _ = std::fs::remove_file(&self.cfg.paths.endpoint_file);
        }
        let _ = std::fs::remove_file(&self.cfg.paths.key);
        self.log.line("exit");
    }

    // ------------------------------------------------------------ accept

    fn accept_all(&mut self) {
        loop {
            // SAFETY: accept on our non-blocking listener.
            let fd = unsafe { libc::accept(self.listener.as_raw_fd(), std::ptr::null_mut(), std::ptr::null_mut()) };
            if fd < 0 {
                return;
            }
            // SAFETY: fd returned by accept.
            let sock = unsafe { OwnedFd::from_raw_fd(fd) };
            if set_cloexec(fd, true).is_err() || set_nonblocking(fd, true).is_err() {
                continue;
            }
            // P10: uid check before a single byte is sent.
            match sys::peer_uid(fd) {
                Ok(uid) if uid == sys::euid() => {}
                Ok(uid) => {
                    self.log.line(&format!("refused peer uid {uid} (closed silently)"));
                    continue;
                }
                Err(e) => {
                    self.log.line(&format!("peer cred failed: {}", e.kind()));
                    continue;
                }
            }
            let nonce = match auth::random_bytes::<32>() {
                Ok(n) => n,
                Err(_) => continue,
            };
            let id = self.next_conn;
            self.next_conn += 1;
            let mut c = Conn::new(sock, Kind::Hello { nonce });
            c.queue(
                None,
                Reply::Challenge {
                    nonce: auth::hex(&nonce),
                    ptyd_version: reported_version(),
                    proto_min: PROTO_MIN,
                    proto_max: PROTO_MAX,
                    boot_id: self.boot_id.clone(),
                },
            );
            self.conns.insert(id, c);
        }
    }

    // ------------------------------------------------------------ conn io

    fn stream_pending(&self, id: u64) -> bool {
        let Some(c) = self.conns.get(&id) else { return false };
        let Kind::Data { sid: Some(sid) } = &c.kind else { return false };
        self.sessions.get(sid).map(|s| s.stream == Some(id) && s.sent < s.ring.end()).unwrap_or(false)
    }

    fn conn_readable(&mut self, id: u64) {
        let mut buf = [0u8; READ_CHUNK];
        loop {
            let Some(c) = self.conns.get_mut(&id) else { return };
            match sys::recv_with_fds(c.sock.as_raw_fd(), &mut buf) {
                Ok((0, _)) => {
                    self.drop_conn(id);
                    return;
                }
                Ok((n, fds)) => {
                    c.rfds.extend(fds);
                    let attached = match &c.kind {
                        Kind::Data { sid: Some(sid) } => Some(sid.clone()),
                        _ => None,
                    };
                    match attached {
                        Some(sid) => self.input_to(&sid, id, &buf[..n]),
                        None => {
                            c.dec.push(&buf[..n]);
                            self.process_frames(id);
                        }
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    self.drop_conn(id);
                    return;
                }
            }
        }
    }

    fn input_to(&mut self, sid: &str, conn: u64, bytes: &[u8]) {
        if let Some(s) = self.sessions.get_mut(sid) {
            if s.stream == Some(conn) && s.running() && s.master.is_some() {
                s.input.extend(bytes);
            }
        }
        self.master_writable(sid);
    }

    fn conn_writable(&mut self, id: u64) {
        let Some(c) = self.conns.get_mut(&id) else { return };
        while !c.wbuf.is_empty() {
            let (a, _) = c.wbuf.as_slices();
            let fd_here = c.wfds.front().map(|(at, _)| *at == c.written).unwrap_or(false);
            // An fd rides on the first byte of its frame; send only up to the
            // next fd boundary otherwise.
            let limit = match c.wfds.front() {
                Some((at, _)) if !fd_here => ((*at - c.written) as usize).min(a.len()),
                _ => a.len(),
            };
            let pass = if fd_here { c.wfds.front().map(|(_, f)| f.as_raw_fd()) } else { None };
            match sys::send_with_fd(c.sock.as_raw_fd(), &a[..limit], pass) {
                Ok(n) => {
                    if fd_here {
                        c.wfds.pop_front();
                    }
                    c.wbuf.drain(..n);
                    c.written += n as u64;
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    c.closing = true;
                    c.wbuf.clear();
                    return;
                }
            }
        }
        if c.wbuf.len() > CTRL_WBUF_MAX {
            c.closing = true;
        }
        if matches!(c.kind, Kind::Data { sid: Some(_) }) {
            self.flush_stream(id);
        }
    }

    /// Write ring bytes to an attached data connection.
    fn flush_stream(&mut self, id: u64) {
        let Some(c) = self.conns.get_mut(&id) else { return };
        if !c.wbuf.is_empty() {
            return;
        }
        let Kind::Data { sid: Some(sid) } = &c.kind else { return };
        let Some(s) = self.sessions.get_mut(sid) else { return };
        if s.stream != Some(id) {
            return;
        }
        loop {
            let (a, b) = s.ring.slices_from(s.sent);
            let chunk = if !a.is_empty() { a } else { b };
            if chunk.is_empty() {
                break;
            }
            match sys::send_with_fd(c.sock.as_raw_fd(), chunk, None) {
                Ok(n) => s.sent += n as u64,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    c.closing = true;
                    return;
                }
            }
        }
        // Exited and fully drained: EOF tells the daemon's backend.
        if !s.running() && s.sent >= s.ring.end() {
            c.closing = true;
        }
    }

    fn flush_streams(&mut self) {
        let ids: Vec<u64> = self
            .conns
            .iter()
            .filter(|(_, c)| matches!(c.kind, Kind::Data { sid: Some(_) }))
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.conn_writable(id);
        }
    }

    fn reap_closed_conns(&mut self) {
        let dead: Vec<u64> = self.conns.iter().filter(|(_, c)| c.closing && c.wbuf.is_empty()).map(|(id, _)| *id).collect();
        for id in dead {
            self.drop_conn(id);
        }
    }

    fn drop_conn(&mut self, id: u64) {
        let Some(c) = self.conns.remove(&id) else { return };
        if let Kind::Data { sid: Some(sid) } = &c.kind {
            if let Some(s) = self.sessions.get_mut(sid) {
                if s.stream == Some(id) {
                    s.stream = None;
                }
            }
        }
        if self.owner == Some(id) {
            self.owner = None;
            self.owner_since = None;
            self.ownerless_since = Some(Instant::now());
            self.orphan_warned = false;
            self.log.line("owner disconnected");
        }
    }

    // ------------------------------------------------------------ frames

    fn process_frames(&mut self, id: u64) {
        loop {
            let Some(c) = self.conns.get_mut(&id) else { return };
            if matches!(c.kind, Kind::Data { sid: Some(_) }) {
                // Bytes after `attach` in the same read are raw input.
                let rest = c.dec.take_rest();
                if let Kind::Data { sid: Some(sid) } = &c.kind {
                    let sid = sid.clone();
                    if !rest.is_empty() {
                        self.input_to(&sid, id, &rest);
                    }
                }
                return;
            }
            let frame = match c.dec.next_frame() {
                Ok(Some(f)) => f,
                Ok(None) => return,
                Err(proto::FrameError::TooLarge) => {
                    self.log.line("frame_too_large: closing connection");
                    self.drop_conn(id);
                    return;
                }
            };
            self.handle_frame(id, &frame);
        }
    }

    fn handle_frame(&mut self, id: u64, raw: &[u8]) {
        let parsed: Result<Frame<Request>, _> = serde_json::from_slice(raw);
        let frame = match parsed {
            Ok(f) => f,
            Err(_) => {
                // Echo the id if the envelope at least had one.
                let rid = serde_json::from_slice::<serde_json::Value>(raw).ok().and_then(|v| v.get("id").and_then(|i| i.as_u64()));
                let known = serde_json::from_slice::<serde_json::Value>(raw)
                    .ok()
                    .and_then(|v| v.get("t").and_then(|t| t.as_str()).map(str::to_string));
                let code = match known.as_deref() {
                    Some(t) if !KNOWN_T.contains(&t) => "unknown_message",
                    _ => "bad_request",
                };
                let hello = matches!(self.conns.get(&id).map(|c| &c.kind), Some(Kind::Hello { .. }));
                if let Some(c) = self.conns.get_mut(&id) {
                    if hello {
                        c.queue(None, Reply::Refused { code: "proto".into(), detail: "expected hello".into() });
                        c.closing = true;
                    } else {
                        c.queue(rid, Reply::Error { code: code.into(), detail: String::new() });
                    }
                }
                return;
            }
        };
        let Some(c) = self.conns.get(&id) else { return };
        match (&c.kind, frame.body) {
            (Kind::Hello { nonce }, Request::Hello { proto, client: _, client_version, mac, takeover, role }) => {
                let nonce = *nonce;
                self.on_hello(id, nonce, proto, &client_version, &mac, takeover, role);
            }
            (Kind::Hello { .. }, _) => {
                let c = self.conns.get_mut(&id).unwrap();
                c.queue(None, Reply::Refused { code: "proto".into(), detail: "expected hello".into() });
                c.closing = true;
            }
            (Kind::Data { sid: None }, Request::Attach { sid, from }) => self.on_attach(id, frame.id, sid, from),
            (Kind::Data { .. }, _) => self.reply(id, frame.id, err("data_connection", "only attach is allowed")),
            (Kind::Owner, Request::Hello { .. }) | (Kind::Owner, Request::Attach { .. }) => {
                self.reply(id, frame.id, err("bad_request", "not on an owner connection"))
            }
            (Kind::Owner, body) => self.on_request(id, frame.id, body),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn on_hello(&mut self, id: u64, nonce: [u8; 32], proto: u32, client_version: &str, mac: &str, takeover: bool, role: Role) {
        let refuse = |s: &mut Server, code: &str, detail: &str| {
            s.log.line(&format!("refused hello: {code}"));
            if let Some(c) = s.conns.get_mut(&id) {
                c.queue(None, Reply::Refused { code: code.into(), detail: detail.into() });
                c.closing = true;
            }
        };
        if !auth::verify(&self.key, &nonce, client_version, mac) {
            return refuse(self, "bad_mac", "");
        }
        if !(PROTO_MIN..=PROTO_MAX).contains(&proto) {
            return refuse(self, "proto", &format!("ptyd speaks {PROTO_MIN}..{PROTO_MAX}"));
        }
        match role {
            Role::Owner => {
                if let Some(old) = self.owner {
                    if !takeover {
                        return refuse(self, "busy", "an owner is connected");
                    }
                    if let Some(oc) = self.conns.get_mut(&old) {
                        oc.queue(None, Reply::Evicted);
                        oc.closing = true;
                    }
                    self.log.line("owner evicted by takeover");
                }
                let warn = self.orphan_seconds_left_if_past_half();
                self.owner = Some(id);
                self.owner_since = Some(now_secs());
                self.ownerless_since = None;
                self.orphan_warned = false;
                let n = self.sessions.len();
                let c = self.conns.get_mut(&id).unwrap();
                c.kind = Kind::Owner;
                c.queue(None, Reply::Welcome { proto, sessions: n });
                if let Some(left) = warn {
                    c.queue(None, Reply::OrphanWarning { seconds_left: left });
                }
                self.log.line(&format!("owner attached (client v{client_version})"));
            }
            Role::Data => {
                let n = self.sessions.len();
                let c = self.conns.get_mut(&id).unwrap();
                c.kind = Kind::Data { sid: None };
                c.queue(None, Reply::Welcome { proto, sessions: n });
            }
        }
    }

    fn orphan_seconds_left_if_past_half(&self) -> Option<u64> {
        let since = self.ownerless_since?;
        if !self.sessions.values().any(|s| s.running()) {
            return None;
        }
        let gone = since.elapsed();
        if gone * 2 >= self.cfg.orphan {
            Some(self.cfg.orphan.saturating_sub(gone).as_secs())
        } else {
            None
        }
    }

    fn on_attach(&mut self, id: u64, rid: Option<u64>, sid: String, from: u64) {
        let Some(s) = self.sessions.get_mut(&sid) else {
            return self.reply(id, rid, err("unknown_session", ""));
        };
        let old = s.stream.replace(id);
        let start = from.max(s.ring.start()).min(s.ring.end());
        let end = s.ring.end();
        s.sent = start;
        if let Some(old) = old {
            if old != id {
                if let Some(oc) = self.conns.get_mut(&old) {
                    oc.closing = true;
                    oc.wbuf.clear();
                    oc.kind = Kind::Data { sid: None };
                }
            }
        }
        let c = self.conns.get_mut(&id).unwrap();
        c.kind = Kind::Data { sid: Some(sid.clone()) };
        c.queue(rid, Reply::Attached { start, end });
        // Anything already buffered behind the attach frame is input.
        self.process_frames(id);
    }

    fn reply(&mut self, id: u64, rid: Option<u64>, body: Reply) {
        if let Some(c) = self.conns.get_mut(&id) {
            c.queue(rid, body);
        }
    }

    fn event(&mut self, body: Reply) {
        if let Some(o) = self.owner {
            if let Some(c) = self.conns.get_mut(&o) {
                c.queue(None, body);
            }
        }
    }

    // ------------------------------------------------------------ requests

    fn on_request(&mut self, id: u64, rid: Option<u64>, body: Request) {
        match body {
            Request::Spawn { sid, argv, env, cwd, cols, rows, ring_bytes, meta } => {
                let r = self.do_spawn(sid, argv, env, cwd, cols, rows, ring_bytes, meta);
                self.reply(id, rid, r);
            }
            Request::List => {
                let mut v: Vec<SessionInfo> = self.sessions.iter().map(|(k, s)| s.info(k)).collect();
                v.sort_by(|a, b| a.sid.cmp(&b.sid));
                self.reply(id, rid, Reply::Sessions { sessions: v });
            }
            Request::UpdateMeta { sid, meta } => {
                let r = if meta_too_big(&meta) {
                    err("meta_too_large", "")
                } else {
                    match self.sessions.get_mut(&sid) {
                        Some(s) => {
                            s.meta = meta;
                            Reply::Ok
                        }
                        None => err("unknown_session", ""),
                    }
                };
                self.reply(id, rid, r);
            }
            Request::Resize { sid, cols, rows } => {
                let r = match self.sessions.get_mut(&sid) {
                    Some(s) => match &s.master {
                        Some(m) => {
                            let ws = libc::winsize { ws_row: rows.max(1), ws_col: cols.max(1), ws_xpixel: 0, ws_ypixel: 0 };
                            // SAFETY: TIOCSWINSZ on our master.
                            if unsafe { libc::ioctl(m.as_raw_fd(), libc::TIOCSWINSZ as _, &ws) } == 0 {
                                s.cols = cols.max(1);
                                s.rows = rows.max(1);
                                Reply::Ok
                            } else {
                                err("resize_failed", &io::Error::last_os_error().kind().to_string())
                            }
                        }
                        None => err("exited", ""),
                    },
                    None => err("unknown_session", ""),
                };
                self.reply(id, rid, r);
            }
            Request::Signal { sid, sig, target } => {
                let r = match self.sessions.get(&sid) {
                    Some(s) if s.running() => signal_session(s, sig, target),
                    Some(_) => err("exited", ""),
                    None => err("unknown_session", ""),
                };
                self.reply(id, rid, r);
            }
            Request::FgPgrp { sid } => {
                let r = match self.sessions.get(&sid).and_then(|s| s.master.as_ref()) {
                    Some(m) => {
                        // SAFETY: tcgetpgrp on our master.
                        let pg = unsafe { libc::tcgetpgrp(m.as_raw_fd()) };
                        Reply::Pgrp { pgid: if pg > 0 { Some(pg) } else { None } }
                    }
                    None if self.sessions.contains_key(&sid) => Reply::Pgrp { pgid: None },
                    None => err("unknown_session", ""),
                };
                self.reply(id, rid, r);
            }
            Request::Kill { sid, grace_ms } => {
                let grace = Duration::from_millis(grace_ms.unwrap_or(KILL_GRACE_DEFAULT));
                match self.sessions.get_mut(&sid) {
                    None => self.reply(id, rid, err("unknown_session", "")),
                    Some(s) if !s.running() => self.reply(id, rid, Reply::Ok),
                    Some(s) => {
                        group_signal(s.pid, libc::SIGHUP);
                        if s.kill_deadline.is_none() {
                            s.kill_deadline = Some(Instant::now() + grace);
                        }
                        s.kill_waiters.push((id, rid));
                        // A child that died instantly is reaped on SIGCHLD.
                    }
                }
            }
            Request::AckExit { sid } => {
                let r = match self.sessions.get(&sid) {
                    Some(s) if s.running() => err("running", ""),
                    Some(_) => {
                        self.sessions.remove(&sid);
                        Reply::Ok
                    }
                    None => err("unknown_session", ""),
                };
                self.reply(id, rid, r);
            }
            Request::HoldUnixListener { name, path, mode, owner_uid } => self.do_hold_listener(id, rid, name, path, mode, owner_uid),
            Request::HoldFd { name } => {
                let fd = self.conns.get_mut(&id).and_then(|c| c.rfds.pop_front());
                let r = match fd {
                    Some(fd) => {
                        if let Some(old) = self.held.insert(name, Held { fd, unlink: None }) {
                            drop(old);
                        }
                        Reply::Ok
                    }
                    None => err("no_fd", "hold_fd needs an SCM_RIGHTS fd"),
                };
                self.reply(id, rid, r);
            }
            Request::TakeFd { name } => match self.held.get(&name).map(|h| dup_fd(h.fd.as_raw_fd())) {
                Some(Ok(fd)) => {
                    if let Some(c) = self.conns.get_mut(&id) {
                        c.queue_with_fd(rid, Reply::Ok, fd);
                    }
                }
                Some(Err(e)) => self.reply(id, rid, err("dup_failed", &e.kind().to_string())),
                None => self.reply(id, rid, err("unknown", "")),
            },
            Request::ReleaseFd { name } => {
                if let Some(h) = self.held.remove(&name) {
                    if let Some(p) = h.unlink {
                        let _ = std::fs::remove_file(p);
                    }
                }
                self.reply(id, rid, Reply::Ok);
            }
            Request::Upgrade { exe, sha256 } => self.do_upgrade(id, rid, exe, sha256),
            Request::Shutdown { end_sessions } => {
                let running = self.sessions.values().filter(|s| s.running()).count();
                if !end_sessions && running > 0 {
                    return self.reply(id, rid, err("sessions_running", &running.to_string()));
                }
                self.log.line(&format!("shutdown requested end_sessions={end_sessions} running={running}"));
                self.begin_shutdown(Some((id, rid)));
            }
            Request::Ping => {
                let n = self.sessions.len();
                let since = self.owner_since;
                #[cfg(target_os = "linux")]
                // SAFETY: prctl query with constant args.
                let dumpable = Some(unsafe { libc::prctl(libc::PR_GET_DUMPABLE, 0, 0, 0, 0) } != 0);
                #[cfg(not(target_os = "linux"))]
                let dumpable = None;
                let mut env_keys: Vec<String> = std::env::vars_os().map(|(k, _)| k.to_string_lossy().into_owned()).collect();
                env_keys.sort();
                self.reply(id, rid, Reply::Pong { sessions: n, owner_since: since, dumpable, env_keys });
            }
            Request::Hello { .. } | Request::Attach { .. } => unreachable!("filtered by handle_frame"),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn do_spawn(
        &mut self,
        sid: String,
        argv: Vec<String>,
        env: Vec<(String, String)>,
        cwd: Option<String>,
        cols: u16,
        rows: u16,
        ring_bytes: Option<u64>,
        meta: serde_json::Value,
    ) -> Reply {
        if self.shutting_down.is_some() {
            return err("shutting_down", "");
        }
        if sid.is_empty() || sid.len() > 256 {
            return err("bad_sid", "");
        }
        if self.sessions.contains_key(&sid) {
            return err("dup_sid", "");
        }
        if meta_too_big(&meta) {
            return err("meta_too_large", "");
        }
        let spec = SpawnSpec { argv: &argv, env: &env, cwd: cwd.as_deref(), cols, rows };
        match spawn(&spec) {
            Ok(child) => {
                self.log.line(&format!("spawn sid={sid} pid={}", child.pid));
                self.sessions.insert(
                    sid.clone(),
                    Session {
                        pid: child.pid,
                        master: Some(child.master),
                        ring: Ring::new(clamp_ring(ring_bytes)),
                        state: SState::Running,
                        cols: cols.max(1),
                        rows: rows.max(1),
                        started_at: now_secs(),
                        meta,
                        input: VecDeque::new(),
                        stream: None,
                        sent: 0,
                        kill_deadline: None,
                        kill_waiters: Vec::new(),
                    },
                );
                self.idle_since = None;
                Reply::Spawned { sid, pid: child.pid }
            }
            Err(e) => {
                self.log.line(&format!("spawn failed sid={sid}: {}", e.kind()));
                let code = if e.kind() == io::ErrorKind::NotFound { "enoent" } else { "spawn_failed" };
                err(code, &e.to_string())
            }
        }
    }

    fn do_hold_listener(&mut self, id: u64, rid: Option<u64>, name: String, path: String, mode: u32, owner_uid: Option<u32>) {
        // Idempotent: an existing listener is handed out again, never rebound
        // (adoption relies on the same socket surviving, P44).
        if let Some(h) = self.held.get(&name) {
            match dup_fd(h.fd.as_raw_fd()) {
                Ok(fd) => {
                    if let Some(c) = self.conns.get_mut(&id) {
                        c.queue_with_fd(rid, Reply::Ok, fd);
                    }
                }
                Err(e) => self.reply(id, rid, err("dup_failed", &e.kind().to_string())),
            }
            return;
        }
        let p = PathBuf::from(&path);
        let r = listen_unix(&p, mode & 0o777, 64).and_then(|fd| {
            if let Some(uid) = owner_uid {
                if uid != sys::euid() {
                    use std::os::unix::ffi::OsStrExt;
                    let c = std::ffi::CString::new(p.as_os_str().as_bytes()).map_err(io::Error::other)?;
                    // SAFETY: chown on a path we just bound.
                    cvt(unsafe { libc::chown(c.as_ptr(), uid, u32::MAX) })?;
                }
            }
            let dup = dup_fd(fd.as_raw_fd())?;
            Ok((fd, dup))
        });
        match r {
            Ok((fd, dup)) => {
                self.held.insert(name, Held { fd, unlink: Some(p) });
                if let Some(c) = self.conns.get_mut(&id) {
                    c.queue_with_fd(rid, Reply::Ok, dup);
                }
            }
            Err(e) => self.reply(id, rid, err("listen_failed", &e.kind().to_string())),
        }
    }

    fn begin_shutdown(&mut self, waiter: Option<(u64, Option<u64>)>) {
        let mut any = false;
        for s in self.sessions.values_mut() {
            if s.running() {
                group_signal(s.pid, libc::SIGHUP);
                any = true;
            }
        }
        let (cid, rid) = waiter.unwrap_or((0, None));
        if !any {
            self.reply(cid, rid, Reply::Ok);
            self.finish_shutdown();
            return;
        }
        self.shutting_down = Some((cid, rid, Instant::now() + Duration::from_millis(SHUTDOWN_GRACE_MS)));
    }

    fn finish_shutdown(&mut self) {
        // Flush pending replies best-effort (non-blocking: a client that
        // stopped reading must not keep ptyd alive), then exit the loop.
        let ids: Vec<u64> = self.conns.keys().copied().collect();
        for id in ids {
            self.conn_writable(id);
        }
        self.exit_now = true;
    }

    // ------------------------------------------------------------ masters

    fn master_readable(&mut self, sid: &str) {
        let mut buf = [0u8; READ_CHUNK];
        let mut total = 0usize;
        let mut eof = false;
        {
            let Some(s) = self.sessions.get_mut(sid) else { return };
            let Some(m) = &s.master else { return };
            let fd = m.as_raw_fd();
            while total < MAX_READ_PER_WAKE {
                // SAFETY: read into a local buffer from our master.
                let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
                if n > 0 {
                    s.ring.push(&buf[..n as usize]);
                    total += n as usize;
                    continue;
                }
                if n == 0 {
                    eof = true;
                    break;
                }
                let e = io::Error::last_os_error();
                match e.kind() {
                    io::ErrorKind::WouldBlock => break,
                    io::ErrorKind::Interrupted => continue,
                    _ => {
                        // EIO: the slave side is gone (Linux).
                        eof = true;
                        break;
                    }
                }
            }
        }
        if total > 0 {
            self.enforce_total_cap();
            self.check_overrun(sid);
            if let Some(stream) = self.sessions.get(sid).and_then(|s| s.stream) {
                self.flush_stream(stream);
            }
        }
        if eof {
            // The child is reaped on SIGCHLD; stop polling a hung-up master
            // (POLLHUP is level-triggered and would spin).
            if let Some(s) = self.sessions.get_mut(sid) {
                if s.running() {
                    self.try_reap(sid);
                }
            }
            if let Some(s) = self.sessions.get_mut(sid) {
                if s.running() {
                    // Grandchildren may still hold the slave; the leader is
                    // alive. Keep the record running but stop reading.
                    s.master = None;
                }
            }
        }
    }

    fn master_writable(&mut self, sid: &str) {
        let Some(s) = self.sessions.get_mut(sid) else { return };
        let Some(m) = &s.master else {
            s.input.clear();
            return;
        };
        while !s.input.is_empty() {
            let (a, _) = s.input.as_slices();
            // SAFETY: write from our buffer to our master.
            let n = unsafe { libc::write(m.as_raw_fd(), a.as_ptr() as *const libc::c_void, a.len()) };
            if n > 0 {
                s.input.drain(..n as usize);
                continue;
            }
            let e = io::Error::last_os_error();
            match e.kind() {
                io::ErrorKind::Interrupted => continue,
                _ => return,
            }
        }
    }

    fn enforce_total_cap(&mut self) {
        let mut total: usize = self.sessions.values().map(|s| s.ring.len()).sum();
        while total > TOTAL_CAP {
            let Some((_, big)) = self.sessions.iter_mut().max_by_key(|(_, s)| s.ring.len()) else { return };
            let cut = (total - TOTAL_CAP).min(big.ring.len());
            if cut == 0 {
                return;
            }
            big.ring.trim_front(cut);
            total -= cut;
        }
        let sids: Vec<String> = self.sessions.keys().cloned().collect();
        for sid in sids {
            self.check_overrun(&sid);
        }
    }

    /// P23: a wrap past an attached stream's unsent cursor closes the stream.
    fn check_overrun(&mut self, sid: &str) {
        let Some(s) = self.sessions.get_mut(sid) else { return };
        let Some(stream) = s.stream else { return };
        if s.sent >= s.ring.start() {
            return;
        }
        let lost = s.ring.start() - s.sent;
        s.stream = None;
        if let Some(c) = self.conns.get_mut(&stream) {
            c.closing = true;
            c.wbuf.clear();
            c.kind = Kind::Data { sid: None };
        }
        self.log.line(&format!("overrun sid={sid} lost={lost}"));
        self.event(Reply::Overrun { sid: sid.to_string(), lost_bytes: lost });
    }

    // ------------------------------------------------------------ children

    fn on_signals(&mut self) {
        let mut buf = [0u8; 64];
        let mut term = false;
        loop {
            // SAFETY: draining our non-blocking pipe.
            let n = unsafe { libc::read(self.sig_r.as_raw_fd(), buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
            if n <= 0 {
                break;
            }
            for b in &buf[..n as usize] {
                if *b as libc::c_int == libc::SIGTERM || *b as libc::c_int == libc::SIGINT {
                    term = true;
                }
            }
        }
        let sids: Vec<String> = self.sessions.iter().filter(|(_, s)| s.running()).map(|(k, _)| k.clone()).collect();
        for sid in sids {
            self.try_reap(&sid);
        }
        if term && self.shutting_down.is_none() {
            // Someone deliberately signalled ptyd: an explicit stop. End
            // the agents with grace rather than orphaning them.
            self.log.line("SIGTERM: ending sessions and exiting");
            self.begin_shutdown(None);
        }
    }

    fn try_reap(&mut self, sid: &str) {
        let Some(s) = self.sessions.get(sid) else { return };
        if !s.running() {
            return;
        }
        let mut st = 0;
        // SAFETY: waitpid on our own child.
        let r = unsafe { libc::waitpid(s.pid, &mut st, libc::WNOHANG) };
        if r != s.pid {
            return;
        }
        let (code, signal) = if libc::WIFEXITED(st) {
            (Some(libc::WEXITSTATUS(st)), None)
        } else if libc::WIFSIGNALED(st) {
            (None, Some(libc::WTERMSIG(st)))
        } else {
            (None, None)
        };
        // Drain whatever the child left in the PTY, then hang up the slave
        // for any lingering grandchildren (same as alacritty dropping Pty).
        self.drain_master(sid);
        let at = now_secs();
        let Some(s) = self.sessions.get_mut(sid) else { return };
        s.master = None;
        s.input.clear();
        s.kill_deadline = None;
        s.state = SState::Exited { code, signal, at, when: Instant::now() };
        let waiters = std::mem::take(&mut s.kill_waiters);
        let stream = s.stream;
        self.log.line(&format!("exited sid={sid} code={code:?} signal={signal:?}"));
        for (cid, rid) in waiters {
            self.reply(cid, rid, Reply::Ok);
        }
        self.event(Reply::Exited { sid: sid.to_string(), code, signal, at });
        if let Some(stream) = stream {
            self.flush_stream(stream);
        }
    }

    fn drain_master(&mut self, sid: &str) {
        let mut buf = [0u8; READ_CHUNK];
        let Some(s) = self.sessions.get_mut(sid) else { return };
        let Some(m) = &s.master else { return };
        let fd = m.as_raw_fd();
        for _ in 0..1024 {
            // SAFETY: non-blocking read from our master.
            let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
            if n > 0 {
                s.ring.push(&buf[..n as usize]);
            } else if n < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            } else {
                break;
            }
        }
        self.enforce_total_cap();
    }

    // ------------------------------------------------------------ timers

    fn timers(&mut self) {
        let now = Instant::now();
        // Kill escalation (P18).
        let due: Vec<String> = self
            .sessions
            .iter()
            .filter(|(_, s)| s.running() && s.kill_deadline.map(|d| d <= now).unwrap_or(false))
            .map(|(k, _)| k.clone())
            .collect();
        for sid in due {
            if let Some(s) = self.sessions.get_mut(&sid) {
                group_signal(s.pid, libc::SIGKILL);
                s.kill_deadline = None;
            }
            // SIGCHLD will follow; reap now if it already happened.
            self.try_reap(&sid);
        }
        // Shutdown escalation.
        if let Some((cid, rid, deadline)) = self.shutting_down {
            let running: Vec<String> = self.sessions.iter().filter(|(_, s)| s.running()).map(|(k, _)| k.clone()).collect();
            if running.is_empty() {
                self.shutting_down = None;
                self.reply(cid, rid, Reply::Ok);
                self.finish_shutdown();
                return;
            }
            if now >= deadline {
                for sid in &running {
                    if let Some(s) = self.sessions.get(sid) {
                        group_signal(s.pid, libc::SIGKILL);
                    }
                }
                // Give SIGKILL a moment, then reap synchronously.
                std::thread::sleep(Duration::from_millis(50));
                for sid in &running {
                    self.try_reap(sid);
                }
                self.shutting_down = Some((cid, rid, now + Duration::from_millis(200)));
            }
        }
        // Exited records with no owner for an hour (P25).
        if self.owner.is_none() {
            let ttl = self.cfg.exited_ttl;
            self.sessions.retain(|_, s| match s.state {
                SState::Exited { when, .. } => when.elapsed() < ttl,
                SState::Running => true,
            });
        }
        // Orphan timeout (§7.11).
        if self.owner.is_none() && self.shutting_down.is_none() {
            if let Some(since) = self.ownerless_since {
                let running = self.sessions.values().any(|s| s.running());
                if running && since.elapsed() >= self.cfg.orphan {
                    self.log.line("orphan timeout: ending sessions");
                    self.begin_shutdown(None);
                    return;
                }
                if running && !self.orphan_warned && since.elapsed() * 2 >= self.cfg.orphan {
                    self.orphan_warned = true;
                    self.log.line("orphan timeout half-way");
                }
            }
        }
        // Idle exit (P7).
        // Held fds do not keep ptyd alive: with no sessions they are stale,
        // and cleanup() unlinks their paths.
        let idle = self.sessions.is_empty() && self.owner.is_none() && self.shutting_down.is_none();
        if idle {
            let since = *self.idle_since.get_or_insert(now);
            if since.elapsed() >= self.cfg.idle {
                self.log.line("idle exit");
                self.exit_now = true;
            }
        } else {
            self.idle_since = None;
        }
    }
}

const KNOWN_T: &[&str] = &[
    "hello", "spawn", "list", "update_meta", "resize", "signal", "fg_pgrp", "kill", "ack_exit",
    "hold_unix_listener", "hold_fd", "take_fd", "release_fd", "upgrade", "shutdown", "ping", "attach",
];

/// The version the challenge reports. Debug builds honour
/// `K2_PTYD_TEST_VERSION` so tests can stand up an "older" ptyd (P65);
/// the knob is not on the env allowlist, so a self-upgrade drops it.
fn reported_version() -> String {
    if cfg!(debug_assertions) {
        if let Some(v) = std::env::var("K2_PTYD_TEST_VERSION").ok().filter(|v| !v.is_empty()) {
            return v;
        }
    }
    PTYD_VERSION.into()
}

fn err(code: &str, detail: &str) -> Reply {
    Reply::Error { code: code.into(), detail: detail.into() }
}

fn meta_too_big(meta: &serde_json::Value) -> bool {
    serde_json::to_vec(meta).map(|v| v.len() > MAX_META).unwrap_or(true)
}

fn dup_fd(fd: RawFd) -> io::Result<OwnedFd> {
    // SAFETY: F_DUPFD_CLOEXEC on a valid fd.
    let n = cvt(unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) })?;
    // SAFETY: n is a fresh fd.
    Ok(unsafe { OwnedFd::from_raw_fd(n) })
}

/// P17: `killpg` only when the pid leads its own group, else `kill`.
pub fn group_signal(pid: i32, sig: libc::c_int) {
    // SAFETY: signalling our own child / its group.
    unsafe {
        if libc::getpgid(pid) == pid {
            libc::killpg(pid, sig);
        } else {
            libc::kill(pid, sig);
        }
    }
}

fn signal_session(s: &Session, sig: Sig, target: SigTarget) -> Reply {
    let n = sig.number();
    match target {
        SigTarget::Group => {
            group_signal(s.pid, n);
            Reply::Ok
        }
        SigTarget::Leader => {
            // SAFETY: signalling our own child.
            unsafe { libc::kill(s.pid, n) };
            Reply::Ok
        }
        SigTarget::Foreground => match &s.master {
            Some(m) => {
                // SAFETY: tcgetpgrp on our master.
                let pg = unsafe { libc::tcgetpgrp(m.as_raw_fd()) };
                if pg > 0 {
                    // SAFETY: signalling the PTY's foreground group.
                    unsafe { libc::killpg(pg, n) };
                    Reply::Ok
                } else {
                    err("no_foreground", "")
                }
            }
            None => err("exited", ""),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fuzz finding (2026-10-20): any peer can make ptyd log a line per
    /// bad frame, so the log rotates at its cap and never grows past
    /// cap + one line, keeping exactly one older generation.
    #[test]
    fn log_rotates_at_its_cap_and_keeps_one_generation() {
        let dir = std::env::temp_dir().join(format!("k2-ptyd-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("logs").join("ptyd.log");
        let rotated = dir.join("logs").join("ptyd.log.1");
        let mut log = Log::open_capped(&path, 4096);
        for i in 0..2000 {
            log.line(&format!("frame_too_large: closing connection {i}"));
            let len = std::fs::metadata(&path).unwrap().len();
            assert!(len <= 4096 + 128, "log grew past its cap: {len} bytes at line {i}");
        }
        assert!(rotated.is_file(), "no rotated generation");
        assert!(std::fs::metadata(&rotated).unwrap().len() <= 4096 + 128);
        let entries = std::fs::read_dir(dir.join("logs")).unwrap().count();
        assert_eq!(entries, 2, "exactly ptyd.log and ptyd.log.1");
        let last = std::fs::read_to_string(&path).unwrap();
        assert!(last.contains("closing connection 1999"), "newest line is in the live log");
        // Reopening resumes the size count (a restart can't reset the cap).
        drop(log);
        let mut again = Log::open_capped(&path, 4096);
        for _ in 0..200 {
            again.line("x");
            assert!(std::fs::metadata(&path).unwrap().len() <= 4096 + 128);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
