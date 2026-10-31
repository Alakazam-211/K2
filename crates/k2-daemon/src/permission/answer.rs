//! A1 — The `permission` / `permission.batch` arms behind
//! `overlay_routes::answer_action_card` (signature frozen; `confirmToken`,
//! `item`, `reason` and `client` come from `params`, and since 0.46.6
//! `reveal` and `mode` for a card that hands its approver a secret once,
//! `apps.user-password-reset`: `reveal: 1` says this client can show it,
//! `mode` picks one of the card's modes): the catalog floor,
//! the locked apply with the current-state check (`already_done` /
//! `stale`), single use (§4.1, §4.5, §6.1, §6.2; C12, Rosson Q1).
//!
//! The order for every item (§6.2), never cut:
//! 1. **compare-and-set** `pending|already_done → applying` in one SQL
//!    statement; zero rows → 409 `request_not_pending`, before any apply;
//! 2. **current state**: the card text hash and the action's `check` on the
//!    stored values: already in the asked-for state → `already_done`
//!    (nothing applied, "Already on"); values that moved → `stale`;
//! 3. **apply** through the route's own entry as the answerer (+ confirm);
//! 4. **close**: the card (when no item is open), `result`, the stored
//!    view, one audit line and the agent's one turn.
//!
//! A missing or refused confirm decides nothing: the request goes back to
//! its open state and the person can try again (403
//! `vault_confirm_required`, 429 `rate_limited`).

use std::collections::HashMap;

use k2_core::overlay::{CardAction, PostAuthor};
use k2_core::permission::catalog::Proof;
use k2_core::permission::request::{code, limits, PermissionRequest, RequestState};
use k2_core::permission::{audit_event, label, CARD_ACTION_BATCH};
use serde_json::json;

use super::actions::{Answerer, Applied, Check};
use super::present::{self, CardClose};
use super::store::{self, Decided};
use super::inject;
use crate::cli_response::CliResponse;

thread_local! {
    /// The request `apply` is running for (its display id): the confirm a
    /// card action takes is bound to it (`secretId`).
    static CURRENT_REQUEST: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
    /// What `apply` hands back to the person who answered ONLY (the HTTP
    /// answer's `forYou`): never the card, the stored result or the agent's
    /// turn (`sew.owner-handoff`'s one-time link).
    static FOR_ANSWERER: std::cell::RefCell<Option<serde_json::Value>> = const { std::cell::RefCell::new(None) };
    /// Where the answering request came in (`loopback` | `lan` | `tunnel`
    /// | `app-link`), tagged by the dispatcher on `/cli/thread/*`. `None`
    /// off the dispatcher: an action that cares refuses.
    static ANSWER_INGRESS: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
    /// The answer's `reveal` / `mode` while `apply` runs (0.46.6,
    /// `apps.user-password-reset`), checked by the action's gate first.
    static ANSWER_OPTS: std::cell::RefCell<AnswerOpts> = const { std::cell::RefCell::new(AnswerOpts { reveal: false, mode: None }) };
}

/// What an Allow carried beyond its label (0.46.6): `reveal` (this client
/// can show a secret to the person who answered) and the `mode` picked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AnswerOpts {
    pub reveal: bool,
    pub mode: Option<String>,
}

/// The answer `apply` is running for: `reveal` and the mode the action's
/// gate settled. Off the answer path (a test calling `apply`): nothing.
pub(crate) fn current_answer() -> AnswerOpts {
    ANSWER_OPTS.with(|c| c.borrow().clone())
}

fn truthy(v: Option<&String>) -> bool {
    v.is_some_and(|s| matches!(s.trim(), "1" | "true" | "yes"))
}

/// Run `f` with the answering request's ingress tagged (the dispatcher
/// knows it per request, as the Settings routes do).
pub(crate) fn with_ingress<T>(ingress: &str, f: impl FnOnce() -> T) -> T {
    let before = ANSWER_INGRESS.with(|c| c.replace(Some(ingress.to_string())));
    let out = f();
    ANSWER_INGRESS.with(|c| *c.borrow_mut() = before);
    out
}

