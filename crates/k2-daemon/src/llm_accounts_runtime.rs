//! LLM login wallet, daemon side: login terminals, keep-warm loop, boot
//! import, idle-slot refresh and usage. Routes live in
//! `llm_accounts_routes.rs`; the wallet itself in
//! `k2_core::llm_accounts`.
//!
//! **Login terminal.** A daemon-owned PTY runs the tool's OWN login
//! command, argv directly (no shell, no history file), with no passport
//! and no hook variables (`K2_CELL=login`). Temp-home method: the tool's
//! home variable points at the new wallet slot, so the active login is
//! never touched. Live-swap fallback: the tool's normal login runs, the
//! daemon watches the live store for a change, captures it, and restores
//! the previous login. The daemon pulls the sign-in URL (a `BROWSER`
//! capture script plus a URL scan of the screen), a device code, and a
//! "paste the code" prompt out of the terminal so a phone never has to
//! read it. Ends itself after 15 minutes or when the CLI exits.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use k2_core::llm_accounts::{self as wallet_core, state, store, wallet, Entry, Tool, WalletError};
use k2_core::terminal::{AlacEvent, DaemonPtyConfig, DaemonPtySession, ScanRow, VisibleRow};
use serde::Serialize;
use serde_json::{json, Value};

pub const LOGIN_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Finished logins stay visible to `login/status` this long.
const LOGIN_KEEP: Duration = Duration::from_secs(10 * 60);
pub const LIVE_SWAP_BANNER: &str =
    "Signing in temporarily switches this tool for every session on this server.";
pub const SWITCH_NOTE: &str =
    "Changing the server default token affects every chat that uses Server default on this server.";
const KEEP_WARM_EVERY: Duration = Duration::from_secs(30 * 60);
const KEEP_WARM_FIRST: Duration = Duration::from_secs(120);

/// Where the browser for a login is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LoginMode {
    /// The browser is on the daemon's own machine.
    ThisComputer,
    /// The person signs in on another device (web, phone, SSH): device
    /// code where the tool has one, otherwise the paste-code flow.
    OtherDevice,
}

impl LoginMode {
    pub fn parse(s: Option<&str>) -> LoginMode {
        match s.map(str::trim) {
            Some("this_computer") | Some("this-computer") | Some("local") => LoginMode::ThisComputer,
            _ => LoginMode::OtherDevice,
        }
    }
}

/// Who started a login (only they, or the owner token, may type into it
/// or see its URL / code / screen).
pub type StarterKey = String;

struct Login {
    login_id: String,
    account_id: String,
    tool: Tool,
    label: String,
    mode: LoginMode,
    method: wallet::LoginMethod,
    started_by: StarterKey,
    started_at: i64,
    started: Instant,
    finished: Option<Instant>,
    pty: Option<Arc<DaemonPtySession>>,
    dir: PathBuf,
    state: String,
    url: Option<String>,
    code: Option<String>,
    error: Option<String>,
    screen: Vec<String>,
    new_entry: bool,
    prev_state: String,
    ticket: Option<wallet::LiveSwapTicket>,
    cancel: bool,
    offer_make_active: bool,
}

/// What a client sees of a login.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginView {
    pub login_id: String,
    pub account_id: String,
    pub tool: String,
    pub label: String,
    pub mode: LoginMode,
    pub method: wallet::LoginMethod,
    pub state: String,
    /// The whole sign-in URL (never cut; clients open and copy this).
    pub url: Option<String>,
    /// The URL's host ("claude.ai"), for "Sign in at …".
    pub url_host: Option<String>,
    pub code: Option<String>,
    pub error: Option<String>,
    /// The visible terminal as logical lines (a long URL stays whole on
    /// one line).
    pub screen: Vec<String>,
    pub started_at: i64,
    pub done: bool,
    pub banner: Option<String>,
    pub offer_make_active: bool,
}

fn logins() -> &'static Mutex<HashMap<String, Login>> {
    static L: OnceLock<Mutex<HashMap<String, Login>>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock_logins() -> std::sync::MutexGuard<'static, HashMap<String, Login>> {
    logins().lock().unwrap_or_else(|p| p.into_inner())
}

const TERMINAL: &[&str] = &["signed_in", "failed", "cancelled", "timed_out"];

fn is_done(state: &str) -> bool {
    TERMINAL.contains(&state)
}

fn view_of(l: &Login, full: bool) -> LoginView {
    LoginView {
        login_id: l.login_id.clone(),
        account_id: l.account_id.clone(),
        tool: l.tool.as_str().into(),
        label: l.label.clone(),
        mode: l.mode,
        method: l.method,
        state: l.state.clone(),
        url: if full { l.url.clone() } else { None },
        url_host: if full { l.url.as_deref().and_then(url_host) } else { None },
        code: if full { l.code.clone() } else { None },
        error: l.error.clone(),
        screen: if full { l.screen.clone() } else { Vec::new() },
        started_at: l.started_at,
        done: is_done(&l.state),
        banner: (l.method == wallet::LoginMethod::LiveSwap).then(|| LIVE_SWAP_BANNER.to_string()),
        offer_make_active: l.offer_make_active,
    }
}

fn may_see(l: &Login, who: &StarterKey) -> bool {
    who == "owner-token" || *who == l.started_by
}

fn prune() {
    let mut map = lock_logins();
    map.retain(|_, l| l.finished.map(|f| f.elapsed() < LOGIN_KEEP).unwrap_or(true));
}

/// Every login a human may see (url/code/screen only for its starter
/// and the owner token).
pub fn views(who: &StarterKey) -> Vec<LoginView> {
    prune();
    let map = lock_logins();
    let mut v: Vec<LoginView> = map.values().map(|l| view_of(l, may_see(l, who))).collect();
    v.sort_by_key(|x| x.started_at);
    v
}

pub fn view(login_id: &str, who: &StarterKey) -> Option<LoginView> {
    let map = lock_logins();
    map.get(login_id).map(|l| view_of(l, may_see(l, who)))
}

/// Is a sign-in running for this account right now?
pub fn running_for(account_id: &str) -> Option<String> {
    let map = lock_logins();
    map.values()
        .find(|l| l.account_id == account_id && !is_done(&l.state))
        .map(|l| l.login_id.clone())
}

// ── Detection (pure; unit tested) ───────────────────────────────────

/// Columns of the login terminal. Wide enough that real sign-in URLs
/// (Claude ~460, Codex browser ~700 characters) never wrap at all.
pub const LOGIN_COLS: u16 = 1000;
/// Rows of the login terminal.
pub const LOGIN_ROWS: u16 = 40;
/// Scrollback rows read above the screen each tick, so a URL that
/// scrolled off before the watcher looked is still found.
pub const LOGIN_HISTORY: usize = 200;

/// Is this a usable sign-in URL? It must parse as https and its host
/// must not be this machine (a local OAuth callback). Only the host is
/// tested: a `redirect_uri=http://localhost…` query parameter is part
/// of a real sign-in URL and must not drop it.
pub fn sign_in_url_ok(u: &str) -> bool {
    if !u.starts_with("https://") || u.len() < 12 {
        return false;
    }
    match url::Url::parse(u) {
        Ok(p) => p.scheme() == "https" && p.host().is_some() && !is_local_host(&p),
        Err(_) => false,
    }
}

fn is_local_host(u: &url::Url) -> bool {
    match u.host() {
        Some(url::Host::Domain(d)) => {
            let d = d.trim_end_matches('.').to_ascii_lowercase();
            d == "localhost" || d.ends_with(".localhost")
        }
        Some(url::Host::Ipv4(ip)) => ip.is_loopback() || ip.is_unspecified(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback() || ip.is_unspecified(),
        None => false,
    }
}

/// Characters that may appear in a URL (RFC 3986 unreserved, reserved
/// and `%`).
fn is_url_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "-._~:/?#[]@!$&'()*+,;=%".contains(c)
}

/// Trim what surrounds a URL in prose, and nothing that belongs to it:
/// a trailing `.` or `,`; a `)` or `]` only when it closes nothing in
/// the URL; a `'` only when the URL was opened with one.
fn trim_url(u: &str, quoted: bool) -> &str {
    let mut s = u;
    loop {
        let before = s.len();
        s = s.trim_end_matches(['.', ',']);
        if s.ends_with(')') && s.matches(')').count() > s.matches('(').count() {
            s = &s[..s.len() - 1];
        }
        if s.ends_with(']') && s.matches(']').count() > s.matches('[').count() {
            s = &s[..s.len() - 1];
        }
        if quoted && s.ends_with('\'') {
            s = &s[..s.len() - 1];
        }
        if s.len() == before {
            return s;
        }
    }
}

/// Every sign-in URL in some text, in order: https, not a local
/// callback, never shortened except for surrounding punctuation.
pub fn url_candidates(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw in text.split(|c: char| c.is_whitespace() || c == '"' || c == '<' || c == '>' || c == '`') {
        let Some(pos) = raw.find("https://") else { continue };
        let quoted = raw[..pos].ends_with('\'');
        let tail = &raw[pos..];
        let end = tail.find(|c: char| !is_url_char(c)).unwrap_or(tail.len());
        let u = trim_url(&tail[..end], quoted);
        if sign_in_url_ok(u) {
            out.push(u.to_string());
        }
    }
    out
}

/// The first sign-in URL in some text: https, not a localhost callback.
pub fn find_url(text: &str) -> Option<String> {
    url_candidates(text).into_iter().next()
}

/// The hosts a tool's sign-in page lives on (a host or any subdomain
/// of it matches). A link on another host is only used when no
/// vendor link is on screen.
pub fn vendor_hosts(tool: Tool) -> &'static [&'static str] {
    match tool {
        Tool::Claude => &["claude.ai", "claude.com", "anthropic.com"],
        Tool::Codex => &["auth.openai.com", "chatgpt.com", "openai.com"],
        Tool::Grok => &["x.ai"],
        Tool::Gemini => &["accounts.google.com"],
    }
}

