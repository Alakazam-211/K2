//! NEON versions of the hot loops, for aarch64 targets with NEON (all the
//! usual ones). Each one gives the same bytes as its portable version, which
//! the tests check.
//!
//! This module and `sse2.rs` are the only ones with `unsafe` code. Every
//! intrinsic is called inside a private `unsafe fn` that first asserts
//! the lengths of the slices it reads and writes, and the public functions
//! cut those slices with checked arithmetic and ordinary bounds checks, so
//! every load and store lands inside a checked slice whatever the input.
//! The modules compile only when the target has the feature. The code is our own,
//! written from the RFC's formulas and the portable versions in this crate.

use core::arch::aarch64::*;

use crate::loop_filter::{luma_edge_portable, rows_len, span, Kind, Limits};

// ---------------------------------------------------------------------------
// Loop filter.

/// Eight taps (p3 to q3) of 16 segments, one vector per tap.
type Taps = [uint8x16_t; 8];

/// Transposes two 8x8 byte blocks at once: in each 64-bit half, byte `j`
/// of vector `i` moves to byte `i` of vector `j`.
#[inline(always)]
unsafe fn transpose(r: Taps) -> Taps {
    let t01 = vtrnq_u8(r[0], r[1]);
    let t23 = vtrnq_u8(r[2], r[3]);
    let t45 = vtrnq_u8(r[4], r[5]);
    let t67 = vtrnq_u8(r[6], r[7]);
    let u02 = vtrnq_u16(vreinterpretq_u16_u8(t01.0), vreinterpretq_u16_u8(t23.0));
    let u13 = vtrnq_u16(vreinterpretq_u16_u8(t01.1), vreinterpretq_u16_u8(t23.1));
    let u46 = vtrnq_u16(vreinterpretq_u16_u8(t45.0), vreinterpretq_u16_u8(t67.0));
    let u57 = vtrnq_u16(vreinterpretq_u16_u8(t45.1), vreinterpretq_u16_u8(t67.1));
    let v04 = vtrnq_u32(vreinterpretq_u32_u16(u02.0), vreinterpretq_u32_u16(u46.0));
    let v15 = vtrnq_u32(vreinterpretq_u32_u16(u13.0), vreinterpretq_u32_u16(u57.0));
    let v26 = vtrnq_u32(vreinterpretq_u32_u16(u02.1), vreinterpretq_u32_u16(u46.1));
    let v37 = vtrnq_u32(vreinterpretq_u32_u16(u13.1), vreinterpretq_u32_u16(u57.1));
    [
        vreinterpretq_u8_u32(v04.0),
        vreinterpretq_u8_u32(v15.0),
        vreinterpretq_u8_u32(v26.0),
        vreinterpretq_u8_u32(v37.0),
        vreinterpretq_u8_u32(v04.1),
        vreinterpretq_u8_u32(v15.1),
        vreinterpretq_u8_u32(v26.1),
        vreinterpretq_u8_u32(v37.1),
    ]
}

/// Pixel to signed value (`v - 128`).
#[inline(always)]
unsafe fn sg(v: uint8x16_t) -> int8x16_t {
    vreinterpretq_s8_u8(veorq_u8(v, vdupq_n_u8(0x80)))
}

/// Signed value back to a pixel.
#[inline(always)]
unsafe fn us(v: int8x16_t) -> uint8x16_t {
    veorq_u8(vreinterpretq_u8_s8(v), vdupq_n_u8(0x80))
}

/// `|p0 - q0| * 2 + |p1 - q1| / 2 <= edge` (saturating, as the portable code).
#[inline(always)]
unsafe fn simple_mask(t: &Taps, edge: u8) -> uint8x16_t {
    let d = vabdq_u8(t[3], t[4]);
    let sum = vqaddq_u8(vqaddq_u8(d, d), vshrq_n_u8::<1>(vabdq_u8(t[2], t[5])));
    vcleq_u8(sum, vdupq_n_u8(edge))
}

