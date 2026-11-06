//! Intra prediction (RFC 6386 Section 12) and inter prediction with
//! sub-pixel interpolation (Section 18).
//!
//! Intra prediction works in a small workspace that holds the block plus a
//! one-pixel border: row 0 is the row above (index 0 is the above-left
//! pixel, then the pixels above, then for luma four above-right pixels) and
//! column 0 of the following rows is the column to the left.

use crate::tables::*;

/// Stride of the luma workspace: P, 16 above, 4 above-right.
pub const LUMA_WS_STRIDE: usize = 21;
pub const LUMA_WS_SIZE: usize = LUMA_WS_STRIDE * 17;
/// Stride of a chroma workspace: P and 8 above.
pub const CHROMA_WS_STRIDE: usize = 9;
pub const CHROMA_WS_SIZE: usize = CHROMA_WS_STRIDE * 9;

#[inline(always)]
fn clamp255(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// Whole-block prediction (16x16 luma or 8x8 chroma) for the four
/// macroblock-level modes. `n` is the block size; the block starts at
/// `ws[stride + 1]`.
pub fn predict_block(
    ws: &mut [u8],
    stride: usize,
    n: usize,
    mode: u8,
    have_above: bool,
    have_left: bool,
) {
    let base = stride + 1;
    match mode {
        DC_PRED => {
            let shift = if n == 16 { 3 } else { 2 };
            let mut sum = 0u32;
            let mut count_shift = shift;
            if have_above {
                sum += ws[1..1 + n].iter().map(|&p| p as u32).sum::<u32>();
                count_shift += 1;
            }
            if have_left {
                sum += (0..n).map(|r| ws[(r + 1) * stride] as u32).sum::<u32>();
                count_shift += 1;
            }
            let dc = if have_above || have_left {
                ((sum + (1 << (count_shift - 1))) >> count_shift) as u8
            } else {
                128
            };
            for r in 0..n {
                ws[base + r * stride..base + r * stride + n].fill(dc);
            }
        }
        V_PRED => {
            let (top, rest) = ws.split_at_mut(stride);
            for r in 0..n {
                rest[1 + r * stride..1 + r * stride + n].copy_from_slice(&top[1..1 + n]);
            }
        }
        H_PRED => {
            for r in 0..n {
                let l = ws[(r + 1) * stride];
                ws[base + r * stride..base + r * stride + n].fill(l);
            }
        }
        TM_PRED => {
            let p = ws[0] as i32;
            for r in 0..n {
                let l = ws[(r + 1) * stride] as i32 - p;
                for c in 0..n {
                    let a = ws[1 + c] as i32;
                    ws[base + r * stride + c] = clamp255(l + a);
                }
            }
        }
        _ => unreachable!("not a whole-block intra mode"),
    }
}

/// One 4x4 subblock prediction (Section 12.3). `pos` is the block's top-left
/// in the workspace; the row above (from `pos - stride - 1`, 9 pixels with
/// the above-left and four above-right) and the column to the left must be
/// filled in.
pub fn predict_subblock(ws: &mut [u8], stride: usize, pos: usize, mode: u8) {
    let a = pos - stride; // above row: a-1 is the above-left pixel
    let above = |i: isize| ws[(a as isize + i) as usize] as i32;
    // Edge pixels as the RFC names them: E[0..9] runs from the bottom of the
    // left column, through the corner, along the top to the above-right.
    let l = |i: usize| ws[pos - 1 + i * stride] as i32;
    let p = above(-1);
    let t: [i32; 8] = [
        above(0),
        above(1),
        above(2),
        above(3),
        above(4),
        above(5),
        above(6),
        above(7),
    ];
    let lf: [i32; 4] = [l(0), l(1), l(2), l(3)];
    let mut b = [0u8; 16];
    let avg3 = |x: i32, y: i32, z: i32| ((x + 2 * y + z + 2) >> 2) as u8;
    let avg2 = |x: i32, y: i32| ((x + y + 1) >> 1) as u8;
    match mode {
        B_DC_PRED => {
            let s: i32 = t[..4].iter().sum::<i32>() + lf.iter().sum::<i32>();
            b = [((s + 4) >> 3) as u8; 16];
        }
        B_TM_PRED => {
            for r in 0..4 {
                for c in 0..4 {
                    b[r * 4 + c] = clamp255(lf[r] + t[c] - p);
                }
            }
        }
        B_VE_PRED => {
            let row = [
                avg3(p, t[0], t[1]),
                avg3(t[0], t[1], t[2]),
                avg3(t[1], t[2], t[3]),
                avg3(t[2], t[3], t[4]),
            ];
            for r in 0..4 {
                b[r * 4..r * 4 + 4].copy_from_slice(&row);
            }
        }
        B_HE_PRED => {
            let v = [
                avg3(p, lf[0], lf[1]),
                avg3(lf[0], lf[1], lf[2]),
                avg3(lf[1], lf[2], lf[3]),
                avg3(lf[2], lf[3], lf[3]),
            ];
            for r in 0..4 {
                b[r * 4..r * 4 + 4].fill(v[r]);
            }
        }
        B_LD_PRED => {
            for r in 0..4 {
                for c in 0..4 {
                    let i = r + c;
                    b[r * 4 + c] = if i < 6 {
                        avg3(t[i], t[i + 1], t[i + 2])
                    } else {
                        avg3(t[6], t[7], t[7])
                    };
                }
            }
        }
        B_RD_PRED => {
            // e[0..9]: left bottom to top, P, then above.
            let e = [lf[3], lf[2], lf[1], lf[0], p, t[0], t[1], t[2], t[3]];
            for r in 0..4 {
                for c in 0..4 {
                    let i = 3 - r + c;
                    b[r * 4 + c] = avg3(e[i], e[i + 1], e[i + 2]);
                }
            }
        }
        B_VR_PRED => {
            let e = [lf[3], lf[2], lf[1], lf[0], p, t[0], t[1], t[2], t[3]];
            b[12] = avg3(e[1], e[2], e[3]);
            b[8] = avg3(e[2], e[3], e[4]);
            let v = avg3(e[3], e[4], e[5]);
            b[13] = v;
            b[4] = v;
            let v = avg2(e[4], e[5]);
            b[9] = v;
            b[0] = v;
            let v = avg3(e[4], e[5], e[6]);
            b[14] = v;
            b[5] = v;
            let v = avg2(e[5], e[6]);
            b[10] = v;
            b[1] = v;
            let v = avg3(e[5], e[6], e[7]);
            b[15] = v;
            b[6] = v;
            let v = avg2(e[6], e[7]);
            b[11] = v;
            b[2] = v;
            b[7] = avg3(e[6], e[7], e[8]);
            b[3] = avg2(e[7], e[8]);
        }
        B_VL_PRED => {
            b[0] = avg2(t[0], t[1]);
            b[4] = avg3(t[0], t[1], t[2]);
            let v = avg2(t[1], t[2]);
            b[8] = v;
            b[1] = v;
            let v = avg3(t[1], t[2], t[3]);
            b[5] = v;
            b[12] = v;
            let v = avg2(t[2], t[3]);
            b[9] = v;
            b[2] = v;
            let v = avg3(t[2], t[3], t[4]);
            b[13] = v;
            b[6] = v;
            let v = avg2(t[3], t[4]);
            b[10] = v;
            b[3] = v;
            let v = avg3(t[3], t[4], t[5]);
            b[14] = v;
            b[7] = v;
            // The last two break the pattern (Section 12.3).
            b[11] = avg3(t[4], t[5], t[6]);
            b[15] = avg3(t[5], t[6], t[7]);
        }
        B_HD_PRED => {
            let e = [lf[3], lf[2], lf[1], lf[0], p, t[0], t[1], t[2], t[3]];
            b[12] = avg2(e[0], e[1]);
            b[13] = avg3(e[0], e[1], e[2]);
            let v = avg2(e[1], e[2]);
            b[8] = v;
            b[14] = v;
            let v = avg3(e[1], e[2], e[3]);
            b[9] = v;
            b[15] = v;
            let v = avg3(e[2], e[3], e[4]);
            b[11] = v;
            b[5] = v;
            let v = avg2(e[2], e[3]);
            b[4] = v;
            b[10] = v;
            let v = avg2(e[3], e[4]);
            b[6] = v;
            b[0] = v;
            let v = avg3(e[3], e[4], e[5]);
            b[7] = v;
            b[1] = v;
            b[2] = avg3(e[4], e[5], e[6]);
            b[3] = avg3(e[5], e[6], e[7]);
        }
        B_HU_PRED => {
            b[0] = avg2(lf[0], lf[1]);
            b[1] = avg3(lf[0], lf[1], lf[2]);
            let v = avg2(lf[1], lf[2]);
            b[2] = v;
            b[4] = v;
            let v = avg3(lf[1], lf[2], lf[3]);
            b[3] = v;
            b[5] = v;
            let v = avg2(lf[2], lf[3]);
            b[6] = v;
            b[8] = v;
            let v = avg3(lf[2], lf[3], lf[3]);
            b[7] = v;
            b[9] = v;
            let v = lf[3] as u8;
            for i in [10, 11, 12, 13, 14, 15] {
                b[i] = v;
            }
        }
        _ => unreachable!("not a subblock mode"),
    }
    for r in 0..4 {
        ws[pos + r * stride..pos + r * stride + 4].copy_from_slice(&b[r * 4..r * 4 + 4]);
    }
}

/// Fills the border of an intra-prediction workspace from the frame
/// (Section 12.2): 127 above the frame, 129 left of it, the above-left
/// pixel 127 on the top row and 129 on the left column.
#[allow(clippy::too_many_arguments)]
pub fn fill_ws_border(
    ws: &mut [u8],
    ws_stride: usize,
    plane: &[u8],
    s: usize,
    n: usize,
    mbx: usize,
    mby: usize,
    above_right: bool,
    last_col: bool,
) {
    let x = mbx * n;
    let y = mby * n;
    let extra = if above_right { 4 } else { 0 };
    if mby == 0 {
        ws[..1 + n + extra].fill(127);
    } else {
        let a = (y - 1) * s + x;
        ws[1..1 + n].copy_from_slice(&plane[a..a + n]);
        if above_right {
            if last_col {
                let v = plane[a + n - 1];
                ws[1 + n..1 + n + 4].fill(v);
            } else {
                ws[1 + n..1 + n + 4].copy_from_slice(&plane[a + n..a + n + 4]);
            }
        }
        ws[0] = if mbx == 0 { 129 } else { plane[a - 1] };
    }
    for r in 0..n {
        ws[(r + 1) * ws_stride] = if mbx == 0 {
            129
        } else {
            plane[(y + r) * s + x - 1]
        };
    }
}

/// Copies the predicted block out of a workspace into a plane of stride `s`.
pub fn store_ws(
    ws: &[u8],
    ws_stride: usize,
    plane: &mut [u8],
    s: usize,
    n: usize,
    mbx: usize,
    mby: usize,
) {
    for r in 0..n {
        let d = (mby * n + r) * s + mbx * n;
        plane[d..d + n].copy_from_slice(&ws[(r + 1) * ws_stride + 1..(r + 1) * ws_stride + 1 + n]);
    }
}

// --------------------------------------------------------------------------
// Inter prediction.

pub type Filter = [i32; 6];

pub const SIXTAP_FILTERS: [Filter; 8] = [
    [0, 0, 128, 0, 0, 0],
    [0, -6, 123, 12, -1, 0],
    [2, -11, 108, 36, -8, 1],
    [0, -9, 93, 50, -6, 0],
    [3, -16, 77, 77, -16, 3],
    [0, -6, 50, 93, -9, 0],
    [1, -8, 36, 108, -11, 2],
    [0, -1, 12, 123, -6, 0],
];

pub const BILINEAR_FILTERS: [Filter; 8] = [
    [0, 0, 128, 0, 0, 0],
    [0, 0, 112, 16, 0, 0],
    [0, 0, 96, 32, 0, 0],
    [0, 0, 80, 48, 0, 0],
    [0, 0, 64, 64, 0, 0],
    [0, 0, 48, 80, 0, 0],
    [0, 0, 32, 96, 0, 0],
    [0, 0, 16, 112, 0, 0],
];

/// A reference plane: `w` x `h` pixels, stride `w`.
pub struct RefPlane<'a> {
    pub data: &'a [u8],
    pub w: usize,
    pub h: usize,
}