/// The host of a URL, for "Sign in at claude.ai".
pub fn url_host(u: &str) -> Option<String> {
    url::Url::parse(u).ok().and_then(|p| p.host_str().map(|h| h.to_string()))
}

fn host_is_vendor(tool: Tool, u: &url::Url) -> bool {
    let Some(h) = u.host_str() else { return false };
    let h = h.to_ascii_lowercase();
    vendor_hosts(tool).iter().any(|d| h == *d || h.ends_with(&format!(".{d}")))
}

/// Does the URL look like a sign-in page (not a docs or status link)?
fn looks_like_sign_in(u: &url::Url) -> bool {
    let path = u.path().to_ascii_lowercase();
    let path_hit = ["oauth", "authorize", "device", "login", "signin", "sign-in", "activate", "auth"]
        .iter()
        .any(|w| path.contains(w));
    let query_hit = u.query_pairs().any(|(k, _)| {
        matches!(k.as_ref(), "client_id" | "user_code" | "code_challenge" | "response_type" | "redirect_uri" | "state")
    });
    path_hit || query_hit
}

/// Does a query parameter send the browser back to this machine
/// (`redirect_uri=http://localhost:1455/…`)? Such a link only finishes
/// in a browser on the server itself.
pub fn redirects_to_local(u: &str) -> bool {
    let Ok(p) = url::Url::parse(u) else { return false };
    p.query_pairs()
        .any(|(_, v)| url::Url::parse(&v).map(|r| is_local_host(&r)).unwrap_or(false))
}

/// Where a sign-in URL was read from. Higher wins a tie.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum UrlSource {
    /// Text on the terminal (may have been cut by layout).
    Screen,
    /// The BROWSER shim: the tool's own "open this URL" call, whole.
    ShimFile,
    /// An OSC 8 hyperlink target: whole, can't be cut by layout.
    Osc8,
}

/// A candidate sign-in URL and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlCandidate {
    pub url: String,
    pub source: UrlSource,
}

/// Pick the sign-in URL deliberately, never "the first https link":
/// 1. on another device, a link that sends the browser back to
///    localhost can't finish, so any other link beats it;
/// 2. a link on the tool's own sign-in hosts ([`vendor_hosts`]);
/// 3. a link shaped like a sign-in (oauth/device path, client_id…);
/// 4. the source: OSC 8 target > BROWSER shim > screen text;
/// 5. the latest one (a tool that issues a new link prints it last).
pub fn pick_url(tool: Tool, mode: LoginMode, cands: &[UrlCandidate]) -> Option<UrlCandidate> {
    cands
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            let p = url::Url::parse(&c.url).ok()?;
            let reachable = mode == LoginMode::ThisComputer || !redirects_to_local(&c.url);
            Some(((reachable, host_is_vendor(tool, &p), looks_like_sign_in(&p), c.source, i), c))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, c)| c.clone())
}

