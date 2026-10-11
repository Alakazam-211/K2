//! TEST ONLY (`--features test-signer`): a minisign signer for the TEST
//! key, so daemon and helper tests (and the Linux integration run) can make
//! signed fixture builds, including tampered ones. Output is byte-for-byte
//! the format `minisign -S` writes (prehashed `ED` algorithm).
//!
//! The seed lives in `tests/fixtures/test-signing-seed.hex`. It signs for
//! [`crate::keys::TEST_KEY`] only, which no release build trusts.

use base64::Engine as _;
use blake2::{Blake2b512, Digest};
use ring::signature::Ed25519KeyPair;

/// The TEST key's raw key id bytes (minisign keynum).
pub const TEST_KEY_ID_BYTES: [u8; 8] = [0x58, 0x0c, 0x01, 0x2f, 0x3e, 0x34, 0x16, 0x57];

fn test_seed() -> [u8; 32] {
    let hex = include_str!("../tests/fixtures/test-signing-seed.hex").trim();
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).expect("test seed hex");
    }
    out
}

/// Sign `data` with the TEST key and `trusted_comment`.
pub fn sign_with_test_key(data: &[u8], trusted_comment: &str) -> String {
    sign(&test_seed(), TEST_KEY_ID_BYTES, data, trusted_comment)
}

/// Sign with any seed + key id (tests use a second random key to prove an
/// untrusted key is refused).
pub fn sign(seed: &[u8; 32], key_id: [u8; 8], data: &[u8], trusted_comment: &str) -> String {
    let kp = Ed25519KeyPair::from_seed_unchecked(seed).expect("ed25519 seed");
    let prehash = Blake2b512::digest(data);
    let sig = kp.sign(&prehash);
    let mut bin1 = Vec::with_capacity(74);
    bin1.extend_from_slice(b"ED");
    bin1.extend_from_slice(&key_id);
    bin1.extend_from_slice(sig.as_ref());
    let mut global_msg = sig.as_ref().to_vec();
    global_msg.extend_from_slice(trusted_comment.as_bytes());
    let global = kp.sign(&global_msg);
    let b64 = base64::engine::general_purpose::STANDARD;
    format!(
        "untrusted comment: signature from k2 TEST key\n{}\ntrusted comment: {trusted_comment}\n{}\n",
        b64.encode(&bin1),
        b64.encode(global.as_ref())
    )
}

/// The minisign public key line for a seed + key id.
pub fn public_key_b64(seed: &[u8; 32], key_id: [u8; 8]) -> String {
    use ring::signature::KeyPair as _;
    let kp = Ed25519KeyPair::from_seed_unchecked(seed).expect("ed25519 seed");
    let mut raw = b"Ed".to_vec();
    raw.extend_from_slice(&key_id);
    raw.extend_from_slice(kp.public_key().as_ref());
    base64::engine::general_purpose::STANDARD.encode(raw)
}

/// A fake ELF for `arch` that embeds `marker` (as a real build would).
pub fn fake_elf(arch: &str, marker: &str) -> Vec<u8> {
    let mut h = vec![0u8; 64];
    h[0..4].copy_from_slice(b"\x7fELF");
    h[4] = 2;
    h[5] = 1;
    let m = crate::version::elf_machine(arch).unwrap_or(0);
    h[18..20].copy_from_slice(&m.to_le_bytes());
    h.extend_from_slice(b"\0padding\0");
    h.extend_from_slice(marker.as_bytes());
    h.extend_from_slice(b"\0tail\0");
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::TEST_KEY;
    use crate::sig::{trusted_comment, verify_signed_build};
    use crate::version::format_marker;

    #[test]
    fn test_seed_matches_the_published_test_key() {
        assert_eq!(
            public_key_b64(&test_seed(), TEST_KEY_ID_BYTES),
            TEST_KEY.public_key_b64
        );
    }

    #[test]
    fn a_test_signed_build_verifies_and_each_tamper_fails() {
        let elf = fake_elf("x86_64", &format_marker("0.45.5", 4, "x86_64"));
        let tc = trusted_comment("x86_64", "0.45.5", 4);
        let sig = sign_with_test_key(&elf, &tc);
        let keys = [TEST_KEY];
        let v = verify_signed_build(&elf, &sig, &keys, "x86_64").expect("verifies");
        assert_eq!(v.key_id, TEST_KEY.key_id);
        assert_eq!(v.protocol, 4);

        // No trusted keys: refuse.
        assert!(verify_signed_build(&elf, &sig, &[], "x86_64").is_err());
        // One flipped byte in the binary.
        let mut bad = elf.clone();
        bad[70] ^= 1;
        assert!(verify_signed_build(&bad, &sig, &keys, "x86_64").is_err());
        // Tampered trusted comment (global signature fails).
        let forged = sig.replace("version:0.45.5", "version:0.45.9");
        assert!(verify_signed_build(&elf, &forged, &keys, "x86_64").is_err());
        // Validly signed, but naming the daemon asset.
        let s = sign_with_test_key(
            &elf,
            "file:k2-daemon-linux-x86_64 version:0.45.5 protocol:4",
        );
        assert_eq!(
            verify_signed_build(&elf, &s, &keys, "x86_64"),
            Err("trusted comment names another file")
        );
        // Wrong arch.
        assert!(verify_signed_build(&elf, &sig, &keys, "aarch64").is_err());
        // Marker disagrees with the comment.
        let s = sign_with_test_key(&elf, &trusted_comment("x86_64", "0.45.6", 4));
        assert_eq!(
            verify_signed_build(&elf, &s, &keys, "x86_64"),
            Err("build marker does not match the trusted comment")
        );
        // Zero and two markers.
        let none = fake_elf("x86_64", "no marker");
        let s = sign_with_test_key(&none, &tc);
        assert_eq!(
            verify_signed_build(&none, &s, &keys, "x86_64"),
            Err("build marker missing or repeated")
        );
        let mut two = elf.clone();
        two.extend_from_slice(format_marker("0.45.5", 4, "x86_64").as_bytes());
        let s = sign_with_test_key(&two, &tc);
        assert_eq!(
            verify_signed_build(&two, &s, &keys, "x86_64"),
            Err("build marker missing or repeated")
        );
        // Another (untrusted) key.
        let other = sign(&[7u8; 32], [1, 2, 3, 4, 5, 6, 7, 8], &elf, &tc);
        assert!(verify_signed_build(&elf, &other, &keys, "x86_64").is_err());
    }
}
