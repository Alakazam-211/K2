//! Keys, fingerprints and domain-separated signatures (CN2).
//!
//! Both ends use ECDSA P-256 with ASN.1/DER signatures, the same primitive
//! as federation (`envelope.rs`, `roster.rs`), so the controller signs
//! with its existing tunnel key (`~/.k2/tunnel-key.pem`, CN3) and a
//! fingerprint is SHA-256 of the SPKI DER, 64 lowercase hex, exactly like
//! `federation::peers::fingerprint_of_spki_der`.
//!
//! Every signature is over a domain string plus length-prefixed parts
//! ([`signed_bytes`]), so a signature made for one purpose (a handshake,
//! a frame, a receipt, an enrollment) never verifies for another.

use aws_lc_rs::digest;
use aws_lc_rs::encoding::AsDer;
use aws_lc_rs::rand::{SecureRandom, SystemRandom};
use aws_lc_rs::signature::{
    EcdsaKeyPair, KeyPair, UnparsedPublicKey, ECDSA_P256_SHA256_ASN1,
    ECDSA_P256_SHA256_ASN1_SIGNING,
};
use base64::Engine;

/// The fixed DER header of a P-256 SubjectPublicKeyInfo (uncompressed
/// point follows): SEQUENCE { SEQUENCE { id-ecPublicKey, prime256v1 },
/// BIT STRING (0 unused bits) }.
const P256_SPKI_PREFIX: [u8; 26] = [
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08,
    0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
];

pub fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub fn unb64(s: &str) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::STANDARD
        .decode(s.trim().as_bytes())
        .map_err(|e| format!("base64: {e}"))
}

/// Lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// SHA-256 digest bytes.
pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    let d = digest::digest(&digest::SHA256, bytes);
    let mut out = [0u8; 32];
    out.copy_from_slice(d.as_ref());
    out
}

/// SHA-256 as 64 lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&sha256(bytes))
}

/// Streaming SHA-256 (bundles, logs).
pub struct Sha256Stream(digest::Context);

impl Default for Sha256Stream {
    fn default() -> Self {
        Self(digest::Context::new(&digest::SHA256))
    }
}

impl Sha256Stream {
    pub fn update(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }
    pub fn finish_hex(self) -> String {
        hex(self.0.finish().as_ref())
    }
}

/// `n` bytes from the system CSPRNG.
pub fn random_bytes(n: usize) -> Result<Vec<u8>, String> {
    let mut v = vec![0u8; n];
    SystemRandom::new().fill(&mut v).map_err(|_| "system random failed".to_string())?;
    Ok(v)
}

/// A fresh 32-byte nonce, base64.
pub fn nonce_b64() -> Result<String, String> {
    Ok(b64(&random_bytes(32)?))
}

/// A random id: 16 bytes as 32 lowercase hex.
pub fn random_id() -> Result<String, String> {
    Ok(hex(&random_bytes(16)?))
}

/// SHA-256(SPKI DER) as 64 lowercase hex.
pub fn fingerprint_of_spki_der(der: &[u8]) -> String {
    sha256_hex(der)
}

/// Wrap DER in a PEM block with 64-character lines.
pub fn pem_encode(label: &str, der: &[u8]) -> String {
    let body = b64(der);
    let mut out = format!("-----BEGIN {label}-----\n");
    for chunk in body.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(chunk).unwrap_or(""));
        out.push('\n');
    }
    out.push_str(&format!("-----END {label}-----\n"));
    out
}

/// Decode the first PEM block with `label`. Fails loud on a missing or
/// empty body: a malformed pin must never verify as empty.
pub fn pem_decode(label: &str, pem: &str) -> Result<Vec<u8>, String> {
    let begin = format!("-----BEGIN {label}-----");
    let end = format!("-----END {label}-----");
    let start = pem.find(&begin).ok_or_else(|| format!("no {label} PEM block"))? + begin.len();
    let stop = pem[start..].find(&end).ok_or_else(|| format!("unterminated {label} PEM block"))? + start;
    let body: String = pem[start..stop].chars().filter(|c| !c.is_whitespace()).collect();
    if body.is_empty() {
        return Err(format!("empty {label} PEM block"));
    }
    unb64(&body)
}

/// The raw uncompressed point of a P-256 SPKI. Refuses anything else.
pub fn p256_point_from_spki(der: &[u8]) -> Result<&[u8], String> {
    if der.len() != 91 || der[..26] != P256_SPKI_PREFIX || der[26] != 0x04 {
        return Err("not a P-256 SubjectPublicKeyInfo".to_string());
    }
    Ok(&der[26..])
}

/// Fingerprint of a PEM `PUBLIC KEY` (validated as P-256).
pub fn fingerprint_of_spki_pem(pem: &str) -> Result<String, String> {
    let der = pem_decode("PUBLIC KEY", pem)?;
    p256_point_from_spki(&der)?;
    Ok(fingerprint_of_spki_der(&der))
}

/// The exact bytes a domain-separated signature covers:
/// `domain`, a zero byte, then each part as a u32 big-endian length and
/// its bytes. Unambiguous for any parts (no separator can be forged).
pub fn signed_bytes(domain: &str, parts: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(domain.len() + 1 + parts.iter().map(|p| p.len() + 4).sum::<usize>());
    out.extend_from_slice(domain.as_bytes());
    out.push(0);
    for p in parts {
        out.extend_from_slice(&(p.len() as u32).to_be_bytes());
        out.extend_from_slice(p);
    }
    out
}

