//! App users' personal wallets, daemon side (prd-app-user-llm-logins-v1
//! rev 2): the vault sealer, person sign-ins, the person layer of the
//! spawn decision, and the sweep (re-seal, keep-warm, lifecycle). Routes
//! live in `person_wallet_routes.rs`; the wallet itself in
//! `k2_core::person_wallet`.
//!
//! **Person sign-ins** run the vendor CLI's own login in a daemon PTY with
//! the item's slot as the tool's home, always the "other device" flow
//! (device code or paste code), never LiveSwap. Unlike server sign-ins,
//! only the person who started one sees its URL / code and may type into
//! it: there is no owner-token bypass (V15), and no terminal screen is
//! ever sent.
//!
//! **Sealing.** Idle subscription slots are sealed in the vault's `person`
//! scope (system namespace; no route returns a system value). A spawn
//! unseals under the slot lock; the sweep re-seals every slot no session
//! uses, picking up the refreshed token.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use k2_core::llm_accounts::pins::{SpawnLogin, Source};
use k2_core::llm_accounts::{kind, state, store, wallet, Tool, WalletError};
use k2_core::person_wallet::{self as pw, decide, Item, SealKind, Sealer};
use k2_core::terminal::{AlacEvent, DaemonPtyConfig, DaemonPtySession};
use k2_vault::{Actor, Delivery, Kind, Namespace, PutMode, PutRequest, Requester, Scope, SecretBytes, SecretStore, Selector, Use};
use serde::Serialize;

use crate::llm_accounts_runtime as rt;

const SWEEP_EVERY: Duration = Duration::from_secs(60);
const WARM_EVERY: Duration = Duration::from_secs(30 * 60);

pub fn audit(event: &str, user: &str, outcome: &str, ingress: &str) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(event, user, outcome.to_string(), ingress, "-", "cli"));
}

/// `app:<principalId>`, the person key in audit lines (V41).
pub fn person_key(principal_id: &str) -> String {
    format!("app:{principal_id}")
}

// ── The vault's person scope ────────────────────────────────────────

/// The real [`Sealer`]: `system` namespace, scope `person:<principal>`,
/// name `wallet-<item>`, delivery `none` (nothing is ever delivered from
/// it; only the daemon resolves it with `Use::System`).
pub struct VaultSealer;

const SUBSYSTEM: &str = "person-wallet";

fn sel(principal_id: &str, item_id: &str) -> Selector {
    Selector::system(Scope::person(principal_id), pw::secret_name(item_id))
}

impl Sealer for VaultSealer {
    fn seal(&self, principal_id: &str, item_id: &str, kind_: SealKind, bytes: &[u8]) -> Result<(), String> {
        let vault = crate::vault::get().map_err(|e| format!("vault: {}", e.code))?;
        let k = match kind_ {
            SealKind::LoginSlot => Kind::LoginSlot,
            SealKind::ApiKey => Kind::ApiKey,
        };
        let mut r = PutRequest::new(Namespace::System, Scope::person(principal_id), pw::secret_name(item_id), k);
        r.default_delivery = Some(Delivery::None);
        r.delivery_ceiling = Some(Delivery::None);
        r.origin = format!("system:{SUBSYSTEM}");
        r.description = "A person's wallet item (App user); daemon-only.".into();
        r.mode = PutMode::Upsert;
        r.idempotent = true;
        vault
            .put(r, SecretBytes::from_slice(bytes), &Actor::System(SUBSYSTEM.into()))
            .map(|_| ())
            .map_err(|e| format!("vault: {}", e.code))
    }

    fn unseal(&self, principal_id: &str, item_id: &str) -> Result<Option<Vec<u8>>, String> {
        let vault = crate::vault::get().map_err(|e| format!("vault: {}", e.code))?;
        match vault.resolve(&sel(principal_id, item_id), Use::System, &Requester::System(SUBSYSTEM.into())) {
            Ok(r) => Ok(Some(r.value.expose().to_vec())),
            Err(e) if e.status == 404 || e.code == "secret_not_active" => Ok(None),
            Err(e) => Err(format!("vault: {}", e.code)),
        }
    }

