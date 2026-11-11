//! The JSON messages (`J`).
//!
//! - **Requests** (daemon → deskd) carry `op`, and `id` unless `nr: true`
//!   (fire-and-forget, as k2-simhelper's `nr`: a failure comes back as an
//!   `input_error` event).
//! - **Replies** (deskd → daemon) carry the request's `id` and `ok`.
//! - **Events** (deskd → daemon) carry `event` and no `id`. The first
//!   message deskd sends is always the `hello` event.

use serde::{Deserialize, Serialize};

use crate::link::{Codec, Encoder, HelloFields, LayerSpec};

/// TCC state as deskd sees it. `required` is false where the OS has no
/// such grant (Linux), and then both flags read true.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Tcc {
    pub required: bool,
    /// `CGPreflightScreenCaptureAccess()`.
    pub screen: bool,
    /// `AXIsProcessTrusted()`.
    pub accessibility: bool,
}

impl Tcc {
    pub const NOT_REQUIRED: Tcc = Tcc { required: false, screen: true, accessibility: true };
    pub fn granted(&self) -> bool {
        !self.required || (self.screen && self.accessibility)
    }
}

/// How the frames reach the daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FramesVia {
    /// The read end of a pipe, sent over the socket with `SCM_RIGHTS`
    /// right after `hello` (macOS).
    Fd,
    /// The child's fd 3, set up by the daemon at spawn (Linux).
    Fd3,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    /// [`crate::PROTOCOL_VERSION`] of the deskd that answered.
    pub version: u32,
    /// deskd's build line ([`crate::VERSION`]).
    pub build: String,
    /// Always [`crate::ROLE`].
    pub role: String,
    /// `macos` or `linux`.
    pub os: String,
    /// What deskd can drive here: `macos`, `x11`, `wayland` (no input in
    /// D4; D9 adds the portal) or `none` (no display in the session).
    pub backend: String,
    pub tcc: Tcc,
    pub pid: u32,
    pub frames: FramesVia,
    /// Protocol 4's fields (`link::HelloFields`, so the daemon reads them
    /// from this hello as it does from a sim helper's `ready`): every
    /// protocol deskd speaks (`[4]`).
    #[serde(default)]
    pub accepts: Vec<u32>,
    /// The codecs deskd can encode here, in its order of preference
    /// (macOS: H.264; empty where capture isn't built yet).
    #[serde(default)]
    pub codecs: Vec<Codec>,
    /// Its H.264 encoder, from a probe session at start (`unknown` with no
    /// H.264, or when the probe couldn't open one).
    #[serde(default = "unknown_encoder")]
    pub encoder: Encoder,
    /// deskd's clock now (ns), the clock of every unit's capture and emit
    /// times.
    #[serde(default)]
    pub clock_ns: u64,
}

fn unknown_encoder() -> Encoder {
    Encoder::Unknown
}

impl Hello {
    /// The protocol-4 fields, for the daemon's `LinkClock` and codec
    /// negotiation.
    pub fn link_fields(&self) -> HelloFields {
        HelloFields { version: self.version, accepts: self.accepts.clone(), codecs: self.codecs.clone(), encoder: self.encoder, clock_ns: self.clock_ns }
    }
}

/// Mouse buttons (`button` in §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WheelMode {
    #[default]
    Pixel,
    Line,
}

/// One input event, in the viewer's `t` shapes (§6) so the stream route
/// can forward a checked viewer message as it is. Coordinates are the
/// chosen display's points, origin top left.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "kebab-case")]
pub enum InputEvent {
    Pointer {
        #[serde(default)]
        display: u32,
        x: f64,
        y: f64,
    },
    Button { button: Button, down: bool },
    /// Browser signs: `dy > 0` scrolls the content up (wheel towards you).
    Wheel {
        dx: f64,
        dy: f64,
        #[serde(default)]
        mode: WheelMode,
    },
    /// A physical key by `KeyboardEvent.code` ([`crate::keymap`]).
    Key { code: String, down: bool },
    /// Typed as Unicode (R12). Checked with [`check_text`].
    Text { text: String },
}

