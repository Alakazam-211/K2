//! `entity rotate` and `entity scale`: each entity turned or scaled about a point, its followers
//! (attributes, vertices) with it, by the point table the move uses.
//!
//! - **Rotate** `--by DEG --about x,y`: points turn about the centre (in plan: `z` stays);
//!   direction vectors turn (an ellipse's major axis, a multiline text's direction, a spline's
//!   end tangents); angles in degrees gain `DEG`, put in `[0, 360)` (an arc's ends, a text's,
//!   a block reference's and its attributes' rotation; written in when the entity has none);
//!   a multiline text with no direction gains `DEG` in radians on its rotation. Bulges stay.
//!   Quarter turns are exact: each coordinate is the centre plus or minus the other's offset,
//!   added in decimal on the file's tokens; any other angle is computed in `f64` and written
//!   as the shortest decimal that reads back.
//! - **Scale** `--factor F --about x,y` (F above 0, uniform): points move away from the centre
//!   by F (`z` from 0 by F); sizes scale by F: radii, text heights and a multiline text's
//!   widths, a block reference's three scales (written in when it has none), polyline widths,
//!   an ellipse's major axis. Angles, ratios and bulges stay. Products are exact decimals
//!   where they can be.

use std::collections::{BTreeSet, HashMap};

use k2_mpc_cad_model::args::{self, Spec};

use super::check::{Intent, Want};
use super::ops::{addresses, data_pairs, keep_spaces, list, point_codes};
use super::write::Writer;
use super::{num, rules, Cx, Out};
use crate::error::{CadError, VALUE_NOT_WRITABLE};
use crate::structure::{Structure, Unit};
use crate::{handle, mutant};

const ENTITY_ROTATE: Spec = Spec {
    verb: "entity rotate",
    valued: &["--by", "--about", "--unit"],
    bare: &[],
    words: true,
};

const ENTITY_SCALE: Spec = Spec {
    verb: "entity scale",
    valued: &["--factor", "--about", "--unit"],
    bare: &[],
    words: true,
};

/// What a call does to each number.
enum Xf {
    /// A turn by `deg` (as given), `quarter` turns when it is a multiple of 90.
    Rotate {
        deg: String,
        quarter: Option<u8>,
        cos: f64,
        sin: f64,
        c: [String; 2],
        /// Below this, a computed coordinate is the sine's and cosine's noise: written 0.
        eps: f64,
    },
    /// A scale by `f` (as given).
    Scale { f: String, c: [String; 2] },
}

/// `v`, or 0 when its magnitude is below `eps` (a turn's sine and cosine noise: zero but for
/// the last bits a platform's maths library computes).
fn snap(v: f64, eps: f64) -> f64 {
    if v.abs() < eps && !mutant::active("RotateNoiseKept") {
        0.0
    } else {
        v
    }
}

/// The size below which a computed coordinate of `doc` is noise: 1e-12 of its scale (the
/// drawing-model read's rule, [`k2_mpc_cad_model::draw::noise_scale`]).
pub(super) fn noise_eps(doc: &k2_mpc_cad_model::Doc) -> f64 {
    1e-12 * k2_mpc_cad_model::draw::noise_scale(doc)
}

fn bad_number(h: u64) -> CadError {
    CadError::new(
        VALUE_NOT_WRITABLE,
        format!(
            "a number of {} isn't one the verb can write (not a number, or not finite after it)",
            handle::key(h)
        ),
        "save the drawing again in the app",
    )
}

impl Xf {
    /// A point `(x, y)` as moved.
    fn point(&self, x: &str, y: &str) -> Option<(String, String)> {
        match self {
            Xf::Rotate {
                quarter,
                cos,
                sin,
                c,
                eps,
                ..
            } => {
                let (dx, dy) = (num::sub(x, &c[0])?, num::sub(y, &c[1])?);
                match quarter {
                    Some(0) => Some((x.to_owned(), y.to_owned())),
                    Some(1) => Some((num::sub(&c[0], &dy)?, num::add(&c[1], &dx)?)),
                    Some(2) => Some((num::sub(&c[0], &dx)?, num::sub(&c[1], &dy)?)),
                    Some(_) => Some((num::add(&c[0], &dy)?, num::sub(&c[1], &dx)?)),
                    None => {
                        let (dx, dy) = (num::parse(&dx)?, num::parse(&dy)?);
                        let cx = num::parse(&c[0])?;
                        let cy = num::parse(&c[1])?;
                        let nx = snap(cx + dx * cos - dy * sin, *eps);
                        let ny = snap(cy + dx * sin + dy * cos, *eps);
                        (nx.is_finite() && ny.is_finite())
                            .then(|| (num::fmt_f64(nx), num::fmt_f64(ny)))
                    }
                }
            }
            Xf::Scale { f, c } => {
                let about = |v: &str, c: &str| -> Option<String> {
                    if mutant::active("ScaleAboutOrigin") {
                        return num::mul(v, f);
                    }
                    num::add(c, &num::mul(&num::sub(v, c)?, f)?)
                };
                Some((about(x, &c[0])?, about(y, &c[1])?))
            }
        }
    }

