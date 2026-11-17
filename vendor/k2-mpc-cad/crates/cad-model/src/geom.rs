//! Boxes of 2D geometry, format-neutral: lines and points, circles and arcs (in their own
//! frame), bulged polyline segments and full ellipses. Exact where the extent has a closed form;
//! a reader marks the rest approximate.

use std::f64::consts::{FRAC_PI_2, TAU};

use crate::mutant;

/// A box being grown point by point.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Acc {
    b: Option<[f64; 4]>,
}

impl Acc {
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes in point `(x, y)`; a non-finite one is ignored.
    pub fn add(&mut self, x: f64, y: f64) {
        if !x.is_finite() || !y.is_finite() {
            return;
        }
        self.b = Some(match self.b {
            None => [x, y, x, y],
            Some([x0, y0, x1, y1]) => [x0.min(x), y0.min(y), x1.max(x), y1.max(y)],
        });
    }

    /// Takes in box `b`.
    pub fn add_box(&mut self, b: [f64; 4]) {
        self.add(b[0], b[1]);
        self.add(b[2], b[3]);
    }

    /// The box, when a point was taken in.
    pub fn get(&self) -> Option<[f64; 4]> {
        self.b.map(|b| b.map(|v| if v == 0.0 { 0.0 } else { v }))
    }
}

/// Whether extrusion `n` is up (0, 0, 1), within rounding: the entity's frame is the plan's.
pub fn is_up(n: [f64; 3]) -> bool {
    n[0].abs() < 1e-12 && n[1].abs() < 1e-12 && (n[2] - 1.0).abs() < 1e-12
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn unit(a: [f64; 3]) -> Option<[f64; 3]> {
    let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
    (l > 0.0 && l.is_finite()).then(|| [a[0] / l, a[1] / l, a[2] / l])
}

/// The world directions of the x and y axes of the frame whose extrusion is `n` (the arbitrary
/// axis rule): `None` for a zero or non-finite `n`.
pub fn frame(n: [f64; 3]) -> Option<([f64; 3], [f64; 3], [f64; 3])> {
    let n = if mutant::active("OcsIgnored") {
        [0.0, 0.0, 1.0]
    } else {
        unit(n)?
    };
    let limit = 1.0 / 64.0;
    let ax = if n[0].abs() < limit && n[1].abs() < limit {
        cross([0.0, 1.0, 0.0], n)
    } else {
        cross([0.0, 0.0, 1.0], n)
    };
    let ax = unit(ax)?;
    let ay = unit(cross(n, ax))?;
    Some((ax, ay, n))
}

/// Point `p` of the frame whose extrusion is `n`, in world coordinates.
pub fn to_world(p: [f64; 3], n: [f64; 3]) -> Option<[f64; 3]> {
    let (ax, ay, az) = frame(n)?;
    Some([
        p[0] * ax[0] + p[1] * ay[0] + p[2] * az[0],
        p[0] * ax[1] + p[1] * ay[1] + p[2] * az[1],
        p[0] * ax[2] + p[1] * ay[2] + p[2] * az[2],
    ])
}

/// The exact box, in world x and y, of the circle with centre `c` and radius `r` in the frame
/// whose extrusion is `n`.
pub fn circle(c: [f64; 3], r: f64, n: [f64; 3]) -> Option<[f64; 4]> {
    let (ax, ay, _) = frame(n)?;
    let w = to_world(c, n)?;
    let ex = r.abs() * (ax[0] * ax[0] + ay[0] * ay[0]).sqrt();
    let ey = r.abs() * (ax[1] * ax[1] + ay[1] * ay[1]).sqrt();
    let mut a = Acc::new();
    a.add(w[0] - ex, w[1] - ey);
    a.add(w[0] + ex, w[1] + ey);
    a.get()
}

/// Takes in the arc of centre `c` and radius `r` from angle `start` (radians), turning
/// counter-clockwise by `sweep` (0 to 2π): its ends and every axis crossing on the way.
pub fn arc_sweep(acc: &mut Acc, c: [f64; 2], r: f64, start: f64, sweep: f64) {
    let sweep = sweep.clamp(0.0, TAU);
    acc.add(c[0] + r * start.cos(), c[1] + r * start.sin());
    let end = start + sweep;
    acc.add(c[0] + r * end.cos(), c[1] + r * end.sin());
    // the axis directions: the first multiple of π/2 at or after the start
    let mut k = (start / FRAC_PI_2).ceil();
    loop {
        let a = k * FRAC_PI_2;
        if a > end + 1e-15 {
            break;
        }
        let q = k.rem_euclid(4.0) as i64;
        let (dx, dy) = match q {
            0 => (1.0, 0.0),
            1 => (0.0, 1.0),
            2 => (-1.0, 0.0),
            _ => (0.0, -1.0),
        };
        acc.add(c[0] + r * dx, c[1] + r * dy);
        k += 1.0;
    }
}

/// The box, in the arc's own frame, of the arc of centre `c` and radius `r` from `a0` to `a1`
/// degrees, counter-clockwise.
pub fn arc_deg(c: [f64; 2], r: f64, a0: f64, a1: f64) -> Option<[f64; 4]> {
    let (s, e) = (a0.to_radians(), a1.to_radians());
    let mut sweep = (e - s).rem_euclid(TAU);
    if sweep == 0.0 && (a1 - a0).abs() >= 360.0 {
        sweep = TAU;
    }
    let mut acc = Acc::new();
    arc_sweep(&mut acc, c, r, s.rem_euclid(TAU), sweep);
    acc.get()
}

/// Takes in the polyline segment from `p0` to `p1` with bulge `b` (the tangent of a quarter of
/// its included angle; positive: counter-clockwise).
pub fn bulge_seg(acc: &mut Acc, p0: [f64; 2], p1: [f64; 2], b: f64) {
    acc.add(p0[0], p0[1]);
    acc.add(p1[0], p1[1]);
    if b == 0.0 || !b.is_finite() || mutant::active("BulgeIgnored") {
        return;
    }
    let (vx, vy) = (p1[0] - p0[0], p1[1] - p0[1]);
    let l = (vx * vx + vy * vy).sqrt();
    if l == 0.0 {
        return;
    }
    let theta = 4.0 * b.atan();
    // the centre is off the chord's middle, along its left normal
    let d = (l / 2.0) * (1.0 - b * b) / (2.0 * b);
    let (mx, my) = ((p0[0] + p1[0]) / 2.0, (p0[1] + p1[1]) / 2.0);
    let (nx, ny) = (-vy / l, vx / l);
    let c = [mx + nx * d, my + ny * d];
    let r = ((p0[0] - c[0]).powi(2) + (p0[1] - c[1]).powi(2)).sqrt();
    let a0 = (p0[1] - c[1]).atan2(p0[0] - c[0]);
    let a1 = (p1[1] - c[1]).atan2(p1[0] - c[0]);
    if theta > 0.0 {
        arc_sweep(acc, c, r, a0.rem_euclid(TAU), theta.min(TAU));
    } else {
        arc_sweep(acc, c, r, a1.rem_euclid(TAU), (-theta).min(TAU));
    }
}

/// The box of the whole ellipse of centre `c`, major axis vector `major` and minor-to-major
/// ratio `ratio`, in its plane.
pub fn ellipse(c: [f64; 2], major: [f64; 2], ratio: f64) -> Option<[f64; 4]> {
    let b = [-major[1] * ratio, major[0] * ratio];
    let ex = (major[0] * major[0] + b[0] * b[0]).sqrt();
    let ey = (major[1] * major[1] + b[1] * b[1]).sqrt();
    let mut a = Acc::new();
    a.add(c[0] - ex, c[1] - ey);
    a.add(c[0] + ex, c[1] + ey);
    a.get()
}

/// A number as text: the shortest form that reads back the same, `0` for minus zero.
pub fn num(x: f64) -> String {
    if x == 0.0 {
        "0".into()
    } else {
        format!("{x}")
    }
}

/// `x0,y0,x1,y1`.
pub fn box_text(b: [f64; 4]) -> String {
    format!("{},{},{},{}", num(b[0]), num(b[1]), num(b[2]), num(b[3]))
}

/// Whether `inner` lies wholly inside `outer`.
pub fn inside(inner: [f64; 4], outer: [f64; 4]) -> bool {
    inner[0] >= outer[0] && inner[1] >= outer[1] && inner[2] <= outer[2] && inner[3] <= outer[3]
}

/// Whether `a` and `b` touch or overlap.
pub fn touches(a: [f64; 4], b: [f64; 4]) -> bool {
    a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3]
}

