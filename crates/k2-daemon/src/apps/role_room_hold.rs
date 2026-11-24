//! `k2 app role room <role> <room>` by an App's own agent (`POST
//! /cli/skin/roles/room`): one room line on a role.
//!
//! | the role | "Let this agent manage its Apps' users" on | off |
//! |---|---|---|
//! | of one of its own Apps, the owner's too | the agent, at once, audited | one held request + card for the owner ([`hold_for_owner`]): Allow applies exactly that line, "Always allow" turns the switch on |
//! | box-wide (no App) | one held request + an **Owner-only** card ([`hold_box_wide`], 0.46.18): the hint first says to make an App role instead; Allow applies exactly that line; never an Admin, never "Always allow" | the same |
//! | another workspace's App | refused, and the hint says what to do instead | the same |
//!
//! The checks themselves are `skin_routes::agent_scoped` (its own room, roles
//! it wrote, person-only caps). This only turns its refusal for a role of the
//! agent's own App into the owner's card, and gives the other refusals a next
//! step. The approval runs [`apply`] as the owner who approved it, with the
//! values fixed at request time: that role (still that App's, still the
//! workspace's), that room, those caps, or that line cleared.

use k2_core::app_grace::requests as reqs;
use k2_core::deploy::{codes, store};
use k2_core::skin;
use serde_json::{json, Value as J};

use super::agent_grants::{hold, Step, SWITCH_NAME};
use super::routes::{no, respond, ws_handle, Who, R};
use crate::cli_response::CliResponse;
use k2_core::brand::{CLI_NAME, PRODUCT_NAME};

/// The route this answers for.
pub const PATH: &str = "/cli/skin/roles/room";

/// The route a held role-room request carries (never an HTTP route: the
/// approval runs [`apply`], as the owner who approved).
pub const APPLY_ROUTE: &str = "/cli/app/role/room/apply";

/// The route a held room line on a **box-wide** role carries (0.46.18):
/// the approval runs [`apply_box_wide`]. Only the owner approves it (the
/// owner token or an Owner login, never an Admin): every login holding
/// the role gets the change, box logins too.
pub const BOX_APPLY_ROUTE: &str = "/cli/app/role/room/box/apply";

/// Is `r` a held request only the owner approves (an Owner floor)?
pub(crate) fn owner_only(r: &reqs::AppRequest) -> bool {
    r.body.get("route").and_then(J::as_str) == Some(BOX_APPLY_ROUTE)
}

/// `POST /cli/skin/roles/room` from the dispatcher: `agent_ws` and `session`
/// are the agent's (from its passport), `None` for the owner and logins.
pub fn scoped(agent_ws: Option<&str>, session: Option<&str>, body: &[u8], actor: &str) -> CliResponse {
    let r = crate::skin_routes::agent_scoped(agent_ws, PATH, body, actor, crate::skin_routes::handle_roles_room);
    match agent_ws {
        Some(ws) if refused(&r) => after_refusal(ws, session.unwrap_or_default(), body, r),
        _ => r,
    }
}

fn error_code(r: &CliResponse) -> Option<String> {
    let v: J = serde_json::from_str(&r.body).ok()?;
    v.pointer("/error/code").and_then(J::as_str).map(str::to_string)
}

/// A refusal of the agent itself (nothing was written): `owner_only`, or the
/// App-users switch being off.
fn refused(r: &CliResponse) -> bool {
    r.status.starts_with("403") && error_code(r).is_some_and(|c| c == "owner_only" || c == crate::skin_routes::APP_USERS_OFF)
}

/// The first non-empty text of `keys`, as the handler reads its body.
fn text<'a>(v: &'a J, keys: &[&str]) -> Option<&'a str> {
    keys.iter().filter_map(|k| v.get(*k).and_then(J::as_str)).map(str::trim).find(|s| !s.is_empty())
}

/// The service `id`, when it is one of workspace `ws`'s.
fn own_service(id: &str, ws: &str) -> Option<k2_core::published_services::PublishedService> {
    let h = k2_core::db::shared();
    let conn = h.lock();
    k2_core::published_services::get_by_id(&conn, id).ok().flatten().filter(|s| s.project_id == ws)
}

/// One room line, checked: the role, the App it belongs to, the room's id,
/// and the caps it gets (`None`: the line is cleared).
struct Ask {
    role: skin::SkinRole,
    svc: k2_core::published_services::PublishedService,
    room: String,
    caps: Option<Vec<String>>,
}

