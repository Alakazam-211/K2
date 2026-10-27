//! Live test of the iOS lane against a real Simulator (vs-live test 9, M-A18).
//!
//! Needs a Mac with Xcode and an iOS runtime. It only compiles with
//! `--features sim-live-ios` (Cargo `required-features`), and once compiled
//! a missing Xcode is a failure, never a skip:
//!
//!   cargo test -p k2-core --features sim-live-ios --test sim_live_ios -- --nocapture
//!
//! It uses a throwaway sim home in the temp dir (its own device set and
//! helper cache), boots ONE device, and deletes it at the end even on a
//! failure (each device is ~2 GB of disk). It never touches ~/.k2 or the
//! person's own Simulators.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use k2_core::sim::ios::{IosLane, IosLaneConfig};
use k2_core::sim::lane::*;
use k2_core::sim::tree::{RawNode, Role};

const FIXTURE_ID: &str = "dev.k2.simfixture";
const KEY: &str = "k2livetest";

/// Shuts down and deletes the test devices, then the temp home, however the test ends.
struct Cleanup {
    home: PathBuf,
    set: PathBuf,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        let set = self.set.to_string_lossy().into_owned();
        let _ = Command::new("xcrun").args(["simctl", "--set", &set, "shutdown", "all"]).status();
        let _ = Command::new("xcrun").args(["simctl", "--set", &set, "delete", "all"]).status();
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

/// Builds the fixture .app for the Simulator with this Mac's Xcode.
fn build_fixture(out: &Path) -> PathBuf {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sim-ios");
    let app = out.join("K2Fixture.app");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::copy(src.join("Info.plist"), app.join("Info.plist")).unwrap();
    let arch = if cfg!(target_arch = "aarch64") { "arm64" } else { "x86_64" };
    let st = Command::new("xcrun")
        .args(["-sdk", "iphonesimulator", "swiftc", "-parse-as-library", "-O", "-target"])
        .arg(format!("{arch}-apple-ios17.0-simulator"))
        .arg(src.join("main.swift"))
        .arg("-o")
        .arg(app.join("K2Fixture"))
        .status()
        .expect("xcrun must run: this test needs Xcode");
    assert!(st.success(), "building the fixture app failed (is Xcode installed with the iOS SDK?)");
    let st = Command::new("codesign").args(["-s", "-"]).arg(&app).status().unwrap();
    assert!(st.success(), "ad-hoc signing the fixture failed");
    app
}

fn find<'a>(n: &'a RawNode, id: &str) -> Option<&'a RawNode> {
    if n.identifier.as_deref() == Some(id) {
        return Some(n);
    }
    n.children.iter().find_map(|c| find(c, id))
}

fn root(t: &RawTree) -> &RawNode {
    &t.roots[0]
}

/// Polls the tree until `pred` holds (UI updates land asynchronously).
fn tree_until(lane: &IosLane, dev: &DeviceRef, what: &str, pred: impl Fn(&RawTree) -> bool) -> RawTree {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let t = lane.tree(dev).expect("tree");
        if pred(&t) {
            return t;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}; last tree: {:#?}", t.roots);
        std::thread::sleep(Duration::from_millis(250));
    }
}

