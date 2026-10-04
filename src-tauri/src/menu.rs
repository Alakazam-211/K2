// Native app menu bar is macOS-only. Win/Linux open the same actions from
// the K2 logo. `window_new` / `open_new_window` below.
// Without these cfg gates, Linux CI (`-D warnings`) fails on dead_code.
#[cfg(target_os = "macos")]
use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
#[cfg(target_os = "macos")]
use tauri::{AppHandle, Emitter, EventTarget, Manager};
#[cfg(not(target_os = "macos"))]
use tauri::{AppHandle, Manager};

/// prd-zen-mode-v1 Z30/Z53 — the View menu's "Enter / Exit Zen Mode" item,
/// kept so `set_zen_menu_label` can flip its text for the focused window.
#[cfg(target_os = "macos")]
pub struct ZenMenuItem(pub MenuItem<tauri::Wry>);

/// The View-menu "Zen Shortcuts" item (Omarchy addition 4): opens the Zen
/// cheat sheet in the focused window. Enabled only while that window shows
/// Zen (flipped with the Zen label). No accelerator: the webview's `?` and
/// Ctrl+Cmd+/ keydown are its only keyboard owners (the Cmd+L lesson).
#[cfg(target_os = "macos")]
pub struct ZenShortcutsMenuItem(pub MenuItem<tauri::Wry>);

/// Menu text for the Zen item.
#[cfg(any(target_os = "macos", test))]
pub fn zen_menu_label(in_zen: bool) -> &'static str {
    if in_zen {
        "Exit Zen Mode"
    } else {
        "Enter Zen Mode"
    }
}

