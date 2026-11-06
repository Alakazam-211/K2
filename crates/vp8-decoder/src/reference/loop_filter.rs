//! The loop filter (RFC 6386 Section 15).
//!
//! Each function filters `count` (at most 16) segments straddling one edge.
//! `pos` is the first pixel after the edge (q0) of the first segment,
//! `across` is the step across the edge (1 for a vertical edge, the stride
//! for a horizontal one) and `along` the step to the next segment.
//!
//! The eight pixels of every segment (p3 to q3) are first gathered into
//! small fixed arrays, one row per tap, so the filters run on plain arrays
//! without bounds checks; the changed pixels are then written back.

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
type Segments = [[u8; 16]; 8];

fn load(buf: &[u8], pos: usize, across: usize, along: usize, count: usize) -> Segments {
    let mut seg = [[0u8; 16]; 8];
    if across == 1 {
        // A vertical edge: each segment is 8 contiguous pixels of one row.
        for i in 0..count {
            let p = pos + i * along;
            let s: &[u8; 8] = buf[p - 4..p + 4].try_into().expect("8 pixels");
            for (row, &v) in seg.iter_mut().zip(s.iter()) {
                row[i] = v;
            }
        }
    } else {
        // A horizontal edge: each tap is a run of `count` pixels of one row.
        for (k, row) in seg.iter_mut().enumerate() {
            let p = pos + k * across - 4 * across;
            row[..count].copy_from_slice(&buf[p..p + count]);
        }
    }
    seg
}

/// Writes back taps `first..last` (the ones a filter may change).
// The segment index walks the transposed array, so a range loop is clearest.
#[allow(clippy::too_many_arguments, clippy::needless_range_loop)]
fn store(
    buf: &mut [u8],
    pos: usize,
    across: usize,
    along: usize,
    count: usize,
    seg: &Segments,
    first: usize,
    last: usize,
) {
    if across == 1 {
        for i in 0..count {
            let p = pos + i * along;
            let d = &mut buf[p - 4 + first..p - 4 + last];
            for (k, px) in (first..last).zip(d.iter_mut()) {
                *px = seg[k][i];
            }
        }
    } else {
        for (k, row) in seg.iter().enumerate().take(last).skip(first) {
            let p = pos + k * across - 4 * across;
            buf[p..p + count].copy_from_slice(&row[..count]);
        }
    }
}

#[inline(always)]
fn c8(v: i32) -> i32 {
    v.clamp(-128, 127)
}

#[inline(always)]
fn u2s(v: u8) -> i32 {
    v as i32 - 128
}

#[inline(always)]
fn s2u(v: i32) -> u8 {
    (c8(v) + 128) as u8
}

// The filters below work on all 16 lanes at once, without branches, so
// the compiler can vectorise them. Lanes past `count` hold zeros and are
// never stored.

#[inline(always)]
fn abs_diff(a: u8, b: u8) -> i32 {
    (a as i32 - b as i32).abs()
}

#[inline(always)]
fn simple_mask(seg: &Segments, i: usize, limit: i32) -> bool {
    abs_diff(seg[3][i], seg[4][i]) * 2 + (abs_diff(seg[2][i], seg[5][i]) >> 1) <= limit
}

#[inline(always)]
fn normal_mask(seg: &Segments, i: usize, edge: i32, interior: i32) -> bool {
    simple_mask(seg, i, edge)
        & (abs_diff(seg[0][i], seg[1][i]) <= interior)
        & (abs_diff(seg[1][i], seg[2][i]) <= interior)
        & (abs_diff(seg[2][i], seg[3][i]) <= interior)
        & (abs_diff(seg[7][i], seg[6][i]) <= interior)
        & (abs_diff(seg[6][i], seg[5][i]) <= interior)
        & (abs_diff(seg[5][i], seg[4][i]) <= interior)
}

#[inline(always)]
fn hev(seg: &Segments, i: usize, threshold: i32) -> bool {
    (abs_diff(seg[2][i], seg[3][i]) > threshold) | (abs_diff(seg[5][i], seg[4][i]) > threshold)
}

#[inline(always)]
fn pick(cond: bool, a: u8, b: u8) -> u8 {
    if cond {
        a
    } else {
        b
    }
}

/// The simple filter on one edge (luma only). `limit` is the combined edge limit.
pub fn simple_edge(
    buf: &mut [u8],
    pos: usize,
    across: usize,
    along: usize,
    count: usize,
    limit: i32,
) {
    let mut seg = load(buf, pos, across, along, count);
    let old = seg;
    for i in 0..16 {
        let mask = simple_mask(&old, i, limit);
        let (p1, p0, q0, q1) = (
            u2s(old[2][i]),
            u2s(old[3][i]),
            u2s(old[4][i]),
            u2s(old[5][i]),
        );
        let a = c8(c8(p1 - q1) + 3 * (q0 - p0));
        let f1 = c8(a + 4) >> 3;
        let f2 = c8(a + 3) >> 3;
        seg[4][i] = pick(mask, s2u(q0 - f1), old[4][i]);
        seg[3][i] = pick(mask, s2u(p0 + f2), old[3][i]);
    }
    store(buf, pos, across, along, count, &seg, 3, 5);
}

