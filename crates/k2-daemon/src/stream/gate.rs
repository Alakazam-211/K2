//! [`ViewerGate`]: what one viewer may send of its layer's units. **Never a
//! delta after a gap**: a delta goes only right after the unit before it
//! (same layer, seq + 1) and only while the viewer has room in its ack
//! window. Otherwise it is dropped and the viewer waits for the next key
//! unit (asking the hub for one: [`Took::Skip`]). A key unit that finds
//! no room is held (a newer one replaces it) and goes when room frees, so a
//! JPEG viewer gets the newest frame after each ack, as before.
//!
//! **Skip to the next keyframe (R8, PRD remote desktop §3.5; S4).** Besides
//! a full window, a viewer skips when its oldest unacked unit is older than
//! [`skip_age`], `max(2 × srtt, 250 ms)` (once per stuck unit;
//! [`ViewerGate::check_age`]), or when one socket send took longer than
//! [`SKIP_SEND`], 100 ms ([`ViewerGate::send_took`]). Each skip is counted by
//! why ([`SkipReason`]) and asks the hub for a key once; the drops while it
//! waits ask nothing more. [`ViewerFlow`] is the whole send side as the
//! viewer loop runs it (the window, the gate, the asks, the stats).

use std::time::{Duration, Instant};

use k2_core::sim::stream::{SkipCounts, SkipReason, StatsWindow};
use k2_core::sim::video::flow::AckWindow;
use k2_core::stream::EncodedUnit;

use super::Subscription;

/// The skip rule's age floor: an oldest unacked unit older than this (or
/// [`SKIP_AGE_RTTS`] round trips, whichever is longer) starts a skip.
pub const SKIP_AGE_MIN: Duration = Duration::from_millis(250);
/// The skip rule's age in smoothed round trips.
pub const SKIP_AGE_RTTS: u32 = 2;
/// One socket send longer than this starts a skip.
pub const SKIP_SEND: Duration = Duration::from_millis(100);

/// The oldest unacked unit's age past which a viewer skips:
/// `max(2 × srtt, 250 ms)` (250 ms before the first ack).
pub fn skip_age(srtt: Option<Duration>) -> Duration {
    srtt.map_or(SKIP_AGE_MIN, |s| (s * SKIP_AGE_RTTS).max(SKIP_AGE_MIN))
}

/// What [`ViewerGate::offer`] did with a unit.
#[derive(Debug)]
pub enum Took {
    /// Send it now.
    Send(EncodedUnit),
    /// A key unit kept until there is room ([`ViewerGate::take_held`]).
    Held,
    /// A delta the viewer can't take, and it was on the chain: it skips to
    /// the next key unit and should ask the hub for one (the hub throttles
    /// catch-up asks per layer).
    Skip(SkipReason),
    /// A delta dropped while the viewer already waits for a key unit (it
    /// skipped and asked, or it just joined or moved and the hub asked).
    Dropped,
    /// Another layer's (the viewer moved): ignored.
    Stale,
}

#[derive(Debug)]
pub struct ViewerGate {
    layer: u8,
    /// The seq of the last unit sent or held on this layer: a delta must be
    /// the one after it. `None` = waiting for a key unit.
    chain: Option<u64>,
    held: Option<EncodedUnit>,
    /// Wire seqs stay monotonic across a layer move: wire = seq + offset.
    offset: u32,
    rebase: bool,
    last_wire: Option<u32>,
    /// Deltas dropped.
    pub skips: u64,
    /// Skips to the next key unit, by why (each one asked the hub once).
    pub skipped: SkipCounts,
    /// The send time of the oldest unacked unit the age rule last fired
    /// for: it fires once per stuck unit.
    aged: Option<Instant>,
}

impl ViewerGate {
    pub fn new(layer: u8) -> Self {
        ViewerGate { layer, chain: None, held: None, offset: 0, rebase: false, last_wire: None, skips: 0, skipped: SkipCounts::default(), aged: None }
    }

    pub fn layer(&self) -> u8 {
        self.layer
    }

    /// The viewer moved to `layer` (a new rate, or the JPEG fallback): wait
    /// for a key unit there.
    pub fn moved(&mut self, layer: u8) {
        if layer != self.layer {
            self.layer = layer;
            self.chain = None;
            self.held = None;
            self.rebase = true;
        }
    }

