//! Shape checks only (PRD §6 step 2, step 6): the daemon reads the payload
//! with the `k2-approve` crate's strict parser so that a payload no device
//! would show is refused at the ask, and reads the record the same way at
//! the answer. Nothing here trusts or verifies anything: the bytes are kept
//! exactly as received, and only the relying party checks a signature.

use base64::Engine;

use super::routes::bad;
use crate::cli_response::CliResponse;
use k2_core::approve_requests::MAX_ANSWER_WINDOW_SECS;

/// What the store keeps beside the bytes (for lists, the card and expiry).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Shape {
    pub kind: String,
    pub rp: String,
    pub nonce: String,
    pub answer_by: i64,
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

/// Decode `payload_b64` and check the bytes: the strict form, and an
/// `answer_by` in the future and at most an hour away.
pub(crate) fn payload(payload_b64: Option<&str>, now: i64) -> Result<(Vec<u8>, Shape), CliResponse> {
    let Some(text) = payload_b64.map(str::trim).filter(|s| !s.is_empty()) else {
        return Err(bad("approval_bad_payload", "send the payload's bytes as payload_b64 (standard base64)"));
    };
    let bytes = b64().decode(text).map_err(|_| bad("approval_bad_payload", "payload_b64 is not standard base64"))?;
    let p = k2_approve::payload::parse(&bytes)
        .map_err(|r| bad("approval_bad_payload", format!("the payload is refused ({}): {}", r.code.as_str(), r.detail)))?;
    if p.answer_by_at <= now {
        return Err(bad("approval_expired", "the payload's answer_by has already passed; make a new payload"));
    }
    if p.answer_by_at > now + MAX_ANSWER_WINDOW_SECS {
        return Err(bad(
            "approval_bad_payload",
            format!("answer_by is at most {} minutes away; approvals are answered in minutes", MAX_ANSWER_WINDOW_SECS / 60),
        ));
    }
    let shape = Shape { kind: p.kind, rp: p.rp, nonce: p.nonce, answer_by: p.answer_by_at };
    Ok((bytes, shape))
}

/// A device's approval record, read strictly: it must carry exactly the
/// stored bytes, and its key must be one the payload lists. Returns the
/// record's text (stored as posted), its key id and alg.
pub(crate) fn record(text: &str, stored: &[u8]) -> Result<(String, String, String), CliResponse> {
    let r = k2_approve::record::parse(text.as_bytes())
        .map_err(|e| bad("approval_bad_record", format!("the approval record is refused ({}): {}", e.code.as_str(), e.detail)))?;
    if r.payload != stored {
        return Err(bad("approval_bad_record", "the record signs different bytes from the ones this request carries"));
    }
    let listed = k2_approve::payload::parse(stored).map(|p| p.approver_keys.iter().any(|k| k == &r.key_id)).unwrap_or(false);
    if !listed {
        return Err(bad("approval_bad_record", "the record's key is not one this payload accepts (approver_keys)"));
    }
    Ok((text.to_string(), r.key_id, r.alg.as_str().to_string()))
}

/// The payload's bytes as standard base64 (the phone's copy).
pub(crate) fn encode(bytes: &[u8]) -> String {
    b64().encode(bytes)
}
