//! The sign-in door on an App's own hosts (0.46.9, prd-app-hosted-sign-in-v1
//! with its vs-live review V1–V33).
//!
//! An App's own page (or a native shell) posts a password to
//! `/.k2/auth/*` on the App's host. K2 answers that path before the App's
//! server sees a byte of it, and hands back a one-time code. The App's
//! server swaps the code, with its PKCE verifier, for the person's pass
//! ([`exchange_response`]), then sets its own cookie. K2 serves no pages.
//!
//! ```text
//! GET  /.k2/auth/ping             → {k2: true, app, v: 1, door}
//! POST /.k2/auth/sign-in          {username, password, challenge}            → {ok, code, expiresIn}
//! POST /.k2/auth/set-password     {token, password, challenge?}              → {ok, code?, expiresIn?}
//! POST /.k2/auth/change-password  {username, oldPassword, password, challenge} → {ok, code, expiresIn}
//!
//! POST /v1/apps/<service id>/auth/exchange  the App key (scope signin; users opens it)
//! POST /cli/skin/auth/exchange              the App's gateway link (catalog auth.exchange)
//!      {code, verifier, device?: {id, name}} → the login answer + mustChangePassword: false, authAt, via
//! ```
//!
//! **Where it answers** (V1, V4). Only on an App's HTTP/1.1 rewrite path:
//! the K2 Connect E2E listener on `<app>.<box>.k2.dev`, the App TLS door
//! (custom names, Path A and B) and the App's front port. The hook runs in
//! `client_ip::forward_doored` ahead of the gate, G2 and the local answers,
//! on **every** request of a connection, and on the first one before the
//! App is dialed (`apps::router`). A raw App (`host.clientIp: false` on its
//! k2.dev label or front port), a `--cmd` service's raw splice and anything
//! K2 doesn't front have no door: `ping` then gets the App's own answer, not
//! `{k2: true}`, and the docs say to send no password. A down App gets K2's
//! plain-text 503 before any door (no App, nothing to finish a sign-in).
//!
//! **The path** (V2). The door owns `/.k2/auth` and everything under it,
//! matched canonically ([`classify`]: decoded twice, `\` to `/`, any case,
//! dots and empty segments resolved, absolute-form stripped). Only the exact
//! verb paths are handled; anything else under it gets the door's `404`,
//! never the App.
//!
//! **The switch** (V32). Off by default for the box (`k2 app auth door
//! on`), and per App (`k2 app auth door off <app>`, V23: an App that owns
//! `/.k2/auth` itself). `K2_APP_AUTH_DOOR=1|0` overrides the box switch.
//!
//! **What a request must be.** `POST` with `Content-Type: application/json`
//! and a `Content-Length` body of at most [`MAX_BODY`] that arrives within
//! [`BODY_DEADLINE`] (chunked is 411, an upgrade 400, V3). A request
//! carrying `Origin` must come from the host it was sent to, or an origin
//! the App lists (`k2 app auth origins`, or its phone pairing's webview
//! origins); `null` only when listed (V10, V11). No `Origin` (a native app,
//! curl) passes. A listed origin gets `Access-Control-Allow-Origin` and the
//! door answers its preflight. No cookie is ever set; every answer is
//! `Cache-Control: no-store` and closes the connection.
//!
//! **Limits** (V7, V18). Its own buckets, before any password work: every
//! attempt per (App, visitor) and per App, failures per (App, person), per
//! App and box-wide, the person's 3-in-15-minutes login lockout (shared with
//! every other sign-in) and 500 ms on a failure. `set-password` uses the
//! reset limits. A connection without a visitor address (no PROXY header)
//! has no per-visitor bucket: the per-App ones still hold.
//!
//! **The code** (V12, V13). `k2lgn_` + 32 random bytes, kept in memory as a
//! digest for [`CODE_TTL`], bound to the App, the S256 challenge and the
//! password hash it was made with. One use. Another App's key gets
//! `bad_code` and the code stays; a wrong verifier uses it up. The exchange
//! mints only while the person's password is still that one. A restart or
//! a hand-off drops codes: during a hand-off the door answers `503 retry`
//! (V8).
//!
//! **Audit** (V9). One row per door request in the App's call audit
//! (`k2 app audit`): verb, status, code and the visitor; never the body.
//!
//! **Not here** (left for later, named so nobody guesses): enterprise SSO
//! (OIDC/SAML through the system browser) is a Rosson decision; a second
//! kind of [`DoorVerb`] and a second code minter are its extension point.
//! The box-host route for an App hosted elsewhere (S3), `k2 app validate`
//! warnings (S6) and passkeys are later slices.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use k2_core::skin::{self, app_login, auth_door as store, must_change, SkinLoginOutcome};
use serde_json::{json, Value as J};
use sha2::{Digest, Sha256};

use crate::cli_response::CliResponse;
use crate::client_ip::{DoorBody, DoorCall, DoorHook};

/// The door's namespace on an App's hosts.
pub const PREFIX: &str = "/.k2/auth";
pub const PING: &str = "/.k2/auth/ping";
pub const SIGN_IN: &str = "/.k2/auth/sign-in";
pub const SET_PASSWORD: &str = "/.k2/auth/set-password";
pub const CHANGE_PASSWORD: &str = "/.k2/auth/change-password";
/// The exchange on an App's gateway link (catalog `auth.exchange`).
pub const LINK_EXCHANGE: &str = "/cli/skin/auth/exchange";
/// The exchange's verb under `/v1/apps/<id>/` (the App key).
pub const KEY_EXCHANGE_VERB: &str = "auth/exchange";
/// Whether the door is on for the link's own App (catalog
/// `auth.doorStatus`, 0.46.11): a read on the gateway link, the App pass.
pub const LINK_STATUS: &str = "/cli/skin/auth/door";

