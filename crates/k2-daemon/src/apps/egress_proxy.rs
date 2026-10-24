//! The App egress proxy (`prd-k2-app-runtime-v1` §4.1, §10.4, S15).
//!
//! A guarded App has no route out except this: one loopback listener per
//! App on its proxy port (`k2_root_proto::apps::proxy_port_for_gateway`),
//! forwarded into the container by pasta (`-T`) and admitted by the App's
//! own cgroup rule in `inet k2app` (no App reaches another App's proxy).
//! **The listener is the App's identity**, like the gateway link: no
//! credential crosses the wall. The engine sets `HTTP(S)_PROXY`,
//! `NO_PROXY` and `NODE_USE_ENV_PROXY=1` inside the container.
//!
//! Per connection:
//! 1. limits: at most [`MAX_CONNS`] at once per App and a token bucket on
//!    new connections; a 16 KiB request head read within 10 s;
//! 2. `CONNECT host:port` (TLS and other TCP) or an absolute-form
//!    `http://` request (plain HTTP, one request per connection: the
//!    `Host` header is forced to the target and `Connection: close` set,
//!    so a second request can't ride to another virtual host). Anything
//!    else (an origin-form request, an `https://` URL) is refused: the
//!    proxy never speaks to K2 itself;
//! 3. the policy: never this box's own names (its `*.k2.dev` names and
//!    bound domains: reach K2 through the gateway), never local names,
//!    then the App's allowlist (`broad` lets any public host through and
//!    records it);
//! 4. DNS in the proxy: the daemon's egress gate resolves once, refuses
//!    private, loopback, link-local, metadata, CGNAT and this box's own
//!    addresses (and everything under air-gap), and the proxy dials only
//!    the vetted addresses (`Purpose::AppEgress`) — no rebinding;
//! 5. CONNECT to a name: when the App starts TLS, the ClientHello's SNI
//!    must be the CONNECT name (no fronting to another name on a shared
//!    IP);
//! 6. splice with an idle timeout; the App stopping cuts its tunnels;
//! 7. every decision is counted in `app_egress_hosts`, a first-seen host
//!    is written to the deploy audit (and a refused one tells the App's
//!    agent how to ask), and one JSON line per connection goes to
//!    `~/.k2/apps/<id>/logs/egress.log` (host and port only; never a path,
//!    query or header).
//!
//! Keys never pass here (a CONNECT tunnel can't inject into TLS): outbound
//! third-party keys go through the vault key proxy (AR16/AR44). Its seam
//! into this policy is [`check_for_key_proxy`].

use std::collections::HashMap;
use std::io::Write as _;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use k2_core::deploy::egress::{self as pol, HostSpec, Policy, Verdict};
use serde_json::{json, Value as J};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpStream;
use tokio::sync::watch;

use crate::egress::EgressError;

/// Concurrent connections per App.
pub const MAX_CONNS: usize = 64;
/// New connections per second per App (sustained) and the burst.
pub const RATE_PER_SEC: f64 = 20.0;
pub const RATE_BURST: f64 = 100.0;
/// Largest request head (CONNECT line or plain-HTTP head).
pub const HEAD_MAX: usize = 16 * 1024;
const HEAD_TIMEOUT: Duration = Duration::from_secs(10);
/// A tunnel with no bytes either way for this long is closed.
#[cfg(not(test))]
const IDLE: Duration = Duration::from_secs(300);
#[cfg(test)]
const IDLE: Duration = Duration::from_millis(1500);
/// The egress log rolls to `.1` at this size.
const LOG_ROLL_BYTES: u64 = 4 * 1024 * 1024;

// ── refusals ─────────────────────────────────────────────────────────

/// Why a connection was refused: an HTTP status and a stable code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    pub status: u16,
    pub code: &'static str,
    pub hint: String,
}

fn refused(status: u16, code: &'static str, hint: impl Into<String>) -> Refused {
    Refused { status, code, hint: hint.into() }
}

pub mod codes {
    /// Not on the App's allowlist, a local name, or a private/local address.
    pub const DENIED: &str = "egress_denied";
    /// One of this box's own names.
    pub const SELF: &str = "egress_self";
    pub const AIRGAP: &str = "airgap";
    pub const BAD_REQUEST: &str = "bad_proxy_request";
    pub const TOO_MANY: &str = "too_many_connections";
    pub const RATE: &str = "rate_limited";
    pub const UPSTREAM: &str = "upstream_unreachable";
    pub const SNI: &str = "sni_mismatch";
}

fn status_text(s: u16) -> &'static str {
    match s {
        200 => "200 Connection Established",
        400 => "400 Bad Request",
        403 => "403 Forbidden",
        429 => "429 Too Many Requests",
        502 => "502 Bad Gateway",
        504 => "504 Gateway Timeout",
        _ => "500 Internal Server Error",
    }
}

/// The proxy's own answer (a refusal). It is meant for the App's HTTP
/// client, but the listener is on loopback where a browser on the box can
/// reach it too, so it carries the deny-all frame headers (FA6).
fn refusal_bytes(app: &str, r: &Refused) -> Vec<u8> {
    let body = json!({ "error": { "code": r.code, "hint": r.hint, "app": app } }).to_string();
    format!(
        "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{frame}Connection: close\r\nX-K2-Egress: refused\r\n\r\n{body}",
        status_text(r.status),
        body.len(),
        frame = k2_core::frame_policy::FramePolicy::deny_all().header_lines(),
    )
    .into_bytes()
}

// ── seams (tests) ────────────────────────────────────────────────────