/// The answering request's ingress, if the dispatcher tagged one.
pub(crate) fn answer_ingress() -> Option<String> {
    ANSWER_INGRESS.with(|c| c.borrow().clone())
}

/// A refusal that decides nothing: the request goes back to its open state
/// and the person can answer again (from the right place, or confirmed).
pub(crate) const LOOPBACK_ONLY: &str = "loopback_only";

/// The same kind of refusal for a secret the answering client can't show
/// (`apps.user-password-reset`).
pub(crate) use super::actions::password_reset::REVEAL_NEEDS_SCREEN;

/// The same for an answer the card's own rules leave to the owner (an Admin
/// allowing a chat key past the Admin ceiling, `apps.chat-key`): the card
/// stays open for the owner.
pub(crate) use super::actions::chat_cards::OWNER_IN_PERSON;

/// The display id of the request being applied on this thread.
pub(crate) fn current_request() -> Option<String> {
    CURRENT_REQUEST.with(|c| c.borrow().clone())
}

/// Give the answering person (only) `v` in the answer's `forYou`.
pub(crate) fn set_for_answerer(v: serde_json::Value) {
    FOR_ANSWERER.with(|c| *c.borrow_mut() = Some(v));
}

fn take_for_answerer() -> Option<serde_json::Value> {
    FOR_ANSWERER.with(|c| c.borrow_mut().take())
}

/// The card being answered.
pub(crate) struct CardRef<'a> {
    pub conversation_id: &'a str,
    pub card_id: &'a str,
}

fn status_line(code: u16) -> &'static str {
    match code {
        200 => "200 OK",
        400 => "400 Bad Request",
        403 => "403 Forbidden",
        404 => "404 Not Found",
        405 => "405 Method Not Allowed",
        409 => "409 Conflict",
        413 => "413 Payload Too Large",
        429 => "429 Too Many Requests",
        _ => "500 Internal Server Error",
    }
}

pub(crate) fn err(status: u16, code: &str, hint: impl std::fmt::Display) -> CliResponse {
    CliResponse {
        status: status_line(status),
        content_type: "application/json",
        body: json!({ "ok": false, "error": { "code": code, "hint": hint.to_string() } }).to_string(),
    }
}

/// 403 `action_card_person_only` with `required` (§17.3).
pub(crate) fn below_floor(required: &str) -> CliResponse {
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: json!({
            "ok": false,
            "error": {
                "code": code::ACTION_CARD_PERSON_ONLY,
                "hint": format!("only a person with the {required} role answers this card"),
                "required": required,
            }
        })
        .to_string(),
    }
}

/// Where the answer came from (`view.answer.via`): the body's `client`
/// when it is one of the four, else the owner token is the desktop and a
/// login the web.
fn via_of(params: &HashMap<String, String>, who: &Answerer) -> String {
    match params.get("client").map(|s| s.trim()) {
        Some(c @ ("desktop" | "phone" | "web" | "terminal")) => c.to_string(),
        _ => match who {
            Answerer::OwnerToken => "desktop".to_string(),
            Answerer::Login { .. } => "web".to_string(),
        },
    }
}

fn who_name(who: &Answerer) -> String {
    match who {
        Answerer::OwnerToken => "owner".to_string(),
        Answerer::Login { user, .. } => user.clone(),
    }
}

fn who_label(who: &Answerer) -> (String, &'static str) {
    match who {
        Answerer::OwnerToken => ("owner".to_string(), "owner-token"),
        Answerer::Login { user, .. } => (format!("login:{user}"), "login"),
    }
}

/// One Access Audit line (§6.5): `permission-request|answer|…` with who
/// and `via`. Lane A2 owns the Access Audit filter and the note/undo/
/// switch events; A1 writes the request and answer lines.
pub(crate) fn audit(event: &str, user: &str, via: &str, req: &PermissionRequest) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        event,
        user,
        format!("{} {} {} via={via}", req.display_id(), req.action, req.state.as_str()),
        "-",
        "-",
        "cli",
    ));
}

