//! Agents review the K5 rooms' transcripts after a call (K2 0.46.11; Rosson
//! 2026-11-10). The store is `k2_core::voice_transcripts`.
//!
//! ```text
//! GET  /cli/voice/recordings[?room=&after=&limit=]   the K5 rooms' recordings (no text)
//! GET  /cli/voice/transcript?id=[&after=&limit=]     one recording's transcript, a page at a time
//! GET  /cli/voice/transcripts[?room=]                who may read, and the rooms' subscriptions
//! POST /cli/voice/transcripts/subscribe {room, agent, on}   "send transcripts to <agent>"
//! GET  /cli/voice/transcripts/reads[?id=&limit=]     the read log (owner and Admins)
//! ```
//!
//! **Who reads.** An agent whose workspace holds "Read call transcripts"
//! (`voice.transcripts-read`, Owner only, off by default), and the owner's
//! or an Admin's own login. Never the bare owner token for the text (agents
//! can read that token on a shared box, and without their passport the CLI
//! falls back to it): it may list, never read. Members and Voice & Chat only
//! logins are refused (they read a room's transcript as messages in Sew,
//! where they are members).
//!
//! **Scope.** Only recordings in the K5 rooms (the K2 Admin Space). A
//! recording in an App's Space answers `not_found`, as if it didn't exist:
//! an App's Spaces keep their own rule (`server_read`).
//!
//! **Sew.** Sew 0.9.0's compliance reads (no `app_id`, `mgmt_recording_reads`)
//! with `Sew-Actor: k2:owner`: the owner turned the switch on. Sew audits
//! `recording.transcript_read`; K2 logs every page beside it with the agent
//! or person, the recording and the line count.
//!
//! **After a call** ([`notify_once`]): a room may send a note to chosen
//! agents' inboxes when a recording's transcript is final (opt-in per room
//! and per agent). The notifier follows the K5 Space's recordings list
//! while any subscription exists.
//!
//! **The K5 rooms' consent mode** ([`ensure_consent_mode`], Rosson
//! 2026-11-10): in K2's own rooms people are told a recording is on and
//! can't opt out: K2 sets each room's `require_consent` (declining means not
//! joining), and Sew drops a person who doesn't answer in time
//! (`recording.consent_prompt_timeout_ms`, set when recording is enabled).

use std::collections::{HashMap, HashSet};

use k2_core::connect_users::Role;
use k2_core::tool_hook::sew_wire::{self, recording as wire};
use k2_core::tool_hook::{self as hook};
use k2_core::voice_transcripts as store;
use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::compute_routes::Who;
use crate::sew_mgmt::{MgmtClient, MgmtResponse};

pub const LIST: &str = "/cli/voice/recordings";
pub const TRANSCRIPT: &str = "/cli/voice/transcript";
pub const STATE: &str = "/cli/voice/transcripts";
pub const SUBSCRIBE: &str = "/cli/voice/transcripts/subscribe";
pub const READS: &str = "/cli/voice/transcripts/reads";

pub const GET_ROUTES: [&str; 4] = [LIST, TRANSCRIPT, STATE, READS];
pub const POST_ROUTES: [&str; 1] = [SUBSCRIBE];
pub const MAX_BODY: usize = 16 * 1024;
/// Default and largest pages.
pub const LIMIT_DEFAULT: u32 = 50;
pub const TRANSCRIPT_LIMIT_DEFAULT: u32 = 200;
/// The notifier reads at most this many pages a pass.
const NOTIFY_PAGES: usize = 10;

pub fn is_route(p: &str) -> bool {
    GET_ROUTES.contains(&p) || POST_ROUTES.contains(&p)
}

pub fn is_post_route(p: &str) -> bool {
    POST_ROUTES.contains(&p)
}

/// Does any agent on this server hold the switch? The K2 recording notice
/// then says "Agents on this server may read the transcript."
pub fn any_reader() -> bool {
    store::any_reader()
}

fn error(status: u16, code: &str, hint: impl Into<String>) -> CliResponse {
    let hint = hint.into();
    let line = match status {
        400 => "400 Bad Request",
        401 => "401 Unauthorized",
        403 => "403 Forbidden",
        404 => "404 Not Found",
        405 => "405 Method Not Allowed",
        409 => "409 Conflict",
        502 => "502 Bad Gateway",
        _ => "503 Service Unavailable",
    };
    let body = json!({ "ok": false, "code": code, "hint": hint, "error": { "code": code, "hint": hint } });
    CliResponse { status: line, content_type: "application/json", body: body.to_string() }
}

