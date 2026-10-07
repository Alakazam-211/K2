//! Official Dannon skin gateway helper (`k2-daemon --skin-gateway`).
//!
//! Child of k2-daemon (`kind=skin`). Serves static UI + HttpOnly cookie
//! on this origin + allowlisted Thread proxy. Holds `k2skn_` in **this
//! process memory only**. Never binds inside the parent daemon.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

pub const COOKIE_NAME: &str = "k2_skin_ui";
pub const GATEWAY_FORBIDDEN_JSON: &str = r#"{"error":"not allowed"}"#;
/// 403 body for a state-changing request or socket that came from another
/// origin (security-review-app-cookie-csrf-v1 §6 option 1). Written before
/// the session lookup and before any upstream connect.
pub const CROSS_ORIGIN_REFUSED_JSON: &str =
    r#"{"error":"cross-origin request refused","code":"cross_origin_refused"}"#;
const LOGIN_HTML: &str = include_str!("login.html");
const RESET_HTML: &str = include_str!("reset.html");
const APP_HTML: &str = include_str!("app.html");
const APP_CSS: &str = include_str!("app.css");
const APP_JS: &str = include_str!("app.js");

/// Cookie `Max-Age` and gateway map lifetime: the same TTL the daemon
/// gives the `k2skn_` session pass it mints on `/cli/skin/login`
/// (`skin::create_session_token` uses `connect_users::session_ttl_days`).
fn session_max_age_secs() -> i64 {
    k2_core::connect_users::session_ttl_days().saturating_mul(86_400)
}

#[derive(Clone)]
struct Session {
    token: String,
    /// Dropped from the map once past this; the pass is expired upstream
    /// by then too.
    expires: Instant,
}

struct Gateway {
    upstream_host: String,
    root: Option<PathBuf>,
    sessions: Mutex<HashMap<String, Session>>,
}

#[derive(Debug)]
struct Args {
    listen: SocketAddr,
    upstream: String,
    root: Option<PathBuf>,
}

/// Test-harness opt-in: when this env is `1` the helper exits as soon as
/// the process that spawned it is gone. Production never sets it — helpers
/// are `setsid` session leaders MEANT to survive a daemon restart
/// (`publish_runtime::boot_desired_running` reattaches by pid + port). Set
/// by the integration harnesses so a killed / timed-out test binary can
/// not leave `k2-daemon --skin-gateway` orphans (2026-09-12: two survived
/// a `cargo test` run and were killed by hand).
pub const EXIT_WITH_PARENT_ENV: &str = "K2_PUBLISH_HELPER_EXIT_WITH_PARENT";

/// Poll `getppid()`; the daemon that spawned us dying reparents this
/// helper (to 1 / a subreaper), which is the exit signal. Unix only.
fn exit_with_parent_if_requested() {
    let requested = std::env::var(EXIT_WITH_PARENT_ENV)
        .map(|v| v.trim() == "1")
        .unwrap_or(false);
    if !requested {
        return;
    }
    #[cfg(unix)]
    {
        // SAFETY: getppid has no preconditions and cannot fail.
        let parent = unsafe { libc::getppid() };
        std::thread::Builder::new()
            .name("skin-gateway-parent-watch".into())
            .spawn(move || loop {
                std::thread::sleep(Duration::from_millis(250));
                // SAFETY: as above.
                let now = unsafe { libc::getppid() };
                if now != parent {
                    eprintln!("skin-gateway: parent {parent} gone (now {now}); exiting");
                    std::process::exit(0);
                }
            })
            .expect("spawn parent-watch thread");
    }
}

/// Entry from `k2-daemon --skin-gateway …`. Does **not** boot the daemon.
pub fn run_from_args(args: &[String]) -> i32 {
    exit_with_parent_if_requested();
    match parse_args(args) {
        Ok(a) => {
            let rt = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    eprintln!("skin-gateway runtime: {e}");
                    return 1;
                }
            };
            match rt.block_on(serve(a)) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("skin-gateway: {e}");
                    1
                }
            }
        }
        Err(e) => {
            eprintln!("{e}");
            2
        }
    }
}

fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut listen: Option<SocketAddr> = None;
    let mut upstream: Option<String> = None;
    let mut root: Option<PathBuf> = None;
    let mut i = 1usize;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "--skin-gateway" {
            i += 1;
            continue;
        }
        let (key, inline) = if let Some(rest) = a.strip_prefix("--listen=") {
            ("--listen", Some(rest.to_string()))
        } else if let Some(rest) = a.strip_prefix("--upstream=") {
            ("--upstream", Some(rest.to_string()))
        } else if let Some(rest) = a.strip_prefix("--root=") {
            ("--root", Some(rest.to_string()))
        } else {
            (a, None)
        };
        match key {
            "--listen" => {
                let v = match inline {
                    Some(v) => v,
                    None => {
                        i += 1;
                        args.get(i).cloned().ok_or("--listen needs 127.0.0.1:PORT")?
                    }
                };
                let addr: SocketAddr = v
                    .parse()
                    .map_err(|_| format!("invalid --listen {v}"))?;
                if !addr.ip().is_loopback() {
                    return Err("--listen must be loopback 127.0.0.1".into());
                }
                listen = Some(addr);
            }
            "--upstream" => {
                let v = match inline {
                    Some(v) => v,
                    None => {
                        i += 1;
                        args.get(i)
                            .cloned()
                            .ok_or("--upstream needs http://127.0.0.1:PORT")?
                    }
                };
                if !v.starts_with("http://127.0.0.1:") && !v.starts_with("http://localhost:") {
                    return Err("--upstream must be http://127.0.0.1:<daemon-port>".into());
                }
                upstream = Some(v.trim_end_matches('/').to_string());
            }
            "--root" => {
                let v = match inline {
                    Some(v) => v,
                    None => {
                        i += 1;
                        args.get(i).cloned().ok_or("--root needs a directory")?
                    }
                };
                root = Some(PathBuf::from(v));
            }
            "--help" | "-h" => {
                return Err(
                    "k2-daemon --skin-gateway --listen 127.0.0.1:N --upstream http://127.0.0.1:DAEMON [--root DIR]"
                        .into(),
                );
            }
            _ => return Err(format!("unknown skin-gateway flag {a}")),
        }
        i += 1;
    }
    let listen = listen.ok_or("missing --listen 127.0.0.1:N")?;
    let upstream = upstream.ok_or("missing --upstream http://127.0.0.1:DAEMON")?;
    Ok(Args {
        listen,
        upstream,
        root,
    })
}

/// Synthesize helper argv (no exe). Daemon port is **current**, never persisted.
pub fn helper_argv(listen_port: u16, daemon_port: u16, root: Option<&Path>) -> Vec<String> {
    let mut v = vec![
        "--skin-gateway".into(),
        "--listen".into(),
        format!("127.0.0.1:{listen_port}"),
        "--upstream".into(),
        format!("http://127.0.0.1:{daemon_port}"),
    ];
    if let Some(root) = root {
        v.push("--root".into());
        v.push(root.display().to_string());
    }
    v
}

pub fn never_proxy(path: &str) -> bool {
    let p = path_only(path);
    matches!(
        p,
        "/cli/sessions/grid"
            | "/cli/sessions/bytes"
            | "/cli/sessions/events"
            | "/cli/sessions/subscribe"
            | "/cli/chat/transcript"
            | "/cli/grid"
            | "/cli/pty"
            | "/cli/auth/login"
            | "/events"
    ) || p.starts_with("/cli/terminal/")
        || p == "/cli/terminal"
        || p.starts_with("/v1/")
        || p == "/v1"
}

/// Every `(method, path)` the helper forwards with the guest's pass. The
/// one list: [`allowlisted_http`] reads it, and the cross-origin tests walk
/// it, so a new mutating route is covered the moment it is added here.
pub const HTTP_ALLOWLIST: &[(&str, &str)] = &[
    ("GET", "/cli/skin/agents"),
    ("HEAD", "/cli/skin/agents"),
    ("GET", "/cli/thread"),
    ("HEAD", "/cli/thread"),
    ("POST", "/cli/thread/post"),
    ("POST", "/cli/thread/answer"),
    ("POST", "/cli/thread/void"),
    ("GET", "/cli/fs/read-dir"),
    ("HEAD", "/cli/fs/read-dir"),
    ("GET", "/cli/fs/read-file"),
    ("HEAD", "/cli/fs/read-file"),
    ("GET", "/cli/fs/read-binary"),
    ("HEAD", "/cli/fs/read-binary"),
    ("GET", "/cli/fs/read-range"),
    ("HEAD", "/cli/fs/read-range"),
    ("POST", "/cli/fs/write-file"),
    ("POST", "/cli/fs/upload-binary"),
    ("POST", "/cli/fs/create"),
    ("POST", "/cli/fs/copy"),
    ("POST", "/cli/fs/move"),
    ("POST", "/cli/fs/rename"),
    ("GET", "/cli/workspace/resources"),
    ("POST", "/cli/workspace/resources/add"),
    ("POST", "/cli/workspace/resources/remove"),
    ("POST", "/cli/workspace/ensure-pinned-chat"),
    ("GET", "/cli/feedback/list"),
    ("HEAD", "/cli/feedback/list"),
    ("GET", "/cli/feedback/show"),
    ("HEAD", "/cli/feedback/show"),
    ("POST", "/cli/feedback/create"),
    ("POST", "/cli/feedback/comment"),
    ("POST", "/cli/feedback/answer"),
    ("POST", "/cli/feedback/resolve"),
    ("POST", "/cli/feedback/assign"),
    ("GET", "/cli/wiki/index"),
    ("HEAD", "/cli/wiki/index"),
    ("GET", "/cli/wiki/note"),
    ("HEAD", "/cli/wiki/note"),
    ("GET", "/cli/store/list"),
    ("HEAD", "/cli/store/list"),
    ("GET", "/cli/store/get"),
    ("HEAD", "/cli/store/get"),
    ("GET", "/cli/store/query"),
    ("HEAD", "/cli/store/query"),
    ("GET", "/cli/db/tables"),
    ("HEAD", "/cli/db/tables"),
    ("GET", "/cli/db/rows"),
    ("HEAD", "/cli/db/rows"),
    ("POST", "/cli/db/rows"),
    ("POST", "/cli/db/rows/update"),
    ("POST", "/cli/db/rows/delete"),
    ("POST", "/cli/db/query"),
    ("POST", "/cli/skin/password/change"),
    // App heartbeats (prd-app-heartbeats-surface-v1 AH5/AH6).
    // Reads: heartbeats:read. Writes: POST only, heartbeats:write.
    ("GET", "/cli/heartbeat/list"),
    ("HEAD", "/cli/heartbeat/list"),
    ("GET", "/cli/heartbeat/show"),
    ("HEAD", "/cli/heartbeat/show"),
    ("GET", "/cli/heartbeat/status"),
    ("HEAD", "/cli/heartbeat/status"),
    ("GET", "/cli/heartbeat/fires-list"),
    ("HEAD", "/cli/heartbeat/fires-list"),
    ("POST", "/cli/heartbeat/add"),
    ("POST", "/cli/heartbeat/edit"),
    ("POST", "/cli/heartbeat/enable"),
    ("POST", "/cli/heartbeat/rename"),
    ("POST", "/cli/heartbeat/archive"),
    ("POST", "/cli/heartbeat/fire"),
    // Agent working (prd-daemon-activity-and-thread-working-v1 AP3): the
    // room's guest snapshot, activity:read. GET only.
    ("GET", "/cli/activity/snapshot"),
];

/// POST routes the helper answers itself (login, logout, password). They
/// are not proxied as-is but they change state, so the cross-origin check
/// covers them (and the tests walk this list).
#[allow(dead_code)] // read by tests; the bin target compiles this module privately
pub const GATEWAY_POST_ROUTES: &[&str] = &[
    "/login",
    "/logout",
    "/cli/skin/password/reset",
    "/cli/skin/password/change",
];

pub fn allowlisted_http(method: &str, path: &str) -> bool {
    let p = path_only(path);
    let m = method.to_ascii_uppercase();
    HTTP_ALLOWLIST
        .iter()
        .any(|(am, ap)| *am == m.as_str() && *ap == p)
}

