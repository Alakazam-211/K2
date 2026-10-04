//! Watch `~/.k2/zen/` and announce changes (prd-zen-mode-v1 Z12, vs-live Z61).
//!
//! Shape follows `charter_compose_watch` and the 0.40.104 RSS lesson:
//! - two DIRECTORIES, non-recursive (`zen/` and `zen/pages/`), so editor
//!   rename-saves are seen and `.history/` is never walked;
//! - the bounded `notify_bound` channel (`try_send`, drop on full), so a
//!   burst can't grow memory;
//! - `should_observe` drops Access/Other kinds before paths are touched;
//! - a path filter that keeps only `zen.toml` and `pages/<id>.toml`
//!   (never `.history/`, `homes.json`, `grants.json` or editor temp files);
//! - a 250 ms trailing debounce: a burst of saves is one re-validate.
//!
//! Started lazily: at boot only when the folder exists, otherwise by the
//! first `POST /cli/zen/page/ensure`. One watcher per process.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};

use k2_core::log_debug;
use k2_core::zen::store::{valid_home_id, PAGES_DIR, ZEN_FILE};

use crate::notify_bound::{should_observe, DroppingHandler, NOTIFY_CHANNEL_BOUND};

pub const DEBOUNCE: Duration = Duration::from_millis(250);
const TICK: Duration = Duration::from_millis(50);

static STARTED: AtomicBool = AtomicBool::new(false);
static RUNNING: AtomicBool = AtomicBool::new(false);

/// Whether the watcher thread is alive (for `k2 zen doctor`).
pub fn is_running() -> bool {
    RUNNING.load(Ordering::Relaxed)
}

/// Boot hook: baseline + watch only when this computer has set Zen up.
pub fn start_at_boot() {
    if k2_core::zen::is_set_up() {
        crate::zen_routes::prime();
        start();
    }
}

/// Start the watcher once. No-op when the folder doesn't exist yet.
pub fn start() {
    let root = k2_core::zen::zen_root();
    if !root.is_dir() || STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    crate::zen_routes::prime();
    let spawned = std::thread::Builder::new()
        .name("k2-zen-watch".into())
        .spawn(move || {
            RUNNING.store(true, Ordering::Relaxed);
            let stop = Arc::new(AtomicBool::new(false));
            let r = watch_loop(&root, &stop, || {
                if let Err(e) = crate::zen_routes::refresh_and_emit() {
                    log_debug!("[daemon/zen-watch] refresh: {e}");
                }
            });
            RUNNING.store(false, Ordering::Relaxed);
            STARTED.store(false, Ordering::SeqCst);
            if let Err(e) = r {
                log_debug!("[daemon/zen-watch] watcher exited: {e}");
            }
        });
    if let Err(e) = spawned {
        STARTED.store(false, Ordering::SeqCst);
        log_debug!("[daemon/zen-watch] failed to spawn: {e}");
    }
}

/// The root as given and as the OS reports it (macOS reports `/private/var`
/// for `/var` temp folders).
fn root_forms(root: &Path) -> Vec<PathBuf> {
    let mut out = vec![root.to_path_buf()];
    if let Ok(c) = std::fs::canonicalize(root) {
        if c != root {
            out.push(c);
        }
    }
    out
}

/// True for `<root>/zen.toml`, `<root>/pages/<id>.toml` and `<root>/pages`
/// itself. Everything else (`.history/…`, `homes.json`, `grants.json`,
/// `zen.toml.swp`, `.#zen.toml`, atomic-write temp files) is ignored.
pub(crate) fn is_zen_source(roots: &[PathBuf], path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let Some(parent) = path.parent() else { return false };
    if roots.iter().any(|r| r == parent) {
        return name == ZEN_FILE || name == PAGES_DIR;
    }
    let in_pages = parent.file_name().and_then(|n| n.to_str()) == Some(PAGES_DIR)
        && parent.parent().is_some_and(|gp| roots.iter().any(|r| r == gp));
    in_pages
        && name
            .strip_suffix(".toml")
            .is_some_and(valid_home_id)
}

