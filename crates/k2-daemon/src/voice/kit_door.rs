//! Sew's web kit on the call door (`/_k2/voice/sew/`): a native shell (K2
//! Companion first) running `@sew/voice` in native mode reaches the nine Sew
//! routes the kit sends, and one event feed ([`super::kit_feed`]), as the
//! person. Spec: Sew's "K2 door for the Sew web kit"; vs-live
//! `spec-voice-door-kit-routes-v1-vs-live`.
//!
//! **Which lane.** The bearer picks it (`sew_lane::serve_door`): `sewcc_` is
//! the call engine's lane, unchanged; any other bearer on a kit route is
//! this one. Cookies never count and never cross.
//!
//! **Who.** On the box address: a person's own Connect login (a desktop
//! login or a paired phone's device session), by the rule `/cli/voice/join`
//! uses (`routes::person_of`): the owner token, passports, App passes and
//! API keys are refused with join's codes. On an App's address: an App login
//! session (`k2skn_`) of that App only. K2 runs each route with the person's
//! OWN Sew session (`sews_`, held in the daemon's memory, never handed out):
//! on the box, the session their call join uses (no `app` claim); on an App,
//! a session from that App session's own chat pass (the App's `app` claim and
//! caps, the scope the App's server has when it mints the call credential).
//! Sew checks every right; K2 never acts with anyone else's session.
//!
//! **The allowlist** ([`kit_route`]): the method and every segment exact; ids
//! are `[0-9]{1,19}`; no query except R1's `ids=` list and the recording
//! reads' one cursor. Anything else is 404 and never reaches Sew.
//!
//! **Recording reads (Sew 0.9.0, flag `app_recording`; K2 PRD
//! prd-k2-bobbin-in-apps-v1 K-f):** the 0.9.0 kit's reads use Sew's own
//! client routes, R10–R13: `GET /channels/{id}/recording` (the active one),
//! `GET /channels/{id}/recordings[?before=<id>]`, `GET /recordings/{id}` and
//! `GET /messages/{record message id}/replies[?after=<id>]` (the transcript is
//! that message's thread; Sew checks the person's rights on each, B14). With
//! the flag, R3's answer also keeps `recording_rule` and `recording_required`
//! (the kit hides Stop in a required room). Without it every one of them is
//! 404 here, as before, and R3 is trimmed as before. App sessions get every kit route, R4–R7 (recording,
//! member mute, deafen and move) too: Sew checks the person's own rights on
//! each (the channel's recording permission and consent, the Space's
//! moderation permissions), and its answer comes back unchanged. An App that
//! wants finer rules sets them in its own code.
//!
//! **What crosses.** To Sew: K2's own request line, `Authorization: Bearer
//! sews_…`, the JSON body re-serialized (a JSON object, at most
//! [`MAX_BODY`]), and only `accept`, `idempotency-key` and `if-none-match`.
//! Back: Sew's status and body unchanged (R3's 200 trimmed to `{id,
//! recording_audio_days}`, all the kit reads), with only `content-type`,
//! `retry-after`, `etag`, `cache-control`, `idempotent-replayed` and
//! `x-ratelimit-*`.
//!
//! **Defence in depth.** R9 revokes only a device on the person's own R8
//! list. A 401 from Sew drops the cached session; a GET retries once on a
//! fresh one, a POST never. A flood guard per login session (429 with
//! `Retry-After`) on top of Sew's own limits. K2's own answers use Sew's
//! `ErrorBody` shape (`{"ok": false, "error": {code, hint, details}}`), so the
//! kit reads them as it reads Sew's.
//!
//! Tokens are never logged and never appear in an answer K2 writes.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::bff::{self, Refusal};
use crate::chat_pass::identity::{Provenance, Refusal as IdRefusal};

/// The kit's request bodies are a few small fields.
pub const MAX_BODY: usize = 16 * 1024;
/// The largest answer K2 relays (R1's 100 members fit many times over).
pub const MAX_ANSWER: usize = 1024 * 1024;
/// R1 takes at most this many ids.
pub const MAX_IDS: usize = 100;
/// Seconds in K2's own `Retry-After`.
pub const RETRY_SECS: u64 = 2;
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
/// Per login session: a burst, then a steady rate (the engine lane's numbers).
pub const RATE_BURST: f64 = 60.0;
const RATE_PER_SEC: f64 = 5.0;
const RATE_TRACKED_MAX: usize = 4096;
const RATE_IDLE: Duration = Duration::from_secs(600);
const HEADER_VALUE_MAX: usize = 256;

/// Request headers that cross to Sew (besides K2's own bearer and framing).
const FORWARD_UP: &[&str] = &["accept", "idempotency-key", "if-none-match"];
/// Answer headers that come back (plus every `x-ratelimit-*`).
const FORWARD_BACK: &[&str] = &["content-type", "retry-after", "etag", "cache-control", "idempotent-replayed"];