#[cfg(target_os = "macos")]
pub fn create_menu(handle: &AppHandle) -> Result<Menu<tauri::Wry>, tauri::Error> {
    let menu = Menu::new(handle)?;

    // App submenu (macOS)
    let app_menu = Submenu::with_items(
        handle,
        "K2",
        true,
        &[
            &PredefinedMenuItem::about(handle, Some("About K2"), None)?,
            &PredefinedMenuItem::separator(handle)?,
            &MenuItem::with_id(handle, "settings", "Settings...", true, Some("CmdOrCtrl+,"))?,
            &MenuItem::with_id(handle, "check-for-updates", "Check for Updates...", true, None::<&str>)?,
            &PredefinedMenuItem::separator(handle)?,
            &PredefinedMenuItem::services(handle, None)?,
            &PredefinedMenuItem::separator(handle)?,
            &PredefinedMenuItem::hide(handle, None)?,
            &PredefinedMenuItem::hide_others(handle, None)?,
            &PredefinedMenuItem::show_all(handle, None)?,
            &PredefinedMenuItem::separator(handle)?,
            &PredefinedMenuItem::quit(handle, None)?,
        ],
    )?;

    // File submenu
    let file_menu = Submenu::with_items(
        handle,
        "File",
        true,
        &[
            // NO accelerators on any File action that useTerminalShortcuts.ts
            // also binds (Cmd+N / T / Shift+T / D / O / W). Binding here AND
            // in the webview double-fires (menu event + keydown) — the
            // Cmd+Shift+T multi-spawn bug (2026-07-07) and the same-class
            // Cmd+N multi-note bug (2026-07-09). Same duplicate-binding trap
            // tray.rs documents for Cmd+Q. Menu items stay clickable; the
            // webview keydown is the single keyboard owner.
            &MenuItem::with_id(handle, "new-document", "New Document", true, None::<&str>)?,
            &MenuItem::with_id(handle, "new-tab", "New Tab", true, None::<&str>)?,
            &MenuItem::with_id(handle, "launch-agent", "Launch Default Agent", true, None::<&str>)?,
            &PredefinedMenuItem::separator(handle)?,
            &MenuItem::with_id(handle, "split-pane", "Split Pane", true, None::<&str>)?,
            &PredefinedMenuItem::separator(handle)?,
            &MenuItem::with_id(handle, "open-workspace", "Open Workspace...", true, None::<&str>)?,
            &PredefinedMenuItem::separator(handle)?,
            &MenuItem::with_id(handle, "close-tab", "Close Tab", true, None::<&str>)?,
        ],
    )?;

    // Edit submenu
    //
    // 0.37.9 — submenu title MUST be literally "Edit" so macOS
    // auto-injects the native `Start Dictation…` and `Emoji &
    // Symbols` items at runtime. AppKit keys this auto-injection
    // on the localized title string. We deliberately DO NOT add a
    // custom `MenuItem::with_id("start-dictation", ...)` here —
    // doing so suppresses the OS auto-inject and Fn-Fn falls back
    // to firing `startDictation:` against a responder that has no
    // such selector → silent failure. (The original 0.37.9 attempt
    // shipped that custom item; agent research traced the
    // suppression bug to it. See:
    // https://github.com/tauri-apps/muda/issues/83
    // https://github.com/electron/electron/issues/8283 )
    let edit_menu = Submenu::with_items(
        handle,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(handle, None)?,
            &PredefinedMenuItem::redo(handle, None)?,
            &PredefinedMenuItem::separator(handle)?,
            &PredefinedMenuItem::cut(handle, None)?,
            &PredefinedMenuItem::copy(handle, None)?,
            &PredefinedMenuItem::paste(handle, None)?,
            &PredefinedMenuItem::select_all(handle, None)?,
        ],
    )?;

    // prd-zen-mode-v1 Z30/Z53: the escape hatch. The native accelerator
    // (Ctrl+Cmd+Z) is the ONLY keyboard owner of the chord on macOS: there is
    // no webview keydown for it (two owners double-toggle, the Cmd+L lesson).
    let zen_item = MenuItem::with_id(handle, "zen-toggle", zen_menu_label(false), true, Some("Ctrl+Cmd+Z"))?;
    handle.manage(ZenMenuItem(zen_item.clone()));
    let zen_shortcuts_item = MenuItem::with_id(handle, "zen-shortcuts", "Zen Shortcuts", false, None::<&str>)?;
    handle.manage(ZenShortcutsMenuItem(zen_shortcuts_item.clone()));

    // View submenu
    let view_menu = Submenu::with_items(
        handle,
        "View",
        true,
        &[
            &MenuItem::with_id(handle, "command-palette", "Command Palette", true, Some("CmdOrCtrl+K"))?,
            // 0.40.31 — "Review Queue" (CmdOrCtrl+P) re-pointed to the ⌘J
            // Running Agents session switcher; the Review Queue UI is gone.
            &MenuItem::with_id(handle, "running-agents", "Running Agents", true, Some("CmdOrCtrl+J"))?,
            &MenuItem::with_id(handle, "projects", "Projects", true, Some("CmdOrCtrl+P"))?,
            &MenuItem::with_id(handle, "toggle-sidebar", "Toggle Sidebar", true, Some("CmdOrCtrl+B"))?,
            // No Cmd+L accelerator. App.tsx keydown is the only keyboard
            // owner on every platform (Win/Linux menus have none). A menu
            // accelerator and that keydown both toggled and the second
            // call closed the switcher. The item stays clickable.
            &MenuItem::with_id(handle, "server-switcher", "Switch Server", true, None::<&str>)?,
            &MenuItem::with_id(handle, "toggle-assistant", "Toggle Assistant", true, Some("CmdOrCtrl+Shift+L"))?,
            &MenuItem::with_id(handle, "focus-window", "Open in Focus Window", true, Some("CmdOrCtrl+Shift+F"))?,
            &zen_item,
            &zen_shortcuts_item,
            &PredefinedMenuItem::separator(handle)?,
            &MenuItem::with_id(handle, "app-zoom-in", "Zoom In", true, None::<&str>)?,
            &MenuItem::with_id(handle, "app-zoom-out", "Zoom Out", true, None::<&str>)?,
            &MenuItem::with_id(handle, "app-zoom-reset", "Zoom Reset", true, None::<&str>)?,
            &PredefinedMenuItem::separator(handle)?,
            &MenuItem::with_id(handle, "terminal-zoom-in", "Terminal Zoom In", true, Some("CmdOrCtrl+Shift+Equal"))?,
            &MenuItem::with_id(handle, "terminal-zoom-out", "Terminal Zoom Out", true, Some("CmdOrCtrl+Shift+-"))?,
            &PredefinedMenuItem::separator(handle)?,
            &PredefinedMenuItem::fullscreen(handle, None)?,
        ],
    )?;

    // Window submenu
    let window_menu = Submenu::with_items(
        handle,
        "Window",
        true,
        &[
            // No accelerator. useNewWindowShortcut is the only keyboard
            // owner (Ctrl+Shift+N, and Cmd+Shift+N on macOS). Binding it
            // here too opened a second window. The item stays clickable.
            &MenuItem::with_id(handle, "new-window", "New Window", true, None::<&str>)?,
            &PredefinedMenuItem::separator(handle)?,
            &PredefinedMenuItem::minimize(handle, None)?,
            &PredefinedMenuItem::maximize(handle, None)?,
            &PredefinedMenuItem::separator(handle)?,
            &PredefinedMenuItem::close_window(handle, None)?,
        ],
    )?;

    menu.append(&app_menu)?;
    menu.append(&file_menu)?;
    menu.append(&edit_menu)?;
    menu.append(&view_menu)?;
    menu.append(&window_menu)?;

    Ok(menu)
}

