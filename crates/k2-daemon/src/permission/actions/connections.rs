//! E1 — `connections.create` ("Allow agents to create connections for this
//! workspace", standing, with its `connections.create-off` twin) and
//! `connections.add to=` (one connection, one time). PRD rev 3.1 §5.2.
//!
//! `connections.add-between from= to=` (0.46.18, App role papercut 6): an
//! agent asks a person to connect two OTHER workspaces of this server, by
//! exact handle, so there is no list to misclick. Asked only (no wall).
//! The card names both, each with its folder and how many workspaces it
//! connects to (a hub shows); the asking agent gets no connection itself.
//! An Admin floor; an agent never answers it. Allow runs the Settings
//! add's own entry for `from`, as the person.
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

use super::switches::{applied, check_state, off_message, off_spec, param, ws_path, CONNECTIONS_TAB, GONE};
use super::{Answerer, Applied, Check, Ctx, Registered};

const SETTING: &str = "Let this agent add connections";

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
    settings: CONNECTIONS_TAB,
    walls: &[Wall {
        matches: WallMatch::Code { status: 403, code: "agents_create_connections_disabled" },
        route_prefix: "/cli/",
        method: WallMethod::Any,
        params: &[],
    }],
    coverage: WallCoverage::Code,
    form: None,
    machine_hints: &[],
};

pub(crate) static CREATE_OFF: ActionSpec = ActionSpec {
    settings: CONNECTIONS_TAB,
    ..off_spec(
        "connections.create-off",
        Floor::Admin,
        &[],
        SETTING,
        "Stop {ws} creating or removing connections itself",
        "Connections it already has stay.",
    )
};

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
    settings: CONNECTIONS_TAB,
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
    form: None,
    machine_hints: &[],
};

pub(crate) const ADD_BETWEEN_ACTION: &str = "connections.add-between";

pub(crate) static ADD_BETWEEN: ActionSpec = ActionSpec {
    id: ADD_BETWEEN_ACTION,
    area: Area::Switches,
    params: &[
        ParamSpec { key: "from", kind: ParamKind::Workspace, required: true, list: false, choices: &[] },
        ParamSpec { key: "to", kind: ParamKind::Workspace, required: true, list: false, choices: &[] },
        // 0.46.18 papercut 8: a short note K2 delivers once to both agents.
        ParamSpec { key: "note", kind: ParamKind::Text, required: false, list: false, choices: &[] },
    ],
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "Connections",
        what_changes: "Connect {from} to {to}",
        consequences: "{from_detail} and {to_detail} can message each other. {ws} asked for it and gets no connection itself. \
            Check both: a wrong workspace, or a hub, gives it many peers at once.{note_words}",
        scope: "{from} and {to}",
        duration: "once",
        retry: "",
    },
    undo: Undo::None,
    settings: CONNECTIONS_TAB,
    walls: &[],
    coverage: WallCoverage::None,
    form: None,
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
        Some(p) => applied(&crate::misc_routes::connections_switch_entry(who, &p, false), off_message(ctx, CONNECTIONS_TAB)),
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

/// A workspace of this server by its exact handle: `(id, path, handle)`.
fn local_workspace(handle: &str) -> Result<(String, String, String), String> {
    let h = handle.trim();
    if h.is_empty() || k2_core::connections::is_remote_target(h) {
        return Err(format!("name a workspace of this server by its handle, not '{h}'"));
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT id, path, handle FROM projects WHERE handle = ?1 COLLATE NOCASE", rusqlite::params![h], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
    })
    .map_err(|_| format!("no workspace with the handle '{h}' on this server"))
}

/// Both ends of `connections.add-between`, checked: two workspaces of this
/// server, neither the asking one, not the same.
fn between(ctx: &Ctx) -> Result<((String, String, String), (String, String, String)), String> {
    let from = local_workspace(param(ctx, "from").ok_or("name the first workspace: from=<handle>")?)?;
    let to = local_workspace(param(ctx, "to").ok_or("name the second workspace: to=<handle>")?)?;
    if from.0 == to.0 {
        return Err(format!("from= and to= are both {}", from.2));
    }
    if from.0 == ctx.workspace_id || to.0 == ctx.workspace_id {
        return Err(format!("{} is the asking workspace: for its own connection ask connections.add to=<workspace>", ctx.workspace_handle));
    }
    Ok((from, to))
}

/// How many workspaces `id` connects to.
fn connection_count(id: &str) -> i64 {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT COUNT(DISTINCT CASE WHEN source_project_id = ?1 THEN target_project_id ELSE source_project_id END) \
           FROM workspace_relations WHERE (source_project_id = ?1 OR target_project_id = ?1) AND source_project_id <> target_project_id",
        rusqlite::params![id],
        |r| r.get(0),
    )
    .unwrap_or(0)
}

/// One end in words: `@handle (~/folder, N connections)`, a hub named so.
fn end_words(w: &(String, String, String)) -> String {
    let n = connection_count(&w.0);
    let folder = crate::federation_rosters::folder_label(&w.1, dirs::home_dir().as_deref());
    let hub = if n >= 10 { ", a hub" } else { "" };
    format!("@{} ({folder}, {n} connection{}{hub})", w.2, if n == 1 { "" } else { "s" })
}

/// The card's computed words: `{from_detail}` and `{to_detail}`.
pub(crate) fn with_computed(action: &str, ctx: &Ctx) -> Option<Ctx> {
    if action != ADD_BETWEEN_ACTION {
        return None;
    }
    let mut out = ctx.clone();
    for key in ["from", "to"] {
        let words = match param(ctx, key).map(local_workspace) {
            Some(Ok(w)) => end_words(&w),
            _ => param(ctx, key).unwrap_or("?").to_string(),
        };
        out.params.insert(format!("{key}_detail"), serde_json::Value::String(words));
    }
    let note_words = match crate::connection_note::checked(param(ctx, "note")) {
        Ok(Some(n)) => format!(" {} tells both agents once: \"{n}\"", k2_core::brand::PRODUCT_NAME),
        _ => String::new(),
    };
    out.params.insert("note_words".to_string(), serde_json::Value::String(note_words));
    Some(out)
}

fn check_add_between(ctx: &Ctx) -> Check {
    if let Err(e) = crate::connection_note::checked(param(ctx, "note")) {
        return Check::Moved(e);
    }
    match between(ctx) {
        Ok((from, to)) => check_state(true, k2_core::connections::are_local_peers(&from.0, &to.0)),
        Err(m) => Check::Moved(m),
    }
}

fn apply_add_between(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    let (from, to) = match between(ctx) {
        Ok(x) => x,
        Err(m) => return Applied::Refused { code: "workspace_not_found".into(), message: m },
    };
    let note = param(ctx, "note");
    applied(&crate::misc_routes::connections_add_entry(who, &from.1, &to.2, note), format!("Connected {} to {}.", from.2, to.2))
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
    applied(&crate::misc_routes::connections_add_entry(who, &p, to, None), format!("Connected {} to {to}.", ctx.workspace_handle))
}

pub(crate) fn actions() -> Vec<Registered> {
    vec![
        Registered { spec: &CREATE, check: check_create, apply: apply_create },
        Registered { spec: &CREATE_OFF, check: check_create_off, apply: apply_create_off },
        Registered { spec: &ADD, check: check_add, apply: apply_add },
        Registered { spec: &ADD_BETWEEN, check: check_add_between, apply: apply_add_between },
    ]
}

/// This area's rows that aren't card-able in v1 (none: both connection
/// rows are cards).
pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    Vec::new()
}
