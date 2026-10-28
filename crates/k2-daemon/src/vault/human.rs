//! The vault socket `~/.k2/vault.sock` and the person check (VS2).
//!
//! A few vault steps are for people only: seeing and confirming the
//! recovery key, and setting the vault confirm password. In 0.45.2 the
//! K2 app has no signed channel to the daemon yet (VS1 is deferred), so
//! the person channels are:
//!
//! - a Connect login (`Channel::Login`), and
//! - `k2 vault …` from a terminal **outside K2** over this socket
//!   (`Channel::Tty`): the peer must be this user, not one of the daemon's
//!   own processes (no ancestor is the daemon; on Linux, not in the
//!   daemon's cgroup; no K2 session variables in its environment), and the
//!   CLI says its stdin/stdout are a terminal.
//!
//! Honest limit (same as the PRD's): an agent that deliberately leaves the
//! daemon's process tree (e.g. `sudo systemd-run` on a sudo box, or asking
//! macOS to open a new terminal window) gets past this. It stops a helpful
//! or tricked agent from running these steps from its own terminal.

#[cfg(unix)]
use std::collections::HashMap;
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(unix)]
use tokio::net::UnixStream;

#[cfg(unix)]
use super::actor::{Channel, VaultCaller};

pub const SOCKET_NAME: &str = "vault.sock";
#[cfg(unix)]
const MAX_HEAD: usize = 16 * 1024;
#[cfg(unix)]
const MAX_BODY: usize = 256 * 1024;

/// Env names that mark a K2 session (passport sockets and tokens).
const K2_SESSION_MARKERS: &[&str] = &["K2_HOOK_SOCK=", "K2_HOOK_TOKEN=", "K2SO_HOOK_SOCK=", "K2SO_HOOK_TOKEN=", "K2_CELL="];

pub fn socket_path(home: &Path) -> PathBuf {
    home.join(SOCKET_NAME)
}

/// Why a peer is or isn't a person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerVerdict {
    pub person: bool,
    pub reason: &'static str,
}

fn verdict(person: bool, reason: &'static str) -> PeerVerdict {
    PeerVerdict { person, reason }
}

/// Pure part of the check, for tests: the peer's ancestry (pid → … → 1),
/// its environment, and (Linux) the two cgroup paths.
pub fn judge(daemon_pid: u32, ancestry: &[u32], env: &[String], peer_cgroup: Option<&str>, daemon_cgroup: Option<&str>) -> PeerVerdict {
    judge_with(&[daemon_pid], false, ancestry, env, peer_cgroup, daemon_cgroup)
}

/// [`judge`] with every K2 process that can be an agent's ancestor.
///
/// k2-ptyd (PRD prd-k2-ptyd-v1 §8 S-10, P74, review V2): an agent hosted
/// by k2-ptyd is ptyd's child, not the daemon's — and after a daemon
/// restart its ancestry is agent → ptyd → launchd/init, with no daemon
/// pid at all. So `k2_pids` carries ptyd's pid next to the daemon's, and
/// `ptyd_ancestor` says an ancestor's executable is a `k2-ptyd` copy
/// (covers a ptyd this daemon hasn't connected to). Either one means
/// "inside K2": the person check never gets weaker on macOS, which has no
/// cgroup belt.
pub fn judge_with(
    k2_pids: &[u32],
    ptyd_ancestor: bool,
    ancestry: &[u32],
    env: &[String],
    peer_cgroup: Option<&str>,
    daemon_cgroup: Option<&str>,
) -> PeerVerdict {
    if ancestry.iter().any(|a| k2_pids.contains(a)) || ptyd_ancestor {
        return verdict(false, "runs inside K2 (a K2 terminal or agent)");
    }
    if env.iter().any(|e| K2_SESSION_MARKERS.iter().any(|m| e.starts_with(m))) {
        return verdict(false, "carries a K2 session (a K2 terminal or agent)");
    }
    if let (Some(p), Some(d)) = (peer_cgroup, daemon_cgroup) {
        let d = d.trim_end_matches('/');
        if !d.is_empty() && d != "/" && (p == d || p.starts_with(&format!("{d}/"))) {
            return verdict(false, "runs in K2's own service group");
        }
    }
    verdict(true, "a terminal outside K2")
}

