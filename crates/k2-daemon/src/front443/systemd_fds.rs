//! `LISTEN_FDS` pickup (systemd socket passing, `sd_listen_fds(3)`): the
//! front's :443 socket arrives as an inherited fd named
//! [`FRONT_FD_NAME`] when the root helper's `k2-front-443.socket` is wired
//! to the daemon's unit (`Sockets=` drop-in).
//!
//! [`capture`] runs once at process start, before any thread or child
//! exists (from `handoff::capture_process_context`): it takes the fd,
//! marks every passed fd close-on-exec, closes fds that aren't ours, and
//! removes `LISTEN_PID`/`LISTEN_FDS`/`LISTEN_FDNAMES` from the
//! environment, so no agent, terminal or published service inherits a
//! listening socket or a claim to one. A hand-off successor never sees
//! these variables (its predecessor removed them); it receives the socket
//! as `Role::Front443` instead.

use std::sync::{Mutex, OnceLock};

use k2_root_proto::paths::FRONT_FD_NAME;

/// First passed fd (`SD_LISTEN_FDS_START`).
const FDS_START: i32 = 3;
/// Never trust more than this many passed fds.
const MAX_FDS: usize = 32;

fn slot() -> &'static Mutex<Option<std::net::TcpListener>> {
    static S: OnceLock<Mutex<Option<std::net::TcpListener>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(None))
}

/// What systemd passed at start (for `/cli/front/status`).
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Passed {
    /// `LISTEN_PID` named this process.
    pub for_us: bool,
    pub fds: usize,
    pub names: Vec<String>,
    /// The front's socket was among them and is a listening TCP socket.
    pub front: bool,
    pub refused: Option<String>,
}

fn passed() -> &'static Mutex<Passed> {
    static P: OnceLock<Mutex<Passed>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(Passed::default()))
}

pub fn report() -> Passed {
    passed().lock().unwrap_or_else(|p| p.into_inner()).clone()
}

/// Take the systemd-passed front socket (once).
pub fn take_front() -> Option<std::net::TcpListener> {
    slot().lock().unwrap_or_else(|p| p.into_inner()).take()
}

/// Parse the three variables. Pure: `(fd count, names)` when they are for
/// `own_pid`, else `None`.
pub fn parse(pid: Option<&str>, fds: Option<&str>, names: Option<&str>, own_pid: u32) -> Option<(usize, Vec<String>)> {
    let pid: u32 = pid?.trim().parse().ok()?;
    if pid != own_pid {
        return None;
    }
    let n: usize = fds?.trim().parse().ok()?;
    if n == 0 || n > MAX_FDS {
        return None;
    }
    let names: Vec<String> = names
        .map(|s| s.split(':').map(str::to_string).collect())
        .unwrap_or_default();
    Some((n, names))
}

/// Call once at process start (single-threaded).
pub fn capture() {
    let pid = std::env::var("LISTEN_PID").ok();
    let fds = std::env::var("LISTEN_FDS").ok();
    let names = std::env::var("LISTEN_FDNAMES").ok();
    for k in ["LISTEN_PID", "LISTEN_FDS", "LISTEN_FDNAMES"] {
        std::env::remove_var(k);
    }
    #[cfg(unix)]
    {
        let Some((n, names)) = parse(pid.as_deref(), fds.as_deref(), names.as_deref(), std::process::id()) else {
            return;
        };
        let mut rep = Passed { for_us: true, fds: n, names: names.clone(), ..Default::default() };
        for i in 0..n {
            let fd = FDS_START + i as i32;
            // SAFETY: fcntl on an fd number; failure is reported, nothing else.
            unsafe {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags >= 0 {
                    libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
                }
            }
            let ours = names.get(i).map(String::as_str) == Some(FRONT_FD_NAME);
            if ours && rep.front {
                rep.refused = Some("two fds named k2-front-443".into());
                // SAFETY: an fd systemd passed us that nothing else owns.
                unsafe { libc::close(fd) };
                continue;
            }
            if ours && is_listening_stream(fd) {
                use std::os::fd::FromRawFd;
                // SAFETY: systemd passed this fd to this process; nothing
                // else owns it, and it is a listening stream socket.
                let l = unsafe { std::net::TcpListener::from_raw_fd(fd) };
                *slot().lock().unwrap_or_else(|p| p.into_inner()) = Some(l);
                rep.front = true;
            } else {
                if ours {
                    rep.refused = Some("the k2-front-443 fd is not a listening TCP socket".into());
                }
                // Not ours: never leak a passed fd into children or keep it.
                // SAFETY: an fd systemd passed us that nothing else owns.
                unsafe { libc::close(fd) };
            }
        }
        *passed().lock().unwrap_or_else(|p| p.into_inner()) = rep;
    }
    #[cfg(not(unix))]
    {
        let _ = (pid, fds, names);
    }
}

#[cfg(unix)]
fn is_listening_stream(fd: i32) -> bool {
    let get = |opt: libc::c_int| -> Option<libc::c_int> {
        let mut v: libc::c_int = 0;
        let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        // SAFETY: valid out-pointers sized for a c_int.
        let rc = unsafe {
            libc::getsockopt(fd, libc::SOL_SOCKET, opt, &mut v as *mut libc::c_int as *mut libc::c_void, &mut len)
        };
        (rc == 0).then_some(v)
    };
    if get(libc::SO_TYPE) != Some(libc::SOCK_STREAM) {
        return false;
    }
    match get(libc::SO_ACCEPTCONN) {
        // Linux (where systemd passes the socket): 1 = listening.
        Some(v) => v != 0,
        // macOS has no SO_ACCEPTCONN for getsockopt (ENOPROTOOPT): a
        // listening socket is the stream socket with no peer.
        None => {
            let mut sa: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
            let mut len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
            // SAFETY: valid out-pointers sized for sockaddr_storage.
            let rc = unsafe { libc::getpeername(fd, &mut sa as *mut _ as *mut libc::sockaddr, &mut len) };
            rc != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOTCONN)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listen_vars_are_only_for_the_named_pid() {
        assert_eq!(
            parse(Some("42"), Some("1"), Some("k2-front-443"), 42),
            Some((1, vec!["k2-front-443".to_string()]))
        );
        assert_eq!(
            parse(Some("42"), Some("2"), Some("other:k2-front-443"), 42).map(|(n, v)| (n, v[1].clone())),
            Some((2, "k2-front-443".to_string()))
        );
        assert_eq!(parse(Some("41"), Some("1"), Some("k2-front-443"), 42), None, "another process's fds");
        assert_eq!(parse(None, Some("1"), None, 42), None);
        assert_eq!(parse(Some("42"), Some("0"), None, 42), None);
        assert_eq!(parse(Some("42"), Some("999"), None, 42), None, "absurd counts refused");
        assert_eq!(parse(Some("x"), Some("1"), None, 42), None);
        assert_eq!(parse(Some("42"), Some("1"), None, 42), Some((1, vec![])), "unnamed fd: not ours");
    }

    #[cfg(unix)]
    #[test]
    fn only_a_listening_tcp_socket_counts() {
        use std::os::fd::AsRawFd;
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        assert!(is_listening_stream(l.as_raw_fd()));
        let c = std::net::TcpStream::connect(l.local_addr().unwrap()).unwrap();
        assert!(!is_listening_stream(c.as_raw_fd()), "a connected stream is not a listener");
        let u = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        assert!(!is_listening_stream(u.as_raw_fd()));
    }
}
