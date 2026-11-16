//! `k2 app user …` by an App's own agent on a login of its OWN App whose
//! rooms reach outside its workspace (0.46.14, an owner's request): held for
//! an owner or admin on a card, instead of refused.
//!
//! "Let this agent manage its Apps' users" covers the logins wholly inside the
//! agent's workspace (`skin_routes::principal_in_workspace`). A login of its own
//! App that talks in another workspace's room (a box-wide role naming that room,
//! rooms on the login itself) was refused outright: "only the owner changes
//! guests and roles outside your workspace". Now:
//!
//! | the change (the agent has the App-users switch) | a login only of its own Apps, reaching another workspace's room | any other login (another workspace's App, a box login, a test guest, chat or mail grants) |
//! |---|---|---|
//! | `k2 app user add <u> --app <its app> --role <box-wide role naming another room>` | one held request + card ([`ADD_APPLY`]) | refused, as before |
//! | `k2 app user remove <u>` | held ([`REMOVE_APPLY`]) | refused |
//! | `k2 app user role <u> <role>` (a box-wide role, or one of the login's own App) | held ([`ROLE_APPLY`]) | refused |
//! | `k2 app user reset <u> --app <its app>` | held ([`RESET_APPLY`]): on approval K2 mails the person the App's one-time reset link | 404, as before |
//! | `k2 app user password <u> --password-stdin` | held ([`PASSWORD_APPLY`]): the approval opens ONE password set for the agent; the password is never on the request, the card or a log | refused |
//!
//! Without the switch the routes' gate refuses first (`app_users_off`, with
//! the "ask for the switch" path), so nothing here runs.
//!
//! **The approval** (an Owner or Admin, `approvals::apply_door`) runs exactly
//! the change the card showed, as the person who approved it, while the login
//! is still the same one (by id) and still only the agent's Apps'; else
//! nothing, and the agent is told to ask again. Deny: nothing.
//!
//! **Passwords.** A held request never keeps one (as a held test guest,
//! `agent_grants::test_guest_held`). An approved password or add (when the
//! add carried a password) lets the agent set that login's password once,
//! within [`PASSWORD_ONCE_SECS`], with the same command: single use, a
//! compare-and-set on the request's stored result (`passwordSet`), the way
//! `test_guests::password` spends its one set. The set ties the login to the
//! agent's workspace and asks the person for their own password at the next
//! sign-in, as any password an agent sets.

use k2_core::app_grace::requests as reqs;
use k2_core::deploy::{codes, store};
use k2_core::skin;
use serde_json::{json, Value as J};

use super::agent_grants::{hold, Step};
use super::routes::{no, respond, ws_handle, Who, R};
use crate::cli_response::CliResponse;

/// The routes a held request carries (never HTTP routes: the approval runs
/// [`apply`] as the owner or admin who approved).
pub const ADD_APPLY: &str = "/cli/app/user/add/apply";
pub const REMOVE_APPLY: &str = "/cli/app/user/remove/apply";
pub const PASSWORD_APPLY: &str = "/cli/app/user/password/apply";
pub const RESET_APPLY: &str = "/cli/app/user/reset/apply";
pub const ROLE_APPLY: &str = "/cli/app/user/role/apply";

/// Every route [`apply`] carries out.
pub const APPLY_ROUTES: &[&str] = &[ADD_APPLY, REMOVE_APPLY, PASSWORD_APPLY, RESET_APPLY, ROLE_APPLY];

/// How long the one password set an approval opens stays usable.
pub const PASSWORD_ONCE_SECS: i64 = 24 * 3600;

/// The skin-manage routes this wraps.
const USERS: &str = "/cli/skin/users";
const REMOVE: &str = "/cli/skin/users/remove";
const PASSWORD: &str = "/cli/skin/users/password";
const ASSIGN: &str = "/cli/skin/roles/assign";

// ── what a login is ─────────────────────────────────────────────────

/// A login only of the agent's own Apps: what K2 may put on a card.
pub(crate) struct OwnLogin {
    pub p: skin::SkinPrincipal,
    /// Its Apps' service ids (all the workspace's).
    pub apps: Vec<String>,
    /// The rooms it reaches outside the workspace (ids), sorted.
    pub outside: Vec<String>,
}

fn store_err(e: impl std::fmt::Display) -> super::routes::Refusal {
    no("db_error", 500, e.to_string())
}

/// The published service `id`, when it is workspace `ws`'s.
fn service_of(id: &str, ws: &str) -> Option<k2_core::published_services::PublishedService> {
    let h = k2_core::db::shared();
    let conn = h.lock();
    k2_core::published_services::get_by_id(&conn, id).ok().flatten().filter(|s| s.project_id == ws)
}

