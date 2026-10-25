//! A small QR Code encoder (byte mode, error correction level M, versions
//! 1–10, so up to 213 bytes), for the phone pairing link
//! (phone-web-off): the daemon returns the module matrix and every client
//! (the `k2` CLI, Settings) only draws it, so there is one encoder.
//!
//! Written for K2 from ISO/IEC 18004 and the structure of Project Nayuki's
//! public QR Code generator (MIT); no code is copied. Each row is a string
//! of `'1'` (dark) and `'0'` (light), without the 4-module quiet zone the
//! renderer adds.

/// Error-correction codewords per block, level M, by version (index 0 unused).
const ECC_PER_BLOCK_M: [usize; 11] = [0, 10, 16, 26, 18, 24, 16, 18, 22, 22, 26];
/// Error-correction blocks, level M, by version.
const BLOCKS_M: [usize; 11] = [0, 1, 1, 1, 2, 2, 4, 4, 4, 5, 5];
/// The highest version this encoder makes.
pub const MAX_VERSION: usize = 10;
/// Level M's two format bits.
const ECL_M_BITS: u32 = 0;

/// The QR matrix for `text`, or `None` when it is longer than version 10-M
/// holds.
pub fn encode(text: &str) -> Option<Vec<String>> {
    let data = text.as_bytes();
    let version = (1..=MAX_VERSION).find(|&v| {
        let cc_bits = if v <= 9 { 8 } else { 16 };
        data.len() < (1usize << cc_bits) && 4 + cc_bits + 8 * data.len() <= data_codewords(v) * 8
    })?;
    let codewords = add_ecc_and_interleave(version, &data_bits(version, data));
    let mut q = Matrix::new(version);
    q.draw_function_patterns();
    q.draw_codewords(&codewords);
    let mut best: Option<(u32, Vec<Vec<bool>>)> = None;
    for mask in 0..8u8 {
        let mut trial = q.clone();
        trial.apply_mask(mask);
        trial.draw_format_bits(mask);
        let score = trial.penalty();
        if best.as_ref().is_none_or(|(s, _)| score < *s) {
            best = Some((score, trial.modules));
        }
    }
    let (_, modules) = best?;
    Some(modules.iter().map(|row| row.iter().map(|&d| if d { '1' } else { '0' }).collect()).collect())
}

fn raw_data_modules(ver: usize) -> usize {
    let v = ver as isize;
    let mut result = (16 * v + 128) * v + 64;
    if ver >= 2 {
        let num_align = v / 7 + 2;
        result -= (25 * num_align - 10) * num_align - 55;
        if ver >= 7 {
            result -= 36;
        }
    }
    result as usize
}

fn data_codewords(ver: usize) -> usize {
    raw_data_modules(ver) / 8 - ECC_PER_BLOCK_M[ver] * BLOCKS_M[ver]
}

fn push_bits(buf: &mut Vec<bool>, val: u32, len: usize) {
    for i in (0..len).rev() {
        buf.push((val >> i) & 1 != 0);
    }
}

/// Mode indicator, count, bytes, terminator and pad bytes, as codewords.
fn data_bits(ver: usize, data: &[u8]) -> Vec<u8> {
    let cc_bits = if ver <= 9 { 8 } else { 16 };
    let capacity = data_codewords(ver) * 8;
    let mut bits = Vec::with_capacity(capacity);
    push_bits(&mut bits, 0b0100, 4);
    push_bits(&mut bits, data.len() as u32, cc_bits);
    for &b in data {
        push_bits(&mut bits, b as u32, 8);
    }
    let term = (capacity - bits.len()).min(4);
    push_bits(&mut bits, 0, term);
    let pad = (8 - bits.len() % 8) % 8;
    push_bits(&mut bits, 0, pad);
    let mut pad_byte = 0xECu32;
    while bits.len() < capacity {
        push_bits(&mut bits, pad_byte, 8);
        pad_byte ^= 0xEC ^ 0x11;
    }
    bits.chunks(8).map(|c| c.iter().fold(0u8, |acc, &b| (acc << 1) | b as u8)).collect()
}

