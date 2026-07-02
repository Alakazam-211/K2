//! Signed-envelope seal/open — pure ECDSA P-256 sign-on-send /
//! verify-against-pinned-key-on-receive of an
//! [`AgentSignal`](crate::awareness::AgentSignal).
//!
//! Phase 1 of `prd-cross-server-agent-comms.md`. These are **pure functions**
//! with no I/O and no network: [`seal`] wraps a signal in a
//! [`FederationEnvelope`] signed by the daemon's EXISTING tunnel keypair
//! ([`crate::tunnel::tls::load_or_generate_keypair`]); [`open`] verifies an
//! envelope against a **pinned** peer SPKI and returns the inner signal.
//!
//! ## What "verify" enforces (Phase 1)
//!
//!   1. The envelope parses as well-formed JSON.
//!   2. The claimed `from_fingerprint` **equals the fingerprint of the pinned
//!      key** we were told to verify against (the "selector": you can only open
//!      an envelope against the key it claims to come from — DECISION-2 / Risk
//!      M4). A mismatch is rejected before any crypto.
//!   3. The ECDSA signature validates over the canonical JSON of the payload
//!      against the pinned public key. Tamper / wrong key → reject.
//!
//! Replay/skew/TTL/loop enforcement, trust-state lookup, and capability gating
//! are **NOT** here — those belong to the Phase-3 ingress handler, which calls
//! [`crate::federation::peers::PeerStore::require_peer`] AFTER `open` succeeds.
//! `open` proves *authenticity*, not *authorization*.

use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{
    EcdsaKeyPair, UnparsedPublicKey, ECDSA_P256_SHA256_ASN1, ECDSA_P256_SHA256_ASN1_SIGNING,
};
use base64::Engine;
use chrono::{DateTime, Utc};
use rcgen::KeyPair;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::awareness::AgentSignal;

use super::peers::{fingerprint_of_spki_pem, raw_point_from_spki_der, spki_pem_to_der};

/// The signed body. The signature is computed over the **canonical JSON** of
/// this struct; `open` re-derives the same bytes to verify. Field set mirrors
/// the PRD's `SignedPayload` (replay/loop fields carried for forward-compat;
/// they are not *enforced* until the Phase-3 ingress handler).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SignedPayload {
    /// Claimed sender id = SHA-256(SPKI) hex. Verified against the pinned key.
    pub from_fingerprint: String,
    /// Routing hint → `<to_subdomain>.k2.dev`. A hint only.
    pub to_subdomain: String,
    /// Idempotency / dedup key (Phase-3 ingress dedups on this).
    pub msg_uuid: String,
    /// Hop budget (Phase-3 drops on 0). Carried, not enforced, in Phase 1.
    pub ttl: u8,
    /// Daemon-fingerprint chain for loop detection (Phase-3). Seeded with the
    /// sender's own fingerprint at seal time.
    pub trace: Vec<String>,
    /// Per-message nonce (Phase-3 replay LRU). Carried, not enforced here.
    pub nonce: String,
    /// Send timestamp (Phase-3 skew window). Carried, not enforced here.
    pub at: DateTime<Utc>,
    /// The reused structured signal. On the recipient the rendered `from` is
    /// re-derived from the verified peer, never trusted from these bytes.
    pub signal: AgentSignal,
}

/// The wire envelope: a signed payload plus its detached base64 ECDSA-P256
/// signature (ASN.1/DER encoded).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FederationEnvelope {
    pub payload: SignedPayload,
    /// Base64(ASN.1-DER ECDSA-P256 signature over `canonical_json(payload)`).
    pub sig: String,
}

/// Why a seal/open failed. Every `open` failure is a REJECT — there is no
/// "accept on error" path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FederationError {
    /// Could not serialize / sign the payload.
    Encode(String),
    /// Envelope bytes were not well-formed.
    Decode(String),
    /// The pinned key was malformed / not a P-256 SPKI.
    Key(String),
    /// Claimed `from_fingerprint` does not match the pinned key's fingerprint
    /// (the "unknown selector": opening against the wrong key).
    FingerprintMismatch { claimed: String, pinned: String },
    /// Signature did not verify against the pinned key (tamper / wrong signer).
    BadSignature,
}

