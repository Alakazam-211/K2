//! Workbench W1 (0.46.7): `git.land sidecar= tip=` and `sidecar.retire
//! sidecar= discard=` — an agent asks a person to Land its worktree
//! sidecar's commits on the workspace's main branch, or to Retire it
//! (`research-t3code-ui-as-garden-v1-vs-live` §4.1 "Who"; Rosson
//! 2026-10-29: Land only after a person confirms).
//!
//! An agent is refused at the route (`403 land_needs_person` /
//! `retire_needs_person`, naming the sidecar, the branch tip and the
//! discard choice), which these actions' walls teach. A person at the
//! Admin floor (the routes' own) taps Allow and `apply` runs the route
//! entry ([`crate::sidecar_worktree::land_entry`] / `retire_entry`) as
//! them: Land is bound to the tip the agent asked about (a branch that
//! moved since is `stale`), so its own checks (main clean, on the base, no
//! merge, conflict abort) run again at the click.
//!
//! One-time, click proof, no undo.

use k2_core::permission::catalog::{
    ActionSpec, Area, Direction, Expiry, Floor, Grant, NotCardableSpec, ParamFrom, ParamKind, ParamSpec, Proof, SettingsRef,
    Undo, Wall, WallCoverage, WallMatch, WallMethod, Words,
};
use serde_json::{json, Value};

use super::held::{error_of, route_floor, text_param};
use super::{Answerer, Applied, Check, Ctx, Registered};
use crate::sidecar_worktree::{LAND_NEEDS_PERSON, LAND_ROUTE, RETIRE_NEEDS_PERSON, RETIRE_ROUTE};

pub(crate) const LAND_ACTION: &str = "git.land";
pub(crate) const RETIRE_ACTION: &str = "sidecar.retire";

const SETTINGS: SettingsRef = SettingsRef { page: "workspaces/{ws}", path: "k2 sidecar review {ws}/{sidecar}" };

pub(crate) static LAND: ActionSpec = ActionSpec {
    id: LAND_ACTION,
    area: Area::Misc,
    params: &[
        ParamSpec { key: "sidecar", kind: ParamKind::Sidecar, required: true, list: false, choices: &[] },
        ParamSpec { key: "tip", kind: ParamKind::Commit, required: true, list: false, choices: &[] },
    ],
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "Land {ws}/{sidecar}",
        what_changes: "Land {ws}/{sidecar}'s commits (up to {tip}) on {ws}'s main branch",
        consequences: "K2 cherry-picks the worktree's commits that aren't on the main branch yet onto it, in {ws}'s main checkout, then retires the worktree (to the Trash). It refuses if the main checkout has uncommitted changes, and stops with nothing changed on a conflict. Read them first: k2 sidecar review {ws}/{sidecar}.",
        scope: "{ws}/{sidecar} at {tip}",
        duration: "once",
        retry: "",
    },
    undo: Undo::None,
    settings: SETTINGS,
    walls: &[Wall {
        matches: WallMatch::Code { status: 403, code: LAND_NEEDS_PERSON },
        route_prefix: LAND_ROUTE,
        method: WallMethod::Post,
        params: &[("sidecar", ParamFrom::Response("/error/sidecar")), ("tip", ParamFrom::Response("/error/tip"))],
    }],
    coverage: WallCoverage::Code,
    machine_hints: &[],
};

pub(crate) static RETIRE: ActionSpec = ActionSpec {
    id: RETIRE_ACTION,
    area: Area::Misc,
    params: &[
        ParamSpec { key: "sidecar", kind: ParamKind::Sidecar, required: true, list: false, choices: &[] },
        ParamSpec { key: "discard", kind: ParamKind::Choice, required: true, list: false, choices: &["no", "yes"] },
    ],
    floor: Floor::Admin,
    direction: Direction::Tighten,
    grant: Grant::OneTime,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "Retire {ws}/{sidecar}",
        what_changes: "Retire {ws}/{sidecar} (discard its unlanded work: {discard})",
        consequences: "The sidecar stops and its worktree goes to the Trash; its Thread stays. With discard no, K2 refuses while it has commits not on the main branch or uncommitted files. With discard yes, those are thrown away and its branch is deleted.",
        scope: "{ws}/{sidecar}",
        duration: "once",
        retry: "",
    },
    undo: Undo::None,
    settings: SETTINGS,
    walls: &[Wall {
        matches: WallMatch::Code { status: 403, code: RETIRE_NEEDS_PERSON },
        route_prefix: RETIRE_ROUTE,
        method: WallMethod::Post,
        params: &[("sidecar", ParamFrom::Response("/error/sidecar")), ("discard", ParamFrom::Response("/error/discard"))],
    }],
    coverage: WallCoverage::Code,
    machine_hints: &[],
};

