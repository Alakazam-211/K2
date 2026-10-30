//! The `/cli/sim/stream` wire format (`prd-mobile-dev-v1` §7, M-A4). One
//! WebSocket per viewer:
//!
//! ```text
//! GET /cli/sim/stream?workspace=<id|handle>&token=<…>[&fps=<n>]
//! ```
//!
//! **Daemon → viewer, binary:** one frame per message:
//! a 16-byte header ([`FrameHeader`]) then a baseline JPEG.
//!
//! ```text
//! 0..4   magic  b"K2SF"
//! 4..8   seq    u32 LE  (per device; gaps mean this viewer skipped frames)
//! 8..10  width  u16 LE  JPEG width in pixels
//! 10..12 height u16 LE  JPEG height in pixels
//! 12..14 wpt    u16 LE  screen width in points (current orientation)
//! 14..16 hpt    u16 LE  screen height in points
//! 16..   JPEG
//! ```
//!
//! A viewer maps a click at canvas `(cx, cy)` to points as
//! `cx / canvas_w * wpt`. Frames are sent one at a time: the daemon sends
//! the next frame only after the viewer's [`ViewerMsg::Ack`] for the last
//! one (one frame in flight), and no faster than the viewer's fps cap. A
//! slow viewer just skips frames and never holds others back.
//!
//! **Video (0.46.7).** A viewer may offer codecs and a window in the query
//! (`&codecs=h264,jpeg&win=3`). The daemon picks one ([`negotiate`]) and
//! says so in `hello` (`codec`, `window`); a viewer that offers nothing gets
//! the JPEG wire above with one frame in flight, exactly as before. An
//! H.264 frame is one binary message with a 24-byte [`VideoHeader`] then an
//! Annex B access unit (SPS + PPS in front of every keyframe):
//!
//! ```text
//! 0..4   magic  b"K2SV"
//! 4..8   seq    u32 LE
//! 8..10  width  u16 LE  picture width in pixels (crop the decoded frame to this)
//! 10..12 height u16 LE
//! 12..14 wpt    u16 LE  screen width in points
//! 14..16 hpt    u16 LE
//! 16     codec  u8      1 = H.264
//! 17     flags  u8      bit 0 = keyframe
//! 18..20 age    u16 LE  ms from capture to send on the daemon (saturating)
//! 20..24 ts     u32 LE  capture time, ms since the viewer's stream began
//! 24..   Annex B
//! ```
//!
//! With a window of N the daemon keeps up to N frames unacked (fewer when
//! the round trip doesn't need them, `k2_simvideo::flow::AckWindow`) and
//! always sends the newest frame, so stale frames are skipped, not queued.
//! An H.264 viewer gets its own encoder: frames are skipped *before*
//! encoding, so the reference chain never breaks. Keyframes come on join,
//! on a size change (rotation) and on [`ViewerMsg::Keyframe`]. If the
//! encoder fails the daemon falls back to JPEG on the same socket and says
//! so with [`ServerMsg::Codec`]; a viewer reads either magic at any time.
//!
//! **Daemon → viewer, text:** JSON [`ServerMsg`] (`{"t": "hello", …}`).
//!
//! **Viewer → daemon, text:** JSON [`ViewerMsg`]. Input is in points.
//! Agents (passports) and App passes are refused before the upgrade; the
//! token is re-checked every 5 s and the socket closes (code 4401) when it
//! is revoked.

use serde::{Deserialize, Serialize};

use super::hold::HoldView;
use super::lane::{Appearance, Button, DeviceProfile, EditOp, Key, LaneKind, Orientation, TouchPhase};

pub const MAGIC: [u8; 4] = *b"K2SF";
pub const HEADER_LEN: usize = 16;

/// fps caps (PRD §7, M-A19).
pub const LOCAL_MAX_FPS: u32 = 30;
pub const REMOTE_DEFAULT_FPS: u32 = 8;
pub const REMOTE_MIN_FPS: u32 = 2;
pub const REMOTE_MAX_FPS: u32 = 15;

