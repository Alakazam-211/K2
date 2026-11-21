//! `apps.test-guest-rooms app= login= role= rooms=` (0.46.17): an owner or
//! admin lets an agent's test login into the rooms its role names. The rules
//! and the route that files it: `crate::apps::test_guest_rooms`.
//!
//! - **Who asks:** the App's own agent, for a test login it made
//!   (`k2 app user rooms-ok <app> <login>`, or `k2 permission request` with
//!   the values that command reads). `--assign <login>` asks one person.
//! - **Who answers:** the owner token, an Owner or an Admin login; never an
//!   agent (a card is a person's).
//! - **Proof:** a click.
//! - **Allow** opens exactly the rooms on the card, for that login and that
//!   role, with an audit line. A card whose role or rooms changed since the
//!   ask, or whose login was made again since, is stale; a login that isn't
//!   a test login (or is one a person made) never takes it.

use k2_core::permission::catalog::{
    ActionSpec, Area, Direction, Expiry, Floor, Grant, NotCardableSpec, ParamKind, ParamSpec, Proof, SettingsRef, Undo, WallCoverage,
    Words,
};
use serde_json::Value;

use super::held::{text_param, who_of};
use super::{Answerer, Applied, Check, Ctx, Registered};
use crate::apps::test_guest_rooms as tgr;
use crate::compute_routes::Who;

pub(crate) const ACTION: &str = tgr::ACTION;

pub(crate) static TEST_GUEST_ROOMS: ActionSpec = ActionSpec {
    id: ACTION,
    area: Area::Apps,
    params: &[
        ParamSpec { key: "app", kind: ParamKind::App, required: true, list: false, choices: &[] },
        ParamSpec { key: "login", kind: ParamKind::Text, required: true, list: false, choices: &[] },
        ParamSpec { key: "role", kind: ParamKind::Text, required: true, list: false, choices: &[] },
        ParamSpec { key: "rooms", kind: ParamKind::Workspace, required: true, list: true, choices: &[] },
    ],
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "{login}'s rooms on {app}",
        what_changes: "Let {ws}'s test login {login} into {rooms} with its role {role}",
        consequences: "{login} is a test login {ws}'s agent made for {app}, so it signs in only to {ws}'s own room. Allow lets it use {rooms} too, with what the role {role} gives there (never a right only a person may have), until the test login ends. A different role or another room needs a new card.",
        scope: "{login} on {app}",
        duration: "until the test login ends",
        retry: "sign in as it again",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "app-approvals", path: "Settings → Cores → App Approvals" },
    walls: &[],
    coverage: WallCoverage::None,
    form: None,
    machine_hints: &[],
};

/// The rooms the card names, in its order.
fn rooms_of(ctx: &Ctx) -> Vec<String> {
    match ctx.params.get("rooms") {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        Some(Value::String(s)) => s.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        _ => Vec::new(),
    }
}

/// The asking agent, as the route reads it.
fn asker(ctx: &Ctx) -> Who {
    Who::Agent { project_id: ctx.workspace_id.clone(), session_id: String::new() }
}

/// The login now, and whether the card still fits it.
fn plan_of(ctx: &Ctx) -> Result<tgr::Plan, String> {
    let who = asker(ctx);
    let p = tgr::plan(&who, Some(ctx.workspace_id.as_str()), text_param(ctx, "app"), text_param(ctx, "login")).map_err(|r| r.hint)?;
    tgr::matches(&p, text_param(ctx, "role").unwrap_or(""), &rooms_of(ctx))?;
    Ok(p)
}

fn check(ctx: &Ctx) -> Check {
    match plan_of(ctx) {
        Ok(p) if p.done() => Check::AlreadyDone,
        Ok(_) => Check::Needed,
        Err(why) => Check::Moved(why),
    }
}

/// Opens the rooms as the person who clicked. A login made again since the
/// ask is a different login: stale, nothing opened.
fn apply(ctx: &Ctx, who: &Answerer, _token: Option<&str>) -> Applied {
    let refused = |code: &str, message: String| Applied::Refused { code: code.into(), message };
    let p = match plan_of(ctx) {
        Ok(p) => p,
        Err(why) => return refused("stale", why),
    };
    if let Some(asked) = super::super::answer::current_request().and_then(|id| crate::permission::store::get(&id).ok().flatten()) {
        if p.created_at > asked.created_at {
            return refused("stale", format!("{} was made again after the ask: ask again", p.username));
        }
    }
    let by = crate::apps::routes::actor(&who_of(who));
    match tgr::open(&p, &by, "card") {
        Ok(v) => Applied::Done { message: v["note"].as_str().unwrap_or("Opened.").to_string() },
        Err(r) => refused(r.code, r.hint),
    }
}

pub(crate) fn actions() -> Vec<Registered> {
    vec![Registered { spec: &TEST_GUEST_ROOMS, check, apply }]
}

pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    Vec::new()
}
