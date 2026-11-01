//! The voice BFF: a person's Sew session, held by the hosting daemon
//! (prd-k2-voice-rooms-v1 v1.9 §4.4, §9.2, §9.4; vs-live V13, V16, V20).
//!
//! **Why the daemon holds it.** A chat pass never goes to a browser or over
//! the tunnel (`chat_pass::mint`). The daemon mints the person's pass
//! itself, trades it for a Sew session on Sew's loopback client listener
//! (`POST /api/v1/auth/k2`), keeps that session in memory keyed by the
//! login session's `sid`, and hands the client only what it needs to
//! connect: Sew's join answer (signalling URL, media token, e2ee init and
//! the media policy, `media.relay_only` included).
//!
//! **Whose session.** Only a person's own login on this server: a Connect
//! login (`k2:connect:<uuid>`), or the owner's linked login (`k2:owner`).
//! The bare owner token never joins or moderates (agents can read it on
//! shared-user boxes; Rosson 11-01), agents never join
//! (`agents_not_in_voice`), App users are never in the K2 Admin Space.
//!
//! **Moderation** (V20) goes through the moderator's OWN Sew session, so
//! Sew checks their level (owner and Owner/Admin logins are `moderator` in
//! the Admin Space) and Sew's audit names them.
//!
//! **Held joins.** While the voice lane reconnects a join waits up to 20 s
//! ([`super::lane::gate_join`], M11); while a level sync for the person is
//! still on its way (V13) it waits a few seconds, then answers "retry
//! shortly".
//!
//! **Sessions end with the login.** Every call re-checks the bearer
//! (`chat_pass::identity::classify`); a signed-out, kicked or removed login
//! never reaches its cached Sew session again, and K2's existing revoke
//! (`k2_sid`) ends it on Sew's side.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use k2_core::connect_users::Role;
use k2_core::tool_hook::admin_space::{self, RoomRow};
use k2_core::tool_hook::sew_wire::ExternalId;
use k2_core::tool_hook::ConnectChatGate;

use super::occupancy::Occupant;
use crate::chat_pass::identity::{PassIdentity, Provenance, Refusal as IdRefusal};

/// Sew's client routes the BFF calls (Sew 0.6.0 `sew-proto::routes`).
pub mod paths {
    pub const AUTH_K2: &str = "/api/v1/auth/k2";
    pub const ME_VOICE_LEAVE: &str = "/api/v1/me/voice/leave";
    pub const ME_VOICE_STATE: &str = "/api/v1/me/voice/state";
    pub const ME_VOICE_STATS: &str = "/api/v1/me/voice/stats";
    pub const ME_VOICE_KEY_PACKAGES: &str = "/api/v1/me/voice/keys/key_packages";
    pub const ME_VOICE_KEYS_JOIN: &str = "/api/v1/me/voice/keys/join";

    pub fn channel_join(channel_id: &str) -> String {
        format!("/api/v1/channels/{channel_id}/voice/join")
    }
    pub fn member_voice(space_id: &str, member_id: &str) -> String {
        format!("/api/v1/spaces/{space_id}/members/{member_id}/voice")
    }
    pub fn member_move(space_id: &str, member_id: &str) -> String {
        format!("/api/v1/spaces/{space_id}/members/{member_id}/voice/move")
    }
}

/// Seconds a client waits before retrying a "pending" answer.
pub const RETRY_SECS: u64 = 2;
/// A cached Sew session is renewed this close to its expiry.
const RENEW_BEFORE_SECS: i64 = 60;
/// A session whose answer names no expiry is kept this long.
const DEFAULT_SESSION_SECS: i64 = 600;

// ── refusals ─────────────────────────────────────────────────────────

/// A refused BFF call: the HTTP status, a stable code and a hint.
#[derive(Debug, Clone, PartialEq)]
pub struct Refusal {
    pub status: u16,
    pub code: String,
    pub hint: String,
    pub details: Option<Value>,
    pub retry_after_secs: Option<u64>,
}

