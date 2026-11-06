//! The loop filter (RFC 6386 Section 15).
//!
//! Each function filters `count` segments straddling one edge. `pos` is the
//! first pixel after the edge (q0) of the first segment, `across` is the step
//! across the edge (1 for a vertical edge, the stride for a horizontal one)
//! and `along` the step to the next segment.

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

/// Per-macroblock filter parameters.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct FilterParams {
    /// The filter level (0 = no filtering).
    pub(crate) level: i32,
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
            level,
            interior_limit: interior,
            hev_threshold: hev,
        }
    }
}

/// The simple filter's segment test and filter, used by both filter types.
#[inline(always)]
fn common_adjust(buf: &mut [u8], pos: usize, across: usize, use_outer_taps: bool) -> i32 {
    let p1 = u2s(buf[pos - 2 * across]);
    let p0 = u2s(buf[pos - across]);
    let q0 = u2s(buf[pos]);
    let q1 = u2s(buf[pos + across]);
    let mut a = 3 * (q0 - p0);
    if use_outer_taps {
        a += c8(p1 - q1);
    }
    a = c8(a);
    let f1 = c8(a + 4) >> 3;
    let f2 = c8(a + 3) >> 3;
    buf[pos] = s2u(q0 - f1);
    buf[pos - across] = s2u(p0 + f2);
    f1
}

#[inline(always)]
fn simple_threshold(buf: &[u8], pos: usize, across: usize, limit: i32) -> bool {
    let p1 = buf[pos - 2 * across] as i32;
    let p0 = buf[pos - across] as i32;
    let q0 = buf[pos] as i32;
    let q1 = buf[pos + across] as i32;
    (p0 - q0).abs() * 2 + ((p1 - q1).abs() >> 1) <= limit
}

#[inline(always)]
fn normal_threshold(buf: &[u8], pos: usize, across: usize, edge: i32, interior: i32) -> bool {
    if !simple_threshold(buf, pos, across, edge) {
        return false;
    }
    let p3 = buf[pos - 4 * across] as i32;
    let p2 = buf[pos - 3 * across] as i32;
    let p1 = buf[pos - 2 * across] as i32;
    let p0 = buf[pos - across] as i32;
    let q0 = buf[pos] as i32;
    let q1 = buf[pos + across] as i32;
    let q2 = buf[pos + 2 * across] as i32;
    let q3 = buf[pos + 3 * across] as i32;
    (p3 - p2).abs() <= interior
        && (p2 - p1).abs() <= interior
        && (p1 - p0).abs() <= interior
        && (q3 - q2).abs() <= interior
        && (q2 - q1).abs() <= interior
        && (q1 - q0).abs() <= interior
}

#[inline(always)]
fn high_edge_variance(buf: &[u8], pos: usize, across: usize, threshold: i32) -> bool {
    let p1 = buf[pos - 2 * across] as i32;
    let p0 = buf[pos - across] as i32;
    let q0 = buf[pos] as i32;
    let q1 = buf[pos + across] as i32;
    (p1 - p0).abs() > threshold || (q1 - q0).abs() > threshold
}

/// The simple filter on one edge (luma only). `limit` is the combined edge limit.
pub(crate) fn simple_edge(buf: &mut [u8], pos: usize, across: usize, along: usize, count: usize, limit: i32) {
    for i in 0..count {
        let p = pos + i * along;
        if simple_threshold(buf, p, across, limit) {
            common_adjust(buf, p, across, true);
        }
    }
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
    for i in 0..count {
        let p = pos + i * along;
        if !normal_threshold(buf, p, across, edge_limit, params.interior_limit) {
            continue;
        }
        if high_edge_variance(buf, p, across, params.hev_threshold) {
            common_adjust(buf, p, across, true);
        } else {
            let p2 = u2s(buf[p - 3 * across]);
            let p1 = u2s(buf[p - 2 * across]);
            let p0 = u2s(buf[p - across]);
            let q0 = u2s(buf[p]);
            let q1 = u2s(buf[p + across]);
            let q2 = u2s(buf[p + 2 * across]);
            let w = c8(c8(p1 - q1) + 3 * (q0 - p0));
            let a = c8((27 * w + 63) >> 7);
            buf[p] = s2u(q0 - a);
            buf[p - across] = s2u(p0 + a);
            let a = c8((18 * w + 63) >> 7);
            buf[p + across] = s2u(q1 - a);
            buf[p - 2 * across] = s2u(p1 + a);
            let a = c8((9 * w + 63) >> 7);
            buf[p + 2 * across] = s2u(q2 - a);
            buf[p - 3 * across] = s2u(p2 + a);
        }
    }
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
    for i in 0..count {
        let p = pos + i * along;
        if !normal_threshold(buf, p, across, edge_limit, params.interior_limit) {
            continue;
        }
        let hev = high_edge_variance(buf, p, across, params.hev_threshold);
        let f1 = common_adjust(buf, p, across, hev);
        if !hev {
            let a = (f1 + 1) >> 1;
            let p1 = u2s(buf[p - 2 * across]);
            let q1 = u2s(buf[p + across]);
            buf[p + across] = s2u(q1 - a);
            buf[p - 2 * across] = s2u(p1 + a);
        }
    }
}
