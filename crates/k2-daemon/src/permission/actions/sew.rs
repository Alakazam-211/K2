//! `sew.owner-handoff` (Rosson 2026-10-28): a "Manage Sew" agent asks
//! for Sew's web app to be opened as its install owner, and the owner
//! answers on a permission card.
//!
//! - **Who asks:** an agent whose workspace has "Manage Sew"
//!   (`chat_manage`). Agents never become Sew's install owner: the
//!   `/cli/sew/owner-handoff` route refuses every passport (`owner_only`);
//!   that refusal is this card's wall, so the agent's error ends with its
//!   "Ask with" line.
//! - **Who answers:** the Owner only (the floor), with the same confirm the
//!   Settings path takes (`chat-owner-handoff`: Touch ID, the vault
//!   password or the Connect password), bound to this request.
//! - **Only from the box or its LAN** (parity with Settings): an answer
//!   through the tunnel (Connect, a phone) is refused with [`LOCAL_ONLY`]
//!   and the request stays open; the dispatcher tags each answer's ingress.
//! - **On Allow:** K2 mints the one-time owner handoff (60 s, single use,
//!   audited `chat-handoff`) **for the person who allowed it**: the link
//!   goes back in that person's own answer (`forYou.openUrl`), which their
//!   client opens. The card and the agent's turn say only that it was
//!   opened for the owner; the link, the code and the app URL with its code
//!   never reach the agent.
//! - **The Settings path stays:** Settings → Cores → Chat Hosting → Open
//!   Sew as owner, and `k2 hostchat open`, unchanged.
//! - Sew HC2: promoting someone to install owner or admin lifts their Sew
//!   bans; the card says so.

use k2_core::permission::catalog::{
    ActionSpec, Area, ConfirmAction, Direction, Expiry, Floor, Grant, NotCardableSpec, Proof, SettingsRef, Undo, Wall,
    WallCoverage, WallMatch, WallMethod, Words, MAX_EXPIRY_SECS,
};
use serde_json::json;

use super::{Answerer, Applied, Check, Ctx, Registered};
use crate::vault::actor::{Channel, VaultCaller};

/// The card's route wall: an agent's own call to the owner handoff.
pub(crate) const HANDOFF_ROUTE: &str = crate::chat_pass::mint::OWNER_HANDOFF;
/// Why a tunnel answer (Connect, a phone) doesn't open Sew.
pub(crate) const LOCAL_ONLY: &str = "Open this from K2 on the box's own computer (or Settings → Chat / Voice Hosting there): Sew opens as its owner only from the box or its LAN, never through the tunnel";

/// The one-time link lives this long (the handoff code's TTL).
pub(crate) const LINK_TTL_SECS: i64 = crate::chat_pass::handoff::TTL_SECS;

pub(crate) static OWNER_HANDOFF: ActionSpec = ActionSpec {
    id: "sew.owner-handoff",
    area: Area::Sew,
    params: &[],
    floor: Floor::Owner,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::SettingsConfirm { action: ConfirmAction::ChatOwnerHandoff },
    expiry: Expiry { default_secs: 24 * 3600, max_secs: MAX_EXPIRY_SECS },
    words: Words {
        setting: "Open Sew as owner",
        what_changes: "Open Sew's web app as its install owner, for you ({ws} asked)",
        consequences: "K2 opens a one-time link for you only (it works once, for 60 seconds); {ws} never gets it and never becomes Sew's owner. In Sew, promoting someone to install owner or admin lifts their Sew bans.",
        scope: "you, once",
        duration: "once",
        retry: "",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "sew", path: "Settings → Cores → Chat / Voice Hosting → Open Sew as owner" },
    walls: &[Wall {
        matches: WallMatch::Code { status: 403, code: "owner_only" },
        route_prefix: HANDOFF_ROUTE,
        method: WallMethod::Post,
        params: &[],
    }],
    coverage: WallCoverage::PrefixOnly,
    machine_hints: &[],
};

