//! The supervisor (review sections 2 and 3).
//!
//! The exec helper, already in the session's cgroup or scope and under its
//! Landlock view, forks. The child installs the filter (after
//! no-new-privileges and Landlock) with a new listener, hands the listener
//! over (the parent takes it with `pidfd_getfd`), closes every descriptor
//! above 2 and runs the program. The parent stays as the session's first
//! process: child subreaper, non-dumpable, every signal blocked and read
//! from a `signalfd` (no handlers), and answers the session's notifications
//! until the program exits, then ends what is left of the session and exits
//! with the program's status.
//!
//! Each `connect()`: the caller's own socket by a copy of its descriptor
//! (`pidfd_getfd`), its family from the socket (`SO_DOMAIN`), its address
//! copied out once (clamped to `sockaddr_storage`), the notification
//! re-checked, the rule applied, and `connect()` done here on that copy;
//! the result goes back (`EINPROGRESS` included). Blocking non-Unix
//! connects go to a small capped pool of threads. Every error refuses
//! (record mode: lets the call run). A panic aborts: the session ends.

use std::collections::{HashSet, VecDeque};
use std::ffi::CString;
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use super::filter::{self, Arch, EPERM, ENOSYS};
use super::sys::{self, Notif, NotifResp};
use super::{parse_unix, plain_absolute, shown, BlockSpec, Mode, Record, UnixName};

/// Blocking non-Unix connects in flight at once; past it a connect is
/// refused (`EAGAIN`).
pub const POOL_CAP: usize = 32;
/// After a hang-up the program gets this long to exit before it is killed.
pub const HUP_GRACE: Duration = Duration::from_secs(5);
/// How long the supervisor spends ending what is left of the session.
pub const END_GRACE: Duration = Duration::from_secs(2);

const AF_UNIX: i32 = libc::AF_UNIX;

// ── the Unix rule ────────────────────────────────────────────────────

/// The session's hook socket: its fixed path, owned by K2's user.
pub struct HookRule {
    pub path: Vec<u8>,
    pub uid: u32,
}

/// The session's temp folder: a handle held from spawn.
pub struct TmpRule {
    /// The folder's path as the session names it (no trailing `/`).
    pub prefix: Vec<u8>,
    pub dir: OwnedFd,
    pub dev: libc::dev_t,
}

#[derive(Default)]
pub struct Rules {
    pub hook: Option<HookRule>,
    pub tmp: Option<TmpRule>,
}

