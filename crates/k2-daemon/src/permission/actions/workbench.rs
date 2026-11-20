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
//!
//! D1 (0.46.17): `git.review-approve agent= branch=` and `git.commit-merge
//! [message=]`, an agent's own merge that a person makes for it.

use k2_core::permission::catalog::{
    ActionSpec, Area, Direction, Expiry, Floor, Grant, NotCardableSpec, ParamFrom, ParamKind, ParamSpec, Proof, SettingsRef,
    Undo, Wall, WallCoverage, WallMatch, WallMethod, Words,
};
use serde_json::{json, Value};

use super::held::{error_of, route_floor, text_param};
use super::{Answerer, Applied, Check, Ctx, Registered};
use crate::sidecar_worktree::{LAND_NEEDS_PERSON, LAND_ROUTE, RETIRE_NEEDS_PERSON, RETIRE_ROUTE};
use k2_core::cli_name;

pub(crate) const LAND_ACTION: &str = "git.land";
pub(crate) const REVIEW_APPROVE_ACTION: &str = "git.review-approve";
pub(crate) const COMMIT_MERGE_ACTION: &str = "git.commit-merge";
const REVIEW_APPROVE_ROUTE: &str = "/cli/review/approve";
const COMMIT_MERGE_ROUTE: &str = "/cli/commit-merge";
pub(crate) const RETIRE_ACTION: &str = "sidecar.retire";

const SETTINGS: SettingsRef = SettingsRef { page: "workspaces/{ws}", path: concat!(cli_name!(), " sidecar review {ws}/{sidecar}") };

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
        consequences: concat!("K2 cherry-picks the worktree's commits that aren't on the main branch yet onto it, in {ws}'s main checkout, then retires the worktree (to the Trash). It refuses if the main checkout has uncommitted changes, and stops with nothing changed on a conflict. Read them first: ", cli_name!(), " sidecar review {ws}/{sidecar}."),
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
    form: None,
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
    form: None,
    machine_hints: &[],
};

/// D1 (0.46.17): an agent approves and merges its OWN review branch (or
/// one whose author can't be told) only through a person: the route answers
/// its passport `403 merge_needs_person` naming the agent and the branch
/// (`passport_routes::merge_needs_person`), which this card teaches. The
/// person's click runs the same route as the owner.
pub(crate) static REVIEW_APPROVE: ActionSpec = ActionSpec {
    id: REVIEW_APPROVE_ACTION,
    area: Area::Misc,
    params: &[
        ParamSpec { key: "agent", kind: ParamKind::Text, required: true, list: false, choices: &[] },
        ParamSpec { key: "branch", kind: ParamKind::Branch, required: true, list: false, choices: &[] },
    ],
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "Approve {branch} in {ws}",
        what_changes: "Merge {branch} into {ws}'s current branch and close {agent}'s review",
        consequences: "The branch is merged (fast-forward when it can be), its worktree and branch are removed and the review's done items are cleared. It stops with nothing merged on a conflict.",
        scope: "{ws}, {branch}",
        duration: "once",
        retry: "",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "workspaces/{ws}", path: "the review queue of {ws}" },
    walls: &[Wall {
        matches: WallMatch::Code { status: 403, code: crate::passport_routes::MERGE_NEEDS_PERSON },
        route_prefix: REVIEW_APPROVE_ROUTE,
        method: WallMethod::Post,
        params: &[("agent", ParamFrom::Response("/error/agent")), ("branch", ParamFrom::Response("/error/branch"))],
    }],
    coverage: WallCoverage::Code,
    form: None,
    machine_hints: &[],
};

/// D1 (0.46.17): an agent commits; `k2 commit --merge` from its passport
/// is a person's step (who wrote what it merges can't be told).
pub(crate) static COMMIT_MERGE: ActionSpec = ActionSpec {
    id: COMMIT_MERGE_ACTION,
    area: Area::Misc,
    params: &[ParamSpec { key: "message", kind: ParamKind::Text, required: false, list: false, choices: &[] }],
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "Commit and merge in {ws}",
        what_changes: "Commit {ws}'s changes and merge them, with the AI commit",
        consequences: "The desktop app opens the AI commit terminal in {ws}, which commits the changes and merges them.",
        scope: "{ws}",
        duration: "once",
        retry: "",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "workspaces/{ws}", path: "the Changes tab of {ws}" },
    walls: &[Wall {
        matches: WallMatch::Code { status: 403, code: crate::passport_routes::MERGE_NEEDS_PERSON },
        route_prefix: COMMIT_MERGE_ROUTE,
        method: WallMethod::Post,
        params: &[("message", ParamFrom::Query("message"))],
    }],
    coverage: WallCoverage::Code,
    form: None,
    machine_hints: &[],
};

/// The asking workspace's folder, by its id.
fn workspace_path(ctx: &Ctx) -> Result<String, String> {
    crate::workspace_msg::resolve_workspace(&ctx.workspace_id).ok_or_else(|| "the workspace is gone".to_string())
}

fn check_review_approve(ctx: &Ctx) -> Check {
    let (Some(_), Some(branch)) = (text_param(ctx, "agent"), text_param(ctx, "branch")) else {
        return Check::Moved("name the agent and its branch: agent=<name> branch=<branch>".into());
    };
    let path = match workspace_path(ctx) {
        Ok(p) => p,
        Err(e) => return Check::Moved(e),
    };
    if k2_core::git::list_branches(&path).local.iter().any(|b| b == branch) {
        Check::Needed
    } else {
        Check::Moved(format!("{branch} is not a branch of this workspace (merged or removed already?)"))
    }
}

fn check_commit_merge(ctx: &Ctx) -> Check {
    match workspace_path(ctx) {
        Ok(_) => Check::Needed,
        Err(e) => Check::Moved(e),
    }
}

/// The route as the person who clicked: the owner's dispatch, no passport.
fn run_route(route: &str, ctx: &Ctx, who: &Answerer, keys: &[&str]) -> Result<crate::cli_response::CliResponse, Applied> {
    route_floor(route, who)?;
    let path = workspace_path(ctx).map_err(|e| Applied::Refused { code: "bad_params".into(), message: e })?;
    let mut params = std::collections::HashMap::new();
    params.insert("project".to_string(), path);
    for k in keys {
        if let Some(v) = text_param(ctx, k) {
            params.insert((*k).to_string(), v.to_string());
        }
    }
    Ok(crate::cli::dispatch(route, &params))
}

fn apply_review_approve(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    match run_route(REVIEW_APPROVE_ROUTE, ctx, who, &["agent", "branch"]) {
        Ok(r) => applied(&r, |v| v["message"].as_str().unwrap_or("Approved and merged.").to_string()),
        Err(refused) => refused,
    }
}

fn apply_commit_merge(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    match run_route(COMMIT_MERGE_ROUTE, ctx, who, &["message"]) {
        Ok(r) => applied(&r, |_| "The AI commit terminal opens to commit and merge.".to_string()),
        Err(refused) => refused,
    }
}

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
        return Check::Moved(concat!("give the branch tip you want landed: tip=<sha> (", cli_name!(), " sidecar review <name>)").into());
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
        Registered { spec: &REVIEW_APPROVE, check: check_review_approve, apply: apply_review_approve },
        Registered { spec: &COMMIT_MERGE, check: check_commit_merge, apply: apply_commit_merge },
    ]
}

pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    Vec::new()
}

#[cfg(test)]
#[path = "workbench_tests.rs"]
mod tests;
