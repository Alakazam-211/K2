//! The vault's loopback proxy (prd-vault-for-agents-v1 A3, prd-k2-vault-v1
//! §9.3): an SDK or script gets a placeholder key and a loopback base URL,
//! and the real key never reaches it.
//!
//! - One listener on `127.0.0.1`, never another address, on a port kept in
//!   `<vault dir>/proxy.port` and re-bound after a daemon restart: sessions
//!   survive restarts in k2-ptyd and placeholders aren't dropped at boot, so
//!   their base URLs must keep working. If the port is taken the listener
//!   moves and status says so.
//! - `/p/<placeholder>/<provider>/<rest>`. Each request is checked afresh:
//!   loopback peer; the placeholder's hash and expiry; its session still
//!   live (or that session's process still running); its secret still resolving for that session for its one site.
//!   A revoked grant stops a live placeholder at the next call.
//! - The destination comes only from the secret's one site, never from the
//!   request. Nothing the caller sends can move it.
//! - The placeholder is never forwarded: headers holding it are dropped,
//!   and anywhere else in the request (body, query, path) is a 400. The
//!   whole request body is read before the first byte goes out, so that
//!   check can't come after part of it left.
//! - The secret goes only where its sealed injection says, with the same
//!   code as `k2 vault fetch`, through the same egress gate (vetting, IP
//!   pinning, no redirects but same-origin, re-vetted).
//! - Responses stream, passing through the redactor, so an echoed key comes
//!   back as `[k2v:NAME]`. Only the fetch response-header allowlist returns.
//! - Logs name a fixed refusal code and nothing else: [`log_refusal`] takes
//!   a `&'static str`, so a path, a placeholder or a value can't reach it.
//!
//! Not TLS interception, no CA, no `HTTPS_PROXY`: a tool that can't take a
//! base URL stays on env or file delivery.

use std::collections::{HashMap, HashSet};
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty, Full, LengthLimitError, Limited};
use hyper::body::{Body, Bytes, Frame};
use hyper::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use k2_vault::delivery::{default_env_name, validate_env_name};
use k2_vault::fetch::{
    inject_body, inject_edit, sign_edit, upstream_error, validate_method, InjectEdit, MAX_REDIRECTS, MAX_RESPONSE, RESPONSE_HEADER_ALLOWLIST,
};
use k2_vault::hosts::{canonical_ip, parse_stored, HostKind, Origin};
use k2_vault::inject::{is_forbidden_header, Sign};
use k2_vault::redact::Redactor;
use k2_vault::refs::PLACEHOLDER_PREFIX;
use k2_vault::{LocalVault, Requester, SecretBytes, SecretStore, Selector, Use, VaultError};
use parking_lot::{Mutex, RwLock};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, Semaphore};

use crate::egress::{self, Destination, Purpose};
use crate::mail::oauth::{EgressHttp, HttpClient};
use crate::vault::actor::VaultCaller;
use crate::vault::oauth_refresh;
use crate::vault_fetch::{follow_target, ovh_time, send_kind, set_query_param, Allowlist, Injection, RateLimiter};

/// Holds the listener's port, next to `vault.db`.
pub const PORT_FILE: &str = "proxy.port";
/// A stalled upstream ends after this long with no bytes; a long generation
/// keeps streaming.
const READ_IDLE: Duration = Duration::from_secs(600);
/// Open requests per session (PRD 4.2 step 8).
const MAX_STREAMS: usize = 32;
/// Placeholders one session may hold unexpired, so a loop can't fill the table.
const MAX_PLACEHOLDERS: u32 = 64;
/// Open connections to the listener, from everyone.
const MAX_CONNECTIONS: usize = 2048;
/// The largest request body. All of it is read first (see the module doc).
const MAX_REQUEST_BODY: usize = MAX_RESPONSE;
/// How long a vetted destination and its connection pool are reused.
const CLIENT_TTL: Duration = Duration::from_secs(60);
/// A daemon restart can overlap the old process's exit for a moment.
const BIND_TRIES: u32 = 10;
const BIND_RETRY: Duration = Duration::from_millis(200);

type ProxyBody = BoxBody<Bytes, io::Error>;

// ── providers (PRD 4.3) ─────────────────────────────────────────────

struct Provider {
    id: &'static str,
    host: &'static str,
    key_env: &'static str,
    base_env: &'static str,
    /// Added to the base URL (OpenAI's SDKs expect `/v1` in it).
    base_path: &'static str,
}

