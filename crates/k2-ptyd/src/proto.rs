//! Wire protocol (PRD §7.2–§7.4): `u32` big-endian length + one UTF-8 JSON
//! object with a `t` tag, at most [`MAX_FRAME`] bytes. Requests carry `id`,
//! responses echo it; events carry none.

use std::io::{self, Read, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// Highest protocol this build speaks.
pub const PROTO_MAX: u32 = 1;
/// Lowest protocol this build still speaks.
pub const PROTO_MIN: u32 = 1;
/// P19: a frame over 1 MiB closes the connection (`frame_too_large`).
pub const MAX_FRAME: usize = 1 << 20;
/// Max size of `meta` on `spawn` / `update_meta` (PRD §7.3).
pub const MAX_META: usize = 64 * 1024;

/// ptyd's own version (independent of the K2 release).
pub const PTYD_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Parse a ptyd version `MAJOR.MINOR.PATCH` (a `-suffix` or `+build` is
/// ignored). `None` for anything else. Used for the P65 upgrade decision
/// and the self-upgrade downgrade guard.
pub fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let core = v.trim().split(['-', '+']).next()?;
    let mut it = core.split('.');
    let a = it.next()?.parse().ok()?;
    let b = it.next()?.parse().ok()?;
    let c = it.next()?.parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    Some((a, b, c))
}

/// Connection role in `hello`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// The single control client (the daemon). Gets events.
    Owner,
    /// A per-session byte stream (after `attach`).
    Data,
}

/// `signal` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sig {
    Hup,
    Int,
    Term,
    Kill,
}

impl Sig {
    #[cfg(unix)]
    pub fn number(self) -> libc::c_int {
        match self {
            Sig::Hup => libc::SIGHUP,
            Sig::Int => libc::SIGINT,
            Sig::Term => libc::SIGTERM,
            Sig::Kill => libc::SIGKILL,
        }
    }
}

/// `signal` target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SigTarget {
    /// `killpg(pid)` only when `getpgid(pid) == pid`, else `kill(pid)` (P17).
    Group,
    /// `kill(pid)`.
    Leader,
    /// `killpg(tcgetpgrp(master))`.
    Foreground,
}

/// Client → ptyd messages. `hello` is the only one without an `id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Request {
    Hello {
        proto: u32,
        client: String,
        client_version: String,
        /// hex(HMAC-SHA256(key, nonce ‖ client_version)).
        mac: String,
        takeover: bool,
        role: Role,
    },
    Spawn {
        sid: String,
        argv: Vec<String>,
        /// The complete child environment (nothing is inherited from ptyd).
        env: Vec<(String, String)>,
        cwd: Option<String>,
        cols: u16,
        rows: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ring_bytes: Option<u64>,
        #[serde(default)]
        meta: serde_json::Value,
    },
    List,
    UpdateMeta {
        sid: String,
        meta: serde_json::Value,
    },
    Resize {
        sid: String,
        cols: u16,
        rows: u16,
    },
    Signal {
        sid: String,
        sig: Sig,
        target: SigTarget,
    },
    FgPgrp {
        sid: String,
    },
    Kill {
        sid: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        grace_ms: Option<u64>,
    },
    AckExit {
        sid: String,
    },
    HoldUnixListener {
        name: String,
        path: String,
        mode: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        owner_uid: Option<u32>,
    },
    /// The fd travels as SCM_RIGHTS on the same message.
    HoldFd {
        name: String,
    },
    TakeFd {
        name: String,
    },
    ReleaseFd {
        name: String,
    },
    Upgrade {
        exe: String,
        /// Security review F6: the sha256 (hex) the exe must have; ptyd
        /// verifies the file it opens, then selftests and execs that file.
        /// Required: an upgrade without it is refused.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sha256: Option<String>,
    },
    Shutdown {
        end_sessions: bool,
    },
    Ping,
    /// Data connections only: after `attached` the connection is raw bytes.
    Attach {
        sid: String,
        from: u64,
    },
}

/// A session's lifecycle state in `list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Running,
    Exited {
        code: Option<i32>,
        signal: Option<i32>,
        at: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub sid: String,
    pub pid: i32,
    pub state: SessionState,
    pub cols: u16,
    pub rows: u16,
    pub ring_start: u64,
    pub ring_end: u64,
    /// Unix seconds.
    pub started_at: u64,
    pub meta: serde_json::Value,
}

/// ptyd → client messages: handshake, responses (with `id`) and events
/// (without).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Reply {
    Challenge {
        nonce: String,
        ptyd_version: String,
        proto_min: u32,
        proto_max: u32,
        boot_id: String,
    },
    Welcome {
        proto: u32,
        sessions: usize,
    },
    Refused {
        code: String,
        detail: String,
    },
    Evicted,
    Spawned {
        sid: String,
        pid: i32,
    },
    Sessions {
        sessions: Vec<SessionInfo>,
    },
    Ok,
    Error {
        code: String,
        detail: String,
    },
    Pgrp {
        pgid: Option<i32>,
    },
    Pong {
        sessions: usize,
        owner_since: Option<u64>,
        /// ptyd's own `PR_GET_DUMPABLE` (Linux; `None` elsewhere). S-3/P71,
        /// reported by ptyd itself so a check never needs another process's
        /// `/proc` (hidden under `ProtectProc=invisible`). Additive.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dumpable: Option<bool>,
        /// The NAMES of ptyd's environment variables (never values; P5/S-4).
        /// Additive.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        env_keys: Vec<String>,
    },
    Attached {
        start: u64,
        end: u64,
    },
    // Events (no id).
    Exited {
        sid: String,
        code: Option<i32>,
        signal: Option<i32>,
        at: u64,
    },
    Overrun {
        sid: String,
        lost_bytes: u64,
    },
    OrphanWarning {
        seconds_left: u64,
    },
}

