//! SHA-256, and HMAC-SHA256: the keyed hash that addresses blobs (tier 0, per space key)
//! and names a file's content for disk sync (the synced hash and echo suppression, per doc
//! key). No plain content hash is stored (K-PURGE-1, -13): a hash of a short secret can be
//! brute-forced, a keyed one can't, and destroying its key makes it meaningless.
//!
//! Written here rather than taken from a crate: the kit needs one stable content hash, and
//! the store keeps it across releases, so it can't be `std`'s `DefaultHasher` (its algorithm
//! may change between Rust releases). It's checked against the FIPS 180-4 test vectors and
//! HMAC against RFC 4231's, below. [`Sha256`] and [`HmacSha256`] stream, so a blob of any
//! size is hashed in constant memory.

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

const H0: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

fn block(h: &mut [u32; 8], b: &[u8]) {
    let mut w = [0u32; 64];
    for (i, c) in b.chunks_exact(4).enumerate() {
        w[i] = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
    }
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
    }
    let [mut a, mut b_, mut c, mut d, mut e, mut f, mut g, mut hh] = *h;
    for i in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ (!e & g);
        let t1 = hh
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(K[i])
            .wrapping_add(w[i]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b_) ^ (a & c) ^ (b_ & c);
        let t2 = s0.wrapping_add(maj);
        hh = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b_;
        b_ = a;
        a = t1.wrapping_add(t2);
    }
    for (x, y) in h.iter_mut().zip([a, b_, c, d, e, f, g, hh]) {
        *x = x.wrapping_add(y);
    }
}

/// SHA-256 of `data`.
pub(crate) fn sha256(data: &[u8]) -> [u8; 32] {
    let mut s = Sha256::new();
    s.update(data);
    s.finish()
}

/// Streaming SHA-256: feed any number of [`Sha256::update`] calls, then [`Sha256::finish`].
#[derive(Clone)]
pub(crate) struct Sha256 {
    h: [u32; 8],
    buf: [u8; 64],
    buf_len: usize,
    total: u64,
}

impl Sha256 {
    pub(crate) fn new() -> Self {
        Self {
            h: H0,
            buf: [0; 64],
            buf_len: 0,
            total: 0,
        }
    }

    pub(crate) fn update(&mut self, mut data: &[u8]) {
        self.total = self.total.wrapping_add(data.len() as u64);
        if self.buf_len > 0 {
            let take = (64 - self.buf_len).min(data.len());
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            data = &data[take..];
            if self.buf_len < 64 {
                return;
            }
            let b = self.buf;
            block(&mut self.h, &b);
            self.buf_len = 0;
        }
        let mut chunks = data.chunks_exact(64);
        for c in &mut chunks {
            block(&mut self.h, c);
        }
        let rest = chunks.remainder();
        self.buf[..rest.len()].copy_from_slice(rest);
        self.buf_len = rest.len();
    }

    pub(crate) fn finish(mut self) -> [u8; 32] {
        let bits = self.total.wrapping_mul(8);
        let rest = self.buf_len;
        let mut tail = [0u8; 128];
        tail[..rest].copy_from_slice(&self.buf[..rest]);
        tail[rest] = 0x80;
        let n = if rest < 56 { 64 } else { 128 };
        tail[n - 8..n].copy_from_slice(&bits.to_be_bytes());
        for c in tail[..n].chunks_exact(64) {
            block(&mut self.h, c);
        }
        let mut out = [0u8; 32];
        for (o, x) in out.chunks_exact_mut(4).zip(self.h) {
            o.copy_from_slice(&x.to_be_bytes());
        }
        out
    }
}

/// Streaming HMAC-SHA256 (RFC 2104) under a key of at most 64 bytes (blob keys are 32).
#[derive(Clone)]
pub(crate) struct HmacSha256 {
    inner: Sha256,
    outer: Sha256,
}

impl HmacSha256 {
    pub(crate) fn new(key: &[u8]) -> Self {
        let mut k = [0u8; 64];
        if key.len() > 64 {
            k[..32].copy_from_slice(&sha256(key));
        } else {
            k[..key.len()].copy_from_slice(key);
        }
        let (mut inner, mut outer) = (Sha256::new(), Sha256::new());
        inner.update(&k.map(|b| b ^ 0x36));
        outer.update(&k.map(|b| b ^ 0x5c));
        Self { inner, outer }
    }

    pub(crate) fn update(&mut self, data: &[u8]) {
        self.inner.update(data);
    }

    pub(crate) fn finish(self) -> [u8; 32] {
        let mut outer = self.outer;
        outer.update(&self.inner.finish());
        outer.finish()
    }
}

