//! Hatches: boundary loops as paths, and a pattern's lines clipped to the boundary.
//!
//! The pattern's definition lines are in drawing units, with the hatch's angle and scale already
//! applied (as the format stores them). Each definition line's family is laid across the
//! boundary's box, cut where it crosses the loops (tessellated for the cut only), kept inside by
//! the island style, then dashed. The segment count is estimated **before** any line is made:
//! over [`HATCH_SEGMENTS_MAX`] the hatch is `dense` (no lines, its definition lines still given),
//! and a hatch that passes the cap while being made is `dense` too: never truncated.

use k2_mpc_base::errors::Value;

use super::fl;

use super::curve::arc_points;
use super::text::plan;
use super::{aci, Edge, HatchRaw, LoopRaw, HATCH_SEGMENTS_MAX, PATH_POINTS_MAX};
use crate::geom;
use crate::mutant;

fn pt(p: [f64; 2]) -> Value {
    Value::List(vec![fl(p[0]), fl(p[1])])
}

/// Point of ellipse `(c, major, ratio)` at parameter `t` (radians).
fn ellipse_at(c: [f64; 2], major: [f64; 2], ratio: f64, t: f64) -> [f64; 2] {
    let minor = [-major[1] * ratio, major[0] * ratio];
    [
        c[0] + major[0] * t.cos() + minor[0] * t.sin(),
        c[1] + major[1] * t.cos() + minor[1] * t.sin(),
    ]
}

/// An edge's start and end angles (radians) and direction, as drawn: a clockwise edge's angles
/// are stored mirrored.
fn angles(a0: f64, a1: f64, ccw: bool) -> (f64, f64) {
    if ccw {
        (a0.to_radians(), a1.to_radians())
    } else {
        (-a0.to_radians(), -a1.to_radians())
    }
}

/// The counter-clockwise sweep from `s` to `e` (a full turn when they meet), or clockwise.
fn sweep(s: f64, e: f64, ccw: bool) -> f64 {
    use std::f64::consts::TAU;
    let d = if ccw { e - s } else { s - e };
    let mut d = d.rem_euclid(TAU);
    if d == 0.0 {
        d = TAU;
    }
    if ccw {
        d
    } else {
        -d
    }
}

/// A loop as a closed polygon in the hatch's frame, each curve within `tol`.
pub fn loop_polygon(l: &LoopRaw, tol: f64) -> Vec<[f64; 2]> {
    let mut out: Vec<[f64; 2]> = Vec::new();
    let add = |p: [f64; 2], out: &mut Vec<[f64; 2]>| {
        if out.last() != Some(&p) && out.len() < PATH_POINTS_MAX * 4 {
            out.push(p);
        }
    };
    if let Some((v, closed)) = &l.poly {
        for (k, (p0, b)) in v.iter().enumerate() {
            add(*p0, &mut out);
            let next = match v.get(k + 1) {
                Some(n) => Some(n.0),
                None if *closed || v.len() > 2 => Some(v[0].0),
                None => None,
            };
            if let (Some(p1), true) = (next, *b != 0.0) {
                for q in bulge_points(*p0, p1, *b, tol) {
                    add(q, &mut out);
                }
            }
        }
        return out;
    }
    for e in &l.edges {
        match e {
            Edge::Line(a, b) => {
                add(*a, &mut out);
                add(*b, &mut out);
            }
            Edge::Arc { c, r, a0, a1, ccw } => {
                let (s, en) = angles(*a0, *a1, *ccw);
                for q in arc_points(*c, *r, s, sweep(s, en, *ccw), tol) {
                    add(q, &mut out);
                }
            }
            Edge::Ellipse {
                c,
                major,
                ratio,
                a0,
                a1,
                ccw,
            } => {
                let (s, en) = angles(*a0, *a1, *ccw);
                let sw = sweep(s, en, *ccw);
                let r = (major[0].hypot(major[1])).max(1e-12);
                for k in arc_points([0.0, 0.0], r, s, sw, tol) {
                    let t = k[1].atan2(k[0]);
                    add(ellipse_at(*c, *major, *ratio, t), &mut out);
                }
            }
            Edge::Spline(s) => {
                if let Ok(p) = super::curve::spline_path(s) {
                    for q in p.points {
                        add(q, &mut out);
                    }
                }
            }
        }
    }
    out
}

