//! `tan` for a given camera's field of view, with only `+ - * /` in a fixed
//! order, so a camera view is bit-identical on every IEEE 754 machine (the
//! platform's `tan` may differ in its last bit between libms and
//! architectures). Rust never contracts `a * b + c` into a fused multiply-add
//! on its own and nothing here calls `mul_add`.
//!
//! **Method** (textbook): the angle is quartered, sine and cosine of the
//! quarter come from their Taylor series (Horner form, terms to x^17 and
//! x^18; at |x| <= pi/8 the first term left out is below 1e-21), then the
//! double-angle formulas are applied twice (`sin 2y = 2 sin y cos y`,
//! `cos 2y = cos^2 y - sin^2 y`) and `tan = sin / cos`. The coefficients are
//! `1/n!` as f64 constants, evaluated at compile time (correctly rounded
//! everywhere). `tests::close_to_the_platforms_tan` checks it against libm's
//! `tan` (relative error below 1e-13 up to a field of view of 3 radians).

const S3: f64 = -1.0 / 6.0;
const S5: f64 = 1.0 / 120.0;
const S7: f64 = -1.0 / 5_040.0;
const S9: f64 = 1.0 / 362_880.0;
const S11: f64 = -1.0 / 39_916_800.0;
const S13: f64 = 1.0 / 6_227_020_800.0;
const S15: f64 = -1.0 / 1_307_674_368_000.0;
const S17: f64 = 1.0 / 355_687_428_096_000.0;
const C2: f64 = -1.0 / 2.0;
const C4: f64 = 1.0 / 24.0;
const C6: f64 = -1.0 / 720.0;
const C8: f64 = 1.0 / 40_320.0;
const C10: f64 = -1.0 / 3_628_800.0;
const C12: f64 = 1.0 / 479_001_600.0;
const C14: f64 = -1.0 / 87_178_291_200.0;
const C16: f64 = 1.0 / 20_922_789_888_000.0;
const C18: f64 = -1.0 / 6_402_373_705_728_000.0;

/// Sine and cosine of a small angle (|y| <= pi/8).
fn sin_cos_small(y: f64) -> (f64, f64) {
    let y2 = y * y;
    let s = y * (1.0 + y2 * (S3 + y2 * (S5 + y2 * (S7 + y2 * (S9 + y2 * (S11 + y2 * (S13 + y2 * (S15 + y2 * S17))))))));
    let c = 1.0 + y2 * (C2 + y2 * (C4 + y2 * (C6 + y2 * (C8 + y2 * (C10 + y2 * (C12 + y2 * (C14 + y2 * (C16 + y2 * C18))))))));
    (s, c)
}

/// `tan(x)` for `0 < x < pi/2` (a half field of view). Outside that range
/// the result is still finite or infinite, never a panic; the caller checks it.
pub fn tan(x: f64) -> f64 {
    let (mut s, mut c) = sin_cos_small(x * 0.25);
    for _ in 0..2 {
        let s2 = 2.0 * s * c;
        let c2 = c * c - s * s;
        s = s2;
        c = c2;
    }
    s / c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_to_the_platforms_tan() {
        let mut worst = 0.0f64;
        for i in 1..3000 {
            let x = i as f64 * 0.0005;
            let want = x.tan();
            let got = tan(x);
            worst = worst.max(((got - want) / want).abs());
        }
        assert!(worst < 1e-13, "relative error {worst}");
        // Near pi/2 (a field of view just under pi) it loses digits, as any
        // cos-based tan does, but stays close and finite.
        let x = (std::f32::consts::PI as f64 - 1e-4) * 0.5;
        assert!(((tan(x) - x.tan()) / x.tan()).abs() < 1e-9);
        // Exact values: tan(pi/4) = 1.
        assert!((tan(std::f64::consts::FRAC_PI_4) - 1.0).abs() < 1e-15);
    }

    /// The same bits on every machine: pinned values (computed once in f64 by
    /// the same operations; a change here is a change to every camera view's
    /// pixels).
    #[test]
    fn pinned_bits() {
        for (x, bits) in [(0.345f64, PINNED[0]), (0.6911112070083618, PINNED[1]), (1.2, PINNED[2])] {
            assert_eq!(tan(x).to_bits(), bits, "tan({x}) = {:e} ({:#x})", tan(x), tan(x).to_bits());
        }
    }

    const PINNED: [u64; 3] = [0x3fd6_fff5_b052_4311, 0x3fea_7878_a1ec_da13, 0x4004_93c4_3acb_164b];
}
