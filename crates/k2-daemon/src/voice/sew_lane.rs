//! The native call engine's door to Sew (prd-k2-voice-rooms-v1 §14.11).
//!
//! `https://<label>.k2.dev/_k2/voice/sew/<route>` → Sew's loopback client
//! listener, prefix stripped. A call's engine (`sew-call-client`, in the
//! desktop app) uses it for its own key traffic, leave, mute state, stats and
//! credential renewal, with the call-scoped credential Sew minted in the join
//! answer. Sew's client API stays loopback-only; this is the one way in, and
//! only for those routes.
//!
//! **On App addresses too** (0.46.9): `https://<app>.<label>.k2.dev/_k2/voice/sew/`
//! and the App's live custom names, for a third-party native app whose own
//! server mints the credential. Same allowlist, same credential rule, same
//! limits; only the switch differs ([`serve_app`]; the App's gate is
//! `apps::voice_door`).
//!
//! K2's rules, on top of Sew's own check of the credential's scope on every
//! request:
//! - off (404) on the box's label unless the voice rooms preview is on, and on
//!   an App's address unless that App's door is open (`voice_door_off`);
//! - anything not on Sew's allowlist answers 404 here and is never forwarded,
//!   `/api/v1/gateway/ticket` included;
//! - only a `sewcc_` bearer is accepted, and it is the only credential that
//!   crosses: K2 adds none, and drops cookies and every other header it
//!   doesn't list;
//! - `GET /api/v1/gateway` without a WebSocket upgrade is a 404 too; query
//!   strings pass through unchanged;
//! - a rate limit per credential, a cap on relays at once;
//! - no socket limit of K2's own: a second call socket on one credential
//!   passes through, and Sew takes it over (the first closes `4011`
//!   `session_revoked`, Sew's door spec rev 2).
//!
//! **Sew's web kit** (`@sew/voice` in a native shell) uses the same door
//! with the person's own K2 login as the bearer instead: on a kit route
//! (`kit_door::kit_route`), any bearer that isn't `sewcc_` goes to
//! [`super::kit_door`], which runs the route as that person with their own
//! Sew session. A non-`sewcc_` bearer on an engine-only route is still 401.
//!
//! The credential is never logged, and never appears in an answer K2 writes.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::json;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

use super::bff;
use super::kit_door::{self, Door, KitCtx};

/// The lane's path prefix (with its slash, so `/_k2/voice/sewx` isn't it).
pub const PREFIX: &str = "/_k2/voice/sew/";
const BEARER_PREFIX: &str = "sewcc_";
/// KeyPackages and commits are the largest bodies the engine sends.
pub const MAX_BODY: usize = 1024 * 1024;
const MAX_HEAD: usize = 16 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// One HTTP relay, start to end. The gateway socket has no limit.
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Relays at once, across all credentials (a socket counts for its whole life).
const MAX_RELAYS: usize = 256;
/// Per credential: a burst, then a steady rate.
const RATE_BURST: f64 = 60.0;
const RATE_PER_SEC: f64 = 5.0;
const RATE_TRACKED_MAX: usize = 4096;
const RATE_IDLE: Duration = Duration::from_secs(600);

/// The headers that cross, besides the bearer and the framing K2 writes.
const FORWARD_HTTP: &[&str] = &["content-type", "accept", "user-agent"];
const FORWARD_WS: &[&str] = &[
    "sec-websocket-key",
    "sec-websocket-version",
    "sec-websocket-protocol",
    "sec-websocket-extensions",
    "user-agent",
];

/// What an allowed route is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Http,
    Gateway,
}

/// True for a request target on this lane (a query is fine).
pub fn is_lane(path_and_query: &str) -> bool {
    path_and_query.starts_with(PREFIX)
}

fn id_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Sew's allowlist (§14.11), on the path with the prefix stripped.
pub fn allowed(method: &str, path: &str) -> Option<Route> {
    const CALL_KEYS: [&str; 8] = ["state", "commit", "ready", "invalid", "key_package", "release", "rekey", "report"];
    let segs: Vec<&str> = path.strip_prefix("/api/v1/")?.split('/').collect();
    let route = match (method, segs.as_slice()) {
        ("GET", ["gateway"]) => Route::Gateway,
        ("POST", ["devices"]) => Route::Http,
        ("POST", ["devices", id, "renew"]) if id_ok(id) => Route::Http,
        ("GET", ["devices", "crl"]) => Route::Http,
        ("GET", ["me", "devices"]) => Route::Http,
        ("POST", ["me", "voice", "keys", "key_packages" | "join"]) => Route::Http,
        ("POST", ["calls", id, "keys", which]) if id_ok(id) && CALL_KEYS.contains(which) => Route::Http,
        ("GET", ["me", "voice"]) => Route::Http,
        ("POST", ["me", "voice", "state" | "leave" | "stats" | "credential"]) => Route::Http,
        _ => return None,
    };
    Some(route)
}

