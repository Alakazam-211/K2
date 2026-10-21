//! A2 — `POST /cli/thread/permission-note {addr, id, text, item?}` (PRD rev
//! 3.1 §4.3, §17.5): a person who can't answer the card leaves a note for
//! the people who can. Rosson's rule: while a card waits, people keep
//! working with the agent, and can leave a note for the owner or admin.
//!
//! - **Who:** people below the card's floor (a Member on an Admin card, an
//!   Admin on an Owner card). An approver gets 403 `note_not_allowed` (they
//!   answer instead). Agents, K2 terminals, Garden widgets and App guests
//!   are refused 403 `action_card_person_only` before this runs
//!   (`overlay_routes::handle_permission_card_route`). App guests never see
//!   a permission card at all (K2's rule, 2026-10-21), so there is no guest
//!   note.
//! - **One per person per request:** sending again replaces it; `""`
//!   deletes it. Only while the request is waiting (409
//!   `request_not_pending` after).
//! - **Shape:** at most 500 characters, one paragraph, no links, nothing
//!   secret-shaped: else 400 `bad_note` with `reason` `too_long`,
//!   `newline`, `link` or `secret`.
//! - Approvers see it in `view.notes` ("**dana** (member): …"); the writer
//!   sees it in `noteBox.mine` (lane A3's projection). **A note never
//!   reaches the agent's turn** (the `[permission:]` outcome turn is built
//!   from the request row, never from notes).
//! - The card is rewritten at the same id and seq (a new frame); one
//!   `permission-note` audit line (never the text).

use std::collections::HashMap;

use k2_core::permission::card::Note;
use k2_core::permission::request::{code, limits, PermissionRequest};
use serde_json::json;

use super::actions::Answerer;
use super::answer::err;
use super::audit::{self, NoteChange};
use super::present::{self, CardClose};
use super::store;
use crate::cli_response::CliResponse;

/// The permission card a person route acts on (resolved by
/// `overlay_routes`: the Thread address, imported-card and person checks
/// already done).
pub(crate) struct CardCtx<'a> {
    pub conversation_id: &'a str,
    pub card_id: &'a str,
    /// `choice.action.id == "permission.batch"`.
    pub batch: bool,
}

/// The request on the card a person route names: `item` on a batch card
/// (`pr_…` or the held id), else the card's own request. A batch card
/// without `item` names its one request that `fits`, if exactly one does.
pub(crate) fn target(
    params: &HashMap<String, String>,
    card: &CardCtx<'_>,
    fits: impl Fn(&PermissionRequest) -> bool,
) -> Result<PermissionRequest, CliResponse> {
    let reqs = store::for_card(card.conversation_id, card.card_id).map_err(|e| err(500, "db", e))?;
    if reqs.is_empty() {
        return Err(err(409, code::REQUEST_NOT_PENDING, "this card has no request"));
    }
    let item = params.get("item").map(|s| s.trim()).filter(|s| !s.is_empty());
    match item {
        Some(id) => reqs
            .into_iter()
            .find(|r| r.display_id() == id || r.id.as_str() == id)
            .ok_or_else(|| err(400, "usage", format!("{id} is not on this card"))),
        None if !card.batch => Ok(reqs.into_iter().next().expect("checked non-empty")),
        None => {
            let mut fitting: Vec<PermissionRequest> = reqs.into_iter().filter(|r| fits(r)).collect();
            match fitting.len() {
                1 => Ok(fitting.remove(0)),
                0 => Err(err(409, code::REQUEST_NOT_PENDING, "nothing on this card fits; name the item")),
                _ => Err(err(400, "usage", "a batch card: name the item (item: pr_…)")),
            }
        }
    }
}

/// Why a note is refused (`bad_note`'s `reason`, §17.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BadNote {
    TooLong,
    Newline,
    Link,
    Secret,
}

