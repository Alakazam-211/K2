//! Two cards for chat in an App (App-governed Spaces, 0.46.7,
//! prd-app-governed-spaces-k2-v1 §2.3, §4, §10.1):
//!
//! `apps.chat-key app= [level=] [caps=] [uploads=]`: an App's own agent
//! asks for its App's chat key to be turned on or widened.
//! `k2 app grant <app> --kind chat-key` (`POST /cli/app/chat-key`) answers
//! an agent `owner_only` (vs-live AG-6: agents can read the owner token, so
//! a key needs a person). With cards on for the agent's workspace, that
//! refusal also names the App and what was asked, and ends with the line to
//! ask with.
//!
//! `sew.space-governance app= space= to=app|sew`: an agent asks for one of
//! its App's Spaces to be switched to App-governed (or back).
//! `k2 hostchat app-space governance` (`POST /cli/sew/app-spaces/governance`)
//! answers an agent `person_only`, "Manage Sew" or not (AG-7). The wall reads
//! the App, the Space and the side from the refused call's own body.
//!
//! - **Who asks:** only for an App (a published service) of the asking
//!   workspace, found there by name or id, and (governance) only a Space
//!   bound to that App. Anything else is refused when asked, in the same
//!   words as a name that doesn't exist. Cards off: the routes' refusals are
//!   today's, byte for byte.
//! - **Who answers:** who the routes admit: the owner token, an Owner or an
//!   Admin login (both routes' policy rows are Admin). The chat key's own
//!   rules still hold at the click: an Admin gives a key only within the
//!   defaults (the App's approved chat caps, level member or moderator, 500
//!   people, uploads not newly on); past that the card says only the owner
//!   can allow it, and an Admin's Allow is refused with [`OWNER_IN_PERSON`],
//!   which leaves the card open for the owner.
//! - **Proof:** the same fresh confirm the routes take, on the card bound to
//!   the request (`apps-chat-key`, `sew-space-governance`); at the click K2
//!   turns it into the route's own (`chat-key` bound to the App,
//!   `chat-governance` bound to `<app>:<space>:<side>`) for the same person,
//!   so the route still decides for itself.
//! - **On Allow:** the route's own entry runs as the person who clicked, by
//!   the App's service id. Once: the answer is a compare-and-set and the
//!   confirm works once.

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::permission::catalog::{
    ActionSpec, Area, ConfirmAction, Direction, Expiry, Floor, Grant, NotCardableSpec, ParamFrom, ParamKind, ParamSpec, Proof,
    SettingsRef, Undo, Wall, WallCoverage, WallMatch, WallMethod, Words,
};
use k2_core::skin::app_chat_key as ck;
use k2_vault::HumanProof;
use serde_json::{json, Value};

use super::held::{error_of, route_floor, text_param};
use super::vault::{own_service, OwnService};
use super::{Answerer, Applied, Check, Ctx, Registered};
use crate::compute_routes::Who;

/// An answer the card's rules leave to the owner: the request goes back to
/// open (`permission::answer`), so the owner can still allow it.
pub(crate) const OWNER_IN_PERSON: &str = "owner_in_person";

// ── apps.chat-key ───────────────────────────────────────────────────

pub(crate) const KEY_ACTION: &str = "apps.chat-key";
pub(crate) const KEY_ROUTE: &str = crate::apps::chat_key_routes::KEY;
/// The card's confirm (`ConfirmAction::AppsChatKey`), bound to the request.
pub(crate) const KEY_CONFIRM: &str = "apps-chat-key";
pub(crate) const LEVELS: [&str; 3] = ["member", "moderator", "admin"];
pub(crate) const UPLOADS: [&str; 2] = ["on", "off"];

