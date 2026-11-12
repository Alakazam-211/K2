//! The relying party's check (PRD §7), in this order:
//!
//! 1. the record's own form (`record`);
//! 2. the bytes: the relying party's stored copy when it has one, and the
//!    record's bytes must equal it (`payload_mismatch`); then the payload's
//!    strict form and fields;
//! 3. the key, among the keys trusted (`unknown_key`), of the record's
//!    algorithm (`alg_mismatch`), and the signature over the bytes
//!    (`bad_signature`);
//! 4. the key is one the payload accepts (`key_not_listed`) and belongs to
//!    the person it names (`wrong_approver`); `rp` and kind are the relying
//!    party's (`wrong_rp`, `wrong_kind`); the pinned check and view meet
//!    its policy (`weak_check`, `weak_view`, `not_attested`);
//! 5. time by the relying party's clock (`issued_future`, `expired`), the
//!    nonce (`wrong_nonce`), and single use (`nonce_used`, through the
//!    relying party's [`NonceStore`]).
//!
//! Only then may it act, after re-checking the world.

use crate::keys::TrustedKey;
use crate::payload::{self, Payload};
use crate::record::{self, Record};
use crate::{refuse, Alg, Check, Code, Environment, Result, View};
use std::path::PathBuf;

/// How far ahead of the relying party's clock `issued` may be.
pub const ISSUED_SKEW_SECS: i64 = 300;

/// What a relying party requires. `None` skips that check; a real relying
/// party sets `rp`, `kind`, `stored_payload` and `nonce`.
#[derive(Debug, Clone)]
pub struct Require<'a> {
    pub rp: Option<&'a str>,
    pub kind: Option<&'a str>,
    /// The weakest check accepted.
    pub check: Check,
    /// The weakest view accepted (page < host < device).
    pub view: View,
    /// The weakest enrolment proof accepted: None takes any key, Some(env)
    /// needs an App Attest attestation at least that strong (development <
    /// production). Releases need production.
    pub attest: Option<Environment>,
    /// The relying party's clock, seconds since 1970 UTC.
    pub now: i64,
    /// The bytes the relying party wrote and kept for this request.
    pub stored_payload: Option<&'a [u8]>,
    /// The nonce the relying party expects.
    pub nonce: Option<&'a str>,
}

/// An approval that passed. Its check and view are the pinned key's.
#[derive(Debug, Clone)]
pub struct Verified {
    pub key_name: String,
    pub person: String,
    pub key_id: String,
    pub alg: Alg,
    pub check: Check,
    pub view: View,
    pub attest: Option<Environment>,
    pub payload: Payload,
}

/// A signature over `msg` by `key` (ES256: DER ECDSA over SHA-256(msg)).
pub fn signature_ok(key: &crate::keys::PublicKey, msg: &[u8], sig: &[u8]) -> bool {
    use aws_lc_rs::signature::{UnparsedPublicKey, ECDSA_P256_SHA256_ASN1, RSA_PKCS1_2048_8192_SHA256};
    match key.alg {
        Alg::Es256 => UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, &key.raw).verify(msg, sig).is_ok(),
        Alg::Rs256 => UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, &key.raw).verify(msg, sig).is_ok(),
    }
}

/// Check a record (its JSON bytes). Pure: it marks nothing used.
pub fn verify(record_json: &[u8], trusted: &[TrustedKey], req: &Require) -> Result<Verified> {
    let rec = record::parse(record_json)?;
    verify_record(&rec, trusted, req)
}