    fn destroy(&self, principal_id: &str, item_id: &str) -> Result<(), String> {
        let vault = crate::vault::get().map_err(|e| format!("vault: {}", e.code))?;
        match vault.destroy(&sel(principal_id, item_id), &Actor::System(SUBSYSTEM.into())) {
            Ok(()) => Ok(()),
            Err(e) if e.status == 404 => Ok(()),
            Err(e) => Err(format!("vault: {}", e.code)),
        }
    }
}

fn with_conn<T>(f: impl FnOnce(&rusqlite::Connection) -> T) -> T {
    let db = k2_core::db::shared();
    let conn = db.lock();
    f(&conn)
}

// ── Person sign-ins ─────────────────────────────────────────────────

struct PLogin {
    login_id: String,
    item_id: String,
    principal_id: String,
    tool: Tool,
    label: String,
    started_at: i64,
    started: Instant,
    finished: Option<Instant>,
    pty: Option<Arc<DaemonPtySession>>,
    dir: PathBuf,
    state: String,
    url: Option<String>,
    code: Option<String>,
    needs_paste: bool,
    error: Option<String>,
    new_item: bool,
    prev_state: String,
    cancel: bool,
}

/// What the person who started a sign-in sees. Never a terminal screen
/// (it can hold an account email or org text).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonLoginView {
    pub login_id: String,
    pub item_id: String,
    pub provider: String,
    pub label: String,
    pub state: String,
    /// The whole sign-in URL (open and copy exactly this).
    pub url: Option<String>,
    /// The URL's host ("claude.ai"), for "Sign in at …".
    pub url_host: Option<String>,
    pub code: Option<String>,
    pub needs_paste: bool,
    pub error: Option<String>,
    pub started_at: i64,
    pub done: bool,
}

fn logins() -> &'static Mutex<HashMap<String, PLogin>> {
    static L: OnceLock<Mutex<HashMap<String, PLogin>>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock_logins() -> std::sync::MutexGuard<'static, HashMap<String, PLogin>> {
    logins().lock().unwrap_or_else(|p| p.into_inner())
}

const TERMINAL: &[&str] = &["signed_in", "failed", "cancelled", "timed_out"];
const LOGIN_KEEP: Duration = Duration::from_secs(10 * 60);

fn is_done(s: &str) -> bool {
    TERMINAL.contains(&s)
}

fn url_host(u: &str) -> Option<String> {
    url::Url::parse(u).ok().and_then(|p| p.host_str().map(str::to_string))
}

fn view_of(l: &PLogin) -> PersonLoginView {
    PersonLoginView {
        login_id: l.login_id.clone(),
        item_id: l.item_id.clone(),
        provider: l.tool.as_str().into(),
        label: l.label.clone(),
        state: l.state.clone(),
        url: l.url.clone(),
        url_host: l.url.as_deref().and_then(url_host),
        code: l.code.clone(),
        needs_paste: l.needs_paste,
        error: l.error.clone(),
        started_at: l.started_at,
        done: is_done(&l.state),
    }
}

fn prune() {
    lock_logins().retain(|_, l| l.finished.map(|f| f.elapsed() < LOGIN_KEEP).unwrap_or(true));
}

/// A person's own sign-in. Anyone else (another person, the owner token,
/// an agent) gets `None`, the same as a sign-in that doesn't exist.
pub fn view(login_id: &str, principal_id: &str) -> Option<PersonLoginView> {
    prune();
    let map = lock_logins();
    map.get(login_id).filter(|l| l.principal_id == principal_id).map(view_of)
}

/// The running sign-in for an item, if any.
pub fn running_for(item_id: &str) -> Option<String> {
    lock_logins().values().find(|l| l.item_id == item_id && !is_done(&l.state)).map(|l| l.login_id.clone())
}

