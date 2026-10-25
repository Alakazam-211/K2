//! A guest's own push device (prd-custom-app-push-v1 §5, CP1–CP11).
//!
//! ```text
//! POST /cli/skin/push/register   {deviceId, platform, pushToken, environment?, channel?, previews?, mutedTopics?}
//! POST /cli/skin/push/unregister {deviceId}
//! GET  /cli/skin/push/device     ?deviceId=
//! POST /cli/skin/push/prefs      {deviceId, previews?, mutedTopics?}
//! ```
//!
//! And the App's own verbs on its link (its link pass, `push:send`, §8):
//!
//! ```text
//! POST /cli/skin/push/send       {to, title, body, data?, topic?, …}   → 202 {sendId, queued, skipped}
//! GET  /cli/skin/push/status     ?sendId=                              counts only
//! GET  /cli/skin/push/devices                                          per guest: device count, platforms
//! ```
//!
//! - **A guest's own session pass only** (`principal_id` set, `session=1`),
//!   on any door: the main port (a native app with `Authorization: Bearer
//!   k2skn_…`, admitted by the A′ fence because these are catalog `app`
//!   rows), the browser helper, or the App's link with a guest session of
//!   that same App (AR50). The App's own link pass, an App key, a platform
//!   token or the owner token is `guest_required`.
//! - The App is the pass's App and the guest is the pass's principal; a
//!   body never names either (CP3).
//! - Registering needs the App's push on (`push_off`), never `host`
//!   (`not_for_host`), and the `app` channel (`k2-companion` is
//!   `channel_unavailable` until Companion signs App guests in, §10). At
//!   most [`REGISTERS_PER_HOUR`] registers per pass an hour.
//! - Unregister answers `{ok, removed}` the same for a miss and for someone
//!   else's device (no oracle). A push token is never in an answer.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use k2_core::skin::{app_push as store, SkinPass};
use serde_json::{json, Value};

use super::budget::Limiter;
use super::{off_response, refusal, refusal_with};
use crate::cli_response::CliResponse;

pub const REGISTER: &str = "/cli/skin/push/register";
pub const UNREGISTER: &str = "/cli/skin/push/unregister";
pub const DEVICE: &str = "/cli/skin/push/device";
pub const PREFS: &str = "/cli/skin/push/prefs";
pub const SEND: &str = "/cli/skin/push/send";
pub const STATUS: &str = "/cli/skin/push/status";
pub const DEVICES: &str = "/cli/skin/push/devices";

/// The routes a pass calls (the dispatcher resolves the pass).
pub const PASS_ROUTES: &[&str] = &[REGISTER, UNREGISTER, DEVICE, PREFS, SEND, STATUS, DEVICES];
/// The POST ones (the dispatcher's method guard).
pub const POST_ROUTES: &[&str] = &[REGISTER, UNREGISTER, PREFS, SEND];

/// Registers per pass an hour (CP8).
pub const REGISTERS_PER_HOUR: usize = 30;

fn registers() -> &'static Limiter {
    static L: OnceLock<Limiter> = OnceLock::new();
    L.get_or_init(|| Limiter::new(Duration::from_secs(3600)))
}

fn usage(hint: &str) -> CliResponse {
    refusal("400 Bad Request", "usage", hint)
}

fn guest_required() -> CliResponse {
    refusal("403 Forbidden", "guest_required", "push devices are a guest's own: sign the person in and send their session pass")
}

/// No credential at all on a pass route.
pub fn pass_required() -> CliResponse {
    refusal("401 Unauthorized", "pass_required", "send the guest's session pass (Authorization: Bearer k2skn_…)")
}

fn json_body(body: &[u8]) -> Result<Value, CliResponse> {
    if body.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(json!({}));
    }
    match serde_json::from_slice::<Value>(body) {
        Ok(v @ Value::Object(_)) => Ok(v),
        _ => Err(usage("the body must be a JSON object")),
    }
}

/// The guest a pass is for, or the refusal.
fn guest(pass: &SkinPass) -> Result<&str, CliResponse> {
    pass.principal_id.as_deref().filter(|_| pass.session).ok_or_else(guest_required)
}

