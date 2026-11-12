//! A baseline JPEG encoder for the JPEG layer (viewers that can't decode
//! VP8 or H.264: PRD remote desktop §3.5, Linux virtual desktop §2.4).
//! Pure Rust, no dependency: 4:2:0 baseline JFIF with the standard
//! (Annex K) Huffman tables and quality-scaled quantisation tables, an
//! AAN floating-point DCT, written straight from the I420 planes the
//! capture already makes (no round trip through RGB).
//!
//! It runs as a kessel-video [`EncoderCore`] on a [`ThreadedEncoder`], so
//! the capture engine drives it like its video encoders (submit, poll,
//! busy, wake). Every unit is a key.

use kessel_video::kit::{CoreFrame, EncoderCore, ThreadedEncoder};
use kessel_video::{CodecConfig, ColorSpace, ContentHint, EncoderConfig, FrameBuffer, Implementation, PixelFormat, Rates, VideoEncoder, VideoError, VideoFrame, Wake};

/// The quality a layer gets when it names none.
pub const DEFAULT_QUALITY: u8 = 70;

/// The encoder's name (stats and logs).
pub const NAME: &str = "K2 Screen JPEG (baseline, Rust)";

/// Natural (row-major) index of each zig-zag position.
const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// Annex K.1 luminance quantisation, natural order.
const LUMA_Q: [u16; 64] = [
    16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57, 69, 56, 14, 17, 22, 29, 51, 87, 80, 62, 18, 22, 37, 56, 68, 109, 103, 77, 24, 35, 55, 64, 81, 104, 113, 92, 49, 64, 78, 87, 103, 121, 120, 101, 72, 92, 95, 98, 112, 100, 103, 99,
];

/// Annex K.1 chrominance quantisation, natural order.
const CHROMA_Q: [u16; 64] = [
    17, 18, 24, 47, 99, 99, 99, 99, 18, 21, 26, 66, 99, 99, 99, 99, 24, 26, 56, 99, 99, 99, 99, 99, 47, 66, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
];

// Annex K.3 Huffman tables: code counts for lengths 1-16, then symbols.
const DC_LUMA_BITS: [u8; 16] = [0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0];
const DC_CHROMA_BITS: [u8; 16] = [0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0];
const DC_VALS: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
const AC_LUMA_BITS: [u8; 16] = [0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 0x7d];
const AC_LUMA_VALS: [u8; 162] = [
    0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07, 0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xa1, 0x08, 0x23, 0x42, 0xb1, 0xc1, 0x15, 0x52, 0xd1, 0xf0, 0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0a, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe1, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa,
];
const AC_CHROMA_BITS: [u8; 16] = [0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 0x77];
const AC_CHROMA_VALS: [u8; 162] = [
    0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21, 0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61, 0x71, 0x13, 0x22, 0x32, 0x81, 0x08, 0x14, 0x42, 0x91, 0xa1, 0xb1, 0xc1, 0x09, 0x23, 0x33, 0x52, 0xf0, 0x15, 0x62, 0x72, 0xd1, 0x0a, 0x16, 0x24, 0x34, 0xe1, 0x25, 0xf1, 0x17, 0x18, 0x19, 0x1a, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa,
];

/// cos(k·π/16)·√2 for k ≥ 1, 1 for k = 0: the AAN DCT's output scale.
const AAN: [f32; 8] = [1.0, 1.387_039_8, 1.306_563, 1.175_875_6, 1.0, 0.785_694_96, 0.541_196_1, 0.275_899_38];

/// A Huffman table as (code, length) by symbol.
struct Huff {
    code: [u16; 256],
    len: [u8; 256],
}

impl Huff {
    fn new(bits: &[u8; 16], vals: &[u8]) -> Huff {
        let mut h = Huff { code: [0; 256], len: [0; 256] };
        let mut code = 0u16;
        let mut k = 0;
        for (i, &n) in bits.iter().enumerate() {
            for _ in 0..n {
                h.code[vals[k] as usize] = code;
                h.len[vals[k] as usize] = i as u8 + 1;
                code += 1;
                k += 1;
            }
            code <<= 1;
        }
        h
    }
}

/// Bits out, with 0xFF stuffing.
struct Bits {
    out: Vec<u8>,
    acc: u32,
    n: u32,
}