const PROVIDERS: &[Provider] = &[
    Provider { id: "anthropic", host: "api.anthropic.com", key_env: "ANTHROPIC_API_KEY", base_env: "ANTHROPIC_BASE_URL", base_path: "" },
    Provider { id: "openai", host: "api.openai.com", key_env: "OPENAI_API_KEY", base_env: "OPENAI_BASE_URL", base_path: "/v1" },
    Provider { id: "xai", host: "api.x.ai", key_env: "XAI_API_KEY", base_env: "XAI_BASE_URL", base_path: "" },
    Provider { id: "gemini", host: "generativelanguage.googleapis.com", key_env: "GEMINI_API_KEY", base_env: "GOOGLE_GEMINI_BASE_URL", base_path: "" },
];

const GENERIC: &str = "generic";

fn provider_for(origin: &Origin) -> Option<&'static Provider> {
    let HostKind::Domain(d) = &origin.host else { return None };
    if !origin.https || origin.port != 443 {
        return None;
    }
    PROVIDERS.iter().find(|p| p.host == d)
}

fn provider_id(origin: &Origin) -> &'static str {
    provider_for(origin).map_or(GENERIC, |p| p.id)
}

/// The env names and URL shape for a secret's one site. K2 picks the names
/// (the table, or the grant's own env name for `generic`), never the agent.
struct EnvPair {
    provider: &'static str,
    key_env: String,
    base_env: String,
    base_path: &'static str,
}

fn env_pair(origin: &Origin, grant_env: Option<&str>, secret_name: &str) -> Result<EnvPair, VaultError> {
    if let Some(p) = provider_for(origin) {
        return Ok(EnvPair { provider: p.id, key_env: p.key_env.to_string(), base_env: p.base_env.to_string(), base_path: p.base_path });
    }
    let key_env = grant_env.map_or_else(|| default_env_name(secret_name), str::to_string);
    validate_env_name(&key_env)?;
    let base_env = format!("{key_env}_BASE_URL");
    Ok(EnvPair { provider: GENERIC, key_env, base_env, base_path: "" })
}

fn base_url(port: u16, token: &str, pair: &EnvPair) -> String {
    format!("http://127.0.0.1:{port}/p/{token}/{}{}", pair.provider, pair.base_path)
}

/// The single exact site a secret may reach through the proxy.
fn single_origin(hosts: &[String]) -> Result<Origin, VaultError> {
    let pats = parse_stored(hosts);
    match (hosts.len(), pats.as_slice()) {
        (1, [p]) if !p.is_wildcard() => Ok(Origin::parse_url(&p.render())?.1),
        _ => Err(VaultError::bad_request("proxy_needs_one_host", "proxy-url needs a secret bound to one host; use k2 vault fetch")),
    }
}

// ── the listener ────────────────────────────────────────────────────

/// Where the listener is, for `k2 vault status` and `/boot-status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Listening {
    pub port: u16,
    /// The stored port was taken at boot, so older base URLs are dead.
    pub moved: bool,
}

static LISTENING: RwLock<Option<Listening>> = RwLock::new(None);
static STARTED: AtomicBool = AtomicBool::new(false);

pub fn listening() -> Option<Listening> {
    *LISTENING.read()
}

#[cfg(test)]
pub(crate) fn set_listening_for_test(l: Option<Listening>) {
    *LISTENING.write() = l;
}

/// The `proxy` object in `k2 vault status`.
pub fn status_json() -> Value {
    match listening() {
        Some(l) if l.moved => json!({
            "port": l.port,
            "moved": true,
            "hint": "proxy moved: re-run k2 vault proxy-url",
        }),
        Some(l) => json!({ "port": l.port, "moved": false }),
        None => json!({ "port": null, "moved": false, "hint": "the proxy isn't listening; k2 vault proxy-url can't be used" }),
    }
}

/// What the listener needs from the daemon. Tests pass their own.
#[derive(Clone)]
pub struct Deps {
    pub vault: Arc<dyn Fn() -> Result<Arc<LocalVault>, VaultError> + Send + Sync>,
    pub live: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    pub rate: &'static RateLimiter,
    streams: Streams,
    /// Where a refreshing sign-in's token POST goes (a stub in tests).
    oauth_http: Arc<dyn HttpClient>,
    reconnect_card: Arc<ReconnectCard>,
}

/// Post a reconnect card: (workspace, scope word, secret).
pub(crate) type ReconnectCard = dyn Fn(&str, &str, &k2_vault::SecretMeta) -> Result<(), String> + Send + Sync;

impl Deps {
    pub fn new(
        vault: Arc<dyn Fn() -> Result<Arc<LocalVault>, VaultError> + Send + Sync>,
        live: Arc<dyn Fn(&str) -> bool + Send + Sync>,
        rate: &'static RateLimiter,
    ) -> Deps {
        Deps {
            vault,
            live,
            rate,
            streams: Streams::default(),
            oauth_http: Arc::new(EgressHttp::default()),
            reconnect_card: Arc::new(oauth_refresh::post_live_card),
        }
    }

