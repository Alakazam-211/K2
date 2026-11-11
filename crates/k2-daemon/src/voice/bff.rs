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
//!
//! **Phones** (gap-voice-calls-mobile-v1 §11 row 4). A paired phone's device
//! session is a Connect login session of the same person, so it joins under
//! exactly that person's rules (same room level, caps and refusals); it only
//! has its own `sid`, so its own Sew session and call. An ended device
//! session is refused `device_session_ended` (pair again).
//!
//! **One call per person.** Sew keeps one voice state per member: a join
//! from any of the person's clients replaces their call on the others. K2
//! keeps a short record of every call that ended ([`ENDED_KEEP`]), so a
//! leave of a call that already ended (left before, moved, joined from
//! another device, or dropped by Sew) answers `left: true, already: true`
//! and never reaches Sew, where it would end the person's newer call.
//! A call id that was never this login session's is still `call_not_found`.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use k2_core::connect_users::Role;
use k2_core::tool_hook::admin_space::{self, RoomRow};
use k2_core::tool_hook::sew_wire::{self, ExternalId};
use k2_core::tool_hook::ConnectChatGate;

use super::occupancy::{self, Occupant};
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
    /// The person's answer to a recording notice (Sew 0.7.x `RECORDING_CONSENT`).
    pub fn recording_consent(recording_id: &str) -> String {
        format!("/api/v1/recordings/{recording_id}/consent")
    }
    /// Record (Sew 0.9.0 `CHANNEL_RECORDING_START`).
    pub fn recording_start(channel_id: &str) -> String {
        format!("/api/v1/channels/{channel_id}/recording/start")
    }
    /// Stop (Sew 0.9.0 `RECORDING_STOP`).
    pub fn recording_stop(recording_id: &str) -> String {
        format!("/api/v1/recordings/{recording_id}/stop")
    }
    /// The channel as the person sees it (its `my_perms`).
    pub fn channel(channel_id: &str) -> String {
        format!("/api/v1/channels/{channel_id}")
    }
}

/// Seconds a client waits before retrying a "pending" answer.
pub const RETRY_SECS: u64 = 2;
/// How long Sew's voice presence can still list a client that vanished (a
/// killed app). Sew's timing (the phone gap list, row 13): the media room
/// drops it after about 20 s (15 s without packets, then 5 s of cleanup),
/// then Sew's 20 s reconnect window runs, then its 1 s admit sweep. Sew
/// measured the drop at 40.9 s; 45 s leaves headroom over that. K2's
/// occupancy follower long-polls, so it adds no fixed delay of its own.
pub const PRESENCE_LAG_SECS: u64 = 45;
/// How long an ended call is remembered, so its leave stays a success.
pub const ENDED_KEEP: Duration = Duration::from_secs(3600);
/// Ended calls remembered at most (oldest dropped first).
pub const ENDED_MAX: usize = 4096;
/// The occupancy feed state that means presence is current.
pub const FEED_LIVE: &str = "live";

