//! The processing core: one single-threaded state machine that owns echo
//! cancellation, the encoder, every participant's jitter buffer and decoder,
//! the mixer and the meters. The engine's processing thread drives it in
//! 10 ms blocks; tests drive it directly with a simulated clock.

use crate::apm::{Apm, ProcessingConfig};
use crate::codec::{has_lbrr, Decoded, OpusDec, OpusEnc};
use crate::error::AudioError;
use crate::events::{Level, Levels};
use crate::frame::{dbov, EncodedFrame, EncoderCfg, RecvFrame, BLOCK_SAMPLES, FRAME_SAMPLES};
use crate::jitter::{JitterBuffer, JitterConfig, Next};
use crate::level::{LevelMeter, Vad};
use crate::stats::{CaptureStats, Stats};
use bytes::Bytes;
use std::time::{Duration, Instant};

/// Release time of the speaking indicator.
const SPEAKING_RELEASE_MS: u32 = 300;

/// Why the capture path is paused. A pause ends when no reason is left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pause {
    /// The caller suspended the call ([`crate::AudioEngine::suspend`]).
    Caller = 1,
    /// The phone backend has no device open (the OS's session is inactive,
    /// or a route change is reopening it).
    #[cfg_attr(
        not(any(test, all(feature = "os-voice", any(target_os = "ios", target_os = "android")))),
        allow(dead_code)
    )]
    Device = 2,
}

#[derive(Clone, Debug)]
pub(crate) struct CoreConfig {
    pub(crate) processing: ProcessingConfig,
    pub(crate) encoder: EncoderCfg,
    pub(crate) jitter: JitterConfig,
    pub(crate) initial_ts48k: u32,
    pub(crate) levels_interval_ms: u32,
    pub(crate) speaking_threshold_dbov: f32,
    pub(crate) max_participants: usize,
}

/// One remote participant's receive chain.
pub(crate) struct Participant {
    id: u64,
    jb: JitterBuffer,
    dec: OpusDec,
    pcm: [f32; FRAME_SAMPLES],
    meter: LevelMeter,
}

impl Participant {
    /// Build off the processing thread (it allocates), then hand it over.
    pub(crate) fn new(id: u64, cfg: &CoreConfig) -> Result<Box<Participant>, AudioError> {
        Ok(Box::new(Participant {
            id,
            jb: JitterBuffer::new(cfg.jitter.clone()),
            dec: OpusDec::new()?,
            pcm: [0.0; FRAME_SAMPLES],
            meter: LevelMeter::new(
                cfg.levels_interval_ms,
                cfg.speaking_threshold_dbov,
                SPEAKING_RELEASE_MS,
            ),
        }))
    }

    fn decode_tick(&mut self, now: Instant) {
        match self.jb.next(now) {
            Next::Silence => self.pcm.fill(0.0),
            Next::Decode(p) => {
                self.dec.decode(&p, &mut self.pcm);
            }
            Next::Fec(p) => {
                // decode_fec conceals by itself when it can't use the FEC.
                let used = if has_lbrr(&p) {
                    self.dec.decode_fec(&p, &mut self.pcm) == Decoded::Fec
                } else {
                    self.dec.conceal(&mut self.pcm);
                    false
                };
                if !used {
                    self.jb.fec_unavailable();
                }
            }
            Next::Conceal | Next::Dtx => self.dec.conceal(&mut self.pcm),
        }
    }
}

pub(crate) struct Core {
    cfg: CoreConfig,
    apm: Apm,
    enc: OpusEnc,
    participants: Vec<Box<Participant>>,
    capture_enabled: bool,
    deafened: bool,
    cap_frame: [f32; FRAME_SAMPLES],
    cap_len: usize,
    processed: [f32; BLOCK_SAMPLES],
    mix: [f32; BLOCK_SAMPLES],
    render_phase: usize,
    ts48k: u32,
    suppressed_run: u32,
    vad: Vad,
    local_meter: LevelMeter,
    level_blocks: u32,
    blocks_since_levels: u32,
    levels_seq: u64,
    levels_due: bool,
    /// [`Pause`] reasons, as bits. Paused while any is set.
    pause: u8,
    paused_at: Option<Instant>,
    /// A 20 ms frame has completed since the engine started.
    captured_any: bool,
    /// The last frame handed out was speech, so a pause should end it with
    /// a frame of silence.
    speech_open: bool,
    pub(crate) capture_stats: CaptureStats,
    pub(crate) recv_queue_drops: u64,
    unknown_frames: u64,
}

