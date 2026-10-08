//! Watch `~/.k2/zen/` and announce changes (prd-zen-mode-v1 Z12, vs-live Z61).
//!
//! Shape follows `charter_compose_watch` and the 0.40.104 RSS lesson:
//! - DIRECTORIES, non-recursive (`zen/`, `zen/gardens/`, `zen/themes/` and
//!   each `zen/themes/<name>/`), so editor rename-saves are seen and
//!   `.history/` is never walked;
//! - the bounded `notify_bound` channel (`try_send`, drop on full), so a
//!   burst can't grow memory;
//! - `should_observe` drops Access/Other kinds before paths are touched;
//! - a path filter that keeps only `zen.toml`, `gardens/<id>.toml`,
//!   `themes/<name>/theme.toml` and a theme's background images (never
//!   `.history/`, `gardens.json`, `active.json`, `grants.json` or editor
//!   temp files): `gardens.json` is written only by routes, which emit
//!   themselves (G18);
//! - a 250 ms trailing debounce: a burst of saves is one re-validate.
//!
//! Started lazily: at boot only when Zen is set up, otherwise by
//! `POST /cli/zen/setup`. One watcher per process.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};

use k2_core::log_debug;
use k2_core::zen::schema::image_ext;
use k2_core::zen::store::{valid_garden_id, GARDENS_DIR, THEMES_DIR, THEME_FILE, ZEN_FILE};
use k2_core::zen::valid_theme_name;
use k2_core::zen::widgets::{CODE_EXTS, WIDGETS_DIR};

use crate::notify_bound::{should_observe, DroppingHandler, NOTIFY_CHANNEL_BOUND};

pub const DEBOUNCE: Duration = Duration::from_millis(250);
/// How often the list of `themes/<name>/` folders is re-read (also right
/// after any change directly under `themes/`).
const THEME_RESCAN: Duration = Duration::from_secs(1);
const TICK: Duration = Duration::from_millis(50);

static STARTED: AtomicBool = AtomicBool::new(false);
static RUNNING: AtomicBool = AtomicBool::new(false);

/// Whether the watcher thread is alive (for `k2 zen doctor`).
pub fn is_running() -> bool {
    RUNNING.load(Ordering::Relaxed)
}