/// The room and caps a refused room line asked for (`None` caps: clear
/// the line); `Err` is the answer to give instead.
fn line_of(v: &J) -> Result<(String, Option<Vec<String>>), Option<CliResponse>> {
    let Some(Ok(room)) = text(v, &["handle", "id"]).map(|t| skin::resolve_room_tokens(&[t.to_string()])) else { return Err(None) };
    let [room] = room.as_slice() else { return Err(None) };
    if v.get("clear").and_then(J::as_bool) == Some(true) {
        return Ok((room.clone(), None));
    }
    let sent: Option<Vec<String>> =
        v.get("caps").or_else(|| v.get("capabilities")).and_then(J::as_array).map(|a| a.iter().filter_map(J::as_str).map(str::to_string).collect());
    match skin::parse_caps(sent.as_deref()) {
        Ok(c) => Ok((room.clone(), Some(c))),
        Err(e) => Err(Some(CliResponse::bad_request(e))),
    }
}

fn after_refusal(ws: &str, session: &str, body: &[u8], refusal: CliResponse) -> CliResponse {
    let Ok(v) = serde_json::from_slice::<J>(body) else { return refusal };
    let Some(Ok(Some(role))) = text(&v, &["id", "name", "role"]).map(skin::find_role) else { return refusal };
    let who = Who::Agent { project_id: ws.to_string(), session_id: session.to_string() };
    let reason = v.get("reason").and_then(J::as_str);
    // A box-wide role: one Owner-only card (0.46.18), switch or not.
    if role.app_id.is_none() {
        let (room, caps) = match line_of(&v) {
            Ok(x) => x,
            Err(answer) => return answer.unwrap_or(refusal),
        };
        return respond(hold_box_wide(&who, ws, &role, &room, caps, reason));
    }
    let Some(svc) = role.app_id.as_deref().and_then(|app| own_service(app, ws)) else { return not_cardable(&role, &v, refusal) };
    // With the switch on, its own App's role was the agent's to change: a
    // refusal here is about something else, and stays as it is.
    if k2_core::workspace::settings::agents_can_manage_app_users(ws) {
        return refusal;
    }
    let (room, caps) = match line_of(&v) {
        Ok(x) => x,
        Err(answer) => return answer.unwrap_or(refusal),
    };
    let ask = Ask { role, svc, room, caps };
    respond(hold_for_owner(&who, ws, &ask, reason))
}

/// Another workspace's App's role: refused as before, with the next step at
/// the end (no card can give it: it isn't the agent's App, nor the box's).
fn not_cardable(role: &skin::SkinRole, v: &J, refusal: CliResponse) -> CliResponse {
    if !refusal.body.contains("outside your workspace") {
        return refusal;
    }
    let room = text(v, &["handle", "id"]).unwrap_or("<room>");
    let what = format!("role '{}' belongs to another workspace's App: only that App's agent or the owner changes it", role.name);
    let hint = format!(
        "{what}: it reaches outside your workspace. Instead, make a role of your own App with that room ({CLI_NAME} app role create <name> --app <your app>, then {CLI_NAME} app role \
         room <name> {room} --caps …), or ask your human to run {CLI_NAME} app role room {} {room} from a terminal outside {PRODUCT_NAME}",
        role.name
    );
    super::routes::refusal_response(&no(codes::OWNER_ONLY, 403, hint))
}

/// How many logins hold `role` now (a grant with it, or it as their own role).
fn holders(role: &skin::SkinRole) -> usize {
    let grants = skin::list_grants().unwrap_or_default();
    let mut who: std::collections::BTreeSet<String> =
        grants.iter().filter(|g| g.role_id.as_deref() == Some(role.id.as_str())).map(|g| format!("{}:{}", g.subject_kind, g.subject_id)).collect();
    if let Ok(users) = skin::list_principals() {
        who.extend(users.iter().filter(|u| u.role_id.as_deref() == Some(role.id.as_str())).map(|u| format!("principal:{}", u.id)));
    }
    who.len()
}

