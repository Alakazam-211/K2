//! `IosLane`: the iOS Simulator on a Mac daemon (PRD §3.3, L-I), behind the
//! shared [`SimLane`] trait.
//!
//! simctl does device lifecycle, apps, URLs, location, appearance, shake and
//! logs, always with `--set` K2's own device set (`~/.k2/sim/ios/set`). The
//! helper does frames, input and the accessibility tree. Devices are named
//! `K2-<workspace_key>--<model>`. The core owns the box limits, the idle
//! reaper and the records; this lane only runs devices.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value as J};

use super::helper::{self, FrameSlot, HelperIo, HelperProc};
use super::simctl::{self, device_name, workspace_of, DeviceState, ProcessRunner, Runner, SimDevice, SimError, SimResult, Simctl, DEVICE_PREFIX};
use super::tree;
use crate::sim::lane::*;

/// Xcode versions the helper has been run against end to end (the live
/// test). Others are tried, and the helper refuses clearly if a private
/// API it needs is gone; doctor shows this list.
pub const TESTED_XCODE: &[&str] = &["26.5 (17F42)", "26.6 (17F113)"];

/// The Darwin notification the Simulator's own Shake menu posts.
const SHAKE: &str = "com.apple.UIKit.SimulatorShake";

#[derive(Debug, Clone)]
pub struct IosLaneConfig {
    /// K2's own device set.
    pub set_path: PathBuf,
    /// Where the compiled helper lives.
    pub helper_dir: PathBuf,
}

impl IosLaneConfig {
    /// The standard layout under the sim home (`~/.k2/sim`).
    pub fn under(sim_home: &Path) -> Self {
        let ios = sim_home.join("ios");
        Self { set_path: ios.join("set"), helper_dir: ios.join("bin") }
    }
}

/// Starts a helper for a booted device: `(set, udid, xcode version)`.
pub type HelperFactory = Box<dyn Fn(&Path, &str, &str) -> SimResult<Arc<dyn HelperIo>> + Send + Sync>;

#[derive(Default)]
struct Live {
    helper: Option<Arc<dyn HelperIo>>,
    /// Screen in points and pixels per point, from the helper's hello.
    screen: (f32, f32, f32),
    app_id: Option<String>,
    /// Where the last tap landed: what `Type` types into when nothing reports focus.
    last_tap: Option<Point>,
    /// A person's touch-down point, for raw `Touch` phases.
    touch_down: Option<(Point, Instant)>,
    /// bundle id → executable, from the installs this lane did (log predicates).
    executables: HashMap<String, String>,
}

pub struct IosLane {
    cfg: IosLaneConfig,
    simctl: Simctl,
    live: Mutex<HashMap<String, Live>>,
    factory: HelperFactory,
    xcode: Mutex<Option<String>>,
}

const TREE_TIMEOUT: Duration = Duration::from_secs(30);
const INPUT_TIMEOUT: Duration = Duration::from_secs(20);

/// Internal error → the shared error, keeping the teaching message.
pub fn lane_err(e: SimError) -> LaneError {
    let m = e.message;
    match e.code {
        "unsupported_xcode" => LaneError::unavailable(format!("sim_ios_unsupported_xcode: {m} K2's helper is tested with Xcode {}.", TESTED_XCODE.join(", "))),
        "unsupported_os" | "no_xcode" | "no_runtime" | "no_swiftc" | "helper_build_failed" => LaneError::unavailable(m),
        "timeout" | "helper_timeout" => LaneError::timeout(m),
        "no_device" | "not_booted" | "helper_gone" | "helper_exited" => LaneError::not_running(m),
        "unsupported" => LaneError::unsupported(m),
        "wrong_platform" | "bad_app" | "bad_request" | "bad_profile" | "unmappable" | "stale_element" | "no_app" => LaneError::invalid(m),
        _ => LaneError::tool(m),
    }
}

trait Lane<T> {
    fn lane(self) -> Result<T, LaneError>;
}
impl<T> Lane<T> for SimResult<T> {
    fn lane(self) -> Result<T, LaneError> {
        self.map_err(lane_err)
    }
}

impl IosLane {
    /// The real lane: `xcrun` for simctl, the compiled helper for the rest.
    pub fn new(cfg: IosLaneConfig) -> Self {
        let runner: Arc<dyn Runner> = Arc::new(ProcessRunner);
        let helper_dir = cfg.helper_dir.clone();
        let build_runner = runner.clone();
        let factory: HelperFactory = Box::new(move |set, udid, xcode| {
            let bin = helper::ensure_built(&helper_dir, build_runner.as_ref(), xcode)?;
            Ok(Arc::new(HelperProc::spawn(&bin, set, udid)?) as Arc<dyn HelperIo>)
        });
        Self::with(cfg, runner, factory)
    }

    /// The lane with injected seams (tests).
    pub fn with(cfg: IosLaneConfig, runner: Arc<dyn Runner>, factory: HelperFactory) -> Self {
        let simctl = Simctl::new(cfg.set_path.clone(), runner);
        Self { cfg, simctl, live: Mutex::new(HashMap::new()), factory, xcode: Mutex::new(None) }
    }

    pub fn config(&self) -> &IosLaneConfig {
        &self.cfg
    }

    fn ensure_set(&self) -> SimResult<()> {
        std::fs::create_dir_all(&self.cfg.set_path).map_err(|e| SimError::new("tool", format!("cannot create {}: {e}", self.cfg.set_path.display())))
    }

    /// `xcodebuild -version` → "26.5 (17F42)", cached.
    pub fn xcode_version(&self) -> SimResult<String> {
        if let Some(v) = self.xcode.lock().unwrap().clone() {
            return Ok(v);
        }
        let out = self.simctl.runner().run("xcrun", &["xcodebuild".into(), "-version".into()], Duration::from_secs(30))?;
        if out.status != 0 {
            return Err(SimError::new("no_xcode", "Xcode isn't ready on this Mac (xcodebuild -version failed). Install Xcode and open it once."));
        }
        let v = parse_xcode_version(&out.stdout).ok_or_else(|| SimError::new("no_xcode", format!("unrecognised xcodebuild -version output: {:?}", out.stdout.trim())))?;
        *self.xcode.lock().unwrap() = Some(v.clone());
        Ok(v)
    }

    /// K2's devices (by name prefix; the set is K2's own anyway).
    fn k2_devices(&self) -> SimResult<Vec<SimDevice>> {
        self.ensure_set()?;
        Ok(self.simctl.devices()?.into_iter().filter(|d| d.name.starts_with(DEVICE_PREFIX)).collect())
    }

    /// The device, which must be one of K2's.
    fn owned(&self, udid: &str) -> SimResult<SimDevice> {
        self.k2_devices()?
            .into_iter()
            .find(|d| d.udid == udid)
            .ok_or_else(|| SimError::new("no_device", format!("{udid} isn't one of K2's devices any more. Run k2 sim boot.")))
    }

    /// The live helper for a booted device, started on first need.
    fn helper(&self, udid: &str) -> SimResult<Arc<dyn HelperIo>> {
        if let Some(h) = self.live.lock().unwrap().get(udid).and_then(|l| l.helper.clone()) {
            if h.alive() {
                return Ok(h);
            }
        }
        let d = self.owned(udid)?;
        if d.state != DeviceState::Booted {
            return Err(SimError::new("not_booted", format!("{} isn't running. Run k2 sim boot.", d.name)));
        }
        let xcode = self.xcode_version()?;
        let h = (self.factory)(self.simctl.set_path(), udid, &xcode)?;
        let hello = h.call("hello", json!({}), Duration::from_secs(20))?;
        let f = |p: &str| hello.pointer(p).and_then(J::as_f64).unwrap_or(0.0) as f32;
        let mut live = self.live.lock().unwrap();
        let l = live.entry(udid.to_string()).or_default();
        l.helper = Some(h.clone());
        l.screen = (f("/screen/width"), f("/screen/height"), f("/screen/scale"));
        Ok(h)
    }

