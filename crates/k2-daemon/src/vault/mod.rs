//! Daemon glue for the K2 vault (`prd-k2-vault-v1.md` §5.1).
//!
//! - [`boot`] opens `~/.k2/vault/vault.db` once, from the `serve` path,
//!   after the single-instance lock (VS14). Nothing else opens a KeyStore.
//! - [`get`] hands the open vault to routes and consumers.
//! - Key store selection (§7.2, KV23, KV26): macOS → Keychain via
//!   Security.framework (no file fallback); Linux → the root helper's
//!   keeper when it answers protocol ≥ 4, else systemd encrypted
//!   credentials when the unit provides them (Q24), else the file key.
//!   A better store showing up later is adopted in place (re-wrap, then
//!   the file key is removed). A test build (`k2_core::TEST_BUILD`) on
//!   Linux always takes the file key under its own home.

pub mod actor;
pub mod adopt;
pub mod human;
#[cfg(target_os = "macos")]
pub mod keychain;
pub mod keeper_client;
#[cfg(target_os = "macos")]
pub mod local_auth;
pub mod mainrefs;
pub mod publish_env;
pub mod refstore;
pub mod sealer;
pub mod server_refs;
pub mod spawn;
#[cfg(unix)]
pub mod ssh_agent;
/// The vault ssh-agent is a Unix socket server; on Windows a stand-in that
/// refuses `agent` delivery until the named-pipe agent lands (pending
/// Windows parity).
#[cfg(not(unix))]
#[path = "ssh_agent_windows.rs"]
pub mod ssh_agent;
pub mod systemd_creds;
pub mod tunnel;

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use k2_vault::keystore::file::FileKeyStore;
use k2_vault::{KeyStore, LocalVault, VaultError};
use parking_lot::RwLock;

/// The open vault, or why it isn't open.
static VAULT: OnceLock<RwLock<Result<Arc<LocalVault>, VaultError>>> = OnceLock::new();

fn cell() -> &'static RwLock<Result<Arc<LocalVault>, VaultError>> {
    VAULT.get_or_init(|| RwLock::new(Err(VaultError::new("vault_not_open", 503, "the vault isn't open yet"))))
}

/// The open vault.
pub fn get() -> Result<Arc<LocalVault>, VaultError> {
    cell().read().clone()
}

/// Was the daemon made non-dumpable when the vault opened (Q18)?
static NON_DUMPABLE: OnceLock<bool> = OnceLock::new();

pub fn non_dumpable() -> bool {
    NON_DUMPABLE.get().copied().unwrap_or(false)
}

pub fn vault_dir(home: &Path) -> PathBuf {
    home.join("vault")
}

pub fn file_key_dir(home: &Path) -> PathBuf {
    vault_dir(home).join("key")
}

/// True when `k2_home` is the account's own `~/.k2` (password-database
/// home, not `$HOME`) and no test harness marker is set.
#[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
fn keychain_home(k2_home: &Path, real_home: Option<&Path>) -> bool {
    if std::env::var_os("K2_TEST_AGENT_SHIM_DIR").is_some() {
        return false;
    }
    let Some(real) = real_home else { return false };
    let want = real.join(".k2");
    match (dunce::canonicalize(&k2_home), dunce::canonicalize(&want)) {
        (Ok(a), Ok(b)) => a == b,
        _ => k2_home == want,
    }
}

/// The key store this box should use now.
pub fn select_key_store(home: &Path) -> Arc<dyn KeyStore> {
    #[cfg(target_os = "macos")]
    {
        // The login Keychain belongs to the account's own K2 home. A daemon
        // started with another HOME (a test harness, a temp home) keeps its
        // box key in a file there instead, so it never reads or replaces
        // the account's real vault key.
        if keychain_home(home, k2_core::agent_hooks::install::passwd_home().as_deref()) {
            return Arc::new(keychain::KeychainKeyStore::new());
        }
        Arc::new(FileKeyStore::new(file_key_dir(home)))
    }
    // Windows: the account's own K2 home seals its box key with DPAPI (the
    // Keychain's role on macOS); any other home keeps a file there. A vault
    // made on the file key moves onto DPAPI at the next boot
    // (open_with_upgrade re-wraps it).
    #[cfg(windows)]
    {
        if keychain_home(home, dirs::home_dir().as_deref()) {
            return Arc::new(k2_vault::keystore::dpapi::DpapiKeyStore::new(file_key_dir(home)));
        }
        Arc::new(FileKeyStore::new(file_key_dir(home)))
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        // The root helper and the unit's systemd credential hold the BOX's
        // key: a test daemon (temp HOME) that took them failed `keep kek
        // init` on every vault test of the 2026-10-27 soak (k2-linux, K2
        // installed), or worse, would read the box's real key. A test build
        // keeps its key in a file under its own home, like a temp home on
        // macOS and Windows; the helper transport also refuses any call
        // from a test build (`keeper_client`).
        if k2_core::TEST_BUILD {
            return Arc::new(FileKeyStore::new(file_key_dir(home)));
        }
        let t = keeper_client::SudoHelper::detect();
        if let Some(t) = t {
            if k2_vault::keystore::keeper::probe(&t).is_some() {
                return Arc::new(k2_vault::keystore::keeper::KeeperKeyStore::new(t));
            }
        }
        if let Some(s) = systemd_creds::SystemdCredsKeyStore::detect() {
            return Arc::new(s);
        }
        Arc::new(FileKeyStore::new(file_key_dir(home)))
    }
}

