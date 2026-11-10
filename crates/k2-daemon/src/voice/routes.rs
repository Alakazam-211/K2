//! `/cli/voice/*` (prd-k2-voice-rooms-v1 v1.9 §9.1, §9.4; lane C).
//!
//! ```text
//! GET  /cli/voice/rooms                    rooms, who's in them, hosting (any login, owner token, passports)
//! GET  /cli/voice/status                   hosting, the Admin Space, members, holds, lane, feed (owner/Admin; agents read)
//! POST /cli/voice/settings {count, now?}   the room count; now = "Close now" (10 s notice, then archive)
//! POST /cli/voice/rooms/rename {room, name}
//! POST /cli/voice/rooms/close {room, now?} close the top room (the count drops by one)
//! POST /cli/voice/join {room, join?}       a person's own login only → callId + Sew's join answer
//! POST /cli/voice/session/leave {callId?}
//! POST /cli/voice/session/state|stats|keys/key_packages|keys/join {callId, body}
//! POST /cli/voice/moderate {room, person, action}   kick|mute|unmute|deafen|undeafen (owner/Admin login)
//! POST /cli/voice/recording/consent {recording_id, consent, notice_version?}
//!                                          a person's own login only → Sew's recording
//! POST /cli/voice/repair {}                check the K5 Space with Sew and repair it
//!                                          (owner/Admin, or an agent with Manage Chat / Voice Hosting)
//! ```
//!
//! Every route answers 404 `voice_rooms_off` while the preview is off (the
//! family is dark, like compute's). Every POST route is POST-only (`if
//! !is_post { 405 }` here, and the dispatcher's `require_post`). Floors for
//! logins live in `route_policy`; this handler classifies every caller:
//!
//! | caller | rooms | status | settings/rename/close | join, session, consent | moderate |
//! |---|---|---|---|---|---|
//! | owner token | ✓ | ✓ | ✓ | `owner_token_cannot_join` | `owner_token_cannot_moderate` |
//! | Owner/Admin login | ✓ | ✓ | ✓ | ✓ | ✓ (own Sew session) |
//! | Member login | ✓ | route floor | route floor | ✓ | `moderator_only` |
//! | agent or K2 terminal passport | ✓ | ✓ (read) | `owner_or_admin_only` | `agents_not_in_voice` | `agents_cannot_moderate` |
//! | ...whose workspace has Manage Chat / Voice Hosting | as above, and `repair` | | | | |
//! | App pass, API key | `app_pass_not_allowed` | … | … | … | … |
//! | an ended phone pairing | `device_session_ended` (403) on every route | | | | |
//!
//! A paired phone (K2 Companion) signs in with its device session, a login
//! session of the same person: it is that person's Login row above, with the
//! same answers (gap-voice-calls-mobile-v1 §11 row 4). `rooms` carries `me`
//! for a login caller: where Sew's presence lists the person, `here` (this
//! session's call) or `elsewhere` (another device or session), never claimed
//! live (`mayBeLeaving`: presence trails a killed app by up to about 45 s).
//! `session/leave` is idempotent for the session's own calls (`already`).
//!
//! `recording/consent` is the person's answer to a recording notice, sent to
//! Sew's `POST /api/v1/recordings/{id}/consent` with their own Sew session
//! (the same callers as join: a login or a paired phone). A grant must name
//! the `notice_version` the person saw; unknown keys are a 400.
//!
//! Refusals: `{"ok": false, "error": {"code", "hint", "details"?}}` (K2's
//! house shape, which `k2 voice` reads) with `code`, `hint` and `details`
//! mirrored at the top level for the desktop client's parser
//! (`voice-wire.ts`). A "retry shortly" refusal carries
//! `details.retry_after_ms`.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde_json::{json, Value};

use k2_core::connect_users::Role;
use k2_core::tool_hook::admin_space::{self, RoomRow};
use k2_core::voice_rooms;

use super::bff::{self, Forward, Person, Refusal};
use super::occupancy::{self, Occupant};
use crate::cli_response::CliResponse;
use crate::compute_routes::Who;

pub const ROOMS: &str = "/cli/voice/rooms";
pub const STATUS: &str = "/cli/voice/status";
pub const SETTINGS: &str = "/cli/voice/settings";
pub const ROOM_RENAME: &str = "/cli/voice/rooms/rename";
pub const ROOM_CLOSE: &str = "/cli/voice/rooms/close";
pub const JOIN: &str = "/cli/voice/join";
pub const LEAVE: &str = "/cli/voice/session/leave";
pub const SESSION_STATE: &str = "/cli/voice/session/state";
pub const SESSION_STATS: &str = "/cli/voice/session/stats";
pub const SESSION_KEY_PACKAGES: &str = "/cli/voice/session/keys/key_packages";
pub const SESSION_KEYS_JOIN: &str = "/cli/voice/session/keys/join";
pub const MODERATE: &str = "/cli/voice/moderate";
pub const RECORDING_CONSENT: &str = "/cli/voice/recording/consent";
/// Sew 0.9.0: the K5 rooms' recording (`required`, `startLevel`); read
/// through `GET /cli/voice/status` (`recording`).
pub const RECORDING_SETTINGS: &str = "/cli/voice/recording/settings";
pub const REPAIR: &str = "/cli/voice/repair";

pub const GET_ROUTES: &[&str] = &[ROOMS, STATUS];
pub const POST_ROUTES: &[&str] = &[
    SETTINGS,
    ROOM_RENAME,
    ROOM_CLOSE,
    JOIN,
    LEAVE,
    SESSION_STATE,
    SESSION_STATS,
    SESSION_KEY_PACKAGES,
    SESSION_KEYS_JOIN,
    MODERATE,
    RECORDING_CONSENT,
    RECORDING_SETTINGS,
    REPAIR,
];

/// Largest body (KeyPackages ride `session/keys/key_packages`).
pub const MAX_BODY: usize = 256 * 1024;
/// "Close now": people get this long before the room is archived (§3.4).
pub const CLOSE_NOTICE: Duration = Duration::from_secs(10);

