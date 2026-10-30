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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
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

/// "Some workspace may have a marker", without a lost update (0.46.5,
/// first-run review item 9). It used to be one flag: a [`defer_init`] that
/// landed while the watch loop's scan was finding no markers was then
/// overwritten with "none", and the loop skipped that marker until a spawn
/// or a recheck found it. Now [`defer_init`] bumps `deferred`; a scan reads
/// it BEFORE looking and records that number in `clean` only when it found
/// nothing. A marker written during the scan bumps `deferred` past what the
/// scan read, so the next tick looks again.
pub(crate) struct PendingFlag {
    deferred: AtomicU64,
    clean: AtomicU64,
}

impl PendingFlag {
    /// Starts "may be pending" (markers from before this daemon run).
    pub(crate) const fn new() -> Self {
        Self { deferred: AtomicU64::new(1), clean: AtomicU64::new(0) }
    }

    /// A marker was written.
    pub(crate) fn mark(&self) {
        self.deferred.fetch_add(1, Ordering::SeqCst);
    }

    /// Before a scan: `None` when the last scan found nothing and no marker
    /// was written since; else the number to hand [`Self::found_none`].
    pub(crate) fn begin_scan(&self) -> Option<u64> {
        let seen = self.deferred.load(Ordering::SeqCst);
        (self.clean.load(Ordering::SeqCst) != seen).then_some(seen)
    }

    /// The scan that began at `seen` found no marker.
    pub(crate) fn found_none(&self, seen: u64) {
        self.clean.fetch_max(seen, Ordering::SeqCst);
    }
}

static MAYBE_PENDING: PendingFlag = PendingFlag::new();

pub fn marker_path(root: &Path) -> PathBuf {
    root.join(".k2").join("git-deferred.json")
}

pub fn read_deferred(root: &Path) -> Option<Deferred> {
    let text = std::fs::read_to_string(marker_path(root)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Leave the marker: `git init` on `branch` (and the identity seed) wait
/// for git. A name git would refuse is refused now (it would otherwise
/// wait forever).
///
/// `root` must be an existing folder: a typo'd path is refused, never
/// created (0.46.5, first-run review item 14; with git, `git init` in a
/// missing folder fails the same way).
pub fn defer_init(root: &Path, branch: &str) -> Result<(), String> {
    let branch = valid_branch(branch)?;
    require_folder(root)?;
    let path = marker_path(root);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }
    let d = Deferred {
        branch,
        deferred_at: SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0),
    };
    let text = serde_json::to_string_pretty(&d).map_err(|e| e.to_string())?;
    crate::fs_atomic::atomic_write_str(&path, &text).map_err(|e| format!("write {}: {e}", path.display()))?;
    MAYBE_PENDING.mark();
    Ok(())
}

/// `root` is an existing folder, or the error Add workspace shows.
pub fn require_folder(root: &Path) -> Result<(), String> {
    if root.is_dir() {
        return Ok(());
    }
    if root.exists() {
        return Err(format!("{} isn't a folder", root.display()));
    }
    Err(format!("There's no folder at {} (check the path; K2 doesn't create it)", root.display()))
}

pub fn clear_deferred(root: &Path) {
    let _ = std::fs::remove_file(marker_path(root));
}

/// After a failed run, wait this long before the watch loop tries that
/// folder again (a spawn or a recheck still tries at once).
const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(300);

/// The last failure per folder: when, and what git said. Kept until the
/// steps succeed (the backoff only looks at the newest RETRY window).
static LAST_FAILURE: Mutex<Vec<(PathBuf, Instant, String)>> = Mutex::new(Vec::new());

/// One deferred run at a time (the loop, a spawn and a recheck could
/// otherwise both `git init` and commit the same folder).
static RUN_LOCK: Mutex<()> = Mutex::new(());

fn last_failure(root: &Path) -> Option<(Instant, String)> {
    LAST_FAILURE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|(p, _, _)| p == root)
        .map(|(_, at, e)| (*at, e.clone()))
}

fn set_failure(root: &Path, err: Option<String>) {
    let mut g = LAST_FAILURE.lock().unwrap_or_else(|e| e.into_inner());
    g.retain(|(p, _, _)| p != root);
    if let Some(e) = err {
        g.push((root.to_path_buf(), Instant::now(), e));
    }
}

