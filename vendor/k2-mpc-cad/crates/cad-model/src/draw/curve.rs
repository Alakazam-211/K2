//! Curves as paths with a stated tolerance: splines (B-splines, rational or not) and arcs in a
//! tilted frame; and the plan transforms of block references.
//!
//! **Spline tolerance, guaranteed rather than sampled:** the spline is split into Bézier pieces
//! (each piece's control points are blossom values, computed from the spline's own control
//! points and knots), and each piece is halved until every one of its control points is within
//! the tolerance of the chord between its ends. With positive weights a piece lies inside the
//! hull of its control points, so every point of the curve is then within the tolerance of the
//! path.

use super::{SplineRaw, PATH_POINTS_MAX};
use crate::mutant;

/// A spline's path.
#[derive(Debug, Clone, PartialEq)]
pub struct SplinePath {
    pub points: Vec<[f64; 2]>,
    /// The tolerance asked for.
    pub tol: f64,
    /// The tolerance the path meets (the asked one, unless the point cap made it coarser).
    pub achieved: f64,
}

/// Why a spline has no path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoPath {
    /// A weight at or below zero.
    Weights,
    /// Its values don't make a spline (counts, order, knots).
    Unreadable,
    /// More pieces than the point cap.
    OverCap,
}

/// The tolerance for a spline with these control points: 1e-4 of their box's diagonal, and at
/// least 1e-9 of the largest coordinate (so it is never zero).
pub fn spline_tol(ctrl: &[[f64; 3]]) -> f64 {
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    let mut big = 0.0f64;
    for p in ctrl {
        lo = [lo[0].min(p[0]), lo[1].min(p[1])];
        hi = [hi[0].max(p[0]), hi[1].max(p[1])];
        big = big.max(p[0].abs()).max(p[1].abs());
    }
    let diag = ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2)).sqrt();
    let t = (1e-4 * diag).max(1e-9 * big);
    if t.is_finite() && t > 0.0 {
        t
    } else {
        1e-12
    }
}

type H = [f64; 3]; // x·w, y·w, w

fn lerp(a: H, b: H, t: f64) -> H {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// The blossom of the span `[U[k], U[k+1]]` at `args` (p values): de Boor with one argument per
/// level.
fn blossom(u: &[f64], pw: &[H], p: usize, k: usize, args: &[f64]) -> H {
    let mut d: Vec<H> = pw[k - p..=k].to_vec();
    for (r, &x) in args.iter().enumerate() {
        let r = r + 1;
        for j in (r..=p).rev() {
            let i = k - p + j;
            let den = u[i + p + 1 - r] - u[i];
            let a = if den > 0.0 { (x - u[i]) / den } else { 0.0 };
            d[j] = lerp(d[j - 1], d[j], a);
        }
    }
    d[p]
}

fn project(h: H) -> [f64; 2] {
    [h[0] / h[2], h[1] / h[2]]
}

fn dist_to_segment(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    crate::geom::to_segment(p, a, b)
}

/// The Bézier pieces of a spline, in homogeneous coordinates.
fn pieces(s: &SplineRaw) -> Result<Vec<Vec<H>>, NoPath> {
    let p = usize::try_from(s.degree).map_err(|_| NoPath::Unreadable)?;
    let n = s.ctrl.len();
    if p == 0 || p > 25 || n < p + 1 || s.knots.len() != n + p + 1 {
        return Err(NoPath::Unreadable);
    }
    if !s.weights.is_empty() && s.weights.len() != n {
        return Err(NoPath::Unreadable);
    }
    if s.weights
        .iter()
        .any(|w| w.is_nan() || *w <= 0.0 || !w.is_finite())
    {
        return Err(NoPath::Weights);
    }
    let u = &s.knots;
    if u.iter().any(|x| !x.is_finite()) || u.windows(2).any(|w| w[1] < w[0]) {
        return Err(NoPath::Unreadable);
    }
    if s.ctrl
        .iter()
        .any(|c| !c[0].is_finite() || !c[1].is_finite())
    {
        return Err(NoPath::Unreadable);
    }
    let pw: Vec<H> = s
        .ctrl
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let w = s.weights.get(i).copied().unwrap_or(1.0);
            [c[0] * w, c[1] * w, w]
        })
        .collect();
    let mut out = Vec::new();
    // the domain's spans: [U[k], U[k+1]] for k in p..n, non-empty
    for k in p..n {
        let (a, b) = (u[k], u[k + 1]);
        if b <= a {
            continue;
        }
        if out.len() >= PATH_POINTS_MAX {
            return Err(NoPath::OverCap);
        }
        let mut piece = Vec::with_capacity(p + 1);
        for j in 0..=p {
            let mut args = vec![a; p - j];
            args.resize(p, b);
            piece.push(blossom(u, &pw, p, k, &args));
        }
        out.push(piece);
    }
    if out.is_empty() {
        return Err(NoPath::Unreadable);
    }
    Ok(out)
}