/// `<workspace>/<name>` of the agent's App.
fn app_handle(id: &str, ws: &str) -> String {
    match service_of(id, ws) {
        Some(s) => format!("{}/{}", ws_handle(ws), s.name),
        None => id.to_string(),
    }
}

/// Every room a role names (its rooms and its room lines).
fn role_rooms(r: &skin::SkinRole) -> Vec<String> {
    r.rooms.iter().chain(r.room_policy.keys()).cloned().collect()
}

/// The rooms of `rooms` outside `ws`, deduped and sorted.
fn outside_of(rooms: impl IntoIterator<Item = String>, ws: &str) -> Vec<String> {
    let mut out: Vec<String> = rooms.into_iter().filter(|r| r != ws && !r.is_empty()).collect();
    out.sort();
    out.dedup();
    out
}

/// Every room login `p` reaches: its own rooms, its role's, and each grant's
/// rooms and role.
fn rooms_reached(p: &skin::SkinPrincipal, roles: &[skin::SkinRole], grants: &[&skin::SkinGrant]) -> Vec<String> {
    let role = |id: &Option<String>| id.as_ref().and_then(|id| roles.iter().find(|r| &r.id == id));
    let mut out: Vec<String> = p.default_rooms.clone();
    if let Some(r) = role(&p.role_id) {
        out.extend(role_rooms(r));
    }
    for g in grants {
        out.extend(skin::parse_rooms_json(g.scope.as_deref()));
        if let Some(r) = role(&g.role_id) {
            out.extend(role_rooms(r));
        }
    }
    out
}

/// `username` as a login only of workspace `ws`'s Apps: every grant it holds
/// is an App login on one of `ws`'s Apps (never the box login, never another
/// workspace's App, never a chat, mail or database grant), it isn't another
/// workspace's agent, tied to or made by another workspace, nor a test guest
/// (those have their own path). `Ok(None)`: any other login, or none.
pub(crate) fn own_login(ws: &str, username: &str) -> R<Option<OwnLogin>> {
    let Some(u) = skin::login_principal_username(username).map_err(store_err)? else { return Ok(None) };
    let Some(p) = skin::list_principals().map_err(store_err)?.into_iter().find(|p| p.username == u) else { return Ok(None) };
    if p.agent_workspace.as_deref().is_some_and(|a| a != ws) {
        return Ok(None);
    }
    if skin::test_guests::get(&p.id).map_err(store_err)?.is_some() {
        return Ok(None);
    }
    if skin::ties::get(&p.id).map_err(store_err)?.is_some_and(|t| t.workspace_id != ws) {
        return Ok(None);
    }
    if skin::creators::workspace_of(&p.id).map_err(store_err)?.is_some_and(|c| c != ws) {
        return Ok(None);
    }
    let grants = skin::list_grants().map_err(store_err)?;
    let mine: Vec<&skin::SkinGrant> = grants.iter().filter(|g| g.subject_kind == "principal" && g.subject_id == p.id).collect();
    if mine.is_empty() {
        return Ok(None);
    }
    let mut apps: Vec<String> = Vec::new();
    for g in &mine {
        if g.kind != "app" || g.target_id == skin::HOST_APP_ID || service_of(&g.target_id, ws).is_none() {
            return Ok(None);
        }
        if !apps.contains(&g.target_id) {
            apps.push(g.target_id.clone());
        }
    }
    let roles = skin::list_roles().map_err(store_err)?;
    let outside = outside_of(rooms_reached(&p, &roles, &mine), ws);
    Ok(Some(OwnLogin { p, apps, outside }))
}

/// Is `p` wholly inside `ws` (the switch's own reach)?
fn inside(p: &skin::SkinPrincipal, ws: &str) -> R<bool> {
    let roles = skin::list_roles().map_err(store_err)?;
    let grants = skin::list_grants().map_err(store_err)?;
    Ok(crate::skin_routes::principal_in_workspace(p, &roles, &grants, ws))
}

/// Room handles, comma-joined.
fn handles(ids: &[String]) -> String {
    ids.iter().map(|id| ws_handle(id)).collect::<Vec<_>>().join(", ")
}

/// `<app>, <app>` of a login.
fn apps_words(l: &OwnLogin, ws: &str) -> String {
    l.apps.iter().map(|a| app_handle(a, ws)).collect::<Vec<_>>().join(", ")
}

// ── the refusal this turns into a card ──────────────────────────────

