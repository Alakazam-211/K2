//! Views: auto framing, the turntable angle set, and given cameras.
//!
//! Auto and turntable views use only `+ - * / sqrt` (the angles are 45-degree azimuth steps at
//! 30 degrees of elevation, whose sines and cosines are 0, 1/2, sqrt(1/2), sqrt(3/4), 1), and a
//! fixed vertical field of view with tan(fov/2) = 1/2 (about 53.13 degrees). A given camera's
//! yfov goes through [`crate::trig::tan`] (only `+ - * /`). So every view's matrices are
//! bit-identical on every IEEE machine.
//!
//! **Auto (the 3/4 view):** from the front-right and above, by the scene's up axis (front is +Z
//! for `"y"`, -Y for `"z"`), fitted to the bounding box of what is drawn. **Turntable:** frame k
//! of 8 at azimuth 45 + 45k degrees and 30 degrees of elevation, fitted to the bounding sphere so
//! every frame has the same scale (frame 0 looks the way the auto view does).
//!
//! **Content framing** ([`Framing::Contents`], asked per auto view): the same 3/4 direction,
//! fitted to the contents' box ([`crate::contents`]) instead of every object's, with everything
//! still drawn. The eye moves along the view direction to the contents' centre, and a lens shift
//! (an off-axis projection, [`Frame::shift_x`] and [`Frame::shift_y`]) centres the contents'
//! projected box exactly, rather than turning the camera off its 3/4 direction. Every other view
//! has no shift.

use serde::{Deserialize, Serialize};

use crate::contents::{self, Classes};
use crate::math::*;
use crate::scene::{Camera, Scene, UpAxis};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewSpec {
    /// The fixed 3/4 view (turntable frame 0's direction), fitted tightly to the bounding box.
    Auto,
    /// `cameras[i]`.
    Camera(u32),
    /// Frame k of [`TURNTABLE_FRAMES`]: azimuth 45 + 45k degrees, fitted to the bounding
    /// sphere so every frame has the same scale.
    Turntable(u8),
}

pub const TURNTABLE_FRAMES: u8 = 8;
/// tan(vertical fov / 2) of the auto and turntable views.
pub const AUTO_TAN_HALF_FOV: f64 = 0.5;
/// Margin around the bounding sphere.
pub const AUTO_MARGIN: f64 = 1.05;
/// Halvings of the content fit's distance search: from a bracket of any size down to the f64
/// resolution of the distance (each step halves it; 64 is past the 52-bit mantissa).
pub const FIT_STEPS: u32 = 64;

/// How an auto view is fitted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Framing {
    /// Every drawn object's box (the default; byte for byte what the auto view always drew).
    #[default]
    Bounds,
    /// The contents' box only, shell still drawn, centred with a lens shift
    /// ([`frame_contents`]); falls back to [`Framing::Bounds`] when nothing is contents.
    Contents,
}

impl Framing {
    pub fn is_bounds(&self) -> bool {
        *self == Framing::Bounds
    }
}

/// World-to-view (camera looks down -Z, +Y up), and the projection scales.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub view: M4,
    /// x_clip = fx * x_view, y_clip = fy * y_view, w = -z_view.
    pub fx: f32,
    pub fy: f32,
    /// Near plane distance (> 0).
    pub near: f32,
    /// Lens shift in NDC units: `x_ndc = fx * x_view / w + shift_x` (same for y). 0 except for a
    /// content-framed view.
    pub shift_x: f32,
    pub shift_y: f32,
}

impl Frame {
    /// World-to-view, column-major, as f32: the f64 matrix the rasterizer composes with each
    /// object's transform (in f64, then rounded to f32 once per object), itself rounded once.
    pub fn view_f32(&self) -> [f32; 16] {
        m4_to_f32(&self.view)
    }

    /// The projection, column-major, OpenGL's clip convention with an infinite far plane:
    /// `x_clip = fx * x_view`, `y_clip = fy * y_view`, `z_clip = -z_view - 2 near`,
    /// `w_clip = -z_view`; so NDC z is -1 on the near plane and tends to 1 far away. The
    /// rasterizer draws exactly `x_clip / w_clip` and `y_clip / w_clip` with these f32 values
    /// (it tests depth as 1/w, never NDC z). The lens shift is `[8]` and `[9]`, negated (they
    /// multiply `z_view = -w`): `x_clip = fx * x_view + shift_x * w_clip`; both are +0 unshifted.
    pub fn projection(&self) -> [f32; 16] {
        [
            self.fx, 0.0, 0.0, 0.0, //
            0.0, self.fy, 0.0, 0.0, //
            0.0 - self.shift_x, 0.0 - self.shift_y, -1.0, -1.0, //
            0.0, 0.0, -2.0 * self.near, 0.0,
        ]
    }
}

