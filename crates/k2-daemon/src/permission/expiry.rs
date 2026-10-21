//! A1 — The hook the card sweep calls for the action cards it closed: mark
//! the request `expired` and wake the agent once (§3.3, C20). A request's
//! expiry **is** its card's `expiresAt`, so the two never disagree.
//!
//! Two entry points, both from `overlay_routes`:
//! - [`on_cards_expired`]: the card sweep (and the check-on-touch before an
//!   answer) just turned these cards `expired`;
//! - [`sweep_requests`]: every minute, open requests past their time whose
//!   card is already closed or gone (a lost card never strands a request).
//!
//! The sweep's `already_done` republish (§6.2) is lane A3's.

use k2_core::overlay::CardCallback;
use k2_core::permission::request::RequestState;
use k2_core::permission::{CARD_ACTION_BATCH, CARD_ACTION_PERMISSION};

use super::{answer, store};

const EXPIRED_MESSAGE: &str = "Expired unanswered.";

fn is_permission_card(cb: &CardCallback) -> bool {
    cb.doc
        .choice
        .as_ref()
        .and_then(|c| c.action.as_ref())
        .is_some_and(|a| a.id == CARD_ACTION_PERMISSION || a.id == CARD_ACTION_BATCH)
}

/// Cards the sweep closed: their open requests turn `expired`, the card's
/// stored view and result follow (same id and seq, a new frame) and each
/// agent is woken once. Returns how many requests expired.
pub(crate) fn on_cards_expired(cbs: &[CardCallback]) -> usize {
    let mut n = 0;
    for cb in cbs.iter().filter(|cb| is_permission_card(cb)) {
        let reqs = match store::for_card(&cb.conversation_id, &cb.doc_id) {
            Ok(r) => r,
            Err(e) => {
                k2_core::log_debug!("[permission] expired card {}: {e}", cb.doc_id);
                continue;
            }
        };
        let open: Vec<_> = reqs.into_iter().filter(|r| r.is_answerable()).collect();
        n += answer::close_open(&open, RequestState::Expired, EXPIRED_MESSAGE).len();
    }
    n
}

/// Open requests past their time (`expires_at <= now`): expire them and
/// their cards. Normally the card sweep got there first; this catches a
/// request whose card is gone or was closed some other way.
pub(crate) fn sweep_requests(now: i64) -> usize {
    match store::open_due(now) {
        Ok(reqs) if !reqs.is_empty() => answer::close_open(&reqs, RequestState::Expired, EXPIRED_MESSAGE).len(),
        Ok(_) => 0,
        Err(e) => {
            k2_core::log_debug!("[permission] request expiry sweep: {e}");
            0
        }
    }
}
