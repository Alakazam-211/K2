//! The device stream engine, hub side (fix 10, S2; PRD remote desktop §3.4
//! E2–E4, E6). Source-neutral: the sim route (`sim_stream_ws`) feeds it a
//! [`k2_core::stream::pixel::PixelLayers`]; the desktop route (D6) will feed
//! it k2-deskd's protocol-4 units ([`k2_core::stream::link_source`]).
//! [`viewer::serve`] is the viewer loop both routes run (S5).
//!
//! **One hub per [`SourceKey`], one encode per layer, never per viewer.** A
//! hub's capture thread owns the [`EncodedSource`], pulls `next_unit` and
//! hands each unit to the viewers subscribed to its layer (a bounded queue
//! per viewer; a viewer that can't keep up loses units there and its
//! [`ViewerGate`] waits for the next key unit).
//!
//! **Layers** are keyed by codec, fps tier and remote-ness ([`LayerKey`]).
//! A viewer's clamped fps rounds up to a tier ([`TIERS`]; never past its
//! rate table cap). At most [`MAX_LAYERS`] per source: past that a viewer
//! joins the nearest higher-fps layer of its codec and remote-ness (else
//! the nearest lower). Only a viewer whose codec and remote-ness have no
//! layer at all opens one past the cap, so it never gets a codec it didn't
//! negotiate or a local bitrate on a remote link. Every layer gets an
//! explicit width ([`EncodedSource::sizes`]): the screen's points width
//! until a viewer on it sends `pw`, then the widest `pw` clamped to the
//! native width; a layer waits out of the list until the source knows its
//! sizes (its first frame). Its
//! bitrate is [`bitrate_for`] of its last unit's size, tier and
//! remote-ness, stepped by a per-layer [`RateCtl`] that follows the layer's
//! most congested viewer. A viewer congested for [`MOVE_DOWN_AFTER`] while
//! another on its layer isn't moves to the next layer down (the next lower
//! tier of its codec and remote-ness, or the nearest lower layer when no
//! more fit; [`HubEvent::Moved`]); a lone viewer stays (the rate is its
//! own). The hub sends the whole list whenever it changes;
//! a layer with no viewers leaves the list (paused). Ids are small u8s,
//! stable while the layer lives and not reused at once.
//!
//! **Keyframes.** A viewer joining a running layer gets
//! `request_keyframe(layer, now)` at once, never throttled, and nothing on
//! that layer until a key unit. A viewer's own ask is throttled per layer
//! ([`KEYFRAME_MIN_GAP`]); a catching-up viewer's ([`Subscription::catch_up`])
//! to one per [`CATCH_UP_GAP`] per layer. A throttled catch-up isn't lost:
//! while a viewer on the layer still waits for a key unit and none has
//! reached it, the hub asks again as soon as the gap ends (S4), so a viewer
//! that skips twice inside a second waits at most the gap and a frame.
//! What makes a viewer skip is [`ViewerGate`]'s (R8, S4).
//!
//! **Codecs.** H.264, VP8 and JPEG layers are alike to the hub. A pixel
//! source never encodes VP8: a VP8 layer on one fails at once and its
//! viewers move to JPEG, as on any encoder failure (routes negotiate VP8
//! only with a source that gives it, `negotiate_with`).
//!
//! **Failure.** A layer whose encoder fails moves its viewers to a JPEG
//! layer ([`HubEvent::Moved`]); a source that ends tells every viewer
//! ([`HubEvent::Ended`]) and the hub goes away.

mod gate;
pub mod rates;
pub mod viewer;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use k2_core::log_debug;
use k2_core::sim::stream::ViewerPath;
use k2_core::sim::video::bitrate_for;
use k2_core::sim::video::flow::RateCtl;
use k2_core::stream::{thread_cpu_ns, Codec, EncodedSource, EncodedUnit, LayerSpec, SourceError, SourceKey, SourceSizes};
use tokio::sync::mpsc;

pub use gate::{skip_age, Took, ViewerFlow, ViewerGate, SKIP_AGE_MIN, SKIP_AGE_RTTS, SKIP_SEND};
pub use viewer::{serve, Auth, Coalesce, DesktopRule, EngineMsg, Hooks, InputSink, Negotiated, Offer, PointerWheel, Push, Refusal, ServeError, Source, StreamMsg, Verdict, Wheel, AUTH_TICK};

