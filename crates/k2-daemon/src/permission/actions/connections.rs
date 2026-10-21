//! E1 — `connections.create` ("Allow agents to create connections for this
//! workspace", standing, with its `connections.create-off` twin) and
//! `connections.add to=` (one connection, one time). PRD rev 3.1 §5.2.
//!
//! Both apply through the Settings routes' own entries (§6.4, C15):
//! - `connections.create` → `misc_routes::connections_switch_entry`
//!   (`POST /cli/agents-create-connections`, Admin in route_policy);
//! - `connections.add` → `misc_routes::connections_add_entry`
//!   (`/cli/connections action=add`; route_policy Member, and the add is
//!   privileged only for the owner token or an Owner/Admin login, as the
//!   dispatcher stamps `actor_privileged`).

use k2_core::permission::catalog::{
    ActionSpec, Area, Direction, Expiry, Floor, Grant, NotCardableSpec, ParamKind, ParamSpec, Proof, Undo, Wall,
    WallCoverage, WallMatch, WallMethod, Words,
};

use super::switches::{applied, check_state, off_message, off_spec, param, ws_path, AGENT_TAB, GONE};
use super::{Answerer, Applied, Check, Ctx, Registered};

const SETTING: &str = "Allow agents to create connections for this workspace";

pub(crate) static CREATE: ActionSpec = ActionSpec {
    id: "connections.create",
    area: Area::Switches,
    params: &[],
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::Standing,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: SETTING,
        what_changes: "Let {ws} connect itself to other workspaces and remove its connections",
        consequences: "Connected workspaces can message each other. Connections to other servers still need those servers paired.",
        scope: "{ws}",
        duration: "until someone turns it off",
        retry: "run your command again",
    },
    undo: Undo::Twin { action: "connections.create-off" },
    settings: AGENT_TAB,
    walls: &[Wall {
        matches: WallMatch::Code { status: 403, code: "agents_create_connections_disabled" },
        route_prefix: "/cli/",
        method: WallMethod::Any,
        params: &[],
    }],
    coverage: WallCoverage::Code,
    machine_hints: &[],
};

pub(crate) static CREATE_OFF: ActionSpec = off_spec(
    "connections.create-off",
    Floor::Admin,
    &[],
    SETTING,
    "Stop {ws} creating or removing connections itself",
    "Connections it already has stay.",
);

pub(crate) static ADD: ActionSpec = ActionSpec {
    id: "connections.add",
    area: Area::Switches,
    params: &[ParamSpec { key: "to", kind: ParamKind::Workspace, required: true, list: false, choices: &[] }],
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "Connections",
        what_changes: "Connect {ws} to {to}",
        consequences: "{ws} and {to} can message each other. It doesn't let {ws} make other connections.",
        scope: "{ws} and {to}",
        duration: "once",
        retry: "run your command again",
    },
    undo: Undo::None,
    settings: AGENT_TAB,
    walls: &[
        Wall {
            matches: WallMatch::Code { status: 403, code: "agents_create_connections_disabled" },
            route_prefix: "/cli/connections",
            method: WallMethod::Any,
            params: &[],
        },
        Wall { matches: WallMatch::Code { status: 403, code: "not_connected" }, route_prefix: "/cli/", method: WallMethod::Any, params: &[] },
    ],
    coverage: WallCoverage::Code,
    machine_hints: &[],
};

/// `connections.create`: on = the effective gate (the server-wide master
/// lets every workspace in too); off = this workspace's own switch.
fn switch_on(ctx: &Ctx, want_on: bool) -> Result<bool, String> {
    let p = ws_path(&ctx.workspace_id).ok_or_else(|| GONE.to_string())?;
    Ok(if want_on {
        k2_core::workspace::settings::agents_can_create_connections_for_path(&p)
    } else {
        k2_core::workspace::settings::get_agents_can_create_connections(&p)
    })
}

fn check_create(ctx: &Ctx) -> Check {
    match switch_on(ctx, true) {
        Ok(on) => check_state(true, on),
        Err(m) => Check::Moved(m),
    }
}

fn check_create_off(ctx: &Ctx) -> Check {
    match switch_on(ctx, false) {
        Ok(on) => check_state(false, on),
        Err(m) => Check::Moved(m),
    }
}

fn apply_create(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    match ws_path(&ctx.workspace_id) {
        Some(p) => applied(&crate::misc_routes::connections_switch_entry(who, &p, true), ""),
        None => Applied::Refused { code: "workspace_not_found".into(), message: GONE.into() },
    }
}

fn apply_create_off(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    match ws_path(&ctx.workspace_id) {
        Some(p) => applied(&crate::misc_routes::connections_switch_entry(who, &p, false), off_message(ctx)),
        None => Applied::Refused { code: "workspace_not_found".into(), message: GONE.into() },
    }
}

/// Is `ctx`'s workspace already connected to `to=`? `Err` = the values
/// don't hold (no such workspace, itself, a malformed remote address).
fn connected(ctx: &Ctx) -> Result<bool, String> {
    ws_path(&ctx.workspace_id).ok_or_else(|| GONE.to_string())?;
    let to = param(ctx, "to").ok_or("name the workspace: to=<workspace or agent::host>")?;
    if k2_core::connections::is_remote_target(to) {
        // Read-only (the `list` action may soft-bind fingerprints; this
        // runs at render time too).
        let want = k2_core::connections::normalize_remote_addr(to)?;
        let rows = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::db::schema::WorkspaceRemoteConnection::list_for_source(&conn, &ctx.workspace_id).map_err(|e| e.to_string())?
        };
        return Ok(rows.iter().any(|r| {
            k2_core::connections::normalize_remote_addr(&k2_core::connections::display_remote_addr(&r.agent, &r.host))
                .is_ok_and(|a| a.eq_ignore_ascii_case(&want))
        }));
    }
    let target_path = crate::workspace_msg::resolve_workspace(to).ok_or_else(|| format!("no workspace '{to}' on this server"))?;
    let target_id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::workspace::agent_identity::resolve_project_id(&conn, &target_path)
    }
    .ok_or_else(|| format!("no workspace '{to}' on this server"))?;
    if target_id == ctx.workspace_id {
        return Err(format!("{to} is {} itself", ctx.workspace_handle));
    }
    Ok(k2_core::connections::are_local_peers(&ctx.workspace_id, &target_id))
}

fn check_add(ctx: &Ctx) -> Check {
    match connected(ctx) {
        Ok(on) => check_state(true, on),
        Err(m) => Check::Moved(m),
    }
}

fn apply_add(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    let (Some(p), Some(to)) = (ws_path(&ctx.workspace_id), param(ctx, "to")) else {
        return Applied::Refused { code: "workspace_not_found".into(), message: GONE.into() };
    };
    applied(&crate::misc_routes::connections_add_entry(who, &p, to), format!("Connected {} to {to}.", ctx.workspace_handle))
}

pub(crate) fn actions() -> Vec<Registered> {
    vec![
        Registered { spec: &CREATE, check: check_create, apply: apply_create },
        Registered { spec: &CREATE_OFF, check: check_create_off, apply: apply_create_off },
        Registered { spec: &ADD, check: check_add, apply: apply_add },
    ]
}

/// This area's rows that aren't card-able in v1 (none: both connection
/// rows are cards).
pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    Vec::new()
}