fn ok(v: Value) -> CliResponse {
    CliResponse::ok_json(v.to_string())
}

// ── who's asking ─────────────────────────────────────────────────────

/// The caller as these routes see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reader {
    /// The bare owner token: lists, never reads the text.
    OwnerToken,
    /// A person's own login.
    Login { username: String, role: Role },
    /// An agent's (or a K2 terminal's) passport of `workspace_id`.
    Agent { workspace_id: String, handle: String },
    /// An App pass, an API key, or no token.
    Refused(u16, &'static str),
}

impl Reader {
    pub fn of(who: &Who) -> Reader {
        match who {
            Who::Owner => Reader::OwnerToken,
            Who::Login { username, role } => Reader::Login { username: username.clone(), role: *role },
            Who::Agent { project_id, .. } | Who::Shell { project_id, .. } => {
                Reader::Agent { workspace_id: project_id.clone(), handle: crate::compute_routes::ws_handle(project_id) }
            }
            Who::Wrong => Reader::Refused(403, "app_pass_not_allowed"),
            Who::Invalid => Reader::Refused(401, "unauthenticated"),
        }
    }

    fn label(&self) -> String {
        match self {
            Reader::OwnerToken => "owner".into(),
            Reader::Login { username, .. } => format!("login:{username}"),
            Reader::Agent { handle, .. } => format!("agent:{handle}"),
            Reader::Refused(..) => "-".into(),
        }
    }

    fn manages(&self) -> bool {
        match self {
            Reader::OwnerToken => true,
            Reader::Login { role, .. } => *role >= Role::Admin,
            _ => false,
        }
    }
}

fn refused(r: &Reader) -> Option<CliResponse> {
    match r {
        Reader::Refused(401, _) => Some(error(401, "unauthenticated", "invalid or missing token")),
        Reader::Refused(_, code) => Some(error(403, code, "transcripts are for this server's agents and its owner and Admins, never App passes or API keys")),
        _ => None,
    }
}

/// The refusal an agent without the switch gets: the walls map adds the
/// card line (`k2 permission request voice.transcripts-read`).
pub fn switch_off(handle: &str) -> CliResponse {
    error(
        403,
        store::OFF_CODE,
        format!(
            "{handle} can't read call transcripts: \"Read call transcripts\" is off for it (Settings → Workspaces → {handle} → Agent). Only the server's owner turns it on."
        ),
    )
}

/// May `r` list the K5 rooms' recordings (ids, rooms, times, no text)?
pub fn may_list(r: &Reader, chat_manager: bool, reader_switch: bool) -> Result<(), CliResponse> {
    if let Some(x) = refused(r) {
        return Err(x);
    }
    match r {
        _ if r.manages() => Ok(()),
        Reader::Agent { .. } if reader_switch || chat_manager => Ok(()),
        Reader::Agent { handle, .. } => Err(switch_off(handle)),
        _ => Err(error(403, "owner_or_admin_only", "the rooms' recordings are the owner's and Admins' (members read a transcript in the room's chat)")),
    }
}

/// May `r` read a K5 room's transcript text?
pub fn may_read(r: &Reader, reader_switch: bool) -> Result<(), CliResponse> {
    if let Some(x) = refused(r) {
        return Err(x);
    }
    match r {
        Reader::OwnerToken => Err(error(
            403,
            "owner_token_cannot_read",
            "the bare owner token never reads a transcript (agents can read that token): read it from your own owner or Admin login, or turn on \"Read call transcripts\" for an agent",
        )),
        Reader::Login { role, .. } if *role >= Role::Admin => Ok(()),
        Reader::Agent { .. } if reader_switch => Ok(()),
        Reader::Agent { handle, .. } => Err(switch_off(handle)),
        _ => Err(error(403, "owner_or_admin_only", "transcripts are read by the owner, Admins, and agents the owner allowed")),
    }
}

// ── params ───────────────────────────────────────────────────────────

fn param<'a>(p: &'a HashMap<String, String>, k: &str) -> Option<&'a str> {
    p.get(k).map(|s| s.trim()).filter(|s| !s.is_empty())
}

/// Sew's ids here are decimal.
fn dec(s: &str) -> bool {
    !s.is_empty() && s.len() <= 19 && !s.starts_with('0') && s.bytes().all(|b| b.is_ascii_digit())
}

