//! The helper link, protocol 4: what a frame source process (k2-simhelper,
//! k2-deskd) and the daemon say to each other. Fix 1: the source encodes
//! and the daemon relays.
//!
//! **Framing** (both directions, unchanged since protocol 1):
//! `[u32 BE len][u8 kind][payload]`, `len` counting kind + payload.
//! `J` = one UTF-8 JSON object, `F` = one encoded unit. A reader takes
//! exactly one message's bytes, so a socket can be handed on after it.
//!
//! **Units** (`F`, protocol 4): a [`UNIT_HEADER_LEN`]-byte [`UnitHeader`],
//! then the payload: an Annex B access unit for H.264 (SPS + PPS in front of
//! every IDR, so a viewer can start on any keyframe), one VP8 frame, or one
//! baseline JPEG. All big-endian:
//!
//! ```text
//! 0      codec    u8   1 = H.264, 2 = VP8, 3 = JPEG
//! 1      flags    u8   bit 0 key (IDR / VP8 key frame / every JPEG)
//!                      bit 1 source start: the first unit after a source or
//!                            size change (always also key)
//!                      bit 2 discardable: no later unit references it
//!                      bits 3-7 zero
//! 2      layer    u8   the layer id the daemon gave in `layers`
//! 3      reserved u8   zero; readers ignore it
//! 4..8   source   u32  the display (deskd); 0 for a simulator
//! 8..12  width    u32  coded picture width in pixels
//! 12..16 height   u32
//! 16..24 seq      u64  per layer, +1 per unit; a gap = units the source dropped
//! 24..32 captured u64  ns of the source's uptime when the OS handed it the pixels
//! 32..40 emitted  u64  same clock, when the encoder gave the unit back
//! ```
//!
//! The clock is the one `hello.clock_ns` reads, so the daemon maps it onto
//! its own (as protocol 2-3 do).
//!
//! **Requests** (daemon to source, `J`): [`Request`]. Protocol 4 adds
//! `layers`, `keyframe` and `rate`; each may carry `"nr": true` (no reply on
//! success; a failure comes back as an `input_error` event, protocol 3).
//! A source keeps its own requests and events beside these (deskd's
//! `inject`, `displays`, …; the sim helper's `tree`, `touch`, …).
//!
//! **Version gate.** A source's `ready` event and `hello` reply carry
//! `version` (the newest it speaks) and `accepts` (every version it can
//! speak; a source without `accepts` speaks only `version`). The daemon
//! picks with [`negotiate`]. On a sim helper the request it sends then says
//! which: protocol 3's `frames` gets protocol 3's JPEG frames, protocol 4's
//! `layers` gets units. deskd speaks 4 only.
//!
//! **Layers.** One encode per layer, never per viewer. The daemon sends the
//! whole list each time it changes; a layer missing from the list, or with
//! `on: false`, is paused (no capture work, no output). A layer's first unit
//! after it starts, after a size change or after a source change is a key
//! unit with `source start`.
//!
//! **Keyframes** come only on a layer's start, a size or source change, and
//! a `keyframe` request: never on a timer. VideoToolbox applies a forced
//! keyframe to the next frame it takes, which can be one captured before the
//! change, so a request carries `since_ns` and the source puts the keyframe
//! on the first frame captured at or after it.

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The protocol this module speaks.
pub const PROTOCOL_VERSION: u32 = 4;
/// The oldest protocol a daemon on this module still accepts from a sim
/// helper (3: JPEG frames on fd 3, read by `k2_core::sim::ios::helper`).
pub const OLDEST_PROTOCOL: u32 = 3;

/// No message is larger than this (a 4K keyframe is far below).
pub const MAX_MESSAGE: usize = 64 * 1024 * 1024;
pub const KIND_JSON: u8 = b'J';
pub const KIND_UNIT: u8 = b'F';

/// The unit header's length in bytes.
pub const UNIT_HEADER_LEN: usize = 40;

const FLAG_KEY: u8 = 1;
const FLAG_SOURCE_START: u8 = 1 << 1;
const FLAG_DISCARDABLE: u8 = 1 << 2;
const FLAGS_KNOWN: u8 = FLAG_KEY | FLAG_SOURCE_START | FLAG_DISCARDABLE;

/// A unit's codec. 1 and 2 are also the viewer wire's K2SV codec byte;
/// JPEG goes to viewers as K2SF.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[repr(u8)]
pub enum Codec {
    H264 = 1,
    Vp8 = 2,
    Jpeg = 3,
}