/// Resolve + vet a destination: the vetted addresses to dial.
pub type Resolver = Arc<dyn Fn(&str, u16) -> Result<Vec<SocketAddr>, EgressError> + Send + Sync>;
/// One decision, recorded: `(app, host, port, let_through, rule, code)`.
pub type Recorder = Arc<dyn Fn(&str, &str, u16, bool, Option<&HostSpec>, &str) + Send + Sync>;

static RESOLVER: RwLock<Option<Resolver>> = RwLock::new(None);
static RECORDER: RwLock<Option<Recorder>> = RwLock::new(None);
static OWN_NAMES: RwLock<Option<(Instant, Arc<Vec<String>>)>> = RwLock::new(None);
#[cfg(test)]
static OWN_NAMES_FAKE: RwLock<Option<Vec<String>>> = RwLock::new(None);

/// Replace the resolver (tests). `None` restores the egress gate.
pub fn set_resolver(r: Option<Resolver>) {
    *RESOLVER.write().unwrap_or_else(|e| e.into_inner()) = r;
}

/// Replace the recorder (tests). `None` restores the DB + audit one.
pub fn set_recorder(r: Option<Recorder>) {
    *RECORDER.write().unwrap_or_else(|e| e.into_inner()) = r;
}

#[cfg(test)]
pub(crate) fn set_own_names_for_test(names: Option<Vec<String>>) {
    *OWN_NAMES_FAKE.write().unwrap() = names;
}

fn resolve(host: &str, port: u16) -> Result<Vec<SocketAddr>, EgressError> {
    if let Some(r) = RESOLVER.read().unwrap_or_else(|e| e.into_inner()).as_ref() {
        return r(host, port);
    }
    crate::egress::vet_host_port(host, port)?.socket_addrs(port)
}

fn record(app: &str, host: &str, port: u16, through: bool, rule: Option<&HostSpec>, code: &str) {
    if let Some(r) = RECORDER.read().unwrap_or_else(|e| e.into_inner()).as_ref() {
        return r(app, host, port, through, rule, code);
    }
    record_db(app, host, port, through, rule, code);
}

/// This box's own names: `<primary>.k2.dev` (and every name under it:
/// nested App labels) and the custom hostnames bound here. Cached 60 s.
fn own_names() -> Arc<Vec<String>> {
    #[cfg(test)]
    if let Some(n) = OWN_NAMES_FAKE.read().unwrap().as_ref() {
        return Arc::new(n.clone());
    }
    if let Some((at, n)) = OWN_NAMES.read().unwrap_or_else(|e| e.into_inner()).as_ref() {
        if at.elapsed() < Duration::from_secs(60) {
            return n.clone();
        }
    }
    let mut names = Vec::new();
    let primary = k2_core::tunnel::subdomains::current().primary.clone();
    if !primary.is_empty() {
        names.push(format!("{primary}.{}", k2_core::tunnel::config::SUBDOMAIN_HOST));
    }
    if let Some(db) = k2_core::db::try_shared() {
        let conn = db.lock();
        if let Ok(rows) = k2_core::domains::list_names(&conn) {
            names.extend(rows.into_iter().map(|r| r.hostname.trim_end_matches('.').to_ascii_lowercase()));
        }
    }
    let n = Arc::new(names);
    *OWN_NAMES.write().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), n.clone()));
    n
}

/// Is `host` one of this box's own names (or under its tunnel name)?
pub fn is_own_name(host: &str) -> bool {
    let h = host.trim_start_matches("*.");
    own_names().iter().enumerate().any(|(i, n)| {
        // The first entry is the tunnel name: everything under it routes here.
        h == n || (i == 0 && h.ends_with(&format!(".{n}")))
    })
}

// ── policies ─────────────────────────────────────────────────────────

fn policies() -> &'static RwLock<HashMap<String, Arc<Policy>>> {
    static P: OnceLock<RwLock<HashMap<String, Arc<Policy>>>> = OnceLock::new();
    P.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Install an App's policy (tests, and [`reload`]).
pub fn set_policy(app: &str, p: Policy) {
    policies().write().unwrap_or_else(|e| e.into_inner()).insert(app.to_string(), Arc::new(p));
}

fn policy_of(app: &str) -> Option<Arc<Policy>> {
    policies().read().unwrap_or_else(|e| e.into_inner()).get(app).cloned()
}

/// Re-read an App's policy from the DB (after any change). Narrowing
/// applies to the next connection; open tunnels to a removed host are cut.
pub fn reload(app: &str) {
    let p = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        pol::policy(&conn, app)
    };
    match p {
        Ok(p) => {
            set_policy(app, p.clone());
            cut_unlisted(app, &p);
        }
        Err(e) => k2_core::log_debug!("[apps] egress policy {app}: {e}"),
    }
}

// ── builds (S16) ─────────────────────────────────────────────────────

/// The package registries an in-wall build may reach (HTTPS only),
/// through the App's own proxy, while the build runs: registry-only
/// egress (S16). Everything else stays the App's policy.
pub const BUILD_REGISTRIES: &[&str] = &["registry.npmjs.org", "registry.yarnpkg.com", "pypi.org", "files.pythonhosted.org"];

fn build_windows() -> &'static Mutex<HashMap<String, usize>> {
    static W: OnceLock<Mutex<HashMap<String, usize>>> = OnceLock::new();
    W.get_or_init(Default::default)
}

/// While this lives, `app`'s proxy also lets the package registries
/// through (a build in the App's slot). Dropping it closes the window and
/// cuts open registry tunnels the App's own policy doesn't allow.
pub struct BuildWindow {
    app: String,
}

