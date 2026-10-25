//! Pre-trust CLI folders when a K2 workspace is added or a harness spawns.
//!
//! Merge-only grants into Claude / Codex / Grok / Gemini stores and Cursor's
//! trust marker. Never `$HOME` or `/`. Best-effort: lock/parse/EPERM logs
//! that tool and continues. See `.k2/prds/prd-cli-folder-trust-on-add-v1.md`.
//!
//! **Every grant goes through the folder-trust gate** ([`gate`],
//! prd-security-sweep-0464-v1 SS9–SS11): K2 scans the folder first
//! ([`scan`]); a clean folder is trusted as before, a risky one needs a
//! person's decision (a Thread card). The raw writers below are private to
//! this module and [`gate`]; the public entry points ([`trust_cli_folder`],
//! [`gate_harness_spawn`], [`trust_all_registered_projects`]) are gate
//! doors. K2 records which entries it wrote ([`GrantedKeys`]) so "Don't
//! trust" removes exactly those and reports the rest ([`leftover_grants`]).

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fs_atomic;

/// Risky per-project config scanner (SS8).
pub mod scan;
/// The folder-trust gate: decisions, card asks, re-scan (SS9–SS11).
pub mod gate;

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

/// Add / register door: run the gate for `abs_path` in the process HOME.
/// A clean folder is granted as before; a risky one is granted only under
/// a decision that covers it, else held (a card is presented).
pub fn trust_cli_folder(abs_path: &str) -> gate::Outcome {
    gate::ensure(abs_path, gate::Door::Add, &gate::Actor::default(), None)
}

/// Same as [`trust_cli_folder`] but writes into `home` (guest / sandbox / tests).
pub fn trust_cli_folder_in_home(abs_path: &str, home: &Path) -> gate::Outcome {
    gate::ensure(abs_path, gate::Door::Add, &gate::Actor::default(), Some(home))
}

/// Re-assert grants for every registered `projects.path` (boot sweep,
/// P13/P16/P19; `POST /cli/trust-folders`). Sweep door: only decisions that
/// already grant are re-asserted. A risky folder with no decision is never
/// granted and no card is presented in bulk.
pub fn trust_all_registered_projects() -> usize {
    let paths = match crate::projects_ops::projects_list() {
        Ok(ps) => ps.into_iter().map(|p| p.path).collect::<Vec<_>>(),
        Err(e) => {
            log_debug!("[cli-folder-trust] boot sweep: projects_list failed: {e}");
            return 0;
        }
    };
    let mut granted = 0usize;
    for p in &paths {
        if let gate::Outcome::Granted { .. } =
            gate::ensure(p, gate::Door::Sweep, &gate::Actor::by("sweep"), None)
        {
            granted += 1;
        }
    }
    log_debug!(
        "[cli-folder-trust] sweep: {granted} of {} project path(s) re-asserted",
        paths.len()
    );
    granted
}

/// Spawn door (P18 + SS11). Every program but a plain interactive login
/// shell ([`needs_gate`]): a harness with any args (`claude -p` runs
/// project hooks with no trust dialog), a wrapper, a launch-block command,
/// a shell with env or args of its own. `Ok` when the folder may run it
/// (grants written as before), `Err` with the refusal when the folder is
/// held (a card is waiting), denied, or can't be checked at all (no cwd,
/// `$HOME`, `/`, a parent of `$HOME`: review round 2 H1, a skip never
/// passes). Only the exempt plain shell passes without a decision.
///
/// `env_home`: the child's HOME when it differs from the daemon's. A guest
/// path that doesn't exist on the host gets no host write, but the decision
/// still applies (P21).
pub fn gate_harness_spawn(
    command: Option<&str>,
    args: &[String],
    env: &std::collections::HashMap<String, String>,
    cwd: Option<&Path>,
    env_home: Option<&str>,
    actor: &gate::Actor,
) -> Result<Option<gate::Outcome>, gate::Refusal> {
    if !needs_gate(command, args, env) {
        return Ok(None);
    }
    let Some(cwd) = cwd else {
        return Err(gate::refusal_unchecked("(no folder)", "the session has no folder K2 can check"));
    };
    let cwd_s = cwd.to_string_lossy();
    let outcome = match env_home.map(str::trim).filter(|s| !s.is_empty()) {
        Some(h) => {
            let home = Path::new(h);
            if home.is_dir() {
                gate::ensure(&cwd_s, gate::Door::Spawn, actor, Some(home))
            } else {
                // Guest path that does not exist on the host (P21): no host
                // write, the decision still holds.
                log_debug!(
                    "[cli-folder-trust] no host write for missing HOME {} (guest-init seeds in-cell)",
                    home.display()
                );
                gate::ensure_no_write(&cwd_s, gate::Door::Spawn, actor)
            }
        }
        None => gate::ensure(&cwd_s, gate::Door::Spawn, actor, None),
    };
    match outcome {
        gate::Outcome::Held(r) | gate::Outcome::Denied(r) => Err(r),
        // A test build's non-temp HOME writes nothing and checks nothing
        // (never in a release build); every other skip refuses.
        gate::Outcome::Skipped(why) if why == gate::SKIP_TEST_HOME => Ok(Some(gate::Outcome::Skipped(why))),
        gate::Outcome::Skipped(why) => Err(gate::refusal_unchecked(&cwd_s, &why)),
        other => Ok(Some(other)),
    }
}