/// Halves Bézier piece `b` until flat within `tol`, appending each end point after the first
/// to `out`; stops (returning false) when `out` would pass `cap` points.
fn flatten(b: &[H], tol: f64, depth: u32, out: &mut Vec<[f64; 2]>, cap: usize) -> bool {
    let a = project(b[0]);
    let z = project(b[b.len() - 1]);
    let flat = b[1..b.len() - 1]
        .iter()
        .all(|h| dist_to_segment(project(*h), a, z) <= tol);
    if flat || depth >= 40 {
        if out.len() >= cap {
            return false;
        }
        out.push(z);
        return true;
    }
    // de Casteljau at one half
    let mut left = Vec::with_capacity(b.len());
    let mut right = Vec::with_capacity(b.len());
    let mut cur = b.to_vec();
    left.push(cur[0]);
    right.push(cur[cur.len() - 1]);
    while cur.len() > 1 {
        cur = cur.windows(2).map(|w| lerp(w[0], w[1], 0.5)).collect();
        left.push(cur[0]);
        right.push(cur[cur.len() - 1]);
    }
    right.reverse();
    flatten(&left, tol, depth + 1, out, cap) && flatten(&right, tol, depth + 1, out, cap)
}

/// A spline's path at its tolerance ([`spline_tol`]), at most [`PATH_POINTS_MAX`] points.
pub fn spline_path(s: &SplineRaw) -> Result<SplinePath, NoPath> {
    let pcs = pieces(s)?;
    let tol = spline_tol(&s.ctrl);
    // the planted bug: a path a hundred times looser than it says
    let mut use_tol = if mutant::active("SplineTolLoose") {
        tol * 100.0
    } else {
        tol
    };
    loop {
        let mut pts = vec![project(pcs[0][0])];
        let ok = pcs
            .iter()
            .all(|b| flatten(b, use_tol, 0, &mut pts, PATH_POINTS_MAX));
        if ok {
            return Ok(SplinePath {
                points: pts,
                tol,
                achieved: if mutant::active("SplineTolLoose") {
                    tol
                } else {
                    use_tol
                },
            });
        }
        use_tol *= 2.0;
        if !use_tol.is_finite() {
            return Err(NoPath::OverCap);
        }
    }
}

/// The points of the arc of centre `c` and radius `r` from `a0` turning counter-clockwise by
/// `sweep` (radians), each chord within `tol` of the arc.
pub fn arc_points(c: [f64; 2], r: f64, a0: f64, sweep: f64, tol: f64) -> Vec<[f64; 2]> {
    let r = r.abs();
    let max_step = if r > tol {
        2.0 * (1.0 - tol / r).clamp(-1.0, 1.0).acos()
    } else {
        std::f64::consts::FRAC_PI_2
    };
    let n = ((sweep.abs() / max_step.max(1e-9)).ceil() as usize).clamp(1, PATH_POINTS_MAX - 1);
    (0..=n)
        .map(|k| {
            let a = a0 + sweep * k as f64 / n as f64;
            [c[0] + r * a.cos(), c[1] + r * a.sin()]
        })
        .collect()
}

/// A 2D affine map `[a, b, c, d, e, f]`: `x' = a x + c y + e`, `y' = b x + d y + f`.
pub type Mat = [f64; 6];

