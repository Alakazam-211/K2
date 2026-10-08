//! Primitives: HKDF-SHA256 subkeys, HMAC-SHA256, XChaCha20-Poly1305, and
//! the fixed binary encoding used for every AAD and MAC input (VS17).
//!
//! Info strings are versioned and domain-separated (`k2-vault/v1 …`).

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use crate::error::{Result, VaultError};
use crate::secret::Key32;

pub const FORMAT_VERSION: u8 = 1;

pub const INFO_ROW_MAC: &[u8] = b"k2-vault/v1 row-mac";
pub const INFO_AUDIT: &[u8] = b"k2-vault/v1 audit";
pub const INFO_FINGERPRINT: &[u8] = b"k2-vault/v1 fingerprint";
pub const INFO_PLACEHOLDER: &[u8] = b"k2-vault/v1 placeholder";
pub const INFO_HEADER: &[u8] = b"k2-vault/v1 header";
pub const INFO_BOX_WRAP: &[u8] = b"k2-vault/v1 box-wrap";
pub const INFO_RECOVERY: &[u8] = b"k2-vault/v1 recovery";
pub const INFO_SUBSYSTEM: &[u8] = b"k2-vault/v1 subsystem ";

type HmacSha256 = Hmac<Sha256>;

/// HKDF-SHA256 → 32-byte key in locked memory.
pub fn hkdf32(ikm: &[u8], salt: &[u8], info: &[u8]) -> Key32 {
    let hk = Hkdf::<Sha256>::new(Some(salt), ikm);
    let mut out = [0u8; 32];
    hk.expand(info, &mut out).expect("32 bytes is a valid HKDF length");
    Key32::from_array(out)
}

pub fn hmac(key: &Key32, data: &[u8]) -> [u8; 32] {
    let mut m = <HmacSha256 as Mac>::new_from_slice(key.as_bytes()).expect("any key length");
    m.update(data);
    m.finalize().into_bytes().into()
}

/// Constant-time MAC check.
pub fn hmac_verify(key: &Key32, data: &[u8], tag: &[u8]) -> bool {
    let mut m = <HmacSha256 as Mac>::new_from_slice(key.as_bytes()).expect("any key length");
    m.update(data);
    m.verify_slice(tag).is_ok()
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::getrandom(&mut b).expect("OS randomness");
    b
}

/// XChaCha20-Poly1305 seal with a fresh random 24-byte nonce.
pub fn xseal(key: &Key32, aad: &[u8], plaintext: &[u8]) -> ([u8; 24], Vec<u8>) {
    let cipher = XChaCha20Poly1305::new(key.as_bytes().into());
    let nonce = random_bytes::<24>();
    let ct = cipher
        .encrypt(XNonce::from_slice(&nonce), Payload { msg: plaintext, aad })
        .expect("XChaCha20-Poly1305 encryption does not fail for in-memory buffers");
    (nonce, ct)
}

pub fn xopen(key: &Key32, nonce: &[u8], aad: &[u8], ct: &[u8]) -> Result<Vec<u8>> {
    if nonce.len() != 24 {
        return Err(VaultError::tamper("bad nonce length"));
    }
    let cipher = XChaCha20Poly1305::new(key.as_bytes().into());
    cipher
        .decrypt(XNonce::from_slice(nonce), Payload { msg: ct, aad })
        .map_err(|_| VaultError::tamper("ciphertext failed authentication"))
}

/// Fixed binary encoding for AAD/MAC inputs: typed, length-prefixed
/// fields, so no two different field lists encode to the same bytes.
#[derive(Default)]
pub struct Enc(Vec<u8>);

impl Enc {
    pub fn new(tag: &str) -> Enc {
        let mut e = Enc(Vec::with_capacity(128));
        e.0.push(FORMAT_VERSION);
        e.str(tag);
        e
    }

    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(b'b');
        self.0.push(v);
        self
    }

    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.0.push(b'i');
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }

    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.0.push(b'l');
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }

    pub fn i64(&mut self, v: i64) -> &mut Self {
        self.u64(v as u64)
    }

    pub fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.0.push(b's');
        self.0.extend_from_slice(&(v.len() as u32).to_be_bytes());
        self.0.extend_from_slice(v);
        self
    }

    pub fn str(&mut self, v: &str) -> &mut Self {
        self.bytes(v.as_bytes())
    }

    pub fn opt_str(&mut self, v: Option<&str>) -> &mut Self {
        match v {
            Some(s) => {
                self.0.push(b'S');
                self.str(s)
            }
            None => {
                self.0.push(b'N');
                self
            }
        }
    }

    pub fn opt_i64(&mut self, v: Option<i64>) -> &mut Self {
        match v {
            Some(n) => {
                self.0.push(b'S');
                self.i64(n)
            }
            None => {
                self.0.push(b'N');
                self
            }
        }
    }

    pub fn finish(&self) -> &[u8] {
        &self.0
    }
}

pub fn hex(b: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(b.len() * 2);
    for &x in b {
        s.push(H[(x >> 4) as usize] as char);
        s.push(H[(x & 15) as usize] as char);
    }
    s
}

pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// Base62 token of `n` characters from OS randomness (unbiased).
pub fn base62(n: usize) -> String {
    const A: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut out = String::with_capacity(n);
    while out.len() < n {
        let b = random_bytes::<64>();
        for x in b {
            // 248 = 62 * 4: reject the tail to stay unbiased.
            if x < 248 {
                out.push(A[(x % 62) as usize] as char);
                if out.len() == n {
                    break;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xseal_round_trip_and_aad_binding() {
        let k = Key32::random();
        let (n, ct) = xseal(&k, b"aad-1", b"hello");
        assert_eq!(xopen(&k, &n, b"aad-1", &ct).expect("opens"), b"hello");
        let e = xopen(&k, &n, b"aad-2", &ct).expect_err("other AAD must fail");
        assert_eq!(e.code, "vault_tamper");
        let other = Key32::random();
        assert!(xopen(&other, &n, b"aad-1", &ct).is_err());
    }

    #[test]
    fn encoding_is_unambiguous() {
        let mut a = Enc::new("t");
        a.str("ab").str("c");
        let mut b = Enc::new("t");
        b.str("a").str("bc");
        assert_ne!(a.finish(), b.finish());
        let mut c = Enc::new("t");
        c.opt_str(None);
        let mut d = Enc::new("t");
        d.opt_str(Some(""));
        assert_ne!(c.finish(), d.finish());
    }

    #[test]
    fn hkdf_info_separates_keys() {
        let a = hkdf32(b"ikm", b"salt", INFO_ROW_MAC);
        let b = hkdf32(b"ikm", b"salt", INFO_AUDIT);
        assert!(a != b);
        let a2 = hkdf32(b"ikm", b"salt", INFO_ROW_MAC);
        assert!(a == a2);
    }

    #[test]
    fn base62_has_requested_length_and_alphabet() {
        let t = base62(32);
        assert_eq!(t.len(), 32);
        assert!(t.chars().all(|c| c.is_ascii_alphanumeric()));
        assert_ne!(base62(32), t);
    }

    #[test]
    fn hex_round_trip() {
        assert_eq!(unhex(&hex(&[0, 1, 0xab, 0xff])).expect("hex"), vec![0, 1, 0xab, 0xff]);
        assert!(unhex("abc").is_none());
        assert!(unhex("zz").is_none());
    }
}
