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
//! How (Linux): the peer of a loopback TCP connection is a socket on this
//! machine, listed in `/proc/net/tcp{,6}` with its owner's uid: the row
//! whose local address is the peer's and whose remote address is ours.
//! The uid must be in the tenant range AND name a `k2job-` user, so a
//! human whose uid happens to sit in that range is never refused. A row
//! that can't be found (the peer already gone) refuses nothing. macOS:
//! tenants share the node's uid behind Seatbelt in 0.46.3 and loopback
//! TCP is already denied to them by their profile, so this is a no-op.

use std::net::SocketAddr;

use k2_core::compute::proto::share::{TENANT_UID_FIRST, TENANT_UID_LAST, TENANT_USER_PREFIX};

/// The tenant uid owning the peer socket of a connection (local = the
/// daemon's end), when the peer is a compute tenant's job user.
pub fn tenant_peer(local: SocketAddr, peer: SocketAddr) -> Option<u32> {
    #[cfg(target_os = "linux")]
    {
        let uid = linux::socket_owner(peer, local)?;
        is_tenant_user(uid).then_some(uid)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (local, peer);
        None
    }
}

/// A uid in the tenant range whose user is a `k2job-<slot>` account.
pub fn is_tenant_user(uid: u32) -> bool {
    (TENANT_UID_FIRST..=TENANT_UID_LAST).contains(&uid) && user_name(uid).is_some_and(|n| n.starts_with(TENANT_USER_PREFIX))
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

    /// The uid owning the socket whose local address is `local` and remote
    /// address is `remote`, from one `/proc/net/tcp{,6}` text.
    pub fn owner_in(text: &str, local: SocketAddr, remote: SocketAddr) -> Option<u32> {
        let (local, remote) = (canon(local), canon(remote));
        for line in text.lines().skip(1) {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() < 8 {
                continue;
            }
            if parse_addr(f[1]) == Some(local) && parse_addr(f[2]) == Some(remote) {
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

    /// The uid owning the socket at `addr` connected to `remote`.
    pub fn socket_owner(addr: SocketAddr, remote: SocketAddr) -> Option<u32> {
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
        let text = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n   0: 0100007F:1F90 0100007F:A3BC 01 00000000:00000000 00:00000000 00000000  1000        0 1 1 0000000000000000 20 4 30 10 -1\n   1: 0100007F:A3BC 0100007F:1F90 01 00000000:00000000 00:00000000 00000000 61003        0 2 1 0000000000000000 20 4 30 10 -1\n";
        let daemon = "127.0.0.1:8080".parse().unwrap();
        let client = "127.0.0.1:41916".parse().unwrap();
        assert_eq!(owner_in(text, client, daemon), Some(61003), "the client's own row names its owner");
        assert_eq!(owner_in(text, daemon, client), Some(1000));
        assert_eq!(owner_in(text, "127.0.0.1:1".parse().unwrap(), daemon), None);
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
        assert_eq!(super::tenant_peer(local, peer), Some(uid), "the tenant's client is found");
        let mine = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let (s2, peer2) = l.accept().unwrap();
        assert_eq!(super::tenant_peer(s2.local_addr().unwrap(), peer2), None, "root's own client never is");
        drop((mine, s2, s));
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::process::Command::new("userdel").arg(name).status();
    }

    #[test]
    fn only_k2job_users_in_the_range_are_tenants() {
        assert!(!super::is_tenant_user(0));
        assert!(!super::is_tenant_user(unsafe { libc::geteuid() }), "this test's own user never is");
        assert!(!super::is_tenant_user(super::TENANT_UID_LAST + 1));
    }
}