/// The kit's routes (Sew's table R1–R9) and the feed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KitRoute {
    /// R1 `GET /members?ids=…`
    Members,
    /// R2 `POST /channels/{id}/call/decline`
    CallDecline,
    /// R3 `GET /channels/{id}`
    Channel,
    /// R4 `POST /channels/{id}/recording/start`
    RecordingStart,
    /// R5 `POST /recordings/{id}/stop`
    RecordingStop,
    /// R6 `POST /spaces/{s}/members/{m}/voice`
    MemberVoice,
    /// R7 `POST /spaces/{s}/members/{m}/voice/move`
    MemberMove,
    /// R8 `GET /me/devices`
    MyDevices,
    /// R9 `POST /devices/{id}/revoke`
    DeviceRevoke,
    /// R10 `GET /channels/{id}/recording` (Sew 0.9.0, `app_recording`)
    ChannelRecording,
    /// R11 `GET /channels/{id}/recordings[?before=<id>]` (`app_recording`)
    ChannelRecordings,
    /// R12 `GET /recordings/{id}` (`app_recording`)
    Recording,
    /// R13 `GET /messages/{id}/replies[?after=<id>]`: a recording's
    /// transcript thread (`app_recording`)
    TranscriptReplies,
    /// `GET /_k2/voice/sew/feed`, the event feed (WebSocket).
    Feed,
}

impl KitRoute {
    pub fn is_post(self) -> bool {
        matches!(
            self,
            KitRoute::CallDecline
                | KitRoute::RecordingStart
                | KitRoute::RecordingStop
                | KitRoute::MemberVoice
                | KitRoute::MemberMove
                | KitRoute::DeviceRevoke
        )
    }
}

/// A Sew id: a decimal of 1–19 digits.
pub fn dec(s: &str) -> bool {
    (1..=19).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit())
}

/// R1's only query: `ids=` and 1–[`MAX_IDS`] ids joined by commas.
pub fn ids_query(q: &str) -> bool {
    let Some(list) = q.strip_prefix("ids=") else { return false };
    let n = list.split(',').count();
    n <= MAX_IDS && list.split(',').all(dec)
}

/// Does this box's Sew report `app_recording` (the kit's recording reads,
/// R3's recording fields and `transcript_ready` on the feed)?
pub fn recording_kit_on() -> bool {
    k2_core::tool_hook::sew_has(k2_core::tool_hook::features::APP_RECORDING).unwrap_or(false)
}

/// A one-cursor query: absent, or exactly `<name>=<id>`.
fn cursor_query(q: Option<&str>, name: &str) -> bool {
    match q {
        None => true,
        Some(q) => q.strip_prefix(name).and_then(|r| r.strip_prefix('=')).is_some_and(dec),
    }
}

/// The kit route for a request on the door: `path` has the door prefix
/// stripped (`/api/v1/…` or `/feed`), `query` is after `?` when there is one.
/// The recording reads follow this box's Sew ([`recording_kit_on`]).
pub fn kit_route(method: &str, path: &str, query: Option<&str>) -> Option<KitRoute> {
    kit_route_with(method, path, query, recording_kit_on())
}

/// [`kit_route`] with Sew's `app_recording` given (pure).
pub fn kit_route_with(method: &str, path: &str, query: Option<&str>, recording: bool) -> Option<KitRoute> {
    if path == "/feed" {
        return (method == "GET" && query.is_none()).then_some(KitRoute::Feed);
    }
    let segs: Vec<&str> = path.strip_prefix("/api/v1/")?.split('/').collect();
    let route = match (method, segs.as_slice()) {
        ("GET", ["members"]) => KitRoute::Members,
        ("POST", ["channels", id, "call", "decline"]) if dec(id) => KitRoute::CallDecline,
        ("GET", ["channels", id]) if dec(id) => KitRoute::Channel,
        ("POST", ["channels", id, "recording", "start"]) if dec(id) => KitRoute::RecordingStart,
        ("POST", ["recordings", id, "stop"]) if dec(id) => KitRoute::RecordingStop,
        ("POST", ["spaces", s, "members", m, "voice"]) if dec(s) && dec(m) => KitRoute::MemberVoice,
        ("POST", ["spaces", s, "members", m, "voice", "move"]) if dec(s) && dec(m) => KitRoute::MemberMove,
        ("GET", ["me", "devices"]) => KitRoute::MyDevices,
        ("POST", ["devices", id, "revoke"]) if dec(id) => KitRoute::DeviceRevoke,
        ("GET", ["channels", id, "recording"]) if recording && dec(id) => KitRoute::ChannelRecording,
        ("GET", ["channels", id, "recordings"]) if recording && dec(id) => KitRoute::ChannelRecordings,
        ("GET", ["recordings", id]) if recording && dec(id) => KitRoute::Recording,
        ("GET", ["messages", id, "replies"]) if recording && dec(id) => KitRoute::TranscriptReplies,
        _ => return None,
    };
    let query_ok = match (route, query) {
        (KitRoute::ChannelRecordings, q) => cursor_query(q, "before"),
        (KitRoute::TranscriptReplies, q) => cursor_query(q, "after"),
        (KitRoute::Members, Some(q)) => ids_query(q),
        (_, None) => route != KitRoute::Members,
        (_, Some(_)) => false,
    };
    query_ok.then_some(route)
}