#[cfg(target_os = "macos")]
pub fn handle_menu_event(app: &AppHandle, event: MenuEvent) {
    let id = event.id().as_ref();
    match id {
        "settings" => {
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.emit("menu:open-settings", ());
            }
        }
        "check-for-updates" => {
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.emit("menu:check-for-updates", ());
            }
        }
        "app-zoom-in" | "app-zoom-out" | "app-zoom-reset" => {
            // Zoom via menu items (keyboard zoom handled in App.tsx)
            use std::sync::atomic::{AtomicU32, Ordering};
            static ZOOM_LEVEL: AtomicU32 = AtomicU32::new(100); // percentage

            let current = ZOOM_LEVEL.load(Ordering::Relaxed);
            let next = match id {
                "app-zoom-in" => (current + 10).min(200),
                "app-zoom-out" => current.saturating_sub(10).max(50),
                _ => 100, // reset
            };
            ZOOM_LEVEL.store(next, Ordering::Relaxed);

            if let Some(win) = app.get_webview_window("main") {
                let scale = next as f64 / 100.0;
                let js = format!(
                    "window.__k2soZoom={scale};if(typeof window.__k2ApplyPageTitle==='function')window.__k2ApplyPageTitle();"
                );
                let _ = win.eval(&js);
            }
        }
        "terminal-zoom-in" => {
            emit_to_focused(app, "terminal:zoom-in");
        }
        "terminal-zoom-out" => {
            emit_to_focused(app, "terminal:zoom-out");
        }
        "new-document" => {
            emit_to_focused(app, "menu:new-document");
        }
        "new-tab" => {
            emit_to_focused(app, "menu:new-tab");
        }
        "launch-agent" => {
            emit_to_focused(app, "menu:launch-agent");
        }
        "split-pane" => {
            emit_to_focused(app, "menu:split-pane");
        }
        "open-workspace" => {
            emit_to_focused(app, "menu:open-workspace");
        }
        "close-tab" => {
            emit_to_focused(app, "menu:close-tab");
        }
        "command-palette" => {
            emit_to_focused(app, "menu:command-palette");
        }
        "running-agents" => {
            emit_to_focused(app, "menu:running-agents");
        }
        "projects" => {
            emit_to_focused(app, "menu:projects");
        }
        "toggle-sidebar" => {
            emit_to_focused(app, "menu:toggle-sidebar");
        }
        "server-switcher" => {
            emit_to_focused(app, "menu:server-switcher");
        }
        "toggle-assistant" => {
            emit_to_focused(app, "menu:toggle-assistant");
        }
        "focus-window" => {
            emit_to_focused(app, "menu:focus-window");
        }
        "zen-toggle" => {
            emit_to_focused_window_only(app, "menu:zen-toggle");
        }
        "zen-shortcuts" => {
            emit_to_focused_window_only(app, "menu:zen-shortcuts");
        }
        "new-window" => {
            let _ = open_new_window(app);
        }
        _ => {}
    }
}

/// prd-zen-mode-v1 Z53 — flip the View menu's Zen item between "Enter Zen
/// Mode" and "Exit Zen Mode" for the focused window. The menu bar is
/// app-wide on macOS, so the focused window calls this when it gains focus
/// and when its Zen state changes. A no-op on Linux and Windows (their menu
/// is the in-app panel, which reads the state itself).
#[tauri::command]
pub fn set_zen_menu_label(app: AppHandle, in_zen: bool) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        if let Some(item) = app.try_state::<ZenMenuItem>() {
            item.0.set_text(zen_menu_label(in_zen)).map_err(|e| e.to_string())?;
        }
        if let Some(item) = app.try_state::<ZenShortcutsMenuItem>() {
            item.0.set_enabled(in_zen).map_err(|e| e.to_string())?;
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (&app, in_zen);
    }
    Ok(())
}

