//! MD5 (RFC 1321), for comparing decoded frames with the test vectors'
//! published checksums. Test code only.

const S: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20,
    4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10,
    15, 21,
];

fn k(i: usize) -> u32 {
    ((i as f64 + 1.0).sin().abs() * 4294967296.0) as u32
}

pub struct Md5 {
    state: [u32; 4],
    buf: Vec<u8>,
    len: u64,
    k: [u32; 64],
}

impl Md5 {
    pub fn new() -> Self {
        let mut kt = [0u32; 64];
        for (i, v) in kt.iter_mut().enumerate() {
            *v = k(i);
        }
        Md5 {
            state: [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476],
            buf: Vec::with_capacity(64),
            len: 0,
            k: kt,
        }
    }

    // The round number picks the function, the message word and the shift,
    // as RFC 1321 writes it.
    #[allow(clippy::needless_range_loop)]
    fn block(&mut self, chunk: &[u8]) {
        let mut m = [0u32; 16];
        for (i, w) in m.iter_mut().enumerate() {
            *w = u32::from_le_bytes([chunk[i * 4], chunk[i * 4 + 1], chunk[i * 4 + 2], chunk[i * 4 + 3]]);
        }
        let [mut a, mut b, mut c, mut d] = self.state;
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(self.k[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(S[i]));
        }
        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.len += data.len() as u64;
        if !self.buf.is_empty() {
            let take = (64 - self.buf.len()).min(data.len());
            self.buf.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.buf.len() < 64 {
                return;
            }
            let b = std::mem::take(&mut self.buf);
            self.block(&b);
        }
        while data.len() >= 64 {
            self.block(&data[..64]);
            data = &data[64..];
        }
        self.buf.extend_from_slice(data);
    }

    pub fn hex(mut self) -> String {
        let bits = self.len.wrapping_mul(8);
        let mut pad = vec![0x80u8];
        while (self.len + pad.len() as u64) % 64 != 56 {
            pad.push(0);
        }
        pad.extend_from_slice(&bits.to_le_bytes());
        let len = self.len;
        self.update(&pad);
        self.len = len;
        self.state.iter().flat_map(|w| w.to_le_bytes()).map(|b| format!("{b:02x}")).collect()
    }
}

#[test]
fn md5_known_answers() {
    let h = |s: &[u8]| {
        let mut m = Md5::new();
        m.update(s);
        m.hex()
    };
    assert_eq!(h(b""), "d41d8cd98f00b204e9800998ecf8427e");
    assert_eq!(h(b"The quick brown fox jumps over the lazy dog"), "9e107d9d372bb6826bd81d3542a419d6");
    let long: Vec<u8> = (0..1000u32).map(|i| i as u8).collect();
    let mut m = Md5::new();
    for chunk in long.chunks(37) {
        m.update(chunk);
    }
    assert_eq!(m.hex(), h(&long));
}
