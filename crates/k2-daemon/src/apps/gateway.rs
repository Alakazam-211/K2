//! The App gateway link (`prd-k2-app-runtime-v1` §5.1–5.2, S14): the one
//! door a walled App comes in through to reach K2.
//!
//! **One listener per App.** For each running guarded App the daemon
//! listens on `127.0.0.1:<gateway port>` (its id slot's port in
//! 38600–38663). Inside the container pasta forwards `127.0.0.1:8730` to
//! it, and the per-App nft rule admits only that App's cgroup, so an App
//! reaches its own link and no other (and never the daemon's main port).
//! Every connection runs the **real** route dispatcher tagged
//! [`Ingress::App`], inside a task-local [`LinkCtx`], so the gateway knows
//! the App **from the listener that accepted the socket** (§5.1).
//!
//! **The gate** ([`gate`], run by the dispatcher on every request of a
//! link connection, keep-alive included) admits only:
//! - a loopback `Host` (`127.0.0.1`, `localhost`, `[::1]`; else 421);
//! - no cookies (the link carries none; 400);
//! - an App-facing route from the shared table
//!   (`k2_core::app_surface`, the browser helper's list minus
//!   helper-only rows; sockets need their parameter; else 404
//!   `not_on_link`);
//! - JSON bodies on writes (415 `json_required`);
//! - **this link's App pass** (`k2skn_`, header or `?token=`): a pass for
//!   another link or App, a guest session or a platform token is
//!   `pass_wrong_link`; no pass `pass_required`; anything else
//!   `pass_invalid`.
//!
//! Then the route's own handler runs with the pass, exactly as it does for
//! the browser helper: caps and rooms decide. A link pass carries the
//! App's manifest caps (minus `files:agentconfig`) in its own workspace's
//! room only.
//!
//! **The pass** (`k2_core::skin::link`) lives an hour and is rotated here
//! at 80%; the engine swaps the App's pass file and the old pass stays
//! valid until its own expiry. Stopping or uninstalling the App closes the
//! link and revokes every pass.
//!
//! **The door (0.46.1, lane 2)**:
//! - **quotas** (§5.4, `super::quotas`): per-App rate (token bucket),
//!   sockets, body size and live subscriptions, with the owner's per-App
//!   override; refusals carry `limit` and `retryAfterMs`;
//! - **per-call audit** (§5.5, `super::audit`): every request on a link,
//!   admitted or refused, is one hash-chained row in `app-audit.db`, with
//!   alerts on denial spikes, first write verbs, `pass_wrong_link` and
//!   quota hits;
//! - **suspend** (`super::suspend`): a suspended App's link answers `401
//!   app_suspended` and [`open`] refuses it; suspend revokes every pass;
//! - **the grants formula** (§5.3, `super::grants`): [`open`] narrows the
//!   engine's manifest ceiling ([`link_policy`]) to the App's approved set;
//! - the remote App pass (AR10, `skin::link::mint_remote_pass`) is refused
//!   here like every pass that isn't this link's (`pass_wrong_link`).
//!
//! Still seams: S23 replaces the hand-written route table with rows
//! generated from `contract/catalog.json` (only
//! `k2_core::app_surface::{link_allows_http, ws_param}` change); S13's
//! process exception opens a link with its own [`Deliver`];
//! `/v1/apps/<own id>/…` (the App-scoped key, S45).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use k2_core::skin::link::{self as passes, PresentedPass};
use k2_core::skin::SkinPass;
use serde_json::{json, Value};

use crate::routes::dispatcher::Ingress;

/// Connections a link serves at once by default (§5.4: 64 sockets per
/// App; the owner's override is `super::quotas`).
pub const MAX_CONNECTIONS: usize = super::quotas::DEFAULTS.sockets as usize;

/// How a rotated pass reaches the App (the guarded engine's `pass-write`).
pub type Deliver = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