impl InputEvent {
    pub fn kind(&self) -> &'static str {
        match self {
            InputEvent::Pointer { .. } => "pointer",
            InputEvent::Button { .. } => "button",
            InputEvent::Wheel { .. } => "wheel",
            InputEvent::Key { .. } => "key",
            InputEvent::Text { .. } => "text",
        }
    }
}

/// One `text` message's cap (the sim's, `sim/stream.rs` TEXT_MAX_CHARS).
pub const TEXT_MAX_CHARS: usize = 256;

/// The sim's text rule, word for word (vs-live R15): no control
/// characters, so one `text` can't run a shell line; Enter, Tab and
/// Backspace come as keys.
pub fn check_text(text: &str) -> Result<(), &'static str> {
    let n = text.chars().count();
    if n == 0 {
        return Err("Empty text.");
    }
    if n > TEXT_MAX_CHARS {
        return Err("Typed text is over 256 characters; send it as a paste.");
    }
    if text.chars().any(char::is_control) {
        return Err("Typed text can't hold control characters; send Enter, Tab and Backspace as keys.");
    }
    Ok(())
}

/// Why the daemon asks deskd to let go of every held key and button
/// (vs-live R3). deskd also does it on its own for `local_hold`,
/// `link_closed`, `sigterm` and `startup`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseReason {
    HandBack,
    LeaseLost,
    ViewOnly,
    ViewerClosed,
    DisplaySwitch,
    LocalHold,
    LinkClosed,
    Sigterm,
    Startup,
    Other,
}

/// What the daemon asks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// Reply: `{tcc}`.
    TccStatus,
    /// Fires `CGRequestScreenCaptureAccess()` and the Accessibility prompt,
    /// and opens the matching System Settings pane. Reply: `{tcc}`.
    TccRequest,
    /// Usually `nr`. Dropped (not queued) during a local hold (R14).
    Inject { ev: InputEvent },
    /// Reply: `{released}` (how many ups were posted).
    ReleaseAll { reason: ReleaseReason },
    /// `text: None` hides the banner. Reply: `{}`.
    Banner { text: Option<String> },
    /// Reply: `{hid_idle_ms, local_idle_ms, hold}`. `local_idle_ms` is
    /// null until deskd has seen local input.
    Idle,
    /// Reply: `{displays: [Display]}`.
    Displays,
    /// Reply: `{}`.
    Ping,
    /// Protocol 4: the whole layer list (`link::Request::Layers`), usually
    /// `nr`. A layer missing or `on: false` is paused; no layer on = no
    /// capture at all. Reply: `{}`.
    Layers { layers: Vec<LayerSpec> },
    /// Protocol 4: a keyframe on `layer`, on the first frame captured at or
    /// after `since_ns` (deskd's clock; absent = the next frame). On a still
    /// screen deskd encodes the last frame again. Reply: `{}`.
    Keyframe {
        layer: u8,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        since_ns: Option<u64>,
    },
    /// Protocol 4: a layer's new rate targets, without a keyframe.
    /// Reply: `{}`.
    Rate { layer: u8, fps: u32, bitrate: u32 },
    /// Capture this display instead (v1 rule: one capture per desktop, so
    /// every layer moves). Each layer's next unit is a key with `source
    /// start` from it. Reply: `{display}`; `no_such_display` otherwise.
    /// (The field is `display`, not `id`: `id` is the request's.)
    Display { display: u32 },
    /// The host's clipboard, text only (the controller's Copy theirs; the
    /// daemon gates who). Reply: `{text}` or `{text: null, reason}` with a
    /// [`clip`] reason. deskd never logs the text.
    ClipboardGet,
    /// Put text on the host's clipboard (the controller's Send my
    /// clipboard). At most [`CLIPBOARD_MAX_BYTES`] of UTF-8. Reply: `{}`;
    /// `empty` / `too_large` / `unsupported` otherwise. Never logged.
    ClipboardSet { text: String },
}

