//! Splice provenance and proxy-target policy (0.46.3).
//!
//! The E2E TLS listener proxies a nested label (`<label>.<sub>.k2.dev`) to
//! the label's target on this machine. If that target is the daemon's OWN
//! main HTTP port, the spliced bytes arrive on the main listener from a
//! loopback peer with no proxy header, so the dispatcher would treat
//! internet traffic as this machine (`Ingress::Loopback`). Three layers
//! stop that:
//!
//! 1. **Provenance** ([`mark`], [`is_spliced`], [`settle_ingress`]): every
//!    upstream socket the daemon dials on behalf of a tunnel visitor is
//!    recorded by its local address until the splice ends. The main
//!    listener's dispatcher looks the peer up after the first request
//!    bytes arrive (the splicer marks before it forwards a byte, so there
//!    is no race) and serves a match as `Ingress::Tunnel`. This covers
//!    every alias (`localhost`, `[::1]`, `127.0.0.2`, `0.0.0.0`, mapped
//!    IPv6, DNS names, the box's own LAN address on a LAN-listen box).
//! 2. **Dial belt** ([`refuse_self_dial`]): after the dial, an upstream
//!    whose peer is a registered daemon port on this machine is dropped
//!    before any byte is forwarded (the visitor gets a 502).
//! 3. **Creation policy** ([`check_target`]): `k2 publish run`, hostname
//!    attach and `/cli/connect/proxy` refuse a target that is the daemon,
//!    a cloud metadata address (everyone), or anything but loopback for an
//!    agent.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Mutex;

use tokio::net::TcpStream;

use crate::routes::dispatcher::Ingress;

fn canonical(a: SocketAddr) -> SocketAddr {
    SocketAddr::new(a.ip().to_canonical(), a.port())
}

// ── daemon ports ──────────────────────────────────────────────────────

static DAEMON_PORTS: Mutex<Vec<u16>> = Mutex::new(Vec::new());

/// A registered daemon main-listener port. Dropping it unregisters (the
/// production daemon keeps it for the process life; each in-process test
/// daemon holds one).
#[derive(Debug)]
pub struct DaemonPortReg(u16);

impl Drop for DaemonPortReg {
    fn drop(&mut self) {
        let mut g = DAEMON_PORTS.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(i) = g.iter().position(|p| *p == self.0) {
            g.remove(i);
        }
    }
}

/// Record `port` as a port the daemon's main listener (the privileged,
/// loopback-trusting one) serves.
pub fn register_daemon_port(port: u16) -> DaemonPortReg {
    DAEMON_PORTS.lock().unwrap_or_else(|p| p.into_inner()).push(port);
    DaemonPortReg(port)
}

/// True when `port` is a registered daemon main-listener port.
pub fn is_daemon_port(port: u16) -> bool {
    port != 0
        && DAEMON_PORTS
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains(&port)
}

/// True when `ip` reaches this machine: loopback, unspecified (dials the
/// local host), or an address one of this machine's interfaces holds
/// (binding a UDP socket to it succeeds only then).
pub fn is_this_machine(ip: IpAddr) -> bool {
    let ip = ip.to_canonical();
    if ip.is_loopback() || ip.is_unspecified() {
        return true;
    }
    std::net::UdpSocket::bind(SocketAddr::new(ip, 0)).is_ok()
}

/// True when `addr` is the daemon's own main listener (any alias).
pub fn is_daemon_addr(addr: SocketAddr) -> bool {
    is_daemon_port(addr.port()) && is_this_machine(addr.ip())
}

// ── splice provenance ─────────────────────────────────────────────────

/// Local address of a live daemon-dialed upstream → live mark count.
static SPLICED: Mutex<Option<HashMap<SocketAddr, usize>>> = Mutex::new(None);

/// A live splice's provenance record. Hold it until the splice ends;
/// dropping it forgets the address.
#[derive(Debug)]
pub struct SpliceMark(Option<SocketAddr>);