    fn screen(&self, udid: &str) -> (f32, f32, f32) {
        self.live.lock().unwrap().get(udid).map(|l| l.screen).unwrap_or_default()
    }

    /// The newest runtime with the profile (a device-type id or name), or the default.
    fn pick(&self, profile: Option<&str>) -> SimResult<(simctl::Runtime, simctl::DeviceType)> {
        let runtimes = self.simctl.ios_runtimes()?;
        let newest = runtimes.first().ok_or_else(no_runtime)?;
        match profile {
            Some(p) => {
                for r in &runtimes {
                    if let Some(t) = r.device_types.iter().find(|t| t.identifier == p || t.name.eq_ignore_ascii_case(p)) {
                        return Ok((r.clone(), t.clone()));
                    }
                }
                let names: Vec<&str> = newest.device_types.iter().filter(|t| t.family == "iPhone").map(|t| t.name.as_str()).collect();
                Err(SimError::new("bad_profile", format!("No iOS device {p:?}. Try one of: {}", names.join(", "))))
            }
            None => default_type(newest).map(|t| (newest.clone(), t)).ok_or_else(no_runtime),
        }
    }

    /// Screen size from the device type's profile.plist: (w_pt, h_pt, scale).
    fn screen_of(&self, t: &simctl::DeviceType) -> Option<(u32, u32, f32)> {
        let plist = Path::new(&t.bundle_path).join("Contents/Resources/profile.plist");
        let args: Vec<String> = vec!["-convert".into(), "json".into(), "-o".into(), "-".into(), plist.to_string_lossy().into_owned()];
        let out = self.simctl.runner().run("/usr/bin/plutil", &args, Duration::from_secs(10)).ok()?;
        let v: J = serde_json::from_str(&out.stdout).ok()?;
        let scale = v.get("mainScreenScale")?.as_f64()?;
        let w = v.get("mainScreenWidth")?.as_f64()? / scale;
        let h = v.get("mainScreenHeight")?.as_f64()? / scale;
        Some((w.round() as u32, h.round() as u32, scale as f32))
    }

    fn profile(&self, r: &simctl::Runtime, t: &simctl::DeviceType, default: bool) -> DeviceProfile {
        let (w, h, s) = self.screen_of(t).unwrap_or((0, 0, 0.0));
        DeviceProfile { id: t.identifier.clone(), name: t.name.clone(), width_pt: w, height_pt: h, scale: s, os: r.name.clone(), default }
    }

    /// Type text as a keyboard would, checked against the field it went
    /// into; falls back to setting the value through accessibility when
    /// the keyboard path can't (a character with no US key, or HID input
    /// not arriving, as Xcode 27's Device Hub may cause, M-A17). A secret
    /// is always set, never typed key by key.
    fn type_text(&self, h: &dyn HelperIo, hint: Option<Point>, text: &str, secret: bool) -> SimResult<&'static str> {
        let mut args = json!({});
        if let Some(p) = hint {
            args = json!({"x": p.x, "y": p.y});
        }
        let target = match h.call("focus", args, INPUT_TIMEOUT) {
            Ok(v) => v.get("frame").cloned().filter(|f| f.as_array().map_or(false, |a| a.len() == 4)),
            Err(e) if e.code == "stale_element" => None,
            Err(e) => return Err(e),
        };
        let Some(frame) = target else {
            if secret {
                return Err(SimError::new("bad_request", "Nothing has focus to put the secret in. Tap the field first (k2 sim tap e3), then type --secret."));
            }
            return match h.call("type", json!({"text": text}), INPUT_TIMEOUT) {
                Ok(_) => Ok("typed"),
                Err(e) if e.code == "unmappable" => Err(SimError::new("unmappable", format!("{} Tap the field first so K2 can set the text directly.", e.message))),
                Err(e) => Err(e),
            };
        };
        let read = |h: &dyn HelperIo| -> SimResult<(String, bool)> {
            let v = h.call("value", json!({"frame": frame}), INPUT_TIMEOUT)?;
            let secure = v.get("subrole").and_then(J::as_str) == Some("AXSecureTextField");
            let val = tree::effective_value(v.get("value").and_then(J::as_str).map(str::to_string), v.get("placeholder").and_then(J::as_str));
            Ok((val.unwrap_or_default(), secure))
        };
        let (old, secure) = read(h)?;
        // A secure field's value reads as bullets: append to nothing we can see.
        let want = if secure { text.to_string() } else { format!("{old}{text}") };
        if secret {
            h.call("set_value", json!({"frame": frame, "text": want}), INPUT_TIMEOUT)?;
            return Ok("set");
        }
        match h.call("type", json!({"text": text}), INPUT_TIMEOUT) {
            Ok(_) => {}
            Err(e) if e.code == "unmappable" => {
                h.call("set_value", json!({"frame": frame, "text": want}), INPUT_TIMEOUT)?;
                return Ok("set");
            }
            Err(e) => return Err(e),
        }
        if secure {
            return Ok("typed"); // bullets can't be compared; the keystrokes went in
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut seen = old.clone();
        while Instant::now() < deadline {
            seen = read(h)?.0;
            if seen == want {
                return Ok("typed");
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if seen == old {
            // No keystroke arrived: the HID keyboard path is down. Set it instead.
            h.call("set_value", json!({"frame": frame, "text": want}), INPUT_TIMEOUT)?;
            let after = read(h)?.0;
            if after == want {
                return Ok("set");
            }
            return Err(SimError::new("type_failed", format!("Neither typing nor setting the value reached the field (it shows {after:?}).")));
        }
        Err(SimError::new(
            "type_incomplete",
            format!("Typed {} characters but the field shows {seen:?}; the app may be rewriting input. Read k2 sim tree.", text.chars().count()),
        ))
    }

    /// A person typing on the stream: key events straight away, no focus
    /// lookup, no read-back. Text with a character the US HID keyboard
    /// can't type (an IME commit, `é`) goes in through the pasteboard; the
    /// helper checks the whole string first, so nothing is half-typed.
    fn person_keys(&self, h: &dyn HelperIo, udid: &str, text: &str) -> SimResult<&'static str> {
        match h.call("type", json!({ "text": text }), INPUT_TIMEOUT) {
            Ok(_) => Ok("typed"),
            Err(e) if e.code == "unmappable" => self.paste(h, udid, text).map(|_| "pasted"),
            Err(e) => Err(e),
        }
    }

    /// The device pasteboard gets `text` (simctl pbcopy, over stdin), then
    /// ⌘V on the HID keyboard pastes it into the focused field.
    fn paste(&self, h: &dyn HelperIo, udid: &str, text: &str) -> SimResult<()> {
        self.simctl.pbcopy(udid, text)?;
        h.call("key", paste_args(), INPUT_TIMEOUT).map(|_| ())
    }

    fn set_last_tap(&self, udid: &str, p: Point) {
        self.live.lock().unwrap().entry(udid.to_string()).or_default().last_tap = Some(p);
    }

    fn check_point(&self, udid: &str, p: Point) -> SimResult<()> {
        let (w, h, _) = self.screen(udid);
        if w > 0.0 && (p.x < 0.0 || p.y < 0.0 || p.x > w || p.y > h) {
            return Err(SimError::new("bad_request", format!("{},{} is off the {w}x{h} pt screen", p.x, p.y)));
        }
        Ok(())
    }

    fn booted(&self, d: &SimDevice) -> SimResult<BootedDevice> {
        let (r, t) = match self.simctl.ios_runtimes()?.into_iter().find(|r| r.identifier == d.runtime) {
            Some(r) => {
                let t = r.device_types.iter().find(|t| t.identifier == d.device_type).cloned();
                (r, t)
            }
            None => (simctl::Runtime { identifier: d.runtime.clone(), name: d.runtime.clone(), version: String::new(), device_types: vec![] }, None),
        };
        let profile = match t {
            Some(t) => self.profile(&r, &t, false),
            None => DeviceProfile { id: d.device_type.clone(), name: model_of(&d.name), width_pt: 0, height_pt: 0, scale: 0.0, os: r.name.clone(), default: false },
        };
        Ok(BootedDevice { device_id: d.udid.clone(), profile, pid: None, lane_state: json!({"name": d.name}) })
    }
}

fn parse_xcode_version(s: &str) -> Option<String> {
    let mut lines = s.lines();
    let ver = lines.next()?.strip_prefix("Xcode ")?.trim().to_string();
    let build = lines.find_map(|l| l.strip_prefix("Build version ")).map(str::trim);
    Some(match build {
        Some(b) => format!("{ver} ({b})"),
        None => ver,
    })
}

fn no_runtime() -> SimError {
    SimError::new("no_runtime", "No iOS Simulator runtime is installed. Install one in Xcode (Settings → Components) or run: xcodebuild -downloadPlatform iOS")
}

/// The plainest current iPhone ("iPhone 17" over "iPhone 17 Pro").
fn default_type(r: &simctl::Runtime) -> Option<simctl::DeviceType> {
    let phones: Vec<&simctl::DeviceType> = r.device_types.iter().filter(|t| t.family == "iPhone").collect();
    let plain = phones
        .iter()
        .filter_map(|t| t.name.strip_prefix("iPhone ").and_then(|n| n.parse::<u32>().ok()).map(|n| (n, *t)))
        .max_by_key(|(n, _)| *n)
        .map(|(_, t)| t.clone());
    plain.or_else(|| phones.first().map(|t| (*t).clone()))
}

fn model_of(name: &str) -> String {
    name.split_once("--").map(|(_, m)| m.replace('-', " ")).unwrap_or_default()
}

/// Bundle ids go into a log predicate: keep them to the characters bundle ids use.
fn checked_app_id(id: &str) -> SimResult<&str> {
    if !id.is_empty() && id.len() <= 255 && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')) {
        Ok(id)
    } else {
        Err(SimError::new("bad_request", format!("{id:?} isn't a bundle id")))
    }
}

