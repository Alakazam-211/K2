//! Compute tenants never reach the daemon as this machine (0.46.3, review
//! A M2 interim; Linux).
//!
//! Another K2 server's jobs on a shared compute node run as that slot's
//! own user (`k2job-<slot>`, uid in [`TENANT_UID_FIRST`]..=
//! [`TENANT_UID_LAST`]) in the host's network namespace, so loopback is
//! open to them, and the main listener trusts a loopback peer as
//! `ClientTrust::Local`. Until tenant units get their own network
//! namespace (0.46.4: a per-tenant netns with a pasta egress), the main
//! listener looks up who owns the peer's socket and refuses a tenant user
//! outright, before any routing or trust decision.
//!
//! How (Linux): the peer of a connection from this machine (loopback, or
//! one of its own addresses) is a socket on this machine, listed in
//! `/proc/net/tcp{,6}` with its owner's uid: the row whose local address is
//! the peer's and whose remote address is ours. It fails closed (review A
//! follow-up): such a connection is served only when a LIVE row is found
//! (an open socket: inode ≠ 0; a closed one is printed with uid 0) and its
//! uid isn't a tenant's. Any uid in the tenant range is a tenant unless
//! the user database names it as someone other than a `k2job-` user (a
//! person whose uid happens to sit there); the owner's own job user
//! `k2job` is a system uid, outside the range. The desktop app and the
//! CLI hold their sockets open until the reply, so they always have a
//! live row. A peer on another machine (LAN) has no row here and isn't a
//! tenant. macOS: tenants share the node's uid behind Seatbelt in 0.46.3
//! and their profile already closes loopback TCP: a no-op.

use std::net::SocketAddr;

use k2_core::compute::proto::share::{TENANT_UID_FIRST, TENANT_UID_LAST, TENANT_USER_PREFIX};

/// Why a connection is refused (local = the daemon's end), or `None`
/// when it may be served: `Some` for a compute tenant's job user, and on
/// Linux for a peer on this machine whose socket can't be found live.
pub fn refuse_peer(local: SocketAddr, peer: SocketAddr) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        if !crate::splice_guard::is_this_machine(peer.ip()) {
            return None;
        }
        match linux::live_socket_owner(peer, local) {
            Some(uid) if is_tenant_user(uid) => Some(format!("compute tenant uid {uid}")),
            Some(_) => None,
            None => Some("no live socket for this peer (closed before the check)".into()),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (local, peer);
        None
    }
}

/// A uid in the tenant range, unless the user database names it as
/// someone other than a `k2job-<slot>` user (a lookup that fails counts
/// as a tenant).
pub fn is_tenant_user(uid: u32) -> bool {
    (TENANT_UID_FIRST..=TENANT_UID_LAST).contains(&uid) && user_name(uid).is_none_or(|n| n.starts_with(TENANT_USER_PREFIX))
}

fn user_name(uid: u32) -> Option<String> {
    let mut buf = vec![0 as libc::c_char; 4096];
    let mut pw: libc::passwd = unsafe { std::mem::zeroed() };
    let mut out: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: getpwuid_r with a valid buffer; the name is copied out
    // before the buffer is dropped.
    let rc = unsafe { libc::getpwuid_r(uid as libc::uid_t, &mut pw, buf.as_mut_ptr(), buf.len(), &mut out) };
    if rc != 0 || out.is_null() || pw.pw_name.is_null() {
        return None;
    }
    // SAFETY: pw_name points into `buf`, NUL-terminated by getpwuid_r.
    Some(unsafe { std::ffi::CStr::from_ptr(pw.pw_name) }.to_string_lossy().into_owned())
}

/// The refusal a tenant peer gets (never served, never `Local`).
pub fn refusal_response() -> Vec<u8> {
    let body = r#"{"error":{"code":"compute_tenant_refused","hint":"a compute job of another K2 server can't use this K2 server; it runs walled on this machine"}}"#;
    format!(
        "HTTP/1.1 403 Forbidden\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{}Connection: close\r\n\r\n{body}",
        body.len(),
        k2_core::frame_policy::CONNECT_HEADER_LINES
    )
    .into_bytes()
}

/// Parsing of `/proc/net/tcp{,6}` (pure; tested on every OS).
pub mod procnet {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

