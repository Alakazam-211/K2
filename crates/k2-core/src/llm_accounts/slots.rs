//! Where a server's saved logins are kept: the file store (today's
//! `~/.k2/llm-accounts/<tool>/<id>/` slots, [`store`]) or the K2 vault
//! (prd-llm-logins-in-vault-v1).
//!
//! **0.46.6 (Rosson 2026-10-27): both, with a switch.** The switch
//! ([`storage`]: `llmAccountsStorage` in the app settings, overridden by
//! `K2_LLM_ACCOUNTS_STORAGE=files|vault`) picks which store is the source
//! of truth. Default `files` everywhere; `vault` on z3mbp, one Linux box
//! and the test fleet.
//! - **files**: today's code, unchanged ([`FileStore`]). The vault is
//!   never touched.
//! - **vault** ([`VaultStore`]): every write goes to the slot file (as
//!   today) AND is sealed in the vault, read back and compared. Reads
//!   come from the vault whenever the slot has no whole credential. The
//!   plain files stay in place (removed logins included), so switching
//!   back to files is instant and loses nothing. Nothing is wiped
//!   automatically; that is a later, explicit step.
//!
//! **Plaintext is newest (A13).** K2 always writes the slot file first and
//! the vault second, and only a CLI writes a slot file on its own (a
//! pinned session's refresh). So when the file and the vault disagree,
//! the file wins and is sealed as a new version (old versions stay 30
//! days). A file that isn't a whole credential is never sealed over a
//! good vault copy.
//!
//! **Vault address (A1).** Namespace `system`, scope `server`, name
//! `llm-login.<tool>.<id>`, kind `login_slot` or `api_key`, delivery and
//! ceiling `none`. No person or agent ever reads a value; never copied to
//! another server (clone carries `user` only and never `login_slot`).
//!
//! **No new migration (A12).** Whether a login is sealed is worked out
//! from the facts (is there a vault item, does it match the file).
//!
//! The person wallet (`pw_*` slots) keeps its own sealing and reads its
//! slots through [`store`] directly; every server-login read goes through
//! here (a test fails on a direct `store::read_slot` elsewhere).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

use serde::Serialize;

use super::store::{self, CredMeta};
use super::{Entry, Tool, WalletError};
use crate::sealed_slots::{self, SealAddr, SealKind, SealScope, SealedState, SecretSealer};

// ── The switch ──────────────────────────────────────────────────────

/// Which store is the source of truth for saved logins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Storage {
    Files,
    Vault,
}

impl Storage {
    pub fn as_str(self) -> &'static str {
        match self {
            Storage::Files => "files",
            Storage::Vault => "vault",
        }
    }
    pub fn parse(s: &str) -> Option<Storage> {
        match s.trim().to_ascii_lowercase().as_str() {
            "files" | "file" => Some(Storage::Files),
            "vault" => Some(Storage::Vault),
            _ => None,
        }
    }
}

/// The field escape: `K2_LLM_ACCOUNTS_STORAGE=files|vault` wins over the
/// setting (the `K2_LLM_LOGIN_LIVE` pattern).
pub const ENV: &str = "K2_LLM_ACCOUNTS_STORAGE";

/// The setting's wire name in the app settings file.
pub const SETTING_KEY: &str = "llmAccountsStorage";

/// The default while the vault proves itself (0.46.6). 0.46.7 flips it.
pub const DEFAULT: Storage = Storage::Files;

/// 0 = unset, 1 = files, 2 = vault. The daemon mirrors the persisted
/// setting here at boot and on every change.
static SETTING: AtomicU8 = AtomicU8::new(0);

pub fn set_setting(s: Option<Storage>) {
    SETTING.store(
        match s {
            None => 0,
            Some(Storage::Files) => 1,
            Some(Storage::Vault) => 2,
        },
        Ordering::SeqCst,
    );
}

pub fn setting() -> Option<Storage> {
    match SETTING.load(Ordering::SeqCst) {
        1 => Some(Storage::Files),
        2 => Some(Storage::Vault),
        _ => None,
    }
}

/// The env override, when set to a known value.
pub fn env_override() -> Option<Storage> {
    std::env::var(ENV).ok().and_then(|v| Storage::parse(&v))
}

