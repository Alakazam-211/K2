//! `app.login-grant op=add|remove|role user= app= role=` (Rosson 2026-10-30):
//! an App's own agent asks to give one of the box's App users a login on its
//! App, with a role, or to take one away; or (0.46.16) to move a login it
//! already has onto another role in place (`op=role`), which keeps their
//! sessions (remove then add would end them).
//!
//! `op=role` has no wall of its own: `k2 app grant role` is the agent's own
//! with "Let this agent manage its Apps' users", and without it that refusal
//! (`app_users_off`) asks for the switch. This is the one-time ask for an
//! agent whose owner keeps the switch off. It is an op of this action, not an
//! action of its own: the same values (person, App, role), the same scope,
//! the same floor and answer, and one `user=`/`app=` pair names the login
//! (a person has at most one login per App).
//!
//! The wall: `k2 app grant add|rm --subject-kind principal --subject <user>
//! --kind app --target <app> [--role <role>]` (`POST /cli/skin/grants`,
//! `/cli/skin/grants/delete`) answers an agent `owner_only`. With cards on,
//! that refusal ends with the line to ask with. Only `kind=app` with a
//! principal subject is a wall of this card; every other grant stays "It
//! can't be requested."
//!
//! - **Who asks:** only for an App (a published service) of the asking
//!   workspace, found in that workspace by name or id; only for a person
//!   who already exists on the box; only a role that exists and is box-wide
//!   or that App's own. Never an agent's login (an agent can't give itself
//!   or another agent access this way) and never a box login (a person with
//!   a `host` grant stays the owner's, as everywhere else an agent touches
//!   App users).
//! - **Who answers:** who the route admits today: the owner token or an
//!   Owner login (`owner_role_identity`, the route policy's `Owner` row).
//!   An Admin can't add an App login from Settings or a terminal either, so
//!   the card doesn't let one; never an agent.
//! - **Proof:** a click, the same as the route (it takes no confirm).
//! - **On Allow:** `apply` runs the route's own handler as the person who
//!   clicked (`owner-token` / `user:<name>`), by the person's and the App's
//!   ids, so the route's own checks run again at the click. Once: the
//!   answer is a compare-and-set.
//! - **The words** name the person, the App, the role and what that role
//!   lets them do, room by room, in plain words, computed from the role as
//!   it is when asked. If the role changes before the click, the card is
//!   stale and nothing is applied.

use std::collections::HashMap;

use k2_core::permission::catalog::{
    ActionSpec, Area, Direction, Expiry, Floor, Grant, NotCardableSpec, ParamFrom, ParamKind, ParamSpec, Proof, SettingsRef, Undo,
    Wall, WallCoverage, WallMatch, WallMethod, Words,
};
use k2_core::skin::{self, SkinGrant, SkinPrincipal, SkinRole};
use serde_json::{json, Value};

use super::held::{error_of, route_floor, text_param};
use super::vault::{own_service, OwnService};
use super::{Answerer, Applied, Check, Ctx, Registered};

pub(crate) const ACTION: &str = "app.login-grant";
pub(crate) const ROUTE_ADD: &str = "/cli/skin/grants";
pub(crate) const ROUTE_REMOVE: &str = "/cli/skin/grants/delete";
pub(crate) const ROUTE_ROLE: &str = "/cli/skin/grants/role";
pub(crate) const OPS: [&str; 3] = ["add", "remove", "role"];
/// The route's own floor (`owner_role_identity`): the owner.
pub(crate) const FLOOR: Floor = Floor::Owner;

pub(crate) static LOGIN_GRANT: ActionSpec = ActionSpec {
    id: ACTION,
    area: Area::Apps,
    params: &[
        ParamSpec { key: "op", kind: ParamKind::Choice, required: true, list: false, choices: &OPS },
        ParamSpec { key: "user", kind: ParamKind::Principal, required: true, list: false, choices: &[] },
        ParamSpec { key: "app", kind: ParamKind::App, required: true, list: false, choices: &[] },
        ParamSpec { key: "role", kind: ParamKind::AppRole, required: false, list: false, choices: &[] },
    ],
    floor: FLOOR,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "{user}'s login on {app}",
        what_changes: "{what}",
        consequences: "{effects}",
        scope: "{user} on {app}",
        duration: "until someone changes it",
        retry: "",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "people", path: "Settings → User Access" },
    walls: &[
        Wall {
            matches: WallMatch::Code { status: 403, code: "owner_only" },
            route_prefix: ROUTE_ADD,
            method: WallMethod::Post,
            params: &[("user", ParamFrom::Body("/subjectId")), ("app", ParamFrom::Body("/targetId")), ("role", ParamFrom::Body("/roleId"))],
        },
        Wall {
            matches: WallMatch::Code { status: 403, code: "owner_only" },
            route_prefix: ROUTE_REMOVE,
            method: WallMethod::Post,
            params: &[("user", ParamFrom::Body("/subjectId")), ("app", ParamFrom::Body("/targetId"))],
        },
    ],
    coverage: WallCoverage::PrefixOnly,
    form: None,
    machine_hints: &[],
};