pub fn is_route(p: &str) -> bool {
    GET_ROUTES.contains(&p) || POST_ROUTES.contains(&p)
}

pub fn is_post_route(p: &str) -> bool {
    POST_ROUTES.contains(&p)
}

// ── answers ──────────────────────────────────────────────────────────

fn status_line(code: u16) -> &'static str {
    match code {
        200 => "200 OK",
        400 => "400 Bad Request",
        401 => "401 Unauthorized",
        403 => "403 Forbidden",
        404 => "404 Not Found",
        405 => "405 Method Not Allowed",
        409 => "409 Conflict",
        410 => "410 Gone",
        413 => "413 Payload Too Large",
        422 => "422 Unprocessable Entity",
        429 => "429 Too Many Requests",
        500 => "500 Internal Server Error",
        503 => "503 Service Unavailable",
        _ => "502 Bad Gateway",
    }
}

fn ok(v: Value) -> CliResponse {
    CliResponse::ok_json(v.to_string())
}

/// A refusal in K2's shape, mirrored at the top level (module doc).
pub fn refuse(r: &Refusal) -> CliResponse {
    let mut details = r.details.clone();
    if let Some(s) = r.retry_after_secs {
        let mut d = details.take().unwrap_or_else(|| json!({}));
        if let Some(o) = d.as_object_mut() {
            o.insert("retry_after_ms".into(), json!(s * 1000));
        }
        details = Some(d);
    }
    let mut err = json!({ "code": r.code, "hint": r.hint });
    let mut body = json!({ "ok": false, "code": r.code, "hint": r.hint });
    if let Some(d) = details {
        err["details"] = d.clone();
        body["details"] = d;
    }
    body["error"] = err;
    CliResponse { status: status_line(r.status), content_type: "application/json", body: body.to_string() }
}

fn no(status: u16, code: &str, hint: &str) -> CliResponse {
    refuse(&Refusal::new(status, code, hint))
}

/// The family is dark while the preview is off.
pub fn dark() -> CliResponse {
    no(
        404,
        "voice_rooms_off",
        "voice rooms are a preview: the owner turns them on (k2 settings set voiceRoomsPreview true, or K2_VOICE_ROOMS=1)",
    )
}

pub fn too_large() -> CliResponse {
    no(413, "too_large", "the body is too large")
}

// ── who's calling ────────────────────────────────────────────────────

/// The caller as these routes see them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caller {
    /// The owner daemon token (agents can read it on shared-user boxes).
    OwnerToken,
    /// A Connect login session (the token, to classify the person).
    Login { username: String, role: Role, token: String },
    /// An agent's or a K2 terminal's passport.
    Passport,
    /// A passport whose workspace has "Manage Chat / Voice Hosting"
    /// (`chat_manage`) on: a [`Caller::Passport`] that may also run
    /// `repair` (the box's chat server upkeep is that switch's).
    ChatManager,
    /// An App pass or API key.
    App,
    /// A paired phone's device session that ended (revoked, unpaired,
    /// signed out, expired, or its login changed).
    DeviceEnded,
    Invalid,
}

impl Caller {
    pub fn from_tcp(query: &str, bearer: Option<&str>, owner_token: &str) -> Caller {
        let presented = crate::routes::http::extract_token(query)
            .filter(|s| !s.is_empty())
            .or_else(|| bearer.map(str::trim).filter(|s| !s.is_empty()))
            .unwrap_or("")
            .to_string();
        Caller::from_who(crate::compute_routes::who_from_tcp(query, bearer, owner_token), presented)
    }

    /// The caller for a resolved [`Who`] (`presented`: the token, for a
    /// login and an ended phone pairing).
    pub fn from_who(who: Who, presented: String) -> Caller {
        match who {
            Who::Owner => Caller::OwnerToken,
            Who::Login { username, role } => Caller::Login { username, role, token: presented },
            Who::Agent { project_id, .. } | Who::Shell { project_id, .. } => {
                if k2_core::workspace::settings::agents_can_manage_chat(&project_id) {
                    Caller::ChatManager
                } else {
                    Caller::Passport
                }
            }
            Who::Wrong => Caller::App,
            Who::Invalid if k2_core::connect_users::token_state(&presented) == k2_core::connect_users::TokenState::DeviceEnded => {
                Caller::DeviceEnded
            }
            Who::Invalid => Caller::Invalid,
        }
    }

    fn manages(&self) -> bool {
        match self {
            Caller::OwnerToken => true,
            Caller::Login { role, .. } => *role >= Role::Admin,
            _ => false,
        }
    }
}

fn not_a_caller(c: &Caller) -> Option<CliResponse> {
    caller_refusal(c).map(|r| refuse(&r))
}

/// [`not_a_caller`] as a refusal (the kit door writes Sew's error shape).
fn caller_refusal(c: &Caller) -> Option<Refusal> {
    match c {
        Caller::App => Some(Refusal::new(403, "app_pass_not_allowed", "voice rooms are for this server's own logins, never App passes or API keys")),
        Caller::DeviceEnded => Some(bff::device_session_ended()),
        Caller::Invalid => Some(Refusal::new(401, "unauthenticated", "invalid or missing token")),
        _ => None,
    }
}

fn need_manager(c: &Caller) -> Result<(), CliResponse> {
    if let Some(r) = not_a_caller(c) {
        return Err(r);
    }
    match c {
        _ if c.manages() => Ok(()),
        Caller::Passport | Caller::ChatManager => Err(no(
            403,
            "owner_or_admin_only",
            "agents read voice rooms (k2 voice rooms, k2 voice status); the owner or an Admin changes them. Ask your human in your Thread.",
        )),
        _ => Err(no(403, "owner_or_admin_only", "only the owner or an Admin changes voice rooms")),
    }
}