/// H.264 viewers: 2–30 fps remote (default 24), up to 30 local. A P frame
/// is a few KB where a JPEG is tens, so remote viewers may go faster.
pub const REMOTE_VIDEO_DEFAULT_FPS: u32 = 24;
pub const REMOTE_VIDEO_MAX_FPS: u32 = 30;

/// The most frames a viewer may have in flight (`win=`).
pub use k2_simvideo::flow::MAX_WINDOW;

pub const VIDEO_MAGIC: [u8; 4] = *b"K2SV";
pub const VIDEO_HEADER_LEN: usize = 24;
/// [`VideoHeader::codec`] for H.264.
pub const VIDEO_CODEC_H264: u8 = 1;
/// [`VideoHeader`] flags bit: an IDR access unit with SPS + PPS.
pub const VIDEO_FLAG_KEY: u8 = 1;

/// Close codes the daemon uses.
pub const CLOSE_AUTH_REVOKED: u16 = 4401;
pub const CLOSE_FORBIDDEN: u16 = 4403;
pub const CLOSE_DEVICE_STOPPED: u16 = 4410;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    pub seq: u32,
    pub width_px: u16,
    pub height_px: u16,
    pub width_pt: u16,
    pub height_pt: u16,
}

impl FrameHeader {
    pub fn encode(&self, jpeg: &[u8]) -> Vec<u8> {
        let mut v = Vec::with_capacity(HEADER_LEN + jpeg.len());
        v.extend_from_slice(&MAGIC);
        v.extend_from_slice(&self.seq.to_le_bytes());
        v.extend_from_slice(&self.width_px.to_le_bytes());
        v.extend_from_slice(&self.height_px.to_le_bytes());
        v.extend_from_slice(&self.width_pt.to_le_bytes());
        v.extend_from_slice(&self.height_pt.to_le_bytes());
        v.extend_from_slice(jpeg);
        v
    }

    /// Split a binary message into header and JPEG.
    pub fn decode(msg: &[u8]) -> Option<(FrameHeader, &[u8])> {
        if msg.len() < HEADER_LEN || msg[0..4] != MAGIC {
            return None;
        }
        let u16at = |i: usize| u16::from_le_bytes([msg[i], msg[i + 1]]);
        Some((
            FrameHeader {
                seq: u32::from_le_bytes([msg[4], msg[5], msg[6], msg[7]]),
                width_px: u16at(8),
                height_px: u16at(10),
                width_pt: u16at(12),
                height_pt: u16at(14),
            },
            &msg[HEADER_LEN..],
        ))
    }
}

/// One H.264 frame's header (24 bytes, little-endian).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoHeader {
    pub seq: u32,
    pub width_px: u16,
    pub height_px: u16,
    pub width_pt: u16,
    pub height_pt: u16,
    pub codec: u8,
    pub key: bool,
    /// Capture → send on the daemon, ms (saturating).
    pub age_ms: u16,
    /// Capture time, ms since this viewer's stream began.
    pub ts_ms: u32,
}

impl VideoHeader {
    pub fn encode(&self, au: &[u8]) -> Vec<u8> {
        let mut v = Vec::with_capacity(VIDEO_HEADER_LEN + au.len());
        v.extend_from_slice(&VIDEO_MAGIC);
        v.extend_from_slice(&self.seq.to_le_bytes());
        v.extend_from_slice(&self.width_px.to_le_bytes());
        v.extend_from_slice(&self.height_px.to_le_bytes());
        v.extend_from_slice(&self.width_pt.to_le_bytes());
        v.extend_from_slice(&self.height_pt.to_le_bytes());
        v.push(self.codec);
        v.push(if self.key { VIDEO_FLAG_KEY } else { 0 });
        v.extend_from_slice(&self.age_ms.to_le_bytes());
        v.extend_from_slice(&self.ts_ms.to_le_bytes());
        v.extend_from_slice(au);
        v
    }

