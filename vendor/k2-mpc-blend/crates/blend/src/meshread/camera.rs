//! A perspective camera in world space, from its world matrix, its camera data and the drawn
//! scene's render size ([`camera`]); and the fixed arc tangent it needs ([`atan`]).
//!
//! - **Where it looks:** `position` is the world matrix applied to the origin; `forward` the
//!   camera's local -Z through it, normalised; `up` its local +Y through it, made orthogonal to
//!   `forward` and normalised. A direction of zero length is `camera_degenerate`.
//! - **The image:** width `xsch * xasp` over height `ysch * yasp` (the render size and the pixel
//!   aspect), which must be finite and positive (`camera_no_aspect`).
//! - **The vertical field of view**, with `L` the lens, `sx` and `sy` the sensor's width and
//!   height, `a` the aspect: the sensor fit `horizontal`, and `auto` when `a >= 1`, give
//!   `2 atan(sx / (2 L a))`; `vertical` gives `2 atan(sy / (2 L))`; `auto` when `a < 1` gives
//!   `2 atan(sx / (2 L))` (the sensor's width spans the larger side, the height). It must lie in
//!   (0, pi) (`camera_bad_fov`).
//! - **The sensor fit's values** are `0` auto, `1` horizontal, `2` vertical (`PROVENANCE.md`,
//!   "Cameras": pinned by the program's own field of view for a camera in each fit); any other
//!   is `camera_sensor_fit_unknown`.
//!   Only perspective cameras (`type` 0) are written (`camera_not_perspective`).
//!
//! **Deterministic:** `f64` with a fixed order of operations, `sqrt` (correctly rounded), and
//! [`atan`], built of IEEE operations only, never the platform's library.

use k2_mpc_base::placement::Mat4;

use super::k2scene::SensorFit;
use crate::mutant;

/// The arc tangent of `x`, in radians, from IEEE operations only (`+`, `-`, `*`, `/`, `sqrt`) in
/// a fixed order, so every platform gives the same bits. `x` above 1 is taken as
/// `pi/2 - atan(1/x)`; the argument is then halved three times
/// (`atan t = 2 atan(t / (1 + sqrt(1 + t^2)))`), and the series `t - t^3/3 + t^5/5 - ...`
/// summed to 12 terms, where `t^2 < 0.01`. A NaN gives NaN.
pub fn atan(x: f64) -> f64 {
    if x.is_nan() {
        return x;
    }
    if x < 0.0 {
        return -atan(-x);
    }
    if x.is_infinite() {
        return std::f64::consts::FRAC_PI_2;
    }
    if x > 1.0 {
        return std::f64::consts::FRAC_PI_2 - atan(1.0 / x);
    }
    let mut t = x;
    for _ in 0..3 {
        t /= 1.0 + (1.0 + t * t).sqrt();
    }
    let t2 = t * t;
    // Horner, from the last term down: t * (1 - t2/3 + t2^2/5 - ... - t2^11/23)
    let mut s = 0.0f64;
    for k in (0..12).rev() {
        let term = 1.0 / f64::from(2 * k + 1);
        s = if k % 2 == 0 { term } else { -term } + t2 * s;
    }
    8.0 * (t * s)
}

/// The drawn scene's render size: its width and height in pixels, and the pixel aspect.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Render {
    /// `r.xsch`.
    pub xsch: f64,
    /// `r.ysch`.
    pub ysch: f64,
    /// `r.xasp`.
    pub xasp: f64,
    /// `r.yasp`.
    pub yasp: f64,
}

/// A camera's data, as the file stores it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraData {
    /// `type`: 0 perspective.
    pub kind: i64,
    /// `lens`, in mm.
    pub lens: f32,
    /// `sensor_x`, `sensor_y`, in mm.
    pub sensor: [f32; 2],
    /// `sensor_fit`.
    pub fit: i64,
    /// `clipsta`, the near clip distance.
    pub clipsta: f32,
}

/// What [`camera`] works out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lens {
    /// Where it is.
    pub position: [f32; 3],
    /// Where it looks.
    pub forward: [f32; 3],
    /// Its up, orthogonal to `forward`.
    pub up: [f32; 3],
    /// The vertical field of view, in radians.
    pub yfov: f32,
    /// Width over height.
    pub aspect: f32,
    /// The near clip distance.
    pub znear: f32,
    /// The lens, in mm.
    pub lens_mm: f32,
    /// The sensor's width and height, in mm.
    pub sensor_mm: [f32; 2],
    /// How the sensor fits.
    pub fit: SensorFit,
}

