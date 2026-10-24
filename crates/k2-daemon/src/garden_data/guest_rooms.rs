//! RS-A15 `guests.rooms` / `guests.setRooms`, the Garden transport
//! (room-scope amendment 2026-10-18: one reveal = App gateway + Garden
//! bridge). The same catalog rows as the App route
//! (`crate::guest_rooms_routes`), reached through `POST /cli/zen/call` on
//! this computer's daemon.
//!
//! - **Owner rights** (Gardens act as the owner): any local App's guests,
//!   any room on this box. A widget an agent made acts with that agent's
//!   rights instead (creator rights, room-scope §0.2): only its own
//!   workspace's Apps, only under the owner's `guests-rooms` grant and
//!   within it. An unsigned widget is `author_unknown`.
//! - The widget's manifest asks for `guests:rooms` (`cap_not_granted`
//!   otherwise); a paused placement can't write.
//! - **Every write that changes something waits for a person** (E4, like
//!   G8 `guests.invite`): without a confirm the daemon answers `403
//!   confirm_required` with the exact change (`change`) and a `secretId`
//!   bound to it. K2's renderer, never the widget, shows that change on a
//!   sheet outside the frame, takes the person's vault confirm (Touch ID,
//!   the login or vault password; action [`CONFIRM_ACTION`], that
//!   `secretId`) and repeats the call with `confirmToken`. The token works
//!   once, within 60 seconds, for exactly that change. A no-op set needs no
//!   confirm (nothing changes).
//! - The rules are `k2_core::skin::guest_rooms` (own guests, never above
//!   the role, idempotent, audited with `via: garden`, 60/min per App).
//! - `app.rooms` / `guests.list` (prd-app-caps-0464-v1 AC3, catalog v22):
//!   reads, so no confirm sheet; each spends the placement's read budget.
//!   The manifest must ask for `guests:rooms` (`app.rooms` has no cap row,
//!   the check is here). Owner rights: any local App. An agent-made widget:
//!   only its own workspace's Apps; `app.rooms` reads without the grant
//!   (as the App itself can), `guests.list` needs the owner's grant.

use serde_json::{json, Value as J};
use sha2::{Digest, Sha256};

use k2_core::contract::{catalog, Effect};
use k2_core::garden_data::{authors, MadeBy};
use k2_core::skin::guest_rooms::{self as gr, Ceiling};

use super::budget::{self, BudgetKey, Spend};
use super::call::{made_by_label, resolve, CallBody, Placed};
use crate::cli_response::CliResponse;

/// The vault confirm action a Garden guest-rooms write takes
/// (`vault_routes::CONFIRM_ACTIONS`).
pub const CONFIRM_ACTION: &str = "garden-guests";

pub fn is_verb(verb: &str) -> bool {
    matches!(verb, "guests.rooms" | "guests.setRooms" | "app.rooms" | "guests.list")
}

fn status_line(code: u16) -> &'static str {
    match code {
        400 => "400 Bad Request",
        403 => "403 Forbidden",
        404 => "404 Not Found",
        409 => "409 Conflict",
        423 => "423 Locked",
        429 => "429 Too Many Requests",
        503 => "503 Service Unavailable",
        _ => "500 Internal Server Error",
    }
}

fn no(status: u16, code: &str, hint: impl Into<String>, extra: Option<J>) -> CliResponse {
    let mut e = json!({ "code": code, "hint": hint.into() });
    if let (Some(J::Object(x)), Some(o)) = (extra, e.as_object_mut()) {
        o.extend(x);
    }
    CliResponse { status: status_line(status), content_type: "application/json", body: json!({ "error": e }).to_string() }
}

fn core(e: gr::Refusal) -> CliResponse {
    let extra = match &e {
        gr::Refusal::RateLimited { retry_after_ms } => Some(json!({ "retryAfterMs": retry_after_ms })),
        gr::Refusal::OutsideApp(r) | gr::Refusal::AboveRole(r) => Some(json!({ "rooms": r })),
        _ => None,
    };
    no(e.status(), e.code(), e.hint(), extra)
}

/// One `guests.*` call through the widget door (owner token and Origin
/// already checked by `call::handle`).
pub fn handle(b: &CallBody) -> CliResponse {
    match run(b) {
        Ok(v) => crate::garden_data::ok(json!({ "ok": true, "result": v })),
        Err(r) => r,
    }
}

