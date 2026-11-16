//! The loop filter (RFC 6386 Section 15).
//!
//! Each edge is filtered as up to 16 segments of eight pixels (p3 to q3)
//! straddling it. The segments are gathered into a `Segs` block, one row
//! per tap and one lane per segment, filtered all at once, and the changed
//! taps written back. For chroma, the U edge fills lanes 0 to 7 and the
//! matching V edge lanes 8 to 15, so both planes are filtered together.
//!
//! The filters work in 8-bit lanes with saturating arithmetic, which is
//! exactly the RFC's clamping (`c()` in Section 15.2) and which the
//! compiler can turn into vector code. On aarch64 and x86_64 the gathering
//! and filtering use NEON or SSE2 instead (`neon.rs`, `sse2.rs`), with the
//! same results bit for bit; `--cfg vp8_portable` turns them off. Strides
//! under a segment's width, where the rows overlap, always take the
//! portable code (only the hidden `filter_mb` can pass them).

/// Per-macroblock filter parameters.
#[derive(Clone, Copy, Debug, Default)]
pub struct FilterParams {
    pub interior_limit: i32,
    pub hev_threshold: i32,
}

impl FilterParams {
    pub fn new(level: i32, sharpness: u8, keyframe: bool) -> Self {
        let mut interior = level;
        if sharpness != 0 {
            interior >>= if sharpness > 4 { 2 } else { 1 };
            interior = interior.min(9 - sharpness as i32);
        }
        interior = interior.max(1);
        let mut hev = 0;
        if level >= 40 {
            hev = 2;
        } else if level >= 15 {
            hev = 1;
        }
        if !keyframe && level >= 20 {
            hev += 1;
        }
        FilterParams {
            interior_limit: interior,
            hev_threshold: hev,
        }
    }
}

/// The pixels of up to 16 segments: `seg[k][i]` is tap `k` (0 = p3 ...
/// 7 = q3) of segment `i`.
pub(crate) type Segs = [[u8; 16]; 8];

/// The limits of one edge, as 8-bit values. For the RFC's levels (0 to
/// 63) the edge limit is at most (63 + 2) * 2 + 63 = 193 and the others at
/// most 63, so they fit; any other level is clamped to 0..=255.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Limits {
    pub(crate) edge: u8,
    pub(crate) interior: u8,
    pub(crate) hev: u8,
}

impl Limits {
    fn new(edge: i64, p: &FilterParams) -> Self {
        let byte = |v: i64| v.clamp(0, 255) as u8;
        Limits {
            edge: byte(edge),
            interior: byte(p.interior_limit.into()),
            hev: byte(p.hev_threshold.into()),
        }
    }

    /// The limits of a macroblock's own edges and of the edges between its
    /// subblocks (Section 15.2): edge limits `(level + 2) * 2 + interior`
    /// and `level * 2 + interior`. `filter_mb` is reachable from outside
    /// the crate, so `level` may be any `i32`: the sums are done in `i64`,
    /// where they can't overflow (they are within 3 * 2^31 + 4 of zero),
    /// and then clamped to a byte.
    fn for_level(level: i32, p: &FilterParams) -> (Self, Self) {
        let (level, interior) = (i64::from(level), i64::from(p.interior_limit));
        (
            Limits::new((level + 2) * 2 + interior, p),
            Limits::new(level * 2 + interior, p),
        )
    }
}

// ---------------------------------------------------------------------------
// The bytes an edge touches. `filter_mb` and `Planes` are reachable from
// outside the crate (the hidden shared modules), so strides and positions
// may be anything: every size is computed with checked arithmetic and an
// overflow panics, as an out-of-range index does.

/// `rows * s + width`: the bytes from the start of row 0 to the end of a
/// `width`-pixel run in row `rows` (stride `s`). Panics on overflow.
#[inline(always)]
pub(crate) fn rows_len(rows: usize, s: usize, width: usize) -> usize {
    rows.checked_mul(s)
        .and_then(|v| v.checked_add(width))
        .expect("loop filter: stride too large")
}

#[inline(always)]
fn add(a: usize, b: usize) -> usize {
    a.checked_add(b)
        .expect("loop filter: edge outside the plane")
}

#[inline(always)]
fn mul(a: usize, b: usize) -> usize {
    a.checked_mul(b)
        .expect("loop filter: edge outside the plane")
}

/// The range of an edge's segments: it starts `back_rows` rows and
/// `back_cols` pixels before `pos` and is `rows_len(rows, s, width)` long.
/// Panics on overflow or if it would start before 0.
#[inline(always)]
pub(crate) fn span(
    pos: usize,
    back_rows: usize,
    back_cols: usize,
    s: usize,
    rows: usize,
    width: usize,
) -> core::ops::Range<usize> {
    let back = rows_len(back_rows, s, back_cols);
    let start = pos
        .checked_sub(back)
        .expect("loop filter: edge outside the plane");
    let end = start
        .checked_add(rows_len(rows, s, width))
        .expect("loop filter: edge outside the plane");
    start..end
}

// ---------------------------------------------------------------------------
// Gathering and writing back (portable).