/// Layer rates. A viewer's fps rounds up to the nearest.
pub const TIERS: [u32; 4] = [60, 30, 15, 8];
/// The most layers a source runs (one encode each).
pub const MAX_LAYERS: usize = 3;
/// A viewer's keyframe asks closer together than this are merged (per layer).
pub const KEYFRAME_MIN_GAP: Duration = Duration::from_millis(250);
/// Catch-up asks (a viewer that dropped deltas): one per layer per this.
pub const CATCH_UP_GAP: Duration = Duration::from_secs(1);
/// A viewer congested this long while another on its layer isn't moves to
/// the next layer down (PRD remote desktop §3.5, rate control per layer).
pub const MOVE_DOWN_AFTER: Duration = Duration::from_secs(5);
/// Units a viewer may have queued before the hub drops its newer ones.
pub const VIEWER_QUEUE: usize = 8;
/// How long the capture thread waits in `next_unit` before it looks at the
/// layer list and keyframe asks again.
const PULL_WAIT: Duration = Duration::from_millis(25);
/// A hub with no viewers ends after this (the first viewer's grace).
const IDLE_GRACE: Duration = Duration::from_secs(5);
/// A layer's assumed size before its first unit (today's default).
const GUESS_SIZE: (u32, u32) = (400, 900);

/// The tier a viewer at `fps` gets: the smallest tier at or above it, never
/// above its rate table `cap`.
pub fn tier_for(fps: u32, cap: u32) -> u32 {
    TIERS.iter().rev().copied().find(|t| *t >= fps).unwrap_or(TIERS[0]).min(cap.max(1))
}

/// What makes two viewers share a layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LayerKey {
    pub codec: Codec,
    /// The tier.
    pub fps: u32,
    pub remote: bool,
}

/// A video layer's target bitrate: [`bitrate_for`] of its size, tier and
/// remote-ness. A VP8 layer (k2-deskd on Linux, PRD §3.5) is capped at the
/// remote 4 Mb/s on every path: one software encode on one core.
pub fn layer_bitrate(key: &LayerKey, width: u32, height: u32) -> u32 {
    bitrate_for(width, height, key.fps, key.remote || key.codec == Codec::Vp8)
}

/// What a viewer wants from a hub.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Want {
    pub codec: Codec,
    /// Its fps after the rate table (`rates::clamp_fps`).
    pub fps: u32,
    pub path: ViewerPath,
    /// Its pane width in device pixels (`pw=`), if it said.
    pub pw: Option<u32>,
    /// It asked for `stats=1` (the hub then samples its CPU time).
    pub stats: bool,
}

/// What a hub tells one viewer besides units.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HubEvent {
    /// It is on `layer` now, at `fps`: its layer's encoder failed (a JPEG
    /// layer, the JPEG row of the rate table), or its link stayed congested
    /// while its layer's others didn't (the same codec, the next rate down).
    Moved { layer: u8, codec: Codec, fps: u32, why: String },
    /// The source ended (the device stopped).
    Ended(String),
}

struct Layer {
    id: u8,
    key: LayerKey,
    /// The width the source gets (`None` until it knows its sizes).
    max_width: Option<u32>,
    /// The last unit's size (the bitrate's).
    size: Option<(u32, u32)>,
    rate: RateCtl,
    last_key_ask: Option<Instant>,
    last_catch_up: Option<Instant>,
}

impl Layer {
    fn spec(&self) -> LayerSpec {
        let bitrate = if self.key.codec.is_video() { self.rate.current() } else { 0 };
        LayerSpec { id: self.id, codec: self.key.codec, max_width: self.max_width.unwrap_or(0), fps: self.key.fps, bitrate, quality: None, on: self.max_width.is_some() }
    }
}

struct Sub {
    layer: u8,
    want: Want,
    units: mpsc::Sender<EncodedUnit>,
    events: mpsc::UnboundedSender<HubEvent>,
    congested: bool,
    /// Since when it has been congested on this layer.
    congested_since: Option<Instant>,
    /// It asked for a catch-up key unit and none has reached its queue.
    waiting: bool,
}

impl Sub {
    fn new(layer: u8, want: Want, units: mpsc::Sender<EncodedUnit>, events: mpsc::UnboundedSender<HubEvent>) -> Self {
        Sub { layer, want, units, events, congested: false, congested_since: None, waiting: false }
    }

    /// On a new layer: its link starts clear there, and the hub asked for
    /// its key as a joiner's.
    fn fresh(&mut self) {
        self.congested = false;
        self.congested_since = None;
        self.waiting = false;
    }
}

