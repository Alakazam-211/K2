//! The hub's S4 rules on a fake clock (PRD remote desktop §3.5: skip to the
//! next keyframe, R8; rate control per layer). A hub with no capture
//! thread, stepped by hand as the capture loop steps it; a fake source that
//! keys on request; viewers driven through [`ViewerFlow`] as the viewer
//! loop drives them, with acks that come back after a round trip (or not,
//! while a viewer's link stalls). Linux is enough: no encodes, no sockets.

use super::*;
use k2_core::sim::stream::{SkipCounts, StatsWindow};
use k2_core::stream::Bytes;
use std::collections::VecDeque;

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// A hub on `clock` (not in the hub map, no capture thread) whose source
/// knows its sizes. A sim key: local H.264 goes to 60 (the rollback switch
/// is never set in tests).
fn hub(clock: &Arc<Mutex<Instant>>) -> Arc<Hub> {
    let c = clock.clone();
    let hub = Arc::new(Hub::new(SourceKey::Sim(0), 0, Arc::new(move || *lock(&c))));
    lock(&hub.state).sizes = Some(SourceSizes { points: 1280, native_px: Some(1280) });
    hub
}

fn want(fps: u32) -> Want {
    Want { codec: Codec::H264, fps, path: ViewerPath::Local, pw: None, stats: false, height: None }
}

fn unit(layer: u8, seq: u64, key: bool, at: Instant) -> EncodedUnit {
    EncodedUnit {
        codec: Codec::H264,
        key,
        source_start: seq == 1,
        layer,
        width: 1280,
        height: 720,
        width_pt: 1280,
        height_pt: 720,
        seq,
        captured_at: at,
        stamps: Default::default(),
        pulled_at: at,
        encode: None,
        data: Bytes::from_static(b"x"),
    }
}

fn set(clock: &Arc<Mutex<Instant>>, t: Instant) {
    *lock(clock) = t;
}

/// A fake source on a fake clock: per layer a unit every 1/fps, a key unit
/// first and after each ask; the asks it got, at their time from the start.
struct Run {
    hub: Arc<Hub>,
    clock: Arc<Mutex<Instant>>,
    t0: Instant,
    /// (spec, last seq, next due, key wanted)
    layers: HashMap<u8, (LayerSpec, u64, Instant, bool)>,
    asks: Vec<(Duration, u8)>,
}

impl Run {
    fn new() -> Run {
        let t0 = Instant::now();
        let clock = Arc::new(Mutex::new(t0));
        Run { hub: hub(&clock), clock, t0, layers: HashMap::new(), asks: Vec::new() }
    }

    fn now(&self) -> Instant {
        *lock(&self.clock)
    }

    /// One capture-loop turn: the hub's bookkeeping, the layer list, the
    /// asks, then each layer's unit if one is due.
    fn turn(&mut self) {
        let now = self.now();
        let (list, asks) = self.hub.step(now);
        if let Some(list) = list {
            self.layers.retain(|id, _| list.iter().any(|l| l.id == *id && l.on));
            for l in list.into_iter().filter(|l| l.on) {
                self.layers.entry(l.id).and_modify(|e| e.0 = l).or_insert((l, 0, now, true));
            }
        }
        for (layer, _) in asks {
            self.asks.push((now - self.t0, layer));
            if let Some(e) = self.layers.get_mut(&layer) {
                e.3 = true;
            }
        }
        let mut ids: Vec<u8> = self.layers.keys().copied().collect();
        ids.sort();
        for id in ids {
            let Some(e) = self.layers.get_mut(&id) else { continue };
            if e.2 > now {
                continue;
            }
            e.1 += 1;
            let key = std::mem::take(&mut e.3);
            e.2 += Duration::from_secs_f64(1.0 / e.0.fps.max(1) as f64);
            let u = unit(id, e.1, key, now);
            self.hub.fan_out(u);
        }
    }

    /// Run for `dur` in 1 ms steps: a turn, then each viewer.
    fn run(&mut self, dur: Duration, viewers: &mut [&mut Viewer]) {
        let end = self.now() + dur;
        while self.now() < end {
            self.turn();
            let now = self.now();
            for v in viewers.iter_mut() {
                v.pump(now);
            }
            *lock(&self.clock) += ms(1);
        }
    }
}

