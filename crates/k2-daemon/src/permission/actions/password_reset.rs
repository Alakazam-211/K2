//! `apps.user-password-reset` (Rosson 2026-10-29, 0.46.6 phase 1;
//! `prd-password-reset-card-v1.md` + its vs-live PR-A1 to PR-A12): an
//! App's own agent asks a person to reset an App user's password, and the
//! person who allows it is the only one who ever sees the result.
//!
//! - **Who asks:** an agent passport only (`/cli/permission/request`
//!   refuses anything else), for a person who holds a grant on an App of
//!   the asking workspace (`skin::ties::app_workspace(app) == ws`). Never a
//!   test guest (its agent sets its password itself). Asking needs no
//!   switch: the approver decides.
//! - **Who answers:** [`FLOOR`], the same floor as every route the card
//!   calls (`/cli/skin/users/password`, `/unlock` and `/reset-link`; PR-A2).
//!   Admins since Rosson's Q1 (2026-10-29), for App-only users: a person
//!   with a box login stays the owner's, so their request is stored with
//!   the Owner floor ([`floor_for`]) and the routes' shared rule
//!   (`skin_routes::owner_or_admin_for`) refuses an Admin at the click too.
//!   An Admin's reset files the owner one Inbox line (that same rule).
//! - **Modes** (`mode=`, default `password`, Rosson 2026-10-29):
//!   - `password`: the route's own `generate: true` (one generator, in
//!     k2-core, shared with Settings → User Access), then its unlock. Every
//!     session ends; the tie stays (only a used owner reset drops a tie).
//!   - `link`: an `owner_reset` link (24 h, once) through the reset-link
//!     route with `code: true`; a bare code when the box has no front door.
//!   - `email`: the same link, mailed by the route to the stored email from
//!     a sender no agent reads. Offered only when the route would send it
//!     ([`email_blocker`]); the agent never passes `to` or `from`.
//! - **The secret goes to the person who allowed it, once:** `apply` hands
//!   it back in that HTTP answer only (`forYou`, PR-A3). Nothing is stored,
//!   logged or audited with it; the card, the Thread, the stored request and
//!   the agent's turn say only who reset it and when. A client that can't
//!   show it (an older build, the phone, a terminal) is refused
//!   [`REVEAL_NEEDS_SCREEN`] and nothing is decided (PR-A3 sub-B); `email`
//!   needs no screen.
//! - **Never "Allow all"** (PR-A4): `forYou` holds one value, so this card
//!   never joins a batch and is never covered by a batch's Allow all.
//! - **Apply** calls each route's own entry as the approver, never the
//!   agent path (which would tie the person; PR-A6).

use k2_core::permission::catalog::{
    ActionSpec, Area, Direction, Expiry, Floor, Grant, NotCardableSpec, ParamKind, ParamSpec, Proof, SettingsRef, Undo,
    Wall, WallCoverage, WallMatch, WallMethod, Words,
};
use k2_core::permission::card::RevealView;
use k2_core::permission::request::PermissionRequest;
use k2_core::skin;
use serde_json::{json, Value};

use super::held::{error_of, route_floor, text_param};
use super::{Answerer, Applied, Check, Ctx, Registered};
use crate::cli_response::CliResponse;

/// The card's id (PR-A1: the catalog allows only `a-z 0-9 . -`).
pub(crate) const ACTION: &str = "apps.user-password-reset";

/// Who answers: the floor of every route the card calls (PR-A2): the
/// owner or an Admin (Rosson Q1, 2026-10-29), for App-only users; a box
/// login's request is Owner ([`floor_for`]). The walls map reads this.
pub(crate) const FLOOR: Floor = Floor::Admin;

/// A client that can't show a secret answered Allow on a password or a
/// link: nothing is decided, and the request stays open.
pub(crate) const REVEAL_NEEDS_SCREEN: &str = "reveal_needs_screen";

/// What that refusal says.
pub(crate) const REVEAL_HINT: &str = "From here you can only email them a link (when the card offers it). To see a new password or link, open this card in K2 on your computer or the web: it is shown there once, only to you.";

/// How long the card shows the secret before wiping it.
pub(crate) const REVEAL_SECS: i64 = 60;

/// The modes, in the order `k2 permission list` shows them.
pub(crate) const MODES: &[&str] = &["password", "link", "email"];

/// The mode when the agent names none (Rosson 2026-10-29): K2 makes a
/// password and shows it once.
const DEFAULT_MODE: &str = "password";