/// Why a call had already ended (`why` on an `already: true` leave).
pub mod ended {
    /// This login session left it before.
    pub const LEFT: &str = "already_left";
    /// This login session joined another room (a move).
    pub const MOVED: &str = "moved";
    /// The person joined from another device or session.
    pub const JOINED_ELSEWHERE: &str = "joined_elsewhere";
    /// The person left from another device or session.
    pub const LEFT_ELSEWHERE: &str = "left_elsewhere";
    /// Sew had already dropped the person (the app closed or lost its link).
    pub const SEW_DROPPED: &str = "sew_dropped";
    /// Sew lists the person in another room now.
    pub const IN_ANOTHER_ROOM: &str = "in_another_room";
}
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
    // Sew 0.7.3: a recorded room asks each joiner first, and no answer is
    // never a yes. Sew's details pass through unchanged (`reason`,
    // `consent_prompt`, `recording`, `auto`): the native engine reads them as
    // `ConsentRequired` and holds the join for the person's answer.
    if reason == "recording_consent_required" {
        let details = a.body.pointer("/error/details").cloned().unwrap_or(Value::Null);
        let h = if hint.is_empty() { "this room is being recorded: answer its recording notice to join".to_string() } else { hint };
        return Refusal::new(409, "recording_consent_required", h).details(details);
    }
    // Sew 0.7.2: a join's `origin` must be the session App's. K2's voice rooms
    // have no App, so a body naming one always lands here; K2's clients send none.
    if is("origin_not_listed") {
        return Refusal::new(422, "origin_not_listed", "voice rooms take no origin: leave it out of the join")
            .details(json!({ "sew": a.body.get("error").cloned().unwrap_or(Value::Null) }));
    }
    if is("channel_full") {
        return Refusal::new(409, "room_full", if hint.is_empty() { "this room is full".to_string() } else { hint });
    }
    // sew-56 v1.3 SW-11: the join/move refusal is a 503 whose
    // `details.reason` is `voice_unavailable` (branch on the reason, not the
    // code); `media_path_refusal` reads the media-path-down case.
    if reason == sew_wire::reasons::VOICE_UNAVAILABLE || code == sew_wire::reasons::VOICE_UNAVAILABLE {
        let mut details = a.body.pointer("/error/details").cloned().unwrap_or_else(|| json!({}));
        let api: Option<sew_wire::ApiError> = a.body.get("error").cloned().and_then(|e| serde_json::from_value(e).ok());
        let why = api.as_ref().and_then(|e| sew_wire::media_path_refusal(a.status, e)).filter(|w| !w.is_empty());
        if let Some(o) = details.as_object_mut() {
            o.entry("reason").or_insert_with(|| json!(sew_wire::reasons::VOICE_UNAVAILABLE));
        }
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
    /// GET `path` with `bearer` (the recording controls read the channel's
    /// `my_perms`). A fake that never reads answers this.
    fn get(&self, bearer: Option<&str>, path: &str) -> Result<SewAnswer, String> {
        let _ = bearer;
        Err(format!("{path}: this Sew client sends no GET"))
    }
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

fn answer_of(path: &str, r: reqwest::blocking::Response) -> Result<SewAnswer, String> {
    let status = r.status().as_u16();
    let text = r.text().map_err(|e| format!("{path}: reading the answer: {e}"))?;
    let body = if text.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&text).unwrap_or_else(|_| json!({ "raw": text.chars().take(200).collect::<String>() }))
    };
    Ok(SewAnswer { status, body })
}

impl SewApi for HttpSew {
    fn get(&self, bearer: Option<&str>, path: &str) -> Result<SewAnswer, String> {
        let url = format!("http://{}{path}", crate::sew::ops::client_addr());
        let mut req = http()?.get(&url);
        if let Some(b) = bearer {
            req = req.bearer_auth(b);
        }
        let r = req.send().map_err(|e| format!("{path}: {e}"))?;
        answer_of(path, r)
    }

    fn post(&self, bearer: Option<&str>, path: &str, body: &Value) -> Result<SewAnswer, String> {
        let url = format!("http://{}{path}", crate::sew::ops::client_addr());
        let mut req = http()?.post(&url).header("content-type", "application/json").body(body.to_string());
        if let Some(b) = bearer {
            req = req.bearer_auth(b);
        }
        let r = req.send().map_err(|e| format!("{path}: {e}"))?;
        answer_of(path, r)
    }
}

/// A pass minter: the person's identity → a signed chat pass.
pub type MintFn = dyn Fn(&PassIdentity) -> Result<String, String> + Send + Sync;
/// A Connect login's member gate (`(uuid, login)`).
pub type ConnectGateFn = dyn Fn(&str, &str) -> Result<ConnectChatGate, String> + Send + Sync;
/// V13: is the person's Admin Space add or level sync still on its way?
pub type LevelPendingFn = dyn Fn(&ExternalId) -> Result<bool, String> + Send + Sync;

/// Where Sew's voice presence (the SW-3 feed, [`occupancy`]) lists one
/// person. It can trail a vanished client by [`PRESENCE_LAG_SECS`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Presence {
    /// The feed's state: `live`, `retrying` (the last snapshot) or `off`.
    pub feed: String,
    /// The room's channel id and the person's entry, when listed.
    pub listed: Option<(String, Occupant)>,
}

/// The person's presence by external id.
pub type PresenceFn = dyn Fn(&ExternalId) -> Presence + Send + Sync;