pub fn build_window(app: &str) -> BuildWindow {
    *build_windows().lock().unwrap_or_else(|e| e.into_inner()).entry(app.to_string()).or_insert(0) += 1;
    BuildWindow { app: app.to_string() }
}

impl Drop for BuildWindow {
    fn drop(&mut self) {
        let closed = {
            let mut w = build_windows().lock().unwrap_or_else(|e| e.into_inner());
            match w.get_mut(&self.app) {
                Some(n) if *n > 1 => {
                    *n -= 1;
                    false
                }
                _ => {
                    w.remove(&self.app);
                    true
                }
            }
        };
        if closed {
            if let Some(p) = policy_of(&self.app) {
                cut_unlisted(&self.app, &p);
            }
        }
    }
}

/// A registry the open build window lets through (`registry.npmjs.org:443`).
fn build_allows(app: &str, host: &str, port: u16) -> Option<HostSpec> {
    let open = build_windows().lock().unwrap_or_else(|e| e.into_inner()).get(app).is_some_and(|n| *n > 0);
    (open && port == 443 && BUILD_REGISTRIES.contains(&host)).then(|| HostSpec { host: host.into(), port })
}

// ── the decision ─────────────────────────────────────────────────────

/// The policy part of a decision (no DNS): may `app` go to `host:port`?
/// `host` is normalized ([`pol::normalize_name`]).
pub fn check(app: &str, host: &str, port: u16) -> Result<Verdict, Refused> {
    let is_ip = host.parse::<std::net::IpAddr>().is_ok();
    if !is_ip && pol::local_name(host) {
        return Err(refused(403, codes::DENIED, format!("{host} is a local name; local names never leave the box")));
    }
    if !is_ip && is_own_name(host) {
        return Err(refused(
            403,
            codes::SELF,
            format!("{host} is this box: an App reaches K2 through its gateway (K2_GATEWAY_URL), never through the egress proxy"),
        ));
    }
    if let Some(r) = build_allows(app, host, port) {
        return Ok(Verdict::Listed(r));
    }
    let p = policy_of(app).ok_or_else(|| refused(403, codes::DENIED, format!("App {app} has no egress policy loaded")))?;
    match p.verdict(host, port) {
        Verdict::NotListed => Err(refused(
            403,
            codes::DENIED,
            format!(
                "{} isn't on App {app}'s egress allowlist. Ask for it (the owner approves): k2 app egress add {app} {}",
                HostSpec { host: host.into(), port }.display(),
                HostSpec { host: host.into(), port }.display()
            ),
        )),
        v => Ok(v),
    }
}

/// The seam for the vault key proxy (lane A, AR16/AR44): before the key
/// proxy dials an upstream for an App, it asks this App's egress policy
/// (same allowlist, own-name and local-name rules, same counters and
/// first-seen audit) and gets the vetted addresses to dial. The key proxy
/// keeps its own host binding per secret; both must pass.
pub fn check_for_key_proxy(app: &str, host: &str, port: u16) -> Result<Vec<SocketAddr>, Refused> {
    let host = pol::normalize_name(host).ok_or_else(|| refused(400, codes::BAD_REQUEST, "not a host name"))?;
    // The key proxy may be asked before the App's proxy ever started
    // (a stopped App, a fresh daemon): read the policy from the DB.
    if policy_of(app).is_none() {
        reload(app);
    }
    decide(app, &host, port).map(|(a, _)| a)
}

/// The full decision (blocking: DNS), recorded.
fn decide(app: &str, host: &str, port: u16) -> Result<(Vec<SocketAddr>, Verdict), Refused> {
    let v = match check(app, host, port) {
        Ok(v) => v,
        Err(r) => {
            record(app, host, port, false, None, r.code);
            return Err(r);
        }
    };
    match resolve(host, port) {
        Ok(addrs) if !addrs.is_empty() => {
            let rule = match &v {
                Verdict::Listed(r) => Some(r.clone()),
                _ => None,
            };
            record(app, host, port, true, rule.as_ref(), "ok");
            Ok((addrs, v))
        }
        Ok(_) => Err(refused(502, codes::UPSTREAM, format!("couldn't resolve {host}"))),
        Err(e) => {
            let r = match e.code {
                "address_not_allowed" => refused(403, codes::DENIED, format!("{host} points at a private, local or metadata address; Apps never reach those")),
                "airgap" => refused(403, codes::AIRGAP, e.hint.clone()),
                _ => refused(502, codes::UPSTREAM, e.hint.clone()),
            };
            if r.status == 403 {
                record(app, host, port, false, None, r.code);
            }
            Err(r)
        }
    }
}

/// Production recorder: counters, first-seen audit, and a note to the
/// App's agent the first time a host is refused.
fn record_db(app: &str, host: &str, port: u16, through: bool, rule: Option<&HostSpec>, code: &str) {
    // The reserved build slot is no App (no rows to count against); its
    // connections are in its egress log.
    if app == super::wall_build::BUILD_SLOT {
        return;
    }
    let now = k2_core::deploy::now();
    let (first, ws) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let first = pol::record(&conn, app, host, port, through, now).unwrap_or_else(|e| {
            k2_core::log_debug!("[apps] egress record {app}: {e}");
            false
        });
        if let Some(r) = rule.filter(|r| r.is_wildcard()) {
            let _ = pol::record_rule_hit(&conn, app, r, now);
        }
        if first {
            let payload = json!({ "host": host, "port": port, "allowed": through, "code": code });
            if let Err(e) = k2_core::deploy::audit::append(&conn, k2_core::deploy::audit::SUBJECT_APP, app, "egress_first_seen", "system", None, &payload) {
                k2_core::log_debug!("[apps] audit {app} egress_first_seen: {e}");
            }
        }
        let ws = k2_core::deploy::store::get_app(&conn, app).ok().flatten().map(|a| a.owner_workspace);
        (first, ws)
    };
    if first && !through && code == codes::DENIED {
        if let Some(ws) = ws {
            let spec = HostSpec { host: host.into(), port };
            super::alerts::notify(
                &ws,
                &format!(
                    "[k2 apps] App {app} tried to reach {spec}, which isn't on its egress allowlist (refused). \
                     If it needs it: k2 app egress add {app} {spec} — the owner approves. See k2 app egress {app}."
                ),
            );
        }
    }
}