/// `garden:<widget>` (owner-made) or `garden:<widget> (agent:<handle>)`.
fn actor(w: &Placed, maker: &MadeBy) -> String {
    match maker {
        MadeBy::Agent { .. } => format!("garden:{} ({})", w.widget, made_by_label(maker)),
        _ => format!("garden:{}", w.widget),
    }
}

/// The confirm's binding: this placement, App, guest and resulting rooms.
pub fn secret_id(w_garden: &str, w_placement: &str, service: &str, username: &str, rooms: &[String]) -> String {
    let mut r = rooms.to_vec();
    r.sort();
    let canon = json!([w_garden, w_placement, service, username, r]).to_string();
    let h = Sha256::digest(canon.as_bytes());
    format!("guests:{}", h.iter().map(|b| format!("{b:02x}")).collect::<String>())
}

fn take_confirm(token: &str, secret: &str) -> Result<(), CliResponse> {
    let again = || no(403, "confirm_required", "confirm again: a confirm works once, for exactly this change, within 60 seconds", None);
    #[cfg(test)]
    let vault = crate::apps::egress_routes::test_seams::vault();
    #[cfg(not(test))]
    let vault = crate::vault::get().ok();
    let Some(vault) = vault else {
        return Err(no(503, "failed", "the vault isn't open, so K2 can't take your confirm", None));
    };
    vault.confirm.take(token, CONFIRM_ACTION, Some(secret), "owner").map(|_| ()).map_err(|_| again())
}

