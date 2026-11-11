//! The engine with fakes: a scripted display capture (frames only when the
//! test pushes one, as ScreenCaptureKit gives them only on change) and
//! kessel-video's structural H.264 encoder. Logical time for pacing; the
//! encoders run on their own threads, so [`pump`] steps until the units
//! a test expects are out.

use super::*;
use k2_deskd_proto::link::Codec;
use kessel_video::fake::{pattern_frame, FakeBackend, FakeConfig};
use kessel_video::{SourceStats, VideoError};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

type Frames = Arc<Mutex<VecDeque<VideoFrame>>>;

#[derive(Default)]
pub(crate) struct Script {
    /// Display id → own size.
    displays: HashMap<u32, (u32, u32)>,
    main: Option<u32>,
    opens: Vec<Plan>,
    /// The latest capture's queues.
    frames: Option<Frames>,
    events: Option<Arc<Mutex<VecDeque<SourceEvent>>>>,
    open_error: Option<VideoError>,
    /// Captures dropped (closed).
    pub(crate) closed: Arc<Mutex<u32>>,
}

#[derive(Clone)]
pub(crate) struct FakeOpener(Arc<Mutex<Script>>);

/// One display (id 1, `w` x `h`) for other modules' tests.
pub(crate) fn fake_opener(w: u32, h: u32) -> (FakeOpener, Arc<Mutex<Script>>) {
    let mut s = Script { main: Some(1), ..Script::default() };
    s.displays.insert(1, (w, h));
    let script = Arc::new(Mutex::new(s));
    (FakeOpener(script.clone()), script)
}

/// Waits for the capture to open, then pushes one `w` x `h` frame
/// captured now.
pub(crate) fn push_frame(script: &Arc<Mutex<Script>>, w: u32, h: u32) {
    let t0 = Instant::now();
    let frames = loop {
        if let Some(f) = script.lock().unwrap().frames.clone() {
            break f;
        }
        assert!(t0.elapsed() < Duration::from_secs(5), "the capture never opened");
        std::thread::sleep(Duration::from_millis(2));
    };
    frames.lock().unwrap().push_back(pattern_frame(w, h, 1, now_us()).expect("a frame"));
}

struct FakeSource {
    frames: Frames,
    events: Arc<Mutex<VecDeque<SourceEvent>>>,
    closed: Arc<Mutex<u32>>,
}

impl FrameSource for FakeSource {
    fn try_recv(&mut self) -> Option<VideoFrame> {
        self.frames.lock().unwrap().pop_front()
    }
    fn try_event(&mut self) -> Option<SourceEvent> {
        self.events.lock().unwrap().pop_front()
    }
    fn set_constraints(&mut self, _c: kessel_video::CaptureConstraints) -> Result<(), VideoError> {
        Ok(())
    }
    fn set_paused(&mut self, _paused: bool) {}
    fn stats(&self) -> SourceStats {
        SourceStats::default()
    }
}

impl Drop for FakeSource {
    fn drop(&mut self) {
        *self.closed.lock().unwrap() += 1;
    }
}

impl Opener for FakeOpener {
    fn default_display(&mut self) -> Option<u32> {
        self.0.lock().unwrap().main
    }
    fn native_size(&mut self, display: u32) -> Option<(u32, u32)> {
        self.0.lock().unwrap().displays.get(&display).copied()
    }
    fn open(&mut self, plan: &Plan, _wake: Wake) -> Result<Box<dyn FrameSource>, VideoError> {
        let mut s = self.0.lock().unwrap();
        s.opens.push(*plan);
        if let Some(e) = s.open_error.take() {
            return Err(e);
        }
        let frames: Frames = Arc::default();
        let events: Arc<Mutex<VecDeque<SourceEvent>>> = Arc::default();
        s.frames = Some(frames.clone());
        s.events = Some(events.clone());
        Ok(Box::new(FakeSource { frames, events, closed: s.closed.clone() }))
    }
}

#[derive(Default)]
struct Rec {
    units: Mutex<Vec<(UnitHeader, Vec<u8>)>>,
    events: Mutex<Vec<Event>>,
}