    /// A unit off the hub's queue; `room` = the viewer may put one more
    /// unit in flight now. Call [`Self::take_held`] first when room frees.
    pub fn offer(&mut self, u: EncodedUnit, room: bool) -> Took {
        if u.layer != self.layer {
            return Took::Stale;
        }
        let room = room && self.held.is_none();
        if u.key {
            self.chain = Some(u.seq);
            if room {
                return Took::Send(u);
            }
            self.held = Some(u);
            return Took::Held;
        }
        if room && self.chain.is_some_and(|c| u.seq == c.wrapping_add(1)) {
            self.chain = Some(u.seq);
            return Took::Send(u);
        }
        self.skips += 1;
        if self.chain.take().is_some() {
            self.skipped.add(SkipReason::Window);
            return Took::Skip(SkipReason::Window);
        }
        Took::Dropped
    }

    /// Is it on the chain, with no key unit held? (Only then can a rule
    /// start a skip: otherwise it already waits for a key.)
    fn chained(&self) -> bool {
        self.chain.is_some() && self.held.is_none()
    }

    /// The age rule, before a delta is offered at `now`: the window's
    /// oldest unacked unit is older than [`skip_age`], so drop deltas until
    /// the next key unit. Once per stuck unit. True when it started a skip
    /// (ask the hub for a key).
    pub fn check_age(&mut self, w: &AckWindow, now: Instant) -> bool {
        let Some(sent) = w.oldest_sent() else { return false };
        if !self.chained() || self.aged.is_some_and(|t| t >= sent) || now.saturating_duration_since(sent) <= skip_age(w.srtt()) {
            return false;
        }
        self.aged = Some(sent);
        self.chain = None;
        self.skipped.add(SkipReason::Age);
        true
    }

    /// The send rule: a socket send just took `took`. Longer than
    /// [`SKIP_SEND`] drops deltas until the next key unit. True when it
    /// started a skip (ask the hub for a key).
    pub fn send_took(&mut self, took: Duration) -> bool {
        if took <= SKIP_SEND || !self.chained() {
            return false;
        }
        self.chain = None;
        self.skipped.add(SkipReason::Send);
        true
    }

    /// The held key unit, once there is room.
    pub fn take_held(&mut self, room: bool) -> Option<EncodedUnit> {
        if room {
            self.held.take()
        } else {
            None
        }
    }

    /// The wire seq (K2SF / K2SV header, acks) for a unit being sent; call
    /// once per sent unit, in order. A source whose seqs start over (a
    /// protocol-4 source on a new display, or restarted) is rebased too, so
    /// wire seqs only rise.
    pub fn wire_seq(&mut self, u: &EncodedUnit) -> u32 {
        let seq = u.seq as u32;
        let behind = self.last_wire.is_some_and(|l| (seq.wrapping_add(self.offset).wrapping_sub(l) as i32) <= 0);
        if self.rebase || behind {
            self.offset = self.last_wire.map_or(0, |w| w.wrapping_add(1)).wrapping_sub(seq);
            self.rebase = false;
        }
        let w = seq.wrapping_add(self.offset);
        self.last_wire = Some(w);
        w
    }
}

/// May the viewer put one more unit in flight? (The window's delay-based
/// cap: a layer is paced at its source, so a delta that fits the window
/// goes; the round-trip limit would only drop it and cost a keyframe.)
fn room(w: &AckWindow) -> bool {
    (w.in_flight() as u32) < w.cap()
}

/// One viewer's send side: its ack window, its [`ViewerGate`] and the skip
/// rule, on whatever clock it is handed (the viewer loop's real one; the
/// hub tests' fake one). A skip asks the hub for a key once
/// ([`Subscription::catch_up`]) and counts in the stats window.
#[derive(Debug)]
pub struct ViewerFlow {
    pub gate: ViewerGate,
    pub window: AckWindow,
    /// A video stream (only its link state steers a layer's rate).
    pub video: bool,
    /// What the hub was last told of this viewer's link.
    congested: bool,
}

impl ViewerFlow {
    pub fn new(layer: u8, window_max: u32, video: bool) -> Self {
        ViewerFlow { gate: ViewerGate::new(layer), window: AckWindow::new(window_max), video, congested: false }
    }

    /// The key unit that waited for room, once there is room, with its
    /// wire seq. The caller sends it, then calls [`Self::sent`].
    pub fn held(&mut self) -> Option<(EncodedUnit, u32)> {
        let u = self.gate.take_held(room(&self.window))?;
        let seq = self.gate.wire_seq(&u);
        Some((u, seq))
    }

    /// A unit off the hub at `now`: the age rule, then the gate. Some = send
    /// it now with this wire seq, then call [`Self::sent`].
    pub fn offer(&mut self, u: EncodedUnit, now: Instant, sub: &Subscription, mut stats: Option<&mut StatsWindow>) -> Option<(EncodedUnit, u32)> {
        if !u.key && u.layer == self.gate.layer() && self.gate.check_age(&self.window, now) {
            skipped(SkipReason::Age, sub, stats.as_deref_mut());
        }
        match self.gate.offer(u, room(&self.window)) {
            Took::Send(u) => {
                let seq = self.gate.wire_seq(&u);
                Some((u, seq))
            }
            Took::Skip(why) => {
                skipped(why, sub, stats);
                None
            }
            Took::Held | Took::Dropped | Took::Stale => None,
        }
    }