/// A viewer as the viewer loop runs it: its [`ViewerFlow`], acks after
/// `rtt` (held while its link stalls, then all at once), what it got.
struct Viewer {
    sub: Subscription,
    flow: ViewerFlow,
    rtt: Duration,
    /// Spans (from the run's start) its acks don't come back in.
    stalls: Vec<(Duration, Duration)>,
    t0: Instant,
    acks: VecDeque<(Instant, u32)>,
    /// (when, source seq, key) of each unit sent to it.
    got: Vec<(Duration, u64, bool)>,
    /// When each of its skips started.
    skips: Vec<Duration>,
    stats: StatsWindow,
}

impl Viewer {
    fn join(run: &Run, fps: u32, window: u32) -> Viewer {
        let sub = Hub::subscribe(&run.hub, want(fps));
        let flow = ViewerFlow::new(sub.layer(), window, true);
        Viewer { sub, flow, rtt: ms(10), stalls: Vec::new(), t0: run.t0, acks: VecDeque::new(), got: Vec::new(), skips: Vec::new(), stats: StatsWindow::default() }
    }

    fn stalled(&self, now: Instant) -> bool {
        let at = now - self.t0;
        self.stalls.iter().any(|(a, b)| at >= *a && at < *b)
    }

    fn pump(&mut self, now: Instant) {
        if !self.stalled(now) {
            while self.acks.front().is_some_and(|(due, _)| *due <= now) {
                let Some((_, seq)) = self.acks.pop_front() else { break };
                self.flow.ack(seq, now, &self.sub);
            }
        }
        while let Ok(ev) = self.sub.events.try_recv() {
            if let HubEvent::Moved { layer, .. } = ev {
                self.flow.moved(layer);
            }
        }
        if let Some((u, seq)) = self.flow.held() {
            self.deliver(u, seq, now);
        }
        while let Ok(u) = self.sub.units.try_recv() {
            let before = self.flow.gate.skipped.total();
            let out = self.flow.offer(u, now, &self.sub, Some(&mut self.stats));
            if self.flow.gate.skipped.total() > before {
                self.skips.push(now - self.t0);
            }
            if let Some((u, seq)) = out {
                self.deliver(u, seq, now);
            }
        }
    }

    fn deliver(&mut self, u: EncodedUnit, seq: u32, now: Instant) {
        self.flow.sent(seq, now, ms(1), &self.sub, Some(&mut self.stats));
        self.got.push((now - self.t0, u.seq, u.key));
        self.acks.push_back((now + self.rtt, seq));
    }

    fn keys(&self) -> usize {
        self.got.iter().filter(|g| g.2).count()
    }
}

/// Never a delta after a gap: each unit sent is a key or the one after the
/// unit before it.
fn assert_chain(who: &str, got: &[(Duration, u64, bool)]) {
    assert!(got.first().is_some_and(|g| g.2), "{who} starts on a key: {got:?}");
    for w in got.windows(2) {
        assert!(w[1].2 || w[1].1 == w[0].1 + 1, "{who}: delta {} after {}", w[1].1, w[0].1);
    }
}

/// (a) R8: a viewer whose acks stall past max(2 × srtt, 250 ms) skips to the
/// next key unit (the age rule: at 8 fps its window of 3 still has room
/// then) and asks once; the other viewer on its layer keeps its rate and
/// gets only that one extra key.
#[test]
fn a_stalled_viewer_skips_on_age_and_its_layer_mates_keep_their_rate() {
    let mut run = Run::new();
    let mut a = Viewer::join(&run, 8, 3);
    let mut b = Viewer::join(&run, 8, 3);
    assert_eq!(a.sub.layer(), b.sub.layer(), "one layer");
    a.stalls.push((ms(1000), ms(1600)));
    run.run(ms(3000), &mut [&mut a, &mut b]);
    assert_eq!(a.flow.gate.skipped, SkipCounts { window: 0, age: 1, send: 0 }, "{:?}", a.got);
    let skip = a.skips[0];
    assert!(skip > ms(1250) && skip <= ms(1400), "once its oldest unacked unit passed 250 ms: {skip:?}");
    assert_chain("a", &a.got);
    let after: Vec<_> = a.got.iter().filter(|g| g.0 > skip).collect();
    assert!(after.first().is_some_and(|g| g.2), "the first unit after the skip is a key: {after:?}");
    assert!(after.len() >= 8, "and it streams again: {after:?}");
    assert_chain("b", &b.got);
    assert_eq!(b.flow.gate.skipped.total(), 0, "b never skips");
    assert!((23..=25).contains(&b.got.len()), "b keeps 8 fps for 3 s: {}", b.got.len());
    assert_eq!(b.keys(), 2, "b's keys: the layer's first and the one a asked for");
    let catch_ups: Vec<_> = run.asks.iter().filter(|(t, _)| *t > Duration::ZERO).collect();
    assert_eq!(catch_ups.len(), 1, "one ask after b joined: {:?}", run.asks);
}

