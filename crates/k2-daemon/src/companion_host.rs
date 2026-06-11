//! Daemon-side glue for k2so-core's companion bridges.
//!
//! Phase 2 Unit 1 — moves companion (ngrok tunnel) lifecycle ownership
//! from Tauri to the daemon. Registers implementations of
//! `CompanionSettingsProvider`, `TerminalProvider`, `CompanionEventSink`,
//! and `AppEventSource` that wrap daemon-owned state (the shared
//! `TerminalManager` singleton, the daemon's broadcast event channel,
//! the on-disk `~/.k2so/settings.json`) so the companion module — which
//! lives in k2so-core and can't depend on Tauri — can still start its
//! tunnel, emit lifecycle events, mirror agent events to WS clients,
//! and read terminal snapshots without an `AppHandle`.
//!
//! Call [`register`] once during daemon startup before any companion
//! code runs.

use std::sync::Arc;

use k2_core::companion::{
    app_event_source::{AppEventHandler, AppEventSource},
    event_sink::CompanionEventSink,
    settings_bridge::{CompanionSettingsProvider, CompanionSettingsSnapshot},
    terminal_bridge::TerminalProvider,
};
use k2_core::log_debug;
use k2_core::terminal::grid_types::GridUpdate;
use std::sync::Mutex as StdMutex;
use tokio::sync::broadcast;

use crate::events::WireEvent;

/// Reads terminal grids + scrollback through the process-wide shared
/// `TerminalManager` (`k2_core::terminal::shared()`). No `AppHandle`
/// needed — daemon and Tauri see the same singleton.
struct DaemonTerminalProvider;

impl TerminalProvider for DaemonTerminalProvider {
    fn get_grid(&self, terminal_id: &str) -> Result<GridUpdate, String> {
        let manager = k2_core::terminal::shared();
        let manager = manager.lock();
        manager.get_grid(terminal_id)
    }

    fn read_lines_with_scrollback(
        &self,
        terminal_id: &str,
        limit: usize,
        include_scrollback: bool,
    ) -> Result<Vec<String>, String> {
        let manager = k2_core::terminal::shared();
        let manager = manager.lock();
        manager.read_lines_with_scrollback(terminal_id, limit, include_scrollback)
    }
}

/// Routes core's `emit(event, payload)` onto the daemon's broadcast
/// channel as a `WireEvent`. Companion lifecycle events
/// (`companion:tunnel_activated`, etc.) thus reach `/events` WS
/// subscribers — including the Tauri thin client's `daemon_events.rs`
/// re-emitter — without any extra plumbing.
///
/// Because `WireEvent::event` is `&'static str` and companion calls
/// `emit(&str, ..)` with arbitrary names, we intern the known
/// companion event names here. Unknown names are logged + dropped —
/// these aren't user-facing and a missing toast doesn't affect tunnel
/// behavior.
struct DaemonCompanionEventSink {
    tx: broadcast::Sender<WireEvent>,
}

impl CompanionEventSink for DaemonCompanionEventSink {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        // Phase 2 Unit 3: WireEvent.event is `String` now (was
        // `&'static str`) so we can pass the dynamic event name
        // straight through. Still guard known names so a typo in
        // a future companion event surfaces in logs instead of
        // silently broadcasting.
        match event {
            "companion:tunnel_activated"
            | "companion:tunnel_deactivated" => {}
            other => {
                log_debug!(
                    "[daemon/companion] dropping unknown companion event '{other}'"
                );
                return;
            }
        }
        let _ = self.tx.send(WireEvent {
            event: event.to_string(),
            payload,
        });
    }
}

/// Subscribes companion handlers to host-app events by tapping the
/// daemon's broadcast channel — the same channel `DaemonBroadcastSink`
/// pushes hook events into. The handler is called for every
/// `WireEvent` whose `event` name matches one of `events`.
///
/// Handlers are kept alive by holding a strong `Arc` in a static
/// keep-alive list (same lifetime semantics as the Tauri impl).
///
/// **Why we capture a `Handle` rather than calling `tokio::spawn`
/// directly:** companion's `subscribe()` runs inside
/// `start_companion()` which itself executes on a
/// `std::thread::spawn`'d worker thread (not a tokio context). A
/// bare `tokio::spawn` from there panics with "no reactor running".
/// We snapshot the daemon's multi-thread runtime `Handle` at
/// `register()` time and use it to dispatch the subscriber task.
struct DaemonAppEventSource {
    tx: broadcast::Sender<WireEvent>,
    handle: tokio::runtime::Handle,
}

static HANDLER_KEEPALIVE: StdMutex<Vec<Arc<AppEventHandler>>> =
    StdMutex::new(Vec::new());

