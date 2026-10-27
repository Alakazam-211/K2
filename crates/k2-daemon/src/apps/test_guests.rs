//! Test guests for an App (0.46.4, NSI B: "test guests need the owner
//! each time"): short-lived guests scoped to one App, to test it with.
//!
//! ```text
//! POST /cli/app/test-guest/add  {app, username, role?, expires?, password?}
//! POST /cli/app/test-guest/rm   {username}
//! GET  /cli/app/test-guests     ?app=
//! ```
//!
//! **Who:** a person (the owner token, an Owner login, or a human in a K2
//! shell tab; route floor Owner, like `/cli/skin/users`), or the App's
//! own agent while "Let this agent manage its Apps' users" is on for its
//! workspace (`apps.manage-users`, 0225, Rosson 2026-10-26;
//! `super::agent_grants`). An agent without
//! it files one held request the owner approves from a card on its
//! Thread (the replay runs every rule below again; the password is never
//! kept with the request). Another workspace's agent never sees the App
//! (`app_not_found`); a Member changes nothing. An agent removes its own
//! workspace's test guests with "Build and run Apps" (removing narrows).
//!
//! **What K2 holds it to** (with `k2_core::skin::test_guests`):
//! - an App-only guest of THIS App (one `kind=app` grant, no box login);
//!   with no role it may get rooms (`rooms`), an agent's only its own
//!   workspace's room;
//! - one an agent makes (or a card it asked for) is stamped and tied to
//!   the agent's workspace (`skin::creators`): it is never "a person";
//!   it signs in only through this App's gateway link, never the box login,
//!   another App, the browser helper or a device sign-in;
//! - a role only when the App already defines it (`roles.app_id` = this
//!   App), never a box-wide role and never a new one;
//! - 24 h by default, at most 7 days; its sessions end with it; expired
//!   ones are removed at the next add or list;
//! - at most 5 live per App;
//! - shown as `test` in `k2 app user list`; every add and remove is in the
//!   App's audit and the Access Audit, and an add leaves a note on the
//!   workspace's Thread.

use std::collections::HashMap;

use k2_core::deploy::{self, codes, store::AppRow};
use k2_core::skin::{self, test_guests as tg};
use serde_json::{json, Value as J};

use super::routes::{actor, agent_ws, app_for, json_body, need_build, need_credential, no, s, ws_by_id, Who, R};

pub const ROUTES: &[&str] = &["/cli/app/test-guest/add", "/cli/app/test-guest/rm", "/cli/app/test-guests", PASSWORD];
pub const POST_ROUTES: &[&str] = &["/cli/app/test-guest/add", "/cli/app/test-guest/rm", PASSWORD];

/// `k2 app user password <name> --test --password-stdin`: set a test
/// guest's password ([`password`]).
pub const PASSWORD: &str = "/cli/app/test-guest/password";

pub const TOO_MANY: &str = "too_many_test_guests";
pub const ROLE_NOT_THIS_APPS: &str = "role_not_this_apps";
pub const NOT_A_TEST_GUEST: &str = "not_a_test_guest";

/// May an agent passport manage test guests through the skin-manage
/// routes (`/cli/skin/users/*`: password, email, rooms, unlock, remove)?
/// **Off**: an agent makes and removes its test guests only here, under
/// "Let this agent manage its Apps' users" (`super::agent_grants`).
pub const AGENTS_MAY_MANAGE: bool = false;

/// Remove: a person, or the App's own agent with "Let this agent manage
/// its Apps' users" or "Build and run Apps" (removing narrows; [`rm`]
/// keeps it to its own workspace's guests).
fn need_remover(who: &Who) -> R<()> {
    need_credential(who)?;
    if super::agent_grants::may_self(who) {
        return Ok(());
    }
    need_build(who)
}

pub(crate) fn handle(path: &str, who: &Who, body: &[u8], params: &HashMap<String, String>) -> R<J> {
    match path {
        "/cli/app/test-guest/add" => add(who, body),
        "/cli/app/test-guest/rm" => rm(who, body),
        PASSWORD => password(who, body),
        "/cli/app/test-guests" => list(who, params),
        _ => Err(no("not_found", 404, "no such App route")),
    }
}

