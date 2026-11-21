//! `vault.owner-confirm` (0.46.17, Rosson's card #12917): the owner answers
//! their own terminal's vault confirm from the phone.
//!
//! K2 presents this card itself, never an agent: the owner ran a vault step
//! that needs a confirm (`k2 vault grant` of a value someone can read,
//! `reveal`, the first-setup steps, `rekey` and the rest) from their own
//! terminal, and that terminal asked for the phone too
//! (`vault_routes::phone`). The card goes on the Thread of the workspace the
//! terminal is in, pushed to the owner's phones only, and names the exact
//! step in K2's words (`step`, plus the secret, its scope, who gets it, how
//! and under which name where those apply). Values are names only, never a
//! value.
//!
//! - **Allow** takes the card's own confirm (`vault-owner-confirm`, bound to
//!   the request): an owner login's password, or what the same person could
//!   use for the step itself (`phone::strong_enough`), then releases the
//!   terminal's step ([`apply`]).
//! - **Deny** (or Dismiss) stops the terminal at once: it reads the request.
//! - **Who answers:** the owner token or an Owner login (Owner floor).
//! - **Who asks:** K2 only. An agent's `k2 permission request` for it is
//!   refused ([`requestable`]), and it isn't listed for agents.
//! - **Quiet:** no agent turn and no Tickets item: it's the owner's own step
//!   and lasts minutes (`phone::WINDOW_SECS`).

use k2_core::permission::catalog::{
    ActionSpec, Area, ConfirmAction, Direction, Expiry, Floor, Grant, NotCardableSpec, ParamKind, ParamSpec, Proof, SettingsRef, Undo,
    WallCoverage, Words,
};
use k2_core::permission::request::{params_hash, PermissionRequest, RequestId, RequestState};
use serde_json::{Map, Value};

use super::held::text_param;
use super::{Answerer, Applied, Check, Ctx, Registered};
use crate::permission::{answer, approvers, limits, present, store};
use crate::vault_routes::phone;

/// The card's id.
pub(crate) const ACTION: &str = "vault.owner-confirm";
/// The session K2's own owner-confirm cards carry.
pub(crate) const K2_SESSION: &str = "k2:vault-owner-confirm";

const fn text(key: &'static str, required: bool) -> ParamSpec {
    ParamSpec { key, kind: ParamKind::Text, required, list: false, choices: &[] }
}

pub(crate) static OWNER_CONFIRM: ActionSpec = ActionSpec {
    id: ACTION,
    area: Area::Vault,
    params: &[
        ParamSpec { key: "id", kind: ParamKind::HeldRequest, required: true, list: false, choices: &[] },
        text("step", true),
        text("secret", false),
        text("scope", false),
        text("to", false),
        text("as", false),
        text("env_name", false),
        text("file_name", false),
    ],
    floor: Floor::Owner,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::SettingsConfirm { action: ConfirmAction::VaultOwnerConfirm },
    expiry: Expiry { default_secs: phone::WINDOW_SECS, max_secs: phone::WINDOW_SECS },
    words: Words {
        setting: "Confirm a vault step from your terminal",
        what_changes: "{step}",
        consequences: "Allow it only if you just ran this yourself. Your terminal goes on as soon as you allow it. Deny, or let it expire, and nothing happens.",
        scope: "this one step, once",
        duration: "once",
        retry: "",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "vault", path: "Settings → Vault" },
    // No wall: no agent refusal teaches it; K2 presents it itself.
    walls: &[],
    coverage: WallCoverage::None,
    form: None,
    machine_hints: &[],
};

/// Is this one of K2's owner-confirm requests? (No agent turn, no Tickets
/// item, never requested by an agent.)
pub(crate) fn quiet(req: &PermissionRequest) -> bool {
    req.action == ACTION
}

/// An agent may not ask for this card.
pub(crate) fn requestable(action: &str) -> bool {
    action != ACTION
}

fn check(ctx: &Ctx) -> Check {
    match text_param(ctx, "id") {
        Some(id) if phone::is_waiting(id) => Check::Needed,
        _ => Check::Moved("the terminal's step isn't waiting any more (answered on the computer, withdrawn or expired)".into()),
    }
}

