//! Recording in this server's chat, asked by an agent on a card (0.46.17):
//!
//! - `voice.recording-start-level level=none|moderator|member`: who may
//!   start recordings in the K5 voice rooms. An agent's own
//!   `k2 voice settings --start-level <level>` is refused
//!   `recording_level_needs_owner` (`voice::routes`), and that refusal is
//!   the wall. `--require-recording` stays the owner's and isn't card-able.
//! - `voice.recording-dm dm=on|off`: recording in direct messages
//!   (`recording.dm_enabled`). An agent's `k2 hostchat recording policy set
//!   dm_enabled on|off` is refused `person_only`; the wall matches only a
//!   body whose `changes` is that one key (`walls::ROWS`). A person's own
//!   path stays `k2 hostchat recording policy set` from a terminal outside
//!   K2.
//!
//! - **Who answers:** the owner or an Admin (the routes' floor), once, with
//!   a confirm bound to the request (Touch ID, the vault password or the
//!   Connect password). A Member's Allow is refused and changes nothing.
//! - **On Allow:** the approver's side applies it, through the route's own
//!   entry as the person who clicked; the agent runs nothing. The dm card's
//!   confirm becomes the policy route's own (`recording-policy`, bound to
//!   this install) for the same person.
//! - **Then:** `access_changed` goes to each login whose right to start a
//!   recording changed (`login_access_live`'s frame on the sessions and
//!   voice sockets), so their app reads its recording controls again. Sew's
//!   own `perms_changed` still reaches the people in a call.

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::permission::catalog::{
    ActionSpec, Area, ConfirmAction, Direction, Expiry, Floor, Grant, NotCardableSpec, ParamFrom, ParamKind, ParamSpec, Proof,
    SettingsRef, Undo, Wall, WallCoverage, WallMatch, WallMethod, Words,
};
use k2_core::tool_hook::sew_wire::recording as wire;
use k2_vault::HumanProof;
use serde_json::{json, Value};

use super::held::{error_of, route_floor, text_param};
use super::{Answerer, Applied, Check, Ctx, Registered};
use crate::compute_routes::Who;
use crate::voice::routes::Caller;
use k2_core::brand::CLI_NAME;
use k2_core::{cli_name, product_name};

pub(crate) const START_ACTION: &str = "voice.recording-start-level";
pub(crate) const DM_ACTION: &str = "voice.recording-dm";
/// The cards' confirms, bound to the request.
pub(crate) const START_CONFIRM: &str = "voice-recording-start-level";
pub(crate) const DM_CONFIRM: &str = "voice-recording-dm";
pub(crate) const START_ROUTE: &str = crate::voice::routes::RECORDING_SETTINGS;
pub(crate) const DM_ROUTE: &str = crate::sew::recording_settings::POLICY;
pub(crate) const ON_OFF: [&str; 2] = ["on", "off"];

pub(crate) static START_LEVEL: ActionSpec = ActionSpec {
    id: START_ACTION,
    area: Area::Sew,
    params: &[ParamSpec { key: "level", kind: ParamKind::Choice, required: true, list: false, choices: &crate::voice::routes::CARD_LEVELS }],
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::SettingsConfirm { action: ConfirmAction::VoiceRecordingStartLevel },
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "Who starts recordings",
        what_changes: "{what}",
        consequences: concat!(
            "This is the voice rooms' rule for everyone on this server, not only {ws}. Each person in a call still answers their own recording notice: nobody is recorded without saying yes. To change it again: ",
            cli_name!(),
            " voice settings --start-level none|moderator|member (the owner or an admin)."
        ),
        scope: "this server's voice rooms",
        duration: "until someone changes it",
        retry: "",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "voice", path: concat!(cli_name!(), " voice settings --start-level <level> (the owner or an admin)") },
    walls: &[Wall {
        matches: WallMatch::Code { status: 403, code: crate::voice::routes::LEVEL_NEEDS_OWNER },
        route_prefix: START_ROUTE,
        method: WallMethod::Post,
        params: &[("level", ParamFrom::Body("/startLevel"))],
    }],
    coverage: WallCoverage::Code,
    form: None,
    machine_hints: &[],
};