/// (c) Two skips inside a second: the second ask is throttled but kept,
/// and asked when the layer's gap ends, so the picture is back within the
/// gap and a frame (S2's known gap); asks stay at most one a second.
#[test]
fn a_second_skip_inside_a_second_is_asked_when_the_gap_ends() {
    let mut run = Run::new();
    let mut a = Viewer::join(&run, 30, 3);
    a.stalls = vec![(ms(1000), ms(1150)), (ms(1400), ms(1550))];
    run.run(ms(3000), &mut [&mut a]);
    assert_eq!(a.flow.gate.skipped, SkipCounts { window: 2, age: 0, send: 0 }, "{:?}", a.skips);
    let (s1, s2) = (a.skips[0], a.skips[1]);
    assert!(s2 - s1 < CATCH_UP_GAP, "two skips inside a second: {s1:?}, {s2:?}");
    let asks: Vec<Duration> = run.asks.iter().map(|(t, _)| *t).collect();
    assert_eq!(asks.len(), 2, "a lone viewer: no joiner ask, one per skip: {asks:?}");
    assert!(asks[0] >= s1 && asks[0] <= s1 + ms(1), "the first ask goes at once: {asks:?} after {s1:?}");
    assert!(asks[1] >= s1 + CATCH_UP_GAP && asks[1] <= s1 + CATCH_UP_GAP + ms(1), "the kept one goes when the gap ends: {asks:?}");
    let frame = Duration::from_secs_f64(1.0 / 30.0);
    let back = a.got.iter().find(|g| g.0 > s2).expect("the picture comes back");
    assert!(back.2, "on a key: {back:?}");
    assert!(back.0 <= s1 + CATCH_UP_GAP + frame + ms(2), "within the gap and a frame: back at {:?}, first skip {s1:?}", back.0);
    assert_chain("a", &a.got);
    let st = a.stats.take(0.0, 0.0);
    assert_eq!((st.skips.window, st.catch_ups), (2, 2));
}

/// (c, d) A joiner's ask is never throttled, even right after a catch-up
/// and with one kept. A key unit that reaches the waiting viewer ends the
/// wait, so the kept ask is dropped; with no key, it goes when the gap
/// ends, and again a gap later, never closer.
#[test]
fn a_joiner_is_never_throttled_and_a_key_ends_a_kept_ask() {
    let t0 = Instant::now();
    let clock = Arc::new(Mutex::new(t0));
    let hub = hub(&clock);
    let a = Hub::subscribe(&hub, want(30));
    let _ = hub.step(t0);
    assert!(a.catch_up(), "the first catch-up goes");
    assert!(!a.catch_up(), "a second inside the gap is kept, not asked");
    let before = hub.key_asks();
    let j = Hub::subscribe(&hub, want(30));
    assert_eq!(j.layer(), a.layer());
    assert_eq!(hub.key_asks(), before + 1, "the joiner's ask went at once");
    let (_, asks) = hub.step(t0);
    assert_eq!(asks.len(), 2, "the catch-up and the joiner's: {asks:?}");
    hub.fan_out(unit(a.layer(), 9, true, t0));
    assert!(hub.step(t0 + CATCH_UP_GAP).1.is_empty(), "the joiner's key reached a: nothing kept");
    let t1 = t0 + CATCH_UP_GAP;
    set(&clock, t1);
    assert!(a.catch_up(), "a new skip after the gap goes");
    set(&clock, t1 + ms(300));
    assert!(!a.catch_up(), "kept");
    assert_eq!(hub.step(t1 + ms(300)).1.len(), 1, "only the first");
    assert!(hub.step(t1 + CATCH_UP_GAP - ms(1)).1.is_empty(), "not before the gap ends");
    assert_eq!(hub.step(t1 + CATCH_UP_GAP).1, vec![(a.layer(), None)], "kept, and asked when the gap ends");
    assert!(hub.step(t1 + CATCH_UP_GAP + ms(999)).1.is_empty(), "at most one a second");
    assert_eq!(hub.step(t1 + CATCH_UP_GAP * 2).1.len(), 1, "still no key: again a gap later");
    set(&clock, t1 + CATCH_UP_GAP * 2);
    let late = Hub::subscribe(&hub, want(30));
    assert_eq!(hub.step(t1 + CATCH_UP_GAP * 2).1.len(), 1, "a joiner inside the gap is still asked for at once");
    drop((a, j, late));
}

