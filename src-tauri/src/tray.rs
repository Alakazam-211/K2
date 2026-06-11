//! macOS menubar icon for the persistent-agents feature.
//!
//! Gives the user visibility into "what's running when K2SO is closed"
//! — the key UX gap that persistent agents introduces. The default
//! "keep daemon running on quit" preference is only safe because the
//! menubar icon surfaces the daemon's state outside the main window.
//!
//! Menu structure (intentionally minimal — the only things a user
//! actually needs to know from the menubar):
//!
//! ```text
//!   Server Status: Running  (PID 1234, up 2h 14m)
//!   Ngrok URL: https://abc.ngrok.app         (click → copy to clipboard)
//!   ─────────────────────────────────────
//!   Connected parties (2):
//!     · iPhone 15 Pro  2 min ago
//!     · iPad Pro       47 sec ago
//!   ─────────────────────────────────────
//!   Quit K2SO                          (honors keep-daemon-on-quit)
//! ```
//!
//! Updates every 10s via `MenuItem::set_text` on the status + URL
//! rows individually — replacing the whole menu via `tray.set_menu`
//! auto-closes a menu that's currently open, which was the 'menu
//! dismisses while I'm reading it' bug. We only do a full rebuild
//! when the parties count changes (rare), accepting the edge case
//! that an open menu closes in that moment.
//!
//! Color dots (rendered via Unicode emoji, which macOS draws in full
//! color even on template-style tray menus):
//!   🟢 Running
//!   🔴 Not running
//!   🟠 Unreachable / launching / crashed

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use parking_lot::Mutex;
use tauri::image::Image;
use tauri::menu::{IconMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Wry};

/// Menubar icon bytes — embedded at compile time so there's no
/// runtime file-path dependency (works regardless of where the
/// K2SO.app bundle is installed). The source file is a 44x44 RGBA
/// PNG with transparent "K2" / "SO" cutouts on a black rounded
/// square. Template mode (below) makes macOS auto-invert for dark
/// mode.
const TRAY_ICON_PNG: &[u8] = include_bytes!("../../resources/tray-icon.png");

/// Inline status-color icons for the "Server Status" menu row.
/// 32x32 PNGs with rounded-corner colored squares on a transparent
/// background. Rendered at macOS's native menu-icon size (~16pt),
/// which is significantly smaller than the emoji squares we had
/// before — the color indicator no longer dwarfs the text.
const STATUS_ICON_GREEN: &[u8] = include_bytes!("../../resources/status-green.png");
const STATUS_ICON_ORANGE: &[u8] = include_bytes!("../../resources/status-orange.png");
const STATUS_ICON_RED: &[u8] = include_bytes!("../../resources/status-red.png");

/// What the status row should be rendering right now. Drives both
/// the label text and which color icon to show.
#[derive(Clone, Copy, PartialEq, Eq)]
enum StatusState {
    Running,
    Unreachable,
    NotRunning,
}

impl StatusState {
    fn icon(&self) -> Option<Image<'_>> {
        let bytes = match self {
            StatusState::Running => STATUS_ICON_GREEN,
            StatusState::Unreachable => STATUS_ICON_ORANGE,
            StatusState::NotRunning => STATUS_ICON_RED,
        };
        Image::from_bytes(bytes).ok()
    }
}

/// Refresh interval for the tray menu contents. 10s is slow enough
/// that we don't spin on `daemon_status()` (which hits the daemon's
/// HTTP surface) but fast enough that "Running → Stopped" transitions
/// feel immediate to users who open the menu.
const REFRESH_INTERVAL: Duration = Duration::from_secs(10);

/// Menu-item IDs we listen for in the tray's `on_menu_event` handler.
/// Using `&'static str` constants rather than match-arms on dynamic
/// IDs so typos are caught at compile time.
mod ids {
    pub const COPY_TUNNEL_URL: &str = "k2so.tray.copy-tunnel-url";
    pub const QUIT: &str = "k2so.tray.quit";
}