    /// Tests: a stub token endpoint and reconnect card.
    #[cfg(test)]
    pub(crate) fn with_oauth(mut self, http: Arc<dyn HttpClient>, card: Arc<ReconnectCard>) -> Deps {
        self.oauth_http = http;
        self.reconnect_card = card;
        self
    }
}

/// PRD 4.2 step 3: the session is listed as running, or the process that got
/// its secrets dir still is (GH#81: an id can leave the map while its process
/// runs, and that process's placeholders must keep working).
pub fn session_counts_as_live(session_id: &str) -> bool {
    crate::session_liveness::live_session_ids().contains(session_id) || super::spawn::session_process_lives(session_id)
}

fn daemon_deps() -> Deps {
    Deps::new(
        Arc::new(super::get),
        // A restored or moved vault is served by the same listener: `get`
        // reads the current one each call.
        Arc::new(session_counts_as_live),
        crate::vault_use_routes::rate(),
    )
}

/// Start the listener once for this daemon (the vault may open late).
pub fn start_once(vault_dir: PathBuf) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else { return };
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    handle.spawn(async move {
        match serve(&vault_dir, daemon_deps()).await {
            Ok(running) => {
                *LISTENING.write() = Some(running.listening);
                if running.listening.moved {
                    k2_core::log_debug!("WARN [vault-proxy] the stored port was taken; now on 127.0.0.1:{}", running.listening.port);
                } else {
                    k2_core::log_debug!("[vault-proxy] listening on 127.0.0.1:{}", running.listening.port);
                }
            }
            Err(e) => k2_core::log_debug!("WARN [vault-proxy] couldn't listen ({})", e.kind()),
        }
    });
}

/// A started listener.
pub struct Running {
    pub listening: Listening,
    #[cfg_attr(not(test), allow(dead_code))]
    task: tokio::task::JoinHandle<()>,
}

impl Running {
    /// Stop accepting and free the port (a daemon going away, in tests).
    #[cfg(test)]
    pub async fn stop(self) {
        self.task.abort();
        let _ = self.task.await;
    }
}

/// Bind `127.0.0.1` on the stored port, or a new one (`moved` when the
/// stored one was taken). Never any other address.
async fn bind_stable(dir: &Path) -> io::Result<(TcpListener, bool)> {
    let file = dir.join(PORT_FILE);
    let want: Option<u16> = std::fs::read_to_string(&file).ok().and_then(|s| s.trim().parse().ok()).filter(|p| *p != 0);
    let mut moved = false;
    let listener = match want {
        None => TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?,
        Some(port) => {
            let mut got = None;
            for attempt in 0..BIND_TRIES {
                if attempt > 0 {
                    tokio::time::sleep(BIND_RETRY).await;
                }
                match TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await {
                    Ok(l) => {
                        got = Some(l);
                        break;
                    }
                    Err(e) if e.kind() == io::ErrorKind::AddrInUse => continue,
                    Err(_) => break,
                }
            }
            match got {
                Some(l) => l,
                None => {
                    moved = true;
                    TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?
                }
            }
        }
    };
    let port = listener.local_addr()?.port();
    if want != Some(port) {
        std::fs::write(&file, format!("{port}\n"))?;
    }
    Ok((listener, moved))
}

/// Bind and start accepting.
pub async fn serve(vault_dir: &Path, deps: Deps) -> io::Result<Running> {
    let (listener, moved) = bind_stable(vault_dir).await?;
    let port = listener.local_addr()?.port();
    let conns = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    let task = tokio::spawn(async move {
        loop {
            let (sock, peer) = match listener.accept().await {
                Ok(x) => x,
                // Out of file descriptors and the like: don't spin.
                Err(_) => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                }
            };
            let Ok(permit) = conns.clone().try_acquire_owned() else { continue };
            if !peer_allowed(peer) {
                continue;
            }
            let deps = deps.clone();
            tokio::spawn(async move {
                let _permit = permit;
                let svc = service_fn(move |req| {
                    let deps = deps.clone();
                    async move { Ok::<_, std::convert::Infallible>(handle(&deps, peer, req).await) }
                });
                let _ = http1::Builder::new()
                    .timer(TokioTimer::new())
                    .header_read_timeout(Duration::from_secs(30))
                    .serve_connection(TokioIo::new(sock), svc)
                    .await;
            });
        }
    });
    Ok(Running { listening: Listening { port, moved }, task })
}

fn peer_allowed(peer: SocketAddr) -> bool {
    canonical_ip(peer.ip()).is_loopback()
}

