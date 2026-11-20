//! `ci.preview` (0.46.17): turn on CI on K2 Compute for the whole server,
//! and its `ci.preview-off` twin.
//!
//! CI on K2 Compute is the server switch `ciPreview` (an owner-only key of
//! `POST /cli/settings/update`, `settings_routes::OWNER_ONLY_KEYS`). Off,
//! every CI surface answers 409 `ci_preview_off`, and an agent's own
//! `k2 settings set ciPreview true` is refused `owner_only` (agents never
//! reach `/cli/settings/*`). With cards on, both refusals end with "Ask
//! with: k2 permission request ci.preview" (the walls in `walls::ROWS`).
//!
//! - **Who answers:** the owner (the owner token or an Owner login), the
//!   key's own floor, both ways. Proof: a click, as in Settings.
//! - **On Allow:** `settings_routes::handle_settings_update_as` with
//!   `{"ciPreview": true}` (or `false` for the twin) as the person who
//!   clicked, at their current role: the route's owner-only check runs
//!   again at the click. Exactly that one key is written.
//! - It takes effect at once (`compute::set_ci_enabled` in the route); CI
//!   also needs compute nodes on (`computePreview`), and the card says so.

use k2_core::permission::catalog::{
    ActionSpec, Area, Direction, Expiry, Floor, Grant, NotCardableSpec, Proof, SettingsRef, Undo, Wall, WallCoverage, WallMatch,
    WallMethod, Words,
};
use k2_core::product_name;
use serde_json::{json, Value};

use super::switches::{applied, check_state};
use super::{Answerer, Applied, Check, Ctx, Registered};
use crate::settings_routes::SettingsActor;
use k2_core::cli_name;

pub(crate) const ACTION: &str = "ci.preview";
pub(crate) const ACTION_OFF: &str = "ci.preview-off";

/// The settings key this card writes, and nothing else.
pub(crate) const KEY: &str = "ciPreview";

/// The feature's name ("CI on K2 Compute").
macro_rules! ci_name {
    () => {
        concat!("CI on ", product_name!(), " Compute")
    };
}

/// The switch's Settings row (Settings → Compute → Machines).
pub(crate) const SETTINGS: SettingsRef =
    SettingsRef { page: "compute", path: concat!("Settings → Compute → ", ci_name!(), " (preview)") };
const SETTING: &str = concat!(ci_name!(), " (preview)");

/// The settings route an agent's `k2 settings set ciPreview true` hits.
pub(crate) const SETTINGS_ROUTE: &str = "/cli/settings/update";

pub(crate) static CI_PREVIEW: ActionSpec = ActionSpec {
    id: ACTION,
    area: Area::Compute,
    params: &[],
    floor: Floor::Owner,
    direction: Direction::Loosen,
    grant: Grant::Standing,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: SETTING,
        what_changes: concat!("Turn on ", ci_name!(), ": ", product_name!(), " checks every pushed commit on your compute machines"),
        consequences: concat!("This is for the whole server, not only {ws}: agents that may use compute can have their repos' pushes checked (", cli_name!(), " ci). CI checks go ahead of other jobs in the compute queues, so other jobs may wait longer. It works only while compute nodes are on (Settings → Compute). Turn it off any time in Settings → Compute."),
        scope: "this server",
        duration: "until someone turns it off",
        retry: "run your command again",
    },
    undo: Undo::Twin { action: ACTION_OFF },
    settings: SETTINGS,
    walls: &[
        // Every CI surface while the preview is off (`k2 ci`, rosters,
        // lanes, seats, signer pairing, rightsize, stats…).
        Wall {
            matches: WallMatch::Code { status: 409, code: k2_core::compute::CI_PREVIEW_OFF },
            route_prefix: "/cli/",
            method: WallMethod::Any,
            params: &[],
        },
        // The agent's own `k2 settings set ciPreview true`: the cell
        // socket's teaching `owner_only`, and TCP's `token_ok` refusal.
        Wall { matches: WallMatch::Code { status: 403, code: "owner_only" }, route_prefix: SETTINGS_ROUTE, method: WallMethod::Post, params: &[] },
        Wall {
            matches: WallMatch::Code { status: 403, code: crate::permission::walls::SETTINGS_TCP_REFUSAL },
            route_prefix: SETTINGS_ROUTE,
            method: WallMethod::Post,
            params: &[],
        },
    ],
    coverage: WallCoverage::Code,
    form: None,
    machine_hints: &[],
};

