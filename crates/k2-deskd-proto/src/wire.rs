//! Framing: `[u32 BE len][u8 kind][payload]`, `len` counting kind +
//! payload. `J` carries one JSON object; `F` carries one encoded frame.
//!
//! The `F` header below is this crate's guess at protocol 4's encoded
//! frame message, with the two fields the PRD asks K2 Render for (§3.3): a
//! codec byte and a source (display) id. D3 only plumbs it; D5/D8b emit it.

use std::io::{self, Read, Write};

/// No message is larger than this (a 4K keyframe is far below).
pub const MAX_MESSAGE: usize = 64 * 1024 * 1024;
pub const KIND_JSON: u8 = b'J';
pub const KIND_FRAME: u8 = b'F';

/// The codec byte. 1 and 2 match the K2SV codec byte (§6: 1 = H.264,
/// 2 = VP8). 3 = JPEG is local to deskd (JPEG goes to viewers as K2SF).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
}

/// One encoded frame's header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    pub codec: Codec,
    /// A keyframe (IDR for H.264, a key frame for VP8; every JPEG).
    pub key: bool,
    /// The display it shows (the source id, so a switch shows on the wire).
    pub display: u32,
    pub width: u32,
    pub height: u32,
    pub seq: u64,
    /// deskd's clock (ns of uptime) when the OS handed it the pixels.
    pub captured_ns: u64,
    /// deskd's clock when the encoder gave the unit back.
    pub emitted_ns: u64,
}

/// `codec u8, flags u8, reserved u16, display u32, width u32, height u32,
/// seq u64, captured_ns u64, emitted_ns u64`, all big-endian.
pub const FRAME_HEADER_LEN: usize = 40;
const FLAG_KEY: u8 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub header: FrameHeader,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Json(serde_json::Value),
    Frame(Frame),
}

fn bad(m: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, m.into())
}

/// Writes one message in a single `write_all` (several writers never
/// interleave halves when each holds the writer's lock for the call).
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
pub fn write_json(w: &mut impl Write, v: &impl serde::Serialize) -> io::Result<()> {
    let body = serde_json::to_vec(v).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    write_raw(w, KIND_JSON, &[&body])
}

pub fn encode_frame_header(h: &FrameHeader) -> [u8; FRAME_HEADER_LEN] {
    let mut b = [0u8; FRAME_HEADER_LEN];
    b[0] = h.codec as u8;
    b[1] = if h.key { FLAG_KEY } else { 0 };
    b[4..8].copy_from_slice(&h.display.to_be_bytes());
    b[8..12].copy_from_slice(&h.width.to_be_bytes());
    b[12..16].copy_from_slice(&h.height.to_be_bytes());
    b[16..24].copy_from_slice(&h.seq.to_be_bytes());
    b[24..32].copy_from_slice(&h.captured_ns.to_be_bytes());
    b[32..40].copy_from_slice(&h.emitted_ns.to_be_bytes());
    b
}

pub fn decode_frame_header(b: &[u8]) -> io::Result<FrameHeader> {
    if b.len() < FRAME_HEADER_LEN {
        return Err(bad(format!("frame header of {} bytes, want {FRAME_HEADER_LEN}", b.len())));
    }
    let codec = Codec::from_byte(b[0]).ok_or_else(|| bad(format!("unknown codec byte {}", b[0])))?;
    let u32_at = |i: usize| u32::from_be_bytes(b[i..i + 4].try_into().expect("4 bytes"));
    let u64_at = |i: usize| u64::from_be_bytes(b[i..i + 8].try_into().expect("8 bytes"));
    Ok(FrameHeader {
        codec,
        key: b[1] & FLAG_KEY != 0,
        display: u32_at(4),
        width: u32_at(8),
        height: u32_at(12),
        seq: u64_at(16),
        captured_ns: u64_at(24),
        emitted_ns: u64_at(32),
    })
}

/// Writes one `F` message.
pub fn write_frame(w: &mut impl Write, h: &FrameHeader, data: &[u8]) -> io::Result<()> {
    write_raw(w, KIND_FRAME, &[&encode_frame_header(h), data])
}