/// Refuses every caller that isn't a person's own login (or paired phone):
/// the owner token, passports, App passes and bad tokens.
fn not_a_person(c: &Caller) -> Option<CliResponse> {
    person_refusal(c).map(|r| refuse(&r))
}

/// [`not_a_person`] as a refusal.
fn person_refusal(c: &Caller) -> Option<Refusal> {
    if let Some(r) = caller_refusal(c) {
        return Some(r);
    }
    match c {
        Caller::OwnerToken => Some(Refusal::new(
            403,
            "owner_token_cannot_join",
            "the bare owner token can't join a voice room (agents can read it): join from your own login on this server",
        )),
        Caller::Passport | Caller::ChatManager => Some(Refusal::new(
            403,
            "agents_not_in_voice",
            "agents don't join voice rooms (the CLI has no audio; people join from the K2 app)",
        )),
        Caller::Login { .. } => None,
        Caller::App | Caller::DeviceEnded | Caller::Invalid => Some(Refusal::new(401, "unauthenticated", "invalid or missing token")),
    }
}

/// The person behind the caller, for join and the call's routes.
fn need_person(c: &Caller, owner_token: &str) -> Result<Person, CliResponse> {
    person_of(c, owner_token).map_err(|r| refuse(&r))
}

/// The person behind the caller as join sees them, or the refusal join
/// answers. The kit door uses the same rule on the box address
/// (`voice::kit_door`).
pub(crate) fn person_of(c: &Caller, owner_token: &str) -> Result<Person, Refusal> {
    if let Some(r) = person_refusal(c) {
        return Err(r);
    }
    match c {
        Caller::Login { username, role, token } => bff::person_of_login(token, username, *role, owner_token),
        _ => Err(Refusal::new(401, "unauthenticated", "invalid or missing token")),
    }
}

fn body_json(body: &[u8]) -> Result<Value, CliResponse> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(json!({}));
    }
    match serde_json::from_slice::<Value>(body) {
        Ok(v @ Value::Object(_)) => Ok(v),
        _ => Err(no(400, "usage", "the body is a JSON object")),
    }
}

/// A string or number field as a string (`room` may be `1`).
fn field(v: &Value, k: &str) -> Option<String> {
    match v.get(k) {
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    }
}

// ── the rooms view ───────────────────────────────────────────────────

