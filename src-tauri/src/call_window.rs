//! The call's own window (Rosson 2026-11-10, "pop out"): a voice room's video
//! and controls move out of the room into one OS window, as a Focus window
//! does for one agent. OS integration only: the call itself lives in the
//! sew-voice plugin (this process, not a webview), so the new window just
//! adopts it, as any K2 window does, and closing it never hangs up.
//!
//! One window per app (label `window-call`, fragment `#call`): opening it
//! again focuses it. `window-*` so the default capability covers it. The
//! renderer passes the size and place it saved last time (per client, in
//! its localStorage); a place no longer on a display is dropped and the
//! window is centred. Every window hears `k2-call-popout` `{ open }` when
//! it opens and when it is destroyed, so the call comes back to the room
//! (the floating tile) however the window goes.

use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager};

pub const CALL_WINDOW_LABEL: &str = "window-call";
pub const CALL_POPOUT_EVENT: &str = "k2-call-popout";

/// Logical px.
pub const CALL_WINDOW_MIN: (f64, f64) = (360.0, 240.0);
pub const CALL_WINDOW_DEFAULT: (f64, f64) = (720.0, 480.0);
/// Larger than any display: a saved size beyond it is junk.
const CALL_WINDOW_MAX: f64 = 16_384.0;
/// How much of the window must sit on a display for its saved place to count.
const MIN_VISIBLE: (f64, f64) = (160.0, 100.0);

/// A window's frame, or a display's, in logical px.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct CallWindowFrame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CallWindowPlacement {
    pub width: f64,
    pub height: f64,
    /// None: centre it.
    pub position: Option<(f64, f64)>,
}

fn overlap(a: &CallWindowFrame, b: &CallWindowFrame) -> (f64, f64) {
    let w = (a.x + a.width).min(b.x + b.width) - a.x.max(b.x);
    let h = (a.y + a.height).min(b.y + b.height) - a.y.max(b.y);
    (w.max(0.0), h.max(0.0))
}

/// The saved frame, where it still makes sense: a size within bounds (else
/// the default), and a place that is still on a display (else centred).
pub fn call_window_placement(saved: Option<CallWindowFrame>, monitors: &[CallWindowFrame]) -> CallWindowPlacement {
    let default = CallWindowPlacement {
        width: CALL_WINDOW_DEFAULT.0,
        height: CALL_WINDOW_DEFAULT.1,
        position: None,
    };
    let Some(s) = saved else { return default };
    let finite = [s.x, s.y, s.width, s.height].iter().all(|v| v.is_finite());
    if !finite {
        return default;
    }
    let size_ok = s.width <= CALL_WINDOW_MAX && s.height <= CALL_WINDOW_MAX && s.width > 0.0 && s.height > 0.0;
    let (width, height) = if size_ok {
        (s.width.max(CALL_WINDOW_MIN.0), s.height.max(CALL_WINDOW_MIN.1))
    } else {
        CALL_WINDOW_DEFAULT
    };
    let frame = CallWindowFrame { x: s.x, y: s.y, width, height };
    let on_screen = monitors.is_empty()
        || monitors.iter().any(|m| {
            let (w, h) = overlap(&frame, m);
            w >= MIN_VISIBLE.0 && h >= MIN_VISIBLE.1
        });
    CallWindowPlacement {
        width,
        height,
        position: if size_ok && on_screen { Some((s.x, s.y)) } else { None },
    }
}

fn logical_monitors(app: &AppHandle) -> Vec<CallWindowFrame> {
    let Ok(all) = app.available_monitors() else { return Vec::new() };
    all.iter()
        .map(|m| {
            let scale = m.scale_factor();
            let pos = m.position().to_logical::<f64>(scale);
            let size = m.size().to_logical::<f64>(scale);
            CallWindowFrame { x: pos.x, y: pos.y, width: size.width, height: size.height }
        })
        .collect()
}