// ── one request ─────────────────────────────────────────────────────

/// `/p/<placeholder>/<provider>/<rest>` → the three parts.
fn parse_path(path: &str) -> Option<(&str, &str, &str)> {
    let rest = path.strip_prefix("/p/")?;
    let (token, rest) = rest.split_once('/')?;
    let (provider, rest) = rest.split_once('/').unwrap_or((rest, ""));
    (!token.is_empty() && !provider.is_empty()).then_some((token, provider, rest))
}

/// Every refusal the listener logs goes through here. A `&'static str`
/// can't hold a path, a placeholder or a value.
fn log_refusal(code: &'static str) {
    #[cfg(test)]
    tests::LOG.lock().push(format!("[vault-proxy] refused: {code}"));
    k2_core::log_debug!("[vault-proxy] refused: {code}");
}

fn full(b: impl Into<Bytes>) -> ProxyBody {
    Full::new(b.into()).map_err(|n: std::convert::Infallible| match n {}).boxed()
}

fn empty() -> ProxyBody {
    Empty::<Bytes>::new().map_err(|n: std::convert::Infallible| match n {}).boxed()
}

fn refusal(e: &VaultError) -> Response<ProxyBody> {
    let status = StatusCode::from_u16(e.status).unwrap_or(StatusCode::BAD_GATEWAY);
    let body = json!({ "error": { "code": e.code, "hint": e.hint } }).to_string();
    let mut r = Response::new(full(body));
    *r.status_mut() = status;
    r.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    r
}

/// Answer one request.
async fn handle<B>(deps: &Deps, peer: SocketAddr, req: Request<B>) -> Response<ProxyBody>
where
    B: Body<Data = Bytes> + Send + 'static,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    match proxy_call(deps, peer, req).await {
        Ok(r) => r,
        Err(e) => {
            log_refusal(e.code);
            refusal(&e)
        }
    }
}

/// What a placeholder authorizes, right now.
struct Authorized {
    session_id: String,
    origin: Origin,
    injection: Injection,
    allow: Allowlist,
}

/// Steps 2–4 of PRD 4.2. Blocking (SQLite, the liveness snapshot).
fn authorize(deps: &Deps, token: &str, provider: &str) -> Result<Authorized, VaultError> {
    let vault = (deps.vault)()?;
    let p = vault.placeholder_lookup(token)?;
    if p.provider != provider {
        return Err(VaultError::bad_request(
            "provider_mismatch",
            format!("this placeholder is for {}; use the base URL k2 vault proxy-url printed", p.provider),
        ));
    }
    if !(deps.live)(&p.session_id) {
        return Err(VaultError::new("session_ended", 401, "this placeholder's session has ended"));
    }
    deps.rate.check(&format!("{}/{}", p.workspace_id, p.session_id))?;
    let who = Requester::Session { workspace_id: p.workspace_id.clone(), session_id: Some(p.session_id.clone()) };
    let sel = Selector::Id(p.secret_id.clone());
    let target = vault.proxy_target(&sel, &who)?;
    // The site the placeholder was minted for must still be its one site.
    let changed = || VaultError::forbidden("proxy_site_changed", "this secret's sites changed; run k2 vault proxy-url again");
    let origin = single_origin(&target.hosts).map_err(|_| changed())?;
    if provider_id(&origin) != p.provider {
        return Err(changed());
    }
    let r = vault.resolve(&sel, Use::Proxy(&origin), &who)?;
    let (dst, fmt) = r.inject.choose(None, None)?;
    // A refreshing sign-in is sent as its current access token, as fetch does.
    let (value, sign) = if oauth_refresh::is_oauth(&r.inject) {
        let oauth = oauth_refresh::Deps { http: &*deps.oauth_http, asker_workspace: &p.workspace_id, post_card: &*deps.reconnect_card };
        (oauth_refresh::access_token(&vault, &r.meta.id, &oauth)?, None)
    } else {
        (r.value, r.inject.sign.clone())
    };
    let injection = Injection { name: r.meta.name.clone(), value, dst, fmt, allowed: parse_stored(&target.hosts), sign };
    let allow = Allowlist::from_injections(std::slice::from_ref(&injection));
    Ok(Authorized { session_id: p.session_id, origin, injection, allow })
}

fn misplaced(where_: &str) -> VaultError {
    VaultError::bad_request(
        "placeholder_misplaced",
        format!("the placeholder is in the {where_}; K2 never forwards it. Send it only as the API key, or remove it from the {where_}"),
    )
}

fn holds_any_placeholder(v: &[u8]) -> bool {
    v.windows(PLACEHOLDER_PREFIX.len()).any(|w| w == PLACEHOLDER_PREFIX.as_bytes())
}

