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

    // Quit asks about a board file's unsaved edit only with `board_file_preview` on; otherwise the predefined item,
    // exactly as ever (the feature is dark). A Dock or logout quit still goes straight out: the tab says so on reopen.
    let quit_custom = MenuItem::with_id(handle, "board-file-quit", "Quit K2", true, Some("CmdOrCtrl+Q"))?;
    let quit_predefined = PredefinedMenuItem::quit(handle, None)?;
    let quit_item: &dyn tauri::menu::IsMenuItem<tauri::Wry> =
        if k2_client::app_settings_view::load().board_file_preview { &quit_custom } else { &quit_predefined };

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
            // Send feedback to K2 (prd-k2-product-feedback-v1 PF4): Zen has no
            // top bar, so the dialog is reachable from here too.
            &MenuItem::with_id(handle, "send-feedback", "Send Feedback...", true, None::<&str>)?,
            &PredefinedMenuItem::separator(handle)?,
            &PredefinedMenuItem::services(handle, None)?,
            &PredefinedMenuItem::separator(handle)?,
            &PredefinedMenuItem::hide(handle, None)?,
            &PredefinedMenuItem::hide_others(handle, None)?,
            &PredefinedMenuItem::show_all(handle, None)?,
            &PredefinedMenuItem::separator(handle)?,
            quit_item,
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
        // Only with `board_file_preview` on (see `app_menu`): Quit has to be able to ask about a board file's edit
        // that isn't saved, which the predefined item (NSApplication terminate:) can't.
        "board-file-quit" => {
            if crate::commands::settings::BOARD_FILE_UNSAVED.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = app.emit("board-file:quit-requested", ());
            } else {
                app.exit(0);
            }
        }
        // Every command that acts on a window goes to the window in front
        // ONLY (`emit_to_focused_window_only`): ⌘, opened Settings in every
        // open K2 window while it was `app.emit`. Check for Updates also
        // opens Settings, so it goes to the window in front too.
        // ⌘, : the accelerator consumes the keystroke, so this event is the
        // only way Settings opens from the keyboard. The window is found by
        // `focused_window_label`, which counts a window holding a Browser or
        // Design child webview.
        "settings" => {
            emit_to_focused_window_only(app, "menu:open-settings");
        }
        "check-for-updates" => {
            emit_to_focused_window_only(app, "menu:check-for-updates");
        }
        "send-feedback" => {
            emit_to_focused_window_only(app, "menu:send-feedback");
        }
        // Page zoom is per window (`window.__k2Zoom`, the same CSS path as
        // the webview's Cmd+= / Cmd+-). The window's own listener steps it.
        "app-zoom-in" => {
            emit_to_focused_window_only(app, "app:zoom-in");
        }
        "app-zoom-out" => {
            emit_to_focused_window_only(app, "app:zoom-out");
        }
        "app-zoom-reset" => {
            emit_to_focused_window_only(app, "app:zoom-reset");
        }
        "terminal-zoom-in" => {
            emit_to_focused_window_only(app, "terminal:zoom-in");
        }
        "terminal-zoom-out" => {
            emit_to_focused_window_only(app, "terminal:zoom-out");
        }
        "new-document" => {
            emit_to_focused_window_only(app, "menu:new-document");
        }
        "new-tab" => {
            emit_to_focused_window_only(app, "menu:new-tab");
        }
        "launch-agent" => {
            emit_to_focused_window_only(app, "menu:launch-agent");
        }
        "split-pane" => {
            emit_to_focused_window_only(app, "menu:split-pane");
        }
        "open-workspace" => {
            emit_to_focused_window_only(app, "menu:open-workspace");
        }
        "close-tab" => {
            emit_to_focused_window_only(app, "menu:close-tab");
        }
        "command-palette" => {
            emit_to_focused_window_only(app, "menu:command-palette");
        }
        "running-agents" => {
            emit_to_focused_window_only(app, "menu:running-agents");
        }
        "projects" => {
            emit_to_focused_window_only(app, "menu:projects");
        }
        "toggle-sidebar" => {
            emit_to_focused_window_only(app, "menu:toggle-sidebar");
        }
        "server-switcher" => {
            emit_to_focused_window_only(app, "menu:server-switcher");
        }
        "focus-window" => {
            emit_to_focused_window_only(app, "menu:focus-window");
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

/// Windows: the system menu (Restore, Move, Size, Minimize, Maximize,
/// Close) at the pointer, for a right-click on K2's own title bar (the
/// window has no native frame, so Windows doesn't show it by itself). The
/// chosen item runs as the same WM_SYSCOMMAND a framed window sends. A
/// sync command: it runs on the main thread, which owns the window.
/// Elsewhere it does nothing.
#[tauri::command]
pub fn window_system_menu(window: tauri::WebviewWindow) -> Result<(), String> {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{
            EnableMenuItem, GetCursorPos, GetSystemMenu, PostMessageW, TrackPopupMenu, MF_BYCOMMAND, MF_ENABLED,
            MF_GRAYED, SC_CLOSE, SC_MAXIMIZE, SC_MINIMIZE, SC_MOVE, SC_RESTORE, SC_SIZE, TPM_RETURNCMD,
            TPM_RIGHTBUTTON, WM_SYSCOMMAND,
        };
        let hwnd = HWND(window.hwnd().map_err(|e| e.to_string())?.0);
        let maximized = window.is_maximized().unwrap_or(false);
        // SAFETY: Win32 calls on this window's own handle, on its thread;
        // the system menu belongs to the window and is not destroyed here.
        unsafe {
            let menu = GetSystemMenu(hwnd, false);
            if menu.is_invalid() {
                return Ok(());
            }
            for (id, on) in [
                (SC_RESTORE, maximized),
                (SC_MOVE, !maximized),
                (SC_SIZE, !maximized),
                (SC_MAXIMIZE, !maximized),
                (SC_MINIMIZE, true),
                (SC_CLOSE, true),
            ] {
                let _ = EnableMenuItem(menu, id, MF_BYCOMMAND | if on { MF_ENABLED } else { MF_GRAYED });
            }
            let mut pt = POINT::default();
            GetCursorPos(&mut pt).map_err(|e| e.to_string())?;
            let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, None, hwnd, None);
            if cmd.0 != 0 {
                PostMessageW(Some(hwnd), WM_SYSCOMMAND, WPARAM(cmd.0 as usize), LPARAM(0)).map_err(|e| e.to_string())?;
            }
        }
    }
    #[cfg(not(windows))]
    let _ = window;
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
    let (width, height) = crate::window::new_window_logical_size(app);
    let builder = crate::k2_app_window::k2_app_window_builder(app, &label, webview_url)
        .title("K2 | local")
        .inner_size(width, height)
        .min_inner_size(
            crate::window::MIN_WIDTH as f64,
            crate::window::MIN_HEIGHT as f64,
        )
        .center();
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

