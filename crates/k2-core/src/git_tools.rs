//! First run without git (0.46.4, Rosson 2026-10-26).
//!
//! On a fresh Mac `/usr/bin/git` is Apple's shim: until the Command Line
//! Tools (CLT, about 1–2 GB; full Xcode is not needed) are installed,
//! every git call fails and opens Apple's install dialog. A first-time
//! owner adding a workspace used to get
//! `git config --global user.name failed: xcode-select: note: No developer
//! tools were found…` and no workspace. On Linux a box can simply have no
//! git.
//!
//! [`crate::git_safe::availability`] says whether git runs. This module is
//! what the daemon does while it doesn't:
//!
//! - **Deferred steps.** Add workspace never fails for want of git. "Init
//!   git" adds the workspace at once and leaves a marker,
//!   `<workspace>/.k2/git-deferred.json` (the branch to create). Once git
//!   runs, [`run_deferred`] does what was skipped: the global
//!   `user.name`/`user.email` seed, `git init`, the empty first commit.
//!   The daemon's watch loop and every PTY spawn in that folder retry it.
//! - **Install.** [`start_apple_install`] runs `xcode-select --install`,
//!   which only opens Apple's own installer dialog (never sudo). The
//!   headless path (`softwareupdate`) needs admin, so K2 only shows it as
//!   steps ([`apple_headless_steps`]). Linux gets the package-manager line
//!   for its distro ([`linux_install_command`]); K2 installs nothing there.
//! - **Agents' note.** A PTY spawned while git doesn't run gets
//!   `~/.k2/bin/git-wait` first on PATH. Its `git` runs the real one as
//!   soon as there is one, and until then prints one line ("git isn't
//!   installed yet; the owner is installing Apple's command line tools")
//!   instead of Apple's dialog and xcrun error.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::git_safe::{self, GitProbe};

/// What an agent sees when it runs git on a Mac without the CLT.
pub const AGENT_NOTE_MAC: &str = "git isn't installed yet; the owner is installing Apple's command line tools";

/// How a workspace whose git steps wait is marked (macOS).
pub const WAITING_MARK_MAC: &str = "git: waiting for Apple's command line tools";
/// How a workspace whose git steps wait is marked (Linux and others).
pub const WAITING_MARK_OTHER: &str = "git: waiting for git to be installed";

pub fn waiting_mark() -> &'static str {
    if cfg!(target_os = "macos") {
        WAITING_MARK_MAC
    } else {
        WAITING_MARK_OTHER
    }
}

/// What an agent sees when it runs git while git isn't installed.
pub fn agent_note() -> String {
    if cfg!(target_os = "macos") {
        AGENT_NOTE_MAC.to_string()
    } else {
        format!(
            "git isn't installed on this machine yet; the owner can install it with: {}",
            linux_install_command(&std::fs::read_to_string("/etc/os-release").unwrap_or_default())
        )
    }
}

// ── Deferred git steps ──────────────────────────────────────────────────

/// The skipped steps for one workspace folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Deferred {
    /// The branch `git init` creates.
    pub branch: String,
    /// Unix seconds when the steps were deferred.
    pub deferred_at: i64,
}

/// Some workspace may have a marker; cleared when a scan finds none.
static MAYBE_PENDING: AtomicBool = AtomicBool::new(true);

pub fn marker_path(root: &Path) -> PathBuf {
    root.join(".k2").join("git-deferred.json")
}

pub fn read_deferred(root: &Path) -> Option<Deferred> {
    let text = std::fs::read_to_string(marker_path(root)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Leave the marker: `git init` on `branch` (and the identity seed) wait
/// for git.
pub fn defer_init(root: &Path, branch: &str) -> Result<(), String> {
    let path = marker_path(root);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }
    let d = Deferred {
        branch: branch.to_string(),
        deferred_at: SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0),
    };
    let text = serde_json::to_string_pretty(&d).map_err(|e| e.to_string())?;
    crate::fs_atomic::atomic_write_str(&path, &text).map_err(|e| format!("write {}: {e}", path.display()))?;
    MAYBE_PENDING.store(true, Ordering::Relaxed);
    Ok(())
}

pub fn clear_deferred(root: &Path) {
    let _ = std::fs::remove_file(marker_path(root));
}

/// After a failed run, wait this long before the watch loop tries that
/// folder again (a spawn or a recheck still tries at once).
const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(300);

static LAST_FAILURE: Mutex<Vec<(PathBuf, Instant)>> = Mutex::new(Vec::new());

