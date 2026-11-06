//! Rate control: picks each frame's quantizer to hit a target bitrate.
//!
//! The model is `bits = k * pixels / step(q)`, with `k` learnt from each
//! coded frame (one `k` for keyframes, one per temporal layer). A leaky
//! buffer tracks how far the stream is over or under budget; the frame
//! target leans against it, and frames are dropped when the stream runs
//! too far over.

use vp8_decoder::tables::AC_QUANT;

/// Quantizer range used for real-time coding.
pub(crate) const Q_MIN: u8 = 4;
pub(crate) const Q_MAX: u8 = 112;

pub(crate) struct RateControl {
    bitrate: f64,
    max_bitrate: f64,
    fps: f64,
    pixels: f64,
    /// Bits over budget (positive: we owe bits).
    buffer: f64,
    k_key: Option<f64>,
    k_inter: [Option<f64>; 3],
    last_q: [Option<u8>; 3],
    weights: [f64; 3],
}

impl RateControl {
    pub(crate) fn new(bitrate: u32, max_bitrate: u32, fps: u32, pixels: u32, weights: [f64; 3]) -> Self {
        RateControl {
            bitrate: bitrate.max(1) as f64,
            max_bitrate: max_bitrate.max(bitrate).max(1) as f64,
            fps: fps.max(1) as f64,
            pixels: pixels.max(1) as f64,
            buffer: 0.0,
            k_key: None,
            k_inter: [None; 3],
            last_q: [None; 3],
            weights,
        }
    }

    pub(crate) fn set_rates(&mut self, bitrate: u32, max_bitrate: u32, fps: u32) {
        self.bitrate = bitrate.max(1) as f64;
        self.max_bitrate = max_bitrate.max(bitrate).max(1) as f64;
        self.fps = fps.max(1) as f64;
        // Keep the debt in proportion to the new budget.
        self.buffer = self.buffer.clamp(-0.5 * self.bitrate, 0.6 * self.max_bitrate);
    }

    fn per_frame(&self) -> f64 {
        self.bitrate / self.fps
    }

    /// Whether to skip this (non-key) frame: the stream is far over budget.
    pub(crate) fn should_drop(&self) -> bool {
        self.buffer > 0.5 * self.max_bitrate
    }

    /// Records a skipped frame.
    pub(crate) fn dropped(&mut self) {
        self.buffer -= self.per_frame();
    }

    pub(crate) fn pick_q(&self, key: bool, tid: usize) -> u8 {
        let per_frame = self.per_frame();
        let target = if key {
            (per_frame * 8.0).min(self.bitrate * 0.5).max(per_frame)
        } else {
            let lean = (1.0 - self.buffer / (0.5 * self.bitrate)).clamp(0.3, 1.6);
            per_frame * self.weights[tid] * lean
        };
        let k = if key {
            self.k_key.unwrap_or(30.0)
        } else {
            self.k_inter[tid].or(self.k_inter[0]).unwrap_or(4.0)
        };
        // The smallest q whose predicted size fits the target.
        let mut q = Q_MAX;
        for cand in Q_MIN..=Q_MAX {
            if k * self.pixels / AC_QUANT[cand as usize] as f64 <= target {
                q = cand;
                break;
            }
        }
        if !key {
            if let Some(last) = self.last_q[tid] {
                q = q.clamp(last.saturating_sub(12).max(Q_MIN), (last + 12).min(Q_MAX));
            }
        }
        q
    }

    pub(crate) fn update(&mut self, bits: usize, key: bool, tid: usize, q: u8) {
        let bits = bits as f64;
        let measured = bits * AC_QUANT[q as usize] as f64 / self.pixels;
        let slot = if key { &mut self.k_key } else { &mut self.k_inter[tid] };
        *slot = Some(match *slot {
            Some(k) => 0.5 * k + 0.5 * measured,
            None => measured,
        });
        if key {
            self.last_q = [Some(q); 3];
        } else {
            self.last_q[tid] = Some(q);
        }
        self.buffer += bits - self.per_frame();
        self.buffer = self.buffer.max(-0.5 * self.bitrate);
    }
}
