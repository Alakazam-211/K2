//! Pairing v2 (0.45.2): the commit-then-reveal code of [`super::sas`] over
//! four owner-run steps. Each step's output is the next step's input, so a
//! carrier (the desktop app holding both servers' owner logins, or two
//! owners copying text with `k2 fed pair-*`) moves them between servers:
//!
//! | step | runs on | in | out |
//! |---|---|---|---|
//! | [`init`] | initiator A | – | [`Offer`]: A's key + address + commitment C |
//! | [`respond`] | responder B | Offer | [`Answer`]: B's key + address + Nb |
//! | [`reveal`] | A | Answer | [`Reveal`] (Na) + A's code |
//! | [`verify`] | B | Reveal | B's code (aborts when Na does not open C) |
//!
//! Then each owner confirms the code on their own server ([`confirm`]). The
//! respond/reveal steps pin the other server as `Pending` (pairing version
//! 2); a peer already pinned keeps its row (address, trust, capabilities):
//! re-pairing only upgrades `pairing_version` on a matching confirm.
//!
//! Sessions, nonces and expected codes are in memory, single-use and expire
//! after [`super::sas::NONCE_TTL_SECS`]. A daemon restart mid-pairing means
//! pairing again. A confirm attempt consumes the expected code: one guess.
//!
//! v1 compatibility: peers pinned before 0.45.2 have `pairing_version` 1 and
//! keep working; [`confirm`] keeps the v1 code check only for an already
//! Trusted v1 peer. No NEW pairing completes through v1.

use std::collections::HashMap;
use std::sync::Mutex;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use super::peers::{fingerprint_of_spki_pem, FederationPeer, PeerStore, PeerTrust};
use super::sas::{self, Nonce};

/// The pairing version a v2 confirm records on the peer row.
pub const PAIRING_VERSION: u32 = 2;

/// Step 1 output (A → B).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offer {
    pub v: u32,
    pub session: String,
    pub fingerprint: String,
    pub public_key_pem: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub subdomain: String,
    #[serde(default, alias = "baseUrl")]
    pub base_url: String,
    /// Hex `SHA-256("k2-sas-commit-v2" || fp_a || Na)`.
    pub commitment: String,
}

/// Step 2 output (B → A).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    pub v: u32,
    pub session: String,
    pub fingerprint: String,
    pub public_key_pem: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub subdomain: String,
    #[serde(default, alias = "baseUrl")]
    pub base_url: String,
    /// Hex Nb.
    pub nonce: String,
}

/// Step 3 output (A → B).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reveal {
    pub v: u32,
    pub session: String,
    pub fingerprint: String,
    /// Hex Na.
    pub nonce: String,
}

/// Why a step failed. Every failure changes no trust state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairV2Error {
    BadInput(String),
    UnsupportedVersion(u32),
    SelfPair,
    Blocked,
    UnknownSession,
    Expired,
    CommitmentMismatch,
}

impl PairV2Error {
    pub fn status(&self) -> &'static str {
        match self {
            Self::BadInput(_) | Self::UnsupportedVersion(_) | Self::SelfPair => "400 Bad Request",
            Self::Blocked | Self::CommitmentMismatch => "403 Forbidden",
            Self::UnknownSession | Self::Expired => "409 Conflict",
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::BadInput(_) => "bad_input",
            Self::UnsupportedVersion(_) => "unsupported_pairing_version",
            Self::SelfPair => "self_pair",
            Self::Blocked => "blocked",
            Self::UnknownSession => "unknown_pairing_session",
            Self::Expired => "pairing_expired",
            Self::CommitmentMismatch => "commitment_mismatch",
        }
    }
}

impl std::fmt::Display for PairV2Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadInput(e) => write!(f, "{e}"),
            Self::UnsupportedVersion(v) => write!(f, "pairing version {v} is not supported (this server pairs with v2)"),
            Self::SelfPair => write!(f, "refusing to pair with this server itself"),
            Self::Blocked => write!(f, "that peer is Blocked; unblock it before pairing"),
            Self::UnknownSession => write!(
                f,
                "no pairing in progress for that step (it was already used, or the server restarted); start again"
            ),
            Self::Expired => write!(f, "the pairing step expired (10 minutes); start again"),
            Self::CommitmentMismatch => write!(
                f,
                "the other server's revealed value does not match what it committed to; pairing aborted \
                 (someone may be between the two servers)"
            ),
        }
    }
}

