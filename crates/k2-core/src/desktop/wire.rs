//! The `/cli/desktop/stream` text messages (`prd-remote-desktop-tab-v1` §6).
//!
//! ```text
//! GET /cli/desktop/stream?ticket=<k2dt_…>&display=<id>&codecs=h264,vp8,jpeg&win=3[&fps=<n>][&view=1][&stats=1]
//! ```
//!
//! Binary frames are the sim's K2SF / K2SV headers
//! (`crate::sim::stream`); the desktop adds K2SV codec byte
//! [`VIDEO_CODEC_VP8`]. The `wpt`/`hpt` fields carry the display's size in
//! points. This file is the text side: [`ServerMsg`] (daemon → viewer) and
//! [`ViewerMsg`] (viewer → daemon), tagged by `t` in kebab-case like the
//! sim's. The tab's `desktop-wire.ts` mirrors it field for field (the
//! goldens below pin the JSON).
//!
//! No long-lived token rides the URL (vs-live R20): a viewer first asks
//! `POST /cli/desktop/ticket` for a single-use ticket that lasts 60 s
//! ([`super::ticket`]).

use serde::{Deserialize, Serialize};

/// K2SV codec byte for H.264 (the sim's).
pub const VIDEO_CODEC_H264: u8 = crate::sim::stream::VIDEO_CODEC_H264;
/// K2SV codec byte for VP8 (desktop, Linux hosts).
pub const VIDEO_CODEC_VP8: u8 = 2;

/// Close codes (the sim's numbers).
pub const CLOSE_AUTH_REVOKED: u16 = crate::sim::stream::CLOSE_AUTH_REVOKED;
pub const CLOSE_FORBIDDEN: u16 = crate::sim::stream::CLOSE_FORBIDDEN;
pub const CLOSE_DESKTOP_OFF: u16 = crate::sim::stream::CLOSE_DEVICE_STOPPED;

/// Close reasons (the WebSocket close frame's text).
pub mod reason {
    /// 4401: the login was revoked, or its grant is gone.
    pub const AUTH_REVOKED: &str = "auth_revoked";
    pub const GRANT_GONE: &str = "grant_gone";
    /// 4403.
    pub const FORBIDDEN: &str = "forbidden";
    pub const KICKED: &str = "kicked";
    pub const SAME_MACHINE: &str = "same_machine";
    pub const AUDIT_UNAVAILABLE: &str = "audit_unavailable";
    /// 4410.
    pub const DESKTOP_OFF: &str = "desktop_off";
    pub const DESKD_ENDED: &str = "deskd_ended";
}

/// The stream's codec (`hello.codec`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Codec {
    Jpeg,
    H264,
    Vp8,
}

/// Which way the viewer reached the daemon (the rate table's key).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ViewerPath {
    Local,
    Lan,
    Tunnel,
}

/// Which tunnel carries the stream (§3.7): the main one, or the media lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Lane {
    Main,
    Media,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Encoder {
    Hardware,
    Software,
}

/// A viewer watches, or drives (holds the control lease).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    View,
    Control,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::View => "view",
            Mode::Control => "control",
        }
    }

    pub fn parse(s: &str) -> Option<Mode> {
        match s.trim() {
            "view" => Some(Mode::View),
            "control" => Some(Mode::Control),
            _ => None,
        }
    }
}

/// The host's OS family (`hello.os`; the tab flips ⌘ and Ctrl when the
/// viewer's differs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HostOs {
    Macos,
    Linux,
    Windows,
}

/// One display of the desktop (`displays`, `hello.displays`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Display {
    /// Stable for the session (macOS `CGDirectDisplayID`, an X11 output name).
    pub id: String,
    pub name: String,
    pub width_pt: u32,
    pub height_pt: u32,
    pub width_px: u32,
    pub height_px: u32,
    pub scale: f32,
    pub main: bool,
}

