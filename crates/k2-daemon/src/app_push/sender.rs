//! The direct sender: the daemon calls Apple and Google with the App
//! owner's own keys (prd-custom-app-push-v1 §7, CP18–CP23).
//!
//! - **APNs** (CP18): `POST <host>/3/device/<token>` over HTTP/2 (reqwest's
//!   `http2` feature; Apple picks h2 by ALPN), `api.push.apple.com` or
//!   `api.sandbox.push.apple.com` per device. One ES256 provider token per
//!   App, re-minted after 50 minutes (Apple: valid 60, refresh no more
//!   often than every 20), never after a `TooManyProviderTokenUpdates` for
//!   20 minutes.
//! - **FCM** (CP19): HTTP v1 `POST <host>/v1/projects/<id>/messages:send`,
//!   an OAuth2 access token from an RS256 assertion to the pinned
//!   `token_uri`, cached until 5 minutes before it expires.
//! - **Fixed hosts** (CP20, CP42): the vendor hosts are constants. A debug
//!   or test build may point them at a local fake with
//!   `K2_APP_PUSH_TEST_APNS_URL` / `K2_APP_PUSH_TEST_FCM_URL` (loopback
//!   `http://` only); a release build never reads them ([`hosts_for`]).
//! - **Results** (CP21): `sent`, `dead` (the row is deleted), `failed`.
//!   A credential refusal pauses the App's platform for 10 minutes, stores
//!   the reason code and leaves one inbox note an hour. FCM's
//!   `INVALID_ARGUMENT` prunes only when its field violation names
//!   `message.token` (A6): a payload bug never wipes an App's devices.
//! - **Logs** (CP41): counts and reason codes only; never a token, title,
//!   body, data value, JWT, key or a URL with a token (APNs URLs are logged
//!   as `<host>/3/device/<redacted>`). A transport error is reduced to its
//!   kind: reqwest's own message names the URL.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use k2_core::skin::app_push as store;
use serde_json::Value as J;

use super::creds;
use super::payload::{self, Notification, Stamps};

pub const APNS_PRODUCTION: &str = "https://api.push.apple.com";
pub const APNS_SANDBOX: &str = "https://api.sandbox.push.apple.com";
pub const FCM_HOST: &str = "https://fcm.googleapis.com";
pub const TEST_APNS_ENV: &str = "K2_APP_PUSH_TEST_APNS_URL";
pub const TEST_FCM_ENV: &str = "K2_APP_PUSH_TEST_FCM_URL";
/// Per request (CP21).
pub const TIMEOUT: Duration = Duration::from_secs(10);
/// A provider token is re-minted after this long (the relay's 50 minutes).
pub const JWT_REFRESH_SECS: i64 = 50 * 60;
/// After `TooManyProviderTokenUpdates`, keep the token at least this long.
pub const JWT_HOLD_SECS: i64 = 20 * 60;
/// An access token is refreshed this long before it expires.
pub const OAUTH_EARLY_SECS: i64 = 5 * 60;
/// A credential refusal pauses the App's platform this long (the breaker).
pub const BREAKER: Duration = Duration::from_secs(10 * 60);
/// At most one inbox note per App this often.
pub const NOTE_EVERY: Duration = Duration::from_secs(3600);
/// Retry a 5xx or transport error once, after this long.
#[cfg(not(test))]
pub const RETRY_DELAY: Duration = Duration::from_secs(1);
#[cfg(test)]
pub const RETRY_DELAY: Duration = Duration::from_millis(5);
/// The longest `Retry-After` honoured for a whole App.
pub const MAX_BACKOFF_SECS: u64 = 3600;
/// The most of a vendor answer read.
pub const MAX_RESPONSE_BYTES: usize = 64 * 1024;

// ── hosts ────────────────────────────────────────────────────────────

/// Where the sender sends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hosts {
    pub apns_production: String,
    pub apns_sandbox: String,
    pub fcm: String,
    pub google_token: String,
}

impl Hosts {
    pub fn vendor() -> Hosts {
        Hosts {
            apns_production: APNS_PRODUCTION.into(),
            apns_sandbox: APNS_SANDBOX.into(),
            fcm: FCM_HOST.into(),
            google_token: creds::GOOGLE_TOKEN_URI.into(),
        }
    }
}

