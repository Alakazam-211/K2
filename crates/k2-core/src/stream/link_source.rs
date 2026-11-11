//! [`LinkSource`]: a protocol-4 source (k2-deskd; the sim helper once it
//! encodes) as an [`EncodedSource`] (PRD remote desktop §3.4 E10). The
//! source encodes; this relays its units to a hub.
//!
//! **Two halves.** A [`LinkFeed`] owns the frames pipe (any `Read + Send`:
//! the read end deskd hands over by `SCM_RIGHTS` as an `OwnedFd` / `File`,
//! or a child's fd 3) and one reader thread for the pipe's life. A
//! [`LinkSource`] is one hub's view of a feed: it turns units into
//! [`EncodedUnit`]s and sends the hub's `layers` and `keyframe` asks through
//! a sender the caller gives ([`SendRequest`]: the deskd link owns its
//! socket, so this never touches it). One source is attached to a feed at a
//! time; a hub that ends drops its source (which pauses every layer) and
//! the next hub attaches a new one to the same feed, so no second reader
//! ever races the first on the pipe.
//!
//! **Clock.** A unit's capture and emit times are on the source's clock;
//! [`LinkClock`] maps them onto the daemon's from `hello.clock_ns` and the
//! moment the hello reply was read, never later than the unit's read time.
//!
//! **Sizes** come from the caller ([`SizesFn`], deskd's display info),
//! because the hub keeps a layer paused until it knows the screen's width,
//! and a link source sends nothing before it has a layer. A unit's
//! `wpt` / `hpt` are its display's points size; a display the caller
//! doesn't know reads as its own pixels.
//!
//! **Queue.** The reader puts units in a bounded queue ([`LINK_QUEUE`]).
//! When it is full a delta is dropped and its layer **drops to the next
//! key unit**: its later deltas are dropped too, and the source is asked for
//! a keyframe once. A key unit that finds the queue full replaces its own
//! layer's queued units, and if that frees nothing, the oldest unit's layer
//! drops to a key the same way. Every dropped unit is counted
//! ([`LinkStats`]).
//!
//! **Displays.** A unit's `source` is deskd's display id. A layer whose
//! source changes must start with a key + `source_start` unit (deskd puts
//! the keyframe on the new display's first frame); that unit makes it the
//! current display (the one [`EncodedSource::sizes`] reports) and is counted
//! as a switch. A unit from another display without `source_start` is a
//! stale one and is dropped.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{self, Read};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use k2_simvideo::link::{self, HelloFields, Message, Request, Unit};

use super::{Bytes, EncodedSource, EncodedUnit, LayerSpec, SourceError, SourceSizes};
use crate::sim::lane::{FrameStamps, LaneError};

/// Units the feed keeps for its source before it drops to keyframes (about
/// half a second of one 60 fps layer).
pub const LINK_QUEUE: usize = 32;

/// Writes one protocol-4 request to the source (the caller serialises it,
/// adds `"nr": true` and writes it on its own link).
pub type SendRequest = Box<dyn FnMut(&Request) -> io::Result<()> + Send>;

/// A display's size by its id (deskd's `displays`); `None` = unknown.
pub type SizesFn = Box<dyn Fn(u32) -> Option<DisplaySize> + Send>;

/// One display's size, as deskd reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplaySize {
    /// Width and height in points (input coordinates; the K2SF / K2SV
    /// header's `wpt` / `hpt`).
    pub points: (u32, u32),
    /// Width and height in native pixels, when known.
    pub native_px: Option<(u32, u32)>,
}

/// The source's clock (`hello.clock_ns`, ns of uptime) against the
/// daemon's, taken once when the hello reply arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkClock {
    pub source_ns: u64,
    pub at: Instant,
}

impl LinkClock {
    pub fn new(source_ns: u64, at: Instant) -> Self {
        LinkClock { source_ns, at }
    }

    /// From the hello fields, read at `at`.
    pub fn from_hello(h: &HelloFields, at: Instant) -> Self {
        LinkClock::new(h.clock_ns, at)
    }

    /// A source time on the daemon's clock, never after `limit` (a unit
    /// can't have been made after the daemon read it).
    pub fn to_instant(&self, ns: u64, limit: Instant) -> Instant {
        let t = if ns >= self.source_ns {
            self.at.checked_add(Duration::from_nanos(ns - self.source_ns))
        } else {
            self.at.checked_sub(Duration::from_nanos(self.source_ns - ns))
        };
        t.unwrap_or(limit).min(limit)
    }