/// The live presence: the occupancy follower's snapshot.
pub fn live_presence(ext: &ExternalId) -> Presence {
    let feed = occupancy::feed_status().state;
    let want = ext.to_string();
    let listed = occupancy::snapshot()
        .1
        .into_iter()
        .find_map(|(ch, people)| people.into_iter().find(|o| o.external_id.as_deref() == Some(want.as_str())).map(|o| (ch, o)));
    Presence { feed, listed }
}

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
    /// Where Sew lists a person now ([`live_presence`]).
    pub presence: Arc<PresenceFn>,
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
            presence: Arc::new(live_presence),
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

/// Which of the person's clients a login session is (never a secret).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Client {
    /// A login session (the desktop app, a browser, the CLI).
    Login,
    /// A paired phone's device session, with the phone's name.
    Phone { name: String },
}

impl Client {
    pub fn json(&self) -> Value {
        match self {
            Client::Login => json!({ "kind": "login" }),
            Client::Phone { name } => json!({ "kind": "phone", "name": name }),
        }
    }
}

/// A person on this server, from their own login session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    pub ident: PassIdentity,
    pub ext: ExternalId,
    pub login: String,
    pub role: Role,
    /// Which client this session is (the join route sets it; it only labels
    /// the call for the person's other clients).
    pub client: Client,
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
        Err(_) if k2_core::connect_users::token_state(token) == k2_core::connect_users::TokenState::DeviceEnded => {
            return Err(device_session_ended())
        }
        Err(_) => return Err(Refusal::new(401, "unauthenticated", "that login session is unknown, expired or revoked")),
    };
    if ident.provenance != Provenance::ConnectLogin {
        return Err(Refusal::new(403, "person_login_required", "join and moderate from a person's own login on this server"));
    }
    let ext = ExternalId::parse(&ident.sub).map_err(|e| Refusal::new(503, "voice_unavailable", format!("identity: {e}")))?;
    Ok(Person { ident, ext, login: login.to_string(), role, client: Client::Login })
}

/// A paired phone whose device session ended (revoked, unpaired, signed
/// out, expired, or its login changed): the phone pairs again.
pub fn device_session_ended() -> Refusal {
    Refusal::new(
        403,
        "device_session_ended",
        "this phone's pairing ended (revoked, signed out, unused for 30 days, 180 days old, or its login's password changed): pair it again with a new code",
    )
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
    /// The person's external id (one call per person in Sew).
    person: String,
    channel_id: String,
    room: String,
    client: Client,
    at: Instant,
}

/// A call that ended, kept [`ENDED_KEEP`] so its leave stays a success.
#[derive(Debug, Clone)]
struct Ended {
    sid: String,
    channel_id: String,
    room: String,
    why: &'static str,
    at: Instant,
}

fn sessions() -> &'static Mutex<HashMap<String, Sess>> {
    static S: OnceLock<Mutex<HashMap<String, Sess>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

fn calls() -> &'static Mutex<HashMap<String, Call>> {
    static C: OnceLock<Mutex<HashMap<String, Call>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

fn ended_calls() -> &'static Mutex<HashMap<String, Ended>> {
    static E: OnceLock<Mutex<HashMap<String, Ended>>> = OnceLock::new();
    E.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Move the live calls `pick` names a reason for to the ended record.
