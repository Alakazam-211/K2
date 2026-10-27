//! The `SimLane` trait: the one seam every device backend implements
//! (`prd-mobile-dev-v1` §3.1, vs-live M-A21).
//!
//! Everything above this trait is lane-blind: the CLI, the tree format
//! ([`super::tree`]), `/cli/sim/stream`, the control hold
//! ([`super::hold`]), the tab and `/cli/sim/notes`. A lane only turns these
//! calls into its own tools (`emulator` + gRPC + adb for `android`;
//! `simctl` + AXe + the frame helper for `ios`; a leased Mac node for
//! `ios-node`).
//!
//! ## Rules every lane keeps
//!
//! - **Blocking calls.** Every method may block (spawn a tool, wait on a
//!   socket). The daemon calls them from `spawn_blocking` / a worker
//!   thread, never on the async runtime. Methods take `&self`; a lane is
//!   shared (`Arc<dyn SimLane>`) and keeps its own interior state.
//! - **Units are points.** Every coordinate and size the trait passes is in
//!   *points*: Android dp, iOS points. The lane converts to pixels itself.
//!   Frames carry their pixel size and the pixel-per-point `scale`.
//! - **K2's own devices only.** A lane never lists, boots, stops or erases
//!   a device it did not create under K2's own folder / device set
//!   (`~/.k2/sim/…`). [`DeviceRef::device_id`] is always one the lane
//!   returned from [`SimLane::boot`] or [`SimLane::adopt`].
//! - **No secrets in argv or logs.** [`SimText`] hides its value from
//!   `Debug`; a lane passes typed text to the device over a socket or stdin,
//!   never on a command line, and never logs it.
//! - **Loopback only.** Any port or helper a lane opens binds 127.0.0.1 and
//!   carries a random per-boot token only the daemon holds.
//! - **Errors teach.** [`LaneError`] variants map 1:1 to the CLI's exit
//!   codes and HTTP statuses (see [`LaneError::http_status`]); the message
//!   is for a person or an agent to act on.

use std::fmt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::tree::RawNode;

// ── lanes ────────────────────────────────────────────────────────────

/// Which backend runs a device. Serialised lowercase-kebab
/// (`android`, `ios`, `ios-node`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LaneKind {
    /// Android emulator, headless, on this box (KVM / Hypervisor.framework).
    Android,
    /// iOS Simulator on a Mac daemon with Xcode.
    Ios,
    /// iOS Simulator on a leased Mac compute node (later; not 0.46.6).
    IosNode,
}

impl LaneKind {
    pub const ALL: [LaneKind; 3] = [LaneKind::Android, LaneKind::Ios, LaneKind::IosNode];

    pub fn as_str(self) -> &'static str {
        match self {
            LaneKind::Android => "android",
            LaneKind::Ios => "ios",
            LaneKind::IosNode => "ios-node",
        }
    }

    pub fn parse(s: &str) -> Option<LaneKind> {
        match s.trim().to_ascii_lowercase().as_str() {
            "android" => Some(LaneKind::Android),
            "ios" => Some(LaneKind::Ios),
            "ios-node" => Some(LaneKind::IosNode),
            _ => None,
        }
    }

    /// The `--platform` a person types (`android` | `ios`). `ios-node` is
    /// still platform `ios`; the core picks `ios` over `ios-node` when both
    /// are usable.
    pub fn platform(self) -> &'static str {
        match self {
            LaneKind::Android => "android",
            LaneKind::Ios | LaneKind::IosNode => "ios",
        }
    }
}

impl fmt::Display for LaneKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What [`SimLane::probe`] found. The core caches it at boot (M-A13) and
/// re-probes on `/cli/sim/status?refresh=1`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaneProbe {
    pub lane: LaneKind,
    /// The lane can run (or can be made to run without an owner step other
    /// than the licence / first-use install). Drives the boot-status key.
    pub usable: bool,
    /// The lane is fully ready to boot now (tools installed, licence
    /// accepted, KVM usable). `usable && !ready` means a first-use step is
    /// pending; [`LaneProbe::needs`] says which.
    pub ready: bool,
    /// Owner or first-use steps still needed, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub needs: Vec<LaneNeed>,
    /// One line for the doctor / status view, e.g.
    /// `"/dev/kvm: permission denied for user k2 (not in group kvm)"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Tool versions found (`emulator`, `adb`, `xcode`, `axe`…).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub versions: Vec<(String, String)>,
}