/// Reads one message; `Ok(None)` at a clean end of stream (EOF before the
/// first length byte). EOF inside a message is an error. Reads exactly the
/// message's bytes, so an unbuffered socket can be handed on after it (the
/// macOS fd byte follows `hello`).
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
    let (kind, payload) = (body[0], &body[1..]);
    match kind {
        KIND_JSON => {
            let v: serde_json::Value = serde_json::from_slice(payload).map_err(|e| bad(format!("JSON: {e}")))?;
            if !v.is_object() {
                return Err(bad("a J message must be a JSON object"));
            }
            Ok(Some(Message::Json(v)))
        }
        KIND_FRAME => {
            let header = decode_frame_header(payload)?;
            Ok(Some(Message::Frame(Frame { header, data: payload[FRAME_HEADER_LEN..].to_vec() })))
        }
        k => Err(bad(format!("message kind {k:#04x}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn hdr() -> FrameHeader {
        FrameHeader { codec: Codec::Vp8, key: true, display: 7, width: 1920, height: 1080, seq: 42, captured_ns: 1_000, emitted_ns: 2_000 }
    }

    #[test]
    fn json_round_trips_with_the_exact_length_prefix() {
        let mut buf = Vec::new();
        write_json(&mut buf, &json!({"event": "hello", "version": 4})).unwrap();
        let body_len = u32::from_be_bytes(buf[0..4].try_into().unwrap()) as usize;
        assert_eq!(body_len, buf.len() - 4);
        assert_eq!(buf[4], b'J');
        let m = read_message(&mut &buf[..]).unwrap().unwrap();
        assert_eq!(m, Message::Json(json!({"event": "hello", "version": 4})));
    }

    #[test]
    fn frame_round_trips_every_header_field() {
        let mut buf = Vec::new();
        write_frame(&mut buf, &hdr(), b"payload").unwrap();
        match read_message(&mut &buf[..]).unwrap().unwrap() {
            Message::Frame(f) => {
                assert_eq!(f.header, hdr());
                assert_eq!(f.data, b"payload");
            }
            other => panic!("want a frame, got {other:?}"),
        }
    }

    #[test]
    fn non_key_flag_and_each_codec_survive() {
        for codec in [Codec::H264, Codec::Vp8, Codec::Jpeg] {
            let h = FrameHeader { codec, key: false, ..hdr() };
            assert_eq!(decode_frame_header(&encode_frame_header(&h)).unwrap(), h);
        }
    }

    #[test]
    fn clean_eof_is_none_and_a_torn_message_is_an_error() {
        assert!(read_message(&mut &b""[..]).unwrap().is_none());
        let mut buf = Vec::new();
        write_json(&mut buf, &json!({"a": 1})).unwrap();
        let torn = &buf[..buf.len() - 1];
        assert!(read_message(&mut &torn[..]).is_err());
        assert!(read_message(&mut &buf[..2]).is_err());
    }

    #[test]
    fn refuses_zero_length_oversize_unknown_kind_and_non_objects() {
        assert!(read_message(&mut &[0u8, 0, 0, 0][..]).is_err());
        let big = ((MAX_MESSAGE + 1) as u32).to_be_bytes();
        assert!(read_message(&mut &big[..]).is_err());
        assert!(read_message(&mut &[0u8, 0, 0, 2, b'Z', 0][..]).is_err());
        let arr = b"[1]";
        let mut buf = vec![0, 0, 0, (arr.len() + 1) as u8, b'J'];
        buf.extend_from_slice(arr);
        assert!(read_message(&mut &buf[..]).is_err());
    }

    #[test]
    fn refuses_a_short_frame_header_and_an_unknown_codec() {
        let mut buf = vec![0, 0, 0, 5, b'F', 1, 0, 0, 0];
        assert!(read_message(&mut &buf[..]).is_err());
        let mut h = encode_frame_header(&hdr());
        h[0] = 9;
        buf = Vec::new();
        buf.extend_from_slice(&((1 + FRAME_HEADER_LEN) as u32).to_be_bytes());
        buf.push(b'F');
        buf.extend_from_slice(&h);
        assert!(read_message(&mut &buf[..]).is_err());
    }

    #[test]
    fn reads_exactly_one_message_and_leaves_the_rest() {
        let mut buf = Vec::new();
        write_json(&mut buf, &json!({"n": 1})).unwrap();
        buf.push(b'P');
        let mut r = &buf[..];
        read_message(&mut r).unwrap().unwrap();
        assert_eq!(r, b"P");
    }
}
