//! `agent` delivery for `kind=ssh_key` (prd-vault-gaps-v1 item 3): one
//! ssh-agent socket per session (or `k2 job` run), served by the daemon.
//! The key never leaves K2: the socket lists the keys granted to that
//! session as `agent` and signs with them; nothing else.
//!
//! - **Where.** `<secrets dir>/a.sock` (the session's memory-backed 0700 dir,
//!   [`super::spawn`]). A path over [`MAX_SOCK_PATH`] bytes (macOS
//!   `$TMPDIR`) uses `<short root>/<8 base62>/a.sock`: `$TMPDIR/k2a-<ns>`
//!   (per user), else `/tmp/k2a-<uid>-<ns>`, where `<ns>` is 8 hex chars of
//!   the vault id (one root per K2 home); 0700 dirs, a socket holds no
//!   secret bytes. The record keeps the root it used.
//! - **The daemon's record.** Every socket is recorded in vault.db
//!   (`agent_socket`, MAC'd: tag, workspace, secrets dir, socket path)
//!   when it starts and dropped when it stops. A restarted daemon re-serves
//!   a socket ([`on_adopted`]) and the boot sweep removes one
//!   ([`boot_sweep`]) only from that record, never from files in the
//!   secrets dir or the ptyd meta (both writable by any same-user process).
//!   Once k2-ptyd spawns the session, the record is bound to that ptyd
//!   instance and process (boot id, pid, start time). Re-serving waits for
//!   ptyd adoption, needs that same process, and needs the ptyd meta to
//!   agree with the record; a record never bound to ptyd is never re-served.
//! - **Who may connect.** The peer's uid must be the daemon's euid
//!   (`SO_PEERCRED` / `getpeereid`); anything else is closed at accept.
//!   At most [`MAX_CONNS`] connections per socket; a frame must arrive
//!   within [`FRAME_DEADLINE`] once started, and a connection with no
//!   request for [`IDLE_DEADLINE`] is closed.
//! - **What it answers.** k2-vault's `ssh_agent` subset: identities, sign
//!   (ed25519; RSA SHA-2 only), `session-bind@openssh.com`; everything else
//!   is `FAILURE`; a bad frame, an oversized frame or a bind whose host
//!   signature fails closes the connection.
//! - **Every signature** re-checks the grant in the vault (revocation,
//!   disable, expiry, rotation take effect at once), the per-secret
//!   settings, and the per-(session, secret) rate limit (default burst
//!   60/s, 240/min). Listings and refused signs are throttled per session
//!   before any vault work. One rolled-up `sign` audit entry per signature
//!   or refusal.
//!
//! Honest line: anything in the session, or any same-user process that
//! finds the socket, can sign while the session runs. The forwarding
//! refusal and the audit's host key are advisory: they come from what the
//! ssh client says in its session bind; a client that sends no bind (or a
//! process speaking the protocol itself) can't be told apart.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use k2_vault::ssh_agent::{self as proto, BindCheck, Request};
use k2_vault::vault::AgentSocket;
use k2_vault::{LocalVault, Requester};
use parking_lot::Mutex;

/// The socket's name.
pub const SOCK_NAME: &str = "a.sock";
/// `sun_path` is 104 bytes on macOS, 108 on Linux; keep a margin.
pub const MAX_SOCK_PATH: usize = 100;
/// Connections one socket serves at once.
pub const MAX_CONNS: usize = 32;
/// Session binds one connection may send (OpenSSH's own agent: 16).
pub const MAX_BINDS: usize = 16;
/// Once a frame's first byte arrives, the rest must within this.
pub const FRAME_DEADLINE: Duration = Duration::from_secs(10);
/// A connection that sends no request for this long is closed (ssh opens
/// one per use; nothing legitimate idles this long).
pub const IDLE_DEADLINE: Duration = Duration::from_secs(300);
/// Listings per session: a second and a minute.
const IDS_PER_SECOND: usize = 20;
const IDS_PER_MINUTE: usize = 600;
/// Refused signs per session before further signs are refused without
/// touching the vault.
const REFUSALS_PER_SECOND: usize = 10;
const REFUSALS_PER_MINUTE: usize = 120;

/// Read deadlines of one socket (tests shorten them).
#[derive(Debug, Clone, Copy)]
pub struct Deadlines {
    pub frame: Duration,
    pub idle: Duration,
}

impl Default for Deadlines {
    fn default() -> Self {
        Deadlines { frame: FRAME_DEADLINE, idle: IDLE_DEADLINE }
    }
}

struct Ctx {
    who: Requester,
    /// Session id (or job run id): the rate-limit keys' first half.
    tag: String,
    stop: AtomicBool,
    conns: AtomicUsize,
    deadlines: Deadlines,
    /// The last limits read per secret, so the rate check runs before the
    /// policy read (the fresh policy then decides).
    limits: Mutex<HashMap<String, (usize, usize)>>,
}

