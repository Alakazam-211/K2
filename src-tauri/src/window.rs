//! Window state read/write helpers.
//!
//! Phase 2 Unit 4 — the previous in-process SQLite reads/writes
//! moved to the daemon's `/cli/window-state/{get,set}` routes.
//! These helpers now proxy via `DaemonClient`.
//!
//! Restore validates the saved frame against the current monitors.
//! A tiny or off-screen rect (unplugged display, first spawn in a
//! corner, Cmd+Q that never saved) is discarded and the window is
//! centered at a usable default size.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{Manager, PhysicalPosition, PhysicalSize};

use crate::daemon_client::DaemonClient;

pub const MIN_WIDTH: u32 = 800;
pub const MIN_HEIGHT: u32 = 600;
pub const DEFAULT_WIDTH: u32 = 1400;
pub const DEFAULT_HEIGHT: u32 = 900;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowState {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub is_maximized: bool,
}

/// Physical display rectangle (same space as `outer_position` / `outer_size`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowFrame {
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

    let monitors = monitor_rects(app);
    if !monitors.is_empty()
        && !frame_is_usable(position.x, position.y, size.width, size.height, &monitors)
    {
        // Don't persist a tiny / off-screen frame over a previously good one.
        return;
    }

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

/// Place the main window: last good frame if it still fits a display,
/// otherwise a centered 1400×900 (clamped to the work area).
pub fn apply_restored_frame(app: &tauri::AppHandle) {
    let Some(win) = app.get_webview_window("main") else {
        return;
    };
    let monitors = monitor_rects(app);
    let saved = load_window_state(app);
    let frame = resolve_window_frame(saved.as_ref(), &monitors);
    let _ = win.set_position(PhysicalPosition::new(frame.x, frame.y));
    let _ = win.set_size(PhysicalSize::new(frame.width, frame.height));
    if frame.is_maximized {
        let _ = win.maximize();
    }
}

/// Debounced save so Moved/Resized don't hammer the daemon.
pub fn schedule_save_window_state(app: &tauri::AppHandle) {
    static PENDING: AtomicBool = AtomicBool::new(false);
    if PENDING.swap(true, Ordering::Relaxed) {
        return;
    }
    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(400));
        PENDING.store(false, Ordering::Relaxed);
        save_window_state(&handle);
    });
}

fn monitor_rects(app: &tauri::AppHandle) -> Vec<MonitorRect> {
    let mut out = Vec::new();
    if let Ok(Some(primary)) = app.primary_monitor() {
        out.push(monitor_to_rect(&primary));
    }
    if let Ok(all) = app.available_monitors() {
        for mon in all {
            let r = monitor_to_rect(&mon);
            if !out.iter().any(|e| e == &r) {
                out.push(r);
            }
        }
    }
    out
}

fn monitor_to_rect(mon: &tauri::Monitor) -> MonitorRect {
    let pos = mon.position();
    let size = mon.size();
    MonitorRect {
        x: pos.x,
        y: pos.y,
        width: size.width,
        height: size.height,
    }
}

fn rects_intersect(
    ax: i32,
    ay: i32,
    aw: u32,
    ah: u32,
    bx: i32,
    by: i32,
    bw: u32,
    bh: u32,
) -> bool {
    let aw = aw as i64;
    let ah = ah as i64;
    let bw = bw as i64;
    let bh = bh as i64;
    let ax = ax as i64;
    let ay = ay as i64;
    let bx = bx as i64;
    let by = by as i64;
    ax < bx + bw && ax + aw > bx && ay < by + bh && ay + ah > by
}

/// True when the window is large enough and its title bar can be grabbed
/// on at least one current display.
pub fn frame_is_usable(x: i32, y: i32, width: u32, height: u32, monitors: &[MonitorRect]) -> bool {
    if width < MIN_WIDTH || height < MIN_HEIGHT {
        return false;
    }
    if monitors.is_empty() {
        return true;
    }
    let bar_w = width.min(160);
    let bar_h = height.min(40);
    monitors.iter().any(|m| {
        rects_intersect(x, y, bar_w, bar_h, m.x, m.y, m.width, m.height)
    })
}

fn center_frame(width: u32, height: u32, monitors: &[MonitorRect]) -> WindowFrame {
    let Some(m) = monitors.first() else {
        return WindowFrame {
            x: 80,
            y: 60,
            width,
            height,
            is_maximized: false,
        };
    };
    let max_w = m.width.saturating_sub(40).max(MIN_WIDTH);
    let max_h = m.height.saturating_sub(40).max(MIN_HEIGHT);
    let width = width.clamp(MIN_WIDTH, max_w);
    let height = height.clamp(MIN_HEIGHT, max_h);
    WindowFrame {
        x: m.x + (m.width as i32 - width as i32) / 2,
        y: m.y + (m.height as i32 - height as i32) / 2,
        width,
        height,
        is_maximized: false,
    }
}