fn gf_mul(x: u8, y: u8) -> u8 {
    let mut z: u8 = 0;
    for i in (0..8).rev() {
        z = (z << 1) ^ ((z >> 7) * 0x1D);
        z ^= ((y >> i) & 1) * x;
    }
    z
}

fn rs_divisor(degree: usize) -> Vec<u8> {
    let mut result = vec![0u8; degree - 1];
    result.push(1);
    let mut root: u8 = 1;
    for _ in 0..degree {
        for j in 0..degree {
            result[j] = gf_mul(result[j], root);
            if j + 1 < result.len() {
                result[j] ^= result[j + 1];
            }
        }
        root = gf_mul(root, 0x02);
    }
    result
}

fn rs_remainder(data: &[u8], divisor: &[u8]) -> Vec<u8> {
    let mut result = vec![0u8; divisor.len()];
    for &b in data {
        let factor = b ^ result.remove(0);
        result.push(0);
        for (x, &y) in result.iter_mut().zip(divisor) {
            *x ^= gf_mul(y, factor);
        }
    }
    result
}

fn add_ecc_and_interleave(ver: usize, data: &[u8]) -> Vec<u8> {
    let num_blocks = BLOCKS_M[ver];
    let ecc_len = ECC_PER_BLOCK_M[ver];
    let raw = raw_data_modules(ver) / 8;
    let num_short = num_blocks - raw % num_blocks;
    let short_len = raw / num_blocks;
    let divisor = rs_divisor(ecc_len);
    let mut blocks: Vec<Vec<u8>> = Vec::with_capacity(num_blocks);
    let mut k = 0;
    for i in 0..num_blocks {
        let dat_len = short_len - ecc_len + usize::from(i >= num_short);
        let mut block = data[k..k + dat_len].to_vec();
        k += dat_len;
        let ecc = rs_remainder(&block, &divisor);
        if i < num_short {
            block.push(0);
        }
        block.extend_from_slice(&ecc);
        blocks.push(block);
    }
    let mut result = Vec::with_capacity(raw);
    for i in 0..blocks[0].len() {
        for (j, block) in blocks.iter().enumerate() {
            if i != short_len - ecc_len || j >= num_short {
                result.push(block[i]);
            }
        }
    }
    result
}

#[derive(Clone)]
struct Matrix {
    version: usize,
    size: usize,
    modules: Vec<Vec<bool>>,
    function: Vec<Vec<bool>>,
}

impl Matrix {
    fn new(version: usize) -> Self {
        let size = version * 4 + 17;
        Matrix { version, size, modules: vec![vec![false; size]; size], function: vec![vec![false; size]; size] }
    }

    fn set_function(&mut self, x: usize, y: usize, dark: bool) {
        self.modules[y][x] = dark;
        self.function[y][x] = true;
    }

    fn alignment_positions(&self) -> Vec<usize> {
        if self.version == 1 {
            return Vec::new();
        }
        let num = self.version / 7 + 2;
        let step = (self.version * 4 + num * 2 + 1) / (num * 2 - 2) * 2;
        let mut result: Vec<usize> = (0..num - 1).map(|i| self.size - 7 - i * step).collect();
        result.push(6);
        result.reverse();
        result
    }

    fn draw_finder(&mut self, cx: isize, cy: isize) {
        for dy in -4isize..=4 {
            for dx in -4isize..=4 {
                let (x, y) = (cx + dx, cy + dy);
                if x >= 0 && y >= 0 && (x as usize) < self.size && (y as usize) < self.size {
                    let dist = dx.abs().max(dy.abs());
                    self.set_function(x as usize, y as usize, dist != 2 && dist != 4);
                }
            }
        }
    }

