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
pub(crate) struct FilterParams {
    pub(crate) interior_limit: i32,
    pub(crate) hev_threshold: i32,
}

impl FilterParams {
    pub(crate) fn new(level: i32, sharpness: u8, keyframe: bool) -> Self {
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
#[allow(clippy::too_many_arguments)]
fn store(buf: &mut [u8], pos: usize, across: usize, along: usize, count: usize, seg: &Segments, first: usize, last: usize) {
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

/// Reads one segment's eight pixels as unsigned values.
#[inline(always)]
fn taps(seg: &Segments, i: usize) -> [i32; 8] {
    std::array::from_fn(|k| seg[k][i] as i32)
}

#[inline(always)]
fn simple_ok(t: &[i32; 8], limit: i32) -> bool {
    (t[3] - t[4]).abs() * 2 + ((t[2] - t[5]).abs() >> 1) <= limit
}

#[inline(always)]
fn normal_ok(t: &[i32; 8], edge: i32, interior: i32) -> bool {
    simple_ok(t, edge)
        && (t[0] - t[1]).abs() <= interior
        && (t[1] - t[2]).abs() <= interior
        && (t[2] - t[3]).abs() <= interior
        && (t[7] - t[6]).abs() <= interior
        && (t[6] - t[5]).abs() <= interior
        && (t[5] - t[4]).abs() <= interior
}

#[inline(always)]
fn hev(t: &[i32; 8], threshold: i32) -> bool {
    (t[2] - t[3]).abs() > threshold || (t[5] - t[4]).abs() > threshold
}

/// The common adjustment of p0 and q0 (used by every filter). Returns the
/// q0 step `f1`.
#[inline(always)]
fn common_adjust(seg: &mut Segments, i: usize, use_outer_taps: bool) -> i32 {
    let p1 = u2s(seg[2][i]);
    let p0 = u2s(seg[3][i]);
    let q0 = u2s(seg[4][i]);
    let q1 = u2s(seg[5][i]);
    let mut a = 3 * (q0 - p0);
    if use_outer_taps {
        a += c8(p1 - q1);
    }
    a = c8(a);
    let f1 = c8(a + 4) >> 3;
    let f2 = c8(a + 3) >> 3;
    seg[4][i] = s2u(q0 - f1);
    seg[3][i] = s2u(p0 + f2);
    f1
}

/// The simple filter on one edge (luma only). `limit` is the combined edge limit.
pub(crate) fn simple_edge(buf: &mut [u8], pos: usize, across: usize, along: usize, count: usize, limit: i32) {
    let mut seg = load(buf, pos, across, along, count);
    for i in 0..count {
        if simple_ok(&taps(&seg, i), limit) {
            common_adjust(&mut seg, i, true);
        }
    }
    store(buf, pos, across, along, count, &seg, 3, 5);
}

/// The normal filter across a macroblock edge.
pub(crate) fn mb_edge(
    buf: &mut [u8],
    pos: usize,
    across: usize,
    along: usize,
    count: usize,
    edge_limit: i32,
    params: &FilterParams,
) {
    let mut seg = load(buf, pos, across, along, count);
    for i in 0..count {
        let t = taps(&seg, i);
        if !normal_ok(&t, edge_limit, params.interior_limit) {
            continue;
        }
        if hev(&t, params.hev_threshold) {
            common_adjust(&mut seg, i, true);
        } else {
            let [_, p2, p1, p0, q0, q1, q2, _] = t.map(|v| v - 128);
            let w = c8(c8(p1 - q1) + 3 * (q0 - p0));
            let a = c8((27 * w + 63) >> 7);
            seg[4][i] = s2u(q0 - a);
            seg[3][i] = s2u(p0 + a);
            let a = c8((18 * w + 63) >> 7);
            seg[5][i] = s2u(q1 - a);
            seg[2][i] = s2u(p1 + a);
            let a = c8((9 * w + 63) >> 7);
            seg[6][i] = s2u(q2 - a);
            seg[1][i] = s2u(p2 + a);
        }
    }
    store(buf, pos, across, along, count, &seg, 1, 7);
}

/// The normal filter across an edge between subblocks.
pub(crate) fn subblock_edge(
    buf: &mut [u8],
    pos: usize,
    across: usize,
    along: usize,
    count: usize,
    edge_limit: i32,
    params: &FilterParams,
) {
    let mut seg = load(buf, pos, across, along, count);
    for i in 0..count {
        let t = taps(&seg, i);
        if !normal_ok(&t, edge_limit, params.interior_limit) {
            continue;
        }
        let high = hev(&t, params.hev_threshold);
        let f1 = common_adjust(&mut seg, i, high);
        if !high {
            let a = (f1 + 1) >> 1;
            let p1 = u2s(seg[2][i]);
            let q1 = u2s(seg[5][i]);
            seg[5][i] = s2u(q1 - a);
            seg[2][i] = s2u(p1 + a);
        }
    }
    store(buf, pos, across, along, count, &seg, 2, 6);
}
