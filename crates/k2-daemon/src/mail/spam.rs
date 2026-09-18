//! Spam train / allow-block / quarantine (prd-hostmail-next-cli-v1 N10–N12).
//!
//! Train = `Email/set` `$junk` + move Junk/Inbox. No `x:Spam`.
//! allow/block = `x:MemoryLookupKey` then `InvalidateCaches`.
//! Quarantine = that mailbox's Junk; discard = Trash, never destroy.

use std::collections::HashMap;

use crate::cli_response::CliResponse;
use crate::mail::hosted::{self, err_json};
use crate::mail::jmap::StalwartClient;

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct TrainBody {
    address: Option<String>,
    message_id: Option<String>,
    class: Option<String>,
}

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default)]
struct SenderBody {
    sender: Option<String>,
}

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default)]
struct QuarantineBody {
    address: Option<String>,
    id: Option<String>,
}

pub(crate) fn sender_domain(sender: &str) -> Result<String, String> {
    let s = sender.trim().to_ascii_lowercase();
    if s.is_empty() {
        return Err("missing 'sender'".to_string());
    }
    let domain = s.split_once('@').map(|(_, d)| d).unwrap_or(s.as_str());
    let domain = domain.trim_start_matches('.').trim();
    if domain.is_empty() || !domain.contains('.') {
        return Err(format!(
            "sender domain of '{sender}' is empty or not a registrable domain"
        ));
    }
    Ok(domain.to_string())
}

fn require_address_and_id(
    address: Option<&String>,
    id: Option<&String>,
) -> Result<(String, String), CliResponse> {
    let address = address
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            err_json(
                "400 Bad Request",
                "usage",
                "missing 'address' — minted mailbox".to_string(),
            )
        })?;
    let id = id
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| err_json("400 Bad Request", "usage", "missing message id".to_string()))?;
    let row = hosted::load_active_hosted(address).map_err(hosted::addr_err)?;
    Ok((row.address, id.to_string()))
}

fn probe_namespace(engine: &StalwartClient, candidates: &[&str]) -> Result<String, String> {
    let mut errors = Vec::new();
    for ns in candidates {
        match engine.memory_lookup_query_namespace(ns) {
            Ok(_) => return Ok((*ns).to_string()),
            Err(e) => errors.push(format!("{ns}: {e}")),
        }
    }
    Err(format!(
        "x:MemoryLookupKey namespace probe failed (tried {}): {}",
        candidates.join(", "),
        errors.join("; ")
    ))
}

fn allow_or_block(sender: &str, allow: bool) -> CliResponse {
    let domain = match sender_domain(sender) {
        Ok(d) => d,
        Err(h) => return err_json("400 Bad Request", "usage", h),
    };
    let (engine, _) = match hosted::engine() {
        Ok(e) => e,
        Err(r) => return r,
    };
    let candidates: &[&str] = if allow {
        &["trusted-domains"]
    } else {
        &["blocked-domains", "blocked-domain"]
    };
    let ns = match probe_namespace(&engine, candidates) {
        Ok(n) => n,
        Err(e) => return err_json("502 Bad Gateway", "engine", e),
    };
    if let Err(e) = engine.memory_lookup_key_create(&ns, &domain) {
        return err_json("502 Bad Gateway", "engine", e);
    }
    if let Err(e) = engine.action_invalidate_caches() {
        return err_json("502 Bad Gateway", "engine", e);
    }
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "action": if allow { "allow" } else { "block" },
            "namespace": ns,
            "key": domain,
            "sender": sender,
        })
        .to_string(),
    )
}

/// POST `/cli/mail/spam/train` `{address, messageId, class:"ham"|"spam"}`
pub fn handle_train(body: &[u8]) -> CliResponse {
    let b: TrainBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => {
            return err_json(
                "400 Bad Request",
                "usage",
                format!("invalid JSON body: {e}"),
            )
        }
    };
    let class = b
        .class
        .as_deref()
        .map(str::trim)
        .unwrap_or("")
        .to_ascii_lowercase();
    if class != "ham" && class != "spam" {
        return err_json(
            "400 Bad Request",
            "usage",
            "class must be 'ham' or 'spam'".to_string(),
        );
    }
    let (address, message_id) =
        match require_address_and_id(b.address.as_ref(), b.message_id.as_ref()) {
            Ok(v) => v,
            Err(r) => return r,
        };
    let row = match hosted::load_active_hosted(&address) {
        Ok(r) => r,
        Err(e) => return hosted::addr_err(e),
    };
    let account_id = match hosted::account_id_of(&row) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let (engine, _) = match hosted::engine() {
        Ok(e) => e,
        Err(r) => return r,
    };
    let dest_role = if class == "spam" { "junk" } else { "inbox" };
    let dest = match engine.mailbox_id_for_role(&account_id, dest_role) {
        Ok(id) => id,
        Err(e) => return err_json("502 Bad Gateway", "engine", e),
    };
    if let Err(e) = engine.email_set_keyword(&account_id, &message_id, "$junk", class == "spam") {
        return err_json("502 Bad Gateway", "engine", e);
    }
    if let Err(e) = engine.email_move(&account_id, &message_id, &dest) {
        return err_json("502 Bad Gateway", "engine", e);
    }
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "address": address,
            "id": message_id,
            "class": class,
            "folder": dest_role,
        })
        .to_string(),
    )
}