/// Open the vault at boot. Called once from `async_main`, after the
/// single-instance lock. Failures leave the vault closed with a reason
/// that `/cli/vault/status` shows; the daemon keeps running.
pub fn boot(home: &Path) {
    let _ = BOOT_HOME.set(home.to_path_buf());
    let dir = vault_dir(home);
    let ks = select_key_store(home);
    let r = open_with_upgrade(&dir, home, ks);
    // Publish it first: the adoption steps below go through `get()`.
    *cell().write() = r.clone();
    match &r {
        Ok(v) => {
            let a = adopt::thread_secrets(v, &k2_core::overlay::vault::vault_root());
            if !a.moved.is_empty() || !a.errors.is_empty() {
                k2_core::log_debug!("[vault] thread-secrets: moved {} kept {} errors {:?}", a.moved.len(), a.kept.len(), a.errors);
            }
            record_adoption(a);
            for (path, engine, subsystem, orphans) in [
                (crate::mail::secrets::legacy_path(), "stalwart", "mail", true),
                (crate::sql::secrets::legacy_path(), "postgres", "db", false),
            ] {
                let a = adopt::json_store(v, &path, engine, subsystem, orphans);
                if !a.moved.is_empty() || !a.errors.is_empty() {
                    k2_core::log_debug!("[vault] {}: moved {} errors {:?}", a.store, a.moved.len(), a.errors);
                }
                // Q23 / VS9: credentials K2 generated get new values once
                // they're in the vault, so old plaintext copies go dead.
                if !a.moved.is_empty() && a.errors.is_empty() {
                    rotate_generated_after_move(subsystem);
                }
                record_adoption(a);
            }
            match tunnel::adopt(v) {
                Ok(moved) => record_adoption(adopt::Adopted {
                    store: "tunnel.json token".into(),
                    moved: if moved { vec![tunnel::NAME.into()] } else { vec![] },
                    ..Default::default()
                }),
                Err(e) => record_adoption(adopt::Adopted {
                    store: "tunnel.json token".into(),
                    errors: vec![e.hint],
                    ..Default::default()
                }),
            }
            let _ = k2_core::secret_refs::install(Box::new(mainrefs::Holder));
            let _ = k2_core::redaction::install(Box::new(|s: &str| redact_str(s)));
            record_adoption(mainrefs::adopt(v));
            spawn::sweep_at_boot();
            // prd-vault-gaps-v1 item 3: bind agent socket records to the
            // ptyd process each session is spawned in.
            #[cfg(unix)]
            k2_core::terminal::ptyd::set_spawned_hook(ssh_agent::on_ptyd_spawned);
            publish_env::seed_existing();
            // Non-dumpable before the R8 pass reads any value (round 2 fix 6).
            let nd = k2_vault::secret::set_non_dumpable();
            let _ = NON_DUMPABLE.set(nd);
            // R8 (prd-env-to-vault-v1): once, before any service starts.
            server_refs::upgrade_once(v);
            // VU-B1 (0.46.6): one owner notice for a backup key that was
            // added without a person's confirm. Never removed automatically.
            crate::vault_use_routes::notice_unconfirmed_holders(v);
            start_ticker(v.clone());
            k2_core::log_debug!(
                "[vault] open: key={} recovery={} non_dumpable={nd}",
                v.key_store().kind().as_str(),
                v.recovery_state().0
            );
        }
        Err(e) => {
            // BM-A12: the tunnel token is in here; say so, not "not paired".
            k2_core::tunnel::config::set_vault_closed(Some(e.code));
            k2_core::log_debug!("[vault] NOT open: {e}");
            let _ = HOME.set(home.to_path_buf());
            match after_failure(&e.code, 0) {
                RetryNext::Wait(_) => spawn_open_retry(home.to_path_buf()),
                RetryNext::StopCancelled => stopped_cancelled(),
                RetryNext::StopFatal => set_retry(RetryState::stopped("fatal", &e.code)),
            }
        }
    }
}

/// The daemon's K2 home, for an open asked for later (`POST /cli/vault/open`).
static HOME: OnceLock<PathBuf> = OnceLock::new();

/// Key-store failures that can clear on their own: a locked login
/// Keychain (a Mac mini reached only by SSH), a root helper that didn't
/// answer. A missing key or a substituted vault never clears by waiting,
/// so those are not retried.
pub fn retryable(code: &str) -> bool {
    matches!(code, "keychain_error" | "keeper_error")
}

/// A person said no to the prompt for the vault key (macOS
/// `errSecUserCanceled`). Asking again on a timer would put the same
/// dialog up over and over, so K2 stops asking.
pub fn user_cancelled(code: &str) -> bool {
    code == "user_cancelled"
}

/// The wait before retry number `tries` (1-based): 1, 2, 4, 8, 16 minutes,
/// then every 30 (prd-llm-logins-in-vault-v1 Q4, with a back-off so a
/// prompt can't come back every minute).
pub fn backoff(tries: u32) -> std::time::Duration {
    let mins = 1u64.checked_shl(tries.saturating_sub(1)).unwrap_or(u64::MAX).min(30);
    std::time::Duration::from_secs(mins * 60)
}

/// What the retry does after a failed open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetryNext {
    /// Wait this long, then try again.
    Wait(std::time::Duration),
    /// The person cancelled the prompt: stop, tell the owner once, and
    /// open only on an explicit action or the next daemon start.
    StopCancelled,
    /// Waiting can't fix it.
    StopFatal,
}

