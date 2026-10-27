//! The watcher behind the kit's disk sync (vs-live MI13). The kit never
//! watches: K2 tells a loaded doc its file changed (`file_changed()`), and
//! the kit's 250 ms `tick()` stat-checks every loaded file as a backup.
//!
//! Two sources, both debounced (150 ms of quiet; a watcher can fire
//! mid-write and the kit merges what it reads):
//! - **`FsChanged`** from `fs_live` (every registered workspace, recursive,
//!   `.k2/` excluded): a path that is a loaded doc's file → `file_changed()`;
//!   a folder that holds one → `check_disk()` (cheap stat). A lagged
//!   receiver polls every loaded file (`poll_disk`).
//! - **Its own non-recursive watch** on the folder of every loaded doc under
//!   `.k2/` (`.k2/wiki/**`, `.k2/prds/**`, …), which `fs_live` skips.
//!   Folders are watched while a doc in them is loaded (reference counted).
//!
//! The kit's own temp files (`.<name>.~mp<pid>-<n>.tmp`, `is_write_temp`)
//! are ignored.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::Duration;

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};

use k2_core::log_debug;
use k2_multiplayer_base::session::is_write_temp;

use crate::notify_bound::{should_observe, DroppingHandler, NOTIFY_CHANNEL_BOUND};

/// Quiet period before a burst of events is handled.
const DEBOUNCE: Duration = Duration::from_millis(150);

/// The `.k2/` folder watch.
pub struct Watch {
    inner: Mutex<Option<Inner>>,
}

struct Inner {
    watcher: RecommendedWatcher,
    /// Watched folder → loaded docs in it.
    dirs: HashMap<PathBuf, usize>,
}

impl Watch {
    /// Start the `.k2/` watcher and its event thread. A watcher that fails to
    /// start leaves `.k2/` docs to the kit's stat backup (logged).
    pub fn start() -> Watch {
        let (tx, rx) = mpsc::sync_channel::<notify::Result<Event>>(NOTIFY_CHANNEL_BOUND);
        let inner = match RecommendedWatcher::new(DroppingHandler::new(tx), notify::Config::default()) {
            Ok(w) => Some(Inner { watcher: w, dirs: HashMap::new() }),
            Err(e) => {
                log_debug!("[docs] WARN: .k2/ watcher failed to start ({e}); the stat backup covers .k2/ docs");
                None
            }
        };
        let _ = std::thread::Builder::new().name("k2-docs-watch".into()).spawn(move || loop {
            let first = match rx.recv() {
                Ok(ev) => ev,
                Err(_) => return,
            };
            let mut paths: HashSet<PathBuf> = HashSet::new();
            let mut take = |ev: notify::Result<Event>| {
                if let Ok(ev) = ev {
                    if should_observe(ev.kind) {
                        paths.extend(ev.paths);
                    }
                }
            };
            take(first);
            loop {
                match rx.recv_timeout(DEBOUNCE) {
                    Ok(ev) => take(ev),
                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
            changed(paths.iter().map(PathBuf::as_path));
        });
        Watch { inner: Mutex::new(inner) }
    }

    /// A doc under `.k2/` was loaded: watch its folder.
    pub fn watch_k2_file(&self, file: &Path) {
        let Some(dir) = file.parent() else { return };
        let mut g = self.inner.lock().expect("docs watch");
        let Some(inner) = g.as_mut() else { return };
        let n = inner.dirs.entry(dir.to_path_buf()).or_insert(0);
        *n += 1;
        if *n == 1 {
            if let Err(e) = inner.watcher.watch(dir, RecursiveMode::NonRecursive) {
                log_debug!("[docs] WARN: can't watch {} ({e}); the stat backup covers it", dir.display());
            }
        }
    }

    /// A doc under `.k2/` unloaded: drop its folder's watch when it was the
    /// last one there.
    pub fn unwatch_k2_file(&self, file: &Path) {
        let Some(dir) = file.parent() else { return };
        let mut g = self.inner.lock().expect("docs watch");
        let Some(inner) = g.as_mut() else { return };
        if let Some(n) = inner.dirs.get_mut(dir) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                inner.dirs.remove(dir);
                let _ = inner.watcher.unwatch(dir);
            }
        }
    }

    /// Release: drop the watcher (its event thread ends).
    pub fn stop(&self) {
        self.inner.lock().expect("docs watch").take();
    }
}

/// The path as the docs index holds it (canonical), also for a file that
/// is gone (its folder canonicalised).
fn canonical(p: &Path) -> Option<PathBuf> {
    if let Ok(c) = p.canonicalize() {
        return Some(c);
    }
    let dir = p.parent()?.canonicalize().ok()?;
    Some(dir.join(p.file_name()?))
}

/// Tell the loaded docs at these paths that their files changed.
pub(crate) fn changed<'a>(paths: impl Iterator<Item = &'a Path>) {
    let Ok(rt) = super::runtime() else { return };
    let mut seen: HashSet<(String, String)> = HashSet::new();
    for p in paths {
        if p.file_name().and_then(|n| n.to_str()).is_some_and(is_write_temp) {
            continue;
        }
        let Some(c) = canonical(p) else { continue };
        for (key, is_file) in rt.docs_at(&c) {
            if !seen.insert((key.space.clone(), key.path.clone())) {
                continue;
            }
            let Some(s) = rt.get(&key) else { continue };
            let r = if is_file { s.file_changed() } else { s.check_disk() };
            if let Err(e) = r {
                log_debug!("[docs] WARN: disk check of {} failed: {e}", super::host::key_label(&key));
            }
        }
    }
}

/// Feed `FsChanged` (outside `.k2/`) to the loaded docs. One thread for the
/// process; it ends once the runtime is released.
pub fn spawn_fs_changed_listener() {
    let mut rx = crate::session_events::subscribe();
    let _ = std::thread::Builder::new().name("k2-docs-fs".into()).spawn(move || loop {
        use tokio::sync::broadcast::error::RecvError;
        match rx.blocking_recv() {
            Ok(crate::session_events::SessionEvent::FsChanged { paths, .. }) => {
                if matches!(super::runtime(), Err(super::Unavailable::Released)) {
                    return;
                }
                changed(paths.iter().map(Path::new));
            }
            Ok(_) => {
                if matches!(super::runtime(), Err(super::Unavailable::Released)) {
                    return;
                }
            }
            Err(RecvError::Lagged(n)) => {
                log_debug!("[docs] fs events lagged by {n}; checking every loaded file");
                if let Ok(rt) = super::runtime() {
                    rt.poll_disk();
                }
            }
            Err(RecvError::Closed) => return,
        }
    });
}

impl super::Runtime {
    /// Stat-check every loaded file now (a lagged watcher).
    pub(crate) fn poll_disk(&self) {
        match self.manager.poll_disk() {
            Ok(rows) => {
                for (k, r) in rows {
                    if let Err(e) = r {
                        log_debug!("[docs] WARN: disk check of {} failed: {e}", super::host::key_label(&k));
                    }
                }
            }
            Err(e) => log_debug!("[docs] WARN: poll_disk: {e}"),
        }
    }
}
