//! The send API (prd-custom-app-push-v1 §8, CP24–CP33), one rule set for
//! every door:
//!
//! | Caller | Door | Needs |
//! |---|---|---|
//! | the App's backend in the wall | `POST /cli/skin/push/send` on its link, its link pass | `push:send` approved for the App |
//! | the App's backend outside it | `POST /v1/apps/<own id>/push/send`, its App key | key scope `push` |
//! | the App's agent | `POST /cli/app/push/send` (`k2 app push send`) | its own App, switch 2 |
//! | the owner, an Admin | the same route | Admin floor |
//! | a guest | never | |
//!
//! The App always comes from the credential, never the body (CP24, CP40);
//! recipients are filtered to the App's own guests holding a live, enabled
//! `kind=app` grant at send time; a guest of another App is
//! `skipped.not_a_guest`, the same answer as an unknown name. Sends are
//! queued on the bounded worker and answered `202`.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use k2_core::skin::{self, app_push as store};
use serde_json::{json, Value as J};

use super::budget::Limiter;
use super::payload::{self, Notification, Refused, Stamps};
use super::sendlog::{self, SendRecord, Skipped};
use super::sender::Target;
use super::worker::{self, Job};

/// Recipients one call may name or resolve (CP25, CP33).
pub const MAX_RECIPIENTS: usize = 1_000;
/// `push.send` calls per App an hour (CP33).
pub const SENDS_PER_HOUR: usize = 600;
/// Alert pushes per guest per App an hour (Thread bucket parity).
pub const ALERTS_PER_GUEST_HOUR: usize = 30;
/// Silent pushes per guest per App an hour.
pub const SILENT_PER_GUEST_HOUR: usize = 60;
/// Device sends per App an hour (box protection).
pub const DEVICE_SENDS_PER_HOUR: usize = 20_000;

/// Who sends (the audit actor and the `k2Kind`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    /// `app:<id>` | `app-key:<id>` | `agent:<handle>` | `owner` | `user:<name>` | `k2:agent-reply`.
    pub actor: String,
    pub kind: &'static str,
}

/// Whom a send targets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum To {
    Guest(String),
    Guests(Vec<String>),
    Room(String),
}

impl To {
    fn kind(&self) -> &'static str {
        match self {
            To::Guest(_) => "guest",
            To::Guests(_) => "guests",
            To::Room(_) => "room",
        }
    }
}

/// A refusal (status, code, hint, extra fields).
#[derive(Debug, Clone, PartialEq)]
pub struct SendErr {
    pub status: u16,
    pub code: &'static str,
    pub hint: String,
    pub extra: serde_json::Map<String, J>,
}

fn err(status: u16, code: &'static str, hint: impl Into<String>) -> SendErr {
    SendErr { status, code, hint: hint.into(), extra: serde_json::Map::new() }
}

fn err_with(status: u16, code: &'static str, hint: impl Into<String>, extra: J) -> SendErr {
    SendErr { status, code, hint: hint.into(), extra: extra.as_object().cloned().unwrap_or_default() }
}

impl From<Refused> for SendErr {
    fn from(r: Refused) -> Self {
        match r {
            Refused::Usage(m) => err(400, "usage", m),
            Refused::TooLarge { limit } => err_with(400, "too_large", "the notification is larger than Apple's and Google's 4,096 bytes", json!({ "limit": limit })),
        }
    }
}

/// `to`: `{guest}` | `{guests: [...]}` | `{room}`.
pub fn parse_to(v: &J) -> Result<To, SendErr> {
    let Some(to) = v.get("to").and_then(J::as_object) else {
        return Err(err(400, "usage", "to is {guest: <principalId or username>}, {guests: [...]} or {room: <handle>}"));
    };
    if to.len() != 1 {
        return Err(err(400, "usage", "to names exactly one of guest, guests or room"));
    }
    let s = |k: &str| to.get(k).and_then(J::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    if to.contains_key("guest") {
        return s("guest").map(To::Guest).ok_or_else(|| err(400, "usage", "to.guest is a principal id or username"));
    }
    if let Some(g) = to.get("guests") {
        let Some(a) = g.as_array() else { return Err(err(400, "usage", "to.guests is an array of principal ids or usernames")) };
        if a.len() > MAX_RECIPIENTS {
            return Err(err_with(413, "too_many_recipients", format!("at most {MAX_RECIPIENTS} recipients per call; send in batches"), json!({ "count": a.len(), "limit": MAX_RECIPIENTS })));
        }
        let names: Vec<String> = a.iter().filter_map(J::as_str).map(str::to_string).collect();
        if names.len() != a.len() || names.is_empty() {
            return Err(err(400, "usage", "to.guests is a non-empty array of strings"));
        }
        return Ok(To::Guests(names));
    }
    if to.contains_key("room") {
        return s("room").map(To::Room).ok_or_else(|| err(400, "usage", "to.room is a room handle or project id"));
    }
    Err(err(400, "usage", "to names guest, guests or room"))
}

fn calls() -> &'static Limiter {
    static L: OnceLock<Limiter> = OnceLock::new();
    L.get_or_init(|| Limiter::new(Duration::from_secs(3600)))
}

