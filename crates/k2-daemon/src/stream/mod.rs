//! The device stream engine, hub side (fix 10, S2; PRD remote desktop §3.4
//! E2–E4, E6). Source-neutral: the sim route (`sim_stream_ws`) feeds it a
//! [`k2_core::stream::pixel::PixelLayers`]; the desktop route (S5) will feed
//! it k2-deskd's protocol-4 units.
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
//! most congested viewer. The hub sends the whole list whenever it changes;
//! a layer with no viewers leaves the list (paused). Ids are small u8s,
//! stable while the layer lives and not reused at once.
//!
//! **Keyframes.** A viewer joining a running layer gets
//! `request_keyframe(layer, now)` at once, never throttled, and nothing on
//! that layer until a key unit. A viewer's own ask is throttled per layer
//! ([`KEYFRAME_MIN_GAP`]); a catching-up viewer's ([`Subscription::catch_up`])
//! to one per [`CATCH_UP_GAP`] per layer.
//!
//! **Failure.** A layer whose encoder fails moves its viewers to a JPEG
//! layer ([`HubEvent::Moved`]); a source that ends tells every viewer
//! ([`HubEvent::Ended`]) and the hub goes away.

mod gate;
pub mod rates;

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

pub use gate::{Took, ViewerGate};

/// Layer rates. A viewer's fps rounds up to the nearest.
pub const TIERS: [u32; 4] = [60, 30, 15, 8];
/// The most layers a source runs (one encode each).
pub const MAX_LAYERS: usize = 3;
/// A viewer's keyframe asks closer together than this are merged (per layer).
pub const KEYFRAME_MIN_GAP: Duration = Duration::from_millis(250);
/// Catch-up asks (a viewer that dropped deltas): one per layer per this.
pub const CATCH_UP_GAP: Duration = Duration::from_secs(1);
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
    /// Its layer's encoder failed: it is on `layer` now, a JPEG layer at
    /// `fps` (the JPEG row of the rate table).
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
}

#[derive(Default)]
struct State {
    layers: Vec<Layer>,
    viewers: HashMap<u64, Sub>,
    next_id: u8,
    /// The layer list changed since the source last got it.
    dirty: bool,
    /// Keyframe asks for the source: (layer, since).
    asks: Vec<(u8, Option<Instant>)>,
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
    let hub = Arc::new(Hub {
        key: key.clone(),
        gen: GEN.fetch_add(1, Ordering::SeqCst),
        state: Mutex::new(State::default()),
        stats_viewers: AtomicUsize::new(0),
        cpu_ns: AtomicU64::new(0),
        units: AtomicU64::new(0),
        key_asks: AtomicU64::new(0),
    });
    let sub = Hub::subscribe(&hub, want);
    let h2 = hub.clone();
    std::thread::Builder::new().name(format!("k2-stream-{key}")).spawn(move || capture(h2, source)).map_err(|e| JoinError::Thread(format!("capture thread: {e}")))?;
    map.insert(key, hub);
    Ok(sub)
}

impl Hub {
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
        st.viewers.insert(id, Sub { layer, want, units: units_tx, events: events_tx, congested: false });
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
                let now = Instant::now();
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
        let rate = RateCtl::new(if key.codec.is_video() { bitrate_for(w, h, key.fps, remote) } else { 0 });
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
    /// in its queue (the rest lose it and catch up on a key unit).
    fn fan_out(&self, u: EncodedUnit) {
        self.units.fetch_add(1, Ordering::SeqCst);
        let mut st = lock(&self.state);
        let mut retuned = false;
        if let Some(l) = st.layers.iter_mut().find(|l| l.id == u.layer) {
            if u.codec.is_video() && l.size != Some((u.width, u.height)) {
                l.size = Some((u.width, u.height));
                let target = bitrate_for(u.width, u.height, l.key.fps, l.key.remote);
                if target != l.rate.target() {
                    l.rate.retarget(target);
                    retuned = true;
                }
            }
        }
        st.dirty |= retuned;
        for s in st.viewers.values().filter(|s| s.layer == u.layer) {
            let _ = s.units.try_send(u.clone());
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

    /// `layer`'s encoder failed: its viewers go to JPEG.
    fn layer_failed(&self, layer: u8, why: &str) {
        let mut st = lock(&self.state);
        let kind = self.key.kind();
        let movers: Vec<u64> = st.viewers.iter().filter(|(_, s)| s.layer == layer).map(|(id, _)| *id).collect();
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
                s.congested = false;
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
        let (layers, asks) = {
            let mut st = lock(&hub.state);
            hub.rate_tick(&mut st, Instant::now());
            // A layer whose width isn't known yet stays out (paused).
            let layers = std::mem::take(&mut st.dirty).then(|| st.layers.iter().filter(|l| l.max_width.is_some()).map(Layer::spec).collect::<Vec<_>>());
            if let Some(l) = &layers {
                st.sent = l.clone();
            }
            (layers, std::mem::take(&mut st.asks))
        };
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

    /// A viewer that dropped deltas asks for a key unit: one per layer per
    /// [`CATCH_UP_GAP`]. True when passed on. (S2: two skips inside that
    /// second can leave a viewer on its last frame for up to a second; S4
    /// owns the skip rule and its thresholds.)
    pub fn catch_up(&self) -> bool {
        self.ask(true)
    }

    fn ask(&self, catch_up: bool) -> bool {
        let mut st = lock(&self.hub.state);
        let Some(id) = st.viewers.get(&self.id).map(|s| s.layer) else { return false };
        let now = Instant::now();
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
            s.congested = false;
        }
        self.hub.refresh_width(&mut st, to);
        to
    }

    /// The viewer's link state (its ack window's `congested`).
    pub fn set_congested(&self, congested: bool) {
        if let Some(s) = lock(&self.hub.state).viewers.get_mut(&self.id) {
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
mod tests {
    use super::*;

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