/// What the rule says about one path.
pub enum Check {
    /// Allowed: a path-only handle on the socket that was checked.
    Allowed(OwnedFd, &'static str),
    Refused(String),
}

impl Rules {
    /// Apply the rule to a pathname socket's path.
    pub fn check(&self, path: &[u8]) -> Check {
        if path.first() != Some(&b'/') {
            return Check::Refused("a relative name".into());
        }
        if let Some(h) = self.hook.as_ref().filter(|h| h.path == path) {
            return match open_hook(h) {
                Ok(fd) => Check::Allowed(fd, "the session's hook socket"),
                Err(why) => Check::Refused(format!("the session's hook socket path, but {why}")),
            };
        }
        if let Some(t) = &self.tmp {
            let n = t.prefix.len();
            if path.len() > n + 1 && path.starts_with(&t.prefix) && path[n] == b'/' {
                return match open_beneath(t, &path[n + 1..]) {
                    Ok(fd) => Check::Allowed(fd, "a socket in the session's temp folder"),
                    Err(why) => Check::Refused(format!("under the session's temp folder, but {why}")),
                };
            }
        }
        Check::Refused("neither the session's hook socket nor under its temp folder".into())
    }
}

fn cstr(b: &[u8]) -> Result<CString, String> {
    CString::new(b.to_vec()).map_err(|_| "a NUL in the name".to_string())
}

fn is_sock(st: &libc::stat) -> bool {
    st.st_mode & libc::S_IFMT == libc::S_IFSOCK
}

fn open_hook(h: &HookRule) -> Result<OwnedFd, String> {
    let c = cstr(&h.path)?;
    let fd = sys::openat2(libc::AT_FDCWD, &c, libc::O_PATH | libc::O_CLOEXEC, sys::RESOLVE_NO_SYMLINKS | sys::RESOLVE_NO_MAGICLINKS)
        .map_err(|e| match e.raw_os_error() {
            Some(libc::ELOOP) => "a link on the way".to_string(),
            _ => format!("it can't be opened ({e})"),
        })?;
    let st = sys::fstat(fd.as_raw_fd()).map_err(|e| format!("fstat: {e}"))?;
    if !is_sock(&st) {
        return Err("it isn't a socket".into());
    }
    if st.st_uid != h.uid {
        return Err(format!("it belongs to uid {}, not K2's user", st.st_uid));
    }
    Ok(fd)
}

fn open_beneath(t: &TmpRule, rel: &[u8]) -> Result<OwnedFd, String> {
    let c = cstr(rel)?;
    let fd = sys::openat2(t.dir.as_raw_fd(), &c, libc::O_PATH | libc::O_CLOEXEC, sys::RESOLVE_BENEATH | sys::RESOLVE_NO_MAGICLINKS)
        .map_err(|e| match e.raw_os_error() {
            Some(libc::EXDEV) => "it leads out of the folder".to_string(),
            _ => format!("it can't be opened beneath the folder ({e})"),
        })?;
    let st = sys::fstat(fd.as_raw_fd()).map_err(|e| format!("fstat: {e}"))?;
    if !is_sock(&st) {
        return Err("it isn't a socket".into());
    }
    if st.st_dev != t.dev {
        return Err("it is on another device".into());
    }
    if st.st_nlink != 1 {
        return Err(format!("it has {} names (a hard link)", st.st_nlink));
    }
    Ok(fd)
}

/// The address that connects to the object behind `fd` (a path-only
/// handle), not to a name.
pub fn via_fd(fd: &OwnedFd) -> Vec<u8> {
    sys::unix_addr(format!("/proc/self/fd/{}", fd.as_raw_fd()).as_bytes())
}

// ── the log ──────────────────────────────────────────────────────────

/// The decision log: an append-only file opened before the fence.
pub struct Log {
    inner: Mutex<(Option<std::fs::File>, u64)>,
}

impl Log {
    pub fn none() -> Log {
        Log { inner: Mutex::new((None, 0)) }
    }

    /// Open (or make, 0600) the log; one past [`super::LOG_ROTATE_AT`] is
    /// moved to `<log>.1` first.
    pub fn open(path: &Path) -> io::Result<Log> {
        use std::os::unix::fs::OpenOptionsExt;
        if std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && m.len() > super::LOG_ROTATE_AT) {
            let mut aside = path.as_os_str().to_owned();
            aside.push(".1");
            std::fs::rename(path, aside)?;
        }
        let f = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(path)?;
        Ok(Log { inner: Mutex::new((Some(f), 0)) })
    }

    pub fn line(&self, v: &serde_json::Value) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut s = v.to_string();
        s.push('\n');
        let full = g.1 + s.len() as u64 > super::LOG_SESSION_CAP;
        let Some(f) = g.0.as_mut() else { return };
        if full {
            let _ = f.write_all(b"{\"event\":\"log full\",\"why\":\"this session wrote its cap; later decisions are not logged\"}\n");
            g.0 = None;
            return;
        }
        if f.write_all(s.as_bytes()).is_ok() {
            g.1 += s.len() as u64;
        }
    }

    pub fn write(&self, r: &Record) {
        if let Ok(v) = serde_json::to_value(r) {
            self.line(&v);
        }
    }
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

// ── preparing (in the exec helper, before the fence) ─────────────────

/// What the supervisor holds from spawn.
pub struct Prepared {
    pub mode: Mode,
    pub rules: Rules,
    pub log: Log,
}