/// One viewer, as the owner sees it in [`ServerMsg::Viewers`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewerName {
    pub name: String,
    pub mode: Mode,
}

/// Daemon → viewer (text frames).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "kebab-case")]
pub enum ServerMsg {
    /// First message on every socket.
    #[serde(rename_all = "camelCase")]
    Hello {
        /// `console` in v1.
        desktop: String,
        display: String,
        displays: Vec<Display>,
        codec: Codec,
        window: u32,
        fps: u32,
        path: ViewerPath,
        lane: Lane,
        encoder: Encoder,
        mode: Mode,
        /// Who drives right now (a name), or nobody.
        controller: Option<String>,
        os: HostOs,
        /// Physical input at the machine pauses remote input for 3 s.
        local_wins: bool,
        same_machine: bool,
    },
    /// The machine's state changed (`super::MachineState` wire names).
    State { state: String, message: String },
    /// Who drives.
    #[serde(rename_all = "camelCase")]
    Control {
        controller: Option<String>,
        you: bool,
        /// Someone at the machine is using it: remote input is dropped
        /// (never queued) until the hold ends.
        local_hold: bool,
    },
    /// Who is watching (the owner only).
    Viewers { viewers: Vec<ViewerName> },
    /// A cursor shape, sent once per shape.
    #[serde(rename_all = "camelCase")]
    Cursor { id: u32, hot_x: u32, hot_y: u32, png: String },
    CursorPos { x: f32, y: f32, visible: bool },
    Displays { displays: Vec<Display> },
    /// The answer to `clipboard-get`. `text` is null with a `reason` when
    /// the host refused (`concealed`: a password manager's copy).
    Clipboard {
        text: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Fps { fps: u32 },
    Codec { codec: Codec },
    Error { code: String, message: String },
}

/// A mouse button (`button`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Button {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

impl Button {
    pub const ALL: [Button; 5] = [Button::Left, Button::Right, Button::Middle, Button::Back, Button::Forward];
}

/// How a wheel delta is measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WheelMode {
    Pixel,
    Line,
}

/// Viewer → daemon (text frames). Coordinates are the chosen display's
/// points, origin top left.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "kebab-case")]
pub enum ViewerMsg {
    /// The viewer drew frame `seq`.
    Ack { seq: u32 },
    /// Ask for another rate (`0` pauses a hidden tab).
    Fps { fps: u32 },
    /// Send a keyframe next.
    Keyframe,
    /// Absolute pointer position (coalesced).
    Pointer { x: f32, y: f32 },
    Button { button: Button, down: bool },
    /// Summed when coalesced.
    Wheel { dx: f32, dy: f32, mode: WheelMode },
    /// A physical key by `KeyboardEvent.code` (never coalesced).
    Key { code: String, down: bool },
    /// Typed text (IME, dead keys, a phone's soft keyboard): printable only,
    /// at most [`super::input::TEXT_MAX_CHARS`].
    Text { text: String },
    /// Put text on the host's clipboard (controller only).
    ClipboardSet { text: String },
    /// Read the host's clipboard (controller only).
    ClipboardGet,
    TakeControl,
    HandBack,
    /// Switch display.
    Display { id: String },
    /// Drop to view-only (`on: true`) or ask to drive again.
    View { on: bool },
}