/// Handle stored in Tauri state so the refresh loop can rebuild the
/// menu. `Arc<Mutex<>>` because both the refresh thread and the
/// menu-event handler need write access.
pub struct TrayState {
    pub app: AppHandle<Wry>,
}

/// Cached handles to the menu items whose labels we mutate over time
/// (status + Ngrok URL). Storing the `MenuItem<Wry>` clones here
/// lets the refresh thread call `.set_text(...)` to update the text
/// in place — which, on macOS, does NOT close an open menu. Full
/// menu replacements via `tray.set_menu(...)` *do* close it; that's
/// the behavior we're avoiding.
///
/// `last_party_count` tracks the number of connected companion
/// sessions we last rendered. If the count changes we fall back to
/// a full rebuild (variable-length lists can't be updated in place
/// without keeping a fixed slot pool — deferred). A full rebuild
/// *will* dismiss an open menu, but in normal usage the parties
/// count changes rarely and the user is not usually looking.
struct LiveTrayItems {
    status: IconMenuItem<Wry>,
    status_state: StatusState,
    ngrok: MenuItem<Wry>,
    parties_header: MenuItem<Wry>,
    last_party_count: usize,
}

static LIVE_ITEMS: OnceLock<Mutex<Option<LiveTrayItems>>> = OnceLock::new();

fn live_items_slot() -> &'static Mutex<Option<LiveTrayItems>> {
    LIVE_ITEMS.get_or_init(|| Mutex::new(None))
}

/// Install the tray icon + wire the refresh loop. Called once from
/// `lib.rs::setup()` AFTER the main window is created (so "Show K2SO"
/// has something to target).
pub fn install(app: &AppHandle<Wry>) -> Result<(), String> {
    let menu = build_menu(app)?;

    // Load the icon bytes into a `tauri::image::Image` and pass to
    // `.icon()`. Without this, the tray icon falls back to whatever
    // tauri.conf.json specified — but the config path creates a
    // SECOND tray that shadows the code-built one on macOS (Tauri
    // issue #11931). Our fix: remove `trayIcon` from the config and
    // attach the image here in Rust. One tray, one menu, menu pops.
    let icon = Image::from_bytes(TRAY_ICON_PNG)
        .map_err(|e| format!("decode tray icon: {e}"))?;

    let app_clone = app.clone();
    let _tray = TrayIconBuilder::with_id("k2so-main")
        .icon(icon)
        .icon_as_template(true)
        .tooltip("K2SO")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(move |app, event| handle_menu_event(app, event.id().as_ref()))
        .build(app)
        .map_err(|e| format!("build tray icon: {e}"))?;

    // Periodic refresh. Writes new text directly into the cached
    // MenuItems via `.set_text()` — this does NOT close a menu
    // that's currently open (unlike `tray.set_menu(...)` which
    // replaces the whole menu and dismisses it). Full rebuild only
    // triggers on parties-count changes (rare).
    let refresh_app = app.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(REFRESH_INTERVAL);
        refresh_in_place(&refresh_app);
    });

    // Stash a handle so future callers can trigger an immediate
    // refresh (e.g., right after daemon_install completes).
    app.manage(Arc::new(Mutex::new(TrayState {
        app: app_clone,
    })));
    Ok(())
}