/// The decision a person sent for one item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decision {
    Allow,
    /// [`label::ALWAYS`] (K2's App cards whose held request "Let this agent
    /// manage its Apps' users" covers): Allow, then turn that switch on
    /// (`apps::held_cards::turn_on_always`).
    Always,
    Deny,
    Close,
}

fn decision_of(label_text: &str) -> Option<Decision> {
    match label_text {
        label::ALLOW => Some(Decision::Allow),
        label::ALWAYS => Some(Decision::Always),
        label::DENY => Some(Decision::Deny),
        label::CLOSE => Some(Decision::Close),
        _ => None,
    }
}

/// The person's answer to a permission or batch card.
pub(crate) fn answer(
    params: &HashMap<String, String>,
    card: CardRef<'_>,
    action: &CardAction,
    who: &Answerer,
    by: Option<PostAuthor>,
) -> CliResponse {
    let Some(answer) = params.get("answer").or_else(|| params.get("option")).map(|a| a.trim().to_string()).filter(|a| !a.is_empty())
    else {
        return err(400, "usage", "answer required");
    };
    let reqs = match store::for_card(card.conversation_id, card.card_id) {
        Ok(r) => r,
        Err(e) => return err(500, "db", e),
    };
    if reqs.is_empty() {
        return err(409, code::REQUEST_NOT_PENDING, "this card has no open request");
    }
    let batch = action.id == CARD_ACTION_BATCH;
    let reason = params.get("reason").map(|r| r.trim().to_string()).filter(|r| !r.is_empty());
    if reason.as_deref().is_some_and(|r| r.chars().count() > limits::DENY_REASON_MAX_CHARS || r.contains(['\n', '\r'])) {
        return err(400, "usage", format!("a deny reason is one line of at most {} characters", limits::DENY_REASON_MAX_CHARS));
    }
    let token = params.get("confirmToken").or_else(|| params.get("confirm_token")).map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    let via = via_of(params, who);
    let opts = AnswerOpts {
        reveal: truthy(params.get("reveal")),
        mode: params.get("mode").map(|m| m.trim().to_string()).filter(|m| !m.is_empty()),
    };

    // What to decide: (request, decision).
    let mut targets: Vec<(PermissionRequest, Decision)> = Vec::new();
    let item = params.get("item").map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let all_label = batch && item.is_none() && (answer == label::ALLOW_ALL || answer == label::DENY_ALL);
    if all_label {
        // Every open item must be at this person's floor (§4.5).
        for r in reqs.iter().filter(|r| r.is_answerable()) {
            if !present::meets_floor(present::floor_of(&r.floor), who) {
                return below_floor(present::floor_of(&r.floor).as_str());
            }
        }
        for r in reqs.iter().filter(|r| r.is_answerable()) {
            let confirm = present::find_action(&r.action).is_some_and(|a| !present::batchable(&a));
            if answer == label::ALLOW_ALL {
                // "Allow all" covers click items only; confirm items (and
                // a card that shows its approver a secret) are answered one
                // by one, as Settings does.
                if !confirm {
                    targets.push((r.clone(), Decision::Allow));
                }
            } else {
                targets.push((r.clone(), Decision::Deny));
            }
        }
        if targets.is_empty() {
            return err(409, code::REQUEST_NOT_PENDING, "nothing on this card is waiting for that answer");
        }
    } else {
        let Some(d) = decision_of(&answer) else {
            return err(400, "usage", format!("'{answer}' is not an option on this card"));
        };
        if d == Decision::Always && (batch || !crate::apps::held_cards::offers_always(&reqs[0])) {
            return err(400, "usage", format!("'{answer}' is not an option on this card"));
        }
        let target = if batch {
            let Some(id) = item.as_deref() else {
                return err(400, "usage", format!("'{answer}' on a batch card names an item"));
            };
            match reqs.iter().find(|r| r.display_id() == id || r.id.as_str() == id) {
                Some(r) => r.clone(),
                None => return err(400, "usage", format!("{id} is not on this card")),
            }
        } else {
            reqs[0].clone()
        };
        let floor = present::floor_of(&target.floor);
        if !present::meets_floor(floor, who) {
            return below_floor(floor.as_str());
        }
        targets.push((target, d));
    }

    let single = !batch;
    let mut decided: Vec<PermissionRequest> = Vec::new();
    for (req, d) in targets {
        match decide_one(&req, d, who, &via, token.as_deref(), reason.as_deref(), &opts, single.then_some(&card)) {
            Ok(done) => decided.push(done),
            // One item of an "Allow all"/"Deny all" lost a race: skip it.
            Err(_) if all_label => {}
            Err(r) => return r,
        }
    }
    if decided.is_empty() {
        return err(409, code::REQUEST_NOT_PENDING, "someone answered first, or the request closed");
    }
    let cb = present::refresh_card(card.conversation_id, card.card_id, CardClose::Answer { label: &answer, by });
    let (user, kind) = who_label(who);
    for r in &decided {
        audit(audit_event::ANSWER, &user, kind, r);
        inject::send_outcome(r);
    }
    let (status, seq) = match &cb {
        Some(cb) => (cb.doc.choice.as_ref().map(|c| c.status.clone()).unwrap_or_default(), cb.seq),
        None => (String::new(), 0),
    };
    let result = match (&cb, single) {
        (_, true) => present::result_for(&decided[0]),
        (Some(cb), false) => cb.doc.choice.as_ref().and_then(|c| c.action.as_ref()).and_then(|a| a.result.clone()).unwrap_or(json!(null)),
        (None, false) => json!(null),
    };
    let mut out = json!({
        "ok": true,
        "id": card.card_id,
        "seq": seq,
        "kind": "choice",
        "status": status,
        "answer": answer,
        "action": { "id": action.id, "result": result },
        "decided": decided.iter().map(|r| json!({ "id": r.display_id(), "state": r.state.as_str() })).collect::<Vec<_>>(),
    });
    // Only this HTTP answer carries it (never the card or the agent's turn).
    if let Some(v) = take_for_answerer() {
        out["forYou"] = v;
    }
    CliResponse::ok_json(out.to_string())
}