fn loopback_http(raw: &str) -> Option<String> {
    let u = url::Url::parse(raw.trim()).ok()?;
    if u.scheme() != "http" || !u.username().is_empty() || u.password().is_some() || u.query().is_some() || u.fragment().is_some() {
        return None;
    }
    if !matches!(u.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")) {
        return None;
    }
    Some(u.as_str().trim_end_matches('/').to_string())
}

/// The hosts for a build: `debug = false` (a release build) always answers
/// the vendor constants, whatever the overrides say. A debug build takes a
/// loopback `http://` override (the test fake); anything else is ignored.
pub fn hosts_for(debug: bool, apns_override: Option<&str>, fcm_override: Option<&str>) -> Hosts {
    let mut h = Hosts::vendor();
    if !debug {
        return h;
    }
    if let Some(a) = apns_override.and_then(loopback_http) {
        h.apns_production = a.clone();
        h.apns_sandbox = format!("{a}/sandbox");
    }
    if let Some(f) = fcm_override.and_then(loopback_http) {
        h.fcm = f.clone();
        h.google_token = format!("{f}/token");
    }
    h
}

/// This build's hosts. The override env vars are read only in a debug build
/// (compile-time gate).
pub fn hosts() -> Hosts {
    #[cfg(debug_assertions)]
    {
        hosts_for(true, std::env::var(TEST_APNS_ENV).ok().as_deref(), std::env::var(TEST_FCM_ENV).ok().as_deref())
    }
    #[cfg(not(debug_assertions))]
    {
        hosts_for(false, None, None)
    }
}

// ── transport ────────────────────────────────────────────────────────

/// One HTTP POST.
pub struct HttpReq {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl std::fmt::Debug for HttpReq {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpReq").field("url", &redact_url(&self.url)).field("headers", &self.headers.len()).finish()
    }
}

/// Its answer.
#[derive(Clone, PartialEq, Eq)]
pub struct HttpResp {
    pub status: u16,
    /// `Retry-After`, in seconds, when the vendor sent one.
    pub retry_after: Option<u64>,
    pub body: Vec<u8>,
}

impl std::fmt::Debug for HttpResp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The body can carry an access token (Google's token answer).
        f.debug_struct("HttpResp").field("status", &self.status).field("retry_after", &self.retry_after).field("body", &"<redacted>").finish()
    }
}

/// A transport failure, reduced to its kind (never the URL).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportErr {
    Timeout,
    Network,
}

/// The seam tests fake (the `DispatchSender` pattern).
pub trait Transport: Send + Sync {
    fn post(&self, r: &HttpReq) -> Result<HttpResp, TransportErr>;
}

/// The real transport: one blocking client with HTTP/2 by ALPN and a 10 s
/// timeout.
pub struct ReqwestTransport {
    client: reqwest::blocking::Client,
}

impl ReqwestTransport {
    pub fn new() -> Result<Self, String> {
        let client = reqwest::blocking::Client::builder()
            .timeout(TIMEOUT)
            .connect_timeout(TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .pool_max_idle_per_host(8)
            .build()
            .map_err(|_| "the push HTTP client didn't start".to_string())?;
        Ok(ReqwestTransport { client })
    }
}

impl Transport for ReqwestTransport {
    fn post(&self, r: &HttpReq) -> Result<HttpResp, TransportErr> {
        let mut req = self.client.post(&r.url).body(r.body.clone());
        for (k, v) in &r.headers {
            req = req.header(k.as_str(), v.as_str());
        }
        let resp = req.send().map_err(|e| if e.is_timeout() { TransportErr::Timeout } else { TransportErr::Network })?;
        let status = resp.status().as_u16();
        let retry_after = resp.headers().get("retry-after").and_then(|v| v.to_str().ok()).and_then(|s| s.trim().parse::<u64>().ok());
        let mut body = Vec::new();
        use std::io::Read;
        let _ = resp.take(MAX_RESPONSE_BYTES as u64).read_to_end(&mut body);
        Ok(HttpResp { status, retry_after, body })
    }
}

/// `<host>/3/device/<redacted>` for an APNs URL; any other URL as given
/// (FCM URLs carry the project id only, tokens go in the body).
pub fn redact_url(url: &str) -> String {
    match url.find("/3/device/") {
        Some(i) => format!("{}/3/device/<redacted>", &url[..i]),
        None => url.to_string(),
    }
}

// ── logging (a test sink proves what it carries) ─────────────────────

type LogSink = Box<dyn Fn(&str) + Send + Sync>;

fn log_sink() -> &'static RwLock<Option<LogSink>> {
    static S: OnceLock<RwLock<Option<LogSink>>> = OnceLock::new();
    S.get_or_init(|| RwLock::new(None))
}