/// The store in effect now.
pub fn storage() -> Storage {
    env_override().or_else(setting).unwrap_or(DEFAULT)
}

/// Where [`storage`] came from: `env`, `setting` or `default`.
pub fn storage_source() -> &'static str {
    if env_override().is_some() {
        "env"
    } else if setting().is_some() {
        "setting"
    } else {
        "default"
    }
}

// ── The vault holder ────────────────────────────────────────────────

static SEALER: OnceLock<RwLock<Option<Arc<dyn SecretSealer>>>> = OnceLock::new();

fn sealer_cell() -> &'static RwLock<Option<Arc<dyn SecretSealer>>> {
    SEALER.get_or_init(|| RwLock::new(None))
}

/// The daemon installs the vault at boot (whether or not it opened: the
/// sealer answers `Err` while it is closed).
pub fn install_sealer(s: Arc<dyn SecretSealer>) {
    *sealer_cell().write().unwrap_or_else(|p| p.into_inner()) = Some(s);
    forget_known();
}

#[cfg(any(test, feature = "test-util"))]
pub fn uninstall_sealer() {
    *sealer_cell().write().unwrap_or_else(|p| p.into_inner()) = None;
    forget_known();
}

fn sealer() -> Result<Arc<dyn SecretSealer>, String> {
    sealer_cell()
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
        .ok_or_else(|| "vault: vault_not_open".to_string())
}

/// `llm-login.<tool>.<id>`.
pub fn vault_name(tool: Tool, id: &str) -> String {
    format!("llm-login.{}.{}", tool.as_str(), id)
}

pub fn addr(tool: Tool, id: &str) -> SealAddr {
    SealAddr { scope: SealScope::Server, name: vault_name(tool, id) }
}

fn unavailable(e: &str) -> WalletError {
    WalletError::VaultUnavailable(format!(
        "the K2 vault isn't open ({}), so this saved login can't be read; the server default keeps working. Check Settings → Vault",
        e.trim_start_matches("vault: ")
    ))
}

// ── What the vault is known to hold ─────────────────────────────────

/// name → fingerprint of the value the vault is known to hold, active.
/// Saves a vault read (a decrypt) on every slot read. Only this module
/// writes these items, so the cache is exact until a vault restore
/// ([`forget_known`]).
fn known() -> &'static Mutex<HashMap<String, [u8; 32]>> {
    static K: OnceLock<Mutex<HashMap<String, [u8; 32]>>> = OnceLock::new();
    K.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Forget what the vault holds (a vault restore swapped the file).
pub fn forget_known() {
    known().lock().unwrap_or_else(|p| p.into_inner()).clear();
}

fn fp(b: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(b).into()
}

fn note_known(name: &str, f: Option<[u8; 32]>) {
    let mut k = known().lock().unwrap_or_else(|p| p.into_inner());
    match f {
        Some(f) => {
            k.insert(name.to_string(), f);
        }
        None => {
            k.remove(name);
        }
    }
}

fn is_known(name: &str, f: &[u8; 32]) -> bool {
    known().lock().unwrap_or_else(|p| p.into_inner()).get(name) == Some(f)
}

/// What one sync of a plaintext value into the vault did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Synced {
    /// The vault already held these bytes.
    Unchanged,
    /// Sealed as a new version, read back and matched.
    Sealed,
    /// The vault is closed or the read-back didn't match: the plaintext
    /// is kept and sealed on a later pass ("waiting to seal").
    Waiting(String),
}

/// Make the vault hold `bytes` for this login (plaintext is newest).
/// Never logs or returns a value.
fn sync_up(tool: Tool, id: &str, kind: SealKind, bytes: &[u8]) -> Synced {
    let name = vault_name(tool, id);
    let f = fp(bytes);
    if is_known(&name, &f) {
        return Synced::Unchanged;
    }
    let s = match sealer() {
        Ok(s) => s,
        Err(e) => return Synced::Waiting(e),
    };
    let a = addr(tool, id);
    match s.get_sealed(&a) {
        Ok(Some(v)) if v == bytes => {
            note_known(&name, Some(f));
            Synced::Unchanged
        }
        Ok(_) => match sealed_slots::seal_verified(&*s, &a, kind, bytes) {
            Ok(()) => {
                note_known(&name, Some(f));
                Synced::Sealed
            }
            Err(e) => {
                note_known(&name, None);
                Synced::Waiting(e)
            }
        },
        Err(e) => Synced::Waiting(e),
    }
}