fn error_of(r: &CliResponse) -> (String, String) {
    let v: J = serde_json::from_str(&r.body).unwrap_or(J::Null);
    let code = v.pointer("/error/code").and_then(J::as_str).unwrap_or_default().to_string();
    let hint = v.pointer("/error/hint").and_then(J::as_str).unwrap_or_default().to_string();
    (code, hint)
}

/// The agent was refused because the login or the role reaches outside its
/// workspace (`skin_routes::outside_workspace`), and nothing was written.
fn refused_outside(r: &CliResponse) -> bool {
    let (code, hint) = error_of(r);
    r.status.starts_with("403") && code == "owner_only" && hint.contains("outside your workspace")
}

fn text<'a>(v: &'a J, keys: &[&str]) -> Option<&'a str> {
    keys.iter().filter_map(|k| v.get(*k).and_then(J::as_str)).map(str::trim).find(|s| !s.is_empty())
}

fn agent(ws: &str, session: Option<&str>) -> Who {
    Who::Agent { project_id: ws.to_string(), session_id: session.unwrap_or_default().to_string() }
}

/// The dispatcher's entry for the skin-manage routes an agent's `k2 app user`
/// calls (`/cli/skin/users`, `…/remove`, `…/password`, `/cli/skin/roles/assign`):
/// run `inner` (the route's own scoped call, every check of
/// today); when it refused the agent of `agent_ws` because a login of its own
/// App, or the role, reaches outside its workspace, hold the change for an
/// owner or admin instead. Everything else answers as `inner` did.
pub fn scoped(path: &str, agent_ws: Option<&str>, session: Option<&str>, body: &[u8], inner: impl FnOnce() -> CliResponse) -> CliResponse {
    let r = inner();
    let Some(ws) = agent_ws else { return r };
    if !refused_outside(&r) {
        return r;
    }
    let Ok(v) = serde_json::from_slice::<J>(body) else { return r };
    let held = match path {
        USERS => add_held(ws, session, &v),
        REMOVE => existing_held(ws, session, &v, Op::Remove),
        PASSWORD => password_or_held(ws, session, &v, body),
        ASSIGN => existing_held(ws, session, &v, Op::Role),
        _ => Ok(None),
    };
    match held {
        Ok(Some(resp)) => resp,
        Ok(None) => r,
        Err(e) => super::routes::refusal_response(&e),
    }
}