pub(crate) static DM: ActionSpec = ActionSpec {
    id: DM_ACTION,
    area: Area::Sew,
    params: &[ParamSpec { key: "dm", kind: ParamKind::Choice, required: true, list: false, choices: &ON_OFF }],
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::SettingsConfirm { action: ConfirmAction::VoiceRecordingDm },
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "Recording in direct messages",
        what_changes: "{what}",
        consequences: "{effects}",
        scope: "this server's chat, for everyone",
        duration: "until someone changes it",
        retry: "",
    },
    undo: Undo::None,
    settings: SettingsRef {
        page: "sew",
        path: concat!(cli_name!(), " hostchat recording policy set dm_enabled on|off (a terminal outside ", product_name!(), ")"),
    },
    // `walls::ROWS` matches only a body whose `changes` is exactly
    // `recording.dm_enabled` (true gives dm=on, false dm=off).
    walls: &[Wall { matches: WallMatch::Code { status: 403, code: "person_only" }, route_prefix: DM_ROUTE, method: WallMethod::Post, params: &[] }],
    coverage: WallCoverage::Code,
    form: None,
    machine_hints: &[],
};

/// A level in the card's words.
pub(crate) fn level_words(level: &str) -> &'static str {
    match level {
        "none" => "nobody",
        "moderator" => "moderators and above",
        "member" => "every member",
        _ => "someone",
    }
}

fn level_of(ctx: &Ctx) -> Result<&str, String> {
    match text_param(ctx, "level") {
        Some(l) if crate::voice::routes::CARD_LEVELS.contains(&l) => Ok(l),
        Some(l) => Err(format!("level={l}: none, moderator or member")),
        None => Err("say who: level=none, level=moderator or level=member".into()),
    }
}

fn dm_of(ctx: &Ctx) -> Result<bool, String> {
    match text_param(ctx, "dm") {
        Some("on") => Ok(true),
        Some("off") => Ok(false),
        Some(other) => Err(format!("dm={other}: on or off")),
        None => Err("say which: dm=on or dm=off".into()),
    }
}

/// The K5 rooms' start level now (`None`: it inherits the org's rule), and
/// whether the Space is locked against recording.
fn k5_level() -> Result<(Option<String>, bool), String> {
    let space = k2_core::tool_hook::recording_k5_space()?.ok_or("the voice rooms' Space isn't made yet")?;
    let (level, _, locked) = k2_core::tool_hook::recording_space(&space)?;
    Ok((level, locked))
}

fn app_recording_missing() -> Check {
    Check::Unavailable {
        code: "sew_feature_missing",
        why: concat!("this server's chat can't record yet (it needs Sew 0.9.0: app_recording): the owner upgrades it (", cli_name!(), " hostchat upgrade)").into(),
    }
}

fn app_recording() -> bool {
    k2_core::tool_hook::sew_recording_features().map(|f| f.app_recording).unwrap_or(false)
}

fn check_start(ctx: &Ctx) -> Check {
    let level = match level_of(ctx) {
        Ok(l) => l,
        Err(e) => return Check::Moved(e),
    };
    if !app_recording() {
        return app_recording_missing();
    }
    match k5_level() {
        Err(e) => Check::Unavailable { code: "voice_space_not_ready", why: e },
        Ok((_, true)) if level != "none" => Check::Unavailable {
            code: "space_locked",
            why: "the voice rooms are locked against recording (CUI/FCI): only the owner lifts it, in person".into(),
        },
        Ok((now, _)) if now.as_deref() == Some(level) => Check::AlreadyDone,
        Ok(_) => Check::Needed,
    }
}

/// The DM policy as K2 asked for it, else as Sew shows it.
fn dm_now() -> Option<bool> {
    let rows = k2_core::tool_hook::recording_policies().ok()?;
    let row = rows.into_iter().find(|r| r.key == wire::POLICY_DM_ENABLED)?;
    row.want.as_ref().and_then(Value::as_bool).or_else(|| row.seen.as_ref().and_then(Value::as_bool))
}