fn io_failed(what: &'static str) -> VaultError {
    VaultError::new("proxy_failed", 500, what)
}

/// A vetted destination and its connection pool, kept briefly per origin.
#[allow(clippy::type_complexity)]
fn clients() -> &'static Mutex<HashMap<(String, bool), (Instant, Destination, reqwest::Client)>> {
    static C: OnceLock<Mutex<HashMap<(String, bool), (Instant, Destination, reqwest::Client)>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

async fn client_for(url: &url::Url, origin: &Origin, exact_ip: bool) -> Result<reqwest::Client, VaultError> {
    let key = (origin.render(), exact_ip);
    let cached = {
        let g = clients().lock();
        g.get(&key).filter(|(at, _, _)| at.elapsed() < CLIENT_TTL).map(|(_, d, c)| (d.clone(), c.clone()))
    };
    if let Some((dest, client)) = cached {
        // Air-gap may have turned on since the vet.
        egress::recheck(&dest)?;
        return Ok(client);
    }
    let u = url.clone();
    let dest = tokio::task::spawn_blocking(move || Destination::vet(&u, exact_ip)).await.map_err(|_| io_failed("the lookup stopped"))??;
    let client = egress::async_client(Purpose::VaultProxy, &dest, READ_IDLE)?;
    let mut g = clients().lock();
    g.retain(|_, (at, _, _)| at.elapsed() < CLIENT_TTL);
    g.insert(key, (Instant::now(), dest, client.clone()));
    Ok(client)
}