/// The most clipboard text deskd reads or writes, in UTF-8 bytes (1 MiB).
pub const CLIPBOARD_MAX_BYTES: usize = 1024 * 1024;

/// Why `clipboard_get` gave no text (`reason`).
pub mod clip {
    /// Nothing on the clipboard.
    pub const EMPTY: &str = "empty";
    /// Something, but not text (an image, files).
    pub const NOT_TEXT: &str = "not_text";
    /// A password manager's copy (marked `org.nspasteboard.ConcealedType`).
    pub const CONCEALED: &str = "concealed";
    /// Over [`super::CLIPBOARD_MAX_BYTES`].
    pub const TOO_LARGE: &str = "too_large";
    /// The person at the Mac set K2 Screen's paste access to Deny.
    pub const DENIED: &str = "denied";
    /// No clipboard here yet (Linux until its slice).
    pub const UNSUPPORTED: &str = "unsupported";
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub nr: bool,
    #[serde(flatten)]
    pub op: Op,
}

/// One display (§3.6). Points origin is global (macOS: CG global points;
/// X11: root pixels, scale 1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Display {
    pub id: u32,
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub width_pt: f64,
    pub height_pt: f64,
    pub width_px: u32,
    pub height_px: u32,
    pub scale: f64,
    pub main: bool,
}

/// What deskd says on its own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Hello(Hello),
    /// The 5 s re-check saw a change (and once more when both are granted).
    TccChanged { tcc: Tcc },
    /// Local input started (`hold: true`) or the 3 s hold ran out
    /// (`hold: false`). Remote input is dropped while held.
    LocalInput { hold: bool },
    /// An `nr` request failed, or an injected event was dropped.
    InputError { op: String, code: String, error: String },
    /// Someone clicked Disconnect on the banner.
    BannerDisconnect,
    /// deskd can't go on (no display, …); it exits after sending this.
    Fatal { code: String, error: String },
    /// A layer can't run: a codec deskd doesn't make here, or its encoder
    /// failed to open (or failed twice in a row). It sends nothing until
    /// the next `layers` list changes it. Codes in [`codes`].
    LayerFailed { layer: u8, code: String, error: String },
    /// The capture stopped or couldn't start (the display went away,
    /// Screen Recording was revoked, …). deskd tries again every few
    /// seconds while a layer is on.
    CaptureError { display: u32, code: String, error: String },
}

/// `input_error`, `layer_failed` and `capture_error` codes.
pub mod codes {
    pub const LOCAL_HOLD: &str = "local_hold";
    pub const NO_ACCESSIBILITY: &str = "no_accessibility";
    pub const UNKNOWN_KEY: &str = "unknown_key";
    pub const BAD_TEXT: &str = "bad_text";
    pub const NO_DISPLAY: &str = "no_display";
    pub const POST_FAILED: &str = "post_failed";
    pub const UNSUPPORTED: &str = "unsupported";
    pub const BAD_REQUEST: &str = "bad_request";
    pub const NO_SUCH_DISPLAY: &str = "no_such_display";
    pub const ENCODER_FAILED: &str = "encoder_failed";
    pub const CAPTURE_FAILED: &str = "capture_failed";
    pub const NO_SCREEN_PERMISSION: &str = "no_screen_permission";
    /// `clipboard_set` refusals.
    pub const EMPTY: &str = "empty";
    pub const TOO_LARGE: &str = "too_large";
    pub const CLIPBOARD_FAILED: &str = "clipboard_failed";
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn requests_have_the_wire_shapes() {
        let r = Request { id: None, nr: true, op: Op::Inject { ev: InputEvent::Key { code: "KeyA".into(), down: true } } };
        assert_eq!(serde_json::to_value(&r).unwrap(), json!({"nr": true, "op": "inject", "ev": {"t": "key", "code": "KeyA", "down": true}}));
        let r = Request { id: Some(3), nr: false, op: Op::ReleaseAll { reason: ReleaseReason::HandBack } };
        assert_eq!(serde_json::to_value(&r).unwrap(), json!({"id": 3, "op": "release_all", "reason": "hand_back"}));
        let r: Request = serde_json::from_value(json!({"id": 9, "op": "tcc_request"})).unwrap();
        assert_eq!(r, Request { id: Some(9), nr: false, op: Op::TccRequest });
        let r: Request = serde_json::from_value(json!({"id": 1, "op": "banner", "text": null})).unwrap();
        assert_eq!(r.op, Op::Banner { text: None });
    }