/// Counters for `k2 app link` (and S19's quotas and audit later).
#[derive(Debug, Default)]
pub struct LinkStats {
    pub requests: AtomicU64,
    pub refused: AtomicU64,
    pub wrong_link: AtomicU64,
    pub live: AtomicUsize,
}

/// The link a connection arrived on (task-local while it is served).
#[derive(Debug)]
pub struct LinkCtx {
    pub app_id: String,
    pub service_id: String,
    pub workspace_id: String,
    pub port: u16,
    pub link_id: String,
    pub stats: LinkStats,
}

tokio::task_local! {
    static LINK: Arc<LinkCtx>;
}

/// The link the current request arrived on (`None` off a link).
pub fn current() -> Option<Arc<LinkCtx>> {
    LINK.try_with(|l| l.clone()).ok()
}

/// The dispatcher's one pass resolver. On a link only that link's App
/// pass resolves; anywhere else a link pass never does (k2-core keeps it
/// out of `resolve_skin_token`).
pub fn resolve_pass(raw: &str) -> Option<SkinPass> {
    match current() {
        Some(l) => passes::resolve_link_pass(raw, &l.link_id, &l.service_id),
        None => k2_core::skin::resolve_skin_token(raw),
    }
}

/// The link id a listener on `port` gets (stable across daemon restarts,
/// so a pass delivered before a restart keeps working after it).
pub fn link_id_for(port: u16) -> String {
    format!("gw:{port}")
}

/// The in-wall URL every guarded App's code dials.
pub fn in_wall_url() -> String {
    format!("http://127.0.0.1:{}", k2_root_proto::engine::IN_WALL_GATEWAY_PORT)
}

// ── policy ───────────────────────────────────────────────────────────

/// What an App's link pass may carry (0.46.0): the live Seed's manifest
/// caps, kept to the caps K2 knows minus `files:agentconfig`, in the App's
/// own workspace room only. The grants formula (§5.3, 0.46.1) narrows this
/// further with the owner's per-App grant; nothing here can widen it.
pub fn link_policy(manifest_caps: &[String], workspace_id: &str) -> (Vec<String>, Vec<String>, Vec<String>) {
    let (caps, dropped) = passes::link_caps(manifest_caps);
    (caps, vec![workspace_id.to_string()], dropped)
}

// ── the registry ─────────────────────────────────────────────────────

/// What a link is opened with.
#[derive(Clone)]
pub struct LinkSpec {
    pub app_id: String,
    pub service_id: String,
    pub workspace_id: String,
    /// Host loopback port (0 = any free port: tests only).
    pub port: u16,
    pub caps: Vec<String>,
    pub rooms: Vec<String>,
}

struct Entry {
    ctx: Arc<LinkCtx>,
    spec: LinkSpec,
    task: tokio::task::JoinHandle<()>,
    deliver: Option<Deliver>,
    pass_id: Option<String>,
    issued_at: i64,
    expires_at: i64,
    rotations: u64,
    last_error: Option<String>,
}

fn links() -> &'static Mutex<HashMap<String, Entry>> {
    static L: OnceLock<Mutex<HashMap<String, Entry>>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(HashMap::new()))
}

struct Installed {
    state: crate::DaemonState,
    handle: tokio::runtime::Handle,
}

fn installed() -> &'static Mutex<Option<Installed>> {
    static I: OnceLock<Mutex<Option<Installed>>> = OnceLock::new();
    I.get_or_init(|| Mutex::new(None))
}

