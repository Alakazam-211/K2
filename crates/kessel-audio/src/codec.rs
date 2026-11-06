//! Opus encode and decode (ropus, a pure-Rust port of the reference
//! implementation): 48 kHz mono, 20 ms frames.

use crate::error::{AudioError, ErrorCode};
use crate::frame::{EncoderCfg, FEC_LOSS_HINT_PCT, FRAME_SAMPLES, SAMPLE_RATE};
use ropus::{OpusDecoder, OpusEncoder};

const OPUS_APPLICATION_VOIP: i32 = 2048;
const OPUS_SIGNAL_VOICE: i32 = 3001;
/// Opus caps one frame's packet at 1275 bytes (RFC 6716 section 3.2.1).
const MAX_PACKET: usize = 1276;
/// The longest frame a packet can hold: 120 ms.
const MAX_DECODE: usize = SAMPLE_RATE as usize * 120 / 1000;

/// One encoded frame, borrowed from the encoder's buffer.
pub(crate) struct Encoded<'a> {
    pub(crate) packet: &'a [u8],
    /// The encoder is in a DTX run.
    pub(crate) in_dtx: bool,
}

impl Encoded<'_> {
    /// A packet of at most 2 bytes carries no audio: in DTX it means
    /// "nothing to send for this frame".
    pub(crate) fn is_empty_dtx(&self) -> bool {
        self.packet.len() <= 2
    }
}

pub(crate) struct OpusEnc {
    enc: OpusEncoder,
    cfg: EncoderCfg,
    buf: [u8; MAX_PACKET],
}

fn opus_err(what: &str, code: i32) -> AudioError {
    AudioError::new(ErrorCode::Backend, format!("opus {what} failed: {code}"))
}

impl OpusEnc {
    pub(crate) fn new(cfg: EncoderCfg) -> Result<Self, AudioError> {
        let enc = OpusEncoder::new(SAMPLE_RATE as i32, 1, OPUS_APPLICATION_VOIP)
            .map_err(|c| opus_err("encoder init", c))?;
        let mut e = OpusEnc {
            enc,
            cfg,
            buf: [0; MAX_PACKET],
        };
        let r = e.enc.set_signal(OPUS_SIGNAL_VOICE);
        if r != 0 {
            return Err(opus_err("set_signal", r));
        }
        e.apply(cfg)?;
        Ok(e)
    }

    /// Apply new settings without resetting the codec state.
    pub(crate) fn apply(&mut self, cfg: EncoderCfg) -> Result<(), AudioError> {
        let bitrate = cfg.bitrate_bps.clamp(500, 512_000) as i32;
        let steps = [
            ("set_bitrate", self.enc.set_bitrate(bitrate)),
            (
                "set_complexity",
                self.enc.set_complexity(cfg.complexity.min(10) as i32),
            ),
            ("set_inband_fec", self.enc.set_inband_fec(cfg.fec as i32)),
            (
                "set_packet_loss_perc",
                self.enc
                    .set_packet_loss_perc(if cfg.fec { FEC_LOSS_HINT_PCT as i32 } else { 0 }),
            ),
            ("set_dtx", self.enc.set_dtx(cfg.dtx as i32)),
        ];
        for (what, code) in steps {
            if code != 0 {
                return Err(opus_err(what, code));
            }
        }
        self.cfg = cfg;
        Ok(())
    }

    pub(crate) fn cfg(&self) -> EncoderCfg {
        self.cfg
    }

    /// Encode one 20 ms frame (960 samples in [-1, 1]).
    pub(crate) fn encode(&mut self, pcm: &[f32]) -> Result<Encoded<'_>, AudioError> {
        debug_assert_eq!(pcm.len(), FRAME_SAMPLES);
        let n = self
            .enc
            .encode_float(pcm, FRAME_SAMPLES as i32, &mut self.buf, MAX_PACKET as i32)
            .map_err(|c| opus_err("encode", c))? as usize;
        let in_dtx = self.enc.get_in_dtx() != 0;
        Ok(Encoded {
            packet: &self.buf[..n],
            in_dtx,
        })
    }
}

/// Whether an Opus packet carries in-band FEC (SILK LBRR data) for the
/// frame before it. Mono, single-frame (code 0) packets only, which is what
/// this crate's encoder sends; anything else reports false. Written from
/// the bitstream layout in RFC 6716 (Section 4.2.3, header bits): the LBRR
/// flag follows the VAD flags at the top of the first payload byte. Gives
/// the same answer as libopus' `opus_packet_has_lbrr` for these packets.
pub(crate) fn has_lbrr(packet: &[u8]) -> bool {
    if packet.len() < 2 {
        return false;
    }
    let toc = packet[0];
    let config = toc >> 3;
    let stereo = toc & 0x04 != 0;
    let code = toc & 0x03;
    if config >= 16 || stereo || code != 0 {
        // CELT-only (no SILK layer), stereo, or several frames.
        return false;
    }
    // SILK frames in this Opus frame: 10/20 ms -> 1, 40 ms -> 2, 60 ms -> 3.
    let silk_frames = if config < 12 {
        match config & 3 {
            2 => 2,
            3 => 3,
            _ => 1,
        }
    } else {
        1
    };
    (packet[1] >> (7 - silk_frames)) & 1 == 1
}