/// After the `tries`-th failed open (0 = the boot open).
pub fn after_failure(code: &str, tries: u32) -> RetryNext {
    if user_cancelled(code) {
        RetryNext::StopCancelled
    } else if retryable(code) {
        RetryNext::Wait(backoff(tries + 1))
    } else {
        RetryNext::StopFatal
    }
}

/// The retry's state, for `/cli/vault/status` while the vault is closed.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryState {
    /// `waiting`, `stopped`, or empty (no retry).
    pub state: String,
    /// `cancelled` or `fatal` when stopped.
    pub reason: Option<String>,
    pub tries: u32,
    pub next_in_secs: Option<u64>,
    pub last_code: Option<String>,
}

impl RetryState {
    fn stopped(reason: &str, code: &str) -> RetryState {
        RetryState { state: "stopped".into(), reason: Some(reason.into()), last_code: Some(code.into()), ..Default::default() }
    }
}

static RETRY: parking_lot::Mutex<Option<RetryState>> = parking_lot::Mutex::new(None);

fn set_retry(s: RetryState) {
    *RETRY.lock() = Some(s);
}

pub fn retry_state() -> Option<RetryState> {
    RETRY.lock().clone()
}

/// The one notice when a person cancelled the vault-key prompt.
pub const CANCELLED_TITLE: &str = "K2 couldn't open the vault; unlock it";
pub const CANCELLED_BODY: &str = "macOS asked for permission to read the K2 vault's key and the request was cancelled, so K2 stopped asking. \
Until the vault opens, vault secrets and saved LLM logins that only the vault holds can't be used (the server default login keeps working). \
To open it, run `k2 vault open` in a terminal on this Mac (allow the prompt), or restart K2.";

fn stopped_cancelled() {
    set_retry(RetryState::stopped("cancelled", "user_cancelled"));
    k2_core::log_debug!("[vault] the vault-key prompt was cancelled; K2 won't ask again until `k2 vault open` or the next start");
    file_owner_notice(CANCELLED_TITLE, CANCELLED_BODY);
}

fn file_owner_notice(title: &str, body: &str) {
    #[cfg(not(test))]
    if let Err(e) = crate::feedback_routes::file_owner_notice(k2_core::db::AUDIT_SENTINEL_IDS[0], "k2-vault", title, body) {
        k2_core::log_debug!("WARN [vault] owner notice '{title}': {e}");
    }
    #[cfg(test)]
    OWNER_NOTICES.lock().push(format!("{title}\n{body}"));
}

#[cfg(test)]
pub static OWNER_NOTICES: parking_lot::Mutex<Vec<String>> = parking_lot::Mutex::new(Vec::new());

/// The retry loop, with the open and the wait passed in (tests drive it
/// with a fake key store and no real sleep). Returns the open vault, or
/// `None` when it stopped.
pub fn run_open_retry(
    mut open: impl FnMut() -> Result<Arc<LocalVault>, VaultError>,
    mut wait: impl FnMut(std::time::Duration),
    already_open: impl Fn() -> bool,
    record: impl Fn(&VaultError),
) -> Option<Arc<LocalVault>> {
    let mut tries: u32 = 0;
    let mut next = backoff(1);
    loop {
        set_retry(RetryState { state: "waiting".into(), tries, next_in_secs: Some(next.as_secs()), ..Default::default() });
        wait(next);
        if already_open() {
            *RETRY.lock() = None;
            return None; // opened another way (`k2 vault open`, a restore)
        }
        tries += 1;
        match open() {
            Ok(v) => {
                *RETRY.lock() = None;
                k2_core::log_debug!("[vault] open after {tries} retr{}", if tries == 1 { "y" } else { "ies" });
                return Some(v);
            }
            Err(e) => {
                // Keep the newest reason for /cli/vault/status.
                record(&e);
                match after_failure(&e.code, tries) {
                    RetryNext::Wait(d) => {
                        if tries <= 5 || tries % 10 == 0 {
                            k2_core::log_debug!("[vault] still not open ({}), next try in {} min", e.code, d.as_secs() / 60);
                        }
                        next = d;
                    }
                    RetryNext::StopCancelled => {
                        stopped_cancelled();
                        return None;
                    }
                    RetryNext::StopFatal => {
                        k2_core::log_debug!("[vault] retry stopped: {e}");
                        set_retry(RetryState::stopped("fatal", &e.code));
                        return None;
                    }
                }
            }
        }
    }
}

/// Try to open the vault again with a back-off (1, 2, 4, 8, 16, then
/// every 30 minutes) until it opens, a person cancels the key prompt, or
/// it fails for a reason waiting can't fix. On success the vault is
/// served at once ([`get`]) with its runtime steps; the boot-only steps
/// (moving old stores in, the R8 upgrade, the secrets-dir sweep) wait for
/// the next daemon start, because services are already running.
fn spawn_open_retry(home: PathBuf) {
    let rt = tokio::runtime::Handle::try_current().ok();
    let _ = std::thread::Builder::new().name("vault-open-retry".into()).spawn(move || {
        let _enter = rt.as_ref().map(|h| h.enter());
        let opened = run_open_retry(
            || open_with_upgrade(&vault_dir(&home), &home, select_key_store(&home)),
            std::thread::sleep,
            || get().is_ok(),
            |e| {
                let mut c = cell().write();
                if c.is_err() {
                    *c = Err(e.clone());
                }
            },
        );
        if let Some(v) = opened {
            publish_late(v);
        }
    });
}

