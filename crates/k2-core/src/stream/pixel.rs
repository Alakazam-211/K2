//! [`PixelLayers`]: today's pixel and JPEG frame sources as an
//! [`EncodedSource`]. It pulls each frame once and encodes it once per `on`
//! layer that is due: H.264 through [`k2_simvideo::open_h264`]
//! (VideoToolbox on a Mac, nothing elsewhere), JPEG through
//! [`crate::sim::encode`] (a lane JPEG that already fits goes through
//! untouched, as before).
//!
//! **Pacing.** Each layer runs on its own fixed schedule
//! ([`k2_simvideo::flow::Pacer`]): the next encode is due one gap after the
//! last was *due*, never after the encode ended, and the schedule restarts
//! from now after a stall instead of bursting. A frame up to a quarter gap
//! early goes; an older one waits for a fresher frame until a quarter gap
//! past due, then goes, so a still screen's last change is never lost.
//!
//! **Sizes** (protocol 4's `max_width`): 0 is the source's own native
//! size; a width at or below the screen's points width is points (no
//! scaling: the lane captures at points, or at `base` when that is
//! [`FrameSize::Native`], the `K2_SIM_FRAME_SIZE=native` rollback, and the
//! frame is scaled to points as before); a wider one is native pixels
//! scaled down to it (never up, keeping the aspect). The inner source is
//! retuned to the fastest layer's rate, and to native pixels (capped at
//! the widest layer when every layer has a width) while any layer is wider
//! than points. [`EncodedSource::sizes`] reports the points width of the
//! newest frame and the native width once a native frame showed it.
//!
//! **Keyframes.** A layer's first unit and the first after a size change
//! are key + `source_start`. [`EncodedSource::request_keyframe`] keys the
//! layer's next encode of a frame captured at or after `since`; when no such
//! frame comes within one layer gap (a still screen), it keys the newest
//! frame it has. Rate control may drop a frame (`Ok(None)` from the
//! encoder): the ask then stays for the next one.
//!
//! Encodes run on the caller's thread (the hub's capture thread), one after
//! another.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use image::RgbImage;
use k2_simvideo::flow::Pacer;
use k2_simvideo::{EncoderConfig, VideoEncoder};

use super::{thread_cpu_ns, Bytes, Codec, EncodeSpan, EncodedSource, EncodedUnit, LayerSpec, SourceError, SourceSizes};
use crate::sim::encode;
use crate::sim::lane::{Frame, FrameData, FrameOpts, FrameSize, FrameSource, LaneError};

/// A boxed lane source is a source (the manager hands out boxes).
impl<T: FrameSource + ?Sized> FrameSource for Box<T> {
    fn next_frame(&mut self, wait: Duration) -> Result<Option<Frame>, LaneError> {
        (**self).next_frame(wait)
    }

    fn retune(&mut self, opts: FrameOpts) {
        (**self).retune(opts)
    }
}

type Clock = Box<dyn Fn() -> Instant + Send>;

/// The newest frame pulled, and when.
struct Latest {
    frame: Frame,
    pulled_at: Instant,
    /// Pull count (1 = the first frame).
    n: u64,
}

#[derive(Debug, Clone, Copy)]
struct KeyAsk {
    since: Option<Instant>,
    asked: Instant,
}

struct Layer {
    spec: LayerSpec,
    pacer: Pacer,
    /// The last unit's seq (0 = none yet).
    seq: u64,
    /// The pull last encoded on this layer (0 = none).
    done: u64,
    /// The last unit's size: a different one is a source start.
    size: Option<(u32, u32)>,
    enc: Option<Box<dyn VideoEncoder>>,
    key: Option<KeyAsk>,
    /// The encoder's timestamps count from here.
    started: Instant,
    /// The encoder failed: no more units until a `set_layers` drops it.
    failed: bool,
}

impl Layer {
    fn new(spec: LayerSpec, now: Instant) -> Layer {
        Layer { spec, pacer: Pacer::new(spec.fps.max(1)), seq: 0, done: 0, size: None, enc: None, key: None, started: now, failed: false }
    }

