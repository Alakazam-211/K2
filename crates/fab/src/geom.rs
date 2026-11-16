//! Point bounds in a file's own integer units, exact (integer arithmetic only, so every
//! platform gets the same box). A box holds the points a file states for its objects (draw,
//! arc and flash points, region and rout vertices) and the extreme points of its arcs; it is not
//! the ink, which depends on the aperture's shape and is a renderer's to work out.

/// A point in a file's integer units.
pub type Pt = (i64, i64);

/// An axis-aligned box, or nothing yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Bounds {
    /// `(min_x, min_y, max_x, max_y)`, or `None` before the first point.
    pub b: Option<(i64, i64, i64, i64)>,
}

impl Bounds {
    /// Adds a point.
    pub fn add(&mut self, (x, y): Pt) {
        self.b = Some(match self.b {
            None => (x, y, x, y),
            Some((a, b, c, d)) => (a.min(x), b.min(y), c.max(x), d.max(y)),
        });
    }

    /// Adds another box.
    pub fn union(&mut self, o: &Bounds) {
        if let Some((a, b, c, d)) = o.b {
            self.add((a, b));
            self.add((c, d));
        }
    }

    /// Adds a circular arc from `s` to `e` about `c`, counterclockwise when `ccw`: its end points
    /// and the points where it crosses the axes through `c`, at the larger of its two radii
    /// rounded up. `full`: the arc is a whole circle.
    pub fn add_arc(&mut self, s: Pt, e: Pt, c: Pt, ccw: bool, full: bool) {
        self.add(s);
        self.add(e);
        let sv = (
            i128::from(s.0) - i128::from(c.0),
            i128::from(s.1) - i128::from(c.1),
        );
        let ev = (
            i128::from(e.0) - i128::from(c.0),
            i128::from(e.1) - i128::from(c.1),
        );
        let r2 = norm2(sv).max(norm2(ev));
        if r2 == 0 {
            return;
        }
        let r = isqrt_ceil(r2.unsigned_abs());
        // a counterclockwise sweep from `from` to `to`
        let (from, to) = if ccw { (sv, ev) } else { (ev, sv) };
        for (d, p) in [
            ((1, 0), (sat(i128::from(c.0) + r), c.1)),
            ((0, 1), (c.0, sat(i128::from(c.1) + r))),
            ((-1, 0), (sat(i128::from(c.0) - r), c.1)),
            ((0, -1), (c.0, sat(i128::from(c.1) - r))),
        ] {
            if full || ccw_contains(from, to, d) {
                self.add(p);
            }
        }
    }

    /// Adds the box that holds any arc of at most half a turn from `s` to `e`: the circle that
    /// has the chord as its diameter (every point of such an arc sees the chord at a right angle
    /// or more, so lies in it), its radius rounded up.
    pub fn add_minor_arc_hull(&mut self, s: Pt, e: Pt) {
        self.add(s);
        self.add(e);
        let (dx, dy) = (
            i128::from(e.0) - i128::from(s.0),
            i128::from(e.1) - i128::from(s.1),
        );
        // the radius of the circle on the chord, from twice the midpoint to stay in integers
        let half = (isqrt_ceil(norm2((dx, dy)).unsigned_abs()) + 1) / 2;
        let (mx2, my2) = (
            i128::from(s.0) + i128::from(e.0),
            i128::from(s.1) + i128::from(e.1),
        );
        let lo = |m2: i128| sat(m2.div_euclid(2) - half);
        let hi = |m2: i128| sat(m2.div_euclid(2) + m2.rem_euclid(2) + half);
        self.add((lo(mx2), lo(my2)));
        self.add((hi(mx2), hi(my2)));
    }
}

/// The squared length, saturating (coordinates far outside any file's range only grow a box).
fn norm2(v: (i128, i128)) -> i128 {
    v.0.saturating_mul(v.0)
        .saturating_add(v.1.saturating_mul(v.1))
}

fn sat(v: i128) -> i64 {
    v.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}

/// The square root of `v`, rounded up.
pub fn isqrt_ceil(v: u128) -> i128 {
    if v == 0 {
        return 0;
    }
    // Newton's method from above, on integers
    let mut x = 1u128 << ((128 - v.leading_zeros()).div_ceil(2));
    loop {
        let y = (x + v / x) / 2;
        if y >= x {
            break;
        }
        x = y;
    }
    // x is now floor(sqrt(v)) or close to it: settle exactly
    while x.checked_mul(x).is_none_or(|q| q > v) {
        x -= 1;
    }
    while (x + 1).checked_mul(x + 1).is_some_and(|q| q <= v) {
        x += 1;
    }
    let r = if x * x == v { x } else { x + 1 };
    r as i128
}

/// Whether direction `d` lies on the counterclockwise sweep from `from` to `to` (both nonzero),
/// ends included; `from == to` in direction is a zero sweep.
fn ccw_contains(from: (i128, i128), to: (i128, i128), d: (i128, i128)) -> bool {
    // the half-turn a vector is in, counted counterclockwise from `from`: 0 for the first half
    // (including `from`'s own direction), 1 for the second
    let cross = |a: (i128, i128), b: (i128, i128)| a.0 * b.1 - a.1 * b.0;
    let dot = |a: (i128, i128), b: (i128, i128)| a.0 * b.0 + a.1 * b.1;
    let half = |v: (i128, i128)| {
        let c = cross(from, v);
        if c > 0 || (c == 0 && dot(from, v) > 0) {
            0
        } else {
            1
        }
    };
    let (hd, ht) = (half(d), half(to));
    if hd != ht {
        return hd < ht;
    }
    // same half: d comes no later than `to`
    cross(d, to) >= 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn square_roots_round_up() {
        assert_eq!(isqrt_ceil(0), 0);
        assert_eq!(isqrt_ceil(1), 1);
        assert_eq!(isqrt_ceil(2), 2);
        assert_eq!(isqrt_ceil(16), 4);
        assert_eq!(isqrt_ceil(17), 5);
        assert_eq!(isqrt_ceil(u128::from(u64::MAX)), 1 << 32);
    }

    #[test]
    fn arcs_add_the_axis_points_they_pass() {
        // a quarter circle counterclockwise from +x to +y about the origin: no other axis point
        let mut b = Bounds::default();
        b.add_arc((10, 0), (0, 10), (0, 0), true, false);
        assert_eq!(b.b, Some((0, 0, 10, 10)));
        // the same quarter clockwise: three quarters of the circle, through -x and -y
        let mut b = Bounds::default();
        b.add_arc((10, 0), (0, 10), (0, 0), false, false);
        assert_eq!(b.b, Some((-10, -10, 10, 10)));
        // a half circle counterclockwise from +y to -y: through -x
        let mut b = Bounds::default();
        b.add_arc((0, 10), (0, -10), (0, 0), true, false);
        assert_eq!(b.b, Some((-10, -10, 0, 10)));
        // a whole circle
        let mut b = Bounds::default();
        b.add_arc((5, 0), (5, 0), (0, 0), true, true);
        assert_eq!(b.b, Some((-5, -5, 5, 5)));
    }

    #[test]
    fn a_minor_arc_hull_holds_the_half_circle() {
        let mut b = Bounds::default();
        b.add_minor_arc_hull((0, 0), (10, 0));
        assert_eq!(b.b, Some((0, -5, 10, 5)));
    }
}
