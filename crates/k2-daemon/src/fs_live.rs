//! Daemon-side recursive filesystem watcher for multi-writer Files-drawer
//! live refresh.
//!
//! Agents write via shell/tools (often NOT through `/cli/fs/*`). Other
//! clients mutate the host FS (local multi-window or K2 Connect). The
//! local Tauri `fs://change` watcher only covers the client Mac. This
//! module watches every registered project root on the **daemon machine**
//! and broadcasts [`SessionEvent::FsChanged`] so every thin client can
//! refresh its FileTree.
//!
//! Debounce (~200ms) matches `src-tauri/src/watcher.rs`. Paths are
//! batched and grouped by longest-prefix project root before emit.
//!
//! Changes tab v1: the same batches (with `.git` paths, which
//! `fs_changed` drops) feed [`crate::git_live`], which maps `.git/index`,
//! `HEAD`, `refs/**` and non-ignored edits to a debounced `git_changed`.
//! Worktree checkouts outside every project root, and git dirs outside
//! the watched roots, get their own watches.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};

use k2_core::log_debug;

use crate::notify_bound::{should_observe, DroppingHandler, NOTIFY_CHANNEL_BOUND};
use crate::session_events;

/// Request a re-sync of the watched project roots (e.g. after
/// `ProjectsChanged`). Cheap — the watcher loop polls this flag.
static RESYNC_REQUESTED: AtomicBool = AtomicBool::new(false);

/// True once [`start`] has been called. Prevents double-start in tests.
static STARTED: OnceLock<()> = OnceLock::new();

const DEBOUNCE: Duration = Duration::from_millis(200);
const RESYNC_POLL: Duration = Duration::from_secs(5 * 60);
/// How long to block waiting for the next notify event before checking
/// the resync flag / periodic resync.
const RECV_TIMEOUT: Duration = Duration::from_secs(2);

/// Start the background watcher thread. Idempotent. Call after the DB
/// is ready so `projects_list()` returns real roots.
pub fn start() {
    if STARTED.set(()).is_err() {
        return;
    }
    if let Err(e) = std::thread::Builder::new()
        .name("k2-fs-live".into())
        .spawn(|| {
            if let Err(e) = run_loop() {
                log_debug!("[daemon/fs_live] watcher exited: {e}");
            }
        })
    {
        log_debug!("[daemon/fs_live] failed to spawn watcher thread: {e}");
    }
}

/// Ask the watcher to re-read `projects_list()` and update its watch set.
/// Safe to call from any thread (e.g. after project create/delete).
pub fn resync_watches() {
    RESYNC_REQUESTED.store(true, Ordering::Relaxed);
}

/// What the watcher covers: recursive Files roots (`fs_changed` +
/// working-tree git edits) and the workspaces `git_changed` is mapped for.
#[derive(Debug, Clone, Default)]
pub(crate) struct WatchPlan {
    /// Recursively watched directories (project roots, plus worktree
    /// checkouts that live outside every project root).
    pub roots: Vec<String>,
    /// Workspace paths `git_changed` is emitted for (project roots and
    /// every worktree workspace, even one inside a project root).
    pub git_workspaces: Vec<String>,
}

/// The live plan: registered projects + worktree workspaces.
fn db_plan() -> WatchPlan {
    let projects = k2_core::projects_ops::projects_list_with_workspaces().unwrap_or_default();
    let mut project_roots: Vec<String> = Vec::new();
    let mut worktrees: Vec<String> = Vec::new();
    for p in projects {
        let root = p.project.path.trim_end_matches('/').to_string();
        if !root.is_empty() && Path::new(&root).is_dir() {
            project_roots.push(root);
        }
        for ws in p.workspaces {
            if let Some(wt) = ws.worktree_path {
                let wt = wt.trim_end_matches('/').to_string();
                if !wt.is_empty() && Path::new(&wt).is_dir() {
                    worktrees.push(wt);
                }
            }
        }
    }
    plan_from(project_roots, worktrees)
}

/// Build a plan: every project root is a recursive root; a worktree is a
/// recursive root only when no project root already covers it.
pub(crate) fn plan_from(project_roots: Vec<String>, worktrees: Vec<String>) -> WatchPlan {
    let under_root = |p: &str| {
        project_roots.iter().any(|r| session_events::cwd_path_boundary_matches(p, r))
    };
    let mut roots = project_roots.clone();
    let mut git_workspaces = project_roots.clone();
    for wt in worktrees {
        if !under_root(&wt) && !roots.contains(&wt) {
            roots.push(wt.clone());
        }
        if !git_workspaces.contains(&wt) {
            git_workspaces.push(wt);
        }
    }
    roots.sort();
    roots.dedup();
    WatchPlan { roots, git_workspaces }
}

fn run_loop() -> Result<(), String> {
    static NEVER: AtomicBool = AtomicBool::new(false);
    run_loop_with(&db_plan, &NEVER)
}

