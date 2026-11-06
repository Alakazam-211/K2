//! The boolean entropy encoder (RFC 6386 Section 7.3), used by the tests
//! and by vp8-encoder.

pub struct BoolEncoder {
    out: Vec<u8>,
    range: u32,
    bottom: u32,
    bit_count: i32,
}

impl BoolEncoder {
    pub fn new() -> Self {
        BoolEncoder {
            out: Vec::new(),
            range: 255,
            bottom: 0,
            bit_count: 24,
        }
    }

    fn add_one_to_output(&mut self) {
        // Propagate a carry into the bytes already written.
        for b in self.out.iter_mut().rev() {
            if *b == 255 {
                *b = 0;
            } else {
                *b += 1;
                return;
            }
        }
    }

    pub fn write(&mut self, prob: u8, bit: bool) {
        let split = 1 + (((self.range - 1) * prob as u32) >> 8);
        if bit {
            self.bottom = self.bottom.wrapping_add(split);
            self.range -= split;
        } else {
            self.range = split;
        }
        while self.range < 128 {
            self.range <<= 1;
            if self.bottom & (1 << 31) != 0 {
                self.add_one_to_output();
            }
            self.bottom <<= 1;
            self.bit_count -= 1;
            if self.bit_count == 0 {
                self.out.push((self.bottom >> 24) as u8);
                self.bottom &= (1 << 24) - 1;
                self.bit_count = 8;
            }
        }
    }

    pub fn write_bit(&mut self, bit: bool) {
        self.write(128, bit);
    }

    pub fn write_literal(&mut self, v: u32, n: u32) {
        for i in (0..n).rev() {
            self.write(128, (v >> i) & 1 == 1);
        }
    }

    /// The inverse of the decoder's optional signed value: a flag, then a
    /// magnitude of `n` bits and a sign bit.
    pub fn write_optional_signed(&mut self, v: i32, n: u32) {
        self.write_bit(v != 0);
        if v != 0 {
            self.write_literal(v.unsigned_abs(), n);
            self.write_bit(v < 0);
        }
    }

    /// Writes `value` with a tree (leaves stored as `-value`, as the
    /// decoder reads them).
    pub fn write_tree(&mut self, tree: &[i8], probs: &[u8], value: u8) {
        let mut path = [(0usize, false); 16];
        let len = tree_path(tree, 0, value, &mut path, 0).expect("the value is a leaf of the tree");
        for &(node, bit) in &path[..len] {
            self.write(probs[node >> 1], bit);
        }
    }

    /// Bytes written so far (the final size is a few bytes more).
    pub fn len(&self) -> usize {
        self.out.len()
    }

    /// Whether nothing has been written yet.
    pub fn is_empty(&self) -> bool {
        self.out.is_empty()
    }

    pub fn finish(mut self) -> Vec<u8> {
        let mut c = self.bit_count;
        let mut v = self.bottom;
        if v & (1u32 << (32 - c)) != 0 {
            self.add_one_to_output();
        }
        v <<= c & 7;
        c >>= 3;
        while c > 0 {
            v <<= 8;
            c -= 1;
        }
        for _ in 0..4 {
            self.out.push((v >> 24) as u8);
            v <<= 8;
        }
        self.out
    }
}

/// The path of branches from `node` to the leaf `value`.
fn tree_path(tree: &[i8], node: usize, value: u8, path: &mut [(usize, bool); 16], depth: usize) -> Option<usize> {
    for bit in [false, true] {
        let next = tree[node + bit as usize];
        path[depth] = (node, bit);
        if next <= 0 {
            if (-next) as u8 == value {
                return Some(depth + 1);
            }
        } else if let Some(n) = tree_path(tree, next as usize, value, path, depth + 1) {
            return Some(n);
        }
    }
    None
}
