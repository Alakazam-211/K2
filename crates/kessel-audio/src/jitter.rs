//! Per-participant jitter buffer.
//!
//! Packets are kept in a ring indexed by sequence number and played one per
//! 20 ms tick. The buffer tells the decoder what to do each tick ([`Next`]):
//! decode a packet, rebuild a lost one from the next packet's FEC, conceal,
//! or continue the sender's DTX silence.
//!
//! - **Delay:** the first packet of a talkspurt is held for the target delay,
//!   which follows the spread of packet transit times (95th percentile minus
//!   minimum over the last ~2.5 s), clamped to `[min_delay, max_delay]`.
//! - **Loss:** a missing packet with later packets already here is lost:
//!   FEC from the next packet when it's here, else concealment.
//! - **Empty buffer:** the tick is concealed and the playout clock stops. The
//!   next packet decides what the gap was. The next sequence number with a
//!   later timestamp means the sender was in DTX: the gap counts as DTX and
//!   the new talkspurt is held for the target delay. Anything else means a
//!   stall or loss: the gap counts as concealment and playout resumes at
//!   once.
//! - **Too much delay** (after a stall, or when the target falls): one
//!   packet is skipped every few ticks until the delay is near the target.

use crate::frame::FRAME_SAMPLES;
use crate::stats::ParticipantStats;
use bytes::Bytes;
use std::time::{Duration, Instant};

/// Jitter-buffer settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JitterConfig {
    /// The least delay the buffer adds, milliseconds.
    pub min_delay_ms: u32,
    /// The most delay the buffer aims for, milliseconds. Stalls can push the
    /// delay higher for a while; it is trimmed back afterwards.
    pub max_delay_ms: u32,
    /// Ring size in 20 ms packets. Packets further ahead than this make the
    /// buffer resynchronise.
    pub capacity_frames: usize,
}

impl Default for JitterConfig {
    /// 40 to 200 ms, 64 packets (1.28 s).
    fn default() -> Self {
        JitterConfig {
            min_delay_ms: 40,
            max_delay_ms: 200,
            capacity_frames: 64,
        }
    }
}

/// What to play for one 20 ms tick.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Next {
    /// Nothing has been heard yet: play silence, leave the decoder alone.
    Silence,
    /// Decode this packet.
    Decode(Bytes),
    /// The packet for this tick is lost; this is the next packet, which may
    /// carry its FEC. Don't consume it.
    Fec(Bytes),
    /// Conceal (packet loss concealment).
    Conceal,
    /// The sender is in DTX: continue its comfort noise / silence.
    Dtx,
}

const HALF_FRAME: i64 = FRAME_SAMPLES as i64 / 2;
const FRAME: i64 = FRAME_SAMPLES as i64;
const TRANSITS: usize = 128;
const HINTS: usize = 16;
const HISTORY: usize = 256;
/// After this many starved ticks (1 s) play silence instead of concealment.
const STARVE_CONCEAL_TICKS: u32 = 50;
/// How many ticks to stretch while waiting for one missing packet.
const MAX_WAIT_TICKS: u32 = 3;

#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    Idle,
    Prebuffer { until: Instant, had_audio: bool },
    Playing,
    Starved { ticks: u32 },
}

struct Slot {
    seq: i64,
    ts: i64,
    payload: Bytes,
}

fn extend16(reference: i64, seq: u16) -> i64 {
    reference + seq.wrapping_sub(reference as u16) as i16 as i64
}

fn extend32(reference: i64, ts: u32) -> i64 {
    reference + ts.wrapping_sub(reference as u32) as i32 as i64
}

#[derive(Default)]
struct Counters {
    received: u64,
    late: u64,
    duplicate: u64,
    decoded: u64,
    fec: u64,
    concealed: u64,
    dtx: u64,
    dropped: u64,
}