/// `POST /cli/vault/open`: a person asked K2 to open the vault now (it may
/// show the key prompt once). Also the way back after a cancelled prompt.
pub fn open_now() -> Result<Arc<LocalVault>, VaultError> {
    if let Ok(v) = get() {
        return Ok(v);
    }
    let home = HOME.get().cloned().ok_or_else(|| VaultError::new("vault_not_open", 503, "the vault isn't open yet"))?;
    match open_with_upgrade(&vault_dir(&home), &home, select_key_store(&home)) {
        Ok(v) => {
            *RETRY.lock() = None;
            publish_late(v.clone());
            Ok(v)
        }
        Err(e) => {
            *cell().write() = Err(e.clone());
            if user_cancelled(&e.code) {
                set_retry(RetryState::stopped("cancelled", &e.code));
            }
            Err(e)
        }
    }
}

/// Serve a vault that opened after boot and start its runtime steps,
/// once (the retry and `k2 vault open` can race).
fn publish_late(v: Arc<LocalVault>) {
    let mut c = cell().write();
    if c.is_ok() {
        return;
    }
    *c = Ok(v.clone());
    drop(c);
    if !LATE_DONE.swap(true, std::sync::atomic::Ordering::SeqCst) {
        on_late_open(&v);
    }
}

/// The late-open runtime steps ran once (a retry, `k2 vault open` and a
/// box-move import into a closed vault can race).
static LATE_DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The runtime half of a boot open, for a vault that opened late.
fn on_late_open(v: &Arc<LocalVault>) {
    // The tunnel token lives in the vault (BM-A12): its holder comes with it.
    if let Err(e) = tunnel::adopt(v) {
        k2_core::log_debug!("[vault] tunnel token after a late open: {}", e.code);
    }
    let _ = k2_core::secret_refs::install(Box::new(mainrefs::Holder));
    let _ = k2_core::redaction::install(Box::new(|s: &str| redact_str(s)));
    #[cfg(unix)]
    k2_core::terminal::ptyd::set_spawned_hook(ssh_agent::on_ptyd_spawned);
    let nd = k2_vault::secret::set_non_dumpable();
    let _ = NON_DUMPABLE.set(nd);
    start_ticker(v.clone());
    // Saved LLM logins: what the closed vault couldn't take is sealed now.
    k2_core::llm_accounts::slots::forget_known();
    let _ = std::thread::Builder::new()
        .name("vault-late-llm".into())
        .spawn(|| {
            let _ = crate::llm_accounts_runtime::seal_pass("vault opened");
        });
    k2_core::log_debug!(
        "[vault] open (late): key={} recovery={} non_dumpable={nd}; moving old stores in waits for the next start",
        v.key_store().kind().as_str(),
        v.recovery_state().0
    );
}

/// The K2 home `boot` opened the vault from (`~/.k2`).
static BOOT_HOME: OnceLock<PathBuf> = OnceLock::new();

/// The K2 home of this daemon's vault (`None` before boot, and in tests
/// that never boot).
pub fn boot_home() -> Option<PathBuf> {
    BOOT_HOME.get().cloned()
}

// ── box moves (prd-box-move-carry-v1) ───────────────────────────────
//
// The routes live in `migrate_secrets_routes`; the key store is opened
// here only (VS14), whether the vault is open or not (BM-A5).

/// The import with `--apply`: adopt `mf` into this box's vault folder with
/// this box's key store, then serve it (ticker, tunnel token holder).
/// Works while the vault is closed. On a failure the old files are back
/// and the old vault (if one was open) keeps serving.
pub fn move_import_apply(
    home: &Path,
    identity: &k2_vault::agefmt::Identity,
    mf: &k2_vault::boxmove::MoveFile,
) -> Result<(Arc<LocalVault>, k2_vault::boxmove::ImportRecord), VaultError> {
    let dir = vault_dir(home);
    let ks = select_key_store(home);
    let imported = k2_vault::boxmove::apply_move(&dir, ks, identity, mf, k2_vault::OpenOptions::default())?;
    let v = Arc::new(imported.vault);
    serve_replaced(&v);
    Ok((v, imported.record))
}

/// `--undo` on the new box: put the replaced vault back and open it again
/// (closed, as before, when it didn't open then). A store that only
/// counts up keeps the import's higher `gen`; that rollback is
/// acknowledged with the undo's own confirm (`proof`).
pub fn move_import_undo(home: &Path, actor: &k2_vault::Actor, proof: Option<&k2_vault::HumanProof>) -> Result<k2_vault::boxmove::ImportRecord, VaultError> {
    let dir = vault_dir(home);
    let ks = select_key_store(home);
    let rec = k2_vault::boxmove::undo_import(&dir, &ks)?;
    match LocalVault::open(&dir, ks, k2_vault::OpenOptions::default()) {
        Ok(v) => {
            if v.in_rollback() {
                if let Some(p) = proof {
                    v.acknowledge_rollback(actor, p)?;
                }
            }
            serve_replaced(&Arc::new(v));
        }
        Err(e) => {
            k2_core::tunnel::config::set_vault_closed(Some(e.code));
            *cell().write() = Err(e);
        }
    }
    Ok(rec)
}