    /// The same layer with new targets: no keyframe (a new width brings one
    /// with the first frame of the new size).
    fn retune(&mut self, spec: LayerSpec) {
        if spec.fps != self.spec.fps {
            self.pacer.set_fps(spec.fps.max(1));
        }
        let rate_changed = spec.fps != self.spec.fps || spec.bitrate != self.spec.bitrate;
        self.spec = spec;
        if let (true, Some(e)) = (rate_changed, self.enc.as_mut()) {
            let c = e.config();
            e.set_rate(spec.fps.max(1), bitrate_or_default(&spec, c.width, c.height));
        }
    }

    /// When this layer may next encode the newest frame (`None` = nothing to
    /// do until a new frame or a keyframe ask).
    fn encodable_at(&self, latest: Option<&Latest>, now: Instant) -> Option<Instant> {
        if self.failed {
            return None;
        }
        let l = latest?;
        if l.n == self.done && self.key.is_none() {
            return None;
        }
        let at = now + self.pacer.wait(l.pulled_at, now);
        match self.key {
            // The newest frame is older than the ask: wait for a newer one,
            // at most one gap (a still screen sends none).
            Some(k) if k.since.is_some_and(|s| l.frame.captured_at < s) => Some(at.max(k.asked + self.pacer.gap())),
            _ => Some(at),
        }
    }
}

fn bitrate_or_default(spec: &LayerSpec, w: u32, h: u32) -> u32 {
    if spec.bitrate > 0 {
        spec.bitrate
    } else {
        k2_simvideo::bitrate_for(w, h, spec.fps, false)
    }
}

/// The size a layer of width `max_width` sends a frame at: the frame's own
/// size for 0 (native), points for a width at or below its points width,
/// else the frame scaled down to the width (never up), keeping the aspect.
pub fn target_size(f: &Frame, max_width: u32) -> (u32, u32) {
    let (fw, fh) = (f.width_px.max(1), f.height_px.max(1));
    if max_width == 0 {
        return (fw, fh);
    }
    let (pw, ph) = points_of(f);
    if max_width <= pw {
        return (pw.max(1), ph.max(1));
    }
    if fw <= max_width {
        return (fw, fh);
    }
    let h = (fh as u64 * max_width as u64 + fw as u64 / 2) / fw as u64;
    (max_width, (h as u32).max(1))
}

/// The screen in points for this frame (the K2 headers): its own size when
/// it is points-sized (scale 1), else scaled by its scale.
pub fn points_of(f: &Frame) -> (u32, u32) {
    if (f.scale - 1.0).abs() < 1e-3 {
        (f.width_px, f.height_px)
    } else {
        encode::points_size(f)
    }
}

fn rgb_at(f: &Frame, w: u32, h: u32) -> Result<RgbImage, String> {
    Ok(encode::scale_to(encode::to_rgb(f)?, w, h))
}

/// A JPEG of `f` at `w`x`h` `(jpeg, w, h)`. A lane JPEG no wider than that
/// (one pixel over from rounding is fine) goes through byte for byte.
fn jpeg_at(f: &Frame, w: u32, h: u32, quality: u8) -> Result<(Vec<u8>, u32, u32), String> {
    if let FrameData::Jpeg(b) = &f.data {
        if f.width_px <= w + 1 {
            return Ok((b.clone(), f.width_px, f.height_px));
        }
    }
    Ok((encode::jpeg(&rgb_at(f, w, h)?, quality)?, w, h))
}

/// One encode's output: `(key, data, width, height)`.
type Encoded = (bool, Vec<u8>, u32, u32);

/// One H.264 encode; `None` = rate control dropped it.
fn h264_at(layer: &mut Layer, f: &Frame, w: u32, h: u32) -> Result<Option<Encoded>, String> {
    let rgb = rgb_at(f, w, h)?;
    let reopen = layer.enc.as_ref().is_none_or(|e| {
        let c = e.config();
        (c.width, c.height) != (w, h)
    });
    if reopen {
        drop(layer.enc.take()); // free the old session first
        let cfg = EncoderConfig { width: w, height: h, fps: layer.spec.fps.max(1), bitrate_bps: bitrate_or_default(&layer.spec, w, h) };
        layer.enc = Some(k2_simvideo::open_h264(&cfg)?);
    }
    let force = reopen || layer.key.is_some() || layer.size != Some((w, h));
    let ts_ms = f.captured_at.saturating_duration_since(layer.started).as_millis().min(u32::MAX as u128) as u32;
    let e = layer.enc.as_mut().ok_or("no encoder")?;
    Ok(e.encode_rgb(rgb.as_raw(), force, ts_ms)?.map(|c| (c.key, c.data, w, h)))
}

