//! The words App-role answers carry (0.46.18 papercuts 2 to 5): why a
//! login isn't the agent's, where the owner changes its role, a role's
//! scope on the wire, and the box-wide hint.

use k2_core::skin;

use super::{app_name_of, app_of_workspace, role_in_workspace, rooms_in};

/// Why login `p` isn't wholly `ws`'s (0.46.18 papercut 4): the first of
/// [`super::principal_in_workspace`]'s and `apps::user_hold::own_login`'s reasons
/// that holds, in words. Never names another workspace's guest's details
/// beyond the workspace handle or App name.
pub(crate) fn outside_reason(p: &skin::SkinPrincipal, roles: &[skin::SkinRole], grants: &[skin::SkinGrant], ws: &str) -> String {
    let ws_h = |id: &str| crate::apps::routes::ws_handle(id);
    if let Some(a) = p.agent_workspace.as_deref().filter(|a| *a != ws) {
        return format!("it is the agent login of {}", ws_h(a));
    }
    if let Ok(Some(t)) = skin::test_guests::get(&p.id) {
        if t.workspace_id != ws {
            return format!("it is a test login of {}", ws_h(&t.workspace_id));
        }
    }
    if let Ok(Some(t)) = skin::ties::get(&p.id) {
        if t.workspace_id != ws {
            return format!("{}'s agent set it up", ws_h(&t.workspace_id));
        }
    }
    if let Ok(Some(c)) = skin::creators::workspace_of(&p.id) {
        if c != ws {
            return format!("{}'s agent added it", ws_h(&c));
        }
    }
    let mine: Vec<&skin::SkinGrant> = grants.iter().filter(|g| g.subject_kind == "principal" && g.subject_id == p.id).collect();
    if mine.iter().any(|g| g.kind == "app" && g.target_id == skin::HOST_APP_ID) {
        return "it also has a box login".to_string();
    }
    if let Some(g) = mine.iter().find(|g| g.kind != "app") {
        return format!("it holds a {} grant (box-wide)", g.kind);
    }
    if let Some(g) = mine.iter().find(|g| !app_of_workspace(&g.target_id, ws)) {
        return format!("it is a login on another workspace's App ({})", app_name_of(&g.target_id));
    }
    let role_named = |id: &Option<String>| id.as_ref().and_then(|id| roles.iter().find(|r| &r.id == id));
    let wide = role_named(&p.role_id).into_iter().chain(mine.iter().filter_map(|g| role_named(&g.role_id))).find(|r| match r.app_id.as_deref() {
        Some(app) => !app_of_workspace(app, ws),
        None => !role_in_workspace(r, ws),
    });
    if let Some(r) = wide {
        return match r.app_id.as_deref() {
            None => format!("its role {} is box-wide and reaches another workspace's room", r.name),
            Some(_) => format!("its role {} belongs to another workspace's App", r.name),
        };
    }
    if !rooms_in(&p.default_rooms, ws) {
        return "its own rooms reach another workspace".to_string();
    }
    if mine.is_empty() && p.default_rooms.is_empty() {
        return "it has nothing tying it to your workspace (the owner's login)".to_string();
    }
    "it reaches outside your workspace (another workspace's room or App)".to_string()
}

/// The App id `k2 app user rooms-ok` takes for service `svc` (its runtime
/// App's id, else the service's name).
pub(super) fn runtime_app_name(svc: &str) -> String {
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::deploy::store::get_app_by_service(&conn, svc).ok().flatten()
    };
    row.map(|a| a.app_id).unwrap_or_else(|| app_name_of(svc))
}

/// Where the owner changes a login's role, in words (papercut 2's path).
pub(crate) fn owners_role_path(username: &str) -> String {
    format!(
        "Only the owner changes it: in the {product} app, Settings → Access → User Access → {username} → What they can use → Sign-in and Apps → the App's Role, \
         or {cli} app user role {username} <role> from a terminal outside {product}",
        product = k2_core::brand::PRODUCT_NAME,
        cli = k2_core::brand::CLI_NAME,
    )
}

/// What a box-wide role reaches, in one line (`role create` without an App,
/// 0.46.18 papercut 3).
pub const BOX_WIDE_HINT: &str = concat!(
    "box-wide: every App and the box login can use it, and only the owner edits it; for one App: --app <app> (to narrow this one: ",
    k2_core::cli_name!(),
    " app role update <role> --app <app>)"
);

/// A role on the wire, with its scope (0.46.18 papercut 3): `scope` is
/// `box` (no App: every App and the box login) or `app`, and `appLabel`
/// names the App as `<workspace>/<name>`.
pub fn role_json(role: &skin::SkinRole) -> serde_json::Value {
    let mut v = serde_json::to_value(role).unwrap_or_else(|_| serde_json::json!({}));
    match role.app_id.as_deref() {
        None => v["scope"] = serde_json::json!("box"),
        Some(app) => {
            v["scope"] = serde_json::json!("app");
            v["appLabel"] = serde_json::json!(app_label(app));
        }
    }
    v
}

/// An App as `<workspace>/<name>` (its id when it is gone).
pub(super) fn app_label(app: &str) -> String {
    let svc = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::published_services::get_by_id(&conn, app).ok().flatten()
    };
    match svc {
        Some(s) => format!("{}/{}", crate::apps::routes::ws_handle(&s.project_id), s.name),
        None => app.to_string(),
    }
}