/// Gathers `N` segments across a horizontal edge (the taps are rows of
/// stride `s`; `pos` is q0 of the first segment) into lanes `lane..lane+N`.
#[inline(always)]
pub(crate) fn load_h<const N: usize>(
    buf: &[u8],
    pos: usize,
    s: usize,
    seg: &mut Segs,
    lane: usize,
) {
    let src = &buf[span(pos, 4, 0, s, 7, N)];
    for (k, row) in seg.iter_mut().enumerate() {
        let r: &[u8; N] = src[k * s..k * s + N].try_into().expect("N pixels");
        row[lane..lane + N].copy_from_slice(r);
    }
}

/// Writes taps `first..last` of lanes `lane..lane+N` back across a
/// horizontal edge.
#[inline(always)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn store_h<const N: usize>(
    buf: &mut [u8],
    pos: usize,
    s: usize,
    seg: &Segs,
    lane: usize,
    first: usize,
    last: usize,
) {
    let dst = &mut buf[span(pos, 4, 0, s, 7, N)];
    for (k, row) in seg.iter().enumerate().take(last).skip(first) {
        let r: &[u8; N] = row[lane..lane + N].try_into().expect("N lanes");
        dst[k * s..k * s + N].copy_from_slice(r);
    }
}

/// Gathers `N` segments across a vertical edge (each segment is eight
/// pixels of one row; the rows are `s` apart).
#[inline(always)]
pub(crate) fn load_v<const N: usize>(
    buf: &[u8],
    pos: usize,
    s: usize,
    seg: &mut Segs,
    lane: usize,
) {
    let src = &buf[span(pos, 0, 4, s, N - 1, 8)];
    for i in 0..N {
        let px: &[u8; 8] = src[i * s..i * s + 8].try_into().expect("8 pixels");
        for (row, &v) in seg.iter_mut().zip(px.iter()) {
            row[lane + i] = v;
        }
    }
}

/// Writes taps `first..last` back across a vertical edge.
#[inline(always)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn store_v<const N: usize>(
    buf: &mut [u8],
    pos: usize,
    s: usize,
    seg: &Segs,
    lane: usize,
    first: usize,
    last: usize,
) {
    let dst = &mut buf[span(pos, 0, 4, s, N - 1, 8)];
    for i in 0..N {
        let px: &mut [u8; 8] = (&mut dst[i * s..i * s + 8]).try_into().expect("8 pixels");
        for k in first..last {
            px[k] = seg[k][lane + i];
        }
    }
}

// ---------------------------------------------------------------------------
// The filters on 16 lanes (portable). Each lane is independent and the
// code has no branches, so it vectorises.

/// Pixel to signed value: `v - 128`, as the reference decoder's `u2s`.
#[inline(always)]
fn s(v: u8) -> i8 {
    (v ^ 0x80) as i8
}

/// Signed value back to a pixel.
#[inline(always)]
fn u(v: i8) -> u8 {
    (v as u8) ^ 0x80
}

#[inline(always)]
fn clamp8(v: i16) -> i8 {
    v.clamp(-128, 127) as i8
}

#[inline(always)]
fn pick(cond: bool, a: u8, b: u8) -> u8 {
    if cond {
        a
    } else {
        b
    }
}

/// `|p0 - q0| * 2 + |p1 - q1| / 2 <= edge`, in saturating 8-bit steps: a
/// saturated sum is 255, which is above any edge limit, so the answer is
/// the same as in wide arithmetic.
#[inline(always)]
fn simple_mask(p1: u8, p0: u8, q0: u8, q1: u8, edge: u8) -> bool {
    let d = p0.abs_diff(q0);
    d.saturating_add(d).saturating_add(p1.abs_diff(q1) >> 1) <= edge
}

/// The common filter value `c(c(P1 - Q1) + 3 * (Q0 - P0))`, with the outer
/// taps zeroed when `outer` is false.
#[inline(always)]
fn common(p1: i8, p0: i8, q0: i8, q1: i8, outer: bool) -> i8 {
    let o = if outer { p1.saturating_sub(q1) } else { 0 };
    clamp8(o as i16 + 3 * (q0 as i16 - p0 as i16))
}

/// The simple filter (luma only) on all 16 lanes.
pub(crate) fn simple_lanes(seg: &mut Segs, l: Limits) {
    let [_, _, p1r, p0r, q0r, q1r, _, _] = seg;
    for i in 0..16 {
        let (p1, p0, q0, q1) = (p1r[i], p0r[i], q0r[i], q1r[i]);
        let mask = simple_mask(p1, p0, q0, q1, l.edge);
        let a = common(s(p1), s(p0), s(q0), s(q1), true);
        let f1 = a.saturating_add(4) >> 3;
        let f2 = a.saturating_add(3) >> 3;
        q0r[i] = pick(mask, u(s(q0).saturating_sub(f1)), q0);
        p0r[i] = pick(mask, u(s(p0).saturating_add(f2)), p0);
    }
}

