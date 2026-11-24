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
//! - **`publish.transfer`** (0.46.18): walled. A site's name another workspace
//!   on this server holds (after a server swap, the old workspace's), which
//!   `--transfer` moves for the owner or an Admin only: an agent asks to move
//!   it to its own workspace, and a person allows it with one click. Apply runs
//!   the claim route's own entry with `transfer=1`, as that person (the label
//!   rule's manager check runs again, and the move is audited `label-transfer`
//!   with who clicked). The card names the workspace that holds the name now,
//!   so a name that moves again before the click is `stale`.

use k2_core::permission::catalog::{
    ActionSpec, Area, CardForm, Direction, Expiry, Floor, Grant, NotCardableSpec, ParamFrom, ParamKind, ParamSpec, Proof, SettingsRef, Undo,
    Wall, WallCoverage, WallMatch, WallMethod, Words,
};
use k2_core::permission::request::PermissionRequest;
use k2_core::site_visibility::{self as sv, Mode};

use super::held::text_param;
use super::{Answerer, Applied, Check, Ctx, Registered};
use crate::cli_response::CliResponse;
use crate::publish_cards::{PIN, PUBLIC, PUBLIC_NEEDS_PERSON};
use k2_core::{cli_name, product_name};
use serde_json::json;
use k2_core::brand::PRODUCT_NAME;

const ROUTE: &str = "/cli/publish/visibility";

/// `publish.transfer`'s id, and the route its Allow runs (`k2 publish
/// subdomain claim <label> --transfer`).
pub(crate) const TRANSFER: &str = "publish.transfer";
pub(crate) const CLAIM_ROUTE: &str = "/cli/publish/subdomain/claim";

/// Asked Allow before the PIN is set: nothing is decided.
pub(crate) const PIN_FORM: CardForm = CardForm { id: "publish-pin", refusal: "pin_form_first" };
pub(crate) const PIN_FORM_FIRST: &str = PIN_FORM.refusal;

const SETTINGS: SettingsRef = SettingsRef { page: "k2-connect", path: "Settings → Network → Tunnel → Addresses" };

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
        consequences: concat!(product_name!(), " stops checking who opens it. Everyone signed in to it now is signed out, as they no longer need to be."),
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
    form: None,
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
        what_changes: concat!("You choose a PIN in ", product_name!(), "'s own form. Anyone with the address and the PIN can open {label}"),
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
    // The PIN goes into K2's own form on the card first.
    form: Some(PIN_FORM),
    machine_hints: &[],
};

pub(crate) static PUBLISH_TRANSFER: ActionSpec = ActionSpec {
    id: TRANSFER,
    area: Area::Domains,
    params: LABEL_ONLY,
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "Move the name {label} to {ws}",
        what_changes: "Move the site name {label} to {ws}. It belongs to {holder} now",
        consequences: "{ws} can then run and point {label} at its own services and Apps. The workspace that holds it now loses the name: what it serves there is no longer reached at that address once {ws} points it elsewhere. The move is recorded in the access audit log.",
        scope: "the name {label} on this server",
        duration: "until someone moves it again",
        retry: "run your command again, without --transfer",
    },
    undo: Undo::None,
    settings: SETTINGS,
    walls: &[
        // An agent's claim, run or start of a name another workspace holds.
        Wall {
            matches: WallMatch::Code { status: 409, code: crate::label_owner::CODE },
            route_prefix: "/cli/publish/",
            method: WallMethod::Post,
            params: &[("label", ParamFrom::Response("/label"))],
        },
        // The same with --transfer (the owner's or an Admin's).
        Wall {
            matches: WallMatch::Code { status: 403, code: crate::label_owner::TRANSFER_CODE },
            route_prefix: "/cli/publish/",
            method: WallMethod::Post,
            params: &[("label", ParamFrom::Response("/label"))],
        },
    ],
    coverage: WallCoverage::Code,
    form: None,
    machine_hints: &[],
};

/// The label the card names, lowercased, checked to be a DNS label.
fn transfer_label(ctx: &Ctx) -> Result<String, String> {
    let Some(label) = text_param(ctx, "label") else { return Err("name the site: label=<name>".into()) };
    let label = label.to_ascii_lowercase();
    let ok = label.len() <= 63 && label.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if !ok {
        return Err("a site name is 1-63 of a-z, 0-9 and -".into());
    }
    if k2_core::skin::is_reserved_nested_label(&label) {
        return Err(k2_core::skin::reserved_nested_label_error(&label));
    }
    Ok(label)
}

/// The workspace that holds `label` now (`None`: no workspace does).
fn holder_of(label: &str) -> Result<Option<String>, String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::db::schema::SubdomainWorkspace::owner_of(&conn, label).map_err(|e| e.to_string())
}