fn limit_of(p: &HashMap<String, String>, default: u32) -> Result<u32, CliResponse> {
    match param(p, "limit") {
        None => Ok(default),
        Some(l) => l
            .parse::<u32>()
            .ok()
            .filter(|n| (1..=wire::LIMIT_MAX).contains(n))
            .ok_or_else(|| error(400, "usage", format!("limit is 1 to {}", wire::LIMIT_MAX))),
    }
}

fn after_of(p: &HashMap<String, String>) -> Result<Option<String>, CliResponse> {
    match param(p, "after") {
        None => Ok(None),
        Some(a) if dec(a) => Ok(Some(a.to_string())),
        Some(_) => Err(error(400, "usage", "after is the next cursor a page gave you")),
    }
}

// ── Sew ──────────────────────────────────────────────────────────────

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, CliResponse> {
    tokio::task::spawn_blocking(f).await.map_err(|e| CliResponse::internal_error(format!("worker join: {e}")))
}

async fn chat_client() -> Result<MgmtClient, CliResponse> {
    match blocking(|| hook::get_target(hook::CHAT_KIND)).await? {
        Ok(Some(t)) if t.enabled => Ok(MgmtClient::new(&t.socket, &t.token_file)),
        Ok(_) => Err(error(503, "chat_unavailable", "Chat / Voice Hosting isn't on for this server")),
        Err(e) => Err(error(503, "chat_unavailable", e)),
    }
}

/// The K5 Space, its rooms, and Sew's flag: everything a read checks first.
struct K5 {
    space: String,
    /// channel id → (slug, name).
    rooms: HashMap<String, (String, String)>,
}

fn k5() -> Result<K5, CliResponse> {
    let flags = hook::sew_recording_features().map_err(|e| error(503, "chat_unavailable", e))?;
    if !flags.mgmt_recording_reads {
        return Err(error(409, "sew_feature_missing", "this server's chat can't hand out recordings yet (it needs Sew 0.9.0: mgmt_recording_reads)"));
    }
    let view = hook::admin_space_view().map_err(|e| error(503, "voice_unavailable", format!("K2 could not read the rooms: {e}")))?;
    let space = view
        .space
        .as_ref()
        .and_then(|s| s.space_id.clone())
        .ok_or_else(|| error(409, "voice_space_not_ready", "this server's voice rooms aren't made yet (k2 voice status)"))?;
    let rooms = view
        .rooms
        .iter()
        .filter_map(|r| r.channel_id.clone().map(|ch| (ch, (r.slug.clone(), r.name.clone().unwrap_or_else(|| r.desired_name())))))
        .collect();
    Ok(K5 { space, rooms })
}

/// A room by slug, number, name or channel id → its channel id.
fn room_channel(k5: &K5, key: &str) -> Result<String, CliResponse> {
    let k = key.trim();
    let slug = k.parse::<u32>().ok().map(|n| format!("room-{n}"));
    k5.rooms
        .iter()
        .find(|(ch, (s, n))| ch.as_str() == k || s == k || slug.as_deref() == Some(s.as_str()) || n.eq_ignore_ascii_case(k))
        .map(|(ch, _)| ch.clone())
        .ok_or_else(|| error(404, "room_not_found", format!("no room '{k}' on this server (k2 voice rooms lists them)")))
}

fn sew_refusal(r: &MgmtResponse) -> CliResponse {
    let reason = r.error().and_then(|e| e.error.detail_str("reason").map(str::to_string));
    match (r.status, reason.as_deref()) {
        (404, _) => error(404, "not_found", "no such recording in this server's voice rooms"),
        (403, Some("sew_actor_admin_required")) => {
            error(502, "failed", "this server's chat didn't take K2's owner read (sew_actor_admin_required): run k2 hostchat repair")
        }
        (s, _) if s >= 500 => error(503, "chat_unavailable", "this server's chat didn't answer; try again"),
        _ => error(502, "failed", format!("chat refused the read: {}", r.error_line())),
    }
}

async fn get(client: &MgmtClient, path: &str) -> Result<MgmtResponse, CliResponse> {
    let r = client
        .get_as(path, sew_wire::OWNER_ACTOR)
        .await
        .map_err(|e| error(503, "chat_unavailable", format!("this server's chat didn't answer: {e}")))?;
    if !r.is_success() {
        return Err(sew_refusal(&r));
    }
    Ok(r)
}