// ── who ──────────────────────────────────────────────────────────────

/// Which address the request came in on.
#[derive(Clone)]
pub enum Door {
    /// The box's own label: a person's Connect login.
    Box { owner_token: String },
    /// An App's address (`apps::voice_door`): an App login session of it.
    App { app_id: String },
}

impl std::fmt::Debug for Door {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Door::Box { .. } => f.write_str("Box"),
            Door::App { app_id } => write!(f, "App({app_id})"),
        }
    }
}

#[derive(Clone)]
enum Who {
    Login(Box<bff::Person>),
    AppSession { bearer: String },
    StandIn,
}

/// The person behind a kit request.
#[derive(Clone)]
pub struct KitPerson {
    /// Their login session's id (`cu_…`, `skn_…`): the key of their Sew
    /// session and of K2's limits.
    pub sid: String,
    /// For logs (never a secret).
    pub actor: String,
    /// An App's person on its address (for logs: they get the same routes).
    pub app_session: bool,
    who: Who,
}

impl KitPerson {
    /// A Voice & Chat only login (re-review L7: its feed is narrowed to its
    /// own room).
    pub fn is_voice_chat(&self) -> bool {
        matches!(&self.who, Who::Login(p) if p.role == k2_core::connect_users::Role::VoiceChat)
    }

    /// A person with no K2 login behind it (tests).
    #[doc(hidden)]
    pub fn stand_in(sid: &str, app_session: bool) -> KitPerson {
        KitPerson { sid: sid.into(), actor: format!("stand-in:{sid}"), app_session, who: Who::StandIn }
    }
}

impl std::fmt::Debug for KitPerson {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "KitPerson({}, app_session: {})", self.actor, self.app_session)
    }
}

/// The person behind `bearer` at `door` (blocking).
pub type ResolveFn = dyn Fn(&Door, &str) -> Result<KitPerson, Refusal> + Send + Sync;
/// The person's Sew session token (`fresh`: drop the cached one first) (blocking).
pub type SessionFn = dyn Fn(&KitPerson, bool) -> Result<String, Refusal> + Send + Sync;

/// What the kit lane talks to. Production: [`KitCtx::live`].
pub struct KitDeps {
    pub resolve: Arc<ResolveFn>,
    pub session: Arc<SessionFn>,
    /// How often an open feed checks the login is still good.
    pub recheck: Duration,
}

/// One door's kit lane.
#[derive(Clone)]
pub struct KitCtx {
    pub door: Door,
    pub deps: Arc<KitDeps>,
}

impl KitCtx {
    pub fn live(door: Door) -> KitCtx {
        static D: OnceLock<Arc<KitDeps>> = OnceLock::new();
        let deps = D
            .get_or_init(|| {
                Arc::new(KitDeps {
                    resolve: Arc::new(live_resolve),
                    session: Arc::new(live_session),
                    recheck: Duration::from_secs(60),
                })
            })
            .clone();
        KitCtx { door, deps }
    }
}

fn identity_refusal(r: IdRefusal) -> Refusal {
    match r {
        IdRefusal::Unauthenticated => Refusal::new(401, "unauthenticated", "that login session is unknown, expired or revoked"),
        IdRefusal::NotAllowed { hint, .. } => Refusal::new(403, "credential_not_allowed", hint),
        IdRefusal::NoChatGrant { .. } => Refusal::new(403, "no_chat_grant", "this person has no chat access (grant kind chat)"),
        IdRefusal::AgentSubject { .. } => Refusal::new(403, "agent_subject", "this App login is an agent's: agents don't use calls"),
        IdRefusal::TiedPendingReset { .. } => {
            Refusal::new(403, "tied_pending_reset", crate::chat_pass::mint::TIED_PENDING_RESET_MESSAGE)
        }
        IdRefusal::NoChatCaps { .. } => Refusal::new(403, "no_chat_caps", "this person's App role holds no chat cap the App may hand out"),
        IdRefusal::Unavailable(e) => Refusal::new(503, "chat_unavailable", format!("K2's user store is unavailable: {e}")).retry(RETRY_SECS),
    }
}