fn label() -> Option<String> {
    k2_core::tunnel::config::load()
        .ok()
        .map(|c| c.subdomain.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn close_now_slot() -> &'static Mutex<HashMap<String, i64>> {
    static S: OnceLock<Mutex<HashMap<String, i64>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Pending "Close now" times (unix ms) by room slug.
fn close_now_pending() -> HashMap<String, i64> {
    close_now_slot().lock().unwrap_or_else(|p| p.into_inner()).clone()
}

fn role_name(person: &str, external_id: Option<&str>) -> Option<&'static str> {
    if external_id == Some("k2:owner") {
        return Some("owner");
    }
    match k2_core::connect_users::role_for_user(person)? {
        Role::Owner => Some("owner"),
        Role::Admin => Some("admin"),
        Role::Member => Some("member"),
        // prd-voice-chat-only-members-v1 §6.2 (V9): explicit, no wildcard.
        Role::VoiceChat => Some("voice_chat"),
    }
}

/// One occupant as the client reads it (`voice-wire.ts` `VoiceOccupant`)
/// plus Sew's own fields.
fn occupant_json(o: &Occupant) -> Value {
    json!({
        "user": o.person,
        "name": Value::Null,
        "role": role_name(&o.person, o.external_id.as_deref()),
        "color": Value::Null,
        "muted": o.self_mute || o.server_mute,
        "deafened": o.self_deaf || o.server_deaf,
        "speaking": false,
        "memberId": o.member_id,
        "externalId": o.external_id,
        "connection": o.connection,
        "selfMute": o.self_mute,
        "selfDeaf": o.self_deaf,
        "serverMute": o.server_mute,
        "serverDeaf": o.server_deaf,
        "joinedAt": o.joined_at,
    })
}

/// One room in the list (`voice-wire.ts` `VoiceRoom` plus K2's fields).
pub fn room_json(r: &RoomRow, people: &[Occupant], hosting: &super::Hosting, closes_at: Option<i64>) -> Value {
    let closing = r.state == admin_space::ROOM_CLOSING;
    let limit = voice_rooms::ROOM_USER_LIMIT;
    let unavailable: Option<String> = if closing {
        None
    } else if hosting.state == "voice_lane_down" {
        Some(k2_core::tool_hook::sew_wire::ENGINE_MEDIA_PATH_DOWN.to_string())
    } else if hosting.state != super::HOSTING_OK && hosting.state != "voice_lane_reconnecting" {
        Some(hosting.state.clone())
    } else if people.len() as u32 >= limit {
        Some("room_full".into())
    } else {
        None
    };
    json!({
        "id": r.slug,
        "n": r.n,
        "slug": r.slug,
        "name": r.name.clone().unwrap_or_else(|| r.desired_name()),
        "channelId": r.channel_id,
        "limit": limit,
        "count": people.len(),
        "occupants": people.iter().map(occupant_json).collect::<Vec<_>>(),
        "closing": closing,
        "closesAt": closes_at,
        "unavailable": unavailable,
    })
}

/// L8: empty a room's occupant list unless `sub` is in it (its `count`
/// stays).
pub fn narrow_occupants(room: &mut Value, people: &[Occupant], sub: &str) {
    if !people.iter().any(|o| o.external_id.as_deref() == Some(sub)) {
        room["occupants"] = json!([]);
    }
}

/// The rooms that show: open or closing, with a channel.
fn shown(r: &RoomRow) -> bool {
    r.channel_id.is_some() && (r.state == admin_space::ROOM_ACTIVE || r.state == admin_space::ROOM_CLOSING)
}

fn rooms_view(c: &Caller, owner_token: &str) -> CliResponse {
    let hosting = super::hosting_state();
    let view = match k2_core::tool_hook::admin_space_view() {
        Ok(v) => v,
        Err(e) => return no(503, "voice_unavailable", &format!("K2 could not read the rooms: {e}")),
    };
    let (rev, occ) = occupancy::snapshot();
    let closes = close_now_pending();
    let empty: Vec<Occupant> = Vec::new();
    // Security review L8: a Voice & Chat only login sees who is in a room
    // only for the room it is in itself (the counts stay for every room).
    let own_sub = match c {
        Caller::Login { username, role: Role::VoiceChat, token } => {
            Some(bff::person_of_login(token, username, Role::VoiceChat, owner_token).map(|p| p.ident.sub).unwrap_or_default())
        }
        _ => None,
    };
    let rooms: Vec<Value> = view
        .rooms
        .iter()
        .filter(|r| shown(r))
        .map(|r| {
            let people = r.channel_id.as_deref().and_then(|ch| occ.get(ch)).unwrap_or(&empty);
            let mut v = room_json(r, people, &hosting, closes.get(&r.slug).copied());
            if let Some(sub) = &own_sub {
                narrow_occupants(&mut v, people, sub);
            }
            v
        })
        .collect();
    // The caller's own voice state (logins only: the owner token and
    // passports are nobody in a room).
    let me = match c {
        Caller::Login { username, role, token } => bff::person_of_login(token, username, *role, owner_token)
            .ok()
            .map(|p| bff::me_view(&p, &view.rooms, &bff::deps()))
            .unwrap_or(Value::Null),
        _ => Value::Null,
    };
    ok(json!({
        "ok": true,
        "server": label(),
        "hosting": hosting,
        "rooms": rooms,
        "me": me,
        "rev": rev,
        "canManage": c.manages(),
        "lane": super::lane::state(),
        "feed": occupancy::feed_status().state,
    }))
}

fn status_view() -> CliResponse {
    let hosting = super::hosting_state();
    let view = match k2_core::tool_hook::admin_space_view() {
        Ok(v) => v,
        Err(e) => return no(503, "voice_unavailable", &format!("K2 could not read the K2 Admin Space: {e}")),
    };
    let flags = k2_core::tool_hook::sew_voice_features().unwrap_or_default();
    let (sessions, calls) = bff::counts();
    let carried = view.space.as_ref().map(|s| s.carried.clone()).unwrap_or_default();
    let count = k2_core::app_settings::load().voice_rooms_count;
    let feed = occupancy::feed_status();
    let stale = match crate::sew_chat::install::stale_spaces() {
        Ok(s) => s,
        Err(e) => return no(503, "voice_unavailable", &format!("K2 could not read the stale Spaces: {e}")),
    };
    ok(json!({
        "ok": true,
        "server": label(),
        "preview": k2_core::voice_preview::enabled(),
        "tunnelPreview": voice_rooms::tunnel_enabled(),
        "hosting": hosting,
        "tier": super::pro_view(),
        "count": count,
        // The TURN tier voice runs now (`direct`, `passthrough`, `tunnel`);
        // null while voice isn't installed.
        "media": { "tier": crate::sew::voice::current_tier(&crate::sew::supervisor::live()) },
        "features": flags,
        "space": view.space,
        "spaceHealth": space_health(view.space.as_ref(), &feed),
        "staleSpaces": stale,
        "members": view.members,
        "rooms": view.rooms,
        "holds": view.holds,
        "lastPlan": k2_core::tool_hook::admin_space_last_summary(),
        "carriedAgents": carried,
        "lane": super::lane::state(),
        "feed": feed,
        "bff": { "sessions": sessions, "calls": calls },
        "closeNow": close_now_pending(),
        "recording": recording_view(),
    }))
}

/// The K5 rooms' recording, as K2 wants it (`GET /cli/voice/status`).
fn recording_view() -> Value {
    let flags = k2_core::tool_hook::sew_recording_features().unwrap_or_default();
    let space = k2_core::tool_hook::recording_k5_space().ok().flatten();
    let (level, required, locked) = match &space {
        Some(s) => k2_core::tool_hook::recording_space(s).unwrap_or((None, None, false)),
        None => (None, None, false),
    };
    json!({
        "spaceId": space,
        "startLevel": level,
        "required": required,
        "locked": locked,
        "flags": flags,
        // K2's desktop answers consent prompts (K-g): until then the
        // "require recording" switch stays hidden and refused.
        "consentSheet": k2_core::recording::consent_sheet(),
    })
}

/// `POST /cli/voice/recording/settings {required?, startLevel?}` (Sew
/// 0.9.0, R2): the K5 rooms' recording. `required` (record every call
/// with two or more people) is the owner's, and refused
/// `client_consent_missing` until K2's desktop can answer a consent prompt
/// (Rosson's answer 6); `startLevel` (`none | moderator | member`; the K5
/// Space has no admin level) is the owner's or an Admin's. `null` inherits
/// the org's rule.
fn recording_settings(c: &Caller, b: &Value) -> CliResponse {
    if let Err(r) = need_manager(c) {
        return r;
    }
    let required = match b.get("required") {
        None => None,
        Some(Value::Null) => Some(Value::Null),
        Some(Value::Bool(x)) => Some(json!(x)),
        Some(_) => return no(400, "usage", "required is true, false, or null to inherit the org's rule"),
    };
    let level = match b.get("startLevel") {
        None => None,
        Some(Value::Null) => Some(Value::Null),
        Some(Value::String(l)) if matches!(l.trim(), "none" | "moderator" | "member") => Some(json!(l.trim())),
        Some(_) => return no(400, "usage", "startLevel is none, moderator, member, or null to inherit (the K5 Space has no admin level)"),
    };
    if required.is_none() && level.is_none() {
        return no(400, "usage", "send {\"required\"?: true|false|null, \"startLevel\"?: \"none\"|\"moderator\"|\"member\"|null}");
    }
    let flags = k2_core::tool_hook::sew_recording_features().unwrap_or_default();
    if required.is_some() {
        if !k2_core::recording::consent_sheet() {
            return no(
                409,
                "client_consent_missing",
                "K2's desktop can't answer a recording consent prompt yet, so a room that requires recording would hold every desktop join: this switch comes with that update",
            );
        }
        if !matches!(c, Caller::OwnerToken | Caller::Login { role: Role::Owner, .. }) {
            return no(403, "owner_only", "only the server's owner requires recording in the K5 rooms");
        }
        if !flags.recording_required {
            return no(409, "sew_feature_missing", "this server's chat can't require recording yet (it needs Sew 0.9.0: recording_required)");
        }
    }
    if level.is_some() && !flags.app_recording {
        return no(409, "sew_feature_missing", "this server's chat can't record in rooms yet (it needs Sew 0.9.0: app_recording)");
    }
    let space = match k2_core::tool_hook::recording_k5_space() {
        Ok(Some(s)) => s,
        Ok(None) => return no(409, "voice_space_not_ready", "the K5 rooms' Space isn't made yet (k2 voice status)"),
        Err(e) => return no(503, "voice_unavailable", &format!("K2 could not read the K5 Space: {e}")),
    };
    let widening = required == Some(json!(true)) || level.as_ref().and_then(Value::as_str).is_some_and(|l| l != "none");
    match k2_core::tool_hook::recording_space(&space) {
        Ok((_, _, true)) if widening => {
            return no(409, "space_locked", "the K5 rooms are locked against recording (CUI/FCI): only the owner lifts it, in person");
        }
        Ok(_) => {}
        Err(e) => return no(503, "voice_unavailable", &e),
    }
    let by = match c {
        Caller::Login { username, .. } => format!("login:{username}"),
        _ => "owner".to_string(),
    };
    match k2_core::tool_hook::recording_set_space(&space, level, required, &by) {
        Ok(changed) => {
            let mut v = recording_view();
            v["ok"] = json!(true);
            v["changed"] = json!(changed);
            ok(v)
        }
        Err(e) => no(503, "voice_unavailable", &e),
    }
}

/// What `k2 voice status` says about the K5 Space: `state` none | ready |
/// checking | suspect | blocked, a `line`, and the `fix` to run. Never
/// "ready" while K2 doubts it.
pub fn space_health(sp: Option<&admin_space::SpaceRow>, feed: &occupancy::FeedStatus) -> Value {
    let Some(sp) = sp else {
        return json!({"state": "none", "line": "not made yet"});
    };
    if sp.state == admin_space::SPACE_BLOCKED {
        return json!({"state": "blocked", "line": sp.why.clone().unwrap_or_else(|| "Sew refused it".into()),
            "fix": "fix what blocked it, then run k2 voice repair"});
    }
    if sp.verify.is_some() {
        return json!({"state": "checking",
            "line": format!("K2 is asking Sew whether it still holds this Space ({}); if it's gone, K2 makes it again",
                sp.suspect.as_deref().unwrap_or("a routine check")),
            "fix": "k2 voice repair runs it now"});
    }
    let feed_gone = feed.state == "retrying"
        && feed.last_error.as_deref().is_some_and(admin_space::says_not_found)
        && feed.space_id.as_deref() == sp.space_id.as_deref();
    if feed_gone {
        return json!({"state": "suspect",
            "line": format!("Sew answered {} for this Space", feed.last_error.as_deref().unwrap_or("not_found")),
            "fix": "k2 voice repair"});
    }
    json!({"state": "ready", "line": "ready"})
}

// ── owner actions ────────────────────────────────────────────────────

/// Store the count; with `now`, live rooms above it get the 10 s notice
/// (they go `closing` at once: no one new joins), then are archived.
fn set_count(count: u32, now: bool) -> Result<Value, CliResponse> {
    if count > voice_rooms::ROOMS_MAX {
        return Err(no(400, "usage", &format!("rooms is 0-{}", voice_rooms::ROOMS_MAX)));
    }
    let live: Vec<String> = k2_core::tool_hook::admin_rooms_above(count)
        .map_err(|e| no(503, "voice_unavailable", &format!("K2 could not read the rooms: {e}")))?
        .into_iter()
        .map(|r| r.slug)
        .collect();
    k2_core::tool_hook::admin_rooms_set_count(count, false).map_err(|e| no(400, "usage", &e))?;
    let mut close_at: Option<i64> = None;
    if now && !live.is_empty() {
        let at = chrono::Utc::now().timestamp_millis() + CLOSE_NOTICE.as_millis() as i64;
        close_at = Some(at);
        {
            let mut g = close_now_slot().lock().unwrap_or_else(|p| p.into_inner());
            for s in &live {
                g.insert(s.clone(), at);
            }
        }
        let slugs = live.clone();
        let spawned = std::thread::Builder::new().name("voice-close-now".into()).spawn(move || {
            std::thread::sleep(CLOSE_NOTICE);
            // A count raised (or changed again) during the notice wins.
            if k2_core::app_settings::load().voice_rooms_count == count {
                if let Err(e) = k2_core::tool_hook::admin_rooms_set_count(count, true) {
                    k2_core::log_debug!("[voice-rooms] close now: {e}");
                }
            }
            let mut g = close_now_slot().lock().unwrap_or_else(|p| p.into_inner());
            for s in &slugs {
                if g.get(s) == Some(&at) {
                    g.remove(s);
                }
            }
            drop(g);
            occupancy::emit_rooms_changed();
            occupancy::kick();
        });
        if let Err(e) = spawned {
            return Err(no(500, "internal", &format!("close now: {e}")));
        }
    }
    occupancy::emit_rooms_changed();
    occupancy::kick();
    Ok(json!({ "ok": true, "count": count, "closing": live, "closeNowAt": close_at }))
}

fn settings(c: &Caller, b: &Value) -> CliResponse {
    if let Err(r) = need_manager(c) {
        return r;
    }
    let Some(count) = b.get("count").and_then(Value::as_u64) else {
        return no(400, "usage", "send {\"count\": 0-10, \"now\"?: true}");
    };
    let now = b.get("now").and_then(Value::as_bool).unwrap_or(false);
    match set_count(u32::try_from(count).unwrap_or(u32::MAX), now) {
        Ok(v) => ok(v),
        Err(r) => r,
    }
}

fn rename(c: &Caller, b: &Value) -> CliResponse {
    if let Err(r) = need_manager(c) {
        return r;
    }
    let (Some(room), Some(name)) = (field(b, "room"), b.get("name").and_then(Value::as_str)) else {
        return no(400, "usage", "send {\"room\": \"room-1\", \"name\": \"Standup\"}");
    };
    match k2_core::tool_hook::admin_room_rename(&room, name) {
        Ok(slug) => {
            occupancy::emit_rooms_changed();
            ok(json!({ "ok": true, "room": slug, "name": name.trim() }))
        }
        Err(e) => no(400, "usage", &e),
    }
}

fn close(c: &Caller, b: &Value) -> CliResponse {
    if let Err(r) = need_manager(c) {
        return r;
    }
    let Some(key) = field(b, "room") else {
        return no(400, "usage", "send {\"room\": \"room-2\", \"now\"?: true}");
    };
    let view = match k2_core::tool_hook::admin_space_view() {
        Ok(v) => v,
        Err(e) => return no(503, "voice_unavailable", &format!("K2 could not read the rooms: {e}")),
    };
    let room = match bff::find_room(&view.rooms, &key) {
        Ok(r) => r,
        Err(r) => return refuse(&r),
    };
    let count = k2_core::app_settings::load().voice_rooms_count;
    if room.n != count || count == 0 {
        return refuse(
            &Refusal::new(
                409,
                "close_from_top",
                format!(
                    "rooms close from the top: room-{count} closes first. To close {}, lower the count: k2 voice set rooms {}",
                    room.slug,
                    room.n.saturating_sub(1)
                ),
            )
            .details(json!({ "count": count, "room": room.slug })),
        );
    }
    let now = b.get("now").and_then(Value::as_bool).unwrap_or(false);
    match set_count(count - 1, now) {
        Ok(v) => ok(v),
        Err(r) => r,
    }
}

// ── the person's routes ──────────────────────────────────────────────

/// Which client a login token is: a paired phone (with its name) or not.
pub fn client_of(token: &str) -> bff::Client {
    use k2_core::connect_users as cu;
    if !cu::looks_like_session_token(token) {
        return bff::Client::Login;
    }
    let Some((user, sid)) = cu::validate_device_session(token) else {
        return bff::Client::Login;
    };
    let name = cu::device_sessions(Some(&user))
        .ok()
        .and_then(|v| v.into_iter().find(|d| d.id == sid))
        .map(|d| d.name)
        .unwrap_or_default();
    bff::Client::Phone { name }
}

fn join(c: &Caller, owner_token: &str, b: &Value) -> CliResponse {
    let mut p = match need_person(c, owner_token) {
        Ok(p) => p,
        Err(r) => return r,
    };
    if let Caller::Login { token, .. } = c {
        p.client = client_of(token);
    }
    let Some(room) = field(b, "room") else {
        return no(400, "usage", "send {\"room\": \"room-1\", \"join\"?: {Sew's join body}}");
    };
    let hosting = super::hosting_state();
    match hosting.state.as_str() {
        super::HOSTING_OK | "voice_lane_reconnecting" | "voice_lane_down" => {}
        "not_pro" => return no(403, "host_not_pro", "this server's voice rooms are paused (they need the Pro plan)"),
        _ => {
            return refuse(
                &Refusal::new(503, "voice_unavailable", hosting.line.clone())
                    .details(json!({ "hosting": hosting }))
                    .retry(bff::RETRY_SECS),
            )
        }
    }
    let view = match k2_core::tool_hook::admin_space_view() {
        Ok(v) => v,
        Err(e) => return no(503, "voice_unavailable", &format!("K2 could not read the rooms: {e}")),
    };
    let join_body = b.get("join").cloned().unwrap_or_else(|| json!({}));
    match bff::join(&p, &room, &join_body, &view.rooms, &bff::deps()) {
        Ok(mut v) => {
            v["server"] = json!(label());
            ok(v)
        }
        Err(r) => refuse(&r),
    }
}

fn leave(c: &Caller, owner_token: &str, b: &Value) -> CliResponse {
    let p = match need_person(c, owner_token) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let call = b.get("callId").and_then(Value::as_str);
    match bff::leave(&p, call, &bff::deps()) {
        Ok(v) => ok(v),
        Err(r) => refuse(&r),
    }
}

fn session(c: &Caller, owner_token: &str, which: Forward, b: &Value) -> CliResponse {
    let p = match need_person(c, owner_token) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let Some(call) = b.get("callId").and_then(Value::as_str) else {
        return no(400, "usage", "send {\"callId\": \"vc_…\", \"body\": {Sew's body}}");
    };
    let body = b.get("body").cloned().unwrap_or_else(|| json!({}));
    match bff::forward(&p, which, call, &body, &bff::deps()) {
        Ok(v) => ok(v),
        Err(r) => refuse(&r),
    }
}

/// `POST /cli/voice/recording/consent`: the caller's class first, then the
/// body (400), then the person's own login (a phone's session included).
fn consent(c: &Caller, owner_token: &str, b: &Value) -> CliResponse {
    if let Some(r) = not_a_person(c) {
        return r;
    }
    let req = match bff::consent_body(b) {
        Ok(req) => req,
        Err(r) => return refuse(&r),
    };
    let p = match need_person(c, owner_token) {
        Ok(p) => p,
        Err(r) => return r,
    };
    match bff::recording_consent(&p, &req, &bff::deps()) {
        Ok(v) => ok(v),
        Err(r) => refuse(&r),
    }
}

fn moderate(c: &Caller, owner_token: &str, b: &Value) -> CliResponse {
    match c {
        Caller::OwnerToken => {
            return no(
                403,
                "owner_token_cannot_moderate",
                "the bare owner token doesn't moderate (Sew's audit names a person): moderate from your own owner or Admin login",
            )
        }
        Caller::Passport | Caller::ChatManager => return no(403, "agents_cannot_moderate", "agents don't moderate voice rooms"),
        _ => {}
    }
    let p = match need_person(c, owner_token) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let (Some(room), Some(who), Some(action)) = (field(b, "room"), field(b, "person"), field(b, "action")) else {
        return no(400, "usage", "send {\"room\": \"room-1\", \"person\": \"<login>|owner|<member id>\", \"action\": \"kick|mute|unmute|deafen|undeafen\"}");
    };
    let view = match k2_core::tool_hook::admin_space_view() {
        Ok(v) => v,
        Err(e) => return no(503, "voice_unavailable", &format!("K2 could not read the rooms: {e}")),
    };
    let Some(space) = view.space.as_ref().filter(|s| s.state == admin_space::SPACE_READY).and_then(|s| s.space_id.clone()) else {
        return no(503, "voice_unavailable", "the K2 Admin Space isn't ready (k2 voice status)");
    };
    let occ: BTreeMap<String, Vec<Occupant>> = occupancy::snapshot().1;
    match bff::moderate(&p, &room, &who, &action, &space, &view.rooms, &occ, &bff::deps()) {
        Ok(v) => ok(v),
        Err(r) => refuse(&r),
    }
}

// ── repair ───────────────────────────────────────────────────────────

static REPAIR_WAIT_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(30_000);

/// Tests: how long `repair` waits for the worker before it answers.
pub fn set_repair_wait_for_tests(ms: u64) {
    REPAIR_WAIT_MS.store(ms, std::sync::atomic::Ordering::Relaxed);
}

/// `POST /cli/voice/repair`: check the K5 Space with Sew (the idempotent
/// create), re-link what moved, re-make what's gone, re-add every member,
/// retry what Sew refused, and set aside agent Spaces from an earlier Sew
/// install. Waits (up to 30 s) for the worker, then says what it found and
/// fixed. Safe to run twice: a second run finds the Space there and
/// changes nothing.
fn repair(c: &Caller) -> CliResponse {
    if let Some(r) = not_a_caller(c) {
        return r;
    }
    match c {
        _ if c.manages() => {}
        Caller::ChatManager => {}
        Caller::Passport => {
            return no(
                403,
                "chat_manage_off",
                "k2 voice repair is the owner's, an Admin's, or an agent's whose workspace has \"Manage Chat / Voice Hosting\" on \
                 (the owner or an Admin runs k2 agent access set <workspace> chat_manage on). Ask your human in your Thread.",
            )
        }
        _ => return no(403, "owner_or_admin_only", "only the owner or an Admin repairs voice rooms"),
    }
    let start = match crate::voice_admin::repair_begin("k2 voice repair") {
        Ok(s) => s,
        Err(e) => return no(503, "voice_unavailable", &format!("K2 could not start the repair: {e}")),
    };
    let wait = Duration::from_millis(REPAIR_WAIT_MS.load(std::sync::atomic::Ordering::Relaxed));
    let deadline = std::time::Instant::now() + wait;
    let (view, settled) = loop {
        let v = match k2_core::tool_hook::admin_space_view() {
            Ok(v) => v,
            Err(e) => return no(503, "voice_unavailable", &format!("K2 could not read the K5 Space: {e}")),
        };
        let done = crate::voice_admin::repair_settled(&start, &v);
        if done || std::time::Instant::now() >= deadline {
            break (v, done);
        }
        std::thread::sleep(Duration::from_millis(250));
    };
    occupancy::emit_rooms_changed();
    occupancy::kick();
    match crate::voice_admin::repair_report(&start, &view, settled) {
        Ok(v) => ok(v),
        Err(e) => no(500, "internal", &format!("repair report: {e}")),
    }
}

// ── the entry point ──────────────────────────────────────────────────

/// One `/cli/voice/*` call (blocking; the dispatcher runs it on the
/// blocking pool: a held join waits up to 20 s).
pub fn handle(path: &str, caller: Caller, owner_token: &str, is_post: bool, body: &[u8]) -> CliResponse {
    if !k2_core::voice_preview::enabled() {
        return dark();
    }
    if is_post_route(path) && !is_post {
        return CliResponse::method_not_allowed();
    }
    if !is_post_route(path) && is_post {
        return CliResponse::method_not_allowed();
    }
    if GET_ROUTES.contains(&path) {
        if let Some(r) = not_a_caller(&caller) {
            return r;
        }
        return match path {
            ROOMS => rooms_view(&caller, owner_token),
            _ => match &caller {
                Caller::Login { role, .. } if *role < Role::Admin => no(403, "owner_or_admin_only", "k2 voice status is the owner's and Admins'; k2 voice rooms is everyone's"),
                _ => status_view(),
            },
        };
    }
    let b = match body_json(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    match path {
        SETTINGS => settings(&caller, &b),
        ROOM_RENAME => rename(&caller, &b),
        ROOM_CLOSE => close(&caller, &b),
        JOIN => join(&caller, owner_token, &b),
        LEAVE => leave(&caller, owner_token, &b),
        SESSION_STATE => session(&caller, owner_token, Forward::State, &b),
        SESSION_STATS => session(&caller, owner_token, Forward::Stats, &b),
        SESSION_KEY_PACKAGES => session(&caller, owner_token, Forward::KeyPackages, &b),
        SESSION_KEYS_JOIN => session(&caller, owner_token, Forward::KeysJoin, &b),
        MODERATE => moderate(&caller, owner_token, &b),
        RECORDING_CONSENT => consent(&caller, owner_token, &b),
        RECORDING_SETTINGS => recording_settings(&caller, &b),
        REPAIR => repair(&caller),
        _ => CliResponse::not_found(),
    }
}

#[cfg(test)]
mod role_name_tests {
    use super::*;

    fn code_of(r: &CliResponse) -> (String, String) {
        let v: Value = serde_json::from_str(&r.body).expect("json");
        (r.status.to_string(), v["error"]["code"].as_str().unwrap_or("").to_string())
    }

    /// PRD §8 K-c (Rosson's answer 6): the K5 rooms' `required` is refused
    /// `client_consent_missing` while K2's desktop can't answer consent (the
    /// K-g const off), for the owner too; with it on, the owner gets past it
    /// (here to Sew's flag), an Admin never.
    #[test]
    fn k5_required_waits_for_the_desktop_consent_sheet() {
        let _home = k2_core::test_env::TempHome::new();
        k2_core::recording::set_for_tests(false, false);
        for v in [json!({"required": true}), json!({"required": false}), json!({"required": null})] {
            let r = recording_settings(&Caller::OwnerToken, &v);
            assert_eq!(code_of(&r), ("409 Conflict".to_string(), "client_consent_missing".to_string()), "{v}: {}", r.body);
        }
        let r = recording_settings(&Caller::Passport, &json!({"required": true}));
        assert_eq!(code_of(&r).1, "owner_or_admin_only", "an agent never");
        k2_core::recording::set_for_tests(false, true);
        let admin = Caller::Login { username: "ada".into(), role: Role::Admin, token: "t".into() };
        assert_eq!(code_of(&recording_settings(&admin, &json!({"required": true}))).1, "owner_only");
        assert_eq!(code_of(&recording_settings(&Caller::OwnerToken, &json!({"required": true}))).1, "sew_feature_missing", "Sew 0.8.x");
        assert_eq!(code_of(&recording_settings(&Caller::OwnerToken, &json!({"startLevel": "admin"}))).1, "usage", "the K5 Space has no admin level");
        assert_eq!(code_of(&recording_settings(&Caller::OwnerToken, &json!({}))).1, "usage");
        k2_core::recording::set_for_tests(false, false);
        assert!(POST_ROUTES.contains(&RECORDING_SETTINGS) && !GET_ROUTES.contains(&RECORDING_SETTINGS));
    }

    /// Security review L8: a room's occupants show only to someone in it.
    #[test]
    fn occupants_narrow_to_the_viewers_own_room() {
        let me = Occupant {
            member_id: "1".into(),
            external_id: Some("k2:connect:me".into()),
            person: "me".into(),
            connection: "connected".into(),
            self_mute: false,
            self_deaf: false,
            server_mute: false,
            server_deaf: false,
            joined_at: String::new(),
        };
        let mut other = me.clone();
        other.external_id = Some("k2:connect:other".into());
        let mut room = json!({ "count": 1, "occupants": [{"user": "other"}] });
        narrow_occupants(&mut room, std::slice::from_ref(&other), "k2:connect:me");
        assert_eq!(room, json!({ "count": 1, "occupants": [] }), "not my room: the count stays, the names go");
        let mut mine = json!({ "count": 2, "occupants": [{"user": "me"}, {"user": "other"}] });
        narrow_occupants(&mut mine, &[me, other], "k2:connect:me");
        assert_eq!(mine["occupants"].as_array().map(Vec::len), Some(2), "my room shows everyone in it");
    }

    /// prd-voice-chat-only-members-v1 V9: every role has its own name, no
    /// wildcard; a Voice & Chat only occupant reads `voice_chat`.
    #[test]
    fn each_role_names_itself() {
        crate::test_support::with_temp_home(|| {
            for (name, role) in [("rnown", Role::Owner), ("rnadm", Role::Admin), ("rnmem", Role::Member), ("rnvc", Role::VoiceChat)] {
                k2_core::connect_users::add_user(name, "password123").expect("add");
                if role != Role::Member {
                    k2_core::connect_users::set_role(name, role).expect("role");
                }
                assert_eq!(role_name(name, None), Some(role.as_wire()), "{name}");
            }
            assert_eq!(role_name("whoever", Some("k2:owner")), Some("owner"));
            assert_eq!(role_name("nobody-here", None), None);
        });
    }

    /// The call door (`/_k2/voice/sew/*`, follow-up 2): its kit routes run as
    /// `person_of`'s person, and a Voice & Chat only login is one exactly as a
    /// Member is: its own `k2:connect:<uuid>` member, a login client. The
    /// engine routes take only the join's `sewcc_` call credential, which
    /// carries no K2 role at all (Sew scopes it, and both roles are Sew
    /// `member`s), so video and `local:screen` publish are Sew's decision for
    /// both alike.
    #[test]
    fn a_voice_chat_login_is_the_same_kind_of_person_as_a_member_at_the_call_door() {
        crate::test_support::with_temp_home(|| {
            const OWNER: &str = "owner-token-call-door-test";
            let mut seen = Vec::new();
            for (name, role) in [("cdmem", Role::Member), ("cdvc", Role::VoiceChat)] {
                k2_core::connect_users::add_user(name, "password123").expect("add");
                if role != Role::Member {
                    k2_core::connect_users::set_role(name, role).expect("role");
                }
                let tok = k2_core::connect_users::create_session(name);
                let caller = Caller::from_tcp("", Some(&tok), OWNER);
                let p = match person_of(&caller, OWNER) {
                    Ok(p) => p,
                    Err(r) => panic!("{name}: the call door's person rule refused it: {} {}", r.code, r.hint),
                };
                assert_eq!(p.login, name);
                assert_eq!(p.role, role);
                let id = k2_core::connect_users::user_uuid(name).expect("store").expect("an id");
                assert_eq!(p.ident.sub, format!("k2:connect:{id}"), "{name}: its own Sew member");
                seen.push(p.ident.sew_role.as_str());
            }
            assert_eq!(seen, vec!["member", "member"], "the same Sew level for both");
        });
    }
}