/// How a 20 ms frame of decoded audio came about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Decoded {
    Packet,
    Fec,
    Concealed,
}

pub(crate) struct OpusDec {
    dec: OpusDecoder,
    scratch: Vec<f32>,
}

impl OpusDec {
    pub(crate) fn new() -> Result<Self, AudioError> {
        Ok(OpusDec {
            dec: OpusDecoder::new(SAMPLE_RATE as i32, 1).map_err(|c| opus_err("decoder init", c))?,
            scratch: vec![0.0; MAX_DECODE],
        })
    }

    /// Decode a packet into `out` (960 samples). A packet that doesn't
    /// decode to exactly one 20 ms frame is concealed instead; returns how
    /// the output was made.
    pub(crate) fn decode(&mut self, packet: &[u8], out: &mut [f32]) -> Decoded {
        debug_assert_eq!(out.len(), FRAME_SAMPLES);
        match self
            .dec
            .decode_float(Some(packet), &mut self.scratch, MAX_DECODE as i32, false)
        {
            Ok(n) if n as usize == FRAME_SAMPLES => {
                out.copy_from_slice(&self.scratch[..FRAME_SAMPLES]);
                Decoded::Packet
            }
            _ => {
                self.conceal(out);
                Decoded::Concealed
            }
        }
    }

    /// Rebuild the frame before `next` from `next`'s in-band FEC. Falls back
    /// to concealment when `next` carries none (the decoder does that
    /// itself) or fails.
    pub(crate) fn decode_fec(&mut self, next: &[u8], out: &mut [f32]) -> Decoded {
        match self.dec.decode_float(Some(next), out, FRAME_SAMPLES as i32, true) {
            Ok(n) if n as usize == FRAME_SAMPLES => Decoded::Fec,
            _ => {
                self.conceal(out);
                Decoded::Concealed
            }
        }
    }