    /// A daemon time on the source's clock (a keyframe's `since_ns`).
    pub fn to_ns(&self, t: Instant) -> u64 {
        if t >= self.at {
            self.source_ns.saturating_add(t.duration_since(self.at).as_nanos().min(u64::MAX as u128) as u64)
        } else {
            self.source_ns.saturating_sub(self.at.duration_since(t).as_nanos().min(u64::MAX as u128) as u64)
        }
    }
}

/// What a feed has done so far.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LinkStats {
    /// Units read off the pipe.
    pub read: u64,
    /// Units dropped by the queue rule (overflow and the drop to a key) or
    /// as stale (another display without `source_start`).
    pub dropped: u64,
    /// Times a layer dropped to a key unit.
    pub overflows: u64,
    /// Display switches seen (`source_start` on a new source id).
    pub switches: u64,
    /// Units read while no source was attached (no hub runs).
    pub idle: u64,
}

#[derive(Default)]
struct Counters {
    read: AtomicU64,
    dropped: AtomicU64,
    overflows: AtomicU64,
    switches: AtomicU64,
    idle: AtomicU64,
}

/// One unit as read, and when.
struct Raw {
    unit: Unit,
    received_at: Instant,
}

struct Queue {
    units: VecDeque<Raw>,
    cap: usize,
    /// The attached source's generation (0 = none attached).
    attached: u64,
    next_gen: u64,
    /// Layers dropping to the next key unit.
    broken: HashSet<u8>,
    /// Layers that need a keyframe ask (sent by the source, which holds
    /// the sender).
    asks: Vec<u8>,
    /// Layers the source said it can't run (the route's
    /// [`LinkFeed::fail_layer`]), reported to the hub one at a time.
    failed: VecDeque<(u8, String)>,
    ended: Option<String>,
}

struct Shared {
    q: Mutex<Queue>,
    cv: Condvar,
    n: Counters,
}

fn lock(m: &Mutex<Queue>) -> MutexGuard<'_, Queue> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Shared {
    fn count(&self, c: &AtomicU64, n: usize) {
        c.fetch_add(n as u64, Ordering::Relaxed);
    }

    /// `layer` drops to its next key unit: its queued units go, and its
    /// source is asked for a key once.
    fn drop_to_key(&self, q: &mut Queue, layer: u8) {
        let before = q.units.len();
        q.units.retain(|r| r.unit.header.layer != layer);
        self.count(&self.n.dropped, before - q.units.len());
        if q.broken.insert(layer) {
            self.count(&self.n.overflows, 1);
            q.asks.push(layer);
        }
    }

    /// One unit off the pipe (counted as read once it is queued or dropped).
    fn push(&self, unit: Unit, received_at: Instant) {
        self.queue(unit, received_at);
        self.count(&self.n.read, 1);
    }

    fn queue(&self, unit: Unit, received_at: Instant) {
        let mut q = lock(&self.q);
        if q.attached == 0 {
            self.count(&self.n.idle, 1);
            return;
        }
        let layer = unit.header.layer;
        if q.broken.contains(&layer) {
            if !unit.header.key {
                self.count(&self.n.dropped, 1);
                return;
            }
            q.broken.remove(&layer);
        }
        if q.units.len() >= q.cap {
            if !unit.header.key {
                self.count(&self.n.dropped, 1);
                if q.broken.insert(layer) {
                    self.count(&self.n.overflows, 1);
                    q.asks.push(layer);
                }
                return;
            }
            // A key unit supersedes its own layer's queued units.
            let before = q.units.len();
            q.units.retain(|r| r.unit.header.layer != layer);
            if q.units.len() < before {
                self.count(&self.n.dropped, before - q.units.len());
                self.count(&self.n.overflows, 1);
            }
            if q.units.len() >= q.cap {
                if let Some(oldest) = q.units.front().map(|r| r.unit.header.layer) {
                    self.drop_to_key(&mut q, oldest);
                }
            }
        }
        q.units.push_back(Raw { unit, received_at });
        drop(q);
        self.cv.notify_all();
    }

    fn end(&self, why: String) {
        lock(&self.q).ended = Some(why);
        self.cv.notify_all();
    }
}

/// The reader side of one frames pipe. Cloning shares it.
#[derive(Clone)]
pub struct LinkFeed {
    shared: Arc<Shared>,
}