/// Rebuild logical lines from the visible rows. A row the terminal
/// soft-wrapped (alacritty's WRAPLINE: the tool wrote one long line,
/// e.g. Claude's sign-in URL, and the terminal broke it at the last
/// column) continues on the next row with no newline between them.
/// Rows the tool ended with its own newline stay separate lines.
pub fn logical_lines(rows: &[VisibleRow]) -> Vec<String> {
    let mut out = Vec::with_capacity(rows.len());
    let mut cur = String::new();
    for r in rows {
        cur.push_str(&r.text);
        if !r.wrapped {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Does the text so far end inside a URL (its last token is one)?
fn ends_in_url(cur: &str) -> bool {
    let last = cur.rsplit(char::is_whitespace).next().unwrap_or("");
    last.find("https://").map(|p| last[p..].chars().all(is_url_char)).unwrap_or(false)
}

/// Is a row the rest of a URL the tool broke with its own newline at
/// the terminal edge? One token of URL characters, nothing else, and
/// not something that stands alone (a new link, a device code, a
/// "Label:" prompt).
fn is_url_continuation(row: &str) -> bool {
    let s = row.trim();
    !s.is_empty()
        && s.chars().all(is_url_char)
        && !s.starts_with("http://")
        && !s.starts_with("https://")
        && !s.ends_with(':')
        && find_device_code(&format!("code {s}")).as_deref() != Some(s)
}

/// Logical lines from scan rows (screen and scrollback). Joins a
/// terminal soft wrap (WRAPLINE) like [`logical_lines`], and also a
/// HARD wrap: a CLI that breaks a long URL itself writes a newline at
/// the last column, so alacritty sets no WRAPLINE. A row that fills
/// every column and ends inside a URL is joined to the next row when
/// that row is only the URL's continuation ([`is_url_continuation`]).
/// A URL that ends exactly at the edge followed by ordinary text is
/// not glued.
pub fn scan_lines(rows: &[ScanRow], cols: usize) -> Vec<String> {
    let mut out = Vec::with_capacity(rows.len());
    let mut cur = String::new();
    let mut continuing = false;
    for (i, r) in rows.iter().enumerate() {
        cur.push_str(if continuing { r.text.trim_start() } else { &r.text });
        continuing = false;
        if r.wrapped {
            continue;
        }
        let at_edge = cols > 0 && r.text.chars().count() >= cols;
        if at_edge && ends_in_url(&cur) {
            if let Some(next) = rows.get(i + 1) {
                if is_url_continuation(&next.text) {
                    continuing = true;
                    continue;
                }
            }
        }
        out.push(std::mem::take(&mut cur));
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// One read of the login terminal: the sign-in URL candidates from
/// every source, the text to search for a code, and what to show.
#[derive(Debug, Clone, Default)]
pub struct LoginScan {
    pub candidates: Vec<UrlCandidate>,
    /// Screen + scrollback as logical lines (for the device code).
    pub all_text: String,
    /// The visible screen as logical lines (for the paste prompt).
    pub screen_text: String,
    /// The last 30 visible logical lines, trailing blanks dropped
    /// ("Show terminal"; a long URL stays on one line).
    pub screen: Vec<String>,
}

/// Read scan rows and the BROWSER-shim file's lines into candidates.
pub fn scan_login(rows: &[ScanRow], cols: usize, shim_lines: &str) -> LoginScan {
    let mut candidates: Vec<UrlCandidate> = Vec::new();
    for r in rows {
        for l in &r.links {
            if sign_in_url_ok(l) && !candidates.iter().any(|c| c.url == *l) {
                candidates.push(UrlCandidate { url: l.clone(), source: UrlSource::Osc8 });
            }
        }
    }
    // The shim gets the URL as one argument: take each line whole.
    for line in shim_lines.lines() {
        let u = line.trim();
        if sign_in_url_ok(u) {
            candidates.push(UrlCandidate { url: u.to_string(), source: UrlSource::ShimFile });
        } else {
            for u in url_candidates(u) {
                candidates.push(UrlCandidate { url: u, source: UrlSource::ShimFile });
            }
        }
    }
    let all = scan_lines(rows, cols);
    let all_text = all.join("\n");
    for u in url_candidates(&all_text) {
        candidates.push(UrlCandidate { url: u, source: UrlSource::Screen });
    }
    let first_visible = rows.iter().position(|r| r.visible).unwrap_or(rows.len());
    let visible = scan_lines(&rows[first_visible..], cols);
    let mut screen: Vec<String> = visible.iter().map(|l| l.trim_end().to_string()).collect();
    while screen.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
        screen.pop();
    }
    let start = screen.len().saturating_sub(30);
    LoginScan { candidates, all_text, screen_text: visible.join("\n"), screen: screen[start..].to_vec() }
}

/// The sign-in URL to hold after one read. A whole URL (OSC 8 target or
/// the BROWSER-shim file) wins whenever there is one. A screen read
/// never shortens what we hold: a read that is a prefix of the stored
/// URL (cut at a row edge, or partly scrolled) keeps the stored one; a
/// longer read that extends it, or a different URL (the tool issued a
/// new one), replaces it.
pub fn next_url(stored: Option<&str>, from_file: Option<String>, from_screen: Option<String>) -> Option<String> {
    if from_file.is_some() {
        return from_file;
    }
    match (stored, from_screen) {
        (Some(old), Some(new)) if old.starts_with(new.as_str()) => Some(old.to_string()),
        (_, Some(new)) => Some(new),
        (old, None) => old.map(str::to_string),
    }
}

/// [`next_url`] for a [`pick_url`] result: an OSC 8 or shim pick is
/// whole and replaces what we hold; a screen pick never shortens it.
pub fn next_url_from(stored: Option<&str>, picked: Option<UrlCandidate>) -> Option<String> {
    match picked {
        Some(c) if c.source == UrlSource::Screen => next_url(stored, None, Some(c.url)),
        Some(c) => next_url(stored, Some(c.url), None),
        None => next_url(stored, None, None),
    }
}

/// A device code shown in the terminal: `XXXX-XXXX` / `XXXX-XXXXX`
/// (letters and digits, upper case), next to the word "code".
pub fn find_device_code(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    if !lower.contains("code") {
        return None;
    }
    for raw in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')) {
        let parts: Vec<&str> = raw.split('-').collect();
        if parts.len() == 2
            && (4..=5).contains(&parts[0].len())
            && (4..=5).contains(&parts[1].len())
            && parts.iter().all(|p| p.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()))
            && parts.iter().any(|p| p.chars().any(|c| c.is_ascii_digit()) || p.len() >= 4)
        {
            return Some(raw.to_string());
        }
    }
    None
}

/// Is the tool waiting for a pasted authorization code?
pub fn wants_pasted_code(text: &str) -> bool {
    let l = text.to_ascii_lowercase();
    l.contains("paste code here") || l.contains("paste the code") || l.contains("enter the code") && l.contains("paste")
}

// ── The login command per tool ──────────────────────────────────────

/// argv and env for a tool's own login (never through a shell).
pub fn login_command(
    tool: Tool,
    mode: LoginMode,
    method: wallet::LoginMethod,
    slot: &Path,
) -> (String, Vec<String>, Vec<(String, String)>) {
    let temp = method == wallet::LoginMethod::TempHome;
    let mut env = Vec::new();
    if temp {
        env.push((tool.home_env_var().to_string(), slot.to_string_lossy().into_owned()));
    }
    let args: Vec<String> = match tool {
        // Never reached: start_login refuses tools without sign-in logins.
        Tool::Gemini => Vec::new(),
        Tool::Claude => vec!["auth".into(), "login".into(), "--claudeai".into()],
        Tool::Codex => {
            let mut a = vec![
                "login".into(),
                "-c".into(),
                "cli_auth_credentials_store=\"file\"".into(),
            ];
            if mode == LoginMode::OtherDevice {
                a.push("--device-auth".into());
            }
            a
        }
        Tool::Grok => {
            let mut a = vec!["login".into()];
            if mode == LoginMode::OtherDevice {
                a.push("--device-auth".into());
            }
            if temp {
                // Never share the running leader (it holds the active
                // login): an isolated leader socket inside the slot.
                a.push("--leader-socket".into());
                a.push(slot.join("leader-login.sock").to_string_lossy().into_owned());
            }
            a
        }
    };
    (tool.program().to_string(), args, env)
}

fn write_browser_script(dir: &Path, mode: LoginMode) -> std::io::Result<PathBuf> {
    let script = dir.join("browser.sh");
    let urls = dir.join("urls");
    let open_line = match mode {
        LoginMode::ThisComputer if cfg!(target_os = "macos") => "/usr/bin/open \"$1\" >/dev/null 2>&1 &\n",
        LoginMode::ThisComputer => "command -v xdg-open >/dev/null 2>&1 && xdg-open \"$1\" >/dev/null 2>&1 &\n",
        LoginMode::OtherDevice => "",
    };
    let body = format!(
        "#!/bin/sh\n# K2 login terminal: hand the sign-in URL to the daemon.\nprintf '%s\\n' \"$1\" >> '{}'\n{}exit 0\n",
        urls.to_string_lossy().replace('\'', "'\\''"),
        open_line
    );
    std::fs::write(&script, body)?;
    store::set_mode(&script, 0o700)?;
    Ok(script)
}

// ── Start / input / cancel ─────────────────────────────────────────

fn emit(tool: Option<Tool>) {
    crate::session_events::emit_llm_accounts_changed(tool.map(|t| t.as_str()));
}

/// Start the login terminal for `entry`. One at a time per account: a
/// second request returns the running one.
pub fn start_login(
    entry: &Entry,
    new_entry: bool,
    prev_state: &str,
    mode: LoginMode,
    started_by: &StarterKey,
) -> Result<LoginView, WalletError> {
    if k2_core::airgap::enabled() {
        return Err(WalletError::AirGap);
    }
    let tool = entry.tool().ok_or_else(|| WalletError::UnknownTool(entry.tool.clone()))?;
    if !tool.subscription_supported() || entry.is_api_key() {
        return Err(WalletError::Conflict(
            "api_key_login",
            "this token is an API token; there is no sign-in to run".into(),
        ));
    }
    if let Some(id) = running_for(&entry.id) {
        return view(&id, started_by).ok_or(WalletError::NotFound(id));
    }
    let method = wallet::login_method(tool);
    let login_id = format!("login_{}", &uuid::Uuid::new_v4().simple().to_string()[..16]);
    let dir = store::wallet_root().join(".login").join(&login_id);
    store::ensure_private_dir(&dir).map_err(|e| WalletError::Io(e.to_string()))?;
    let browser = write_browser_script(&dir, mode).map_err(|e| WalletError::Io(e.to_string()))?;
    let slot = store::slot_dir(tool, &entry.id);
    store::ensure_private_dir(&slot).map_err(|e| WalletError::Io(e.to_string()))?;

    let ticket = if method == wallet::LoginMethod::LiveSwap {
        let db = k2_core::db::shared();
        let conn = db.lock();
        Some(wallet::begin_live_swap(&conn, tool, &entry.id, Some(started_by))?)
    } else {
        None
    };

    let (program, args, extra_env) = login_command(tool, mode, method, &slot);
    let mut env: HashMap<String, String> = HashMap::new();
    for (k, v) in extra_env {
        env.insert(k, v);
    }
    env.insert("BROWSER".into(), browser.to_string_lossy().into_owned());
    env.insert("K2_CELL".into(), "login".into());
    if mode == LoginMode::OtherDevice {
        env.insert("NO_OPEN_BROWSER".into(), "1".into());
    }
    let cwd = if method == wallet::LoginMethod::TempHome { slot.clone() } else { dirs::home_dir().unwrap_or_else(|| slot.clone()) };
    let cfg = DaemonPtyConfig {
        cols: LOGIN_COLS,
        rows: LOGIN_ROWS,
        cwd: Some(cwd),
        program: Some(program),
        args,
        env,
        drain_on_exit: true,
        label: format!("{} sign-in", tool.display()),
        ..Default::default()
    };
    let pty = match DaemonPtySession::spawn(cfg) {
        Ok(p) => p,
        Err(e) => {
            if let Some(t) = &ticket {
                let db = k2_core::db::shared();
                let conn = db.lock();
                let _ = wallet::live_swap_abort(&conn, t);
            } else if new_entry {
                let db = k2_core::db::shared();
                let conn = db.lock();
                let _ = wallet::discard_unfinished(&conn, &entry.id);
            }
            let _ = std::fs::remove_dir_all(&dir);
            return Err(WalletError::Io(format!("could not start {} sign-in: {e}", tool.display())));
        }
    };
    let events = pty.subscribe_events();
    let login = Login {
        login_id: login_id.clone(),
        account_id: entry.id.clone(),
        tool,
        label: entry.label.clone(),
        mode,
        method,
        started_by: started_by.clone(),
        started_at: chrono::Utc::now().timestamp(),
        started: Instant::now(),
        finished: None,
        pty: Some(pty),
        dir,
        state: "signing_in".into(),
        url: None,
        code: None,
        error: None,
        screen: Vec::new(),
        new_entry,
        prev_state: prev_state.to_string(),
        ticket,
        cancel: false,
        offer_make_active: false,
    };
    let v = view_of(&login, true);
    lock_logins().insert(login_id.clone(), login);
    let id = login_id.clone();
    std::thread::Builder::new()
        .name(format!("llm-login-{}", &login_id[6..12]))
        .spawn(move || watch(id, events))
        .map_err(|e| WalletError::Io(e.to_string()))?;
    emit(Some(tool));
    Ok(v)
}

/// Type into a login terminal (the pasted code + Enter). Only its
/// starter or the owner token. The text is never logged or stored.
pub fn input(login_id: &str, text: &str, who: &StarterKey) -> Result<(), (u16, &'static str, String)> {
    let map = lock_logins();
    let l = map.get(login_id).ok_or((404, "not_found", "no such login".to_string()))?;
    if !may_see(l, who) {
        return Err((403, "forbidden", "only the person who started this sign-in can type into it".into()));
    }
    if is_done(&l.state) {
        return Err((409, "login_done", "this sign-in has already finished".into()));
    }
    let clean: String = text.chars().filter(|c| !c.is_control()).collect();
    if clean.is_empty() || clean.len() > 4096 {
        return Err((400, "bad_request", "text must be 1–4096 printable characters".into()));
    }
    if let Some(pty) = &l.pty {
        let mut bytes = clean.into_bytes();
        bytes.push(b'\r');
        pty.write(bytes);
    }
    Ok(())
}

pub fn cancel(login_id: &str, who: &StarterKey) -> Result<LoginView, (u16, &'static str, String)> {
    let mut map = lock_logins();
    let l = map.get_mut(login_id).ok_or((404, "not_found", "no such login".to_string()))?;
    if !may_see(l, who) {
        return Err((403, "forbidden", "only the person who started this sign-in can cancel it".into()));
    }
    l.cancel = true;
    Ok(view_of(l, true))
}

// ── Watcher ─────────────────────────────────────────────────────────

fn update<F: FnOnce(&mut Login)>(id: &str, f: F) -> Option<(String, Tool)> {
    let mut map = lock_logins();
    let l = map.get_mut(id)?;
    let before = (l.state.clone(), l.url.clone(), l.code.clone());
    f(l);
    let after = (l.state.clone(), l.url.clone(), l.code.clone());
    (before != after).then(|| (l.state.clone(), l.tool))
}

fn finish(id: &str, state_: &str, error: Option<String>, offer: bool) {
    let dir_and_pty = {
        let mut map = lock_logins();
        map.get_mut(id).map(|l| {
            l.state = state_.to_string();
            l.error = error;
            l.offer_make_active = offer;
            l.finished = Some(Instant::now());
            l.ticket = None;
            (l.dir.clone(), l.pty.take(), l.tool)
        })
    };
    if let Some((dir, pty, tool)) = dir_and_pty {
        if let Some(p) = pty {
            p.kill();
        }
        let _ = std::fs::remove_dir_all(&dir);
        emit(Some(tool));
    }
}

fn restore_entry_state(account_id: &str, tool: Tool, prev: &str, new_entry: bool) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    if new_entry {
        let _ = wallet::discard_unfinished(&conn, account_id);
        return;
    }
    let st = if store::slot_has_cred(tool, account_id) {
        if prev == state::SIGNING_IN { state::SIGNED_IN.to_string() } else { prev.to_string() }
    } else {
        state::NOT_SET_UP.to_string()
    };
    let _ = wallet_core::update_meta(&conn, account_id, &wallet_core::MetaUpdate { state: Some(st), ..Default::default() });
}

fn active_is(tool: Tool, id: &str) -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    wallet_core::active_id(&conn, tool).ok().flatten().as_deref() == Some(id)
}