struct Server {
    dir: PathBuf,
    sock: PathBuf,
    /// The short-path dir when the socket isn't in `dir`.
    fallback: Option<PathBuf>,
    ctx: Arc<Ctx>,
}

static SERVERS: Mutex<Vec<Server>> = Mutex::new(Vec::new());

/// Per-(session, secret) signature limits, per-session listing and
/// refusal budgets; each call passes its own limits.
fn limiter() -> &'static crate::vault_fetch::RateLimiter {
    static L: OnceLock<crate::vault_fetch::RateLimiter> = OnceLock::new();
    let d = k2_vault::vault::AgentPolicy::default();
    L.get_or_init(|| crate::vault_fetch::RateLimiter::new(d.burst as usize, d.per_minute as usize))
}

fn euid() -> u32 {
    unsafe { libc::geteuid() }
}

/// This vault's short-path namespace: 8 hex chars of its id. Every K2 home
/// (production, a worktree smoke daemon, a test) has its own vault and so
/// its own short-path root: one never sweeps another's sockets.
pub fn namespace(vault: &LocalVault) -> String {
    vault.vault_id().chars().filter(|c| c.is_ascii_hexdigit()).take(8).collect::<String>().to_ascii_lowercase()
}

/// Is `name` a short-path root's name for namespace `ns`
/// (`k2a-<ns>` or `k2a-<uid>-<ns>`)?
fn root_name_ok(name: &str, ns: &str) -> bool {
    ns.len() == 8 && (name == format!("k2a-{ns}") || name == format!("k2a-{}-{ns}", euid()))
}

/// Short roots for sockets whose dir path is too long, in order:
/// `$TMPDIR/k2a-<ns>` (per user on macOS), then `/tmp/k2a-<uid>-<ns>`.
pub fn fallback_roots(ns: &str) -> Vec<PathBuf> {
    let mut v = vec![];
    if let Some(t) = std::env::var_os("TMPDIR").filter(|t| !t.is_empty()) {
        v.push(PathBuf::from(t).join(format!("k2a-{ns}")));
    }
    v.push(PathBuf::from(format!("/tmp/k2a-{}-{ns}", euid())));
    v
}

/// Make `dir` 0700 and ours (create if missing); refuse symlinks and other
/// owners.
fn ensure_private(dir: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(dir) {
        Ok(m) => {
            if m.file_type().is_symlink() || !m.is_dir() || m.uid() != euid() {
                return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "unsafe agent socket dir"));
            }
            if m.permissions().mode() & 0o077 != 0 {
                std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => std::fs::DirBuilder::new().mode(0o700).create(dir),
        Err(e) => Err(e),
    }
}

/// Where a new socket for `dir` goes: the socket path, and (for a short
/// path) its own dir and the root it was made under.
pub struct SockPath {
    pub sock: PathBuf,
    pub short: Option<(PathBuf, PathBuf)>,
}

pub fn sock_path_for(ns: &str, dir: &Path) -> std::io::Result<SockPath> {
    let direct = dir.join(SOCK_NAME);
    if direct.as_os_str().len() <= MAX_SOCK_PATH {
        return Ok(SockPath { sock: direct, short: None });
    }
    let mut last = std::io::Error::new(std::io::ErrorKind::InvalidInput, "agent socket path too long");
    for root in fallback_roots(ns) {
        // `<root>/<8>/a.sock` must fit.
        if root.as_os_str().len() + 1 + 8 + 1 + SOCK_NAME.len() > MAX_SOCK_PATH {
            continue;
        }
        if let Some(parent) = root.parent() {
            if !parent.is_dir() {
                continue;
            }
        }
        if let Err(e) = ensure_private(&root) {
            last = e;
            continue;
        }
        let sub = root.join(k2_vault::crypto::base62(8));
        std::fs::DirBuilder::new().mode(0o700).create(&sub)?;
        let m = std::fs::symlink_metadata(&sub)?;
        if m.file_type().is_symlink() || m.uid() != euid() || m.permissions().mode() & 0o077 != 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "agent socket dir check failed"));
        }
        return Ok(SockPath { sock: sub.join(SOCK_NAME), short: Some((sub, root)) });
    }
    Err(last)
}

/// A short path must be exactly `<root>/<8 base62>/a.sock` for the given
/// root; returns its dir.
fn short_dir_under(p: &Path, root: &Path) -> Option<PathBuf> {
    let parent = p.parent()?;
    let name = parent.file_name()?.to_str()?;
    let ok = p.file_name()? == SOCK_NAME
        && parent.parent()? == root
        && name.len() == 8
        && name.bytes().all(|b| b.is_ascii_alphanumeric());
    ok.then(|| parent.to_path_buf())
}

