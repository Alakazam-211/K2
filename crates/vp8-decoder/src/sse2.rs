//! SSE2 versions of the hot loops, for x86_64 (where SSE2 is always
//! present). Each one gives the same bytes as its portable version, which
//! the tests check. The structure follows `neon.rs`.
//!
//! This module and `neon.rs` are the only ones with `unsafe` code. Every
//! intrinsic is called inside a private `unsafe fn`, and each public
//! function slices its buffers with ordinary bounds checks before it calls
//! one, so every load and store lands inside a slice that was checked. The
//! code is our own, written from the RFC's formulas and the portable
//! versions in this crate.

use core::arch::x86_64::*;

use crate::loop_filter::{Kind, Limits};

/// Eight taps (p3 to q3) of 16 segments, one vector per tap.
type Taps = [__m128i; 8];

/// Rows 0..16 (eight bytes each, in the low half of each vector) to taps.
#[inline(always)]
unsafe fn rows_to_taps(r: &[__m128i; 16]) -> Taps {
    let mut a = [_mm_setzero_si128(); 8];
    for (i, v) in a.iter_mut().enumerate() {
        *v = _mm_unpacklo_epi8(r[2 * i], r[2 * i + 1]);
    }
    // b[0..4]: rows 0..8; b[4..8]: rows 8..16. Even: columns 0..4, odd: 4..8.
    let b = [
        _mm_unpacklo_epi16(a[0], a[1]),
        _mm_unpackhi_epi16(a[0], a[1]),
        _mm_unpacklo_epi16(a[2], a[3]),
        _mm_unpackhi_epi16(a[2], a[3]),
        _mm_unpacklo_epi16(a[4], a[5]),
        _mm_unpackhi_epi16(a[4], a[5]),
        _mm_unpacklo_epi16(a[6], a[7]),
        _mm_unpackhi_epi16(a[6], a[7]),
    ];
    // c[k]: columns 2k and 2k + 1 of rows 0..8; c[4 + k]: of rows 8..16.
    let c = [
        _mm_unpacklo_epi32(b[0], b[2]),
        _mm_unpackhi_epi32(b[0], b[2]),
        _mm_unpacklo_epi32(b[1], b[3]),
        _mm_unpackhi_epi32(b[1], b[3]),
        _mm_unpacklo_epi32(b[4], b[6]),
        _mm_unpackhi_epi32(b[4], b[6]),
        _mm_unpacklo_epi32(b[5], b[7]),
        _mm_unpackhi_epi32(b[5], b[7]),
    ];
    [
        _mm_unpacklo_epi64(c[0], c[4]),
        _mm_unpackhi_epi64(c[0], c[4]),
        _mm_unpacklo_epi64(c[1], c[5]),
        _mm_unpackhi_epi64(c[1], c[5]),
        _mm_unpacklo_epi64(c[2], c[6]),
        _mm_unpackhi_epi64(c[2], c[6]),
        _mm_unpacklo_epi64(c[3], c[7]),
        _mm_unpackhi_epi64(c[3], c[7]),
    ]
}

/// Taps back to rows: `out[i]` holds row `i` (taps 0..8) in its low half.
#[inline(always)]
unsafe fn taps_to_rows(t: &Taps) -> [__m128i; 16] {
    // d[2k]: taps 2k, 2k + 1 of rows 0..8; d[2k + 1]: of rows 8..16.
    let d = [
        _mm_unpacklo_epi8(t[0], t[1]),
        _mm_unpackhi_epi8(t[0], t[1]),
        _mm_unpacklo_epi8(t[2], t[3]),
        _mm_unpackhi_epi8(t[2], t[3]),
        _mm_unpacklo_epi8(t[4], t[5]),
        _mm_unpackhi_epi8(t[4], t[5]),
        _mm_unpacklo_epi8(t[6], t[7]),
        _mm_unpackhi_epi8(t[6], t[7]),
    ];
    let mut out = [_mm_setzero_si128(); 16];
    for half in 0..2 {
        let (d01, d23, d45, d67) = (d[half], d[2 + half], d[4 + half], d[6 + half]);
        // Rows 0..4 / 4..8 of this half: taps 0..4, then taps 4..8.
        let e0 = _mm_unpacklo_epi16(d01, d23);
        let e1 = _mm_unpackhi_epi16(d01, d23);
        let e2 = _mm_unpacklo_epi16(d45, d67);
        let e3 = _mm_unpackhi_epi16(d45, d67);
        let f = [
            _mm_unpacklo_epi32(e0, e2),
            _mm_unpackhi_epi32(e0, e2),
            _mm_unpacklo_epi32(e1, e3),
            _mm_unpackhi_epi32(e1, e3),
        ];
        for (k, v) in f.iter().enumerate() {
            out[half * 8 + 2 * k] = *v;
            out[half * 8 + 2 * k + 1] = _mm_unpackhi_epi64(*v, *v);
        }
    }
    out
}