impl std::error::Error for PairV2Error {}

struct InitiatorSession {
    local_fp: String,
    na: Nonce,
    created: DateTime<Utc>,
}

struct ResponderSession {
    session: String,
    c: [u8; 32],
    nb: Nonce,
    created: DateTime<Utc>,
}

struct PendingConfirm {
    sas: String,
    created: DateTime<Utc>,
}

static INITIATORS: Mutex<Option<HashMap<String, InitiatorSession>>> = Mutex::new(None);
/// Keyed by (local fp, initiator fp).
static RESPONDERS: Mutex<Option<HashMap<(String, String), ResponderSession>>> = Mutex::new(None);
/// Keyed by (local fp, peer fp).
static CONFIRMS: Mutex<Option<HashMap<(String, String), PendingConfirm>>> = Mutex::new(None);

fn expired(created: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    now - created >= Duration::seconds(sas::NONCE_TTL_SECS) || created > now + Duration::seconds(60)
}

fn with_map<K, V, R>(m: &Mutex<Option<HashMap<K, V>>>, f: impl FnOnce(&mut HashMap<K, V>) -> R) -> R {
    let mut g = m.lock().unwrap_or_else(|p| p.into_inner());
    f(g.get_or_insert_with(HashMap::new))
}

fn new_session_id() -> Result<String, PairV2Error> {
    let n = sas::new_nonce().map_err(PairV2Error::BadInput)?;
    Ok(sas::to_hex(&n[..16]))
}

fn check_v(v: u32) -> Result<(), PairV2Error> {
    if v == PAIRING_VERSION {
        Ok(())
    } else {
        Err(PairV2Error::UnsupportedVersion(v))
    }
}

/// The fingerprint of `pem`, which must equal `claimed`.
fn fp_of(pem: &str, claimed: &str) -> Result<String, PairV2Error> {
    let fp = fingerprint_of_spki_pem(pem).map_err(PairV2Error::BadInput)?;
    if fp != claimed.trim() {
        return Err(PairV2Error::BadInput(
            "the presented key does not match the presented fingerprint".to_string(),
        ));
    }
    Ok(fp)
}

/// Pin `pem` as a Pending v2 peer when it is not pinned yet. An existing
/// row (any version) is kept as is: re-pairing keeps address and
/// permissions. A Blocked row refuses.
fn pin_if_new(
    store: &mut PeerStore,
    label: &str,
    subdomain: &str,
    base_url: &str,
    pem: &str,
) -> Result<String, PairV2Error> {
    let fp = fingerprint_of_spki_pem(pem).map_err(PairV2Error::BadInput)?;
    match store.get(&fp) {
        Some(p) if p.trust == PeerTrust::Blocked => Err(PairV2Error::Blocked),
        Some(_) => Ok(fp),
        None => {
            let (sub, url) = super::pairing::normalize_pair_routing(subdomain, base_url)
                .map_err(PairV2Error::BadInput)?;
            let peer = FederationPeer::pin(label, sub, pem).map_err(PairV2Error::BadInput)?.with_base_url(url);
            store.upsert(peer);
            Ok(fp)
        }
    }
}