impl Core {
    pub(crate) fn new(cfg: CoreConfig) -> Result<Core, AudioError> {
        let enc = OpusEnc::new(cfg.encoder)?;
        let apm = Apm::new(&cfg.processing);
        let level_blocks = (cfg.levels_interval_ms / 10).max(1);
        Ok(Core {
            apm,
            enc,
            participants: Vec::with_capacity(cfg.max_participants),
            capture_enabled: true,
            deafened: false,
            cap_frame: [0.0; FRAME_SAMPLES],
            cap_len: 0,
            processed: [0.0; BLOCK_SAMPLES],
            mix: [0.0; BLOCK_SAMPLES],
            render_phase: 0,
            ts48k: cfg.initial_ts48k,
            suppressed_run: 0,
            vad: Vad::new(),
            local_meter: LevelMeter::new(
                cfg.levels_interval_ms,
                cfg.speaking_threshold_dbov,
                SPEAKING_RELEASE_MS,
            ),
            level_blocks,
            blocks_since_levels: 0,
            levels_seq: 0,
            levels_due: false,
            pause: 0,
            paused_at: None,
            captured_any: false,
            speech_open: false,
            capture_stats: CaptureStats::default(),
            recv_queue_drops: 0,
            unknown_frames: 0,
            cfg,
        })
    }

    /// Add a participant. Gives it back if the id exists or the table is full.
    pub(crate) fn add(&mut self, p: Box<Participant>) -> Result<(), Box<Participant>> {
        if self.participants.len() >= self.cfg.max_participants
            || self.participants.iter().any(|q| q.id == p.id)
        {
            return Err(p);
        }
        self.participants.push(p);
        Ok(())
    }

    pub(crate) fn remove(&mut self, id: u64) -> Option<Box<Participant>> {
        let i = self.participants.iter().position(|p| p.id == id)?;
        Some(self.participants.swap_remove(i))
    }

    pub(crate) fn insert(&mut self, f: RecvFrame) {
        match self.participants.iter_mut().find(|p| p.id == f.participant) {
            Some(p) => p.jb.insert(f.seq, f.ts48k, f.payload, f.arrival),
            None => self.unknown_frames += 1,
        }
    }

    pub(crate) fn lost(&mut self, participant: u64, seq: u16) {
        if let Some(p) = self.participants.iter_mut().find(|p| p.id == participant) {
            p.jb.hint_lost(seq);
        }
    }

    pub(crate) fn set_encoder(&mut self, cfg: EncoderCfg) -> Result<(), AudioError> {
        self.enc.apply(cfg)
    }

    pub(crate) fn set_capture(&mut self, on: bool) {
        self.capture_enabled = on;
        if !on {
            self.local_meter.silence();
        }
    }

    pub(crate) fn set_deafened(&mut self, on: bool) {
        self.deafened = on;
    }

    pub(crate) fn is_paused(&self) -> bool {
        self.pause != 0
    }