/// `|a - b|` per byte.
#[inline(always)]
unsafe fn abd(a: __m128i, b: __m128i) -> __m128i {
    _mm_or_si128(_mm_subs_epu8(a, b), _mm_subs_epu8(b, a))
}

/// All ones where `a <= b` (unsigned bytes).
#[inline(always)]
unsafe fn le(a: __m128i, b: __m128i) -> __m128i {
    _mm_cmpeq_epi8(_mm_min_epu8(a, b), a)
}

/// `cond ? a : b` per byte (`cond` all ones or all zeros).
#[inline(always)]
unsafe fn sel(cond: __m128i, a: __m128i, b: __m128i) -> __m128i {
    _mm_or_si128(_mm_and_si128(cond, a), _mm_andnot_si128(cond, b))
}

/// Pixel to signed value and back (`v ^ 0x80`).
#[inline(always)]
unsafe fn flip(v: __m128i) -> __m128i {
    _mm_xor_si128(v, _mm_set1_epi8(-128))
}

/// The low and high eight signed bytes, sign-extended to 16 bits.
#[inline(always)]
unsafe fn widen(v: __m128i) -> (__m128i, __m128i) {
    (
        _mm_srai_epi16::<8>(_mm_unpacklo_epi8(v, v)),
        _mm_srai_epi16::<8>(_mm_unpackhi_epi8(v, v)),
    )
}

/// Signed bytes shifted right by 3, arithmetically.
#[inline(always)]
unsafe fn sra3(v: __m128i) -> __m128i {
    let lo = _mm_srai_epi16::<11>(_mm_unpacklo_epi8(v, v));
    let hi = _mm_srai_epi16::<11>(_mm_unpackhi_epi8(v, v));
    _mm_packs_epi16(lo, hi)
}

#[inline(always)]
unsafe fn simple_mask(t: &Taps, edge: u8) -> __m128i {
    let d = abd(t[3], t[4]);
    let half = _mm_and_si128(_mm_srli_epi16::<1>(abd(t[2], t[5])), _mm_set1_epi8(0x7f));
    le(
        _mm_adds_epu8(_mm_adds_epu8(d, d), half),
        _mm_set1_epi8(edge as i8),
    )
}

#[inline(always)]
unsafe fn normal_mask_hev(t: &Taps, l: Limits) -> (__m128i, __m128i) {
    let p1p0 = abd(t[2], t[3]);
    let q1q0 = abd(t[5], t[4]);
    let m = _mm_max_epu8(
        _mm_max_epu8(abd(t[0], t[1]), abd(t[1], t[2])),
        _mm_max_epu8(abd(t[7], t[6]), abd(t[6], t[5])),
    );
    let m = _mm_max_epu8(m, _mm_max_epu8(p1p0, q1q0));
    let mask = _mm_and_si128(
        simple_mask(t, l.edge),
        le(m, _mm_set1_epi8(l.interior as i8)),
    );
    let not_hev = le(_mm_max_epu8(p1p0, q1q0), _mm_set1_epi8(l.hev as i8));
    let hev = _mm_xor_si128(not_hev, _mm_set1_epi8(-1));
    (mask, hev)
}

/// `c(outer + 3 * (q0 - p0))` in 16 bits, narrowed with saturation.
#[inline(always)]
unsafe fn common(outer: __m128i, p0: __m128i, q0: __m128i) -> __m128i {
    let (q0l, q0h) = widen(q0);
    let (p0l, p0h) = widen(p0);
    let (ol, oh) = widen(outer);
    let three = _mm_set1_epi16(3);
    let lo = _mm_add_epi16(_mm_mullo_epi16(_mm_sub_epi16(q0l, p0l), three), ol);
    let hi = _mm_add_epi16(_mm_mullo_epi16(_mm_sub_epi16(q0h, p0h), three), oh);
    _mm_packs_epi16(lo, hi)
}