/// One room line on a **box-wide** role, asked by an agent (0.46.18,
/// Rosson: an Owner-only card, never a "run as me" path). The answer's
/// hint names the App role first: that is the agent's own to make.
fn hold_box_wide(who: &Who, ws: &str, role: &skin::SkinRole, room: &str, caps: Option<Vec<String>>, reason: Option<&str>) -> R<J> {
    let room_h = ws_handle(room);
    let whose = if room == ws { "your own room".to_string() } else { format!("the {room_h} workspace's room") };
    let now = role.room_policy.get(room);
    let n = holders(role);
    let reach = format!(
        "every login holding the box-wide role {} gets this, box logins too ({n} hold it now)",
        role.name
    );
    let (summary, change) = match (&caps, now) {
        (None, _) => (format!("box-wide role {}: take the room {room_h} off it", role.name), format!("take {room_h} ({whose}) off the box-wide role {}", role.name)),
        (Some(c), None) => (
            format!("box-wide role {}: add the room {room_h} with {} ({reach})", role.name, caps_words(c)),
            format!("add {room_h} ({whose}) to the box-wide role {} with {}", role.name, caps_words(c)),
        ),
        (Some(c), Some(old)) => (
            format!("box-wide role {}: the room {room_h} with {} (now {}; {reach})", role.name, caps_words(c), caps_words(old)),
            format!("change what the box-wide role {} may do in {room_h} ({whose}) from {} to {}", role.name, caps_words(old), caps_words(c)),
        ),
    };
    let instead = format!(
        "or, instead, make an App role: the agent makes a role of its own App with that room itself ({CLI_NAME} app role create <name> --app <its app>, \
         then {CLI_NAME} app role room <name> {room_h} …), and the box-wide role {} stays as it is",
        role.name,
    );
    let mut out = hold(
        Step {
            who,
            app: None,
            label: &format!("box-wide role {}", role.name),
            workspace: ws,
            route: BOX_APPLY_ROUTE,
            body: json!({
                "workspace": ws,
                "role": role.id,
                "roleName": role.name,
                "room": room,
                "roomHandle": room_h,
                "caps": caps,
                "clear": caps.is_none(),
            }),
            lines: vec![
                change,
                format!(
                    "{reach}; nothing else on the role changes. Undo: {} app role room {} {room_h} {}",
                    k2_core::brand::CLI_NAME,
                    role.name,
                    match now {
                        Some(old) => format!("--caps {}", old.join(",")),
                        None => "--clear".to_string(),
                    }
                ),
                instead,
            ],
            summary,
            why: "a box-wide role is the owner's: only the owner approves it (never an admin)".to_string(),
            reason,
        },
        false,
    )?;
    // The App role first: the agent can do that now, without the owner.
    let held = out.get("hint").and_then(J::as_str).unwrap_or_default().to_string();
    out["hint"] = json!(format!(
        "role '{}' is box-wide (no App): every login holding it gets its rooms, box logins too. Better: make a role of your own App with that room \
         ({CLI_NAME} app role create <name> --app <your app>, then {CLI_NAME} app role room <name> {room_h} --caps …). If it must be this role: {held}",
        role.name,
    ));
    Ok(out)
}

/// The caps in words.
fn caps_words(caps: &[String]) -> String {
    caps.join(", ")
}

fn hold_for_owner(who: &Who, ws: &str, a: &Ask, reason: Option<&str>) -> R<J> {
    let app = format!("{}/{}", ws_handle(ws), a.svc.name);
    let room = ws_handle(&a.room);
    let whose = if a.room == ws { "your own room".to_string() } else { format!("the {room} workspace's room") };
    let now = a.role.room_policy.get(&a.room);
    let (summary, change) = match (&a.caps, now) {
        (None, _) => (format!("role {} of {app}: take the room {room} off it", a.role.name), format!("take {room} ({whose}) off the role {}", a.role.name)),
        (Some(c), None) => (
            format!("role {} of {app}: add the room {room} with {}", a.role.name, caps_words(c)),
            format!("add {room} ({whose}) to the role {} with {}", a.role.name, caps_words(c)),
        ),
        (Some(c), Some(old)) => (
            format!("role {} of {app}: the room {room} with {} (now {})", a.role.name, caps_words(c), caps_words(old)),
            format!("change what the role {} may do in {room} ({whose}) from {} to {}", a.role.name, caps_words(old), caps_words(c)),
        ),
    };
    let row = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        store::get_app_by_service(&conn, &a.svc.id).ok().flatten()
    };
    hold(
        Step {
            who,
            app: row.as_ref(),
            label: &a.svc.name,
            workspace: ws,
            route: APPLY_ROUTE,
            body: json!({
                "workspace": ws,
                "service": a.svc.id,
                "app": app,
                "role": a.role.id,
                "roleName": a.role.name,
                "room": a.room,
                "roomHandle": room,
                "caps": a.caps,
                "clear": a.caps.is_none(),
            }),
            lines: vec![
                change,
                format!(
                    "everyone with the role {} on {app} gets this at their next request; nothing else on the role changes. Undo: {CLI_NAME} app role room {} {room} {}",
                    a.role.name,
                    a.role.name,
                    match now {
                        Some(old) => format!("--caps {}", old.join(",")),
                        None => "--clear".to_string(),
                    }
                ),
            ],
            summary,
            why: format!("\"{SWITCH_NAME}\" is off for this agent"),
            reason,
        },
        true,
    )
}