/// Start the vendor CLI's own sign-in for a person's subscription item.
/// One at a time per item: a second request returns the running one (to
/// its starter only).
pub fn start(item: &Item, new_item: bool, prev_state: &str) -> Result<PersonLoginView, WalletError> {
    if k2_core::airgap::enabled() {
        return Err(WalletError::AirGap);
    }
    let tool = item.tool().ok_or_else(|| WalletError::UnknownTool(item.provider.clone()))?;
    if !item.is_subscription() || !tool.subscription_supported() {
        return Err(WalletError::Conflict("api_key_login", "this item is an API key; there is no sign-in to run".into()));
    }
    if let Some(id) = running_for(&item.id) {
        return view(&id, &item.principal_id).ok_or(WalletError::NotFound(id));
    }
    let login_id = format!("plogin_{}", &uuid::Uuid::new_v4().simple().to_string()[..16]);
    let dir = store::wallet_root().join(".login").join(&login_id);
    store::ensure_private_dir(&dir).map_err(|e| WalletError::Io(e.to_string()))?;
    let browser = rt::write_browser_script(&dir, rt::LoginMode::OtherDevice).map_err(|e| WalletError::Io(e.to_string()))?;
    let slot = pw::slot_dir(tool, &item.id);
    store::ensure_private_dir(&slot).map_err(|e| WalletError::Io(e.to_string()))?;
    if tool == Tool::Codex {
        // A full private home from the first sign-in (never a shadow).
        pw::prepare_person_home(tool, &item.id, None)?;
    }
    // Person sign-ins never use LiveSwap (V28), whatever K2_LLM_LOGIN_LIVE says.
    let (program, args, extra_env) = rt::login_command(tool, rt::LoginMode::OtherDevice, wallet::LoginMethod::TempHome, &slot);
    let mut env: HashMap<String, String> = extra_env.into_iter().collect();
    env.insert("BROWSER".into(), browser.to_string_lossy().into_owned());
    env.insert("K2_CELL".into(), "login".into());
    env.insert("NO_OPEN_BROWSER".into(), "1".into());
    let cfg = DaemonPtyConfig {
        cols: rt::LOGIN_COLS,
        rows: rt::LOGIN_ROWS,
        cwd: Some(slot.clone()),
        program: Some(program),
        args,
        env,
        drain_on_exit: true,
        label: format!("{} sign-in (wallet)", tool.display()),
        ..Default::default()
    };
    let pty = match DaemonPtySession::spawn(cfg) {
        Ok(p) => p,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(WalletError::Io(format!("could not start {} sign-in: {e}", tool.display())));
        }
    };
    let events = pty.subscribe_events();
    with_conn(|c| pw::set_state(c, &item.id, state::SIGNING_IN, None))?;
    let l = PLogin {
        login_id: login_id.clone(),
        item_id: item.id.clone(),
        principal_id: item.principal_id.clone(),
        tool,
        label: item.label.clone(),
        started_at: chrono::Utc::now().timestamp(),
        started: Instant::now(),
        finished: None,
        pty: Some(pty),
        dir,
        state: "signing_in".into(),
        url: None,
        code: None,
        needs_paste: false,
        error: None,
        new_item,
        prev_state: prev_state.to_string(),
        cancel: false,
    };
    let v = view_of(&l);
    lock_logins().insert(login_id.clone(), l);
    let id = login_id.clone();
    std::thread::Builder::new()
        .name(format!("wallet-login-{}", &login_id[7..13]))
        .spawn(move || watch(id, events))
        .map_err(|e| WalletError::Io(e.to_string()))?;
    Ok(v)
}

/// Tests: a running sign-in for `item` whose screen showed `url` / `code`
/// (no PTY, no CLI). Returns its login id.
#[cfg(test)]
pub fn insert_test_login(item: &Item, url: Option<&str>, code: Option<&str>, paste: bool) -> String {
    let login_id = format!("plogin_{}", &uuid::Uuid::new_v4().simple().to_string()[..16]);
    lock_logins().insert(
        login_id.clone(),
        PLogin {
            login_id: login_id.clone(),
            item_id: item.id.clone(),
            principal_id: item.principal_id.clone(),
            tool: item.tool().expect("tool"),
            label: item.label.clone(),
            started_at: 1,
            started: Instant::now(),
            finished: None,
            pty: None,
            dir: std::env::temp_dir(),
            state: if paste { "waiting_for_code".into() } else { "waiting_for_browser".into() },
            url: url.map(str::to_string),
            code: code.map(str::to_string),
            needs_paste: paste,
            error: None,
            new_item: false,
            prev_state: "signed_in".into(),
            cancel: false,
        },
    );
    login_id
}