async fn proxy_call<B>(deps: &Deps, peer: SocketAddr, req: Request<B>) -> Result<Response<ProxyBody>, VaultError>
where
    B: Body<Data = Bytes> + Send + 'static,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    if !peer_allowed(peer) {
        return Err(VaultError::forbidden("not_loopback", "the vault proxy only answers this machine"));
    }
    let (parts, body) = req.into_parts();
    let (token, provider, rest) =
        parse_path(parts.uri.path()).ok_or_else(|| VaultError::not_found("this isn't a vault proxy path; use the base URL k2 vault proxy-url printed"))?;
    let mut method = Method::from_bytes(validate_method(parts.method.as_str())?.as_bytes())
        .map_err(|_| VaultError::bad_request("bad_method", "method isn't supported"))?;

    let auth = {
        let deps2 = deps.clone();
        let (t, p) = (token.to_string(), provider.to_string());
        tokio::task::spawn_blocking(move || authorize(&deps2, &t, &p)).await.map_err(|_| io_failed("the check stopped"))??
    };
    let slot = deps.streams.acquire(&auth.session_id).ok_or_else(|| VaultError::new("too_many_streams", 429, "too many open proxy calls for this session"))?;
    let Authorized { origin, injection: inj, allow, .. } = auth;

    // The whole request is in hand before anything goes out.
    let collected = Limited::new(body, MAX_REQUEST_BODY).collect().await.map_err(|e| {
        if e.downcast_ref::<LengthLimitError>().is_some() {
            VaultError::new("body_too_large", 413, format!("request bodies through the proxy are at most {} MiB", MAX_REQUEST_BODY / (1024 * 1024)))
        } else {
            VaultError::bad_request("bad_body", "couldn't read the request body")
        }
    })?;
    let raw_body = collected.to_bytes();

    let scan = Redactor::new(vec![("placeholder", token.as_bytes())]);
    if scan.hits(rest.as_bytes()) {
        return Err(misplaced("path"));
    }
    if scan.hits(&raw_body) {
        return Err(misplaced("body"));
    }

    // The destination is the secret's site. Nothing from the request but
    // the path and the query goes into it.
    let mut url = url::Url::parse(&origin.render()).map_err(|_| VaultError::bad_request("bad_host", "the secret's site isn't a URL"))?;
    url.set_path(&format!("/{rest}"));
    url.set_query(parts.uri.query());
    if Origin::from_url(&url)? != origin {
        return Err(VaultError::forbidden("host_not_allowed", "that path leaves the secret's site"));
    }

    // Sealed injection, as fetch does it. `wire` keeps the value as it
    // goes out, for the redactor.
    let mut header_edit: Option<(HeaderName, HeaderValue)> = None;
    let mut query_edit: Option<(String, SecretBytes)> = None;
    let mut owned: Vec<String> = Vec::new();
    let mut wire: Option<SecretBytes> = None;
    let mut out_body: Option<SecretBytes> = (!raw_body.is_empty()).then(|| SecretBytes::from_slice(&raw_body));
    match &inj.sign {
        Some(sg) => {
            if let Sign::Ovh = sg {
                Sign::ovh_keys(inj.value.expose())?;
            }
            owned.extend(sg.header_names());
        }
        None => match inject_edit(&inj.dst, &inj.fmt, &inj.value)? {
            InjectEdit::Header { name, value } => {
                let n = HeaderName::from_bytes(name.as_bytes()).map_err(|_| VaultError::bad_request("bad_inject", format!("bad header name: {name}")))?;
                let mut v = HeaderValue::from_bytes(value.expose()).map_err(|_| VaultError::bad_request("value_not_header_safe", "this value has a control character"))?;
                v.set_sensitive(true);
                wire = Some(value.duplicate());
                owned.push(name);
                header_edit = Some((n, v));
            }
            InjectEdit::Query { param, value } => {
                if value.expose_str().is_none() {
                    return Err(VaultError::bad_request("value_not_query_safe", "this value isn't text"));
                }
                wire = Some(value.duplicate());
                query_edit = Some((param, value));
            }
            InjectEdit::Body => {
                let current = out_body.as_ref().ok_or_else(|| VaultError::bad_request("body_marker_missing", "this secret goes in the request body; send a body"))?;
                out_body = Some(SecretBytes::new(inject_body(current.expose(), &inj.fmt, &inj.value)?));
            }
        },
    }
    // The secret's own query parameter is the one place a placeholder in
    // the query is expected: it is replaced, then the rest is checked.
    if let Some((param, value)) = &query_edit {
        set_query_param(&mut url, param, value.expose_str().expect("checked above"));
    }
    if url.query().is_some_and(|q| scan.hits(q.as_bytes())) {
        return Err(misplaced("query"));
    }

    // Headers: framing and routing are K2's; a header the secret owns is
    // replaced; anything holding a placeholder is dropped.
    let mut agent_headers: Vec<(HeaderName, HeaderValue)> = Vec::new();
    for (n, v) in parts.headers.iter() {
        if is_forbidden_header(n.as_str()) || owned.iter().any(|o| o.eq_ignore_ascii_case(n.as_str())) {
            continue;
        }
        if scan.hits(v.as_bytes()) || scan.hits(n.as_str().as_bytes()) || holds_any_placeholder(v.as_bytes()) {
            continue;
        }
        agent_headers.push((n.clone(), v.clone()));
    }

    // Echoes of the value come back masked: raw, and as it went out.
    let ovh_parts: Vec<SecretBytes> = match &inj.sign {
        Some(Sign::Ovh) => Sign::ovh_keys(inj.value.expose())
            .map(|k| [k.application_key, k.application_secret, k.consumer_key].into_iter().map(SecretBytes::from_string).collect())
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let mut entries: Vec<(&str, &[u8])> = vec![(inj.name.as_str(), inj.value.expose())];
    if let Some(w) = &wire {
        entries.push((inj.name.as_str(), w.expose()));
    }
    for p in &ovh_parts {
        entries.push((inj.name.as_str(), p.expose()));
    }
    let own = Redactor::new(entries);

    let exact_ip = allow.exact_ip(&origin);
    let mut hops = 0usize;
    let resp = loop {
        allow.check(&Origin::from_url(&url)?)?;
        let client = client_for(&url, &origin, exact_ip).await?;
        let mut headers = HeaderMap::new();
        for (n, v) in &agent_headers {
            headers.append(n.clone(), v.clone());
        }
        if let Some((n, v)) = &header_edit {
            headers.insert(n.clone(), v.clone());
        }
        // Signers: from the exact URL, method and body sent, per hop.
        if let Some(sg) = &inj.sign {
            let now = if matches!(sg, Sign::Ovh) {
                let (u, o, a) = (url.clone(), origin.clone(), allow.clone());
                let deadline = Instant::now() + Duration::from_secs(30);
                let (t, _) = tokio::task::spawn_blocking(move || ovh_time(&u, &o, &a, exact_ip, Purpose::VaultProxy, deadline))
                    .await
                    .map_err(|_| io_failed("the clock check stopped"))?;
                t
            } else {
                chrono::Utc::now().timestamp()
            };
            let body_bytes: &[u8] = out_body.as_ref().map_or(&[], |b| b.expose());
            for (n, v) in sign_edit(sg, &inj.value, method.as_str(), url.as_str(), body_bytes, now)? {
                let name = HeaderName::from_bytes(n.as_bytes()).map_err(|_| VaultError::bad_request("bad_inject", format!("bad header name: {n}")))?;
                let mut value = HeaderValue::from_bytes(v.expose()).map_err(|_| VaultError::bad_request("value_not_header_safe", "this value has a control character"))?;
                value.set_sensitive(true);
                headers.insert(name, value);
            }
        }
        // Compression would blind the redactor.
        headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));

        let mut b = client.request(method.clone(), url.clone()).headers(headers);
        if let Some(body) = &out_body {
            b = b.body(body.expose().to_vec());
        }
        let resp = b.send().await.map_err(|e| upstream_error(send_kind(&e.without_url()), &origin))?;
        let status = resp.status();
        if status.is_redirection() && hops < MAX_REDIRECTS {
            let next = follow_target(&url, resp.headers(), &origin).filter(|n| Origin::from_url(n).is_ok_and(|o| allow.check(&o).is_ok()));
            if let Some(next) = next {
                hops += 1;
                let code = status.as_u16();
                if (code == 303 && method != Method::HEAD) || ((code == 301 || code == 302) && method == Method::POST) {
                    method = Method::GET;
                    out_body = None;
                }
                url = next;
                if let Some((param, value)) = &query_edit {
                    set_query_param(&mut url, param, value.expose_str().expect("checked above"));
                }
                continue;
            }
        }
        break resp;
    };

    let status = resp.status();
    let no_body = method == Method::HEAD || matches!(status.as_u16(), 204 | 304);
    if !no_body && !is_identity(resp.headers()) {
        return Err(upstream_error("the response used a compression K2 can't redact", &origin));
    }
    let mut answer = Response::new(empty());
    *answer.status_mut() = status;
    for (name, value) in resp.headers() {
        if !RESPONSE_HEADER_ALLOWLIST.contains(&name.as_str()) || (name == CONTENT_LENGTH && !no_body) {
            continue;
        }
        // Redaction can change a body's length, so it goes chunked.
        if let Ok(v) = HeaderValue::from_bytes(&own.redact(value.as_bytes())) {
            answer.headers_mut().append(name.clone(), v);
        }
    }
    if no_body {
        return Ok(answer);
    }
    let (tx, rx) = mpsc::channel::<Result<Frame<Bytes>, io::Error>>(8);
    tokio::spawn(async move {
        let _slot = slot;
        let mut resp = resp;
        let mut redact = own.stream();
        loop {
            match resp.chunk().await {
                Ok(Some(c)) => {
                    let out = redact.push(&c);
                    if !out.is_empty() && tx.send(Ok(Frame::data(Bytes::from(out)))).await.is_err() {
                        return; // the caller went away
                    }
                }
                Ok(None) => {
                    let out = redact.finish();
                    if !out.is_empty() {
                        let _ = tx.send(Ok(Frame::data(Bytes::from(out)))).await;
                    }
                    return;
                }
                // Abort the connection rather than end it cleanly: a
                // truncated answer must not look complete.
                Err(_) => {
                    let _ = tx.send(Err(io::Error::other("the upstream connection failed"))).await;
                    return;
                }
            }
        }
    });
    *answer.body_mut() = ChannelBody { rx }.boxed();
    Ok(answer)
}