// ── listeners ────────────────────────────────────────────────────────

struct Bucket {
    tokens: f64,
    at: Instant,
}

impl Bucket {
    fn take(&mut self) -> bool {
        let now = Instant::now();
        self.tokens = (self.tokens + now.duration_since(self.at).as_secs_f64() * RATE_PER_SEC).min(RATE_BURST);
        self.at = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// One App's proxy listener.
pub struct Listener {
    pub port: u16,
    stop: watch::Sender<bool>,
    active: Arc<AtomicUsize>,
    total: AtomicU64,
    refused: AtomicU64,
    bytes_up: AtomicU64,
    bytes_down: AtomicU64,
    bucket: Mutex<Bucket>,
    /// Open tunnels: (host, port) → cut signal.
    tunnels: Mutex<HashMap<u64, (String, u16, watch::Sender<bool>)>>,
    next_tunnel: AtomicU64,
}

fn listeners() -> &'static Mutex<HashMap<String, Arc<Listener>>> {
    static L: OnceLock<Mutex<HashMap<String, Arc<Listener>>>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(HashMap::new()))
}

fn runtime() -> Option<&'static tokio::runtime::Runtime> {
    static RT: OnceLock<Option<tokio::runtime::Runtime>> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("apps-egress")
            .enable_all()
            .build()
            .map_err(|e| k2_core::log_debug!("[apps] egress runtime: {e}"))
            .ok()
    })
    .as_ref()
}

/// Open (or keep) `app`'s proxy listener on `127.0.0.1:port` and load its
/// policy. Idempotent; a different port replaces the old listener.
pub fn ensure(app: &str, port: u16) -> Result<(), String> {
    {
        let mut g = listeners().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(l) = g.get(app) {
            if l.port == port {
                drop(g);
                reload(app);
                return Ok(());
            }
            let _ = l.stop.send(true);
            g.remove(app);
        }
    }
    reload(app);
    start_listener(app, port)
}

fn start_listener(app: &str, port: u16) -> Result<(), String> {
    let rt = runtime().ok_or("the egress runtime couldn't start")?;
    // R19 (update continuity, 0.46.4): the listener a hand-off's
    // predecessor passed over comes first. R36: while that predecessor
    // still holds the port (an older daemon that can't pass it), retry in
    // the background until it lets go; the App's egress is never lost.
    let std_l = match crate::handoff::take_inherited_app(app, crate::handoff::AppKind::Egress) {
        Some(l) => l,
        None => match std::net::TcpListener::bind(("127.0.0.1", port)) {
            Ok(l) => l,
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && crate::handoff::predecessor_alive() => {
                retry_while_predecessor_drains(app, port);
                return Err(format!("egress proxy for {app} on 127.0.0.1:{port}: held by the previous K2 daemon while it drains; retrying"));
            }
            Err(e) => return Err(format!("egress proxy for {app} on 127.0.0.1:{port}: {e}")),
        },
    };
    std_l.set_nonblocking(true).map_err(|e| e.to_string())?;
    let bound = std_l.local_addr().map_err(|e| e.to_string())?.port();
    let l = {
        let _g = rt.enter();
        Arc::new(tokio::net::TcpListener::from_std(std_l).map_err(|e| e.to_string())?)
    };
    crate::handoff::register_app(app, crate::handoff::AppKind::Egress, &l);
    let (stop, mut stop_rx) = watch::channel(false);
    let lis = Arc::new(Listener {
        port: bound,
        stop,
        active: Arc::new(AtomicUsize::new(0)),
        total: AtomicU64::new(0),
        refused: AtomicU64::new(0),
        bytes_up: AtomicU64::new(0),
        bytes_down: AtomicU64::new(0),
        bucket: Mutex::new(Bucket { tokens: RATE_BURST, at: Instant::now() }),
        tunnels: Mutex::new(HashMap::new()),
        next_tunnel: AtomicU64::new(1),
    });
    listeners().lock().unwrap_or_else(|e| e.into_inner()).insert(app.to_string(), lis.clone());
    let app = app.to_string();
    rt.spawn(async move {
        loop {
            let conn_stop = stop_rx.clone();
            tokio::select! {
                a = l.accept() => match a {
                    Ok((c, _)) => {
                        let (app, lis, stop) = (app.clone(), lis.clone(), conn_stop);
                        // R37: the old daemon drains these before it exits.
                        let guard = crate::handoff::app_conn_guard();
                        tokio::spawn(async move {
                            serve(c, app, lis, stop).await;
                            drop(guard);
                        });
                    }
                    Err(e) => {
                        k2_core::log_debug!("[apps] egress accept {app}: {e}");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                },
                _ = stop_rx.changed() => break,
                // R37: stop accepting at a hand-off's quiesce; the socket stays
                // open for the successor (R19).
                _ = crate::handoff::quiesced() => break,
            }
        }
    });
    Ok(())
}

/// R36: re-open `app`'s proxy on `port` once the hand-off's predecessor
/// lets go of it (every 250 ms while it drains, one last try after it
/// exits). One retry thread per App.
fn retry_while_predecessor_drains(app: &str, port: u16) {
    static RETRYING: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    let set = RETRYING.get_or_init(|| Mutex::new(std::collections::HashSet::new()));
    if !set.lock().unwrap_or_else(|e| e.into_inner()).insert(app.to_string()) {
        return;
    }
    let app = app.to_string();
    let _ = std::thread::Builder::new().name("apps-egress-rebind".into()).spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(250));
            let gone = !crate::handoff::predecessor_alive();
            match ensure(&app, port) {
                Ok(()) => {
                    k2_core::log_debug!("[apps] {app}: egress proxy back on 127.0.0.1:{port} (the previous daemon let go)");
                    break;
                }
                Err(e) if gone => {
                    super::runtime_notes::error(&format!("{app}: egress proxy on 127.0.0.1:{port} after the hand-off: {e}"));
                    break;
                }
                Err(_) => {}
            }
        }
        set.lock().unwrap_or_else(|e| e.into_inner()).remove(&app);
    });
}