fn log_waiting(tool: Tool, id: &str, what: &str, why: &str) {
    crate::log_debug!("[llm-accounts] {}/{id}: {what} waiting to seal ({why}); the file copy is kept", tool.as_str());
}

// ── The two stores ──────────────────────────────────────────────────

/// One saved login's credential, wherever it is kept. Every server-login
/// read and write goes through this (the person wallet keeps its own).
pub trait SlotStore: Sync {
    fn storage(&self) -> Storage;
    /// The login's credential: `Ok(None)` = none saved.
    fn read(&self, tool: Tool, id: &str) -> Result<Option<Vec<u8>>, WalletError>;
    /// Save a whole credential (refuses one that isn't).
    fn write(&self, tool: Tool, id: &str, bytes: &[u8]) -> Result<(), WalletError>;
    fn read_api_key(&self, tool: Tool, id: &str) -> Result<Option<String>, WalletError>;
    fn write_api_key(&self, tool: Tool, id: &str, key: &str) -> Result<(), WalletError>;
    /// Is a usable login saved (a sign-in, or an API key)?
    fn has_cred(&self, tool: Tool, id: &str) -> bool;
    /// The login was removed (its folder already moved to `.removed/`).
    fn removed(&self, tool: Tool, id: &str);
}

/// Today's code, unchanged: slot files only.
pub struct FileStore;

impl SlotStore for FileStore {
    fn storage(&self) -> Storage {
        Storage::Files
    }
    fn read(&self, tool: Tool, id: &str) -> Result<Option<Vec<u8>>, WalletError> {
        store::read_slot(tool, id)
    }
    fn write(&self, tool: Tool, id: &str, bytes: &[u8]) -> Result<(), WalletError> {
        store::write_slot(tool, id, bytes)
    }
    fn read_api_key(&self, tool: Tool, id: &str) -> Result<Option<String>, WalletError> {
        Ok(store::read_api_key(tool, id))
    }
    fn write_api_key(&self, tool: Tool, id: &str, key: &str) -> Result<(), WalletError> {
        store::write_api_key(tool, id, key)
    }
    fn has_cred(&self, tool: Tool, id: &str) -> bool {
        store::slot_has_cred(tool, id)
    }
    fn removed(&self, _tool: Tool, _id: &str) {}
}

/// The vault is the source of truth; the slot files are kept in step.
pub struct VaultStore;

impl SlotStore for VaultStore {
    fn storage(&self) -> Storage {
        Storage::Vault
    }

    fn read(&self, tool: Tool, id: &str) -> Result<Option<Vec<u8>>, WalletError> {
        let plain = store::read_slot(tool, id);
        if let Ok(Some(b)) = &plain {
            if store::validate_cred(tool, b).is_ok() {
                // Plaintext is newest: the vault catches up first.
                if let Synced::Waiting(why) = sync_up(tool, id, SealKind::LoginSlot, b) {
                    log_waiting(tool, id, "sign-in", &why);
                }
                return plain;
            }
        }
        // No whole credential in the slot: the vault's copy (a sign-in
        // only: an API key's item is never handed out as a credential).
        if !tool.subscription_supported() {
            return plain;
        }
        match sealer().and_then(|s| s.get_sealed(&addr(tool, id))) {
            Ok(Some(v)) if store::validate_cred(tool, &v).is_ok() => {
                note_known(&vault_name(tool, id), Some(fp(&v)));
                Ok(Some(v))
            }
            Ok(_) => plain,
            Err(e) => match plain {
                // Nothing on disk and the vault can't be asked.
                Ok(None) => Err(unavailable(&e)),
                other => other,
            },
        }
    }

    fn write(&self, tool: Tool, id: &str, bytes: &[u8]) -> Result<(), WalletError> {
        // The file first (it validates), then the vault: a vault that
        // lags is caught up by the next read; a vault ahead of the file
        // can't happen.
        store::write_slot(tool, id, bytes)?;
        if let Synced::Waiting(why) = sync_up(tool, id, SealKind::LoginSlot, bytes) {
            log_waiting(tool, id, "sign-in", &why);
        }
        Ok(())
    }