impl Codec {
    pub fn from_byte(b: u8) -> Option<Codec> {
        match b {
            1 => Some(Codec::H264),
            2 => Some(Codec::Vp8),
            3 => Some(Codec::Jpeg),
            _ => None,
        }
    }

    /// The name in JSON (`"h264"`, `"vp8"`, `"jpeg"`).
    pub fn as_str(self) -> &'static str {
        match self {
            Codec::H264 => "h264",
            Codec::Vp8 => "vp8",
            Codec::Jpeg => "jpeg",
        }
    }

    /// A video codec (inter frames: deltas need the units before them).
    pub fn is_video(self) -> bool {
        self != Codec::Jpeg
    }
}

/// One unit's header (see the module docs for the bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnitHeader {
    pub codec: Codec,
    /// A keyframe: a viewer can start here. Every JPEG is one.
    pub key: bool,
    /// The first unit after the layer started or its source or size
    /// changed. Always also `key`.
    pub source_start: bool,
    /// No later unit references this one (a viewer may skip it).
    pub discardable: bool,
    pub layer: u8,
    /// The display (deskd); 0 for a simulator.
    pub source: u32,
    pub width: u32,
    pub height: u32,
    /// Per layer, +1 per unit.
    pub seq: u64,
    /// The source's clock (ns of uptime) when the OS handed it the pixels.
    pub captured_ns: u64,
    /// The source's clock when the encoder gave the unit back.
    pub emitted_ns: u64,
}

impl UnitHeader {
    pub fn encode(&self) -> [u8; UNIT_HEADER_LEN] {
        let mut b = [0u8; UNIT_HEADER_LEN];
        b[0] = self.codec as u8;
        b[1] = (if self.key { FLAG_KEY } else { 0 })
            | (if self.source_start { FLAG_SOURCE_START } else { 0 })
            | (if self.discardable { FLAG_DISCARDABLE } else { 0 });
        b[2] = self.layer;
        b[4..8].copy_from_slice(&self.source.to_be_bytes());
        b[8..12].copy_from_slice(&self.width.to_be_bytes());
        b[12..16].copy_from_slice(&self.height.to_be_bytes());
        b[16..24].copy_from_slice(&self.seq.to_be_bytes());
        b[24..32].copy_from_slice(&self.captured_ns.to_be_bytes());
        b[32..40].copy_from_slice(&self.emitted_ns.to_be_bytes());
        b
    }

    /// Reads a header from the front of `b`. Refuses an unknown codec, an
    /// unknown flag bit, and a source start that isn't a key unit.
    pub fn decode(b: &[u8]) -> io::Result<UnitHeader> {
        if b.len() < UNIT_HEADER_LEN {
            return Err(bad(format!("unit header of {} bytes, want {UNIT_HEADER_LEN}", b.len())));
        }
        let codec = Codec::from_byte(b[0]).ok_or_else(|| bad(format!("unknown codec byte {}", b[0])))?;
        let flags = b[1];
        if flags & !FLAGS_KNOWN != 0 {
            return Err(bad(format!("unknown unit flags {flags:#04x}")));
        }
        let key = flags & FLAG_KEY != 0;
        let source_start = flags & FLAG_SOURCE_START != 0;
        if source_start && !key {
            return Err(bad("a source-start unit must be a key unit"));
        }
        let u32_at = |i: usize| u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        let u64_at = |i: usize| {
            let mut a = [0u8; 8];
            a.copy_from_slice(&b[i..i + 8]);
            u64::from_be_bytes(a)
        };
        Ok(UnitHeader {
            codec,
            key,
            source_start,
            discardable: flags & FLAG_DISCARDABLE != 0,
            layer: b[2],
            source: u32_at(4),
            width: u32_at(8),
            height: u32_at(12),
            seq: u64_at(16),
            captured_ns: u64_at(24),
            emitted_ns: u64_at(32),
        })
    }
}

/// One encoded unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    pub header: UnitHeader,
    pub data: Vec<u8>,
}

/// One message off a link.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Json(Value),
    Unit(Unit),
}

fn bad(m: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, m.into())
}

/// Writes one message in a single `write_all`, so writers that each hold
/// the writer's lock for the call never interleave halves.
fn write_raw(w: &mut impl Write, kind: u8, parts: &[&[u8]]) -> io::Result<()> {
    let n = 1 + parts.iter().map(|p| p.len()).sum::<usize>();
    if n > MAX_MESSAGE {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("message of {n} bytes is over {MAX_MESSAGE}")));
    }
    let mut buf = Vec::with_capacity(4 + n);
    buf.extend_from_slice(&(n as u32).to_be_bytes());
    buf.push(kind);
    for p in parts {
        buf.extend_from_slice(p);
    }
    w.write_all(&buf)?;
    w.flush()
}