fn store_err(e: String) -> super::routes::Refusal {
    no("db_error", 500, e)
}

fn app_audit(app: &AppRow, event: &str, by: &str, payload: J) {
    let h = k2_core::db::shared();
    let conn = h.lock();
    if let Err(e) = deploy::audit::append(&conn, deploy::audit::SUBJECT_APP, &app.app_id, event, by, None, &payload) {
        k2_core::log_debug!("[apps] audit {} {event}: {e}", app.app_id);
    }
}

fn access_audit(event: &str, by: &str, detail: String) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(event, by, detail, "cli", "-", "apps"));
}

/// Remove this App's expired test guests (their sessions go with them).
fn sweep(app: &AppRow, by: &str) {
    match tg::expired_usernames(Some(&app.service_id)) {
        Ok(names) => {
            for u in names {
                if skin::remove_principal_as(&u, by).unwrap_or(false) {
                    app_audit(app, "test_guest_expired", by, json!({ "username": u }));
                }
            }
        }
        Err(e) => k2_core::log_debug!("[apps] {} test guest sweep: {e}", app.app_id),
    }
}

fn principal(username: &str) -> R<Option<skin::SkinPrincipal>> {
    let u = username.trim().to_ascii_lowercase();
    Ok(skin::list_principals().map_err(store_err)?.into_iter().find(|p| p.username == u))
}

/// `24 h`, `90 min`, `7 days`.
fn human_ttl(secs: i64) -> String {
    match secs {
        s if s % 86_400 == 0 && s >= 2 * 86_400 => format!("{} days", s / 86_400),
        s if s % 3600 == 0 => format!("{} h", s / 3600),
        s => format!("{} min", s / 60),
    }
}

/// One test guest for the answers and the list.
fn guest_json(app: &AppRow, username: &str, t: &tg::TestGuest) -> J {
    let (role, rooms) = grant_view(&t.principal_id, &t.app);
    json!({
        "username": username,
        "app": app.app_id,
        "appServiceId": t.app,
        "role": role,
        "rooms": rooms,
        "expiresAt": t.expires_at,
        "expired": t.expired_at(deploy::now()),
        "createdBy": t.created_by,
        "createdAt": t.created_at,
    })
}

/// A principal's role name and room handles on ONE App's grant (where a
/// test guest's role and rooms live; 0.46.5 item 15: `k2 app user list`
/// showed "—" for both because it read only the box-level fields).
pub(crate) fn grant_view(principal_id: &str, service_id: &str) -> (Option<String>, Vec<String>) {
    let grants = skin::list_grants().unwrap_or_default();
    let Some(g) = grants
        .iter()
        .find(|g| g.enabled && g.kind == "app" && g.subject_kind == "principal" && g.subject_id == principal_id && g.target_id == service_id)
    else {
        return (None, Vec::new());
    };
    let roles = skin::list_roles().unwrap_or_default();
    let role = g.role_id.as_deref().and_then(|id| roles.iter().find(|r| r.id == id));
    let room_ids: Vec<String> = match role {
        Some(r) => r.rooms.clone(),
        None => g.scope.as_deref().and_then(|s| serde_json::from_str::<Vec<String>>(s).ok()).unwrap_or_default(),
    };
    (role.map(|r| r.name.clone()), room_ids.iter().map(|id| super::routes::ws_handle(id)).collect())
}