impl Drop for SpliceMark {
    fn drop(&mut self) {
        let Some(addr) = self.0 else { return };
        let mut g = SPLICED.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(map) = g.as_mut() {
            if let Some(n) = map.get_mut(&addr) {
                *n -= 1;
                if *n == 0 {
                    map.remove(&addr);
                }
            }
        }
    }
}

/// Record `upstream` (a socket the daemon dialed for a tunnel visitor) so
/// that, should it land on the daemon's own main listener, it is served as
/// `Ingress::Tunnel`. Call BEFORE forwarding any byte.
pub fn mark(upstream: &TcpStream) -> SpliceMark {
    let Ok(local) = upstream.local_addr() else {
        return SpliceMark(None);
    };
    let local = canonical(local);
    let mut g = SPLICED.lock().unwrap_or_else(|p| p.into_inner());
    *g.get_or_insert_with(HashMap::new).entry(local).or_insert(0) += 1;
    SpliceMark(Some(local))
}

/// True when `peer` (as the main listener saw it) is the local end of a
/// live daemon-dialed splice.
pub fn is_spliced(peer: SocketAddr) -> bool {
    let peer = canonical(peer);
    SPLICED
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .is_some_and(|m| m.contains_key(&peer))
}

/// The ingress a main-listener connection really has: `Tunnel` when its
/// peer is a live splice, else `ingress` unchanged. Only `Loopback` and
/// `Lan` (the main listener's classes) are ever changed.
pub fn settle_ingress(ingress: Ingress, peer: Option<SocketAddr>) -> Ingress {
    match (ingress, peer) {
        (Ingress::Loopback | Ingress::Lan, Some(p)) if is_spliced(p) => {
            k2_core::log_debug!(
                "[daemon/splice-guard] main-listener connection from {p} is a tunnel splice — serving as tunnel ingress"
            );
            Ingress::Tunnel
        }
        _ => ingress,
    }
}

/// Dial belt: `Err` when the dialed `upstream` is connected to the
/// daemon's own main listener (or a cloud metadata address). The caller
/// drops it and answers 502; nothing has been forwarded yet.
pub fn refuse_self_dial(upstream: &TcpStream) -> Result<(), String> {
    let peer = upstream
        .peer_addr()
        .map_err(|e| format!("upstream peer address unknown: {e}"))?;
    let peer = canonical(peer);
    if is_metadata_ip(peer.ip()) {
        return Err(format!("upstream {peer} is a cloud metadata address"));
    }
    if is_daemon_port(peer.port()) {
        let same_host = upstream
            .local_addr()
            .map(|l| l.ip().to_canonical() == peer.ip())
            .unwrap_or(false);
        if same_host || is_this_machine(peer.ip()) {
            return Err(format!("upstream {peer} is this K2 daemon's own port"));
        }
    }
    Ok(())
}

// ── target policy ─────────────────────────────────────────────────────

/// Who is setting a proxy target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetCaller {
    /// An agent passport (or anyone below Admin): loopback targets only.
    Agent,
    /// The owner token or an Owner/Admin login: today's rules, minus the
    /// daemon itself and the metadata address.
    Operator,
}

impl TargetCaller {
    /// From the label-ownership caller of the current request.
    pub fn from_label_caller(c: &crate::label_owner::Caller) -> Self {
        if c.is_manager() {
            TargetCaller::Operator
        } else {
            TargetCaller::Agent
        }
    }
}

/// Why a target was refused: a stable `code` + a message for people.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetRefusal {
    pub code: &'static str,
    pub message: String,
}

impl std::fmt::Display for TargetRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

/// Cloud instance metadata (`169.254.169.254`, AWS IPv6 `fd00:ec2::254`).
pub fn is_metadata_ip(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(v4) => v4 == Ipv4Addr::new(169, 254, 169, 254),
        IpAddr::V6(v6) => v6 == Ipv6Addr::new(0xfd00, 0xec2, 0, 0, 0, 0, 0, 0x254),
    }
}

fn refusal(code: &'static str, message: impl Into<String>) -> TargetRefusal {
    TargetRefusal { code, message: message.into() }
}