pub(crate) struct JitterBuffer {
    cfg: JitterConfig,
    slots: Vec<Option<Slot>>,
    buffered: usize,
    state: State,
    next_seq: i64,
    play_ts: i64,
    highest: Option<(i64, i64)>,
    base_seq: i64,
    // Arrival-time bookkeeping.
    t0: Option<Instant>,
    last_transit: Option<f64>,
    jitter_s: f64,
    transits: [f64; TRANSITS],
    transit_n: usize,
    transit_i: usize,
    sort_scratch: [f64; TRANSITS],
    target_ms: f32,
    // Duplicate detection over the last HISTORY sequence numbers.
    seen: [i64; HISTORY],
    hints: [i64; HINTS],
    hint_i: usize,
    drop_cooldown: u32,
    waited: u32,
    pending: u32,
    conceal_run: u32,
    longest_conceal_run: u32,
    delay_ema_ms: f32,
    n: Counters,
}

impl JitterBuffer {
    pub(crate) fn new(cfg: JitterConfig) -> Self {
        let cap = cfg.capacity_frames.max(8);
        let mut slots = Vec::with_capacity(cap);
        slots.resize_with(cap, || None);
        let target = cfg.min_delay_ms as f32;
        JitterBuffer {
            cfg: JitterConfig {
                capacity_frames: cap,
                ..cfg
            },
            slots,
            buffered: 0,
            state: State::Idle,
            next_seq: 0,
            play_ts: 0,
            highest: None,
            base_seq: 0,
            t0: None,
            last_transit: None,
            jitter_s: 0.0,
            transits: [0.0; TRANSITS],
            transit_n: 0,
            transit_i: 0,
            sort_scratch: [0.0; TRANSITS],
            target_ms: target,
            seen: [i64::MIN; HISTORY],
            hints: [i64::MIN; HINTS],
            hint_i: 0,
            drop_cooldown: 0,
            waited: 0,
            pending: 0,
            conceal_run: 0,
            longest_conceal_run: 0,
            delay_ema_ms: 0.0,
            n: Counters::default(),
        }
    }

    fn idx(&self, seq: i64) -> usize {
        seq.rem_euclid(self.slots.len() as i64) as usize
    }

    fn slot_has(&self, seq: i64) -> bool {
        matches!(&self.slots[self.idx(seq)], Some(s) if s.seq == seq)
    }

    fn target(&self) -> Duration {
        Duration::from_micros((self.target_ms * 1000.0) as u64)
    }

    fn clear_slots(&mut self) {
        for s in self.slots.iter_mut() {
            *s = None;
        }
        self.buffered = 0;
    }

    /// The transport says this packet is gone.
    pub(crate) fn hint_lost(&mut self, seq: u16) {
        let reference = match self.highest {
            Some((h, _)) => h,
            None => return,
        };
        let ext = extend16(reference, seq);
        self.hints[self.hint_i] = ext;
        self.hint_i = (self.hint_i + 1) % HINTS;
    }

    fn hinted(&self, seq: i64) -> bool {
        self.hints.contains(&seq)
    }