/// A pixel [`FrameSource`] as an [`EncodedSource`] (see the module docs).
pub struct PixelLayers<S: FrameSource> {
    source: S,
    base: FrameSize,
    tuned: FrameOpts,
    layers: Vec<Layer>,
    latest: Option<Latest>,
    pulls: u64,
    /// The newest frame's points width.
    points: Option<u32>,
    native: Option<u32>,
    ready: VecDeque<EncodedUnit>,
    failed: VecDeque<(u8, String)>,
    encodes: u64,
    clock: Clock,
}

impl<S: FrameSource> PixelLayers<S> {
    /// `opened_with` = what `source` was opened with (it is retuned only
    /// when the layers want something else); `base` = what the source
    /// captures at while every layer is points-sized.
    pub fn new(source: S, opened_with: FrameOpts, base: FrameSize) -> Self {
        Self::with_clock(source, opened_with, base, Box::new(Instant::now))
    }

    fn with_clock(source: S, opened_with: FrameOpts, base: FrameSize, clock: Clock) -> Self {
        PixelLayers { source, base, tuned: opened_with, layers: Vec::new(), latest: None, pulls: 0, points: None, native: None, ready: VecDeque::new(), failed: VecDeque::new(), encodes: 0, clock }
    }

    /// Units encoded so far, every layer.
    pub fn encodes(&self) -> u64 {
        self.encodes
    }

    fn on_frame(&mut self, f: Frame, now: Instant) {
        self.pulls += 1;
        self.points = Some(points_of(&f).0);
        // Native and uncapped (or under the cap): this is the source's own width.
        if self.tuned.size == FrameSize::Native && self.tuned.max_width_px.is_none_or(|c| f.width_px < c) {
            self.native = Some(f.width_px);
        }
        self.latest = Some(Latest { frame: f, pulled_at: now, n: self.pulls });
    }

    fn encode_due(&mut self, now: Instant) {
        for i in 0..self.layers.len() {
            match self.layers[i].encodable_at(self.latest.as_ref(), now) {
                Some(at) if at <= now => self.encode(i, now),
                _ => {}
            }
        }
    }

    fn encode(&mut self, i: usize, now: Instant) {
        let Some(l) = self.latest.as_ref() else { return };
        let layer = &mut self.layers[i];
        let f = &l.frame;
        // The schedule moves on from when this encode was due, not from
        // when it ends.
        layer.pacer.sent(now);
        layer.done = l.n;
        let (tw, th) = target_size(f, layer.spec.max_width);
        let (t0, c0) = (Instant::now(), thread_cpu_ns());
        let out = match layer.spec.codec {
            Codec::Jpeg => jpeg_at(f, tw, th, layer.spec.quality.unwrap_or(encode::JPEG_QUALITY)).map(|(d, w, h)| Some((true, d, w, h))),
            Codec::H264 => h264_at(layer, f, tw, th),
            Codec::Vp8 => Err("this source has no VP8 encoder".to_string()),
        };
        let span = EncodeSpan { start: t0, end: Instant::now(), cpu_ns: thread_cpu_ns().saturating_sub(c0) };
        match out {
            Ok(Some((key, data, w, h))) => {
                let source_start = key && layer.size != Some((w, h));
                if key {
                    layer.key = None;
                    layer.size = Some((w, h));
                }
                layer.seq += 1;
                self.encodes += 1;
                let (width_pt, height_pt) = points_of(f);
                self.ready.push_back(EncodedUnit {
                    codec: layer.spec.codec,
                    key,
                    source_start,
                    layer: layer.spec.id,
                    width: w,
                    height: h,
                    width_pt,
                    height_pt,
                    seq: layer.seq,
                    captured_at: f.captured_at,
                    stamps: f.stamps,
                    pulled_at: l.pulled_at,
                    encode: Some(span),
                    data: Bytes::from(data),
                });
            }
            Ok(None) => {} // rate control dropped it; any key ask waits for the next
            Err(why) => {
                layer.failed = true;
                layer.enc = None;
                self.failed.push_back((layer.spec.id, why));
            }
        }
    }

