//! `approve.request` (prd-signed-approvals-v1 S1): let an agent ask people
//! for signed approvals, and its `approve.request-off` twin.
//!
//! An agent without the permission is refused `approve_request_off` by
//! `POST /cli/approve/request`; with cards on, that refusal ends with
//! "Ask with: k2 permission request approve.request". Who it may then ask
//! is a separate rule (Rosson's question 3): by default only the person it
//! works for; anyone else allows it for themselves.
//!
//! - **Who answers:** an Owner or Admin (the `/cli/approve/agents` floor),
//!   by a click, as Settings would.
//! - **On Allow:** `approve::routes::agents_entry`, the route's own entry,
//!   as the person who clicked.

use k2_core::permission::catalog::{
    ActionSpec, Area, Direction, Expiry, Floor, Grant, NotCardableSpec, Proof, SettingsRef, Undo, Wall, WallCoverage, WallMatch,
    WallMethod, Words,
};

use super::switches::{applied, check_state};
use super::{Answerer, Applied, Check, Ctx, Registered};

pub(crate) const ACTION: &str = "approve.request";
pub(crate) const ACTION_OFF: &str = "approve.request-off";

/// No Settings row yet: the place is the command (the card's "Open in
/// Settings" still lands on the agent's page).
const SETTINGS: SettingsRef =
    SettingsRef { page: "workspaces/{ws}/permissions", path: "k2 approve agents {ws} on|off (an Owner or Admin)" };
const SETTING: &str = "Let this agent ask people for signed approvals";

pub(crate) static APPROVE_REQUEST: ActionSpec = ActionSpec {
    id: ACTION,
    area: Area::Switches,
    params: &[],
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::Standing,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: SETTING,
        what_changes: "Let {ws} ask people to approve things on their phone, with Face ID or their passcode",
        consequences: "{ws} can ask the person it works for. Anyone else must allow it first, and anyone can block it. Each approval is shown on the person's own phone and signed there; {ws} can never approve anything itself.",
        scope: "{ws}",
        duration: "until someone turns it off",
        retry: "run your command again",
    },
    undo: Undo::Twin { action: ACTION_OFF },
    settings: SETTINGS,
    walls: &[Wall {
        matches: WallMatch::Code { status: 403, code: crate::approve::routes::CODE_OFF },
        route_prefix: crate::approve::routes::REQUEST,
        method: WallMethod::Post,
        params: &[],
    }],
    coverage: WallCoverage::Code,
    machine_hints: &[],
};

pub(crate) static APPROVE_REQUEST_OFF: ActionSpec = ActionSpec {
    id: ACTION_OFF,
    area: Area::Switches,
    params: &[],
    floor: Floor::Admin,
    direction: Direction::Tighten,
    grant: Grant::Standing,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: SETTING,
        what_changes: "Stop {ws} asking people for signed approvals",
        consequences: "Requests already waiting stay open until they are answered or expire.",
        scope: "{ws}",
        duration: "until someone turns it on again",
        retry: "",
    },
    undo: Undo::None,
    settings: SETTINGS,
    walls: &[],
    coverage: WallCoverage::None,
    machine_hints: &[],
};

fn is_on(ws: &str) -> Result<bool, String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::approve_requests::agent_may_request(&conn, ws).map_err(|e| e.to_string())
}

fn check_on(ctx: &Ctx) -> Check {
    match is_on(&ctx.workspace_id) {
        Ok(on) => check_state(true, on),
        Err(e) => Check::Moved(e),
    }
}

fn check_off(ctx: &Ctx) -> Check {
    match is_on(&ctx.workspace_id) {
        Ok(on) => check_state(false, on),
        Err(e) => Check::Moved(e),
    }
}

fn apply_on(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    let r = crate::approve::routes::agents_entry(who, &ctx.workspace_id, true);
    applied(&r, format!("{} may ask people for signed approvals now: the person it works for, and anyone who allows it.", ctx.workspace_handle))
}

fn apply_off(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    let r = crate::approve::routes::agents_entry(who, &ctx.workspace_id, false);
    applied(&r, format!("{} can't ask for signed approvals now.", ctx.workspace_handle))
}

pub(crate) fn actions() -> Vec<Registered> {
    vec![
        Registered { spec: &APPROVE_REQUEST, check: check_on, apply: apply_on },
        Registered { spec: &APPROVE_REQUEST_OFF, check: check_off, apply: apply_off },
    ]
}

pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    Vec::new()
}