/// The normal filter's mask and high-edge-variance flag.
#[inline(always)]
unsafe fn normal_mask_hev(t: &Taps, l: Limits) -> (uint8x16_t, uint8x16_t) {
    let p1p0 = vabdq_u8(t[2], t[3]);
    let q1q0 = vabdq_u8(t[5], t[4]);
    let m = vmaxq_u8(
        vmaxq_u8(vabdq_u8(t[0], t[1]), vabdq_u8(t[1], t[2])),
        vmaxq_u8(vabdq_u8(t[7], t[6]), vabdq_u8(t[6], t[5])),
    );
    let m = vmaxq_u8(m, vmaxq_u8(p1p0, q1q0));
    let mask = vandq_u8(simple_mask(t, l.edge), vcleq_u8(m, vdupq_n_u8(l.interior)));
    let hev = vcgtq_u8(vmaxq_u8(p1p0, q1q0), vdupq_n_u8(l.hev));
    (mask, hev)
}

/// `c(outer + 3 * (q0 - p0))`, computed in 16 bits and narrowed with
/// saturation (the RFC's clamp).
#[inline(always)]
unsafe fn common(outer: int8x16_t, p0: int8x16_t, q0: int8x16_t) -> int8x16_t {
    let lo = vsubl_s8(vget_low_s8(q0), vget_low_s8(p0));
    let hi = vsubl_high_s8(q0, p0);
    let lo = vaddw_s8(vmulq_n_s16(lo, 3), vget_low_s8(outer));
    let hi = vaddw_high_s8(vmulq_n_s16(hi, 3), outer);
    vcombine_s8(vqmovn_s16(lo), vqmovn_s16(hi))
}

/// `c((k * w + 63) >> 7)`.
#[inline(always)]
unsafe fn tap_weight(w: int8x16_t, k: i8) -> int8x16_t {
    let kk = vdup_n_s8(k);
    let lo = vmull_s8(vget_low_s8(w), kk);
    let hi = vmull_high_s8(w, vdupq_n_s8(k));
    let r = vdupq_n_s16(63);
    let lo = vshrq_n_s16::<7>(vaddq_s16(lo, r));
    let hi = vshrq_n_s16::<7>(vaddq_s16(hi, r));
    vcombine_s8(vqmovn_s16(lo), vqmovn_s16(hi))
}

#[inline(always)]
unsafe fn simple_filter(t: &mut Taps, l: Limits) {
    let mask = simple_mask(t, l.edge);
    let (p1, p0, q0, q1) = (sg(t[2]), sg(t[3]), sg(t[4]), sg(t[5]));
    let a = common(vqsubq_s8(p1, q1), p0, q0);
    let f1 = vshrq_n_s8::<3>(vqaddq_s8(a, vdupq_n_s8(4)));
    let f2 = vshrq_n_s8::<3>(vqaddq_s8(a, vdupq_n_s8(3)));
    t[4] = vbslq_u8(mask, us(vqsubq_s8(q0, f1)), t[4]);
    t[3] = vbslq_u8(mask, us(vqaddq_s8(p0, f2)), t[3]);
}

#[inline(always)]
unsafe fn mb_filter(t: &mut Taps, l: Limits) {
    let (mask, hev) = normal_mask_hev(t, l);
    let (p2, p1, p0) = (sg(t[1]), sg(t[2]), sg(t[3]));
    let (q0, q1, q2) = (sg(t[4]), sg(t[5]), sg(t[6]));
    let w = common(vqsubq_s8(p1, q1), p0, q0);
    let f1 = vshrq_n_s8::<3>(vqaddq_s8(w, vdupq_n_s8(4)));
    let f2 = vshrq_n_s8::<3>(vqaddq_s8(w, vdupq_n_s8(3)));
    let a27 = tap_weight(w, 27);
    let a18 = tap_weight(w, 18);
    let a9 = tap_weight(w, 9);
    let only = vandq_u8(mask, hev);
    let wide = vbicq_u8(mask, hev);
    t[4] = vbslq_u8(
        only,
        us(vqsubq_s8(q0, f1)),
        vbslq_u8(wide, us(vqsubq_s8(q0, a27)), t[4]),
    );
    t[3] = vbslq_u8(
        only,
        us(vqaddq_s8(p0, f2)),
        vbslq_u8(wide, us(vqaddq_s8(p0, a27)), t[3]),
    );
    t[5] = vbslq_u8(wide, us(vqsubq_s8(q1, a18)), t[5]);
    t[2] = vbslq_u8(wide, us(vqaddq_s8(p1, a18)), t[2]);
    t[6] = vbslq_u8(wide, us(vqsubq_s8(q2, a9)), t[6]);
    t[1] = vbslq_u8(wide, us(vqaddq_s8(p2, a9)), t[1]);
}