impl BadNote {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            BadNote::TooLong => "too_long",
            BadNote::Newline => "newline",
            BadNote::Link => "link",
            BadNote::Secret => "secret",
        }
    }

    fn hint(self) -> String {
        match self {
            BadNote::TooLong => format!("a note is at most {} characters", limits::NOTE_MAX_CHARS),
            BadNote::Newline => "a note is one paragraph (no line breaks)".to_string(),
            BadNote::Link => "a note carries no links".to_string(),
            BadNote::Secret => "that looks like a secret; never put one on a card".to_string(),
        }
    }
}

/// Check a (trimmed) note's text. `""` is always fine (it deletes).
pub(crate) fn check_text(text: &str) -> Result<(), BadNote> {
    if text.chars().count() > limits::NOTE_MAX_CHARS {
        return Err(BadNote::TooLong);
    }
    if text.contains(['\n', '\r', '\u{2028}', '\u{2029}']) || text.chars().any(char::is_control) {
        return Err(BadNote::Newline);
    }
    let lower = text.to_ascii_lowercase();
    if ["http://", "https://", "www.", "://"].iter().any(|l| lower.contains(l)) {
        return Err(BadNote::Link);
    }
    if super::routes::secret_shaped(text) {
        return Err(BadNote::Secret);
    }
    Ok(())
}

fn bad_note(b: BadNote) -> CliResponse {
    CliResponse {
        status: "400 Bad Request",
        content_type: "application/json",
        body: json!({ "ok": false, "error": { "code": code::BAD_NOTE, "hint": b.hint(), "reason": b.as_str() } }).to_string(),
    }
}

/// The person's note on a waiting request.
pub(crate) fn handle(params: &HashMap<String, String>, card: &CardCtx<'_>, who: &Answerer) -> CliResponse {
    let Some(raw) = params.get("text").or_else(|| params.get("note")) else {
        return err(400, "usage", "text required (\"\" deletes your note)");
    };
    let text = raw.trim();
    let req = match target(params, card, |r| r.is_answerable()) {
        Ok(r) => r,
        Err(e) => return e,
    };
    if !req.is_answerable() {
        return err(409, code::REQUEST_NOT_PENDING, format!("{} is {}: notes are for a waiting request", req.display_id(), req.state.as_str()));
    }
    let floor = present::floor_of(&req.floor);
    let (user, role) = match who {
        Answerer::Login { user, role } if !present::meets_floor(floor, who) => (user.clone(), role.clone()),
        _ => {
            return err(
                403,
                code::NOTE_NOT_ALLOWED,
                "you can answer this card: Allow or Deny it (a note is for people who wait on an answer)",
            )
        }
    };
    if let Err(b) = check_text(text) {
        return bad_note(b);
    }
    let had = match store::notes(req.id.as_str()) {
        Ok(n) => n.iter().any(|n| n.by == user),
        Err(e) => return err(500, "db", e),
    };
    let now = k2_core::overlay::now_secs();
    if let Err(e) = store::put_note(req.id.as_str(), &user, "person", &role, text, now) {
        return err(500, "db", e);
    }
    let change = match (text.is_empty(), had) {
        (true, _) => NoteChange::Deleted,
        (false, true) => NoteChange::Replaced,
        (false, false) => NoteChange::Written,
    };
    // Same id and seq, a new frame: approvers see the note at once.
    let cb = present::refresh_card(card.conversation_id, card.card_id, CardClose::Void);
    audit::note(who, &req, change);
    let note = if text.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::to_value(Note { by: user, role, text: text.to_string(), at: now }).unwrap_or_default()
    };
    CliResponse::ok_json(
        json!({
            "ok": true,
            "id": card.card_id,
            "seq": cb.as_ref().map(|c| c.seq),
            "requestId": req.display_id(),
            "note": note,
        })
        .to_string(),
    )
}

#[cfg(test)]
#[path = "reach_tests.rs"]
mod reach_tests;
