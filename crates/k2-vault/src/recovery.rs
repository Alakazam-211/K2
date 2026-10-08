//! Recovery key (§7.4, VS17): 24 BIP-39 English words from 256 bits of
//! OS randomness. The age identity is `HKDF(seed, "k2-vault/v1 recovery")`
//! used as an X25519 scalar, so the phrase can also be exported as an
//! `AGE-SECRET-KEY-1…` and opened with stock `rage`.
//!
//! K2 keeps only the public recipient. The phrase is shown once, to a
//! person, and never stored.

use zeroize::Zeroize;

use crate::agefmt::Identity;
use crate::crypto::{hkdf32, sha256, INFO_RECOVERY};
use crate::error::{Result, VaultError};
use crate::secret::{Key32, SecretBytes};

const WORDLIST: &str = include_str!("../data/bip39-english.txt");

fn words() -> Vec<&'static str> {
    WORDLIST.lines().map(str::trim).filter(|w| !w.is_empty()).collect()
}

/// A freshly generated recovery seed.
pub struct RecoverySeed(Key32);

impl RecoverySeed {
    pub fn generate() -> RecoverySeed {
        RecoverySeed(Key32::random())
    }

    pub fn from_entropy(e: &[u8; 32]) -> RecoverySeed {
        RecoverySeed(Key32::from_bytes(e))
    }

    /// The 24-word phrase (space separated).
    pub fn phrase(&self) -> SecretBytes {
        let list = words();
        let entropy = self.0.as_bytes();
        let check = sha256(entropy)[0];
        // 256 bits entropy + 8 bits checksum = 264 bits = 24 × 11.
        let mut bits = [0u8; 33];
        bits[..32].copy_from_slice(entropy);
        bits[32] = check;
        let mut out = String::with_capacity(24 * 9);
        for i in 0..24 {
            let mut idx = 0usize;
            for b in 0..11 {
                let bit = i * 11 + b;
                let v = (bits[bit / 8] >> (7 - (bit % 8))) & 1;
                idx = (idx << 1) | v as usize;
            }
            if i > 0 {
                out.push(' ');
            }
            out.push_str(list[idx]);
        }
        bits.zeroize();
        SecretBytes::from_string(out)
    }

    /// Parse a typed-back phrase (case and extra whitespace ignored).
    pub fn from_phrase(phrase: &str) -> Result<RecoverySeed> {
        let list = words();
        let typed: Vec<String> = phrase.split_whitespace().map(|w| w.to_lowercase()).collect();
        if typed.len() != 24 {
            return Err(VaultError::bad_request("bad_recovery_phrase", "a recovery phrase has 24 words"));
        }
        let mut bits = [0u8; 33];
        for (i, w) in typed.iter().enumerate() {
            let idx = list.iter().position(|x| x == w).ok_or_else(|| {
                VaultError::bad_request("bad_recovery_phrase", format!("word {} isn't a recovery word", i + 1))
            })?;
            for b in 0..11 {
                if (idx >> (10 - b)) & 1 == 1 {
                    let bit = i * 11 + b;
                    bits[bit / 8] |= 1 << (7 - (bit % 8));
                }
            }
        }
        let mut entropy = [0u8; 32];
        entropy.copy_from_slice(&bits[..32]);
        let ok = sha256(&entropy)[0] == bits[32];
        bits.zeroize();
        if !ok {
            entropy.zeroize();
            return Err(VaultError::bad_request("bad_recovery_phrase", "the phrase's checksum doesn't match"));
        }
        Ok(RecoverySeed(Key32::from_array(entropy)))
    }

    /// The age identity derived from this seed.
    pub fn identity(&self) -> Identity {
        Identity::from_scalar(hkdf32(self.0.as_bytes(), &[], INFO_RECOVERY))
    }

    /// Word `n` (1-based) of the phrase, for the type-back check.
    pub fn word(&self, n: usize) -> Option<String> {
        let p = self.phrase();
        let s = p.expose_str()?;
        s.split(' ').nth(n.checked_sub(1)?).map(str::to_string)
    }
}

/// The words a person types back after writing the phrase down (§7.4).
pub const CHECK_WORDS: [usize; 3] = [3, 11, 20];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wordlist_is_the_bip39_english_list() {
        let w = words();
        assert_eq!(w.len(), 2048);
        assert_eq!(w[0], "abandon");
        assert_eq!(w[2047], "zoo");
    }

    #[test]
    fn bip39_test_vector_all_zero_entropy() {
        // Trezor vector: 256-bit zero entropy.
        let seed = RecoverySeed::from_entropy(&[0u8; 32]);
        let p = seed.phrase();
        let expected = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";
        assert_eq!(p.expose_str().expect("utf8"), expected);
    }

    #[test]
    fn bip39_test_vector_7f() {
        let seed = RecoverySeed::from_entropy(&[0x7f; 32]);
        let expected = "legal winner thank year wave sausage worth useful legal winner thank year wave sausage worth useful legal winner thank year wave sausage worth title";
        assert_eq!(seed.phrase().expose_str().expect("utf8"), expected);
    }

    #[test]
    fn phrase_round_trip_and_checksum() {
        let s = RecoverySeed::generate();
        let p = s.phrase();
        let back = RecoverySeed::from_phrase(p.expose_str().expect("utf8")).expect("parses");
        assert_eq!(back.identity().recipient(), s.identity().recipient());
        let mut words: Vec<String> = p.expose_str().expect("utf8").split(' ').map(String::from).collect();
        words.swap(0, 1);
        if words[0] != words[1] {
            let e = RecoverySeed::from_phrase(&words.join(" "));
            // A swap usually breaks the checksum; when it doesn't, the
            // identity must differ.
            if let Ok(other) = e {
                assert_ne!(other.identity().recipient(), s.identity().recipient());
            }
        }
        assert!(RecoverySeed::from_phrase("abandon zoo").is_err());
    }

    #[test]
    fn identity_derivation_vector_is_stable() {
        // Test vector: zero entropy → fixed recipient. If this changes,
        // every recovery key ever shown stops working.
        let seed = RecoverySeed::from_entropy(&[0u8; 32]);
        let r1 = seed.identity().recipient().encode();
        let r2 = RecoverySeed::from_entropy(&[0u8; 32]).identity().recipient().encode();
        assert_eq!(r1, r2);
        assert_eq!(r1, RECOVERY_VECTOR_ZERO.trim());
        // The HKDF step, computed independently (Python hmac/hashlib).
        let scalar = crate::crypto::hex(seed.identity().scalar().as_bytes());
        assert_eq!(scalar, "0e2570b162faed616b4d3e2b807f34db0bbdb48ff990af4963e5de6d54e47e53");
        // And the X25519 step against the `age` crate.
        let enc = seed.identity().encode();
        let age_id: age::x25519::Identity = enc.expose_str().expect("utf8").parse().expect("age parses");
        assert_eq!(age_id.to_public().to_string(), r1);
    }

    /// Recipient for the all-zero seed under `k2-vault/v1 recovery`.
    const RECOVERY_VECTOR_ZERO: &str = include_str!("../data/recovery-vector-zero.txt");
}
