//! Live test of D5's encode path on a real Mac: deskd's capture engine on
//! its own thread, VideoToolbox through kessel-video, protocol-4 units on a
//! real pipe, read back with the engine's reader (`k2_simvideo::link`, what
//! the daemon's `LinkSource` uses) and decoded again with VideoToolbox.
//!
//! The screen is scripted, not ScreenCaptureKit: frames are NV12 IOSurface
//! buffers like ScreenCaptureKit's (a moving bar at the plan's rate, made
//! once and cycled), handed over only while the "screen" moves. So this
//! runs with no GUI session and no Screen Recording (a compute job on
//! k2n1-m4 as `_k2node`). The ScreenCaptureKit half is
//! `examples/sck_live.rs`, which needs a logged-in session and the grant.
//!
//!   cargo test -p k2-deskd --features mac-live --test mac_live_encode -- --nocapture --test-threads=1
//!
//! Without the feature it isn't built; on a Mac with no VideoToolbox H.264
//! it fails (never a skip). It prints what it measured.

#![cfg(all(target_os = "macos", feature = "mac-live"))]

use std::collections::HashMap;
use std::io::{PipeReader, PipeWriter};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use k2_deskd::capture::{self, Capture, Opener, Out, Plan};
use k2_deskd_proto::link::{self, Codec, Encoder as LinkEncoder, LayerSpec, Message, UnitHeader};
use k2_deskd_proto::msg::Event;
use kessel_video::fake::pattern_frame;
use kessel_video::{DecoderConfig, EncodedInput, FrameSource, PixelFormat, SourceEvent, SourceStats, VideoBackend, VideoCodec, VideoError, VideoFrame, Wake};

/// A Retina MacBook's panel and the minis' 1080p virtual display.
const DISPLAYS: [(u32, (u32, u32)); 2] = [(1, (3024, 1964)), (2, (1920, 1080))];
const BAR_FRAMES: u32 = 12;

struct Screen {
    backend: Arc<dyn VideoBackend>,
    moving: Arc<AtomicBool>,
    /// Made once per size: NV12 IOSurface buffers.
    frames: Mutex<HashMap<(u32, u32), Arc<Vec<VideoFrame>>>>,
    opens: Mutex<Vec<Plan>>,
    /// Frames handed to the engine.
    pushed: Arc<std::sync::atomic::AtomicU64>,
    /// CPU the scripted screen's threads spent (spinning), to leave out of
    /// the process's.
    gen_cpu_ns: Arc<std::sync::atomic::AtomicU64>,
}

fn thread_cpu_ns() -> u64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: clock_gettime fills the timespec it's given.
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

impl Screen {
    fn frames(&self, w: u32, h: u32) -> Arc<Vec<VideoFrame>> {
        let mut m = self.frames.lock().unwrap();
        m.entry((w, h))
            .or_insert_with(|| {
                Arc::new(
                    (0..BAR_FRAMES)
                        .map(|n| {
                            let f = pattern_frame(w, h, n * 5, 0).expect("a pattern");
                            self.backend.convert(&f, w, h, PixelFormat::Native).expect("an IOSurface NV12 buffer, as ScreenCaptureKit gives")
                        })
                        .collect(),
                )
            })
            .clone()
    }
}

/// One open capture: a thread that pushes the next bar frame each
/// interval while the screen moves.
struct Scripted {
    q: Arc<Mutex<std::collections::VecDeque<VideoFrame>>>,
    stop: Arc<AtomicBool>,
}

impl FrameSource for Scripted {
    fn try_recv(&mut self) -> Option<VideoFrame> {
        self.q.lock().unwrap().pop_front()
    }
    fn try_event(&mut self) -> Option<SourceEvent> {
        None
    }
    fn set_constraints(&mut self, _c: kessel_video::CaptureConstraints) -> Result<(), VideoError> {
        Ok(())
    }
    fn set_paused(&mut self, _p: bool) {}
    fn stats(&self) -> SourceStats {
        SourceStats::default()
    }
}