/// The socket a record may name: `<its secrets dir>/a.sock`, or a short
/// path under the root the record itself names (a root of this vault's
/// namespace, wherever `$TMPDIR` points now) that no other record names.
/// Anything else is ignored: a record that points at another session's
/// socket never re-serves or removes it.
fn record_socket(rec: &AgentSocket, all: &[AgentSocket], ns: &str) -> Option<(PathBuf, Option<PathBuf>)> {
    let sock = PathBuf::from(&rec.sock_path);
    if rec.short_root.is_empty() {
        return (sock == Path::new(&rec.secrets_dir).join(SOCK_NAME)).then_some((sock, None));
    }
    let root = Path::new(&rec.short_root);
    if !root.is_absolute() || !root.file_name().and_then(|n| n.to_str()).is_some_and(|n| root_name_ok(n, ns)) {
        return None;
    }
    let parent = short_dir_under(&sock, root)?;
    let shared = all.iter().any(|o| o.tag != rec.tag && Path::new(&o.sock_path) == sock);
    (!shared).then_some((sock, Some(parent)))
}

fn bind(sock: &Path) -> std::io::Result<UnixListener> {
    let l = UnixListener::bind(sock)?;
    std::fs::set_permissions(sock, std::fs::Permissions::from_mode(0o600))?;
    Ok(l)
}

/// `(kind, workspace)` of a requester that may hold an agent socket.
fn record_owner(who: &Requester) -> Option<(&'static str, String)> {
    match who {
        Requester::Session { workspace_id, session_id: Some(_) } => Some(("session", workspace_id.clone())),
        Requester::Job { workspace_id, .. } => Some(("job", workspace_id.clone())),
        _ => None,
    }
}

/// Start serving `who`'s agent keys on a new socket for secrets dir `dir`,
/// and record it. Returns the socket path (the session's `SSH_AUTH_SOCK`).
pub fn start(vault: &LocalVault, who: Requester, tag: &str, dir: &Path) -> std::io::Result<PathBuf> {
    start_with(vault, who, tag, dir, Deadlines::default())
}

pub fn start_with(vault: &LocalVault, who: Requester, tag: &str, dir: &Path, deadlines: Deadlines) -> std::io::Result<PathBuf> {
    let (kind, ws) = record_owner(&who).ok_or_else(|| std::io::Error::other("no agent socket for this requester"))?;
    let SockPath { sock, short } = sock_path_for(&namespace(vault), dir)?;
    let fallback = short.as_ref().map(|(d, _)| d.clone());
    let cleanup = |sock: &Path| {
        let _ = std::fs::remove_file(sock);
        if let Some(f) = &fallback {
            let _ = std::fs::remove_dir(f);
        }
    };
    let l = match bind(&sock) {
        Ok(l) => l,
        Err(e) => {
            if let Some(f) = &fallback {
                let _ = std::fs::remove_dir(f);
            }
            return Err(e);
        }
    };
    let rec = AgentSocket {
        tag: tag.to_string(),
        kind: kind.to_string(),
        workspace_id: ws,
        secrets_dir: dir.to_string_lossy().into_owned(),
        sock_path: sock.to_string_lossy().into_owned(),
        short_root: short.as_ref().map(|(_, r)| r.to_string_lossy().into_owned()).unwrap_or_default(),
        created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        ..Default::default()
    };
    if let Err(e) = vault.agent_socket_put(&rec) {
        cleanup(&sock);
        return Err(std::io::Error::other(format!("record: {}", e.code)));
    }
    run(l, who, tag, dir, &sock, fallback.clone(), deadlines);
    Ok(sock)
}

/// The k2-ptyd spawn hook: bind session `sid`'s agent socket record (if it
/// has one) to this ptyd instance and the process ptyd just started, with
/// the start time ptyd itself records (its session list) and the kernel's
/// exact start key for that pid ([`process_start_key`]).
pub fn on_ptyd_spawned(sid: &str, pid: i32, boot_id: &str) {
    let Ok(v) = super::get() else { return };
    if !v.agent_sockets().is_ok_and(|all| all.iter().any(|r| r.tag == sid && r.kind == "session")) {
        return;
    }
    #[cfg(unix)]
    {
        let started = k2_core::terminal::ptyd::client::global()
            .owner()
            .ok()
            .and_then(|o| k2_core::terminal::ptyd::adopt::list(&o).running.into_iter().find(|s| s.sid == sid && s.pid == pid))
            .map(|s| s.started_at as i64);
        let key = process_start_key(i64::from(pid));
        match started.zip(key) {
            Some((t, k)) => match v.agent_socket_bind(sid, boot_id, i64::from(pid), t, k) {
                Ok(true) => {}
                Ok(false) => k2_core::log_debug!("[vault] ssh-agent record for {sid} is bound to another process; left as is"),
                Err(e) => k2_core::log_debug!("[vault] ssh-agent record for {sid} not bound: {}", e.code),
            },
            None => k2_core::log_debug!("[vault] ssh-agent record for {sid} not bound: ptyd didn't list the session, or the OS has no start for it"),
        }
    }
}

