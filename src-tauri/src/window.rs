//! Window state read/write helpers.
//!
//! Phase 2 Unit 4 — the previous in-process SQLite reads/writes
//! moved to the daemon's `/cli/window-state/{get,set}` routes.
//! These helpers now proxy via `DaemonClient`.
//!
//! Saved frames and monitor rects are physical pixels; the size rules
//! (minimum, first-launch default) are logical points, converted with
//! each monitor's scale factor. On a 2x display a "1400×900" applied as
//! physical pixels is a 700×450 window, which the minimum then clamps.
//!
//! Restore keeps the saved frame, including a small one the person
//! chose, pulled into the work area of a current display. With no saved
//! frame (first launch) the window opens at a comfortable default.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{Manager, PhysicalPosition, PhysicalSize};

use crate::daemon_client::DaemonClient;

/// Minimum window size, logical points (matches tauri.conf.json).
pub const MIN_WIDTH: u32 = 800;
pub const MIN_HEIGHT: u32 = 600;
/// First-launch / new-window size, logical points.
pub const DEFAULT_WIDTH: u32 = 1280;
pub const DEFAULT_HEIGHT: u32 = 820;
/// The default never takes more than this share of the work area.
const DEFAULT_MAX_SHARE: f64 = 0.85;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowState {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub is_maximized: bool,
}

/// A display's work area (screen minus menu bar / dock / taskbar), in
/// physical pixels (same space as `outer_position` / `outer_size`),
/// plus the display's scale factor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MonitorRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale: f64,
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
    client
        .cli_get_json("/cli/window-state/get", &[])
        .ok()
        .flatten()
}

/// Place the main window: the saved frame pulled into view, or the
/// centered first-launch default when nothing is saved.
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

/// Size, in logical points, for a new main-style window (New Window):
/// the same default as a first launch, sized for the primary display,
/// which is where the window builder's `center()` places it.
pub fn new_window_logical_size(app: &tauri::AppHandle) -> (f64, f64) {
    let (w, h) = match app.primary_monitor() {
        Ok(Some(mon)) => {
            let m = monitor_to_rect(&mon);
            let s = scale_of(&m);
            default_logical_size(m.width as f64 / s, m.height as f64 / s)
        }
        _ => (DEFAULT_WIDTH, DEFAULT_HEIGHT),
    };
    (w as f64, h as f64)
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
    let work = mon.work_area();
    let (pos, size) = if work.size.width > 0 && work.size.height > 0 {
        (work.position, work.size)
    } else {
        (*mon.position(), *mon.size())
    };
    MonitorRect {
        x: pos.x,
        y: pos.y,
        width: size.width,
        height: size.height,
        scale: mon.scale_factor(),
    }
}

/// Minimum on-screen slice, logical points — a title-bar peek at the
/// bezel is a sliver, not a usable restore.
const MIN_VISIBLE_WIDTH: u32 = 400;
const MIN_VISIBLE_HEIGHT: u32 = 300;

fn scale_of(m: &MonitorRect) -> f64 {
    if m.scale.is_finite() && m.scale > 0.0 {
        m.scale
    } else {
        1.0
    }
}

fn to_physical(logical: u32, scale: f64) -> u32 {
    (logical as f64 * scale).round() as u32
}

/// Physical → logical with one point of slack, so a minimum-size window
/// saved on a fractional scale (rounded down a pixel) still counts as
/// minimum size.
fn to_logical_ceil(physical: u32, scale: f64) -> u32 {
    ((physical as f64 + 1.0) / scale).floor() as u32
}

fn intersection_size(
    ax: i32,
    ay: i32,
    aw: u32,
    ah: u32,
    bx: i32,
    by: i32,
    bw: u32,
    bh: u32,
) -> (u32, u32) {
    let ax2 = ax as i64 + aw as i64;
    let ay2 = ay as i64 + ah as i64;
    let bx2 = bx as i64 + bw as i64;
    let by2 = by as i64 + bh as i64;
    let x1 = (ax as i64).max(bx as i64);
    let y1 = (ay as i64).max(by as i64);
    let x2 = ax2.min(bx2);
    let y2 = ay2.min(by2);
    if x2 <= x1 || y2 <= y1 {
        return (0, 0);
    }
    ((x2 - x1) as u32, (y2 - y1) as u32)
}

