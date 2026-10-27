//! Live Android lane test (prd-mobile-dev-v1 vs-live M-A18, test 9): boot a
//! headless emulator, read the tree, launch an app, tap, read the diff,
//! stream frames, install an APK, read logs, stop, adopt, erase.
//!
//! Only with `--features sim-live-android`, on a Linux box with KVM (the
//! k2n9 sandbox lane), never on the M5:
//!
//! ```text
//! K2_SIM_LIVE_BASE=/srv/k2-sandbox/live cargo test -p k2-core \
//!   --features sim-live-android --test sim_live_android -- --nocapture
//! ```
//!
//! `K2_SIM_LIVE_BASE` is a K2 home for the test: `sim/android/sdk` must be
//! installed and `sim/licences.json` must hold the box owner's acceptance
//! (the test never accepts a licence). Anything missing fails loudly.
#![cfg(feature = "sim-live-android")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use k2_core::sim::api::*;
use k2_core::sim::lane::*;
use k2_core::sim::manager::{Actor, Manager, Ws};
use k2_core::sim::stream::SimState;

fn base() -> PathBuf {
    let b = std::env::var("K2_SIM_LIVE_BASE").expect("K2_SIM_LIVE_BASE must point at the live test's K2 home");
    PathBuf::from(b)
}

fn agent() -> Actor {
    Actor::Agent { handle: "live".into() }
}

fn find(t: &TreeResp, pred: impl Fn(&k2_core::sim::tree::Element) -> bool) -> Option<String> {
    t.snapshot.elements.iter().find(|e| pred(e)).map(|e| e.id.clone())
}