pub(crate) static CHAT_KEY: ActionSpec = ActionSpec {
    id: KEY_ACTION,
    area: Area::Apps,
    params: &[
        ParamSpec { key: "app", kind: ParamKind::App, required: true, list: false, choices: &[] },
        ParamSpec { key: "level", kind: ParamKind::Choice, required: false, list: false, choices: &LEVELS },
        ParamSpec { key: "caps", kind: ParamKind::ChatCaps, required: false, list: true, choices: &[] },
        ParamSpec { key: "uploads", kind: ParamKind::Choice, required: false, list: false, choices: &UPLOADS },
    ],
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::SettingsConfirm { action: ConfirmAction::AppsChatKey },
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "{app}'s chat key",
        what_changes: "{what}",
        consequences: "{effects}",
        scope: "{app}, for its own users",
        duration: "until someone revokes it",
        retry: "",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "apps", path: "k2 app grant <app> --kind chat-key (a terminal outside K2)" },
    walls: &[Wall {
        matches: WallMatch::Code { status: 403, code: "owner_only" },
        route_prefix: KEY_ROUTE,
        method: WallMethod::Post,
        params: &[
            ("app", ParamFrom::Response("/error/app")),
            ("level", ParamFrom::Response("/error/level")),
            ("caps", ParamFrom::Response("/error/caps")),
            ("uploads", ParamFrom::Response("/error/uploads")),
        ],
    }],
    coverage: WallCoverage::Code,
    machine_hints: &[],
};

/// The App `label` names in `workspace_id` only: its publish name, its
/// service id, or its runtime App id.
pub(crate) fn own_app(workspace_id: &str, label: &str) -> Option<OwnService> {
    if let Some(s) = own_service(workspace_id, label) {
        return Some(s);
    }
    let svc = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::deploy::store::get_app(&conn, &label.trim().to_ascii_lowercase())
            .ok()
            .flatten()
            .filter(|a| a.owner_workspace == workspace_id)
            .map(|a| a.service_id)
    }?;
    own_service(workspace_id, &svc)
}

/// A list value (`["a","b"]`, or a comma or space list).
fn caps_param(ctx: &Ctx) -> Vec<String> {
    match ctx.params.get("caps") {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        Some(Value::String(s)) => s.split([',', ' ']).map(str::trim).filter(|c| !c.is_empty()).map(str::to_string).collect(),
        _ => Vec::new(),
    }
}

/// The App and the route body (`{app, caps?, maxLevel?, uploads?}`, no
/// token) of one request, checked.
fn key_of(ctx: &Ctx) -> Result<(OwnService, Value), String> {
    let app = text_param(ctx, "app").ok_or("name your App: app=<its name>")?;
    let svc = own_app(&ctx.workspace_id, app).ok_or_else(|| format!("{} has no App named {app}", ctx.workspace_handle))?;
    let mut body = json!({ "app": svc.id });
    if let Some(l) = text_param(ctx, "level") {
        if !LEVELS.contains(&l) {
            return Err(format!("level={l}: member, moderator or admin"));
        }
        body["maxLevel"] = json!(l);
    }
    let caps = caps_param(ctx);
    if !caps.is_empty() {
        body["caps"] = json!(ck::normalize_caps(&caps).map_err(|e| format!("caps: {e}"))?);
    }
    if let Some(u) = text_param(ctx, "uploads") {
        if !UPLOADS.contains(&u) {
            return Err(format!("uploads={u}: on or off"));
        }
        body["uploads"] = json!(u == "on");
    }
    Ok((svc, body))
}

/// What the key would be, from the route's own rules.
fn preview_of(ctx: &Ctx) -> Result<(OwnService, Value, crate::apps::chat_key_routes::KeyPreview), String> {
    let (svc, body) = key_of(ctx)?;
    let p = crate::apps::chat_key_routes::preview(&svc.id, &body).map_err(|r| r.hint)?;
    Ok((svc, body, p))
}