pub(crate) static CI_PREVIEW_OFF: ActionSpec = ActionSpec {
    id: ACTION_OFF,
    area: Area::Compute,
    params: &[],
    // The key is the owner's both ways (`OWNER_ONLY_KEYS`): never wider.
    floor: Floor::Owner,
    direction: Direction::Tighten,
    grant: Grant::Standing,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: SETTING,
        what_changes: concat!("Turn off ", ci_name!(), " on this server"),
        consequences: concat!(
            product_name!(),
            " stops checking pushed commits, and every ", cli_name!(), " ci command answers ci_preview_off until someone turns it on again. Past runs are kept."
        ),
        scope: "this server",
        duration: "until someone turns it on again",
        retry: "",
    },
    undo: Undo::None,
    settings: SETTINGS,
    walls: &[],
    coverage: WallCoverage::None,
    form: None,
    machine_hints: &[],
};

/// The persisted switch (not `ci_enabled`, which also needs compute on and
/// honours `K2_CI`): the card changes exactly this.
fn is_on() -> bool {
    k2_core::app_settings::load().ci_preview
}

fn check_on(_ctx: &Ctx) -> Check {
    check_state(true, is_on())
}

fn check_off(_ctx: &Ctx) -> Check {
    check_state(false, is_on())
}

/// The settings route's actor for the person who clicked, at their
/// **current** role (a login demoted since the card was drawn is what it
/// is now). Anyone below Admin is a Member to the route.
pub(crate) fn actor_of(who: &Answerer) -> (SettingsActor, String) {
    match who {
        Answerer::OwnerToken => (SettingsActor::Owner, "owner-token".to_string()),
        Answerer::Login { user, .. } => {
            let actor = match k2_core::connect_users::role_for_user(user) {
                Some(k2_core::connect_users::Role::Owner) => SettingsActor::Owner,
                Some(k2_core::connect_users::Role::Admin) => SettingsActor::Admin,
                _ => SettingsActor::Member,
            };
            (actor, format!("user:{user}"))
        }
    }
}

/// `{"ciPreview": on}` through the settings route's own entry.
fn write(who: &Answerer, on: bool) -> crate::cli_response::CliResponse {
    let (actor, by) = actor_of(who);
    let body = json!({ KEY: Value::Bool(on) }).to_string();
    crate::settings_routes::handle_settings_update_as(body.as_bytes(), actor, &by)
}

fn apply_on(_ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    let r = write(who, true);
    let message = if k2_core::compute::enabled() {
        concat!(ci_name!(), " is on for this server: pushes of every repo an agent sets up with ", cli_name!(), " ci are checked.")
    } else {
        concat!(
            ci_name!(),
            " is on for this server. Compute nodes are off, so nothing is checked until the owner turns them on in Settings → Compute."
        )
    };
    applied(&r, message)
}

fn apply_off(_ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    let r = write(who, false);
    applied(&r, format!(concat!(ci_name!(), " is off. Turn it back on in {}."), SETTINGS.path))
}

pub(crate) fn actions() -> Vec<Registered> {
    vec![
        Registered { spec: &CI_PREVIEW, check: check_on, apply: apply_on },
        Registered { spec: &CI_PREVIEW_OFF, check: check_off, apply: apply_off },
    ]
}

pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    Vec::new()
}

#[cfg(test)]
#[path = "ci_preview_tests.rs"]
mod tests;