impl Bits {
    fn put(&mut self, code: u32, len: u32) {
        if len == 0 {
            return;
        }
        self.acc = (self.acc << len) | (code & ((1 << len) - 1));
        self.n += len;
        while self.n >= 8 {
            let b = (self.acc >> (self.n - 8)) as u8;
            self.out.push(b);
            if b == 0xff {
                self.out.push(0);
            }
            self.n -= 8;
        }
        self.acc &= (1 << self.n) - 1;
    }

    fn flush(&mut self) {
        if self.n > 0 {
            let pad = 8 - self.n;
            self.put((1 << pad) - 1, pad);
        }
    }
}

/// The bit count of `v`'s magnitude (JPEG's "size" category).
fn category(v: i32) -> u32 {
    32 - v.unsigned_abs().leading_zeros()
}

/// A quality-scaled table (the IJG formula), natural order.
fn scaled(base: &[u16; 64], quality: u8) -> [u16; 64] {
    let q = quality.clamp(1, 100) as u32;
    let s = if q < 50 { 5000 / q } else { 200 - 2 * q };
    let mut t = [0u16; 64];
    for i in 0..64 {
        t[i] = ((base[i] as u32 * s + 50) / 100).clamp(1, 255) as u16;
    }
    t
}

/// 1 / (q · AAN scale · 8) per natural index: quantising after the DCT.
fn divisors(q: &[u16; 64]) -> [f32; 64] {
    let mut d = [0f32; 64];
    for r in 0..8 {
        for c in 0..8 {
            d[r * 8 + c] = 1.0 / (q[r * 8 + c] as f32 * AAN[r] * AAN[c] * 8.0);
        }
    }
    d
}

/// The AAN forward DCT in place (rows, then columns), unscaled.
fn fdct(d: &mut [f32; 64]) {
    for pass in 0..2 {
        for i in 0..8 {
            let (base, step) = if pass == 0 { (i * 8, 1) } else { (i, 8) };
            let at = |k: usize| base + k * step;
            let t0 = d[at(0)] + d[at(7)];
            let t7 = d[at(0)] - d[at(7)];
            let t1 = d[at(1)] + d[at(6)];
            let t6 = d[at(1)] - d[at(6)];
            let t2 = d[at(2)] + d[at(5)];
            let t5 = d[at(2)] - d[at(5)];
            let t3 = d[at(3)] + d[at(4)];
            let t4 = d[at(3)] - d[at(4)];
            // Even part.
            let t10 = t0 + t3;
            let t13 = t0 - t3;
            let t11 = t1 + t2;
            let t12 = t1 - t2;
            d[at(0)] = t10 + t11;
            d[at(4)] = t10 - t11;
            let z1 = (t12 + t13) * 0.707_106_77;
            d[at(2)] = t13 + z1;
            d[at(6)] = t13 - z1;
            // Odd part.
            let t10 = t4 + t5;
            let t11 = t5 + t6;
            let t12 = t6 + t7;
            let z5 = (t10 - t12) * 0.382_683_43;
            let z2 = 0.541_196_1 * t10 + z5;
            let z4 = 1.306_563 * t12 + z5;
            let z3 = t11 * 0.707_106_77;
            let z11 = t7 + z3;
            let z13 = t7 - z3;
            d[at(5)] = z13 + z2;
            d[at(3)] = z13 - z2;
            d[at(1)] = z11 + z4;
            d[at(7)] = z11 - z4;
        }
    }
}

/// The encoder's fixed state for one quality.
pub struct Jpeg {
    quality: u8,
    q: [[u16; 64]; 2],
    div: [[f32; 64]; 2],
    dc: [Huff; 2],
    ac: [Huff; 2],
}

impl Jpeg {
    pub fn new(quality: u8) -> Jpeg {
        let quality = quality.clamp(1, 100);
        let q = [scaled(&LUMA_Q, quality), scaled(&CHROMA_Q, quality)];
        let div = [divisors(&q[0]), divisors(&q[1])];
        Jpeg {
            quality,
            q,
            div,
            dc: [Huff::new(&DC_LUMA_BITS, &DC_VALS), Huff::new(&DC_CHROMA_BITS, &DC_VALS)],
            ac: [Huff::new(&AC_LUMA_BITS, &AC_LUMA_VALS), Huff::new(&AC_CHROMA_BITS, &AC_CHROMA_VALS)],
        }
    }

    pub fn quality(&self) -> u8 {
        self.quality
    }