fn caps_words(caps: &[String]) -> String {
    let words: Vec<&str> = caps
        .iter()
        .map(|c| match c.as_str() {
            "chat:read" => "read chat",
            "chat:post" => "post",
            "chat:respond" => "answer in threads",
            "voice:join" => "join calls",
            "voice:video" => "turn on video",
            other => other,
        })
        .collect();
    match words.as_slice() {
        [] => "nothing".into(),
        [one] => (*one).to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// `{app}` by name, `{what}` and `{effects}` from the stored values and the
/// key as it is now.
fn key_words(ctx: &Ctx, out: &mut Ctx) {
    let ws = &ctx.workspace_handle;
    let (what, effects) = match preview_of(ctx) {
        Ok((svc, _, p)) => {
            let n = &svc.name;
            out.params.insert("app".into(), json!(n));
            let w = &p.want;
            let ceiling = format!(
                "people may {}, up to level {}, up to {} people at once, uploads {}",
                caps_words(&w.caps),
                w.max_level.as_str(),
                w.max_members,
                if w.uploads { "on" } else { "off" }
            );
            let what = if p.before.as_ref().is_some_and(|b| b.enabled) {
                format!("Widen the chat key of {ws}'s App {n}: {ceiling}")
            } else {
                format!("Give {ws}'s App {n} a chat key, so K2 signs chat passes for its users: {ceiling}")
            };
            let owner = if p.admin_may {
                String::new()
            } else {
                " Only the server's owner can allow this one: it asks for level admin, chat caps past what the App's manifest asked for and the owner approved, more than 500 people, or uploads back on.".into()
            };
            (
                what,
                format!(
                    "{n}'s own users get chat with no setup per person, within that ceiling. K2 keeps the key and signs every pass; {n} never sees it, so it can't leak. The App decides who is in which channel only in a Space a person switches to App-governed.{owner} To take it back: k2 app grant {n} --kind chat-key --revoke (Sew then ends {n}'s chat sessions)."
                ),
            )
        }
        Err(e) => (format!("Give a chat key to one of {ws}'s Apps"), e),
    };
    out.params.insert("what".into(), json!(what));
    out.params.insert("effects".into(), json!(effects));
}

fn sew_flag() -> Result<bool, String> {
    k2_core::tool_hook::sew_has(k2_core::tool_hook::features::APP_GOVERNED_SPACES)
}

fn flag_missing() -> Check {
    Check::Unavailable {
        code: "sew_feature_missing",
        why: "this server's Sew can't do App-governed Spaces yet (it needs Sew 0.6.0: app_governed_spaces): the owner upgrades it (k2 hostchat upgrade)".into(),
    }
}

/// Still wanted: the App is the workspace's own, the values hold under the
/// route's rules, and they would turn the key on or widen it.
fn check_key(ctx: &Ctx) -> Check {
    let (_, _, p) = match preview_of(ctx) {
        Ok(v) => v,
        Err(e) => return Check::Moved(e),
    };
    match sew_flag() {
        Ok(true) => {}
        Ok(false) => return flag_missing(),
        Err(e) => return Check::Unavailable { code: "chat_unavailable", why: e },
    }
    if p.widens {
        Check::Needed
    } else {
        // Nothing to turn on or widen: narrowing is the agent's own to do.
        Check::AlreadyDone
    }
}

/// The route's own caller for who clicked.
fn route_who(who: &Answerer) -> Who {
    match who {
        Answerer::OwnerToken => Who::Owner,
        Answerer::Login { user, role } => Who::Login { username: user.clone(), role: Role::parse_wire(role).unwrap_or(Role::Member) },
    }
}

/// The card's confirm for this request, as the person who answered: the
/// method they confirmed with.
fn take_confirm(action: &str, who: &Answerer, token: Option<&str>) -> Result<(std::sync::Arc<k2_vault::LocalVault>, String), Applied> {
    let Some(token) = token else {
        return Err(Applied::Refused { code: "vault_confirm_required".into(), message: "confirm it's you first".into() });
    };
    let Some(request) = super::super::answer::current_request() else {
        return Err(Applied::Refused { code: "failed".into(), message: "no request is being answered".into() });
    };
    let vault = crate::vault::get().map_err(|e| Applied::Refused { code: e.code.into(), message: e.hint })?;
    match vault.confirm.take(token, action, Some(&request), &who.stamp()) {
        Ok(HumanProof::Confirmed { method }) => Ok((vault, method)),
        Ok(HumanProof::None) => Err(Applied::Refused { code: "vault_confirm_required".into(), message: "confirm again".into() }),
        Err(e) => Err(Applied::Refused { code: e.code.into(), message: e.hint }),
    }
}

fn refused_from(status: &str, body: &str, fallback: &str) -> Option<Applied> {
    if status.starts_with("200") {
        return None;
    }
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let (code, message) = error_of(&v, fallback);
    Some(Applied::Refused { code, message })
}

fn apply_key(ctx: &Ctx, who: &Answerer, token: Option<&str>) -> Applied {
    let (svc, mut body, p) = match preview_of(ctx) {
        Ok(v) => v,
        Err(e) => return Applied::Refused { code: "bad_params".into(), message: e },
    };
    if let Err(refused) = route_floor(KEY_ROUTE, who) {
        return refused;
    }
    // The chat key's own rule, before the confirm is spent: past the Admin
    // ceiling only the owner gives it. The card stays open for them.
    let owner = match who {
        Answerer::OwnerToken => true,
        Answerer::Login { role, .. } => matches!(Role::parse_wire(role), Ok(Role::Owner)),
    };
    if !owner && !p.admin_may {
        return Applied::Refused {
            code: OWNER_IN_PERSON.into(),
            message: "only the server's owner can allow this one (level admin, caps past the App's approved ones, more than 500 people, or uploads back on); it stays open for them".into(),
        };
    }
    let (vault, method) = match take_confirm(KEY_CONFIRM, who, token) {
        Ok(v) => v,
        Err(refused) => return refused,
    };
    // The route's own confirm, for this App, the same person and method.
    let route_token = vault.confirm.mint(crate::apps::chat_key_routes::CONFIRM_ACTION, Some(&svc.id), &who.stamp(), &method);
    body["confirmToken"] = json!(route_token);
    let r = crate::apps::guest_rooms::handle(KEY_ROUTE, &route_who(who), true, body.to_string().as_bytes(), &HashMap::new());
    if let Some(refused) = refused_from(r.status, &r.body, "chat_key_refused") {
        return refused;
    }
    let v: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
    let k = &v["key"];
    let caps: Vec<String> = k["caps"].as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
    Applied::Done {
        message: format!(
            "{} holds a chat key now: {}, up to level {}, up to {} people, uploads {}. Its server asks chat.pass() with the person's own session.",
            svc.name,
            caps.join(" "),
            k["maxLevel"].as_str().unwrap_or("?"),
            k["maxMembers"],
            if k["uploads"].as_bool() == Some(true) { "on" } else { "off" }
        ),
    }
}

// ── sew.space-governance ────────────────────────────────────────────

pub(crate) const GOV_ACTION: &str = "sew.space-governance";
pub(crate) const GOV_ROUTE: &str = crate::sew::s32::APP_SPACES_GOVERNANCE;
/// The card's confirm (`ConfirmAction::SewSpaceGovernance`), bound to the
/// request.
pub(crate) const GOV_CONFIRM: &str = "sew-space-governance";
pub(crate) const SIDES: [&str; 2] = ["app", "sew"];

pub(crate) static SPACE_GOVERNANCE: ActionSpec = ActionSpec {
    id: GOV_ACTION,
    area: Area::Sew,
    params: &[
        ParamSpec { key: "app", kind: ParamKind::App, required: true, list: false, choices: &[] },
        ParamSpec { key: "space", kind: ParamKind::SewSpace, required: true, list: false, choices: &[] },
        ParamSpec { key: "to", kind: ParamKind::Choice, required: true, list: false, choices: &SIDES },
    ],
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::SettingsConfirm { action: ConfirmAction::SewSpaceGovernance },
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "who governs chat Space {space_label}",
        what_changes: "{what}",
        consequences: "{effects}",
        scope: "chat Space {space_label}, for everyone in it",
        duration: "until someone switches it back",
        retry: "",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "sew", path: "k2 hostchat app-space governance <app> <space> app|sew (a terminal outside K2)" },
    walls: &[Wall {
        matches: WallMatch::Code { status: 403, code: "person_only" },
        route_prefix: GOV_ROUTE,
        method: WallMethod::Post,
        params: &[("app", ParamFrom::Body("/app")), ("space", ParamFrom::Body("/spaceId")), ("to", ParamFrom::Body("/governance"))],
    }],
    coverage: WallCoverage::Code,
    machine_hints: &[],
};

/// `<slug> (<id>)` where the asking workspace's chat access knows the
/// Space's slug, else its id.
fn space_label(workspace_id: &str, space: &str) -> String {
    let slug = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::sew_chat::store::list_access(&conn, workspace_id).ok()
    }
    .and_then(|rows| rows.into_iter().find(|r| r.space_id == space).map(|r| r.space_slug));
    match slug {
        Some(s) => format!("{s} ({space})"),
        None => space.to_string(),
    }
}