/// Writes one `J` message.
pub fn write_json(w: &mut impl Write, v: &impl Serialize) -> io::Result<()> {
    let body = serde_json::to_vec(v).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    write_raw(w, KIND_JSON, &[&body])
}

/// Writes one `F` message.
pub fn write_unit(w: &mut impl Write, h: &UnitHeader, data: &[u8]) -> io::Result<()> {
    write_raw(w, KIND_UNIT, &[&h.encode(), data])
}

/// Reads one message; `Ok(None)` at a clean end of stream (EOF before the
/// first length byte). EOF inside a message is an error. Reads exactly the
/// message's bytes.
pub fn read_message(r: &mut impl Read) -> io::Result<Option<Message>> {
    let mut len = [0u8; 4];
    let mut got = 0;
    while got < 4 {
        match r.read(&mut len[got..]) {
            Ok(0) if got == 0 => return Ok(None),
            Ok(0) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "end of stream inside a length")),
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    let n = u32::from_be_bytes(len) as usize;
    if n == 0 || n > MAX_MESSAGE {
        return Err(bad(format!("message of {n} bytes")));
    }
    let mut body = vec![0u8; n];
    r.read_exact(&mut body)?;
    let kind = body[0];
    match kind {
        KIND_JSON => {
            let v: Value = serde_json::from_slice(&body[1..]).map_err(|e| bad(format!("JSON: {e}")))?;
            if !v.is_object() {
                return Err(bad("a J message must be a JSON object"));
            }
            Ok(Some(Message::Json(v)))
        }
        KIND_UNIT => {
            let header = UnitHeader::decode(&body[1..])?;
            // Reuse the body's allocation for the payload.
            body.drain(..1 + UNIT_HEADER_LEN);
            Ok(Some(Message::Unit(Unit { header, data: body })))
        }
        k => Err(bad(format!("message kind {k:#04x}"))),
    }
}

// ── the version gate ──────────────────────────────────────────────────

/// The versions a source can speak, from its `ready` event or `hello`
/// reply: `accepts` if present, else `[version]`, else none.
pub fn spoken(ready: &Value) -> Vec<u32> {
    if let Some(a) = ready.get("accepts").and_then(Value::as_array) {
        return a.iter().filter_map(Value::as_u64).filter_map(|v| u32::try_from(v).ok()).collect();
    }
    ready.get("version").and_then(Value::as_u64).and_then(|v| u32::try_from(v).ok()).into_iter().collect()
}

/// The newest version both sides speak: the highest of `theirs` within
/// `oldest..=newest`. `None` = no common version (`needs_update`).
pub fn negotiate(oldest: u32, newest: u32, theirs: &[u32]) -> Option<u32> {
    theirs.iter().copied().filter(|v| (oldest..=newest).contains(v)).max()
}

// ── requests and the hello fields ─────────────────────────────────────

/// One layer the daemon wants (`layers` request).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayerSpec {
    /// The id units of this layer carry.
    pub id: u8,
    pub codec: Codec,
    /// The widest picture to send, in pixels; the source scales down to it
    /// and keeps the aspect. 0 = the source's own size.
    #[serde(default)]
    pub max_width: u32,
    /// The highest rate; the source paces to it on a fixed schedule.
    pub fps: u32,
    /// Target bits a second (video). JPEG ignores it.
    #[serde(default)]
    pub bitrate: u32,
    /// JPEG quality, 1-100 (JPEG only; the source's default if absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<u8>,
    /// false = paused: no capture work and no units.
    #[serde(default = "yes")]
    pub on: bool,
}

fn yes() -> bool {
    true
}

/// A protocol-4 request (daemon to source). Serialises as
/// `{"op": "layers", …}`; the caller adds `id` or `nr`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// The whole layer list (replaces the last one).
    Layers { layers: Vec<LayerSpec> },
    /// A keyframe on `layer`, on the first frame captured at or after
    /// `since_ns` (the source's clock; absent = the next frame).
    Keyframe {
        layer: u8,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        since_ns: Option<u64>,
    },
    /// New rate targets for one layer (rate control), without a keyframe.
    Rate { layer: u8, fps: u32, bitrate: u32 },
}

/// The encoder a source's session runs on (`hello.encoder`): after opening,
/// the source checks it (VideoToolbox: `UsingHardwareAcceleratedVideoEncoder`,
/// else the encoder list's entry for the session's `EncoderID`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Encoder {
    Hardware,
    Software,
    /// Nothing said which; never a guess.
    Unknown,
}