    /// Pause or unpause the capture path for one reason.
    ///
    /// Paused, no frames are produced and capture blocks are ignored. A
    /// [`Pause::Caller`] pause also plays silence (participants are still
    /// decoded, so their levels, statistics and jitter buffers stay
    /// current). The receiver must see the gap as a pause, not as loss:
    ///
    /// - when the pause starts after speech, one frame of encoded silence
    ///   (marked `dtx`) closes the talkspurt, so the far end's decoder fades
    ///   out instead of concealing;
    /// - when it ends, `ts48k` jumps by the paused time (whole frames,
    ///   rounded up). The device may have been stopped, so the capture clock
    ///   can't measure it. The next frame's timestamp is then later than its
    ///   sequence number accounts for, which a receiver reads as DTX.
    ///
    /// Returns the closing frame, if there is one.
    pub(crate) fn set_paused(&mut self, why: Pause, on: bool, now: Instant) -> Option<EncodedFrame> {
        let was = self.is_paused();
        if on {
            self.pause |= why as u8;
        } else {
            self.pause &= !(why as u8);
        }
        match (was, self.is_paused()) {
            (false, true) => {
                self.paused_at = Some(now);
                let tail = self.close_talkspurt();
                self.cap_len = 0;
                self.local_meter.silence();
                tail
            }
            (true, false) => {
                if let Some(at) = self.paused_at.take() {
                    if self.captured_any {
                        self.ts48k = self.ts48k.wrapping_add(gap_ts48k(now.saturating_duration_since(at)));
                    }
                }
                self.cap_len = 0;
                None
            }
            _ => None,
        }
    }

    /// One frame of silence after speech, so the far end hears the talkspurt
    /// end rather than concealment of the last frame.
    fn close_talkspurt(&mut self) -> Option<EncodedFrame> {
        if !self.capture_enabled || !self.speech_open {
            return None;
        }
        self.speech_open = false;
        let ts = self.ts48k;
        self.ts48k = self.ts48k.wrapping_add(FRAME_SAMPLES as u32);
        self.cap_frame.fill(0.0);
        let dtx_on = self.enc.cfg().dtx;
        match self.enc.encode(&self.cap_frame) {
            Ok(e) if dtx_on && e.is_empty_dtx() => None,
            Ok(e) => Some(EncodedFrame {
                payload: Bytes::copy_from_slice(e.packet),
                ts48k: ts,
                dtx: true,
                vad: false,
                level_dbov: -127.0,
            }),
            Err(_) => None,
        }
    }

    /// Produce the next 10 ms of playback into `out` (480 samples) and feed
    /// it to the echo canceller as the reference.
    pub(crate) fn render_block(&mut self, now: Instant, out: &mut [f32]) {
        debug_assert_eq!(out.len(), BLOCK_SAMPLES);
        if self.render_phase == 0 {
            for p in self.participants.iter_mut() {
                p.decode_tick(now);
            }
        }
        let off = self.render_phase * BLOCK_SAMPLES;
        self.mix.fill(0.0);
        for p in self.participants.iter_mut() {
            let half = &p.pcm[off..off + BLOCK_SAMPLES];
            for (m, &v) in self.mix.iter_mut().zip(half) {
                *m += v;
            }
            p.meter.feed(half);
        }
        self.render_phase ^= 1;
        soft_limit(&mut self.mix);
        // A suspended call plays silence. A device pause doesn't: playback
        // can run without a microphone.
        if self.deafened || self.pause & Pause::Caller as u8 != 0 {
            out.fill(0.0);
        } else {
            out.copy_from_slice(&self.mix);
        }
        self.apm.render(out);

        self.blocks_since_levels += 1;
        if self.blocks_since_levels >= self.level_blocks {
            self.blocks_since_levels = 0;
            self.levels_seq += 1;
            self.levels_due = true;
        }
    }

