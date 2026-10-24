//! `agent` delivery for `kind=ssh_key` (prd-vault-gaps-v1 item 3): one
//! ssh-agent socket per session (or `k2 job` run), served by the daemon.
//! The key never leaves K2: the socket lists the keys granted to that
//! session as `agent` and signs with them; nothing else.
//!
//! - **Where.** `<secrets dir>/a.sock` (the session's memory-backed 0700 dir,
//!   [`super::spawn`]). A path over [`MAX_SOCK_PATH`] bytes (macOS
//!   `$TMPDIR`) uses `/tmp/k2a-<uid>/<8 base62>/a.sock` (0700 dirs; a socket
//!   holds no secret bytes) and records it in `<secrets dir>/a.sock.path`.
//! - **Who may connect.** The peer's uid must be the daemon's euid
//!   (`SO_PEERCRED` / `getpeereid`); anything else is closed at accept.
//!   At most [`MAX_CONNS`] connections per socket.
//! - **What it answers.** k2-vault's `ssh_agent` subset: identities, sign
//!   (ed25519; RSA SHA-2 only), `session-bind@openssh.com`; everything else
//!   is `FAILURE`; a bad frame, an oversized frame or a bind whose host
//!   signature fails closes the connection.
//! - **Every signature** re-checks the grant in the vault (revocation,
//!   disable, expiry, rotation take effect at once), the per-secret
//!   settings (forwarded signing is refused unless the secret allows it),
//!   and the per-(session, secret) rate limit (default burst 60/s,
//!   240/min). One rolled-up `sign` audit entry per signature or refusal.
//! - **Lifecycle.** Stopped and unlinked with the secrets dir (session end,
//!   job run end, the 60 s sweep). At boot, sessions that survived in
//!   k2-ptyd keep their dir ([`super::spawn::sweep_at_boot`]); their socket
//!   is re-bound at the same path ([`resume`]), so `SSH_AUTH_SOCK` in the
//!   running session stays valid.
//!
//! Honest line: anything in the session, or any same-user process that
//! finds the socket, can sign while the session runs. Clients that don't
//! announce forwarding (no session bind) can't be told apart.

use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use k2_vault::ssh_agent::{self as proto, BindCheck, Request};
use k2_vault::Requester;
use parking_lot::Mutex;

/// The socket's name inside the secrets dir.
pub const SOCK_NAME: &str = "a.sock";
/// When the socket lives under `/tmp/k2a-<uid>/…`, its path is recorded here
/// (inside the secrets dir) so a restarted daemon re-binds the same path.
pub const SOCK_PATH_FILE: &str = "a.sock.path";
/// `sun_path` is 104 bytes on macOS, 108 on Linux; keep a margin.
pub const MAX_SOCK_PATH: usize = 100;
/// Connections one socket serves at once.
pub const MAX_CONNS: usize = 32;
/// Session binds one connection may send (OpenSSH's own agent: 16).
pub const MAX_BINDS: usize = 16;

struct Ctx {
    who: Requester,
    /// Session id (or job run id): the rate-limit key's first half.
    tag: String,
    stop: AtomicBool,
    conns: AtomicUsize,
}

struct Server {
    dir: PathBuf,
    sock: PathBuf,
    /// `/tmp/k2a-<uid>/<8>` when the short path was used.
    fallback: Option<PathBuf>,
    ctx: Arc<Ctx>,
}

static SERVERS: Mutex<Vec<Server>> = Mutex::new(Vec::new());

/// Per-(session, secret) signature limits; each call passes the secret's
/// own burst and per-minute limit.
fn limiter() -> &'static crate::vault_fetch::RateLimiter {
    static L: OnceLock<crate::vault_fetch::RateLimiter> = OnceLock::new();
    let d = k2_vault::vault::AgentPolicy::default();
    L.get_or_init(|| crate::vault_fetch::RateLimiter::new(d.burst as usize, d.per_minute as usize))
}

fn euid() -> u32 {
    unsafe { libc::geteuid() }
}