impl Out for Rec {
    fn unit(&self, h: &UnitHeader, data: &[u8]) -> bool {
        self.units.lock().unwrap().push((*h, data.to_vec()));
        true
    }
    fn event(&self, e: &Event) {
        self.events.lock().unwrap().push(e.clone());
    }
}

impl Rec {
    fn take(&self) -> Vec<UnitHeader> {
        self.units.lock().unwrap().drain(..).map(|(h, _)| h).collect()
    }
    fn events(&self) -> Vec<Event> {
        self.events.lock().unwrap().drain(..).collect()
    }
}

struct T {
    engine: Engine<FakeOpener>,
    script: Arc<Mutex<Script>>,
    out: Rec,
    /// Logical µs.
    now: u64,
    n: u32,
}

fn t() -> T {
    let mut s = Script { main: Some(1), ..Script::default() };
    s.displays.insert(1, (1920, 1080));
    s.displays.insert(2, (1280, 720));
    let script = Arc::new(Mutex::new(s));
    let backend: Arc<dyn VideoBackend> = FakeBackend::new(FakeConfig::default());
    T { engine: Engine::new(FakeOpener(script.clone()), backend, kessel_video::no_wake()), script, out: Rec::default(), now: 1_000_000, n: 0 }
}

fn h264(id: u8, max_width: u32, fps: u32) -> LayerSpec {
    LayerSpec { id, codec: Codec::H264, max_width, fps, bitrate: 2_000_000, quality: None, on: true }
}

impl T {
    fn layers(&mut self, l: &[LayerSpec]) {
        self.engine.set_layers(l, self.now, &self.out);
    }

    /// The capture pushes one frame of the current display's size,
    /// captured now (a change on screen).
    fn frame(&mut self) {
        let s = self.script.lock().unwrap();
        let plan = *s.opens.last().expect("a capture is open");
        let (nw, nh) = s.displays[&plan.display];
        let (w, h) = layer_size((nw, nh), plan.width);
        self.n += 1;
        let f = pattern_frame(w, h, self.n, self.now).expect("a frame");
        s.frames.as_ref().expect("frames").lock().unwrap().push_back(f);
    }

    fn advance_ms(&mut self, ms: u64) {
        self.now += ms * 1000;
    }

    /// Steps (logical time still) until `n` units are out, or 5 s pass.
    fn pump(&mut self, n: usize) -> Vec<UnitHeader> {
        let t0 = Instant::now();
        loop {
            self.engine.step(self.now, &self.out);
            if self.out.units.lock().unwrap().len() >= n {
                break;
            }
            assert!(t0.elapsed() < Duration::from_secs(5), "wanted {n} units, got {:?}", self.out.units.lock().unwrap().iter().map(|u| u.0).collect::<Vec<_>>());
            std::thread::sleep(Duration::from_millis(1));
        }
        // Anything else the encoders give in a moment is a failure to see.
        std::thread::sleep(Duration::from_millis(30));
        self.engine.step(self.now, &self.out);
        self.out.take()
    }

    /// Steps for a while (logical time still) and returns every unit out.
    fn settle(&mut self) -> Vec<UnitHeader> {
        for _ in 0..40 {
            self.engine.step(self.now, &self.out);
            std::thread::sleep(Duration::from_millis(1));
        }
        self.out.take()
    }

    fn opens(&self) -> Vec<Plan> {
        self.script.lock().unwrap().opens.clone()
    }
}

// ── layer selection ───────────────────────────────────────────────────

#[test]
fn the_plan_is_the_widest_layer_at_the_highest_rate_within_the_display() {
    let native = (3024, 1964);
    assert_eq!(capture_plan(&[h264(0, 1512, 60), h264(1, 1920, 30)], 7, native), Some(Plan { display: 7, width: 1920, fps: 60 }));
    // 0 = the display's own width; a layer can't ask past it.
    assert_eq!(capture_plan(&[h264(0, 0, 30)], 7, native), Some(Plan { display: 7, width: 3024, fps: 30 }));
    assert_eq!(capture_plan(&[h264(0, 3840, 30)], 7, native).map(|p| p.width), Some(3024));
    // Off layers, and codecs the engine doesn't make, don't count.
    let off = LayerSpec { on: false, ..h264(1, 3000, 120) };
    let jpeg = LayerSpec { codec: Codec::Jpeg, ..h264(2, 3000, 120) };
    assert_eq!(capture_plan(&[h264(0, 1280, 30), off, jpeg], 7, native), Some(Plan { display: 7, width: 1280, fps: 30 }));
    assert_eq!(capture_plan(&[off, jpeg], 7, native), None);
    assert_eq!(capture_plan(&[], 7, native), None);
    assert_eq!(capture_plan(&[h264(0, 1280, 500)], 7, native).map(|p| p.fps), Some(MAX_FPS));
}

