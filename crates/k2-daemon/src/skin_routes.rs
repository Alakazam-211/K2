//! Skin roster / passes / front-door (`/cli/skin*`).
//!
//! GET `/cli/skin/users` and GET `/cli/skin-tokens` admit owner-tier
//! (`owner_role_identity`) **or** a workspace-agent scoped hook. Manage
//! mutations admit owner-tier **or** a scoped hook when that workspace's
//! Agent-tab `agents_can_manage_skin` column is ON. Leftover front-door /
//! Hydra stay owner-only. Grants (`/cli/skin/grants`), grant enabled /
//! host login, and access templates are owner-only and are not agent
//! verbs. Mutations are POST-only (GET twins 405). Raw `k2skn_…` secret
//! is returned once on create.

use std::collections::{HashMap, HashSet};

use crate::cli_response::CliResponse;
use k2_core::skin::{
    self, RoomPolicy, CAP_ACTIVITY_READ, CAP_FILES_READ, CAP_FILES_WRITE, CAP_STORE_READ,
    CAP_STORE_WRITE, CAP_THREAD_POST, CAP_THREAD_READ, CAP_TICKETS_POST, CAP_TICKETS_READ,
    CAP_WIKI_READ,
};
use k2_core::skin_door;

fn json_body(body: &[u8]) -> Result<serde_json::Value, CliResponse> {
    match serde_json::from_slice(body) {
        Ok(v) => Ok(v),
        Err(_) if body.iter().all(|b| b.is_ascii_whitespace()) => Ok(serde_json::json!({})),
        Err(e) => Err(CliResponse::bad_request(format!("invalid JSON body: {e}"))),
    }
}

fn str_field<'a>(v: &'a serde_json::Value, keys: &[&str]) -> Option<&'a str> {
    for k in keys {
        if let Some(s) = v.get(*k).and_then(|x| x.as_str()) {
            let t = s.trim();
            if !t.is_empty() {
                return Some(t);
            }
        }
    }
    None
}

fn caps_field(v: &serde_json::Value) -> Option<Vec<String>> {
    v.get("caps")
        .or_else(|| v.get("capabilities"))
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
}

fn rooms_field(v: &serde_json::Value) -> Option<Vec<String>> {
    v.get("rooms")
        .or_else(|| v.get("agents"))
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
}

fn apply_tokens_field(v: &serde_json::Value) -> bool {
    match v
        .get("applyTokens")
        .or_else(|| v.get("apply_tokens"))
        .or_else(|| v.get("apply-tokens"))
    {
        None => false,
        Some(x) if x.is_null() => false,
        Some(x) => x.as_bool().unwrap_or(false),
    }
}

fn resolve_rooms_or_400(raw: Option<Vec<String>>) -> Result<Vec<String>, CliResponse> {
    let Some(list) = raw else {
        return Err(CliResponse::bad_request(
            "rooms must include at least one workspace",
        ));
    };
    match skin::resolve_room_tokens(&list) {
        Ok(ids) => Ok(ids),
        Err(e) => Err(CliResponse::bad_request(e)),
    }
}

/// Roles may be Thread-dark (`[]`). Unknown handles still 400.
fn resolve_rooms_allow_empty(raw: Option<Vec<String>>) -> Result<Vec<String>, CliResponse> {
    match skin::resolve_room_tokens(&raw.unwrap_or_default()) {
        Ok(ids) => Ok(ids),
        Err(e) => Err(CliResponse::bad_request(e)),
    }
}

const ROOM_ACCESS_TEACHING: &str = "use roomAccess (per-room functions), not caps+rooms";

fn room_access_value(v: &serde_json::Value) -> Option<&serde_json::Value> {
    v.get("roomAccess").or_else(|| v.get("room_access"))
}

fn parse_room_access(v: &serde_json::Value) -> Result<RoomPolicy, CliResponse> {
    let Some(arr) = v.as_array() else {
        return Err(CliResponse::bad_request("roomAccess must be an array"));
    };
    let mut seen: HashSet<String> = HashSet::new();
    let mut policy = RoomPolicy::new();
    for item in arr {
        let Some(handle) = str_field(item, &["handle", "id"]) else {
            return Err(CliResponse::bad_request(
                "roomAccess item missing handle or id",
            ));
        };
        let key = handle.to_ascii_lowercase();
        if !seen.insert(key) {
            return Err(CliResponse::bad_request(format!(
                "duplicate handle {handle:?}"
            )));
        }
        let ids = match skin::resolve_room_tokens(&[handle.to_string()]) {
            Ok(ids) if ids.len() == 1 => ids,
            Ok(_) => {
                return Err(CliResponse::bad_request(format!(
                    "unknown workspace handle {handle:?}"
                )))
            }
            Err(e) => return Err(CliResponse::bad_request(e)),
        };
        let project_id = ids[0].clone();
        if policy.contains_key(&project_id) {
            return Err(CliResponse::bad_request(format!(
                "duplicate handle {handle:?}"
            )));
        }
        let caps = match skin::parse_caps(caps_field(item).as_deref()) {
            Ok(c) => c,
            Err(e) => return Err(CliResponse::bad_request(e)),
        };
        policy.insert(project_id, caps);
    }
    Ok(policy)
}

/// Create: missing both → empty map. Update: missing both → `None` (keep).
fn role_policy_from_body(
    v: &serde_json::Value,
    is_update: bool,
) -> Result<Option<RoomPolicy>, CliResponse> {
    if let Some(ra) = room_access_value(v) {
        return Ok(Some(parse_room_access(ra)?));
    }
    let caps = caps_field(v);
    let rooms = rooms_field(v);
    if caps.is_some() && rooms.is_some() {
        return Err(CliResponse::bad_request(ROOM_ACCESS_TEACHING.to_string()));
    }
    if caps.is_some() && rooms.is_none() {
        return Err(CliResponse::bad_request(
            "caps require rooms or roomAccess (per-room functions)",
        ));
    }
    if let Some(list) = rooms {
        let ids = resolve_rooms_allow_empty(Some(list))?;
        return Ok(Some(skin::thread_only_room_policy(&ids)));
    }
    if is_update {
        Ok(None)
    } else {
        Ok(Some(RoomPolicy::new()))
    }
}

fn apply_field(v: &serde_json::Value) -> bool {
    match v.get("apply").or_else(|| v.get("Apply")) {
        None => true,
        Some(x) if x.is_null() => true,
        Some(x) => x.as_bool().unwrap_or(true),
    }
}

fn ui_port_field(v: &serde_json::Value, current: Option<u16>) -> Option<u16> {
    let raw = v.get("uiPort").or_else(|| v.get("ui_port"));
    match raw {
        None => current,
        Some(x) if x.is_null() => None,
        Some(x) => {
            if let Some(n) = x.as_u64() {
                u16::try_from(n).ok()
            } else if let Some(s) = x.as_str() {
                let t = s.trim();
                if t.is_empty() {
                    None
                } else {
                    t.parse().ok()
                }
            } else {
                current
            }
        }
    }
}

fn status_json() -> CliResponse {
    match skin_door::status() {
        Ok(d) => CliResponse::ok_json(serde_json::to_string(&d).unwrap_or_else(|_| "{}".into())),
        Err(e) => CliResponse::internal_error(e),
    }
}

pub fn handle_users_get() -> CliResponse {
    match skin::list_principals() {
        Ok(users) => {
            // 0.46.4: a test guest says so (`test`: its App, its end).
            let tests: HashMap<String, skin::test_guests::TestGuest> =
                skin::test_guests::list(None).unwrap_or_default().into_iter().map(|t| (t.principal_id.clone(), t)).collect();
            let users: Vec<serde_json::Value> = users
                .iter()
                .map(|u| {
                    let mut v = serde_json::to_value(u).unwrap_or(serde_json::Value::Null);
                    if let Some(t) = tests.get(&u.id) {
                        let app_id = {
                            let db = k2_core::db::shared();
                            let conn = db.lock();
                            k2_core::deploy::store::get_app_by_service(&conn, &t.app).ok().flatten().map(|a| a.app_id)
                        };
                        v["test"] = serde_json::json!({
                            "app": app_id,
                            "appServiceId": t.app,
                            "expiresAt": t.expires_at,
                            "expired": t.expired_at(k2_core::deploy::now()),
                            "createdBy": t.created_by,
                        });
                    }
                    v
                })
                .collect();
            CliResponse::ok_json(serde_json::json!({ "users": users }).to_string())
        }
        Err(e) => CliResponse::internal_error(e),
    }
}

fn email_field(v: &serde_json::Value) -> Result<Option<&str>, CliResponse> {
    match v.get("email") {
        None => Ok(None),
        Some(x) if x.is_null() => Ok(None),
        Some(x) => match x.as_str() {
            Some(s) => Ok(Some(s)),
            None => Err(CliResponse::bad_request("invalid email")),
        },
    }
}

/// Q5 default (S24-12): an agent with "manage Apps" keeps exactly its
/// 0.45.4 powers in this slice — it may add box-login guests as before, but
/// naming an `app` (an App-only guest, or App rooms) is owner-only. Flip to
/// `true` to let agents do it for their own workspace's Apps with that
/// App's roles (the vs-live recommendation, not built here).
pub const AGENTS_MAY_NAME_APP: bool = false;

/// Owner-tier actors are `owner-token` / `user:<name>`; a scoped hook that
/// passed the manage gate is `agent:<handle>` ([`skin_manage_actor_label`]).
pub fn actor_is_agent(actor: &str) -> bool {
    actor.starts_with("agent:")
}

/// The principal named `username` (normalized), if there is one.
/// Resolved as login and unlock resolve a name (an email works too), so a
/// guest named by its email is checked like one named by its username
/// (security review C).
fn principal_named(username: &str) -> Result<Option<skin::SkinPrincipal>, CliResponse> {
    let Some(u) = skin::login_principal_username(username).map_err(CliResponse::internal_error)? else { return Ok(None) };
    skin::list_principals().map(|ps| ps.into_iter().find(|p| p.username == u)).map_err(CliResponse::internal_error)
}

/// The routes [`agent_scoped`] guards: a guest's password, email, rooms,
/// lockout, removal and role.
pub const AGENT_SCOPED_ROUTES: &[&str] = &[
    "/cli/skin/users/password",
    "/cli/skin/users/email",
    "/cli/skin/users/rooms",
    "/cli/skin/users/unlock",
    "/cli/skin/users/remove",
    "/cli/skin/roles/assign",
    "/cli/skin/roles/unassign",
];

/// The platform-pass writes [`agent_scoped`] guards: mint, rooms, revoke.
pub const AGENT_SCOPED_PASS_ROUTES: &[&str] = &["/cli/skin-tokens", "/cli/skin-tokens/rooms", "/cli/skin-tokens/revoke"];

/// The role writes [`agent_scoped`] guards: making, editing (its room
/// lines and their folders) and removing a role.
pub const AGENT_SCOPED_ROLE_ROUTES: &[&str] = &[
    "/cli/skin/roles",
    "/cli/skin/roles/update",
    "/cli/skin/roles/room",
    "/cli/skin/roles/paths",
    "/cli/skin/roles/remove",
    "/cli/skin/roles/from-template",
];

/// 0.46.4 security review (the older hole): the skin-manage gate passes a
/// switch-1 agent for every guest on the box, other workspaces' too. On
/// [`AGENT_SCOPED_ROUTES`] an agent (`agent_ws` = its workspace, from the
/// gate) acts only on a guest wholly inside its own workspace
/// ([`principal_in_workspace`]), and hands it only rooms or a role inside
/// that workspace; anything else is `owner_only` and touches nothing. The
/// owner, Owner logins and shell tabs (`agent_ws` = `None`) are unchanged.
/// An unknown guest falls through to the handler's own answer.
pub fn agent_scoped(
    agent_ws: Option<&str>,
    path: &str,
    body: &[u8],
    actor: &str,
    handler: fn(&[u8], &str) -> CliResponse,
) -> CliResponse {
    if let Some(ws) = agent_ws {
        let checked = if AGENT_SCOPED_ROLE_ROUTES.contains(&path) {
            agent_may_write_role(ws, path, body)
        } else if AGENT_SCOPED_PASS_ROUTES.contains(&path) {
            agent_may_write_pass(ws, path, body)
        } else {
            agent_may_touch(ws, path, body)
        };
        if let Err(r) = checked {
            k2_core::log_debug!("[skin] actor={actor} refused outside its workspace on {path}");
            return r;
        }
        // Round-2 review B1: an agent that sets a guest's password or email
        // could sign in as it, so the guest is tied to the agent's
        // workspace (and any person claim dropped) BEFORE the change: it
        // can't answer cards until a person takes it over again.
        if matches!(path, "/cli/skin/users/password" | "/cli/skin/users/email") {
            if let Err(r) = tie_named_guest(ws, body) {
                return r;
            }
        }
    }
    handler(body, actor)
}

/// Tie the guest a password or email body names to the agent's workspace
/// `ws` ([`skin::creators::tie`]). An unreadable body or an unknown guest
/// falls through to the handler's own answer (nothing to tie).
fn tie_named_guest(ws: &str, body: &[u8]) -> Result<(), CliResponse> {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) else { return Ok(()) };
    let Some(username) = str_field(&v, &["username", "name"]) else { return Ok(()) };
    let Some(p) = principal_named(username)? else { return Ok(()) };
    skin::creators::tie(&p.id, ws).map_err(CliResponse::internal_error)
}

fn outside_workspace(what: String) -> CliResponse {
    owner_only_with_hint(&format!("{what}: only the owner changes guests and roles outside your workspace (ask your human)"))
}