/// POST `/cli/app/test-guest/add {app, username, role?, expires?, rooms?, password?}`.
fn add(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    let ws = agent_ws(who).map(str::to_string);
    if ws.is_none() {
        need_build(who)?;
    }
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    // A held request's replay names the App's service id too: the same App
    // id on a different App since is refused, never made.
    if let Some(svc) = s(&v, "service") {
        if svc != app.service_id {
            return Err(no("app_changed", 409, format!("{} is a different App now than when this was asked: ask again", app.app_id)));
        }
    }
    let raw = s(&v, "username").ok_or_else(|| no(codes::BAD_REQUEST, 400, "username is required"))?;
    // Normalized first (no control characters, the length cap): this is
    // the name every card, note and audit line shows.
    let username = skin::normalize_username(raw).map_err(|e| no(codes::BAD_REQUEST, 400, e))?;
    let rooms = super::agent_grants::list_field(&v, "rooms").unwrap_or_default();
    let rooms = super::agent_grants::resolve_rooms(&rooms)?;
    if let Some(ws) = ws.as_deref() {
        super::agent_grants::own_rooms_only(ws, &rooms)?;
        if !super::agent_grants::may_self(who) {
            // The test-guest rules first, so a request the owner approves
            // can't fail on them (they run again at the approval).
            let expires = s(&v, "expires");
            tg::parse_ttl(expires).map_err(|e| no(codes::BAD_REQUEST, 400, e))?;
            let role_id = role_of(&app, s(&v, "role"), true)?;
            let ask = super::agent_grants::TestGuestAsk {
                username: &username,
                role_id: role_id.as_deref(),
                rooms: &rooms,
                expires,
                reason: s(&v, "reason"),
                password_given: s(&v, "password").is_some_and(|p| !p.is_empty()),
            };
            return super::agent_grants::test_guest_held(who, ws, &app, &ask);
        }
    }
    let spec = NewTestGuest { username: &username, role: s(&v, "role"), expires: s(&v, "expires"), rooms: &rooms, password: s(&v, "password") };
    add_checked(&app, &actor(who), ws.as_deref(), &spec)
}

/// What a test guest add asks for.
pub(crate) struct NewTestGuest<'a> {
    pub username: &'a str,
    pub role: Option<&'a str>,
    pub expires: Option<&'a str>,
    /// Room ids (resolved), only without a role.
    pub rooms: &'a [String],
    pub password: Option<&'a str>,
}