/// Open the log and the temp folder's handle (before Landlock: the log is
/// outside the view). `Ok(None)`: record mode on a kernel without what the
/// block needs (said in the log; enforce refuses the spawn instead).
pub fn prepare(spec: &BlockSpec) -> Result<Option<Prepared>, String> {
    let log = Log::open(Path::new(&spec.log)).map_err(|e| format!("its decision log {}: {e}", spec.log))?;
    if let Err(why) = super::support() {
        return match spec.mode {
            Mode::Record => {
                log.line(&serde_json::json!({ "t": now(), "event": "not started", "mode": "record", "why": why }));
                Ok(None)
            }
            Mode::Enforce => Err(why),
        };
    }
    let hook = match spec.hook_sock.as_deref() {
        Some(h) if plain_absolute(h) => Some(HookRule { path: h.as_bytes().to_vec(), uid: spec.k2_uid }),
        Some(h) => return Err(format!("the hook socket path {h:?} isn't a plain absolute path")),
        None => None,
    };
    if !plain_absolute(&spec.tmp_dir) {
        return Err(format!("the temp folder {:?} isn't a plain absolute path", spec.tmp_dir));
    }
    let c = cstr(spec.tmp_dir.as_bytes())?;
    let dir = sys::openat2(libc::AT_FDCWD, &c, libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC, sys::RESOLVE_NO_SYMLINKS)
        .map_err(|e| format!("the temp folder {}: {e}", spec.tmp_dir))?;
    let st = sys::fstat(dir.as_raw_fd()).map_err(|e| format!("the temp folder: {e}"))?;
    // SAFETY: geteuid has no failure mode.
    if st.st_uid != unsafe { libc::geteuid() } {
        return Err(format!("the temp folder {} isn't this user's", spec.tmp_dir));
    }
    let tmp = TmpRule { prefix: spec.tmp_dir.trim_end_matches('/').as_bytes().to_vec(), dir, dev: st.st_dev };
    Ok(Some(Prepared { mode: spec.mode, rules: Rules { hook, tmp: Some(tmp) }, log }))
}

// ── the pool for blocking connects ───────────────────────────────────

type Job = Box<dyn FnOnce() + Send>;

struct PoolState {
    q: VecDeque<Job>,
    idle: usize,
    total: usize,
}

struct PoolInner {
    st: Mutex<PoolState>,
    cv: Condvar,
    cap: usize,
}

/// A capped set of threads, started on demand, gone after 30 s idle.
pub struct Pool(Arc<PoolInner>);

impl Pool {
    pub fn new(cap: usize) -> Pool {
        Pool(Arc::new(PoolInner { st: Mutex::new(PoolState { q: VecDeque::new(), idle: 0, total: 0 }), cv: Condvar::new(), cap }))
    }

    /// Run `job` on a pool thread; `Err(job)` when the pool is full.
    pub fn submit(&self, job: Job) -> Result<(), Job> {
        let mut s = self.0.st.lock().unwrap_or_else(|e| e.into_inner());
        if s.idle > s.q.len() {
            s.q.push_back(job);
            drop(s);
            self.0.cv.notify_one();
            return Ok(());
        }
        if s.total >= self.0.cap {
            return Err(job);
        }
        s.total += 1;
        drop(s);
        let first = Arc::new(Mutex::new(Some(job)));
        let (inner, slot) = (Arc::clone(&self.0), Arc::clone(&first));
        let spawned = std::thread::Builder::new().name("k2-sockblock".into()).stack_size(256 << 10).spawn(move || {
            let job = slot.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some(j) = job {
                j();
            }
            worker(inner)
        });
        if spawned.is_err() {
            self.0.st.lock().unwrap_or_else(|e| e.into_inner()).total -= 1;
            let job = first.lock().unwrap_or_else(|e| e.into_inner()).take();
            return job.map_or(Ok(()), Err);
        }
        Ok(())
    }
}

fn worker(p: Arc<PoolInner>) {
    let mut s = p.st.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if let Some(j) = s.q.pop_front() {
            drop(s);
            j();
            s = p.st.lock().unwrap_or_else(|e| e.into_inner());
            continue;
        }
        s.idle += 1;
        let (g, to) = p.cv.wait_timeout(s, Duration::from_secs(30)).unwrap_or_else(|e| e.into_inner());
        s = g;
        s.idle -= 1;
        if to.timed_out() && s.q.is_empty() {
            s.total -= 1;
            return;
        }
    }
}