/// HMAC-SHA256 of `data` under `key`, in one call.
pub(crate) fn hmac(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut m = HmacSha256::new(key);
    m.update(data);
    m.finish()
}

/// First byte of a keyed file hash ([`file_hash`]).
const KEYED_FILE_HASH: u8 = 1;

/// The keyed hash of a file's bytes under its doc's key: `1 ‖ HMAC-SHA256(key, bytes)`
/// (33 bytes). A 32-byte hash is a format-2 store's plain SHA-256 ([`legacy_matches`]).
pub(crate) fn file_hash(key: &[u8; 32], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(33);
    out.push(KEYED_FILE_HASH);
    out.extend_from_slice(&hmac(key, data));
    out
}

/// Whether `stored` is a format-2 plain SHA-256 of `data` (read once, then replaced by a
/// keyed hash; never written).
pub(crate) fn legacy_matches(stored: &[u8], data: &[u8]) -> bool {
    stored.len() == 32 && stored == sha256(data)
}

/// 32 random bytes from the OS (`/dev/urandom`): blob space keys and per-doc hash keys.
pub(crate) fn random32() -> std::io::Result<[u8; 32]> {
    use std::io::Read;
    let mut k = [0u8; 32];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut k))?;
    Ok(k)
}

/// Lowercase hex.
pub(crate) fn hex(b: &[u8]) -> String {
    use std::fmt::Write;
    b.iter()
        .fold(String::with_capacity(b.len() * 2), |mut s, x| {
            let _ = write!(s, "{x:02x}");
            s
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fips_180_4_vectors() {
        let v = |d: &[u8]| hex(&sha256(d));
        assert_eq!(
            v(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            v(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            v(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        assert_eq!(
            v(b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu"),
            "cf5b16a778af8380036ce59e7b0492370b249b11e8f07a51afac45037afee9d1"
        );
        assert_eq!(
            v(&vec![b'a'; 1_000_000]),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
        // every padding boundary
        for n in 50..140 {
            assert_ne!(sha256(&vec![7u8; n]), sha256(&vec![7u8; n + 1]));
        }
    }

    #[test]
    fn streaming_matches_one_shot_at_every_split() {
        let data: Vec<u8> = (0..300u32).map(|i| (i * 7 + 3) as u8).collect();
        for len in [0, 1, 55, 56, 63, 64, 65, 119, 128, 200, 300] {
            let whole = sha256(&data[..len]);
            for step in [1, 3, 63, 64, 65, 200] {
                let mut s = Sha256::new();
                for c in data[..len].chunks(step) {
                    s.update(c);
                }
                assert_eq!(s.finish(), whole, "len {len} step {step}");
            }
        }
    }

    #[test]
    fn hmac_rfc_4231_vectors() {
        let mac = |k: &[u8], d: &[u8]| {
            let mut m = HmacSha256::new(k);
            m.update(d);
            hex(&m.finish())
        };
        // test case 1
        assert_eq!(
            mac(&[0x0b; 20], b"Hi There"),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        // test case 2
        assert_eq!(
            mac(b"Jefe", b"what do ya want for nothing?"),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        // test case 3
        assert_eq!(
            mac(&[0xaa; 20], &[0xdd; 50]),
            "773ea91e36800e46854db8ebd09181a72959098b3ef8c122d9635514ced565fe"
        );
        // test case 6: a key longer than the block is hashed first
        assert_eq!(
            mac(
                &[0xaa; 131],
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            ),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
        // streaming: same result in pieces
        let mut m = HmacSha256::new(b"Jefe");
        for c in b"what do ya want for nothing?".chunks(5) {
            m.update(c);
        }
        assert_eq!(
            hex(&m.finish()),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn file_hashes_are_keyed_and_legacy_is_read_only() {
        let (k1, k2) = ([1u8; 32], [2u8; 32]);
        let h = file_hash(&k1, b"secret");
        assert_eq!(h.len(), 33);
        assert_eq!(h[0], KEYED_FILE_HASH);
        assert_eq!(&h[1..], &hmac(&k1, b"secret"));
        // another key (a purge rotates it): unrelated
        assert_ne!(h, file_hash(&k2, b"secret"));
        // never the plain hash
        assert_ne!(&h[1..], &sha256(b"secret"));
        assert!(legacy_matches(&sha256(b"secret"), b"secret"));
        assert!(!legacy_matches(&h, b"secret"));
        assert!(!legacy_matches(&sha256(b"secret"), b"other"));
        let (a, b) = (random32().unwrap(), random32().unwrap());
        assert_ne!(a, b);
    }
}
