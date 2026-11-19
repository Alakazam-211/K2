//! Live receive, `POST /p5/msg` (prd-postal-in-k2-v1 S6): **not mounted
//! in this release.** A K2 box's `<label>.postal.bot` name has to reach the
//! box's own K2 tunnel first, and that routing is the plane's P-6 (K2 Web).
//! Until then every Postal message to a K2 box comes by hold.
//!
//! What is here is the check the listener will run, in K2's review order
//! (rule 2): the `p5-msg-v1` hop proof **first**, against the pinned key
//! of the friend the request says it is from, before the body is parsed;
//! only then the SDK's wire v2 verify. No plane call is made on
//! unauthenticated input (F5).

// Not mounted until S6 (above); its tests run it.
#![cfg_attr(not(test), allow(dead_code))]

use k2_core::postal::sdk::pairing::PublicKey;

/// The proof's domain tag (p5-crypto `TAG_MSG`).
pub const TAG_MSG: &str = "p5-msg-v1";
pub const MSG_PATH: &str = "/p5/msg";

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

/// `"p5-msg-v1" || method || path || content_sha256_hex || timestamp || nonce`.
fn proof_message(method: &str, path: &str, content_sha256_hex: &str, timestamp: &str, nonce: &str) -> Vec<u8> {
    [TAG_MSG, method, path, content_sha256_hex, timestamp, nonce].concat().into_bytes()
}

/// Why a live request was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveRefusal {
    /// Missing or malformed proof headers.
    BadProof,
    /// The sender isn't a friend here (no pinned key for `X-P5-From`).
    NotFriend,
    /// The proof doesn't verify against the pinned key.
    ProofFailed,
    /// Outside ±300 s.
    Stale,
}

/// The hop proof of one `POST /p5/msg`: `from_key` is the pinned key of
/// the friend `X-P5-From` names; `proof_hex` the `X-P5-Proof` header.
pub fn verify_hop(from_key: Option<&PublicKey>, body: &[u8], timestamp: &str, nonce: &str, proof_hex: &str, now: i64) -> Result<(), LiveRefusal> {
    let key = from_key.ok_or(LiveRefusal::NotFriend)?;
    if timestamp.is_empty() || !timestamp.bytes().all(|b| b.is_ascii_digit()) || nonce.is_empty() || nonce.len() > 128 {
        return Err(LiveRefusal::BadProof);
    }
    let ts: i64 = timestamp.parse().map_err(|_| LiveRefusal::BadProof)?;
    if (now - ts).abs() > k2_core::postal::sdk::replay::MAX_SKEW_SECS {
        return Err(LiveRefusal::Stale);
    }
    let sig: [u8; 64] = from_hex(proof_hex).and_then(|v| v.try_into().ok()).ok_or(LiveRefusal::BadProof)?;
    let sha: String = k2_core::postal::sdk::badge::sha256(body).iter().map(|b| format!("{b:02x}")).collect();
    debug_assert!(is_sha256_hex(&sha));
    if key.verifies(&proof_message("POST", MSG_PATH, &sha, timestamp, nonce), &sig) {
        Ok(())
    } else {
        Err(LiveRefusal::ProofFailed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_core::postal::sdk::identity::Identity;
    use k2_core::postal::sdk::pairing::Signer;

    #[test]
    fn the_hop_proof_is_checked_against_the_pinned_key_before_anything_else() {
        let alice = Identity::from_seed([0x11; 32]);
        let mallory = Identity::from_seed([0x66; 32]);
        let body = br#"{"v":2}"#;
        let sha: String = k2_core::postal::sdk::badge::sha256(body).iter().map(|b| format!("{b:02x}")).collect();
        let sign = |who: &Identity| -> String {
            who.sign(&proof_message("POST", MSG_PATH, &sha, "1800000000", "n1")).iter().map(|b| format!("{b:02x}")).collect()
        };
        let pinned = alice.public_key();
        assert_eq!(verify_hop(Some(&pinned), body, "1800000000", "n1", &sign(&alice), 1_800_000_010), Ok(()));
        assert_eq!(verify_hop(Some(&pinned), body, "1800000000", "n1", &sign(&mallory), 1_800_000_010), Err(LiveRefusal::ProofFailed));
        assert_eq!(verify_hop(None, body, "1800000000", "n1", &sign(&alice), 1_800_000_010), Err(LiveRefusal::NotFriend));
        assert_eq!(verify_hop(Some(&pinned), b"{}", "1800000000", "n1", &sign(&alice), 1_800_000_010), Err(LiveRefusal::ProofFailed), "another body");
        assert_eq!(verify_hop(Some(&pinned), body, "1800000000", "n1", &sign(&alice), 1_800_001_000), Err(LiveRefusal::Stale));
        assert_eq!(verify_hop(Some(&pinned), body, "x", "n1", &sign(&alice), 1_800_000_010), Err(LiveRefusal::BadProof));
    }
}
