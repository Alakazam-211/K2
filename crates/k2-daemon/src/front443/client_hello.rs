//! A TLS ClientHello peek: the server name (SNI) without decrypting
//! anything, so the front can route a connection and then hand the SAME
//! bytes to the backend that terminates TLS.
//!
//! Real ClientHellos do not fit one segment or even one record any more:
//! Chrome's and Firefox's post-quantum key share (X25519MLKEM768, ~1.2 KB)
//! puts a ClientHello at ~1.8–2 KB, over one Ethernet MSS, and a sender
//! may fragment the handshake message across several TLS records (RFC
//! 8446 §5.1). So [`peek`] is called on everything received so far and
//! answers [`Peek::NeedMore`] until the whole handshake message is in,
//! however it was split. Bounded: [`MAX_HELLO_BYTES`] of records, and the
//! caller holds a deadline (slowloris).
//!
//! Strict where it costs nothing: the first record must be a handshake
//! record carrying a ClientHello; lengths must agree exactly; extensions
//! may not repeat; a server name must be one well-formed DNS host name.
//! Anything else is [`Peek::Bad`] and the connection is closed.

use std::collections::HashSet;

/// Most bytes of TLS records read before the ClientHello must be complete.
pub const MAX_HELLO_BYTES: usize = 16 * 1024;

/// RFC 8446 §5.1: a record's plaintext is at most 2^14 bytes.
const MAX_RECORD: usize = 16 * 1024;

const CONTENT_HANDSHAKE: u8 = 0x16;
const HANDSHAKE_CLIENT_HELLO: u8 = 0x01;
const EXT_SERVER_NAME: u16 = 0x0000;
const EXT_ALPN: u16 = 0x0010;

/// What a ClientHello said that the front cares about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    /// The `host_name` server name, lowercased; `None` when the client
    /// sent no SNI (an IP-literal URL, an old client).
    pub sni: Option<String>,
    /// ALPN protocol names offered, in order (e.g. `acme-tls/1` from an
    /// ACME TLS-ALPN-01 validator, `h2`, `http/1.1`).
    pub alpn: Vec<String>,
    /// Bytes of `buf` covered by the records that carried the ClientHello.
    pub consumed: usize,
    /// How many TLS records the ClientHello spanned.
    pub records: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Peek {
    /// Not enough bytes yet; read more (within the deadline and the cap).
    NeedMore,
    Done(Hello),
    /// The first byte is not a TLS handshake record (plain HTTP, SSH, a
    /// PROXY header from someone else…).
    NotTls,
    /// A TLS-looking stream that is not an acceptable ClientHello, or one
    /// that would need more than [`MAX_HELLO_BYTES`].
    Bad(&'static str),
}

/// Peek at everything received so far (`buf` always starts at the first
/// byte of the connection).
pub fn peek(buf: &[u8]) -> Peek {
    if buf.is_empty() {
        return Peek::NeedMore;
    }
    if buf[0] != CONTENT_HANDSHAKE {
        return Peek::NotTls;
    }
    let mut pos = 0usize;
    let mut records = 0usize;
    let mut hs: Vec<u8> = Vec::new();
    loop {
        if buf.len() < pos + 5 {
            return more_or_cap(pos + 5);
        }
        let h = &buf[pos..pos + 5];
        if h[0] != CONTENT_HANDSHAKE {
            return Peek::Bad("a non-handshake record before the ClientHello completed");
        }
        // legacy_record_version: 0x0301 in a first flight (0x0300..=0x0303 seen).
        if h[1] != 0x03 || h[2] > 0x04 {
            return Peek::Bad("record version");
        }
        let len = usize::from(u16::from_be_bytes([h[3], h[4]]));
        if len == 0 {
            return Peek::Bad("empty handshake record");
        }
        if len > MAX_RECORD {
            return Peek::Bad("record longer than 2^14");
        }
        let end = pos + 5 + len;
        if end > MAX_HELLO_BYTES {
            return Peek::Bad("ClientHello larger than the front reads");
        }
        if buf.len() < end {
            return Peek::NeedMore;
        }
        hs.extend_from_slice(&buf[pos + 5..end]);
        pos = end;
        records += 1;
        if hs.len() < 4 {
            continue;
        }
        if hs[0] != HANDSHAKE_CLIENT_HELLO {
            return Peek::Bad("first handshake message is not a ClientHello");
        }
        let body_len = (usize::from(hs[1]) << 16) | (usize::from(hs[2]) << 8) | usize::from(hs[3]);
        if 4 + body_len > MAX_HELLO_BYTES {
            return Peek::Bad("ClientHello larger than the front reads");
        }
        if hs.len() < 4 + body_len {
            continue;
        }
        return match parse_body(&hs[4..4 + body_len]) {
            Ok((sni, alpn)) => Peek::Done(Hello { sni, alpn, consumed: pos, records }),
            Err(why) => Peek::Bad(why),
        };
    }
}

fn more_or_cap(need: usize) -> Peek {
    if need > MAX_HELLO_BYTES {
        Peek::Bad("ClientHello larger than the front reads")
    } else {
        Peek::NeedMore
    }
}

/// A bounds-checked reader over the ClientHello body.
struct Cur<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Cur<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], &'static str> {
        if self.b.len() - self.at < n {
            return Err("truncated ClientHello");
        }
        let s = &self.b[self.at..self.at + n];
        self.at += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, &'static str> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, &'static str> {
        let s = self.take(2)?;
        Ok(u16::from_be_bytes([s[0], s[1]]))
    }
    fn rest(&self) -> usize {
        self.b.len() - self.at
    }
}

