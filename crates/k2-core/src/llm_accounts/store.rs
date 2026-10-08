//! Where logins live: wallet slots, the tools' live stores, locks, and
//! metadata parsing. Token bytes move only between these stores; nothing
//! here hands a token to a caller as text.
//!
//! Live stores (verified against the installed binaries 2026-10-07):
//! - **Claude.** macOS: keychain item `Claude Code-credentials` (plus
//!   `-<8 hex of sha256(dir)>` only when `CLAUDE_CONFIG_DIR` is set),
//!   account `$USER` (or `claude-code-user`), written by Claude through
//!   `security -i` with a hex (`-X`) payload. Fallback and Linux:
//!   `<config dir>/.credentials.json`, 0600.
//! - **Codex.** `<CODEX_HOME or ~/.codex>/auth.json` (file store). A
//!   Codex that keeps its login in the OS keyring ("Codex Auth") can't be
//!   switched by K2; the route says so.
//! - **Grok.** `<GROK_HOME or ~/.grok>/auth.json`, guarded by Grok's own
//!   `auth.json.lock` flock, which K2 also takes for a swap.
//!
//! Tests never reach the keychain: [`keychain_enabled`] is false under
//! `cfg(test)`, under a temp `HOME`, and whenever the agent shim dir is
//! set.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use super::{Tool, WalletError};

// ── Paths ───────────────────────────────────────────────────────────

/// `~/.k2/llm-accounts` — the wallet. Excluded from clone/migration
/// bundles and anything synced or published.
pub fn wallet_root() -> PathBuf {
    crate::paths::k2_home().join(WALLET_DIR_NAME)
}

/// The wallet's folder name under `~/.k2` (clone/migration excludes it).
pub const WALLET_DIR_NAME: &str = "llm-accounts";

pub fn tool_dir(tool: Tool) -> PathBuf {
    wallet_root().join(tool.as_str())
}

/// `~/.k2/llm-accounts/<tool>/<id>/` — one login, laid out as the tool's
/// own home so the tool's login / refresh / usage commands can run in it.
pub fn slot_dir(tool: Tool, id: &str) -> PathBuf {
    tool_dir(tool).join(id)
}

pub fn slot_cred_path(tool: Tool, id: &str) -> PathBuf {
    slot_dir(tool, id).join(tool.cred_file_name())
}

/// `~/.k2/llm-accounts/.removed/` (Trash, never expunge).
pub fn removed_root() -> PathBuf {
    wallet_root().join(".removed")
}

fn user_home() -> PathBuf {
    dirs::home_dir().unwrap_or_default()
}

/// The tool's live home: its home env var in the daemon's own env when
/// set (a user who exports `CLAUDE_CONFIG_DIR`), else the default.
pub fn live_home(tool: Tool) -> (PathBuf, bool) {
    if let Ok(v) = std::env::var(tool.home_env_var()) {
        if !v.trim().is_empty() {
            return (PathBuf::from(v), true);
        }
    }
    let dir = match tool {
        Tool::Claude => user_home().join(".claude"),
        Tool::Codex => user_home().join(".codex"),
        Tool::Grok => user_home().join(".grok"),
        Tool::Gemini => user_home().join(".gemini"),
    };
    (dir, false)
}

pub fn live_cred_path(tool: Tool) -> PathBuf {
    live_home(tool).0.join(tool.cred_file_name())
}

/// Claude's global config for the live home (`~/.claude.json`, or
/// `<dir>/.claude.json` when `CLAUDE_CONFIG_DIR` is set). Read only for
/// the `oauthAccount` email/org labels.
pub fn claude_live_global_config() -> PathBuf {
    let (dir, env_set) = live_home(Tool::Claude);
    if env_set {
        dir.join(".claude.json")
    } else {
        user_home().join(".claude.json")
    }
}

// ── Claude keychain naming ──────────────────────────────────────────

