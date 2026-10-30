//! Flow control for one viewer of the device stream.
//!
//! **[`AckWindow`]**: up to `max` frames in flight (the viewer acks each
//! seq after it draws it). Today's wire allowed one, so a remote viewer got
//! about one frame per round trip. The window lets the daemon keep the
//! pipe full, but only as full as the round trip needs: the limit is
//! `ceil(srtt × fps)`, clamped to 1..=cap, so a fast local viewer never
//! queues frames it could have had fresher. `cap` starts at max and drops
//! by one (per round trip) while acks show a queue on the link (srtt well
//! above the best round trip), then grows back once a second when the link
//! is clear: on a thin link more frames in flight only add delay. The
//! daemon always sends the newest frame when a slot frees (stale frames are
//! skipped, never queued).
//!
//! **[`RateCtl`]**: the H.264 bitrate. Target = [`crate::bitrate_for`];
//! when acks come back much slower than the best round trip seen (a queue
//! is building on the link) it steps down 25%, then climbs back 10% a
//! second while the link is clear.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// The most frames a viewer may have in flight.
pub const MAX_WINDOW: u32 = 3;

/// An unacked frame older than this is given up on, so a lost ack can
/// never stall the stream for good.
pub const ACK_EXPIRY: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct AckWindow {
    max: u32,
    /// The delay-based cap (1..=max).
    cap: u32,
    last_cap_change: Option<Instant>,
    inflight: VecDeque<(u32, Instant)>,
    srtt: Option<Duration>,
    min_rtt: Option<Duration>,
}

impl AckWindow {
    /// `max` is the negotiated window (1 = the old one-in-flight wire).
    pub fn new(max: u32) -> Self {
        let max = max.clamp(1, MAX_WINDOW);
        AckWindow { max, cap: max, last_cap_change: None, inflight: VecDeque::new(), srtt: None, min_rtt: None }
    }

    pub fn max(&self) -> u32 {
        self.max
    }

    /// The delay-based cap now (1..=max).
    pub fn cap(&self) -> u32 {
        self.cap
    }

    pub fn in_flight(&self) -> usize {
        self.inflight.len()
    }

    pub fn srtt(&self) -> Option<Duration> {
        self.srtt
    }

    pub fn min_rtt(&self) -> Option<Duration> {
        self.min_rtt
    }

    /// How many frames may be in flight at `fps`: enough to cover one
    /// round trip, never more than the cap. Before the first ack the whole
    /// window is open (the first frames are keyframes and the viewer wants
    /// them fast).
    pub fn limit(&self, fps: u32) -> u32 {
        match self.srtt {
            None => self.cap,
            Some(rtt) => {
                let need = (rtt.as_secs_f64() * fps.max(1) as f64).ceil() as u32;
                need.clamp(1, self.cap)
            }
        }
    }

    pub fn can_send(&self, fps: u32) -> bool {
        (self.inflight.len() as u32) < self.limit(fps)
    }

    /// Frame `seq` left (or was handed to the encoder, which reserves its
    /// slot).
    pub fn sent(&mut self, seq: u32, now: Instant) {
        self.inflight.push_back((seq, now));
    }

    /// The encoder dropped `seq`: free its slot without an RTT sample.
    pub fn cancel(&mut self, seq: u32) -> bool {
        match self.inflight.iter().position(|(s, _)| *s == seq) {
            Some(i) => {
                self.inflight.remove(i);
                true
            }
            None => false,
        }
    }

    /// The viewer acked `seq`. Acks come in order, so this also clears any
    /// older frame still listed (the viewer skipped it). Returns false for a
    /// seq that isn't in flight (late, duplicate, or from before a reset).
    pub fn ack(&mut self, seq: u32, now: Instant) -> bool {
        let Some(i) = self.inflight.iter().position(|(s, _)| *s == seq) else {
            return false;
        };
        let (_, sent) = self.inflight[i];
        self.inflight.drain(..=i);
        let rtt = now.saturating_duration_since(sent);
        self.srtt = Some(match self.srtt {
            None => rtt,
            // RFC 6298's 1/8 gain.
            Some(s) => (s * 7 + rtt) / 8,
        });
        self.min_rtt = Some(self.min_rtt.map_or(rtt, |m| m.min(rtt)));
        self.adjust_cap(now);
        true
    }

