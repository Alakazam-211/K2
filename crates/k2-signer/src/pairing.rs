//! Signer ↔ server pairing (PRD §4.2, vs-live V17, V29): the person at the
//! signing machine pairs it with their K2 server once; the signer pins the
//! server's tunnel-key fingerprint (the `controller_fp` every receipt
//! signs), and the server records the signer's public key.
//!
//! **The SAS is the check.** The person must read the 6 digits on a screen
//! the courier can't fake: the K2 server's own UI (its desktop app, or `k2`
//! in a terminal on the server), never a page or message the courier
//! relays. Everything else here is there so that the SAS can't be gamed:
//!
//! - **Commit, then reveal** (security review H2): neither end can pick its
//!   nonce after seeing the other's, so whoever carries the messages can't
//!   grind a nonce until the 6-digit SAS matches.
//! - **The code's MAC is defence in depth.** The one-time code keys an HMAC
//!   over the server's offer, and the hello and offer don't carry the code
//!   or its hash. But the person types the code on the server, so whoever
//!   sees that screen or the server route also sees it. The SAS, not the
//!   code, is what proves which server is on the other end.
//! - **Root's state only** (review M-a): the pairing state lives in root's
//!   private folder, and `pair-accept` checks the offer's signature, MAC and
//!   commitment again before it computes the SAS.
//!
//! 1. **`pair-begin`** (admin, at the signer): a one-time 10-character code
//!    and the signer's fingerprint prefix, `ABCDE-FGHJK.<16 hex>`, for the
//!    person to type at their server; and a [`Hello`] with the signer's key
//!    and a **commitment** to a fresh signer nonce (the nonce stays here).
//! 2. **`k2 ci signer add <code string>`** (owner/Admin at the server, with a
//!    confirm token, never an agent, V17): the server checks the hello's key
//!    against the fingerprint prefix, picks its own nonce, and answers an
//!    [`OfferMsg`]: the [`Offer`] signed with its tunnel key over
//!    `k2-signer-pair-offer-v1`, plus an HMAC of it under a key derived from
//!    the code (`k2-signer-pair-mac-v1`).
//! 3. **`pair-reveal`** (admin, at the signer): the signer checks the
//!    signature, the MAC (proof the server got the code from the person) and
//!    that the offer carries its own commitment; it keeps that one offer and
//!    reveals its nonce in a signed [`Reveal`] (once: a second offer is
//!    refused). The server checks the reveal against the commitment and
//!    shows the SAS of both fingerprints, both nonces and the offer.
//! 4. **`pair-accept <the 6 digits the server shows>`** (admin, at the
//!    signer): it pins the controller only when the person typed the SAS the
//!    signer computes itself, and answers a signed [`Done`], which the server
//!    checks to record the signer.
//!
//! The SAS scheme is `k2_node_proto::pairing::sas_digits` with the frozen
//! domain `k2-signer-sas-v1`; nothing in k2-node-proto changed. The signer
//! never talks to the network: the messages travel through the person's
//! `k2` CLI and the server's route.
//!
//! **Server side (builder O).** [`server_offer`], [`server_sas`] and
//! [`server_finish`] are the reference implementation and the interface for
//! the daemon route. The route must:
//! - take `k2 ci signer add` only from the owner or an Admin with a confirm
//!   token, never an agent (V17);
//! - show the SAS **only in the server's own UI** (the desktop app or a
//!   `k2` command run on the server), never return it to the caller that
//!   carries the hello/offer/reveal (a courier who learns the SAS could
//!   relay it);
//! - keep [`ServerPending`] server-side, and record the signer only after
//!   [`server_finish`] verifies.

use aws_lc_rs::hmac;
use k2_node_proto::ci::{Signed, DOMAIN_SIGNER_SAS};
use k2_node_proto::crypto::{self, signed_bytes, SigningKey};
use k2_node_proto::pairing::{normalize_code, sas_digits, sas_matches, EnrollString};
use serde::{Deserialize, Serialize};

