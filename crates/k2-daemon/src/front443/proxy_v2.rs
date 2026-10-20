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

// ── Reading a header (0.46.3, client IP for tunnelled Apps) ──────────
//
// The front only writes PROXY v2. The tunnel's E2E listener reads it: with
// `transport.proxyProtocolVersion = "v2"` frpc puts one header in front of
// each connection it forwards, carrying the address frps saw. Who may send
// one, and what the address is worth, is decided in `crate::client_ip`.

/// Bytes before the address block: signature, ver/cmd, family, length.
pub const FIXED_LEN: usize = 16;
/// Longest address block (+ TLVs) we accept. The largest address family
/// (AF_UNIX) is 216 bytes; frpc writes no TLVs. Anything longer is refused
/// instead of being buffered.
pub const MAX_BLOCK_LEN: usize = 512;

/// What a header said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parsed {
    /// `PROXY` over TCP: the connection's real source and destination.
    Tcp { src: SocketAddr, dst: SocketAddr },
    /// `LOCAL`: the sender's own connection (a health check), no client.
    Local,
    /// `PROXY` for a family we don't route (UNSPEC, UDP, UNIX): no client.
    Other,
}

/// Why a header was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// Fewer bytes than the header says it has.
    Short,
    /// The 12-byte signature doesn't match.
    BadSignature,
    /// Version nibble is not 2.
    BadVersion(u8),
    /// Command nibble is neither LOCAL nor PROXY.
    BadCommand(u8),
    /// The address block is shorter than its family needs.
    BadLength { family: u8, len: usize },
    /// The address block is longer than [`MAX_BLOCK_LEN`].
    TooLong(usize),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::Short => write!(f, "PROXY v2 header is truncated"),
            ParseError::BadSignature => write!(f, "not a PROXY v2 signature"),
            ParseError::BadVersion(v) => write!(f, "PROXY header version {v}, want 2"),
            ParseError::BadCommand(c) => write!(f, "PROXY v2 command {c:#x} is neither LOCAL nor PROXY"),
            ParseError::BadLength { family, len } => {
                write!(f, "PROXY v2 family {family:#04x} needs more than {len} address bytes")
            }
            ParseError::TooLong(n) => write!(f, "PROXY v2 address block of {n} bytes is over {MAX_BLOCK_LEN}"),
        }
    }
}

/// Length of the address block a 16-byte prefix announces, after checking
/// the signature, version and command. The caller reads that many more
/// bytes and hands the whole header to [`parse`].
pub fn block_len(fixed: &[u8]) -> Result<usize, ParseError> {
    if fixed.len() < FIXED_LEN {
        return Err(ParseError::Short);
    }
    if fixed[..12] != SIGNATURE {
        return Err(ParseError::BadSignature);
    }
    let ver = fixed[12] >> 4;
    if ver != 2 {
        return Err(ParseError::BadVersion(ver));
    }
    let cmd = fixed[12] & 0x0F;
    if cmd > 1 {
        return Err(ParseError::BadCommand(cmd));
    }
    let len = u16::from_be_bytes([fixed[14], fixed[15]]) as usize;
    if len > MAX_BLOCK_LEN {
        return Err(ParseError::TooLong(len));
    }
    Ok(len)
}

/// Parse one complete header at the start of `buf`. Returns what it said
/// and how many bytes it took (signature through the end of the address
/// block, TLVs skipped). Bytes after it are not looked at.
pub fn parse(buf: &[u8]) -> Result<(Parsed, usize), ParseError> {
    let len = block_len(buf)?;
    let total = FIXED_LEN + len;
    if buf.len() < total {
        return Err(ParseError::Short);
    }
    let block = &buf[FIXED_LEN..total];
    if buf[12] & 0x0F == 0 {
        // LOCAL: the receiver must ignore the address block.
        return Ok((Parsed::Local, total));
    }
    let family = buf[13];
    let need = match family {
        FAM_TCP4 | 0x12 => 12,
        FAM_TCP6 | 0x22 => 36,
        0x31 | 0x32 => 216,
        _ => 0,
    };
    if len < need {
        return Err(ParseError::BadLength { family, len });
    }
    let port = |o: usize| u16::from_be_bytes([block[o], block[o + 1]]);
    let parsed = match family {
        FAM_TCP4 => {
            let ip = |o: usize| IpAddr::from([block[o], block[o + 1], block[o + 2], block[o + 3]]);
            Parsed::Tcp { src: SocketAddr::new(ip(0), port(8)), dst: SocketAddr::new(ip(4), port(10)) }
        }
        FAM_TCP6 => {
            let ip = |o: usize| {
                let mut b = [0u8; 16];
                b.copy_from_slice(&block[o..o + 16]);
                IpAddr::V6(Ipv6Addr::from(b))
            };
            Parsed::Tcp { src: SocketAddr::new(ip(0), port(32)), dst: SocketAddr::new(ip(16), port(34)) }
        }
        _ => Parsed::Other,
    };
    Ok((parsed, total))
}