    fn read_api_key(&self, tool: Tool, id: &str) -> Result<Option<String>, WalletError> {
        if let Some(k) = store::read_api_key(tool, id) {
            if let Synced::Waiting(why) = sync_up(tool, id, SealKind::ApiKey, k.as_bytes()) {
                log_waiting(tool, id, "API key", &why);
            }
            return Ok(Some(k));
        }
        match sealer().and_then(|s| s.get_sealed(&addr(tool, id))) {
            Ok(Some(v)) => {
                let k = String::from_utf8(v).map_err(|_| WalletError::Io("the sealed API key isn't text".into()))?;
                let k = k.trim().to_string();
                Ok((!k.is_empty()).then_some(k))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(unavailable(&e)),
        }
    }

    /// An API key is sealed straight away: with the vault closed nothing
    /// is saved (PRD §4 "Add").
    fn write_api_key(&self, tool: Tool, id: &str, key: &str) -> Result<(), WalletError> {
        let s = sealer().map_err(|e| unavailable(&e))?;
        let a = addr(tool, id);
        s.sealed_state(&a).map_err(|e| unavailable(&e))?;
        let path = store::api_key_path(tool, id);
        let prev = std::fs::read(&path).ok();
        store::write_api_key(tool, id, key)?;
        if let Err(why) = sealed_slots::seal_verified(&*s, &a, SealKind::ApiKey, key.as_bytes()) {
            // Put the file back as it was: neither store changed.
            let restored = match &prev {
                Some(p) => store::write_private(&path, p).map_err(|e| e.to_string()),
                None => sealed_slots::overwrite_then_unlink(&path).map_err(|e| e.to_string()),
            };
            if let Err(r) = restored {
                crate::log_debug!("[llm-accounts] {}/{id}: API key file not restored after a failed seal ({r})", tool.as_str());
            }
            note_known(&a.name, None);
            return Err(WalletError::VaultUnavailable(format!(
                "could not seal the API key in the K2 vault ({why}); nothing was saved"
            )));
        }
        note_known(&a.name, Some(fp(key.as_bytes())));
        Ok(())
    }

    /// Through [`Self::read`], so a sign-in a CLI left in the slot is
    /// sealed before anything uses it.
    fn has_cred(&self, tool: Tool, id: &str) -> bool {
        matches!(self.read(tool, id), Ok(Some(_))) || matches!(self.read_api_key(tool, id), Ok(Some(_)))
    }

    /// Keep the sealed copy, disabled (Trash, never expunge). A closed
    /// vault leaves it for the next sync pass, which disables the items
    /// of removed logins.
    fn removed(&self, tool: Tool, id: &str) {
        let a = addr(tool, id);
        note_known(&a.name, None);
        if let Err(e) = sealer().and_then(|s| s.disable_sealed(&a)) {
            crate::log_debug!("[llm-accounts] {}/{id}: vault copy not disabled yet ({e}); the next sync pass does it", tool.as_str());
        }
    }
}

static FILES: FileStore = FileStore;
static VAULT: VaultStore = VaultStore;

/// The store in effect now ([`storage`]).
pub fn current() -> &'static dyn SlotStore {
    match storage() {
        Storage::Files => &FILES,
        Storage::Vault => &VAULT,
    }
}

// ── What the wallet calls ───────────────────────────────────────────

pub(crate) fn read_slot(tool: Tool, id: &str) -> Result<Option<Vec<u8>>, WalletError> {
    current().read(tool, id)
}

pub(crate) fn write_slot(tool: Tool, id: &str, bytes: &[u8]) -> Result<(), WalletError> {
    current().write(tool, id, bytes)
}

/// Does the login hold a usable credential (a sign-in, or an API key)?
pub fn has_cred(tool: Tool, id: &str) -> bool {
    current().has_cred(tool, id)
}

pub(crate) fn read_api_key(tool: Tool, id: &str) -> Result<Option<String>, WalletError> {
    current().read_api_key(tool, id)
}