/// The `sewcc_<selector>.<secret>` bearer of a request head, if it has that
/// shape. Only the shape: Sew decides whether it is any good.
pub fn sewcc_bearer(head: &str) -> Option<&str> {
    let t = crate::routes::http::extract_bearer(head)?;
    let rest = t.strip_prefix(BEARER_PREFIX)?;
    let ok = t.len() <= 512 && !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'~' | b'+' | b'/' | b'='));
    ok.then_some(t)
}

/// The part that names the credential; the secret stays out of K2's tables.
fn selector(bearer: &str) -> &str {
    let rest = bearer.strip_prefix(BEARER_PREFIX).unwrap_or(bearer);
    rest.split('.').next().unwrap_or(rest)
}

fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().skip(1).find_map(|line| {
        let (n, v) = line.split_once(':')?;
        n.trim().eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

fn wants_upgrade(head: &str) -> bool {
    header(head, "upgrade").is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
}

/// The request K2 sends Sew: the bearer, the listed headers and K2's framing.
fn upstream_head(method: &str, target: &str, host: &str, bearer: &str, head: &str, body_len: Option<usize>, ws: bool) -> String {
    let mut out = format!("{method} {target} HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {bearer}\r\n");
    for name in if ws { FORWARD_WS } else { FORWARD_HTTP } {
        // A control character in a value is never ours to pass on.
        if let Some(v) = header(head, name).filter(|v| !v.chars().any(char::is_control)) {
            out.push_str(&format!("{name}: {v}\r\n"));
        }
    }
    if ws {
        out.push_str("Connection: Upgrade\r\nUpgrade: websocket\r\n");
    } else {
        out.push_str(&format!("Content-Length: {}\r\nConnection: close\r\n", body_len.unwrap_or(0)));
    }
    out.push_str("\r\n");
    out
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

/// One request's worth of this credential's budget; false when it is spent.
fn take_token(sel: &str, now: Instant) -> bool {
    let mut g = buckets().lock().unwrap_or_else(|e| e.into_inner());
    if g.len() >= RATE_TRACKED_MAX && !g.contains_key(sel) {
        g.retain(|_, b| now.duration_since(b.at) < RATE_IDLE);
        if g.len() >= RATE_TRACKED_MAX {
            return false;
        }
    }
    let b = g.entry(sel.to_string()).or_insert(Bucket { tokens: RATE_BURST, at: now });
    b.tokens = (b.tokens + now.duration_since(b.at).as_secs_f64() * RATE_PER_SEC).min(RATE_BURST);
    b.at = now;
    if b.tokens < 1.0 {
        return false;
    }
    b.tokens -= 1.0;
    true
}

static RELAYS: AtomicUsize = AtomicUsize::new(0);

/// One relay at once of [`MAX_RELAYS`] (the kit lane counts in it too).
pub(super) struct RelaySlot;
impl RelaySlot {
    pub(super) fn take() -> Option<RelaySlot> {
        let n = RELAYS.fetch_add(1, Ordering::SeqCst);
        if n >= MAX_RELAYS {
            RELAYS.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(RelaySlot)
    }
}
impl Drop for RelaySlot {
    fn drop(&mut self) {
        RELAYS.fetch_sub(1, Ordering::SeqCst);
    }
}


// ── answers K2 writes itself ─────────────────────────────────────────

async fn answer<S: AsyncWrite + Unpin>(client: &mut S, status: &str, code: &str, hint: &str, retry_after: Option<u64>) {
    let body = json!({ "error": { "code": code, "hint": hint, "details": null } }).to_string();
    let retry = retry_after.map(|s| format!("Retry-After: {s}\r\n")).unwrap_or_default();
    let msg = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{retry}{frame}Connection: close\r\n\r\n{body}",
        body.len(),
        frame = k2_core::frame_policy::CONNECT_HEADER_LINES,
    );
    let _ = client.write_all(msg.as_bytes()).await;
    let _ = client.flush().await;
}

// ── the relay ────────────────────────────────────────────────────────

/// Serve one lane request whose head is still unread on `client` (the
/// dispatcher only peeked). `head_len` is the head's length with its blank
/// line. The connection closes afterwards.
/// `owner_token`: the box's, so the kit lane refuses it as join does.
pub async fn serve<S: AsyncRead + AsyncWrite + Unpin>(client: &mut S, head: &str, head_len: usize, owner_token: &str) {
    let kit = KitCtx::live(Door::Box { owner_token: owner_token.to_string() });
    serve_box(client, head, head_len, k2_core::voice_preview::enabled(), &crate::sew::ops::client_addr(), Some(&kit)).await;
}

/// The box label's door without the kit lane (tests of the engine lane).
#[cfg(test)]
async fn serve_to<S: AsyncRead + AsyncWrite + Unpin>(client: &mut S, head: &str, head_len: usize, preview_on: bool, sew_addr: &str) {
    serve_box(client, head, head_len, preview_on, sew_addr, None).await;
}

/// The box label's door: off while the voice rooms preview is.
pub(super) async fn serve_box<S: AsyncRead + AsyncWrite + Unpin>(
    client: &mut S,
    head: &str,
    head_len: usize,
    preview_on: bool,
    sew_addr: &str,
    kit: Option<&KitCtx>,
) {
    let off = (!preview_on).then_some(("voice_rooms_off", "no such route"));
    serve_door(client, head, head_len, off, sew_addr, kit).await;
}

/// The 404 code on an App's address whose door is shut.
pub const APP_DOOR_OFF: &str = "voice_door_off";
const APP_DOOR_OFF_HINT: &str = "no call door on this App's address: the App needs Sew's client API approved (box service sew-api), \
     voice:join approved if it asks for chat caps, and voice installed on this server (k2 study chat-voice)";

/// An App address's door (`apps::voice_door`): this lane, open only when
/// `open` (that App's gate). The allowlist, the credential rule and the limits
/// are the box label's.
/// `kit`: the kit lane for the App's people (`None`: engine lane only).
pub async fn serve_app<S: AsyncRead + AsyncWrite + Unpin>(
    client: &mut S,
    head: &str,
    head_len: usize,
    open: bool,
    sew_addr: &str,
    kit: Option<&KitCtx>,
) {
    let off = (!open).then_some((APP_DOOR_OFF, APP_DOOR_OFF_HINT));
    serve_door(client, head, head_len, off, sew_addr, kit).await;
}

/// `off`: the door is shut; answer 404 with this `(code, hint)`.
async fn serve_door<S: AsyncRead + AsyncWrite + Unpin>(
    client: &mut S,
    head: &str,
    head_len: usize,
    off: Option<(&str, &str)>,
    sew_addr: &str,
    kit: Option<&KitCtx>,
) {
    let mut consumed = vec![0u8; head_len];
    if client.read_exact(&mut consumed).await.is_err() {
        return;
    }
    let not_found = "no such route";
    let first = head.lines().next().unwrap_or("");
    let mut parts = first.split_whitespace();
    let (Some(method), Some(target)) = (parts.next(), parts.next()) else {
        return answer(client, "400 Bad Request", "usage", "bad request line", None).await;
    };
    if let Some((code, hint)) = off {
        return answer(client, "404 Not Found", code, hint, None).await;
    }
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (target, None),
    };
    let Some(sew_path) = path.strip_prefix(&PREFIX[..PREFIX.len() - 1]) else {
        return answer(client, "404 Not Found", "not_found", not_found, None).await;
    };
    // Sew's web kit: a kit route with the person's own login, not a call
    // credential (`kit_door`).
    if let Some(kit) = kit.filter(|_| sewcc_bearer(head).is_none()) {
        if kit_door::kit_route(method, sew_path, query).is_some() {
            return match crate::routes::http::extract_bearer(head) {
                Some(b) => kit_door::serve(client, method, sew_path, query, head, b, kit, sew_addr).await,
                None => {
                    let r = bff::Refusal::new(401, "unauthenticated", "send the person's own K2 login session (or a call credential on the engine's routes)");
                    kit_door::refuse(client, &r).await
                }
            };
        }
    }
    let Some(route) = allowed(method, sew_path) else {
        return answer(client, "404 Not Found", "not_found", not_found, None).await;
    };
    // The gateway is only ever a WebSocket: a plain GET isn't a route.
    if route == Route::Gateway && !wants_upgrade(head) {
        return answer(client, "404 Not Found", "not_found", not_found, None).await;
    }
    let Some(bearer) = sewcc_bearer(head) else {
        return answer(client, "401 Unauthorized", "call_credential_required", "this door takes a call credential", None).await;
    };
    let sel = selector(bearer);
    if !take_token(sel, Instant::now()) {
        return answer(client, "429 Too Many Requests", "rate_limited", "too many requests for this call", Some(2)).await;
    }
    let Some(_slot) = RelaySlot::take() else {
        return answer(client, "503 Service Unavailable", "voice_unavailable", "too many calls through this server right now", Some(2)).await;
    };
    let up_target = match query {
        Some(q) => format!("{sew_path}?{q}"),
        None => sew_path.to_string(),
    };
    match route {
        Route::Http => {
            let body_len = header(head, "content-length").and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
            if body_len > MAX_BODY {
                return answer(client, "413 Payload Too Large", "too_large", "request body too large", None).await;
            }
            let mut body = vec![0u8; body_len];
            match tokio::time::timeout(HTTP_TIMEOUT, client.read_exact(&mut body)).await {
                Ok(Ok(_)) => {}
                _ => return,
            }
            let out = upstream_head(method, &up_target, sew_addr, bearer, head, Some(body_len), false);
            relay_http(client, sew_addr, out.as_bytes(), &body).await;
        }
        Route::Gateway => {
            let out = upstream_head(method, &up_target, sew_addr, bearer, head, None, true);
            relay_gateway(client, sew_addr, out.as_bytes()).await;
        }
    }
}

async fn connect_sew(addr: &str) -> Option<TcpStream> {
    let sa: SocketAddr = addr.parse().ok()?;
    // Never out of this box, whatever a config file says.
    if !sa.ip().is_loopback() {
        k2_core::log_debug!("[voice/sew-lane] Sew's client listener {addr} is not loopback; refusing");
        return None;
    }
    tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(sa)).await.ok()?.ok()
}

async fn relay_http<S: AsyncRead + AsyncWrite + Unpin>(client: &mut S, addr: &str, head: &[u8], body: &[u8]) {
    let unreachable = "K2 could not reach this server's Sew";
    let Some(mut up) = connect_sew(addr).await else {
        return answer(client, "502 Bad Gateway", "voice_unavailable", unreachable, Some(2)).await;
    };
    if up.write_all(head).await.is_err() || up.write_all(body).await.is_err() {
        return answer(client, "502 Bad Gateway", "voice_unavailable", unreachable, Some(2)).await;
    }
    // Sew closes after its answer (`Connection: close`).
    let copied = tokio::time::timeout(HTTP_TIMEOUT, tokio::io::copy(&mut up, client)).await;
    if copied.is_err() {
        k2_core::log_debug!("[voice/sew-lane] an HTTP relay passed {}s", HTTP_TIMEOUT.as_secs());
    }
    let _ = client.flush().await;
}

async fn relay_gateway<S: AsyncRead + AsyncWrite + Unpin>(client: &mut S, addr: &str, head: &[u8]) {
    let unreachable = "K2 could not reach this server's Sew";
    let Some(mut up) = connect_sew(addr).await else {
        return answer(client, "502 Bad Gateway", "voice_unavailable", unreachable, Some(2)).await;
    };
    if up.write_all(head).await.is_err() {
        return answer(client, "502 Bad Gateway", "voice_unavailable", unreachable, Some(2)).await;
    }
    // Sew's handshake reply goes to the client as is, whatever it says; only a
    // 101 becomes a socket.
    let mut raw = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if raw.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if raw.len() > MAX_HEAD {
            return answer(client, "502 Bad Gateway", "voice_unavailable", "Sew's answer was too long", None).await;
        }
        match tokio::time::timeout(HANDSHAKE_TIMEOUT, up.read(&mut chunk)).await {
            Ok(Ok(n)) if n > 0 => raw.extend_from_slice(&chunk[..n]),
            _ => return answer(client, "502 Bad Gateway", "voice_unavailable", unreachable, Some(2)).await,
        }
    }
    let switching = raw.starts_with(b"HTTP/1.1 101");
    if client.write_all(&raw).await.is_err() {
        return;
    }
    let _ = client.flush().await;
    if !switching {
        // The rest of a refusal's body, if any, then close.
        let _ = tokio::time::timeout(HTTP_TIMEOUT, tokio::io::copy(&mut up, client)).await;
        return;
    }
    // No limit of K2's own: Sew decides when a second socket takes over and
    // closes the first itself, with its own close frame.
    let _ = tokio::io::copy_bidirectional(client, &mut up).await;
}

#[cfg(test)]
#[path = "sew_lane_tests.rs"]
mod tests;