#[cfg(test)]
mod parse_tests {
    use super::*;

    fn sa(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn round_trips_what_the_front_writes() {
        let h = header(sa("203.0.113.7:51000"), sa("10.0.0.2:443"));
        assert_eq!(parse(&h), Ok((Parsed::Tcp { src: sa("203.0.113.7:51000"), dst: sa("10.0.0.2:443") }, h.len())));
        let h6 = header(sa("[2001:db8::7]:51000"), sa("[2001:db8::1]:443"));
        assert_eq!(
            parse(&h6),
            Ok((Parsed::Tcp { src: sa("[2001:db8::7]:51000"), dst: sa("[2001:db8::1]:443") }, h6.len()))
        );
    }

    #[test]
    fn stops_at_the_header_and_skips_tlvs() {
        let mut h = header(sa("198.51.100.4:1000"), sa("127.0.0.1:443"));
        // Grow the block by a 5-byte TLV (type 0x04 NOOP, len 2).
        let len = u16::from_be_bytes([h[14], h[15]]) + 5;
        h[14..16].copy_from_slice(&len.to_be_bytes());
        h.extend_from_slice(&[0x04, 0x00, 0x02, 0xAA, 0xBB]);
        let total = h.len();
        h.extend_from_slice(b"\x16\x03\x01rest-is-tls");
        let (p, used) = parse(&h).unwrap();
        assert_eq!(used, total);
        assert_eq!(p, Parsed::Tcp { src: sa("198.51.100.4:1000"), dst: sa("127.0.0.1:443") });
    }

    #[test]
    fn local_and_other_families_carry_no_client() {
        assert_eq!(parse(&local_header()), Ok((Parsed::Local, 16)));
        // PROXY over UDP4.
        let mut udp = header(sa("198.51.100.4:1000"), sa("127.0.0.1:443"));
        udp[13] = 0x12;
        assert_eq!(parse(&udp).unwrap().0, Parsed::Other);
        // PROXY with UNSPEC and an empty block.
        let mut unspec = SIGNATURE.to_vec();
        unspec.extend_from_slice(&[0x21, 0x00, 0x00, 0x00]);
        assert_eq!(parse(&unspec), Ok((Parsed::Other, 16)));
    }

    #[test]
    fn malformed_headers_are_refused() {
        let good = header(sa("203.0.113.7:51000"), sa("10.0.0.2:443"));
        assert_eq!(parse(&good[..10]), Err(ParseError::Short));
        assert_eq!(parse(&good[..20]), Err(ParseError::Short), "address block cut short");
        let mut sig = good.clone();
        sig[3] ^= 0xFF;
        assert_eq!(parse(&sig), Err(ParseError::BadSignature));
        assert_eq!(parse(b"PROXY TCP4 1.2.3.4 5.6.7.8 1 2\r\n"), Err(ParseError::BadSignature), "v1 text is not v2");
        let mut ver = good.clone();
        ver[12] = 0x11;
        assert_eq!(parse(&ver), Err(ParseError::BadVersion(1)));
        let mut cmd = good.clone();
        cmd[12] = 0x2F;
        assert_eq!(parse(&cmd), Err(ParseError::BadCommand(0xF)));
        let mut short_block = good.clone();
        short_block[14..16].copy_from_slice(&4u16.to_be_bytes());
        assert_eq!(parse(&short_block), Err(ParseError::BadLength { family: 0x11, len: 4 }));
        let mut long = good.clone();
        long[14..16].copy_from_slice(&((MAX_BLOCK_LEN + 1) as u16).to_be_bytes());
        assert_eq!(parse(&long), Err(ParseError::TooLong(MAX_BLOCK_LEN + 1)));
        // A TLS ClientHello is not a header.
        assert_eq!(parse(b"\x16\x03\x01\x02\x00\x01\x00\x01\xfc\x03\x03abcdefgh"), Err(ParseError::BadSignature));
    }
}
