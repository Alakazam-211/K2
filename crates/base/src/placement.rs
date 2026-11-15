//! A unit's placement in 3D (`docs/design/view-3d-slot-r1.1.md`): where a unit or a container
//! sits, in the file's own form, relative to another element or to the doc's space.
//!
//! - **The types** ([`Placement`], [`Step`], [`Rotation`], [`Space`]): the numbers a pack's file
//!   stores, never computed by the kit. Two placements are equal when every number is, bit for
//!   bit.
//! - **The slot** ([`SpaceSlot`], in [`crate::view_index::ViewSchema::space`]): the fields a
//!   placement is read from, the doc's space, the numbers' precision.
//! - **On the wire** ([`Placement::to_value`], [`Placement::from_value`]): an `F32` slot's
//!   numbers are printed as the shortest decimal that reads back as the same `f32`. A step, a
//!   rotation or a word a reader doesn't know makes the placement unknown (`None`), never a
//!   skipped step.
//! - **The matrices** ([`Placement::matrix`], [`world_matrix`]): column-major 4x4, by the
//!   formulas of the design, with a fixed pure-Rust sine and cosine, so the same placement
//!   gives the same bits on every host. For drawing only: the kit never compares matrices.

use std::collections::HashSet;

use crate::errors::Value as Json;

/// The most steps a placement from a pack may have; a placement with more is dropped. The
/// client and the helpers take any number.
pub const PLACEMENT_STEPS_MAX: usize = 4;

/// A 4x4 matrix, column-major: `m[col * 4 + row]`, the translation in `m[12..15]`.
pub type Mat4 = [f64; 16];

/// How a slot's numbers are held: `F32` when the file stores `f32` (each number is exactly an
/// `f32`, printed as its shortest `f32` decimal), `F64` otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Precision {
    F32,
    #[default]
    F64,
}

impl Precision {
    pub fn as_str(self) -> &'static str {
        match self {
            Precision::F32 => "f32",
            Precision::F64 => "f64",
        }
    }
}

/// An Euler order word: the point is turned about the first axis, then the second, then the
/// third, each about the parent frame's fixed axes (`XYZ` is `Rz · Ry · Rx`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EulerOrder {
    Xyz,
    Xzy,
    Yxz,
    Yzx,
    Zxy,
    Zyx,
}

impl EulerOrder {
    pub const ALL: [EulerOrder; 6] = [
        EulerOrder::Xyz,
        EulerOrder::Xzy,
        EulerOrder::Yxz,
        EulerOrder::Yzx,
        EulerOrder::Zxy,
        EulerOrder::Zyx,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            EulerOrder::Xyz => "XYZ",
            EulerOrder::Xzy => "XZY",
            EulerOrder::Yxz => "YXZ",
            EulerOrder::Yzx => "YZX",
            EulerOrder::Zxy => "ZXY",
            EulerOrder::Zyx => "ZYX",
        }
    }

    pub fn parse(word: &str) -> Option<EulerOrder> {
        EulerOrder::ALL.into_iter().find(|o| o.as_str() == word)
    }

    /// The axes in the order the point is turned about them (0 = X, 1 = Y, 2 = Z).
    fn axes(self) -> [usize; 3] {
        match self {
            EulerOrder::Xyz => [0, 1, 2],
            EulerOrder::Xzy => [0, 2, 1],
            EulerOrder::Yxz => [1, 0, 2],
            EulerOrder::Yzx => [1, 2, 0],
            EulerOrder::Zxy => [2, 0, 1],
            EulerOrder::Zyx => [2, 1, 0],
        }
    }
}

/// A rotation, in radians.
#[derive(Debug, Clone)]
pub enum Rotation {
    /// The angles about X, Y and Z, in that slot order whatever the order word.
    Euler {
        order: EulerOrder,
        angles: [f64; 3],
    },
    /// `w, x, y, z`, as stored (normalised by the helpers, not by the kit).
    Quat([f64; 4]),
    AxisAngle {
        axis: [f64; 3],
        angle: f64,
    },
}

/// One step of a placement.
#[derive(Debug, Clone)]
pub enum Step {
    /// Scale, then rotate, then translate: `T(loc) · R · S(scale)`.
    Trs {
        loc: [f64; 3],
        rot: Rotation,
        scale: [f64; 3],
    },
    /// An origin and two directions, built as the IFC spec builds its axes; `None`: the
    /// default direction.
    Axes {
        origin: [f64; 3],
        z: Option<[f64; 3]>,
        x: Option<[f64; 3]>,
    },
    /// A 4x4 affine matrix, column-major.
    Matrix(Mat4),
}