    /// Add a received packet.
    pub(crate) fn insert(&mut self, seq: u16, ts: u32, payload: Bytes, arrival: Instant) {
        let (ext_seq, ext_ts) = match self.highest {
            None => (seq as i64, ts as i64),
            Some((hs, hts)) => (extend16(hs, seq), extend32(hts, ts)),
        };

        // Duplicates (of a buffered or an already played packet).
        let h = ext_seq.rem_euclid(HISTORY as i64) as usize;
        if self.seen[h] == ext_seq {
            self.n.duplicate += 1;
            return;
        }
        self.seen[h] = ext_seq;
        self.n.received += 1;

        // RFC 3550 interarrival jitter, and the transit window for the target.
        let t0 = *self.t0.get_or_insert(arrival);
        let transit = arrival.saturating_duration_since(t0).as_secs_f64()
            - ext_ts as f64 / crate::frame::SAMPLE_RATE as f64;
        if let Some(lt) = self.last_transit {
            self.jitter_s += ((transit - lt).abs() - self.jitter_s) / 16.0;
        }
        self.last_transit = Some(transit);
        self.transits[self.transit_i] = transit;
        self.transit_i = (self.transit_i + 1) % TRANSITS;
        self.transit_n = (self.transit_n + 1).min(TRANSITS);
        self.recompute_target();

        if self.highest.is_none_or(|(hs, _)| ext_seq > hs) {
            self.highest = Some((ext_seq, ext_ts));
        }

        match self.state {
            State::Idle => {
                self.base_seq = ext_seq;
                self.next_seq = ext_seq;
                self.play_ts = ext_ts;
                self.state = State::Prebuffer {
                    until: arrival + self.target(),
                    had_audio: false,
                };
            }
            State::Prebuffer { .. } => {
                if ext_seq < self.next_seq {
                    self.next_seq = ext_seq;
                    self.play_ts = ext_ts;
                }
                if ext_seq < self.base_seq {
                    self.base_seq = ext_seq;
                }
            }
            State::Playing => {
                if ext_seq < self.next_seq {
                    self.n.late += 1;
                    return;
                }
            }
            State::Starved { .. } => {
                if ext_seq < self.next_seq {
                    self.n.late += 1;
                    return;
                }
                if ext_seq == self.next_seq && ext_ts > self.play_ts + HALF_FRAME {
                    // The sender stopped sending (DTX) and resumed.
                    self.n.dtx += self.pending as u64;
                    self.pending = 0;
                    self.next_seq = ext_seq;
                    self.play_ts = ext_ts;
                    self.state = State::Prebuffer {
                        until: arrival + self.target(),
                        had_audio: true,
                    };
                } else {
                    // A stall or a loss: the gap was concealment.
                    self.count_concealed(self.pending);
                    self.pending = 0;
                    self.state = State::Playing;
                }
            }
        }

        // Too far ahead for the ring: start over from this packet.
        if ext_seq - self.next_seq >= self.slots.len() as i64 {
            self.clear_slots();
            let had_audio = self.n.decoded > 0;
            self.next_seq = ext_seq;
            self.play_ts = ext_ts;
            self.state = State::Prebuffer {
                until: arrival + self.target(),
                had_audio,
            };
        }

        let i = self.idx(ext_seq);
        if self.slots[i].is_none() {
            self.buffered += 1;
        }
        self.slots[i] = Some(Slot {
            seq: ext_seq,
            ts: ext_ts,
            payload,
        });
    }

    fn recompute_target(&mut self) {
        let n = self.transit_n;
        if n < 2 {
            return;
        }
        let s = &mut self.sort_scratch[..n];
        s.copy_from_slice(&self.transits[..n]);
        s.sort_unstable_by(|a, b| a.total_cmp(b));
        let spread_ms = ((s[(n - 1) * 95 / 100] - s[0]) * 1000.0) as f32;
        self.target_ms = (spread_ms + 10.0).clamp(
            self.cfg.min_delay_ms as f32,
            self.cfg.max_delay_ms.max(self.cfg.min_delay_ms) as f32,
        );
    }

    /// What to play for the next 20 ms.
    pub(crate) fn next(&mut self, now: Instant) -> Next {
        match self.state {
            State::Idle => Next::Silence,
            State::Prebuffer { until, had_audio } => {
                if now >= until {
                    self.state = State::Playing;
                    self.play()
                } else if had_audio {
                    self.n.dtx += 1;
                    Next::Dtx
                } else {
                    Next::Silence
                }
            }
            State::Playing => self.play(),
            State::Starved { ticks } => {
                let ticks = ticks + 1;
                self.state = State::Starved { ticks };
                if let Some(n) = self.take_hinted_loss() {
                    return n;
                }
                self.pending += 1;
                if ticks > STARVE_CONCEAL_TICKS {
                    Next::Silence
                } else {
                    Next::Conceal
                }
            }
        }
    }