/// Reads one key from an Info.plist with `plutil` (binary or XML).
fn plist_string(runner: &dyn Runner, plist: &Path, key: &str) -> Option<String> {
    let args: Vec<String> = vec!["-extract".into(), key.into(), "raw".into(), "-o".into(), "-".into(), plist.to_string_lossy().into_owned()];
    let out = runner.run("/usr/bin/plutil", &args, Duration::from_secs(10)).ok()?;
    let v = out.stdout.trim().to_string();
    (out.status == 0 && !v.is_empty()).then_some(v)
}

/// HID keyboard usages for the non-text keys.
fn key_usage(k: Key) -> u32 {
    match k {
        Key::Enter => 0x28,
        Key::Escape => 0x29,
        Key::Backspace => 0x2A,
        Key::Tab => 0x2B,
        Key::Delete => 0x4C,
        Key::ArrowRight => 0x4F,
        Key::ArrowLeft => 0x50,
        Key::ArrowDown => 0x51,
        Key::ArrowUp => 0x52,
    }
}

/// HID usage of a lowercase letter (`a` = 0x04).
fn letter_usage(c: char) -> u32 {
    debug_assert!(c.is_ascii_lowercase(), "{c:?}");
    c as u32 - 'a' as u32 + 0x04
}

/// The helper's `key` request for a named key with Shift / Option.
fn key_args(k: Key, m: KeyMods) -> J {
    json!({ "usage": key_usage(k), "shift": m.shift, "alt": m.alt })
}

/// ⌘<letter> (⇧⌘Z for redo): what an iPhone with a hardware keyboard
/// takes for editing.
fn edit_args(op: EditOp) -> J {
    let (c, shift) = op.chord();
    json!({ "usage": letter_usage(c), "shift": shift, "cmd": true })
}

/// ⌘V.
fn paste_args() -> J {
    json!({ "usage": letter_usage('v'), "cmd": true })
}

/// Pulls the helper's newest JPEG; dropping it stops the capture.
struct IosFrames {
    helper: Arc<dyn HelperIo>,
    slot: Arc<FrameSlot>,
    width_pt: f32,
    native_px: f32,
}

impl IosFrames {
    fn configure(&self, opts: FrameOpts) -> SimResult<()> {
        let scale = match opts.max_width_px {
            Some(w) if self.native_px > 0.0 => (w as f32 / self.native_px).clamp(0.05, 1.0),
            _ => 1.0,
        };
        self.helper.call("frames", json!({"on": true, "fps": opts.max_fps.clamp(1, 60), "scale": scale, "quality": 0.7}), INPUT_TIMEOUT).map(|_| ())
    }
}

impl FrameSource for IosFrames {
    fn next_frame(&mut self, wait: Duration) -> Result<Option<Frame>, LaneError> {
        match self.slot.take(wait) {
            Ok(Some(f)) => Ok(Some(Frame {
                width_px: f.width,
                height_px: f.height,
                scale: if self.width_pt > 0.0 { f.width as f32 / self.width_pt } else { 1.0 },
                orientation: Orientation::Portrait,
                seq: f.seq,
                captured_at: Instant::now(),
                data: FrameData::Jpeg(f.jpeg),
            })),
            Ok(None) => Ok(None),
            Err(()) => Err(LaneError::not_running("The Simulator helper stopped; the device may have shut down.")),
        }
    }

    fn retune(&mut self, opts: FrameOpts) {
        let _ = self.configure(opts);
    }
}

impl Drop for IosFrames {
    fn drop(&mut self) {
        let _ = self.helper.call("frames", json!({"on": false}), Duration::from_secs(5));
        self.helper.frame_slot(None);
    }
}

impl SimLane for IosLane {
    fn kind(&self) -> LaneKind {
        LaneKind::Ios
    }

    fn probe(&self) -> LaneProbe {
        let mut p = LaneProbe { lane: LaneKind::Ios, usable: false, ready: false, needs: vec![], detail: None, versions: vec![] };
        if !cfg!(target_os = "macos") {
            p.detail = Some("The iOS Simulator needs a Mac with Xcode.".into());
            return p;
        }
        let xcode = match self.xcode_version() {
            Ok(v) => v,
            Err(e) => {
                p.detail = Some(e.message);
                p.needs.push(LaneNeed::OwnerStep { what: "Install Xcode from the App Store and open it once".into(), command: "xcode-select --install".into() });
                return p;
            }
        };
        p.versions.push(("xcode".into(), xcode.clone()));
        p.versions.push(("xcode-tested".into(), TESTED_XCODE.join(", ")));
        let runtimes = match self.simctl.ios_runtimes() {
            Ok(r) => r,
            Err(e) => {
                p.detail = Some(e.message);
                return p;
            }
        };
        p.usable = true;
        match runtimes.first() {
            Some(r) => p.versions.push(("ios-runtime".into(), r.name.clone())),
            None => p.needs.push(LaneNeed::OwnerStep { what: "Install an iOS Simulator runtime".into(), command: "xcodebuild -downloadPlatform iOS".into() }),
        }
        let swiftc = self.simctl.runner().run("xcrun", &["--find".into(), "swiftc".into()], Duration::from_secs(30));
        if !matches!(swiftc, Ok(ref o) if o.status == 0) {
            p.needs.push(LaneNeed::OwnerStep { what: "Install Xcode's command-line tools (K2 builds its Simulator helper with them)".into(), command: "xcode-select --install".into() });
        }
        p.ready = p.needs.is_empty();
        let tested = TESTED_XCODE.contains(&xcode.as_str());
        p.detail = Some(if tested {
            format!("Xcode {xcode}; K2's Simulator helper builds on first use")
        } else {
            format!("Xcode {xcode} is untested (tested: {}); K2 checks the private APIs it needs at start and refuses clearly if one is gone", TESTED_XCODE.join(", "))
        });
        p
    }