/// The protocol-4 fields of a source's `ready` event / `hello` reply. A
/// source adds its own beside them (deskd's `role`, `os`, TCC state; the
/// sim helper's screen and Xcode).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloFields {
    /// The newest protocol it speaks.
    pub version: u32,
    /// Every protocol it can speak.
    pub accepts: Vec<u32>,
    /// The codecs it can encode, in its order of preference.
    pub codecs: Vec<Codec>,
    /// Its H.264 encoder, as checked at start (`unknown` before the first
    /// session, or with no H.264).
    pub encoder: Encoder,
    /// Its clock now (ns of uptime), for the daemon's clock map.
    pub clock_ns: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn hdr() -> UnitHeader {
        UnitHeader {
            codec: Codec::H264,
            key: true,
            source_start: true,
            discardable: false,
            layer: 2,
            source: 7,
            width: 1206,
            height: 2622,
            seq: 42,
            captured_ns: 1_000,
            emitted_ns: 2_000,
        }
    }

    /// The bytes the Swift helper and deskd must write (a golden: change
    /// it only with a protocol bump).
    #[test]
    fn the_header_bytes_are_the_documented_layout() {
        let b = hdr().encode();
        let want: [u8; UNIT_HEADER_LEN] = [
            1, 0b011, 2, 0, // codec, flags (key + source start), layer, reserved
            0, 0, 0, 7, // source
            0, 0, 0x04, 0xb6, // width 1206
            0, 0, 0x0a, 0x3e, // height 2622
            0, 0, 0, 0, 0, 0, 0, 42, // seq
            0, 0, 0, 0, 0, 0, 0x03, 0xe8, // captured 1000
            0, 0, 0, 0, 0, 0, 0x07, 0xd0, // emitted 2000
        ];
        assert_eq!(b, want);
        assert_eq!(UnitHeader::decode(&b).unwrap(), hdr());
    }

    #[test]
    fn units_and_json_round_trip_with_the_exact_length_prefix() {
        let mut buf = Vec::new();
        write_json(&mut buf, &json!({"event": "ready", "version": 4, "accepts": [3, 4]})).unwrap();
        write_unit(&mut buf, &hdr(), b"annexb").unwrap();
        let first = u32::from_be_bytes(buf[0..4].try_into().unwrap()) as usize;
        assert_eq!(buf[4], KIND_JSON);
        let second = &buf[4 + first..];
        assert_eq!(u32::from_be_bytes(second[0..4].try_into().unwrap()) as usize, 1 + UNIT_HEADER_LEN + 6);
        let mut r = &buf[..];
        assert!(matches!(read_message(&mut r).unwrap(), Some(Message::Json(v)) if v["accepts"] == json!([3, 4])));
        match read_message(&mut r).unwrap() {
            Some(Message::Unit(u)) => {
                assert_eq!(u.header, hdr());
                assert_eq!(u.data, b"annexb");
            }
            other => panic!("want a unit, got {other:?}"),
        }
        assert!(read_message(&mut r).unwrap().is_none(), "a clean end");
    }

    #[test]
    fn every_codec_and_flag_mix_survives() {
        for codec in [Codec::H264, Codec::Vp8, Codec::Jpeg] {
            for (key, source_start, discardable) in [(false, false, false), (true, false, false), (true, true, false), (false, false, true)] {
                let h = UnitHeader { codec, key, source_start, discardable, ..hdr() };
                assert_eq!(UnitHeader::decode(&h.encode()).unwrap(), h);
            }
        }
    }

    #[test]
    fn bad_headers_are_refused() {
        let mut b = hdr().encode();
        b[0] = 9;
        assert!(UnitHeader::decode(&b).is_err(), "unknown codec");
        let mut b = hdr().encode();
        b[1] = 0b1000;
        assert!(UnitHeader::decode(&b).is_err(), "unknown flag");
        let mut b = hdr().encode();
        b[1] = FLAG_SOURCE_START;
        assert!(UnitHeader::decode(&b).is_err(), "source start without key");
        assert!(UnitHeader::decode(&b[..39]).is_err(), "short");
        let mut b = hdr().encode();
        b[3] = 0xff;
        assert_eq!(UnitHeader::decode(&b).unwrap(), hdr(), "reserved is ignored");
    }

    #[test]
    fn torn_and_garbage_messages_are_errors_not_huge_allocations() {
        let mut buf = Vec::new();
        write_unit(&mut buf, &hdr(), b"x").unwrap();
        assert!(read_message(&mut &buf[..buf.len() - 1]).is_err(), "torn body");
        assert!(read_message(&mut &buf[..2]).is_err(), "torn length");
        assert!(read_message(&mut &[0xff, 0xff, 0xff, 0xff, b'F'][..]).is_err(), "over MAX_MESSAGE");
        assert!(read_message(&mut &[0, 0, 0, 0][..]).is_err(), "zero length");
        assert!(read_message(&mut &[0, 0, 0, 2, b'Q', 0][..]).is_err(), "unknown kind");
        assert!(read_message(&mut &[0, 0, 0, 3, b'J', b'[', b']'][..]).is_err(), "J must be an object");
        let mut short = Vec::new();
        short.extend_from_slice(&17u32.to_be_bytes());
        short.push(KIND_UNIT);
        short.extend_from_slice(&[0u8; 16]);
        assert!(read_message(&mut &short[..]).is_err(), "a protocol 3 frame header is not a unit");
    }

    #[test]
    fn the_gate_picks_the_newest_common_version() {
        assert_eq!(negotiate(3, 4, &spoken(&json!({"version": 4, "accepts": [3, 4]}))), Some(4));
        assert_eq!(negotiate(3, 4, &spoken(&json!({"version": 3}))), Some(3), "a protocol 3 helper has no accepts");
        assert_eq!(negotiate(4, 4, &spoken(&json!({"version": 3}))), None, "deskd's daemon side wants 4");
        assert_eq!(negotiate(3, 4, &spoken(&json!({"version": 5, "accepts": [4, 5]}))), Some(4), "a newer source steps down");
        assert_eq!(negotiate(3, 4, &spoken(&json!({"version": 9, "accepts": [9]}))), None);
        assert_eq!(negotiate(3, 4, &spoken(&json!({}))), None);
    }

    #[test]
    fn requests_have_stable_json() {
        let layers = Request::Layers {
            layers: vec![
                LayerSpec { id: 0, codec: Codec::H264, max_width: 1206, fps: 60, bitrate: 12_000_000, quality: None, on: true },
                LayerSpec { id: 1, codec: Codec::Jpeg, max_width: 402, fps: 15, bitrate: 0, quality: Some(70), on: false },
            ],
        };
        assert_eq!(
            serde_json::to_value(&layers).unwrap(),
            json!({"op": "layers", "layers": [
                {"id": 0, "codec": "h264", "max_width": 1206, "fps": 60, "bitrate": 12_000_000, "on": true},
                {"id": 1, "codec": "jpeg", "max_width": 402, "fps": 15, "bitrate": 0, "quality": 70, "on": false},
            ]})
        );
        assert_eq!(serde_json::to_value(Request::Keyframe { layer: 1, since_ns: Some(5) }).unwrap(), json!({"op": "keyframe", "layer": 1, "since_ns": 5}));
        assert_eq!(serde_json::to_value(Request::Keyframe { layer: 0, since_ns: None }).unwrap(), json!({"op": "keyframe", "layer": 0}));
        assert_eq!(
            serde_json::to_value(Request::Rate { layer: 0, fps: 30, bitrate: 2_000_000 }).unwrap(),
            json!({"op": "rate", "layer": 0, "fps": 30, "bitrate": 2_000_000})
        );
        // A minimal layer from a hand-written request: defaults fill in.
        let l: LayerSpec = serde_json::from_value(json!({"id": 3, "codec": "vp8", "fps": 15})).unwrap();
        assert_eq!(l, LayerSpec { id: 3, codec: Codec::Vp8, max_width: 0, fps: 15, bitrate: 0, quality: None, on: true });
        assert!(serde_json::from_value::<Request>(json!({"op": "nope"})).is_err());
    }

    #[test]
    fn hello_fields_read_from_a_ready_event_with_extra_fields() {
        let v = json!({"event": "ready", "version": 4, "accepts": [3, 4], "codecs": ["h264", "jpeg"], "encoder": "hardware", "clock_ns": 9, "xcode": "26.6"});
        let h: HelloFields = serde_json::from_value(v).unwrap();
        assert_eq!(h, HelloFields { version: 4, accepts: vec![3, 4], codecs: vec![Codec::H264, Codec::Jpeg], encoder: Encoder::Hardware, clock_ns: 9 });
        assert_eq!(Codec::Vp8.as_str(), "vp8");
        assert!(Codec::H264.is_video() && !Codec::Jpeg.is_video());
    }
}