#[test]
fn an_agent_drives_the_ios_simulator_end_to_end() {
    let home = std::env::temp_dir().join(format!("k2-sim-live-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let cfg = IosLaneConfig::under(&home);
    let _cleanup = Cleanup { home: home.clone(), set: cfg.set_path.clone() };
    let lane = IosLane::new(cfg.clone());

    let probe = lane.probe();
    eprintln!("probe: {probe:?}");
    assert!(probe.usable && probe.ready, "this test needs a Mac with Xcode and an iOS Simulator runtime: {probe:?}");
    let t0 = Instant::now();
    lane.install(&[], &|_, _| {}).expect("building the helper (install)");
    eprintln!("helper built in {:?}", t0.elapsed());
    let profiles = lane.profiles().unwrap();
    assert!(profiles[0].default && profiles[0].width_pt > 300 && profiles[0].scale >= 2.0, "{:?}", profiles[0]);

    let app = build_fixture(&home);
    let data_dir = home.join("data").join(KEY);
    let req = BootRequest {
        workspace_id: "live-test".into(),
        workspace_key: KEY.into(),
        profile_id: None,
        data_dir: data_dir.clone(),
        boot_timeout: Duration::from_secs(240),
    };
    let t0 = Instant::now();
    let booted = lane.boot(&req).expect("boot");
    eprintln!("booted {} ({}, {}) in {:?}", booted.device_id, booted.profile.name, booted.profile.os, t0.elapsed());
    let dev = DeviceRef { workspace_id: req.workspace_id.clone(), workspace_key: KEY.into(), device_id: booted.device_id.clone(), data_dir: data_dir.clone(), lane_state: booted.lane_state.clone() };
    assert!(lane.alive(&dev));
    assert_eq!(lane.adopt(&dev).unwrap().unwrap().device_id, dev.device_id);

    // K2's device is in K2's set and NOT in the person's default set.
    let default_list = Command::new("xcrun").args(["simctl", "list", "devices"]).output().unwrap();
    assert!(!String::from_utf8_lossy(&default_list.stdout).contains(&dev.device_id), "K2's device leaked into the default Simulator set");

    let installed = lane.install_app(&dev, &app).expect("install");
    assert_eq!((installed.app_id.as_str(), installed.replaced), (FIXTURE_ID, false));
    assert!(lane.install_app(&dev, &app).unwrap().replaced, "a second install replaces");
    lane.launch(&dev, FIXTURE_ID).expect("launch");

    let t = tree_until(&lane, &dev, "the fixture's Save button", |t| find(root(t), "save").is_some());
    assert!(t.width_pt > 300.0 && t.height_pt > 600.0, "screen in points: {}x{}", t.width_pt, t.height_pt);
    assert_eq!(t.app_id.as_deref(), Some(FIXTURE_ID));
    let email = find(root(&t), "email").expect("email field").clone();
    let save = find(root(&t), "save").expect("save button").clone();
    assert_eq!(email.role(), Role::Textfield);
    assert_eq!(save.role(), Role::Button);
    assert_eq!(email.value, None, "an empty field has no value (not its placeholder)");

    // The core taps the element, then types into focus. No Simulator window (M-A17).
    lane.act(&dev, &Action::Tap { at: email.frame.unwrap().center() }).expect("tap email");
    let t0 = Instant::now();
    lane.act(&dev, &Action::Type { text: SimText::plain("live@k2.dev") }).expect("type");
    eprintln!("typed in {:?}", t0.elapsed());
    lane.act(&dev, &Action::Tap { at: save.frame.unwrap().center() }).expect("tap save");
    tree_until(&lane, &dev, "Saved live@k2.dev", |t| find(root(t), "status").and_then(|s| s.label.clone()).as_deref() == Some("Saved live@k2.dev"));

    // Text with no US key goes through accessibility; so does a secret.
    lane.act(&dev, &Action::Tap { at: email.frame.unwrap().center() }).unwrap();
    lane.act(&dev, &Action::Type { text: SimText::plain(" ünï ✓") }).expect("type unicode");
    let t = tree_until(&lane, &dev, "the unicode value", |t| find(root(t), "email").and_then(|e| e.value.clone()).as_deref() == Some("live@k2.dev ünï ✓"));
    lane.act(&dev, &Action::Type { text: SimText::secret("-s3cret") }).expect("type secret");
    tree_until(&lane, &dev, "the secret appended", |t| find(root(t), "email").and_then(|e| e.value.clone()).as_deref() == Some("live@k2.dev ünï ✓-s3cret"));
    lane.act(&dev, &Action::Key { key: Key::Enter }).expect("enter");

    // Frames: start the animation, pull for 2 s at 15 fps.
    let mut src = lane.frames(&dev, FrameOpts { max_fps: 15, max_width_px: Some(t.width_pt as u32) }).expect("frames");
    let anim = find(root(&t), "animate").expect("animate button").frame.unwrap().center();
    lane.act(&dev, &Action::Tap { at: anim }).unwrap();
    let start = Instant::now();
    let mut got = vec![];
    while start.elapsed() < Duration::from_secs(2) {
        if let Some(f) = src.next_frame(Duration::from_millis(200)).expect("next_frame") {
            got.push(f);
        }
    }
    lane.act(&dev, &Action::Tap { at: anim }).unwrap(); // stop the animation
    assert!(got.len() >= 15, "expected ~30 frames at 15 fps over 2 s, got {}", got.len());
    assert!(got.len() <= 34, "the fps cap didn't hold: {} frames in 2 s at 15 fps", got.len());
    let f = &got[0];
    assert!(matches!(&f.data, FrameData::Jpeg(b) if b.starts_with(&[0xff, 0xd8])), "frames are JPEG");
    assert!((f.width_px as f32 - t.width_pt).abs() < 2.0, "frames at points width: {} vs {}", f.width_px, t.width_pt);
    assert!((f.scale - 1.0).abs() < 0.02, "scale is pixels per point: {}", f.scale);
    assert!(got.windows(2).all(|w| w[1].seq > w[0].seq));
    // A still screen costs nothing: after the animation stops, frames stop
    // (the text cursor may still blink, so focus moves off the field first).
    lane.act(&dev, &Action::Button { button: Button::Home }).unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    while src.next_frame(Duration::ZERO).unwrap().is_some() {}
    let mut idle = 0;
    let s = Instant::now();
    while s.elapsed() < Duration::from_secs(1) {
        if src.next_frame(Duration::from_millis(200)).unwrap().is_some() {
            idle += 1;
        }
    }
    assert!(idle <= 2, "a still home screen kept streaming: {idle} frames in 1 s");
    drop(src);

    // A full-resolution still.
    let shot = lane.screenshot(&dev).unwrap();
    assert!(matches!(&shot.data, FrameData::Png(b) if b.starts_with(b"\x89PNG")));
    assert!(shot.scale >= 2.0 && shot.width_px as f32 >= t.width_pt * 2.0, "{}x{} @{}", shot.width_px, shot.height_px, shot.scale);

    // Home left the app.
    tree_until(&lane, &dev, "the home screen", |t| find(root(t), "save").is_none());

    // Logs run inside the device and are bounded.
    let logs = lane.logs(&dev, &LogQuery { app_id: Some(FIXTURE_ID.into()), since: Some(Duration::from_secs(120)), grep: None, max_lines: 20, max_bytes: 8192 }).expect("logs");
    assert!(logs.len() <= 20);

    // Stop keeps the device; erase removes it and the data dir.
    lane.stop(&dev).expect("stop");
    assert!(!lane.alive(&dev));
    assert_eq!(lane.adopt(&dev).unwrap(), None);
    std::fs::create_dir_all(&data_dir).unwrap();
    lane.erase(KEY, &data_dir).expect("erase");
    assert!(!data_dir.exists());
    let left = Command::new("xcrun").args(["simctl", "--set"]).arg(&cfg.set_path).args(["list", "devices"]).output().unwrap();
    assert!(!String::from_utf8_lossy(&left.stdout).contains(&dev.device_id), "erase left the device behind");
}
