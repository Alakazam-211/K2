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

use crate::notify_bound::{DroppingHandler, NOTIFY_CHANNEL_BOUND};

pub(crate) use crate::notify_bound::should_observe;

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
/// Updates the watch set only — does not immediately recompose.
pub fn resync_watches() {
    RESYNC_REQUESTED.store(true, Ordering::Relaxed);
}

/// Dirs the charter watcher would subscribe for the current project list.
/// Resync applies this set; it does not compose.
pub fn wanted_watch_dirs() -> HashMap<PathBuf, HashSet<String>> {
    let projects = match k2_core::projects_ops::projects_list() {
        Ok(p) => p,
        Err(e) => {
            log_debug!("[daemon/charter-watch] projects_list: {e}");
            return HashMap::new();
        }
    };

    let mut wanted: HashMap<PathBuf, HashSet<String>> = HashMap::new();
    for proj in projects {
        let root = PathBuf::from(&proj.path);
        for src in compose_source_paths(&proj.path) {
            let dir = src.parent().unwrap_or(root.as_path()).to_path_buf();
            wanted.entry(dir).or_default().insert(proj.path.clone());
        }
        // Also watch the workspace dot-dir so new PROJECT.md / ROLE.md appear.
        let dot = k2_core::workspace_dot_dir(&proj.path);
        wanted.entry(dot).or_default().insert(proj.path.clone());
        wanted.entry(root).or_default().insert(proj.path.clone());
    }
    wanted
}

/// True when `project_path` is in the wanted charter-watch set.
#[cfg(test)]
pub fn project_in_wanted_watch_set(project_path: &str) -> bool {
    wanted_watch_dirs()
        .values()
        .any(|set| set.iter().any(|p| p == project_path))
}