impl LinkFeed {
    /// Starts the pipe's reader thread, with a queue of [`LINK_QUEUE`].
    pub fn spawn<R: Read + Send + 'static>(frames: R) -> io::Result<LinkFeed> {
        LinkFeed::with_queue(frames, LINK_QUEUE)
    }

    /// [`Self::spawn`] with a queue of `cap` units (at least 1).
    pub fn with_queue<R: Read + Send + 'static>(mut frames: R, cap: usize) -> io::Result<LinkFeed> {
        let shared = Arc::new(Shared {
            q: Mutex::new(Queue { units: VecDeque::new(), cap: cap.max(1), attached: 0, next_gen: 1, broken: HashSet::new(), asks: Vec::new(), failed: VecDeque::new(), ended: None }),
            cv: Condvar::new(),
            n: Counters::default(),
        });
        let s = shared.clone();
        std::thread::Builder::new().name("k2-link-feed".into()).spawn(move || loop {
            match link::read_message(&mut frames) {
                Ok(Some(Message::Unit(u))) => s.push(u, Instant::now()),
                // The frames pipe carries units; anything else on it is ignored.
                Ok(Some(Message::Json(_))) => {}
                Ok(None) => return s.end("the frames pipe closed".into()),
                Err(e) => return s.end(format!("the frames pipe: {e}")),
            }
        })?;
        Ok(LinkFeed { shared })
    }

    pub fn stats(&self) -> LinkStats {
        let n = &self.shared.n;
        let g = |c: &AtomicU64| c.load(Ordering::Relaxed);
        LinkStats { read: g(&n.read), dropped: g(&n.dropped), overflows: g(&n.overflows), switches: g(&n.switches), idle: g(&n.idle) }
    }

    /// The pipe ended (EOF or a read error), and why.
    pub fn ended(&self) -> Option<String> {
        lock(&self.shared.q).ended.clone()
    }

    /// The source can't run `layer` (deskd's `layer_failed` event on its
    /// control link: a codec it has no encoder for, a session that won't
    /// open). The attached source reports it to its hub as
    /// [`SourceError::LayerFailed`] (the hub moves that layer's viewers to
    /// JPEG) ahead of any queued unit, and the layer's queued units go.
    /// Nothing is attached: ignored (a new hub sends its own layers).
    pub fn fail_layer(&self, layer: u8, why: impl Into<String>) {
        let mut q = lock(&self.shared.q);
        if q.attached == 0 {
            return;
        }
        let before = q.units.len();
        q.units.retain(|r| r.unit.header.layer != layer);
        self.shared.count(&self.shared.n.dropped, before - q.units.len());
        q.failed.push_back((layer, why.into()));
        drop(q);
        self.shared.cv.notify_all();
    }
}

/// One hub's source over a [`LinkFeed`]. Dropping it pauses every layer
/// (an empty `layers`) and detaches it from the feed.
pub struct LinkSource {
    shared: Arc<Shared>,
    gen: u64,
    clock: LinkClock,
    sizes: SizesFn,
    send: SendRequest,
    /// The current display (the last switch, or the caller's to start).
    display: u32,
    /// Each layer's display (a change must be a source start).
    layer_display: HashMap<u8, u32>,
    /// The layers the hub has on (others' units are stale).
    on: HashSet<u8>,
}

