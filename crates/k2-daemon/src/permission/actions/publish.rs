//! `publish.public` and `publish.pin` (prd-publish-visibility-and-dev-mode-v1
//! PV-7, PV-10, vs-live PA-17; K2 presents both: `crate::publish_cards`).
//!
//! - **`publish.public`:** walled. An agent asks to make its own site public; a
//!   person allows it with one click. Apply calls the visibility route's own
//!   entry as that person. Agents never ask this for a dev site.
//! - **`publish.pin`:** asked, with no wall. The person types the PIN into K2's
//!   own form, which posts it straight to the route; the PIN never passes
//!   through this card. So the answer gate refuses Allow until the site has a
//!   PIN ([`gate_answer`]), and a site that has one is "already on".

use k2_core::permission::catalog::{
    ActionSpec, Area, Direction, Expiry, Floor, Grant, NotCardableSpec, ParamFrom, ParamKind, ParamSpec, Proof, SettingsRef, Undo, Wall,
    WallCoverage, WallMatch, WallMethod, Words,
};
use k2_core::permission::request::PermissionRequest;
use k2_core::site_visibility::{self as sv, Mode};

use super::held::text_param;
use super::{Answerer, Applied, Check, Ctx, Registered};
use crate::cli_response::CliResponse;
use crate::publish_cards::{PIN, PUBLIC, PUBLIC_NEEDS_PERSON};

const ROUTE: &str = "/cli/publish/visibility";

/// Asked Allow before the PIN is set: nothing is decided.
pub(crate) const PIN_FORM_FIRST: &str = "pin_form_first";

const SETTINGS: SettingsRef = SettingsRef { page: "k2-connect", path: "Settings → K2 Connect → URLs" };

const LABEL_ONLY: &[ParamSpec] = &[ParamSpec { key: "label", kind: ParamKind::Site, required: true, list: false, choices: &[] }];
const LABEL_REASON: &[ParamSpec] = &[
    ParamSpec { key: "label", kind: ParamKind::Site, required: true, list: false, choices: &[] },
    ParamSpec { key: "reason", kind: ParamKind::Text, required: true, list: false, choices: &[] },
];

pub(crate) static PUBLISH_PUBLIC: ActionSpec = ActionSpec {
    id: PUBLIC,
    area: Area::Domains,
    params: LABEL_REASON,
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "Make {label} public",
        what_changes: "Anyone with the address can open {label}. {ws}'s reason: {reason}",
        consequences: "K2 stops checking who opens it. Everyone signed in to it now is signed out, as they no longer need to be.",
        scope: "the site {label}",
        duration: "until someone changes it",
        retry: "run your command again",
    },
    undo: Undo::None,
    settings: SETTINGS,
    walls: &[Wall {
        matches: WallMatch::Code { status: 403, code: PUBLIC_NEEDS_PERSON },
        route_prefix: ROUTE,
        method: WallMethod::Post,
        params: &[("label", ParamFrom::Response("/label")), ("reason", ParamFrom::Body("/reason"))],
    }],
    coverage: WallCoverage::Code,
    machine_hints: &[],
};

pub(crate) static PUBLISH_PIN: ActionSpec = ActionSpec {
    id: PIN,
    area: Area::Domains,
    params: LABEL_ONLY,
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "Put {label} behind a PIN",
        what_changes: "You choose a PIN in K2's own form. Anyone with the address and the PIN can open {label}",
        consequences: "{ws} never sees the PIN. Send the address and the PIN separately.",
        scope: "the site {label}",
        duration: "until someone changes it",
        retry: "",
    },
    undo: Undo::None,
    settings: SETTINGS,
    // Asked only. The refusal carries K2's card already.
    walls: &[],
    coverage: WallCoverage::None,
    machine_hints: &[],
};

/// The label as stored, and its current visibility.
fn current(ctx: &Ctx) -> Result<sv::SiteVisibility, String> {
    let Some(label) = text_param(ctx, "label") else { return Err("name the site: label=<name>".into()) };
    let db = k2_core::db::shared();
    let conn = db.lock();
    let v = sv::effective(&conn, label).map_err(|e| e.split_once(": ").map(|(_, w)| w.to_string()).unwrap_or(e))?;
    let holds = k2_core::db::schema::SubdomainWorkspace::owner_of(&conn, &v.label).ok().flatten().as_deref() == Some(ctx.workspace_id.as_str())
        || k2_core::published_services::get_by_project_name(&conn, &ctx.workspace_id, &v.label).ok().flatten().is_some();
    if !holds {
        return Err(format!("{} isn't a site of {}", v.label, ctx.workspace_handle));
    }
    Ok(v)
}