/// The monitor holding the largest visible part of the rect, if that
/// part is a real chunk of window (not a few pixels at the edge).
fn usable_monitor(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    monitors: &[MonitorRect],
) -> Option<&MonitorRect> {
    monitors
        .iter()
        .filter_map(|m| {
            let (vw, vh) = intersection_size(x, y, width, height, m.x, m.y, m.width, m.height);
            let s = scale_of(m);
            let enough =
                vw >= to_physical(MIN_VISIBLE_WIDTH, s) && vh >= to_physical(MIN_VISIBLE_HEIGHT, s);
            enough.then_some((m, vw as u64 * vh as u64))
        })
        .max_by_key(|(_, area)| *area)
        .map(|(m, _)| m)
}

/// True when the window is at least the minimum size and a real chunk of
/// it sits on a current display. Sizes compare in logical points.
pub fn frame_is_usable(x: i32, y: i32, width: u32, height: u32, monitors: &[MonitorRect]) -> bool {
    if monitors.is_empty() {
        return width >= MIN_WIDTH && height >= MIN_HEIGHT;
    }
    let Some(m) = usable_monitor(x, y, width, height, monitors) else {
        return false;
    };
    let s = scale_of(m);
    to_logical_ceil(width, s) >= MIN_WIDTH && to_logical_ceil(height, s) >= MIN_HEIGHT
}

/// First-launch / new-window size in logical points for a work area of
/// `work_width`×`work_height` logical points: 1280×820, but at most 85%
/// of the work area, and never under the minimum.
pub fn default_logical_size(work_width: f64, work_height: f64) -> (u32, u32) {
    let cap = |work: f64| (work * DEFAULT_MAX_SHARE).floor().max(0.0) as u32;
    let width = DEFAULT_WIDTH.min(cap(work_width)).max(MIN_WIDTH);
    let height = DEFAULT_HEIGHT.min(cap(work_height)).max(MIN_HEIGHT);
    (width, height)
}

fn default_frame(monitors: &[MonitorRect]) -> WindowFrame {
    let Some(m) = monitors.first() else {
        return WindowFrame {
            x: 80,
            y: 60,
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            is_maximized: false,
        };
    };
    let s = scale_of(m);
    let (lw, lh) = default_logical_size(m.width as f64 / s, m.height as f64 / s);
    let width = to_physical(lw, s);
    let height = to_physical(lh, s);
    WindowFrame {
        x: m.x + (m.width as i32 - width as i32) / 2,
        y: m.y + (m.height as i32 - height as i32) / 2,
        width,
        height,
        is_maximized: false,
    }
}

/// Clamp one axis of a window into `[start, start + span)`: shrink to the
/// span, then shift so the whole window is inside.
fn clamp_axis(pos: i32, len: u32, start: i32, span: u32) -> (i32, u32) {
    let len = len.min(span);
    let max_pos = start as i64 + span as i64 - len as i64;
    let pos = (pos as i64).clamp(start as i64, max_pos) as i32;
    (pos, len)
}

