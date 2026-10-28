//! Sew for Apps A2 (0.46.6): `guests:chat`, the App transport. An App
//! gives or takes back chat for its own guests with its own pass, under the
//! owner's `guests-chat` grant, so it never carries the owner token.
//!
//! ```text
//! GET  /cli/skin/guests/chat ?username=<guest>[&app=<own id>]                guests.chat
//! POST /cli/skin/guests/chat {username, scope: member|guest|none, app?}      guests.setChat
//!   → 200 AppGuestChat {ok, app, username, principalId, scope, enabled, managedBy, members, maxMembers, changed}
//! ```
//!
//! - **Only the App's own pass** (its link pass inside the wall, or its
//!   remote pass): a guest's session pass, a platform pass, the owner token
//!   and a login get `app_pass_required` (the same door rule as
//!   `guests.rooms`, [`crate::guest_rooms_routes::app_of`]).
//! - **Only with the owner's grant** (`k2 app grant <app> --kind
//!   guests-chat [--max N]`): without it `403 grant_required`.
//! - `app`, when sent, must be this App's own id; anything else is the same
//!   `404 not_found` as a guest that isn't this App's.
//! - The rules (own guests, own grants, the member ceiling, never a shared
//!   person, audited, 60/min) are `k2_core::skin::app_chat`.
//!
//! What a set writes is the person's `chat` grant: S27 provisions their
//! Sew member (`k2:principal:<id>`) through the outbox and K31 adds them to
//! the App's bound Spaces. A chat pass minted before the provision lands
//! gets Sew's `not_provisioned`; retry after a moment.

use std::collections::HashMap;

use k2_core::skin::app_chat::{self as ac, Caller, Refusal, Scope};
use k2_core::skin::SkinPass;
use serde_json::{json, Value as J};

use crate::cli_response::CliResponse;
use crate::guest_rooms_routes::{app_of, error, pass_invalid, OnLink};

pub const ROUTE: &str = "/cli/skin/guests/chat";

/// A core refusal as the wire answer (shared with the CLI door).
pub fn refusal(r: &Refusal) -> CliResponse {
    let extra = match r {
        Refusal::RateLimited { retry_after_ms } => Some(json!({ "retryAfterMs": retry_after_ms, "limit": ac::RATE_PER_MIN })),
        Refusal::Limit { max } => Some(json!({ "maxMembers": max })),
        _ => None,
    };
    error(r.status(), r.code(), r.hint(), extra)
}

pub fn app_pass_required() -> CliResponse {
    error(
        403,
        "app_pass_required",
        "guest chat takes this App's own pass (K2_PASS_FILE inside the wall, or its remote pass); never a guest's pass, the owner token or a login",
        None,
    )
}

pub fn grant_required() -> CliResponse {
    error(
        403,
        "grant_required",
        "the owner hasn't granted this App guests:chat (k2 app grant <app> --kind guests-chat [--max N])",
        None,
    )
}

fn field<'a>(v: &'a J, k: &str) -> Option<&'a str> {
    v.get(k).and_then(J::as_str).map(str::trim).filter(|s| !s.is_empty())
}

/// The audit actor: `app:<id>` on its link, `app-remote:<service id>` off it.
fn actor(app: &str, link: Option<&OnLink>) -> String {
    match link {
        Some(l) => format!("app:{}", l.app_id),
        None => format!("app-remote:{app}"),
    }
}

/// One call with an already-resolved pass (`None` = a `k2skn_` that didn't
/// resolve). Blocking: run on a blocking task.
pub fn handle(pass: Option<SkinPass>, link: Option<&OnLink>, is_post: bool, body: &[u8], params: &HashMap<String, String>) -> CliResponse {
    let Some(pass) = pass else { return pass_invalid() };
    let Some(app) = app_of(&pass, link) else { return app_pass_required() };
    let v: J = if is_post {
        match serde_json::from_slice::<J>(body) {
            Ok(v @ J::Object(_)) => v,
            _ => return error(400, "usage", "the body is a JSON object {username, scope, app?}", None),
        }
    } else {
        json!(params)
    };
    if field(&v, "app").is_some_and(|a| a != app) {
        return refusal(&Refusal::NotFound);
    }
    match ac::get_grant(&app) {
        Ok(Some(_)) => {}
        Ok(None) => return grant_required(),
        Err(e) => return refusal(&Refusal::Store(e)),
    }
    let Some(username) = field(&v, "username") else {
        return error(400, "usage", "username is required", None);
    };
    let r = if is_post {
        let Some(scope) = field(&v, "scope").and_then(Scope::parse) else {
            return error(400, "usage", "scope is member, guest or none", None);
        };
        ac::set(&Caller::App, Some(&app), username, scope, &actor(&app, link), "app")
    } else {
        ac::read(&app, username)
    };
    match r {
        Ok(view) => CliResponse::ok_json(serde_json::to_string(&view).unwrap_or_else(|_| "{}".into())),
        Err(e) => refusal(&e),
    }
}

#[cfg(test)]
#[path = "guest_chat_routes_tests.rs"]
mod tests;
