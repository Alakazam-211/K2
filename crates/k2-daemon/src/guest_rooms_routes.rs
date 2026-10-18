//! RS-A15 `guests:rooms`, the App transport (room-scope amendment
//! 2026-10-18, 0.46.1): the App-scoped twin of the owner-only
//! `POST /cli/skin/users/rooms`, so an App's server stops carrying the
//! owner token.
//!
//! ```text
//! GET  /cli/skin/guests/rooms ?username=<guest>[&app=<own id>]          guests.rooms
//! POST /cli/skin/guests/rooms {username, rooms: [handle|id…], app?}     guests.setRooms
//!   → 200 AppGuestRooms {ok, app, username, rooms, roomHandles, role, enabled, changed}
//! ```
//!
//! - **Only the App's own pass**: its gateway-link pass (inside the wall,
//!   `Authorization: Bearer` from `K2_PASS_FILE`) or its remote pass (an
//!   App hosted elsewhere, main port or tunnel). A guest's session pass, a
//!   platform pass, the owner token and a Connect login are refused
//!   (`app_pass_required`; logins already stop at the route floor).
//! - **Only with the owner's grant** (`k2 app grant <app> --kind
//!   guests-rooms`): without it `403 grant_required`.
//! - `app`, when sent, must be this App's own service id; anything else is
//!   the same `404 not_found` as a guest that isn't this App's.
//! - The rules (own guests, the grant's rooms plus the App's own, never
//!   above the role, idempotent, audited, 60/min) are
//!   `k2_core::skin::guest_rooms`.
//!
//! The catalog rows are `guests.rooms` / `guests.setRooms` (door `link`,
//! also `garden-local`: the Garden transport is `garden_data::guest_rooms`).

use std::collections::HashMap;

use k2_core::skin::guest_rooms::{self as gr, Ceiling, Refusal};
use k2_core::skin::SkinPass;
use serde_json::{json, Value as J};

use crate::cli_response::CliResponse;

pub const ROUTE: &str = "/cli/skin/guests/rooms";

fn status_line(code: u16) -> &'static str {
    match code {
        200 => "200 OK",
        400 => "400 Bad Request",
        401 => "401 Unauthorized",
        403 => "403 Forbidden",
        404 => "404 Not Found",
        409 => "409 Conflict",
        429 => "429 Too Many Requests",
        _ => "500 Internal Server Error",
    }
}

pub fn error(status: u16, code: &str, hint: impl Into<String>, extra: Option<J>) -> CliResponse {
    let mut e = json!({ "code": code, "hint": hint.into() });
    if let (Some(J::Object(x)), Some(o)) = (extra, e.as_object_mut()) {
        o.extend(x);
    }
    CliResponse { status: status_line(status), content_type: "application/json", body: json!({ "ok": false, "error": e }).to_string() }
}

/// A core refusal as the wire answer (shared with the CLI door).
pub fn refusal(r: &Refusal) -> CliResponse {
    let extra = match r {
        Refusal::RateLimited { retry_after_ms } => Some(json!({ "retryAfterMs": retry_after_ms, "limit": gr::RATE_PER_MIN })),
        Refusal::OutsideApp(rooms) | Refusal::AboveRole(rooms) => Some(json!({ "rooms": rooms })),
        _ => None,
    };
    error(r.status(), r.code(), r.hint(), extra)
}

pub fn app_pass_required() -> CliResponse {
    error(
        403,
        "app_pass_required",
        "guest rooms take this App's own pass (K2_PASS_FILE inside the wall, or its remote pass); never a guest's pass, the owner token or a login",
        None,
    )
}

pub fn pass_invalid() -> CliResponse {
    error(401, "pass_invalid", "the pass is expired or revoked: read K2_PASS_FILE again (K2 rotates it)", None)
}

/// The gateway link a request came in on (the dispatcher reads the
/// task-local link before the blocking handler runs: a blocking thread
/// doesn't see it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnLink {
    pub app_id: String,
    pub service_id: String,
}