/// A P-256 signing key (the node key, or the controller's tunnel key).
pub struct SigningKey {
    pair: EcdsaKeyPair,
    spki_der: Vec<u8>,
}

impl std::fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SigningKey({})", self.fingerprint())
    }
}

impl SigningKey {
    /// A new random key.
    pub fn generate() -> Result<Self, String> {
        let pair = EcdsaKeyPair::generate(&ECDSA_P256_SHA256_ASN1_SIGNING)
            .map_err(|_| "generate P-256 key failed".to_string())?;
        Self::from_pair(pair)
    }

    /// From PKCS#8 DER (rcgen's `KeyPair::serialize_der()` is PKCS#8 v1).
    pub fn from_pkcs8_der(der: &[u8]) -> Result<Self, String> {
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, der)
            .map_err(|e| format!("load P-256 PKCS#8 key: {e}"))?;
        Self::from_pair(pair)
    }

    /// From a PEM `PRIVATE KEY` (PKCS#8) block.
    pub fn from_pkcs8_pem(pem: &str) -> Result<Self, String> {
        Self::from_pkcs8_der(&pem_decode("PRIVATE KEY", pem)?)
    }

    fn from_pair(pair: EcdsaKeyPair) -> Result<Self, String> {
        let spki = pair
            .public_key()
            .as_der()
            .map_err(|_| "encode public key failed".to_string())?;
        let spki_der = spki.as_ref().to_vec();
        p256_point_from_spki(&spki_der)?;
        Ok(Self { pair, spki_der })
    }

    /// PKCS#8 v1 PEM of the private key (written 0600 by the caller).
    pub fn to_pkcs8_pem(&self) -> Result<String, String> {
        let doc = self.pair.to_pkcs8v1().map_err(|_| "encode PKCS#8 failed".to_string())?;
        Ok(pem_encode("PRIVATE KEY", doc.as_ref()))
    }

    pub fn spki_der(&self) -> &[u8] {
        &self.spki_der
    }

    pub fn spki_pem(&self) -> String {
        pem_encode("PUBLIC KEY", &self.spki_der)
    }

    pub fn fingerprint(&self) -> String {
        fingerprint_of_spki_der(&self.spki_der)
    }

    /// Sign `parts` under `domain` ([`signed_bytes`]). Base64 DER signature.
    pub fn sign_domain(&self, domain: &str, parts: &[&[u8]]) -> Result<String, String> {
        let msg = signed_bytes(domain, parts);
        let sig = self
            .pair
            .sign(&SystemRandom::new(), &msg)
            .map_err(|_| "sign failed".to_string())?;
        Ok(b64(sig.as_ref()))
    }
}

/// Verify a [`SigningKey::sign_domain`] signature against an SPKI DER.
pub fn verify_domain(spki_der: &[u8], domain: &str, parts: &[&[u8]], sig_b64: &str) -> bool {
    let Ok(point) = p256_point_from_spki(spki_der) else {
        return false;
    };
    let Ok(sig) = unb64(sig_b64) else {
        return false;
    };
    let msg = signed_bytes(domain, parts);
    UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, point)
        .verify(&msg, &sig)
        .is_ok()
}

/// Constant-time equality for short secrets and codes (length leaks;
/// values don't).
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_verify_round_trip_and_domain_separation() {
        let k = SigningKey::generate().unwrap();
        let sig = k.sign_domain("d1", &[b"a", b"bc"]).unwrap();
        assert!(verify_domain(k.spki_der(), "d1", &[b"a", b"bc"], &sig));
        // Other domain, other split of the same bytes, other key: all fail.
        assert!(!verify_domain(k.spki_der(), "d2", &[b"a", b"bc"], &sig));
        assert!(!verify_domain(k.spki_der(), "d1", &[b"ab", b"c"], &sig));
        let other = SigningKey::generate().unwrap();
        assert!(!verify_domain(other.spki_der(), "d1", &[b"a", b"bc"], &sig));
        assert!(!verify_domain(k.spki_der(), "d1", &[b"a", b"bc"], "not base64!"));
    }

    #[test]
    fn pem_round_trip_keeps_identity() {
        let k = SigningKey::generate().unwrap();
        let pem = k.to_pkcs8_pem().unwrap();
        assert!(pem.starts_with("-----BEGIN PRIVATE KEY-----\n"));
        let back = SigningKey::from_pkcs8_pem(&pem).unwrap();
        assert_eq!(back.fingerprint(), k.fingerprint());
        assert_eq!(fingerprint_of_spki_pem(&k.spki_pem()).unwrap(), k.fingerprint());
        assert_eq!(k.fingerprint().len(), 64);
    }

    #[test]
    fn bad_pem_and_bad_spki_fail_loud() {
        assert!(pem_decode("PUBLIC KEY", "-----BEGIN PUBLIC KEY-----\n-----END PUBLIC KEY-----\n").is_err());
        assert!(pem_decode("PUBLIC KEY", "garbage").is_err());
        assert!(p256_point_from_spki(&[0u8; 91]).is_err());
        assert!(!verify_domain(&[1, 2, 3], "d", &[], "AAAA"));
    }

    #[test]
    fn signed_bytes_layout_is_pinned() {
        assert_eq!(
            signed_bytes("x", &[b"ab", b""]),
            vec![b'x', 0, 0, 0, 0, 2, b'a', b'b', 0, 0, 0, 0]
        );
    }

    #[test]
    fn ct_eq_basics() {
        assert!(ct_eq(b"123456", b"123456"));
        assert!(!ct_eq(b"123456", b"123457"));
        assert!(!ct_eq(b"12345", b"123456"));
    }
}