impl Drop for Scripted {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

struct ScreenOpener(Arc<Screen>);

impl Opener for ScreenOpener {
    fn default_display(&mut self) -> Option<u32> {
        Some(DISPLAYS[0].0)
    }
    fn native_size(&mut self, display: u32) -> Option<(u32, u32)> {
        DISPLAYS.iter().find(|d| d.0 == display).map(|d| d.1)
    }
    fn open(&mut self, plan: &Plan, wake: Wake) -> Result<Box<dyn FrameSource>, VideoError> {
        self.0.opens.lock().unwrap().push(*plan);
        let native = self.native_size(plan.display).ok_or(VideoError::NotFound)?;
        let (w, h) = capture::layer_size(native, plan.width);
        let frames = self.0.frames(w, h);
        let q: Arc<Mutex<std::collections::VecDeque<VideoFrame>>> = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        let (q2, stop2, moving, pushed) = (q.clone(), stop.clone(), self.0.moving.clone(), self.0.pushed.clone());
        let interval = Duration::from_nanos(1_000_000_000 / plan.fps.max(1) as u64);
        let gen_cpu = self.0.gen_cpu_ns.clone();
        std::thread::spawn(move || {
            // ScreenCaptureKit delivers on its own high-QoS queue.
            capture::realtime_qos();
            let mut n = 0usize;
            let mut next = Instant::now();
            let mut cpu_seen = thread_cpu_ns();
            // The first frame always comes (ScreenCaptureKit's first sample).
            let mut first = true;
            while !stop2.load(Ordering::SeqCst) {
                if first || moving.load(Ordering::SeqCst) {
                    let mut f = frames[n % frames.len()].clone();
                    f.capture_us = capture::now_us();
                    let mut g = q2.lock().unwrap();
                    // ScreenCaptureKit's queue: 2 deep, oldest dropped.
                    if g.len() >= 2 {
                        g.pop_front();
                    }
                    g.push_back(f);
                    drop(g);
                    pushed.fetch_add(1, Ordering::SeqCst);
                    wake();
                    n += 1;
                    first = false;
                }
                next += interval;
                if Instant::now() > next {
                    next = Instant::now();
                }
                // Spin, not sleep: a compute job's sleeps are coalesced
                // (16 ms became ~55 ms), and the screen's timing is what
                // this test holds the engine to. Its CPU is counted apart.
                while Instant::now() < next && !stop2.load(Ordering::Relaxed) {
                    std::hint::spin_loop();
                }
                let c = thread_cpu_ns();
                gen_cpu.fetch_add(c - cpu_seen, Ordering::Relaxed);
                cpu_seen = c;
            }
        });
        Ok(Box::new(Scripted { q, stop }))
    }
}

/// Units onto a real pipe, as deskd's frames pipe; events kept.
struct PipeOut {
    w: Mutex<PipeWriter>,
    events: Mutex<Vec<Event>>,
}

impl Out for PipeOut {
    fn unit(&self, h: &UnitHeader, data: &[u8]) -> bool {
        link::write_unit(&mut *self.w.lock().unwrap(), h, data).is_ok()
    }
    fn event(&self, e: &Event) {
        self.events.lock().unwrap().push(e.clone());
    }
}

struct Got {
    h: UnitHeader,
    data: Vec<u8>,
    read_us: u64,
}

fn reader(mut r: PipeReader) -> Arc<Mutex<Vec<Got>>> {
    let got: Arc<Mutex<Vec<Got>>> = Arc::default();
    let g = got.clone();
    std::thread::spawn(move || loop {
        match link::read_message(&mut r) {
            Ok(Some(Message::Unit(u))) => g.lock().unwrap().push(Got { h: u.header, data: u.data, read_us: capture::now_us() }),
            Ok(Some(Message::Json(v))) => panic!("JSON on the frames pipe: {v}"),
            Ok(None) => return,
            Err(e) => panic!("the frames pipe: {e}"),
        }
    });
    got
}

fn take(got: &Arc<Mutex<Vec<Got>>>) -> Vec<Got> {
    std::mem::take(&mut *got.lock().unwrap())
}

fn pct(mut v: Vec<f64>, p: f64) -> f64 {
    assert!(!v.is_empty(), "no samples");
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() - 1) as f64 * p).round() as usize]
}