/// Env keys K2 itself (or the terminal) sets on a spawn. A shell carrying
/// any other key (`PROMPT_COMMAND`, `ENV`, `ZDOTDIR`, `PATH`, a launch
/// block's env…) can run folder code before the first prompt.
fn k2_own_env_key(k: &str) -> bool {
    k.starts_with("K2_")
        || k.starts_with("K2SO_")
        || matches!(k, "TERM" | "COLORTERM" | "TERM_PROGRAM" | "TERM_PROGRAM_VERSION" | "LANG" | "LC_ALL" | "LC_CTYPE" | "COLUMNS" | "LINES")
}

/// `cmd` is the person's login shell: what K2 itself starts for a shell
/// tab ([`crate::terminal::detect_shell`]), by its path or its bare name,
/// or an absolute path listed in `/etc/shells`. A relative path
/// (`./bin/zsh`) or another name is not.
fn is_login_shell(cmd: &str) -> bool {
    let login = crate::terminal::detect_shell();
    if cmd == login {
        return true;
    }
    let p = Path::new(cmd);
    if !p.is_absolute() {
        return !cmd.contains('/') && !cmd.contains('\\') && Path::new(&login).file_name().and_then(|n| n.to_str()) == Some(cmd);
    }
    read_to_string_lossy(Path::new("/etc/shells"))
        .map(|t| t.lines().map(str::trim).any(|l| !l.starts_with('#') && l == cmd))
        .unwrap_or(false)
}