/// The asking workspace's worktree sidecar the request names, read through
/// the route as the owner (a read; the person's floor is applied at apply).
fn review_of(ctx: &Ctx) -> Result<Value, String> {
    let name = text_param(ctx, "sidecar").ok_or("name the sidecar: sidecar=<name>")?;
    let mut params = std::collections::HashMap::new();
    params.insert("workspace".to_string(), ctx.workspace_id.clone());
    params.insert("name".to_string(), name.to_string());
    let r = crate::sidecar_routes::handle(
        "/cli/sidecar/review",
        crate::sidecar_routes::Caller::Owner,
        &[],
        &params,
        "permission-card",
    );
    let v: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
    if !r.status.starts_with("200") {
        return Err(error_of(&v, "no_such_sidecar").1);
    }
    Ok(v)
}

fn check_land(ctx: &Ctx) -> Check {
    let Some(tip) = text_param(ctx, "tip") else {
        return Check::Moved("give the branch tip you want landed: tip=<sha> (k2 sidecar review <name>)".into());
    };
    let v = match review_of(ctx) {
        Ok(v) => v,
        Err(e) => return Check::Moved(e),
    };
    if v["retiredAt"].is_i64() {
        return Check::Moved(format!("{} is retired", v["address"].as_str().unwrap_or("that sidecar")));
    }
    let now = v["tip"].as_str().unwrap_or("");
    if !k2_core::worktree_task::sha_matches(now, tip) {
        return Check::Moved(format!(
            "{} moved since the ask (now {}); ask again with its new tip",
            v["branch"].as_str().unwrap_or("the branch"),
            &now[..now.len().min(12)]
        ));
    }
    if v["commits"].as_array().is_some_and(|c| c.is_empty()) {
        return Check::AlreadyDone;
    }
    Check::Needed
}

fn check_retire(ctx: &Ctx) -> Check {
    if !matches!(text_param(ctx, "discard"), Some("yes" | "no")) {
        return Check::Moved("say discard=no (refuse unlanded work) or discard=yes (throw it away)".into());
    }
    match review_of(ctx) {
        Err(e) => Check::Moved(e),
        Ok(v) if v["retiredAt"].is_i64() => Check::AlreadyDone,
        Ok(_) => Check::Needed,
    }
}

fn as_caller(who: &Answerer) -> crate::sidecar_routes::Caller {
    match who {
        Answerer::OwnerToken => crate::sidecar_routes::Caller::Owner,
        Answerer::Login { user, .. } => crate::sidecar_routes::Caller::Login { username: user.clone() },
    }
}

fn applied(r: &crate::cli_response::CliResponse, done: impl FnOnce(&Value) -> String) -> Applied {
    let v: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
    if !r.status.starts_with("200") {
        let (code, message) = error_of(&v, "refused");
        return Applied::Refused { code, message };
    }
    Applied::Done { message: done(&v) }
}

fn apply_land(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    let (Some(name), Some(tip)) = (text_param(ctx, "sidecar"), text_param(ctx, "tip")) else {
        return Applied::Refused { code: "bad_params".into(), message: "the request names no sidecar or tip".into() };
    };
    if let Err(refused) = route_floor(LAND_ROUTE, who) {
        return refused;
    }
    let body = json!({ "workspace": ctx.workspace_id, "name": name, "expectTip": tip });
    let r = crate::sidecar_worktree::land_entry(&as_caller(who), &body);
    applied(&r, |v| {
        let n = v["count"].as_u64().unwrap_or(0);
        let mut m = format!("Landed {n} commit{} on {}.", if n == 1 { "" } else { "s" }, v["onto"].as_str().unwrap_or("main"));
        if v["retired"].is_object() {
            m.push_str(" The worktree is retired.");
        } else if let Some(e) = v["retireError"]["hint"].as_str() {
            m.push_str(&format!(" It was not retired: {e}"));
        }
        m
    })
}

fn apply_retire(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    let Some(name) = text_param(ctx, "sidecar") else {
        return Applied::Refused { code: "bad_params".into(), message: "the request names no sidecar".into() };
    };
    if let Err(refused) = route_floor(RETIRE_ROUTE, who) {
        return refused;
    }
    let discard = text_param(ctx, "discard") == Some("yes");
    let body = json!({ "workspace": ctx.workspace_id, "name": name, "discard": discard });
    let r = crate::sidecar_worktree::retire_entry(&as_caller(who), &body);
    applied(&r, |v| {
        format!(
            "Retired {}: the worktree is in the Trash{}.",
            v["address"].as_str().unwrap_or("it"),
            if v["branchDeleted"] == json!(true) { " and its branch is deleted" } else { "" }
        )
    })
}

pub(crate) fn actions() -> Vec<Registered> {
    vec![
        Registered { spec: &LAND, check: check_land, apply: apply_land },
        Registered { spec: &RETIRE, check: check_retire, apply: apply_retire },
    ]
}

pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    Vec::new()
}

#[cfg(test)]
#[path = "workbench_tests.rs"]
mod tests;