/// The card's route wall: an agent's call to the emergency rollback.
pub(crate) const ROLLBACK_ROUTE: &str = "/cli/sew/server/rollback";

/// `sew.rollback-data-loss` (Rosson 2026-10-28): a "Manage Sew" agent asks
/// for Sew to be rolled back across a schema change (after an upgrade that
/// changed Sew's data, e.g. 0.3.1 → 0.5.0). Only the Owner answers, with a
/// confirm bound to this request. On Allow K2 runs the same steps as
/// `k2 hostchat rollback --accept-data-loss`: a backup of today's chat
/// first, then Sew's rollback to the pre-upgrade backup, then K2's checks.
/// `{to}`, `{since}` and `{from}` are K2's own values ([`with_computed`]),
/// never the agent's.
pub(crate) static ROLLBACK_DATA_LOSS: ActionSpec = ActionSpec {
    id: "sew.rollback-data-loss",
    area: Area::Sew,
    params: &[],
    floor: Floor::Owner,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::SettingsConfirm { action: ConfirmAction::SewRollbackDataLoss },
    expiry: Expiry { default_secs: 24 * 3600, max_secs: MAX_EXPIRY_SECS },
    words: Words {
        setting: "Roll Sew back",
        what_changes: "Roll this server's chat (Sew) back from {from} to {to}",
        consequences: "Chat written since {since} will be lost from the live chat; a backup of it is kept. Approving runs \
            it now: K2 backs up the chat on Sew {from}, then everyone's chat goes back to how it was at {since}. Chat is offline \
            for a few minutes while it runs. To bring the newer chat back later: k2 hostchat upgrade, then k2 hostchat \
            restore <the kept backup> --confirm.",
        scope: "this server's chat, for everyone",
        duration: "once",
        retry: "",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "sew", path: "k2 hostchat rollback --accept-data-loss (a terminal outside K2)" },
    walls: &[Wall {
        matches: WallMatch::Code { status: 403, code: crate::sew::routes::ROLLBACK_NEEDS_PERSON },
        route_prefix: ROLLBACK_ROUTE,
        method: WallMethod::Post,
        params: &[],
    }],
    coverage: WallCoverage::Code,
    machine_hints: &[],
};

#[cfg(test)]
pub(crate) mod test_seams {
    use std::cell::RefCell;

    thread_local! {
        /// `Some(url)`: Sew's app URL for this thread (no Sew install).
        pub(crate) static APP_URL: RefCell<Option<String>> = const { RefCell::new(None) };
        /// The one confirm a test minted: `(token, secret, stamp)`.
        pub(crate) static CONFIRM: RefCell<Option<(String, String, String)>> = const { RefCell::new(None) };
        /// `Some(plan)`: the rollback plan for this thread (no Sew install).
        pub(crate) static PLAN: RefCell<Option<Result<serde_json::Value, String>>> = const { RefCell::new(None) };
        /// The bundles a test's Allow rolled back to (instead of running it).
        pub(crate) static RAN: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }
}

/// Where the emergency rollback would go (`crate::sew::supervisor::rollback_plan`).
fn rollback_plan() -> Result<serde_json::Value, String> {
    #[cfg(test)]
    if let Some(p) = test_seams::PLAN.with(|c| c.borrow().clone()) {
        return p;
    }
    crate::sew::supervisor::rollback_plan(&crate::sew::supervisor::live()).map_err(|f| f.hint)
}

/// The words hook (`permission::present`): K2's `{from}`, `{to}` and
/// `{since}` for this card (the rollback plan now), never the agent's.
pub(crate) fn with_computed(action: &str, ctx: &Ctx) -> Option<Ctx> {
    if action != ROLLBACK_DATA_LOSS.id {
        return None;
    }
    let plan = rollback_plan().ok();
    let get = |k: &str, dflt: &str| plan.as_ref().and_then(|p| p[k].as_str()).unwrap_or(dflt).to_string();
    let mut out = ctx.clone();
    out.params.insert("from".into(), json!(get("from", "the current version")));
    out.params.insert("to".into(), json!(get("to", "the previous version")));
    out.params.insert("since".into(), json!(get("since", "the last Sew upgrade")));
    Some(out)
}