/// Re-bind `sock` (left by the previous daemon) and serve it.
fn rebind(rec: &AgentSocket, sock: &Path, fallback: Option<PathBuf>) -> std::io::Result<PathBuf> {
    if let Some(f) = &fallback {
        if let Some(root) = f.parent() {
            ensure_private(root)?;
        }
        ensure_private(f)?;
    }
    // Only ever replace our own socket, never a file, link or someone else's.
    if let Ok(m) = std::fs::symlink_metadata(sock) {
        if !m.file_type().is_socket() || m.uid() != euid() {
            return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "not our agent socket"));
        }
        std::fs::remove_file(sock)?;
    }
    let l = bind(sock)?;
    let who = Requester::Session { workspace_id: rec.workspace_id.clone(), session_id: Some(rec.tag.clone()) };
    run(l, who, &rec.tag, Path::new(&rec.secrets_dir), sock, fallback, Deadlines::default());
    Ok(sock.to_path_buf())
}

/// Why a restarted daemon won't re-serve a session's socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeRefused {
    /// No record, or one that isn't a session's.
    NoRecord,
    /// The session wasn't spawned under k2-ptyd (its record was never
    /// bound to a ptyd process).
    NotPtyd,
    /// The adopted process isn't the one the record was bound to (another
    /// ptyd instance, pid or start time: a replayed record, a forged spawn).
    ProcessMismatch,
    /// The ptyd meta's workspace isn't the recorded one.
    WorkspaceMismatch,
    /// The kept secrets dir (or the meta's) isn't the recorded one.
    DirMismatch,
    /// The recorded socket path isn't one this session may use.
    BadSocketPath,
    /// The OS says the bound pid isn't alive, or isn't the process the
    /// record was bound to (its kernel start key differs: a reused pid, a
    /// fake ptyd reporting a replayed record's values).
    ProcessGone,
    Io(String),
}

/// macOS: how far the kernel's start time may be from the time ptyd
/// recorded at spawn (ptyd stamps it right after the fork).
pub const START_TIME_SLACK_SECS: i64 = 2;

/// The kernel's exact start key of a live process (`None`: not alive, a
/// zombie, or unknown), read at spawn and compared exactly at adoption.
/// Linux: `/proc/<pid>/stat` field 22, the raw starttime in clock ticks
/// since boot (no `btime` and no wall clock, so a clock step can't move
/// it). macOS: `proc_pidinfo` start timeval in microseconds (stored at
/// fork).
#[cfg(target_os = "linux")]
pub fn process_start_key(pid: i64) -> Option<i64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = &stat[stat.rfind(')')? + 1..];
    let f: Vec<&str> = rest.split_whitespace().collect();
    // f[0] is field 3 (state); field 22 (starttime) is f[19].
    if matches!(*f.first()?, "Z" | "X" | "x") {
        return None;
    }
    f.get(19)?.parse::<i64>().ok().filter(|t| *t > 0)
}

#[cfg(target_os = "macos")]
fn bsd_info(pid: i64) -> Option<libc::proc_bsdinfo> {
    let pid = libc::c_int::try_from(pid).ok()?;
    // SAFETY: proc_pidinfo writes at most `size` bytes into `info`.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let n = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, &mut info as *mut _ as *mut libc::c_void, size) };
    // SZOMB = 5: a zombie isn't alive.
    (n == size && info.pbi_status != 5).then_some(info)
}

#[cfg(target_os = "macos")]
pub fn process_start_key(pid: i64) -> Option<i64> {
    bsd_info(pid).map(|i| i.pbi_start_tvsec as i64 * 1_000_000 + i.pbi_start_tvusec as i64).filter(|k| *k > 0)
}

/// macOS: the kernel's start time (Unix seconds) of a live process.
#[cfg(target_os = "macos")]
pub fn process_start_secs(pid: i64) -> Option<i64> {
    bsd_info(pid).map(|i| i.pbi_start_tvsec as i64)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn process_start_key(_pid: i64) -> Option<i64> {
    None
}

/// Is `pid` alive and the very process the record was bound to: its
/// kernel start key equals `proc_start` exactly. macOS also keeps the
/// check that ptyd's `started_at` is the kernel's start time (within the
/// slack); Linux doesn't compare wall-clock times at all (a clock step
/// would fail it closed), the tick count is exact.
pub fn process_matches(pid: i64, started_at: i64, proc_start: i64) -> bool {
    if process_start_key(pid) != Some(proc_start) {
        return false;
    }
    #[cfg(target_os = "macos")]
    {
        process_start_secs(pid).is_some_and(|t| (t - started_at).abs() <= START_TIME_SLACK_SECS)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = started_at;
        true
    }
}

/// What k2-ptyd says about the adopted session: its instance and process.
#[derive(Debug, Clone, Copy)]
pub struct Adopted<'a> {
    pub boot_id: &'a str,
    pub pid: i64,
    pub started_at: i64,
}

