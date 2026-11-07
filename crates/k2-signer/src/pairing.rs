//! Signer ↔ server pairing (PRD §4.2, vs-live V17, V29): the person at the
//! signing machine pairs it with their K2 server once; the signer pins the
//! server's tunnel-key fingerprint (the `controller_fp` every receipt
//! signs), and the server records the signer's public key.
//!
//! The scheme is `k2_node_proto::pairing`'s (compute enrol), with its own
//! SAS domain `k2-signer-sas-v1` (`ci::DOMAIN_SIGNER_SAS`):
//!
//! 1. **`pair-begin`** (admin, at the signer): a one-time 10-character code
//!    and the signer's fingerprint prefix: `ABCDE-FGHJK.<16 hex>`. The
//!    signer keeps only the code's binding. It prints a [`Hello`] (the
//!    signer's full key) for the server.
//! 2. **`k2 ci signer add <code string>`** (owner/Admin at the server, with a
//!    confirm token, never an agent, V17): the server checks the hello's key
//!    against the fingerprint prefix, and only **after** it has the key picks
//!    a fresh nonce (commit, then nonce). It answers an [`Offer`] signed
//!    with its tunnel key over `k2-signer-pair-offer-v1`, binding the
//!    signer's fp, the nonce and the code's binding (proof it knows the
//!    code), and shows the 6-digit SAS.
//! 3. **`pair-accept <the SAS the server shows>`** (admin, at the signer):
//!    the signer checks the offer's signature and code binding, computes the
//!    SAS itself, and pins the controller only when the person typed the
//!    same six digits. It answers a [`Done`] signed with the signer's key
//!    over `k2-signer-pair-done-v1`, which the server checks to record the
//!    signer (proof of possession).
//!
//! The signer never talks to the network: hello, offer and done travel
//! through the person's `k2` CLI (a thin wrapper) and the server's route.
//! **Server side:** [`server_offer`] and [`server_finish`] are the reference
//! implementation and the interface for the daemon route (builder O).

use k2_node_proto::ci::{Signed, DOMAIN_SIGNER_SAS};
use k2_node_proto::crypto::{self, SigningKey};
use k2_node_proto::pairing::{code_binding, sas_digits, sas_matches, EnrollString};
use serde::{Deserialize, Serialize};

/// The offer's signature domain (server tunnel key).
pub const DOMAIN_PAIR_OFFER: &str = "k2-signer-pair-offer-v1";
/// The done message's signature domain (signer key).
pub const DOMAIN_PAIR_DONE: &str = "k2-signer-pair-done-v1";
/// A pairing code lives this long.
pub const CODE_TTL_SECS: i64 = 600;
pub const PAIR_V: u32 = 1;

/// Signer → server: the signer's key (sent after the person ran `k2 ci
/// signer add <code string>` there).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hello {
    pub v: u32,
    pub kind: String,
    pub signer_fp: String,
    /// base64 P-256 SPKI.
    pub signer_spki: String,
    pub signer_label: String,
}

/// Server → signer (signed with the server's tunnel key).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Offer {
    pub v: u32,
    pub kind: String,
    pub server_fp: String,
    pub server_spki: String,
    pub server_label: String,
    pub signer_fp: String,
    /// base64, 16 random bytes picked after the hello arrived.
    pub nonce: String,
    /// `pairing::code_binding` of the code: proof the server got it from the person.
    pub code_binding: String,
    pub at: i64,
}

/// Signer → server (signed with the signer's key): the person confirmed the SAS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Done {
    pub v: u32,
    pub kind: String,
    pub signer_fp: String,
    pub server_fp: String,
    pub nonce: String,
    pub code_binding: String,
    pub at: i64,
}

/// What the signer keeps between `pair-begin` and `pair-accept`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pending {
    pub code_binding: String,
    pub created_at: i64,
    pub label: String,
}