/// Step 1 (initiator). `local_pem` is this server's SPKI PEM; label and
/// address are what the responder should pin.
pub fn init(
    local_pem: &str,
    label: &str,
    subdomain: &str,
    base_url: &str,
    now: DateTime<Utc>,
) -> Result<Offer, PairV2Error> {
    let local_fp = fingerprint_of_spki_pem(local_pem).map_err(PairV2Error::BadInput)?;
    // Same routing rules the responder applies: fail here, not over there.
    super::pairing::normalize_pair_routing(subdomain, base_url).map_err(PairV2Error::BadInput)?;
    let na = sas::new_nonce().map_err(PairV2Error::BadInput)?;
    let session = new_session_id()?;
    let c = sas::commitment(&local_fp, &na);
    with_map(&INITIATORS, |m| {
        m.retain(|_, s| !expired(s.created, now));
        m.insert(session.clone(), InitiatorSession { local_fp: local_fp.clone(), na, created: now });
    });
    Ok(Offer {
        v: PAIRING_VERSION,
        session,
        fingerprint: local_fp,
        public_key_pem: local_pem.to_string(),
        label: label.to_string(),
        subdomain: subdomain.trim().to_string(),
        base_url: base_url.trim().to_string(),
        commitment: sas::to_hex(&c),
    })
}

/// Step 2 (responder): pin the initiator (Pending v2 unless already pinned),
/// remember its commitment, and answer with a fresh Nb.
#[allow(clippy::too_many_arguments)]
pub fn respond(
    store: &mut PeerStore,
    offer: &Offer,
    local_pem: &str,
    label: &str,
    subdomain: &str,
    base_url: &str,
    now: DateTime<Utc>,
) -> Result<Answer, PairV2Error> {
    check_v(offer.v)?;
    let local_fp = fingerprint_of_spki_pem(local_pem).map_err(PairV2Error::BadInput)?;
    let fp_a = fp_of(&offer.public_key_pem, &offer.fingerprint)?;
    if fp_a == local_fp {
        return Err(PairV2Error::SelfPair);
    }
    let c = sas::from_hex32(&offer.commitment).map_err(|e| PairV2Error::BadInput(format!("commitment: {e}")))?;
    if offer.session.trim().is_empty() {
        return Err(PairV2Error::BadInput("missing pairing session".to_string()));
    }
    pin_if_new(store, &offer.label, &offer.subdomain, &offer.base_url, &offer.public_key_pem)?;
    let nb = sas::new_nonce().map_err(PairV2Error::BadInput)?;
    with_map(&RESPONDERS, |m| {
        m.retain(|_, s| !expired(s.created, now));
        // A newer attempt from the same initiator replaces an older one.
        m.insert(
            (local_fp.clone(), fp_a.clone()),
            ResponderSession { session: offer.session.clone(), c, nb, created: now },
        );
    });
    Ok(Answer {
        v: PAIRING_VERSION,
        session: offer.session.clone(),
        fingerprint: local_fp,
        public_key_pem: local_pem.to_string(),
        label: label.to_string(),
        subdomain: subdomain.trim().to_string(),
        base_url: base_url.trim().to_string(),
        nonce: sas::to_hex(&nb),
    })
}

/// What [`reveal`] returns to the initiator's owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revealed {
    pub reveal: Reveal,
    pub sas: String,
    pub peer_fingerprint: String,
}

/// Step 3 (initiator): consume the session, pin the responder (Pending v2
/// unless already pinned; `peer_subdomain` / `peer_base_url` override the
/// address it offered), compute the code and reveal Na.
pub fn reveal(
    store: &mut PeerStore,
    answer: &Answer,
    peer_subdomain: Option<&str>,
    peer_base_url: Option<&str>,
    now: DateTime<Utc>,
) -> Result<Revealed, PairV2Error> {
    check_v(answer.v)?;
    let sess = with_map(&INITIATORS, |m| m.remove(answer.session.trim())).ok_or(PairV2Error::UnknownSession)?;
    if expired(sess.created, now) {
        return Err(PairV2Error::Expired);
    }
    let fp_b = fp_of(&answer.public_key_pem, &answer.fingerprint)?;
    if fp_b == sess.local_fp {
        return Err(PairV2Error::SelfPair);
    }
    let nb = sas::from_hex32(&answer.nonce).map_err(|e| PairV2Error::BadInput(format!("nonce: {e}")))?;
    let sub = peer_subdomain.filter(|s| !s.trim().is_empty()).unwrap_or(&answer.subdomain);
    let url = peer_base_url.filter(|s| !s.trim().is_empty()).unwrap_or(&answer.base_url);
    pin_if_new(store, &answer.label, sub, url, &answer.public_key_pem)?;
    let code = sas::sas_v2(&sess.local_fp, &fp_b, &sess.na, &nb);
    with_map(&CONFIRMS, |m| {
        m.retain(|_, p| !expired(p.created, now));
        m.insert((sess.local_fp.clone(), fp_b.clone()), PendingConfirm { sas: code.clone(), created: now });
    });
    Ok(Revealed {
        reveal: Reveal {
            v: PAIRING_VERSION,
            session: answer.session.clone(),
            fingerprint: sess.local_fp.clone(),
            nonce: sas::to_hex(&sess.na),
        },
        sas: code,
        peer_fingerprint: fp_b,
    })
}