/// Still wanted: the asking workspace has "Manage Sew" and there is a
/// rollback point.
fn check_rollback(ctx: &Ctx) -> Check {
    if !k2_core::workspace::settings::agents_can_manage_chat(&ctx.workspace_id) {
        return Check::Unavailable {
            code: "chat_manage_off",
            why: format!("\"Manage Chat / Voice Hosting\" is off for {}: only an agent that manages it may ask", ctx.workspace_handle),
        };
    }
    match rollback_plan() {
        Ok(_) => Check::Needed,
        Err(why) => Check::Moved(why),
    }
}

/// Take the card's confirm (`sew-rollback-data-loss`, bound to this
/// request) as the person who answered.
fn take_rollback(token: &str, secret: &str, stamp: &str) -> Result<String, k2_vault::VaultError> {
    #[cfg(test)]
    {
        let ok = test_seams::CONFIRM.with(|c| c.borrow_mut().take()).is_some_and(|(t, s, st)| t == token && s == secret && st == stamp);
        return if ok { Ok("local-auth".into()) } else { Err(k2_vault::VaultError::forbidden("vault_confirm_required", "confirm again")) };
    }
    #[cfg(not(test))]
    {
        let v = crate::vault::get()?;
        match v.confirm.take(token, crate::sew::supervisor::CONFIRM_ROLLBACK_DATA_LOSS, Some(secret), stamp)? {
            k2_vault::HumanProof::Confirmed { method } => Ok(method),
            k2_vault::HumanProof::None => Err(k2_vault::VaultError::forbidden("vault_confirm_required", "confirm again")),
        }
    }
}

/// Start the rollback (it takes minutes: a backup, Sew's rollback, K2's
/// checks); `k2 hostchat status` shows how it ended.
fn start_rollback(bundle: &str, who: &str) {
    #[cfg(test)]
    {
        let _ = who;
        test_seams::RAN.with(|c| c.borrow_mut().push(bundle.to_string()));
    }
    #[cfg(not(test))]
    {
        let (bundle, who) = (bundle.to_string(), who.to_string());
        let spawned = std::thread::Builder::new().name("sew-rollback".into()).spawn(move || {
            let env = crate::sew::supervisor::live();
            let req = crate::sew::supervisor::RollbackRequest { media_tarball_path: None };
            match crate::sew::supervisor::rollback_accept_data_loss(&env, &req, &bundle) {
                Ok(r) => k2_core::log_debug!("[sew] card rollback by {who}: {}", r["result"]),
                Err(f) => k2_core::log_debug!("[sew] card rollback by {who} refused: {} {}", f.code, f.hint),
            }
        });
        if let Err(e) = spawned {
            k2_core::log_debug!("[sew] card rollback: could not start: {e}");
        }
    }
}

/// On Allow: the confirm for this request, then the same steps as
/// `k2 hostchat rollback --accept-data-loss`, as the owner who answered.
fn apply_rollback(_ctx: &Ctx, who: &Answerer, token: Option<&str>) -> Applied {
    let Some(token) = token else {
        return Applied::Refused { code: "vault_confirm_required".into(), message: "confirm it's you first".into() };
    };
    let Some(secret) = super::super::answer::current_request() else {
        return Applied::Refused { code: "failed".into(), message: "no request is being answered".into() };
    };
    let stamp = who.stamp();
    if let Err(e) = take_rollback(token, &secret, &stamp) {
        return Applied::Refused { code: e.code.to_string(), message: e.hint };
    }
    let plan = match rollback_plan() {
        Ok(p) => p,
        Err(why) => return Applied::Refused { code: "no_rollback_point".into(), message: why },
    };
    let Some(bundle) = plan["bundle"].as_str().filter(|b| k2_root_proto::sew::bundle_id_ok(b)) else {
        return Applied::Refused { code: "no_rollback_point".into(), message: "K2 has no rollback point for Sew".into() };
    };
    crate::vault::auth_audit(crate::sew::supervisor::CONFIRM_ROLLBACK_DATA_LOSS, &stamp, &format!("card confirmed bundle={bundle}"), "card");
    start_rollback(bundle, &stamp);
    Applied::Done {
        message: format!(
            "Sew is rolling back to {} (chat written since {} leaves the live chat; K2 backs it up first). k2 hostchat status \
             shows when it's done.",
            plan["to"].as_str().unwrap_or("the previous version"),
            plan["since"].as_str().unwrap_or("the last upgrade")
        ),
    }
}

