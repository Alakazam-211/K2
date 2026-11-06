//! The API's data types: codecs and capabilities, capture, encoder and
//! decoder settings, and events.
//!
//! Every struct is `#[non_exhaustive]`, so a caller in another crate can't
//! build one with a struct literal: each has a constructor (or `Default`)
//! and public fields to set afterwards.

use crate::error::{invalid, VideoError};
use crate::frame::{PixelFormat, Planes};
use bytes::Bytes;
use std::any::Any;
use std::sync::Arc;

// ───────────────────────────── codecs and capabilities ─────────────────────────────

/// A video codec. More variants (VP9, AV1) are added as minor versions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum VideoCodec {
    /// VP8 (RFC 6386). Encoded frames are raw VP8 frames: the frame tag
    /// first, no IVF/WebM container, one frame per [`EncodedFrame`].
    Vp8,
    /// H.264 / AVC. Encoded frames are one Annex B access unit with 4-byte
    /// start codes, SPS and PPS before every IDR.
    H264,
}

impl VideoCodec {
    /// The SDP / RTP name: `VP8`, `H264`.
    pub fn sdp_name(&self) -> &'static str {
        match self {
            VideoCodec::Vp8 => "VP8",
            VideoCodec::H264 => "H264",
        }
    }
}

/// Encode or decode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CodecDirection {
    /// Encode.
    Encode,
    /// Decode.
    Decode,
}

/// Who runs the codec.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Implementation {
    /// A GPU / media engine.
    Hardware,
    /// The CPU.
    Software,
}

/// The caller's preference when creating a codec.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ImplPreference {
    /// Hardware if one is free, else software.
    #[default]
    PreferHardware,
    /// Hardware or [`VideoError::Unsupported`].
    HardwareOnly,
    /// Software only.
    SoftwareOnly,
}

/// H.264 profiles. Never with B-frames.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum H264Profile {
    /// `profile_idc` 66 with constraint_set1 (`42e0`). Every browser decodes
    /// it.
    #[default]
    ConstrainedBaseline,
    /// `profile_idc` 100 with constraint_set4 and 5 (`640c`): High with no
    /// B-frames.
    ConstrainedHigh,
}

/// An H.264 `level_idc` (31 = 3.1, 40 = 4.0).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct H264Level(pub u8);

impl H264Level {
    /// Level 3.1: up to 1280x720 at 30 fps.
    pub const L3_1: H264Level = H264Level(31);
    /// Level 4.0: up to 1920x1080 at 30 fps.
    pub const L4_0: H264Level = H264Level(40);
    /// Level 5.1: up to 4096x2304 at 30 fps.
    pub const L5_1: H264Level = H264Level(51);

    /// The lowest level whose macroblock rate and frame size cover
    /// `width` x `height` at `fps` (Table A-1), at most 5.2.
    pub fn for_size(width: u32, height: u32, fps: u32) -> H264Level {
        let mbs = (width.div_ceil(16) * height.div_ceil(16)) as u64;
        let rate = mbs * fps.max(1) as u64;
        const TABLE: [(u8, u64, u64); 12] = [
            (10, 1_485, 99),
            (11, 3_000, 396),
            (12, 6_000, 396),
            (13, 11_880, 396),
            (20, 11_880, 396),
            (21, 19_800, 792),
            (22, 20_250, 1_620),
            (30, 40_500, 1_620),
            (31, 108_000, 3_600),
            (32, 216_000, 5_120),
            (40, 245_760, 8_192),
            (51, 983_040, 36_864),
        ];
        for (level, max_rate, max_fs) in TABLE {
            if rate <= max_rate && mbs <= max_fs {
                return H264Level(level);
            }
        }
        H264Level(52)
    }
}

/// H.264 abilities of one codec entry.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct H264Capability {
    /// The profiles it does.
    pub profiles: Vec<H264Profile>,
    /// The highest level.
    pub max_level: H264Level,
}

impl H264Capability {
    /// Profiles and the highest level.
    pub fn new(profiles: Vec<H264Profile>, max_level: H264Level) -> H264Capability {
        H264Capability {
            profiles,
            max_level,
        }
    }
}