/// One governance request, checked: the App, the Space bound to it, the
/// side, and the side K2 holds now.
struct Switch {
    svc: OwnService,
    space: String,
    to: &'static str,
    now: String,
}

fn switch_of(ctx: &Ctx) -> Result<Switch, String> {
    let app = text_param(ctx, "app").ok_or("name your App: app=<its name>")?;
    let space = text_param(ctx, "space").ok_or("name the Space: space=<its id> (k2 hostchat app-spaces)")?;
    let to = match text_param(ctx, "to") {
        Some("app") => "app",
        Some("sew") => "sew",
        Some(other) => return Err(format!("to={other}: app or sew")),
        None => return Err("say which: to=app or to=sew".into()),
    };
    let svc = own_app(&ctx.workspace_id, app).ok_or_else(|| format!("{} has no App named {app}", ctx.workspace_handle))?;
    let bound = k2_core::tool_hook::app_spaces()?.into_iter().find(|v| v.binding.space_id == space && v.binding.app_id == svc.id);
    let Some(view) = bound else {
        return Err(format!("{space} isn't a Space bound to {}'s App {} (k2 hostchat app-spaces)", ctx.workspace_handle, svc.name));
    };
    Ok(Switch { svc, space: space.to_string(), to, now: view.governance.unwrap_or_else(|| "sew".into()) })
}

