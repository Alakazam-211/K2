//! `focus-groups.create name=`, `focus-groups.rename group= to=` and
//! `focus-groups.assign target= group=` (0.46.17, Rosson): a manager agent
//! asks a person to create or rename a focus group, or to put a workspace in
//! one (`group=none` takes it out). The agent can't do it itself:
//! `POST /cli/focus-groups/create|update|assign` refuse its passport with
//! `focus_group_needs_person`, naming the values, which these actions'
//! walls teach (`crate::focus_group_routes`). The owner or an admin taps
//! Allow and `apply` runs that same route as them, after the route's own
//! floor.
//!
//! One-time, click proof, no undo: each card makes exactly the change it
//! names, once. A later change, or the same change for another workspace,
//! is another card. Focus groups are this server's, not a workspace's, so
//! the card names the group and the workspace in full.

use k2_core::permission::catalog::{
    ActionSpec, Area, Direction, Expiry, Floor, Grant, NotCardableSpec, ParamFrom, ParamKind, ParamSpec, Proof, SettingsRef,
    Undo, Wall, WallCoverage, WallMatch, WallMethod, Words,
};
use serde_json::{json, Value};

use super::held::{route_floor, text_param};
use super::switches::applied;
use super::{Answerer, Applied, Check, Ctx, Registered};
use crate::focus_group_routes::{
    resolve_group, ASSIGN, ASSIGN_ACTION, CREATE, CREATE_ACTION, NEEDS_PERSON, NONE, RENAME_ACTION, UPDATE,
};

const SETTINGS: SettingsRef = SettingsRef { page: "workspaces", path: "Settings → Workspaces / Agents" };

const fn text(key: &'static str) -> ParamSpec {
    ParamSpec { key, kind: ParamKind::Text, required: true, list: false, choices: &[] }
}