/// The normal filter's mask and high-edge-variance flag for one lane.
#[inline(always)]
fn normal_mask_hev(px: [u8; 8], l: Limits) -> (bool, bool) {
    let [p3, p2, p1, p0, q0, q1, q2, q3] = px;
    let i = l.interior;
    let mask = simple_mask(p1, p0, q0, q1, l.edge)
        & (p3.abs_diff(p2) <= i)
        & (p2.abs_diff(p1) <= i)
        & (p1.abs_diff(p0) <= i)
        & (q3.abs_diff(q2) <= i)
        & (q2.abs_diff(q1) <= i)
        & (q1.abs_diff(q0) <= i);
    let hev = (p1.abs_diff(p0) > l.hev) | (q1.abs_diff(q0) > l.hev);
    (mask, hev)
}

/// The normal filter across a macroblock edge, on all 16 lanes.
pub(crate) fn mb_lanes(seg: &mut Segs, l: Limits) {
    let [p3r, p2r, p1r, p0r, q0r, q1r, q2r, q3r] = seg;
    for i in 0..16 {
        let px = [
            p3r[i], p2r[i], p1r[i], p0r[i], q0r[i], q1r[i], q2r[i], q3r[i],
        ];
        let (mask, high) = normal_mask_hev(px, l);
        let [_, p2, p1, p0, q0, q1, q2, _] = px;
        let w = common(s(p1), s(p0), s(q0), s(q1), true);
        // High edge variance: only p0 and q0 move.
        let f1 = w.saturating_add(4) >> 3;
        let f2 = w.saturating_add(3) >> 3;
        // Otherwise three pixels on each side, by 27, 18 and 9 / 128 of w.
        let a27 = clamp8((27 * w as i16 + 63) >> 7);
        let a18 = clamp8((18 * w as i16 + 63) >> 7);
        let a9 = clamp8((9 * w as i16 + 63) >> 7);
        let only = mask & high;
        let wide = mask & !high;
        q0r[i] = pick(
            only,
            u(s(q0).saturating_sub(f1)),
            pick(wide, u(s(q0).saturating_sub(a27)), q0),
        );
        p0r[i] = pick(
            only,
            u(s(p0).saturating_add(f2)),
            pick(wide, u(s(p0).saturating_add(a27)), p0),
        );
        q1r[i] = pick(wide, u(s(q1).saturating_sub(a18)), q1);
        p1r[i] = pick(wide, u(s(p1).saturating_add(a18)), p1);
        q2r[i] = pick(wide, u(s(q2).saturating_sub(a9)), q2);
        p2r[i] = pick(wide, u(s(p2).saturating_add(a9)), p2);
    }
}

/// The normal filter across an edge between subblocks, on all 16 lanes.
pub(crate) fn subblock_lanes(seg: &mut Segs, l: Limits) {
    let [p3r, p2r, p1r, p0r, q0r, q1r, q2r, q3r] = seg;
    for i in 0..16 {
        let px = [
            p3r[i], p2r[i], p1r[i], p0r[i], q0r[i], q1r[i], q2r[i], q3r[i],
        ];
        let (mask, high) = normal_mask_hev(px, l);
        let [_, _, p1, p0, q0, q1, _, _] = px;
        // The outer taps count only at high edge variance.
        let a = common(s(p1), s(p0), s(q0), s(q1), high);
        let f1 = a.saturating_add(4) >> 3;
        let f2 = a.saturating_add(3) >> 3;
        let a2 = (f1 + 1) >> 1;
        let soft = mask & !high;
        q0r[i] = pick(mask, u(s(q0).saturating_sub(f1)), q0);
        p0r[i] = pick(mask, u(s(p0).saturating_add(f2)), p0);
        q1r[i] = pick(soft, u(s(q1).saturating_sub(a2)), q1);
        p1r[i] = pick(soft, u(s(p1).saturating_add(a2)), p1);
    }
}

// ---------------------------------------------------------------------------
// Edges.

/// Which filter an edge gets.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    Simple,
    Macroblock,
    Subblock,
}

impl Kind {
    /// The taps the filter may change.
    fn taps(self) -> (usize, usize) {
        match self {
            Kind::Simple => (3, 5),
            Kind::Macroblock => (1, 7),
            Kind::Subblock => (2, 6),
        }
    }
}

#[inline(always)]
fn run(kind: Kind, seg: &mut Segs, l: Limits) {
    match kind {
        Kind::Simple => simple_lanes(seg, l),
        Kind::Macroblock => mb_lanes(seg, l),
        Kind::Subblock => subblock_lanes(seg, l),
    }
}

/// One luma edge of 16 segments. `vertical`: the edge is vertical (the
/// segments are rows).
#[inline(always)]
fn luma_edge<const PORTABLE: bool>(
    buf: &mut [u8],
    pos: usize,
    s: usize,
    vertical: bool,
    kind: Kind,
    l: Limits,
) {
    #[cfg(any(
        all(target_arch = "aarch64", target_feature = "neon", not(vp8_portable)),
        all(target_arch = "x86_64", target_feature = "sse2", not(vp8_portable))
    ))]
    if !PORTABLE {
        crate::simd::luma_edge(buf, pos, s, vertical, kind, l);
        return;
    }
    luma_edge_portable(buf, pos, s, vertical, kind, l);
}