fn gov_words(ctx: &Ctx, out: &mut Ctx) {
    let ws = &ctx.workspace_handle;
    let label = text_param(ctx, "space").map(|s| space_label(&ctx.workspace_id, s)).unwrap_or_else(|| "?".into());
    out.params.insert("space_label".into(), json!(label));
    let (what, effects) = match switch_of(ctx) {
        Ok(s) => {
            let n = &s.svc.name;
            out.params.insert("app".into(), json!(n));
            if s.to == "app" {
                (
                    format!("Let {ws}'s App {n} govern the chat Space {label}: the App decides who is in which private channel there, and each person's level"),
                    format!(
                        "Sew makes {n}'s service owner the Space's owner. Box users in it (the owner, admins) leave the Space first; their messages stay. From then on K2 keeps every App user's private channels and level in step with their role in {n}, and people can't change the Space's channels or roles in chat. Switch it back with: k2 hostchat app-space governance {n} {} sew.",
                        s.space
                    ),
                )
            } else {
                (
                    format!("Give the chat Space {label} back to Sew's own rules (from {ws}'s App {n})"),
                    format!(
                        "People manage the Space in chat again, with Sew's roles; private channels and levels stay as they are. {n} can't set its people's channels there until someone switches it back."
                    ),
                )
            }
        }
        Err(e) => (format!("Switch who governs a chat Space of {ws}'s App"), e),
    };
    out.params.insert("what".into(), json!(what));
    out.params.insert("effects".into(), json!(effects));
}