/// The daemon state and runtime the links dispatch with. Called at boot
/// (before the App supervisor re-adopts running Apps) and by the test
/// harness. Must run inside a tokio runtime.
pub fn install(state: crate::DaemonState) {
    let handle = tokio::runtime::Handle::current();
    *installed().lock().unwrap_or_else(|e| e.into_inner()) = Some(Installed { state, handle });
    ensure_ticker();
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn bind(handle: &tokio::runtime::Handle, port: u16) -> Result<(tokio::net::TcpListener, u16), String> {
    let std_l = std::net::TcpListener::bind(("127.0.0.1", port)).map_err(|e| format!("bind the App's gateway link 127.0.0.1:{port}: {e}"))?;
    let bound = std_l.local_addr().map_err(|e| e.to_string())?.port();
    std_l.set_nonblocking(true).map_err(|e| e.to_string())?;
    let _rt = handle.enter();
    let l = tokio::net::TcpListener::from_std(std_l).map_err(|e| e.to_string())?;
    Ok((l, bound))
}

fn spawn_accept(handle: &tokio::runtime::Handle, l: tokio::net::TcpListener, ctx: Arc<LinkCtx>, state: crate::DaemonState) -> tokio::task::JoinHandle<()> {
    handle.spawn(async move {
        loop {
            let accepted = tokio::select! {
                r = l.accept() => r,
                _ = crate::handoff::quiesced() => return,
            };
            let Ok((mut stream, _)) = accepted else {
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            };
            let max = super::quotas::quotas_for(&ctx.app_id).sockets as usize;
            if ctx.stats.live.load(Ordering::SeqCst) >= max {
                ctx.stats.refused.fetch_add(1, Ordering::SeqCst);
                super::audit::record(k2_core::app_audit::Call {
                    at_ms: now() * 1000,
                    app_id: ctx.app_id.clone(),
                    actor: "app".into(),
                    verb: "CONNECT link".into(),
                    result: 429,
                    code: Some("too_many_sockets".into()),
                    ..Default::default()
                });
                let app = ctx.app_id.clone();
                tokio::spawn(async move {
                    let body = refusal("too_many_sockets", &format!("an App holds at most {max} connections to its link"), Some(&app), Some(max as u64), None);
                    crate::routes::http::send_response(&mut stream, "429 Too Many Requests", "application/json", &body).await;
                });
                continue;
            }
            let ctx = ctx.clone();
            let st = state.clone();
            tokio::spawn(async move {
                ctx.stats.live.fetch_add(1, Ordering::SeqCst);
                let c2 = ctx.clone();
                super::audit::scope(LINK.scope(ctx, crate::routes::dispatcher::dispatch(stream, st, Ingress::App))).await;
                c2.stats.live.fetch_sub(1, Ordering::SeqCst);
            });
        }
    })
}

/// Open (or keep) `spec.app_id`'s link and mint a fresh pass for it.
/// Returns the raw pass: the caller hands it to the App (a guarded start
/// carries it). Later rotations go through `deliver`. An open link on the
/// same port keeps its listener (a redeploy, a second instance).
pub fn open(mut spec: LinkSpec, deliver: Option<Deliver>) -> Result<String, String> {
    // The door: a suspended App gets no link; the pass carries the
    // manifest ceiling narrowed to the App's approved set (§5.3).
    if super::suspend::is_suspended(&spec.app_id) {
        return Err(format!("{} is suspended (k2 app resume {}, the owner)", spec.app_id, spec.app_id));
    }
    let eff = super::grants::narrow_for_link(&spec.app_id, &spec.caps, &spec.workspace_id);
    spec.caps = eff.caps;
    if !eff.withheld.is_empty() {
        k2_core::log_debug!("[apps/gateway] {}: caps withheld until the owner approves them: {}", spec.app_id, eff.withheld.join(", "));
    }
    let (state, handle) = {
        let g = installed().lock().unwrap_or_else(|e| e.into_inner());
        let i = g.as_ref().ok_or("the gateway has no daemon state yet (boot order)")?;
        (i.state.clone(), i.handle.clone())
    };
    let mut map = links().lock().unwrap_or_else(|e| e.into_inner());
    let keep = map.get(&spec.app_id).is_some_and(|e| !e.task.is_finished() && (spec.port == 0 || e.ctx.port == spec.port) && e.spec.service_id == spec.service_id);
    if !keep {
        if let Some(old) = map.remove(&spec.app_id) {
            old.task.abort();
        }
        let (l, port) = bind(&handle, spec.port)?;
        let ctx = Arc::new(LinkCtx {
            app_id: spec.app_id.clone(),
            service_id: spec.service_id.clone(),
            workspace_id: spec.workspace_id.clone(),
            port,
            link_id: link_id_for(port),
            stats: LinkStats::default(),
        });
        let task = spawn_accept(&handle, l, ctx.clone(), state);
        map.insert(
            spec.app_id.clone(),
            Entry { ctx, spec: spec.clone(), task, deliver: None, pass_id: None, issued_at: 0, expires_at: 0, rotations: 0, last_error: None },
        );
    }
    let e = map.get_mut(&spec.app_id).ok_or("link vanished")?;
    e.spec.caps = spec.caps.clone();
    e.spec.rooms = spec.rooms.clone();
    if deliver.is_some() {
        e.deliver = deliver;
    }
    let m = passes::mint_link_pass(&e.ctx.service_id, &e.ctx.app_id, &e.ctx.link_id, &e.spec.caps, &e.spec.rooms)?;
    e.pass_id = Some(m.id);
    e.issued_at = m.issued_at;
    e.expires_at = m.expires_at;
    e.last_error = None;
    Ok(m.raw)
}

/// Live connections on `app_id`'s link.
pub fn connections(app_id: &str) -> usize {
    links().lock().unwrap_or_else(|e| e.into_inner()).get(app_id).map(|e| e.ctx.stats.live.load(Ordering::SeqCst)).unwrap_or(0)
}

/// The port `app_id`'s link listens on, when open.
pub fn port_of(app_id: &str) -> Option<u16> {
    links().lock().unwrap_or_else(|e| e.into_inner()).get(app_id).map(|e| e.ctx.port)
}

/// Close `app_id`'s link; `revoke` also ends every pass it had (stop,
/// uninstall). Returns how many passes were revoked.
pub fn close(app_id: &str, service_id: Option<&str>, revoke: bool) -> usize {
    let entry = links().lock().unwrap_or_else(|e| e.into_inner()).remove(app_id);
    let svc = entry.as_ref().map(|e| e.ctx.service_id.clone()).or_else(|| service_id.map(str::to_string));
    if let Some(e) = entry {
        e.task.abort();
    }
    match (revoke, svc) {
        (true, Some(svc)) => passes::revoke_link_passes(&svc).unwrap_or_else(|e| {
            k2_core::log_debug!("[apps/gateway] {app_id}: revoke passes: {e}");
            0
        }),
        _ => 0,
    }
}

/// Mint and deliver a new pass now. The old pass keeps working until its
/// own expiry; a pass that can't be delivered is revoked at once (it never
/// left the daemon). Returns the new expiry.
pub fn rotate(app_id: &str) -> Result<i64, String> {
    let (ctx, caps, rooms, deliver) = {
        let map = links().lock().unwrap_or_else(|e| e.into_inner());
        let e = map.get(app_id).ok_or_else(|| format!("{app_id} has no open gateway link (it isn't running walled)"))?;
        (e.ctx.clone(), e.spec.caps.clone(), e.spec.rooms.clone(), e.deliver.clone())
    };
    let deliver = deliver.ok_or("this link has no way to hand the App a new pass")?;
    let m = passes::mint_link_pass(&ctx.service_id, &ctx.app_id, &ctx.link_id, &caps, &rooms)?;
    let res = deliver(&m.raw);
    let mut map = links().lock().unwrap_or_else(|e| e.into_inner());
    match res {
        Ok(()) => {
            if let Some(e) = map.get_mut(app_id) {
                e.pass_id = Some(m.id);
                e.issued_at = m.issued_at;
                e.expires_at = m.expires_at;
                e.rotations += 1;
                e.last_error = None;
            }
            Ok(m.expires_at)
        }
        Err(why) => {
            let _ = passes::revoke_link_pass(&m.id);
            if let Some(e) = map.get_mut(app_id) {
                e.last_error = Some(why.clone());
            }
            Err(format!("the new pass couldn't reach the App: {why}"))
        }
    }
}

/// `k2 app link` / inspect: the link's state.
pub fn status_json(app_id: &str) -> Value {
    let map = links().lock().unwrap_or_else(|e| e.into_inner());
    let Some(e) = map.get(app_id) else {
        return json!({ "open": false });
    };
    json!({
        "open": !e.task.is_finished(),
        "port": e.ctx.port,
        "linkId": e.ctx.link_id,
        "inWallUrl": in_wall_url(),
        "passFile": k2_root_proto::engine::PASS_TARGET,
        "caps": e.spec.caps,
        "rooms": e.spec.rooms,
        "passIssuedAt": e.issued_at,
        "passExpiresAt": e.expires_at,
        "rotateAt": e.issued_at + passes::LINK_PASS_ROTATE_AFTER_SECS,
        "rotations": e.rotations,
        "lastError": e.last_error,
        "livePasses": passes::live_link_passes(&e.ctx.service_id).ok(),
        "requests": e.ctx.stats.requests.load(Ordering::SeqCst),
        "refused": e.ctx.stats.refused.load(Ordering::SeqCst),
        "wrongLink": e.ctx.stats.wrong_link.load(Ordering::SeqCst),
        "connections": e.ctx.stats.live.load(Ordering::SeqCst),
    })
}

// ── health and rotation ──────────────────────────────────────────────

/// One pass of the link keeper: re-bind a link whose listener died, rotate
/// passes at 80% of their life, prune old pass rows.
pub fn tick() {
    let ids: Vec<(String, bool, i64)> = links()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .map(|(k, e)| (k.clone(), e.task.is_finished(), e.issued_at))
        .collect();
    for (app_id, dead, issued_at) in ids {
        if dead {
            rebind(&app_id);
        }
        // Only a running App gets new passes (a crash-looping one keeps
        // what it has until it expires).
        let running = super::supervisor::live_instance(&app_id).is_some();
        if running && issued_at > 0 && now() >= issued_at + passes::LINK_PASS_ROTATE_AFTER_SECS {
            if let Err(e) = rotate(&app_id) {
                k2_core::log_debug!("[apps/gateway] {app_id}: rotation failed (the current pass stays valid until it expires): {e}");
            }
        }
    }
    let _ = passes::prune_link_passes();
}

fn rebind(app_id: &str) {
    let Some((state, handle)) = installed().lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|i| (i.state.clone(), i.handle.clone())) else {
        return;
    };
    let mut map = links().lock().unwrap_or_else(|e| e.into_inner());
    let Some(e) = map.get_mut(app_id) else { return };
    match bind(&handle, e.ctx.port) {
        Ok((l, _)) => {
            e.task = spawn_accept(&handle, l, e.ctx.clone(), state);
            k2_core::log_debug!("[apps/gateway] {app_id}: link listener on :{} had stopped; re-bound", e.ctx.port);
        }
        Err(err) => e.last_error = Some(err),
    }
}