fn check_dm(ctx: &Ctx) -> Check {
    let on = match dm_of(ctx) {
        Ok(b) => b,
        Err(e) => return Check::Moved(e),
    };
    if !app_recording() {
        return app_recording_missing();
    }
    if dm_now() == Some(on) {
        Check::AlreadyDone
    } else {
        Check::Needed
    }
}

/// The card's confirm, taken for this request and this person.
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

/// The person who clicked, at their **current** role.
fn role_now(who: &Answerer) -> Role {
    match who {
        Answerer::OwnerToken => Role::Owner,
        Answerer::Login { user, .. } => k2_core::connect_users::role_for_user(user).unwrap_or(Role::Member),
    }
}

fn voice_caller(who: &Answerer) -> Caller {
    match who {
        Answerer::OwnerToken => Caller::OwnerToken,
        Answerer::Login { user, .. } => Caller::Login { username: user.clone(), role: role_now(who), token: String::new() },
    }
}

fn sew_who(who: &Answerer) -> Who {
    match who {
        Answerer::OwnerToken => Who::Owner,
        Answerer::Login { user, .. } => Who::Login { username: user.clone(), role: role_now(who) },
    }
}

/// An Admin or the owner, now (a login demoted since the card was drawn is
/// what it is now). Checked before the confirm is spent.
fn floor_now(path: &str, who: &Answerer) -> Result<(), Applied> {
    route_floor(path, who)?;
    if role_now(who) < Role::Admin {
        return Err(Applied::Refused {
            code: "owner_or_admin_only".into(),
            message: "only the server's owner or an admin allows this".into(),
        });
    }
    Ok(())
}

/// May a login of `role` start a recording at `level`? `None` (inherit)
/// answers `None`: K2 doesn't know the org's rule here.
fn may_start(level: Option<&str>, role: Role) -> Option<bool> {
    if role == Role::VoiceChat {
        // A Voice & Chat only login never records (`voice::routes`).
        return Some(false);
    }
    match level? {
        "member" => Some(true),
        "moderator" => Some(role >= Role::Admin),
        _ => Some(false),
    }
}

/// The logins whose right to start a recording differs between `before`
/// and `after`, from `(id, username, role, disabled)` rows. An unknown side
/// (inherit) counts as a change for everyone.
pub(crate) fn start_right_changed(before: Option<&str>, after: Option<&str>, logins: &[(String, String, Role, bool)]) -> Vec<String> {
    logins
        .iter()
        .filter(|(_, _, _, disabled)| !disabled)
        .filter(|(_, _, role, _)| {
            let (b, a) = (may_start(before, *role), may_start(after, *role));
            b.is_none() || a.is_none() || b != a
        })
        .map(|(_, name, _, _)| name.clone())
        .collect()
}

/// `access_changed` for each login, through the login hub's own frame.
fn tell(logins: Vec<String>) -> usize {
    let n = logins.len();
    for login in logins {
        let _ = crate::session_events::emit(crate::session_events::SessionEvent::AccessChanged { login });
    }
    n
}

fn logins() -> Vec<(String, String, Role, bool)> {
    k2_core::connect_users::admin_space_facts().unwrap_or_default()
}