/// `Claude Code-credentials`, plus `-<first 8 hex of sha256(dir)>` only
/// when `CLAUDE_CONFIG_DIR` is set (`None` = not set).
pub fn claude_keychain_service(config_dir: Option<&Path>) -> String {
    use sha2::{Digest, Sha256};
    match config_dir {
        None => "Claude Code-credentials".to_string(),
        Some(dir) => {
            let digest = Sha256::digest(dir.to_string_lossy().as_bytes());
            let hex: String = digest.iter().take(4).map(|b| format!("{b:02x}")).collect();
            format!("Claude Code-credentials-{hex}")
        }
    }
}

/// The keychain account Claude uses: `$USER` when it matches
/// `^[a-zA-Z0-9._-]+$`, else `claude-code-user`.
pub fn claude_keychain_account() -> String {
    let user = std::env::var("USER").unwrap_or_default();
    if !user.is_empty()
        && user.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        user
    } else {
        "claude-code-user".to_string()
    }
}

/// The live keychain service for Claude (honors a daemon-level
/// `CLAUDE_CONFIG_DIR`).
pub fn claude_live_keychain_service() -> String {
    let (dir, env_set) = live_home(Tool::Claude);
    claude_keychain_service(if env_set { Some(dir.as_path()) } else { None })
}

/// May K2 touch the macOS keychain? Never in tests, never under a temp
/// `HOME`, never with the agent shim dir set, and `K2_LLM_ACCOUNTS_KEYCHAIN=0`
/// turns it off.
pub fn keychain_enabled() -> bool {
    if cfg!(test) || !cfg!(target_os = "macos") {
        return false;
    }
    if std::env::var("K2_LLM_ACCOUNTS_KEYCHAIN").map(|v| v.trim() == "0").unwrap_or(false) {
        return false;
    }
    let g = crate::terminal::agent_spawn_guard::GuardEnv::from_process();
    !g.home_is_temp() && g.shim_dirs.is_none()
}

pub mod keychain {
    //! Claude's keychain items (macOS). Callers check
    //! [`super::keychain_enabled`] first. Reads go through
    //! `/usr/bin/security` (the trusted reader on Claude's item). Writes
    //! set the data in process via [`crate::macos_keychain`]: no size
    //! limit (`security -i` splits lines over ~4 KB, and in 0.45.0 that
    //! wrote a truncated login), and the secret is never on argv. Every
    //! write is read back before it returns `Ok`.