/// The offer's signature domain (server tunnel key).
pub const DOMAIN_PAIR_OFFER: &str = "k2-signer-pair-offer-v1";
/// The reveal's signature domain (signer key).
pub const DOMAIN_PAIR_REVEAL: &str = "k2-signer-pair-reveal-v1";
/// The done message's signature domain (signer key).
pub const DOMAIN_PAIR_DONE: &str = "k2-signer-pair-done-v1";
/// The signer nonce's commitment.
pub const DOMAIN_PAIR_COMMIT: &str = "k2-signer-pair-commit-v1";
/// The code-derived MAC key over the offer.
pub const DOMAIN_PAIR_MAC: &str = "k2-signer-pair-mac-v1";
/// A pairing code lives this long.
pub const CODE_TTL_SECS: i64 = 600;
pub const PAIR_V: u32 = 2;
/// Longest label shown to a person.
pub const MAX_LABEL: usize = 64;

/// Signer → server: the signer's key and its nonce commitment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hello {
    pub v: u32,
    pub kind: String,
    pub signer_fp: String,
    /// base64 P-256 SPKI.
    pub signer_spki: String,
    pub signer_label: String,
    /// hex sha256 of `signed_bytes(k2-signer-pair-commit-v1, [signer_fp, signer_nonce])`.
    pub signer_commit: String,
}

/// Server → signer, signed with the server's tunnel key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Offer {
    pub v: u32,
    pub kind: String,
    pub server_fp: String,
    pub server_spki: String,
    pub server_label: String,
    pub signer_fp: String,
    /// The hello's commitment, echoed: the offer was made for it.
    pub signer_commit: String,
    /// base64, 16 bytes.
    pub server_nonce: String,
    pub at: i64,
}

/// The offer plus the code-keyed MAC over its signed body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OfferMsg {
    pub offer: Signed,
    /// base64 HMAC-SHA256(code key, offer.body).
    pub mac: String,
}

/// Signer → server (signed with the signer's key): the nonce, for this offer only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reveal {
    pub v: u32,
    pub kind: String,
    pub signer_fp: String,
    pub server_fp: String,
    pub offer_digest: String,
    pub signer_nonce: String,
}

/// Signer → server (signed with the signer's key): the person confirmed the SAS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Done {
    pub v: u32,
    pub kind: String,
    pub signer_fp: String,
    pub server_fp: String,
    pub offer_digest: String,
    pub at: i64,
}

/// What the signer keeps between the steps (a 0600 file of the signing user).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pending {
    /// hex of the code-derived MAC key (the code itself is not kept).
    pub mac_key: String,
    pub signer_nonce: String,
    pub created_at: i64,
    pub label: String,
    /// The one offer (with its MAC) the nonce was revealed for; checked
    /// again in full at `pair-accept` (M-a).
    pub offer: Option<OfferMsg>,
}

/// What the server keeps between its steps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerPending {
    pub hello: Hello,
    pub offer: Signed,
}

/// A label shown to a person: 1–64 printable ASCII characters (no control
/// characters, no escape sequences).
pub fn check_label(what: &str, s: &str) -> Result<(), String> {
    if s.is_empty() || s.len() > MAX_LABEL || !s.chars().all(|c| c.is_ascii_graphic() || c == ' ') {
        return Err(format!("{what}: 1-{MAX_LABEL} printable ASCII characters"));
    }
    Ok(())
}

fn mac_key_of(code: &str) -> Result<[u8; 32], String> {
    let n = normalize_code(code).ok_or("not a pairing code")?;
    Ok(crypto::sha256(&signed_bytes(DOMAIN_PAIR_MAC, &[n.as_bytes()])))
}

fn mac(key: &[u8], body: &str) -> String {
    crypto::b64(hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, key), body.as_bytes()).as_ref())
}

fn mac_ok(key: &[u8], body: &str, tag_b64: &str) -> bool {
    let Ok(tag) = crypto::unb64(tag_b64) else { return false };
    hmac::verify(&hmac::Key::new(hmac::HMAC_SHA256, key), body.as_bytes(), &tag).is_ok()
}