/// The focused window's label, counting a window that hosts a Browser or
/// Design child webview. With `browser-pane` (the default build), never
/// `get_webview_window(..)` or `webview_windows()` in the menu paths: tauri counts a window as a
/// "webview window" only while it holds one webview, so once a Browser or
/// Design tab adds its child webview to the window both skip it and every
/// menu event (⌘, Settings among them) was silently dropped.
#[cfg(target_os = "macos")]
fn focused_window_label(app: &AppHandle) -> Option<String> {
    #[cfg(feature = "browser-pane")]
    let windows = app.windows();
    #[cfg(not(feature = "browser-pane"))]
    let windows = app.webview_windows();
    let windows: Vec<(String, bool)> = windows
        .into_iter()
        .map(|(label, w)| {
            let focused = w.is_focused().unwrap_or(false);
            (label, focused)
        })
        .collect();
    menu_target_label(&windows)
}

/// Which window a window-local menu command goes to: the focused one; with
/// none focused (the menu bar used with no key window), "main" if it is
/// open, else any open window, so the command still lands somewhere.
#[cfg(any(target_os = "macos", test))]
fn menu_target_label(windows: &[(String, bool)]) -> Option<String> {
    windows
        .iter()
        .find(|(_, focused)| *focused)
        .or_else(|| windows.iter().find(|(label, _)| label == "main"))
        .or_else(|| windows.first())
        .map(|(label, _)| label.clone())
}

/// Send a menu command to the window in front ONLY. `Emitter::emit`
/// reaches every window's untargeted listener, so ⌘, opened Settings (and
/// New Tab opened a tab, ⌘B toggled the sidebar …) in every open window.
/// One emit, to that window's label; the renderer listens with target
/// `AnyLabel { label: <its own label> }` (`listenOwnWindow`), since an
/// untargeted JS listener hears every emit whatever its target.
#[cfg(target_os = "macos")]
fn emit_to_focused_window_only(app: &AppHandle, event: &str) {
    let Some(label) = focused_window_label(app) else {
        return;
    };
    let _ = app.emit_to(EventTarget::AnyLabel { label }, event, ());
}

#[cfg(test)]
mod zen_menu_tests {
    use super::{menu_target_label, zen_menu_label};

    #[test]
    fn zen_menu_label_flips() {
        assert_eq!(zen_menu_label(false), "Enter Zen Mode");
        assert_eq!(zen_menu_label(true), "Exit Zen Mode");
    }

    fn windows(list: &[(&str, bool)]) -> Vec<(String, bool)> {
        list.iter().map(|(l, f)| (l.to_string(), *f)).collect()
    }

    #[test]
    fn menu_commands_go_to_the_focused_window_only() {
        let w = windows(&[("main", false), ("window-a", true), ("window-b", false)]);
        assert_eq!(menu_target_label(&w).as_deref(), Some("window-a"));
    }

    #[test]
    fn with_no_window_focused_main_then_any_window() {
        let w = windows(&[("window-a", false), ("main", false)]);
        assert_eq!(menu_target_label(&w).as_deref(), Some("main"));
        let w = windows(&[("window-a", false), ("window-b", false)]);
        assert_eq!(menu_target_label(&w).as_deref(), Some("window-a"));
        assert_eq!(menu_target_label(&[]), None);
    }
}
