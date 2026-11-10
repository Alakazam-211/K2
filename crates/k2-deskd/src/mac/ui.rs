//! The in-use banner (S6), as ordinary UX like any screen-share indicator
//! (Rosson 2026-11-10: not a security guarantee). A small floating panel
//! at the top of the main screen: "Rosson is controlling this screen from
//! MacBook over K2 Connect" and a **Disconnect** button.
//!
//! - Non-activating, on every Space and over full-screen apps; it never
//!   takes focus from what the person is doing.
//! - Left out of capture: `NSWindowSharingNone` keeps it out of other apps'
//!   captures, and kessel-video's own-app filter (by pid) keeps it out of
//!   deskd's own (D5).
//! - AppKit runs on the main thread. The daemon link and the tap run on
//!   other threads and set [`BannerShared`]; the main loop pumps events
//!   every 50 ms (the k2-menubar way) and applies the newest state.
//! - The button is a push-on/push-off button polled for "on", so no
//!   Objective-C target class is needed.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSButton, NSButtonType, NSColor, NSControlStateValueOff,
    NSControlStateValueOn, NSEventMask, NSFont, NSPanel, NSScreen, NSStatusWindowLevel, NSTextField, NSWindowCollectionBehavior,
    NSWindowSharingType, NSWindowStyleMask,
};
use objc2_foundation::{ns_string, NSDate, NSDefaultRunLoopMode, NSPoint, NSRect, NSSize, NSString};

/// What the banner should show; set from any thread.
pub struct BannerShared {
    want: Mutex<Option<String>>,
    gen: AtomicU64,
    on_disconnect: Box<dyn Fn() + Send + Sync>,
}

impl BannerShared {
    pub fn new(on_disconnect: Box<dyn Fn() + Send + Sync>) -> Arc<Self> {
        Arc::new(BannerShared { want: Mutex::new(None), gen: AtomicU64::new(0), on_disconnect })
    }

    pub fn set(&self, text: Option<String>) {
        *self.want.lock().unwrap() = text;
        self.gen.fetch_add(1, Ordering::SeqCst);
    }
}

pub const HEIGHT: f64 = 36.0;
const BUTTON_W: f64 = 104.0;
const PAD: f64 = 12.0;

/// Panel width for a text (pure, for tests).
pub fn width_for(text: &str) -> f64 {
    (text.chars().count() as f64 * 7.4 + BUTTON_W + 3.0 * PAD).clamp(360.0, 960.0)
}

struct Panel {
    panel: Retained<NSPanel>,
    label: Retained<NSTextField>,
    button: Retained<NSButton>,
}

fn build(mtm: MainThreadMarker) -> Panel {
    let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(480.0, HEIGHT));
    let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
        NSPanel::alloc(mtm),
        rect,
        NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
        NSBackingStoreType::Buffered,
        false,
    );
    // SAFETY: we keep the Retained ourselves, so the panel must not
    // release itself on close.
    unsafe { panel.setReleasedWhenClosed(false) };
    panel.setFloatingPanel(true);
    panel.setBecomesKeyOnlyIfNeeded(true);
    panel.setHidesOnDeactivate(false);
    panel.setMovable(false);
    panel.setLevel(NSStatusWindowLevel);
    panel.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::IgnoresCycle,
    );
    panel.setSharingType(NSWindowSharingType::None);
    panel.setOpaque(false);
    panel.setHasShadow(true);
    panel.setBackgroundColor(Some(&NSColor::colorWithSRGBRed_green_blue_alpha(0.10, 0.10, 0.11, 0.94)));

    let label = NSTextField::labelWithString(ns_string!(""), mtm);
    label.setTextColor(Some(&NSColor::whiteColor()));
    label.setFont(Some(&NSFont::systemFontOfSize(13.0)));
    // SAFETY: no target and no action: the button is polled for its state.
    let button = unsafe { NSButton::buttonWithTitle_target_action(ns_string!("Disconnect"), None, None, mtm) };
    button.setButtonType(NSButtonType::PushOnPushOff);
    button.setState(NSControlStateValueOff);
    let view = panel.contentView().expect("a panel has a content view");
    view.addSubview(&label);
    view.addSubview(&button);
    Panel { panel, label, button }
}

fn show(p: &Panel, text: &str, mtm: MainThreadMarker) {
    let w = width_for(text);
    p.label.setStringValue(&NSString::from_str(text));
    p.label.setFrame(NSRect::new(NSPoint::new(PAD, 9.0), NSSize::new(w - BUTTON_W - 3.0 * PAD, 18.0)));
    p.button.setFrame(NSRect::new(NSPoint::new(w - BUTTON_W - PAD, 4.0), NSSize::new(BUTTON_W, 28.0)));
    let screen = NSScreen::mainScreen(mtm).map(|s| s.visibleFrame());
    let (x, y) = match screen {
        Some(f) => (f.origin.x + (f.size.width - w) / 2.0, f.origin.y + f.size.height - HEIGHT - 8.0),
        None => (40.0, 40.0),
    };
    p.panel.setFrame_display(NSRect::new(NSPoint::new(x, y), NSSize::new(w, HEIGHT)), true);
    p.panel.orderFrontRegardless();
}

/// Runs AppKit on the main thread for ever. Call from `main`.
pub fn run_main_loop(shared: Arc<BannerShared>) -> ! {
    let mtm = MainThreadMarker::new().expect("deskd's UI must run on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    // No Dock icon (the bundle is LSUIElement too).
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    app.finishLaunching();
    let mut panel: Option<Panel> = None;
    let mut applied = 0u64;
    loop {
        let g = shared.gen.load(Ordering::SeqCst);
        if g != applied {
            applied = g;
            let want = shared.want.lock().unwrap().clone();
            match want {
                Some(text) => {
                    let p = panel.get_or_insert_with(|| build(mtm));
                    p.button.setState(NSControlStateValueOff);
                    show(p, &text, mtm);
                }
                None => {
                    if let Some(p) = &panel {
                        p.panel.orderOut(None);
                    }
                }
            }
        }
        if let Some(p) = &panel {
            if p.button.state() == NSControlStateValueOn {
                p.button.setState(NSControlStateValueOff);
                (shared.on_disconnect)();
            }
        }
        let until = NSDate::dateWithTimeIntervalSinceNow(0.05);
        // SAFETY: NSDefaultRunLoopMode is a framework constant.
        while let Some(event) = app.nextEventMatchingMask_untilDate_inMode_dequeue(NSEventMask::Any, Some(&until), unsafe { NSDefaultRunLoopMode }, true) {
            app.sendEvent(&event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_grows_with_the_text_and_stays_in_bounds() {
        assert_eq!(width_for(""), 360.0);
        let long = "Rosson is controlling this screen from MacBook Pro over K2 Connect";
        assert!(width_for(long) > 360.0 && width_for(long) < 960.0);
        assert_eq!(width_for(&"x".repeat(500)), 960.0);
    }

    #[test]
    fn set_bumps_the_generation() {
        let s = BannerShared::new(Box::new(|| {}));
        s.set(Some("a".into()));
        s.set(None);
        assert_eq!(s.gen.load(Ordering::SeqCst), 2);
        assert_eq!(*s.want.lock().unwrap(), None);
    }
}