/// One output row of `N` pixels of a filter pass. `taps[k]` is the source
/// row for tap `k` (its first element lines up with output 0).
///
/// The six-tap filters have non-negative taps 0, 2, 3 and 5 and
/// non-positive taps 1 and 4 (checked by a test), so the sum splits into
/// a positive and a negative part that each fit in 16 bits (at most 160 *
/// 255). `(pos - neg + 64) >> 7` clamped to 0..=255 is then exactly the
/// RFC's result: when `pos < neg` the RFC's value is negative or rounds to
/// 0, and the saturating subtraction gives 0 too. 16-bit lanes let the
/// compiler use twice as many per vector as 32-bit ones.
#[inline(always)]
fn filter_row<const N: usize, const BILINEAR: bool>(
    out: &mut [u8],
    taps: [&[u8]; 6],
    f: &Filter,
) {
    let out: &mut [u8; N] = (&mut out[..N]).try_into().expect("N pixels");
    let row = |k: usize| fixed::<N>(taps[k]);
    if BILINEAR {
        // Only taps 2 and 3 are non-zero, and the result never needs clamping.
        let (a, b) = (row(2), row(3));
        let (f2, f3) = (f[2] as u16, f[3] as u16);
        for c in 0..N {
            out[c] = ((a[c] as u16 * f2 + b[c] as u16 * f3 + 64) >> 7) as u8;
        }
    } else {
        let (t0, t1, t2, t3, t4, t5) = (row(0), row(1), row(2), row(3), row(4), row(5));
        let (f0, f2, f3, f5) = (f[0] as u16, f[2] as u16, f[3] as u16, f[5] as u16);
        let (f1, f4) = ((-f[1]) as u16, (-f[4]) as u16);
        for c in 0..N {
            let pos = t0[c] as u16 * f0 + t2[c] as u16 * f2 + t3[c] as u16 * f3 + t5[c] as u16 * f5;
            let neg = t1[c] as u16 * f1 + t4[c] as u16 * f4;
            out[c] = ((pos.saturating_sub(neg) + 64) >> 7).min(255) as u8;
        }
    }
}