fn cpu_s() -> f64 {
    // SAFETY: getrusage fills the struct it's given.
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
    let t = |tv: libc::timeval| tv.tv_sec as f64 + tv.tv_usec as f64 / 1e6;
    t(ru.ru_utime) + t(ru.ru_stime)
}

/// Every unit of a layer decodes with VideoToolbox, in order.
fn decode_all(backend: &dyn VideoBackend, units: &[&Got]) -> (u64, u64) {
    let mut dec = backend.decoder(DecoderConfig::new(VideoCodec::H264), kessel_video::no_wake()).expect("a VideoToolbox H.264 decoder");
    let (mut ok, mut errors) = (0u64, 0u64);
    for u in units {
        let input = EncodedInput::new(VideoCodec::H264, u.data.clone(), u.h.key, u.h.captured_ns / 1000);
        let t0 = Instant::now();
        loop {
            match dec.submit(input.clone()) {
                Ok(()) => break,
                Err(kessel_video::DecodeError::Busy) if t0.elapsed() < Duration::from_secs(5) => std::thread::sleep(Duration::from_millis(1)),
                Err(e) => panic!("submit {:?}: {e}", u.h),
            }
        }
        // One output per input (no reordering); wait for it.
        let t0 = Instant::now();
        loop {
            match dec.poll() {
                Some(Ok(_)) => {
                    ok += 1;
                    break;
                }
                Some(Err(_)) => {
                    errors += 1;
                    break;
                }
                None if t0.elapsed() < Duration::from_secs(5) => std::thread::sleep(Duration::from_micros(200)),
                None => panic!("no decoded frame for {:?}", u.h),
            }
        }
    }
    (ok, errors)
}

fn h264(id: u8, max_width: u32, fps: u32, bitrate: u32) -> LayerSpec {
    LayerSpec { id, codec: Codec::H264, max_width, fps, bitrate, quality: None, on: true }
}