fn device_sends() -> &'static Limiter {
    static L: OnceLock<Limiter> = OnceLock::new();
    L.get_or_init(|| Limiter::new(Duration::from_secs(3600)))
}

fn per_guest() -> &'static Limiter {
    static L: OnceLock<Limiter> = OnceLock::new();
    L.get_or_init(|| Limiter::new(Duration::from_secs(3600)))
}

fn rate_limited(wait: u64, limit: usize, what: &str) -> SendErr {
    err_with(429, "rate_limited", format!("over this App's {what}"), json!({ "retryAfterMs": wait, "limit": limit }))
}

/// The rooms an App's guests may be handed: the App's own room plus its
/// guests-rooms grant (CP25, A4).
fn app_rooms(app: &str) -> Result<Vec<String>, SendErr> {
    if let Some(c) = skin::guest_rooms::app_ceiling(app).map_err(|e| err(500, "failed", e))? {
        return Ok(c);
    }
    Ok(super::published(app).map(|r| vec![r.project_id]).unwrap_or_default())
}

/// Resolve the recipients (principal ids) and the not-a-guest count.
fn recipients(app: &str, to: &To) -> Result<(Vec<String>, usize), SendErr> {
    match to {
        To::Guest(g) => store::resolve_guests(app, std::slice::from_ref(g)).map_err(|e| err(500, "failed", e)),
        To::Guests(gs) => store::resolve_guests(app, gs).map_err(|e| err(500, "failed", e)),
        To::Room(r) => {
            let outside = || err(403, "outside_app_rooms", "that room isn't one of this App's rooms (or doesn't exist: the same answer)");
            let ids = skin::resolve_room_tokens(std::slice::from_ref(r)).map_err(|_| outside())?;
            let Some(room) = ids.into_iter().next() else { return Err(outside()) };
            if !app_rooms(app)?.contains(&room) {
                return Err(outside());
            }
            let guests = store::room_guests(app, &room, None).map_err(|e| err(500, "failed", e))?;
            if guests.len() > MAX_RECIPIENTS {
                return Err(err_with(
                    413,
                    "too_many_recipients",
                    format!("that room has more than {MAX_RECIPIENTS} of this App's guests; send in batches with to.guests"),
                    json!({ "count": guests.len(), "limit": MAX_RECIPIENTS }),
                ));
            }
            Ok((guests, 0))
        }
    }
}