/// Tests: capture every App push log line.
#[cfg(test)]
pub(crate) fn set_log_sink(s: Option<LogSink>) {
    *log_sink().write().unwrap_or_else(|e| e.into_inner()) = s;
}

/// Every App push log line goes through here.
pub(crate) fn log(line: &str) {
    if let Some(s) = log_sink().read().unwrap_or_else(|e| e.into_inner()).as_ref() {
        s(line);
    }
    k2_core::log_debug!("[app-push] {line}");
}

// ── outcomes ─────────────────────────────────────────────────────────

/// One device send's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Sent,
    /// The vendor said the token is gone; the row was deleted.
    /// `wrong_app`: a token from another app (a misconfigured bundle id).
    Dead { wrong_app: bool },
    /// Not delivered (a reason code for the log and status).
    Failed(String),
    /// Dropped before sending (air-gap).
    Dropped,
}

/// One device to send to. `token` is redacted in `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct Target {
    pub id: String,
    pub app: String,
    pub platform: String,
    pub environment: String,
    pub token: String,
}

impl std::fmt::Debug for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Target")
            .field("id", &self.id)
            .field("app", &self.app)
            .field("platform", &self.platform)
            .field("environment", &self.environment)
            .field("token", &"<redacted>")
            .finish()
    }
}

impl From<&store::PushDevice> for Target {
    fn from(d: &store::PushDevice) -> Self {
        Target { id: d.id.clone(), app: d.app.clone(), platform: d.platform.clone(), environment: d.environment.clone(), token: d.push_token.clone() }
    }
}

/// What one vendor answer means.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Verdict {
    Sent,
    Dead { wrong_app: bool },
    /// Stop: the App's key was refused (a reason code).
    Cred(String),
    /// Re-mint the provider token / refresh the access token once.
    Reauth(String),
    /// Retry once after [`RETRY_DELAY`].
    Retry(String),
    /// Pause the App's platform (seconds) and fail.
    Backoff(String, u64),
    /// Keep the provider token longer and fail.
    HoldJwt(String),
    Failed(String),
}

fn apns_reason(body: &[u8]) -> String {
    serde_json::from_slice::<J>(body)
        .ok()
        .and_then(|v| v.get("reason").and_then(J::as_str).map(str::to_string))
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(48)
        .collect()
}

/// APNs status + reason → verdict (CP21, A8).
fn apns_verdict(status: u16, body: &[u8]) -> Verdict {
    let reason = apns_reason(body);
    let code = |r: &str| format!("apns:{}", if r.is_empty() { status.to_string() } else { r.to_string() });
    match (status, reason.as_str()) {
        (200, _) => Verdict::Sent,
        (410, _) => Verdict::Dead { wrong_app: false },
        (400, "BadDeviceToken") => Verdict::Dead { wrong_app: false },
        (400, "DeviceTokenNotForTopic") => Verdict::Dead { wrong_app: true },
        (400, "TopicDisallowed") => Verdict::Cred(code(&reason)),
        (403, "ExpiredProviderToken") => Verdict::Reauth(code(&reason)),
        (403, _) => Verdict::Cred(code(&reason)),
        (429, "TooManyProviderTokenUpdates") => Verdict::HoldJwt(code(&reason)),
        (s, _) if s >= 500 => Verdict::Retry(code(&reason)),
        _ => Verdict::Failed(code(&reason)),
    }
}