#[test]
fn android_emulator_end_to_end() {
    let base = base();
    k2_core::sim::set_base_for_tests(base.clone());
    let lane = Arc::new(k2_core::sim::android::AndroidLane::new(base.join("sim").join("android")));
    let probe = lane.probe();
    println!("probe: {probe:?}");
    assert!(probe.usable, "KVM is not usable here: {:?}", probe.detail);
    assert!(probe.ready, "the lane is not ready (SDK or the owner's licence acceptance missing): {:?}", probe.needs);
    k2_core::sim::register_lane(lane.clone());
    k2_core::sim::limits::save_settings(&k2_core::sim::limits::SimSettings { max_devices: Some(2), ..Default::default() }).expect("limits");

    let ws_dir = base.join("ws-live");
    std::fs::create_dir_all(&ws_dir).expect("workspace dir");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT OR REPLACE INTO projects (id, name, path) VALUES ('p-live', 'live', ?1)",
            rusqlite::params![ws_dir.display().to_string()],
        )
        .expect("project");
    }
    let ws = Ws { id: "p-live".into(), handle: "live".into(), path: ws_dir.clone() };
    let m = Manager::new();
    m.refresh_probes();

    // Boot (cold the first time, Quick Boot after).
    let t0 = Instant::now();
    let v = m.boot(&ws, &agent(), &BootReq { no_tab: true, ..Default::default() }).expect("boot");
    println!("boot: {:?} in {:?}", v.state, t0.elapsed());
    assert_eq!(v.state, SimState::Running);
    assert_eq!(v.profile.as_ref().map(|p| p.name.as_str()), Some("Pixel 9"));

    // Launch Settings, read the tree.
    let r = m.launch(&ws, &agent(), "com.android.settings", false).expect("launch settings");
    println!("{}", r.line);
    std::thread::sleep(Duration::from_secs(2));
    let t1 = Instant::now();
    let t = m.tree(&ws, &agent(), false, None, false).expect("tree");
    println!("tree in {:?} (lane {} ms):\n{}", t1.elapsed(), t.snapshot.took_ms, t.text);
    assert_eq!(t.snapshot.app_id.as_deref(), Some("com.android.settings"));
    assert!(t.snapshot.elements.iter().any(|e| e.interactive), "no interactive elements");
    assert!((t.snapshot.width_pt - 411.0).abs() < 2.0, "{}", t.snapshot.width_pt);

    // Tap the search box (or the first clickable) and read the diff.
    let target = find(&t, |e| e.interactive && e.label.as_deref().is_some_and(|l| l.to_lowercase().contains("search")))
        .or_else(|| find(&t, |e| e.interactive && e.label.is_some()))
        .expect("something to tap");
    let r = m
        .act(&ws, &agent(), &ActReq { workspace: None, action: ActSpec::Tap { element: Some(target.clone()), x: None, y: None, hold_ms: None }, no_diff: false }, None)
        .expect("tap");
    println!("{}", r.line);
    assert!(r.line.starts_with(&format!("ok tap {target}")), "{}", r.line);
    assert!(!r.diff.is_empty(), "tapping {target} changed nothing on screen: {}", r.line);

    // Type into whatever has focus (the search field, if it opened).
    let r = m.act(&ws, &agent(), &ActReq { workspace: None, action: ActSpec::Type { into: None, text: Some("wifi".into()), secret: None }, no_diff: false }, None).expect("type");
    println!("{}", r.line);
    let typed_secret = m.act(&ws, &agent(), &ActReq { workspace: None, action: ActSpec::Type { into: None, text: None, secret: Some("x".into()) }, no_diff: true }, Some(SimText::secret("s3cr3t-live")));
    let r = typed_secret.expect("secret type");
    assert!(!r.line.contains("s3cr3t"), "{}", r.line);

    // Buttons, rotation, appearance, location, URL.
    for spec in [
        ActSpec::Button { button: Button::Back },
        ActSpec::Button { button: Button::Home },
        ActSpec::Appearance { appearance: Appearance::Dark },
        ActSpec::Appearance { appearance: Appearance::Light },
        ActSpec::Location { lat: 45.5, lon: -73.6 },
        ActSpec::Rotate { turn: Some(Direction::Right), orientation: None },
        ActSpec::Rotate { turn: None, orientation: Some(Orientation::Portrait) },
        ActSpec::OpenUrl { url: "https://example.com/'; echo pwned".into() },
        ActSpec::Swipe { direction: Direction::Up, from: None },
    ] {
        let r = m.act(&ws, &agent(), &ActReq { workspace: None, action: spec.clone(), no_diff: true }, None).unwrap_or_else(|e| panic!("{spec:?}: {e:?}"));
        println!("{}", r.line);
    }

    // Screenshot (points-size PNG in the workspace) and frames.
    let s = m.screenshot(&ws, &agent(), None).expect("screenshot");
    println!("screenshot {} {}x{}", s.path, s.width, s.height);
    assert!(PathBuf::from(&s.path).is_file());
    assert!((s.width as i64 - 411).abs() <= 1, "{}x{}", s.width, s.height);
    let row = m.running(&ws).expect("running row");
    let mut src = m.frames(&row, FrameOpts::default()).expect("frames");
    let t2 = Instant::now();
    let f = src.next_frame(Duration::from_secs(5)).expect("frame ok").expect("a first frame within 5 s");
    println!("first frame {}x{} in {:?}", f.width_px, f.height_px, t2.elapsed());
    assert!(f.width_px >= 400 && f.width_px <= 420, "{}x{}", f.width_px, f.height_px);
    let (jpeg, w, h) = k2_core::sim::encode::frame_jpeg(&f, k2_core::sim::encode::JPEG_QUALITY).expect("jpeg");
    println!("jpeg {} KB {w}x{h}", jpeg.len() / 1024);
    drop(src);

    // Install: pull a preinstalled APK and install it back.
    let apk = ws_dir.join("reinstall.apk");
    let pkg = std::env::var("K2_SIM_LIVE_APK_PACKAGE").unwrap_or_else(|_| "com.android.chrome".into());
    let adb = base.join("sim/android/sdk/platform-tools/adb");
    let st: serde_json::Value = row.lane_state.clone();
    let serial = format!("emulator-{}", st["consolePort"]);
    let env = [("ANDROID_ADB_SERVER_PORT", "5137"), ("ANDROID_USER_HOME", &*base.join("sim/android/user").display().to_string())];
    let path_out = std::process::Command::new(&adb).args(["-s", &serial, "shell", "pm", "path", &pkg]).envs(env.iter().map(|(a, b)| (*a, *b))).output().expect("pm path");
    let remote = String::from_utf8_lossy(&path_out.stdout).lines().find_map(|l| l.strip_prefix("package:")).map(|s| s.trim().to_string());
    match remote {
        Some(remote) => {
            let pulled = std::process::Command::new(&adb).args(["-s", &serial, "pull", &remote, &apk.display().to_string()]).envs(env.iter().map(|(a, b)| (*a, *b))).output().expect("pull");
            assert!(pulled.status.success(), "pull: {}", String::from_utf8_lossy(&pulled.stderr));
            let t3 = Instant::now();
            let r = m.install_app(&ws, &agent(), "reinstall.apk").expect("install");
            println!("{} in {:?}", r.line, t3.elapsed());
            assert_eq!(r.outcome.app_id, pkg);
        }
        None => panic!("{pkg} is not on this image; set K2_SIM_LIVE_APK_PACKAGE to an installed package"),
    }

    // Logs: bounded and redacted.
    let l = m.logs(&ws, None, Some(Duration::from_secs(120)), None, Some(20)).expect("logs");
    println!("logs: {} lines, truncated={}", l.lines.len(), l.truncated);
    assert!(!l.lines.is_empty());
    assert!(l.lines.len() <= 20);

    // Stop keeps the data; a stopped device isn't adopted; erase reclaims.
    let t4 = Instant::now();
    let v = m.stop(&ws, &agent(), "live test").expect("stop");
    println!("stop in {:?}", t4.elapsed());
    assert_eq!(v.state, SimState::Stopped);
    let dir = k2_core::sim::device_dir(LaneKind::Android, &k2_core::sim::workspace_key("p-live"));
    assert!(dir.join("avd").is_dir(), "stop keeps the device data");
    assert!(!lane.alive(&DeviceRef {
        workspace_id: "p-live".into(),
        workspace_key: k2_core::sim::workspace_key("p-live"),
        device_id: String::new(),
        data_dir: dir.clone(),
        lane_state: row.lane_state.clone(),
    }));

    // Quick Boot: the second boot is fast.
    let t5 = Instant::now();
    m.boot(&ws, &agent(), &BootReq { no_tab: true, ..Default::default() }).expect("second boot");
    println!("quick boot in {:?}", t5.elapsed());
    if std::env::var("K2_SIM_LIVE_KEEP").is_err() {
        m.erase(&ws, &Actor::Person { name: "owner".into() }).expect("erase");
        assert!(!dir.exists(), "erase removes the device folder");
    }
}