/// Carry out an approved room line on a box-wide role as the owner who
/// approved it: exactly the line it showed, while the role is still
/// box-wide (made an App's since, it asks again).
pub(crate) fn apply_box_wide(r: &reqs::AppRequest, by: &str) -> R<J> {
    let b = r.body.get("body").cloned().unwrap_or(J::Null);
    let ws = b["workspace"].as_str().unwrap_or_default();
    if ws.is_empty() || r.requester_workspace.as_deref() != Some(ws) {
        return Err(no(codes::BAD_REQUEST, 400, "unreadable held request"));
    }
    let (Some(role_id), Some(room)) = (b["role"].as_str(), b["room"].as_str()) else {
        return Err(no(codes::BAD_REQUEST, 400, "unreadable held request"));
    };
    let role = match skin::find_role(role_id) {
        Ok(Some(role)) if role.id == role_id => role,
        Ok(_) => return Err(no(codes::FORBIDDEN, 409, "the role in this request is gone: ask again")),
        Err(e) => return Err(no("db_error", 500, e)),
    };
    if role.app_id.is_some() {
        return Err(no(codes::FORBIDDEN, 409, format!("the role {} isn't box-wide any more: ask again", role.name)));
    }
    let mut line = json!({ "id": role.id, "handle": room });
    if b["clear"].as_bool() == Some(true) {
        line["clear"] = json!(true);
    } else {
        line["caps"] = b["caps"].clone();
    }
    let resp = crate::skin_routes::agent_scoped(None, PATH, line.to_string().as_bytes(), by, crate::skin_routes::handle_roles_room);
    let v: J = serde_json::from_str(&resp.body).unwrap_or(J::Null);
    if !resp.status.starts_with('2') {
        let hint = v.pointer("/error/hint").and_then(J::as_str).or_else(|| v.get("error").and_then(J::as_str)).unwrap_or("the change failed");
        return Err(no("role_room_failed", 409, hint.to_string()));
    }
    Ok(json!({ "applied": BOX_APPLY_ROUTE, "role": role.name, "room": ws_handle(room), "answer": v }))
}

/// Carry out an approved role-room request as the owner who approved it
/// (`approvals::apply_door`): exactly the line it showed, on the role it
/// named, while that role is still the requesting workspace's App's.
pub(crate) fn apply(r: &reqs::AppRequest, by: &str) -> R<J> {
    let b = r.body.get("body").cloned().unwrap_or(J::Null);
    let ws = b["workspace"].as_str().unwrap_or_default();
    if ws.is_empty() || r.requester_workspace.as_deref() != Some(ws) {
        return Err(no(codes::BAD_REQUEST, 400, "unreadable held request"));
    }
    let (Some(role_id), Some(svc), Some(room)) = (b["role"].as_str(), b["service"].as_str(), b["room"].as_str()) else {
        return Err(no(codes::BAD_REQUEST, 400, "unreadable held request"));
    };
    let role = match skin::find_role(role_id) {
        Ok(Some(role)) if role.id == role_id => role,
        Ok(_) => return Err(no(codes::FORBIDDEN, 409, "the role in this request is gone: ask again")),
        Err(e) => return Err(no("db_error", 500, e)),
    };
    if role.app_id.as_deref() != Some(svc) || own_service(svc, ws).is_none() {
        return Err(no(codes::FORBIDDEN, 409, format!("the role {} isn't the requesting workspace's App's any more: ask again", role.name)));
    }
    let mut line = json!({ "id": role.id, "handle": room });
    if b["clear"].as_bool() == Some(true) {
        line["clear"] = json!(true);
    } else {
        line["caps"] = b["caps"].clone();
    }
    // The owner's write: audited as the approver, and the role is the owner's again.
    let resp = crate::skin_routes::agent_scoped(None, PATH, line.to_string().as_bytes(), by, crate::skin_routes::handle_roles_room);
    let v: J = serde_json::from_str(&resp.body).unwrap_or(J::Null);
    if !resp.status.starts_with('2') {
        let hint = v.pointer("/error/hint").and_then(J::as_str).or_else(|| v.get("error").and_then(J::as_str)).unwrap_or("the change failed");
        return Err(no("role_room_failed", 409, hint.to_string()));
    }
    Ok(json!({ "applied": APPLY_ROUTE, "role": role.name, "room": ws_handle(room), "answer": v }))
}