/// The largest body the door reads (V3: the biggest legal one is well
/// under 1 KiB).
pub const MAX_BODY: u64 = 8 * 1024;
/// One deadline over the whole body (V3).
pub const BODY_DEADLINE: Duration = Duration::from_secs(10);
/// A one-time code's prefix: never a pass, key or token of another kind.
pub const CODE_PREFIX: &str = "k2lgn_";
/// How long a code lives.
pub const CODE_TTL: Duration = Duration::from_secs(60);
/// Most codes held at once (the oldest goes first past it).
pub const MAX_CODES: usize = 20_000;
/// Overrides the box switch: `1`/`on` or `0`/`off`.
pub const ENV_SWITCH: &str = "K2_APP_AUTH_DOOR";

/// Every attempt per (App, visitor) per window (V7: about 20 for shared
/// networks, recorded at admit, before any hash).
pub const ATTEMPTS_PER_VISITOR: usize = 20;
/// Every attempt per App per window (a flood breaker).
pub const ATTEMPTS_PER_APP: usize = 600;
/// Failures per (App, person) per window.
pub const FAILS_PER_PERSON: usize = crate::login_throttle::LIMIT;
/// Failures per App per window.
pub const FAILS_PER_APP: usize = 60;
/// Failures through every App's door per window.
pub const FAILS_ALL: usize = 300;
/// `set-password` attempts per (App, visitor) per window (the reset door's).
pub const SETS_PER_VISITOR: usize = crate::login_throttle::RESET_LIMIT;
/// `set-password` attempts per App per window.
pub const SETS_PER_APP: usize = crate::login_throttle::RESET_LINK_LIMIT;
/// The limiter's window, also every 429's `Retry-After`.
pub const WINDOW_SECS: i64 = crate::login_throttle::WINDOW_SECS;

// ── the path ─────────────────────────────────────────────────────────

/// What a request target is to the door.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoorVerb {
    Ping,
    SignIn,
    SetPassword,
    ChangePassword,
    /// Under the door but not a verb: the door's 404.
    Other,
}

impl DoorVerb {
    /// The audit's name.
    pub fn name(self) -> &'static str {
        match self {
            DoorVerb::Ping => "ping",
            DoorVerb::SignIn => "sign-in",
            DoorVerb::SetPassword => "set-password",
            DoorVerb::ChangePassword => "change-password",
            DoorVerb::Other => "other",
        }
    }
}

/// The verb an exact target names (`/.k2/auth/<verb>`, a query allowed).
fn exact(target: &str) -> Option<DoorVerb> {
    let path = target.split('?').next().unwrap_or("");
    match path {
        PING => Some(DoorVerb::Ping),
        SIGN_IN => Some(DoorVerb::SignIn),
        SET_PASSWORD => Some(DoorVerb::SetPassword),
        CHANGE_PASSWORD => Some(DoorVerb::ChangePassword),
        _ => None,
    }
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex_val(b[i + 1]), hex_val(b[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A target as a browser or a lenient server might read it: absolute-form
/// stripped, decoded twice, `\` as `/`, lowercased, `;params`, `.`, `..`
/// and empty segments resolved.
pub fn canonical(target: &str) -> String {
    let mut t = target.trim();
    if let Some(i) = t.find("://") {
        if !t[..i].contains('/') {
            let rest = &t[i + 3..];
            t = rest.find('/').map(|j| &rest[j..]).unwrap_or("/");
        }
    }
    let t = t.split(['?', '#']).next().unwrap_or("");
    let decoded = pct_decode(&pct_decode(t));
    let decoded = decoded.split(['?', '#']).next().unwrap_or("").replace('\\', "/").to_ascii_lowercase();
    let mut segs: Vec<&str> = Vec::new();
    for seg in decoded.split('/') {
        match seg.split(';').next().unwrap_or("") {
            "" | "." => {}
            ".." => {
                segs.pop();
            }
            s => segs.push(s),
        }
    }
    format!("/{}", segs.join("/"))
}

/// The decoded, lowercased path with empty and `.` segments dropped but
/// `..` kept: `/.k2/auth/../x` still starts with the door (a server may
/// route it anywhere, so the door claims it rather than guess).
fn squeezed(target: &str) -> String {
    let mut t = target.trim();
    if let Some(i) = t.find("://") {
        if !t[..i].contains('/') {
            let rest = &t[i + 3..];
            t = rest.find('/').map(|j| &rest[j..]).unwrap_or("/");
        }
    }
    let t = t.split(['?', '#']).next().unwrap_or("");
    let decoded = pct_decode(&pct_decode(t));
    let decoded = decoded.split(['?', '#']).next().unwrap_or("").replace('\\', "/").to_ascii_lowercase();
    let segs: Vec<&str> = decoded.split('/').map(|s| s.split(';').next().unwrap_or("")).filter(|s| !s.is_empty() && *s != ".").collect();
    format!("/{}", segs.join("/"))
}

/// Is `target` the door's at all (under `/.k2/auth`, read canonically or
/// before its `..` segments resolve)?
fn under_door(target: &str) -> bool {
    let lower = target.to_ascii_lowercase();
    // Fast path: no escape, no backslash and no `.k2` means no spelling of
    // the door (the common case on every App request).
    if !lower.contains('%') && !lower.contains('\\') && !lower.contains(".k2") {
        return false;
    }
    let c = canonical(target);
    let q = squeezed(target);
    c == PREFIX || c.starts_with("/.k2/auth/") || q == PREFIX || q.starts_with("/.k2/auth/")
}

/// What `target` is to the door; `None`: the App's.
pub fn classify(target: &str) -> Option<DoorVerb> {
    exact(target).or_else(|| under_door(target).then_some(DoorVerb::Other))
}

/// [`classify`] as the forwarder's yes/no.
pub fn claims(target: &str) -> bool {
    classify(target).is_some()
}

// ── the switch ───────────────────────────────────────────────────────

const SWITCH_TTL: Duration = Duration::from_secs(5);

pub fn env_override() -> Option<bool> {
    match std::env::var(ENV_SWITCH).ok()?.trim().to_ascii_lowercase().as_str() {
        "1" | "on" | "true" | "yes" => Some(true),
        "0" | "off" | "false" | "no" => Some(false),
        _ => None,
    }
}

fn box_cache() -> &'static Mutex<Option<(Instant, bool)>> {
    static C: OnceLock<Mutex<Option<(Instant, bool)>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// Is the door on for this box (the env override, else skin.db, read at
/// most every [`SWITCH_TTL`])?
pub fn box_on() -> bool {
    if let Some(v) = env_override() {
        return v;
    }
    let mut g = box_cache().lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, v)) = *g {
        if at.elapsed() < SWITCH_TTL {
            return v;
        }
    }
    let v = store::box_on().unwrap_or(false);
    *g = Some((Instant::now(), v));
    v
}

/// The runtime App's service id when the door is on for it.
type AppCache = Mutex<HashMap<String, (Instant, Option<String>)>>;

fn app_cache() -> &'static AppCache {
    static C: OnceLock<AppCache> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// Forget cached switch reads (after a change, so it applies at once).
pub fn invalidate() {
    app_cache().lock().unwrap_or_else(|e| e.into_inner()).clear();
    *box_cache().lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The service id of runtime App `app_id` when the door is on for it.
pub fn door_service(app_id: &str) -> Option<String> {
    if !box_on() {
        return None;
    }
    {
        let g = app_cache().lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, v)) = g.get(app_id) {
            if at.elapsed() < SWITCH_TTL {
                return v.clone();
            }
        }
    }
    let svc = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        k2_core::deploy::store::get_app(&conn, app_id).ok().flatten().map(|a| a.service_id)
    };
    let v = svc.filter(|s| !store::settings(s).map(|st| st.off).unwrap_or(true));
    let mut g = app_cache().lock().unwrap_or_else(|e| e.into_inner());
    if g.len() > 4096 {
        g.clear();
    }
    g.insert(app_id.to_string(), (Instant::now(), v.clone()));
    v
}