/// Decide one request: the compare-and-set, the current-state check, the
/// apply, the stored decision. Returns the request as decided. `single`:
/// the card is this request's alone, so the "applying" view is drawn.
#[allow(clippy::too_many_arguments)]
fn decide_one(
    req: &PermissionRequest,
    d: Decision,
    who: &Answerer,
    via: &str,
    token: Option<&str>,
    reason: Option<&str>,
    opts: &AnswerOpts,
    single: Option<&CardRef<'_>>,
) -> Result<PermissionRequest, CliResponse> {
    let now = k2_core::overlay::now_secs();
    let by = who_name(who);
    let registered = present::find_action(&req.action);
    let prior = req.state;
    let record = |state: RequestState, message: Option<&str>, reason: Option<&str>| -> Result<PermissionRequest, CliResponse> {
        store::decide(req.id.as_str(), &Decided { state, by: Some(&by), via: Some(via), reason, message, at: now })
            .map_err(|e| err(500, "db", e))?;
        store::get(req.id.as_str()).map_err(|e| err(500, "db", e))?.ok_or_else(|| err(500, "db", "the request vanished"))
    };

    if d == Decision::Deny {
        if !store::cas_open_to(req.id.as_str(), RequestState::Denied).map_err(|e| err(500, "db", e))? {
            return Err(err(409, code::REQUEST_NOT_PENDING, "someone answered first, or the request closed"));
        }
        let msg = match reason {
            Some(r) => format!("Denied: {r}"),
            None => "Denied.".to_string(),
        };
        let denied = record(RequestState::Denied, Some(&msg), reason)?;
        // A Garden card's Deny is "Not now" / "Keep it" (prd-garden-access-v1
        // §1.5): stored in its asks table; a no-op for every other action.
        crate::zen_garden_cards::on_denied(&denied);
        return Ok(denied);
    }

    // A confirm action needs its Settings confirm before anything moves.
    let confirm = registered.as_ref().is_some_and(|r| matches!(r.spec.proof, Proof::SettingsConfirm { .. }));
    if matches!(d, Decision::Allow | Decision::Always) && confirm && token.is_none() {
        return Err(err(403, code::VAULT_CONFIRM_REQUIRED, "this action takes the same confirm as Settings: confirm, then answer with confirmToken"));
    }
    // A card that shows its approver a secret (0.46.6): the mode must be
    // one it offers now, and a password or a link needs a client that can
    // show it. Refused here, before anything is decided.
    let mut applied_opts = AnswerOpts::default();
    if matches!(d, Decision::Allow | Decision::Always) && super::actions::password_reset::is_reveal(&req.action) {
        let mode = super::actions::password_reset::gate_answer(req, opts.mode.as_deref(), opts.reveal, via)?;
        applied_opts = AnswerOpts { reveal: opts.reveal, mode: Some(mode) };
    }

    // `publish.pin`: Allow never carries a PIN. The site must have one first,
    // from K2's own form. Refused here, before anything is decided.
    if matches!(d, Decision::Allow | Decision::Always) {
        super::actions::publish::gate_answer(req)?;
    }

    // 1. The compare-and-set, before any apply (C12).
    if !store::cas_open_to(req.id.as_str(), RequestState::Applying).map_err(|e| err(500, "db", e))? {
        return Err(err(409, code::REQUEST_NOT_PENDING, "someone answered first, or the request closed"));
    }
    let Some(registered) = registered else {
        return record(RequestState::Stale, Some("K2 no longer offers this request"), None);
    };

    // 2. The current state, on the stored values and the stored text.
    let ctx = present::ctx_of(req);
    if present::text_hash_of(registered.spec, &ctx) != req.text_hash {
        return record(RequestState::Stale, Some("the card's wording changed after it was asked"), None);
    }
    match (registered.check)(&ctx) {
        Check::AlreadyDone => {
            let on = present::on_word(registered.spec.direction);
            let msg = format!("Already {on}. Nothing was applied.");
            let msg = msg[..1].to_uppercase() + &msg[1..];
            return record(RequestState::AlreadyDone, Some(&msg), None);
        }
        Check::Moved(why) | Check::Unavailable { why, .. } => return record(RequestState::Stale, Some(&why), None),
        Check::Needed => {}
    }
    if d == Decision::Close {
        // `Close` answers an already-done card; this one isn't any more.
        store::revert_applying(req.id.as_str(), prior).map_err(|e| err(500, "db", e))?;
        return Err(err(400, "usage", "'Close' is not an option on this card: it needs an answer again"));
    }

    // "Applying…" (the card still pending, same id and seq) only once the
    // check passed: the card is on the asking agent's Thread, and an
    // earlier flip told it when to swap the version under the check
    // (security review finding 1).
    if matches!(d, Decision::Allow | Decision::Always) {
        if let Some(card) = single {
            if let Some(view) = store::get(req.id.as_str()).ok().flatten().and_then(|r| present::view_for(&[r], false, None)) {
                if let Err(e) = crate::overlay_routes::update_action_view(card.conversation_id, card.card_id, view) {
                    k2_core::log_debug!("[permission] applying view for {}: {e}", req.id);
                }
            }
        }
    }

    // 3. The apply, through the route's own entry, as the answerer.
    CURRENT_REQUEST.with(|c| *c.borrow_mut() = Some(req.display_id().to_string()));
    ANSWER_OPTS.with(|c| *c.borrow_mut() = applied_opts);
    let applied = (registered.apply)(&ctx, who, token);
    ANSWER_OPTS.with(|c| *c.borrow_mut() = AnswerOpts::default());
    CURRENT_REQUEST.with(|c| *c.borrow_mut() = None);
    match applied {
        Applied::Done { message } => {
            let w = present::words(registered.spec, &ctx);
            let mut msg = if message.trim().is_empty() {
                match registered.spec.grant {
                    k2_core::permission::catalog::Grant::Standing => format!("Allowed. Turn it off again in {}.", w.settings_path),
                    k2_core::permission::catalog::Grant::OneTime => "Allowed.".to_string(),
                }
            } else {
                message
            };
            if d == Decision::Always {
                // The one-off is done; the switch is a second write through
                // the Settings route, as this person. Its refusal never
                // undoes the one-off: the card says what happened.
                match crate::apps::held_cards::turn_on_always(&ctx, who) {
                    Ok(line) => msg = format!("{msg} {line}"),
                    Err(e) => msg = format!("{msg} The switch was not turned on: {e}"),
                }
            }
            record(RequestState::Approved, Some(&msg), None)
        }
        Applied::Refused { code: c, message }
            if c == code::VAULT_CONFIRM_REQUIRED
                || c == "rate_limited"
                || c == LOOPBACK_ONLY
                || c == REVEAL_NEEDS_SCREEN
                || c == OWNER_IN_PERSON =>
        {
            store::revert_applying(req.id.as_str(), prior).map_err(|e| err(500, "db", e))?;
            let status = if c == "rate_limited" { 429 } else { 403 };
            Err(err(status, &c, message))
        }
        Applied::Refused { message, .. } => record(RequestState::Failed, Some(&message), None),
    }
}

