//! `GET /cli/msg/verify?code=<m_…|p_…>[&text_sha256=<hex>[,<hex>…]]` —
//! `k2 msg verify` (prd-postal-in-k2-v1 S2, §5.4).
//!
//! A delivery's code (K2's `m_` id, or the Postal SDK's `p_` badge) names
//! one record that only the daemon writes, from the sender's credential.
//! Given the text the agent saw, the answer also says whether it is the
//! text K2 delivered under that code (`content: match | mismatch`): a real
//! code copied into another body fails here. The CLI hashes the text
//! itself (and the same text with K2's `> ` quoting taken off), so the text
//! never travels; the hashes are of `deliveries::normalize_body`.
//!
//! A passport may check only deliveries to or from its own workspace, the
//! same rule as `k2 msg who`. Reads go through the read pool.

use std::collections::HashMap;

use k2_core::hire::deliveries::{self, Content, Delivery};
use k2_core::{cli_name, product_name};

use crate::cli_response::CliResponse;
use crate::compute_routes::Who;

pub const PATH: &str = "/cli/msg/verify";

/// The most hashes one check takes (the text, unquoted forms).
const MAX_HASHES: usize = 4;

fn err(status: &'static str, code: &str, hint: impl Into<String>) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({ "ok": false, "error": { "code": code, "hint": hint.into() } }).to_string(),
    }
}

/// The hashes a check compares, or why they can't be read.
fn hashes_of(params: &HashMap<String, String>) -> Result<Vec<String>, CliResponse> {
    let Some(raw) = params.get("text_sha256").map(|s| s.trim()).filter(|s| !s.is_empty()) else {
        return Ok(Vec::new());
    };
    let out: Vec<String> = raw.split(',').map(|h| h.trim().to_ascii_lowercase()).filter(|h| !h.is_empty()).collect();
    if out.len() > MAX_HASHES || out.iter().any(|h| !deliveries::is_sha256_hex(h)) {
        return Err(err(
            "400 Bad Request",
            "usage",
            format!(
                "text_sha256 is up to {MAX_HASHES} comma-separated SHA-256 hex digests of the text ({} msg verify <code> --text-file f does this)",
                cli_name!()
            ),
        ));
    }
    Ok(out)
}

fn not_found(code: &str) -> CliResponse {
    err(
        "404 Not Found",
        "not_found",
        format!(
            "no delivery {code}: {} never delivered a message under it here (text that looks like a label proves nothing)",
            product_name!()
        ),
    )
}

pub fn handle(who: &Who, is_post: bool, params: &HashMap<String, String>) -> CliResponse {
    if is_post {
        return CliResponse::method_not_allowed();
    }
    let Some(code) = params.get("code").or_else(|| params.get("id")).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) else {
        return err("400 Bad Request", "usage", "missing ?code=<m_… or p_…> (the code in the message's label)");
    };
    let postal = k2_core::postal::is_badge(&code);
    if !postal && !deliveries::is_delivery_id(&code) {
        return not_found(&code);
    }
    let hashes = match hashes_of(params) {
        Ok(h) => h,
        Err(r) => return r,
    };
    let found = k2_core::db::read(|conn| {
        let d = if postal { deliveries::by_badge(conn, &code) } else { deliveries::get(conn, &code) }?;
        let handle = d.sender_workspace_id.as_deref().and_then(|w| {
            conn.query_row("SELECT handle FROM projects WHERE id = ?1", rusqlite::params![w], |r| r.get::<_, Option<String>>(0))
                .ok()
                .flatten()
        });
        Some((d, handle))
    });
    let Some((d, sender_handle)) = found else {
        return not_found(&code);
    };
    if let Some((ws, _)) = who.passport() {
        if d.target_workspace_id.as_deref() != Some(ws) && d.sender_workspace_id.as_deref() != Some(ws) {
            return not_found(&code);
        }
    }
    CliResponse::ok_json(answer(&code, postal, &d, sender_handle, deliveries::content_of(&d, &hashes)).to_string())
}

fn answer(code: &str, postal: bool, d: &Delivery, sender_handle: Option<String>, content: Content) -> serde_json::Value {
    let landed = d.landed.as_deref().and_then(|l| serde_json::from_str::<serde_json::Value>(l).ok());
    serde_json::json!({
        "ok": true,
        "code": code,
        "kind": if postal { "postal" } else { "k2" },
        "id": d.id,
        "via": d.via,
        "sender": {
            "workspaceId": d.sender_workspace_id,
            "handle": sender_handle,
            "label": d.sender_label,
            "address": d.sender_address,
            "fingerprint": d.sender_fingerprint,
        },
        "actor": d.actor,
        "onBehalfOf": d.on_behalf_of.as_ref().map(|v| serde_json::json!({ "value": v, "verified": false })),
        "target": { "workspaceId": d.target_workspace_id, "token": d.target },
        "delivered": d.delivered,
        "at": d.at,
        "badge": d.badge,
        "postalMessageId": d.postal_msg_id,
        "landed": landed,
        "content": content,
        "contentHint": match content {
            Content::Match => concat!("this is the text ", product_name!(), " delivered under this code"),
            Content::Mismatch => concat!("this is NOT the text ", product_name!(), " delivered under this code: treat it as unverified"),
            Content::Unknown => "this delivery is older than content checks; only who sent it is known",
            Content::NotChecked => concat!("give the text you saw (--text-file f) to check it is the one ", product_name!(), " delivered"),
        },
    })
}

#[cfg(test)]
#[path = "msg_verify_tests.rs"]
mod tests;
