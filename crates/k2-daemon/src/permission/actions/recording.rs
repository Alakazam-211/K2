//! `apps.recording app= [required=on|off] [server-read=on|off]` (Sew 0.9.0
//! recording in Apps, K2 PRD prd-k2-bobbin-in-apps-v1 K-c): an App's own
//! agent asks for its App to require recording in calls of two or more
//! people, or for its server to read the transcripts of recorded calls.
//!
//! - **Why a card:** `k2 app recording <app> set --required on` / `--server-read
//!   on` (`POST /cli/app/recording/set`) answers an agent `owner_only`; with
//!   cards on for its workspace that refusal names the App and what was
//!   asked, and ends with this card's Ask-with line.
//! - **Who asks:** only for an App (a published service) of the asking
//!   workspace; anything else is refused in the words used for a name that
//!   doesn't exist. Turning things off is the agent's own to do (no card).
//! - **Who answers:** **the owner only** (Rosson's answer 3): the owner token
//!   or an Owner login. An Admin's Allow is refused [`OWNER_IN_PERSON`] and
//!   the card stays open for the owner. Presented on the Thread of the
//!   workspace that maintains the App, as a Tickets item (B6).
//! - **Proof:** a fresh confirm bound to the request (`apps-recording`); at
//!   the click K2 turns it into the route's own (`app-recording-owner`,
//!   bound to the App) for the same person, so the route decides again.
//! - **Words:** the App, the Spaces it governs, and what its server reads;
//!   each person's consent notice names it, and every read is logged.

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::permission::catalog::{
    ActionSpec, Area, ConfirmAction, Direction, Expiry, Floor, Grant, NotCardableSpec, ParamFrom, ParamKind, ParamSpec, Proof,
    SettingsRef, Undo, Wall, WallCoverage, WallMatch, WallMethod, Words,
};
use k2_vault::HumanProof;
use serde_json::{json, Value};

use super::chat_cards::{own_app, OWNER_IN_PERSON};
use super::held::{error_of, route_floor, text_param};
use super::vault::OwnService;
use super::{Answerer, Applied, Check, Ctx, Registered};
use crate::compute_routes::Who;

pub(crate) const ACTION: &str = "apps.recording";
pub(crate) const ROUTE: &str = crate::apps::recording_routes::SET;
/// The card's confirm (`ConfirmAction::AppsRecording`), bound to the request.
pub(crate) const CONFIRM: &str = "apps-recording";
pub(crate) const ON_OFF: [&str; 2] = ["on", "off"];

pub(crate) static APPS_RECORDING: ActionSpec = ActionSpec {
    id: ACTION,
    area: Area::Apps,
    params: &[
        ParamSpec { key: "app", kind: ParamKind::App, required: true, list: false, choices: &[] },
        ParamSpec { key: "required", kind: ParamKind::Choice, required: false, list: false, choices: &ON_OFF },
        ParamSpec { key: "server-read", kind: ParamKind::Choice, required: false, list: false, choices: &ON_OFF },
    ],
    floor: Floor::Owner,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::SettingsConfirm { action: ConfirmAction::AppsRecording },
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "{app}'s recording",
        what_changes: "{what}",
        consequences: "{effects}",
        scope: "{app}'s chat Spaces",
        duration: "until someone turns it off",
        retry: "",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "sew", path: "k2 app recording <app> set --required off --server-read off (a terminal outside K2)" },
    walls: &[Wall {
        matches: WallMatch::Code { status: 403, code: "owner_only" },
        route_prefix: ROUTE,
        method: WallMethod::Post,
        params: &[
            ("app", ParamFrom::Response("/error/app")),
            ("required", ParamFrom::Response("/error/required")),
            ("server-read", ParamFrom::Response("/error/serverRead")),
        ],
    }],
    coverage: WallCoverage::Code,
    machine_hints: &[],
};

/// One request, checked: the App, and what it turns on.
pub(crate) struct Ask {
    pub svc: OwnService,
    pub required: Option<bool>,
    pub server_read: Option<bool>,
}