/// `/tmp/k2a-<uid>`: the short root for sockets whose dir path is too long.
pub fn fallback_root() -> PathBuf {
    PathBuf::from(format!("/tmp/k2a-{}", euid()))
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

/// Where a new socket for `dir` goes: `(socket path, fallback dir)`.
pub fn sock_path_for(dir: &Path) -> std::io::Result<(PathBuf, Option<PathBuf>)> {
    let direct = dir.join(SOCK_NAME);
    if direct.as_os_str().len() <= MAX_SOCK_PATH {
        return Ok((direct, None));
    }
    let root = fallback_root();
    ensure_private(&root)?;
    let sub = root.join(k2_vault::crypto::base62(8));
    std::fs::DirBuilder::new().mode(0o700).create(&sub)?;
    let m = std::fs::symlink_metadata(&sub)?;
    if m.file_type().is_symlink() || m.uid() != euid() || m.permissions().mode() & 0o077 != 0 {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "agent socket dir check failed"));
    }
    let sock = sub.join(SOCK_NAME);
    if sock.as_os_str().len() > MAX_SOCK_PATH {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "agent socket path too long"));
    }
    Ok((sock, Some(sub)))
}

/// A recorded fallback path must be exactly `<fallback root>/<8 base62>/a.sock`.
fn valid_fallback(p: &Path) -> Option<PathBuf> {
    let parent = p.parent()?;
    let name = parent.file_name()?.to_str()?;
    let ok = p.file_name()? == SOCK_NAME
        && parent.parent()? == fallback_root().as_path()
        && name.len() == 8
        && name.bytes().all(|b| b.is_ascii_alphanumeric());
    ok.then(|| parent.to_path_buf())
}

fn bind(sock: &Path) -> std::io::Result<UnixListener> {
    let l = UnixListener::bind(sock)?;
    std::fs::set_permissions(sock, std::fs::Permissions::from_mode(0o600))?;
    Ok(l)
}

/// Start serving `who`'s agent keys on a new socket for secrets dir `dir`.
/// Returns the socket path (the session's `SSH_AUTH_SOCK`).
pub fn start(who: Requester, tag: &str, dir: &Path) -> std::io::Result<PathBuf> {
    let (sock, fallback) = sock_path_for(dir)?;
    let l = match bind(&sock) {
        Ok(l) => l,
        Err(e) => {
            if let Some(f) = &fallback {
                let _ = std::fs::remove_dir(f);
            }
            return Err(e);
        }
    };
    if fallback.is_some() {
        let rec = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(dir.join(SOCK_PATH_FILE))
            .and_then(|mut f| f.write_all(sock.to_string_lossy().as_bytes()));
        if let Err(e) = rec {
            let _ = std::fs::remove_file(&sock);
            if let Some(f) = &fallback {
                let _ = std::fs::remove_dir(f);
            }
            return Err(e);
        }
    }
    run(l, who, tag, dir, &sock, fallback);
    Ok(sock)
}

/// Boot: re-serve the socket of a session that survived a daemon restart
/// in k2-ptyd, at the path its `SSH_AUTH_SOCK` already names. `Ok(None)`
/// when `dir` had no agent socket.
pub fn resume(who: Requester, tag: &str, dir: &Path) -> std::io::Result<Option<PathBuf>> {
    let rec = dir.join(SOCK_PATH_FILE);
    let (sock, fallback) = match std::fs::symlink_metadata(&rec) {
        Ok(m) if m.is_file() && m.uid() == euid() => {
            let raw = std::fs::read(&rec)?;
            let p = PathBuf::from(String::from_utf8(raw).map_err(|_| std::io::Error::other("bad socket record"))?);
            let parent = valid_fallback(&p).ok_or_else(|| std::io::Error::other("socket record outside the agent root"))?;
            ensure_private(&fallback_root())?;
            ensure_private(&parent)?;
            (p, Some(parent))
        }
        Ok(_) => return Err(std::io::Error::other("bad socket record")),
        Err(_) => {
            let p = dir.join(SOCK_NAME);
            if std::fs::symlink_metadata(&p).is_err() {
                return Ok(None);
            }
            (p, None)
        }
    };
    // The old daemon's socket file is still there: only ever replace our
    // own socket, never a file, link or someone else's.
    if let Ok(m) = std::fs::symlink_metadata(&sock) {
        if !m.file_type().is_socket() || m.uid() != euid() {
            return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "not our agent socket"));
        }
        std::fs::remove_file(&sock)?;
    }
    let l = bind(&sock)?;
    run(l, who, tag, dir, &sock, fallback);
    Ok(Some(sock))
}

fn run(l: UnixListener, who: Requester, tag: &str, dir: &Path, sock: &Path, fallback: Option<PathBuf>) {
    let ctx = Arc::new(Ctx { who, tag: tag.to_string(), stop: AtomicBool::new(false), conns: AtomicUsize::new(0) });
    SERVERS.lock().push(Server { dir: dir.to_path_buf(), sock: sock.to_path_buf(), fallback, ctx: ctx.clone() });
    let c2 = ctx.clone();
    let spawned = std::thread::Builder::new().name("k2-ssh-agent".into()).spawn(move || accept_loop(l, c2));
    if spawned.is_err() {
        ctx.stop.store(true, Ordering::SeqCst);
    }
    k2_core::log_debug!("[vault] ssh-agent for {} at {}", ctx.who.label(), sock.display());
}