/// `POST /cli/skin/users/reset-link` from an agent of `ws` (with the App-users
/// switch): a login of its own App that reaches another workspace's room is
/// held (on approval K2 mails the person the App's reset link); everything
/// else is `app_user_invites::handle_agent_reset`, as before.
pub fn reset(ws: &str, session: Option<&str>, body: &[u8], actor: &str, is_post: bool) -> CliResponse {
    let ours = || -> R<Option<CliResponse>> {
        if !is_post {
            return Ok(None);
        }
        let Ok(v) = serde_json::from_slice::<J>(body) else { return Ok(None) };
        // Those are the owner's, or take back links: the handler's answer.
        if text(&v, &["to", "from"]).is_some() || v.get("code").and_then(J::as_bool) == Some(true) || v.get("revoke").and_then(J::as_bool) == Some(true) {
            return Ok(None);
        }
        let Some(username) = text(&v, &["username", "name"]) else { return Ok(None) };
        let Some(l) = own_login(ws, username)? else { return Ok(None) };
        if l.outside.is_empty() || inside(&l.p, ws)? {
            return Ok(None);
        }
        let app = match text(&v, &["app", "appId"]) {
            Some(raw) => match crate::skin_routes::resolve_app_target(raw) {
                Ok(a) if l.apps.contains(&a) => a,
                _ => return Ok(None),
            },
            None => match l.apps.as_slice() {
                [one] => one.clone(),
                _ => return Ok(None),
            },
        };
        if l.p.email.is_none() {
            return Err(no(
                "no_email",
                409,
                format!("'{}' has no email: a reset goes as a one-time link to the email on the login. Ask your human to set one, or ask for one password set (k2 app user password {} --password-stdin)", l.p.username, l.p.username),
            ));
        }
        let what = format!("the login {} (App {}; it talks in {})", l.p.username, app_handle(&app, ws), handles(&l.outside));
        let body = json!({ "workspace": ws, "principal": l.p.id, "username": l.p.username, "service": app, "outside": l.outside });
        Ok(Some(respond(file(
            ws,
            session,
            &app,
            RESET_APPLY,
            body,
            format!("mail {what} a one-time link to choose a new password"),
            vec![
                "K2 mails the App's reset link to the email on the login, from the App's address: nobody else sees the link, and no password is on this card".to_string(),
                "their current password keeps working until they use the link".to_string(),
            ],
            (l.p.username.as_str(), l.outside.as_slice()),
            text(&v, &["reason"]),
        ))))
    };
    match ours() {
        Ok(Some(r)) => r,
        Ok(None) => crate::app_user_invites::handle_agent_reset(body, actor, ws, is_post),
        Err(e) => super::routes::refusal_response(&e),
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Op {
    Remove,
    Role,
}

/// One held request for a change to a login of the agent's own App.
#[allow(clippy::too_many_arguments)]
fn file(ws: &str, session: Option<&str>, app: &str, route: &'static str, body: J, change: String, more: Vec<String>, who_for: (&str, &[String]), reason: Option<&str>) -> R<J> {
    let (username, outside) = who_for;
    let who = agent(ws, session);
    let row = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        store::get_app_by_service(&conn, app).ok().flatten()
    };
    let label = service_of(app, ws).map(|s| s.name).unwrap_or_else(|| app.to_string());
    let mut lines = vec![change.clone()];
    lines.extend(more);
    hold(
        Step {
            who: &who,
            app: row.as_ref(),
            label: &label,
            workspace: ws,
            route,
            body,
            lines,
            summary: change,
            why: format!(
                "{username} talks in {}, outside the agent's workspace: changing it is the owner's or an admin's",
                handles(outside)
            ),
            reason,
        },
        false,
    )
}

/// A role an agent may ask for on login `l`: box-wide, or a role of one of
/// its Apps. `None`: another App's role (stays refused).
fn role_for(l: &OwnLogin, token: &str) -> R<Option<skin::SkinRole>> {
    match skin::find_role(token).map_err(store_err)? {
        Some(r) if r.app_id.as_deref().is_none_or(|a| l.apps.iter().any(|x| x == a)) => Ok(Some(r)),
        _ => Ok(None),
    }
}

/// The role's name and the rooms it names (`ws`'s first, then the others).
fn role_words(r: &skin::SkinRole, ws: &str) -> String {
    let out = outside_of(role_rooms(r), ws);
    let inside: Vec<String> = role_rooms(r).into_iter().filter(|x| x == ws).take(1).collect();
    let mut all = inside;
    all.extend(out);
    if all.is_empty() {
        format!("the role {} (no rooms)", r.name)
    } else {
        format!("the role {} (rooms {})", r.name, handles(&all))
    }
}

/// Remove and role on an existing login.
fn existing_held(ws: &str, session: Option<&str>, v: &J, op: Op) -> R<Option<CliResponse>> {
    let Some(username) = text(v, &["username", "name"]) else { return Ok(None) };
    let Some(mut l) = own_login(ws, username)? else { return Ok(None) };
    let role = match op {
        Op::Role => {
            let Some(token) = text(v, &["role", "id"]) else { return Ok(None) };
            match role_for(&l, token)? {
                Some(r) => Some(r),
                None => return Ok(None),
            }
        }
        _ => None,
    };
    // What reaches outside: the login now, and the role it would get.
    let now_outside = l.outside.clone();
    if let Some(r) = &role {
        let mut all = l.outside.clone();
        all.extend(role_rooms(r));
        l.outside = outside_of(all, ws);
    }
    if l.outside.is_empty() {
        return Ok(None);
    }
    let app = l.apps[0].clone();
    let apps = apps_words(&l, ws);
    let talks = if now_outside.is_empty() {
        format!("it talks only in {}'s room now", ws_handle(ws))
    } else {
        format!("it talks in {} now", handles(&now_outside))
    };
    let mut body = json!({ "workspace": ws, "principal": l.p.id, "username": l.p.username, "apps": l.apps, "outside": l.outside });
    let (route, change, more) = match op {
        Op::Remove => (
            REMOVE_APPLY,
            format!("remove the login {} (App {apps}; it talks in {})", l.p.username, handles(&now_outside)),
            vec![format!("{} can no longer sign in to {apps}; their sessions end. There is no undo: add them again to bring them back", l.p.username)],
        ),
        Op::Role => {
            let r = role.as_ref().expect("a role");
            body["role"] = json!(r.id);
            body["roleName"] = json!(r.name);
            (
                ROLE_APPLY,
                format!("give the login {} (App {apps}) {}; {talks}", l.p.username, role_words(r, ws)),
                vec![format!("{} then talks where the role {} says, at their next request; nothing else on the login changes", l.p.username, r.name)],
            )
        }
    };
    Ok(Some(respond(file(ws, session, &app, route, body, change, more, (l.p.username.as_str(), l.outside.as_slice()), text(v, &["reason"])))))
}

/// `k2 app user add <u> --app <its app> --role <box-wide role naming another
/// workspace's room>`.
fn add_held(ws: &str, session: Option<&str>, v: &J) -> R<Option<CliResponse>> {
    let (Some(raw), Some(app_raw), Some(token)) = (text(v, &["username", "name"]), text(v, &["app", "appId", "app_id"]), text(v, &["role", "roleId", "role_id"])) else {
        return Ok(None);
    };
    let Ok(app) = crate::skin_routes::resolve_app_target(app_raw) else { return Ok(None) };
    if service_of(&app, ws).is_none() {
        return Ok(None);
    }
    let Some(role) = skin::find_role(token).map_err(store_err)? else { return Ok(None) };
    if role.app_id.as_deref().is_some_and(|a| a != app) {
        return Ok(None);
    }
    let outside = outside_of(role_rooms(&role), ws);
    if outside.is_empty() {
        return Ok(None);
    }
    let username = skin::normalize_username(raw).map_err(|e| no(codes::BAD_REQUEST, 400, e))?;
    if let Some(r) = crate::skin_routes::reserved_name_refused(&username) {
        return Ok(Some(r));
    }
    if skin::principal_exists(&username).map_err(store_err)? {
        return Err(no(codes::BAD_REQUEST, 400, format!("skin user '{username}' already exists")));
    }
    let email = match v.get("email") {
        None | Some(J::Null) => None,
        Some(J::String(e)) if e.trim().is_empty() => None,
        Some(J::String(e)) => Some(skin::normalize_email(e).map_err(|e| no(codes::BAD_REQUEST, 400, e))?.unwrap_or_default()).filter(|e| !e.is_empty()),
        Some(_) => return Err(no(codes::BAD_REQUEST, 400, "invalid email")),
    };
    let password_wanted = text(v, &["password"]).is_some() || v.get("generate").and_then(J::as_bool) == Some(true);
    let handle = app_handle(&app, ws);
    let body = json!({
        "workspace": ws,
        "service": app,
        "app": handle,
        "username": username,
        "email": email,
        "role": role.id,
        "roleName": role.name,
        "outside": outside,
        "passwordWanted": password_wanted,
    });
    let mut more = vec![format!(
        "it signs in only to {handle} (no box login) and is made as {}'s login, like any login its agent adds; it talks in {} (a room outside {} the App can't hand out waits until the person chooses their own password)",
        ws_handle(ws),
        handles(&outside),
        ws_handle(ws)
    )];
    more.push(if password_wanted {
        format!("no password is on this card or kept: after approval the agent sets it once (k2 app user password {username} --password-stdin), and {username} chooses their own at the next sign-in")
    } else {
        "no password yet: they get one through an invite or a reset".to_string()
    });
    let held = file(
        ws,
        session,
        &app,
        ADD_APPLY,
        body,
        format!("add the login {username} to the App {handle} with {}", role_words(&role, ws)),
        more,
        (username.as_str(), outside.as_slice()),
        text(v, &["reason"]),
    );
    Ok(Some(respond(held.map(|mut out| {
        if password_wanted {
            out["passwordNotUsed"] = json!(true);
            out["afterApproval"] = json!(after_approval(&username));
        }
        out
    }))))
}

/// What the agent does after an approved password set or add.
fn after_approval(username: &str) -> String {
    format!(
        "After approval, set the password once within {} h: k2 app user password {username} --password-stdin. The password you sent was not used or kept.",
        PASSWORD_ONCE_SECS / 3600
    )
}

/// `k2 app user password <u> --password-stdin` on a login of its own App that
/// reaches another workspace's room: the one set an approval opened, else
/// one held request (never the password).
fn password_or_held(ws: &str, session: Option<&str>, v: &J, body: &[u8]) -> R<Option<CliResponse>> {
    let Some(username) = text(v, &["username", "name"]) else { return Ok(None) };
    let Some(l) = own_login(ws, username)? else { return Ok(None) };
    if l.outside.is_empty() {
        return Ok(None);
    }
    if let Some(id) = password_once_for(ws, &l.p.id)? {
        return Ok(Some(spend_once(ws, &id, body, &l)));
    }
    let app = l.apps[0].clone();
    let apps = apps_words(&l, ws);
    let req = json!({ "workspace": ws, "principal": l.p.id, "username": l.p.username, "apps": l.apps, "outside": l.outside });
    let held = file(
        ws,
        session,
        &app,
        PASSWORD_APPLY,
        req,
        format!("let the agent set a new password for the login {} (App {apps}; it talks in {}) once", l.p.username, handles(&l.outside)),
        vec![
            format!(
                "no password is on this card or kept: after approval the agent sets it once within {} h, and {} chooses their own at the next sign-in",
                PASSWORD_ONCE_SECS / 3600,
                l.p.username
            ),
            format!("the login is then tied to {}: its rooms outside it wait until {} chooses their own password", ws_handle(ws), l.p.username),
        ],
        (l.p.username.as_str(), l.outside.as_slice()),
        text(v, &["reason"]),
    );
    let given = text(v, &["password"]).is_some() || v.get("generate").and_then(J::as_bool) == Some(true);
    Ok(Some(respond(held.map(|mut out| {
        out["afterApproval"] = json!(after_approval(&l.p.username));
        if given {
            out["passwordNotUsed"] = json!(true);
        }
        out
    }))))
}

/// The approved request of `ws` that opened one password set for principal
/// `pid`, while it is unused and fresh.
fn password_once_for(ws: &str, pid: &str) -> R<Option<String>> {
    let rows = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        let f = reqs::Filter { app_id: None, service_id: None, workspace: None, status: Some(reqs::STATUS_APPROVED) };
        reqs::list(&conn, &f, 1000).map_err(store_err)?
    };
    let now = k2_core::deploy::now();
    Ok(rows
        .into_iter()
        .find(|r| {
            let route = r.body.get("route").and_then(J::as_str);
            let opens = match route {
                Some(PASSWORD_APPLY) => true,
                Some(ADD_APPLY) => r.body.pointer("/body/passwordWanted").and_then(J::as_bool) == Some(true),
                _ => false,
            };
            r.requester_workspace.as_deref() == Some(ws)
                && opens
                && r.result.as_ref().and_then(|x| x.get("principal")).and_then(J::as_str) == Some(pid)
                && r.result.as_ref().and_then(|x| x.get("passwordSet")).and_then(J::as_bool) != Some(true)
                && r.decided_at.is_some_and(|at| now - at <= PASSWORD_ONCE_SECS)
        })
        .map(|r| r.id))
}

