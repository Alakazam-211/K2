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
    //! `security` CLI wrappers (macOS). Callers check
    //! [`super::keychain_enabled`] first.
    use std::io::Write;
    use std::process::{Command, Stdio};

    /// Read a generic password. `Ok(None)` = no such item.
    pub fn read(service: &str, account: &str) -> Result<Option<Vec<u8>>, String> {
        let out = Command::new("/usr/bin/security")
            .args(["find-generic-password", "-a", account, "-s", service, "-w"])
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("security: {e}"))?;
        if out.status.success() {
            let raw = String::from_utf8_lossy(&out.stdout).trim_end_matches('\n').to_string();
            if raw.is_empty() {
                return Ok(None);
            }
            // `security -w` prints hex when the secret isn't printable.
            if !raw.starts_with('{') && raw.len() % 2 == 0 && raw.chars().all(|c| c.is_ascii_hexdigit()) {
                if let Some(bytes) = decode_hex(&raw) {
                    return Ok(Some(bytes));
                }
            }
            return Ok(Some(raw.into_bytes()));
        }
        // 44 = errSecItemNotFound.
        if out.status.code() == Some(44) {
            return Ok(None);
        }
        Err(format!("security find-generic-password exited {:?}", out.status.code()))
    }

    /// Create or update, the way Claude itself does: `security -i` with
    /// the payload hex-encoded on stdin (not on argv).
    pub fn write(service: &str, account: &str, secret: &[u8]) -> Result<(), String> {
        let hex: String = secret.iter().map(|b| format!("{b:02x}")).collect();
        let line = format!("add-generic-password -U -a \"{account}\" -s \"{service}\" -X \"{hex}\" \n");
        let mut child = Command::new("/usr/bin/security")
            .arg("-i")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("security: {e}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(line.as_bytes()).map_err(|e| format!("security stdin: {e}"))?;
        }
        let out = child.wait_with_output().map_err(|e| format!("security: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(format!("security add-generic-password exited {:?}", out.status.code()))
        }
    }

    pub fn delete(service: &str, account: &str) -> Result<(), String> {
        let out = Command::new("/usr/bin/security")
            .args(["delete-generic-password", "-a", account, "-s", service])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|e| format!("security: {e}"))?;
        if out.success() || out.code() == Some(44) {
            Ok(())
        } else {
            Err(format!("security delete-generic-password exited {:?}", out.code()))
        }
    }

    fn decode_hex(s: &str) -> Option<Vec<u8>> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
            .collect()
    }
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

pub fn slot_has_cred(tool: Tool, id: &str) -> bool {
    matches!(read_nonempty(&slot_cred_path(tool, id)), Ok(Some(_)))
}

pub(crate) fn read_slot(tool: Tool, id: &str) -> Result<Option<Vec<u8>>, WalletError> {
    read_nonempty(&slot_cred_path(tool, id))
}

pub(crate) fn write_slot(tool: Tool, id: &str, bytes: &[u8]) -> Result<(), WalletError> {
    let dir = slot_dir(tool, id);
    ensure_private_dir(&dir).map_err(|e| WalletError::Io(e.to_string()))?;
    write_private(&slot_cred_path(tool, id), bytes).map_err(|e| WalletError::Io(e.to_string()))
}

// ── Live store ──────────────────────────────────────────────────────

/// Read the tool's live login. `Ok(None)` = signed out.
pub(crate) fn read_live(tool: Tool) -> Result<Option<Vec<u8>>, WalletError> {
    if tool == Tool::Claude && keychain_enabled() {
        let svc = claude_live_keychain_service();
        match keychain::read(&svc, &claude_keychain_account()) {
            Ok(Some(b)) => return Ok(Some(b)),
            Ok(None) => {}
            Err(e) => return Err(WalletError::LiveStoreUnavailable(format!(
                "could not read Claude's keychain item ({e}); unlock the login keychain on this Mac"
            ))),
        }
    }
    read_nonempty(&live_cred_path(tool))
}

/// Write the tool's live login atomically. Claude on macOS: keychain
/// item, plus the fallback file when one already exists (so the two
/// never disagree). Grok: under Grok's own `auth.json.lock`.
pub(crate) fn write_live(tool: Tool, bytes: &[u8]) -> Result<(), WalletError> {
    match tool {
        Tool::Claude => {
            let file = live_cred_path(tool);
            if keychain_enabled() {
                let svc = claude_live_keychain_service();
                keychain::write(&svc, &claude_keychain_account(), bytes).map_err(|e| {
                    WalletError::LiveStoreUnavailable(format!(
                        "could not write Claude's keychain item ({e}); unlock the login keychain on this Mac"
                    ))
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

static TOOL_MUTEX: [Mutex<()>; 3] = [Mutex::new(()), Mutex::new(()), Mutex::new(())];

fn mutex_for(tool: Tool) -> &'static Mutex<()> {
    match tool {
        Tool::Claude => &TOOL_MUTEX[0],
        Tool::Codex => &TOOL_MUTEX[1],
        Tool::Grok => &TOOL_MUTEX[2],
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