fn ensure_ticker() {
    static T: OnceLock<()> = OnceLock::new();
    if T.set(()).is_err() {
        return;
    }
    let _ = std::thread::Builder::new().name("apps-gateway".into()).spawn(|| loop {
        std::thread::sleep(Duration::from_secs(15));
        tick();
    });
}

// ── the gate ─────────────────────────────────────────────────────────

/// What the dispatcher does with a request on a link.
#[derive(Debug, PartialEq, Eq)]
pub enum Gate {
    /// Admitted: route it.
    Continue,
    /// Answer this and close.
    Answer { status: &'static str, body: String },
}

fn status_text(code: u16) -> &'static str {
    match code {
        200 => "200 OK",
        400 => "400 Bad Request",
        401 => "401 Unauthorized",
        403 => "403 Forbidden",
        404 => "404 Not Found",
        413 => "413 Payload Too Large",
        415 => "415 Unsupported Media Type",
        421 => "421 Misdirected Request",
        429 => "429 Too Many Requests",
        _ => "500 Internal Server Error",
    }
}

/// A structured refusal (§5.4: `error`, `app`, `limit`, `retryAfterMs`).
fn refusal(code: &str, hint: &str, app: Option<&str>, limit: Option<u64>, retry_after_ms: Option<u64>) -> String {
    let mut e = json!({ "code": code, "hint": hint, "app": app });
    if let Some(l) = limit {
        e["limit"] = json!(l);
    }
    if let Some(r) = retry_after_ms {
        e["retryAfterMs"] = json!(r);
    }
    json!({ "error": e }).to_string()
}

