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
//! - `GET  /cli/grokbots/thread?agent=<id>&since=<seq>&limit=<n>` → the
//!   bot's collected Thread: `{agent, addr, conversationId, supported,
//!   messages, nextSince, hasMoreBefore, stale?}` (`seq` = Thread seq)
//! - `POST /cli/grokbots/send`     `{agent, text}` → `{ok, clientNonce}`
//! - `GET  /cli/thread?addr=gb:<name>` (the dispatcher hands a `gb:`
//!   address here, [`thread_by_addr`]) → the same Thread in the overlay
//!   snapshot shape, so `k2 thread gb:<name>` reads it. Same gates.
//!
//! **Collected Threads.** K2 keeps every Grok Bot message it sees in its
//! own Thread store, one conversation per bot (`gb:<name>`, see
//! `k2_core::external_agents::collect`): from the moment collection starts
//! (a start line), new `store.db` rows for a bot whose chat is on this
//! computer, each new latest roster entry for a **stale** bot (live
//! conversation elsewhere; gap lines where some may be missing), and every
//! send made here. The poller collects headless; `thread` also collects
//! before it reads (a read never waits for the poller). Older history in
//! Grok Bot's `store.db` is not shown. Collected Threads never wake an
//! agent, reach a PTY or federate; agents and App guests can't read them
//! (`/cli/thread` on a cell socket answers 403 for `gb:`).
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

use k2_core::external_agents::collect;
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

const SETTINGS_HINT: &str = "Settings → 3rd Party Bots → Grok Bot";

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
    let (installed, mut agents) = match p.list_agents() {
        Ok(a) => (true, a),
        Err(ExtError::NotInstalled) => (false, Vec::new()),
        Err(e) => return ext_err(e),
    };
    for a in agents.iter_mut().filter(|a| a.stale) {
        if let Some(text) = collected_preview(a) {
            a.last_message_preview = Some(text);
        }
    }
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
            Ok(n) => n.clamp(1, grokbot::MAX_PAGE),
            Err(_) => return refuse("400 Bad Request", "bad_input", "limit must be a number"),
        },
        None => 100,
    };
    if !k2_core::external_agents::valid_agent_id(agent) {
        return refuse("400 Bad Request", "bad_input", "invalid agent id");
    }
    let p = provider();
    let bot = match collect_now(&p, agent) {
        Ok(b) => b,
        Err(r) => return r,
    };
    // After a cursor: everything newer (no line skipped); else the newest
    // `limit`.
    let page = match collect::page(agent, since, None, if since.is_some() { 0 } else { limit }) {
        Ok(pg) => pg,
        Err(e) => return CliResponse::internal_error(format!("read Thread: {e}")),
    };
    let next_since = page
        .items
        .iter()
        .map(|i| i.seq)
        .max()
        .unwrap_or(since.unwrap_or(0))
        .max(since.unwrap_or(0));
    let messages: Vec<J> = page.items.iter().map(collect::message_json).collect();
    let mut v = json!({
        "ok": true,
        "agent": agent,
        "addr": collect::address(&bot.name),
        "conversationId": collect::conversation_id(agent),
        "supported": bot.unsupported.is_none(),
        "messages": messages,
        "nextSince": next_since,
        "hasMoreBefore": since.is_none() && page.has_more,
    });
    if let Some(r) = bot.unsupported {
        v["reason"] = J::String(r);
    }
    if bot.stale {
        v["stale"] = J::Bool(true);
    }
    resp("200 OK", v)
}

/// What a read learned about the bot while collecting.
struct Bot {
    name: String,
    stale: bool,
    unsupported: Option<String>,
}