type Parsed = (Option<String>, Vec<String>);

fn parse_body(body: &[u8]) -> Result<Parsed, &'static str> {
    let mut c = Cur { b: body, at: 0 };
    let legacy_version = c.u16()?;
    if !(0x0300..=0x0304).contains(&legacy_version) {
        return Err("ClientHello version");
    }
    c.take(32)?; // random
    let sid = usize::from(c.u8()?);
    if sid > 32 {
        return Err("session id longer than 32");
    }
    c.take(sid)?;
    let suites = usize::from(c.u16()?);
    if suites < 2 || suites % 2 != 0 {
        return Err("cipher suites");
    }
    c.take(suites)?;
    let comp = usize::from(c.u8()?);
    if comp < 1 {
        return Err("compression methods");
    }
    c.take(comp)?;
    if c.rest() == 0 {
        // A TLS 1.2-era hello with no extensions at all: no SNI.
        return Ok((None, Vec::new()));
    }
    let ext_len = usize::from(c.u16()?);
    if ext_len != c.rest() {
        return Err("extensions length");
    }
    let mut seen = HashSet::new();
    let mut sni = None;
    let mut alpn = Vec::new();
    while c.rest() > 0 {
        let ty = c.u16()?;
        let len = usize::from(c.u16()?);
        let data = c.take(len)?;
        if !seen.insert(ty) {
            return Err("repeated extension");
        }
        match ty {
            EXT_SERVER_NAME => sni = parse_server_name(data)?,
            EXT_ALPN => alpn = parse_alpn(data)?,
            _ => {}
        }
    }
    Ok((sni, alpn))
}

/// RFC 6066 §3: a list of (name_type, name); at most one `host_name` (0).
fn parse_server_name(data: &[u8]) -> Result<Option<String>, &'static str> {
    let mut c = Cur { b: data, at: 0 };
    let list = usize::from(c.u16()?);
    if list != c.rest() || list == 0 {
        return Err("server_name list length");
    }
    let mut host = None;
    while c.rest() > 0 {
        let kind = c.u8()?;
        let len = usize::from(c.u16()?);
        let name = c.take(len)?;
        if kind != 0 {
            continue; // unknown name types are skipped (RFC 6066)
        }
        if host.is_some() {
            return Err("two host names");
        }
        host = Some(host_name(name).ok_or("invalid server name")?);
    }
    Ok(host)
}

/// A DNS host name as SNI carries it: ASCII letters, digits, `-`, `_`,
/// dot-separated labels of 1–63, at most 253 bytes, no trailing dot.
/// Lowercased for routing.
pub fn host_name(b: &[u8]) -> Option<String> {
    if b.is_empty() || b.len() > 253 {
        return None;
    }
    let s = std::str::from_utf8(b).ok()?;
    for label in s.split('.') {
        if label.is_empty() || label.len() > 63 {
            return None;
        }
        if !label.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_') {
            return None;
        }
    }
    Some(s.to_ascii_lowercase())
}

/// A minimal well-formed ClientHello (one record) naming `sni`: the
/// front's own route probe (`front443::probe_route`). Never sent anywhere
/// but through the front's socket on loopback.
pub fn synthetic(sni: &str) -> Vec<u8> {
    let name = sni.as_bytes();
    let mut ext = vec![0x00, 0x00];
    ext.extend_from_slice(&((name.len() + 5) as u16).to_be_bytes());
    ext.extend_from_slice(&((name.len() + 3) as u16).to_be_bytes());
    ext.push(0);
    ext.extend_from_slice(&(name.len() as u16).to_be_bytes());
    ext.extend_from_slice(name);
    let mut body = vec![0x03, 0x03];
    body.extend_from_slice(&[0x4b; 32]);
    body.push(0);
    body.extend_from_slice(&[0x00, 0x02, 0x13, 0x01, 0x01, 0x00]);
    body.extend_from_slice(&(ext.len() as u16).to_be_bytes());
    body.extend_from_slice(&ext);
    let mut rec = vec![CONTENT_HANDSHAKE, 0x03, 0x01];
    rec.extend_from_slice(&((body.len() + 4) as u16).to_be_bytes());
    rec.push(HANDSHAKE_CLIENT_HELLO);
    rec.extend_from_slice(&(body.len() as u32).to_be_bytes()[1..]);
    rec.extend_from_slice(&body);
    rec
}

fn parse_alpn(data: &[u8]) -> Result<Vec<String>, &'static str> {
    let mut c = Cur { b: data, at: 0 };
    let list = usize::from(c.u16()?);
    if list != c.rest() || list == 0 {
        return Err("ALPN list length");
    }
    let mut out = Vec::new();
    while c.rest() > 0 {
        let len = usize::from(c.u8()?);
        if len == 0 {
            return Err("empty ALPN name");
        }
        let p = c.take(len)?;
        out.push(String::from_utf8_lossy(p).into_owned());
    }
    Ok(out)
}