pub fn mesh_bounds(positions: &[[f32; 3]]) -> Option<(D3, D3)> {
    let mut it = positions.iter();
    let first = d3(*it.next()?);
    let (mut lo, mut hi) = (first, first);
    for p in it {
        for k in 0..3 {
            let v = p[k] as f64;
            if v < lo[k] {
                lo[k] = v;
            }
            if v > hi[k] {
                hi[k] = v;
            }
        }
    }
    Some((lo, hi))
}

/// World bounds of the active objects (each mesh's local box, its 8 corners transformed).
pub fn scene_bounds(scene: &Scene, active: &[u32]) -> Option<(D3, D3)> {
    let mut cache: Vec<Option<Option<(D3, D3)>>> = vec![None; scene.meshes.len()];
    let mut acc: Option<(D3, D3)> = None;
    for &oi in active {
        let o = &scene.objects[oi as usize];
        let mi = o.mesh as usize;
        if cache[mi].is_none() {
            cache[mi] = Some(mesh_bounds(&scene.meshes[mi].positions));
        }
        let Some((lo, hi)) = cache[mi].unwrap() else { continue };
        let m = m4_from_f32(&o.transform);
        for c in 0..8 {
            let p = [
                if c & 1 == 0 { lo[0] } else { hi[0] },
                if c & 2 == 0 { lo[1] } else { hi[1] },
                if c & 4 == 0 { lo[2] } else { hi[2] },
            ];
            let w = m4_point(&m, p);
            acc = Some(match acc {
                None => (w, w),
                Some((mut a, mut b)) => {
                    for k in 0..3 {
                        if w[k] < a[k] {
                            a[k] = w[k];
                        }
                        if w[k] > b[k] {
                            b[k] = w[k];
                        }
                    }
                    (a, b)
                }
            });
        }
    }
    acc
}

/// One object's world box (its mesh's local box, 8 corners through its transform), the mesh
/// boxes cached in `cache` (one slot per mesh). `None` for an empty mesh.
pub fn object_box(scene: &Scene, oi: u32, cache: &mut [Option<Option<(D3, D3)>>]) -> Option<(D3, D3)> {
    let o = &scene.objects[oi as usize];
    let mi = o.mesh as usize;
    if cache[mi].is_none() {
        cache[mi] = Some(mesh_bounds(&scene.meshes[mi].positions));
    }
    let (lo, hi) = cache[mi].unwrap()?;
    let m = m4_from_f32(&o.transform);
    let mut acc: Option<(D3, D3)> = None;
    for c in 0..8 {
        let p = [
            if c & 1 == 0 { lo[0] } else { hi[0] },
            if c & 2 == 0 { lo[1] } else { hi[1] },
            if c & 4 == 0 { lo[2] } else { hi[2] },
        ];
        let w = m4_point(&m, p);
        acc = Some(match acc {
            None => (w, w),
            Some((mut a, mut b)) => {
                for k in 0..3 {
                    if w[k] < a[k] {
                        a[k] = w[k];
                    }
                    if w[k] > b[k] {
                        b[k] = w[k];
                    }
                }
                (a, b)
            }
        });
    }
    acc
}

/// Centre and radius of the active objects' bounding sphere (the box's circumsphere).
pub fn bounding_sphere(scene: &Scene, active: &[u32]) -> (D3, f64) {
    match scene_bounds(scene, active) {
        Some((lo, hi)) => {
            let c = scaled(addd(lo, hi), 0.5);
            let r = 0.5 * lend(subd(hi, lo));
            (c, if r > 0.0 && r.is_finite() { r } else { 1.0 })
        }
        None => ([0.0; 3], 1.0),
    }
}