/// The door's hook for runtime App `app_id` on its rewrite path, or `None`
/// when the door is off for it (box or App).
pub fn hook_for(app_id: &str) -> Option<DoorHook> {
    door_service(app_id)?;
    Some(hook(app_id.to_string()))
}

/// The hook for `app_id`, unconditionally (tests drive it directly).
pub fn hook(app_id: String) -> DoorHook {
    DoorHook {
        claims,
        answer: Arc::new(move |call: DoorCall| {
            let id = app_id.clone();
            Box::pin(async move { answer(id, call).await })
        }),
        max_body: MAX_BODY,
        body_deadline: BODY_DEADLINE,
    }
}

// ── answers ──────────────────────────────────────────────────────────

fn status_line(code: u16) -> &'static str {
    match code {
        200 => "200 OK",
        204 => "204 No Content",
        400 => "400 Bad Request",
        401 => "401 Unauthorized",
        403 => "403 Forbidden",
        404 => "404 Not Found",
        405 => "405 Method Not Allowed",
        408 => "408 Request Timeout",
        411 => "411 Length Required",
        413 => "413 Payload Too Large",
        415 => "415 Unsupported Media Type",
        423 => "423 Locked",
        429 => "429 Too Many Requests",
        503 => "503 Service Unavailable",
        _ => "500 Internal Server Error",
    }
}

/// One answer before it is written.
#[derive(Debug, Clone)]
struct Out {
    status: u16,
    body: J,
    retry_after: Option<i64>,
    /// Counts toward the failure buckets (and earns the 500 ms).
    failed: bool,
    /// Extra header lines (`Allow: …`).
    extra: String,
    /// The person a code was made for (the audit's actor).
    principal: Option<String>,
}

impl Out {
    fn ok(body: J) -> Self {
        Out { status: 200, body, retry_after: None, failed: false, extra: String::new(), principal: None }
    }

    fn code(&self) -> Option<String> {
        self.body.get("error").and_then(|e| e.get("code")).and_then(J::as_str).map(str::to_string)
    }
}

fn refuse(status: u16, code: &str, hint: &str) -> Out {
    Out {
        status,
        body: json!({ "ok": false, "error": { "code": code, "hint": hint } }),
        retry_after: None,
        failed: false,
        extra: String::new(),
        principal: None,
    }
}

fn refuse_after(status: u16, code: &str, hint: &str, secs: i64) -> Out {
    let mut o = refuse(status, code, hint);
    o.body["error"]["retryAfterSecs"] = json!(secs);
    o.retry_after = Some(secs);
    o
}

fn failure(mut o: Out) -> Out {
    o.failed = true;
    o
}

fn bad_login() -> Out {
    failure(refuse(401, "bad_login", "that username or password is wrong"))
}

/// The whole HTTP answer. `head`: a HEAD request (no body bytes).
fn write(o: &Out, cors: Option<&str>, head: bool) -> Vec<u8> {
    let body = if o.status == 204 { String::new() } else { o.body.to_string() };
    let mut h = format!("HTTP/1.1 {}\r\n{}", status_line(o.status), k2_core::frame_policy::CONNECT_HEADER_LINES);
    if o.status != 204 {
        h.push_str("Content-Type: application/json\r\n");
    }
    h.push_str("Cache-Control: no-store\r\nX-K2-Door: auth\r\nX-Content-Type-Options: nosniff\r\n");
    if let Some(secs) = o.retry_after {
        h.push_str(&format!("Retry-After: {secs}\r\n"));
    }
    if let Some(origin) = cors {
        h.push_str(&format!("Access-Control-Allow-Origin: {origin}\r\nVary: Origin\r\n"));
    }
    h.push_str(&o.extra);
    h.push_str(&format!("Content-Length: {}\r\nConnection: close\r\n\r\n", body.len()));
    let mut out = h.into_bytes();
    if !head {
        out.extend_from_slice(body.as_bytes());
    }
    out
}