fn is_identity(h: &HeaderMap) -> bool {
    h.get_all(CONTENT_ENCODING).iter().all(|v| {
        v.to_str().is_ok_and(|s| s.split(',').all(|p| matches!(p.trim().to_ascii_lowercase().as_str(), "" | "identity")))
    })
}

struct ChannelBody {
    rx: mpsc::Receiver<Result<Frame<Bytes>, io::Error>>,
}

impl Body for ChannelBody {
    type Data = Bytes;
    type Error = io::Error;

    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, io::Error>>> {
        self.rx.poll_recv(cx)
    }
}

/// Open requests per session.
#[derive(Clone, Default)]
struct Streams(Arc<Mutex<HashMap<String, usize>>>);

struct Slot {
    map: Arc<Mutex<HashMap<String, usize>>>,
    session: String,
}

impl Streams {
    fn acquire(&self, session: &str) -> Option<Slot> {
        let mut g = self.0.lock();
        let n = g.entry(session.to_string()).or_insert(0);
        if *n >= MAX_STREAMS {
            return None;
        }
        *n += 1;
        Some(Slot { map: self.0.clone(), session: session.to_string() })
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut g = self.map.lock();
        if let Some(n) = g.get_mut(&self.session) {
            *n -= 1;
            if *n == 0 {
                g.remove(&self.session);
            }
        }
    }
}

// ── placeholders for a session ──────────────────────────────────────

/// `POST /cli/vault/proxy-url`: a placeholder and base URL for the caller's
/// own session. The session comes from the passport, never from the body.
/// Any secret the caller may `fetch` works, with no policy change (V1).
pub fn mint_for_caller(
    vault: &LocalVault,
    c: &VaultCaller,
    sel: &Selector,
    ttl_secs: Option<i64>,
    live: &dyn Fn(&str) -> bool,
) -> Result<Value, VaultError> {
    mint_checked(vault, c, sel, ttl_secs, live, &|_| Ok(()))
}

