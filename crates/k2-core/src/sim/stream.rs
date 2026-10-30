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
}

/// Viewer → daemon (text frames). Coordinates are in points.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "kebab-case")]
pub enum ViewerMsg {
    /// The viewer finished drawing frame `seq`; send the next.
    Ack { seq: u32 },
    /// Ask for a different rate (clamped; remote 2–15, local ≤ 30).
    Fps { fps: u32 },
    /// Pointer phases (down / move / up) for drags and taps.
    Touch { phase: TouchPhase, x: f32, y: f32 },
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
    fn json_shapes_are_stable() {
        let m: ViewerMsg = serde_json::from_str(r#"{"t":"tap","x":10,"y":20.5}"#).expect("tap");
        assert_eq!(m, ViewerMsg::Tap { x: 10.0, y: 20.5 });
        let m: ViewerMsg = serde_json::from_str(r#"{"t":"take-over"}"#).expect("take-over");
        assert_eq!(m, ViewerMsg::TakeOver);
        let m: ViewerMsg = serde_json::from_str(r#"{"t":"button","button":"home"}"#).expect("button");
        assert_eq!(m, ViewerMsg::Button { button: Button::Home });
        let m: ViewerMsg = serde_json::from_str(r#"{"t":"touch","phase":"down","x":1,"y":2}"#).expect("touch");
        assert_eq!(m, ViewerMsg::Touch { phase: TouchPhase::Down, x: 1.0, y: 2.0 });
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
}