fn deny(ctx: Option<&LinkCtx>, status: u16, code: &str, hint: &str) -> Gate {
    deny_with(ctx, status, code, hint, None, None)
}

fn deny_with(ctx: Option<&LinkCtx>, status: u16, code: &str, hint: &str, limit: Option<u64>, retry: Option<u64>) -> Gate {
    if let Some(c) = ctx {
        c.stats.refused.fetch_add(1, Ordering::SeqCst);
    }
    Gate::Answer { status: status_text(status), body: refusal(code, hint, ctx.map(|c| c.app_id.as_str()), limit, retry) }
}

fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().skip(1).take_while(|l| !l.trim().is_empty()).find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim().eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

fn host_is_loopback(host: &str) -> bool {
    let h = host.trim();
    let name = if let Some(rest) = h.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        h.rsplit_once(':').map(|(n, p)| if p.bytes().all(|b| b.is_ascii_digit()) { n } else { h }).unwrap_or(h)
    };
    matches!(name.to_ascii_lowercase().as_str(), "127.0.0.1" | "localhost" | "::1")
}

fn query_token(query: &str) -> Option<&str> {
    query.split('&').find_map(|p| p.strip_prefix("token=")).filter(|t| !t.is_empty())
}

fn bearer(head: &str) -> Option<&str> {
    let v = header(head, "authorization")?;
    let (scheme, tok) = v.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| tok.trim()).filter(|t| !t.is_empty())
}