    /// What the daemon's stream sends (`managed.rs`) parses as is.
    #[test]
    fn the_daemons_clipboard_requests_parse() {
        let r: Request = serde_json::from_value(json!({"id": 5, "op": "clipboard_get"})).unwrap();
        assert_eq!(r, Request { id: Some(5), nr: false, op: Op::ClipboardGet });
        let r: Request = serde_json::from_value(json!({"id": 6, "op": "clipboard_set", "text": "héllo\nthere"})).unwrap();
        assert_eq!(r.op, Op::ClipboardSet { text: "héllo\nthere".into() });
        assert!(serde_json::from_value::<Request>(json!({"id": 7, "op": "clipboard_set"})).is_err(), "text is required");
    }

    #[test]
    fn viewer_shapes_parse_as_input_events() {
        let p: InputEvent = serde_json::from_value(json!({"t": "pointer", "x": 10.5, "y": 3})).unwrap();
        assert_eq!(p, InputEvent::Pointer { display: 0, x: 10.5, y: 3.0 });
        let w: InputEvent = serde_json::from_value(json!({"t": "wheel", "dx": 0, "dy": -4, "mode": "line"})).unwrap();
        assert_eq!(w, InputEvent::Wheel { dx: 0.0, dy: -4.0, mode: WheelMode::Line });
        let b: InputEvent = serde_json::from_value(json!({"t": "button", "button": "forward", "down": false})).unwrap();
        assert_eq!(b, InputEvent::Button { button: Button::Forward, down: false });
        assert!(serde_json::from_value::<InputEvent>(json!({"t": "button", "button": "fifth", "down": true})).is_err());
    }

    #[test]
    fn events_round_trip() {
        let h = Event::Hello(Hello {
            version: 4,
            build: "0.1.0".into(),
            role: "desktop".into(),
            os: "macos".into(),
            backend: "macos".into(),
            tcc: Tcc { required: true, screen: false, accessibility: true },
            pid: 12,
            frames: FramesVia::Fd,
            accepts: vec![4],
            codecs: vec![Codec::H264],
            encoder: Encoder::Hardware,
            clock_ns: 77,
        });
        let v = serde_json::to_value(&h).unwrap();
        assert_eq!(v["event"], "hello");
        assert_eq!(v["frames"], "fd");
        assert_eq!(serde_json::from_value::<Event>(v).unwrap(), h);
        let v = serde_json::to_value(Event::BannerDisconnect).unwrap();
        assert_eq!(v, json!({"event": "banner_disconnect"}));
        let v = serde_json::to_value(Event::LayerFailed { layer: 2, code: codes::UNSUPPORTED.into(), error: "no JPEG".into() }).unwrap();
        assert_eq!(v, json!({"event": "layer_failed", "layer": 2, "code": "unsupported", "error": "no JPEG"}));
    }