/// What [`run_deferred`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeferredRun {
    /// No marker in this folder.
    NothingWaiting,
    /// git still doesn't run; the marker stays.
    GitNotReady,
    /// Tried too recently after a failure (watch loop only).
    Backoff,
    /// The steps ran; the folder is a repo with a first commit on `branch`.
    Done { branch: String },
    /// The folder became a repo some other way; the marker is gone.
    AlreadyARepo,
}

/// Do the skipped git steps for `root` if git runs now. `force` skips the
/// failure backoff.
pub fn run_deferred(root: &Path, force: bool) -> Result<DeferredRun, String> {
    let Some(d) = read_deferred(root) else {
        return Ok(DeferredRun::NothingWaiting);
    };
    if !git_safe::availability().is_ready() {
        return Ok(DeferredRun::GitNotReady);
    }
    {
        let mut fails = LAST_FAILURE.lock().unwrap_or_else(|e| e.into_inner());
        fails.retain(|(_, at)| at.elapsed() < RETRY_AFTER_FAILURE);
        if !force && fails.iter().any(|(p, _)| p == root) {
            return Ok(DeferredRun::Backoff);
        }
    }
    if root.join(".git").exists() {
        clear_deferred(root);
        return Ok(DeferredRun::AlreadyARepo);
    }
    let root_str = root.to_string_lossy().to_string();
    match crate::projects_ops::init_and_first_commit(&root_str, &d.branch) {
        Ok(()) => {
            clear_deferred(root);
            crate::log_debug!("[git-tools] {root_str}: deferred git init on {} done", d.branch);
            Ok(DeferredRun::Done { branch: d.branch })
        }
        Err(e) => {
            LAST_FAILURE.lock().unwrap_or_else(|e| e.into_inner()).push((root.to_path_buf(), Instant::now()));
            crate::log_debug!("[git-tools] {root_str}: deferred git init failed: {e}");
            Err(e)
        }
    }
}

/// A registered workspace (project root) for the scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitingWorkspace {
    pub project_id: String,
    pub name: String,
    pub path: String,
    pub branch: String,
    pub note: String,
}

/// Registered workspaces whose git steps wait.
pub fn waiting_workspaces() -> Vec<WaitingWorkspace> {
    let projects = {
        let db = crate::db::shared();
        let conn = db.lock();
        crate::db::schema::Project::list(&conn).unwrap_or_default()
    };
    projects
        .into_iter()
        .filter_map(|p| {
            let d = read_deferred(Path::new(&p.path))?;
            Some(WaitingWorkspace {
                project_id: p.id,
                name: p.name,
                path: p.path,
                branch: d.branch,
                note: waiting_mark().to_string(),
            })
        })
        .collect()
}

/// Run every waiting workspace's steps if git runs now. Returns the
/// folders that changed (Done or AlreadyARepo).
pub fn run_all_deferred(force: bool) -> Vec<String> {
    if !force && !MAYBE_PENDING.load(Ordering::Relaxed) {
        return Vec::new();
    }
    let waiting = waiting_workspaces();
    if waiting.is_empty() {
        MAYBE_PENDING.store(false, Ordering::Relaxed);
        return Vec::new();
    }
    let mut changed = Vec::new();
    for w in waiting {
        if let Ok(DeferredRun::Done { .. } | DeferredRun::AlreadyARepo) = run_deferred(Path::new(&w.path), force) {
            changed.push(w.path);
        }
    }
    changed
}

/// Head branch by libgit2 (no git binary needed): for Add workspace while
/// git doesn't run.
pub fn head_branch_without_cli(path: &str) -> Option<String> {
    let repo = git2::Repository::discover(path).ok()?;
    let head = repo.head().ok()?;
    head.shorthand().map(str::to_string).filter(|s| !s.is_empty() && s != "HEAD")
}

// ── PTY spawns: the agents' note ────────────────────────────────────────

/// The `git` that sits first on a spawned agent's PATH while git doesn't
/// run. It hands over to the first real git it finds (the same rules as
/// the daemon's probe: with `mac`, Apple's shim only once `xcode-select
/// -p` names a developer folder with a git in it), else prints `note` and
/// fails. Shell builtins only, so it works with any PATH.
pub fn wait_shim_script(note: &str, mac: bool) -> String {
    let note = note.replace('\'', "'\\''");
    let mac = u8::from(mac);
    format!(
        r#"#!/bin/sh
# K2 git-wait shim (0.46.4). Staged by the K2 daemon while git isn't
# installed; hands over to the real git once there is one.
self_dir=$(cd "${{0%/*}}" && pwd -P)
mac={mac}
old_ifs=$IFS
IFS=:
for d in $PATH; do
  IFS=$old_ifs
  [ -n "$d" ] || continue
  [ -x "$d/git" ] || continue
  real=$(cd "$d" 2>/dev/null && pwd -P) || continue
  [ "$real" = "$self_dir" ] && continue
  if [ "$mac" = 1 ] && [ -x "$d/xcode-select" ]; then
    dev=$("$d/xcode-select" -p 2>/dev/null) || continue
    [ -x "$dev/usr/bin/git" ] || continue
  fi
  exec "$d/git" "$@"
done
echo '{note}' >&2
exit 1
"#
    )
}

