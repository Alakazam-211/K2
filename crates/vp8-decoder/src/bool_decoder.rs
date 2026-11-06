//! The boolean entropy decoder (RFC 6386 Section 7).
//!
//! Reading past the end of a partition yields zero bits, as the RFC's
//! reference decoder does. A truncated partition therefore decodes to
//! something rather than failing; the caller checks partition sizes up front.

/// Bits held in `value` beyond the 8 that are compared against the split.
const VALUE_BITS: i32 = 64;

pub(crate) struct BoolDecoder<'a> {
    data: &'a [u8],
    pos: usize,
    /// The window, left-aligned: the top 8 bits are compared with the split.
    value: u64,
    /// Valid bits in `value` below the top 8. Refilled when it goes negative.
    count: i32,
    range: u32,
}

impl<'a> BoolDecoder<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        let mut d = BoolDecoder {
            data,
            pos: 0,
            value: 0,
            count: -8,
            range: 255,
        };
        d.fill();
        d
    }

    #[inline]
    fn fill(&mut self) {
        let mut shift = VALUE_BITS - 8 - (self.count + 8);
        while shift >= 0 {
            match self.data.get(self.pos) {
                Some(&b) => {
                    self.value |= (b as u64) << shift;
                    self.pos += 1;
                    self.count += 8;
                    shift -= 8;
                }
                None => {
                    // Past the end: the rest is zeros. Never refill again.
                    self.count = 0x4000_0000;
                    return;
                }
            }
        }
    }

    /// One bool whose probability of being 0 is `prob / 256`.
    #[inline]
    pub(crate) fn read(&mut self, prob: u8) -> bool {
        let split = 1 + (((self.range - 1) * prob as u32) >> 8);
        if self.count < 0 {
            self.fill();
        }
        let big_split = (split as u64) << (VALUE_BITS - 8);
        let bit = if self.value >= big_split {
            self.range -= split;
            self.value -= big_split;
            true
        } else {
            self.range = split;
            false
        };
        // Normalise so that range is in 128..=255.
        let shift = self.range.leading_zeros() - 24;
        self.range <<= shift;
        self.value <<= shift;
        self.count -= shift as i32;
        bit
    }

    #[inline]
    pub(crate) fn read_bit(&mut self) -> bool {
        self.read(128)
    }

    /// An unsigned `n`-bit literal, most significant bit first (`L(n)`).
    pub(crate) fn read_literal(&mut self, n: u32) -> u32 {
        let mut v = 0;
        for _ in 0..n {
            v = (v << 1) | self.read_bit() as u32;
        }
        v
    }

    /// A flag, then (if set) an `n`-bit magnitude and a sign bit; 0 if unset.
    pub(crate) fn read_optional_signed(&mut self, n: u32) -> i32 {
        if !self.read_bit() {
            return 0;
        }
        let v = self.read_literal(n) as i32;
        if self.read_bit() {
            -v
        } else {
            v
        }
    }

    /// A tree-coded value (RFC 6386 Section 8.1). Leaves are stored as
    /// non-positive numbers (`-value`); index 0 is only ever the root.
    #[inline]
    pub(crate) fn read_tree(&mut self, tree: &[i8], probs: &[u8]) -> u8 {
        self.read_tree_from(tree, probs, 0)
    }

    #[inline]
    pub(crate) fn read_tree_from(&mut self, tree: &[i8], probs: &[u8], start: usize) -> u8 {
        let mut i = start;
        loop {
            let n = tree[i + self.read(probs[i >> 1]) as usize];
            if n <= 0 {
                return (-n) as u8;
            }
            i = n as usize;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bool_encoder::BoolEncoder;

    #[test]
    fn round_trips_with_the_encoder() {
        let mut seed = 0x1234_5678u32;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        let mut items = Vec::new();
        for _ in 0..20_000 {
            let prob = (rnd() % 255 + 1) as u8;
            // Bias the bit by the probability so the coder is exercised realistically.
            let bit = (rnd() % 256) >= prob as u32;
            items.push((prob, bit));
        }
        let mut e = BoolEncoder::new();
        for &(p, b) in &items {
            e.write(p, b);
        }
        e.write_literal(0xABCDE, 20);
        let data = e.finish();
        let mut d = BoolDecoder::new(&data);
        for (i, &(p, b)) in items.iter().enumerate() {
            assert_eq!(d.read(p), b, "bool {i}");
        }
        assert_eq!(d.read_literal(20), 0xABCDE);
    }

    #[test]
    fn reads_zeros_past_the_end() {
        let mut d = BoolDecoder::new(&[]);
        for _ in 0..100 {
            assert!(!d.read(1));
        }
    }
}