/// What one backend can do with one codec in one direction. One entry per
/// (codec, direction, implementation).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct CodecCapability {
    /// The codec.
    pub codec: VideoCodec,
    /// Encode or decode.
    pub direction: CodecDirection,
    /// Hardware or software.
    pub implementation: Implementation,
    /// For logs and stats: "VideoToolbox", "kessel-vp8 0.1", ...
    pub name: String,
    /// The largest coded width.
    pub max_width: u32,
    /// The largest coded height.
    pub max_height: u32,
    /// The highest frame rate at `max_width` x `max_height`.
    pub max_fps: u32,
    /// How many may be open at once on this machine, if known. `None`: no
    /// known limit.
    pub max_instances: Option<u32>,
    /// Encode: the most temporal layers it can mark. 1 = none. Decode: 1.
    pub max_temporal_layers: u8,
    /// H.264 only: profiles and the highest level.
    pub h264: Option<H264Capability>,
    /// Encode: accepted input formats. Decode: formats it can output.
    pub formats: Vec<PixelFormat>,
}

impl CodecCapability {
    /// An entry with no instance limit, no temporal layers, no H.264
    /// details and no formats; set the rest on the result.
    pub fn new(
        codec: VideoCodec,
        direction: CodecDirection,
        implementation: Implementation,
        name: impl Into<String>,
        max_width: u32,
        max_height: u32,
        max_fps: u32,
    ) -> CodecCapability {
        CodecCapability {
            codec,
            direction,
            implementation,
            name: name.into(),
            max_width,
            max_height,
            max_fps,
            max_instances: None,
            max_temporal_layers: 1,
            h264: None,
            formats: Vec::new(),
        }
    }
}

/// Screen capture support.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ScreenCapability {
    /// Any screen capture at all.
    pub supported: bool,
    /// `pick_screen` shows the OS picker.
    pub system_picker: bool,
    /// `screen_sources` lists displays and windows.
    pub list: bool,
    /// Single windows, not only displays.
    pub windows: bool,
    /// The cursor can be drawn into frames.
    pub cursor: bool,
    /// [`CaptureConstraints::exclude_own_windows`] works here.
    pub exclude_own_windows: bool,
    /// Screen frames come from outside the backend, through a source the
    /// app feeds ([`crate::frame_in()`]): on iOS, from the app's broadcast
    /// upload extension. `pick_screen` and `open_screen` answer
    /// `Unsupported`; wait for the app's frames instead of opening a picker.
    pub frame_in: bool,
}

/// The whole capability answer of a backend.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Capabilities {
    /// "apple", "windows", "linux", "android", "fake", "null".
    pub backend: &'static str,
    /// The OS and version, for logs.
    pub os: String,
    /// Codec entries.
    pub codecs: Vec<CodecCapability>,
    /// A camera API is present (cameras may still be zero).
    pub camera: bool,
    /// Screen capture.
    pub screen: ScreenCapability,
}

impl Capabilities {
    /// No codecs, no camera, no screen.
    pub fn new(backend: &'static str, os: impl Into<String>) -> Capabilities {
        Capabilities {
            backend,
            os: os.into(),
            codecs: Vec::new(),
            camera: false,
            screen: ScreenCapability::default(),
        }
    }

    /// The best entry for `codec` in `dir` (hardware first, then the largest
    /// max size).
    pub fn best(&self, codec: VideoCodec, dir: CodecDirection) -> Option<&CodecCapability> {
        self.codecs
            .iter()
            .filter(|c| c.codec == codec && c.direction == dir)
            .max_by_key(|c| {
                (
                    c.implementation == Implementation::Hardware,
                    c.max_width as u64 * c.max_height as u64,
                )
            })
    }
}

// ───────────────────────────── permissions and capture ─────────────────────────────

/// The state of an OS permission.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Permission {
    /// Allowed.
    Granted,
    /// Refused (or the app can't ask: no usage description).
    Denied,
    /// Policy forbids it.
    Restricted,
    /// Never asked.
    NotDetermined,
}

/// Which way a camera faces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Facing {
    /// The person's side (phones, laptops' built-in cameras).
    Front,
    /// The far side (phones).
    Back,
    /// A plugged-in camera.
    External,
    /// The OS doesn't say.
    Unknown,
}