    /// The daemon reads the engine's hello fields out of deskd's hello
    /// (`LinkClock::from_hello`, codec negotiation), and a D3-era hello
    /// without them still parses (as no codecs, unknown encoder).
    #[test]
    fn hello_carries_protocol_4s_fields() {
        let v = json!({"event": "hello", "version": 4, "build": "0.1.0", "role": "desktop", "os": "macos", "backend": "macos",
            "tcc": {"required": true, "screen": true, "accessibility": true}, "pid": 9, "frames": "fd",
            "accepts": [4], "codecs": ["h264"], "encoder": "software", "clock_ns": 123});
        let f: HelloFields = serde_json::from_value(v.clone()).expect("the engine's fields");
        assert_eq!(f, HelloFields { version: 4, accepts: vec![4], codecs: vec![Codec::H264], encoder: Encoder::Software, clock_ns: 123 });
        let Event::Hello(h) = serde_json::from_value::<Event>(v).unwrap() else { panic!("a hello") };
        assert_eq!(h.link_fields(), f);
        let old = json!({"event": "hello", "version": 4, "build": "0.1.0", "role": "desktop", "os": "linux", "backend": "x11",
            "tcc": {"required": false, "screen": true, "accessibility": true}, "pid": 9, "frames": "fd3"});
        let Event::Hello(h) = serde_json::from_value::<Event>(old).unwrap() else { panic!("a hello") };
        assert_eq!((h.codecs.len(), h.encoder, h.clock_ns), (0, Encoder::Unknown, 0));
    }

    /// What the engine's `LinkSource` writes (`link::Request` plus `nr`)
    /// parses as deskd's own request, unchanged.
    #[test]
    fn the_engines_requests_parse_as_ops() {
        use crate::link::Request as LinkRequest;
        let layer = LayerSpec { id: 1, codec: Codec::H264, max_width: 1920, fps: 30, bitrate: 4_000_000, quality: None, on: true };
        for (r, op) in [
            (LinkRequest::Layers { layers: vec![layer] }, Op::Layers { layers: vec![layer] }),
            (LinkRequest::Layers { layers: vec![] }, Op::Layers { layers: vec![] }),
            (LinkRequest::Keyframe { layer: 1, since_ns: Some(55) }, Op::Keyframe { layer: 1, since_ns: Some(55) }),
            (LinkRequest::Keyframe { layer: 0, since_ns: None }, Op::Keyframe { layer: 0, since_ns: None }),
            (LinkRequest::Rate { layer: 1, fps: 30, bitrate: 2_000_000 }, Op::Rate { layer: 1, fps: 30, bitrate: 2_000_000 }),
        ] {
            let mut v = serde_json::to_value(&r).unwrap();
            v["nr"] = json!(true);
            let parsed: Request = serde_json::from_value(v.clone()).unwrap_or_else(|e| panic!("{v}: {e}"));
            assert_eq!(parsed, Request { id: None, nr: true, op });
            // And back: the same JSON the engine wrote.
            assert_eq!(serde_json::to_value(&parsed).unwrap(), v);
        }
        // `display` names the display; `id` stays the request's.
        let d: Request = serde_json::from_value(json!({"id": 4, "op": "display", "display": 2})).unwrap();
        assert_eq!(d, Request { id: Some(4), nr: false, op: Op::Display { display: 2 } });
    }

    #[test]
    fn text_rule_matches_the_sim() {
        assert!(check_text("héllo wörld 👋").is_ok());
        assert_eq!(check_text(""), Err("Empty text."));
        assert!(check_text("ls\n").is_err());
        assert!(check_text("a\tb").is_err());
        assert!(check_text(&"x".repeat(256)).is_ok());
        assert!(check_text(&"x".repeat(257)).is_err());
        // 256 characters, more bytes: the cap counts characters.
        assert!(check_text(&"é".repeat(256)).is_ok());
    }

    #[test]
    fn tcc_granted() {
        assert!(Tcc::NOT_REQUIRED.granted());
        assert!(!Tcc { required: true, screen: true, accessibility: false }.granted());
        assert!(Tcc { required: true, screen: true, accessibility: true }.granted());
    }
}
