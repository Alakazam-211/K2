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
    crate::paths::home_dir().unwrap_or_default()
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
    //! [`super::keychain_enabled`] first. Everything goes through
    //! [`crate::macos_keychain`]: reads and writes by `/usr/bin/security`
    //! (secret on stdin, never argv), so an item keeps the `apple-tool:`
    //! partition Claude Code's own `security` reads need. 0.45.1/0.45.2
    //! set the data from the daemon process instead, which replaced that
    //! partition with K2's and made every Claude read prompt. A read of an
    //! item left that way is refused (never a dialog) with the one-line
    //! fix; a write repairs it. Every write is read back before `Ok`.

    /// Claude's items are CLI-owned: Claude Code reads them through
    /// `/usr/bin/security`, so a login too big for `security -i` may go
    /// on `security`'s argv, as Claude writes it itself.
    #[cfg(target_os = "macos")]
    pub const CLAUDE_ITEM: crate::macos_keychain::WriteOptions = crate::macos_keychain::WriteOptions {
        keychain: None,
        label: None,
        trusted_apps: Vec::new(),
        replace_acl: false,
        cli_owned: true,
    };

    /// Read a generic password. `Ok(None)` = no such item (or empty).
    /// Waits at most [`super::keychain_read_wait`]: a read macOS holds on
    /// a dialog keeps waiting in the background and this answers an error
    /// ([`super::bounded_keychain_read`]).
    #[cfg(target_os = "macos")]
    pub fn read(service: &str, account: &str) -> Result<Option<Vec<u8>>, String> {
        super::bounded_keychain_read(service, account, super::macos_reader())
            .map(|v| v.filter(|b| !b.is_empty()))
            .map_err(|e| e.to_string())
    }

    /// Does the item exist? Attributes only: no prompt, readable or not.
    #[cfg(target_os = "macos")]
    pub fn exists(service: &str, account: &str) -> Result<bool, String> {
        crate::macos_keychain::exists(service, account, None).map_err(|e| e.to_string())
    }

    /// Create or update by `security`, then read back and compare.
    #[cfg(target_os = "macos")]
    pub fn write(service: &str, account: &str, secret: &[u8]) -> Result<(), String> {
        crate::macos_keychain::write(service, account, secret, &CLAUDE_ITEM).map_err(|e| e.to_string())
    }

    #[cfg(target_os = "macos")]
    pub fn delete(service: &str, account: &str) -> Result<(), String> {
        crate::macos_keychain::delete(service, account, None).map_err(|e| e.to_string())
    }

    /// The item needs the partition repair (`Some`), or doesn't / is
    /// missing (`None`). In process, attributes and ACL only: no prompt.
    #[cfg(target_os = "macos")]
    pub fn needs_repair(service: &str, account: &str) -> Result<Option<crate::macos_keychain::Repair>, String> {
        match crate::macos_keychain::check_item(service, account, None) {
            Ok(crate::macos_keychain::ItemAccess::NeedsRepair(r)) => Ok(Some(r)),
            Ok(_) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    #[cfg(not(target_os = "macos"))]
    pub fn read(_service: &str, _account: &str) -> Result<Option<Vec<u8>>, String> {
        Err("the keychain is macOS-only".into())
    }

    #[cfg(not(target_os = "macos"))]
    pub fn exists(_service: &str, _account: &str) -> Result<bool, String> {
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

// ── Bounded keychain reads ──────────────────────────────────────────
//
// A keychain read is a `/usr/bin/security` child. When macOS shows a
// dialog for it ("… wants to use your confidential information stored in
// Claude Code-credentials"), the child waits until the person answers,
// and a dialog behind other windows waits forever. K2 used to wait with
// it, under the tool lock and often the database, so every later Claude
// task answered "busy" until the daemon was killed. Now a read runs on its
// own thread and the caller waits at most [`keychain_read_wait`]: past
// that it gets [`KeychainReadError::Pending`] (said to the person as
// "answer the keychain dialog, then try again") while the read keeps
// waiting for the dialog. Reads of one item share one child (no stack of
// dialogs), and an answer that comes after its caller gave up is handed
// to the next read of that item within [`LATE_ANSWER_KEEP`].

/// Why a keychain read gave no answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeychainReadError {
    /// Not answered within the wait: macOS is most likely showing a
    /// dialog for it. The read goes on in the background.
    Pending { waited: Duration },
    /// The person declined the macOS dialog.
    Declined,
    Failed(String),
}

impl std::fmt::Display for KeychainReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeychainReadError::Pending { waited } => write!(
                f,
                "macOS has not answered the keychain read after {} s (a keychain dialog may be open)",
                waited.as_secs()
            ),
            KeychainReadError::Declined => write!(f, "the macOS keychain dialog was declined"),
            KeychainReadError::Failed(e) => write!(f, "{e}"),
        }
    }
}