/// Send one notification for `app` (a service id the caller's credential
/// named). `Ok` is the `202` body.
pub fn send(app: &str, caller: &Caller, to: &To, n: Notification) -> Result<J, SendErr> {
    super::state(app).map_err(|off| match off {
        super::Off::Host => err(400, "not_for_host", "push is for a published App's own users, never the box login (host)"),
        _ => err(403, "push_off", "this App doesn't send notifications (the owner turns them on: k2 app push on <app>)"),
    })?;
    if crate::push_routes::airgap_blocks() {
        crate::push_routes::note_airgap_drop();
        return Err(err(503, "airgap", "air-gap is on: no notification leaves this server"));
    }
    if crate::vault::get().is_err() {
        return Err(err(503, "vault_closed", "the vault isn't open yet; try again when K2 has finished starting"));
    }
    let cfg = store::config(app).map_err(|e| err(500, "failed", e))?;
    if cfg.apns.is_none() && cfg.fcm.is_none() {
        return Err(err(409, "push_not_configured", "this App has no Apple or Google push key yet (the owner runs k2 app push setup)"));
    }
    calls().take(app, SENDS_PER_HOUR).map_err(|w| rate_limited(w, SENDS_PER_HOUR, "push.send calls an hour"))?;
    let (guests, not_a_guest) = recipients(app, to)?;
    let devices = store::targets(app, &guests).map_err(|e| err(500, "failed", e))?;
    let mut skipped = Skipped { not_a_guest, ..Default::default() };
    let notification = Arc::new(n);
    let send_id = payload::new_send_id();
    let guest_max = if notification.silent { SILENT_PER_GUEST_HOUR } else { ALERTS_PER_GUEST_HOUR };
    let bucket = if notification.silent { "silent" } else { "alert" };
    // Each guest's sendable devices first, with no budget taken yet.
    let mut plan: Vec<(&String, Vec<&store::PushDevice>)> = Vec::new();
    for pid in &guests {
        let mine: Vec<&store::PushDevice> = devices.iter().filter(|d| &d.principal_id == pid).collect();
        if mine.is_empty() {
            skipped.no_devices += 1;
            continue;
        }
        let mut sendable: Vec<&store::PushDevice> = Vec::new();
        for d in mine {
            if notification.topic.as_ref().is_some_and(|t| d.muted_topics.contains(t)) {
                skipped.muted_topic += 1;
            } else if (d.platform == store::PLATFORM_APNS && cfg.apns.is_none()) || (d.platform == store::PLATFORM_FCM && cfg.fcm.is_none()) {
                skipped.platform_not_configured += 1;
            } else {
                sendable.push(d);
            }
        }
        if !sendable.is_empty() {
            plan.push((pid, sendable));
        }
    }
    // The App's device budget before any guest's bucket (review B-L5): a
    // send the box refuses never spends a guest's hourly allowance.
    let candidates: usize = plan.iter().map(|(_, d)| d.len()).sum();
    if candidates > 0 {
        device_sends().take_n(app, candidates, DEVICE_SENDS_PER_HOUR).map_err(|w| rate_limited(w, DEVICE_SENDS_PER_HOUR, "device sends an hour"))?;
    }
    let mut jobs: Vec<Job> = Vec::new();
    for (pid, sendable) in plan {
        if per_guest().take(&format!("{app}\0{pid}\0{bucket}"), guest_max).is_err() {
            skipped.rate_limited += 1;
            continue;
        }
        let who = super::who(app, pid).map_err(|e| err(500, "failed", e))?;
        for d in sendable {
            jobs.push(Job {
                target: Target::from(d),
                notification: notification.clone(),
                stamps: Stamps { app: app.to_string(), send_id: send_id.clone(), kind: caller.kind, who: who.clone() },
            });
        }
    }
    let queued = jobs.len();
    let rec = SendRecord::new(&send_id, &caller.actor, to.kind(), guests.len() + not_a_guest, notification.topic.clone(), queued, skipped.clone());
    // The record exists before a worker can answer for it.
    sendlog::start(app, rec);
    if queued > 0 {
        if let Err(pending) = worker::enqueue(app, jobs) {
            sendlog::refused(app, &send_id, "push_queue_full");
            return Err(err_with(
                429,
                "push_queue_full",
                "this App has too many notifications waiting to go out; try again shortly",
                json!({ "pending": pending, "limit": worker::MAX_PENDING_PER_APP }),
            ));
        }
    }
    Ok(json!({ "ok": true, "sendId": send_id, "queued": queued, "skipped": skipped }))
}

/// A send's counts as `push.status` / `k2 app push log` answer them.
pub fn record_json(r: &SendRecord) -> J {
    json!({
        "sendId": r.send_id,
        "at": r.at,
        "queued": r.queued,
        "sent": r.sent,
        "dead": r.dead,
        "failed": r.failed,
        "wrongApp": r.wrong_app,
        "dropped": r.dropped,
        "skipped": r.skipped,
    })
}

/// Parse a whole send body (`to` + the notification).
pub fn parse_request(body: &[u8]) -> Result<(To, Notification), SendErr> {
    let v: J = serde_json::from_slice(body).map_err(|_| err(400, "usage", "the body must be a JSON object"))?;
    let to = parse_to(&v)?;
    let n = payload::parse(&v)?;
    Ok((to, n))
}

/// Answer a [`SendErr`] in the App routes' shape.
pub fn err_response(e: &SendErr) -> crate::cli_response::CliResponse {
    let status = match e.status {
        400 => "400 Bad Request",
        403 => "403 Forbidden",
        404 => "404 Not Found",
        409 => "409 Conflict",
        413 => "413 Payload Too Large",
        429 => "429 Too Many Requests",
        503 => "503 Service Unavailable",
        _ => "500 Internal Server Error",
    };
    super::refusal_with(status, e.code, &e.hint, e.extra.clone())
}

#[cfg(test)]
#[path = "send_tests.rs"]
mod tests;
