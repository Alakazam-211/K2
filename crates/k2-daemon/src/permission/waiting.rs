//! A2 — "Waiting on you": `GET /cli/permission/pending` (PRD rev 3.1 §6.7,
//! §17.8) and the Permission Cards page.
//!
//! People only (the owner token or a login; the route table's floor is
//! Admin, so a Member never gets here over TCP, and an agent passport is
//! refused). It lists only the open requests **this viewer can answer**
//! (its role meets each request's floor), oldest first, each with what a
//! client needs to open the card (`addr` + `cardId`) and draw the row. It
//! never carries the agent's `--why` or a note: those are on the card.

use k2_core::permission::catalog::Proof;
use k2_core::permission::request::PermissionRequest;
use serde_json::{json, Value};

use super::actions::Answerer;
use super::answer::err;
use super::{present, store};
use crate::cli_response::CliResponse;
use crate::compute_routes::Who;

/// `GET /cli/permission/pending`: the open requests `who` may answer.
pub(crate) fn pending(who: &Who) -> CliResponse {
    let answerer = match who {
        Who::Owner => Answerer::OwnerToken,
        Who::Login { username, role } => Answerer::Login { user: username.clone(), role: role_str(*role).to_string() },
        _ => {
            return err(403, "forbidden", "\"Waiting on you\" is for people: an owner or admin login, or the desktop app");
        }
    };
    match store::open(None) {
        Ok(reqs) => CliResponse::ok_json(json!({ "requests": rows_for(&reqs, &answerer) }).to_string()),
        Err(e) => err(500, "db", e),
    }
}

fn role_str(r: k2_core::connect_users::Role) -> &'static str {
    match r {
        k2_core::connect_users::Role::Owner => "owner",
        k2_core::connect_users::Role::Admin => "admin",
        k2_core::connect_users::Role::Member => "member",
    }
}

/// The rows of `reqs` that `who` may answer (floor met, still open, still
/// on a card), oldest first. Pure over the rows (tests drive it).
pub(crate) fn rows_for(reqs: &[PermissionRequest], who: &Answerer) -> Vec<Value> {
    let now = k2_core::overlay::now_secs();
    reqs.iter()
        .filter(|r| r.is_answerable() && r.expires_at > now)
        .filter(|r| present::meets_floor(present::floor_of(&r.floor), who))
        .filter_map(row)
        .collect()
}

/// One §17.8 row: `{requestId, addr, cardId, seq, action, setting,
/// whatChanges, agent, askedFor, floor, proof:{kind}, createdAt, expiresAt}`.
fn row(r: &PermissionRequest) -> Option<Value> {
    let conv = r.conversation_id.as_deref()?;
    let card = r.card_id.as_deref()?;
    let seq = k2_core::overlay::find_thread_item(conv, card).ok().flatten().map(|i| i.seq)?;
    let agent = present::workspace_handle(&r.workspace_id);
    let (setting, what, proof) = match present::find_action(&r.action) {
        Some(reg) => {
            let w = present::words(reg.spec, &present::ctx_of(r));
            let proof = match reg.spec.proof {
                Proof::Click => "click",
                Proof::SettingsConfirm { .. } => "confirm",
            };
            (w.setting, w.what_changes, proof)
        }
        None => (r.action.clone(), String::new(), "click"),
    };
    Some(json!({
        "requestId": r.display_id(),
        "addr": r.addr.clone().unwrap_or_else(|| agent.clone()),
        "cardId": card,
        "seq": seq,
        "action": r.action,
        "setting": setting,
        "whatChanges": what,
        "agent": agent,
        "askedFor": r.asked_for,
        "floor": present::floor_of(&r.floor).as_str(),
        "proof": { "kind": proof },
        "createdAt": r.created_at,
        "expiresAt": r.expires_at,
    }))
}