/// The signer nonce's commitment.
pub fn commit(signer_fp: &str, signer_nonce_b64: &str) -> String {
    crypto::sha256_hex(&signed_bytes(DOMAIN_PAIR_COMMIT, &[signer_fp.as_bytes(), signer_nonce_b64.as_bytes()]))
}

/// The SAS both ends show (roles fixed: server first).
pub fn sas(server_fp: &str, signer_fp: &str, server_nonce: &str, signer_nonce: &str, offer_digest: &str) -> String {
    sas_digits(
        DOMAIN_SIGNER_SAS,
        &[server_fp.as_bytes(), signer_fp.as_bytes(), server_nonce.as_bytes(), signer_nonce.as_bytes(), offer_digest.as_bytes()],
    )
}

fn nonce16() -> Result<String, String> {
    Ok(crypto::b64(&crypto::random_bytes(16)?))
}

/// Signer: start a pairing. Returns the code string for the person, the
/// hello for the server, and what to keep.
pub fn signer_begin(identity: &SigningKey, label: &str, now: i64) -> Result<(String, Hello, Pending), String> {
    check_label("the signer's label", label)?;
    let code = k2_node_proto::pairing::new_code()?;
    let es = EnrollString::new(&code, &identity.fingerprint()).ok_or("could not make the pairing code")?;
    let signer_nonce = nonce16()?;
    let hello = Hello {
        v: PAIR_V,
        kind: "k2-signer-hello".into(),
        signer_fp: identity.fingerprint(),
        signer_spki: crypto::b64(identity.spki_der()),
        signer_label: label.to_string(),
        signer_commit: commit(&identity.fingerprint(), &signer_nonce),
    };
    let pending =
        Pending { mac_key: crypto::hex(&mac_key_of(&es.code)?), signer_nonce, created_at: now, label: label.to_string(), offer: None };
    Ok((es.render(), hello, pending))
}

/// Every check of an offer against this pairing: the code's MAC, the
/// server's signature, labels, this signer, this hello's commitment.
fn verify_offer(identity: &SigningKey, pending: &Pending, msg: &OfferMsg) -> Result<Offer, String> {
    let key = (0..pending.mac_key.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(pending.mac_key.get(i..i + 2).unwrap_or("zz"), 16))
        .collect::<Result<Vec<u8>, _>>()
        .map_err(|_| "the pending pairing is damaged".to_string())?;
    if key.len() != 32 || !mac_ok(&key, &msg.offer.body, &msg.mac) {
        return Err("the offer was not made with this pairing code (a server the person did not give the code to)".into());
    }
    let peek: Offer = msg.offer.peek(DOMAIN_PAIR_OFFER)?;
    let der = crypto::unb64(&peek.server_spki)?;
    crypto::p256_point_from_spki(&der)?;
    let o: Offer = msg.offer.open_node(&der, DOMAIN_PAIR_OFFER)?;
    if o.v != PAIR_V || o.kind != "k2-signer-offer" {
        return Err("not a k2-signer pairing offer".into());
    }
    if crypto::fingerprint_of_spki_der(&der) != o.server_fp || !k2_node_proto::ci::is_fp(&o.server_fp) {
        return Err("the offer's server key is not its fingerprint".into());
    }
    check_label("the server's label", &o.server_label)?;
    if o.signer_fp != identity.fingerprint() {
        return Err("the offer is for another signer".into());
    }
    if o.signer_commit != commit(&identity.fingerprint(), &pending.signer_nonce) {
        return Err("the offer is not for this pairing's hello".into());
    }
    if crypto::unb64(&o.server_nonce)?.len() != 16 {
        return Err("the offer's nonce is not 16 bytes".into());
    }
    Ok(o)
}

