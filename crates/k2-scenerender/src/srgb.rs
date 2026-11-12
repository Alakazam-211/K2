//! sRGB transfer, as fixed tables: the render path never calls `powf`, so
//! the output is the same whatever the platform's libm (an aarch64 Mac, an x86
//! Linux box, a different libc).
//!
//! **How the tables are made** (the sRGB curve, IEC 61966-2-1:
//! `decode(s) = s / 12.92` for `s <= 0.04045`, else `((s + 0.055) / 1.055)^2.4`),
//! computed in f64:
//!
//! - [`ENCODE_EDGES`]: for each sRGB code boundary `k + 0.5` (k = 0..254), the
//!   smallest 16-bit linear level `i` with `i / 65535 >= decode((k + 0.5) / 255)`,
//!   that is `ceil(decode((k + 0.5) / 255) * 65535)`. A linear level `i`
//!   encodes to the number of edges `<= i` (round to the nearest code, in
//!   linear light). The nearest any edge comes to a whole number is 1.4e-4,
//!   so a last-bit difference in a libm can't move one.
//! - [`DECODE_BITS`]: `decode(c / 255)` for each code `c`, rounded to f32 (its
//!   bits), for the background colour.
//!
//! `tests::the_tables_regenerate_in_f64` computes both again with the
//! platform's `powf` and compares, and checks the margins.

/// `ceil(decode((k + 0.5) / 255) * 65535)` for k = 0..254.
pub const ENCODE_EDGES: [u16; 255] = [
    10, 30, 50, 70, 90, 110, 130, 150, 170, 189, 209, 230, 253, 276, 301, 327,
    354, 382, 412, 443, 475, 509, 544, 580, 618, 657, 698, 740, 783, 828, 875, 923,
    972, 1023, 1075, 1129, 1185, 1242, 1300, 1360, 1422, 1486, 1551, 1617, 1685, 1755, 1827, 1900,
    1975, 2052, 2130, 2210, 2292, 2376, 2461, 2548, 2637, 2727, 2820, 2914, 3010, 3108, 3208, 3309,
    3412, 3518, 3625, 3734, 3844, 3957, 4072, 4188, 4307, 4427, 4550, 4674, 4800, 4928, 5059, 5191,
    5325, 5461, 5599, 5740, 5882, 6026, 6173, 6321, 6471, 6624, 6778, 6935, 7094, 7255, 7418, 7583,
    7750, 7919, 8091, 8265, 8440, 8618, 8798, 8981, 9165, 9352, 9541, 9732, 9925, 10121, 10318, 10518,
    10720, 10925, 11132, 11341, 11552, 11765, 11981, 12199, 12420, 12643, 12868, 13095, 13325, 13557, 13791, 14028,
    14267, 14508, 14752, 14998, 15247, 15498, 15751, 16007, 16265, 16525, 16788, 17054, 17321, 17592, 17864, 18139,
    18417, 18697, 18980, 19264, 19552, 19842, 20134, 20429, 20727, 21027, 21329, 21634, 21942, 22252, 22564, 22880,
    23197, 23518, 23840, 24166, 24494, 24824, 25158, 25493, 25832, 26173, 26516, 26862, 27211, 27563, 27917, 28273,
    28633, 28995, 29359, 29727, 30097, 30469, 30845, 31223, 31603, 31987, 32373, 32762, 33153, 33547, 33944, 34344,
    34747, 35152, 35560, 35970, 36384, 36800, 37219, 37640, 38065, 38492, 38922, 39355, 39790, 40229, 40670, 41114,
    41561, 42011, 42463, 42918, 43377, 43838, 44301, 44768, 45238, 45710, 46185, 46663, 47144, 47628, 48115, 48605,
    49097, 49593, 50091, 50592, 51096, 51604, 52114, 52627, 53142, 53661, 54183, 54708, 55235, 55766, 56300, 56836,
    57376, 57918, 58464, 59012, 59564, 60118, 60675, 61236, 61799, 62366, 62935, 63508, 64083, 64662, 65244,
];