fn agent_may_touch(ws: &str, path: &str, body: &[u8]) -> Result<(), CliResponse> {
    // A body the handler can't read is its 400, not ours.
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) else { return Ok(()) };
    let Some(username) = str_field(&v, &["username", "name"]) else { return Ok(()) };
    let Some(p) = principal_named(username)? else {
        // Unlock clears lockout rows by the name given: a name that is no
        // guest is never an agent's to clear.
        if path == "/cli/skin/users/unlock" {
            return Err(outside_workspace(format!("'{username}' is no guest of your workspace")));
        }
        return Ok(());
    };
    let roles = skin::list_roles().map_err(CliResponse::internal_error)?;
    let grants = skin::list_grants().map_err(CliResponse::internal_error)?;
    if !principal_in_workspace(&p, &roles, &grants, ws) {
        return Err(outside_workspace(format!(
            "'{}' isn't your workspace's guest: it reaches outside your workspace (another workspace's App or room, a box-wide grant) or it is the owner's",
            p.username
        )));
    }
    match path {
        "/cli/skin/roles/assign" => {
            if let Some(token) = str_field(&v, &["role", "name", "id"]) {
                // The handler's own lookup (id first, then name), so a role
                // named like another role's id is never the one checked.
                if let Some(r) = skin::find_role(token).map_err(CliResponse::internal_error)? {
                    if !role_in_workspace(&r, ws) {
                        return Err(outside_workspace(format!("role '{}' reaches outside your workspace", r.name)));
                    }
                }
            }
        }
        "/cli/skin/users/rooms" => {
            if let Some(list) = rooms_field(&v) {
                if let Ok(ids) = skin::resolve_room_tokens(&list) {
                    if ids.iter().any(|id| id != ws) {
                        return Err(outside_workspace("those rooms are outside your workspace".to_string()));
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// The role writes (security review, 0.46.4): an agent makes or edits
/// only roles whose rooms are all in its own workspace (and, for an App's
/// role, an App of it). Making one with a room elsewhere, giving a role a
/// room elsewhere, or editing or removing a role that already reaches
/// elsewhere is `owner_only`. What the handler can't read is its own 400.
fn agent_may_write_role(ws: &str, path: &str, body: &[u8]) -> Result<(), CliResponse> {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) else { return Ok(()) };
    let existing = |keys: &[&str]| -> Result<(), CliResponse> {
        let Some(token) = str_field(&v, keys) else { return Ok(()) };
        // The handlers' own lookup (id first, then name): a decoy role named
        // like another role's id never stands in for it.
        match skin::find_role(token).map_err(CliResponse::internal_error)? {
            Some(r) if !role_in_workspace(&r, ws) => {
                Err(outside_workspace(format!("role '{}' reaches outside your workspace (another workspace's room or App)", r.name)))
            }
            _ => Ok(()),
        }
    };
    let rooms_ok = |ids: &[String]| -> Result<(), CliResponse> {
        if rooms_in(ids, ws) {
            Ok(())
        } else {
            Err(outside_workspace("a role of yours may name only your workspace's room".to_string()))
        }
    };
    let room_token_ok = |keys: &[&str]| -> Result<(), CliResponse> {
        let Some(token) = str_field(&v, keys) else { return Ok(()) };
        match skin::resolve_room_tokens(&[token.to_string()]) {
            Ok(ids) => rooms_ok(&ids),
            Err(_) => Ok(()),
        }
    };
    let app_ok = |raw: Option<&str>| -> Result<(), CliResponse> {
        match raw.map(resolve_app_target) {
            Some(Ok(a)) if a != skin::HOST_APP_ID && !app_of_workspace(&a, ws) => {
                Err(outside_workspace(format!("'{a}' isn't an App of your workspace")))
            }
            _ => Ok(()),
        }
    };
    match path {
        "/cli/skin/roles" => {
            if let Ok(Some(p)) = role_policy_from_body(&v, false) {
                rooms_ok(&p.keys().cloned().collect::<Vec<_>>())?;
            }
            app_ok(opt_text_field(&v, &["appId", "app_id", "app"]))?;
        }
        "/cli/skin/roles/update" => {
            existing(&["id", "name", "role"])?;
            if let Ok(Some(p)) = role_policy_from_body(&v, true) {
                rooms_ok(&p.keys().cloned().collect::<Vec<_>>())?;
            }
        }
        "/cli/skin/roles/room" => {
            existing(&["id", "name", "role"])?;
            room_token_ok(&["handle", "id"])?;
        }
        "/cli/skin/roles/paths" => {
            existing(&["name", "role", "id"])?;
            room_token_ok(&["handle", "room", "workspace"])?;
        }
        "/cli/skin/roles/remove" => existing(&["id", "name", "role"])?,
        "/cli/skin/roles/from-template" => {
            let app = str_field(&v, &["app", "appId"]);
            app_ok(app)?;
            let tokens: Vec<String> = v
                .get("rooms")
                .and_then(|x| x.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str()).map(str::to_string).collect())
                .unwrap_or_default();
            if !tokens.is_empty() {
                if let Ok(ids) = skin::resolve_room_tokens(&tokens) {
                    rooms_ok(&ids)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// The platform-pass writes (security review, 0.46.4): an agent mints a
/// pass only for its own workspace's rooms, and changes the rooms of, or
/// revokes, only a pass whose rooms (and App) are all its workspace's. A
/// guest's sign-in session is not a platform pass: an agent never changes
/// its rooms (they come from the guest's grants) and revokes it only for a
/// guest wholly inside its workspace. Anything else is `owner_only`,
/// nothing written. An unknown id is the handler's own answer.
fn agent_may_write_pass(ws: &str, path: &str, body: &[u8]) -> Result<(), CliResponse> {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) else { return Ok(()) };
    let new_rooms_ok = || -> Result<(), CliResponse> {
        match rooms_field(&v).map(|list| skin::resolve_room_tokens(&list)) {
            Some(Ok(ids)) if !rooms_in(&ids, ws) => {
                Err(outside_workspace("a platform pass of yours may name only your workspace's room".to_string()))
            }
            _ => Ok(()),
        }
    };
    if path == "/cli/skin-tokens" {
        return new_rooms_ok();
    }
    let Some(id) = str_field(&v, &["id", "tokenId", "token_id"]) else { return Ok(()) };
    let Some(t) = skin::token_scope(id).map_err(CliResponse::internal_error)? else { return Ok(()) };
    if t.session {
        if path == "/cli/skin-tokens/rooms" {
            return Err(outside_workspace("that is a guest's sign-in, not a platform pass (its rooms come from the guest's grants)".to_string()));
        }
        let inside = match t.principal_id.as_deref() {
            Some(pid) => {
                let roles = skin::list_roles().map_err(CliResponse::internal_error)?;
                let grants = skin::list_grants().map_err(CliResponse::internal_error)?;
                skin::list_principals()
                    .map_err(CliResponse::internal_error)?
                    .into_iter()
                    .find(|p| p.id == pid)
                    .is_some_and(|p| principal_in_workspace(&p, &roles, &grants, ws))
            }
            None => false,
        };
        if !inside {
            return Err(outside_workspace("that sign-in belongs to a guest outside your workspace".to_string()));
        }
    }
    // A pass whose rooms are empty or can't be read says nothing about
    // where it reaches: never an agent's (hardening).
    if !t.session && (!t.rooms_readable || t.rooms.is_empty()) {
        return Err(outside_workspace("that pass names no room an agent can check (none, or unreadable)".to_string()));
    }
    if t.app_service_id.as_deref().is_some_and(|app| !app_of_workspace(app, ws)) || !rooms_in(&t.rooms, ws) {
        return Err(outside_workspace("that pass reaches outside your workspace (another workspace's room or App)".to_string()));
    }
    // A pass holding a person-only cap: the handler refuses any change of
    // its rooms itself, naming the cap.
    if path == "/cli/skin-tokens/rooms" && person_only_cap(Some(&t.caps)).is_none() {
        new_rooms_ok()?;
    }
    Ok(())
}

/// Every room in `rooms` is `ws`.
fn rooms_in(rooms: &[String], ws: &str) -> bool {
    rooms.iter().all(|r| r == ws)
}

/// A role whose rooms are all `ws`, and, for an App's role, an App of `ws`.
fn role_in_workspace(r: &skin::SkinRole, ws: &str) -> bool {
    rooms_in(&r.rooms, ws) && r.room_policy.keys().all(|k| k == ws) && r.app_id.as_deref().map_or(true, |app| app_of_workspace(app, ws))
}

/// The published service `id` belongs to workspace `ws`.
fn app_of_workspace(id: &str, ws: &str) -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    matches!(k2_core::published_services::get_by_id(&conn, id), Ok(Some(svc)) if svc.project_id == ws)
}

/// Is guest `p` wholly inside workspace `ws`? Its rooms, its role, and
/// every grant it holds: the box login (`app=host`) or an App of `ws`,
/// each with rooms and a role inside `ws`. Any other grant (chat,
/// mailbox, database, cli: box-wide) or an agent mark for another
/// workspace puts it outside. It must also be tied to `ws`: a room, a
/// role room or an App of `ws`, the agent mark, or the stamp of `ws`'s
/// agent having added it (`skin::creators`). A guest with nothing yet is
/// inside only when this workspace's agent added it; the owner's empty
/// guests stay the owner's (security review B).
pub(crate) fn principal_in_workspace(p: &skin::SkinPrincipal, roles: &[skin::SkinRole], grants: &[skin::SkinGrant], ws: &str) -> bool {
    let role = |id: &Option<String>| id.as_ref().map(|id| roles.iter().find(|r| &r.id == id));
    let role_ok = |id: &Option<String>| match role(id) {
        None => true,
        Some(found) => found.is_some_and(|r| role_in_workspace(r, ws)),
    };
    let role_ties = |id: &Option<String>| matches!(role(id), Some(Some(r)) if !r.rooms.is_empty() || r.app_id.is_some());
    if p.agent_workspace.as_deref().is_some_and(|a| a != ws) || !rooms_in(&p.default_rooms, ws) || !role_ok(&p.role_id) {
        return false;
    }
    let mut tied = p.agent_workspace.as_deref() == Some(ws) || !p.default_rooms.is_empty() || role_ties(&p.role_id);
    for g in grants.iter().filter(|g| g.subject_kind == "principal" && g.subject_id == p.id) {
        let own_app = g.kind == "app" && g.target_id != skin::HOST_APP_ID && app_of_workspace(&g.target_id, ws);
        let target_ok = g.kind == "app" && (g.target_id == skin::HOST_APP_ID || own_app);
        let scope = match g.scope.as_deref().map(str::trim) {
            None | Some("") => Some(Vec::new()),
            Some(raw) => serde_json::from_str::<Vec<String>>(raw).ok(),
        };
        let scope_ok = scope.as_ref().is_some_and(|rooms| rooms_in(rooms, ws));
        if !(target_ok && scope_ok && role_ok(&g.role_id)) {
            return false;
        }
        tied |= own_app || scope.is_some_and(|rooms| !rooms.is_empty()) || role_ties(&g.role_id);
    }
    tied || skin::creators::workspace_of(&p.id).ok().flatten().as_deref() == Some(ws)
}

/// 0.46.4 security review: a test guest is the owner's while agent test
/// guests are parked (`apps::test_guests::AGENTS_MAY_MANAGE`). An agent
/// that passed the skin-manage gate may not reset its password, change
/// its email, rooms or role, unlock it or remove it: otherwise it could
/// sign in as an owner-made test guest through the App's link and use
/// that guest's role. `None` = go on.
fn agent_test_guest_refused(actor: &str, username: &str) -> Option<CliResponse> {
    if !actor_is_agent(actor) || crate::apps::test_guests::AGENTS_MAY_MANAGE {
        return None;
    }
    match principal_named(username) {
        Ok(Some(p)) => match skin::test_guests::get(&p.id) {
            Ok(Some(_)) => Some(owner_only_with_hint(&format!(
                "'{}' is a test guest: test guests are the owner's (ask your human: k2 app user … {} --test from a terminal outside K2)",
                p.username, p.username
            ))),
            Ok(None) => None,
            Err(e) => Some(CliResponse::internal_error(e)),
        },
        Ok(None) => None,
        Err(r) => Some(r),
    }
}

/// GH#72 (Rosson 2026-10-14): [`skin::CAP_FILES_AGENT_CONFIG`] — letting
/// App guests write the agent's own `.k2/` folder — is owner tooling. An
/// agent with "manage Apps" may not add it to a role's room, mint it on a
/// platform pass, widen a pass that holds it, or hand a role that holds
/// it to a guest. Removing it (tightening) stays open to agents.
fn agent_config_refused() -> CliResponse {
    owner_only_with_hint(
        "files:agentconfig (app guests may write the agent's .k2 folder) is owner-only — ask your human (Settings → User Access, or k2 app role room <role> <handle> --caps …,files:write,files:agentconfig)",
    )
}

/// 0.46.3 Thread downloads: [`skin::CAP_FILES_DOWNLOAD`] is person-only
/// the same way (Rosson 2026-10-22: Thread access doesn't mean file
/// downloads; a person turns downloads on).
fn files_download_refused() -> CliResponse {
    owner_only_with_hint(
        "files:download (app guests may save Thread files) is person-only — ask your human (Settings → User Access, or k2 app role room <role> <handle> --caps …,files:download)",
    )
}

/// prd-app-caps-0464-v1 AC18 (Q4): adding or widening a role line's
/// download entries is person-only, like `files:download` itself; an agent
/// with "manage Apps" may still narrow them.
fn download_entries_refused(entry: &str) -> CliResponse {
    owner_only_with_hint(&format!(
        "adding download entries ({entry:?}: app guests may save it) is person-only — ask your human (k2 app role paths <role> <room> --download …); removing entries stays open to you"
    ))
}

/// The refusal for an agent granting the person-only `cap`.
fn person_only_refused(cap: &str) -> CliResponse {
    if cap == skin::CAP_FILES_DOWNLOAD {
        files_download_refused()
    } else {
        agent_config_refused()
    }
}

/// The first person-only cap ([`skin::PERSON_ONLY_CAPS`]) in `caps`.
fn person_only_cap(caps: Option<&[String]>) -> Option<&'static str> {
    caps.and_then(|c| skin::person_only_cap_in(c))
}

/// Stored room policy of a role (`None` when no such role yet).
fn stored_role_policy(id_or_name: &str) -> Result<Option<RoomPolicy>, CliResponse> {
    skin::find_role(id_or_name)
        .map(|r| r.map(|r| r.room_policy))
        .map_err(CliResponse::internal_error)
}

fn app_field(v: &serde_json::Value) -> Option<&str> {
    str_field(v, &["app", "appId", "app_id"])
}

fn agent_app_refused() -> CliResponse {
    owner_only_with_hint(
        "naming an App (app) on an app user is owner-only — ask your human (k2 app user add <username> --app <app> [--role R], k2 app user rooms <username> --app <app> --agent <room>)",
    )
}

/// An App target from an owner/agent body or CLI: `host`, a published
/// service id, `<workspace>/<publish name>` (workspace handle, alias or
/// id), or (0.46.4) a bare App id / publish name that names one App on the
/// box. Returns the service id (or `host`). Unknown or ambiguous → Err
/// (400; ambiguous lists the `<workspace>/<name>` candidates). This is
/// owner tooling; the public login route never reports which Apps exist.
pub fn resolve_app_target(raw: &str) -> Result<String, String> {
    let t = raw.trim();
    if t.is_empty() {
        return Err("missing app".to_string());
    }
    if t.eq_ignore_ascii_case(skin::HOST_APP_ID) {
        return Ok(skin::HOST_APP_ID.to_string());
    }
    let db = k2_core::db::shared();
    if let Some((ws, name)) = t.split_once('/') {
        drop(db);
        let ids = skin::resolve_room_tokens(&[ws.trim().to_string()])
            .map_err(|_| format!("unknown app {t:?} (workspace {ws:?})"))?;
        let Some(project_id) = ids.first() else {
            return Err(format!("unknown app {t:?}"));
        };
        let db = k2_core::db::shared();
        let conn = db.lock();
        return match k2_core::published_services::get_by_project_name(&conn, project_id, name) {
            Ok(Some(row)) => Ok(row.id),
            Ok(None) => Err(format!("unknown app {t:?} (no published service {name:?})")),
            Err(e) => Err(format!("app lookup: {e}")),
        };
    }
    let conn = db.lock();
    match k2_core::published_services::get_by_id(&conn, t) {
        Ok(Some(row)) => return Ok(row.id),
        Ok(None) => {}
        Err(e) => return Err(format!("app lookup: {e}")),
    }
    // NSI 0.46.4 #4: the bare App id (what `k2 app list` shows and an
    // agent types) or a bare publish name, when it names ONE App on the
    // box. Two with that name: say so and list them, never guess.
    match k2_core::deploy::store::get_app(&conn, &t.to_ascii_lowercase()) {
        Ok(Some(app)) => return Ok(app.service_id),
        Ok(None) => {}
        Err(e) => return Err(format!("app lookup: {e}")),
    }
    let named: Vec<k2_core::published_services::PublishedService> = match k2_core::published_services::list_all(&conn) {
        Ok(rows) => rows.into_iter().filter(|r| r.name == t).collect(),
        Err(e) => return Err(format!("app lookup: {e}")),
    };
    drop(conn);
    match named.as_slice() {
        [one] => Ok(one.id.clone()),
        [] => Err(format!(
            "unknown app {t:?} (use host, an App id, a published service id, or <workspace>/<name>)"
        )),
        many => {
            let names: Vec<String> = many
                .iter()
                .map(|r| format!("{}/{}", crate::apps::routes::ws_handle(&r.project_id), r.name))
                .collect();
            Err(format!(
                "app {t:?} is ambiguous: {} services on this box have that name; name one as <workspace>/<name>: {}",
                many.len(),
                names.join(", ")
            ))
        }
    }
}

/// Login: `host`, or the exact id of a published service that exists now.
/// Anything else is the same generic 401 as a bad password (no App
/// enumeration, and a removed App's leftover grants can't mint a pass).
fn login_app_is_live(app: &str) -> bool {
    if app == skin::HOST_APP_ID {
        return true;
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    matches!(
        k2_core::published_services::get_by_id(&conn, app),
        Ok(Some(_))
    )
}

/// `POST /cli/skin/users` with the caller's workspace from the gate: a
/// guest an agent adds is stamped as its workspace's (`skin::creators`),
/// so that agent may set it up while it has nothing yet (security review
/// B). If the stamp can't be written the guest is removed again: an
/// unstamped guest would be the owner's, not what the agent asked for.
pub fn handle_users_post_as(agent_ws: Option<&str>, body: &[u8], actor: &str) -> CliResponse {
    let r = handle_users_post(body, actor);
    let Some(ws) = agent_ws else { return r };
    let requested = serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|v| str_field(&v, &["username", "name"]).map(str::to_string));
    stamp_agent_added(r, ws, requested.as_deref(), actor)
}

/// Stamp the guest an agent's add just made (`r`, the handler's answer) as
/// workspace `ws`'s. Whenever it can't be stamped (no id in the answer, or
/// the stamp write fails) the guest is removed again, by the answer's
/// username or else the one asked for: an unstamped guest would read as
/// the owner's, with a password the agent may have picked.
fn stamp_agent_added(r: CliResponse, ws: &str, requested: Option<&str>, actor: &str) -> CliResponse {
    if !r.status.starts_with("200") {
        return r;
    }
    let v = serde_json::from_str::<serde_json::Value>(&r.body).ok();
    let username = v
        .as_ref()
        .and_then(|v| v["username"].as_str())
        .map(str::to_string)
        .or_else(|| requested.and_then(|u| skin::normalize_username(u).ok()));
    let undo = |why: String| {
        if let Some(u) = &username {
            let _ = skin::remove_principal_as(u, actor);
        }
        CliResponse::internal_error(format!("'{}' wasn't added: {why}", username.as_deref().unwrap_or("the new guest")))
    };
    let Some(id) = v.as_ref().and_then(|v| v["id"].as_str()).map(str::to_string) else {
        return undo("the new guest has no id to stamp".to_string());
    };
    if let Err(e) = skin::creators::stamp(&id, ws) {
        return undo(e);
    }
    r
}

pub fn handle_users_post(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(username) = str_field(&v, &["username", "name"]) else {
        return CliResponse::bad_request("missing username");
    };
    let email = match email_field(&v) {
        Ok(e) => e,
        Err(r) => return r,
    };
    let role = str_field(&v, &["role", "roleId", "role_id"]);
    let added = match app_field(&v) {
        // S24-12: an App-only guest — that App's grant, no box login.
        Some(raw_app) => {
            if actor_is_agent(actor) && !AGENTS_MAY_NAME_APP {
                return agent_app_refused();
            }
            let app = match resolve_app_target(raw_app) {
                Ok(a) if a == skin::HOST_APP_ID => {
                    return CliResponse::bad_request(
                        "app host is the box login: add the user without app",
                    )
                }
                Ok(a) => a,
                Err(e) => return CliResponse::bad_request(e),
            };
            skin::add_principal_for_app(username, email, &app, role, actor)
        }
        None => {
            if role.is_some() {
                return CliResponse::bad_request(
                    "role on user add needs app (box-login roles: k2 app user role <username> <role>)",
                );
            }
            skin::add_principal_with_email(username, email)
        }
    };
    match added {
        Ok(p) => {
            k2_core::log_debug!("[skin] actor={actor} added principal {}", p.username);
            let password = str_field(&v, &["password"]);
            if password.is_some() {
                match skin::set_principal_password(&p.username, password) {
                    Ok(p) => {
                        return CliResponse::ok_json(
                            serde_json::to_string(&p).unwrap_or_else(|_| "{}".into()),
                        )
                    }
                    Err(e) => return CliResponse::bad_request(e),
                }
            }
            CliResponse::ok_json(serde_json::to_string(&p).unwrap_or_else(|_| "{}".into()))
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_users_email(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(username) = str_field(&v, &["username", "name"]) else {
        return CliResponse::bad_request("missing username");
    };
    if let Some(r) = agent_test_guest_refused(actor, username) {
        return r;
    }
    let email = match email_field(&v) {
        Ok(e) => e,
        Err(r) => return r,
    };
    match skin::set_principal_email_as(username, email, actor) {
        Ok(p) => {
            k2_core::log_debug!(
                "[skin] actor={actor} set email for {} has_email={}",
                p.username,
                p.email.is_some()
            );
            CliResponse::ok_json(serde_json::to_string(&p).unwrap_or_else(|_| "{}".into()))
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

fn full_name_field(v: &serde_json::Value) -> Result<Option<&str>, CliResponse> {
    match v
        .get("fullName")
        .or_else(|| v.get("full_name"))
        .or_else(|| v.get("name"))
    {
        None => Ok(None),
        Some(x) if x.is_null() => Ok(None),
        Some(x) => match x.as_str() {
            Some(s) => Ok(Some(s)),
            None => Err(CliResponse::bad_request("invalid full name")),
        },
    }
}

/// Cap 64, strip ESC, drop controls, trim. Empty after sanitize → clear.
fn cleaned_full_name(raw: Option<&str>) -> Option<String> {
    raw.and_then(crate::workspace_msg::sanitize_owner_display_name)
}

/// Owner-only. Empty / omitted / controls-only clears. Does not rewrite sessions.
pub fn handle_users_full_name(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(username) = str_field(&v, &["username"]) else {
        return CliResponse::bad_request("missing username");
    };
    let full_name = match full_name_field(&v) {
        Ok(n) => n,
        Err(r) => return r,
    };
    let cleaned = cleaned_full_name(full_name);
    match skin::set_principal_full_name_as(username, cleaned.as_deref(), actor) {
        Ok(p) => {
            k2_core::log_debug!(
                "[skin] actor={actor} set full name for {} has_full_name={}",
                p.username,
                p.full_name.is_some()
            );
            CliResponse::ok_json(serde_json::to_string(&p).unwrap_or_else(|_| "{}".into()))
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

/// Owner-only (`POST /cli/skin/users/agent`): mark a guest principal as
/// the agent of one workspace (`{"username","workspace"}`), or clear the
/// mark (`{"username","clear":true}`). A marked principal's brokered db
/// calls stamp actor kind `agent`. Clearing needs the explicit flag, so a
/// body that drops `workspace` is a 400, never a silent unmark.
pub fn handle_users_agent(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(username) = str_field(&v, &["username"]) else {
        return CliResponse::bad_request("missing username");
    };
    let clear = v.get("clear").and_then(|x| x.as_bool()).unwrap_or(false);
    let workspace = v
        .get("workspace")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let target = match (clear, workspace) {
        (true, None) => None,
        (false, Some(ws)) => Some(ws),
        (true, Some(_)) => {
            return CliResponse::bad_request("pass workspace or clear, not both");
        }
        (false, None) => {
            return CliResponse::bad_request("missing workspace (or clear: true to unmark)");
        }
    };
    match skin::set_principal_agent(username, target) {
        Ok(p) => {
            k2_core::log_debug!(
                "[skin] actor={actor} agent mark for {} workspace={}",
                p.username,
                p.agent_workspace.as_deref().unwrap_or("-")
            );
            k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
                "app_user_agent_mark",
                actor,
                match p.agent_workspace.as_deref() {
                    Some(ws) => format!("marked {} as agent of {ws}", p.username),
                    None => format!("cleared agent mark on {}", p.username),
                },
                "-",
                "-",
                "cli",
            ));
            CliResponse::ok_json(serde_json::to_string(&p).unwrap_or_else(|_| "{}".into()))
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_users_password(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(username) = str_field(&v, &["username", "name"]) else {
        return CliResponse::bad_request("missing username");
    };
    if let Some(r) = agent_test_guest_refused(actor, username) {
        return r;
    }
    let password = v
        .get("password")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    match skin::set_principal_password(username, password) {
        Ok(p) => {
            k2_core::log_debug!(
                "[skin] actor={actor} set password for {} has_password={}",
                p.username,
                p.has_password
            );
            CliResponse::ok_json(serde_json::to_string(&p).unwrap_or_else(|_| "{}".into()))
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

/// Delete one `login_lockouts` row. Body is only `ok`, `username`, `cleared`.
pub fn handle_users_unlock(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(username) = str_field(&v, &["username", "name"]) else {
        return CliResponse::bad_request("missing username");
    };
    if let Some(r) = agent_test_guest_refused(actor, username) {
        return r;
    }
    match skin::clear_login_lockout(username) {
        Ok((key, cleared)) => {
            k2_core::log_debug!("[skin] actor={actor} unlock {key} cleared={cleared}");
            CliResponse::ok_json(
                serde_json::json!({
                    "ok": true,
                    "username": key,
                    "cleared": cleared,
                })
                .to_string(),
            )
        }
        Err(e) => CliResponse::internal_error(e),
    }
}

pub fn handle_users_remove(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(username) = str_field(&v, &["username", "name"]) else {
        return CliResponse::bad_request("missing username");
    };
    if let Some(r) = agent_test_guest_refused(actor, username) {
        return r;
    }
    // app-read-state-v1: the guest's read state goes with them.
    let principal_id = skin::host_login_detail(username).ok().map(|d| d.principal_id);
    match skin::remove_principal_as(username, actor) {
        Ok(true) => {
            if let Some(id) = principal_id {
                crate::read_routes::forget_guest(&id);
            }
            k2_core::log_debug!("[skin] actor={actor} removed principal {username}");
            CliResponse::ok_json(r#"{"success":true}"#.to_string())
        }
        Ok(false) => CliResponse::bad_request(format!("skin user '{username}' not found")),
        Err(e) => CliResponse::internal_error(e),
    }
}

pub fn handle_tokens_get() -> CliResponse {
    match skin::list_tokens() {
        Ok(tokens) => CliResponse::ok_json(serde_json::json!({ "tokens": tokens }).to_string()),
        Err(e) => CliResponse::internal_error(e),
    }
}

pub fn handle_tokens_post(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if v.get("username").is_some() {
        return CliResponse::bad_request("use name (platform label), not username");
    }
    let Some(name) = str_field(&v, &["name"]) else {
        return CliResponse::bad_request("missing name");
    };
    let caps = caps_field(&v);
    if actor_is_agent(actor) {
        if let Some(cap) = person_only_cap(caps.as_deref()) {
            return person_only_refused(cap);
        }
    }
    let rooms = match rooms_field(&v) {
        None => {
            return CliResponse::bad_request("rooms must include at least one workspace");
        }
        Some(list) if list.is_empty() => {
            return CliResponse::bad_request("rooms must include at least one workspace");
        }
        Some(list) => match resolve_rooms_or_400(Some(list)) {
            Ok(ids) if ids.is_empty() => {
                return CliResponse::bad_request("rooms must include at least one workspace");
            }
            Ok(ids) => ids,
            Err(e) => return e,
        },
    };
    match skin::create_token(name, caps.as_deref(), &rooms) {
        Ok((meta, raw)) => {
            k2_core::log_debug!(
                "[skin] actor={actor} minted platform token {} name={} caps={:?} rooms={:?}",
                meta.id,
                meta.name,
                meta.caps,
                meta.rooms
            );
            CliResponse::ok_json(
                serde_json::json!({
                    "id": meta.id,
                    "name": meta.name,
                    "prefix": meta.prefix,
                    "caps": meta.caps,
                    "rooms": meta.rooms,
                    "roomHandles": meta.room_handles,
                    "createdAt": meta.created_at,
                    "token": raw,
                })
                .to_string(),
            )
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_tokens_rooms(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(id) = str_field(&v, &["id", "tokenId", "token_id"]) else {
        return CliResponse::bad_request("missing id");
    };
    let Some(list) = rooms_field(&v) else {
        return CliResponse::bad_request("missing rooms");
    };
    if actor_is_agent(actor) {
        // A platform pass is caps × rooms: new rooms on a pass that holds
        // the `.k2/` opt-in (or files:download) would widen it.
        match skin::list_tokens() {
            Ok(tokens) => {
                if let Some(cap) = tokens.iter().filter(|t| t.id == id).find_map(|t| person_only_cap(Some(&t.caps))) {
                    return person_only_refused(cap);
                }
            }
            Err(e) => return CliResponse::internal_error(e),
        }
    }
    let rooms = match resolve_rooms_or_400(Some(list)) {
        Ok(ids) => ids,
        Err(e) => return e,
    };
    match skin::set_token_rooms(id, &rooms) {
        Ok(meta) => {
            k2_core::log_debug!(
                "[skin] actor={actor} token {} rooms={:?}",
                meta.id,
                meta.rooms
            );
            CliResponse::ok_json(serde_json::to_string(&meta).unwrap_or_else(|_| "{}".into()))
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_grants_get() -> CliResponse {
    let grants = match skin::list_grants() {
        Ok(g) => g,
        Err(e) => return CliResponse::internal_error(e),
    };
    // Sew S27 §17.1: hooked (chat) grants carry `hookState` (synced |
    // pending | blocked | held) and `hookError`. Additive.
    let states = match k2_core::tool_hook::hook_states(&grants) {
        Ok(s) => s,
        Err(e) => return CliResponse::internal_error(e),
    };
    let rows: Vec<serde_json::Value> = grants
        .iter()
        .map(|g| {
            let mut v = serde_json::to_value(g).unwrap_or_default();
            if let Some((state, err)) = states.get(&g.id) {
                v["hookState"] = serde_json::json!(state);
                if let Some(e) = err {
                    v["hookError"] = serde_json::json!(e);
                }
            }
            v
        })
        .collect();
    CliResponse::ok_json(serde_json::json!({ "grants": rows }).to_string())
}

fn opt_text_field<'a>(v: &'a serde_json::Value, keys: &[&str]) -> Option<&'a str> {
    str_field(v, keys)
}

pub fn handle_grants_post(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(subject_kind) = str_field(&v, &["subjectKind", "subject_kind"]) else {
        return CliResponse::bad_request("missing subjectKind");
    };
    let Some(subject_id) = str_field(&v, &["subjectId", "subject_id", "subject"]) else {
        return CliResponse::bad_request("missing subject id");
    };
    let Some(kind) = str_field(&v, &["kind"]) else {
        return CliResponse::bad_request("missing kind");
    };
    // Sew S27: a hooked kind (chat) fills its one target (`sew`), resolves
    // a principal username / workspace handle in core, and defaults the
    // principal scope to `member`.
    let hooked = k2_core::tool_hook::provisioner_for(kind);
    let target_id = match (str_field(&v, &["targetId", "target_id", "target"]), hooked) {
        (Some(t), _) => t,
        (None, Some(p)) => p.default_target(),
        (None, None) => return CliResponse::bad_request("missing target id"),
    };
    let is_app = kind.trim().eq_ignore_ascii_case("app");
    // S24-11: an App target may be `host`, a service id, or
    // `<workspace>/<publish name>`; an unknown App is a 400.
    let target_id = if is_app {
        match resolve_app_target(target_id) {
            Ok(id) => id,
            Err(e) => return CliResponse::bad_request(e),
        }
    } else {
        target_id.to_string()
    };
    // S24-7: a role-less App grant is a rooms list (`rooms` array, or a
    // `scope` JSON array) of handles or ids, stored as room ids.
    let scope = if is_app {
        let list: Option<Vec<String>> = match (rooms_field(&v), opt_text_field(&v, &["scope"])) {
            (Some(_), Some(_)) => {
                return CliResponse::bad_request("pass rooms or scope, not both")
            }
            (Some(list), None) => Some(list),
            (None, Some(raw)) => match serde_json::from_str::<Vec<String>>(raw) {
                Ok(list) => Some(list),
                Err(_) => {
                    return CliResponse::bad_request(
                        "kind=app scope must be a JSON array of rooms, e.g. [\"sales\"]",
                    )
                }
            },
            (None, None) => None,
        };
        match list {
            Some(list) => match resolve_rooms_allow_empty(Some(list)) {
                Ok(ids) => Some(serde_json::to_string(&ids).unwrap_or_else(|_| "[]".into())),
                Err(e) => return e,
            },
            None => None,
        }
    } else {
        opt_text_field(&v, &["scope"]).map(str::to_string)
    };
    let write = skin::SkinGrantWrite {
        subject_kind: subject_kind.to_string(),
        subject_id: subject_id.to_string(),
        kind: kind.to_string(),
        target_id,
        role: opt_text_field(&v, &["roleId", "role_id", "role"]).map(str::to_string),
        scope,
        created_by: Some(actor.to_string()),
    };
    match skin::create_grant(&write) {
        Ok(grant) => {
            k2_core::log_debug!(
                "[skin] actor={actor} grant {} kind={} target={} subject={} {}",
                grant.id,
                grant.kind,
                grant.target_id,
                grant.subject_kind,
                grant.subject_id
            );
            CliResponse::ok_json(serde_json::to_string(&grant).unwrap_or_else(|_| "{}".into()))
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_grants_delete(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    // Sew S27 §17.1: the response lists cascaded assistant grants.
    let deleted = if let Some(id) = str_field(&v, &["id"]) {
        skin::delete_grant_by_id_with_cascade(id, actor)
    } else {
        let Some(subject_kind) = str_field(&v, &["subjectKind", "subject_kind"]) else {
            return CliResponse::bad_request("missing grant id or subjectKind");
        };
        let Some(subject_id) = str_field(&v, &["subjectId", "subject_id", "subject"]) else {
            return CliResponse::bad_request("missing subject id");
        };
        let Some(kind) = str_field(&v, &["kind"]) else {
            return CliResponse::bad_request("missing kind");
        };
        let target_id = match str_field(&v, &["targetId", "target_id", "target"]) {
            Some(t) => t,
            None if k2_core::tool_hook::provisioner_for(kind).is_some() => "",
            None => return CliResponse::bad_request("missing target id"),
        };
        skin::delete_grant_by_key_with_cascade(subject_kind, subject_id, kind, target_id, actor)
    };
    match deleted {
        Ok(Some(cascaded)) => {
            k2_core::log_debug!(
                "[skin] actor={actor} revoked grant (cascaded {})",
                cascaded.len()
            );
            CliResponse::ok_json(
                serde_json::json!({ "success": true, "cascaded": cascaded }).to_string(),
            )
        }
        Ok(None) => CliResponse::bad_request("unknown grant"),
        Err(e) => CliResponse::bad_request(e),
    }
}

// ── Sew S27 grant hooks (owner-tier; POSTs are POST-only) ────────────

/// `POST /cli/skin/grants/update {id, scope}` → the updated grant.
pub fn handle_grants_update(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(id) = str_field(&v, &["id"]) else {
        return CliResponse::bad_request("missing grant id");
    };
    let Some(scope) = str_field(&v, &["scope"]) else {
        return CliResponse::bad_request("missing scope");
    };
    match skin::update_grant_scope(id, scope, actor) {
        Ok(g) => {
            k2_core::log_debug!("[skin] actor={actor} grant {} scope updated", g.id);
            CliResponse::ok_json(serde_json::to_string(&g).unwrap_or_else(|_| "{}".into()))
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

/// `GET /cli/skin/grant-hooks` → targets, outbox counts, blocked + held rows.
pub fn handle_grant_hooks_get() -> CliResponse {
    match k2_core::tool_hook::status() {
        Ok(v) => CliResponse::ok_json(v.to_string()),
        Err(e) => CliResponse::internal_error(e),
    }
}

fn query_value(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then(|| percent_decode(v))
    })
}

/// `GET /cli/skin/grant-hooks/log?limit=50&subject=<ext id>` → newest
/// first; payload only on blocked rows (payloads hold emails).
pub fn handle_grant_hooks_log(query: &str) -> CliResponse {
    let limit = match query_value(query, "limit") {
        None => 50,
        Some(l) => match l.parse::<usize>() {
            Ok(n) if (1..=500).contains(&n) => n,
            _ => return CliResponse::bad_request("limit must be 1–500"),
        },
    };
    let subject = query_value(query, "subject").filter(|s| !s.trim().is_empty());
    match k2_core::tool_hook::log(limit, subject.as_deref()) {
        Ok(rows) => CliResponse::ok_json(
            serde_json::json!({
                "rows": rows,
                "actorNote": "actor is reported by K2; it is not proof a person acted",
            })
            .to_string(),
        ),
        Err(e) => CliResponse::internal_error(e),
    }
}

/// `POST /cli/skin/grant-hooks/target {kind, enabled, socket?, tokenFile?}`.
pub fn handle_grant_hooks_target(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let kind = str_field(&v, &["kind"]).unwrap_or(k2_core::tool_hook::CHAT_KIND);
    let enabled = match require_bool(&v, &["enabled"]) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let socket = str_field(&v, &["socket"]);
    let token_file = str_field(&v, &["tokenFile", "token_file"]);
    match k2_core::tool_hook::set_target(kind, enabled, socket, token_file, actor) {
        Ok(t) => CliResponse::ok_json(serde_json::to_string(&t).unwrap_or_else(|_| "{}".into())),
        Err(e) => CliResponse::bad_request(e),
    }
}

/// `POST /cli/skin/grant-hooks/retry {id}` (blocked, or pending that failed).
pub fn handle_grant_hooks_retry(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(id) = str_field(&v, &["id"]) else {
        return CliResponse::bad_request("missing outbox row id");
    };
    match k2_core::tool_hook::retry(id) {
        Ok(row) => {
            k2_core::log_debug!("[grant-hook] actor={actor} retry row {id}");
            CliResponse::ok_json(serde_json::to_string(&row).unwrap_or_else(|_| "{}".into()))
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

/// `POST /cli/skin/grant-hooks/skip {id}` (blocked only); then reconcile.
pub fn handle_grant_hooks_skip(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(id) = str_field(&v, &["id"]) else {
        return CliResponse::bad_request("missing outbox row id");
    };
    match k2_core::tool_hook::skip(id) {
        Ok(row) => {
            k2_core::log_debug!("[grant-hook] actor={actor} skip row {id}");
            CliResponse::ok_json(serde_json::to_string(&row).unwrap_or_else(|_| "{}".into()))
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

/// `POST /cli/skin/grant-hooks/reconcile {}` → `{started:true}`.
pub fn handle_grant_hooks_reconcile(_body: &[u8], actor: &str) -> CliResponse {
    k2_core::log_debug!("[grant-hook] actor={actor} asked for a reconcile");
    k2_core::tool_hook::kick_reconcile();
    CliResponse::ok_json(r#"{"started":true}"#.to_string())
}

fn require_bool(v: &serde_json::Value, keys: &[&str]) -> Result<bool, CliResponse> {
    for k in keys {
        if let Some(raw) = v.get(*k) {
            return raw
                .as_bool()
                .ok_or_else(|| CliResponse::bad_request(format!("{k} must be a boolean")));
        }
    }
    Err(CliResponse::bad_request(format!("missing {}", keys[0])))
}

pub fn handle_grants_enabled(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(id) = str_field(&v, &["id"]) else {
        return CliResponse::bad_request("missing grant id");
    };
    let enabled = match require_bool(&v, &["enabled"]) {
        Ok(b) => b,
        Err(r) => return r,
    };
    match skin::set_grant_enabled_as(id, enabled, actor) {
        Ok(grant) => {
            k2_core::log_debug!(
                "[skin] actor={actor} grant {} enabled={}",
                grant.id,
                grant.enabled
            );
            CliResponse::ok_json(serde_json::to_string(&grant).unwrap_or_else(|_| "{}".into()))
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_grants_host(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(username) = str_field(&v, &["username"]) else {
        return CliResponse::bad_request("missing username");
    };
    let present = match require_bool(&v, &["present"]) {
        Ok(b) => b,
        Err(r) => return r,
    };
    match skin::set_host_login(username, present) {
        Ok(changed) => {
            k2_core::log_debug!(
                "[skin] actor={actor} host login {username} present={present} changed={changed}"
            );
            CliResponse::ok_json(
                serde_json::json!({ "success": true, "present": present }).to_string(),
            )
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

/// Owner (S24-11): `POST /cli/skin/grants/copy-host {app, dryRun?}` — give
/// everyone who has the box login the same access on one App (same rules
/// as the 0.45.5 upgrade backfill). Use it after publishing a new App (it
/// starts with nobody) or before a BFF starts sending `app`.
pub fn handle_grants_copy_host(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(raw) = app_field(&v).or_else(|| str_field(&v, &["target", "targetId"])) else {
        return CliResponse::bad_request("missing app");
    };
    let app = match resolve_app_target(raw) {
        Ok(a) if a == skin::HOST_APP_ID => {
            return CliResponse::bad_request("copy-host needs a published App, not host")
        }
        Ok(a) => a,
        Err(e) => return CliResponse::bad_request(e),
    };
    let dry_run = v
        .get("dryRun")
        .or_else(|| v.get("dry_run"))
        .and_then(|x| x.as_bool())
        .unwrap_or(false);
    // The App's workspace: a box guest an agent added lands only on its
    // own workspace's Apps (security review E).
    let app_ws = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::published_services::get_by_id(&conn, &app).ok().flatten().map(|r| r.project_id)
    };
    match skin::copy_host_grants_to_app_in(&app, app_ws.as_deref(), actor, dry_run) {
        Ok(report) => {
            k2_core::log_debug!(
                "[skin] actor={actor} copy-host app={} dry_run={dry_run} added={} kept={} skipped={}",
                report.app,
                report.added.len(),
                report.kept_existing.len(),
                report.skipped_other_app_role.len()
            );
            let kept: Vec<serde_json::Value> = report
                .kept_existing
                .iter()
                .map(|(u, enabled)| serde_json::json!({ "username": u, "enabled": enabled }))
                .collect();
            let mut out = serde_json::json!({
                "ok": true,
                "app": report.app,
                "dryRun": report.dry_run,
                "added": report.added,
                "keptExisting": kept,
                "skippedOtherAppRole": report.skipped_other_app_role,
            });
            if !report.skipped_other_workspace_agent.is_empty() {
                out["skippedOtherWorkspaceAgent"] = serde_json::json!(report.skipped_other_workspace_agent);
            }
            CliResponse::ok_json(out.to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_templates_get() -> CliResponse {
    match skin::list_access_templates() {
        Ok(templates) => {
            CliResponse::ok_json(serde_json::json!({ "templates": templates }).to_string())
        }
        Err(e) => CliResponse::internal_error(e),
    }
}

pub fn handle_templates_post(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(name) = str_field(&v, &["name"]) else {
        return CliResponse::bad_request("missing template name");
    };
    match skin::create_access_template(name) {
        Ok(template) => {
            k2_core::log_debug!(
                "[skin] actor={actor} template {} name={}",
                template.id,
                template.name
            );
            CliResponse::ok_json(serde_json::to_string(&template).unwrap_or_else(|_| "{}".into()))
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_templates_update(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(id) = str_field(&v, &["id"]) else {
        return CliResponse::bad_request("missing template id");
    };
    let Some(name) = str_field(&v, &["name"]) else {
        return CliResponse::bad_request("missing template name");
    };
    match skin::update_access_template(id, name) {
        Ok(template) => {
            k2_core::log_debug!(
                "[skin] actor={actor} template {} name={}",
                template.id,
                template.name
            );
            CliResponse::ok_json(serde_json::to_string(&template).unwrap_or_else(|_| "{}".into()))
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_templates_delete(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(id) = str_field(&v, &["id"]) else {
        return CliResponse::bad_request("missing template id");
    };
    match skin::delete_access_template(id) {
        Ok(true) => {
            k2_core::log_debug!("[skin] actor={actor} deleted template {id}");
            CliResponse::ok_json(r#"{"success":true}"#.to_string())
        }
        Ok(false) => CliResponse::bad_request("unknown template"),
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_template_lines_post(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(template_id) = str_field(&v, &["templateId", "template_id"]) else {
        return CliResponse::bad_request("missing template id");
    };
    let Some(kind) = str_field(&v, &["kind"]) else {
        return CliResponse::bad_request("missing kind");
    };
    let role = opt_text_field(&v, &["roleId", "role_id", "role"]);
    match skin::create_template_line(template_id, kind, role) {
        Ok(line) => {
            k2_core::log_debug!(
                "[skin] actor={actor} template line {} kind={}",
                line.id,
                line.kind
            );
            CliResponse::ok_json(serde_json::to_string(&line).unwrap_or_else(|_| "{}".into()))
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_template_lines_update(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(id) = str_field(&v, &["id"]) else {
        return CliResponse::bad_request("missing template line id");
    };
    let Some(kind) = str_field(&v, &["kind"]) else {
        return CliResponse::bad_request("missing kind");
    };
    let role = opt_text_field(&v, &["roleId", "role_id", "role"]);
    match skin::update_template_line(id, kind, role) {
        Ok(line) => {
            k2_core::log_debug!(
                "[skin] actor={actor} template line {} kind={}",
                line.id,
                line.kind
            );
            CliResponse::ok_json(serde_json::to_string(&line).unwrap_or_else(|_| "{}".into()))
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_template_lines_delete(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(id) = str_field(&v, &["id"]) else {
        return CliResponse::bad_request("missing template line id");
    };
    match skin::delete_template_line(id) {
        Ok(true) => {
            k2_core::log_debug!("[skin] actor={actor} deleted template line {id}");
            CliResponse::ok_json(r#"{"success":true}"#.to_string())
        }
        Ok(false) => CliResponse::bad_request("unknown template line"),
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_templates_apply(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(principal_id) = str_field(&v, &["principalId", "principal_id"]) else {
        return CliResponse::bad_request("missing subject id");
    };
    let Some(template_id) = str_field(&v, &["templateId", "template_id"]) else {
        return CliResponse::bad_request("missing template id");
    };
    let Some(arr) = v.get("lines").and_then(|x| x.as_array()) else {
        return CliResponse::bad_request("missing lines");
    };
    let mut lines = Vec::with_capacity(arr.len());
    for item in arr {
        let Some(line_id) = str_field(item, &["lineId", "line_id", "id"]) else {
            return CliResponse::bad_request("missing line id");
        };
        let Some(target_id) = str_field(item, &["targetId", "target_id", "target"]) else {
            return CliResponse::bad_request("missing target id");
        };
        lines.push(skin::TemplateApplyLine {
            line_id: line_id.to_string(),
            target_id: target_id.to_string(),
            scope: opt_text_field(item, &["scope"]).map(str::to_string),
            role: opt_text_field(item, &["roleId", "role_id", "role"]).map(str::to_string),
        });
    }
    match skin::apply_access_template(principal_id, template_id, &lines) {
        Ok(grants) => {
            k2_core::log_debug!(
                "[skin] actor={actor} applied template {template_id} grants={}",
                grants.len()
            );
            CliResponse::ok_json(serde_json::json!({ "grants": grants }).to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_access_post(path: &str, body: &[u8], actor: &str) -> CliResponse {
    match path {
        "/cli/skin/grants/enabled" => handle_grants_enabled(body, actor),
        "/cli/skin/grants/host" => handle_grants_host(body, actor),
        "/cli/skin/grants/copy-host" => handle_grants_copy_host(body, actor),
        "/cli/skin/grants/update" => handle_grants_update(body, actor),
        "/cli/skin/grant-hooks/target" => handle_grant_hooks_target(body, actor),
        "/cli/skin/grant-hooks/retry" => handle_grant_hooks_retry(body, actor),
        "/cli/skin/grant-hooks/skip" => handle_grant_hooks_skip(body, actor),
        "/cli/skin/grant-hooks/reconcile" => handle_grant_hooks_reconcile(body, actor),
        "/cli/skin/templates/update" => handle_templates_update(body, actor),
        "/cli/skin/templates/delete" => handle_templates_delete(body, actor),
        "/cli/skin/templates/lines" => handle_template_lines_post(body, actor),
        "/cli/skin/templates/lines/update" => handle_template_lines_update(body, actor),
        "/cli/skin/templates/lines/delete" => handle_template_lines_delete(body, actor),
        "/cli/skin/templates/apply" => handle_templates_apply(body, actor),
        _ => CliResponse::not_found(),
    }
}

pub fn handle_roles_get() -> CliResponse {
    match skin::list_roles() {
        Ok(roles) => CliResponse::ok_json(serde_json::json!({ "roles": roles }).to_string()),
        Err(e) => CliResponse::internal_error(e),
    }
}

pub fn handle_roles_post(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if v.get("username").is_some() {
        return CliResponse::bad_request("use name (role label), not username");
    }
    let Some(name) = str_field(&v, &["name"]) else {
        return CliResponse::bad_request("missing name");
    };
    let policy = match role_policy_from_body(&v, false) {
        Ok(Some(p)) => p,
        Ok(None) => RoomPolicy::new(),
        Err(e) => return e,
    };
    if actor_is_agent(actor) {
        if let Some(cap) = skin::person_only_cap_in_policy(&policy) {
            return person_only_refused(cap);
        }
    }
    // `app` may be a service id or `<workspace>/<name>` (S24-15); `host`
    // means host-wide (NULL), the same as leaving it out.
    let app_id = match opt_text_field(&v, &["appId", "app_id", "app"]) {
        None => None,
        Some(raw) => match resolve_app_target(raw) {
            Ok(a) if a == skin::HOST_APP_ID => None,
            Ok(a) => Some(a),
            Err(e) => return CliResponse::bad_request(e),
        },
    };
    match skin::create_role_for_app(name, &policy, app_id.as_deref()) {
        Ok(role) => {
            k2_core::log_debug!(
                "[skin] actor={actor} created role {} name={} app={:?} caps={:?} rooms={:?}",
                role.id,
                role.name,
                role.app_id,
                role.caps,
                role.rooms
            );
            CliResponse::ok_json(serde_json::to_string(&role).unwrap_or_else(|_| "{}".into()))
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_roles_update(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(id_or_name) = str_field(&v, &["id", "name", "role"]) else {
        return CliResponse::bad_request("missing id or name");
    };
    let policy = match role_policy_from_body(&v, true) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if actor_is_agent(actor) {
        if let Some(new) = policy.as_ref() {
            let old = match stored_role_policy(id_or_name) {
                Ok(o) => o,
                Err(e) => return e,
            };
            if let Some(cap) = skin::person_only_cap_added(old.as_ref(), new) {
                return person_only_refused(cap);
            }
        }
    }
    match skin::update_role(id_or_name, policy.as_ref()) {
        Ok(role) => {
            k2_core::log_debug!(
                "[skin] actor={actor} updated role {} caps={:?} rooms={:?}",
                role.name,
                role.caps,
                role.rooms
            );
            CliResponse::ok_json(serde_json::to_string(&role).unwrap_or_else(|_| "{}".into()))
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_roles_room(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(id_or_name) = str_field(&v, &["id", "name", "role"]) else {
        return CliResponse::bad_request("missing id or name");
    };
    let Some(handle) = str_field(&v, &["handle", "id"]) else {
        return CliResponse::bad_request("missing handle or id");
    };
    let clear = match v.get("clear") {
        None => false,
        Some(x) if x.is_null() => false,
        Some(x) => x.as_bool().unwrap_or(false),
    };
    let ids = match skin::resolve_room_tokens(&[handle.to_string()]) {
        Ok(ids) if ids.len() == 1 => ids,
        Ok(_) => return CliResponse::bad_request(format!("unknown workspace handle {handle:?}")),
        Err(e) => return CliResponse::bad_request(e),
    };
    let project_id = &ids[0];
    let result = if clear {
        skin::clear_role_room(id_or_name, project_id)
    } else {
        let caps = caps_field(&v);
        if actor_is_agent(actor) && person_only_cap(caps.as_deref()).is_some() {
            let old = match stored_role_policy(id_or_name) {
                Ok(old) => old,
                Err(e) => return e,
            };
            let had = |p: &str| old.as_ref().and_then(|o| o.get(project_id)).is_some_and(|c| c.iter().any(|x| x == p));
            let added = caps
                .as_deref()
                .into_iter()
                .flatten()
                .find_map(|c| skin::PERSON_ONLY_CAPS.iter().copied().find(|p| c == p && !had(p)));
            if let Some(cap) = added {
                return person_only_refused(cap);
            }
        }
        skin::set_role_room(id_or_name, project_id, caps.as_deref())
    };
    match result {
        Ok(role) => {
            k2_core::log_debug!(
                "[skin] actor={actor} role room {} handle={} clear={clear} rooms={:?}",
                role.name,
                handle,
                role.rooms
            );
            CliResponse::ok_json(serde_json::to_string(&role).unwrap_or_else(|_| "{}".into()))
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

/// One folder list from a body key: an array of strings (absent = empty).
fn folder_list(v: &serde_json::Value, key: &str) -> Result<Vec<String>, CliResponse> {
    match v.get(key) {
        None | Some(serde_json::Value::Null) => Ok(Vec::new()),
        Some(serde_json::Value::Array(a)) => a
            .iter()
            .map(|x| {
                x.as_str()
                    .map(|s| s.to_string())
                    .ok_or_else(|| CliResponse::bad_request(format!("{key} must be an array of folder strings")))
            })
            .collect(),
        Some(_) => Err(CliResponse::bad_request(format!("{key} must be an array of folder strings"))),
    }
}

/// `POST /cli/skin/roles/paths` (`k2 app role paths`, room-scope R3,
/// RS-A1..A3): set one room line's folders on a role. Body `{name, handle,
/// read?: [..], write?: [..], upload?: [..], uploadDir?: ".."}` replaces
/// the line's folders; `{name, handle, clear: true}` (or nothing listed)
/// puts the line back to the whole room. The room must already be on the
/// role. `upload` (0.46.4, AC8) is the line's drop folders: narrower than
/// `write`, so an agent with "manage Apps" may set them like any folder.
///
/// `download` (0.46.4, AC15/AC18a) is the line's download entries (folders
/// or single files `files:download` may save without `files:read`):
/// - an omitted `download` keeps the stored entries (`download: []`
///   clears them), so editing read folders never drops what a person set;
/// - a body that names only `download` changes only the entries, never
///   the line's read/write/upload folders (so it can't unwall a line);
/// - adding or widening an entry is person-only: an agent gets 403
///   `owner_only` when the new list holds an entry the stored list doesn't
///   cover; narrowing stays open (AC18, Q4);
/// - `clear: true` still clears everything (narrowing).
pub fn handle_roles_paths(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(id_or_name) = str_field(&v, &["name", "role", "id"]) else {
        return CliResponse::bad_request("missing role name");
    };
    let Some(handle) = str_field(&v, &["handle", "room", "workspace"]) else {
        return CliResponse::bad_request("missing handle (the room)");
    };
    let ids = match skin::resolve_room_tokens(&[handle.to_string()]) {
        Ok(ids) if ids.len() == 1 => ids,
        Ok(_) => return CliResponse::bad_request(format!("unknown workspace handle {handle:?}")),
        Err(e) => return CliResponse::bad_request(e),
    };
    let clear = v.get("clear").and_then(|x| x.as_bool()).unwrap_or(false);
    let req: Option<PathsBody> = if clear {
        None
    } else {
        let read = match folder_list(&v, "read") {
            Ok(l) => l,
            Err(r) => return r,
        };
        let write = match folder_list(&v, "write") {
            Ok(l) => l,
            Err(r) => return r,
        };
        let upload = match folder_list(&v, "upload") {
            Ok(l) => l,
            Err(r) => return r,
        };
        let upload_dir = match v.get("uploadDir").or_else(|| v.get("upload_dir")) {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::String(s)) => Some(s.clone()),
            Some(_) => return CliResponse::bad_request("uploadDir must be a folder string"),
        };
        let download_sent = !matches!(v.get("download"), None | Some(serde_json::Value::Null));
        let download = match folder_list(&v, "download") {
            Ok(l) => l,
            Err(r) => return r,
        };
        let folders_sent = !read.is_empty() || !write.is_empty() || !upload.is_empty() || upload_dir.is_some();
        if !folders_sent && !download_sent {
            return CliResponse::bad_request(
                "nothing to set: send read, write, upload, uploadDir or download, or clear:true for the whole room",
            );
        }
        let mut sent_download: Vec<String> = Vec::new();
        for e in &download {
            match skin::paths::normalize_folder(e) {
                Ok(f) if !sent_download.contains(&f) => sent_download.push(f),
                Ok(_) => {}
                Err(e) => return CliResponse::bad_request(format!("download: {e}")),
            }
        }
        Some(PathsBody { download_sent, sent_download, folders_sent, read, write, upload_dir, upload })
    };
    let is_agent = actor_is_agent(actor);
    // Review A5: the stored line is read, the widening checked and the new
    // line written in ONE transaction (`update_role_room_paths`).
    let written = skin::paths::update_role_room_paths(id_or_name, &ids[0], |stored| {
        let Some(b) = &req else { return Ok(skin::RoomPathScope::default()) };
        // The stored line: kept download entries, the widening check, and a
        // download-only body's other folders.
        let download = if b.download_sent {
            if is_agent {
                if let Some(entry) = skin::paths::download_widening(&stored.download, &b.sent_download) {
                    return Err(skin::paths::PathsUpdateError::Refused(entry.to_string()));
                }
            }
            b.sent_download.clone()
        } else {
            stored.download.clone()
        };
        Ok(if b.folders_sent {
            skin::RoomPathScope {
                read: b.read.clone(),
                write: b.write.clone(),
                upload_dir: b.upload_dir.clone(),
                upload: b.upload.clone(),
                download,
            }
        } else {
            skin::RoomPathScope { download, ..stored.clone() }
        })
    });
    match written {
        Err(skin::paths::PathsUpdateError::Refused(entry)) => download_entries_refused(&entry),
        Err(skin::paths::PathsUpdateError::Bad(e)) => CliResponse::bad_request(e),
        Ok(role) => {
            let scope = role.room_paths.get(&ids[0]).cloned().unwrap_or_default();
            k2_core::log_debug!(
                "[skin] actor={actor} role paths {} handle={handle} clear={clear} read={:?} write={:?} upload={:?} uploadDir={:?} download={:?}",
                role.name,
                scope.read,
                scope.write,
                scope.upload,
                scope.upload_dir,
                scope.download
            );
            CliResponse::ok_json(serde_json::to_string(&role).unwrap_or_else(|_| "{}".into()))
        }
    }
}

/// What a `POST /cli/skin/roles/paths` body asks for (`download`
/// normalized here for the widening check; the store normalizes the rest).
struct PathsBody {
    download_sent: bool,
    sent_download: Vec<String>,
    folders_sent: bool,
    read: Vec<String>,
    write: Vec<String>,
    upload_dir: Option<String>,
    upload: Vec<String>,
}

pub fn handle_roles_remove(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(id_or_name) = str_field(&v, &["id", "name", "role"]) else {
        return CliResponse::bad_request("missing id or name");
    };
    match skin::remove_role(id_or_name) {
        Ok(true) => {
            k2_core::log_debug!("[skin] actor={actor} removed role {id_or_name}");
            CliResponse::ok_json(r#"{"success":true}"#.to_string())
        }
        Ok(false) => CliResponse::bad_request(format!("unknown skin role '{id_or_name}'")),
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_roles_assign(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(username) = str_field(&v, &["username"]) else {
        return CliResponse::bad_request("missing username");
    };
    if let Some(r) = agent_test_guest_refused(actor, username) {
        return r;
    }
    let Some(role) = str_field(&v, &["role", "name", "id"]) else {
        return CliResponse::bad_request("missing role");
    };
    if actor_is_agent(actor) {
        match stored_role_policy(role) {
            Ok(Some(p)) => {
                if let Some(cap) = skin::person_only_cap_in_policy(&p) {
                    return person_only_refused(cap);
                }
            }
            Ok(_) => {}
            Err(e) => return e,
        }
    }
    match skin::assign_role(username, role) {
        Ok(p) => {
            k2_core::log_debug!(
                "[skin] actor={actor} assigned role {} to {}",
                p.role_name.as_deref().unwrap_or(role),
                p.username
            );
            CliResponse::ok_json(serde_json::to_string(&p).unwrap_or_else(|_| "{}".into()))
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_roles_unassign(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(username) = str_field(&v, &["username", "name"]) else {
        return CliResponse::bad_request("missing username");
    };
    if let Some(r) = agent_test_guest_refused(actor, username) {
        return r;
    }
    match skin::unassign_role(username) {
        Ok(p) => {
            k2_core::log_debug!("[skin] actor={actor} unassigned role from {}", p.username);
            CliResponse::ok_json(serde_json::to_string(&p).unwrap_or_else(|_| "{}".into()))
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_users_rooms(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(username) = str_field(&v, &["username", "name"]) else {
        return CliResponse::bad_request("missing username");
    };
    if let Some(r) = agent_test_guest_refused(actor, username) {
        return r;
    }
    let Some(list) = rooms_field(&v) else {
        return CliResponse::bad_request("missing rooms");
    };
    let rooms = match resolve_rooms_or_400(Some(list)) {
        Ok(ids) => ids,
        Err(e) => return e,
    };
    let apply = apply_tokens_field(&v);
    // S24-15: `app` sets that App grant's role-less rooms (owner only).
    if let Some(raw_app) = app_field(&v) {
        if actor_is_agent(actor) && !AGENTS_MAY_NAME_APP {
            return agent_app_refused();
        }
        let app = match resolve_app_target(raw_app) {
            Ok(a) => a,
            Err(e) => return CliResponse::bad_request(e),
        };
        return match skin::set_app_grant_rooms(username, &app, &rooms, apply, actor) {
            Ok(grant) => {
                k2_core::log_debug!(
                    "[skin] actor={actor} user {username} app {app} rooms={rooms:?} apply_tokens={apply}"
                );
                let handles = skin::handles_for_project_ids(&rooms);
                CliResponse::ok_json(
                    serde_json::json!({
                        "username": username.to_ascii_lowercase(),
                        "app": app,
                        "rooms": rooms,
                        "roomHandles": handles,
                        "grant": grant,
                    })
                    .to_string(),
                )
            }
            Err(e) => CliResponse::bad_request(e),
        };
    }
    match skin::set_principal_default_rooms(username, &rooms, apply) {
        Ok(p) => {
            k2_core::log_debug!(
                "[skin] actor={actor} user {} default_rooms={:?} apply_tokens={apply}",
                p.username,
                p.default_rooms
            );
            CliResponse::ok_json(serde_json::to_string(&p).unwrap_or_else(|_| "{}".into()))
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

/// `GET /cli/skin/agents` (`roomAgents.list`): the agents in the pass's
/// rooms. 0.46.4 (agent-unread-tickets-v1): each row adds `unread`,
/// `waiting`, `muted` where the pass holds `thread:read` in that room and
/// `ticketsWaiting` where it holds `tickets:read`
/// ([`crate::skin_me_routes::agent_counts`]); nothing for another room.
pub fn handle_agents_get(pass: &skin::SkinPass) -> CliResponse {
    let agents = skin::live_agents(&pass.rooms);
    let mut rows: Vec<serde_json::Value> = match agents.iter().map(serde_json::to_value).collect() {
        Ok(rows) => rows,
        Err(e) => return CliResponse::internal_error(format!("serialize agents: {e}")),
    };
    crate::skin_me_routes::add_agent_counts(pass, &mut rows, "projectId");
    CliResponse::ok_json(serde_json::json!({ "agents": rows }).to_string())
}

pub fn handle_tokens_revoke(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(id) = str_field(&v, &["id", "tokenId", "token_id"]) else {
        return CliResponse::bad_request("missing id");
    };
    match skin::revoke_token(id) {
        Ok(true) => {
            k2_core::log_debug!("[skin] actor={actor} revoked token {id}");
            CliResponse::ok_json(r#"{"success":true}"#.to_string())
        }
        Ok(false) => CliResponse::ok_json(r#"{"success":false}"#.to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_front_door_get() -> CliResponse {
    status_json()
}

pub fn handle_hydra_get() -> CliResponse {
    CliResponse::ok_json(crate::skin_hydra::status_json().to_string())
}

pub fn handle_hydra_post(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let enabled = match v.get("enabled").or_else(|| v.get("Enabled")) {
        Some(x) => match x.as_bool() {
            Some(b) => b,
            None => return CliResponse::bad_request("enabled must be true or false"),
        },
        None => return CliResponse::bad_request("missing enabled (true|false)"),
    };
    let apply = apply_field(&v);
    match crate::skin_hydra::apply(enabled, apply) {
        Ok(st) => {
            k2_core::log_debug!(
                "[skin] actor={actor} hydra enabled={enabled} apply={apply} running={}",
                st.get("running").and_then(|x| x.as_bool()).unwrap_or(false)
            );
            CliResponse::ok_json(st.to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_front_door_post(body: &[u8], actor: &str, daemon_port: u16) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(mode) = str_field(&v, &["mode"]) else {
        return CliResponse::bad_request("missing mode (connect|direct)");
    };
    let url = str_field(&v, &["url"]);
    let hint = str_field(&v, &["hint"]);
    let current = skin::get_front_door().ok().and_then(|d| d.ui_port);
    let ui_port = ui_port_field(&v, current);
    let apply = apply_field(&v);
    match skin::set_front_door(mode, url, hint, ui_port) {
        Ok(d) => {
            k2_core::log_debug!(
                "[skin] actor={actor} front-door mode={} apply={apply}",
                d.mode
            );
            if apply {
                let applied = skin_door::apply(daemon_port);
                // front443: Skin Direct's routes follow the Caddy just applied.
                crate::front443::rebuild();
                match applied {
                    Ok(st) => CliResponse::ok_json(
                        serde_json::to_string(&st).unwrap_or_else(|_| "{}".into()),
                    ),
                    Err(e) => CliResponse::bad_request(e),
                }
            } else {
                status_json()
            }
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn missing_cap_response(cap: &str) -> CliResponse {
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: serde_json::json!({ "error": format!("missing capability {cap}") }).to_string(),
    }
}

pub fn revoked_skin_response() -> CliResponse {
    CliResponse {
        status: "401 Unauthorized",
        content_type: "application/json",
        body: r#"{"error":"invalid or revoked skin token"}"#.to_string(),
    }
}

pub const SKIN_ROOM_JSON: &str =
    r#"{"ok":false,"error":{"code":"skin_room","hint":"this pass cannot use that agent"}}"#;

pub fn skin_room_response() -> CliResponse {
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: SKIN_ROOM_JSON.to_string(),
    }
}

/// Owner/Connect/hook on `GET /cli/skin/agents` — that list is skin-token only.
pub fn skin_agents_forbidden() -> CliResponse {
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": {
                "code": "skin_token",
                "hint": "GET /cli/skin/agents is for a live skin pass",
            },
        })
        .to_string(),
    }
}

pub fn skin_terminal_forbidden() -> CliResponse {
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: r#"{"error":"skin tokens cannot use the terminal"}"#.to_string(),
    }
}

/// `GET /cli/db/dsn` with a skin token — guests never hold a DSN.
pub fn skin_dsn_forbidden() -> CliResponse {
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: r#"{"error":"skin tokens cannot use database credentials"}"#.to_string(),
    }
}

/// Platform `--name` tokens cannot store-read (no principal to stamp).
pub fn platform_store_forbidden() -> CliResponse {
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: r#"{"error":"platform tokens cannot use store"}"#.to_string(),
    }
}

/// Host belt: `Host: skin.*` must never be a kingdom door even for
/// `token_ok` / Connect sessions. Teaching JSON, fail loud.
pub fn skin_host_forbidden(path: &str) -> Option<CliResponse> {
    let blocked = path == "/cli/sessions/grid"
        || path == "/cli/sessions/bytes"
        || path.starts_with("/cli/terminal/")
        || path == "/cli/auth/login"
        // Sew S28 §17 item 5: no chat pass or key route on a skin Host.
        || path.starts_with("/cli/sew/")
        || path.starts_with("/v1/");
    if !blocked {
        return None;
    }
    Some(CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: serde_json::json!({
            "error": skin_door::PATH_FILTER_ERROR,
            "hint": "Grid/PTY is not available with a skin token. Use Thread (and files if scoped).",
        })
        .to_string(),
    })
}

pub const THREAD_READ: &str = CAP_THREAD_READ;
pub const THREAD_POST: &str = CAP_THREAD_POST;
pub const FILES_READ: &str = CAP_FILES_READ;
pub const FILES_WRITE: &str = CAP_FILES_WRITE;
pub const ACTIVITY_READ: &str = CAP_ACTIVITY_READ;
pub const TICKETS_READ: &str = CAP_TICKETS_READ;
pub const TICKETS_POST: &str = CAP_TICKETS_POST;
pub const WIKI_READ: &str = CAP_WIKI_READ;
pub const STORE_READ: &str = CAP_STORE_READ;
pub const STORE_WRITE: &str = CAP_STORE_WRITE;

const OWNER_ONLY_HINT: &str = "requires owner/admin — ask your human (k2 app user add/remove/password, k2 app role create/update/remove, k2 app user role/unassign, k2 app token create/revoke/rooms; leftover k2 skin / k2 skin-token; use k2 app user list / k2 app role list / k2 app token list to read the roster). Host the UI with k2 publish, not k2 app.";

const OWNER_ONLY_MANAGE_HINT: &str = "requires owner/admin — ask your human (k2 app user add/remove/password, k2 app role create/update/remove, k2 app user role/unassign, k2 app token create/revoke/rooms; leftover k2 skin / k2 skin-token; use k2 app user list / k2 app role list / k2 app token list to read the roster). Host the UI with k2 publish, not k2 app. To let this workspace's agent manage Apps, Settings → Workspaces → (this workspace) → Agent → Allow this agent to manage Apps.";

fn owner_only_with_hint(hint: &str) -> CliResponse {
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": {
                "code": "owner_only",
                "hint": hint,
            },
        })
        .to_string(),
    }
}

/// Stable teaching response for leftover owner surfaces (front-door /
/// Hydra). CLI maps `owner_only` + 403 → exit 3. Missing/garbage
/// credentials stay [`CliResponse::forbidden`].
pub fn owner_only_response() -> CliResponse {
    owner_only_with_hint(OWNER_ONLY_HINT)
}

/// Teaching response for Skin Access **manage** mutations when a valid
/// scoped hook hits a workspace whose Agent-tab toggle is OFF.
pub fn owner_only_manage_response() -> CliResponse {
    owner_only_with_hint(OWNER_ONLY_MANAGE_HINT)
}

/// Actor label for a scoped hook that passed the manage gate.
/// `agent:<handle>` from [`workspace_address_name_shared`]. Never
/// `"owner"` / `"owner-token"`.
pub fn skin_manage_actor_label(principal: &crate::session_token::HookPrincipal) -> String {
    let handle = k2_core::workspace_session_handles::workspace_address_name_shared(
        principal.workspace_uuid.trim(),
    )
    .ok()
    .filter(|s| !s.is_empty())
    .or_else(|| {
        let addr = principal.agent_address.trim();
        if !addr.is_empty() {
            Some(addr.to_string())
        } else {
            None
        }
    })
    .unwrap_or_else(|| "agent".to_string());
    format!("agent:{handle}")
}

/// Attach the non-secret actor string to a 200 JSON object so tests
/// can assert it is not `"owner-token"` when a hook mutated.
pub fn stamp_actor(mut r: CliResponse, actor: &str) -> CliResponse {
    if !r.status.starts_with("200") {
        return r;
    }
    if let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&r.body) {
        if v.is_object() {
            v["actor"] = serde_json::json!(actor);
            r.body = v.to_string();
        }
    }
    r
}

fn enable_field(v: &serde_json::Value) -> Option<bool> {
    let raw = v.get("enable")?;
    if let Some(n) = raw.as_i64() {
        return match n {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        };
    }
    if let Some(s) = raw.as_str() {
        return match s.trim() {
            "0" => Some(false),
            "1" => Some(true),
            _ => None,
        };
    }
    None
}

/// `POST /cli/agents-manage-skin` `{project, enable: 0|1}` — owner-only
/// writer for `projects.agents_can_manage_skin`, which is switch 1 "Build
/// and run Apps" since 0.46.3: a wrapper over the one switch writer
/// (`apps::access::set_switches`: audit rows, the agent's note; off also
/// turns switch 2 off). Not `workspace/set`. `by` is the owner identity.
pub fn handle_agents_manage_skin(body: &[u8], by: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(project) = str_field(&v, &["project"]) else {
        return CliResponse::bad_request("missing project");
    };
    let Some(enable) = enable_field(&v) else {
        return CliResponse::bad_request("enable must be 0 or 1");
    };
    let Some(path) = crate::workspace_msg::resolve_workspace(project) else {
        return crate::workspace_routes::workspace_not_found_response(project);
    };
    let id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::workspace::agent_identity::resolve_project_id(&conn, &path)
    };
    let Some(id) = id else {
        return crate::workspace_routes::workspace_not_found_response(project);
    };
    let change = crate::apps::access::SwitchChange { build: Some(enable), full: None };
    match crate::apps::access::set_switches(&id, change, true, by) {
        Ok(v) => {
            k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
                "agent-access-set",
                by,
                format!("ok {} apps_build={enable} (agents-manage-skin)", v["workspace"].as_str().unwrap_or("?")),
                "cli",
                "-",
                "cli",
            ));
            CliResponse::ok_json(
                serde_json::json!({
                    "success": true,
                    "agentsCanManageSkin": enable,
                    "agentsCanManageAppsFull": v["agentsCanManageAppsFull"],
                })
                .to_string(),
            )
        }
        Err(e) => CliResponse::bad_request(e.hint),
    }
}

const LOGIN_FAILED_JSON: &str = r#"{"error":"invalid username or password"}"#;

fn login_failed_json() -> CliResponse {
    CliResponse {
        status: "401 Unauthorized",
        content_type: "application/json",
        body: LOGIN_FAILED_JSON.to_string(),
    }
}

fn login_failed_html() -> CliResponse {
    CliResponse {
        status: "401 Unauthorized",
        content_type: "text/html; charset=utf-8",
        body: "<!DOCTYPE html><html><body><p>invalid username or password</p></body></html>"
            .to_string(),
    }
}

pub struct SkinLoginReply {
    pub response: CliResponse,
    /// Raw `k2skn_` for Set-Cookie (and JSON `token`). Never the hash.
    pub token: Option<String>,
    /// HTML form success → 302 Location. JSON leaves this `None`.
    pub location: Option<String>,
    /// A locked login (423): seconds until it may try again, sent as
    /// `Retry-After`. `None` on every other reply.
    pub retry_after: Option<i64>,
}

/// Machine-readable lockout reply (nsi BFF, 2026-10-13): `423 Locked`
/// `{"error":"locked","retryAfterSecs":N}`. Given for any presented
/// username/email with a live lock — known principal or not — because the
/// lock is counted per presented string, so it is no username oracle. It
/// is checked before the password, so it is no password oracle either.
pub const LOGIN_LOCKED_ERROR: &str = "locked";

fn login_locked(form: bool, retry_after_secs: i64) -> SkinLoginReply {
    let mins = (retry_after_secs + 59) / 60;
    let response = if form {
        CliResponse {
            status: "423 Locked",
            content_type: "text/html; charset=utf-8",
            body: format!(
                "<!DOCTYPE html><html><body><p>Too many tries. Try again in {mins} minute(s).</p></body></html>"
            ),
        }
    } else {
        CliResponse {
            status: "423 Locked",
            content_type: "application/json",
            body: serde_json::json!({
                "error": LOGIN_LOCKED_ERROR,
                "retryAfterSecs": retry_after_secs,
            })
            .to_string(),
        }
    };
    SkinLoginReply {
        response,
        token: None,
        location: None,
        retry_after: Some(retry_after_secs),
    }
}

fn is_form_body(content_type: &str) -> bool {
    let ct = content_type.to_ascii_lowercase();
    ct.contains("application/x-www-form-urlencoded") || ct.contains("multipart/form-data")
}

fn percent_decode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let bytes = raw.replace('+', " ").into_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = &bytes[i + 1..i + 3];
            if let Ok(s) = std::str::from_utf8(hex) {
                if let Ok(v) = u8::from_str_radix(s, 16) {
                    out.push(v as char);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn parse_form_pairs(body: &[u8]) -> Vec<(String, String)> {
    let s = String::from_utf8_lossy(body);
    s.split('&')
        .filter_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            Some((percent_decode(k.trim()), percent_decode(v.trim())))
        })
        .collect()
}

fn form_field<'a>(pairs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// Relative path only — block `//host` and scheme URLs.
pub fn safe_next(raw: Option<&str>) -> String {
    let t = raw.unwrap_or("/").trim();
    if t.starts_with('/')
        && !t.starts_with("//")
        && !t.contains("://")
        && !t.contains('\r')
        && !t.contains('\n')
    {
        t.to_string()
    } else {
        "/".to_string()
    }
}

/// Public `POST /cli/skin/login`. Generic 401. Dummy argon2 + lockout live
/// in core (`check_and_record_login`); caller adds the 500 ms 401 delay.
pub fn handle_login(body: &[u8], content_type: &str) -> SkinLoginReply {
    handle_login_on(body, content_type, None)
}

/// [`handle_login`] for a login that came through an App's gateway link
/// (`link_app` = that App's service id): only there may a test guest of
/// that App sign in (`skin::test_guests`, 0.46.4).
pub fn handle_login_on(body: &[u8], content_type: &str, link_app: Option<&str>) -> SkinLoginReply {
    skin::test_guests::with_link_login(link_app, || login_inner(body, content_type))
}

fn login_inner(body: &[u8], content_type: &str) -> SkinLoginReply {
    let form = is_form_body(content_type);
    let parsed = if form {
        let pairs = parse_form_pairs(body);
        let username = form_field(&pairs, "username").unwrap_or("").to_string();
        let password = form_field(&pairs, "password").unwrap_or("").to_string();
        let next = form_field(&pairs, "next").map(str::to_string);
        let app = form_field(&pairs, "app").map(str::to_string);
        Some((username, password, next, app, None))
    } else {
        match serde_json::from_slice::<serde_json::Value>(body) {
            Ok(v) => {
                let username = str_field(&v, &["username", "name"])
                    .unwrap_or("")
                    .to_string();
                let password = v
                    .get("password")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                let next = str_field(&v, &["next"]).map(str::to_string);
                let app = str_field(&v, &["app", "appId"]).map(str::to_string);
                // app-mobile-access-v1: `device` asks for a device sign-in
                // (a refresh token beside the pass). A bad shape is 400
                // before any password work.
                let device = match skin::app_login::parse_device(v.get("device")) {
                    Ok(d) => d,
                    Err(e) => {
                        return SkinLoginReply {
                            response: CliResponse::bad_request(e),
                            token: None,
                            location: None,
                            retry_after: None,
                        }
                    }
                };
                Some((username, password, next, app, device))
            }
            Err(_) => None,
        }
    };
    let Some((username, password, next, app, device)) = parsed else {
        return SkinLoginReply {
            response: if form {
                login_failed_html()
            } else {
                login_failed_json()
            },
            token: None,
            location: None,
            retry_after: None,
        };
    };
    if username.trim().is_empty() || password.is_empty() {
        return SkinLoginReply {
            response: if form {
                login_failed_html()
            } else {
                login_failed_json()
            },
            token: None,
            location: None,
            retry_after: None,
        };
    }
    let principal = match skin::check_and_record_login(&username, &password) {
        skin::SkinLoginOutcome::Ok(p) => p,
        skin::SkinLoginOutcome::LockedOut { retry_after_secs } => {
            return login_locked(form, retry_after_secs);
        }
        skin::SkinLoginOutcome::BadCreds => {
            return SkinLoginReply {
                response: if form {
                    login_failed_html()
                } else {
                    login_failed_json()
                },
                token: None,
                location: None,
                retry_after: None,
            };
        }
    };
    // S24-9: no `app` = the box login (`host`), exactly as before 0.45.5 —
    // BFFs that name no App keep working. With `app`, the pass is bound to
    // that App and needs this person's enabled grant for it. An unknown or
    // removed App is the same generic 401 as a bad password.
    let app = skin::normalize_app_target(app.as_deref());
    // Device sign-in (app-mobile-access-v1): the pass plus a rotating
    // refresh token, when the App has "Stay signed in" on. Off (0 days) or
    // no device = exactly today's login.
    if let (Some(device), false) = (device.as_ref(), form) {
        if !login_app_is_live(&app) {
            return SkinLoginReply { response: login_failed_json(), token: None, location: None, retry_after: None };
        }
        match skin::app_login::login_device(&principal.username, &app, device) {
            Ok(Some(d)) => {
                let mut body = crate::app_mobile_routes::device_session_json(&d, &principal.id);
                body["rooms"] = serde_json::json!(d.session.meta.rooms);
                body["roomHandles"] = serde_json::json!(d.session.meta.room_handles);
                body["caps"] = serde_json::json!(d.session.meta.caps);
                body["role"] = serde_json::json!(d.session.role_name);
                body["roomAccess"] = serde_json::json!(d.session.meta.room_access);
                let raw = d.session.raw.clone();
                return SkinLoginReply {
                    response: CliResponse::ok_json(body.to_string()),
                    token: Some(raw),
                    location: None,
                    retry_after: None,
                };
            }
            Ok(None) => {}
            Err(e) if e == skin::HOST_LOGIN_DENIED => {
                return SkinLoginReply { response: login_failed_json(), token: None, location: None, retry_after: None };
            }
            Err(e) => {
                return SkinLoginReply { response: CliResponse::internal_error(e), token: None, location: None, retry_after: None };
            }
        }
    }
    let minted = if login_app_is_live(&app) {
        skin::create_app_session_token(&principal.username, &app)
    } else {
        Err(skin::HOST_LOGIN_DENIED.to_string())
    };
    let session = match minted {
        Ok(v) => v,
        Err(e) if e == skin::HOST_LOGIN_DENIED => {
            return SkinLoginReply {
                response: if form {
                    login_failed_html()
                } else {
                    login_failed_json()
                },
                token: None,
                location: None,
                retry_after: None,
            };
        }
        Err(e) => {
            return SkinLoginReply {
                response: CliResponse::internal_error(e),
                token: None,
                location: None,
                retry_after: None,
            };
        }
    };
    let skin::SkinAppSession {
        meta,
        raw,
        app,
        role_name,
    } = session;
    if form {
        let location = safe_next(next.as_deref());
        return SkinLoginReply {
            response: CliResponse {
                status: "302 Found",
                content_type: "text/html; charset=utf-8",
                body: "<!DOCTYPE html><html><body>ok</body></html>".to_string(),
            },
            token: Some(raw),
            location: Some(location),
            retry_after: None,
        };
    }
    // `role` is the role on the grant this login used (AM1). For a host
    // login that equals the assigned role, as before 0.45.5. `app` is new.
    let body = serde_json::json!({
        "ok": true,
        "token": raw.clone(),
        "username": principal.username,
        "principalId": principal.id,
        "rooms": meta.rooms,
        "roomHandles": meta.room_handles,
        "caps": meta.caps,
        "role": role_name,
        "roomAccess": meta.room_access,
        "app": app,
    })
    .to_string();
    debug_assert!(
        !body.contains("password_hash") && !body.contains("passwordHash"),
        "password_hash must never be on the wire"
    );
    debug_assert!(
        serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .map_or(true, |v| v.get("email").is_none()),
        "login JSON must not include email"
    );
    SkinLoginReply {
        response: CliResponse::ok_json(body),
        token: Some(raw),
        location: None,
        retry_after: None,
    }
}

/// Session `k2skn_` only. Static partner key → 403, not revoked.
pub fn handle_logout(presented: Option<&str>) -> CliResponse {
    let Some(raw) = presented.map(str::trim).filter(|s| !s.is_empty()) else {
        return CliResponse {
            status: "401 Unauthorized",
            content_type: "application/json",
            body: r#"{"error":"invalid or revoked skin token"}"#.to_string(),
        };
    };
    let Some(pass) = skin::resolve_skin_token(raw) else {
        return CliResponse {
            status: "401 Unauthorized",
            content_type: "application/json",
            body: r#"{"error":"invalid or revoked skin token"}"#.to_string(),
        };
    };
    if !pass.session {
        return CliResponse {
            status: "403 Forbidden",
            content_type: "application/json",
            body: r#"{"error":"static skin tokens cannot be logged out"}"#.to_string(),
        };
    }
    match skin::revoke_token(&pass.id) {
        Ok(_) => CliResponse::ok_json(r#"{"ok":true}"#.to_string()),
        Err(e) => CliResponse::internal_error(e),
    }
}

const FORGOT_OK_JSON: &str = r#"{"ok":true}"#;
const RESET_FAILED_JSON: &str = r#"{"error":"invalid or expired reset token"}"#;
const CHANGE_FAILED_JSON: &str = r#"{"error":"invalid password"}"#;
const FORGOT_THROTTLED_JSON: &str = r#"{"error":"too many requests"}"#;
const FORGOT_FORBIDDEN_JSON: &str = r#"{"error":"forbidden"}"#;
const FORGOT_UNAUTHORIZED_JSON: &str = r#"{"error":"invalid or missing auth token"}"#;

fn forgot_unauthorized() -> CliResponse {
    CliResponse {
        status: "401 Unauthorized",
        content_type: "application/json",
        body: FORGOT_UNAUTHORIZED_JSON.to_string(),
    }
}

fn forgot_forbidden() -> CliResponse {
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: FORGOT_FORBIDDEN_JSON.to_string(),
    }
}

fn reset_failed() -> CliResponse {
    CliResponse {
        status: "401 Unauthorized",
        content_type: "application/json",
        body: RESET_FAILED_JSON.to_string(),
    }
}

fn change_failed() -> CliResponse {
    CliResponse {
        status: "401 Unauthorized",
        content_type: "application/json",
        body: CHANGE_FAILED_JSON.to_string(),
    }
}

/// Owner-ROLE or platform `--name`. Guest session / hook / Connect member
/// 403 before lookup. Missing/invalid → 401. Does not use `token_ok`.
fn forgot_auth(query: &str, owner_token: &str) -> Result<(), CliResponse> {
    let presented = crate::routes::http::extract_token(query).unwrap_or("");
    if presented.is_empty() {
        return Err(forgot_unauthorized());
    }
    if crate::routes::http::owner_role_identity(query, owner_token).is_some() {
        return Ok(());
    }
    if skin::is_skin_token(presented) {
        return match skin::resolve_skin_token(presented) {
            Some(pass) if !pass.session => Ok(()),
            Some(_) => Err(forgot_forbidden()),
            None => Err(forgot_unauthorized()),
        };
    }
    if crate::session_token::validate_hook(presented).is_some() {
        return Err(owner_only_response());
    }
    if k2_core::connect_users::validate_session(presented).is_some() {
        return Err(forgot_forbidden());
    }
    Err(forgot_unauthorized())
}

/// `POST /cli/skin/password/forgot` `{username}` — owner | platform `--name`.
pub fn handle_password_forgot(body: &[u8], query: &str, owner_token: &str) -> CliResponse {
    if let Err(r) = forgot_auth(query, owner_token) {
        return r;
    }
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let username = v
        .get("username")
        .or_else(|| v.get("name"))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    match skin::password_reset_throttled(&username) {
        Ok(true) => {
            return CliResponse {
                status: "429 Too Many Requests",
                content_type: "application/json",
                body: FORGOT_THROTTLED_JSON.to_string(),
            };
        }
        Ok(false) => {}
        Err(e) => return CliResponse::internal_error(e),
    }
    match skin::mint_password_reset(&username) {
        Ok(Some(mint)) => {
            let body = serde_json::json!({
                "ok": true,
                "token": mint.token,
                "expiresAt": mint.expires_at,
                "email": mint.email,
            })
            .to_string();
            debug_assert!(
                !body.contains("k2skn_"),
                "reset mint must not be a skin pass"
            );
            CliResponse::ok_json(body)
        }
        Ok(None) => CliResponse::ok_json(FORGOT_OK_JSON.to_string()),
        Err(e) => CliResponse::internal_error(e),
    }
}

/// Public `POST /cli/skin/password/reset` `{token, password}`. Ignore
/// presented auth. Empty or `<8` → 400. Bad/expired/used → 401 + dummy.
/// Does not mint a session. Caller adds the 500 ms 401 delay.
pub fn handle_password_reset(body: &[u8]) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(token) = str_field(&v, &["token"]) else {
        return CliResponse::bad_request("missing token");
    };
    let password = v.get("password").and_then(|x| x.as_str()).unwrap_or("");
    if password.is_empty() || password.len() < skin::PUBLIC_PASSWORD_MIN_LEN {
        return CliResponse::bad_request("password must be at least 8 characters");
    }
    match skin::consume_password_reset(token, password) {
        Ok(skin::SkinResetConsume::Ok) => {
            let out = r#"{"ok":true}"#.to_string();
            debug_assert!(
                !out.contains("k2skn_") && !out.contains("token"),
                "consume must not mint a session"
            );
            CliResponse::ok_json(out)
        }
        Ok(skin::SkinResetConsume::BadToken) => reset_failed(),
        Err(e) => CliResponse::internal_error(e),
    }
}

/// Session `k2skn_` only. Body `{oldPassword, password}` — no username.
/// Platform 403. Success revokes all session=1 including caller.
pub fn handle_password_change(body: &[u8], presented: Option<&str>) -> CliResponse {
    let Some(raw) = presented.map(str::trim).filter(|s| !s.is_empty()) else {
        return CliResponse {
            status: "401 Unauthorized",
            content_type: "application/json",
            body: r#"{"error":"invalid or revoked skin token"}"#.to_string(),
        };
    };
    let Some(pass) = skin::resolve_skin_token(raw) else {
        return CliResponse {
            status: "401 Unauthorized",
            content_type: "application/json",
            body: r#"{"error":"invalid or revoked skin token"}"#.to_string(),
        };
    };
    if !pass.session {
        return CliResponse {
            status: "403 Forbidden",
            content_type: "application/json",
            body: r#"{"error":"static skin tokens cannot change password"}"#.to_string(),
        };
    }
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(old) = str_field(&v, &["oldPassword", "old_password"]) else {
        return CliResponse::bad_request("missing oldPassword");
    };
    let password = v.get("password").and_then(|x| x.as_str()).unwrap_or("");
    if password.is_empty() || password.len() < skin::PUBLIC_PASSWORD_MIN_LEN {
        return CliResponse::bad_request("password must be at least 8 characters");
    }
    if !skin::verify_principal_password(&pass.username, old) {
        return change_failed();
    }
    match skin::set_principal_password(&pass.username, Some(password)) {
        Ok(_) => CliResponse::ok_json(r#"{"ok":true}"#.to_string()),
        Err(e) => CliResponse::internal_error(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Q5 (S24-12): an agent that may manage Apps keeps exactly its 0.45.4
    /// powers — naming an App on user add is owner-only. Refused before any
    /// store is touched.
    #[test]
    fn agent_cannot_name_an_app_on_user_add() {
        assert!(!AGENTS_MAY_NAME_APP, "Q5 default for 0.45.5");
        assert!(actor_is_agent("agent:sales"));
        assert!(!actor_is_agent("owner-token"));
        assert!(!actor_is_agent("user:rosson"));
        let r = handle_users_post(
            br#"{"username":"eve","app":"0f5e7c1a-3b2d-4e6f-8a9b-1c2d3e4f5a6b"}"#,
            "agent:sales",
        );
        assert!(r.status.starts_with("403"), "{} {}", r.status, r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["error"]["code"], "owner_only", "{v}");
        let r = handle_users_post(br#"{"username":"eve","role":"x"}"#, "owner-token");
        assert!(r.status.starts_with("400"), "role needs app: {}", r.body);
    }

    /// GH#72: an agent can't mint the `.k2/` write opt-in on a platform
    /// pass. Refused before any store is touched.
    #[test]
    fn agent_cannot_mint_the_agent_config_opt_in() {
        let r = handle_tokens_post(
            br#"{"name":"agent-tok","caps":["files:write","files:agentconfig"],"rooms":["sales"]}"#,
            "agent:sales",
        );
        assert_eq!(r.status, "403 Forbidden", "{}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["error"]["code"], "owner_only", "{v}");
        assert!(
            v["error"]["hint"].as_str().expect("hint").contains("files:agentconfig"),
            "{v}"
        );
    }

    #[test]
    fn full_name_sanitizer_caps_at_64_and_strips_controls() {
        let dirty = format!("A\u{1b}\n{}", "x".repeat(80));
        let clean = cleaned_full_name(Some(&dirty)).expect("usable name remains");
        assert!(!clean.contains('\u{1b}'), "ESC must not survive: {clean:?}");
        assert!(!clean.contains('\n'), "newline must not survive: {clean:?}");
        assert!(
            !clean.chars().any(|c| c.is_control()),
            "controls must not survive: {clean:?}"
        );
        assert_eq!(clean.chars().count(), 64, "{clean:?}");
        assert!(clean.starts_with("Ax"), "{clean:?}");
        assert_eq!(
            cleaned_full_name(Some("Ada/Lovelace: MD")).as_deref(),
            Some("Ada/Lovelace: MD"),
            "slash and colon stay (not validate_display_name)"
        );
        assert_eq!(cleaned_full_name(Some("  Ada  ")).as_deref(), Some("Ada"));
        assert!(cleaned_full_name(Some("\u{1b}\n\t")).is_none());
        assert!(cleaned_full_name(Some("   ")).is_none());
        assert!(cleaned_full_name(None).is_none());
    }

    #[test]
    fn safe_next_rejects_open_redirects() {
        assert_eq!(safe_next(Some("https://evil.example")), "/");
        assert_eq!(safe_next(Some("//evil")), "/");
        assert_eq!(safe_next(Some("/app")), "/app");
    }

    #[test]
    fn login_failed_bodies_are_generic() {
        let a = login_failed_json();
        let b = login_failed_json();
        assert_eq!(a.body, b.body);
        assert_eq!(a.status, "401 Unauthorized");
        assert!(!a.body.contains("guest"));
        assert_eq!(a.body, LOGIN_FAILED_JSON);
    }

    #[test]
    fn reset_and_change_failures_are_generic() {
        let a = reset_failed();
        let b = reset_failed();
        assert_eq!(a.status, "401 Unauthorized");
        assert_eq!(a.body, b.body);
        assert_eq!(a.body, RESET_FAILED_JSON);
        assert!(!a.body.contains("expired") || a.body.contains("invalid or expired"));
        let c = change_failed();
        assert_eq!(c.status, "401 Unauthorized");
        assert_eq!(c.body, CHANGE_FAILED_JSON);
        assert!(!c.body.contains("username"));
        assert!(!c.body.contains("k2skn_"));
    }

    #[test]
    fn owner_only_response_has_stable_code_and_403() {
        let r = owner_only_response();
        assert_eq!(r.status, "403 Forbidden");
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"]["code"], "owner_only");
        let hint = v["error"]["hint"].as_str().unwrap_or("");
        assert!(
            hint.contains("owner") || hint.contains("human"),
            "hint should teach owner/human: {hint}"
        );
        assert!(
            !hint.contains("Allow this agent to manage Apps")
                && !hint.contains("Allow this agent to manage Skin Access"),
            "leftover/hydra keep today's hint without the Agent-tab sentence: {hint}"
        );
        assert!(
            !r.body.contains("Invalid or missing auth token"),
            "must not look like a broken passport: {}",
            r.body
        );
    }

    #[test]
    fn owner_only_manage_response_adds_agent_tab_sentence() {
        let r = owner_only_manage_response();
        assert_eq!(r.status, "403 Forbidden");
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["error"]["code"], "owner_only");
        let hint = v["error"]["hint"].as_str().expect("hint");
        assert!(
            hint.contains("Allow this agent to manage Apps"),
            "manage OFF must teach the Agent-tab toggle: {hint}"
        );
        assert!(
            !r.body.contains("gated"),
            "must stay owner_only, not gated: {}",
            r.body
        );
    }

    #[test]
    fn an_agent_added_guest_that_cant_be_stamped_is_removed_again() {
        let _home = crate::test_support::TempHome::new();
        let exists = |u: &str| skin::list_principals().expect("principals").iter().any(|p| p.username == u);
        // The answer has no id: removed (by the answer's username).
        skin::add_principal("nit-noid").expect("add");
        let r = stamp_agent_added(CliResponse::ok_json(r#"{"username":"nit-noid"}"#.to_string()), "ws-nit", Some("nit-noid"), "agent:nit");
        assert_eq!(r.status, "500 Internal Server Error", "{}", r.body);
        assert!(!exists("nit-noid"), "an unstamped guest never stays behind");
        // No id and no username in the answer: removed by the name asked for.
        skin::add_principal("nit-bare").expect("add");
        let r = stamp_agent_added(CliResponse::ok_json("{}".to_string()), "ws-nit", Some("Nit-Bare"), "agent:nit");
        assert_eq!(r.status, "500 Internal Server Error", "{}", r.body);
        assert!(!exists("nit-bare"), "removed by the requested name");
        // A full answer: stamped and kept.
        let p = skin::add_principal("nit-ok").expect("add");
        let body = serde_json::to_string(&p).expect("json");
        let r = stamp_agent_added(CliResponse::ok_json(body), "ws-nit", Some("nit-ok"), "agent:nit");
        assert_eq!(r.status, "200 OK", "{}", r.body);
        assert!(exists("nit-ok"));
        assert_eq!(skin::creators::workspace_of(&p.id).expect("stamp lookup").as_deref(), Some("ws-nit"));
        // A refusal passes through untouched.
        let r = stamp_agent_added(CliResponse::bad_request("nope"), "ws-nit", Some("nit-ok"), "agent:nit");
        assert!(r.status.starts_with("400"), "{}", r.status);
        assert!(exists("nit-ok"));
    }

    #[test]
    fn skin_manage_actor_label_is_agent_handle_never_owner_token() {
        k2_core::db::init_for_tests();
        let db = k2_core::db::shared();
        let conn = db.lock();
        let id = uuid::Uuid::new_v4().to_string();
        let handle = format!("sales{}", &id[..8]);
        let path = format!("/tmp/k2-skin-actor-{id}");
        conn.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
            rusqlite::params![id, handle, path],
        )
        .expect("seed");
        drop(conn);
        let principal = crate::session_token::HookPrincipal {
            workspace_uuid: id,
            agent_address: "sidecar".to_string(),
        };
        let actor = skin_manage_actor_label(&principal);
        assert_eq!(actor, format!("agent:{handle}"));
        assert_ne!(actor, "owner");
        assert_ne!(actor, "owner-token");
    }

    #[test]
    fn stamp_actor_skips_non_200_and_sets_field_on_ok() {
        let denied = owner_only_manage_response();
        let stamped = stamp_actor(denied, "agent:sales");
        assert!(!stamped.body.contains("agent:sales"), "{}", stamped.body);
        let ok = CliResponse::ok_json(r#"{"username":"bob"}"#.to_string());
        let stamped = stamp_actor(ok, "agent:sales");
        let v: serde_json::Value = serde_json::from_str(&stamped.body).expect("json");
        assert_eq!(v["actor"], "agent:sales");
        assert_eq!(v["username"], "bob");
    }

    #[test]
    fn hydra_get_unsupported_on_non_linux() {
        let _g = crate::skin_hydra::hydra_test_lock();
        k2_core::db::init_for_tests();
        {
            let db = k2_core::db::shared();
            let _ = db.lock().execute("DELETE FROM skin_hydra", []);
        }
        crate::skin_hydra::stop();
        if cfg!(target_os = "linux") {
            return;
        }
        let r = handle_hydra_get();
        assert_eq!(r.status, "200 OK");
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["supported"], false, "{}", r.body);
        assert_eq!(v["enabled"], false, "{}", r.body);
        assert_eq!(v["running"], false, "{}", r.body);
        assert!(
            v["hint"].as_str().unwrap_or("").contains("LINUX"),
            "{}",
            r.body
        );
    }

    #[test]
    fn hydra_post_off_is_not_running() {
        let _g = crate::skin_hydra::hydra_test_lock();
        k2_core::db::init_for_tests();
        {
            let db = k2_core::db::shared();
            let _ = db.lock().execute("DELETE FROM skin_hydra", []);
        }
        let r = handle_hydra_post(br#"{"enabled":false,"apply":true}"#, "owner");
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["enabled"], false, "{}", r.body);
        assert_eq!(v["running"], false, "{}", r.body);
    }

    #[test]
    fn hydra_post_requires_enabled() {
        let r = handle_hydra_post(br#"{}"#, "owner");
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("enabled"), "{}", r.body);
    }
}