    /// Read a generic password. `Ok(None)` = no such item (or empty).
    #[cfg(target_os = "macos")]
    pub fn read(service: &str, account: &str) -> Result<Option<Vec<u8>>, String> {
        match crate::macos_keychain::read(service, account, None) {
            Ok(Some(b)) if b.is_empty() => Ok(None),
            Ok(v) => Ok(v),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Create or update, then read back and compare. A new item is
    /// created by `security` itself, so it gets the same access list as
    /// one Claude Code made.
    #[cfg(target_os = "macos")]
    pub fn write(service: &str, account: &str, secret: &[u8]) -> Result<(), String> {
        crate::macos_keychain::write(service, account, secret, &Default::default()).map_err(|e| e.to_string())
    }

    #[cfg(target_os = "macos")]
    pub fn delete(service: &str, account: &str) -> Result<(), String> {
        crate::macos_keychain::delete(service, account, None).map_err(|e| e.to_string())
    }

    #[cfg(not(target_os = "macos"))]
    pub fn read(_service: &str, _account: &str) -> Result<Option<Vec<u8>>, String> {
        Err("the keychain is macOS-only".into())
    }

    #[cfg(not(target_os = "macos"))]
    pub fn write(_service: &str, _account: &str, _secret: &[u8]) -> Result<(), String> {
        Err("the keychain is macOS-only".into())
    }

    #[cfg(not(target_os = "macos"))]
    pub fn delete(_service: &str, _account: &str) -> Result<(), String> {
        Err("the keychain is macOS-only".into())
    }
}

// ── Credential validation ──────────────────────────────────────────

/// Is `bytes` a whole credential for `tool`? Claude, Codex and Grok keep
/// a JSON object, and a truncated or half-written store doesn't parse.
/// K2 never writes a credential that fails this into a live store or a
/// wallet slot, so a broken live store can't overwrite a good slot.
pub fn validate_cred(tool: Tool, bytes: &[u8]) -> Result<(), String> {
    if !bytes.iter().any(|c| !c.is_ascii_whitespace()) {
        return Err("it is empty".into());
    }
    match tool {
        Tool::Claude | Tool::Codex | Tool::Grok => match serde_json::from_slice::<serde_json::Value>(bytes) {
            // Claude: a sign-in has a `claudeAiOauth` token. (`{}` is also
            // the placeholder a new keychain item holds for an instant.)
            Ok(v @ serde_json::Value::Object(_)) if tool == Tool::Claude => {
                let o = &v["claudeAiOauth"];
                let has = |k: &str| o[k].as_str().map(|s| !s.is_empty()).unwrap_or(false);
                if has("accessToken") || has("refreshToken") {
                    Ok(())
                } else {
                    Err("it has no claudeAiOauth token".into())
                }
            }
            Ok(serde_json::Value::Object(_)) => Ok(()),
            Ok(_) => Err("it is not a JSON object".into()),
            // Never the parser's message: it can quote the input.
            Err(e) => Err(format!(
                "it is not valid JSON: {} at byte {} of {}",
                match e.classify() {
                    serde_json::error::Category::Eof => "cut off before the end",
                    serde_json::error::Category::Syntax => "syntax error",
                    serde_json::error::Category::Data => "data error",
                    serde_json::error::Category::Io => "read error",
                },
                e.column(),
                bytes.len()
            )),
        },
        Tool::Gemini => Ok(()),
    }
}

pub(crate) fn invalid_cred(tool: Tool, what: &str, why: String) -> WalletError {
    WalletError::Conflict(
        "credential_invalid",
        format!("{}'s {what} is not a whole sign-in ({why}); K2 left everything as it was", tool.display()),
    )
}

// ── Private file IO ────────────────────────────────────────────────

#[cfg(unix)]
pub fn set_mode(p: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(p, fs::Permissions::from_mode(mode))
}
#[cfg(not(unix))]
pub fn set_mode(_p: &Path, _mode: u32) -> std::io::Result<()> {
    Ok(())
}

/// Create `dir` (and the wallet parents) with mode 0700.
pub fn ensure_private_dir(dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    set_mode(dir, 0o700)?;
    let root = wallet_root();
    let mut p = dir.parent();
    while let Some(parent) = p {
        if !parent.starts_with(&root) {
            break;
        }
        let _ = set_mode(parent, 0o700);
        p = parent.parent();
    }
    Ok(())
}

/// Atomic write with mode 0600 from the first byte (tmp file created
/// 0600 next to the target, fsync, rename).
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(
        ".{}.k2tmp-{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("cred"),
        std::process::id()
    ));
    let res = (|| -> std::io::Result<()> {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        Ok(())
    })();
    if let Err(e) = res {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    set_mode(&tmp, 0o600)?;
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        e
    })
}

fn read_nonempty(path: &Path) -> Result<Option<Vec<u8>>, WalletError> {
    match fs::read(path) {
        Ok(b) if b.iter().any(|c| !c.is_ascii_whitespace()) => Ok(Some(b)),
        Ok(_) => Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(WalletError::Io(format!("{}: {e}", path.display()))),
    }
}

// ── Slots ───────────────────────────────────────────────────────────

/// Does the slot hold a usable login (a sign-in, or an API key)?
pub fn slot_has_cred(tool: Tool, id: &str) -> bool {
    matches!(read_slot(tool, id), Ok(Some(_))) || has_api_key(tool, id)
}

/// A slot's credential. Claude on macOS: Claude's storage is
/// "keychain-with-plaintext-fallback" (binary 2.1.292): with
/// `CLAUDE_CONFIG_DIR=<slot>` it reads the hashed keychain item first and
/// the slot's `.credentials.json` only when that item is absent; when it
/// refreshes it writes the item and deletes the file. So for a slot a
/// pinned session has used, the hashed item is the truth.
pub(crate) fn read_slot(tool: Tool, id: &str) -> Result<Option<Vec<u8>>, WalletError> {
    if tool == Tool::Claude && keychain_enabled() {
        let svc = claude_keychain_service(Some(&slot_dir(tool, id)));
        if let Ok(Some(b)) = keychain::read(&svc, &claude_keychain_account()) {
            return Ok(Some(b));
        }
    }
    read_nonempty(&slot_cred_path(tool, id))
}

