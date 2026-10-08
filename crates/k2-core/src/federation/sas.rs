//! Commit-then-reveal short authentication string (pairing v2, 0.45.2).
//!
//! The v1 code ([`super::pairing::sas_code`]) hashed only the two key
//! fingerprints, so anyone able to substitute a key during pairing could
//! generate keys offline until the 6-digit code matched. v2 binds the code
//! to two fresh random nonces and fixes their order of disclosure (the
//! numeric-comparison pattern of ZRTP / Bluetooth Secure Simple Pairing):
//!
//! 1. The initiator A picks a 32-byte nonce `Na` and sends only its
//!    commitment `C = SHA-256("k2-sas-commit-v2" || fp_a || Na)`.
//! 2. The responder B picks `Nb` and sends it in clear only after it has C.
//! 3. A reveals `Na`; B checks it against C and aborts on a mismatch.
//! 4. Both show `SAS = SHA-256("k2-sas-v2" || fp_a || fp_b || Na || Nb)`
//!    reduced to 6 decimal digits, initiator first.
//!
//! A party in the middle must fix its key and nonce before it can learn the
//! other side's nonce, so it gets one guess per pairing attempt (1 in 10^6)
//! instead of unlimited offline tries.
//!
//! This module is the single implementation: federation pairing uses it,
//! and other pairing flows (compute-node enroll) are meant to call it too.
//! Nonces are secrets until revealed; callers never log them.

use aws_lc_rs::rand::{SecureRandom, SystemRandom};
use sha2::{Digest, Sha256};

/// Domain prefix of the commitment.
pub const COMMIT_PREFIX: &[u8] = b"k2-sas-commit-v2";
/// Domain prefix of the code.
pub const SAS_PREFIX: &[u8] = b"k2-sas-v2";
/// Nonce length in bytes.
pub const NONCE_LEN: usize = 32;
/// Nonces (and the sessions holding them) are single-use and expire after
/// this many seconds.
pub const NONCE_TTL_SECS: i64 = 600;

/// A pairing nonce.
pub type Nonce = [u8; NONCE_LEN];

/// A fresh random nonce.
pub fn new_nonce() -> Result<Nonce, String> {
    let mut n = [0u8; NONCE_LEN];
    SystemRandom::new()
        .fill(&mut n)
        .map_err(|_| "system random source failed".to_string())?;
    Ok(n)
}

/// `SHA-256(COMMIT_PREFIX || fp_a || Na)`.
pub fn commitment(fp_a: &str, na: &Nonce) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(COMMIT_PREFIX);
    h.update(fp_a.as_bytes());
    h.update(na);
    h.finalize().into()
}

/// Whether `na` opens commitment `c` for initiator `fp_a` (constant time).
pub fn verify_commitment(c: &[u8; 32], fp_a: &str, na: &Nonce) -> bool {
    use subtle::ConstantTimeEq;
    commitment(fp_a, na).ct_eq(c).into()
}

/// The 6-digit code: the first 4 bytes of
/// `SHA-256(SAS_PREFIX || fp_a || fp_b || Na || Nb)` as a big-endian u32,
/// mod 10^6, zero-padded. `fp_a`/`Na` are always the initiator's.
pub fn sas_v2(fp_a: &str, fp_b: &str, na: &Nonce, nb: &Nonce) -> String {
    let mut h = Sha256::new();
    h.update(SAS_PREFIX);
    h.update(fp_a.as_bytes());
    h.update(fp_b.as_bytes());
    h.update(na);
    h.update(nb);
    let d = h.finalize();
    let n = u32::from_be_bytes([d[0], d[1], d[2], d[3]]);
    format!("{:06}", n % 1_000_000)
}

/// Constant-time compare of two codes (length is public).
pub fn sas_eq(a: &str, b: &str) -> bool {
    use subtle::ConstantTimeEq;
    let (a, b) = (a.trim().as_bytes(), b.trim().as_bytes());
    a.len() == b.len() && bool::from(a.ct_eq(b))
}