/// FCM's error code: the FcmError detail's `errorCode`, else the status.
fn fcm_error(body: &[u8]) -> (String, bool) {
    let v: J = serde_json::from_slice(body).unwrap_or(J::Null);
    let err = &v["error"];
    let mut code = String::new();
    let mut names_token = false;
    if let Some(details) = err["details"].as_array() {
        for d in details {
            if let Some(c) = d["errorCode"].as_str() {
                code = c.to_string();
            }
            if let Some(fv) = d["fieldViolations"].as_array() {
                names_token |= fv.iter().any(|f| f["field"].as_str() == Some("message.token"));
            }
        }
    }
    if code.is_empty() {
        code = err["status"].as_str().unwrap_or("").to_string();
    }
    (code.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').take(48).collect(), names_token)
}

/// FCM status + error → verdict (CP21, A6, A8).
fn fcm_verdict(status: u16, body: &[u8], retry_after: Option<u64>) -> Verdict {
    if status == 200 {
        return Verdict::Sent;
    }
    let (code, names_token) = fcm_error(body);
    let tag = format!("fcm:{}", if code.is_empty() { status.to_string() } else { code.clone() });
    match (status, code.as_str()) {
        // Only Google's UNREGISTERED says the token is gone; any other 404
        // (a wrong project, a stray route) must never delete devices.
        (404, "UNREGISTERED") => Verdict::Dead { wrong_app: false },
        (403, "SENDER_ID_MISMATCH") => Verdict::Dead { wrong_app: true },
        (400, "INVALID_ARGUMENT") if names_token => Verdict::Dead { wrong_app: false },
        (401, _) | (403, _) => Verdict::Reauth(tag),
        (429, _) => Verdict::Backoff(tag, retry_after.unwrap_or(60).min(MAX_BACKOFF_SECS)),
        (s, _) if s >= 500 => Verdict::Retry(tag),
        _ => Verdict::Failed(tag),
    }
}

/// The code a wrong-app answer is stored under.
fn wrong_app_code(platform: &str) -> &'static str {
    if platform == store::PLATFORM_APNS {
        "apns:DeviceTokenNotForTopic"
    } else {
        "fcm:SENDER_ID_MISMATCH"
    }
}

// ── auth caches ──────────────────────────────────────────────────────

#[derive(Clone)]
struct ApnsAuth {
    /// The key it was minted with: (fingerprint, key id, team id).
    key: (String, String, String),
    jwt: String,
    minted: i64,
    hold_until: i64,
}

#[derive(Clone)]
struct FcmAuth {
    fpr: String,
    token: String,
    expires_at: i64,
}

/// Why auth couldn't be had.
#[derive(Debug, Clone, PartialEq, Eq)]
enum AuthErr {
    /// The App's key was refused (a code): the breaker.
    Cred(String),
    /// Anything else (vault closed, no key, a transport error): fail.
    Fail(String),
}

fn key_err(e: creds::KeyError) -> AuthErr {
    AuthErr::Fail(e.code())
}

/// The sender. One per daemon ([`global`]); tests build their own with a
/// fake transport and clock.
pub struct Sender {
    hosts: Hosts,
    transport: Arc<dyn Transport>,
    now: fn() -> i64,
    apns: Mutex<HashMap<String, ApnsAuth>>,
    fcm: Mutex<HashMap<String, FcmAuth>>,
    paused: Mutex<HashMap<(String, String), Instant>>,
    noted: Mutex<HashMap<String, Instant>>,
    proven: Mutex<std::collections::HashSet<(String, String)>>,
}

fn wall_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

impl Sender {
    pub fn new(hosts: Hosts, transport: Arc<dyn Transport>) -> Self {
        Self::with_clock(hosts, transport, wall_now)
    }

    pub fn with_clock(hosts: Hosts, transport: Arc<dyn Transport>, now: fn() -> i64) -> Self {
        Sender {
            hosts,
            transport,
            now,
            apns: Mutex::new(HashMap::new()),
            fcm: Mutex::new(HashMap::new()),
            paused: Mutex::new(HashMap::new()),
            noted: Mutex::new(HashMap::new()),
            proven: Mutex::new(std::collections::HashSet::new()),
        }
    }

    fn transport_for(&self, app: &str) -> Arc<dyn Transport> {
        #[cfg(test)]
        if let Some(t) = test_transport(app) {
            return t;
        }
        let _ = app;
        self.transport.clone()
    }

    /// Is the App's platform paused (the breaker or a back-off)?
    pub fn paused(&self, app: &str, platform: &str) -> bool {
        let mut m = self.paused.lock().unwrap_or_else(|e| e.into_inner());
        let key = (app.to_string(), platform.to_string());
        match m.get(&key) {
            Some(until) if Instant::now() < *until => true,
            Some(_) => {
                m.remove(&key);
                false
            }
            None => false,
        }
    }

    fn pause(&self, app: &str, platform: &str, d: Duration) {
        self.paused.lock().unwrap_or_else(|e| e.into_inner()).insert((app.to_string(), platform.to_string()), Instant::now() + d);
    }