pub(crate) fn write_api_key(tool: Tool, id: &str, key: &str) -> Result<(), WalletError> {
    current().write_api_key(tool, id, key)
}

pub fn has_api_key(tool: Tool, id: &str) -> bool {
    matches!(read_api_key(tool, id), Ok(Some(_)))
}

pub(crate) fn removed(tool: Tool, id: &str) {
    current().removed(tool, id)
}

/// A saved login's metadata (never a token).
pub fn slot_meta(tool: Tool, id: &str) -> CredMeta {
    store::slot_meta_from(tool, id, read_slot(tool, id).ok().flatten().as_deref())
}

/// An idle Claude login's access token, for its usage probe only.
pub fn claude_access_token(id: &str) -> Option<String> {
    let b = read_slot(Tool::Claude, id).ok().flatten()?;
    let v: serde_json::Value = serde_json::from_slice(&b).ok()?;
    v["claudeAiOauth"]["accessToken"].as_str().map(str::to_string)
}

/// A spawn on a saved login needs its credential in the slot (the CLI's
/// home). With the vault as the source of truth and no whole credential
/// in the slot, write the vault's copy there (0600). The caller holds the
/// slot lock. `Ok(true)` when it wrote one. A no-op in files mode.
pub fn ensure_plaintext(tool: Tool, id: &str) -> Result<bool, WalletError> {
    if storage() != Storage::Vault {
        return Ok(false);
    }
    if let Ok(Some(b)) = store::read_slot(tool, id) {
        if store::validate_cred(tool, &b).is_ok() {
            return Ok(false);
        }
    }
    let v = sealer().map_err(|e| unavailable(&e))?.get_sealed(&addr(tool, id)).map_err(|e| unavailable(&e))?;
    match v {
        Some(v) => {
            store::write_slot(tool, id, &v)?;
            Ok(true)
        }
        None => Ok(false),
    }
}

// ── Sync, migration and the way back ────────────────────────────────

/// What one pass over every login did (counts and ids only; never a
/// value).
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SyncReport {
    pub at: i64,
    /// Sealed as a new version (first time, or the file was newer).
    pub sealed: usize,
    /// The vault already held the file's bytes.
    pub unchanged: usize,
    /// The vault is closed or a read-back failed: plaintext kept.
    pub waiting: usize,
    /// A slot file that isn't a whole credential: left alone, never
    /// sealed over a good copy (ids).
    pub broken: Vec<String>,
    /// Removed logins whose copy is now kept disabled.
    pub disabled: usize,
    /// A switch or refresh held the login: next pass.
    pub busy: usize,
    /// No credential anywhere (not set up, or a removed login without
    /// files).
    pub empty: usize,
    /// The vault couldn't be reached at all.
    pub vault_closed: bool,
}

fn now() -> i64 {
    super::now()
}

const PASS_LOCK_WAIT: Duration = Duration::from_secs(2);