impl std::fmt::Display for FederationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Encode(e) => write!(f, "encode/sign envelope: {e}"),
            Self::Decode(e) => write!(f, "decode envelope: {e}"),
            Self::Key(e) => write!(f, "pinned key: {e}"),
            Self::FingerprintMismatch { claimed, pinned } => write!(
                f,
                "envelope claims sender {claimed} but pinned key is {pinned}"
            ),
            Self::BadSignature => write!(f, "signature verification failed"),
        }
    }
}

impl std::error::Error for FederationError {}

/// Canonical JSON bytes of a payload — the exact bytes signed/verified. Compact
/// (no whitespace); serde serializes struct fields in declaration order and
/// `serde_json::Value` maps with sorted keys, so this is deterministic for a
/// given payload value (and a serde round-trip of the payload reproduces it).
fn canonical(payload: &SignedPayload) -> Result<Vec<u8>, FederationError> {
    serde_json::to_vec(payload).map_err(|e| FederationError::Encode(e.to_string()))
}

/// Sign `signal` into a [`FederationEnvelope`] with the daemon's keypair
/// `my_key`, addressed to `to_subdomain` with hop budget `ttl`. Returns the
/// serialized envelope bytes (JSON). Pure — no I/O beyond the CSPRNG.
pub fn seal(
    signal: &AgentSignal,
    my_key: &KeyPair,
    to_subdomain: &str,
    ttl: u8,
) -> Result<Vec<u8>, FederationError> {
    let from_fingerprint = super::peers::fingerprint_of_spki_der(&my_key.public_key_der());

    let payload = SignedPayload {
        from_fingerprint: from_fingerprint.clone(),
        to_subdomain: to_subdomain.to_string(),
        msg_uuid: Uuid::new_v4().to_string(),
        ttl,
        trace: vec![from_fingerprint],
        nonce: Uuid::new_v4().to_string(),
        at: Utc::now(),
        signal: signal.clone(),
    };

    let bytes = canonical(&payload)?;

    // rcgen emits a PKCS#8 v1 private key; aws-lc-rs parses it directly.
    let pkcs8 = my_key.serialize_der();
    let signing = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &pkcs8)
        .map_err(|e| FederationError::Encode(format!("load signing key: {e}")))?;
    let rng = SystemRandom::new();
    let sig = signing
        .sign(&rng, &bytes)
        .map_err(|e| FederationError::Encode(format!("sign: {e}")))?;
    let sig_b64 = base64::engine::general_purpose::STANDARD.encode(sig.as_ref());

    let envelope = FederationEnvelope {
        payload,
        sig: sig_b64,
    };
    serde_json::to_vec(&envelope).map_err(|e| FederationError::Encode(e.to_string()))
}

/// Re-sign a previously-sealed envelope with a FRESH `at` timestamp and a
/// FRESH `nonce`, preserving everything else — **`msg_uuid` above all** (the
/// end-to-end idempotency key the receive-side dedupe keys on). This is the
/// outbox drain's answer to the skew window: a queued envelope older than
/// [`super::ingress::DEFAULT_SKEW_SECS`] would be rejected as stale on
/// delivery, so the drain re-seals it just before the dial. Only the original
/// sender can do this (it needs the signing key), so a re-sealed envelope is
/// exactly as authentic as the first seal. Pure — no I/O beyond the CSPRNG.
pub fn reseal(envelope_bytes: &[u8], my_key: &KeyPair) -> Result<Vec<u8>, FederationError> {
    let envelope: FederationEnvelope =
        serde_json::from_slice(envelope_bytes).map_err(|e| FederationError::Decode(e.to_string()))?;
    let mut payload = envelope.payload;

    // Refuse to re-sign someone else's envelope: the stored bytes must claim
    // OUR fingerprint (an outbox only ever holds envelopes we sealed, but the
    // check keeps the primitive safe for any caller).
    let my_fp = super::peers::fingerprint_of_spki_der(&my_key.public_key_der());
    if payload.from_fingerprint != my_fp {
        return Err(FederationError::FingerprintMismatch {
            claimed: payload.from_fingerprint,
            pinned: my_fp,
        });
    }

    payload.nonce = Uuid::new_v4().to_string();
    payload.at = Utc::now();

    let bytes = canonical(&payload)?;
    let pkcs8 = my_key.serialize_der();
    let signing = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &pkcs8)
        .map_err(|e| FederationError::Encode(format!("load signing key: {e}")))?;
    let rng = SystemRandom::new();
    let sig = signing
        .sign(&rng, &bytes)
        .map_err(|e| FederationError::Encode(format!("sign: {e}")))?;
    let sig_b64 = base64::engine::general_purpose::STANDARD.encode(sig.as_ref());

    let envelope = FederationEnvelope {
        payload,
        sig: sig_b64,
    };
    serde_json::to_vec(&envelope).map_err(|e| FederationError::Encode(e.to_string()))
}