// ── answering notifications ──────────────────────────────────────────

struct Sup {
    listener: OwnedFd,
    mode: Mode,
    rules: Rules,
    log: Log,
    arch: Arch,
    pool: Pool,
    foreign_seen: Mutex<HashSet<u32>>,
}

impl Sup {
    fn reply(&self, id: u64, errno: i32) {
        let r = NotifResp { id, val: 0, error: if errno == 0 { 0 } else { -errno }, flags: 0 };
        let _ = sys::notif_send(self.listener.as_raw_fd(), &r);
    }

    /// Record mode: let the call run as the caller made it.
    fn let_run(&self, id: u64) {
        let r = NotifResp { id, val: 0, error: 0, flags: sys::NOTIF_FLAG_CONTINUE };
        let _ = sys::notif_send(self.listener.as_raw_fd(), &r);
    }

    #[allow(clippy::too_many_arguments)]
    fn rec(&self, pid: u32, call: &str, family: &str, path: Option<String>, verdict: &str, applied: &str, reason: &str) {
        self.log.write(&Record {
            t: now(),
            pid,
            call,
            family: family.to_string(),
            path,
            verdict,
            mode: self.mode.as_str(),
            applied,
            reason: reason.to_string(),
        });
    }

    /// Something enforce refuses: record logs it and lets it run; enforce
    /// logs it and refuses with `errno`.
    #[allow(clippy::too_many_arguments)]
    fn refuse(&self, n: &Notif, call: &str, family: &str, path: Option<String>, reason: &str, errno: i32, log: bool) {
        match self.mode {
            Mode::Record => {
                if log {
                    self.rec(n.pid, call, family, path, "refuse", "allowed", reason);
                }
                self.let_run(n.id);
            }
            Mode::Enforce => {
                if log {
                    self.rec(n.pid, call, family, path, "refuse", "refused", reason);
                }
                self.reply(n.id, errno);
            }
        }
    }

    fn handle(self: &Arc<Self>, n: Notif) {
        let nrs = self.arch.nrs();
        if self.arch.foreign(&n.data) {
            // Once per process: a 32-bit program notifies on every call.
            let first = {
                let mut s = self.foreign_seen.lock().unwrap_or_else(|e| e.into_inner());
                s.len() < 4096 && s.insert(n.pid)
            };
            let why = format!("a call through another architecture's entry (32-bit or x32), number {}", n.data.nr);
            return self.refuse(&n, "foreign", "-", None, &why, ENOSYS as i32, first);
        }
        let nr = n.data.nr as u32;
        if nr == nrs.connect {
            return self.connect(n);
        }
        let (call, why, errno) = if nr == nrs.io_uring_setup {
            ("io_uring_setup", "io_uring (its queued connects would skip this block)", ENOSYS)
        } else if nr == nrs.seccomp {
            ("seccomp", "a seccomp notification listener of its own", EPERM)
        } else if nr == nrs.socket || nr == nrs.socketpair {
            (if nr == nrs.socket { "socket" } else { "socketpair" }, "a Unix datagram socket", EPERM)
        } else {
            ("other", "a call the filter doesn't name", ENOSYS)
        };
        let family = if call.starts_with("socket") { "unix" } else { "-" };
        self.refuse(&n, call, family, None, why, errno as i32, true);
    }

