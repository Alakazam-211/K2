//! RS-A15 `guests:rooms`, the owner's grant and the CLI door (0.46.1).
//! Like the grace routes, these serve every published service (a `--cmd`
//! BFF, a runtime App) and are not behind the runtime preview flag.
//!
//! ```text
//! GET  /cli/app/grants         ?app=                       the App's owner grants (kind guests-rooms)
//! POST /cli/app/grant          {app, kind, rooms?}         the owner: give (or re-give) the grant
//! POST /cli/app/grant/revoke   {app, kind}                 narrows: the owner, or an agent for its own App
//! GET  /cli/app/guest/rooms    ?app=&username=             one guest's rooms on that App
//! POST /cli/app/guest/rooms    {app, username, rooms}      set them
//! GET  /cli/app/guest/ceiling  ?app=                       the App's own room, grant and ceiling (app.rooms)
//! GET  /cli/app/guest/list     ?app=&cursor=&limit=        the App's guests with their rooms (guests.list)
//! ```
//!
//! Who may do what:
//! - **The grant** is the owner's (owner token, an Owner login, or a human
//!   in a plain K2 shell tab): `k2 app grant <app> --kind guests-rooms
//!   [--room <handle>…]`. Its rooms (plus the App's own room) are what the
//!   App may hand its guests. An agent gives its own workspace's App the
//!   grant in its own room with "Let this agent manage its Apps' users" on
//!   (`apps.manage-users`, 0.46.4);
//!   without it, or naming another workspace's room, the call is one held
//!   request with a card for the owner (`super::agent_grants`). Revoking
//!   narrows, so an agent may revoke its own workspace's App's grant.
//!   `POST /cli/app/grant/copy-host` lives here too (`agent_grants`).
//! - **Guest rooms**: the owner acts with owner rights (any room on the
//!   box). An agent acts only on its own workspace's Apps, with "Let this
//!   agent manage its Apps' users" on, only on guests wholly in its
//!   workspace (never a box login or an owner-made test guest), only while
//!   the grant stands, and only within it (`k2 app guest rooms <app> <user>
//!   [set …]`, end to end). Admin and Member logins: never.
//! - The rules themselves are `k2_core::skin::guest_rooms`.
//! - **Ceiling and list** (prd-app-caps-0464-v1 AC4, 0.46.4): reads. The
//!   owner reads any App; an agent reads its own workspace's App's ceiling
//!   (with or without the grant, as the App itself can) and lists its
//!   guests only while the owner's grant stands. Admin and Member logins:
//!   never (`owner_only`). Nothing is written.

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::deploy::codes;
use k2_core::skin::guest_rooms::{self as gr, Ceiling};
use serde_json::{json, Value as J};

use super::grace::{target, Target};
use super::routes::{actor, json_body, need_credential, no, s, ws_handle, Refusal, Who, R};
use crate::cli_response::CliResponse;

pub const ROUTES: &[&str] = &[
    "/cli/app/grants",
    "/cli/app/grant",
    "/cli/app/grant/revoke",
    "/cli/app/guest/rooms",
    GUEST_CEILING,
    GUEST_LIST,
    super::agent_grants::COPY_HOST,
];
pub const POST_ROUTES: &[&str] = &["/cli/app/grant", "/cli/app/grant/revoke", super::agent_grants::COPY_HOST];
/// `/cli/app/guest/rooms` takes GET (read) and POST (set).
pub const GUEST_ROOMS: &str = "/cli/app/guest/rooms";
/// `k2 app guest ceiling <app>` (GET only).
pub const GUEST_CEILING: &str = "/cli/app/guest/ceiling";
/// `k2 app guest list <app>` (GET only).
pub const GUEST_LIST: &str = "/cli/app/guest/list";

pub fn handle(path: &str, who: &Who, is_post: bool, body: &[u8], params: &HashMap<String, String>) -> CliResponse {
    let r = match (path, is_post) {
        ("/cli/app/grants", _) => list(who, params),
        ("/cli/app/grant", _) => give(who, body),
        ("/cli/app/grant/revoke", _) => take_back(who, body),
        (super::agent_grants::COPY_HOST, _) => super::agent_grants::copy_host(who, body),
        (GUEST_ROOMS, false) => guest_read(who, params),
        (GUEST_ROOMS, true) => guest_set(who, body),
        (GUEST_CEILING, false) => guest_ceiling(who, params),
        (GUEST_LIST, false) => guest_list(who, params),
        (GUEST_CEILING | GUEST_LIST, true) => Err(no("method_not_allowed", 405, format!("{path} is a GET"))),
        _ => Err(no("not_found", 404, "no such App route")),
    };
    // A held request (an agent's grant or copy-host for the owner) is
    // `202` (exit 2 in the CLI); everything else as before.
    super::routes::respond(r)
}

/// The owner in person, or a human in a plain K2 shell tab (NT5).
fn owner_rights(who: &Who) -> bool {
    matches!(who, Who::Owner | Who::Login { role: Role::Owner, .. } | Who::Shell { .. })
}