/// Verify `bytes` as a [`FederationEnvelope`] against `sender_pinned_pubkey_pem`
/// (a pinned P-256 SPKI PEM) and return the inner [`AgentSignal`].
///
/// Rejects on: malformed bytes, a `from_fingerprint` that doesn't match the
/// pinned key, or a signature that doesn't verify (tamper / wrong key). Proves
/// authenticity only; the caller still gates authorization via
/// [`PeerStore::require_peer`](super::peers::PeerStore::require_peer).
pub fn open(bytes: &[u8], sender_pinned_pubkey_pem: &str) -> Result<AgentSignal, FederationError> {
    let envelope: FederationEnvelope =
        serde_json::from_slice(bytes).map_err(|e| FederationError::Decode(e.to_string()))?;

    // Bind the claimed sender to the pinned key BEFORE any crypto: you can only
    // open an envelope against the very key it claims to come from.
    let pinned_fp = fingerprint_of_spki_pem(sender_pinned_pubkey_pem)
        .map_err(FederationError::Key)?;
    if envelope.payload.from_fingerprint != pinned_fp {
        return Err(FederationError::FingerprintMismatch {
            claimed: envelope.payload.from_fingerprint.clone(),
            pinned: pinned_fp,
        });
    }

    let der = spki_pem_to_der(sender_pinned_pubkey_pem).map_err(FederationError::Key)?;
    let raw_point = raw_point_from_spki_der(&der).map_err(FederationError::Key)?;

    let sig = base64::engine::general_purpose::STANDARD
        .decode(envelope.sig.as_bytes())
        .map_err(|e| FederationError::Decode(format!("sig base64: {e}")))?;

    let bytes_signed = canonical(&envelope.payload)?;

    UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, raw_point)
        .verify(&bytes_signed, &sig)
        .map_err(|_| FederationError::BadSignature)?;

    Ok(envelope.payload.signal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::awareness::{AgentAddress, SignalKind, WorkspaceId};
    use crate::tunnel::tls::load_or_generate_keypair;
    use crate::tunnel::test_support::with_temp_home;

    fn sample_signal() -> AgentSignal {
        AgentSignal::new(
            AgentAddress::Agent {
                workspace: WorkspaceId("ws-a".into()),
                name: "alice".into(),
            },
            AgentAddress::Agent {
                workspace: WorkspaceId("ws-b".into()),
                name: "bob".into(),
            },
            SignalKind::Msg {
                text: "hello across the wire".into(),
            },
        )
    }

    #[test]
    fn seal_open_round_trip() {
        with_temp_home(|| {
            let my_key = load_or_generate_keypair().expect("keypair");
            let my_pem = my_key.public_key_pem();
            let signal = sample_signal();

            let bytes = seal(&signal, &my_key, "rosson", 8).expect("seal");
            let opened = open(&bytes, &my_pem).expect("open against pinned key");
            assert_eq!(opened, signal, "round-tripped signal must be identical");
        });
    }

    #[test]
    fn reseal_preserves_msg_uuid_and_signal_with_fresh_nonce_and_at() {
        with_temp_home(|| {
            let my_key = load_or_generate_keypair().expect("keypair");
            let my_pem = my_key.public_key_pem();
            let signal = sample_signal();

            let first = seal(&signal, &my_key, "rosson", 8).expect("seal");
            let old: FederationEnvelope = serde_json::from_slice(&first).unwrap();

            let second = reseal(&first, &my_key).expect("reseal");
            let new: FederationEnvelope = serde_json::from_slice(&second).unwrap();

            // The idempotency key + signal survive; freshness fields rotate.
            assert_eq!(new.payload.msg_uuid, old.payload.msg_uuid, "msg_uuid must be preserved");
            assert_eq!(new.payload.signal, old.payload.signal, "signal must be preserved");
            assert_ne!(new.payload.nonce, old.payload.nonce, "nonce must rotate");
            assert!(new.payload.at >= old.payload.at, "at must be re-stamped");

            // And the re-signed envelope verifies against the same key.
            let opened = open(&second, &my_pem).expect("resealed envelope must open");
            assert_eq!(opened, signal);
        });
    }

    #[test]
    fn reseal_rejects_foreign_envelope() {
        with_temp_home(|| {
            let my_key = load_or_generate_keypair().expect("keypair");
            let bytes = seal(&sample_signal(), &my_key, "rosson", 8).expect("seal");
            let other = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
                .expect("other key");
            let err = reseal(&bytes, &other).expect_err("must not re-sign a foreign envelope");
            assert!(
                matches!(err, FederationError::FingerprintMismatch { .. }),
                "got: {err}"
            );
        });
    }

    #[test]
    fn open_rejects_tampered_payload() {
        with_temp_home(|| {
            let my_key = load_or_generate_keypair().expect("keypair");
            let my_pem = my_key.public_key_pem();
            let bytes = seal(&sample_signal(), &my_key, "rosson", 8).expect("seal");

            // Mutate the payload (flip the message text) but keep the old sig.
            let mut env: FederationEnvelope = serde_json::from_slice(&bytes).unwrap();
            env.payload.signal.kind = SignalKind::Msg {
                text: "TAMPERED".into(),
            };
            let tampered = serde_json::to_vec(&env).unwrap();

            let err = open(&tampered, &my_pem).expect_err("tamper must be rejected");
            assert_eq!(err, FederationError::BadSignature, "got: {err}");
        });
    }

    #[test]
    fn open_rejects_wrong_unpinned_key() {
        with_temp_home(|| {
            let my_key = load_or_generate_keypair().expect("keypair");
            let bytes = seal(&sample_signal(), &my_key, "rosson", 8).expect("seal");

            // A different key the envelope was NOT signed with. Its fingerprint
            // won't match the claimed sender → rejected before crypto even runs.
            let attacker = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
                .expect("attacker key");
            let err = open(&bytes, &attacker.public_key_pem())
                .expect_err("wrong key must be rejected");
            assert!(
                matches!(err, FederationError::FingerprintMismatch { .. }),
                "expected fingerprint mismatch, got: {err}"
            );
        });
    }

    #[test]
    fn open_rejects_sig_forged_for_wrong_signer() {
        // Forge an envelope that CLAIMS the attacker's fingerprint (so the
        // selector check passes against the attacker's pinned key) but whose sig
        // was made by a different key → signature verification must fail.
        with_temp_home(|| {
            let real_key = load_or_generate_keypair().expect("real keypair");
            let attacker = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
                .expect("attacker key");

            // Seal with the real key, then rewrite from_fingerprint + trace to
            // claim the attacker's identity (sig now matches neither selector
            // re-serialization nor the attacker key).
            let bytes = seal(&sample_signal(), &real_key, "rosson", 8).expect("seal");
            let mut env: FederationEnvelope = serde_json::from_slice(&bytes).unwrap();
            let attacker_fp =
                super::super::peers::fingerprint_of_spki_pem(&attacker.public_key_pem()).unwrap();
            env.payload.from_fingerprint = attacker_fp;
            let forged = serde_json::to_vec(&env).unwrap();

            let err = open(&forged, &attacker.public_key_pem())
                .expect_err("forged sender must be rejected");
            assert_eq!(err, FederationError::BadSignature, "got: {err}");
        });
    }

    #[test]
    fn open_rejects_garbage_bytes() {
        let err = open(b"not json at all", "").expect_err("garbage must be rejected");
        assert!(matches!(err, FederationError::Decode(_)), "got: {err}");
    }

    #[test]
    fn open_rejects_malformed_pinned_key() {
        with_temp_home(|| {
            let my_key = load_or_generate_keypair().expect("keypair");
            let bytes = seal(&sample_signal(), &my_key, "rosson", 8).expect("seal");
            let err = open(&bytes, "-----BEGIN PUBLIC KEY-----\n-----END PUBLIC KEY-----")
                .expect_err("empty pinned key must be rejected");
            assert!(matches!(err, FederationError::Key(_)), "got: {err}");
        });
    }
}