/// One role capability in plain words (unknown ones as they are).
pub(crate) fn cap_words(cap: &str) -> String {
    match cap {
        "thread:read" => "read the Thread",
        "thread:post" => "post in the Thread",
        "files:read" => "see files",
        "files:write" => "change files",
        "files:download" => "download files",
        "files:upload" => "upload files",
        "files:agentconfig" => "change the agent's own files",
        "tickets:read" => "see tickets",
        "tickets:post" => "file tickets",
        "wiki:read" => "read the wiki",
        "store:read" => "read the App's data",
        "store:write" => "change the App's data",
        "activity:read" => "see what the agent is doing",
        "heartbeats:read" => "see heartbeats",
        "heartbeats:write" => "change heartbeats",
        "agent:identity" => "change the agent's name and picture",
        "agent:handle" => "change the agent's handle",
        "room:members" => "see who else is in the room",
        "project:write" => "change the workspace's settings",
        "push:send" => "send push notifications",
        "email:send" => "send email",
        "chat:read" => "read chat",
        "chat:post" => "post in chat",
        "chat:respond" => "answer in chat",
        other => return other.to_string(),
    }
    .to_string()
}

fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// What a role lets a person do, room by room.
pub(crate) fn role_allows(role: &SkinRole) -> String {
    let rooms: Vec<String> = role
        .room_access
        .iter()
        .map(|r| {
            let caps: Vec<String> = r.caps.iter().map(|c| cap_words(c)).collect();
            if caps.is_empty() {
                format!("in {}: nothing", r.handle)
            } else {
                format!("in {}: {}", r.handle, and_list(&caps))
            }
        })
        .collect();
    if rooms.is_empty() {
        "sign in, and nothing more (the role has no rooms)".to_string()
    } else {
        rooms.join("; ")
    }
}

/// The person `user` names (a username or an id), anywhere on the box.
pub(crate) fn principal(user: &str) -> Option<SkinPrincipal> {
    let want = user.trim().to_ascii_lowercase();
    skin::list_principals().ok()?.into_iter().find(|p| p.username == want || p.id == user.trim())
}

pub(crate) fn role_named(token: &str) -> Option<SkinRole> {
    let t = token.trim();
    skin::list_roles().ok()?.into_iter().find(|r| r.id == t || r.name.eq_ignore_ascii_case(t))
}

pub(crate) fn login_on(p: &SkinPrincipal, svc: &OwnService) -> Option<SkinGrant> {
    skin::list_grants().ok()?.into_iter().find(|g| g.subject_kind == "principal" && g.subject_id == p.id && g.kind == "app" && g.target_id == svc.id)
}

/// What one request does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Add,
    Remove,
    /// Move the login they have onto another role, in place.
    Role,
}

/// The checked values of one request.
struct Login {
    op: Op,
    person: SkinPrincipal,
    svc: OwnService,
    role: Option<SkinRole>,
}

fn login_of(ctx: &Ctx) -> Result<Login, String> {
    let op = text_param(ctx, "op").ok_or("say what: op=add, op=remove or op=role")?;
    let op = match op {
        "add" => Op::Add,
        "remove" => Op::Remove,
        "role" => Op::Role,
        other => return Err(format!("op={other}: add, remove or role")),
    };
    let user = text_param(ctx, "user").ok_or("name the person: user=<their username>")?;
    let app = text_param(ctx, "app").ok_or("name your App: app=<its name>")?;
    let svc = own_service(&ctx.workspace_id, app).ok_or_else(|| format!("{} has no App named {app}", ctx.workspace_handle))?;
    let person = principal(user).ok_or_else(|| format!("there's no App user named {user} on this server (k2 app user list)"))?;
    if skin::principal_agent_workspace(&person.id).map_err(|e| e.to_string())?.is_some() {
        return Err(format!("{} is an agent's login: an agent never gives an agent access to an App this way", person.username));
    }
    let grants = skin::list_grants().map_err(|e| e.to_string())?;
    if crate::skin_routes::has_box_login(&person.id, &grants) {
        return Err(format!(
            "{u} has a box login, so this card doesn't apply. For access to your App, ask the owner on this one instead: k2 permission request apps.user-grant-box user={u} app={app} role=<role> --why \"<what you need it for>\"",
            u = person.username,
        ));
    }
    let role = match (op, text_param(ctx, "role")) {
        (Op::Add | Op::Role, None) => return Err("name the role they get: role=<role> (k2 app role list)".into()),
        (Op::Remove, Some(_)) => return Err("role= goes with op=add or op=role".into()),
        (Op::Remove, None) => None,
        (Op::Add | Op::Role, Some(r)) => {
            let role = role_named(r).ok_or_else(|| format!("there's no role named {r} on this server (k2 app role list)"))?;
            if role.app_id.as_deref().is_some_and(|a| a != svc.id) {
                return Err(format!("the role {} belongs to another App", role.name));
            }
            Some(role)
        }
    };
    Ok(Login { op, person, svc, role })
}

