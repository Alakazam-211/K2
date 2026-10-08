//! Enrollment by code (§7.2 manual path, CN1).
//!
//! The owner gives the node `ABCDE-FGHJK.0123456789abcdef`: a one-time
//! code plus the first 16 hex of the controller's fingerprint. The node
//! sends only the code's binding hash and its public key; the controller
//! answers with its key, a fresh nonce and a signature over the node's
//! fingerprint. The node refuses any controller whose fingerprint doesn't
//! start with the prefix, then both sides show the same SAS.

use std::collections::BTreeMap;

use k2_node_proto::crypto::{self, SigningKey};
use k2_node_proto::frames::{self, EnrollProof, EnrollRequest, HandshakeFrame, Refused, HANDSHAKE_SECS, PROTOCOL};
use k2_node_proto::pairing::{self, EnrollString};
use tokio_tungstenite::tungstenite::Message;

use crate::identity::Pin;
use crate::node::NODE_VERSION;
use crate::session::{parse_hs, recv_text, send_hs};

/// Node names: `mini-1`, `z13flow`.
pub fn valid_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 32
        && n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && n.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
}

pub struct Enrolled {
    pub pin: Pin,
    /// Display form (`482 913`).
    pub sas: String,
}

/// Run the enroll exchange on an open socket.
pub async fn enroll_on<S>(
    ws: &mut S,
    key: &SigningKey,
    controller_base: &str,
    enroll: &EnrollString,
    name: &str,
    labels: &BTreeMap<String, String>,
    now: i64,
) -> Result<Enrolled, String>
where
    S: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
        + futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
{
    if !valid_name(name) {
        return Err(format!("node name {name:?}: use 1-32 of a-z, 0-9 and -"));
    }
    let binding = pairing::code_binding(&enroll.code);
    let node_fp = key.fingerprint();
    let fut = async {
        send_hs(ws, &HandshakeFrame::EnrollRequest(EnrollRequest {
            code_binding: binding.clone(),
            node_public_key_pem: key.spki_pem(),
            name: name.to_string(),
            labels: labels.clone(),
            protocol: PROTOCOL,
            node_version: NODE_VERSION.to_string(),
        }))
        .await?;
        let ch = match parse_hs(&recv_text(ws).await?)? {
            HandshakeFrame::EnrollChallenge(c) => c,
            HandshakeFrame::Refused(r) => return Err(refused_msg(&r)),
            other => return Err(format!("protocol_error: expected enroll_challenge, got {other:?}")),
        };
        let controller_fp = crypto::fingerprint_of_spki_pem(&ch.controller_public_key_pem)?;
        if !enroll.matches_controller(&controller_fp) {
            let _ = send_hs(ws, &HandshakeFrame::Refused(Refused {
                code: "controller_mismatch".into(),
                message: "controller fingerprint doesn't match the enroll string".into(),
            }))
            .await;
            return Err(format!(
                "this controller's fingerprint starts with {}, not {} as the enroll string says. Refusing: \
                 something between this machine and the controller may be impersonating it.",
                &controller_fp[..16],
                enroll.controller_fp_prefix
            ));
        }
        let spki = crypto::pem_decode("PUBLIC KEY", &ch.controller_public_key_pem)?;
        if !crypto::verify_domain(
            &spki,
            frames::DOMAIN_ENROLL_CONTROLLER,
            &[node_fp.as_bytes(), ch.enroll_nonce.as_bytes(), binding.as_bytes(), controller_fp.as_bytes()],
            &ch.sig,
        ) {
            let _ = send_hs(ws, &HandshakeFrame::Refused(Refused { code: "bad_signature".into(), message: "enroll signature is bad".into() })).await;
            return Err("the controller's enroll signature doesn't verify for this node's key; refusing".into());
        }
        let sig = key.sign_domain(
            frames::DOMAIN_ENROLL_NODE,
            &[controller_fp.as_bytes(), ch.enroll_nonce.as_bytes(), binding.as_bytes(), node_fp.as_bytes()],
        )?;
        send_hs(ws, &HandshakeFrame::EnrollProof(EnrollProof { sig })).await?;
        let done = match parse_hs(&recv_text(ws).await?)? {
            HandshakeFrame::EnrollDone(d) => d,
            HandshakeFrame::Refused(r) => return Err(refused_msg(&r)),
            other => return Err(format!("protocol_error: expected enroll_done, got {other:?}")),
        };
        if done.node_id != ch.node_id {
            return Err("protocol_error: node id changed during enrollment".into());
        }
        let sas = pairing::display_sas(&pairing::compute_sas(&controller_fp, &node_fp, &ch.enroll_nonce, &binding));
        let pin = Pin {
            controller_fp,
            controller_public_key_pem: ch.controller_public_key_pem.clone(),
            routes: vec![controller_base.trim_end_matches('/').to_string()],
            node_id: done.node_id,
            name: done.name,
            enrolled_at: now,
            sas: sas.clone(),
            revoked: false,
            revoked_reason: None,
            labels: labels.clone(),
            confirmed: false,
        };
        Ok(Enrolled { pin, sas })
    };
    match tokio::time::timeout(std::time::Duration::from_secs(HANDSHAKE_SECS * 2), fut).await {
        Err(_) => Err("enrollment timed out".into()),
        Ok(r) => r,
    }
}

fn refused_msg(r: &Refused) -> String {
    let hint = match r.code.as_str() {
        "code_invalid" => " (wrong or used code: mint a new one with `k2 compute node add` on the controller)",
        "code_locked" => " (too many wrong codes: mint a new one on the controller)",
        "name_taken" => " (pick another --name)",
        "compute_off" => " (compute nodes are off on the controller: K2_COMPUTE)",
        _ => "",
    };
    format!("controller refused: {}: {}{hint}", r.code, r.message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert!(valid_name("mini-1"));
        assert!(valid_name("z13flow"));
        assert!(!valid_name("-x"));
        assert!(!valid_name("Mini"));
        assert!(!valid_name(""));
        assert!(!valid_name(&"a".repeat(33)));
    }
}
