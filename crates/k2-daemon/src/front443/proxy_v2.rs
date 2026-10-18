//! The PROXY protocol v2 header the front writes before the client's bytes
//! (HAProxy's spec, §2.2), so a backend behind a loopback hop still sees
//! the real client address: sew-media's `passthrough` TURN tier (required
//! there), Stalwart's `overrideProxyTrustedNetworks` listener, Caddy ≥ 2.7.
//!
//! Only the `PROXY` command over TCP is written (the front has no health
//! checks of its own), with no TLVs. A v4-mapped IPv6 address (a v4 client
//! on the dual-stack `[::]:443` socket) is written as plain TCP over IPv4;
//! if the two ends still differ in family, both go out as IPv6.

use std::net::{IpAddr, Ipv6Addr, SocketAddr};

/// The 12-byte v2 signature.
pub const SIGNATURE: [u8; 12] = [0x0D, 0x0A, 0x0D, 0x0A, 0x00, 0x0D, 0x0A, 0x51, 0x55, 0x49, 0x54, 0x0A];

/// Version 2, command PROXY.
const VER_CMD_PROXY: u8 = 0x21;
/// Version 2, command LOCAL.
const VER_CMD_LOCAL: u8 = 0x20;
/// AF_INET + STREAM.
const FAM_TCP4: u8 = 0x11;
/// AF_INET6 + STREAM.
const FAM_TCP6: u8 = 0x21;

/// `::ffff:a.b.c.d` → `a.b.c.d`; everything else unchanged.
pub fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(IpAddr::V6(v6)),
        v4 => v4,
    }
}

fn as_v6(ip: IpAddr) -> Ipv6Addr {
    match ip {
        IpAddr::V4(v4) => v4.to_ipv6_mapped(),
        IpAddr::V6(v6) => v6,
    }
}

/// `PROXY` header for a TCP connection from `src` (the client) to `dst`
/// (the address it reached: this box's :443).
pub fn header(src: SocketAddr, dst: SocketAddr) -> Vec<u8> {
    let (s, d) = (canonical(src.ip()), canonical(dst.ip()));
    let mut h = Vec::with_capacity(16 + 36);
    h.extend_from_slice(&SIGNATURE);
    h.push(VER_CMD_PROXY);
    match (s, d) {
        (IpAddr::V4(s4), IpAddr::V4(d4)) => {
            h.push(FAM_TCP4);
            h.extend_from_slice(&12u16.to_be_bytes());
            h.extend_from_slice(&s4.octets());
            h.extend_from_slice(&d4.octets());
        }
        (s, d) => {
            h.push(FAM_TCP6);
            h.extend_from_slice(&36u16.to_be_bytes());
            h.extend_from_slice(&as_v6(s).octets());
            h.extend_from_slice(&as_v6(d).octets());
        }
    }
    h.extend_from_slice(&src.port().to_be_bytes());
    h.extend_from_slice(&dst.port().to_be_bytes());
    h
}

/// `LOCAL` header (no addresses): a health check that is not a client.
/// The front never sends it on a client connection; kept for the
/// acceptance tooling and tests.
pub fn local_header() -> Vec<u8> {
    let mut h = SIGNATURE.to_vec();
    h.push(VER_CMD_LOCAL);
    h.push(0x00);
    h.extend_from_slice(&0u16.to_be_bytes());
    h
}