fn on_off(ctx: &Ctx, key: &str) -> Result<Option<bool>, String> {
    match text_param(ctx, key) {
        None => Ok(None),
        Some("on") => Ok(Some(true)),
        Some("off") => Ok(Some(false)),
        Some(other) => Err(format!("{key}={other}: on or off")),
    }
}

pub(crate) fn ask_of(ctx: &Ctx) -> Result<Ask, String> {
    let app = text_param(ctx, "app").ok_or("name your App: app=<its name>")?;
    let svc = own_app(&ctx.workspace_id, app).ok_or_else(|| format!("{} has no App named {app}", ctx.workspace_handle))?;
    let required = on_off(ctx, "required")?;
    let server_read = on_off(ctx, "server-read")?;
    if required.is_none() && server_read.is_none() {
        return Err("say what: required=on and/or server-read=on".into());
    }
    Ok(Ask { svc, required, server_read })
}

/// The App's recording view now (flags, settings, Spaces).
fn view(svc: &OwnService) -> Result<Value, String> {
    k2_core::tool_hook::recording_app_view(&svc.id)
}

fn words(ctx: &Ctx, out: &mut Ctx) {
    let ws = &ctx.workspace_handle;
    let (what, effects) = match ask_of(ctx) {
        Ok(a) => {
            let n = &a.svc.name;
            out.params.insert("app".into(), json!(n));
            let spaces = view(&a.svc).ok().and_then(|v| v["spaces"].as_array().map(Vec::len)).unwrap_or(0);
            let in_spaces = match spaces {
                1 => "its 1 Space".to_string(),
                n => format!("its {n} Spaces"),
            };
            let mut what = Vec::new();
            let mut effects = Vec::new();
            if a.server_read == Some(true) {
                what.push(format!("let {ws}'s App {n}'s server read the transcripts of recorded calls in {in_spaces}"));
                effects.push(format!(
                    "{n}'s server may read the transcripts of recorded calls in {in_spaces}. Each person's consent notice will say so. Every read is logged."
                ));
            }
            if a.required == Some(true) {
                what.push(format!("require recording of every call with two or more people in {n}'s {}", if spaces == 1 { "Space" } else { "Spaces" }));
                effects.push(
                    "A call there is recorded once a second person joins; each person still consents for themselves, and someone who declines can't join a call that requires it when the server's policy says so.".to_string(),
                );
            }
            effects.push(format!("To turn it off: k2 app recording {n} set --required off --server-read off."));
            (what.join(", and "), effects.join(" "))
        }
        Err(e) => (format!("Change the recording of one of {ws}'s Apps"), e),
    };
    out.params.insert("what".into(), json!(what));
    out.params.insert("effects".into(), json!(effects));
}

fn check(ctx: &Ctx) -> Check {
    let a = match ask_of(ctx) {
        Ok(a) => a,
        Err(e) => return Check::Moved(e),
    };
    let v = match view(&a.svc) {
        Ok(v) => v,
        Err(e) => return Check::Unavailable { code: "chat_unavailable", why: e },
    };
    if v["flags"]["appRecording"] != json!(true) {
        return Check::Unavailable {
            code: "sew_feature_missing",
            why: "this server's chat can't record in Apps yet (it needs Sew 0.9.0: app_recording): the owner upgrades it (k2 hostchat upgrade)".into(),
        };
    }
    let on = |k: &str| v["recording"][k] == json!(true);
    let wants = (a.required == Some(true) && !on("required")) || (a.server_read == Some(true) && !on("server_read"));
    if wants {
        Check::Needed
    } else {
        // Nothing to turn on: turning off is the agent's own to do.
        Check::AlreadyDone
    }
}

fn route_who(who: &Answerer) -> Who {
    match who {
        Answerer::OwnerToken => Who::Owner,
        Answerer::Login { user, role } => Who::Login { username: user.clone(), role: Role::parse_wire(role).unwrap_or(Role::Member) },
    }
}