/// Spend the one set of request `id` on this call's password (single use,
/// before the write; given back when the write fails).
fn spend_once(ws: &str, id: &str, body: &[u8], l: &OwnLogin) -> CliResponse {
    let won = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute(
            "UPDATE app_widenings SET result_json = json_set(COALESCE(result_json, '{}'), '$.passwordSet', json('true')) \
             WHERE id = ?1 AND COALESCE(json_extract(result_json, '$.passwordSet'), 0) = 0",
            rusqlite::params![id],
        )
    };
    match won {
        Ok(1) => {}
        Ok(_) => return super::routes::refusal_response(&no(codes::OWNER_ONLY, 403, format!("the one password set for {} is used: ask again", l.p.username))),
        Err(e) => return super::routes::refusal_response(&store_err(e)),
    }
    let actor = format!("agent:{}", ws_handle(ws));
    let r = agent_password_once(ws, body, &actor);
    if !r.status.starts_with("200") {
        let h = k2_core::db::shared();
        let conn = h.lock();
        if let Err(e) = conn.execute("UPDATE app_widenings SET result_json = json_remove(result_json, '$.passwordSet') WHERE id = ?1", rusqlite::params![id]) {
            k2_core::log_debug!("[apps] give back the password set of {id}: {e}");
        }
        return r;
    }
    let mut v: J = serde_json::from_str(&r.body).unwrap_or(J::Null);
    if v.is_object() {
        v["once"] = json!(true);
        v["approvedRequest"] = json!(id);
    }
    CliResponse::ok_json(v.to_string())
}