/// Sew's web app URL on this box (Settings → Chat Hosting → app URL; `k2
/// hostchat app-url set`), for a Sew that is installed in K2 mode.
fn app_url() -> Option<String> {
    #[cfg(test)]
    if let Some(u) = test_seams::APP_URL.with(|c| c.borrow().clone()) {
        return Some(u);
    }
    let row = crate::sew::store::load()?;
    if matches!(row.status.as_str(), "removed" | "disabled") || crate::sew::supervisor::row_provider(&row) != k2_root_proto::sew::PROVIDER_K2 {
        return None;
    }
    row.app_url.filter(|u| !u.trim().is_empty())
}

/// Still wanted: the asking workspace has "Manage Sew" and Sew has a web
/// app to open.
fn check(ctx: &Ctx) -> Check {
    if !k2_core::workspace::settings::agents_can_manage_chat(&ctx.workspace_id) {
        return Check::Unavailable {
            code: "chat_manage_off",
            why: format!("\"Manage Chat / Voice Hosting\" is off for {}: only an agent that manages it may ask", ctx.workspace_handle),
        };
    }
    if app_url().is_none() {
        return Check::Moved("Sew has no web app URL to open yet (k2 hostchat app-url set https://HOST)".into());
    }
    Check::Needed
}

fn caller_of(who: &Answerer) -> VaultCaller {
    use k2_vault::Actor;
    match who {
        Answerer::OwnerToken => VaultCaller { actor: Actor::Owner, channel: Channel::Plain, session_id: None },
        Answerer::Login { user, role } => VaultCaller {
            actor: Actor::Login {
                username: user.clone(),
                role: match role.as_str() {
                    "owner" => k2_vault::Role::Owner,
                    "admin" => k2_vault::Role::Admin,
                    _ => k2_vault::Role::Member,
                },
            },
            channel: Channel::Login,
            session_id: None,
        },
    }
}

/// The confirm, taken for this request (`secretId` = its id), as the
/// person who answered.
fn take(token: &str, secret: &str, stamp: &str) -> Result<String, k2_vault::VaultError> {
    #[cfg(test)]
    {
        let ok = test_seams::CONFIRM.with(|c| c.borrow_mut().take()).is_some_and(|(t, s, st)| t == token && s == secret && st == stamp);
        return if ok { Ok("local-auth".into()) } else { Err(k2_vault::VaultError::forbidden("vault_confirm_required", "confirm again")) };
    }
    #[cfg(not(test))]
    {
        let v = crate::vault::get()?;
        match v.confirm.take(token, crate::chat_pass::handoff::CONFIRM_ACTION, Some(secret), stamp)? {
            k2_vault::HumanProof::Confirmed { method } => Ok(method),
            k2_vault::HumanProof::None => Err(k2_vault::VaultError::forbidden("vault_confirm_required", "confirm again")),
        }
    }
}