// ── origins ──────────────────────────────────────────────────────────

/// What `Origin` says about a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OriginIs {
    /// No `Origin` (a native app, curl, a server).
    Absent,
    /// The page is on the host the request was sent to.
    SameHost,
    /// An origin the App lists: answered with CORS.
    Listed(String),
    /// Anything else: refused `bad_origin`.
    Foreign,
}

/// Judge `origin` for a request to `host`, given the App's listed origins.
pub fn origin_is(origin: Option<&str>, host: Option<&str>, listed: &[String]) -> OriginIs {
    let Some(o) = origin.map(str::trim) else { return OriginIs::Absent };
    let lower = o.to_ascii_lowercase();
    if listed.iter().any(|l| *l == lower) {
        return OriginIs::Listed(lower);
    }
    if lower == "null" {
        return OriginIs::Foreign;
    }
    if let Some(h) = host.map(|h| h.trim().to_ascii_lowercase()).filter(|h| !h.is_empty()) {
        if lower == format!("https://{h}") || lower == format!("http://{h}") {
            return OriginIs::SameHost;
        }
    }
    OriginIs::Foreign
}

// ── the code store ───────────────────────────────────────────────────

/// What a code stands for.
#[derive(Debug, Clone)]
struct Entry {
    app: String,
    principal_id: String,
    challenge: String,
    hash: String,
    auth_at: i64,
    label: Option<String>,
    via: &'static str,
    made: Instant,
}

fn codes() -> &'static Mutex<HashMap<String, Entry>> {
    static C: OnceLock<Mutex<HashMap<String, Entry>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

fn digest(code: &str) -> String {
    Sha256::digest(code.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn s256(verifier: &str) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// A new one-time code for `e` (shown once).
fn mint_code(e: Entry) -> Result<String, String> {
    let mut b = [0u8; 32];
    getrandom::getrandom(&mut b).map_err(|e| format!("random: {e}"))?;
    let code = format!("{CODE_PREFIX}{}", b.iter().map(|x| format!("{x:02x}")).collect::<String>());
    let mut g = codes().lock().unwrap_or_else(|p| p.into_inner());
    g.retain(|_, v| v.made.elapsed() < CODE_TTL);
    while g.len() >= MAX_CODES {
        let Some(oldest) = g.iter().min_by_key(|(_, v)| v.made).map(|(k, _)| k.clone()) else { break };
        g.remove(&oldest);
    }
    g.insert(digest(&code), e);
    Ok(code)
}

/// Take the code for `app` with `verifier`. `None`: unknown, expired, used,
/// another App's (not used up then) or the wrong verifier (used up).
fn take_code(code: &str, app: &str, verifier: &str) -> Option<Entry> {
    let key = digest(code.trim());
    let mut g = codes().lock().unwrap_or_else(|p| p.into_inner());
    g.retain(|_, v| v.made.elapsed() < CODE_TTL);
    if g.get(&key)?.app != app {
        return None;
    }
    let e = g.remove(&key)?;
    let want = s256(verifier);
    if subtle::ConstantTimeEq::ct_eq(want.as_bytes(), e.challenge.as_bytes()).into() {
        Some(e)
    } else {
        None
    }
}

/// Codes held now (tests).
pub fn codes_held() -> usize {
    codes().lock().unwrap_or_else(|p| p.into_inner()).len()
}

/// Tests: age every held code by `by` (expiry without sleeping).
pub fn age_codes_for_tests(by: Duration) {
    let mut g = codes().lock().unwrap_or_else(|p| p.into_inner());
    for v in g.values_mut() {
        v.made = v.made.checked_sub(by).unwrap_or(v.made);
    }
}

/// An S256 challenge: exactly 43 base64url characters (V11b).
pub fn valid_challenge(c: &str) -> bool {
    c.len() == 43 && c.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A PKCE verifier: 43 to 128 of `[A-Za-z0-9-._~]` (RFC 7636).
pub fn valid_verifier(v: &str) -> bool {
    (43..=128).contains(&v.len()) && v.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~'))
}

// ── the limiter ──────────────────────────────────────────────────────

fn limiter() -> &'static crate::login_throttle::LoginLimiter {
    static L: OnceLock<crate::login_throttle::LoginLimiter> = OnceLock::new();
    L.get_or_init(crate::login_throttle::LoginLimiter::new)
}

/// Tests: drop every door bucket.
pub fn reset_limits_for_tests() {
    limiter().reset();
}

struct Keys {
    visitor: Option<String>,
    app: String,
    app_fails: String,
    all_fails: String,
    person: Option<String>,
}

fn keys(svc: &str, visitor: Option<IpAddr>, person: Option<&str>, set: bool) -> Keys {
    // The skin table for sign-in buckets, the reset table for set-password.
    let p = if set { crate::login_throttle::RESET_KEY_PREFIX } else { crate::login_throttle::SKIN_KEY_PREFIX };
    Keys {
        visitor: visitor.map(|ip| format!("{p}door:{svc}:v:{}", super::guard::visitor_key(ip))),
        app: format!("{p}door:{svc}"),
        app_fails: format!("{p}door-fails:{svc}"),
        all_fails: format!("{p}door-fails:*"),
        person: person.map(|u| format!("{p}door:{svc}:u:{}", k2_core::skin::login_throttle_subject(u))),
    }
}

fn limited() -> Out {
    refuse_after(429, "rate_limited", "too many sign-in attempts: wait a few minutes", WINDOW_SECS)
}

/// Admit a sign-in or change-password attempt (records it), before any hash.
fn admit_login(k: &Keys) -> Result<(), Out> {
    let l = limiter();
    let now = l.now();
    if let Some(v) = k.visitor.as_deref() {
        if l.check_and_record_with_limit(v, ATTEMPTS_PER_VISITOR, now) == crate::login_throttle::Verdict::Limited {
            return Err(limited());
        }
    }
    if let Some(p) = k.person.as_deref() {
        if l.is_limited(p, FAILS_PER_PERSON, now) {
            return Err(limited());
        }
    }
    if l.is_limited(&k.app_fails, FAILS_PER_APP, now) || l.is_limited(&k.all_fails, FAILS_ALL, now) {
        return Err(limited());
    }
    if l.check_and_record_with_limit(&k.app, ATTEMPTS_PER_APP, now) == crate::login_throttle::Verdict::Limited {
        return Err(limited());
    }
    Ok(())
}

fn record_failure(k: &Keys) {
    let l = limiter();
    let now = l.now();
    if let Some(p) = k.person.as_deref() {
        l.record_with_limit(p, FAILS_PER_PERSON, now);
    }
    l.record_with_limit(&k.app_fails, FAILS_PER_APP, now);
    l.record_with_limit(&k.all_fails, FAILS_ALL, now);
}

/// Admit a set-password attempt (the reset door's limits).
fn admit_set(k: &Keys) -> Result<(), Out> {
    let l = limiter();
    let now = l.now();
    if let Some(v) = k.visitor.as_deref() {
        if l.check_and_record_with_limit(v, SETS_PER_VISITOR, now) == crate::login_throttle::Verdict::Limited {
            return Err(limited());
        }
    }
    if l.check_and_record_with_limit(&k.app, SETS_PER_APP, now) == crate::login_throttle::Verdict::Limited {
        return Err(limited());
    }
    Ok(())
}

// ── the verbs (blocking) ─────────────────────────────────────────────

/// What a verb needs beyond its body.
struct Ctx {
    svc: String,
    label: Option<String>,
}

fn text<'a>(v: &'a J, k: &str) -> Option<&'a str> {
    v.get(k).and_then(J::as_str)
}