/// [`luma_edge`] without SIMD intrinsics.
pub(crate) fn luma_edge_portable(
    buf: &mut [u8],
    pos: usize,
    s: usize,
    vertical: bool,
    kind: Kind,
    l: Limits,
) {
    let mut seg = [[0u8; 16]; 8];
    let (first, last) = kind.taps();
    if vertical {
        load_v::<16>(buf, pos, s, &mut seg, 0);
        run(kind, &mut seg, l);
        store_v::<16>(buf, pos, s, &seg, 0, first, last);
    } else {
        load_h::<16>(buf, pos, s, &mut seg, 0);
        run(kind, &mut seg, l);
        store_h::<16>(buf, pos, s, &seg, 0, first, last);
    }
}

/// The same chroma edge in U and V (eight segments each), filtered together.
#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn chroma_edge<const PORTABLE: bool>(
    ub: &mut [u8],
    vb: &mut [u8],
    pos: usize,
    s: usize,
    vertical: bool,
    kind: Kind,
    l: Limits,
) {
    #[cfg(any(
        all(target_arch = "aarch64", target_feature = "neon", not(vp8_portable)),
        all(target_arch = "x86_64", target_feature = "sse2", not(vp8_portable))
    ))]
    if !PORTABLE {
        crate::simd::chroma_edge(ub, vb, pos, s, vertical, kind, l);
        return;
    }
    chroma_edge_portable(ub, vb, pos, s, vertical, kind, l);
}

/// [`chroma_edge`] without SIMD intrinsics.
#[allow(clippy::too_many_arguments)]
pub(crate) fn chroma_edge_portable(
    ub: &mut [u8],
    vb: &mut [u8],
    pos: usize,
    s: usize,
    vertical: bool,
    kind: Kind,
    l: Limits,
) {
    let mut seg = [[0u8; 16]; 8];
    let (first, last) = kind.taps();
    if vertical {
        load_v::<8>(ub, pos, s, &mut seg, 0);
        load_v::<8>(vb, pos, s, &mut seg, 8);
        run(kind, &mut seg, l);
        store_v::<8>(ub, pos, s, &seg, 0, first, last);
        store_v::<8>(vb, pos, s, &seg, 8, first, last);
    } else {
        load_h::<8>(ub, pos, s, &mut seg, 0);
        load_h::<8>(vb, pos, s, &mut seg, 8);
        run(kind, &mut seg, l);
        store_h::<8>(ub, pos, s, &seg, 0, first, last);
        store_h::<8>(vb, pos, s, &seg, 8, first, last);
    }
}

/// The three planes of a frame being filtered.
pub struct Planes<'a> {
    pub y: &'a mut [u8],
    pub u: &'a mut [u8],
    pub v: &'a mut [u8],
    pub y_stride: usize,
    pub uv_stride: usize,
}

/// Filters the edges one macroblock is responsible for (Section 15.1):
/// its left and top edges and, if `inner`, the edges between its subblocks.
/// `level` is the macroblock's final filter level (non-zero).
#[allow(clippy::too_many_arguments)]
pub fn filter_mb(
    planes: Planes<'_>,
    row: usize,
    col: usize,
    level: i32,
    sharpness: u8,
    keyframe: bool,
    inner: bool,
    simple: bool,
) {
    filter_mb_impl::<false>(planes, row, col, level, sharpness, keyframe, inner, simple);
}