/// Boot hook: baseline + watch only when this computer has set Zen up
/// (it has a Garden list). Per-Home pages never shipped: nothing migrates.
pub fn start_at_boot() {
    if k2_core::zen::is_set_up() {
        // GS17: archive K2's defaults and run the Garden sync loader first.
        k2_core::zen::ZenFiles::local().sync_boot();
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

/// True for `<root>/zen.toml`, `<root>/gardens/<id>.toml`, `<root>/gardens`,
/// `<root>/themes`, `<root>/themes/<name>`, `<root>/themes/<name>/theme.toml`
/// and a theme's image files. Everything else (`.history/…`, `gardens.json`,
/// `active.json`, `grants.json`, `zen.toml.swp`, `.#zen.toml`, atomic-write
/// temp files) is ignored.
pub(crate) fn is_zen_source(roots: &[PathBuf], path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let Some(parent) = path.parent() else { return false };
    let is_root = |p: &Path| roots.iter().any(|r| r == p);
    if is_root(parent) {
        return name == ZEN_FILE || name == GARDENS_DIR || name == THEMES_DIR || name == WIDGETS_DIR;
    }
    let parent_name = parent.file_name().and_then(|n| n.to_str());
    let grand = parent.parent();
    if parent_name == Some(GARDENS_DIR) && grand.is_some_and(is_root) {
        return name.strip_suffix(".toml").is_some_and(valid_garden_id);
    }
    if parent_name == Some(THEMES_DIR) && grand.is_some_and(is_root) {
        return valid_theme_name(name);
    }
    if parent_name == Some(WIDGETS_DIR) && grand.is_some_and(is_root) {
        return valid_theme_name(name);
    }
    // A widget folder's files (UW12): code, images, fonts (and .svg, so its
    // error shows). Never dotfiles or editor temp files.
    let in_widget = parent_name.is_some_and(valid_theme_name)
        && grand.is_some_and(|g| {
            g.file_name().and_then(|n| n.to_str()) == Some(WIDGETS_DIR) && g.parent().is_some_and(is_root)
        });
    if in_widget {
        let lower = name.to_ascii_lowercase();
        let ext = lower.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
        return !name.starts_with('.')
            && (CODE_EXTS.contains(&ext) || k2_core::zen::bundle::asset_mime(&lower).is_some() || ext == "svg");
    }
    let in_theme = parent_name.is_some_and(valid_theme_name)
        && grand.is_some_and(|g| {
            g.file_name().and_then(|n| n.to_str()) == Some(THEMES_DIR) && g.parent().is_some_and(is_root)
        });
    in_theme && !name.starts_with('.') && (name == THEME_FILE || image_ext(name).is_some())
}

/// `themes/<name>/` and `widgets/<name>/` folders to watch now.
fn theme_dirs(root: &Path) -> BTreeSet<PathBuf> {
    let mut out = sub_dirs(root, THEMES_DIR);
    out.extend(sub_dirs(root, WIDGETS_DIR));
    out
}

fn sub_dirs(root: &Path, dir: &str) -> BTreeSet<PathBuf> {
    std::fs::read_dir(root.join(dir))
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().is_dir() && valid_theme_name(&e.file_name().to_string_lossy()))
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default()
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
    let gardens = root.join(GARDENS_DIR);
    let themes = root.join(THEMES_DIR);
    let widgets = root.join(WIDGETS_DIR);
    let mut gardens_watched = false;
    let mut themes_watched = false;
    let mut widgets_watched = false;
    let mut theme_watched: BTreeSet<PathBuf> = BTreeSet::new();
    let mut last_scan = Instant::now() - THEME_RESCAN;
    let roots = root_forms(root);
    let mut pending: Option<Instant> = None;
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        if !gardens_watched && gardens.is_dir() {
            gardens_watched = watcher.watch(&gardens, RecursiveMode::NonRecursive).is_ok();
        } else if gardens_watched && !gardens.is_dir() {
            gardens_watched = false;
        }
        if !themes_watched && themes.is_dir() {
            themes_watched = watcher.watch(&themes, RecursiveMode::NonRecursive).is_ok();
            last_scan = Instant::now() - THEME_RESCAN;
        } else if themes_watched && !themes.is_dir() {
            themes_watched = false;
        }
        if !widgets_watched && widgets.is_dir() {
            widgets_watched = watcher.watch(&widgets, RecursiveMode::NonRecursive).is_ok();
            last_scan = Instant::now() - THEME_RESCAN;
        } else if widgets_watched && !widgets.is_dir() {
            widgets_watched = false;
        }
        // Theme and widget folders come and go (`k2 zen theme new`,
        // `k2 zen widget new`, a hand mkdir): add a non-recursive watch for
        // each new one, at most once a second.
        if (themes_watched || widgets_watched) && last_scan.elapsed() >= THEME_RESCAN {
            last_scan = Instant::now();
            let now = theme_dirs(root);
            theme_watched.retain(|d| now.contains(d));
            for d in now {
                if !theme_watched.contains(&d) && watcher.watch(&d, RecursiveMode::NonRecursive).is_ok() {
                    theme_watched.insert(d);
                    // A folder made with its files already inside: re-read.
                    pending = Some(Instant::now());
                }
            }
        }
        match rx.recv_timeout(TICK) {
            Ok(Ok(ev)) => {
                if should_observe(ev.kind) && ev.paths.iter().any(|p| is_zen_source(&roots, p)) {
                    pending = Some(Instant::now());
                    if ev
                        .paths
                        .iter()
                        .any(|p| p.parent().is_some_and(|pp| pp.ends_with(THEMES_DIR) || pp.ends_with(WIDGETS_DIR)))
                    {
                        last_scan = Instant::now() - THEME_RESCAN;
                    }
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
        std::fs::create_dir_all(dir.join(GARDENS_DIR)).expect("mkdir zen/gardens");
        std::fs::write(dir.join(ZEN_FILE), "schema = 1\n").expect("write zen.toml");
        dir
    }

    #[test]
    fn source_filter_keeps_only_zen_files() {
        let root = PathBuf::from("/tmp/k2zen");
        let roots = vec![root.clone()];
        assert!(is_zen_source(&roots, &root.join("zen.toml")));
        assert!(is_zen_source(&roots, &root.join("gardens/g-3f9a12c0.toml")));
        assert!(is_zen_source(&roots, &root.join("gardens/abc-123.toml")));
        assert!(is_zen_source(&roots, &root.join("gardens")));
        assert!(is_zen_source(&roots, &root.join("themes")));
        assert!(is_zen_source(&roots, &root.join("themes/sunset")));
        assert!(is_zen_source(&roots, &root.join("themes/sunset/theme.toml")));
        assert!(is_zen_source(&roots, &root.join("themes/sunset/background.jpg")));
        assert!(is_zen_source(&roots, &root.join("themes/sunset/wall.PNG")));
        // Zen v2 widget folders (UW12).
        for yes in [
            "widgets",
            "widgets/agent-arcade",
            "widgets/agent-arcade/manifest.json",
            "widgets/agent-arcade/index.html",
            "widgets/agent-arcade/game.js",
            "widgets/agent-arcade/game.css",
            "widgets/agent-arcade/cat.png",
            "widgets/agent-arcade/font.woff2",
            "widgets/agent-arcade/logo.svg",
        ] {
            assert!(is_zen_source(&roots, &root.join(yes)), "{yes} must be watched");
        }
        for no in [
            "widgets/Arcade/index.html",
            "widgets/agent-arcade/.game.js.swp",
            "widgets/agent-arcade/game.js~",
            "widgets/agent-arcade/notes.md",
            "widgets/agent-arcade/assets/cat.png",
            ".history/widgets/agent-arcade/last-good.html",
        ] {
            assert!(!is_zen_source(&roots, &root.join(no)), "{no} must be ignored");
        }
        for no in [
            "gardens.json",
            "homes.json",
            "grants.json",
            "pages",
            "pages/abc-123.toml",
            ".history/gardens/g-3f9a12c0.toml/20261004T120000000Z-000.toml",
            ".history/gardens/g-3f9a12c0.toml/deleted.json",
            ".history",
            ".history/zen.toml/20261004T120000000Z-000.toml",
            "zen.toml.swp",
            ".zen.toml.swp",
            ".#zen.toml",
            "zen.toml~",
            "gardens/abc.toml.tmp",
            "gardens/.hidden.toml",
            "gardens/sub/x.toml",
            "gardens/a b.toml",
            "active.json",
            "themes/Sunset/theme.toml",
            "themes/sunset/notes.md",
            "themes/sunset/.theme.toml.swp",
            "themes/sunset/theme.toml.tmp",
            "themes/sunset/sub/theme.toml",
            ".history/themes/sunset/theme.toml/20261004T120000000Z-000.toml",
        ] {
            assert!(!is_zen_source(&roots, &root.join(no)), "{no} must be ignored");
        }
        assert!(!is_zen_source(&roots, Path::new("/elsewhere/zen.toml")));
    }

    /// T1.7 / TG1.7: a 500-write burst to one Garden page is one
    /// re-validate; `.history/`, `gardens.json`, `grants.json` and `pages/`
    /// writes are none; the channel is the shared bound.
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

        let page = root.join(GARDENS_DIR).join("g-3f9a12c0.toml");
        for i in 0..500 {
            std::fs::write(&page, format!("schema = 1\n# save {i}\n")).expect("burst write");
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
        std::fs::write(root.join("gardens.json"), "{}").expect("gardens.json write");
        std::fs::write(root.join("grants.json"), "{}").expect("grants write");
        std::fs::create_dir_all(root.join("pages")).expect("mkdir pages");
        std::fs::write(root.join("pages").join("home-1.toml"), "schema = 1\n").expect("pages write");
        std::fs::write(root.join(".zen.toml.swp"), "x").expect("swap write");
        std::thread::sleep(Duration::from_millis(1200));
        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            ".history/, gardens.json, grants.json, pages/ and swap files must never trigger the watcher"
        );

        std::fs::write(root.join(ZEN_FILE), "schema = 1\n# zen save\n").expect("zen.toml write");
        let deadline = Instant::now() + Duration::from_secs(5);
        while count.load(Ordering::SeqCst) == 1 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(count.load(Ordering::SeqCst), 2, "a zen.toml save must re-validate");

        stop.store(true, Ordering::SeqCst);
        handle.join().expect("watch thread");
        let _ = std::fs::remove_dir_all(&root);
    }
}