/// The one table of sockets the helper forwards, and the query parameter
/// each one must carry. A socket cannot be allowlisted without naming its
/// parameter (GA1). `/cli/activity/events` was once allowlisted without it,
/// so the helper asked it for `conversation=` and refused every app.
pub const WS_ALLOWLIST: &[(&str, &str)] = &[
    ("/cli/overlay/events", "conversation"),
    ("/cli/fs/events", "workspace"),
    ("/cli/activity/events", "workspace"),
];

pub fn ws_required_param(path: &str) -> Option<&'static str> {
    let p = path_only(path);
    WS_ALLOWLIST
        .iter()
        .find(|(wp, _)| *wp == p)
        .map(|(_, param)| *param)
}

pub fn allowlisted_ws(path: &str) -> bool {
    ws_required_param(path).is_some()
}

/// GA2: true when some `&`-separated pair has exactly `key` and a non-empty
/// value. `xworkspace=a` and `workspace=` do not count. The daemon still
/// validates the value.
fn query_has_param(query: &str, key: &str) -> bool {
    query.split('&').any(|pair| match pair.split_once('=') {
        Some((k, v)) => k == key && !v.is_empty(),
        None => false,
    })
}

pub fn is_static_path(path: &str) -> bool {
    let p = path_only(path);
    p == "/" || p == "/login" || p == "/reset" || p == "/index.html" || p.starts_with("/assets")
}

fn path_only(path: &str) -> &str {
    path.split('?').next().unwrap_or(path)
}

async fn serve(args: Args) -> Result<(), String> {
    if let Some(root) = args.root.as_ref() {
        eprintln!(
            "skin-gateway: {} is PUBLIC (no cookie). Login guards Thread and files. Do not put wiki, contracts, or other private files in this folder.",
            root.display()
        );
    }
    let listener = TcpListener::bind(args.listen)
        .await
        .map_err(|e| format!("bind {}: {e}", args.listen))?;
    let host = args
        .upstream
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .to_string();
    let gw = Arc::new(Gateway {
        upstream_host: host,
        root: args.root,
        sessions: Mutex::new(HashMap::new()),
    });
    serve_listener(listener, gw).await
}

async fn serve_listener(listener: TcpListener, gw: Arc<Gateway>) -> Result<(), String> {
    loop {
        let (stream, _) = listener.accept().await.map_err(|e| format!("accept: {e}"))?;
        let gw = Arc::clone(&gw);
        tokio::spawn(async move {
            let _ = handle_conn(gw, stream).await;
        });
    }
}

