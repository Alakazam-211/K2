//! `agent.harness-switch harness=` (0.46.6, Rosson): move the asking
//! workspace's pinned chat to another harness. An agent can't do it
//! itself (it would restart its own session): `POST /cli/agent/harness`
//! and the older `/cli/workspace/swap-canonical` refuse it with
//! `harness_needs_person`, which this action's walls teach. A person at
//! the Admin floor taps Allow and `apply` runs that same route entry
//! ([`crate::agent_harness::handle`]) as them, so its own checks run again
//! at the click.
//!
//! One-time, click proof, no undo (moving back is another switch). The
//! approved turn reaches the agent's new chat (its address follows the
//! pinned chat).

use k2_core::permission::catalog::{
    ActionSpec, Area, Direction, Expiry, Floor, Grant, NotCardableSpec, ParamFrom, ParamKind, ParamSpec, Proof,
    SettingsRef, Undo, Wall, WallCoverage, WallMatch, WallMethod, Words,
};
use k2_core::workspace::canonical_swap::{normalize_harness, KNOWN_HARNESSES};
use serde_json::{json, Value};

use super::held::{error_of, route_floor, text_param};
use super::{Answerer, Applied, Check, Ctx, Registered};
use crate::agent_harness::{CARD_ACTION, NEEDS_PERSON, ROUTE};

const SWAP_ROUTE: &str = "/cli/workspace/swap-canonical";

pub(crate) static SWITCH: ActionSpec = ActionSpec {
    id: CARD_ACTION,
    area: Area::Misc,
    params: &[ParamSpec { key: "harness", kind: ParamKind::Choice, required: true, list: false, choices: &KNOWN_HARNESSES }],
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "Pinned chat on {harness}",
        what_changes: "Move {ws}'s pinned chat to {harness}",
        consequences: "K2 ends {ws}'s current pinned chat and starts a new {harness} chat with a summary of the recent conversation. The old chat stays in Chat history. Heartbeats, Threads and messages go to the new chat.",
        scope: "{ws}",
        duration: "once",
        retry: "",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "workspaces/{ws}", path: "Settings → Workspaces → {ws} → Default Agent" },
    walls: &[
        Wall {
            matches: WallMatch::Code { status: 403, code: NEEDS_PERSON },
            route_prefix: ROUTE,
            method: WallMethod::Post,
            params: &[("harness", ParamFrom::Response("/error/harness"))],
        },
        Wall {
            matches: WallMatch::Code { status: 403, code: NEEDS_PERSON },
            route_prefix: SWAP_ROUTE,
            method: WallMethod::Post,
            params: &[("harness", ParamFrom::Response("/error/harness"))],
        },
    ],
    coverage: WallCoverage::Code,
    machine_hints: &[],
};

/// The asked-for harness, normalized.
fn harness_of(ctx: &Ctx) -> Result<&'static str, String> {
    let raw = text_param(ctx, "harness").ok_or("name the harness: harness=<claude|codex|grok|gemini|cursor|pi|hermes>")?;
    normalize_harness(raw)
}

fn path_of(workspace_id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT path FROM projects WHERE id = ?1", rusqlite::params![workspace_id], |r| r.get::<_, String>(0)).ok()
}

/// Already on that harness → `AlreadyDone`; else `Needed`.
fn check(ctx: &Ctx) -> Check {
    let harness = match harness_of(ctx) {
        Ok(h) => h,
        Err(e) => return Check::Moved(e),
    };
    let Some(path) = path_of(&ctx.workspace_id) else {
        return Check::Moved(format!("{} is no longer a workspace on this server", ctx.workspace_handle));
    };
    if crate::agent_harness::current_harness(&ctx.workspace_id, &path) == harness {
        Check::AlreadyDone
    } else {
        Check::Needed
    }
}

/// Through the route's own entry, as the person who clicked.
fn apply(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    let harness = match harness_of(ctx) {
        Ok(h) => h,
        Err(e) => return Applied::Refused { code: "bad_params".into(), message: e },
    };
    if let Err(refused) = route_floor(ROUTE, who) {
        return refused;
    }
    let caller = match who {
        Answerer::OwnerToken => crate::sidecar_routes::Caller::Owner,
        Answerer::Login { user, .. } => crate::sidecar_routes::Caller::Login { username: user.clone() },
    };
    let body = json!({ "workspace": ctx.workspace_id, "harness": harness });
    let r = crate::agent_harness::handle(body.to_string().as_bytes(), &caller, "permission-card");
    let v: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
    if !r.status.starts_with("200") {
        let (code, message) = error_of(&v, "harness_refused");
        return Applied::Refused { code, message };
    }
    if v["alreadyOn"] == true {
        return Applied::Done { message: format!("It was already on {harness}.") };
    }
    Applied::Done { message: format!("The pinned chat runs on {harness} now; the old chat is in Chat history.") }
}

pub(crate) fn actions() -> Vec<Registered> {
    vec![Registered { spec: &SWITCH, check, apply }]
}

pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    Vec::new()
}

#[cfg(test)]
#[path = "harness_tests.rs"]
mod tests;