/// The secrets dir `dir` is going: stop its agent, unlink the socket and
/// the fallback dir. Safe to call for a dir with no agent.
pub fn stop_dir(dir: &Path) {
    let gone: Vec<Server> = {
        let mut g = SERVERS.lock();
        let (gone, keep): (Vec<_>, Vec<_>) = g.drain(..).partition(|s| s.dir == dir);
        *g = keep;
        gone
    };
    for s in gone {
        s.ctx.stop.store(true, Ordering::SeqCst);
        let _ = std::fs::remove_file(&s.sock);
        if let Some(f) = &s.fallback {
            let _ = std::fs::remove_dir(f);
        }
        k2_core::log_debug!("[vault] ssh-agent for {} stopped", s.ctx.who.label());
    }
    // A dir from before a restart (the boot sweep): its short-path socket
    // isn't registered, so follow the record.
    if let Ok(raw) = std::fs::read(dir.join(SOCK_PATH_FILE)) {
        if let Some(parent) = String::from_utf8(raw).ok().map(PathBuf::from).as_deref().and_then(valid_fallback) {
            if std::fs::symlink_metadata(&parent).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink() && m.uid() == euid()) {
                let sock = parent.join(SOCK_NAME);
                if std::fs::symlink_metadata(&sock).is_ok_and(|m| m.file_type().is_socket()) {
                    let _ = std::fs::remove_file(&sock);
                }
                let _ = std::fs::remove_dir(&parent);
            }
        }
    }
}

/// Tests: what a daemon crash leaves behind. The server stops, but its
/// socket file and fallback dir stay (as after a restart under k2-ptyd).
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
            && s.set_read_timeout(Some(Duration::from_secs(1))).is_ok()
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

/// Read exactly `buf.len()` bytes; `false` on EOF, error or stop.
fn read_full(s: &mut UnixStream, buf: &mut [u8], stop: &AtomicBool) -> bool {
    let mut got = 0;
    while got < buf.len() {
        if stop.load(Ordering::SeqCst) {
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

/// One frame's payload; `None` closes the connection (EOF, stop, a zero
/// or oversized length).
fn read_frame(s: &mut UnixStream, stop: &AtomicBool) -> Option<Vec<u8>> {
    let mut len = [0u8; 4];
    if !read_full(s, &mut len, stop) {
        return None;
    }
    let n = u32::from_be_bytes(len) as usize;
    if n == 0 || n > proto::MAX_FRAME {
        return None;
    }
    let mut buf = vec![0u8; n];
    read_full(s, &mut buf, stop).then_some(buf)
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
    while let Some(payload) = read_frame(&mut s, &ctx.stop) {
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
                    // Fail closed: the forwarding flag still counts.
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
    let Ok(v) = super::get() else { return failure };
    let host = conn.host_fp.as_deref();
    let ids = match v.agent_identities(&ctx.who) {
        Ok(i) => i,
        Err(_) => return failure,
    };
    let Some(id) = ids.into_iter().find(|i| i.public_blob == key_blob) else {
        v.agent_note(None, None, &ctx.who, "deny:not_granted", host);
        return failure;
    };
    if rsa_sha1(key_blob, flags) {
        v.agent_note(Some(&id.secret_id), None, &ctx.who, "deny:ssh_rsa_sha1", host);
        return failure;
    }
    let Ok(policy) = v.agent_policy(&id.secret_id) else { return failure };
    if conn.forwarded && !policy.allow_forwarded {
        v.agent_note(Some(&id.secret_id), None, &ctx.who, "deny:forwarded", host);
        return failure;
    }
    let key = format!("{}/{}", ctx.tag, id.secret_id);
    if !limiter().allow_at(&key, policy.burst as usize, policy.per_minute as usize, Instant::now()) {
        v.agent_note(Some(&id.secret_id), None, &ctx.who, "deny:rate_limited", host);
        return failure;
    }
    match v.agent_sign(&id.secret_id, &ctx.who, key_blob, data, flags, host) {
        Ok(sig) => proto::sign_response(&sig),
        Err(_) => failure,
    }
}

#[cfg(test)]
#[path = "ssh_agent_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "ssh_agent_live_tests.rs"]
mod live_tests;
