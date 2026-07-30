//! `/cli/mail/link/oauth/{start,status}` — the O4 OAuth LINK routes
//! (prd-email-oauth-providers-v1 §7). These begin an OAuth 2.0 link for a
//! Gmail (auth-code + loopback, §3b) or Microsoft 365 (device-code, §3a)
//! account and report progress — the daemon runs the whole flow
//! SERVER-SIDE; the CLI/UI only long-poll `status`.
//!
//! GATING: both routes are owner/Primary surface. `start` (POST) is
//! owner-or-admin-gated in the dispatcher (`is_owner_level_mutation`'s
//! `/cli/mail/link/oauth/` prefix); `status` (GET) has its own
//! owner-or-admin dispatcher clause (the `approvals/list` precedent). An
//! agent token never begins or observes a link.
//!
//! TOKENS + CODES NEVER LEAVE THE DAEMON. `start` returns only the
//! user-facing bits — Microsoft's `userCode`+`verificationUrl`, or (Gmail)
//! a "browser opened" acknowledgement. The `device_code`, the loopback
//! `code`, and every access/refresh token stay inside the daemon: the
//! server-side poll/exchange obtains them, `external::add_oauth_inbox`
//! VAULTS them (`ext-inbox-<id>-oauth`), and nothing here logs or returns
//! them (prd §7/§10, pre-mortem #9/#15).
//!
//! REMOTE-Gmail: Google's mail scope is not device-flow eligible, so Gmail
//! uses auth-code + loopback. When the desktop client is talking to a
//! **remote** daemon it passes `clientCapture: true` + a **client-local**
//! `redirectUri` (`http://127.0.0.1:<port>/cb`): the daemon does **not**
//! open a browser; the client opens the system browser on the user's
//! machine, captures the loopback code, and POSTs
//! `/cli/mail/link/oauth/complete`. When the client does not do that and
//! the daemon cannot open a system browser, `start` returns
//! `remote_unsupported` (teach, never hang). Microsoft (device flow) has
//! no such limit — it works local + remote without capture.
//!
//! No real network in this module's tests: the flow ENGINE ([`run_device_
//! flow`]/[`run_loopback_flow`]/[`serve_loopback`]) is injected with
//! O1's mock `HttpClient` + a real localhost loopback fed by a client
//! thread — no Google/Microsoft, no browser, ever.
//!
//! LOOPBACK HARDENING (M2): the 127.0.0.1 listener no longer trusts the
//! FIRST connection. It answers any callback whose `state` does not match
//! ours with a benign page and KEEPS listening, so a local attacker racing
//! `GET /cb?code=<their-code>&state=<stolen-state>` cannot abort or hijack
//! the link; only a state-valid callback proceeds, and PKCE (O1) then makes
//! even a state-valid injected code fail at the token exchange.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::cli_response::CliResponse;
use crate::mail::external::{self, ExtError};
use crate::mail::oauth::{self, FlowKind, HttpClient, OauthProvider, Poll, ReqwestHttp, Tokens};
use crate::mail::secrets::{FileSecretStore, SecretStore};

// ── Response helpers (the shared {code, hint} error contract) ───────────

fn error_response(status: &'static str, code: &str, hint: &str) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": { "code": code, "hint": hint },
        })
        .to_string(),
    }
}

fn ok_json(v: serde_json::Value) -> CliResponse {
    CliResponse::ok_json(v.to_string())
}

/// Resolve the caller's `workspace` to `(path, project_id)` — identity
/// from the resolved workspace, never trusted raw (mirrors
/// routes_external).
fn resolve_caller(project: &str) -> Result<(String, String), CliResponse> {
    let Some(path) = crate::workspace_msg::resolve_workspace(project) else {
        return Err(crate::workspace_routes::workspace_not_found_response(project));
    };
    let project_id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::workspace::agent_identity::resolve_project_id(&conn, &path)
    };
    match project_id {
        Some(id) => Ok((path, id)),
        None => Err(error_response(
            "404 Not Found",
            "not_found",
            &format!("workspace not registered: {path}"),
        )),
    }
}

fn ext_error_hint(e: ExtError) -> String {
    match e {
        ExtError::Usage(h) | ExtError::NotFound(h) | ExtError::Exists(h) | ExtError::Engine(h) => h,
    }
}

// ── The pending-link registry (start records; the server-side flow task
// updates; status reads). In-memory + short-lived — a code/token NEVER
// lives here, only the terminal state + the connected address/hint. ─────

/// The state a link progresses through (prd §7 `status` contract).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinkStatus {
    Pending,
    Connected,
    Denied,
    Expired,
    Error,
}

impl LinkStatus {
    fn as_str(self) -> &'static str {
        match self {
            LinkStatus::Pending => "pending",
            LinkStatus::Connected => "connected",
            LinkStatus::Denied => "denied",
            LinkStatus::Expired => "expired",
            LinkStatus::Error => "error",
        }
    }
}

/// One pending link's observable state. Carries NO secret — the address is
/// the account being linked (already known to the owner who started it).
#[derive(Debug, Clone)]
struct LinkEntry {
    status: LinkStatus,
    address: String,
    hint: Option<String>,
}

fn registry() -> &'static Mutex<HashMap<String, LinkEntry>> {
    static REG: OnceLock<Mutex<HashMap<String, LinkEntry>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

fn insert_pending(link_id: &str, address: &str) {
    registry()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(
            link_id.to_string(),
            LinkEntry { status: LinkStatus::Pending, address: address.to_string(), hint: None },
        );
}

// ── Client-capture secrets (remote Gmail) ───────────────────────────────
//
// When the K2 desktop app links Gmail against a remote daemon, the
// **client** binds 127.0.0.1 and captures Google's redirect; the daemon
// only holds PKCE verifier + state until `/complete` arrives. These
// secrets NEVER go on the status response.

struct ClientCapture {
    provider: OauthProvider,
    project_id: String,
    address: String,
    state: String,
    verifier: String,
    redirect_uri: String,
    expires_at: Instant,
}

fn capture_registry() -> &'static Mutex<HashMap<String, ClientCapture>> {
    static REG: OnceLock<Mutex<HashMap<String, ClientCapture>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Accept only loopback redirect URIs the desktop client may bind.
fn validate_client_redirect_uri(uri: &str) -> Result<(), String> {
    let uri = uri.trim();
    // http://127.0.0.1:<port>/cb  or  http://localhost:<port>/cb  (+ optional trailing path only /cb)
    let ok = (uri.starts_with("http://127.0.0.1:") || uri.starts_with("http://localhost:"))
        && uri.contains("/cb")
        && !uri.contains('@')
        && uri.len() < 128;
    if ok {
        Ok(())
    } else {
        Err(
            "redirectUri must be http://127.0.0.1:<port>/cb (client loopback only)"
                .to_string(),
        )
    }
}