// ── the approval ────────────────────────────────────────────────────

fn failed(r: &CliResponse) -> super::routes::Refusal {
    let (code, hint) = error_of(r);
    let v: J = serde_json::from_str(&r.body).unwrap_or(J::Null);
    let hint = if hint.is_empty() { v.get("error").and_then(J::as_str).unwrap_or("the change failed").to_string() } else { hint };
    no("user_change_failed", 409, if code.is_empty() { hint } else { format!("{code}: {hint}") })
}

/// The login a held request named, still that one (by id) and still only
/// the requesting workspace's Apps'.
fn still_own(b: &J, ws: &str) -> R<OwnLogin> {
    let (Some(pid), Some(username)) = (b["principal"].as_str(), b["username"].as_str()) else {
        return Err(no(codes::BAD_REQUEST, 400, "unreadable held request"));
    };
    match own_login(ws, username)? {
        Some(l) if l.p.id == pid => Ok(l),
        _ => Err(no(
            codes::FORBIDDEN,
            409,
            format!("{username} isn't the same login, or isn't only {}'s Apps' any more: nothing changed; ask again", ws_handle(ws)),
        )),
    }
}

/// Carry out an approved request whose route is one of [`APPLY_ROUTES`], as
/// the owner or admin `by` who approved it (`approvals::apply_door`). `None`:
/// not ours.
pub(crate) fn apply(r: &reqs::AppRequest, by: &str) -> Option<R<J>> {
    let route = r.body.get("route").and_then(J::as_str)?;
    if !APPLY_ROUTES.contains(&route) {
        return None;
    }
    let b = r.body.get("body").cloned().unwrap_or(J::Null);
    let ws = b["workspace"].as_str().unwrap_or_default().to_string();
    Some((|| -> R<J> {
        if ws.is_empty() || r.requester_workspace.as_deref() != Some(ws.as_str()) {
            return Err(no(codes::BAD_REQUEST, 400, "unreadable held request"));
        }
        // As the person who approved: audited as them (`skin_routes::agent_scoped`
        // with no agent workspace is the owner's own write).
        let run = |path: &str, line: J, handler: fn(&[u8], &str) -> CliResponse| -> R<J> {
            let resp = crate::skin_routes::agent_scoped(None, path, line.to_string().as_bytes(), by, handler);
            if !resp.status.starts_with('2') {
                return Err(failed(&resp));
            }
            Ok(serde_json::from_str(&resp.body).unwrap_or(J::Null))
        };
        match route {
            REMOVE_APPLY => {
                let l = still_own(&b, &ws)?;
                let answer = run(REMOVE, json!({ "username": l.p.username }), crate::skin_routes::handle_users_remove)?;
                Ok(json!({ "applied": route, "username": l.p.username, "removed": true, "answer": answer }))
            }
            ROLE_APPLY => {
                let l = still_own(&b, &ws)?;
                let role_id = b["role"].as_str().ok_or_else(|| no(codes::BAD_REQUEST, 400, "unreadable held request"))?;
                match role_for(&l, role_id)? {
                    Some(role) if role.id == role_id => {}
                    _ => return Err(no(codes::FORBIDDEN, 409, "the role in this request is gone or changed App: nothing changed; ask again")),
                }
                let answer = run(ASSIGN, json!({ "username": l.p.username, "role": role_id }), crate::skin_routes::handle_roles_assign)?;
                Ok(json!({ "applied": route, "username": l.p.username, "role": b["roleName"], "answer": answer }))
            }
            RESET_APPLY => {
                let l = still_own(&b, &ws)?;
                let app = b["service"].as_str().filter(|a| l.apps.iter().any(|x| x == *a)).ok_or_else(|| {
                    no(codes::FORBIDDEN, 409, format!("{} isn't on that App any more: nothing sent; ask again", l.p.username))
                })?;
                match crate::app_user_invites::app_key_reset(app, &l.p.username, by) {
                    Ok(out) => Ok(json!({ "applied": route, "username": l.p.username, "app": app, "answer": out })),
                    Err((status, code, hint)) => Err(no(code, status, hint)),
                }
            }
            PASSWORD_APPLY => {
                let l = still_own(&b, &ws)?;
                Ok(json!({
                    "applied": route,
                    "username": l.p.username,
                    "principal": l.p.id,
                    "note": format!("the agent may set {}'s password once within {} h", l.p.username, PASSWORD_ONCE_SECS / 3600),
                }))
            }
            ADD_APPLY => {
                let svc = b["service"].as_str().ok_or_else(|| no(codes::BAD_REQUEST, 400, "unreadable held request"))?;
                if service_of(svc, &ws).is_none() {
                    return Err(no(codes::FORBIDDEN, 409, "the App in this request isn't the requesting workspace's any more: nothing added; ask again"));
                }
                let role_id = b["role"].as_str().ok_or_else(|| no(codes::BAD_REQUEST, 400, "unreadable held request"))?;
                match skin::find_role(role_id).map_err(store_err)? {
                    Some(role) if role.id == role_id && role.app_id.as_deref().is_none_or(|a| a == svc) => {}
                    _ => return Err(no(codes::FORBIDDEN, 409, "the role in this request is gone or changed App: nothing added; ask again")),
                }
                let resp = approved_agent_add(&ws, &b, by);
                if !resp.status.starts_with('2') {
                    return Err(failed(&resp));
                }
                let answer: J = serde_json::from_str(&resp.body).unwrap_or(J::Null);
                let mut out = json!({ "applied": route, "username": b["username"], "principal": answer["id"], "answer": answer });
                if b["passwordWanted"].as_bool() == Some(true) {
                    out["note"] = json!(format!(
                        "the agent may set {}'s password once within {} h",
                        b["username"].as_str().unwrap_or_default(),
                        PASSWORD_ONCE_SECS / 3600
                    ));
                }
                Ok(out)
            }
            _ => Err(no(codes::BAD_REQUEST, 400, format!("'{route}' isn't an App user change"))),
        }
    })())
}