/// Mint the owner handoff through the Settings route's own entry, as the
/// answerer, with the confirm they took; the link goes to them only.
fn apply(ctx: &Ctx, who: &Answerer, token: Option<&str>) -> Applied {
    // Parity with Settings (`POST /cli/sew/owner-handoff` refuses the
    // tunnel): only an answer from the box itself or its LAN mints. A
    // phone or a Connect session through the tunnel is refused and the
    // request stays open.
    if !matches!(super::super::answer::answer_ingress().as_deref(), Some("loopback" | "lan")) {
        return Applied::Refused { code: super::super::answer::LOOPBACK_ONLY.into(), message: LOCAL_ONLY.into() };
    }
    let Some(url) = app_url() else {
        return Applied::Refused { code: "no_app_url".into(), message: "Sew has no web app URL to open (k2 hostchat app-url set https://HOST)".into() };
    };
    let Some(token) = token else {
        return Applied::Refused { code: "vault_confirm_required".into(), message: "confirm it's you first".into() };
    };
    let Some(secret) = super::super::answer::current_request() else {
        return Applied::Refused { code: "failed".into(), message: "no request is being answered".into() };
    };
    if crate::sew::health::GATE_HANDOFF {
        if let Err(details) = crate::sew::health::mint_gate(&crate::sew::supervisor::live()) {
            let hint = details["hint"].as_str().unwrap_or("Sew is not ready").to_string();
            return Applied::Refused { code: "sew_unavailable".into(), message: hint };
        }
    }
    let take_for = |t: &str, stamp: &str| take(t, &secret, stamp);
    let body = json!({ "confirmToken": token }).to_string();
    let reply = crate::chat_pass::mint::handle_owner_handoff_with(
        "permission-card",
        Ok(caller_of(who)),
        body.as_bytes(),
        crate::chat_pass::keyring::now(),
        &take_for,
        &crate::chat_pass::ratelimit::HANDOFFS,
        &crate::chat_pass::handoff::CODES,
    );
    let v: serde_json::Value = serde_json::from_str(&reply.response.body).unwrap_or_default();
    let Some(code) = v["code"].as_str().filter(|_| reply.response.status.starts_with("200")) else {
        let c = v["error"]["code"].as_str().unwrap_or("failed").to_string();
        let hint = v["error"]["hint"].as_str().unwrap_or("Sew didn't open").to_string();
        return Applied::Refused { code: c, message: hint };
    };
    let open = format!("{}/k2/owner-handoff#code={code}", url.trim_end_matches('/'));
    super::super::answer::set_for_answerer(json!({
        "kind": "sew.owner-handoff",
        "openUrl": open,
        "expiresInMs": LINK_TTL_SECS * 1000,
    }));
    Applied::Done {
        message: format!(
            "Sew opened for the owner who allowed it (a one-time link, only theirs; {} never gets it).",
            ctx.workspace_handle
        ),
    }
}

// ── sew.recording-enable (0.46.11 K-a) ──────────────────────────────────
//
// A "Manage Sew" agent asks for Sew's recorder to be installed and
// recording turned on. `POST /cli/sew/recording/enable` answers every
// passport `person_only` (it never installs anything itself); that refusal
// is this card's wall. An Owner or an Admin answers (the route's floor), with
// the same confirm the CLI takes, bound to this request; at the click K2
// turns it into the route's own (bound to the Sew install) for the same
// person, and the route's own entry runs as them.

/// The card's route wall.
pub(crate) const RECORDING_ROUTE: &str = crate::sew::recording::ENABLE_ROUTE;

pub(crate) static RECORDING_ENABLE: ActionSpec = ActionSpec {
    id: crate::sew::recording::CARD,
    area: Area::Sew,
    params: &[],
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::SettingsConfirm { action: ConfirmAction::SewRecordingEnable },
    expiry: Expiry { default_secs: 24 * 3600, max_secs: MAX_EXPIRY_SECS },
    words: Words {
        setting: "Install Sew's recorder",
        what_changes: "Install Sew's recorder and transcriber on this server and turn recording on ({ws} asked)",
        consequences: "Calls can then be recorded and transcribed where an App or a room allows it, and only for people who \
            consent; nobody is recorded without saying yes. It needs 8 GiB of RAM and some disk under /var/lib. Turning it \
            off later (k2 hostchat recording disable) keeps the recorder and every recording; uninstalling it never deletes \
            recordings.",
        scope: "this server's chat, for everyone",
        duration: "until someone turns it off",
        retry: "",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "sew", path: "k2 hostchat recording enable (a terminal outside K2)" },
    walls: &[Wall {
        matches: WallMatch::Code { status: 403, code: "person_only" },
        route_prefix: RECORDING_ROUTE,
        method: WallMethod::Post,
        params: &[],
    }],
    coverage: WallCoverage::Code,
    machine_hints: &[],
};