/// `m` then `n` (`n · m`).
pub fn then(m: Mat, n: Mat) -> Mat {
    [
        n[0] * m[0] + n[2] * m[1],
        n[1] * m[0] + n[3] * m[1],
        n[0] * m[2] + n[2] * m[3],
        n[1] * m[2] + n[3] * m[3],
        n[0] * m[4] + n[2] * m[5] + n[4],
        n[1] * m[4] + n[3] * m[5] + n[5],
    ]
}

pub fn apply(m: Mat, p: [f64; 2]) -> [f64; 2] {
    [
        m[0] * p[0] + m[2] * p[1] + m[4],
        m[1] * p[0] + m[3] * p[1] + m[5],
    ]
}

/// A block reference's map from the block's coordinates to the world plan: minus the block's
/// base point, scaled, rotated (degrees), moved to its insertion point, in its frame `n`.
pub fn insert_matrix(
    base: [f64; 2],
    at: [f64; 3],
    scale: [f64; 3],
    rot_deg: f64,
    n: [f64; 3],
) -> Mat {
    let (s, c) = rot_deg.to_radians().sin_cos();
    let local: Mat = [
        scale[0] * c,
        scale[0] * s,
        -scale[1] * s,
        scale[1] * c,
        0.0,
        0.0,
    ];
    let to_base: Mat = [1.0, 0.0, 0.0, 1.0, -base[0], -base[1]];
    let mut m = then(to_base, local);
    m[4] += at[0];
    m[5] += at[1];
    if mutant::active("BasePointIgnored") {
        m = then([1.0, 0.0, 0.0, 1.0, 0.0, 0.0], local);
        m[4] += at[0];
        m[5] += at[1];
    }
    if crate::geom::is_up(n) {
        return m;
    }
    // the frame's axes in the world plan, the elevation's offset with them
    let ax = crate::geom::to_world([1.0, 0.0, 0.0], n).unwrap_or([1.0, 0.0, 0.0]);
    let ay = crate::geom::to_world([0.0, 1.0, 0.0], n).unwrap_or([0.0, 1.0, 0.0]);
    let az = crate::geom::to_world([0.0, 0.0, at[2]], n).unwrap_or([0.0, 0.0, 0.0]);
    then(m, [ax[0], ax[1], ay[0], ay[1], az[0], az[1]])
}

/// The box of box `b` under `m`.
pub fn map_box(m: Mat, b: [f64; 4]) -> [f64; 4] {
    let mut acc = crate::geom::Acc::new();
    for p in [[b[0], b[1]], [b[2], b[1]], [b[0], b[3]], [b[2], b[3]]] {
        let q = apply(m, p);
        acc.add(q[0], q[1]);
    }
    acc.get().unwrap_or(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamped_cubic_ends_at_its_end_points() {
        let s = SplineRaw {
            degree: 3,
            flags: 8,
            knots: vec![0.0, 0.0, 0.0, 0.0, 1.0, 2.0, 2.0, 2.0, 2.0],
            ctrl: vec![
                [0.0, 0.0, 0.0],
                [1.0, 2.0, 0.0],
                [3.0, 2.0, 0.0],
                [4.0, 0.0, 0.0],
                [5.0, 1.0, 0.0],
            ],
            weights: vec![],
            fit: vec![],
        };
        let p = spline_path(&s).expect("a path");
        assert_eq!(p.points.first(), Some(&[0.0, 0.0]));
        let last = p.points.last().copied().unwrap_or_default();
        assert!((last[0] - 5.0).abs() < 1e-12 && (last[1] - 1.0).abs() < 1e-12);
        assert!(p.points.len() > 4);
        assert_eq!(p.achieved, p.tol);
    }

    #[test]
    fn insert_matrix_moves_base_to_insertion() {
        let m = insert_matrix(
            [1.0, 1.0],
            [10.0, 0.0, 0.0],
            [2.0, 2.0, 1.0],
            90.0,
            [0.0, 0.0, 1.0],
        );
        let q = apply(m, [1.0, 1.0]);
        assert!((q[0] - 10.0).abs() < 1e-12 && q[1].abs() < 1e-12);
        let r = apply(m, [2.0, 1.0]);
        assert!((r[0] - 10.0).abs() < 1e-12 && (r[1] - 2.0).abs() < 1e-12);
    }
}