/// Save a credential into a wallet slot. Refuses anything that isn't a
/// whole credential ([`validate_cred`]): a slot is the only copy of an
/// idle login, so it is never overwritten with a broken one.
pub(crate) fn write_slot(tool: Tool, id: &str, bytes: &[u8]) -> Result<(), WalletError> {
    validate_cred(tool, bytes).map_err(|why| invalid_cred(tool, "credential to save", why))?;
    let dir = slot_dir(tool, id);
    ensure_private_dir(&dir).map_err(|e| WalletError::Io(e.to_string()))?;
    if tool == Tool::Claude && keychain_enabled() {
        // Keep a hashed item Claude made for this slot in step; the file
        // is K2's copy and what Claude falls back to.
        let svc = claude_keychain_service(Some(&dir));
        let acct = claude_keychain_account();
        if let Ok(Some(_)) = keychain::read(&svc, &acct) {
            keychain::write(&svc, &acct, bytes).map_err(WalletError::Io)?;
        }
    }
    write_private(&slot_cred_path(tool, id), bytes).map_err(|e| WalletError::Io(e.to_string()))
}

// ── Live store ──────────────────────────────────────────────────────

/// Read the tool's live login. `Ok(None)` = signed out.
pub(crate) fn read_live(tool: Tool) -> Result<Option<Vec<u8>>, WalletError> {
    if !tool.subscription_supported() {
        return Ok(None);
    }
    if tool == Tool::Claude && keychain_enabled() {
        let svc = claude_live_keychain_service();
        match keychain::read(&svc, &claude_keychain_account()) {
            Ok(Some(b)) => return Ok(Some(b)),
            Ok(None) => {}
            // The error says "unlock" only when the keychain really is
            // locked (macos_keychain maps the status codes).
            Err(e) => return Err(WalletError::LiveStoreUnavailable(format!(
                "could not read Claude's keychain item: {e}"
            ))),
        }
    }
    read_nonempty(&live_cred_path(tool))
}

/// Make `bytes` the tool's live login, all or nothing: refuse a
/// credential that isn't whole ([`validate_cred`]), remember what is live
/// now, write, read back and compare, and on ANY failure put the previous
/// live value back exactly (or remove the new one when the tool was
/// signed out). The swap is committed only when the read-back matches.
pub(crate) fn write_live(tool: Tool, bytes: &[u8]) -> Result<(), WalletError> {
    validate_cred(tool, bytes).map_err(|why| invalid_cred(tool, "login to make live", why))?;
    if tool == Tool::Gemini {
        return Err(WalletError::LiveStoreUnavailable(
            "Gemini subscriptions aren't supported; use an API token".into(),
        ));
    }
    // Can't read what's live → can't restore it → don't write.
    let prev = read_live(tool)?;
    commit_verified(
        tool,
        bytes,
        prev.as_deref(),
        |b| write_live_raw(tool, b),
        || read_live(tool),
        || clear_live(tool),
    )
}

/// The write → read back → compare → restore sequence behind
/// [`write_live`], with the store operations passed in so tests can
/// drive a store that truncates or fails.
pub(crate) fn commit_verified(
    tool: Tool,
    bytes: &[u8],
    prev: Option<&[u8]>,
    write: impl Fn(&[u8]) -> Result<(), WalletError>,
    read: impl Fn() -> Result<Option<Vec<u8>>, WalletError>,
    clear: impl Fn() -> Result<(), WalletError>,
) -> Result<(), WalletError> {
    let check = |want: &[u8]| -> Result<(), WalletError> {
        match read()? {
            Some(got) if got == want => Ok(()),
            Some(got) => Err(WalletError::LiveStoreUnavailable(format!(
                "it read back as {} bytes instead of the {} written",
                got.len(),
                want.len()
            ))),
            None => Err(WalletError::LiveStoreUnavailable("it read back empty".into())),
        }
    };
    let Err(e) = write(bytes).and_then(|_| check(bytes)) else {
        return Ok(());
    };
    let restored = match prev {
        Some(p) => write(p).and_then(|_| check(p)),
        None => clear(),
    };
    let what = match prev {
        Some(_) => "the previous login was put back",
        None => "the tool was left signed out, as it was",
    };
    Err(WalletError::LiveStoreUnavailable(match restored {
        Ok(()) => format!("could not make the new {} login live ({e}); {what}", tool.display()),
        Err(r) => format!(
            "could not make the new {} login live ({e}), and restoring the previous one ALSO failed ({r}); sign in to {} again",
            tool.display(),
            tool.display()
        ),
    }))
}

