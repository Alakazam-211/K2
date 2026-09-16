//! Pre-trust CLI folders when a K2 workspace is added or a harness spawns.
//!
//! Merge-only grants into Claude / Codex / Grok / Gemini stores (and an
//! optional Cursor marker). Never `$HOME` or `/`. Best-effort: lock/parse/
//! EPERM logs that tool and continues. See `.k2/prds/prd-cli-folder-trust-on-add-v1.md`.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use crate::fs_atomic;

const HARNESS_BASENAMES: &[&str] = &["claude", "codex", "grok", "gemini", "cursor-agent"];
const LOGIN_SHELL_BASENAMES: &[&str] = &[
    "zsh",
    "bash",
    "sh",
    "fish",
    "cmd",
    "cmd.exe",
    "pwsh",
    "powershell",
    "powershell.exe",
];

/// Merge folder-trust grants for `abs_path` into the current process HOME.
///
/// Idempotent. Never errors to the caller (P20). Refuse `$HOME`, `/`, and
/// any parent of `$HOME` (P4).
pub fn trust_cli_folder(abs_path: &str) {
    let Some(home) = current_home() else {
        log_debug!("[cli-folder-trust] skip: no HOME");
        return;
    };
    trust_cli_folder_in_home(abs_path, &home);
}

/// Same as [`trust_cli_folder`] but writes into `home` (guest / sandbox / tests).
pub fn trust_cli_folder_in_home(abs_path: &str, home: &Path) {
    if abs_path.trim().is_empty() {
        return;
    }
    if skip_unisolated_home(home) {
        log_debug!(
            "[cli-folder-trust] skip: test HOME is not isolated ({})",
            home.display()
        );
        return;
    }
    let Some(cwd) = canonical_abs(Path::new(abs_path)) else {
        log_debug!("[cli-folder-trust] skip: cannot resolve {abs_path}");
        return;
    };
    let home = canonical_abs(home).unwrap_or_else(|| home.to_path_buf());
    if is_refused_path(&cwd, &home) {
        log_debug!(
            "[cli-folder-trust] refuse {} (HOME={}, / or parent of HOME)",
            cwd.display(),
            home.display()
        );
        return;
    }
    let keys = grant_keys(&cwd, &home);
    if keys.is_empty() {
        return;
    }
    grant_claude(home.as_path(), &keys);
    grant_codex(home.as_path(), &keys);
    grant_grok(home.as_path(), &keys);
    grant_gemini(home.as_path(), &keys);
    grant_cursor(&cwd);
}

/// Re-assert grants for every registered `projects.path` (boot sweep, P13/P16/P19).
pub fn trust_all_registered_projects() -> usize {
    let paths = match crate::projects_ops::projects_list() {
        Ok(ps) => ps.into_iter().map(|p| p.path).collect::<Vec<_>>(),
        Err(e) => {
            log_debug!("[cli-folder-trust] boot sweep: projects_list failed: {e}");
            return 0;
        }
    };
    let n = paths.len();
    for p in &paths {
        trust_cli_folder(p);
    }
    log_debug!("[cli-folder-trust] boot sweep: re-asserted {n} project path(s)");
    n
}

/// Spawn-door helper (P18). No-op for reuse callers (they never reach here),
/// login-shell, empty command, or non-harness binaries.
pub fn maybe_trust_harness_spawn(
    command: Option<&str>,
    cwd: Option<&Path>,
    env_home: Option<&str>,
) {
    if !is_folder_trust_harness(command) {
        return;
    }
    let Some(cwd) = cwd else {
        return;
    };
    let cwd_s = cwd.to_string_lossy();
    match env_home.map(str::trim).filter(|s| !s.is_empty()) {
        Some(h) => {
            let home = Path::new(h);
            if !home.is_dir() {
                // Guest path that does not exist on the host (P21).
                log_debug!(
                    "[cli-folder-trust] skip host write for missing HOME {} (guest-init seeds in-cell)",
                    home.display()
                );
                return;
            }
            trust_cli_folder_in_home(&cwd_s, home);
        }
        None => trust_cli_folder(&cwd_s),
    }
}