    /// Lift a pause (new keys were set).
    pub fn reset_app(&self, app: &str) {
        self.paused.lock().unwrap_or_else(|e| e.into_inner()).retain(|(a, _), _| a != app);
        self.proven.lock().unwrap_or_else(|e| e.into_inner()).retain(|(a, _)| a != app);
        self.apns.lock().unwrap_or_else(|e| e.into_inner()).remove(app);
        self.fcm.lock().unwrap_or_else(|e| e.into_inner()).remove(app);
    }

    fn apns_jwt(&self, app: &str, force: bool) -> Result<String, AuthErr> {
        let now = (self.now)();
        let meta = store::config(app).map_err(|_| AuthErr::Fail("store".into()))?.apns.ok_or(AuthErr::Fail("push_not_configured".into()))?;
        let id = (meta.key_fingerprint, meta.key_id, meta.team_id);
        {
            let m = self.apns.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(a) = m.get(app).filter(|a| a.key == id) {
                let fresh = now - a.minted < JWT_REFRESH_SECS;
                let held = now < a.hold_until;
                if (fresh && !force) || held {
                    return Ok(a.jwt.clone());
                }
            }
        }
        let key = creds::load_apns(app).map_err(key_err)?;
        let jwt = creds::apns_jwt(&key, now).map_err(|_| AuthErr::Cred("apns:key_unusable".into()))?;
        self.apns.lock().unwrap_or_else(|e| e.into_inner()).insert(app.to_string(), ApnsAuth { key: id, jwt: jwt.clone(), minted: now, hold_until: 0 });
        Ok(jwt)
    }

    fn hold_jwt(&self, app: &str) {
        let now = (self.now)();
        if let Some(a) = self.apns.lock().unwrap_or_else(|e| e.into_inner()).get_mut(app) {
            a.hold_until = a.minted.max(now) + JWT_HOLD_SECS;
        }
    }

    fn fcm_token(&self, app: &str, force: bool) -> Result<(String, String), AuthErr> {
        let now = (self.now)();
        let cfg = store::config(app).map_err(|_| AuthErr::Fail("store".into()))?;
        let meta = cfg.fcm.ok_or(AuthErr::Fail("push_not_configured".into()))?;
        if !force {
            let m = self.fcm.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(a) = m.get(app).filter(|a| a.fpr == meta.key_fingerprint && now < a.expires_at - OAUTH_EARLY_SECS) {
                return Ok((a.token.clone(), meta.project_id.clone()));
            }
        }
        let key = creds::load_fcm(app).map_err(key_err)?;
        let assertion = creds::fcm_assertion(&key, now).map_err(|_| AuthErr::Cred("fcm:key_unusable".into()))?;
        let body = format!("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer&assertion={assertion}");
        let req = HttpReq {
            url: self.hosts.google_token.clone(),
            headers: vec![("content-type".into(), "application/x-www-form-urlencoded".into())],
            body: body.into_bytes(),
        };
        let resp = self.transport_for(app).post(&req).map_err(|e| AuthErr::Fail(format!("fcm:oauth_{e:?}").to_ascii_lowercase()))?;
        if resp.status == 400 || resp.status == 401 || resp.status == 403 {
            let err: String = serde_json::from_slice::<J>(&resp.body)
                .ok()
                .and_then(|v| v["error"].as_str().map(str::to_string))
                .unwrap_or_else(|| resp.status.to_string())
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
                .take(48)
                .collect();
            return Err(AuthErr::Cred(format!("fcm:oauth_{err}")));
        }
        if resp.status != 200 {
            return Err(AuthErr::Fail(format!("fcm:oauth_{}", resp.status)));
        }
        let v: J = serde_json::from_slice(&resp.body).map_err(|_| AuthErr::Fail("fcm:oauth_bad_answer".into()))?;
        let token = v["access_token"].as_str().filter(|s| !s.is_empty()).ok_or(AuthErr::Fail("fcm:oauth_bad_answer".into()))?.to_string();
        let expires_in = v["expires_in"].as_i64().unwrap_or(3600).clamp(60, 86_400);
        self.fcm
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(app.to_string(), FcmAuth { fpr: meta.key_fingerprint.clone(), token: token.clone(), expires_at: now + expires_in });
        Ok((token, meta.project_id))
    }