/// `<home>/.k2/bin/git-wait`.
pub fn wait_shim_dir_at(home: &Path) -> PathBuf {
    home.join(".k2").join("bin").join("git-wait")
}

/// Write the shim (`body`, from [`wait_shim_script`]) under `home`;
/// rewritten only when its text changed.
pub fn stage_wait_shim_at(home: &Path, body: &str) -> Result<PathBuf, String> {
    let dir = wait_shim_dir_at(home);
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let path = dir.join("git");
    if std::fs::read_to_string(&path).ok().as_deref() != Some(body) {
        crate::fs_atomic::atomic_write_mode(&path, body.as_bytes(), 0o755)
            .map_err(|e| format!("write {}: {e}", path.display()))?;
    }
    Ok(dir)
}

/// Called by every PTY spawn. When git runs: do any waiting steps for
/// `cwd` and return `None`. When it doesn't: return the shim folder to put
/// first on the child's PATH.
pub fn before_spawn(cwd: Option<&Path>) -> Option<PathBuf> {
    if cfg!(windows) {
        return None;
    }
    if git_safe::availability().is_ready() {
        if let Some(cwd) = cwd {
            if marker_path(cwd).exists() {
                let _ = run_deferred(cwd, true);
            }
        }
        return None;
    }
    let home = dirs::home_dir()?;
    match stage_wait_shim_at(&home, &wait_shim_script(&agent_note(), cfg!(target_os = "macos"))) {
        Ok(dir) => Some(dir),
        Err(e) => {
            crate::log_debug!("[git-tools] git-wait shim not staged: {e}");
            None
        }
    }
}

// ── Installing ──────────────────────────────────────────────────────────

/// Where the install stands, for the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallState {
    Ready,
    NotInstalled,
    Installing,
}

/// After the click, count as installing for this long even before
/// Apple's installer shows up in the process list.
const INSTALL_GRACE: Duration = Duration::from_secs(90);
/// Never count as installing longer than this.
const INSTALL_MAX: Duration = Duration::from_secs(3 * 60 * 60);

/// Pure: the state from what the daemon knows.
pub fn install_state(ready: bool, mac: bool, started: Option<SystemTime>, now: SystemTime, installer_running: bool) -> InstallState {
    if ready {
        return InstallState::Ready;
    }
    if !mac {
        return InstallState::NotInstalled;
    }
    let Some(since) = started.and_then(|t| now.duration_since(t).ok()) else {
        return InstallState::NotInstalled;
    };
    if since < INSTALL_MAX && (installer_running || since < INSTALL_GRACE) {
        InstallState::Installing
    } else {
        InstallState::NotInstalled
    }
}

static INSTALL_STARTED: Mutex<Option<SystemTime>> = Mutex::new(None);

/// The process name of Apple's installer window.
const APPLE_INSTALLER: &str = "Install Command Line Developer Tools";

/// Is Apple's installer window up (downloading or asking)?
pub fn apple_installer_running() -> bool {
    if !cfg!(target_os = "macos") {
        return false;
    }
    std::process::Command::new("/usr/bin/pgrep")
        .args(["-f", APPLE_INSTALLER])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// What [`start_apple_install`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallStart {
    /// Apple's installer dialog was asked for.
    Started,
    /// git already runs; nothing to install.
    AlreadyReady,
}

/// The `xcode-select` K2 runs: the one beside the probed folders' shim,
/// else `/usr/bin/xcode-select`.
fn xcode_select_path() -> PathBuf {
    git_safe::probe_dirs()
        .into_iter()
        .map(|d| d.join("xcode-select"))
        .find(|p| git_safe::is_exec_file(p))
        .unwrap_or_else(|| PathBuf::from("/usr/bin/xcode-select"))
}