    /// A unit went out (wire `seq`), finishing at `at` after `took`: it is
    /// in flight, and a slow send starts a skip.
    pub fn sent(&mut self, seq: u32, at: Instant, took: Duration, sub: &Subscription, stats: Option<&mut StatsWindow>) {
        self.window.sent(seq, at);
        if self.gate.send_took(took) {
            skipped(SkipReason::Send, sub, stats);
        }
    }

    /// The viewer acked `seq` at `now`. The layer's bitrate follows its most
    /// congested viewer, so a change in this one's link goes to the hub.
    pub fn ack(&mut self, seq: u32, now: Instant, sub: &Subscription) {
        self.window.ack(seq, now);
        if self.video && self.window.congested() != self.congested {
            self.congested = !self.congested;
            sub.set_congested(self.congested);
        }
    }

    /// The hub put the viewer on `layer` (a new rate, a move down, the JPEG
    /// fallback): wait for a key unit there. A new layer starts with the
    /// link clear, as the hub does.
    pub fn moved(&mut self, layer: u8) {
        if layer != self.gate.layer() {
            self.congested = false;
        }
        self.gate.moved(layer);
    }
}

fn skipped(why: SkipReason, sub: &Subscription, stats: Option<&mut StatsWindow>) {
    if let Some(st) = stats {
        st.skip(why);
        st.catch_up();
    }
    sub.catch_up();
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_core::stream::{Bytes, Codec};

    fn unit(layer: u8, seq: u64, key: bool) -> EncodedUnit {
        let now = Instant::now();
        EncodedUnit {
            codec: Codec::H264,
            key,
            source_start: false,
            layer,
            width: 4,
            height: 8,
            width_pt: 4,
            height_pt: 8,
            seq,
            captured_at: now,
            stamps: Default::default(),
            pulled_at: now,
            encode: None,
            data: Bytes::from_static(b"x"),
        }
    }

    fn sent(t: Took) -> Option<u64> {
        match t {
            Took::Send(u) => Some(u.seq),
            _ => None,
        }
    }

    #[test]
    fn a_joiner_waits_for_a_key_then_takes_the_chain() {
        let mut g = ViewerGate::new(0);
        assert!(matches!(g.offer(unit(0, 7, false), true), Took::Dropped), "a delta before any key");
        assert_eq!(sent(g.offer(unit(0, 8, true), true)), Some(8));
        assert_eq!(sent(g.offer(unit(0, 9, false), true)), Some(9));
        assert!(matches!(g.offer(unit(0, 11, false), true), Took::Skip(SkipReason::Window)), "a gap: a skip");
        assert!(matches!(g.offer(unit(0, 12, false), true), Took::Dropped), "still waiting");
        assert_eq!(sent(g.offer(unit(0, 13, true), true)), Some(13));
        assert_eq!(g.skips, 3);
        assert_eq!(g.skipped, SkipCounts { window: 1, age: 0, send: 0 }, "the wait for the first key is no skip");
    }

    #[test]
    fn no_room_drops_deltas_and_holds_the_newest_key() {
        let mut g = ViewerGate::new(0);
        assert_eq!(sent(g.offer(unit(0, 1, true), true)), Some(1));
        assert!(matches!(g.offer(unit(0, 2, false), false), Took::Skip(SkipReason::Window)), "window full");
        assert!(matches!(g.offer(unit(0, 3, false), true), Took::Dropped), "the chain broke at 2");
        assert!(matches!(g.offer(unit(0, 4, true), false), Took::Held));
        assert!(matches!(g.offer(unit(0, 5, true), false), Took::Held), "a newer key replaces it");
        assert!(g.take_held(false).is_none());
        assert!(matches!(g.offer(unit(0, 6, false), true), Took::Skip(SkipReason::Window)), "a held key goes first");
        assert_eq!(g.take_held(true).map(|u| u.seq), Some(5));
        // 6 was dropped after the held key: wait for the next key.
        assert!(matches!(g.offer(unit(0, 7, false), true), Took::Dropped));
        assert_eq!(sent(g.offer(unit(0, 8, true), true)), Some(8));
        assert_eq!(sent(g.offer(unit(0, 9, false), true)), Some(9));
    }

    #[test]
    fn a_move_ignores_the_old_layer_and_keeps_wire_seqs_rising() {
        let mut g = ViewerGate::new(0);
        let a = unit(0, 40, true);
        assert_eq!(g.wire_seq(&a), 40);
        g.moved(2);
        assert!(matches!(g.offer(unit(0, 41, false), true), Took::Stale));
        let b = unit(2, 3, true);
        assert!(matches!(g.offer(b.clone(), true), Took::Send(_)));
        assert_eq!(g.wire_seq(&b), 41, "rebased past the last one sent");
        assert_eq!(g.wire_seq(&unit(2, 4, false)), 42);
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// R8: an oldest unacked unit older than max(2 × srtt, 250 ms) starts a
    /// skip, once per stuck unit and only on the chain.
    #[test]
    fn an_old_unacked_unit_starts_one_skip_per_stuck_unit() {
        assert_eq!((skip_age(None), skip_age(Some(ms(100))), skip_age(Some(ms(200)))), (ms(250), ms(250), ms(400)));
        let t = Instant::now();
        let mut g = ViewerGate::new(0);
        let mut w = AckWindow::new(3);
        assert!(!g.check_age(&w, t + ms(900)), "nothing in flight");
        assert_eq!(sent(g.offer(unit(0, 1, true), true)), Some(1));
        w.sent(1, t);
        assert!(!g.check_age(&w, t + ms(250)), "not older than 250 ms yet");
        assert!(g.check_age(&w, t + ms(251)));
        assert!(matches!(g.offer(unit(0, 2, false), true), Took::Dropped), "it waits for a key (it already asked)");
        assert_eq!(sent(g.offer(unit(0, 3, true), true)), Some(3));
        w.sent(3, t + ms(260));
        assert!(!g.check_age(&w, t + ms(600)), "the same stuck unit fires once");
        // The ack of 3 clears both, with a 240 ms round trip: the age is now 480 ms.
        assert!(w.ack(3, t + ms(500)));
        assert_eq!(sent(g.offer(unit(0, 4, false), true)), Some(4));
        w.sent(4, t + ms(500));
        assert!(!g.check_age(&w, t + ms(980)), "2 × srtt, not 250 ms");
        assert!(g.check_age(&w, t + ms(981)), "a new stuck unit fires again");
        assert_eq!(g.skipped, SkipCounts { window: 0, age: 2, send: 0 });
        // A held key: it already waits for room and a key.
        let mut h = ViewerGate::new(0);
        let mut hw = AckWindow::new(1);
        assert_eq!(sent(h.offer(unit(0, 1, true), true)), Some(1));
        hw.sent(1, t);
        assert!(matches!(h.offer(unit(0, 2, true), false), Took::Held));
        assert!(!h.check_age(&hw, t + ms(900)));
    }

    /// R8: one send longer than 100 ms starts a skip; a viewer already
    /// waiting for a key doesn't start another.
    #[test]
    fn a_send_over_100_ms_starts_a_skip() {
        let mut g = ViewerGate::new(0);
        assert!(!g.send_took(ms(900)), "not on a chain yet");
        assert_eq!(sent(g.offer(unit(0, 1, true), true)), Some(1));
        assert!(!g.send_took(ms(100)), "100 ms is not longer than 100 ms");
        assert_eq!(sent(g.offer(unit(0, 2, false), true)), Some(2));
        assert!(g.send_took(ms(101)));
        assert!(matches!(g.offer(unit(0, 3, false), true), Took::Dropped), "deltas wait for the next key");
        assert!(!g.send_took(ms(500)), "already waiting");
        assert_eq!(sent(g.offer(unit(0, 4, true), true)), Some(4));
        assert_eq!(sent(g.offer(unit(0, 5, false), true)), Some(5));
        assert_eq!(g.skipped, SkipCounts { window: 0, age: 0, send: 1 });
    }

    /// A source that starts its seqs over on the same layer (a new display)
    /// starts on a key unit; the chain follows it and wire seqs keep rising.
    #[test]
    fn a_source_that_starts_over_keeps_wire_seqs_rising() {
        let mut g = ViewerGate::new(0);
        for (seq, key) in [(90, true), (91, false)] {
            let u = unit(0, seq, key);
            assert!(matches!(g.offer(u.clone(), true), Took::Send(_)));
            assert_eq!(g.wire_seq(&u), seq as u32);
        }
        let restart = EncodedUnit { source_start: true, ..unit(0, 1, true) };
        assert!(matches!(g.offer(restart.clone(), true), Took::Send(_)));
        assert_eq!(g.wire_seq(&restart), 92);
        let next = unit(0, 2, false);
        assert!(matches!(g.offer(next.clone(), true), Took::Send(_)), "the chain follows the new seqs");
        assert_eq!(g.wire_seq(&next), 93);
    }
}