    fn connect(self: &Arc<Self>, n: Notif) {
        let id = n.id;
        let fd = n.data.args[0] as u32 as i32;
        let addr = n.data.args[1];
        let len = n.data.args[2] as u32 as i32;
        if len < 0 || len as usize > std::mem::size_of::<libc::sockaddr_storage>() {
            // What the kernel says itself.
            return self.reply(id, libc::EINVAL);
        }
        let sfd = match fetch(n.pid, fd) {
            Ok(s) => s,
            Err(e) if e.raw_os_error() == Some(libc::EBADF) => return self.reply(id, libc::EBADF),
            Err(e) => return self.refuse(&n, "connect", "?", None, &format!("its socket can't be taken ({e})"), libc::EPERM, true),
        };
        let domain = match sys::so_domain(sfd.as_raw_fd()) {
            Ok(d) => d,
            Err(e) => return self.reply(id, e.raw_os_error().unwrap_or(libc::ENOTSOCK)),
        };
        let family = super::family_name(domain);
        let bytes = match sys::read_mem(n.pid as i32, addr, len as usize) {
            Ok(b) => b,
            Err(e) if e.raw_os_error() == Some(libc::EFAULT) => return self.reply(id, libc::EFAULT),
            Err(e) => return self.refuse(&n, "connect", &family, None, &format!("its address can't be read ({e})"), libc::EPERM, true),
        };
        // The caller still waits: the socket and the address are its own.
        if !sys::notif_id_valid(self.listener.as_raw_fd(), id) {
            return;
        }
        if domain != AF_UNIX {
            if self.mode == Mode::Record {
                self.rec(n.pid, "connect", &family, None, "allow", "allowed", "not a Unix socket");
            }
            return self.connect_other(n, sfd, bytes, family);
        }
        let name = parse_unix(&bytes);
        match &name {
            UnixName::Invalid => self.reply(id, libc::EINVAL),
            UnixName::Abstract(_) => self.refuse(&n, "connect", "unix", shown(&name), "an abstract name", libc::ECONNREFUSED, true),
            UnixName::Path(p) => match self.rules.check(p) {
                Check::Allowed(target, why) => {
                    if self.mode == Mode::Record {
                        self.rec(n.pid, "connect", "unix", shown(&name), "allow", "allowed", why);
                    }
                    let e = sys::connect_bytes(sfd.as_raw_fd(), &via_fd(&target));
                    self.reply(id, e);
                }
                Check::Refused(why) => self.refuse(&n, "connect", "unix", shown(&name), &why, libc::ECONNREFUSED, true),
            },
        }
    }

    fn connect_other(self: &Arc<Self>, n: Notif, sfd: OwnedFd, bytes: Vec<u8>, family: String) {
        if !sys::is_blocking(sfd.as_raw_fd()) {
            return self.reply(n.id, sys::connect_bytes(sfd.as_raw_fd(), &bytes));
        }
        let me = Arc::clone(self);
        let id = n.id;
        let job: Job = Box::new(move || {
            let e = sys::connect_bytes(sfd.as_raw_fd(), &bytes);
            me.reply(id, e);
        });
        if self.pool.submit(job).is_err() {
            let why = format!("{POOL_CAP} blocking connects already in flight");
            self.refuse(&n, "connect", &family, None, &why, libc::EAGAIN, true);
        }
    }
}

/// The caller's descriptor `fd`, by a copy (its thread's process).
fn fetch(tid: u32, fd: RawFd) -> io::Result<OwnedFd> {
    // A process's main thread: its own id. Another thread (6.8 opens no
    // pidfd on a thread): its process, read from /proc.
    let pidfd = match sys::pidfd_open(tid as i32) {
        Err(e) if e.raw_os_error() == Some(libc::EINVAL) => sys::pidfd_open(sys::tgid_of(tid)?)?,
        r => r?,
    };
    sys::pidfd_getfd(&pidfd, fd)
}

// ── the session ──────────────────────────────────────────────────────

fn sigset_all() -> libc::sigset_t {
    // SAFETY: sigfillset initialises the set.
    unsafe {
        let mut s: libc::sigset_t = std::mem::zeroed();
        libc::sigfillset(&mut s);
        s
    }
}

fn pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0 as RawFd; 2];
    // SAFETY: a two-element out-array.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: both just made.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

fn write_all(fd: RawFd, b: &[u8]) -> bool {
    // SAFETY: b is valid for its length.
    unsafe { libc::write(fd, b.as_ptr().cast(), b.len()) == b.len() as isize }
}