/// Envelope: an optional request id beside the tagged body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Frame<T> {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    #[serde(flatten)]
    pub body: T,
}

impl<T> Frame<T> {
    pub fn new(id: Option<u64>, body: T) -> Self {
        Frame { id, body }
    }
}

/// Encode one frame (length prefix + JSON).
pub fn encode<T: Serialize>(v: &T) -> io::Result<Vec<u8>> {
    let json = serde_json::to_vec(v).map_err(io::Error::other)?;
    if json.len() > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "frame_too_large"));
    }
    let mut out = Vec::with_capacity(4 + json.len());
    out.extend_from_slice(&(json.len() as u32).to_be_bytes());
    out.extend_from_slice(&json);
    Ok(out)
}

/// Blocking write of one frame.
pub fn write_frame<W: Write, T: Serialize>(w: &mut W, v: &T) -> io::Result<()> {
    w.write_all(&encode(v)?)
}

/// Blocking read of one frame.
pub fn read_frame<R: Read, T: DeserializeOwned>(r: &mut R) -> io::Result<T> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame_too_large"));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    serde_json::from_slice(&buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Errors from [`FrameDecoder::next_frame`].
#[derive(Debug, PartialEq, Eq)]
pub enum FrameError {
    TooLarge,
}

/// Incremental decoder for non-blocking sockets.
#[derive(Default)]
pub struct FrameDecoder {
    buf: Vec<u8>,
}

impl FrameDecoder {
    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// The next complete frame's JSON bytes, if one is buffered.
    pub fn next_frame(&mut self) -> Result<Option<Vec<u8>>, FrameError> {
        if self.buf.len() < 4 {
            return Ok(None);
        }
        let len = u32::from_be_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]) as usize;
        if len > MAX_FRAME {
            return Err(FrameError::TooLarge);
        }
        if self.buf.len() < 4 + len {
            return Ok(None);
        }
        let frame = self.buf[4..4 + len].to_vec();
        self.buf.drain(..4 + len);
        Ok(Some(frame))
    }

    /// Bytes buffered past the last complete frame (raw data after `attach`).
    pub fn take_rest(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_wire_shape_is_tagged_with_id_beside_it() {
        let f = Frame::new(Some(7), Request::Resize { sid: "s1".into(), cols: 80, rows: 24 });
        let v: serde_json::Value = serde_json::to_value(&f).unwrap();
        assert_eq!(v, serde_json::json!({"id":7,"t":"resize","sid":"s1","cols":80,"rows":24}));
        let back: Frame<Request> = serde_json::from_value(v).unwrap();
        assert_eq!(back, f);
    }

    #[test]
    fn event_has_no_id_and_state_shapes_match_the_prd() {
        let f = Frame::new(None, Reply::Exited { sid: "a".into(), code: Some(0), signal: None, at: 5 });
        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(v, serde_json::json!({"t":"exited","sid":"a","code":0,"signal":null,"at":5}));
        assert_eq!(serde_json::to_value(SessionState::Running).unwrap(), serde_json::json!("running"));
        assert_eq!(
            serde_json::to_value(SessionState::Exited { code: None, signal: Some(9), at: 1 }).unwrap(),
            serde_json::json!({"exited":{"code":null,"signal":9,"at":1}})
        );
    }

    #[test]
    fn versions_parse_and_order() {
        assert_eq!(parse_version("0.2.0"), Some((0, 2, 0)));
        assert_eq!(parse_version(" 1.10.3-rc1 "), Some((1, 10, 3)));
        assert!(parse_version("0.10.0") > parse_version("0.9.9"));
        for bad in ["", "1", "1.2", "1.2.3.4", "a.b.c", "1..2"] {
            assert_eq!(parse_version(bad), None, "{bad}");
        }
    }

    #[test]
    fn unknown_t_does_not_parse_as_a_request() {
        let r: Result<Frame<Request>, _> = serde_json::from_str(r#"{"id":1,"t":"nope"}"#);
        assert!(r.is_err());
    }

    #[test]
    fn decoder_splits_frames_and_rejects_oversize() {
        let mut d = FrameDecoder::default();
        let a = encode(&Frame::new(Some(1), Request::Ping)).unwrap();
        let b = encode(&Frame::new(Some(2), Request::List)).unwrap();
        let mut both = a.clone();
        both.extend_from_slice(&b);
        d.push(&both[..3]);
        assert_eq!(d.next_frame(), Ok(None));
        d.push(&both[3..]);
        assert_eq!(d.next_frame().unwrap().unwrap(), a[4..].to_vec());
        assert_eq!(d.next_frame().unwrap().unwrap(), b[4..].to_vec());
        assert_eq!(d.next_frame(), Ok(None));
        let mut big = FrameDecoder::default();
        big.push(&((MAX_FRAME as u32) + 1).to_be_bytes());
        assert_eq!(big.next_frame(), Err(FrameError::TooLarge));
    }
}