fn set_terminal(link_id: &str, status: LinkStatus, hint: Option<String>) {
    let mut map = registry().lock().unwrap_or_else(|p| p.into_inner());
    if let Some(e) = map.get_mut(link_id) {
        e.status = status;
        if hint.is_some() {
            e.hint = hint;
        }
    }
}

/// On a successful token acquisition (either flow): mint the linked inbox
/// row + VAULT the tokens (`external::add_oauth_inbox`), then flip the link
/// to connected. A row/vault failure flips it to `error` with the reason
/// (never a token).
fn complete_with_tokens(
    secrets: &dyn SecretStore,
    provider: OauthProvider,
    link_id: &str,
    owner_project_id: &str,
    address: &str,
    tokens: &Tokens,
    now: i64,
) {
    match external::add_oauth_inbox(secrets, owner_project_id, provider, address, tokens, now) {
        Ok(_) => set_terminal(
            link_id,
            LinkStatus::Connected,
            Some(format!(
                "connected {address} — agents in the bound workspace can read it and save reply \
                 drafts. {}",
                if provider == OauthProvider::Gmail {
                    "Drafting is always on; raise it to 'send' with 'k2 mail access set-level' \
                     to let agents also send via SMTP (XOAUTH2)."
                } else {
                    "Drafting is always on; sending from Microsoft inboxes is not yet available \
                     (draft-only until Graph send lands)."
                }
            )),
        ),
        Err(e) => set_terminal(link_id, LinkStatus::Error, Some(ext_error_hint(e))),
    }
}

// ── Flow ENGINE (testable — O1 mock HttpClient, injected clock/sleep) ───

/// The server-side DEVICE-code poll loop (Microsoft, §3a): poll the token
/// endpoint every `interval` (honor `slow_down` with +5 s) until connected
/// / denied / expired, or the `device_code` lifetime elapses. On success it
/// creates the row + vaults tokens; every terminal outcome updates the
/// registry. The `device_code` is a PARAMETER, never returned or logged.
#[allow(clippy::too_many_arguments)]
fn run_device_flow(
    secrets: &dyn SecretStore,
    http: &dyn HttpClient,
    provider: OauthProvider,
    link_id: &str,
    owner_project_id: &str,
    address: &str,
    device_code: &str,
    interval_secs: i64,
    expires_in_secs: i64,
    now_fn: &mut dyn FnMut() -> i64,
    sleep_fn: &mut dyn FnMut(Duration),
) {
    let deadline = now_fn().saturating_add(expires_in_secs.max(0));
    let mut interval = interval_secs.max(1);
    loop {
        if now_fn() >= deadline {
            set_terminal(
                link_id,
                LinkStatus::Expired,
                Some("the device code expired before approval — run 'k2 mail link add' again".to_string()),
            );
            return;
        }
        match oauth::device_poll(provider, device_code, None, http) {
            Ok(Poll::Ok(tokens)) => {
                complete_with_tokens(secrets, provider, link_id, owner_project_id, address, &tokens, now_fn());
                return;
            }
            Ok(Poll::Pending) => {}
            Ok(Poll::SlowDown) => interval = interval.saturating_add(5),
            Ok(Poll::Denied) => {
                set_terminal(
                    link_id,
                    LinkStatus::Denied,
                    Some("access was denied in the browser".to_string()),
                );
                return;
            }
            Ok(Poll::Expired) => {
                set_terminal(
                    link_id,
                    LinkStatus::Expired,
                    Some("the device code expired before approval — run 'k2 mail link add' again".to_string()),
                );
                return;
            }
            Err(e) => {
                set_terminal(link_id, LinkStatus::Error, Some(format!("oauth error: {e}")));
                return;
            }
        }
        sleep_fn(Duration::from_secs(interval.max(1) as u64));
    }
}

/// What the loopback listener captured off the one redirect request.
struct LoopbackCapture {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// The server-side LOOPBACK flow (Gmail, §3b): await the state-VALID
/// redirect on the 127.0.0.1 listener (M2 — bad-state callbacks are ignored
/// and we keep listening; see [`serve_loopback`]), exchange the `code` for
/// tokens WITH the flow's PKCE `code_verifier` (M2), create the row + vault
/// them. Every terminal outcome updates the registry; the `code`/verifier/
/// tokens never leave the daemon.
#[allow(clippy::too_many_arguments)]
fn run_loopback_flow(
    secrets: &dyn SecretStore,
    http: &dyn HttpClient,
    provider: OauthProvider,
    link_id: &str,
    owner_project_id: &str,
    address: &str,
    listener: TcpListener,
    redirect_uri: &str,
    expected_state: &str,
    code_verifier: &str,
    timeout: Duration,
    now_fn: &mut dyn FnMut() -> i64,
) {
    // `serve_loopback` only returns a capture whose `state` matched ours —
    // it silently ignores (and keeps listening past) any wrong/missing-state
    // connection, so a local attacker's race can neither abort nor hijack
    // the link. A timeout means the real browser never returned in time.
    let capture = match serve_loopback(&listener, expected_state, timeout) {
        Ok(c) => c,
        Err(_) => {
            set_terminal(
                link_id,
                LinkStatus::Expired,
                Some("timed out waiting for the browser approval — run 'k2 mail link add' again".to_string()),
            );
            return;
        }
    };
    if let Some(err) = capture.error {
        // Google redirects `?error=access_denied` on a refusal (with our
        // `state`, so it reaches us here rather than being ignored).
        let status = if err == "access_denied" { LinkStatus::Denied } else { LinkStatus::Error };
        set_terminal(link_id, status, Some(format!("the browser returned '{err}'")));
        return;
    }
    let Some(code) = capture.code else {
        set_terminal(
            link_id,
            LinkStatus::Error,
            Some("the redirect carried no authorization code".to_string()),
        );
        return;
    };
    // `None` client_secret override → the provider's placeholder resolves
    // (Google's Desktop secret; Microsoft omits it). Same seam as
    // `client_id_override`, currently `None`. `code_verifier` completes the
    // PKCE proof — an injected code minted in another session fails here.
    match oauth::loopback_exchange(provider, &code, redirect_uri, code_verifier, None, None, http) {
        Ok(tokens) => {
            complete_with_tokens(secrets, provider, link_id, owner_project_id, address, &tokens, now_fn())
        }
        Err(e) => set_terminal(link_id, LinkStatus::Error, Some(format!("oauth error: {e}"))),
    }
}

/// Write the tiny fixed HTTP text response the loopback always serves (the
/// browser tab that hit the redirect just shows this line). Carries no
/// secret and no per-request data — identical bytes for good and bad
/// callbacks so it leaks nothing.
fn write_loopback_page(stream: &mut std::net::TcpStream, page: &str) {
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{}",
        page.len(),
        page
    );
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.flush();
}

