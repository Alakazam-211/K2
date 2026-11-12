//! Small vector and matrix helpers.
//!
//! Determinism: only `+ - * /` and `sqrt` (all correctly rounded by IEEE 754), evaluated in the
//! order written. Rust never contracts `a * b + c` into a fused multiply-add on its own, and
//! nothing here calls `mul_add`, so the results are the same on every IEEE machine. No libm
//! function is called on the render path: the sRGB curve is a table ([`crate::srgb`]) and a
//! camera's field of view goes through [`crate::trig::tan`].

pub type V3 = [f32; 3];
pub type D3 = [f64; 3];
/// Column-major 4x4: element (row r, column c) is `m[c * 4 + r]`.
pub type M4 = [f64; 16];

#[inline]
pub fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[inline]
pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[inline]
pub fn neg(a: V3) -> V3 {
    [-a[0], -a[1], -a[2]]
}

#[inline]
pub fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// Unit vector, or zero for a zero or non-finite input.
#[inline]
pub fn normalize(a: V3) -> V3 {
    let l2 = dot(a, a);
    if l2 > 0.0 && l2.is_finite() {
        let inv = 1.0 / l2.sqrt();
        [a[0] * inv, a[1] * inv, a[2] * inv]
    } else {
        [0.0; 3]
    }
}

#[inline]
pub fn dotd(a: D3, b: D3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[inline]
pub fn subd(a: D3, b: D3) -> D3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[inline]
pub fn addd(a: D3, b: D3) -> D3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

#[inline]
pub fn scaled(a: D3, s: f64) -> D3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

#[inline]
pub fn crossd(a: D3, b: D3) -> D3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

pub fn lend(a: D3) -> f64 {
    dotd(a, a).sqrt()
}

/// Unit vector, or `None` when the length is (near) zero or not finite.
pub fn normalized(a: D3) -> Option<D3> {
    let l = lend(a);
    if l > 1e-12 && l.is_finite() {
        Some(scaled(a, 1.0 / l))
    } else {
        None
    }
}

pub fn d3(a: [f32; 3]) -> D3 {
    [a[0] as f64, a[1] as f64, a[2] as f64]
}

pub const IDENTITY: M4 = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

pub fn m4_from_f32(m: &[f32; 16]) -> M4 {
    m.map(f64::from)
}

pub fn m4_to_f32(m: &M4) -> [f32; 16] {
    m.map(|v| v as f32)
}

pub fn m4_mul(a: &M4, b: &M4) -> M4 {
    let mut r = [0.0; 16];
    for c in 0..4 {
        for row in 0..4 {
            let mut s = 0.0;
            for k in 0..4 {
                s += a[k * 4 + row] * b[c * 4 + k];
            }
            r[c * 4 + row] = s;
        }
    }
    r
}

/// An affine point transform (the projective row is ignored).
pub fn m4_point(m: &M4, p: D3) -> D3 {
    [
        m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12],
        m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13],
        m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14],
    ]
}

/// A direction transform (no translation).
pub fn m4_dir(m: &M4, d: D3) -> D3 {
    [
        m[0] * d[0] + m[4] * d[1] + m[8] * d[2],
        m[1] * d[0] + m[5] * d[1] + m[9] * d[2],
        m[2] * d[0] + m[6] * d[1] + m[10] * d[2],
    ]
}

fn col(m: &M4, c: usize) -> D3 {
    [m[c * 4], m[c * 4 + 1], m[c * 4 + 2]]
}

/// Determinant of the upper 3x3. Negative: the transform mirrors (winding flips).
pub fn m3_det(m: &M4) -> f64 {
    dotd(col(m, 0), crossd(col(m, 1), col(m, 2)))
}

/// The normal matrix as three columns: the cofactor matrix of the upper 3x3
/// (= det * inverse-transpose), times the determinant's sign. Unnormalized.
pub fn normal_matrix(m: &M4) -> [D3; 3] {
    let (c0, c1, c2) = (col(m, 0), col(m, 1), col(m, 2));
    let s = if m3_det(m) < 0.0 { -1.0 } else { 1.0 };
    [scaled(crossd(c1, c2), s), scaled(crossd(c2, c0), s), scaled(crossd(c0, c1), s)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_matrix_of_scale_and_mirror() {
        // Non-uniform scale: the normal of the plane x + y = 0 must stay perpendicular.
        let mut m = IDENTITY;
        m[0] = 2.0;
        let nm = normal_matrix(&m);
        let n = [1.0, 1.0, 0.0];
        let t = [nm[0][0] * n[0] + nm[1][0] * n[1], nm[0][1] * n[0] + nm[1][1] * n[1], 0.0];
        // a vector in the plane (1, -1, 0) maps to (2, -1, 0); the normal must be perpendicular to it.
        assert!((t[0] * 2.0 - t[1]).abs() < 1e-12);
        // Mirror in x: the determinant is negative, the normal keeps its outward sense.
        let mut mm = IDENTITY;
        mm[0] = -1.0;
        assert!(m3_det(&mm) < 0.0);
        let nm = normal_matrix(&mm);
        // +x normal on a mirrored mesh points to -x in the world.
        assert_eq!(nm[0][0], -1.0);
    }
}
