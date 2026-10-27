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
use super::lane::{Appearance, Button, DeviceProfile, Key, LaneKind, Orientation, TouchPhase};

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
    /// Typed text (printable characters).
    Text { text: String },
    Key { key: Key },
    Button { button: Button },
    Rotate { orientation: Orientation },
    Appearance { appearance: Appearance },
    OpenUrl { url: String },
    TakeOver,
    HandBack,
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
}