#[cfg(target_os = "linux")]
mod os {
    pub fn peer(fd: i32) -> Option<(u32, u32)> {
        let mut c = libc::ucred { pid: 0, uid: 0, gid: 0 };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: valid fd, sized out-param.
        let rc = unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_PEERCRED, &mut c as *mut _ as *mut libc::c_void, &mut len) };
        (rc == 0).then_some((c.pid as u32, c.uid))
    }

    pub fn parent(pid: u32) -> Option<u32> {
        let s = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // comm may contain spaces/parens: ppid is the 2nd field after ')'.
        let rest = &s[s.rfind(')')? + 1..];
        rest.split_whitespace().nth(1)?.parse().ok()
    }

    pub fn env(pid: u32) -> Vec<String> {
        std::fs::read(format!("/proc/{pid}/environ"))
            .map(|b| b.split(|&c| c == 0).map(|x| String::from_utf8_lossy(x).into_owned()).collect())
            .unwrap_or_default()
    }

    pub fn cgroup(pid: &str) -> Option<String> {
        let s = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
        s.lines().find_map(|l| l.strip_prefix("0::").map(str::to_string))
    }
}

#[cfg(target_os = "macos")]
mod os {
    pub fn peer(fd: i32) -> Option<(u32, u32)> {
        let mut pid: libc::pid_t = 0;
        let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
        // SAFETY: valid fd, sized out-param. SOL_LOCAL = 0, LOCAL_PEERPID = 2.
        let rc = unsafe { libc::getsockopt(fd, 0, 0x002, &mut pid as *mut _ as *mut libc::c_void, &mut len) };
        if rc != 0 {
            return None;
        }
        let mut uid: libc::uid_t = 0;
        let mut gid: libc::gid_t = 0;
        // SAFETY: valid fd.
        if unsafe { libc::getpeereid(fd, &mut uid, &mut gid) } != 0 {
            return None;
        }
        Some((pid as u32, uid))
    }

    pub fn parent(pid: u32) -> Option<u32> {
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        // SAFETY: sized out-param.
        let n = unsafe {
            libc::proc_pidinfo(pid as libc::c_int, libc::PROC_PIDTBSDINFO, 0, &mut info as *mut _ as *mut libc::c_void, size)
        };
        (n == size).then_some(info.pbi_ppid)
    }

    pub fn env(pid: u32) -> Vec<String> {
        let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as libc::c_int];
        let mut size: libc::size_t = 0;
        // SAFETY: size query.
        if unsafe { libc::sysctl(mib.as_mut_ptr(), 3, std::ptr::null_mut(), &mut size, std::ptr::null_mut(), 0) } != 0 {
            return vec![];
        }
        let mut buf = vec![0u8; size];
        // SAFETY: buffer of `size` bytes.
        if unsafe { libc::sysctl(mib.as_mut_ptr(), 3, buf.as_mut_ptr() as *mut libc::c_void, &mut size, std::ptr::null_mut(), 0) } != 0 {
            return vec![];
        }
        buf.truncate(size);
        if buf.len() < 4 {
            return vec![];
        }
        let argc = i32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]).max(0) as usize;
        // exec path, NUL padding, argv[argc], then env.
        let parts: Vec<&[u8]> = buf[4..].split(|&c| c == 0).filter(|p| !p.is_empty()).collect();
        parts.iter().skip(1 + argc).map(|p| String::from_utf8_lossy(p).into_owned()).collect()
    }

    pub fn cgroup(_pid: &str) -> Option<String> {
        None
    }
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
mod os {
    pub fn peer(_fd: i32) -> Option<(u32, u32)> {
        None
    }
    pub fn parent(_pid: u32) -> Option<u32> {
        None
    }
    pub fn env(_pid: u32) -> Vec<String> {
        vec![]
    }
    pub fn cgroup(_pid: &str) -> Option<String> {
        None
    }
}