    fn draw_function_patterns(&mut self) {
        for i in 0..self.size {
            self.set_function(6, i, i % 2 == 0);
            self.set_function(i, 6, i % 2 == 0);
        }
        let s = self.size as isize;
        self.draw_finder(3, 3);
        self.draw_finder(s - 4, 3);
        self.draw_finder(3, s - 4);
        let pos = self.alignment_positions();
        let n = pos.len();
        for i in 0..n {
            for j in 0..n {
                if (i == 0 && j == 0) || (i == 0 && j == n - 1) || (i == n - 1 && j == 0) {
                    continue;
                }
                for dy in -2isize..=2 {
                    for dx in -2isize..=2 {
                        let x = (pos[i] as isize + dx) as usize;
                        let y = (pos[j] as isize + dy) as usize;
                        self.set_function(x, y, dx.abs().max(dy.abs()) != 1);
                    }
                }
            }
        }
        self.draw_format_bits(0);
        self.draw_version();
    }

    fn draw_format_bits(&mut self, mask: u8) {
        let data = (ECL_M_BITS << 3) | mask as u32;
        let mut rem = data;
        for _ in 0..10 {
            rem = (rem << 1) ^ ((rem >> 9) * 0x537);
        }
        let bits = ((data << 10) | rem) ^ 0x5412;
        let bit = |i: usize| (bits >> i) & 1 != 0;
        for i in 0..6 {
            self.set_function(8, i, bit(i));
        }
        self.set_function(8, 7, bit(6));
        self.set_function(8, 8, bit(7));
        self.set_function(7, 8, bit(8));
        for i in 9..15 {
            self.set_function(14 - i, 8, bit(i));
        }
        let size = self.size;
        for i in 0..8 {
            self.set_function(size - 1 - i, 8, bit(i));
        }
        for i in 8..15 {
            self.set_function(8, size - 15 + i, bit(i));
        }
        self.set_function(8, size - 8, true);
    }

    fn draw_version(&mut self) {
        if self.version < 7 {
            return;
        }
        let mut rem = self.version as u32;
        for _ in 0..12 {
            rem = (rem << 1) ^ ((rem >> 11) * 0x1F25);
        }
        let bits = ((self.version as u32) << 12) | rem;
        for i in 0..18 {
            let dark = (bits >> i) & 1 != 0;
            let a = self.size - 11 + i % 3;
            let b = i / 3;
            self.set_function(a, b, dark);
            self.set_function(b, a, dark);
        }
    }

    fn draw_codewords(&mut self, data: &[u8]) {
        let mut i = 0usize;
        let mut right = self.size as isize - 1;
        while right >= 1 {
            if right == 6 {
                right = 5;
            }
            for vert in 0..self.size {
                for j in 0..2 {
                    let x = (right - j) as usize;
                    let upward = ((right + 1) & 2) == 0;
                    let y = if upward { self.size - 1 - vert } else { vert };
                    if !self.function[y][x] && i < data.len() * 8 {
                        self.modules[y][x] = (data[i >> 3] >> (7 - (i & 7))) & 1 != 0;
                        i += 1;
                    }
                }
            }
            right -= 2;
        }
    }

    fn apply_mask(&mut self, mask: u8) {
        for y in 0..self.size {
            for x in 0..self.size {
                let invert = match mask {
                    0 => (x + y) % 2 == 0,
                    1 => y % 2 == 0,
                    2 => x % 3 == 0,
                    3 => (x + y) % 3 == 0,
                    4 => (x / 3 + y / 2) % 2 == 0,
                    5 => x * y % 2 + x * y % 3 == 0,
                    6 => (x * y % 2 + x * y % 3) % 2 == 0,
                    _ => ((x + y) % 2 + x * y % 3) % 2 == 0,
                };
                if invert && !self.function[y][x] {
                    self.modules[y][x] ^= true;
                }
            }
        }
    }