    fn headers(&self, out: &mut Vec<u8>, w: u16, h: u16) {
        out.extend_from_slice(&[0xff, 0xd8]);
        // JFIF APP0: version 1.1, no density, no thumbnail.
        out.extend_from_slice(&[0xff, 0xe0, 0, 16, b'J', b'F', b'I', b'F', 0, 1, 1, 0, 0, 1, 0, 1, 0, 0]);
        for (id, q) in self.q.iter().enumerate() {
            out.extend_from_slice(&[0xff, 0xdb, 0, 67, id as u8]);
            out.extend(ZIGZAG.iter().map(|&n| q[n] as u8));
        }
        out.extend_from_slice(&[0xff, 0xc0, 0, 17, 8]);
        out.extend_from_slice(&h.to_be_bytes());
        out.extend_from_slice(&w.to_be_bytes());
        out.extend_from_slice(&[3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
        for (class_id, bits, vals) in [
            (0x00u8, &DC_LUMA_BITS, &DC_VALS[..]),
            (0x10, &AC_LUMA_BITS, &AC_LUMA_VALS[..]),
            (0x01, &DC_CHROMA_BITS, &DC_VALS[..]),
            (0x11, &AC_CHROMA_BITS, &AC_CHROMA_VALS[..]),
        ] {
            let len = 2 + 1 + 16 + vals.len();
            out.extend_from_slice(&[0xff, 0xc4]);
            out.extend_from_slice(&(len as u16).to_be_bytes());
            out.push(class_id);
            out.extend_from_slice(bits);
            out.extend_from_slice(vals);
        }
        out.extend_from_slice(&[0xff, 0xda, 0, 12, 3, 1, 0x00, 2, 0x11, 3, 0x11, 0, 63, 0]);
    }

    fn block(&self, bits: &mut Bits, px: &mut [f32; 64], table: usize, prev_dc: &mut i32) {
        fdct(px);
        let div = &self.div[table];
        let mut zz = [0i32; 64];
        for (k, &n) in ZIGZAG.iter().enumerate() {
            // Baseline holds AC values in 10 bits (DC differences in 11).
            zz[k] = ((px[n] * div[n]).round() as i32).clamp(-1023, 1023);
        }
        let (dc, ac) = (&self.dc[table], &self.ac[table]);
        let diff = (zz[0] - *prev_dc).clamp(-2047, 2047);
        *prev_dc += diff;
        let cat = category(diff);
        bits.put(dc.code[cat as usize] as u32, dc.len[cat as usize] as u32);
        bits.put(if diff < 0 { (diff - 1) as u32 } else { diff as u32 }, cat);
        let mut run = 0u32;
        for &v in &zz[1..] {
            if v == 0 {
                run += 1;
                continue;
            }
            while run >= 16 {
                bits.put(ac.code[0xf0] as u32, ac.len[0xf0] as u32);
                run -= 16;
            }
            let cat = category(v);
            let sym = ((run << 4) | cat) as usize;
            bits.put(ac.code[sym] as u32, ac.len[sym] as u32);
            bits.put(if v < 0 { (v - 1) as u32 } else { v as u32 }, cat);
            run = 0;
        }
        if run > 0 {
            bits.put(ac.code[0] as u32, ac.len[0] as u32);
        }
    }

    /// One JPEG of a `w` x `h` I420 picture (planes and strides as given,
    /// in `color`). Edges are padded by repeating the last row and column.
    #[allow(clippy::too_many_arguments)]
    pub fn encode_i420(&self, y: &[u8], ys: usize, u: &[u8], us: usize, v: &[u8], vs: usize, w: u32, h: u32, color: ColorSpace) -> Vec<u8> {
        // JFIF is full range: stretch limited-range samples.
        let full = color == ColorSpace::Bt709Full;
        let ymap: [f32; 256] = std::array::from_fn(|i| if full { i as f32 } else { ((i as f32 - 16.0) * 255.0 / 219.0).clamp(0.0, 255.0) } - 128.0);
        let cmap: [f32; 256] = std::array::from_fn(|i| if full { i as f32 - 128.0 } else { ((i as f32 - 128.0) * 255.0 / 224.0).clamp(-128.0, 127.0) });
        let (w, h) = (w.max(1) as usize, h.max(1) as usize);
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let mut out = Vec::with_capacity(w * h / 4 + 1024);
        self.headers(&mut out, w as u16, h as u16);
        let mut bits = Bits { out, acc: 0, n: 0 };
        let mut dc = [0i32; 3];
        let mut px = [0f32; 64];
        for my in 0..h.div_ceil(16) {
            for mx in 0..w.div_ceil(16) {
                for (by, bx) in [(0, 0), (0, 8), (8, 0), (8, 8)] {
                    for r in 0..8 {
                        let row = &y[(my * 16 + by + r).min(h - 1) * ys..];
                        for c in 0..8 {
                            px[r * 8 + c] = ymap[row[(mx * 16 + bx + c).min(w - 1)] as usize];
                        }
                    }
                    self.block(&mut bits, &mut px, 0, &mut dc[0]);
                }
                for (plane, stride, slot) in [(u, us, 1), (v, vs, 2)] {
                    for r in 0..8 {
                        let row = &plane[(my * 8 + r).min(ch - 1) * stride..];
                        for c in 0..8 {
                            px[r * 8 + c] = cmap[row[(mx * 8 + c).min(cw - 1)] as usize];
                        }
                    }
                    self.block(&mut bits, &mut px, 1, &mut dc[slot]);
                }
            }
        }
        bits.flush();
        let mut out = bits.out;
        out.extend_from_slice(&[0xff, 0xd9]);
        out
    }
}

/// The JPEG layer's [`EncoderCore`]: I420 in, one JPEG out per frame.
pub struct JpegCore {
    jpeg: Jpeg,
}

impl EncoderCore for JpegCore {
    fn input_format(&self) -> PixelFormat {
        PixelFormat::I420
    }

    fn encode(&mut self, frame: &VideoFrame, _keyframe: bool) -> Result<Option<CoreFrame>, VideoError> {
        let FrameBuffer::Planes(p) = &frame.buffer else {
            return Err(VideoError::Invalid("the JPEG encoder takes CPU planes".into()));
        };
        if p.format != PixelFormat::I420 || frame.width > u16::MAX as u32 || frame.height > u16::MAX as u32 {
            return Err(VideoError::Invalid(format!("a {:?} {}x{} picture", p.format, frame.width, frame.height)));
        }
        let plane = |i: usize| &p.data[p.offsets[i]..];
        let s = |i: usize| p.strides[i] as usize;
        let data = self.jpeg.encode_i420(plane(0), s(0), plane(1), s(1), plane(2), s(2), frame.width, frame.height, frame.color);
        Ok(Some(CoreFrame::new(data, true)))
    }

    fn set_rates(&mut self, _rates: Rates) -> Result<(), VideoError> {
        Ok(())
    }

    fn set_size(&mut self, _width: u32, _height: u32) -> Result<(), VideoError> {
        Ok(())
    }

    fn implementation(&self) -> Implementation {
        Implementation::Software
    }

    fn name(&self) -> String {
        format!("{NAME}, quality {}", self.jpeg.quality())
    }
}

/// A JPEG "encoder" for a layer of `width` x `height` at `fps`: the
/// engine treats it as any other. Its outputs say VP8 in their codec field
/// (kessel-video's threaded encoder knows only video codecs); the engine
/// labels units by the layer's codec, never by that field.
pub fn encoder(width: u32, height: u32, fps: u32, quality: u8, wake: Wake) -> Result<Box<dyn VideoEncoder>, VideoError> {
    let mut cfg = EncoderConfig::new(CodecConfig::Vp8, width, height, fps.clamp(1, 240), 1_000_000);
    cfg.content = ContentHint::Screen;
    cfg.validate()?;
    let core = JpegCore { jpeg: Jpeg::new(quality) };
    Ok(Box::new(ThreadedEncoder::start(Box::new(core), cfg, wake)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kessel_video::{convert, Planes};

    #[test]
    fn the_standard_tables_hold_every_symbol_once() {
        for (bits, vals) in [(&AC_LUMA_BITS, &AC_LUMA_VALS), (&AC_CHROMA_BITS, &AC_CHROMA_VALS)] {
            assert_eq!(bits.iter().map(|&b| b as usize).sum::<usize>(), vals.len());
            let mut seen = std::collections::HashSet::new();
            for &v in vals.iter() {
                assert!(seen.insert(v), "symbol {v:#x} twice");
                assert!((1..=10).contains(&(v & 15)) || v == 0 || v == 0xf0, "{v:#x}");
            }
            assert_eq!(seen.len(), 162, "every run/size pair, EOB and ZRL");
        }
        for bits in [&DC_LUMA_BITS, &DC_CHROMA_BITS] {
            assert_eq!(bits.iter().map(|&b| b as usize).sum::<usize>(), DC_VALS.len());
        }
        let mut zz = ZIGZAG.to_vec();
        zz.sort();
        assert_eq!(zz, (0..64).collect::<Vec<_>>());
    }

    #[test]
    fn categories_and_quality_scaling() {
        assert_eq!((category(0), category(1), category(-1), category(2), category(-3), category(1023)), (0, 1, 1, 2, 2, 10));
        assert_eq!(scaled(&LUMA_Q, 50), LUMA_Q, "quality 50 is the standard table");
        assert!(scaled(&LUMA_Q, 100).iter().all(|&q| q == 1));
        assert!(scaled(&LUMA_Q, 1).iter().all(|&q| q == 255));
    }

    /// A picture with edges, gradients and a size that isn't a multiple of
    /// 16, through kessel-video's BGRA → I420 conversion and this encoder,
    /// decodes (with the `image` crate's decoder) to what went in.
    fn round_trip(w: u32, h: u32, quality: u8) -> (f64, usize) {
        let mut bgra = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let at = ((y * w + x) * 4) as usize;
                let checker = ((x / 24) + (y / 24)) % 2 == 0;
                bgra[at] = (x * 255 / w) as u8;
                bgra[at + 1] = if checker { 220 } else { 40 };
                bgra[at + 2] = (y * 255 / h) as u8;
                bgra[at + 3] = 255;
            }
        }
        let f = VideoFrame::from_planes(Planes::tight(PixelFormat::Bgra, w, h, bgra.clone()).unwrap(), w, h, 0).unwrap();
        let i420 = convert(&f, w, h, PixelFormat::I420).unwrap();
        let mut core = JpegCore { jpeg: Jpeg::new(quality) };
        let out = core.encode(&i420, true).unwrap().unwrap();
        assert!(out.keyframe);
        let data = out.data.to_vec();
        assert_eq!(&data[..2], &[0xff, 0xd8]);
        assert_eq!(&data[data.len() - 2..], &[0xff, 0xd9]);
        let img = image::load_from_memory_with_format(&data, image::ImageFormat::Jpeg).expect("decodes").to_rgb8();
        assert_eq!((img.width(), img.height()), (w, h));
        let mut se = 0f64;
        for y in 0..h {
            for x in 0..w {
                let p = img.get_pixel(x, y).0;
                let at = ((y * w + x) * 4) as usize;
                for (c, want) in [(0, bgra[at + 2]), (1, bgra[at + 1]), (2, bgra[at])] {
                    let d = p[c] as f64 - want as f64;
                    se += d * d;
                }
            }
        }
        let mse = se / (w * h * 3) as f64;
        (10.0 * (255.0f64 * 255.0 / mse.max(1e-9)).log10(), data.len())
    }

    #[test]
    fn a_picture_round_trips_through_a_standard_decoder() {
        let (psnr, bytes) = round_trip(203, 117, 90);
        eprintln!("[jpeg] 203x117 q90: {psnr:.1} dB, {bytes} bytes");
        assert!(psnr > 28.0, "PSNR {psnr:.1} dB at quality 90");
        let (psnr_lo, bytes_lo) = round_trip(203, 117, 30);
        eprintln!("[jpeg] 203x117 q30: {psnr_lo:.1} dB, {bytes_lo} bytes");
        assert!(psnr_lo > 22.0 && psnr_lo < psnr, "PSNR {psnr_lo:.1} dB at quality 30");
        assert!(bytes_lo < bytes, "lower quality, fewer bytes ({bytes_lo} vs {bytes})");
        // Even and tiny sizes too.
        assert!(round_trip(64, 32, 70).0 > 25.0);
        assert!(round_trip(1, 1, 70).0 > 20.0);
    }

    #[test]
    fn the_encoder_runs_on_its_thread_and_every_unit_is_a_key() {
        let mut e = encoder(64, 48, 15, 70, kessel_video::no_wake()).unwrap();
        let f = kessel_video::fake::pattern_frame(64, 48, 1, 1234).unwrap();
        e.submit(&f, kessel_video::EncodeOptions::default()).unwrap();
        let out = kessel_video::kit::wait_for(std::time::Duration::from_secs(5), || e.poll()).expect("an output").unwrap();
        assert!(out.keyframe);
        assert_eq!((out.width, out.height, out.capture_us), (64, 48, 1234));
        assert!(out.data.starts_with(&[0xff, 0xd8]));
        assert!(e.info().name.starts_with(NAME));
    }
}
