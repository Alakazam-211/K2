//! Per-window macOS corner radius.
//!
//! The renderer passes 0.5 for style id `square` and 0 for every other
//! id. 0 restores the system squircle; do not send 0 for Square and do
//! not hardcode 16. Private `-[NSWindow _setCornerRadius:]`, or KVC
//! `cornerRadius` when that selector is missing. Never set the key
//! `_cornerRadius` (the set throws). `invalidateShadow` runs after the
//! set. The invoking window is the only one touched. Non-mac is a no-op.

#[cfg(target_os = "macos")]
#[allow(deprecated)]
mod imp {
    use std::cell::Cell;
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};

    use cocoa::base::{id, nil, BOOL, YES};
    use cocoa::foundation::NSString;
    use objc::declare::ClassDecl;
    use objc::runtime::{Class, Object, Sel};
    use objc::{class, msg_send, sel, sel_impl};

    static RADII: OnceLock<Mutex<HashMap<usize, f64>>> = OnceLock::new();

    fn radii() -> std::sync::MutexGuard<'static, HashMap<usize, f64>> {
        RADII
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    pub fn apply(window: &tauri::Window, radius: f64) {
        let Ok(handle) = window.ns_window() else {
            return;
        };
        unsafe { apply_ns(handle as id, radius) }
    }

    unsafe fn apply_ns(ns_window: id, radius: f64) {
        if ns_window == nil {
            return;
        }
        // Remember this window only. A Glass window keeps its own entry.
        radii().insert(ns_window as usize, radius);
        ensure_observers();
        set_radius(ns_window, radius);
    }

    /// Private setter, then `invalidateShadow`. The KVC fallback key is
    /// `cornerRadius`. Do not set `_cornerRadius`.
    unsafe fn set_radius(ns_window: id, radius: f64) {
        thread_local! {
            static BUSY: Cell<bool> = const { Cell::new(false) };
        }
        // invalidateShadow can deliver a notification before we return.
        if BUSY.with(|busy| busy.replace(true)) {
            return;
        }
        let sel_set = sel!(_setCornerRadius:);
        let responds: BOOL = msg_send![ns_window, respondsToSelector: sel_set];
        if responds == YES {
            let _: () = msg_send![ns_window, _setCornerRadius: radius];
        } else {
            let key: id = NSString::alloc(nil).init_str("cornerRadius");
            let num: id = msg_send![class!(NSNumber), numberWithDouble: radius];
            let _: () = msg_send![ns_window, setValue: num forKey: key];
            let _: () = msg_send![key, release];
        }
        let _: () = msg_send![ns_window, invalidateShadow];
        BUSY.with(|busy| busy.set(false));
    }

    fn reapply_window(window: id) {
        if window == nil {
            return;
        }
        let saved = radii().get(&(window as usize)).copied();
        if let Some(radius) = saved {
            unsafe { set_radius(window, radius) }
        }
    }

    extern "C" fn on_note(_this: &Object, _: Sel, note: id) {
        let name: id = unsafe { msg_send![note, name] };
        let obj: id = unsafe { msg_send![note, object] };
        if ns_eq(name, "NSWindowWillCloseNotification") {
            if obj != nil {
                radii().remove(&(obj as usize));
            }
            return;
        }
        // The green button does not emit DOM `fullscreenchange`. AppKit
        // also rewrites the system radius across fullscreen (0 while
        // fullscreen, 16 after exit), so put this window's value back.
        if ns_eq(name, "NSWindowDidEnterFullScreenNotification")
            || ns_eq(name, "NSWindowDidExitFullScreenNotification")
        {
            reapply_window(obj);
        }
    }

    fn ns_eq(s: id, expected: &str) -> bool {
        if s == nil {
            return false;
        }
        unsafe { NSString::isEqualToString(s, expected) }
    }

    fn ensure_observers() {
        static ONCE: OnceLock<()> = OnceLock::new();
        ONCE.get_or_init(|| unsafe {
            let owner = tracker();
            if owner == nil {
                return;
            }
            let center: id = msg_send![class!(NSNotificationCenter), defaultCenter];
            for name in [
                "NSWindowDidEnterFullScreenNotification",
                "NSWindowDidExitFullScreenNotification",
                "NSWindowWillCloseNotification",
            ] {
                let nsname = NSString::alloc(nil).init_str(name);
                let _: () = msg_send![
                    center,
                    addObserver: owner
                    selector: sel!(k2OnCornerNote:)
                    name: nsname
                    object: nil
                ];
                let _: () = msg_send![nsname, release];
            }
        });
    }

    fn tracker_class() -> Option<&'static Class> {
        static SLOT: OnceLock<usize> = OnceLock::new();
        let ptr = *SLOT.get_or_init(|| unsafe {
            let Some(mut decl) = ClassDecl::new("K2WindowCornerObserver", class!(NSObject)) else {
                return Class::get("K2WindowCornerObserver")
                    .map(|c| c as *const Class as usize)
                    .unwrap_or(0);
            };
            decl.add_method(
                sel!(k2OnCornerNote:),
                on_note as extern "C" fn(&Object, Sel, id),
            );
            decl.register() as *const Class as usize
        });
        if ptr == 0 {
            None
        } else {
            Some(unsafe { &*(ptr as *const Class) })
        }
    }

    fn tracker() -> id {
        static SLOT: OnceLock<usize> = OnceLock::new();
        let ptr = *SLOT.get_or_init(|| {
            let Some(cls) = tracker_class() else {
                return 0;
            };
            unsafe {
                let obj: id = msg_send![cls, alloc];
                let obj: id = msg_send![obj, init];
                obj as usize
            }
        });
        ptr as id
    }
}

/// Set the invoking window's corner radius. No-op off macOS.
///
/// `radius` is the renderer's decision: 0.5 for `square`, 0 otherwise.
/// Not an argument of `set_traffic_light_inset`.
#[tauri::command]
pub fn set_window_corner_radius(window: tauri::Window, radius: f64) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let w = window.clone();
        window
            .run_on_main_thread(move || imp::apply(&w, radius))
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (window, radius);
        Ok(())
    }
}