/// A vault replaced the served one (restore, a box-move import or undo):
/// serve it, install the tunnel token holder if the box booted closed,
/// and give it its own ticker (the old one stops at its next tick).
fn serve_replaced(v: &Arc<LocalVault>) {
    let was_closed = {
        let mut c = cell().write();
        let was = c.is_err();
        *c = Ok(v.clone());
        was
    };
    // Only a daemon that booted its vault: tests swap vaults in a temp
    // dir and must never touch a tunnel.json.
    if boot_home().is_none() {
        return;
    }
    // A box that booted closed (an import onto vault_key_missing) gets the
    // runtime steps a late open gets; an open one just its new ticker.
    if was_closed && !LATE_DONE.swap(true, std::sync::atomic::Ordering::SeqCst) {
        on_late_open(v);
        return;
    }
    if let Err(e) = tunnel::adopt(v) {
        k2_core::log_debug!("[vault] tunnel token after replace: {}", e.code);
    }
    let _ = k2_core::secret_refs::install(Box::new(mainrefs::Holder));
    let _ = k2_core::redaction::install(Box::new(|s: &str| redact_str(s)));
    start_ticker(v.clone());
}

fn rotate_generated_after_move(subsystem: &'static str) {
    let _ = std::thread::Builder::new().name(format!("vault-rotate-{subsystem}")).spawn(move || {
        // Let the engines come up first.
        std::thread::sleep(std::time::Duration::from_secs(60));
        match subsystem {
            "db" if crate::sql::sql_supported() => {
                for line in crate::sql::routes::rotate_after_vault_adoption() {
                    k2_core::log_debug!("[vault] db rotate: {line}");
                }
            }
            "mail" if crate::mail::supervisor::mail_supported() => match crate::mail::supervisor::rotate_leftover_admins_live() {
                Ok(r) => k2_core::log_debug!("[vault] mail admin rotate: {r:?}"),
                Err(e) => k2_core::log_debug!("[vault] mail admin rotate skipped: {e}"),
            },
            _ => {}
        }
    });
}

/// A restore swapped the vault file: serve the new one.
pub fn replace(v: Arc<LocalVault>) {
    serve_replaced(&v);
}

/// Every minute: flush audit roll-ups, expire chat secrets and old card
/// requests, drop versions past retention, and remove secrets dirs of
/// sessions that are gone.
fn start_ticker(v: Arc<LocalVault>) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else { return };
    handle.spawn(async move {
        let mut t = tokio::time::interval(std::time::Duration::from_secs(60));
        t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            t.tick().await;
            // A restore or box-move import replaced this vault: its own
            // ticker serves the new one.
            if !matches!(get(), Ok(cur) if Arc::ptr_eq(&cur, &v)) {
                break;
            }
            let v2 = v.clone();
            let _ = tokio::task::spawn_blocking(move || {
                match v2.tick() {
                    // Their Thread cards say so (the overlay sweep may
                    // already have closed them: that is a no-op).
                    Ok(expired) => {
                        for r in expired {
                            crate::overlay_routes::close_expired_card(&r.conversation, &r.card_id);
                        }
                    }
                    Err(e) => k2_core::log_debug!("[vault] tick: {}", e.code),
                }
                // Secret handoffs: pending copies past 7 days, --ttl copies (H13).
                crate::vault_handoff_routes::tick(&v2);
                // One liveness answer for every per-session sweep: the map,
                // sessions waiting for k2-ptyd adoption (this first tick
                // fires before boot's B1), and sessions still spawning.
                let live = crate::session_liveness::live_session_ids();
                spawn::sweep_dead(&|s: &str| live.contains(s));
                // Papercut 8: a file granted to a live session (by any path:
                // a grant, a set, a card the owner approved) reaches its
                // $K2_SECRETS_DIR within a minute.
                spawn::top_up_all(&v2);
            })
            .await;
        }
    });
}

/// Open with `ks`; when `ks` isn't the file store and the vault was made
/// on the file key, move it in place: re-wrap to the new key, anchor, then
/// remove the file key (§7.2 "Upgrades in place").
fn open_with_upgrade(dir: &Path, home: &Path, ks: Arc<dyn KeyStore>) -> Result<Arc<LocalVault>, VaultError> {
    let opts = k2_vault::OpenOptions::default();
    let file = Arc::new(FileKeyStore::new(file_key_dir(home)));
    let is_file = ks.kind() == k2_vault::KeyHome::File;
    let file_kek = if is_file { None } else { file.load().ok().flatten() };
    let vault_exists = dir.join("vault.db").exists();
    if let (Some(fk), true) = (file_kek, vault_exists) {
        match ks.load()? {
            // Same bytes (e.g. the file key sealed as a systemd credential):
            // nothing to re-wrap; the file copy just goes.
            Some(k) if k == fk => {
                let v = LocalVault::open(dir, ks.clone(), opts)?;
                v.reanchor();
                file.remove()?;
                return Ok(Arc::new(v));
            }
            other => {
                // The vault lives on the file key: re-wrap to the better
                // store's key (made now if it has none), verify by
                // reopening, then remove the file key.
                let v = LocalVault::open(dir, file.clone() as Arc<dyn KeyStore>, opts.clone())?;
                let new_kek = match other {
                    Some(k) => k,
                    None => ks.init()?,
                };
                v.rewrap_box(&new_kek, "system:vault", "rekey-store")?;
                drop(v);
                let v2 = LocalVault::open(dir, ks.clone(), opts)?;
                v2.reanchor();
                file.remove()?;
                return Ok(Arc::new(v2));
            }
        }
    }
    Ok(Arc::new(LocalVault::open(dir, ks, opts)?))
}

