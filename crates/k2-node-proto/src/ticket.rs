//! Enroll tickets: the one-click path for machines on the same Connect
//! account (§7.2, CN15).
//!
//! The control plane (k2-connect, `POST /compute/enroll-ticket`) checks
//! that the signed-in account owns `controller_sub`, then signs
//!
//! ```text
//! k2-compute-ticket-v1\n{controller_sub}\n{node_fp}\n{node_name}\n{exp}\n{nonce}
//! ```
//!
//! with Ed25519 under a ticket-only kid. The ticket string the node
//! carries is base64url (no padding) of the canonical JSON of [`Ticket`].
//! The node only passes it through; the controller verifies it
//! (`k2_core::compute::ticket`) and it only ever gets the node to
//! **pending**: the SAS confirm on the controller still decides.

use serde::{Deserialize, Serialize};

pub const MESSAGE_PREFIX: &str = "k2-compute-ticket-v1";
/// A ticket lives at most this long (the control plane mints 10 minutes).
pub const MAX_LIFETIME_SECS: i64 = 15 * 60;
/// Largest ticket string we parse.
pub const MAX_TICKET_LEN: usize = 2048;

/// Domain for the SAS / enroll-signature binding of a ticket.
const TICKET_BINDING_DOMAIN: &str = "k2-compute-ticket-binding-v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ticket {
    pub v: u32,
    pub kid: String,
    pub controller_sub: String,
    pub node_fp: String,
    pub node_name: String,
    pub exp: i64,
    pub nonce: String,
    /// Ed25519 over [`Ticket::message`], base64 (standard, padded).
    pub sig: String,
}

fn field_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 256 && !s.contains('\n') && !s.contains('\r')
}

impl Ticket {
    /// The exact bytes the control plane signs.
    pub fn message(&self) -> Result<Vec<u8>, String> {
        for (n, f) in [
            ("controller_sub", &self.controller_sub),
            ("node_fp", &self.node_fp),
            ("node_name", &self.node_name),
            ("nonce", &self.nonce),
        ] {
            if !field_ok(f) {
                return Err(format!("ticket {n} is empty, too long or has a newline"));
            }
        }
        Ok(format!(
            "{MESSAGE_PREFIX}\n{}\n{}\n{}\n{}\n{}",
            self.controller_sub, self.node_fp, self.node_name, self.exp, self.nonce
        )
        .into_bytes())
    }

    /// The string form (base64url of canonical JSON).
    pub fn encode(&self) -> Result<String, String> {
        use base64::Engine;
        let json = crate::canonical::to_vec(self)?;
        Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json))
    }

    pub fn decode(s: &str) -> Result<Ticket, String> {
        use base64::Engine;
        let s = s.trim();
        if s.is_empty() || s.len() > MAX_TICKET_LEN {
            return Err("ticket is empty or too long".into());
        }
        let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(s)
            .map_err(|_| "ticket isn't base64url".to_string())?;
        let t: Ticket = serde_json::from_slice(&raw).map_err(|e| format!("ticket JSON: {e}"))?;
        if t.v != 1 {
            return Err(format!("ticket version {} isn't supported", t.v));
        }
        t.message()?;
        Ok(t)
    }

    /// What the enroll signatures and the SAS bind to on the ticket path
    /// (the code path binds to `pairing::code_binding`).
    pub fn binding(&self) -> String {
        let nonce = self.nonce.as_bytes();
        crate::crypto::hex(&crate::crypto::sha256(&crate::crypto::signed_bytes(
            TICKET_BINDING_DOMAIN,
            &[self.controller_sub.as_bytes(), self.node_fp.as_bytes(), nonce],
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t() -> Ticket {
        Ticket {
            v: 1,
            kid: "k2-compute-ticket-2026-10".into(),
            controller_sub: "rosson".into(),
            node_fp: "ab".repeat(32),
            node_name: "mini-1".into(),
            exp: 1_800_000_000,
            nonce: "n0nce".into(),
            sig: "c2ln".into(),
        }
    }

    #[test]
    fn encode_decode_round_trip_and_message_is_exact() {
        let s = t().encode().unwrap();
        assert!(!s.contains('='), "no padding");
        assert_eq!(Ticket::decode(&s).unwrap(), t());
        let m = String::from_utf8(t().message().unwrap()).unwrap();
        assert_eq!(m, format!("k2-compute-ticket-v1\nrosson\n{}\nmini-1\n1800000000\nn0nce", "ab".repeat(32)));
    }

    #[test]
    fn newline_injection_and_junk_are_refused() {
        let mut bad = t();
        bad.node_name = "mini\n1".into();
        assert!(bad.message().is_err());
        assert!(Ticket::decode(&bad.encode().unwrap()).is_err());
        assert!(Ticket::decode("not base64 !!").is_err());
        assert!(Ticket::decode(&"A".repeat(MAX_TICKET_LEN + 1)).is_err());
        let mut v2 = t();
        v2.v = 2;
        assert!(Ticket::decode(&v2.encode().unwrap()).is_err());
    }

    #[test]
    fn binding_depends_on_sub_fp_and_nonce() {
        let a = t().binding();
        let mut b = t();
        b.nonce = "other".into();
        assert_ne!(a, b.binding());
        assert_eq!(a.len(), 64);
    }
}