/// Reads one keychain item: `(service, account)` → its data.
pub type KeychainReader = dyn Fn(&str, &str) -> Result<Option<Vec<u8>>, KeychainReadError> + Send + Sync;

static KEYCHAIN_READ_WAIT_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(5_000);

/// How long a caller waits for a keychain read before it answers
/// [`KeychainReadError::Pending`].
pub fn keychain_read_wait() -> Duration {
    Duration::from_millis(KEYCHAIN_READ_WAIT_MS.load(std::sync::atomic::Ordering::Relaxed))
}

#[cfg(any(test, feature = "test-util"))]
pub fn set_keychain_read_wait(d: Duration) {
    KEYCHAIN_READ_WAIT_MS.store(d.as_millis() as u64, std::sync::atomic::Ordering::Relaxed);
}

/// An answer that came after its caller stopped waiting is kept this long
/// for the next read of the same item (the person answered the dialog,
/// then pressed Try again).
pub const LATE_ANSWER_KEEP: Duration = Duration::from_secs(120);

type ReadResult = Result<Option<Vec<u8>>, KeychainReadError>;

struct Flight {
    service: String,
    state: Mutex<FlightState>,
    cv: std::sync::Condvar,
}

struct FlightState {
    started: Instant,
    result: Option<ReadResult>,
    done_at: Option<Instant>,
    delivered: bool,
}

type FlightMap = std::collections::HashMap<String, std::sync::Arc<Flight>>;

fn flights() -> &'static Mutex<FlightMap> {
    static F: std::sync::OnceLock<Mutex<FlightMap>> = std::sync::OnceLock::new();
    F.get_or_init(|| Mutex::new(FlightMap::new()))
}

/// Read `(service, account)` with `reader`, waiting at most
/// [`keychain_read_wait`] (see the section note above).
pub fn bounded_keychain_read(service: &str, account: &str, reader: std::sync::Arc<KeychainReader>) -> ReadResult {
    use std::sync::Arc;
    // The reader is part of the key: a test's fake never joins a real read.
    let key = format!("{service}\u{0}{account}\u{0}{:p}", Arc::as_ptr(&reader) as *const ());
    let flight = {
        let mut map = flights().lock().unwrap_or_else(|p| p.into_inner());
        let reuse = map
            .get(&key)
            .filter(|f| {
                let st = f.state.lock().unwrap_or_else(|p| p.into_inner());
                match (&st.result, st.done_at) {
                    (None, _) => true,
                    (Some(_), Some(at)) => !st.delivered && at.elapsed() < LATE_ANSWER_KEEP,
                    _ => false,
                }
            })
            .cloned();
        match reuse {
            Some(f) => f,
            None => {
                let f = Arc::new(Flight {
                    service: service.to_string(),
                    state: Mutex::new(FlightState { started: Instant::now(), result: None, done_at: None, delivered: false }),
                    cv: std::sync::Condvar::new(),
                });
                map.insert(key.clone(), f.clone());
                let (w, svc, acct) = (f.clone(), service.to_string(), account.to_string());
                let spawned = std::thread::Builder::new().name("keychain-read".into()).spawn(move || {
                    let r = reader(&svc, &acct);
                    let mut st = w.state.lock().unwrap_or_else(|p| p.into_inner());
                    st.result = Some(r);
                    st.done_at = Some(Instant::now());
                    w.cv.notify_all();
                });
                if let Err(e) = spawned {
                    map.remove(&key);
                    return Err(KeychainReadError::Failed(format!("could not start the keychain read: {e}")));
                }
                f
            }
        }
    };
    let st = flight.state.lock().unwrap_or_else(|p| p.into_inner());
    let (mut st, _) = flight
        .cv
        .wait_timeout_while(st, keychain_read_wait(), |s| s.result.is_none())
        .unwrap_or_else(|p| p.into_inner());
    let Some(r) = st.result.clone() else {
        let waited = st.started.elapsed();
        crate::log_debug!(
            "[llm-accounts] keychain read of \"{service}\" not answered after {} ms (a macOS dialog may be open); K2 stopped waiting",
            waited.as_millis()
        );
        return Err(KeychainReadError::Pending { waited });
    };
    st.delivered = true;
    drop(st);
    let mut map = flights().lock().unwrap_or_else(|p| p.into_inner());
    if map.get(&key).is_some_and(|f| Arc::ptr_eq(f, &flight)) {
        map.remove(&key);
    }
    r
}