/// `c((k * w + 63) >> 7)`.
#[inline(always)]
unsafe fn tap_weight(w: __m128i, k: i16) -> __m128i {
    let (lo, hi) = widen(w);
    let (kk, r) = (_mm_set1_epi16(k), _mm_set1_epi16(63));
    let lo = _mm_srai_epi16::<7>(_mm_add_epi16(_mm_mullo_epi16(lo, kk), r));
    let hi = _mm_srai_epi16::<7>(_mm_add_epi16(_mm_mullo_epi16(hi, kk), r));
    _mm_packs_epi16(lo, hi)
}

/// `(f + 1) >> 1` on signed bytes.
#[inline(always)]
unsafe fn half_round(f: __m128i) -> __m128i {
    let (lo, hi) = widen(f);
    let one = _mm_set1_epi16(1);
    _mm_packs_epi16(
        _mm_srai_epi16::<1>(_mm_add_epi16(lo, one)),
        _mm_srai_epi16::<1>(_mm_add_epi16(hi, one)),
    )
}

#[inline(always)]
unsafe fn simple_filter(t: &mut Taps, l: Limits) {
    let mask = simple_mask(t, l.edge);
    let (p1, p0, q0, q1) = (flip(t[2]), flip(t[3]), flip(t[4]), flip(t[5]));
    let a = common(_mm_subs_epi8(p1, q1), p0, q0);
    let f1 = sra3(_mm_adds_epi8(a, _mm_set1_epi8(4)));
    let f2 = sra3(_mm_adds_epi8(a, _mm_set1_epi8(3)));
    t[4] = sel(mask, flip(_mm_subs_epi8(q0, f1)), t[4]);
    t[3] = sel(mask, flip(_mm_adds_epi8(p0, f2)), t[3]);
}

#[inline(always)]
unsafe fn mb_filter(t: &mut Taps, l: Limits) {
    let (mask, hev) = normal_mask_hev(t, l);
    let (p2, p1, p0) = (flip(t[1]), flip(t[2]), flip(t[3]));
    let (q0, q1, q2) = (flip(t[4]), flip(t[5]), flip(t[6]));
    let w = common(_mm_subs_epi8(p1, q1), p0, q0);
    let f1 = sra3(_mm_adds_epi8(w, _mm_set1_epi8(4)));
    let f2 = sra3(_mm_adds_epi8(w, _mm_set1_epi8(3)));
    let a27 = tap_weight(w, 27);
    let a18 = tap_weight(w, 18);
    let a9 = tap_weight(w, 9);
    let only = _mm_and_si128(mask, hev);
    let wide = _mm_andnot_si128(hev, mask);
    t[4] = sel(
        only,
        flip(_mm_subs_epi8(q0, f1)),
        sel(wide, flip(_mm_subs_epi8(q0, a27)), t[4]),
    );
    t[3] = sel(
        only,
        flip(_mm_adds_epi8(p0, f2)),
        sel(wide, flip(_mm_adds_epi8(p0, a27)), t[3]),
    );
    t[5] = sel(wide, flip(_mm_subs_epi8(q1, a18)), t[5]);
    t[2] = sel(wide, flip(_mm_adds_epi8(p1, a18)), t[2]);
    t[6] = sel(wide, flip(_mm_subs_epi8(q2, a9)), t[6]);
    t[1] = sel(wide, flip(_mm_adds_epi8(p2, a9)), t[1]);
}