/// The card's words from the stored values: `{user}`, `{app}` by name,
/// `{what}` and `{effects}` (what the role allows).
pub(crate) fn with_computed(action: &str, ctx: &Ctx) -> Option<Ctx> {
    if action != ACTION {
        return None;
    }
    let mut out = ctx.clone();
    let ws = ctx.workspace_handle.clone();
    let (what, effects) = match login_of(ctx) {
        Ok(l) => {
            out.params.insert("user".into(), json!(l.person.username));
            out.params.insert("app".into(), json!(l.svc.name));
            let (u, a) = (&l.person.username, &l.svc.name);
            match (&l.role, l.op) {
                (Some(r), Op::Role) => {
                    let now = login_on(&l.person, &l.svc);
                    let from = now
                        .as_ref()
                        .and_then(|g| g.role_id.as_deref())
                        .and_then(|rid| skin::list_roles().ok()?.into_iter().find(|x| x.id == rid))
                        .map(|x| x.name);
                    let back = match (&now, &from) {
                        (Some(g), Some(old)) => format!(" Put it back with k2 app grant role {} --role {old}.", g.id),
                        _ => String::new(),
                    };
                    (
                        match &from {
                            Some(old) => format!("Let {ws} move {u}'s login on its App {a} from the role {old} to the role {}", r.name),
                            None => format!("Let {ws} give {u}'s login on its App {a} the role {}", r.name),
                        },
                        format!(
                            "With the role {}, {u} can {}. Their login stays, and so do their open sessions: they get the new role at their next request.{back}",
                            r.name,
                            role_allows(r)
                        ),
                    )
                }
                (Some(r), _) => (
                    format!("Let {ws} give {u} a login on its App {a}, with the role {}", r.name),
                    format!(
                        "With the role {}, {u} can {}. They sign in to {a} with their own password, and {a}'s server can see it when they do. Remove it again with k2 app grant rm --subject-kind principal --subject {u} --kind app --target {a}.",
                        r.name,
                        role_allows(r)
                    ),
                ),
                (None, _) => (
                    format!("Let {ws} remove {u}'s login on its App {a}"),
                    format!("{u} can't sign in to {a} any more, and their open sessions there end. Their other Apps and their account stay."),
                ),
            }
        }
        Err(e) => (format!("Let {ws} change a login on its App"), e),
    };
    out.params.insert("what".into(), json!(what));
    out.params.insert("effects".into(), json!(effects));
    Some(out)
}

fn check(ctx: &Ctx) -> Check {
    let l = match login_of(ctx) {
        Ok(l) => l,
        Err(e) => return Check::Moved(e),
    };
    let same_role = |g: &SkinGrant| g.role_id.is_some() && g.role_id == l.role.as_ref().map(|r| r.id.clone()) && g.scope.is_none();
    match (l.op, login_on(&l.person, &l.svc)) {
        (Op::Add, Some(g)) if same_role(&g) && g.enabled => Check::AlreadyDone,
        (Op::Add, Some(_)) => Check::Moved(format!(
            "{u} already has a login on {a}: to change its role and keep their sessions, ask with op=role: k2 permission request app.login-grant op=role user={u} app={a} role={r} --why \"<what you need it for>\"",
            u = l.person.username,
            a = l.svc.name,
            r = l.role.as_ref().map(|r| r.name.as_str()).unwrap_or("<role>"),
        )),
        (Op::Add, None) => Check::Needed,
        (Op::Remove, Some(_)) => Check::Needed,
        (Op::Remove, None) => Check::AlreadyDone,
        (Op::Role, Some(g)) if same_role(&g) => Check::AlreadyDone,
        (Op::Role, Some(_)) => Check::Needed,
        (Op::Role, None) => Check::Moved(format!(
            "{u} has no login on {a} to move: ask with op=add instead: k2 permission request app.login-grant op=add user={u} app={a} role={r} --why \"<what you need it for>\"",
            u = l.person.username,
            a = l.svc.name,
            r = l.role.as_ref().map(|r| r.name.as_str()).unwrap_or("<role>"),
        )),
    }
}