/// `decode(c / 255)` as f32 bits, c = 0..255.
pub const DECODE_BITS: [u32; 256] = [
    0x00000000, 0x399f22b4, 0x3a1f22b4, 0x3a6eb40e, 0x3a9f22b4, 0x3ac6eb61, 0x3aeeb40e, 0x3b0b3e5d,
    0x3b1f22b4, 0x3b33070a, 0x3b46eb61, 0x3b5b518e, 0x3b70f18f, 0x3b83e1c6, 0x3b8fe616, 0x3b9c87fd,
    0x3ba9c9b6, 0x3bb7ad6f, 0x3bc6354a, 0x3bd56360, 0x3be539c1, 0x3bf5ba71, 0x3c0373b6, 0x3c0c6153,
    0x3c15a705, 0x3c1f45be, 0x3c293e6b, 0x3c3391f7, 0x3c3e4149, 0x3c494d44, 0x3c54b6c9, 0x3c607eb4,
    0x3c6ca5df, 0x3c792d22, 0x3c830aa9, 0x3c89af9f, 0x3c9085dc, 0x3c978dc6, 0x3c9ec7c2, 0x3ca63433,
    0x3cadd37d, 0x3cb5a602, 0x3cbdac21, 0x3cc5e63a, 0x3cce54ac, 0x3cd6f7d5, 0x3cdfd010, 0x3ce8ddba,
    0x3cf2212d, 0x3cfb9ac3, 0x3d02a56a, 0x3d0798dd, 0x3d0ca7e6, 0x3d11d2af, 0x3d171964, 0x3d1c7c30,
    0x3d21fb3c, 0x3d2796b2, 0x3d2d4ebb, 0x3d332381, 0x3d39152b, 0x3d3f23e4, 0x3d454fd2, 0x3d4b991d,
    0x3d51ffec, 0x3d588468, 0x3d5f26b6, 0x3d65e6fd, 0x3d6cc563, 0x3d73c20e, 0x3d7add24, 0x3d810b65,
    0x3d84b793, 0x3d88732e, 0x3d8c3e48, 0x3d9018f4, 0x3d940344, 0x3d97fd49, 0x3d9c0715, 0x3da020ba,
    0x3da44a4a, 0x3da883d6, 0x3daccd6f, 0x3db12727, 0x3db5910f, 0x3dba0b38, 0x3dbe95b3, 0x3dc33090,
    0x3dc7dbe0, 0x3dcc97b4, 0x3dd1641d, 0x3dd6412b, 0x3ddb2eee, 0x3de02d76, 0x3de53cd4, 0x3dea5d18,
    0x3def8e51, 0x3df4d090, 0x3dfa23e5, 0x3dff885e, 0x3e027f06, 0x3e05427f, 0x3e080ea2, 0x3e0ae377,
    0x3e0dc104, 0x3e10a753, 0x3e13966a, 0x3e168e51, 0x3e198f0f, 0x3e1c98ac, 0x3e1fab30, 0x3e22c6a1,
    0x3e25eb07, 0x3e29186a, 0x3e2c4ed0, 0x3e2f8e42, 0x3e32d6c5, 0x3e362862, 0x3e39831f, 0x3e3ce703,
    0x3e405417, 0x3e43ca60, 0x3e4749e6, 0x3e4ad2af, 0x3e4e64c3, 0x3e520029, 0x3e55a4e7, 0x3e595305,
    0x3e5d0a89, 0x3e60cb7a, 0x3e6495df, 0x3e6869be, 0x3e6c471f, 0x3e702e07, 0x3e741e7e, 0x3e78188b,
    0x3e7c1c33, 0x3e8014bf, 0x3e822039, 0x3e84308b, 0x3e8645b8, 0x3e885fc3, 0x3e8a7eb0, 0x3e8ca281,
    0x3e8ecb3b, 0x3e90f8df, 0x3e932b72, 0x3e9562f6, 0x3e979f6f, 0x3e99e0e0, 0x3e9c274c, 0x3e9e72b6,
    0x3ea0c321, 0x3ea31890, 0x3ea57307, 0x3ea7d288, 0x3eaa3716, 0x3eaca0b6, 0x3eaf0f68, 0x3eb18332,
    0x3eb3fc15, 0x3eb67a14, 0x3eb8fd34, 0x3ebb8576, 0x3ebe12de, 0x3ec0a56e, 0x3ec33d2a, 0x3ec5da14,
    0x3ec87c30, 0x3ecb2380, 0x3ecdd008, 0x3ed081ca, 0x3ed338c9, 0x3ed5f508, 0x3ed8b68a, 0x3edb7d52,
    0x3ede4963, 0x3ee11abf, 0x3ee3f169, 0x3ee6cd65, 0x3ee9aeb5, 0x3eec955b, 0x3eef815c, 0x3ef272b8,
    0x3ef56974, 0x3ef86593, 0x3efb6716, 0x3efe6e00, 0x3f00bd2b, 0x3f02460c, 0x3f03d1a5, 0x3f055ff7,
    0x3f06f104, 0x3f0884cd, 0x3f0a1b54, 0x3f0bb499, 0x3f0d509f, 0x3f0eef65, 0x3f1090ef, 0x3f12353d,
    0x3f13dc50, 0x3f15862a, 0x3f1732cc, 0x3f18e237, 0x3f1a946e, 0x3f1c4970, 0x3f1e0140, 0x3f1fbbde,
    0x3f21794d, 0x3f23398c, 0x3f24fc9f, 0x3f26c285, 0x3f288b41, 0x3f2a56d2, 0x3f2c253c, 0x3f2df67f,
    0x3f2fca9c, 0x3f31a194, 0x3f337b6a, 0x3f35581d, 0x3f3737b0, 0x3f391a24, 0x3f3aff7a, 0x3f3ce7b2,
    0x3f3ed2cf, 0x3f40c0d2, 0x3f42b1bc, 0x3f44a58e, 0x3f469c49, 0x3f4895ef, 0x3f4a9280, 0x3f4c91ff,
    0x3f4e946c, 0x3f5099c9, 0x3f52a216, 0x3f54ad56, 0x3f56bb88, 0x3f58ccaf, 0x3f5ae0cc, 0x3f5cf7df,
    0x3f5f11ea, 0x3f612eef, 0x3f634eee, 0x3f6571e9, 0x3f6797e0, 0x3f69c0d5, 0x3f6becca, 0x3f6e1bbf,
    0x3f704db5, 0x3f7282ae, 0x3f74baab, 0x3f76f5ae, 0x3f7933b6, 0x3f7b74c6, 0x3f7db8de, 0x3f800000,
];