/// Seal every saved login's file into the vault (the migration at boot or
/// when the switch flips to `vault`, then every few minutes). Idempotent;
/// never wipes; skips logins mid-sign-in and ones a switch holds. Removed
/// logins (`removed`, with their `.removed/` folder) are sealed and kept
/// disabled. Takes the tool and slot locks per login, never the
/// database.
pub fn sync_to_vault(entries: &[Entry], removed: &[Entry]) -> SyncReport {
    let mut r = SyncReport { at: now(), ..Default::default() };
    let s = match sealer() {
        Ok(s) if s.sealed_state(&addr(Tool::Claude, "acc_probe")).is_ok() => s,
        _ => {
            r.vault_closed = true;
            return r;
        }
    };
    for e in entries {
        let Some(tool) = e.tool() else { continue };
        if e.state == super::state::SIGNING_IN || e.removed_at.is_some() {
            continue;
        }
        let Ok(_tl) = store::lock_tool(tool, PASS_LOCK_WAIT) else {
            r.busy += 1;
            continue;
        };
        let Ok(_sl) = store::lock_slot(tool, &e.id, PASS_LOCK_WAIT) else {
            r.busy += 1;
            continue;
        };
        let (kind, bytes) = if e.is_api_key() {
            (SealKind::ApiKey, store::read_api_key(tool, &e.id).map(String::into_bytes))
        } else {
            match store::read_slot(tool, &e.id) {
                Ok(Some(b)) if store::validate_cred(tool, &b).is_ok() => (SealKind::LoginSlot, Some(b)),
                Ok(Some(_)) => {
                    r.broken.push(e.id.clone());
                    continue;
                }
                _ => (SealKind::LoginSlot, None),
            }
        };
        match bytes {
            Some(b) => match sync_up(tool, &e.id, kind, &b) {
                Synced::Unchanged => r.unchanged += 1,
                Synced::Sealed => r.sealed += 1,
                Synced::Waiting(why) => {
                    log_waiting(tool, &e.id, "login", &why);
                    r.waiting += 1;
                }
            },
            None => r.empty += 1,
        }
    }
    for e in removed {
        let Some(tool) = e.tool() else { continue };
        let a = addr(tool, &e.id);
        let was_active = match s.sealed_state(&a) {
            Ok(Some(SealedState::Disabled)) => {
                r.unchanged += 1;
                continue;
            }
            Ok(st) => st == Some(SealedState::Active),
            Err(why) => {
                log_waiting(tool, &e.id, "removed login", &why);
                r.waiting += 1;
                continue;
            }
        };
        let found = removed_copy(tool, e);
        let sealed = match &found {
            Some((kind, b)) => sealed_slots::seal_verified(&*s, &a, *kind, b),
            None => Ok(()),
        };
        if let Err(why) = sealed {
            log_waiting(tool, &e.id, "removed login", &why);
            r.waiting += 1;
            continue;
        }
        note_known(&a.name, None);
        match s.disable_sealed(&a) {
            Ok(()) if found.is_some() || was_active => r.disabled += 1,
            Ok(()) => r.empty += 1,
            Err(why) => {
                log_waiting(tool, &e.id, "removed login", &why);
                r.waiting += 1;
            }
        }
    }
    r
}

/// A removed login's credential from its newest `.removed/` folder
/// (`<tool>-<id>-<ts>`, or `<id>-<ts>` from older K2s).
fn removed_copy(tool: Tool, e: &Entry) -> Option<(SealKind, Vec<u8>)> {
    let root = store::removed_root();
    let mut best: Option<(i64, std::path::PathBuf)> = None;
    for d in std::fs::read_dir(&root).ok()?.flatten() {
        let name = d.file_name().to_string_lossy().into_owned();
        let rest = name.strip_prefix(&format!("{}-", tool.as_str())).unwrap_or(&name);
        let Some(ts) = rest.strip_prefix(&format!("{}-", e.id)) else { continue };
        let ts: i64 = ts.parse().unwrap_or(0);
        if best.as_ref().is_none_or(|(b, _)| ts > *b) {
            best = Some((ts, d.path()));
        }
    }
    let dir = best?.1;
    if e.is_api_key() {
        let k = std::fs::read_to_string(dir.join("api-key")).ok()?;
        let k = k.trim().to_string();
        return (!k.is_empty()).then(|| (SealKind::ApiKey, k.into_bytes()));
    }
    let b = std::fs::read(dir.join(tool.cred_file_name())).ok()?;
    store::validate_cred(tool, &b).ok()?;
    Some((SealKind::LoginSlot, b))
}

/// What switching back to files did.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExportReport {
    pub at: i64,
    /// Slot files written back from the vault (they were missing).
    pub restored: usize,
    /// The slot file was already whole (the usual case in 0.46.6).
    pub already: usize,
    /// Neither a file nor a vault copy.
    pub empty: usize,
    /// The vault couldn't be asked for a missing file (ids).
    pub failed: Vec<String>,
    pub busy: usize,
}

