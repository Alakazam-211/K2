//! Live test of the helper's protocol 4 (fix 1, S3a): k2-simhelper encodes
//! H.264 itself with VideoToolbox, straight from the framebuffer, and sends
//! units on fd 3. S3b: the stream hub's source, opened the way the hub opens
//! it (`SimLane::stream_source`), relays those units with no daemon encode;
//! with `K2_SIM_HELPER_ENCODE=0` it is protocol 3 through the daemon's own
//! encoder again (the ignored rollback test, run on its own:
//! `K2_SIM_HELPER_ENCODE=0 cargo test … --test sim_live_ios_p4 -- --ignored`).
//!
//! Same rules as `sim_live_ios.rs`: a Mac with Xcode and an iOS runtime,
//! `--features sim-live-ios`, a missing Xcode is a failure, never a skip:
//!
//!   cargo test -p k2-core --features sim-live-ios --test sim_live_ios_p4 -- --nocapture --test-threads=1
//!
//! A throwaway sim home in the temp dir (its own device set and helper
//! cache), ONE device, deleted at the end even on a failure. It never
//! touches ~/.k2 or the person's own Simulators. It prints what it measured.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use k2_core::sim::ios::helper::{self, HelperIo, HelperProc, ReceivedUnit};
use k2_core::sim::ios::simctl::ProcessRunner;
use k2_core::sim::ios::{IosLane, IosLaneConfig};
use k2_core::sim::lane::*;
use k2_core::sim::stream::LOCAL_MAX_FPS;
use k2_core::sim::tree::RawNode;
use k2_core::stream::{EncodedSource, EncodedUnit};
use k2_simvideo::annexb::{nal_type, nal_units};
use k2_simvideo::link::{Codec, Encoder, LayerSpec};
use serde_json::json;

const FIXTURE_ID: &str = "dev.k2.simfixture";
const KEY: &str = "k2livep4";

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

fn h264(id: u8, max_width: u32, fps: u32, bitrate: u32) -> LayerSpec {
    LayerSpec { id, codec: Codec::H264, max_width, fps, bitrate, quality: None, on: true }
}

/// Every unit for `d`.
fn collect(h: &HelperProc, d: Duration) -> Vec<ReceivedUnit> {
    let end = Instant::now() + d;
    let mut got = vec![];
    while Instant::now() < end {
        if let Some(u) = h.next_unit(Duration::from_millis(20)).expect("the helper is alive") {
            got.push(u);
        }
    }
    got
}

/// Waits for one unit of `layer` (up to `d`), keeping the others it reads.
fn first_of(h: &HelperProc, layer: u8, d: Duration) -> (ReceivedUnit, Vec<ReceivedUnit>) {
    let end = Instant::now() + d;
    let mut before = vec![];
    while Instant::now() < end {
        if let Some(u) = h.next_unit(Duration::from_millis(20)).expect("the helper is alive") {
            if u.unit.header.layer == layer {
                return (u, before);
            }
            before.push(u);
        }
    }
    panic!("no unit of layer {layer} within {d:?}");
}

fn drain(h: &HelperProc) {
    while h.next_unit(Duration::ZERO).expect("the helper is alive").is_some() {}
}

fn of(units: &[ReceivedUnit], layer: u8) -> Vec<&ReceivedUnit> {
    units.iter().filter(|u| u.unit.header.layer == layer).collect()
}