/// The watch loop. Calls `on_change` once per debounced burst of real
/// source changes. Returns when `stop` is set or the channel closes.
pub(crate) fn watch_loop(
    root: &Path,
    stop: &AtomicBool,
    mut on_change: impl FnMut(),
) -> Result<(), String> {
    let (tx, rx) = mpsc::sync_channel::<notify::Result<Event>>(NOTIFY_CHANNEL_BOUND);
    let mut watcher = RecommendedWatcher::new(DroppingHandler::new(tx), Config::default())
        .map_err(|e| format!("create watcher: {e}"))?;
    watcher
        .watch(root, RecursiveMode::NonRecursive)
        .map_err(|e| format!("watch {}: {e}", root.display()))?;
    let pages = root.join(PAGES_DIR);
    let mut pages_watched = false;
    let roots = root_forms(root);
    let mut pending: Option<Instant> = None;
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        if !pages_watched && pages.is_dir() {
            pages_watched = watcher.watch(&pages, RecursiveMode::NonRecursive).is_ok();
        } else if pages_watched && !pages.is_dir() {
            pages_watched = false;
        }
        match rx.recv_timeout(TICK) {
            Ok(Ok(ev)) => {
                if should_observe(ev.kind) && ev.paths.iter().any(|p| is_zen_source(&roots, p)) {
                    pending = Some(Instant::now());
                }
            }
            Ok(Err(e)) => log_debug!("[daemon/zen-watch] notify error: {e}"),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err("watcher channel closed".into()),
        }
        if pending.is_some_and(|t| t.elapsed() >= DEBOUNCE) {
            pending = None;
            on_change();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "k2-zen-watch-{tag}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(dir.join(PAGES_DIR)).expect("mkdir zen/pages");
        std::fs::write(dir.join(ZEN_FILE), "schema = 1\n").expect("write zen.toml");
        dir
    }

    #[test]
    fn source_filter_keeps_only_zen_files() {
        let root = PathBuf::from("/tmp/k2zen");
        let roots = vec![root.clone()];
        assert!(is_zen_source(&roots, &root.join("zen.toml")));
        assert!(is_zen_source(&roots, &root.join("pages/abc-123.toml")));
        assert!(is_zen_source(&roots, &root.join("pages")));
        for no in [
            "homes.json",
            "grants.json",
            ".history",
            ".history/zen.toml/20261004T120000000Z-000.toml",
            "zen.toml.swp",
            ".zen.toml.swp",
            ".#zen.toml",
            "zen.toml~",
            "pages/abc.toml.tmp",
            "pages/.hidden.toml",
            "pages/sub/x.toml",
        ] {
            assert!(!is_zen_source(&roots, &root.join(no)), "{no} must be ignored");
        }
        assert!(!is_zen_source(&roots, Path::new("/elsewhere/zen.toml")));
    }

    /// T1.7: a 500-write burst is one re-validate; `.history/`, `homes.json`
    /// and `grants.json` writes are none; the channel is the shared bound.
    #[test]
    fn burst_is_one_change_and_daemon_files_are_none() {
        assert_eq!(NOTIFY_CHANNEL_BOUND, 256, "zen watch must use the shared bounded channel");
        let root = temp_root("burst");
        std::thread::sleep(Duration::from_millis(300));
        let count = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let handle = {
            let (root, count, stop) = (root.clone(), count.clone(), stop.clone());
            std::thread::spawn(move || {
                watch_loop(&root, &stop, || {
                    count.fetch_add(1, Ordering::SeqCst);
                })
                .expect("watch loop")
            })
        };
        std::thread::sleep(Duration::from_millis(500));

        for i in 0..500 {
            std::fs::write(root.join(ZEN_FILE), format!("schema = 1\n# save {i}\n"))
                .expect("burst write");
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while count.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(1200));
        assert_eq!(count.load(Ordering::SeqCst), 1, "500 writes in a burst must re-validate exactly once");

        let hist = root.join(".history").join(ZEN_FILE);
        std::fs::create_dir_all(&hist).expect("mkdir history");
        for i in 0..50 {
            std::fs::write(hist.join(format!("20261004T1200{i:02}000Z-000.toml")), "schema = 1\n")
                .expect("history write");
        }
        std::fs::write(root.join("homes.json"), "{}").expect("homes write");
        std::fs::write(root.join("grants.json"), "{}").expect("grants write");
        std::fs::write(root.join(".zen.toml.swp"), "x").expect("swap write");
        std::thread::sleep(Duration::from_millis(1200));
        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            ".history/, homes.json, grants.json and swap files must never trigger the watcher"
        );

        std::fs::write(root.join(PAGES_DIR).join("home-1.toml"), "schema = 1\n").expect("page write");
        let deadline = Instant::now() + Duration::from_secs(5);
        while count.load(Ordering::SeqCst) == 1 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(count.load(Ordering::SeqCst), 2, "a page save must re-validate");

        stop.store(true, Ordering::SeqCst);
        handle.join().expect("watch thread");
        let _ = std::fs::remove_dir_all(&root);
    }
}
