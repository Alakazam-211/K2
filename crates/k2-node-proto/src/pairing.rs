//! Enroll codes and the short authentication string (CN1).
//!
//! **Why not federation's `sas_code`.** That one hashes only the two
//! fingerprints, so an active man in the middle can grind keys offline
//! until the 6-digit codes match. Here:
//!
//! 1. The joiner (node) learns the controller's fingerprint before it
//!    connects: the enroll string carries its first 16 hex characters
//!    ([`EnrollString`]), and the node refuses a controller whose full
//!    SPKI fingerprint doesn't start with them. That alone stops the
//!    middle: it would need a P-256 key with a chosen 64-bit prefix.
//! 2. The node sends its public key first; the controller picks a fresh
//!    16-byte `enroll_nonce` only after that (commit, then nonce), and
//!    signs `(node_fp, enroll_nonce, code binding)` with its key.
//! 3. The SAS mixes both fingerprints, the nonce and the hash of the
//!    one-time code ([`code_binding`]). It is defence in depth, shown on
//!    both ends and confirmed by a human on the controller.
//!
//! [`sas_digits`] is domain-separated and generic so federation pairing
//! can reuse it with its own domain string (SEC-1) without sharing codes.

use crate::crypto::{ct_eq, random_bytes, sha256, signed_bytes};

/// Crockford base32: no I, L, O, U.
pub const CODE_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// Characters in an enroll code (10 × 5 bits = 50 bits).
pub const CODE_LEN: usize = 10;
/// Hex characters of the controller fingerprint carried up front.
pub const FP_PREFIX_LEN: usize = 16;
/// Domain of the compute SAS.
pub const SAS_DOMAIN_COMPUTE: &str = "k2-compute-sas-v1";
const CODE_BINDING_DOMAIN: &str = "k2-compute-code-v1";

/// A new random enroll code, formatted `XXXXX-XXXXX`.
pub fn new_code() -> Result<String, String> {
    let raw = random_bytes(CODE_LEN)?;
    let s: String = raw
        .iter()
        .map(|b| CODE_ALPHABET[(b & 0x1f) as usize] as char)
        .collect();
    Ok(format_code(&s))
}

/// `ABCDEFGHJK` → `ABCDE-FGHJK`.
pub fn format_code(normalized: &str) -> String {
    if normalized.len() == CODE_LEN {
        format!("{}-{}", &normalized[..5], &normalized[5..])
    } else {
        normalized.to_string()
    }
}

/// Upper-case, drop `-` and spaces, read `O` as `0` and `I`/`L` as `1`.
/// `None` when the result isn't exactly [`CODE_LEN`] alphabet characters.
pub fn normalize_code(input: &str) -> Option<String> {
    let mut out = String::with_capacity(CODE_LEN);
    for c in input.chars() {
        let c = c.to_ascii_uppercase();
        let c = match c {
            '-' | ' ' => continue,
            'O' => '0',
            'I' | 'L' => '1',
            other => other,
        };
        if !CODE_ALPHABET.contains(&(c as u8)) || !c.is_ascii() {
            return None;
        }
        out.push(c);
    }
    (out.len() == CODE_LEN).then_some(out)
}

/// The hash of a normalized code that the SAS and the enroll signature
/// bind to. The code itself never goes into a signature or a log.
pub fn code_binding(normalized: &str) -> String {
    crate::crypto::hex(&sha256(&signed_bytes(CODE_BINDING_DOMAIN, &[normalized.as_bytes()])))
}

/// What the owner hands to the node: the one-time code plus the first 16
/// hex characters of the controller's fingerprint, written
/// `ABCDE-FGHJK.0123456789abcdef`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrollString {
    /// Normalized code (no dash).
    pub code: String,
    /// Lowercase hex, [`FP_PREFIX_LEN`] characters.
    pub controller_fp_prefix: String,
}

impl EnrollString {
    pub fn new(code: &str, controller_fp: &str) -> Option<Self> {
        let code = normalize_code(code)?;
        let fp = controller_fp.trim().to_ascii_lowercase();
        if fp.len() < FP_PREFIX_LEN || !fp.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        Some(Self { code, controller_fp_prefix: fp[..FP_PREFIX_LEN].to_string() })
    }

    pub fn parse(s: &str) -> Option<Self> {
        let (code, fp) = s.trim().split_once('.')?;
        if fp.len() != FP_PREFIX_LEN {
            return None;
        }
        Self::new(code, fp)
    }

    pub fn render(&self) -> String {
        format!("{}.{}", format_code(&self.code), self.controller_fp_prefix)
    }

    /// True when the full controller fingerprint starts with the prefix.
    pub fn matches_controller(&self, full_fp: &str) -> bool {
        let full = full_fp.trim().to_ascii_lowercase();
        full.len() == 64
            && ct_eq(full[..FP_PREFIX_LEN].as_bytes(), self.controller_fp_prefix.as_bytes())
    }
}

