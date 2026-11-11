//! Framing: `[u32 BE len][u8 kind][payload]`, `len` counting kind +
//! payload. `J` carries one JSON object; `F` carries one encoded unit.
//!
//! This is K2 Render's helper link, protocol 4 ([`k2_simvideo::link`]),
//! re-exported: the control link (requests, replies, events) and the frames
//! pipe read and write exactly what k2-simhelper's do, and the daemon's
//! `LinkSource` reads deskd's units with the same reader. A unit's header
//! carries the codec, key / source-start / discardable flags, the layer id,
//! the source (the display, deskd's v1 rule: one capture per desktop), the
//! coded size, a per-layer sequence number and the capture and emit times on
//! deskd's clock (`hello.clock_ns`).
//!
//! (D3 carried a local guess at this header; D5 replaced it with the
//! engine's, vs-live R28.)

pub use k2_simvideo::link::{read_message, write_json, write_unit, Codec, Message, Unit, UnitHeader, KIND_JSON, KIND_UNIT, MAX_MESSAGE, UNIT_HEADER_LEN};

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A JSON message and a unit share one stream as they do on a socket,
    /// and each read takes exactly one message (the macOS fd byte follows
    /// `hello` on an unbuffered socket).
    #[test]
    fn control_and_units_share_the_framing() {
        let h = UnitHeader { codec: Codec::H264, key: true, source_start: true, discardable: false, layer: 1, source: 7, width: 1920, height: 1080, seq: 0, captured_ns: 1_000, emitted_ns: 2_000 };
        let mut buf = Vec::new();
        write_json(&mut buf, &json!({"event": "hello", "version": 4})).unwrap();
        write_unit(&mut buf, &h, b"annexb").unwrap();
        buf.push(b'P');
        let mut r = &buf[..];
        assert_eq!(read_message(&mut r).unwrap(), Some(Message::Json(json!({"event": "hello", "version": 4}))));
        match read_message(&mut r).unwrap() {
            Some(Message::Unit(u)) => {
                assert_eq!(u.header, h);
                assert_eq!(u.data, b"annexb");
            }
            other => panic!("want a unit, got {other:?}"),
        }
        assert_eq!(r, b"P");
    }
}
