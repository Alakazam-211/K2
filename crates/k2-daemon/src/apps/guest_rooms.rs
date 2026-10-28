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
//! GET  /cli/app/guest/chat     ?username=[&app=]           one person's chat (Sew for Apps A2)
//! POST /cli/app/guest/chat     {username, scope, app?}     set it: member | guest | none
//! ```
//!
//! **Chat (Sew for Apps A2, 0.46.6).** `POST /cli/app/grant {app, kind:
//! "guests-chat", maxMembers?}` is the owner's only (an agent's is
//! refused, never held: chat is box-wide). `k2 app user chat` is the owner
//! for any person (with `app`, that App's guest, audited on the App), or an
//! agent for its own workspace's App with "Let this agent manage its Apps'
//! users" on, under that App's grant and rules, as the App itself
//! (`k2_core::skin::app_chat`).
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
//!   agent manage its Apps' users" on, only on this App's guests (never a
//!   box login or an owner-made test guest), only while the grant stands,
//!   and only within it (`k2 app guest rooms <app> <user>
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
use k2_core::skin::app_chat as ac;
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
    GUEST_CHAT,
    super::agent_grants::COPY_HOST,
];
pub const POST_ROUTES: &[&str] = &["/cli/app/grant", "/cli/app/grant/revoke", super::agent_grants::COPY_HOST];
/// `/cli/app/guest/rooms` takes GET (read) and POST (set).
pub const GUEST_ROOMS: &str = "/cli/app/guest/rooms";
/// `k2 app guest ceiling <app>` (GET only).
pub const GUEST_CEILING: &str = "/cli/app/guest/ceiling";
/// `k2 app guest list <app>` (GET only).
pub const GUEST_LIST: &str = "/cli/app/guest/list";
/// `k2 app user chat <user> [member|guest|none] [--app <app>]` (Sew for
/// Apps A2): GET reads, POST sets.
pub const GUEST_CHAT: &str = "/cli/app/guest/chat";

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
        (GUEST_CHAT, false) => chat_read(who, params),
        (GUEST_CHAT, true) => chat_set(who, body),
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

/// The two owner grants an App can hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Rooms,
    Chat,
}