const ROUTE_PASSWORD: &str = "/cli/skin/users/password";
const ROUTE_UNLOCK: &str = "/cli/skin/users/unlock";
const ROUTE_RESET_LINK: &str = crate::app_user_invites::RESET_LINK_ROUTE;

/// The audit event of an approved card (the routes add their own lines).
pub(crate) const AUDIT_EVENT: &str = "app_user_password_reset_card";

pub(crate) static PASSWORD_RESET: ActionSpec = ActionSpec {
    id: ACTION,
    area: Area::Apps,
    params: &[
        ParamSpec { key: "user", kind: ParamKind::AppUser, required: true, list: false, choices: &[] },
        ParamSpec { key: "app", kind: ParamKind::App, required: false, list: false, choices: &[] },
        ParamSpec { key: "mode", kind: ParamKind::Choice, required: false, list: false, choices: MODES },
    ],
    floor: FLOOR,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    // Parity with Settings → User Access and `k2 app user reset`: a click.
    proof: Proof::Click,
    // A lockout is urgent; a stale ask shouldn't sit.
    expiry: Expiry { default_secs: 24 * 3600, max_secs: 3 * 24 * 3600 },
    words: Words {
        setting: "Reset {user}'s password ({app})",
        what_changes: "{made}; {ws} never sees it",
        consequences: "{effects}",
        scope: "{user} on {app}",
        duration: "once",
        retry: "",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "people", path: "Settings → User Access" },
    walls: &[
        Wall {
            matches: WallMatch::Code { status: 403, code: "owner_only" },
            route_prefix: ROUTE_RESET_LINK,
            method: WallMethod::Post,
            params: &[],
        },
        Wall {
            matches: WallMatch::Code { status: 403, code: "owner_only" },
            route_prefix: ROUTE_PASSWORD,
            method: WallMethod::Post,
            params: &[],
        },
    ],
    coverage: WallCoverage::PrefixOnly,
    machine_hints: &[],
};

/// Does this action hand a secret to the person who allowed it? Such an
/// action never joins a batch and is never in a batch's "Allow all".
pub(crate) fn is_reveal(action: &str) -> bool {
    action == ACTION
}

/// Does `mode` show a secret on the approver's screen?
pub(crate) fn needs_screen(mode: &str) -> bool {
    mode != "email"
}

fn mode_of(ctx: &Ctx) -> &str {
    text_param(ctx, "mode").unwrap_or(DEFAULT_MODE)
}

/// Who and where, resolved from the request's values.
struct Target {
    person: skin::SkinPrincipal,
    /// The App's id.
    app: String,
    /// The App's name, for people.
    app_name: String,
    /// The person holds a box login (a `host` grant): the owner's alone.
    box_login: bool,
}

fn app_name(app: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::published_services::get_by_id(&conn, app).ok().flatten().map(|s| s.name).unwrap_or_else(|| app.to_string())
}

/// The person and the App, or why the request can't go on (for the
/// asking agent: names only, never an email).
fn target(ctx: &Ctx) -> Result<Target, String> {
    let Some(user) = text_param(ctx, "user") else { return Err("name the person: user=<username>".into()) };
    let found = skin::login_principal_username(user)?;
    let person = match found {
        Some(u) => skin::list_principals()?.into_iter().find(|p| p.username == u),
        None => None,
    };
    let Some(person) = person else { return Err(format!("no App user {user:?} (k2 app user list)")) };
    if skin::test_guests::get(&person.id)?.is_some() {
        return Err(format!(
            "'{}' is a test guest (a short-lived user for testing an App): set its password yourself (k2 app user password {} --password-stdin)",
            person.username, person.username
        ));
    }
    let grants = skin::list_grants()?;
    let apps: Vec<String> = grants
        .iter()
        .filter(|g| g.enabled && g.subject_kind == "principal" && g.subject_id == person.id && g.kind == "app")
        .map(|g| g.target_id.clone())
        .collect();
    let ws = ctx.workspace_id.as_str();
    let mine = |a: &str| a != skin::HOST_APP_ID && skin::ties::app_workspace(a).as_deref() == Some(ws);
    let app = match text_param(ctx, "app") {
        Some(raw) => crate::skin_routes::resolve_app_target(raw)?,
        // K2 picks: the App the person's tie names, then any App of yours
        // they hold.
        None => {
            let tie_ws = skin::ties::get(&person.id)?.map(|t| t.workspace_id);
            let pick = apps
                .iter()
                .filter(|a| mine(a))
                .find(|a| tie_ws.is_some() && skin::ties::app_workspace(a) == tie_ws)
                .or_else(|| apps.iter().find(|a| mine(a)));
            match pick {
                Some(a) => a.clone(),
                None => return Err(format!("{} doesn't use any App of {}: only the owner resets them (Settings → User Access)", person.username, ctx.workspace_handle)),
            }
        }
    };
    if !mine(&app) {
        return Err(format!("{} isn't an App of {}: only the owner resets people there (Settings → User Access)", app_name(&app), ctx.workspace_handle));
    }
    if !apps.iter().any(|a| a == &app) {
        return Err(format!("{} has no access to {}", person.username, app_name(&app)));
    }
    let app_name = app_name(&app);
    let box_login = crate::skin_routes::has_box_login(&person.id, &grants);
    Ok(Target { person, app, app_name, box_login })
}