impl AppEventSource for DaemonAppEventSource {
    fn subscribe(&self, events: &[&'static str], handler: AppEventHandler) {
        let handler_arc: Arc<AppEventHandler> = Arc::new(handler);
        if let Ok(mut keepalive) = HANDLER_KEEPALIVE.lock() {
            keepalive.push(handler_arc.clone());
        }
        let wanted: Vec<&'static str> = events.to_vec();
        let mut rx = self.tx.subscribe();
        // Dispatch via the captured runtime handle so this works
        // when called from a non-tokio caller (see struct docs).
        self.handle.spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(frame) => {
                        // Phase 2 Unit 3: WireEvent.event is now `String`
                        // (so we can carry dynamic terminal-event names
                        // like `terminal:grid:<uuid>`). The wanted-list
                        // is still static-name based, so compare on
                        // `as_str`.
                        let event_name = frame.event.as_str();
                        if !wanted.iter().any(|w| *w == event_name) {
                            continue;
                        }
                        // companion's handler signature expects a JSON
                        // string payload (matches the Tauri `event.payload()`
                        // form, which is a raw JSON-encoded string).
                        let payload_str =
                            serde_json::to_string(&frame.payload).unwrap_or_default();
                        (handler_arc)(event_name, &payload_str);
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        log_debug!(
                            "[daemon/companion] app-event subscriber lagged by {n} frames"
                        );
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }
}

/// Daemon-side settings provider. Reads `~/.k2so/settings.json` on
/// every call — no caching, because Tauri's `settings_update` writes
/// the same file from another process and we want to see those
/// updates immediately. Only the `companion` sub-object is parsed;
/// other top-level keys are ignored.
///
/// The provider returns sensible defaults if the file is missing,
/// malformed, or the `companion` key is absent — matching the
/// behavior of `AppSettings::default()` on the Tauri side.
struct DaemonCompanionSettingsProvider;

impl CompanionSettingsProvider for DaemonCompanionSettingsProvider {
    fn read(&self) -> CompanionSettingsSnapshot {
        read_companion_settings_from_disk().unwrap_or_default()
    }

    fn clear_password_hash_after_migration(&self) {
        if let Err(e) = clear_companion_password_hash_on_disk() {
            log_debug!(
                "[daemon/companion] WARN: clear_password_hash_after_migration: {e}"
            );
        }
    }
}

/// Read the `companion` block from `~/.k2so/settings.json` into a
/// snapshot. Returns `None` if the file or block isn't present;
/// returns `Some(default)` if the JSON is malformed (so the daemon
/// keeps booting even with a partially corrupted settings file).
fn read_companion_settings_from_disk() -> Option<CompanionSettingsSnapshot> {
    let path = settings_path()?;
    let raw = std::fs::read_to_string(&path).ok()?;
    let parsed: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            log_debug!(
                "[daemon/companion] settings.json malformed at {path:?}: {e} — using defaults"
            );
            return Some(CompanionSettingsSnapshot::default());
        }
    };
    let c = parsed.get("companion")?;
    Some(CompanionSettingsSnapshot {
        username: c
            .get("username")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        password_hash: c
            .get("passwordHash")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        password_set: c
            .get("passwordSet")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        ngrok_auth_token: c
            .get("ngrokAuthToken")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        ngrok_domain: c
            .get("ngrokDomain")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        cors_origins: c
            .get("corsOrigins")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default(),
        allow_remote_spawn: c
            .get("allowRemoteSpawn")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        auto_start: c
            .get("autoStart")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    })
}