/// k2-ptyd adopted session `sid` after a restart: re-serve its agent
/// socket from the daemon's record, if it had one. The record must be bound
/// to this same ptyd instance and process (`proc`), and the ptyd meta's
/// workspace and secrets dir and the dir the boot sweep kept must all
/// match it.
pub fn resume_adopted(
    vault: &LocalVault,
    sid: &str,
    proc: Adopted<'_>,
    meta_ws: Option<&str>,
    meta_dir: Option<&Path>,
    kept_dir: Option<&Path>,
) -> Result<PathBuf, ResumeRefused> {
    let all = vault.agent_sockets().map_err(|e| ResumeRefused::Io(e.code.to_string()))?;
    let rec = all.iter().find(|r| r.tag == sid && r.kind == "session").cloned().ok_or(ResumeRefused::NoRecord)?;
    if rec.ptyd_boot.is_empty() || rec.pid <= 0 || rec.started_at <= 0 || rec.proc_start <= 0 {
        return Err(ResumeRefused::NotPtyd);
    }
    if rec.ptyd_boot != proc.boot_id || rec.pid != proc.pid || rec.started_at != proc.started_at {
        return Err(ResumeRefused::ProcessMismatch);
    }
    if meta_ws != Some(rec.workspace_id.as_str()) {
        return Err(ResumeRefused::WorkspaceMismatch);
    }
    let recorded = Path::new(&rec.secrets_dir);
    if meta_dir != Some(recorded) || kept_dir != Some(recorded) {
        return Err(ResumeRefused::DirMismatch);
    }
    let (sock, fallback) = record_socket(&rec, &all, &namespace(vault)).ok_or(ResumeRefused::BadSocketPath)?;
    // What ptyd reported must also be what the OS says: the pid is alive
    // and is the process bound at spawn (a fake ptyd can repeat a replayed
    // record's values, not the kernel's).
    if !process_matches(rec.pid, rec.started_at, rec.proc_start) {
        return Err(ResumeRefused::ProcessGone);
    }
    rebind(&rec, &sock, fallback).map_err(|e| ResumeRefused::Io(e.to_string()))
}

/// The hook ptyd adoption calls (after the session is registered). A
/// record that is refused is dropped: nothing will re-serve it later.
#[cfg(unix)]
pub fn on_adopted(info: &k2_ptyd::proto::SessionInfo, boot_id: &str) {
    let Ok(vault) = super::get() else { return };
    let sid = info.sid.as_str();
    let meta_ws = info.meta.get("workspace_uuid").and_then(|v| v.as_str());
    let meta_dir = info.meta.get("secrets_dir").and_then(|v| v.as_str()).map(Path::new);
    let kept = super::spawn::kept_dir(sid);
    let proc = Adopted { boot_id, pid: i64::from(info.pid), started_at: info.started_at as i64 };
    match resume_adopted(&vault, sid, proc, meta_ws, meta_dir, kept.as_deref()) {
        Ok(sock) => k2_core::log_debug!("[vault] ssh-agent re-served for adopted session {sid} at {}", sock.display()),
        Err(ResumeRefused::NoRecord) => {}
        Err(why) => {
            k2_core::log_debug!("[vault] ssh-agent for adopted session {sid} NOT re-served: {why:?}");
            let _ = vault.agent_socket_remove(sid);
        }
    }
}

fn run(l: UnixListener, who: Requester, tag: &str, dir: &Path, sock: &Path, fallback: Option<PathBuf>, deadlines: Deadlines) {
    let ctx = Arc::new(Ctx {
        who,
        tag: tag.to_string(),
        stop: AtomicBool::new(false),
        conns: AtomicUsize::new(0),
        deadlines,
        limits: Mutex::new(HashMap::new()),
    });
    SERVERS.lock().push(Server { dir: dir.to_path_buf(), sock: sock.to_path_buf(), fallback, ctx: ctx.clone() });
    let c2 = ctx.clone();
    let spawned = std::thread::Builder::new().name("k2-ssh-agent".into()).spawn(move || accept_loop(l, c2));
    if spawned.is_err() {
        ctx.stop.store(true, Ordering::SeqCst);
    }
    k2_core::log_debug!("[vault] ssh-agent for {} at {}", ctx.who.label(), sock.display());
}

/// Unlink a socket we own (never a file, a link or another user's).
fn unlink_socket(p: &Path) {
    if std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_socket() && m.uid() == euid()) {
        let _ = std::fs::remove_file(p);
    }
}

/// Remove a short-path dir: its socket, then the (empty) dir.
fn remove_fallback(dir: &Path) {
    if std::fs::symlink_metadata(dir).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink() && m.uid() == euid()) {
        unlink_socket(&dir.join(SOCK_NAME));
        let _ = std::fs::remove_dir(dir);
    }
}