fn watch(id: String, mut events: tokio::sync::broadcast::Receiver<AlacEvent>) {
    loop {
        std::thread::sleep(Duration::from_millis(400));
        let snap = {
            let map = lock_logins();
            match map.get(&id) {
                None => return,
                Some(l) => (
                    l.pty.clone(),
                    l.dir.clone(),
                    l.started.elapsed(),
                    l.cancel,
                    l.method,
                    l.ticket.clone(),
                    l.tool,
                    l.account_id.clone(),
                    l.new_entry,
                    l.prev_state.clone(),
                    l.mode,
                ),
            }
        };
        let (pty, dir, elapsed, cancel_req, method, ticket, tool, account_id, new_entry, prev_state, mode) = snap;
        let Some(pty) = pty else { return };

        // Screen + detection. Every source is read each tick: OSC 8
        // link targets and scrollback from the terminal (logical lines,
        // soft and hard wraps joined), and the BROWSER-shim file (the
        // tool's own "open this URL" call). `pick_url` chooses the
        // sign-in link by vendor rules, never "the first https link".
        let rows = pty.scan_rows(LOGIN_HISTORY);
        let shim = std::fs::read_to_string(dir.join("urls")).unwrap_or_default();
        let scan = scan_login(&rows, LOGIN_COLS as usize, &shim);
        let picked = pick_url(tool, mode, &scan.candidates);
        let code = find_device_code(&scan.all_text);
        let paste = wants_pasted_code(&scan.screen_text);
        let screen = scan.screen;
        if let Some((_, t)) = update(&id, |l| {
            l.screen = screen;
            l.url = next_url_from(l.url.as_deref(), picked);
            if code.is_some() {
                l.code = code.clone();
            }
            if !is_done(&l.state) {
                l.state = if paste {
                    "waiting_for_code".into()
                } else if l.url.is_some() || l.code.is_some() {
                    "waiting_for_browser".into()
                } else {
                    "signing_in".into()
                };
            }
        }) {
            emit(Some(t));
        }

        // Live-swap: the live store changed → capture + restore.
        if let Some(t) = &ticket {
            if let Ok(Some(bytes)) = wallet::live_swap_poll(t) {
                let res = {
                    let db = k2_core::db::shared();
                    let conn = db.lock();
                    wallet::live_swap_complete(&conn, t, &bytes, None)
                };
                drop(bytes);
                match res {
                    Ok(_) => {
                        let offer = !active_is(tool, &account_id);
                        finish(&id, "signed_in", None, offer);
                    }
                    Err(e) => {
                        let db = k2_core::db::shared();
                        let conn = db.lock();
                        let _ = wallet::live_swap_abort(&conn, t);
                        drop(conn);
                        finish(&id, "failed", Some(e.to_string()), false);
                    }
                }
                return;
            }
        }

        let mut exited = !pty.is_child_alive();
        loop {
            match events.try_recv() {
                Ok(AlacEvent::ChildExit(_)) => {
                    exited = true;
                    pty.mark_child_exited();
                }
                Ok(_) => continue,
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }

        let give_up = if cancel_req {
            Some(("cancelled", None))
        } else if elapsed > LOGIN_TIMEOUT {
            Some(("timed_out", Some("sign-in took longer than 15 minutes".to_string())))
        } else {
            None
        };
        if let Some((st, err)) = give_up {
            pty.kill();
            if let Some(t) = &ticket {
                let db = k2_core::db::shared();
                let conn = db.lock();
                let _ = wallet::live_swap_abort(&conn, t);
            } else {
                restore_entry_state(&account_id, tool, &prev_state, new_entry);
            }
            finish(&id, st, err, false);
            return;
        }

        if exited {
            // Let the last output land, then decide.
            std::thread::sleep(Duration::from_millis(300));
            update(&id, |l| {
                if !is_done(&l.state) {
                    l.state = "verifying".into();
                }
            });
            match (&ticket, method) {
                (Some(t), _) => {
                    // The CLI exited without changing the live store.
                    let res = wallet::live_swap_poll(t);
                    match res {
                        Ok(Some(bytes)) => {
                            let r = {
                                let db = k2_core::db::shared();
                                let conn = db.lock();
                                wallet::live_swap_complete(&conn, t, &bytes, None)
                            };
                            match r {
                                Ok(_) => {
                                    let offer = !active_is(tool, &account_id);
                                    finish(&id, "signed_in", None, offer);
                                }
                                Err(e) => finish(&id, "failed", Some(e.to_string()), false),
                            }
                        }
                        _ => {
                            let db = k2_core::db::shared();
                            let conn = db.lock();
                            let _ = wallet::live_swap_abort(&conn, t);
                            drop(conn);
                            finish(&id, "failed", Some("the sign-in did not finish".into()), false);
                        }
                    }
                }
                (None, _) => {
                    let res = {
                        let db = k2_core::db::shared();
                        let conn = db.lock();
                        wallet::finalize_login(&conn, &account_id, None)
                    };
                    match res {
                        Ok(_) => {
                            let offer = !active_is(tool, &account_id);
                            finish(&id, "signed_in", None, offer);
                        }
                        Err(e) => {
                            restore_entry_state(&account_id, tool, &prev_state, new_entry);
                            finish(&id, "failed", Some(e.to_string()), false);
                        }
                    }
                }
            }
            return;
        }
    }
}

// ── Spawn doors: pins, API keys, recorded homes ─────────────────────

/// Sessions running on a pinned subscription slot: account id → child
/// pids. A slot with a live pid is CLI-owned; keep-warm skips it.
fn in_use_map() -> &'static Mutex<HashMap<String, Vec<i32>>> {
    static M: OnceLock<Mutex<HashMap<String, Vec<i32>>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

fn pid_alive(pid: i32) -> bool {
    #[cfg(unix)]
    {
        pid > 0 && unsafe { libc::kill(pid as libc::pid_t, 0) } == 0
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

/// Is a pinned slot in use by a running session?
pub fn slot_in_use(account_id: &str) -> bool {
    let mut m = in_use_map().lock().unwrap_or_else(|p| p.into_inner());
    match m.get_mut(account_id) {
        Some(pids) => {
            pids.retain(|p| pid_alive(*p));
            !pids.is_empty()
        }
        None => false,
    }
}

/// Holds the slot lock across a pinned spawn; [`SpawnGuard::spawned`]
/// marks the slot in use by the child.
pub struct SpawnGuard {
    pub login: k2_core::llm_accounts::pins::SpawnLogin,
    _slot: Option<store::SlotLock>,
}

impl SpawnGuard {
    pub fn spawned(self, pid: Option<i32>) {
        if let (Some((_, id)), Some(pid)) = (self.login.pinned_slot(), pid) {
            in_use_map()
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .entry(id.to_string())
                .or_default()
                .push(pid);
        }
    }
}

fn is_resume_argv(tool: Tool, args: &[String]) -> bool {
    match tool {
        Tool::Codex => args.iter().any(|a| a == "resume"),
        Tool::Claude => args.iter().any(|a| a == "--resume" || a == "-r" || a == "--continue" || a == "-c"),
        _ => args.iter().any(|a| a == "--resume" || a == "-r" || a == "--continue"),
    }
}

/// Decide and apply the login for an agent spawn (both spawn doors call
/// this right before `DaemonPtySession::spawn`): resume → its chat's pin
/// (history copied into that login's home first), else the recorded
/// login; fresh → chat pin > workspace pin > pool. Adds the env (a
/// pinned slot's home variable, or an API key) to `env`, prepares a
/// pinned slot's home, records the decision, and returns a guard that
/// holds the slot lock until the child is up. `None` for non-agent
/// programs and for plain pool spawns with nothing to inject.
pub fn apply_spawn_login(
    program: Option<&str>,
    args: &[String],
    env: &mut HashMap<String, String>,
    session_key: &str,
    project_id: Option<&str>,
    cwd: Option<&Path>,
) -> Result<Option<SpawnGuard>, WalletError> {
    let Some(tool) = program.and_then(Tool::from_command) else {
        return Ok(None);
    };
    let conversation = k2_core::workspace::provider_resume::session_id_from_spawn_argv(program.unwrap_or(""), args);
    let resume = is_resume_argv(tool, args);
    let login = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        // A chat pick made on this key follows the conversation the argv
        // names (pre-minted or resumed), so the chat reopened under
        // another key finds it.
        if let Some(cid) = conversation.as_deref() {
            k2_core::llm_accounts::pins::mirror_to_conversation(&conn, tool, session_key, cid)?;
        }
        let login = k2_core::llm_accounts::pins::decide_spawn(
            &conn,
            tool,
            session_key,
            project_id,
            conversation.as_deref(),
            resume,
        )?;
        // A fresh spawn, or a resume moved to its chat's token: record
        // where the conversation now runs.
        if !resume || login.source == k2_core::llm_accounts::pins::Source::SessionPin {
            k2_core::llm_accounts::pins::record_spawn(&conn, &login, session_key, conversation.as_deref())?;
        }
        login
    };
    let slot_lock = match login.pinned_slot() {
        Some((t, id)) => {
            let l = store::lock_slot(t, id, Duration::from_secs(30))?;
            k2_core::llm_accounts::pins::prepare_slot_home(t, id, cwd)?;
            Some(l)
        }
        None => None,
    };
    if let Some(c) = &login.carry {
        k2_core::llm_accounts::pins::carry_conversation(tool, &c.from, &c.to, &c.conversation_id)
            .map_err(|e| WalletError::Io(format!("couldn't bring this chat's history to its token: {e}")))?;
    }
    for (k, v) in &login.env {
        env.insert(k.clone(), v.clone());
    }
    if login.account_id.is_none() && login.env.is_empty() {
        return Ok(None);
    }
    Ok(Some(SpawnGuard { login, _slot: slot_lock }))
}

// ── Refresh of idle slots ───────────────────────────────────────────

/// HTTP for the Claude refresh grant. Errors carry the status and the
/// OAuth error code only (never the body, which could echo a token).
fn post_json(url: &str, body: &Value) -> Result<Value, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let resp = client
        .post(url)
        .header("Content-Type", "application/json")
        .body(serde_json::to_vec(body).map_err(|e| e.to_string())?)
        .send()
        .map_err(|_| "refresh request failed".to_string())?;
    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if !status.is_success() {
        let code = v["error"]
            .as_str()
            .map(str::to_string)
            .or_else(|| v["error"]["type"].as_str().map(str::to_string))
            .unwrap_or_default();
        let code: String = code.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').take(60).collect();
        return Err(format!("{} {}", status.as_u16(), code));
    }
    Ok(v)
}

/// Run the tool's own CLI in an idle Grok slot so it refreshes its
/// token itself (`grok models`, no model tokens spent).
fn grok_warm(slot: &Path) -> Result<(), String> {
    let search = k2_core::terminal::login_path::augmented_path(&k2_core::terminal::login_path::process_path());
    let guard = k2_core::terminal::agent_spawn_guard::GuardEnv::from_process();
    let program = k2_core::terminal::agent_spawn_guard::resolve_program("grok", &search, &guard)?;
    let mut child = std::process::Command::new(program)
        .arg("models")
        .env("GROK_HOME", slot)
        .env("PATH", &search)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match child.try_wait() {
            Ok(Some(st)) if st.success() => return Ok(()),
            Ok(Some(st)) => return Err(format!("401 grok models exited {:?}", st.code())),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(200)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("grok models timed out".into());
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// May the daemon call a vendor about a login (identity, usage after a
/// switch)? Never under air-gap, never when usage probes are denied, and
/// never from tests (agent shim dir or a temp HOME: fake tokens never
/// leave the machine).
pub fn vendor_calls_allowed() -> bool {
    if k2_core::airgap::enabled() {
        return false;
    }
    if std::env::var_os("K2_SUBSCRIPTION_PROBE").as_deref() == Some(std::ffi::OsStr::new("deny")) {
        return false;
    }
    let g = k2_core::terminal::agent_spawn_guard::GuardEnv::from_process();
    !g.home_is_temp() && g.shim_dirs.is_none()
}

/// Claude's own account check (Claude Code 2.1.300 `hYe`): the profile
/// of the account an OAuth access token belongs to.
const CLAUDE_PROFILE_URL: &str = "https://api.anthropic.com/api/oauth/profile";
/// The whole profile check, connect included.
const PROFILE_CHECK_TIMEOUT: Duration = Duration::from_secs(5);

/// Which account a Claude credential belongs to, asked of Anthropic with
/// the credential's own access token. The token never leaves this
/// function except as the bearer header; errors are logged by status
/// only. `None` when it can't tell (expired token, offline, 4xx).
fn claude_identify(cred: &[u8]) -> Option<k2_core::llm_accounts::identity::AccountIdentity> {
    let v: Value = serde_json::from_slice(cred).ok()?;
    let o = &v["claudeAiOauth"];
    let token = o["accessToken"].as_str().filter(|s| !s.is_empty())?;
    if let Some(exp) = o["expiresAt"].as_i64() {
        let exp_s = if exp > 1_000_000_000_000 { exp / 1000 } else { exp };
        if exp_s <= chrono::Utc::now().timestamp() + 30 {
            return None;
        }
    }
    // Through the daemon's egress gate (air-gap refuses before any DNS
    // lookup; resolve once, vet, pin). Phase 1 of a switch runs this with
    // no database lock held; a slow or failed answer means "can't tell",
    // and the switch falls back to ~/.claude.json.
    let url = url::Url::parse(CLAUDE_PROFILE_URL).ok()?;
    let dest = match crate::egress::Destination::vet(&url, false) {
        Ok(d) => d,
        Err(e) => {
            k2_core::log_debug!("[llm-accounts] claude profile check: {}", e.code);
            return None;
        }
    };
    let client = match crate::egress::client(crate::egress::Purpose::LlmAccountCheck, &dest, PROFILE_CHECK_TIMEOUT) {
        Ok(c) => c,
        Err(e) => {
            k2_core::log_debug!("[llm-accounts] claude profile check: {}", e.code);
            return None;
        }
    };
    let resp = client
        .get(url)
        .bearer_auth(token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("Accept", "application/json")
        .header("Accept-Encoding", "identity")
        .send();
    let resp = match resp {
        Ok(r) => r,
        Err(_) => {
            k2_core::log_debug!("[llm-accounts] claude profile check: request failed");
            return None;
        }
    };
    if !resp.status().is_success() {
        k2_core::log_debug!("[llm-accounts] claude profile check: status {}", resp.status().as_u16());
        return None;
    }
    let body: Value = serde_json::from_str(&resp.text().ok()?).ok()?;
    k2_core::llm_accounts::identity::AccountIdentity::from_claude_profile_response(&body)
}

/// The daemon's refresher for idle wallet slots.
pub struct DaemonRefresher;

impl wallet::Refresher for DaemonRefresher {
    fn identify(&self, tool: Tool, cred: &[u8]) -> Option<k2_core::llm_accounts::identity::AccountIdentity> {
        if tool != Tool::Claude || !vendor_calls_allowed() {
            return None;
        }
        claude_identify(cred)
    }

    fn refresh(&self, tool: Tool, id: &str) -> Result<(), String> {
        if k2_core::airgap::enabled() {
            return Err(k2_core::airgap::TEACHING.to_string());
        }
        match tool {
            Tool::Claude => wallet::claude_refresh_slot(id, &post_json),
            Tool::Codex => crate::subscription_usage::codex_refresh_slot(&store::slot_dir(tool, id)),
            Tool::Grok => grok_warm(&store::slot_dir(tool, id)),
            Tool::Gemini => Err("Gemini tokens are API tokens; nothing to refresh".into()),
        }
    }
}

// ── Usage of idle slots ─────────────────────────────────────────────

/// Probe usage for one idle login and store the snapshot. Holds the tool
/// lock; never runs for the active login (its row is the shared cache)
/// and never under air-gap.
pub fn probe_idle_usage(entry: &Entry) -> Result<(), String> {
    if k2_core::airgap::enabled() {
        return Ok(());
    }
    let tool = entry.tool().ok_or("unknown tool")?;
    if active_is(tool, &entry.id) || !store::slot_has_cred(tool, &entry.id) {
        return Ok(());
    }
    let _lock = store::lock_tool(tool, Duration::from_secs(5)).map_err(|e| e.to_string())?;
    if active_is(tool, &entry.id) {
        return Ok(());
    }
    let slot = store::slot_dir(tool, &entry.id);
    let claude_token = if tool == Tool::Claude {
        let meta = store::slot_meta(tool, &entry.id);
        let expired = meta.expires_at.map(|x| x <= chrono::Utc::now().timestamp() + 60).unwrap_or(false);
        if expired && meta.has_refresh {
            let _ = wallet::claude_refresh_slot(&entry.id, &post_json);
        }
        let meta = store::slot_meta(tool, &entry.id);
        store::slot_claude_access_token(&entry.id).map(|t| (t, meta.expires_at.map(|s| s * 1000), meta.plan.unwrap_or_default()))
    } else {
        None
    };
    // The slot's own last reading (never another token's): a failed
    // check keeps it with its old time.
    let prev: Option<crate::subscription_usage::HarnessUsage> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        wallet_core::get(&conn, &entry.id)
            .ok()
            .flatten()
            .and_then(|e| e.usage_json)
            .and_then(|s| serde_json::from_str(&s).ok())
    };
    let mut row = crate::subscription_usage::probe_wallet_slot(tool.as_str(), &slot, claude_token, prev.as_ref());
    row.source = "probe".into();
    crate::subscription_usage::note_outcome(&format!("acct:{}", entry.id), &row, chrono::Utc::now());
    let text = serde_json::to_string(&row).map_err(|e| e.to_string())?;
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        // A switch may have made it live while the check ran: then the shared
        // probe owns its row.
        if active_is(tool, &entry.id) {
            return Ok(());
        }
        wallet_core::update_meta(
            &conn,
            &entry.id,
            &wallet_core::MetaUpdate {
                usage_json: Some(text),
                usage_checked_at: if row.error.is_none() { Some(chrono::Utc::now().timestamp()) } else { None },
                ..Default::default()
            },
        )
        .map_err(|e| e.to_string())?;
    }
    crate::session_events::emit_subscription_usage_changed(Some(tool.as_str()), Some(&entry.id), "probe");
    Ok(())
}

/// Usage v2 (P9, Q5): probe idle signed-in logins whose reading is older
/// than `max_age`, skipping a login a running session reported on lately
/// (live feed) and one that is backing off after failures / a 429. Runs
/// from the 15-minute subscription loop; never under air-gap.
pub fn probe_idle_stale(max_age: Duration) {
    if k2_core::airgap::enabled() {
        return;
    }
    let now = chrono::Utc::now();
    let entries = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        wallet_core::list(&conn).unwrap_or_default()
    };
    for e in entries {
        let Some(tool) = e.tool() else { continue };
        if e.state != state::SIGNED_IN || e.is_api_key() || !tool.subscription_supported() || active_is(tool, &e.id) {
            continue;
        }
        let fresh = e.usage_checked_at.is_some_and(|t| now.timestamp() - t < max_age.as_secs() as i64);
        let reading: Option<crate::subscription_usage::HarnessUsage> =
            e.usage_json.as_deref().and_then(|s| serde_json::from_str(s).ok());
        if fresh || reading.as_ref().is_some_and(|r| crate::subscription_usage::live_fresh(r, now, max_age)) {
            continue;
        }
        if crate::subscription_usage::backing_off(&format!("acct:{}", e.id), now).is_some() {
            continue;
        }
        if let Err(err) = probe_idle_usage(&e) {
            k2_core::log_debug!("[llm-accounts] usage {}: {err}", e.id);
        }
    }
}

// ── Boot + loop ─────────────────────────────────────────────────────

/// Import each tool's live login as the active "Default" wallet entry
/// when nothing is active yet. Local reads only.
pub fn boot_import() {
    for tool in Tool::ALL {
        let r = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            wallet::import_live(&conn, tool, Some("boot"))
        };
        match r {
            Ok(Some(_)) => emit(Some(tool)),
            Ok(None) => {}
            Err(e) => k2_core::log_debug!("[llm-accounts] import {}: {}", tool.as_str(), e),
        }
    }
    // Claude items an earlier K2 (0.45.1/0.45.2) left unreadable by
    // `/usr/bin/security`: say so once at boot, with the fix. Settings →
    // LLMs and `k2 llm tokens list` show the same.
    let repairs = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        wallet_core::keychain_repairs(&conn)
    };
    match repairs {
        Ok(list) => {
            for r in list {
                k2_core::log_debug!(
                    "[llm-accounts] WARN: keychain item \"{}\" ({}) makes macOS ask for your password on every Claude read; fix once in Terminal: {}",
                    r.service,
                    r.holds,
                    r.command
                );
            }
        }
        Err(e) => k2_core::log_debug!("[llm-accounts] keychain check: {e}"),
    }
}