/// Still wanted: the asking workspace has "Manage Sew", this server's Sew
/// can record (`app_recording`), and recording isn't on yet.
fn check_recording(ctx: &Ctx) -> Check {
    if !k2_core::workspace::settings::agents_can_manage_chat(&ctx.workspace_id) {
        return Check::Unavailable {
            code: "chat_manage_off",
            why: format!("\"Manage Chat / Voice Hosting\" is off for {}: only an agent that manages it may ask", ctx.workspace_handle),
        };
    }
    let env = crate::sew::supervisor::live();
    if !crate::sew::recording::feature_on(&env) {
        return Check::Unavailable {
            code: "sew_feature_missing",
            why: "this server's Sew can't record yet (it needs Sew 0.9.0: app_recording)".into(),
        };
    }
    let st = crate::sew::recording::load(&env);
    if st.installed && st.enabled {
        return Check::AlreadyDone;
    }
    Check::Needed
}

/// On Allow: the card's confirm for this request, turned into the route's
/// own for the same person, then the route as them (Owner or Admin).
fn apply_recording(_ctx: &Ctx, who: &Answerer, token: Option<&str>) -> Applied {
    if let Err(refused) = super::held::route_floor(RECORDING_ROUTE, who) {
        return refused;
    }
    let Some(token) = token else {
        return Applied::Refused { code: "vault_confirm_required".into(), message: "confirm it's you first".into() };
    };
    let Some(request) = super::super::answer::current_request() else {
        return Applied::Refused { code: "failed".into(), message: "no request is being answered".into() };
    };
    let stamp = who.stamp();
    let vault = match crate::vault::get() {
        Ok(v) => v,
        Err(e) => return Applied::Refused { code: e.code.into(), message: e.hint },
    };
    let method = match vault.confirm.take(token, crate::sew::recording::CONFIRM_ENABLE, Some(&request), &stamp) {
        Ok(k2_vault::HumanProof::Confirmed { method }) => method,
        Ok(k2_vault::HumanProof::None) => return Applied::Refused { code: "vault_confirm_required".into(), message: "confirm again".into() },
        Err(e) => return Applied::Refused { code: e.code.into(), message: e.hint },
    };
    let route_token = vault.confirm.mint(crate::sew::recording::CONFIRM_ENABLE, Some(&crate::sew::routes::confirm_secret_id()), &stamp, &method);
    let body = json!({ "confirmToken": route_token });
    let route_who = match who {
        Answerer::OwnerToken => crate::compute_routes::Who::Owner,
        Answerer::Login { user, role } => crate::compute_routes::Who::Login {
            username: user.clone(),
            role: k2_core::connect_users::Role::parse_wire(role).unwrap_or(k2_core::connect_users::Role::Member),
        },
    };
    let r = crate::sew::routes::handle(RECORDING_ROUTE, route_who, body.to_string().as_bytes(), &std::collections::HashMap::new());
    if !r.status.starts_with("200") {
        let v: serde_json::Value = serde_json::from_str(&r.body).unwrap_or_default();
        let (code, message) = super::held::error_of(&v, "recording_refused");
        return Applied::Refused { code, message };
    }
    Applied::Done {
        message: "Sew's recorder is installing; recording turns on when it's done (k2 hostchat recording status). Each person \
                  still consents for themselves."
            .into(),
    }
}

pub(crate) fn actions() -> Vec<Registered> {
    vec![
        Registered { spec: &OWNER_HANDOFF, check, apply },
        Registered { spec: &ROLLBACK_DATA_LOSS, check: check_rollback, apply: apply_rollback },
        Registered { spec: &RECORDING_ENABLE, check: check_recording, apply: apply_recording },
    ]
}

pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    Vec::new()
}

#[cfg(test)]
#[path = "sew_tests.rs"]
mod tests;