/// Ask macOS to install the Command Line Tools: `xcode-select --install`,
/// which opens Apple's own installer dialog in the owner's session. Never
/// sudo. Not macOS → Err.
pub fn start_apple_install() -> Result<InstallStart, String> {
    if !cfg!(target_os = "macos") {
        return Err("Apple's command line tools are macOS only".to_string());
    }
    if git_safe::refresh().is_ready() {
        return Ok(InstallStart::AlreadyReady);
    }
    let xs = xcode_select_path();
    let mut child = std::process::Command::new(&xs)
        .arg("--install")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("couldn't run {} --install: {e}", xs.display()))?;
    // `xcode-select --install` returns once the dialog is up; reap it.
    let _ = std::thread::Builder::new().name("xcode-select-install".into()).spawn(move || {
        let _ = child.wait();
    });
    *INSTALL_STARTED.lock().unwrap_or_else(|e| e.into_inner()) = Some(SystemTime::now());
    Ok(InstallStart::Started)
}

/// Steps for a Mac reached only over SSH (no screen for Apple's dialog).
/// Needs an admin password, so K2 shows them and never runs them.
pub fn apple_headless_steps() -> Vec<String> {
    vec![
        "touch /tmp/.com.apple.dt.CommandLineTools.installondemand.in-progress".to_string(),
        "softwareupdate --list   # note the \"Command Line Tools for Xcode-NN.N\" label".to_string(),
        "sudo softwareupdate -i \"Command Line Tools for Xcode-NN.N\" --verbose".to_string(),
        "rm /tmp/.com.apple.dt.CommandLineTools.installondemand.in-progress".to_string(),
    ]
}

/// The install line for this Linux distro, from `/etc/os-release` text.
/// Pure.
pub fn linux_install_command(os_release: &str) -> String {
    let mut ids = String::new();
    for line in os_release.lines() {
        if let Some(v) = line.strip_prefix("ID=").or_else(|| line.strip_prefix("ID_LIKE=")) {
            ids.push(' ');
            ids.push_str(&v.trim_matches('"').to_ascii_lowercase());
        }
    }
    let has = |names: &[&str]| ids.split_whitespace().any(|w| names.contains(&w));
    if has(&["debian", "ubuntu"]) {
        "sudo apt install git".to_string()
    } else if has(&["fedora", "rhel", "centos", "rocky", "almalinux"]) {
        "sudo dnf install git".to_string()
    } else if has(&["arch", "manjaro", "endeavouros"]) {
        "sudo pacman -S git".to_string()
    } else if has(&["opensuse", "suse", "opensuse-leap", "opensuse-tumbleweed", "sles"]) {
        "sudo zypper install git".to_string()
    } else if has(&["alpine"]) {
        "sudo apk add git".to_string()
    } else {
        "install git with your package manager (apt, dnf or pacman)".to_string()
    }
}

// ── Status for the UI and `GET /cli/git-tools` ──────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitToolsStatus {
    /// `macos` | `linux` | `windows` | other.
    pub os: &'static str,
    pub state: InstallState,
    /// `apple_clt` (macOS) or `git`; `None` when git runs.
    pub needs: Option<&'static str>,
    pub git_path: Option<String>,
    pub git_version: Option<String>,
    /// The one-click install is offered (macOS, git not running).
    pub can_install: bool,
    /// Unix ms of the last install click, if any.
    pub install_started_at: Option<i64>,
    /// Commands for the owner to run themselves (headless Mac, Linux).
    pub how_to: Vec<String>,
    /// Workspaces whose git steps wait.
    pub waiting: Vec<WaitingWorkspace>,
}

pub fn status(fresh: bool) -> GitToolsStatus {
    let probe = if fresh { git_safe::refresh() } else { git_safe::availability() };
    let mac = cfg!(target_os = "macos");
    let started = *INSTALL_STARTED.lock().unwrap_or_else(|e| e.into_inner());
    let ready = probe.is_ready();
    let installer = !ready && mac && started.is_some() && apple_installer_running();
    let state = install_state(ready, mac, started, SystemTime::now(), installer);
    let (git_path, git_version) = match &probe {
        GitProbe::Ready { program, version } => (Some(program.display().to_string()), Some(version.clone())),
        _ => (None, None),
    };
    let how_to = if ready {
        Vec::new()
    } else if mac {
        apple_headless_steps()
    } else if cfg!(target_os = "linux") {
        vec![linux_install_command(&std::fs::read_to_string("/etc/os-release").unwrap_or_default())]
    } else {
        vec!["Install Git for Windows from git-scm.com".to_string()]
    };
    GitToolsStatus {
        os: std::env::consts::OS,
        state,
        needs: match probe {
            GitProbe::Ready { .. } => None,
            GitProbe::NeedsAppleClt => Some("apple_clt"),
            GitProbe::Missing => Some("git"),
        },
        git_path,
        git_version,
        can_install: mac && !ready,
        install_started_at: started.and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64),
        how_to,
        waiting: waiting_workspaces(),
    }
}

#[cfg(test)]
#[path = "git_tools_tests.rs"]
mod tests;
