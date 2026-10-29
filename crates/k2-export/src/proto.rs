//! The worker's wire (vs-live §3.2): stdin/stdout, no port, no token.
//!
//! **Worker → host, stdout:** frames `[tag u8][len u32 BE][payload]`, a
//! payload at most [`MAX_FRAME`] bytes.
//! - `H` hello `{proto, version, exporters}`, sent first;
//! - `P` progress, 16 bytes: `done u64 BE`, `total u64 BE` (at most 4 a second);
//! - `D` output bytes;
//! - `R` the final report (JSON): an export's `ExportReport`, or `describe`'s targets;
//! - `E` the final error: the kit's `ErrorReport` JSON (`id, exit, message, next, rule, details`).
//!
//! Exactly one `R` or `E`, then exit [`EXIT_OK`] / [`EXIT_ERROR`]. Anything
//! else (EOF without a final frame, an unknown tag, an oversized frame, a
//! frame after the final one) is a protocol breach: the host kills the
//! worker and reports `MPC-E007`.
//!
//! **Host → worker, stdin:** one JSON header line ([`Header`]), then exactly
//! `source.size` raw input bytes, then EOF.

use std::io::{self, Read, Write};

/// The protocol version. The host refuses a worker whose hello differs.
pub const PROTO: u64 = 1;

/// Largest frame payload.
pub const MAX_FRAME: usize = 1 << 20;

/// Largest header line.
pub const MAX_HEADER: usize = 64 << 10;

pub const TAG_HELLO: u8 = b'H';
pub const TAG_PROGRESS: u8 = b'P';
pub const TAG_DATA: u8 = b'D';
pub const TAG_REPORT: u8 = b'R';
pub const TAG_ERROR: u8 = b'E';

/// The worker's exit after an `R` frame.
pub const EXIT_OK: i32 = 0;
/// The worker's exit after an `E` frame.
pub const EXIT_ERROR: i32 = 3;

/// The worker binary's name (beside the daemon, in `~/.k2/bin/export/`).
pub const BINARY_NAME: &str = "k2-export";

/// The worker's version line (its own, like k2-ptyd's).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// One frame's bytes.
pub fn frame(tag: u8, payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(5 + payload.len());
    v.push(tag);
    v.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    v.extend_from_slice(payload);
    v
}

/// Write one frame (a payload over [`MAX_FRAME`] is split by the caller).
pub fn write_frame(w: &mut dyn Write, tag: u8, payload: &[u8]) -> io::Result<()> {
    debug_assert!(payload.len() <= MAX_FRAME);
    w.write_all(&[tag])?;
    w.write_all(&(payload.len() as u32).to_be_bytes())?;
    w.write_all(payload)?;
    w.flush()
}

/// A frame read from the worker.
#[derive(Debug, PartialEq, Eq)]
pub enum Frame {
    Hello(Vec<u8>),
    Progress { done: u64, total: u64 },
    Data(Vec<u8>),
    Report(Vec<u8>),
    Error(Vec<u8>),
}

/// Why a frame couldn't be read.
#[derive(Debug)]
pub enum FrameError {
    /// End of stream before a frame started.
    Eof,
    /// A breach: an unknown tag, an oversized or truncated frame, a bad
    /// progress payload.
    Breach(String),
    Io(io::Error),
}

/// Read one frame.
pub fn read_frame(r: &mut dyn Read) -> Result<Frame, FrameError> {
    let mut head = [0u8; 5];
    let mut got = 0;
    while got < head.len() {
        match r.read(&mut head[got..]) {
            Ok(0) if got == 0 => return Err(FrameError::Eof),
            Ok(0) => return Err(FrameError::Breach("truncated frame header".into())),
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(FrameError::Io(e)),
        }
    }
    let tag = head[0];
    let len = u32::from_be_bytes([head[1], head[2], head[3], head[4]]) as usize;
    if len > MAX_FRAME {
        return Err(FrameError::Breach(format!("frame of {len} bytes (at most {MAX_FRAME})")));
    }
    if !matches!(tag, TAG_HELLO | TAG_PROGRESS | TAG_DATA | TAG_REPORT | TAG_ERROR) {
        return Err(FrameError::Breach(format!("unknown frame tag 0x{tag:02x}")));
    }
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload).map_err(|e| match e.kind() {
        io::ErrorKind::UnexpectedEof => FrameError::Breach("truncated frame".into()),
        _ => FrameError::Io(e),
    })?;
    Ok(match tag {
        TAG_HELLO => Frame::Hello(payload),
        TAG_PROGRESS => {
            if payload.len() != 16 {
                return Err(FrameError::Breach("progress frame is not 16 bytes".into()));
            }
            let done = u64::from_be_bytes(payload[..8].try_into().expect("8 bytes"));
            let total = u64::from_be_bytes(payload[8..].try_into().expect("8 bytes"));
            Frame::Progress { done, total }
        }
        TAG_DATA => Frame::Data(payload),
        TAG_REPORT => Frame::Report(payload),
        _ => Frame::Error(payload),
    })
}