/// Lowercase hex.
pub fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Parse exactly 32 bytes of hex.
pub fn from_hex32(s: &str) -> Result<[u8; 32], String> {
    let s = s.trim();
    if s.len() != 64 || !s.is_ascii() {
        return Err("expected 64 hex characters".to_string());
    }
    let mut out = [0u8; 32];
    for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
        let pair = std::str::from_utf8(chunk).map_err(|_| "bad hex".to_string())?;
        out[i] = u8::from_str_radix(pair, 16).map_err(|_| "bad hex".to_string())?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NA: Nonce = [0x11; 32];
    const NB: Nonce = [0x22; 32];

    #[test]
    fn both_sides_compute_the_same_code() {
        let (fa, fb) = ("aa".repeat(32), "bb".repeat(32));
        let c = commitment(&fa, &NA);
        // B, after A's reveal:
        assert!(verify_commitment(&c, &fa, &NA));
        let at_b = sas_v2(&fa, &fb, &NA, &NB);
        let at_a = sas_v2(&fa, &fb, &NA, &NB);
        assert_eq!(at_a, at_b);
        assert_eq!(at_a.len(), 6);
        assert!(at_a.chars().all(|c| c.is_ascii_digit()));
        assert!(sas_eq(&at_a, &at_b));
        assert!(!sas_eq(&at_a, "12345"), "a different length never matches");
    }

    #[test]
    fn pinned_vector() {
        // Pinned so the derivation cannot drift between releases (both
        // sides of a pairing may run different builds).
        let (fa, fb) = ("aa".repeat(32), "bb".repeat(32));
        let mut h = Sha256::new();
        h.update(b"k2-sas-v2");
        h.update(fa.as_bytes());
        h.update(fb.as_bytes());
        h.update(NA);
        h.update(NB);
        let d = h.finalize();
        let want = format!("{:06}", u32::from_be_bytes([d[0], d[1], d[2], d[3]]) % 1_000_000);
        assert_eq!(sas_v2(&fa, &fb, &NA, &NB), want);
        let mut h = Sha256::new();
        h.update(b"k2-sas-commit-v2");
        h.update(fa.as_bytes());
        h.update(NA);
        let c: [u8; 32] = h.finalize().into();
        assert_eq!(commitment(&fa, &NA), c);
    }

    #[test]
    fn literal_vector_matches_an_independent_implementation() {
        // Computed with Python's hashlib (the CLI test's stand-in peer uses
        // the same formulas), so both sides are pinned to one value.
        let (fa, fb) = ("aa".repeat(32), "bb".repeat(32));
        assert_eq!(sas_v2(&fa, &fb, &NA, &NB), "632758");
        assert_eq!(
            to_hex(&commitment(&fa, &NA)),
            "2d6e7a972a208c69e0ed6c881379b0b1a2b215d49cbee9b17fc4a5653cb2b569"
        );
    }

    #[test]
    fn a_wrong_reveal_or_another_initiator_does_not_open_the_commitment() {
        let fa = "aa".repeat(32);
        let c = commitment(&fa, &NA);
        let mut wrong = NA;
        wrong[31] ^= 1;
        assert!(!verify_commitment(&c, &fa, &wrong));
        assert!(!verify_commitment(&c, &"cc".repeat(32), &NA), "the commitment binds fp_a");
    }

    #[test]
    fn the_code_depends_on_both_nonces_and_both_keys() {
        // Fixed nonces: the code an attacker would need is determined by Na
        // and Nb, so choosing another fp_b after seeing only C (Na still
        // hidden) gives an unrelated code.
        let (fa, fb, fm) = ("aa".repeat(32), "bb".repeat(32), "ee".repeat(32));
        let real = sas_v2(&fa, &fb, &NA, &NB);
        let mut na2 = NA;
        na2[0] ^= 0x80;
        let mut nb2 = NB;
        nb2[0] ^= 0x80;
        let variants = [
            sas_v2(&fa, &fm, &NA, &NB),
            sas_v2(&fa, &fb, &na2, &NB),
            sas_v2(&fa, &fb, &NA, &nb2),
            sas_v2(&fb, &fa, &NA, &NB),
        ];
        for v in &variants {
            assert_ne!(v, &real, "changing any input must change the code (fixed vectors)");
        }
        // Order is fixed: initiator first.
        assert_ne!(sas_v2(&fa, &fb, &NA, &NB), sas_v2(&fa, &fb, &NB, &NA));
    }

    #[test]
    fn hex_round_trip() {
        let n = new_nonce().expect("nonce");
        assert_eq!(from_hex32(&to_hex(&n)).expect("parse"), n);
        assert!(from_hex32("zz").is_err());
        assert!(from_hex32(&"g".repeat(64)).is_err());
        assert_ne!(new_nonce().expect("a"), new_nonce().expect("b"));
    }
}
