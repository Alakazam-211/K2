//! What a card that is no longer waiting says about itself: who decided
//! what and when ("Already answered: … by … at …", "This card expired at
//! …"), and the 409s that carry those words.

use super::*;

/// "2026-11-05 15:12 UTC" for a card's `answeredAt` (the person may be in
/// any time zone, so the server says UTC).
pub(crate) fn card_time_words(at: i64) -> String {
    chrono::DateTime::from_timestamp(at, 0).map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string()).unwrap_or_else(|| at.to_string())
}

/// Who a card's `answeredBy` names, for a sentence: the owner's display
/// name (else "the owner"), a login's username.
pub(crate) fn card_author_words(a: Option<&overlay::PostAuthor>) -> String {
    match a {
        Some(a) if a.kind == k2_core::overlay::doc::AUTHOR_OWNER => {
            a.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "the owner".to_string())
        }
        Some(a) => a.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| a.login.clone()),
        None => "K2".to_string(),
    }
}

/// What a closed card says about itself: "Already answered: “Always trust
/// this folder” by baden at 2026-11-05 15:12 UTC. Always trusted. …".
pub(crate) fn closed_card_words(doc: &overlay::OverlayDoc) -> String {
    let Some(c) = doc.choice.as_ref() else {
        return "this card is no longer waiting for an answer".to_string();
    };
    let at = c.answered_at.map(|t| format!(" at {}", card_time_words(t))).unwrap_or_default();
    let who = card_author_words(c.answered_by.as_ref());
    let result = c
        .action
        .as_ref()
        .and_then(|a| a.result.as_ref())
        .and_then(|r| r["message"].as_str())
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(|m| format!(" {m}"))
        .unwrap_or_default();
    match c.status.as_str() {
        "answered" => format!("Already answered: “{}” by {who}{at}.{result}", c.answer.as_deref().unwrap_or("")),
        "voided" => format!("This card was closed by {who}{at}."),
        "expired" => format!("This card expired{at}."),
        other => format!("This card is {other}."),
    }
}

/// The 409 for a tap on a card that is no longer waiting (another viewer,
/// the CLI or Settings decided first; a view that missed the frame): it
/// says who decided what and when, and carries the card as it is now so
/// the client updates it in place. Never silent.
pub(crate) fn card_not_pending_response(conversation_id: &str, card_id: &str) -> CliResponse {
    let doc = overlay::get_thread_doc(conversation_id, card_id).ok().flatten();
    let seq = k2_core::overlay::store::thread_seq_of(conversation_id, card_id).ok().flatten();
    let hint = doc.as_ref().map(closed_card_words).unwrap_or_else(|| "this card was already answered or dismissed".to_string());
    CliResponse {
        status: "409 Conflict",
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": { "code": "card_not_pending", "hint": hint },
            "id": card_id,
            "seq": seq,
            "conversation_id": conversation_id,
            "doc": doc,
        })
        .to_string(),
    }
}

/// The 409 for an answer to a plain card that is no longer pending. A card
/// whose time ran out (the expiry sweep closed it before this answer, so
/// check-on-touch found nothing due) says so, "This card expired at …", as
/// an action card does; any other close keeps the generic words.
pub(super) fn expired_or_not_pending(conversation_id: &str, card_id: &str) -> CliResponse {
    match overlay::get_thread_doc(conversation_id, card_id) {
        Ok(Some(doc)) if doc.choice.as_ref().is_some_and(|c| c.status == "expired") => {
            error_json("409 Conflict", "card_not_pending", closed_card_words(&doc))
        }
        _ => card_error(overlay::CARD_NOT_PENDING.to_string()),
    }
}