/// The route's own actor label for who clicked (`owner_role_identity`).
fn actor_of(who: &Answerer) -> String {
    match who {
        Answerer::OwnerToken => "owner-token".to_string(),
        Answerer::Login { user, .. } => format!("user:{user}"),
    }
}

/// Through the route's own handler, as the person who clicked.
fn apply(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    let l = match login_of(ctx) {
        Ok(l) => l,
        Err(e) => return Applied::Refused { code: "bad_params".into(), message: e },
    };
    let existing = login_on(&l.person, &l.svc);
    let route = match l.op {
        Op::Add => ROUTE_ADD,
        Op::Remove => ROUTE_REMOVE,
        Op::Role => ROUTE_ROLE,
    };
    if let Err(refused) = route_floor(route, who) {
        return refused;
    }
    // `owner_role_identity`: the owner token or an Owner login, nobody else.
    if let Answerer::Login { user, .. } = who {
        let owner = k2_core::connect_users::role_for_user(user).is_some_and(k2_core::connect_users::can_change_roles);
        if !owner {
            return Applied::Refused { code: "owner_only".into(), message: "only the server's owner adds, removes or re-roles App logins".into() };
        }
    }
    let actor = actor_of(who);
    let mut body = json!({ "subjectKind": "principal", "subjectId": l.person.id, "kind": "app", "targetId": l.svc.id });
    let r = match (l.op, &l.role) {
        // The in-place path (`k2 app grant role`): the login keeps its id,
        // its on/off and its sessions.
        (Op::Role, Some(role)) => {
            let Some(g) = existing else {
                return Applied::Refused { code: "stale".into(), message: format!("{} has no login on {} any more", l.person.username, l.svc.name) };
            };
            crate::skin_routes::handle_grants_role(json!({ "id": g.id, "role": role.id }).to_string().as_bytes(), &actor)
        }
        (_, Some(role)) => {
            body["roleId"] = json!(role.id);
            crate::skin_routes::handle_grants_post(body.to_string().as_bytes(), &actor)
        }
        (_, None) => crate::skin_routes::handle_grants_delete(body.to_string().as_bytes(), &actor),
    };
    let v: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
    if !r.status.starts_with("200") {
        let (code, message) = error_of(&v, "refused");
        return Applied::Refused { code, message };
    }
    let (u, a) = (&l.person.username, &l.svc.name);
    Applied::Done {
        message: match (l.op, &l.role) {
            (Op::Role, Some(role)) => format!("{u}'s login on {a} has the role {} now; their sessions stayed.", role.name),
            (_, Some(role)) => format!("{u} has a login on {a} with the role {} now.", role.name),
            (_, None) => format!("{u}'s login on {a} is removed."),
        },
    }
}

/// `GET /cli/skin/grants/own`'s rows: the logins on one workspace's Apps.
pub(crate) fn own_logins(workspace_id: &str) -> Result<Vec<Value>, String> {
    let services: HashMap<String, String> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let mut st = conn.prepare("SELECT id, name FROM published_services WHERE project_id = ?1").map_err(|e| e.to_string())?;
        let rows = st
            .query_map(rusqlite::params![workspace_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())?
    };
    let people: HashMap<String, String> = skin::list_principals()?.into_iter().map(|p| (p.id, p.username)).collect();
    let roles: HashMap<String, String> = skin::list_roles()?.into_iter().map(|r| (r.id, r.name)).collect();
    Ok(skin::list_grants()?
        .into_iter()
        .filter(|g| g.kind == "app" && services.contains_key(&g.target_id))
        .map(|g| {
            let rooms: Vec<String> = g.scope.as_deref().and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default();
            json!({
                "id": g.id,
                "subjectKind": g.subject_kind,
                "subjectId": g.subject_id,
                "username": people.get(&g.subject_id),
                "kind": g.kind,
                "targetId": g.target_id,
                "app": services.get(&g.target_id),
                "roleId": g.role_id,
                "role": g.role_id.as_ref().and_then(|r| roles.get(r)),
                "rooms": skin::handles_for_project_ids(&rooms),
                "enabled": g.enabled,
                "createdAt": g.created_at,
            })
        })
        .collect())
}

pub(crate) fn actions() -> Vec<Registered> {
    vec![Registered { spec: &LOGIN_GRANT, check, apply }]
}

pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    Vec::new()
}

#[cfg(test)]
#[path = "app_login_tests.rs"]
mod tests;
