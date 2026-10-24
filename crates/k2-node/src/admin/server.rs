//! The executor's local Unix socket (prd-k2-node-admin-v1 §5): the only
//! way in. No network door. One request per connection, one at a time.
//!
//! Framing: `u32 BE length ‖ bytes`. Request = one JSON
//! [`LocalRequest`] frame (≤ 1 MiB + 4 KiB); an `update` adds one binary
//! frame of exactly `size` bytes (≤ [`MAX_UPDATE_BYTES`]). Response = one
//! JSON [`LocalResponse`] frame.
//!
//! Peer check first (before reading a byte): `SO_PEERCRED` on Linux,
//! `getpeereid` on macOS; only root or the configured node user.

use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::time::Duration;

use k2_node_proto::admin::{code, AdminRefusal, AdminResult, MAX_COMMAND_BYTES};

use super::exec::{Executor, LocalRequest, LocalResponse};

pub const MAX_HEADER_BYTES: usize = MAX_COMMAND_BYTES + 4096;
/// Largest `k2-node` build the executor reads into memory.
pub const MAX_UPDATE_BYTES: u64 = 256 << 20;

pub fn write_frame(w: &mut impl Write, bytes: &[u8]) -> Result<(), String> {
    let n = u32::try_from(bytes.len()).map_err(|_| "frame too large".to_string())?;
    w.write_all(&n.to_be_bytes()).map_err(|e| format!("write: {e}"))?;
    w.write_all(bytes).map_err(|e| format!("write: {e}"))?;
    w.flush().map_err(|e| format!("write: {e}"))
}

pub fn read_frame(r: &mut impl Read, max: usize) -> Result<Vec<u8>, String> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len).map_err(|e| format!("read: {e}"))?;
    let n = u32::from_be_bytes(len) as usize;
    if n > max {
        return Err(format!("frame of {n} bytes is over the {max} cap"));
    }
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf).map_err(|e| format!("read: {e}"))?;
    Ok(buf)
}

/// The peer's uid.
pub fn peer_uid(s: &UnixStream) -> Result<u32, String> {
    let fd = s.as_raw_fd();
    #[cfg(target_os = "linux")]
    {
        let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: fd is a live socket; cred/len are valid out-pointers.
        let rc = unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_PEERCRED, &mut cred as *mut _ as *mut libc::c_void, &mut len) };
        if rc != 0 {
            return Err(format!("SO_PEERCRED: {}", std::io::Error::last_os_error()));
        }
        Ok(cred.uid)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let (mut uid, mut gid): (libc::uid_t, libc::gid_t) = (0, 0);
        // SAFETY: fd is a live socket; uid/gid are valid out-pointers.
        if unsafe { libc::getpeereid(fd, &mut uid, &mut gid) } != 0 {
            return Err(format!("getpeereid: {}", std::io::Error::last_os_error()));
        }
        Ok(uid)
    }
}

/// uid and primary gid of a user name.
pub fn user_ids(name: &str) -> Option<(u32, u32)> {
    let c = std::ffi::CString::new(name).ok()?;
    // SAFETY: getpwnam reads a static buffer; we copy the ids out at once.
    unsafe {
        let pw = libc::getpwnam(c.as_ptr());
        if pw.is_null() {
            return None;
        }
        Some(((*pw).pw_uid, (*pw).pw_gid))
    }
}

/// Who may connect.
#[derive(Debug, Clone)]
pub struct PeerRule {
    pub allowed_uids: Vec<u32>,
}

impl PeerRule {
    /// Root plus the node user from admin.toml (default `k2node`/`_k2node`).
    pub fn for_node_user(user: &str) -> Self {
        let mut allowed_uids = vec![0];
        if let Some((u, _)) = user_ids(user) {
            allowed_uids.push(u);
        }
        Self { allowed_uids }
    }
}

