//! `apps.user-grant-box user= app= role=` (Rosson 2026-11-02, 0.46.8): an
//! App's own agent asks the owner to give a **box login** access to its App.
//!
//! An agent never changes a box login's access (`skin_routes::has_box_login`
//! refuses it on every route, and `app.login-grant` refuses it at the ask),
//! and that stays. This card is the owner's path that needs no terminal: the
//! owner reads who, which App, which role and what that role allows, and
//! taps Allow on the Thread.
//!
//! - **Who asks:** an agent passport, only for an App of its own workspace,
//!   a person who holds a box login (`host` grant) and isn't an agent's
//!   login, a role that is box-wide or that App's. Reached by asking (no
//!   wall of its own): `app.login-grant` and the box-login refusal name it.
//! - **Who answers:** the owner token or an Owner login, what the grant
//!   routes admit. Never an agent, never a tied App login
//!   (`overlay_routes::refuse_card_non_person`), never an Admin.
//! - **On Allow:** the route's own handler as the person who clicked, with
//!   the checked ids: `POST /cli/skin/grants` for a new login on the App,
//!   `POST /cli/skin/grants/role` when they already have one (their live
//!   sessions stay and follow). Nothing about the box login itself changes:
//!   the `host` grant is never touched, and the App's grant is the effect.
//! - **The words** are computed from the role as it is when asked, room by
//!   room. If the role changes before the click the card is stale.
//!
//! Chat through an App is a separate question: a box login's chat is its own
//! grant, and this card doesn't touch it.

use k2_core::permission::catalog::{
    ActionSpec, Area, Direction, Expiry, Floor, Grant, NotCardableSpec, ParamKind, ParamSpec, Proof, SettingsRef, Undo, WallCoverage, Words,
};
use k2_core::skin::{self, SkinPrincipal, SkinRole};
use serde_json::{json, Value};

use super::app_login::{login_on, principal, role_allows, role_named};
use super::held::{error_of, route_floor, text_param};
use super::vault::{own_service, OwnService};
use super::{Answerer, Applied, Check, Ctx, Registered};

pub(crate) const ACTION: &str = "apps.user-grant-box";
pub(crate) const ROUTE_ADD: &str = "/cli/skin/grants";
pub(crate) const ROUTE_ROLE: &str = "/cli/skin/grants/role";
/// The grant routes' own floor: the owner.
pub(crate) const FLOOR: Floor = Floor::Owner;

pub(crate) static BOX_LOGIN_GRANT: ActionSpec = ActionSpec {
    id: ACTION,
    area: Area::Apps,
    params: &[
        ParamSpec { key: "user", kind: ParamKind::Principal, required: true, list: false, choices: &[] },
        ParamSpec { key: "app", kind: ParamKind::App, required: true, list: false, choices: &[] },
        ParamSpec { key: "role", kind: ParamKind::AppRole, required: true, list: false, choices: &[] },
    ],
    floor: FLOOR,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "{user}'s access to {app}",
        what_changes: "{what}",
        consequences: "{effects}",
        scope: "{user} on {app}",
        duration: "until someone changes it",
        retry: "",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "people", path: "Settings → User Access" },
    walls: &[],
    coverage: WallCoverage::None,
    machine_hints: &[],
};

/// The checked values of one request.
struct Ask {
    person: SkinPrincipal,
    svc: OwnService,
    role: SkinRole,
}

fn ask_of(ctx: &Ctx) -> Result<Ask, String> {
    let user = text_param(ctx, "user").ok_or("name the person: user=<their username>")?;
    let app = text_param(ctx, "app").ok_or("name your App: app=<its name>")?;
    let role = text_param(ctx, "role").ok_or("name the role they get: role=<role> (k2 app role list)")?;
    let svc = own_service(&ctx.workspace_id, app).ok_or_else(|| format!("{} has no App named {app}", ctx.workspace_handle))?;
    let person = principal(user).ok_or_else(|| format!("there's no login named {user} on this server (k2 app user list)"))?;
    if skin::principal_agent_workspace(&person.id).map_err(|e| e.to_string())?.is_some() {
        return Err(format!("{} is an agent's login: an agent never gives an agent access to an App this way", person.username));
    }
    let grants = skin::list_grants().map_err(|e| e.to_string())?;
    if !crate::skin_routes::has_box_login(&person.id, &grants) {
        return Err(format!(
            "{u} has no box login, only App logins, so this card doesn't apply. Ask on this one instead: k2 permission request app.login-grant op=add user={u} app={app} role={role} --why \"<what you need it for>\"",
            u = person.username,
        ));
    }
    let role = role_named(role).ok_or_else(|| format!("there's no role named {role} on this server (k2 app role list)"))?;
    if role.app_id.as_deref().is_some_and(|a| a != svc.id) {
        return Err(format!("the role {} belongs to another App", role.name));
    }
    Ok(Ask { person, svc, role })
}

