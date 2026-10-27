//! K2 presents an agent's held App request as a card on the agent's
//! Thread (audit-app-agent-access-0464 §4 "one approval system"), Beta or
//! not: the `apps.approve` permission card (`permission/actions/held.rs`),
//! posted by K2 itself the way `zen_garden_cards` posts its Garden cards.
//! The owner answers it on the Thread, the phone or Settings → App
//! Approvals; every door approves through `/cli/app/approve` with the same
//! fresh owner confirm, so a card is never wider than Settings.
//!
//! Which requests: the App grants an agent can't apply itself
//! (`super::agent_grants`): copy-host, the guests-rooms grant, a test
//! guest without "Let this agent manage its Apps' users", and a label transfer. The
//! other held kinds keep today's flow (Settings → App Approvals; the Beta's
//! `k2 permission request apps.approve …`). Every held kind could take
//! the same card: call [`present_for`] where it is filed.
//!
//! **"Allow, and let it manage its Apps' users from now on"**
//! (`label::ALWAYS`). A card whose request "Let this agent manage its
//! Apps' users" would have covered (a test guest; the guests-rooms grant
//! in the agent's own room) also offers it: the approval, then that
//! switch through the Settings route, as the person ([`offers_always`],
//! [`turn_on_always`]). The card states what the switch covers
//! ([`SWITCH_COVERS`]). A plain Allow gives a usable test guest: the agent
//! sets its password once (`test_guests::password`).

use k2_core::app_grace::requests as reqs;
use k2_core::permission::catalog::Floor;
use k2_core::permission::request::{params_hash, PermissionRequest, RequestId, RequestState};
use serde_json::{json, Map, Value as J};

use crate::permission::actions::{Answerer, Applied, Ctx};
use crate::permission::{answer, limits, present, store};

/// What "Let this agent manage its Apps' users" covers, in one line: the
/// card says it next to the option that turns it on, the switch's own
/// catalog entry (`apps.manage-users`) says it, and the desktop says it
/// word for word (`APP_USERS_SWITCH_COVERS` in the renderer; a test below
/// fails when they differ).
pub(crate) const SWITCH_COVERS: &str = "On its own Apps only: add and remove App users and test guests, set their passwords, \
     email and names, unlock them, give them roles and its own room, invite people, give its Apps the guests-rooms grant \
     in its own room, and mint App keys that manage its Apps' users. The users it makes are tied to it. Never a box login \
     or another workspace's guests.";

/// The catalog action K2's App cards carry.
pub(crate) const ACTION: &str = "apps.approve";
/// The session K2's own App cards carry (never batched with an agent's
/// own Beta asks).
pub(crate) const K2_SESSION: &str = "k2:apps";