/// Judge the peer of a connected socket.
#[cfg(unix)]
pub fn check_peer(stream: &UnixStream) -> PeerVerdict {
    let (pid, uid) = match os::peer(stream.as_raw_fd()) {
        Some(p) => p,
        None => return verdict(false, "couldn't identify the caller"),
    };
    // SAFETY: geteuid has no preconditions.
    if uid != unsafe { libc::geteuid() } {
        return verdict(false, "another user");
    }
    let me = std::process::id();
    let mut ancestry = vec![pid];
    let mut cur = pid;
    for _ in 0..64 {
        match os::parent(cur) {
            Some(p) if p > 1 && p != cur => {
                ancestry.push(p);
                cur = p;
            }
            _ => break,
        }
    }
    let env = os::env(pid);
    let pc = os::cgroup(&pid.to_string());
    let dc = os::cgroup("self");
    // P74: k2-ptyd hosts agents too (it is their parent).
    let mut k2_pids = vec![me];
    if let Some(p) = k2_core::terminal::ptyd::client::global().ptyd_pid().and_then(|p| u32::try_from(p).ok()) {
        k2_pids.push(p);
    }
    let ptyd_ancestor = ancestry.iter().skip(1).any(|a| is_ptyd_exe(*a));
    judge_with(&k2_pids, ptyd_ancestor, &ancestry, &env, pc.as_deref(), dc.as_deref())
}

/// Is `pid` running a `k2-ptyd` binary (any copy: versioned, bundled)?
#[cfg(unix)]
fn is_ptyd_exe(pid: u32) -> bool {
    let Ok(p) = i32::try_from(pid) else { return false };
    k2_core::terminal::ptyd::client::exe_of(p)
        .and_then(|e| e.file_name().map(|n| n == k2_core::terminal::ptyd::client::PTYD_BINARY_NAME))
        .unwrap_or(false)
}

/// Windows: no person channel from a terminal yet (pending: the vault
/// person check over a named pipe, Windows parity). The person steps take a
/// Connect login there until then.
#[cfg(not(unix))]
pub fn serve(_home: &Path) {
    k2_core::log_debug!("[vault] no terminal person channel on Windows yet; person steps need a Connect login");
}

/// Bind `~/.k2/vault.sock` (0600) and serve vault routes on it.
#[cfg(unix)]
pub fn serve(home: &Path) {
    let path = socket_path(home);
    let _ = std::fs::remove_file(&path);
    let listener = match std::os::unix::net::UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            k2_core::log_debug!("[vault] vault.sock bind failed: {e}");
            return;
        }
    };
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)) {
            k2_core::log_debug!("[vault] vault.sock chmod failed: {e}");
            return;
        }
    }
    if listener.set_nonblocking(true).is_err() {
        return;
    }
    let l = match tokio::net::UnixListener::from_std(listener) {
        Ok(l) => l,
        Err(_) => return,
    };
    tokio::spawn(async move {
        loop {
            match l.accept().await {
                Ok((s, _)) => {
                    tokio::spawn(handle_conn(s));
                }
                Err(e) => {
                    k2_core::log_debug!("[vault] vault.sock accept: {e}");
                    break;
                }
            }
        }
    });
}

#[cfg(unix)]
async fn read_request(s: &mut UnixStream) -> Result<(String, Vec<u8>), &'static str> {
    let mut buf: Vec<u8> = Vec::with_capacity(2048);
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p;
        }
        if buf.len() > MAX_HEAD {
            return Err("431 Request Header Fields Too Large");
        }
        match s.read(&mut chunk).await {
            Ok(0) | Err(_) => return Err("400 Bad Request"),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    if crate::routes::http::request_is_chunked(&head) {
        return Err("400 Bad Request");
    }
    let len = head
        .lines()
        .find_map(|l| {
            let (n, v) = l.split_once(':')?;
            n.trim().eq_ignore_ascii_case("content-length").then(|| v.trim().parse::<usize>().ok())?
        })
        .unwrap_or(0);
    if len > MAX_BODY {
        return Err("413 Payload Too Large");
    }
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < len {
        match s.read(&mut chunk).await {
            Ok(0) | Err(_) => return Err("400 Bad Request"),
            Ok(n) => body.extend_from_slice(&chunk[..n]),
        }
    }
    body.truncate(len);
    Ok((head, body))
}