#[inline(always)]
fn fixed<const N: usize>(s: &[u8]) -> &[u8; N] {
    s[..N].try_into().expect("N pixels")
}

/// The six source rows for a horizontal pass whose output 0 is at `p`.
#[inline(always)]
fn h_taps(src: &[u8], p: usize, n: usize) -> [&[u8]; 6] {
    let s = &src[p - 2..p + n + 3];
    [
        &s[0..n],
        &s[1..n + 1],
        &s[2..n + 2],
        &s[3..n + 3],
        &s[4..n + 4],
        &s[5..n + 5],
    ]
}

/// The six source rows for a vertical pass whose output 0 is at `p`.
#[inline(always)]
fn v_taps(src: &[u8], p: usize, stride: usize, n: usize) -> [&[u8]; 6] {
    let block = &src[p - 2 * stride..p + 3 * stride + n];
    let row = |k: usize| &block[k * stride..k * stride + n];
    [row(0), row(1), row(2), row(3), row(4), row(5)]
}

/// Predicts a `bw` x `bh` block at (`x`, `y`) of the current plane from
/// `refp`, displaced by (`mvx`, `mvy`) in eighth pixels of this plane, into
/// `dst` at `dst_pos` (stride `dst_stride`). Pixels outside the reference
/// plane repeat its edge pixels, without limit. `bw` is 4, 8 or 16 and
/// `bh` at most 16.
#[allow(clippy::too_many_arguments)]
pub fn inter_predict(
    dst: &mut [u8],
    dst_pos: usize,
    dst_stride: usize,
    refp: &RefPlane<'_>,
    x: i32,
    y: i32,
    bw: usize,
    bh: usize,
    mvx: i32,
    mvy: i32,
    filters: &[Filter; 8],
    bilinear: bool,
) {
    assert!(bh <= 16, "block height {bh}");
    let m = Motion {
        ix: x + (mvx >> 3),
        iy: y + (mvy >> 3),
        fx: (mvx & 7) as usize,
        fy: (mvy & 7) as usize,
    };
    match (bw, bilinear) {
        (16, false) => predict_n::<16, false>(dst, dst_pos, dst_stride, refp, &m, bh, filters),
        (16, true) => predict_n::<16, true>(dst, dst_pos, dst_stride, refp, &m, bh, filters),
        (8, false) => predict_n::<8, false>(dst, dst_pos, dst_stride, refp, &m, bh, filters),
        (8, true) => predict_n::<8, true>(dst, dst_pos, dst_stride, refp, &m, bh, filters),
        (4, false) => predict_n::<4, false>(dst, dst_pos, dst_stride, refp, &m, bh, filters),
        (4, true) => predict_n::<4, true>(dst, dst_pos, dst_stride, refp, &m, bh, filters),
        _ => panic!("block width {bw}"),
    }
}

