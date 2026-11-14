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

use serde::{Deserialize, Serialize};

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

/// World-to-view (camera looks down -Z, +Y up), and the projection scales.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub view: M4,
    /// x_clip = fx * x_view, y_clip = fy * y_view, w = -z_view.
    pub fx: f32,
    pub fy: f32,
    /// Near plane distance (> 0).
    pub near: f32,
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
    /// (it tests depth as 1/w, never NDC z). No lens shift yet (`[8]` and `[9]` are 0).
    pub fn projection(&self) -> [f32; 16] {
        [
            self.fx, 0.0, 0.0, 0.0, //
            0.0, self.fy, 0.0, 0.0, //
            0.0, 0.0, -1.0, -1.0, //
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
    Ok(Frame { view, fx: (1.0 / (tan_v * aspect)) as f32, fy: (1.0 / tan_v) as f32, near: ((dist - rr) * 0.5) as f32 })
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
    Ok(Frame { view, fx: (1.0 / (tan_v * aspect)) as f32, fy: (1.0 / tan_v) as f32, near: near as f32 })
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
    }
}