/// The progress payload.
pub fn progress_payload(done: u64, total: u64) -> [u8; 16] {
    let mut p = [0u8; 16];
    p[..8].copy_from_slice(&done.to_be_bytes());
    p[8..].copy_from_slice(&total.to_be_bytes());
    p
}

/// What is exported, as the kit's `SourceKind`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// A live doc of this pack (`markdown`, `text`).
    Pack(String),
    /// A tier-0 file by its lower-case extension (`png`; empty for none).
    Format(String),
}

impl Kind {
    pub fn json(&self) -> serde_json::Value {
        match self {
            Kind::Pack(p) => serde_json::json!({ "pack": p }),
            Kind::Format(f) => serde_json::json!({ "format": f }),
        }
    }

    pub fn from_json(v: &serde_json::Value) -> Option<Kind> {
        if let Some(p) = v.get("pack").and_then(|p| p.as_str()) {
            return Some(Kind::Pack(p.to_string()));
        }
        v.get("format").and_then(|f| f.as_str()).map(|f| Kind::Format(f.to_string()))
    }

    pub fn kit(&self) -> k2_mpc_base::export::SourceKind {
        match self {
            Kind::Pack(p) => k2_mpc_base::export::SourceKind::Pack(p.clone()),
            Kind::Format(f) => k2_mpc_base::export::SourceKind::Format(f.clone()),
        }
    }
}

/// One export's limits on the wire. `memory_bytes` is the host's (the
/// worker's counting allocator); the rest are the kit's `ExportLimits`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_input_bytes: u64,
    pub max_output_bytes: u64,
    pub max_pixels: u64,
    pub max_pages: u32,
    pub max_wall_ms: u64,
    pub memory_bytes: u64,
}

impl Limits {
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "maxInputBytes": self.max_input_bytes,
            "maxOutputBytes": self.max_output_bytes,
            "maxPixels": self.max_pixels,
            "maxPages": self.max_pages,
            "maxWallMs": self.max_wall_ms,
            "memoryBytes": self.memory_bytes,
        })
    }

    pub fn from_json(v: &serde_json::Value) -> Option<Limits> {
        let u = |k: &str| v.get(k).and_then(|x| x.as_u64());
        Some(Limits {
            max_input_bytes: u("maxInputBytes")?,
            max_output_bytes: u("maxOutputBytes")?,
            max_pixels: u("maxPixels")?,
            max_pages: u32::try_from(u("maxPages")?).ok()?,
            max_wall_ms: u("maxWallMs")?,
            memory_bytes: u("memoryBytes")?,
        })
    }

    pub fn kit(&self) -> k2_mpc_base::export::ExportLimits {
        let mut l = k2_mpc_base::export::ExportLimits::default();
        l.max_input_bytes = self.max_input_bytes;
        l.max_output_bytes = self.max_output_bytes;
        l.max_pixels = self.max_pixels;
        l.max_pages = self.max_pages;
        l.max_wall_ms = self.max_wall_ms;
        l
    }
}

/// The header line the host sends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Header {
    /// The targets offered for `kind` (one `R` frame).
    Describe { kind: Kind },
    /// Export `source.size` bytes (they follow the header) of `kind` as `target`.
    Export {
        job: String,
        kind: Kind,
        target: String,
        options: Vec<(String, String)>,
        limits: Limits,
        name: String,
        size: u64,
    },
}