fn p50(mut v: Vec<f64>) -> f64 {
    assert!(!v.is_empty());
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn p95(mut v: Vec<f64>) -> f64 {
    assert!(!v.is_empty());
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[(v.len() * 95 / 100).min(v.len() - 1)]
}

/// Intervals between units as the daemon read them, in ms.
fn gaps_ms(units: &[&ReceivedUnit]) -> Vec<f64> {
    units.windows(2).map(|w| (w[1].received_at - w[0].received_at).as_secs_f64() * 1000.0).collect()
}

fn nal_types(data: &[u8]) -> Vec<u8> {
    nal_units(data).iter().filter_map(|n| nal_type(n)).collect()
}

/// A booted device in a throwaway home with the fixture app running,
/// deleted when it drops.
struct Booted {
    lane: IosLane,
    cfg: IosLaneConfig,
    dev: DeviceRef,
    xcode: String,
    _cleanup: Cleanup,
}

fn boot_fixture(tag: &str) -> Booted {
    let home = std::env::temp_dir().join(format!("k2-sim-live-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let cfg = IosLaneConfig::under(&home);
    let cleanup = Cleanup { home: home.clone(), set: cfg.set_path.clone() };
    let lane = IosLane::new(cfg.clone());

    let probe = lane.probe();
    assert!(probe.usable && probe.ready, "this test needs a Mac with Xcode and an iOS Simulator runtime: {probe:?}");
    lane.install(&[], &|_, _| {}).expect("building the helper (install)");
    let xcode = lane.xcode_version().unwrap();
    let app = build_fixture(&home);
    let data_dir = home.join("data").join(KEY);
    let req = BootRequest { workspace_id: "live-p4".into(), workspace_key: KEY.into(), profile_id: None, data_dir: data_dir.clone(), boot_timeout: Duration::from_secs(240) };
    let t0 = Instant::now();
    let booted = lane.boot(&req).expect("boot");
    eprintln!("booted {} ({}, {}) in {:?} with Xcode {xcode}", booted.device_id, booted.profile.name, booted.profile.os, t0.elapsed());
    let dev = DeviceRef { workspace_id: req.workspace_id.clone(), workspace_key: KEY.into(), device_id: booted.device_id.clone(), data_dir: data_dir.clone(), lane_state: booted.lane_state.clone() };
    lane.install_app(&dev, &app).expect("install");
    lane.launch(&dev, FIXTURE_ID).expect("launch");
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        // "No frontmost app yet" while the fixture launches.
        match lane.tree(&dev) {
            Ok(t) if t.roots.first().is_some_and(|r| find(r, "animate").is_some()) => break,
            r => assert!(Instant::now() < deadline, "the fixture's Animate button never showed: {:?}", r.err()),
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Booted { lane, cfg, dev, xcode, _cleanup: cleanup }
}

/// The next unit of `layer` off a hub source (up to `d`).
fn next_of(src: &mut dyn EncodedSource, layer: u8, d: Duration) -> EncodedUnit {
    let end = Instant::now() + d;
    while Instant::now() < end {
        if let Some(u) = src.next_unit(Duration::from_millis(20)).expect("the source is alive") {
            if u.layer == layer {
                return u;
            }
        }
    }
    panic!("no unit of layer {layer} within {d:?}");
}

/// Every unit of `layer` off a hub source for `d`.
fn collect_src(src: &mut dyn EncodedSource, layer: u8, d: Duration) -> Vec<EncodedUnit> {
    let end = Instant::now() + d;
    let mut got = vec![];
    while Instant::now() < end {
        if let Some(u) = src.next_unit(Duration::from_millis(20)).expect("the source is alive") {
            if u.layer == layer {
                got.push(u);
            }
        }
    }
    got
}

fn ms(a: Instant, b: Instant) -> f64 {
    b.saturating_duration_since(a).as_secs_f64() * 1000.0
}

fn tap(lane: &IosLane, dev: &DeviceRef, (x, y): (f64, f64)) {
    lane.act(dev, &Action::Tap { at: Point { x: x as f32, y: y as f32 } }).expect("tap");
}

#[test]
fn the_helper_encodes_h264_layers_with_videotoolbox() {
    let Booted { _cleanup, lane, cfg, dev, xcode } = boot_fixture("p4"); // the cleanup drops last

    // The helper, straight through helper.rs: the gate picks 4.
    let bin = helper::ensure_built(&cfg.helper_dir, &ProcessRunner, &xcode).expect("the helper binary");
    let h = HelperProc::spawn(&bin, &cfg.set_path, &dev.device_id).expect("spawn the helper");
    assert_eq!(h.protocol(), 4, "the gate picks protocol 4 (is K2_SIM_HELPER_ENCODE=0 set?)");
    eprintln!("ready: codecs {:?}, encoder {:?}", HelperIo::codecs(&h), HelperIo::encoder(&h));
    assert_eq!(HelperIo::codecs(&h), vec![Codec::H264, Codec::Jpeg]);
    assert_eq!(HelperIo::encoder(&h), Encoder::Hardware, "this node's VideoToolbox should give a hardware H.264 session; helper log: {:#?}", h.stderr_tail());
    let hello = h.call("hello", json!({}), Duration::from_secs(20)).expect("hello");
    assert_eq!(hello["accepts"], json!([3, 4]));
    let (wpt, hpt, scale) = (hello["screen"]["width"].as_f64().unwrap(), hello["screen"]["height"].as_f64().unwrap(), hello["screen"]["scale"].as_f64().unwrap());
    let native_w = ((wpt * scale).round() as u32) & !1;
    let native_h = ((hpt * scale).round() as u32) & !1;
    eprintln!("screen {wpt}x{hpt} pt @{scale}: {native_w}x{native_h} px");

    // Layer 0: H.264 at native width, 60 fps. Its first unit is a key and a
    // source start with SPS, PPS and an IDR, even on a still screen.
    let t0 = Instant::now();
    h.set_layers(&[h264(0, 0, 60, 12_000_000)]).expect("layers");
    let (first, _) = first_of(&h, 0, Duration::from_secs(5));
    let hd = first.unit.header;
    eprintln!("first unit after {:?}: {}x{} {} B, NALs {:?}", t0.elapsed(), hd.width, hd.height, first.unit.data.len(), nal_types(&first.unit.data));
    assert_eq!(hd.codec, Codec::H264);
    assert!(hd.key && hd.source_start, "{hd:?}");
    let types = nal_types(&first.unit.data);
    for t in [7, 8, 5] {
        assert!(types.contains(&t), "NAL type {t} in the first unit: {types:?}");
    }
    assert_eq!((hd.width, hd.height), (native_w, native_h), "native size, even");
    assert!(hd.emitted_ns >= hd.captured_ns);

    // Motion: the fixture's animation runs at the display rate.
    let anim = (wpt * 0.5, hpt * 0.60);
    h.call("tap", json!({"x": anim.0, "y": anim.1}), Duration::from_secs(20)).expect("tap Animate");
    std::thread::sleep(Duration::from_millis(500));
    drain(&h);
    let units = collect(&h, Duration::from_secs(2));
    let l0 = of(&units, 0);
    let rate = l0.len() as f64 / 2.0;
    let gaps = gaps_ms(&l0);
    let bytes: usize = l0.iter().map(|u| u.unit.data.len()).sum();
    let encode: Vec<f64> = l0.iter().map(|u| (u.unit.header.emitted_ns - u.unit.header.captured_ns) as f64 / 1e6).collect();
    let pipe: Vec<f64> = l0.iter().filter_map(|u| u.times(h.clock()).1.map(|e| (u.received_at - e).as_secs_f64() * 1000.0)).collect();
    eprintln!(
        "60 fps layer, moving: {rate:.1} units/s, interval p50 {:.2} ms p95 {:.2} ms, {:.2} Mb/s, capture→emit p50 {:.2} ms p95 {:.2} ms, emit→read p50 {:.2} ms, keys {}",
        p50(gaps.clone()),
        p95(gaps.clone()),
        bytes as f64 * 8.0 / 2.0 / 1e6,
        p50(encode.clone()),
        p95(encode),
        p50(pipe),
        l0.iter().filter(|u| u.unit.header.key).count()
    );
    assert!(rate >= 50.0, "≥ 50 units/s while the screen moves, got {rate}");
    assert!(p50(gaps) <= 18.5, "unit interval p50 ≤ 18.5 ms");
    assert!(l0.windows(2).all(|w| w[1].unit.header.seq == w[0].unit.header.seq + 1), "seq +1 per unit");
    assert!(l0.iter().all(|u| !u.unit.header.source_start), "no source start without a change");
    assert!(l0.iter().filter(|u| u.unit.header.key).count() <= 1, "keyframes only on demand");

    // A keyframe request: a key unit within 3 units, on a frame read at or
    // after since_ns.
    let c = h.clock().expect("ready carried clock_ns");
    let since = c.helper_ns + c.at.elapsed().as_nanos() as u64;
    h.request_keyframe(0, Some(since)).expect("keyframe");
    let mut n = 0;
    let key = loop {
        let (u, _) = first_of(&h, 0, Duration::from_secs(2));
        n += 1;
        if u.unit.header.key {
            break u;
        }
        assert!(n < 3, "no key unit within 3 units of the request");
    };
    eprintln!("keyframe request: key on unit {n} after it ({} B, NALs {:?}, captured {} ns after since)", key.unit.data.len(), nal_types(&key.unit.data), key.unit.header.captured_ns as i64 - since as i64);
    assert!(!key.unit.header.source_start, "a requested key is not a source start");
    assert!(nal_types(&key.unit.data).starts_with(&[7, 8]), "SPS + PPS in front of the IDR");

    // rate: new targets in place, no keyframe, paced to the new fps.
    h.set_rate(0, 30, 4_000_000).expect("rate");
    std::thread::sleep(Duration::from_millis(100));
    drain(&h);
    let units = collect(&h, Duration::from_secs(2));
    let l0 = of(&units, 0);
    eprintln!("after rate 30 fps / 4 Mb/s: {:.1} units/s, {:.2} Mb/s", l0.len() as f64 / 2.0, l0.iter().map(|u| u.unit.data.len()).sum::<usize>() as f64 * 8.0 / 2.0 / 1e6);
    assert!((50..=66).contains(&l0.len()), "≈ 30 units/s after rate, got {} in 2 s", l0.len());
    assert!(l0.iter().all(|u| !u.unit.header.key), "a rate change makes no keyframe");

    // A second layer at 15 fps beside the first (back at 60).
    h.set_layers(&[h264(0, 0, 60, 12_000_000), h264(1, 402, 15, 1_000_000)]).expect("two layers");
    let (l1first, _) = first_of(&h, 1, Duration::from_secs(5));
    let hd1 = l1first.unit.header;
    assert!(hd1.key && hd1.source_start, "{hd1:?}");
    assert_eq!(hd1.width, 402);
    assert_eq!(hd1.height, ((native_h as f64 * 402.0 / native_w as f64).round() as u32) & !1, "aspect kept, even");
    let units = collect(&h, Duration::from_secs(2));
    let (l0, l1) = (of(&units, 0), of(&units, 1));
    eprintln!("two layers: layer 0 {:.1} units/s, layer 1 ({}x{}) {:.1} units/s, {:.2} Mb/s", l0.len() as f64 / 2.0, hd1.width, hd1.height, l1.len() as f64 / 2.0, l1.iter().map(|u| u.unit.data.len()).sum::<usize>() as f64 * 8.0 / 2.0 / 1e6);
    assert!(l0.len() >= 100, "layer 0 keeps ≥ 50 units/s beside layer 1: {} in 2 s", l0.len());
    assert!((26..=32).contains(&l1.len()), "layer 1 at ≈ 15 units/s: {} in 2 s", l1.len());
    assert!(l1.iter().all(|u| !u.unit.header.source_start));

    // A JPEG layer replaces layer 1 (missing from the list: torn down).
    h.set_layers(&[h264(0, 0, 60, 12_000_000), LayerSpec { id: 2, codec: Codec::Jpeg, max_width: 402, fps: 15, bitrate: 0, quality: Some(70), on: true }]).expect("a JPEG layer");
    let t_swap = Instant::now();
    let (j, _) = first_of(&h, 2, Duration::from_secs(5));
    assert_eq!(j.unit.header.codec, Codec::Jpeg);
    assert!(j.unit.header.key && j.unit.header.source_start);
    assert!(j.unit.data.starts_with(&[0xff, 0xd8]), "a JPEG");
    let units = collect(&h, Duration::from_secs(2));
    let late_l1 = units.iter().filter(|u| u.unit.header.layer == 1 && u.received_at > t_swap + Duration::from_millis(100)).count();
    let l2 = of(&units, 2);
    eprintln!("JPEG layer: {}x{}, {:.1} units/s, {} B each (first)", j.unit.header.width, j.unit.header.height, l2.len() as f64 / 2.0, j.unit.data.len());
    assert_eq!(late_l1, 0, "a layer missing from the list stops");
    assert!(l2.iter().all(|u| u.unit.header.codec == Codec::Jpeg && u.unit.header.key));
    assert!(l2.len() >= 20, "JPEG at ≈ 15 units/s: {} in 2 s", l2.len());

    // on: false stops a layer's units within 100 ms.
    drain(&h);
    let t_off = Instant::now();
    h.set_layers(&[LayerSpec { on: false, ..h264(0, 0, 60, 12_000_000) }]).expect("layers off");
    let units = collect(&h, Duration::from_secs(1));
    let last = units.iter().filter(|u| u.unit.header.layer == 0).map(|u| u.received_at - t_off).max();
    eprintln!("off: the last layer 0 unit came {last:?} after the request; {} units in the next second", units.len());
    assert!(last.is_none_or(|d| d <= Duration::from_millis(100)), "units 100 ms after on: false: {last:?}");
    assert!(units.iter().all(|u| u.received_at - t_off <= Duration::from_millis(100)), "nothing streams with no layer on");
    assert_eq!(h.units_dropped(), 0, "the queue never overflowed");

    let hello = h.call("hello", json!({}), Duration::from_secs(20)).expect("hello");
    eprintln!("hello after: encoder {}, codecs {}", hello["encoder"], hello["codecs"]);
    eprintln!("helper log:\n  {}", h.stderr_tail().join("\n  "));
    h.call("tap", json!({"x": anim.0, "y": anim.1}), Duration::from_secs(20)).expect("tap Stop");
    drop(h);

    // S3b: the hub's source, opened as the hub opens it, is the lane's
    // helper's own units.
    the_hub_source_relays_the_helpers_units(&lane, &dev, (wpt, hpt), native_w);
    lane.stop(&dev).expect("stop");
}

/// One H.264 layer at native width, 60 fps, as the hub asks for a viewer's
/// `pw` at the device's width: units come straight from the helper (no
/// daemon encode), ≥ 50 a second while the fixture animates, the first a
/// key + source start, and a keyframe ask brings a key.
fn the_hub_source_relays_the_helpers_units(lane: &IosLane, dev: &DeviceRef, (wpt, hpt): (f64, f64), native_w: u32) {
    let mut src = lane.stream_source(dev, FrameOpts::points(LOCAL_MAX_FPS), FrameSize::Points).expect("the hub's source");
    let sizes = src.sizes().expect("a protocol 4 source knows its sizes from the hello");
    eprintln!("hub source: sizes {sizes:?}");
    assert_eq!((sizes.points, sizes.native_px), (wpt.round() as u32, Some(native_w)));
    let t0 = Instant::now();
    src.set_layers(&[h264(0, native_w, 60, 12_000_000)]);
    let first = next_of(src.as_mut(), 0, Duration::from_secs(5));
    eprintln!("hub source: first unit after {:?}: {}x{} ({}x{} pt) {} B, key {}, source start {}", t0.elapsed(), first.width, first.height, first.width_pt, first.height_pt, first.data.len(), first.key, first.source_start);
    assert!(first.key && first.source_start, "the first unit is a key + source start");
    assert_eq!(first.width, native_w);
    assert_eq!((first.width_pt, first.height_pt), (wpt.round() as u32, hpt.round() as u32));
    assert!(first.encode.is_none(), "the helper encoded it, not the daemon");
    let anim = (wpt * 0.5, hpt * 0.60);
    tap(lane, dev, anim); // Animate
    std::thread::sleep(Duration::from_millis(500));
    while src.next_unit(Duration::ZERO).expect("alive").is_some() {}
    let units = collect_src(src.as_mut(), 0, Duration::from_secs(2));
    let rate = units.len() as f64 / 2.0;
    let gaps: Vec<f64> = units.windows(2).map(|w| ms(w[0].pulled_at, w[1].pulled_at)).collect();
    let bytes: usize = units.iter().map(|u| u.data.len()).sum();
    let cap_emit: Vec<f64> = units.iter().filter_map(|u| u.stamps.emitted_at.map(|e| ms(u.captured_at, e))).collect();
    let emit_read: Vec<f64> = units.iter().filter_map(|u| Some(ms(u.stamps.emitted_at?, u.stamps.received_at?))).collect();
    let read_pull: Vec<f64> = units.iter().filter_map(|u| u.stamps.received_at.map(|r| ms(r, u.pulled_at))).collect();
    let cap_pull: Vec<f64> = units.iter().map(|u| ms(u.captured_at, u.pulled_at)).collect();
    eprintln!(
        "hub source, 60 fps layer at {native_w} px, moving: {rate:.1} units/s, interval p50 {:.2} ms p95 {:.2} ms, {:.2} Mb/s, capture→emit p50 {:.2} ms, emit→read p50 {:.2} ms, read→pull p50 {:.2} ms, capture→pull p50 {:.2} ms p95 {:.2} ms, keys {}",
        p50(gaps.clone()),
        p95(gaps),
        bytes as f64 * 8.0 / 2.0 / 1e6,
        p50(cap_emit),
        p50(emit_read),
        p50(read_pull),
        p50(cap_pull.clone()),
        p95(cap_pull),
        units.iter().filter(|u| u.key).count()
    );
    assert!(rate >= 50.0, "≥ 50 units/s off the hub's source while the screen moves, got {rate}");
    assert!(units.iter().all(|u| u.encode.is_none() && u.width == native_w), "every unit relayed at native width");
    assert!(units.windows(2).all(|w| w[1].seq == w[0].seq + 1), "seq +1 per unit");
    // The hub's keyframe ask (its clock) lands on the helper's clock.
    let asked = Instant::now();
    src.request_keyframe(0, Some(asked));
    let mut n = 0;
    let key = loop {
        let u = next_of(src.as_mut(), 0, Duration::from_secs(2));
        n += 1;
        if u.key {
            break u;
        }
        assert!(n < 4, "no key unit within 3 units of the ask");
    };
    eprintln!("hub source: keyframe ask answered on unit {n} ({} B), captured {:.2} ms after the ask", key.data.len(), key.captured_at.saturating_duration_since(asked).as_secs_f64() * 1000.0);
    tap(lane, dev, anim); // Stop
    drop(src); // every layer off
}

/// The rollback, live: with `K2_SIM_HELPER_ENCODE=0` the gate holds the
/// helper at protocol 3 and the hub's source is S2's again: the helper's
/// JPEG frames, encoded once per layer by the daemon.
#[test]
#[ignore = "run on its own with K2_SIM_HELPER_ENCODE=0 (the switch is read once per process)"]
fn the_rollback_streams_protocol_3_through_the_daemon() {
    assert_eq!(std::env::var("K2_SIM_HELPER_ENCODE").ok().as_deref(), Some("0"), "run this test with K2_SIM_HELPER_ENCODE=0");
    let Booted { _cleanup, lane, cfg, dev, xcode } = boot_fixture("p4-rollback");
    let bin = helper::ensure_built(&cfg.helper_dir, &ProcessRunner, &xcode).expect("the helper binary");
    let h = HelperProc::spawn(&bin, &cfg.set_path, &dev.device_id).expect("spawn the helper");
    assert_eq!(h.protocol(), 3, "the switch holds the gate at 3");
    let hello = h.call("hello", json!({}), Duration::from_secs(20)).expect("hello");
    let (wpt, hpt) = (hello["screen"]["width"].as_f64().unwrap(), hello["screen"]["height"].as_f64().unwrap());
    drop(h);
    let mut src = lane.stream_source(&dev, FrameOpts::points(LOCAL_MAX_FPS), FrameSize::Points).expect("the hub's source");
    assert!(src.sizes().is_none(), "a pixel source knows its sizes at its first frame");
    let pts = wpt.round() as u32;
    src.set_layers(&[h264(0, pts, 30, 2_000_000)]);
    let first = next_of(src.as_mut(), 0, Duration::from_secs(10));
    assert!(first.key && first.source_start);
    let anim = (wpt * 0.5, hpt * 0.60);
    tap(&lane, &dev, anim);
    std::thread::sleep(Duration::from_millis(500));
    let units = collect_src(src.as_mut(), 0, Duration::from_secs(2));
    eprintln!("rollback: {}x{} H.264, {:.1} units/s, sizes {:?}", first.width, first.height, units.len() as f64 / 2.0, src.sizes());
    assert!(first.encode.is_some() && units.iter().all(|u| u.encode.is_some()), "every unit is a daemon encode");
    assert!(units.len() >= 40, "≈ 30 units/s through the daemon: {} in 2 s", units.len());
    let enc: Vec<f64> = units.iter().filter_map(|u| u.encode.map(|e| ms(e.start, e.end))).collect();
    eprintln!("rollback: the daemon's encode p50 {:.2} ms p95 {:.2} ms", p50(enc.clone()), p95(enc));
    assert_eq!(first.width, pts, "points, as S2 sends by default");
    tap(&lane, &dev, anim);
    drop(src);
    lane.stop(&dev).expect("stop");
}