/// Make the test guest: every test-guest rule, for every caller. `agent`:
/// the workspace whose agent made it, or asked for it on a card (scope
/// already checked by the caller): no `files:agentconfig` role, and the
/// guest is stamped and tied to that workspace.
pub(crate) fn add_checked(app: &AppRow, by: &str, agent: Option<&str>, n: &NewTestGuest<'_>) -> R<J> {
    let by = by.to_string();
    let ttl = tg::parse_ttl(n.expires).map_err(|e| no(codes::BAD_REQUEST, 400, e))?;
    sweep(app, &by);
    let live = tg::live_count(&app.service_id).map_err(store_err)?;
    if live >= tg::MAX_PER_APP {
        return Err(no(
            TOO_MANY,
            409,
            format!("{} already has {live} test guests (at most {}): remove one first (k2 app test-guests {})", app.app_id, tg::MAX_PER_APP, app.app_id),
        ));
    }
    let role = role_of(app, n.role, agent.is_some())?;
    if role.is_some() && !n.rooms.is_empty() {
        return Err(no(codes::BAD_REQUEST, 400, "a test guest with a role gets its role's rooms: give rooms only without a role"));
    }
    let password = n.password.map(str::to_string).unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string());
    // An agent's guest (its own call, or its approved request) is stamped
    // and tied to its workspace in the same transaction as the principal:
    // it is never "a person" (`skin::ties`), never the owner's.
    let added_by = skin::AddedBy { agent_workspace: agent, ..Default::default() };
    let p = skin::add_principal_for_app_by(n.username, None, &app.service_id, role.as_deref(), &by, added_by)
        .map_err(|e| no(codes::BAD_REQUEST, 409, e))?;
    let undo = |e: String| {
        let _ = skin::remove_principal_as(&p.username, &by);
        no("db_error", 500, e)
    };
    skin::set_principal_password(&p.username, Some(&password)).map_err(|e| {
        let _ = skin::remove_principal_as(&p.username, &by);
        no(codes::BAD_REQUEST, 400, e)
    })?;
    if !n.rooms.is_empty() {
        skin::set_app_grant_rooms(&p.username, &app.service_id, n.rooms, true, &by).map(|_| ()).map_err(undo)?;
    }
    let expires_at = deploy::now() + ttl;
    let t = tg::record(&p.id, &app.service_id, &app.owner_workspace, &by, expires_at).map_err(undo)?;
    let role_name = role.as_deref().and_then(|id| skin::list_roles().ok()?.into_iter().find(|r| r.id == id).map(|r| r.name));
    let room_handles: Vec<String> = n.rooms.iter().map(|r| super::routes::ws_handle(r)).collect();
    app_audit(
        app,
        "test_guest_add",
        &by,
        json!({ "username": p.username, "role": role_name, "rooms": room_handles, "expiresAt": expires_at, "agent": agent.map(super::routes::ws_handle) }),
    );
    access_audit("app-test-guest", &by, format!("added {} to {} (role {}, until {expires_at})", p.username, app.app_id, role_name.as_deref().unwrap_or("none")));
    // A note on the workspace's Thread (no wake).
    let owner_note = ws_by_id(&app.owner_workspace)
        .map(|w| {
            let text = format!(
                "K2: {by} added the test guest **{}** to App **{}** (role {}{}, for {}). It signs in only through the App's gateway link \
                 and ends on its own. `k2 app user remove {} --test` removes it now.",
                p.username,
                app.app_id,
                role_name.as_deref().unwrap_or("none"),
                if room_handles.is_empty() { String::new() } else { format!(", rooms {}", room_handles.join(", ")) },
                human_ttl(ttl),
                p.username
            );
            match crate::overlay_routes::post_workspace_note(&w.path, &text, "k2-apps") {
                Ok(()) => true,
                Err(e) => {
                    k2_core::log_debug!("[apps] test guest note for {}: {e}", app.app_id);
                    false
                }
            }
        })
        .unwrap_or(false);
    let mut out = guest_json(app, &p.username, &t);
    out["ok"] = json!(true);
    out["role"] = json!(role_name);
    out["rooms"] = json!(room_handles);
    out["ownerNote"] = json!(owner_note);
    out["tied"] = json!(agent.is_some());
    if n.password.is_none() {
        // Shown once: K2 keeps only its hash.
        out["password"] = json!(password);
    }
    out["note"] = json!(format!(
        "{} is a test guest of {} until {expires_at} (unix): it signs in only through the App's link (POST /cli/skin/login with the App pass), \
         never the box login. Remove it with k2 app user remove {} --test.",
        p.username, app.app_id, p.username
    ));
    Ok(out)
}

/// Only a role this App already defines; never box-wide, never new; an
/// agent's never one that writes the agent's `.k2` folder.
pub(crate) fn role_of(app: &AppRow, token: Option<&str>, agent: bool) -> R<Option<String>> {
    let Some(token) = token.map(str::trim).filter(|r| !r.is_empty()) else { return Ok(None) };
    let roles = skin::list_roles().map_err(store_err)?;
    let mine: Vec<&skin::SkinRole> = roles.iter().filter(|r| r.app_id.as_deref() == Some(app.service_id.as_str())).collect();
    let Some(r) = mine.iter().find(|r| r.id == token || r.name.eq_ignore_ascii_case(token)) else {
        let names: Vec<&str> = mine.iter().map(|r| r.name.as_str()).collect();
        return Err(no(
            ROLE_NOT_THIS_APPS,
            400,
            if names.is_empty() {
                format!("{} defines no roles: a test guest gets one of the App's own roles or none (k2 app role create … --app {})", app.app_id, app.app_id)
            } else {
                format!("'{token}' isn't one of {}'s own roles ({}): a test guest gets only a role the App already defines", app.app_id, names.join(", "))
            },
        ));
    };
    if agent && r.caps.iter().any(|c| c == skin::CAP_FILES_AGENT_CONFIG) {
        return Err(no(
            codes::OWNER_ONLY,
            403,
            format!("role '{}' lets guests write the agent's .k2 folder (files:agentconfig): handing it out stays the owner's", r.name),
        ));
    }
    Ok(Some(r.id.clone()))
}