impl Header {
    pub fn json(&self) -> serde_json::Value {
        match self {
            Header::Describe { kind } => serde_json::json!({ "proto": PROTO, "op": "describe", "kind": kind.json() }),
            Header::Export { job, kind, target, options, limits, name, size } => {
                let opts: serde_json::Map<String, serde_json::Value> =
                    options.iter().map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone()))).collect();
                serde_json::json!({
                    "proto": PROTO,
                    "op": "export",
                    "job": job,
                    "kind": kind.json(),
                    "target": target,
                    "options": opts,
                    "limits": limits.json(),
                    "source": { "name": name, "size": size },
                })
            }
        }
    }

    /// Parse a header line. `Err` names what is wrong.
    pub fn parse(line: &[u8]) -> Result<Header, String> {
        let v: serde_json::Value = serde_json::from_slice(line).map_err(|e| format!("header is not JSON: {e}"))?;
        let proto = v.get("proto").and_then(|p| p.as_u64()).ok_or("header has no proto")?;
        if proto != PROTO {
            return Err(format!("header proto {proto}, this worker speaks {PROTO}"));
        }
        let kind = v.get("kind").and_then(Kind::from_json).ok_or("header has no kind")?;
        match v.get("op").and_then(|o| o.as_str()) {
            Some("describe") => Ok(Header::Describe { kind }),
            Some("export") => {
                let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
                let options = match v.get("options") {
                    Some(serde_json::Value::Object(m)) => m
                        .iter()
                        .map(|(k, val)| val.as_str().map(|s| (k.clone(), s.to_string())).ok_or("an option value is not a string"))
                        .collect::<Result<Vec<_>, _>>()?,
                    None | Some(serde_json::Value::Null) => Vec::new(),
                    Some(_) => return Err("options is not an object".into()),
                };
                let src = v.get("source").ok_or("header has no source")?;
                Ok(Header::Export {
                    job: s("job").ok_or("header has no job")?,
                    kind,
                    target: s("target").ok_or("header has no target")?,
                    options,
                    limits: v.get("limits").and_then(Limits::from_json).ok_or("header has no limits")?,
                    name: src.get("name").and_then(|n| n.as_str()).ok_or("source has no name")?.to_string(),
                    size: src.get("size").and_then(|n| n.as_u64()).ok_or("source has no size")?,
                })
            }
            other => Err(format!("unknown op {other:?}")),
        }
    }
}

/// A target as `describe` and `--list` show it.
pub fn target_json(exporter: &str, t: &k2_mpc_base::export::ExportTarget) -> serde_json::Value {
    use k2_mpc_base::export::{Fidelity, OptionKind};
    let fidelity = match t.fidelity {
        Fidelity::Lossless => "lossless",
        Fidelity::Lossy => "lossy",
        Fidelity::Flatten => "flatten",
        Fidelity::Render => "render",
        _ => "other",
    };
    let options: Vec<serde_json::Value> = t
        .options
        .iter()
        .map(|o| {
            let kind = match o.kind {
                OptionKind::Bool => serde_json::json!({ "type": "bool" }),
                OptionKind::Int { min, max } => serde_json::json!({ "type": "int", "min": min, "max": max }),
                OptionKind::Choice(words) => serde_json::json!({ "type": "choice", "values": words }),
                OptionKind::Text => serde_json::json!({ "type": "text" }),
                _ => serde_json::json!({ "type": "other" }),
            };
            serde_json::json!({ "name": o.name, "kind": kind, "default": o.default, "doc": o.doc })
        })
        .collect();
    serde_json::json!({
        "id": t.id,
        "exporter": exporter,
        "mediaType": t.media_type,
        "extension": t.extension,
        "fidelity": fidelity,
        "options": options,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_and_breaches_are_named() {
        let mut buf = Vec::new();
        write_frame(&mut buf, TAG_DATA, b"abc").expect("write");
        write_frame(&mut buf, TAG_PROGRESS, &progress_payload(3, 9)).expect("write");
        let mut r = &buf[..];
        assert_eq!(read_frame(&mut r).expect("data"), Frame::Data(b"abc".to_vec()));
        assert_eq!(read_frame(&mut r).expect("progress"), Frame::Progress { done: 3, total: 9 });
        assert!(matches!(read_frame(&mut r), Err(FrameError::Eof)));

        let bad = frame(b'Z', b"x");
        assert!(matches!(read_frame(&mut &bad[..]), Err(FrameError::Breach(m)) if m.contains("unknown frame tag")));
        let mut big = vec![TAG_DATA];
        big.extend_from_slice(&((MAX_FRAME as u32) + 1).to_be_bytes());
        assert!(matches!(read_frame(&mut &big[..]), Err(FrameError::Breach(m)) if m.contains("at most")));
        let short = &frame(TAG_DATA, b"abcdef")[..7];
        assert!(matches!(read_frame(&mut &short[..]), Err(FrameError::Breach(m)) if m.contains("truncated")));
    }

    #[test]
    fn headers_round_trip_and_a_wrong_proto_is_refused() {
        let h = Header::Export {
            job: "j1".into(),
            kind: Kind::Pack("markdown".into()),
            target: "as-is".into(),
            options: vec![("a".into(), "1".into())],
            limits: Limits {
                max_input_bytes: 1,
                max_output_bytes: 2,
                max_pixels: 3,
                max_pages: 4,
                max_wall_ms: 5,
                memory_bytes: 6,
            },
            name: "notes.md".into(),
            size: 7,
        };
        let line = h.json().to_string();
        assert_eq!(Header::parse(line.as_bytes()).expect("parse"), h);
        let d = Header::Describe { kind: Kind::Format("png".into()) };
        assert_eq!(Header::parse(d.json().to_string().as_bytes()).expect("parse"), d);
        let wrong = line.replace("\"proto\":1", "\"proto\":2");
        assert!(Header::parse(wrong.as_bytes()).expect_err("proto 2").contains("proto 2"));
    }
}