// ── the writes an approval opens (skin_routes' own helpers) ──────────

/// An agent's held add an owner or admin `by` approved (0.46.14): the login on App `b.service` with role `b.role` (both checked
/// again by the caller), made as the agent's login of `ws` (its creator stamp
/// and tie, like any login it adds), never with a password. Audited like the
/// agent's own add, as `by`.
pub(crate) fn approved_agent_add(ws: &str, b: &serde_json::Value, by: &str) -> CliResponse {
    let (Some(username), Some(app), Some(role)) = (b["username"].as_str(), b["service"].as_str(), b["role"].as_str()) else {
        return CliResponse::bad_request("unreadable held request");
    };
    if let Some(r) = crate::skin_routes::reserved_name_refused(username) {
        return r;
    }
    let email = b["email"].as_str().filter(|e| !e.trim().is_empty());
    let line = serde_json::json!({ "username": username, "app": app, "role": role, "email": email });
    let r = match skin::add_principal_for_app_by(username, email, app, Some(role), by, skin::AddedBy { agent_workspace: Some(ws), ..Default::default() }) {
        Ok(p) => {
            k2_core::log_debug!("[skin] actor={by} added principal {} (approved for {ws})", p.username);
            crate::skin_routes::with_waiting(CliResponse::ok_json(serde_json::to_string(&p).unwrap_or_else(|_| "{}".into())), line.to_string().as_bytes())
        }
        Err(e) => CliResponse::bad_request(e),
    };
    crate::skin_routes::audit_write("/cli/skin/users", line.to_string().as_bytes(), by, Some(ws), Vec::new(), &r);
    r
}