/// Where a unit (or a container) sits. Equal when every number is, bit for bit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Placement {
    /// The address of the element it is relative to, a unit or a container; `None`: the doc's
    /// space (a unit's container plays no part unless this names it).
    pub relative_to: Option<String>,
    /// The first step is outermost, nearest the parent.
    pub steps: Vec<Step>,
    /// A reason word when the file can't say exactly where the unit is (`driven`).
    pub approx: Option<String>,
    /// How its numbers are held: set by the kit from the slot.
    pub precision: Precision,
}

/// Which axis is up in the doc's space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Y,
    Z,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handed {
    Right,
    Left,
}

/// The doc's space: where `relative_to: None` puts a placement.
#[derive(Debug, Clone)]
pub struct Space {
    pub length_unit: String,
    /// How many metres one length unit is; `None` when the file doesn't say, or says several.
    pub metres_per_unit: Option<f64>,
    pub up: Axis,
    pub handed: Handed,
}

/// An `f64` compared bit for bit, so a slot's constant can be `Eq`.
#[derive(Debug, Clone, Copy)]
pub struct ExactF64(pub f64);

impl PartialEq for ExactF64 {
    fn eq(&self, o: &Self) -> bool {
        self.0.to_bits() == o.0.to_bits()
    }
}
impl Eq for ExactF64 {}

/// Where the doc's space comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpaceSrc {
    Fixed {
        length_unit: &'static str,
        metres_per_unit: Option<ExactF64>,
        up: Axis,
        handed: Handed,
    },
    /// The pack's [`crate::session::Pack::view_space`], from the map roots named here: a
    /// change to any of them asks again.
    Pack { roots: &'static [&'static str] },
}

/// The view schema's 3D slot ([`crate::view_index::ViewSchema::space`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpaceSlot {
    /// The unit fields a placement is read from: a change to one asks the pack again.
    pub unit_fields: &'static [&'static str],
    /// The same for containers; `&[]`: containers have no placement.
    pub container_fields: &'static [&'static str],
    pub space: SpaceSrc,
    pub precision: Precision,
}

impl SpaceSlot {
    /// As the snapshot's schema carries it (`schema.space`).
    pub fn to_value(&self) -> Json {
        Json::obj([
            ("unit_fields", Json::list(self.unit_fields.iter().copied())),
            (
                "container_fields",
                Json::list(self.container_fields.iter().copied()),
            ),
            ("precision", Json::from(self.precision.as_str())),
        ])
    }

    /// The fixed space, when the slot gives one.
    pub fn fixed(&self) -> Option<Space> {
        match self.space {
            SpaceSrc::Fixed {
                length_unit,
                metres_per_unit,
                up,
                handed,
            } => Some(Space {
                length_unit: length_unit.to_owned(),
                metres_per_unit: metres_per_unit.map(|m| m.0),
                up,
                handed,
            }),
            SpaceSrc::Pack { .. } => None,
        }
    }
}

fn same(a: &[f64], b: &[f64]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits())
}

fn same_opt(a: &Option<[f64; 3]>, b: &Option<[f64; 3]>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => same(a, b),
        _ => false,
    }
}

impl PartialEq for Rotation {
    fn eq(&self, o: &Self) -> bool {
        match (self, o) {
            (
                Rotation::Euler { order, angles },
                Rotation::Euler {
                    order: o2,
                    angles: a2,
                },
            ) => order == o2 && same(angles, a2),
            (Rotation::Quat(a), Rotation::Quat(b)) => same(a, b),
            (
                Rotation::AxisAngle { axis, angle },
                Rotation::AxisAngle {
                    axis: x2,
                    angle: t2,
                },
            ) => same(axis, x2) && angle.to_bits() == t2.to_bits(),
            _ => false,
        }
    }
}
impl Eq for Rotation {}