#[inline(always)]
unsafe fn subblock_filter(t: &mut Taps, l: Limits) {
    let (mask, hev) = normal_mask_hev(t, l);
    let (p1, p0, q0, q1) = (flip(t[2]), flip(t[3]), flip(t[4]), flip(t[5]));
    // The outer taps count only at high edge variance.
    let outer = _mm_and_si128(_mm_subs_epi8(p1, q1), hev);
    let a = common(outer, p0, q0);
    let f1 = sra3(_mm_adds_epi8(a, _mm_set1_epi8(4)));
    let f2 = sra3(_mm_adds_epi8(a, _mm_set1_epi8(3)));
    let a2 = half_round(f1);
    let soft = _mm_andnot_si128(hev, mask);
    t[4] = sel(mask, flip(_mm_subs_epi8(q0, f1)), t[4]);
    t[3] = sel(mask, flip(_mm_adds_epi8(p0, f2)), t[3]);
    t[5] = sel(soft, flip(_mm_subs_epi8(q1, a2)), t[5]);
    t[2] = sel(soft, flip(_mm_adds_epi8(p1, a2)), t[2]);
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
/// Safety: `buf.len() >= 7 * s + 16`.
#[inline(always)]
unsafe fn h16(buf: &mut [u8], s: usize, kind: Kind, l: Limits) {
    let p = buf.as_mut_ptr();
    let mut t: Taps = [_mm_setzero_si128(); 8];
    for (k, v) in t.iter_mut().enumerate() {
        *v = _mm_loadu_si128(p.add(k * s) as *const __m128i);
    }
    filter(kind, &mut t, l);
    for (k, v) in t.iter().enumerate().take(7).skip(1) {
        _mm_storeu_si128(p.add(k * s) as *mut __m128i, *v);
    }
}

/// Filters 16 segments across a vertical edge: rows `0..8` of `a` and
/// `0..8` of `b` (stride `s`), each eight pixels starting at p3.
///
/// Safety: `a` and `b` each point to at least `7 * s + 8` bytes, and their
/// rows do not overlap.
#[inline(always)]
#[allow(clippy::needless_range_loop)]
unsafe fn v16(a: *mut u8, b: *mut u8, s: usize, kind: Kind, l: Limits) {
    let mut r = [_mm_setzero_si128(); 16];
    for i in 0..8 {
        r[i] = _mm_loadl_epi64(a.add(i * s) as *const __m128i);
        r[8 + i] = _mm_loadl_epi64(b.add(i * s) as *const __m128i);
    }
    let mut t = rows_to_taps(&r);
    filter(kind, &mut t, l);
    let r = taps_to_rows(&t);
    for i in 0..8 {
        _mm_storel_epi64(a.add(i * s) as *mut __m128i, r[i]);
        _mm_storel_epi64(b.add(i * s) as *mut __m128i, r[8 + i]);
    }
}

/// Filters 8 segments of `ub` and 8 of `vb` across a horizontal edge.
///
/// Safety: both slices hold at least `7 * s + 8` bytes.
#[inline(always)]
unsafe fn h8x2(ub: &mut [u8], vb: &mut [u8], s: usize, kind: Kind, l: Limits) {
    let (pu, pv) = (ub.as_mut_ptr(), vb.as_mut_ptr());
    let mut t: Taps = [_mm_setzero_si128(); 8];
    for (k, v) in t.iter_mut().enumerate() {
        *v = _mm_unpacklo_epi64(
            _mm_loadl_epi64(pu.add(k * s) as *const __m128i),
            _mm_loadl_epi64(pv.add(k * s) as *const __m128i),
        );
    }
    filter(kind, &mut t, l);
    for (k, v) in t.iter().enumerate().take(7).skip(1) {
        _mm_storel_epi64(pu.add(k * s) as *mut __m128i, *v);
        _mm_storel_epi64(pv.add(k * s) as *mut __m128i, _mm_unpackhi_epi64(*v, *v));
    }
}

/// [`crate::loop_filter`]'s luma edge: 16 segments, q0 of the first at `pos`.
pub(crate) fn luma_edge(buf: &mut [u8], pos: usize, s: usize, vertical: bool, kind: Kind, l: Limits) {
    if vertical {
        let block = &mut buf[pos - 4..pos - 4 + 15 * s + 8];
        let (top, bottom) = block.split_at_mut(8 * s);
        assert!(top.len() >= 7 * s + 8 && bottom.len() >= 7 * s + 8);
        // SAFETY: `top` holds rows 0..8 and `bottom` rows 8..16, each at
        // least `7 * s + 8` bytes (asserted), and they do not overlap.
        unsafe { v16(top.as_mut_ptr(), bottom.as_mut_ptr(), s, kind, l) }
    } else {
        let block = &mut buf[pos - 4 * s..pos + 3 * s + 16];
        // SAFETY: `block` is exactly `7 * s + 16` bytes.
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
        let u = &mut ub[pos - 4..pos - 4 + 7 * s + 8];
        let v = &mut vb[pos - 4..pos - 4 + 7 * s + 8];
        // SAFETY: each slice is exactly `7 * s + 8` bytes, and they are
        // different buffers.
        unsafe { v16(u.as_mut_ptr(), v.as_mut_ptr(), s, kind, l) }
    } else {
        let u = &mut ub[pos - 4 * s..pos + 3 * s + 8];
        let v = &mut vb[pos - 4 * s..pos + 3 * s + 8];
        // SAFETY: each slice is exactly `7 * s + 8` bytes.
        unsafe { h8x2(u, v, s, kind, l) }
    }
}