/// Serve one connection.
pub fn serve_conn(exec: &Executor, rule: &PeerRule, mut s: UnixStream) {
    let _ = s.set_read_timeout(Some(Duration::from_secs(30)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(30)));
    let reply = |s: &mut UnixStream, r: LocalResponse| {
        if let Ok(b) = serde_json::to_vec(&r) {
            let _ = write_frame(s, &b);
        }
    };
    let refusal = |code_s: &str, msg: &str| LocalResponse::Result(AdminResult::refused("", "", &AdminRefusal::new(code_s, msg)));
    let peer = match peer_uid(&s) {
        Ok(uid) if rule.allowed_uids.contains(&uid) => uid,
        Ok(_) | Err(_) => {
            let r = refusal(code::PEER_NOT_ALLOWED, "only root or the node user may talk to the executor");
            let _ = exec.audit().append(
                super::store::AuditEntry { event: "refused".into(), code: Some(code::PEER_NOT_ALLOWED.into()), ..Default::default() },
                exec.now(),
            );
            return reply(&mut s, r);
        }
    };
    let header = match read_frame(&mut s, MAX_HEADER_BYTES) {
        Ok(h) => h,
        Err(_) => return reply(&mut s, refusal(code::TOO_LARGE, "request unreadable or over 1 MiB")),
    };
    let req: LocalRequest = match serde_json::from_slice(&header) {
        Ok(r) => r,
        Err(_) => return reply(&mut s, refusal(code::MALFORMED, "request does not parse")),
    };
    if matches!(req, LocalRequest::OwnerChange { .. }) && !owner_change_peer_ok(peer) {
        // The machine's own owner change: root at the machine only, never
        // the node user (whose jobs could otherwise hand root admin away).
        let _ = exec.audit().append(
            super::store::AuditEntry { event: "refused".into(), verb: Some(k2_node_proto::share::VERB_OWNER.into()), code: Some(code::PEER_NOT_ALLOWED.into()), ..Default::default() },
            exec.now(),
        );
        return reply(&mut s, refusal(code::PEER_NOT_ALLOWED, "only root at this machine may change the owner here (sudo k2-node servers owner …)"));
    }
    if let LocalRequest::Job { job } = &req {
        // A start brings the job's stdout/stderr as SCM_RIGHTS.
        let fds = if matches!(job, super::jobs::JobOp::Start(_)) {
            match super::fdpass::recv_fds(&s) {
                Ok(f) => f,
                Err(_) => return reply(&mut s, refusal(code::MALFORMED, "a job start carries its stdout and stderr")),
            }
        } else {
            Vec::new()
        };
        let a = exec.handle_job(job, fds);
        return reply(&mut s, LocalResponse::Job(a));
    }
    let payload = if let LocalRequest::Update { size, .. } = &req {
        if *size == 0 || *size > MAX_UPDATE_BYTES {
            return reply(&mut s, refusal(code::TOO_LARGE, "build size out of range"));
        }
        let _ = s.set_read_timeout(Some(Duration::from_secs(120)));
        match read_frame(&mut s, MAX_UPDATE_BYTES as usize) {
            Ok(b) if b.len() as u64 == *size => Some(b),
            _ => return reply(&mut s, refusal(code::MALFORMED, "build bytes missing or the wrong size")),
        }
    } else {
        None
    };
    let resp = exec.handle(&req, payload.as_deref());
    reply(&mut s, resp);
}

/// May this peer send `owner_change`? Root only (tests, which run as one
/// user, also let their own uid through).
pub(crate) fn owner_change_peer_ok(uid: u32) -> bool {
    uid == 0 || (cfg!(test) && uid == unsafe { libc::geteuid() })
}

/// The listener: systemd socket activation (`LISTEN_FDS=1` for this pid),
/// launchd socket activation (macOS), or bind `path` ourselves.
pub fn listener(path: &Path, group_gid: Option<u32>) -> Result<UnixListener, String> {
    if let Some(l) = systemd_listener() {
        return Ok(l);
    }
    #[cfg(target_os = "macos")]
    if let Some(l) = super::launchd::activated_listener() {
        return Ok(l);
    }
    bind(path, group_gid)
}

fn systemd_listener() -> Option<UnixListener> {
    let pid: u32 = std::env::var("LISTEN_PID").ok()?.parse().ok()?;
    let n: u32 = std::env::var("LISTEN_FDS").ok()?.parse().ok()?;
    if pid != std::process::id() || n != 1 {
        return None;
    }
    use std::os::fd::FromRawFd;
    // systemd hands fd 3 over without FD_CLOEXEC: set it, so no child
    // (the preflight runs as the node user) inherits root's listener (A-L1 / B-L3).
    // SAFETY: fcntl on the fd systemd passed to this pid.
    unsafe { libc::fcntl(3, libc::F_SETFD, libc::FD_CLOEXEC) };
    // SAFETY: systemd passed fd 3 to this pid (checked above) and nothing
    // else owns it.
    Some(unsafe { UnixListener::from_raw_fd(3) })
}

/// Bind the socket: replace a stale *socket* only, 0660, group = the
/// node user's group.
pub fn bind(path: &Path, group_gid: Option<u32>) -> Result<UnixListener, String> {
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};
    if let Ok(m) = std::fs::symlink_metadata(path) {
        if !m.file_type().is_socket() {
            return Err(format!("{} exists and is not a socket", path.display()));
        }
        std::fs::remove_file(path).map_err(|e| format!("remove stale {}: {e}", path.display()))?;
    }
    let l = UnixListener::bind(path).map_err(|e| format!("bind {}: {e}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o660)).map_err(|e| format!("chmod {}: {e}", path.display()))?;
    if let Some(g) = group_gid {
        let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).map_err(|e| e.to_string())?;
        // SAFETY: valid C string path; uid -1 keeps the owner.
        if unsafe { libc::chown(c.as_ptr(), u32::MAX, g) } != 0 {
            return Err(format!("chown {}: {}", path.display(), std::io::Error::last_os_error()));
        }
    }
    Ok(l)
}

