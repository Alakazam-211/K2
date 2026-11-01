//! The frame interface: encoded frames out of the capture path, received
//! frames into the jitter buffers, and the encoder settings.
//!
//! This is the public contract with transport crates. Everything here is
//! plaintext Opus: the transport encrypts after [`EncodedFrame`] leaves and
//! decrypts (and verifies) before a [`RecvFrame`] arrives. kessel-audio never
//! sees keys or ciphertext.

use bytes::Bytes;
use crossbeam_queue::ArrayQueue;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Sample rate of every PCM signal inside the pipeline and of the Opus
/// timestamps: 48 kHz.
pub const SAMPLE_RATE: u32 = 48_000;
/// Opus frame duration: 20 ms.
pub const FRAME_MS: u32 = 20;
/// Samples per 20 ms frame at 48 kHz (also the `ts48k` step per frame).
pub const FRAME_SAMPLES: usize = 960;
/// Samples per 10 ms block, the unit of echo cancellation and device I/O.
pub(crate) const BLOCK_SAMPLES: usize = 480;

/// One encoded 20 ms frame from the capture path.
///
/// `payload` is a plaintext Opus packet (mono, 48 kHz). The transport
/// encrypts it before sending.
///
/// # Timestamps
///
/// `ts48k` is the capture time of the frame's first sample in 48 kHz units.
/// It advances by [`FRAME_SAMPLES`] (960) per 20 ms of captured audio,
/// whether or not a frame was sent for it:
///
/// - **DTX:** while the encoder is in discontinuous transmission, frames
///   that carry nothing are not pushed. The next frame that is pushed (a
///   comfort-noise update, or speech again) carries the true capture time,
///   so its `ts48k` has advanced across the gap.
/// - **Mute:** with capture disabled ([`crate::AudioEngine::set_capture`])
///   no frames are pushed, and `ts48k` keeps advancing with the capture clock.
/// - **Wrap-around:** `ts48k` is a `u32` that wraps (modulo 2^32, about
///   24.8 hours at 48 kHz), like an RTP timestamp. Compare timestamps with
///   wrapping arithmetic (`a.wrapping_sub(b) as i32`). The first value is
///   [`crate::EngineConfig::initial_ts48k`].
#[derive(Clone, Debug, PartialEq)]
pub struct EncodedFrame {
    /// The Opus packet, plaintext. Never empty: frames with nothing to send
    /// (DTX) are not pushed.
    pub payload: Bytes,
    /// Capture timestamp of the first sample, 48 kHz units, wrapping.
    pub ts48k: u32,
    /// The encoder was in DTX when it produced this frame: a comfort-noise
    /// update rather than speech.
    pub dtx: bool,
    /// Voice activity after echo cancellation and noise suppression.
    pub vad: bool,
    /// Level of the processed frame in dBov (0 = full scale, down to -127).
    pub level_dbov: f32,
}

/// One received frame for a remote participant.
///
/// The transport must only pass frames that decrypted and verified. There is
/// no plaintext fallback.
#[derive(Clone, Debug)]
pub struct RecvFrame {
    /// The transport's stable id for the remote track (opaque here).
    pub participant: u64,
    /// The Opus packet, already decrypted and verified. 20 ms frames.
    pub payload: Bytes,
    /// RTP sequence number (wrapping).
    pub seq: u16,
    /// RTP timestamp, 48 kHz units (wrapping).
    pub ts48k: u32,
    /// When the packet arrived.
    pub arrival: Instant,
}

/// Encoder settings, set per call from the caller's media policy. They may
/// change mid-call ([`crate::AudioEngine::set_encoder`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EncoderCfg {
    /// Target bitrate in bits per second (clamped by Opus to 500..=512000).
    pub bitrate_bps: u32,
    /// In-band forward error correction. When on, the encoder is told to
    /// expect [`FEC_LOSS_HINT_PCT`] packet loss so that it actually spends
    /// bits on redundancy.
    pub fec: bool,
    /// Discontinuous transmission: send almost nothing during silence.
    pub dtx: bool,
    /// Encoder complexity, 0..=10.
    pub complexity: u8,
}

/// The packet-loss percentage the encoder is told to expect when
/// [`EncoderCfg::fec`] is on. Opus adds in-band FEC only when this is above 0.
pub const FEC_LOSS_HINT_PCT: u8 = 10;

impl Default for EncoderCfg {
    /// 32 kbit/s, FEC on, DTX on, complexity 9.
    fn default() -> Self {
        EncoderCfg {
            bitrate_bps: 32_000,
            fec: true,
            dtx: true,
            complexity: 9,
        }
    }
}

/// The receiving end of the encoded-frame queue (bounded, lock-free).
///
/// Clones share one queue: each frame is delivered once, to whichever clone
/// pops it. The audio side never blocks on it: when the queue is full, new
/// frames are dropped and counted in [`crate::CaptureStats::frames_dropped`].
#[derive(Clone)]
pub struct FrameRx {
    pub(crate) q: Arc<ArrayQueue<EncodedFrame>>,
}

impl FrameRx {
    /// Take the oldest frame, if any. Never blocks.
    pub fn try_recv(&self) -> Option<EncodedFrame> {
        self.q.pop()
    }

    /// Wait up to `timeout` for a frame. Polls once a millisecond; use
    /// [`FrameRx::try_recv`] from an async context.
    pub fn recv_timeout(&self, timeout: Duration) -> Option<EncodedFrame> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(f) = self.q.pop() {
                return Some(f);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Frames waiting.
    pub fn len(&self) -> usize {
        self.q.len()
    }

    /// No frames waiting.
    pub fn is_empty(&self) -> bool {
        self.q.is_empty()
    }

    /// The queue's capacity in frames.
    pub fn capacity(&self) -> usize {
        self.q.capacity()
    }
}

impl std::fmt::Debug for FrameRx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrameRx")
            .field("len", &self.q.len())
            .field("capacity", &self.q.capacity())
            .finish()
    }
}

/// Level in dBov of a block of samples in [-1, 1]: 10·log10(mean square),
/// clamped to [-127, 0] (0 dBov is a full-scale square wave).
pub(crate) fn dbov(x: &[f32]) -> f32 {
    if x.is_empty() {
        return -127.0;
    }
    let ms: f64 = x.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>() / x.len() as f64;
    dbov_from_mean_square(ms)
}

pub(crate) fn dbov_from_mean_square(ms: f64) -> f32 {
    if ms <= 1e-13 {
        return -127.0;
    }
    ((10.0 * ms.log10()) as f32).clamp(-127.0, 0.0)
}
