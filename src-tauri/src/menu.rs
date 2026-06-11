use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Manager};

pub fn create_menu(handle: &AppHandle) -> Result<Menu<tauri::Wry>, tauri::Error> {
    let menu = Menu::new(handle)?;

    // App submenu (macOS)
    let app_menu = Submenu::with_items(
        handle,
        "K2SO",
        true,
        &[
            &PredefinedMenuItem::about(handle, Some("About K2SO"), None)?,
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
            &MenuItem::with_id(handle, "new-document", "New Document", true, Some("CmdOrCtrl+N"))?,
            &MenuItem::with_id(handle, "new-tab", "New Tab", true, Some("CmdOrCtrl+T"))?,
            &MenuItem::with_id(handle, "launch-agent", "Launch Default Agent", true, Some("CmdOrCtrl+Shift+T"))?,
            &PredefinedMenuItem::separator(handle)?,
            &MenuItem::with_id(handle, "split-pane", "Split Pane", true, Some("CmdOrCtrl+D"))?,
            &PredefinedMenuItem::separator(handle)?,
            &MenuItem::with_id(handle, "open-workspace", "Open Workspace...", true, Some("CmdOrCtrl+O"))?,
            &PredefinedMenuItem::separator(handle)?,
            &MenuItem::with_id(handle, "close-tab", "Close Tab", true, Some("CmdOrCtrl+W"))?,
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

    // View submenu
    let view_menu = Submenu::with_items(
        handle,
        "View",
        true,
        &[
            &MenuItem::with_id(handle, "command-palette", "Command Palette", true, Some("CmdOrCtrl+K"))?,
            &MenuItem::with_id(handle, "review-queue", "Review Queue", true, Some("CmdOrCtrl+P"))?,
            &MenuItem::with_id(handle, "toggle-sidebar", "Toggle Sidebar", true, Some("CmdOrCtrl+B"))?,
            &MenuItem::with_id(handle, "toggle-assistant", "Toggle Assistant", true, Some("CmdOrCtrl+L"))?,
            &MenuItem::with_id(handle, "focus-window", "Open in Focus Window", true, Some("CmdOrCtrl+Shift+F"))?,
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
            &MenuItem::with_id(handle, "new-window", "New Window", true, Some("CmdOrCtrl+Shift+N"))?,
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
                    "document.documentElement.style.zoom='{}';document.title='{}'",
                    scale,
                    if next == 100 { "K2SO".to_string() } else { format!("K2SO — {}%", next) }
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
        "review-queue" => {
            emit_to_focused(app, "menu:review-queue");
        }
        "toggle-sidebar" => {
            emit_to_focused(app, "menu:toggle-sidebar");
        }
        "toggle-assistant" => {
            emit_to_focused(app, "menu:toggle-assistant");
        }
        "focus-window" => {
            emit_to_focused(app, "menu:focus-window");
        }
        "new-window" => {
            use tauri::WebviewWindowBuilder;

            // Generate a unique label for the new window
            let label = format!("window-{}", uuid::Uuid::new_v4());
            let webview_url = if cfg!(debug_assertions) {
                tauri::WebviewUrl::External(
                    url::Url::parse("http://localhost:5173").unwrap(),
                )
            } else {
                tauri::WebviewUrl::App("index.html".into())
            };

            let _ = WebviewWindowBuilder::new(app, &label, webview_url)
                .title("K2SO")
                .inner_size(1400.0, 900.0)
                .min_inner_size(800.0, 600.0)
                .hidden_title(true)
                .title_bar_style(tauri::TitleBarStyle::Overlay)
                .build();
        }
        _ => {}
    }
}

fn emit_to_focused(app: &AppHandle, event: &str) {
    // Emit to all windows — the focused one will handle it
    for (_, win) in app.webview_windows() {
        let _ = win.emit(event, ());
    }
}