fn apply_start(ctx: &Ctx, who: &Answerer, token: Option<&str>) -> Applied {
    let level = match level_of(ctx) {
        Ok(l) => l.to_string(),
        Err(e) => return Applied::Refused { code: "bad_params".into(), message: e },
    };
    if let Err(refused) = floor_now(START_ROUTE, who) {
        return refused;
    }
    if let Err(refused) = take_confirm(START_CONFIRM, who, token) {
        return refused;
    }
    let before = k5_level().ok().and_then(|(l, _)| l);
    let r = crate::voice::routes::recording_settings_as(&voice_caller(who), &json!({ "startLevel": level }));
    if !r.status.starts_with("200") {
        let v: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
        let (code, message) = error_of(&v, "recording_refused");
        return Applied::Refused { code, message };
    }
    tell(start_right_changed(before.as_deref(), Some(&level), &logins()));
    Applied::Done {
        message: format!(
            "{} may start recordings in this server's voice rooms now. Each person still answers their own recording notice.",
            capital(level_words(&level))
        ),
    }
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn apply_dm(ctx: &Ctx, who: &Answerer, token: Option<&str>) -> Applied {
    let on = match dm_of(ctx) {
        Ok(b) => b,
        Err(e) => return Applied::Refused { code: "bad_params".into(), message: e },
    };
    if let Err(refused) = floor_now(DM_ROUTE, who) {
        return refused;
    }
    let (vault, method) = match take_confirm(DM_CONFIRM, who, token) {
        Ok(v) => v,
        Err(refused) => return refused,
    };
    let route_token =
        vault.confirm.mint(crate::sew::recording_settings::CONFIRM_POLICY, Some(&crate::sew::routes::confirm_secret_id()), &who.stamp(), &method);
    let body = json!({ "changes": { wire::POLICY_DM_ENABLED: on }, "confirmToken": route_token });
    let r = crate::sew::routes::handle(DM_ROUTE, sew_who(who), body.to_string().as_bytes(), &HashMap::new());
    if !r.status.starts_with("200") {
        let v: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
        let (code, message) = error_of(&v, "recording_refused");
        return Applied::Refused { code, message };
    }
    // Every login may be in a direct message: each one's right changed.
    tell(logins().into_iter().filter(|(_, _, role, disabled)| !disabled && *role != Role::VoiceChat).map(|(_, name, _, _)| name).collect());
    Applied::Done {
        message: if on {
            "Recording in direct messages is on: either person in a DM call can press Record, and the other answers the notice.".into()
        } else {
            "Recording in direct messages is off. Recordings already made are kept.".into()
        },
    }
}

fn words(action: &str, ctx: &Ctx, out: &mut Ctx) {
    let ws = &ctx.workspace_handle;
    if action == START_ACTION {
        let what = match level_of(ctx) {
            Ok(l) => format!("Let {} start recordings in this server's chat ({ws} asked)", level_words(l)),
            Err(_) => format!("Change who may start recordings in this server's chat ({ws} asked)"),
        };
        out.params.insert("what".into(), json!(what));
        return;
    }
    let (what, effects) = match dm_of(ctx) {
        Ok(true) => (
            format!("Let people record calls in direct messages on this server's chat ({ws} asked)"),
            "In a DM call either person can then press Record; the other sees a notice and answers it, and nobody is recorded without saying yes.".to_string(),
        ),
        Ok(false) => (
            format!("Turn off recording in direct messages on this server's chat ({ws} asked)"),
            "DM calls stop offering Record. Recordings already made are kept.".to_string(),
        ),
        Err(e) => (format!("Change recording in direct messages ({ws} asked)"), e),
    };
    out.params.insert("what".into(), json!(what));
    out.params.insert(
        "effects".into(),
        json!(format!("{effects} This is for everyone on this server, not only {ws}. To change it again: {CLI_NAME} hostchat recording policy set dm_enabled on|off (the owner or an admin, in person).")),
    );
}

/// The cards' words K2 computes from the stored values.
pub(crate) fn with_computed(action: &str, ctx: &Ctx) -> Option<Ctx> {
    if action != START_ACTION && action != DM_ACTION {
        return None;
    }
    let mut out = ctx.clone();
    words(action, ctx, &mut out);
    Some(out)
}

/// The macOS sheet's words for a card's confirm, from the request's stored
/// values. `None`: not one of these confirms.
pub(crate) fn confirm_reason(action: &str, request: Option<&str>) -> Option<Result<String, String>> {
    let card = match action {
        START_CONFIRM => START_ACTION,
        DM_CONFIRM => DM_ACTION,
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
        Registered { spec: &START_LEVEL, check: check_start, apply: apply_start },
        Registered { spec: &DM, check: check_dm, apply: apply_dm },
    ]
}

pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    Vec::new()
}

#[cfg(test)]
#[path = "voice_recording_tests.rs"]
mod tests;