/// Open a secondary main-layout window (shared by menu + Win/Linux App Menu).
#[tauri::command]
pub fn window_new(app: AppHandle) -> Result<(), String> {
    open_new_window(&app).map_err(|e| e.to_string())
}

/// Tickets "Open in window": one window per ticket (label `window-ticket-<id>`,
/// fragment `#ticket=<id>`), focused if it is already open. The renderer
/// shows only that ticket's detail there. OS integration only: the ticket
/// itself is read from the daemon by the renderer.
#[tauri::command]
pub fn window_open_ticket(app: AppHandle, ticket_id: String) -> Result<(), String> {
    let id = ticket_id.trim();
    if id.is_empty()
        || id.len() > 64
        || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err(format!("invalid ticket id '{ticket_id}'"));
    }
    // `window-*` so the default capability (main, window-*, focus-*) covers it.
    let label = format!("window-ticket-{id}");
    if let Some(existing) = app.get_webview_window(&label) {
        existing.set_focus().map_err(|e| e.to_string())?;
        return Ok(());
    }
    let fragment = format!("ticket={id}");
    let webview_url = crate::k2_app_window::k2_app_webview_url(&app, Some(&fragment));
    let builder = crate::k2_app_window::k2_app_window_builder(&app, &label, webview_url)
        .title("K2 | Ticket")
        .inner_size(1100.0, 860.0)
        .min_inner_size(600.0, 400.0);
    #[cfg(target_os = "macos")]
    let builder = builder
        .hidden_title(true)
        .title_bar_style(tauri::TitleBarStyle::Overlay);
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    let builder = builder.decorations(false);
    builder.build().map_err(|e| e.to_string())?;
    Ok(())
}

pub fn open_new_window(app: &AppHandle) -> Result<(), tauri::Error> {
    let label = format!("window-{}", uuid::Uuid::new_v4());
    let webview_url = crate::k2_app_window::k2_app_webview_url(app, None);
    let builder = crate::k2_app_window::k2_app_window_builder(app, &label, webview_url)
        .title("K2 | local")
        .inner_size(1400.0, 900.0)
        .min_inner_size(800.0, 600.0);
    // hidden_title / TitleBarStyle::Overlay are macOS-only builder methods.
    #[cfg(target_os = "macos")]
    let builder = builder
        .hidden_title(true)
        .title_bar_style(tauri::TitleBarStyle::Overlay);
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    let builder = builder.decorations(false);
    builder.build()?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn emit_to_focused(app: &AppHandle, event: &str) {
    // Emit to the FOCUSED window only. The old "emit to all windows"
    // loop meant every extra webview (Focus windows, project windows)
    // ALSO ran the action — a second amplifier in the Cmd+Shift+T
    // multi-spawn bug (one menu event = one spawn PER WINDOW). A menu
    // action is user-initiated, so a focused window exists in practice;
    // if none reports focused (edge: menu clicked during a focus
    // transition), fall back to "main" rather than all.
    let focused = app
        .webview_windows()
        .into_iter()
        .find(|(_, w)| w.is_focused().unwrap_or(false));
    if let Some((_, win)) = focused {
        let _ = win.emit(event, ());
    } else if let Some(win) = app.webview_windows().get("main") {
        let _ = win.emit(event, ());
    }
}

/// Like `emit_to_focused`, but delivered ONLY to that window's listeners.
/// `Emitter::emit` reaches every window's untargeted listener, so the Zen
/// toggle (per Home, shared across windows) would flip once per window.
/// The renderer listens with target `AnyLabel { label: <its own label> }`.
#[cfg(target_os = "macos")]
fn emit_to_focused_window_only(app: &AppHandle, event: &str) {
    let label = app
        .webview_windows()
        .into_iter()
        .find(|(_, w)| w.is_focused().unwrap_or(false))
        .map(|(label, _)| label)
        .unwrap_or_else(|| "main".to_string());
    let _ = app.emit_to(EventTarget::AnyLabel { label }, event, ());
}

#[cfg(test)]
mod zen_menu_tests {
    use super::zen_menu_label;

    #[test]
    fn zen_menu_label_flips() {
        assert_eq!(zen_menu_label(false), "Enter Zen Mode");
        assert_eq!(zen_menu_label(true), "Exit Zen Mode");
    }
}