/// The points along a bulged segment from `p0` to `p1` (its end included, its start not).
fn bulge_points(p0: [f64; 2], p1: [f64; 2], b: f64, tol: f64) -> Vec<[f64; 2]> {
    let (vx, vy) = (p1[0] - p0[0], p1[1] - p0[1]);
    let l = vx.hypot(vy);
    if l == 0.0 || !b.is_finite() {
        return vec![p1];
    }
    let theta = 4.0 * b.atan();
    let d = (l / 2.0) * (1.0 - b * b) / (2.0 * b);
    let (mx, my) = ((p0[0] + p1[0]) / 2.0, (p0[1] + p1[1]) / 2.0);
    let c = [mx - vy / l * d, my + vx / l * d];
    let r = (p0[0] - c[0]).hypot(p0[1] - c[1]);
    let a0 = (p0[1] - c[1]).atan2(p0[0] - c[0]);
    let mut pts = arc_points(c, r, a0, theta, tol);
    pts.remove(0);
    if let Some(last) = pts.last_mut() {
        *last = p1;
    }
    pts
}

/// The loops whose area the island style hatches: all (odd parity), the outermost, or the
/// external ones only.
fn kept_loops(h: &HatchRaw) -> Vec<&LoopRaw> {
    let pick: Vec<&LoopRaw> = match h.style {
        1 => h.loops.iter().filter(|l| l.flags & (1 | 16) != 0).collect(),
        2 => h.loops.iter().filter(|l| l.flags & 1 != 0).collect(),
        _ => h.loops.iter().collect(),
    };
    if pick.is_empty() {
        h.loops.iter().take(1).collect()
    } else {
        pick
    }
}

/// The pattern's lines, in the hatch's frame, or `None` when dense.
pub fn pattern_lines(h: &HatchRaw, tol: f64) -> Option<Vec<[f64; 4]>> {
    let polys: Vec<Vec<[f64; 2]>> = kept_loops(h)
        .iter()
        .map(|l| loop_polygon(l, tol))
        .filter(|p| p.len() >= 3)
        .collect();
    let mut acc = geom::Acc::new();
    for p in &polys {
        for q in p {
            acc.add(q[0], q[1]);
        }
    }
    let Some(bb) = acc.get() else {
        return Some(Vec::new());
    };
    let diag = (bb[2] - bb[0]).hypot(bb[3] - bb[1]);
    let check_cap = !mutant::active("HatchOverCap");
    // the estimate, before any line is made
    let mut estimate = 0.0f64;
    for d in &h.defs {
        let a = d.angle.to_radians();
        let nrm = [-a.sin(), a.cos()];
        let s = (d.offset[0] * nrm[0] + d.offset[1] * nrm[1]).abs();
        if s.is_nan() || s <= diag * 1e-9 {
            continue;
        }
        let lines = diag / s + 1.0;
        let len: f64 = d.dashes.iter().map(|x| x.abs()).sum();
        let per = if d.dashes.is_empty() || len <= 0.0 {
            1.0
        } else {
            (diag / len + 1.0) * d.dashes.iter().filter(|x| **x >= 0.0).count() as f64
        };
        estimate += lines * per;
    }
    if check_cap && (estimate > HATCH_SEGMENTS_MAX as f64 || !estimate.is_finite()) {
        return None;
    }
    let edges: Vec<([f64; 2], [f64; 2])> = polys
        .iter()
        .flat_map(|p| geom::edges(p).collect::<Vec<_>>())
        .collect();
    let mut out: Vec<[f64; 4]> = Vec::new();
    for d in &h.defs {
        let a = d.angle.to_radians();
        let dir = [a.cos(), a.sin()];
        let nrm = [-a.sin(), a.cos()];
        let on = d.offset[0] * nrm[0] + d.offset[1] * nrm[1];
        if on.is_nan() || on.abs() <= diag * 1e-9 {
            continue;
        }
        let corners = [
            [bb[0], bb[1]],
            [bb[2], bb[1]],
            [bb[0], bb[3]],
            [bb[2], bb[3]],
        ];
        let q: Vec<f64> = corners
            .iter()
            .map(|c| ((c[0] - d.base[0]) * nrm[0] + (c[1] - d.base[1]) * nrm[1]) / on)
            .collect();
        let kmin = q.iter().copied().fold(f64::INFINITY, f64::min).floor() as i64;
        let kmax = q.iter().copied().fold(f64::NEG_INFINITY, f64::max).ceil() as i64;
        let len: f64 = d.dashes.iter().map(|x| x.abs()).sum();
        for k in kmin..=kmax {
            let o = [
                d.base[0] + d.offset[0] * k as f64,
                d.base[1] + d.offset[1] * k as f64,
            ];
            // the line's crossings with the boundary, as parameters along it
            let mut ts: Vec<f64> = Vec::new();
            for (p, r) in &edges {
                let pn = (p[0] - o[0]) * nrm[0] + (p[1] - o[1]) * nrm[1];
                let rn = (r[0] - o[0]) * nrm[0] + (r[1] - o[1]) * nrm[1];
                if (pn > 0.0) != (rn > 0.0) {
                    let f = pn / (pn - rn);
                    let x = [p[0] + (r[0] - p[0]) * f, p[1] + (r[1] - p[1]) * f];
                    ts.push((x[0] - o[0]) * dir[0] + (x[1] - o[1]) * dir[1]);
                }
            }
            ts.sort_by(f64::total_cmp);
            let spans: Vec<(f64, f64)> = if mutant::active("HatchUnclipped") {
                let ex: Vec<f64> = corners
                    .iter()
                    .map(|c| (c[0] - o[0]) * dir[0] + (c[1] - o[1]) * dir[1])
                    .collect();
                vec![(
                    ex.iter().copied().fold(f64::INFINITY, f64::min),
                    ex.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                )]
            } else {
                ts.chunks_exact(2).map(|w| (w[0], w[1])).collect()
            };
            let at = |t: f64| [o[0] + dir[0] * t, o[1] + dir[1] * t];
            for (ta, tb) in spans {
                if tb <= ta {
                    continue;
                }
                if d.dashes.is_empty() || len <= 0.0 {
                    let (p, r) = (at(ta), at(tb));
                    out.push([p[0], p[1], r[0], r[1]]);
                } else {
                    let mut pos = (ta / len).floor() * len;
                    'pattern: while pos < tb {
                        for x in &d.dashes {
                            let (s, e) = (pos, pos + x.abs());
                            if *x >= 0.0 && e >= ta && s <= tb {
                                let (s, e) = (s.max(ta), e.min(tb));
                                if *x == 0.0 || e > s {
                                    let (p, r) = (at(s), at(e));
                                    out.push([p[0], p[1], r[0], r[1]]);
                                }
                            }
                            pos = e;
                            if check_cap && out.len() > HATCH_SEGMENTS_MAX {
                                return None;
                            }
                            if pos >= tb {
                                break 'pattern;
                            }
                        }
                    }
                }
                if check_cap && out.len() > HATCH_SEGMENTS_MAX {
                    return None;
                }
            }
        }
    }
    Some(out)
}