    /// A direction vector `(x, y)` as turned or scaled.
    fn vector(&self, x: &str, y: &str) -> Option<(String, String)> {
        match self {
            Xf::Rotate {
                quarter,
                cos,
                sin,
                eps,
                ..
            } => match quarter {
                Some(0) => Some((x.to_owned(), y.to_owned())),
                Some(1) => Some((num::neg(y), x.to_owned())),
                Some(2) => Some((num::neg(x), num::neg(y))),
                Some(_) => Some((y.to_owned(), num::neg(x))),
                None => {
                    let (x, y) = (num::parse(x)?, num::parse(y)?);
                    let (nx, ny) = (snap(x * cos - y * sin, *eps), snap(x * sin + y * cos, *eps));
                    (nx.is_finite() && ny.is_finite()).then(|| (num::fmt_f64(nx), num::fmt_f64(ny)))
                }
            },
            Xf::Scale { f, .. } => Some((num::mul(x, f)?, num::mul(y, f)?)),
        }
    }
}

/// The kinds' numbers beyond points, per verb class.
struct Codes {
    /// Direction vectors (x codes; y is ten more).
    vectors: &'static [i32],
    /// Angles in degrees, and whether one is written in when the unit has none.
    degrees: &'static [i32],
    degrees_add: bool,
    /// Sizes a scale multiplies.
    sizes: &'static [i32],
    /// Sizes written in (at F) when the unit has none: a block reference's scales.
    sizes_add: &'static [i32],
}

fn codes_of(s: &Structure<'_>, u: &Unit, kind: &str) -> Codes {
    let none: &'static [i32] = &[];
    let mut c = Codes {
        vectors: none,
        degrees: none,
        degrees_add: false,
        sizes: none,
        sizes_add: none,
    };
    match kind {
        "ARC" => {
            c.degrees = &[50, 51];
            c.sizes = &[40];
        }
        "CIRCLE" => c.sizes = &[40],
        "ELLIPSE" => {
            c.vectors = &[11];
            c.sizes = &[];
        }
        "SPLINE" => c.vectors = &[12, 13],
        "TEXT" | "ATTRIB" => {
            c.degrees = &[50];
            c.degrees_add = true;
            c.sizes = &[40];
        }
        "MTEXT" => {
            if super::ops::first(s, u, 11).is_some() {
                c.vectors = &[11];
            }
            c.sizes = &[40, 41, 46];
        }
        "INSERT" => {
            c.degrees = &[50];
            c.degrees_add = true;
            c.sizes = &[41, 42, 43];
            c.sizes_add = &[41, 42, 43];
        }
        "LWPOLYLINE" => c.sizes = &[40, 41, 43],
        "POLYLINE" | "VERTEX" => c.sizes = &[40, 41],
        _ => {}
    }
    c
}

/// Where a pair of code `code` goes in unit `u` when written in: in the kind's own subclass
/// group (after the first subclass marker but `AcDbEntity`, up to the next marker), before its
/// first pair whose code comes after `code` in `order`, else at the group's end.
fn slot(s: &Structure<'_>, u: &Unit, code: i32, order: &[i32]) -> usize {
    let data = data_pairs(s, u);
    let marker = |p: usize| s.tok.pairs[p].code == 100;
    let start = data
        .iter()
        .position(|&p| marker(p) && !s.text(p).trim().eq_ignore_ascii_case("AcDbEntity"))
        .map_or(0, |k| k + 1);
    let end = data[start..]
        .iter()
        .position(|&p| marker(p))
        .map_or(data.len(), |k| start + k);
    let rank = |c: i32| order.iter().position(|&x| x == c);
    let r = rank(code).unwrap_or(usize::MAX);
    let group = &data[start..end];
    if group.is_empty() {
        return data.get(end).copied().unwrap_or(u.end);
    }
    for &p in group {
        if rank(s.tok.pairs[p].code).is_some_and(|x| x > r) {
            return p;
        }
    }
    group.last().map_or(u.end, |&p| p + 1)
}