/// [`mint_for_caller`], with one more step once the use is allowed and
/// before a placeholder exists: `before_mint` gets the secret's id. `k2 vault
/// run` refreshes a sign-in there, so an expired one refuses before the
/// command starts.
pub(crate) fn mint_checked(
    vault: &LocalVault,
    c: &VaultCaller,
    sel: &Selector,
    ttl_secs: Option<i64>,
    live: &dyn Fn(&str) -> bool,
    before_mint: &dyn Fn(&str) -> Result<(), VaultError>,
) -> Result<Value, VaultError> {
    let a = c.actor.agent().ok_or_else(|| VaultError::forbidden("use_as_agent", "k2 vault proxy-url runs for an agent session; people call the site themselves"))?;
    let who = c.requester()?;
    let session_id = match &who {
        Requester::Session { session_id: Some(s), .. } if !s.trim().is_empty() => s.clone(),
        _ => return Err(VaultError::forbidden("use_as_agent", "k2 vault proxy-url runs for an agent session")),
    };
    if !live(&session_id) {
        return Err(VaultError::conflict("session_ended", "this session isn't running, so a placeholder would be dead on arrival"));
    }
    let Some(on) = listening() else {
        return Err(VaultError::new("proxy_unavailable", 503, "the vault proxy isn't listening; check k2 vault status, or use k2 vault fetch"));
    };
    let target = vault.proxy_target(sel, &who)?;
    let origin = single_origin(&target.hosts)?;
    // Refuses what this caller may not use, with the usual audit.
    let r = vault.resolve(sel, Use::Proxy(&origin), &who)?;
    r.inject.choose(None, None)?;
    before_mint(&target.secret_id)?;
    let pair = env_pair(&origin, target.env_name.as_deref(), &target.name)?;
    if vault.placeholders_live_count(&session_id)? >= MAX_PLACEHOLDERS {
        return Err(VaultError::new("too_many_placeholders", 429, "this session holds too many placeholders; reuse the ones you have"));
    }
    let expires_at = ttl_secs.map(|s| (chrono::Utc::now() + chrono::Duration::seconds(s)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
    let token = vault.placeholder_mint(&target.secret_id, &a.workspace_id, &session_id, pair.provider, expires_at.clone())?;
    Ok(json!({
        "envName": pair.key_env,
        "placeholder": token,
        "baseUrlEnv": pair.base_env,
        "baseUrl": base_url(on.port, &token, &pair),
        "provider": pair.provider,
        "host": origin.render(),
        "expiresAt": expires_at,
    }))
}

/// Spawn: mint one placeholder per `proxy` delivery for this session and
/// set its env pair. A name already in `env` stays: a secret delivered as
/// env, or K2's own login for the agent, isn't overwritten.
pub fn deliver_at_spawn(
    vault: &LocalVault,
    env: &mut HashMap<String, String>,
    who: &Requester,
    workspace_id: &str,
    session_id: &str,
    sandboxed: bool,
    skipped: &mut Vec<String>,
) {
    let items = match vault.proxy_items(who) {
        Ok(items) => items,
        Err(e) => {
            k2_core::log_debug!("WARN [vault] spawn {session_id}: proxy items: {}", e.code);
            return;
        }
    };
    if items.is_empty() {
        return;
    }
    // A sandboxed cell has its own network: 127.0.0.1 there isn't K2.
    let on = if sandboxed { None } else { listening() };
    let Some(on) = on else {
        skipped.extend(items.iter().map(|i| format!("{}: proxy_unavailable", i.name)));
        return;
    };
    let mut taken: HashSet<String> = HashSet::new();
    for item in items {
        let pair = match single_origin(&item.hosts).and_then(|o| env_pair(&o, item.env_name.as_deref(), &item.name)) {
            Ok(p) => p,
            Err(e) => {
                skipped.push(format!("{}: {}", item.name, e.code));
                continue;
            }
        };
        if env.contains_key(&pair.key_env) || env.contains_key(&pair.base_env) || taken.contains(&pair.key_env) {
            skipped.push(format!("{}: env_taken", item.name));
            continue;
        }
        match vault.placeholder_mint(&item.secret_id, workspace_id, session_id, pair.provider, None) {
            Ok(token) => {
                env.insert(pair.base_env.clone(), base_url(on.port, &token, &pair));
                taken.insert(pair.key_env.clone());
                env.insert(pair.key_env, token);
            }
            Err(e) => skipped.push(format!("{}: {}", item.name, e.code)),
        }
    }
}

#[cfg(test)]
#[path = "proxy_tests.rs"]
pub(crate) mod tests;