/// Keychain reads still waiting on macOS (a dialog, most likely): the
/// item and how long, for the status route and busy answers.
pub fn pending_keychain_reads() -> Vec<(String, Duration)> {
    let map = flights().lock().unwrap_or_else(|p| p.into_inner());
    map.values()
        .filter_map(|f| {
            let st = f.state.lock().unwrap_or_else(|p| p.into_inner());
            st.result.is_none().then(|| (f.service.clone(), st.started.elapsed()))
        })
        .collect()
}

/// The real reader: `/usr/bin/security` through [`crate::macos_keychain`].
#[cfg(target_os = "macos")]
fn macos_reader() -> std::sync::Arc<KeychainReader> {
    static R: std::sync::OnceLock<std::sync::Arc<KeychainReader>> = std::sync::OnceLock::new();
    R.get_or_init(|| {
        std::sync::Arc::new(|service: &str, account: &str| match crate::macos_keychain::read(service, account, None) {
            Ok(Some(b)) if b.is_empty() => Ok(None),
            Ok(v) => Ok(v),
            Err(crate::macos_keychain::KeychainError::Status { status: Some(-128), .. }) => Err(KeychainReadError::Declined),
            Err(e) => Err(KeychainReadError::Failed(e.to_string())),
        })
    })
    .clone()
}

#[cfg(any(test, feature = "test-util"))]
static TEST_READER: std::sync::RwLock<Option<std::sync::Arc<KeychainReader>>> = std::sync::RwLock::new(None);

/// Tests: read Claude's items (the live one, a slot's) with `reader`
/// instead of the keychain until the guard drops. Hold the test env lock.
#[cfg(any(test, feature = "test-util"))]
pub fn use_test_keychain_reader(reader: std::sync::Arc<KeychainReader>) -> TestKeychainReader {
    *TEST_READER.write().unwrap_or_else(|p| p.into_inner()) = Some(reader);
    TestKeychainReader(())
}

#[cfg(any(test, feature = "test-util"))]
pub struct TestKeychainReader(());

#[cfg(any(test, feature = "test-util"))]
impl Drop for TestKeychainReader {
    fn drop(&mut self) {
        *TEST_READER.write().unwrap_or_else(|p| p.into_inner()) = None;
    }
}

/// Who reads Claude's keychain items here: a test's fake, else the
/// keychain when K2 may use it ([`keychain_enabled`]), else nobody.
fn claude_reader() -> Option<std::sync::Arc<KeychainReader>> {
    #[cfg(any(test, feature = "test-util"))]
    if let Some(r) = TEST_READER.read().unwrap_or_else(|p| p.into_inner()).clone() {
        return Some(r);
    }
    #[cfg(target_os = "macos")]
    if keychain_enabled() {
        return Some(macos_reader());
    }
    None
}

/// What the person reads when macOS holds a read of Claude's login on a
/// dialog.
pub const KEYCHAIN_PROMPT_HINT: &str = "macOS is asking to let K2 read Claude's login. Answer the keychain dialog (it may be behind other windows), then try again.";
/// What the person reads when they declined that dialog.
pub const KEYCHAIN_DECLINED_HINT: &str =
    "macOS didn't let K2 read Claude's login (the keychain dialog was declined). Try again and choose Allow or Always Allow.";