fn read_exact(fd: RawFd, b: &mut [u8]) -> bool {
    // SAFETY: b is valid for its length.
    unsafe { libc::read(fd, b.as_mut_ptr().cast(), b.len()) == b.len() as isize }
}

fn refused(why: &str) -> i32 {
    eprintln!("{} agent isolation refused to start this terminal: its Unix-socket block: {why}. Nothing ran.", crate::brand::PRODUCT_NAME);
    crate::agent_fence::EXIT_REFUSED
}

/// Run `cmd` under the block. Returns only when the program never started
/// (the refusal's exit status); otherwise exits with the program's status.
pub fn run(prep: Prepared, cmd: std::process::Command) -> i32 {
    let Some(arch) = Arch::native() else { return refused("this architecture isn't supported") };
    let prog = filter::program(arch, prep.mode);
    let notif_size = match sys::notif_size() {
        Ok(n) => n,
        Err(e) => return refused(&format!("seccomp notification sizes: {e}")),
    };
    let all = sigset_all();
    // SAFETY: plain process calls on this (single-threaded) process.
    let (old, tty) = unsafe {
        let mut old: libc::sigset_t = std::mem::zeroed();
        libc::pthread_sigmask(libc::SIG_SETMASK, &all, &mut old);
        libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0);
        (old, libc::isatty(0) == 1 && libc::tcgetpgrp(0) == libc::getpgrp())
    };
    let ((up_r, up_w), (down_r, down_w)) = match (pipe(), pipe()) {
        (Ok(a), Ok(b)) => (a, b),
        _ => return refused("pipes"),
    };
    // SAFETY: getpid has no failure mode.
    let parent = unsafe { libc::getpid() };
    // SAFETY: the helper is single-threaded here; the child only runs this
    // module's code, then execs.
    let child = unsafe { libc::fork() };
    if child < 0 {
        return refused(&format!("fork: {}", io::Error::last_os_error()));
    }
    if child == 0 {
        drop(up_r);
        drop(down_w);
        child_side(&prog, tty, up_w.into_raw_fd(), down_r.into_raw_fd(), &old, parent, cmd);
    }
    drop(up_w);
    drop(down_r);
    let kill_child = |why: String| -> i32 {
        // SAFETY: our own child, not yet running the program.
        unsafe {
            libc::kill(child, libc::SIGKILL);
            libc::waitpid(child, std::ptr::null_mut(), 0);
        }
        refused(&why)
    };
    let mut b = [0u8; 4];
    if !read_exact(up_r.as_raw_fd(), &mut b) {
        return kill_child("the child didn't report its filter".into());
    }
    let lfd = i32::from_ne_bytes(b);
    if lfd < 0 {
        return kill_child(format!("installing the filter: {}", io::Error::from_raw_os_error(-lfd)));
    }
    let listener = match sys::pidfd_open(child).and_then(|p| sys::pidfd_getfd(&p, lfd)) {
        Ok(l) => l,
        Err(e) => return kill_child(format!("taking the listener: {e}")),
    };
    // SAFETY: plain process calls on this process.
    let sigfd = unsafe {
        libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0);
        libc::signalfd(-1, &all, libc::SFD_CLOEXEC | libc::SFD_NONBLOCK)
    };
    if sigfd < 0 {
        return kill_child(format!("signalfd: {}", io::Error::last_os_error()));
    }
    // SAFETY: just made.
    let sigfd = unsafe { OwnedFd::from_raw_fd(sigfd) };
    raise_nofile();
    std::panic::set_hook(Box::new(|info| {
        eprintln!("{}: the Unix-socket block's supervisor stopped: {info}", crate::brand::PRODUCT_NAME);
        std::process::abort();
    }));
    let start = serde_json::json!({
        "t": now(), "event": "start", "mode": prep.mode.as_str(), "pid": child,
        "hook": prep.rules.hook.as_ref().map(|h| String::from_utf8_lossy(&h.path).into_owned()),
        "tmp": prep.rules.tmp.as_ref().map(|t| String::from_utf8_lossy(&t.prefix).into_owned()),
    });
    if prep.mode == Mode::Record {
        prep.log.line(&start);
    }
    if !write_all(down_w.as_raw_fd(), b"1") {
        return kill_child("the child went away before its program".into());
    }
    drop(down_w);
    drop(up_r);
    let sup = Arc::new(Sup {
        listener,
        mode: prep.mode,
        rules: prep.rules,
        log: prep.log,
        arch,
        pool: Pool::new(POOL_CAP),
        foreign_seen: Mutex::new(HashSet::new()),
    });
    let status = main_loop(&sup, &sigfd, child, tty, notif_size);
    end_session();
    exit_like(status)
}