    /// POST with one retry after [`RETRY_DELAY`] for a 5xx or a transport
    /// error; `verdict` maps the answer.
    fn post_mapped(&self, app: &str, req: &HttpReq, verdict: &dyn Fn(&HttpResp) -> Verdict) -> Verdict {
        let t = self.transport_for(app);
        let mut last = Verdict::Failed("transport".into());
        for attempt in 0..2 {
            if attempt > 0 {
                std::thread::sleep(RETRY_DELAY);
            }
            match t.post(req) {
                Ok(r) => match verdict(&r) {
                    Verdict::Retry(c) => last = Verdict::Failed(c),
                    v => return v,
                },
                Err(TransportErr::Timeout) => last = Verdict::Failed("timeout".into()),
                Err(TransportErr::Network) => last = Verdict::Failed("network".into()),
            }
        }
        last
    }

    fn apns_send(&self, t: &Target, n: &Notification, s: &Stamps) -> Verdict {
        let host = if t.environment == store::ENV_SANDBOX { &self.hosts.apns_sandbox } else { &self.hosts.apns_production };
        let bundle = match store::config(&t.app) {
            Ok(c) => match c.apns {
                Some(m) => m.bundle_id,
                None => return Verdict::Failed("push_not_configured".into()),
            },
            Err(_) => return Verdict::Failed("store".into()),
        };
        let body = payload::apns_body(n, s);
        let mut reauthed = false;
        loop {
            let jwt = match self.apns_jwt(&t.app, reauthed) {
                Ok(j) => j,
                Err(AuthErr::Cred(c)) => return Verdict::Cred(c),
                Err(AuthErr::Fail(c)) => return Verdict::Failed(c),
            };
            let mut headers = vec![("authorization".to_string(), format!("bearer {jwt}")), ("content-type".to_string(), "application/json".to_string())];
            headers.extend(payload::apns_headers(n, &bundle, (self.now)()));
            let req = HttpReq { url: format!("{host}/3/device/{}", t.token), headers, body: body.clone() };
            match self.post_mapped(&t.app, &req, &|r| apns_verdict(r.status, &r.body)) {
                Verdict::Reauth(_) if !reauthed => reauthed = true,
                Verdict::Reauth(c) => return Verdict::Cred(c),
                Verdict::HoldJwt(c) => {
                    self.hold_jwt(&t.app);
                    return Verdict::Failed(c);
                }
                v => return v,
            }
        }
    }

    fn fcm_send(&self, t: &Target, n: &Notification, s: &Stamps) -> Verdict {
        let mut refreshed = false;
        loop {
            let (token, project) = match self.fcm_token(&t.app, refreshed) {
                Ok(x) => x,
                Err(AuthErr::Cred(c)) => return Verdict::Cred(c),
                Err(AuthErr::Fail(c)) => return Verdict::Failed(c),
            };
            let req = HttpReq {
                url: format!("{}/v1/projects/{project}/messages:send", self.hosts.fcm),
                headers: vec![("authorization".into(), format!("Bearer {token}")), ("content-type".into(), "application/json".into())],
                body: payload::fcm_body(n, s, &t.token, (self.now)()),
            };
            match self.post_mapped(&t.app, &req, &|r| fcm_verdict(r.status, &r.body, r.retry_after)) {
                Verdict::Reauth(_) if !refreshed => refreshed = true,
                Verdict::Reauth(c) => return Verdict::Cred(c),
                v => return v,
            }
        }
    }

    /// Send one notification to one device (blocking; the worker calls it).
    pub fn deliver(&self, t: &Target, n: &Notification, s: &Stamps) -> Outcome {
        if crate::push_routes::airgap_blocks() {
            crate::push_routes::note_airgap_drop();
            return Outcome::Dropped;
        }
        if self.paused(&t.app, &t.platform) {
            return Outcome::Failed(format!("{}:paused", t.platform));
        }
        let v = match t.platform.as_str() {
            store::PLATFORM_APNS => self.apns_send(t, n, s),
            store::PLATFORM_FCM => self.fcm_send(t, n, s),
            _ => Verdict::Failed("platform".into()),
        };
        match v {
            Verdict::Sent => {
                self.prove(&t.app, &t.platform);
                Outcome::Sent
            }
            // A token "from another app" before this key ever got a send
            // through means the key's bundle id / Firebase project is wrong,
            // not that the phone is gone: keep the rows, trip the breaker.
            Verdict::Dead { wrong_app: true } if !self.proven(&t.app, &t.platform) => {
                let code = wrong_app_code(&t.platform);
                self.cred_refused(&t.app, &t.platform, code);
                Outcome::Failed(code.to_string())
            }
            Verdict::Dead { wrong_app } => {
                if let Err(e) = store::delete_dead(&t.app, &t.id, &t.token) {
                    log(&format!("{} device {}: dead token not removed: {e}", t.app, t.id));
                }
                log(&format!("{} device {} ({}): token gone{}, row removed", t.app, t.id, t.platform, if wrong_app { " (another app's token)" } else { "" }));
                Outcome::Dead { wrong_app }
            }
            Verdict::Cred(code) => {
                self.cred_refused(&t.app, &t.platform, &code);
                Outcome::Failed(code)
            }
            Verdict::Backoff(code, secs) => {
                self.pause(&t.app, &t.platform, Duration::from_secs(secs));
                log(&format!("{} {}: back off {secs}s ({code})", t.app, t.platform));
                Outcome::Failed(code)
            }
            Verdict::Failed(code) | Verdict::Retry(code) | Verdict::Reauth(code) | Verdict::HoldJwt(code) => Outcome::Failed(code),
        }
    }