/// POST `/cli/app/test-guest/rm {username}`: a person removes any test
/// guest; an agent with "Build and run Apps" only its own workspace's
/// Apps'.
fn rm(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    if agent_ws(who).is_none() {
        need_remover(who)?;
    }
    let v = json_body(body)?;
    let username = s(&v, "username").ok_or_else(|| no(codes::BAD_REQUEST, 400, "username is required"))?;
    let missing = || no(NOT_A_TEST_GUEST, 404, format!("'{username}' isn't a test guest you can remove (k2 app test-guests <app>)"));
    let p = principal(username)?.ok_or_else(missing)?;
    let t = tg::get(&p.id).map_err(store_err)?.ok_or_else(missing)?;
    let app = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        deploy::store::get_app_by_service(&conn, &t.app).ok().flatten()
    };
    if let Some(ws) = agent_ws(who) {
        // The App's CURRENT workspace (an App handed over since is another
        // workspace's), and only a test guest its own workspace's agent
        // made: an owner-made one stays the owner's (review 0.46.4).
        if app.as_ref().is_none_or(|a| a.owner_workspace != ws) {
            return Err(missing());
        }
        if skin::creators::workspace_of(&p.id).map_err(store_err)?.as_deref() != Some(ws) {
            return Err(no(
                codes::OWNER_ONLY,
                403,
                format!("'{}' is a test guest the owner made: it stays the owner's (ask your human: k2 app user remove {} --test)", p.username, p.username),
            ));
        }
        // Another workspace's App answered 404 above, switch or not (no
        // oracle); its own: the switch.
        need_remover(who)?;
    }
    let by = actor(who);
    let removed = skin::remove_principal_as(&p.username, &by).map_err(store_err)?;
    if let Some(a) = &app {
        app_audit(a, "test_guest_rm", &by, json!({ "username": p.username }));
    }
    access_audit("app-test-guest", &by, format!("removed {}", p.username));
    Ok(json!({ "ok": true, "username": p.username, "removed": removed }))
}

/// POST `/cli/app/test-guest/password {username, password}`.
///
/// - A person: any test guest.
/// - The App's own agent, on a test guest its own workspace made (the App
///   still its workspace's): with "Let this agent manage its Apps' users"
///   any number of times; without it exactly ONCE per test guest the owner
///   approved from its held request (review 0.46.4: a plain Allow must give
///   a usable guest without turning the switch on). The once is a
///   compare-and-set on that request's stored result (`passwordSet`), so
///   two calls can't both win; the password itself is never stored there.
fn password(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    let v = json_body(body)?;
    let username = s(&v, "username").ok_or_else(|| no(codes::BAD_REQUEST, 400, "username is required"))?;
    let pw = s(&v, "password").ok_or_else(|| no(codes::BAD_REQUEST, 400, "password is required (k2 app user password <name> --test --password-stdin)"))?;
    let missing = || no(NOT_A_TEST_GUEST, 404, format!("'{username}' isn't a test guest you can set a password for (k2 app test-guests <app>)"));
    let p = principal(username)?.ok_or_else(missing)?;
    let t = tg::get(&p.id).map_err(store_err)?.ok_or_else(missing)?;
    let app = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        deploy::store::get_app_by_service(&conn, &t.app).ok().flatten()
    };
    // Checked before the one password set is used (review 0.46.4): a
    // refused password never spends it.
    if pw.trim().chars().count() < skin::PUBLIC_PASSWORD_MIN_LEN {
        return Err(no("bad_password", 400, format!("password must be at least {} characters", skin::PUBLIC_PASSWORD_MIN_LEN)));
    }
    let by = actor(who);
    let mut once: Option<String> = None;
    if let Some(ws) = agent_ws(who) {
        if app.as_ref().is_none_or(|a| a.owner_workspace != ws) {
            return Err(missing());
        }
        if skin::creators::workspace_of(&p.id).map_err(store_err)?.as_deref() != Some(ws) {
            return Err(no(codes::OWNER_ONLY, 403, format!("'{}' is a test guest the owner made: its password stays the owner's", p.username)));
        }
        if !super::agent_grants::may_self(who) {
            let id = approved_request_for(ws, &p.username)?.ok_or_else(|| {
                super::agent_grants::users_off(&format!(
                    "Setting '{}''s password again (its one password set after the owner's approval is used)",
                    p.username
                ))
            })?;
            once = Some(id);
        }
    } else {
        need_build(who)?;
    }
    if let Some(id) = once.as_deref() {
        // Single use, before the write: a lost race sets nothing.
        let won = {
            let h = k2_core::db::shared();
            let conn = h.lock();
            conn.execute(
                "UPDATE app_widenings SET result_json = json_set(COALESCE(result_json, '{}'), '$.passwordSet', json('true')) \
                 WHERE id = ?1 AND COALESCE(json_extract(result_json, '$.passwordSet'), 0) = 0",
                rusqlite::params![id],
            )
            .map_err(store_err_db)?
        };
        if won != 1 {
            return Err(super::agent_grants::users_off(&format!("Setting '{}''s password again", p.username)));
        }
    }
    if let Err(e) = skin::set_principal_password(&p.username, Some(pw)) {
        // The write failed: the one set is given back.
        if let Some(id) = once.as_deref() {
            let h = k2_core::db::shared();
            let conn = h.lock();
            if let Err(e) = conn.execute(
                "UPDATE app_widenings SET result_json = json_remove(result_json, '$.passwordSet') WHERE id = ?1",
                rusqlite::params![id],
            ) {
                k2_core::log_debug!("[apps] give back the password set of {id}: {e}");
            }
        }
        return Err(no(codes::BAD_REQUEST, 400, e));
    }
    if let Some(a) = &app {
        app_audit(a, "test_guest_password", &by, json!({ "username": p.username, "approvedRequest": once }));
    }
    access_audit("app-test-guest", &by, format!("set the password of {}", p.username));
    Ok(json!({ "ok": true, "username": p.username, "once": once.is_some() }))
}