/// Check a branch name for `git init --initial-branch` before deferring
/// it: trimmed, not empty, no leading `-`, and a valid `refs/heads/<b>`.
/// Returns the trimmed name.
pub fn valid_branch(branch: &str) -> Result<String, String> {
    let b = branch.trim();
    if b.is_empty() {
        return Err("The branch name is empty".to_string());
    }
    if b.starts_with('-') {
        return Err(format!("\"{b}\" isn't a valid branch name (it can't start with -)"));
    }
    if !git2::Reference::is_valid_name(&format!("refs/heads/{b}")) {
        return Err(format!("\"{b}\" isn't a valid branch name"));
    }
    Ok(b.to_string())
}

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
    if read_deferred(root).is_none() {
        return Ok(DeferredRun::NothingWaiting);
    }
    if !git_safe::availability().is_ready() {
        return Ok(DeferredRun::GitNotReady);
    }
    let _one_at_a_time = RUN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // Re-read inside the lock: another caller may have just done it.
    let Some(d) = read_deferred(root) else {
        return Ok(DeferredRun::NothingWaiting);
    };
    if !force && last_failure(root).is_some_and(|(at, _)| at.elapsed() < RETRY_AFTER_FAILURE) {
        return Ok(DeferredRun::Backoff);
    }
    if root.join(".git").exists() {
        clear_deferred(root);
        set_failure(root, None);
        return Ok(DeferredRun::AlreadyARepo);
    }
    let root_str = root.to_string_lossy().to_string();
    let result = valid_branch(&d.branch).and_then(|b| crate::projects_ops::init_and_first_commit(&root_str, &b, true).map(|()| b));
    match result {
        Ok(branch) => {
            clear_deferred(root);
            set_failure(root, None);
            crate::log_debug!("[git-tools] {root_str}: deferred git init on {branch} done");
            Ok(DeferredRun::Done { branch })
        }
        Err(e) => {
            set_failure(root, Some(e.clone()));
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
    /// The last time the waiting steps were tried and failed: what git
    /// said. `None` until a try fails.
    pub last_error: Option<String>,
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
            let root = Path::new(&p.path);
            let d = read_deferred(root)?;
            let last_error = last_failure(root).map(|(_, e)| e);
            Some(WaitingWorkspace {
                project_id: p.id,
                name: p.name,
                branch: d.branch,
                note: waiting_mark().to_string(),
                last_error,
                path: p.path,
            })
        })
        .collect()
}