/// POST `/cli/mail/spam/allow` `{sender}`
pub fn handle_allow(body: &[u8]) -> CliResponse {
    let b: SenderBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => {
            return err_json(
                "400 Bad Request",
                "usage",
                format!("invalid JSON body: {e}"),
            )
        }
    };
    let sender = b.sender.as_deref().unwrap_or("");
    allow_or_block(sender, true)
}

/// POST `/cli/mail/spam/block` `{sender}`
pub fn handle_block(body: &[u8]) -> CliResponse {
    let b: SenderBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => {
            return err_json(
                "400 Bad Request",
                "usage",
                format!("invalid JSON body: {e}"),
            )
        }
    };
    let sender = b.sender.as_deref().unwrap_or("");
    allow_or_block(sender, false)
}

/// GET `/cli/mail/spam/quarantine?address=`
pub fn handle_quarantine_get(params: &HashMap<String, String>) -> CliResponse {
    let address = crate::cli::str_param(params, "address");
    if address.trim().is_empty() {
        return err_json(
            "400 Bad Request",
            "usage",
            "missing 'address' — which mailbox's Junk?".to_string(),
        );
    }
    let row = match hosted::load_active_hosted(&address) {
        Ok(r) => r,
        Err(e) => return hosted::addr_err(e),
    };
    let account_id = match hosted::account_id_of(&row) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let (engine, _) = match hosted::engine() {
        Ok(e) => e,
        Err(r) => return r,
    };
    let junk = match engine.mailbox_id_for_role(&account_id, "junk") {
        Ok(id) => id,
        Err(e) => return err_json("502 Bad Gateway", "engine", e),
    };
    let ids = match engine.email_query_ids(
        &account_id,
        serde_json::json!({ "inMailbox": junk }),
        50,
        0,
    ) {
        Ok(ids) => ids,
        Err(e) => return err_json("502 Bad Gateway", "engine", e),
    };
    let summaries = match engine.email_get_summaries(&account_id, &ids) {
        Ok(s) => s,
        Err(e) => return err_json("502 Bad Gateway", "engine", e),
    };
    let messages: Vec<serde_json::Value> = summaries
        .iter()
        .map(|s| {
            serde_json::json!({
                "id": s.id,
                "subject": s.subject,
                "from": s.from.iter().map(|a| a.email.clone()).collect::<Vec<_>>(),
                "receivedAt": s.received_at,
            })
        })
        .collect();
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "address": row.address,
            "folder": "junk",
            "messages": messages,
        })
        .to_string(),
    )
}

fn quarantine_move(body: &[u8], to_role: &str, clear_junk: bool) -> CliResponse {
    let b: QuarantineBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => {
            return err_json(
                "400 Bad Request",
                "usage",
                format!("invalid JSON body: {e}"),
            )
        }
    };
    let (address, id) = match require_address_and_id(b.address.as_ref(), b.id.as_ref()) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let row = match hosted::load_active_hosted(&address) {
        Ok(r) => r,
        Err(e) => return hosted::addr_err(e),
    };
    let account_id = match hosted::account_id_of(&row) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let (engine, _) = match hosted::engine() {
        Ok(e) => e,
        Err(r) => return r,
    };
    let dest = match engine.mailbox_id_for_role(&account_id, to_role) {
        Ok(id) => id,
        Err(e) => return err_json("502 Bad Gateway", "engine", e),
    };
    if clear_junk {
        if let Err(e) = engine.email_set_keyword(&account_id, &id, "$junk", false) {
            return err_json("502 Bad Gateway", "engine", e);
        }
    }
    if let Err(e) = engine.email_move(&account_id, &id, &dest) {
        return err_json("502 Bad Gateway", "engine", e);
    }
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "address": address,
            "id": id,
            "folder": to_role,
            "destroyed": false,
        })
        .to_string(),
    )
}

/// POST `/cli/mail/spam/quarantine/release` `{address, id}` → Inbox.
pub fn handle_quarantine_release(body: &[u8]) -> CliResponse {
    quarantine_move(body, "inbox", true)
}

/// POST `/cli/mail/spam/quarantine/discard` `{address, id}` → Trash.
/// Never Email/set destroy. Never EXPUNGE.
pub fn handle_quarantine_discard(body: &[u8]) -> CliResponse {
    quarantine_move(body, "trash", false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sender_domain_takes_domain_of_mailbox() {
        assert_eq!(sender_domain("Eve@Spam.Example").unwrap(), "spam.example");
        assert_eq!(sender_domain("spam.example").unwrap(), "spam.example");
        assert!(sender_domain("").is_err());
        assert!(sender_domain("nodot").is_err());
    }

    #[test]
    fn train_without_class_is_usage() {
        let r = handle_train(br#"{"address":"a@b.test","messageId":"m1"}"#);
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        assert!(
            r.body.contains("ham") || r.body.contains("class"),
            "{}",
            r.body
        );
    }

    #[test]
    fn allow_without_sender_is_usage() {
        let r = handle_allow(br#"{}"#);
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
    }

    #[test]
    fn quarantine_get_without_address_is_usage_not_405() {
        let r = handle_quarantine_get(&HashMap::new());
        assert_eq!(r.status, "400 Bad Request");
        assert_ne!(r.status, "405 Method Not Allowed");
    }

    #[test]
    fn discard_unknown_address_is_not_found_not_destroy() {
        let r = handle_quarantine_discard(br#"{"address":"no-such-spam@example.test","id":"m1"}"#);
        assert_eq!(r.status, "404 Not Found", "{}", r.body);
        assert!(
            !r.body.to_ascii_lowercase().contains("expunge"),
            "{}",
            r.body
        );
        assert!(!r.body.contains("destroy"), "{}", r.body);
    }
}