/// Present `r`'s card on the requesting agent's pinned Thread. Returns
/// the answer's `card` value: `{card, request, addr}`, `{card, waiting}`
/// when one waits already, or `{card: null}` (no Thread: Settings → App
/// Approvals still lists it).
pub(crate) fn present_for(r: &reqs::AppRequest) -> J {
    let Some(ws) = r.requester_workspace.as_deref() else { return json!({ "card": null }) };
    let mut params = Map::new();
    params.insert("request".to_string(), J::String(r.id.clone()));
    let hash = params_hash(&params);
    if let Ok(Some(open)) = store::open_same(ws, ACTION, &hash) {
        return json!({ "card": open.card_id, "request": open.display_id(), "waiting": true });
    }
    let Some(reg) = present::find_action(ACTION) else {
        k2_core::log_debug!("[app-cards] {ACTION} is not registered");
        return json!({ "card": null });
    };
    let Some(conversation_id) = crate::overlay_routes::pinned_chat_of(ws) else {
        return json!({ "card": null, "where": "Settings → App Approvals" });
    };
    let handle = present::workspace_handle(ws);
    let at = k2_core::overlay::now_secs();
    let ctx = Ctx { workspace_id: ws.to_string(), workspace_handle: handle.clone(), params: params.clone() };
    let req = PermissionRequest {
        id: RequestId::new(),
        workspace_id: ws.to_string(),
        session_id: Some(K2_SESSION.to_string()),
        conversation_id: None,
        addr: Some(handle.clone()),
        card_id: None,
        action: ACTION.to_string(),
        params: J::Object(params),
        params_hash: hash.clone(),
        text_hash: present::text_hash_of(reg.spec, &ctx),
        why: why_of(r),
        state: RequestState::Pending,
        floor: present::floor_str(reg.spec.floor).to_string(),
        held_ref: Some(r.id.clone()),
        batch_id: None,
        ask_number: limits::ask_number(ws, ACTION, &hash, at),
        asked_for: None,
        created_at: at,
        expires_at: r.expires_at.min(at + reg.spec.expiry.max_secs),
        answered_at: None,
        answered_by: None,
        answered_via: None,
        reason: None,
        result_message: None,
        updated_why: None,
        updated_at: None,
    };
    if let Err(e) = store::insert(&req) {
        k2_core::log_debug!("[app-cards] request for {}: {e}", r.id);
        return json!({ "card": null });
    }
    let (prompt, options) = present::prompt_and_options(std::slice::from_ref(&req), false);
    let opts: Vec<&str> = options.iter().map(String::as_str).collect();
    let card_action = present::card_action(std::slice::from_ref(&req), false, present::single_dedupe(&req));
    // A copy-host request's summary is a count (the agent's notes repeat
    // it); the owner's card names who, from the request's first line.
    let detail = match r.body.get("route").and_then(J::as_str) {
        Some(super::agent_grants::COPY_HOST_APPLY) => r.diff.pointer("/lines/0").and_then(J::as_str).unwrap_or(&r.summary),
        _ => r.summary.as_str(),
    };
    let mut text = format!("{prompt}\n{detail}");
    if options.iter().any(|o| o == k2_core::permission::label::ALWAYS) {
        text.push_str(&format!(
            "\n\"{}\" also turns on \"Let this agent manage its Apps' users\" for {handle}: {SWITCH_COVERS}",
            k2_core::permission::label::ALWAYS
        ));
    }
    let posted = crate::overlay_routes::present_action_card(
        Some(&conversation_id),
        ws,
        &text,
        &opts,
        card_action,
        false,
        req.expires_at,
        crate::overlay_routes::CardAudience::Logins(present::eligible_logins(Floor::Owner)),
    );
    let (card_id, addr) = match posted {
        Ok(crate::overlay_routes::Presented::Card { id, addr }) => (id, addr),
        other => {
            if let Err(e) = store::cas_open_to(req.id.as_str(), RequestState::Cancelled) {
                k2_core::log_debug!("[app-cards] withdraw {}: {e}", req.id);
            }
            k2_core::log_debug!("[app-cards] card for {}: {other:?}", r.id);
            return json!({ "card": null, "where": "Settings → App Approvals" });
        }
    };
    if let Err(e) = store::set_card(req.id.as_str(), &conversation_id, &card_id, None, req.expires_at) {
        k2_core::log_debug!("[app-cards] set card {card_id}: {e}");
    }
    if let Ok(Some(stored)) = store::get(req.id.as_str()) {
        answer::audit(k2_core::permission::audit_event::REQUEST, "k2", "daemon", &stored);
    }
    json!({ "card": card_id, "request": r.id, "addr": addr })
}

/// The card's "<agent> says:" line: the agent's own reason when it gave
/// one (one line, ≤ 200 characters, never secret-shaped), else K2's.
fn why_of(r: &reqs::AppRequest) -> String {
    let own = r
        .reason
        .as_deref()
        .map(str::trim)
        .filter(|w| !w.is_empty() && w.chars().count() <= 200 && !w.contains(['\n', '\r']))
        .filter(|w| !crate::permission::routes::secret_shaped(w));
    match own {
        Some(w) => w.to_string(),
        None => format!("K2 held this for you: {}", r.reasons.first().map(String::as_str).unwrap_or("an owner step")),
    }
}