    fn install(&self, _licence_accepted: &[String], progress: &(dyn Fn(u64, u64) + Sync)) -> Result<(), LaneError> {
        // Nothing to download: build the helper now so the first boot is quick.
        let xcode = self.xcode_version().lane()?;
        progress(0, 1);
        helper::ensure_built(&self.cfg.helper_dir, self.simctl.runner().as_ref(), &xcode).lane()?;
        progress(1, 1);
        Ok(())
    }

    fn profiles(&self) -> Result<Vec<DeviceProfile>, LaneError> {
        let runtimes = self.simctl.ios_runtimes().lane()?;
        let newest = runtimes.first().ok_or_else(no_runtime).lane()?;
        let default = default_type(newest).map(|t| t.identifier);
        let mut out: Vec<DeviceProfile> = newest
            .device_types
            .iter()
            .filter(|t| t.family == "iPhone") // tablets later (PRD §2.2)
            .map(|t| self.profile(newest, t, Some(&t.identifier) == default.as_ref()))
            .collect();
        out.sort_by_key(|p| !p.default);
        Ok(out)
    }

    fn boot(&self, req: &BootRequest) -> Result<BootedDevice, LaneError> {
        if simctl::fold(&req.workspace_key).is_empty() {
            return Err(LaneError::invalid("boot needs a workspace key"));
        }
        let (runtime, dtype) = self.pick(req.profile_id.as_deref()).lane()?;
        let name = device_name(&req.workspace_key, &dtype.name);
        let devices = self.k2_devices().lane()?;
        let udid = match devices.iter().find(|d| d.name == name) {
            Some(d) => d.udid.clone(),
            None => self.simctl.create(&name, &dtype.identifier, &runtime.identifier).lane()?,
        };
        let started = Instant::now();
        self.simctl.boot(&udid).lane()?;
        let left = req.boot_timeout.saturating_sub(started.elapsed()).max(Duration::from_secs(5));
        self.simctl.bootstatus(&udid, left).lane()?;
        self.live.lock().unwrap().insert(udid.clone(), Live::default());
        let d = self.owned(&udid).lane()?;
        let mut b = self.booted(&d).lane()?;
        b.profile = self.profile(&runtime, &dtype, b.profile.default);
        Ok(b)
    }

    fn adopt(&self, dev: &DeviceRef) -> Result<Option<BootedDevice>, LaneError> {
        let d = match self.k2_devices().lane()?.into_iter().find(|d| d.udid == dev.device_id) {
            Some(d) if d.state == DeviceState::Booted => d,
            _ => return Ok(None),
        };
        self.live.lock().unwrap().entry(d.udid.clone()).or_default();
        self.booted(&d).lane().map(Some)
    }

    fn stop(&self, dev: &DeviceRef) -> Result<(), LaneError> {
        self.owned(&dev.device_id).lane()?;
        // Dropping the helper closes its stdin; it also exits when the device stops.
        self.live.lock().unwrap().remove(&dev.device_id);
        self.simctl.shutdown(&dev.device_id).lane()
    }

    fn erase(&self, workspace_key: &str, data_dir: &Path) -> Result<(), LaneError> {
        let ws = simctl::fold(workspace_key);
        if ws.is_empty() {
            return Err(LaneError::invalid("erase needs a workspace key"));
        }
        for d in self.k2_devices().lane()?.into_iter().filter(|d| workspace_of(&d.name) == Some(ws.as_str())) {
            self.live.lock().unwrap().remove(&d.udid);
            if d.state != DeviceState::Shutdown {
                self.simctl.shutdown(&d.udid).lane()?;
            }
            self.simctl.delete(&d.udid).lane()?;
        }
        if data_dir.exists() {
            std::fs::remove_dir_all(data_dir).map_err(|e| LaneError::tool(format!("cannot remove {}: {e}", data_dir.display())))?;
        }
        Ok(())
    }

    fn install_app(&self, dev: &DeviceRef, artifact: &Path) -> Result<InstallOutcome, LaneError> {
        let plist = artifact.join("Info.plist");
        if artifact.extension().and_then(|e| e.to_str()) != Some("app") || !artifact.is_dir() || !plist.is_file() {
            return Err(LaneError::invalid(format!(
                "{} isn't an iOS .app bundle (a folder ending in .app with an Info.plist). Build for the Simulator, then pass the .app from the build folder.",
                artifact.display()
            )));
        }
        let runner = self.simctl.runner().clone();
        let app_id = plist_string(runner.as_ref(), &plist, "CFBundleIdentifier").ok_or_else(|| LaneError::invalid(format!("{} has no CFBundleIdentifier", plist.display())))?;
        checked_app_id(&app_id).lane()?;
        let version = plist_string(runner.as_ref(), &plist, "CFBundleShortVersionString");
        let exe = plist_string(runner.as_ref(), &plist, "CFBundleExecutable");
        self.owned(&dev.device_id).lane()?;
        let replaced = self.simctl.has_app(&dev.device_id, &app_id).lane()?;
        self.simctl.install(&dev.device_id, artifact).lane()?;
        if let Some(exe) = exe {
            self.live.lock().unwrap().entry(dev.device_id.clone()).or_default().executables.insert(app_id.clone(), exe);
        }
        Ok(InstallOutcome { app_id, version, replaced })
    }

    fn launch(&self, dev: &DeviceRef, app_id: &str) -> Result<(), LaneError> {
        let app_id = checked_app_id(app_id).lane()?;
        self.simctl.launch(&dev.device_id, app_id).lane()?;
        self.live.lock().unwrap().entry(dev.device_id.clone()).or_default().app_id = Some(app_id.to_string());
        Ok(())
    }

    fn terminate(&self, dev: &DeviceRef, app_id: &str) -> Result<(), LaneError> {
        self.simctl.terminate(&dev.device_id, checked_app_id(app_id).lane()?).lane()
    }

    fn tree(&self, dev: &DeviceRef) -> Result<RawTree, LaneError> {
        let t = Instant::now();
        let h = self.helper(&dev.device_id).lane()?;
        let reply = h.call("tree", json!({"grid": "auto"}), TREE_TIMEOUT).lane()?;
        let root = tree::from_helper(&reply).lane()?;
        let (w, hh, _) = self.screen(&dev.device_id);
        let app_id = self.live.lock().unwrap().get(&dev.device_id).and_then(|l| l.app_id.clone());
        Ok(RawTree { width_pt: w, height_pt: hh, app_id, title: None, roots: vec![root], took_ms: t.elapsed().as_millis() as u64 })
    }