/// A block's whole-pixel source position and its eighth-pixel fractions.
struct Motion {
    ix: i32,
    iy: i32,
    fx: usize,
    fy: usize,
}

#[inline(always)]
fn predict_n<const N: usize, const BILINEAR: bool>(
    dst: &mut [u8],
    dst_pos: usize,
    dst_stride: usize,
    refp: &RefPlane<'_>,
    m: &Motion,
    bh: usize,
    filters: &[Filter; 8],
) {
    // The source window is (ix - 2, iy - 2) to (ix + N + 2, iy + bh + 2).
    if m.ix >= 2
        && m.iy >= 2
        && (m.ix as usize) + N + 3 <= refp.w
        && (m.iy as usize) + bh + 3 <= refp.h
    {
        let origin = m.iy as usize * refp.w + m.ix as usize;
        filter_block::<N, BILINEAR>(
            dst, dst_pos, dst_stride, refp.data, refp.w, origin, bh, m.fx, m.fy, filters,
        );
    } else {
        predict_near_edge::<N, BILINEAR>(dst, dst_pos, dst_stride, refp, m, bh, filters);
    }
}

/// [`predict_n`] for a window that crosses the reference plane's edge: the
/// window is first copied out with the edge pixels repeated.
#[inline(never)]
fn predict_near_edge<const N: usize, const BILINEAR: bool>(
    dst: &mut [u8],
    dst_pos: usize,
    dst_stride: usize,
    refp: &RefPlane<'_>,
    m: &Motion,
    bh: usize,
    filters: &[Filter; 8],
) {
    let mut emu = [0u8; 21 * 21];
    let ew = N + 5;
    let max_x = refp.w as i32 - 1;
    let max_y = refp.h as i32 - 1;
    for r in 0..bh + 5 {
        let sy = (m.iy - 2 + r as i32).clamp(0, max_y) as usize * refp.w;
        for c in 0..ew {
            let sx = (m.ix - 2 + c as i32).clamp(0, max_x) as usize;
            emu[r * ew + c] = refp.data[sy + sx];
        }
    }
    filter_block::<N, BILINEAR>(
        dst,
        dst_pos,
        dst_stride,
        &emu,
        ew,
        2 * ew + 2,
        bh,
        m.fx,
        m.fy,
        filters,
    );
}