/// One mode a camera offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct CaptureMode {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// The highest frame rate.
    pub max_fps: u32,
    /// What the device delivers in this mode.
    pub format: PixelFormat,
}

impl CaptureMode {
    /// A mode.
    pub fn new(width: u32, height: u32, max_fps: u32, format: PixelFormat) -> CaptureMode {
        CaptureMode {
            width,
            height,
            max_fps,
            format,
        }
    }
}

/// A camera.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct CameraInfo {
    /// Stable across runs where the OS allows.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Which way it faces.
    pub facing: Facing,
    /// The system default.
    pub is_default: bool,
    /// Its modes.
    pub modes: Vec<CaptureMode>,
}

impl CameraInfo {
    /// A camera with no modes listed.
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        facing: Facing,
        is_default: bool,
    ) -> CameraInfo {
        CameraInfo {
            id: id.into(),
            name: name.into(),
            facing,
            is_default,
            modes: Vec::new(),
        }
    }
}

/// What to ask of a capture source. The backend picks the nearest mode at
/// or above the size (then scales down) and at or above `fps` (then drops
/// frames evenly). `None`: the device's default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct CaptureConstraints {
    /// The widest frame wanted.
    pub max_width: Option<u32>,
    /// The tallest frame wanted.
    pub max_height: Option<u32>,
    /// Frames per second.
    pub fps: Option<u32>,
    /// Phones: prefer this camera when no id is given.
    pub facing: Option<Facing>,
    /// Prefer native buffers when an encoder of this backend will take them.
    pub prefer_native: bool,
    /// Screen only: draw the cursor.
    pub cursor: bool,
    /// Display capture only: leave this app's own windows out, where the OS
    /// allows it ([`ScreenCapability::exclude_own_windows`]); otherwise
    /// ignored.
    pub exclude_own_windows: bool,
}

impl CaptureConstraints {
    /// At most `max_width` x `max_height` at `fps`.
    pub fn new(max_width: u32, max_height: u32, fps: u32) -> CaptureConstraints {
        CaptureConstraints {
            max_width: Some(max_width),
            max_height: Some(max_height),
            fps: Some(fps),
            ..CaptureConstraints::default()
        }
    }

    pub(crate) fn validate(&self) -> Result<(), VideoError> {
        for v in [self.max_width, self.max_height].into_iter().flatten() {
            if !(16..=8192).contains(&v) {
                return Err(invalid(format!("a capture size of {v}")));
            }
        }
        if let Some(f) = self.fps {
            if !(1..=240).contains(&f) {
                return Err(invalid(format!("{f} fps")));
            }
        }
        Ok(())
    }
}

/// Pick the mode for `c`: the smallest at or above the wanted size and rate,
/// else the largest. Ties go to the higher rate.
pub fn nearest_mode(modes: &[CaptureMode], c: &CaptureConstraints) -> Option<CaptureMode> {
    let (w, h, fps) = (
        c.max_width.unwrap_or(1280),
        c.max_height.unwrap_or(720),
        c.fps.unwrap_or(30),
    );
    let area = |m: &CaptureMode| m.width as u64 * m.height as u64;
    modes
        .iter()
        .filter(|m| m.width >= w && m.height >= h && m.max_fps >= fps)
        .min_by_key(|m| (area(m), std::cmp::Reverse(m.max_fps)))
        .or_else(|| {
            // Nothing covers it: the largest at the rate, else the largest.
            modes
                .iter()
                .filter(|m| m.max_fps >= fps)
                .max_by_key(|m| (area(m), m.max_fps))
                .or_else(|| modes.iter().max_by_key(|m| (area(m), m.max_fps)))
        })
        .copied()
}

/// Why a source ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum EndReason {
    /// The caller dropped or closed it.
    Closed,
    /// The device went away.
    DeviceLost,
    /// The person stopped it from OS UI.
    UserStopped,
    /// The shared window closed.
    SourceClosed,
    /// The permission was revoked while running.
    Revoked,
}

/// Why a source paused without ending.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Interruption {
    /// Phones: the app went to the background.
    Background,
    /// Another app took the camera.
    InUseByAnother,
    /// Thermal pressure: the OS stopped the camera.
    Thermal,
    /// A screen source is minimised or occluded.
    Hidden,
}