fn edge_value(e: &Edge, n: [f64; 3], tol: f64) -> Value {
    let up = geom::is_up(n);
    match e {
        Edge::Line(a, b) => Value::obj([(
            "line",
            Value::List(vec![
                pt(plan([a[0], a[1], 0.0], n)),
                pt(plan([b[0], b[1], 0.0], n)),
            ]),
        )]),
        Edge::Arc { c, r, a0, a1, ccw } if up => {
            let (s, e) = angles(*a0, *a1, *ccw);
            Value::obj([(
                "arc",
                Value::obj([
                    ("c", pt(*c)),
                    ("r", fl(*r)),
                    ("a0", fl(s)),
                    ("a1", fl(e)),
                    ("ccw", Value::from(*ccw)),
                ]),
            )])
        }
        Edge::Ellipse {
            c,
            major,
            ratio,
            a0,
            a1,
            ccw,
        } if up => {
            let (s, e) = angles(*a0, *a1, *ccw);
            Value::obj([(
                "ellipse",
                Value::obj([
                    ("c", pt(*c)),
                    ("major", pt(*major)),
                    ("ratio", fl(*ratio)),
                    ("p0", fl(s)),
                    ("p1", fl(e)),
                    ("ccw", Value::from(*ccw)),
                ]),
            )])
        }
        other => {
            // a spline edge, or a curve in a frame other than the plan's: a path
            let l = LoopRaw {
                flags: 0,
                edges: vec![other.clone()],
                poly: None,
            };
            let pts: Vec<Value> = loop_polygon(&l, tol)
                .into_iter()
                .map(|p| pt(plan([p[0], p[1], 0.0], n)))
                .collect();
            Value::obj([("path", Value::List(pts))])
        }
    }
}