/// Signer: check the server's offer; returns the offer's body and the signed
/// [`Reveal`] for the server. The caller stores `msg` in `Pending.offer`:
/// the nonce is revealed for that one offer only.
pub fn signer_reveal(identity: &SigningKey, pending: &Pending, msg: &OfferMsg, now: i64) -> Result<(Offer, Signed), String> {
    if now > pending.created_at + CODE_TTL_SECS {
        return Err("the pairing code expired: start again (pair-begin)".into());
    }
    if let Some(o) = &pending.offer {
        if o != msg {
            return Err("this pairing's nonce was already revealed to another offer: start again (pair-begin)".into());
        }
    }
    let o = verify_offer(identity, pending, msg)?;
    let reveal = Reveal {
        v: PAIR_V,
        kind: "k2-signer-reveal".into(),
        signer_fp: identity.fingerprint(),
        server_fp: o.server_fp.clone(),
        offer_digest: msg.offer.digest(),
        signer_nonce: pending.signer_nonce.clone(),
    };
    let signed = Signed::sign_node(identity, DOMAIN_PAIR_REVEAL, &reveal)?;
    Ok((o, signed))
}

/// Signer: the SAS the person typed (read off the server's own screen)
/// against the one the signer computes for the revealed offer, which is
/// checked again in full first (M-a). Returns the server's fp, SPKI, label
/// and the signed [`Done`].
pub fn signer_accept(identity: &SigningKey, pending: &Pending, typed_sas: &str, now: i64) -> Result<(String, String, String, Signed), String> {
    if now > pending.created_at + CODE_TTL_SECS {
        return Err("the pairing code expired: start again (pair-begin)".into());
    }
    let Some(msg) = &pending.offer else {
        return Err("no offer revealed yet (pair-reveal first)".into());
    };
    let o = verify_offer(identity, pending, msg)?;
    let want = sas(&o.server_fp, &o.signer_fp, &o.server_nonce, &pending.signer_nonce, &msg.offer.digest());
    if !sas_matches(&want, typed_sas) {
        return Err("the SAS you typed is not the one this signer computed: not paired, and this pairing is void (start again with pair-begin; check you are pairing with your own server)".into());
    }
    let done =
        Done { v: PAIR_V, kind: "k2-signer-done".into(), signer_fp: identity.fingerprint(), server_fp: o.server_fp.clone(), offer_digest: msg.offer.digest(), at: now };
    let signed = Signed::sign_node(identity, DOMAIN_PAIR_DONE, &done)?;
    Ok((o.server_fp, o.server_spki, o.server_label, signed))
}

/// Server (reference / interface for the daemon route): answer a hello for
/// the code string the person typed. No SAS yet: it needs the reveal.
pub fn server_offer(server_key: &SigningKey, server_label: &str, code_string: &str, hello: &Hello, now: i64) -> Result<(OfferMsg, ServerPending), String> {
    check_label("the server's label", server_label)?;
    let es = EnrollString::parse(code_string).ok_or("not a signer pairing code (ABCDE-FGHJK.<16 hex>)")?;
    if hello.v != PAIR_V || hello.kind != "k2-signer-hello" || !k2_node_proto::ci::is_fp(&hello.signer_commit) {
        return Err("not a k2-signer hello".into());
    }
    check_label("the signer's label", &hello.signer_label)?;
    let der = crypto::unb64(&hello.signer_spki)?;
    crypto::p256_point_from_spki(&der)?;
    let fp = crypto::fingerprint_of_spki_der(&der);
    if fp != hello.signer_fp || !es.matches_controller(&fp) {
        return Err("the signer's key is not the one the code names".into());
    }
    let o = Offer {
        v: PAIR_V,
        kind: "k2-signer-offer".into(),
        server_fp: server_key.fingerprint(),
        server_spki: crypto::b64(server_key.spki_der()),
        server_label: server_label.to_string(),
        signer_fp: fp,
        signer_commit: hello.signer_commit.clone(),
        server_nonce: nonce16()?,
        at: now,
    };
    let signed = Signed::sign_node(server_key, DOMAIN_PAIR_OFFER, &o)?;
    let tag = mac(&mac_key_of(&es.code)?, &signed.body);
    Ok((OfferMsg { offer: signed.clone(), mac: tag }, ServerPending { hello: hello.clone(), offer: signed }))
}