impl LinkSource {
    /// Starts a feed on `frames` and attaches a source to it (one hub; use
    /// [`Self::attach`] to keep the feed across hubs). `display` is the
    /// display the source captures to start with.
    pub fn new<R: Read + Send + 'static>(frames: R, clock: LinkClock, display: u32, sizes: SizesFn, send: SendRequest) -> io::Result<LinkSource> {
        Ok(LinkSource::attach(&LinkFeed::spawn(frames)?, clock, display, sizes, send))
    }

    /// A source on `feed`. It replaces any source attached before (that one
    /// sees its source end) and starts with an empty queue.
    pub fn attach(feed: &LinkFeed, clock: LinkClock, display: u32, sizes: SizesFn, send: SendRequest) -> LinkSource {
        let shared = feed.shared.clone();
        let gen = {
            let mut q = lock(&shared.q);
            let gen = q.next_gen;
            q.next_gen += 1;
            q.attached = gen;
            q.units.clear();
            q.broken.clear();
            q.asks.clear();
            q.failed.clear();
            gen
        };
        shared.cv.notify_all();
        LinkSource { shared, gen, clock, sizes, send, display, layer_display: HashMap::new(), on: HashSet::new() }
    }

    /// The display the source shows now.
    pub fn display(&self) -> u32 {
        self.display
    }

    fn request(&mut self, r: &Request) {
        if let Err(e) = (self.send)(r) {
            crate::log_debug!("[stream] link request {r:?} failed: {e}");
        }
    }

    /// A unit as read → the hub's unit, or `None` for a stale one.
    fn relay(&mut self, raw: Raw) -> Option<EncodedUnit> {
        let h = raw.unit.header;
        if !self.on.contains(&h.layer) {
            return None;
        }
        match self.layer_display.get(&h.layer) {
            Some(d) if *d == h.source => {}
            // A new display (or the layer's first unit) starts with a key
            // + source start; anything else from it is stale.
            _ if !h.source_start => return None,
            _ => {
                self.layer_display.insert(h.layer, h.source);
                if h.source != self.display {
                    self.display = h.source;
                    self.shared.count(&self.shared.n.switches, 1);
                }
            }
        }
        let (width_pt, height_pt) = (self.sizes)(h.source).map_or((h.width, h.height), |d| d.points);
        let limit = raw.received_at;
        Some(EncodedUnit {
            codec: h.codec,
            key: h.key,
            source_start: h.source_start,
            layer: h.layer,
            width: h.width,
            height: h.height,
            width_pt,
            height_pt,
            seq: h.seq,
            captured_at: self.clock.to_instant(h.captured_ns, limit),
            stamps: FrameStamps { emitted_at: Some(self.clock.to_instant(h.emitted_ns, limit)), received_at: Some(raw.received_at) },
            pulled_at: Instant::now(),
            encode: None,
            data: Bytes::from(raw.unit.data),
        })
    }
}

impl EncodedSource for LinkSource {
    fn next_unit(&mut self, wait: Duration) -> Result<Option<EncodedUnit>, SourceError> {
        let deadline = Instant::now() + wait;
        loop {
            let (raw, asks) = {
                let mut q = lock(&self.shared.q);
                loop {
                    if q.attached != self.gen {
                        return Err(SourceError::Ended(LaneError::Unavailable { message: "another stream took this link".into() }));
                    }
                    // A failed layer goes to the hub first (one per call);
                    // a layer the hub no longer has on is forgotten.
                    while let Some((layer, why)) = q.failed.pop_front() {
                        if self.on.contains(&layer) {
                            self.on.remove(&layer);
                            self.layer_display.remove(&layer);
                            return Err(SourceError::LayerFailed { layer, why });
                        }
                    }
                    if !q.asks.is_empty() || !q.units.is_empty() {
                        break;
                    }
                    if let Some(why) = &q.ended {
                        return Err(SourceError::Ended(LaneError::Unavailable { message: why.clone() }));
                    }
                    let now = Instant::now();
                    if now >= deadline {
                        return Ok(None);
                    }
                    q = self.shared.cv.wait_timeout(q, deadline - now).unwrap_or_else(|e| e.into_inner()).0;
                }
                (q.units.pop_front(), std::mem::take(&mut q.asks))
            };
            for layer in asks {
                if self.on.contains(&layer) {
                    self.request(&Request::Keyframe { layer, since_ns: None });
                }
            }
            match raw.map(|r| self.relay(r)) {
                Some(Some(u)) => return Ok(Some(u)),
                Some(None) => self.shared.count(&self.shared.n.dropped, 1),
                None => {}
            }
        }
    }

    fn set_layers(&mut self, layers: &[LayerSpec]) {
        self.on = layers.iter().filter(|l| l.on).map(|l| l.id).collect();
        self.layer_display.retain(|id, _| layers.iter().any(|l| l.on && l.id == *id));
        self.request(&Request::Layers { layers: layers.to_vec() });
    }

    fn request_keyframe(&mut self, layer: u8, since: Option<Instant>) {
        let since_ns = since.map(|t| self.clock.to_ns(t));
        self.request(&Request::Keyframe { layer, since_ns });
    }

    fn sizes(&self) -> Option<SourceSizes> {
        (self.sizes)(self.display).map(|d| SourceSizes { points: d.points.0, native_px: d.native_px.map(|n| n.0) })
    }
}