    /// Split a binary message into header and access unit.
    pub fn decode(msg: &[u8]) -> Option<(VideoHeader, &[u8])> {
        if msg.len() < VIDEO_HEADER_LEN || msg[0..4] != VIDEO_MAGIC {
            return None;
        }
        let u16at = |i: usize| u16::from_le_bytes([msg[i], msg[i + 1]]);
        let u32at = |i: usize| u32::from_le_bytes([msg[i], msg[i + 1], msg[i + 2], msg[i + 3]]);
        Some((
            VideoHeader {
                seq: u32at(4),
                width_px: u16at(8),
                height_px: u16at(10),
                width_pt: u16at(12),
                height_pt: u16at(14),
                codec: msg[16],
                key: msg[17] & VIDEO_FLAG_KEY != 0,
                age_ms: u16at(18),
                ts_ms: u32at(20),
            },
            &msg[VIDEO_HEADER_LEN..],
        ))
    }
}

/// The stream's codec (`hello.codec`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Codec {
    /// K2SF frames (every viewer and daemon understands them).
    #[default]
    Jpeg,
    /// K2SV frames, Annex B H.264.
    H264,
}

impl Codec {
    pub fn as_str(self) -> &'static str {
        match self {
            Codec::Jpeg => "jpeg",
            Codec::H264 => "h264",
        }
    }
    pub fn parse(s: &str) -> Option<Codec> {
        match s.trim() {
            "jpeg" => Some(Codec::Jpeg),
            "h264" => Some(Codec::H264),
            _ => None,
        }
    }
}

/// The viewer's `codecs=` list, in its order of preference (unknown names
/// are skipped; an old viewer sends none).
pub fn parse_offer(q: Option<&str>) -> Vec<Codec> {
    let mut out = Vec::new();
    for c in q.unwrap_or("").split(',').filter_map(Codec::parse) {
        if !out.contains(&c) {
            out.push(c);
        }
    }
    out
}

/// H.264 when the viewer offered it and this host can encode it; JPEG
/// otherwise (every viewer can draw JPEG, offered or not).
pub fn negotiate(offer: &[Codec], host_h264: bool) -> Codec {
    if host_h264 && offer.contains(&Codec::H264) {
        Codec::H264
    } else {
        Codec::Jpeg
    }
}

/// The viewer's `win=`: 1 (the old one-in-flight wire) when absent or
/// unreadable, else clamped to 1..=[`MAX_WINDOW`].
pub fn negotiate_window(q: Option<&str>) -> u32 {
    q.and_then(|s| s.trim().parse::<u32>().ok()).map_or(1, |n| n.clamp(1, MAX_WINDOW))
}

/// The device state clients draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SimState {
    /// Tools are downloading (first use).
    Installing,
    Booting,
    Running,
    Stopping,
    Stopped,
    Failed,
}

impl SimState {
    pub fn as_str(self) -> &'static str {
        match self {
            SimState::Installing => "installing",
            SimState::Booting => "booting",
            SimState::Running => "running",
            SimState::Stopping => "stopping",
            SimState::Stopped => "stopped",
            SimState::Failed => "failed",
        }
    }
    pub fn parse(s: &str) -> Option<SimState> {
        Some(match s {
            "installing" => SimState::Installing,
            "booting" => SimState::Booting,
            "running" => SimState::Running,
            "stopping" => SimState::Stopping,
            "stopped" => SimState::Stopped,
            "failed" => SimState::Failed,
            _ => return None,
        })
    }
    /// Counts against the box's device limit.
    pub fn occupies_slot(self) -> bool {
        matches!(self, SimState::Booting | SimState::Running | SimState::Stopping)
    }
}

