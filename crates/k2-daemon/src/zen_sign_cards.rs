//! K2's `zen.sign` card (0.46.5, S0 with Rosson's addition): "Sign
//! <widget>'s current version". From 0.46.5 a widget's version covers its
//! manifest (caps, reads, sources, libraries, CDN files), so a manifest
//! change is a new version that runs without the person's data until
//! someone on every placing Garden's list signs it.
//!
//! - **An agent on every placing Garden's list** signs it itself, as today
//!   (`k2 zen widget sign <w>` vouches; `zen_access_routes::sign`).
//! - **Any other agent** gets 403 `sign_needs_person` from `widget/sign`,
//!   and K2 puts this card on its Thread with the person ([`present_sign`]),
//!   or asks on purpose with `k2 zen widget sign <w> --ask`. The card shows
//!   the version's manifest changes in K2's plain words ([`changes_for`]:
//!   caps added or removed, neighbour reads, sources, libraries, CDN URLs),
//!   never the agent's. Allow signs **that exact version** as the person
//!   (`zen_access_routes::sign_entry`); a later edit is a new version and
//!   needs a new card. "Turn off again" runs the `zen.sign-off` twin.
//! - **The one-time carry log**: a widget whose signature was carried at
//!   upgrade although its manifest had changed offers the same card (the
//!   answers about it carry `review` with the `--ask` command).
//!
//! Like K2's Garden cards (`zen_garden_cards`), this is a standard
//! permission card presented by K2 itself, Beta or not, while Garden access
//! is on (always, in a running daemon).

use k2_core::overlay;
use k2_core::permission::catalog::Floor;
use k2_core::permission::request::{params_hash, PermissionRequest, RequestId, RequestState};
use serde_json::{json, Map, Value as J};

use crate::permission::actions::Ctx;
use crate::permission::{answer, limits, present, store};

/// The catalog actions.
pub(crate) const SIGN: &str = "zen.sign";
pub(crate) const SIGN_OFF: &str = "zen.sign-off";

/// The session K2's own sign requests carry (never batched with an
/// agent's Beta asks).
pub(crate) const K2_SESSION: &str = "k2:zen-sign";

