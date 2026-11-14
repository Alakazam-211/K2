//! The Unix socket between k2-node and simhost (IR-9).
//!
//! Every connection starts with one JSON line from k2-node ([`Hello`]) and
//! one JSON line back ([`Answer`]). A [`Hello::Session`] connection then
//! carries the session's plain stream messages both ways, framed as
//! `u32 BE length ‖ channel u8 ‖ payload` (length = 1 + payload): the same
//! channels as the encrypted stream (0 = control, 1… = helpers). k2-node
//! decrypts and encrypts; simhost never sees a key.
//!
//! simhost answers only its configured peer uid (`_k2node`), checked with
//! the kernel's peer credentials, never with anything the peer says.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use k2_node_proto::share::Slot;
use k2_node_proto::sim::{SimNeed, SimRuntime};
use serde::{Deserialize, Serialize};

/// Largest framed message (the stream's own limit).
pub const MAX_MESSAGE: usize = k2_node_proto::sim::stream::MAX_MESSAGE;
/// Largest hello or answer line.
pub const MAX_LINE: usize = 1 << 20;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Hello {
    /// Xcode, runtimes, the helper, disk use (`Offer.sim`).
    Facts,
    /// The Simulator user's resident memory, summed (IR-8).
    Rss,
    /// k2-node started: shut every device down (IR-7).
    Reset,
    /// A session ended: shut this workspace's devices down; data stays.
    Shutdown { slot: Slot, workspace_key: String },
    /// `sim_erase`: delete this workspace's devices, uploads and temp files.
    Erase { slot: Slot, workspace_key: String },
    /// The session's plain stream from here on.
    Session { slot: Slot, workspace_key: String, session: String },
}

impl Hello {
    pub fn op(&self) -> &'static str {
        match self {
            Hello::Facts => "facts",
            Hello::Rss => "rss",
            Hello::Reset => "reset",
            Hello::Shutdown { .. } => "shutdown",
            Hello::Erase { .. } => "erase",
            Hello::Session { .. } => "session",
        }
    }
}

/// What simhost found on this Mac (k2-node turns it into `Offer.sim`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostFacts {
    #[serde(default)]
    pub xcode: Option<String>,
    #[serde(default)]
    pub xcode_path: Option<String>,
    /// iOS runtimes with their iPhones, newest first.
    #[serde(default)]
    pub runtimes: Vec<SimRuntime>,
    /// Where device type bundles live (`plutil` may read their profiles).
    #[serde(default)]
    pub devicetype_roots: Vec<String>,
    /// The helper built with this Xcode (protocol [`k2_node_proto::sim::HELPER_PROTOCOL`]).
    #[serde(default)]
    pub helper_built: bool,
    /// Owner steps still needed (exact commands).
    #[serde(default)]
    pub needs: Vec<SimNeed>,
    /// Bytes under the Simulator user's home (IR-15).
    #[serde(default)]
    pub bytes: u64,
}

impl HostFacts {
    /// Xcode, a runtime with an iPhone, and the helper: nothing missing.
    pub fn ready(&self) -> bool {
        self.needs.is_empty() && self.xcode.is_some() && self.helper_built && self.runtimes.iter().any(|r| !r.device_types.is_empty())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facts: Option<HostFacts>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rss: Option<u64>,
    /// Devices a reset, shutdown or erase acted on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
}

impl Answer {
    pub fn ok() -> Self {
        Self { ok: true, ..Default::default() }
    }
    pub fn err(e: impl Into<String>) -> Self {
        Self { ok: false, error: Some(e.into()), ..Default::default() }
    }
}

/// One JSON line.
pub fn write_line<T: Serialize>(w: &mut impl Write, v: &T) -> std::io::Result<()> {
    let mut b = serde_json::to_vec(v).map_err(std::io::Error::other)?;
    b.push(b'\n');
    w.write_all(&b)?;
    w.flush()
}

/// One JSON line, read a byte at a time so nothing after it is consumed
/// (a session's frames follow its answer on the same socket).
pub fn read_line<T: for<'de> Deserialize<'de>>(r: &mut impl Read) -> std::io::Result<T> {
    let mut buf = Vec::new();
    let mut b = [0u8; 1];
    loop {
        if r.read(&mut b)? == 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "the socket closed before a full line"));
        }
        if b[0] == b'\n' {
            break;
        }
        buf.push(b[0]);
        if buf.len() > MAX_LINE {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "line too long"));
        }
    }
    serde_json::from_slice(&buf).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// `u32 BE (1 + payload) ‖ channel ‖ payload`.
pub fn encode_frame(channel: u8, payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(5 + payload.len());
    v.extend_from_slice(&((payload.len() + 1) as u32).to_be_bytes());
    v.push(channel);
    v.extend_from_slice(payload);
    v
}

pub fn write_frame(w: &mut impl Write, channel: u8, payload: &[u8]) -> std::io::Result<()> {
    if payload.len() + 1 > MAX_MESSAGE {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "message too large"));
    }
    w.write_all(&encode_frame(channel, payload))?;
    w.flush()
}

/// One frame; `Ok(None)` at a clean end.
pub fn read_frame(r: &mut impl Read) -> std::io::Result<Option<(u8, Vec<u8>)>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let n = u32::from_be_bytes(len) as usize;
    if n == 0 || n > MAX_MESSAGE {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("a {n}-byte message")));
    }
    let mut body = vec![0u8; n];
    r.read_exact(&mut body)?;
    let channel = body[0];
    body.remove(0);
    Ok(Some((channel, body)))
}