/// Daemon → viewer (text frames).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "kebab-case")]
pub enum ServerMsg {
    /// First message on every socket.
    #[serde(rename_all = "camelCase")]
    Hello {
        /// `s<n>`.
        sim: String,
        lane: LaneKind,
        state: SimState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile: Option<DeviceProfile>,
        /// The cap this socket got (after the daemon's local/remote clamp).
        fps: u32,
        remote: bool,
        hold: HoldView,
        /// "separate, not sealed" text for the tab (PRD §8).
        isolation: String,
        /// What input this device takes beyond one finger. Absent from
        /// older daemons: the tab then offers no pinch.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input: Option<InputCaps>,
        /// The codec this socket streams (0.46.7; absent from older
        /// daemons, which means JPEG).
        #[serde(default)]
        codec: Codec,
        /// Frames this viewer may have in flight (0.46.7; absent = 1).
        #[serde(default = "one")]
        window: u32,
    },
    State {
        state: SimState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    Hold { hold: HoldView },
    /// A viewer input was refused (e.g. the device is stopped).
    Error { code: String, message: String },
    /// The fps cap changed (the viewer asked, or the daemon clamped it).
    Fps { fps: u32 },
    /// The codec changed mid-stream (the H.264 encoder failed and this
    /// socket fell back to JPEG). Only sent to viewers that offered video.
    Codec {
        codec: Codec,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
}

fn one() -> u32 {
    1
}

/// What a device's input takes beyond one finger (`hello.input`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputCaps {
    /// Two fingers at once (`touch` with `x2`/`y2`).
    pub multi_touch: bool,
    /// `touch` up takes `vx`/`vy` (this daemon replays the fling).
    pub fling: bool,
}

/// Viewer → daemon (text frames). Coordinates are in points.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "kebab-case")]
pub enum ViewerMsg {
    /// The viewer finished drawing frame `seq`; send the next.
    Ack { seq: u32 },
    /// Ask for a different rate (clamped; remote 2–15, local ≤ 30; H.264
    /// remote 2–30).
    Fps { fps: u32 },
    /// Send a keyframe next (the viewer's decoder lost its place). Ignored
    /// on a JPEG stream.
    Keyframe,
    /// Pointer phases (down / move / up) for drags and taps.
    ///
    /// - `x2`/`y2`: a second finger (pinch). Both or neither; refused
    ///   (never sent as one finger) unless `hello.input.multiTouch`.
    /// - `vx`/`vy` on `up`: the release velocity in points per second.
    ///   The daemon replays the last few moves at that speed on the
    ///   device's own clock before lifting ([`super::gesture::fling_tail`]),
    ///   so a flick coasts however the moves were bunched in transit.
    ///   Older daemons ignore both and just lift.
    Touch {
        phase: TouchPhase,
        x: f32,
        y: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        x2: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        y2: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vx: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vy: Option<f32>,
    },
    Tap { x: f32, y: f32 },
    #[serde(rename_all = "camelCase")]
    Swipe { x1: f32, y1: f32, x2: f32, y2: f32, #[serde(default)] duration_ms: Option<u32> },
    /// Typed text: one keystroke's character or one IME commit
    /// (printable, at most [`TEXT_MAX_CHARS`]).
    Text { text: String },
    /// A non-text key, with Shift / Option held or not.
    Key {
        key: Key,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        shift: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        alt: bool,
    },
    /// The host clipboard's text, on the person's explicit ⌘V (at most
    /// [`PASTE_MAX_BYTES`]).
    Paste { text: String },
    /// A ⌘ / Ctrl editing chord the person pressed inside the device.
    Edit { op: EditOp },
    Button { button: Button },
    Rotate { orientation: Orientation },
    Appearance { appearance: Appearance },
    OpenUrl { url: String },
    TakeOver,
    HandBack,
}

// ── keyboard input: caps and rate ────────────────────────────────────

/// One `text` message: a keystroke or an IME commit. Longer text is a paste.
pub const TEXT_MAX_CHARS: usize = 256;
/// One `paste` message (UTF-8 bytes).
pub const PASTE_MAX_BYTES: usize = 64 * 1024;
/// Keystrokes (`text`, `key`, `edit`): a burst of this many, then
/// [`KEYS_PER_SEC`]. Held-key repeat is ~30/s; fast typing ~15/s.
pub const KEYS_BURST: f64 = 60.0;
pub const KEYS_PER_SEC: f64 = 30.0;
/// Pastes: a burst of this many, then one every two seconds.
pub const PASTES_BURST: f64 = 3.0;
pub const PASTES_PER_SEC: f64 = 0.5;

/// Why a viewer's keyboard message was refused (sent back as
/// [`ServerMsg::Error`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputRefusal {
    pub code: &'static str,
    pub message: String,
}

impl InputRefusal {
    fn invalid(m: impl Into<String>) -> Self {
        InputRefusal { code: "sim_invalid", message: m.into() }
    }
}

/// Size and content checks for keyboard messages (other messages pass).
pub fn check_input(m: &ViewerMsg) -> Result<(), InputRefusal> {
    match m {
        ViewerMsg::Text { text } => {
            let n = text.chars().count();
            if n == 0 {
                return Err(InputRefusal::invalid("Empty text."));
            }
            if n > TEXT_MAX_CHARS {
                return Err(InputRefusal::invalid(format!("Typed text is over {TEXT_MAX_CHARS} characters; send it as a paste.")));
            }
            if text.chars().any(char::is_control) {
                return Err(InputRefusal::invalid("Typed text can't hold control characters; send Enter, Tab and Backspace as keys."));
            }
            Ok(())
        }
        ViewerMsg::Paste { text } => {
            if text.is_empty() {
                return Err(InputRefusal::invalid("The clipboard has no text."));
            }
            if text.len() > PASTE_MAX_BYTES {
                return Err(InputRefusal::invalid(format!("The clipboard text is over {} KB; paste less at a time.", PASTE_MAX_BYTES / 1024)));
            }
            if text.chars().any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t')) {
                return Err(InputRefusal::invalid("The clipboard text holds control characters."));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// A token bucket.
#[derive(Debug, Clone)]
struct Bucket {
    cap: f64,
    per_sec: f64,
    tokens: f64,
    at: std::time::Instant,
}

impl Bucket {
    fn new(cap: f64, per_sec: f64, now: std::time::Instant) -> Self {
        Bucket { cap, per_sec, tokens: cap, at: now }
    }
    fn take(&mut self, now: std::time::Instant) -> bool {
        let dt = now.saturating_duration_since(self.at).as_secs_f64();
        self.at = self.at.max(now);
        self.tokens = (self.tokens + dt * self.per_sec).min(self.cap);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// One viewer's keyboard rate limit. Touch is not counted here (the
/// input queue bounds it).
#[derive(Debug, Clone)]
pub struct InputBudget {
    keys: Bucket,
    pastes: Bucket,
}

impl InputBudget {
    pub fn new(now: std::time::Instant) -> Self {
        InputBudget { keys: Bucket::new(KEYS_BURST, KEYS_PER_SEC, now), pastes: Bucket::new(PASTES_BURST, PASTES_PER_SEC, now) }
    }

    /// Spend one token for a keyboard message; `Err` when this viewer
    /// sends too fast. Other messages are free.
    pub fn take(&mut self, m: &ViewerMsg, now: std::time::Instant) -> Result<(), InputRefusal> {
        let ok = match m {
            ViewerMsg::Text { .. } | ViewerMsg::Key { .. } | ViewerMsg::Edit { .. } => self.keys.take(now),
            ViewerMsg::Paste { .. } => self.pastes.take(now),
            _ => true,
        };
        if ok {
            Ok(())
        } else {
            Err(InputRefusal { code: "sim_rate_limited", message: "Typing faster than the device takes it; slow down.".into() })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_round_trips() {
        let h = FrameHeader { seq: 77, width_px: 411, height_px: 923, width_pt: 411, height_pt: 923 };
        let msg = h.encode(&[0xff, 0xd8, 0xff]);
        assert_eq!(msg.len(), HEADER_LEN + 3);
        let (d, jpeg) = FrameHeader::decode(&msg).expect("decodes");
        assert_eq!(d, h);
        assert_eq!(jpeg, &[0xff, 0xd8, 0xff]);
        assert!(FrameHeader::decode(b"nope").is_none());
    }

    #[test]
    fn video_header_round_trips_and_refuses_jpeg_frames() {
        let h = VideoHeader {
            seq: 0x0102_0304,
            width_px: 412,
            height_px: 916,
            width_pt: 411,
            height_pt: 915,
            codec: VIDEO_CODEC_H264,
            key: true,
            age_ms: 7,
            ts_ms: 123_456,
        };
        let msg = h.encode(&[0, 0, 0, 1, 0x65]);
        assert_eq!(msg.len(), VIDEO_HEADER_LEN + 5);
        assert_eq!(&msg[..4], b"K2SV");
        assert_eq!(&msg[4..8], &[4, 3, 2, 1], "LE seq");
        assert_eq!(msg[16], 1);
        assert_eq!(msg[17], VIDEO_FLAG_KEY);
        let (d, au) = VideoHeader::decode(&msg).expect("decodes");
        assert_eq!(d, h);
        assert_eq!(au, &[0, 0, 0, 1, 0x65]);
        let delta = VideoHeader { key: false, ..h }.encode(&[]);
        assert!(!VideoHeader::decode(&delta).expect("delta").0.key);
        // A K2SF frame is not a K2SV frame, and vice versa.
        let jpeg = FrameHeader { seq: 1, width_px: 1, height_px: 1, width_pt: 1, height_pt: 1 }.encode(&[0xff; 16]);
        assert!(VideoHeader::decode(&jpeg).is_none());
        assert!(FrameHeader::decode(&msg).is_none());
        assert!(VideoHeader::decode(&msg[..20]).is_none(), "short");
    }

    #[test]
    fn negotiation_prefers_video_only_when_both_sides_can() {
        assert_eq!(parse_offer(Some("h264,jpeg")), vec![Codec::H264, Codec::Jpeg]);
        assert_eq!(parse_offer(Some("av1, h264 ,h264,")), vec![Codec::H264]);
        assert!(parse_offer(None).is_empty());
        assert_eq!(negotiate(&[Codec::H264, Codec::Jpeg], true), Codec::H264);
        assert_eq!(negotiate(&[Codec::H264, Codec::Jpeg], false), Codec::Jpeg, "host can't encode");
        assert_eq!(negotiate(&[Codec::Jpeg], true), Codec::Jpeg, "viewer can't decode");
        assert_eq!(negotiate(&[], true), Codec::Jpeg, "an old viewer offers nothing");
        assert_eq!(negotiate_window(None), 1, "an old viewer: one in flight");
        assert_eq!(negotiate_window(Some("3")), 3);
        assert_eq!(negotiate_window(Some("99")), MAX_WINDOW);
        assert_eq!(negotiate_window(Some("0")), 1);
        assert_eq!(negotiate_window(Some("lots")), 1);
    }

    #[test]
    fn hello_says_codec_and_window_and_old_hellos_still_parse() {
        let hello = ServerMsg::Hello {
            sim: "s1".into(),
            lane: LaneKind::Android,
            state: SimState::Running,
            profile: None,
            fps: 30,
            remote: false,
            hold: HoldView::Free,
            isolation: "x".into(),
            input: None,
            codec: Codec::H264,
            window: 3,
        };
        let v = serde_json::to_value(&hello).expect("json");
        assert_eq!(v["codec"], "h264");
        assert_eq!(v["window"], 3);
        // A hello from an older daemon (no codec, no window) = JPEG, 1.
        let old: ServerMsg = serde_json::from_str(
            r#"{"t":"hello","sim":"s1","lane":"android","state":"running","fps":8,"remote":true,"hold":{"kind":"free"},"isolation":"x"}"#,
        )
        .expect("old hello");
        match old {
            ServerMsg::Hello { codec, window, .. } => assert_eq!((codec, window), (Codec::Jpeg, 1)),
            other => panic!("not a hello: {other:?}"),
        }
        let s = serde_json::to_value(ServerMsg::Codec { codec: Codec::Jpeg, message: None }).expect("json");
        assert_eq!(s, serde_json::json!({"t":"codec","codec":"jpeg"}));
        let k: ViewerMsg = serde_json::from_str(r#"{"t":"keyframe"}"#).expect("keyframe");
        assert_eq!(k, ViewerMsg::Keyframe);
    }

    #[test]
    fn json_shapes_are_stable() {
        let m: ViewerMsg = serde_json::from_str(r#"{"t":"tap","x":10,"y":20.5}"#).expect("tap");
        assert_eq!(m, ViewerMsg::Tap { x: 10.0, y: 20.5 });
        let m: ViewerMsg = serde_json::from_str(r#"{"t":"take-over"}"#).expect("take-over");
        assert_eq!(m, ViewerMsg::TakeOver);
        let m: ViewerMsg = serde_json::from_str(r#"{"t":"button","button":"home"}"#).expect("button");
        assert_eq!(m, ViewerMsg::Button { button: Button::Home });
        let m: ViewerMsg = serde_json::from_str(r#"{"t":"touch","phase":"down","x":1,"y":2}"#).expect("touch");
        assert_eq!(m, ViewerMsg::Touch { phase: TouchPhase::Down, x: 1.0, y: 2.0, x2: None, y2: None, vx: None, vy: None });
        let s = serde_json::to_value(ServerMsg::Hold { hold: HoldView::Person { by: "rosson".into(), ms: 2000 } }).expect("json");
        assert_eq!(s, serde_json::json!({"t":"hold","hold":{"kind":"person","by":"rosson","ms":2000}}));
        let s = serde_json::to_value(ServerMsg::Hold { hold: HoldView::Takeover { by: "rosson".into(), idle_ms: 5 } }).expect("json");
        assert_eq!(s, serde_json::json!({"t":"hold","hold":{"kind":"takeover","by":"rosson","idleMs":5}}));
        let s = serde_json::to_value(ServerMsg::State { state: SimState::Running, message: None }).expect("json");
        assert_eq!(s, serde_json::json!({"t":"state","state":"running"}));
    }

    #[test]
    fn keyboard_messages_parse_with_optional_modifiers() {
        let m: ViewerMsg = serde_json::from_str(r#"{"t":"key","key":"arrow-left"}"#).expect("plain key");
        assert_eq!(m, ViewerMsg::Key { key: Key::ArrowLeft, shift: false, alt: false });
        let m: ViewerMsg = serde_json::from_str(r#"{"t":"key","key":"arrow-left","shift":true,"alt":true}"#).expect("chord");
        assert_eq!(m, ViewerMsg::Key { key: Key::ArrowLeft, shift: true, alt: true });
        assert_eq!(serde_json::to_value(&m).expect("json"), serde_json::json!({"t":"key","key":"arrow-left","shift":true,"alt":true}));
        let plain = ViewerMsg::Key { key: Key::Enter, shift: false, alt: false };
        assert_eq!(serde_json::to_value(&plain).expect("json"), serde_json::json!({"t":"key","key":"enter"}), "unset modifiers stay off the wire");
        let m: ViewerMsg = serde_json::from_str(r#"{"t":"paste","text":"hello\nworld"}"#).expect("paste");
        assert_eq!(m, ViewerMsg::Paste { text: "hello\nworld".into() });
        let m: ViewerMsg = serde_json::from_str(r#"{"t":"edit","op":"select-all"}"#).expect("edit");
        assert_eq!(m, ViewerMsg::Edit { op: EditOp::SelectAll });
        assert!(serde_json::from_str::<ViewerMsg>(r#"{"t":"edit","op":"format-disk"}"#).is_err());
        assert!(serde_json::from_str::<ViewerMsg>(r#"{"t":"key","key":"f13"}"#).is_err());
    }

    #[test]
    fn keyboard_messages_are_capped() {
        let text = |s: &str| ViewerMsg::Text { text: s.into() };
        assert_eq!(check_input(&text("a")), Ok(()));
        assert_eq!(check_input(&text("日本語")), Ok(()));
        assert_eq!(check_input(&text(&"é".repeat(TEXT_MAX_CHARS))), Ok(()), "the cap counts characters, not bytes");
        assert_eq!(check_input(&text(&"x".repeat(TEXT_MAX_CHARS + 1))).expect_err("too long").code, "sim_invalid");
        assert!(check_input(&text("")).is_err());
        assert!(check_input(&text("a\nb")).is_err(), "Enter is a key, not text");
        assert!(check_input(&text("\u{1b}[A")).is_err());
        let paste = |s: String| ViewerMsg::Paste { text: s };
        assert_eq!(check_input(&paste("line 1\r\nline 2\tend".into())), Ok(()));
        assert_eq!(check_input(&paste("x".repeat(PASTE_MAX_BYTES))), Ok(()));
        let e = check_input(&paste("x".repeat(PASTE_MAX_BYTES + 1))).expect_err("too big");
        assert!(e.message.contains("64 KB"), "{}", e.message);
        assert!(check_input(&paste(String::new())).is_err());
        assert!(check_input(&paste("bell\u{7}".into())).is_err());
        assert_eq!(check_input(&ViewerMsg::Tap { x: 1.0, y: 1.0 }), Ok(()));
    }

    #[test]
    fn keyboard_messages_are_rate_limited_per_viewer() {
        let t0 = std::time::Instant::now();
        let mut b = InputBudget::new(t0);
        let key = ViewerMsg::Key { key: Key::Backspace, shift: false, alt: false };
        for i in 0..KEYS_BURST as usize {
            assert_eq!(b.take(&key, t0), Ok(()), "burst {i}");
        }
        let e = b.take(&ViewerMsg::Text { text: "a".into() }, t0).expect_err("over the burst");
        assert_eq!(e.code, "sim_rate_limited");
        // Taps never spend the keyboard budget.
        assert_eq!(b.take(&ViewerMsg::Tap { x: 1.0, y: 1.0 }, t0), Ok(()));
        // 100 ms later three keys' worth has refilled (30/s).
        let t1 = t0 + std::time::Duration::from_millis(100);
        for _ in 0..3 {
            assert_eq!(b.take(&ViewerMsg::Edit { op: EditOp::Undo }, t1), Ok(()));
        }
        assert!(b.take(&key, t1).is_err());
        // Pastes have their own, smaller budget.
        let p = ViewerMsg::Paste { text: "x".into() };
        for _ in 0..PASTES_BURST as usize {
            assert_eq!(b.take(&p, t1), Ok(()));
        }
        assert!(b.take(&p, t1).is_err());
        assert_eq!(b.take(&p, t1 + std::time::Duration::from_secs(2)), Ok(()), "one paste every two seconds");
    }

    /// Two fingers and a release velocity ride the same `touch` message;
    /// an old client's one-finger `touch` still parses, and a plain touch
    /// serializes without the new keys (old daemons see what they knew).
    #[test]
    fn touch_takes_a_second_finger_and_a_velocity() {
        let m: ViewerMsg = serde_json::from_str(r#"{"t":"touch","phase":"move","x":100,"y":200,"x2":300,"y2":400}"#).expect("two fingers");
        assert_eq!(m, ViewerMsg::Touch { phase: TouchPhase::Move, x: 100.0, y: 200.0, x2: Some(300.0), y2: Some(400.0), vx: None, vy: None });
        let m: ViewerMsg = serde_json::from_str(r#"{"t":"touch","phase":"up","x":5,"y":6,"vx":-120.5,"vy":2400}"#).expect("fling");
        assert_eq!(m, ViewerMsg::Touch { phase: TouchPhase::Up, x: 5.0, y: 6.0, x2: None, y2: None, vx: Some(-120.5), vy: Some(2400.0) });
        let plain = serde_json::to_value(ViewerMsg::Touch { phase: TouchPhase::Up, x: 1.0, y: 2.0, x2: None, y2: None, vx: None, vy: None }).expect("json");
        assert_eq!(plain, serde_json::json!({"t":"touch","phase":"up","x":1.0,"y":2.0}));
    }

    #[test]
    fn hello_reports_input_caps_and_parses_without_them() {
        let hello = ServerMsg::Hello {
            sim: "s1".into(),
            lane: LaneKind::Android,
            state: SimState::Running,
            profile: None,
            fps: 30,
            remote: false,
            hold: HoldView::Free,
            isolation: "x".into(),
            input: Some(InputCaps { multi_touch: true, fling: true }),
            codec: Codec::Jpeg,
            window: 1,
        };
        let v = serde_json::to_value(&hello).expect("json");
        assert_eq!(v["input"], serde_json::json!({"multiTouch": true, "fling": true}));
        let mut old = v.clone();
        old.as_object_mut().expect("object").remove("input");
        let back: ServerMsg = serde_json::from_value(old).expect("an older daemon's hello");
        assert!(matches!(back, ServerMsg::Hello { input: None, .. }));
    }
}