fn query_has(query: &str, key: &str) -> bool {
    query.split('&').any(|pair| matches!(pair.split_once('='), Some((k, v)) if k == key && !v.is_empty()))
}

/// The route-table half of the gate (pure): may this link serve it?
pub fn route_allowed(method: &str, path: &str, query: &str, upgrade: bool) -> bool {
    if upgrade {
        return method.eq_ignore_ascii_case("GET") && k2_core::app_surface::ws_param(path).is_some_and(|p| query_has(query, p));
    }
    k2_core::app_surface::link_allows_http(method, path)
}

/// What the gate learned about a request, for its audit row.
#[derive(Default)]
struct Seen {
    subscribed: bool,
}

/// Check one request on a link (the dispatcher calls this before anything
/// else routes it). `head` is the request line and headers. Every request,
/// admitted or refused, opens its audit row here.
pub fn gate(head: &str, method: &str, path_and_query: &str) -> Gate {
    let Some(ctx) = current() else {
        return deny(None, 500, "link_context_missing", "a link request outside its link (a K2 bug)");
    };
    let c = ctx.as_ref();
    c.stats.requests.fetch_add(1, Ordering::SeqCst);
    let path = path_and_query.split_once('?').map(|(p, _)| p).unwrap_or(path_and_query);
    let bytes_in = header(head, "content-length").and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    let mut seen = Seen::default();
    let g = gate_on(c, head, method, path_and_query, bytes_in, &mut seen);
    let refused = match &g {
        Gate::Answer { status, body } if !status.starts_with("200") => {
            let n: u16 = status.split_whitespace().next().and_then(|x| x.parse().ok()).unwrap_or(0);
            let code = serde_json::from_str::<Value>(body).ok().and_then(|v| v["error"]["code"].as_str().map(str::to_string)).unwrap_or_default();
            Some((n, code))
        }
        _ => None,
    };
    super::audit::begin(
        c,
        super::audit::Begin {
            method,
            path,
            actor: "app",
            room: Some(c.workspace_id.clone()),
            bytes_in,
            refused: refused.as_ref().map(|(n, code)| (*n, code.as_str())),
            subscribed: seen.subscribed,
        },
    );
    g
}