/// The card's words from the stored values: `{user}`, `{app}` by name,
/// `{what}` and `{effects}` (what the role allows).
pub(crate) fn with_computed(action: &str, ctx: &Ctx) -> Option<Ctx> {
    if action != ACTION {
        return None;
    }
    let mut out = ctx.clone();
    let ws = ctx.workspace_handle.clone();
    let (what, effects) = match ask_of(ctx) {
        Ok(a) => {
            out.params.insert("user".into(), json!(a.person.username));
            out.params.insert("app".into(), json!(a.svc.name));
            let (u, app) = (&a.person.username, &a.svc.name);
            (
                format!("Let {ws} give {u}, who has a box login here, access to its App {app} with the role {}", a.role.name),
                format!(
                    "When {u} signs in to {app} with the role {}, they can: {}. Their box login itself doesn't change, and they use their own password; {ws} never sees it. Undo it with k2 app grant rm --subject-kind principal --subject {u} --kind app --target {app}.",
                    a.role.name,
                    role_allows(&a.role)
                ),
            )
        }
        Err(e) => (format!("Let {ws} give a box login access to its App"), e),
    };
    out.params.insert("what".into(), json!(what));
    out.params.insert("effects".into(), json!(effects));
    Some(out)
}

fn check(ctx: &Ctx) -> Check {
    let a = match ask_of(ctx) {
        Ok(a) => a,
        Err(e) => return Check::Moved(e),
    };
    match login_on(&a.person, &a.svc) {
        Some(g) if g.enabled && g.role_id.as_deref() == Some(a.role.id.as_str()) => Check::AlreadyDone,
        _ => Check::Needed,
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
    let a = match ask_of(ctx) {
        Ok(a) => a,
        Err(e) => return Applied::Refused { code: "bad_params".into(), message: e },
    };
    let existing = login_on(&a.person, &a.svc);
    let route = if existing.is_some() { ROUTE_ROLE } else { ROUTE_ADD };
    if let Err(refused) = route_floor(route, who) {
        return refused;
    }
    // `owner_role_identity`: the owner token or an Owner login, nobody else.
    if let Answerer::Login { user, .. } = who {
        let owner = k2_core::connect_users::role_for_user(user).is_some_and(k2_core::connect_users::can_change_roles);
        if !owner {
            return Applied::Refused { code: "owner_only".into(), message: "only the server's owner changes a box login's access".into() };
        }
    }
    let actor = actor_of(who);
    let r = match &existing {
        Some(g) => crate::skin_routes::handle_grants_role(json!({ "id": g.id, "role": a.role.id }).to_string().as_bytes(), &actor),
        None => {
            let body = json!({ "subjectKind": "principal", "subjectId": a.person.id, "kind": "app", "targetId": a.svc.id, "roleId": a.role.id });
            crate::skin_routes::handle_grants_post(body.to_string().as_bytes(), &actor)
        }
    };
    if !r.status.starts_with("200") {
        let v: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
        let (code, message) = error_of(&v, "refused");
        return Applied::Refused { code, message };
    }
    Applied::Done { message: format!("{} has access to {} with the role {} now.", a.person.username, a.svc.name, a.role.name) }
}

pub(crate) fn actions() -> Vec<Registered> {
    vec![Registered { spec: &BOX_LOGIN_GRANT, check, apply }]
}

pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    Vec::new()
}

#[cfg(test)]
#[path = "box_login_grant_tests.rs"]
mod tests;