    /// Has the platform's current key had an accepted send (cached once
    /// true; new keys reset the cache with [`Self::reset_app`])?
    fn proven(&self, app: &str, platform: &str) -> bool {
        let key = (app.to_string(), platform.to_string());
        if self.proven.lock().unwrap_or_else(|e| e.into_inner()).contains(&key) {
            return true;
        }
        let cfg = match store::config(app) {
            Ok(c) => c,
            Err(_) => return false,
        };
        let yes = match platform {
            store::PLATFORM_APNS => cfg.apns.and_then(|m| m.proven_at).is_some(),
            store::PLATFORM_FCM => cfg.fcm.and_then(|m| m.proven_at).is_some(),
            _ => false,
        };
        if yes {
            self.proven.lock().unwrap_or_else(|e| e.into_inner()).insert(key);
        }
        yes
    }

    fn prove(&self, app: &str, platform: &str) {
        if self.proven(app, platform) {
            return;
        }
        match store::mark_proven(app, platform) {
            Ok(_) => {
                self.proven.lock().unwrap_or_else(|e| e.into_inner()).insert((app.to_string(), platform.to_string()));
            }
            Err(e) => log(&format!("{app} {platform}: first accepted send not recorded: {e}")),
        }
    }

    /// The breaker (CP21): pause the platform, store the code, note once an
    /// hour.
    fn cred_refused(&self, app: &str, platform: &str, code: &str) {
        self.pause(app, platform, BREAKER);
        match platform {
            store::PLATFORM_APNS => {
                self.apns.lock().unwrap_or_else(|e| e.into_inner()).remove(app);
            }
            _ => {
                self.fcm.lock().unwrap_or_else(|e| e.into_inner()).remove(app);
            }
        }
        if let Err(e) = store::set_cred_error(app, code) {
            log(&format!("{app}: credential error not stored: {e}"));
        }
        log(&format!("{app} {platform}: key refused ({code}); paused {} minutes", BREAKER.as_secs() / 60));
        let due = {
            let mut m = self.noted.lock().unwrap_or_else(|e| e.into_inner());
            let due = m.get(app).is_none_or(|t| t.elapsed() >= NOTE_EVERY);
            if due {
                m.insert(app.to_string(), Instant::now());
            }
            due
        };
        if due {
            super::audit_line("app-push-key-refused", "k2:app-push", &format!("app={app} platform={platform} code={code}"));
            if let Some(row) = super::published(app) {
                crate::apps::alerts::note(
                    &row.project_id,
                    "App push keys refused",
                    &format!(
                        "{} refused {}'s push key ({code}). Sends to {} devices are paused for {} minutes. The owner checks the key with k2 app push check {} and replaces it with k2 app push setup.",
                        if platform == store::PLATFORM_APNS { "Apple" } else { "Google" },
                        row.name,
                        platform,
                        BREAKER.as_secs() / 60,
                        row.name
                    ),
                );
            }
        }
    }