impl Refusal {
    pub fn new(status: u16, code: &str, hint: impl Into<String>) -> Refusal {
        Refusal { status, code: code.into(), hint: hint.into(), details: None, retry_after_secs: None }
    }
    pub fn details(mut self, v: Value) -> Refusal {
        self.details = Some(v);
        self
    }
    pub fn retry(mut self, secs: u64) -> Refusal {
        self.retry_after_secs = Some(secs);
        self
    }
}

fn unreachable(e: String) -> Refusal {
    Refusal::new(503, "voice_unavailable", format!("K2 could not reach this server's Sew: {e}"))
        .details(json!({ "engine": "unreachable" }))
        .retry(RETRY_SECS)
}

/// A Sew error answer as a K2 refusal (§4.6 codes where they apply).
pub fn sew_refusal(a: &SewAnswer) -> Refusal {
    let code = a.body.pointer("/error/code").and_then(Value::as_str).unwrap_or("");
    let reason = a.body.pointer("/error/details/reason").and_then(Value::as_str).unwrap_or("");
    let hint = a.body.pointer("/error/hint").and_then(Value::as_str).unwrap_or("").to_string();
    let is = |x: &str| code == x || reason == x;
    if is("channel_closing") {
        return Refusal::new(409, "room_closing", "this room is closing: the people in it stay, no one new joins");
    }
    if is("channel_full") {
        return Refusal::new(409, "room_full", if hint.is_empty() { "this room is full".to_string() } else { hint });
    }
    if code == "voice_unavailable" {
        let details = a.body.pointer("/error/details").cloned().unwrap_or(Value::Null);
        let why = details.get("media_path_reason").and_then(Value::as_str).map(str::to_string);
        let h = match why {
            Some(w) => format!("voice on this server is unavailable ({w})"),
            None => "voice on this server is restarting; try again in a moment".to_string(),
        };
        return Refusal::new(503, "voice_unavailable", h).details(details).retry(RETRY_SECS);
    }
    if is("not_space_member") || is("not_provisioned") {
        return Refusal::new(503, "voice_member_pending", "K2 is adding this login to the K2 Admin Space; retry shortly")
            .retry(RETRY_SECS);
    }
    if is("agents_not_in_voice") {
        return Refusal::new(403, "agents_not_in_voice", "agents don't join voice rooms");
    }
    // Sew sew-56 v1.2 SW-1: client routes on the K2 Admin Space answer
    // `403 missing_permission` with these reasons. K2's own passes carry
    // no `app` claim and never name an App user, so either one means the
    // session is not a box login's: say so plainly.
    if code == "missing_permission" && reason == "app_session_not_allowed" {
        return Refusal::new(
            403,
            "app_session_not_allowed",
            "voice rooms take this server's own logins: an App's session can't join or moderate them",
        )
        .details(json!({ "sew": a.body.get("error").cloned().unwrap_or(Value::Null) }));
    }
    if code == "missing_permission" && reason == "app_user_not_allowed" {
        return Refusal::new(
            403,
            "app_user_not_allowed",
            "App users are never in the K2 Admin Space: voice rooms are for this server's owner, Admins and Members",
        )
        .details(json!({ "sew": a.body.get("error").cloned().unwrap_or(Value::Null) }));
    }
    if code == "missing_permission" {
        let why = if reason.is_empty() { "no reason given".to_string() } else { reason.to_string() };
        return Refusal::new(
            403,
            "missing_permission",
            format!("Sew refused: this login's level in the K2 Admin Space doesn't allow that ({why})"),
        )
        .details(json!({ "sew": a.body.get("error").cloned().unwrap_or(Value::Null) }));
    }
    let status = if a.status >= 400 { a.status } else { 502 };
    let code = if code.is_empty() { "sew_error" } else { code };
    let mut r = Refusal::new(status, code, if hint.is_empty() { format!("Sew answered {}", a.status) } else { hint });
    if let Some(e) = a.body.get("error") {
        r = r.details(json!({ "sew": e }));
    }
    r
}

// ── the Sew seam ─────────────────────────────────────────────────────

/// One answer from Sew's client API (the body as JSON; `Null` if none).
#[derive(Debug, Clone, PartialEq)]
pub struct SewAnswer {
    pub status: u16,
    pub body: Value,
}