/// Run every waiting workspace's steps if git runs now. Returns the
/// folders that changed (Done or AlreadyARepo).
pub fn run_all_deferred(force: bool) -> Vec<String> {
    let seen = MAYBE_PENDING.begin_scan();
    if !force && seen.is_none() {
        return Vec::new();
    }
    let waiting = waiting_workspaces();
    if waiting.is_empty() {
        if let Some(seen) = seen {
            MAYBE_PENDING.found_none(seen);
        }
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
/// the daemon's probe: Apple's exact shim (`apple`, macOS) only once its
/// `xcode-select -p` names a developer folder with a git in it), else
/// prints `note` and fails. Shell builtins only, so it works with any PATH.
/// `apple` paths are compared with each PATH folder's real path
/// (`pwd -P`), so pass canonical ones.
pub fn wait_shim_script(note: &str, apple: Option<&git_safe::ApplePair>) -> String {
    let q = |s: &str| s.replace('\'', "'\\''");
    let note = q(note);
    let (apple_git, apple_xs) = match apple {
        Some(p) => (q(&p.git.to_string_lossy()), q(&p.xcode_select.to_string_lossy())),
        None => (String::new(), String::new()),
    };
    format!(
        r#"#!/bin/sh
# K2 git-wait shim (0.46.4). Staged by the K2 daemon while git isn't
# installed; hands over to the real git once there is one.
self_dir=$(cd "${{0%/*}}" && pwd -P)
apple_git='{apple_git}'
apple_xs='{apple_xs}'
old_ifs=$IFS
IFS=:
for d in $PATH; do
  IFS=$old_ifs
  [ -n "$d" ] || continue
  [ -x "$d/git" ] || continue
  real=$(cd "$d" 2>/dev/null && pwd -P) || continue
  [ "$real" = "$self_dir" ] && continue
  case "$real" in */.k2/bin/git-wait) continue ;; esac
  if [ -n "$apple_git" ] && [ "$real/git" = "$apple_git" ]; then
    dev=$("$apple_xs" -p 2>/dev/null) || continue
    [ -x "$dev/usr/bin/git" ] || continue
  fi
  exec "$real/git" "$@"
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

static DONE_LISTENER: OnceLock<fn(&Path)> = OnceLock::new();

/// Register what to call when a spawn finished a folder's waiting git
/// steps (the daemon: `ProjectsChanged` + watch resync). First call wins.
pub fn set_done_listener(f: fn(&Path)) {
    let _ = DONE_LISTENER.set(f);
}

/// Called by every PTY spawn. When git runs: do any waiting steps for
/// `cwd` and return `None`, telling the listener when they finished, as
/// the watch loop and a recheck do (0.46.5, first-run review item 12:
/// clients used to learn only on their next poll). When it doesn't: return
/// the shim folder to put first on the child's PATH.
pub fn before_spawn(cwd: Option<&Path>) -> Option<PathBuf> {
    if git_safe::availability().is_ready() {
        if let Some(cwd) = cwd {
            if marker_path(cwd).exists() {
                if let Ok(DeferredRun::Done { .. } | DeferredRun::AlreadyARepo) = run_deferred(cwd, true) {
                    if let Some(f) = DONE_LISTENER.get() {
                        f(cwd);
                    }
                }
            }
        }
        return None;
    }
    // Windows: no shim. It is a sh script named `git`, which a Windows
    // program can't run (it looks for git.exe), and Windows K2 needs Git
    // for Windows anyway. The waiting steps above still run there.
    if cfg!(windows) {
        return None;
    }
    let home = crate::paths::home_dir()?;
    match stage_wait_shim_at(&home, &wait_shim_script(&agent_note(), git_safe::apple_pair().as_ref())) {
        Ok(dir) => Some(dir),
        Err(e) => {
            crate::log_debug!("[git-tools] git-wait shim not staged: {e}");
            None
        }
    }
}

// ── Installing ──────────────────────────────────────────────────────────

/// Where git stands, for the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallState {
    Ready,
    NotInstalled,
    Installing,
    /// A git is there but fails (`gitError` says how; `howTo` the fix).
    GitBroken,
}

/// After the click, count as installing for this long even before
/// Apple's installer shows up in the process list.
const INSTALL_GRACE: Duration = Duration::from_secs(90);
/// Never count as installing longer than this.
const INSTALL_MAX: Duration = Duration::from_secs(3 * 60 * 60);

/// Pure: the state from what the daemon knows.
pub fn install_state(
    probe: &GitProbe,
    mac: bool,
    started: Option<SystemTime>,
    now: SystemTime,
    installer_running: bool,
) -> InstallState {
    match probe {
        GitProbe::Ready { .. } => return InstallState::Ready,
        GitProbe::Broken { .. } => return InstallState::GitBroken,
        GitProbe::NeedsAppleClt | GitProbe::Missing => {}
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

/// When the owner last started Apple's installer (this daemon run).
pub fn install_started_at() -> Option<SystemTime> {
    *INSTALL_STARTED.lock().unwrap_or_else(|e| e.into_inner())
}

/// The process name of Apple's installer window.
const APPLE_INSTALLER: &str = "Install Command Line Developer Tools";

/// Is Apple's installer window up (downloading or asking)?
pub fn apple_installer_running() -> bool {
    if !cfg!(target_os = "macos") {
        return false;
    }
    crate::bg_proc::command("/usr/bin/pgrep")
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

/// How long to watch `xcode-select --install` for a quick refusal.
const INSTALL_WATCH: Duration = Duration::from_secs(2);

/// Ask macOS to install the Command Line Tools: `/usr/bin/xcode-select
/// --install`, which opens Apple's own installer dialog in the owner's
/// session. Never sudo. Watches the child for about 2 s: a non-zero exit
/// (for example "already installed") is an `Err` with what it said, and
/// nothing counts as installing. Not macOS → Err.
pub fn start_apple_install() -> Result<InstallStart, String> {
    use std::io::Read;
    if !cfg!(target_os = "macos") {
        return Err("Apple's command line tools are macOS only".to_string());
    }
    match git_safe::refresh() {
        GitProbe::Ready { .. } => return Ok(InstallStart::AlreadyReady),
        GitProbe::Broken { program, error } => {
            return Err(format!(
                "git is installed but fails ({}: {error}). {}",
                program.display(),
                broken_git_fix(&program, &error, true, "")
            ))
        }
        GitProbe::NeedsAppleClt | GitProbe::Missing => {}
    }
    let pair = git_safe::apple_pair().ok_or("Apple's command line tools are macOS only")?;
    let xs = pair.xcode_select;
    let mut child = crate::bg_proc::command(&xs)
        .arg("--install")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("couldn't run {} --install: {e}", xs.display()))?;
    let deadline = Instant::now() + INSTALL_WATCH;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if status.success() {
                    break;
                }
                let mut said = String::new();
                if let Some(mut e) = child.stderr.take() {
                    let _ = e.read_to_string(&mut said);
                }
                if let Some(mut o) = child.stdout.take() {
                    let mut out = String::new();
                    let _ = o.read_to_string(&mut out);
                    if said.trim().is_empty() {
                        said = out;
                    }
                }
                let said = said.trim();
                let code = status.code().map(|c| format!("exit {c}")).unwrap_or_else(|| "killed".into());
                return Err(if said.to_ascii_lowercase().contains("already installed") {
                    format!(
                        "Apple's command line tools are already installed ({} says: {said}). If git still fails, `k2 git-tools` shows what it says.",
                        xs.display()
                    )
                } else {
                    format!("{} --install failed ({code}): {said}", xs.display())
                });
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            Ok(None) => {
                // Still running (the dialog is up): reap it later.
                let _ = std::thread::Builder::new().name("xcode-select-install".into()).spawn(move || {
                    let _ = child.wait_with_output();
                });
                break;
            }
            Err(e) => return Err(format!("{} --install: {e}", xs.display())),
        }
    }
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

/// How to fix a git that is there but fails. Pure.
pub fn broken_git_fix(program: &Path, error: &str, mac: bool, os_release: &str) -> String {
    if error.contains("xcodebuild -license") || (mac && error.to_ascii_lowercase().contains("license")) {
        "Accept Apple's licence: run `sudo xcodebuild -license` in Terminal (or open Xcode once)".to_string()
    } else if mac {
        format!("Run `{} --version` in Terminal to see what it needs", program.display())
    } else {
        format!("Reinstall git: {}", linux_install_command(os_release))
    }
}

// ── Status for the UI and `GET /cli/git-tools` ──────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitToolsStatus {
    /// `macos` | `linux` | `windows` | other.
    pub os: &'static str,
    pub state: InstallState,
    /// `apple_clt` (macOS), `git`, or `fix_git` (a git that fails);
    /// `None` when git runs.
    pub needs: Option<&'static str>,
    pub git_path: Option<String>,
    pub git_version: Option<String>,
    /// What a git that is there but fails said (state `git_broken`).
    pub git_error: Option<String>,
    /// The one-click install is offered (macOS, the tools missing).
    pub can_install: bool,
    /// Unix ms of the last install click, if any.
    pub install_started_at: Option<i64>,
    /// Commands for the owner to run themselves (headless Mac, Linux, a
    /// broken git's fix).
    pub how_to: Vec<String>,
    /// Workspaces whose git steps wait.
    pub waiting: Vec<WaitingWorkspace>,
}

/// Pure: the status from a probe and what the daemon knows.
pub fn status_from(
    probe: &GitProbe,
    mac: bool,
    started: Option<SystemTime>,
    installer_running: bool,
    os_release: &str,
    waiting: Vec<WaitingWorkspace>,
) -> GitToolsStatus {
    let state = install_state(probe, mac, started, SystemTime::now(), installer_running);
    let (git_path, git_version, git_error) = match probe {
        GitProbe::Ready { program, version } => (Some(program.display().to_string()), Some(version.clone()), None),
        GitProbe::Broken { program, error } => (Some(program.display().to_string()), None, Some(error.clone())),
        _ => (None, None, None),
    };
    let how_to = match probe {
        GitProbe::Ready { .. } => Vec::new(),
        GitProbe::Broken { program, error } => vec![broken_git_fix(program, error, mac, os_release)],
        _ if mac => apple_headless_steps(),
        _ if cfg!(windows) => vec!["Install Git for Windows from git-scm.com".to_string()],
        _ => vec![linux_install_command(os_release)],
    };
    GitToolsStatus {
        os: std::env::consts::OS,
        state,
        needs: match probe {
            GitProbe::Ready { .. } => None,
            GitProbe::NeedsAppleClt => Some("apple_clt"),
            GitProbe::Missing => Some("git"),
            GitProbe::Broken { .. } => Some("fix_git"),
        },
        git_path,
        git_version,
        git_error,
        can_install: mac && matches!(probe, GitProbe::NeedsAppleClt),
        install_started_at: started.and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64),
        how_to,
        waiting,
    }
}

pub fn status(fresh: bool) -> GitToolsStatus {
    let probe = if fresh { git_safe::refresh() } else { git_safe::availability() };
    let mac = cfg!(target_os = "macos");
    let started = install_started_at();
    let installer = !probe.is_ready() && mac && started.is_some() && apple_installer_running();
    let os_release = if cfg!(target_os = "linux") { std::fs::read_to_string("/etc/os-release").unwrap_or_default() } else { String::new() };
    status_from(&probe, mac, started, installer, &os_release, waiting_workspaces())
}

/// What the CLI prints (`k2 git-tools`, `?format=text`): plain lines, so
/// the CLI needs no python3 (on a fresh Mac `/usr/bin/python3` is another
/// of Apple's shims). Pure.
pub fn render_text(s: &GitToolsStatus, started: Option<InstallStart>, finished: &[String]) -> String {
    let mut out: Vec<String> = Vec::new();
    match started {
        Some(InstallStart::Started) => {
            out.push("Apple's installer is open on this Mac's screen (it can be behind the K2 window).".into());
            out.push("Click Install, not Get Xcode (that opens the App Store for the full Xcode),".into());
            out.push("agree to Apple's licence, then wait for the download (about 5-15 min).".into());
        }
        Some(InstallStart::AlreadyReady) => out.push("git already works here; nothing to install.".into()),
        None => {}
    }
    out.push(match s.state {
        InstallState::Ready => format!(
            "git        : ready ({}, {})",
            s.git_version.as_deref().unwrap_or("?"),
            s.git_path.as_deref().unwrap_or("?")
        ),
        InstallState::Installing => "git        : installing Apple's command line tools".into(),
        InstallState::GitBroken => format!(
            "git        : found but failing ({}): {}",
            s.git_path.as_deref().unwrap_or("?"),
            s.git_error.as_deref().unwrap_or("?")
        ),
        InstallState::NotInstalled if s.needs == Some("apple_clt") => {
            "git        : not installed (needs Apple's command line tools)".into()
        }
        InstallState::NotInstalled => "git        : not installed (needs git)".into(),
    });
    for p in finished {
        out.push(format!("finished   : {p}"));
    }
    for w in &s.waiting {
        out.push(format!("waiting    : {} ({}) {}", w.name, w.path, w.note));
        if let Some(e) = &w.last_error {
            out.push(format!("  last try : {e}"));
        }
    }
    if matches!(s.state, InstallState::NotInstalled | InstallState::GitBroken) && !s.how_to.is_empty() {
        if s.state == InstallState::GitBroken {
            out.push("fix        :".into());
        } else if s.can_install {
            out.push("install    : k2 git-tools install   (opens Apple's installer on this Mac)".into());
            out.push("over SSH   : run these yourself (admin password needed):".into());
        } else {
            out.push("install    : run this yourself:".into());
        }
        for line in &s.how_to {
            out.push(format!("  {line}"));
        }
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

#[cfg(test)]
#[path = "git_tools_tests.rs"]
mod tests;
