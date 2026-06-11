//! Window state read/write helpers.
//!
//! Phase 2 Unit 4 — the previous in-process SQLite reads/writes
//! moved to the daemon's `/cli/window-state/{get,set}` routes.
//! These helpers now proxy via `DaemonClient`.
//!
//! `migrate_json_window_state` was confirmed dead code on 2026-05-23
//! and removed entirely.

use serde::{Deserialize, Serialize};
use tauri::Manager;

use crate::daemon_client::DaemonClient;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowState {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub is_maximized: bool,
}

pub fn save_window_state(app: &tauri::AppHandle) {
    let Some(win) = app.get_webview_window("main") else {
        return;
    };

    let is_maximized = win.is_maximized().unwrap_or(false);
    let client = match DaemonClient::try_connect() {
        Ok(c) => c,
        Err(_) => return,
    };

    if is_maximized {
        // Touch only the is_maximized flag; preserve the last
        // windowed geometry. Daemon honors `onlyMaximizedFlag` by
        // ignoring the x/y/w/h fields when set.
        let _ = client.cli_post_json(
            "/cli/window-state/set",
            &serde_json::json!({
                "x": 0,
                "y": 0,
                "width": 0,
                "height": 0,
                "isMaximized": true,
                "onlyMaximizedFlag": true,
            }),
        );
        return;
    }

    let position = match win.outer_position() {
        Ok(p) => p,
        Err(_) => return,
    };
    let size = match win.outer_size() {
        Ok(s) => s,
        Err(_) => return,
    };

    let _ = client.cli_post_json(
        "/cli/window-state/set",
        &serde_json::json!({
            "x": position.x,
            "y": position.y,
            "width": size.width,
            "height": size.height,
            "isMaximized": false,
            "onlyMaximizedFlag": false,
        }),
    );
}

pub fn load_window_state(_app: &tauri::AppHandle) -> Option<WindowState> {
    let client = DaemonClient::try_connect().ok()?;
    client.cli_get_json("/cli/window-state/get", &[]).ok().flatten()
}