fn app_service_id(app_id: &str) -> Result<String, Refusal> {
    let h = k2_core::db::shared();
    let conn = h.lock();
    match k2_core::deploy::store::get_app(&conn, app_id) {
        Ok(Some(app)) => Ok(app.service_id),
        Ok(None) => Err(Refusal::new(404, "not_found", "no such App on this server")),
        Err(e) => Err(Refusal::new(503, "voice_unavailable", format!("K2 could not read the App: {e}")).retry(RETRY_SECS)),
    }
}

fn live_resolve(door: &Door, bearer: &str) -> Result<KitPerson, Refusal> {
    match door {
        Door::Box { owner_token } => {
            // The bearer only: never `?token=` (the query is the kit's).
            let who = crate::compute_routes::who_from_tcp("", Some(bearer), owner_token);
            let caller = super::routes::Caller::from_who(who, bearer.to_string());
            let mut p = super::routes::person_of(&caller, owner_token)?;
            p.client = super::routes::client_of(bearer);
            Ok(KitPerson { sid: p.ident.sid.clone(), actor: p.ident.actor.clone(), app_session: false, who: Who::Login(Box::new(p)) })
        }
        Door::App { app_id } => {
            if !k2_core::skin::is_skin_token(bearer) {
                return Err(Refusal::new(401, "unauthenticated", "on an App's address this door takes the person's App login session"));
            }
            let service_id = app_service_id(app_id)?;
            // No owner token here: an App's address never takes it.
            let ident = crate::chat_pass::identity::classify(bearer, "").map_err(identity_refusal)?;
            if ident.provenance != Provenance::AppSession || ident.app.as_deref() != Some(service_id.as_str()) {
                return Err(Refusal::new(403, "wrong_app", "this login session isn't one of this App's"));
            }
            Ok(KitPerson { sid: ident.sid, actor: ident.actor, app_session: true, who: Who::AppSession { bearer: bearer.to_string() } })
        }
    }
}

/// An App session's chat pass, by K2's own `/cli/sew/pass` rules (its chat
/// grant, the App's caps ceiling, the keyed-App gate, the mint limit).
fn app_pass(bearer: &str) -> Result<String, Refusal> {
    let reply = crate::chat_pass::mint::handle_pass(&crate::chat_pass::mint::PassRequest {
        ingress: "loopback",
        ip: "127.0.0.1",
        browser: false,
        bearer: Some(bearer),
        owner_token: "",
        body: b"{}",
        link: None,
    });
    let status: u16 = reply.response.status.split_whitespace().next().and_then(|s| s.parse().ok()).unwrap_or(502);
    let body: Value = serde_json::from_str(&reply.response.body).unwrap_or(Value::Null);
    if status == 200 {
        if let Some(p) = body.get("pass").and_then(Value::as_str) {
            return Ok(p.to_string());
        }
    }
    let code = body.pointer("/error/code").and_then(Value::as_str).unwrap_or("chat_unavailable");
    let hint = body.pointer("/error/hint").and_then(Value::as_str).unwrap_or("K2 could not mint this person's chat pass");
    let mut r = Refusal::new(if status >= 400 { status } else { 502 }, code, hint);
    if let Some(s) = reply.retry_after_secs {
        r = r.retry(u64::try_from(s).unwrap_or(RETRY_SECS));
    }
    Err(r)
}

fn live_session(p: &KitPerson, fresh: bool) -> Result<String, Refusal> {
    match &p.who {
        Who::Login(person) => bff::person_session(person, fresh),
        Who::AppSession { bearer } => bff::cached_session(&p.sid, fresh, &bff::deps(), || app_pass(bearer)),
        Who::StandIn => Err(Refusal::new(503, "voice_unavailable", "no login behind this person")),
    }
}

/// [`KitDeps::resolve`] off the async runtime.
pub(super) async fn resolve(ctx: &KitCtx, bearer: &str) -> Result<KitPerson, Refusal> {
    let (deps, door, bearer) = (ctx.deps.clone(), ctx.door.clone(), bearer.to_string());
    tokio::task::spawn_blocking(move || (deps.resolve)(&door, &bearer))
        .await
        .unwrap_or_else(|e| Err(Refusal::new(500, "internal", format!("resolve: {e}"))))
}

async fn session(ctx: &KitCtx, p: &KitPerson, fresh: bool) -> Result<String, Refusal> {
    let (deps, p) = (ctx.deps.clone(), p.clone());
    tokio::task::spawn_blocking(move || (deps.session)(&p, fresh))
        .await
        .unwrap_or_else(|e| Err(Refusal::new(500, "internal", format!("session: {e}"))))
}

// ── limits ───────────────────────────────────────────────────────────