#[cfg(unix)]
async fn write(s: &mut UnixStream, status: &str, ctype: &str, body: &str) {
    let r = format!("HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    let _ = s.write_all(r.as_bytes()).await;
}

#[cfg(unix)]
async fn handle_conn(mut s: UnixStream) {
    let v = check_peer(&s);
    let (head, body) = match read_request(&mut s).await {
        Ok(x) => x,
        Err(st) => {
            write(&mut s, st, "application/json", r#"{"error":{"code":"bad_request"}}"#).await;
            return;
        }
    };
    let first = head.lines().next().unwrap_or("");
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("").to_string();
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (target.clone(), String::new()),
    };
    if !crate::vault_routes::is_route(&path) {
        write(&mut s, "404 Not Found", "application/json", r#"{"error":{"code":"not_found","hint":"vault.sock serves /cli/vault/* and /cli/migrate/secrets/* only"}}"#).await;
        return;
    }
    let is_post = method.eq_ignore_ascii_case("POST");
    if crate::vault_routes::is_post_route(&path) && !is_post {
        write(&mut s, "405 Method Not Allowed", "application/json", r#"{"error":{"code":"post_required"}}"#).await;
        return;
    }
    let says_tty = head.lines().any(|l| {
        l.split_once(':').is_some_and(|(n, v)| n.trim().eq_ignore_ascii_case("x-k2-tty") && v.trim() == "1")
    });
    let channel = if v.person && says_tty { Channel::Tty } else { Channel::Plain };
    let caller = Ok(VaultCaller { actor: k2_vault::Actor::Owner, channel, session_id: None });
    let params: HashMap<String, String> = crate::routes::http::parse_params(&target, &query);
    let not_person = (!v.person).then_some(v.reason);
    let r = tokio::task::spawn_blocking(move || crate::vault_routes::handle_with_peer(&path, caller, &body, &params, "vault.sock", not_person))
        .await
        .unwrap_or_else(|e| crate::cli_response::CliResponse::internal_error(format!("worker join: {e}")));
    write(&mut s, r.status, r.content_type, &r.body).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// P74 (V2): an agent under k2-ptyd, after a daemon restart, has NO
    /// daemon ancestor and (macOS) no cgroup — with its K2 env stripped it
    /// must still be refused, by ptyd's pid or by a `k2-ptyd` ancestor.
    #[test]
    fn judge_refuses_agents_hosted_by_ptyd() {
        let (daemon, ptyd) = (100, 200);
        // agent 500 → ptyd 200 → (launchd)
        let v = judge_with(&[daemon, ptyd], false, &[500, 200], &["HOME=/h".into()], None, None);
        assert!(!v.person, "ptyd pid in the ancestry is inside K2");
        let v = judge_with(&[daemon], true, &[500, 300], &[], None, None);
        assert!(!v.person, "an ancestor running a k2-ptyd copy is inside K2");
        assert!(judge_with(&[daemon, ptyd], false, &[500, 300], &[], None, None).person, "an outside terminal still passes");
        // Before the fix the daemon-only check let it through:
        assert!(judge(daemon, &[500, 200], &[], None, None).person);
    }

    #[test]
    fn judge_refuses_k2_processes() {
        let ok = judge(100, &[500, 400, 300], &["HOME=/h".into()], Some("/user.slice/user-1000.slice/session-3.scope"), Some("/system.slice/k2.service"));
        assert!(ok.person, "{ok:?}");
        assert!(!judge(100, &[500, 100, 1], &[], None, None).person, "daemon ancestor");
        assert!(!judge(100, &[500], &["K2_HOOK_SOCK=/x".into()], None, None).person, "K2 session env");
        assert!(!judge(100, &[500], &[], Some("/system.slice/k2.service"), Some("/system.slice/k2.service")).person, "same unit");
        assert!(!judge(100, &[500], &[], Some("/system.slice/k2.service/sub"), Some("/system.slice/k2.service")).person, "below unit");
        assert!(judge(100, &[500], &[], Some("/system.slice/k2.serviceX"), Some("/system.slice/k2.service")).person, "prefix only");
    }

    // pending Windows vault person channel (named pipe): a Unix socket pair.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_process_inside_the_daemon_tree_is_not_a_person() {
        // The test process plays the daemon: a socket pair's peer is this
        // very process, which is its own ancestor.
        let (a, _b) = UnixStream::pair().expect("pair");
        let v = check_peer(&a);
        assert!(!v.person, "{v:?}");
    }
}