fn open_signer_msg<T: Serialize + serde::de::DeserializeOwned>(p: &ServerPending, s: &Signed, domain: &str) -> Result<T, String> {
    let der = crypto::unb64(&p.hello.signer_spki)?;
    s.open_node(&der, domain)
}

/// Server: check the signer's reveal against its commitment; returns the SAS
/// to show the person.
pub fn server_sas(p: &ServerPending, reveal: &Signed) -> Result<String, String> {
    let r: Reveal = open_signer_msg(p, reveal, DOMAIN_PAIR_REVEAL)?;
    let o: Offer = p.offer.peek(DOMAIN_PAIR_OFFER)?;
    if r.v != PAIR_V || r.kind != "k2-signer-reveal" || r.signer_fp != o.signer_fp || r.server_fp != o.server_fp || r.offer_digest != p.offer.digest() {
        return Err("the reveal is not for this offer".into());
    }
    if commit(&r.signer_fp, &r.signer_nonce) != p.hello.signer_commit {
        return Err("the revealed nonce is not the one the hello committed to".into());
    }
    Ok(sas(&o.server_fp, &o.signer_fp, &o.server_nonce, &r.signer_nonce, &p.offer.digest()))
}

/// Server: check the signer's done; returns the signer's fp to record (with
/// the hello's SPKI).
pub fn server_finish(p: &ServerPending, done: &Signed) -> Result<String, String> {
    let d: Done = open_signer_msg(p, done, DOMAIN_PAIR_DONE)?;
    let o: Offer = p.offer.peek(DOMAIN_PAIR_OFFER)?;
    if d.v != PAIR_V || d.kind != "k2-signer-done" || d.signer_fp != o.signer_fp || d.server_fp != o.server_fp || d.offer_digest != p.offer.digest() {
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
        let (code, hello, mut pending) = signer_begin(&signer, "z3mbpZ", now).unwrap();
        assert!(code.contains('.'));
        // The hello and the offer carry neither the code nor its hash.
        let (msg, sp) = server_offer(&server, "client.example", &code, &hello, now + 5).unwrap();
        let wire = serde_json::to_string(&(&hello, &msg)).unwrap();
        let normalized = normalize_code(code.split('.').next().unwrap()).unwrap();
        assert!(!wire.contains(&normalized) && !wire.contains(&k2_node_proto::pairing::code_binding(&normalized)));
        // No SAS before the reveal; accept refuses.
        assert!(signer_accept(&signer, &pending, "123456", now + 9).unwrap_err().contains("pair-reveal first"));
        let (_, reveal) = signer_reveal(&signer, &pending, &msg, now + 10).unwrap();
        pending.offer = Some(msg.clone());
        let shown = server_sas(&sp, &reveal).unwrap();
        let wrong = if shown == "000000" { "111111" } else { "000000" };
        assert!(signer_accept(&signer, &pending, wrong, now + 12).is_err());
        let (fp, spki, label, done) = signer_accept(&signer, &pending, &shown, now + 12).unwrap();
        assert_eq!((fp, spki, label), (server.fingerprint(), crypto::b64(server.spki_der()), "client.example".to_string()));
        assert_eq!(server_finish(&sp, &done).unwrap(), signer.fingerprint());
        assert!(signer_accept(&signer, &pending, &shown, now + CODE_TTL_SECS + 1).unwrap_err().contains("expired"));
        // A tampered offer fails its MAC; a server without the code can't make one.
        let mut bad = msg.clone();
        bad.offer.body = bad.offer.body.replace("client.example", "client.exampla");
        assert!(signer_reveal(&signer, &PendingNoOffer(&pending).get(), &bad, now + 10).unwrap_err().contains("pairing code"));
        let (other_code, _, _) = signer_begin(&signer, "x", now).unwrap();
        let (no_code, _) = server_offer(&server, "evil", &other_code, &hello, now).unwrap();
        assert!(signer_reveal(&signer, &PendingNoOffer(&pending).get(), &no_code, now + 10).unwrap_err().contains("pairing code"));
        // A label with control characters is refused before anyone sees it.
        assert!(server_offer(&server, "evil\x1b[2Jlabel", &code, &hello, now).is_err());
        // The code names the signer: another signer's hello is refused.
        let other = SigningKey::generate().unwrap();
        let (_, other_hello, _) = signer_begin(&other, "x", now).unwrap();
        assert!(server_offer(&server, "s", &code, &other_hello, now).is_err());
        // A reveal for another nonce than the commitment is refused by the server.
        let forged = Reveal {
            v: PAIR_V,
            kind: "k2-signer-reveal".into(),
            signer_fp: signer.fingerprint(),
            server_fp: server.fingerprint(),
            offer_digest: msg.offer.digest(),
            signer_nonce: crypto::b64(&[9u8; 16]),
        };
        let forged = Signed::sign_node(&signer, DOMAIN_PAIR_REVEAL, &forged).unwrap();
        assert!(server_sas(&sp, &forged).unwrap_err().contains("committed"));
    }

    /// H2 / L-b: the nonce is revealed for one offer only. With
    /// `pending.offer` = A, an offer B (here: the same server's key, its
    /// nonce ground after seeing the reveal) is refused, every time.
    #[test]
    fn a_ground_nonce_no_longer_passes() {
        let signer = SigningKey::generate().unwrap();
        let attacker = SigningKey::generate().unwrap();
        let now = 1_800_000_000;
        let (code, hello, mut pending) = signer_begin(&signer, "z3mbpZ", now).unwrap();
        let (a, _) = server_offer(&attacker, "client.example", &code, &hello, now).unwrap();
        let (_, reveal) = signer_reveal(&signer, &pending, &a, now + 1).unwrap();
        pending.offer = Some(a.clone());
        let revealed: Reveal = reveal.peek(DOMAIN_PAIR_REVEAL).unwrap();
        assert_eq!(revealed.signer_nonce, pending.signer_nonce);
        // Now the courier knows the signer's nonce: it grinds a second offer B
        // (another server nonce, the code's MAC redone, as if it knew the code).
        let mut o: Offer = a.offer.peek(DOMAIN_PAIR_OFFER).unwrap();
        o.server_nonce = crypto::b64(&[7u8; 16]);
        let b_offer = Signed::sign_node(&attacker, DOMAIN_PAIR_OFFER, &o).unwrap();
        let b = OfferMsg { mac: mac(&mac_key_of(code.split('.').next().unwrap()).unwrap(), &b_offer.body), offer: b_offer };
        assert!(signer_reveal(&signer, &pending, &b, now + 2).unwrap_err().contains("already revealed to another offer"));
        // And pair-accept re-checks what pending.offer holds (M-a): a B swapped
        // in behind the signer's back fails its commitment or MAC check …
        let mut swapped = pending.clone();
        let other = SigningKey::generate().unwrap();
        let (_, other_hello, _) = signer_begin(&other, "x", now).unwrap();
        let (c, _) = server_offer(&attacker, "client.example", &code, &Hello { signer_fp: signer.fingerprint(), signer_spki: crypto::b64(signer.spki_der()), ..other_hello }, now).unwrap();
        swapped.offer = Some(c);
        assert!(signer_accept(&signer, &swapped, "000000", now + 3).unwrap_err().contains("not for this pairing's hello"));
        // … and a damaged MAC on the revealed offer too.
        let mut bad_mac = pending.clone();
        bad_mac.offer = Some(OfferMsg { mac: crypto::b64(&[0u8; 32]), ..a.clone() });
        assert!(signer_accept(&signer, &bad_mac, "000000", now + 3).unwrap_err().contains("not made with this pairing code"));
    }

    struct PendingNoOffer<'a>(&'a Pending);
    impl PendingNoOffer<'_> {
        fn get(&self) -> Pending {
            Pending { offer: None, ..self.0.clone() }
        }
    }
}