/// Remove a live login K2 just wrote when the tool was signed out before.
fn clear_live(tool: Tool) -> Result<(), WalletError> {
    if tool == Tool::Claude && keychain_enabled() {
        let svc = claude_live_keychain_service();
        keychain::delete(&svc, &claude_keychain_account()).map_err(WalletError::LiveStoreUnavailable)?;
        return Ok(());
    }
    match fs::remove_file(live_cred_path(tool)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(WalletError::Io(e.to_string())),
    }
}

/// Write the tool's live store (no checks; see [`write_live`]). Claude
/// on macOS: keychain item, plus the fallback file when one already
/// exists (so the two never disagree). Grok: under Grok's own
/// `auth.json.lock`.
fn write_live_raw(tool: Tool, bytes: &[u8]) -> Result<(), WalletError> {
    match tool {
        Tool::Claude => {
            let file = live_cred_path(tool);
            if keychain_enabled() {
                let svc = claude_live_keychain_service();
                keychain::write(&svc, &claude_keychain_account(), bytes).map_err(|e| {
                    WalletError::LiveStoreUnavailable(format!("could not write Claude's keychain item: {e}"))
                })?;
                if file.exists() {
                    write_private(&file, bytes).map_err(|e| WalletError::Io(e.to_string()))?;
                }
                Ok(())
            } else {
                write_private(&file, bytes).map_err(|e| WalletError::Io(e.to_string()))
            }
        }
        Tool::Codex => {
            write_private(&live_cred_path(tool), bytes).map_err(|e| WalletError::Io(e.to_string()))
        }
        Tool::Gemini => Err(WalletError::LiveStoreUnavailable(
            "Gemini subscriptions aren't supported; use an API token".into(),
        )),
        Tool::Grok => {
            let (home, _) = live_home(tool);
            let _grok_lock = FileLock::acquire(&home.join("auth.json.lock"), Duration::from_secs(10))
                .map_err(|_| WalletError::LockBusy("Grok (auth.json.lock)".into()))?;
            write_private(&live_cred_path(tool), bytes).map_err(|e| WalletError::Io(e.to_string()))
        }
    }
}

// ── Locks ───────────────────────────────────────────────────────────

/// An exclusive `flock` on a file, released on drop.
pub struct FileLock {
    #[allow(dead_code)]
    file: fs::File,
}