/// Collect what's new for `agent` before a read (a read never waits for
/// the poller), and tell other clients when that changed the Thread. A bot
/// the roster no longer lists still reads its collected Thread; one K2
/// never collected is 404.
fn collect_now(p: &GrokBot, agent: &str) -> Result<Bot, CliResponse> {
    let roster = match p.list_agents() {
        Ok(l) => l,
        Err(ExtError::NotInstalled) => Vec::new(),
        Err(e) => return Err(ext_err(e)),
    };
    let names: HashMap<String, String> = roster.iter().map(|a| (a.id.clone(), a.name.clone())).collect();
    if let Ok(moved) = collect::migrate_legacy_tail(p, &names) {
        for id in moved {
            crate::grokbots_watch::announce_agent(&id);
        }
    }
    let now = chrono::Utc::now().timestamp_millis();
    match roster.iter().find(|a| a.id.eq_ignore_ascii_case(agent)) {
        Some(a) => {
            let synced = collect::sync_agent(p, a, true, now)
                .map_err(|e| CliResponse::internal_error(format!("collect: {e}")))?;
            if synced.changed {
                crate::grokbots_watch::announce_agent(&a.id);
            }
            Ok(Bot {
                name: a.name.clone(),
                stale: a.stale,
                unsupported: synced.unsupported,
            })
        }
        None => match collect::started(agent) {
            Ok(true) => Ok(Bot {
                name: collect::page(agent, None, Some(2), 1)
                    .ok()
                    .and_then(|pg| pg.items.first().and_then(|i| i.doc.to.clone()))
                    .and_then(|to| collect::parse_addr(&to).map(str::to_string))
                    .unwrap_or_else(|| grokbot::fallback_name_for(agent)),
                stale: false,
                unsupported: None,
            }),
            Ok(false) => Err(ext_err(ExtError::UnknownAgent(agent.to_string()))),
            Err(e) => Err(CliResponse::internal_error(format!("read Thread: {e}"))),
        },
    }
}

/// A stale bot's row preview: the newest collected line when it is at
/// least as new as the roster's own latest activity (a K2 send just made, a
/// reply the poller saw). Otherwise the roster's preview stands.
fn collected_preview(a: &k2_core::external_agents::ExternalAgent) -> Option<String> {
    let (at, text) = collect::newest_line(&a.id)?;
    if a.last_activity_at.is_some_and(|t| at < t) {
        return None;
    }
    grokbot::clean_preview(&text)
}

/// `gb:<name>` / `gb:<agent id>` → the bot's agent id (and its name).
fn resolve_gb(p: &GrokBot, wanted: &str) -> Result<(String, String), CliResponse> {
    let roster = match p.list_agents() {
        Ok(l) => l,
        Err(ExtError::NotInstalled) => Vec::new(),
        Err(e) => return Err(ext_err(e)),
    };
    if let Some(a) = roster.iter().find(|a| a.id.eq_ignore_ascii_case(wanted)) {
        return Ok((a.id.clone(), a.name.clone()));
    }
    let named: Vec<_> = roster
        .iter()
        .filter(|a| a.name.eq_ignore_ascii_case(wanted))
        .collect();
    match named.as_slice() {
        [a] => Ok((a.id.clone(), a.name.clone())),
        [] if k2_core::external_agents::valid_agent_id(wanted)
            && collect::started(wanted).unwrap_or(false) =>
        {
            Ok((wanted.to_string(), wanted.to_string()))
        }
        [] => Err(refuse(
            "404 Not Found",
            "unknown_agent",
            format!("no Grok Bot named '{wanted}' on this server"),
        )),
        many => Err(refuse(
            "409 Conflict",
            "ambiguous_agent",
            format!(
                "several Grok Bots are named '{wanted}': use gb:<id> with one of {}",
                many.iter().map(|a| a.id.as_str()).collect::<Vec<_>>().join(", ")
            ),
        )),
    }
}

/// Whether `/cli/thread` should hand `addr` to [`thread_by_addr`].
pub fn is_gb_addr(addr: &str) -> bool {
    collect::parse_addr(addr).is_some()
}

