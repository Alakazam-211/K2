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
use k2_core::terminal::{AlacEvent, DaemonPtyConfig, DaemonPtySession};
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
    pub url: Option<String>,
    pub code: Option<String>,
    pub error: Option<String>,
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

/// The first sign-in URL in some text: https, not a localhost callback.
pub fn find_url(text: &str) -> Option<String> {
    for raw in text.split(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == '<' || c == '>') {
        let w = raw.trim_matches(|c: char| c == '(' || c == ')' || c == ',' || c == '.' || c == '`');
        if let Some(pos) = w.find("https://") {
            let u = &w[pos..];
            if u.contains("localhost") || u.contains("127.0.0.1") || u.len() < 12 {
                continue;
            }
            return Some(u.to_string());
        }
    }
    None
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
        cols: 400,
        rows: 40,
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
                ),
            }
        };
        let (pty, dir, elapsed, cancel_req, method, ticket, tool, account_id, new_entry, prev_state) = snap;
        let Some(pty) = pty else { return };

        // Screen + detection.
        let rows = pty.visible_text_rows();
        let text = rows.join("\n");
        let file_url = std::fs::read_to_string(dir.join("urls"))
            .ok()
            .and_then(|s| s.lines().rev().find_map(|l| find_url(l)));
        let url = file_url.or_else(|| find_url(&text));
        let code = find_device_code(&text);
        let paste = wants_pasted_code(&text);
        let mut screen: Vec<String> = rows;
        while screen.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
            screen.pop();
        }
        let start = screen.len().saturating_sub(30);
        let screen: Vec<String> = screen[start..].to_vec();
        if let Some((_, t)) = update(&id, |l| {
            l.screen = screen;
            if url.is_some() {
                l.url = url.clone();
            }
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

/// The daemon's refresher for idle wallet slots.
pub struct DaemonRefresher;

impl wallet::Refresher for DaemonRefresher {
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
    let row = crate::subscription_usage::probe_wallet_slot(tool.as_str(), &slot, claude_token);
    let text = serde_json::to_string(&row).map_err(|e| e.to_string())?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    wallet_core::update_meta(
        &conn,
        &entry.id,
        &wallet_core::MetaUpdate {
            usage_json: Some(text),
            usage_checked_at: Some(chrono::Utc::now().timestamp()),
            ..Default::default()
        },
    )
    .map_err(|e| e.to_string())
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