fn owner_only(what: &str) -> Refusal {
    no(codes::OWNER_ONLY, 403, format!("only the server's owner {what} (the owner token or an Owner login; never an agent)"))
}

fn kind_of(v: &J) -> R<()> {
    match s(v, "kind") {
        Some(gr::GRANT_KIND) => Ok(()),
        Some(k) => Err(no(codes::BAD_REQUEST, 400, format!("unknown grant kind '{k}' (kinds: {})", gr::GRANT_KIND))),
        None => Err(no(codes::BAD_REQUEST, 400, format!("name the kind (--kind {})", gr::GRANT_KIND))),
    }
}

fn store(e: String) -> Refusal {
    no("db_error", 500, e)
}

fn grant_json(t: &Target, g: &gr::AppOwnerGrant) -> J {
    let mut ceiling = g.rooms.clone();
    if !ceiling.contains(&t.project_id) {
        ceiling.push(t.project_id.clone());
    }
    json!({
        "kind": g.kind,
        "rooms": g.rooms,
        "roomHandles": k2_core::skin::handles_for_project_ids(&g.rooms),
        "handsOut": k2_core::skin::handles_for_project_ids(&ceiling),
        "grantedBy": g.granted_by,
        "grantedAt": g.granted_at,
    })
}

fn app_json(t: &Target) -> J {
    json!({
        "serviceId": t.service_id,
        "name": t.name,
        "handle": format!("{}/{}", ws_handle(&t.project_id), t.name),
        "appId": t.app_id,
    })
}

/// GET `/cli/app/grants?app=`.
fn list(who: &Who, params: &HashMap<String, String>) -> R<J> {
    need_credential(who)?;
    let t = target(who, params.get("app").map(String::as_str))?;
    let grants: Vec<J> = gr::get_grant(&t.service_id).map_err(store)?.iter().map(|g| grant_json(&t, g)).collect();
    Ok(json!({ "ok": true, "app": app_json(&t), "grants": grants }))
}

fn rooms_arg(v: &J) -> R<Vec<String>> {
    let tokens = crate::guest_rooms_routes::rooms_of(v).unwrap_or_default();
    let (ids, unknown) = gr::resolve_rooms(&tokens);
    if !unknown.is_empty() {
        return Err(no(codes::BAD_REQUEST, 400, format!("unknown room: {}", unknown.join(", "))));
    }
    Ok(ids)
}

/// POST `/cli/app/grant {app, kind, rooms?}`: the owner's; an agent's for
/// its own workspace's App at once in its own room with "Let this agent
/// manage its Apps' users" on, else one held request with a card
/// (`agent_grants`).
fn give(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    let v = json_body(body)?;
    kind_of(&v)?;
    let agent = super::routes::agent_ws(who).map(str::to_string);
    if agent.is_none() && !owner_rights(who) {
        return Err(owner_only("grants an App guests:rooms (k2 app grant <app> --kind guests-rooms)"));
    }
    let t = target(who, s(&v, "app"))?;
    let rooms = rooms_arg(&v)?;
    if let Some(ws) = agent.as_deref() {
        // Own room + "manage its Apps' users": at once. Otherwise one held request and a
        // card for the owner (`agent_grants`).
        if let Some(held) = super::agent_grants::guest_rooms_agent(who, ws, &t, &rooms, s(&v, "reason"))? {
            return Ok(held);
        }
    }
    let (before, after) = gr::grant(&t.service_id, &rooms, &actor(who)).map_err(store)?;
    Ok(json!({
        "ok": true,
        "app": app_json(&t),
        "grant": grant_json(&t, &after),
        "replaced": before.map(|b| k2_core::skin::handles_for_project_ids(&b.rooms)),
    }))
}

/// POST `/cli/app/grant/revoke {app, kind}`: narrows.
fn take_back(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    let v = json_body(body)?;
    kind_of(&v)?;
    if matches!(who, Who::Login { role, .. } if *role != Role::Owner) {
        return Err(owner_only("revokes an App's grant over a login"));
    }
    let t = target(who, s(&v, "app"))?;
    let was = gr::revoke(&t.service_id, &actor(who)).map_err(store)?;
    Ok(json!({ "ok": true, "app": app_json(&t), "revoked": was.is_some() }))
}

/// The ceiling the caller acts with on `t`'s guests.
fn ceiling_for(who: &Who, t: &Target) -> R<Ceiling> {
    if owner_rights(who) {
        return Ok(Ceiling::AnyRoom);
    }
    match who {
        // `target` already kept a passport to its own workspace's Apps.
        Who::Agent { .. } => match gr::app_ceiling(&t.service_id).map_err(store)? {
            Some(rooms) => Ok(Ceiling::Rooms(rooms)),
            None => Err(no(
                "grant_required",
                403,
                format!(
                    "{} has no guests:rooms grant: run k2 app grant {} --kind guests-rooms (with \"Let this agent manage its Apps' users\" \
                     on it applies at once in your own room; otherwise the owner gets a card on your Thread)",
                    t.name, t.name
                ),
            )),
        },
        _ => Err(owner_only("sets guests' rooms over a login")),
    }
}