    fn act(&self, dev: &DeviceRef, action: &Action) -> Result<(), LaneError> {
        let id = dev.device_id.as_str();
        match action {
            Action::OpenUrl { url } => self.simctl.open_url(id, url).lane(),
            Action::Location { lat, lon } => {
                if !(-90.0..=90.0).contains(lat) || !(-180.0..=180.0).contains(lon) {
                    return Err(LaneError::invalid(format!("{lat},{lon} isn't a latitude,longitude")));
                }
                self.simctl.location(id, *lat, *lon).lane()
            }
            Action::Appearance { appearance } => self.simctl.appearance(id, *appearance == Appearance::Dark).lane(),
            Action::Rotate { orientation: Orientation::Portrait } => Ok(()),
            Action::Rotate { .. } => Err(LaneError::unsupported("Rotating isn't available on the iOS lane yet; the device stays in portrait.")),
            Action::Button { button } => match button {
                Button::Home => self.helper(id).lane()?.call("button", json!({"name": "home"}), INPUT_TIMEOUT).map(|_| ()).lane(),
                Button::Lock => self.helper(id).lane()?.call("button", json!({"name": "lock"}), INPUT_TIMEOUT).map(|_| ()).lane(),
                Button::DevMenu => self.simctl.notify(id, SHAKE).lane(),
                Button::Back => Err(LaneError::unsupported("iPhones have no Back button. Tap the app's own back control (find it with k2 sim tree), or swipe right from the left edge.")),
                Button::VolumeUp | Button::VolumeDown | Button::AppSwitch => Err(LaneError::unsupported(format!("{button:?} isn't available on the iOS lane yet."))),
            },
            Action::Tap { at } => {
                let h = self.helper(id).lane()?;
                self.check_point(id, *at).lane()?;
                h.call("tap", json!({"x": at.x, "y": at.y}), INPUT_TIMEOUT).lane()?;
                self.set_last_tap(id, *at);
                Ok(())
            }
            Action::LongPress { at, hold } => {
                let h = self.helper(id).lane()?;
                self.check_point(id, *at).lane()?;
                h.call("tap", json!({"x": at.x, "y": at.y, "hold_ms": hold.as_millis() as u64}), INPUT_TIMEOUT).lane()?;
                self.set_last_tap(id, *at);
                Ok(())
            }
            Action::Swipe { from, to, duration } => {
                let h = self.helper(id).lane()?;
                self.check_point(id, *from).lane()?;
                self.check_point(id, *to).lane()?;
                h.call("swipe", json!({"x1": from.x, "y1": from.y, "x2": to.x, "y2": to.y, "ms": duration.as_millis() as u64}), INPUT_TIMEOUT).map(|_| ()).lane()
            }
            Action::Touch { phase, at } => {
                let h = self.helper(id).lane()?;
                let down = !matches!(phase, TouchPhase::Up);
                h.call("touch", json!({"down": down, "x": at.x, "y": at.y}), INPUT_TIMEOUT).lane()?;
                let mut live = self.live.lock().unwrap();
                let l = live.entry(id.to_string()).or_default();
                match phase {
                    TouchPhase::Down => l.touch_down = Some((*at, Instant::now())),
                    TouchPhase::Move => {}
                    TouchPhase::Up => {
                        // A short, still press is a tap: remember where, for Type.
                        if let Some((p, _)) = l.touch_down.take() {
                            if (p.x - at.x).abs() < 10.0 && (p.y - at.y).abs() < 10.0 {
                                l.last_tap = Some(*at);
                            }
                        }
                    }
                }
                Ok(())
            }
            // The helper sends one finger; pinch waits until two-finger
            // Indigo messages are proven on a real Simulator.
            Action::TwoTouch { .. } => Err(LaneError::unsupported("Pinch isn't available on this device yet.")),
            Action::Key { key, mods } => self.helper(id).lane()?.call("key", key_args(*key, *mods), INPUT_TIMEOUT).map(|_| ()).lane(),
            Action::Edit { op } => self.helper(id).lane()?.call("key", edit_args(*op), INPUT_TIMEOUT).map(|_| ()).lane(),
            Action::Keystrokes { text } => {
                let h = self.helper(id).lane()?;
                self.person_keys(h.as_ref(), id, text.expose()).map(|_| ()).lane()
            }
            Action::Paste { text } => {
                let h = self.helper(id).lane()?;
                self.paste(h.as_ref(), id, text.expose()).lane()
            }
            Action::Type { text } => {
                let h = self.helper(id).lane()?;
                let hint = self.live.lock().unwrap().get(id).and_then(|l| l.last_tap);
                self.type_text(h.as_ref(), hint, text.expose(), text.secret).map(|_| ()).lane()
            }
        }
    }

    fn screenshot(&self, dev: &DeviceRef) -> Result<Frame, LaneError> {
        let h = self.helper(&dev.device_id).lane()?;
        std::fs::create_dir_all(&dev.data_dir).map_err(|e| LaneError::tool(format!("cannot create {}: {e}", dev.data_dir.display())))?;
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let path = dev.data_dir.join(format!(".shot-{}-{nanos}.png", std::process::id()));
        let v = h.call("image", json!({"path": path.to_string_lossy(), "scale": 1.0}), INPUT_TIMEOUT).lane();
        let bytes = std::fs::read(&path);
        let _ = std::fs::remove_file(&path);
        let v = v?;
        let bytes = bytes.map_err(|e| LaneError::tool(format!("the helper wrote no screenshot: {e}")))?;
        let (wpt, _, scale) = self.screen(&dev.device_id);
        let w = v.get("width").and_then(J::as_u64).unwrap_or(0) as u32;
        Ok(Frame {
            width_px: w,
            height_px: v.get("height").and_then(J::as_u64).unwrap_or(0) as u32,
            scale: if wpt > 0.0 { w as f32 / wpt } else { scale },
            orientation: Orientation::Portrait,
            seq: 0,
            captured_at: Instant::now(),
            data: FrameData::Png(bytes),
        })
    }

    fn frames(&self, dev: &DeviceRef, opts: FrameOpts) -> Result<Box<dyn FrameSource>, LaneError> {
        let h = self.helper(&dev.device_id).lane()?;
        let (w, _, scale) = self.screen(&dev.device_id);
        let slot = Arc::new(FrameSlot::default());
        h.frame_slot(Some(slot.clone()));
        let src = IosFrames { helper: h.clone(), slot, width_pt: w, native_px: w * scale };
        src.configure(opts).lane()?;
        Ok(Box::new(src))
    }

    fn logs(&self, dev: &DeviceRef, q: &LogQuery) -> Result<Vec<String>, LaneError> {
        let predicate = match &q.app_id {
            Some(id) => {
                let id = checked_app_id(id).lane()?;
                let exe = self.live.lock().unwrap().get(&dev.device_id).and_then(|l| l.executables.get(id).cloned());
                let process = exe.filter(|e| checked_app_id(e).is_ok()).unwrap_or_else(|| id.rsplit('.').next().unwrap_or(id).to_string());
                Some(format!("subsystem BEGINSWITH \"{id}\" OR process == \"{process}\""))
            }
            None => None,
        };
        self.owned(&dev.device_id).lane()?;
        let since = q.since.unwrap_or(Duration::from_secs(120)).as_secs();
        let raw = self.simctl.log_show(&dev.device_id, since, predicate.as_deref()).lane()?;
        Ok(bound_logs(&raw, q))
    }

    fn alive(&self, dev: &DeviceRef) -> bool {
        matches!(self.k2_devices(), Ok(ds) if ds.iter().any(|d| d.udid == dev.device_id && d.state == DeviceState::Booted))
    }
}

/// Drops `log show`'s headers, applies `grep`, and keeps the most recent
/// lines inside both caps, oldest first. (The core redacts.)
pub fn bound_logs(raw: &str, q: &LogQuery) -> Vec<String> {
    let needle = q.grep.as_ref().map(|g| g.to_lowercase());
    let lines: Vec<&str> = raw
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with("Timestamp") && !l.starts_with("Filtering the log data"))
        .filter(|l| needle.as_ref().map_or(true, |n| l.to_lowercase().contains(n)))
        .collect();
    let mut kept: Vec<String> = Vec::new();
    let mut bytes = 0usize;
    for l in lines.iter().rev() {
        if kept.len() >= q.max_lines || bytes + l.len() + 1 > q.max_bytes {
            break;
        }
        bytes += l.len() + 1;
        kept.push(l.to_string());
    }
    kept.reverse();
    kept
}