impl SewAnswer {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// Sew's client API on this box (loopback). Tests install a fake.
pub trait SewApi: Send + Sync {
    /// POST `path` with `bearer` (a `sews_` session) or none.
    fn post(&self, bearer: Option<&str>, path: &str, body: &Value) -> Result<SewAnswer, String>;
}

/// The real client: Sew's loopback client listener (`sew::ops::client_addr`).
pub struct HttpSew;

fn http() -> Result<&'static reqwest::blocking::Client, String> {
    static C: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    if let Some(c) = C.get() {
        return Ok(c);
    }
    let c = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    Ok(C.get_or_init(|| c))
}

impl SewApi for HttpSew {
    fn post(&self, bearer: Option<&str>, path: &str, body: &Value) -> Result<SewAnswer, String> {
        let url = format!("http://{}{path}", crate::sew::ops::client_addr());
        let mut req = http()?.post(&url).header("content-type", "application/json").body(body.to_string());
        if let Some(b) = bearer {
            req = req.bearer_auth(b);
        }
        let r = req.send().map_err(|e| format!("{path}: {e}"))?;
        let status = r.status().as_u16();
        let text = r.text().map_err(|e| format!("{path}: reading the answer: {e}"))?;
        let body = if text.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).unwrap_or_else(|_| json!({ "raw": text.chars().take(200).collect::<String>() }))
        };
        Ok(SewAnswer { status, body })
    }
}

/// A pass minter: the person's identity → a signed chat pass.
pub type MintFn = dyn Fn(&PassIdentity) -> Result<String, String> + Send + Sync;
/// A Connect login's member gate (`(uuid, login)`).
pub type ConnectGateFn = dyn Fn(&str, &str) -> Result<ConnectChatGate, String> + Send + Sync;
/// V13: is the person's Admin Space add or level sync still on its way?
pub type LevelPendingFn = dyn Fn(&ExternalId) -> Result<bool, String> + Send + Sync;

/// What the BFF talks to. Production: [`Deps::live`].
pub struct Deps {
    pub sew: Arc<dyn SewApi>,
    pub mint: Arc<MintFn>,
    pub connect_gate: Arc<ConnectGateFn>,
    pub level_pending: Arc<LevelPendingFn>,
    /// How long a join waits for a reconnecting voice lane (M11: 20 s).
    pub lane_hold: Duration,
    /// How long a join waits for a pending member or level sync.
    pub member_wait: Duration,
}

fn live_mint(ident: &PassIdentity) -> Result<String, String> {
    let install = crate::chat_pass::link::install_id_for_mint(&crate::chat_pass::link::SewRow)?;
    let claims = crate::chat_pass::ChatPassClaims::new(&install, &ident.sub, &ident.sid, crate::chat_pass::keyring::now())?;
    crate::chat_pass::sign_chat_pass(&claims).map_err(|e| e.to_string())
}

impl Deps {
    pub fn live() -> Deps {
        Deps {
            sew: Arc::new(HttpSew),
            mint: Arc::new(live_mint),
            connect_gate: Arc::new(k2_core::tool_hook::connect_chat_gate),
            level_pending: Arc::new(k2_core::tool_hook::admin_level_pending),
            lane_hold: super::lane::HOLD_MAX,
            member_wait: Duration::from_secs(8),
        }
    }
}

fn seam() -> &'static RwLock<Option<Arc<Deps>>> {
    static S: RwLock<Option<Arc<Deps>>> = RwLock::new(None);
    &S
}

/// The deps in use.
pub fn deps() -> Arc<Deps> {
    if let Some(d) = seam().read().unwrap_or_else(|p| p.into_inner()).clone() {
        return d;
    }
    static LIVE: OnceLock<Arc<Deps>> = OnceLock::new();
    LIVE.get_or_init(|| Arc::new(Deps::live())).clone()
}

/// Tests: swap the deps (`None` = live). Never called in production.
#[doc(hidden)]
pub fn set_deps(d: Option<Arc<Deps>>) {
    *seam().write().unwrap_or_else(|p| p.into_inner()) = d;
}