static ADOPTED: parking_lot::Mutex<Vec<adopt::Adopted>> = parking_lot::Mutex::new(Vec::new());

fn record_adoption(a: adopt::Adopted) {
    let mut g = ADOPTED.lock();
    g.retain(|x| x.store != a.store);
    g.push(a);
}

/// Which old stores moved into the vault (status `adopted`).
pub fn adopted() -> Vec<adopt::Adopted> {
    ADOPTED.lock().clone()
}

/// One vault for the whole test binary (temp dir, fake key store),
/// installed as the daemon's vault. Tests never touch a real key store.
#[cfg(test)]
pub fn ensure_test_vault() -> Arc<LocalVault> {
    static TEST_VAULT: OnceLock<(k2_vault::fsutil::TestDir, Arc<LocalVault>)> = OnceLock::new();
    let (_, v) = TEST_VAULT.get_or_init(|| {
        let dir = k2_vault::fsutil::TestDir::new("daemon-vault");
        let ks: Arc<dyn KeyStore> = Arc::new(k2_vault::keystore::fake::FakeKeyStore::new());
        let v = Arc::new(
            LocalVault::open(dir.path().join("vault"), ks, k2_vault::OpenOptions { kdf: k2_vault::confirm::KdfCost::Floor })
                .expect("open test vault"),
        );
        (dir, v)
    });
    *cell().write() = Ok(v.clone());
    let _ = k2_core::redaction::install(Box::new(|s: &str| redact_str(s)));
    v.clone()
}

// ── workspace helpers ───────────────────────────────────────────────

/// Workspace token (name, handle, path or id) → projects.id.
pub fn workspace_id(token: &str) -> Option<String> {
    let t = token.trim();
    if t.is_empty() {
        return None;
    }
    let path = crate::workspace_msg::resolve_workspace(t)?;
    crate::canonical_session::lookup_project_id(&path)
}

/// projects.id → the handle people see (`sales`).
pub fn workspace_label(id: &str) -> String {
    k2_core::workspace_session_handles::workspace_address_name_shared(id).unwrap_or_else(|_| id.to_string())
}

/// projects.id → workspace folder.
pub fn workspace_path(id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT path FROM projects WHERE id = ?1", rusqlite::params![id], |r| r.get::<_, String>(0)).ok()
}

/// A `k2v://` reference as people write it (workspace handle, not id).
pub fn display_ref(meta: &k2_vault::SecretMeta) -> Option<String> {
    use k2_vault::ScopeKind;
    if meta.namespace != k2_vault::Namespace::User {
        return None;
    }
    let scope_ref = match meta.scope.kind {
        ScopeKind::Workspace => workspace_label(&meta.scope.id),
        ScopeKind::Server => String::new(),
        _ => meta.scope.id.clone(),
    };
    Some(
        k2_vault::refs::VaultRef { scope_kind: meta.scope.kind, scope_ref, name: meta.name.clone(), version: None }
            .render(),
    )
}

/// Parse a `k2v://` reference into a selector (workspace names → ids).
pub fn selector_for_ref(s: &str) -> Result<k2_vault::Selector, VaultError> {
    use k2_vault::{Scope, ScopeKind, Selector};
    let r = k2_vault::refs::VaultRef::parse(s)?;
    let scope = match r.scope_kind {
        ScopeKind::Server => Scope::server(),
        ScopeKind::Workspace => {
            let id = workspace_id(&r.scope_ref)
                .ok_or_else(|| VaultError::not_found(format!("no workspace named {}", r.scope_ref)))?;
            Scope::workspace(id)
        }
        ScopeKind::Session => Scope::session(r.scope_ref.clone()),
        _ => return Err(VaultError::bad_request("scope_reserved", "that scope isn't available yet")),
    };
    Ok(Selector::user(scope, r.name))
}

/// The redactor for K2's own surfaces (KV33). Empty when the vault is
/// closed.
pub fn redactor() -> Arc<k2_vault::redact::Redactor> {
    match get() {
        Ok(v) => v.redactor(),
        Err(_) => Arc::new(k2_vault::redact::Redactor::empty()),
    }
}

/// Redact a string for a K2 surface.
pub fn redact_str(s: &str) -> String {
    let r = redactor();
    if r.is_empty() {
        s.to_string()
    } else {
        r.redact_str(s)
    }
}

/// `auth-audit.jsonl` line for reveal-class actions and their denials
/// (KV32), so `k2 users audit` / Settings → Logs show them.
pub fn auth_audit(event: &str, user: &str, outcome: &str, ingress: &str) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        event,
        user,
        outcome.to_string(),
        ingress,
        "-",
        "vault",
    ));
}

#[cfg(test)]
mod upgrade_tests {
    use std::sync::Arc;

    use k2_vault::keystore::fake::FakeKeyStore;
    use k2_vault::keystore::file::FileKeyStore;
    use k2_vault::{Actor, KeyStore, Kind, Namespace, PutRequest, Requester, Scope, SecretBytes, SecretStore, Selector, Use};