/// `GET /cli/thread?addr=gb:<name>` for a TCP caller: the bot's collected
/// Thread in the overlay snapshot shape (`k2 thread gb:<name>`). Same
/// gates as the Grok Bots routes (people only, Show on). Paging like
/// `/cli/thread`: `since_seq`, `before_seq`, `limit` (default 25, `0` =
/// all, at most 500).
pub fn thread_by_addr(who: Who, params: &HashMap<String, String>) -> CliResponse {
    if !is_human(&who) {
        return humans_only();
    }
    if !settings::load().show {
        return show_off();
    }
    let addr = params.get("addr").map(String::as_str).unwrap_or("");
    let Some(wanted) = collect::parse_addr(addr) else {
        return refuse("400 Bad Request", "bad_input", "addr must be gb:<bot name>");
    };
    let p = provider();
    let (id, _) = match resolve_gb(&p, wanted) {
        Ok(r) => r,
        Err(r) => return r,
    };
    // inbox-catchup-v1 (§3.1): every `GET /cli/thread` answer carries the
    // overlay `rev` (read before the store read) and `instanceId`.
    let rev = crate::overlay_ws::current_rev();
    let instance_id = crate::boot_status::instance_id();
    let bot = match collect_now(&p, &id) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let num = |k: &str| params.get(k).and_then(|v| v.parse::<i64>().ok());
    let limit = match num("limit") {
        None => 25,
        Some(0) => 0,
        Some(n) if n < 0 => 25,
        Some(n) => (n as usize).clamp(1, 500),
    };
    // thread-delta-v1: `after_seq=N`, same shape as the workspace Thread's.
    match crate::overlay_routes::parse_after_seq(params) {
        Err(r) => return r,
        Ok(Some(after)) => {
            if params.contains_key("since_seq") || params.contains_key("before_seq") {
                return refuse("400 Bad Request", "usage", "after_seq cannot be combined with since_seq or before_seq");
            }
            return match collect::delta(&id, after, limit) {
                Ok(d) => resp(
                    "200 OK",
                    json!({
                        "ok": true,
                        "collection": "thread",
                        "addr": collect::address(&bot.name),
                        "conversation_id": collect::conversation_id(&id),
                        "after_seq": after,
                        "items": d.items,
                        "has_more": d.has_more,
                        "changed": d.changed,
                        "removed": crate::overlay_routes::removed_json(&d.removed),
                        "pastAddresses": [],
                        "rev": rev,
                        "instanceId": instance_id,
                    }),
                ),
                Err(e) => CliResponse::internal_error(format!("read Thread: {e}")),
            };
        }
        Ok(None) => {}
    }
    match collect::page(&id, num("since_seq"), num("before_seq"), limit) {
        Ok(page) => resp(
            "200 OK",
            json!({
                "ok": true,
                "collection": "thread",
                "addr": collect::address(&bot.name),
                "conversation_id": collect::conversation_id(&id),
                "items": page.items,
                "has_more": page.has_more,
                "pastAddresses": [],
                "rev": rev,
                "instanceId": instance_id,
            }),
        ),
        Err(e) => CliResponse::internal_error(format!("read Thread: {e}")),
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
    let sender = sender_name(who);
    let prompt = from_envelope(&sender, &b.text);
    let p = provider();
    match p.send(&b.agent, &prompt) {
        Ok(r) => {
            log_debug!(
                "[grokbots] send to {} by {} ({} bytes{})",
                b.agent,
                who.label(),
                b.text.len(),
                if r.agent_stale { ", stale" } else { "" }
            );
            // Into the bot's Thread at once (linked later to the line Grok
            // Bot reports for it, so it never shows twice).
            if let Err(e) = collect::record_own_send(
                &p,
                &b.agent,
                &r.client_nonce,
                &prompt,
                &sender,
                Some(post_author(who)),
                now,
            ) {
                log_debug!("[grokbots] send to {} not recorded in its Thread: {e}", b.agent);
            }
            crate::grokbots_watch::announce_agent(&b.agent);
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
/// owner token → the shared stamp rule (the linked account's username,
/// else `owner`; 0.46.0, the old `ownerDisplayName` is not read), a
/// Connect login → its username. Falls back to `the owner`.
fn sender_name(who: &Who) -> String {
    let raw = match who {
        Who::Owner => k2_core::person_name::sender_identity_now(k2_core::person_name::Sender::OwnerToken),
        Who::Login { username, .. } => username.clone(),
        _ => String::new(),
    };
    clean_from_name(&raw).unwrap_or_else(|| "the owner".to_string())
}

/// Who sent, as the Thread line's `author` (the credential, like a Thread
/// post's).
fn post_author(who: &Who) -> k2_core::overlay::PostAuthor {
    match who {
        Who::Login { username, .. } => k2_core::overlay::PostAuthor::user(username),
        _ => k2_core::overlay::PostAuthor::owner_named(&k2_core::person_name::sender_identity_now(k2_core::person_name::Sender::OwnerToken)),
    }
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