    /// `k2 app push check` (CP16): one real call per configured platform to
    /// a well-formed fake token. A token refusal means the key (and, for
    /// Apple, the team and topic it may push to) works. A fake token can't
    /// prove that the bundle id or Firebase project matches the phone app:
    /// only a real device's token can ([`CHECK_LIMITS`]).
    pub fn check(&self, app: &str, platform: &str) -> Result<&'static str, String> {
        let fake = Target {
            id: "check".into(),
            app: app.to_string(),
            platform: platform.to_string(),
            environment: store::ENV_PRODUCTION.into(),
            token: if platform == store::PLATFORM_APNS { "0".repeat(64) } else { "k2-check-not-a-real-token".into() },
        };
        let n = Notification {
            title: Some("K2 key check".into()),
            body: Some("K2 key check".into()),
            data: Default::default(),
            topic: None,
            collapse_id: None,
            thread_id: None,
            sound: None,
            badge: None,
            silent: false,
            high_priority: false,
            ttl_secs: 0,
            mutable: false,
            android_channel: None,
        };
        let s = Stamps { app: app.to_string(), send_id: "check".into(), kind: payload::KIND_APP, who: "0".repeat(32) };
        let v = match platform {
            store::PLATFORM_APNS => self.apns_send(&fake, &n, &s),
            store::PLATFORM_FCM => self.fcm_send(&fake, &n, &s),
            _ => return Err("platform is apns or fcm".into()),
        };
        match v {
            // A wrong-app answer names a key/topic mismatch.
            Verdict::Dead { wrong_app: true } => {
                let c = wrong_app_code(platform).to_string();
                if let Err(e) = store::set_cred_error(app, &c) {
                    log(&format!("{app}: credential error not stored: {e}"));
                }
                Err(c)
            }
            // The fake token was refused: the key itself works.
            Verdict::Dead { wrong_app: false } => Ok("ok"),
            Verdict::Failed(c) if c == "fcm:INVALID_ARGUMENT" => Ok("ok"),
            Verdict::Sent => Ok("ok"),
            Verdict::Cred(c) => {
                if let Err(e) = store::set_cred_error(app, &c) {
                    log(&format!("{app}: credential error not stored: {e}"));
                }
                Err(c)
            }
            Verdict::Failed(c) | Verdict::Retry(c) | Verdict::Reauth(c) | Verdict::HoldJwt(c) | Verdict::Backoff(c, _) => Err(c),
        }
    }
}

/// What `push check` can't prove, per platform, in plain words.
pub const CHECK_LIMITS: &[(&str, &str)] = &[
    (
        store::PLATFORM_APNS,
        "Apple checks the key, the team and that the team may push to this bundle id. It can't tell from a test token \
         whether your phone app is built with this bundle id: the first real send does (a mismatch pauses sends and \
         sets the key error apns:DeviceTokenNotForTopic; nothing is deleted).",
    ),
    (
        store::PLATFORM_FCM,
        "Google checks the service account. It can't tell from a test token whether your phone app belongs to this \
         Firebase project: the first real send does (a mismatch pauses sends and sets the key error \
         fcm:SENDER_ID_MISMATCH; nothing is deleted).",
    ),
];

// ── the daemon's sender ──────────────────────────────────────────────

/// The daemon's one sender (the real transport, this build's hosts).
pub fn global() -> &'static Sender {
    static S: OnceLock<Sender> = OnceLock::new();
    S.get_or_init(|| {
        let transport: Arc<dyn Transport> = match ReqwestTransport::new() {
            Ok(t) => Arc::new(t),
            Err(e) => {
                log(&e);
                Arc::new(Unavailable)
            }
        };
        Sender::new(hosts(), transport)
    })
}

/// A transport that never sends (the HTTP client didn't start).
struct Unavailable;

impl Transport for Unavailable {
    fn post(&self, _: &HttpReq) -> Result<HttpResp, TransportErr> {
        Err(TransportErr::Network)
    }
}

#[cfg(test)]
fn test_transports() -> &'static Mutex<HashMap<String, Arc<dyn Transport>>> {
    static T: OnceLock<Mutex<HashMap<String, Arc<dyn Transport>>>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Tests: every send for `app` through the global sender goes to this fake
/// (apps are unique per test, so parallel tests never share one).
#[cfg(test)]
pub(crate) fn install_test_transport(app: &str, t: Arc<dyn Transport>) {
    test_transports().lock().unwrap_or_else(|e| e.into_inner()).insert(app.to_string(), t);
}

#[cfg(test)]
fn test_transport(app: &str) -> Option<Arc<dyn Transport>> {
    test_transports().lock().unwrap_or_else(|e| e.into_inner()).get(app).cloned()
}

#[cfg(test)]
#[path = "sender_tests.rs"]
mod tests;
