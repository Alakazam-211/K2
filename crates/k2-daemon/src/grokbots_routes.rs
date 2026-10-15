//! `/cli/grokbots/*` — Local Grok Bots (0.45.7).
//!
//! Grok Bot agents running on THIS computer, for the daemon's own OS user,
//! shown next to K2's agents. The adapter is `k2_core::external_agents::grokbot`;
//! this module is the gate in front of it.
//!
//! Who may call (mirrors a human talking to a K2 agent, never an agent):
//! - the owner daemon token or a Connect login: reads (Member floor);
//! - send and settings writes: the owner token or an Owner/Admin login
//!   (Admin floor in `route_policy`; a send speaks as the Grok Bot account
//!   holder and runs a billed turn);
//! - agent passports, App passes (`k2skn_`) and API keys: always refused
//!   here (403 `grokbots_humans_only`), so no agent and no App guest can
//!   read a human's Grok Bot chat or send as the human.
//!
//! Consent (`k2_core::external_agents::settings`, both OFF by default):
//! - Show off → `list` answers `{enabled:false, agents:[]}` without
//!   touching `~/sand-data`; `thread`/`send` answer 403 `grokbots_off`.
//! - Send off → `send` answers 403 `grokbots_send_off`.
//! - `detect` (Admin) is the explicit preview in Settings: install marker
//!   + profile names only, so the owner sees what K2 found before enabling.
//!
//! Routes:
//! - `GET  /cli/grokbots/settings` → `{show, send, canEdit}`
//! - `POST /cli/grokbots/settings` `{show?, send?}` → same shape
//! - `GET  /cli/grokbots/detect`   → `{installed, dataPath, agents}`
//! - `GET  /cli/grokbots/list`     → `{enabled, sendEnabled, status, agents}`
//! - `GET  /cli/grokbots/status`   → `{enabled, sendEnabled, status}`
//! - `GET  /cli/grokbots/thread?agent=<id>&since=<seq>&limit=<n>` → `ThreadPage`
//! - `POST /cli/grokbots/send`     `{agent, text}` → `{ok, clientNonce}`
//!
//! A send reaches Grok Bot as `[from <name>] <text>`, so the bot knows who
//! wrote. The name comes from the credential, never the body (the same
//! sources as a Thread card's `answeredBy`): the owner token → the owner's
//! display name from Settings, a Connect login → its username. A
//! `[from …]` the person typed is kept as text behind ours.
//!
//! The gateway token is never returned, logged or carried off the box.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;
use serde_json::{json, Value as J};

use k2_core::external_agents::grokbot::{self, GrokBot};
use k2_core::external_agents::rate_limit::SendLimiter;
use k2_core::external_agents::settings::{self, LocalGrokBotsSettings};
use k2_core::external_agents::{ExtError, ExternalAgentProvider};
use k2_core::log_debug;

use crate::cli_response::CliResponse;
use crate::compute_routes::Who;

pub const MAX_BODY: usize = 64 * 1024;

pub const GET_ROUTES: &[&str] = &[
    "/cli/grokbots/detect",
    "/cli/grokbots/list",
    "/cli/grokbots/settings",
    "/cli/grokbots/status",
    "/cli/grokbots/thread",
];

pub const POST_ROUTES: &[&str] = &["/cli/grokbots/send", "/cli/grokbots/settings"];

pub fn is_route(path: &str) -> bool {
    GET_ROUTES.contains(&path) || POST_ROUTES.contains(&path)
}

/// POST-only rows (a GET on them is 405).
pub fn is_post_only(path: &str) -> bool {
    POST_ROUTES.contains(&path) && !GET_ROUTES.contains(&path)
}

/// Classify a TCP caller (same classifier as compute / sources).
pub fn who_from_tcp(query: &str, bearer: Option<&str>, owner_token: &str) -> Who {
    crate::compute_routes::who_from_tcp(query, bearer, owner_token)
}

fn resp(status: &'static str, body: J) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: body.to_string(),
    }
}

fn refuse(status: &'static str, code: &str, message: impl Into<String>) -> CliResponse {
    resp(
        status,
        json!({ "ok": false, "error": code, "message": message.into() }),
    )
}

pub fn too_large() -> CliResponse {
    refuse(
        "413 Payload Too Large",
        "body_too_large",
        format!("Bodies here are at most {} KiB.", MAX_BODY / 1024),
    )
}

const SETTINGS_HINT: &str = "Settings → K2 Server → Local Grok Bots";

fn humans_only() -> CliResponse {
    refuse(
        "403 Forbidden",
        "grokbots_humans_only",
        "Local Grok Bots are for people signed in to this server. Agents and App guests can't read or send here.",
    )
}

fn show_off() -> CliResponse {
    refuse(
        "403 Forbidden",
        "grokbots_off",
        format!("Local Grok Bots are off on this server. Turn on Show in {SETTINGS_HINT}."),
    )
}