impl PartialEq for Step {
    fn eq(&self, o: &Self) -> bool {
        match (self, o) {
            (
                Step::Trs { loc, rot, scale },
                Step::Trs {
                    loc: l2,
                    rot: r2,
                    scale: s2,
                },
            ) => same(loc, l2) && rot == r2 && same(scale, s2),
            (
                Step::Axes { origin, z, x },
                Step::Axes {
                    origin: o2,
                    z: z2,
                    x: x2,
                },
            ) => same(origin, o2) && same_opt(z, z2) && same_opt(x, x2),
            (Step::Matrix(a), Step::Matrix(b)) => same(a, b),
            _ => false,
        }
    }
}
impl Eq for Step {}

impl PartialEq for Space {
    fn eq(&self, o: &Self) -> bool {
        self.length_unit == o.length_unit
            && self.metres_per_unit.map(f64::to_bits) == o.metres_per_unit.map(f64::to_bits)
            && self.up == o.up
            && self.handed == o.handed
    }
}
impl Eq for Space {}

// ----- the numbers -----

impl Rotation {
    fn numbers(&self, out: &mut Vec<f64>) {
        match self {
            Rotation::Euler { angles, .. } => out.extend(angles),
            Rotation::Quat(q) => out.extend(q),
            Rotation::AxisAngle { axis, angle } => {
                out.extend(axis);
                out.push(*angle);
            }
        }
    }
}

impl Step {
    fn numbers(&self, out: &mut Vec<f64>) {
        match self {
            Step::Trs { loc, rot, scale } => {
                out.extend(loc);
                rot.numbers(out);
                out.extend(scale);
            }
            Step::Axes { origin, z, x } => {
                out.extend(origin);
                out.extend(z.iter().flatten());
                out.extend(x.iter().flatten());
            }
            Step::Matrix(m) => out.extend(m),
        }
    }
}

impl Placement {
    /// Every number it holds, in order.
    pub fn numbers(&self) -> Vec<f64> {
        let mut out = Vec::new();
        for s in &self.steps {
            s.numbers(&mut out);
        }
        out
    }

    /// Why a slot of precision `precision` can't carry it: no step, more than
    /// [`PLACEMENT_STEPS_MAX`], a number that isn't finite, or (for `F32`) a number that isn't
    /// an `f32`. `None`: it can.
    pub fn problem(&self, precision: Precision) -> Option<String> {
        if self.steps.is_empty() {
            return Some("a placement with no step".into());
        }
        if self.steps.len() > PLACEMENT_STEPS_MAX {
            return Some(format!(
                "{} steps, at most {PLACEMENT_STEPS_MAX}",
                self.steps.len()
            ));
        }
        for n in self.numbers() {
            if !n.is_finite() {
                return Some(format!("the number {n} isn't finite"));
            }
            if precision == Precision::F32 && (n as f32 as f64).to_bits() != n.to_bits() {
                return Some(format!("the number {n} in an f32 slot isn't an f32"));
            }
        }
        None
    }
}

// ----- the wire -----

/// A number as the wire carries it at `p`: for `F32`, the shortest decimal that reads back as
/// the same `f32`.
fn num(x: f64, p: Precision) -> Json {
    let v = match p {
        Precision::F32 if !planted_f32_as_f64_digits() => {
            (x as f32).to_string().parse::<f64>().unwrap_or(x)
        }
        _ => x,
    };
    Json::float(v).unwrap_or(Json::Null)
}

fn nums(xs: &[f64], p: Precision) -> Json {
    Json::List(xs.iter().map(|x| num(*x, p)).collect())
}

impl Rotation {
    fn to_value(&self, p: Precision) -> Json {
        match self {
            Rotation::Euler { order, angles } => Json::obj([
                ("euler", Json::from(order.as_str())),
                ("angles", nums(angles, p)),
            ]),
            Rotation::Quat(q) => Json::obj([("quat", nums(q, p))]),
            Rotation::AxisAngle { axis, angle } => {
                Json::obj([("axis", nums(axis, p)), ("angle", num(*angle, p))])
            }
        }
    }
}

impl Step {
    fn to_value(&self, p: Precision) -> Json {
        match self {
            Step::Trs { loc, rot, scale } => Json::obj([(
                "trs",
                Json::obj([
                    ("loc", nums(loc, p)),
                    ("rot", rot.to_value(p)),
                    ("scale", nums(scale, p)),
                ]),
            )]),
            Step::Axes { origin, z, x } => {
                let dir = |d: &Option<[f64; 3]>| d.as_ref().map_or(Json::Null, |d| nums(d, p));
                Json::obj([(
                    "axes",
                    Json::obj([("origin", nums(origin, p)), ("z", dir(z)), ("x", dir(x))]),
                )])
            }
            Step::Matrix(m) => Json::obj([("matrix", nums(m, p))]),
        }
    }
}