/// Step 4 (responder): consume the session, check Na against the
/// commitment (abort on a mismatch, never fall back), compute the code.
/// Returns `(code, initiator fingerprint)`.
pub fn verify(local_fp: &str, rv: &Reveal, now: DateTime<Utc>) -> Result<(String, String), PairV2Error> {
    check_v(rv.v)?;
    let fp_a = rv.fingerprint.trim().to_string();
    let sess = with_map(&RESPONDERS, |m| m.remove(&(local_fp.to_string(), fp_a.clone())))
        .ok_or(PairV2Error::UnknownSession)?;
    if sess.session != rv.session.trim() {
        return Err(PairV2Error::UnknownSession);
    }
    if expired(sess.created, now) {
        return Err(PairV2Error::Expired);
    }
    let na = sas::from_hex32(&rv.nonce).map_err(|e| PairV2Error::BadInput(format!("nonce: {e}")))?;
    if !sas::verify_commitment(&sess.c, &fp_a, &na) {
        return Err(PairV2Error::CommitmentMismatch);
    }
    let code = sas::sas_v2(&fp_a, local_fp, &na, &sess.nb);
    with_map(&CONFIRMS, |m| {
        m.retain(|_, p| !expired(p.created, now));
        m.insert((local_fp.to_string(), fp_a.clone()), PendingConfirm { sas: code.clone(), created: now });
    });
    Ok((code, fp_a))
}

/// How [`confirm`] completed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmKind {
    /// A v2 code matched: the peer is Trusted with `pairing_version` 2.
    V2,
    /// No v2 pairing in progress; an already Trusted v1 peer matched its v1
    /// code (old behaviour, nothing new is trusted).
    LegacyTrusted,
}