#[cfg(test)]
mod tests {
    use super::super::simctl::{FakeRunner, Output};
    use super::*;
    use std::sync::Mutex as M;

    const RUNTIMES: &str = r#"{"runtimes":[{"identifier":"com.apple.CoreSimulator.SimRuntime.iOS-26-5","name":"iOS 26.5","version":"26.5","platform":"iOS","isAvailable":true,
      "supportedDeviceTypes":[{"name":"iPhone 17 Pro","identifier":"t17p","productFamily":"iPhone","bundlePath":"/p/17p"},{"name":"iPhone 17","identifier":"t17","productFamily":"iPhone","bundlePath":"/p/17"},
      {"name":"iPhone 16","identifier":"t16","productFamily":"iPhone","bundlePath":"/p/16"},{"name":"iPad Air 11-inch (M3)","identifier":"tpad","productFamily":"iPad","bundlePath":"/p/pad"}]}]}"#;

    /// A simctl stand-in with a little device state.
    struct World {
        devices: M<Vec<(String, String, &'static str)>>, // udid, name, state
        next: M<u32>,
    }

    fn lane_with(world: Arc<World>, xcode: &'static str) -> (IosLane, Arc<FakeRunner>) {
        let w = world.clone();
        let fake = Arc::new(FakeRunner {
            calls: M::new(vec![]),
            answer: Box::new(move |prog, a| {
                let ok = |s: String| Ok(Output { status: 0, stdout: s, stderr: String::new() });
                if prog == "/usr/bin/plutil" {
                    if a[0] == "-convert" {
                        return ok(r#"{"mainScreenWidth":1206,"mainScreenHeight":2622,"mainScreenScale":3}"#.into());
                    }
                    return ok(match a[1].as_str() {
                        "CFBundleIdentifier" => "dev.k2.simfixture".into(),
                        "CFBundleExecutable" => "K2Fixture".into(),
                        "CFBundleShortVersionString" => "1.0".into(),
                        _ => String::new(),
                    });
                }
                if a[0] == "xcodebuild" {
                    return ok(format!("Xcode {xcode}\nBuild version 17F42\n"));
                }
                if a[0] == "--find" || a[0] == "simctl" && a.get(1).map(String::as_str) == Some("help") {
                    return ok("/x\n".into());
                }
                let verb = a.get(3).map(String::as_str).unwrap_or("");
                let mut devs = w.devices.lock().unwrap();
                let set_state = |devs: &mut Vec<(String, String, &'static str)>, id: &str, s: &'static str| {
                    for d in devs.iter_mut() {
                        if d.0 == id {
                            d.2 = s;
                        }
                    }
                };
                match (verb, a.get(5).map(String::as_str)) {
                    ("list", Some("runtimes")) => ok(RUNTIMES.into()),
                    ("list", Some("devices")) => {
                        let list: Vec<J> = devs
                            .iter()
                            .map(|(u, n, s)| json!({"udid": u, "name": n, "state": s, "isAvailable": true, "deviceTypeIdentifier": "t17"}))
                            .collect();
                        ok(json!({"devices": {"com.apple.CoreSimulator.SimRuntime.iOS-26-5": list}}).to_string())
                    }
                    ("create", _) => {
                        let mut n = w.next.lock().unwrap();
                        *n += 1;
                        let u = format!("00000000-0000-0000-0000-{:012}", *n);
                        devs.push((u.clone(), a[4].clone(), "Shutdown"));
                        ok(format!("{u}\n"))
                    }
                    ("boot", _) => {
                        set_state(&mut devs, &a[4], "Booted");
                        ok(String::new())
                    }
                    ("shutdown", _) => {
                        set_state(&mut devs, &a[4], "Shutdown");
                        ok(String::new())
                    }
                    ("delete", _) => {
                        devs.retain(|d| d.0 != a[4]);
                        ok(String::new())
                    }
                    ("get_app_container", _) => Ok(Output { status: 1, stdout: String::new(), stderr: "not installed".into() }),
                    ("spawn", _) => ok("Timestamp               Ty Process[PID:TID]\n2026-10-27 a K2Fixture: hello\n2026-10-27 b K2Fixture: saved\n".into()),
                    _ => ok(String::new()),
                }
            }),
        });
        let cfg = IosLaneConfig { set_path: std::env::temp_dir().join(format!("k2-ios-lane-set-{}", std::process::id())), helper_dir: PathBuf::from("/k2/sim/ios/bin") };
        let factory: HelperFactory = Box::new(|_, _, _| Err(SimError::new("no_helper_in_this_test", "")));
        (IosLane::with(cfg, fake.clone(), factory), fake)
    }

    fn world() -> Arc<World> {
        Arc::new(World { devices: M::new(vec![("USER-DEVICE".into(), "iPhone 17".into(), "Booted")]), next: M::new(0) })
    }

    fn req(key: &str, profile: Option<&str>) -> BootRequest {
        BootRequest {
            workspace_id: format!("id-{key}"),
            workspace_key: key.into(),
            profile_id: profile.map(str::to_string),
            data_dir: std::env::temp_dir().join(format!("k2-ios-data-{key}-{}", std::process::id())),
            boot_timeout: Duration::from_secs(60),
        }
    }

    fn dref(b: &BootedDevice, key: &str) -> DeviceRef {
        let r = req(key, None);
        DeviceRef { workspace_id: r.workspace_id, workspace_key: key.into(), device_id: b.device_id.clone(), data_dir: r.data_dir, lane_state: b.lane_state.clone() }
    }

    #[test]
    fn boot_creates_a_named_device_in_k2s_set_and_reuses_it() {
        let w = world();
        let (lane, fake) = lane_with(w.clone(), "26.5");
        let d = lane.boot(&req("ab12", None)).unwrap();
        assert_eq!((d.profile.name.as_str(), d.profile.id.as_str(), d.profile.os.as_str()), ("iPhone 17", "t17", "iOS 26.5"));
        assert_eq!((d.profile.width_pt, d.profile.height_pt, d.profile.scale), (402, 874, 3.0));
        let again = lane.boot(&req("ab12", None)).unwrap();
        assert_eq!(again.device_id, d.device_id);
        let creates = fake.calls.lock().unwrap().iter().filter(|(_, a)| a.get(3).map(String::as_str) == Some("create")).count();
        assert_eq!(creates, 1);
        assert!(w.devices.lock().unwrap().iter().any(|x| x.1 == "K2-ab12--iPhone-17"));
        for (prog, a) in fake.calls.lock().unwrap().iter().filter(|(p, a)| p == "xcrun" && a[0] == "simctl") {
            assert_eq!(&a[..2], &["simctl", "--set"], "{prog} {a:?}");
            assert_eq!(a[2], lane.config().set_path.to_string_lossy(), "{a:?}");
        }
        let pro = lane.boot(&req("ab12", Some("t17p"))).unwrap();
        assert_ne!(pro.device_id, d.device_id, "a different model is its own device");
        assert_eq!(lane_err_code(lane.boot(&req("ab12", Some("Nokia 3310"))).unwrap_err()), "sim_invalid");
    }

    fn lane_err_code(e: LaneError) -> &'static str {
        e.code()
    }

    #[test]
    fn the_persons_own_simulators_are_never_touched() {
        let (lane, _) = lane_with(world(), "26.5");
        let foreign = DeviceRef { workspace_id: "x".into(), workspace_key: "x".into(), device_id: "USER-DEVICE".into(), data_dir: PathBuf::from("/nope"), lane_state: J::Null };
        assert_eq!(lane.stop(&foreign).unwrap_err().code(), "sim_not_running");
        assert_eq!(lane.adopt(&foreign).unwrap(), None);
        assert!(!lane.alive(&foreign));
        assert_eq!(lane.install_app(&foreign, Path::new("/tmp/none.app")).unwrap_err().code(), "sim_invalid");
    }

    #[test]
    fn adopt_stop_and_erase_follow_the_device() {
        let w = world();
        let (lane, fake) = lane_with(w.clone(), "26.5");
        let b = lane.boot(&req("ws1", None)).unwrap();
        let r = dref(&b, "ws1");
        assert!(lane.alive(&r));
        assert_eq!(lane.adopt(&r).unwrap().unwrap().device_id, b.device_id);
        lane.stop(&r).unwrap();
        assert!(!lane.alive(&r));
        assert_eq!(lane.adopt(&r).unwrap(), None, "a stopped device isn't adopted");
        assert!(w.devices.lock().unwrap().iter().any(|d| d.0 == b.device_id), "stop keeps the device and its data");
        std::fs::create_dir_all(&r.data_dir).unwrap();
        lane.erase("ws1", &r.data_dir).unwrap();
        assert!(!w.devices.lock().unwrap().iter().any(|d| d.0 == b.device_id));
        assert!(!r.data_dir.exists());
        assert!(w.devices.lock().unwrap().iter().any(|d| d.0 == "USER-DEVICE"), "erase never touches other devices");
        assert!(!fake.calls.lock().unwrap().iter().any(|(_, a)| a.iter().any(|x| x == "USER-DEVICE")));
    }

    #[test]
    fn install_checks_the_bundle_and_reads_its_id() {
        let (lane, _) = lane_with(world(), "26.5");
        let b = lane.boot(&req("ws2", None)).unwrap();
        let r = dref(&b, "ws2");
        let tmp = std::env::temp_dir().join(format!("k2-ios-install-{}", std::process::id()));
        let app = tmp.join("K2Fixture.app");
        std::fs::create_dir_all(&app).unwrap();
        assert_eq!(lane.install_app(&r, &app).unwrap_err().code(), "sim_invalid", "no Info.plist");
        std::fs::write(app.join("Info.plist"), "<plist/>").unwrap();
        let got = lane.install_app(&r, &app).unwrap();
        assert_eq!((got.app_id.as_str(), got.version.as_deref(), got.replaced), ("dev.k2.simfixture", Some("1.0"), false));
        assert_eq!(lane.install_app(&r, &tmp).unwrap_err().code(), "sim_invalid", "not a .app");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn logs_are_bounded_and_keep_the_newest() {
        let raw = "Timestamp  Ty Process\nFiltering the log data using \"x\"\nl1 boot\nl2 Saved\nl3 saved again\nl4 other\n";
        let q = |grep: Option<&str>, lines: usize, bytes: usize| LogQuery { app_id: None, since: None, grep: grep.map(str::to_string), max_lines: lines, max_bytes: bytes };
        assert_eq!(bound_logs(raw, &q(Some("SAVED"), 1, 1000)), vec!["l3 saved again".to_string()]);
        assert_eq!(bound_logs(raw, &q(None, 100, 1000)).len(), 4);
        assert_eq!(bound_logs(raw, &q(None, 100, 9)), vec!["l4 other".to_string()]);
    }

    #[test]
    fn log_predicates_refuse_injection() {
        let (lane, _) = lane_with(world(), "26.5");
        let b = lane.boot(&req("ws3", None)).unwrap();
        let r = dref(&b, "ws3");
        let q = |app: &str| LogQuery { app_id: Some(app.into()), since: None, grep: None, max_lines: 50, max_bytes: 10_000 };
        assert_eq!(lane.logs(&r, &q("x\" OR 1==1 OR \"")).unwrap_err().code(), "sim_invalid");
        assert_eq!(lane.launch(&r, "a;rm").unwrap_err().code(), "sim_invalid");
        assert_eq!(lane.logs(&r, &q("dev.k2.simfixture")).unwrap().len(), 2);
    }

    #[test]
    fn ios_has_no_back_button_and_says_what_to_do() {
        let (lane, _) = lane_with(world(), "26.5");
        let r = DeviceRef { workspace_id: "x".into(), workspace_key: "x".into(), device_id: "x".into(), data_dir: PathBuf::new(), lane_state: J::Null };
        let e = lane.act(&r, &Action::Button { button: Button::Back }).unwrap_err();
        assert_eq!(e.code(), "sim_unsupported");
        assert!(e.message().contains("k2 sim tree"));
        assert!(lane.act(&r, &Action::Rotate { orientation: Orientation::Portrait }).is_ok());
    }

    #[test]
    fn probe_names_xcode_and_flags_an_untested_one() {
        let (lane, _) = lane_with(world(), "26.5");
        let p = lane.probe();
        if cfg!(target_os = "macos") {
            assert!(p.usable && p.ready, "{p:?}");
            assert!(p.versions.contains(&("xcode".to_string(), "26.5 (17F42)".to_string())), "{p:?}");
            assert!(p.detail.as_deref().unwrap().contains("first use"));
            let (lane, _) = lane_with(world(), "99.0");
            let p = lane.probe();
            assert!(p.detail.as_deref().unwrap().contains("untested"), "{p:?}");
        } else {
            assert!(!p.usable);
        }
    }

    #[test]
    fn unsupported_xcode_is_a_clear_unavailable_error() {
        let e = lane_err(SimError::new("unsupported_xcode", "Xcode 28.0 (1A1) lacks what K2's Simulator helper needs: AXPTranslator."));
        assert_eq!(e.code(), "sim_unavailable");
        assert!(e.message().starts_with("sim_ios_unsupported_xcode:") && e.message().contains("28.0") && e.message().contains("26.5"), "{}", e.message());
        assert_eq!(lane_err(SimError::new("no_swiftc", "x")).code(), "sim_unavailable");
        assert_eq!(lane_err(SimError::new("helper_timeout", "x")).code(), "sim_timeout");
    }

    #[test]
    fn xcode_versions_parse() {
        assert_eq!(parse_xcode_version("Xcode 26.5\nBuild version 17F42\n").as_deref(), Some("26.5 (17F42)"));
        assert_eq!(parse_xcode_version("Xcode 27.0\n").as_deref(), Some("27.0"));
        assert_eq!(parse_xcode_version("garbage"), None);
    }

    /// A helper that plays a text field: typing appends unless `hid_dead`.
    struct FakeField {
        value: M<String>,
        hid_dead: bool,
        focused: bool,
        calls: M<Vec<String>>,
    }
    impl HelperIo for FakeField {
        fn call(&self, op: &str, args: J, _t: Duration) -> SimResult<J> {
            self.calls.lock().unwrap().push(op.to_string());
            match op {
                "focus" if self.focused || args.get("x").is_some() => Ok(json!({"ok": true, "role": "AXTextField", "frame": [40, 245, 321, 34]})),
                "focus" => Err(SimError::new("stale_element", "nothing has focus")),
                "value" => {
                    let v = self.value.lock().unwrap().clone();
                    Ok(json!({"ok": true, "role": "AXTextField", "value": if v.is_empty() { "Email".to_string() } else { v }, "placeholder": "Email"}))
                }
                "type" => {
                    let t = args["text"].as_str().unwrap();
                    if t.chars().any(|c| !c.is_ascii()) {
                        return Err(SimError::new("unmappable", "no US-keyboard key for U+00E9"));
                    }
                    if !self.hid_dead {
                        self.value.lock().unwrap().push_str(t);
                    }
                    Ok(json!({"ok": true}))
                }
                "set_value" => {
                    *self.value.lock().unwrap() = args["text"].as_str().unwrap().to_string();
                    Ok(json!({"ok": true}))
                }
                _ => Err(SimError::new("bad_op", op.to_string())),
            }
        }
        fn frame_slot(&self, _s: Option<Arc<FrameSlot>>) {}
        fn alive(&self) -> bool {
            true
        }
    }

    fn field(v: &str, hid_dead: bool) -> FakeField {
        FakeField { value: M::new(v.into()), hid_dead, focused: true, calls: M::new(vec![]) }
    }

    #[test]
    fn typing_appends_and_is_checked_against_the_field() {
        let (lane, _) = lane_with(world(), "26.5");
        let f = field("", false);
        assert_eq!(lane.type_text(&f, None, "a@k2.dev", false).unwrap(), "typed");
        assert_eq!(*f.value.lock().unwrap(), "a@k2.dev", "placeholder is not taken as the old value");
        let f = field("x", false);
        lane.type_text(&f, None, "y", false).unwrap();
        assert_eq!(*f.value.lock().unwrap(), "xy");
    }

    #[test]
    fn a_dead_keyboard_path_falls_back_to_setting_the_value() {
        let (lane, _) = lane_with(world(), "26.5");
        let f = field("x", true);
        assert_eq!(lane.type_text(&f, None, "yz", false).unwrap(), "set");
        assert_eq!(*f.value.lock().unwrap(), "xyz");
    }

    #[test]
    fn characters_without_a_us_key_are_set_not_typed() {
        let (lane, _) = lane_with(world(), "26.5");
        let f = field("", false);
        assert_eq!(lane.type_text(&f, None, "café", false).unwrap(), "set");
        assert_eq!(*f.value.lock().unwrap(), "café");
        let mut nofocus = field("", false);
        nofocus.focused = false;
        let e = lane.type_text(&nofocus, None, "café", false).unwrap_err();
        assert!(e.message.contains("Tap the field first"), "{}", e.message);
        assert_eq!(lane.type_text(&nofocus, Some(Point { x: 200.0, y: 262.0 }), "café", false).unwrap(), "set", "the last tap finds the field");
    }

    #[test]
    fn secrets_are_never_typed_key_by_key() {
        let (lane, _) = lane_with(world(), "26.5");
        let f = field("", false);
        assert_eq!(lane.type_text(&f, None, "hunter2", true).unwrap(), "set");
        assert!(!f.calls.lock().unwrap().contains(&"type".to_string()));
        let mut nofocus = field("", false);
        nofocus.focused = false;
        assert_eq!(lane.type_text(&nofocus, None, "hunter2", true).unwrap_err().code, "bad_request");
        assert!(!nofocus.calls.lock().unwrap().contains(&"type".to_string()));
    }

    #[test]
    fn default_profile_is_the_plain_newest_iphone() {
        let r = simctl::parse_ios_runtimes(RUNTIMES).unwrap();
        assert_eq!(default_type(&r[0]).unwrap().name, "iPhone 17");
        assert_eq!(model_of("K2-ws--iPhone-17-Pro"), "iPhone 17 Pro");
        let (lane, _) = lane_with(world(), "26.5");
        let p = lane.profiles().unwrap();
        assert_eq!(p[0].name, "iPhone 17");
        assert!(p[0].default && p.iter().filter(|p| p.default).count() == 1);
        assert!(!p.iter().any(|p| p.name.contains("iPad")));
    }

    /// A helper that records each call; `type` refuses non-ASCII like the
    /// real one (it checks the whole string before typing anything).
    struct FakeKeys {
        calls: M<Vec<(String, J)>>,
    }
    impl HelperIo for FakeKeys {
        fn call(&self, op: &str, args: J, _t: Duration) -> SimResult<J> {
            self.calls.lock().unwrap().push((op.to_string(), args.clone()));
            match op {
                "type" if args["text"].as_str().unwrap().chars().any(|c| !c.is_ascii()) => Err(SimError::new("unmappable", "no US-keyboard key for U+65E5")),
                "type" | "key" => Ok(json!({"ok": true})),
                _ => Err(SimError::new("bad_op", op.to_string())),
            }
        }
        fn frame_slot(&self, _s: Option<Arc<FrameSlot>>) {}
        fn alive(&self) -> bool {
            true
        }
    }

    fn pbcopies(fake: &FakeRunner) -> Vec<Vec<String>> {
        fake.calls.lock().unwrap().iter().filter(|(_, a)| a.get(3).map(String::as_str) == Some("pbcopy")).map(|(_, a)| a.clone()).collect()
    }

    #[test]
    fn a_persons_typing_goes_straight_to_the_keyboard_with_no_read_back() {
        let (lane, fake) = lane_with(world(), "26.5");
        let h = FakeKeys { calls: M::new(vec![]) };
        assert_eq!(lane.person_keys(&h, "UDID-1", "h").unwrap(), "typed");
        let calls = h.calls.lock().unwrap().clone();
        assert_eq!(calls, vec![("type".to_string(), json!({"text": "h"}))], "one call: no focus lookup, no value read");
        assert!(pbcopies(&fake).is_empty());
    }

    #[test]
    fn ime_text_and_pastes_go_through_the_device_pasteboard_then_cmd_v() {
        let (lane, fake) = lane_with(world(), "26.5");
        let h = FakeKeys { calls: M::new(vec![]) };
        assert_eq!(lane.person_keys(&h, "UDID-1", "日本語").unwrap(), "pasted");
        let calls = h.calls.lock().unwrap().clone();
        assert_eq!(calls[0].0, "type");
        assert_eq!(calls[1], ("key".to_string(), json!({"usage": 0x19, "cmd": true})), "⌘V after the pasteboard is set");
        let pb = pbcopies(&fake);
        assert_eq!(pb.len(), 1);
        let a = &pb[0];
        assert_eq!(&a[..2], &["simctl", "--set"]);
        assert_eq!(a[2], lane.config().set_path.to_string_lossy(), "K2's own device set");
        assert_eq!(&a[3..5], &["pbcopy", "UDID-1"]);
        assert_eq!(a[5], "<stdin>日本語", "the text rides stdin, never argv");
        assert_eq!(a.len(), 6);
        // A paste never types key by key.
        let h = FakeKeys { calls: M::new(vec![]) };
        lane.paste(&h, "UDID-1", "line 1\nline 2").unwrap();
        assert_eq!(h.calls.lock().unwrap().iter().map(|c| c.0.as_str()).collect::<Vec<_>>(), vec!["key"]);
        assert_eq!(pbcopies(&fake)[1][5], "<stdin>line 1\nline 2");
    }

    #[test]
    fn keys_carry_shift_and_option_and_edit_chords_are_command_letters() {
        assert_eq!(key_args(Key::ArrowLeft, KeyMods { shift: true, alt: false }), json!({"usage": 0x50, "shift": true, "alt": false}));
        assert_eq!(key_args(Key::Backspace, KeyMods { shift: false, alt: true }), json!({"usage": 0x2A, "shift": false, "alt": true}));
        assert_eq!(edit_args(EditOp::SelectAll), json!({"usage": 0x04, "shift": false, "cmd": true}));
        assert_eq!(edit_args(EditOp::Undo), json!({"usage": 0x1D, "shift": false, "cmd": true}));
        assert_eq!(edit_args(EditOp::Redo), json!({"usage": 0x1D, "shift": true, "cmd": true}));
        assert_eq!(edit_args(EditOp::Cut), json!({"usage": 0x1B, "shift": false, "cmd": true}));
        assert_eq!(edit_args(EditOp::Copy), json!({"usage": 0x06, "shift": false, "cmd": true}));
        assert_eq!(paste_args(), json!({"usage": 0x19, "cmd": true}));
    }
}