/// A step between "usable" and "ready".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum LaneNeed {
    /// The box owner must accept a licence on a card (never the agent,
    /// M-A23). `id` is the licence id (e.g. `android-sdk-license`).
    Licence { id: String, title: String },
    /// Tools download on first use (`bytes` is an estimate when known).
    Install {
        what: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bytes: Option<u64>,
    },
    /// An owner step outside K2 (e.g. add the daemon user to `kvm`).
    /// `command` is the exact command to run, for the doctor line.
    OwnerStep { what: String, command: String },
}

/// A device model the lane can boot (`k2 sim list-devices`, the tab's
/// chooser).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceProfile {
    /// Stable id the lane understands, e.g. `pixel_9` or
    /// `com.apple.CoreSimulator.SimDeviceType.iPhone-16`.
    pub id: String,
    /// Shown to people: `"Pixel 9"`, `"iPhone 16"`.
    pub name: String,
    /// Screen size in points (portrait).
    pub width_pt: u32,
    pub height_pt: u32,
    /// Pixels per point.
    pub scale: f32,
    /// OS image the profile boots (`android-35 google_apis x86_64`,
    /// `iOS 27.0`).
    pub os: String,
    /// The lane boots this one when no `--device` is given.
    #[serde(default)]
    pub default: bool,
}

// ── devices ──────────────────────────────────────────────────────────

/// What the core asks a lane to boot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootRequest {
    /// `projects.id` of the workspace that owns the device.
    pub workspace_id: String,
    /// A short filesystem-safe key for the workspace (the lane names its
    /// AVD / device after it, e.g. `k2-<key>`). Stable per workspace.
    pub workspace_key: String,
    /// [`DeviceProfile::id`]; `None` = the lane's default.
    pub profile_id: Option<String>,
    /// Where the lane keeps this workspace's device data
    /// (`~/.k2/sim/<lane>/<workspace_key>/`). The core creates it.
    pub data_dir: PathBuf,
    /// How long the lane may take to reach "booted" before failing.
    pub boot_timeout: Duration,
}

/// What a lane returns once a device is up.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BootedDevice {
    /// The lane's handle for the device: AVD name + console port for
    /// Android (`k2-ab12cd@5554`), UDID for iOS. Opaque to the core; it is
    /// stored and handed back in [`DeviceRef`].
    pub device_id: String,
    pub profile: DeviceProfile,
    /// OS process id of the device process, when the lane has one (the
    /// reaper and adopt use it; `None` for a node lease).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// Lane-private state the lane needs to re-attach after a daemon
    /// restart (ports, gRPC token file path…). Never shown to clients;
    /// never holds a secret value (a token lives in a 0600 file the lane
    /// names here).
    #[serde(default)]
    pub lane_state: serde_json::Value,
}

/// A booted device, as the core hands it back to the lane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRef {
    pub workspace_id: String,
    pub workspace_key: String,
    pub device_id: String,
    pub data_dir: PathBuf,
    pub lane_state: serde_json::Value,
}

/// What an install did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallOutcome {
    /// The package name / bundle id read from the artifact.
    pub app_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// True when an older copy was replaced (`adb install -r`).
    #[serde(default)]
    pub replaced: bool,
}

// ── input ────────────────────────────────────────────────────────────

/// Text typed into the device. `Debug` never shows the value, so a stray
/// `{:?}` can't leak a vault secret (M-A20).
#[derive(Clone, PartialEq, Eq)]
pub struct SimText {
    value: String,
    /// Came from the vault (`type --secret`). The lane must also keep it
    /// out of any screenshot it saves while the field has focus.
    pub secret: bool,
}

impl SimText {
    pub fn plain(s: impl Into<String>) -> SimText {
        SimText { value: s.into(), secret: false }
    }
    pub fn secret(s: impl Into<String>) -> SimText {
        SimText { value: s.into(), secret: true }
    }
    /// The value, for the lane's input channel only.
    pub fn expose(&self) -> &str {
        &self.value
    }
    pub fn len_chars(&self) -> usize {
        self.value.chars().count()
    }
}

impl fmt::Debug for SimText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.secret {
            write!(f, "SimText(secret, {} chars)", self.len_chars())
        } else {
            write!(f, "SimText({} chars)", self.len_chars())
        }
    }
}