// ── Always allow ────────────────────────────────────────────────────

/// The held App request behind an `apps.approve` permission request.
fn held_of(req: &PermissionRequest) -> Option<reqs::AppRequest> {
    if req.action != ACTION {
        return None;
    }
    let id = req.params.get("request").and_then(J::as_str)?;
    let h = k2_core::db::shared();
    let conn = h.lock();
    reqs::get(&conn, id).ok().flatten()
}

/// Would "Let this agent manage its Apps' users" have let the requesting agent do
/// this itself? A test guest of its own App, or the guests-rooms grant in
/// its own room only. Never copy-host or a label transfer (owner steps).
fn switch_covers(r: &reqs::AppRequest) -> bool {
    let Some(ws) = r.requester_workspace.as_deref() else { return false };
    if !r.requested_by.starts_with("agent:") || r.kind != reqs::KIND_MANAGE {
        return false;
    }
    match r.body.get("route").and_then(J::as_str) {
        Some(super::agent_grants::TEST_GUEST_ADD) => true,
        Some(super::agent_grants::GUEST_ROOMS_APPLY) => r
            .body
            .pointer("/body/rooms")
            .and_then(J::as_array)
            .is_some_and(|rooms| rooms.iter().all(|x| x.as_str() == Some(ws))),
        _ => false,
    }
}

/// Does this card offer "Always allow"? A single `apps.approve` card whose
/// held request the switch covers, while the switch is still off.
pub(crate) fn offers_always(req: &PermissionRequest) -> bool {
    held_of(req).is_some_and(|r| switch_covers(&r) && !k2_core::workspace::settings::agents_can_manage_app_users(&req.workspace_id))
}

/// [`offers_always`] for a card's view: the request by its display id.
pub(crate) fn offers_always_for(action: &str, request_id: &str) -> bool {
    if action != ACTION {
        return false;
    }
    match store::get_by_display_id(request_id) {
        Ok(Some(req)) => offers_always(&req),
        _ => false,
    }
}

/// "Always allow": after the approval, turn on "Let this agent manage its
/// Apps' users" for the asking workspace through the Settings route's own
/// entry, as the person (the owner both ways). `Ok(line)` for the card.
pub(crate) fn turn_on_always(ctx: &Ctx, who: &Answerer) -> Result<String, String> {
    let ws = ctx.workspace_id.as_str();
    if k2_core::workspace::settings::agents_can_manage_app_users(ws) {
        return Ok(format!("Let this agent manage its Apps' users was already on for {}.", ctx.workspace_handle));
    }
    let r = crate::sidecar_routes::access_entry(who, &json!({ "toggle": "app_users", "workspace": ws, "value": true }));
    if let Applied::Refused { message, .. } = crate::permission::actions::switches::applied(&r, "") {
        return Err(message);
    }
    Ok(format!(
        "Let this agent manage its Apps' users is now on for {}: it manages its own Apps' users itself next time \
         (turn it off in Settings → Workspaces → {} → Agent → Apps).",
        ctx.workspace_handle, ctx.workspace_handle
    ))
}

#[cfg(test)]
mod covers_tests {
    /// The desktop's copy of [`super::SWITCH_COVERS`] is the same words.
    #[test]
    fn the_renderer_says_what_the_switch_covers_word_for_word() {
        let ts = include_str!("../../../../src/renderer/components/SessionView/permissionCardView.ts");
        let want = format!("export const APP_USERS_SWITCH_COVERS =\n  \"{}\"", super::SWITCH_COVERS);
        assert!(ts.contains(&want), "permissionCardView.ts must say, word for word:\n{want}");
    }
}
