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

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};

use k2_core::log_debug;

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

fn run_loop() -> Result<(), String> {
    let (tx, rx) = mpsc::channel::<Result<Event, notify::Error>>();
    let mut watcher = RecommendedWatcher::new(
        tx,
        Config::default().with_poll_interval(Duration::from_millis(200)),
    )
    .map_err(|e| format!("create watcher: {e}"))?;

    let mut watched: HashSet<String> = HashSet::new();
    sync_watches(&mut watcher, &mut watched);
    log_debug!(
        "[daemon/fs_live] watching {} project root(s)",
        watched.len()
    );

    let mut last_resync = Instant::now();

    loop {
        // Periodic + explicit resync of the project-root set.
        if RESYNC_REQUESTED.swap(false, Ordering::Relaxed)
            || last_resync.elapsed() >= RESYNC_POLL
        {
            sync_watches(&mut watcher, &mut watched);
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
        emit_batched(coalesced);
    }
}

fn collect_paths(event: &Event, out: &mut HashSet<String>) {
    for p in &event.paths {
        let s = p.to_string_lossy().to_string();
        if s.is_empty() || is_noisy(&s) {
            continue;
        }
        out.insert(s);
    }
}

/// Skip high-churn paths that would flood the bus.
///
/// Agent workspaces constantly rewrite under `.k2/` / `.k2so/`, and
/// `.git/` / build dirs thrash. Emitting those as `fs_changed` made
/// every connected Files tree force-reload (0.40.58 bounce loop on
/// hosts like NSI). User-visible source files still pass through.
fn is_noisy(path: &str) -> bool {
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

/// Align the live watcher set with the current registered projects.
fn sync_watches(watcher: &mut RecommendedWatcher, watched: &mut HashSet<String>) {
    let desired: HashSet<String> = k2_core::projects_ops::projects_list()
        .unwrap_or_default()
        .into_iter()
        .map(|p| p.path.trim_end_matches('/').to_string())
        .filter(|p| !p.is_empty() && Path::new(p).is_dir())
        .collect();

    // Unwatch removed roots.
    let to_remove: Vec<String> = watched.difference(&desired).cloned().collect();
    for root in to_remove {
        if let Err(e) = watcher.unwatch(Path::new(&root)) {
            log_debug!("[daemon/fs_live] unwatch {root}: {e}");
        }
        watched.remove(&root);
    }

    // Watch new roots.
    let to_add: Vec<String> = desired.difference(watched).cloned().collect();
    for root in to_add {
        match watcher.watch(Path::new(&root), RecursiveMode::Recursive) {
            Ok(()) => {
                watched.insert(root);
            }
            Err(e) => {
                log_debug!("[daemon/fs_live] watch {root}: {e}");
            }
        }
    }
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
        let a = map
            .get("/tmp/k2-fs-live-group-a")
            .expect("group a parent");
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
        let mut watcher = RecommendedWatcher::new(tx, Config::default())
            .expect("watcher");
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