/// True when `command` is claude/codex/grok/gemini/cursor-agent (basename).
pub fn is_folder_trust_harness(command: Option<&str>) -> bool {
    let Some(cmd) = command.map(str::trim).filter(|s| !s.is_empty()) else {
        return false;
    };
    if cmd == "$SHELL" {
        return false;
    }
    let base = program_basename(cmd);
    if LOGIN_SHELL_BASENAMES
        .iter()
        .any(|s| base.eq_ignore_ascii_case(s))
    {
        return false;
    }
    HARNESS_BASENAMES
        .iter()
        .any(|s| base.eq_ignore_ascii_case(s))
}

fn program_basename(program: &str) -> String {
    let trimmed = program.trim();
    let name = Path::new(trimmed)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(trimmed);
    let lower = name.to_ascii_lowercase();
    lower.strip_suffix(".exe").unwrap_or(&lower).to_string()
}

fn current_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(dirs::home_dir)
}

/// Tests (k2-core cfg(test) or daemon test-util) must not merge throwaway
/// workspace paths into the developer's real `~/.claude.json`.
fn skip_unisolated_home(home: &Path) -> bool {
    #[cfg(any(test, feature = "test-util"))]
    {
        let tmp = std::env::temp_dir();
        let home_c = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
        let tmp_c = tmp.canonicalize().unwrap_or(tmp);
        return !home_c.starts_with(&tmp_c);
    }
    #[cfg(not(any(test, feature = "test-util")))]
    {
        let _ = home;
        false
    }
}

fn canonical_abs(path: &Path) -> Option<PathBuf> {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    match abs.canonicalize() {
        Ok(p) => Some(p),
        Err(_) => {
            // Path may not exist yet; still strip `/.` / trailing slash.
            let mut s = abs.to_string_lossy().replace('\\', "/");
            while s.len() > 1 && s.ends_with('/') {
                s.pop();
            }
            if s.is_empty() {
                None
            } else {
                Some(PathBuf::from(s))
            }
        }
    }
}

fn is_refused_path(path: &Path, home: &Path) -> bool {
    if path.parent().is_none() {
        return true;
    }
    if path == home {
        return true;
    }
    // Path is a parent of $HOME (`/Users` when HOME is `/Users/you`).
    home.starts_with(path) && home != path
}

/// Exact cwd plus git root when they differ (worktree / nested git).
fn grant_keys(cwd: &Path, home: &Path) -> Vec<String> {
    let mut keys = Vec::new();
    push_key(&mut keys, cwd, home);
    if let Some(root) = git_root(cwd) {
        push_key(&mut keys, &root, home);
    }
    keys
}

fn push_key(keys: &mut Vec<String>, path: &Path, home: &Path) {
    if is_refused_path(path, home) {
        log_debug!(
            "[cli-folder-trust] skip extra key {} (refused)",
            path.display()
        );
        return;
    }
    let s = path.to_string_lossy().to_string();
    if !keys.iter().any(|k| k == &s) {
        keys.push(s);
    }
}

fn git_root(path: &Path) -> Option<PathBuf> {
    let repo = git2::Repository::discover(path).ok()?;
    if let Some(main) = git_common_workdir(path) {
        return Some(main);
    }
    repo.workdir()
        .and_then(|w| canonical_abs(w).or_else(|| Some(w.to_path_buf())))
}

/// Main checkout for a worktree: parent of `git rev-parse --git-common-dir`.
fn git_common_workdir(path: &Path) -> Option<PathBuf> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .current_dir(path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&out.stdout);
    let common = raw.trim();
    if common.is_empty() {
        return None;
    }
    let common_path = {
        let p = Path::new(common);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            path.join(p)
        }
    };
    let common_path = canonical_abs(&common_path).unwrap_or(common_path);
    if common_path
        .file_name()
        .map(|n| n == ".git")
        .unwrap_or(false)
    {
        common_path.parent().map(|p| p.to_path_buf())
    } else {
        None
    }
}

