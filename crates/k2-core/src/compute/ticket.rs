//! Controller side of the one-click enroll ticket (§7.2, CN15).
//!
//! The control plane signs tickets with an Ed25519 key that is **only**
//! for tickets (never an edge-attestation key: `edge_attest`'s keys are a
//! separate list, so an edge key can never mint a node). This server
//! trusts:
//!
//! - [`BAKED_KEYS`] — empty until k2-connect ships the route and its key
//!   (spec: `.k2/prds/compute-polish-v1-notes.md`); then the public half
//!   is baked here in a release;
//! - `~/.k2/compute-ticket-keys.json` (`{"keys":[{"kid","pub"}]}`, the
//!   owner's file) for rotation and for testing before the bake.
//!
//! No trusted key → [`supported`] is false and every ticket is refused
//! with `ticket_unsupported` (the capability check: enroll codes keep
//! working). A valid ticket only makes the node **pending**; the SAS
//! confirm still decides.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use aws_lc_rs::signature::{UnparsedPublicKey, ED25519};
use base64::Engine;

use super::proto::ticket::{Ticket, MAX_LIFETIME_SECS};

/// Baked first-party ticket keys `(kid, base64 raw 32-byte public key)`.
pub const BAKED_KEYS: &[(&str, &str)] = &[];

/// The kid the spec reserves for the first key.
pub const FIRST_KID: &str = "k2-compute-ticket-2026-10";

pub fn override_path() -> PathBuf {
    crate::paths::k2_home().join("compute-ticket-keys.json")
}

fn decode_pub(b64: &str) -> Option<[u8; 32]> {
    base64::engine::general_purpose::STANDARD.decode(b64.trim()).ok()?.try_into().ok()
}

/// `kid → key`: baked, then the override file (a bad file is ignored).
pub fn trusted_keys() -> HashMap<String, [u8; 32]> {
    let mut keys = HashMap::new();
    for (kid, b64) in BAKED_KEYS {
        if let Some(k) = decode_pub(b64) {
            keys.insert((*kid).to_string(), k);
        }
    }
    let Ok(raw) = std::fs::read_to_string(override_path()) else { return keys };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
        crate::log_debug!("[compute] ignoring malformed {}", override_path().display());
        return keys;
    };
    for e in v.get("keys").and_then(|k| k.as_array()).cloned().unwrap_or_default() {
        let kid = e.get("kid").and_then(|k| k.as_str()).unwrap_or("").trim().to_string();
        let key = e.get("pub").and_then(|k| k.as_str()).and_then(decode_pub);
        if let (false, Some(k)) = (kid.is_empty(), key) {
            keys.insert(kid, k);
        }
    }
    keys
}

/// True when this server can verify one-click tickets at all.
pub fn supported() -> bool {
    !trusted_keys().is_empty()
}

/// Why a ticket was refused (`code()` goes on the wire).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TicketRefusal {
    Unsupported,
    Malformed(String),
    UnknownKid,
    BadSignature,
    Expired,
    TooLong,
    WrongController,
    WrongNode,
    Replayed,
}

impl TicketRefusal {
    pub fn code(&self) -> &'static str {
        match self {
            TicketRefusal::Unsupported => "ticket_unsupported",
            _ => "ticket_invalid",
        }
    }
    pub fn message(&self) -> String {
        match self {
            TicketRefusal::Unsupported => "this server doesn't take one-click tickets yet; use an enroll code".into(),
            TicketRefusal::Malformed(e) => format!("the ticket is malformed: {e}"),
            TicketRefusal::UnknownKid => "the ticket was signed with a key this server doesn't trust".into(),
            TicketRefusal::BadSignature => "the ticket's signature doesn't verify".into(),
            TicketRefusal::Expired => "the ticket expired; get a new one".into(),
            TicketRefusal::TooLong => "the ticket lives too long to be one the control plane minted".into(),
            TicketRefusal::WrongController => "the ticket is for another server".into(),
            TicketRefusal::WrongNode => "the ticket is for another node key or name".into(),
            TicketRefusal::Replayed => "this ticket was already used".into(),
        }
    }
}