/// The watcher loop over `plan()` (re-read on resync). Returns when
/// `stop` is set (tests) or the notify channel dies.
pub(crate) fn run_loop_with(plan: &dyn Fn() -> WatchPlan, stop: &AtomicBool) -> Result<(), String> {
    let (tx, rx) = mpsc::sync_channel::<notify::Result<Event>>(NOTIFY_CHANNEL_BOUND);
    let mut watcher = RecommendedWatcher::new(
        DroppingHandler::new(tx),
        Config::default().with_poll_interval(Duration::from_millis(200)),
    )
    .map_err(|e| format!("create watcher: {e}"))?;

    let mut watched: HashMap<String, RecursiveMode> = HashMap::new();
    let mut git_set = sync_watches(&mut watcher, &mut watched, &plan());
    log_debug!(
        "[daemon/fs_live] watching {} path(s), {} git workspace(s)",
        watched.len(),
        git_set.repos.len()
    );

    let mut last_resync = Instant::now();

    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        // Periodic + explicit resync of the watch set.
        if RESYNC_REQUESTED.swap(false, Ordering::Relaxed) || last_resync.elapsed() >= RESYNC_POLL {
            git_set = sync_watches(&mut watcher, &mut watched, &plan());
            last_resync = Instant::now();
        }

        let first = match rx.recv_timeout(RECV_TIMEOUT) {
            Ok(Ok(event)) => event,
            Ok(Err(e)) => {
                log_debug!("[daemon/fs_live] notify error: {e}");
                continue;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("notify channel disconnected".into());
            }
        };

        // Coalesce unique paths within the debounce window.
        let mut coalesced: HashSet<String> = HashSet::new();
        collect_paths(&first, &mut coalesced);

        let deadline = Instant::now() + DEBOUNCE;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match rx.recv_timeout(remaining) {
                Ok(Ok(event)) => collect_paths(&event, &mut coalesced),
                Ok(Err(_)) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("notify channel disconnected".into());
                }
            }
        }

        if coalesced.is_empty() {
            continue;
        }
        // Changes tab: `.git` signals + non-ignored edits → git_changed
        // (git_live debounces). Sees the raw set, `.git` paths included.
        git_set.touch_affected(&coalesced, &is_noisy);
        let files: HashSet<String> = coalesced.into_iter().filter(|p| !is_noisy(p)).collect();
        if !files.is_empty() {
            emit_batched(files);
        }
    }
}

/// Every non-empty path of an observed event (noise is filtered later:
/// `.git` paths matter to `git_changed`, not to `fs_changed`).
fn collect_paths(event: &Event, out: &mut HashSet<String>) {
    if !should_observe(event.kind) {
        return;
    }
    for p in &event.paths {
        let s = p.to_string_lossy().to_string();
        if !s.is_empty() {
            out.insert(s);
        }
    }
}

/// Skip high-churn paths that would flood the bus.
///
/// Agent workspaces constantly rewrite under `.k2/` / `.k2so/`, and
/// `.git/` / build dirs thrash. Emitting those as `fs_changed` made
/// every connected Files tree force-reload (0.40.58 bounce loop on
/// hosts like NSI). User-visible source files still pass through.
pub(crate) fn is_noisy(path: &str) -> bool {
    // Normalize separators for a simple contains check.
    let p = path.replace('\\', "/");
    let lower = p.to_ascii_lowercase();

    // Workspace agent / K2 runtime state (heartbeats, mail, skills, DB).
    if p.contains("/.k2/")
        || p.ends_with("/.k2")
        || p.contains("/.k2so/")
        || p.ends_with("/.k2so")
        || p.contains("/.claude/")
        || p.contains("/.cursor/")
        || p.contains("/.codex/")
        || p.contains("/.opencode/")
    {
        return true;
    }

    // VCS + package/build trees.
    if p.contains("/.git/")
        || p.ends_with("/.git")
        || p.contains("/node_modules/")
        || p.ends_with("/node_modules")
        || p.contains("/target/debug/")
        || p.contains("/target/release/")
        || p.contains("/target/tmp/")
        || p.contains("/__pycache__/")
        || p.contains("/.venv/")
        || p.contains("/venv/")
        || p.contains("/.tox/")
        || p.contains("/dist/")
        || p.contains("/.next/")
        || p.contains("/.nuxt/")
        || p.contains("/.turbo/")
        || p.contains("/.cache/")
    {
        return true;
    }

    // Editor / OS junk + logs / journals.
    if p.ends_with(".swp")
        || p.ends_with(".swx")
        || p.ends_with("~")
        || p.ends_with("/.ds_store")
        || lower.ends_with("/.ds_store")
        || p.ends_with(".log")
        || p.ends_with(".log.1")
        || p.ends_with("-journal")
        || p.ends_with(".sqlite-wal")
        || p.ends_with(".sqlite-shm")
        || p.ends_with(".db-wal")
        || p.ends_with(".db-shm")
    {
        return true;
    }

    false
}

/// Map each path → workspace root (longest project prefix), then emit
/// one `fs_changed` per workspace with the batched paths.
fn emit_batched(paths: HashSet<String>) {
    let mut by_ws: HashMap<String, Vec<String>> = HashMap::new();
    for p in paths {
        let ws = session_events::resolve_workspace_for_path(&p);
        if ws.is_empty() {
            continue;
        }
        by_ws.entry(ws).or_default().push(p);
    }
    for (ws, ps) in by_ws {
        session_events::emit_fs_changed(&ws, ps);
    }
}