fn recording_json(r: &sew_wire::RecordingSummary, k5: &K5) -> Value {
    let room = k5.rooms.get(&r.channel_id);
    json!({
        "id": r.id,
        "room": room.map(|(s, _)| s.clone()),
        "roomName": room.map(|(_, n)| n.clone()),
        "channelId": r.channel_id,
        "state": r.state,
        "trigger": r.trigger,
        "startedBy": r.started_by,
        "startedAt": r.started_at,
        "stoppedAt": r.stopped_at,
        "transcriptReady": r.transcript_final(),
        "transcriptState": r.transcript_state,
        "people": r.consents.iter().map(|c| json!({ "externalId": c.external_id, "consent": c.consent })).collect::<Vec<_>>(),
    })
}

/// One recording, only if it is in the K5 Space (`not_found` otherwise).
async fn k5_recording(client: &MgmtClient, k5: &K5, id: &str) -> Result<sew_wire::RecordingSummary, CliResponse> {
    let r = get(client, &sew_wire::compliance_recording_path(id)).await?;
    let rec: sew_wire::RecordingSummary =
        serde_json::from_slice(&r.body).map_err(|e| error(502, "failed", format!("chat's answer wasn't a recording ({e})")))?;
    if rec.space_id != k5.space {
        return Err(error(404, "not_found", "no such recording in this server's voice rooms"));
    }
    Ok(rec)
}

// ── the routes ───────────────────────────────────────────────────────

async fn list(r: &Reader, params: &HashMap<String, String>) -> CliResponse {
    let (manager, switch) = match r {
        Reader::Agent { workspace_id, .. } => {
            let ws = workspace_id.clone();
            match blocking(move || (k2_core::workspace::settings::agents_can_manage_chat(&ws), store::can_read(&ws))).await {
                Ok(x) => x,
                Err(e) => return e,
            }
        }
        _ => (false, false),
    };
    if let Err(e) = may_list(r, manager, switch) {
        return e;
    }
    let (limit, after) = match (limit_of(params, LIMIT_DEFAULT), after_of(params)) {
        (Ok(l), Ok(a)) => (l, a),
        (Err(e), _) | (_, Err(e)) => return e,
    };
    let k5 = match blocking(k5).await {
        Ok(Ok(k)) => k,
        Ok(Err(e)) | Err(e) => return e,
    };
    let only = match param(params, "room").map(|room| room_channel(&k5, room)) {
        None => None,
        Some(Ok(ch)) => Some(ch),
        Some(Err(e)) => return e,
    };
    let client = match chat_client().await {
        Ok(c) => c,
        Err(e) => return e,
    };
    let resp = match get(&client, &sew_wire::compliance_space_recordings_path(&k5.space, after.as_deref(), limit)).await {
        Ok(r) => r,
        Err(e) => return e,
    };
    let page: sew_wire::RecordingsPage = match serde_json::from_slice(&resp.body) {
        Ok(p) => p,
        Err(e) => return error(502, "failed", format!("chat's answer wasn't a recordings page ({e})")),
    };
    let items: Vec<Value> =
        page.recordings.iter().filter(|x| only.as_ref().is_none_or(|ch| &x.channel_id == ch)).map(|x| recording_json(x, &k5)).collect();
    ok(json!({ "ok": true, "recordings": items, "next": page.next, "canRead": (matches!(r, Reader::Login { .. }) && r.manages()) || switch }))
}

async fn transcript(r: &Reader, params: &HashMap<String, String>) -> CliResponse {
    let switch = match r {
        Reader::Agent { workspace_id, .. } => {
            let ws = workspace_id.clone();
            match blocking(move || store::can_read(&ws)).await {
                Ok(x) => x,
                Err(e) => return e,
            }
        }
        _ => false,
    };
    if let Err(e) = may_read(r, switch) {
        return e;
    }
    let Some(id) = param(params, "id").filter(|i| dec(i)).map(str::to_string) else {
        return error(400, "usage", "name the recording: id=<its id> (k2 voice recordings lists them)");
    };
    let (limit, after) = match (limit_of(params, TRANSCRIPT_LIMIT_DEFAULT), after_of(params)) {
        (Ok(l), Ok(a)) => (l, a),
        (Err(e), _) | (_, Err(e)) => return e,
    };
    let k5 = match blocking(k5).await {
        Ok(Ok(k)) => k,
        Ok(Err(e)) | Err(e) => return e,
    };
    let client = match chat_client().await {
        Ok(c) => c,
        Err(e) => return e,
    };
    let rec = match k5_recording(&client, &k5, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let resp = match get(&client, &sew_wire::compliance_transcript_path(&id, after.as_deref(), limit)).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let page: sew_wire::TranscriptPage = match serde_json::from_slice(&resp.body) {
        Ok(p) => p,
        Err(e) => return error(502, "failed", format!("chat's answer wasn't a transcript ({e})")),
    };
    let lines: Vec<Value> = page
        .turns
        .iter()
        .map(|t| json!({ "id": t.id, "speaker": speaker_name(&t.speaker_external_id), "speakerExternalId": t.speaker_external_id, "startMs": t.start_ms, "endMs": t.end_ms, "text": t.text }))
        .collect();
    // The log comes first: a read that can't be logged isn't handed out.
    let (label, ws, n) = (r.label(), match r {
        Reader::Agent { workspace_id, .. } => Some(workspace_id.clone()),
        _ => None,
    }, lines.len());
    let rid = id.clone();
    match blocking(move || store::log_read(&label, ws.as_deref(), &rid, n)).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => return error(503, "audit_unavailable", format!("K2 couldn't log the read, so it doesn't hand it out: {e}")),
        Err(e) => return e,
    }
    ok(json!({
        "ok": true,
        "recording": recording_json(&rec, &k5),
        "lines": lines,
        "next": page.next,
        "logged": true,
    }))
}