fn kind_of(v: &J) -> R<Kind> {
    match s(v, "kind") {
        Some(gr::GRANT_KIND) => Ok(Kind::Rooms),
        Some(ac::GRANT_KIND) => Ok(Kind::Chat),
        Some(k) => Err(no(
            codes::BAD_REQUEST,
            400,
            format!("unknown grant kind '{k}' (kinds: {}, {})", gr::GRANT_KIND, ac::GRANT_KIND),
        )),
        None => Err(no(codes::BAD_REQUEST, 400, format!("name the kind (--kind {} or {})", gr::GRANT_KIND, ac::GRANT_KIND))),
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
    let mut grants: Vec<J> = gr::get_grant(&t.service_id).map_err(store)?.iter().map(|g| grant_json(&t, g)).collect();
    if let Some(g) = ac::get_grant(&t.service_id).map_err(store)? {
        grants.push(chat_grant_json(&g));
    }
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
    if kind_of(&v)? == Kind::Chat {
        return give_chat(who, &v);
    }
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
    let kind = kind_of(&v)?;
    if matches!(who, Who::Login { role, .. } if *role != Role::Owner) {
        return Err(owner_only("revokes an App's grant over a login"));
    }
    let t = target(who, s(&v, "app"))?;
    let was = match kind {
        Kind::Rooms => gr::revoke(&t.service_id, &actor(who)).map_err(store)?.is_some(),
        Kind::Chat => ac::revoke(&t.service_id, &actor(who)).map_err(store)?.is_some(),
    };
    Ok(json!({ "ok": true, "app": app_json(&t), "revoked": was }))
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
    // agent manage its Apps' users", never on a box login and never on an
    // owner-made test guest (review 0.46.4: this path skipped them).
    if let Some(ws) = super::routes::agent_ws(who) {
        if !super::agent_grants::may_self(who) {
            return Err(super::agent_grants::users_off("Setting your App guests' rooms"));
        }
        // The rules (`gr::set`) already keep it to THIS App's guests and the
        // owner's grant (which may name other rooms on purpose); on top,
        // never a box login and never an owner-made test guest.
        let _ = ws;
        let grants = k2_core::skin::list_grants().map_err(store)?;
        if let Some(p) = k2_core::skin::list_principals().map_err(store)?.into_iter().find(|p| p.username == user.trim().to_ascii_lowercase()) {
            if crate::skin_routes::has_box_login(&p.id, &grants) {
                return Err(super::agent_grants::refusal_of(&crate::skin_routes::box_login_refused(&p.username)));
            }
        }
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

// ── chat (Sew for Apps A2) ───────────────────────────────────────────

fn chat_grant_json(g: &ac::ChatGrant) -> J {
    json!({
        "kind": g.kind,
        "maxMembers": g.max_members,
        "grantedBy": g.granted_by,
        "grantedAt": g.granted_at,
    })
}

/// `POST /cli/app/grant {app, kind: guests-chat, maxMembers?}`: the
/// owner's only. A chat grant is box-wide (one Sew account per person), so
/// an agent never gives its App this (no held card either: the owner runs
/// `k2 app grant <app> --kind guests-chat`).
fn give_chat(who: &Who, v: &J) -> R<J> {
    if !owner_rights(who) {
        return Err(owner_only("grants an App guests:chat (k2 app grant <app> --kind guests-chat [--max N])"));
    }
    let t = target(who, s(v, "app"))?;
    let max = match v.get("maxMembers") {
        None | Some(J::Null) => None,
        Some(n) => Some(
            n.as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| no(codes::BAD_REQUEST, 400, format!("maxMembers is a number, 1..={}", ac::MAX_MAX_MEMBERS)))?,
        ),
    };
    let (before, after) = ac::grant(&t.service_id, max, &actor(who)).map_err(|e| no(codes::BAD_REQUEST, 400, e))?;
    // Sew 2026-10-28 (rule 3): until Sew scopes sessions to an App, the App
    // can't use the grant, and chat the owner gives its people lets the App
    // see all their chat.
    let scoped = ac::app_scoped().map_err(store)?;
    Ok(json!({
        "ok": true,
        "app": app_json(&t),
        "grant": chat_grant_json(&after),
        "replaced": before.map(|b| b.max_members),
        "appScoped": scoped,
        "warning": (!scoped).then_some(ac::WARN_UNSCOPED),
    }))
}

fn chat_refusal(e: ac::Refusal) -> Refusal {
    let extra = match &e {
        ac::Refusal::RateLimited { retry_after_ms } => Some(json!({ "retryAfterMs": retry_after_ms })),
        ac::Refusal::Limit { max } => Some(json!({ "maxMembers": max })),
        _ => None,
    };
    Refusal { code: e.code(), status: e.status(), hint: e.hint(), extra }
}

/// Who the caller is for `k2 app user chat`, and the App it acts in.
/// `write` = a set: an agent then needs "manage its Apps' users" and the
/// App's guests-chat grant. A read (its own App's users) needs neither.
fn chat_caller(who: &Who, app: Option<&str>, write: bool) -> R<(ac::Caller, Option<Target>)> {
    if owner_rights(who) {
        let t = match app {
            Some(a) => Some(target(who, Some(a))?),
            None => None,
        };
        return Ok((ac::Caller::Owner, t));
    }
    match who {
        Who::Agent { .. } => {
            let Some(a) = app else {
                return Err(no(codes::BAD_REQUEST, 400, "an agent names its own App (--app <app>)"));
            };
            // `target` keeps a passport to its own workspace's Apps.
            let t = target(who, Some(a))?;
            if !write {
                return Ok((ac::Caller::App, Some(t)));
            }
            if !super::agent_grants::may_self(who) {
                return Err(super::agent_grants::users_off("Giving your App's guests chat"));
            }
            if ac::get_grant(&t.service_id).map_err(store)?.is_none() {
                return Err(no(
                    "grant_required",
                    403,
                    format!("{} has no guests:chat grant: the owner runs k2 app grant {} --kind guests-chat", t.name, t.name),
                ));
            }
            Ok((ac::Caller::App, Some(t)))
        }
        _ => Err(owner_only("changes a person's chat over a login")),
    }
}

/// GET `/cli/app/guest/chat?username=[&app=]`.
fn chat_read(who: &Who, params: &HashMap<String, String>) -> R<J> {
    need_credential(who)?;
    let app = params.get("app").map(|a| a.trim()).filter(|a| !a.is_empty());
    let (caller, t) = chat_caller(who, app, false)?;
    let user = params.get("username").map(String::as_str).filter(|u| !u.trim().is_empty());
    let user = user.ok_or_else(|| no(codes::BAD_REQUEST, 400, "name the person (username)"))?;
    let svc = t.as_ref().map(|t| t.service_id.as_str());
    let view = match (caller, svc) {
        (ac::Caller::App, Some(svc)) => ac::read(svc, user),
        _ => ac::owner_read(svc, user),
    }
    .map_err(chat_refusal)?;
    serde_json::to_value(view).map_err(|e| store(e.to_string()))
}

/// POST `/cli/app/guest/chat {username, scope, app?}`.
fn chat_set(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    let v = json_body(body)?;
    let (caller, t) = chat_caller(who, s(&v, "app"), true)?;
    let user = s(&v, "username").ok_or_else(|| no(codes::BAD_REQUEST, 400, "name the person (username)"))?;
    let scope = s(&v, "scope")
        .and_then(ac::Scope::parse)
        .ok_or_else(|| no(codes::BAD_REQUEST, 400, "scope is member, guest or none"))?;
    if caller == ac::Caller::App {
        // As for guest rooms: never a box login, never an owner-made test guest.
        let grants = k2_core::skin::list_grants().map_err(store)?;
        let want = user.trim().to_ascii_lowercase();
        if let Some(p) = k2_core::skin::list_principals().map_err(store)?.into_iter().find(|p| p.username == want) {
            if crate::skin_routes::has_box_login(&p.id, &grants) {
                return Err(super::agent_grants::refusal_of(&crate::skin_routes::box_login_refused(&p.username)));
            }
        }
        if let Some(r) = crate::skin_routes::agent_test_guest_refused(&actor(who), user) {
            return Err(super::agent_grants::refusal_of(&r));
        }
    }
    let svc = t.as_ref().map(|t| t.service_id.clone());
    let view = ac::set(&caller, svc.as_deref(), user, scope, &actor(who), "cli").map_err(chat_refusal)?;
    serde_json::to_value(view).map_err(|e| store(e.to_string()))
}

#[cfg(test)]
#[path = "guest_rooms_tests.rs"]
mod tests;