/// The normal filter across a macroblock edge.
pub fn mb_edge(
    buf: &mut [u8],
    pos: usize,
    across: usize,
    along: usize,
    count: usize,
    edge_limit: i32,
    params: &FilterParams,
) {
    let mut seg = load(buf, pos, across, along, count);
    let old = seg;
    for i in 0..16 {
        let mask = normal_mask(&old, i, edge_limit, params.interior_limit);
        let high = hev(&old, i, params.hev_threshold);
        let (p2, p1, p0) = (u2s(old[1][i]), u2s(old[2][i]), u2s(old[3][i]));
        let (q0, q1, q2) = (u2s(old[4][i]), u2s(old[5][i]), u2s(old[6][i]));
        let w = c8(c8(p1 - q1) + 3 * (q0 - p0));
        // High edge variance: only p0 and q0 move.
        let f1 = c8(w + 4) >> 3;
        let f2 = c8(w + 3) >> 3;
        // Otherwise three pixels on each side, by 27, 18 and 9 / 128 of w.
        let a27 = c8((27 * w + 63) >> 7);
        let a18 = c8((18 * w + 63) >> 7);
        let a9 = c8((9 * w + 63) >> 7);
        let only = mask & high;
        let wide = mask & !high;
        seg[4][i] = pick(only, s2u(q0 - f1), pick(wide, s2u(q0 - a27), old[4][i]));
        seg[3][i] = pick(only, s2u(p0 + f2), pick(wide, s2u(p0 + a27), old[3][i]));
        seg[5][i] = pick(wide, s2u(q1 - a18), old[5][i]);
        seg[2][i] = pick(wide, s2u(p1 + a18), old[2][i]);
        seg[6][i] = pick(wide, s2u(q2 - a9), old[6][i]);
        seg[1][i] = pick(wide, s2u(p2 + a9), old[1][i]);
    }
    store(buf, pos, across, along, count, &seg, 1, 7);
}

/// The normal filter across an edge between subblocks.
pub fn subblock_edge(
    buf: &mut [u8],
    pos: usize,
    across: usize,
    along: usize,
    count: usize,
    edge_limit: i32,
    params: &FilterParams,
) {
    let mut seg = load(buf, pos, across, along, count);
    let old = seg;
    for i in 0..16 {
        let mask = normal_mask(&old, i, edge_limit, params.interior_limit);
        let high = hev(&old, i, params.hev_threshold);
        let (p1, p0, q0, q1) = (
            u2s(old[2][i]),
            u2s(old[3][i]),
            u2s(old[4][i]),
            u2s(old[5][i]),
        );
        // The outer taps count only at high edge variance.
        let outer = if high { c8(p1 - q1) } else { 0 };
        let a = c8(outer + 3 * (q0 - p0));
        let f1 = c8(a + 4) >> 3;
        let f2 = c8(a + 3) >> 3;
        let a2 = (f1 + 1) >> 1;
        let soft = mask & !high;
        seg[4][i] = pick(mask, s2u(q0 - f1), old[4][i]);
        seg[3][i] = pick(mask, s2u(p0 + f2), old[3][i]);
        seg[5][i] = pick(soft, s2u(q1 - a2), old[5][i]);
        seg[2][i] = pick(soft, s2u(p1 + a2), old[2][i]);
    }
    store(buf, pos, across, along, count, &seg, 2, 6);
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
    let p = FilterParams::new(level, sharpness, keyframe);
    let mb_limit = (level + 2) * 2 + p.interior_limit;
    let b_limit = level * 2 + p.interior_limit;
    if simple {
        let s = planes.y_stride;
        let pos = row * 16 * s + col * 16;
        let buf = planes.y;
        if col > 0 {
            simple_edge(buf, pos, 1, s, 16, mb_limit);
        }
        if inner {
            for k in [4, 8, 12] {
                simple_edge(buf, pos + k, 1, s, 16, b_limit);
            }
        }
        if row > 0 {
            simple_edge(buf, pos, s, 1, 16, mb_limit);
        }
        if inner {
            for k in [4, 8, 12] {
                simple_edge(buf, pos + k * s, s, 1, 16, b_limit);
            }
        }
        return;
    }
    filter_plane_mb(
        planes.y,
        planes.y_stride,
        16,
        row,
        col,
        inner,
        mb_limit,
        b_limit,
        &p,
    );
    filter_plane_mb(
        planes.u,
        planes.uv_stride,
        8,
        row,
        col,
        inner,
        mb_limit,
        b_limit,
        &p,
    );
    filter_plane_mb(
        planes.v,
        planes.uv_stride,
        8,
        row,
        col,
        inner,
        mb_limit,
        b_limit,
        &p,
    );
}

#[allow(clippy::too_many_arguments)]
fn filter_plane_mb(
    buf: &mut [u8],
    s: usize,
    n: usize,
    row: usize,
    col: usize,
    inner: bool,
    mb_limit: i32,
    b_limit: i32,
    p: &FilterParams,
) {
    let pos = row * n * s + col * n;
    if col > 0 {
        mb_edge(buf, pos, 1, s, n, mb_limit, p);
    }
    if inner {
        for k in (4..n).step_by(4) {
            subblock_edge(buf, pos + k, 1, s, n, b_limit, p);
        }
    }
    if row > 0 {
        mb_edge(buf, pos, s, 1, n, mb_limit, p);
    }
    if inner {
        for k in (4..n).step_by(4) {
            subblock_edge(buf, pos + k * s, s, 1, n, b_limit, p);
        }
    }
}