/// The SAS both ends show (roles fixed: server first).
pub fn sas(server_fp: &str, signer_fp: &str, nonce_b64: &str, binding: &str) -> String {
    sas_digits(DOMAIN_SIGNER_SAS, &[server_fp.as_bytes(), signer_fp.as_bytes(), nonce_b64.as_bytes(), binding.as_bytes()])
}

fn sign<T: Serialize>(key: &SigningKey, domain: &str, doc: &T) -> Result<Signed, String> {
    Signed::sign_node(key, domain, doc)
}

/// Signer: start a pairing. Returns the code string for the person, the
/// hello for the server, and what to keep.
pub fn signer_begin(identity: &SigningKey, label: &str, now: i64) -> Result<(String, Hello, Pending), String> {
    let code = k2_node_proto::pairing::new_code()?;
    let es = EnrollString::new(&code, &identity.fingerprint()).ok_or("could not make the pairing code")?;
    let hello = Hello {
        v: PAIR_V,
        kind: "k2-signer-hello".into(),
        signer_fp: identity.fingerprint(),
        signer_spki: crypto::b64(identity.spki_der()),
        signer_label: label.to_string(),
    };
    let pending = Pending { code_binding: code_binding(&es.code), created_at: now, label: label.to_string() };
    Ok((es.render(), hello, pending))
}

/// Signer: check the server's offer and the SAS the person typed (read off
/// the server's screen). Returns the server's fp, its SPKI, its label, and
/// the signed [`Done`] for the server.
pub fn signer_accept(
    identity: &SigningKey,
    pending: &Pending,
    offer: &Signed,
    typed_sas: &str,
    now: i64,
) -> Result<(String, String, String, Signed), String> {
    if now > pending.created_at + CODE_TTL_SECS {
        return Err("the pairing code expired: start again (pair-begin)".into());
    }
    let peek: Offer = offer.peek(DOMAIN_PAIR_OFFER)?;
    let der = crypto::unb64(&peek.server_spki)?;
    crypto::p256_point_from_spki(&der)?;
    let o: Offer = offer.open_node(&der, DOMAIN_PAIR_OFFER)?;
    if o.v != PAIR_V || o.kind != "k2-signer-offer" {
        return Err("not a k2-signer pairing offer".into());
    }
    if crypto::fingerprint_of_spki_der(&der) != o.server_fp || !k2_node_proto::ci::is_fp(&o.server_fp) {
        return Err("the offer's server key is not its fingerprint".into());
    }
    if o.signer_fp != identity.fingerprint() {
        return Err("the offer is for another signer".into());
    }
    if !crypto::ct_eq(o.code_binding.as_bytes(), pending.code_binding.as_bytes()) {
        return Err("the offer does not know this pairing code (a server the person did not give the code to)".into());
    }
    let nonce = crypto::unb64(&o.nonce)?;
    if nonce.len() != 16 {
        return Err("the offer's nonce is not 16 bytes".into());
    }
    let want = sas(&o.server_fp, &o.signer_fp, &o.nonce, &o.code_binding);
    if !sas_matches(&want, typed_sas) {
        return Err("the SAS you typed is not the one this signer computed: not paired (check you are pairing with your own server)".into());
    }
    let done = Done {
        v: PAIR_V,
        kind: "k2-signer-done".into(),
        signer_fp: identity.fingerprint(),
        server_fp: o.server_fp.clone(),
        nonce: o.nonce.clone(),
        code_binding: o.code_binding.clone(),
        at: now,
    };
    let signed = sign(identity, DOMAIN_PAIR_DONE, &done)?;
    Ok((o.server_fp, o.server_spki, o.server_label, signed))
}