/// The owner's confirm on this server. See the module doc.
pub fn confirm(
    store: &mut PeerStore,
    local_fp: &str,
    peer_fp: &str,
    code: &str,
    capabilities: &[String],
    now: DateTime<Utc>,
) -> Result<ConfirmKind, String> {
    let pending = with_map(&CONFIRMS, |m| m.remove(&(local_fp.to_string(), peer_fp.to_string())));
    let peer = store
        .get(peer_fp)
        .cloned()
        .ok_or_else(|| format!("no peer pinned with fingerprint {peer_fp}"))?;
    if peer.trust == PeerTrust::Blocked {
        return Err("peer is Blocked — unblock it explicitly before pairing".to_string());
    }
    let Some(pending) = pending else {
        if peer.trust == PeerTrust::Trusted && peer.pairing_version < PAIRING_VERSION {
            super::pairing::apply_pair_confirm(store, peer_fp, code, local_fp, capabilities)?;
            return Ok(ConfirmKind::LegacyTrusted);
        }
        return Err(
            "no pairing code is waiting for this peer: start a new pairing (pairing v2; the code \
             expires after 10 minutes and is single-use)"
                .to_string(),
        );
    };
    if expired(pending.created, now) {
        return Err("the pairing code expired (10 minutes); pair again".to_string());
    }
    if !sas::sas_eq(code, &pending.sas) {
        return Err("pairing code mismatch — refusing to confirm; pair again".to_string());
    }
    let row = store.get_mut(peer_fp).ok_or_else(|| "peer vanished during confirm".to_string())?;
    row.pairing_version = PAIRING_VERSION;
    if row.trust == PeerTrust::Pending {
        row.trust = PeerTrust::Trusted;
        let caps: Vec<String> = if capabilities.is_empty() {
            super::pairing::DEFAULT_CONFIRM_CAPS.iter().map(|s| s.to_string()).collect()
        } else {
            capabilities.to_vec()
        };
        row.capabilities.extend(caps);
    }
    // A re-pair of a Trusted peer keeps its capabilities and address.
    Ok(ConfirmKind::V2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::KeyPair;

    fn key() -> KeyPair {
        KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("gen")
    }

    struct Box_ {
        pem: String,
        fp: String,
        store: PeerStore,
    }

    impl Box_ {
        fn new() -> Self {
            let pem = key().public_key_pem();
            let fp = fingerprint_of_spki_pem(&pem).expect("fp");
            Self { pem, fp, store: PeerStore::default() }
        }
    }

    /// The whole exchange; returns (A's code, B's code).
    fn run(a: &mut Box_, b: &mut Box_, now: DateTime<Utc>) -> (String, String) {
        let offer = init(&a.pem.clone(), "a", "alpha", "", now).expect("init");
        let answer = respond(&mut b.store, &offer, &b.pem.clone(), "b", "bravo", "", now).expect("respond");
        let r = reveal(&mut a.store, &answer, None, None, now).expect("reveal");
        let (code_b, fp_a) = verify(&b.fp.clone(), &r.reveal, now).expect("verify");
        assert_eq!(fp_a, a.fp.clone());
        assert_eq!(r.peer_fingerprint, b.fp.clone());
        (r.sas, code_b)
    }

    #[test]
    fn both_sides_compute_the_same_code_and_confirm_to_trusted_v2() {
        let (mut a, mut b) = (Box_::new(), Box_::new());
        let now = Utc::now();
        let (ca, cb) = run(&mut a, &mut b, now);
        assert_eq!(ca, cb);
        assert_eq!(a.store.get(&b.fp.clone()).expect("B at A").trust, PeerTrust::Pending);
        assert_eq!(b.store.get(&a.fp.clone()).expect("A at B").trust, PeerTrust::Pending);
        assert_eq!(b.store.get(&a.fp.clone()).expect("A at B").subdomain, "alpha");
        assert_eq!(confirm(&mut a.store, &a.fp.clone(), &b.fp.clone(), &ca, &[], now), Ok(ConfirmKind::V2));
        assert_eq!(confirm(&mut b.store, &b.fp.clone(), &a.fp.clone(), &cb, &[], now), Ok(ConfirmKind::V2));
        for (s, fp) in [(&a.store, b.fp.clone()), (&b.store, a.fp.clone())] {
            let p = s.get(&fp).expect("peer");
            assert_eq!(p.trust, PeerTrust::Trusted);
            assert_eq!(p.pairing_version, PAIRING_VERSION);
            assert!(p.capabilities.contains("inbound"));
        }
    }

    #[test]
    fn a_wrong_code_is_rejected_once_and_the_code_is_consumed() {
        let (mut a, mut b) = (Box_::new(), Box_::new());
        let now = Utc::now();
        let (ca, _) = run(&mut a, &mut b, now);
        let wrong = if ca == "000000" { "000001" } else { "000000" };
        let err = confirm(&mut a.store, &a.fp.clone(), &b.fp.clone(), wrong, &[], now).expect_err("wrong");
        assert!(err.contains("mismatch"), "{err}");
        // The right code no longer works: one guess per attempt.
        let err = confirm(&mut a.store, &a.fp.clone(), &b.fp.clone(), &ca, &[], now).expect_err("consumed");
        assert!(err.contains("start a new pairing"), "{err}");
        assert_eq!(a.store.get(&b.fp.clone()).expect("B").trust, PeerTrust::Pending);
    }

    #[test]
    fn a_reveal_that_does_not_open_the_commitment_aborts() {
        let (mut a, mut b) = (Box_::new(), Box_::new());
        let now = Utc::now();
        let offer = init(&a.pem.clone(), "a", "alpha", "", now).expect("init");
        let answer = respond(&mut b.store, &offer, &b.pem.clone(), "b", "bravo", "", now).expect("respond");
        let mut r = reveal(&mut a.store, &answer, None, None, now).expect("reveal").reveal;
        r.nonce = sas::to_hex(&[0x42; 32]);
        assert_eq!(verify(&b.fp.clone(), &r, now), Err(PairV2Error::CommitmentMismatch));
        // Aborted for good: the session is gone, no retry with another value.
        assert_eq!(verify(&b.fp.clone(), &r, now), Err(PairV2Error::UnknownSession));
        let err = confirm(&mut b.store, &b.fp.clone(), &a.fp.clone(), "123456", &[], now).expect_err("no code");
        assert!(err.contains("start a new pairing"), "{err}");
    }

    #[test]
    fn a_middle_party_that_swaps_its_own_key_gets_a_different_code() {
        // M sits between A and B: it receives A's offer and, before seeing
        // Na, must answer A with its own key and nonce. Its code at A then
        // differs from B's code except by a 1-in-10^6 chance.
        let (mut a, mut b, mut m) = (Box_::new(), Box_::new(), Box_::new());
        let now = Utc::now();
        let offer = init(&a.pem.clone(), "a", "alpha", "", now).expect("init");
        let to_b = respond(&mut b.store, &offer, &b.pem.clone(), "b", "bravo", "", now).expect("B answers");
        let mut to_a = respond(&mut m.store, &offer, &m.pem.clone(), "b", "bravo", "", now).expect("M answers");
        to_a.session = offer.session.clone();
        let r = reveal(&mut a.store, &to_a, None, None, now).expect("A reveals to M");
        let (code_b, _) = verify(&b.fp.clone(), &r.reveal, now).expect("B verifies the same Na");
        assert_eq!(r.peer_fingerprint, m.fp.clone());
        // B computed its code with its own Nb; A's code used M's key + nonce.
        assert_eq!(code_b, sas::sas_v2(&a.fp.clone(), &b.fp.clone(), &sas::from_hex32(&r.reveal.nonce).expect("na"), &sas::from_hex32(&to_b.nonce).expect("nb")));
        assert_eq!(r.sas, sas::sas_v2(&a.fp.clone(), &m.fp.clone(), &sas::from_hex32(&r.reveal.nonce).expect("na"), &sas::from_hex32(&to_a.nonce).expect("nm")));
    }

    #[test]
    fn a_reused_or_expired_nonce_is_rejected() {
        let (mut a, mut b) = (Box_::new(), Box_::new());
        let now = Utc::now();
        let offer = init(&a.pem.clone(), "a", "alpha", "", now).expect("init");
        let answer = respond(&mut b.store, &offer, &b.pem.clone(), "b", "bravo", "", now).expect("respond");
        reveal(&mut a.store, &answer, None, None, now).expect("first reveal");
        assert_eq!(
            reveal(&mut a.store, &answer, None, None, now).err(),
            Some(PairV2Error::UnknownSession),
            "Na is single-use"
        );

        // Expired at the initiator.
        let (mut a2, mut b2) = (Box_::new(), Box_::new());
        let offer = init(&a2.pem.clone(), "a", "alpha", "", now).expect("init");
        let answer = respond(&mut b2.store, &offer, &b2.pem.clone(), "b", "bravo", "", now).expect("respond");
        let later = now + Duration::seconds(sas::NONCE_TTL_SECS + 1);
        assert_eq!(reveal(&mut a2.store, &answer, None, None, later).err(), Some(PairV2Error::Expired));

        // Expired at the responder.
        let (mut a3, mut b3) = (Box_::new(), Box_::new());
        let offer = init(&a3.pem.clone(), "a", "alpha", "", now).expect("init");
        let answer = respond(&mut b3.store, &offer, &b3.pem.clone(), "b", "bravo", "", now).expect("respond");
        let r = reveal(&mut a3.store, &answer, None, None, now).expect("reveal");
        assert_eq!(verify(&b3.fp.clone(), &r.reveal, later), Err(PairV2Error::Expired));

        // An expired confirm.
        let (mut a4, mut b4) = (Box_::new(), Box_::new());
        let (ca, _) = run(&mut a4, &mut b4, now);
        let err = confirm(&mut a4.store, &a4.fp.clone(), &b4.fp.clone(), &ca, &[], later).expect_err("expired");
        assert!(err.contains("expired"), "{err}");
    }

    #[test]
    fn re_pairing_a_trusted_v1_peer_keeps_its_row_and_upgrades_the_version() {
        let (mut a, mut b) = (Box_::new(), Box_::new());
        let now = Utc::now();
        // B already trusts A from a v1 pairing, with a custom capability and a LAN URL.
        let mut v1 = FederationPeer::pin("old-a", "alpha-old", a.pem.clone()).expect("pin").with_base_url("http://192.168.1.9:38471");
        v1.pairing_version = 1;
        v1.trust = PeerTrust::Trusted;
        v1.capabilities.insert("msg:ws1".to_string());
        b.store.upsert(v1.clone());
        let (_, cb) = run(&mut a, &mut b, now);
        // During the re-pair the row is untouched (still Trusted v1).
        assert_eq!(b.store.get(&a.fp.clone()).expect("A"), &v1);
        assert_eq!(confirm(&mut b.store, &b.fp.clone(), &a.fp.clone(), &cb, &[], now), Ok(ConfirmKind::V2));
        let after = b.store.get(&a.fp.clone()).expect("A");
        assert_eq!(after.pairing_version, PAIRING_VERSION);
        assert_eq!(after.trust, PeerTrust::Trusted);
        assert_eq!(after.subdomain, "alpha-old");
        assert_eq!(after.base_url, "http://192.168.1.9:38471");
        assert_eq!(after.capabilities, v1.capabilities, "permissions kept");
    }

    #[test]
    fn a_pending_peer_cannot_be_confirmed_with_a_v1_code() {
        let mut store = PeerStore::default();
        let peer_key = key();
        let mut p = FederationPeer::pin("p", "p", peer_key.public_key_pem()).expect("pin");
        p.pairing_version = 1;
        let fp = store.upsert(p);
        let local = "local-fp";
        let v1 = super::super::pairing::sas_code(local, &fp);
        let err = confirm(&mut store, local, &fp, &v1, &[], Utc::now()).expect_err("v1 for a new pairing");
        assert!(err.contains("pairing v2"), "{err}");
        assert_eq!(store.get(&fp).expect("p").trust, PeerTrust::Pending);
    }

    #[test]
    fn version_and_self_and_blocked_are_refused() {
        let (a, mut b) = (Box_::new(), Box_::new());
        let now = Utc::now();
        let mut offer = init(&a.pem.clone(), "a", "alpha", "", now).expect("init");
        offer.v = 1;
        assert_eq!(
            respond(&mut b.store, &offer, &b.pem.clone(), "b", "bravo", "", now).err(),
            Some(PairV2Error::UnsupportedVersion(1))
        );
        let self_offer = init(&b.pem.clone(), "b", "bravo", "", now).expect("init");
        assert_eq!(
            respond(&mut b.store, &self_offer, &b.pem.clone(), "b", "bravo", "", now).err(),
            Some(PairV2Error::SelfPair)
        );
        let fp = b.store.upsert(FederationPeer::pin("a", "alpha", a.pem.clone()).expect("pin"));
        b.store.set_trust(&fp, PeerTrust::Blocked);
        let offer = init(&a.pem.clone(), "a", "alpha", "", now).expect("init");
        assert_eq!(
            respond(&mut b.store, &offer, &b.pem.clone(), "b", "bravo", "", now).err(),
            Some(PairV2Error::Blocked)
        );
        // A key that does not match the claimed fingerprint.
        let mut forged = init(&a.pem.clone(), "a", "alpha", "", now).expect("init");
        forged.public_key_pem = key().public_key_pem();
        assert!(matches!(
            respond(&mut b.store, &forged, &b.pem.clone(), "b", "bravo", "", now),
            Err(PairV2Error::BadInput(_))
        ));
    }
}