fn list(v: &Json) -> Option<&[Json]> {
    match v {
        Json::List(l) => Some(l),
        _ => None,
    }
}

fn fixed<const N: usize>(v: Option<&Json>) -> Option<[f64; N]> {
    let l = list(v?)?;
    if l.len() != N {
        return None;
    }
    let mut out = [0.0; N];
    for (o, x) in out.iter_mut().zip(l) {
        *o = x.as_f64()?;
    }
    Some(out)
}

/// The only key of an object, and its value.
fn only(v: &Json) -> Option<(&str, &Json)> {
    match v {
        Json::Obj(m) if m.len() == 1 => Some((m[0].0.as_str(), &m[0].1)),
        _ => None,
    }
}

impl Rotation {
    fn from_value(v: &Json) -> Option<Rotation> {
        let Json::Obj(m) = v else { return None };
        let has = |k: &str| m.iter().any(|(x, _)| x == k);
        if has("euler") && m.len() == 2 {
            let order = EulerOrder::parse(v.get("euler")?.as_str()?)?;
            return Some(Rotation::Euler {
                order,
                angles: fixed(v.get("angles"))?,
            });
        }
        if has("quat") && m.len() == 1 {
            return Some(Rotation::Quat(fixed(v.get("quat"))?));
        }
        if has("axis") && m.len() == 2 {
            return Some(Rotation::AxisAngle {
                axis: fixed(v.get("axis"))?,
                angle: v.get("angle")?.as_f64()?,
            });
        }
        None
    }
}

impl Step {
    fn from_value(v: &Json) -> Option<Step> {
        let (kind, body) = only(v)?;
        match kind {
            "trs" => Some(Step::Trs {
                loc: fixed(body.get("loc"))?,
                rot: Rotation::from_value(body.get("rot")?)?,
                scale: fixed(body.get("scale"))?,
            }),
            "axes" => {
                let dir = |k: &str| -> Option<Option<[f64; 3]>> {
                    match body.get(k) {
                        None | Some(Json::Null) => Some(None),
                        d => fixed(d).map(Some),
                    }
                };
                Some(Step::Axes {
                    origin: fixed(body.get("origin"))?,
                    z: dir("z")?,
                    x: dir("x")?,
                })
            }
            "matrix" => Some(Step::Matrix(fixed(Some(body))?)),
            _ => None,
        }
    }
}

impl Placement {
    /// As the wire carries it: `{relative_to, steps}`, and `approx` when there is one.
    pub fn to_value(&self) -> Json {
        let p = self.precision;
        let mut v = vec![
            (
                "relative_to".to_owned(),
                Json::from(self.relative_to.as_deref()),
            ),
            (
                "steps".to_owned(),
                Json::List(self.steps.iter().map(|s| s.to_value(p)).collect()),
            ),
        ];
        if let Some(a) = &self.approx {
            v.push(("approx".to_owned(), Json::from(a.as_str())));
        }
        Json::Obj(v)
    }

    /// A placement read from the wire at precision `precision`; `None` for `null`, and for
    /// anything a reader doesn't know (a step or rotation kind, an order word): an unknown
    /// placement, never one with a step skipped. No step limit.
    pub fn from_value(v: &Json, precision: Precision) -> Option<Placement> {
        let relative_to = match v.get("relative_to") {
            None | Some(Json::Null) => None,
            Some(r) => Some(r.as_str()?.to_owned()),
        };
        let steps = list(v.get("steps")?)?
            .iter()
            .map(Step::from_value)
            .collect::<Option<Vec<_>>>()?;
        if steps.is_empty() {
            return None;
        }
        let approx = match v.get("approx") {
            None | Some(Json::Null) => None,
            Some(a) => Some(a.as_str()?.to_owned()),
        };
        Some(Placement {
            relative_to,
            steps,
            approx,
            precision,
        })
    }
}