/// Open the call's window, or focus it if it's open.
///
/// `async`: Tauri v2 recommends building windows from async commands (a
/// sync command that creates one can deadlock on Windows).
#[tauri::command]
pub async fn window_open_call(app: AppHandle, frame: Option<CallWindowFrame>) -> Result<(), String> {
    if let Some(existing) = app.get_webview_window(CALL_WINDOW_LABEL) {
        let _ = existing.unminimize();
        existing.set_focus().map_err(|e| e.to_string())?;
        return Ok(());
    }
    let placed = call_window_placement(frame, &logical_monitors(&app));
    let webview_url = crate::k2_app_window::k2_app_webview_url(&app, Some("call"));
    let builder = crate::k2_app_window::k2_app_window_builder(&app, CALL_WINDOW_LABEL, webview_url)
        .title("K2 | Call")
        .inner_size(placed.width, placed.height)
        .min_inner_size(CALL_WINDOW_MIN.0, CALL_WINDOW_MIN.1)
        .resizable(true);
    let builder = match placed.position {
        Some((x, y)) => builder.position(x, y),
        None => builder.center(),
    };
    // hidden_title / TitleBarStyle::Overlay are macOS-only builder methods.
    #[cfg(target_os = "macos")]
    let builder = builder
        .hidden_title(true)
        .title_bar_style(tauri::TitleBarStyle::Overlay);
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    let builder = builder.decorations(false);
    let window = builder.build().map_err(|e| e.to_string())?;
    let handle = app.clone();
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::Destroyed = event {
            let _ = handle.emit(CALL_POPOUT_EVENT, serde_json::json!({ "open": false }));
        }
    });
    let _ = app.emit(CALL_POPOUT_EVENT, serde_json::json!({ "open": true }));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DISPLAY: CallWindowFrame = CallWindowFrame { x: 0.0, y: 0.0, width: 1512.0, height: 982.0 };

    fn frame(x: f64, y: f64, width: f64, height: f64) -> Option<CallWindowFrame> {
        Some(CallWindowFrame { x, y, width, height })
    }

    #[test]
    fn nothing_saved_is_the_default_size_centred() {
        assert_eq!(
            call_window_placement(None, &[DISPLAY]),
            CallWindowPlacement { width: 720.0, height: 480.0, position: None }
        );
    }

    #[test]
    fn a_saved_frame_on_a_display_comes_back() {
        assert_eq!(
            call_window_placement(frame(100.0, 80.0, 900.0, 560.0), &[DISPLAY]),
            CallWindowPlacement { width: 900.0, height: 560.0, position: Some((100.0, 80.0)) }
        );
    }

    #[test]
    fn a_place_off_every_display_is_centred_but_keeps_its_size() {
        // The second display was unplugged.
        assert_eq!(
            call_window_placement(frame(2000.0, 100.0, 900.0, 560.0), &[DISPLAY]),
            CallWindowPlacement { width: 900.0, height: 560.0, position: None }
        );
        // A sliver at the edge is not on the display either.
        assert_eq!(call_window_placement(frame(1450.0, 100.0, 900.0, 560.0), &[DISPLAY]).position, None);
    }

    #[test]
    fn a_second_display_counts() {
        let right = CallWindowFrame { x: 1512.0, y: 0.0, width: 1920.0, height: 1080.0 };
        assert_eq!(
            call_window_placement(frame(2000.0, 100.0, 900.0, 560.0), &[DISPLAY, right]).position,
            Some((2000.0, 100.0))
        );
    }

    #[test]
    fn too_small_grows_to_the_minimum_and_junk_is_the_default() {
        let small = call_window_placement(frame(10.0, 10.0, 100.0, 50.0), &[DISPLAY]);
        assert_eq!((small.width, small.height), (360.0, 240.0));
        for junk in [frame(f64::NAN, 0.0, 800.0, 500.0), frame(0.0, 0.0, 1e9, 500.0), frame(0.0, 0.0, -5.0, 500.0)] {
            assert_eq!(
                call_window_placement(junk, &[DISPLAY]),
                CallWindowPlacement { width: 720.0, height: 480.0, position: None }
            );
        }
    }

    #[test]
    fn no_display_list_trusts_the_saved_place() {
        assert_eq!(call_window_placement(frame(5.0, 6.0, 800.0, 500.0), &[]).position, Some((5.0, 6.0)));
    }
}