/// The hub's clock (a fake one in tests).
type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;
/// Keyframe asks for the source: (layer, since).
type Asks = Vec<(u8, Option<Instant>)>;

#[derive(Default)]
struct State {
    layers: Vec<Layer>,
    viewers: HashMap<u64, Sub>,
    next_id: u8,
    /// The layer list changed since the source last got it.
    dirty: bool,
    /// Keyframe asks for the source.
    asks: Asks,
    /// The list the source has now.
    sent: Vec<LayerSpec>,
    sizes: Option<SourceSizes>,
    ended: Option<String>,
}

/// One source's hub.
pub struct Hub {
    key: SourceKey,
    gen: u64,
    state: Mutex<State>,
    stats_viewers: AtomicUsize,
    cpu_ns: AtomicU64,
    units: AtomicU64,
    key_asks: AtomicU64,
    clock: Clock,
}

fn hubs() -> &'static Mutex<HashMap<SourceKey, Arc<Hub>>> {
    static H: OnceLock<Mutex<HashMap<SourceKey, Arc<Hub>>>> = OnceLock::new();
    H.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The live hub for `key`, if any.
pub fn hub_for(key: &SourceKey) -> Option<Arc<Hub>> {
    lock(hubs()).get(key).filter(|h| lock(&h.state).ended.is_none()).cloned()
}

/// Why [`join`] failed.
#[derive(Debug)]
pub enum JoinError<E> {
    /// The source couldn't open.
    Open(E),
    /// The capture thread couldn't start.
    Thread(String),
}

/// Subscribe to `key`'s hub, starting it (with the source `open` returns)
/// if none runs. The subscription is made before this returns, so a hub
/// can't end between the two.
pub fn join<E>(key: SourceKey, want: Want, open: impl FnOnce() -> Result<Box<dyn EncodedSource>, E>) -> Result<Subscription, JoinError<E>> {
    let mut map = lock(hubs());
    if let Some(h) = map.get(&key) {
        if lock(&h.state).ended.is_none() {
            return Ok(Hub::subscribe(h, want));
        }
    }
    let source = open().map_err(JoinError::Open)?;
    static GEN: AtomicU64 = AtomicU64::new(1);
    let hub = Arc::new(Hub::new(key.clone(), GEN.fetch_add(1, Ordering::SeqCst), Arc::new(Instant::now)));
    let sub = Hub::subscribe(&hub, want);
    let h2 = hub.clone();
    std::thread::Builder::new().name(format!("k2-stream-{key}")).spawn(move || capture(h2, source)).map_err(|e| JoinError::Thread(format!("capture thread: {e}")))?;
    map.insert(key, hub);
    Ok(sub)
}

impl Hub {
    fn new(key: SourceKey, gen: u64, clock: Clock) -> Hub {
        Hub {
            key,
            gen,
            state: Mutex::new(State::default()),
            stats_viewers: AtomicUsize::new(0),
            cpu_ns: AtomicU64::new(0),
            units: AtomicU64::new(0),
            key_asks: AtomicU64::new(0),
            clock,
        }
    }

    fn now(&self) -> Instant {
        (self.clock)()
    }

    pub fn key(&self) -> &SourceKey {
        &self.key
    }

    /// The layer list the source has now.
    pub fn layer_specs(&self) -> Vec<LayerSpec> {
        lock(&self.state).sent.clone()
    }

    /// Units the source has given (one encode each for a pixel source).
    pub fn units(&self) -> u64 {
        self.units.load(Ordering::SeqCst)
    }

    /// Keyframe asks passed to the source (joiners, viewers, catch-ups).
    pub fn key_asks(&self) -> u64 {
        self.key_asks.load(Ordering::SeqCst)
    }

    /// The capture thread's CPU time (ns), sampled while a stats viewer
    /// watches. It includes a pixel source's encodes.
    pub fn cpu_ns(&self) -> u64 {
        self.cpu_ns.load(Ordering::Relaxed)
    }