/// Close `app`'s listener and every tunnel it holds (stop, uninstall).
pub fn stop(app: &str) {
    if let Some(l) = listeners().lock().unwrap_or_else(|e| e.into_inner()).remove(app) {
        let _ = l.stop.send(true);
    }
    policies().write().unwrap_or_else(|e| e.into_inner()).remove(app);
}

/// The port `app`'s proxy listens on, if it does.
pub fn listening(app: &str) -> Option<u16> {
    listeners().lock().unwrap_or_else(|e| e.into_inner()).get(app).map(|l| l.port)
}

/// Counters for `k2 app egress`.
pub fn status_json(app: &str) -> J {
    match listeners().lock().unwrap_or_else(|e| e.into_inner()).get(app) {
        Some(l) => json!({
            "listening": true,
            "port": l.port,
            "active": l.active.load(Ordering::SeqCst),
            "connections": l.total.load(Ordering::SeqCst),
            "refused": l.refused.load(Ordering::SeqCst),
            "bytesUp": l.bytes_up.load(Ordering::SeqCst),
            "bytesDown": l.bytes_down.load(Ordering::SeqCst),
        }),
        None => json!({ "listening": false }),
    }
}

/// Cut open tunnels whose host the policy no longer lets through
/// (narrowing applies at once, not at the next connection only).
fn cut_unlisted(app: &str, p: &Policy) {
    let Some(l) = listeners().lock().unwrap_or_else(|e| e.into_inner()).get(app).cloned() else { return };
    let t = l.tunnels.lock().unwrap_or_else(|e| e.into_inner());
    for (host, port, cut) in t.values() {
        if matches!(p.verdict(host, *port), Verdict::NotListed) {
            let _ = cut.send(true);
        }
    }
}

// ── one connection ───────────────────────────────────────────────────

/// A parsed request head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Head {
    pub connect: bool,
    pub method: String,
    /// Normalized host (IPv6 without brackets).
    pub host: String,
    pub port: u16,
    /// Plain HTTP: the origin-form target (`/path?query`).
    pub path: String,
    /// Plain HTTP: header lines as sent (name, value), in order.
    pub headers: Vec<(String, String)>,
}

fn token_ok(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

/// Parse a proxy request head (without the final blank line).
pub(crate) fn parse_head(head: &str) -> Result<Head, Refused> {
    let bad = |m: &str| refused(400, codes::BAD_REQUEST, m.to_string());
    let mut lines = head.split("\r\n");
    let first = lines.next().unwrap_or("");
    let mut parts = first.split(' ');
    let (method, target, version) = match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(m), Some(t), Some(v), None) => (m, t, v),
        _ => return Err(bad("malformed request line")),
    };
    if !token_ok(method) || !matches!(version, "HTTP/1.1" | "HTTP/1.0") {
        return Err(bad("malformed request line"));
    }
    let mut headers = Vec::new();
    for l in lines {
        if l.is_empty() {
            continue;
        }
        if l.starts_with(' ') || l.starts_with('\t') {
            return Err(bad("folded header lines are refused"));
        }
        let (n, v) = l.split_once(':').ok_or_else(|| bad("malformed header line"))?;
        if !token_ok(n) || v.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0) {
            return Err(bad("malformed header line"));
        }
        headers.push((n.to_string(), v.trim().to_string()));
    }
    if method == "CONNECT" {
        let (h, p) = if let Some(rest) = target.strip_prefix('[') {
            let (h, after) = rest.split_once(']').ok_or_else(|| bad("CONNECT needs host:port"))?;
            (h, after.strip_prefix(':').ok_or_else(|| bad("CONNECT needs host:port"))?)
        } else {
            target.rsplit_once(':').ok_or_else(|| bad("CONNECT needs host:port"))?
        };
        let port = p.parse::<u16>().ok().filter(|p| *p > 0).ok_or_else(|| bad("CONNECT needs a port 1-65535"))?;
        let host = pol::normalize_name(h).ok_or_else(|| bad("CONNECT to something that isn't a host name"))?;
        return Ok(Head { connect: true, method: method.into(), host, port, path: String::new(), headers });
    }
    if target.starts_with('/') || target == "*" {
        return Err(refused(
            400,
            codes::BAD_REQUEST,
            "this is the App's egress proxy: send absolute http:// URLs or CONNECT host:port (K2 itself is the gateway, K2_GATEWAY_URL)",
        ));
    }
    let lower = target.to_ascii_lowercase();
    if lower.starts_with("https://") {
        return Err(bad("an https:// URL goes through CONNECT (set HTTPS_PROXY), never as a plain proxy request"));
    }
    if !lower.starts_with("http://") {
        return Err(bad("only http:// URLs and CONNECT"));
    }
    let url = url::Url::parse(target).map_err(|_| bad("malformed URL"))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(bad("no user info in proxy URLs"));
    }
    let host = url.host_str().and_then(pol::normalize_name).ok_or_else(|| bad("the URL has no host"))?;
    let port = url.port_or_known_default().unwrap_or(80);
    let mut path = url.path().to_string();
    if let Some(q) = url.query() {
        path.push('?');
        path.push_str(q);
    }
    Ok(Head { connect: false, method: method.into(), host, port, path, headers })
}