/// Whether a K2-started session in a folder must pass the folder decision
/// (reviews A-H2, round 2 H2): every program does — a wrapper (`npx`,
/// `ccr`, `happy`), `bash -c "…; claude -p"`, a launch block — except a
/// plain interactive login shell: no command / `$SHELL` / the login shell
/// ([`is_login_shell`]), no args but `-l`/`--login`/`-i`, and no env beyond
/// K2's own. That is the person's own terminal tab.
pub fn needs_gate(command: Option<&str>, args: &[String], env: &std::collections::HashMap<String, String>) -> bool {
    let plain_env = env.keys().all(|k| k2_own_env_key(k));
    let plain_args = args.iter().all(|a| matches!(a.as_str(), "-l" | "--login" | "-i"));
    let Some(cmd) = command.map(str::trim).filter(|s| !s.is_empty()) else {
        return !(plain_env && plain_args);
    };
    if cmd == "$SHELL" {
        return !(plain_env && plain_args);
    }
    let base = program_basename(cmd);
    let is_shell = LOGIN_SHELL_BASENAMES.iter().any(|s| base.eq_ignore_ascii_case(s));
    !(is_shell && is_login_shell(cmd) && plain_args && plain_env)
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

pub fn current_home() -> Option<PathBuf> {
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

/// Exact cwd plus its repo's main checkout when they differ (worktree /
/// subfolder). The main checkout is only another repo's when the link is
/// verified ([`main_checkout`]).
fn grant_keys(cwd: &Path, home: &Path) -> Vec<String> {
    let mut keys = Vec::new();
    push_key(&mut keys, cwd, home);
    if let Some(root) = main_checkout(cwd, home) {
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

/// The nearest folder at or above `path` that holds a `.git` (a folder, or
/// a worktree's or submodule's file). Read from the filesystem, no git
/// process. `$HOME`, `/` and parents of `$HOME` don't count: a dotfiles
/// repo in `$HOME` is not every folder's repo.
pub(crate) fn worktree_top(path: &Path, home: &Path) -> Option<PathBuf> {
    for d in path.ancestors() {
        if fs::symlink_metadata(d.join(".git")).is_ok() {
            if is_refused_path(d, home) {
                return None;
            }
            return Some(d.to_path_buf());
        }
    }
    None
}

/// The repo's main checkout for `path` (review A-M4). A work tree with a
/// `.git` FOLDER is its own main checkout. A `.git` FILE (`gitdir: …`) is
/// followed only when it is a real worktree of a registered workspace:
/// the gitdir is `<main>/.git/worktrees/<name>`, that folder's `gitdir`
/// back-link names this work tree's `.git`, and `<main>` is a registered
/// workspace. Otherwise (a submodule, a hand-made or planted `.git` file)
/// the folder is its own scope, so a `.git` file can't borrow another
/// repo's decision or grants.
pub(crate) fn main_checkout(path: &Path, home: &Path) -> Option<PathBuf> {
    let top = worktree_top(path, home)?;
    let dotgit = top.join(".git");
    let meta = fs::symlink_metadata(&dotgit).ok()?;
    if meta.is_file() {
        if let Some(main) = linked_main_checkout(&top) {
            if !is_refused_path(&main, home) {
                return Some(main);
            }
        }
    }
    Some(top)
}

fn linked_main_checkout(top: &Path) -> Option<PathBuf> {
    let dotgit = top.join(".git");
    let text = read_to_string_lossy(&dotgit)?;
    let gitdir = text.lines().find_map(|l| l.trim().strip_prefix("gitdir:"))?.trim().to_string();
    let gitdir = if Path::new(&gitdir).is_absolute() { PathBuf::from(&gitdir) } else { top.join(&gitdir) };
    let wt = gitdir.canonicalize().ok()?;
    let worktrees = wt.parent()?;
    if worktrees.file_name()? != "worktrees" {
        return None;
    }
    let common = worktrees.parent()?;
    if common.file_name()? != ".git" || !fs::symlink_metadata(common).map(|m| m.is_dir()).unwrap_or(false) {
        return None;
    }
    let main = common.parent()?.to_path_buf();
    let back = read_to_string_lossy(&wt.join("gitdir"))?;
    let back = PathBuf::from(back.trim());
    let back = if back.is_absolute() { back } else { wt.join(back) };
    if back.canonicalize().ok()? != dotgit.canonicalize().ok()? {
        return None;
    }
    is_registered_workspace(&main).then_some(main)
}

/// `path` is a registered workspace folder (`projects.path`, compared as
/// canonical paths).
pub(crate) fn is_registered_workspace(path: &Path) -> bool {
    let Some(want) = canonical_abs(path) else { return false };
    let Ok(projects) = crate::projects_ops::projects_list() else { return false };
    projects
        .iter()
        .any(|p| !p.path.is_empty() && canonical_abs(Path::new(&p.path)).as_deref() == Some(want.as_path()))
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

fn grant_claude(home: &Path, keys: &[String]) -> Vec<String> {
    let path = home.join(".claude.json");
    match grant_claude_inner(&path, keys) {
        Ok(changed) => changed,
        Err(e) => {
            log_debug!("[cli-folder-trust] claude {}: {e}", path.display());
            Vec::new()
        }
    }
}

fn grant_claude_inner(path: &Path, keys: &[String]) -> Result<Vec<String>, String> {
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
    let mut changed: Vec<String> = Vec::new();
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
            changed.push(key.clone());
        }
    }
    if changed.is_empty() {
        return Ok(changed);
    }
    let body = serialize_json(&root, &original);
    atomic_write_with_bak(path, body.as_bytes()).map_err(|e| e.to_string())?;
    Ok(changed)
}

// ── Codex ─────────────────────────────────────────────────────────────

fn grant_codex(home: &Path, keys: &[String]) -> Vec<String> {
    let path = home.join(".codex").join("config.toml");
    match grant_codex_inner(&path, keys) {
        Ok(changed) => changed,
        Err(e) => {
            log_debug!("[cli-folder-trust] codex {}: {e}", path.display());
            Vec::new()
        }
    }
}

fn grant_codex_inner(path: &Path, keys: &[String]) -> Result<Vec<String>, String> {
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
    let mut changed: Vec<String> = Vec::new();
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
        changed.push(key.clone());
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
        return Ok(changed);
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
        return Ok(changed);
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
    Ok(changed)
}

// ── Grok ──────────────────────────────────────────────────────────────

fn grant_grok(home: &Path, keys: &[String]) -> Vec<String> {
    let path = home.join(".grok").join("trusted_folders.toml");
    match grant_grok_inner(&path, keys) {
        Ok(changed) => changed,
        Err(e) => {
            log_debug!("[cli-folder-trust] grok {}: {e}", path.display());
            Vec::new()
        }
    }
}

fn grant_grok_inner(path: &Path, keys: &[String]) -> Result<Vec<String>, String> {
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
    let mut changed: Vec<String> = Vec::new();
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
        changed.push(key.clone());
        if entry.is_some() {
            needs_rewrite = true;
        } else {
            missing.push(key.as_str());
        }
    }
    if !needs_rewrite && missing.is_empty() {
        return Ok(changed);
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
        return Ok(changed);
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
    Ok(changed)
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

fn grant_gemini(home: &Path, keys: &[String]) -> Vec<String> {
    let path = home.join(".gemini").join("trustedFolders.json");
    match grant_gemini_inner(&path, keys) {
        Ok(changed) => changed,
        Err(e) => {
            log_debug!("[cli-folder-trust] gemini {}: {e}", path.display());
            Vec::new()
        }
    }
}

fn grant_gemini_inner(path: &Path, keys: &[String]) -> Result<Vec<String>, String> {
    let original = read_to_string_lossy(path).unwrap_or_default();
    let mut root: serde_json::Value = if original.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(&original).map_err(|e| format!("parse: {e}"))?
    };
    let obj = root
        .as_object_mut()
        .ok_or_else(|| "trustedFolders.json is not an object".to_string())?;
    let mut changed: Vec<String> = Vec::new();
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
        obj.insert(key.clone(), serde_json::Value::String("TRUST_FOLDER".into()));
        changed.push(key);
    }
    if changed.is_empty() {
        return Ok(changed);
    }
    let body = serialize_json(&root, &original);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    atomic_write_with_bak(path, body.as_bytes()).map_err(|e| e.to_string())?;
    Ok(changed)
}

// ── Cursor ────────────────────────────────────────────────────────────
//
// cursor-agent reads `<data>/projects/<slug(path)>/.workspace-trusted`,
// where `<data>` is `$CURSOR_DATA_DIR` or `~/.cursor` and the slug is the
// path with every run of non-alphanumerics turned into one `-`, trimmed
// (cursor-agent 2026.09.15 `utils/dist/workspace-paths.js` `r_`,
// `cursor-config/dist/paths.js` `Xq`; U0 finding 3). Before 0.46.4 K2 wrote
// `<ws>/.cursor/.workspace-trusted`, which Cursor never reads; that file is
// removed when it is K2's (`trusted\n`).

/// What K2 writes as `trustMethod` in a Cursor marker, so a revoke removes
/// only K2's own markers.
const CURSOR_TRUST_METHOD: &str = "k2";
/// The content of K2's pre-0.46.4 in-folder marker.
const LEGACY_CURSOR_MARKER: &[u8] = b"trusted\n";

/// Cursor's per-folder slug (`r_` in cursor-agent).
pub(crate) fn cursor_slug(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut dash = false;
    for c in path.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            dash = false;
        } else if !dash {
            out.push('-');
            dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// Cursor's data dir for `home`: `$CURSOR_DATA_DIR` when `home` is the
/// process HOME and it is set, else `<home>/.cursor`.
fn cursor_data_dir(home: &Path) -> PathBuf {
    let is_process_home = current_home()
        .and_then(|h| canonical_abs(&h))
        .is_some_and(|h| canonical_abs(home).as_deref() == Some(h.as_path()));
    if is_process_home {
        if let Some(d) = std::env::var_os("CURSOR_DATA_DIR").filter(|d| !d.is_empty()) {
            return PathBuf::from(d);
        }
    }
    home.join(".cursor")
}

/// Where cursor-agent looks for `key`'s trust marker under `home`.
pub(crate) fn cursor_marker_path(home: &Path, key: &str) -> PathBuf {
    cursor_data_dir(home)
        .join("projects")
        .join(cursor_slug(key))
        .join(".workspace-trusted")
}

/// Write Cursor's marker for each key, only when Cursor's data dir exists
/// (Cursor is installed). Returns the keys K2 wrote a marker for.
fn grant_cursor(home: &Path, keys: &[String]) -> Vec<String> {
    let mut changed = Vec::new();
    for key in keys {
        remove_legacy_cursor_marker(Path::new(key));
    }
    if !cursor_data_dir(home).is_dir() {
        return changed;
    }
    for key in keys {
        let marker = cursor_marker_path(home, key);
        if marker.exists() {
            continue;
        }
        let body = serde_json::json!({
            "trustedAt": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "workspacePath": key,
            "trustMethod": CURSOR_TRUST_METHOD,
        });
        let text = serde_json::to_string_pretty(&body).unwrap_or_default();
        let res = marker
            .parent()
            .map(fs::create_dir_all)
            .unwrap_or(Ok(()))
            .and_then(|_| fs_atomic::atomic_write(&marker, text.as_bytes()));
        match res {
            Ok(()) => changed.push(key.clone()),
            Err(e) => log_debug!("[cli-folder-trust] cursor {}: {e}", marker.display()),
        }
    }
    changed
}

/// K2's pre-0.46.4 marker inside the folder (never read by Cursor).
fn remove_legacy_cursor_marker(folder: &Path) {
    let legacy = folder.join(".cursor").join(".workspace-trusted");
    if fs::read(&legacy).ok().as_deref() == Some(LEGACY_CURSOR_MARKER) {
        if let Err(e) = fs::remove_file(&legacy) {
            log_debug!("[cli-folder-trust] legacy cursor marker {}: {e}", legacy.display());
        }
    }
}

fn cursor_marker_is_k2s(marker: &Path) -> bool {
    read_to_string_lossy(marker)
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get("trustMethod").and_then(|m| m.as_str()).map(|m| m == CURSOR_TRUST_METHOD))
        .unwrap_or(false)
}

// ── What K2 wrote, removing it, and what else still trusts a folder ──

/// The trust entries K2 itself wrote, per store (`folder_trust.granted_keys`).
/// An entry the person (or the CLI's own dialog) had before K2 wrote is
/// never listed, so "Don't trust" never removes it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantedKeys {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub claude: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub codex: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub grok: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gemini: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cursor: Vec<String>,
}

impl GrantedKeys {
    pub fn is_empty(&self) -> bool {
        self.claude.is_empty()
            && self.codex.is_empty()
            && self.grok.is_empty()
            && self.gemini.is_empty()
            && self.cursor.is_empty()
    }

    /// Union, keeping order and no duplicates.
    pub fn merge(&mut self, other: &GrantedKeys) {
        fn add(into: &mut Vec<String>, from: &[String]) {
            for k in from {
                if !into.contains(k) {
                    into.push(k.clone());
                }
            }
        }
        add(&mut self.claude, &other.claude);
        add(&mut self.codex, &other.codex);
        add(&mut self.grok, &other.grok);
        add(&mut self.gemini, &other.gemini);
        add(&mut self.cursor, &other.cursor);
    }
}

/// Write every store's grant for `keys` into `home`; returns what changed.
/// Callers are the gate only (a clean folder, or a decision that covers
/// the scan).
pub(crate) fn write_grants(home: &Path, keys: &[String]) -> GrantedKeys {
    GrantedKeys {
        claude: grant_claude(home, keys),
        codex: grant_codex(home, keys),
        grok: grant_grok(home, keys),
        gemini: grant_gemini(home, keys),
        cursor: grant_cursor(home, keys),
    }
}

/// The entries for `keys` that are trusted in `home` right now AND look
/// like K2 wrote them (0.46.3 upgrade; review B#9: never record the
/// person's own entries as K2's, so "Don't trust" never removes them):
/// - Claude: the project entry holds only `hasTrustDialogAccepted` (K2
///   makes `{}` + the flag; Claude's own dialog leaves more keys);
/// - Gemini: the path and its lowercased twin both say TRUST_FOLDER (K2's
///   own quirk; Gemini writes one key);
/// - Cursor: the marker says `trustMethod: k2`.
///
/// Codex and Grok entries look the same whoever wrote them, so they are
/// not recorded: after "Don't trust" they are reported, not removed.
pub(crate) fn upgrade_k2_grants(home: &Path, keys: &[String]) -> GrantedKeys {
    let mut gemini = Vec::new();
    for k in keys {
        let lower = k.to_lowercase();
        if lower != *k && gemini_trusted(home, k) && gemini_trusted(home, &lower) {
            gemini.push(k.clone());
            gemini.push(lower);
        }
    }
    GrantedKeys {
        claude: keys.iter().filter(|k| claude_entry_is_k2s(home, k)).cloned().collect(),
        codex: Vec::new(),
        grok: Vec::new(),
        gemini,
        cursor: keys
            .iter()
            .filter(|k| cursor_marker_path(home, k).is_file() && cursor_marker_is_k2s(&cursor_marker_path(home, k)))
            .cloned()
            .collect(),
    }
}

fn claude_entry_is_k2s(home: &Path, key: &str) -> bool {
    let Some(text) = read_to_string_lossy(&home.join(".claude.json")) else { return false };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { return false };
    let Some(entry) = v.get("projects").and_then(|p| p.get(key)).and_then(|e| e.as_object()) else { return false };
    entry.len() == 1 && entry.get("hasTrustDialogAccepted") == Some(&serde_json::Value::Bool(true))
}

/// Remove `keys`' trust from every pinned-login and wallet slot home
/// (`~/.k2/llm-accounts/{claude,codex}/<id>`): the slot's own
/// `.claude.json` and the Codex shadow `config.toml` (review item 14).
/// Returns the errors (never silent).
pub(crate) fn revoke_slot_homes(keys: &[String]) -> Vec<String> {
    let mut errors = Vec::new();
    for tool in [crate::llm_accounts::Tool::Claude, crate::llm_accounts::Tool::Codex] {
        let Ok(rd) = fs::read_dir(crate::llm_accounts::store::tool_dir(tool)) else { continue };
        for e in rd.filter_map(|e| e.ok()) {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || !e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let slot = e.path();
            let r = match tool {
                crate::llm_accounts::Tool::Claude => crate::llm_accounts::pins::strip_claude_slot_trust(&slot, keys),
                _ => crate::llm_accounts::pins::strip_codex_slot_trust(&slot, keys),
            };
            if let Err(err) = r {
                errors.push(format!("{}: {err}", slot.display()));
            }
        }
    }
    errors
}

/// Remove exactly the entries in `granted` from `home`'s stores. Errors
/// per store are collected (the gate reports them; a failed removal is
/// never silent).
pub(crate) fn revoke_grants(home: &Path, granted: &GrantedKeys) -> Vec<String> {
    let mut errors = Vec::new();
    if !granted.claude.is_empty() {
        if let Err(e) = revoke_claude(&home.join(".claude.json"), &granted.claude) {
            errors.push(format!("Claude (~/.claude.json): {e}"));
        }
    }
    if !granted.codex.is_empty() {
        if let Err(e) = revoke_toml_tables(&home.join(".codex").join("config.toml"), "projects", &granted.codex, Some("trust_level")) {
            errors.push(format!("Codex (~/.codex/config.toml): {e}"));
        }
    }
    if !granted.grok.is_empty() {
        let path = home.join(".grok").join("trusted_folders.toml");
        let res = grok_try_lock(&path).and_then(|_lock| revoke_toml_tables(&path, "folders", &granted.grok, None));
        if let Err(e) = res {
            errors.push(format!("Grok (~/.grok/trusted_folders.toml): {e}"));
        }
    }
    if !granted.gemini.is_empty() {
        if let Err(e) = revoke_gemini(&home.join(".gemini").join("trustedFolders.json"), &granted.gemini) {
            errors.push(format!("Gemini (~/.gemini/trustedFolders.json): {e}"));
        }
    }
    for key in &granted.cursor {
        let marker = cursor_marker_path(home, key);
        if marker.is_file() && cursor_marker_is_k2s(&marker) {
            if let Err(e) = fs::remove_file(&marker) {
                errors.push(format!("Cursor ({}): {e}", marker.display()));
            }
        }
    }
    errors
}

fn revoke_claude(path: &Path, keys: &[String]) -> Result<(), String> {
    let Some(original) = read_to_string_lossy(path) else {
        return Ok(());
    };
    if original.trim().is_empty() {
        return Ok(());
    }
    let mut root: serde_json::Value = serde_json::from_str(&original).map_err(|e| format!("parse: {e}"))?;
    let mut changed = false;
    if let Some(projects) = root.get_mut("projects").and_then(|p| p.as_object_mut()) {
        for key in keys {
            if let Some(entry) = projects.get_mut(key).and_then(|e| e.as_object_mut()) {
                if entry.remove("hasTrustDialogAccepted").is_some() {
                    changed = true;
                }
            }
        }
    }
    if !changed {
        return Ok(());
    }
    let body = serialize_json(&root, &original);
    atomic_write_with_bak(path, body.as_bytes()).map_err(|e| e.to_string())
}

/// Remove `[<table>."<key>"]` for each key (or only its `field`, dropping
/// the table when that leaves it empty).
fn revoke_toml_tables(path: &Path, table: &str, keys: &[String], field: Option<&str>) -> Result<(), String> {
    let Some(original) = read_to_string_lossy(path) else {
        return Ok(());
    };
    if original.trim().is_empty() {
        return Ok(());
    }
    let mut root: toml::Value = original.parse::<toml::Value>().map_err(|e| format!("parse: {e}"))?;
    let mut changed = false;
    if let Some(t) = root.get_mut(table).and_then(|t| t.as_table_mut()) {
        for key in keys {
            match field {
                Some(f) => {
                    let now_empty = match t.get_mut(key.as_str()).and_then(|e| e.as_table_mut()) {
                        Some(entry) => {
                            if entry.remove(f).is_some() {
                                changed = true;
                            }
                            entry.is_empty()
                        }
                        None => false,
                    };
                    if now_empty {
                        t.remove(key.as_str());
                    }
                }
                None => {
                    if t.remove(key.as_str()).is_some() {
                        changed = true;
                    }
                }
            }
        }
    }
    if !changed {
        return Ok(());
    }
    let body = toml::to_string(&root).map_err(|e| e.to_string())?;
    atomic_write_with_bak(path, body.as_bytes()).map_err(|e| e.to_string())
}

fn revoke_gemini(path: &Path, keys: &[String]) -> Result<(), String> {
    let Some(original) = read_to_string_lossy(path) else {
        return Ok(());
    };
    if original.trim().is_empty() {
        return Ok(());
    }
    let mut root: serde_json::Value = serde_json::from_str(&original).map_err(|e| format!("parse: {e}"))?;
    let obj = root.as_object_mut().ok_or_else(|| "trustedFolders.json is not an object".to_string())?;
    let mut changed = false;
    for key in keys {
        if obj.get(key).and_then(|v| v.as_str()) == Some("TRUST_FOLDER") {
            obj.remove(key);
            changed = true;
        }
    }
    if !changed {
        return Ok(());
    }
    let body = serialize_json(&root, &original);
    atomic_write_with_bak(path, body.as_bytes()).map_err(|e| e.to_string())
}

/// Remove every entry [`leftover_grants`] lists for `keys` (review round 2
/// M2): exact and parent-folder entries in every store, whoever wrote
/// them (the person's own, 0.46.3's), Gemini `TRUST_PARENT` entries that
/// trust a parent, and Cursor markers of any method. Returns the lines it
/// removed and the errors (never silent).
pub fn forget_grants(home: &Path, keys: &[String]) -> (Vec<String>, Vec<String>) {
    let before = leftover_grants(home, keys, true);
    let mut all: Vec<String> = Vec::new();
    for key in keys {
        let mut cur = Some(Path::new(key));
        while let Some(dir) = cur {
            let d = dir.to_string_lossy().to_string();
            if !all.contains(&d) {
                all.push(d);
            }
            cur = dir.parent();
        }
    }
    let mut gemini: Vec<String> = all.clone();
    gemini.extend(all.iter().map(|k| k.to_lowercase()));
    gemini.sort();
    gemini.dedup();
    let keys_all = GrantedKeys { claude: all.clone(), codex: all.clone(), grok: all.clone(), gemini: gemini.clone(), cursor: Vec::new() };
    let mut errors = revoke_grants(home, &keys_all);
    if let Err(e) = forget_gemini_parents(&home.join(".gemini").join("trustedFolders.json"), &all) {
        errors.push(format!("Gemini (~/.gemini/trustedFolders.json): {e}"));
    }
    for key in &all {
        let marker = cursor_marker_path(home, key);
        if marker.is_file() {
            if let Err(e) = fs::remove_file(&marker) {
                errors.push(format!("Cursor ({}): {e}", marker.display()));
            }
        }
    }
    errors.extend(revoke_slot_homes(&all));
    let after = leftover_grants(home, keys, true);
    let removed = before.into_iter().filter(|l| !after.contains(l)).collect();
    (removed, errors)
}

/// Drop Gemini `TRUST_PARENT` entries whose parent is one of `dirs`.
fn forget_gemini_parents(path: &Path, dirs: &[String]) -> Result<(), String> {
    let Some(original) = read_to_string_lossy(path) else {
        return Ok(());
    };
    if original.trim().is_empty() {
        return Ok(());
    }
    let mut root: serde_json::Value = serde_json::from_str(&original).map_err(|e| format!("parse: {e}"))?;
    let obj = root.as_object_mut().ok_or_else(|| "trustedFolders.json is not an object".to_string())?;
    let drop: Vec<String> = obj
        .iter()
        .filter(|(k, v)| {
            v.as_str() == Some("TRUST_PARENT")
                && Path::new(k.as_str()).parent().map(|p| dirs.iter().any(|d| p.to_string_lossy() == d.as_str())).unwrap_or(false)
        })
        .map(|(k, _)| k.clone())
        .collect();
    if drop.is_empty() {
        return Ok(());
    }
    for k in &drop {
        obj.remove(k);
    }
    let body = serialize_json(&root, &original);
    atomic_write_with_bak(path, body.as_bytes()).map_err(|e| e.to_string())
}

/// Every grant that still makes a CLI trust one of `keys` in `home`: an
/// exact entry in any store, or a parent folder's entry for the CLIs that
/// inherit it (Gemini and Cursor always; Claude outside git; U0 Q4). Each
/// line names the tool, the entry and its file, for the card result and
/// `status`. Used after "Don't trust" removed K2's own entries.
pub fn leftover_grants(home: &Path, keys: &[String], in_git: bool) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut push = |line: String| {
        if seen.insert(line.clone()) {
            out.push(line);
        }
    };
    for key in keys {
        if claude_trusted(home, key) {
            push(format!("Claude: {key} (~/.claude.json)"));
        }
        if codex_trusted(home, key) {
            push(format!("Codex: {key} (~/.codex/config.toml)"));
        }
        if grok_trusted(home, key) {
            push(format!("Grok: {key} (~/.grok/trusted_folders.toml)"));
        }
        if gemini_trusted(home, key) || gemini_trusted(home, &key.to_lowercase()) {
            push(format!("Gemini: {key} (~/.gemini/trustedFolders.json)"));
        }
        if cursor_marker_path(home, key).is_file() {
            push(format!("Cursor: {key} ({})", cursor_marker_path(home, key).display()));
        }
        // Every ancestor, in every store (review A-H1/B#4): a parent's
        // entry may trust this folder in a CLI that walks up.
        let _ = in_git;
        let mut cur = Path::new(key).parent();
        while let Some(dir) = cur {
            let d = dir.to_string_lossy().to_string();
            if claude_trusted(home, &d) {
                push(format!("Claude: parent folder {d} (~/.claude.json)"));
            }
            if codex_trusted(home, &d) {
                push(format!("Codex: parent folder {d} (~/.codex/config.toml)"));
            }
            if grok_trusted(home, &d) {
                push(format!("Grok: parent folder {d} (~/.grok/trusted_folders.toml)"));
            }
            if gemini_trusted(home, &d) || gemini_trusted(home, &d.to_lowercase()) {
                push(format!("Gemini: parent folder {d} (~/.gemini/trustedFolders.json)"));
            }
            if gemini_parent_trusted(home, &d) {
                push(format!("Gemini: parent folder {d} (TRUST_PARENT in ~/.gemini/trustedFolders.json)"));
            }
            if cursor_marker_path(home, &d).is_file() {
                push(format!("Cursor: parent folder {d} ({})", cursor_marker_path(home, &d).display()));
            }
            cur = dir.parent();
        }
    }
    out
}

