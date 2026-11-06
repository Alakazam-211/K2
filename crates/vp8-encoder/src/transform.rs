//! Forward transforms and quantization.
//!
//! RFC 6386 specifies only the inverse transforms. The forward ones here
//! are the exact transposes of those inverses (derived for this crate), so
//! that decoding a quantized-free block gives back the input up to rounding.

use vp8_decoder::tables::{AC_QUANT, DC_QUANT};

// sqrt(2) * cos(pi / 8) and sqrt(2) * sin(pi / 8), scaled by 2^12.
// Residuals are within +-255, so every intermediate fits an i32.
const C: i32 = 5352;
const S: i32 = 2217;

#[inline(always)]
fn round_shift(v: i32, shift: u32) -> i32 {
    let half = 1i32 << (shift - 1);
    if v >= 0 {
        (v + half) >> shift
    } else {
        -((-v + half) >> shift)
    }
}

/// Forward DCT of a 4x4 block of residuals (`src` row-major, stride 4).
/// The decoder's inverse brings the result back to `src`.
pub(crate) fn fdct4x4(src: &[i32; 16], out: &mut [i32; 16]) {
    // The inverse is P = M X M^T / 8 with M^T M = 4I, so X = M^T P M / 2.
    // Rows first, kept with 3 fractional bits; then columns.
    let mut tmp = [0i32; 16];
    for r in 0..4 {
        let x = &src[r * 4..r * 4 + 4];
        let a0 = x[0] + x[3];
        let a1 = x[1] + x[2];
        let d0 = x[0] - x[3];
        let d1 = x[1] - x[2];
        tmp[r * 4] = (a0 + a1) << 3;
        tmp[r * 4 + 2] = (a0 - a1) << 3;
        tmp[r * 4 + 1] = round_shift(C * d0 + S * d1, 9);
        tmp[r * 4 + 3] = round_shift(S * d0 - C * d1, 9);
    }
    for c in 0..4 {
        let a0 = tmp[c] + tmp[12 + c];
        let a1 = tmp[4 + c] + tmp[8 + c];
        let d0 = tmp[c] - tmp[12 + c];
        let d1 = tmp[4 + c] - tmp[8 + c];
        // Scale 2^3 from the rows, 2^12 from the column constants, and the final halving.
        out[c] = round_shift((a0 + a1) << 12, 16);
        out[8 + c] = round_shift((a0 - a1) << 12, 16);
        out[4 + c] = round_shift(C * d0 + S * d1, 16);
        out[12 + c] = round_shift(S * d0 - C * d1, 16);
    }
}

/// Forward Walsh-Hadamard transform of the 16 luma DCs (block raster order).
pub(crate) fn fwht4x4(src: &[i32; 16], out: &mut [i32; 16]) {
    // The inverse is Y = H X H / 8 with H H = 4I (H symmetric), so X = H Y H / 2.
    let h = |x: [i32; 4]| -> [i32; 4] {
        [x[0] + x[1] + x[2] + x[3], x[0] + x[1] - x[2] - x[3], x[0] - x[1] - x[2] + x[3], x[0] - x[1] + x[2] - x[3]]
    };
    let mut tmp = [0i32; 16];
    for c in 0..4 {
        let col = h([src[c], src[4 + c], src[8 + c], src[12 + c]]);
        for r in 0..4 {
            tmp[r * 4 + c] = col[r];
        }
    }
    for r in 0..4 {
        let row = h([tmp[r * 4], tmp[r * 4 + 1], tmp[r * 4 + 2], tmp[r * 4 + 3]]);
        for c in 0..4 {
            let v = row[c];
            out[r * 4 + c] = if v >= 0 { (v + 1) >> 1 } else { -((-v + 1) >> 1) };
        }
    }
}