/// Six decimal digits from a domain-separated SHA-256 of `parts`.
/// Generic so federation can reuse it under its own domain (SEC-1).
pub fn sas_digits(domain: &str, parts: &[&[u8]]) -> String {
    let d = sha256(&signed_bytes(domain, parts));
    let n = u64::from_be_bytes([d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]]);
    format!("{:06}", n % 1_000_000)
}

/// The compute SAS. Roles are fixed (controller first), so it is not
/// order-independent like federation's.
pub fn compute_sas(controller_fp: &str, node_fp: &str, enroll_nonce_b64: &str, binding: &str) -> String {
    sas_digits(
        SAS_DOMAIN_COMPUTE,
        &[
            controller_fp.as_bytes(),
            node_fp.as_bytes(),
            enroll_nonce_b64.as_bytes(),
            binding.as_bytes(),
        ],
    )
}

/// `482913` → `482 913` for display.
pub fn display_sas(digits: &str) -> String {
    if digits.len() == 6 {
        format!("{} {}", &digits[..3], &digits[3..])
    } else {
        digits.to_string()
    }
}

/// Compare a typed SAS (spaces allowed) with the expected digits in
/// constant time.
pub fn sas_matches(expected: &str, typed: &str) -> bool {
    let t: String = typed.chars().filter(|c| !c.is_whitespace() && *c != '-').collect();
    t.len() == 6 && t.chars().all(|c| c.is_ascii_digit()) && ct_eq(expected.as_bytes(), t.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_ten_alphabet_chars_and_normalize() {
        for _ in 0..200 {
            let c = new_code().unwrap();
            assert_eq!(c.len(), 11);
            assert_eq!(&c[5..6], "-");
            let n = normalize_code(&c).unwrap();
            assert_eq!(n.len(), CODE_LEN);
            assert!(n.bytes().all(|b| CODE_ALPHABET.contains(&b)));
        }
        assert_eq!(normalize_code("abcde-fghjk").as_deref(), Some("ABCDEFGHJK"));
        assert_eq!(normalize_code("0o1il 23456").as_deref(), Some("0011123456"));
        assert_eq!(normalize_code("ABCDE-FGHJ"), None);
        assert_eq!(normalize_code("ABCDE-FGHJU"), None, "U is not in the alphabet");
        assert_eq!(normalize_code("ABCDE-FGHJKX"), None);
    }

    #[test]
    fn enroll_string_round_trip_and_prefix_check() {
        let fp = "0123456789abcdef".repeat(4);
        let e = EnrollString::new("abcde-fghjk", &fp).unwrap();
        assert_eq!(e.render(), "ABCDE-FGHJK.0123456789abcdef");
        assert_eq!(EnrollString::parse(&e.render()).unwrap(), e);
        assert!(e.matches_controller(&fp));
        let mut other = fp.clone();
        other.replace_range(15..16, "0");
        assert!(!e.matches_controller(&other));
        assert!(!e.matches_controller("0123456789abcdef"), "a short fp never matches");
        assert!(EnrollString::parse("ABCDE-FGHJK.0123").is_none());
        assert!(EnrollString::parse("ABCDE-FGHJK").is_none());
    }

    #[test]
    fn sas_depends_on_every_input() {
        let base = compute_sas("c", "n", "nonce", "bind");
        assert_eq!(base.len(), 6);
        assert_eq!(base, compute_sas("c", "n", "nonce", "bind"));
        assert_ne!(base, compute_sas("n", "c", "nonce", "bind"), "roles are not symmetric");
        assert_ne!(base, compute_sas("c", "n", "nonce2", "bind"));
        assert_ne!(base, compute_sas("c", "n", "nonce", "bind2"));
        // A federation domain over the same parts gives another code.
        assert_ne!(base, sas_digits("k2-federation-sas-v2", &[b"c", b"n", b"nonce", b"bind"]));
    }

    /// Pinned vector: both ends (controller in k2-core, node in k2-node)
    /// link this crate, and this vector pins the function so a change is
    /// a reviewed, versioned protocol change.
    #[test]
    fn sas_vector_is_pinned() {
        let fp_c = "a".repeat(64);
        let fp_n = "b".repeat(64);
        let bind = code_binding("ABCDEFGHJK");
        assert_eq!(bind.len(), 64);
        let sas = compute_sas(&fp_c, &fp_n, "AAAAAAAAAAAAAAAAAAAAAA==", &bind);
        assert_eq!(sas, compute_sas(&fp_c, &fp_n, "AAAAAAAAAAAAAAAAAAAAAA==", &bind));
        assert_eq!(sas, PINNED_SAS, "SAS function changed: bump PROTOCOL and update this vector");
    }
    const PINNED_SAS: &str = "501975";

    #[test]
    fn sas_matches_ignores_spaces_only() {
        assert!(sas_matches("482913", "482 913"));
        assert!(sas_matches("482913", "482-913"));
        assert!(!sas_matches("482913", "482914"));
        assert!(!sas_matches("482913", "48291"));
        assert!(!sas_matches("482913", "48291x"));
        assert_eq!(display_sas("482913"), "482 913");
    }
}