fn run(b: &CallBody) -> Result<J, CliResponse> {
    let row = catalog().verb(&b.verb).ok_or_else(|| no(400, "unknown_verb", format!("no verb {}", b.verb), None))?;
    let w = resolve(b).map_err(|e| crate::garden_data::response(&e))?;
    if !w.signed && w.full_caps.iter().any(|c| c == gr::CAP) {
        // Garden access (switch on, G15): only a version that passes the
        // signer gate changes or reads guests as the person.
        return Err(crate::garden_data::response(&crate::garden_data::call::needs_editor(&w)));
    }
    if !w.caps.iter().any(|c| c == gr::CAP) {
        return Err(no(
            403,
            "cap_not_granted",
            format!("{} needs {}; add it to the widget's manifest caps", b.verb, gr::CAP),
            Some(json!({ "cap": gr::CAP })),
        ));
    }
    let write = row.effect == Effect::Write;
    if write && crate::zen_routes::placement_pause(&w.garden, &w.placement).is_some() {
        return Err(no(423, "paused", "this widget is paused: too many writes. The person can Resume it on the widget", None));
    }
    let args = b.args.first().cloned().unwrap_or(J::Null);
    let field = |k: &str| args.get(k).and_then(J::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let app = field("app").ok_or_else(|| {
        no(400, "usage", format!("{}: in a Garden, name the App: {{app: <service id or workspace/name>, username}}", b.verb), None)
    })?;
    let t = crate::apps::grace::target(&crate::compute_routes::Who::Owner, Some(&app))
        .map_err(|_| no(404, "not_found", format!("no App '{app}' on this computer"), None))?;
    let maker = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        authors::maker_of(&conn, &w.widget, &w.hash).map_err(|e| crate::garden_data::response(&e))?
    };
    // app.rooms (AC3): what the App itself may read, no grant needed; an
    // agent-made widget only for its own workspace's Apps.
    if b.verb == "app.rooms" {
        if !k2_core::zen::garden_access::enabled() {
            match &maker {
                MadeBy::Owner | MadeBy::K2 => {}
                MadeBy::Agent { project_id } if t.project_id == *project_id => {}
                MadeBy::Agent { .. } => return Err(no(404, "not_found", format!("no App '{app}' this widget's maker can reach"), None)),
                MadeBy::Unclaimed => return Err(author_unknown(&w)),
            }
        }
        spend_read(&w)?;
        let view = gr::app_rooms(&t.service_id).map_err(|e| no(500, "failed", e, None))?;
        return serde_json::to_value(view).map_err(|e| no(500, "failed", e.to_string(), None));
    }
    let username = if b.verb == "guests.list" {
        String::new()
    } else {
        field("username").ok_or_else(|| no(400, "usage", format!("{} needs username", b.verb), None))?
    };
    let ceiling = match &maker {
        // Garden access (G15, §1.6): a gate-passing version (checked above)
        // acts as the owner: any App, any room, the vault confirm unchanged.
        _ if k2_core::zen::garden_access::enabled() => Ceiling::AnyRoom,
        MadeBy::Owner | MadeBy::K2 => Ceiling::AnyRoom,
        MadeBy::Agent { project_id } => {
            if t.project_id != *project_id {
                return Err(no(404, "not_found", format!("no App '{app}' this widget's maker can reach"), None));
            }
            match gr::app_ceiling(&t.service_id).map_err(|e| no(500, "failed", e, None))? {
                Some(rooms) => Ceiling::Rooms(rooms),
                None => {
                    return Err(no(
                        403,
                        "grant_required",
                        format!("an agent-made widget needs the owner's grant: k2 app grant {} --kind guests-rooms", t.name),
                        None,
                    ))
                }
            }
        }
        MadeBy::Unclaimed => return Err(author_unknown(&w)),
    };
    // guests.list (AC3): a read under the same ceiling rules as guests.rooms.
    // (The ceiling match above already refused an agent-made widget
    // without the owner's grant.)
    if b.verb == "guests.list" {
        spend_read(&w)?;
        let limit = match args.get("limit") {
            None | Some(J::Null) => None,
            Some(J::Number(n)) => match n.as_u64() {
                Some(n) => Some(n as usize),
                None => return Err(no(400, "usage", format!("limit is 1..={}", gr::LIST_MAX), None)),
            },
            Some(_) => return Err(no(400, "usage", format!("limit is 1..={}", gr::LIST_MAX), None)),
        };
        let cursor = field("cursor").unwrap_or_default();
        let page = gr::list(&t.service_id, &cursor, limit).map_err(core)?;
        return serde_json::to_value(page).map_err(|e| no(500, "failed", e.to_string(), None));
    }
    if !write {
        let view = gr::read(&t.service_id, &username).map_err(core)?;
        return serde_json::to_value(view).map_err(|e| no(500, "failed", e.to_string(), None));
    }
    let rooms: Vec<String> = match args.get("rooms") {
        Some(J::Array(a)) => a.iter().filter_map(J::as_str).map(str::to_string).collect(),
        _ => return Err(no(400, "usage", "guests.setRooms needs rooms: a list of room handles or ids ([] takes every room away)", None)),
    };
    // The confirm-less first call checks everything and counts against
    // the App's rate too, so a widget can't flood the person with sheets.
    if b.confirm_token.is_none() {
        gr::rate_take(&t.service_id).map_err(core)?;
    }
    let (plan, ids) = gr::plan(&t.service_id, &username, &rooms, &ceiling).map_err(core)?;
    if !plan.view.changed {
        return serde_json::to_value(plan.view).map_err(|e| no(500, "failed", e.to_string(), None));
    }
    let secret = secret_id(&w.garden, &w.placement, &t.service_id, &plan.view.username, &ids);
    let Some(token) = b.confirm_token.as_deref().filter(|t| !t.is_empty()) else {
        let change = json!({
            "app": t.service_id,
            "appName": t.name,
            "appHandle": format!("{}/{}", crate::compute_routes::ws_handle(&t.project_id), t.name),
            "username": plan.view.username,
            "role": plan.view.role,
            "before": plan.before,
            "beforeHandles": k2_core::skin::handles_for_project_ids(&plan.before),
            "after": ids,
            "afterHandles": plan.view.room_handles,
            "widget": w.widget,
            "madeBy": made_by_label(&maker),
        });
        return Err(no(
            403,
            "confirm_required",
            "changing a guest's rooms needs you: K2 shows the change on its own sheet",
            Some(json!({ "secretId": secret, "action": CONFIRM_ACTION, "change": change })),
        ));
    };
    take_confirm(token, &secret)?;
    let out = gr::set(&t.service_id, &username, &rooms, &ceiling, &actor(&w, &maker), "garden").map_err(core)?;
    serde_json::to_value(out.view).map_err(|e| no(500, "failed", e.to_string(), None))
}

fn author_unknown(w: &Placed) -> CliResponse {
    no(
        403,
        "author_unknown",
        format!("nobody signed this version of {0}. Run `k2 zen validate --widget {0}`, or click It's mine", w.widget),
        None,
    )
}

/// One read from this placement's budget (the reads AC3 adds).
fn spend_read(w: &Placed) -> Result<(), CliResponse> {
    let key = BudgetKey::Placement { garden: w.garden.clone(), placement: w.placement.clone() };
    budget::spend(&key, Spend::Read).map_err(|e| crate::garden_data::response(&e))
}

#[cfg(test)]
#[path = "guest_rooms_tests.rs"]
mod tests;