fn gate_on(c: &LinkCtx, head: &str, method: &str, path_and_query: &str, bytes_in: u64, seen: &mut Seen) -> Gate {
    let (path, query) = path_and_query.split_once('?').unwrap_or((path_and_query, ""));
    match header(head, "host") {
        Some(h) if host_is_loopback(h) => {}
        _ => return deny(Some(c), 421, "host_not_link", "the gateway link answers only K2_GATEWAY_URL (a 127.0.0.1 Host)"),
    }
    if header(head, "cookie").is_some() {
        return deny(Some(c), 400, "cookies_not_allowed", "the gateway link carries no cookies: send the App pass as Authorization: Bearer");
    }
    let me = path == "/gw/me";
    let upgrade = header(head, "upgrade").is_some_and(|u| u.eq_ignore_ascii_case("websocket"));
    if !(me && method == "GET") && !route_allowed(method, path, query, upgrade) {
        return deny(
            Some(c),
            404,
            "not_on_link",
            &format!("{method} {path} isn't open to Apps (the App-facing routes are in k2 study app-api; GET /gw/me names this link)"),
        );
    }
    if method == "POST" {
        let has_body = header(head, "content-length").and_then(|v| v.parse::<u64>().ok()).unwrap_or(0) > 0;
        let json_ct = header(head, "content-type").is_some_and(|v| v.to_ascii_lowercase().starts_with("application/json"));
        if has_body && !json_ct {
            return deny(Some(c), 415, "json_required", "writes on the gateway link send Content-Type: application/json");
        }
    }
    // The door: a suspended App is refused before its pass is even looked
    // at (suspend also revoked every pass).
    if super::suspend::is_suspended(&c.app_id) {
        return deny(Some(c), 401, super::suspend::APP_SUSPENDED, "this App is suspended; only the owner resumes it");
    }
    let q = super::quotas::quotas_for(&c.app_id);
    let limit = super::quotas::body_limit(&q, path);
    if bytes_in > limit {
        return deny_with(Some(c), 413, "too_large", &format!("request bodies on this route are at most {limit} bytes"), Some(limit), None);
    }
    if let Err((wait, q)) = super::quotas::take(&c.app_id) {
        return deny_with(
            Some(c),
            429,
            "rate_limited",
            &format!("this App may make {} calls a second (bursts of {}); retry after {wait} ms", q.rps, q.burst),
            Some(q.rps as u64),
            Some(wait),
        );
    }
    let presented = bearer(head).or_else(|| query_token(query));
    let Some(raw) = presented else {
        return deny(Some(c), 401, "pass_required", "send this App's pass (the file K2_PASS_FILE names) as Authorization: Bearer");
    };
    if !k2_core::skin::is_skin_token(raw) || k2_core::skin::is_api_key_prefix(raw) {
        return deny(Some(c), 401, "wrong_credential", "only this App's pass works on its gateway link");
    }
    let Some(pass) = passes::resolve_link_pass(raw, &c.link_id, &c.service_id) else {
        return match passes::classify_presented(raw) {
            PresentedPass::Unknown => deny(Some(c), 401, "pass_invalid", "the pass is expired or revoked: read K2_PASS_FILE again (K2 rotates it)"),
            other => {
                c.stats.wrong_link.fetch_add(1, Ordering::SeqCst);
                let what = match other {
                    PresentedPass::Link { app_service_id, .. } if app_service_id != c.service_id => "another App's pass",
                    PresentedPass::Link { .. } => "a pass for another link",
                    _ => "a pass that isn't a link pass",
                };
                k2_core::log_debug!("[apps/gateway] {}: pass_wrong_link ({what}) on :{}", c.app_id, c.port);
                deny(Some(c), 403, "pass_wrong_link", &format!("{what} was presented on {}'s gateway link", c.app_id))
            }
        };
    };
    if me {
        let body = json!({
            "ok": true,
            "app": c.app_id,
            "appServiceId": c.service_id,
            "workspace": c.workspace_id,
            "link": c.link_id,
            "caps": pass.caps,
            "rooms": pass.rooms,
            "passFile": k2_root_proto::engine::PASS_TARGET,
            "rotation": "K2 rotates the pass at 80% of its hour; read the file again on pass_invalid",
        });
        return Gate::Answer { status: status_text(200), body: body.to_string() };
    }
    if upgrade {
        if !super::quotas::subscribe(&c.app_id) {
            return deny_with(
                Some(c),
                429,
                "too_many_subscriptions",
                &format!("this App holds at most {} live subscriptions", q.subscriptions),
                Some(q.subscriptions as u64),
                None,
            );
        }
        seen.subscribed = true;
    }
    Gate::Continue
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_and_routes() {
        for ok in ["127.0.0.1", "127.0.0.1:8730", "localhost:38601", "LOCALHOST", "[::1]:8730"] {
            assert!(host_is_loopback(ok), "{ok}");
        }
        for bad in ["evil.example", "127.0.0.1.evil.example", "rosson.k2.dev", "10.0.0.2:8730", "", "[::2]:1"] {
            assert!(!host_is_loopback(bad), "{bad}");
        }
        assert!(route_allowed("GET", "/cli/thread", "", false));
        assert!(!route_allowed("POST", "/cli/skin/password/change", "", false), "helper-only");
        assert!(!route_allowed("GET", "/cli/app/deploy", "", false));
        assert!(route_allowed("GET", "/cli/overlay/events", "conversation=c", true));
        assert!(!route_allowed("GET", "/cli/overlay/events", "conversation=", true), "the socket's parameter");
        assert!(!route_allowed("GET", "/cli/sessions/events", "x=1", true));
        assert_eq!(link_id_for(38601), "gw:38601");
        assert_eq!(in_wall_url(), "http://127.0.0.1:8730");
    }

    #[test]
    fn credentials_come_from_the_header_or_the_query_only() {
        let head = "GET /cli/thread HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer k2skn_abc\r\n\r\n";
        assert_eq!(bearer(head), Some("k2skn_abc"));
        assert_eq!(bearer("GET / HTTP/1.1\r\nAuthorization: Basic x\r\n\r\n"), None);
        assert_eq!(query_token("a=1&token=k2skn_q"), Some("k2skn_q"));
        assert_eq!(query_token("token="), None);
    }

    #[test]
    fn off_a_link_the_gate_refuses_and_resolve_is_the_general_one() {
        assert!(current().is_none());
        match gate("GET /cli/thread HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n", "GET", "/cli/thread") {
            Gate::Answer { status, body } => {
                assert!(status.starts_with("500"));
                assert!(body.contains("link_context_missing"));
            }
            g => panic!("{g:?}"),
        }
    }

    #[test]
    fn link_policy_is_the_manifest_in_the_own_room() {
        let (caps, rooms, dropped) = link_policy(&["thread:read".into(), "files:agentconfig".into()], "ws-1");
        assert_eq!(caps, vec!["thread:read".to_string()]);
        assert_eq!(rooms, vec!["ws-1".to_string()]);
        assert_eq!(dropped, vec!["files:agentconfig".to_string()]);
    }
}