/// `{holder}`: who holds the name now, as any agent may be told (never a
/// workspace id or folder).
fn holder_words(workspace_id: &str) -> String {
    let m = crate::apps::coordination::maintainer_of(workspace_id);
    match (m.kind, &m.agent, &m.address) {
        ("agent", Some(a), Some(addr)) => format!("{a}'s workspace ({addr})"),
        ("parked", _, _) => "a parked workspace with no agent".to_string(),
        _ => "a workspace that was removed".to_string(),
    }
}

/// The card's computed words (`{holder}`, and `{label}` as stored).
pub(crate) fn with_computed(action: &str, ctx: &Ctx) -> Option<Ctx> {
    if action != TRANSFER {
        return None;
    }
    let mut out = ctx.clone();
    let holder = match transfer_label(ctx).and_then(|l| holder_of(&l).map(|h| (l, h))) {
        Ok((label, Some(h))) => {
            out.params.insert("label".into(), json!(label));
            holder_words(&h)
        }
        Ok((label, None)) => {
            out.params.insert("label".into(), json!(label));
            "no workspace".to_string()
        }
        Err(_) => "a workspace this server can't find".to_string(),
    };
    out.params.insert("holder".into(), json!(holder));
    Some(out)
}

fn check_transfer(ctx: &Ctx) -> Check {
    let label = match transfer_label(ctx) {
        Ok(l) => l,
        Err(why) => return Check::Moved(why),
    };
    match holder_of(&label) {
        Err(e) => Check::Moved(e),
        Ok(Some(h)) if h == ctx.workspace_id => Check::AlreadyDone,
        Ok(Some(_)) => Check::Needed,
        Ok(None) => Check::Moved(format!(
            "no workspace holds {label}, so there is nothing to move: run {} publish subdomain claim {label} (when it already serves something on this server, that asks the owner)",
            k2_core::brand::CLI_NAME
        )),
    }
}

/// Allow: the claim route's own entry with `transfer=1`, as the person who
/// clicked (an Owner or Admin by the card's floor; a manager for the label
/// rule), for the asking workspace.
fn apply_transfer(ctx: &Ctx, who: &Answerer, _token: Option<&str>) -> Applied {
    let refused = |code: &str, message: String| Applied::Refused { code: code.into(), message };
    let label = match transfer_label(ctx) {
        Ok(l) => l,
        Err(why) => return refused("bad_params", why),
    };
    if let Err(r) = super::held::route_floor(CLAIM_ROUTE, who) {
        return r;
    }
    // A person answers a card; a passport never does (the label rule would
    // read it as an agent and refuse anyway).
    if crate::caller_workspace::request_principal().is_some() {
        return refused("person_only", "a person answers this card, never an agent's session".into());
    }
    let from = holder_of(&label).ok().flatten().map(|h| holder_words(&h)).unwrap_or_else(|| "no workspace".into());
    let mut params = std::collections::HashMap::new();
    params.insert("label".to_string(), label.clone());
    params.insert("project".to_string(), ctx.workspace_id.clone());
    params.insert("transfer".to_string(), "1".to_string());
    let caller = crate::label_owner::Caller::Manager { actor: who.stamp() };
    let r = crate::label_owner::with_caller(caller, || crate::publish_routes::handle_subdomain_claim(&params));
    let v: serde_json::Value = serde_json::from_str(&r.body).unwrap_or(serde_json::Value::Null);
    if !r.status.starts_with("200") || v["success"] != true {
        let code = v["code"].as_str().or_else(|| v.pointer("/error/code").and_then(|c| c.as_str())).unwrap_or("transfer_refused");
        let message = v["error"].as_str().or_else(|| v.pointer("/error/hint").and_then(|c| c.as_str())).unwrap_or(concat!(k2_core::product_name!(), " couldn't move it"));
        return Applied::Refused { code: code.to_string(), message: message.to_string() };
    }
    if v["projectId"] != json!(ctx.workspace_id) {
        return refused("failed", format!("the claim came back for another workspace: {v}"));
    }
    Applied::Done {
        message: format!(
            "{label} is {}'s now (it was {from}'s). Run your command again, without --transfer: {} publish run {label} … or {} publish subdomain point {label} --target …",
            ctx.workspace_handle,
            k2_core::brand::CLI_NAME,
            k2_core::brand::CLI_NAME
        ),
    }
}

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
        return Check::Moved(format!("{} is a dev server: only the owner or an admin can make it public, from a terminal outside {PRODUCT_NAME}", v.label));
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
        message: v["error"].as_str().unwrap_or(concat!(product_name!(), " couldn't change it")).to_string(),
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
    concat!("Set the PIN first, in ", product_name!(), "'s own form on this card, or from a terminal outside ", product_name!(), ": ", cli_name!(), " publish visibility <name> pin").to_string()
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
        Registered { spec: &PUBLISH_TRANSFER, check: check_transfer, apply: apply_transfer },
    ]
}

pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    Vec::new()
}

#[cfg(test)]
#[path = "publish_transfer_tests.rs"]
mod transfer_tests;

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