/// A source's events, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SourceEvent {
    /// First frame, and after any change of the delivered size or rate.
    Started {
        /// Delivered width.
        width: u32,
        /// Delivered height.
        height: u32,
        /// Delivered rate.
        fps: u32,
    },
    /// Paused by the OS.
    Interrupted(Interruption),
    /// Back after an interruption.
    Resumed,
    /// Terminal: no more frames; drop the source.
    Ended(EndReason),
    /// A non-terminal problem.
    Error(VideoError),
}

/// A source's counters since open.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct SourceStats {
    /// Frames queued.
    pub frames: u64,
    /// Frames dropped (queue full, or skipped to keep the rate).
    pub dropped: u64,
    /// The delivered width.
    pub width: u32,
    /// The delivered height.
    pub height: u32,
    /// The delivered rate.
    pub fps: u32,
}

/// Display or window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ScreenKind {
    /// A whole display.
    Display,
    /// One window.
    Window,
}

/// A display or window from `screen_sources`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ScreenSourceInfo {
    /// For [`ScreenSelection::Listed`].
    pub id: String,
    /// Display or window.
    pub kind: ScreenKind,
    /// Display name or window title.
    pub title: String,
    /// The owning app, when known.
    pub app: Option<String>,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// A small preview, if cheap (at most 320 px wide, RGBA). Its size is
    /// in `thumbnail_width` and `thumbnail_height`
    /// ([`ScreenSourceInfo::thumbnail_size`]); set all three with
    /// [`ScreenSourceInfo::with_thumbnail`].
    pub thumbnail: Option<Planes>,
    /// The thumbnail's width in pixels, 0 with no thumbnail.
    pub thumbnail_width: u32,
    /// The thumbnail's height in pixels, 0 with no thumbnail.
    pub thumbnail_height: u32,
}

impl ScreenSourceInfo {
    /// A source with no app and no thumbnail.
    pub fn new(
        id: impl Into<String>,
        kind: ScreenKind,
        title: impl Into<String>,
        width: u32,
        height: u32,
    ) -> ScreenSourceInfo {
        ScreenSourceInfo {
            id: id.into(),
            kind,
            title: title.into(),
            app: None,
            width,
            height,
            thumbnail: None,
            thumbnail_width: 0,
            thumbnail_height: 0,
        }
    }

    /// The same source with a `width` x `height` RGBA thumbnail, at most 320
    /// px wide. Errors if the planes aren't RGBA, are wider than that, or
    /// don't hold a frame of that size.
    pub fn with_thumbnail(
        mut self,
        planes: Planes,
        width: u32,
        height: u32,
    ) -> Result<ScreenSourceInfo, VideoError> {
        if planes.format != PixelFormat::Rgba {
            return Err(invalid(format!(
                "a thumbnail is RGBA, not {:?}",
                planes.format
            )));
        }
        if width > 320 {
            return Err(invalid(format!(
                "a thumbnail is at most 320 px wide, not {width}"
            )));
        }
        planes.validate(width, height)?;
        self.thumbnail = Some(planes);
        self.thumbnail_width = width;
        self.thumbnail_height = height;
        Ok(self)
    }

    /// The thumbnail's width and height, when there is a thumbnail and its
    /// size is set.
    pub fn thumbnail_size(&self) -> Option<(u32, u32)> {
        let sized = self.thumbnail_width > 0 && self.thumbnail_height > 0;
        (self.thumbnail.is_some() && sized).then_some((self.thumbnail_width, self.thumbnail_height))
    }
}

/// A chosen screen source: from the list, or the OS picker's opaque handle.
#[derive(Clone)]
#[non_exhaustive]
pub enum ScreenSelection {
    /// An id from `screen_sources`.
    Listed(String),
    /// What the system picker or portal returned.
    Picked(Arc<dyn Any + Send + Sync>),
}

impl std::fmt::Debug for ScreenSelection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScreenSelection::Listed(id) => f.debug_tuple("Listed").field(id).finish(),
            ScreenSelection::Picked(_) => f.write_str("Picked(..)"),
        }
    }
}

