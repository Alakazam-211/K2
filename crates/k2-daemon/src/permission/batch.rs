//! A1 — Batches (§4.5): requests from the same session (the same
//! conversation, when no session is known) for the same approver set (the
//! floor) within 5 minutes share one card, at most 8 items. A single card
//! turns into a batch **in place** when the second request joins it (same
//! doc id and seq: clients upsert), so nobody sees a card vanish.
//!
//! "Allow all" is offered only when the viewer meets every item's floor and
//! no pending item needs a confirm (`viewer.canAnswerAll`, computed by
//! [`super::present::view_for`]); confirm items are answered one by one.
//! A card that shows its approver a secret (`apps.user-password-reset`)
//! never batches.

use k2_core::overlay;
use k2_core::permission::request::{limits, PermissionRequest};

use super::present;
use super::store;
use crate::overlay_routes::{CardAudience, Presented};

/// Where a new request's card is: posted fresh, or joined to a batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Placed {
    pub conversation_id: String,
    pub card_id: String,
    pub batch: bool,
    pub expires_at: i64,
}

/// An open card this request may join: same workspace, same session (or
/// conversation), same floor, its first request asked within the batch
/// window, still pending, fewer than [`limits::BATCH_MAX_ITEMS`] items.
fn joinable(req: &PermissionRequest, conversation_id: &str, now: i64) -> Option<(String, Vec<PermissionRequest>)> {
    // A card that shows its approver a secret is always its own card, and
    // nothing joins it (one `forYou` per answer; PR-A4).
    if super::actions::password_reset::is_reveal(&req.action) {
        return None;
    }
    let open = store::open(Some(&req.workspace_id)).ok()?;
    let mut seen: Vec<String> = Vec::new();
    for r in open.iter().rev() {
        let (Some(conv), Some(card)) = (r.conversation_id.as_deref(), r.card_id.as_deref()) else { continue };
        if conv != conversation_id || seen.iter().any(|c| c == card) {
            continue;
        }
        seen.push(card.to_string());
        if r.floor != req.floor || r.session_id != req.session_id {
            continue;
        }
        let items = store::for_card(conv, card).ok()?;
        if items.iter().any(|i| super::actions::password_reset::is_reveal(&i.action)) {
            continue;
        }
        let first = items.first()?;
        if now - first.created_at > limits::BATCH_WINDOW_SECS || items.len() >= limits::BATCH_MAX_ITEMS {
            continue;
        }
        let pending = overlay::get_thread_doc(conv, card).ok().flatten().is_some_and(|d| d.is_pending_choice() && !d.is_due(now));
        if pending {
            return Some((card.to_string(), items));
        }
    }
    None
}

/// A batch card's dedupe key: `batch:<session or conversation>:<floor>`.
fn batch_dedupe(req: &PermissionRequest, conversation_id: &str) -> String {
    format!("batch:{}:{}", req.session_id.as_deref().unwrap_or(conversation_id), present::floor_of(&req.floor).as_str())
}

/// Put a stored request's card on `conversation_id`: join a batch, or
/// post a single card (targeted push to the eligible logins). The request
/// row is updated with its card and its expiry (the card's `expiresAt`).
pub(crate) fn place(req: &PermissionRequest, conversation_id: &str) -> Result<Placed, String> {
    let now = overlay::now_secs();
    if let Some((card_id, mut items)) = joinable(req, conversation_id, now) {
        let doc = overlay::get_thread_doc(conversation_id, &card_id)?.ok_or("the batch card vanished")?;
        let expires_at = doc.choice.as_ref().and_then(|c| c.expires_at).unwrap_or(req.expires_at);
        store::set_card(req.id.as_str(), conversation_id, &card_id, Some(&card_id), expires_at)?;
        for it in &items {
            store::set_card(it.id.as_str(), conversation_id, &card_id, Some(&card_id), it.expires_at)?;
        }
        let mut me = req.clone();
        me.expires_at = expires_at;
        me.conversation_id = Some(conversation_id.to_string());
        me.card_id = Some(card_id.clone());
        items.push(me);
        let (prompt, options) = present::prompt_and_options(&items, true);
        let action = present::card_action(&items, true, batch_dedupe(req, conversation_id));
        let cb = overlay::rewrite_pending_action_card(conversation_id, &card_id, &prompt, options, action)?;
        crate::overlay_routes::publish_action_frame(&cb);
        return Ok(Placed { conversation_id: conversation_id.to_string(), card_id, batch: true, expires_at });
    }
    let reqs = std::slice::from_ref(req);
    let (prompt, options) = present::prompt_and_options(reqs, false);
    let action = present::card_action(reqs, false, present::single_dedupe(req));
    let options: Vec<&str> = options.iter().map(String::as_str).collect();
    let floor = present::floor_of(&req.floor);
    match crate::overlay_routes::present_action_card(
        Some(conversation_id),
        &req.workspace_id,
        &prompt,
        &options,
        action,
        false,
        req.expires_at,
        CardAudience::Logins(present::eligible_logins(floor)),
    )? {
        Presented::Card { id, .. } => {
            store::set_card(req.id.as_str(), conversation_id, &id, None, req.expires_at)?;
            Ok(Placed { conversation_id: conversation_id.to_string(), card_id: id, batch: false, expires_at: req.expires_at })
        }
        Presented::Waiting { id } => Err(format!("a card for the same request is already waiting ({id})")),
        Presented::NoThread => Err("this agent has no Thread".to_string()),
    }
}