/// Await the loopback redirect, parsing `?code`/`?state`/`?error` from each
/// connection. Bounded by `timeout` (non-blocking accept loop) so a browser
/// that never returns can't hang the flow thread.
///
/// M2 — KEEP LISTENING PAST BAD STATE. A connection whose `state` does not
/// match `expected_state` (a local attacker racing the callback with a
/// stolen `state`+port, or any stray probe) is answered with a benign page
/// and IGNORED; the loop keeps accepting until a state-VALID callback
/// arrives or the deadline passes. Only a state-valid capture is returned —
/// so exactly one, the genuine browser redirect, ever reaches the exchange.
/// (An earlier design trusted the FIRST connection, which let an attacker
/// abort the link or inject a code; PKCE then closes the residual window.)
fn serve_loopback(
    listener: &TcpListener,
    expected_state: &str,
    timeout: Duration,
) -> Result<LoopbackCapture, String> {
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let deadline = Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let mut buf = [0u8; 4096];
                let mut data: Vec<u8> = Vec::new();
                loop {
                    match stream.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            data.extend_from_slice(&buf[..n]);
                            // Stop at end-of-headers (the redirect is a bare
                            // GET) or a sane cap.
                            if data.windows(4).any(|w| w == b"\r\n\r\n") || data.len() > 16_384 {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                let req = String::from_utf8_lossy(&data);
                let capture = parse_loopback_request(&req);
                // M2 CSRF/injection guard: only a state-VALID callback is
                // honored. Anything else (wrong/missing state — an attacker's
                // race or a stray probe) gets a benign page and is ignored;
                // we keep listening for the real one until the deadline.
                if oauth::validate_state(expected_state, capture.state.as_deref().unwrap_or(""))
                    .is_err()
                {
                    write_loopback_page(
                        &mut stream,
                        "This authorization request could not be matched. You can close this tab.",
                    );
                    if Instant::now() >= deadline {
                        return Err("loopback timeout".to_string());
                    }
                    continue;
                }
                write_loopback_page(
                    &mut stream,
                    "You can close this tab and return to your terminal.",
                );
                return Ok(capture);
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err("loopback timeout".to_string());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// Percent-decode a query value (`%XX`). `+` is left verbatim (auth codes
/// are percent-encoded, never form `+`-space; leaving it avoids corrupting
/// a code that legitimately contains one).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Parse the loopback request's first line
/// (`GET /cb?code=…&state=… HTTP/1.1`) into a [`LoopbackCapture`].
fn parse_loopback_request(req: &str) -> LoopbackCapture {
    let mut cap = LoopbackCapture { code: None, state: None, error: None };
    let first = req.lines().next().unwrap_or("");
    let target = first.split_whitespace().nth(1).unwrap_or("");
    let query = target.split_once('?').map(|(_, q)| q).unwrap_or("");
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let val = percent_decode(v);
        let val = if val.is_empty() { None } else { Some(val) };
        match k {
            "code" => cap.code = val,
            "state" => cap.state = val,
            "error" => cap.error = val,
            _ => {}
        }
    }
    cap
}

// ── System-browser open (Gmail loopback; teach-not-hang when it can't) ──

/// Open `url` in the daemon machine's SYSTEM browser (never a webview —
/// Google rejects webviews, prd §2). A spawn failure (no opener / headless
/// / remote daemon) is the "remote_unsupported" signal the caller teaches.
///
/// Linux headless: `xdg-open` often **spawns successfully** with no
/// `DISPLAY` and does nothing visible — that used to return "browser
/// opened" while the user stared at an empty Settings card. Refuse when
/// there is no usable display so the desktop client must use clientCapture.
fn open_in_system_browser(url: &str) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let display = std::env::var("DISPLAY").unwrap_or_default();
        let wayland = std::env::var("WAYLAND_DISPLAY").unwrap_or_default();
        if display.trim().is_empty() && wayland.trim().is_empty() {
            return Err(
                "no DISPLAY/WAYLAND_DISPLAY — this host is headless; use the K2 desktop \
                 app's client-capture Gmail flow (in-app browser)"
                    .into(),
            );
        }
    }
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(not(target_os = "macos"))]
    let opener = "xdg-open";
    std::process::Command::new(opener)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("{opener}: {e}"))
}

// ── POST /cli/mail/link/oauth/start (owner) ─────────────────────────────

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct StartBody {
    address: String,
    provider: String,
    workspace: String,
    /// When true (desktop client on a **remote** daemon), the client binds
    /// loopback + opens the browser; the daemon only mints PKCE/state and
    /// waits for `POST …/complete`. Requires `redirect_uri`.
    client_capture: bool,
    /// Client-local `http://127.0.0.1:<port>/cb` (required with client_capture).
    redirect_uri: String,
}

/// POST `/cli/mail/link/oauth/start` — begin an OAuth link. Microsoft →
/// device-code (returns `userCode`+`verificationUrl`); Gmail → either
/// server-side loopback + system browser (local daemon) or **client
/// capture** (`clientCapture` + `redirectUri`: return `authorizationUrl`,
/// no browser on the daemon). A SERVER-SIDE task (or `/complete`) then
/// exchanges and vaults tokens. Codes/tokens are NEVER in the response.
pub fn handle_link_oauth_start(body: &[u8]) -> CliResponse {
    let b: StartBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => {
            return error_response("400 Bad Request", "usage", &format!("invalid JSON body: {e}"))
        }
    };
    if b.address.trim().is_empty() {
        return error_response(
            "400 Bad Request",
            "usage",
            "missing 'address' — the account to link, e.g. you@gmail.com",
        );
    }
    if b.provider.trim().is_empty() {
        return error_response(
            "400 Bad Request",
            "usage",
            "missing 'provider' — 'gmail' or 'microsoft'",
        );
    }
    if b.workspace.trim().is_empty() {
        return error_response(
            "400 Bad Request",
            "usage",
            "missing 'workspace' — the ONE workspace this inbox binds to",
        );
    }
    let provider = match OauthProvider::from_str(b.provider.trim()) {
        Some(p) => p,
        None => {
            return error_response(
                "400 Bad Request",
                "usage",
                &format!(
                    "unknown provider '{}' — 'gmail' (IMAP-XOAUTH2) or 'microsoft' (Graph)",
                    b.provider.trim()
                ),
            )
        }
    };
    let (_path, project_id) = match resolve_caller(&b.workspace) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let address = b.address.trim().to_string();
    let link_id = uuid::Uuid::new_v4().to_string();
    match provider.config().default_flow {
        FlowKind::DeviceCode => {
            if b.client_capture {
                return error_response(
                    "400 Bad Request",
                    "usage",
                    "clientCapture is only for Gmail loopback — Microsoft uses device flow",
                );
            }
            start_device(provider, &link_id, &project_id, &address)
        }
        FlowKind::Loopback if b.client_capture => {
            start_loopback_client_capture(
                provider,
                &link_id,
                &project_id,
                &address,
                b.redirect_uri.trim(),
            )
        }
        FlowKind::Loopback => start_loopback(provider, &link_id, &project_id, &address),
    }
}