fn connect(socket: &Path, timeout: Duration) -> Result<UnixStream, String> {
    let s = UnixStream::connect(socket).map_err(|e| format!("the Simulator service ({}) isn't reachable: {e}", socket.display()))?;
    s.set_read_timeout(Some(timeout)).map_err(|e| e.to_string())?;
    s.set_write_timeout(Some(timeout)).map_err(|e| e.to_string())?;
    Ok(s)
}

/// One request and its answer (blocking).
pub fn call(socket: &Path, hello: &Hello, timeout: Duration) -> Result<Answer, String> {
    let mut s = connect(socket, timeout)?;
    write_line(&mut s, hello).map_err(|e| format!("simhost {}: {e}", hello.op()))?;
    let a: Answer = read_line(&mut s).map_err(|e| format!("simhost {}: {e}", hello.op()))?;
    if a.ok {
        Ok(a)
    } else {
        Err(a.error.unwrap_or_else(|| format!("simhost refused {}", hello.op())))
    }
}

/// Open a session's plain stream; the socket is blocking with no timeout.
pub fn open_session(socket: &Path, slot: Slot, workspace_key: &str, session: &str) -> Result<UnixStream, String> {
    let mut s = connect(socket, Duration::from_secs(10))?;
    let h = Hello::Session { slot, workspace_key: workspace_key.into(), session: session.into() };
    write_line(&mut s, &h).map_err(|e| format!("simhost session: {e}"))?;
    let a: Answer = read_line(&mut s).map_err(|e| format!("simhost session: {e}"))?;
    if !a.ok {
        return Err(a.error.unwrap_or_else(|| "simhost refused the session".into()));
    }
    s.set_read_timeout(None).map_err(|e| e.to_string())?;
    s.set_write_timeout(None).map_err(|e| e.to_string())?;
    Ok(s)
}

/// The peer's uid, from the kernel.
pub fn peer_uid(s: &UnixStream) -> std::io::Result<u32> {
    use std::os::fd::AsRawFd;
    let fd = s.as_raw_fd();
    #[cfg(any(target_os = "macos", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd"))]
    {
        let mut uid: libc::uid_t = 0;
        let mut gid: libc::gid_t = 0;
        // SAFETY: getpeereid writes two integers we own.
        if unsafe { libc::getpeereid(fd, &mut uid, &mut gid) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(uid)
    }
    #[cfg(target_os = "linux")]
    {
        let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: SO_PEERCRED fills a ucred we own, of the size we pass.
        let r = unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_PEERCRED, &mut cred as *mut libc::ucred as *mut libc::c_void, &mut len) };
        if r != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(cred.uid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_and_lines_round_trip() {
        let mut buf = Vec::new();
        write_frame(&mut buf, 3, b"hello").unwrap();
        write_frame(&mut buf, 0, b"").unwrap();
        let mut r = std::io::Cursor::new(buf);
        assert_eq!(read_frame(&mut r).unwrap(), Some((3, b"hello".to_vec())));
        assert_eq!(read_frame(&mut r).unwrap(), Some((0, vec![])));
        assert_eq!(read_frame(&mut r).unwrap(), None);
        let mut bad = std::io::Cursor::new(((MAX_MESSAGE + 1) as u32).to_be_bytes().to_vec());
        assert!(read_frame(&mut bad).is_err(), "oversize is refused before it's read");
        let mut l = Vec::new();
        let h = Hello::Session { slot: 1, workspace_key: "0123456789abcdef01234567".into(), session: "a".repeat(32) };
        write_line(&mut l, &h).unwrap();
        l.extend_from_slice(b"\x00\x00");
        let mut r = std::io::Cursor::new(l);
        assert_eq!(read_line::<Hello>(&mut r).unwrap(), h);
        assert_eq!(r.position(), r.get_ref().len() as u64 - 2, "nothing after the line is read");
    }

    #[test]
    fn facts_are_ready_only_with_xcode_a_runtime_and_the_helper() {
        let rt = SimRuntime {
            identifier: "com.apple.CoreSimulator.SimRuntime.iOS-26-5".into(),
            name: "iOS 26.5".into(),
            version: "26.5".into(),
            device_types: vec![k2_node_proto::sim::SimDeviceType {
                identifier: "com.apple.CoreSimulator.SimDeviceType.iPhone-17".into(),
                name: "iPhone 17".into(),
                width_pt: 402,
                height_pt: 874,
                scale_milli: 3000,
                default: true,
            }],
        };
        let full = HostFacts { xcode: Some("26.6 (17F113)".into()), runtimes: vec![rt.clone()], helper_built: true, ..Default::default() };
        assert!(full.ready());
        assert!(!HostFacts { xcode: None, ..full.clone() }.ready());
        assert!(!HostFacts { runtimes: vec![], ..full.clone() }.ready());
        assert!(!HostFacts { runtimes: vec![SimRuntime { device_types: vec![], ..rt }], ..full.clone() }.ready());
        assert!(!HostFacts { helper_built: false, ..full.clone() }.ready());
        assert!(!HostFacts { needs: vec![SimNeed { what: "x".into(), command: "y".into() }], ..full }.ready());
    }

    #[test]
    fn the_peer_uid_is_the_kernels() {
        let (a, _b) = UnixStream::pair().unwrap();
        assert_eq!(peer_uid(&a).unwrap(), unsafe { libc::getuid() });
    }
}