impl OnLink {
    /// The link the current request arrived on (`None` off a link).
    pub fn current() -> Option<Self> {
        crate::apps::gateway::current().map(|l| OnLink { app_id: l.app_id.clone(), service_id: l.service_id.clone() })
    }
}

/// The App whose own pass `pass` is: on a link, the link pass's App (the
/// link resolver only returns this link's pass or a guest session of this
/// App); off a link, a remote pass's App (`skin::link::remote_pass_app`;
/// the general resolver leaves `app` empty for non-session passes). A
/// guest's session pass and a platform pass are `None`.
pub fn app_of(pass: &SkinPass, link: Option<&OnLink>) -> Option<String> {
    if pass.session || pass.principal_id.is_some() {
        return None;
    }
    if let Some(l) = link {
        return (pass.app == l.service_id && !l.service_id.is_empty()).then(|| l.service_id.clone());
    }
    k2_core::skin::link::remote_pass_app(&pass.id).filter(|a| !a.trim().is_empty() && a != k2_core::skin::HOST_APP_ID)
}

/// The audit actor: `app:<id>` on its link, `app-remote:<service id>` off it.
fn actor(app: &str, link: Option<&OnLink>) -> String {
    match link {
        Some(l) => format!("app:{}", l.app_id),
        None => format!("app-remote:{app}"),
    }
}

fn field<'a>(v: &'a J, k: &str) -> Option<&'a str> {
    v.get(k).and_then(J::as_str).map(str::trim).filter(|s| !s.is_empty())
}

/// `rooms` as a list (`[]` allowed); a comma string is accepted too.
pub fn rooms_of(v: &J) -> Option<Vec<String>> {
    match v.get("rooms")? {
        J::Array(a) => a.iter().map(|x| x.as_str().map(str::to_string)).collect(),
        J::String(s) => Some(s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()),
        _ => None,
    }
}

fn ok(view: &gr::GuestRooms) -> CliResponse {
    CliResponse::ok_json(serde_json::to_string(view).unwrap_or_else(|_| "{}".into()))
}

/// One call with an already-resolved pass (`None` = a `k2skn_` that didn't
/// resolve). Blocking: run on a blocking task.
pub fn handle(pass: Option<SkinPass>, link: Option<&OnLink>, is_post: bool, body: &[u8], params: &HashMap<String, String>) -> CliResponse {
    let Some(pass) = pass else { return pass_invalid() };
    let Some(app) = app_of(&pass, link) else { return app_pass_required() };
    let v: J = if is_post {
        match serde_json::from_slice::<J>(body) {
            Ok(v @ J::Object(_)) => v,
            _ => return error(400, "usage", "the body is a JSON object {username, rooms, app?}", None),
        }
    } else {
        json!(params)
    };
    // Naming another App is the same answer as a guest that isn't ours.
    if field(&v, "app").is_some_and(|a| a != app) {
        return refusal(&Refusal::NotFound);
    }
    let ceiling = match gr::app_ceiling(&app) {
        Ok(Some(rooms)) => Ceiling::Rooms(rooms),
        Ok(None) => {
            return error(
                403,
                "grant_required",
                "the owner hasn't granted this App guests:rooms (k2 app grant <app> --kind guests-rooms)",
                None,
            )
        }
        Err(e) => return refusal(&Refusal::Store(e)),
    };
    let Some(username) = field(&v, "username") else {
        return error(400, "usage", "username is required", None);
    };
    let r = if is_post {
        let Some(rooms) = rooms_of(&v) else {
            return error(400, "usage", "rooms is required: a list of room handles or ids ([] takes every room away)", None);
        };
        gr::set(&app, username, &rooms, &ceiling, &actor(&app, link), "app").map(|o| o.view)
    } else {
        gr::read(&app, username)
    };
    match r {
        Ok(view) => ok(&view),
        Err(e) => refusal(&e),
    }
}

#[cfg(test)]
#[path = "guest_rooms_routes_tests.rs"]
mod tests;