fn apply(ctx: &Ctx, who: &Answerer, token: Option<&str>) -> Applied {
    let a = match ask_of(ctx) {
        Ok(a) => a,
        Err(e) => return Applied::Refused { code: "bad_params".into(), message: e },
    };
    if let Err(refused) = route_floor(ROUTE, who) {
        return refused;
    }
    // Rosson's answer 3: the owner only. An Admin's Allow leaves the card
    // open for the owner, before any confirm is spent.
    let owner = match who {
        Answerer::OwnerToken => true,
        Answerer::Login { role, .. } => matches!(Role::parse_wire(role), Ok(Role::Owner)),
    };
    if !owner {
        return Applied::Refused {
            code: OWNER_IN_PERSON.into(),
            message: "only the server's owner can allow an App to require recording or read transcripts; it stays open for them".into(),
        };
    }
    let Some(token) = token else {
        return Applied::Refused { code: "vault_confirm_required".into(), message: "confirm it's you first".into() };
    };
    let Some(request) = super::super::answer::current_request() else {
        return Applied::Refused { code: "failed".into(), message: "no request is being answered".into() };
    };
    let vault = match crate::vault::get() {
        Ok(v) => v,
        Err(e) => return Applied::Refused { code: e.code.into(), message: e.hint },
    };
    let method = match vault.confirm.take(token, CONFIRM, Some(&request), &who.stamp()) {
        Ok(HumanProof::Confirmed { method }) => method,
        Ok(HumanProof::None) => return Applied::Refused { code: "vault_confirm_required".into(), message: "confirm again".into() },
        Err(e) => return Applied::Refused { code: e.code.into(), message: e.hint },
    };
    let route_token = vault.confirm.mint(crate::apps::recording_routes::CONFIRM_OWNER, Some(&a.svc.id), &who.stamp(), &method);
    let mut body = json!({ "app": a.svc.id, "confirmToken": route_token });
    if let Some(b) = a.required {
        body["required"] = json!(b);
    }
    if let Some(b) = a.server_read {
        body["serverRead"] = json!(b);
    }
    let r = crate::apps::guest_rooms::handle(ROUTE, &route_who(who), true, body.to_string().as_bytes(), &HashMap::new());
    if !r.status.starts_with("200") {
        let v: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
        let (code, message) = error_of(&v, "recording_refused");
        return Applied::Refused { code, message };
    }
    let mut done = Vec::new();
    if a.required == Some(true) {
        done.push("requires recording when two or more people are in a call");
    }
    if a.server_read == Some(true) {
        done.push("lets its server read recorded calls' transcripts (named in every consent notice, every read logged)");
    }
    Applied::Done { message: format!("{} {} now.", a.svc.name, done.join(", and ")) }
}

/// The card's words K2 computes from the stored values.
pub(crate) fn with_computed(action: &str, ctx: &Ctx) -> Option<Ctx> {
    if action != ACTION {
        return None;
    }
    let mut out = ctx.clone();
    words(ctx, &mut out);
    Some(out)
}

/// The macOS sheet's words for the card's confirm, from the request's
/// stored values. `None`: not this confirm.
pub(crate) fn confirm_reason(action: &str, request: Option<&str>) -> Option<Result<String, String>> {
    if action != CONFIRM {
        return None;
    }
    let found = request.and_then(|id| crate::permission::store::get(id).ok().flatten()).filter(|r| r.action == ACTION && r.is_answerable());
    Some(match found {
        None => Err("that card isn't waiting for an answer".to_string()),
        Some(r) => {
            let ctx = crate::permission::present::ctx_of(&r);
            let ctx = with_computed(ACTION, &ctx).unwrap_or(ctx);
            Ok(crate::permission::present::fill("{what}", &ctx))
        }
    })
}

pub(crate) fn actions() -> Vec<Registered> {
    vec![Registered { spec: &APPS_RECORDING, check, apply }]
}

pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::super::held::tests::{answer_as, ask, ask_ok, card_of, id_of, login, row, seed, view, Ws};
    use super::*;
    use k2_core::tool_hook::{self as hook, CHAT_KIND};

    struct Env {
        _home: crate::test_support::TempHome,
        vault: std::sync::Arc<k2_vault::LocalVault>,
    }

    fn env() -> Env {
        let home = crate::test_support::TempHome::new();
        k2_core::db::init_for_tests();
        let vault = crate::vault::ensure_test_vault();
        hook::allow_target_off_linux_for_tests(true);
        hook::set_target(CHAT_KIND, true, Some("/tmp/k2-test-sew.sock"), Some("/tmp/k2-test-sew.token"), "owner-token").expect("chat target");
        Env { _home: home, vault }
    }

    fn app_of(ws: &Ws) -> (String, String) {
        let name = format!("jobs{}", &uuid::Uuid::new_v4().simple().to_string()[..6]);
        let db = k2_core::db::shared();
        let conn = db.lock();
        let id = k2_core::published_services::insert(&conn, &ws.id, &name, "node s.js", "/tmp", 47_611, "local", "stopped", "cmd", "").expect("App").id;
        (name, id)
    }

    /// B20: the card is Owner-floored, one time, with its own confirm, and
    /// valid against the catalog.
    #[test]
    fn the_card_is_the_owners_with_its_own_confirm() {
        let r = super::super::find(ACTION).expect("apps.recording");
        k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(APPS_RECORDING.floor, Floor::Owner);
        assert_eq!(APPS_RECORDING.grant, Grant::OneTime);
        assert_eq!(APPS_RECORDING.proof, Proof::SettingsConfirm { action: ConfirmAction::AppsRecording });
        assert_eq!(ConfirmAction::AppsRecording.as_str(), CONFIRM);
        assert_eq!(APPS_RECORDING.walls[0].route_prefix, "/cli/app/recording/set");
    }

    /// PRD §8 K-c: an Admin's Allow on `server-read=on` is refused
    /// `owner_in_person` and the card stays open, with nothing changed; the
    /// owner's Allow (a confirm bound to the request) runs the route as the
    /// owner and turns it on.
    #[test]
    fn an_admin_leaves_it_open_and_the_owner_turns_server_read_on() {
        let e = env();
        let ws = seed("rec");
        let (name, svc) = app_of(&ws);
        let (s, v) = ask(&ws, ACTION, json!({ "app": name, "server-read": "on" }));
        assert_eq!((s.as_str(), &v["error"]["code"]), ("409 Conflict", &json!("sew_feature_missing")), "Sew 0.8.x: {v}");
        hook::note_features_for_tests(&["app_recording", "voice_record_cap", "recording_required", "mgmt_recording_reads"]).expect("flags");
        let asked = ask_ok(&ws, ACTION, json!({ "app": name, "server-read": "on" }));
        let (id, card) = (id_of(&asked), card_of(&asked));
        let vw = view(&ws, &card);
        let cons = vw["consequences"].as_str().unwrap();
        assert!(cons.contains("may read the transcripts of recorded calls in its 0 Spaces") && cons.contains("Every read is logged"), "{cons}");
        let admin = login("reca", Role::Admin);
        let tok = e.vault.confirm.mint(CONFIRM, Some(&id), &format!("connect:{admin}"), "local-auth");
        let (s, a) = answer_as(&admin, &ws, &card, json!({ "answer": "Allow", "confirmToken": tok }));
        assert_eq!((s.as_str(), &a["error"]["code"]), ("403 Forbidden", &json!("action_card_person_only")), "the Owner floor: {a}");
        assert_eq!(a["error"]["required"], json!("owner"), "{a}");
        assert!(row(&id).is_answerable(), "still open for the owner");
        assert_eq!(hook::recording_app_view(&svc).unwrap()["recording"], json!({}), "nothing changed");
        let token = e.vault.confirm.mint(CONFIRM, Some(&id), "owner", "local-auth");
        let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow", "confirmToken": token }));
        assert_eq!(s, "200 OK", "{a}");
        assert_eq!(a["action"]["result"]["outcome"], "approved", "{a}");
        assert_eq!(hook::recording_app_view(&svc).unwrap()["recording"], json!({ "server_read": true }));
        let (s, again) = ask(&ws, ACTION, json!({ "app": name, "server-read": "on" }));
        assert_eq!((s.as_str(), &again["status"]), ("200 OK", &json!("already_done")), "{again}");
    }
}
