//! The root-helper signing keys (RH7: separate from the app-updater key).
//!
//! Copies of these public keys live in exactly three places, and
//! `key_copies_are_equal` (k2-root-helper tests) fails when they drift:
//! - here ([`RELEASE_KEYS`]), compiled into the helper and the daemon;
//! - `scripts/install-root-helper.sh` (`K2_ROOT_HELPER_PUBKEYS`);
//! - `scripts/sign-root-helper.sh` reads the secret key, never a copy.
//!
//! **Key ceremony** (Rosson, on z3mbpZ; steps in `scripts/sign-root-helper.sh`):
//! a primary key whose encrypted secret lives only on z3mbpZ (password in
//! the login keychain) and a backup key kept offline. Both public keys go
//! in [`RELEASE_KEYS`]. Until then the list is empty, which refuses every
//! signed self-update and every downloaded install (fail closed), and
//! `release.sh` refuses to tag.

/// One trusted minisign public key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrustedKey {
    /// `primary` | `backup` | `test`.
    pub role: &'static str,
    /// minisign key id as minisign prints it (16 upper hex).
    pub key_id: &'static str,
    /// The base64 line of a `minisign.pub` file ("Ed" + key id + key).
    pub public_key_b64: &'static str,
}

/// Release keys: primary first, backup second. Empty until the key
/// ceremony (see the module doc). Rotation (§9.4): release N is signed by
/// A and carries [A, B]; release N+1 is signed by B and carries [B, C].
pub const RELEASE_KEYS: &[TrustedKey] = &[];

/// TEST ONLY. Trusted only by a helper built with `--features test-keys`
/// (never a release build; `sign-root-helper.sh` refuses such a binary).
/// The matching secret seed is in
/// `crates/k2-root-helper/tests/fixtures/test-signing-seed.hex`.
pub const TEST_KEY: TrustedKey = TrustedKey {
    role: "test",
    key_id: "5716343E2F010C58",
    public_key_b64: "RWRYDAEvPjQWV8mrqr/kwYOF5mOedtRN5O3L1w6MJlK+y2YFPT2gHE5d",
};

// A `test-keys` helper build embeds the string
// `K2_ROOT_HELPER_TEST_KEYS_ENABLED` (k2-root-helper `TEST_KEYS_FLAG`);
// `sign-root-helper.sh` refuses to sign a binary that has it. It is not a
// const here on purpose: a release binary must never contain it.

/// The keys a build trusts.
pub fn trusted_keys(test_keys: bool) -> Vec<TrustedKey> {
    let mut v: Vec<TrustedKey> = RELEASE_KEYS.to_vec();
    if test_keys {
        v.push(TEST_KEY);
    }
    v
}

/// minisign prints the key id as the little-endian u64 of the 8 raw bytes.
pub fn key_id_of(public_key_b64: &str) -> Option<String> {
    let raw = b64_decode(public_key_b64)?;
    if raw.len() != 42 || raw[0] != b'E' {
        return None;
    }
    let mut id = [0u8; 8];
    id.copy_from_slice(&raw[2..10]);
    Some(format!("{:016X}", u64::from_le_bytes(id)))
}

/// Small strict base64 (standard alphabet, padded) so this crate needs no
/// base64 dependency outside the test signer.
pub(crate) fn b64_decode(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a' + 26) as u32,
            b'0'..=b'9' => (c - b'0' + 52) as u32,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        })
    }
    let b = s.trim().as_bytes();
    if b.is_empty() || b.len() % 4 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(b.len() / 4 * 3);
    for (i, chunk) in b.chunks(4).enumerate() {
        let last = i == b.len() / 4 - 1;
        let pad = chunk.iter().rev().take_while(|&&c| c == b'=').count();
        if pad > 2 || (pad > 0 && !last) {
            return None;
        }
        let mut n = 0u32;
        for (j, &c) in chunk.iter().enumerate() {
            let v = if j >= 4 - pad { 0 } else { val(c)? };
            n = (n << 6) | v;
        }
        let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        out.extend_from_slice(&bytes[..3 - pad]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_id_matches_its_bytes() {
        assert_eq!(
            key_id_of(TEST_KEY.public_key_b64).as_deref(),
            Some(TEST_KEY.key_id)
        );
        for k in RELEASE_KEYS {
            assert_eq!(
                key_id_of(k.public_key_b64).as_deref(),
                Some(k.key_id),
                "{k:?}"
            );
            assert!(matches!(k.role, "primary" | "backup"), "{k:?}");
        }
    }

    #[test]
    fn release_keys_never_include_the_test_key() {
        assert!(RELEASE_KEYS.iter().all(|k| k.key_id != TEST_KEY.key_id));
        assert!(!trusted_keys(false).contains(&TEST_KEY));
        assert!(trusted_keys(true).contains(&TEST_KEY));
    }

    #[test]
    fn base64_is_strict() {
        assert_eq!(b64_decode("TWFu").unwrap(), b"Man");
        assert_eq!(b64_decode("TWE=").unwrap(), b"Ma");
        assert_eq!(b64_decode("TQ==").unwrap(), b"M");
        assert!(b64_decode("TQ=").is_none());
        assert!(b64_decode("T===").is_none());
        assert!(b64_decode("TQ==TWFu").is_none());
        assert!(b64_decode("TW-u").is_none());
    }
}