// ── Store readers ─────────────────────────────────────────────────────

pub(crate) fn claude_trusted(home: &Path, key: &str) -> bool {
    let p = home.join(".claude.json");
    let Some(text) = read_to_string_lossy(&p) else { return false };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { return false };
    v.get("projects")
        .and_then(|p| p.get(key))
        .and_then(|e| e.get("hasTrustDialogAccepted"))
        .and_then(|v| v.as_bool())
        == Some(true)
}

pub(crate) fn codex_trusted(home: &Path, key: &str) -> bool {
    let p = home.join(".codex").join("config.toml");
    let Some(text) = read_to_string_lossy(&p) else { return false };
    let Ok(v) = text.parse::<toml::Value>() else { return false };
    v.get("projects")
        .and_then(|p| p.get(key))
        .and_then(|e| e.get("trust_level"))
        .and_then(|v| v.as_str())
        == Some("trusted")
}

pub(crate) fn grok_trusted(home: &Path, key: &str) -> bool {
    let p = home.join(".grok").join("trusted_folders.toml");
    let Some(text) = read_to_string_lossy(&p) else { return false };
    let Ok(v) = text.parse::<toml::Value>() else { return false };
    v.get("folders")
        .and_then(|p| p.get(key))
        .and_then(|e| e.get("trusted"))
        .and_then(|v| v.as_bool())
        == Some(true)
}

