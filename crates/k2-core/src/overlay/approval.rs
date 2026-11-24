//! The `approval` card (prd-signed-approvals-v1 S1): a signed approval an
//! agent asked a person for, on the Thread between them.
//!
//! The card never carries the payload bytes or any words describing them.
//! A device fetches the bytes from the request (`GET /cli/approve/get`),
//! works out the words itself and signs on its own screen. What the card
//! holds is the daemon's: the request id, the state, who asked (as the
//! daemon reports it), who is asked, `answerBy`, and after an answer, which
//! device answered. `body` is the generic line older renderers show
//! ("Approval requested: open K2 to read it", SA-15). The card is answered
//! only through `/cli/approve/answer` (the asked person's own login), never
//! the Thread's card answer route, so no passport can take it. App guests
//! never see it (`OverlayDoc::is_action_card`).

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use super::doc::{now_secs, OverlayDoc};
use super::{catalog, store, thread_item, CardCallback, OverlayItem, OverlayLink, WRITE};
use crate::product_name;

/// `kind` of the doc.
pub const KIND: &str = "approval";

/// The plain line every renderer may show (SA-15): never the payload's
/// words, which only the person's device works out.
pub const GENERIC_LINE: &str = concat!("Approval requested: open ", product_name!(), " to read it");

/// The body of an `approval` card.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalCardBody {
    /// `ap_…`.
    pub request_id: String,
    /// `waiting` | `approved` | `declined` | `expired` | `cancelled`.
    pub status: String,
    /// The asking agent's handle, as this daemon reports it (not signed:
    /// the relying party can't know who asked, SA-11).
    pub requested_by: String,
    /// The asked person's login name.
    pub person: String,
    /// Unix seconds: after this the request expires.
    pub answer_by: i64,
    /// After an answer: the device's own name for itself (display only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// After an answer: `phone`, `desktop` or `web`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered_via: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered_at: Option<i64>,
}

impl OverlayDoc {
    /// An `approval` card from the agent's address to the person.
    pub fn approval_card(id: String, from: String, to: String, card: ApprovalCardBody) -> Self {
        let mut doc = Self::text(id, from, to, GENERIC_LINE.to_string(), "card");
        doc.kind = KIND.to_string();
        doc.approval = Some(card);
        doc
    }

    pub fn is_pending_approval(&self) -> bool {
        self.kind == KIND && self.approval.as_ref().is_some_and(|a| a.status == "waiting")
    }
}

/// Post an `approval` card onto a Thread. No overlay expiry entry: the
/// approval store owns `answer_by` and closes the card with
/// [`mark_approval`].
pub fn post_approval(
    conn: &Connection,
    conversation_id: &str,
    project_id: &str,
    from: &str,
    to: &str,
    card: ApprovalCardBody,
) -> Result<(OverlayItem, Vec<OverlayLink>), String> {
    let _guard = WRITE.lock();
    let seq = catalog::next_thread_seq(conn, conversation_id, project_id)?;
    let id = uuid::Uuid::new_v4().to_string();
    let doc = OverlayDoc::approval_card(id.clone(), from.trim().to_string(), to.trim().to_string(), card);
    store::commit_write(&doc, Some((conversation_id, seq)), &[], None)?;
    Ok(super::item_and_thread_link(conversation_id, seq, id, doc))
}

/// Close a waiting `approval` card: its status and, after an answer, the
/// device that answered. A card already closed is [`super::CARD_NOT_PENDING`].
pub fn mark_approval(
    conversation_id: &str,
    card_id: &str,
    status: &str,
    device: Option<String>,
    answered_via: Option<String>,
) -> Result<CardCallback, String> {
    let _guard = WRITE.lock();
    let item = thread_item(conversation_id, card_id)?;
    let mut doc = item.doc;
    if !doc.is_pending_approval() {
        return Err(super::CARD_NOT_PENDING.to_string());
    }
    let id = doc.approval.as_ref().map(|a| a.request_id.clone()).unwrap_or_default();
    if let Some(a) = doc.approval.as_mut() {
        a.status = status.to_string();
        a.device = device;
        a.answered_via = answered_via;
        a.answered_at = Some(now_secs());
    }
    store::put_card_doc(conversation_id, item.seq, &doc)?;
    Ok(CardCallback {
        conversation_id: conversation_id.to_string(),
        doc_id: doc.id.clone(),
        seq: item.seq,
        doc,
        inject_line: format!("approval {id} {status}"),
    })
}