    /// Shrink the cap while a queue builds (at most once per round trip),
    /// grow it back once a second while the link is clear.
    fn adjust_cap(&mut self, now: Instant) {
        let (Some(s), Some(m)) = (self.srtt, self.min_rtt) else { return };
        let since = self.last_cap_change.map(|t| now.saturating_duration_since(t));
        let queued = s > m * 3 / 2 + Duration::from_millis(10);
        let clear = s < m * 6 / 5 + Duration::from_millis(5);
        if queued && self.cap > 1 && since.is_none_or(|d| d >= s) {
            self.cap -= 1;
            self.last_cap_change = Some(now);
        } else if clear && self.cap < self.max && since.is_none_or(|d| d >= Duration::from_secs(1)) {
            self.cap += 1;
            self.last_cap_change = Some(now);
        }
    }

    /// Give up on frames unacked for longer than `after`. Returns how many.
    pub fn expire(&mut self, now: Instant, after: Duration) -> usize {
        let before = self.inflight.len();
        self.inflight.retain(|(_, t)| now.saturating_duration_since(*t) < after);
        before - self.inflight.len()
    }

    /// Is a queue building on the link? (srtt well above the best seen.)
    pub fn congested(&self) -> bool {
        match (self.srtt, self.min_rtt) {
            (Some(s), Some(m)) => s > m * 2 + Duration::from_millis(40),
            _ => false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RateCtl {
    target: u32,
    floor: u32,
    current: u32,
    last_change: Option<Instant>,
}

impl RateCtl {
    pub const DOWN_EVERY: Duration = Duration::from_millis(500);
    pub const UP_EVERY: Duration = Duration::from_secs(1);

    pub fn new(target: u32) -> Self {
        RateCtl { target, floor: (target / 8).max(crate::MIN_BITRATE).min(target), current: target, last_change: None }
    }

    pub fn current(&self) -> u32 {
        self.current
    }

    pub fn target(&self) -> u32 {
        self.target
    }

    /// A new target (size or fps changed). Keeps any congestion cut.
    pub fn retarget(&mut self, target: u32) {
        let cut = self.current < self.target;
        self.target = target;
        self.floor = (target / 8).max(crate::MIN_BITRATE).min(target);
        self.current = if cut { self.current.clamp(self.floor, target) } else { target };
    }

    /// Feed the link state; returns the new bitrate when it changes.
    pub fn update(&mut self, congested: bool, now: Instant) -> Option<u32> {
        let since = self.last_change.map(|t| now.saturating_duration_since(t));
        let next = if congested {
            if since.is_some_and(|d| d < Self::DOWN_EVERY) {
                return None;
            }
            ((self.current as u64 * 3 / 4) as u32).max(self.floor)
        } else {
            if self.current >= self.target || since.is_some_and(|d| d < Self::UP_EVERY) {
                return None;
            }
            ((self.current as u64 * 11 / 10) as u32).min(self.target)
        };
        if next == self.current {
            return None;
        }
        self.current = next;
        self.last_change = Some(now);
        Some(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn window_one_is_the_old_wire() {
        let t = Instant::now();
        let mut w = AckWindow::new(1);
        assert!(w.can_send(30));
        w.sent(1, t);
        assert!(!w.can_send(30), "one in flight");
        assert!(w.ack(1, t + ms(80)));
        assert!(w.can_send(30));
        assert_eq!(AckWindow::new(0).max(), 1);
        assert_eq!(AckWindow::new(9).max(), MAX_WINDOW);
    }

    #[test]
    fn window_opens_to_cover_the_round_trip_and_no_further() {
        let t = Instant::now();
        let mut w = AckWindow::new(3);
        assert_eq!(w.limit(30), 3, "fully open before the first sample");
        // A local viewer: 5 ms RTT at 30 fps needs one frame in flight.
        w.sent(1, t);
        w.ack(1, t + ms(5));
        assert_eq!(w.limit(30), 1);
        // A remote viewer: 80 ms at 24 fps needs ceil(1.92) = 2.
        let mut r = AckWindow::new(3);
        r.sent(1, t);
        r.ack(1, t + ms(80));
        assert_eq!(r.limit(24), 2);
        // 150 ms at 30 fps would need 5; capped at the window.
        let mut s = AckWindow::new(3);
        s.sent(1, t);
        s.ack(1, t + ms(150));
        assert_eq!(s.limit(30), 3);
        s.sent(2, t);
        s.sent(3, t);
        s.sent(4, t);
        assert!(!s.can_send(30));
    }

    #[test]
    fn acks_are_cumulative_and_unknown_ones_are_ignored() {
        let t = Instant::now();
        let mut w = AckWindow::new(3);
        w.sent(10, t);
        w.sent(11, t);
        w.sent(12, t);
        assert!(!w.ack(9, t), "not in flight");
        assert!(w.ack(11, t + ms(30)), "acks 10 too");
        assert_eq!(w.in_flight(), 1);
        assert!(!w.ack(10, t + ms(31)), "already cleared");
        assert!(w.cancel(12));
        assert!(!w.cancel(12));
        assert_eq!(w.in_flight(), 0);
        assert_eq!(w.srtt(), Some(ms(30)));
    }

    #[test]
    fn a_lost_ack_expires_instead_of_stalling() {
        let t = Instant::now();
        let mut w = AckWindow::new(1);
        w.sent(1, t);
        assert_eq!(w.expire(t + ms(100), ACK_EXPIRY), 0);
        assert_eq!(w.expire(t + ACK_EXPIRY + ms(1), ACK_EXPIRY), 1);
        assert!(w.can_send(30));
    }

    #[test]
    fn a_queue_on_the_link_shrinks_the_window_and_a_clear_link_grows_it() {
        let t = Instant::now();
        let mut w = AckWindow::new(3);
        let mut seq = 0u32;
        let mut now = t;
        let mut step = |w: &mut AckWindow, rtt: u64, now: &mut Instant| {
            seq += 1;
            w.sent(seq, *now);
            *now += ms(rtt);
            w.ack(seq, *now);
        };
        step(&mut w, 100, &mut now);
        assert_eq!(w.cap(), 3);
        // Frames now take 400 ms to come back: a queue. One cut per round
        // trip, down to 1.
        for _ in 0..30 {
            step(&mut w, 400, &mut now);
        }
        assert_eq!(w.cap(), 1, "srtt {:?} min {:?}", w.srtt(), w.min_rtt());
        assert_eq!(w.limit(30), 1);
        // The link clears: back up, one a second.
        for _ in 0..40 {
            step(&mut w, 100, &mut now);
        }
        assert_eq!(w.cap(), 3);
        // A steady local link never cuts.
        let mut l = AckWindow::new(3);
        for i in 0..200 {
            step(&mut l, 4 + (i % 5), &mut now);
        }
        assert_eq!(l.cap(), 3);
    }

    #[test]
    fn congestion_is_a_round_trip_well_above_the_best() {
        let t = Instant::now();
        let mut w = AckWindow::new(3);
        w.sent(1, t);
        w.ack(1, t + ms(40));
        assert!(!w.congested());
        for i in 2..40 {
            w.sent(i, t);
            w.ack(i, t + ms(400));
        }
        assert!(w.congested(), "srtt {:?} min {:?}", w.srtt(), w.min_rtt());
    }

    #[test]
    fn rate_steps_down_on_congestion_and_climbs_back() {
        let t = Instant::now();
        let mut r = RateCtl::new(4_000_000);
        assert_eq!(r.update(false, t), None, "already at target");
        assert_eq!(r.update(true, t), Some(3_000_000));
        assert_eq!(r.update(true, t + ms(100)), None, "at most one cut per 500 ms");
        assert_eq!(r.update(true, t + ms(600)), Some(2_250_000));
        assert_eq!(r.update(false, t + ms(900)), None, "climbs once a second");
        assert_eq!(r.update(false, t + ms(1700)), Some(2_475_000));
        // Never below the floor.
        let mut low = RateCtl::new(400_000);
        for i in 0..20 {
            low.update(true, t + ms(1000 * i));
        }
        assert_eq!(low.current(), crate::MIN_BITRATE);
        // A new target keeps the cut, clamped to it.
        r.retarget(8_000_000);
        assert_eq!(r.current(), 2_475_000, "still cut after a retarget");
        r.retarget(1_000_000);
        assert_eq!(r.current(), 1_000_000);
        let mut fresh = RateCtl::new(1_000_000);
        fresh.retarget(2_000_000);
        assert_eq!(fresh.current(), 2_000_000, "uncut follows the target");
    }
}