/// [`filter_mb`], with `PORTABLE` forcing the code without intrinsics
/// (for tests that compare the two).
#[allow(clippy::too_many_arguments)]
pub(crate) fn filter_mb_impl<const PORTABLE: bool>(
    planes: Planes<'_>,
    row: usize,
    col: usize,
    level: i32,
    sharpness: u8,
    keyframe: bool,
    inner: bool,
    simple: bool,
) {
    let p = FilterParams::new(level, sharpness, keyframe);
    let (mb, sub) = Limits::for_level(level, &p);
    let s = planes.y_stride;
    let pos = rows_len(mul(row, 16), s, mul(col, 16));
    let y = planes.y;
    if simple {
        if col > 0 {
            luma_edge::<PORTABLE>(y, pos, s, true, Kind::Simple, mb);
        }
        if inner {
            for k in [4, 8, 12] {
                luma_edge::<PORTABLE>(y, add(pos, k), s, true, Kind::Simple, sub);
            }
        }
        if row > 0 {
            luma_edge::<PORTABLE>(y, pos, s, false, Kind::Simple, mb);
        }
        if inner {
            for k in [4, 8, 12] {
                luma_edge::<PORTABLE>(y, rows_len(k, s, pos), s, false, Kind::Simple, sub);
            }
        }
        return;
    }
    // Luma, then chroma, each in the order the RFC gives: left edge, inner
    // vertical edges, top edge, inner horizontal edges. The planes are
    // independent, so filtering U and V together changes nothing.
    if col > 0 {
        luma_edge::<PORTABLE>(y, pos, s, true, Kind::Macroblock, mb);
    }
    if inner {
        for k in [4, 8, 12] {
            luma_edge::<PORTABLE>(y, add(pos, k), s, true, Kind::Subblock, sub);
        }
    }
    if row > 0 {
        luma_edge::<PORTABLE>(y, pos, s, false, Kind::Macroblock, mb);
    }
    if inner {
        for k in [4, 8, 12] {
            luma_edge::<PORTABLE>(y, rows_len(k, s, pos), s, false, Kind::Subblock, sub);
        }
    }
    let cs = planes.uv_stride;
    let cpos = rows_len(mul(row, 8), cs, mul(col, 8));
    let (ub, vb) = (planes.u, planes.v);
    if col > 0 {
        chroma_edge::<PORTABLE>(ub, vb, cpos, cs, true, Kind::Macroblock, mb);
    }
    if inner {
        chroma_edge::<PORTABLE>(ub, vb, add(cpos, 4), cs, true, Kind::Subblock, sub);
    }
    if row > 0 {
        chroma_edge::<PORTABLE>(ub, vb, cpos, cs, false, Kind::Macroblock, mb);
    }
    if inner {
        chroma_edge::<PORTABLE>(
            ub,
            vb,
            rows_len(4, cs, cpos),
            cs,
            false,
            Kind::Subblock,
            sub,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference::loop_filter as reference;

    /// A small xorshift generator.
    pub(crate) struct Rng(pub(crate) u32);

    impl Rng {
        pub(crate) fn next(&mut self) -> u32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 17;
            self.0 ^= self.0 << 5;
            self.0
        }
    }

    /// Pixels that exercise every branch: smooth areas with small steps
    /// (filters on) mixed with random jumps (filters off, high variance).
    fn picture(rng: &mut Rng, len: usize) -> Vec<u8> {
        let mut v = Vec::with_capacity(len);
        let mut cur = 128i32;
        for _ in 0..len {
            let r = rng.next();
            cur = match r % 8 {
                0 => (r >> 8) as i32 & 255,
                1..=3 => cur + ((r >> 8) % 41) as i32 - 20,
                _ => cur + ((r >> 8) % 7) as i32 - 3,
            }
            .clamp(0, 255);
            v.push(cur as u8);
        }
        v
    }

    /// Filters every macroblock of a 3x3-macroblock frame, in decode order.
    fn filter_frame(
        which: u8,
        planes: (&mut [u8], &mut [u8], &mut [u8]),
        level: i32,
        sharpness: u8,
        keyframe: bool,
        inner: bool,
        simple: bool,
    ) {
        let (y, u, v) = planes;
        for row in 0..3 {
            for col in 0..3 {
                let args = (row, col, level, sharpness, keyframe, inner, simple);
                match which {
                    0 => reference::filter_mb(
                        reference::Planes {
                            y: &mut *y,
                            u: &mut *u,
                            v: &mut *v,
                            y_stride: 48,
                            uv_stride: 24,
                        },
                        args.0,
                        args.1,
                        args.2,
                        args.3,
                        args.4,
                        args.5,
                        args.6,
                    ),
                    1 => filter_mb_impl::<true>(
                        Planes {
                            y: &mut *y,
                            u: &mut *u,
                            v: &mut *v,
                            y_stride: 48,
                            uv_stride: 24,
                        },
                        args.0,
                        args.1,
                        args.2,
                        args.3,
                        args.4,
                        args.5,
                        args.6,
                    ),
                    _ => filter_mb(
                        Planes {
                            y: &mut *y,
                            u: &mut *u,
                            v: &mut *v,
                            y_stride: 48,
                            uv_stride: 24,
                        },
                        args.0,
                        args.1,
                        args.2,
                        args.3,
                        args.4,
                        args.5,
                        args.6,
                    ),
                }
            }
        }
    }

    /// Every edge kind, both directions, luma and chroma, at every level,
    /// sharpness and frame type: the portable filter and the platform's
    /// (NEON on aarch64) match the scalar reference filter byte for byte.
    #[test]
    #[cfg_attr(
        miri,
        ignore = "slow under Miri; edge_strides_and_sizes covers the SIMD paths"
    )]
    fn portable_and_simd_match_the_reference_filter() {
        let mut rng = Rng(0x9e37_79b9);
        let mut changed = 0u32;
        let mut cases = 0u32;
        for level in 1..64 {
            for sharpness in 0..8u8 {
                for keyframe in [false, true] {
                    for simple in [false, true] {
                        for inner in [false, true] {
                            let y = picture(&mut rng, 48 * 48);
                            let u = picture(&mut rng, 24 * 24);
                            let v = picture(&mut rng, 24 * 24);
                            let mut out = Vec::new();
                            for which in 0..3u8 {
                                let (mut y, mut u, mut v) = (y.clone(), u.clone(), v.clone());
                                filter_frame(
                                    which,
                                    (&mut y, &mut u, &mut v),
                                    level,
                                    sharpness,
                                    keyframe,
                                    inner,
                                    simple,
                                );
                                out.push((y, u, v));
                            }
                            let what = format!(
                                "level {level} sharpness {sharpness} key {keyframe} simple {simple} inner {inner}"
                            );
                            changed += (out[0].0 != y) as u32;
                            cases += 1;
                            for (name, o) in [("portable", &out[1]), ("platform", &out[2])] {
                                assert!(o.0 == out[0].0, "{name} luma differs: {what}");
                                assert!(o.1 == out[0].1, "{name} U differs: {what}");
                                assert!(o.2 == out[0].2, "{name} V differs: {what}");
                            }
                        }
                    }
                }
            }
        }
        // The test pictures must actually exercise the filters.
        assert!(
            changed * 10 > cases * 9,
            "only {changed} of {cases} cases changed the luma"
        );
    }

    /// The lane filters match the reference on random segments at every
    /// limit, across a horizontal edge and, transposed, a vertical one.
    #[test]
    #[cfg_attr(miri, ignore = "slow under Miri")]
    fn lanes_match_the_reference_in_both_directions() {
        let mut rng = Rng(0x1234_5677);
        for round in 0..4000u32 {
            let pixels = picture(&mut rng, 8 * 16);
            let level = (round % 63 + 1) as i32;
            let (sharp, key) = ((round / 63 % 8) as u8, round % 2 == 0);
            let p = FilterParams::new(level, sharp, key);
            let rp = reference::FilterParams::new(level, sharp, key);
            let edge = if round % 3 == 0 {
                (level + 2) * 2 + p.interior_limit
            } else {
                level * 2 + p.interior_limit
            };
            let l = Limits::new(edge.into(), &p);
            for kind in [Kind::Simple, Kind::Macroblock, Kind::Subblock] {
                // The reference on a horizontal edge of 16 columns.
                let mut want = pixels.clone();
                match kind {
                    Kind::Simple => reference::simple_edge(&mut want, 4 * 16, 16, 1, 16, edge),
                    Kind::Macroblock => reference::mb_edge(&mut want, 4 * 16, 16, 1, 16, edge, &rp),
                    Kind::Subblock => {
                        reference::subblock_edge(&mut want, 4 * 16, 16, 1, 16, edge, &rp)
                    }
                }
                let mut seg = [[0u8; 16]; 8];
                for (k, row) in seg.iter_mut().enumerate() {
                    row.copy_from_slice(&pixels[k * 16..k * 16 + 16]);
                }
                run(kind, &mut seg, l);
                let lanes: Vec<u8> = seg.iter().flatten().copied().collect();
                assert_eq!(lanes, want, "{kind:?} lanes, round {round}");

                let mut h = pixels.clone();
                luma_edge_portable(&mut h, 4 * 16, 16, false, kind, l);
                assert_eq!(h, want, "{kind:?} portable horizontal, round {round}");

                // The same segments as rows of eight across a vertical edge.
                let t = |px: &[u8]| -> Vec<u8> {
                    (0..16)
                        .flat_map(|i| (0..8).map(move |k| px[k * 16 + i]))
                        .collect()
                };
                let mut v = t(&pixels);
                luma_edge_portable(&mut v, 4, 8, true, kind, l);
                assert_eq!(v, t(&want), "{kind:?} portable vertical, round {round}");
                #[cfg(any(
                    all(target_arch = "aarch64", target_feature = "neon", not(vp8_portable)),
                    all(target_arch = "x86_64", target_feature = "sse2", not(vp8_portable))
                ))]
                {
                    let mut v = t(&pixels);
                    crate::simd::luma_edge(&mut v, 4, 8, true, kind, l);
                    assert_eq!(v, t(&want), "{kind:?} SIMD vertical, round {round}");
                    let mut h = pixels.clone();
                    crate::simd::luma_edge(&mut h, 4 * 16, 16, false, kind, l);
                    assert_eq!(h, want, "{kind:?} SIMD horizontal, round {round}");
                }
            }
        }
    }

    /// Runs `filter_mb` (the platform's path, or the portable one) on
    /// planes the caller made and reports whether it panicked.
    #[cfg(target_pointer_width = "64")]
    #[allow(clippy::too_many_arguments)]
    fn panics(
        portable: bool,
        y: &mut [u8],
        u: &mut [u8],
        v: &mut [u8],
        y_stride: usize,
        uv_stride: usize,
        row: usize,
        col: usize,
    ) -> bool {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let planes = Planes {
                y,
                u,
                v,
                y_stride,
                uv_stride,
            };
            if portable {
                filter_mb_impl::<true>(planes, row, col, 63, 0, true, true, false);
            } else {
                filter_mb(planes, row, col, 63, 0, true, true, false);
            }
        }))
        .is_err()
    }

    /// A stride whose arithmetic overflows (7 * s wraps to 1) must panic,
    /// on the SIMD path and the portable one, and never touch memory
    /// outside the planes. So must other huge strides and positions.
    #[test]
    #[cfg(target_pointer_width = "64")]
    fn overflowing_strides_and_positions_panic() {
        let wraps = 0x6db6_db6d_b6db_6db7usize;
        assert_eq!(wraps.wrapping_mul(7), 1);
        for portable in [false, true] {
            let cases: [(usize, usize, usize, usize); 5] = [
                // (y_stride, uv_stride, row, col)
                (16, wraps, 0, 0),
                (wraps, 8, 0, 0),
                (16, usize::MAX, 0, 0),
                (usize::MAX / 3, 8, 0, 1),
                (16, 8, usize::MAX / 8, 1),
            ];
            for (ys, cs, row, col) in cases {
                let (mut y, mut u, mut v) = (vec![0x80u8; 256], vec![0x80u8; 64], vec![0x80u8; 64]);
                assert!(
                    panics(portable, &mut y, &mut u, &mut v, ys, cs, row, col),
                    "no panic: portable {portable}, strides {ys:#x} {cs:#x}, mb ({row}, {col})"
                );
            }
        }
    }

    /// Strides under a segment's width (rows that overlap: under 16 pixels
    /// across a horizontal luma edge, under eight otherwise) give the same
    /// bytes on every path: the SIMD path hands them to the portable code.
    #[test]
    fn tiny_strides_match_the_portable_code() {
        let mut rng = Rng(0x0bad_cafe);
        let p = FilterParams::new(30, 0, false);
        let l = Limits::new((30 * 2 + p.interior_limit).into(), &p);
        for s in [0usize, 1, 3, 7, 8, 9, 15, 16] {
            for kind in [Kind::Simple, Kind::Macroblock, Kind::Subblock] {
                for vertical in [false, true] {
                    // q0 of the first segment: four pixels (vertical) or four
                    // rows (horizontal) in, with 16 bytes of slack at the end.
                    let pos = if vertical { 4 } else { 4 * s };
                    let len = 15 * s + 16 + 16;
                    let buf = picture(&mut rng, len);
                    let mut a = buf.clone();
                    let mut b = buf.clone();
                    luma_edge_portable(&mut a, pos, s, vertical, kind, l);
                    luma_edge::<false>(&mut b, pos, s, vertical, kind, l);
                    assert_eq!(a, b, "luma {kind:?} vertical {vertical} stride {s}");

                    let (u, v) = (picture(&mut rng, len), picture(&mut rng, len));
                    let (mut ua, mut va) = (u.clone(), v.clone());
                    let (mut ub, mut vb) = (u.clone(), v.clone());
                    chroma_edge_portable(&mut ua, &mut va, pos, s, vertical, kind, l);
                    chroma_edge::<false>(&mut ub, &mut vb, pos, s, vertical, kind, l);
                    assert_eq!(ua, ub, "U {kind:?} vertical {vertical} stride {s}");
                    assert_eq!(va, vb, "V {kind:?} vertical {vertical} stride {s}");
                }
            }
        }
    }

    /// Whole macroblocks through `filter_mb` at strides whose rows overlap
    /// (luma under 16, chroma under 8): the platform path matches the
    /// portable one byte for byte.
    #[test]
    fn overlapping_strides_through_filter_mb_match_the_portable_code() {
        let mut rng = Rng(0x0dd5_7a1d);
        let mut changed = 0;
        for (ys, cs) in [(1usize, 1usize), (4, 3), (8, 7), (15, 7), (15, 8), (16, 4)] {
            for (row, col) in [(0usize, 0usize), (1, 1)] {
                for simple in [false, true] {
                    let y = picture(&mut rng, 40 * ys + 64);
                    let u = picture(&mut rng, 20 * cs + 32);
                    let v = picture(&mut rng, 20 * cs + 32);
                    let mut out = Vec::new();
                    for portable in [true, false] {
                        let (mut y, mut u, mut v) = (y.clone(), u.clone(), v.clone());
                        let planes = Planes {
                            y: &mut y,
                            u: &mut u,
                            v: &mut v,
                            y_stride: ys,
                            uv_stride: cs,
                        };
                        if portable {
                            filter_mb_impl::<true>(planes, row, col, 40, 0, false, true, simple);
                        } else {
                            filter_mb(planes, row, col, 40, 0, false, true, simple);
                        }
                        out.push((y, u, v));
                    }
                    let what = format!("strides {ys}/{cs}, mb ({row}, {col}), simple {simple}");
                    assert!(out[1].0 == out[0].0, "luma differs: {what}");
                    assert!(out[1].1 == out[0].1, "U differs: {what}");
                    assert!(out[1].2 == out[0].2, "V differs: {what}");
                    changed += (out[0].0 != y) as u32;
                }
            }
        }
        assert!(changed > 0, "no case changed the luma");
    }

    /// The edge limits at the ends of `i32`, and where the old `i32` sum
    /// `(level + 2) * 2 + interior` overflowed (at sharpness 0 the
    /// interior limit is `level`, so the sum is `3 * level + 4`): computed
    /// exactly, then clamped. `filter_mb` doesn't panic at any of them,
    /// and its platform and portable paths agree.
    #[test]
    fn extreme_levels_give_clamped_limits() {
        // The largest level where 3 * level + 4 still fits in an i32.
        let last_fit: i32 = 715_827_881;
        let sum = |level: i32| level.checked_mul(3).and_then(|v| v.checked_add(4));
        assert!(sum(last_fit).is_some(), "{last_fit} must fit");
        assert!(sum(last_fit + 1).is_none(), "{} must overflow", last_fit + 1);
        let levels = [
            i32::MIN,
            i32::MIN + 1,
            -1,
            0,
            1,
            63,
            64,
            last_fit,
            last_fit + 1,
            i32::MAX / 2 - 2,
            i32::MAX / 2 - 1,
            i32::MAX - 1,
            i32::MAX,
        ];
        let mut rng = Rng(0x7e57_0001);
        for level in levels {
            for sharpness in [0u8, 7] {
                for keyframe in [false, true] {
                    let p = FilterParams::new(level, sharpness, keyframe);
                    let (mb, sub) = Limits::for_level(level, &p);
                    let wide = |extra: i128| -> u8 {
                        (i128::from(level) * 2 + extra + i128::from(p.interior_limit))
                            .clamp(0, 255) as u8
                    };
                    let what = format!("level {level} sharpness {sharpness} key {keyframe}");
                    assert_eq!(mb.edge, wide(4), "macroblock edge limit: {what}");
                    assert_eq!(sub.edge, wide(0), "subblock edge limit: {what}");
                    assert_eq!(mb.interior, sub.interior, "{what}");
                    assert_eq!(mb.hev, sub.hev, "{what}");

                    let (y, u, v) = (
                        picture(&mut rng, 48 * 48),
                        picture(&mut rng, 24 * 24),
                        picture(&mut rng, 24 * 24),
                    );
                    let mut out = Vec::new();
                    for which in 1..3u8 {
                        let (mut y, mut u, mut v) = (y.clone(), u.clone(), v.clone());
                        filter_frame(
                            which,
                            (&mut y, &mut u, &mut v),
                            level,
                            sharpness,
                            keyframe,
                            true,
                            false,
                        );
                        out.push((y, u, v));
                    }
                    assert!(out[0] == out[1], "portable and platform differ: {what}");
                }
            }
        }
        // At the RFC's top level the limits are unchanged.
        let p = FilterParams::new(63, 0, true);
        let (mb, sub) = Limits::for_level(63, &p);
        assert_eq!((mb.edge, sub.edge, mb.interior, mb.hev), (193, 189, 63, 2));
    }

    /// Frames of one macroblock to several, with strides from the tightest
    /// possible to odd and wide ones, in buffers exactly as long as the
    /// planes need (so an over-read or over-write is an index panic, or
    /// undefined behaviour Miri reports): the portable and platform paths
    /// match the reference byte for byte.
    #[test]
    fn edge_strides_and_sizes_match_the_reference() {
        let mut rng = Rng(0x5eed_1234);
        let sizes: &[(usize, usize)] = if cfg!(miri) {
            &[(1, 1), (2, 1), (1, 2)]
        } else {
            &[(1, 1), (1, 3), (3, 1), (2, 2), (5, 2), (2, 5)]
        };
        let extras: &[usize] = if cfg!(miri) {
            &[0, 7]
        } else {
            &[0, 1, 7, 15, 33]
        };
        let levels: &[i32] = if cfg!(miri) { &[40] } else { &[1, 20, 40, 63] };
        let mut cases = 0;
        for &(cols, rows) in sizes {
            for &extra in extras {
                let (ys, cs) = (16 * cols + extra, 8 * cols + extra / 2);
                // Exactly long enough: the last row ends at the buffer's end.
                let (ylen, clen) = (
                    ys * (16 * rows - 1) + 16 * cols,
                    cs * (8 * rows - 1) + 8 * cols,
                );
                for &level in levels {
                    for simple in [false, true] {
                        let y = picture(&mut rng, ylen);
                        let u = picture(&mut rng, clen);
                        let v = picture(&mut rng, clen);
                        let mut out = Vec::new();
                        for which in 0..3u8 {
                            let (mut y, mut u, mut v) = (y.clone(), u.clone(), v.clone());
                            for row in 0..rows {
                                for col in 0..cols {
                                    let args = (row, col, level, 3, false, true, simple);
                                    match which {
                                        0 => reference::filter_mb(
                                            reference::Planes {
                                                y: &mut y,
                                                u: &mut u,
                                                v: &mut v,
                                                y_stride: ys,
                                                uv_stride: cs,
                                            },
                                            args.0,
                                            args.1,
                                            args.2,
                                            args.3,
                                            args.4,
                                            args.5,
                                            args.6,
                                        ),
                                        1 => filter_mb_impl::<true>(
                                            Planes {
                                                y: &mut y,
                                                u: &mut u,
                                                v: &mut v,
                                                y_stride: ys,
                                                uv_stride: cs,
                                            },
                                            args.0,
                                            args.1,
                                            args.2,
                                            args.3,
                                            args.4,
                                            args.5,
                                            args.6,
                                        ),
                                        _ => filter_mb(
                                            Planes {
                                                y: &mut y,
                                                u: &mut u,
                                                v: &mut v,
                                                y_stride: ys,
                                                uv_stride: cs,
                                            },
                                            args.0,
                                            args.1,
                                            args.2,
                                            args.3,
                                            args.4,
                                            args.5,
                                            args.6,
                                        ),
                                    }
                                }
                            }
                            out.push((y, u, v));
                        }
                        let what = format!(
                            "{cols}x{rows} MBs, strides {ys}/{cs}, level {level}, simple {simple}"
                        );
                        for (name, o) in [("portable", &out[1]), ("platform", &out[2])] {
                            assert!(o.0 == out[0].0, "{name} luma differs: {what}");
                            assert!(o.1 == out[0].1, "{name} U differs: {what}");
                            assert!(o.2 == out[0].2, "{name} V differs: {what}");
                        }
                        cases += 1;
                    }
                }
            }
        }
        assert_eq!(cases, sizes.len() * extras.len() * levels.len() * 2);
    }
}