/// Why the reset-link route wouldn't mail this person a link right now
/// (PR-A9: the route's own refusals, in its order), or `None` when it
/// would. Never names the address.
fn email_blocker(t: &Target) -> Option<(&'static str, String)> {
    let u = &t.person.username;
    match crate::app_user_invites::k2_set_password_page() {
        Ok(Some(_)) => {}
        Ok(None) => return Some(("no_front_door", "this box has no front door, so K2 has no set-password page to email: ask again with mode=password or mode=link".into())),
        Err(why) => return Some(("front_door_unsafe", format!("{why}: ask again with mode=password or mode=link"))),
    }
    match skin::ties::stored_email_refused(&t.person.id) {
        Ok(None) => {}
        Ok(Some(_)) => {
            return Some((
                "email_unconfirmed",
                format!("{u}'s stored email isn't one the owner confirmed (an agent could have chosen it), so K2 won't mail a reset there: ask again with mode=password or mode=link"),
            ))
        }
        Err(e) => return Some(("failed", e)),
    }
    if t.person.email.as_deref().is_none_or(|e| e.trim().is_empty()) {
        return Some(("no_email", format!("{u} has no email on file: ask again with mode=password or mode=link")));
    }
    match crate::app_user_invites::owner_reset_sender(&t.app, None) {
        Ok(Some(_)) => None,
        _ => Some((
            "owner_sender_required",
            "this box has no sender no agent reads (Settings → Email Notifications): ask again with mode=password or mode=link".into(),
        )),
    }
}

/// The values K2 computes for the card's words (`present::words_raw`):
/// `{made}` and `{effects}` for the mode, and `{app}` / `{user}` when the
/// request names none. `None` for every other action. With no `user` (the
/// list), words for "the person".
pub(crate) fn with_computed(action: &str, ctx: &Ctx) -> Option<Ctx> {
    if action != ACTION {
        return None;
    }
    let mut out = ctx.clone();
    let user = text_param(ctx, "user").map(str::to_string);
    let who = user.clone().unwrap_or_else(|| "the person".to_string());
    if user.is_none() {
        out.params.insert("user".into(), Value::String("the person".into()));
    }
    // The App by its name (the agent may have named it by id or
    // `<ws>/<name>`; K2 picks one when it named none).
    let shown = match text_param(ctx, "app") {
        Some(raw) => Some(crate::skin_routes::resolve_app_target(raw).map(|id| app_name(&id)).unwrap_or_else(|_| raw.to_string())),
        None if user.is_some() => target(ctx).ok().map(|t| t.app_name),
        None => None,
    };
    out.params.insert("app".into(), Value::String(shown.unwrap_or_else(|| "their App".into())));
    let (made, effects) = match mode_of(ctx) {
        "link" => (
            format!("K2 makes a one-time link for {who} to choose a new password and shows it to you once"),
            format!("Give {who} the link yourself. When they use it (once, within 24 hours) they choose their own password, are signed out everywhere and their sign-in lock is cleared."),
        ),
        "email" => (
            format!("K2 emails {who} a one-time link to choose a new password, at their address on file"),
            format!("When {who} uses the link (once, within 24 hours) they choose their own password, are signed out everywhere and their sign-in lock is cleared."),
        ),
        // password (the default)
        _ => (
            format!("K2 makes a new password for {who} and shows it to you once"),
            format!("{who} is signed out everywhere and their sign-in lock is cleared. Their old password stops working at once. Tell or send {who} the new one yourself. The next time they sign in, K2 asks them to choose a new password."),
        ),
    };
    out.params.insert("made".into(), Value::String(made));
    out.params.insert("effects".into(), Value::String(effects));
    Some(out)
}