/// Pick the main window's frame. No saved frame (first launch) → the
/// centered default. A saved frame keeps its size and position, a small
/// one too, and is pulled into the work area of the display it is on: a
/// size larger than that display is shrunk to fit, an edge hanging off
/// screen is moved back in. A frame that is off every display moves to
/// the primary display. A saved size under the minimum cannot come from
/// a person (the window enforces the minimum), so it gets the default.
pub fn resolve_window_frame(saved: Option<&WindowState>, monitors: &[MonitorRect]) -> WindowFrame {
    let default = default_frame(monitors);
    let Some(saved) = saved else {
        return default;
    };
    let with_max = |mut f: WindowFrame| {
        f.is_maximized = saved.is_maximized;
        f
    };
    let Some(primary) = monitors.first() else {
        if saved.width >= MIN_WIDTH && saved.height >= MIN_HEIGHT {
            return WindowFrame {
                x: saved.x,
                y: saved.y,
                width: saved.width,
                height: saved.height,
                is_maximized: saved.is_maximized,
            };
        }
        return with_max(default);
    };

    let on = usable_monitor(saved.x, saved.y, saved.width, saved.height, monitors);
    let target = on.unwrap_or(primary);
    let s = scale_of(target);
    if to_logical_ceil(saved.width, s) < MIN_WIDTH || to_logical_ceil(saved.height, s) < MIN_HEIGHT
    {
        return with_max(default);
    }

    let (x, y) = match on {
        Some(_) => (saved.x, saved.y),
        // Off every display: center on the primary one.
        None => (
            target.x + (target.width as i32 - saved.width as i32) / 2,
            target.y + (target.height as i32 - saved.height as i32) / 2,
        ),
    };
    let (x, width) = clamp_axis(x, saved.width, target.x, target.width);
    let (y, height) = clamp_axis(y, saved.height, target.y, target.height);
    WindowFrame {
        x,
        y,
        width,
        height,
        is_maximized: saved.is_maximized,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 14" laptop work area at 2x: 1512×945 points under a 37-point menu bar.
    fn retina_laptop() -> MonitorRect {
        MonitorRect {
            x: 0,
            y: 74,
            width: 3024,
            height: 1890,
            scale: 2.0,
        }
    }

    /// 1x laptop work area, 1512×982.
    fn laptop() -> MonitorRect {
        MonitorRect {
            x: 0,
            y: 0,
            width: 1512,
            height: 982,
            scale: 1.0,
        }
    }

    /// 1x external display to the right of `laptop()`.
    fn external() -> MonitorRect {
        MonitorRect {
            x: 1512,
            y: 0,
            width: 2560,
            height: 1440,
            scale: 1.0,
        }
    }

    /// Small 1x display: 1024×700 work area.
    fn small() -> MonitorRect {
        MonitorRect {
            x: 0,
            y: 0,
            width: 1024,
            height: 700,
            scale: 1.0,
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

    fn inside(f: &WindowFrame, m: &MonitorRect) -> bool {
        f.x >= m.x
            && f.y >= m.y
            && f.x as i64 + f.width as i64 <= m.x as i64 + m.width as i64
            && f.y as i64 + f.height as i64 <= m.y as i64 + m.height as i64
    }

    #[test]
    fn first_launch_on_retina_opens_comfortable_not_minimum() {
        // Applying the default as physical pixels on a 2x display gave a
        // window half the size in points, which the minimum then clamped.
        let m = retina_laptop();
        let f = resolve_window_frame(None, &[m]);
        assert_eq!(
            (f.width, f.height),
            (1280 * 2, 803 * 2),
            "85% of 945 points is 803"
        );
        assert_eq!(f.x, (3024 - 2560) / 2);
        assert_eq!(f.y, 74 + (1890 - 1606) / 2);
        assert!(inside(&f, &m));
        assert!(!f.is_maximized);
    }

    #[test]
    fn first_launch_on_a_large_display_uses_the_full_default() {
        let f = resolve_window_frame(None, &[external()]);
        assert_eq!((f.width, f.height), (DEFAULT_WIDTH, DEFAULT_HEIGHT));
        assert_eq!(f.x, 1512 + (2560 - 1280) / 2);
        assert_eq!(f.y, (1440 - 820) / 2);
    }

    #[test]
    fn first_launch_on_a_small_display_takes_85_percent() {
        let f = resolve_window_frame(None, &[small()]);
        assert_eq!(
            (f.width, f.height),
            (870, 600),
            "85% of 1024×700, height floored at the minimum"
        );
        assert!(inside(&f, &small()));
        assert_eq!(default_logical_size(1100.0, 900.0), (935, 765));
        assert_eq!(
            default_logical_size(2560.0, 1440.0),
            (DEFAULT_WIDTH, DEFAULT_HEIGHT)
        );
    }

    #[test]
    fn keeps_a_valid_saved_frame() {
        let s = saved(100, 80, 1200, 800);
        let f = resolve_window_frame(Some(&s), &[laptop()]);
        assert_eq!((f.x, f.y, f.width, f.height), (100, 80, 1200, 800));
    }

    #[test]
    fn keeps_a_saved_minimum_size_window_on_retina() {
        // 800×600 points, the smallest a person can make it, at 2x.
        let s = saved(200, 300, 1600, 1200);
        let f = resolve_window_frame(Some(&s), &[retina_laptop()]);
        assert_eq!((f.x, f.y, f.width, f.height), (200, 300, 1600, 1200));
        assert!(frame_is_usable(200, 300, 1600, 1200, &[retina_laptop()]));
    }

    #[test]
    fn keeps_a_saved_minimum_size_window_on_1x() {
        let s = saved(40, 40, 800, 600);
        let f = resolve_window_frame(Some(&s), &[laptop()]);
        assert_eq!((f.x, f.y, f.width, f.height), (40, 40, 800, 600));
    }

    #[test]
    fn saved_size_under_the_minimum_uses_the_default() {
        let s = saved(10, 10, 200, 120);
        let f = resolve_window_frame(Some(&s), &[laptop()]);
        assert_eq!(f, resolve_window_frame(None, &[laptop()]));
        // 1000×700 physical is 500×350 points at 2x: not a size a person can pick.
        let s = saved(10, 80, 1000, 700);
        let f = resolve_window_frame(Some(&s), &[retina_laptop()]);
        assert_eq!(f, resolve_window_frame(None, &[retina_laptop()]));
    }

    #[test]
    fn maximized_only_row_uses_the_default_and_stays_maximized() {
        let s = WindowState {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            is_maximized: true,
        };
        let f = resolve_window_frame(Some(&s), &[laptop()]);
        assert!(f.is_maximized);
        assert_eq!((f.width, f.height), (DEFAULT_WIDTH, DEFAULT_HEIGHT));
    }

    #[test]
    fn saved_size_larger_than_the_display_is_shrunk_into_view() {
        let s = saved(-200, -50, 2560, 1440);
        let f = resolve_window_frame(Some(&s), &[laptop()]);
        assert_eq!((f.x, f.y, f.width, f.height), (0, 0, 1512, 982));
    }

    #[test]
    fn edge_hanging_off_screen_is_moved_back_in() {
        let s = saved(700, 400, 1200, 800);
        let f = resolve_window_frame(Some(&s), &[laptop()]);
        assert_eq!((f.width, f.height), (1200, 800));
        assert_eq!((f.x, f.y), (1512 - 1200, 982 - 800));
    }

    #[test]
    fn offscreen_saved_position_recenters_keeping_size() {
        let s = saved(8000, 40, 1200, 800);
        let f = resolve_window_frame(Some(&s), &[laptop()]);
        assert_eq!((f.width, f.height), (1200, 800));
        assert_eq!((f.x, f.y), ((1512 - 1200) / 2, (982 - 800) / 2));
    }

    #[test]
    fn unplugged_external_display_recenters_on_laptop() {
        let s = saved(1800, 100, 1400, 900);
        let f = resolve_window_frame(Some(&s), &[laptop()]);
        assert_eq!((f.width, f.height), (1400, 900));
        assert!(inside(&f, &laptop()));
    }

    #[test]
    fn dual_display_keeps_window_on_external() {
        let s = saved(1700, 80, 1400, 900);
        let f = resolve_window_frame(Some(&s), &[laptop(), external()]);
        assert_eq!((f.x, f.y, f.width, f.height), (1700, 80, 1400, 900));
    }

    #[test]
    fn sliver_on_the_right_edge_recenters() {
        // 32px of a 1200-wide window on a 1512 display.
        let s = saved(1480, 80, 1200, 800);
        let f = resolve_window_frame(Some(&s), &[laptop()]);
        assert_eq!((f.width, f.height), (1200, 800));
        assert!(inside(&f, &laptop()));
    }

    #[test]
    fn frame_is_usable_rejects_tiny_and_offscreen() {
        let m = [laptop()];
        assert!(!frame_is_usable(10, 10, 200, 100, &m));
        assert!(!frame_is_usable(9000, 10, 1200, 800, &m));
        assert!(frame_is_usable(40, 40, 1200, 800, &m));
        // 1200×800 physical is 600×400 points at 2x: under the minimum.
        assert!(!frame_is_usable(40, 80, 1200, 800, &[retina_laptop()]));
    }

    #[test]
    fn config_sizes_match_the_constants() {
        let conf: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json"))
            .expect("tauri.conf.json parses");
        let main = &conf["app"]["windows"][0];
        assert_eq!(main["label"], "main");
        assert_eq!(main["width"].as_u64(), Some(DEFAULT_WIDTH as u64));
        assert_eq!(main["height"].as_u64(), Some(DEFAULT_HEIGHT as u64));
        assert_eq!(main["minWidth"].as_u64(), Some(MIN_WIDTH as u64));
        assert_eq!(main["minHeight"].as_u64(), Some(MIN_HEIGHT as u64));
    }
}