    /// §7.2 "Upgrades in place": a vault on the file key moves to a better
    /// store (here the fake standing in for the keeper/Keychain); values
    /// survive; the file key is removed only after the reopen verified.
    #[test]
    fn file_key_vault_moves_to_a_better_store_in_place() {
        let t = k2_vault::fsutil::TestDir::new("upgrade");
        let home = t.path().to_path_buf();
        let dir = super::vault_dir(&home);
        {
            let file: Arc<dyn KeyStore> = Arc::new(FileKeyStore::new(super::file_key_dir(&home)));
            let v = k2_vault::LocalVault::open(&dir, file, k2_vault::OpenOptions { kdf: k2_vault::confirm::KdfCost::Floor }).expect("open on file key");
            let mut r = PutRequest::new(Namespace::System, Scope::engine("x"), "K", Kind::Password);
            r.default_delivery = Some(k2_vault::Delivery::None);
            v.put(r, SecretBytes::from_slice(b"survives"), &Actor::System("t".into())).expect("put");
        }
        assert!(super::file_key_dir(&home).join("kek").exists());
        let better = Arc::new(FakeKeyStore::new());
        let v = super::open_with_upgrade(&dir, &home, better.clone() as Arc<dyn KeyStore>).expect("upgrade");
        assert!(!super::file_key_dir(&home).join("kek").exists(), "file key removed after the move");
        assert!(better.has_kek());
        let got = v
            .resolve(&Selector::system(Scope::engine("x"), "K"), Use::System, &Requester::System("t".into()))
            .expect("value survives");
        assert_eq!(got.value.expose(), b"survives");
        assert_eq!(v.key_store().kind(), k2_vault::KeyHome::Fake);
    }
}

#[cfg(test)]
mod tests {
    // ── The boot open retry: back-off, and a cancelled prompt stops it ──

    /// The retry state is process-wide: these tests take turns.
    static RETRY_TESTS: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

