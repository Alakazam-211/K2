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
//!   Show K2                                  (brings the window back)
//!   ─────────────────────────────────────
//!   Server Status: Running  (up 2h 14m)
//!   ─────────────────────────────────────
//!   Quit K2                            (honors keep-daemon-on-quit)
//! ```
//!
//! Updates every 10s via `set_text` on the status row — replacing the
//! whole menu via `tray.set_menu` auto-closes a menu that's currently
//! open, which was the 'menu dismisses while I'm reading it' bug.
//!
//! Color dots (rendered via Unicode emoji, which macOS draws in full
//! color even on template-style tray menus):
//!   🟢 Running
//!   🔴 Not running
//!   🟠 Unreachable / launching / crashed

use std::sync::OnceLock;
use std::time::Duration;

use parking_lot::Mutex;
use tauri::image::Image;
use tauri::menu::{IconMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Wry};

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
    pub const SHOW: &str = "k2so.tray.show";
    pub const QUIT: &str = "k2so.tray.quit";
}

/// Cached handle to the status row, whose label we change over time.
/// Storing the clone here lets the refresh thread call `.set_text(...)`
/// to update the text in place — which, on macOS, does NOT close an
/// open menu. Full menu replacements via `tray.set_menu(...)` *do*
/// close it; that's the behavior we're avoiding.
struct LiveTrayItems {
    status: IconMenuItem<Wry>,
    status_state: StatusState,
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

    // Windows: a left click on a tray icon opens the app and the right
    // click shows the menu (the Windows convention). Linux keeps the
    // menu on any click.
    let _tray = TrayIconBuilder::with_id("k2so-main")
        .icon(icon)
        .icon_as_template(true)
        .tooltip("K2")
        .menu(&menu)
        .show_menu_on_left_click(!cfg!(windows))
        .on_menu_event(move |app, event| handle_menu_event(app, event.id().as_ref()))
        .on_tray_icon_event(|tray, event| {
            if cfg!(windows) && opens_window(&event) {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)
        .map_err(|e| format!("build tray icon: {e}"))?;

    // Periodic refresh. Writes new text directly into the cached
    // status item via `.set_text()` — this does NOT close a menu
    // that's currently open (unlike `tray.set_menu(...)` which
    // replaces the whole menu and dismisses it).
    let refresh_app = app.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(REFRESH_INTERVAL);
        refresh_in_place(&refresh_app);
    });

    Ok(())
}

/// Update the status row in place — no menu replacement, no
/// auto-dismiss of a currently-open menu. Builds the menu once if it
/// was never built.
fn refresh_in_place(app: &AppHandle<Wry>) {
    if live_items_slot().lock().is_none() {
        refresh(app);
        return;
    }
    let (status_label, status_state) = compose_status_label();
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
    }
}

/// Force an immediate menu rebuild. Called from Tauri command
/// handlers after state-changing actions (daemon install/uninstall/
/// restart) so the user doesn't have to wait for the next 10s tick.
pub fn refresh(app: &AppHandle<Wry>) {
    if let Ok(menu) = build_menu(app) {
        if let Some(tray) = app.tray_by_id("k2so-main") {
            let _ = tray.set_menu(Some(menu));
        }
    }
}

/// Bring the main window back: closing it with "keep running" on only
/// hides it, so the tray (and a second launch, on Windows) is the way
/// back.
pub fn show_main_window(app: &AppHandle<Wry>) {
    use tauri::Manager;
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
    }
}

/// A left-button click released on the tray icon (Windows opens the app
/// on that, as other tray apps do).
fn opens_window(event: &tauri::tray::TrayIconEvent) -> bool {
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
    matches!(
        event,
        TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. }
    )
}

fn build_menu(app: &AppHandle<Wry>) -> Result<Menu<Wry>, String> {
    let menu = Menu::new(app).map_err(|e| format!("menu new: {e}"))?;

    // 0. Show K2 — the way back to a window that "keep running" hid.
    let show = MenuItem::with_id(app, ids::SHOW, "Show K2", true, None::<&str>)
        .map_err(|e| format!("show: {e}"))?;
    menu.append(&show).map_err(|e| format!("show: {e}"))?;
    let sep0 = PredefinedMenuItem::separator(app).map_err(|e| format!("sep0: {e}"))?;
    menu.append(&sep0).map_err(|e| format!("sep0: {e}"))?;

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

    // 2. Separator + Quit. Quit honors the keep-daemon-on-quit
    // setting via the normal ExitRequested handler in lib.rs — no
    // tray-local override.
    let sep1 = PredefinedMenuItem::separator(app).map_err(|e| format!("sep1: {e}"))?;
    menu.append(&sep1).map_err(|e| format!("sep1: {e}"))?;

    // IMPORTANT: no `CmdOrCtrl+Q` accelerator here. The OS-level Cmd+Q
    // already routes through our `RunEvent::ExitRequested` handler,
    // so a menu-item accelerator would create a duplicate binding and
    // on some Tauri v2 builds hides the item. Keeping the label only.
    let quit = MenuItem::with_id(app, ids::QUIT, "Quit K2", true, None::<&str>)
        .map_err(|e| format!("quit: {e}"))?;
    menu.append(&quit).map_err(|e| format!("quit: {e}"))?;

    // Cache the status item so the 10s refresh can update its label
    // in place.
    *live_items_slot().lock() = Some(LiveTrayItems { status: status.clone(), status_state });

    Ok(menu)
}

/// Human-readable status line for the tray menu's status row: the
/// persistent daemon's state. Green when it answers (with uptime),
/// orange when its files are there but it doesn't answer (crashed or
/// launching), red when it isn't installed.
fn compose_status_label() -> (String, StatusState) {
    // Color squares live in the IconMenuItem's icon slot (see
    // build_menu); the returned label is plain text and the caller
    // pairs it with the icon from `StatusState::icon()`.
    match crate::commands::daemon::daemon_status_blocking() {
        crate::commands::daemon::DaemonStatusResponse::Running { uptime_secs, .. } => (
            format!("Server Status: Running  (up {})", format_uptime(uptime_secs)),
            StatusState::Running,
        ),
        crate::commands::daemon::DaemonStatusResponse::Unreachable { .. } => {
            ("Server Status: Unreachable".to_string(), StatusState::Unreachable)
        }
        _ => ("Server Status: Not running".to_string(), StatusState::NotRunning),
    }
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
        ids::SHOW => show_main_window(app),
        ids::QUIT => {
            // Honors the "keep daemon running when K2SO quits" setting
            // via the RunEvent::ExitRequested handler in lib.rs. No
            // tray-local override — one quit path, one policy.
            // A board file's unsaved edit asks first, in the page (never set while the feature is dark).
            if crate::commands::settings::BOARD_FILE_UNSAVED.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = app.emit("board-file:quit-requested", ());
            } else {
                app.exit(0);
            }
        }
        _ => {}
    }
}