/// The device answer (CP9): never the token, the key fingerprints or the
/// bundle id.
pub fn device_json(d: &store::PushDevice) -> Result<Value, CliResponse> {
    let who = super::who(&d.app, &d.principal_id).map_err(CliResponse::internal_error)?;
    let cfg = store::config(&d.app).map_err(CliResponse::internal_error)?;
    Ok(json!({
        "ok": true,
        "id": d.id,
        "deviceId": d.device_id,
        "platform": d.platform,
        "environment": d.environment,
        "channel": d.channel,
        "previews": d.previews,
        "mutedTopics": d.muted_topics,
        "lastSeenAt": d.last_seen_at,
        "who": who,
        "configured": { "apns": cfg.apns.is_some(), "fcm": cfg.fcm.is_some() },
    }))
}

fn answer(r: Result<Value, CliResponse>) -> CliResponse {
    match r {
        Ok(v) => CliResponse::ok_json(v.to_string()),
        Err(e) => e,
    }
}

/// `POST /cli/skin/push/register`.
pub fn handle_register(pass: &SkinPass, body: &[u8]) -> CliResponse {
    answer((|| {
        let pid = guest(pass)?;
        if pass.app.trim().is_empty() || pass.app == k2_core::skin::HOST_APP_ID {
            return Err(off_response(super::Off::Host));
        }
        let v = json_body(body)?;
        let reg = store::parse_registration(&v).map_err(|e| usage(&e))?;
        if reg.channel == store::CHANNEL_COMPANION {
            return Err(refusal(
                "400 Bad Request",
                "channel_unavailable",
                "K2 Companion can't receive this App's notifications yet: register your own app (channel app)",
            ));
        }
        super::state(&pass.app).map_err(off_response)?;
        if let Err(wait) = registers().take(&pass.id, REGISTERS_PER_HOUR) {
            let mut extra = serde_json::Map::new();
            extra.insert("retryAfterMs".into(), json!(wait));
            extra.insert("limit".into(), json!(REGISTERS_PER_HOUR));
            return Err(refusal_with(
                "429 Too Many Requests",
                "rate_limited",
                "too many registers from this pass this hour; register on launch and when the token changes",
                extra,
            ));
        }
        match store::register(&pass.app, pid, &pass.id, &reg) {
            Ok(d) => device_json(&d),
            Err(store::RegisterError::AppFull { limit }) => {
                let mut extra = serde_json::Map::new();
                extra.insert("limit".into(), json!(limit));
                Err(refusal_with(
                    "409 Conflict",
                    "push_devices_full",
                    "this App holds as many push devices as its owner allows (k2 app push limit)",
                    extra,
                ))
            }
            Err(store::RegisterError::Store(e)) => Err(CliResponse::internal_error(e)),
        }
    })())
}

/// `POST /cli/skin/push/unregister {deviceId}`.
pub fn handle_unregister(pass: &SkinPass, body: &[u8]) -> CliResponse {
    answer((|| {
        let pid = guest(pass)?;
        let v = json_body(body)?;
        let device = v.get("deviceId").and_then(Value::as_str).map(str::trim).filter(|s| store::valid_device_id(s));
        let Some(device) = device else { return Err(usage("send {deviceId}")) };
        let removed = store::unregister(&pass.app, pid, device).map_err(CliResponse::internal_error)?;
        Ok(json!({ "ok": true, "removed": removed }))
    })())
}

fn not_found() -> CliResponse {
    refusal("404 Not Found", "not_found", "no push device of yours by that deviceId on this App")
}

/// `GET /cli/skin/push/device?deviceId=`.
pub fn handle_device(pass: &SkinPass, params: &HashMap<String, String>) -> CliResponse {
    answer((|| {
        let pid = guest(pass)?;
        let device = params.get("deviceId").map(|s| s.trim()).filter(|s| store::valid_device_id(s));
        let Some(device) = device else { return Err(usage("send ?deviceId=")) };
        match store::get_own(&pass.app, pid, device).map_err(CliResponse::internal_error)? {
            Some(d) => device_json(&d),
            None => Err(not_found()),
        }
    })())
}

/// `POST /cli/skin/push/prefs {deviceId, previews?, mutedTopics?}`.
pub fn handle_prefs(pass: &SkinPass, body: &[u8]) -> CliResponse {
    answer((|| {
        let pid = guest(pass)?;
        let v = json_body(body)?;
        let (device, previews, muted) = store::parse_prefs(&v).map_err(|e| usage(&e))?;
        match store::set_prefs(&pass.app, pid, &device, previews, muted.as_deref()).map_err(CliResponse::internal_error)? {
            Some(d) => device_json(&d),
            None => Err(not_found()),
        }
    })())
}