    /// A root helper whose every call fails with `code`.
    struct FakeKeeper(&'static str);
    impl k2_vault::keystore::keeper::KeeperTransport for FakeKeeper {
        fn call(&self, _args: &[&str]) -> Result<Vec<u8>, k2_vault::VaultError> {
            Err(k2_vault::VaultError::new(self.0, 500, "fake keeper"))
        }
    }

    #[test]
    fn the_open_retry_backs_off_to_thirty_minutes() {
        let mins: Vec<u64> = (1..=8).map(|t| super::backoff(t).as_secs() / 60).collect();
        assert_eq!(mins, vec![1, 2, 4, 8, 16, 30, 30, 30]);
        assert_eq!(super::backoff(200).as_secs(), 30 * 60, "no overflow");
        assert_eq!(super::after_failure("keychain_error", 0), super::RetryNext::Wait(std::time::Duration::from_secs(60)));
        assert_eq!(super::after_failure("keeper_error", 3), super::RetryNext::Wait(std::time::Duration::from_secs(8 * 60)));
        assert_eq!(super::after_failure("user_cancelled", 0), super::RetryNext::StopCancelled);
        assert_eq!(super::after_failure("vault_key_missing", 2), super::RetryNext::StopFatal);
        assert_eq!(super::after_failure("vault_substituted", 0), super::RetryNext::StopFatal);
    }

    #[test]
    fn a_cancelled_key_prompt_stops_the_retry_and_tells_the_owner_once() {
        let _turn = RETRY_TESTS.lock();
        let dir = k2_vault::fsutil::TestDir::new("vault-open-retry");
        let codes = ["keeper_error", "keeper_error", "user_cancelled", "keeper_error", "keeper_error"];
        let opened = std::sync::atomic::AtomicUsize::new(0);
        let waits = std::cell::RefCell::new(Vec::new());
        let recorded = std::cell::RefCell::new(Vec::new());
        let before = super::OWNER_NOTICES.lock().iter().filter(|n| n.starts_with(super::CANCELLED_TITLE)).count();
        let out = super::run_open_retry(
            || {
                let code = codes[opened.fetch_add(1, std::sync::atomic::Ordering::SeqCst)];
                let ks: std::sync::Arc<dyn k2_vault::KeyStore> =
                    std::sync::Arc::new(k2_vault::keystore::keeper::KeeperKeyStore::new(FakeKeeper(code)));
                k2_vault::LocalVault::open(dir.path().join("vault"), ks, k2_vault::OpenOptions { kdf: k2_vault::confirm::KdfCost::Floor })
                    .map(std::sync::Arc::new)
            },
            |d| waits.borrow_mut().push(d.as_secs() / 60),
            || false,
            |e| recorded.borrow_mut().push(e.code.to_string()),
        );
        assert!(out.is_none(), "never opened");
        assert_eq!(opened.load(std::sync::atomic::Ordering::SeqCst), 3, "no try after the person cancelled the prompt");
        assert_eq!(*waits.borrow(), vec![1, 2, 4], "1, 2, 4 minutes: backing off, never every minute");
        assert_eq!(*recorded.borrow(), vec!["keeper_error", "keeper_error", "user_cancelled"]);
        let notices: Vec<String> =
            super::OWNER_NOTICES.lock().iter().filter(|n| n.starts_with(super::CANCELLED_TITLE)).cloned().collect();
        assert_eq!(notices.len(), before + 1, "one notice on the owner's Thread");
        assert!(notices.last().unwrap().contains("k2 vault open"), "it says how to open it");
        let st = super::retry_state().expect("a retry state");
        assert_eq!((st.state.as_str(), st.reason.as_deref()), ("stopped", Some("cancelled")));
    }

    #[test]
    fn the_retry_ends_quietly_when_the_vault_opened_another_way() {
        let _turn = RETRY_TESTS.lock();
        let tries = std::sync::atomic::AtomicUsize::new(0);
        let out = super::run_open_retry(
            || {
                tries.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err(k2_vault::VaultError::new("keychain_error", 500, "locked"))
            },
            |_| {},
            || true,
            |_| panic!("nothing to record"),
        );
        assert!(out.is_none());
        assert_eq!(tries.load(std::sync::atomic::Ordering::SeqCst), 0, "no open after `k2 vault open` won");
    }

    /// A daemon on another HOME (tests, temp homes) never uses the
    /// account's login Keychain; the account's own ~/.k2 does (outside a
    /// test harness).
    #[test]
    fn only_the_accounts_own_home_uses_the_keychain() {
        let d = k2_vault::fsutil::TestDir::new("kc-home");
        let real = d.path().join("real");
        std::fs::create_dir_all(real.join(".k2")).expect("mkdir");
        let other = d.path().join("other").join(".k2");
        std::fs::create_dir_all(&other).expect("mkdir");
        assert!(!super::keychain_home(&other, Some(&real)));
        assert!(!super::keychain_home(&real.join(".k2"), None));
        let marked = std::env::var_os("K2_TEST_AGENT_SHIM_DIR").is_some();
        assert_eq!(super::keychain_home(&real.join(".k2"), Some(&real)), !marked);
    }

    /// KV33: text K2 stores (here a ticket comment and an inbox note) never
    /// keeps a known secret value.
    #[test]
    fn k2_surfaces_store_known_values_redacted() {
        use k2_vault::{Actor, Kind, Namespace, PutRequest, Scope, SecretBytes, SecretStore as _};
        let v = super::ensure_test_vault();
        let val = format!("k2test_SURFACE_SENTINEL_{}", uuid::Uuid::new_v4().simple());
        let mut r = PutRequest::new(Namespace::User, Scope::server(), "GH_TOKEN_SURFACE", Kind::ApiKey);
        r.hosts = vec!["api.github.com".into()];
        v.put(r, SecretBytes::from_slice(val.as_bytes()), &Actor::Owner).expect("put");
        let out = k2_core::redaction::redact(&format!("token is {val} ok"));
        assert_eq!(out, "token is [k2v:GH_TOKEN_SURFACE] ok");
        let dir = k2_vault::fsutil::TestDir::new("inbox-redact");
        let item = k2_core::inbox::compose(dir.path(), "note", &format!("here: {val}"), None, None, None).expect("compose");
        let text = std::fs::read_to_string(dir.path().join(".k2/inbox").join(&item.filename)).expect("read");
        assert!(!text.contains(&val), "{text}");
        assert!(text.contains("[k2v:GH_TOKEN_SURFACE]"));
    }

    /// VS14: only the serve path opens the KeyStore. Every call site of
    /// `LocalVault::open` / `load_or_init` in the daemon is in this file.
    #[test]
    fn only_boot_opens_the_key_store() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = vec![];
        let mut stack = vec![root.clone()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).expect("read src dir") {
                let p = e.expect("entry").path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if p.extension().and_then(|x| x.to_str()) != Some("rs") {
                    continue;
                }
                // '/' separators on every OS (the exemptions below use them).
                let rel = p.strip_prefix(&root).expect("rel").to_string_lossy().replace('\\', "/");
                if rel == "vault/mod.rs" || rel.ends_with("tests.rs") || rel.ends_with("/tests") {
                    continue;
                }
                let src = std::fs::read_to_string(&p).expect("read");
                // Test modules may open their own temp vaults.
                let prod = src.split("#[cfg(test)]").next().unwrap_or("");
                for needle in ["LocalVault::open(", ".load_or_init(", "LocalVault::adopt_with_identity("] {
                    if prod.contains(needle) {
                        offenders.push(format!("{rel}: {needle}"));
                    }
                }
            }
        }
        assert!(offenders.is_empty(), "only vault/mod.rs may open the key store: {offenders:?}");
    }

    /// Soak 2026-10-27 (k2-linux, a box with K2 installed): every test
    /// daemon that opened the vault took the BOX's key home (the installed
    /// `k2-root-helper`) and failed `keep kek init`. A test build keeps the
    /// key in a file under the test's own home, whatever the box offers:
    /// here a systemd credential the box's unit handed down
    /// (`$CREDENTIALS_DIRECTORY/k2-vault-kek`). The root helper half can't
    /// be staged without root (its paths are fixed under /usr); the same
    /// rule covers it, and the transport refuses any call from a test build
    /// (`keeper_client::tests`).
    #[cfg(target_os = "linux")]
    #[test]
    fn a_test_build_never_takes_the_boxs_key_home() {
        let home = k2_core::test_env::TempHome::new();
        let creds = home.path().join("creds");
        std::fs::create_dir_all(&creds).expect("mkdir creds");
        std::fs::write(creds.join(super::systemd_creds::CREDENTIAL_NAME), [7u8; 32]).expect("write credential");
        let _c = k2_core::test_env::EnvVar::set("CREDENTIALS_DIRECTORY", &creds);
        let ks = super::select_key_store(&home.path().join(".k2"));
        assert_eq!(ks.kind(), k2_vault::KeyHome::File, "a test daemon took the box's key home");
    }
}