/// A speaker's name as people say it: `owner`, a login, or the id.
fn speaker_name(ext: &str) -> String {
    match sew_wire::ExternalId::parse(ext) {
        Ok(id) => super::occupancy::person_name(Some(&id), "", &super::occupancy::login_of_uuid),
        Err(_) => ext.to_string(),
    }
}

fn state_view(only: Option<&str>) -> Result<Value, CliResponse> {
    let readers = store::readers().map_err(|e| error(503, "store_unavailable", e))?;
    let subs = store::subscriptions().map_err(|e| error(503, "store_unavailable", e))?;
    let view = hook::admin_space_view().map_err(|e| error(503, "voice_unavailable", format!("K2 could not read the rooms: {e}")))?;
    let rooms: Vec<Value> = view
        .rooms
        .iter()
        .filter(|r| r.channel_id.is_some())
        .filter(|r| only.is_none_or(|o| r.slug == o || r.channel_id.as_deref() == Some(o)))
        .map(|r| {
            let ch = r.channel_id.clone().unwrap_or_default();
            let agents: Vec<String> =
                subs.iter().filter(|s| s.channel_id == ch).map(|s| crate::compute_routes::ws_handle(&s.workspace_id)).collect();
            json!({ "room": r.slug, "name": r.name.clone().unwrap_or_else(|| r.desired_name()), "channelId": ch, "sendTo": agents })
        })
        .collect();
    Ok(json!({
        "ok": true,
        "readers": readers.iter().map(|w| crate::compute_routes::ws_handle(w)).collect::<Vec<_>>(),
        "readerIds": readers,
        "agentReaders": !readers.is_empty(),
        "noticeLine": (!readers.is_empty()).then_some(store::AGENT_READERS_LINE),
        "rooms": rooms,
    }))
}

fn subscribe(r: &Reader, b: &Value) -> CliResponse {
    if let Some(x) = refused(r) {
        return x;
    }
    let (Some(room), Some(agent)) = (b.get("room").and_then(Value::as_str), b.get("agent").and_then(Value::as_str)) else {
        return error(400, "usage", "send {\"room\": \"room-1\", \"agent\": \"<workspace>\", \"on\": true|false}");
    };
    let on = b.get("on").and_then(Value::as_bool).unwrap_or(true);
    let Some((_, target)) = crate::comms::resolve_target_project(agent) else {
        return error(404, "workspace_not_found", format!("no workspace '{agent}' on this server"));
    };
    let handle = crate::compute_routes::ws_handle(&target);
    match r {
        _ if r.manages() => {}
        Reader::Agent { workspace_id, .. } if *workspace_id == target => {}
        Reader::Agent { .. } => {
            return error(403, "own_workspace_only", "an agent subscribes only itself; the owner or an Admin subscribes another agent")
        }
        _ => return error(403, "owner_or_admin_only", "the owner or an Admin chooses who gets a room's transcripts"),
    }
    if on && !store::can_read(&target) {
        return match r {
            Reader::Agent { .. } => switch_off(&handle),
            _ => error(
                409,
                "agent_cannot_read",
                format!("{handle} can't read call transcripts, so a note would point at nothing: the owner turns on \"Read call transcripts\" for it first"),
            ),
        };
    }
    let k5 = match k5_rooms_only() {
        Ok(k) => k,
        Err(e) => return e,
    };
    let channel = match room_channel(&k5, room) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match store::subscribe(&channel, &target, on, &r.label()) {
        Ok(changed) => match state_view(Some(&channel)) {
            Ok(mut v) => {
                v["changed"] = json!(changed);
                v["agent"] = json!(handle);
                v["on"] = json!(on);
                ok(v)
            }
            Err(e) => e,
        },
        Err(e) => error(503, "store_unavailable", e),
    }
}