/// The order of a kind's own codes, for a pair written in.
fn order_of(kind: &str) -> &'static [i32] {
    match kind {
        "INSERT" => &[
            66, 2, 10, 20, 30, 41, 42, 43, 50, 70, 71, 44, 45, 210, 220, 230,
        ],
        "MTEXT" => &[
            10, 20, 30, 40, 41, 46, 71, 72, 3, 1, 7, 210, 220, 230, 11, 21, 31, 42, 43, 50, 73, 44,
        ],
        _ => &[
            10, 20, 30, 40, 1, 50, 41, 51, 7, 71, 72, 11, 21, 31, 210, 220, 230,
        ],
    }
}

/// `--about x,y`: its two numbers as given.
fn about(sp: &Spec, v: &str) -> Result<[String; 2], CadError> {
    let parts: Vec<&str> = v.split(',').map(str::trim).collect();
    match parts.as_slice() {
        [x, y] if num::parse(x).is_some() && num::parse(y).is_some() => {
            Ok([(*x).to_owned(), (*y).to_owned()])
        }
        _ => Err(args::usage(sp, "--about takes x,y: two numbers")),
    }
}

/// The pair index's place among the entity's pairs of its code (head and followers), from 0.
fn nth_map(s: &Structure<'_>, units: &[&Unit]) -> HashMap<usize, usize> {
    let mut seen: HashMap<i32, usize> = HashMap::new();
    let mut out = HashMap::new();
    for u in units {
        for (p, pair) in s.tok.pairs.iter().enumerate().take(u.end).skip(u.first) {
            let n = seen.entry(pair.code).or_default();
            out.insert(p, *n);
            *n += 1;
        }
    }
    out
}