/// The view for `spec`. `aspect` = image width / height.
pub fn frame(scene: &Scene, active: &[u32], spec: ViewSpec, aspect: f64) -> Result<Frame, String> {
    let (c, r) = bounding_sphere(scene, active);
    match spec {
        ViewSpec::Auto => match scene_bounds(scene, active) {
            Some(bb) => orbit_box(scene.up, bb, r, aspect),
            None => orbit(scene.up, c, r, 0, aspect),
        },
        ViewSpec::Turntable(k) if k < TURNTABLE_FRAMES => orbit(scene.up, c, r, k, aspect),
        ViewSpec::Turntable(k) => Err(format!("turntable frame {k}: there are {TURNTABLE_FRAMES}")),
        ViewSpec::Camera(i) => {
            let cam = scene.cameras.get(i as usize).ok_or_else(|| format!("camera {i} doesn't exist"))?;
            given(cam, r, aspect)
        }
    }
}

/// What a content-framed view framed.
#[derive(Clone, Debug, PartialEq)]
pub struct ContentFraming {
    /// The contents were framed; `false`: it fell back to the bounds framing (no object is
    /// contents: none at all, or all shell).
    pub contents: bool,
    /// The shell objects, drawn but left out of the frame (indices into `scene.objects`,
    /// ascending); empty on a fallback.
    pub left_out: Vec<u32>,
    /// The contents were smaller than the least frame ([`contents::MIN_FRAME_FRACTION`]).
    pub grown: bool,
}

/// The auto view fitted to the contents of `active` ([`contents::content_frame`] of `classes`,
/// which [`contents::classify`] made for the same `active`): the 3/4 direction of
/// [`ViewSpec::Auto`], the eye on the line through the contents' box centre, as near as lets the
/// box's 8 corners project inside the margin once centred, and the lens shift that centres them.
/// With nothing to frame, exactly [`frame`]'s auto view (the same bytes as bounds framing).
pub fn frame_contents(scene: &Scene, active: &[u32], classes: &Classes, aspect: f64) -> Result<(Frame, ContentFraming), String> {
    let fallback = || -> Result<(Frame, ContentFraming), String> {
        Ok((frame(scene, active, ViewSpec::Auto, aspect)?, ContentFraming { contents: false, left_out: Vec::new(), grown: false }))
    };
    let Some(cf) = contents::content_frame(active, classes) else { return fallback() };
    match orbit_box_shifted(scene.up, (cf.lo, cf.hi), aspect) {
        Some(f) => Ok((f, ContentFraming { contents: true, left_out: cf.left_out, grown: cf.grown })),
        None => fallback(),
    }
}