/// Type the pasted code (+ Enter). Its starter only; never logged.
pub fn input(login_id: &str, text: &str, principal_id: &str) -> Result<(), (u16, &'static str, String)> {
    let map = lock_logins();
    let l = map
        .get(login_id)
        .filter(|l| l.principal_id == principal_id)
        .ok_or((404, "not_found", "no such sign-in".to_string()))?;
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

pub fn cancel(login_id: &str, principal_id: &str) -> Result<PersonLoginView, (u16, &'static str, String)> {
    let mut map = lock_logins();
    let l = map
        .get_mut(login_id)
        .filter(|l| l.principal_id == principal_id)
        .ok_or((404, "not_found", "no such sign-in".to_string()))?;
    l.cancel = true;
    Ok(view_of(l))
}

fn finish(id: &str, st: &str, error: Option<String>) {
    let parts = {
        let mut map = lock_logins();
        map.get_mut(id).map(|l| {
            l.state = st.to_string();
            l.error = error;
            l.finished = Some(Instant::now());
            (l.dir.clone(), l.pty.take())
        })
    };
    if let Some((dir, pty)) = parts {
        if let Some(p) = pty {
            p.kill();
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

fn give_back(item_id: &str, tool: Tool, prev: &str, new_item: bool) {
    with_conn(|c| {
        if new_item {
            let _ = pw::discard_unfinished(c, &VaultSealer, item_id);
            return;
        }
        let sealed = pw::get(c, item_id).ok().flatten().map(|i| i.sealed).unwrap_or(false);
        let st = if sealed || pw::slot_has_plaintext(tool, item_id) {
            if prev == state::SIGNING_IN { state::SIGNED_IN } else { prev }
        } else {
            state::NOT_SET_UP
        };
        let _ = pw::set_state(c, item_id, st, None);
    });
}

fn watch(id: String, mut events: tokio::sync::broadcast::Receiver<AlacEvent>) {
    loop {
        std::thread::sleep(Duration::from_millis(400));
        let snap = {
            let map = lock_logins();
            match map.get(&id) {
                None => return,
                Some(l) => (l.pty.clone(), l.dir.clone(), l.started.elapsed(), l.cancel, l.tool, l.item_id.clone(), l.new_item, l.prev_state.clone(), l.url.clone(), l.principal_id.clone()),
            }
        };
        let (pty, dir, elapsed, cancel_req, tool, item_id, new_item, prev_state, stored_url, principal_id) = snap;
        let Some(pty) = pty else { return };
        // One read of the terminal, through the same detection server
        // sign-ins use (S1's fixes land in that one function).
        let read = rt::read_login_tick(&pty, &dir, tool, rt::LoginMode::OtherDevice, stored_url.as_deref());
        {
            let mut map = lock_logins();
            if let Some(l) = map.get_mut(&id) {
                l.url = read.url;
                if read.code.is_some() {
                    l.code = read.code;
                }
                l.needs_paste = read.paste;
                if !is_done(&l.state) {
                    l.state = if read.paste {
                        "waiting_for_code".into()
                    } else if l.url.is_some() || l.code.is_some() {
                        "waiting_for_browser".into()
                    } else {
                        "signing_in".into()
                    };
                }
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
        } else if elapsed > rt::LOGIN_TIMEOUT {
            Some(("timed_out", Some("sign-in took longer than 15 minutes".to_string())))
        } else {
            None
        };
        if let Some((st, err)) = give_up {
            pty.kill();
            give_back(&item_id, tool, &prev_state, new_item);
            finish(&id, st, err);
            return;
        }
        if exited {
            std::thread::sleep(Duration::from_millis(300));
            let res = with_conn(|c| pw::finalize_signin(c, &VaultSealer, &item_id));
            match res {
                Ok(item) => {
                    audit("wallet.signed_in", &person_key(&principal_id), &format!("{} sealed={}", item.provider, item.sealed), "cli");
                    finish(&id, "signed_in", None);
                }
                Err(e) => {
                    give_back(&item_id, tool, &prev_state, new_item);
                    finish(&id, "failed", Some(e.to_string()));
                }
            }
            return;
        }
    }
}

// ── The spawn decision's person layer ──────────────────────────────

/// Sessions running on a person item: pid → (item, project). The sweep
/// stops the ones whose item may no longer run that agent.
fn running() -> &'static Mutex<HashMap<i32, (String, String)>> {
    static M: OnceLock<Mutex<HashMap<i32, (String, String)>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Called once the child is up (SpawnGuard::spawned).
pub fn note_spawn(item_id: &str, project_id: &str, pid: i32) {
    running().lock().unwrap_or_else(|p| p.into_inner()).insert(pid, (item_id.to_string(), project_id.to_string()));
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

fn stop_pid(pid: i32) {
    #[cfg(unix)]
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGTERM);
    }
    #[cfg(not(unix))]
    let _ = pid;
}

/// Stop every live session on `item_id` (optionally only in `project`).
/// Returns how many were stopped.
pub fn stop_sessions(item_id: &str, project: Option<&str>) -> usize {
    let mut m = running().lock().unwrap_or_else(|p| p.into_inner());
    m.retain(|pid, _| pid_alive(*pid));
    let pids: Vec<i32> = m
        .iter()
        .filter(|(_, (i, p))| i == item_id && project.map(|x| x == p).unwrap_or(true))
        .map(|(pid, _)| *pid)
        .collect();
    for pid in &pids {
        stop_pid(*pid);
        m.remove(pid);
    }
    pids.len()
}

/// The live (pid, item, project) rows.
fn running_rows() -> Vec<(i32, String, String)> {
    let mut m = running().lock().unwrap_or_else(|p| p.into_inner());
    m.retain(|pid, _| pid_alive(*pid));
    m.iter().map(|(pid, (i, p))| (*pid, i.clone(), p.clone())).collect()
}

/// Facts from skin.db for a candidate (read with no main-DB lock held).
fn facts_for(principal_id: &str, project_id: Option<&str>) -> decide::Facts {
    let person = k2_core::skin::wallet_access::person(principal_id).ok().flatten();
    let others_in_room = match project_id {
        Some(pid) if !pid.is_empty() => k2_core::skin::wallet_access::people_holding_room(pid)
            .unwrap_or_default()
            .into_iter()
            .filter(|p| p != principal_id)
            .collect(),
        _ => Vec::new(),
    };
    decide::Facts { person, others_in_room }
}

/// What the person layer decided for one spawn.
pub enum Step {
    /// Not a person spawn: the existing rules decide.
    Existing,
    /// Run on the person's item. The slot (for a subscription) is unsealed
    /// and its lock held until the child is up.
    Ready { login: SpawnLogin, slot: Option<store::SlotLock> },
    /// The person's pick can't be used and the App falls back to the
    /// company plan: the existing FRESH rules decide; a resumed
    /// conversation's history comes from `carry_from`.
    Fallback { carry_from: Option<PathBuf> },
}

/// The person layer of the spawn decision (called first by
/// `apply_spawn_login`). Errors refuse the spawn (409).
pub fn spawn_step(
    tool: Tool,
    session_key: &str,
    project_id: Option<&str>,
    conversation_id: Option<&str>,
    is_resume: bool,
    cwd: Option<&Path>,
) -> Result<Step, WalletError> {
    let cand = with_conn(|c| decide::candidate(c, tool, session_key, project_id, conversation_id, is_resume))?;
    let Some(cand) = cand else { return Ok(Step::Existing) };
    let room = cand.mark.as_ref().map(|m| m.project_id.clone()).or_else(|| project_id.map(str::to_string));
    let facts = facts_for(&cand.principal_id, room.as_deref());
    let fall_back = |why: &decide::Unusable| -> Result<Step, WalletError> {
        audit(
            "wallet.fallback_used",
            &person_key(&cand.principal_id),
            &format!("{} {} {}", tool.as_str(), cand.item_id, why.code()),
            "spawn",
        );
        let carry_from = cand.resume.then(|| pw::slot_dir(tool, &cand.item_id));
        Ok(Step::Fallback { carry_from })
    };
    match decide::plan(&cand, &facts) {
        decide::Plan::Refuse(why) => {
            audit("wallet.refused", &person_key(&cand.principal_id), &format!("{} {}", tool.as_str(), why.code()), "spawn");
            Err(decide::refusal(&cand, &facts, &why))
        }
        decide::Plan::Fallback(why) => fall_back(&why),
        decide::Plan::Person(item) => {
            let slot = if item.is_subscription() {
                let lock = store::lock_slot(tool, &item.id, Duration::from_secs(30))?;
                if let Err(e) = pw::unseal_into_slot(&VaultSealer, &item) {
                    drop(lock);
                    let why = decide::Unusable::NeedsLogin;
                    k2_core::log_debug!("[wallet] {}: unseal failed: {}", item.id, e.code());
                    return if cand.settings.fallback { fall_back(&why) } else { Err(decide::refusal(&cand, &facts, &why)) };
                }
                pw::prepare_person_home(tool, &item.id, cwd)?;
                Some(lock)
            } else {
                None
            };
            let env = pw::spawn_env(&VaultSealer, &item)?;
            let login = SpawnLogin {
                tool,
                account_id: Some(item.id.clone()),
                kind: Some(if item.is_api_key() { kind::API_KEY } else { kind::SUBSCRIPTION }.to_string()),
                source: Source::PersonPin,
                home: item.is_subscription().then(|| pw::slot_dir(tool, &item.id)),
                env,
                carry: None,
            };
            with_conn(|c| -> Result<(), WalletError> {
                pw::touch_used(c, &item.id)?;
                if !cand.resume {
                    k2_core::llm_accounts::pins::record_spawn(c, &login, session_key, conversation_id)?;
                }
                Ok(())
            })?;
            Ok(Step::Ready { login, slot })
        }
    }
}

// ── Sweep: re-seal, keep-warm, lifecycle ───────────────────────────

/// Re-seal every subscription slot that holds plaintext and no session or
/// sign-in uses. Returns the ids sealed.
pub fn reseal_idle() -> Vec<String> {
    let items = with_conn(|c| pw::list_all(c)).unwrap_or_default();
    let mut out = Vec::new();
    for item in items {
        let Some(tool) = item.tool() else { continue };
        if !item.is_subscription() || item.state == state::SIGNING_IN || running_for(&item.id).is_some() {
            continue;
        }
        if rt::slot_in_use(&item.id) || !pw::slot_has_plaintext(tool, &item.id) {
            continue;
        }
        let Ok(_lock) = store::lock_slot(tool, &item.id, Duration::from_secs(2)) else { continue };
        if rt::slot_in_use(&item.id) {
            continue;
        }
        match with_conn(|c| pw::seal_slot(c, &VaultSealer, &item)) {
            Ok(true) => out.push(item.id.clone()),
            Ok(false) => {}
            Err(e) => k2_core::log_debug!("[wallet] re-seal {}: {}", item.id, e.code()),
        }
    }
    out
}

/// Is an idle sealed item due for a refresh (daily, or within an hour of
/// expiry)?
pub fn warm_due(item: &Item, now: i64) -> bool {
    let last = item.refreshed_at.or(item.last_used_at).unwrap_or(item.created_at);
    if now - last >= wallet::WARM_EVERY_SECS {
        return true;
    }
    matches!(item.expires_at, Some(x) if item.provider != "grok" && x - now <= wallet::WARM_BEFORE_EXPIRY_SECS)
}

/// Keep idle sealed subscriptions fresh: unseal, let the vendor refresh,
/// re-seal (all under the slot lock). Never under air-gap.
pub fn keep_warm(refresher: &dyn wallet::Refresher) {
    if k2_core::airgap::enabled() {
        return;
    }
    let now = chrono::Utc::now().timestamp();
    let items = with_conn(|c| pw::list_all(c)).unwrap_or_default();
    for item in items {
        let Some(tool) = item.tool() else { continue };
        if !item.is_subscription() || !item.sealed || item.state != state::SIGNED_IN || item.dormant_since.is_some() {
            continue;
        }
        if rt::slot_in_use(&item.id) || running_for(&item.id).is_some() || !warm_due(&item, now) {
            continue;
        }
        let Ok(_lock) = store::lock_slot(tool, &item.id, Duration::from_secs(5)) else { continue };
        if pw::unseal_into_slot(&VaultSealer, &item).is_err() {
            continue;
        }
        let res = refresher.refresh(tool, &item.id);
        with_conn(|c| {
            match &res {
                Ok(()) => {
                    let _ = pw::set_refreshed(c, &item.id, now);
                }
                Err(err) if wallet::refresh_error_is_permanent(err) => {
                    let _ = pw::set_state(c, &item.id, state::NEEDS_LOGIN, Some("refresh failed"));
                }
                Err(_) => {}
            }
            // Whatever happened, nothing stays in plaintext.
            if let Err(e) = pw::seal_slot(c, &VaultSealer, &item) {
                k2_core::log_debug!("[wallet] keep-warm re-seal {}: {}", item.id, e.code());
            }
        });
    }
}

/// Delete one item for good: stop its sessions first. Audited.
pub fn delete_now(item: &Item, event: &str, by: &str, ingress: &str) -> Result<(), WalletError> {
    if let Some(l) = running_for(&item.id) {
        let mut map = lock_logins();
        if let Some(x) = map.get_mut(&l) {
            x.cancel = true;
        }
    }
    let stopped = stop_sessions(&item.id, None);
    if let Some(tool) = item.tool() {
        let _lock = store::lock_slot(tool, &item.id, Duration::from_secs(10))?;
        with_conn(|c| pw::delete_item(c, &VaultSealer, item))?;
    } else {
        with_conn(|c| pw::delete_item(c, &VaultSealer, item))?;
    }
    audit(event, by, &format!("{} {} {} stopped={stopped}", person_key(&item.principal_id), item.provider, item.id), ingress);
    Ok(())
}

/// Clear a principal's personal-agent marks, one audit line each.
fn clear_marks<'a>(pid: &str, marks: impl Iterator<Item = &'a pw::PersonAgent>, why: &str) {
    for m in marks {
        match with_conn(|c| pw::set_person_agent(c, &m.project_id, None, Some("sweep"))) {
            Ok(true) => audit(
                "wallet.personal_agent",
                "system",
                &format!("clear {} (was {} via {}; {why})", m.project_id, person_key(pid), m.service_id),
                "sweep",
            ),
            Ok(false) => {}
            Err(e) => k2_core::log_debug!("[wallet] clear mark {}: {}", m.project_id, e.code()),
        }
    }
}

/// Lifecycle (Rosson #4/#5). Returns `(dormant, woke, deleted, stopped)`
/// counts for tests and logs.
/// - principal gone (or an agent login): delete every item now, drop
///   their personal-agent marks;
/// - no enabled grant on any published App: dormant (sessions stopped);
///   access back: awake;
/// - dormant for 30 days: delete;
/// - a running session whose item may no longer run that agent: stopped.
pub fn lifecycle(now: i64) -> (usize, usize, usize, usize) {
    let (items, marks) = with_conn(|c| (pw::list_all(c).unwrap_or_default(), pw::person_agents(c).unwrap_or_default()));
    let mut principals: Vec<String> = items.iter().map(|i| i.principal_id.clone()).chain(marks.iter().map(|m| m.principal_id.clone())).collect();
    principals.sort();
    principals.dedup();
    let (mut dormant, mut woke, mut deleted, mut stopped) = (0, 0, 0, 0);
    for pid in principals {
        let person = match k2_core::skin::wallet_access::person(&pid) {
            Ok(p) => p,
            Err(e) => {
                // Can't read skin.db: change nothing (fail safe, not destructive).
                k2_core::log_debug!("[wallet] lifecycle: skin read failed: {e}");
                continue;
            }
        };
        let mine: Vec<&Item> = items.iter().filter(|i| i.principal_id == pid).collect();
        // Gone, or turned into an agent's own login: no longer a person, so
        // the wallet and the personal-agent marks both go (PRD §3.4).
        let p = match person {
            None => Err("person_gone"),
            Some(p) if p.is_agent_login => Err("now_an_agent_login"),
            Some(p) => Ok(p),
        };
        match p {
            Err(why) => {
                for i in &mine {
                    if delete_now(i, "wallet.expired", "system", "sweep").is_ok() {
                        deleted += 1;
                    }
                }
                clear_marks(&pid, marks.iter().filter(|m| m.principal_id == pid), why);
            }
            Ok(p) => {
                if !p.has_any_app() {
                    let n = with_conn(|c| pw::set_dormant(c, &pid, Some(now))).unwrap_or(0);
                    if n > 0 {
                        dormant += n;
                        audit("wallet.dormant", &person_key(&pid), &format!("{n} item(s)"), "sweep");
                    }
                    for i in &mine {
                        stopped += stop_sessions(&i.id, None);
                    }
                } else {
                    let n = with_conn(|c| pw::set_dormant(c, &pid, None)).unwrap_or(0);
                    woke += n;
                }
            }
        }
    }
    // Dormant for 30 days → deleted.
    let items = with_conn(|c| pw::list_all(c)).unwrap_or_default();
    for i in items.iter().filter(|i| i.dormant_since.is_some_and(|d| now - d >= pw::DORMANT_KEEP_SECS)) {
        if delete_now(i, "wallet.expired", "system", "sweep").is_ok() {
            deleted += 1;
        }
    }
    // Running sessions whose item may no longer run that agent.
    for (pid, item_id, project) in running_rows() {
        let (item, mark, settings) = with_conn(|c| {
            let item = pw::get(c, &item_id).ok().flatten();
            let mark = pw::person_agent(c, &project).ok().flatten();
            let svc = mark.as_ref().map(|m| m.service_id.clone()).unwrap_or_default();
            (item, mark, pw::app_settings(c, &svc).unwrap_or_default())
        });
        let principal = item.as_ref().map(|i| i.principal_id.clone()).unwrap_or_default();
        let facts = facts_for(&principal, Some(&project));
        // A sign-in that merely went stale keeps running (the CLI owns it).
        let usable = match decide::check_usable(item.as_ref(), mark.as_ref(), &facts, &settings) {
            Ok(()) | Err(decide::Unusable::NeedsLogin) => true,
            Err(_) => false,
        };
        if !usable {
            stop_pid(pid);
            running().lock().unwrap_or_else(|p| p.into_inner()).remove(&pid);
            audit("wallet.stopped", &person_key(&principal), &format!("{item_id} in {project}"), "sweep");
            stopped += 1;
        }
    }
    (dormant, woke, deleted, stopped)
}

/// Reconcile every item's `sealed` flag with the vault and the slot.
pub fn reconcile() -> usize {
    match with_conn(|c| pw::reconcile_sealed(c, &VaultSealer)) {
        Ok(n) => {
            if n > 0 {
                k2_core::log_debug!("[wallet] reconciled the sealed flag of {n} item(s)");
            }
            n
        }
        Err(e) => {
            k2_core::log_debug!("[wallet] reconcile: {}", e.code());
            0
        }
    }
}

/// The sweep loop: re-seal and lifecycle every minute, keep-warm every 30
/// minutes. `K2_LLM_ACCOUNTS_LOOP=0` turns it off (with the server loop).
pub fn spawn_loop() {
    if std::env::var("K2_LLM_ACCOUNTS_LOOP").map(|v| v == "0").unwrap_or(false) {
        return;
    }
    let _ = std::thread::Builder::new().name("person-wallet".into()).spawn(|| {
        let mut last_warm = Instant::now();
        // Boot: the `sealed` column follows the facts (vault item present,
        // no plaintext), whatever a crash mid-seal left behind.
        reconcile();
        loop {
            std::thread::sleep(SWEEP_EVERY);
            let _ = reseal_idle();
            reconcile();
            let _ = lifecycle(chrono::Utc::now().timestamp());
            if last_warm.elapsed() >= WARM_EVERY {
                keep_warm(&rt::DaemonRefresher);
                last_warm = Instant::now();
            }
        }
    });
}