/// A hardware or system button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Button {
    Home,
    /// Android only; an iOS lane answers [`LaneError::Unsupported`].
    Back,
    Lock,
    VolumeUp,
    VolumeDown,
    /// Android app switcher / iOS app switcher.
    AppSwitch,
    /// Shake (iOS) / menu key (Android): opens the React Native dev menu.
    DevMenu,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Appearance {
    Light,
    Dark,
}

/// A point on the screen, in points, origin top-left of the current
/// orientation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

/// A keyboard key that is not text (the stream's key events; `k2 sim`
/// sends text through [`Action::Type`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Key {
    Enter,
    Backspace,
    Delete,
    Tab,
    Escape,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
}

/// One input or device action. The core resolves element ids to points
/// before calling the lane, so lanes only see coordinates.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Tap { at: Point },
    /// Press for `hold` (long press).
    LongPress { at: Point, hold: Duration },
    /// A straight drag from `from` to `to` over `duration`.
    Swipe { from: Point, to: Point, duration: Duration },
    /// Raw touch phases from a person's pointer on the stream (down, move…,
    /// up). Lanes that can't stream phases may buffer until `Up` and replay
    /// as a tap / swipe.
    Touch { phase: TouchPhase, at: Point },
    /// Type text into whatever has focus.
    Type { text: SimText },
    Key { key: Key },
    Button { button: Button },
    /// Rotate to an absolute orientation.
    Rotate { orientation: Orientation },
    OpenUrl { url: String },
    Location { lat: f64, lon: f64 },
    Appearance { appearance: Appearance },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TouchPhase {
    Down,
    Move,
    Up,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Orientation {
    Portrait,
    LandscapeLeft,
    LandscapeRight,
    PortraitUpsideDown,
}

impl Action {
    /// True for actions that change what is on the screen (the control
    /// hold applies to these; reads never wait).
    pub fn is_input(&self) -> bool {
        true
    }

    /// Short verb for audit rows and the action line (`tap`, `type`…).
    pub fn verb(&self) -> &'static str {
        match self {
            Action::Tap { .. } => "tap",
            Action::LongPress { .. } => "long-press",
            Action::Swipe { .. } => "swipe",
            Action::Touch { .. } => "touch",
            Action::Type { .. } => "type",
            Action::Key { .. } => "key",
            Action::Button { .. } => "button",
            Action::Rotate { .. } => "rotate",
            Action::OpenUrl { .. } => "open-url",
            Action::Location { .. } => "location",
            Action::Appearance { .. } => "appearance",
        }
    }
}

// ── tree ─────────────────────────────────────────────────────────────

/// One screen's accessibility tree as the lane read it. The core turns it
/// into a [`super::tree::Snapshot`] (ids, interactive filter, diff); lanes
/// never assign ids.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawTree {
    /// Screen size in points for the current orientation.
    pub width_pt: f32,
    pub height_pt: f32,
    /// The foreground app (package / bundle id), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    /// The screen / window title, when the lane can tell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Top-level windows / roots, in z-order (back to front).
    pub roots: Vec<RawNode>,
    /// How long the lane took to read it (the tab shows nothing; the spike
    /// and `--timing` do).
    #[serde(default)]
    pub took_ms: u64,
}

// ── frames ───────────────────────────────────────────────────────────

/// Pixel data of one frame.
#[derive(Clone, PartialEq, Eq)]
pub enum FrameData {
    /// Tightly packed RGB, 3 bytes per pixel (the emulator's RGB888).
    Rgb888(Vec<u8>),
    /// Tightly packed RGBA, 4 bytes per pixel.
    Rgba8888(Vec<u8>),
    /// Already a JPEG (serve-sim, a Mac node): the encoder passes it
    /// through as-is, with no re-encode (PRD §7).
    Jpeg(Vec<u8>),
    /// A PNG (screenshots; the encoder decodes it if it must stream it).
    Png(Vec<u8>),
}

impl fmt::Debug for FrameData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FrameData::Rgb888(b) => write!(f, "Rgb888({} B)", b.len()),
            FrameData::Rgba8888(b) => write!(f, "Rgba8888({} B)", b.len()),
            FrameData::Jpeg(b) => write!(f, "Jpeg({} B)", b.len()),
            FrameData::Png(b) => write!(f, "Png({} B)", b.len()),
        }
    }
}