/// Pick a frame that is either the saved one (if still on-screen and
/// large enough) or a centered default.
pub fn resolve_window_frame(saved: Option<&WindowState>, monitors: &[MonitorRect]) -> WindowFrame {
    let default = center_frame(DEFAULT_WIDTH, DEFAULT_HEIGHT, monitors);
    let Some(saved) = saved else {
        return default;
    };

    let size_ok = saved.width >= MIN_WIDTH && saved.height >= MIN_HEIGHT;
    let pos_ok = size_ok
        && frame_is_usable(saved.x, saved.y, saved.width, saved.height, monitors);

    if size_ok && pos_ok {
        return WindowFrame {
            x: saved.x,
            y: saved.y,
            width: saved.width,
            height: saved.height,
            is_maximized: saved.is_maximized,
        };
    }

    if size_ok {
        let mut centered = center_frame(saved.width, saved.height, monitors);
        centered.is_maximized = saved.is_maximized;
        return centered;
    }

    let mut fallback = default;
    fallback.is_maximized = saved.is_maximized;
    fallback
}

#[cfg(test)]
mod tests {
    use super::*;

    fn laptop() -> MonitorRect {
        MonitorRect {
            x: 0,
            y: 0,
            width: 1512,
            height: 982,
        }
    }

    fn external() -> MonitorRect {
        MonitorRect {
            x: 1512,
            y: 0,
            width: 2560,
            height: 1440,
        }
    }

    fn saved(x: i32, y: i32, width: u32, height: u32) -> WindowState {
        WindowState {
            x,
            y,
            width,
            height,
            is_maximized: false,
        }
    }

    #[test]
    fn no_saved_state_centers_default_on_primary() {
        let f = resolve_window_frame(None, &[laptop()]);
        assert_eq!(f.width, DEFAULT_WIDTH.min(1512 - 40));
        assert_eq!(f.height, DEFAULT_HEIGHT.min(982 - 40));
        assert_eq!(f.x, (1512 - f.width as i32) / 2);
        assert_eq!(f.y, (982 - f.height as i32) / 2);
        assert!(!f.is_maximized);
    }

    #[test]
    fn keeps_a_valid_saved_frame() {
        let s = saved(100, 80, 1200, 800);
        let f = resolve_window_frame(Some(&s), &[laptop()]);
        assert_eq!(f.x, 100);
        assert_eq!(f.y, 80);
        assert_eq!(f.width, 1200);
        assert_eq!(f.height, 800);
    }

    #[test]
    fn offscreen_saved_position_recenters_keeping_size() {
        let s = saved(8000, 40, 1200, 800);
        let f = resolve_window_frame(Some(&s), &[laptop()]);
        assert_eq!(f.width, 1200);
        assert_eq!(f.height, 800);
        assert!(f.x >= 0 && f.x + f.width as i32 <= 1512);
        assert!(f.y >= 0);
    }

    #[test]
    fn tiny_saved_size_uses_centered_default() {
        let s = saved(10, 10, 200, 120);
        let f = resolve_window_frame(Some(&s), &[laptop()]);
        assert!(f.width >= MIN_WIDTH);
        assert!(f.height >= MIN_HEIGHT);
        assert_eq!(f.width, DEFAULT_WIDTH.min(1512 - 40));
    }

    #[test]
    fn unplugged_external_display_recenters_on_laptop() {
        let s = saved(1800, 100, 1400, 900);
        let f = resolve_window_frame(Some(&s), &[laptop()]);
        assert!(f.x + 80 < 1512);
        assert!(f.x >= 0);
    }

    #[test]
    fn dual_display_keeps_window_on_external() {
        let s = saved(1700, 80, 1400, 900);
        let f = resolve_window_frame(Some(&s), &[laptop(), external()]);
        assert_eq!(f.x, 1700);
        assert_eq!(f.width, 1400);
    }

    #[test]
    fn frame_is_usable_rejects_tiny_and_offscreen() {
        let m = [laptop()];
        assert!(!frame_is_usable(10, 10, 200, 100, &m));
        assert!(!frame_is_usable(9000, 10, 1200, 800, &m));
        assert!(frame_is_usable(40, 40, 1200, 800, &m));
    }
}