impl Space {
    /// As the snapshot's view carries it (`view.space`, a frame's `view_space`).
    pub fn to_value(&self) -> Json {
        Json::obj([
            ("length_unit", Json::from(self.length_unit.as_str())),
            (
                "metres_per_unit",
                self.metres_per_unit
                    .and_then(Json::float)
                    .unwrap_or(Json::Null),
            ),
            (
                "up",
                Json::from(match self.up {
                    Axis::Y => "y",
                    Axis::Z => "z",
                }),
            ),
            (
                "handed",
                Json::from(match self.handed {
                    Handed::Right => "right",
                    Handed::Left => "left",
                }),
            ),
        ])
    }
}

// ----- a fixed sine and cosine -----

/// The constants of the fixed sine and cosine, written to every digit their tables give.
#[allow(clippy::excessive_precision, clippy::approx_constant)]
mod k {
    pub const INV_PIO2: f64 = 6.366_197_723_675_813_43e-1;
    pub const PIO2_1: f64 = 1.570_796_326_734_125_614_17e+00;
    pub const PIO2_2: f64 = 6.077_100_506_303_965_976_60e-11;
    pub const PIO2_3: f64 = 2.022_266_248_711_166_455_80e-21;
    pub const S1: f64 = -1.666_666_666_666_663_243_48e-01;
    pub const S2: f64 = 8.333_333_333_322_489_461_24e-03;
    pub const S3: f64 = -1.984_126_982_985_794_931_34e-04;
    pub const S4: f64 = 2.755_731_370_707_006_767_89e-06;
    pub const S5: f64 = -2.505_076_025_340_686_341_95e-08;
    pub const S6: f64 = 1.589_690_995_211_550_102_21e-10;
    pub const C1: f64 = 4.166_666_666_666_660_190_37e-02;
    pub const C2: f64 = -1.388_888_888_887_410_957_49e-03;
    pub const C3: f64 = 2.480_158_728_947_672_941_78e-05;
    pub const C4: f64 = -2.755_731_435_139_066_330_35e-07;
    pub const C5: f64 = 2.087_572_321_298_174_827_90e-09;
    pub const C6: f64 = -1.135_964_755_778_819_482_65e-11;
}
use k::*;

/// `sin x` and `cos x` by one fixed algorithm of IEEE additions, multiplications and an exact
/// rounding (a three-part reduction by pi/2, then the classic minimax polynomials on
/// [-pi/4, pi/4]): the same bits on every host, never the platform's library. Accurate to
/// about one ulp for |x| below 2^20 · pi/2; past that, still the same bits everywhere.
pub fn sin_cos(x: f64) -> (f64, f64) {
    if !x.is_finite() {
        return (f64::NAN, f64::NAN);
    }
    let n = (x * INV_PIO2).round();
    let r = ((x - n * PIO2_1) - n * PIO2_2) - n * PIO2_3;
    let z = r * r;
    let s = r + r * z * (S1 + z * (S2 + z * (S3 + z * (S4 + z * (S5 + z * S6)))));
    let hz = 0.5 * z;
    let w = 1.0 - hz;
    let c =
        w + (((1.0 - w) - hz) + z * z * (C1 + z * (C2 + z * (C3 + z * (C4 + z * (C5 + z * C6))))));
    match (n as i64).rem_euclid(4) {
        0 => (s, c),
        1 => (c, -s),
        2 => (-s, -c),
        _ => (-c, s),
    }
}

// ----- the matrices -----

type M3 = [[f64; 3]; 3];

fn identity() -> Mat4 {
    let mut m = [0.0; 16];
    m[0] = 1.0;
    m[5] = 1.0;
    m[10] = 1.0;
    m[15] = 1.0;
    m
}

/// `a · b`, column-major.
pub fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [0.0; 16];
    for c in 0..4 {
        for r in 0..4 {
            let mut s = 0.0;
            for k in 0..4 {
                s += a[k * 4 + r] * b[c * 4 + k];
            }
            out[c * 4 + r] = s;
        }
    }
    out
}

fn m3_mul(a: &M3, b: &M3) -> M3 {
    let mut out = [[0.0; 3]; 3];
    for (r, row) in out.iter_mut().enumerate() {
        for (c, x) in row.iter_mut().enumerate() {
            *x = a[r][0] * b[0][c] + a[r][1] * b[1][c] + a[r][2] * b[2][c];
        }
    }
    out
}

/// The basic rotation about axis `axis` (0 = X) by `a`.
fn basic(axis: usize, a: f64) -> M3 {
    let (s, c) = sin_cos(a);
    match axis {
        0 => [[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]],
        1 => [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]],
        _ => [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]],
    }
}