/// Quantizer step sizes for one frame: `[dc, ac]` per block type, the same
/// values the decoder derives from the quantizer index.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Quant {
    pub(crate) y1: [i32; 2],
    pub(crate) y2: [i32; 2],
    pub(crate) uv: [i32; 2],
}

impl Quant {
    pub(crate) fn new(q: u8) -> Self {
        let q = q.min(127) as usize;
        Quant {
            y1: [DC_QUANT[q], AC_QUANT[q]],
            y2: [DC_QUANT[q] * 2, (AC_QUANT[q] * 155 / 100).max(8)],
            uv: [DC_QUANT[q].min(132), AC_QUANT[q]],
        }
    }
}

/// The largest level a token can carry (DCT_CAT6: 67 + 2^11 - 1).
pub(crate) const MAX_LEVEL: i32 = 2048 + 66;

/// Quantizes one coefficient with a rounding offset of `bias / 256` of a step.
#[inline(always)]
pub(crate) fn quantize(v: i32, step: i32, bias: i32) -> i32 {
    let a = v.abs();
    // Division by a multiply with 2^32 / divisor rounded up. Just below a
    // boundary it can round up by one, which only nudges the rounding
    // offset; the decoder sees whatever level we choose.
    static RECIPROCALS: std::sync::OnceLock<[u64; 512]> = std::sync::OnceLock::new();
    let inv = RECIPROCALS.get_or_init(|| std::array::from_fn(|d| (1u64 << 32).div_ceil(d.max(1) as u64 * 256)));
    let num = (a * 256 + bias * step) as u64;
    let l = ((num * inv[step as usize]) >> 32).min(MAX_LEVEL as u64) as i32;
    if v < 0 {
        -l
    } else {
        l
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vp8_decoder::idct;

    fn rnd(seed: &mut u32) -> u32 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 17;
        *seed ^= *seed << 5;
        *seed
    }

    #[test]
    fn dct_round_trips_through_the_decoders_inverse() {
        let mut seed = 99u32;
        let mut worst = 0;
        for _ in 0..2000 {
            let mut res = [0i32; 16];
            for v in res.iter_mut() {
                *v = (rnd(&mut seed) % 511) as i32 - 255;
            }
            let mut c = [0i32; 16];
            fdct4x4(&res, &mut c);
            let coeffs: Vec<i16> = c.iter().map(|&v| v as i16).collect();
            let mut pred = [128u8; 16];
            idct::idct_add(&coeffs, &mut pred, 0, 4);
            for i in 0..16 {
                let want = (128 + res[i]).clamp(0, 255);
                worst = worst.max((pred[i] as i32 - want).abs());
            }
        }
        assert!(worst <= 1, "forward then inverse DCT differs by {worst}");
    }

    #[test]
    fn wht_round_trips_through_the_decoders_inverse() {
        let mut seed = 7u32;
        for _ in 0..2000 {
            let mut dcs = [0i32; 16];
            for v in dcs.iter_mut() {
                *v = (rnd(&mut seed) % 4001) as i32 - 2000;
            }
            let mut y2 = [0i32; 16];
            fwht4x4(&dcs, &mut y2);
            let input: [i16; 16] = std::array::from_fn(|i| y2[i] as i16);
            let mut out = [0i16; 256];
            idct::inverse_wht(&input, &mut out);
            for i in 0..16 {
                let d = (out[i * 16] as i32 - dcs[i]).abs();
                assert!(d <= 1, "block {i}: {} vs {}", out[i * 16], dcs[i]);
            }
        }
    }

    #[test]
    fn quantize_rounds_symmetrically() {
        assert_eq!(quantize(100, 10, 128), 10);
        assert_eq!(quantize(-100, 10, 128), -10);
        assert_eq!(quantize(14, 10, 128), 1);
        assert_eq!(quantize(15, 10, 128), 2);
        assert_eq!(quantize(15, 10, 0), 1);
        assert_eq!(quantize(1_000_000, 4, 128), MAX_LEVEL);
    }
}