fn check_public(ctx: &Ctx) -> Check {
    let v = match current(ctx) {
        Ok(v) => v,
        Err(why) => return Check::Moved(why),
    };
    if v.dev {
        return Check::Moved(format!("{} is a dev server: only the owner or an admin can make it public, from a terminal outside K2", v.label));
    }
    if text_param(ctx, "reason").is_none_or(|r| r.trim().is_empty()) {
        return Check::Moved("say why this site should be public: reason=\"…\"".into());
    }
    if v.mode == Mode::Public {
        return Check::AlreadyDone;
    }
    Check::Needed
}

/// Allow: the route's own entry, as the person, whose floor runs again.
fn apply_public(ctx: &Ctx, who: &Answerer, _token: Option<&str>) -> Applied {
    let mut params = std::collections::HashMap::new();
    for key in ["label", "reason"] {
        if let Some(v) = text_param(ctx, key) {
            params.insert(key.to_string(), v.to_string());
        }
    }
    params.insert("mode".to_string(), "public".to_string());
    let r = crate::publish_site_routes::handle_set(&params, &crate::publish_site_routes::who_of(who));
    if r.status.starts_with("200") {
        return Applied::Done { message: format!("{} is public now.", text_param(ctx, "label").unwrap_or("the site")) };
    }
    let v: serde_json::Value = serde_json::from_str(&r.body).unwrap_or(serde_json::Value::Null);
    Applied::Refused {
        code: v["code"].as_str().unwrap_or("failed").to_string(),
        message: v["error"].as_str().unwrap_or("K2 couldn't change it").to_string(),
    }
}

fn check_pin(ctx: &Ctx) -> Check {
    match current(ctx) {
        Ok(v) if v.mode == Mode::Pin => Check::AlreadyDone,
        Ok(_) => Check::Needed,
        Err(why) => Check::Moved(why),
    }
}

/// Reached only past [`gate_answer`], and the check above turns a set PIN into
/// "already on". Nothing here can set one: it never holds a PIN.
fn apply_pin(_ctx: &Ctx, _who: &Answerer, _token: Option<&str>) -> Applied {
    Applied::Refused { code: PIN_FORM_FIRST.to_string(), message: pin_form_words() }
}

fn pin_form_words() -> String {
    "Set the PIN first, in K2's own form on this card, or from a terminal outside K2: k2 publish visibility <name> pin".to_string()
}

/// Allow on `publish.pin` before the site has a PIN (a client without the form:
/// the phone, a terminal, an older build): refused, nothing decided, the
/// request stays open.
pub(crate) fn gate_answer(req: &PermissionRequest) -> Result<(), CliResponse> {
    if req.action != PIN {
        return Ok(());
    }
    let ctx = super::super::present::ctx_of(req);
    match check_pin(&ctx) {
        Check::Needed => Err(super::super::answer::err(409, PIN_FORM_FIRST, pin_form_words())),
        _ => Ok(()),
    }
}

pub(crate) fn actions() -> Vec<Registered> {
    vec![
        Registered { spec: &PUBLISH_PUBLIC, check: check_public, apply: apply_public },
        Registered { spec: &PUBLISH_PIN, check: check_pin, apply: apply_pin },
    ]
}

pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PV-10 / PA-17: the floor, and which of the two is walled.
    #[test]
    fn public_is_walled_and_pin_is_asked_and_both_are_a_managers_answer() {
        assert_eq!(PUBLISH_PUBLIC.floor, Floor::Admin);
        assert_eq!(PUBLISH_PIN.floor, Floor::Admin);
        assert_eq!(PUBLISH_PUBLIC.walls.len(), 1);
        assert!(PUBLISH_PIN.walls.is_empty(), "the PIN is asked, with no wall");
        assert!(matches!(PUBLISH_PUBLIC.walls[0].matches, WallMatch::Code { status: 403, code: PUBLIC_NEEDS_PERSON }));
        assert_eq!(crate::publish_cards::PIN_NEEDS_PERSON, "pin_needs_person");
    }

    /// PV-7: the PIN card has no param that could carry one, and Allow alone
    /// can't set one.
    #[test]
    fn the_pin_card_cannot_carry_a_pin_and_allow_alone_does_nothing() {
        assert_eq!(PUBLISH_PIN.params.iter().map(|p| p.key).collect::<Vec<_>>(), vec!["label"]);
        let ctx = Ctx { workspace_id: "w".into(), workspace_handle: "w".into(), params: serde_json::Map::new() };
        assert!(matches!(apply_pin(&ctx, &Answerer::OwnerToken, None), Applied::Refused { ref code, .. } if code == PIN_FORM_FIRST));
    }
}