/// The rooms, without Sew's read flag (subscribing works before Sew 0.9.0).
fn k5_rooms_only() -> Result<K5, CliResponse> {
    let view = hook::admin_space_view().map_err(|e| error(503, "voice_unavailable", format!("K2 could not read the rooms: {e}")))?;
    let rooms = view
        .rooms
        .iter()
        .filter_map(|r| r.channel_id.clone().map(|ch| (ch, (r.slug.clone(), r.name.clone().unwrap_or_else(|| r.desired_name())))))
        .collect();
    Ok(K5 { space: view.space.and_then(|s| s.space_id).unwrap_or_default(), rooms })
}

fn reads(r: &Reader, params: &HashMap<String, String>) -> CliResponse {
    if let Some(x) = refused(r) {
        return x;
    }
    if !r.manages() {
        return error(403, "owner_or_admin_only", "the transcript read log is the owner's and Admins'");
    }
    let limit = match limit_of(params, LIMIT_DEFAULT) {
        Ok(l) => l,
        Err(e) => return e,
    };
    let id = param(params, "id");
    match store::reads(id, limit) {
        Ok(rows) => ok(json!({ "ok": true, "reads": rows })),
        Err(e) => error(503, "store_unavailable", e),
    }
}

/// [`handle`] for the cell socket, whose verbs run on the blocking pool
/// (`cell_server::dispatch_cell_verb`): the same handler, driven by the
/// daemon's runtime from this thread. Off any runtime (a unit test calling
/// the cell dispatch directly) it gets a small runtime of its own.
pub fn handle_blocking(path: &str, who: &Who, is_post: bool, params: &HashMap<String, String>, body: &[u8]) -> CliResponse {
    let fut = handle(path, who, is_post, params, body);
    match tokio::runtime::Handle::try_current() {
        Ok(h) => h.block_on(fut),
        Err(_) => match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(rt) => rt.block_on(fut),
            Err(e) => CliResponse::internal_error(format!("runtime: {e}")),
        },
    }
}

/// One `/cli/voice/recordings|transcript|transcripts*` call.
pub async fn handle(path: &str, who: &Who, is_post: bool, params: &HashMap<String, String>, body: &[u8]) -> CliResponse {
    if !k2_core::voice_preview::enabled() {
        return super::routes::dark();
    }
    if is_post != is_post_route(path) {
        return CliResponse::method_not_allowed();
    }
    let r = Reader::of(who);
    match path {
        LIST => list(&r, params).await,
        TRANSCRIPT => transcript(&r, params).await,
        STATE => {
            if let Some(x) = refused(&r) {
                return x;
            }
            if let Reader::Login { role, .. } = &r {
                if *role < Role::Admin {
                    return error(403, "owner_or_admin_only", "who reads the rooms' transcripts is the owner's and Admins' to see");
                }
            }
            let room = param(params, "room").map(str::to_string);
            match blocking(move || state_view(room.as_deref())).await {
                Ok(Ok(v)) => ok(v),
                Ok(Err(e)) | Err(e) => e,
            }
        }
        READS => {
            let p = params.clone();
            match blocking(move || reads(&r, &p)).await {
                Ok(v) | Err(v) => v,
            }
        }
        SUBSCRIBE => {
            let b: Value = if body.iter().all(u8::is_ascii_whitespace) {
                json!({})
            } else {
                match serde_json::from_slice(body) {
                    Ok(v @ Value::Object(_)) => v,
                    _ => return error(400, "usage", "the body is a JSON object"),
                }
            };
            match blocking(move || subscribe(&r, &b)).await {
                Ok(v) | Err(v) => v,
            }
        }
        _ => CliResponse::not_found(),
    }
}

// ── after the call: notes to subscribed agents ───────────────────────

/// One note to send: `workspace_id` hears about `recording`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Due {
    pub workspace_id: String,
    pub recording_id: String,
    pub channel_id: String,
}

fn started_secs(at: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(at).ok().map(|t| t.timestamp())
}