async fn handle_conn(gw: Arc<Gateway>, mut stream: TcpStream) -> Result<(), ()> {
    let mut buf = vec![0u8; 64 * 1024];
    let n = match stream.read(&mut buf).await {
        Ok(0) | Err(_) => return Ok(()),
        Ok(n) => n,
    };
    let raw = &buf[..n];
    let header_end = raw.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4);
    let (header_bytes, early_body) = match header_end {
        Some(end) => (&raw[..end], &raw[end..]),
        None => (raw, &raw[0..0]),
    };
    let head = String::from_utf8_lossy(header_bytes);
    let first = head.lines().next().unwrap_or("");
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("GET").to_string();
    let target = parts.next().unwrap_or("/").to_string();
    let path = path_only(&target).to_string();
    let content_length = head.lines().find_map(|l| {
        l.to_ascii_lowercase()
            .strip_prefix("content-length:")
            .and_then(|v| v.trim().parse::<usize>().ok())
    }).unwrap_or(0);
    let mut body = early_body.to_vec();
    if body.len() < content_length {
        let need = content_length - body.len();
        let mut more = vec![0u8; need];
        let mut got = 0;
        while got < need {
            match stream.read(&mut more[got..]).await {
                Ok(0) | Err(_) => break,
                Ok(n) => got += n,
            }
        }
        body.extend_from_slice(&more[..got]);
    }
    if body.len() > content_length {
        body.truncate(content_length);
    }
    let upgrade = header_has_upgrade(&head);
    let cookie = extract_cookie(&head, COOKIE_NAME);
    let secure = request_is_secure(&head);

    if never_proxy(&path) {
        write_json(&mut stream, "403 Forbidden", GATEWAY_FORBIDDEN_JSON).await;
        return Ok(());
    }

    // CSRF / cross-site WebSocket hijack guard. Every state-changing
    // request (any method but GET/HEAD — including /login, /logout and
    // the password routes) and every WebSocket upgrade must come from this
    // app's own origin. Runs before the session lookup and before any
    // upstream connect, so a refused request never reaches the daemon.
    // Rule and rationale: [`check_request_origin`].
    if needs_origin_check(&method, upgrade) {
        if let Err(_why) = check_request_origin(&head) {
            write_json(&mut stream, "403 Forbidden", CROSS_ORIGIN_REFUSED_JSON).await;
            return Ok(());
        }
    }

    if upgrade {
        if !allowlisted_ws(&path) {
            write_json(&mut stream, "403 Forbidden", GATEWAY_FORBIDDEN_JSON).await;
            return Ok(());
        }
        let Some(sid) = cookie else {
            write_json(&mut stream, "401 Unauthorized", r#"{"error":"not logged in"}"#).await;
            return Ok(());
        };
        let token = session_token(&gw, &sid).await;
        let Some(token) = token else {
            write_json(&mut stream, "401 Unauthorized", r#"{"error":"not logged in"}"#).await;
            return Ok(());
        };
        proxy_upgrade(&gw, &mut stream, &head, &target, &token).await;
        return Ok(());
    }

    match (method.as_str(), path.as_str()) {
        ("POST", "/login") => {
            handle_login(&gw, &mut stream, &head, &body, secure).await;
        }
        ("POST", "/logout") => {
            handle_logout(&gw, &mut stream, cookie.as_deref(), secure).await;
        }
        // Public consume; do not require cookie.
        ("POST", "/cli/skin/password/reset") => {
            handle_password_reset(&gw, &mut stream, &body).await;
        }
        ("POST", "/cli/skin/password/change") => {
            handle_password_change(&gw, &mut stream, cookie.as_deref(), &body, secure).await;
        }
        (m, p) if (m == "GET" || m == "HEAD") && is_static_path(p) => {
            serve_static(&gw, &mut stream, p, m == "HEAD").await;
        }
        (m, p) if allowlisted_http(m, p) => {
            let Some(sid) = cookie else {
                write_json(&mut stream, "401 Unauthorized", r#"{"error":"not logged in"}"#).await;
                return Ok(());
            };
            let token = session_token(&gw, &sid).await;
            let Some(token) = token else {
                write_json(&mut stream, "401 Unauthorized", r#"{"error":"not logged in"}"#).await;
                return Ok(());
            };
            proxy_http(&gw, &mut stream, &method, &target, &head, &body, &token).await;
        }
        _ => {
            write_json(&mut stream, "404 Not Found", r#"{"error":"not found"}"#).await;
        }
    }
    Ok(())
}

fn header_has_upgrade(head: &str) -> bool {
    head.lines().any(|l| {
        let l = l.to_ascii_lowercase();
        l.starts_with("upgrade:") && l.contains("websocket")
    })
}

/// GET and HEAD are reads (credentialed CORS reads fail, and `no-cors`
/// responses are opaque). Everything else, and every upgrade, is checked.
fn needs_origin_check(method: &str, upgrade: bool) -> bool {
    upgrade || !(method.eq_ignore_ascii_case("GET") || method.eq_ignore_ascii_case("HEAD"))
}

/// Why a request was refused. Only for tests and logs; the client always
/// gets [`CROSS_ORIGIN_REFUSED_JSON`].
#[derive(Debug, PartialEq, Eq)]
enum OriginRefusal {
    MultipleOrigin,
    MultipleSecFetchSite,
    SecFetchSiteForeign,
    NullOriginWithoutSecFetch,
    BadHost,
    ForeignOrigin,
}

/// The CSRF rule for state-changing requests and WebSocket upgrades
/// (security-review-app-cookie-csrf-v1 §6 option 1).
///
/// 1. More than one `Origin` or `Sec-Fetch-Site` header: refused.
/// 2. `Sec-Fetch-Site`, when present, must be `same-origin` or `none`.
///    Browsers set it and page script cannot forge it.
/// 3. `Origin`, when present, must be this app's own origin:
///    `<scheme>://<Host>` (see [`request_scheme`]), default port ignored,
///    case-insensitive. On a loopback Host both `http://` and `https://`
///    are accepted (dev, `--skin-gateway` on 127.0.0.1, roadmap skin).
///    Anything with a path, userinfo, or another host is refused. `Host`
///    cannot be set by a browser, so it names the origin the page is on.
/// 4. `Origin: null` is refused unless `Sec-Fetch-Site` passed rule 2.
///    A same-origin page with `Referrer-Policy: no-referrer` sends
///    `Origin: null` on POST, and `Sec-Fetch-Site: same-origin` vouches for
///    it; sandboxed frames, `data:` URLs and cross-site redirects (the
///    other sources of `null`) get `cross-site` and fail rule 2.
/// 5. **Neither header present: allowed.** This is the server-caller
///    rule. Every current browser sends `Origin` on every non-GET/HEAD
///    request and on every WebSocket handshake, so a request with neither
///    signal is not a browser and cannot be carrying a victim's cookie. A
///    customer's own backend (BFF) that logs in through `/login` and
///    replays the `k2_skin_ui` cookie server-side, or `curl`, keeps
///    working, cookie or not. (The helper never accepts a browser
///    `Authorization` header, so "Bearer-only" callers have nothing to
///    reach here; BYO backends call the daemon directly with their pass.)
fn check_request_origin(head: &str) -> Result<(), OriginRefusal> {
    let origins = header_values(head, "origin");
    let sec_fetch = header_values(head, "sec-fetch-site");
    if origins.len() > 1 {
        return Err(OriginRefusal::MultipleOrigin);
    }
    if sec_fetch.len() > 1 {
        return Err(OriginRefusal::MultipleSecFetchSite);
    }
    if let Some(site) = sec_fetch.first() {
        let ok = site.eq_ignore_ascii_case("same-origin") || site.eq_ignore_ascii_case("none");
        if !ok {
            return Err(OriginRefusal::SecFetchSiteForeign);
        }
    }
    let Some(origin) = origins.first() else {
        // Rule 5 (no signals) or rule 2 passed with no Origin.
        return Ok(());
    };
    if origin.eq_ignore_ascii_case("null") {
        return if sec_fetch.is_empty() {
            Err(OriginRefusal::NullOriginWithoutSecFetch)
        } else {
            Ok(())
        };
    }
    let hosts = header_values(head, "host");
    if hosts.len() != 1 || hosts[0].is_empty() {
        return Err(OriginRefusal::BadHost);
    }
    if origin_matches_host(origin, hosts[0], request_scheme(head)) {
        Ok(())
    } else {
        Err(OriginRefusal::ForeignOrigin)
    }
}

/// True when `origin` is exactly `<scheme>://<host>` for this request.
fn origin_matches_host(origin: &str, host: &str, scheme: &str) -> bool {
    let Some((o_scheme, o_rest)) = origin.split_once("://") else {
        return false;
    };
    let o_scheme = o_scheme.to_ascii_lowercase();
    if o_scheme != "http" && o_scheme != "https" {
        return false;
    }
    if o_rest.is_empty()
        || o_rest
            .chars()
            .any(|c| matches!(c, '/' | '?' | '#' | '@' | ',' | '\\') || c.is_whitespace())
    {
        return false;
    }
    let scheme_ok = o_scheme == scheme || host_is_loopback(host);
    scheme_ok && normalize_authority(&o_scheme, o_rest) == normalize_authority(&o_scheme, host)
}

/// Lowercase `host[:port]` and drop the scheme's default port.
fn normalize_authority(scheme: &str, authority: &str) -> String {
    let a = authority.trim().to_ascii_lowercase();
    let default = if scheme == "https" { ":443" } else { ":80" };
    match a.strip_suffix(default) {
        Some(stripped) if !stripped.is_empty() => stripped.to_string(),
        _ => a,
    }
}

/// Host part of `host[:port]` / `[v6][:port]`.
fn host_name(host: &str) -> &str {
    let h = host.trim();
    if let Some(rest) = h.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    match h.rsplit_once(':') {
        Some((name, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => name,
        _ => h,
    }
}

/// 127.0.0.0/8, ::1, `localhost`, `*.localhost`. Browsers treat these as
/// secure contexts over plain http; the helper itself only binds loopback.
fn host_is_loopback(host: &str) -> bool {
    let name = host_name(host).to_ascii_lowercase();
    if name == "localhost" || name.ends_with(".localhost") {
        return true;
    }
    name.parse::<std::net::IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

/// The scheme the browser used to reach this app.
///
/// Behind the tunnel (`<label>.<sub>.k2.dev`, custom domains) TLS ends at
/// the daemon's tunnel TLS listener, which byte-splices to this helper and
/// adds **no** `X-Forwarded-Proto` (`tunnel_tls_listener.rs`,
/// `Route::Internal`). So the default for a non-loopback Host is `https`.
/// An explicit `X-Forwarded-Proto` (Caddy sets it on every proxied request)
/// wins: `http` from a plain-http front such as an air-gapped LAN door, or
/// `https` from a TLS front on loopback. A browser cannot add this header
/// to a cross-site request, so it only ever describes the sender's own
/// connection. Loopback Host with no header: `http` (dev).
fn request_scheme(head: &str) -> &'static str {
    if let Some(xfp) = header_value(head, "x-forwarded-proto") {
        let first = xfp.split(',').next().unwrap_or("").trim();
        if first.eq_ignore_ascii_case("https") {
            return "https";
        }
        if first.eq_ignore_ascii_case("http") {
            return "http";
        }
    }
    match header_value(head, "host") {
        Some(h) if host_is_loopback(h) => "http",
        _ => "https",
    }
}

/// `Secure` on the cookie: always, except a loopback/dev Host over http
/// or an explicit plain-http front ([`request_scheme`]).
fn request_is_secure(head: &str) -> bool {
    request_scheme(head) == "https"
}

/// Every value of header `name` (trimmed, empty values included), headers
/// only — the request line is skipped.
fn header_values<'a>(head: &'a str, name: &str) -> Vec<&'a str> {
    head.lines()
        .skip(1)
        .take_while(|l| !l.is_empty())
        .filter_map(|line| {
            let colon = line.find(':')?;
            line[..colon]
                .trim()
                .eq_ignore_ascii_case(name)
                .then(|| line[colon + 1..].trim())
        })
        .collect()
}

/// The `k2skn_` pass behind cookie `sid`, or `None` (unknown or expired;
/// an expired entry is dropped).
async fn session_token(gw: &Gateway, sid: &str) -> Option<String> {
    let mut g = gw.sessions.lock().await;
    match g.get(sid) {
        Some(s) if s.expires > Instant::now() => Some(s.token.clone()),
        Some(_) => {
            g.remove(sid);
            None
        }
        None => None,
    }
}

fn header_value<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    for line in head.lines() {
        let Some(colon) = line.find(':') else { continue };
        if line[..colon].eq_ignore_ascii_case(name) {
            let v = line[colon + 1..].trim();
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

fn extract_cookie(head: &str, name: &str) -> Option<String> {
    let raw = header_value(head, "cookie")?;
    for part in raw.split(';') {
        let part = part.trim();
        if let Some(v) = part.strip_prefix(name).and_then(|s| s.strip_prefix('=')) {
            let v = v.trim();
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// `HttpOnly; SameSite=Lax; Path=/; Max-Age=<pass TTL>`, plus `Secure`
/// unless the request is loopback/dev or an explicit plain-http front
/// ([`request_is_secure`]). Host-only (no `Domain`).
fn set_cookie_value(id: &str, secure: bool) -> String {
    let max_age = session_max_age_secs();
    let mut v = format!("{COOKIE_NAME}={id}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age}");
    if secure {
        v.push_str("; Secure");
    }
    v
}

fn clear_cookie_value(secure: bool) -> String {
    let mut v = format!("{COOKIE_NAME}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0");
    if secure {
        v.push_str("; Secure");
    }
    v
}

async fn handle_login(
    gw: &Gateway,
    stream: &mut TcpStream,
    head: &str,
    body: &[u8],
    secure: bool,
) {
    let ct = header_value(head, "content-type").unwrap_or("");
    let (username, password) = parse_login_body(body, ct);
    if username.is_empty() || password.is_empty() {
        write_json(
            stream,
            "401 Unauthorized",
            r#"{"error":"invalid username or password"}"#,
        )
        .await;
        return;
    }
    let payload = serde_json::json!({
        "username": username,
        "password": password,
    })
    .to_string();
    match upstream_json(
        gw,
        "POST",
        "/cli/skin/login",
        Some(payload.as_bytes()),
        None,
    )
    .await
    {
        Ok((status, resp_body)) => {
            if status != 200 {
                let body = if resp_body.contains("k2skn_") {
                    r#"{"error":"invalid username or password"}"#.to_string()
                } else {
                    resp_body
                };
                write_json(stream, status_line(status), &body).await;
                return;
            }
            let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&resp_body) else {
                write_json(stream, "502 Bad Gateway", r#"{"error":"login upstream"}"#).await;
                return;
            };
            let token = v
                .get("token")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string();
            if !token.starts_with("k2skn_") {
                write_json(stream, "502 Bad Gateway", r#"{"error":"login upstream"}"#).await;
                return;
            }
            if let Some(obj) = v.as_object_mut() {
                obj.remove("token");
            }
            let out = v.to_string();
            if out.contains("k2skn_") {
                write_json(stream, "502 Bad Gateway", r#"{"error":"login upstream"}"#).await;
                return;
            }
            let sid = opaque_session_id();
            {
                let now = Instant::now();
                let mut g = gw.sessions.lock().await;
                // Drop passes that have expired so the map cannot grow forever.
                g.retain(|_, s| s.expires > now);
                g.insert(
                    sid.clone(),
                    Session {
                        token,
                        expires: now
                            + Duration::from_secs(session_max_age_secs().max(0) as u64),
                    },
                );
            }
            write_json_cookie(stream, "200 OK", &out, &set_cookie_value(&sid, secure)).await;
        }
        Err(_) => {
            write_json(stream, "502 Bad Gateway", r#"{"error":"login upstream"}"#).await;
        }
    }
}

fn parse_login_body(body: &[u8], content_type: &str) -> (String, String) {
    if content_type.to_ascii_lowercase().contains("application/json")
        || body.first().copied() == Some(b'{')
    {
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) {
            let username = v
                .get("username")
                .or_else(|| v.get("name"))
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let password = v
                .get("password")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            return (username, password);
        }
    }
    let s = String::from_utf8_lossy(body);
    let mut username = String::new();
    let mut password = String::new();
    for pair in s.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            let k = k.trim();
            let v = urlencoding_lite(v);
            if k.eq_ignore_ascii_case("username") || k.eq_ignore_ascii_case("name") {
                username = v;
            } else if k.eq_ignore_ascii_case("password") {
                password = v;
            }
        }
    }
    (username, password)
}

fn urlencoding_lite(s: &str) -> String {
    let mut out = String::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(' '),
            b'%' if i + 2 < b.len() => {
                let hex = &s[i + 1..i + 3];
                if let Ok(v) = u8::from_str_radix(hex, 16) {
                    out.push(v as char);
                    i += 3;
                    continue;
                }
                out.push('%');
            }
            c => out.push(c as char),
        }
        i += 1;
    }
    out
}

fn opaque_session_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

async fn handle_logout(gw: &Gateway, stream: &mut TcpStream, cookie: Option<&str>, secure: bool) {
    if let Some(sid) = cookie {
        let token = {
            let mut g = gw.sessions.lock().await;
            g.remove(sid).map(|s| s.token)
        };
        if let Some(token) = token {
            let _ = upstream_json(gw, "POST", "/cli/skin/logout", Some(b"{}"), Some(&token)).await;
        }
    }
    write_json_cookie(
        stream,
        "200 OK",
        r#"{"ok":true}"#,
        &clear_cookie_value(secure),
    )
    .await;
}

async fn handle_password_reset(gw: &Gateway, stream: &mut TcpStream, body: &[u8]) {
    match upstream_json(
        gw,
        "POST",
        "/cli/skin/password/reset",
        Some(body),
        None,
    )
    .await
    {
        Ok((status, resp_body)) => {
            let body = if resp_body.contains("k2skn_") {
                r#"{"error":"invalid or expired reset token"}"#.to_string()
            } else {
                resp_body
            };
            write_json(stream, status_line(status), &body).await;
        }
        Err(_) => {
            write_json(stream, "502 Bad Gateway", r#"{"error":"reset upstream"}"#).await;
        }
    }
}

async fn handle_password_change(
    gw: &Gateway,
    stream: &mut TcpStream,
    cookie: Option<&str>,
    body: &[u8],
    secure: bool,
) {
    let Some(sid) = cookie else {
        write_json(stream, "401 Unauthorized", r#"{"error":"not logged in"}"#).await;
        return;
    };
    let token = session_token(gw, sid).await;
    let Some(token) = token else {
        write_json(stream, "401 Unauthorized", r#"{"error":"not logged in"}"#).await;
        return;
    };
    match upstream_json(
        gw,
        "POST",
        "/cli/skin/password/change",
        Some(body),
        Some(&token),
    )
    .await
    {
        Ok((status, resp_body)) => {
            let body = if resp_body.contains("k2skn_") {
                r#"{"error":"invalid password"}"#.to_string()
            } else {
                resp_body
            };
            if status == 200 {
                gw.sessions.lock().await.remove(sid);
                write_json_cookie(stream, "200 OK", &body, &clear_cookie_value(secure)).await;
            } else {
                write_json(stream, status_line(status), &body).await;
            }
        }
        Err(_) => {
            write_json(stream, "502 Bad Gateway", r#"{"error":"change upstream"}"#).await;
        }
    }
}

/// GET `/login`: `<dir>/login.html` when `--root` has it; else bundled.
/// Missing file is bundled, never 404 (130 regression).
fn login_static_bytes(root: Option<&Path>) -> Vec<u8> {
    if let Some(root) = root {
        if let Ok((_, bytes)) = read_static_file(root, "/login") {
            return bytes;
        }
    }
    LOGIN_HTML.as_bytes().to_vec()
}

/// GET `/reset`: `<dir>/reset.html` when `--root` has it; else bundled.
/// Missing file is bundled, never 404.
fn reset_static_bytes(root: Option<&Path>) -> Vec<u8> {
    if let Some(root) = root {
        if let Ok((_, bytes)) = read_static_file(root, "/reset") {
            return bytes;
        }
    }
    RESET_HTML.as_bytes().to_vec()
}

async fn serve_static(gw: &Gateway, stream: &mut TcpStream, path: &str, head_only: bool) {
    if path == "/login" {
        let body = login_static_bytes(gw.root.as_deref());
        write_bytes(
            stream,
            "200 OK",
            "text/html; charset=utf-8",
            &body,
            head_only,
        )
        .await;
        return;
    }
    if path == "/reset" {
        let body = reset_static_bytes(gw.root.as_deref());
        write_bytes(
            stream,
            "200 OK",
            "text/html; charset=utf-8",
            &body,
            head_only,
        )
        .await;
        return;
    }
    if let Some(root) = gw.root.as_ref() {
        match read_static_file(root, path) {
            Ok((ct, bytes)) => {
                write_bytes(stream, "200 OK", &ct, &bytes, head_only).await;
            }
            Err(_) => {
                write_json(stream, "404 Not Found", r#"{"error":"not found"}"#).await;
            }
        }
        return;
    }
    let (ct, body) = match path {
        "/" | "/index.html" => ("text/html; charset=utf-8", APP_HTML.as_bytes()),
        "/login" => ("text/html; charset=utf-8", LOGIN_HTML.as_bytes()),
        "/reset" => ("text/html; charset=utf-8", RESET_HTML.as_bytes()),
        "/assets/app.css" => ("text/css; charset=utf-8", APP_CSS.as_bytes()),
        "/assets/app.js" => ("text/javascript; charset=utf-8", APP_JS.as_bytes()),
        _ => {
            write_json(stream, "404 Not Found", r#"{"error":"not found"}"#).await;
            return;
        }
    };
    write_bytes(stream, "200 OK", ct, body, head_only).await;
}

fn read_static_file(root: &Path, url_path: &str) -> Result<(String, Vec<u8>), ()> {
    let rel = match url_path {
        "/" | "/index.html" => "index.html",
        "/login" => "login.html",
        "/reset" => "reset.html",
        p if p.starts_with('/') => &p[1..],
        p => p,
    };
    if rel.contains('\0') || rel.split('/').any(|c| c == "..") {
        return Err(());
    }
    let joined = root.join(rel);
    let canon = joined.canonicalize().map_err(|_| ())?;
    let root_c = root.canonicalize().map_err(|_| ())?;
    if !canon.starts_with(&root_c) {
        return Err(());
    }
    let bytes = std::fs::read(&canon).map_err(|_| ())?;
    let ct = match canon.extension().and_then(|s| s.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("json") => "application/json",
        _ => "application/octet-stream",
    };
    Ok((ct.to_string(), bytes))
}

fn upstream_addr(gw: &Gateway) -> Result<String, String> {
    Ok(gw.upstream_host.clone())
}

async fn connect_upstream(gw: &Gateway) -> Result<TcpStream, String> {
    let addr = upstream_addr(gw)?;
    tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(&addr))
        .await
        .map_err(|_| "upstream timeout".to_string())?
        .map_err(|e| format!("upstream connect: {e}"))
}

async fn upstream_json(
    gw: &Gateway,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
    bearer: Option<&str>,
) -> Result<(u16, String), String> {
    let mut up = connect_upstream(gw).await?;
    let payload = body.unwrap_or(&[]);
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n",
        gw.upstream_host
    );
    if let Some(t) = bearer {
        req.push_str(&format!("Authorization: Bearer {t}\r\n"));
    }
    if !payload.is_empty() {
        req.push_str("Content-Type: application/json\r\n");
        req.push_str(&format!("Content-Length: {}\r\n", payload.len()));
    } else if method == "POST" {
        req.push_str("Content-Length: 0\r\n");
    }
    req.push_str("\r\n");
    up.write_all(req.as_bytes()).await.map_err(|e| e.to_string())?;
    if !payload.is_empty() {
        up.write_all(payload).await.map_err(|e| e.to_string())?;
    }
    up.flush().await.map_err(|e| e.to_string())?;
    let (_head, status, body) = read_http_message(&mut up).await?;
    Ok((status, String::from_utf8_lossy(&body).into_owned()))
}

async fn read_http_message(stream: &mut TcpStream) -> Result<(String, u16, Vec<u8>), String> {
    let mut raw = Vec::new();
    let mut buf = [0u8; 8192];
    let header_end = loop {
        let n = tokio::time::timeout(Duration::from_secs(10), stream.read(&mut buf))
            .await
            .map_err(|_| "upstream header timeout".to_string())?
            .map_err(|e| e.to_string())?;
        if n == 0 {
            break None;
        }
        raw.extend_from_slice(&buf[..n]);
        if let Some(i) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            break Some(i + 4);
        }
        if raw.len() > 64 * 1024 {
            return Err("upstream headers too large".into());
        }
    };
    let Some(end) = header_end else {
        return Err("upstream closed".into());
    };
    let head = String::from_utf8_lossy(&raw[..end]).into_owned();
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(502);
    let clen = head.lines().find_map(|l| {
        l.to_ascii_lowercase()
            .strip_prefix("content-length:")
            .and_then(|v| v.trim().parse::<usize>().ok())
    });
    let mut body = raw[end..].to_vec();
    if let Some(need) = clen {
        while body.len() < need {
            let n = tokio::time::timeout(Duration::from_secs(10), stream.read(&mut buf))
                .await
                .map_err(|_| "upstream body timeout".to_string())?
                .map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&buf[..n]);
        }
        if body.len() > need {
            body.truncate(need);
        }
    }
    Ok((head, status, body))
}

/// Browser → gateway cookie; gateway → daemon Bearer. Never Cookie, never ?token=,
/// never forward browser Authorization.
async fn proxy_http(
    gw: &Gateway,
    client: &mut TcpStream,
    method: &str,
    target: &str,
    client_head: &str,
    body: &[u8],
    token: &str,
) {
    let path_q = if target.starts_with('/') {
        target
    } else {
        "/"
    };
    // Strip any incoming token= from the official origin.
    let (path, query) = match path_q.split_once('?') {
        Some((p, q)) => (p, q),
        None => (path_q, ""),
    };
    let query = strip_token_query(query);
    let target = if query.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{query}")
    };
    let mut up = match connect_upstream(gw).await {
        Ok(s) => s,
        Err(_) => {
            write_json(client, "502 Bad Gateway", r#"{"error":"upstream"}"#).await;
            return;
        }
    };
    let mut req = format!(
        "{method} {target} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n",
        gw.upstream_host
    );
    if !body.is_empty() {
        let ct = header_value(client_head, "content-type").unwrap_or("application/json");
        req.push_str(&format!("Content-Type: {ct}\r\n"));
        req.push_str(&format!("Content-Length: {}\r\n", body.len()));
    } else if method.eq_ignore_ascii_case("POST") {
        req.push_str("Content-Length: 0\r\n");
    }
    req.push_str("\r\n");
    if up.write_all(req.as_bytes()).await.is_err() {
        write_json(client, "502 Bad Gateway", r#"{"error":"upstream"}"#).await;
        return;
    }
    if !body.is_empty() && up.write_all(body).await.is_err() {
        write_json(client, "502 Bad Gateway", r#"{"error":"upstream"}"#).await;
        return;
    }
    let _ = up.flush().await;
    match read_http_message(&mut up).await {
        Ok((head, _status, body)) => {
            // Rebuild so Content-Length matches the body we actually hold.
            let first = head.lines().next().unwrap_or("HTTP/1.1 200 OK");
            let mut out = format!("{first}\r\n");
            for line in head.lines().skip(1) {
                let l = line.to_ascii_lowercase();
                if l.starts_with("content-length:")
                    || l.starts_with("transfer-encoding:")
                    || l.starts_with("connection:")
                    || is_cors_header(line)
                    || line.is_empty()
                {
                    continue;
                }
                out.push_str(line);
                out.push_str("\r\n");
            }
            out.push_str(&format!("Content-Length: {}\r\nConnection: close\r\n\r\n", body.len()));
            let _ = client.write_all(out.as_bytes()).await;
            let _ = client.write_all(&body).await;
            let _ = client.flush().await;
        }
        Err(_) => {
            write_json(client, "502 Bad Gateway", r#"{"error":"upstream"}"#).await;
        }
    }
}

/// `Access-Control-*` from the daemon (`ACAO: *`, `ACEH: *`) is never
/// passed to the browser: the helper serves one origin and needs no CORS.
fn is_cors_header(line: &str) -> bool {
    line.trim_start()
        .get(..15)
        .map(|p| p.eq_ignore_ascii_case("access-control-"))
        .unwrap_or(false)
}

/// Rebuild an upstream upgrade response head without CORS headers.
fn strip_cors_from_head(head: &str) -> String {
    let mut out = String::with_capacity(head.len());
    for line in head.split("\r\n") {
        if line.is_empty() {
            continue;
        }
        if is_cors_header(line) {
            continue;
        }
        out.push_str(line);
        out.push_str("\r\n");
    }
    out.push_str("\r\n");
    out
}

fn strip_token_query(query: &str) -> String {
    let mut out = String::new();
    for pair in query.split('&') {
        if pair.is_empty() || pair.starts_with("token=") {
            continue;
        }
        if !out.is_empty() {
            out.push('&');
        }
        out.push_str(pair);
    }
    out
}

async fn proxy_upgrade(
    gw: &Gateway,
    client: &mut TcpStream,
    client_head: &str,
    target: &str,
    token: &str,
) {
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p, q),
        None => (target, ""),
    };
    let query = strip_token_query(query);
    let Some(param) = ws_required_param(path) else {
        write_json(client, "403 Forbidden", GATEWAY_FORBIDDEN_JSON).await;
        return;
    };
    if !query_has_param(&query, param) {
        let body = format!(r#"{{"error":"missing {param} query parameter"}}"#);
        write_json(client, "400 Bad Request", &body).await;
        return;
    }
    let target = if query.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{query}")
    };
    let mut up = match connect_upstream(gw).await {
        Ok(s) => s,
        Err(_) => {
            write_json(client, "502 Bad Gateway", r#"{"error":"upstream"}"#).await;
            return;
        }
    };
    let mut req = format!(
        "GET {target} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {token}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n",
        gw.upstream_host
    );
    for name in [
        "sec-websocket-key",
        "sec-websocket-version",
        "sec-websocket-protocol",
        "sec-websocket-extensions",
        "origin",
    ] {
        if let Some(v) = header_value(client_head, name) {
            req.push_str(&format!("{name}: {v}\r\n"));
        }
    }
    req.push_str("\r\n");
    if up.write_all(req.as_bytes()).await.is_err() {
        write_json(client, "502 Bad Gateway", r#"{"error":"upstream"}"#).await;
        return;
    }
    let _ = up.flush().await;
    // Read the daemon's handshake reply head, drop CORS headers, then
    // splice. Bytes after the head (a refusal body, or the first frames)
    // are passed through untouched.
    let mut raw = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(i) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            break Some(i + 4);
        }
        if raw.len() > 64 * 1024 {
            break None;
        }
        match tokio::time::timeout(Duration::from_secs(10), up.read(&mut chunk)).await {
            Ok(Ok(n)) if n > 0 => raw.extend_from_slice(&chunk[..n]),
            _ => break None,
        }
    };
    let Some(end) = head_end else {
        if raw.is_empty() {
            write_json(client, "502 Bad Gateway", r#"{"error":"upstream"}"#).await;
        } else {
            // Unparseable head: forward as-is rather than guess.
            let _ = client.write_all(&raw).await;
        }
        return;
    };
    let head = String::from_utf8_lossy(&raw[..end]).into_owned();
    let out = strip_cors_from_head(&head);
    if client.write_all(out.as_bytes()).await.is_err() {
        return;
    }
    if end < raw.len() && client.write_all(&raw[end..]).await.is_err() {
        return;
    }
    let _ = client.flush().await;
    let _ = tokio::io::copy_bidirectional(client, &mut up).await;
}

fn status_line(code: u16) -> &'static str {
    match code {
        200 => "200 OK",
        401 => "401 Unauthorized",
        403 => "403 Forbidden",
        404 => "404 Not Found",
        400 => "400 Bad Request",
        429 => "429 Too Many Requests",
        502 => "502 Bad Gateway",
        _ => "500 Internal Server Error",
    }
}

async fn write_json(stream: &mut TcpStream, status: &str, body: &str) {
    write_bytes(stream, status, "application/json", body.as_bytes(), false).await;
}

async fn write_json_cookie(stream: &mut TcpStream, status: &str, body: &str, cookie: &str) {
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nCache-Control: no-store\r\nSet-Cookie: {cookie}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(resp.as_bytes()).await;
    let _ = stream.flush().await;
}

fn http_headers(status: &str, ct: &str, len: usize) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ct}\r\nContent-Length: {len}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n"
    )
}

async fn write_bytes(
    stream: &mut TcpStream,
    status: &str,
    ct: &str,
    body: &[u8],
    head_only: bool,
) {
    let resp = http_headers(status, ct, body.len());
    let _ = stream.write_all(resp.as_bytes()).await;
    if !head_only {
        let _ = stream.write_all(body).await;
    }
    let _ = stream.flush().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_proxy_covers_prd_g8() {
        for p in [
            "/cli/sessions/grid",
            "/cli/sessions/bytes",
            "/cli/sessions/events",
            "/cli/sessions/subscribe",
            "/cli/grid",
            "/cli/pty",
            "/cli/terminal/foo",
            "/cli/auth/login",
            "/v1/w",
            "/events",
        ] {
            assert!(never_proxy(p), "{p}");
        }
        assert!(!never_proxy("/cli/thread"));
        assert!(!never_proxy("/cli/thread/post"));
        assert!(!never_proxy("/cli/overlay/events"));
        assert!(!never_proxy("/cli/skin/agents"));
        assert!(!never_proxy("/cli/skin/password/reset"));
        assert!(!never_proxy("/cli/skin/password/change"));
        assert!(!never_proxy("/cli/skin/password/forgot"));
    }

    #[test]
    fn allowlist_is_exact_not_thread_star() {
        assert!(allowlisted_http("GET", "/cli/thread"));
        assert!(allowlisted_http("POST", "/cli/thread/post"));
        assert!(allowlisted_http("POST", "/cli/thread/answer"));
        assert!(allowlisted_http("POST", "/cli/thread/void"));
        assert!(!allowlisted_http("GET", "/cli/thread/post"));
        assert!(!allowlisted_http("GET", "/cli/thread/foo"));
        assert!(!allowlisted_http("GET", "/cli/thread/answer"));
        assert!(!allowlisted_http("POST", "/cli/thread"));
        assert!(!allowlisted_http("POST", "/cli/thread/ask"));
        assert!(!allowlisted_http("POST", "/cli/thread/secret"));
        assert!(allowlisted_ws("/cli/overlay/events?conversation=abc"));
        assert!(allowlisted_ws("/cli/fs/events?workspace=docs"));
        assert!(allowlisted_http("GET", "/cli/fs/read-dir"));
        assert!(allowlisted_http("HEAD", "/cli/fs/read-dir"));
        assert!(allowlisted_http("GET", "/cli/fs/read-file"));
        assert!(allowlisted_http("HEAD", "/cli/fs/read-file"));
        assert!(allowlisted_http("POST", "/cli/fs/write-file"));
        assert!(allowlisted_http("GET", "/cli/fs/read-binary"));
        assert!(allowlisted_http("HEAD", "/cli/fs/read-binary"));
        assert!(allowlisted_http("GET", "/cli/fs/read-range"));
        assert!(allowlisted_http("HEAD", "/cli/fs/read-range"));
        assert!(allowlisted_http("POST", "/cli/fs/upload-binary"));
        assert!(allowlisted_http("POST", "/cli/fs/create"));
        assert!(allowlisted_http("POST", "/cli/fs/copy"));
        assert!(allowlisted_http("POST", "/cli/fs/move"));
        assert!(allowlisted_http("POST", "/cli/workspace/ensure-pinned-chat"));
        assert!(!allowlisted_ws("/cli/sessions/events"));
        assert!(!allowlisted_http("GET", "/cli/fs/info"));
        assert!(!allowlisted_http("POST", "/cli/fs/delete"));
        assert!(!allowlisted_http("POST", "/cli/fs/upload-chunk"));
        assert!(!allowlisted_http("GET", "/cli/fs/events"));
        assert!(!allowlisted_http("POST", "/cli/fs/read-dir"));
        assert!(!allowlisted_http("GET", "/cli/fs/search-tree"));
        assert!(!never_proxy("/cli/fs/events"));
        assert!(!never_proxy("/cli/fs/read-dir"));
        assert!(!never_proxy("/cli/fs/read-binary"));
        assert!(!never_proxy("/cli/workspace/ensure-pinned-chat"));
        assert!(allowlisted_http("GET", "/cli/feedback/list"));
        assert!(allowlisted_http("HEAD", "/cli/feedback/list"));
        assert!(allowlisted_http("GET", "/cli/feedback/show"));
        assert!(allowlisted_http("HEAD", "/cli/feedback/show"));
        assert!(allowlisted_http("POST", "/cli/feedback/create"));
        assert!(allowlisted_http("POST", "/cli/feedback/comment"));
        assert!(allowlisted_http("POST", "/cli/feedback/answer"));
        assert!(allowlisted_http("POST", "/cli/feedback/resolve"));
        assert!(!allowlisted_http("GET", "/cli/feedback/waiting-count"));
        assert!(!allowlisted_http("GET", "/cli/feedback/list-all"));
        // prd-app-tickets-websocket-v1: assign is an app door (tickets:post).
        assert!(allowlisted_http("POST", "/cli/feedback/assign"));
        assert!(!allowlisted_http("GET", "/cli/feedback/assign"));
        assert!(!allowlisted_http("GET", "/cli/feedback/foo"));
        assert!(!allowlisted_http("POST", "/cli/feedback/foo"));
        assert!(!allowlisted_http("GET", "/cli/tickets"));
        assert!(!allowlisted_ws("/cli/feedback/list"));
        assert!(!allowlisted_ws("/cli/sessions/events"));
        assert!(allowlisted_http("GET", "/cli/wiki/index"));
        assert!(allowlisted_http("HEAD", "/cli/wiki/index"));
        assert!(allowlisted_http("GET", "/cli/wiki/note"));
        assert!(allowlisted_http("HEAD", "/cli/wiki/note"));
        assert!(!allowlisted_http("GET", "/cli/wiki/seed"));
        assert!(!allowlisted_http("POST", "/cli/wiki/seed"));
        assert!(!allowlisted_http("GET", "/cli/wiki/serve"));
        assert!(!allowlisted_http("POST", "/cli/wiki/serve"));
        assert!(!allowlisted_http("GET", "/cli/wiki/chat"));
        assert!(!allowlisted_http("POST", "/cli/wiki/chat"));
        assert!(!allowlisted_http("GET", "/cli/wiki/serve/status"));
        assert!(!allowlisted_http("GET", "/cli/wiki/status"));
        assert!(!allowlisted_http("GET", "/cli/wiki/foo"));
        assert!(!allowlisted_http("GET", "/cli/wiki/read"));
        assert!(!allowlisted_http("GET", "/cli/notes"));
        assert!(!allowlisted_http("POST", "/cli/wiki/index"));
        assert!(!allowlisted_ws("/cli/wiki/index"));
        assert!(!allowlisted_ws("/cli/wiki/note"));
        assert!(allowlisted_http("GET", "/cli/store/list"));
        assert!(allowlisted_http("HEAD", "/cli/store/list"));
        assert!(allowlisted_http("GET", "/cli/store/get"));
        assert!(allowlisted_http("HEAD", "/cli/store/get"));
        assert!(allowlisted_http("GET", "/cli/store/query"));
        assert!(allowlisted_http("HEAD", "/cli/store/query"));
        assert!(!allowlisted_http("POST", "/cli/store/put"));
        assert!(!allowlisted_http("POST", "/cli/store/create"));
        assert!(!allowlisted_http("POST", "/cli/store/rm"));
        assert!(!allowlisted_http("POST", "/cli/store/drop"));
        assert!(!allowlisted_http("GET", "/cli/store/put"));
        assert!(!allowlisted_http("GET", "/cli/store/foo"));
        assert!(!allowlisted_http("GET", "/cli/db/dsn"));
        assert!(!allowlisted_http("GET", "/cli/db/list"));
        assert!(!allowlisted_http("GET", "/cli/db/status"));
        assert!(!allowlisted_http("POST", "/cli/db/create"));
        assert!(allowlisted_http("GET", "/cli/db/tables"));
        assert!(allowlisted_http("HEAD", "/cli/db/tables"));
        assert!(allowlisted_http("GET", "/cli/db/rows"));
        assert!(allowlisted_http("HEAD", "/cli/db/rows"));
        assert!(allowlisted_http("POST", "/cli/db/rows"));
        assert!(allowlisted_http("POST", "/cli/db/rows/update"));
        assert!(allowlisted_http("POST", "/cli/db/rows/delete"));
        assert!(allowlisted_http("POST", "/cli/db/query"));
        assert!(!allowlisted_http("GET", "/cli/db/query"));
        assert!(!allowlisted_http("HEAD", "/cli/db/query"));
        assert!(!allowlisted_http("POST", "/cli/db/query/foo"));
        assert!(!allowlisted_ws("/cli/db/query"));
        assert!(!allowlisted_http("POST", "/cli/db/rows/foo"));
        assert!(!allowlisted_http("GET", "/cli/db/foo"));
        assert!(!allowlisted_http("GET", "/cli/db/dump"));
        assert!(!allowlisted_http("GET", "/cli/db/migrate"));
        assert!(!allowlisted_ws("/cli/db/tables"));
        assert!(!allowlisted_ws("/cli/db/rows"));
        assert!(!allowlisted_ws("/cli/store/list"));
        assert!(!allowlisted_ws("/cli/store/get"));
        assert!(!allowlisted_ws("/cli/store/query"));
        assert!(
            allowlisted_http("POST", "/cli/skin/password/change"),
            "change is cookie→Bearer"
        );
        assert!(
            !allowlisted_http("POST", "/cli/skin/password/reset"),
            "reset is public, not cookie-gated"
        );
        assert!(
            !allowlisted_http("POST", "/cli/skin/password/forgot"),
            "forgot 404s on --skin"
        );
        assert!(!allowlisted_http("GET", "/cli/skin/password/change"));
        assert!(!allowlisted_http("POST", "/cli/skin/users/password"));
        assert!(!allowlisted_http("POST", "/cli/skin/users/unlock"));
        assert!(!allowlisted_http("POST", "/cli/users/unlock"));
        assert!(
            !allowlisted_http("POST", "/cli/skin/users/email"),
            "users/email is roster mutate, not --skin"
        );
        assert!(
            !allowlisted_http("POST", "/cli/skin/users/full-name"),
            "users/full-name is owner roster mutate, not --skin"
        );
        assert!(
            !allowlisted_http("GET", "/cli/skin/grants"),
            "grants are owner-only, not --skin"
        );
        assert!(!allowlisted_http("POST", "/cli/skin/grants"));
        assert!(!allowlisted_http("POST", "/cli/skin/grants/delete"));
        assert!(!allowlisted_http("POST", "/cli/skin/grants/enabled"));
        assert!(!allowlisted_http("POST", "/cli/skin/grants/host"));
        assert!(!allowlisted_http("GET", "/cli/skin/templates"));
        assert!(!allowlisted_http("POST", "/cli/skin/templates"));
        assert!(!allowlisted_http("POST", "/cli/skin/templates/apply"));
        assert!(!allowlisted_http("POST", "/cli/skin/templates/lines"));
    }

    #[test]
    fn app_resource_activity_rename_allowlist_is_exact() {
        assert!(allowlisted_http("GET", "/cli/workspace/resources"));
        assert!(allowlisted_http(
            "GET",
            "/cli/workspace/resources?workspace=sales"
        ));
        assert!(allowlisted_http("POST", "/cli/workspace/resources/add"));
        assert!(allowlisted_http("POST", "/cli/workspace/resources/remove"));
        assert!(!allowlisted_http("POST", "/cli/workspace/resources"));
        assert!(!allowlisted_http("GET", "/cli/workspace/resources/add"));
        assert!(!allowlisted_http("GET", "/cli/workspace/resources/remove"));
        assert!(!allowlisted_http(
            "POST",
            "/cli/workspace/resources/add/foo"
        ));
        assert!(!allowlisted_http("GET", "/cli/workspace/resources/foo"));

        assert!(allowlisted_http("POST", "/cli/fs/rename"));
        assert!(!allowlisted_http("GET", "/cli/fs/rename"));
        assert!(!allowlisted_http("POST", "/cli/fs/rename/foo"));

        // AP3: the guest snapshot is one exact GET; the Thread strip's
        // catch-up and the chat transcript never are (AP1).
        assert!(allowlisted_http("GET", "/cli/activity/snapshot"));
        assert!(allowlisted_http("GET", "/cli/activity/snapshot?workspace=sales"));
        assert!(!allowlisted_http("POST", "/cli/activity/snapshot"));
        assert!(!allowlisted_http("HEAD", "/cli/activity/snapshot"));
        assert!(!allowlisted_http("GET", "/cli/activity/snapshot/foo"));
        assert!(!allowlisted_http("GET", "/cli/thread/activity"));
        assert!(!allowlisted_http("GET", "/cli/thread/activity?addr=sales"));
        assert!(!allowlisted_ws("/cli/thread/activity"));
        assert!(!allowlisted_http("GET", "/cli/chat/transcript"));
        assert!(never_proxy("/cli/chat/transcript"));
        assert!(never_proxy("/cli/chat/transcript?session=x"));

        assert!(allowlisted_ws("/cli/activity/events"));
        assert!(allowlisted_ws("/cli/activity/events?workspace=sales"));
        assert!(!allowlisted_ws("/cli/activity/events/foo"));
        assert!(!allowlisted_ws("/cli/activity/events/foo?workspace=sales"));
        assert!(!never_proxy("/cli/activity/events"));

        assert!(!allowlisted_ws("/cli/sessions/events"));
        assert!(!allowlisted_ws("/cli/sessions/events?path=/tmp/sales"));
        assert!(never_proxy("/cli/sessions/events"));
        assert!(!allowlisted_ws("/cli/awareness/subscribe"));
        assert!(!allowlisted_ws("/cli/ops/stream"));
        assert!(!allowlisted_http("GET", "/cli/sessions/events"));
        assert!(!allowlisted_http("GET", "/cli/awareness/subscribe"));
        assert!(!allowlisted_http("GET", "/cli/ops/stream"));
    }

    /// T2 (prd-app-heartbeats-surface-v1 AH5–AH7): exactly four read pairs
    /// and six POST pairs; every other heartbeat and power route in the
    /// route table stays off the helper.
    #[test]
    fn heartbeat_allowlist_is_exact_and_walks_the_route_table() {
        let reads = [
            "/cli/heartbeat/list",
            "/cli/heartbeat/show",
            "/cli/heartbeat/status",
            "/cli/heartbeat/fires-list",
        ];
        let writes = [
            "/cli/heartbeat/add",
            "/cli/heartbeat/edit",
            "/cli/heartbeat/enable",
            "/cli/heartbeat/rename",
            "/cli/heartbeat/archive",
            "/cli/heartbeat/fire",
        ];
        for p in reads {
            assert!(allowlisted_http("GET", p), "{p}");
            assert!(allowlisted_http("GET", &format!("{p}?workspace=sales")), "{p}");
            assert!(!allowlisted_http("POST", p), "POST {p} must not pass");
        }
        for p in writes {
            assert!(allowlisted_http("POST", p), "{p}");
            assert!(!allowlisted_http("GET", p), "GET {p} must not pass");
            assert!(!allowlisted_ws(p), "{p} is not a socket");
        }
        let mut walked = 0;
        for r in crate::routes::route_policy::ROUTES {
            let is_hb = r.path.starts_with("/cli/heartbeat") || r.path.starts_with("/cli/power");
            if !is_hb {
                continue;
            }
            walked += 1;
            let get_ok = allowlisted_http("GET", r.path);
            let post_ok = allowlisted_http("POST", r.path);
            if reads.contains(&r.path) {
                assert!(get_ok && !post_ok, "{} read pair", r.path);
            } else if writes.contains(&r.path) {
                assert!(post_ok && !get_ok, "{} write pair", r.path);
            } else {
                assert!(
                    !get_ok && !post_ok && !allowlisted_ws(r.path),
                    "{} must never be allowlisted (AH7)",
                    r.path
                );
            }
        }
        assert!(walked >= 25, "walked only {walked} heartbeat/power routes");
        for p in [
            "/cli/heartbeat/scheduler-status",
            "/cli/heartbeat/remove",
            "/cli/heartbeat/launch",
            "/cli/heartbeat/unarchive",
            "/cli/heartbeat/list-archived",
            "/cli/heartbeat/set-session",
            "/cli/heartbeat/wake",
            "/cli/heartbeat-log",
            "/cli/scheduler-tick",
        ] {
            assert!(!allowlisted_http("GET", p), "{p}");
            assert!(!allowlisted_http("POST", p), "{p}");
        }
    }

    #[test]
    fn ws_required_param_names_each_socket_ga1() {
        assert_eq!(
            ws_required_param("/cli/overlay/events"),
            Some("conversation")
        );
        assert_eq!(ws_required_param("/cli/fs/events"), Some("workspace"));
        assert_eq!(
            ws_required_param("/cli/activity/events?workspace=sales"),
            Some("workspace")
        );
        assert_eq!(ws_required_param("/cli/activity/events"), Some("workspace"));
        assert_eq!(ws_required_param("/cli/sessions/events"), None);
        assert_eq!(ws_required_param("/cli/activity/events/foo"), None);
        assert_eq!(ws_required_param("/cli/awareness/subscribe"), None);
        for p in [
            "/cli/overlay/events",
            "/cli/fs/events",
            "/cli/activity/events",
            "/cli/sessions/events",
            "/cli/activity/events/foo",
            "/cli/awareness/subscribe",
            "/cli/ops/stream",
            "/cli/grid",
            "/events",
        ] {
            assert_eq!(
                allowlisted_ws(p),
                ws_required_param(p).is_some(),
                "allowlisted_ws and ws_required_param disagree on {p}"
            );
        }
    }

    #[test]
    fn query_has_param_checks_the_key_not_a_substring_ga2() {
        assert!(query_has_param("workspace=a", "workspace"));
        assert!(query_has_param("x=1&workspace=a", "workspace"));
        assert!(!query_has_param("xworkspace=a", "workspace"));
        assert!(!query_has_param("workspace=", "workspace"));
        assert!(!query_has_param("workspace", "workspace"));
        assert!(!query_has_param("conversation=a", "workspace"));
        assert!(!query_has_param("", "workspace"));
        assert!(query_has_param("conversation=c1", "conversation"));
    }

    #[test]
    fn login_html_uses_root_file_else_bundled_never_404() {
        let bundled = login_static_bytes(None);
        let bundled_s = String::from_utf8_lossy(&bundled);
        assert!(
            bundled_s.contains("Sign in — K2"),
            "no --root must be bundled: {bundled_s}"
        );

        let dir = std::env::temp_dir().join(format!(
            "k2-skin-login-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).expect("temp skin dir");
        let missing = login_static_bytes(Some(&dir));
        let missing_s = String::from_utf8_lossy(&missing);
        assert!(
            missing_s.contains("Sign in — K2"),
            "missing login.html must stay bundled, never 404: {missing_s}"
        );
        assert_eq!(missing, bundled);

        let custom = "<!DOCTYPE html><title>Custom Skin Login 2.1</title>";
        std::fs::write(dir.join("login.html"), custom).expect("write login.html");
        let served = login_static_bytes(Some(&dir));
        let served_s = String::from_utf8_lossy(&served);
        assert!(
            served_s.contains("Custom Skin Login 2.1"),
            "present login.html must be served: {served_s}"
        );
        assert!(
            !served_s.contains("Sign in — K2"),
            "custom login must not be the bundled K2 title: {served_s}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn static_and_json_headers_are_no_store() {
        let h = http_headers("200 OK", "text/javascript; charset=utf-8", 12);
        assert!(h.contains("Cache-Control: no-store"), "{h}");
        assert!(h.contains("Content-Type: text/javascript; charset=utf-8"), "{h}");
    }

    #[test]
    fn login_path_is_static_and_not_never_proxy() {
        assert!(is_static_path("/login"));
        assert!(!never_proxy("/login"));
        assert_eq!(
            include_str!("login.html").contains("Sign in"),
            true,
            "bundled GET /login must be a form, not a 404"
        );
    }

    #[test]
    fn reset_path_is_static_and_not_never_proxy() {
        assert!(is_static_path("/reset"));
        assert!(!never_proxy("/reset"));
        let bundled = reset_static_bytes(None);
        let bundled_s = String::from_utf8_lossy(&bundled);
        assert!(
            bundled_s.contains("Reset password"),
            "no --root must be bundled: {bundled_s}"
        );
        assert!(
            bundled_s.contains("/cli/skin/password/reset"),
            "page POSTs consume body, not ?token=: {bundled_s}"
        );
        assert!(
            !bundled_s.contains("/cli/skin/password/reset?"),
            "consume must not put token on the URL: {bundled_s}"
        );
        assert!(
            bundled_s.contains("k2skn_"),
            "leak check must refuse k2skn_ in the response"
        );

        let dir = std::env::temp_dir().join(format!(
            "k2-skin-reset-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).expect("temp skin dir");
        let missing = reset_static_bytes(Some(&dir));
        let missing_s = String::from_utf8_lossy(&missing);
        assert!(
            missing_s.contains("Reset password"),
            "missing reset.html must stay bundled, never 404: {missing_s}"
        );
        assert_eq!(missing, bundled);
        let custom = "<!DOCTYPE html><title>Custom Skin Reset</title>";
        std::fs::write(dir.join("reset.html"), custom).expect("write reset.html");
        let served = reset_static_bytes(Some(&dir));
        let served_s = String::from_utf8_lossy(&served);
        assert!(
            served_s.contains("Custom Skin Reset"),
            "present reset.html must be served: {served_s}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cookie_name_is_k2_skin_ui_not_session() {
        assert_eq!(COOKIE_NAME, "k2_skin_ui");
        assert_ne!(COOKIE_NAME, "k2_skin_session");
        let v = set_cookie_value("abc", false);
        assert_eq!(
            v,
            "k2_skin_ui=abc; HttpOnly; SameSite=Lax; Path=/; Max-Age=604800",
            "loopback/dev cookie"
        );
        let s = set_cookie_value("abc", true);
        assert_eq!(
            s,
            "k2_skin_ui=abc; HttpOnly; SameSite=Lax; Path=/; Max-Age=604800; Secure"
        );
        assert_eq!(
            session_max_age_secs(),
            k2_core::connect_users::session_ttl_days() * 86_400,
            "cookie Max-Age is the session pass TTL"
        );
        assert_eq!(
            clear_cookie_value(true),
            "k2_skin_ui=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0; Secure"
        );
        assert!(!v.contains("k2skn_"));
    }

    // ---- Origin predicate (security-review-app-cookie-csrf-v1 §7.5) ----

    fn req(method: &str, extra: &str) -> String {
        format!("{method} /cli/thread/post HTTP/1.1\r\n{extra}\r\n")
    }

    #[test]
    fn origin_check_applies_to_writes_and_upgrades_only() {
        assert!(!needs_origin_check("GET", false));
        assert!(!needs_origin_check("HEAD", false));
        assert!(!needs_origin_check("get", false));
        assert!(needs_origin_check("GET", true), "every upgrade");
        for m in ["POST", "PUT", "PATCH", "DELETE", "OPTIONS", "post", "FOO"] {
            assert!(needs_origin_check(m, false), "{m}");
        }
    }

    #[test]
    fn origin_predicate_matrix() {
        let ok = |extra: &str| check_request_origin(&req("POST", extra));
        // Same origin, default port in Host or Origin either way, any case.
        assert_eq!(ok("Host: a.b.k2.dev\r\nOrigin: https://a.b.k2.dev\r\n"), Ok(()));
        assert_eq!(ok("Host: a.b.k2.dev:443\r\nOrigin: https://a.b.k2.dev\r\n"), Ok(()));
        assert_eq!(ok("Host: a.b.k2.dev\r\nOrigin: https://a.b.k2.dev:443\r\n"), Ok(()));
        assert_eq!(ok("Host: A.B.K2.DEV\r\nOrigin: HTTPS://a.b.k2.dev\r\n"), Ok(()));
        assert_eq!(ok("host: a.b.k2.dev\r\norigin: https://A.b.k2.dev\r\n"), Ok(()));
        assert_eq!(ok("Host: a.b.k2.dev:8443\r\nOrigin: https://a.b.k2.dev:8443\r\n"), Ok(()));
        // Port mismatch.
        assert_eq!(
            ok("Host: a.b.k2.dev:8443\r\nOrigin: https://a.b.k2.dev\r\n"),
            Err(OriginRefusal::ForeignOrigin)
        );
        // Trailing dot is a different string: refused (fail closed).
        assert_eq!(
            ok("Host: a.b.k2.dev\r\nOrigin: https://a.b.k2.dev.\r\n"),
            Err(OriginRefusal::ForeignOrigin)
        );
        // Sibling on the same site.
        assert_eq!(
            ok("Host: a.b.k2.dev\r\nOrigin: https://evil.k2.dev\r\n"),
            Err(OriginRefusal::ForeignOrigin)
        );
        assert_eq!(
            ok("Host: a.b.k2.dev\r\nOrigin: https://a.b.k2.dev.evil.com\r\n"),
            Err(OriginRefusal::ForeignOrigin)
        );
        // http on a non-loopback Host behind the tunnel: refused.
        assert_eq!(
            ok("Host: a.b.k2.dev\r\nOrigin: http://a.b.k2.dev\r\n"),
            Err(OriginRefusal::ForeignOrigin)
        );
        // ...unless a plain-http front says so.
        assert_eq!(
            ok("Host: 10.0.0.5:38471\r\nX-Forwarded-Proto: http\r\nOrigin: http://10.0.0.5:38471\r\n"),
            Ok(())
        );
        // Origin with a path, userinfo, query, or a list.
        for o in [
            "https://a.b.k2.dev/",
            "https://a.b.k2.dev/x",
            "https://u@a.b.k2.dev",
            "https://a.b.k2.dev?x",
            "https://a.b.k2.dev, https://a.b.k2.dev",
            "a.b.k2.dev",
            "ftp://a.b.k2.dev",
            "https://",
        ] {
            assert_eq!(
                ok(&format!("Host: a.b.k2.dev\r\nOrigin: {o}\r\n")),
                Err(OriginRefusal::ForeignOrigin),
                "{o}"
            );
        }
        // Several Origin headers.
        assert_eq!(
            ok("Host: a.b.k2.dev\r\nOrigin: https://a.b.k2.dev\r\nOrigin: https://a.b.k2.dev\r\n"),
            Err(OriginRefusal::MultipleOrigin)
        );
        // Empty Origin value is not "absent".
        assert_eq!(
            ok("Host: a.b.k2.dev\r\nOrigin: \r\n"),
            Err(OriginRefusal::ForeignOrigin)
        );
        // Missing or doubled Host with an Origin.
        assert_eq!(ok("Origin: https://a.b.k2.dev\r\n"), Err(OriginRefusal::BadHost));
        assert_eq!(
            ok("Host: a.b.k2.dev\r\nHost: evil.k2.dev\r\nOrigin: https://a.b.k2.dev\r\n"),
            Err(OriginRefusal::BadHost)
        );
        // Loopback dev: http and https, any loopback spelling.
        for (host, origin) in [
            ("127.0.0.1:38480", "http://127.0.0.1:38480"),
            ("127.0.0.1:38480", "https://127.0.0.1:38480"),
            ("localhost:5173", "http://localhost:5173"),
            ("[::1]:9000", "http://[::1]:9000"),
            ("app.localhost:9000", "http://app.localhost:9000"),
        ] {
            assert_eq!(
                ok(&format!("Host: {host}\r\nOrigin: {origin}\r\n")),
                Ok(()),
                "{host} {origin}"
            );
        }
        assert_eq!(
            ok("Host: 127.0.0.1:38480\r\nOrigin: http://127.0.0.1:9999\r\n"),
            Err(OriginRefusal::ForeignOrigin)
        );
        assert_eq!(
            ok("Host: 127.0.0.1:38480\r\nOrigin: http://localhost:38480\r\n"),
            Err(OriginRefusal::ForeignOrigin),
            "localhost and 127.0.0.1 are different origins"
        );
    }

    #[test]
    fn sec_fetch_site_and_null_origin_rules() {
        let ok = |extra: &str| check_request_origin(&req("POST", extra));
        let h = "Host: a.b.k2.dev\r\n";
        // No Origin: Sec-Fetch-Site decides.
        assert_eq!(ok(&format!("{h}Sec-Fetch-Site: same-origin\r\n")), Ok(()));
        assert_eq!(ok(&format!("{h}Sec-Fetch-Site: none\r\n")), Ok(()));
        assert_eq!(
            ok(&format!("{h}Sec-Fetch-Site: same-site\r\n")),
            Err(OriginRefusal::SecFetchSiteForeign)
        );
        assert_eq!(
            ok(&format!("{h}Sec-Fetch-Site: cross-site\r\n")),
            Err(OriginRefusal::SecFetchSiteForeign)
        );
        assert_eq!(
            ok(&format!("{h}Sec-Fetch-Site: same-origin\r\nSec-Fetch-Site: same-origin\r\n")),
            Err(OriginRefusal::MultipleSecFetchSite)
        );
        // A matching Origin does not excuse a foreign Sec-Fetch-Site.
        assert_eq!(
            ok(&format!("{h}Origin: https://a.b.k2.dev\r\nSec-Fetch-Site: cross-site\r\n")),
            Err(OriginRefusal::SecFetchSiteForeign)
        );
        // A same-origin Sec-Fetch-Site does not excuse a foreign Origin.
        assert_eq!(
            ok(&format!("{h}Origin: https://evil.k2.dev\r\nSec-Fetch-Site: same-origin\r\n")),
            Err(OriginRefusal::ForeignOrigin)
        );
        // Origin: null — refused alone, allowed when the browser vouches.
        assert_eq!(
            ok(&format!("{h}Origin: null\r\n")),
            Err(OriginRefusal::NullOriginWithoutSecFetch)
        );
        assert_eq!(
            ok(&format!("{h}Origin: null\r\nSec-Fetch-Site: cross-site\r\n")),
            Err(OriginRefusal::SecFetchSiteForeign)
        );
        assert_eq!(ok(&format!("{h}Origin: null\r\nSec-Fetch-Site: same-origin\r\n")), Ok(()));
        // Server-caller rule: no browser signal at all → allowed.
        assert_eq!(ok(h), Ok(()));
        assert_eq!(ok(&format!("{h}Cookie: k2_skin_ui=abc\r\n")), Ok(()));
    }

    #[test]
    fn scheme_and_secure_follow_host_and_explicit_front() {
        let sec = |extra: &str| request_is_secure(&req("POST", extra));
        assert!(sec("Host: a.b.k2.dev\r\n"), "tunnel default is https");
        assert!(sec("Host: app.customer.com\r\n"));
        assert!(sec(""), "no Host: assume https");
        assert!(!sec("Host: 127.0.0.1:38480\r\n"), "loopback dev");
        assert!(!sec("Host: localhost:5173\r\n"));
        assert!(sec("Host: 127.0.0.1:38480\r\nX-Forwarded-Proto: https\r\n"));
        assert!(!sec("Host: 10.0.0.5:38471\r\nX-Forwarded-Proto: http\r\n"));
        assert!(sec("Host: a.b.k2.dev\r\nX-Forwarded-Proto: https, http\r\n"));
        assert!(sec("Host: a.b.k2.dev\r\nX-Forwarded-Proto: bogus\r\n"));
    }

    #[test]
    fn cors_headers_are_recognised_any_case() {
        assert!(is_cors_header("Access-Control-Allow-Origin: *"));
        assert!(is_cors_header("access-control-expose-headers: *"));
        assert!(is_cors_header("ACCESS-CONTROL-ALLOW-CREDENTIALS: true"));
        assert!(!is_cors_header("Content-Type: application/json"));
        assert!(!is_cors_header("X-Access-Control: no"));
        let h = strip_cors_from_head(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nAccess-Control-Allow-Origin: *\r\n\r\n",
        );
        assert_eq!(h, "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\r\n");
    }

    #[test]
    fn route_tables_drive_the_allowlists() {
        // The table is the allowlist: every row passes, case-insensitive
        // method, and nothing outside it does.
        for (m, p) in HTTP_ALLOWLIST {
            assert!(allowlisted_http(m, p), "{m} {p}");
            assert!(allowlisted_http(&m.to_ascii_lowercase(), p), "{m} {p}");
        }
        assert!(HTTP_ALLOWLIST.iter().any(|(m, _)| *m == "POST"));
        for (p, param) in WS_ALLOWLIST {
            assert_eq!(ws_required_param(p), Some(*param));
        }
    }

    // ---- Stand-in upstream + live helper (no daemon, no real data) ----

    use std::sync::Mutex as StdMutex;

    struct Stub {
        port: u16,
        /// `METHOD /path?query` of every request the helper sent upstream.
        seen: Arc<StdMutex<Vec<String>>>,
    }

    impl Stub {
        fn seen(&self) -> Vec<String> {
            self.seen.lock().expect("stub log").clone()
        }
    }

    const STUB_PASS: &str = "k2skn_stubpassnotreal0000000000";

    /// A fake daemon: `/cli/skin/login` hands out a fake pass; upgrades get
    /// a 101 with CORS headers; everything else 200 JSON with the daemon's
    /// CORS headers so stripping can be observed.
    async fn start_stub() -> Stub {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("stub bind");
        let port = listener.local_addr().expect("stub addr").port();
        let seen = Arc::new(StdMutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = listener.accept().await else { return };
                let log = Arc::clone(&log);
                tokio::spawn(async move {
                    let mut raw = Vec::new();
                    let mut buf = [0u8; 4096];
                    let end = loop {
                        if let Some(i) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                            break i + 4;
                        }
                        match s.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => raw.extend_from_slice(&buf[..n]),
                        }
                    };
                    let head = String::from_utf8_lossy(&raw[..end]).into_owned();
                    let clen = header_value(&head, "content-length")
                        .and_then(|v| v.parse::<usize>().ok())
                        .unwrap_or(0);
                    while raw.len() - end < clen {
                        match s.read(&mut buf).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => raw.extend_from_slice(&buf[..n]),
                        }
                    }
                    let first = head.lines().next().unwrap_or("").to_string();
                    let mut parts = first.split_whitespace();
                    let line = format!(
                        "{} {}",
                        parts.next().unwrap_or(""),
                        parts.next().unwrap_or("")
                    );
                    log.lock().expect("stub log").push(line.clone());
                    let resp = if header_has_upgrade(&head) {
                        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: stub\r\nAccess-Control-Allow-Origin: *\r\n\r\n".to_string()
                    } else {
                        let body = if line.starts_with("POST /cli/skin/login") {
                            format!(r#"{{"ok":true,"token":"{STUB_PASS}","username":"stub"}}"#)
                        } else {
                            r#"{"ok":true}"#.to_string()
                        };
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Expose-Headers: *\r\nAccess-Control-Allow-Credentials: true\r\nX-Stub: yes\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        )
                    };
                    let _ = s.write_all(resp.as_bytes()).await;
                    let _ = s.flush().await;
                });
            }
        });
        Stub { port, seen }
    }

    async fn start_helper(upstream_port: u16) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("helper bind");
        let port = listener.local_addr().expect("helper addr").port();
        let gw = Arc::new(Gateway {
            upstream_host: format!("127.0.0.1:{upstream_port}"),
            root: None,
            sessions: Mutex::new(HashMap::new()),
        });
        tokio::spawn(async move {
            let _ = serve_listener(listener, gw).await;
        });
        port
    }

    struct Reply {
        status: u16,
        head: String,
        body: String,
    }

    /// One raw request; reads until the helper closes (it always does).
    async fn send(port: u16, method: &str, target: &str, headers: &str, body: &str) -> Reply {
        let mut s = TcpStream::connect(("127.0.0.1", port)).await.expect("connect helper");
        let req = if body.is_empty() && (method == "GET" || method == "HEAD") {
            format!("{method} {target} HTTP/1.1\r\n{headers}\r\n")
        } else {
            format!(
                "{method} {target} HTTP/1.1\r\n{headers}Content-Type: text/plain\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
        };
        s.write_all(req.as_bytes()).await.expect("write helper");
        let mut raw = Vec::new();
        tokio::time::timeout(Duration::from_secs(10), s.read_to_end(&mut raw))
            .await
            .expect("helper must answer and close within 10s")
            .expect("read helper");
        let text = String::from_utf8_lossy(&raw).into_owned();
        let (head, body) = text
            .split_once("\r\n\r\n")
            .unwrap_or_else(|| panic!("no HTTP head from helper: {text:?}"));
        let status = head
            .split_whitespace()
            .nth(1)
            .and_then(|c| c.parse().ok())
            .unwrap_or_else(|| panic!("bad status line: {head:?}"));
        Reply {
            status,
            head: head.to_string(),
            body: body.to_string(),
        }
    }

    fn set_cookie_of(r: &Reply) -> Option<String> {
        header_values(&format!("{}\r\n", r.head), "set-cookie")
            .first()
            .map(|v| v.to_string())
    }

    const APP: &str = "app.acme.k2.dev";
    const SAME: &str = "Origin: https://app.acme.k2.dev\r\n";

    /// Log in with a same-origin Origin; returns `Cookie: …` header line.
    async fn login(helper: u16) -> String {
        let r = send(
            helper,
            "POST",
            "/login",
            &format!("Host: {APP}\r\n{SAME}"),
            r#"{"username":"stub","password":"not-a-real-password"}"#,
        )
        .await;
        assert_eq!(r.status, 200, "same-origin login: {}", r.body);
        assert!(!r.body.contains("k2skn_"), "{}", r.body);
        let sc = set_cookie_of(&r).expect("Set-Cookie on login");
        let sid = sc
            .strip_prefix("k2_skin_ui=")
            .and_then(|v| v.split(';').next())
            .expect("k2_skin_ui value")
            .to_string();
        format!("Cookie: k2_skin_ui={sid}\r\n")
    }

    /// Every state-changing route the helper knows, from its own tables.
    fn mutating_routes() -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = HTTP_ALLOWLIST
            .iter()
            .filter(|(m, _)| *m != "GET" && *m != "HEAD")
            .map(|(m, p)| (m.to_string(), p.to_string()))
            .collect();
        for p in GATEWAY_POST_ROUTES {
            if !v.iter().any(|(_, vp)| vp == p) {
                v.push(("POST".into(), p.to_string()));
            }
        }
        v
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn skin_gateway_refuses_cross_origin_writes_and_upstream_never_called() {
        let stub = start_stub().await;
        let helper = start_helper(stub.port).await;
        let routes = mutating_routes();
        // 1 thread/post … heartbeat/fire + /login + /logout + reset.
        assert!(routes.len() >= 30, "walked only {} routes", routes.len());
        assert!(routes.iter().any(|(_, p)| p == "/login"));
        assert!(routes.iter().any(|(_, p)| p == "/logout"));
        assert!(routes.iter().any(|(_, p)| p == "/cli/skin/password/reset"));
        let cookie = login(helper).await;
        let foreign_cases = [
            "Origin: https://evil.k2.dev\r\n",
            "Origin: https://evil.k2.dev\r\nSec-Fetch-Site: same-site\r\n",
            "Origin: null\r\n",
            "Sec-Fetch-Site: cross-site\r\n",
            "Sec-Fetch-Site: same-site\r\n",
            "Origin: http://app.acme.k2.dev\r\n",
        ];
        for (m, p) in &routes {
            for bad in foreign_cases {
                let before = stub.seen();
                let r = send(
                    helper,
                    m,
                    &format!("{p}?addr=room"),
                    &format!("Host: {APP}\r\n{cookie}{bad}"),
                    r#"{"username":"stub","password":"x","addr":"room","text":"hi"}"#,
                )
                .await;
                assert_eq!(r.status, 403, "{m} {p} with {bad:?}: {}", r.body);
                assert_eq!(r.body, CROSS_ORIGIN_REFUSED_JSON, "{m} {p}");
                assert!(
                    set_cookie_of(&r).is_none(),
                    "a refusal must not set or clear the cookie: {m} {p}"
                );
                assert_eq!(
                    stub.seen(),
                    before,
                    "{m} {p} with {bad:?} must not reach the upstream"
                );
            }
        }
        // The session survived every refused /logout and password change.
        let still = send(
            helper,
            "POST",
            "/cli/thread/post",
            &format!("Host: {APP}\r\n{cookie}{SAME}"),
            r#"{"addr":"room","text":"still here"}"#,
        )
        .await;
        assert_eq!(still.status, 200, "session must survive refused logouts: {}", still.body);
        // An unknown POST is refused as cross-origin too, before routing.
        let unknown = send(
            helper,
            "POST",
            "/cli/not/a/route",
            &format!("Host: {APP}\r\n{cookie}Origin: https://evil.k2.dev\r\n"),
            "{}",
        )
        .await;
        assert_eq!(unknown.status, 403, "{}", unknown.body);
        assert_eq!(unknown.body, CROSS_ORIGIN_REFUSED_JSON);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn skin_gateway_same_origin_writes_reach_upstream_without_cors() {
        let stub = start_stub().await;
        let helper = start_helper(stub.port).await;
        let ok_cases = [
            SAME.to_string(),
            "Origin: https://app.acme.k2.dev:443\r\nSec-Fetch-Site: same-origin\r\n".to_string(),
            "Sec-Fetch-Site: same-origin\r\n".to_string(),
            "Origin: null\r\nSec-Fetch-Site: same-origin\r\n".to_string(),
            // Server-caller rule: no Origin and no Sec-Fetch-Site.
            String::new(),
        ];
        for (m, p) in mutating_routes() {
            if p == "/login" || p == "/logout" || p == "/cli/skin/password/change" {
                continue; // session-changing; covered below
            }
            for good in &ok_cases {
                let cookie = login(helper).await;
                let before = stub.seen().len();
                let r = send(
                    helper,
                    &m,
                    &format!("{p}?addr=room"),
                    &format!("Host: {APP}\r\n{cookie}{good}"),
                    r#"{"addr":"room","text":"hi"}"#,
                )
                .await;
                assert_eq!(r.status, 200, "{m} {p} with {good:?}: {}", r.body);
                let after = stub.seen();
                assert_eq!(after.len(), before + 1, "{m} {p} must reach upstream once");
                // Proxied routes keep the query; the helper's own reset
                // route calls the daemon on the bare path.
                let want = if GATEWAY_POST_ROUTES.contains(&p.as_str()) {
                    format!("{m} {p}")
                } else {
                    format!("{m} {p}?addr=room")
                };
                assert_eq!(after.last().expect("upstream line"), &want, "{m} {p}");
                let lower = r.head.to_ascii_lowercase();
                assert!(
                    !lower.contains("access-control-"),
                    "CORS headers must be stripped: {}",
                    r.head
                );
                if !GATEWAY_POST_ROUTES.contains(&p.as_str()) {
                    assert!(r.head.contains("X-Stub: yes"), "other headers kept: {}", r.head);
                }
            }
        }
        // Password change and logout, same origin: reach upstream and clear
        // the cookie with Secure (non-loopback Host).
        for p in ["/cli/skin/password/change", "/logout"] {
            let cookie = login(helper).await;
            let before = stub.seen().len();
            let r = send(
                helper,
                "POST",
                p,
                &format!("Host: {APP}\r\n{cookie}{SAME}"),
                r#"{"oldPassword":"x","password":"y"}"#,
            )
            .await;
            assert_eq!(r.status, 200, "{p}: {}", r.body);
            assert_eq!(stub.seen().len(), before + 1, "{p} upstream once");
            assert_eq!(
                set_cookie_of(&r).expect("clear cookie"),
                "k2_skin_ui=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0; Secure",
                "{p}"
            );
            let gone = send(
                helper,
                "POST",
                "/cli/thread/post",
                &format!("Host: {APP}\r\n{cookie}{SAME}"),
                "{}",
            )
            .await;
            assert_eq!(gone.status, 401, "{p} ends the session: {}", gone.body);
        }
        // Reads are not origin-gated (credentialed CORS reads fail anyway).
        let cookie = login(helper).await;
        let read = send(
            helper,
            "GET",
            "/cli/thread?addr=room",
            &format!("Host: {APP}\r\n{cookie}Origin: https://evil.k2.dev\r\n"),
            "",
        )
        .await;
        assert_eq!(read.status, 200, "{}", read.body);
        assert!(!read.head.to_ascii_lowercase().contains("access-control-"), "{}", read.head);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn skin_gateway_websocket_origin_and_cors() {
        let stub = start_stub().await;
        let helper = start_helper(stub.port).await;
        let cookie = login(helper).await;
        let up = "Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n";
        for (p, param) in WS_ALLOWLIST {
            let target = format!("{p}?{param}=room");
            for bad in [
                "Origin: https://evil.k2.dev\r\n",
                "Origin: null\r\n",
                "Origin: https://app.acme.k2.dev\r\nSec-Fetch-Site: cross-site\r\n",
            ] {
                let before = stub.seen();
                let r = send(helper, "GET", &target, &format!("Host: {APP}\r\n{cookie}{up}{bad}"), "")
                    .await;
                assert_eq!(r.status, 403, "{p} {bad:?}: {}", r.body);
                assert_eq!(r.body, CROSS_ORIGIN_REFUSED_JSON);
                assert_eq!(stub.seen(), before, "{p} {bad:?} must not dial upstream");
            }
            let before = stub.seen().len();
            let r = send(helper, "GET", &target, &format!("Host: {APP}\r\n{cookie}{up}{SAME}"), "")
                .await;
            assert_eq!(r.status, 101, "{p} same origin: {}", r.head);
            assert!(
                !r.head.to_ascii_lowercase().contains("access-control-"),
                "101 must not carry CORS: {}",
                r.head
            );
            assert!(r.head.contains("Sec-WebSocket-Accept: stub"), "{}", r.head);
            assert_eq!(stub.seen().len(), before + 1, "{p} dialled once");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn skin_gateway_cookie_attributes_by_host() {
        let stub = start_stub().await;
        let helper = start_helper(stub.port).await;
        let body = r#"{"username":"stub","password":"not-a-real-password"}"#;
        let max_age = session_max_age_secs();
        for (host, origin, xfp, secure) in [
            (APP, "https://app.acme.k2.dev", "", true),
            ("app.customer.example", "https://app.customer.example", "", true),
            ("127.0.0.1:38480", "http://127.0.0.1:38480", "", false),
            ("localhost:38480", "http://localhost:38480", "", false),
            ("127.0.0.1:38480", "https://127.0.0.1:38480", "X-Forwarded-Proto: https\r\n", true),
        ] {
            let r = send(
                helper,
                "POST",
                "/login",
                &format!("Host: {host}\r\nOrigin: {origin}\r\n{xfp}"),
                body,
            )
            .await;
            assert_eq!(r.status, 200, "{host}: {}", r.body);
            let sc = set_cookie_of(&r).expect("Set-Cookie");
            let sid = sc
                .strip_prefix("k2_skin_ui=")
                .and_then(|v| v.split(';').next())
                .expect("sid");
            let mut want =
                format!("k2_skin_ui={sid}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age}");
            if secure {
                want.push_str("; Secure");
            }
            assert_eq!(sc, want, "{host}");
            assert!(!sc.contains("Domain"), "host-only: {sc}");
        }
        assert_eq!(max_age, 7 * 86_400, "pass TTL is 7 days today");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn skin_gateway_expired_session_is_dropped() {
        let stub = start_stub().await;
        let gw = Gateway {
            upstream_host: format!("127.0.0.1:{}", stub.port),
            root: None,
            sessions: Mutex::new(HashMap::new()),
        };
        gw.sessions.lock().await.insert(
            "old".into(),
            Session {
                token: STUB_PASS.into(),
                expires: Instant::now() - Duration::from_secs(1),
            },
        );
        gw.sessions.lock().await.insert(
            "new".into(),
            Session {
                token: STUB_PASS.into(),
                expires: Instant::now() + Duration::from_secs(60),
            },
        );
        assert_eq!(session_token(&gw, "old").await, None);
        assert!(!gw.sessions.lock().await.contains_key("old"), "expired entry removed");
        assert_eq!(session_token(&gw, "new").await.as_deref(), Some(STUB_PASS));
    }

    #[test]
    fn helper_argv_does_not_embed_cmd_shell() {
        let a = helper_argv(8788, 4242, None);
        assert_eq!(a[0], "--skin-gateway");
        assert!(a.contains(&"127.0.0.1:8788".to_string()));
        assert!(a.contains(&"http://127.0.0.1:4242".to_string()));
        assert!(!a.iter().any(|s| s.contains("(skin)")));
    }

    #[test]
    fn parse_listen_rejects_non_loopback() {
        let err = parse_args(&[
            "k2-daemon".into(),
            "--skin-gateway".into(),
            "--listen".into(),
            "0.0.0.0:9".into(),
            "--upstream".into(),
            "http://127.0.0.1:8".into(),
        ])
        .unwrap_err();
        assert!(err.contains("loopback"), "{err}");
    }
}