fn used() -> &'static Mutex<HashMap<String, i64>> {
    static U: OnceLock<Mutex<HashMap<String, i64>>> = OnceLock::new();
    U.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Verify a ticket for this server (`own_sub`: the bare tunnel subdomain,
/// or a full host for custom-domain controllers) and the node that
/// presented it. Consumes the nonce on success.
pub fn verify(ticket: &str, own_sub: &str, node_fp: &str, node_name: &str, now: i64) -> Result<Ticket, TicketRefusal> {
    verify_with(&trusted_keys(), ticket, own_sub, node_fp, node_name, now)
}

pub fn verify_with(
    keys: &HashMap<String, [u8; 32]>,
    ticket: &str,
    own_sub: &str,
    node_fp: &str,
    node_name: &str,
    now: i64,
) -> Result<Ticket, TicketRefusal> {
    if keys.is_empty() {
        return Err(TicketRefusal::Unsupported);
    }
    let t = Ticket::decode(ticket).map_err(TicketRefusal::Malformed)?;
    let key = keys.get(&t.kid).ok_or(TicketRefusal::UnknownKid)?;
    let sig = base64::engine::general_purpose::STANDARD
        .decode(t.sig.trim())
        .map_err(|_| TicketRefusal::Malformed("signature isn't base64".into()))?;
    let msg = t.message().map_err(TicketRefusal::Malformed)?;
    UnparsedPublicKey::new(&ED25519, key).verify(&msg, &sig).map_err(|_| TicketRefusal::BadSignature)?;
    if t.exp < now {
        return Err(TicketRefusal::Expired);
    }
    if t.exp > now + MAX_LIFETIME_SECS {
        return Err(TicketRefusal::TooLong);
    }
    let own = own_sub.trim().to_ascii_lowercase();
    let theirs = t.controller_sub.trim().to_ascii_lowercase();
    if own.is_empty() || (theirs != own && format!("{theirs}.k2.dev") != own && theirs != format!("{own}.k2.dev")) {
        return Err(TicketRefusal::WrongController);
    }
    if t.node_fp != node_fp || t.node_name != node_name {
        return Err(TicketRefusal::WrongNode);
    }
    let mut u = used().lock().unwrap_or_else(|e| e.into_inner());
    u.retain(|_, exp| *exp >= now);
    let key = format!("{}:{}", t.kid, t.nonce);
    if u.contains_key(&key) {
        return Err(TicketRefusal::Replayed);
    }
    u.insert(key, t.exp);
    Ok(t)
}

/// Reference minting (what k2-connect's route does), from a raw 32-byte
/// Ed25519 seed. Used by tests and to print the spec's test vector; a
/// server never holds a ticket seed. Returns `(public key b64, ticket)`.
#[allow(clippy::too_many_arguments)]
pub fn mint_with_seed(
    seed: &[u8; 32],
    kid: &str,
    controller_sub: &str,
    node_fp: &str,
    node_name: &str,
    exp: i64,
    nonce: &str,
) -> Result<(String, String), String> {
    use aws_lc_rs::signature::{Ed25519KeyPair, KeyPair};
    let kp = Ed25519KeyPair::from_seed_unchecked(seed).map_err(|_| "bad seed".to_string())?;
    let mut t = Ticket {
        v: 1,
        kid: kid.into(),
        controller_sub: controller_sub.into(),
        node_fp: node_fp.into(),
        node_name: node_name.into(),
        exp,
        nonce: nonce.into(),
        sig: String::new(),
    };
    t.sig = base64::engine::general_purpose::STANDARD.encode(kp.sign(&t.message()?).as_ref());
    Ok((base64::engine::general_purpose::STANDARD.encode(kp.public_key().as_ref()), t.encode()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_lc_rs::signature::{Ed25519KeyPair, KeyPair};

    fn kp() -> Ed25519KeyPair {
        let rng = aws_lc_rs::rand::SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap()
    }

    fn mint(k: &Ed25519KeyPair, kid: &str, sub: &str, fp: &str, name: &str, exp: i64, nonce: &str) -> String {
        let mut t = Ticket {
            v: 1,
            kid: kid.into(),
            controller_sub: sub.into(),
            node_fp: fp.into(),
            node_name: name.into(),
            exp,
            nonce: nonce.into(),
            sig: String::new(),
        };
        let sig = k.sign(&t.message().unwrap());
        t.sig = base64::engine::general_purpose::STANDARD.encode(sig.as_ref());
        t.encode().unwrap()
    }

    #[test]
    fn a_good_ticket_verifies_once_and_everything_else_is_refused() {
        let k = kp();
        let other = kp();
        let mut keys = HashMap::new();
        keys.insert(FIRST_KID.to_string(), <[u8; 32]>::try_from(k.public_key().as_ref()).unwrap());
        let fp = "ab".repeat(32);
        let now = 1_800_000_000;
        let good = mint(&k, FIRST_KID, "rosson", &fp, "mini-1", now + 600, "nonce-good-1");
        let t = verify_with(&keys, &good, "rosson", &fp, "mini-1", now).unwrap();
        assert_eq!(t.node_name, "mini-1");
        assert_eq!(verify_with(&keys, &good, "rosson", &fp, "mini-1", now), Err(TicketRefusal::Replayed));
        let full_host = mint(&k, FIRST_KID, "rosson", &fp, "mini-1", now + 600, "nonce-host");
        assert!(verify_with(&keys, &full_host, "rosson.k2.dev", &fp, "mini-1", now).is_ok(), "bare sub vs full host");

        let cases = [
            (mint(&other, FIRST_KID, "rosson", &fp, "mini-1", now + 600, "n1"), TicketRefusal::BadSignature),
            (mint(&k, "k2-edge-app-2026-09", "rosson", &fp, "mini-1", now + 600, "n2"), TicketRefusal::UnknownKid),
            (mint(&k, FIRST_KID, "rosson", &fp, "mini-1", now - 1, "n3"), TicketRefusal::Expired),
            (mint(&k, FIRST_KID, "rosson", &fp, "mini-1", now + 3600, "n4"), TicketRefusal::TooLong),
            (mint(&k, FIRST_KID, "akzm", &fp, "mini-1", now + 600, "n5"), TicketRefusal::WrongController),
            (mint(&k, FIRST_KID, "rosson", &"cd".repeat(32), "mini-1", now + 600, "n6"), TicketRefusal::WrongNode),
            (mint(&k, FIRST_KID, "rosson", &fp, "mini-2", now + 600, "n7"), TicketRefusal::WrongNode),
        ];
        for (tk, want) in cases {
            assert_eq!(verify_with(&keys, &tk, "rosson", &fp, "mini-1", now), Err(want));
        }
        assert!(matches!(verify_with(&keys, "garbage", "rosson", &fp, "mini-1", now), Err(TicketRefusal::Malformed(_))));
        assert_eq!(verify_with(&HashMap::new(), &good, "rosson", &fp, "mini-1", now), Err(TicketRefusal::Unsupported));
        assert_eq!(TicketRefusal::Unsupported.code(), "ticket_unsupported");
        assert_eq!(TicketRefusal::Expired.code(), "ticket_invalid");
    }

    /// The spec's reference vector (`.k2/prds/compute-polish-v1-notes.md`):
    /// seed 32×0x07, these fields → this exact public key and signature.
    #[test]
    fn reference_vector_for_the_control_plane_spec() {
        let (pubk, tk) = mint_with_seed(&[7u8; 32], FIRST_KID, "rosson", &"ab".repeat(32), "mini-1", 1_800_000_600, "AAAAAAAAAAAAAAAAAAAAAA").unwrap();
        let t = Ticket::decode(&tk).unwrap();
        assert_eq!(pubk, "6kpsY+KcUgq+9VB7Ey7F+ZVHdq6+vnuSQh7qaRRG0iw=");
        // Cross-checked with Python `cryptography` (Ed25519, same message).
        assert_eq!(t.sig, "elSKSpTjMPZA4A0XR07r7117gxxi/kwgn1k0GiIIFpCn0DTznfltg/UbqhKp3zzGRyxD6cUZ8kIOAmzY+ueQBQ==");
        let mut keys = HashMap::new();
        keys.insert(FIRST_KID.to_string(), decode_pub(&pubk).unwrap());
        assert!(verify_with(&keys, &tk, "rosson", &"ab".repeat(32), "mini-1", 1_800_000_000).is_ok());
    }

    #[test]
    fn ticket_keys_are_separate_from_edge_keys() {
        for (kid, _) in crate::edge_attest::BAKED_KEYS {
            assert!(!BAKED_KEYS.iter().any(|(k, _)| k == kid), "{kid} must not mint tickets");
        }
        assert_ne!(override_path().file_name(), crate::edge_attest::override_path().file_name());
    }
}