pub fn verify_record(rec: &Record, trusted: &[TrustedKey], req: &Require) -> Result<Verified> {
    let bytes: &[u8] = match req.stored_payload {
        Some(stored) => {
            if rec.payload != stored {
                return refuse(Code::PayloadMismatch, "the record's bytes are not the ones this relying party stored");
            }
            stored
        }
        None => &rec.payload,
    };
    let p = payload::parse(bytes)?;
    let Some(k) = trusted.iter().find(|k| k.key_id == rec.key_id) else {
        return refuse(Code::UnknownKey, format!("key {} is not one this relying party trusts", rec.key_id));
    };
    if k.key.alg != rec.alg {
        return refuse(Code::AlgMismatch, format!("the record says {} but key {} is {}", rec.alg.as_str(), k.name, k.key.alg.as_str()));
    }
    if !signature_ok(&k.key, bytes, &rec.sig) {
        return refuse(Code::BadSignature, format!("the signature does not verify with key {}", k.name));
    }
    if !p.approver_keys.contains(&k.key_id) {
        return refuse(Code::KeyNotListed, format!("key {} is not in the payload's approver_keys", k.name));
    }
    if p.approver != k.person {
        return refuse(Code::WrongApprover, format!("key {} is {}'s, but the payload asks {}", k.name, k.person, p.approver));
    }
    if let Some(rp) = req.rp {
        if p.rp != rp {
            return refuse(Code::WrongRp, format!("the payload is for {}, not {rp}", p.rp));
        }
    }
    if let Some(kind) = req.kind {
        if p.kind != kind {
            return refuse(Code::WrongKind, format!("the payload is a {}, not a {kind}", p.kind));
        }
    }
    if k.check < req.check {
        return refuse(Code::WeakCheck, format!("key {} is a {} key; {} is required", k.name, k.check.as_str(), req.check.as_str()));
    }
    if k.view < req.view {
        return refuse(Code::WeakView, format!("key {} has view {}; {} is required", k.name, k.view.as_str(), req.view.as_str()));
    }
    if k.attest < req.attest {
        return refuse(
            Code::NotAttested,
            format!("key {} was attested as {}; {} is required", k.name, k.attest.map_or("none", Environment::as_str), req.attest.map_or("none", Environment::as_str)),
        );
    }
    if p.issued_at > req.now + ISSUED_SKEW_SECS {
        return refuse(Code::IssuedFuture, format!("issued {} is ahead of this clock", p.issued));
    }
    if req.now > p.answer_by_at {
        return refuse(Code::Expired, format!("answer_by {} has passed", p.answer_by));
    }
    if let Some(n) = req.nonce {
        if p.nonce != n {
            return refuse(Code::WrongNonce, format!("the payload's nonce is {}, not {n}", p.nonce));
        }
    }
    Ok(Verified { key_name: k.name.clone(), person: k.person.clone(), key_id: k.key_id.clone(), alg: k.key.alg, check: k.check, view: k.view, attest: k.attest, payload: p })
}

/// The relying party's record of used nonces.
pub trait NonceStore {
    /// Mark `nonce` used. Ok(false) when it already was.
    fn consume(&mut self, nonce: &str) -> std::io::Result<bool>;
}

/// [`verify`], then mark the nonce used: a second call with the same nonce
/// is refused (`nonce_used`).
pub fn verify_once(record_json: &[u8], trusted: &[TrustedKey], req: &Require, store: &mut dyn NonceStore) -> Result<Verified> {
    let v = verify(record_json, trusted, req)?;
    match store.consume(&v.payload.nonce) {
        Ok(true) => Ok(v),
        Ok(false) => refuse(Code::NonceUsed, format!("nonce {} was already used", v.payload.nonce)),
        Err(e) => refuse(Code::NonceUsed, format!("can't record nonce {} as used ({e}); refusing", v.payload.nonce)),
    }
}

/// Used nonces as empty files in a folder (created exclusively).
pub struct DirNonceStore(pub PathBuf);

impl NonceStore for DirNonceStore {
    fn consume(&mut self, nonce: &str) -> std::io::Result<bool> {
        if !payload::is_hex_lower(nonce, 32) {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a nonce"));
        }
        match std::fs::OpenOptions::new().write(true).create_new(true).open(self.0.join(nonce)) {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
            Err(e) => Err(e),
        }
    }
}

/// Nonces held in memory (tests, short-lived relying parties).
#[derive(Default)]
pub struct MemNonceStore(pub std::collections::HashSet<String>);

impl NonceStore for MemNonceStore {
    fn consume(&mut self, nonce: &str) -> std::io::Result<bool> {
        Ok(self.0.insert(nonce.to_string()))
    }
}