fn send_off() -> CliResponse {
    refuse(
        "403 Forbidden",
        "grokbots_send_off",
        format!("Sending to Grok Bots is off on this server. Turn on Send in {SETTINGS_HINT}."),
    )
}

fn admin_only() -> CliResponse {
    refuse(
        "403 Forbidden",
        "grokbots_admin_only",
        "Only the owner or an admin can do this.",
    )
}

fn ext_err(e: ExtError) -> CliResponse {
    let status = match e {
        ExtError::NotInstalled | ExtError::UnknownAgent(_) => "404 Not Found",
        ExtError::BadInput(_) => "400 Bad Request",
        ExtError::Unreachable(_) | ExtError::Protocol(_) => "502 Bad Gateway",
    };
    refuse(status, e.code(), e.to_string())
}

/// The provider for the daemon's own OS user.
fn provider() -> GrokBot {
    GrokBot::for_current_user()
}

fn limiter() -> &'static SendLimiter {
    static L: OnceLock<SendLimiter> = OnceLock::new();
    L.get_or_init(SendLimiter::default)
}

fn is_human(who: &Who) -> bool {
    matches!(who, Who::Owner | Who::Login { .. })
}

fn settings_json(s: &LocalGrokBotsSettings, can_edit: bool) -> J {
    json!({ "show": s.show, "send": s.send, "canEdit": can_edit })
}

pub fn handle(
    path: &str,
    is_post: bool,
    who: Who,
    body: &[u8],
    params: &HashMap<String, String>,
) -> CliResponse {
    if !is_human(&who) {
        return humans_only();
    }
    match (path, is_post) {
        ("/cli/grokbots/settings", false) => ok(settings_json(&settings::load(), who.is_admin())),
        ("/cli/grokbots/settings", true) => write_settings(&who, body),
        ("/cli/grokbots/detect", false) => {
            if !who.is_admin() {
                return admin_only();
            }
            let d = provider().detect();
            resp(
                "200 OK",
                json!({ "ok": true, "installed": d.installed, "dataPath": d.data_path, "agents": d.agents }),
            )
        }
        ("/cli/grokbots/list", false) => list(),
        ("/cli/grokbots/status", false) => status(),
        ("/cli/grokbots/thread", false) => thread(params),
        ("/cli/grokbots/send", true) => send(&who, body),
        (_, false) if POST_ROUTES.contains(&path) => {
            refuse("405 Method Not Allowed", "post_required", "POST required")
        }
        _ => refuse("404 Not Found", "not_found", "no such route"),
    }
}

fn ok(mut v: J) -> CliResponse {
    if let Some(o) = v.as_object_mut() {
        o.insert("ok".into(), J::Bool(true));
    }
    resp("200 OK", v)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SettingsBody {
    show: Option<bool>,
    send: Option<bool>,
}

fn write_settings(who: &Who, body: &[u8]) -> CliResponse {
    if !who.is_admin() {
        return admin_only();
    }
    let b: SettingsBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => {
            return refuse(
                "400 Bad Request",
                "bad_body",
                format!("expected {{show?, send?}}: {e}"),
            )
        }
    };
    let mut s = settings::load();
    if let Some(v) = b.show {
        s.show = v;
    }
    if let Some(v) = b.send {
        s.send = v;
    }
    if let Err(e) = settings::save(&s) {
        return CliResponse::internal_error(format!("save setting: {e}"));
    }
    log_debug!(
        "[grokbots] settings show={} send={} by {}",
        s.show,
        s.send,
        who.label()
    );
    crate::grokbots_watch::nudge_roster();
    ok(settings_json(&s, true))
}

fn list() -> CliResponse {
    let s = settings::load();
    if !s.show {
        // Off: nothing under ~/sand-data is read.
        return resp(
            "200 OK",
            json!({ "ok": true, "enabled": false, "sendEnabled": false, "installed": false, "agents": [] }),
        );
    }
    let p = provider();
    let (installed, agents) = match p.list_agents() {
        Ok(a) => (true, a),
        Err(ExtError::NotInstalled) => (false, Vec::new()),
        Err(e) => return ext_err(e),
    };
    let status = if installed {
        p.status()
    } else {
        Default::default()
    };
    resp(
        "200 OK",
        json!({
            "ok": true,
            "enabled": true,
            "sendEnabled": s.send_allowed(),
            "sendWhileBusy": grokbot::schema::SEND_WHILE_BUSY_ALLOWED,
            "installed": installed,
            "label": grokbot::LABEL,
            "status": status,
            "agents": agents,
        }),
    )
}

