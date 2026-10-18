//! The signed vault-recipient statement (prd-agent-secret-handoff-v1 §5.2
//! step 0, vs-live H3).
//!
//! A server answers a pinned, Trusted peer's signed request with the age
//! recipient of its vault, signed by its federation key (ECDSA P-256) and
//! bound to the requester's fingerprint. Same shape as the address notice
//! (a recipient-bound statement under its own signing prefix). The sender
//! checks it against the PEM it pinned at pairing, then seals the secret
//! to that recipient. The federation key can't encrypt (H1); it only
//! vouches for the vault's key.

use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{EcdsaKeyPair, UnparsedPublicKey, ECDSA_P256_SHA256_ASN1, ECDSA_P256_SHA256_ASN1_SIGNING};
use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use rcgen::KeyPair;
use serde::{Deserialize, Serialize};

use super::peers::{fingerprint_of_spki_der, fingerprint_of_spki_pem, raw_point_from_spki_der, spki_pem_to_der};

/// Domain separation for the signature.
pub const SIG_PREFIX: &str = "k2-federation-vault-recipient-v1\n";
/// Statement version.
pub const VERSION: u32 = 1;
/// The `Custom` signal kind of the signed request.
pub const REQUEST_KIND: &str = "vault-recipient/1";
/// The `Custom` signal kind of a sealed handoff.
pub const HANDOFF_KIND: &str = "vault-handoff/1";
/// A statement is fresh for this long either side of now.
pub const MAX_SKEW_SECS: i64 = 600;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Statement {
    pub v: u32,
    /// The answering server's federation fingerprint.
    pub fingerprint: String,
    /// The server that asked (a statement is refused by anyone else).
    pub to_fingerprint: String,
    pub vault_id: String,
    /// `age1…`
    pub recipient: String,
    pub features: Vec<String>,
    /// RFC 3339.
    pub issued_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedStatement {
    pub statement: Statement,
    pub sig: String,
}

fn signed_bytes(s: &Statement) -> Result<Vec<u8>, String> {
    let json = serde_json::to_vec(s).map_err(|e| format!("encode statement: {e}"))?;
    let mut out = SIG_PREFIX.as_bytes().to_vec();
    out.extend_from_slice(&json);
    Ok(out)
}

/// Sign with `key`; the statement's fingerprint must be the key's.
pub fn sign(s: &Statement, key: &KeyPair) -> Result<SignedStatement, String> {
    let fp = fingerprint_of_spki_der(&key.public_key_der());
    if s.fingerprint != fp {
        return Err(format!("statement names {} but the key is {fp}", s.fingerprint));
    }
    let bytes = signed_bytes(s)?;
    let signing = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &key.serialize_der())
        .map_err(|e| format!("load signing key: {e}"))?;
    let sig = signing.sign(&SystemRandom::new(), &bytes).map_err(|e| format!("sign statement: {e}"))?;
    Ok(SignedStatement { statement: s.clone(), sig: base64::engine::general_purpose::STANDARD.encode(sig.as_ref()) })
}

/// Why a statement was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatementError {
    BadSignature,
    WrongSigner,
    WrongRecipient,
    UnsupportedVersion(u32),
    NoHandoff,
    Stale,
    Key(String),
}

impl std::fmt::Display for StatementError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadSignature => write!(f, "the statement's signature doesn't match the pinned key"),
            Self::WrongSigner => write!(f, "the statement names another server"),
            Self::WrongRecipient => write!(f, "the statement was made for another server"),
            Self::UnsupportedVersion(v) => write!(f, "unsupported statement version {v}"),
            Self::NoHandoff => write!(f, "that server doesn't take vault handoffs"),
            Self::Stale => write!(f, "the statement is too old or from the future"),
            Self::Key(e) => write!(f, "pinned key: {e}"),
        }
    }
}