    /// Process 10 ms of microphone audio (480 samples at 48 kHz). Every
    /// second block completes a 20 ms frame, which is encoded and returned
    /// unless capture is off or DTX has nothing to send.
    pub(crate) fn capture_block(&mut self, mic: &[f32]) -> Option<EncodedFrame> {
        debug_assert_eq!(mic.len(), BLOCK_SAMPLES);
        if self.is_paused() {
            return None;
        }
        self.apm.capture(mic, &mut self.processed);
        self.cap_frame[self.cap_len..self.cap_len + BLOCK_SAMPLES].copy_from_slice(&self.processed);
        self.cap_len += BLOCK_SAMPLES;
        if self.capture_enabled {
            self.local_meter.feed(&self.processed);
        }
        if self.cap_len < FRAME_SAMPLES {
            return None;
        }
        self.cap_len = 0;
        self.captured_any = true;
        let ts = self.ts48k;
        self.ts48k = self.ts48k.wrapping_add(FRAME_SAMPLES as u32);
        if !self.capture_enabled {
            self.speech_open = false;
            return None;
        }
        let level = dbov(&self.cap_frame);
        let vad = self.vad.update(level);
        let dtx_on = self.enc.cfg().dtx;
        match self.enc.encode(&self.cap_frame) {
            Ok(e) if dtx_on && e.is_empty_dtx() => {
                self.capture_stats.frames_dtx_suppressed += 1;
                self.suppressed_run += 1;
                self.speech_open = false;
                None
            }
            Ok(e) => {
                // A frame inside a DTX run (a comfort-noise update) rather
                // than the start of speech.
                let dtx = !vad && (self.suppressed_run > 0 || e.in_dtx);
                self.suppressed_run = 0;
                self.speech_open = vad;
                Some(EncodedFrame {
                    payload: Bytes::copy_from_slice(e.packet),
                    ts48k: ts,
                    dtx,
                    vad,
                    level_dbov: level,
                })
            }
            Err(_) => None,
        }
    }

    /// True once per levels interval; clears itself.
    pub(crate) fn take_levels_due(&mut self) -> bool {
        std::mem::take(&mut self.levels_due)
    }

    /// Fill `out` in place (no allocation while it has the capacity).
    pub(crate) fn fill_levels(&self, out: &mut Levels) {
        out.seq = self.levels_seq;
        out.local = if self.capture_enabled && !self.is_paused() {
            self.local_meter.level()
        } else {
            Level::default()
        };
        out.participants.clear();
        for p in self.participants.iter() {
            out.participants.push((p.id, p.meter.level()));
        }
    }

    /// Fill `out` in place (no allocation while it has the capacity).
    pub(crate) fn fill_stats(&self, out: &mut Stats) {
        out.participants.clear();
        for p in self.participants.iter() {
            out.participants.push((p.id, p.jb.stats()));
        }
        out.aec_delay_ms = self.apm.delay_ms();
        out.aec_erle_db = self.apm.erle_db();
        out.capture = self.capture_stats.clone();
        out.recv_queue_drops = self.recv_queue_drops;
        out.unknown_participant_frames = self.unknown_frames;
    }
}

/// A paused time in 48 kHz units: whole 20 ms frames, rounded up, so any
/// pause reads as a gap.
pub(crate) fn gap_ts48k(d: Duration) -> u32 {
    let frame = Duration::from_millis(crate::frame::FRAME_MS as u64);
    let frames = d.as_nanos().div_ceil(frame.as_nanos());
    // Wraps like the timestamp itself (a pause of more than a day).
    (frames as u64).wrapping_mul(FRAME_SAMPLES as u64) as u32
}

/// Gentle limiter on the mix: unity below 0.9, a tanh knee above it, never
/// past 1.0.
pub(crate) fn soft_limit(x: &mut [f32]) {
    const KNEE: f32 = 0.9;
    for v in x.iter_mut() {
        let a = v.abs();
        if a > KNEE {
            *v = v.signum() * (KNEE + (1.0 - KNEE) * ((a - KNEE) / (1.0 - KNEE)).tanh());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soft_limit_is_transparent_below_the_knee_and_bounded_above() {
        let mut x = [0.5f32, -0.89, 0.95, -3.0, 10.0];
        soft_limit(&mut x);
        assert_eq!(x[0], 0.5);
        assert_eq!(x[1], -0.89);
        assert!(x[2] > 0.9 && x[2] < 0.95);
        assert!(x[3] < -0.99 && x[3] >= -1.0);
        assert!(x[4] > 0.99 && x[4] <= 1.0);
    }
}