fn run_loop() -> Result<(), String> {
    let (tx, rx) = mpsc::sync_channel::<notify::Result<Event>>(NOTIFY_CHANNEL_BOUND);
    let mut watcher = RecommendedWatcher::new(
        DroppingHandler::new(tx),
        Config::default().with_poll_interval(Duration::from_millis(250)),
    )
    .map_err(|e| format!("create watcher: {e}"))?;

    let mut dir_to_projects: HashMap<PathBuf, HashSet<String>> = HashMap::new();
    sync_watches(&mut watcher, &mut dir_to_projects);

    let mut pending: HashMap<String, Instant> = HashMap::new();

    loop {
        match rx.recv_timeout(RECV_TIMEOUT) {
            Ok(Ok(ev)) => {
                consider_event(ev, &dir_to_projects, &mut pending);
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

/// Apply one notify event to the debounce map. Access/Other kinds
/// return before touching `ev.paths` so a canonicalize of ROLE.md
/// cannot be fed back as an OPEN storm.
fn consider_event(
    ev: Event,
    dir_to_projects: &HashMap<PathBuf, HashSet<String>>,
    pending: &mut HashMap<String, Instant>,
) {
    if !should_observe(ev.kind) {
        return;
    }
    for p in ev.paths {
        if let Some(proj) = project_for_event(&p, dir_to_projects) {
            if is_compose_source_path(&proj, &p) {
                pending.insert(proj, Instant::now());
            }
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
    let wanted = wanted_watch_dirs();

    for dir in dir_to_projects.keys() {
        if !wanted.contains_key(dir) {
            let _ = watcher.unwatch(dir);
        }
    }
    for (dir, set) in &wanted {
        if !dir_to_projects.contains_key(dir) && dir.exists() {
            if let Err(e) = watcher.watch(dir, RecursiveMode::NonRecursive) {
                log_debug!("[daemon/charter-watch] watch {}: {e}", dir.display());
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

#[cfg(test)]
mod tests {
    use super::*;
    use k2_core::workspace::agent_identity::persona_md_in;
    use k2_core::workspace::lifecycle::register_workspace_ex;
    use k2_core::workspace::skill_regen::compose_source_paths;

    fn unique_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "k2-charter-watch-{}-{}-{}",
            tag,
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    #[test]
    fn resync_watches_sets_flag_without_recomposing() {
        RESYNC_REQUESTED.store(false, Ordering::Relaxed);
        resync_watches();
        assert!(
            RESYNC_REQUESTED.load(Ordering::Relaxed),
            "resync must only flip the watch-set flag"
        );
    }

    #[test]
    fn register_workspace_is_in_wanted_watch_set() {
        k2_core::db::init_for_tests();
        let dir = unique_dir("hire");
        let path = dir.to_string_lossy().into_owned();
        register_workspace_ex(&path, false, true, false).expect("register");

        assert!(
            project_in_wanted_watch_set(&path),
            "charter watch set must include the new project after register"
        );

        let wanted = wanted_watch_dirs();
        for src in compose_source_paths(&path) {
            let watch_dir = src.parent().unwrap_or(dir.as_path());
            assert!(
                wanted.contains_key(watch_dir),
                "compose source {} should be watched via {}",
                src.display(),
                watch_dir.display()
            );
        }
        let persona = persona_md_in(k2_core::workspace::agent_identity::workspace_agent_path(
            &path,
        ));
        let persona_dir = persona.parent().unwrap_or(dir.as_path());
        assert!(
            wanted.contains_key(persona_dir) || wanted.contains_key(&dir),
            "persona dir must be in the watch set (helper, not hardcoded AGENT.md)"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn should_observe_drops_access_keeps_mutations() {
        use notify::event::{
            AccessKind, AccessMode, CreateKind, DataChange, MetadataKind, ModifyKind, RemoveKind,
            RenameMode,
        };
        use notify::EventKind;

        assert!(
            !should_observe(EventKind::Access(AccessKind::Open(AccessMode::Any))),
            "Access/Open must be ignored"
        );
        assert!(
            !should_observe(EventKind::Access(AccessKind::Close(AccessMode::Write))),
            "Access/Close(Write) must be ignored"
        );
        assert!(
            should_observe(EventKind::Modify(ModifyKind::Data(DataChange::Content))),
            "Modify(Data) must be observed"
        );
        assert!(
            should_observe(EventKind::Modify(ModifyKind::Name(RenameMode::Any))),
            "Modify(Name) must be observed"
        );
        assert!(
            should_observe(EventKind::Modify(ModifyKind::Metadata(MetadataKind::Any))),
            "Modify(Metadata) must be observed"
        );
        assert!(
            should_observe(EventKind::Create(CreateKind::File)),
            "Create must be observed"
        );
        assert!(
            should_observe(EventKind::Remove(RemoveKind::File)),
            "Remove must be observed"
        );
        assert!(
            should_observe(EventKind::Any),
            "Any must be observed (FSEvents imprecise saves)"
        );
        assert!(!should_observe(EventKind::Other), "Other must be ignored");
    }

    #[test]
    fn dropping_handler_drops_on_full() {
        use crate::notify_bound::DroppingHandler;
        use notify::event::ModifyKind;
        use notify::EventKind;

        let (tx, _rx) = mpsc::sync_channel(2);
        let mut handler = DroppingHandler::new(tx);
        let ev = Event::new(EventKind::Modify(ModifyKind::Any));
        for _ in 0..5 {
            notify::EventHandler::handle_event(&mut handler, Ok(ev.clone()));
        }
        let dropped = handler.dropped();
        assert!(
            dropped >= 3,
            "expected at least 3 drops after filling sync_channel(2) with 5 sends, got {dropped}"
        );
    }

    #[test]
    fn consider_event_ignores_access_without_touching_pending() {
        use notify::event::{AccessKind, AccessMode};
        use notify::EventKind;

        let dir = unique_dir("access");
        let proj = dir.to_string_lossy().into_owned();
        let mut map = HashMap::new();
        map.insert(dir.clone(), HashSet::from([proj]));
        let mut pending = HashMap::new();
        let ev = Event::new(EventKind::Access(AccessKind::Open(AccessMode::Any)))
            .add_path(dir.join("ROLE.md"));
        consider_event(ev, &map, &mut pending);
        assert!(
            pending.is_empty(),
            "Access must not enqueue recompose, pending={pending:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