impl Drop for LinkSource {
    fn drop(&mut self) {
        let mine = {
            let mut q = lock(&self.shared.q);
            let mine = q.attached == self.gen;
            if mine {
                q.attached = 0;
                q.units.clear();
                q.asks.clear();
                q.failed.clear();
            }
            mine
        };
        if mine {
            // No viewers: the source stops all capture work.
            self.request(&Request::Layers { layers: Vec::new() });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_simvideo::link::{write_unit, Codec, UnitHeader};
    use std::sync::mpsc;

    fn hdr(layer: u8, seq: u64, key: bool, source: u32) -> UnitHeader {
        UnitHeader { codec: Codec::Vp8, key, source_start: false, discardable: false, layer, source, width: 1280, height: 720, seq, captured_ns: 1_000 + seq, emitted_ns: 2_000 + seq }
    }

    fn sizes() -> SizesFn {
        Box::new(|d| match d {
            1 => Some(DisplaySize { points: (1440, 900), native_px: Some((2880, 1800)) }),
            2 => Some(DisplaySize { points: (1280, 720), native_px: None }),
            _ => None,
        })
    }

    fn sender() -> (SendRequest, mpsc::Receiver<Request>) {
        let (tx, rx) = mpsc::channel();
        (Box::new(move |r: &Request| tx.send(r.clone()).map_err(|_| io::Error::other("gone"))), rx)
    }

    fn layer(id: u8) -> LayerSpec {
        LayerSpec { id, codec: Codec::Vp8, max_width: 1440, fps: 15, bitrate: 4_000_000, quality: None, on: true }
    }

    fn wait_read(feed: &LinkFeed, n: u64) {
        let t0 = Instant::now();
        while feed.stats().read < n {
            assert!(t0.elapsed() < Duration::from_secs(5), "{:?}", feed.stats());
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn drain(src: &mut LinkSource) -> Vec<(u64, bool)> {
        let mut out = Vec::new();
        while let Some(u) = src.next_unit(Duration::from_millis(50)).expect("live") {
            out.push((u.seq, u.key));
        }
        out
    }

    #[test]
    fn the_clock_maps_both_ways_and_never_past_the_read() {
        let at = Instant::now();
        let c = LinkClock::new(1_000_000_000, at);
        let later = at + Duration::from_millis(50);
        assert_eq!(c.to_instant(1_010_000_000, later), at + Duration::from_millis(10));
        assert_eq!(c.to_instant(1_090_000_000, later), later, "never after the read");
        assert_eq!(c.to_ns(at + Duration::from_millis(5)), 1_005_000_000);
        assert_eq!(c.to_ns(at), 1_000_000_000);
        let h = HelloFields { version: 4, accepts: vec![4], codecs: vec![Codec::Vp8], encoder: link::Encoder::Software, clock_ns: 7 };
        assert_eq!(LinkClock::from_hello(&h, at), LinkClock::new(7, at));
    }

    /// Units relay with the display's points size and the source's times on
    /// the daemon's clock; the hub's asks go through the caller's sender;
    /// dropping the source pauses every layer.
    #[test]
    fn units_relay_with_sizes_and_times_and_asks_go_through_the_sender() {
        let (rx, mut tx) = std::io::pipe().expect("pipe");
        let (send, asked) = sender();
        let at = Instant::now();
        let mut src = LinkSource::new(rx, LinkClock::new(0, at), 1, sizes(), send).expect("source");
        assert_eq!(src.sizes(), Some(SourceSizes { points: 1440, native_px: Some(2880) }), "known before any unit");
        src.set_layers(&[layer(0)]);
        assert!(matches!(asked.recv().expect("layers"), Request::Layers { layers } if layers == vec![layer(0)]));
        write_unit(&mut tx, &UnitHeader { source_start: true, ..hdr(0, 1, true, 1) }, b"key").expect("write");
        write_unit(&mut tx, &hdr(0, 2, false, 1), b"delta").expect("write");
        let u = src.next_unit(Duration::from_secs(5)).expect("live").expect("a unit");
        assert!(u.key && u.source_start && u.codec == Codec::Vp8);
        assert_eq!((u.width, u.height, u.width_pt, u.height_pt), (1280, 720, 1440, 900));
        assert_eq!(&u.data[..], b"key");
        assert!(u.encode.is_none(), "the source encoded it");
        assert_eq!(u.captured_at, (at + Duration::from_nanos(1_001)).min(u.stamps.received_at.unwrap()));
        assert!(u.stamps.emitted_at.is_some());
        assert_eq!(src.next_unit(Duration::from_secs(5)).expect("live").expect("a unit").seq, 2);
        let since = at + Duration::from_millis(3);
        src.request_keyframe(0, Some(since));
        assert_eq!(asked.recv().expect("keyframe"), Request::Keyframe { layer: 0, since_ns: Some(3_000_000) });
        drop(src);
        assert_eq!(asked.recv().expect("paused"), Request::Layers { layers: vec![] });
    }

    /// (g) A full queue drops a layer to its next key unit, counts every
    /// unit it drops, and asks the source for a keyframe once.
    #[test]
    fn an_overflow_drops_to_the_next_key_and_counts_it() {
        let (rx, mut tx) = std::io::pipe().expect("pipe");
        let feed = LinkFeed::with_queue(rx, 4).expect("feed");
        let (send, asked) = sender();
        let mut src = LinkSource::attach(&feed, LinkClock::new(0, Instant::now()), 1, sizes(), send);
        src.set_layers(&[layer(0)]);
        let _ = asked.recv();
        // A key and nine deltas while the hub reads nothing: four fit.
        write_unit(&mut tx, &UnitHeader { source_start: true, ..hdr(0, 1, true, 1) }, b"k").expect("w");
        for seq in 2..=10 {
            write_unit(&mut tx, &hdr(0, seq, false, 1), b"d").expect("w");
        }
        wait_read(&feed, 10);
        assert_eq!(feed.stats().dropped, 6, "deltas 5..=10 dropped");
        assert_eq!(feed.stats().overflows, 1);
        assert_eq!(drain(&mut src), vec![(1, true), (2, false), (3, false), (4, false)], "the chain up to the overflow");
        assert_eq!(asked.try_recv().expect("one ask"), Request::Keyframe { layer: 0, since_ns: None });
        assert!(asked.try_recv().is_err(), "only one");
        // Deltas after the overflow still drop until the key.
        write_unit(&mut tx, &hdr(0, 11, false, 1), b"d").expect("w");
        write_unit(&mut tx, &hdr(0, 12, true, 1), b"k").expect("w");
        write_unit(&mut tx, &hdr(0, 13, false, 1), b"d").expect("w");
        wait_read(&feed, 13);
        assert_eq!(drain(&mut src), vec![(12, true), (13, false)]);
        assert_eq!(feed.stats().dropped, 7);
        // A key that finds the queue full replaces its layer's queued units.
        for seq in 14..=17 {
            write_unit(&mut tx, &hdr(0, seq, false, 1), b"d").expect("w");
        }
        write_unit(&mut tx, &hdr(0, 18, true, 1), b"k").expect("w");
        wait_read(&feed, 18);
        assert_eq!(drain(&mut src), vec![(18, true)]);
        assert_eq!(feed.stats().dropped, 11);
        assert_eq!(feed.stats().overflows, 2);
        drop(tx);
        let t0 = Instant::now();
        loop {
            match src.next_unit(Duration::from_millis(50)) {
                Err(SourceError::Ended(e)) => {
                    assert!(e.message().contains("closed"), "{e:?}");
                    break;
                }
                other => assert!(t0.elapsed() < Duration::from_secs(5), "{other:?}"),
            }
        }
    }

    /// A display switch: the new display's first unit is a key + source
    /// start; it becomes the current display (sizes follow) and is counted.
    /// An old-display delta after it, or a new-display unit without the
    /// start, is stale.
    #[test]
    fn a_source_change_is_a_display_switch_on_a_source_start_key() {
        let (rx, mut tx) = std::io::pipe().expect("pipe");
        let feed = LinkFeed::spawn(rx).expect("feed");
        let (send, _asked) = sender();
        let mut src = LinkSource::attach(&feed, LinkClock::new(0, Instant::now()), 1, sizes(), send);
        src.set_layers(&[layer(0)]);
        write_unit(&mut tx, &UnitHeader { source_start: true, ..hdr(0, 1, true, 1) }, b"k").expect("w");
        write_unit(&mut tx, &hdr(0, 2, false, 1), b"d").expect("w");
        write_unit(&mut tx, &hdr(0, 3, true, 2), b"k").expect("w"); // no source start: stale
        write_unit(&mut tx, &UnitHeader { source_start: true, ..hdr(0, 4, true, 2) }, b"k").expect("w");
        write_unit(&mut tx, &hdr(0, 5, false, 1), b"d").expect("w"); // the old display: stale
        write_unit(&mut tx, &hdr(0, 6, false, 2), b"d").expect("w");
        wait_read(&feed, 6);
        let mut got = Vec::new();
        while let Some(u) = src.next_unit(Duration::from_millis(50)).expect("live") {
            got.push((u.seq, u.source_start, u.width_pt));
        }
        assert_eq!(got, vec![(1, true, 1440), (2, false, 1440), (4, true, 1280), (6, false, 1280)]);
        assert_eq!(src.display(), 2);
        assert_eq!(src.sizes(), Some(SourceSizes { points: 1280, native_px: None }));
        assert_eq!(feed.stats().switches, 1);
        assert_eq!(feed.stats().dropped, 2);
    }

    /// D6: deskd's `layer_failed` reaches the hub as `LayerFailed` ahead of
    /// queued units, the layer's queued units go, its later units are stale,
    /// and other layers keep flowing. A failure for a layer that isn't on,
    /// or with nothing attached, is ignored.
    #[test]
    fn a_failed_layer_reaches_the_hub_first_and_others_keep_flowing() {
        let (rx, mut tx) = std::io::pipe().expect("pipe");
        let feed = LinkFeed::spawn(rx).expect("feed");
        feed.fail_layer(0, "nothing attached");
        let (send, _asked) = sender();
        let mut src = LinkSource::attach(&feed, LinkClock::new(0, Instant::now()), 1, sizes(), send);
        src.set_layers(&[layer(0), layer(1)]);
        write_unit(&mut tx, &UnitHeader { source_start: true, ..hdr(0, 1, true, 1) }, b"k0").expect("w");
        write_unit(&mut tx, &UnitHeader { source_start: true, ..hdr(1, 1, true, 1) }, b"k1").expect("w");
        wait_read(&feed, 2);
        feed.fail_layer(7, "never on");
        feed.fail_layer(1, "no JPEG encoder on this Mac");
        match src.next_unit(Duration::from_secs(5)) {
            Err(SourceError::LayerFailed { layer, why }) => assert_eq!((layer, why.as_str()), (1, "no JPEG encoder on this Mac")),
            other => panic!("want LayerFailed first, got {other:?}"),
        }
        write_unit(&mut tx, &hdr(1, 2, false, 1), b"d1").expect("w"); // the failed layer: stale
        write_unit(&mut tx, &hdr(0, 2, false, 1), b"d0").expect("w");
        wait_read(&feed, 4);
        let mut got = Vec::new();
        while let Some(u) = src.next_unit(Duration::from_millis(50)).expect("live") {
            got.push((u.layer, u.seq));
        }
        assert_eq!(got, vec![(0, 1), (0, 2)], "layer 0 flows; layer 1's queued key and later delta are gone");
        assert_eq!(feed.stats().dropped, 2);
    }

    /// A new hub's source on the same feed takes over: the old one ends,
    /// and units read while none was attached are counted as idle.
    #[test]
    fn a_feed_outlives_its_sources_and_one_is_attached_at_a_time() {
        let (rx, mut tx) = std::io::pipe().expect("pipe");
        let feed = LinkFeed::spawn(rx).expect("feed");
        write_unit(&mut tx, &hdr(0, 1, true, 1), b"k").expect("w");
        wait_read(&feed, 1);
        assert_eq!(feed.stats().idle, 1);
        let (send, _a) = sender();
        let mut first = LinkSource::attach(&feed, LinkClock::new(0, Instant::now()), 1, sizes(), send);
        let (send, _b) = sender();
        let mut second = LinkSource::attach(&feed, LinkClock::new(0, Instant::now()), 1, sizes(), send);
        assert!(matches!(first.next_unit(Duration::from_millis(10)), Err(SourceError::Ended(_))));
        second.set_layers(&[layer(0)]);
        write_unit(&mut tx, &UnitHeader { source_start: true, ..hdr(0, 2, true, 1) }, b"k").expect("w");
        assert_eq!(second.next_unit(Duration::from_secs(5)).expect("live").map(|u| u.seq), Some(2));
        drop(first); // not attached: changes nothing
        assert!(second.next_unit(Duration::from_millis(20)).expect("still live").is_none());
    }
}