fn store_err_db(e: rusqlite::Error) -> super::routes::Refusal {
    no("db_error", 500, e.to_string())
}

/// The owner-approved held test-guest request of `ws` that made
/// `username`, while its one password set is unused.
fn approved_request_for(ws: &str, username: &str) -> R<Option<String>> {
    let rows = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        let f = k2_core::app_grace::requests::Filter { app_id: None, service_id: None, workspace: Some(ws), status: Some(k2_core::app_grace::requests::STATUS_APPROVED) };
        k2_core::app_grace::requests::list(&conn, &f, 200).map_err(store_err_db)?
    };
    Ok(rows
        .into_iter()
        .find(|r| {
            r.requester_workspace.as_deref() == Some(ws)
                && r.body.get("route").and_then(J::as_str) == Some(super::agent_grants::TEST_GUEST_ADD)
                && r.body.pointer("/body/username").and_then(J::as_str) == Some(username)
                && !r.result.as_ref().and_then(|x| x.get("passwordSet")).is_some_and(|v| v.as_bool() == Some(true) || v.as_i64().is_some_and(|n| n != 0))
        })
        .map(|r| r.id))
}

/// GET `/cli/app/test-guests?app=`.
fn list(who: &Who, params: &HashMap<String, String>) -> R<J> {
    need_credential(who)?;
    let app = app_for(who, params.get("app").map(String::as_str))?;
    // The sweep removes guests: only for a caller that may remove them.
    if need_remover(who).is_ok() {
        sweep(&app, &actor(who));
    }
    let by_id: HashMap<String, String> = skin::list_principals().map_err(store_err)?.into_iter().map(|p| (p.id, p.username)).collect();
    let guests: Vec<J> = tg::list(Some(&app.service_id))
        .map_err(store_err)?
        .iter()
        .filter_map(|t| by_id.get(&t.principal_id).map(|u| guest_json(&app, u, t)))
        .collect();
    Ok(json!({ "ok": true, "app": app.app_id, "max": tg::MAX_PER_APP, "testGuests": guests }))
}