/// One keep-warm pass: refresh due idle slots, then probe their usage.
pub fn keep_warm_tick() -> Vec<wallet::WarmResult> {
    if k2_core::airgap::enabled() {
        return Vec::new();
    }
    let res = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        wallet::keep_warm(&conn, &DaemonRefresher, chrono::Utc::now().timestamp(), &|id| slot_in_use(id))
    };
    let res = match res {
        Ok(r) => r,
        Err(e) => {
            k2_core::log_debug!("[llm-accounts] keep-warm: {e}");
            return Vec::new();
        }
    };
    let entries = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        wallet_core::list(&conn).unwrap_or_default()
    };
    for e in &entries {
        if e.state == state::SIGNED_IN {
            if let Err(err) = probe_idle_usage(e) {
                k2_core::log_debug!("[llm-accounts] usage {}: {err}", e.id);
            }
        }
    }
    if !res.is_empty() || !entries.is_empty() {
        emit(None);
    }
    res
}

/// Boot import, then the keep-warm loop (first pass after two minutes,
/// then every 30). Skips everything under air-gap.
pub fn spawn() {
    if std::env::var("K2_LLM_ACCOUNTS_LOOP").map(|v| v == "0").unwrap_or(false) {
        boot_import();
        return;
    }
    let _ = std::thread::Builder::new().name("llm-accounts".into()).spawn(|| {
        boot_import();
        std::thread::sleep(KEEP_WARM_FIRST);
        loop {
            let _ = keep_warm_tick();
            std::thread::sleep(KEEP_WARM_EVERY);
        }
    });
}