/// Clear `companion.passwordHash` in `~/.k2so/settings.json`. Used by
/// the auth module after migrating the hash into the macOS Keychain.
/// Best-effort: missing file or unwritable settings is logged but not
/// fatal — the next start attempt will re-discover the keychain hash.
///
/// Writes back the full top-level JSON (preserving other keys) so we
/// don't accidentally strip unrelated settings.
fn clear_companion_password_hash_on_disk() -> Result<(), String> {
    let path = settings_path().ok_or_else(|| "no home dir".to_string())?;
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| format!("read {path:?}: {e}"))?;
    let mut parsed: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| format!("parse {path:?}: {e}"))?;
    if let Some(obj) = parsed.as_object_mut() {
        if let Some(companion) = obj.get_mut("companion").and_then(|v| v.as_object_mut()) {
            companion.insert(
                "passwordHash".to_string(),
                serde_json::Value::String(String::new()),
            );
            companion.insert("passwordSet".to_string(), serde_json::Value::Bool(true));
        }
    }
    let json = serde_json::to_string_pretty(&parsed)
        .map_err(|e| format!("serialize: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json).map_err(|e| format!("write {tmp:?}: {e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("rename {tmp:?} -> {path:?}: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn settings_path() -> Option<std::path::PathBuf> {
    Some(dirs::home_dir()?.join(".k2").join("settings.json"))
}

/// Persist `companion.passwordHash` + `companion.passwordSet` to the
/// on-disk settings.json. Mirrors the post-write the Tauri
/// `companion_set_password` command does via `settings_update`, but
/// without going through that command (daemon doesn't own the full
/// settings surface until Unit 7). Preserves every other key in the
/// file by round-tripping through `serde_json::Value`.
///
/// On success the file is written atomically (`tmp` + `rename`) with
/// mode 0o600 so the hash isn't world-readable.
pub fn persist_companion_password_fields(
    password_hash: &str,
    password_set: bool,
) -> Result<(), String> {
    let path = settings_path().ok_or_else(|| "no home dir".to_string())?;
    // Ensure parent dir exists. settings.json may not exist yet on a
    // fresh install where the user enters companion creds before any
    // other Tauri settings have been written.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("mkdir {parent:?}: {e}"))?;
    }
    let mut parsed: serde_json::Value = match std::fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str(&raw)
            .unwrap_or_else(|_| serde_json::json!({})),
        Err(_) => serde_json::json!({}),
    };
    if !parsed.is_object() {
        parsed = serde_json::json!({});
    }
    let obj = parsed.as_object_mut().expect("ensured object above");
    let companion = obj
        .entry("companion".to_string())
        .or_insert_with(|| serde_json::json!({}));
    if !companion.is_object() {
        *companion = serde_json::json!({});
    }
    let companion_obj = companion.as_object_mut().expect("ensured object above");
    companion_obj.insert(
        "passwordHash".to_string(),
        serde_json::Value::String(password_hash.to_string()),
    );
    companion_obj.insert(
        "passwordSet".to_string(),
        serde_json::Value::Bool(password_set),
    );
    let json = serde_json::to_string_pretty(&parsed)
        .map_err(|e| format!("serialize: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json).map_err(|e| format!("write {tmp:?}: {e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("rename {tmp:?} -> {path:?}: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Register all four daemon-side bridges on the k2so-core ambient
/// singletons. Must run from inside the tokio runtime (so we can
/// capture a `Handle`) before companion code emits / subscribes /
/// reads settings or terminals for the first time.
pub fn register(event_tx: broadcast::Sender<WireEvent>) {
    let handle = tokio::runtime::Handle::current();
    k2_core::companion::settings_bridge::set_provider(Box::new(
        DaemonCompanionSettingsProvider,
    ));
    k2_core::companion::terminal_bridge::set_provider(Box::new(DaemonTerminalProvider));
    k2_core::companion::event_sink::set_sink(Box::new(DaemonCompanionEventSink {
        tx: event_tx.clone(),
    }));
    k2_core::companion::app_event_source::set_source(Box::new(DaemonAppEventSource {
        tx: event_tx,
        handle,
    }));
    log_debug!(
        "[daemon/companion] registered settings/terminal/event/app-event bridges"
    );
}

/// First-boot autostart hook. Reads `companion.auto_start` from the
/// settings provider and, if every credential is set, starts the
/// ngrok tunnel on a detached thread (mirrors the Tauri-side
/// behavior in `src-tauri/src/lib.rs` pre-Phase-2). Returns
/// immediately — the actual tunnel start happens in the background
/// so a slow ngrok connect doesn't block daemon boot.
///
/// Idempotent: if companion is already running (auto_start fires
/// twice, or a manual `/cli/companion/start` ran first) the inner
/// `start_companion()` returns an `already running` error which the
/// retry loop treats as terminal.
pub fn maybe_autostart() {
    std::thread::spawn(|| {
        // Wait briefly so the daemon's HTTP server (which the tunnel
        // proxies to) is fully bound before we ask ngrok to forward.
        std::thread::sleep(std::time::Duration::from_secs(3));
        let snap = k2_core::companion::settings_bridge::read_settings();
        if !snap.auto_start {
            return;
        }
        if snap.username.is_empty() || snap.ngrok_auth_token.is_empty() {
            log_debug!(
                "[daemon/companion] auto_start enabled but credentials incomplete — skipping"
            );
            return;
        }
        if !k2_core::companion::auth::has_password() {
            log_debug!(
                "[daemon/companion] auto_start enabled but no password configured — skipping"
            );
            return;
        }

        // Retry pattern mirrors the Tauri lib.rs implementation: ngrok
        // free tier permits one session at a time and lingers ~30-60s
        // after a previous process death.
        let delays = [0u64, 5, 10, 20];
        for (i, delay) in delays.iter().enumerate() {
            if *delay > 0 {
                log_debug!(
                    "[daemon/companion] auto-start retry {} in {}s...",
                    i + 1,
                    delay
                );
                std::thread::sleep(std::time::Duration::from_secs(*delay));
            }
            match k2_core::companion::start_companion() {
                Ok(url) => {
                    log_debug!("[daemon/companion] auto-started: {url}");
                    return;
                }
                Err(e) => {
                    log_debug!(
                        "[daemon/companion] auto-start attempt {} failed: {}",
                        i + 1,
                        e
                    );
                    if e.contains("already running") || e.contains("cancelled") {
                        return;
                    }
                }
            }
        }
        log_debug!(
            "[daemon/companion] auto-start gave up after {} attempts",
            delays.len()
        );
    });
}