/// Align the live watcher with `plan`: recursive roots plus the git dirs
/// that live outside them. Returns the git watch set for the plan.
fn sync_watches(
    watcher: &mut RecommendedWatcher,
    watched: &mut HashMap<String, RecursiveMode>,
    plan: &WatchPlan,
) -> crate::git_live::GitWatchSet {
    let git_set = crate::git_live::GitWatchSet::build(&plan.git_workspaces);
    let mut desired: HashMap<String, RecursiveMode> = HashMap::new();
    for r in &plan.roots {
        if Path::new(r).is_dir() {
            desired.insert(r.clone(), RecursiveMode::Recursive);
        }
    }
    for (p, recursive) in git_set.extra_watches(&plan.roots) {
        let mode = if recursive { RecursiveMode::Recursive } else { RecursiveMode::NonRecursive };
        desired.entry(p).or_insert(mode);
    }

    // Unwatch removed paths (and paths whose mode changed).
    let stale: Vec<String> = watched
        .iter()
        .filter(|(p, m)| desired.get(*p) != Some(*m))
        .map(|(p, _)| p.clone())
        .collect();
    for p in stale {
        if let Err(e) = watcher.unwatch(Path::new(&p)) {
            log_debug!("[daemon/fs_live] unwatch {p}: {e}");
        }
        watched.remove(&p);
    }

    // Watch new paths.
    for (p, mode) in desired {
        if watched.contains_key(&p) {
            continue;
        }
        match watcher.watch(Path::new(&p), mode) {
            Ok(()) => {
                watched.insert(p, mode);
            }
            Err(e) => {
                log_debug!("[daemon/fs_live] watch {p}: {e}");
            }
        }
    }
    git_set
}

/// Group paths by workspace using the same resolver as the live loop.
/// Pure-ish (depends on projects_list for prefix match; falls back to parent).
#[cfg(test)]
pub(crate) fn group_paths_by_workspace(paths: Vec<String>) -> HashMap<String, Vec<String>> {
    let mut by_ws: HashMap<String, Vec<String>> = HashMap::new();
    for p in paths {
        if p.is_empty() || is_noisy(&p) {
            continue;
        }
        let ws = session_events::resolve_workspace_for_path(&p);
        if ws.is_empty() {
            continue;
        }
        by_ws.entry(ws).or_default().push(p);
    }
    by_ws
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn noisy_filter_skips_churn_and_keeps_source() {
        assert!(is_noisy("/proj/node_modules/foo/index.js"));
        assert!(is_noisy("/proj/.git/objects/ab/cd"));
        assert!(is_noisy("/proj/.git/config")); // whole .git is high-churn
        assert!(is_noisy("/proj/target/debug/deps/x.rlib"));
        assert!(is_noisy("/proj/.k2/AGENTS.md"));
        assert!(is_noisy("/proj/.k2so/state.db"));
        assert!(is_noisy("/proj/agent.log"));
        assert!(!is_noisy("/proj/src/main.rs"));
        assert!(!is_noisy("/proj/README.md"));
    }

    #[test]
    fn group_paths_parent_fallback() {
        let map = group_paths_by_workspace(vec![
            "/tmp/k2-fs-live-group-a/file.txt".into(),
            "/tmp/k2-fs-live-group-a/other.rs".into(),
            String::new(),
            "/tmp/k2-fs-live-group-b/x".into(),
        ]);
        // Both files under the same parent fallback group together.
        let a = map.get("/tmp/k2-fs-live-group-a").expect("group a parent");
        assert_eq!(a.len(), 2);
        assert!(map.contains_key("/tmp/k2-fs-live-group-b"));
    }

    /// End-to-end: create a temp dir, watch it via a short-lived notify
    /// watcher, write a file, and confirm we collect the path. Skips
    /// the full daemon bus — just proves the notify + debounce plumbing
    /// we rely on works in this environment.
    #[test]
    fn notify_picks_up_file_create() {
        let root: PathBuf = std::env::temp_dir().join(format!(
            "k2-fs-live-notify-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&root).expect("mkdir");
        let (tx, rx) = mpsc::channel::<Result<Event, notify::Error>>();
        let mut watcher = RecommendedWatcher::new(tx, Config::default()).expect("watcher");
        watcher
            .watch(&root, RecursiveMode::Recursive)
            .expect("watch");

        let file = root.join("created.txt");
        std::fs::write(&file, b"hi").expect("write");

        // Drain events for up to ~2s looking for our path.
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut found = false;
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(remaining.min(Duration::from_millis(200))) {
                Ok(Ok(ev)) => {
                    for p in &ev.paths {
                        if p.ends_with("created.txt") {
                            found = true;
                            break;
                        }
                    }
                    if found {
                        break;
                    }
                }
                Ok(Err(_)) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        let _ = watcher.unwatch(&root);
        let _ = std::fs::remove_dir_all(&root);
        assert!(found, "notify should report the created file");
    }
}
