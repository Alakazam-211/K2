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
//! **[`Pacer`]**: the viewer's frame schedule, the only pacing point
//! between the device and the viewer (fix 2). Sends are due every `gap`;
//! each send moves the schedule on by one gap from when it was due (not
//! from when it went), so a 60 Hz source and a 24 fps viewer don't beat.
//! A frame that arrives up to a quarter of a gap before it is due goes at
//! once; an older one waits until a quarter gap after the due time for a
//! fresher frame, then goes. More than a gap behind (a still screen, a
//! stall), the schedule restarts from now instead of bursting to catch up.
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

#[derive(Debug, Clone)]
pub struct Pacer {
    gap: Duration,
    next_due: Option<Instant>,
    last_sent: Option<Instant>,
}

impl Pacer {
    pub fn new(fps: u32) -> Self {
        Pacer { gap: Self::gap_for(fps), next_due: None, last_sent: None }
    }

    fn gap_for(fps: u32) -> Duration {
        Duration::from_secs_f64(1.0 / fps.max(1) as f64)
    }

    pub fn gap(&self) -> Duration {
        self.gap
    }

    fn slack(&self) -> Duration {
        self.gap / 4
    }

    /// A new rate: the next frame is due one new gap after the last send.
    pub fn set_fps(&mut self, fps: u32) {
        self.gap = Self::gap_for(fps);
        self.next_due = self.last_sent.map(|t| t + self.gap);
    }

    /// May the newest frame, which arrived at `arrived`, go at `now`?
    pub fn ready(&self, arrived: Instant, now: Instant) -> bool {
        match self.next_due {
            None => true,
            Some(due) => arrived + self.slack() >= due || now >= due + self.slack(),
        }
    }

    /// How long until that frame may go (zero when it may now).
    pub fn wait(&self, arrived: Instant, now: Instant) -> Duration {
        match self.next_due {
            Some(due) if !self.ready(arrived, now) => (due + self.slack()).saturating_duration_since(now),
            _ => Duration::ZERO,
        }
    }

    /// A frame went at `now`.
    pub fn sent(&mut self, now: Instant) {
        let base = match self.next_due {
            Some(due) if now <= due + self.gap => due,
            _ => now,
        };
        self.next_due = Some(base + self.gap);
        self.last_sent = Some(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// Drive a pacer as a viewer does: frames arrive at `arrivals`; the
    /// newest pending one goes when the pacer allows, at an arrival or when
    /// its wait ends (an arrival at the same instant comes first). Ends at
    /// the last arrival. Returns (frame index, send time).
    fn drive(p: &mut Pacer, arrivals: &[Instant]) -> Vec<(usize, Instant)> {
        let mut out = Vec::new();
        let mut pending: Option<(usize, Instant)> = None;
        let mut now = arrivals[0];
        let mut i = 0;
        loop {
            let wake = pending.map(|(_, a)| now + p.wait(a, now));
            match (arrivals.get(i), wake) {
                (None, _) => return out,
                (Some(&t), Some(w)) if w < t => now = w,
                (Some(&t), _) => {
                    now = t;
                    pending = Some((i, t));
                    i += 1;
                }
            }
            if let Some((j, a)) = pending {
                if p.ready(a, now) {
                    p.sent(now);
                    out.push((j, now));
                    pending = None;
                }
            }
        }
    }

    fn source(start: Instant, hz_period: Duration, n: usize, jitter_ms: &[i64]) -> Vec<Instant> {
        (0..n)
            .map(|k| {
                let t = start + ms(100) + hz_period * k as u32;
                let j = jitter_ms.get(k % jitter_ms.len().max(1)).copied().unwrap_or(0);
                if j >= 0 { t + ms(j as u64) } else { t - ms((-j) as u64) }
            })
            .collect()
    }

    const HZ60: Duration = Duration::from_micros(16_667);

    #[test]
    fn pacer_at_30_on_a_60_hz_source_sends_every_second_frame() {
        let t0 = Instant::now();
        let mut p = Pacer::new(30);
        let sent = drive(&mut p, &source(t0, HZ60, 60, &[]));
        let idx: Vec<usize> = sent.iter().map(|(j, _)| *j).collect();
        assert_eq!(idx, (0..60).step_by(2).collect::<Vec<_>>(), "every second frame, each the fresh one");
    }

    #[test]
    fn pacer_at_24_on_60_hz_averages_24_over_ten_seconds() {
        let t0 = Instant::now();
        let mut p = Pacer::new(24);
        let sent = drive(&mut p, &source(t0, HZ60, 600, &[]));
        assert!((239..=241).contains(&sent.len()), "{} frames in 10 s", sent.len());
        // Never two frames closer than three quarters of a gap.
        assert!(sent.windows(2).all(|w| w[1].1 - w[0].1 >= p.gap() * 3 / 4), "no bunching");
    }

    #[test]
    fn pacer_at_60_on_60_hz_sends_every_frame_through_2_ms_of_jitter() {
        let t0 = Instant::now();
        let mut p = Pacer::new(60);
        let arrivals = source(t0, HZ60, 300, &[0, 2, -2, 1, -1, 2, 0, -2]);
        let sent = drive(&mut p, &arrivals);
        assert_eq!(sent.len(), 300, "every frame");
        assert!(sent.iter().zip(&arrivals).all(|((_, at), a)| at == a), "each the moment it arrived");
    }

    #[test]
    fn pacer_restarts_after_a_stall_instead_of_bursting() {
        let t0 = Instant::now();
        let mut p = Pacer::new(30);
        let mut arrivals = source(t0, HZ60, 30, &[]);
        let back = *arrivals.last().unwrap() + Duration::from_secs(1);
        arrivals.extend((0..30).map(|k| back + HZ60 * k));
        let sent = drive(&mut p, &arrivals);
        assert!(sent.windows(2).all(|w| w[1].1 - w[0].1 >= p.gap() * 3 / 4), "no burst after the stall: {sent:?}");
        let after: Vec<&(usize, Instant)> = sent.iter().filter(|(_, t)| *t >= back).collect();
        assert_eq!(after[0].1, back, "the first frame after the stall goes at once");
        assert!((14..=16).contains(&after.len()), "then 30 fps again: {}", after.len());
    }

    #[test]
    fn pacer_holds_an_early_frame_for_a_fresher_one_then_sends_it() {
        let t0 = Instant::now();
        let mut p = Pacer::new(30);
        p.sent(t0);
        let early = t0 + ms(10);
        assert!(!p.ready(early, early), "well before it is due");
        assert_eq!(p.wait(early, early), (t0 + p.gap() + p.gap() / 4) - early);
        assert!(p.ready(early, t0 + p.gap() + p.gap() / 4), "still goes, a quarter gap late at most");
        assert!(p.ready(t0 + ms(26), t0 + ms(26)), "a quarter gap early goes at once");
        p.set_fps(2);
        assert!(!p.ready(t0 + ms(100), t0 + ms(100)));
        p.set_fps(60);
        assert!(p.ready(t0 + ms(17), t0 + ms(17)), "a faster rate is due one new gap after the last send");
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