fn challenge_of(v: &J, required: bool) -> Result<Option<String>, Out> {
    match v.get("challenge") {
        None | Some(J::Null) if !required => Ok(None),
        Some(J::String(c)) if valid_challenge(c) => Ok(Some(c.clone())),
        _ => Err(refuse(
            400,
            "challenge_required",
            "send challenge: the S256 PKCE challenge your server made for this attempt (43 base64url characters)",
        )),
    }
}

fn new_password(v: &J) -> Result<String, Out> {
    let pw = text(v, "password").unwrap_or("");
    if pw.is_empty() {
        return Err(refuse(400, "bad_request", "send password"));
    }
    if pw.len() < skin::PUBLIC_PASSWORD_MIN_LEN {
        return Err(refuse(400, "password_too_short", "a password is at least 8 characters"));
    }
    if skin::password_too_long(pw.trim()) {
        return Err(refuse(400, "password_too_long", "a password is at most 256 bytes"));
    }
    Ok(pw.to_string())
}

fn code_answer(ctx: &Ctx, principal_id: &str, challenge: String, hash: String, via: &'static str) -> Out {
    let e = Entry {
        app: ctx.svc.clone(),
        principal_id: principal_id.to_string(),
        challenge,
        hash,
        auth_at: now_secs(),
        label: ctx.label.clone(),
        via,
        made: Instant::now(),
    };
    match mint_code(e) {
        Ok(code) => {
            let mut o = Out::ok(json!({ "ok": true, "code": code, "expiresIn": CODE_TTL.as_secs() }));
            o.principal = Some(principal_id.to_string());
            o
        }
        Err(e) => refuse(500, "failed", &e),
    }
}

/// `password_change_required`: a right password someone else chose. The
/// change token is minted only where a session would be (no grant: the
/// same bad_login as a wrong password).
fn must_change_answer(ctx: &Ctx, username: &str) -> Out {
    match skin::test_guests::with_link_login(Some(&ctx.svc), || must_change::start(username, &ctx.svc)) {
        Ok(s) => {
            let mut o = refuse(
                403,
                crate::skin_routes::PASSWORD_CHANGE_REQUIRED,
                "someone else set this password: show your set-password form and post {token: changeToken, password, challenge} to /.k2/auth/set-password",
            );
            o.body["mustChangePassword"] = json!(true);
            o.body["changeToken"] = json!(s.token);
            o.body["changeExpiresAt"] = json!(s.expires_at);
            o.body["error"]["changeToken"] = json!(s.token);
            o.body["error"]["changeExpiresAt"] = json!(s.expires_at);
            o
        }
        Err(e) if e == skin::HOST_LOGIN_DENIED => bad_login(),
        Err(e) => refuse(500, "failed", &e),
    }
}

fn dry_run(ctx: &Ctx, username: &str) -> Result<(), String> {
    skin::test_guests::with_link_login(Some(&ctx.svc), || store::dry_run(username, &ctx.svc))
}

