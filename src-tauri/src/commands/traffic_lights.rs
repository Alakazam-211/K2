//! macOS traffic-light repositioning for the Style System's floating-chrome
//! styles (Glass/Bezel/spacious presets): when the chrome is inset from the
//! window edge, the close/minimize/zoom buttons must move down-right with it.
//!
//! AppKit resets standard-button frames on resize/fullscreen transitions, so
//! the renderer re-invokes this after style changes AND on window-resize
//! events (see stores/style.ts). `extra_x`/`extra_y` are logical px offsets
//! from the system default position; (0, 0) restores the default exactly
//! (defaults are captured on first call, before any modification).

#[cfg(target_os = "macos")]
mod imp {
  use std::sync::OnceLock;

  #[derive(Clone, Copy)]
  struct Defaults {
    button_x: f64,
    titlebar_h: f64,
  }

  static DEFAULTS: OnceLock<Defaults> = OnceLock::new();

  pub unsafe fn position(window: &tauri::Window, extra_x: f64, extra_y: f64) {
    use cocoa::appkit::{NSWindow, NSWindowButton};
    use cocoa::base::{id, nil};
    use cocoa::foundation::NSRect;
    use objc::{msg_send, sel, sel_impl};

    let Ok(handle) = window.ns_window() else {
      return;
    };
    let ns_window = handle as id;
    let close = ns_window.standardWindowButton_(NSWindowButton::NSWindowCloseButton);
    let mini = ns_window.standardWindowButton_(NSWindowButton::NSWindowMiniaturizeButton);
    let zoom = ns_window.standardWindowButton_(NSWindowButton::NSWindowZoomButton);
    if close == nil || mini == nil || zoom == nil {
      return;
    }

    let title_bar_container: id = {
      let sv: id = msg_send![close, superview];
      if sv == nil {
        return;
      }
      msg_send![sv, superview]
    };
    if title_bar_container == nil {
      return;
    }

    // System-default geometry, captured before the first modification so
    // (0, 0) can restore it byte-exactly when switching back to Square.
    let defaults = *DEFAULTS.get_or_init(|| {
      let close_rect: NSRect = msg_send![close, frame];
      let tb_rect: NSRect = msg_send![title_bar_container, frame];
      Defaults {
        button_x: close_rect.origin.x,
        titlebar_h: tb_rect.size.height,
      }
    });

    // Grow the title-bar container downward from the top edge; the buttons
    // ride down with it (AppKit vertically centers them in the container).
    let title_bar_h = defaults.titlebar_h + extra_y;
    let win_frame: NSRect = msg_send![ns_window, frame];
    let mut tb_rect: NSRect = msg_send![title_bar_container, frame];
    tb_rect.size.height = title_bar_h;
    tb_rect.origin.y = win_frame.size.height - title_bar_h;
    let _: () = msg_send![title_bar_container, setFrame: tb_rect];

    // Shift the three buttons right, preserving the system spacing.
    let close_f: NSRect = msg_send![close, frame];
    let mini_f: NSRect = msg_send![mini, frame];
    let spacing = mini_f.origin.x - close_f.origin.x;
    for (i, btn) in [close, mini, zoom].iter().enumerate() {
      let mut r: NSRect = msg_send![*btn, frame];
      r.origin.x = defaults.button_x + extra_x + (i as f64) * spacing;
      let _: () = msg_send![*btn, setFrameOrigin: r.origin];
    }
  }
}

/// Offset the macOS traffic lights by (x, y) logical px from their default
/// position. (0, 0) restores the default. No-op on other platforms.
#[tauri::command]
pub fn set_traffic_light_inset(window: tauri::Window, x: f64, y: f64) -> Result<(), String> {
  #[cfg(target_os = "macos")]
  {
    let w = window.clone();
    window
      .run_on_main_thread(move || unsafe { imp::position(&w, x, y) })
      .map_err(|e| e.to_string())
  }
  #[cfg(not(target_os = "macos"))]
  {
    let _ = (window, x, y);
    Ok(())
  }
}