// ── the person ───────────────────────────────────────────────────────

/// A person on this server, from their own login session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    pub ident: PassIdentity,
    pub ext: ExternalId,
    pub login: String,
    pub role: Role,
}

impl Person {
    /// Owner and Owner/Admin logins moderate (their Admin Space level is
    /// `moderator`; Sew checks it again).
    pub fn moderator(&self) -> bool {
        self.ext == ExternalId::K2Owner || self.role >= Role::Admin
    }
}

/// The person behind a Connect login session `token` (`login`, `role`).
pub fn person_of_login(token: &str, login: &str, role: Role, owner_token: &str) -> Result<Person, Refusal> {
    let ident = match crate::chat_pass::identity::classify(token, owner_token) {
        Ok(i) => i,
        Err(IdRefusal::Unavailable(e)) => {
            return Err(Refusal::new(503, "voice_unavailable", format!("K2's user store is unavailable: {e}")).retry(RETRY_SECS))
        }
        Err(_) => return Err(Refusal::new(401, "unauthenticated", "that login session is unknown, expired or revoked")),
    };
    if ident.provenance != Provenance::ConnectLogin {
        return Err(Refusal::new(403, "person_login_required", "join and moderate from a person's own login on this server"));
    }
    let ext = ExternalId::parse(&ident.sub).map_err(|e| Refusal::new(503, "voice_unavailable", format!("identity: {e}")))?;
    Ok(Person { ident, ext, login: login.to_string(), role })
}

// ── Sew sessions and calls ───────────────────────────────────────────

#[derive(Debug, Clone)]
struct Sess {
    token: String,
    expires_at: i64,
}

#[derive(Debug, Clone)]
struct Call {
    sid: String,
    channel_id: String,
    room: String,
}

fn sessions() -> &'static Mutex<HashMap<String, Sess>> {
    static S: OnceLock<Mutex<HashMap<String, Sess>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

fn calls() -> &'static Mutex<HashMap<String, Call>> {
    static C: OnceLock<Mutex<HashMap<String, Call>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

fn new_call_id() -> String {
    format!("vc_{}", uuid::Uuid::new_v4().simple())
}

/// How many Sew sessions and calls the BFF holds (`k2 voice status`).
pub fn counts() -> (usize, usize) {
    let s = sessions().lock().unwrap_or_else(|p| p.into_inner()).len();
    let c = calls().lock().unwrap_or_else(|p| p.into_inner()).len();
    (s, c)
}

/// Drop every session and call (tests; the preview turned off).
pub fn forget_all() {
    sessions().lock().unwrap_or_else(|p| p.into_inner()).clear();
    calls().lock().unwrap_or_else(|p| p.into_inner()).clear();
}

/// Wait out a "pending" answer: `check` until it's false or `max` passes.
fn wait_until_clear(max: Duration, mut check: impl FnMut() -> Result<bool, String>) -> Result<bool, String> {
    let start = Instant::now();
    loop {
        if !check()? {
            return Ok(true);
        }
        if start.elapsed() >= max {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(250).min(max.saturating_sub(start.elapsed())).max(Duration::from_millis(1)));
    }
}

/// SW2: a Connect login's Sew member must exist before its pass is any use.
fn member_gate(p: &Person, d: &Deps) -> Result<(), Refusal> {
    let ExternalId::K2Connect(uuid) = &p.ext else { return Ok(()) };
    let mut last = ConnectChatGate::Pending;
    let ready = wait_until_clear(d.member_wait, || {
        last = (d.connect_gate)(uuid, &p.login)?;
        Ok(matches!(last, ConnectChatGate::Pending))
    })
    .map_err(|e| Refusal::new(503, "chat_unavailable", format!("K2 could not check this login's chat account: {e}")))?;
    match last {
        ConnectChatGate::Ready => Ok(()),
        ConnectChatGate::Pending if !ready => Err(Refusal::new(503, "chat_member_pending", "K2 is adding this login to chat; retry shortly").retry(RETRY_SECS)),
        ConnectChatGate::Pending => Ok(()),
        ConnectChatGate::Blocked(why) => Err(Refusal::new(503, "chat_member_blocked", format!("this login's chat account is stuck: {why}"))),
        ConnectChatGate::Disabled => Err(Refusal::new(403, "no_chat_grant", "this login's chat access is switched off (grant kind chat)")),
        ConnectChatGate::Off => Err(Refusal::new(503, "chat_unavailable", "Chat / Voice Hosting is not enabled on this server")),
    }
}