fn orient(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

fn on_segment(a: [f64; 2], b: [f64; 2], p: [f64; 2]) -> bool {
    orient(a, b, p).abs() <= 1e-9 * (1.0 + (b[0] - a[0]).abs() + (b[1] - a[1]).abs())
        && p[0] >= a[0].min(b[0]) - 1e-12
        && p[0] <= a[0].max(b[0]) + 1e-12
        && p[1] >= a[1].min(b[1]) - 1e-12
        && p[1] <= a[1].max(b[1]) + 1e-12
}

/// Whether segments `a-b` and `c-d` meet (touching counts).
pub fn segments_meet(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> bool {
    let (d1, d2) = (orient(c, d, a), orient(c, d, b));
    let (d3, d4) = (orient(a, b, c), orient(a, b, d));
    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
    {
        return true;
    }
    on_segment(c, d, a) || on_segment(c, d, b) || on_segment(a, b, c) || on_segment(a, b, d)
}

/// The edges of a polygon, closing it.
pub fn edges(poly: &[[f64; 2]]) -> impl Iterator<Item = ([f64; 2], [f64; 2])> + '_ {
    (0..poly.len()).map(move |i| (poly[i], poly[(i + 1) % poly.len()]))
}

/// Whether `p` is inside polygon `poly` or on its boundary.
pub fn in_polygon(p: [f64; 2], poly: &[[f64; 2]]) -> bool {
    if poly.len() < 3 {
        return false;
    }
    if edges(poly).any(|(a, b)| on_segment(a, b, p)) {
        return true;
    }
    let mut inside = false;
    for (a, b) in edges(poly) {
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let x = a[0] + (p[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1]);
            if p[0] < x {
                inside = !inside;
            }
        }
    }
    inside
}

/// The distance from `p` to segment `a-b`.
pub fn to_segment(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 {
        0.0
    } else {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0)
    };
    let (x, y) = (a[0] + t * dx, a[1] + t * dy);
    ((p[0] - x).powi(2) + (p[1] - y).powi(2)).sqrt()
}

/// The segments of an outline (closed: the last vertex back to the first).
pub fn segments(pts: &[[f64; 2]], closed: bool) -> Vec<([f64; 2], [f64; 2])> {
    let mut v: Vec<_> = pts.windows(2).map(|w| (w[0], w[1])).collect();
    if closed && pts.len() > 2 {
        v.push((pts[pts.len() - 1], pts[0]));
    }
    v
}