fn norm(v: &[f64; 3]) -> f64 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

fn unit(v: &[f64; 3]) -> Option<[f64; 3]> {
    let n = norm(v);
    (n > 0.0 && n.is_finite()).then(|| [v[0] / n, v[1] / n, v[2] / n])
}

fn r(x: f64, p: Precision) -> f64 {
    match p {
        Precision::F32 => x as f32 as f64,
        Precision::F64 => x,
    }
}

impl Rotation {
    fn matrix(&self, p: Precision) -> Option<M3> {
        match self {
            Rotation::Euler { order, angles } => {
                let [a, b, c] = order.axes();
                let ra = basic(a, r(angles[a], p));
                let rb = basic(b, r(angles[b], p));
                let rc = basic(c, r(angles[c], p));
                Some(m3_mul(&m3_mul(&rc, &rb), &ra))
            }
            Rotation::Quat(q) => {
                let q = q.map(|x| r(x, p));
                let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
                if n <= 0.0 || !n.is_finite() {
                    return None;
                }
                let (w, x, y, z) = (q[0] / n, q[1] / n, q[2] / n, q[3] / n);
                Some([
                    [
                        1.0 - 2.0 * (y * y + z * z),
                        2.0 * (x * y - w * z),
                        2.0 * (x * z + w * y),
                    ],
                    [
                        2.0 * (x * y + w * z),
                        1.0 - 2.0 * (x * x + z * z),
                        2.0 * (y * z - w * x),
                    ],
                    [
                        2.0 * (x * z - w * y),
                        2.0 * (y * z + w * x),
                        1.0 - 2.0 * (x * x + y * y),
                    ],
                ])
            }
            Rotation::AxisAngle { axis, angle } => {
                let [x, y, z] = unit(&axis.map(|v| r(v, p)))?;
                let (s, c) = sin_cos(r(*angle, p));
                let k = 1.0 - c;
                Some([
                    [c + x * x * k, x * y * k - z * s, x * z * k + y * s],
                    [y * x * k + z * s, c + y * y * k, y * z * k - x * s],
                    [z * x * k - y * s, z * y * k + x * s, c + z * z * k],
                ])
            }
        }
    }
}

/// A 3x3 (rows) and a translation as a column-major 4x4.
fn affine(m: &M3, t: [f64; 3]) -> Mat4 {
    let mut out = identity();
    for (row, vals) in m.iter().enumerate() {
        for (col, v) in vals.iter().enumerate() {
            out[col * 4 + row] = *v;
        }
    }
    out[12] = t[0];
    out[13] = t[1];
    out[14] = t[2];
    out
}

impl Step {
    /// Its matrix at precision `p`, or `None` for a degenerate step (the design's section 1.2).
    pub fn matrix(&self, p: Precision) -> Option<Mat4> {
        match self {
            Step::Trs { loc, rot, scale } => {
                let rm = rot.matrix(p)?;
                let s = scale.map(|v| r(v, p));
                let mut m = rm;
                for row in m.iter_mut() {
                    for (c, v) in row.iter_mut().enumerate() {
                        *v *= s[c];
                    }
                }
                Some(affine(&m, loc.map(|v| r(v, p))))
            }
            Step::Axes { origin, z, x } => {
                let zv = match z {
                    Some(d) => unit(&d.map(|v| r(v, p)))?,
                    None => [0.0, 0.0, 1.0],
                };
                let v = match x {
                    Some(d) => {
                        let d = d.map(|v| r(v, p));
                        norm(&d).gt(&0.0).then_some(d)?
                    }
                    None if zv == [1.0, 0.0, 0.0] => [0.0, 1.0, 0.0],
                    None => [1.0, 0.0, 0.0],
                };
                let dot = v[0] * zv[0] + v[1] * zv[1] + v[2] * zv[2];
                let proj = [v[0] - dot * zv[0], v[1] - dot * zv[1], v[2] - dot * zv[2]];
                let len = norm(&proj);
                if len <= 1e-12 * norm(&v) || len.is_nan() {
                    return None;
                }
                let xv = [proj[0] / len, proj[1] / len, proj[2] / len];
                let yv = [
                    zv[1] * xv[2] - zv[2] * xv[1],
                    zv[2] * xv[0] - zv[0] * xv[2],
                    zv[0] * xv[1] - zv[1] * xv[0],
                ];
                let m = [
                    [xv[0], yv[0], zv[0]],
                    [xv[1], yv[1], zv[1]],
                    [xv[2], yv[2], zv[2]],
                ];
                Some(affine(&m, origin.map(|v| r(v, p))))
            }
            Step::Matrix(m) => {
                let m = m.map(|v| r(v, p));
                (m[3] == 0.0 && m[7] == 0.0 && m[11] == 0.0 && m[15] == 1.0).then_some(m)
            }
        }
    }
}