/// (b) One send over 100 ms starts a skip: the next deltas wait for a key
/// and the viewer asks the hub for one, once.
#[test]
fn a_slow_send_skips_to_the_next_key_and_asks_once() {
    let t0 = Instant::now();
    let clock = Arc::new(Mutex::new(t0));
    let hub = hub(&clock);
    let a = Hub::subscribe(&hub, want(30));
    let _ = hub.step(t0);
    let l = a.layer();
    let mut flow = ViewerFlow::new(l, 3, true);
    let mut st = StatsWindow::default();
    let (_, seq) = flow.offer(unit(l, 1, true, t0), t0, &a, Some(&mut st)).expect("a key goes");
    flow.sent(seq, t0, ms(100), &a, Some(&mut st));
    flow.ack(seq, t0 + ms(5), &a);
    let (_, seq) = flow.offer(unit(l, 2, false, t0), t0 + ms(33), &a, Some(&mut st)).expect("100 ms is not a slow send");
    let asks = hub.key_asks();
    flow.sent(seq, t0 + ms(140), ms(101), &a, Some(&mut st));
    assert_eq!(hub.key_asks(), asks + 1, "it asked for a key");
    assert!(flow.offer(unit(l, 3, false, t0), t0 + ms(141), &a, Some(&mut st)).is_none(), "deltas wait for a key");
    assert!(flow.offer(unit(l, 4, false, t0), t0 + ms(142), &a, Some(&mut st)).is_none());
    assert_eq!(hub.key_asks(), asks + 1, "once");
    assert!(flow.offer(unit(l, 5, true, t0), t0 + ms(150), &a, Some(&mut st)).is_some(), "the next key goes");
    assert!(flow.offer(unit(l, 6, false, t0), t0 + ms(151), &a, Some(&mut st)).is_some(), "then the chain");
    assert_eq!(flow.gate.skipped, SkipCounts { window: 0, age: 0, send: 1 });
    assert_eq!(st.take(0.0, 0.0).skips.send, 1);
}

/// (f) The stats window counts each skip by why, and its catch-up ask.
#[test]
fn stats_count_skips_by_why_and_catch_up_asks() {
    let t0 = Instant::now();
    let clock = Arc::new(Mutex::new(t0));
    let hub = hub(&clock);
    let a = Hub::subscribe(&hub, want(30));
    let _ = hub.step(t0);
    let l = a.layer();
    let mut flow = ViewerFlow::new(l, 1, true);
    let mut st = StatsWindow::default();
    let t = |n: u64| t0 + ms(n);
    let (_, s1) = flow.offer(unit(l, 1, true, t0), t(0), &a, Some(&mut st)).expect("key");
    flow.sent(s1, t(0), ms(1), &a, Some(&mut st));
    // A window of one, full: a window skip.
    assert!(flow.offer(unit(l, 2, false, t0), t(10), &a, Some(&mut st)).is_none());
    assert!(flow.offer(unit(l, 3, true, t0), t(12), &a, Some(&mut st)).is_none(), "held for room");
    flow.ack(s1, t(20), &a);
    let (_, s3) = flow.held().expect("the held key");
    flow.sent(s3, t(20), ms(1), &a, Some(&mut st));
    flow.ack(s3, t(30), &a);
    let (_, s4) = flow.offer(unit(l, 4, false, t0), t(30), &a, Some(&mut st)).expect("the chain");
    flow.sent(s4, t(30), ms(1), &a, Some(&mut st));
    // 4 unacked for more than 250 ms: an age skip.
    assert!(flow.offer(unit(l, 5, false, t0), t(300), &a, Some(&mut st)).is_none());
    assert!(flow.offer(unit(l, 6, true, t0), t(310), &a, Some(&mut st)).is_none(), "held for room");
    flow.ack(s4, t(320), &a);
    let (_, s6) = flow.held().expect("the held key");
    // Its send takes 150 ms: a send skip.
    flow.sent(s6, t(470), ms(150), &a, Some(&mut st));
    assert!(flow.offer(unit(l, 7, false, t0), t(480), &a, Some(&mut st)).is_none());
    let s = st.take(0.0, 0.0);
    assert_eq!(s.skips, SkipCounts { window: 1, age: 1, send: 1 });
    assert_eq!(s.catch_ups, 3, "one ask per skip");
    assert_eq!(flow.gate.skipped, s.skips);
}