/// JSON for a login view.
pub fn login_json(v: &LoginView) -> Value {
    serde_json::to_value(v).unwrap_or_else(|_| json!({}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_detection_skips_localhost_and_trims_punctuation() {
        let screen = "Opening browser to sign in…\nIf the browser didn't open, visit: https://claude.com/cai/oauth/authorize?code=true&client_id=abc&state=xyz\nPaste code here if prompted >";
        assert_eq!(
            find_url(screen).as_deref(),
            Some("https://claude.com/cai/oauth/authorize?code=true&client_id=abc&state=xyz")
        );
        assert!(wants_pasted_code(screen));
        assert_eq!(find_url("listening on http://localhost:1455/auth/callback"), None);
        assert_eq!(find_url("go to (https://auth.example.test/device)."), Some("https://auth.example.test/device".into()));
        assert_eq!(find_url("nothing here"), None);
    }

    /// A real-shaped Claude sign-in URL (~460 chars), longer than any
    /// terminal row.
    const CLAUDE_URL: &str = "https://claude.ai/oauth/authorize?code=true&client_id=9d1c250a-e61b-44d9-88ed-5944d1962f5e&response_type=code&redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback&scope=org%3Acreate_api_key+user%3Aprofile+user%3Ainference+user%3Asessions%3Aclaude_code+user%3Amcp_servers+user%3Afile_upload+user%3Aplugins&code_challenge=hhX09VfGxZ4Oe_6zAToJMg0tIdpFMzGge4PW-yYvPLo&code_challenge_method=S256&state=ciSJq3Xk2mV7pL0aB9cD4eF6gH8iJ1kL2mN3oP4qR5s";

    /// The exact byte shape `claude auth login --claudeai` writes
    /// (captured from the real CLI): one line, an OSC 8 hyperlink and
    /// a color around the URL, then the paste prompt on its own line.
    fn claude_login_bytes(url: &str) -> Vec<u8> {
        format!(
            "Opening browser to sign in\u{2026}\r\nIf the browser didn't open, visit: \x1b]8;;{url}\x07\x1b[94m{url}\x1b[39m\x1b]8;;\x07\r\nPaste code here if prompted > "
        )
        .into_bytes()
    }

    fn screen_url(cols: usize, bytes: &[u8]) -> (Option<String>, Vec<VisibleRow>) {
        let rows = k2_core::terminal::daemon_pty::visible_rows_from_bytes(cols, 40, bytes);
        (find_url(&logical_lines(&rows).join("\n")), rows)
    }

    #[test]
    fn wrapped_claude_url_is_found_whole_at_80_120_and_400_columns() {
        assert!(CLAUDE_URL.len() > 400, "fixture must be longer than the login PTY row");
        for cols in [80usize, 120, 400] {
            let (url, rows) = screen_url(cols, &claude_login_bytes(CLAUDE_URL));
            assert!(rows[1].wrapped, "{cols} cols: the URL row must be soft-wrapped: {rows:?}");
            // The bug: the first row alone held only a cut URL.
            let first_row_only = find_url(&rows[1].text).expect("first row has the URL start");
            assert!(first_row_only.len() < CLAUDE_URL.len(), "{cols} cols: fixture must actually wrap");
            assert_eq!(url.as_deref(), Some(CLAUDE_URL), "{cols} cols: whole URL, rows {rows:?}");
            // The paste prompt after the URL is still its own line.
            assert!(wants_pasted_code(&logical_lines(&rows).join("\n")));
        }
    }

    #[test]
    fn url_ending_exactly_at_the_column_edge_is_not_glued_to_the_next_line() {
        let prefix = "visit: ";
        let url = "https://auth.example.test/device/abcdefghijklmnopqrstuvwxyz0123456789";
        let cols = prefix.len() + url.len();
        let bytes = format!("{prefix}{url}\r\nEnterTheCodeBelow ABCD-1234\r\n");
        let (found, rows) = screen_url(cols, bytes.as_bytes());
        assert!(!rows[0].wrapped, "exact-edge row then newline is not a soft wrap: {rows:?}");
        assert_eq!(found.as_deref(), Some(url));
    }

    #[test]
    fn localhost_callback_is_still_skipped_when_wrapped() {
        let local = format!("https://localhost:1455/auth/callback?state={}", "z".repeat(150));
        let bytes = format!("Listening on {local}\r\nwaiting\r\n");
        let (found, rows) = screen_url(80, bytes.as_bytes());
        assert!(rows[0].wrapped, "fixture must wrap: {rows:?}");
        assert_eq!(found, None);
    }

    #[test]
    fn short_device_urls_and_codes_still_found_through_logical_lines() {
        // Codex / Grok device flows: short URL + code, nothing wraps.
        let bytes = b"Open https://auth.openai.com/codex/device\r\nand enter this one-time code: ABCD-12345\r\n";
        let rows = k2_core::terminal::daemon_pty::visible_rows_from_bytes(400, 40, bytes);
        let text = logical_lines(&rows).join("\n");
        assert_eq!(find_url(&text).as_deref(), Some("https://auth.openai.com/codex/device"));
        assert_eq!(find_device_code(&text).as_deref(), Some("ABCD-12345"));
    }

    #[test]
    fn stored_url_only_upgrades_never_shortens() {
        let cut = &CLAUDE_URL[..360];
        // First read was cut: a later whole read upgrades it.
        assert_eq!(next_url(Some(cut), None, Some(CLAUDE_URL.into())).as_deref(), Some(CLAUDE_URL));
        // A later cut read never shortens the whole one.
        assert_eq!(next_url(Some(CLAUDE_URL), None, Some(cut.into())).as_deref(), Some(CLAUDE_URL));
        // Same read again: unchanged.
        assert_eq!(next_url(Some(CLAUDE_URL), None, Some(CLAUDE_URL.into())).as_deref(), Some(CLAUDE_URL));
        // Nothing on screen this tick: keep what we have.
        assert_eq!(next_url(Some(CLAUDE_URL), None, None).as_deref(), Some(CLAUDE_URL));
        assert_eq!(next_url(None, None, None), None);
        // A different URL (the tool re-issued one) replaces it.
        let other = "https://claude.ai/oauth/authorize?code=true&state=new";
        assert_eq!(next_url(Some(CLAUDE_URL), None, Some(other.into())).as_deref(), Some(other));
        // The BROWSER-shim file always wins over the screen.
        let file = "https://auth.example.test/from-file";
        assert_eq!(next_url(Some(CLAUDE_URL), Some(file.into()), Some(CLAUDE_URL.into())).as_deref(), Some(file));
        assert_eq!(next_url(None, Some(file.into()), None).as_deref(), Some(file));
    }

    // ── S1: the sign-in link is never cut (0.46.3) ─────────────────

    /// Claude's automatic sign-in URL: the redirect goes back to the
    /// CLI's localhost listener (what the BROWSER shim receives).
    const CLAUDE_AUTO_URL: &str = "https://claude.ai/oauth/authorize?code=true&client_id=9d1c250a-e61b-44d9-88ed-5944d1962f5e&response_type=code&redirect_uri=http%3A%2F%2Flocalhost%3A54545%2Fcallback&scope=org%3Acreate_api_key+user%3Aprofile+user%3Ainference+user%3Asessions%3Aclaude_code+user%3Amcp_servers+user%3Afile_upload+user%3Aplugins&code_challenge=hhX09VfGxZ4Oe_6zAToJMg0tIdpFMzGge4PW-yYvPLo&code_challenge_method=S256&state=ciSJq3Xk2mV7pL0aB9cD4eF6gH8iJ1kL2mN3oP4qR5s";

    /// Codex's browser login (~700 chars), `redirect_uri` on localhost.
    fn codex_browser_url() -> String {
        format!(
            "https://auth.openai.com/oauth/authorize?response_type=code&client_id=app_EMoamEEZ73f0CkXaXp7hrann&redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback&scope=openid%20profile%20email%20offline_access%20api.connectors.read%20api.connectors.invoke&code_challenge={}&code_challenge_method=S256&id_token_add_organizations=true&codex_cli_simplified_flow=true&state={}&originator=codex_cli_rs&allowed_workspace_id={}",
            "Qm9zc29uLWNvZGUtY2hhbGxlbmdlLWZpeHR1cmUtMDQ2Mw".repeat(3),
            "c3RhdGUtZml4dHVyZS1mb3ItdGhlLWNvZGV4LWxvZ2lu".repeat(4),
            "ws-".to_string() + &"0123456789abcdef".repeat(6)
        )
    }

    fn scan(cols: usize, bytes: &[u8], shim: &str) -> LoginScan {
        let rows = k2_core::terminal::daemon_pty::scan_rows_from_bytes(cols, LOGIN_ROWS as usize, LOGIN_HISTORY, bytes);
        scan_login(&rows, cols, shim)
    }

    fn picked(tool: Tool, mode: LoginMode, s: &LoginScan) -> UrlCandidate {
        pick_url(tool, mode, &s.candidates).unwrap_or_else(|| panic!("no URL picked from {:?}", s.candidates))
    }

    /// The URL broken by the CLI's own newlines at the column edge (an
    /// Ink/ratatui TUI): no WRAPLINE, no OSC 8.
    fn hard_wrapped(prefix: &str, url: &str, cols: usize, after: &str) -> Vec<u8> {
        let text: Vec<char> = format!("{prefix}{url}").chars().collect();
        let rows: Vec<String> = text.chunks(cols).map(|c| c.iter().collect()).collect();
        format!("{}\r\n{after}", rows.join("\r\n")).into_bytes()
    }

    #[test]
    fn claude_osc8_target_wins_whole_at_80_120_400_and_1000_columns() {
        for cols in [80usize, 120, 400, 1000] {
            let s = scan(cols, &claude_login_bytes(CLAUDE_URL), "");
            assert!(
                s.candidates.iter().any(|c| c.source == UrlSource::Osc8 && c.url == CLAUDE_URL),
                "{cols} cols: OSC 8 target missing: {:?}",
                s.candidates
            );
            let p = picked(Tool::Claude, LoginMode::OtherDevice, &s);
            assert_eq!(p.url, CLAUDE_URL, "{cols} cols");
            assert_eq!(p.source, UrlSource::Osc8, "{cols} cols");
            assert!(wants_pasted_code(&s.screen_text), "{cols} cols: paste prompt");
            // "Show terminal" sends logical lines: the URL is whole on one line.
            assert!(s.screen.iter().any(|l| l.ends_with(CLAUDE_URL)), "{cols} cols: screen {:?}", s.screen);
        }
    }

    #[test]
    fn hard_wrapped_url_is_joined_whole_at_80_and_120_columns() {
        for cols in [80usize, 120] {
            // URL on its own rows, and URL after a prefix on the same row.
            for prefix in ["", "If the browser didn't open, visit: "] {
                let bytes = hard_wrapped(prefix, CLAUDE_URL, cols, "Paste code here if prompted > ");
                let rows = k2_core::terminal::daemon_pty::scan_rows_from_bytes(cols, 40, LOGIN_HISTORY, &bytes);
                assert!(rows.iter().all(|r| !r.wrapped), "{cols}: fixture must be hard-wrapped (no WRAPLINE): {rows:?}");
                // The bug: the old soft-wrap-only join cut it at the first row.
                let visible: Vec<VisibleRow> =
                    rows.iter().map(|r| VisibleRow { text: r.text.clone(), wrapped: r.wrapped }).collect();
                let old = find_url(&logical_lines(&visible).join("\n")).expect("old path finds a cut URL");
                assert!(old.len() < CLAUDE_URL.len(), "{cols}: fixture must cut on the old path");
                let s = scan_login(&rows, cols, "");
                let p = picked(Tool::Claude, LoginMode::OtherDevice, &s);
                assert_eq!(p.url, CLAUDE_URL, "{cols} cols, prefix {prefix:?}");
                assert_eq!(p.source, UrlSource::Screen);
                assert!(wants_pasted_code(&s.screen_text));
                assert!(s.screen.iter().any(|l| l.ends_with(CLAUDE_URL)), "{cols}: screen {:?}", s.screen);
            }
        }
    }

    #[test]
    fn exact_edge_url_is_not_glued_to_text_or_a_device_code_on_the_scan_path() {
        let prefix = "visit: ";
        let url = "https://auth.example.test/device/abcdefghijklmnopqrstuvwxyz0123456789";
        let cols = prefix.len() + url.len();
        for after in ["EnterTheCodeBelow ABCD-1234\r\n", "ABCD-1234\r\n", "Code:\r\n", "https://other.example.test/x\r\n"] {
            let s = scan(cols, format!("{prefix}{url}\r\n{after}").as_bytes(), "");
            let screen: Vec<&str> = s.candidates.iter().filter(|c| c.source == UrlSource::Screen).map(|c| c.url.as_str()).collect();
            assert_eq!(screen.first().copied(), Some(url), "after {after:?}: {screen:?}");
        }
    }

    #[test]
    fn redirect_uri_on_localhost_no_longer_drops_the_url() {
        let codex = codex_browser_url();
        assert!(codex.len() > 650, "fixture is ~700 chars: {}", codex.len());
        assert_eq!(find_url(&format!("navigate to this URL:\n{codex}\n")).as_deref(), Some(codex.as_str()));
        assert_eq!(find_url(CLAUDE_AUTO_URL).as_deref(), Some(CLAUDE_AUTO_URL));
        assert!(redirects_to_local(&codex));
        assert!(!redirects_to_local(CLAUDE_URL), "platform.claude.com callback is not local");
        // A callback HOST is still skipped, however it is spelled.
        for local in [
            "https://localhost:1455/auth/callback?code=x",
            "https://127.0.0.1:1455/auth/callback?code=x",
            "https://[::1]:1455/auth/callback?code=x",
            "https://app.localhost/cb",
        ] {
            assert_eq!(find_url(local), None, "{local}");
        }
    }

    #[test]
    fn codex_browser_login_via_shim_and_via_screen() {
        let codex = codex_browser_url();
        // Shim: the CLI's own BROWSER call, whole.
        let s = scan(LOGIN_COLS as usize, b"Starting local login server on http://localhost:1455.\r\n", &format!("{codex}\n"));
        let p = picked(Tool::Codex, LoginMode::ThisComputer, &s);
        assert_eq!((p.url.as_str(), p.source), (codex.as_str(), UrlSource::ShimFile));
        // Screen only (the browser didn't open), at the real login width
        // and at 400 columns where it soft-wraps.
        for cols in [LOGIN_COLS as usize, 400] {
            let bytes = format!(
                "Starting local login server on http://localhost:1455.\r\nIf your browser did not open, navigate to this URL to authenticate:\r\n\r\n{codex}\r\n"
            );
            let s = scan(cols, bytes.as_bytes(), "");
            let p = picked(Tool::Codex, LoginMode::ThisComputer, &s);
            assert_eq!(p.url, codex, "{cols} cols");
        }
    }

    #[test]
    fn on_another_device_a_link_back_to_localhost_loses_to_one_that_finishes_anywhere() {
        // Claude handed the shim its automatic (localhost) URL and
        // printed the manual one (platform.claude.com callback).
        let s = scan(LOGIN_COLS as usize, &claude_login_bytes(CLAUDE_URL), &format!("{CLAUDE_AUTO_URL}\n"));
        assert_eq!(picked(Tool::Claude, LoginMode::OtherDevice, &s).url, CLAUDE_URL);
        // Same, with no OSC 8: the plain screen URL still beats the shim.
        let plain = format!("If the browser didn't open, visit: {CLAUDE_URL}\r\nPaste code here if prompted > ");
        let s = scan(LOGIN_COLS as usize, plain.as_bytes(), &format!("{CLAUDE_AUTO_URL}\n"));
        assert_eq!(picked(Tool::Claude, LoginMode::OtherDevice, &s).url, CLAUDE_URL);
    }

    #[test]
    fn codex_device_flow_url_and_code() {
        let bytes = b"Welcome to Codex\r\nSee https://status.openai.com for outages.\r\n\r\nFollow these steps to sign in with ChatGPT using device code authorization:\r\n\r\n1. Open this link in your browser and sign in to your account\r\n   https://auth.openai.com/codex/device\r\n\r\n2. Enter this one-time code (expires in 15 minutes)\r\n   ABCD-12345\r\n";
        let s = scan(LOGIN_COLS as usize, bytes, "");
        assert_eq!(picked(Tool::Codex, LoginMode::OtherDevice, &s).url, "https://auth.openai.com/codex/device");
        assert_eq!(find_device_code(&s.all_text).as_deref(), Some("ABCD-12345"));
    }

    #[test]
    fn grok_device_url_with_user_code_beats_an_earlier_docs_link() {
        let url = "https://accounts.x.ai/oauth2/device?user_code=QWER-TY12&client_id=grok-cli";
        let bytes = format!(
            "Grok CLI docs: https://docs.x.ai/docs/grok-cli/getting-started\r\nTo sign in, open {url}\r\nand confirm the code QWER-TY12\r\n"
        );
        let s = scan(LOGIN_COLS as usize, bytes.as_bytes(), "");
        assert_eq!(s.candidates.first().map(|c| c.url.as_str()), Some("https://docs.x.ai/docs/grok-cli/getting-started"));
        assert_eq!(picked(Tool::Grok, LoginMode::OtherDevice, &s).url, url);
        assert_eq!(find_device_code(&s.all_text).as_deref(), Some("QWER-TY12"));
    }

    #[test]
    fn two_links_on_screen_the_sign_in_link_is_picked_not_the_first() {
        // A non-vendor link and a vendor docs link, both before the
        // sign-in link.
        let bytes = format!(
            "Note: https://github.com/anthropics/claude-code/issues/1 is fixed.\r\nDocs: https://docs.anthropic.com/en/docs/claude-code/setup\r\nIf the browser didn't open, visit: {CLAUDE_URL}\r\nPaste code here if prompted > "
        );
        let s = scan(LOGIN_COLS as usize, bytes.as_bytes(), "");
        assert!(s.candidates.len() >= 3, "{:?}", s.candidates);
        assert_ne!(s.candidates[0].url, CLAUDE_URL, "fixture: the sign-in link is not first");
        assert_eq!(picked(Tool::Claude, LoginMode::OtherDevice, &s).url, CLAUDE_URL);
        // No vendor link at all: fall back to a sign-in-shaped link.
        let s = scan(
            LOGIN_COLS as usize,
            b"Help: https://example.test/help\r\nOpen https://sso.example.test/oauth/authorize?client_id=x&state=y\r\n",
            "",
        );
        assert_eq!(
            picked(Tool::Claude, LoginMode::OtherDevice, &s).url,
            "https://sso.example.test/oauth/authorize?client_id=x&state=y"
        );
    }

    #[test]
    fn a_2000_char_url_is_found_whole() {
        let url = format!("https://claude.ai/oauth/authorize?code=true&state={}", "a1B2c3D4e5".repeat(195));
        assert!(url.len() > 2000);
        let s = scan(LOGIN_COLS as usize, format!("visit: {url}\r\n").as_bytes(), "");
        assert_eq!(picked(Tool::Claude, LoginMode::OtherDevice, &s).url, url);
        let s = scan(LOGIN_COLS as usize, &hard_wrapped("visit: ", &url, LOGIN_COLS as usize, "done\r\n"), "");
        assert_eq!(picked(Tool::Claude, LoginMode::OtherDevice, &s).url, url);
    }

    #[test]
    fn a_url_scrolled_off_the_screen_is_still_found() {
        let mut bytes = format!("If the browser didn't open, visit: {CLAUDE_URL}\r\n").into_bytes();
        for i in 0..(LOGIN_ROWS as usize + 20) {
            bytes.extend_from_slice(format!("progress line {i}\r\n").as_bytes());
        }
        // The fixture really scrolled: the visible screen has no URL.
        let visible = k2_core::terminal::daemon_pty::visible_rows_from_bytes(LOGIN_COLS as usize, LOGIN_ROWS as usize, &bytes);
        assert_eq!(find_url(&logical_lines(&visible).join("\n")), None, "fixture must scroll the URL off");
        let s = scan(LOGIN_COLS as usize, &bytes, "");
        assert_eq!(picked(Tool::Claude, LoginMode::OtherDevice, &s).url, CLAUDE_URL);
        // Shown terminal is the visible screen only.
        assert!(s.screen.iter().all(|l| !l.contains("https://")), "{:?}", s.screen);
        // And with OSC 8 (the target scrolls off with the text).
        let mut b = claude_login_bytes(CLAUDE_URL);
        b.extend_from_slice(b"\r\n");
        for i in 0..(LOGIN_ROWS as usize + 20) {
            b.extend_from_slice(format!("progress line {i}\r\n").as_bytes());
        }
        let p = picked(Tool::Claude, LoginMode::OtherDevice, &scan(LOGIN_COLS as usize, &b, ""));
        assert_eq!((p.url.as_str(), p.source), (CLAUDE_URL, UrlSource::Osc8));
    }

    #[test]
    fn punctuation_trimming_keeps_what_belongs_to_the_url() {
        assert_eq!(find_url("go to (https://auth.example.test/device).").as_deref(), Some("https://auth.example.test/device"));
        assert_eq!(find_url("see https://auth.example.test/a_(b) now").as_deref(), Some("https://auth.example.test/a_(b)"));
        assert_eq!(find_url("open https://auth.example.test/it's?x=1, then").as_deref(), Some("https://auth.example.test/it's?x=1"));
        assert_eq!(find_url("open 'https://auth.example.test/q?x=1' now").as_deref(), Some("https://auth.example.test/q?x=1"));
        assert_eq!(find_url("[https://auth.example.test/d]").as_deref(), Some("https://auth.example.test/d"));
        assert_eq!(find_url("`https://auth.example.test/d`").as_deref(), Some("https://auth.example.test/d"));
    }

    #[test]
    fn whole_sources_replace_and_screen_reads_never_shorten() {
        let cut = CLAUDE_URL[..300].to_string();
        let screen = |u: &str| Some(UrlCandidate { url: u.to_string(), source: UrlSource::Screen });
        let osc8 = |u: &str| Some(UrlCandidate { url: u.to_string(), source: UrlSource::Osc8 });
        assert_eq!(next_url_from(Some(CLAUDE_URL), screen(&cut)).as_deref(), Some(CLAUDE_URL));
        assert_eq!(next_url_from(Some(&cut), osc8(CLAUDE_URL)).as_deref(), Some(CLAUDE_URL));
        assert_eq!(next_url_from(Some(CLAUDE_URL), None).as_deref(), Some(CLAUDE_URL));
        assert_eq!(next_url_from(None, None), None);
    }

    #[test]
    fn login_view_carries_the_url_host_only_for_who_may_see() {
        assert_eq!(url_host(CLAUDE_URL).as_deref(), Some("claude.ai"));
        assert_eq!(url_host(&codex_browser_url()).as_deref(), Some("auth.openai.com"));
        let l = Login {
            login_id: "login_t".into(),
            account_id: "acc".into(),
            tool: Tool::Claude,
            label: "x".into(),
            mode: LoginMode::OtherDevice,
            method: wallet::LoginMethod::TempHome,
            started_by: "me".into(),
            started_at: 0,
            started: Instant::now(),
            finished: None,
            pty: None,
            dir: PathBuf::from("/nonexistent"),
            state: "waiting_for_browser".into(),
            url: Some(CLAUDE_URL.into()),
            code: None,
            error: None,
            screen: vec![],
            new_entry: false,
            prev_state: String::new(),
            ticket: None,
            cancel: false,
            offer_make_active: false,
        };
        let full = login_json(&view_of(&l, true));
        assert_eq!(full["url"], CLAUDE_URL);
        assert_eq!(full["urlHost"], "claude.ai");
        let hidden = login_json(&view_of(&l, false));
        assert!(hidden["url"].is_null() && hidden["urlHost"].is_null(), "{hidden}");
    }

    #[test]
    fn device_code_detection() {
        let s = "Open https://auth.example.test/codex/device and enter this one-time code: ABCD-12345";
        assert_eq!(find_device_code(s).as_deref(), Some("ABCD-12345"));
        assert_eq!(find_device_code("Enter code WXYZ-9QRT to continue").as_deref(), Some("WXYZ-9QRT"));
        assert_eq!(find_device_code("no-code here: well-known"), None);
        assert_eq!(find_device_code("ABCD-1234 but no keyword"), None);
        assert!(!wants_pasted_code("Waiting for the browser"));
    }

    #[test]
    fn login_commands_use_the_tools_own_login_and_never_a_shell() {
        let slot = Path::new("/tmp/k2-test-slot");
        let (p, a, e) = login_command(Tool::Claude, LoginMode::OtherDevice, wallet::LoginMethod::TempHome, slot);
        assert_eq!(p, "claude");
        assert_eq!(a, vec!["auth", "login", "--claudeai"]);
        assert_eq!(e, vec![("CLAUDE_CONFIG_DIR".to_string(), "/tmp/k2-test-slot".to_string())]);
        let (p, a, e) = login_command(Tool::Codex, LoginMode::OtherDevice, wallet::LoginMethod::TempHome, slot);
        assert_eq!(p, "codex");
        assert!(a.contains(&"--device-auth".to_string()));
        assert!(a.contains(&"cli_auth_credentials_store=\"file\"".to_string()));
        assert_eq!(e[0].0, "CODEX_HOME");
        let (_, a, _) = login_command(Tool::Codex, LoginMode::ThisComputer, wallet::LoginMethod::TempHome, slot);
        assert!(!a.contains(&"--device-auth".to_string()));
        let (p, a, e) = login_command(Tool::Grok, LoginMode::OtherDevice, wallet::LoginMethod::TempHome, slot);
        assert_eq!(p, "grok");
        assert!(a.contains(&"--device-auth".to_string()));
        assert!(a.iter().any(|x| x.ends_with("leader-login.sock")), "isolated leader: {a:?}");
        assert_eq!(e[0].0, "GROK_HOME");
        // Live-swap: no home variable (the tool's normal login).
        let (_, _, e) = login_command(Tool::Claude, LoginMode::OtherDevice, wallet::LoginMethod::LiveSwap, slot);
        assert!(e.is_empty());
        for (_, _, env) in [login_command(Tool::Claude, LoginMode::OtherDevice, wallet::LoginMethod::TempHome, slot)] {
            assert!(env.iter().all(|(k, _)| k != "HOME" && k != "XDG_CONFIG_HOME"));
        }
    }

    #[test]
    fn mode_parse_defaults_to_other_device() {
        assert_eq!(LoginMode::parse(None), LoginMode::OtherDevice);
        assert_eq!(LoginMode::parse(Some("this_computer")), LoginMode::ThisComputer);
        assert_eq!(LoginMode::parse(Some("bogus")), LoginMode::OtherDevice);
    }
}