/// One of Claude's keychain items (the live one, a slot's), waiting at
/// most [`keychain_read_wait`]. `Ok(None)`: no such item, or K2 doesn't
/// use the keychain here. A read macOS holds on a dialog is
/// [`WalletError::KeychainPrompt`], never a wait.
pub(crate) fn read_claude_item(service: &str, account: &str) -> Result<Option<Vec<u8>>, WalletError> {
    let Some(reader) = claude_reader() else { return Ok(None) };
    match bounded_keychain_read(service, account, reader) {
        Ok(v) => Ok(v.filter(|b| !b.is_empty())),
        Err(KeychainReadError::Pending { .. }) => Err(WalletError::KeychainPrompt(KEYCHAIN_PROMPT_HINT.into())),
        Err(KeychainReadError::Declined) => Err(WalletError::KeychainPrompt(KEYCHAIN_DECLINED_HINT.into())),
        // The error says "unlock" only when the keychain really is locked
        // (macos_keychain maps the status codes).
        Err(KeychainReadError::Failed(e)) => {
            Err(WalletError::LiveStoreUnavailable(format!("could not read Claude's keychain item: {e}")))
        }
    }
}

/// A Claude keychain item macOS won't let `/usr/bin/security` (so Claude
/// Code) read without a password dialog, and the Terminal fix.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeychainRepair {
    pub service: String,
    pub account: String,
    /// Which login it holds: "server default" or a token label.
    pub holds: String,
    pub partitions: Vec<String>,
    pub command: String,
}

/// Claude keychain items that need the partition repair: the live item
/// and each wallet slot's hashed item. Empty when K2 may not touch the
/// keychain ([`keychain_enabled`]) and off macOS. Never prompts.
pub fn keychain_repairs(slots: &[(String, String)]) -> Vec<KeychainRepair> {
    #[cfg(target_os = "macos")]
    {
        if !keychain_enabled() {
            return Vec::new();
        }
        let acct = claude_keychain_account();
        let mut items = vec![(claude_live_keychain_service(), "server default".to_string())];
        for (id, label) in slots {
            items.push((claude_keychain_service(Some(&slot_dir(Tool::Claude, id))), format!("token \"{label}\"")));
        }
        let mut out = Vec::new();
        for (svc, holds) in items {
            if let Ok(Some(r)) = keychain::needs_repair(&svc, &acct) {
                out.push(KeychainRepair { service: r.service, account: r.account, holds, partitions: r.partitions, command: r.command });
            }
        }
        out
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = slots;
        Vec::new()
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
/// Windows has no mode bits: an owner-only mode (no group/other bits, as
/// 0600 / 0700) becomes an owner-only DACL; a wider one keeps what the
/// file inherited.
#[cfg(not(unix))]
pub fn set_mode(p: &Path, mode: u32) -> std::io::Result<()> {
    if mode & 0o077 != 0 {
        return Ok(());
    }
    if p.is_dir() {
        crate::private_fs::make_private_dir(p)
    } else {
        crate::private_fs::make_private_file(p)
    }
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
        // is K2's copy and what Claude falls back to. "Exists" is checked
        // without reading the data, so an item an earlier K2 left
        // unreadable is rewritten (and so repaired), not skipped.
        let svc = claude_keychain_service(Some(&dir));
        let acct = claude_keychain_account();
        if let Ok(true) = keychain::exists(&svc, &acct) {
            keychain::write(&svc, &acct, bytes).map_err(WalletError::Io)?;
        }
    }
    write_private(&slot_cred_path(tool, id), bytes).map_err(|e| WalletError::Io(e.to_string()))
}

/// Take another account's sign-in out of a slot (slot drift): a copy goes
/// to `.removed/<why>-<tool>-<id>-<ts>-<n>/` (0700/0600, like a removed
/// token), then the slot's file and, on macOS, the slot's hashed keychain
/// item are deleted, so the token reads as signed out. The caller holds
/// the tool lock and the slot lock, and has saved the bytes elsewhere.
pub(crate) fn set_aside_slot_cred(tool: Tool, id: &str, bytes: &[u8], why: &str) -> Result<PathBuf, WalletError> {
    let dest = removed_root().join(format!("{why}-{}-{}-{}-{}", tool.as_str(), id, super::now(), &uuid::Uuid::new_v4().simple().to_string()[..6]));
    ensure_private_dir(&removed_root()).map_err(|e| WalletError::Io(e.to_string()))?;
    ensure_private_dir(&dest).map_err(|e| WalletError::Io(e.to_string()))?;
    write_private(&dest.join(tool.cred_file_name()), bytes).map_err(|e| WalletError::Io(e.to_string()))?;
    if tool == Tool::Claude && keychain_enabled() {
        let svc = claude_keychain_service(Some(&slot_dir(tool, id)));
        let acct = claude_keychain_account();
        if let Ok(true) = keychain::exists(&svc, &acct) {
            keychain::delete(&svc, &acct).map_err(WalletError::Io)?;
        }
    }
    match fs::remove_file(slot_cred_path(tool, id)) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(WalletError::Io(e.to_string())),
    }
    Ok(dest)
}