/// (e) The layer's bitrate follows its most congested viewer; another layer
/// keeps its own. A viewer congested for 5 s while the other on its layer
/// isn't moves down a tier (same codec; `Moved`); alone there, it stays
/// however long it is congested, and only its layer's rate drops.
#[test]
fn a_viewer_congested_for_5_s_beside_a_clear_one_moves_down_a_layer() {
    let t0 = Instant::now();
    let clock = Arc::new(Mutex::new(t0));
    let hub = hub(&clock);
    let mut a = Hub::subscribe(&hub, want(30));
    let mut b = Hub::subscribe(&hub, want(30));
    let mut c = Hub::subscribe(&hub, want(60));
    let _ = hub.step(t0);
    let rate = |s: &Subscription| s.layer_spec().expect("spec").bitrate;
    let (r30, r60) = (rate(&a), rate(&c));
    a.set_congested(true);
    let _ = hub.step(t0);
    assert!(rate(&b) < r30, "the layer follows its most congested viewer");
    assert_eq!(rate(&c), r60, "another layer keeps its rate");
    let _ = hub.step(t0 + MOVE_DOWN_AFTER - ms(1));
    assert!(a.events.try_recv().is_err(), "not before 5 s");
    let from = a.layer();
    let _ = hub.step(t0 + MOVE_DOWN_AFTER);
    match a.events.try_recv() {
        Ok(HubEvent::Moved { layer, codec: Codec::H264, fps: 15, .. }) => assert_eq!(layer, a.layer()),
        other => panic!("a moves down to 15 fps H.264: {other:?}"),
    }
    assert_ne!(a.layer(), from);
    assert_eq!(b.layer(), from, "the clear one stays");
    assert_eq!(a.layer_spec().map(|l| (l.codec, l.fps, l.on)), Some((Codec::H264, 15, true)));
    assert!(b.events.try_recv().is_err() && c.events.try_recv().is_err());
    assert_eq!(hub.layer_specs().len(), 3);
    // Alone on its layer (and c alone on its), congested for a minute: no
    // move, the rate control's.
    let t1 = t0 + MOVE_DOWN_AFTER;
    set(&clock, t1);
    a.set_congested(true);
    c.set_congested(true);
    let r15 = rate(&a);
    for s in 1..=60 {
        let _ = hub.step(t1 + Duration::from_secs(s));
    }
    assert!(a.events.try_recv().is_err() && c.events.try_recv().is_err(), "a lone viewer is never moved");
    assert!(rate(&a) < r15 && rate(&c) < r60, "its layer's rate drops instead");
    // Two congested side by side: neither moves.
    set(&clock, t1);
    b.set_congested(false);
    let mut d = Hub::subscribe(&hub, want(30));
    assert_eq!(d.layer(), b.layer());
    b.set_congested(true);
    d.set_congested(true);
    let _ = hub.step(t1 + MOVE_DOWN_AFTER * 2);
    assert!(b.events.try_recv().is_err() && d.events.try_recv().is_err(), "all congested: the rate control's");
}

/// (e) At the cap: with layers at 60, 30 and 8, a viewer moving down from
/// 30 goes to 8 (no fourth layer opens) and waits for a key there (the
/// joiner's ask); one on the lowest tier never moves.
#[test]
fn a_move_down_respects_the_layer_cap_and_stops_at_the_lowest_tier() {
    let t0 = Instant::now();
    let clock = Arc::new(Mutex::new(t0));
    let hub = hub(&clock);
    let _p = Hub::subscribe(&hub, want(60));
    let mut a = Hub::subscribe(&hub, want(30));
    let _b = Hub::subscribe(&hub, want(30));
    let mut q = Hub::subscribe(&hub, want(8));
    let _r = Hub::subscribe(&hub, want(8));
    let _ = hub.step(t0);
    assert_eq!(hub.layer_specs().len(), MAX_LAYERS);
    a.set_congested(true);
    q.set_congested(true);
    let asks = hub.key_asks();
    let (_, sent) = hub.step(t0 + MOVE_DOWN_AFTER);
    match a.events.try_recv() {
        Ok(HubEvent::Moved { layer, fps: 8, codec: Codec::H264, .. }) => assert_eq!(layer, q.layer()),
        other => panic!("a goes to the 8 fps layer: {other:?}"),
    }
    assert_eq!(hub.layer_specs().len(), MAX_LAYERS, "no fourth layer");
    assert_eq!(hub.key_asks(), asks + 1, "a joiner's ask on its new layer");
    assert!(sent.contains(&(q.layer(), Some(t0))), "unthrottled, at once: {sent:?}");
    assert!(q.events.try_recv().is_err(), "nothing below 8 fps");
}