struct Bucket {
    tokens: f64,
    at: Instant,
}

fn buckets() -> &'static Mutex<HashMap<String, Bucket>> {
    static B: OnceLock<Mutex<HashMap<String, Bucket>>> = OnceLock::new();
    B.get_or_init(Default::default)
}

/// One request's worth of this login session's budget; false when spent.
pub fn take_token(sid: &str, now: Instant) -> bool {
    let mut g = buckets().lock().unwrap_or_else(|e| e.into_inner());
    if g.len() >= RATE_TRACKED_MAX && !g.contains_key(sid) {
        g.retain(|_, b| now.duration_since(b.at) < RATE_IDLE);
        if g.len() >= RATE_TRACKED_MAX {
            return false;
        }
    }
    let b = g.entry(sid.to_string()).or_insert(Bucket { tokens: RATE_BURST, at: now });
    b.tokens = (b.tokens + now.duration_since(b.at).as_secs_f64() * RATE_PER_SEC).min(RATE_BURST);
    b.at = now;
    if b.tokens < 1.0 {
        return false;
    }
    b.tokens -= 1.0;
    true
}

// ── Sew ──────────────────────────────────────────────────────────────

/// One answer from Sew, as it came.
#[derive(Debug, Clone)]
pub struct SewHttp {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// What the lane answers: its own refusal, or Sew's answer.
pub(super) enum Out {
    Refuse(Refusal),
    Sew(SewHttp),
}

fn client() -> Result<&'static reqwest::Client, String> {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    if let Some(c) = C.get() {
        return Ok(c);
    }
    let c = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    Ok(C.get_or_init(|| c))
}

/// Sew's loopback listener, never anywhere else.
pub(super) fn loopback(addr: &str) -> Result<SocketAddr, String> {
    let sa: SocketAddr = addr.parse().map_err(|_| "Sew's address isn't an address".to_string())?;
    if !sa.ip().is_loopback() {
        return Err("Sew's client listener is not loopback".into());
    }
    Ok(sa)
}

/// One request to Sew with `bearer` (a `sews_` session).
async fn sew_call(addr: &str, method: &str, target: &str, bearer: &str, body: Option<&[u8]>, extra: &[(String, String)]) -> Result<SewHttp, String> {
    let sa = loopback(addr)?;
    let m = if method == "POST" { reqwest::Method::POST } else { reqwest::Method::GET };
    let mut req = client()?.request(m, format!("http://{sa}{target}")).bearer_auth(bearer);
    for (k, v) in extra {
        req = req.header(k.as_str(), v.as_str());
    }
    if let Some(b) = body {
        req = req.header("content-type", "application/json").body(b.to_vec());
    }
    let mut r = req.send().await.map_err(|e| format!("{e}"))?;
    let status = r.status().as_u16();
    let headers = r
        .headers()
        .iter()
        .filter_map(|(k, v)| Some((k.as_str().to_ascii_lowercase(), v.to_str().ok()?.to_string())))
        .collect();
    let mut out = Vec::new();
    while let Some(c) = r.chunk().await.map_err(|e| format!("{e}"))? {
        out.extend_from_slice(&c);
        if out.len() > MAX_ANSWER {
            return Err("Sew's answer was too large".into());
        }
    }
    Ok(SewHttp { status, headers, body: out })
}

fn unreachable(e: String) -> Refusal {
    k2_core::log_debug!("[voice/kit-door] Sew: {e}");
    Refusal::new(503, "voice_unavailable", "K2 could not reach this server's Sew").retry(RETRY_SECS)
}

/// One Sew request as the person. A 401 means their Sew session is gone:
/// with `retry` (GETs, and K2's own ticket) it renews and retries once;
/// otherwise it renews for next time and the 401 goes back as Sew sent it.
#[allow(clippy::too_many_arguments)]
pub(super) async fn call_as(
    ctx: &KitCtx,
    p: &KitPerson,
    sew_addr: &str,
    method: &str,
    target: &str,
    body: Option<&[u8]>,
    extra: &[(String, String)],
    retry: bool,
) -> Result<SewHttp, Refusal> {
    let tok = session(ctx, p, false).await?;
    let a = sew_call(sew_addr, method, target, &tok, body, extra).await.map_err(unreachable)?;
    if a.status != 401 {
        return Ok(a);
    }
    let fresh = session(ctx, p, true).await;
    if !retry {
        return Ok(a);
    }
    let tok = fresh?;
    sew_call(sew_addr, method, target, &tok, body, extra).await.map_err(unreachable)
}

// ── answers ──────────────────────────────────────────────────────────

fn reason(status: u16) -> &'static str {
    reqwest::StatusCode::from_u16(status).ok().and_then(|c| c.canonical_reason()).unwrap_or("Unknown")
}

