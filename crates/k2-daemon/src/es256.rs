//! Shared ES256 (P-256) helpers for the daemon's JWT signers: the
//! host-session capability key (`v1_capabilities`) and the chat-pass
//! keyring (`chat_pass`).
//!
//! - [`random_secret`]: a CSPRNG P-256 secret (rejection-sampled `getrandom`).
//! - [`private_pem`]: PKCS#8 PEM for a secret.
//! - [`jwk_from_private_pem`]: the public JWK and its `kid`.
//!
//! `kid` rule: lowercase hex of the first 8 bytes of SHA-256(x ‖ y), so
//! 16 hex characters. JWK shape: `{kty, crv, x, y, use, alg, kid}`. Only
//! public material ever leaves these helpers in a JWK.

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use base64::Engine;
use p256::elliptic_curve::sec1::ToEncodedPoint;
use p256::pkcs8::{DecodePrivateKey, EncodePrivateKey, LineEnding};
use p256::SecretKey;
use sha2::{Digest, Sha256};

/// CSPRNG P-256 secret via `getrandom`.
pub fn random_secret() -> Result<SecretKey, String> {
    // Rejection sampling until the 32-byte seed is a valid scalar.
    for _ in 0..64 {
        let mut bytes = [0u8; 32];
        getrandom::getrandom(&mut bytes).map_err(|e| format!("os rng unavailable for a P-256 key: {e}"))?;
        if let Ok(sk) = SecretKey::from_slice(&bytes) {
            return Ok(sk);
        }
    }
    Err("failed to sample a valid P-256 secret key".into())
}

/// PKCS#8 PEM (LF line endings) for `secret`.
pub fn private_pem(secret: &SecretKey) -> Result<String, String> {
    secret
        .to_pkcs8_pem(LineEnding::LF)
        .map(|p| p.to_string())
        .map_err(|e| format!("encode P-256 key as PKCS#8 PEM: {e}"))
}

/// `(kid, public JWK)` for a PKCS#8 P-256 private key PEM.
pub fn jwk_from_private_pem(pem: &str) -> Result<(String, serde_json::Value), String> {
    let secret = SecretKey::from_pkcs8_pem(pem).map_err(|e| format!("parse P-256 private key: {e}"))?;
    let public = secret.public_key();
    let point = public.to_encoded_point(false);
    let x = point.x().ok_or_else(|| "P-256 public point missing x".to_string())?;
    let y = point.y().ok_or_else(|| "P-256 public point missing y".to_string())?;
    let mut hasher = Sha256::new();
    hasher.update(x.as_slice());
    hasher.update(y.as_slice());
    let digest = hasher.finalize();
    let kid = bytes_to_hex(&digest[..8]);
    let jwk = serde_json::json!({
        "kty": "EC",
        "crv": "P-256",
        "x": B64URL.encode(x.as_slice()),
        "y": B64URL.encode(y.as_slice()),
        "use": "sig",
        "alg": "ES256",
        "kid": kid,
    });
    Ok((kid, jwk))
}

/// Lowercase hex.
pub fn bytes_to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn jwk_is_public_only_and_kid_is_16_hex() {
        let sk = super::random_secret().expect("secret");
        let pem = super::private_pem(&sk).expect("pem");
        let (kid, jwk) = super::jwk_from_private_pem(&pem).expect("jwk");
        assert_eq!(kid.len(), 16);
        assert!(kid.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)), "{kid}");
        let obj = jwk.as_object().expect("object");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["alg", "crv", "kid", "kty", "use", "x", "y"]);
        assert_eq!(jwk["kid"], kid);
    }
}