/// The floor a request is stored with, where it isn't the catalog's: a
/// person with a box login is the owner's alone (Rosson Q1). `None`: the
/// catalog's floor.
pub(crate) fn floor_for(action: &str, ctx: &Ctx) -> Option<Floor> {
    if action != ACTION {
        return None;
    }
    target(ctx).ok().filter(|t| t.box_login).map(|_| Floor::Owner)
}

/// `view.reveal` for this action's card (`None` for every other action).
pub(crate) fn reveal_view(req: &PermissionRequest) -> Option<RevealView> {
    if req.action != ACTION {
        return None;
    }
    let ctx = super::super::present::ctx_of(req);
    let mode = mode_of(&ctx).to_string();
    let email_offered = req.is_answerable() && target(&ctx).ok().is_some_and(|t| email_blocker(&t).is_none());
    Some(RevealView { mode, email_offered })
}

/// Still wanted, at the ask, at render and at the click.
fn check(ctx: &Ctx) -> Check {
    let t = match target(ctx) {
        Ok(t) => t,
        Err(why) => return Check::Moved(why),
    };
    if mode_of(ctx) == "email" {
        if let Some((code, why)) = email_blocker(&t) {
            return Check::Unavailable { code, why };
        }
    }
    Check::Needed
}

/// The answer's own gate, before anything is decided (PR-A3 sub-B, PR-A5):
/// the mode the person picked (`mode`, else the asked one) must be one of
/// the card's and, for `email`, work now; a password or a link needs a
/// client that says it can show it (`reveal`) and isn't the phone or a
/// terminal. Returns the mode to apply. A refusal leaves the request open.
pub(crate) fn gate_answer(req: &PermissionRequest, mode: Option<&str>, reveal: bool, via: &str) -> Result<String, CliResponse> {
    let ctx = super::super::present::ctx_of(req);
    let mode = mode.map(str::trim).filter(|m| !m.is_empty()).unwrap_or_else(|| mode_of(&ctx)).to_string();
    if !MODES.contains(&mode.as_str()) {
        return Err(super::super::answer::err(400, "usage", format!("mode is one of {}", MODES.join(", "))));
    }
    if mode == "email" {
        let blocked = match target(&ctx) {
            Ok(t) => email_blocker(&t),
            Err(why) => Some(("stale", why)),
        };
        if let Some((code, why)) = blocked {
            return Err(super::super::answer::err(409, code, why));
        }
    }
    if needs_screen(&mode) && !(reveal && !matches!(via, "phone" | "terminal")) {
        return Err(super::super::answer::err(403, REVEAL_NEEDS_SCREEN, REVEAL_HINT));
    }
    Ok(mode)
}

/// The route's own caller string for the approver (the dispatcher's
/// `owner_role_identity`): `owner-token` or `user:<login>`.
fn actor_of(who: &Answerer) -> String {
    match who {
        Answerer::OwnerToken => "owner-token".to_string(),
        Answerer::Login { user, .. } => format!("user:{user}"),
    }
}

/// How the card and the agent name the approver.
fn who_word(who: &Answerer) -> String {
    match who {
        Answerer::OwnerToken => "the owner".to_string(),
        Answerer::Login { user, .. } => user.clone(),
    }
}

fn now_hm() -> String {
    chrono::Local::now().format("%H:%M").to_string()
}

fn parsed(r: &CliResponse) -> Value {
    serde_json::from_str(&r.body).unwrap_or(Value::Null)
}

fn refused(r: &CliResponse, fallback: &str) -> Applied {
    let (code, message) = error_of(&parsed(r), fallback);
    Applied::Refused { code, message }
}

fn audit(t: &Target, mode: &str, who: &Answerer, ctx: &Ctx) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        AUDIT_EVENT,
        &actor_of(who),
        format!(
            "user={} app={} mode={mode} approved_by={} asked_by=agent:{}",
            t.person.username,
            t.app_name,
            actor_of(who),
            ctx.workspace_handle
        ),
        "-",
        "-",
        "cli",
    ));
}