/// Update live menu items in place — no menu replacement, no
/// auto-dismiss of a currently-open menu. Recomputes the three
/// dynamic labels (status, Ngrok URL, parties header) and calls
/// `set_text` on each cached handle. Full menu rebuild fires only
/// when the parties count has changed since the last refresh.
fn refresh_in_place(app: &AppHandle<Wry>) {
    let companion = k2_core::companion::companion_status();
    let party_count = companion
        .get("connectedClients")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;

    // Decide if the parties-count change forces a full rebuild.
    // Holding the lock briefly to read last_party_count without
    // keeping the guard across the rebuild.
    let needs_full_rebuild = {
        let guard = live_items_slot().lock();
        match guard.as_ref() {
            Some(items) => items.last_party_count != party_count,
            None => true, // never built — fall through to rebuild
        }
    };

    if needs_full_rebuild {
        if let Ok(menu) = build_menu(app) {
            if let Some(tray) = app.tray_by_id("k2so-main") {
                let _ = tray.set_menu(Some(menu));
            }
        }
        return;
    }

    // Same parties count — do the in-place label updates only.
    let (status_label, status_state) = compose_status_label();
    let tunnel_url = companion
        .get("tunnelUrl")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let ngrok_label = match &tunnel_url {
        Some(url) => format!("Ngrok URL: {}", url),
        None => "Ngrok URL: Offline".to_string(),
    };
    *last_tunnel_url_slot().lock() = tunnel_url;

    if let Some(items) = live_items_slot().lock().as_mut() {
        let _ = items.status.set_text(status_label);
        // Only touch the icon when the state actually changes —
        // set_icon re-decodes the PNG and is measurably heavier than
        // set_text. For the common case (state stable, only uptime
        // label ticking upward), we skip it entirely.
        if items.status_state != status_state {
            let _ = items.status.set_icon(status_state.icon());
            items.status_state = status_state;
        }
        let _ = items.ngrok.set_text(ngrok_label);
        // Parties header label doesn't change while count is
        // stable, so no update needed here.
    }
}

/// Force an immediate menu rebuild. Called from Tauri command
/// handlers after state-changing actions (daemon install/uninstall/
/// restart) so the user doesn't have to wait for the next 10s tick.
#[allow(dead_code)]
pub fn refresh(app: &AppHandle<Wry>) {
    if let Ok(menu) = build_menu(app) {
        if let Some(tray) = app.tray_by_id("k2so-main") {
            let _ = tray.set_menu(Some(menu));
        }
    }
}

