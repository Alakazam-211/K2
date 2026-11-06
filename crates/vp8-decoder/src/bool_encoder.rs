//! A boolean entropy encoder (RFC 6386 Section 7.3), for testing the decoder.

pub(crate) struct BoolEncoder {
    out: Vec<u8>,
    range: u32,
    bottom: u32,
    bit_count: i32,
}

impl BoolEncoder {
    pub(crate) fn new() -> Self {
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

    pub(crate) fn write(&mut self, prob: u8, bit: bool) {
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

    pub(crate) fn write_literal(&mut self, v: u32, n: u32) {
        for i in (0..n).rev() {
            self.write(128, (v >> i) & 1 == 1);
        }
    }

    pub(crate) fn finish(mut self) -> Vec<u8> {
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