// ── the App's own verbs on its link ──────────────────────────────────

/// The App a link pass sends for: this link's own App pass (never a guest
/// session on the link, never off a link), holding `push:send`.
/// `link_app` is the link's App (the dispatcher read it on the request's
/// task; the handler runs on a blocking thread).
fn app_on_link(pass: &SkinPass, link_app: Option<&str>) -> Result<String, CliResponse> {
    let not_app = || refusal("403 Forbidden", "app_pass_required", "only this App's own pass, on its link, sends notifications (a guest never does)");
    let Some(app) = link_app else { return Err(not_app()) };
    if pass.session || pass.principal_id.is_some() || pass.app != app {
        return Err(not_app());
    }
    if !pass.has_cap(k2_core::skin::CAP_PUSH_SEND) {
        let mut extra = serde_json::Map::new();
        extra.insert("cap".into(), json!(k2_core::skin::CAP_PUSH_SEND));
        return Err(refusal_with(
            "403 Forbidden",
            "cap_not_granted",
            "this App's pass doesn't carry push:send (add it to the manifest; the owner approves it once)",
            extra,
        ));
    }
    Ok(app.to_string())
}

/// `POST /cli/skin/push/send` (link).
pub fn handle_send(pass: &SkinPass, link_app: Option<&str>, body: &[u8]) -> CliResponse {
    answer((|| {
        let app = app_on_link(pass, link_app)?;
        let (to, n) = super::send::parse_request(body).map_err(|e| super::send::err_response(&e))?;
        let caller = super::send::Caller { actor: format!("app:{app}"), kind: super::payload::KIND_APP };
        super::send::send(&app, &caller, &to, n).map_err(|e| super::send::err_response(&e))
    })())
    .accepted()
}

/// `GET /cli/skin/push/status?sendId=` (link): counts only.
pub fn handle_status(pass: &SkinPass, link_app: Option<&str>, params: &HashMap<String, String>) -> CliResponse {
    answer((|| {
        let app = app_on_link(pass, link_app)?;
        let id = params.get("sendId").map(|s| s.trim()).filter(|s| !s.is_empty()).ok_or_else(|| usage("send ?sendId= (from push.send)"))?;
        match super::sendlog::get(&app, id) {
            Some(r) => {
                let mut v = super::send::record_json(&r);
                v["ok"] = json!(true);
                Ok(v)
            }
            None => Err(refusal("404 Not Found", "not_found", "no send of this App by that id in the last hour")),
        }
    })())
}

/// `GET /cli/skin/push/devices` (link): per guest, a device count and the
/// platforms. Never a token, a device id or a name.
pub fn handle_devices(pass: &SkinPass, link_app: Option<&str>) -> CliResponse {
    answer((|| {
        let app = app_on_link(pass, link_app)?;
        let counts = store::device_counts(&app).map_err(CliResponse::internal_error)?;
        let guests: Vec<Value> = counts.iter().map(|(pid, n, plats)| json!({ "guest": pid, "devices": n, "platforms": plats })).collect();
        Ok(json!({ "ok": true, "guests": guests }))
    })())
}

trait Accepted {
    fn accepted(self) -> Self;
}

impl Accepted for CliResponse {
    /// A queued send answers 202 (CP28).
    fn accepted(mut self) -> Self {
        if self.status.starts_with("200") {
            self.status = "202 Accepted";
        }
        self
    }
}

/// The pass routes (the dispatcher resolved `pass`; `link_app` is the
/// request's link App, if it came in on one).
pub fn handle_pass(path: &str, pass: &SkinPass, link_app: Option<&str>, is_post: bool, body: &[u8], params: &HashMap<String, String>) -> CliResponse {
    match (path, is_post) {
        (REGISTER, true) => handle_register(pass, body),
        (UNREGISTER, true) => handle_unregister(pass, body),
        (DEVICE, false) => handle_device(pass, params),
        (PREFS, true) => handle_prefs(pass, body),
        (SEND, true) => handle_send(pass, link_app, body),
        (STATUS, false) => handle_status(pass, link_app, params),
        (DEVICES, false) => handle_devices(pass, link_app),
        _ => CliResponse::method_not_allowed(),
    }
}

/// A credential that isn't a pass at all (the owner token, an API key, a
/// passport) on a pass route.
pub fn not_a_pass() -> CliResponse {
    guest_required()
}

#[cfg(test)]
#[path = "guest_routes_tests.rs"]
mod tests;