/// The head sent upstream for a plain-HTTP request: origin-form target,
/// `Host` forced to the target, hop-by-hop and proxy headers dropped,
/// `Connection: close` (one request per connection), K2's default
/// `User-Agent` only when the App sent none.
pub(crate) fn upstream_head(h: &Head) -> Vec<u8> {
    let host = HostSpec { host: h.host.clone(), port: h.port };
    let host_hdr = if h.port == 80 {
        if h.host.contains(':') {
            format!("[{}]", h.host)
        } else {
            h.host.clone()
        }
    } else {
        host.display()
    };
    let mut out = format!("{} {} HTTP/1.1\r\nHost: {host_hdr}\r\n", h.method, h.path);
    let mut ua = false;
    for (n, v) in &h.headers {
        let l = n.to_ascii_lowercase();
        if matches!(l.as_str(), "host" | "connection" | "proxy-connection" | "proxy-authorization" | "proxy-authenticate" | "keep-alive" | "te" | "upgrade") {
            continue;
        }
        if l == "user-agent" {
            ua = true;
        }
        out.push_str(&format!("{n}: {v}\r\n"));
    }
    if !ua {
        out.push_str(&format!("User-Agent: {}\r\n", crate::egress::USER_AGENT));
    }
    out.push_str("Connection: close\r\n\r\n");
    out.into_bytes()
}

/// Read up to the end of the head. Returns (head text, bytes after it).
async fn read_head(c: &mut TcpStream) -> Result<(String, Vec<u8>), Refused> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 2048];
    let deadline = tokio::time::Instant::now() + HEAD_TIMEOUT;
    loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8(buf[..i].to_vec()).map_err(|_| refused(400, codes::BAD_REQUEST, "the request head isn't text"))?;
            return Ok((head, buf[i + 4..].to_vec()));
        }
        if buf.len() > HEAD_MAX {
            return Err(refused(400, codes::BAD_REQUEST, format!("request heads are at most {} KiB", HEAD_MAX / 1024)));
        }
        match tokio::time::timeout_at(deadline, c.read(&mut chunk)).await {
            Ok(Ok(0)) => return Err(refused(400, codes::BAD_REQUEST, "closed before the request head ended")),
            Ok(Ok(n)) => buf.extend_from_slice(&chunk[..n]),
            Ok(Err(_)) => return Err(refused(400, codes::BAD_REQUEST, "read error")),
            Err(_) => return Err(refused(400, codes::BAD_REQUEST, "no request head within 10 s")),
        }
    }
}

/// The SNI in a TLS ClientHello record (`None` when there is none).
/// `Err` when the bytes aren't a well-formed ClientHello.
pub(crate) fn client_hello_sni(b: &[u8]) -> Result<Option<String>, ()> {
    let rd16 = |b: &[u8], i: usize| -> Result<usize, ()> { Ok(((*b.get(i).ok_or(())? as usize) << 8) | *b.get(i + 1).ok_or(())? as usize) };
    if b.first() != Some(&0x16) || b.len() < 9 {
        return Err(());
    }
    let rec_len = rd16(b, 3)?;
    let body = b.get(5..5 + rec_len).ok_or(())?;
    if body.first() != Some(&0x01) {
        return Err(());
    }
    let hs_len = ((body[1] as usize) << 16) | ((body[2] as usize) << 8) | body[3] as usize;
    let h = body.get(4..4 + hs_len).ok_or(())?;
    let mut i = 2 + 32; // version, random
    let sid = *h.get(i).ok_or(())? as usize;
    i += 1 + sid;
    let cs = rd16(h, i)?;
    i += 2 + cs;
    let comp = *h.get(i).ok_or(())? as usize;
    i += 1 + comp;
    if i == h.len() {
        return Ok(None);
    }
    let ext_len = rd16(h, i)?;
    i += 2;
    let end = i + ext_len;
    if end > h.len() {
        return Err(());
    }
    while i + 4 <= end {
        let ty = rd16(h, i)?;
        let len = rd16(h, i + 2)?;
        let data = h.get(i + 4..i + 4 + len).ok_or(())?;
        if ty == 0 {
            // server_name_list: u16 len, then (type u8, u16 len, name)*
            let mut j = 2;
            while j + 3 <= data.len() {
                let nt = data[j];
                let nl = rd16(data, j + 1)?;
                let name = data.get(j + 3..j + 3 + nl).ok_or(())?;
                if nt == 0 {
                    return Ok(Some(String::from_utf8(name.to_vec()).map_err(|_| ())?.to_ascii_lowercase()));
                }
                j += 3 + nl;
            }
            return Ok(None);
        }
        i += 4 + len;
    }
    Ok(None)
}