fn status() -> CliResponse {
    let s = settings::load();
    if !s.show {
        return resp(
            "200 OK",
            json!({ "ok": true, "enabled": false, "sendEnabled": false }),
        );
    }
    resp(
        "200 OK",
        json!({ "ok": true, "enabled": true, "sendEnabled": s.send_allowed(), "status": provider().status() }),
    )
}

fn thread(params: &HashMap<String, String>) -> CliResponse {
    if !settings::load().show {
        return show_off();
    }
    let Some(agent) = params.get("agent").filter(|a| !a.is_empty()) else {
        return refuse("400 Bad Request", "bad_input", "agent=<id> is required");
    };
    let since = match params.get("since").filter(|v| !v.is_empty()) {
        Some(v) => match v.parse::<i64>() {
            Ok(n) if n >= 0 => Some(n),
            _ => return refuse("400 Bad Request", "bad_input", "since must be a seq number"),
        },
        None => None,
    };
    let limit = match params.get("limit").filter(|v| !v.is_empty()) {
        Some(v) => match v.parse::<usize>() {
            Ok(n) => n,
            Err(_) => return refuse("400 Bad Request", "bad_input", "limit must be a number"),
        },
        None => 100,
    };
    match provider().read_thread(agent, since, limit) {
        Ok(page) => {
            let mut v = serde_json::to_value(&page).unwrap_or(J::Null);
            if let Some(o) = v.as_object_mut() {
                o.insert("ok".into(), J::Bool(true));
                o.insert("agent".into(), J::String(agent.clone()));
            }
            resp("200 OK", v)
        }
        Err(e) => ext_err(e),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SendBody {
    agent: String,
    text: String,
}

fn send(who: &Who, body: &[u8]) -> CliResponse {
    let s = settings::load();
    if !s.show {
        return show_off();
    }
    if !s.send_allowed() {
        return send_off();
    }
    if !who.is_admin() {
        return admin_only();
    }
    let b: SendBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => {
            return refuse(
                "400 Bad Request",
                "bad_body",
                format!("expected {{agent, text}}: {e}"),
            )
        }
    };
    if !k2_core::external_agents::valid_agent_id(&b.agent) {
        return refuse("400 Bad Request", "bad_input", "invalid agent id");
    }
    if b.text.trim().is_empty() {
        return refuse("400 Bad Request", "bad_input", "message is empty");
    }
    let now = chrono::Utc::now().timestamp_millis();
    if let Err(retry_ms) = limiter().check_and_record(&b.agent, now) {
        return resp(
            "429 Too Many Requests",
            json!({
                "ok": false,
                "error": "rate_limited",
                "message": "Slow down: each message runs a billed Grok turn.",
                "retryAfterMs": retry_ms,
            }),
        );
    }
    let prompt = from_envelope(&sender_name(who), &b.text);
    match provider().send(&b.agent, &prompt) {
        Ok(r) => {
            log_debug!(
                "[grokbots] send to {} by {} ({} bytes)",
                b.agent,
                who.label(),
                b.text.len()
            );
            crate::grokbots_watch::nudge_agent(&b.agent);
            resp(
                "200 OK",
                json!({ "ok": true, "clientNonce": r.client_nonce }),
            )
        }
        Err(e) => {
            log_debug!(
                "[grokbots] send to {} by {} failed: {}",
                b.agent,
                who.label(),
                e.code()
            );
            ext_err(e)
        }
    }
}

/// Longest name the `[from …]` envelope carries (chars).
pub const FROM_NAME_MAX: usize = 64;

/// Who is sending, for the `[from …]` envelope. Credential-derived: the
/// owner token → `owner_display_name` from Settings, a Connect login → its
/// username. Falls back to `the owner`.
fn sender_name(who: &Who) -> String {
    let raw = match who {
        Who::Owner => k2_core::app_settings::load().owner_display_name.unwrap_or_default(),
        Who::Login { username, .. } => username.clone(),
        _ => String::new(),
    };
    clean_from_name(&raw).unwrap_or_else(|| "the owner".to_string())
}

/// A name safe inside `[from <name>]`: no `[`, `]`, line breaks or other
/// control characters (so a name can't close the envelope or start a second
/// one), trimmed, at most [`FROM_NAME_MAX`] chars. None when nothing is left.
pub fn clean_from_name(raw: &str) -> Option<String> {
    let kept: String = raw
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '[' | ']' | '\u{2028}' | '\u{2029}'))
        .collect();
    let capped: String = kept.trim().chars().take(FROM_NAME_MAX).collect();
    let capped = capped.trim();
    (!capped.is_empty()).then(|| capped.to_string())
}

/// The prompt Grok Bot receives: `[from <name>] <text>`. Always prefixed,
/// even when the text already starts with a `[from …]` of its own.
pub fn from_envelope(name: &str, text: &str) -> String {
    format!("[from {name}] {}", text.trim())
}

#[cfg(test)]
mod tests;