impl Placement {
    /// The product of its steps' matrices (the first outermost), or `None` when a step is
    /// degenerate. Numbers of an `F32` placement are rounded to `f32` first.
    pub fn matrix(&self) -> Option<Mat4> {
        if self.steps.is_empty() {
            return None;
        }
        let mut m = identity();
        for s in &self.steps {
            m = mul(&m, &s.matrix(self.precision)?);
        }
        m.iter().all(|v| v.is_finite()).then_some(m)
    }
}

/// What a chain's lookup says of an address: shown and placed, shown with no placement, or not
/// shown at all.
#[derive(Debug, Clone, Copy)]
pub enum Link<'a> {
    Placed(&'a Placement),
    Unplaced,
    NotShown,
}

/// The planted fault [`crate::differential::PlaceMutant::F32AsF64Digits`].
fn planted_f32_as_f64_digits() -> bool {
    plant_on!(
        crate::differential::place_mutant()
            == Some(crate::differential::PlaceMutant::F32AsF64Digits)
    )
}

/// The planted fault [`crate::differential::PlaceMutant::MissingLinkIdentity`].
fn planted_missing_link_identity() -> bool {
    plant_on!(
        crate::differential::place_mutant()
            == Some(crate::differential::PlaceMutant::MissingLinkIdentity)
    )
}

/// The world matrix of `address`: `W(u) = W(relative_to(u)) · P(u)`, `P(u)` alone when
/// `relative_to` is `None` (the doc's space). `None` when the chain is broken: an element with
/// no placement or a degenerate one, an address not shown, or a cycle; never identity for a
/// missing link.
pub fn world_matrix<'a>(address: &str, lookup: impl Fn(&str) -> Link<'a>) -> Option<Mat4> {
    let mut chain: Vec<&Placement> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut at = address.to_owned();
    loop {
        if !seen.insert(at.clone()) {
            return None;
        }
        match lookup(&at) {
            Link::Placed(p) => {
                chain.push(p);
                match &p.relative_to {
                    None => break,
                    Some(next) => at = next.clone(),
                }
            }
            Link::Unplaced | Link::NotShown
                if planted_missing_link_identity() && !chain.is_empty() =>
            {
                break
            }
            Link::Unplaced | Link::NotShown => return None,
        }
    }
    let mut w = identity();
    for p in chain.iter().rev() {
        w = mul(&w, &p.matrix()?);
    }
    Some(w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fixed_sine_is_close_to_the_platform_s() {
        let mut x = -20.0;
        while x < 20.0 {
            let (s, c) = sin_cos(x);
            assert!((s - x.sin()).abs() < 1e-15, "sin {x}: {s} vs {}", x.sin());
            assert!((c - x.cos()).abs() < 1e-15, "cos {x}: {c} vs {}", x.cos());
            x += 0.0137;
        }
        assert_eq!(sin_cos(0.0), (0.0, 1.0));
    }

    #[test]
    fn f32_numbers_print_short_and_read_back() {
        let p = Placement {
            steps: vec![Step::Matrix({
                let mut m = identity();
                m[12] = 0.1f32 as f64;
                m
            })],
            precision: Precision::F32,
            ..Placement::default()
        };
        let j = p.to_value().to_json();
        assert!(j.contains("0.1,"), "{j}");
        let back = Placement::from_value(&p.to_value(), Precision::F32).unwrap();
        let Step::Matrix(m) = &back.steps[0] else {
            panic!("a matrix")
        };
        assert_eq!(m[12] as f32, 0.1f32);
        assert!(p.problem(Precision::F32).is_none());
        let bad = Placement {
            steps: vec![Step::Matrix({
                let mut m = identity();
                m[12] = 0.1;
                m
            })],
            ..Placement::default()
        };
        assert!(bad.problem(Precision::F32).is_some());
        assert!(bad.problem(Precision::F64).is_none());
    }
}
