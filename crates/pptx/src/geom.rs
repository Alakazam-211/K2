//! A shape's transform box on the page (the rendering-data contract r1.4, Item 1).
//!
//! **The rule:** a group child's page outline is the true affine image of its child-space
//! outline under every enclosing group's transform. Per enclosing group, from the innermost
//! out ([`compose`]):
//!
//! 1. map from the group's child space (`chOff`, `chExt`) into its box: `sx = ext.cx /
//!    chExt.cx`, `sy = ext.cy / chExt.cy`. The centre maps exactly. The extent depends on the
//!    child's rotation in that space: a multiple of 180° gives `w × sx, h × sy`; 90° or 270°
//!    off that gives `w × sy, h × sx`; any other angle with `sx ≠ sy` is step 6;
//! 2. a flipped group mirrors the centre within its box;
//! 3. the centre turns about the group's centre by the group's `rot` (clockwise, y down);
//! 4. `flip.h = group.flip.h XOR child.flip.h`, and the same for `v`;
//! 5. `rot = group.rot + child.rot`, but **`group.rot − child.rot` under exactly one group
//!    flip**, modulo 360°;
//! 6. a child rotated by other than a multiple of 90° under `sx ≠ sy` is a parallelogram: its
//!    box keeps the exact centre, the extent `w × sx, h × sy` and its `rot`, and is flagged
//!    `approx`, never given as a wrong exact box;
//! 7. nesting repeats 1 to 6 per level; **the centre is always the true centre**, carried
//!    through every level's exact map (never worked from a parent's approximate box);
//! 8. every descendant of an `approx` group is `approx` too (approx inherited).
//!
//! The box is the transform box: offset and extent, never the ink.

use crate::mutant;

/// A full turn, in 60,000ths of a degree.
pub const TURN: i64 = 21_600_000;
const QUARTER: i64 = 5_400_000;
const HALF: i64 = 10_800_000;

/// A shape's own transform (`a:xfrm`), in its parent's coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Xf {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
    pub rot: i64,
    pub fh: bool,
    pub fv: bool,
}

/// A group's transform: its own, and its child space (`chOff`, `chExt`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GroupXf {
    pub own: Xf,
    pub ch_x: i64,
    pub ch_y: i64,
    pub ch_w: i64,
    pub ch_h: i64,
}

/// A box on the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placed {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
    /// 60,000ths of a degree, clockwise, about the box centre, in `0..TURN`.
    pub rot: i64,
    pub fh: bool,
    pub fv: bool,
    pub approx: bool,
}

/// `(cos, sin)` of `rot`, exact at the quarter turns.
fn cos_sin(rot: i64) -> (f64, f64) {
    match rot.rem_euclid(TURN) {
        0 => (1.0, 0.0),
        QUARTER => (0.0, 1.0),
        HALF => (-1.0, 0.0),
        r if r == 3 * QUARTER => (0.0, -1.0),
        r => {
            let t = (r as f64 / 60_000.0).to_radians();
            (t.cos(), t.sin())
        }
    }
}

/// Place a shape with its own transform `own` inside `groups` (outermost first), whose
/// innermost's box is `parent_approx` already ([`compose`] of that group). Top-level: no
/// groups.
pub fn compose(own: Xf, groups: &[GroupXf], parent_approx: bool) -> Placed {
    let mut cx = own.x as f64 + own.w as f64 / 2.0;
    let mut cy = own.y as f64 + own.h as f64 / 2.0;
    let (mut w, mut h) = (own.w as f64, own.h as f64);
    let mut rot = own.rot.rem_euclid(TURN);
    let (mut fh, mut fv) = (own.fh, own.fv);
    let mut approx = false;
    for g in groups.iter().rev() {
        let o = g.own;
        let sx = if g.ch_w == 0 {
            1.0
        } else {
            o.w as f64 / g.ch_w as f64
        };
        let sy = if g.ch_h == 0 {
            1.0
        } else {
            o.h as f64 / g.ch_h as f64
        };
        // 1: the centre exactly; the extent by the child's angle in this space
        cx = o.x as f64 + (cx - g.ch_x as f64) * sx;
        cy = o.y as f64 + (cy - g.ch_y as f64) * sy;
        let r = rot.rem_euclid(HALF);
        if r == 0 {
            w *= sx;
            h *= sy;
        } else if r == QUARTER {
            (w, h) = (w * sy, h * sx);
        } else {
            // 6: a parallelogram under uneven scaling
            if (sx - sy).abs() > 1e-12 * sx.abs().max(sy.abs()).max(1.0) {
                approx = true;
            }
            w *= sx;
            h *= sy;
        }
        // 2: the group's flip mirrors the centre within its box
        let gcx = o.x as f64 + o.w as f64 / 2.0;
        let gcy = o.y as f64 + o.h as f64 / 2.0;
        if o.fh {
            cx = 2.0 * gcx - cx;
        }
        if o.fv {
            cy = 2.0 * gcy - cy;
        }
        // 3: turn about the group's centre
        if !mutant::active("GroupRotNotComposed") {
            let (c, s) = cos_sin(o.rot);
            let (dx, dy) = (cx - gcx, cy - gcy);
            cx = gcx + dx * c - dy * s;
            cy = gcy + dx * s + dy * c;
        }
        // 4 and 5
        fh ^= o.fh;
        fv ^= o.fv;
        rot = if o.fh != o.fv && !mutant::active("FlipRotAdded") {
            o.rot - rot
        } else if mutant::active("GroupRotNotComposed") {
            rot
        } else {
            o.rot + rot
        }
        .rem_euclid(TURN);
    }
    // 8: inherited
    if parent_approx && !mutant::active("ApproxNotInherited") {
        approx = true;
    }
    Placed {
        x: (cx - w / 2.0).round() as i64,
        y: (cy - h / 2.0).round() as i64,
        w: w.round() as i64,
        h: h.round() as i64,
        rot,
        fh,
        fv,
        approx,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xf(x: i64, y: i64, w: i64, h: i64, rot: i64) -> Xf {
        Xf {
            x,
            y,
            w,
            h,
            rot,
            ..Xf::default()
        }
    }

    #[test]
    fn a_top_level_shape_is_its_own_box() {
        let p = compose(xf(1, 2, 3, 4, -QUARTER), &[], false);
        assert_eq!(
            (p.x, p.y, p.w, p.h, p.rot, p.approx),
            (1, 2, 3, 4, 3 * QUARTER, false)
        );
    }

    #[test]
    fn a_quarter_turned_group_turns_its_children() {
        // the golden sample deck's slide 257: a group turned 90 degrees, scale 1
        let g = GroupXf {
            own: xf(1_000_000, 1_500_000, 4_000_000, 2_000_000, QUARTER),
            ch_x: 0,
            ch_y: 0,
            ch_w: 4_000_000,
            ch_h: 2_000_000,
        };
        let a = compose(xf(0, 0, 2_000_000, 2_000_000, 0), &[g], false);
        assert_eq!(
            (a.x, a.y, a.w, a.h, a.rot),
            (2_000_000, 500_000, 2_000_000, 2_000_000, QUARTER)
        );
        let b = compose(xf(2_000_000, 500_000, 2_000_000, 1_000_000, 0), &[g], false);
        assert_eq!(
            (b.x, b.y, b.w, b.h),
            (2_000_000, 3_000_000, 2_000_000, 1_000_000)
        );
    }
}