impl ViewerMsg {
    /// Messages that act on the machine (refused for a watcher).
    pub fn is_input(&self) -> bool {
        matches!(
            self,
            ViewerMsg::Pointer { .. }
                | ViewerMsg::Button { .. }
                | ViewerMsg::Wheel { .. }
                | ViewerMsg::Key { .. }
                | ViewerMsg::Text { .. }
                | ViewerMsg::ClipboardSet { .. }
                | ViewerMsg::ClipboardGet
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn round_trip_server(m: ServerMsg, golden: serde_json::Value) {
        assert_eq!(serde_json::to_value(&m).expect("serialize"), golden, "{m:?}");
        let back: ServerMsg = serde_json::from_value(golden).expect("deserialize");
        assert_eq!(back, m);
    }

    fn round_trip_viewer(m: ViewerMsg, golden: serde_json::Value) {
        assert_eq!(serde_json::to_value(&m).expect("serialize"), golden, "{m:?}");
        let back: ViewerMsg = serde_json::from_value(golden).expect("deserialize");
        assert_eq!(back, m);
    }

    fn display() -> Display {
        Display {
            id: "1".into(),
            name: "Built-in".into(),
            width_pt: 1512,
            height_pt: 982,
            width_px: 3024,
            height_px: 1964,
            scale: 2.0,
            main: true,
        }
    }

    #[test]
    fn server_message_goldens() {
        round_trip_server(
            ServerMsg::Hello {
                desktop: "console".into(),
                display: "1".into(),
                displays: vec![display()],
                codec: Codec::H264,
                window: 3,
                fps: 60,
                path: ViewerPath::Lan,
                lane: Lane::Main,
                encoder: Encoder::Hardware,
                mode: Mode::View,
                controller: None,
                os: HostOs::Macos,
                local_wins: true,
                same_machine: false,
            },
            json!({
                "t": "hello", "desktop": "console", "display": "1",
                "displays": [{"id": "1", "name": "Built-in", "widthPt": 1512, "heightPt": 982,
                              "widthPx": 3024, "heightPx": 1964, "scale": 2.0, "main": true}],
                "codec": "h264", "window": 3, "fps": 60, "path": "lan", "lane": "main",
                "encoder": "hardware", "mode": "view", "controller": null, "os": "macos",
                "localWins": true, "sameMachine": false
            }),
        );
        round_trip_server(
            ServerMsg::State { state: "locked".into(), message: "This Mac is locked.".into() },
            json!({"t": "state", "state": "locked", "message": "This Mac is locked."}),
        );
        round_trip_server(
            ServerMsg::Control { controller: Some("rosson".into()), you: true, local_hold: false },
            json!({"t": "control", "controller": "rosson", "you": true, "localHold": false}),
        );
        round_trip_server(
            ServerMsg::Viewers { viewers: vec![ViewerName { name: "rosson".into(), mode: Mode::Control }] },
            json!({"t": "viewers", "viewers": [{"name": "rosson", "mode": "control"}]}),
        );
        round_trip_server(
            ServerMsg::Cursor { id: 7, hot_x: 1, hot_y: 2, png: "iVBO".into() },
            json!({"t": "cursor", "id": 7, "hotX": 1, "hotY": 2, "png": "iVBO"}),
        );
        round_trip_server(
            ServerMsg::CursorPos { x: 10.5, y: 20.0, visible: true },
            json!({"t": "cursor-pos", "x": 10.5, "y": 20.0, "visible": true}),
        );
        round_trip_server(
            ServerMsg::Displays { displays: vec![display()] },
            json!({"t": "displays", "displays": [{"id": "1", "name": "Built-in", "widthPt": 1512, "heightPt": 982,
                    "widthPx": 3024, "heightPx": 1964, "scale": 2.0, "main": true}]}),
        );
        round_trip_server(
            ServerMsg::Clipboard { text: Some("hi".into()), reason: None },
            json!({"t": "clipboard", "text": "hi"}),
        );
        round_trip_server(
            ServerMsg::Clipboard { text: None, reason: Some("concealed".into()) },
            json!({"t": "clipboard", "text": null, "reason": "concealed"}),
        );
        round_trip_server(ServerMsg::Fps { fps: 30 }, json!({"t": "fps", "fps": 30}));
        round_trip_server(ServerMsg::Codec { codec: Codec::Vp8 }, json!({"t": "codec", "codec": "vp8"}));
        round_trip_server(
            ServerMsg::Error { code: "desktop_watch_only".into(), message: "You're watching.".into() },
            json!({"t": "error", "code": "desktop_watch_only", "message": "You're watching."}),
        );
    }

    #[test]
    fn viewer_message_goldens() {
        round_trip_viewer(ViewerMsg::Ack { seq: 9 }, json!({"t": "ack", "seq": 9}));
        round_trip_viewer(ViewerMsg::Fps { fps: 0 }, json!({"t": "fps", "fps": 0}));
        round_trip_viewer(ViewerMsg::Keyframe, json!({"t": "keyframe"}));
        round_trip_viewer(ViewerMsg::Pointer { x: 1.5, y: 2.0 }, json!({"t": "pointer", "x": 1.5, "y": 2.0}));
        round_trip_viewer(
            ViewerMsg::Button { button: Button::Forward, down: true },
            json!({"t": "button", "button": "forward", "down": true}),
        );
        round_trip_viewer(
            ViewerMsg::Wheel { dx: 0.0, dy: -3.0, mode: WheelMode::Line },
            json!({"t": "wheel", "dx": 0.0, "dy": -3.0, "mode": "line"}),
        );
        round_trip_viewer(
            ViewerMsg::Key { code: "KeyW".into(), down: false },
            json!({"t": "key", "code": "KeyW", "down": false}),
        );
        round_trip_viewer(ViewerMsg::Text { text: "é".into() }, json!({"t": "text", "text": "é"}));
        round_trip_viewer(ViewerMsg::ClipboardSet { text: "x".into() }, json!({"t": "clipboard-set", "text": "x"}));
        round_trip_viewer(ViewerMsg::ClipboardGet, json!({"t": "clipboard-get"}));
        round_trip_viewer(ViewerMsg::TakeControl, json!({"t": "take-control"}));
        round_trip_viewer(ViewerMsg::HandBack, json!({"t": "hand-back"}));
        round_trip_viewer(ViewerMsg::Display { id: "2".into() }, json!({"t": "display", "id": "2"}));
        round_trip_viewer(ViewerMsg::View { on: true }, json!({"t": "view", "on": true}));
    }

    #[test]
    fn unknown_messages_are_refused() {
        assert!(serde_json::from_value::<ViewerMsg>(json!({"t": "touch", "x": 1, "y": 2})).is_err());
        assert!(serde_json::from_value::<ViewerMsg>(json!({"t": "button", "button": "thumb", "down": true})).is_err());
    }

    #[test]
    fn input_messages_are_the_ones_that_act() {
        let acts = [
            ViewerMsg::Pointer { x: 0.0, y: 0.0 },
            ViewerMsg::Button { button: Button::Left, down: true },
            ViewerMsg::Wheel { dx: 0.0, dy: 1.0, mode: WheelMode::Pixel },
            ViewerMsg::Key { code: "KeyA".into(), down: true },
            ViewerMsg::Text { text: "a".into() },
            ViewerMsg::ClipboardSet { text: "a".into() },
            ViewerMsg::ClipboardGet,
        ];
        for m in &acts {
            assert!(m.is_input(), "{m:?}");
        }
        let watch = [
            ViewerMsg::Ack { seq: 0 },
            ViewerMsg::Fps { fps: 1 },
            ViewerMsg::Keyframe,
            ViewerMsg::TakeControl,
            ViewerMsg::HandBack,
            ViewerMsg::Display { id: "1".into() },
            ViewerMsg::View { on: false },
        ];
        for m in &watch {
            assert!(!m.is_input(), "{m:?}");
        }
    }

    #[test]
    fn close_codes_are_the_sims() {
        assert_eq!((CLOSE_AUTH_REVOKED, CLOSE_FORBIDDEN, CLOSE_DESKTOP_OFF), (4401, 4403, 4410));
        assert_eq!(VIDEO_CODEC_H264, 1);
        assert_eq!(VIDEO_CODEC_VP8, 2);
    }
}