/// The one password set an approval opened for the agent of `ws`
/// (0.46.14): the agent's own set (`users_password` with its
/// workspace: the login is tied to it and asks the person for their own
/// password at the next sign-in), audited like the agent's own.
pub(crate) fn agent_password_once(ws: &str, body: &[u8], actor: &str) -> CliResponse {
    let path = "/cli/skin/users/password";
    let before = crate::skin_routes::write_apps(path, body, Some(ws));
    let r = skin::guest_rooms::written_by(skin::guest_rooms::WrittenBy::Agent(actor.to_string()), || crate::skin_routes::users_password(body, actor, Some(ws)));
    crate::skin_routes::audit_write(path, body, actor, Some(ws), before, &r);
    r
}

// ── a box login, with permission cards off ──────────────────────────

/// Where the owner turns permission cards on for an agent (both switches).
pub const PERMISSION_CARDS_ON_PATH: &str =
    "Settings → Permission Cards → \"Let agents ask with permission cards\", then this agent under \"Agents that can ask\"";

/// The hint when an agent (of `ws`, when known) is refused on a guest that holds
/// a box login. With permission cards on for that agent it names the card to
/// ask with; off, `k2 permission` is an unknown command for it, so it says what
/// the owner can do instead (0.46.14).
pub(crate) fn box_login_hint(ws: Option<&str>, username: &str) -> String {
    let next = if ws.is_some_and(k2_core::permission::gate::enabled_for) {
        format!("To give them access to your App, ask the owner on a card: k2 permission request apps.user-grant-box user={username} app=<your app> role=<role> --why \"<what you need it for>\"")
    } else {
        box_login_grant_by_owner(username)
    };
    format!("'{username}' has a box login: an agent never manages a box login, whatever its switches (ask your human; App users are App-only). {next}")
}

/// With permission cards off: how the owner gives box login `username` access
/// to an App themselves, or turns cards on so the agent can ask.
pub(crate) fn box_login_grant_by_owner(username: &str) -> String {
    format!(
        "To give {username} access to your App, ask your human to do it: Settings → Apps → <your App> → Add a person (the server's logins are listed there too), or k2 app grant add \
         --subject-kind principal --subject <{username}'s principal id> --kind app --target <your app> --role <role> from a terminal outside K2. \
         Or they turn on permission cards ({PERMISSION_CARDS_ON_PATH}) and you ask on a card."
    )
}