#[test]
fn a_layer_size_keeps_the_aspect_and_is_even() {
    assert_eq!(layer_size((3024, 1964), 0), (3024, 1964));
    assert_eq!(layer_size((3024, 1964), 4000), (3024, 1964));
    assert_eq!(layer_size((3024, 1964), 1512), (1512, 982));
    assert_eq!(layer_size((1920, 1080), 1280), (1280, 720));
    assert_eq!(layer_size((1921, 1081), 0), (1920, 1080));
    assert_eq!(layer_size((1920, 1080), 1), (16, 16));
}

#[test]
fn two_layers_share_one_capture_at_the_wider_and_faster_one() {
    let mut t = t();
    t.layers(&[h264(0, 1920, 60), h264(1, 1280, 30)]);
    assert_eq!(t.opens(), vec![Plan { display: 1, width: 1920, fps: 60 }], "one capture for both");
    t.frame();
    let u = t.pump(2);
    let by = |l: u8| u.iter().find(|h| h.layer == l).copied().expect("a unit per layer");
    assert_eq!((by(0).width, by(0).height), (1920, 1080));
    assert_eq!((by(1).width, by(1).height), (1280, 720), "the remote layer is scaled from the one capture");
    for h in [by(0), by(1)] {
        assert!(h.key && h.source_start, "{h:?}");
        assert_eq!((h.codec, h.source, h.seq), (Codec::H264, 1, 0));
        assert_eq!(h.captured_ns, t.now * 1000, "the frame's capture time, on deskd's clock in ns");
        assert!(h.emitted_ns >= h.captured_ns);
    }
}

#[test]
fn a_bitrate_change_never_reopens_the_capture_or_keys_and_a_wider_layer_does() {
    let mut t = t();
    t.layers(&[h264(0, 1280, 30)]);
    t.frame();
    t.pump(1);
    t.layers(&[LayerSpec { bitrate: 900_000, ..h264(0, 1280, 30) }]);
    t.advance_ms(40);
    t.frame();
    let u = t.pump(1);
    assert_eq!(t.opens().len(), 1, "same plan: no reopen");
    assert!(!u[0].key && !u[0].source_start, "a rate change is no keyframe: {:?}", u[0]);
    assert_eq!(u[0].seq, 1);
    // A wider layer: a new capture, and the size change starts the layer over.
    t.layers(&[h264(0, 1920, 30)]);
    assert_eq!(t.opens().last().copied(), Some(Plan { display: 1, width: 1920, fps: 30 }));
    assert_eq!(*t.script.lock().unwrap().closed.lock().unwrap(), 1, "the old capture closed first");
    t.advance_ms(40);
    t.frame();
    let u = t.pump(1);
    assert!(u[0].key && u[0].source_start && u[0].width == 1920, "{:?}", u[0]);
    // A faster layer reopens too (minimumFrameInterval follows the rate).
    t.layers(&[h264(0, 1920, 60)]);
    assert_eq!(t.opens().last().map(|p| p.fps), Some(60));
}