/// Connections served at once (V58). Past it a connection is answered
/// `busy` at once (the node retries), never queued behind a slow one.
pub const MAX_CONNS: usize = 32;

/// Serve until `stop()` says so (checked at least twice a second; the
/// connections in flight finish first).
///
/// Each connection gets its own thread (V58): one tenant's job op that
/// waits (a start polling for its unit, a status, an update upload) never
/// holds up another tenant's start. Root actions that change the node
/// (`command`, `update`, `keys`) still take the executor's `busy` lock;
/// job ops for different units run side by side.
pub fn serve(exec: &Executor, rule: &PeerRule, l: UnixListener, stop: &(dyn Fn() -> bool + Sync)) -> Result<(), String> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let live = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        loop {
            if stop() {
                break;
            }
            // Wait for a connection, waking to check `stop`.
            let mut pfd = libc::pollfd { fd: l.as_raw_fd(), events: libc::POLLIN, revents: 0 };
            // SAFETY: one valid pollfd.
            let n = unsafe { libc::poll(&mut pfd, 1, 500) };
            if n <= 0 {
                continue;
            }
            let s = match l.accept() {
                Ok((s, _)) => s,
                Err(e) => {
                    crate::nlog!("admin: accept: {e}");
                    continue;
                }
            };
            if live.load(Ordering::SeqCst) >= MAX_CONNS {
                busy(exec, s);
                continue;
            }
            live.fetch_add(1, Ordering::SeqCst);
            let live = &live;
            let spawned = std::thread::Builder::new().name("k2-admin-conn".into()).spawn_scoped(scope, move || {
                serve_conn(exec, rule, s);
                live.fetch_sub(1, Ordering::SeqCst);
            });
            if let Err(e) = spawned {
                live.fetch_sub(1, Ordering::SeqCst);
                crate::nlog!("admin: no thread for a connection: {e}");
            }
        }
    });
    Ok(())
}

/// Over [`MAX_CONNS`]: answer `busy` without reading the request.
fn busy(exec: &Executor, mut s: UnixStream) {
    let _ = s.set_write_timeout(Some(Duration::from_secs(5)));
    let r = LocalResponse::Result(AdminResult::refused("", "", &AdminRefusal::new(code::BUSY, "the executor is serving its limit of connections; try again")));
    if let Ok(b) = serde_json::to_vec(&r) {
        let _ = write_frame(&mut s, &b);
    }
    let _ = exec.audit().append(super::store::AuditEntry { event: "refused".into(), code: Some(code::BUSY.into()), ..Default::default() }, exec.now());
}

/// The executor's answer. It may answer (a refusal: not allowed, busy)
/// before reading the whole request and close, so a failed write is
/// followed by a read; only when there's no answer either is it an error.
fn answer(s: &mut UnixStream, sent: Result<(), String>) -> Result<Vec<u8>, String> {
    match (read_frame(s, 4 << 20), sent) {
        (Ok(r), _) => Ok(r),
        (Err(_), Err(w)) => Err(w),
        (Err(e), Ok(())) => Err(e),
    }
}

/// Node side: one request to the executor (blocking; call from
/// `spawn_blocking`).
pub fn request(socket: &Path, req: &LocalRequest, payload: Option<&[u8]>, timeout: Duration) -> Result<LocalResponse, AdminRefusal> {
    let unreachable = |e: String| AdminRefusal::new(code::EXECUTOR_UNREACHABLE, e);
    let mut s = UnixStream::connect(socket).map_err(|e| unreachable(format!("{}: {e}", socket.display())))?;
    let _ = s.set_read_timeout(Some(timeout));
    let _ = s.set_write_timeout(Some(timeout));
    let h = serde_json::to_vec(req).map_err(|e| AdminRefusal::new(code::MALFORMED, e.to_string()))?;
    let sent = write_frame(&mut s, &h).and_then(|_| payload.map_or(Ok(()), |p| write_frame(&mut s, p)));
    let r = answer(&mut s, sent).map_err(unreachable)?;
    serde_json::from_slice(&r).map_err(|e| unreachable(format!("executor answer unreadable: {e}")))
}