#[inline(always)]
unsafe fn subblock_filter(t: &mut Taps, l: Limits) {
    let (mask, hev) = normal_mask_hev(t, l);
    let (p1, p0, q0, q1) = (sg(t[2]), sg(t[3]), sg(t[4]), sg(t[5]));
    // The outer taps count only at high edge variance.
    let outer = vandq_s8(vqsubq_s8(p1, q1), vreinterpretq_s8_u8(hev));
    let a = common(outer, p0, q0);
    let f1 = vshrq_n_s8::<3>(vqaddq_s8(a, vdupq_n_s8(4)));
    let f2 = vshrq_n_s8::<3>(vqaddq_s8(a, vdupq_n_s8(3)));
    // (f1 + 1) >> 1
    let a2 = vrshrq_n_s8::<1>(f1);
    let soft = vbicq_u8(mask, hev);
    t[4] = vbslq_u8(mask, us(vqsubq_s8(q0, f1)), t[4]);
    t[3] = vbslq_u8(mask, us(vqaddq_s8(p0, f2)), t[3]);
    t[5] = vbslq_u8(soft, us(vqsubq_s8(q1, a2)), t[5]);
    t[2] = vbslq_u8(soft, us(vqaddq_s8(p1, a2)), t[2]);
}

#[inline(always)]
unsafe fn filter(kind: Kind, t: &mut Taps, l: Limits) {
    match kind {
        Kind::Simple => simple_filter(t, l),
        Kind::Macroblock => mb_filter(t, l),
        Kind::Subblock => subblock_filter(t, l),
    }
}

/// Filters 16 segments across a horizontal edge. `buf` starts at p3 of the
/// first segment and holds the eight rows (stride `s`) of 16 pixels each.
///
/// Safety: NEON must be on, which the module's cfg guarantees; the length is
/// asserted below, so every access is inside `buf`.
#[inline(always)]
unsafe fn h16(buf: &mut [u8], s: usize, kind: Kind, l: Limits) {
    assert!(
        buf.len() >= rows_len(7, s, 16),
        "loop filter block too short"
    );
    let p = buf.as_mut_ptr();
    let mut t: Taps = [vdupq_n_u8(0); 8];
    for (k, v) in t.iter_mut().enumerate() {
        // In bounds: k * s + 16 <= 7 * s + 16 <= buf.len() (asserted).
        *v = vld1q_u8(p.add(k * s));
    }
    filter(kind, &mut t, l);
    for (k, v) in t.iter().enumerate().take(7).skip(1) {
        vst1q_u8(p.add(k * s), *v);
    }
}

/// Filters 16 segments across a vertical edge: rows `0..8` of `a` and
/// `0..8` of `b` (stride `s`), each eight pixels starting at p3. `a` and
/// `b` are separate slices, so they can't overlap.
///
/// Safety: NEON must be on (the module's cfg); the lengths are asserted below.
#[inline(always)]
unsafe fn v16(a: &mut [u8], b: &mut [u8], s: usize, kind: Kind, l: Limits) {
    let need = rows_len(7, s, 8);
    assert!(
        a.len() >= need && b.len() >= need,
        "loop filter block too short"
    );
    let (pa, pb) = (a.as_mut_ptr(), b.as_mut_ptr());
    let mut r: Taps = [vdupq_n_u8(0); 8];
    for (i, v) in r.iter_mut().enumerate() {
        // In bounds: i * s + 8 <= 7 * s + 8 (asserted) for each slice.
        *v = vcombine_u8(vld1_u8(pa.add(i * s)), vld1_u8(pb.add(i * s)));
    }
    let mut t = transpose(r);
    filter(kind, &mut t, l);
    let r = transpose(t);
    for (i, v) in r.iter().enumerate() {
        vst1_u8(pa.add(i * s), vget_low_u8(*v));
        vst1_u8(pb.add(i * s), vget_high_u8(*v));
    }
}