/// Which notes are due: a final transcript in a subscribed room, for every
/// agent subscribed to it that still holds the switch, started after the
/// agent was subscribed, and not told before. Pure.
pub fn due(
    recordings: &[sew_wire::RecordingSummary],
    subs: &[store::Subscription],
    readers: &HashSet<String>,
    told: &HashSet<(String, String)>,
) -> Vec<Due> {
    let mut out = Vec::new();
    for r in recordings.iter().filter(|r| r.transcript_final()) {
        for s in subs.iter().filter(|s| s.channel_id == r.channel_id && readers.contains(&s.workspace_id)) {
            if told.contains(&(r.id.clone(), s.workspace_id.clone())) {
                continue;
            }
            // A recording from before the subscription is old news.
            if started_secs(&r.started_at).is_none_or(|t| t < s.set_at) {
                continue;
            }
            out.push(Due { workspace_id: s.workspace_id.clone(), recording_id: r.id.clone(), channel_id: r.channel_id.clone() });
        }
    }
    out
}

/// The cursor after a page: past every leading recording whose transcript
/// is final (so a running one is read again next pass).
pub fn next_cursor(prev: Option<&str>, page: &[sew_wire::RecordingSummary]) -> Option<String> {
    let mut cur = prev.map(str::to_string);
    for r in page {
        if !r.transcript_final() {
            break;
        }
        cur = Some(r.id.clone());
    }
    cur
}

/// The note an agent gets.
pub fn note_text(recording_id: &str, room: &str, slug: &str) -> (String, String) {
    (
        format!("Call transcript ready: {room}"),
        format!(
            "A recorded call in {room} has its transcript.\n\nRead it: k2 voice transcript {recording_id}\nThis room's recordings: k2 voice recordings --room {slug}\n\nEvery read is logged. You get these notes because this room sends its transcripts to you; to stop them: k2 voice transcripts unsubscribe {slug} <your workspace>."
        ),
    )
}

/// Where notes go (tests swap it). `(workspace_id, title, body)`.
pub type NoteSink = dyn Fn(&str, &str, &str) + Send + Sync;

fn sink_slot() -> &'static std::sync::RwLock<Option<std::sync::Arc<NoteSink>>> {
    static S: std::sync::RwLock<Option<std::sync::Arc<NoteSink>>> = std::sync::RwLock::new(None);
    &S
}

/// Tests: catch the notes (`None` = the workspace inbox).
#[doc(hidden)]
pub fn set_note_sink(s: Option<std::sync::Arc<NoteSink>>) {
    *sink_slot().write().unwrap_or_else(|e| e.into_inner()) = s;
}

fn send_note(workspace_id: &str, title: &str, body: &str) {
    if let Some(s) = sink_slot().read().unwrap_or_else(|e| e.into_inner()).clone() {
        s(workspace_id, title, body);
        return;
    }
    let path = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::Project::get(&conn, workspace_id).ok().map(|p| p.path)
    };
    let Some(path) = path else { return };
    if let Err(e) = k2_core::inbox::compose(std::path::Path::new(&path), title, body, Some("normal"), Some("voice"), Some("k2-voice")) {
        k2_core::log_debug!("[voice-transcripts] note to {workspace_id}: {e}");
    }
}