#[test]
fn no_layer_on_means_no_capture_and_unsupported_codecs_fail_once() {
    let mut t = t();
    let jpeg = LayerSpec { codec: Codec::Jpeg, ..h264(3, 1280, 15) };
    t.layers(&[jpeg]);
    assert!(t.opens().is_empty(), "nothing deskd can encode: no capture");
    assert_eq!(t.out.events(), vec![Event::LayerFailed { layer: 3, code: codes::UNSUPPORTED.into(), error: "K2 Screen can't encode jpeg here".into() }]);
    t.layers(&[jpeg, h264(0, 1280, 30)]);
    assert!(t.out.events().is_empty(), "reported once while it stays in the list");
    assert_eq!(t.opens().len(), 1);
    assert_eq!(t.engine.running(), vec![0]);
    // Everything off: the capture closes and nothing more comes.
    t.layers(&[]);
    assert_eq!(t.engine.plan(), None);
    assert_eq!(*t.script.lock().unwrap().closed.lock().unwrap(), 1);
    assert!(t.settle().is_empty());
}

// ── change detection ─────────────────────────────────────────────────

#[test]
fn a_still_screen_encodes_nothing() {
    let mut t = t();
    t.layers(&[h264(0, 1920, 60)]);
    t.frame();
    assert_eq!(t.pump(1).len(), 1);
    // Half a second of nothing on screen: no unit, no repeat.
    for _ in 0..30 {
        t.advance_ms(17);
        assert!(t.settle().is_empty());
    }
    assert_eq!(t.engine.stats().repeats, 0);
    // One change: one delta.
    t.frame();
    let u = t.pump(1);
    assert!(!u[0].key && u[0].seq == 1, "{:?}", u[0]);
}

#[test]
fn a_slower_layer_paces_itself_and_never_ends_on_a_stale_frame() {
    let mut t = t();
    t.layers(&[h264(0, 1920, 60), h264(1, 1280, 30)]);
    // Ten frames at 60 fps.
    let mut fast = 0;
    let mut slow = Vec::new();
    for _ in 0..10 {
        t.frame();
        for h in t.pump(1).into_iter().chain(t.settle()) {
            match h.layer {
                0 => fast += 1,
                _ => slow.push(h.captured_ns),
            }
        }
        t.advance_ms(16);
        t.advance_ms(1);
    }
    assert_eq!(fast, 10, "the 60 fps layer takes every frame");
    assert!((5..=6).contains(&slow.len()), "the 30 fps layer takes about every other: {}", slow.len());
    // The screen stops: the slow layer's interval passes and it encodes the
    // last frame it skipped, so its viewers see the final picture.
    let last_capture = (t.now - 17_000) * 1000;
    if slow.last() != Some(&last_capture) {
        t.advance_ms(40);
        let u = t.pump(1);
        assert_eq!((u[0].layer, u[0].captured_ns), (1, last_capture));
    }
}

// ── keyframes ────────────────────────────────────────────────────────

#[test]
fn a_keyframe_on_a_still_screen_repeats_the_last_frame_on_that_layer_only() {
    let mut t = t();
    t.layers(&[h264(0, 1920, 60), h264(1, 1280, 30)]);
    t.frame();
    t.pump(2);
    t.advance_ms(500);
    t.engine.keyframe(1, None, t.now);
    // Not at once: a fresh frame within REPEAT_AFTER would carry it.
    assert!(t.settle().is_empty());
    t.now += REPEAT_AFTER_US;
    let u = t.pump(1);
    assert_eq!(u.len(), 1, "{u:?}");
    assert!(u[0].layer == 1 && u[0].key && !u[0].source_start && u[0].seq == 1, "{:?}", u[0]);
    assert_eq!(u[0].captured_ns, t.now * 1000, "a repeat is stamped when it was made");
    assert_eq!(t.engine.stats().repeats, 1);
    // Asked once: nothing more.
    t.advance_ms(200);
    assert!(t.settle().is_empty());
}

#[test]
fn a_keyframe_since_a_time_skips_frames_captured_before_it() {
    let mut t = t();
    t.layers(&[h264(0, 1920, 60)]);
    t.frame();
    t.pump(1);
    t.advance_ms(17);
    // The daemon asks for a key since 10 ms from now (a joiner's time).
    let since_us = t.now + 10_000;
    t.engine.keyframe(0, Some(since_us * 1000), t.now);
    t.frame(); // captured before `since`: a delta
    let u = t.pump(1);
    assert!(!u[0].key, "{:?}", u[0]);
    t.advance_ms(17);
    t.frame(); // after: the key
    let u = t.pump(1);
    assert!(u[0].key && !u[0].source_start, "{:?}", u[0]);
}