// ── Live store ──────────────────────────────────────────────────────

/// Read the tool's live login. `Ok(None)` = signed out.
pub fn read_live(tool: Tool) -> Result<Option<Vec<u8>>, WalletError> {
    if !tool.subscription_supported() {
        return Ok(None);
    }
    if tool == Tool::Claude {
        if let Some(b) = read_claude_item(&claude_live_keychain_service(), &claude_keychain_account())? {
            return Ok(Some(b));
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
    write_live_expecting(tool, bytes, None)
}

/// [`write_live`] with a compare-and-swap (prd-llm-logins-in-vault-v1
/// A3): with `expect = Some(fp)` it refuses (`live_changed`, nothing
/// written) when the live store no longer holds the bytes whose sha256 is
/// `fp` (`Some(None)`: the tool was signed out). This catches a CLI that
/// refreshed its token after the switch saved the outgoing login. Grok:
/// K2 holds Grok's own `auth.json.lock` from this read to the write.
/// Claude and Codex take no lock K2 can share, so a gap of a few
/// milliseconds remains between the read and the write.
pub(crate) fn write_live_expecting(tool: Tool, bytes: &[u8], expect: Option<Option<[u8; 32]>>) -> Result<(), WalletError> {
    validate_cred(tool, bytes).map_err(|why| invalid_cred(tool, "login to make live", why))?;
    if tool == Tool::Gemini {
        return Err(WalletError::LiveStoreUnavailable(
            "Gemini subscriptions aren't supported; use an API token".into(),
        ));
    }
    // A login `security` can't take on stdin is refused before anything
    // changes (no argv, no in-process write: see `macos_keychain`).
    #[cfg(target_os = "macos")]
    if tool == Tool::Claude && keychain_enabled() {
        let svc = claude_live_keychain_service();
        crate::macos_keychain::check_fits(&svc, &claude_keychain_account(), bytes, true).map_err(|e| {
            WalletError::LiveStoreUnavailable(format!("could not make this Claude login live ({e}); nothing changed"))
        })?;
    }
    // Grok: hold Grok's own lock across the read, the check and the
    // write (and any restore).
    let _grok_lock = if tool == Tool::Grok {
        let (home, _) = live_home(tool);
        Some(
            FileLock::acquire(&home.join("auth.json.lock"), Duration::from_secs(10))
                .map_err(|_| WalletError::LockBusy { tool: "Grok".into(), holder: Some("Grok holds its own auth.json.lock".into()) })?,
        )
    } else {
        None
    };
    // Can't read what's live → can't restore it → don't write.
    let prev = read_live(tool)?;
    if let Some(want) = expect {
        if prev.as_deref().map(sha256) != want {
            return Err(WalletError::Conflict(
                "live_changed",
                format!("{}'s own store changed during the switch (the CLI renewed its token); nothing was switched, try again", tool.display()),
            ));
        }
    }
    commit_verified(
        tool,
        bytes,
        prev.as_deref(),
        |b| write_live_raw_locked(tool, b),
        || read_live(tool),
        || clear_live(tool),
    )
}

fn sha256(b: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(b).into()
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
/// [`write_live_raw`] for a caller that already holds Grok's
/// `auth.json.lock` ([`write_live_expecting`]): flock is per open file,
/// so taking it again here would wait on ourselves.
fn write_live_raw_locked(tool: Tool, bytes: &[u8]) -> Result<(), WalletError> {
    match tool {
        Tool::Grok => write_private(&live_cred_path(tool), bytes).map_err(|e| WalletError::Io(e.to_string())),
        _ => write_live_raw(tool, bytes),
    }
}

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
                .map_err(|_| WalletError::LockBusy { tool: "Grok".into(), holder: Some("Grok holds its own auth.json.lock".into()) })?;
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
                    note_pid(&file);
                    return Ok(FileLock { file });
                }
            }
            // Windows: std's exclusive LockFileEx lock (no flock there).
            #[cfg(not(unix))]
            {
                match file.try_lock() {
                    Ok(()) => {
                        note_pid(&file);
                        return Ok(FileLock { file });
                    }
                    Err(std::fs::TryLockError::WouldBlock) => {}
                    Err(std::fs::TryLockError::Error(e)) => return Err(e),
                }
            }
            if Instant::now() >= deadline {
                return Err(std::io::Error::new(std::io::ErrorKind::WouldBlock, "lock busy"));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// Write this process's id into a lock file it just locked. Information
/// only (who to name in a busy answer): the lock is the flock, which the
/// kernel drops when the holder exits or is killed, so a stale id left in
/// the file never locks anything.
fn note_pid(file: &fs::File) {
    let mut f = file;
    let _ = file.set_len(0);
    let _ = std::io::Seek::seek(&mut f, std::io::SeekFrom::Start(0));
    let _ = writeln!(f, "{}", std::process::id());
}

/// The process id a lock file names (see [`note_pid`]).
pub fn lock_file_pid(path: &Path) -> Option<u32> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

// ── Who holds a lock ────────────────────────────────────────────────

struct Holder {
    task: String,
    since: Instant,
}

fn holders() -> &'static Mutex<std::collections::HashMap<String, Holder>> {
    static H: std::sync::OnceLock<Mutex<std::collections::HashMap<String, Holder>>> = std::sync::OnceLock::new();
    H.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

thread_local! {
    /// What this thread is doing, for busy answers ("finishing a sign-in").
    static TASK: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Name what this thread does until the guard drops; a lock it takes
/// meanwhile is reported as held by that task.
pub fn task(label: &str) -> TaskGuard {
    let prev = TASK.with(|t| t.replace(Some(label.to_string())));
    TaskGuard { prev }
}

pub struct TaskGuard {
    prev: Option<String>,
}

impl Drop for TaskGuard {
    fn drop(&mut self) {
        let prev = self.prev.take();
        TASK.with(|t| *t.borrow_mut() = prev);
    }
}

fn current_task() -> String {
    TASK.with(|t| t.borrow().clone()).unwrap_or_else(|| {
        let th = std::thread::current();
        format!("K2 thread \"{}\"", th.name().unwrap_or("unnamed"))
    })
}

fn note_holder(key: &str) {
    holders()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(key.to_string(), Holder { task: current_task(), since: Instant::now() });
}

fn clear_holder(key: &str) {
    holders().lock().unwrap_or_else(|p| p.into_inner()).remove(key);
}

/// "3 s", "2 min 5 s", "1 h 4 min".
pub fn plain_duration(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 {
        format!("{s} s")
    } else if s < 3600 {
        format!("{} min {} s", s / 60, s % 60)
    } else {
        format!("{} h {} min", s / 3600, (s % 3600) / 60)
    }
}

/// What holds lock `key` in this process ("finishing a sign-in, for
/// 4 s"), else, when the lock file is held by another process, which one.
fn holder_text(key: &str, lock_file: &Path) -> Option<String> {
    if let Some(h) = holders().lock().unwrap_or_else(|p| p.into_inner()).get(key) {
        return Some(format!("{}, for {}", h.task, plain_duration(h.since.elapsed())));
    }
    match lock_file_pid(lock_file) {
        Some(pid) if pid != std::process::id() => Some(format!(
            "another K2 process (pid {pid}) holds {}; if it is stuck, quit it",
            lock_file.display()
        )),
        _ => None,
    }
}

/// Every lock held in this process now and what holds it, plus keychain
/// reads still waiting on macOS: for the status route.
pub fn lock_report() -> serde_json::Value {
    let locks: Vec<serde_json::Value> = holders()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .map(|(k, h)| serde_json::json!({"lock": k, "task": h.task, "heldForSecs": h.since.elapsed().as_secs()}))
        .collect();
    let reads: Vec<serde_json::Value> = pending_keychain_reads()
        .into_iter()
        .map(|(svc, d)| serde_json::json!({"item": svc, "waitingForSecs": d.as_secs()}))
        .collect();
    serde_json::json!({"locks": locks, "keychainReadsWaiting": reads})
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
/// `~/.k2/llm-accounts/<tool>/.lock`. Every swap, import and login
/// finalize for that tool holds it, and every refresh holds it to write.
///
/// Lock order (the 0.46.10 subscription-switch freeze): the tool lock is
/// taken BEFORE the daemon's database lock, and never waited for while
/// the database is held. A thread that holds the database and asks for a
/// tool lock it doesn't hold yet gets one try and then `busy`
/// ([`WalletError::LockBusy`], 409): waiting there stalled every route.
/// The lock is re-entrant per thread, so the daemon takes it first and
/// the wallet code it then calls under the database takes it again for
/// free. No vendor call (refresh, usage check, profile check) runs while
/// it is held: a refresh holds the login's [`SlotLock`] for its network
/// call and this lock only for the write. A hold over
/// [`TOOL_LOCK_WARN_AFTER`] is logged with the thread's name.
pub struct ToolLock {
    pub tool: Tool,
    /// `None`: this thread already held it (re-entrant).
    held: Option<(MutexGuard<'static, ()>, FileLock)>,
    since: Instant,
}

/// A tool lock held longer than this logs a warning when it is released.
pub const TOOL_LOCK_WARN_AFTER: Duration = Duration::from_secs(2);

/// The longest a slot lock is waited for while the database is held.
pub const SLOT_WAIT_UNDER_DB: Duration = Duration::from_millis(250);

thread_local! {
    /// Tool locks this thread holds (per tool), for re-entry.
    static HELD_HERE: std::cell::Cell<[u32; 4]> = const { std::cell::Cell::new([0; 4]) };
}

fn tool_index(tool: Tool) -> usize {
    match tool {
        Tool::Claude => 0,
        Tool::Codex => 1,
        Tool::Grok => 2,
        Tool::Gemini => 3,
    }
}

fn held_here(tool: Tool) -> u32 {
    HELD_HERE.with(|h| h.get()[tool_index(tool)])
}

fn note_held(tool: Tool, taken: bool) {
    HELD_HERE.with(|h| {
        let mut v = h.get();
        let i = tool_index(tool);
        v[i] = if taken { v[i] + 1 } else { v[i].saturating_sub(1) };
        h.set(v);
    });
}

/// Does this thread hold the daemon's database lock right now?
pub fn db_held_here() -> bool {
    crate::db::try_shared().is_some_and(|h| h.is_owned_by_current_thread())
}

/// Does this thread hold `tool`'s lock?
pub fn tool_lock_held_here(tool: Tool) -> bool {
    held_here(tool) > 0
}

impl ToolLock {
    /// How long it has been held.
    pub fn held_for(&self) -> Duration {
        self.since.elapsed()
    }
}

impl Drop for ToolLock {
    fn drop(&mut self) {
        note_held(self.tool, false);
        if self.held.is_some() {
            clear_holder(self.tool.as_str());
            let held = self.since.elapsed();
            if held > TOOL_LOCK_WARN_AFTER {
                let thread = std::thread::current();
                crate::log_debug!(
                    "[llm-accounts] WARN {} lock held {} ms by thread {}",
                    self.tool.as_str(),
                    held.as_millis(),
                    thread.name().unwrap_or("unnamed")
                );
            }
        }
    }
}

/// [`lock_tool`] for a named task (what a busy answer to someone else
/// says holds it).
pub fn lock_tool_as(tool: Tool, timeout: Duration, what: &str) -> Result<ToolLock, WalletError> {
    let _t = task(what);
    lock_tool(tool, timeout)
}

pub fn lock_tool(tool: Tool, timeout: Duration) -> Result<ToolLock, WalletError> {
    if held_here(tool) > 0 {
        note_held(tool, true);
        return Ok(ToolLock { tool, held: None, since: Instant::now() });
    }
    // Never wait for it while holding the database: one try, then busy.
    let under_db = db_held_here();
    let timeout = if under_db { Duration::ZERO } else { timeout };
    let lock_file = tool_dir(tool).join(".lock");
    let busy = || {
        let holder = holder_text(tool.as_str(), &lock_file);
        crate::log_debug!(
            "[llm-accounts] {}: busy{} ({})",
            tool.as_str(),
            if under_db { ", not waiting while holding the database" } else { "" },
            holder.as_deref().unwrap_or("holder unknown")
        );
        WalletError::LockBusy { tool: tool.display().into(), holder }
    };
    let deadline = Instant::now() + timeout;
    let guard = loop {
        match mutex_for(tool).try_lock() {
            Ok(g) => break g,
            Err(std::sync::TryLockError::Poisoned(p)) => break p.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    return Err(busy());
                }
                debug_assert!(!under_db, "waited for the {} lock while holding the database", tool.as_str());
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    };
    let dir = tool_dir(tool);
    ensure_private_dir(&dir).map_err(|e| WalletError::Io(e.to_string()))?;
    let left = deadline.saturating_duration_since(Instant::now());
    let file = FileLock::acquire(&lock_file, left).map_err(|_| busy())?;
    note_held(tool, true);
    note_holder(tool.as_str());
    Ok(ToolLock { tool, held: Some((guard, file)), since: Instant::now() })
}

/// The per-slot lock: one login's credential is never swapped, refreshed
/// and handed to a spawning session at the same time. An in-process
/// mutex per slot plus a flock on `<slot>/.lock`. A refresh holds it for
/// its vendor call (never the tool lock or the database). While the
/// database is held it is waited for at most [`SLOT_WAIT_UNDER_DB`].
pub struct SlotLock {
    // Field order = drop order: the file lock, then the mutex.
    _file: Option<FileLock>,
    _guard: std::sync::MutexGuard<'static, ()>,
    key: String,
}

impl Drop for SlotLock {
    fn drop(&mut self) {
        clear_holder(&self.key);
    }
}

/// Move a slot's folder away (to `.removed/`) while its [`SlotLock`] is
/// held. Windows won't rename a folder with an open file inside it ("Access
/// is denied"), and the lock holds `<slot>/.lock` open, so there the file
/// lock is closed first; the slot's in-process mutex and the caller's tool
/// lock are still held. Elsewhere the flock stays held across the rename,
/// as before.
pub fn rename_locked_slot_dir(_lock: &mut SlotLock, dir: &Path, dest: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        _lock._file = None;
    }
    fs::rename(dir, dest)
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
    let timeout = if db_held_here() { timeout.min(SLOT_WAIT_UNDER_DB) } else { timeout };
    let deadline = Instant::now() + timeout;
    let key = format!("{}/{}", tool.as_str(), id);
    let lock_file = slot_dir(tool, id).join(".lock");
    let busy = || WalletError::LockBusy { tool: tool.display().into(), holder: holder_text(&key, &lock_file) };
    let m = slot_mutex(tool, id);
    let guard = loop {
        match m.try_lock() {
            Ok(g) => break g,
            Err(std::sync::TryLockError::Poisoned(p)) => break p.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    return Err(busy());
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    };
    let dir = slot_dir(tool, id);
    ensure_private_dir(&dir).map_err(|e| WalletError::Io(e.to_string()))?;
    let left = deadline.saturating_duration_since(Instant::now());
    let file = FileLock::acquire(&lock_file, left).map_err(|_| busy())?;
    note_holder(&key);
    Ok(SlotLock { _file: Some(file), _guard: guard, key })
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
    /// Claude's `rateLimitTier` (`max_20x`, …). Idle slots need it for the
    /// plan name: `subscriptionType` alone says "max" where the live login
    /// says "Ultra" (prd-usage-single-source-v1 §4.2).
    pub rate_limit_tier: Option<String>,
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
            m.rate_limit_tier = safe_label(o["rateLimitTier"].as_str());
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
    slot_meta_from(tool, id, read_slot(tool, id).ok().flatten().as_deref())
}

/// [`slot_meta`] from a credential the caller already read (the server
/// wallet reads through `slots`, which may answer from the vault).
pub(crate) fn slot_meta_from(tool: Tool, id: &str, cred: Option<&[u8]>) -> CredMeta {
    let mut m = match cred {
        Some(b) => parse_meta(tool, b),
        None => CredMeta::default(),
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