/// One notifier pass: nothing while no room sends transcripts anywhere;
/// otherwise read the K5 Space's recordings from the cursor, send what is
/// due, and move the cursor past the final ones. Returns the notes sent.
pub async fn notify_once(client: &MgmtClient) -> Result<usize, String> {
    let subs = blocking(store::subscriptions).await.map_err(|e| e.body)??;
    if subs.is_empty() {
        return Ok(0);
    }
    let k5 = blocking(k5).await.map_err(|e| e.body)?.map_err(|e| e.body)?;
    let start = {
        let space = k5.space.clone();
        blocking(move || store::with_store(|c| store::cursor_in(c, &space))).await.map_err(|e| e.body)??
    };
    let mut cursor = start.clone();
    let mut after = start;
    let mut all = Vec::new();
    for _ in 0..NOTIFY_PAGES {
        let r = client
            .get_as(&sew_wire::compliance_space_recordings_path(&k5.space, after.as_deref(), wire::LIMIT_MAX), sew_wire::OWNER_ACTOR)
            .await
            .map_err(|e| e.0)?;
        if !r.is_success() {
            return Err(format!("recordings list: {}", r.error_line()));
        }
        let page: sew_wire::RecordingsPage = serde_json::from_slice(&r.body).map_err(|e| format!("recordings list: {e}"))?;
        if cursor == after {
            cursor = next_cursor(cursor.as_deref(), &page.recordings);
        }
        let more = page.next.clone();
        all.extend(page.recordings);
        match more {
            Some(n) if Some(&n) != after.as_ref() => after = Some(n),
            _ => break,
        }
    }
    let rooms = k5.rooms.clone();
    let space = k5.space.clone();
    let sent = blocking(move || -> Result<usize, String> {
        // Read before taking the store: skin.db's connection isn't re-entrant.
        let readers: HashSet<String> = store::readers()?.into_iter().collect();
        store::with_store(|c| {
            let told = store::notified_in(c)?;
            let notes = due(&all, &subs, &readers, &told);
            let now = chrono::Utc::now().timestamp();
            for d in &notes {
                let (slug, name) = rooms.get(&d.channel_id).cloned().unwrap_or_else(|| (d.channel_id.clone(), d.channel_id.clone()));
                let (title, body) = note_text(&d.recording_id, &name, &slug);
                send_note(&d.workspace_id, &title, &body);
                store::mark_notified_in(c, &d.recording_id, &d.workspace_id, now)?;
            }
            store::set_cursor_in(c, &space, cursor.as_deref())?;
            Ok(notes.len())
        })
    })
    .await
    .map_err(|e| e.body)??;
    Ok(sent)
}

/// How often the notifier looks (only while a room sends transcripts).
pub const NOTIFY_EVERY: std::time::Duration = std::time::Duration::from_secs(60);

/// The notifier's loop, for the daemon's life.
pub async fn run_notifier() {
    loop {
        tokio::time::sleep(NOTIFY_EVERY).await;
        if !k2_core::voice_preview::enabled() {
            continue;
        }
        let flags = blocking(hook::sew_recording_features).await;
        if !matches!(flags, Ok(Ok(f)) if f.mgmt_recording_reads) {
            continue;
        }
        let Ok(client) = chat_client().await else { continue };
        match notify_once(&client).await {
            Ok(0) => {}
            Ok(n) => k2_core::log_debug!("[voice-transcripts] {n} transcript note(s) sent"),
            Err(e) => k2_core::log_debug!("[voice-transcripts] notifier: {e}"),
        }
    }
}

// ── the K5 rooms' consent mode ───────────────────────────────────────

/// The body K2 sends each K5 room: declining the notice means not joining
/// (Rosson 2026-11-10: in K2's own rooms people are told, and can't stay in
/// unrecorded). Sew drops a person who doesn't answer within
/// `recording.consent_prompt_timeout_ms`.
pub fn consent_mode_body() -> Value {
    serde_json::to_value(sew_wire::MgmtChannelRecording { require_consent: sew_wire::Patch::Set(true), ..Default::default() })
        .unwrap_or_else(|_| json!({ "require_consent": true }))
}

/// Set `require_consent` on every open K5 room K2 hasn't set yet (Sew
/// 0.9.0, `app_recording`). Runs on each reconcile sweep; idempotent.
/// Returns the rooms set.
pub async fn ensure_consent_mode(client: &MgmtClient) -> Result<Vec<String>, String> {
    let (channels, done) = blocking(|| -> Result<(Vec<String>, HashSet<String>), String> {
        let view = hook::admin_space_view()?;
        let channels = view
            .rooms
            .iter()
            .filter(|r| r.state == hook::admin_space::ROOM_ACTIVE || r.state == hook::admin_space::ROOM_CLOSING)
            .filter_map(|r| r.channel_id.clone())
            .collect();
        Ok((channels, store::consent_mode_set()?))
    })
    .await
    .map_err(|e| e.body)??;
    let body = consent_mode_body().to_string();
    let mut set = Vec::new();
    for ch in channels.into_iter().filter(|c| !done.contains(c)) {
        let key = format!("k2-k5-consent-{ch}");
        let r = client.post(&sew_wire::channel_recording_path(&ch), body.as_bytes(), &key, sew_wire::OWNER_ACTOR).await.map_err(|e| e.0)?;
        if !r.is_success() {
            return Err(format!("room {ch} consent mode: {}", r.error_line()));
        }
        let c = ch.clone();
        blocking(move || store::note_consent_mode(&c)).await.map_err(|e| e.body)??;
        set.push(ch);
    }
    Ok(set)
}

#[cfg(test)]
#[path = "transcripts_tests.rs"]
mod tests;