/// Lock order: the calls (held by the caller), then the ended record.
fn end_calls(g: &mut HashMap<String, Call>, mut pick: impl FnMut(&Call) -> Option<&'static str>) {
    let gone: Vec<(String, &'static str)> = g.iter().filter_map(|(id, c)| pick(c).map(|why| (id.clone(), why))).collect();
    if gone.is_empty() {
        return;
    }
    let mut e = ended_calls().lock().unwrap_or_else(|p| p.into_inner());
    let now = Instant::now();
    e.retain(|_, x| now.duration_since(x.at) < ENDED_KEEP);
    for (id, why) in gone {
        if let Some(c) = g.remove(&id) {
            e.insert(id, Ended { sid: c.sid, channel_id: c.channel_id, room: c.room, why, at: now });
        }
    }
    while e.len() > ENDED_MAX {
        let Some(oldest) = e.iter().min_by_key(|(_, x)| x.at).map(|(k, _)| k.clone()) else { break };
        e.remove(&oldest);
    }
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
    ended_calls().lock().unwrap_or_else(|p| p.into_inner()).clear();
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
    cached_session(&p.ident.sid, fresh, d, || {
        member_gate(p, d)?;
        (d.mint)(&p.ident).map_err(|e| {
            k2_core::log_debug!("[voice-rooms] pass for {}: {e}", p.ident.actor);
            Refusal::new(503, "chat_unavailable", "Chat / Voice Hosting is not enabled or not reachable on this server")
        })
    })
}

/// The person's Sew session for the kit door (`kit_door`): the same cached
/// session their call join uses, so both have one scope.
pub fn person_session(p: &Person, fresh: bool) -> Result<String, Refusal> {
    session(p, &deps(), fresh)
}

/// Drop the Sew session cached under `sid` (Sew answered 401 with it).
pub fn forget_session(sid: &str) {
    sessions().lock().unwrap_or_else(|e| e.into_inner()).remove(sid);
}

/// The Sew session cached under login session `sid`, or a fresh one from
/// the pass `mint` makes (`fresh` drops the cached one first). Keys are
/// login session ids (`cu_…`, `skn_…`), so a box login and an App login
/// never share an entry.
pub fn cached_session(sid: &str, fresh: bool, d: &Deps, mint: impl FnOnce() -> Result<String, Refusal>) -> Result<String, Refusal> {
    let now = now_secs();
    {
        let mut g = sessions().lock().unwrap_or_else(|e| e.into_inner());
        if fresh {
            g.remove(sid);
        } else if let Some(s) = g.get(sid) {
            if s.expires_at > now + RENEW_BEFORE_SECS {
                return Ok(s.token.clone());
            }
        }
    }
    let pass = mint()?;
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
    g.insert(sid.to_string(), Sess { token: token.clone(), expires_at });
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
                // The same shape Sew's own refusal has (sew-56 v1.3 SW-11).
                "reason": sew_wire::reasons::VOICE_UNAVAILABLE,
                "engine": sew_wire::ENGINE_MEDIA_PATH_DOWN,
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
    if a.status == 404 {
        // The client API masks "gone" and "not yours" alike. Ask K2 to
        // check the K5 Space over mgmt (unmasked); it re-makes the room if
        // Sew lost it (voicetest 11-04).
        crate::voice_admin::suspect(&format!("Sew answered 404 to a join of {}", room.slug));
        return Err(Refusal::new(
            503,
            "voice_unavailable",
            "Sew doesn't know this room right now: K2 is checking the K5 Space and makes the room again if it's gone. \
             Try again in a minute (the owner can run k2 voice repair to do it now)",
        )
        .retry(RETRY_SECS));
    }
    if !a.ok() {
        return Err(sew_refusal(&a));
    }
    let call_id = new_call_id();
    {
        let mut g = calls().lock().unwrap_or_else(|e| e.into_inner());
        // One call per person (Sew keeps one voice state per member): a join
        // from this session elsewhere is a move, and a join from another of
        // the person's devices or sessions takes the call over from it.
        let me = p.ext.to_string();
        end_calls(&mut g, |c| {
            if c.sid == p.ident.sid {
                Some(ended::MOVED)
            } else if c.person == me {
                Some(ended::JOINED_ELSEWHERE)
            } else {
                None
            }
        });
        g.insert(
            call_id.clone(),
            Call {
                sid: p.ident.sid.clone(),
                person: me,
                channel_id: channel.clone(),
                room: room.slug.clone(),
                client: p.client.clone(),
                at: Instant::now(),
            },
        );
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

/// What a call id is to this login session.
enum Mine {
    Live(Call),
    Ended(Ended),
    NotMine,
}

fn mine(p: &Person, call_id: &str) -> Mine {
    if let Some(c) = calls().lock().unwrap_or_else(|e| e.into_inner()).get(call_id) {
        return if c.sid == p.ident.sid { Mine::Live(c.clone()) } else { Mine::NotMine };
    }
    match ended_calls().lock().unwrap_or_else(|e| e.into_inner()).get(call_id) {
        Some(e) if e.sid == p.ident.sid && e.at.elapsed() < ENDED_KEEP => Mine::Ended(e.clone()),
        _ => Mine::NotMine,
    }
}

fn left(room: Option<String>, channel: Option<String>, why: Option<&str>) -> Value {
    let mut v = json!({ "ok": true, "left": true, "already": why.is_some(), "room": room, "channelId": channel });
    if let Some(w) = why {
        v["why"] = json!(w);
    }
    v
}

/// `POST /cli/voice/session/leave {callId?}`: leave the person's call.
/// With no `callId` it leaves whatever call the person is in.
///
/// Idempotent: a call this login session had that already ended answers
/// `left: true, already: true, why` ([`ended`]) and never reaches Sew,
/// where a leave would end the person's newer call on another device. A
/// call id that was never this session's is `404 call_not_found`.
pub fn leave(p: &Person, call_id: Option<&str>, d: &Deps) -> Result<Value, Refusal> {
    let me = p.ext.to_string();
    let Some(id) = call_id else {
        let pr = (d.presence)(&p.ext);
        let had = calls().lock().unwrap_or_else(|e| e.into_inner()).values().any(|c| c.sid == p.ident.sid);
        let a = as_person(p, d, paths::ME_VOICE_LEAVE, &json!({}))?;
        // Sew leaves the member wherever they are: every call of theirs ended.
        end_calls(&mut calls().lock().unwrap_or_else(|e| e.into_inner()), |c| {
            if c.sid == p.ident.sid {
                Some(ended::LEFT)
            } else if c.person == me {
                Some(ended::LEFT_ELSEWHERE)
            } else {
                None
            }
        });
        if !a.ok() {
            return Err(sew_refusal(&a));
        }
        let already = !had && pr.feed == FEED_LIVE && pr.listed.is_none();
        return Ok(left(None, None, already.then_some(ended::SEW_DROPPED)));
    };
    let c = match mine(p, id) {
        Mine::Live(c) => c,
        Mine::Ended(e) => return Ok(left(Some(e.room), Some(e.channel_id), Some(e.why))),
        Mine::NotMine => return Err(Refusal::new(404, "call_not_found", "no call with that id for this login (join again)")),
    };
    let pr = (d.presence)(&p.ext);
    let live = pr.feed == FEED_LIVE;
    let end_this = || {
        end_calls(&mut calls().lock().unwrap_or_else(|e| e.into_inner()), |x| (x.sid == p.ident.sid).then_some(ended::LEFT));
    };
    if live && pr.listed.as_ref().is_some_and(|(ch, _)| *ch != c.channel_id) {
        // Sew has the person in another room: this call is over, and a
        // leave now would end that one.
        end_this();
        return Ok(left(Some(c.room), Some(c.channel_id), Some(ended::IN_ANOTHER_ROOM)));
    }
    let a = as_person(p, d, paths::ME_VOICE_LEAVE, &json!({}))?;
    end_this();
    if !a.ok() {
        return Err(sew_refusal(&a));
    }
    // Sew's leave is idempotent (200 when not in voice); presence says
    // whether Sew had already dropped the person.
    let already = live && pr.listed.is_none();
    Ok(left(Some(c.room), Some(c.channel_id), already.then_some(ended::SEW_DROPPED)))
}

/// The person's own voice state for the rooms list (`me`): where Sew's
/// presence lists them, and whether that call is this login session's
/// (`here`) or another device's or session's (`elsewhere`). Presence can
/// trail a closed or killed app by [`PRESENCE_LAG_SECS`], so a listed call
/// is never claimed live (`mayBeLeaving`).
pub fn me_view(p: &Person, rooms: &[RoomRow], d: &Deps) -> Value {
    let pr = (d.presence)(&p.ext);
    let mut v = json!({
        "user": p.login,
        "presence": pr.feed,
        "presenceLagSecs": PRESENCE_LAG_SECS,
        "inCall": false,
    });
    let Some((channel, o)) = pr.listed else {
        return v;
    };
    let me = p.ext.to_string();
    let (here, on) = {
        let g = calls().lock().unwrap_or_else(|e| e.into_inner());
        let held: Vec<(&String, &Call)> = g.iter().filter(|(_, c)| c.person == me && c.channel_id == channel).collect();
        let here = held.iter().find(|(_, c)| c.sid == p.ident.sid).map(|(id, _)| (*id).clone());
        let on = held.iter().filter(|(_, c)| c.sid != p.ident.sid).max_by_key(|(_, c)| c.at).map(|(_, c)| c.client.json());
        (here, on)
    };
    let room = rooms.iter().find(|r| r.channel_id.as_deref() == Some(channel.as_str()));
    let name = room.map(|r| r.name.clone().unwrap_or_else(|| r.desired_name())).unwrap_or_else(|| "a room".into());
    let hint = if here.is_some() {
        format!(
            "Sew lists you in {name} from this login. If this app isn't in the call any more, it may still be leaving (up to about {PRESENCE_LAG_SECS} s): offer to rejoin."
        )
    } else {
        format!(
            "Sew lists you in {name} from another device or session. A closed app stays listed for up to about {PRESENCE_LAG_SECS} s, so it may still be leaving. Joining here moves the call to this device."
        )
    };
    v["inCall"] = json!(true);
    v["room"] = room.map(room_json).unwrap_or(Value::Null);
    v["channelId"] = json!(channel);
    v["connection"] = json!(o.connection);
    v["joinedAt"] = json!(o.joined_at);
    v["here"] = json!(here.is_some());
    v["elsewhere"] = json!(here.is_none());
    v["callId"] = json!(here);
    // The client holding it, when K2 made that join (null for this
    // session's own call, and after a daemon restart).
    v["on"] = on.unwrap_or(Value::Null);
    v["mayBeLeaving"] = json!(true);
    v["hint"] = json!(hint);
    v
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

// ── recording consent ────────────────────────────────────────────────

/// What `POST /cli/voice/recording/consent` takes (the usage hint).
pub const CONSENT_USAGE: &str = "send {\"recording_id\": \"<the recording's id from Sew's notice>\", \"consent\": true|false, \"notice_version\": <the notice's notice_version>} (notice_version is required to grant)";

/// One answer to a recording notice: Sew's `ConsentRequest` and the
/// recording it is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsentReq {
    /// Sew's recording id, a decimal string (`1 ..= i64::MAX`).
    pub recording_id: String,
    /// Grant (true), or decline or withdraw (false).
    pub consent: bool,
    /// The notice version the person saw. Required to grant, so a grant
    /// always names the notice it answers (Sew then refuses a stale one).
    pub notice_version: Option<u32>,
}

fn consent_usage(why: impl Into<String>) -> Refusal {
    Refusal::new(400, "usage", format!("{}: {CONSENT_USAGE}", why.into()))
}

/// Read `{recording_id, consent, notice_version?}`. Unknown keys are refused
/// (Sew's own body refuses them too), so a misspelt `notice_version` never
/// drops the notice check without a word.
pub fn consent_body(b: &Value) -> Result<ConsentReq, Refusal> {
    let Some(o) = b.as_object() else { return Err(consent_usage("the body is a JSON object")) };
    if let Some(k) = o.keys().find(|k| !matches!(k.as_str(), "recording_id" | "consent" | "notice_version")) {
        return Err(consent_usage(format!("unknown field {k}")));
    }
    let raw = match o.get("recording_id") {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Number(n)) => n.to_string(),
        _ => return Err(consent_usage("recording_id is missing")),
    };
    let id = match raw.parse::<u64>() {
        Ok(n) if raw.bytes().all(|c| c.is_ascii_digit()) && n >= 1 && n <= i64::MAX as u64 => n,
        _ => return Err(consent_usage("recording_id is Sew's id, a whole number from 1 (as a string)")),
    };
    let Some(consent) = o.get("consent").and_then(Value::as_bool) else {
        return Err(consent_usage("consent is true or false"));
    };
    let notice_version = match o.get("notice_version") {
        None | Some(Value::Null) => None,
        Some(v) => match v.as_u64().and_then(|n| u32::try_from(n).ok()) {
            Some(n) => Some(n),
            None => return Err(consent_usage("notice_version is the notice's whole number")),
        },
    };
    if consent && notice_version.is_none() {
        return Err(consent_usage("a grant names the notice the person saw: send its notice_version"));
    }
    Ok(ConsentReq { recording_id: id.to_string(), consent, notice_version })
}

/// A Sew refusal of a consent answer, in K2's words.
fn consent_refusal(a: &SewAnswer) -> Refusal {
    let reason = a.body.pointer("/error/details/reason").and_then(Value::as_str).unwrap_or("");
    let code = a.body.pointer("/error/code").and_then(Value::as_str).unwrap_or("");
    let sew = a.body.get("error").cloned().unwrap_or(Value::Null);
    let status = if a.status >= 400 { a.status } else { 502 };
    match reason {
        "recording_not_active" => {
            Refusal::new(status, "recording_not_active", "that recording isn't running any more: there is nothing to answer")
                .details(json!({ "sew": sew }))
        }
        "not_in_voice" => Refusal::new(
            status,
            "not_in_voice",
            "you aren't connected to that recording's call: consent is answered from inside the call",
        )
        .details(json!({ "sew": sew })),
        // Sew's answer when the notice changed after the person saw it: show
        // the new one (`details.consent_prompt`) and ask again.
        "recording_consent_required" => {
            let d = a.body.pointer("/error/details").cloned().unwrap_or(Value::Null);
            Refusal::new(
                409,
                "consent_notice_changed",
                "the recording notice changed after it was shown: show the new one (details.consent_prompt) and ask again",
            )
            .details(json!({
                // The same reason Sew and the join path use, so a client
                // reading it with Sew's `ProviderError::from_sew_error` gets
                // `ConsentRequired` (and the new notice) with no special case.
                "reason": "recording_consent_required",
                "consent_prompt": d.get("consent_prompt").cloned().unwrap_or(Value::Null),
                "recording": d.get("recording").cloned().unwrap_or(Value::Null),
                "sew": sew,
            }))
        }
        "recording_unavailable" => Refusal::new(503, "recording_unavailable", "recording isn't available on this server's voice right now")
            .details(json!({ "sew": sew }))
            .retry(RETRY_SECS),
        "agents_cannot_record" => Refusal::new(403, "agents_not_in_voice", "agents don't answer recording notices"),
        _ if a.status == 404 || code == "not_found" => {
            Refusal::new(404, "recording_not_found", "no recording with that id that this login can see")
        }
        _ => sew_refusal(a),
    }
}

/// `POST /cli/voice/recording/consent`: the person's answer to a recording
/// notice, sent to Sew as them (their own Sew session, as a join is).
pub fn recording_consent(p: &Person, req: &ConsentReq, d: &Deps) -> Result<Value, Refusal> {
    let mut body = json!({ "consent": req.consent });
    if let Some(v) = req.notice_version {
        body["notice_version"] = json!(v);
    }
    let a = as_person(p, d, &paths::recording_consent(&req.recording_id), &body)?;
    if !a.ok() {
        return Err(consent_refusal(&a));
    }
    Ok(json!({ "ok": true, "recordingId": req.recording_id, "consent": req.consent, "recording": a.body }))
}

// ── Record and Stop (Sew 0.9.0, K-g) ─────────────────────────────────

/// Sew's permission bits K2 reads (`sew_proto::perms::Perms`; a channel's
/// `my_perms` is a decimal string of the u64).
pub mod perms {
    /// May start a recording (`RECORD_CALLS`, bit 23).
    pub const RECORD_CALLS: u64 = 1 << 23;
    /// May manage channels (`MANAGE_CHANNELS`, bit 32): stops anyone's
    /// recording.
    pub const MANAGE_CHANNELS: u64 = 1 << 32;
}

/// One GET as the person; a 401 (the session ran out) renews once.
fn get_as_person(p: &Person, d: &Deps, path: &str) -> Result<SewAnswer, Refusal> {
    let tok = session(p, d, false)?;
    let a = d.sew.get(Some(&tok), path).map_err(unreachable)?;
    if a.status != 401 {
        return Ok(a);
    }
    let tok = session(p, d, true)?;
    d.sew.get(Some(&tok), path).map_err(unreachable)
}

/// What the person asks of a room's recording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordingAct {
    /// Record `room` (a slug, number, channel id or name).
    Start { room: String },
    /// Stop recording `recording_id` (Sew's decimal id).
    Stop { recording_id: String },
}

/// A Sew refusal of a Record or Stop, in K2's words. Sew's own error stays
/// under `details.sew` (its `reason` is what a client branches on).
fn recording_act_refusal(a: &SewAnswer) -> Refusal {
    let reason = a.body.pointer("/error/details/reason").and_then(Value::as_str).unwrap_or("");
    let sew = a.body.get("error").cloned().unwrap_or(Value::Null);
    let status = if a.status >= 400 { a.status } else { 502 };
    let (code, hint): (&str, &str) = match reason {
        "recording_required" => ("recording_required", "this room records every call with two or more people, so its recording can't be stopped while two or more are in it"),
        "recording_active" => ("recording_active", "this room is already being recorded"),
        "recording_not_active" => ("recording_not_active", "that recording isn't running any more"),
        "not_in_voice" => ("not_in_voice", "you aren't in that room's call: Record and Stop are pressed from inside the call"),
        "recording_disabled" => ("recording_disabled", "recording is off on this server (k2 hostchat recording enable)"),
        "recording_unavailable" | "recorder_unavailable" | "transcriber_unavailable" | "recording_capacity" => {
            return Refusal::new(503, "recording_unavailable", "recording isn't available on this server's voice right now")
                .details(json!({ "sew": sew }))
                .retry(RETRY_SECS)
        }
        "cap_ceiling" => ("cap_ceiling", "your login's chat pass doesn't carry voice:record here"),
        "agents_cannot_record" => ("agents_not_in_voice", "agents don't start or stop recordings"),
        _ if a.status == 403 => ("cannot_record", "you may not start or stop a recording in this room"),
        _ if a.status == 404 => ("recording_not_found", "no recording or room with that id that this login can see"),
        _ => return sew_refusal(a),
    };
    Refusal::new(status, code, hint).details(json!({ "sew": sew }))
}

/// `POST /cli/voice/recording/start {room}` and `…/stop {recordingId}`:
/// the person's own Record or Stop, sent to Sew as them (their own Sew
/// session, as consent is). Sew decides who may; pressing Record is the
/// starter's own consent and nobody else's.
pub fn recording_act(p: &Person, act: &RecordingAct, rooms: &[RoomRow], d: &Deps) -> Result<Value, Refusal> {
    let path = match act {
        RecordingAct::Start { room } => {
            let r = find_room(rooms, room)?;
            paths::recording_start(open_channel(r)?)
        }
        RecordingAct::Stop { recording_id } => paths::recording_stop(recording_id),
    };
    let a = as_person(p, d, &path, &json!({}))?;
    if !a.ok() {
        return Err(recording_act_refusal(&a));
    }
    let what = match act {
        RecordingAct::Start { .. } => "started",
        RecordingAct::Stop { .. } => "stopped",
    };
    Ok(json!({ "ok": true, "action": what, "recording": a.body }))
}

/// `GET /cli/voice/recording/controls?room=`: what the person's own Sew
/// permissions in a room allow (Sew 0.9.0 has no `can_record` / `can_stop`
/// in its plugin until 0.9.2; the client derives the buttons from these,
/// `recording-controls.ts`).
pub fn recording_perms(p: &Person, room: &str, rooms: &[RoomRow], d: &Deps) -> Result<Value, Refusal> {
    let r = find_room(rooms, room)?;
    let channel = open_channel(r)?.to_string();
    let a = get_as_person(p, d, &paths::channel(&channel))?;
    if !a.ok() {
        return Err(sew_refusal(&a));
    }
    let bits = match a.body.get("my_perms") {
        Some(Value::String(s)) => s.trim().parse::<u64>().ok(),
        Some(Value::Number(n)) => n.as_u64(),
        _ => None,
    };
    let Some(bits) = bits else {
        return Err(Refusal::new(502, "chat_unavailable", "this server's chat answered the room without the person's permissions"));
    };
    Ok(json!({
        "ok": true,
        "room": r.slug,
        "channelId": channel,
        "myPerms": bits.to_string(),
        "recordCalls": bits & perms::RECORD_CALLS != 0,
        "manageChannels": bits & perms::MANAGE_CHANNELS != 0,
        "recordingRule": a.body.get("recording_rule").cloned().unwrap_or(Value::Null),
        "recordingRequired": a.body.get("recording_required").cloned().unwrap_or(Value::Null),
    }))
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