/// How long a CONNECT tunnel waits for the App's first bytes (the SNI
/// check) before splicing anyway.
#[cfg(not(test))]
const FIRST_BYTES_WAIT: Duration = Duration::from_secs(10);
#[cfg(test)]
const FIRST_BYTES_WAIT: Duration = Duration::from_millis(800);

/// After a CONNECT to a name: if the App speaks TLS, read the whole first
/// record into `have` (forwarded upstream after) and check its SNI names
/// the CONNECT host. Non-TLS bytes pass as they are. Cancel-safe: what was
/// read stays in `have`.
async fn check_sni(c: &mut TcpStream, have: &mut Vec<u8>, host: &str) -> Result<(), Refused> {
    let deadline = tokio::time::Instant::now() + FIRST_BYTES_WAIT;
    let mut chunk = [0u8; 4096];
    while have.is_empty() {
        match tokio::time::timeout_at(deadline, c.read(&mut chunk)).await {
            Ok(Ok(0)) | Ok(Err(_)) | Err(_) => return Ok(()),
            Ok(Ok(n)) => have.extend_from_slice(&chunk[..n]),
        }
    }
    if have[0] != 0x16 {
        return Ok(());
    }
    let deadline = tokio::time::Instant::now() + HEAD_TIMEOUT;
    loop {
        if have.len() >= 5 {
            let need = 5 + (((have[3] as usize) << 8) | have[4] as usize);
            if have.len() >= need {
                return match client_hello_sni(&have[..need]) {
                    Ok(Some(sni)) if sni.trim_end_matches('.') != host => Err(refused(
                        403,
                        codes::SNI,
                        format!("the TLS name {sni} isn't the CONNECT name {host}"),
                    )),
                    Ok(_) => Ok(()),
                    Err(()) => Err(refused(400, codes::BAD_REQUEST, "a malformed TLS ClientHello")),
                };
            }
        }
        if have.len() > 20 * 1024 {
            return Err(refused(400, codes::BAD_REQUEST, "a TLS ClientHello over 20 KiB"));
        }
        match tokio::time::timeout_at(deadline, c.read(&mut chunk)).await {
            Ok(Ok(0)) | Ok(Err(_)) | Err(_) => return Err(refused(400, codes::BAD_REQUEST, "the TLS ClientHello ended early")),
            Ok(Ok(n)) => have.extend_from_slice(&chunk[..n]),
        }
    }
}

struct ActiveGuard(Arc<AtomicUsize>);
impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

async fn serve(mut c: TcpStream, app: String, l: Arc<Listener>, stop: watch::Receiver<bool>) {
    let t0 = Instant::now();
    l.total.fetch_add(1, Ordering::SeqCst);
    let prev = l.active.fetch_add(1, Ordering::SeqCst);
    let _active = ActiveGuard(l.active.clone());
    let limit = if prev >= MAX_CONNS {
        Some(refused(429, codes::TOO_MANY, format!("at most {MAX_CONNS} egress connections at once per App")))
    } else if !l.bucket.lock().unwrap_or_else(|e| e.into_inner()).take() {
        Some(refused(429, codes::RATE, format!("at most {RATE_PER_SEC} new egress connections a second per App")))
    } else {
        None
    };
    if let Some(r) = limit {
        l.refused.fetch_add(1, Ordering::SeqCst);
        let _ = c.write_all(&refusal_bytes(&app, &r)).await;
        log_line(&app, json!({ "result": "refused", "code": r.code, "ms": t0.elapsed().as_millis() as u64 }));
        return;
    }
    let (head, rest) = match read_head(&mut c).await {
        Ok(h) => h,
        Err(r) => return refuse(&mut c, &app, &l, &r, None, t0).await,
    };
    let h = match parse_head(&head) {
        Ok(h) => h,
        Err(r) => return refuse(&mut c, &app, &l, &r, None, t0).await,
    };
    let (app2, host2, port) = (app.clone(), h.host.clone(), h.port);
    let decided = tokio::task::spawn_blocking(move || decide(&app2, &host2, port)).await;
    let addrs = match decided {
        Ok(Ok((a, _))) => a,
        Ok(Err(r)) => return refuse(&mut c, &app, &l, &r, Some(&h), t0).await,
        Err(_) => return refuse(&mut c, &app, &l, &refused(502, codes::UPSTREAM, "egress check failed"), Some(&h), t0).await,
    };
    let mut up = match connect_any(&addrs).await {
        Ok(s) => s,
        Err(why) => {
            let r = refused(502, codes::UPSTREAM, format!("couldn't connect to {}: {why}", HostSpec { host: h.host.clone(), port: h.port }));
            return refuse(&mut c, &app, &l, &r, Some(&h), t0).await;
        }
    };
    let mut first = rest;
    if h.connect {
        if c.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await.is_err() {
            return;
        }
        // CONNECT to a name: a TLS client speaks first, and its SNI must be
        // the CONNECT name. A server-first protocol (the upstream speaks
        // first) isn't TLS yet: nothing to check.
        if h.host.parse::<std::net::IpAddr>().is_err() {
            let sni = tokio::select! {
                r = check_sni(&mut c, &mut first, &h.host) => r,
                _ = up.readable() => Ok(()),
            };
            if let Err(r) = sni {
                // The 200 is out: close without an HTTP answer.
                l.refused.fetch_add(1, Ordering::SeqCst);
                log_line(&app, json!({ "kind": "connect", "host": h.host, "port": h.port, "result": "refused", "code": r.code, "ms": t0.elapsed().as_millis() as u64 }));
                return;
            }
        }
    }
    let lead = if h.connect { first } else { [upstream_head(&h), first].concat() };
    if !lead.is_empty() && up.write_all(&lead).await.is_err() {
        return;
    }
    // Register the tunnel so narrowing can cut it.
    let (cut_tx, cut_rx) = watch::channel(false);
    let id = l.next_tunnel.fetch_add(1, Ordering::SeqCst);
    l.tunnels.lock().unwrap_or_else(|e| e.into_inner()).insert(id, (h.host.clone(), h.port, cut_tx));
    let (upb, downb, how) = splice(c, up, stop, cut_rx).await;
    // What went up before the splice (the SNI-checked first bytes, or the
    // rewritten HTTP head and the body bytes read with it).
    let upb = upb + lead.len() as u64;
    l.tunnels.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
    l.bytes_up.fetch_add(upb, Ordering::SeqCst);
    l.bytes_down.fetch_add(downb, Ordering::SeqCst);
    log_line(
        &app,
        json!({
            "kind": if h.connect { "connect" } else { "http" },
            "host": h.host, "port": h.port, "result": "ok", "end": how,
            "up": upb, "down": downb, "ms": t0.elapsed().as_millis() as u64,
        }),
    );
}