/// K2's own answer, in Sew's `ErrorBody` shape.
pub(super) async fn refuse<S: AsyncWrite + Unpin>(client: &mut S, r: &Refusal) {
    let mut details = r.details.clone().unwrap_or(Value::Null);
    if let Some(s) = r.retry_after_secs {
        if !details.is_object() {
            details = json!({});
        }
        details["retry_after_ms"] = json!(s * 1000);
    }
    let body = json!({ "ok": false, "error": { "code": r.code, "hint": r.hint, "details": details } }).to_string();
    let retry = r.retry_after_secs.map(|s| format!("Retry-After: {s}\r\n")).unwrap_or_default();
    let msg = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{retry}{frame}Connection: close\r\n\r\n{body}",
        r.status,
        reason(r.status),
        body.len(),
        frame = k2_core::frame_policy::CONNECT_HEADER_LINES,
    );
    let _ = client.write_all(msg.as_bytes()).await;
    let _ = client.flush().await;
}

fn crosses_back(name: &str) -> bool {
    FORWARD_BACK.contains(&name) || name.starts_with("x-ratelimit-")
}

/// Sew's answer: its status and body, the listed headers only.
pub(super) async fn relay_answer<S: AsyncWrite + Unpin>(client: &mut S, a: &SewHttp, body: &[u8]) {
    let mut msg = format!("HTTP/1.1 {} {}\r\n{}", a.status, reason(a.status), k2_core::frame_policy::CONNECT_HEADER_LINES);
    for (k, v) in &a.headers {
        if crosses_back(k) && !v.chars().any(char::is_control) {
            msg.push_str(&format!("{k}: {v}\r\n"));
        }
    }
    msg.push_str(&format!("Content-Length: {}\r\nConnection: close\r\n\r\n", body.len()));
    if client.write_all(msg.as_bytes()).await.is_ok() {
        let _ = client.write_all(body).await;
    }
    let _ = client.flush().await;
}

pub(super) async fn write_out<S: AsyncWrite + Unpin>(client: &mut S, out: &Out) {
    match out {
        Out::Refuse(r) => refuse(client, r).await,
        Out::Sew(a) => relay_answer(client, a, &a.body).await,
    }
}

/// R3's 200, trimmed to what the kit reads. With Sew's `app_recording`
/// (`recording`) the channel's `recording_rule` and `recording_required`
/// stay too, when Sew sent them (both are omitted for none / inherited).
pub fn trim_channel(body: &[u8], recording: bool) -> Option<Vec<u8>> {
    let v: Value = serde_json::from_slice(body).ok()?;
    let o = v.as_object()?;
    let id = o.get("id")?.clone();
    let days = o.get("recording_audio_days").cloned().unwrap_or(Value::Null);
    let mut out = json!({ "id": id, "recording_audio_days": days });
    if recording {
        for k in ["recording_rule", "recording_required"] {
            if let Some(x) = o.get(k).filter(|x| !x.is_null()) {
                out[k] = x.clone();
            }
        }
    }
    Some(out.to_string().into_bytes())
}

// ── the request ──────────────────────────────────────────────────────

fn header<'a>(head: &'a str, name: &'a str) -> Option<&'a str> {
    headers(head, name).next()
}

