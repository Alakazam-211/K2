//! The inverse transforms (RFC 6386 Section 14).
//!
//! Intermediate values are stored as 16-bit integers between the passes, as
//! the reference decoder does, so that out-of-range input wraps identically.

const COSPI8SQRT2MINUS1: i32 = 20091;
const SINPI8SQRT2: i32 = 35468;

/// The inverse Walsh-Hadamard transform of the Y2 block. Writes the 16
/// results to `coeffs[i * 16]` (the DC of each luma block).
pub fn inverse_wht(input: &[i16; 16], coeffs: &mut [i16]) {
    let mut tmp = [0i16; 16];
    for i in 0..4 {
        let ip0 = input[i] as i32;
        let ip4 = input[4 + i] as i32;
        let ip8 = input[8 + i] as i32;
        let ip12 = input[12 + i] as i32;
        let a1 = ip0 + ip12;
        let b1 = ip4 + ip8;
        let c1 = ip4 - ip8;
        let d1 = ip0 - ip12;
        tmp[i] = (a1 + b1) as i16;
        tmp[4 + i] = (c1 + d1) as i16;
        tmp[8 + i] = (a1 - b1) as i16;
        tmp[12 + i] = (d1 - c1) as i16;
    }
    for i in 0..4 {
        let ip = &tmp[i * 4..i * 4 + 4];
        let a1 = ip[0] as i32 + ip[3] as i32;
        let b1 = ip[1] as i32 + ip[2] as i32;
        let c1 = ip[1] as i32 - ip[2] as i32;
        let d1 = ip[0] as i32 - ip[3] as i32;
        let a2 = a1 + b1;
        let b2 = c1 + d1;
        let c2 = a1 - b1;
        let d2 = d1 - c1;
        coeffs[(i * 4) * 16] = ((a2 + 3) >> 3) as i16;
        coeffs[(i * 4 + 1) * 16] = ((b2 + 3) >> 3) as i16;
        coeffs[(i * 4 + 2) * 16] = ((c2 + 3) >> 3) as i16;
        coeffs[(i * 4 + 3) * 16] = ((d2 + 3) >> 3) as i16;
    }
}

#[inline(always)]
fn mul_sin(x: i32) -> i32 {
    (x * SINPI8SQRT2) >> 16
}

#[inline(always)]
fn mul_cos(x: i32) -> i32 {
    x + ((x * COSPI8SQRT2MINUS1) >> 16)
}

#[inline(always)]
fn clamp255(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// Inverse DCT of one 4x4 block of dequantized coefficients, added to the
/// prediction already in `dst` (in place).
pub fn idct_add(coeffs: &[i16], dst: &mut [u8], pos: usize, stride: usize) {
    let mut tmp = [0i16; 16];
    // Vertical pass (columns).
    for i in 0..4 {
        let ip0 = coeffs[i] as i32;
        let ip4 = coeffs[4 + i] as i32;
        let ip8 = coeffs[8 + i] as i32;
        let ip12 = coeffs[12 + i] as i32;
        let a1 = ip0 + ip8;
        let b1 = ip0 - ip8;
        let c1 = mul_sin(ip4) - mul_cos(ip12);
        let d1 = mul_cos(ip4) + mul_sin(ip12);
        tmp[i] = (a1 + d1) as i16;
        tmp[12 + i] = (a1 - d1) as i16;
        tmp[4 + i] = (b1 + c1) as i16;
        tmp[8 + i] = (b1 - c1) as i16;
    }
    // Horizontal pass (rows), then add.
    for r in 0..4 {
        let ip = &tmp[r * 4..r * 4 + 4];
        let a1 = ip[0] as i32 + ip[2] as i32;
        let b1 = ip[0] as i32 - ip[2] as i32;
        let c1 = mul_sin(ip[1] as i32) - mul_cos(ip[3] as i32);
        let d1 = mul_cos(ip[1] as i32) + mul_sin(ip[3] as i32);
        let o0 = ((a1 + d1 + 4) >> 3) as i16 as i32;
        let o3 = ((a1 - d1 + 4) >> 3) as i16 as i32;
        let o1 = ((b1 + c1 + 4) >> 3) as i16 as i32;
        let o2 = ((b1 - c1 + 4) >> 3) as i16 as i32;
        let row = &mut dst[pos + r * stride..pos + r * stride + 4];
        row[0] = clamp255(row[0] as i32 + o0);
        row[1] = clamp255(row[1] as i32 + o1);
        row[2] = clamp255(row[2] as i32 + o2);
        row[3] = clamp255(row[3] as i32 + o3);
    }
}

/// The inverse DCT of a block whose only non-zero coefficient is the DC.
pub fn idct_dc_add(dc: i16, dst: &mut [u8], pos: usize, stride: usize) {
    let v = ((dc as i32 + 4) >> 3) as i16 as i32;
    for r in 0..4 {
        let row = &mut dst[pos + r * stride..pos + r * stride + 4];
        for p in row.iter_mut() {
            *p = clamp255(*p as i32 + v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dc_only_matches_the_full_transform() {
        for dc in [-2048i16, -300, -5, -4, -3, 0, 3, 4, 5, 100, 2047] {
            let mut c = [0i16; 16];
            c[0] = dc;
            let mut a = [100u8; 16];
            let mut b = [100u8; 16];
            idct_add(&c, &mut a, 0, 4);
            idct_dc_add(dc, &mut b, 0, 4);
            assert_eq!(a, b, "dc {dc}");
        }
    }

    #[test]
    fn wht_of_dc_only_spreads_evenly() {
        let mut input = [0i16; 16];
        input[0] = 77;
        let mut out = [0i16; 256];
        inverse_wht(&input, &mut out);
        for i in 0..16 {
            assert_eq!(out[i * 16], (77 + 3) >> 3);
        }
    }
}