impl FileLock {
    pub fn acquire(path: &Path, timeout: Duration) -> std::io::Result<FileLock> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)?;
        let deadline = Instant::now() + timeout;
        loop {
            #[cfg(unix)]
            {
                use std::os::unix::io::AsRawFd;
                let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
                if rc == 0 {
                    return Ok(FileLock { file });
                }
            }
            #[cfg(not(unix))]
            {
                return Ok(FileLock { file });
            }
            #[allow(unreachable_code)]
            if Instant::now() >= deadline {
                return Err(std::io::Error::new(std::io::ErrorKind::WouldBlock, "lock busy"));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

static TOOL_MUTEX: [Mutex<()>; 4] = [Mutex::new(()), Mutex::new(()), Mutex::new(()), Mutex::new(())];

fn mutex_for(tool: Tool) -> &'static Mutex<()> {
    match tool {
        Tool::Claude => &TOOL_MUTEX[0],
        Tool::Codex => &TOOL_MUTEX[1],
        Tool::Grok => &TOOL_MUTEX[2],
        Tool::Gemini => &TOOL_MUTEX[3],
    }
}

/// The per-tool lock: an in-process mutex plus a flock on
/// `~/.k2/llm-accounts/<tool>/.lock`. Every swap, import, refresh and
/// login finalize for that tool holds it.
pub struct ToolLock {
    pub tool: Tool,
    _guard: MutexGuard<'static, ()>,
    _file: FileLock,
}

pub fn lock_tool(tool: Tool, timeout: Duration) -> Result<ToolLock, WalletError> {
    let deadline = Instant::now() + timeout;
    let guard = loop {
        match mutex_for(tool).try_lock() {
            Ok(g) => break g,
            Err(std::sync::TryLockError::Poisoned(p)) => break p.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    return Err(WalletError::LockBusy(tool.display().into()));
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    };
    let dir = tool_dir(tool);
    ensure_private_dir(&dir).map_err(|e| WalletError::Io(e.to_string()))?;
    let left = deadline.saturating_duration_since(Instant::now());
    let file = FileLock::acquire(&dir.join(".lock"), left)
        .map_err(|_| WalletError::LockBusy(tool.display().into()))?;
    Ok(ToolLock { tool, _guard: guard, _file: file })
}

/// The per-slot lock: one login's credential is never swapped, refreshed
/// and handed to a spawning session at the same time. An in-process
/// mutex per slot plus a flock on `<slot>/.lock`.
pub struct SlotLock {
    _guard: std::sync::MutexGuard<'static, ()>,
    _file: FileLock,
}

fn slot_mutex(tool: Tool, id: &str) -> &'static Mutex<()> {
    use std::collections::HashMap;
    use std::sync::OnceLock;
    static MAP: OnceLock<Mutex<HashMap<String, &'static Mutex<()>>>> = OnceLock::new();
    let map = MAP.get_or_init(|| Mutex::new(HashMap::new()));
    let mut g = map.lock().unwrap_or_else(|p| p.into_inner());
    g.entry(format!("{}/{}", tool.as_str(), id))
        .or_insert_with(|| Box::leak(Box::new(Mutex::new(()))))
}

pub fn lock_slot(tool: Tool, id: &str, timeout: Duration) -> Result<SlotLock, WalletError> {
    let deadline = Instant::now() + timeout;
    let m = slot_mutex(tool, id);
    let guard = loop {
        match m.try_lock() {
            Ok(g) => break g,
            Err(std::sync::TryLockError::Poisoned(p)) => break p.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    return Err(WalletError::LockBusy(tool.display().into()));
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    };
    let dir = slot_dir(tool, id);
    ensure_private_dir(&dir).map_err(|e| WalletError::Io(e.to_string()))?;
    let left = deadline.saturating_duration_since(Instant::now());
    let file = FileLock::acquire(&dir.join(".lock"), left)
        .map_err(|_| WalletError::LockBusy(tool.display().into()))?;
    Ok(SlotLock { _guard: guard, _file: file })
}

// ── API keys ────────────────────────────────────────────────────────

/// `<slot>/api-key`, 0600. Never returned by any route.
pub fn api_key_path(tool: Tool, id: &str) -> PathBuf {
    slot_dir(tool, id).join("api-key")
}

pub(crate) fn write_api_key(tool: Tool, id: &str, key: &str) -> Result<(), WalletError> {
    ensure_private_dir(&slot_dir(tool, id)).map_err(|e| WalletError::Io(e.to_string()))?;
    write_private(&api_key_path(tool, id), key.as_bytes()).map_err(|e| WalletError::Io(e.to_string()))
}

pub(crate) fn read_api_key(tool: Tool, id: &str) -> Option<String> {
    let b = std::fs::read(api_key_path(tool, id)).ok()?;
    let k = String::from_utf8(b).ok()?.trim().to_string();
    (!k.is_empty()).then_some(k)
}

pub fn has_api_key(tool: Tool, id: &str) -> bool {
    read_api_key(tool, id).is_some()
}

// ── Metadata parsing (never returns a token) ───────────────────────

/// What K2 may know about a credential without holding it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CredMeta {
    pub present: bool,
    pub has_refresh: bool,
    /// Access-token expiry, unix seconds.
    pub expires_at: Option<i64>,
    pub plan: Option<String>,
    pub email: Option<String>,
    pub org: Option<String>,
    /// Codex `last_refresh` (unix seconds).
    pub last_refresh: Option<i64>,
}

fn jwt_claims(token: &str) -> Option<serde_json::Value> {
    use base64::Engine;
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn safe_label(v: Option<&str>) -> Option<String> {
    let s = v?.trim();
    if s.is_empty() || s.len() > 200 || super::looks_like_secret(s) {
        return None;
    }
    Some(s.to_string())
}

/// Parse a credential's metadata. Unknown shapes → `present` only.
pub fn parse_meta(tool: Tool, bytes: &[u8]) -> CredMeta {
    let mut m = CredMeta { present: bytes.iter().any(|c| !c.is_ascii_whitespace()), ..Default::default() };
    let v: serde_json::Value = match serde_json::from_slice(bytes) {
        Ok(v) => v,
        Err(_) => return m,
    };
    match tool {
        Tool::Claude => {
            let o = &v["claudeAiOauth"];
            m.has_refresh = o["refreshToken"].as_str().map(|s| !s.is_empty()).unwrap_or(false);
            m.expires_at = o["expiresAt"].as_i64().map(|x| if x > 1_000_000_000_000 { x / 1000 } else { x });
            m.plan = safe_label(o["subscriptionType"].as_str());
        }
        Tool::Codex => {
            let t = &v["tokens"];
            m.has_refresh = t["refresh_token"].as_str().map(|s| !s.is_empty()).unwrap_or(false);
            if let Some(claims) = t["access_token"].as_str().and_then(jwt_claims) {
                m.expires_at = claims["exp"].as_i64();
            }
            if let Some(claims) = t["id_token"].as_str().and_then(jwt_claims) {
                m.email = safe_label(claims["email"].as_str());
                m.plan = safe_label(claims["https://api.openai.com/auth"]["chatgpt_plan_type"].as_str());
            }
            m.last_refresh = v["last_refresh"]
                .as_str()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.timestamp());
        }
        Tool::Gemini => {}
        Tool::Grok => {
            // Grok's auth.json shape is not documented; presence only,
            // plus any obvious refresh field.
            let s = v.to_string();
            m.has_refresh = s.contains("refresh");
        }
    }
    m
}

/// Claude's `oauthAccount` labels from a `.claude.json` (email, org).
pub fn claude_account_labels(global_config: &Path) -> (Option<String>, Option<String>) {
    let Ok(bytes) = fs::read(global_config) else {
        return (None, None);
    };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return (None, None);
    };
    let a = &v["oauthAccount"];
    (
        safe_label(a["emailAddress"].as_str()),
        safe_label(a["organizationName"].as_str()),
    )
}

