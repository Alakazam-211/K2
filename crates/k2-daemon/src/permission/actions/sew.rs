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
    settings: SettingsRef { page: "sew", path: "Settings → Cores → Chat Hosting → Open Sew as owner" },
    walls: &[Wall {
        matches: WallMatch::Code { status: 403, code: "owner_only" },
        route_prefix: HANDOFF_ROUTE,
        method: WallMethod::Post,
        params: &[],
    }],
    coverage: WallCoverage::PrefixOnly,
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
            why: format!("\"Manage Sew\" is off for {}: only an agent that manages Sew may ask", ctx.workspace_handle),
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

pub(crate) fn actions() -> Vec<Registered> {
    vec![Registered { spec: &OWNER_HANDOFF, check, apply }]
}

pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    Vec::new()
}

#[cfg(test)]
#[path = "sew_tests.rs"]
mod tests;