/// The child: filter, hand-over, close, exec. Never returns.
fn child_side(prog: &[filter::SockFilter], tty: bool, up: RawFd, down: RawFd, old: &libc::sigset_t, parent: i32, mut cmd: std::process::Command) -> ! {
    use std::os::unix::process::CommandExt;
    // SAFETY: plain process calls on this process; _exit never returns.
    unsafe {
        libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0);
        if libc::getppid() != parent {
            libc::_exit(crate::agent_fence::EXIT_REFUSED);
        }
        if tty {
            // The program leads its own group, the terminal's foreground one
            // (Ctrl-C and window-size changes go to it, not the supervisor).
            libc::setpgid(0, 0);
            libc::tcsetpgrp(0, libc::getpid());
        }
        libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0);
    }
    let lfd = match sys::seccomp_install(prog, filter::FILTER_FLAG_NEW_LISTENER | filter::FILTER_FLAG_WAIT_KILLABLE_RECV) {
        Ok(fd) => fd,
        Err(e) => -e.raw_os_error().unwrap_or(libc::EINVAL),
    };
    let mut go = [0u8; 1];
    if !write_all(up, &lfd.to_ne_bytes()) || lfd < 0 || !read_exact(down, &mut go) || go[0] != b'1' {
        // SAFETY: never returns.
        unsafe { libc::_exit(crate::agent_fence::EXIT_REFUSED) };
    }
    // The listener, the pipes and everything else the helper held: a
    // process holding the listener could answer its own notifications.
    sys::close_from(3);
    // SAFETY: restores the mask the helper started with.
    unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, old, std::ptr::null_mut()) };
    let e = cmd.exec();
    eprintln!("{}: {}: {e}", crate::brand::PRODUCT_NAME, cmd.get_program().to_string_lossy());
    // SAFETY: never returns.
    unsafe { libc::_exit(127) }
}

fn raise_nofile() {
    // SAFETY: sized in/out params.
    unsafe {
        let mut r: libc::rlimit = std::mem::zeroed();
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut r) == 0 && r.rlim_cur < r.rlim_max {
            r.rlim_cur = r.rlim_max;
            libc::setrlimit(libc::RLIMIT_NOFILE, &r);
        }
    }
}

/// Signals passed on to the program (a terminal's own go to its group).
const FORWARD: &[i32] = &[libc::SIGHUP, libc::SIGINT, libc::SIGQUIT, libc::SIGTERM, libc::SIGUSR1, libc::SIGUSR2, libc::SIGWINCH, libc::SIGCONT];

fn send(child: i32, tty: bool, sig: i32) {
    // SAFETY: signals to our own child (or its group, which it leads).
    unsafe {
        if tty {
            libc::killpg(child, sig);
        } else {
            libc::kill(child, sig);
        }
    }
}