/// The person's Sew session token: cached, or a fresh pass traded for one.
fn session(p: &Person, d: &Deps, fresh: bool) -> Result<String, Refusal> {
    let now = now_secs();
    {
        let mut g = sessions().lock().unwrap_or_else(|e| e.into_inner());
        if fresh {
            g.remove(&p.ident.sid);
        } else if let Some(s) = g.get(&p.ident.sid) {
            if s.expires_at > now + RENEW_BEFORE_SECS {
                return Ok(s.token.clone());
            }
        }
    }
    member_gate(p, d)?;
    let pass = (d.mint)(&p.ident).map_err(|e| {
        k2_core::log_debug!("[voice-rooms] pass for {}: {e}", p.ident.actor);
        Refusal::new(503, "chat_unavailable", "Chat / Voice Hosting is not enabled or not reachable on this server")
    })?;
    let a = d.sew.post(None, paths::AUTH_K2, &json!({ "pass": pass })).map_err(unreachable)?;
    if !a.ok() {
        return Err(sew_refusal(&a));
    }
    let token = a
        .body
        .get("token")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| unreachable("auth/k2 answered without a session token".into()))?
        .to_string();
    let expires_at = a
        .body
        .get("expires_at")
        .and_then(Value::as_str)
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .map(|t| t.timestamp())
        .unwrap_or(now + DEFAULT_SESSION_SECS);
    let mut g = sessions().lock().unwrap_or_else(|e| e.into_inner());
    g.retain(|_, s| s.expires_at > now);
    g.insert(p.ident.sid.clone(), Sess { token: token.clone(), expires_at });
    Ok(token)
}

/// One Sew call as the person; a 401 (the session ran out) renews once.
fn as_person(p: &Person, d: &Deps, path: &str, body: &Value) -> Result<SewAnswer, Refusal> {
    let tok = session(p, d, false)?;
    let a = d.sew.post(Some(&tok), path, body).map_err(unreachable)?;
    if a.status != 401 {
        return Ok(a);
    }
    let tok = session(p, d, true)?;
    d.sew.post(Some(&tok), path, body).map_err(unreachable)
}

// ── rooms ────────────────────────────────────────────────────────────

/// A room by `3`, `room-3`, its channel id, or its name.
pub fn find_room<'a>(rooms: &'a [RoomRow], key: &str) -> Result<&'a RoomRow, Refusal> {
    let k = key.trim();
    let not_found = || Refusal::new(404, "room_not_found", format!("no room '{k}' on this server (k2 voice rooms lists them)"));
    if k.is_empty() {
        return Err(Refusal::new(400, "usage", "name a room: 1, room-1, its channel id or its name"));
    }
    let hit = if let Some(n) = k.parse::<u32>().ok().filter(|n| (1..=k2_core::voice_rooms::ROOMS_MAX).contains(n)) {
        rooms.iter().find(|r| r.n == n)
    } else {
        rooms
            .iter()
            .find(|r| r.slug == k || r.channel_id.as_deref() == Some(k))
            .or_else(|| rooms.iter().find(|r| r.desired_name().eq_ignore_ascii_case(k)))
    };
    hit.ok_or_else(not_found)
}

/// The room's channel id when it takes joins.
pub fn open_channel(r: &RoomRow) -> Result<&str, Refusal> {
    match (r.state.as_str(), r.channel_id.as_deref()) {
        (admin_space::ROOM_ACTIVE, Some(id)) => Ok(id),
        (admin_space::ROOM_CLOSING, _) => Err(Refusal::new(409, "room_closing", format!("{} is closing: no one new joins", r.desired_name()))),
        _ => Err(Refusal::new(404, "room_not_open", format!("{} isn't open on this server right now", r.desired_name()))),
    }
}