    /// Packet-loss concealment for one 20 ms frame. After a DTX
    /// comfort-noise frame this continues the comfort noise.
    pub(crate) fn conceal(&mut self, out: &mut [f32]) {
        match self.dec.decode_float(None, out, FRAME_SAMPLES as i32, false) {
            Ok(n) if n as usize == FRAME_SAMPLES => {}
            _ => out.fill(0.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A harmonic-rich buzz whose pitch hops every 30 ms (so concealment,
    /// which extrapolates the last pitch, can't guess a lost frame).
    fn speechy(n: usize, seed: u32) -> Vec<f32> {
        let mut phase = 0.0f32;
        (0..n)
            .map(|i| {
                let seg = (i / 1440) as u32;
                let f0 = 100.0 + (seg.wrapping_mul(7919).wrapping_add(seed * 31) % 120) as f32;
                phase += 2.0 * std::f32::consts::PI * f0 / 48_000.0;
                if phase > 2.0 * std::f32::consts::PI {
                    phase -= 2.0 * std::f32::consts::PI;
                }
                let t = i as f32 / 48_000.0;
                let env = 0.6 + 0.4 * (2.0 * std::f32::consts::PI * 4.0 * t).sin();
                let mut s = 0.0;
                for h in 1..12 {
                    s += (phase * h as f32).sin() / h as f32;
                }
                0.1 * env * s
            })
            .collect()
    }

    #[test]
    fn roundtrip_keeps_level() {
        let mut enc = OpusEnc::new(EncoderCfg::default()).unwrap();
        let mut dec = OpusDec::new().unwrap();
        let x = speechy(48_000, 1);
        let mut y = vec![0.0f32; x.len()];
        for (i, f) in x.chunks(FRAME_SAMPLES).enumerate() {
            let e = enc.encode(f).unwrap();
            assert!(!e.is_empty_dtx(), "speech frame {i} came out empty");
            assert!(e.packet.len() < 300, "32 kbit/s frame of {} bytes", e.packet.len());
            let p = e.packet.to_vec();
            assert_eq!(
                dec.decode(&p, &mut y[i * FRAME_SAMPLES..(i + 1) * FRAME_SAMPLES]),
                Decoded::Packet
            );
        }
        let lx = crate::frame::dbov(&x[4800..]);
        let ly = crate::frame::dbov(&y[4800..]);
        assert!((lx - ly).abs() < 1.0, "level in {lx} out {ly}");
    }

    #[test]
    fn dtx_goes_quiet_on_silence() {
        let mut enc = OpusEnc::new(EncoderCfg::default()).unwrap();
        let silence = vec![0.0f32; FRAME_SAMPLES];
        let mut empty = 0;
        let mut in_dtx = 0;
        for _ in 0..100 {
            let e = enc.encode(&silence).unwrap();
            empty += e.is_empty_dtx() as u32;
            in_dtx += e.in_dtx as u32;
        }
        // After a short hangover the encoder sends almost nothing.
        assert!(empty >= 70, "only {empty} of 100 silent frames were empty");
        assert!(in_dtx >= 70, "only {in_dtx} of 100 silent frames were in DTX");
    }

    #[cfg(feature = "speech-tests")]
    fn real_speech(seed: u64, secs: usize) -> Vec<f32> {
        let mut rng = crate::testkit::Rng::new(seed);
        crate::testkit::speech_track(crate::testkit::FAR_POOL, &mut rng, 48_000 * secs)
    }

    #[cfg(feature = "speech-tests")]
    #[test]
    fn lbrr_flag_follows_the_fec_setting() {
        let _cpu = crate::tests::shared_cpu();
        let x = real_speech(3, 10);
        for fec in [true, false] {
            let cfg = EncoderCfg {
                fec,
                dtx: false,
                ..EncoderCfg::default()
            };
            let mut enc = OpusEnc::new(cfg).unwrap();
            let mut speech_frames = 0;
            let mut with = 0;
            for f in x.chunks(FRAME_SAMPLES) {
                let lbrr = has_lbrr(enc.encode(f).unwrap().packet);
                if crate::frame::dbov(f) > -40.0 {
                    speech_frames += 1;
                    with += lbrr as u32;
                }
            }
            assert!(speech_frames > 200);
            if fec {
                assert!(
                    with * 2 >= speech_frames,
                    "only {with} of {speech_frames} speech frames carry FEC"
                );
            } else {
                assert_eq!(with, 0);
            }
        }
    }

    #[test]
    fn dtx_off_sends_every_frame() {
        let cfg = EncoderCfg {
            dtx: false,
            ..EncoderCfg::default()
        };
        let mut enc = OpusEnc::new(cfg).unwrap();
        let silence = vec![0.0f32; FRAME_SAMPLES];
        for _ in 0..100 {
            let e = enc.encode(&silence).unwrap();
            assert!(!e.in_dtx);
        }
    }

    #[cfg(feature = "speech-tests")]
    #[test]
    fn fec_recovers_lost_frames_better_than_plc() {
        let _cpu = crate::tests::shared_cpu();
        let x = real_speech(4, 10);
        let cfg = EncoderCfg {
            dtx: false,
            ..EncoderCfg::default()
        };
        let frames: Vec<Vec<u8>> = {
            let mut enc = OpusEnc::new(cfg).unwrap();
            x.as_chunks::<FRAME_SAMPLES>().0.iter()
                .map(|f| enc.encode(f).unwrap().packet.to_vec())
                .collect()
        };
        // Lose speech frames whose successor carries FEC, spaced apart.
        let mut lost = Vec::new();
        let mut i = 10;
        while i + 1 < frames.len() && lost.len() < 20 {
            let f = &x[i * FRAME_SAMPLES..(i + 1) * FRAME_SAMPLES];
            if has_lbrr(&frames[i + 1]) && crate::frame::dbov(f) > -35.0 {
                lost.push(i);
                i += 12;
            } else {
                i += 1;
            }
        }
        assert!(lost.len() >= 10, "only {} candidate frames", lost.len());
        let decode_all = |mode: u8| -> Vec<f32> {
            let mut dec = OpusDec::new().unwrap();
            let mut y = vec![0.0f32; frames.len() * FRAME_SAMPLES];
            for (k, p) in frames.iter().enumerate() {
                let out = &mut y[k * FRAME_SAMPLES..(k + 1) * FRAME_SAMPLES];
                match (mode, lost.contains(&k)) {
                    (1, true) => {
                        assert_eq!(dec.decode_fec(&frames[k + 1], out), Decoded::Fec);
                    }
                    (2, true) => dec.conceal(out),
                    _ => {
                        dec.decode(p, out);
                    }
                }
            }
            y
        };
        let reference = decode_all(0);
        let snr = |y: &[f32]| -> f64 {
            let (mut s, mut e) = (0.0f64, 0.0f64);
            for &k in &lost {
                for i in k * FRAME_SAMPLES..(k + 1) * FRAME_SAMPLES {
                    s += (reference[i] as f64).powi(2);
                    e += (y[i] as f64 - reference[i] as f64).powi(2);
                }
            }
            10.0 * (s / e.max(1e-12)).log10()
        };
        let snr_fec = snr(&decode_all(1));
        let snr_plc = snr(&decode_all(2));
        eprintln!("[codec] lost-frame SNR: FEC {snr_fec:.1} dB, PLC {snr_plc:.1} dB");
        assert!(
            snr_fec > snr_plc + 6.0,
            "FEC frames ({snr_fec:.1} dB) are not clearly better than PLC ({snr_plc:.1} dB)"
        );
    }
}