/// One entity turned or scaled: every number the table names, written where it changes.
fn entity(
    cx: &Cx<'_, '_>,
    w: &mut Writer<'_, '_>,
    it: &mut Intent,
    i: usize,
    xf: &Xf,
) -> Result<(), CadError> {
    let s = cx.s;
    let eu = &s.entities[i];
    let h = cx.doc.entities[i].handle.unwrap_or(0);
    let units: Vec<&Unit> = std::iter::once(&eu.head).chain(&eu.followers).collect();
    let nth = nth_map(s, &units);
    let rotate = matches!(xf, Xf::Rotate { .. });
    // a pair written in goes at its place among the entity's pairs of its code
    let nth_at = |code: i32, at: usize| {
        units
            .iter()
            .flat_map(|u| u.first..u.end)
            .filter(|&p| p < at && s.tok.pairs[p].code == code)
            .count()
    };
    let put = |w: &mut Writer<'_, '_>, it: &mut Intent, p: usize, new: &str| {
        let old = s.tok.value(p);
        let value = keep_spaces(old, new);
        w.set_value(p, &value);
        it.wants.push(Want::Pair {
            h,
            code: s.tok.pairs[p].code,
            nth: nth[&p],
            value,
        });
    };
    let text = |p: usize| String::from_utf8_lossy(s.tok.value(p)).trim().to_owned();
    for u in &units {
        let kind = u.kind.as_ref();
        let xs = point_codes(s, u, kind);
        let codes = codes_of(s, u, kind);
        let data = data_pairs(s, u);
        let mut done: BTreeSet<usize> = BTreeSet::new();
        // the y pair of the x pair at `p`: the next of code `c + 10` before another `c`
        let y_of = |p: usize, c: i32| {
            data.iter()
                .copied()
                .filter(|&q| q > p)
                .take_while(|&q| s.tok.pairs[q].code != c)
                .find(|&q| s.tok.pairs[q].code == c + 10)
        };
        let mut seen_deg: BTreeSet<i32> = BTreeSet::new();
        let mut seen_size: BTreeSet<i32> = BTreeSet::new();
        for &p in &data {
            if done.contains(&p) {
                continue;
            }
            let c = s.tok.pairs[p].code;
            let is_point = xs.contains(&c);
            // a spline's end tangents are directions: a scale leaves them
            let is_vector = codes.vectors.contains(&c) && (rotate || kind != "SPLINE");
            if is_point || is_vector {
                let Some(q) = y_of(p, c) else { continue };
                done.insert(q);
                let (x, y) = (text(p), text(q));
                let moved = if is_point {
                    xf.point(&x, &y)
                } else {
                    xf.vector(&x, &y)
                };
                let (nx, ny) = moved.ok_or_else(|| bad_number(h))?;
                put(w, it, p, &nx);
                put(w, it, q, &ny);
                // a scale takes z from 0 by F: the point's z, ten past its y
                if let (Xf::Scale { f, .. }, true) = (xf, is_point) {
                    if let Some(z) = data
                        .iter()
                        .copied()
                        .filter(|&r| r > q)
                        .take_while(|&r| s.tok.pairs[r].code != c)
                        .find(|&r| s.tok.pairs[r].code == c + 20)
                    {
                        done.insert(z);
                        let nz = num::mul(&text(z), f).ok_or_else(|| bad_number(h))?;
                        put(w, it, z, &nz);
                    }
                }
                continue;
            }
            match xf {
                Xf::Rotate { deg, .. } if codes.degrees.contains(&c) => {
                    seen_deg.insert(c);
                    if mutant::active("AngleNotTurned") {
                        continue;
                    }
                    let a = num::add(&text(p), deg)
                        .and_then(|a| num::turn(&a))
                        .ok_or_else(|| bad_number(h))?;
                    put(w, it, p, &a);
                }
                Xf::Rotate { deg, .. }
                    if kind == "MTEXT" && c == 50 && codes.vectors.is_empty() =>
                {
                    seen_deg.insert(c);
                    let r = num::parse(&text(p)).ok_or_else(|| bad_number(h))?
                        + num::parse(deg).ok_or_else(|| bad_number(h))?.to_radians();
                    let r = r.rem_euclid(std::f64::consts::TAU);
                    put(w, it, p, &num::fmt_f64(r));
                }
                Xf::Scale { f, .. } if codes.sizes.contains(&c) => {
                    seen_size.insert(c);
                    if mutant::active("SizesLeft") {
                        continue;
                    }
                    let v = num::mul(&text(p), f).ok_or_else(|| bad_number(h))?;
                    put(w, it, p, &v);
                }
                Xf::Scale { f, .. } if kind == "LWPOLYLINE" && c == 38 => {
                    let v = num::mul(&text(p), f).ok_or_else(|| bad_number(h))?;
                    put(w, it, p, &v);
                }
                _ => {}
            }
        }
        // the pairs a unit lacks, written in
        let order = order_of(kind);
        match xf {
            Xf::Rotate { deg, .. } => {
                let missing_deg = codes.degrees_add && !seen_deg.contains(&50);
                let mtext_rad =
                    kind == "MTEXT" && codes.vectors.is_empty() && !seen_deg.contains(&50);
                if (missing_deg || mtext_rad) && !mutant::active("AngleNotTurned") {
                    let v = if mtext_rad {
                        let r = num::parse(deg)
                            .ok_or_else(|| bad_number(h))?
                            .to_radians()
                            .rem_euclid(std::f64::consts::TAU);
                        num::fmt_f64(r)
                    } else {
                        num::turn(deg).ok_or_else(|| bad_number(h))?
                    };
                    if num::parse(&v) != Some(0.0) {
                        let at = slot(s, u, 50, order);
                        let value = v.into_bytes();
                        it.wants.push(Want::Pair {
                            h,
                            code: 50,
                            nth: nth_at(50, at),
                            value: value.clone(),
                        });
                        w.insert_before(at, &[(50, value)]);
                    }
                }
            }
            Xf::Scale { f, .. } if !mutant::active("SizesLeft") => {
                let add: Vec<i32> = codes
                    .sizes_add
                    .iter()
                    .copied()
                    .filter(|c| !seen_size.contains(c))
                    .collect();
                for code in add {
                    let at = slot(s, u, code, order);
                    let value = f.clone().into_bytes();
                    it.wants.push(Want::Pair {
                        h,
                        code,
                        nth: nth_at(code, at),
                        value: value.clone(),
                    });
                    w.insert_before(at, &[(code, value)]);
                }
            }
            _ => {}
        }
    }
    it.changed.insert(h);
    Ok(())
}