/// The sub-pixel filter passes (or a copy at whole-pixel positions),
/// specialised per block width and filter kind so the loops unroll.
#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn filter_block<const N: usize, const BILINEAR: bool>(
    dst: &mut [u8],
    dst_pos: usize,
    dst_stride: usize,
    src: &[u8],
    sstride: usize,
    origin: usize,
    bh: usize,
    fx: usize,
    fy: usize,
    filters: &[Filter; 8],
) {
    if fx == 0 && fy == 0 {
        for r in 0..bh {
            let s: &[u8; N] = src[origin + r * sstride..origin + r * sstride + N]
                .try_into()
                .expect("N pixels");
            let d = dst_pos + r * dst_stride;
            dst[d..d + N].copy_from_slice(s);
        }
        return;
    }
    if fy == 0 {
        let f = &filters[fx];
        for r in 0..bh {
            let d = dst_pos + r * dst_stride;
            filter_row::<N, BILINEAR>(
                &mut dst[d..d + N],
                h_taps(src, origin + r * sstride, N),
                f,
            );
        }
        return;
    }
    if fx == 0 {
        let f = &filters[fy];
        for r in 0..bh {
            let d = dst_pos + r * dst_stride;
            filter_row::<N, BILINEAR>(
                &mut dst[d..d + N],
                v_taps(src, origin + r * sstride, sstride, N),
                f,
            );
        }
        return;
    }
    // Two passes: horizontal over rows -2..bh+3, then vertical.
    let mut tmp = [0u8; 16 * 21];
    let fh = &filters[fx];
    for r in 0..bh + 5 {
        let s = origin - 2 * sstride + r * sstride;
        filter_row::<N, BILINEAR>(&mut tmp[r * N..r * N + N], h_taps(src, s, N), fh);
    }
    let fv = &filters[fy];
    for r in 0..bh {
        let d = dst_pos + r * dst_stride;
        filter_row::<N, BILINEAR>(&mut dst[d..d + N], v_taps(&tmp, (r + 2) * N, N, N), fv);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference::predict as reference;

    /// The split into positive and negative parts in `filter_row` relies
    /// on these signs.
    #[test]
    fn sixtap_tap_signs() {
        for f in SIXTAP_FILTERS.iter().chain(BILINEAR_FILTERS.iter()) {
            assert!(f[0] >= 0 && f[2] >= 0 && f[3] >= 0 && f[5] >= 0, "{f:?}");
            assert!(f[1] <= 0 && f[4] <= 0, "{f:?}");
            assert!(f[0] + f[2] + f[3] + f[5] <= 160, "{f:?}");
        }
    }

    /// Every block size, filter kind and sub-pixel phase, inside the plane
    /// and across all its edges, matches the 0.1.0 code byte for byte.
    #[test]
    fn inter_prediction_matches_the_reference() {
        let (w, h) = (48usize, 40usize);
        let mut seed = 0x2545_f491u32;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        let data: Vec<u8> = (0..w * h)
            .map(|i| {
                // Extremes as well as texture, so the clamps are exercised.
                let r = rnd();
                match r % 5 {
                    0 => 0,
                    1 => 255,
                    _ => ((i * 37 + (r as usize >> 8)) % 256) as u8,
                }
            })
            .collect();
        let refp = RefPlane { data: &data, w, h };
        let rp = reference::RefPlane { data: &data, w, h };
        let mut checked = 0;
        for (bw, bh) in [(16, 16), (8, 8), (4, 4)] {
            for bilinear in [false, true] {
                let (ours, theirs) = if bilinear {
                    (&BILINEAR_FILTERS, &reference::BILINEAR_FILTERS)
                } else {
                    (&SIXTAP_FILTERS, &reference::SIXTAP_FILTERS)
                };
                for (x, y) in [(0, 0), (16, 16), (w as i32 - bw as i32, 8), (8, h as i32 - bh as i32)] {
                    for mvy in -40..40 {
                        for mvx in [-200, -37, -9, -8, -3, 0, 1, 5, 7, 13, 26, 99, 300] {
                            let (mvx, mvy) = (mvx + (mvy & 7) * 3, mvy * 3 + (mvx & 3));
                            let mut a = vec![0x5au8; 24 * 24];
                            let mut b = a.clone();
                            inter_predict(
                                &mut a, 24 + 2, 24, &refp, x, y, bw, bh, mvx, mvy, ours, bilinear,
                            );
                            reference::inter_predict(
                                &mut b, 24 + 2, 24, &rp, x, y, bw, bh, mvx, mvy, theirs, bilinear,
                            );
                            assert!(
                                a == b,
                                "{bw}x{bh} bilinear {bilinear} at ({x}, {y}) mv ({mvx}, {mvy})"
                            );
                            checked += 1;
                        }
                    }
                }
            }
        }
        assert_eq!(checked, 3 * 2 * 4 * 80 * 13);
    }

    #[test]
    fn identity_filters_copy() {
        let data: Vec<u8> = (0..64 * 64).map(|i| (i * 7 % 251) as u8).collect();
        let refp = RefPlane {
            data: &data,
            w: 64,
            h: 64,
        };
        let mut a = vec![0u8; 16 * 16];
        inter_predict(
            &mut a,
            0,
            16,
            &refp,
            16,
            16,
            16,
            16,
            8,
            -16,
            &SIXTAP_FILTERS,
            false,
        );
        for r in 0..16 {
            for c in 0..16 {
                // One pixel right and two up: (17, 14).
                assert_eq!(a[r * 16 + c], data[(14 + r) * 64 + 17 + c]);
            }
        }
    }

    #[test]
    fn edge_emulation_repeats_edges() {
        let data: Vec<u8> = (0..32 * 32).map(|i| (i % 200) as u8).collect();
        let refp = RefPlane {
            data: &data,
            w: 32,
            h: 32,
        };
        let mut a = vec![0u8; 4 * 4];
        // Far to the left and above: every pixel is the corner pixel.
        inter_predict(
            &mut a,
            0,
            4,
            &refp,
            0,
            0,
            4,
            4,
            -800,
            -800,
            &SIXTAP_FILTERS,
            false,
        );
        assert!(a.iter().all(|&p| p == data[0]));
        // A sub-pixel position on a flat region stays flat.
        inter_predict(
            &mut a,
            0,
            4,
            &refp,
            0,
            0,
            4,
            4,
            -803,
            -805,
            &SIXTAP_FILTERS,
            false,
        );
        assert!(a.iter().all(|&p| p == data[0]));
    }
}