/// Allow: the card's confirm, then the terminal's step is released.
fn apply(ctx: &Ctx, who: &Answerer, token: Option<&str>) -> Applied {
    let Some(token) = token else {
        return Applied::Refused { code: "vault_confirm_required".into(), message: "confirm it's you first".into() };
    };
    let Some(request) = answer::current_request() else {
        return Applied::Refused { code: "failed".into(), message: "no request is being answered".into() };
    };
    let Some(id) = text_param(ctx, "id") else {
        return Applied::Refused { code: "bad_params".into(), message: "the card names no step".into() };
    };
    let (owner_token, by) = match who {
        Answerer::OwnerToken => (true, "owner".to_string()),
        Answerer::Login { user, .. } => (false, user.clone()),
    };
    match phone::approve(id, &request, token, &who.stamp(), owner_token, &by) {
        Ok(words) => Applied::Done { message: format!("Confirmed. Your terminal goes on: {}.", words.trim().trim_end_matches('.')) },
        Err((code, message)) => Applied::Refused { code, message },
    }
}

/// Post the card on `ws`'s pinned Thread for the owner. Returns (the
/// request's display id, the card id, the Thread address).
pub(crate) fn present(ws: &str, params: Map<String, Value>, why: String, expires_at: i64) -> Result<(String, String, String), String> {
    let reg = present::find_action(ACTION).ok_or_else(|| format!("{ACTION} is not registered"))?;
    let handle = present::workspace_handle(ws);
    let conversation_id = crate::overlay_routes::pinned_chat_of(ws).ok_or_else(|| format!("{handle} has no Thread to put the card on"))?;
    let at = k2_core::overlay::now_secs();
    let hash = params_hash(&params);
    let held = params.get("id").and_then(Value::as_str).map(str::to_string);
    let ctx = Ctx { workspace_id: ws.to_string(), workspace_handle: handle.clone(), params: params.clone() };
    let req = PermissionRequest {
        id: RequestId::new(),
        workspace_id: ws.to_string(),
        session_id: Some(K2_SESSION.to_string()),
        conversation_id: None,
        addr: Some(handle.clone()),
        card_id: None,
        action: ACTION.to_string(),
        params: Value::Object(params),
        params_hash: hash.clone(),
        text_hash: present::text_hash_of(reg.spec, &ctx),
        why,
        state: RequestState::Pending,
        floor: present::floor_str(reg.spec.floor).to_string(),
        held_ref: held,
        batch_id: None,
        ask_number: limits::ask_number(ws, ACTION, &hash, at),
        asked_for: None,
        created_at: at,
        expires_at,
        answered_at: None,
        answered_by: None,
        answered_via: None,
        reason: None,
        result_message: None,
        updated_why: None,
        updated_at: None,
    };
    store::insert(&req)?;
    let audience = approvers::audience_for_new(&req);
    let (prompt, options) = present::prompt_and_options(std::slice::from_ref(&req), false);
    let opts: Vec<&str> = options.iter().map(String::as_str).collect();
    let card_action = present::card_action(std::slice::from_ref(&req), false, present::single_dedupe(&req));
    let posted = crate::overlay_routes::present_action_card(
        Some(&conversation_id),
        ws,
        &prompt,
        &opts,
        card_action,
        false,
        expires_at,
        crate::overlay_routes::CardAudience::Logins(audience),
    );
    let (card_id, addr) = match posted {
        Ok(crate::overlay_routes::Presented::Card { id, addr }) => (id, addr),
        other => {
            let _ = store::cas_open_to(req.id.as_str(), RequestState::Cancelled);
            return Err(format!("{other:?}"));
        }
    };
    store::set_card(req.id.as_str(), &conversation_id, &card_id, None, expires_at)?;
    if let Ok(Some(stored)) = store::get(req.id.as_str()) {
        answer::audit(k2_core::permission::audit_event::REQUEST, "k2", "daemon", &stored);
    }
    Ok((req.display_id().to_string(), card_id, addr))
}

pub(crate) fn actions() -> Vec<Registered> {
    vec![Registered { spec: &OWNER_CONFIRM, check, apply }]
}

pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    vec![]
}

#[cfg(test)]
#[path = "vault_owner_confirm_tests.rs"]
mod tests;