fn targets(
    cx: &Cx<'_, '_>,
    sp: &Spec,
    words: &[String],
) -> Result<(Vec<String>, Vec<usize>), CadError> {
    let addrs = addresses(sp, words)?;
    let mut idx = Vec::new();
    for addr in &addrs {
        let i = cx.entity(addr)?;
        rules::entity(cx.s, cx.doc, i)?;
        idx.push(i);
    }
    Ok((addrs, idx))
}

pub(crate) fn entity_rotate(
    cx: &Cx<'_, '_>,
    w: &mut Writer<'_, '_>,
    it: &mut Intent,
    out: &mut Out,
    a: &[&str],
) -> Result<(), CadError> {
    let sp = &ENTITY_ROTATE;
    let args = sp.parse(a)?;
    let by = args
        .get("--by")
        .ok_or_else(|| args::usage(sp, "entity rotate needs --by DEG"))?
        .trim()
        .to_owned();
    let deg = num::parse(&by).ok_or_else(|| args::usage(sp, "--by takes degrees, a number"))?;
    let c = about(
        sp,
        args.get("--about")
            .ok_or_else(|| args::usage(sp, "entity rotate needs --about x,y"))?,
    )?;
    cx.unit(args.get("--unit"), sp.verb)?;
    let (addrs, idx) = targets(cx, sp, &args.words)?;
    // a multiple of 90 degrees (as a decimal: 90, 90.0, -270) is a quarter turn, exact
    let quarter = num::turn(&by)
        .and_then(|t| num::parse(&t))
        .filter(|t| {
            t.fract() == 0.0 && (*t as i64) % 90 == 0 && !mutant::active("QuarterTurnInFloat")
        })
        .map(|t| ((t as i64) / 90) as u8);
    let (sin, cos) = deg.to_radians().sin_cos();
    let xf = Xf::Rotate {
        deg: by.clone(),
        quarter,
        cos,
        sin,
        c: c.clone(),
        eps: noise_eps(cx.doc),
    };
    if quarter != Some(0) {
        for &i in &idx {
            entity(cx, w, it, i, &xf)?;
        }
    }
    for &i in &idx {
        it.changed.insert(cx.doc.entities[i].handle.unwrap_or(0));
    }
    out.word = "rotated";
    out.stdout = format!("rotated {} by {by} about {},{}", list(&addrs), c[0], c[1]);
    out.viewing = addrs.first().cloned();
    Ok(())
}

pub(crate) fn entity_scale(
    cx: &Cx<'_, '_>,
    w: &mut Writer<'_, '_>,
    it: &mut Intent,
    out: &mut Out,
    a: &[&str],
) -> Result<(), CadError> {
    let sp = &ENTITY_SCALE;
    let args = sp.parse(a)?;
    let f = args
        .get("--factor")
        .ok_or_else(|| args::usage(sp, "entity scale needs --factor F"))?
        .trim()
        .to_owned();
    if !num::parse(&f).is_some_and(|x| x > 0.0) {
        return Err(args::usage(sp, "--factor takes a number above 0"));
    }
    let c = about(
        sp,
        args.get("--about")
            .ok_or_else(|| args::usage(sp, "entity scale needs --about x,y"))?,
    )?;
    cx.unit(args.get("--unit"), sp.verb)?;
    let (addrs, idx) = targets(cx, sp, &args.words)?;
    let xf = Xf::Scale {
        f: f.clone(),
        c: c.clone(),
    };
    if num::parse(&f) != Some(1.0) {
        for &i in &idx {
            entity(cx, w, it, i, &xf)?;
        }
    }
    for &i in &idx {
        it.changed.insert(cx.doc.entities[i].handle.unwrap_or(0));
    }
    out.word = "scaled";
    out.stdout = format!("scaled {} by {f} about {},{}", list(&addrs), c[0], c[1]);
    out.viewing = addrs.first().cloned();
    Ok(())
}

/// The vector `(x, y)` turned by `deg` degrees: exact for quarter turns, else in `f64`.
pub(super) fn turn_vector(x: &str, y: &str, deg: &str, eps: f64) -> Option<(String, String)> {
    let d = num::parse(deg)?;
    let quarter = num::turn(deg)
        .and_then(|t| num::parse(&t))
        .filter(|t| t.fract() == 0.0 && (*t as i64) % 90 == 0)
        .map(|t| ((t as i64) / 90) as u8);
    let (sin, cos) = d.to_radians().sin_cos();
    Xf::Rotate {
        deg: deg.to_owned(),
        quarter,
        cos,
        sin,
        c: ["0".into(), "0".into()],
        eps,
    }
    .vector(x, y)
}