/// Linear light (f32) to a 16-bit level, clamped: the index into [`encode_table`].
#[inline]
pub fn q16(v: f32) -> usize {
    let v = if v > 1.0 {
        1.0
    } else if v > 0.0 {
        v
    } else {
        0.0
    };
    (v * 65535.0 + 0.5) as usize
}

/// The 65,536-entry table from a 16-bit linear level to its sRGB code.
pub fn encode_table() -> Vec<u8> {
    let mut lut = vec![0u8; 65536];
    let mut k = 0usize;
    for (i, l) in lut.iter_mut().enumerate() {
        while k < 255 && (ENCODE_EDGES[k] as usize) <= i {
            k += 1;
        }
        *l = k as u8;
    }
    lut
}

/// An sRGB code to linear light.
#[inline]
pub fn to_linear(c: u8) -> f32 {
    f32::from_bits(DECODE_BITS[c as usize])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(s: f64) -> f64 {
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    }

    #[test]
    fn the_tables_regenerate_in_f64() {
        let mut nearest = f64::MAX;
        for (k, &edge) in ENCODE_EDGES.iter().enumerate() {
            let x = decode((k as f64 + 0.5) / 255.0) * 65535.0;
            assert_eq!(x.ceil() as u16, edge, "edge {k}");
            nearest = nearest.min((x - x.round()).abs());
        }
        assert!(nearest > 1e-5, "an edge lies {nearest} from a whole level: a libm could move it");
        for (c, &bits) in DECODE_BITS.iter().enumerate() {
            let v = decode(c as f64 / 255.0);
            assert_eq!((v as f32).to_bits(), bits, "code {c}");
            // Not near a rounding tie between two f32 values.
            let f = v as f32;
            let ulp = (f64::from(f32::from_bits(f.to_bits() + 1)) - f64::from(f)).abs();
            if c > 0 {
                assert!(((v - f64::from(f)).abs() - ulp / 2.0).abs() > ulp * 1e-6, "code {c} is at a tie");
            }
        }
    }

    #[test]
    fn every_code_round_trips_and_the_table_is_monotonic() {
        let lut = encode_table();
        for c in 0..=255u8 {
            assert_eq!(lut[q16(to_linear(c))], c, "code {c}");
        }
        assert!(lut.windows(2).all(|w| w[0] <= w[1]));
        assert_eq!((lut[0], lut[65535]), (0, 255));
    }
}