#[test]
fn a_new_layer_on_a_still_screen_starts_from_the_last_frame() {
    let mut t = t();
    t.layers(&[h264(0, 1920, 60)]);
    t.frame();
    t.pump(1);
    t.advance_ms(300);
    // A remote viewer joins: the same plan (no reopen), a new layer.
    t.layers(&[h264(0, 1920, 60), h264(1, 1280, 30)]);
    assert_eq!(t.opens().len(), 1);
    t.now += REPEAT_AFTER_US;
    let u = t.pump(1);
    assert_eq!(u.len(), 1, "only the new layer: {u:?}");
    assert!(u[0].layer == 1 && u[0].key && u[0].source_start && u[0].seq == 0, "{:?}", u[0]);
}

#[test]
fn a_display_switch_moves_every_layer_with_a_source_start_key() {
    let mut t = t();
    t.layers(&[h264(0, 1920, 60), h264(1, 1280, 30)]);
    t.frame();
    t.pump(2);
    t.advance_ms(20);
    t.frame();
    t.pump(1);
    assert_eq!(t.engine.select_display(9, t.now, &t.out), Err((codes::NO_SUCH_DISPLAY, "no display 9".into())));
    // The display already shown: nothing changes, no keyframe owed.
    assert_eq!(t.engine.select_display(1, t.now, &t.out), Ok(1));
    assert_eq!(t.opens().len(), 1);
    t.advance_ms(20);
    t.frame();
    assert!(t.pump(1).iter().all(|h| !h.key), "no key for choosing the same display");
    assert_eq!(t.engine.select_display(2, t.now, &t.out), Ok(2));
    assert_eq!(t.opens().last().copied(), Some(Plan { display: 2, width: 1280, fps: 60 }), "one capture, on the new display");
    // A full interval of the 30 fps layer, so both layers are due.
    t.advance_ms(40);
    t.frame();
    let u = t.pump(2);
    for l in [0u8, 1] {
        let h = u.iter().find(|h| h.layer == l).copied().expect("each layer");
        assert!(h.key && h.source_start && h.source == 2, "{h:?}");
        assert_eq!((h.width, h.height), (1280, 720));
    }
    // The link closes: back to the main display next time.
    t.engine.reset();
    t.layers(&[h264(0, 1920, 60)]);
    assert_eq!(t.opens().last().map(|p| p.display), Some(1));
}

// ── failures ─────────────────────────────────────────────────────────

#[test]
fn a_capture_that_cant_open_says_why_and_is_tried_again() {
    let mut t = t();
    t.script.lock().unwrap().open_error = Some(VideoError::NoPermission);
    t.layers(&[h264(0, 1920, 60)]);
    assert_eq!(t.engine.plan(), None);
    match t.out.events().as_slice() {
        [Event::CaptureError { display: 1, code, .. }] => assert_eq!(code, codes::NO_SCREEN_PERMISSION),
        other => panic!("{other:?}"),
    }
    t.settle();
    assert_eq!(t.opens().len(), 1, "not before RETRY_US");
    t.now += RETRY_US;
    t.engine.step(t.now, &t.out);
    assert_eq!(t.opens().len(), 2);
    assert!(t.engine.plan().is_some(), "Screen Recording came back");
    t.frame();
    assert!(t.pump(1)[0].source_start);
}

#[test]
fn a_capture_that_ends_says_so_and_reopens() {
    let mut t = t();
    t.layers(&[h264(0, 1920, 60)]);
    t.frame();
    t.pump(1);
    t.script.lock().unwrap().events.as_ref().unwrap().lock().unwrap().push_back(SourceEvent::Ended(kessel_video::EndReason::Revoked));
    t.engine.step(t.now, &t.out);
    assert_eq!(t.engine.plan(), None);
    assert!(matches!(t.out.events().as_slice(), [Event::CaptureError { code, .. }] if code == codes::CAPTURE_FAILED));
    t.now += RETRY_US;
    t.engine.step(t.now, &t.out);
    assert_eq!(t.opens().len(), 2);
    t.frame();
    let u = t.pump(1);
    // Same display and size: the layer's encoder goes on (R4: no key is owed).
    assert!(!u[0].source_start && u[0].seq == 1, "{:?}", u[0]);
}