fn room_json(r: &RoomRow) -> Value {
    // `id` is what the join takes (lane D's voice-wire.ts): the slug, which
    // never changes.
    json!({ "id": r.slug, "n": r.n, "slug": r.slug, "name": r.name.clone().unwrap_or_else(|| r.desired_name()), "channelId": r.channel_id })
}

// ── join, leave, the call's own routes, moderation ───────────────────

/// `POST /cli/voice/join {room, join?}`: `join` is Sew's join body as is
/// (`self_mute`, `self_deaf`, `e2ee`, `recording_notice`).
pub fn join(p: &Person, room_key: &str, join_body: &Value, rooms: &[RoomRow], d: &Deps) -> Result<Value, Refusal> {
    if !join_body.is_object() {
        return Err(Refusal::new(400, "usage", "join is Sew's join body, a JSON object ({} for defaults)"));
    }
    let room = find_room(rooms, room_key)?;
    let channel = open_channel(room)?.to_string();
    let held_ms = match super::lane::gate_join(d.lane_hold) {
        super::lane::Gate::Go { held_ms } => held_ms,
        super::lane::Gate::Refuse { state, held_ms } => {
            return Err(Refusal::new(
                503,
                "voice_unavailable",
                format!("voice on this server is unavailable ({}): its network lane is down", state.reason_text()),
            )
            .details(json!({
                "engine": k2_core::tool_hook::sew_wire::ENGINE_MEDIA_PATH_DOWN,
                "media_path_reason": state.reason_text(),
                "lane": state,
                "heldMs": held_ms,
            }))
            .retry(RETRY_SECS));
        }
    };
    let synced = wait_until_clear(d.member_wait, || (d.level_pending)(&p.ext))
        .map_err(|e| Refusal::new(503, "voice_unavailable", format!("K2 could not check this login's room level: {e}")))?;
    if !synced {
        return Err(Refusal::new(503, "voice_member_pending", "K2 is still updating this login's level in the K2 Admin Space; retry shortly")
            .retry(RETRY_SECS));
    }
    let a = as_person(p, d, &paths::channel_join(&channel), join_body)?;
    if !a.ok() {
        return Err(sew_refusal(&a));
    }
    let call_id = new_call_id();
    {
        let mut g = calls().lock().unwrap_or_else(|e| e.into_inner());
        // One call per person: a join elsewhere is a move.
        g.retain(|_, c| c.sid != p.ident.sid);
        g.insert(call_id.clone(), Call { sid: p.ident.sid.clone(), channel_id: channel.clone(), room: room.slug.clone() });
    }
    let relay_only = a.body.pointer("/media/relay_only").and_then(Value::as_bool).unwrap_or(false);
    Ok(json!({
        "ok": true,
        "callId": call_id,
        "room": room_json(room),
        "relayOnly": relay_only,
        "heldMs": held_ms,
        // Sew's join answer as is (url, token, e2ee, media.relay_only …).
        "join": a.body,
    }))
}

fn own_call(p: &Person, call_id: &str) -> Result<Call, Refusal> {
    let g = calls().lock().unwrap_or_else(|e| e.into_inner());
    match g.get(call_id) {
        Some(c) if c.sid == p.ident.sid => Ok(c.clone()),
        _ => Err(Refusal::new(404, "call_not_found", "no call with that id for this login (join again)")),
    }
}

/// `POST /cli/voice/session/leave {callId?}`: leave the person's call.
/// With no `callId` it leaves whatever call the person is in.
pub fn leave(p: &Person, call_id: Option<&str>, d: &Deps) -> Result<Value, Refusal> {
    let room = match call_id {
        Some(id) => Some(own_call(p, id)?.room),
        None => None,
    };
    let a = as_person(p, d, paths::ME_VOICE_LEAVE, &json!({}))?;
    calls().lock().unwrap_or_else(|e| e.into_inner()).retain(|_, c| c.sid != p.ident.sid);
    if !a.ok() {
        return Err(sew_refusal(&a));
    }
    Ok(json!({ "ok": true, "left": true, "room": room }))
}