fn sign_in(ctx: &Ctx, v: &J) -> Out {
    let username = text(v, "username").unwrap_or("").trim().to_string();
    let password = text(v, "password").unwrap_or("").to_string();
    if username.is_empty() || password.is_empty() {
        return refuse(400, "bad_request", "send username and password");
    }
    let challenge = match challenge_of(v, true) {
        Ok(c) => c.unwrap_or_default(),
        Err(o) => return o,
    };
    match skin::check_and_record_login_hashed(&username, &password) {
        (SkinLoginOutcome::Ok(p), Some(hash)) => match dry_run(ctx, &p.username) {
            Ok(()) => code_answer(ctx, &p.id, challenge, hash, "sign-in"),
            Err(e) if e == skin::HOST_LOGIN_DENIED => bad_login(),
            Err(e) => refuse(500, "failed", &e),
        },
        (SkinLoginOutcome::MustChange(p), _) => must_change_answer(ctx, &p.username),
        (SkinLoginOutcome::LockedOut { retry_after_secs }, _) => {
            failure(refuse_after(423, "locked", "too many wrong passwords for this person: try again later", retry_after_secs))
        }
        _ => bad_login(),
    }
}

fn set_password(ctx: &Ctx, v: &J) -> Out {
    let Some(token) = text(v, "token").map(str::trim).filter(|t| !t.is_empty()) else {
        return refuse(400, "bad_request", "send token (the k2inv_ or k2chg_ token your page got) and password");
    };
    let password = match new_password(v) {
        Ok(p) => p,
        Err(o) => return o,
    };
    let challenge = match challenge_of(v, false) {
        Ok(c) => c,
        Err(o) => return o,
    };
    let bad_token = || failure(refuse(401, "bad_token", "that link is used, expired, or not for this App: ask for a new one"));
    // V15: the token's App must be this door's; the owner's reset and the
    // older reset store never open it. Nothing is used up on a refusal.
    let target = match store::token_target(token) {
        Ok(Some(t)) if t.app == ctx.svc && !t.owner_reset => t,
        Ok(_) => return bad_token(),
        Err(e) => return refuse(500, "failed", &e),
    };
    let used = if token.starts_with(must_change::CHANGE_PREFIX) {
        match must_change::consume(token, &password) {
            Ok(must_change::ChangeConsume::Ok) => Ok(()),
            Ok(must_change::ChangeConsume::SamePassword) => {
                return refuse(400, "password_unchanged", "choose a new password, not the temporary one you were given")
            }
            Ok(must_change::ChangeConsume::BadToken) => return bad_token(),
            Err(e) => Err(e),
        }
    } else {
        match skin::invites::consume(token, &password) {
            Ok(skin::invites::Consume::Ok) => Ok(()),
            Ok(skin::invites::Consume::BadToken) => return bad_token(),
            Err(e) => Err(e),
        }
    };
    if let Err(e) = used {
        if e == skin::PASSWORD_TOO_LONG {
            return refuse(400, "password_too_long", "a password is at most 256 bytes");
        }
        return refuse(500, "failed", &e);
    }
    let Some(challenge) = challenge else { return Out::ok(json!({ "ok": true })) };
    // Q8 (V15): the password is set either way; a code only where a
    // session would be minted.
    if dry_run(ctx, &target.username).is_err() {
        return Out::ok(json!({ "ok": true }));
    }
    match store::password_hash_of(&target.principal_id) {
        Ok(Some(hash)) => code_answer(ctx, &target.principal_id, challenge, hash, "set-password"),
        Ok(None) => Out::ok(json!({ "ok": true })),
        Err(e) => refuse(500, "failed", &e),
    }
}

fn change_password(ctx: &Ctx, v: &J) -> Out {
    let username = text(v, "username").unwrap_or("").trim().to_string();
    let old = text(v, "oldPassword").unwrap_or("").to_string();
    if username.is_empty() || old.is_empty() {
        return refuse(400, "bad_request", "send username, oldPassword and password");
    }
    let password = match new_password(v) {
        Ok(p) => p,
        Err(o) => return o,
    };
    let challenge = match challenge_of(v, true) {
        Ok(c) => c.unwrap_or_default(),
        Err(o) => return o,
    };
    if password == old {
        return refuse(400, "password_unchanged", "choose a password that differs from the current one");
    }
    // Q7: wrong old passwords count toward the login lockout here (there is
    // no session to key on); `password.change` with a pass keeps its own.
    match skin::check_and_record_login_hashed(&username, &old) {
        (SkinLoginOutcome::Ok(p), Some(_)) => {
            // V17: a grant on this App first, or no write at all.
            match dry_run(ctx, &p.username) {
                Ok(()) => {}
                Err(e) if e == skin::HOST_LOGIN_DENIED => return bad_login(),
                Err(e) => return refuse(500, "failed", &e),
            }
            if let Err(e) = skin::set_principal_password(&p.username, Some(&password)) {
                return refuse(500, "failed", &e);
            }
            match store::password_hash_of(&p.id) {
                Ok(Some(hash)) => code_answer(ctx, &p.id, challenge, hash, "change-password"),
                Ok(None) => refuse(500, "failed", "the new password wasn't stored"),
                Err(e) => refuse(500, "failed", &e),
            }
        }
        // V17: a temporary password goes the must-change way, never here.
        (SkinLoginOutcome::MustChange(p), _) => must_change_answer(ctx, &p.username),
        (SkinLoginOutcome::LockedOut { retry_after_secs }, _) => {
            failure(refuse_after(423, "locked", "too many wrong passwords for this person: try again later", retry_after_secs))
        }
        _ => bad_login(),
    }
}

// ── one door request ─────────────────────────────────────────────────

/// The runtime App `app_id`'s service id and whether it is suspended.
fn app_facts(app_id: &str) -> Option<(String, bool)> {
    let h = k2_core::db::shared();
    let conn = h.lock();
    let app = k2_core::deploy::store::get_app(&conn, app_id).ok().flatten()?;
    let suspended = super::grants::is_suspended(&conn, app_id).unwrap_or(true);
    Some((app.service_id, suspended))
}

