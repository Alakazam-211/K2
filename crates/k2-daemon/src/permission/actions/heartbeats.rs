//! `heartbeat.pause-other` / `heartbeat.resume-other target= heartbeat=`
//! (0.46.6): pause or resume ANOTHER workspace's heartbeat. A manager agent
//! told "turn off the triage heartbeat in K2 Companion" can't do it itself:
//! `POST /cli/heartbeat/enable` with `workspace=<another>` refuses an agent
//! with `other_workspace` (naming the workspace and the heartbeat), which
//! these actions' walls teach. A person at the Admin floor taps Allow and
//! `apply` runs that same route entry ([`crate::heartbeat_other::handle`])
//! as them, so its own checks (role, workspace, heartbeat) run again at the
//! click, and the change is audited with them as the actor.
//!
//! One-time, click proof, no undo (resuming is the other card).

use k2_core::permission::catalog::{
    ActionSpec, Area, Direction, Expiry, Floor, Grant, NotCardableSpec, ParamFrom, ParamKind, ParamSpec, Proof, SettingsRef,
    Undo, Wall, WallCoverage, WallMatch, WallMethod, Words,
};
use serde_json::Value;

use super::held::{error_of, route_floor, text_param};
use super::{Answerer, Applied, Check, Ctx, Registered};
use crate::heartbeat_other::{OTHER_WORKSPACE, PAUSE_ACTION, RESUME_ACTION, ROUTE};

const PARAMS: &[ParamSpec] = &[
    ParamSpec { key: "target", kind: ParamKind::Workspace, required: true, list: false, choices: &[] },
    ParamSpec { key: "heartbeat", kind: ParamKind::Heartbeat, required: true, list: false, choices: &[] },
];

/// The refusal names the workspace and the heartbeat it was refused on.
const WALL_PARAMS: &[(&str, ParamFrom)] =
    &[("target", ParamFrom::Response("/error/workspace")), ("heartbeat", ParamFrom::Response("/error/heartbeat"))];

const SETTINGS: SettingsRef = SettingsRef { page: "workspaces/{target}", path: "{target} → Heartbeats → {heartbeat}" };

pub(crate) static PAUSE: ActionSpec = ActionSpec {
    id: PAUSE_ACTION,
    area: Area::Misc,
    params: PARAMS,
    floor: Floor::Admin,
    direction: Direction::Tighten,
    grant: Grant::OneTime,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "{target}'s {heartbeat} heartbeat",
        what_changes: "Pause {target}'s {heartbeat} heartbeat",
        consequences: "The {heartbeat} heartbeat stops waking {target} on its schedule until someone resumes it. Nothing else in {target} changes, and {ws} gets no other access to it.",
        scope: "{target}'s {heartbeat} heartbeat",
        duration: "once",
        retry: "",
    },
    undo: Undo::None,
    settings: SETTINGS,
    walls: &[Wall {
        matches: WallMatch::Code { status: 403, code: OTHER_WORKSPACE },
        route_prefix: ROUTE,
        method: WallMethod::Post,
        params: WALL_PARAMS,
    }],
    coverage: WallCoverage::Code,
    machine_hints: &[],
};

pub(crate) static RESUME: ActionSpec = ActionSpec {
    id: RESUME_ACTION,
    area: Area::Misc,
    params: PARAMS,
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "{target}'s {heartbeat} heartbeat",
        what_changes: "Resume {target}'s {heartbeat} heartbeat",
        consequences: "The {heartbeat} heartbeat wakes {target} on its schedule again, from its next slot (no catch-up fire). Nothing else in {target} changes, and {ws} gets no other access to it.",
        scope: "{target}'s {heartbeat} heartbeat",
        duration: "once",
        retry: "",
    },
    undo: Undo::None,
    settings: SETTINGS,
    walls: &[Wall {
        matches: WallMatch::Code { status: 403, code: OTHER_WORKSPACE },
        route_prefix: ROUTE,
        method: WallMethod::Post,
        params: WALL_PARAMS,
    }],
    coverage: WallCoverage::Code,
    machine_hints: &[],
};

/// The named workspace and heartbeat, checked: another workspace on this
/// server (not the asker's own) with a heartbeat of that name, and its
/// enabled flag now.
fn target_of(ctx: &Ctx) -> Result<(crate::heartbeat_other::Target, String, bool), String> {
    let token = text_param(ctx, "target").ok_or("name the workspace: target=<workspace>")?;
    let name = text_param(ctx, "heartbeat").ok_or("name the heartbeat: heartbeat=<name>")?.to_string();
    // The resolver's 400 is `{"error": "workspace not found: …"}`.
    let target = crate::heartbeat_other::resolve(token).map_err(|r| {
        let v: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
        v.get("error").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| error_of(&v, "workspace_not_found").1)
    })?;
    if target.id == ctx.workspace_id {
        return Err(format!(
            "{} is your own workspace: run k2 heartbeat disable|enable {name} yourself, no card needed",
            target.handle
        ));
    }
    let Some(enabled) = crate::heartbeat_other::enabled_of(&target.id, &name) else {
        return Err(format!("no heartbeat named '{name}' in workspace {}", target.handle));
    };
    Ok((target, name, enabled))
}

fn check_for(ctx: &Ctx, want_enabled: bool) -> Check {
    match target_of(ctx) {
        Err(e) => Check::Moved(e),
        Ok((_, _, enabled)) if enabled == want_enabled => Check::AlreadyDone,
        Ok(_) => Check::Needed,
    }
}

fn check_pause(ctx: &Ctx) -> Check {
    check_for(ctx, false)
}

fn check_resume(ctx: &Ctx) -> Check {
    check_for(ctx, true)
}

/// Through the route's own entry, as the person who clicked.
fn apply_for(ctx: &Ctx, who: &Answerer, enabled: bool) -> Applied {
    let (target, name, _) = match target_of(ctx) {
        Ok(t) => t,
        Err(e) => return Applied::Refused { code: "bad_params".into(), message: e },
    };
    if let Err(refused) = route_floor(ROUTE, who) {
        return refused;
    }
    let caller = match who {
        Answerer::OwnerToken => crate::sidecar_routes::Caller::Owner,
        Answerer::Login { user, .. } => crate::sidecar_routes::Caller::Login { username: user.clone() },
    };
    // The person who clicked has no workspace of their own: the route's
    // Admin floor and audit apply, as for any toggle of theirs.
    let r = crate::heartbeat_other::handle(None, Some(&target.id), &name, enabled, &caller, "permission-card");
    let v: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
    if !r.status.starts_with("200") {
        let (code, message) = error_of(&v, "heartbeat_refused");
        return Applied::Refused { code, message };
    }
    let done = if enabled { "resumed" } else { "paused" };
    Applied::Done { message: format!("{}'s {name} heartbeat is {done}.", target.handle) }
}

fn apply_pause(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    apply_for(ctx, who, false)
}

fn apply_resume(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    apply_for(ctx, who, true)
}

pub(crate) fn actions() -> Vec<Registered> {
    vec![
        Registered { spec: &PAUSE, check: check_pause, apply: apply_pause },
        Registered { spec: &RESUME, check: check_resume, apply: apply_resume },
    ]
}

pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    Vec::new()
}

#[cfg(test)]
#[path = "heartbeats_tests.rs"]
mod tests;