/// Microsoft device-code entry: `device_start`, record the pending link,
/// spawn the poll task, return the user-facing code+URL (never the
/// `device_code`).
fn start_device(
    provider: OauthProvider,
    link_id: &str,
    project_id: &str,
    address: &str,
) -> CliResponse {
    let http = ReqwestHttp::default();
    let device = match oauth::device_start(provider, None, &http) {
        Ok(d) => d,
        Err(e) => {
            return error_response(
                "502 Bad Gateway",
                "engine",
                &format!("could not begin device authorization: {e}"),
            )
        }
    };
    insert_pending(link_id, address);
    let (lid, pid, addr, dc, interval, expires) = (
        link_id.to_string(),
        project_id.to_string(),
        address.to_string(),
        device.device_code.clone(),
        device.interval,
        device.expires_in,
    );
    std::thread::spawn(move || {
        let secrets = FileSecretStore::default();
        let http = ReqwestHttp::default();
        let mut now_fn = || chrono::Utc::now().timestamp();
        let mut sleep_fn = |d: Duration| std::thread::sleep(d);
        run_device_flow(
            &secrets, &http, provider, &lid, &pid, &addr, &dc, interval, expires, &mut now_fn,
            &mut sleep_fn,
        );
    });
    ok_json(serde_json::json!({
        "ok": true,
        "flow": "device",
        "linkId": link_id,
        "userCode": device.user_code,
        "verificationUrl": device.verification_uri,
        "expiresIn": device.expires_in,
        "hint": format!("go to {} and enter {}", device.verification_uri, device.user_code),
    }))
}

/// Gmail loopback entry: bind the 127.0.0.1 listener, build the consent
/// URL, OPEN the system browser (teach `remote_unsupported` if it can't),
/// record the pending link, spawn the await-and-exchange task, return the
/// "browser opened" acknowledgement.
fn start_loopback(
    provider: OauthProvider,
    link_id: &str,
    project_id: &str,
    address: &str,
) -> CliResponse {
    let (listener, addr) = match oauth::bind_ephemeral_loopback() {
        Ok(v) => v,
        Err(e) => {
            return error_response(
                "502 Bad Gateway",
                "engine",
                &format!("could not bind a loopback listener: {e}"),
            )
        }
    };
    let redirect_uri = format!("http://{addr}/cb");
    let state = uuid::Uuid::new_v4().to_string();
    // PKCE (M2): a per-flow verifier/challenge pair. The challenge rides on
    // the (argv-visible) consent URL; the verifier stays in this flow's
    // in-memory state and is only revealed on the token exchange — so a
    // local attacker who scrapes `state`/`client_id` off the browser-launch
    // argv still cannot inject a code (they lack the verifier). It is
    // token-grade: never logged, redacted in `PkcePair`'s Debug.
    let pkce = match oauth::generate_pkce() {
        Ok(p) => p,
        Err(e) => {
            return error_response(
                "502 Bad Gateway",
                "engine",
                &format!("could not initialize the secure link: {e}"),
            )
        }
    };
    let url = match oauth::loopback_build_url(provider, None, &redirect_uri, &state, &pkce.challenge)
    {
        Ok(u) => u,
        Err(e) => {
            return error_response(
                "502 Bad Gateway",
                "engine",
                &format!("could not build the consent URL: {e}"),
            )
        }
    };
    if let Err(e) = open_in_system_browser(&url) {
        // Teach, never hang (prd §3b/§11.8): the listener drops here.
        return error_response(
            "409 Conflict",
            "remote_unsupported",
            &format!(
                "link Gmail on the machine running this K2 daemon — its system browser handles \
                 Google's consent (a webview is refused, and a remote daemon can't reach your \
                 browser). The daemon could not open a browser: {e}"
            ),
        );
    }
    insert_pending(link_id, address);
    // The PKCE verifier travels into the flow thread alongside `state` and
    // is consumed only by the token exchange — it never touches the
    // registry, a log line, or the response.
    let (lid, pid, addr2, ruri, st, verifier) = (
        link_id.to_string(),
        project_id.to_string(),
        address.to_string(),
        redirect_uri.clone(),
        state.clone(),
        pkce.verifier.clone(),
    );
    std::thread::spawn(move || {
        let secrets = FileSecretStore::default();
        let http = ReqwestHttp::default();
        let mut now_fn = || chrono::Utc::now().timestamp();
        run_loopback_flow(
            &secrets,
            &http,
            provider,
            &lid,
            &pid,
            &addr2,
            listener,
            &ruri,
            &st,
            &verifier,
            Duration::from_secs(300),
            &mut now_fn,
        );
    });
    ok_json(serde_json::json!({
        "ok": true,
        "flow": "loopback",
        "linkId": link_id,
        "hint": "a browser window opened — approve access there; this terminal is waiting",
    }))
}

/// Gmail remote path: client binds 127.0.0.1 and will open the browser.
/// Daemon holds PKCE verifier + state until `POST …/complete`. Never opens
/// a browser on the daemon host.
fn start_loopback_client_capture(
    provider: OauthProvider,
    link_id: &str,
    project_id: &str,
    address: &str,
    redirect_uri: &str,
) -> CliResponse {
    if redirect_uri.is_empty() {
        return error_response(
            "400 Bad Request",
            "usage",
            "clientCapture requires redirectUri (http://127.0.0.1:<port>/cb)",
        );
    }
    if let Err(h) = validate_client_redirect_uri(redirect_uri) {
        return error_response("400 Bad Request", "usage", &h);
    }
    let state = uuid::Uuid::new_v4().to_string();
    let pkce = match oauth::generate_pkce() {
        Ok(p) => p,
        Err(e) => {
            return error_response(
                "502 Bad Gateway",
                "engine",
                &format!("could not initialize the secure link: {e}"),
            )
        }
    };
    let url = match oauth::loopback_build_url(provider, None, redirect_uri, &state, &pkce.challenge)
    {
        Ok(u) => u,
        Err(e) => {
            return error_response(
                "502 Bad Gateway",
                "engine",
                &format!("could not build the consent URL: {e}"),
            )
        }
    };
    insert_pending(link_id, address);
    {
        let mut map = capture_registry().lock().unwrap_or_else(|p| p.into_inner());
        // Drop expired captures so a long-lived daemon does not accumulate.
        let now = Instant::now();
        map.retain(|_, c| c.expires_at > now);
        map.insert(
            link_id.to_string(),
            ClientCapture {
                provider,
                project_id: project_id.to_string(),
                address: address.to_string(),
                state: state.clone(),
                verifier: pkce.verifier.clone(),
                redirect_uri: redirect_uri.to_string(),
                expires_at: Instant::now() + Duration::from_secs(300),
            },
        );
    }
    // authorizationUrl is public (state + challenge only). Verifier stays here.
    ok_json(serde_json::json!({
        "ok": true,
        "flow": "loopback",
        "linkId": link_id,
        "authorizationUrl": url,
        "state": state,
        "clientCapture": true,
        "hint": "open the authorization URL in a browser on this machine; the app captures the redirect",
    }))
}