fn headers<'a>(head: &'a str, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
    head.lines().skip(1).filter_map(move |line| {
        let (n, v) = line.split_once(':')?;
        n.trim().eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

pub(super) fn bad(hint: &str) -> Refusal {
    Refusal::new(400, "validation_failed", hint)
}

/// The listed request headers, each at most once and plain.
fn forwarded_up(head: &str) -> Result<Vec<(String, String)>, Refusal> {
    let mut out = Vec::new();
    for name in FORWARD_UP {
        let vals: Vec<&str> = headers(head, name).collect();
        match vals.as_slice() {
            [] => {}
            [v] if v.len() <= HEADER_VALUE_MAX && v.bytes().all(|b| (0x20..0x7f).contains(&b)) => {
                out.push((name.to_string(), v.to_string()))
            }
            _ => return Err(bad(&format!("{name}: one plain value of at most {HEADER_VALUE_MAX} characters"))),
        }
    }
    Ok(out)
}

/// The request body: none on a GET; a JSON object on a POST (empty = `{}`),
/// re-serialized so only JSON crosses.
async fn read_body<S: AsyncRead + Unpin>(client: &mut S, head: &str, post: bool) -> Result<Option<Vec<u8>>, Refusal> {
    let lens: Vec<&str> = headers(head, "content-length").collect();
    let len = match lens.as_slice() {
        [] => 0,
        [v] => v.parse::<usize>().map_err(|_| bad("Content-Length is a number"))?,
        _ => return Err(bad("one Content-Length")),
    };
    if !post {
        return if len == 0 { Ok(None) } else { Err(bad("a GET has no body")) };
    }
    if len > MAX_BODY {
        return Err(Refusal::new(413, "too_large", format!("the body is at most {MAX_BODY} bytes")));
    }
    let mut raw = vec![0u8; len];
    match tokio::time::timeout(HTTP_TIMEOUT, client.read_exact(&mut raw)).await {
        Ok(Ok(_)) => {}
        _ => return Err(bad("the body ended early")),
    }
    if raw.iter().all(u8::is_ascii_whitespace) {
        return Ok(Some(b"{}".to_vec()));
    }
    match serde_json::from_slice::<Value>(&raw) {
        Ok(v @ Value::Object(_)) => Ok(Some(v.to_string().into_bytes())),
        _ => Err(bad("the body is a JSON object")),
    }
}

/// R9's check: `id` is on the person's own device list.
async fn own_device(ctx: &KitCtx, p: &KitPerson, sew_addr: &str, id: &str) -> Result<(), Out> {
    let a = call_as(ctx, p, sew_addr, "GET", "/api/v1/me/devices", None, &[], true).await.map_err(Out::Refuse)?;
    if a.status != 200 {
        return Err(Out::Sew(a));
    }
    let v: Value = serde_json::from_slice(&a.body).unwrap_or(Value::Null);
    let as_id = |x: &Value| match x {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    };
    let mine = v.get("items").and_then(Value::as_array).is_some_and(|items| {
        items.iter().any(|d| ["device_id", "id"].iter().any(|k| d.get(*k).and_then(as_id).as_deref() == Some(id)))
    });
    if mine {
        Ok(())
    } else {
        Err(Out::Refuse(
            Refusal::new(404, "not_found", "no device with that id on your own device list").details(json!({ "reason": "not_your_device" })),
        ))
    }
}

/// L8: the kit routes a Voice & Chat only login may never use.
pub fn voice_chat_kit_refusal(route: KitRoute) -> Option<Refusal> {
    matches!(route, KitRoute::RecordingStart | KitRoute::RecordingStop | KitRoute::MemberVoice | KitRoute::MemberMove)
        .then(|| Refusal::new(403, "voice_chat_not_allowed", "a Voice & Chat only login can't record, mute or move people"))
}

/// The Sew member ids of everyone in the same room as `sub` (none when
/// they're in no room).
pub fn room_mates(occ: &std::collections::BTreeMap<String, Vec<super::occupancy::Occupant>>, sub: &str) -> Vec<String> {
    occ.values()
        .find(|people| people.iter().any(|o| o.external_id.as_deref() == Some(sub)))
        .map(|people| people.iter().map(|o| o.member_id.clone()).collect())
        .unwrap_or_default()
}

/// R1's `ids=` narrowed to `allowed`; `None` when nothing is left.
pub fn narrow_members_query(q: &str, allowed: &[String]) -> Option<String> {
    let list = q.strip_prefix("ids=")?;
    let kept: Vec<&str> = list.split(',').filter(|id| allowed.iter().any(|a| a == id)).collect();
    (!kept.is_empty()).then(|| format!("ids={}", kept.join(",")))
}

/// One kit request whose head is consumed. `sew_path` has the door prefix
/// stripped; `bearer` is the request's bearer (never `sewcc_`).
#[allow(clippy::too_many_arguments)]
pub(super) async fn serve<S: AsyncRead + AsyncWrite + Unpin>(
    client: &mut S,
    method: &str,
    sew_path: &str,
    query: Option<&str>,
    head: &str,
    bearer: &str,
    ctx: &KitCtx,
    sew_addr: &str,
) {
    let recording = recording_kit_on();
    let Some(route) = kit_route_with(method, sew_path, query, recording) else {
        return refuse(client, &Refusal::new(404, "not_found", "no such route")).await;
    };
    // One credential and plain framing, or nothing: never two readings.
    if headers(head, "authorization").count() != 1 || header(head, "transfer-encoding").is_some() {
        return refuse(client, &bad("one Authorization header, and no Transfer-Encoding")).await;
    }
    let person = match resolve(ctx, bearer).await {
        Ok(p) => p,
        Err(r) => return refuse(client, &r).await,
    };
    if !take_token(&person.sid, Instant::now()) {
        return refuse(client, &Refusal::new(429, "rate_limited", "too many requests from this login").retry(RETRY_SECS)).await;
    }
    // Security review L8: a Voice & Chat only login joins, talks and
    // shares its screen, but never starts or stops a recording or mutes or
    // moves anyone (K2 refuses before Sew is asked), and `GET /members`
    // names only the people in its own room.
    let mut query = query.map(str::to_string);
    if let Who::Login(p) = &person.who {
        if p.role == k2_core::connect_users::Role::VoiceChat {
            if let Some(r) = voice_chat_kit_refusal(route) {
                return refuse(client, &r).await;
            }
            if route == KitRoute::Members {
                let (_, occ) = super::occupancy::snapshot();
                let allowed = room_mates(&occ, &p.ident.sub);
                match narrow_members_query(query.as_deref().unwrap_or(""), &allowed) {
                    Some(q) => query = Some(q),
                    None => {
                        let a = SewHttp { status: 200, headers: Vec::new(), body: br#"{"items":[],"has_more":false}"#.to_vec() };
                        return relay_answer(client, &a, &a.body).await;
                    }
                }
            }
        }
    }
    let query = query.as_deref();
    if route == KitRoute::Feed {
        return super::kit_feed::serve(client, head, person, bearer, ctx, sew_addr).await;
    }
    let extra = match forwarded_up(head) {
        Ok(x) => x,
        Err(r) => return refuse(client, &r).await,
    };
    let body = match read_body(client, head, route.is_post()).await {
        Ok(b) => b,
        Err(r) => return refuse(client, &r).await,
    };
    let Some(_slot) = super::sew_lane::RelaySlot::take() else {
        return refuse(client, &Refusal::new(503, "voice_unavailable", "too many calls through this server right now").retry(RETRY_SECS)).await;
    };
    if route == KitRoute::DeviceRevoke {
        let id = sew_path.split('/').nth(4).unwrap_or("");
        if let Err(out) = own_device(ctx, &person, sew_addr, id).await {
            return write_out(client, &out).await;
        }
    }
    let target = match query {
        Some(q) => format!("{sew_path}?{q}"),
        None => sew_path.to_string(),
    };
    let a = match call_as(ctx, &person, sew_addr, method, &target, body.as_deref(), &extra, !route.is_post()).await {
        Ok(a) => a,
        Err(r) => return refuse(client, &r).await,
    };
    if route == KitRoute::Channel && a.status == 200 {
        return match trim_channel(&a.body, recording) {
            Some(b) => relay_answer(client, &a, &b).await,
            None => refuse(client, &Refusal::new(502, "voice_unavailable", "Sew's channel answer wasn't a channel")).await,
        };
    }
    relay_answer(client, &a, &a.body).await;
}

#[cfg(test)]
#[path = "kit_door_tests.rs"]
mod tests;

#[cfg(test)]
mod voice_chat_door_tests {
    use super::*;
    use crate::voice::occupancy::Occupant;

    fn occ(member: &str, ext: &str) -> Occupant {
        Occupant {
            member_id: member.into(),
            external_id: Some(ext.into()),
            person: member.into(),
            connection: "connected".into(),
            self_mute: false,
            self_deaf: false,
            server_mute: false,
            server_deaf: false,
            joined_at: String::new(),
        }
    }

    /// Security review L8: recording and moderating are refused for this
    /// role at K2; everything else the kit does stays Sew's to decide.
    #[test]
    fn voice_chat_never_records_mutes_or_moves() {
        for r in [KitRoute::RecordingStart, KitRoute::RecordingStop, KitRoute::MemberVoice, KitRoute::MemberMove] {
            let refusal = voice_chat_kit_refusal(r).unwrap_or_else(|| panic!("{r:?} refused"));
            assert_eq!((refusal.status, refusal.code.as_str()), (403, "voice_chat_not_allowed"));
        }
        for r in [
            KitRoute::Members,
            KitRoute::CallDecline,
            KitRoute::Channel,
            KitRoute::MyDevices,
            KitRoute::DeviceRevoke,
            KitRoute::Feed,
            KitRoute::ChannelRecording,
            KitRoute::ChannelRecordings,
            KitRoute::Recording,
            KitRoute::TranscriptReplies,
        ] {
            assert!(voice_chat_kit_refusal(r).is_none(), "{r:?}");
        }
    }

    /// L8: `GET /members` names only the people in the login's own room.
    #[test]
    fn members_are_narrowed_to_the_logins_room() {
        let mut m = std::collections::BTreeMap::new();
        m.insert("10".to_string(), vec![occ("1", "k2:connect:me"), occ("2", "k2:connect:mate")]);
        m.insert("11".to_string(), vec![occ("3", "k2:connect:other")]);
        let mates = room_mates(&m, "k2:connect:me");
        assert_eq!(mates, vec!["1".to_string(), "2".to_string()]);
        assert_eq!(narrow_members_query("ids=1,2,3", &mates).as_deref(), Some("ids=1,2"));
        assert_eq!(narrow_members_query("ids=3", &mates), None, "nobody left: answered empty, Sew not asked");
        assert!(room_mates(&m, "k2:connect:nobody").is_empty(), "in no room: nobody");
    }
}
