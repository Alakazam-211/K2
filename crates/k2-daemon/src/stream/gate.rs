//! [`ViewerGate`]: what one viewer may send of its layer's units. **Never a
//! delta after a gap**: a delta goes only right after the unit before it
//! (same layer, seq + 1) and only while the viewer has room in its ack
//! window. Otherwise it is dropped and the viewer waits for the next key
//! unit (asking the hub for one: [`Took::Dropped`]). A key unit that finds
//! no room is held (a newer one replaces it) and goes when room frees, so a
//! JPEG viewer gets the newest frame after each ack, as before.

use k2_core::stream::EncodedUnit;

/// What [`ViewerGate::offer`] did with a unit.
#[derive(Debug)]
pub enum Took {
    /// Send it now.
    Send(EncodedUnit),
    /// A key unit kept until there is room ([`ViewerGate::take_held`]).
    Held,
    /// A delta the viewer can't take: it waits for a key unit and should
    /// ask the hub for one (the hub throttles catch-up asks per layer).
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
    /// Deltas dropped (the stats readout's skip count).
    pub skips: u64,
}

impl ViewerGate {
    pub fn new(layer: u8) -> Self {
        ViewerGate { layer, chain: None, held: None, offset: 0, rebase: false, last_wire: None, skips: 0 }
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
        self.chain = None;
        self.skips += 1;
        Took::Dropped
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
    /// once per sent unit, in order.
    pub fn wire_seq(&mut self, u: &EncodedUnit) -> u32 {
        let seq = u.seq as u32;
        if self.rebase {
            self.offset = self.last_wire.map_or(0, |w| w.wrapping_add(1)).wrapping_sub(seq);
            self.rebase = false;
        }
        let w = seq.wrapping_add(self.offset);
        self.last_wire = Some(w);
        w
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_core::stream::{Bytes, Codec};
    use std::time::Instant;

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
        assert!(matches!(g.offer(unit(0, 11, false), true), Took::Dropped), "a gap");
        assert!(matches!(g.offer(unit(0, 12, false), true), Took::Dropped), "still waiting");
        assert_eq!(sent(g.offer(unit(0, 13, true), true)), Some(13));
        assert_eq!(g.skips, 3);
    }

    #[test]
    fn no_room_drops_deltas_and_holds_the_newest_key() {
        let mut g = ViewerGate::new(0);
        assert_eq!(sent(g.offer(unit(0, 1, true), true)), Some(1));
        assert!(matches!(g.offer(unit(0, 2, false), false), Took::Dropped), "window full");
        assert!(matches!(g.offer(unit(0, 3, false), true), Took::Dropped), "the chain broke at 2");
        assert!(matches!(g.offer(unit(0, 4, true), false), Took::Held));
        assert!(matches!(g.offer(unit(0, 5, true), false), Took::Held), "a newer key replaces it");
        assert!(g.take_held(false).is_none());
        assert!(matches!(g.offer(unit(0, 6, false), true), Took::Dropped), "a held key goes first");
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
}