    /// A transport loss hint for the packet we're waiting on resolves an
    /// empty buffer as concealment right away.
    fn take_hinted_loss(&mut self) -> Option<Next> {
        if !self.hinted(self.next_seq) {
            return None;
        }
        self.count_concealed(self.pending + 1);
        self.pending = 0;
        self.waited = 0;
        self.next_seq += 1;
        self.play_ts += FRAME;
        self.state = State::Playing;
        Some(Next::Conceal)
    }

    fn play(&mut self) -> Next {
        self.trim_delay();
        let i = self.idx(self.next_seq);
        let here = matches!(&self.slots[i], Some(s) if s.seq == self.next_seq);
        if here {
            let ts = self.slots[i].as_ref().map(|s| s.ts).unwrap_or(self.play_ts);
            if ts > self.play_ts + HALF_FRAME {
                // A DTX gap inside the buffer: play the gap first.
                self.play_ts += FRAME;
                self.n.dtx += 1;
                self.conceal_run = 0;
                return Next::Dtx;
            }
            if let Some(slot) = self.slots[i].take() {
                self.buffered -= 1;
                self.next_seq += 1;
                self.play_ts = slot.ts + FRAME;
                self.n.decoded += 1;
                self.conceal_run = 0;
                self.waited = 0;
                return Next::Decode(slot.payload);
            }
        }
        let hinted = self.hinted(self.next_seq);
        if self.buffered > 0 || hinted {
            if !hinted && self.waited < MAX_WAIT_TICKS && self.depth_ms() < self.target_ms - 10.0 {
                // Below the target delay: stretch (conceal without moving on)
                // and give the missing packet another tick to arrive.
                self.waited += 1;
                self.count_concealed(1);
                return Next::Conceal;
            }
            self.waited = 0;
            // Lost: later packets are here (or the transport said so).
            let following = self.next_seq + 1;
            let fec = match &self.slots[self.idx(following)] {
                Some(s) if s.seq == following && s.ts - self.play_ts <= FRAME + HALF_FRAME => {
                    Some(s.payload.clone())
                }
                _ => None,
            };
            self.next_seq += 1;
            self.play_ts += FRAME;
            return match fec {
                Some(p) => {
                    self.n.fec += 1;
                    self.conceal_run = 0;
                    Next::Fec(p)
                }
                None => {
                    self.count_concealed(1);
                    Next::Conceal
                }
            };
        }
        self.state = State::Starved { ticks: 1 };
        self.pending = 1;
        Next::Conceal
    }

    /// The packet handed out as [`Next::Fec`] had no FEC after all; the
    /// frame was concealed instead.
    pub(crate) fn fec_unavailable(&mut self) {
        self.n.fec = self.n.fec.saturating_sub(1);
        self.count_concealed(1);
    }

    fn count_concealed(&mut self, frames: u32) {
        self.n.concealed += frames as u64;
        self.conceal_run += frames;
        self.longest_conceal_run = self.longest_conceal_run.max(self.conceal_run);
    }

    /// Audio buffered ahead of the playout point (including the frame due
    /// now), milliseconds.
    fn depth_ms(&self) -> f32 {
        match self.highest {
            Some((_, hts)) => {
                (hts + FRAME - self.play_ts) as f32 * 1000.0 / crate::frame::SAMPLE_RATE as f32
            }
            None => 0.0,
        }
    }

    /// Skip packets while the buffered audio is well above the target.
    fn trim_delay(&mut self) {
        if self.highest.is_none() {
            return;
        }
        let depth_ms = self.depth_ms();
        self.delay_ema_ms = 0.95 * self.delay_ema_ms + 0.05 * depth_ms.max(0.0);
        if self.drop_cooldown > 0 {
            self.drop_cooldown -= 1;
            return;
        }
        let excess = depth_ms - self.target_ms;
        if excess > 60.0 && self.slot_has(self.next_seq) && self.slot_has(self.next_seq + 1) {
            let i = self.idx(self.next_seq);
            if let Some(slot) = self.slots[i].take() {
                self.buffered -= 1;
                self.next_seq += 1;
                self.play_ts = slot.ts + FRAME;
                self.n.dropped += 1;
                self.drop_cooldown = if excess > 200.0 { 0 } else { 4 };
            }
        }
    }