fn bak_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(".bak");
    PathBuf::from(s)
}

fn atomic_write_with_bak(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if path.exists() {
        let bak = bak_path(path);
        if let Err(e) = fs::copy(path, &bak) {
            log_debug!("[cli-folder-trust] .bak copy {} failed: {e}", bak.display());
        }
    }
    fs_atomic::atomic_write(path, bytes)
}

fn serialize_json(value: &serde_json::Value, original: &str) -> String {
    if original.contains('\n') {
        let mut s = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
        if !s.ends_with('\n') {
            s.push('\n');
        }
        s
    } else {
        serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
    }
}

fn read_to_string_lossy(path: &Path) -> Option<String> {
    let mut f = File::open(path).ok()?;
    let mut buf = String::new();
    f.read_to_string(&mut buf).ok()?;
    Some(buf)
}

fn toml_quote(path: &str) -> String {
    format!("\"{}\"", path.replace('\\', "\\\\").replace('"', "\\\""))
}

// ── Claude ────────────────────────────────────────────────────────────

fn grant_claude(home: &Path, keys: &[String]) {
    let path = home.join(".claude.json");
    match grant_claude_inner(&path, keys) {
        Ok(()) => {}
        Err(e) => log_debug!("[cli-folder-trust] claude {}: {e}", path.display()),
    }
}

fn grant_claude_inner(path: &Path, keys: &[String]) -> Result<(), String> {
    let original = read_to_string_lossy(path).unwrap_or_default();
    let mut root: serde_json::Value = if original.trim().is_empty() {
        serde_json::json!({"projects": {}})
    } else {
        serde_json::from_str(&original).map_err(|e| format!("parse: {e}"))?
    };
    let obj = root
        .as_object_mut()
        .ok_or_else(|| "claude.json is not an object".to_string())?;
    let projects = obj
        .entry("projects")
        .or_insert_with(|| serde_json::json!({}));
    let projects = projects
        .as_object_mut()
        .ok_or_else(|| "projects is not an object".to_string())?;
    let mut changed = false;
    for key in keys {
        let entry = projects
            .entry(key.clone())
            .or_insert_with(|| serde_json::json!({}));
        if !entry.is_object() {
            *entry = serde_json::json!({});
        }
        let proj = entry
            .as_object_mut()
            .ok_or_else(|| "project entry is not an object".to_string())?;
        let already = proj
            .get("hasTrustDialogAccepted")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !already {
            proj.insert(
                "hasTrustDialogAccepted".into(),
                serde_json::Value::Bool(true),
            );
            changed = true;
        }
    }
    if !changed {
        return Ok(());
    }
    let body = serialize_json(&root, &original);
    atomic_write_with_bak(path, body.as_bytes()).map_err(|e| e.to_string())?;
    Ok(())
}

// ── Codex ─────────────────────────────────────────────────────────────

fn grant_codex(home: &Path, keys: &[String]) {
    let path = home.join(".codex").join("config.toml");
    match grant_codex_inner(&path, keys) {
        Ok(()) => {}
        Err(e) => log_debug!("[cli-folder-trust] codex {}: {e}", path.display()),
    }
}