// ── POST /cli/mail/link/oauth/complete (owner; client-capture only) ─────

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct CompleteBody {
    link_id: String,
    /// Authorization code from Google's redirect (empty if `error` set).
    code: String,
    /// Must match the `state` issued on start (CSRF).
    state: String,
    /// Provider error from the redirect (`access_denied`, etc.).
    error: String,
}

/// POST `/cli/mail/link/oauth/complete` — desktop client finished the
/// loopback capture. Body may carry `code`+`state` or `error`+`state`.
/// Daemon validates state against the capture registry, exchanges (PKCE),
/// vaults tokens. The code is single-use and never returned on status.
pub fn handle_link_oauth_complete(body: &[u8]) -> CliResponse {
    let b: CompleteBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => {
            return error_response("400 Bad Request", "usage", &format!("invalid JSON body: {e}"))
        }
    };
    let link_id = b.link_id.trim();
    if link_id.is_empty() {
        return error_response(
            "400 Bad Request",
            "usage",
            "missing 'linkId' — the id returned by oauth/start",
        );
    }
    // Validate state BEFORE consuming the capture so a wrong-state probe
    // cannot burn a live linkId (owner still holds the real complete).
    let cap = {
        let mut map = capture_registry().lock().unwrap_or_else(|p| p.into_inner());
        let Some(existing) = map.get(link_id) else {
            return error_response(
                "404 Not Found",
                "not_found",
                "no client-capture link for this linkId — it may have expired; start again",
            );
        };
        if Instant::now() > existing.expires_at {
            map.remove(link_id);
            set_terminal(
                link_id,
                LinkStatus::Expired,
                Some(
                    "the link expired before the browser returned — try Connect Gmail again"
                        .into(),
                ),
            );
            return error_response(
                "410 Gone",
                "expired",
                "the link expired before the browser returned — try Connect Gmail again",
            );
        }
        if oauth::validate_state(&existing.state, b.state.trim()).is_err() {
            // Leave the capture in place for the real client complete.
            return error_response(
                "400 Bad Request",
                "usage",
                "state mismatch — start the Gmail link again",
            );
        }
        // State matched — consume once (no second exchange / replay).
        map.remove(link_id).expect("capture present after get")
    };
    if !b.error.trim().is_empty() {
        let err = b.error.trim();
        let status = if err == "access_denied" {
            LinkStatus::Denied
        } else {
            LinkStatus::Error
        };
        set_terminal(
            link_id,
            status,
            Some(format!("the browser returned '{err}'")),
        );
        return ok_json(serde_json::json!({
            "ok": true,
            "state": status.as_str(),
            "linkId": link_id,
        }));
    }
    let code = b.code.trim();
    if code.is_empty() {
        set_terminal(
            link_id,
            LinkStatus::Error,
            Some("the redirect carried no authorization code".into()),
        );
        return error_response(
            "400 Bad Request",
            "usage",
            "missing 'code' (or set 'error' if the user denied access)",
        );
    }
    // Exchange on a worker path (caller is already spawn_blocking in the
    // mail POST arm). Verifier never leaves this process.
    let secrets = FileSecretStore::default();
    let http = ReqwestHttp::default();
    let now = chrono::Utc::now().timestamp();
    match oauth::loopback_exchange(
        cap.provider,
        code,
        &cap.redirect_uri,
        &cap.verifier,
        None,
        None,
        &http,
    ) {
        Ok(tokens) => {
            complete_with_tokens(
                &secrets,
                cap.provider,
                link_id,
                &cap.project_id,
                &cap.address,
                &tokens,
                now,
            );
            let map = registry().lock().unwrap_or_else(|p| p.into_inner());
            let state = map
                .get(link_id)
                .map(|e| e.status.as_str())
                .unwrap_or("connected");
            ok_json(serde_json::json!({
                "ok": true,
                "state": state,
                "linkId": link_id,
                "address": cap.address,
            }))
        }
        Err(e) => {
            set_terminal(link_id, LinkStatus::Error, Some(format!("oauth error: {e}")));
            error_response(
                "502 Bad Gateway",
                "engine",
                &format!("token exchange failed: {e}"),
            )
        }
    }
}

// ── GET /cli/mail/link/oauth/status (owner) ─────────────────────────────

/// GET `/cli/mail/link/oauth/status?linkId=…` — the long-poll target:
/// `{state: pending|connected|denied|expired|error, address?, hint?}`.
/// Never a token. An unknown/expired-from-memory linkId is a masked
/// not_found.
pub fn handle_link_oauth_status(params: &HashMap<String, String>) -> CliResponse {
    let Some(link_id) = params.get("linkId").map(String::as_str).filter(|s| !s.is_empty()) else {
        return error_response(
            "400 Bad Request",
            "usage",
            "missing 'linkId' — the id returned by 'k2 mail link add --provider …'",
        );
    };
    let map = registry().lock().unwrap_or_else(|p| p.into_inner());
    match map.get(link_id) {
        Some(e) => {
            let mut body = serde_json::json!({
                "ok": true,
                "state": e.status.as_str(),
                "address": e.address,
            });
            if let Some(h) = &e.hint {
                body["hint"] = serde_json::json!(h);
            }
            ok_json(body)
        }
        None => error_response(
            "404 Not Found",
            "not_found",
            "no such link — it may have completed and expired from memory; \
             run 'k2 mail link add --provider …' again",
        ),
    }
}