fn core_refusal(e: gr::Refusal) -> Refusal {
    let extra = match &e {
        gr::Refusal::RateLimited { retry_after_ms } => Some(json!({ "retryAfterMs": retry_after_ms })),
        gr::Refusal::OutsideApp(r) | gr::Refusal::AboveRole(r) => Some(json!({ "rooms": r })),
        _ => None,
    };
    Refusal { code: e.code(), status: e.status(), hint: e.hint(), extra }
}

/// Review B4: the owner's calls don't spend the App's 60/min budget; an
/// agent's do (it acts for its own App).
fn owner_budget<T>(who: &Who, f: impl FnOnce() -> T) -> T {
    if owner_rights(who) {
        gr::with_owner_budget(f)
    } else {
        f()
    }
}

/// GET `/cli/app/guest/rooms?app=&username=`.
fn guest_read(who: &Who, params: &HashMap<String, String>) -> R<J> {
    need_credential(who)?;
    let t = target(who, params.get("app").map(String::as_str))?;
    ceiling_for(who, &t)?;
    let user = params.get("username").map(String::as_str).filter(|u| !u.trim().is_empty());
    let user = user.ok_or_else(|| no(codes::BAD_REQUEST, 400, "name the guest (username)"))?;
    let view = owner_budget(who, || gr::read(&t.service_id, user)).map_err(core_refusal)?;
    serde_json::to_value(view).map_err(|e| store(e.to_string()))
}

/// GET `/cli/app/guest/ceiling?app=`: what `app.rooms` answers that App.
fn guest_ceiling(who: &Who, params: &HashMap<String, String>) -> R<J> {
    need_credential(who)?;
    let t = target(who, params.get("app").map(String::as_str))?;
    if !owner_rights(who) && !matches!(who, Who::Agent { .. }) {
        return Err(owner_only("reads an App's guest ceiling over a login"));
    }
    let v = gr::app_rooms(&t.service_id).map_err(store)?;
    let mut j = serde_json::to_value(v).map_err(|e| store(e.to_string()))?;
    j["appInfo"] = app_json(&t);
    Ok(j)
}

/// GET `/cli/app/guest/list?app=&cursor=&limit=`: what `guests.list`
/// answers that App (an agent: only under the owner's grant).
fn guest_list(who: &Who, params: &HashMap<String, String>) -> R<J> {
    need_credential(who)?;
    let t = target(who, params.get("app").map(String::as_str))?;
    ceiling_for(who, &t)?;
    let limit = match params.get("limit").map(|s| s.trim()).filter(|s| !s.is_empty()) {
        None => None,
        Some(n) => Some(n.parse::<usize>().map_err(|_| no(codes::BAD_REQUEST, 400, format!("limit is a number, 1..={}", gr::LIST_MAX)))?),
    };
    let cursor = params.get("cursor").map(String::as_str).unwrap_or("");
    let page = owner_budget(who, || gr::list(&t.service_id, cursor, limit)).map_err(core_refusal)?;
    serde_json::to_value(page).map_err(|e| store(e.to_string()))
}

/// POST `/cli/app/guest/rooms {app, username, rooms}`.
fn guest_set(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    let v = json_body(body)?;
    let t = target(who, s(&v, "app"))?;
    let user = s(&v, "username").ok_or_else(|| no(codes::BAD_REQUEST, 400, "name the guest (username)"))?;
    // An agent changes its own App's guests' rooms only with "Let this
    // agent manage its Apps' users", and only on a guest the roster door
    // would let it touch: wholly its workspace's, never a box login, never
    // an owner-made test guest (review 0.46.4: this path skipped them).
    if let Some(ws) = super::routes::agent_ws(who) {
        if !super::agent_grants::may_self(who) {
            return Err(super::agent_grants::users_off("Setting your App guests' rooms"));
        }
        let probe = json!({ "username": user }).to_string();
        crate::skin_routes::agent_may_touch(ws, GUEST_ROOMS, probe.as_bytes()).map_err(|r| super::agent_grants::refusal_of(&r))?;
        if let Some(r) = crate::skin_routes::agent_test_guest_refused(&actor(who), user) {
            return Err(super::agent_grants::refusal_of(&r));
        }
    }
    let ceiling = ceiling_for(who, &t)?;
    let rooms = crate::guest_rooms_routes::rooms_of(&v)
        .ok_or_else(|| no(codes::BAD_REQUEST, 400, "rooms is required: handles or ids ([] takes every room away)"))?;
    let out = owner_budget(who, || gr::set(&t.service_id, user, &rooms, &ceiling, &actor(who), "cli")).map_err(core_refusal)?;
    let mut j = serde_json::to_value(&out.view).map_err(|e| store(e.to_string()))?;
    j["before"] = json!(out.before);
    j["beforeHandles"] = json!(k2_core::skin::handles_for_project_ids(&out.before));
    Ok(j)
}

#[cfg(test)]
#[path = "guest_rooms_tests.rs"]
mod tests;