fn grant_codex_inner(path: &Path, keys: &[String]) -> Result<(), String> {
    let original = read_to_string_lossy(path).unwrap_or_default();
    let parsed: Option<toml::Value> = if original.trim().is_empty() {
        Some(toml::Value::Table(toml::Table::new()))
    } else {
        match original.parse::<toml::Value>() {
            Ok(v) => Some(v),
            Err(e) => {
                return Err(format!("parse: {e}"));
            }
        }
    };
    let table = parsed.as_ref().and_then(|v| v.as_table());
    let mut missing: Vec<&str> = Vec::new();
    let mut needs_rewrite = false;
    for key in keys {
        let trusted = table
            .and_then(|t| t.get("projects"))
            .and_then(|p| p.get(key.as_str()))
            .and_then(|e| e.get("trust_level"))
            .and_then(|v| v.as_str())
            == Some("trusted");
        if trusted {
            continue;
        }
        let exists = table
            .and_then(|t| t.get("projects"))
            .and_then(|p| p.get(key.as_str()))
            .is_some();
        if exists {
            needs_rewrite = true;
        } else {
            missing.push(key.as_str());
        }
    }
    if !needs_rewrite && missing.is_empty() {
        return Ok(());
    }
    if needs_rewrite {
        let mut root: toml::Value = original
            .parse::<toml::Value>()
            .unwrap_or_else(|_| toml::Value::Table(toml::Table::new()));
        let projects = root
            .as_table_mut()
            .ok_or_else(|| "codex config is not a table".to_string())?
            .entry("projects".to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        let projects = projects
            .as_table_mut()
            .ok_or_else(|| "projects is not a table".to_string())?;
        for key in keys {
            let entry = projects
                .entry(key.clone())
                .or_insert_with(|| toml::Value::Table(toml::Table::new()));
            let entry = entry
                .as_table_mut()
                .ok_or_else(|| "project entry is not a table".to_string())?;
            entry.insert("trust_level".into(), toml::Value::String("trusted".into()));
        }
        let body = toml::to_string(&root).map_err(|e| e.to_string())?;
        atomic_write_with_bak(path, body.as_bytes()).map_err(|e| e.to_string())?;
        return Ok(());
    }
    let mut append = String::new();
    if !original.is_empty() && !original.ends_with('\n') {
        append.push('\n');
    }
    for key in missing {
        append.push('\n');
        append.push_str(&format!(
            "[projects.{}]\ntrust_level = \"trusted\"\n",
            toml_quote(key)
        ));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    if original.is_empty() {
        atomic_write_with_bak(path, append.trim_start().as_bytes()).map_err(|e| e.to_string())?;
    } else {
        let mut body = original;
        body.push_str(&append);
        atomic_write_with_bak(path, body.as_bytes()).map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ── Grok ──────────────────────────────────────────────────────────────

fn grant_grok(home: &Path, keys: &[String]) {
    let path = home.join(".grok").join("trusted_folders.toml");
    match grant_grok_inner(&path, keys) {
        Ok(()) => {}
        Err(e) => log_debug!("[cli-folder-trust] grok {}: {e}", path.display()),
    }
}

fn grant_grok_inner(path: &Path, keys: &[String]) -> Result<(), String> {
    let _lock = match grok_try_lock(path) {
        Ok(l) => l,
        Err(e) => return Err(e),
    };
    let original = read_to_string_lossy(path).unwrap_or_default();
    let parsed: Option<toml::Value> = if original.trim().is_empty() {
        Some(toml::Value::Table(toml::Table::new()))
    } else {
        match original.parse::<toml::Value>() {
            Ok(v) => Some(v),
            Err(e) => return Err(format!("parse: {e}")),
        }
    };
    let table = parsed.as_ref().and_then(|v| v.as_table());
    let now = chrono::Utc::now().timestamp();
    let mut missing: Vec<&str> = Vec::new();
    let mut needs_rewrite = false;
    for key in keys {
        let entry = table
            .and_then(|t| t.get("folders"))
            .and_then(|p| p.get(key.as_str()));
        let trusted = entry
            .and_then(|e| e.get("trusted"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if trusted {
            continue;
        }
        if entry.is_some() {
            needs_rewrite = true;
        } else {
            missing.push(key.as_str());
        }
    }
    if !needs_rewrite && missing.is_empty() {
        return Ok(());
    }
    if needs_rewrite {
        let mut root: toml::Value = original
            .parse::<toml::Value>()
            .unwrap_or_else(|_| toml::Value::Table(toml::Table::new()));
        let folders = root
            .as_table_mut()
            .ok_or_else(|| "grok store is not a table".to_string())?
            .entry("folders".to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        let folders = folders
            .as_table_mut()
            .ok_or_else(|| "folders is not a table".to_string())?;
        for key in keys {
            let entry = folders
                .entry(key.clone())
                .or_insert_with(|| toml::Value::Table(toml::Table::new()));
            let entry = entry
                .as_table_mut()
                .ok_or_else(|| "folder entry is not a table".to_string())?;
            entry.insert("trusted".into(), toml::Value::Boolean(true));
            if !entry.contains_key("decided_at") {
                entry.insert("decided_at".into(), toml::Value::Integer(now));
            }
        }
        let body = toml::to_string(&root).map_err(|e| e.to_string())?;
        atomic_write_with_bak(path, body.as_bytes()).map_err(|e| e.to_string())?;
        return Ok(());
    }
    let mut append = String::new();
    if !original.is_empty() && !original.ends_with('\n') {
        append.push('\n');
    }
    for key in missing {
        append.push('\n');
        append.push_str(&format!(
            "[folders.{}]\ntrusted = true\ndecided_at = {now}\n",
            toml_quote(key)
        ));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    if original.is_empty() {
        atomic_write_with_bak(path, append.trim_start().as_bytes()).map_err(|e| e.to_string())?;
    } else {
        let mut body = original;
        body.push_str(&append);
        atomic_write_with_bak(path, body.as_bytes()).map_err(|e| e.to_string())?;
    }
    Ok(())
}

struct GrokLock(Option<File>);

fn grok_try_lock(store: &Path) -> Result<GrokLock, String> {
    let parent = store.parent().ok_or_else(|| "no parent".to_string())?;
    fs::create_dir_all(parent).map_err(|e| format!("create .grok: {e}"))?;
    let lock_path = parent.join("trusted_folders.toml.lock");
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .read(true)
        .open(&lock_path)
        .map_err(|e| format!("open grok lock: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc != 0 {
            return Err("grok lock busy (skip)".into());
        }
    }
    Ok(GrokLock(Some(file)))
}

impl Drop for GrokLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(file) = self.0.take() {
            use std::os::unix::io::AsRawFd;
            unsafe {
                libc::flock(file.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }
}

// ── Gemini ────────────────────────────────────────────────────────────

fn grant_gemini(home: &Path, keys: &[String]) {
    let path = home.join(".gemini").join("trustedFolders.json");
    match grant_gemini_inner(&path, keys) {
        Ok(()) => {}
        Err(e) => log_debug!("[cli-folder-trust] gemini {}: {e}", path.display()),
    }
}

fn grant_gemini_inner(path: &Path, keys: &[String]) -> Result<(), String> {
    let original = read_to_string_lossy(path).unwrap_or_default();
    let mut root: serde_json::Value = if original.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(&original).map_err(|e| format!("parse: {e}"))?
    };
    let obj = root
        .as_object_mut()
        .ok_or_else(|| "trustedFolders.json is not an object".to_string())?;
    let mut changed = false;
    let mut to_set: Vec<String> = Vec::new();
    for key in keys {
        to_set.push(key.clone());
        let lower = key.to_lowercase();
        if lower != *key {
            to_set.push(lower);
        }
    }
    for key in to_set {
        let already = obj.get(&key).and_then(|v| v.as_str()) == Some("TRUST_FOLDER");
        if already {
            continue;
        }
        obj.insert(key, serde_json::Value::String("TRUST_FOLDER".into()));
        changed = true;
    }
    if !changed {
        return Ok(());
    }
    let body = serialize_json(&root, &original);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    atomic_write_with_bak(path, body.as_bytes()).map_err(|e| e.to_string())?;
    Ok(())
}

// ── Cursor ────────────────────────────────────────────────────────────

fn grant_cursor(workspace: &Path) {
    let cursor_dir = workspace.join(".cursor");
    if !cursor_dir.is_dir() {
        return;
    }
    let marker = cursor_dir.join(".workspace-trusted");
    if marker.exists() {
        return;
    }
    if let Err(e) = atomic_write_with_bak(&marker, b"trusted\n") {
        log_debug!("[cli-folder-trust] cursor {}: {e}", marker.display());
    }
}

/// Test helper: read grant flags from a HOME tree.
#[cfg(test)]
fn claude_trusted(home: &Path, key: &str) -> bool {
    let p = home.join(".claude.json");
    let text = read_to_string_lossy(&p).unwrap_or_default();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::json!({}));
    v.get("projects")
        .and_then(|p| p.get(key))
        .and_then(|e| e.get("hasTrustDialogAccepted"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

#[cfg(test)]
fn codex_trusted(home: &Path, key: &str) -> bool {
    let p = home.join(".codex").join("config.toml");
    let text = read_to_string_lossy(&p).unwrap_or_default();
    let v: toml::Value = text
        .parse()
        .unwrap_or(toml::Value::Table(Default::default()));
    v.get("projects")
        .and_then(|p| p.get(key))
        .and_then(|e| e.get("trust_level"))
        .and_then(|v| v.as_str())
        == Some("trusted")
}

#[cfg(test)]
fn grok_trusted(home: &Path, key: &str) -> bool {
    let p = home.join(".grok").join("trusted_folders.toml");
    let text = read_to_string_lossy(&p).unwrap_or_default();
    let v: toml::Value = text
        .parse()
        .unwrap_or(toml::Value::Table(Default::default()));
    v.get("folders")
        .and_then(|p| p.get(key))
        .and_then(|e| e.get("trusted"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

#[cfg(test)]
fn gemini_trusted(home: &Path, key: &str) -> bool {
    let p = home.join(".gemini").join("trustedFolders.json");
    let text = read_to_string_lossy(&p).unwrap_or_default();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::json!({}));
    v.get(key).and_then(|v| v.as_str()) == Some("TRUST_FOLDER")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::process::Command;

    struct HomeGuard {
        original: Option<OsString>,
        home: PathBuf,
        _lock: parking_lot::MutexGuard<'static, ()>,
    }

    impl HomeGuard {
        fn new(label: &str) -> Self {
            let lock = crate::themes::HOME_LOCK.lock();
            let home = std::env::temp_dir().join(format!(
                "k2-cli-trust-{label}-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4()
            ));
            fs::create_dir_all(&home).unwrap();
            let original = std::env::var_os("HOME");
            std::env::set_var("HOME", &home);
            Self {
                original,
                home,
                _lock: lock,
            }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match self.original.take() {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            let _ = fs::remove_dir_all(&self.home);
        }
    }

    fn unique_ws(home: &Path, label: &str) -> PathBuf {
        let dir = home.join(format!("ws-{label}"));
        fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    fn key(p: &Path) -> String {
        p.to_string_lossy().into_owned()
    }

    fn assert_all_stores(home: &Path, path: &Path) {
        let k = key(path);
        assert!(claude_trusted(home, &k), "claude missing {k}");
        assert!(codex_trusted(home, &k), "codex missing {k}");
        assert!(grok_trusted(home, &k), "grok missing {k}");
        assert!(gemini_trusted(home, &k), "gemini missing {k}");
    }

    #[test]
    fn harness_detects_basenames_and_skips_shells() {
        assert!(is_folder_trust_harness(Some("claude")));
        assert!(is_folder_trust_harness(Some("/opt/homebrew/bin/codex")));
        assert!(is_folder_trust_harness(Some("grok")));
        assert!(is_folder_trust_harness(Some("gemini")));
        assert!(is_folder_trust_harness(Some("cursor-agent")));
        assert!(is_folder_trust_harness(Some("claude.exe")));
        assert!(!is_folder_trust_harness(None));
        assert!(!is_folder_trust_harness(Some("")));
        assert!(!is_folder_trust_harness(Some("$SHELL")));
        assert!(!is_folder_trust_harness(Some("zsh")));
        assert!(!is_folder_trust_harness(Some("/bin/bash")));
        assert!(!is_folder_trust_harness(Some("pi")));
        assert!(!is_folder_trust_harness(Some("cat")));
    }

    #[test]
    fn add_workspace_writes_each_store_key() {
        let g = HomeGuard::new("add");
        crate::db::init_for_tests();
        let ws = unique_ws(&g.home, "proj");
        let path = key(&ws);
        crate::workspace::lifecycle::register_workspace_ex(&path, false, false, false)
            .expect("register");
        assert_all_stores(&g.home, &ws);
        assert!(
            !ws.join(".cursor").join(".workspace-trusted").exists(),
            "cursor marker only when .cursor/ exists"
        );
    }

    #[test]
    fn refuse_home_and_slash_and_parent_of_home() {
        let g = HomeGuard::new("refuse");
        let home = g.home.canonicalize().unwrap();
        let home_s = key(&home);
        trust_cli_folder(&home_s);
        trust_cli_folder("/");
        if let Some(parent) = home.parent() {
            trust_cli_folder(&key(parent));
        }
        assert!(!claude_trusted(&g.home, &home_s));
        assert!(!codex_trusted(&g.home, &home_s));
        assert!(!grok_trusted(&g.home, &home_s));
        assert!(!gemini_trusted(&g.home, &home_s));
        assert!(!claude_trusted(&g.home, "/"));
        assert!(
            !g.home.join(".claude.json").exists() || {
                let text = read_to_string_lossy(&g.home.join(".claude.json")).unwrap_or_default();
                !text.contains(&format!("\"{home_s}\"")) || !claude_trusted(&g.home, &home_s)
            }
        );
    }

    #[test]
    fn second_call_is_idempotent() {
        let g = HomeGuard::new("idem");
        let ws = unique_ws(&g.home, "idem");
        let path = key(&ws);
        trust_cli_folder(&path);
        assert_all_stores(&g.home, &ws);
        let claude_1 = fs::read(&g.home.join(".claude.json")).unwrap();
        let codex_1 = fs::read(&g.home.join(".codex").join("config.toml")).unwrap();
        let grok_1 = fs::read(&g.home.join(".grok").join("trusted_folders.toml")).unwrap();
        let gemini_1 = fs::read(&g.home.join(".gemini").join("trustedFolders.json")).unwrap();
        trust_cli_folder(&path);
        assert_eq!(fs::read(&g.home.join(".claude.json")).unwrap(), claude_1);
        assert_eq!(
            fs::read(&g.home.join(".codex").join("config.toml")).unwrap(),
            codex_1
        );
        assert_eq!(
            fs::read(&g.home.join(".grok").join("trusted_folders.toml")).unwrap(),
            grok_1
        );
        assert_eq!(
            fs::read(&g.home.join(".gemini").join("trustedFolders.json")).unwrap(),
            gemini_1
        );
    }

    #[test]
    fn worktree_cwd_writes_second_key() {
        let g = HomeGuard::new("wt");
        crate::db::init_for_tests();
        let main = unique_ws(&g.home, "main");
        let git_ok = Command::new("git")
            .args(["init", "--initial-branch=main"])
            .current_dir(&main)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert!(git_ok, "git init");
        let commit_ok = Command::new("git")
            .args([
                "-c",
                "user.email=k2@test",
                "-c",
                "user.name=k2",
                "commit",
                "--allow-empty",
                "-m",
                "init",
            ])
            .current_dir(&main)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert!(commit_ok, "git commit");
        let wt = g.home.join("wt-checkout");
        let wt_ok = Command::new("git")
            .args(["worktree", "add", wt.to_str().unwrap(), "-b", "feat"])
            .current_dir(&main)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert!(wt_ok, "git worktree add");
        let wt = wt.canonicalize().unwrap();
        let main_s = key(&main);
        crate::workspace::lifecycle::register_workspace_ex(&main_s, false, false, false)
            .expect("register");
        assert_all_stores(&g.home, &main);
        maybe_trust_harness_spawn(Some("claude"), Some(&wt), None);
        assert_all_stores(&g.home, &main);
        assert_all_stores(&g.home, &wt);
    }

    #[test]
    fn spawn_helper_skips_empty_and_shell() {
        let g = HomeGuard::new("spawn-skip");
        let ws = unique_ws(&g.home, "skip");
        maybe_trust_harness_spawn(None, Some(&ws), None);
        maybe_trust_harness_spawn(Some("zsh"), Some(&ws), None);
        maybe_trust_harness_spawn(Some("cat"), Some(&ws), None);
        assert!(!g.home.join(".claude.json").exists());
    }

    #[test]
    fn spawn_helper_grants_harness_cwd() {
        let g = HomeGuard::new("spawn-ok");
        let ws = unique_ws(&g.home, "ok");
        maybe_trust_harness_spawn(Some("/usr/local/bin/claude"), Some(&ws), None);
        assert_all_stores(&g.home, &ws);
    }

    #[test]
    fn missing_env_home_skips_host_write() {
        let g = HomeGuard::new("guest");
        let ws = unique_ws(&g.home, "guestws");
        maybe_trust_harness_spawn(
            Some("claude"),
            Some(&ws),
            Some("/home/k2-does-not-exist-on-host"),
        );
        assert!(!g.home.join(".claude.json").exists());
    }

    #[test]
    fn cursor_marker_only_when_dot_cursor_exists() {
        let g = HomeGuard::new("cursor");
        let ws = unique_ws(&g.home, "cur");
        trust_cli_folder(&key(&ws));
        assert!(!ws.join(".cursor").join(".workspace-trusted").exists());
        fs::create_dir_all(ws.join(".cursor")).unwrap();
        trust_cli_folder(&key(&ws));
        assert!(ws.join(".cursor").join(".workspace-trusted").is_file());
    }

    #[test]
    fn claude_flips_false_to_true_without_dropping_siblings() {
        let g = HomeGuard::new("flip");
        let ws = unique_ws(&g.home, "flip");
        let k = key(&ws);
        let existing = serde_json::json!({
            "hasCompletedOnboarding": true,
            "projects": {
                k.clone(): {
                    "hasTrustDialogAccepted": false,
                    "allowedTools": ["Read"],
                }
            }
        });
        fs::write(
            g.home.join(".claude.json"),
            serde_json::to_string_pretty(&existing).unwrap(),
        )
        .unwrap();
        trust_cli_folder(&k);
        assert!(claude_trusted(&g.home, &k));
        let text = read_to_string_lossy(&g.home.join(".claude.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["hasCompletedOnboarding"], true);
        assert_eq!(v["projects"][&k]["allowedTools"][0], "Read");
        assert!(g.home.join(".claude.json.bak").is_file());
    }

    #[test]
    fn add_from_path_and_without_git_also_grant() {
        let g = HomeGuard::new("addpath");
        crate::db::init_for_tests();
        let git_ws = unique_ws(&g.home, "git");
        assert!(Command::new("git")
            .args(["init", "--initial-branch=main"])
            .current_dir(&git_ws)
            .status()
            .unwrap()
            .success());
        match crate::projects_ops::projects_add_from_path_ex(&key(&git_ws), false, false, false) {
            Ok(crate::projects_ops::AddFromPathResult::Project(_)) => {}
            Ok(crate::projects_ops::AddFromPathResult::NeedsGitInit { .. }) => {
                panic!("add-from-path: needs git init")
            }
            Err(e) => panic!("add-from-path: {e}"),
        }
        assert_all_stores(&g.home, &git_ws);

        let plain = unique_ws(&g.home, "plain");
        crate::projects_ops::projects_add_without_git_ex(&key(&plain), false, false, false)
            .expect("without git");
        assert_all_stores(&g.home, &plain);
    }
}