/// Verify against the PEM pinned for `peer_fp`, for this server
/// (`local_fp`), at `now`. Requires the `vault-handoff/1` feature.
pub fn verify(
    signed: &SignedStatement,
    pinned_pem: &str,
    peer_fp: &str,
    local_fp: &str,
    feature: &str,
    now: DateTime<Utc>,
) -> Result<(), StatementError> {
    let s = &signed.statement;
    if s.v != VERSION {
        return Err(StatementError::UnsupportedVersion(s.v));
    }
    let pinned_fp = fingerprint_of_spki_pem(pinned_pem).map_err(StatementError::Key)?;
    if s.fingerprint != pinned_fp || s.fingerprint != peer_fp {
        return Err(StatementError::WrongSigner);
    }
    let der = spki_pem_to_der(pinned_pem).map_err(StatementError::Key)?;
    let point = raw_point_from_spki_der(&der).map_err(StatementError::Key)?;
    let sig = base64::engine::general_purpose::STANDARD.decode(signed.sig.as_bytes()).map_err(|_| StatementError::BadSignature)?;
    let bytes = signed_bytes(s).map_err(|_| StatementError::BadSignature)?;
    UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, point).verify(&bytes, &sig).map_err(|_| StatementError::BadSignature)?;
    if local_fp.is_empty() || s.to_fingerprint != local_fp {
        return Err(StatementError::WrongRecipient);
    }
    if !s.features.iter().any(|f| f == feature) {
        return Err(StatementError::NoHandoff);
    }
    let at = DateTime::parse_from_rfc3339(&s.issued_at).map_err(|_| StatementError::Stale)?.with_timezone(&Utc);
    if (now - at).num_seconds().abs() > Duration::seconds(MAX_SKEW_SECS).num_seconds() {
        return Err(StatementError::Stale);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> KeyPair {
        KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap()
    }

    fn statement(k: &KeyPair, to: &str) -> Statement {
        Statement {
            v: 1,
            fingerprint: fingerprint_of_spki_der(&k.public_key_der()),
            to_fingerprint: to.into(),
            vault_id: "vault-1".into(),
            recipient: "age1example".into(),
            features: vec!["vault-handoff/1".into()],
            issued_at: Utc::now().to_rfc3339(),
        }
    }

    #[test]
    fn a_signed_statement_verifies_only_for_its_signer_recipient_and_feature() {
        let (k, other) = (key(), key());
        let fp = fingerprint_of_spki_der(&k.public_key_der());
        let signed = sign(&statement(&k, "me"), &k).unwrap();
        verify(&signed, &k.public_key_pem(), &fp, "me", "vault-handoff/1", Utc::now()).expect("verifies");
        // Another pinned key.
        let ofp = fingerprint_of_spki_der(&other.public_key_der());
        assert_eq!(verify(&signed, &other.public_key_pem(), &ofp, "me", "vault-handoff/1", Utc::now()), Err(StatementError::WrongSigner));
        // Made for another server.
        assert_eq!(verify(&signed, &k.public_key_pem(), &fp, "you", "vault-handoff/1", Utc::now()), Err(StatementError::WrongRecipient));
        // A tampered recipient.
        let mut t = signed.clone();
        t.statement.recipient = "age1attacker".into();
        assert_eq!(verify(&t, &k.public_key_pem(), &fp, "me", "vault-handoff/1", Utc::now()), Err(StatementError::BadSignature));
        // No handoff feature, and stale.
        let mut s = statement(&k, "me");
        s.features.clear();
        let ns = sign(&s, &k).unwrap();
        assert_eq!(verify(&ns, &k.public_key_pem(), &fp, "me", "vault-handoff/1", Utc::now()), Err(StatementError::NoHandoff));
        assert_eq!(
            verify(&signed, &k.public_key_pem(), &fp, "me", "vault-handoff/1", Utc::now() + Duration::hours(1)),
            Err(StatementError::Stale)
        );
        // A key can't sign for another fingerprint.
        assert!(sign(&statement(&k, "me"), &other).is_err());
    }
}