/// Options for the OS picker.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct PickOptions {
    /// Offer displays.
    pub displays: bool,
    /// Offer windows.
    pub windows: bool,
    /// The app window that owns the picker. Windows needs it (an `HWND`,
    /// as an integer): the system picker opens over it. Ignored elsewhere.
    pub parent_window: Option<u64>,
}

impl PickOptions {
    /// Displays and windows, no owner window.
    pub fn new() -> PickOptions {
        PickOptions::default()
    }

    /// The window that owns the picker (an `HWND` on Windows).
    pub fn with_parent_window(mut self, handle: u64) -> PickOptions {
        self.parent_window = Some(handle);
        self
    }
}

// ───────────────────────────── encoder ─────────────────────────────

/// What the content is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ContentHint {
    /// Motion: keep the frame rate, lower detail first.
    #[default]
    Camera,
    /// Text and UI: keep detail, lower the frame rate first.
    Screen,
}

/// Per-codec settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CodecConfig {
    /// VP8.
    Vp8,
    /// H.264.
    H264 {
        /// The profile.
        profile: H264Profile,
        /// The level.
        level: H264Level,
    },
}

impl CodecConfig {
    /// The codec.
    pub fn codec(&self) -> VideoCodec {
        match self {
            CodecConfig::Vp8 => VideoCodec::Vp8,
            CodecConfig::H264 { .. } => VideoCodec::H264,
        }
    }
}

/// One encoder instance, one simulcast layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct EncoderConfig {
    /// Codec settings.
    pub codec: CodecConfig,
    /// The coded width. Input frames of any size are scaled to it. Even.
    pub width: u32,
    /// The coded height. Even.
    pub height: u32,
    /// The target frame rate.
    pub fps: u32,
    /// Target bitrate, bits per second.
    pub bitrate_bps: u32,
    /// Hard ceiling for any one second. At least `bitrate_bps`.
    pub max_bitrate_bps: u32,
    /// Keyframe every N frames; `None`: only on `request_keyframe`.
    pub keyframe_interval: Option<u32>,
    /// 1 = no temporal layers; 2 or 3 = L1T2 / L1T3 (VP8 only in v1).
    pub temporal_layers: u8,
    /// Camera or screen.
    pub content: ContentHint,
    /// Hardware or software.
    pub implementation: ImplPreference,
}

impl EncoderConfig {
    /// `codec` at `width` x `height`, `fps`, `bitrate_bps` (max 1.5x),
    /// keyframes on request, one temporal layer, camera, hardware preferred.
    pub fn new(
        codec: CodecConfig,
        width: u32,
        height: u32,
        fps: u32,
        bitrate_bps: u32,
    ) -> EncoderConfig {
        EncoderConfig {
            codec,
            width,
            height,
            fps,
            bitrate_bps,
            max_bitrate_bps: bitrate_bps.saturating_add(bitrate_bps / 2),
            keyframe_interval: None,
            temporal_layers: 1,
            content: ContentHint::default(),
            implementation: ImplPreference::default(),
        }
    }

    /// H.264 Constrained Baseline at the level the size and rate need.
    pub fn h264(width: u32, height: u32, fps: u32, bitrate_bps: u32) -> EncoderConfig {
        let level = H264Level::for_size(width, height, fps).max(H264Level::L3_1);
        EncoderConfig::new(
            CodecConfig::H264 {
                profile: H264Profile::ConstrainedBaseline,
                level,
            },
            width,
            height,
            fps,
            bitrate_bps,
        )
    }

    /// Check the configuration on its own (backends check what they can do).
    pub fn validate(&self) -> Result<(), VideoError> {
        validate_size(self.width, self.height)?;
        if !(1..=240).contains(&self.fps) {
            return Err(invalid(format!("{} fps", self.fps)));
        }
        if self.bitrate_bps < 10_000 {
            return Err(invalid(format!("{} bit/s", self.bitrate_bps)));
        }
        if self.max_bitrate_bps < self.bitrate_bps {
            return Err(invalid(format!(
                "max_bitrate_bps {} is below bitrate_bps {}",
                self.max_bitrate_bps, self.bitrate_bps
            )));
        }
        if self.keyframe_interval == Some(0) {
            return Err(invalid("a keyframe interval of 0 frames"));
        }
        let max_tl = if self.codec.codec() == VideoCodec::H264 {
            1
        } else {
            3
        };
        if self.temporal_layers == 0 || self.temporal_layers > max_tl {
            return Err(invalid(format!(
                "{} temporal layers for {:?}",
                self.temporal_layers,
                self.codec.codec()
            )));
        }
        Ok(())
    }
}

