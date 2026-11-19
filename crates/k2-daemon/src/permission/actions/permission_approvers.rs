//! `permission.approvers to=<login>[,<login>…]|everyone` (0.46.16): who
//! agents' permission cards go to when the agent names no one (`--to`).
//!
//! The owner sets it in Settings → Access → Permission Cards → Who agents
//! ask (`permissionApprovers`, an owner-only key of `POST
//! /cli/settings/update`), or allows this card. `everyone` (or an empty
//! list) is the default: every owner or admin who may answer, as before.
//!
//! - **Who asks:** any agent with cards on. No wall: nothing refuses an
//!   agent into it; it asks when a person tells it to, or when it was told
//!   not to contact someone.
//! - **Who answers:** the owner (the owner token or an Owner login), the
//!   key's own floor. Proof: a click, as in Settings.
//! - **On Allow:** `settings_routes::handle_settings_update_as` as the person
//!   who clicked, with the checked list. The words name each person and
//!   what the list means.
//! - It narrows who is asked, never who may answer
//!   ([`crate::permission::approvers`]).

use k2_core::app_settings::{check_permission_approvers, PERMISSION_APPROVERS_KEY};
use k2_core::permission::catalog::{
    ActionSpec, Area, Direction, Expiry, Floor, Grant, NotCardableSpec, ParamKind, ParamSpec, Proof, SettingsRef, Undo, WallCoverage,
    Words,
};
use serde_json::{json, Value};

use super::switches::applied;
use super::{Answerer, Applied, Check, Ctx, Registered};

pub(crate) const ACTION: &str = "permission.approvers";
/// The word for "every owner or admin who may answer" (an empty list).
pub(crate) const EVERYONE: &str = "everyone";

pub(crate) static APPROVERS: ActionSpec = ActionSpec {
    id: ACTION,
    area: Area::Misc,
    params: &[ParamSpec { key: "to", kind: ParamKind::Text, required: true, list: true, choices: &[] }],
    floor: Floor::Owner,
    direction: Direction::Tighten,
    grant: Grant::Standing,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "Who agents' permission cards ask",
        what_changes: "{what}",
        consequences: "{effects}",
        scope: "every agent on this server",
        duration: "until someone changes it",
        retry: "",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "permission-cards", path: "Settings → Access → Permission Cards → Who agents ask" },
    walls: &[],
    coverage: WallCoverage::None,
    form: None,
    machine_hints: &[],
};

/// The names asked for: `[]` for `everyone`, else each checked like the
/// setting (an owner or admin login, or `owner`).
fn list_of(ctx: &Ctx) -> Result<Vec<String>, String> {
    let raw: Vec<String> = match ctx.params.get("to") {
        Some(Value::Array(a)) => a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
        Some(Value::String(s)) => vec![s.clone()],
        _ => return Err("name them: to=<login>[,<login>…] (owners or admins, or owner), or to=everyone".into()),
    };
    let names: Vec<String> = raw.iter().map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()).collect();
    if names.iter().any(|n| n == EVERYONE) {
        if names.len() > 1 {
            return Err("to=everyone stands alone".into());
        }
        return Ok(Vec::new());
    }
    check_permission_approvers(&json!(names))
}

fn now_list() -> Vec<String> {
    k2_core::app_settings::load().permission_approvers
}

fn words_of(names: &[String]) -> String {
    if names.is_empty() {
        "every owner or admin who may answer".to_string()
    } else {
        names.iter().map(|n| if n == "owner" { "the owner (this server's own login)".to_string() } else { n.clone() }).collect::<Vec<_>>().join(", ")
    }
}

/// `{what}` and `{effects}` from the stored values.
pub(crate) fn with_computed(action: &str, ctx: &Ctx) -> Option<Ctx> {
    if action != ACTION {
        return None;
    }
    let mut out = ctx.clone();
    let (what, effects) = match list_of(ctx) {
        Ok(names) => (
            format!("Send agents' permission cards to {} when an agent names no one", words_of(&names)),
            if names.is_empty() {
                "Every owner or admin who may answer a card sees it with Allow and Deny and gets a push, as before. An agent can still name who it asks (k2 permission request … --to).".to_string()
            } else {
                format!(
                    "Only {} see agents' cards with Allow and Deny, get the push and the Tickets item, and see them under Waiting on you; a card none of them may answer goes to the owner. Other owners and admins aren't asked, and can still answer from Settings. Now: {}. An agent can still name who it asks (k2 permission request … --to).",
                    words_of(&names),
                    words_of(&now_list())
                )
            },
        ),
        Err(e) => ("Change who agents' permission cards ask".to_string(), e),
    };
    out.params.insert("what".into(), json!(what));
    out.params.insert("effects".into(), json!(effects));
    Some(out)
}

fn check(ctx: &Ctx) -> Check {
    match list_of(ctx) {
        Ok(names) if names == now_list() => Check::AlreadyDone,
        Ok(_) => Check::Needed,
        Err(e) => Check::Moved(e),
    }
}

/// Through the settings route's own entry, as the person who clicked.
fn apply(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    let names = match list_of(ctx) {
        Ok(n) => n,
        Err(e) => return Applied::Refused { code: "bad_params".into(), message: e },
    };
    let (actor, by) = match who {
        Answerer::OwnerToken => (crate::settings_routes::SettingsActor::Owner, "owner-token".to_string()),
        Answerer::Login { user, role } => (
            if role == "owner" { crate::settings_routes::SettingsActor::Owner } else { crate::settings_routes::SettingsActor::Admin },
            format!("user:{user}"),
        ),
    };
    let mut partial = serde_json::Map::new();
    partial.insert(PERMISSION_APPROVERS_KEY.to_string(), json!(names));
    let body = Value::Object(partial).to_string();
    let r = crate::settings_routes::handle_settings_update_as(body.as_bytes(), actor, &by);
    applied(&r, format!("Agents' permission cards go to {} now.", words_of(&names)))
}

pub(crate) fn actions() -> Vec<Registered> {
    vec![Registered { spec: &APPROVERS, check, apply }]
}

pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    Vec::new()
}

#[cfg(test)]
#[path = "permission_approvers_tests.rs"]
mod tests;