// ──────────────────────────────────────────────────────────────────────
// Inline unit tests — O1 mock HttpClient + an in-memory vault + a real
// localhost loopback fed by a client thread. NO Google/Microsoft, NO
// browser, NO tokens on the wire (house rules / prd §11.16).
// ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::net::TcpStream;
    use std::sync::Mutex as StdMutex;

    // ── Fakes (mirror the O1 engine tests) ──

    /// Scripted HTTP mock: FIFO of canned `(status, body)`; an empty queue
    /// PANICS so a test that must not hit the network fails loudly. Records
    /// every request's form so tests can assert what went on the wire (e.g.
    /// the PKCE `code_verifier`).
    struct MockHttp {
        queue: StdMutex<VecDeque<(u16, String)>>,
        seen: StdMutex<Vec<Vec<(String, String)>>>,
    }
    impl MockHttp {
        fn new(replies: &[(u16, &str)]) -> Self {
            Self {
                queue: StdMutex::new(replies.iter().map(|(s, b)| (*s, b.to_string())).collect()),
                seen: StdMutex::new(Vec::new()),
            }
        }
        /// Value of a form field in the Nth recorded request.
        fn field(&self, n: usize, key: &str) -> Option<String> {
            self.seen.lock().unwrap().get(n).and_then(|form| {
                form.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
            })
        }
    }
    impl HttpClient for MockHttp {
        fn post_form(
            &self,
            _url: &str,
            form: &[(&str, &str)],
        ) -> Result<oauth::HttpResponse, oauth::OauthError> {
            self.seen
                .lock()
                .unwrap()
                .push(form.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect());
            let (status, body) = self
                .queue
                .lock()
                .unwrap()
                .pop_front()
                .expect("MockHttp: unexpected HTTP call (queue empty)");
            Ok(oauth::HttpResponse { status, body })
        }
    }

    #[derive(Default)]
    struct MemStore {
        map: StdMutex<HashMap<String, String>>,
    }
    impl SecretStore for MemStore {
        fn store(&self, _kind: &str, _secret: &str) -> Result<String, String> {
            Err("unused".into())
        }
        fn resolve(&self, k: &str) -> Result<Option<String>, String> {
            Ok(self.map.lock().unwrap().get(k).cloned())
        }
        fn delete(&self, k: &str) -> Result<(), String> {
            self.map.lock().unwrap().remove(k);
            Ok(())
        }
        fn store_exact(&self, k: &str, v: &str) -> Result<(), String> {
            self.map.lock().unwrap().insert(k.to_string(), v.to_string());
            Ok(())
        }
    }

    fn body_json(resp: &CliResponse) -> serde_json::Value {
        serde_json::from_str(&resp.body).expect("valid JSON body")
    }

    fn insert_project() -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, format!("loauth-{}", &id[..8]), format!("/tmp/loauth-{}", &id[..8])],
        )
        .expect("insert project");
        id
    }

    fn cleanup(project_id: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = conn.execute(
            "DELETE FROM mail_external_inboxes WHERE owner_project_id = ?1",
            rusqlite::params![project_id],
        );
        let _ = conn.execute("DELETE FROM projects WHERE id = ?1", rusqlite::params![project_id]);
    }

    /// Read one row's oauth columns straight from the DB.
    fn row_oauth(address: &str) -> Option<(String, String, Option<String>, Option<i64>)> {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT kind, auth_kind, provider, token_expires_at FROM mail_external_inboxes \
             WHERE email_address = ?1",
            rusqlite::params![address],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .ok()
    }

    // ── start/status route validation (no network) ──

    #[test]
    fn start_validates_before_any_flow() {
        assert_eq!(handle_link_oauth_start(b"not json").status, "400 Bad Request");
        for (b, needle) in [
            (r#"{}"#, "address"),
            (r#"{"address":"a@b.example"}"#, "provider"),
            (r#"{"address":"a@b.example","provider":"gmail"}"#, "workspace"),
        ] {
            let resp = handle_link_oauth_start(b.as_bytes());
            assert_eq!(resp.status, "400 Bad Request", "{b}");
            assert!(
                body_json(&resp)["error"]["hint"].as_str().unwrap().contains(needle),
                "{b}: {}",
                resp.body
            );
        }
        // Unknown provider teaches gmail|microsoft.
        let resp = handle_link_oauth_start(
            br#"{"address":"a@b.example","provider":"yahoo","workspace":"x"}"#,
        );
        assert_eq!(resp.status, "400 Bad Request");
        assert!(body_json(&resp)["error"]["hint"].as_str().unwrap().contains("gmail"));
        // Unknown workspace → shared 404 (a valid provider gets past the
        // provider gate, then fails workspace resolution BEFORE any dial).
        let resp = handle_link_oauth_start(
            br#"{"address":"a@b.example","provider":"microsoft","workspace":"no-such-ws-xyz"}"#,
        );
        assert_eq!(resp.status, "404 Not Found", "{}", resp.body);
    }

    #[test]
    fn validate_client_redirect_uri_accepts_loopback_only() {
        assert!(validate_client_redirect_uri("http://127.0.0.1:54321/cb").is_ok());
        assert!(validate_client_redirect_uri("http://localhost:9/cb").is_ok());
        assert!(validate_client_redirect_uri("https://127.0.0.1:1/cb").is_err());
        assert!(validate_client_redirect_uri("http://evil.com/cb").is_err());
        assert!(validate_client_redirect_uri("http://127.0.0.1:1/other").is_err());
        assert!(validate_client_redirect_uri("http://user@127.0.0.1:1/cb").is_err());
    }

    #[test]
    fn client_capture_requires_redirect_and_rejects_microsoft() {
        let pid = insert_project();
        let ws = format!("/tmp/loauth-{}", &pid[..8]);
        // Microsoft + clientCapture → usage
        let body = serde_json::json!({
            "address": "a@b.example",
            "provider": "microsoft",
            "workspace": ws,
            "clientCapture": true,
            "redirectUri": "http://127.0.0.1:9/cb",
        });
        let resp = handle_link_oauth_start(body.to_string().as_bytes());
        assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);
        assert!(
            body_json(&resp)["error"]["hint"]
                .as_str()
                .unwrap()
                .contains("clientCapture"),
            "{}",
            resp.body
        );
        // Gmail clientCapture without redirectUri
        let body = serde_json::json!({
            "address": "a@b.example",
            "provider": "gmail",
            "workspace": ws,
            "clientCapture": true,
        });
        let resp = handle_link_oauth_start(body.to_string().as_bytes());
        assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);
        assert!(
            body_json(&resp)["error"]["hint"]
                .as_str()
                .unwrap()
                .to_lowercase()
                .contains("redirect"),
            "{}",
            resp.body
        );
        cleanup(&pid);
    }

    #[test]
    fn client_capture_start_returns_authorization_url_without_browser() {
        let pid = insert_project();
        let ws = format!("/tmp/loauth-{}", &pid[..8]);
        let body = serde_json::json!({
            "address": "you@gmail.com",
            "provider": "gmail",
            "workspace": ws,
            "clientCapture": true,
            "redirectUri": "http://127.0.0.1:54321/cb",
        });
        let resp = handle_link_oauth_start(body.to_string().as_bytes());
        // If baked Gmail client is REPLACE_ME, loopback_build_url may still
        // succeed with a placeholder client id — we only require the shape.
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let j = body_json(&resp);
        assert_eq!(j["flow"], "loopback");
        assert_eq!(j["clientCapture"], true);
        assert!(j["linkId"].as_str().unwrap().len() > 8);
        assert!(j["authorizationUrl"].as_str().unwrap().starts_with("https://"));
        assert!(j["state"].as_str().unwrap().len() > 8);
        let lid = j["linkId"].as_str().unwrap().to_string();
        let st = j["state"].as_str().unwrap().to_string();
        // Wrong state → usage, capture still live
        let bad = serde_json::json!({
            "linkId": lid,
            "code": "fake",
            "state": "not-the-state",
        });
        let c = handle_link_oauth_complete(bad.to_string().as_bytes());
        assert_eq!(c.status, "400 Bad Request", "{}", c.body);
        // Real complete with access_denied terminates cleanly
        let denied = serde_json::json!({
            "linkId": lid,
            "state": st,
            "error": "access_denied",
        });
        let d = handle_link_oauth_complete(denied.to_string().as_bytes());
        assert_eq!(d.status, "200 OK", "{}", d.body);
        assert_eq!(body_json(&d)["state"], "denied");
        let params: HashMap<String, String> =
            HashMap::from([("linkId".to_string(), lid)]);
        assert_eq!(body_json(&handle_link_oauth_status(&params))["state"], "denied");
        cleanup(&pid);
    }

    #[test]
    fn complete_unknown_link_is_not_found() {
        let resp = handle_link_oauth_complete(
            br#"{"linkId":"ghost","code":"x","state":"y"}"#,
        );
        assert_eq!(resp.status, "404 Not Found");
        assert_eq!(body_json(&resp)["error"]["code"], "not_found");
    }

    #[test]
    fn status_requires_linkid_and_masks_unknown() {
        let resp = handle_link_oauth_status(&HashMap::new());
        assert_eq!(resp.status, "400 Bad Request");
        assert_eq!(body_json(&resp)["error"]["code"], "usage");
        let params: HashMap<String, String> =
            HashMap::from([("linkId".to_string(), "ghost-xyz".to_string())]);
        let resp = handle_link_oauth_status(&params);
        assert_eq!(resp.status, "404 Not Found");
        assert_eq!(body_json(&resp)["error"]["code"], "not_found");
    }

    // ── device flow: pending → success creates a graph row + vaults ──

    #[test]
    fn device_flow_creates_graph_row_and_vaults_tokens() {
        let project = insert_project();
        let address = format!("dev-{}@company.com", &project[..8]);
        let link_id = uuid::Uuid::new_v4().to_string();
        insert_pending(&link_id, &address);

        // authorization_pending (400) → then a success token body.
        let http = MockHttp::new(&[
            (400, r#"{"error":"authorization_pending"}"#),
            (
                200,
                r#"{"access_token":"AT-secret","refresh_token":"RT-secret","token_type":"Bearer","expires_in":3600,"scope":"https://graph.microsoft.com/Mail.ReadWrite"}"#,
            ),
        ]);
        let store = MemStore::default();
        let mut clock = 1_000i64;
        let mut now_fn = || {
            clock += 1;
            clock
        };
        let mut sleep_fn = |_d: Duration| {};
        run_device_flow(
            &store,
            &http,
            OauthProvider::Microsoft,
            &link_id,
            &project,
            &address,
            "DEVICE-CODE-secret",
            5,
            900,
            &mut now_fn,
            &mut sleep_fn,
        );

        // Registry flipped to connected.
        let params: HashMap<String, String> =
            HashMap::from([("linkId".to_string(), link_id.clone())]);
        let resp = handle_link_oauth_status(&params);
        assert_eq!(body_json(&resp)["state"], "connected", "{}", resp.body);

        // Row: kind=graph, auth_kind=oauth, provider=microsoft, expiry set.
        let (kind, auth_kind, provider, expiry) = row_oauth(&address).expect("row created");
        assert_eq!(kind, "graph");
        assert_eq!(auth_kind, "oauth");
        assert_eq!(provider.as_deref(), Some("microsoft"));
        assert!(expiry.unwrap() > 1_000, "token_expires_at set from now+expires_in");

        // Tokens are VAULTED under the oauth key — never a column/response.
        let id: String = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.query_row(
                "SELECT id FROM mail_external_inboxes WHERE email_address = ?1",
                rusqlite::params![address],
                |r| r.get(0),
            )
            .unwrap()
        };
        let vaulted = store.resolve(&oauth::oauth_vault_key(&id)).unwrap().expect("vaulted");
        assert!(vaulted.contains("AT-secret") && vaulted.contains("RT-secret"));
        // The success response body carried NO token.
        assert!(!resp.body.contains("AT-secret") && !resp.body.contains("RT-secret"));

        cleanup(&project);
    }

    #[test]
    fn device_flow_denied_sets_denied_and_creates_no_row() {
        let project = insert_project();
        let address = format!("den-{}@company.com", &project[..8]);
        let link_id = uuid::Uuid::new_v4().to_string();
        insert_pending(&link_id, &address);
        let http = MockHttp::new(&[(400, r#"{"error":"access_denied"}"#)]);
        let store = MemStore::default();
        let mut now_fn = || 1_000i64;
        let mut sleep_fn = |_d: Duration| {};
        run_device_flow(
            &store,
            &http,
            OauthProvider::Microsoft,
            &link_id,
            &project,
            &address,
            "DC",
            5,
            900,
            &mut now_fn,
            &mut sleep_fn,
        );
        let params: HashMap<String, String> =
            HashMap::from([("linkId".to_string(), link_id)]);
        assert_eq!(body_json(&handle_link_oauth_status(&params))["state"], "denied");
        assert!(row_oauth(&address).is_none(), "no row on denial");
        cleanup(&project);
    }

    // ── loopback flow: a client feeds ?code — row is an imap gmail row ──

    #[test]
    fn loopback_flow_captures_code_and_creates_gmail_imap_row() {
        let project = insert_project();
        let address = format!("lp-{}@gmail.com", &project[..8]);
        let link_id = uuid::Uuid::new_v4().to_string();
        insert_pending(&link_id, &address);

        let (listener, addr) = oauth::bind_ephemeral_loopback().expect("bind");
        let redirect_uri = format!("http://{addr}/cb");
        let state = "STATE-nonce".to_string();

        // A client thread plays the browser redirect: GET /cb?code=…&state=…
        let st = state.clone();
        let client = std::thread::spawn(move || {
            // Give the server loop a moment to arm its accept.
            std::thread::sleep(Duration::from_millis(50));
            let mut s = TcpStream::connect(addr).expect("connect loopback");
            let req = format!(
                "GET /cb?code=AUTH%2FCODE-secret&state={st} HTTP/1.1\r\nHost: localhost\r\n\r\n"
            );
            s.write_all(req.as_bytes()).unwrap();
            let mut resp = String::new();
            let _ = s.read_to_string(&mut resp);
            resp
        });

        // Token exchange reply (loopback_exchange POSTs the token endpoint).
        let http = MockHttp::new(&[(
            200,
            r#"{"access_token":"GAT-secret","refresh_token":"GRT-secret","token_type":"Bearer","expires_in":3599,"scope":"https://mail.google.com/"}"#,
        )]);
        let store = MemStore::default();
        let mut now_fn = || 2_000i64;
        run_loopback_flow(
            &store,
            &http,
            OauthProvider::Gmail,
            &link_id,
            &project,
            &address,
            listener,
            &redirect_uri,
            &state,
            "PKCE-VERIFIER-abc",
            Duration::from_secs(5),
            &mut now_fn,
        );
        let served = client.join().unwrap();
        assert!(served.contains("close this tab"), "close-tab page served: {served}");

        // PKCE (M2): the flow's verifier round-tripped into the token POST.
        assert_eq!(http.field(0, "code_verifier").as_deref(), Some("PKCE-VERIFIER-abc"));

        let params: HashMap<String, String> =
            HashMap::from([("linkId".to_string(), link_id)]);
        assert_eq!(body_json(&handle_link_oauth_status(&params))["state"], "connected");
        let (kind, auth_kind, provider, _e) = row_oauth(&address).expect("row");
        assert_eq!(kind, "imap");
        assert_eq!(auth_kind, "oauth");
        assert_eq!(provider.as_deref(), Some("gmail"));
        cleanup(&project);
    }

    // ── M2: a bad-state connection is IGNORED; the listener keeps listening
    // and the flow still completes on a subsequent good-state callback. ──

    #[test]
    fn loopback_bad_state_ignored_then_good_state_completes() {
        let project = insert_project();
        let address = format!("m2-{}@gmail.com", &project[..8]);
        let link_id = uuid::Uuid::new_v4().to_string();
        insert_pending(&link_id, &address);

        let (listener, addr) = oauth::bind_ephemeral_loopback().expect("bind");
        let redirect_uri = format!("http://{addr}/cb");
        let good_state = "GOOD-state".to_string();

        // The attacker connects FIRST with a stolen-but-wrong state + their
        // own code; then the real browser connects with the correct state.
        let gs = good_state.clone();
        let client = std::thread::spawn(move || {
            // 1) Attacker race: wrong state → must be ignored, NOT exchanged.
            std::thread::sleep(Duration::from_millis(50));
            let mut atk = TcpStream::connect(addr).expect("attacker connect");
            atk.write_all(
                b"GET /cb?code=ATTACKER-code&state=WRONG HTTP/1.1\r\nHost: localhost\r\n\r\n",
            )
            .unwrap();
            let mut atk_resp = String::new();
            let _ = atk.read_to_string(&mut atk_resp);

            // 2) Real browser: correct state + the genuine code.
            std::thread::sleep(Duration::from_millis(50));
            let mut real = TcpStream::connect(addr).expect("browser connect");
            let req = format!(
                "GET /cb?code=REAL-code&state={gs} HTTP/1.1\r\nHost: localhost\r\n\r\n"
            );
            real.write_all(req.as_bytes()).unwrap();
            let mut real_resp = String::new();
            let _ = real.read_to_string(&mut real_resp);
            (atk_resp, real_resp)
        });

        // Exactly ONE token-exchange reply is queued. If the attacker's
        // bad-state connection had reached the exchange, the genuine one
        // would panic on the empty queue — so a single reply PROVES only the
        // state-valid callback exchanged.
        let http = MockHttp::new(&[(
            200,
            r#"{"access_token":"GAT","refresh_token":"GRT","token_type":"Bearer","expires_in":3599,"scope":"https://mail.google.com/"}"#,
        )]);
        let store = MemStore::default();
        let mut now_fn = || 4_000i64;
        run_loopback_flow(
            &store,
            &http,
            OauthProvider::Gmail,
            &link_id,
            &project,
            &address,
            listener,
            &redirect_uri,
            &good_state,
            "PKCE-VERIFIER",
            Duration::from_secs(5),
            &mut now_fn,
        );
        let (atk_resp, real_resp) = client.join().unwrap();
        // Both connections got a benign page; only the good one closes-tab.
        assert!(atk_resp.contains("could not be matched"), "attacker page: {atk_resp}");
        assert!(real_resp.contains("close this tab"), "browser page: {real_resp}");

        // The exchange ran exactly once, for the REAL code (never the
        // attacker's), and it carried the flow's PKCE verifier.
        assert_eq!(http.field(0, "code").as_deref(), Some("REAL-code"));
        assert!(http.field(1, "code").is_none(), "only one exchange must run");
        assert_eq!(http.field(0, "code_verifier").as_deref(), Some("PKCE-VERIFIER"));

        let params: HashMap<String, String> =
            HashMap::from([("linkId".to_string(), link_id)]);
        assert_eq!(body_json(&handle_link_oauth_status(&params))["state"], "connected");
        assert!(row_oauth(&address).is_some(), "row created from the genuine callback");
        cleanup(&project);
    }

    #[test]
    fn loopback_wrong_state_only_times_out_no_row() {
        // A lone wrong-state connection is ignored; with no good callback the
        // flow times out (expired) and creates NO row — the attacker cannot
        // even force an early terminal state. Short timeout keeps it fast.
        let project = insert_project();
        let address = format!("m2to-{}@gmail.com", &project[..8]);
        let link_id = uuid::Uuid::new_v4().to_string();
        insert_pending(&link_id, &address);
        let (listener, addr) = oauth::bind_ephemeral_loopback().expect("bind");
        let redirect_uri = format!("http://{addr}/cb");
        let client = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            let mut s = TcpStream::connect(addr).unwrap();
            s.write_all(b"GET /cb?code=X&state=WRONG HTTP/1.1\r\n\r\n").unwrap();
            let mut r = String::new();
            let _ = s.read_to_string(&mut r);
            r
        });
        // Empty HTTP queue → a token exchange would PANIC (proves none ran).
        let http = MockHttp::new(&[]);
        let store = MemStore::default();
        let mut now_fn = || 3_000i64;
        run_loopback_flow(
            &store,
            &http,
            OauthProvider::Gmail,
            &link_id,
            &project,
            &address,
            listener,
            &redirect_uri,
            "EXPECTED-state",
            "PKCE-VERIFIER",
            Duration::from_millis(400),
            &mut now_fn,
        );
        let atk_resp = client.join().unwrap();
        assert!(atk_resp.contains("could not be matched"), "attacker page: {atk_resp}");
        let params: HashMap<String, String> =
            HashMap::from([("linkId".to_string(), link_id)]);
        assert_eq!(body_json(&handle_link_oauth_status(&params))["state"], "expired");
        assert!(row_oauth(&address).is_none(), "no row when only a bad-state callback arrives");
        cleanup(&project);
    }

    // ── pure parsing ──

    #[test]
    fn parse_loopback_request_pulls_code_state_error() {
        let cap = parse_loopback_request("GET /cb?code=abc%2Fdef&state=xyz HTTP/1.1\r\n\r\n");
        assert_eq!(cap.code.as_deref(), Some("abc/def"));
        assert_eq!(cap.state.as_deref(), Some("xyz"));
        assert!(cap.error.is_none());

        let cap = parse_loopback_request("GET /cb?error=access_denied&state=xyz HTTP/1.1");
        assert_eq!(cap.error.as_deref(), Some("access_denied"));
        assert!(cap.code.is_none());
    }
}