pub(crate) fn validate_size(width: u32, height: u32) -> Result<(), VideoError> {
    if width < 16
        || height < 16
        || width > 8192
        || height > 8192
        || !width.is_multiple_of(2)
        || !height.is_multiple_of(2)
    {
        return Err(invalid(format!(
            "coded size {width}x{height} (even, 16 to 8192)"
        )));
    }
    Ok(())
}

/// A runtime rate change. Never causes a keyframe.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Rates {
    /// Bits per second.
    pub bitrate_bps: u32,
    /// Frames per second.
    pub fps: u32,
}

impl Rates {
    /// Bitrate and frame rate.
    pub fn new(bitrate_bps: u32, fps: u32) -> Rates {
        Rates { bitrate_bps, fps }
    }

    pub(crate) fn validate(&self) -> Result<(), VideoError> {
        if self.bitrate_bps < 10_000 || !(1..=240).contains(&self.fps) {
            return Err(invalid(format!(
                "rates {} bit/s at {} fps",
                self.bitrate_bps, self.fps
            )));
        }
        Ok(())
    }
}

/// Options for one `submit`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct EncodeOptions {
    /// Encode this frame as a keyframe.
    pub keyframe: bool,
}

impl EncodeOptions {
    /// A keyframe.
    pub fn keyframe() -> EncodeOptions {
        EncodeOptions { keyframe: true }
    }
}

/// Where an encoded frame sits in the temporal and spatial structure.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct LayerInfo {
    /// 0 = base.
    pub temporal_id: u8,
    /// RFC 7741's Y bit: depends only on the base layer, or a keyframe.
    pub layer_sync: bool,
    /// Reserved for VP9/AV1 SVC: always 0 in v1.
    pub spatial_id: u8,
}

impl LayerInfo {
    /// A layer.
    pub fn new(temporal_id: u8, layer_sync: bool) -> LayerInfo {
        LayerInfo {
            temporal_id,
            layer_sync,
            spatial_id: 0,
        }
    }
}

/// One encoded frame, plaintext.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct EncodedFrame {
    /// The codec.
    pub codec: VideoCodec,
    /// VP8: one raw VP8 frame. H.264: one Annex B access unit.
    pub data: Bytes,
    /// A keyframe.
    pub keyframe: bool,
    /// The input frame's `capture_us`, unchanged.
    pub capture_us: u64,
    /// The coded width.
    pub width: u32,
    /// The coded height.
    pub height: u32,
    /// Temporal layer.
    pub layer: LayerInfo,
    /// Encode time in µs (stats only).
    pub encode_us: u32,
}

impl EncodedFrame {
    /// A frame in the base layer with no encode time.
    pub fn new(
        codec: VideoCodec,
        data: impl Into<Bytes>,
        keyframe: bool,
        capture_us: u64,
        width: u32,
        height: u32,
    ) -> EncodedFrame {
        EncodedFrame {
            codec,
            data: data.into(),
            keyframe,
            capture_us,
            width,
            height,
            layer: LayerInfo::new(0, true),
            encode_us: 0,
        }
    }
}

/// What an encoder actually runs.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct EncoderInfo {
    /// Hardware or software.
    pub implementation: Implementation,
    /// For logs.
    pub name: String,
    /// The configuration, with the latest rates and size.
    pub config: EncoderConfig,
}

impl EncoderInfo {
    /// Info.
    pub fn new(
        implementation: Implementation,
        name: impl Into<String>,
        config: EncoderConfig,
    ) -> EncoderInfo {
        EncoderInfo {
            implementation,
            name: name.into(),
            config,
        }
    }
}