fn check_gov(ctx: &Ctx) -> Check {
    let s = match switch_of(ctx) {
        Ok(s) => s,
        Err(e) => return Check::Moved(e),
    };
    match sew_flag() {
        Ok(true) => {}
        Ok(false) => return flag_missing(),
        Err(e) => return Check::Unavailable { code: "chat_unavailable", why: e },
    }
    if s.now == s.to {
        return Check::AlreadyDone;
    }
    if s.to == "app" {
        match ck::enabled(&s.svc.id) {
            Ok(Some(_)) => {}
            Ok(None) => {
                return Check::Moved(format!(
                    "{n} has no chat key, and an App governs its Spaces only with one: ask for that first: k2 permission request apps.chat-key app={n}",
                    n = s.svc.name
                ))
            }
            Err(e) => return Check::Unavailable { code: "chat_unavailable", why: e },
        }
    }
    Check::Needed
}

fn apply_gov(ctx: &Ctx, who: &Answerer, token: Option<&str>) -> Applied {
    let s = match switch_of(ctx) {
        Ok(s) => s,
        Err(e) => return Applied::Refused { code: "bad_params".into(), message: e },
    };
    if let Err(refused) = route_floor(GOV_ROUTE, who) {
        return refused;
    }
    let (vault, method) = match take_confirm(GOV_CONFIRM, who, token) {
        Ok(v) => v,
        Err(refused) => return refused,
    };
    let bound_to = crate::sew::governed::governance_secret_id(&s.svc.id, &s.space, s.to);
    let route_token = vault.confirm.mint(crate::sew::governed::CONFIRM_GOVERNANCE, Some(&bound_to), &who.stamp(), &method);
    let body = json!({ "app": s.svc.id, "spaceId": s.space, "governance": s.to, "confirmToken": route_token });
    let r = crate::sew::routes::handle(GOV_ROUTE, route_who(who), body.to_string().as_bytes(), &HashMap::new());
    if let Some(refused) = refused_from(r.status, &r.body, "governance_refused") {
        return refused;
    }
    let label = space_label(&ctx.workspace_id, &s.space);
    Applied::Done {
        message: if s.to == "app" {
            format!(
                "Chat Space {label} is App-governed now: {} decides who is in which private channel. Box users in it leave first (k2 app chat {} shows them).",
                s.svc.name, s.svc.name
            )
        } else {
            format!("Chat Space {label} is back on Sew's own rules.")
        },
    }
}

// ── shared ──────────────────────────────────────────────────────────

/// The card's words K2 computes from the stored values.
pub(crate) fn with_computed(action: &str, ctx: &Ctx) -> Option<Ctx> {
    let mut out = ctx.clone();
    match action {
        KEY_ACTION => key_words(ctx, &mut out),
        GOV_ACTION => gov_words(ctx, &mut out),
        _ => return None,
    }
    Some(out)
}

/// The macOS sheet's words for either card's confirm, from the request's
/// stored values, never the client's label. `None`: not these confirms.
pub(crate) fn confirm_reason(action: &str, request: Option<&str>) -> Option<Result<String, String>> {
    let card = match action {
        KEY_CONFIRM => KEY_ACTION,
        GOV_CONFIRM => GOV_ACTION,
        _ => return None,
    };
    let found = request.and_then(|id| crate::permission::store::get(id).ok().flatten()).filter(|r| r.action == card && r.is_answerable());
    Some(match found {
        None => Err("that card isn't waiting for an answer".to_string()),
        Some(r) => {
            let ctx = crate::permission::present::ctx_of(&r);
            let ctx = with_computed(card, &ctx).unwrap_or(ctx);
            Ok(crate::permission::present::fill("{what}", &ctx))
        }
    })
}

pub(crate) fn actions() -> Vec<Registered> {
    vec![
        Registered { spec: &CHAT_KEY, check: check_key, apply: apply_key },
        Registered { spec: &SPACE_GOVERNANCE, check: check_gov, apply: apply_gov },
    ]
}

pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    Vec::new()
}

#[cfg(test)]
#[path = "chat_cards_tests.rs"]
mod tests;