/// The manifest changes the card shows for `widget`'s version `version`
/// (its hash, or a prefix): K2's own words, computed from the files and the
/// signatures, never from the request. "This version is no longer live"
/// when the widget moved on (the check then answers `stale`).
pub(crate) fn changes_for(widget: &str, version: &str) -> String {
    let f = k2_core::zen::ZenFiles::local();
    let st = f.widget_status(widget);
    if st.hash().is_empty() || !st.hash().starts_with(version) {
        return format!("This version of {widget} is no longer live.");
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    match k2_core::zen::version_carry::version_changes(&f, &conn, widget) {
        Ok(Some(t)) => t,
        Ok(None) => format!("This version of {widget} is no longer live."),
        Err(e) => {
            k2_core::log_debug!("[zen-sign] changes for {widget}: {e}");
            format!("K2 couldn't read {widget}'s signatures just now.")
        }
    }
}

/// The words hook (`permission::present`): a `zen.sign` request's values
/// with K2's `changes` added for the `{changes}` placeholder. `None` for
/// every other action.
pub(crate) fn with_computed(action: &str, ctx: &Ctx) -> Option<Ctx> {
    if action != SIGN {
        return None;
    }
    let widget = ctx.params.get("widget").and_then(J::as_str)?;
    let version = ctx.params.get("version").and_then(J::as_str)?;
    let mut out = ctx.clone();
    out.params.insert("changes".to_string(), J::String(changes_for(widget, version)));
    Some(out)
}

/// The card's prompt (what older builds and the inbox show).
fn prompt(agent: &str, widget: &str, changes: &str) -> String {
    format!("{agent} asks you to sign {widget}'s current version. {changes} Signed, it reads and writes Garden data with your rights.")
}

/// Present K2's `zen.sign` card for `widget`'s live version `hash`, asked
/// by workspace `ws`. `explicit`: `--ask` (a refusal is not). Answers:
/// - `{card, request, addr, changes}`: a new card;
/// - `{card, request, waiting: true, changes}`: one already waits for this
///   exact version;
/// - `{card: null, asked: true, changes}`: a refusal after this version's
///   card was answered (no new card unless asked on purpose);
/// - `{card: null, note: true, changes}`: no Thread; an owners-only note;
/// - `{card: null, asked: false, changes}`: Garden access is off.
pub(crate) fn present_sign(ws: &str, widget: &str, hash: &str, explicit: bool) -> J {
    present_sign_with_id(ws, widget, hash, explicit, RequestId::new())
}

pub(crate) fn present_sign_with_id(ws: &str, widget: &str, hash: &str, explicit: bool, id: RequestId) -> J {
    let changes = changes_for(widget, hash);
    if !crate::zen_garden_cards::garden_access_on() {
        return json!({ "card": null, "asked": false, "changes": changes });
    }
    let mut params = Map::new();
    params.insert("widget".to_string(), J::String(widget.to_string()));
    params.insert("version".to_string(), J::String(hash.to_string()));
    let ph = params_hash(&params);
    if let Ok(Some(open)) = store::open_same(ws, SIGN, &ph) {
        return json!({ "card": open.card_id, "request": open.display_id(), "waiting": true, "changes": changes });
    }
    if !explicit && store::count_same_asks_since(ws, SIGN, &ph, 0).unwrap_or(0) > 0 {
        return json!({ "card": null, "asked": true, "changes": changes });
    }
    let handle = present::workspace_handle(ws);
    let text = prompt(&handle, widget, &changes);
    let Some(reg) = present::find_action(SIGN) else {
        k2_core::log_debug!("[zen-sign] {SIGN} is not registered");
        return json!({ "card": null, "asked": false, "changes": changes });
    };
    let Some(conversation_id) = crate::overlay_routes::pinned_chat_of(ws) else {
        return inbox_note(ws, &handle, widget, &text, &changes);
    };
    let at = overlay::now_secs();
    let ctx = Ctx { workspace_id: ws.to_string(), workspace_handle: handle.clone(), params: params.clone() };
    let req = PermissionRequest {
        id,
        workspace_id: ws.to_string(),
        session_id: Some(K2_SESSION.to_string()),
        conversation_id: None,
        addr: Some(handle.clone()),
        card_id: None,
        action: SIGN.to_string(),
        params: J::Object(params),
        params_hash: ph.clone(),
        text_hash: present::text_hash_of(reg.spec, &ctx),
        why: format!("I changed {widget}; it needs your signature to use your data."),
        state: RequestState::Pending,
        floor: present::floor_str(reg.spec.floor).to_string(),
        held_ref: None,
        batch_id: None,
        ask_number: limits::ask_number(ws, SIGN, &ph, at),
        asked_for: None,
        created_at: at,
        expires_at: at + reg.spec.expiry.default_secs,
        answered_at: None,
        answered_by: None,
        answered_via: None,
        reason: None,
        result_message: None,
        updated_why: None,
        updated_at: None,
    };
    if let Err(e) = store::insert(&req) {
        k2_core::log_debug!("[zen-sign] request for {widget}: {e}");
        return json!({ "card": null, "asked": false, "changes": changes });
    }
    let reqs = std::slice::from_ref(&req);
    let card_action = present::card_action(reqs, false, present::single_dedupe(&req));
    let options = [k2_core::permission::label::ALLOW, k2_core::permission::label::DENY];
    let posted = crate::overlay_routes::present_action_card(
        Some(&conversation_id),
        ws,
        &text,
        &options,
        card_action,
        false,
        req.expires_at,
        crate::overlay_routes::CardAudience::Logins(present::eligible_logins(Floor::GardenOwner)),
    );
    let (card_id, addr) = match posted {
        Ok(crate::overlay_routes::Presented::Card { id, addr }) => (id, addr),
        other => {
            if let Err(e) = store::cas_open_to(req.id.as_str(), RequestState::Cancelled) {
                k2_core::log_debug!("[zen-sign] withdraw {}: {e}", req.id);
            }
            k2_core::log_debug!("[zen-sign] card for {widget}: {other:?}");
            return match other {
                Ok(crate::overlay_routes::Presented::NoThread) => inbox_note(ws, &handle, widget, &text, &changes),
                _ => json!({ "card": null, "asked": false, "changes": changes }),
            };
        }
    };
    if let Err(e) = store::set_card(req.id.as_str(), &conversation_id, &card_id, None, req.expires_at) {
        k2_core::log_debug!("[zen-sign] set card {card_id}: {e}");
    }
    if let Ok(Some(stored)) = store::get(req.id.as_str()) {
        answer::audit(k2_core::permission::audit_event::REQUEST, "k2", "daemon", &stored);
    }
    json!({ "card": card_id, "request": req.display_id(), "addr": addr, "changes": changes })
}

/// No Thread: one owners-only inbox note (none while the same is open).
fn inbox_note(ws: &str, agent: &str, widget: &str, text: &str, changes: &str) -> J {
    let title = format!("Sign {widget}'s current version?");
    let open = k2_core::feedback::list_for_project(ws, &k2_core::feedback::ListFilter::Open).unwrap_or_default();
    if !open.iter().any(|i| i.title == title) {
        let body = format!("{text}\n\nTo sign it: open {widget}'s History (its menu in Zen) and press Sign. To say no, do nothing.");
        if let Err(e) = crate::feedback_routes::file_owners_only_notice(ws, agent, &title, &body) {
            k2_core::log_debug!("[zen-sign] inbox note for {widget}: {e}");
        }
    }
    json!({ "card": null, "note": true, "changes": changes })
}

#[cfg(test)]
#[path = "zen_sign_cards_tests.rs"]
mod tests;