pub(crate) fn gemini_trusted(home: &Path, key: &str) -> bool {
    gemini_value(home, key).as_deref() == Some("TRUST_FOLDER")
}

/// Gemini's `TRUST_PARENT` on a child of `dir` trusts `dir` (its dirname).
fn gemini_parent_trusted(home: &Path, dir: &str) -> bool {
    let p = home.join(".gemini").join("trustedFolders.json");
    let Some(text) = read_to_string_lossy(&p) else { return false };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { return false };
    let Some(obj) = v.as_object() else { return false };
    obj.iter().any(|(k, val)| {
        val.as_str() == Some("TRUST_PARENT") && Path::new(k).parent().map(|p| p.to_string_lossy() == dir).unwrap_or(false)
    })
}

fn gemini_value(home: &Path, key: &str) -> Option<String> {
    let p = home.join(".gemini").join("trustedFolders.json");
    let text = read_to_string_lossy(&p)?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v.get(key).and_then(|v| v.as_str()).map(str::to_string)
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::process::Command;

    struct HomeGuard {
        home: PathBuf,
        // Holds the ONE env lock; restores HOME + removes the dir on drop.
        _temp: crate::test_env::TempHome,
    }

    impl HomeGuard {
        fn new(_label: &str) -> Self {
            let temp = crate::test_env::TempHome::new();
            Self { home: temp.path().to_path_buf(), _temp: temp }
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
        gate_harness_spawn(Some("claude"), &[], &HashMap::new(), Some(&wt), None, &gate::Actor::default()).expect("clean worktree passes");
        assert_all_stores(&g.home, &main);
        assert_all_stores(&g.home, &wt);
    }

    /// Reviews A-H2 and round 2 H2: only a plain interactive login shell
    /// (the login shell, no args but `-l`/`-i`, no env beyond K2's own)
    /// skips the gate; every other program (a wrapper, `bash -c`, `cat`, a
    /// shell with env, a relative shell path) is gated.
    #[test]
    fn spawn_helper_gates_everything_but_a_plain_shell() {
        let g = HomeGuard::new("spawn-skip");
        let ws = unique_ws(&g.home, "skip");
        let a = gate::Actor::default();
        let none: Vec<String> = Vec::new();
        let no_env: HashMap<String, String> = HashMap::new();
        let login = crate::terminal::detect_shell();
        let login_name = Path::new(&login).file_name().unwrap().to_string_lossy().to_string();
        assert!(gate_harness_spawn(None, &none, &no_env, Some(&ws), None, &a).expect("no program").is_none());
        assert!(gate_harness_spawn(Some(&login_name), &none, &no_env, Some(&ws), None, &a).expect("shell").is_none());
        assert!(gate_harness_spawn(Some("$SHELL"), &none, &no_env, Some(&ws), None, &a).expect("shell").is_none());
        assert!(gate_harness_spawn(Some(&login), &["-l".to_string()], &no_env, Some(&ws), None, &a).expect("login shell").is_none());
        let k2_env: HashMap<String, String> = [("K2_AGENT_NAME".to_string(), "x".to_string()), ("TERM".to_string(), "xterm".to_string())].into_iter().collect();
        assert!(!needs_gate(Some(&login), &none, &k2_env), "K2's own env keeps a shell plain");
        assert!(!g.home.join(".claude.json").exists(), "a plain shell writes no grant");
        let env1 = |k: &str| -> HashMap<String, String> { [(k.to_string(), "./x".to_string())].into_iter().collect() };
        for (prog, env) in [
            (Some(login.as_str()), env1("PROMPT_COMMAND")),
            (Some(login.as_str()), env1("ZDOTDIR")),
            (Some(login.as_str()), env1("ENV")),
            (Some(login.as_str()), env1("PATH")),
            (Some("$SHELL"), env1("BASH_ENV")),
            (None, env1("NODE_OPTIONS")),
        ] {
            assert!(needs_gate(prog, &none, &env), "{prog:?} {env:?}");
        }
        assert!(needs_gate(Some("./bin/zsh"), &none, &no_env), "a relative shell path runs folder code");
        assert!(needs_gate(Some("/tmp/x/bash"), &none, &no_env), "a shell outside /etc/shells");
        for (prog, args) in [
            ("cat", vec![]),
            ("/bin/bash", vec!["-c".to_string(), "claude -p hi".to_string()]),
            ("npx", vec!["@anthropic-ai/claude-code".to_string()]),
            ("ccr", vec!["code".to_string()]),
            ("happy", vec![]),
        ] {
            assert!(needs_gate(Some(prog), &args, &no_env), "{prog} {args:?}");
            let out = gate_harness_spawn(Some(prog), &args, &no_env, Some(&ws), None, &a).expect("clean folder passes");
            assert!(matches!(out, Some(gate::Outcome::Granted { clean: true, .. })), "{prog}: {out:?}");
        }
        // Round 2 H1: no folder, or one K2 can't check, refuses a gated
        // program instead of passing it.
        let r = gate_harness_spawn(Some("claude"), &none, &no_env, None, None, &a).expect_err("no folder");
        assert_eq!(r.error, gate::DENIED);
        let r = gate_harness_spawn(Some("claude"), &none, &no_env, Some(&g.home), None, &a).expect_err("$HOME");
        assert_eq!(r.error, gate::DENIED);
        assert!(r.message.contains("K2 won't start programs in"), "{}", r.message);
        assert!(gate_harness_spawn(None, &none, &no_env, Some(&g.home), None, &a).expect("a plain shell in $HOME").is_none());
        // A risky folder holds the wrapper too (no card presenter here).
        fs::create_dir_all(ws.join(".claude")).unwrap();
        fs::write(ws.join(".claude/settings.json"), r#"{"hooks":{"Stop":[{"hooks":[{"command":"./x.sh"}]}]}}"#).unwrap();
        let held = gate_harness_spawn(Some("/bin/bash"), &["-c".to_string(), "claude -p x".to_string()], &no_env, Some(&ws), None, &a)
            .expect_err("bash -c claude in a risky folder is held");
        assert_eq!(held.error, gate::PENDING);
        let held = gate_harness_spawn(Some(&login), &none, &env1("PROMPT_COMMAND"), Some(&ws), None, &a)
            .expect_err("a shell with PROMPT_COMMAND in a risky folder is held");
        assert_eq!(held.error, gate::PENDING);
    }

    #[test]
    fn spawn_helper_grants_harness_cwd() {
        let g = HomeGuard::new("spawn-ok");
        let ws = unique_ws(&g.home, "ok");
        let out = gate_harness_spawn(Some("/usr/local/bin/claude"), &[], &HashMap::new(), Some(&ws), None, &gate::Actor::default())
            .expect("clean folder passes");
        assert!(matches!(out, Some(gate::Outcome::Granted { clean: true, .. })), "{out:?}");
        assert_all_stores(&g.home, &ws);
    }

    #[test]
    fn missing_env_home_skips_host_write() {
        let g = HomeGuard::new("guest");
        let ws = unique_ws(&g.home, "guestws");
        let out = gate_harness_spawn(
            Some("claude"),
            &[],
            &HashMap::new(),
            Some(&ws),
            Some("/home/k2-does-not-exist-on-host"),
            &gate::Actor::default(),
        )
        .expect("clean folder passes");
        assert!(matches!(out, Some(gate::Outcome::Granted { .. })), "{out:?}");
        assert!(!g.home.join(".claude.json").exists());
    }

    /// U0 finding 3: cursor-agent reads `~/.cursor/projects/<slug>/
    /// .workspace-trusted`, not `<ws>/.cursor/.workspace-trusted`. K2
    /// writes the marker there only when Cursor's data dir exists, and
    /// removes its own pre-0.46.4 in-folder marker.
    #[test]
    fn cursor_marker_goes_where_cursor_reads_it() {
        let g = HomeGuard::new("cursor");
        let ws = unique_ws(&g.home, "cur dir.v2");
        let k = key(&ws);
        trust_cli_folder(&k);
        let marker = cursor_marker_path(&g.home, &k);
        assert!(!marker.exists(), "no Cursor data dir, no marker");

        fs::create_dir_all(ws.join(".cursor")).unwrap();
        fs::write(ws.join(".cursor").join(".workspace-trusted"), LEGACY_CURSOR_MARKER).unwrap();
        fs::create_dir_all(g.home.join(".cursor")).unwrap();
        trust_cli_folder(&k);
        assert!(marker.is_file(), "marker at {}", marker.display());
        let slug = marker.parent().unwrap().file_name().unwrap().to_string_lossy().to_string();
        assert_eq!(slug, cursor_slug(&k));
        assert!(!slug.contains("--") && !slug.starts_with('-') && !slug.contains(' ') && !slug.contains('.'), "{slug}");
        let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&marker).unwrap()).unwrap();
        assert_eq!(v["workspacePath"], k.as_str());
        assert_eq!(v["trustMethod"], CURSOR_TRUST_METHOD);
        assert!(!ws.join(".cursor").join(".workspace-trusted").exists(), "K2's old in-folder marker is removed");
    }

    #[test]
    fn cursor_slug_matches_cursor_agent() {
        assert_eq!(cursor_slug("/Users/z3thon/DevProjects/Alakazam Labs/K2"), "Users-z3thon-DevProjects-Alakazam-Labs-K2");
        assert_eq!(cursor_slug("/home/k2/ai/nora/"), "home-k2-ai-nora");
        assert_eq!(cursor_slug("/a//b__c"), "a-b-c");
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