/// The tolerance a hatch's curves are cut at: 1e-4 of its boundary's size.
pub fn hatch_tol(h: &HatchRaw) -> f64 {
    let mut acc = geom::Acc::new();
    for l in &h.loops {
        if let Some((v, _)) = &l.poly {
            for (p, _) in v {
                acc.add(p[0], p[1]);
            }
        }
        for e in &l.edges {
            match e {
                Edge::Line(a, b) => {
                    acc.add(a[0], a[1]);
                    acc.add(b[0], b[1]);
                }
                Edge::Arc { c, r, .. } => {
                    acc.add(c[0] - r, c[1] - r);
                    acc.add(c[0] + r, c[1] + r);
                }
                Edge::Ellipse { c, major, .. } => {
                    let r = major[0].hypot(major[1]);
                    acc.add(c[0] - r, c[1] - r);
                    acc.add(c[0] + r, c[1] + r);
                }
                Edge::Spline(s) => {
                    for p in &s.ctrl {
                        acc.add(p[0], p[1]);
                    }
                }
            }
        }
    }
    let d = acc.get().map_or(0.0, |b| (b[2] - b[0]).hypot(b[3] - b[1]));
    (1e-4 * d).max(1e-9)
}

/// A hatch's `geom` object.
pub fn value(h: &HatchRaw) -> Value {
    let tol = hatch_tol(h);
    let n = h.n;
    let loops: Vec<Value> = h
        .loops
        .iter()
        .map(|l| {
            let mut v: Vec<(&str, Value)> = vec![("flags", Value::from(l.flags))];
            if let Some((verts, closed)) = &l.poly {
                if geom::is_up(n) {
                    v.push((
                        "poly",
                        Value::obj([
                            (
                                "verts",
                                Value::List(
                                    verts
                                        .iter()
                                        .map(|(p, b)| {
                                            Value::obj([("p", pt(*p)), ("bulge", fl(*b))])
                                        })
                                        .collect(),
                                ),
                            ),
                            ("closed", Value::from(*closed)),
                        ]),
                    ));
                } else {
                    let pts = super::emit::ring(loop_polygon(l, tol))
                        .into_iter()
                        .map(|p| pt(plan([p[0], p[1], h.elevation], n)))
                        .collect();
                    v.push(("path", Value::List(pts)));
                    v.push(("closed", Value::from(true)));
                }
            } else {
                v.push((
                    "edges",
                    Value::List(l.edges.iter().map(|e| edge_value(e, n, tol)).collect()),
                ));
            }
            Value::obj(v)
        })
        .collect();
    let fill = if h.gradient.is_some() {
        "gradient"
    } else if h.solid {
        "solid"
    } else {
        "pattern"
    };
    let pattern = if fill == "pattern" {
        let (lines, dense) = match pattern_lines(h, tol) {
            Some(segs) => (
                Value::List(
                    segs.iter()
                        .map(|s| {
                            let a = plan([s[0], s[1], h.elevation], n);
                            let b = plan([s[2], s[3], h.elevation], n);
                            Value::List(vec![fl(a[0]), fl(a[1]), fl(b[0]), fl(b[1])])
                        })
                        .collect(),
                ),
                false,
            ),
            None => (Value::Null, true),
        };
        Value::obj([
            ("name", Value::from(h.pattern.as_str())),
            ("angle", fl(h.angle.to_radians())),
            ("scale", fl(h.scale)),
            ("double", Value::from(h.double)),
            ("units", Value::from("drawing")),
            (
                "defs",
                Value::List(
                    h.defs
                        .iter()
                        .map(|d| {
                            Value::obj([
                                ("angle", fl(d.angle.to_radians())),
                                ("base", pt(d.base)),
                                ("offset", pt(d.offset)),
                                (
                                    "dashes",
                                    Value::List(d.dashes.iter().map(|x| fl(*x)).collect()),
                                ),
                            ])
                        })
                        .collect(),
                ),
            ),
            ("lines", lines),
            ("dense", Value::from(dense)),
        ])
    } else {
        Value::Null
    };
    let gradient = match &h.gradient {
        Some(g) => Value::obj([
            ("name", Value::from(g.name.as_str())),
            ("angle", fl(g.angle.to_radians())),
            ("shift", fl(g.shift)),
            ("one_colour", Value::from(g.one_colour)),
            (
                "colours",
                Value::List(
                    g.colours
                        .iter()
                        .map(|(n, c)| {
                            c.map(aci::from_u32)
                                .or_else(|| n.and_then(aci::rgb))
                                .map_or(Value::Null, |c| {
                                    Value::List(
                                        c.iter().map(|x| Value::from(u64::from(*x))).collect(),
                                    )
                                })
                        })
                        .collect(),
                ),
            ),
        ]),
        None => Value::Null,
    };
    Value::obj([
        ("loops", Value::List(loops)),
        ("island_style", Value::from(h.style)),
        ("fill", Value::from(fill)),
        ("pattern", pattern),
        ("gradient", gradient),
        (
            "assoc",
            Value::List(
                h.assoc
                    .iter()
                    .map(|a| Value::from(crate::handle::key(*a)))
                    .collect(),
            ),
        ),
    ])
}