/// Node side: one job-wall op, with fds for a start (blocking).
pub fn job_request(socket: &Path, op: &super::jobs::JobOp, fds: &[std::os::fd::RawFd], timeout: Duration) -> Result<super::jobs::JobAnswer, AdminRefusal> {
    let unreachable = |e: String| AdminRefusal::new(code::EXECUTOR_UNREACHABLE, e);
    let mut s = UnixStream::connect(socket).map_err(|e| unreachable(format!("{}: {e}", socket.display())))?;
    let _ = s.set_read_timeout(Some(timeout));
    let _ = s.set_write_timeout(Some(timeout));
    let h = serde_json::to_vec(&LocalRequest::Job { job: op.clone() }).map_err(|e| AdminRefusal::new(code::MALFORMED, e.to_string()))?;
    let sent = write_frame(&mut s, &h).and_then(|_| if fds.is_empty() { Ok(()) } else { super::fdpass::send_fds(&s, fds).map_err(|e| format!("pass fds: {e}")) });
    let r = answer(&mut s, sent).map_err(unreachable)?;
    match serde_json::from_slice::<LocalResponse>(&r) {
        Ok(LocalResponse::Job(a)) => Ok(a),
        // An older executor: it doesn't know `job` and says so.
        Ok(LocalResponse::Result(res)) => Err(AdminRefusal::new(
            res.code.as_deref().unwrap_or(code::MALFORMED),
            format!("the executor doesn't know job ops (an older k2-node admin?): {}", res.message.unwrap_or_default()),
        )),
        Ok(_) => Err(AdminRefusal::new(code::MALFORMED, "unexpected executor answer")),
        Err(e) => Err(unreachable(format!("executor answer unreadable: {e}"))),
    }
}

/// Test helper: a socket path short enough for `sun_path` (macOS temp
/// dirs are too long), unique per test. Removed with the temp dir's owner.
#[cfg(test)]
pub fn short_sock(_tag_dir: &Path) -> std::path::PathBuf {
    let id = k2_node_proto::crypto::random_id().unwrap_or_default();
    std::path::PathBuf::from(format!("/tmp/k2na-{}.sock", &id[..10]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_and_cap() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"hello").unwrap();
        assert_eq!(read_frame(&mut buf.as_slice(), 10).unwrap(), b"hello");
        assert!(read_frame(&mut buf.as_slice(), 4).is_err());
        assert!(read_frame(&mut &b"\0\0"[..], 10).is_err());
    }

    #[test]
    fn peer_uid_is_ours_on_a_socketpair() {
        let (a, _b) = UnixStream::pair().unwrap();
        assert_eq!(peer_uid(&a).unwrap(), unsafe { libc::geteuid() });
    }

    /// Mixed fleet: a node updated before its executor (or an executor
    /// rolled back) gets a plain refusal for job ops, and the node then
    /// runs jobs without the wall rather than failing them.
    #[test]
    fn an_older_executor_answers_job_ops_with_a_refusal() {
        use super::super::jobs::JobOp;
        let d = crate::util::temp_dir("old-exec");
        let p = short_sock(&d);
        let l = UnixListener::bind(&p).unwrap();
        let t = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let h = read_frame(&mut s, MAX_HEADER_BYTES).unwrap();
            // What a 0.46.2 executor does with `op: job`: no such variant.
            #[derive(serde::Deserialize)]
            #[serde(tag = "op", rename_all = "snake_case")]
            #[allow(dead_code)]
            enum OldRequest {
                Status,
            }
            assert!(serde_json::from_slice::<OldRequest>(&h).is_err(), "an older executor can't parse a job op");
            let r = LocalResponse::Result(AdminResult::refused("", "", &AdminRefusal::new(code::MALFORMED, "request does not parse")));
            write_frame(&mut s, &serde_json::to_vec(&r).unwrap()).unwrap();
        });
        let e = job_request(&p, &JobOp::Ready, &[], Duration::from_secs(5)).unwrap_err();
        t.join().unwrap();
        assert_eq!(e.code, code::MALFORMED);
        assert!(e.message.contains("older"), "{}", e.message);
        let _ = std::fs::remove_file(&p);
        // Nobody listening: unreachable, never a panic.
        assert_eq!(job_request(&p, &JobOp::Ready, &[], Duration::from_secs(1)).unwrap_err().code, code::EXECUTOR_UNREACHABLE);
    }

    #[test]
    fn bind_refuses_a_non_socket() {
        let d = crate::util::temp_dir("bind");
        let p = short_sock(&d);
        std::fs::write(&p, "file").unwrap();
        assert!(bind(&p, None).is_err());
        std::fs::remove_file(&p).unwrap();
        let _l = bind(&p, None).unwrap();
        let _l2 = bind(&p, None).unwrap(); // stale socket replaced
    }
}