/// Filters 8 segments of `ub` and 8 of `vb` across a horizontal edge.
///
/// Safety: NEON must be on (the module's cfg); the lengths are asserted below.
#[inline(always)]
unsafe fn h8x2(ub: &mut [u8], vb: &mut [u8], s: usize, kind: Kind, l: Limits) {
    let need = rows_len(7, s, 8);
    assert!(
        ub.len() >= need && vb.len() >= need,
        "loop filter block too short"
    );
    let (pu, pv) = (ub.as_mut_ptr(), vb.as_mut_ptr());
    let mut t: Taps = [vdupq_n_u8(0); 8];
    for (k, v) in t.iter_mut().enumerate() {
        // In bounds: k * s + 8 <= 7 * s + 8 (asserted) for each slice.
        *v = vcombine_u8(vld1_u8(pu.add(k * s)), vld1_u8(pv.add(k * s)));
    }
    filter(kind, &mut t, l);
    for (k, v) in t.iter().enumerate().take(7).skip(1) {
        vst1_u8(pu.add(k * s), vget_low_u8(*v));
        vst1_u8(pv.add(k * s), vget_high_u8(*v));
    }
}

/// [`crate::loop_filter`]'s luma edge: 16 segments, q0 of the first at `pos`.
/// Every range is computed with checked arithmetic ([`span`]), so a stride
/// or position that overflows panics, as an out-of-range index does.
pub(crate) fn luma_edge(
    buf: &mut [u8],
    pos: usize,
    s: usize,
    vertical: bool,
    kind: Kind,
    l: Limits,
) {
    if vertical {
        if s < 8 {
            // Rows that overlap can't be split into two halves; the
            // portable code handles them (and gives the same bytes).
            return luma_edge_portable(buf, pos, s, vertical, kind, l);
        }
        // Rows 0..16, each eight pixels from p3.
        let block = &mut buf[span(pos, 0, 4, s, 15, 8)];
        let (top, bottom) = block.split_at_mut(8 * s);
        // SAFETY: NEON is on (the module only compiles with
        // it), and `v16` asserts that each half holds its eight rows.
        unsafe { v16(top, bottom, s, kind, l) }
    } else {
        let block = &mut buf[span(pos, 4, 0, s, 7, 16)];
        // SAFETY: NEON is on (the module only compiles with
        // it), and `h16` asserts that `block` holds its eight rows.
        unsafe { h16(block, s, kind, l) }
    }
}

/// [`crate::loop_filter`]'s chroma edge: eight segments in U, eight in V.
#[allow(clippy::too_many_arguments)]
pub(crate) fn chroma_edge(
    ub: &mut [u8],
    vb: &mut [u8],
    pos: usize,
    s: usize,
    vertical: bool,
    kind: Kind,
    l: Limits,
) {
    if vertical {
        let r = span(pos, 0, 4, s, 7, 8);
        let (u, v) = (&mut ub[r.clone()], &mut vb[r]);
        // SAFETY: NEON is on (the module only compiles with
        // it), and `v16` asserts that each slice holds its eight rows.
        unsafe { v16(u, v, s, kind, l) }
    } else {
        let r = span(pos, 4, 0, s, 7, 8);
        let (u, v) = (&mut ub[r.clone()], &mut vb[r]);
        // SAFETY: NEON is on (the module only compiles with
        // it), and `h8x2` asserts that each slice holds its eight rows.
        unsafe { h8x2(u, v, s, kind, l) }
    }
}
