use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::Mutex;
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc;
use std::sync::LazyLock;
use std::time::{Duration, Instant};
use tauri::Emitter;

/// Phase 2 close-out (2026-05-23): the watcher set used to live on
/// `AppState.watchers` and was injected via `tauri::State<AppState>`.
/// Once Units 1-4 emptied every other field out of `AppState`, owning
/// a struct + Tauri `.manage()` registration just for this one map was
/// pure ceremony. Inlined here as a `LazyLock<Mutex<HashMap<_, _>>>`
/// so `state.rs` could be deleted entirely and the surface stays
/// SQL-free / DB-free / LLM-free / terminal-free.
///
/// `parking_lot::Mutex` (matches the rest of the codebase — see
/// `k2_core::db::shared`) is used so we keep the no-poison API
/// invariant. `LazyLock` lets us skip a `.manage()` boot step entirely;
/// the first `fs_watch_dir` / `fs_unwatch_dir` call constructs the map.
static WATCHERS: LazyLock<Mutex<HashMap<String, RecommendedWatcher>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Clone, Serialize)]
struct FsChangePayload {
    path: String,
    kind: String,
}

fn event_kind_label(kind: &EventKind) -> &'static str {
    match kind {
        EventKind::Create(_) => "create",
        EventKind::Modify(_) => "modify",
        EventKind::Remove(_) => "remove",
        _ => "modify",
    }
}

#[tauri::command]
pub fn fs_watch_dir(app: tauri::AppHandle, path: String) -> Result<(), String> {
    let mut watchers = WATCHERS.lock();

    // Already watching this path
    if watchers.contains_key(&path) {
        return Ok(());
    }

    let watch_path = path.clone();
    let app_handle = app.clone();

    let (tx, rx) = mpsc::channel::<Result<Event, notify::Error>>();

    let mut watcher = RecommendedWatcher::new(tx, Config::default().with_poll_interval(Duration::from_millis(200)))
        .map_err(|e| format!("Failed to create watcher: {e}"))?;

    watcher
        .watch(Path::new(&watch_path), RecursiveMode::Recursive)
        .map_err(|e| format!("Failed to watch directory: {e}"))?;

    // Spawn a thread to receive events, coalesce them, and emit deduplicated changes.
    // Within each debounce window (200ms), multiple events for the same path are
    // collapsed into a single emission. This prevents rapid saves from flooding
    // the frontend with redundant directory reloads.
    std::thread::spawn(move || {
        let debounce = Duration::from_millis(200);

        loop {
            // Block until the first event arrives (or channel disconnects)
            let first = match rx.recv_timeout(Duration::from_secs(5)) {
                Ok(Ok(event)) => event,
                Ok(Err(_)) => continue,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };

            // Collect unique (path → kind) pairs within the debounce window.
            // Later events for the same path overwrite earlier ones.
            let mut coalesced: HashMap<String, String> = HashMap::new();

            for p in &first.paths {
                coalesced.insert(
                    p.to_string_lossy().to_string(),
                    event_kind_label(&first.kind).to_string(),
                );
            }

            // Drain additional events within the debounce window
            let deadline = Instant::now() + debounce;
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                match rx.recv_timeout(remaining) {
                    Ok(Ok(event)) => {
                        let kind = event_kind_label(&event.kind).to_string();
                        for p in &event.paths {
                            coalesced.insert(p.to_string_lossy().to_string(), kind.clone());
                        }
                    }
                    Ok(Err(_)) => {}
                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }

            // Emit once per debounce window with the full batch — one IPC
            // crossing per window instead of one per path. Live baseline
            // measured windows of up to 592 paths; this collapses those
            // into a single emit.
            //
            // Frontend listener receives a Vec<FsChangePayload>. The same
            // pattern survives the eventual Tauri-app → daemon split (P4):
            // `ipc_channel.send("fs://change", Vec<FsChangePayload>)`
            // across a Unix socket carries the same shape.
            let batch: Vec<FsChangePayload> = coalesced
                .into_iter()
                .map(|(path, kind)| FsChangePayload { path, kind })
                .collect();
            let batch_size = batch.len();
            if !batch.is_empty() {
                let _ = app_handle.emit("fs://change", &batch);
            }
            if crate::perf::is_enabled() {
                use std::io::Write;
                let _ = writeln!(
                    std::io::stderr(),
                    "[perf] fs_watcher_batch — batch_size={} emit_count={}",
                    batch_size,
                    if batch_size == 0 { 0 } else { 1 },
                );
            }
        }
    });

    watchers.insert(path, watcher);
    Ok(())
}

#[tauri::command]
pub fn fs_unwatch_dir(_app: tauri::AppHandle, path: String) -> Result<(), String> {
    let mut watchers = WATCHERS.lock();

    // Removing the watcher drops it, which stops watching and disconnects the channel
    watchers.remove(&path);
    Ok(())
}