fn build_menu(app: &AppHandle<Wry>) -> Result<Menu<Wry>, String> {
    let menu = Menu::new(app).map_err(|e| format!("menu new: {e}"))?;

    // 1. Server Status — enabled so macOS renders it in normal white
    // (disabled items render dimmed gray). Uses IconMenuItem so the
    // inline color square renders at native menu-icon size (~16pt)
    // rather than the oversized emoji scale.
    let (status_label, status_state) = compose_status_label();
    let status = IconMenuItem::with_id(
        app,
        "k2so.tray.server-status",
        status_label,
        true,
        status_state.icon(),
        None::<&str>,
    )
    .map_err(|e| format!("status: {e}"))?;
    menu.append(&status).map_err(|e| format!("status: {e}"))?;

    // 2. Ngrok URL — clickable, click copies to clipboard. Label reads
    // "Ngrok URL: <url>" if tunnel is up, otherwise "Ngrok URL: Offline"
    // (disabled). The URL lives in companion_status's `tunnelUrl`.
    let companion_status = k2_core::companion::companion_status();
    let tunnel_url = companion_status
        .get("tunnelUrl")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let (ngrok_label, ngrok_enabled) = match &tunnel_url {
        Some(url) => (format!("Ngrok URL: {}", url), true),
        None => ("Ngrok URL: Offline".to_string(), false),
    };
    let ngrok = MenuItem::with_id(
        app,
        ids::COPY_TUNNEL_URL,
        ngrok_label,
        ngrok_enabled,
        None::<&str>,
    )
    .map_err(|e| format!("ngrok: {e}"))?;
    menu.append(&ngrok).map_err(|e| format!("ngrok: {e}"))?;

    // 3. Connected parties list — always rendered so the user can see
    // the header even at zero. Header is a disabled label "Connected
    // parties (N):" and each session is a disabled sub-item below.
    let sep1 = PredefinedMenuItem::separator(app).map_err(|e| format!("sep1: {e}"))?;
    menu.append(&sep1).map_err(|e| format!("sep1: {e}"))?;

    let (parties_header, parties_items) = compose_parties(app, &companion_status);
    menu.append(&parties_header)
        .map_err(|e| format!("parties hdr: {e}"))?;
    for item in &parties_items {
        menu.append(item)
            .map_err(|e| format!("party item: {e}"))?;
    }

    // 4. Separator + Quit. Quit honors the keep-daemon-on-quit
    // setting via the normal ExitRequested handler in lib.rs — no
    // tray-local override.
    let sep2 = PredefinedMenuItem::separator(app).map_err(|e| format!("sep2: {e}"))?;
    menu.append(&sep2).map_err(|e| format!("sep2: {e}"))?;

    // IMPORTANT: no `CmdOrCtrl+Q` accelerator here. The OS-level Cmd+Q
    // already routes through our `RunEvent::ExitRequested` handler,
    // so a menu-item accelerator would create a duplicate binding and
    // on some Tauri v2 builds hides the item. Keeping the label only.
    let quit = MenuItem::with_id(app, ids::QUIT, "Quit K2SO", true, None::<&str>)
        .map_err(|e| format!("quit: {e}"))?;
    menu.append(&quit).map_err(|e| format!("quit: {e}"))?;

    // Stash the tunnel URL in a module-local so the menu-event
    // handler can read it back on click without a second HTTP call.
    *last_tunnel_url_slot().lock() = tunnel_url;

    // Cache handles to the items whose labels we'll update in place
    // during the 10s refresh. Also remember the current party count
    // so the refresh thread can detect when a full rebuild is needed
    // (parties count changed).
    let party_count = companion_status
        .get("connectedClients")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;
    *live_items_slot().lock() = Some(LiveTrayItems {
        status: status.clone(),
        status_state,
        ngrok: ngrok.clone(),
        parties_header: parties_header.clone(),
        last_party_count: party_count,
    });

    Ok(menu)
}

/// Snapshot of the current ngrok URL captured during the last menu
/// rebuild. Used by the `COPY_TUNNEL_URL` click handler — avoids a
/// second call to `companion_status()` just to get the same string
/// back. Standard `OnceLock<Mutex<Option<_>>>` pattern — no new deps.
static LAST_TUNNEL_URL: OnceLock<Mutex<Option<String>>> = OnceLock::new();

fn last_tunnel_url_slot() -> &'static Mutex<Option<String>> {
    LAST_TUNNEL_URL.get_or_init(|| Mutex::new(None))
}

/// Human-readable status line for the tray menu's disabled header.
///
/// "Server" in this context means "whatever is hosting the companion
/// tunnel + HTTP surface the mobile app talks to." That could be:
///   - the persistent daemon (release path, launchd-managed)
///   - the Tauri app itself, in-process (dev path + 0.33.0 fallback)
///
/// We prefer the daemon — it's the flagship path. If it's not
/// running we fall back to checking the Tauri in-process companion
/// via `companion_status().running`. Only when BOTH are absent do we
/// actually say "Not running."
fn compose_status_label() -> (String, StatusState) {
    // Color squares now live in the IconMenuItem's icon slot (see
    // build_menu) — small PNG files sized to macOS's menu-icon
    // standard. The returned label is plain text; the caller pairs
    // it with the icon from `StatusState::icon()`.

    // 1. Daemon first — it's authoritative when it's up. Show only
    //    uptime; PID + "daemon" prefix were noisy in the tray menu.
    if let crate::commands::daemon::DaemonStatusResponse::Running {
        uptime_secs, ..
    } = crate::commands::daemon::daemon_status()
    {
        return (
            format!("Server Status: Running  (up {})", format_uptime(uptime_secs)),
            StatusState::Running,
        );
    }

    // 2. Fallback: Tauri's in-process companion. No uptime reliably
    // available here — just report running.
    let companion = k2_core::companion::companion_status();
    let in_app_running = companion
        .get("running")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if in_app_running {
        ("Server Status: Running".to_string(), StatusState::Running)
    } else {
        // 3. Nothing hosting the companion. Orange for "unreachable"
        // (plist loaded but not answering — crash/launching), red for
        // "not running at all" (no plist, no in-app).
        match crate::commands::daemon::daemon_status() {
            crate::commands::daemon::DaemonStatusResponse::Unreachable { .. } => (
                "Server Status: Unreachable".to_string(),
                StatusState::Unreachable,
            ),
            _ => (
                "Server Status: Not running".to_string(),
                StatusState::NotRunning,
            ),
        }
    }
}