/// Server (reference / interface for the daemon route): answer a hello for
/// the code string the person typed. Returns the signed offer and the SAS
/// to show.
pub fn server_offer(server_key: &SigningKey, server_label: &str, code_string: &str, hello: &Hello, now: i64) -> Result<(Signed, String), String> {
    let es = EnrollString::parse(code_string).ok_or("not a signer pairing code (ABCDE-FGHJK.<16 hex>)")?;
    if hello.v != PAIR_V || hello.kind != "k2-signer-hello" {
        return Err("not a k2-signer hello".into());
    }
    let der = crypto::unb64(&hello.signer_spki)?;
    crypto::p256_point_from_spki(&der)?;
    let fp = crypto::fingerprint_of_spki_der(&der);
    if fp != hello.signer_fp || !es.matches_controller(&fp) {
        return Err("the signer's key is not the one the code names".into());
    }
    let nonce = crypto::b64(&crypto::random_bytes(16)?);
    let binding = code_binding(&es.code);
    let o = Offer {
        v: PAIR_V,
        kind: "k2-signer-offer".into(),
        server_fp: server_key.fingerprint(),
        server_spki: crypto::b64(server_key.spki_der()),
        server_label: server_label.to_string(),
        signer_fp: fp.clone(),
        nonce: nonce.clone(),
        code_binding: binding.clone(),
        at: now,
    };
    let signed = sign(server_key, DOMAIN_PAIR_OFFER, &o)?;
    Ok((signed, sas(&server_key.fingerprint(), &fp, &nonce, &binding)))
}

/// Server: check the signer's done against the offer it sent; returns the
/// signer's fp to record (with the hello's SPKI).
pub fn server_finish(offer: &Signed, hello: &Hello, done: &Signed) -> Result<String, String> {
    let o: Offer = offer.peek(DOMAIN_PAIR_OFFER)?;
    let der = crypto::unb64(&hello.signer_spki)?;
    let d: Done = done.open_node(&der, DOMAIN_PAIR_DONE)?;
    if d.v != PAIR_V
        || d.kind != "k2-signer-done"
        || d.signer_fp != o.signer_fp
        || d.server_fp != o.server_fp
        || d.nonce != o.nonce
        || d.code_binding != o.code_binding
    {
        return Err("the signer's answer is not for this offer".into());
    }
    Ok(d.signer_fp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_round_trip_and_refusals() {
        let signer = SigningKey::generate().unwrap();
        let server = SigningKey::generate().unwrap();
        let now = 1_800_000_000;
        let (code, hello, pending) = signer_begin(&signer, "z3mbpZ", now).unwrap();
        assert!(code.contains('.'));
        let (offer, server_sas) = server_offer(&server, "z3thon.k2.dev", &code, &hello, now + 5).unwrap();
        // A wrong SAS: not paired.
        assert!(signer_accept(&signer, &pending, &offer, "000000", now + 10).is_err() || server_sas == "000000");
        let (fp, spki, label, done) = signer_accept(&signer, &pending, &offer, &server_sas, now + 10).unwrap();
        assert_eq!(fp, server.fingerprint());
        assert_eq!(spki, crypto::b64(server.spki_der()));
        assert_eq!(label, "z3thon.k2.dev");
        assert_eq!(server_finish(&offer, &hello, &done).unwrap(), signer.fingerprint());
        // Expired code.
        assert!(signer_accept(&signer, &pending, &offer, &server_sas, now + CODE_TTL_SECS + 1).unwrap_err().contains("expired"));
        // A server that never got the code (another binding) is refused.
        let (code2, _, _) = signer_begin(&signer, "z3mbpZ", now).unwrap();
        let (offer2, sas2) = server_offer(&server, "evil", &code2, &hello, now).unwrap();
        assert!(signer_accept(&signer, &pending, &offer2, &sas2, now + 10).unwrap_err().contains("pairing code"));
        // A tampered offer is refused.
        let mut bad = offer.clone();
        bad.body = bad.body.replace("z3thon.k2.dev", "z3thon.k2.dew");
        assert!(signer_accept(&signer, &pending, &bad, &server_sas, now + 10).is_err());
        // The code names the signer: another signer's hello is refused by the server.
        let other = SigningKey::generate().unwrap();
        let (_, other_hello, _) = signer_begin(&other, "x", now).unwrap();
        assert!(server_offer(&server, "s", &code, &other_hello, now).is_err());
        // The SAS domain is the frozen one.
        assert_eq!(DOMAIN_SIGNER_SAS, "k2-signer-sas-v1");
    }
}