    pub(crate) fn stats(&self) -> ParticipantStats {
        let expected = match self.highest {
            Some((h, _)) => (h - self.base_seq + 1).max(0) as u64,
            None => 0,
        };
        let lost = expected.saturating_sub(self.n.received);
        let played = self.n.decoded + self.n.fec + self.n.concealed;
        let pct = |a: u64, b: u64| if b == 0 { 0.0 } else { (a as f64 * 100.0 / b as f64) as f32 };
        ParticipantStats {
            jitter_ms: (self.jitter_s * 1000.0) as f32,
            loss_pct: pct(lost, expected),
            concealed_pct: pct(self.n.concealed, played),
            jb_delay_ms: self.delay_ema_ms,
            jb_target_ms: self.target_ms,
            longest_concealment_ms: self.longest_conceal_run as f32 * crate::frame::FRAME_MS as f32,
            packets_received: self.n.received,
            packets_expected: expected,
            packets_late: self.n.late,
            packets_duplicate: self.n.duplicate,
            frames_decoded: self.n.decoded,
            frames_fec: self.n.fec,
            frames_concealed: self.n.concealed,
            frames_dtx: self.n.dtx,
            frames_dropped: self.n.dropped,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TICK: Duration = Duration::from_millis(20);

    struct Sim {
        jb: JitterBuffer,
        t0: Instant,
        tick: u32,
        out: Vec<Next>,
    }

    fn pkt(seq: u16) -> Bytes {
        Bytes::copy_from_slice(&seq.to_be_bytes())
    }

    fn seq_of(b: &Bytes) -> u16 {
        u16::from_be_bytes([b[0], b[1]])
    }

    impl Sim {
        fn new() -> Self {
            Sim {
                jb: JitterBuffer::new(JitterConfig::default()),
                t0: Instant::now(),
                tick: 0,
                out: Vec::new(),
            }
        }
        fn at(&self, ms: u64) -> Instant {
            self.t0 + Duration::from_millis(ms)
        }
        /// Run ticks up to (not including) time `ms`.
        fn run_until(&mut self, ms: u64) {
            while (self.tick as u64) * 20 < ms {
                let now = self.t0 + TICK * self.tick;
                let n = self.jb.next(now);
                self.out.push(n);
                self.tick += 1;
            }
        }
        fn send(&mut self, seq: u16, ts: u32, arrive_ms: u64) {
            self.run_until(arrive_ms);
            self.jb.insert(seq, ts, pkt(seq), self.at(arrive_ms));
        }
        fn decoded(&self) -> Vec<u16> {
            self.out
                .iter()
                .filter_map(|n| match n {
                    Next::Decode(b) => Some(seq_of(b)),
                    _ => None,
                })
                .collect()
        }
        fn count(&self, f: impl Fn(&Next) -> bool) -> usize {
            self.out.iter().filter(|n| f(n)).count()
        }
    }

    #[test]
    fn plays_in_order_after_the_target_delay() {
        let mut s = Sim::new();
        for i in 0..100u16 {
            s.send(i, i as u32 * 960, i as u64 * 20 + 5);
        }
        s.run_until(100 * 20 + 200);
        assert_eq!(s.decoded(), (0..100).collect::<Vec<_>>());
        // First decode happens no earlier than arrival + min delay (40 ms).
        let first = s.out.iter().position(|n| matches!(n, Next::Decode(_))).unwrap();
        assert!(first as u64 * 20 >= 45, "first decode at tick {first}");
        let st = s.jb.stats();
        assert_eq!(st.loss_pct, 0.0);
        assert_eq!(st.frames_concealed, 0);
        assert_eq!(st.packets_received, 100);
        assert!(st.jb_delay_ms > 20.0 && st.jb_delay_ms < 80.0, "delay {}", st.jb_delay_ms);
    }

    #[test]
    fn a_lost_packet_uses_the_next_packets_fec() {
        let mut s = Sim::new();
        for i in 0..50u16 {
            if i == 10 {
                continue;
            }
            s.send(i, i as u32 * 960, i as u64 * 20 + 5);
        }
        s.run_until(50 * 20 + 200);
        let fec: Vec<u16> = s
            .out
            .iter()
            .filter_map(|n| match n {
                Next::Fec(b) => Some(seq_of(b)),
                _ => None,
            })
            .collect();
        assert_eq!(fec, vec![11], "FEC must come from packet 11");
        let d = s.decoded();
        assert_eq!(d.len(), 49);
        assert!(!d.contains(&10));
        let st = s.jb.stats();
        assert_eq!(st.packets_expected, 50);
        assert!((st.loss_pct - 2.0).abs() < 1e-3, "loss {}", st.loss_pct);
        assert_eq!(st.frames_fec, 1);
        assert_eq!(st.frames_concealed, 0);
    }

    #[test]
    fn two_lost_in_a_row_conceal_then_fec() {
        let mut s = Sim::new();
        for i in 0..40u16 {
            if i == 10 || i == 11 {
                continue;
            }
            s.send(i, i as u32 * 960, i as u64 * 20 + 5);
        }
        s.run_until(40 * 20 + 200);
        let st = s.jb.stats();
        assert_eq!(st.frames_concealed, 1, "10 has no FEC source");
        assert_eq!(st.frames_fec, 1, "11 comes from 12's FEC");
        assert_eq!(s.decoded().len(), 38);
    }

    #[test]
    fn reordered_packets_play_in_order() {
        let mut s = Sim::new();
        for i in 0..60u16 {
            // Swap the arrival of each pair (10,11), (20,21), ...
            let arrive = if i % 10 == 0 && i > 0 {
                (i as u64 + 1) * 20 + 8
            } else {
                i as u64 * 20 + 5
            };
            s.send(i, i as u32 * 960, arrive);
        }
        s.run_until(60 * 20 + 200);
        assert_eq!(s.decoded(), (0..60).collect::<Vec<_>>());
        assert_eq!(s.jb.stats().frames_concealed, 0);
        assert_eq!(s.jb.stats().packets_late, 0);
    }

    #[test]
    fn duplicates_are_dropped() {
        let mut s = Sim::new();
        for i in 0..30u16 {
            s.send(i, i as u32 * 960, i as u64 * 20 + 5);
            if i == 7 {
                s.send(i, i as u32 * 960, i as u64 * 20 + 6);
            }
        }
        // A duplicate of an already played packet, much later.
        s.send(3, 3 * 960, 30 * 20);
        s.run_until(30 * 20 + 200);
        assert_eq!(s.decoded(), (0..30).collect::<Vec<_>>());
        let st = s.jb.stats();
        assert_eq!(st.packets_duplicate, 2);
        assert_eq!(st.packets_received, 30);
        assert_eq!(st.loss_pct, 0.0);
    }

    #[test]
    fn a_late_packet_is_concealed_and_counted_late() {
        let mut s = Sim::new();
        for i in 0..50u16 {
            let arrive = if i == 20 { 20 * 20 + 300 } else { i as u64 * 20 + 5 };
            if i != 20 {
                s.send(i, i as u32 * 960, arrive);
            }
            if i == 35 {
                s.send(20, 20 * 960, 20 * 20 + 300);
            }
        }
        s.run_until(50 * 20 + 200);
        let st = s.jb.stats();
        assert_eq!(st.packets_late, 1);
        assert_eq!(st.loss_pct, 0.0, "a late packet still arrived");
        assert_eq!(st.frames_fec + st.frames_concealed, 1);
        assert!(!s.decoded().contains(&20));
    }

    #[test]
    fn a_dtx_gap_is_not_concealment() {
        let mut s = Sim::new();
        for i in 0..50u16 {
            s.send(i, i as u32 * 960, i as u64 * 20 + 5);
        }
        // 600 ms of DTX: the next packet has the next seq and a ts 30 frames on.
        for k in 0..50u16 {
            let i = 50 + k;
            let frame = 80 + k as u32;
            s.send(i, frame * 960, frame as u64 * 20 + 5);
        }
        s.run_until(130 * 20 + 300);
        assert_eq!(s.decoded(), (0..100).collect::<Vec<_>>());
        let st = s.jb.stats();
        assert_eq!(st.frames_concealed, 0, "{st:?}");
        assert_eq!(st.loss_pct, 0.0);
        // The gap (30 frames) plus the re-buffer before the new talkspurt.
        assert!(st.frames_dtx >= 30 && st.frames_dtx <= 34, "{st:?}");
    }

    #[test]
    fn a_stall_is_concealment_and_the_delay_recovers() {
        let mut s = Sim::new();
        for i in 0..400u16 {
            // Packets 100..=114 are held 300 ms by the network, then burst.
            let arrive = if (100..115).contains(&i) {
                115 * 20 + 5
            } else {
                i as u64 * 20 + 5
            };
            s.send(i, i as u32 * 960, arrive);
        }
        s.run_until(400 * 20 + 200);
        let st = s.jb.stats();
        assert_eq!(st.loss_pct, 0.0);
        assert!(st.frames_concealed >= 10, "{st:?}");
        assert!(st.frames_dropped >= 5, "the delay must be trimmed back: {st:?}");
        assert!(st.longest_concealment_ms >= 200.0, "{st:?}");
        assert!(st.jb_delay_ms < st.jb_target_ms + 70.0, "{st:?}");
    }

    #[test]
    fn sequence_and_timestamp_wrap() {
        let mut s = Sim::new();
        let seq0 = 65_500u16;
        let ts0 = u32::MAX - 960 * 20;
        for k in 0..100u32 {
            s.send(
                seq0.wrapping_add(k as u16),
                ts0.wrapping_add(k * 960),
                k as u64 * 20 + 5,
            );
        }
        s.run_until(100 * 20 + 200);
        let d = s.decoded();
        assert_eq!(d.len(), 100);
        assert_eq!(d[0], seq0);
        assert_eq!(d[99], seq0.wrapping_add(99));
        let st = s.jb.stats();
        assert_eq!(st.packets_expected, 100);
        assert_eq!(st.loss_pct, 0.0);
        assert_eq!(st.frames_concealed, 0);
    }

    #[test]
    fn a_loss_hint_resolves_an_empty_buffer() {
        let mut s = Sim::new();
        for i in 0..20u16 {
            s.send(i, i as u32 * 960, i as u64 * 20 + 5);
        }
        // Packet 20 never comes; the transport says so before 21 arrives.
        s.run_until(20 * 20 + 70);
        s.jb.hint_lost(20);
        // The rest arrive 100 ms late, so the buffer is empty at the hint.
        for i in 21..40u16 {
            s.send(i, i as u32 * 960, i as u64 * 20 + 105);
        }
        s.run_until(40 * 20 + 300);
        let st = s.jb.stats();
        assert!(!s.decoded().contains(&20));
        assert_eq!(s.decoded().len(), 39);
        assert!(st.frames_concealed >= 1);
        assert_eq!(st.packets_expected, 40);
    }

    #[test]
    fn target_delay_follows_jitter() {
        let mut s = Sim::new();
        let mut x = 12345u64;
        let mut late_before = 0;
        for i in 0..500u16 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let j = x % 60; // 0..60 ms of extra delay
            s.send(i, i as u32 * 960, i as u64 * 20 + 5 + j);
            if i == 100 {
                late_before = s.jb.stats().packets_late;
            }
        }
        s.run_until(500 * 20 + 300);
        let st = s.jb.stats();
        assert!(st.jb_target_ms >= 60.0, "{st:?}");
        assert!(st.jitter_ms > 10.0, "{st:?}");
        let late_after = st.packets_late - late_before;
        assert!(late_after <= 30, "{late_after} late packets out of 400 after adapting: {st:?}");
        assert!(st.concealed_pct < 15.0, "{st:?}");
    }
}