    /// The four standard penalty rules (runs, 2×2 blocks, finder-like
    /// patterns, dark balance). Any mask decodes; this only picks the
    /// clearest one.
    fn penalty(&self) -> u32 {
        let n = self.size;
        let m = &self.modules;
        let mut score = 0u32;
        let line = |get: &dyn Fn(usize) -> bool| -> u32 {
            let mut s = 0u32;
            let mut run = 1usize;
            for i in 1..n {
                if get(i) == get(i - 1) {
                    run += 1;
                } else {
                    if run >= 5 {
                        s += 3 + (run - 5) as u32;
                    }
                    run = 1;
                }
            }
            if run >= 5 {
                s += 3 + (run - 5) as u32;
            }
            let finder = [true, false, true, true, true, false, true];
            for i in 0..n.saturating_sub(6) {
                if (0..7).all(|k| get(i + k) == finder[k]) {
                    let light_before = (1..=4).all(|k| i < k || !get(i - k));
                    let light_after = (7..11).all(|k| i + k >= n || !get(i + k));
                    if light_before || light_after {
                        s += 40;
                    }
                }
            }
            s
        };
        for y in 0..n {
            score += line(&|x| m[y][x]);
        }
        for x in 0..n {
            score += line(&|y| m[y][x]);
        }
        for y in 0..n - 1 {
            for x in 0..n - 1 {
                let c = m[y][x];
                if c == m[y][x + 1] && c == m[y + 1][x] && c == m[y + 1][x + 1] {
                    score += 3;
                }
            }
        }
        let dark = m.iter().flatten().filter(|&&d| d).count();
        let total = n * n;
        let k = ((dark * 20).abs_diff(total * 10)).div_ceil(total).saturating_sub(1);
        score += k as u32 * 10;
        score
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacities_match_the_standard_for_level_m() {
        // ISO/IEC 18004 Table 7, byte mode, level M.
        let want = [14, 26, 42, 62, 84, 106, 122, 152, 180, 213];
        for (i, cap) in want.iter().enumerate() {
            let v = i + 1;
            let cc = if v <= 9 { 8 } else { 16 };
            assert_eq!((data_codewords(v) * 8 - 4 - cc) / 8, *cap, "version {v}");
        }
    }

    #[test]
    fn reed_solomon_matches_a_known_vector() {
        // Version 1-M "HELLO WORLD" example (alphanumeric data codewords from
        // the standard's worked example) and its 10 published EC codewords.
        let data = [32u8, 91, 11, 120, 209, 114, 220, 77, 67, 64, 236, 17, 236, 17, 236, 17];
        let ecc = rs_remainder(&data, &rs_divisor(10));
        assert_eq!(ecc, vec![196, 35, 39, 119, 235, 215, 231, 226, 93, 23]);
    }

    #[test]
    fn a_pairing_link_encodes_to_a_square_with_finders_and_timing() {
        let link = "https://rosson.k2.dev/pair#K2P-01234-56789-ABCDE-FGHJK";
        let rows = encode(link).expect("fits");
        let n = rows.len();
        assert_eq!(n, 4 * 4 + 17, "version 4 holds {} bytes at level M", link.len());
        assert!(rows.iter().all(|r| r.len() == n));
        let at = |x: usize, y: usize| rows[y].as_bytes()[x] == b'1';
        for (cx, cy) in [(3, 3), (n - 4, 3), (3, n - 4)] {
            assert!(at(cx, cy), "finder centre");
            assert!(!at(cx + 2, cy), "finder ring");
            assert!(at(cx + 3, cy), "finder edge");
        }
        for i in 8..n - 8 {
            assert_eq!(at(i, 6), i % 2 == 0, "timing row");
            assert_eq!(at(6, i), i % 2 == 0, "timing column");
        }
        assert!(at(8, n - 8), "dark module");
        assert_eq!(encode(&"x".repeat(214)), None, "beyond version 10-M");
        assert_eq!(encode(link), Some(rows), "deterministic");
    }
}