fn main_loop(sup: &Arc<Sup>, sigfd: &OwnedFd, child: i32, tty: bool, notif_size: usize) -> i32 {
    let mut status: Option<i32> = None;
    let mut hup_at: Option<Instant> = None;
    let mut listen = true;
    loop {
        let mut fds = [
            libc::pollfd { fd: if listen { sup.listener.as_raw_fd() } else { -1 }, events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: sigfd.as_raw_fd(), events: libc::POLLIN, revents: 0 },
        ];
        // A periodic wake as well: SIGCHLD can't be lost, but reaping is cheap.
        let timeout = hup_at.map_or(1000, |t| t.saturating_duration_since(Instant::now()).as_millis().min(1000) as i32);
        // SAFETY: two valid pollfds.
        unsafe { libc::poll(fds.as_mut_ptr(), 2, timeout) };
        if fds[1].revents & libc::POLLIN != 0 {
            let mut info = [0u8; 128];
            while read_exact(sigfd.as_raw_fd(), &mut info) {
                let sig = u32::from_ne_bytes([info[0], info[1], info[2], info[3]]) as i32;
                if sig == libc::SIGHUP {
                    send(child, tty, libc::SIGHUP);
                    send(child, tty, libc::SIGCONT);
                    hup_at.get_or_insert(Instant::now() + HUP_GRACE);
                } else if FORWARD.contains(&sig) {
                    send(child, tty, sig);
                }
            }
        }
        loop {
            let mut st = 0;
            // SAFETY: an out-param.
            let r = unsafe { libc::waitpid(-1, &mut st, libc::WNOHANG) };
            if r <= 0 {
                break;
            }
            if r == child {
                status = Some(st);
            }
        }
        if let Some(s) = status {
            return s;
        }
        if hup_at.is_some_and(|t| Instant::now() >= t) {
            send(child, tty, libc::SIGKILL);
            hup_at = None;
        }
        if fds[0].revents & libc::POLLIN != 0 {
            match sys::notif_recv(sup.listener.as_raw_fd(), notif_size) {
                Ok(n) => sup.handle(n),
                // The caller went away between the wake and the receive.
                Err(e) if matches!(e.raw_os_error(), Some(libc::ENOENT | libc::EINTR)) => {}
                Err(e) => {
                    // Nothing can be answered: end the session, never leave
                    // it waiting.
                    sup.log.line(&serde_json::json!({ "t": now(), "event": "stopped", "why": format!("receiving: {e}") }));
                    send(child, tty, libc::SIGKILL);
                    listen = false;
                }
            }
        } else if fds[0].revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
            listen = false;
        }
    }
}

/// Every process still under the supervisor (its children, theirs, …).
fn descendants() -> Vec<i32> {
    let mut out = Vec::new();
    // SAFETY: getpid has no failure mode.
    let mut todo = vec![unsafe { libc::getpid() }];
    while let Some(p) = todo.pop() {
        let Ok(rd) = std::fs::read_dir(format!("/proc/{p}/task")) else { continue };
        for t in rd.flatten() {
            let kids = std::fs::read_to_string(t.path().join("children")).unwrap_or_default();
            for k in kids.split_whitespace().filter_map(|k| k.parse::<i32>().ok()) {
                if out.len() < 100_000 {
                    out.push(k);
                    todo.push(k);
                }
            }
        }
    }
    out
}

/// The program has exited: what is left of the session ends with it (a
/// process left after the supervisor could connect nowhere).
fn end_session() {
    let t = Instant::now();
    loop {
        for p in descendants() {
            // SAFETY: processes under this supervisor.
            unsafe { libc::kill(p, libc::SIGKILL) };
        }
        loop {
            // SAFETY: reaps our own children.
            let r = unsafe { libc::waitpid(-1, std::ptr::null_mut(), libc::WNOHANG) };
            if r > 0 {
                continue;
            }
            if r < 0 {
                return;
            }
            break;
        }
        if t.elapsed() > END_GRACE {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Exit the way the program did.
fn exit_like(st: i32) -> ! {
    if libc::WIFEXITED(st) {
        std::process::exit(libc::WEXITSTATUS(st));
    }
    if libc::WIFSIGNALED(st) {
        let sig = libc::WTERMSIG(st);
        // SAFETY: default action for sig, unblocked, then raised on this thread.
        unsafe {
            libc::signal(sig, libc::SIG_DFL);
            let mut s: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut s);
            libc::sigaddset(&mut s, sig);
            libc::pthread_sigmask(libc::SIG_UNBLOCK, &s, std::ptr::null_mut());
            libc::raise(sig);
        }
        std::process::exit(128 + sig);
    }
    std::process::exit(1)
}