/// One captured frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub width_px: u32,
    pub height_px: u32,
    /// Pixels per point for this frame (so viewers map clicks to points).
    pub scale: f32,
    pub orientation: Orientation,
    /// Monotonic per source; a gap means the lane skipped frames.
    pub seq: u64,
    pub captured_at: Instant,
    pub data: FrameData,
}

/// What a frame source should aim for. A lane may ignore what it can't
/// honour; the shared encoder still caps per viewer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameOpts {
    /// Highest rate any current viewer wants.
    pub max_fps: u32,
    /// Ask the lane to scale down to at most this width in pixels (the
    /// encoder streams at points resolution, PRD §7). `None` = native.
    pub max_width_px: Option<u32>,
}

impl Default for FrameOpts {
    fn default() -> Self {
        FrameOpts { max_fps: 30, max_width_px: None }
    }
}

/// A live capture. The shared encoder (one per device, S3) owns it and
/// pulls; dropping it stops the capture and frees the lane's resources.
pub trait FrameSource: Send {
    /// The newest frame that differs from the last one returned, waiting
    /// up to `wait`. `Ok(None)` = the screen did not change in that time
    /// (a still screen costs nothing). An error ends the source; the
    /// encoder may ask the lane for a new one.
    fn next_frame(&mut self, wait: Duration) -> Result<Option<Frame>, LaneError>;

    /// Change the target rate / size while running (a viewer joined or
    /// left). Default: ignored.
    fn retune(&mut self, _opts: FrameOpts) {}
}

// ── logs ─────────────────────────────────────────────────────────────

/// `k2 sim logs`. The core enforces the caps again and redacts every line
/// (`k2-redact`) before anything leaves the daemon; a lane should stop
/// reading once it has `max_lines` / `max_bytes`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogQuery {
    /// Only this app's lines (package / bundle id).
    pub app_id: Option<String>,
    /// Only lines newer than this.
    pub since: Option<Duration>,
    /// Plain substring filter (not a regex; the core passes it as data).
    pub grep: Option<String>,
    pub max_lines: usize,
    pub max_bytes: usize,
}

// ── errors ───────────────────────────────────────────────────────────

/// Why a lane call failed. Each variant has one exit code and one HTTP
/// status, so the CLI and the tab teach the same thing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum LaneError {
    /// The lane can't run here (no KVM, no Xcode, wrong OS). 503.
    Unavailable { message: String },
    /// A first-use / owner step is pending ([`LaneNeed`]). 409, exit 3.
    NeedsSetup { message: String, needs: Vec<LaneNeed> },
    /// The device is not running (or died). 409.
    NotRunning { message: String },
    /// The verb doesn't exist on this lane (Back on iOS). 400.
    Unsupported { message: String },
    /// Bad input (missing APK, bad URL, point off-screen). 400.
    Invalid { message: String },
    /// Waited too long (boot, tree, install). 504.
    Timeout { message: String },
    /// A tool failed. `message` is redacted, bounded tool output. 502.
    Tool { message: String },
}

impl LaneError {
    pub fn message(&self) -> &str {
        match self {
            LaneError::Unavailable { message }
            | LaneError::NeedsSetup { message, .. }
            | LaneError::NotRunning { message }
            | LaneError::Unsupported { message }
            | LaneError::Invalid { message }
            | LaneError::Timeout { message }
            | LaneError::Tool { message } => message,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            LaneError::Unavailable { .. } => "sim_unavailable",
            LaneError::NeedsSetup { .. } => "sim_needs_setup",
            LaneError::NotRunning { .. } => "sim_not_running",
            LaneError::Unsupported { .. } => "sim_unsupported",
            LaneError::Invalid { .. } => "sim_invalid",
            LaneError::Timeout { .. } => "sim_timeout",
            LaneError::Tool { .. } => "sim_tool_failed",
        }
    }

    pub fn http_status(&self) -> u16 {
        match self {
            LaneError::Unavailable { .. } => 503,
            LaneError::NeedsSetup { .. } => 409,
            LaneError::NotRunning { .. } => 409,
            LaneError::Unsupported { .. } => 400,
            LaneError::Invalid { .. } => 400,
            LaneError::Timeout { .. } => 504,
            LaneError::Tool { .. } => 502,
        }
    }

