//! Watch authored charter sources and recompose `.k2/AGENTS.md`.
//!
//! CLI mutate already composes. Editing AGENT.md / PROJECT.md / context
//! layers in a text editor did not — so the generated AGENTS.md could stay
//! stale indefinitely (Scout 2026-08-13). This loop watches the same
//! source paths compose already knows and calls
//! [`k2_core::workspace::skill_regen::recompose_agents_md`].

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};

use k2_core::log_debug;
use k2_core::workspace::skill_regen::{
    compose_source_paths, is_compose_source_path, recompose_agents_md,
};

static RESYNC_REQUESTED: AtomicBool = AtomicBool::new(false);
static STARTED: OnceLock<()> = OnceLock::new();

const DEBOUNCE: Duration = Duration::from_millis(400);
const RECV_TIMEOUT: Duration = Duration::from_secs(2);

/// Start the charter-source watcher. Idempotent. Call after DB is ready.
pub fn start() {
    if STARTED.set(()).is_err() {
        return;
    }
    if let Err(e) = std::thread::Builder::new()
        .name("k2-charter-watch".into())
        .spawn(|| {
            if let Err(e) = run_loop() {
                log_debug!("[daemon/charter-watch] watcher exited: {e}");
            }
        })
    {
        log_debug!("[daemon/charter-watch] failed to spawn: {e}");
    }
}

/// Re-read project list + layer paths (after context add/remove / hire).
pub fn resync_watches() {
    RESYNC_REQUESTED.store(true, Ordering::Relaxed);
}

fn run_loop() -> Result<(), String> {
    let (tx, rx) = mpsc::channel::<Result<Event, notify::Error>>();
    let mut watcher = RecommendedWatcher::new(
        tx,
        Config::default().with_poll_interval(Duration::from_millis(250)),
    )
    .map_err(|e| format!("create watcher: {e}"))?;

    let mut dir_to_projects: HashMap<PathBuf, HashSet<String>> = HashMap::new();
    sync_watches(&mut watcher, &mut dir_to_projects);

    let mut pending: HashMap<String, Instant> = HashMap::new();

    loop {
        match rx.recv_timeout(RECV_TIMEOUT) {
            Ok(Ok(ev)) => {
                for p in ev.paths {
                    if let Some(proj) = project_for_event(&p, &dir_to_projects) {
                        if is_compose_source_path(&proj, &p) {
                            pending.insert(proj, Instant::now());
                        }
                    }
                }
            }
            Ok(Err(e)) => {
                log_debug!("[daemon/charter-watch] notify error: {e}");
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("watcher channel closed".into());
            }
        }

        let now = Instant::now();
        let ready: Vec<String> = pending
            .iter()
            .filter(|(_, t)| now.duration_since(**t) >= DEBOUNCE)
            .map(|(p, _)| p.clone())
            .collect();
        for proj in ready {
            pending.remove(&proj);
            log_debug!("[daemon/charter-watch] recompose {}", proj);
            recompose_agents_md(&proj);
        }

        if RESYNC_REQUESTED.swap(false, Ordering::Relaxed) {
            sync_watches(&mut watcher, &mut dir_to_projects);
        }
    }
}

fn project_for_event(
    path: &Path,
    dir_to_projects: &HashMap<PathBuf, HashSet<String>>,
) -> Option<String> {
    let mut cur = path.parent().unwrap_or(path);
    loop {
        if let Some(set) = dir_to_projects.get(cur) {
            if let Some(p) = set.iter().next() {
                return Some(p.clone());
            }
        }
        match cur.parent() {
            Some(p) if p != cur => cur = p,
            _ => return None,
        }
    }
}

fn sync_watches(
    watcher: &mut RecommendedWatcher,
    dir_to_projects: &mut HashMap<PathBuf, HashSet<String>>,
) {
    let projects = match k2_core::projects_ops::projects_list() {
        Ok(p) => p,
        Err(e) => {
            log_debug!("[daemon/charter-watch] projects_list: {e}");
            return;
        }
    };

    let mut wanted: HashMap<PathBuf, HashSet<String>> = HashMap::new();
    for proj in projects {
        let root = PathBuf::from(&proj.path);
        for src in compose_source_paths(&proj.path) {
            let dir = src.parent().unwrap_or(root.as_path()).to_path_buf();
            wanted.entry(dir).or_default().insert(proj.path.clone());
        }
        // Also watch the workspace dot-dir so new PROJECT.md / AGENT.md appear.
        let dot = k2_core::workspace_dot_dir(&proj.path);
        wanted.entry(dot).or_default().insert(proj.path.clone());
        wanted
            .entry(root)
            .or_default()
            .insert(proj.path.clone());
    }

    for dir in dir_to_projects.keys() {
        if !wanted.contains_key(dir) {
            let _ = watcher.unwatch(dir);
        }
    }
    for (dir, set) in &wanted {
        if !dir_to_projects.contains_key(dir) && dir.exists() {
            if let Err(e) = watcher.watch(dir, RecursiveMode::NonRecursive) {
                log_debug!(
                    "[daemon/charter-watch] watch {}: {e}",
                    dir.display()
                );
            } else {
                log_debug!(
                    "[daemon/charter-watch] watching {} ({} project(s))",
                    dir.display(),
                    set.len()
                );
            }
        }
    }
    *dir_to_projects = wanted;
}