/// An encoder's counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct EncoderStats {
    /// Frames accepted by `submit`.
    pub frames_in: u64,
    /// Frames out.
    pub frames_out: u64,
    /// Keyframes out.
    pub keyframes: u64,
    /// Inputs rate control skipped.
    pub dropped_by_rc: u64,
    /// `submit` calls answered `Busy`.
    pub busy: u64,
    /// Bytes out.
    pub bytes_out: u64,
}

// ───────────────────────────── decoder ─────────────────────────────

/// One decoder per remote track.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct DecoderConfig {
    /// The codec.
    pub codec: VideoCodec,
    /// Hardware or software.
    pub implementation: ImplPreference,
    /// Output CPU planes in this format, or `Native` to keep OS buffers.
    pub output: PixelFormat,
    /// Scale outputs to fit this box, keeping the aspect ratio; `None`: the
    /// coded size.
    pub fit: Option<(u32, u32)>,
}

impl DecoderConfig {
    /// `codec`, hardware preferred, I420 out at the coded size.
    pub fn new(codec: VideoCodec) -> DecoderConfig {
        DecoderConfig {
            codec,
            implementation: ImplPreference::default(),
            output: PixelFormat::I420,
            fit: None,
        }
    }
}

/// One complete, decrypted, verified frame from the depacketiser.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct EncodedInput {
    /// The codec.
    pub codec: VideoCodec,
    /// VP8: raw frame. H.264: Annex B access unit.
    pub data: Bytes,
    /// The caller's reading of the bitstream (VP8 P bit; an H.264 IDR).
    pub keyframe: bool,
    /// Passed through to the output `VideoFrame::capture_us`.
    pub capture_us: u64,
}

impl EncodedInput {
    /// An input.
    pub fn new(
        codec: VideoCodec,
        data: impl Into<Bytes>,
        keyframe: bool,
        capture_us: u64,
    ) -> EncodedInput {
        EncodedInput {
            codec,
            data: data.into(),
            keyframe,
            capture_us,
        }
    }
}

/// A decoder's counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct DecoderStats {
    /// Inputs accepted.
    pub frames_in: u64,
    /// Frames out.
    pub frames_out: u64,
    /// Decoded frames dropped because the output queue was full.
    pub dropped_out: u64,
    /// Errors (including inputs refused for want of a keyframe).
    pub errors: u64,
    /// Hardware or software, once open.
    pub implementation: Option<Implementation>,
}

// ───────────────────────────── backend events ─────────────────────────────

/// Thermal pressure.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum ThermalLevel {
    /// Normal.
    Nominal,
    /// Slightly raised.
    Fair,
    /// High: shed load.
    Serious,
    /// Critical: shed everything that can go.
    Critical,
}