/// THE rule for a proxy target (`host:port`) someone asks this daemon to
/// route public traffic to. Refuses, for everyone: an unparseable target,
/// this daemon's own port (any alias), and the cloud metadata address. For
/// an agent, also anything that is not literally `localhost` or a loopback
/// address (no LAN, no link-local, no public, no DNS names).
pub fn check_target(raw: &str, caller: TargetCaller) -> Result<(), TargetRefusal> {
    use std::net::ToSocketAddrs;
    let t = crate::upstream_dial::parse_target(raw)
        .map_err(|e| refusal("bad_target", format!("invalid target: {e}")))?;
    let literal = if crate::upstream_dial::is_localhost(&t.host) {
        Some(crate::upstream_dial::localhost_candidates(t.port))
    } else {
        t.host
            .parse::<IpAddr>()
            .ok()
            .map(|ip| vec![SocketAddr::new(ip, t.port)])
    };
    let addrs = match (literal, caller) {
        (Some(a), _) => a,
        (None, TargetCaller::Agent) => {
            return Err(refusal(
                "target_not_loopback",
                format!(
                    "agents may point a public address only at an app on this machine \
                     (localhost:<port> or 127.0.0.1:<port>); '{raw}' is not one — ask the server owner"
                ),
            ))
        }
        // An operator may name a host; check what it resolves to now (the
        // dial re-checks the daemon port on every connection).
        (None, TargetCaller::Operator) => (t.host.as_str(), t.port)
            .to_socket_addrs()
            .map(|it| it.collect())
            .unwrap_or_default(),
    };
    for a in &addrs {
        let a = canonical(*a);
        if is_metadata_ip(a.ip()) {
            return Err(refusal(
                "target_metadata",
                format!("'{raw}' is a cloud metadata address and can never be a target"),
            ));
        }
        if is_daemon_addr(a) {
            return Err(refusal(
                "target_is_daemon",
                format!(
                    "'{raw}' is this K2 server's own port; point the address at your app's port instead"
                ),
            ));
        }
        if caller == TargetCaller::Agent && !a.ip().is_loopback() {
            return Err(refusal(
                "target_not_loopback",
                format!(
                    "agents may point a public address only at an app on this machine \
                     (localhost:<port> or 127.0.0.1:<port>); '{raw}' is not loopback — ask the server owner"
                ),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(raw: &str, caller: TargetCaller) -> &'static str {
        match check_target(raw, caller) {
            Ok(()) => "ok",
            Err(r) => r.code,
        }
    }

    /// This machine's outward-facing address (the source a route to a
    /// TEST-NET address would use). A UDP connect sends nothing.
    fn own_lan_ip() -> IpAddr {
        let s = std::net::UdpSocket::bind("0.0.0.0:0").expect("bind udp");
        s.connect("192.0.2.1:9")
            .expect("this test needs an IPv4 route (to find the machine's own address)");
        let ip = s.local_addr().expect("udp local addr").ip();
        assert!(!ip.is_loopback() && !ip.is_unspecified(), "own address {ip}");
        ip
    }

    #[test]
    fn daemon_port_is_refused_under_every_alias_for_everyone() {
        // Reserve a real port number so no parallel test registers it.
        let hold = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let p = hold.local_addr().unwrap().port();
        let _reg = register_daemon_port(p);
        let lan = own_lan_ip();
        let aliases = [
            format!("127.0.0.1:{p}"),
            format!("localhost:{p}"),
            format!("LocalHost.:{p}"),
            format!("http://localhost:{p}/"),
            format!("[::1]:{p}"),
            format!("127.0.0.2:{p}"),
            format!("0.0.0.0:{p}"),
            format!("[::]:{p}"),
            format!("[::ffff:127.0.0.1]:{p}"),
            format!("{lan}:{p}"),
        ];
        for caller in [TargetCaller::Agent, TargetCaller::Operator] {
            for a in &aliases {
                assert_eq!(code(a, caller), "target_is_daemon", "{a} as {caller:?}");
            }
        }
        // Another port on the same hosts is fine.
        let other = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let q = other.local_addr().unwrap().port();
        assert_eq!(code(&format!("127.0.0.1:{q}"), TargetCaller::Agent), "ok");
        assert_eq!(code(&format!("{lan}:{q}"), TargetCaller::Operator), "ok");
    }

    #[test]
    fn agents_get_loopback_only_operators_keep_lan() {
        for ok in ["localhost:3000", "127.0.0.1:3000", "[::1]:3000", "127.0.0.9:8080"] {
            assert_eq!(code(ok, TargetCaller::Agent), "ok", "{ok}");
            assert_eq!(code(ok, TargetCaller::Operator), "ok", "{ok}");
        }
        for (t, agent_code) in [
            ("192.168.255.77:8080", "target_not_loopback"),
            ("10.20.30.40:80", "target_not_loopback"),
            ("169.254.10.20:80", "target_not_loopback"),
            ("[fe80::1]:80", "target_not_loopback"),
            ("8.8.8.8:53", "target_not_loopback"),
            ("app.example.com:3000", "target_not_loopback"),
            ("0.0.0.0:3000", "target_not_loopback"),
        ] {
            assert_eq!(code(t, TargetCaller::Agent), agent_code, "agent {t}");
        }
        // The owner/Admin keep LAN and public targets.
        assert_eq!(code("192.168.255.77:8080", TargetCaller::Operator), "ok");
        assert_eq!(code("10.20.30.40:80", TargetCaller::Operator), "ok");
        // Metadata is refused for everyone, mapped form included.
        for m in ["169.254.169.254:80", "[::ffff:169.254.169.254]:80", "[fd00:ec2::254]:80"] {
            assert_eq!(code(m, TargetCaller::Agent), "target_metadata", "agent {m}");
            assert_eq!(code(m, TargetCaller::Operator), "target_metadata", "operator {m}");
        }
        // Garbage never reaches the control plane.
        assert_eq!(code("no-port", TargetCaller::Operator), "bad_target");
        assert_eq!(code("localhost:0", TargetCaller::Agent), "bad_target");
    }

    #[tokio::test]
    async fn mark_is_live_until_dropped_and_matches_mapped_peers() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = l.local_addr().unwrap().port();
        let up = TcpStream::connect(("127.0.0.1", port)).await.expect("connect");
        let (_srv, peer) = l.accept().await.expect("accept");
        assert!(!is_spliced(peer), "unmarked socket is not a splice");
        assert_eq!(settle_ingress(Ingress::Loopback, Some(peer)), Ingress::Loopback);
        let m = mark(&up);
        assert!(is_spliced(peer));
        let mapped = SocketAddr::new(
            IpAddr::V6(Ipv4Addr::LOCALHOST.to_ipv6_mapped()),
            peer.port(),
        );
        assert!(is_spliced(mapped), "a dual-stack listener sees ::ffff:127.0.0.1");
        assert_eq!(settle_ingress(Ingress::Loopback, Some(peer)), Ingress::Tunnel);
        assert_eq!(settle_ingress(Ingress::Lan, Some(mapped)), Ingress::Tunnel);
        assert_eq!(settle_ingress(Ingress::App, Some(peer)), Ingress::App, "never changes App");
        drop(m);
        assert!(!is_spliced(peer), "forgotten once the splice ends");
    }

    #[tokio::test]
    async fn self_dial_belt_refuses_registered_daemon_port_only() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = l.local_addr().unwrap().port();
        let up = TcpStream::connect(("127.0.0.1", port)).await.expect("connect");
        assert!(refuse_self_dial(&up).is_ok(), "an app port is fine");
        let reg = register_daemon_port(port);
        let err = refuse_self_dial(&up).expect_err("daemon port");
        assert!(err.contains("own port"), "{err}");
        drop(reg);
        assert!(refuse_self_dial(&up).is_ok(), "unregistered again");
    }
}