#[test]
fn a_layer_whose_encoder_cant_open_fails_alone() {
    let mut t = t();
    // The fake allows one encoder: the second layer's open fails.
    let script = t.script.clone();
    let mut cfg = FakeConfig::default();
    cfg.max_instances = Some(1);
    t.engine = Engine::new(FakeOpener(script), FakeBackend::new(cfg), kessel_video::no_wake());
    t.layers(&[h264(0, 1920, 60), h264(1, 1280, 30)]);
    t.frame();
    let u = t.pump(1);
    assert_eq!(u.iter().map(|h| h.layer).collect::<Vec<_>>(), vec![0]);
    match t.out.events().as_slice() {
        [Event::LayerFailed { layer: 1, code, .. }] => assert_eq!(code, codes::ENCODER_FAILED),
        other => panic!("{other:?}"),
    }
    assert_eq!(t.engine.running(), vec![0]);
}

/// An encoder that answers busy is tried again after a short backoff (or
/// its next output), never on every step: the live test once saw 86 000
/// busy submits in 5 s from a zero wait.
#[test]
fn a_busy_encoder_is_not_retried_in_a_tight_loop() {
    let mut t = t();
    let script = t.script.clone();
    let mut cfg = FakeConfig::default();
    cfg.encoder_faults = vec![(2, kessel_video::fake::EncoderFault::Busy)];
    t.engine = Engine::new(FakeOpener(script), FakeBackend::new(cfg), kessel_video::no_wake());
    t.layers(&[h264(0, 1920, 60)]);
    t.frame();
    t.pump(1);
    t.advance_ms(17);
    t.frame();
    let wait = t.engine.step(t.now, &t.out);
    assert_eq!(t.engine.stats().busy, 1);
    assert_eq!(wait, Duration::from_micros(BUSY_BACKOFF_US), "the thread waits out the backoff");
    for _ in 0..50 {
        t.engine.step(t.now, &t.out);
    }
    assert_eq!(t.engine.stats().busy, 1, "no retry at the same instant");
    t.now += BUSY_BACKOFF_US;
    let u = t.pump(1);
    assert!(!u[0].key && u[0].seq == 1, "the waiting frame goes in after the backoff: {:?}", u[0]);
}

/// The thread wrapper: commands reach the engine, units reach the out.
#[test]
fn the_capture_thread_runs_the_engine() {
    let mut s = Script { main: Some(1), ..Script::default() };
    s.displays.insert(1, (640, 360));
    let script = Arc::new(Mutex::new(s));
    let out = Arc::new(Rec::default());
    let backend: Arc<dyn VideoBackend> = FakeBackend::new(FakeConfig::default());
    let c = Capture::spawn(FakeOpener(script.clone()), backend, out.clone(), LinkEncoder::Software).expect("spawn");
    assert_eq!((c.codecs(), c.encoder()), (vec![Codec::H264], LinkEncoder::Software));
    c.layers(vec![h264(4, 0, 30)]);
    let t0 = Instant::now();
    let frames = loop {
        if let Some(f) = script.lock().unwrap().frames.clone() {
            break f;
        }
        assert!(t0.elapsed() < Duration::from_secs(5), "the capture never opened");
        std::thread::sleep(Duration::from_millis(2));
    };
    frames.lock().unwrap().push_back(pattern_frame(640, 360, 1, now_us()).unwrap());
    while out.units.lock().unwrap().is_empty() {
        assert!(t0.elapsed() < Duration::from_secs(5), "no unit");
        std::thread::sleep(Duration::from_millis(2));
    }
    let h = out.take()[0];
    assert!(h.key && h.source_start && h.layer == 4 && (h.width, h.height) == (640, 360), "{h:?}");
    assert_eq!(c.display(1), Ok(1));
    assert!(c.display(5).is_err());
    assert_eq!(c.stats().map(|s| s.units), Some(1));
    c.reset();
    drop(c);
}