/// The call's own Sew routes the BFF forwards (§9.1 allowlist).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Forward {
    /// Self mute / deafen (`/me/voice/state`).
    State,
    /// Upload KeyPackages (`/me/voice/keys/key_packages`).
    KeyPackages,
    /// The key join after a move (`/me/voice/keys/join`).
    KeysJoin,
    /// One call-stats sample (`/me/voice/stats`).
    Stats,
}

impl Forward {
    pub fn path(self) -> &'static str {
        match self {
            Forward::State => paths::ME_VOICE_STATE,
            Forward::KeyPackages => paths::ME_VOICE_KEY_PACKAGES,
            Forward::KeysJoin => paths::ME_VOICE_KEYS_JOIN,
            Forward::Stats => paths::ME_VOICE_STATS,
        }
    }
}

/// `POST /cli/voice/session/<state|keys/key_packages|keys/join|stats>
/// {callId, body}`: Sew's body as is, for the person's own call only.
pub fn forward(p: &Person, which: Forward, call_id: &str, body: &Value, d: &Deps) -> Result<Value, Refusal> {
    own_call(p, call_id)?;
    if !body.is_object() {
        return Err(Refusal::new(400, "usage", "body is Sew's request body, a JSON object"));
    }
    let a = as_person(p, d, which.path(), body)?;
    if !a.ok() {
        return Err(sew_refusal(&a));
    }
    Ok(json!({ "ok": true, "sew": a.body }))
}

/// Moderation actions (§9.1 `POST /cli/voice/moderate`).
pub const ACTIONS: [&str; 5] = ["kick", "mute", "unmute", "deafen", "undeafen"];

/// An occupant of `channel` by login name, `owner`, member id or external id.
pub fn find_occupant<'a>(occupants: &'a BTreeMap<String, Vec<Occupant>>, channel: &str, who: &str) -> Option<&'a Occupant> {
    let w = who.trim();
    occupants.get(channel)?.iter().find(|o| {
        o.person.eq_ignore_ascii_case(w) || o.member_id == w || o.external_id.as_deref() == Some(w)
    })
}

/// `POST /cli/voice/moderate {room, person, action}` through the
/// moderator's own Sew session (V20).
#[allow(clippy::too_many_arguments)]
pub fn moderate(
    p: &Person,
    room_key: &str,
    who: &str,
    action: &str,
    space_id: &str,
    rooms: &[RoomRow],
    occupants: &BTreeMap<String, Vec<Occupant>>,
    d: &Deps,
) -> Result<Value, Refusal> {
    if !ACTIONS.contains(&action) {
        return Err(Refusal::new(400, "usage", format!("action is one of {}", ACTIONS.join(", "))));
    }
    if !p.moderator() {
        return Err(Refusal::new(403, "moderator_only", "only the owner and Owner or Admin logins moderate voice rooms"));
    }
    let room = find_room(rooms, room_key)?;
    let Some(channel) = room.channel_id.as_deref() else {
        return Err(Refusal::new(404, "room_not_open", format!("{} isn't open on this server right now", room.desired_name())));
    };
    let Some(o) = find_occupant(occupants, channel, who) else {
        return Err(Refusal::new(404, "not_in_room", format!("{who} isn't in {} (k2 voice rooms shows who is)", room.desired_name())));
    };
    let (path, body) = match action {
        "kick" => (paths::member_move(space_id, &o.member_id), json!({ "channel_id": null })),
        "mute" => (paths::member_voice(space_id, &o.member_id), json!({ "server_mute": true })),
        "unmute" => (paths::member_voice(space_id, &o.member_id), json!({ "server_mute": false })),
        "deafen" => (paths::member_voice(space_id, &o.member_id), json!({ "server_deaf": true })),
        _ => (paths::member_voice(space_id, &o.member_id), json!({ "server_deaf": false })),
    };
    let a = as_person(p, d, &path, &body)?;
    if !a.ok() {
        return Err(sew_refusal(&a));
    }
    Ok(json!({
        "ok": true,
        "action": action,
        "room": room_json(room),
        "person": o.person,
        "memberId": o.member_id,
        "by": p.login,
        "sew": a.body,
    }))
}