const fn wall(route: &'static str, params: &'static [(&'static str, ParamFrom)]) -> Wall {
    Wall { matches: WallMatch::Code { status: 403, code: NEEDS_PERSON }, route_prefix: route, method: WallMethod::Post, params }
}

const fn one_time(id: &'static str, params: &'static [ParamSpec], words: Words, walls: &'static [Wall]) -> ActionSpec {
    ActionSpec {
        id,
        area: Area::Misc,
        params,
        floor: Floor::Admin,
        direction: Direction::Loosen,
        grant: Grant::OneTime,
        proof: Proof::Click,
        expiry: Expiry::STANDARD,
        words,
        undo: Undo::None,
        settings: SETTINGS,
        walls,
        coverage: WallCoverage::Code,
        form: None,
        machine_hints: &[],
    }
}

pub(crate) static CREATE_GROUP: ActionSpec = one_time(
    CREATE_ACTION,
    &[text("name")],
    Words {
        setting: "Focus group {name}",
        what_changes: "Create the focus group {name}",
        consequences: "A new, empty focus group shows in the sidebar for everyone on this server. {ws} gets no other say over focus groups.",
        scope: "{name}",
        duration: "once",
        retry: "",
    },
    &[wall(CREATE, &[("name", ParamFrom::Response("/error/name"))])],
);

pub(crate) static RENAME_GROUP: ActionSpec = one_time(
    RENAME_ACTION,
    &[text("group"), text("to")],
    Words {
        setting: "Focus group {group}",
        what_changes: "Rename the focus group {group} to {to}",
        consequences: "Everyone on this server sees the new name. The workspaces in it stay in it. {ws} gets no other say over focus groups.",
        scope: "{group}",
        duration: "once",
        retry: "",
    },
    &[wall(UPDATE, &[("group", ParamFrom::Response("/error/group")), ("to", ParamFrom::Response("/error/to"))])],
);

pub(crate) static ASSIGN_GROUP: ActionSpec = one_time(
    ASSIGN_ACTION,
    &[ParamSpec { key: "target", kind: ParamKind::Workspace, required: true, list: false, choices: &[] }, text("group")],
    Words {
        setting: "{target}'s focus group",
        what_changes: "Set {target}'s focus group to {group}",
        consequences: "{target} shows under {group} in the sidebar (none: in no group). Nothing else about {target} changes, and {ws} gets no other say over focus groups.",
        scope: "{target}",
        duration: "once",
        retry: "",
    },
    &[wall(ASSIGN, &[("target", ParamFrom::Response("/error/target")), ("group", ParamFrom::Response("/error/group"))])],
);

fn need(ctx: &Ctx, key: &str, hint: &str) -> Result<String, String> {
    text_param(ctx, key).map(str::to_string).ok_or_else(|| hint.to_string())
}

fn name_taken(name: &str) -> Result<bool, String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let groups = k2_core::db::schema::FocusGroup::list(&conn).map_err(|e| e.to_string())?;
    Ok(groups.iter().any(|g| g.name.eq_ignore_ascii_case(name.trim())))
}

/// The named workspace and the group it should be in (`None` = no group).
fn assign_of(ctx: &Ctx) -> Result<(crate::agent_harness::Target, Option<k2_core::db::schema::FocusGroup>), String> {
    let token = need(ctx, "target", "name the workspace: target=<workspace>")?;
    let group = need(ctx, "group", "name the focus group: group=<name>, or group=none")?;
    let target = crate::agent_harness::resolve(&token).map_err(|r| {
        let v: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
        super::held::error_of(&v, "not_found").1
    })?;
    let group = if group.eq_ignore_ascii_case(NONE) { None } else { Some(resolve_group(&group)?) };
    Ok((target, group))
}

fn current_group_id(workspace_id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT focus_group_id FROM projects WHERE id = ?1", rusqlite::params![workspace_id], |r| r.get::<_, Option<String>>(0))
        .ok()
        .flatten()
}

fn check_create(ctx: &Ctx) -> Check {
    let name = match need(ctx, "name", "name the focus group: name=<name>") {
        Ok(n) => n,
        Err(e) => return Check::Moved(e),
    };
    match name_taken(&name) {
        Ok(true) => Check::AlreadyDone,
        Ok(false) => Check::Needed,
        Err(e) => Check::Moved(e),
    }
}

fn check_rename(ctx: &Ctx) -> Check {
    let (group, to) = match (need(ctx, "group", "name the focus group: group=<name>"), need(ctx, "to", "name the new name: to=<name>")) {
        (Ok(g), Ok(t)) => (g, t),
        (Err(e), _) | (_, Err(e)) => return Check::Moved(e),
    };
    match resolve_group(&group) {
        Ok(g) if g.name == to => Check::AlreadyDone,
        Ok(_) => Check::Needed,
        Err(e) => Check::Moved(e),
    }
}

fn check_assign(ctx: &Ctx) -> Check {
    match assign_of(ctx) {
        Err(e) => Check::Moved(e),
        Ok((target, group)) if current_group_id(&target.id) == group.as_ref().map(|g| g.id.clone()) => Check::AlreadyDone,
        Ok(_) => Check::Needed,
    }
}

/// The route as the person who clicked, after the route's own floor.
fn run(route: &str, body: Value, who: &Answerer) -> Result<crate::cli_response::CliResponse, Applied> {
    route_floor(route, who)?;
    let caller = super::switches::as_caller(who);
    Ok(crate::focus_group_routes::handle_as(route, body.to_string().as_bytes(), &caller))
}

fn apply_create(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    let name = match need(ctx, "name", "name the focus group: name=<name>") {
        Ok(n) => n,
        Err(e) => return Applied::Refused { code: "bad_params".into(), message: e },
    };
    match run(CREATE, json!({ "name": name }), who) {
        Ok(r) => applied(&r, format!("The focus group {name} is made.")),
        Err(refused) => refused,
    }
}

fn apply_rename(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    let (group, to) = match (need(ctx, "group", "name the focus group: group=<name>"), need(ctx, "to", "name the new name: to=<name>")) {
        (Ok(g), Ok(t)) => (g, t),
        (Err(e), _) | (_, Err(e)) => return Applied::Refused { code: "bad_params".into(), message: e },
    };
    let g = match resolve_group(&group) {
        Ok(g) => g,
        Err(e) => return Applied::Refused { code: "bad_params".into(), message: e },
    };
    match run(UPDATE, json!({ "id": g.id, "name": to }), who) {
        Ok(r) => applied(&r, format!("The focus group {} is called {to} now.", g.name)),
        Err(refused) => refused,
    }
}

fn apply_assign(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    let (target, group) = match assign_of(ctx) {
        Ok(t) => t,
        Err(e) => return Applied::Refused { code: "bad_params".into(), message: e },
    };
    let (gid, label) = match &group {
        Some(g) => (json!(g.id), format!("in the focus group {}", g.name)),
        None => (Value::Null, "in no focus group".to_string()),
    };
    match run(ASSIGN, json!({ "projectId": target.id, "focusGroupId": gid }), who) {
        Ok(r) => applied(&r, format!("{} is {label} now.", target.handle)),
        Err(refused) => refused,
    }
}

pub(crate) fn actions() -> Vec<Registered> {
    vec![
        Registered { spec: &CREATE_GROUP, check: check_create, apply: apply_create },
        Registered { spec: &RENAME_GROUP, check: check_rename, apply: apply_rename },
        Registered { spec: &ASSIGN_GROUP, check: check_assign, apply: apply_assign },
    ]
}

pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    Vec::new()
}

#[cfg(test)]
#[path = "focus_groups_tests.rs"]
mod tests;