async fn refuse(c: &mut TcpStream, app: &str, l: &Listener, r: &Refused, h: Option<&Head>, t0: Instant) {
    l.refused.fetch_add(1, Ordering::SeqCst);
    let _ = c.write_all(&refusal_bytes(app, r)).await;
    let _ = c.shutdown().await;
    let mut line = json!({ "result": "refused", "code": r.code, "status": r.status, "ms": t0.elapsed().as_millis() as u64 });
    if let Some(h) = h {
        line["kind"] = json!(if h.connect { "connect" } else { "http" });
        line["host"] = json!(h.host);
        line["port"] = json!(h.port);
    }
    log_line(app, line);
}

async fn connect_any(addrs: &[SocketAddr]) -> Result<TcpStream, String> {
    let t = crate::egress::Purpose::AppEgress.connect_timeout();
    let mut last = "no address".to_string();
    for a in addrs {
        match tokio::time::timeout(t, TcpStream::connect(a)).await {
            Ok(Ok(s)) => {
                let _ = s.set_nodelay(true);
                return Ok(s);
            }
            Ok(Err(e)) => last = e.kind().to_string(),
            Err(_) => last = "timed out".into(),
        }
    }
    Err(last)
}

/// A stream that counts bytes read and stamps the last activity.
struct Tracked<S> {
    inner: S,
    read: Arc<AtomicU64>,
    last: Arc<AtomicU64>,
    epoch: Instant,
}

impl<S: AsyncRead + Unpin> AsyncRead for Tracked<S> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let r = Pin::new(&mut self.inner).poll_read(cx, buf);
        let n = buf.filled().len() - before;
        if n > 0 {
            self.read.fetch_add(n as u64, Ordering::Relaxed);
            self.last.store(self.epoch.elapsed().as_millis() as u64, Ordering::Relaxed);
        }
        r
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Tracked<S> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, b: &[u8]) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, b)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// Splice until both sides close, the tunnel idles past [`IDLE`], the App
/// stops, or a narrowing cuts it. Returns (bytes up, bytes down, why).
async fn splice(c: TcpStream, up: TcpStream, mut stop: watch::Receiver<bool>, mut cut: watch::Receiver<bool>) -> (u64, u64, &'static str) {
    let epoch = Instant::now();
    let last = Arc::new(AtomicU64::new(0));
    let (ru, rd) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)));
    let mut a = Tracked { inner: c, read: ru.clone(), last: last.clone(), epoch };
    let mut b = Tracked { inner: up, read: rd.clone(), last: last.clone(), epoch };
    let idle = async {
        loop {
            tokio::time::sleep(IDLE / 4).await;
            let quiet = (epoch.elapsed().as_millis() as u64).saturating_sub(last.load(Ordering::Relaxed));
            if quiet >= IDLE.as_millis() as u64 {
                return;
            }
        }
    };
    let how = tokio::select! {
        _ = tokio::io::copy_bidirectional(&mut a, &mut b) => "closed",
        _ = idle => "idle",
        _ = stop.changed() => "app_stopped",
        _ = cut.changed() => "narrowed",
    };
    (ru.load(Ordering::Relaxed), rd.load(Ordering::Relaxed), how)
}

// ── the log ──────────────────────────────────────────────────────────

fn log_lock() -> &'static Mutex<()> {
    static L: Mutex<()> = Mutex::new(());
    &L
}

/// `~/.k2/apps/<id>/logs/egress.log`.
pub fn log_path(app: &str) -> std::path::PathBuf {
    k2_core::deploy::app_dir(app).join("logs").join("egress.log")
}

fn log_line(app: &str, mut v: J) {
    v["at"] = json!(k2_core::deploy::now());
    let p = log_path(app);
    let _g = log_lock().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if std::fs::metadata(&p).map(|m| m.len() > LOG_ROLL_BYTES).unwrap_or(false) {
        let _ = std::fs::rename(&p, p.with_extension("log.1"));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let _ = writeln!(f, "{v}");
    }
}

/// The last `n` log lines (newest last), parsed.
pub fn tail(app: &str, n: usize) -> Vec<J> {
    let text = std::fs::read_to_string(log_path(app)).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n.clamp(1, 2000))..].iter().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

#[cfg(test)]
#[path = "egress_proxy_tests.rs"]
mod tests;