/// The secrets dir `dir` is going: stop its agent, unlink the socket and
/// the short-path dir, and drop the record. Safe for a dir with no agent.
pub fn stop_dir(dir: &Path) {
    let gone: Vec<Server> = {
        let mut g = SERVERS.lock();
        let (gone, keep): (Vec<_>, Vec<_>) = g.drain(..).partition(|s| s.dir == dir);
        *g = keep;
        gone
    };
    for s in gone {
        s.ctx.stop.store(true, Ordering::SeqCst);
        unlink_socket(&s.sock);
        if let Some(f) = &s.fallback {
            remove_fallback(f);
        }
        k2_core::log_debug!("[vault] ssh-agent for {} stopped", s.ctx.who.label());
    }
    // A dir from before a restart: follow the daemon's record, not files.
    let Ok(v) = super::get() else { return };
    let Ok(all) = v.agent_sockets() else { return };
    let ns = namespace(&v);
    for rec in all.iter().filter(|r| Path::new(&r.secrets_dir) == dir) {
        if let Some((sock, fallback)) = record_socket(rec, &all, &ns) {
            unlink_socket(&sock);
            if let Some(f) = fallback {
                remove_fallback(&f);
            }
        }
        let _ = v.agent_socket_remove(&rec.tag);
    }
}

/// Boot (after the secrets-dir sweep): drop the record (and socket) of
/// every tag that didn't survive, then remove short-path dirs in this
/// vault's own roots that no record of this vault names (left by a crash).
/// No connect probe (a planted never-accepting listener would block it):
/// the per-vault namespace is what keeps other daemons' sockets out. Each
/// dir is lstat'ed (a real dir, ours) and only its socket (a socket,
/// ours) is unlinked; links are never followed.
pub fn boot_sweep(vault: &LocalVault, kept: &[(String, PathBuf)]) {
    let Ok(all) = vault.agent_sockets() else { return };
    let ns = namespace(vault);
    for rec in &all {
        let survived = kept.iter().any(|(sid, d)| sid == &rec.tag && d == Path::new(&rec.secrets_dir)) && rec.kind == "session";
        if survived {
            continue;
        }
        if let Some((sock, fallback)) = record_socket(rec, &all, &ns) {
            unlink_socket(&sock);
            if let Some(f) = fallback {
                remove_fallback(&f);
            }
        }
        let _ = vault.agent_socket_remove(&rec.tag);
    }
    let left = vault.agent_sockets().unwrap_or_default();
    let named: Vec<PathBuf> = left.iter().filter_map(|r| record_socket(r, &left, &ns).and_then(|(_, f)| f)).collect();
    let mut roots = fallback_roots(&ns);
    for r in &left {
        let root = PathBuf::from(&r.short_root);
        if !r.short_root.is_empty() && root.file_name().and_then(|n| n.to_str()).is_some_and(|n| root_name_ok(n, &ns)) && !roots.contains(&root) {
            roots.push(root);
        }
    }
    for root in roots {
        let ok = std::fs::symlink_metadata(&root).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink() && m.uid() == euid());
        if !ok {
            continue;
        }
        for e in std::fs::read_dir(&root).into_iter().flatten().flatten() {
            let p = e.path();
            if short_dir_under(&p.join(SOCK_NAME), &root).is_none() || named.contains(&p) {
                continue;
            }
            remove_fallback(&p);
        }
    }
}

/// Tests: a live child process, the start time ptyd would have recorded
/// for it (macOS: the kernel's; Linux: now, never compared) and its kernel
/// start key (what the spawn hook binds). Kill it when done.
#[cfg(test)]
pub fn live_child_for_tests() -> (std::process::Child, i64, i64, i64) {
    let child = k2_core::bg_proc::command("sleep").arg("120").spawn().expect("spawn sleep");
    let pid = i64::from(child.id());
    let key = (0..50)
        .find_map(|_| {
            let k = process_start_key(pid);
            if k.is_none() {
                std::thread::sleep(Duration::from_millis(20));
            }
            k
        })
        .expect("the OS reports the child's start key");
    #[cfg(target_os = "macos")]
    let started = process_start_secs(pid).expect("the OS reports the child's start time");
    #[cfg(not(target_os = "macos"))]
    let started = chrono::Utc::now().timestamp();
    (child, pid, started, key)
}

/// Tests: what a daemon crash leaves behind. The server stops, but its
/// socket file, short-path dir and record stay (as after a restart under
/// k2-ptyd).
#[cfg(test)]
pub fn abandon_for_tests(dir: &Path) {
    let mut g = SERVERS.lock();
    for s in g.iter().filter(|s| s.dir == dir) {
        s.ctx.stop.store(true, Ordering::SeqCst);
    }
    g.retain(|s| s.dir != dir);
}