/// Answer one door request for runtime App `app_id`.
pub async fn answer(app_id: String, call: DoorCall) -> Vec<u8> {
    let started = Instant::now();
    let verb = classify(&call.path).unwrap_or(DoorVerb::Other);
    let head = call.method.eq_ignore_ascii_case("HEAD");
    let bytes_in = match &call.body {
        DoorBody::Bytes(b) => b.len() as u64,
        _ => 0,
    };
    let (out, cors) = answer_inner(&app_id, &call, verb).await;
    if out.failed {
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let bytes = write(&out, cors.as_deref(), head);
    super::audit::record(k2_core::app_audit::Call {
        at_ms: chrono::Utc::now().timestamp_millis(),
        app_id: app_id.clone(),
        seed: None,
        actor: out.principal.as_ref().map(|p| format!("guest:{p}")).unwrap_or_else(|| "remote".into()),
        on_behalf_of: Some(format!("visitor:{}", call.visitor.map(|ip| super::guard::visitor_key(ip).to_string()).unwrap_or_else(|| "unknown".into()))),
        verb: format!("{} {PREFIX}/{}", call.method.to_ascii_uppercase(), verb.name()),
        room: None,
        result: out.status,
        code: out.code(),
        bytes_in,
        bytes_out: bytes.len() as u64,
        latency_ms: started.elapsed().as_millis() as u64,
    });
    bytes
}

async fn answer_inner(app_id: &str, call: &DoorCall, verb: DoorVerb) -> (Out, Option<String>) {
    let method = call.method.to_ascii_uppercase();
    if verb == DoorVerb::Other {
        return (refuse(404, "not_found", "no such sign-in verb: /.k2/auth/sign-in, set-password, change-password or ping"), None);
    }
    // The App and its listed origins (one read per door request).
    let Some((svc, suspended)) = app_facts(app_id) else {
        return (refuse(404, "not_found", "no App here"), None);
    };
    let listed = store::allowed_origins(&svc).unwrap_or_default();
    let origin = origin_is(call.origin.as_deref(), call.host.as_deref(), &listed);
    let cors = match &origin {
        OriginIs::Listed(o) => Some(o.clone()),
        _ => None,
    };
    if verb == DoorVerb::Ping {
        if method != "GET" && method != "HEAD" {
            let mut o = refuse(405, "method_not_allowed", "ping is GET");
            o.extra = "Allow: GET, HEAD\r\n".into();
            return (o, cors);
        }
        return (Out::ok(json!({ "k2": true, "app": app_id, "v": 1, "door": PREFIX })), cors);
    }
    if method == "OPTIONS" {
        // A preflight: only for a listed origin.
        return match &origin {
            OriginIs::Listed(_) => {
                let mut o = Out::ok(J::Null);
                o.status = 204;
                o.extra = "Access-Control-Allow-Methods: POST\r\nAccess-Control-Allow-Headers: content-type\r\nAccess-Control-Max-Age: 600\r\n".into();
                (o, cors)
            }
            _ => (refuse(403, "bad_origin", "this page's origin isn't one the App lists (k2 app auth origins)"), None),
        };
    }
    if method != "POST" {
        let mut o = refuse(405, "method_not_allowed", "the sign-in verbs are POST");
        o.extra = "Allow: POST, OPTIONS\r\n".into();
        return (o, cors);
    }
    let body = match &call.body {
        DoorBody::Bytes(b) => b,
        DoorBody::TooLarge => return (refuse(413, "too_large", "a sign-in body is at most 8 KiB"), cors),
        DoorBody::LengthRequired => return (refuse(411, "length_required", "send the body with Content-Length (chunked isn't taken)"), cors),
        DoorBody::Timeout => return (refuse(408, "bad_request", "the body didn't arrive in time"), cors),
        DoorBody::Cut | DoorBody::Upgrade => return (refuse(400, "bad_request", "a sign-in is one JSON POST"), cors),
    };
    if matches!(origin, OriginIs::Foreign) {
        return (refuse(403, "bad_origin", "this page's origin isn't this host or one the App lists (k2 app auth origins)"), None);
    }
    let json_ct = call.content_type.as_deref().map(|c| c.split(';').next().unwrap_or("").trim().eq_ignore_ascii_case("application/json")).unwrap_or(false);
    if !json_ct {
        return (refuse(415, "bad_request", "send Content-Type: application/json"), cors);
    }
    // V8: mid hand-off this process's codes would be stranded.
    if crate::handoff::is_quiesced() {
        return (refuse_after(503, "retry", "K2 is restarting: try again in a second", 1), cors);
    }
    if suspended {
        return (refuse(403, "app_suspended", "this App is suspended: its owner resumes it"), cors);
    }
    let v: J = match serde_json::from_slice(body) {
        Ok(v @ J::Object(_)) => v,
        _ => return (refuse(400, "bad_request", "the body is a JSON object"), cors),
    };
    let set = verb == DoorVerb::SetPassword;
    let person = if set { None } else { text(&v, "username").map(str::to_string) };
    let k = keys(&svc, call.visitor, person.as_deref(), set);
    let admitted = if set { admit_set(&k) } else { admit_login(&k) };
    if let Err(o) = admitted {
        return (o, cors);
    }
    let ctx = Ctx { svc: svc.clone(), label: call.user_agent.as_deref().and_then(skin::profile::label_from_user_agent) };
    let out = tokio::task::spawn_blocking(move || match verb {
        DoorVerb::SignIn => sign_in(&ctx, &v),
        DoorVerb::SetPassword => set_password(&ctx, &v),
        DoorVerb::ChangePassword => change_password(&ctx, &v),
        DoorVerb::Ping | DoorVerb::Other => refuse(404, "not_found", "no such verb"),
    })
    .await
    .unwrap_or_else(|e| refuse(500, "failed", &format!("worker: {e}")));
    if out.failed && !set {
        record_failure(&k);
    }
    (out, cors)
}

// ── the exchange ─────────────────────────────────────────────────────

fn ex_err(status: &'static str, code: &str, hint: &str) -> CliResponse {
    CliResponse { status, content_type: "application/json", body: json!({ "ok": false, "error": { "code": code, "hint": hint } }).to_string() }
}

fn bad_code() -> CliResponse {
    ex_err("401 Unauthorized", "bad_code", "that code is unknown, used, expired, for another App, or the verifier doesn't match: sign in again")
}

/// Swap a door code for the person's pass, for the App `svc` (its service
/// id: the App key's App or the link's). Blocking.
pub fn exchange_response(svc: &str, body: &[u8]) -> CliResponse {
    let v: J = match serde_json::from_slice(body) {
        Ok(v @ J::Object(_)) => v,
        _ => return ex_err("400 Bad Request", "usage", "the body is a JSON object {code, verifier, device?}"),
    };
    let (Some(code), Some(verifier)) = (text(&v, "code"), text(&v, "verifier")) else {
        return ex_err("400 Bad Request", "usage", "send code (from the sign-in door) and verifier (your PKCE verifier)");
    };
    let device = match app_login::parse_device(v.get("device")) {
        Ok(d) => d,
        Err(e) => return ex_err("400 Bad Request", "usage", &e),
    };
    if !code.trim().starts_with(CODE_PREFIX) || !valid_verifier(verifier) {
        return bad_code();
    }
    let Some(e) = take_code(code, svc, verifier) else { return bad_code() };
    // V9: a suspended App signs nobody in.
    let suspended = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        match k2_core::deploy::store::get_app_by_service(&conn, svc) {
            Ok(Some(app)) => super::grants::is_suspended(&conn, &app.app_id).unwrap_or(true),
            _ => false,
        }
    };
    if suspended {
        return ex_err("403 Forbidden", "app_suspended", "this App is suspended: its owner resumes it");
    }
    let minted = skin::test_guests::with_link_login(Some(svc), || store::mint(&e.principal_id, svc, &e.hash, device.as_ref()));
    let mut out = match minted {
        Ok(store::Minted::Device(d)) => {
            let mut body = crate::app_mobile_routes::device_session_json(&d, &e.principal_id);
            body["rooms"] = json!(d.session.meta.rooms);
            body["roomHandles"] = json!(d.session.meta.room_handles);
            body["caps"] = json!(d.session.meta.caps);
            body["role"] = json!(d.session.role_name);
            body["roomAccess"] = json!(d.session.meta.room_access);
            skin::profile::record_session(&d.session.meta.id, skin::profile::VIA_DEVICE, Some(&d.device.name));
            body
        }
        Ok(store::Minted::Plain(s)) => {
            skin::profile::record_session(&s.meta.id, skin::profile::VIA_PASSWORD, e.label.as_deref());
            json!({
                "ok": true,
                "token": s.raw,
                "username": s.meta.name,
                "principalId": e.principal_id,
                "rooms": s.meta.rooms,
                "roomHandles": s.meta.room_handles,
                "caps": s.meta.caps,
                "role": s.role_name,
                "roomAccess": s.meta.room_access,
                "app": s.app,
            })
        }
        // V12: the password changed, must-change came on, or the grant went.
        Err(err) if err == skin::HOST_LOGIN_DENIED => return bad_code(),
        Err(err) => return ex_err("500 Internal Server Error", "failed", &err),
    };
    out["mustChangePassword"] = json!(false);
    // The recent-auth stamp: when K2 checked the password (a pass this
    // young needs no password again for pairing.start).
    out["authAt"] = json!(e.auth_at);
    out["via"] = json!(e.via);
    CliResponse::ok_json(out.to_string())
}