    /// The inner source follows the layers: the fastest one's rate, native
    /// pixels while any is wider than points (capped at the widest when
    /// every layer has a width).
    fn retune_source(&mut self) {
        let on: Vec<&LayerSpec> = self.layers.iter().filter(|l| !l.failed).map(|l| &l.spec).collect();
        if on.is_empty() {
            return;
        }
        let points = self.points;
        let native = on.iter().any(|s| s.max_width == 0 || points.is_some_and(|p| s.max_width > p));
        let opts = FrameOpts {
            max_fps: on.iter().map(|s| s.fps).max().unwrap_or(1).max(1),
            size: if native { FrameSize::Native } else { self.base },
            max_width_px: if native && on.iter().all(|s| s.max_width > 0) { on.iter().map(|s| s.max_width).max() } else { None },
        };
        if opts != self.tuned {
            self.source.retune(opts);
            self.tuned = opts;
        }
    }
}

impl<S: FrameSource> EncodedSource for PixelLayers<S> {
    fn next_unit(&mut self, wait: Duration) -> Result<Option<EncodedUnit>, SourceError> {
        let deadline = (self.clock)() + wait;
        loop {
            if let Some((layer, why)) = self.failed.pop_front() {
                return Err(SourceError::LayerFailed { layer, why });
            }
            if let Some(u) = self.ready.pop_front() {
                return Ok(Some(u));
            }
            let now = (self.clock)();
            self.encode_due(now);
            if !self.failed.is_empty() || !self.ready.is_empty() {
                continue;
            }
            if now >= deadline {
                return Ok(None);
            }
            // Wait for a frame, but no longer than until a layer may send
            // the one it has.
            let until = self.layers.iter().filter_map(|l| l.encodable_at(self.latest.as_ref(), now)).fold(deadline, Instant::min);
            match self.source.next_frame(until.saturating_duration_since(now).max(Duration::from_millis(1))) {
                Ok(Some(f)) => {
                    let at = (self.clock)();
                    self.on_frame(f, at);
                }
                Ok(None) => {}
                Err(e) => return Err(SourceError::Ended(e)),
            }
        }
    }

    fn set_layers(&mut self, specs: &[LayerSpec]) {
        let now = (self.clock)();
        let mut next: Vec<Layer> = Vec::new();
        for s in specs.iter().filter(|s| s.on) {
            if next.iter().any(|l| l.spec.id == s.id) {
                continue; // a repeated id: the first wins
            }
            match self.layers.iter().position(|l| l.spec.id == s.id && l.spec.codec == s.codec) {
                Some(i) => {
                    let mut l = self.layers.swap_remove(i);
                    l.retune(*s);
                    next.push(l);
                }
                None => next.push(Layer::new(*s, now)),
            }
        }
        self.layers = next;
        self.retune_source();
    }

    fn request_keyframe(&mut self, layer: u8, since: Option<Instant>) {
        let now = (self.clock)();
        if let Some(l) = self.layers.iter_mut().find(|l| l.spec.id == layer) {
            l.key = Some(match l.key {
                // Merged: a key at or after the later `since` answers both.
                Some(k) => KeyAsk { since: k.since.max(since), asked: k.asked },
                None => KeyAsk { since, asked: now },
            });
        }
    }