/// The socket serving `dir`, if any (status and tests).
pub fn socket_for(dir: &Path) -> Option<PathBuf> {
    SERVERS.lock().iter().find(|s| s.dir == dir).map(|s| s.sock.clone())
}

fn accept_loop(l: UnixListener, ctx: Arc<Ctx>) {
    if l.set_nonblocking(true).is_err() {
        return;
    }
    let fd = l.as_raw_fd();
    let me = euid();
    while !ctx.stop.load(Ordering::SeqCst) {
        let mut p = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        // SAFETY: one valid pollfd for the duration of the call.
        let n = unsafe { libc::poll(&mut p, 1, 500) };
        if n <= 0 {
            continue;
        }
        let s = match l.accept() {
            Ok((s, _)) => s,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => {
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
        };
        if peer_uid(&s) != Some(me) {
            k2_core::log_debug!("[vault] ssh-agent for {}: refused a connection from another user", ctx.who.label());
            continue;
        }
        if ctx.conns.fetch_add(1, Ordering::SeqCst) >= MAX_CONNS {
            ctx.conns.fetch_sub(1, Ordering::SeqCst);
            continue;
        }
        // macOS: an accepted socket inherits the listener's O_NONBLOCK.
        let ok = s.set_nonblocking(false).is_ok()
            && s.set_read_timeout(Some(Duration::from_millis(250))).is_ok()
            && s.set_write_timeout(Some(Duration::from_secs(10))).is_ok();
        if !ok {
            ctx.conns.fetch_sub(1, Ordering::SeqCst);
            continue;
        }
        let c2 = ctx.clone();
        let r = std::thread::Builder::new().name("k2-ssh-agent-conn".into()).spawn(move || {
            serve_conn(s, &c2);
            c2.conns.fetch_sub(1, Ordering::SeqCst);
        });
        if r.is_err() {
            ctx.conns.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn peer_uid(s: &UnixStream) -> Option<u32> {
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: getsockopt writes at most `len` bytes into `cred`.
    let rc = unsafe {
        libc::getsockopt(s.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, &mut cred as *mut _ as *mut libc::c_void, &mut len)
    };
    (rc == 0).then_some(cred.uid)
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn peer_uid(s: &UnixStream) -> Option<u32> {
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    // SAFETY: getpeereid writes the two ids.
    let rc = unsafe { libc::getpeereid(s.as_raw_fd(), &mut uid, &mut gid) };
    (rc == 0).then_some(uid)
}

/// Read exactly `buf.len()` bytes before `deadline`; `false` on EOF,
/// error, stop or the deadline.
fn read_full(s: &mut UnixStream, buf: &mut [u8], stop: &AtomicBool, deadline: Instant) -> bool {
    let mut got = 0;
    while got < buf.len() {
        if stop.load(Ordering::SeqCst) || Instant::now() >= deadline {
            return false;
        }
        match s.read(&mut buf[got..]) {
            Ok(0) => return false,
            Ok(n) => got += n,
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted) => {}
            Err(_) => return false,
        }
    }
    true
}

/// One frame's payload; `None` closes the connection (EOF, stop, idle or
/// frame deadline, a zero or oversized length).
fn read_frame(s: &mut UnixStream, ctx: &Ctx) -> Option<Vec<u8>> {
    let mut len = [0u8; 4];
    if !read_full(s, &mut len[..1], &ctx.stop, Instant::now() + ctx.deadlines.idle) {
        return None;
    }
    let deadline = Instant::now() + ctx.deadlines.frame;
    if !read_full(s, &mut len[1..], &ctx.stop, deadline) {
        return None;
    }
    let n = u32::from_be_bytes(len) as usize;
    if n == 0 || n > proto::MAX_FRAME {
        return None;
    }
    let mut buf = vec![0u8; n];
    read_full(s, &mut buf, &ctx.stop, deadline).then_some(buf)
}

/// Per-connection state from session binds.
#[derive(Default)]
struct Conn {
    /// Any bind on this connection said `is_forwarding`: sticky.
    forwarded: bool,
    /// The last bind's host-key fingerprint (`?` suffix: unverified).
    host_fp: Option<String>,
    binds: usize,
}

fn serve_conn(mut s: UnixStream, ctx: &Ctx) {
    let mut conn = Conn::default();
    while let Some(payload) = read_frame(&mut s, ctx) {
        let reply = match proto::parse_request(&payload) {
            Err(_) => return,
            Ok(req) => match answer(ctx, &mut conn, req) {
                Some(r) => r,
                None => return,
            },
        };
        if ctx.stop.load(Ordering::SeqCst) || s.write_all(&proto::frame(&reply)).is_err() {
            return;
        }
    }
}

/// The reply to one request; `None` closes the connection.
fn answer(ctx: &Ctx, conn: &mut Conn, req: Request) -> Option<Vec<u8>> {
    let failure = || Some(vec![proto::FAILURE]);
    match req {
        Request::Identities => {
            // Throttled before any vault work.
            if !limiter().allow_at(&format!("ids:{}", ctx.tag), IDS_PER_SECOND, IDS_PER_MINUTE, Instant::now()) {
                return failure();
            }
            let Ok(v) = super::get() else { return failure() };
            match v.agent_identities(&ctx.who) {
                Ok(ids) => Some(proto::identities_answer(&ids.into_iter().map(|i| (i.public_blob, i.name)).collect::<Vec<_>>())),
                Err(_) => failure(),
            }
        }
        Request::Sign { key_blob, data, flags } => Some(sign(ctx, conn, &key_blob, &data, flags)),
        Request::SessionBind(b) => {
            conn.binds += 1;
            if conn.binds > MAX_BINDS {
                return None;
            }
            match b.check() {
                BindCheck::Verified(fp) => {
                    conn.forwarded |= b.forwarding;
                    conn.host_fp = Some(fp);
                    Some(vec![proto::SUCCESS])
                }
                BindCheck::Unverifiable(fp) => {
                    // A host key we can't check (a host certificate, an
                    // unknown type): the forwarding flag still counts, and
                    // the connection keeps serving.
                    conn.forwarded |= b.forwarding;
                    conn.host_fp = Some(format!("{fp}?"));
                    failure()
                }
                BindCheck::Bad => None,
            }
        }
        Request::Extension(_) | Request::Refused(_) => failure(),
    }
}

/// `true` when a key blob is RSA and the request asks for SHA-1.
fn rsa_sha1(key_blob: &[u8], flags: u32) -> bool {
    let is_rsa = key_blob.len() >= 11 && key_blob[..4] == 7u32.to_be_bytes() && &key_blob[4..11] == b"ssh-rsa";
    is_rsa && flags & (proto::RSA_SHA2_256 | proto::RSA_SHA2_512) == 0
}

fn sign(ctx: &Ctx, conn: &Conn, key_blob: &[u8], data: &[u8], flags: u32) -> Vec<u8> {
    let failure = vec![proto::FAILURE];
    let now = Instant::now();
    let refused_key = format!("refused:{}", ctx.tag);
    // A session that keeps getting refused is refused without vault work.
    if !limiter().peek_at(&refused_key, REFUSALS_PER_SECOND, REFUSALS_PER_MINUTE, now) {
        return failure;
    }
    let Ok(v) = super::get() else { return failure };
    let host = conn.host_fp.as_deref();
    let refuse = |secret: Option<&str>, code: &str| {
        limiter().allow_at(&refused_key, REFUSALS_PER_SECOND, REFUSALS_PER_MINUTE, Instant::now());
        v.agent_note(secret, None, &ctx.who, &format!("deny:{code}"), host);
        vec![proto::FAILURE]
    };
    let ids = match v.agent_identities(&ctx.who) {
        Ok(i) => i,
        Err(_) => return failure,
    };
    let Some(id) = ids.into_iter().find(|i| i.public_blob == key_blob) else {
        return refuse(None, "not_granted");
    };
    if rsa_sha1(key_blob, flags) {
        return refuse(Some(&id.secret_id), "ssh_rsa_sha1");
    }
    // The rate check comes before the policy read, with the limits last
    // read for this secret (the defaults the first time).
    let key = format!("{}/{}", ctx.tag, id.secret_id);
    let d = k2_vault::vault::AgentPolicy::default();
    let (b0, m0) = ctx.limits.lock().get(&id.secret_id).copied().unwrap_or((d.burst as usize, d.per_minute as usize));
    if !limiter().peek_at(&key, b0, m0, now) {
        return refuse(Some(&id.secret_id), "rate_limited");
    }
    let Ok(policy) = v.agent_policy(&id.secret_id) else { return failure };
    let (burst, per_minute) = (policy.burst as usize, policy.per_minute as usize);
    ctx.limits.lock().insert(id.secret_id.clone(), (burst, per_minute));
    if conn.forwarded && !policy.allow_forwarded {
        return refuse(Some(&id.secret_id), "forwarded");
    }
    if !limiter().allow_at(&key, burst, per_minute, now) {
        return refuse(Some(&id.secret_id), "rate_limited");
    }
    match v.agent_sign(&id.secret_id, &ctx.who, key_blob, data, flags, host) {
        Ok(sig) => proto::sign_response(&sig),
        Err(_) => {
            // agent_sign audited the refusal itself.
            limiter().allow_at(&refused_key, REFUSALS_PER_SECOND, REFUSALS_PER_MINUTE, Instant::now());
            failure
        }
    }
}

#[cfg(test)]
#[path = "ssh_agent_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "ssh_agent_live_tests.rs"]
mod live_tests;