    pub fn tool(m: impl Into<String>) -> LaneError {
        LaneError::Tool { message: m.into() }
    }
    pub fn invalid(m: impl Into<String>) -> LaneError {
        LaneError::Invalid { message: m.into() }
    }
    pub fn not_running(m: impl Into<String>) -> LaneError {
        LaneError::NotRunning { message: m.into() }
    }
    pub fn unsupported(m: impl Into<String>) -> LaneError {
        LaneError::Unsupported { message: m.into() }
    }
    pub fn timeout(m: impl Into<String>) -> LaneError {
        LaneError::Timeout { message: m.into() }
    }
    pub fn unavailable(m: impl Into<String>) -> LaneError {
        LaneError::Unavailable { message: m.into() }
    }
}

impl fmt::Display for LaneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code(), self.message())
    }
}

impl std::error::Error for LaneError {}

// ── the trait ────────────────────────────────────────────────────────

/// One device backend. See the module docs for the rules.
pub trait SimLane: Send + Sync + 'static {
    fn kind(&self) -> LaneKind;

    /// Can this lane run here, and is it ready? Cheap-ish (file checks,
    /// `--version`); never downloads, never boots, never accepts a
    /// licence.
    fn probe(&self) -> LaneProbe;

    /// First-use setup the owner has approved: download tools / images
    /// into K2's folder, verifying every archive's hash. The core calls
    /// this only after the licence card is accepted by the box owner
    /// (`licence_accepted` lists the accepted licence ids). Idempotent;
    /// `progress` gets `(done_bytes, total_bytes)`.
    fn install(
        &self,
        licence_accepted: &[String],
        progress: &(dyn Fn(u64, u64) + Sync),
    ) -> Result<(), LaneError>;

    /// The devices this lane can boot, default first.
    fn profiles(&self) -> Result<Vec<DeviceProfile>, LaneError>;

    /// Create (if needed) and boot the workspace's device, headless. Returns
    /// once the OS reports boot complete. The core has already checked the
    /// box limits, the disk guard and that this workspace has no running
    /// device.
    fn boot(&self, req: &BootRequest) -> Result<BootedDevice, LaneError>;

    /// After a daemon restart: is this recorded device still running? Return
    /// it (possibly with refreshed `lane_state`) to keep it, or `None` when
    /// it is gone. Must not boot anything.
    fn adopt(&self, dev: &DeviceRef) -> Result<Option<BootedDevice>, LaneError>;

    /// Shut the device down, keeping its data.
    fn stop(&self, dev: &DeviceRef) -> Result<(), LaneError>;

    /// Delete the workspace's device data (the device is already stopped).
    /// Only paths under `data_dir` / K2's own device set.
    fn erase(&self, workspace_key: &str, data_dir: &std::path::Path) -> Result<(), LaneError>;

    /// Install an app (`.apk` / `.app`); the path is inside the workspace
    /// (the core checks).
    fn install_app(&self, dev: &DeviceRef, artifact: &std::path::Path) -> Result<InstallOutcome, LaneError>;

    fn launch(&self, dev: &DeviceRef, app_id: &str) -> Result<(), LaneError>;

    fn terminate(&self, dev: &DeviceRef, app_id: &str) -> Result<(), LaneError>;

    /// Read the current screen's accessibility tree.
    fn tree(&self, dev: &DeviceRef) -> Result<RawTree, LaneError>;

    /// Do one action. Returns when the device accepted it (not when the
    /// app finished reacting; the core waits / re-reads the tree for the
    /// diff).
    fn act(&self, dev: &DeviceRef, action: &Action) -> Result<(), LaneError>;

    /// One full-resolution still (PNG or raw). The core writes the file.
    fn screenshot(&self, dev: &DeviceRef) -> Result<Frame, LaneError>;

    /// Start a live capture for the shared encoder.
    fn frames(&self, dev: &DeviceRef, opts: FrameOpts) -> Result<Box<dyn FrameSource>, LaneError>;

    /// Bounded log lines (oldest first), not yet redacted.
    fn logs(&self, dev: &DeviceRef, q: &LogQuery) -> Result<Vec<String>, LaneError>;

    /// Is the device process still alive? Cheap; the reaper calls it every
    /// sweep. Default: `adopt` returned something.
    fn alive(&self, dev: &DeviceRef) -> bool {
        matches!(self.adopt(dev), Ok(Some(_)))
    }
}