    fn subscribe(hub: &Arc<Hub>, want: Want) -> Subscription {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::SeqCst);
        let (units_tx, units) = mpsc::channel(VIEWER_QUEUE);
        let (events_tx, events) = mpsc::unbounded_channel();
        let mut st = lock(&hub.state);
        let layer = hub.attach(&mut st, id, want);
        st.viewers.insert(id, Sub::new(layer, want, units_tx, events_tx));
        hub.refresh_width(&mut st, layer);
        drop(st);
        if want.stats {
            hub.stats_viewers.fetch_add(1, Ordering::SeqCst);
        }
        Subscription { hub: hub.clone(), id, units, events, stats: want.stats }
    }

    /// The layer for a viewer (made if need be); a running layer it joins
    /// gets an unthrottled keyframe ask. The caller adds the viewer.
    fn attach(&self, st: &mut State, me: u64, want: Want) -> u8 {
        let kind = self.key.kind();
        let remote = want.path.remote();
        let key = LayerKey { codec: want.codec, fps: tier_for(want.fps, rates::cap(kind, want.codec, want.path)), remote };
        let same = |l: &&Layer| l.key.codec == key.codec && l.key.remote == remote;
        let found = st.layers.iter().find(|l| l.key == key).map(|l| l.id).or_else(|| {
            if st.layers.len() < MAX_LAYERS {
                return None;
            }
            let higher = st.layers.iter().filter(same).filter(|l| l.key.fps >= key.fps).min_by_key(|l| l.key.fps);
            higher.or_else(|| st.layers.iter().filter(same).max_by_key(|l| l.key.fps)).map(|l| l.id)
        });
        if let Some(id) = found {
            let others = st.viewers.iter().any(|(v, s)| *v != me && s.layer == id);
            if others {
                let now = self.now();
                if let Some(l) = st.layers.iter_mut().find(|l| l.id == id) {
                    l.last_key_ask = Some(now);
                }
                st.asks.push((id, Some(now)));
                self.key_asks.fetch_add(1, Ordering::SeqCst);
            }
            return id;
        }
        // Round robin, so a layer that just went away doesn't come back
        // under its old id with a new source's units; 255 is "none".
        let mut id = st.next_id;
        while id == u8::MAX || st.layers.iter().any(|l| l.id == id) {
            id = id.wrapping_add(1);
        }
        st.next_id = id.wrapping_add(1);
        let (w, h) = GUESS_SIZE;
        let rate = RateCtl::new(if key.codec.is_video() { layer_bitrate(&key, w, h) } else { 0 });
        st.layers.push(Layer { id, key, max_width: None, size: None, rate, last_key_ask: None, last_catch_up: None });
        st.dirty = true;
        id
    }

    /// A viewer left `layer`: drop the layer when nobody is on it.
    fn detach(&self, st: &mut State, layer: u8) {
        if st.viewers.values().any(|s| s.layer == layer) {
            self.refresh_width(st, layer);
        } else {
            st.layers.retain(|l| l.id != layer);
            st.dirty = true;
        }
    }

    /// A layer's width: the widest `pw` of its viewers clamped to the
    /// native width (as asked while the native width is unknown: a source
    /// never scales up), else the points width; unknown until the source
    /// knows its sizes.
    fn refresh_width(&self, st: &mut State, layer: u8) {
        let pw = st.viewers.values().filter(|s| s.layer == layer).filter_map(|s| s.want.pw).max();
        let w = st.sizes.map(|z| match pw {
            Some(p) => z.native_px.map_or(p, |n| p.min(n)).max(1),
            None => z.points.max(1),
        });
        if let Some(l) = st.layers.iter_mut().find(|l| l.id == layer) {
            if l.max_width != w {
                l.max_width = w;
                st.dirty = true;
            }
        }
    }

    fn leave(&self, me: u64) {
        let mut st = lock(&self.state);
        if let Some(s) = st.viewers.remove(&me) {
            self.detach(&mut st, s.layer);
        }
    }

    /// A unit off the source: to every viewer of its layer that has room
    /// in its queue (the rest lose it and catch up on a key unit). A key
    /// unit in a waiting viewer's queue ends its wait.
    fn fan_out(&self, u: EncodedUnit) {
        self.units.fetch_add(1, Ordering::SeqCst);
        let mut st = lock(&self.state);
        let mut retuned = false;
        if let Some(l) = st.layers.iter_mut().find(|l| l.id == u.layer) {
            if u.codec.is_video() && l.size != Some((u.width, u.height)) {
                l.size = Some((u.width, u.height));
                let target = layer_bitrate(&l.key, u.width, u.height);
                if target != l.rate.target() {
                    l.rate.retarget(target);
                    retuned = true;
                }
            }
        }
        st.dirty |= retuned;
        for s in st.viewers.values_mut().filter(|s| s.layer == u.layer) {
            if s.units.try_send(u.clone()).is_ok() && u.key {
                s.waiting = false;
            }
        }
    }

    /// One turn of the capture loop's bookkeeping at `now`: moves down,
    /// rates, catch-up asks whose gap ended; then the layer list if it
    /// changed (the layers whose width is known) and the keyframe asks for
    /// the source.
    fn step(&self, now: Instant) -> (Option<Vec<LayerSpec>>, Asks) {
        let mut st = lock(&self.state);
        self.move_down(&mut st, now);
        self.rate_tick(&mut st, now);
        self.due_catch_ups(&mut st, now);
        // A layer whose width isn't known yet stays out (paused).
        let layers = std::mem::take(&mut st.dirty).then(|| st.layers.iter().filter(|l| l.max_width.is_some()).map(Layer::spec).collect::<Vec<_>>());
        if let Some(l) = &layers {
            st.sent = l.clone();
        }
        (layers, std::mem::take(&mut st.asks))
    }

    /// A catch-up ask that was throttled is asked once the layer's gap
    /// ends, while one of its viewers still waits for a key unit.
    fn due_catch_ups(&self, st: &mut State, now: Instant) {
        let State { layers, viewers, asks, .. } = st;
        for l in layers.iter_mut() {
            let due = l.last_catch_up.is_none_or(|t| now.saturating_duration_since(t) >= CATCH_UP_GAP);
            if due && viewers.values().any(|s| s.layer == l.id && s.waiting) {
                l.last_catch_up = Some(now);
                asks.push((l.id, None));
                self.key_asks.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    /// The rate a viewer leaving the layer `from` for being congested goes
    /// to: the next tier down when that layer runs or one more fits, else
    /// the nearest lower layer of its codec and remote-ness; none from the
    /// lowest.
    fn lower_fps(st: &State, from: &LayerKey) -> Option<u32> {
        let next = TIERS.iter().copied().find(|t| *t < from.fps)?;
        let same = |l: &&Layer| l.key.codec == from.codec && l.key.remote == from.remote;
        if st.layers.len() < MAX_LAYERS || st.layers.iter().filter(same).any(|l| l.key.fps == next) {
            return Some(next);
        }
        st.layers.iter().filter(same).map(|l| l.key.fps).filter(|f| *f < from.fps).max()
    }

    /// Each video viewer congested for [`MOVE_DOWN_AFTER`] while another on
    /// its layer isn't goes to the next layer down. A lone viewer, or a
    /// layer whose viewers are all congested, is the rate control's.
    fn move_down(&self, st: &mut State, now: Instant) {
        let long = |s: &Sub| s.want.codec.is_video() && s.congested_since.is_some_and(|t| now.saturating_duration_since(t) >= MOVE_DOWN_AFTER);
        let clear_beside = |me: u64, layer: u8| st.viewers.iter().any(|(o, x)| *o != me && x.layer == layer && !x.congested);
        let movers: Vec<u64> = st.viewers.iter().filter(|(id, s)| long(s) && clear_beside(**id, s.layer)).map(|(id, _)| *id).collect();
        for id in movers {
            let Some((from, old)) = st.viewers.get(&id).map(|s| (s.layer, s.want)) else { continue };
            let Some(key) = st.layers.iter().find(|l| l.id == from).map(|l| l.key) else { continue };
            let Some(fps) = Self::lower_fps(st, &key) else { continue };
            let want = Want { fps, ..old };
            if let Some(s) = st.viewers.get_mut(&id) {
                s.layer = u8::MAX;
            }
            self.detach(st, from);
            let to = self.attach(st, id, want);
            if let Some(s) = st.viewers.get_mut(&id) {
                s.layer = to;
                s.want = want;
                s.fresh();
                let why = format!("its link stayed congested for {} s while its layer's others didn't", MOVE_DOWN_AFTER.as_secs());
                let _ = s.events.send(HubEvent::Moved { layer: to, codec: want.codec, fps, why });
            }
            self.refresh_width(st, to);
        }
    }

    /// Each video layer's bitrate follows its most congested viewer.
    fn rate_tick(&self, st: &mut State, now: Instant) {
        let State { layers, viewers, dirty, .. } = st;
        for l in layers.iter_mut().filter(|l| l.key.codec.is_video()) {
            let congested = viewers.values().any(|s| s.layer == l.id && s.congested);
            if l.rate.update(congested, now).is_some() {
                *dirty = true;
            }
        }
    }

    /// `layer`'s encoder failed: its viewers go to JPEG. A JPEG layer that
    /// fails has nowhere to go (a Mac deskd sends no JPEG): its viewers end
    /// with the reason, rather than join a new JPEG layer that fails again.
    fn layer_failed(&self, layer: u8, why: &str) {
        let mut st = lock(&self.state);
        let kind = self.key.kind();
        let movers: Vec<u64> = st.viewers.iter().filter(|(_, s)| s.layer == layer).map(|(id, _)| *id).collect();
        if st.layers.iter().any(|l| l.id == layer && l.key.codec == Codec::Jpeg) {
            for id in movers {
                if let Some(s) = st.viewers.remove(&id) {
                    let _ = s.events.send(HubEvent::Ended(format!("this source can't send JPEG: {why}")));
                }
            }
            st.layers.retain(|l| l.id != layer);
            st.dirty = true;
            return;
        }
        for id in movers {
            let Some(old) = st.viewers.get(&id).map(|s| s.want) else { continue };
            let want = Want { codec: Codec::Jpeg, fps: rates::clamp_fps(kind, Codec::Jpeg, old.path, Some(old.fps)), ..old };
            // Off the failed layer first, so the JPEG layer's joiner ask
            // only counts viewers already there.
            if let Some(s) = st.viewers.get_mut(&id) {
                s.layer = u8::MAX;
            }
            let to = self.attach(&mut st, id, want);
            if let Some(s) = st.viewers.get_mut(&id) {
                s.layer = to;
                s.want = want;
                s.fresh();
                let _ = s.events.send(HubEvent::Moved { layer: to, codec: Codec::Jpeg, fps: want.fps, why: why.to_string() });
            }
            self.refresh_width(&mut st, to);
        }
        st.layers.retain(|l| l.id != layer);
        st.dirty = true;
    }

    fn end(&self, why: &str) {
        let mut st = lock(&self.state);
        st.ended = Some(why.to_string());
        for s in st.viewers.values() {
            let _ = s.events.send(HubEvent::Ended(why.to_string()));
        }
    }
}

/// The capture loop: apply the layer list and keyframe asks, pull a unit,
/// fan it out. It ends when the source does, or when nobody has watched for
/// [`IDLE_GRACE`] after it started.
fn capture(hub: Arc<Hub>, mut source: Box<dyn EncodedSource>) {
    let born = Instant::now();
    loop {
        {
            let mut map = lock(hubs());
            let mut st = lock(&hub.state);
            if st.viewers.is_empty() && born.elapsed() > IDLE_GRACE {
                st.ended = Some("no viewers".into());
                if map.get(&hub.key).is_some_and(|h| h.gen == hub.gen) {
                    map.remove(&hub.key);
                }
                return;
            }
        }
        let (layers, asks) = hub.step(hub.now());
        if let Some(l) = layers {
            source.set_layers(&l);
        }
        for (layer, since) in asks {
            source.request_keyframe(layer, since);
        }
        match source.next_unit(PULL_WAIT) {
            Ok(Some(u)) => {
                hub.fan_out(u);
                if hub.stats_viewers.load(Ordering::Relaxed) > 0 {
                    hub.cpu_ns.store(thread_cpu_ns(), Ordering::Relaxed);
                }
            }
            Ok(None) => {} // a still screen costs nothing
            Err(SourceError::LayerFailed { layer, why }) => {
                log_debug!("[stream] layer {layer} on {} failed ({why}); its viewers go to JPEG", hub.key);
                hub.layer_failed(layer, &why);
            }
            Err(SourceError::Ended(e)) => {
                log_debug!("[stream] source {} ended: {}", hub.key, e.message());
                hub.end(e.message());
                break;
            }
        }
        // A rotation or a first native frame changes the widths.
        let sizes = source.sizes();
        let mut st = lock(&hub.state);
        if sizes.is_some() && sizes != st.sizes {
            st.sizes = sizes;
            let ids: Vec<u8> = st.layers.iter().map(|l| l.id).collect();
            for id in ids {
                hub.refresh_width(&mut st, id);
            }
        }
    }
    let mut map = lock(hubs());
    if map.get(&hub.key).is_some_and(|h| h.gen == hub.gen) {
        map.remove(&hub.key);
    }
}

/// One viewer's place on a hub. Dropping it leaves (its layer pauses when
/// it was the last viewer).
pub struct Subscription {
    hub: Arc<Hub>,
    id: u64,
    /// Its layer's units, in order (a full queue loses the newer ones).
    pub units: mpsc::Receiver<EncodedUnit>,
    pub events: mpsc::UnboundedReceiver<HubEvent>,
    stats: bool,
}

impl Subscription {
    pub fn hub(&self) -> &Arc<Hub> {
        &self.hub
    }

    /// Its layer now.
    pub fn layer(&self) -> u8 {
        lock(&self.hub.state).viewers.get(&self.id).map_or(u8::MAX, |s| s.layer)
    }

    /// Its layer's spec as the hub has it now (`on: false` until the
    /// source knows its sizes).
    pub fn layer_spec(&self) -> Option<LayerSpec> {
        let st = lock(&self.hub.state);
        let id = st.viewers.get(&self.id)?.layer;
        st.layers.iter().find(|l| l.id == id).map(Layer::spec)
    }

    /// The viewer's own keyframe ask (`{"t":"keyframe"}`): merged with any
    /// other within [`KEYFRAME_MIN_GAP`] on the layer. True when passed on.
    pub fn ask_keyframe(&self) -> bool {
        self.ask(false)
    }

    /// A viewer that skipped asks for a key unit: one per layer per
    /// [`CATCH_UP_GAP`]. True when passed on now. A throttled ask is kept:
    /// the hub asks when the gap ends if this viewer (or another on the
    /// layer) still waits and no key unit reached it.
    pub fn catch_up(&self) -> bool {
        self.ask(true)
    }

    fn ask(&self, catch_up: bool) -> bool {
        let mut st = lock(&self.hub.state);
        let Some(me) = st.viewers.get_mut(&self.id) else { return false };
        me.waiting |= catch_up;
        let id = me.layer;
        let now = self.hub.now();
        let Some(l) = st.layers.iter_mut().find(|l| l.id == id) else { return false };
        let (last, gap) = if catch_up { (&mut l.last_catch_up, CATCH_UP_GAP) } else { (&mut l.last_key_ask, KEYFRAME_MIN_GAP) };
        if last.is_some_and(|t| now.saturating_duration_since(t) < gap) {
            return false;
        }
        *last = Some(now);
        st.asks.push((id, None));
        self.hub.key_asks.fetch_add(1, Ordering::SeqCst);
        true
    }

    /// A new fps (already clamped): the viewer may move to another layer,
    /// where it waits for a key unit. Returns its layer.
    pub fn set_fps(&mut self, fps: u32) -> u8 {
        let mut st = lock(&self.hub.state);
        let Some(old) = st.viewers.get(&self.id).map(|s| (s.layer, s.want)) else { return u8::MAX };
        let want = Want { fps, ..old.1 };
        let kind = self.hub.key.kind();
        let same_tier = st.layers.iter().find(|l| l.id == old.0).is_some_and(|l| l.key.fps == tier_for(fps, rates::cap(kind, want.codec, want.path)));
        if same_tier {
            if let Some(s) = st.viewers.get_mut(&self.id) {
                s.want = want;
            }
            return old.0;
        }
        if let Some(s) = st.viewers.get_mut(&self.id) {
            s.layer = u8::MAX;
        }
        self.hub.detach(&mut st, old.0);
        let to = self.hub.attach(&mut st, self.id, want);
        if let Some(s) = st.viewers.get_mut(&self.id) {
            s.layer = to;
            s.want = want;
            // Back on its own layer (no other fits): nothing new to wait for.
            if to != old.0 {
                s.fresh();
            }
        }
        self.hub.refresh_width(&mut st, to);
        to
    }

    /// The viewer's link state (its ack window's `congested`).
    pub fn set_congested(&self, congested: bool) {
        let now = self.hub.now();
        if let Some(s) = lock(&self.hub.state).viewers.get_mut(&self.id) {
            if congested && !s.congested {
                s.congested_since = Some(now);
            } else if !congested {
                s.congested_since = None;
            }
            s.congested = congested;
        }
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.hub.leave(self.id);
        if self.stats {
            self.hub.stats_viewers.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

#[cfg(test)]
#[path = "hub_tests.rs"]
mod hub_tests;

#[cfg(test)]
mod tests {
    use super::*;

    /// A source that fails every layer of one codec as soon as the hub turns
    /// it on, and records each layer list.
    struct FailsCodec {
        fail: Codec,
        failed: Vec<u8>,
        pending: Vec<u8>,
        seen: Arc<Mutex<Vec<Vec<LayerSpec>>>>,
    }

    impl EncodedSource for FailsCodec {
        fn next_unit(&mut self, wait: Duration) -> Result<Option<EncodedUnit>, SourceError> {
            if let Some(layer) = self.pending.pop() {
                return Err(SourceError::LayerFailed { layer, why: format!("no {:?} encoder", self.fail) });
            }
            std::thread::sleep(wait.min(Duration::from_millis(5)));
            Ok(None)
        }
        fn set_layers(&mut self, layers: &[LayerSpec]) {
            lock_seen(&self.seen).push(layers.to_vec());
            for l in layers.iter().filter(|l| l.on && l.codec == self.fail) {
                if !self.failed.contains(&l.id) {
                    self.failed.push(l.id);
                    self.pending.push(l.id);
                }
            }
        }
        fn request_keyframe(&mut self, _layer: u8, _since: Option<Instant>) {}
        fn sizes(&self) -> Option<SourceSizes> {
            Some(SourceSizes { points: 400, native_px: Some(1200) })
        }
    }

    fn lock_seen(m: &Mutex<Vec<Vec<LayerSpec>>>) -> MutexGuard<'_, Vec<Vec<LayerSpec>>> {
        m.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn event_within(sub: &mut Subscription, limit: Duration) -> Option<HubEvent> {
        let t0 = Instant::now();
        while t0.elapsed() < limit {
            if let Ok(e) = sub.events.try_recv() {
                return Some(e);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        None
    }

    fn fails(key: i64, fail: Codec, codec: Codec) -> (Subscription, Arc<Mutex<Vec<Vec<LayerSpec>>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let src = FailsCodec { fail, failed: Vec::new(), pending: Vec::new(), seen: seen.clone() };
        let want = Want { codec, fps: 15, path: ViewerPath::Local, pw: None, stats: false };
        let sub = join(SourceKey::Sim(key), want, move || Ok::<_, String>(Box::new(src) as Box<dyn EncodedSource>)).expect("join");
        (sub, seen)
    }

    /// A failed H.264 layer moves its viewers to JPEG (S2's rule).
    #[test]
    fn a_failed_video_layer_moves_its_viewers_to_jpeg() {
        let (mut sub, _seen) = fails(-9_101, Codec::H264, Codec::H264);
        match event_within(&mut sub, Duration::from_secs(5)) {
            Some(HubEvent::Moved { codec: Codec::Jpeg, .. }) => {}
            other => panic!("want Moved to JPEG, got {other:?}"),
        }
    }

    /// A failed JPEG layer ends its viewers once, with the reason, instead
    /// of opening a new JPEG layer that fails again (a Mac deskd, D6).
    #[test]
    fn a_failed_jpeg_layer_ends_its_viewers_without_a_retry_loop() {
        let (mut sub, seen) = fails(-9_102, Codec::Jpeg, Codec::Jpeg);
        match event_within(&mut sub, Duration::from_secs(5)) {
            Some(HubEvent::Ended(why)) => assert!(why.contains("can't send JPEG") && why.contains("Jpeg"), "{why}"),
            other => panic!("want Ended, got {other:?}"),
        }
        std::thread::sleep(Duration::from_millis(200));
        let jpeg_layers: std::collections::HashSet<u8> = lock_seen(&seen).iter().flatten().filter(|l| l.on && l.codec == Codec::Jpeg).map(|l| l.id).collect();
        assert_eq!(jpeg_layers.len(), 1, "one JPEG layer was tried, not a loop: {:?}", lock_seen(&seen));
        assert!(event_within(&mut sub, Duration::from_millis(50)).is_none(), "one end, nothing after");
    }

    #[test]
    fn fps_rounds_up_to_a_tier_never_past_the_cap() {
        assert_eq!(tier_for(60, 60), 60);
        assert_eq!(tier_for(31, 60), 60);
        assert_eq!(tier_for(24, 30), 30);
        assert_eq!(tier_for(15, 15), 15);
        assert_eq!(tier_for(9, 15), 15);
        assert_eq!(tier_for(8, 15), 8);
        assert_eq!(tier_for(2, 15), 8);
        assert_eq!(tier_for(1, 30), 8);
        assert_eq!(tier_for(45, 45), 45, "a cap between tiers is the layer's rate");
        assert_eq!(tier_for(99, 60), 60);
    }
}