/// The sensor fit's value as the file stores it.
pub fn sensor_fit(v: i64) -> Option<SensorFit> {
    match v {
        0 => Some(SensorFit::Auto),
        1 => Some(SensorFit::Horizontal),
        2 => Some(SensorFit::Vertical),
        _ => None,
    }
}

type V3 = [f64; 3];

fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// `v` normalised, or `None` when its length isn't finite and positive.
fn unit(v: V3) -> Option<V3> {
    let l = dot(v, v).sqrt();
    (l.is_finite() && l > 0.0).then(|| [v[0] / l, v[1] / l, v[2] / l])
}

fn f32s(v: V3) -> Option<[f32; 3]> {
    let o = v.map(|x| x as f32);
    o.iter().all(|x| x.is_finite()).then_some(o)
}

/// The camera of world matrix `w` (column-major), data `d`, in a scene of render size `r`; or
/// the reason word it isn't written (the module's rules).
pub fn camera(w: &Mat4, d: &CameraData, r: Option<&Render>) -> Result<Lens, &'static str> {
    if d.kind != 0 && !mutant::active("OrthoCameraKept") {
        return Err("camera_not_perspective");
    }
    let fit = if mutant::active("SensorFitIgnored") {
        // planted: every known fit taken as automatic
        sensor_fit(d.fit).map(|_| SensorFit::Auto)
    } else {
        sensor_fit(d.fit)
    };
    let fit = fit.ok_or("camera_sensor_fit_unknown")?;
    let positive = |x: f32| x.is_finite() && x > 0.0;
    if !positive(d.lens) || !positive(d.sensor[0]) || !positive(d.sensor[1]) {
        return Err("camera_no_lens");
    }
    if !d.clipsta.is_finite() {
        return Err("camera_no_clip");
    }
    let position = f32s([w[12], w[13], w[14]]).ok_or("camera_degenerate")?;
    let forward = unit([-w[8], -w[9], -w[10]]).ok_or("camera_degenerate")?;
    let y = [w[4], w[5], w[6]];
    let k = dot(y, forward);
    let up = unit([
        y[0] - k * forward[0],
        y[1] - k * forward[1],
        y[2] - k * forward[2],
    ])
    .ok_or("camera_degenerate")?;
    let r = r.ok_or("camera_no_aspect")?;
    let (wd, ht) = (r.xsch * r.xasp, r.ysch * r.yasp);
    let aspect = wd / ht;
    if !(aspect.is_finite() && aspect > 0.0) || !(aspect as f32).is_finite() || aspect as f32 <= 0.0
    {
        return Err("camera_no_aspect");
    }
    let (l, sx, sy) = (
        f64::from(d.lens),
        f64::from(d.sensor[0]),
        f64::from(d.sensor[1]),
    );
    let half = match fit {
        SensorFit::Vertical => sy / (2.0 * l),
        SensorFit::Horizontal => sx / (2.0 * l * aspect),
        SensorFit::Auto if aspect >= 1.0 => sx / (2.0 * l * aspect),
        SensorFit::Auto => sx / (2.0 * l),
    };
    let yfov = (2.0 * atan(half)) as f32;
    if !(yfov > 0.0 && yfov < std::f32::consts::PI) {
        return Err("camera_bad_fov");
    }
    Ok(Lens {
        position,
        forward: f32s(forward).ok_or("camera_degenerate")?,
        up: f32s(up).ok_or("camera_degenerate")?,
        yfov,
        aspect: aspect as f32,
        znear: d.clipsta,
        lens_mm: d.lens,
        sensor_mm: d.sensor,
        fit,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atan_is_the_library_s_to_a_few_ulps() {
        let close = |x: f64| {
            let (a, b) = (atan(x), x.atan());
            assert!(
                (a - b).abs() <= 16.0 * f64::EPSILON * b.abs(),
                "{x}: {a} vs {b}"
            );
            assert_eq!(atan(-x), -a, "{x}");
        };
        let mut n = 0;
        // every magnitude, and a fine sweep where the field of view lives
        for e in -300..=300 {
            for m in [1.0, 1.37, 2.9, 5.5, 8.1] {
                close(m * 10f64.powi(e));
                n += 1;
            }
        }
        for i in 1..=8000 {
            close(f64::from(i) * 0.0005);
            n += 1;
        }
        assert_eq!(atan(0.0), 0.0);
        assert_eq!(atan(f64::INFINITY), std::f64::consts::FRAC_PI_2);
        assert!(atan(f64::NAN).is_nan());
        assert!(n > 10_000, "{n}");
    }
}
