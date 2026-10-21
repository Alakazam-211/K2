//! A2 — `POST /cli/thread/permission-undo {addr, id, item?}`: **Turn off
//! again** (PRD rev 3.1 §4.4, §17.3).
//!
//! On an approved **standing** grant whose action has a tighten twin
//! (`Undo::Twin`), for 7 days after the approval. Person-only (agents,
//! terminals, widgets and guests are refused before this runs), at the
//! action's floor (403 `action_card_person_only` with `required` below
//! it), no confirm. The twin runs through the same Settings route entry
//! as the approval did (`Registered::apply`), with the person as the actor,
//! so the route's own floor check runs again (§6.4).
//!
//! Errors: as for an answer, plus 409 `undo_expired` (past 7 days, a
//! one-time act, not approved, or already turned off again).
//!
//! **Once.** The request row records it: `result_message` becomes
//! "Turned off again by {who}." (a compare-and-set, so two taps run the
//! twin once). The card is rewritten at the same id and seq with that
//! message as its result; [`is_undone`] tells the view to drop `undo`.
//! The request stays `approved` (it was approved; the grant was later
//! turned off). One `permission-undo` audit line.

use std::collections::HashMap;

use k2_core::permission::catalog::{Grant, ParamKind, Undo};
use k2_core::permission::request::{code, limits, PermissionRequest, RequestState};
use serde_json::{json, Value};

use super::actions::{Answerer, Applied, Ctx};
use super::answer::{below_floor, err};
use super::audit;
use super::notes::{target, CardCtx};
use super::present::{self, CardClose};
use super::store;
use crate::cli_response::CliResponse;

/// The result message an undone request carries (the marker [`is_undone`]
/// reads).
pub(crate) const UNDONE_PREFIX: &str = "Turned off again by ";

/// This approved request's grant was turned off again from its card.
pub(crate) fn is_undone(req: &PermissionRequest) -> bool {
    req.result_message.as_deref().is_some_and(|m| m.starts_with(UNDONE_PREFIX))
}

fn expired(hint: impl std::fmt::Display) -> CliResponse {
    err(409, code::UNDO_EXPIRED, hint)
}

fn who_name(who: &Answerer) -> String {
    match who {
        Answerer::OwnerToken => "owner".to_string(),
        Answerer::Login { user, .. } => user.clone(),
    }
}

/// An undoable request: approved, standing, a twin, within 7 days, not
/// undone yet. `Err` = the `undo_expired` hint.
pub(crate) fn undoable(req: &PermissionRequest, now: i64) -> Result<&'static str, String> {
    let Some(reg) = present::find_action(&req.action) else {
        return Err("K2 no longer offers this action; use Settings".to_string());
    };
    let path = present::words(reg.spec, &present::ctx_of(req)).settings_path;
    let twin = match (req.state, reg.spec.grant, reg.spec.undo) {
        (RequestState::Approved, Grant::Standing, Undo::Twin { action }) => action,
        (RequestState::Approved, _, _) => return Err(format!("only a standing grant is turned off again here; use {path}")),
        (s, _, _) => return Err(format!("{} is {}: only an approved grant is turned off again", req.display_id(), s.as_str())),
    };
    if is_undone(req) {
        let m = req.result_message.clone().unwrap_or_default();
        return Err(format!("{} already: {}", req.display_id(), m.trim_end_matches('.')));
    }
    let at = req.answered_at.unwrap_or(req.created_at);
    if now >= at + limits::TURN_OFF_AGAIN_SECS {
        return Err(format!("Turn off again lasts 7 days after the approval; use {path}"));
    }
    Ok(twin)
}

/// The twin's values: the request's own, with `to=off` when the twin takes
/// a `to` choice (the switch rows' tighten form, lane E1).
fn twin_ctx(req: &PermissionRequest, twin: &k2_core::permission::catalog::ActionSpec) -> Ctx {
    let mut ctx = present::ctx_of(req);
    if let Some(p) = twin.params.iter().find(|p| p.key == "to" && p.kind == ParamKind::Choice && p.choices.contains(&"off")) {
        ctx.params.insert(p.key.to_string(), Value::String("off".to_string()));
    }
    ctx
}

/// The compare-and-set that makes undo single use: set the marker on an
/// approved, not-yet-undone row. `true` only for the one caller that won.
fn claim(id: &str, message: &str) -> Result<bool, String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "UPDATE permission_requests SET result_message = ?2 WHERE id = ?1 AND state = 'approved' \
         AND (result_message IS NULL OR substr(result_message, 1, ?3) != ?4)",
        rusqlite::params![id, message, UNDONE_PREFIX.chars().count() as i64, UNDONE_PREFIX],
    )
    .map(|n| n == 1)
    .map_err(|e| format!("permission_requests: {e}"))
}

/// Put the row's message back (the twin's entry refused).
fn unclaim(id: &str, marker: &str, previous: Option<&str>) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    if let Err(e) = conn.execute(
        "UPDATE permission_requests SET result_message = ?3 WHERE id = ?1 AND result_message = ?2",
        rusqlite::params![id, marker, previous],
    ) {
        k2_core::log_debug!("[permission] undo revert {id}: {e}");
    }
}

/// "Turn off again" on the card's approved standing grant.
pub(crate) fn handle(params: &HashMap<String, String>, card: &CardCtx<'_>, who: &Answerer) -> CliResponse {
    let now = k2_core::overlay::now_secs();
    let req = match target(params, card, |r| undoable(r, now).is_ok()) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let floor = present::floor_of(&req.floor);
    if !present::meets_floor(floor, who) {
        return below_floor(floor.as_str());
    }
    let twin_id = match undoable(&req, now) {
        Ok(t) => t,
        Err(hint) => return expired(hint),
    };
    let Some(twin) = present::find_action(twin_id) else {
        return expired(format!("K2 no longer offers {twin_id}; use Settings"));
    };
    let by = who_name(who);
    let marker = format!("{UNDONE_PREFIX}{by}.");
    match claim(req.id.as_str(), &marker) {
        Ok(true) => {}
        Ok(false) => return expired(format!("{} was already turned off again", req.display_id())),
        Err(e) => return err(500, "db", e),
    }
    // The tighten twin, through the Settings route's own entry, as the
    // person: no confirm (§4.4).
    match (twin.apply)(&twin_ctx(&req, twin.spec), who, None) {
        Applied::Done { .. } => {}
        Applied::Refused { code: c, message } => {
            unclaim(req.id.as_str(), &marker, req.result_message.as_deref());
            audit::undo(who, &req, twin_id, &format!("refused:{c}"));
            let status = if c == code::ACTION_CARD_PERSON_ONLY || c == "forbidden" { 403 } else { 409 };
            return err(status, &c, message);
        }
    }
    let done = match store::get(req.id.as_str()) {
        Ok(Some(r)) => r,
        _ => return err(500, "db", "the request vanished"),
    };
    let cb = present::refresh_card(card.conversation_id, card.card_id, CardClose::Void);
    audit::undo(who, &done, twin_id, "applied");
    CliResponse::ok_json(
        json!({
            "ok": true,
            "id": card.card_id,
            "seq": cb.as_ref().map(|c| c.seq),
            "requestId": done.display_id(),
            "state": done.state.as_str(),
            "undone": true,
            "message": marker,
        })
        .to_string(),
    )
}