fn wait_units(got: &Arc<Mutex<Vec<Got>>>, pred: impl Fn(&[Got]) -> bool, within: Duration) {
    let t0 = Instant::now();
    while !pred(&got.lock().unwrap()) {
        assert!(t0.elapsed() < within, "the units didn't come within {within:?}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn deskds_encode_path_on_videotoolbox() {
    let backend = kessel_video::system(kessel_video::SystemConfig::default()).expect("kessel-video's Apple backend");
    let encoder = capture::probe_encoder(&*backend);
    assert_ne!(encoder, LinkEncoder::Unknown, "VideoToolbox must open an H.264 session here");
    let screen = Arc::new(Screen { backend: backend.clone(), moving: Arc::new(AtomicBool::new(true)), frames: Mutex::default(), opens: Mutex::default(), pushed: Arc::default(), gen_cpu_ns: Arc::default() });
    let (r, w) = std::io::pipe().expect("a pipe");
    let got = reader(r);
    let out = Arc::new(PipeOut { w: Mutex::new(w), events: Mutex::default() });
    let cap = Capture::spawn(ScreenOpener(screen.clone()), backend.clone(), out.clone(), encoder).expect("the capture thread");

    // The full layer (this machine, the LAN) and the remote one (Connect),
    // as the hub asks: the full one at the panel's width, 60 fps, 12 Mb/s;
    // the remote one 1920 wide, 30 fps, 4 Mb/s.
    let (full, remote) = (h264(0, 3024, 60, 12_000_000), h264(1, 1920, 30, 4_000_000));

    // The full layer alone first: one session's rate.
    cap.layers(vec![full]);
    std::thread::sleep(Duration::from_secs(3));
    let alone = take(&got);
    let s_alone = cap.stats().expect("stats");
    let span = |us: &[Got]| (us[us.len() - 1].h.captured_ns - us[0].h.captured_ns) as f64 / 1e9;
    let fps_alone = (alone.len() - 1) as f64 / span(&alone);
    let enc_alone = pct(alone.iter().map(|u| (u.h.emitted_ns - u.h.captured_ns) as f64 / 1e6).collect(), 0.5);
    let pushed = screen.pushed.swap(0, Ordering::SeqCst);
    println!("LIVE full alone: {fps_alone:.1} fps over {} units (encode p50 {enc_alone:.2} ms); the scripted screen pushed {pushed}; engine {s_alone:?}", alone.len());
    assert!(s_alone.submitted * 100 >= s_alone.frames * 95, "a lone 60 fps layer encodes (nearly) every frame the screen gives: {s_alone:?}");
    assert!(fps_alone >= 55.0, "full layer alone: {fps_alone:.1} fps");
    cap.layers(vec![]);
    std::thread::sleep(Duration::from_millis(300));
    take(&got);
    screen.opens.lock().unwrap().clear();

    let (cpu_0, gen_0, t_0) = (cpu_s(), screen.gen_cpu_ns.load(Ordering::Relaxed), Instant::now());
    let s_before = cap.stats().expect("stats");
    cap.layers(vec![full, remote]);
    std::thread::sleep(Duration::from_secs(5));
    let wall = t_0.elapsed().as_secs_f64();
    let gen_cpu = (screen.gen_cpu_ns.load(Ordering::Relaxed) - gen_0) as f64 / 1e9;
    let cpu = cpu_s() - cpu_0 - gen_cpu;
    let units = take(&got);
    println!("LIVE both layers: engine {:?} (before {s_before:?})", cap.stats().expect("stats"));
    assert_eq!(*screen.opens.lock().unwrap(), vec![Plan { display: 1, width: 3024, fps: 60 }], "one capture, at the wider and faster layer");
    let of = |l: u8| units.iter().filter(|u| u.h.layer == l).collect::<Vec<_>>();
    let (u0, u1) = (of(0), of(1));
    for (name, us) in [("full", &u0), ("remote", &u1)] {
        assert!(us[0].h.key && us[0].h.source_start, "{name}: the first unit is a key with source start: {:?}", us[0].h);
        assert_eq!(us.iter().filter(|u| u.h.key).count(), 1, "{name}: no periodic keyframes");
        assert!(us.windows(2).all(|p| p[1].h.seq == p[0].h.seq + 1), "{name}: seq has no gaps");
        assert!(us.iter().all(|u| u.data.starts_with(&[0, 0, 0, 1])), "{name}: Annex B");
        let (ok, errors) = decode_all(&*backend, us);
        assert_eq!((ok, errors), (us.len() as u64, 0), "{name}: every unit decodes");
    }
    assert_eq!((u0[0].h.width, u0[0].h.height), (3024, 1964));
    assert_eq!((u1[0].h.width, u1[0].h.height), (1920, 1246));
    let fps = |us: &[&Got]| (us.len() - 1) as f64 / ((us[us.len() - 1].h.captured_ns - us[0].h.captured_ns) as f64 / 1e9);
    let gaps = |us: &[&Got]| us.windows(2).map(|p| (p[1].h.captured_ns - p[0].h.captured_ns) as f64 / 1e6).collect::<Vec<_>>();
    let enc_ms = |us: &[&Got]| us.iter().map(|u| (u.h.emitted_ns - u.h.captured_ns) as f64 / 1e6).collect::<Vec<_>>();
    let pipe_ms = |us: &[&Got]| us.iter().map(|u| (u.read_us * 1000 - u.h.captured_ns) as f64 / 1e6).collect::<Vec<_>>();
    let kb = |us: &[&Got]| us.iter().map(|u| u.data.len()).sum::<usize>() as f64 * 8.0 / 1e6 / wall;
    let (fps0, fps1) = (fps(&u0), fps(&u1));
    let (gap0, gap1) = (pct(gaps(&u0), 0.5), pct(gaps(&u1), 0.5));
    println!(
        "LIVE full 3024x1964: {fps0:.1} fps (interval p50 {gap0:.2} ms), encode p50 {:.2} ms p95 {:.2} ms, capture->daemon read p50 {:.2} ms p95 {:.2} ms, {:.2} Mb/s",
        pct(enc_ms(&u0), 0.5),
        pct(enc_ms(&u0), 0.95),
        pct(pipe_ms(&u0), 0.5),
        pct(pipe_ms(&u0), 0.95),
        kb(&u0)
    );
    println!(
        "LIVE remote 1920x1246: {fps1:.1} fps (interval p50 {gap1:.2} ms), encode p50 {:.2} ms p95 {:.2} ms, capture->daemon read p50 {:.2} ms, {:.2} Mb/s",
        pct(enc_ms(&u1), 0.5),
        pct(enc_ms(&u1), 0.95),
        pct(pipe_ms(&u1), 0.5),
        kb(&u1)
    );
    println!(
        "LIVE encoder {encoder:?}; CPU {:.0}% of one core over {wall:.1} s (engine, two VideoToolbox sessions, the 3024->1920 scale, the reader; the scripted screen's spinning left out)",
        cpu / wall * 100.0
    );
    assert!(gap0 <= 18.5, "full layer at 60 fps: interval p50 {gap0:.2} ms");
    assert!((55.0..=62.0).contains(&fps0), "full layer fps {fps0:.1}");
    // A frame may go in a quarter interval early (the pacing slack), so a
    // 30 fps layer under a 60 fps capture measures a little over 30.
    assert!((27.0..=33.0).contains(&fps1), "remote layer fps {fps1:.1}");
    let s_both = cap.stats().expect("stats");
    assert!(s_both.busy - s_before.busy < 1_000, "a busy encoder is never retried in a tight loop: {s_both:?}");

    // A still screen: nothing at all.
    screen.moving.store(false, Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(200));
    take(&got);
    std::thread::sleep(Duration::from_secs(1));
    let still = take(&got);
    assert!(still.is_empty(), "a still screen encodes nothing: {} units", still.len());

    // A joiner's keyframe on the remote layer, still screen: the last
    // frame again, on that layer only, soon.
    let asked = capture::clock_ns();
    let t_ask = Instant::now();
    cap.keyframe(1, Some(asked));
    wait_units(&got, |g| !g.is_empty(), Duration::from_secs(2));
    let key_ms = t_ask.elapsed().as_secs_f64() * 1e3;
    std::thread::sleep(Duration::from_millis(200));
    let k = take(&got);
    assert_eq!(k.len(), 1, "one unit");
    assert!(k[0].h.layer == 1 && k[0].h.key && !k[0].h.source_start && k[0].h.captured_ns >= asked, "{:?}", k[0].h);
    println!("LIVE keyframe on a still screen: {key_ms:.1} ms from the ask to the daemon's read");

    // The display switch: every layer's next unit is a key with source
    // start, from the new display.
    screen.moving.store(true, Ordering::SeqCst);
    let t_sw = Instant::now();
    cap.display(2).expect("display 2");
    wait_units(&got, |g| g.iter().any(|u| u.h.layer == 0 && u.h.source == 2) && g.iter().any(|u| u.h.layer == 1 && u.h.source == 2), Duration::from_secs(3));
    let sw_ms = t_sw.elapsed().as_secs_f64() * 1e3;
    std::thread::sleep(Duration::from_millis(300));
    let after = take(&got);
    for l in [0u8, 1] {
        let first = after.iter().find(|u| u.h.layer == l && u.h.source == 2).expect("each layer moved");
        assert!(first.h.key && first.h.source_start && (first.h.width, first.h.height) == (1920, 1080), "{:?}", first.h);
    }
    assert!(after.iter().all(|u| u.h.source == 2 || u.h.captured_ns < after.iter().filter(|x| x.h.source == 2).map(|x| x.h.captured_ns).min().unwrap()), "nothing from the old display after the switch");
    println!("LIVE display switch: {sw_ms:.0} ms to both layers' first key from the new display");

    // Everything off: no capture.
    cap.layers(vec![]);
    std::thread::sleep(Duration::from_millis(300));
    take(&got);
    std::thread::sleep(Duration::from_millis(500));
    assert!(take(&got).is_empty(), "no layer on: nothing");
    assert!(out.events.lock().unwrap().is_empty(), "no failures: {:?}", out.events.lock().unwrap());
    let s = cap.stats().expect("stats");
    println!("LIVE engine stats: {s:?}");
}