/// Machine-wide events.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum BackendEvent {
    /// The camera list changed. The full new list.
    Cameras(Vec<CameraInfo>),
    /// Codecs or screen support changed.
    CapabilitiesChanged,
    /// Thermal pressure.
    Thermal(ThermalLevel),
    /// Low-power mode on or off.
    LowPower(bool),
    /// Phones: the app went to the background (`true`) or came back.
    Background(bool),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_follow_table_a1() {
        assert_eq!(H264Level::for_size(320, 180, 30), H264Level(13));
        assert_eq!(H264Level::for_size(640, 360, 30), H264Level(30));
        assert_eq!(H264Level::for_size(1280, 720, 30), H264Level(31));
        assert_eq!(H264Level::for_size(1280, 720, 60), H264Level(32));
        assert_eq!(H264Level::for_size(1920, 1080, 30), H264Level(40));
        assert_eq!(H264Level::for_size(3840, 2160, 30), H264Level(51));
    }

    #[test]
    fn encoder_configs_are_checked() {
        let c = EncoderConfig::h264(1280, 720, 30, 1_500_000);
        assert!(c.validate().is_ok());
        assert_eq!(c.max_bitrate_bps, 2_250_000);
        assert_eq!(
            c.codec,
            CodecConfig::H264 {
                profile: H264Profile::ConstrainedBaseline,
                level: H264Level(31)
            }
        );
        let mut odd = c;
        odd.width = 1281;
        assert!(matches!(odd.validate(), Err(VideoError::Invalid(_))));
        let mut tl = c;
        tl.temporal_layers = 2;
        assert!(tl.validate().is_err(), "H.264 has one temporal layer");
        let mut vp8 = EncoderConfig::new(CodecConfig::Vp8, 640, 360, 30, 500_000);
        vp8.temporal_layers = 3;
        assert!(vp8.validate().is_ok());
        vp8.max_bitrate_bps = 1;
        assert!(vp8.validate().is_err());
        let mut k = c;
        k.keyframe_interval = Some(0);
        assert!(k.validate().is_err());
        assert!(Rates::new(5_000, 30).validate().is_err());
    }

    #[test]
    fn best_prefers_hardware_then_size() {
        let mut caps = Capabilities::new("fake", "test");
        caps.codecs.push(CodecCapability::new(
            VideoCodec::H264,
            CodecDirection::Encode,
            Implementation::Software,
            "sw",
            3840,
            2160,
            30,
        ));
        caps.codecs.push(CodecCapability::new(
            VideoCodec::H264,
            CodecDirection::Encode,
            Implementation::Hardware,
            "hw",
            1920,
            1080,
            30,
        ));
        caps.codecs.push(CodecCapability::new(
            VideoCodec::H264,
            CodecDirection::Decode,
            Implementation::Software,
            "dec",
            1920,
            1080,
            30,
        ));
        assert_eq!(
            caps.best(VideoCodec::H264, CodecDirection::Encode)
                .map(|c| c.name.as_str()),
            Some("hw")
        );
        assert_eq!(
            caps.best(VideoCodec::H264, CodecDirection::Decode)
                .map(|c| c.name.as_str()),
            Some("dec")
        );
        assert!(caps.best(VideoCodec::Vp8, CodecDirection::Encode).is_none());
    }

    #[test]
    fn nearest_mode_covers_the_ask() {
        let f = PixelFormat::Nv12;
        let ms = [
            CaptureMode::new(1920, 1080, 30, f),
            CaptureMode::new(1280, 720, 60, f),
            CaptureMode::new(640, 480, 30, f),
        ];
        let at = |w, h, fps| nearest_mode(&ms, &CaptureConstraints::new(w, h, fps)).expect("one");
        assert_eq!(
            (at(1280, 720, 30).width, at(1280, 720, 30).max_fps),
            (1280, 60)
        );
        assert_eq!(at(640, 360, 30).width, 640);
        assert_eq!(
            at(1920, 1080, 60).width,
            1280,
            "nothing covers 1080p60: the largest at 60"
        );
        assert_eq!(at(4000, 3000, 30).width, 1920);
        assert_eq!(nearest_mode(&[], &CaptureConstraints::default()), None);
        assert!(CaptureConstraints::new(8, 8, 30).validate().is_err());
    }

    #[test]
    fn a_thumbnail_carries_its_size() {
        let info = ScreenSourceInfo::new("d1", ScreenKind::Display, "Main", 2560, 1440);
        assert_eq!(info.thumbnail_size(), None);
        // 160x90 RGBA with rows padded to 768 bytes: the stride alone would
        // say 192 px wide.
        let planes = Planes::new(
            PixelFormat::Rgba,
            160,
            90,
            vec![0u8; 768 * 90],
            [768, 0, 0],
            [0, 0, 0],
        )
        .expect("padded planes");
        let info = info.with_thumbnail(planes, 160, 90).expect("thumbnail");
        assert_eq!(info.thumbnail_size(), Some((160, 90)));
        assert_eq!((info.thumbnail_width, info.thumbnail_height), (160, 90));

        let base = ScreenSourceInfo::new("d1", ScreenKind::Display, "Main", 2560, 1440);
        let bgra = Planes::tight(PixelFormat::Bgra, 4, 4, vec![0u8; 64]).expect("bgra");
        assert!(base.clone().with_thumbnail(bgra, 4, 4).is_err());
        let wide = Planes::tight(PixelFormat::Rgba, 321, 1, vec![0u8; 321 * 4]).expect("wide");
        assert!(base.clone().with_thumbnail(wide, 321, 1).is_err());
        let small = Planes::tight(PixelFormat::Rgba, 4, 4, vec![0u8; 64]).expect("small");
        assert!(base.with_thumbnail(small, 8, 8).is_err());
    }
}