    /// `0100007F:1F90` (v4) or 32 hex digits `:PORT` (v6), as the kernel
    /// prints them: the address in host (little-endian) 32-bit words, the
    /// port as a big-endian number.
    pub fn parse_addr(s: &str) -> Option<SocketAddr> {
        let (ip, port) = s.split_once(':')?;
        let port = u16::from_str_radix(port, 16).ok()?;
        let ip = match ip.len() {
            8 => IpAddr::V4(Ipv4Addr::from(u32::from_str_radix(ip, 16).ok()?.to_le_bytes())),
            32 => {
                let mut b = [0u8; 16];
                for i in 0..4 {
                    let w = u32::from_str_radix(&ip[i * 8..i * 8 + 8], 16).ok()?;
                    b[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
                }
                IpAddr::V6(Ipv6Addr::from(b))
            }
            _ => return None,
        };
        Some(SocketAddr::new(ip.to_canonical(), port))
    }

    /// The uid owning the LIVE socket (inode ≠ 0: an open file; a closed
    /// or orphaned one prints inode 0 and uid 0) whose local address is
    /// `local` and remote address is `remote`, from one `/proc/net/tcp{,6}`
    /// text.
    pub fn owner_in(text: &str, local: SocketAddr, remote: SocketAddr) -> Option<u32> {
        let (local, remote) = (canon(local), canon(remote));
        for line in text.lines().skip(1) {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() < 10 {
                continue;
            }
            if parse_addr(f[1]) == Some(local) && parse_addr(f[2]) == Some(remote) && f[9] != "0" {
                return f[7].parse().ok();
            }
        }
        None
    }

    fn canon(a: SocketAddr) -> SocketAddr {
        SocketAddr::new(a.ip().to_canonical(), a.port())
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::net::SocketAddr;

    /// The uid owning the live socket at `addr` connected to `remote`.
    pub fn live_socket_owner(addr: SocketAddr, remote: SocketAddr) -> Option<u32> {
        ["/proc/net/tcp", "/proc/net/tcp6"]
            .iter()
            .filter_map(|p| std::fs::read_to_string(p).ok())
            .find_map(|t| super::procnet::owner_in(&t, addr, remote))
    }
}

#[cfg(test)]
mod tests {
    use super::procnet::*;

    #[test]
    fn proc_net_rows_parse_v4_and_v6() {
        assert_eq!(parse_addr("0100007F:1F90").unwrap(), "127.0.0.1:8080".parse().unwrap());
        // ::ffff:127.0.0.1 on a v6 socket reads as the v4 address.
        assert_eq!(parse_addr("0000000000000000FFFF00000100007F:A3BC").unwrap(), "127.0.0.1:41916".parse().unwrap());
        assert_eq!(parse_addr("00000000000000000000000001000000:0050").unwrap(), "[::1]:80".parse().unwrap());
        assert!(parse_addr("zz:1").is_none());
        let text = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n   0: 0100007F:1F90 0100007F:A3BC 01 00000000:00000000 00:00000000 00000000  1000        0 1 1 0000000000000000 20 4 30 10 -1\n   1: 0100007F:A3BC 0100007F:1F90 01 00000000:00000000 00:00000000 00000000 61003        0 2 1 0000000000000000 20 4 30 10 -1\n   2: 0100007F:A3BD 0100007F:1F90 07 00000000:00000000 00:00000000 00000000     0        0 0 1 0000000000000000 20 4 30 10 -1\n";
        let daemon = "127.0.0.1:8080".parse().unwrap();
        let client = "127.0.0.1:41916".parse().unwrap();
        assert_eq!(owner_in(text, client, daemon), Some(61003), "the client's own row names its owner");
        assert_eq!(owner_in(text, daemon, client), Some(1000));
        assert_eq!(owner_in(text, "127.0.0.1:1".parse().unwrap(), daemon), None);
        assert_eq!(owner_in(text, "127.0.0.1:41917".parse().unwrap(), daemon), None, "a closed socket (inode 0) is never live");
    }

    /// Linux, as root (the wall container): a loopback client running as a
    /// `k2job-` user in the tenant range is found by its socket's owner;
    /// the test's own (root) client never is.
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "root + useradd: run in the Linux container with --ignored"]
    fn a_tenant_users_loopback_client_is_found() {
        use std::io::Read;
        use std::os::unix::process::CommandExt;
        assert_eq!(unsafe { libc::geteuid() }, 0, "run as root");
        let (name, uid) = ("k2job-7", super::TENANT_UID_LAST - 1);
        let _ = std::process::Command::new("userdel").arg(name).status();
        let ok = std::process::Command::new("useradd")
            .args(["--system", "--uid", &uid.to_string(), "--user-group", "--no-create-home", "--shell", "/usr/sbin/nologin", name])
            .status()
            .unwrap();
        assert!(ok.success(), "useradd {name}");
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let mut child = std::process::Command::new("python3")
            .args(["-c", &format!("import socket,time; s=socket.create_connection(('127.0.0.1',{port})); s.sendall(b'x'); time.sleep(5)")])
            .uid(uid)
            .gid(uid)
            .spawn()
            .unwrap();
        let (mut s, peer) = l.accept().unwrap();
        let mut one = [0u8; 1];
        s.read_exact(&mut one).unwrap();
        let local = s.local_addr().unwrap();
        assert!(super::refuse_peer(local, peer).is_some_and(|w| w.contains(&uid.to_string())), "the tenant's client is refused");
        // The desktop app and the CLI: a client that holds its socket open
        // until the reply (as curl and the app do) is served.
        let mut mine = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        std::io::Write::write_all(&mut mine, b"GET / HTTP/1.1\r\n\r\n").unwrap();
        let (s2, peer2) = l.accept().unwrap();
        assert_eq!(super::refuse_peer(s2.local_addr().unwrap(), peer2), None, "root's own open client is served");
        // A client that sends and closes before the check: no live row,
        // refused (fails closed).
        let mut gone = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        std::io::Write::write_all(&mut gone, b"GET / HTTP/1.1\r\n\r\n").unwrap();
        drop(gone);
        let (s3, peer3) = l.accept().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(super::refuse_peer(s3.local_addr().unwrap(), peer3).is_some(), "a peer gone before the check is refused");
        // A uid in the range whose name can't be looked up is a tenant.
        assert!(super::is_tenant_user(super::TENANT_UID_LAST), "no user in the db at that uid: a tenant");
        drop((mine, s2, s3, s));
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::process::Command::new("userdel").arg(name).status();
    }

    /// Review A follow-up, item 4: a headless daemon whose home is outside
    /// /home and /root (so the tenant unit's ProtectHome doesn't hide it)
    /// still keeps its Unix sockets out of a tenant's reach: the cell socket
    /// is 0600 in a 0700 dir, owned by the daemon user, so a tenant uid's
    /// connect is refused (EACCES), whatever the home's path.
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "root: run in the Linux container with --ignored"]
    fn a_headless_daemons_cell_socket_is_closed_to_tenants() {
        use std::os::unix::process::CommandExt;
        assert_eq!(unsafe { libc::geteuid() }, 0, "run as root");
        let home = std::path::PathBuf::from(format!("/var/lib/k2-uds-test-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        // The test runs alone (--test-threads=1): the daemon's home is $HOME.
        std::env::set_var("HOME", &home);
        let sid = k2_core::session::SessionId::new();
        let _l = crate::cell_uds::bind_cell_socket(&sid).unwrap();
        let sock = crate::cell_uds::cell_socket_path(&sid);
        assert!(sock.starts_with(&home));
        let uid = super::TENANT_UID_LAST - 2;
        let out = std::process::Command::new("python3")
            .args(["-c", "import socket,sys\ntry:\n  s=socket.socket(socket.AF_UNIX); s.connect(sys.argv[1]); print('UDS-OK')\nexcept OSError as e: print('UDS-NO', e.errno)", &sock.display().to_string()])
            .uid(uid)
            .gid(uid)
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&out.stdout);
        assert!(said.contains("UDS-NO 13"), "a tenant uid can't reach the daemon's cell socket: {said} {}", String::from_utf8_lossy(&out.stderr));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn only_k2job_users_in_the_range_are_tenants() {
        assert!(!super::is_tenant_user(0));
        assert!(!super::is_tenant_user(unsafe { libc::geteuid() }), "this test's own user never is");
        assert!(!super::is_tenant_user(super::TENANT_UID_LAST + 1));
    }
}