/// Metadata of a slot (plus Claude's slot `.claude.json` labels).
pub fn slot_meta(tool: Tool, id: &str) -> CredMeta {
    let mut m = match read_slot(tool, id) {
        Ok(Some(b)) => parse_meta(tool, &b),
        _ => CredMeta::default(),
    };
    if tool == Tool::Claude {
        let (e, o) = claude_account_labels(&slot_dir(tool, id).join(".claude.json"));
        m.email = m.email.or(e);
        m.org = m.org.or(o);
    }
    m
}

/// Metadata of the live store.
pub fn live_meta(tool: Tool) -> Result<CredMeta, WalletError> {
    let mut m = match read_live(tool)? {
        Some(b) => parse_meta(tool, &b),
        None => CredMeta::default(),
    };
    if tool == Tool::Claude {
        let (e, o) = claude_account_labels(&claude_live_global_config());
        m.email = m.email.or(e);
        m.org = m.org.or(o);
    }
    Ok(m)
}

/// Read an access token from a slot for a usage probe. Crate-private
/// accessor kept next to the store so callers outside never see a raw
/// credential file.
pub fn slot_claude_access_token(id: &str) -> Option<String> {
    let b = read_slot(Tool::Claude, id).ok().flatten()?;
    let v: serde_json::Value = serde_json::from_slice(&b).ok()?;
    v["claudeAiOauth"]["accessToken"].as_str().map(str::to_string)
}