/// The content fit: `orbit_box`'s direction and margin, with the distance searched so that the
/// box's projection, centred by the lens shift, just fits. `None` when the box is degenerate or
/// a value isn't finite.
///
/// The box is symmetric about its centre and the eye is on the view axis through that centre, so
/// every corner `q` has its opposite `-q`: the projected x extent spans 0, and as the eye backs
/// off each positive projection shrinks and each negative one grows towards 0. The centred
/// half-extent therefore falls as the distance grows, and halving a bracket finds the least
/// distance that fits. Only `+ - * /`, a fixed number of steps.
fn orbit_box_shifted(up: UpAxis, (lo, hi): (D3, D3), aspect: f64) -> Option<Frame> {
    let (dir, upv) = orbit_dir(up, 0);
    let c = scaled(addd(lo, hi), 0.5);
    let r = 0.5 * lend(subd(hi, lo));
    if !(r > 0.0 && r.is_finite() && aspect.is_finite() && aspect > 0.0) {
        return None;
    }
    let f = scaled(dir, -1.0);
    let rt = normalized(crossd(f, upv))?;
    let u = crossd(rt, f);
    let tan_v = AUTO_TAN_HALF_FOV;
    let tan_h = tan_v * aspect;
    let (mut xs, mut ys, mut zs) = ([0.0f64; 8], [0.0f64; 8], [0.0f64; 8]);
    let mut min_z = f64::MAX;
    let mut far = 0.0f64;
    for k in 0..8 {
        let p = [
            if k & 1 == 0 { lo[0] } else { hi[0] },
            if k & 2 == 0 { lo[1] } else { hi[1] },
            if k & 4 == 0 { lo[2] } else { hi[2] },
        ];
        let q = subd(p, c);
        (xs[k], ys[k], zs[k]) = (dotd(q, rt), dotd(q, u), dotd(q, f));
        min_z = min_z.min(zs[k]);
        // orbit_box's distance: every corner inside the margin uncentred, so centred too.
        far = far.max(xs[k].abs() * AUTO_MARGIN / tan_h - zs[k]).max(ys[k].abs() * AUTO_MARGIN / tan_v - zs[k]);
    }
    // NDC extents of the corners from `d` along the view direction: (half x, half y, mid x, mid y).
    let extents = |d: f64| -> (f64, f64, f64, f64) {
        let (mut x0, mut x1, mut y0, mut y1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
        for k in 0..8 {
            let w = d + zs[k];
            let (nx, ny) = (xs[k] / (tan_h * w), ys[k] / (tan_v * w));
            x0 = x0.min(nx);
            x1 = x1.max(nx);
            y0 = y0.min(ny);
            y1 = y1.max(ny);
        }
        ((x1 - x0) * 0.5, (y1 - y0) * 0.5, (x0 + x1) * 0.5, (y0 + y1) * 0.5)
    };
    let limit = 1.0 / AUTO_MARGIN;
    // Every corner in front of the eye (w > 0 strictly), and inside the margin once centred.
    let fits = |d: f64| -> bool {
        let (hx, hy, _, _) = extents(d);
        zs.iter().all(|z| d + z > 0.0) && hx <= limit && hy <= limit
    };
    let room = r * 1e-3;
    let (mut a, mut b) = (-min_z, far.max(-min_z + room));
    // `far` fits up to rounding at its binding corner; back off until it does.
    let mut tries = 0;
    while !fits(b) {
        tries += 1;
        if tries > FIT_STEPS {
            return None;
        }
        b = b + b;
    }
    for _ in 0..FIT_STEPS {
        let m = 0.5 * (a + b);
        if fits(m) {
            b = m;
        } else {
            a = m;
        }
    }
    let dist = b.max(-min_z + room);
    let (_, _, mx, my) = extents(dist);
    let eye = addd(c, scaled(dir, dist));
    let view = look_at_dir(eye, f, upv)?;
    let frame = Frame {
        view,
        fx: (1.0 / tan_h) as f32,
        fy: (1.0 / tan_v) as f32,
        near: ((dist + min_z) * 0.5) as f32,
        shift_x: (0.0 - mx) as f32,
        shift_y: (0.0 - my) as f32,
    };
    let ok = frame.view.iter().all(|v| v.is_finite()) && frame.near > 0.0 && frame.near.is_finite() && frame.shift_x.is_finite() && frame.shift_y.is_finite();
    ok.then_some(frame)
}

/// The direction from the target to the eye for turntable frame k, and the up vector.
fn orbit_dir(up: UpAxis, k: u8) -> (D3, D3) {
    let s = 0.5f64.sqrt();
    let (sin_a, cos_a) = [(s, s), (1.0, 0.0), (s, -s), (0.0, -1.0), (-s, -s), (-1.0, 0.0), (-s, s), (0.0, 1.0)][k as usize];
    let (cos_e, sin_e) = (0.75f64.sqrt(), 0.5);
    let dy = [cos_e * sin_a, sin_e, cos_e * cos_a];
    match up {
        UpAxis::Y => (dy, [0.0, 1.0, 0.0]),
        // Y-up (x, y, z) is Z-up (x, -z, y): glTF's front (+Z) is Blender's front (-Y).
        UpAxis::Z => ([dy[0], -dy[2], dy[1]], [0.0, 0.0, 1.0]),
    }
}

/// The auto view: the turntable-0 direction, aimed at the box centre, pulled back just far
/// enough that all 8 box corners are inside the frame (with the margin).
fn orbit_box(up: UpAxis, (lo, hi): (D3, D3), r: f64, aspect: f64) -> Result<Frame, String> {
    let (dir, upv) = orbit_dir(up, 0);
    let c = scaled(addd(lo, hi), 0.5);
    let f = scaled(dir, -1.0);
    let rt = normalized(crossd(f, upv)).ok_or("degenerate auto camera")?;
    let u = crossd(rt, f);
    let tan_v = AUTO_TAN_HALF_FOV;
    let tan_h = tan_v * aspect;
    let mut dist = 0.0f64;
    let mut min_z = f64::MAX;
    for k in 0..8 {
        let p = [
            if k & 1 == 0 { lo[0] } else { hi[0] },
            if k & 2 == 0 { lo[1] } else { hi[1] },
            if k & 4 == 0 { lo[2] } else { hi[2] },
        ];
        let q = subd(p, c);
        let (x, y, z) = (dotd(q, rt), dotd(q, u), dotd(q, f));
        // Corner depth is dist + z; it must satisfy |x| <= tan_h * depth / margin (same for y).
        dist = dist.max(x.abs() * AUTO_MARGIN / tan_h - z).max(y.abs() * AUTO_MARGIN / tan_v - z);
        min_z = min_z.min(z);
    }
    // Every corner in front of the eye, with room for the near plane.
    let dist = dist.max(-min_z + r * 1e-3);
    let eye = addd(c, scaled(dir, dist));
    let view = look_at_dir(eye, f, upv).ok_or("degenerate auto camera")?;
    Ok(Frame {
        view,
        fx: (1.0 / tan_h) as f32,
        fy: (1.0 / tan_v) as f32,
        near: ((dist + min_z) * 0.5) as f32,
        shift_x: 0.0,
        shift_y: 0.0,
    })
}

fn orbit(up: UpAxis, c: D3, r: f64, k: u8, aspect: f64) -> Result<Frame, String> {
    let (dir, upv) = orbit_dir(up, k);
    let tan_v = AUTO_TAN_HALF_FOV;
    let t = tan_v.min(tan_v * aspect);
    let sin_half = t / (1.0 + t * t).sqrt();
    let rr = r * AUTO_MARGIN;
    let dist = rr / sin_half;
    let eye = addd(c, scaled(dir, dist));
    let view = look_at_dir(eye, subd(c, eye), upv).ok_or("degenerate orbit camera")?;
    Ok(Frame { view, fx: (1.0 / (tan_v * aspect)) as f32, fy: (1.0 / tan_v) as f32, near: ((dist - rr) * 0.5) as f32, shift_x: 0.0, shift_y: 0.0 })
}

fn given(cam: &Camera, scene_r: f64, aspect: f64) -> Result<Frame, String> {
    let eye = d3(cam.position);
    let fwd = match (cam.target, cam.forward) {
        (Some(t), _) => subd(d3(t), eye),
        (None, Some(f)) => d3(f),
        (None, None) => return Err("the camera needs a target or a forward vector".into()),
    };
    let view = look_at_dir(eye, fwd, d3(cam.up)).ok_or("the camera's forward and up are parallel or zero")?;
    let tan_v = crate::trig::tan(cam.yfov as f64 * 0.5);
    if !(tan_v > 0.0 && tan_v.is_finite()) {
        return Err("the camera's yfov is out of range".into());
    }
    let near = cam.znear.map(|z| z as f64).filter(|z| *z > 0.0 && z.is_finite()).unwrap_or(scene_r * 1e-4).max(1e-6);
    Ok(Frame { view, fx: (1.0 / (tan_v * aspect)) as f32, fy: (1.0 / tan_v) as f32, near: near as f32, shift_x: 0.0, shift_y: 0.0 })
}

/// World-to-view for an eye looking along `fwd` with `up` roughly up.
pub fn look_at_dir(eye: D3, fwd: D3, up: D3) -> Option<M4> {
    let f = normalized(fwd)?;
    let r = normalized(crossd(f, normalized(up)?))?;
    let u = crossd(r, f);
    Some([
        r[0], u[0], -f[0], 0.0, //
        r[1], u[1], -f[1], 0.0, //
        r[2], u[2], -f[2], 0.0, //
        -dotd(r, eye), -dotd(u, eye), dotd(f, eye), 1.0,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testgen;

    #[test]
    fn auto_view_centres_the_scene() {
        let s = testgen::white_cube_scene();
        let f = frame(&s, &[0], ViewSpec::Auto, 4.0 / 3.0).unwrap();
        // The centre maps to the view axis.
        let c = m4_point(&f.view, [0.0, 0.0, 0.0]);
        assert!(c[0].abs() < 1e-9 && c[1].abs() < 1e-9 && c[2] < 0.0);
        assert!(f.near > 0.0);
        // Turntable 0 looks the same way as auto, from further back (sphere fit); frames differ.
        let t0 = frame(&s, &[0], ViewSpec::Turntable(0), 4.0 / 3.0).unwrap();
        for i in [2, 6, 10] {
            assert!((t0.view[i] - f.view[i]).abs() < 1e-12);
        }
        assert!(m4_point(&t0.view, [0.0, 0.0, 0.0])[2] < c[2]);
        assert_ne!(frame(&s, &[0], ViewSpec::Turntable(3), 4.0 / 3.0).unwrap(), t0);
        // The box fit: every corner projects inside the frame.
        for k in 0..8 {
            let p = [if k & 1 == 0 { -0.5 } else { 0.5 }, if k & 2 == 0 { -0.5 } else { 0.5 }, if k & 4 == 0 { -0.5 } else { 0.5 }];
            let v = m4_point(&f.view, p);
            let w = -v[2];
            assert!(w > f.near as f64);
            assert!((v[0] * f.fx as f64 / w).abs() <= 1.0 && (v[1] * f.fy as f64 / w).abs() <= 1.0);
        }
        assert!(frame(&s, &[0], ViewSpec::Turntable(8), 1.0).is_err());
        assert_eq!((f.shift_x.to_bits(), f.shift_y.to_bits()), (0, 0), "no shift, +0");
        assert_eq!((f.projection()[8].to_bits(), f.projection()[9].to_bits()), (0, 0), "+0, not -0");
    }

    fn all(s: &Scene) -> Vec<u32> {
        (0..s.objects.len() as u32).collect()
    }

    /// A box's 8 corners through a frame's f32 matrices, as a client projects them: NDC
    /// (min x, min y, max x, max y).
    fn ndc_box(f: &Frame, lo: D3, hi: D3) -> [f64; 4] {
        let (v, p) = (f.view_f32(), f.projection());
        let mul = |m: &[f32; 16], x: [f64; 4]| -> [f64; 4] {
            let mut o = [0.0; 4];
            for (r, out) in o.iter_mut().enumerate() {
                *out = (0..4).map(|c| m[c * 4 + r] as f64 * x[c]).sum();
            }
            o
        };
        let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        for k in 0..8 {
            let q = [if k & 1 == 0 { lo[0] } else { hi[0] }, if k & 2 == 0 { lo[1] } else { hi[1] }, if k & 4 == 0 { lo[2] } else { hi[2] }, 1.0];
            let clip = mul(&p, mul(&v, q));
            assert!(clip[3] >= f.near as f64 * 0.999, "a corner nearer than the near plane");
            let (x, y) = (clip[0] / clip[3], clip[1] / clip[3]);
            b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
        }
        b
    }

    /// The same scene with Z up: Y-up (x, y, z) is Z-up (x, -z, y).
    fn z_up(s: &Scene) -> Scene {
        let rot: M4 = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0];
        let mut z = s.clone();
        z.up = UpAxis::Z;
        for o in &mut z.objects {
            o.transform = m4_to_f32(&m4_mul(&rot, &m4_from_f32(&o.transform)));
        }
        z
    }

    #[test]
    fn content_framing_fits_and_centres_the_contents_with_a_lens_shift() {
        let interior = testgen::interior_scene();
        let cases = [
            ("interior 4:3", interior.clone(), 4.0 / 3.0),
            ("interior 1:1", interior.clone(), 1.0),
            ("interior 9:16", interior.clone(), 9.0 / 16.0),
            ("interior z-up", z_up(&interior), 4.0 / 3.0),
            ("small object 16:9", testgen::small_object_scene(), 16.0 / 9.0),
            ("enclosed", testgen::enclosed_scene(3), 4.0 / 3.0),
        ];
        let limit = 1.0 / AUTO_MARGIN;
        for (name, s, aspect) in cases {
            let a = all(&s);
            let c = contents::classify(&s, &a);
            let cf = contents::content_frame(&a, &c).expect("contents");
            let (f, m) = frame_contents(&s, &a, &c, aspect).expect("a frame");
            assert!(m.contents && m.left_out == cf.left_out && m.grown == cf.grown, "{name}: {m:?}");
            assert!(f.shift_x != 0.0 || f.shift_y != 0.0, "{name}: a real lens shift");
            let b = ndc_box(&f, cf.lo, cf.hi);
            eprintln!("[framing] {name}: shift ({}, {}), contents' box in NDC {b:?}", f.shift_x, f.shift_y);
            // Inside the frame with the margin.
            assert!(b.iter().all(|v| v.abs() <= limit + 1e-5), "{name}: {b:?}");
            // Centred by the shift.
            assert!(((b[0] + b[2]) * 0.5).abs() < 1e-5 && ((b[1] + b[3]) * 0.5).abs() < 1e-5, "{name}: {b:?}");
            // Tight: the binding axis reaches the margin.
            let half = ((b[2] - b[0]) * 0.5).max((b[3] - b[1]) * 0.5);
            assert!((half - limit).abs() < 1e-4, "{name}: half extent {half}");
            // Without the shift the projected box would be off centre: the shift is what centres it.
            let unshifted = ndc_box(&Frame { shift_x: 0.0, shift_y: 0.0, ..f }, cf.lo, cf.hi);
            assert!(((unshifted[0] + unshifted[2]) * 0.5 + f.shift_x as f64).abs() < 1e-5, "{name}");
            assert!(((unshifted[1] + unshifted[3]) * 0.5 + f.shift_y as f64).abs() < 1e-5, "{name}");
            // The auto view's direction exactly: the eye moved, the camera didn't turn.
            let auto = frame(&s, &a, ViewSpec::Auto, aspect).expect("the auto view");
            for i in [0, 1, 2, 4, 5, 6, 8, 9, 10] {
                assert_eq!(f.view[i].to_bits(), auto.view[i].to_bits(), "{name}: view[{i}]");
            }
            assert_eq!((f.fx, f.fy), (auto.fx, auto.fy), "{name}: the same lens");
            // The projection carries the shift.
            assert_eq!((f.projection()[8], f.projection()[9]), (-f.shift_x, -f.shift_y));
        }
    }

    #[test]
    fn content_framing_frames_tighter_than_the_bounds() {
        // The interior's contents fill more of the image than with every object framed.
        let s = testgen::interior_scene();
        let a = all(&s);
        let c = contents::classify(&s, &a);
        let cf = contents::content_frame(&a, &c).expect("contents");
        let (f, _) = frame_contents(&s, &a, &c, 4.0 / 3.0).expect("contents framed");
        let bounds = frame(&s, &a, ViewSpec::Auto, 4.0 / 3.0).expect("bounds framed");
        let area = |b: [f64; 4]| (b[2] - b[0]) * (b[3] - b[1]);
        let (tight, loose) = (area(ndc_box(&f, cf.lo, cf.hi)), area(ndc_box(&bounds, cf.lo, cf.hi)));
        eprintln!("[framing] interior contents' projected area: {tight:.3} framed on contents, {loose:.3} on bounds");
        assert!(tight > loose * 1.2, "{tight} {loose}");
    }

    #[test]
    fn content_framing_falls_back_to_the_bounds_framing() {
        let room = testgen::empty_room_scene();
        let nothing = Scene { objects: Vec::new(), ..room.clone() };
        let empty_mesh = testgen::degenerate_scenes().into_iter().find(|(n, _)| *n == "empty-mesh").expect("empty-mesh").1;
        for (name, s) in [("all shell", room), ("no objects", nothing), ("empty mesh", empty_mesh)] {
            let a = all(&s);
            let c = contents::classify(&s, &a);
            let (f, m) = frame_contents(&s, &a, &c, 4.0 / 3.0).expect("a frame");
            assert_eq!(m, ContentFraming { contents: false, left_out: Vec::new(), grown: false }, "{name}");
            assert_eq!(f, frame(&s, &a, ViewSpec::Auto, 4.0 / 3.0).expect("the auto view"), "{name}");
        }
    }

    #[test]
    fn content_framing_doesnt_depend_on_the_objects_order() {
        let s = testgen::interior_scene();
        let a = all(&s);
        let (f, _) = frame_contents(&s, &a, &contents::classify(&s, &a), 4.0 / 3.0).expect("a frame");
        let mut p = s.clone();
        p.objects.reverse();
        p.objects.rotate_left(4);
        let pa = all(&p);
        let (pf, _) = frame_contents(&p, &pa, &contents::classify(&p, &pa), 4.0 / 3.0).expect("a frame");
        assert_eq!(f, pf);
    }
}