/// Before the switch goes back to files: every login whose slot has no
/// whole credential gets the vault's copy written back (0600, read back
/// by `write_slot`). The vault copies stay (both stores in step). Never
/// overwrites a whole file.
pub fn export_to_files(entries: &[Entry]) -> ExportReport {
    let mut r = ExportReport { at: now(), ..Default::default() };
    for e in entries {
        let Some(tool) = e.tool() else { continue };
        if e.state == super::state::SIGNING_IN || e.removed_at.is_some() {
            continue;
        }
        let Ok(_tl) = store::lock_tool(tool, PASS_LOCK_WAIT) else {
            r.busy += 1;
            continue;
        };
        let Ok(_sl) = store::lock_slot(tool, &e.id, PASS_LOCK_WAIT) else {
            r.busy += 1;
            continue;
        };
        let whole = if e.is_api_key() {
            store::has_api_key(tool, &e.id)
        } else {
            matches!(store::read_slot(tool, &e.id), Ok(Some(b)) if store::validate_cred(tool, &b).is_ok())
        };
        if whole {
            r.already += 1;
            continue;
        }
        match sealer().and_then(|s| s.get_sealed(&addr(tool, &e.id))) {
            Ok(Some(v)) => {
                let wrote = if e.is_api_key() {
                    String::from_utf8(v)
                        .map_err(|_| WalletError::Io("not text".into()))
                        .and_then(|k| store::write_api_key(tool, &e.id, k.trim()))
                } else {
                    store::write_slot(tool, &e.id, &v)
                };
                match wrote {
                    Ok(()) => r.restored += 1,
                    Err(err) => {
                        crate::log_debug!("[llm-accounts] {}/{}: not written back to files ({})", tool.as_str(), e.id, err.code());
                        r.failed.push(e.id.clone());
                    }
                }
            }
            Ok(None) => r.empty += 1,
            Err(_) => r.failed.push(e.id.clone()),
        }
    }
    r
}

/// One login's storage facts for the status route (never a value).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ItemFacts {
    pub id: String,
    pub tool: String,
    pub label: String,
    pub removed: bool,
    /// A whole credential (or API key) is in the slot file.
    pub file: bool,
    /// `active`, `disabled`, `none`, or `unknown` (vault closed).
    pub vault: &'static str,
    /// The vault's active copy equals the file (`None` when either is
    /// missing or the vault is closed).
    pub in_step: Option<bool>,
}

/// Storage facts for each login (reads the vault: status routes only).
pub fn facts(entries: &[Entry]) -> Vec<ItemFacts> {
    let s = sealer().ok();
    entries
        .iter()
        .filter_map(|e| {
            let tool = e.tool()?;
            let file_bytes: Option<Vec<u8>> = if e.removed_at.is_some() {
                removed_copy(tool, e).map(|(_, b)| b)
            } else if e.is_api_key() {
                store::read_api_key(tool, &e.id).map(String::into_bytes)
            } else {
                store::read_slot(tool, &e.id).ok().flatten().filter(|b| store::validate_cred(tool, b).is_ok())
            };
            let a = addr(tool, &e.id);
            let (vault, in_step) = match s.as_ref().map(|s| (s.sealed_state(&a), s)) {
                Some((Ok(Some(SealedState::Active)), s)) => {
                    let step = match (s.get_sealed(&a), &file_bytes) {
                        (Ok(Some(v)), Some(f)) => Some(&v == f),
                        _ => None,
                    };
                    ("active", step)
                }
                Some((Ok(Some(SealedState::Disabled)), _)) => ("disabled", None),
                Some((Ok(None), _)) => ("none", None),
                _ => ("unknown", None),
            };
            Some(ItemFacts {
                id: e.id.clone(),
                tool: e.tool.clone(),
                label: e.label.clone(),
                removed: e.removed_at.is_some(),
                file: file_bytes.is_some(),
                vault,
                in_step,
            })
        })
        .collect()
}

/// Is the vault reachable now (for the status and the nudge)?
pub fn vault_open() -> Result<(), String> {
    sealer()?.sealed_state(&addr(Tool::Claude, "acc_probe")).map(|_| ())
}

/// Has the vault's recovery key been confirmed (`None`: can't tell)?
pub fn recovery_confirmed() -> Option<bool> {
    sealer().ok()?.recovery_confirmed()
}

/// The one-line nudge shown while the vault holds logins but its
/// recovery key was never confirmed (Rosson #5: migrate anyway).
pub const RECOVERY_NUDGE: &str =
    "Your saved LLM logins are in the K2 vault, but its recovery key was never confirmed. Confirm it in Settings → Vault so they can be restored if this Mac's key is lost.";

#[cfg(test)]
#[path = "slots_tests.rs"]
mod tests;