/// Close open requests for the daemon (no person): the agent withdrew
/// them (`cancelled`), their time ran out (`expired`) or a permission-cards
/// switch turned off (`closed`, §2.5). Each request wins its own
/// compare-and-set; its card is refreshed (voided / expired when nothing on
/// it stays open) and the agent gets one turn (none for `cancelled`).
/// Returns the requests closed here.
pub(crate) fn close_open(reqs: &[PermissionRequest], to: RequestState, message: &str) -> Vec<PermissionRequest> {
    debug_assert!(matches!(to, RequestState::Cancelled | RequestState::Expired | RequestState::Closed));
    let now = k2_core::overlay::now_secs();
    let mut closed = Vec::new();
    let mut cards: Vec<(String, String)> = Vec::new();
    for req in reqs {
        match store::cas_open_to(req.id.as_str(), to) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(e) => {
                k2_core::log_debug!("[permission] close {}: {e}", req.id);
                continue;
            }
        }
        if let Err(e) = store::decide(req.id.as_str(), &Decided { state: to, by: None, via: None, reason: None, message: Some(message), at: now }) {
            k2_core::log_debug!("[permission] close {}: {e}", req.id);
        }
        if let (Some(c), Some(k)) = (req.conversation_id.clone(), req.card_id.clone()) {
            if !cards.contains(&(c.clone(), k.clone())) {
                cards.push((c, k));
            }
        }
        if let Ok(Some(r)) = store::get(req.id.as_str()) {
            closed.push(r);
        }
    }
    for (c, k) in &cards {
        present::refresh_card(c, k, if to == RequestState::Expired { CardClose::Expire } else { CardClose::Void });
    }
    for r in &closed {
        audit(audit_event::ANSWER, "k2", "daemon", r);
        inject::send_outcome(r);
    }
    closed
}

/// The switch-off close (§2.5): every open request on this server (or of
/// one workspace, when its agent switch turned off) closes with one turn
/// each. Called from the settings write path (lane B wires it into the
/// `permissionCards` setting and `agent-access/set toggle=permission_cards`),
/// so it runs headless.
pub(crate) fn close_for_switch_off(workspace_id: Option<&str>) -> usize {
    match store::open(workspace_id) {
        Ok(reqs) => close_open(&reqs, RequestState::Closed, "Closed: permission cards were turned off.").len(),
        Err(e) => {
            k2_core::log_debug!("[permission] switch-off close: {e}");
            0
        }
    }
}