// ── the link's status read ───────────────────────────────────────────

/// `auth.doorStatus` for runtime App `app_id` (service `svc`): the link's
/// own App, never one the caller names. `on` is exactly what the door does
/// now ([`door_service`]); each public host says whether `/.k2/auth/*`
/// answers there (a raw k2.dev label has no door; a custom name always
/// has it while `on`). Blocking.
pub fn door_status_response(app_id: &str, svc: &str) -> CliResponse {
    let st = match store::settings(svc) {
        Ok(s) => s,
        Err(e) => return ex_err("500 Internal Server Error", "failed", &e),
    };
    let on = door_service(app_id).is_some();
    let label_door = crate::app_pairing::front_rewrites(svc).is_ok();
    let sub = crate::apps::origins::box_subdomain();
    let suffix = (!sub.is_empty()).then(|| format!(".{sub}.{}", k2_core::tunnel::config::SUBDOMAIN_HOST));
    let hosts: Vec<J> = crate::app_pairing::pairing_hosts(svc)
        .into_iter()
        .map(|h| {
            let label = suffix.as_deref().is_some_and(|s| h.ends_with(s));
            json!({ "host": h, "door": on && (!label || label_door) })
        })
        .collect();
    CliResponse::ok_json(
        json!({
            "ok": true,
            "on": on,
            "app": app_id,
            "box": box_on(),
            "appOff": st.off,
            "door": PREFIX,
            "hosts": hosts,
        })
        .to_string(),
    )
}

/// `auth.doorStatus` from a guest's session: the App's own data is the
/// App pass's only (like `app.self`).
pub fn door_status_not_exposed() -> CliResponse {
    ex_err(
        "403 Forbidden",
        "not_exposed",
        "auth.doorStatus answers this App's own pass on its gateway link (k2.auth.doorStatus or asApp), never a guest",
    )
}

#[cfg(test)]
#[path = "auth_door_tests.rs"]
mod tests;