/// Allow: the routes' own entries, as the approver (PR-A6). The secret
/// goes back in this answer only (`forYou`); the message, for the card and
/// the agent, names who and when.
fn apply(ctx: &Ctx, who: &Answerer, _token: Option<&str>) -> Applied {
    let answer = super::super::answer::current_answer();
    let mode = answer.mode.clone().unwrap_or_else(|| mode_of(ctx).to_string());
    // The answer path checked this before deciding; checked again here so
    // no other caller can reach a reveal without a screen.
    if needs_screen(&mode) && !answer.reveal {
        return Applied::Refused { code: REVEAL_NEEDS_SCREEN.into(), message: REVEAL_HINT.into() };
    }
    let t = match target(ctx) {
        Ok(t) => t,
        Err(why) => return Applied::Refused { code: "stale".into(), message: why },
    };
    let routes: &[&str] = if mode == "password" { &[ROUTE_PASSWORD, ROUTE_UNLOCK] } else { &[ROUTE_RESET_LINK] };
    for r in routes {
        if let Err(refusal) = route_floor(r, who) {
            return refusal;
        }
    }
    let actor = actor_of(who);
    let by = who_word(who);
    let u = t.person.username.clone();
    k2_core::log_debug!("[permission] {ACTION}: mode={mode} user={u} app={} by={actor} asked_by={}", t.app_name, ctx.workspace_handle);
    let applied = match mode.as_str() {
        "password" => {
            let r = crate::skin_routes::handle_users_password(json!({ "username": u, "generate": true }).to_string().as_bytes(), &actor);
            if !r.status.starts_with("200") {
                return refused(&r, "failed");
            }
            let v = parsed(&r);
            let Some(secret) = v["generatedPassword"].as_str().filter(|s| !s.is_empty()) else {
                return Applied::Refused { code: "failed".into(), message: "K2 set no password".into() };
            };
            let unlock = crate::skin_routes::handle_users_unlock(json!({ "username": u }).to_string().as_bytes(), &actor);
            let lock = if unlock.status.starts_with("200") {
                "their sign-in lock is cleared".to_string()
            } else {
                format!("their sign-in lock was NOT cleared ({}); use Unlock in Settings → User Access", error_of(&parsed(&unlock), "failed").1)
            };
            super::super::answer::set_for_answerer(json!({
                "kind": ACTION,
                "mode": "password",
                "username": u,
                "app": t.app_name,
                "secret": secret,
                "revealSecs": REVEAL_SECS,
            }));
            format!(
                "Password reset by {by} at {} (shown to {by} once). {u} is signed out everywhere and {lock}. {} never sees the password.",
                now_hm(),
                ctx.workspace_handle
            )
        }
        "link" => {
            let body = json!({ "username": u, "app": t.app, "code": true });
            let r = crate::app_user_invites::handle_reset_link(body.to_string().as_bytes(), &actor, true);
            if !r.status.starts_with("200") {
                return refused(&r, "failed");
            }
            let v = parsed(&r);
            let mut for_you = json!({
                "kind": ACTION,
                "mode": "link",
                "username": u,
                "app": t.app_name,
                "expiresAt": v["expiresAt"],
                "revealSecs": REVEAL_SECS,
            });
            match (v["link"].as_str(), v["code"].as_str()) {
                (Some(link), _) => for_you["link"] = json!(link),
                (None, Some(code)) => for_you["code"] = json!(code),
                (None, None) => return Applied::Refused { code: "failed".into(), message: "K2 made no link".into() },
            }
            super::super::answer::set_for_answerer(for_you);
            format!(
                "Set-password link for {u} made by {by} at {} (shown to {by} once; it works once, for 24 hours). {} never sees it.",
                now_hm(),
                ctx.workspace_handle
            )
        }
        "email" => {
            let r = crate::app_user_invites::handle_reset_link(json!({ "username": u, "app": t.app }).to_string().as_bytes(), &actor, true);
            if !r.status.starts_with("200") {
                return refused(&r, "failed");
            }
            let held = parsed(&r)["delivered"].as_str() == Some("held");
            // Never the address: the agent reads this line.
            if held {
                format!("A reset link for {u} waits to be sent to their address on file (allowed by {by} at {}).", now_hm())
            } else {
                format!("Reset link emailed to {u}'s address on file by {by} at {} (it works once, for 24 hours).", now_hm())
            }
        }
        other => return Applied::Refused { code: "usage".into(), message: format!("mode {other} is not one of {}", MODES.join(", ")) },
    };
    audit(&t, &mode, who, ctx);
    Applied::Done { message: applied }
}

pub(crate) fn actions() -> Vec<Registered> {
    vec![Registered { spec: &PASSWORD_RESET, check, apply }]
}

pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    Vec::new()
}

#[cfg(test)]
#[path = "password_reset_tests.rs"]
mod tests;