    fn sizes(&self) -> Option<SourceSizes> {
        self.points.map(|points| SourceSizes { points, native_px: self.native })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::lane::Orientation;
    use k2_simvideo::annexb::is_keyframe;
    use k2_simvideo::fake;
    use std::sync::{Arc, Mutex};

    type T = Arc<Mutex<Instant>>;

    /// A screen on a fake clock: a frame every `period` (captured at its
    /// tick), `next_frame` moves the clock to the tick or to the end of the
    /// wait. Sizes follow `retune` like the iOS lane: points 4x8 at scale
    /// 1, native 8x16 at scale 2 (capped by `max_width_px`).
    struct Screen {
        t: T,
        period: Duration,
        next: Instant,
        n: u64,
        /// Frames stop after this many (a still screen).
        still_after: Option<u64>,
        /// No frames in `[from, from + len)`.
        stall: Option<(Instant, Duration)>,
        opts: FrameOpts,
        retunes: Arc<Mutex<Vec<FrameOpts>>>,
        /// Hand out frames this size instead (a rotation), when set.
        rotate: Arc<Mutex<Option<(u32, u32)>>>,
        jpeg: bool,
    }

    impl Screen {
        fn new(t: &T, hz: u32) -> Screen {
            let now = *t.lock().unwrap();
            Screen {
                t: t.clone(),
                period: Duration::from_secs_f64(1.0 / hz as f64),
                next: now,
                n: 0,
                still_after: None,
                stall: None,
                opts: FrameOpts::points(60),
                retunes: Arc::new(Mutex::new(Vec::new())),
                rotate: Arc::new(Mutex::new(None)),
                jpeg: false,
            }
        }

        fn size(&self) -> (u32, u32, f32) {
            if let Some((w, h)) = *self.rotate.lock().unwrap() {
                return (w, h, 1.0);
            }
            match self.opts.size {
                FrameSize::Points => (4, 8, 1.0),
                FrameSize::Native => {
                    let w = self.opts.max_width_px.map_or(8, |c| c.min(8)).max(1);
                    (w, w * 2, w as f32 / 4.0)
                }
            }
        }
    }

    impl FrameSource for Screen {
        fn next_frame(&mut self, wait: Duration) -> Result<Option<Frame>, LaneError> {
            let now = *self.t.lock().unwrap();
            let mut due = self.next;
            if let Some((from, len)) = self.stall {
                if due >= from && due < from + len {
                    due = from + len;
                    self.next = due;
                }
            }
            if self.still_after.is_some_and(|k| self.n >= k) || due > now + wait {
                *self.t.lock().unwrap() = now + wait;
                return Ok(None);
            }
            let at = due.max(now);
            *self.t.lock().unwrap() = at;
            self.next = if at - due > self.period { at + self.period } else { due + self.period };
            self.n += 1;
            let (w, h, scale) = self.size();
            let data = if self.jpeg {
                FrameData::Jpeg(encode::jpeg(&RgbImage::from_pixel(w, h, image::Rgb([9, 99, 199])), 70).unwrap())
            } else {
                FrameData::Rgb888(vec![(self.n % 251) as u8; (w * h * 3) as usize])
            };
            Ok(Some(Frame { width_px: w, height_px: h, scale, orientation: Orientation::Portrait, seq: self.n, captured_at: at, stamps: Default::default(), data }))
        }

        fn retune(&mut self, opts: FrameOpts) {
            self.opts = opts;
            self.retunes.lock().unwrap().push(opts);
        }
    }

    fn layers_on(screen: Screen, t: &T) -> PixelLayers<Screen> {
        let t2 = t.clone();
        PixelLayers::with_clock(screen, FrameOpts::points(60), FrameSize::Points, Box::new(move || *t2.lock().unwrap()))
    }

    fn spec(id: u8, codec: Codec, fps: u32) -> LayerSpec {
        // 4 = the fake screen's points width: points-sized.
        LayerSpec { id, codec, max_width: 4, fps, bitrate: 500_000, quality: None, on: true }
    }

    /// Pull until the fake clock passes `until`.
    fn run(p: &mut PixelLayers<Screen>, t: &T, until: Instant) -> Vec<EncodedUnit> {
        run_timed(p, t, until).into_iter().map(|(u, _)| u).collect()
    }

    /// [`run`] with each unit's send time: the fake clock when `next_unit`
    /// handed it over (encodes don't move the fake clock).
    fn run_timed(p: &mut PixelLayers<Screen>, t: &T, until: Instant) -> Vec<(EncodedUnit, Instant)> {
        let mut out = Vec::new();
        while *t.lock().unwrap() < until {
            match p.next_unit(Duration::from_millis(50)) {
                Ok(Some(u)) => out.push((u, *t.lock().unwrap())),
                Ok(None) => {}
                Err(e) => panic!("{e:?}"),
            }
        }
        out
    }

    /// The next unit of any layer.
    fn next(p: &mut PixelLayers<Screen>) -> EncodedUnit {
        loop {
            if let Some(u) = p.next_unit(Duration::from_millis(50)).expect("unit") {
                return u;
            }
        }
    }

    fn of(units: &[EncodedUnit], layer: u8) -> Vec<&EncodedUnit> {
        units.iter().filter(|u| u.layer == layer).collect()
    }

    /// Each layer keeps its own fixed schedule off one 60 Hz pull: 60, 30,
    /// 24 and 15 fps over ten seconds, never two units closer than three
    /// quarters of a gap, each frame encoded once per layer.
    #[test]
    fn each_layer_keeps_its_own_fixed_schedule() {
        let t: T = Arc::new(Mutex::new(Instant::now()));
        let t0 = *t.lock().unwrap();
        let mut p = layers_on(Screen::new(&t, 60), &t);
        p.set_layers(&[spec(0, Codec::Jpeg, 60), spec(1, Codec::Jpeg, 30), spec(2, Codec::Jpeg, 24), spec(3, Codec::Jpeg, 15)]);
        let timed = run_timed(&mut p, &t, t0 + Duration::from_secs(10));
        let units: Vec<EncodedUnit> = timed.iter().map(|(u, _)| u.clone()).collect();
        for (id, fps) in [(0u8, 60u32), (1, 30), (2, 24), (3, 15)] {
            let l = of(&units, id);
            let want = fps as usize * 10;
            assert!((want - 2..=want + 2).contains(&l.len()), "layer {id} at {fps}: {} units", l.len());
            let gap = Duration::from_secs_f64(1.0 / fps as f64);
            let sent: Vec<Instant> = timed.iter().filter(|(u, _)| u.layer == id).map(|(_, at)| *at).collect();
            assert!(sent.windows(2).all(|w| w[1] - w[0] >= gap * 3 / 4), "layer {id} bunches");
            assert!(l.windows(2).all(|w| w[1].seq == w[0].seq + 1), "seq +1 per unit");
            assert!(l[0].key && l[0].source_start && l.iter().skip(1).all(|u| u.key && !u.source_start), "JPEG is always key; one source start");
        }
        assert_eq!(p.encodes() as usize, units.len() + p.ready.len(), "one encode per unit (some still queued)");
        // The inner source was retuned once, to the fastest layer (it was
        // opened at 60 points already: no retune).
        assert!(p.source.retunes.lock().unwrap().is_empty());
    }

    /// After a stall the schedule restarts from the first new frame: no
    /// burst to catch up.
    #[test]
    fn a_stall_restarts_the_schedule_instead_of_bursting() {
        let t: T = Arc::new(Mutex::new(Instant::now()));
        let t0 = *t.lock().unwrap();
        let mut s = Screen::new(&t, 60);
        s.stall = Some((t0 + Duration::from_secs(1), Duration::from_secs(1)));
        let mut p = layers_on(s, &t);
        p.set_layers(&[spec(0, Codec::Jpeg, 30)]);
        let timed = run_timed(&mut p, &t, t0 + Duration::from_secs(3));
        let gap = Duration::from_secs_f64(1.0 / 30.0);
        assert!(timed.windows(2).all(|w| w[1].1 - w[0].1 >= gap * 3 / 4), "no burst");
        let units: Vec<EncodedUnit> = timed.into_iter().map(|(u, _)| u).collect();
        let back = t0 + Duration::from_secs(2);
        let after: Vec<&EncodedUnit> = units.iter().filter(|u| u.captured_at >= back).collect();
        assert_eq!(after[0].captured_at, back, "the first frame after the stall goes at once");
        assert!((28..=32).contains(&after.len()), "then 30 fps again: {}", after.len());
    }

    /// A still screen's last change is sent (held until a quarter gap past
    /// due, never dropped), and a keyframe ask on it keys the newest frame.
    #[test]
    fn a_still_screen_sends_its_last_change_and_keys_it_on_request() {
        let _g = fake::FACTORY_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _log = fake::install();
        let t: T = Arc::new(Mutex::new(Instant::now()));
        let t0 = *t.lock().unwrap();
        let mut s = Screen::new(&t, 60);
        s.still_after = Some(3);
        let mut p = layers_on(s, &t);
        p.set_layers(&[spec(0, Codec::H264, 15)]);
        let units = run(&mut p, &t, t0 + Duration::from_millis(500));
        // Frames 1-3 come 16 ms apart; at 15 fps the layer sends frame 1,
        // then holds frame 3 (early) until a quarter gap past due and sends it.
        assert_eq!(units.len(), 2, "{units:?}");
        assert!(units[0].key && !units[1].key);
        assert_eq!(units[1].captured_at, t0 + Duration::from_secs_f64(1.0 / 60.0) * 2, "the newest frame, not a stale one");
        // A joiner's ask on the still screen: no newer frame comes, so the
        // newest is keyed one layer gap after the ask.
        let asked = *t.lock().unwrap();
        p.request_keyframe(0, Some(asked));
        let more = run(&mut p, &t, asked + Duration::from_millis(500));
        assert_eq!(more.len(), 1, "{more:?}");
        assert!(more[0].key && !more[0].source_start && is_keyframe(&more[0].data));
        assert_eq!(more[0].seq, 3);
        fake::uninstall();
    }

    /// H.264: the layer's first unit is key + source start, then deltas; an
    /// ask keys the next one; a rotation (new size) reopens the encoder
    /// with a key + source start. One encoder per layer and size.
    #[test]
    fn h264_layers_key_on_start_on_request_and_on_a_new_size() {
        let _g = fake::FACTORY_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let log = fake::install();
        let t: T = Arc::new(Mutex::new(Instant::now()));
        let s = Screen::new(&t, 60);
        let rotate = s.rotate.clone();
        let mut p = layers_on(s, &t);
        p.set_layers(&[spec(0, Codec::H264, 30)]);
        let flags = |u: &EncodedUnit| (u.key, u.source_start, (u.width, u.height));
        let a = next(&mut p);
        let b = next(&mut p);
        assert_eq!(flags(&a), (true, true, (4, 8)));
        assert_eq!(flags(&b), (false, false, (4, 8)));
        assert!(is_keyframe(&a.data) && !is_keyframe(&b.data), "the flag matches the bitstream");
        let _ = next(&mut p);
        p.request_keyframe(0, None);
        let c = next(&mut p);
        assert_eq!(flags(&c), (true, false, (4, 8)), "a key on request, not a source start");
        *rotate.lock().unwrap() = Some((8, 4));
        let d = next(&mut p);
        let e = next(&mut p);
        assert_eq!(flags(&d), (true, true, (8, 4)), "rotation");
        assert_eq!(flags(&e), (false, false, (8, 4)));
        assert_eq!((a.seq, b.seq, c.seq, d.seq, e.seq), (1, 2, 4, 5, 6));
        fake::uninstall();
        assert_eq!(log.lock().unwrap().opened, vec![(4, 8), (8, 4)], "one encoder per size");
    }

    /// A layer whose encoder can't open fails once (the hub moves its
    /// viewers to JPEG); a JPEG layer beside it carries on.
    #[test]
    fn a_failed_encoder_fails_its_layer_once_and_the_others_carry_on() {
        let _g = fake::FACTORY_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        fn failing(_: &EncoderConfig) -> Result<Box<dyn VideoEncoder>, String> {
            Err("no encoder here (test)".into())
        }
        k2_simvideo::set_h264_factory(Some(failing));
        let t: T = Arc::new(Mutex::new(Instant::now()));
        let t0 = *t.lock().unwrap();
        let mut p = layers_on(Screen::new(&t, 60), &t);
        p.set_layers(&[spec(4, Codec::H264, 30), spec(5, Codec::Jpeg, 30)]);
        let mut fails = Vec::new();
        let mut jpeg = 0;
        while *t.lock().unwrap() < t0 + Duration::from_secs(1) {
            match p.next_unit(Duration::from_millis(50)) {
                Ok(Some(u)) => {
                    assert_eq!(u.layer, 5);
                    jpeg += 1;
                }
                Ok(None) => {}
                Err(e) => fails.push(e),
            }
        }
        k2_simvideo::set_h264_factory(None);
        assert_eq!(fails.len(), 1, "{fails:?}");
        assert!(matches!(&fails[0], SourceError::LayerFailed { layer: 4, why } if why.contains("no encoder here")));
        assert!((28..=32).contains(&jpeg), "{jpeg}");
    }

    /// Widths: one wider than points makes the inner source go native
    /// (capped when every layer has a width); a layer scales down, never
    /// up; 0 is native; a points-wide layer stays points from native
    /// frames; the native width is learnt from an uncapped native frame; a
    /// new size is a source start. Rates: the source follows the fastest.
    #[test]
    fn widths_retune_the_source_and_scale_each_layer() {
        let t: T = Arc::new(Mutex::new(Instant::now()));
        let s = Screen::new(&t, 60);
        let retunes = s.retunes.clone();
        let mut p = layers_on(s, &t);
        let pull = |p: &mut PixelLayers<Screen>, layer: u8| loop {
            if let Some(u) = p.next_unit(Duration::from_millis(50)).expect("unit") {
                if u.layer == layer {
                    return u;
                }
            }
        };
        assert_eq!(p.sizes(), None, "no frame yet");
        p.set_layers(&[spec(0, Codec::Jpeg, 15)]);
        let u = pull(&mut p, 0);
        assert_eq!((u.width, u.height, u.width_pt, u.height_pt), (4, 8, 4, 8), "points");
        assert_eq!(p.sizes(), Some(SourceSizes { points: 4, native_px: None }), "native unknown from points frames");
        p.set_layers(&[LayerSpec { max_width: 6, ..spec(0, Codec::Jpeg, 15) }]);
        let u = pull(&mut p, 0);
        assert_eq!((u.width, u.height, u.key, u.source_start), (6, 12, true, true), "capped native, a source start");
        assert_eq!((u.width_pt, u.height_pt), (4, 8), "still the same screen in points");
        assert_eq!(p.sizes().and_then(|s| s.native_px), None, "a capped frame is not the native width");
        p.set_layers(&[LayerSpec { max_width: 6, ..spec(0, Codec::Jpeg, 15) }, LayerSpec { max_width: 0, ..spec(1, Codec::Jpeg, 30) }]);
        // Its first unit may be the 6x12 frame already pulled; then native.
        let mut u = pull(&mut p, 1);
        if u.width == 6 {
            u = pull(&mut p, 1);
        }
        assert_eq!((u.width, u.height), (8, 16), "0 is native");
        assert_eq!(p.sizes(), Some(SourceSizes { points: 4, native_px: Some(8) }));
        let u = pull(&mut p, 0);
        assert_eq!((u.width, u.height, u.source_start), (6, 12, false), "scaled down from 8x16, same size as before");
        p.set_layers(&[LayerSpec { max_width: 99, ..spec(0, Codec::Jpeg, 15) }, spec(2, Codec::Jpeg, 15)]);
        let u = pull(&mut p, 0);
        assert_eq!((u.width, u.height, u.source_start), (8, 16, true), "never scaled up past native");
        let u = pull(&mut p, 2);
        assert_eq!((u.width, u.height), (4, 8), "a points-wide layer is points from native frames");
        let got: Vec<(u32, FrameSize, Option<u32>)> = retunes.lock().unwrap().iter().map(|o| (o.max_fps, o.size, o.max_width_px)).collect();
        assert_eq!(
            got,
            vec![(15, FrameSize::Points, None), (15, FrameSize::Native, Some(6)), (30, FrameSize::Native, None), (15, FrameSize::Native, Some(99))],
            "one retune per change"
        );
    }

    /// A lane JPEG that already fits streams byte for byte, once per layer.
    #[test]
    fn a_fitting_lane_jpeg_passes_through() {
        let t: T = Arc::new(Mutex::new(Instant::now()));
        let mut s = Screen::new(&t, 60);
        s.jpeg = true;
        let mut p = layers_on(s, &t);
        p.set_layers(&[spec(0, Codec::Jpeg, 30)]);
        let u = loop {
            if let Some(u) = p.next_unit(Duration::from_millis(50)).expect("unit") {
                break u;
            }
        };
        let FrameData::Jpeg(src) = &p.latest.as_ref().expect("latest").frame.data else { unreachable!() };
        assert_eq!(&u.data[..], &src[..], "byte for byte");
    }

    /// A paused layer (off, or missing from the list) sends nothing; when it
    /// comes back it starts again with a source start.
    #[test]
    fn a_paused_layer_sends_nothing_and_restarts_with_a_source_start() {
        let t: T = Arc::new(Mutex::new(Instant::now()));
        let t0 = *t.lock().unwrap();
        let mut p = layers_on(Screen::new(&t, 60), &t);
        p.set_layers(&[spec(0, Codec::Jpeg, 30), spec(1, Codec::Jpeg, 15)]);
        let _ = run(&mut p, &t, t0 + Duration::from_millis(500));
        p.set_layers(&[spec(0, Codec::Jpeg, 30), LayerSpec { on: false, ..spec(1, Codec::Jpeg, 15) }]);
        let units = run(&mut p, &t, t0 + Duration::from_millis(1000));
        assert!(units.iter().all(|u| u.layer == 0), "layer 1 is paused");
        p.set_layers(&[spec(0, Codec::Jpeg, 30), spec(1, Codec::Jpeg, 15)]);
        let units = run(&mut p, &t, t0 + Duration::from_millis(1500));
        let back = of(&units, 1);
        assert!(back[0].source_start && back[0].seq == 1, "{:?}", back[0]);
        assert!(of(&units, 0).iter().all(|u| !u.source_start), "layer 0 kept going");
    }
}