/// Build the "Connected parties" block. Returns `(header, items)`.
/// Always renders a header, even at zero — "Connected parties (0):"
/// is a valid status the user wants to see. Items only render when
/// sessions exist.
///
/// Reads from `k2_core::companion::companion_status()` which
/// returns the same `connectedClients` count the Settings pane's
/// companion row displays. Per-session detail (device name, last
/// activity) is surfaced from `sessions` in that response when the
/// daemon knows about clients; absent → just the count header.
fn compose_parties(
    app: &AppHandle<Wry>,
    companion_status: &serde_json::Value,
) -> (MenuItem<Wry>, Vec<MenuItem<Wry>>) {
    let count = companion_status
        .get("connectedClients")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let sessions_json = companion_status.get("sessions").and_then(|v| v.as_array());

    let header_text = format!("Connected parties ({}):", count);
    let header = MenuItem::with_id(
        app,
        "k2so.tray.parties-header",
        header_text,
        false,
        None::<&str>,
    )
    .unwrap_or_else(|_| {
        // Fallback label if MenuItem::with_id ever fails (shouldn't).
        // The unwrap branch lets compose_parties stay infallible so
        // the caller doesn't have to handle Option.
        MenuItem::with_id(app, "k2so.tray.parties-fallback", "Connected parties", false, None::<&str>)
            .expect("bare menu item must build")
    });

    let mut items = Vec::new();
    if let Some(arr) = sessions_json {
        for (i, s) in arr.iter().take(6).enumerate() {
            let device = s
                .get("device")
                .and_then(|v| v.as_str())
                .or_else(|| s.get("label").and_then(|v| v.as_str()))
                .unwrap_or("Unknown device");
            let ago = s
                .get("lastActivityAgo")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let label = if ago.is_empty() {
                format!("   · {}", device)
            } else {
                format!("   · {}  {}", device, ago)
            };
            // Each real party is enabled (normal white). Click is a
            // no-op for now — later we could route to a session-
            // detail view. Header above stays disabled (gray) because
            // it's just a section label.
            if let Ok(item) = MenuItem::with_id(
                app,
                format!("k2so.tray.party.{i}"),
                label,
                true,
                None::<&str>,
            ) {
                items.push(item);
            }
        }
    }

    (header, items)
}

fn format_uptime(secs: u64) -> String {
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    } else {
        format!("{}d {}h", secs / 86400, (secs % 86400) / 3600)
    }
}

fn handle_menu_event(app: &AppHandle<Wry>, id: &str) {
    match id {
        ids::COPY_TUNNEL_URL => {
            // Copy the ngrok URL captured during the last menu build.
            // Using the clipboard plugin the app already registers
                // (tauri-plugin-clipboard-manager) — no new dep.
            if let Some(url) = last_tunnel_url_slot().lock().clone() {
                use tauri_plugin_clipboard_manager::ClipboardExt;
                let _ = app.clipboard().write_text(url);
            }
        }
        ids::QUIT => {
            // Honors the "keep daemon running when K2SO quits" setting
            // via the RunEvent::ExitRequested handler in lib.rs. No
            // tray-local override — one quit path, one policy.
            app.exit(0);
        }
        _ => {}
    }
}
