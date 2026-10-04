//! macOS traffic-light repositioning for the Style System's floating-chrome
//! styles (Glass/Bezel/spacious presets): when the chrome is inset from the
//! window edge, the close/minimize/zoom buttons must move down-right with it.
//!
//! AppKit resets standard-button frames whenever it re-lays out the title
//! bar (resize, screen or backing-scale change, wake, appearance change,
//! fullscreen, setTitle). Rust owns the re-apply: `ensure_observers` routes
//! those notifications back to `position_ns`, once now and once on the next
//! run-loop turn so ours lands after AppKit's layout pass. The renderer's
//! resize / fullscreen / setTitle hooks (stores/style.ts) stay as a backup.
//! `extra_x`/`extra_y` are logical px offsets from the system default
//! position; (0, 0) at zoom 1 restores the default exactly (defaults are
//! captured on first call, before any modification). The math is the pure
//! [`geometry::place`].
//!
//! When `square` is set, the same call paints those three buttons as sharp
//! squares (style id `square` only). Any other style restores the system
//! cells so AppKit draws circles again. The square is centered in the
//! button frame; frame size and origin gaps are not changed.

/// Pure stoplight geometry. No AppKit, so it is unit-tested on every host.
///
/// Coordinates are AppKit's: y grows upward. The title-bar container's top
/// is pinned to the window top, and the button y is measured from the
/// container's bottom edge.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) mod geometry {
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Rect {
        pub x: f64,
        pub y: f64,
        pub w: f64,
        pub h: f64,
    }

    /// System geometry, captured once before K2 moves anything.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Defaults {
        /// Close button origin x.
        pub button_x: f64,
        /// Close button origin y inside the title-bar container.
        pub button_y: f64,
        /// Close button height.
        pub button_h: f64,
        /// Distance from one button's x to the next one's.
        pub spacing: f64,
        /// Title-bar container height.
        pub titlebar_h: f64,
    }

    /// Title-bar container frame plus close / minimize / zoom frames.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Frames {
        pub container: Rect,
        pub buttons: [Rect; 3],
    }

    /// What the renderer asked for: logical px from the system position,
    /// and the app zoom (`document.documentElement.style.zoom`, 1 = 100%).
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Offset {
        pub x: f64,
        pub y: f64,
        pub zoom: f64,
    }

    fn finite_or(v: f64, fallback: f64) -> f64 {
        if v.is_finite() {
            v
        } else {
            fallback
        }
    }

    /// The zoom the renderer sent, or 1 when it is non-finite or <= 0.
    pub fn sane_zoom(zoom: f64) -> f64 {
        if zoom.is_finite() && zoom > 0.0 {
            zoom
        } else {
            1.0
        }
    }

    /// How far the title-bar container grows below its system height.
    ///
    /// At 100% this is `offset.y`. The renderer's top bar is centered on
    /// the buttons there, so the button center sits `c0 + y` below the
    /// window top, where `c0` is the system center. CSS zoom scales the DOM
    /// top bar but not the native buttons, so at zoom `z` the bar center is
    /// `z * (c0 + y)`; the container grows to put the button center there.
    /// Never so little that the button top leaves the window (zoom < 1).
    pub fn effective_y(d: Defaults, offset: Offset) -> f64 {
        let y = finite_or(offset.y, 0.0);
        let z = sane_zoom(offset.zoom);
        if z == 1.0 {
            return y;
        }
        let c0 = d.titlebar_h - d.button_y - d.button_h / 2.0;
        let grown = z * (c0 + y) - c0;
        let floor = d.button_y + d.button_h - d.titlebar_h;
        grown.max(floor)
    }

    /// The single absolute placement. Every output comes from `d`, the
    /// window height and `offset`; from `current` only the container's x and
    /// width and each button's size are kept. Feeding the result back in as
    /// `current` returns it unchanged, so a re-apply never accumulates.
    pub fn place(d: Defaults, current: Frames, window_h: f64, offset: Offset) -> Frames {
        let extra_y = effective_y(d, offset);
        let extra_x = finite_or(offset.x, 0.0);
        let container_h = d.titlebar_h + extra_y;
        let container = Rect {
            x: current.container.x,
            y: window_h - container_h,
            w: current.container.w,
            h: container_h,
        };
        let mut buttons = current.buttons;
        for (i, b) in buttons.iter_mut().enumerate() {
            b.x = d.button_x + extra_x + (i as f64) * d.spacing;
            b.y = d.button_y;
        }
        Frames { container, buttons }
    }

    /// Close-button center, measured down from the window top.
    #[cfg(test)]
    pub fn button_center_from_top(frames: Frames) -> f64 {
        let b = frames.buttons[0];
        frames.container.h - b.y - b.h / 2.0
    }
}

#[cfg(target_os = "macos")]
// cocoa/objc are deprecated in favor of objc2 but frozen as house deps for
// now (see the objc2-migration note in Cargo.toml); same allow as
// commands/permissions.rs. The positioning path and the observer plumbing
// send through objc2 (`C-unwind`), so an ObjC exception there reaches the
// `guarded` catch instead of a no-unwind call site. The square-paint
// helpers still use the `objc` crate; see `guarded`.
#[allow(deprecated)]
mod imp {
    use std::collections::HashMap;
    use std::ffi::{c_char, c_void, CStr};
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::sync::{Mutex, OnceLock};

    use cocoa::appkit::{NSWindow, NSWindowButton};
    use cocoa::base::{id, nil, BOOL, NO, YES};
    use cocoa::foundation::{NSPoint, NSRect, NSSize, NSString};
    use objc::declare::ClassDecl;
    use objc::runtime::{Class, Object, Sel};
    use objc::{class, msg_send, sel, sel_impl};
    use objc2::runtime::{AnyClass, AnyObject};

    use super::geometry::{self, Defaults, Frames, Offset, Rect};

    #[derive(Clone, Copy)]
    struct SavedInset {
        x: f64,
        y: f64,
        zoom: f64,
        square: bool,
    }

    #[derive(Clone, Copy)]
    enum Role {
        Close,
        Mini,
        Zoom,
    }

    static DEFAULTS: OnceLock<Defaults> = OnceLock::new();
    static INSETS: OnceLock<Mutex<HashMap<usize, SavedInset>>> = OnceLock::new();

    const OBJC_ASSOCIATION_ASSIGN: usize = 0;
    const OBJC_ASSOCIATION_RETAIN_NONATOMIC: usize = 1;

    static ORIGINAL_CELL: u8 = 0;
    static ORIGINAL_TARGET: u8 = 0;
    static ORIGINAL_ACTION: u8 = 0;

    #[link(name = "AppKit", kind = "framework")]
    extern "C" {
        fn NSMouseInRect(aPoint: NSPoint, aRect: NSRect, flipped: BOOL) -> BOOL;
    }

    #[link(name = "objc", kind = "dylib")]
    extern "C" {
        fn objc_setAssociatedObject(object: id, key: *const c_void, value: id, policy: usize);
        fn objc_getAssociatedObject(object: id, key: *const c_void) -> id;
        static _NSConcreteGlobalBlock: c_void;
    }

    #[repr(C)]
    struct BlockDescriptor {
        reserved: usize,
        size: usize,
        // Present when BLOCK_HAS_SIGNATURE is set. `@@?@` is `id (^)(id)`.
        signature: *const c_char,
        layout: *const c_char,
    }

    #[repr(C)]
    struct BlockLiteral {
        isa: *const c_void,
        flags: i32,
        reserved: i32,
        invoke: extern "C" fn(*mut BlockLiteral, id) -> id,
        descriptor: *const BlockDescriptor,
    }

    fn insets() -> std::sync::MutexGuard<'static, HashMap<usize, SavedInset>> {
        INSETS
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    // ── fault guard ──────────────────────────────────────────────────
    // Same nesting as tao's `guard_send_event`
    // (third_party/tao/src/platform_impl/macos/event_fault.rs):
    // `catch_unwind` outside, `objc2::exception::catch` inside. An ObjC
    // exception is caught before any Rust `catch_unwind` frame sees it; a
    // Rust panic passes the ObjC catch and stops at `catch_unwind`. Nothing
    // unwinds into an `extern "C"` IMP. A fault drops that one re-apply and
    // writes one line to ~/.k2/client-event-faults.log.
    //
    // Limit: an ObjC exception raised inside an `objc`-crate `msg_send!`
    // (plain `extern "C"`: the square-paint helpers) cannot unwind through
    // that call site. The positioning path uses objc2 sends for that reason.
    pub(super) fn guarded(source: &'static str, body: impl FnOnce()) {
        guarded_with(source, body, crate::event_faults::record_line);
    }

    /// `guarded` with the reporter passed in. `report` gets one line per
    /// fault and runs inside its own catch.
    pub(super) fn guarded_with(
        source: &'static str,
        body: impl FnOnce(),
        report: impl FnOnce(&str),
    ) {
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            objc2::exception::catch(AssertUnwindSafe(body))
        }));
        let detail = match outcome {
            Ok(Ok(())) => return,
            Ok(Err(exception)) => {
                let detail = quietly(|| match exception.as_deref() {
                    Some(e) => format!("objc-exception: {e:?}"),
                    None => "objc-exception: nil".to_owned(),
                });
                quietly(move || drop(exception));
                detail
            }
            Err(payload) => {
                let detail = if let Some(s) = payload.downcast_ref::<&str>() {
                    format!("panic: {s}")
                } else if let Some(s) = payload.downcast_ref::<String>() {
                    format!("panic: {s}")
                } else {
                    "panic: <non-string payload>".to_owned()
                };
                if let Err(again) = catch_unwind(AssertUnwindSafe(move || drop(payload))) {
                    std::mem::forget(again);
                }
                Some(detail)
            }
        };
        let detail = detail.unwrap_or_else(|| "fault while describing the fault".to_owned());
        quietly(|| report(&format!("traffic-lights {source} {detail}")));
    }

    /// Raise an `NSException` through an objc2 send.
    #[cfg(test)]
    pub(super) unsafe fn raise_for_test() {
        let reason = ns_string(c"k2-traffic-light-test-exception");
        let Some(exc) = cls(c"NSException") else {
            panic!("NSException class missing");
        };
        let name = ns_string(c"NSRangeException");
        let null: Obj = std::ptr::null_mut();
        let e: Obj = objc2::msg_send![exc, exceptionWithName: name, reason: reason, userInfo: null];
        let _: () = objc2::msg_send![e, raise];
    }

    /// Run `f` so that it can never unwind. `None` when it raised or panicked.
    fn quietly<T>(f: impl FnOnce() -> T) -> Option<T> {
        match catch_unwind(AssertUnwindSafe(|| {
            objc2::exception::catch(AssertUnwindSafe(f))
        })) {
            Ok(Ok(v)) => Some(v),
            Ok(Err(exception)) => {
                std::mem::forget(exception);
                None
            }
            Err(payload) => {
                std::mem::forget(payload);
                None
            }
        }
    }

    // ── objc2 sends for the positioning path ─────────────────────────
    type Obj = *mut AnyObject;

    fn o(x: id) -> Obj {
        x as Obj
    }

    fn cls(name: &CStr) -> Option<&'static AnyClass> {
        AnyClass::get(name)
    }

    unsafe fn frame_of(view: Obj) -> Rect {
        let r: objc2_foundation::NSRect = objc2::msg_send![view, frame];
        Rect {
            x: r.origin.x,
            y: r.origin.y,
            w: r.size.width,
            h: r.size.height,
        }
    }

    unsafe fn set_frame(view: Obj, r: Rect) {
        let ns = objc2_foundation::NSRect::new(
            objc2_foundation::NSPoint::new(r.x, r.y),
            objc2_foundation::NSSize::new(r.w, r.h),
        );
        let _: () = objc2::msg_send![view, setFrame: ns];
    }

    unsafe fn set_frame_origin(view: Obj, x: f64, y: f64) {
        let p = objc2_foundation::NSPoint::new(x, y);
        let _: () = objc2::msg_send![view, setFrameOrigin: p];
    }

    /// `NSWindowButton`: close 0, miniaturize 1, zoom 2.
    unsafe fn std_button(window: Obj, kind: usize) -> Obj {
        objc2::msg_send![window, standardWindowButton: kind]
    }

    unsafe fn superview(view: Obj) -> Obj {
        objc2::msg_send![view, superview]
    }

    unsafe fn ns_string(s: &CStr) -> Obj {
        let Some(c) = cls(c"NSString") else {
            return std::ptr::null_mut();
        };
        objc2::msg_send![c, stringWithUTF8String: s.as_ptr()]
    }

    unsafe fn is_main_thread() -> bool {
        let Some(c) = cls(c"NSThread") else {
            return false;
        };
        objc2::msg_send![c, isMainThread]
    }

    pub unsafe fn position(
        window: &tauri::Window,
        extra_x: f64,
        extra_y: f64,
        zoom: f64,
        square: bool,
    ) {
        let Ok(handle) = window.ns_window() else {
            return;
        };
        position_ns(handle as id, extra_x, extra_y, zoom, square);
        schedule_next_turn();
    }

    /// The one absolute positioning function. The renderer command, every
    /// observer and the next-turn re-apply all go through here.
    unsafe fn position_ns(ns_window: id, extra_x: f64, extra_y: f64, zoom: f64, square: bool) {
        if ns_window == nil {
            return;
        }
        let w = o(ns_window);
        let close = std_button(w, 0);
        let mini = std_button(w, 1);
        let zoom_btn = std_button(w, 2);
        if close.is_null() || mini.is_null() || zoom_btn.is_null() {
            return;
        }

        let title_bar_container: Obj = {
            let sv = superview(close);
            if sv.is_null() {
                return;
            }
            superview(sv)
        };
        if title_bar_container.is_null() {
            return;
        }

        // System-default geometry, captured before the first modification so
        // (0, 0) can restore it byte-exactly when switching back to Square.
        let defaults = *DEFAULTS.get_or_init(|| {
            let close_rect = frame_of(close);
            let mini_rect = frame_of(mini);
            let tb_rect = frame_of(title_bar_container);
            Defaults {
                button_x: close_rect.x,
                button_y: close_rect.y,
                button_h: close_rect.h,
                spacing: mini_rect.x - close_rect.x,
                titlebar_h: tb_rect.h,
            }
        });

        // Grow the title-bar container downward from the top edge. The buttons
        // are pinned back to the captured y so a later call can move them up
        // again; leaving AppKit's autoresize in place kept them at the first drop.
        // `extra_y` already includes the renderer's 3px nudge. Do not add it
        // to origin.y a second time.
        let window_h = frame_of(w).h;
        let current = Frames {
            container: frame_of(title_bar_container),
            buttons: [frame_of(close), frame_of(mini), frame_of(zoom_btn)],
        };
        let offset = Offset {
            x: extra_x,
            y: extra_y,
            zoom,
        };
        let next = geometry::place(defaults, current, window_h, offset);
        set_frame(title_bar_container, next.container);
        for (btn, r) in [close, mini, zoom_btn].into_iter().zip(next.buttons) {
            set_frame_origin(btn, r.x, r.y);
        }

        insets().insert(
            ns_window as usize,
            SavedInset {
                x: extra_x,
                y: extra_y,
                zoom,
                square,
            },
        );
        apply_paint(ns_window, square);
        ensure_observers();
    }

    fn standard_button(window: id, kind: NSWindowButton) -> id {
        if window == nil {
            return nil;
        }
        unsafe { window.standardWindowButton_(kind) }
    }

    fn apply_paint(window: id, square: bool) {
        for kind in [
            NSWindowButton::NSWindowCloseButton,
            NSWindowButton::NSWindowMiniaturizeButton,
            NSWindowButton::NSWindowZoomButton,
        ] {
            let btn = standard_button(window, kind);
            if btn == nil {
                continue;
            }
            if square {
                install_square(btn);
            } else {
                restore_round(btn);
            }
        }
    }

    /// `_NSThemeWidgetCell` makes the button update its layer and ignore a
    /// custom cell's frame. A plain `NSButtonCell` clears `wantsUpdateLayer`
    /// (measured on macOS 27), so `drawWithFrame:inView:` replaces the circle.
    fn install_square(btn: id) {
        if cell_is_ours(btn_cell(btn)) {
            ensure_tracking(btn);
            let _: () = unsafe { msg_send![btn, setNeedsDisplay: YES] };
            return;
        }
        let old_cell = btn_cell(btn);
        let old_target: id = unsafe { msg_send![btn, target] };
        let old_action: Sel = unsafe { msg_send![btn, action] };
        // setCell: releases the previous cell and clears target/action.
        // Retain the system cell first, then put the originals back so Tauri's
        // close path stays on the button's `_close:` (not a stand-in).
        if old_cell != nil {
            unsafe {
                objc_setAssociatedObject(
                    btn,
                    std::ptr::addr_of!(ORIGINAL_CELL) as *const c_void,
                    old_cell,
                    OBJC_ASSOCIATION_RETAIN_NONATOMIC,
                );
            }
        }
        unsafe {
            objc_setAssociatedObject(
                btn,
                std::ptr::addr_of!(ORIGINAL_TARGET) as *const c_void,
                old_target,
                OBJC_ASSOCIATION_ASSIGN,
            );
        }
        if let Some(name) = action_name(old_action) {
            let ns = unsafe { NSString::alloc(nil).init_str(&name) };
            unsafe {
                objc_setAssociatedObject(
                    btn,
                    std::ptr::addr_of!(ORIGINAL_ACTION) as *const c_void,
                    ns,
                    OBJC_ASSOCIATION_RETAIN_NONATOMIC,
                );
                let _: () = msg_send![ns, release];
            }
        }

        let Some(cls) = our_cell_class() else {
            return;
        };
        let cell: id = unsafe { msg_send![cls, alloc] };
        let blank = ns_utf8("");
        let cell: id = unsafe { msg_send![cell, initTextCell: blank] };
        if cell == nil {
            return;
        }
        let _: () = unsafe { msg_send![cell, setBordered: NO] };
        let _: () = unsafe { msg_send![btn, setCell: cell] };
        let _: () = unsafe { msg_send![cell, release] };
        let _: () = unsafe { msg_send![btn, setTarget: old_target] };
        if !old_action.as_ptr().is_null() {
            let _: () = unsafe { msg_send![btn, setAction: old_action] };
        }
        ensure_tracking(btn);
        let _: () = unsafe { msg_send![btn, setNeedsDisplay: YES] };
    }

    fn restore_round(btn: id) {
        if !cell_is_ours(btn_cell(btn)) {
            remove_tracking(btn);
            return;
        }
        let original: id = unsafe {
            objc_getAssociatedObject(btn, std::ptr::addr_of!(ORIGINAL_CELL) as *const c_void)
        };
        let target: id = unsafe {
            objc_getAssociatedObject(btn, std::ptr::addr_of!(ORIGINAL_TARGET) as *const c_void)
        };
        let action_ns: id = unsafe {
            objc_getAssociatedObject(btn, std::ptr::addr_of!(ORIGINAL_ACTION) as *const c_void)
        };
        if original != nil {
            let _: () = unsafe { msg_send![btn, setCell: original] };
        }
        let _: () = unsafe { msg_send![btn, setTarget: target] };
        if let Some(action) = sel_from_nsstring(action_ns) {
            let _: () = unsafe { msg_send![btn, setAction: action] };
        }
        unsafe {
            objc_setAssociatedObject(
                btn,
                std::ptr::addr_of!(ORIGINAL_CELL) as *const c_void,
                nil,
                OBJC_ASSOCIATION_RETAIN_NONATOMIC,
            );
            objc_setAssociatedObject(
                btn,
                std::ptr::addr_of!(ORIGINAL_TARGET) as *const c_void,
                nil,
                OBJC_ASSOCIATION_ASSIGN,
            );
            objc_setAssociatedObject(
                btn,
                std::ptr::addr_of!(ORIGINAL_ACTION) as *const c_void,
                nil,
                OBJC_ASSOCIATION_RETAIN_NONATOMIC,
            );
        }
        remove_tracking(btn);
        let _: () = unsafe { msg_send![btn, setNeedsDisplay: YES] };
    }

    fn btn_cell(btn: id) -> id {
        if btn == nil {
            return nil;
        }
        unsafe { msg_send![btn, cell] }
    }

    fn cell_is_ours(cell: id) -> bool {
        if cell == nil {
            return false;
        }
        let Some(cls) = our_cell_class() else {
            return false;
        };
        let yes: BOOL = unsafe { msg_send![cell, isKindOfClass: cls] };
        yes == YES
    }

    fn action_name(action: Sel) -> Option<String> {
        if action.as_ptr().is_null() {
            return None;
        }
        Some(action.name().to_string())
    }

    fn sel_from_nsstring(s: id) -> Option<Sel> {
        if s == nil {
            return None;
        }
        let ptr: *const c_char = unsafe { msg_send![s, UTF8String] };
        if ptr.is_null() {
            return None;
        }
        let name = unsafe { CStr::from_ptr(ptr) }.to_str().ok()?;
        if name.is_empty() {
            return None;
        }
        Some(Sel::register(name))
    }

    fn ns_utf8(s: &str) -> id {
        let c = std::ffi::CString::new(s).unwrap_or_else(|_| std::ffi::CString::new("").unwrap());
        unsafe { msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()] }
    }

    fn class_slot(name: &str, build: impl FnOnce() -> *const Class) -> Option<&'static Class> {
        static CELL: OnceLock<usize> = OnceLock::new();
        static TRACKER: OnceLock<usize> = OnceLock::new();
        let slot = if name == "K2SquareTrafficLightCell" {
            &CELL
        } else {
            &TRACKER
        };
        let ptr = *slot.get_or_init(|| build() as usize);
        if ptr == 0 {
            None
        } else {
            Some(unsafe { &*(ptr as *const Class) })
        }
    }

    fn our_cell_class() -> Option<&'static Class> {
        class_slot("K2SquareTrafficLightCell", || unsafe {
            let Some(mut decl) = ClassDecl::new("K2SquareTrafficLightCell", class!(NSButtonCell))
            else {
                return Class::get("K2SquareTrafficLightCell")
                    .map(|c| c as *const Class)
                    .unwrap_or(std::ptr::null());
            };
            decl.add_method(
                sel!(drawWithFrame:inView:),
                draw_with_frame as extern "C" fn(&Object, Sel, NSRect, id),
            );
            decl.add_method(
                sel!(highlight:withFrame:inView:),
                highlight_with_frame as extern "C" fn(&Object, Sel, BOOL, NSRect, id),
            );
            decl.add_method(
                sel!(wantsUpdateLayerInView:),
                wants_update_layer as extern "C" fn(&Object, Sel, id) -> BOOL,
            );
            decl.add_method(
                sel!(accessibilitySubrole),
                ax_subrole as extern "C" fn(&Object, Sel) -> id,
            );
            decl.add_method(
                sel!(accessibilityLabel),
                ax_label as extern "C" fn(&Object, Sel) -> id,
            );
            // A title change asks the close button's cell for theme-widget
            // methods (`setEditedFlag:`, `setTemporarilyDisabled:`, …).
            // NSButtonCell does not have them. Send those to the system cell
            // we kept on the button. Methods this class implements stay here.
            decl.add_method(
                sel!(forwardingTargetForSelector:),
                forwarding_target as extern "C" fn(&Object, Sel, Sel) -> id,
            );
            decl.register() as *const Class
        })
    }

    fn tracker_class() -> Option<&'static Class> {
        class_slot("K2SquareTrafficObserver", || unsafe {
            let Some(mut decl) = ClassDecl::new("K2SquareTrafficObserver", class!(NSObject)) else {
                return Class::get("K2SquareTrafficObserver")
                    .map(|c| c as *const Class)
                    .unwrap_or(std::ptr::null());
            };
            decl.add_method(sel!(k2OnNote:), on_note as extern "C" fn(&Object, Sel, id));
            decl.add_method(
                sel!(k2ReapplyAll:),
                on_reapply_all as extern "C" fn(&Object, Sel, id),
            );
            decl.add_method(
                sel!(mouseEntered:),
                mouse_hover as extern "C" fn(&Object, Sel, id),
            );
            decl.add_method(
                sel!(mouseExited:),
                mouse_hover as extern "C" fn(&Object, Sel, id),
            );
            decl.register() as *const Class
        })
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

    extern "C" fn forwarding_target(this: &Object, _: Sel, asked: Sel) -> id {
        if asked.as_ptr().is_null() {
            return nil;
        }
        unsafe {
            let view: id = msg_send![this, controlView];
            if view == nil {
                return nil;
            }
            let orig: id =
                objc_getAssociatedObject(view, std::ptr::addr_of!(ORIGINAL_CELL) as *const c_void);
            if orig == nil {
                return nil;
            }
            let answers: BOOL = msg_send![orig, respondsToSelector: asked];
            if answers == YES {
                orig
            } else {
                nil
            }
        }
    }

    extern "C" fn draw_with_frame(_this: &Object, _: Sel, frame: NSRect, view: id) {
        unsafe { draw_square(frame, view) }
    }

    extern "C" fn highlight_with_frame(
        this: &Object,
        _: Sel,
        flag: BOOL,
        _frame: NSRect,
        view: id,
    ) {
        // setHighlighted: can call back into highlight:withFrame:inView:.
        // The re-entry would stack-overflow on press; the flag is already set.
        thread_local! {
            static BUSY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        }
        if BUSY.with(|busy| busy.replace(true)) {
            return;
        }
        let cell = this as *const Object as id;
        unsafe {
            let _: () = msg_send![cell, setHighlighted: flag];
            if view != nil {
                let _: () = msg_send![view, setNeedsDisplay: YES];
            }
        }
        BUSY.with(|busy| busy.set(false));
    }

    extern "C" fn wants_update_layer(_this: &Object, _: Sel, _view: id) -> BOOL {
        NO
    }

    extern "C" fn ax_subrole(this: &Object, _: Sel) -> id {
        let cell = this as *const Object as id;
        let view: id = unsafe { msg_send![cell, controlView] };
        ns_utf8(subrole(role_of(view)))
    }

    extern "C" fn ax_label(this: &Object, _: Sel) -> id {
        let cell = this as *const Object as id;
        let view: id = unsafe { msg_send![cell, controlView] };
        ns_utf8(label(role_of(view)))
    }

    extern "C" fn mouse_hover(_this: &Object, _: Sel, event: id) {
        guarded("mouse_hover", || {
            let window: id = unsafe { msg_send![event, window] };
            if window_has_square_paint(window) {
                mark_buttons_dirty(window);
            }
        });
    }

    /// What a notification asks the observer to do.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) enum NoteAction {
        /// Forget the window's saved inset.
        Forget,
        /// Re-apply the window that posted it, now and next turn.
        ReapplyWindow,
        /// Re-apply, then repaint (key change also flips the square colors).
        ReapplyWindowAndRepaint,
        /// Repaint only: the title bar was not re-laid out.
        RepaintWindow,
        /// No window object: re-apply every K2 window, now and next turn.
        ReapplyAll,
        /// App active / inactive: repaint every square window.
        RepaintAll,
    }

    /// Window notifications on the default center. `object: nil`, so they
    /// fire for every K2 window (main, `window-*`, Focus) with no
    /// per-window observer to remove on close.
    pub(super) const WINDOW_NOTES: &[&CStr] = &[
        c"NSWindowDidEnterFullScreenNotification",
        c"NSWindowDidExitFullScreenNotification",
        c"NSWindowDidResizeNotification",
        c"NSWindowDidEndLiveResizeNotification",
        c"NSWindowDidChangeScreenNotification",
        c"NSWindowDidChangeBackingPropertiesNotification",
        c"NSWindowDidDeminiaturizeNotification",
        c"NSWindowDidBecomeKeyNotification",
        c"NSWindowDidBecomeMainNotification",
        c"NSWindowDidResignKeyNotification",
        c"NSWindowWillCloseNotification",
        c"NSApplicationDidBecomeActiveNotification",
        c"NSApplicationDidResignActiveNotification",
        c"NSApplicationDidChangeScreenParametersNotification",
    ];
    /// On `NSWorkspace.sharedWorkspace.notificationCenter`.
    pub(super) const WORKSPACE_NOTES: &[&CStr] = &[
        c"NSWorkspaceDidWakeNotification",
        c"NSWorkspaceScreensDidWakeNotification",
    ];
    /// On `NSDistributedNotificationCenter`.
    pub(super) const DISTRIBUTED_NOTES: &[&CStr] = &[c"AppleInterfaceThemeChangedNotification"];

    pub(super) fn note_action(name: &str) -> Option<NoteAction> {
        Some(match name {
            "NSWindowWillCloseNotification" => NoteAction::Forget,
            "NSWindowDidEnterFullScreenNotification"
            | "NSWindowDidExitFullScreenNotification"
            | "NSWindowDidResizeNotification"
            | "NSWindowDidEndLiveResizeNotification"
            | "NSWindowDidChangeScreenNotification"
            | "NSWindowDidChangeBackingPropertiesNotification"
            | "NSWindowDidDeminiaturizeNotification"
            | "NSWindowDidBecomeMainNotification" => NoteAction::ReapplyWindow,
            "NSWindowDidBecomeKeyNotification" => NoteAction::ReapplyWindowAndRepaint,
            "NSWindowDidResignKeyNotification" => NoteAction::RepaintWindow,
            "NSApplicationDidChangeScreenParametersNotification"
            | "NSWorkspaceDidWakeNotification"
            | "NSWorkspaceScreensDidWakeNotification"
            | "AppleInterfaceThemeChangedNotification" => NoteAction::ReapplyAll,
            "NSApplicationDidBecomeActiveNotification"
            | "NSApplicationDidResignActiveNotification" => NoteAction::RepaintAll,
            _ => return None,
        })
    }

    extern "C" fn on_note(_this: &Object, _: Sel, note: id) {
        guarded("on_note", || unsafe { handle_note(o(note)) });
    }

    extern "C" fn on_reapply_all(_this: &Object, _: Sel, _arg: id) {
        guarded("next_turn", || unsafe { reapply_all() });
    }

    unsafe fn handle_note(note: Obj) {
        if note.is_null() {
            return;
        }
        let name: Obj = objc2::msg_send![note, name];
        let obj: Obj = objc2::msg_send![note, object];
        let Some(name) = utf8(name) else {
            return;
        };
        let Some(action) = note_action(&name) else {
            return;
        };
        // The workspace and distributed centers can post off the main
        // thread. AppKit views are main-thread only: hop, do not touch.
        if !is_main_thread() {
            if matches!(action, NoteAction::Forget) {
                // Never expected off-main; still nothing to touch.
                return;
            }
            let owner = o(tracker());
            if owner.is_null() {
                return;
            }
            let null: Obj = std::ptr::null_mut();
            let _: () = objc2::msg_send![
                owner,
                performSelectorOnMainThread: objc2::sel!(k2ReapplyAll:),
                withObject: null,
                waitUntilDone: false
            ];
            return;
        }
        let window = obj as id;
        match action {
            NoteAction::Forget => {
                if window != nil {
                    insets().remove(&(window as usize));
                }
            }
            NoteAction::ReapplyWindow => {
                reapply_window(window);
                schedule_next_turn();
            }
            NoteAction::ReapplyWindowAndRepaint => {
                reapply_window(window);
                if window_has_square_paint(window) {
                    mark_buttons_dirty(window);
                }
                schedule_next_turn();
            }
            NoteAction::RepaintWindow => {
                if window_has_square_paint(window) {
                    mark_buttons_dirty(window);
                }
            }
            NoteAction::ReapplyAll => {
                reapply_all();
                schedule_next_turn();
            }
            NoteAction::RepaintAll => redraw_square_windows(),
        }
    }

    unsafe fn utf8(s: Obj) -> Option<String> {
        if s.is_null() {
            return None;
        }
        let ptr: *const c_char = objc2::msg_send![s, UTF8String];
        if ptr.is_null() {
            return None;
        }
        Some(CStr::from_ptr(ptr).to_string_lossy().into_owned())
    }

    /// One more re-apply of every K2 window on the next run-loop turn, after
    /// AppKit's own layout pass. Common modes, so it also runs during a live
    /// resize (event-tracking mode). A burst coalesces into one.
    unsafe fn schedule_next_turn() {
        let owner = o(tracker());
        let Some(nsobject) = cls(c"NSObject") else {
            return;
        };
        let Some(nsarray) = cls(c"NSArray") else {
            return;
        };
        if owner.is_null() {
            return;
        }
        let null: Obj = std::ptr::null_mut();
        let sel = objc2::sel!(k2ReapplyAll:);
        let _: () = objc2::msg_send![
            nsobject,
            cancelPreviousPerformRequestsWithTarget: owner,
            selector: sel,
            object: null
        ];
        // NSRunLoopCommonModes == kCFRunLoopCommonModes.
        let common = ns_string(c"kCFRunLoopCommonModes");
        if common.is_null() {
            return;
        }
        let modes: Obj = objc2::msg_send![nsarray, arrayWithObject: common];
        if modes.is_null() {
            return;
        }
        let _: () = objc2::msg_send![
            owner,
            performSelector: sel,
            withObject: null,
            afterDelay: 0.0f64,
            inModes: modes
        ];
    }

    extern "C" fn flags_invoke(_block: *mut BlockLiteral, event: id) -> id {
        guarded("flags_changed", redraw_square_windows);
        event
    }

    fn reapply_window(window: id) {
        if window == nil {
            return;
        }
        let saved = insets().get(&(window as usize)).copied();
        if let Some(saved) = saved {
            unsafe { position_ns(window, saved.x, saved.y, saved.zoom, saved.square) };
        }
    }

    /// Re-apply each open window that K2 has positioned. Walks
    /// `NSApp.windows` rather than the saved keys, so a stale key can never
    /// be messaged.
    unsafe fn reapply_all() {
        let Some(app_cls) = cls(c"NSApplication") else {
            return;
        };
        let app: Obj = objc2::msg_send![app_cls, sharedApplication];
        if app.is_null() {
            return;
        }
        let windows: Obj = objc2::msg_send![app, windows];
        if windows.is_null() {
            return;
        }
        let count: usize = objc2::msg_send![windows, count];
        for i in 0..count {
            let window: Obj = objc2::msg_send![windows, objectAtIndex: i];
            reapply_window(window as id);
        }
    }

    fn window_has_square_paint(window: id) -> bool {
        cell_is_ours(btn_cell(standard_button(
            window,
            NSWindowButton::NSWindowCloseButton,
        )))
    }

    fn mark_buttons_dirty(window: id) {
        for kind in [
            NSWindowButton::NSWindowCloseButton,
            NSWindowButton::NSWindowMiniaturizeButton,
            NSWindowButton::NSWindowZoomButton,
        ] {
            let btn = standard_button(window, kind);
            if btn != nil {
                let _: () = unsafe { msg_send![btn, setNeedsDisplay: YES] };
            }
        }
    }

    fn redraw_square_windows() {
        let app: id = unsafe { msg_send![class!(NSApplication), sharedApplication] };
        if app == nil {
            return;
        }
        let windows: id = unsafe { msg_send![app, windows] };
        if windows == nil {
            return;
        }
        let count: usize = unsafe { msg_send![windows, count] };
        for i in 0..count {
            let window: id = unsafe { msg_send![windows, objectAtIndex: i] };
            if window_has_square_paint(window) {
                mark_buttons_dirty(window);
            }
        }
    }

    fn ensure_observers() {
        static ONCE: OnceLock<()> = OnceLock::new();
        // One app-lifetime observer object for every window and center, so a
        // closing window leaves nothing registered behind it (WillClose only
        // drops its saved inset).
        ONCE.get_or_init(|| unsafe {
            let owner = o(tracker());
            if owner.is_null() {
                return;
            }
            let default_center: Obj = match cls(c"NSNotificationCenter") {
                Some(c) => objc2::msg_send![c, defaultCenter],
                None => std::ptr::null_mut(),
            };
            let workspace_center: Obj = match cls(c"NSWorkspace") {
                Some(c) => {
                    let ws: Obj = objc2::msg_send![c, sharedWorkspace];
                    if ws.is_null() {
                        std::ptr::null_mut()
                    } else {
                        objc2::msg_send![ws, notificationCenter]
                    }
                }
                None => std::ptr::null_mut(),
            };
            let distributed_center: Obj = match cls(c"NSDistributedNotificationCenter") {
                Some(c) => objc2::msg_send![c, defaultCenter],
                None => std::ptr::null_mut(),
            };
            for (center, names) in [
                (default_center, WINDOW_NOTES),
                (workspace_center, WORKSPACE_NOTES),
                (distributed_center, DISTRIBUTED_NOTES),
            ] {
                if center.is_null() {
                    continue;
                }
                for name in names {
                    let nsname = ns_string(name);
                    if nsname.is_null() {
                        continue;
                    }
                    let null: Obj = std::ptr::null_mut();
                    let _: () = objc2::msg_send![
                        center,
                        addObserver: owner,
                        selector: objc2::sel!(k2OnNote:),
                        name: nsname,
                        object: null
                    ];
                }
            }
            install_flags_monitor();
        });
    }

    fn install_flags_monitor() {
        let desc = Box::leak(Box::new(BlockDescriptor {
            reserved: 0,
            size: std::mem::size_of::<BlockLiteral>(),
            signature: b"@@?@\0".as_ptr() as *const c_char,
            layout: std::ptr::null(),
        }));
        let block = Box::leak(Box::new(BlockLiteral {
            isa: std::ptr::addr_of!(_NSConcreteGlobalBlock) as *const c_void,
            // BLOCK_IS_GLOBAL | BLOCK_HAS_SIGNATURE. A global block is what
            // NSEvent copies for the flags-changed monitor.
            flags: (1 << 28) | (1 << 30),
            reserved: 0,
            invoke: flags_invoke,
            descriptor: desc,
        }));
        let mask: usize = 1 << 12;
        let block_id = block as *mut BlockLiteral as id;
        let _: id = unsafe {
            msg_send![class!(NSEvent), addLocalMonitorForEventsMatchingMask: mask handler: block_id]
        };
    }

    fn ensure_tracking(btn: id) {
        if btn == nil || has_our_tracking(btn) || tracker() == nil {
            return;
        }
        let bounds: NSRect = unsafe { msg_send![btn, bounds] };
        // entered/exited | active always | in visible rect | during drag
        let opts: usize = 0x01 | 0x80 | 0x200 | 0x400;
        let info: id = unsafe {
            msg_send![class!(NSDictionary), dictionaryWithObject: ns_utf8("1") forKey: ns_utf8("k2SquareTraffic")]
        };
        let area: id = unsafe { msg_send![class!(NSTrackingArea), alloc] };
        let area: id = unsafe {
            msg_send![area, initWithRect: bounds options: opts owner: tracker() userInfo: info]
        };
        if area == nil {
            return;
        }
        unsafe {
            let _: () = msg_send![btn, addTrackingArea: area];
            let _: () = msg_send![area, release];
        }
    }

    fn has_our_tracking(btn: id) -> bool {
        let areas: id = unsafe { msg_send![btn, trackingAreas] };
        if areas == nil {
            return false;
        }
        let count: usize = unsafe { msg_send![areas, count] };
        for i in 0..count {
            let area: id = unsafe { msg_send![areas, objectAtIndex: i] };
            if area_is_ours(area) {
                return true;
            }
        }
        false
    }

    fn remove_tracking(btn: id) {
        let areas: id = unsafe { msg_send![btn, trackingAreas] };
        if areas == nil {
            return;
        }
        let count: usize = unsafe { msg_send![areas, count] };
        let mut drop_list = Vec::new();
        for i in 0..count {
            let area: id = unsafe { msg_send![areas, objectAtIndex: i] };
            if area_is_ours(area) {
                drop_list.push(area);
            }
        }
        for area in drop_list {
            let _: () = unsafe { msg_send![btn, removeTrackingArea: area] };
        }
    }

    fn area_is_ours(area: id) -> bool {
        if area == nil {
            return false;
        }
        let info: id = unsafe { msg_send![area, userInfo] };
        if info == nil {
            return false;
        }
        let val: id = unsafe { msg_send![info, objectForKey: ns_utf8("k2SquareTraffic")] };
        val != nil
    }

    unsafe fn draw_square(frame: NSRect, view: id) {
        if view == nil {
            return;
        }
        let ctx: id = msg_send![class!(NSGraphicsContext), currentContext];
        if ctx != nil {
            let _: () = msg_send![ctx, saveGraphicsState];
        }
        let sq = square_rect(frame);
        let role = role_of(view);
        let pressed = is_pressed(view);
        let colored = shows_color(view);
        let mut rgb = if colored {
            active_rgb(role)
        } else {
            inactive_rgb(view)
        };
        if pressed {
            rgb = (rgb.0 * 0.72, rgb.1 * 0.72, rgb.2 * 0.72);
        }
        // Unselected windows: the existing grey, 80% translucent.
        // NSRectFill copies and drops alpha, so the fill has to composite.
        let alpha = if colored { 1.0 } else { 0.2 };
        let fill = srgb(rgb.0, rgb.1, rgb.2, alpha);
        let _: () = msg_send![fill, setFill];
        let path: id = msg_send![class!(NSBezierPath), bezierPathWithRect: sq];
        let _: () = msg_send![path, fill];
        let hovered = group_hovered(view);
        if colored && (hovered || pressed) {
            stroke_glyph(role, sq, window_is_fullscreen(view), option_held());
        }
        if ctx != nil {
            let _: () = msg_send![ctx, restoreGraphicsState];
        }
    }

    fn square_rect(frame: NSRect) -> NSRect {
        // 2px smaller than the button frame, still centered. The frame
        // stays the hit target.
        let full = frame.size.width.min(frame.size.height);
        let side = (full - 2.0).max(1.0);
        NSRect::new(
            NSPoint::new(
                frame.origin.x + (frame.size.width - side) / 2.0,
                frame.origin.y + (frame.size.height - side) / 2.0,
            ),
            NSSize::new(side, side),
        )
    }

    fn role_of(view: id) -> Role {
        if view == nil {
            return Role::Close;
        }
        let window: id = unsafe { msg_send![view, window] };
        if window == nil {
            return Role::Close;
        }
        if view == standard_button(window, NSWindowButton::NSWindowMiniaturizeButton) {
            Role::Mini
        } else if view == standard_button(window, NSWindowButton::NSWindowZoomButton) {
            Role::Zoom
        } else {
            Role::Close
        }
    }

    fn subrole(role: Role) -> &'static str {
        match role {
            Role::Close => "AXCloseButton",
            Role::Mini => "AXMinimizeButton",
            Role::Zoom => "AXFullScreenButton",
        }
    }

    fn label(role: Role) -> &'static str {
        match role {
            Role::Close => "close button",
            Role::Mini => "minimize button",
            Role::Zoom => "full screen button",
        }
    }

    fn active_rgb(role: Role) -> (f64, f64, f64) {
        match role {
            // Apple's key-window lights: #FF5F57 / #FEBC2E / #28C840.
            Role::Close => (1.0, 95.0 / 255.0, 87.0 / 255.0),
            Role::Mini => (254.0 / 255.0, 188.0 / 255.0, 46.0 / 255.0),
            Role::Zoom => (40.0 / 255.0, 200.0 / 255.0, 64.0 / 255.0),
        }
    }

    fn inactive_rgb(view: id) -> (f64, f64, f64) {
        if view_is_dark(view) {
            (0.30, 0.30, 0.30)
        } else {
            (0.84, 0.84, 0.84)
        }
    }

    fn view_is_dark(view: id) -> bool {
        let appearance: id = unsafe { msg_send![view, effectiveAppearance] };
        if appearance == nil {
            return false;
        }
        let name: id = unsafe { msg_send![appearance, name] };
        if name == nil {
            return false;
        }
        let ptr: *const c_char = unsafe { msg_send![name, UTF8String] };
        if ptr.is_null() {
            return false;
        }
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .contains("Dark")
    }

    fn shows_color(view: id) -> bool {
        let window: id = unsafe { msg_send![view, window] };
        if window == nil {
            return false;
        }
        let key: BOOL = unsafe { msg_send![window, isKeyWindow] };
        let app: id = unsafe { msg_send![class!(NSApplication), sharedApplication] };
        let active: BOOL = unsafe { msg_send![app, isActive] };
        key == YES && active == YES
    }

    fn is_pressed(view: id) -> bool {
        let highlighted: BOOL = unsafe { msg_send![view, isHighlighted] };
        highlighted == YES
    }

    fn window_is_fullscreen(view: id) -> bool {
        let window: id = unsafe { msg_send![view, window] };
        if window == nil {
            return false;
        }
        let mask: usize = unsafe { msg_send![window, styleMask] };
        mask & (1 << 14) != 0
    }

    fn option_held() -> bool {
        let flags: usize = unsafe { msg_send![class!(NSEvent), modifierFlags] };
        flags & (1 << 19) != 0
    }

    fn group_hovered(view: id) -> bool {
        let window: id = unsafe { msg_send![view, window] };
        if window == nil {
            return false;
        }
        let mouse: NSPoint = unsafe { msg_send![window, mouseLocationOutsideOfEventStream] };
        for kind in [
            NSWindowButton::NSWindowCloseButton,
            NSWindowButton::NSWindowMiniaturizeButton,
            NSWindowButton::NSWindowZoomButton,
        ] {
            let btn = standard_button(window, kind);
            if btn == nil {
                continue;
            }
            let sv: id = unsafe { msg_send![btn, superview] };
            if sv == nil {
                continue;
            }
            let local: NSPoint = unsafe { msg_send![sv, convertPoint: mouse fromView: nil] };
            let frame: NSRect = unsafe { msg_send![btn, frame] };
            let flipped: BOOL = unsafe { msg_send![sv, isFlipped] };
            if unsafe { NSMouseInRect(local, frame, flipped) } == YES {
                return true;
            }
        }
        false
    }

    fn srgb(r: f64, g: f64, b: f64, a: f64) -> id {
        unsafe { msg_send![class!(NSColor), colorWithSRGBRed: r green: g blue: b alpha: a] }
    }

    fn at(sq: NSRect, u: f64, v: f64) -> NSPoint {
        NSPoint::new(
            sq.origin.x + u * sq.size.width,
            sq.origin.y + v * sq.size.height,
        )
    }

    fn add_line(path: id, a: NSPoint, b: NSPoint) {
        unsafe {
            let _: () = msg_send![path, moveToPoint: a];
            let _: () = msg_send![path, lineToPoint: b];
        }
    }

    fn add_arrow(path: id, from: NSPoint, to: NSPoint, head: f64) {
        add_line(path, from, to);
        let dx = to.x - from.x;
        let dy = to.y - from.y;
        let len = (dx * dx + dy * dy).sqrt();
        if len < 0.01 {
            return;
        }
        let ux = dx / len;
        let uy = dy / len;
        let px = -uy;
        let py = ux;
        let wing = head * 0.72;
        let bx = to.x - ux * head;
        let by = to.y - uy * head;
        add_line(path, NSPoint::new(bx + px * wing, by + py * wing), to);
        add_line(path, NSPoint::new(bx - px * wing, by - py * wing), to);
    }

    fn stroke_glyph(role: Role, sq: NSRect, fullscreen: bool, option: bool) {
        let path: id = unsafe { msg_send![class!(NSBezierPath), bezierPath] };
        let lw = (sq.size.width / 14.0) * 1.25;
        unsafe {
            let _: () = msg_send![path, setLineWidth: lw];
            // Round cap and join, so the tiny marks match the system glyphs' ends.
            let _: () = msg_send![path, setLineCapStyle: 1usize];
            let _: () = msg_send![path, setLineJoinStyle: 1usize];
        }
        match role {
            Role::Close => {
                add_line(path, at(sq, 0.30, 0.30), at(sq, 0.70, 0.70));
                add_line(path, at(sq, 0.70, 0.30), at(sq, 0.30, 0.70));
            }
            Role::Mini => {
                add_line(path, at(sq, 0.28, 0.50), at(sq, 0.72, 0.50));
            }
            Role::Zoom if option => {
                add_line(path, at(sq, 0.28, 0.50), at(sq, 0.72, 0.50));
                add_line(path, at(sq, 0.50, 0.28), at(sq, 0.50, 0.72));
            }
            Role::Zoom if fullscreen => {
                let head = sq.size.width * 0.18;
                add_arrow(path, at(sq, 0.24, 0.76), at(sq, 0.44, 0.56), head);
                add_arrow(path, at(sq, 0.76, 0.24), at(sq, 0.56, 0.44), head);
            }
            Role::Zoom => {
                let head = sq.size.width * 0.18;
                add_arrow(path, at(sq, 0.46, 0.54), at(sq, 0.24, 0.76), head);
                add_arrow(path, at(sq, 0.54, 0.46), at(sq, 0.76, 0.24), head);
            }
        }
        let ink = srgb(0.0, 0.0, 0.0, 0.62);
        unsafe {
            let _: () = msg_send![ink, setStroke];
            let _: () = msg_send![path, stroke];
        }
    }
}

/// Offset the macOS traffic lights by (x, y) logical px from their default
/// position, and paint them square when `square` is true. (0, 0) restores
/// the default position. `zoom` is the renderer's app zoom (1 when absent):
/// above or below 100% the buttons follow the scaled top bar's center.
/// No-op on other platforms.
#[tauri::command]
pub fn set_traffic_light_inset(
    window: tauri::Window,
    x: f64,
    y: f64,
    square: bool,
    zoom: Option<f64>,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let zoom = geometry::sane_zoom(zoom.unwrap_or(1.0));
        let w = window.clone();
        window
            .run_on_main_thread(move || {
                imp::guarded("set_traffic_light_inset", || unsafe {
                    imp::position(&w, x, y, zoom, square)
                })
            })
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (window, x, y, square, zoom);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::geometry::{
        button_center_from_top, effective_y, place, sane_zoom, Defaults, Frames, Offset, Rect,
    };

    /// The research note's example: close at (7, 6), 28 px title bar.
    const D: Defaults = Defaults {
        button_x: 7.0,
        button_y: 6.0,
        button_h: 14.0,
        spacing: 20.0,
        titlebar_h: 28.0,
    };
    const WINDOW_H: f64 = 900.0;
    const WINDOW_W: f64 = 1400.0;

    /// What AppKit lays out before K2 moves anything.
    fn system_frames(window_h: f64) -> Frames {
        let button = |i: usize| Rect {
            x: D.button_x + (i as f64) * D.spacing,
            y: D.button_y,
            w: 14.0,
            h: D.button_h,
        };
        Frames {
            container: Rect {
                x: 0.0,
                y: window_h - D.titlebar_h,
                w: WINDOW_W,
                h: D.titlebar_h,
            },
            buttons: [button(0), button(1), button(2)],
        }
    }

    fn at(x: f64, y: f64) -> Offset {
        Offset { x, y, zoom: 1.0 }
    }

    #[test]
    fn places_container_and_buttons_from_the_defaults() {
        for (x, y) in [(0.0, 3.0), (10.0, 13.0), (12.0, 15.0), (6.0, 9.0)] {
            let f = place(D, system_frames(WINDOW_H), WINDOW_H, at(x, y));
            assert_eq!(f.container.h, 28.0 + y);
            assert_eq!(f.container.y, WINDOW_H - 28.0 - y);
            assert_eq!(f.container.x, 0.0);
            assert_eq!(f.container.w, WINDOW_W);
            for (i, b) in f.buttons.iter().enumerate() {
                assert_eq!(b.y, 6.0, "button {i} y");
                assert_eq!(b.x, 7.0 + x + (i as f64) * 20.0, "button {i} x");
                assert_eq!((b.w, b.h), (14.0, 14.0), "button {i} size");
            }
        }
    }

    #[test]
    fn reapplying_100_times_never_accumulates() {
        for offset in [
            at(10.0, 13.0),
            at(0.0, 3.0),
            Offset {
                x: 10.0,
                y: 13.0,
                zoom: 1.5,
            },
            Offset {
                x: 0.0,
                y: 3.0,
                zoom: 0.5,
            },
            Offset {
                x: 6.0,
                y: 9.0,
                zoom: 2.0,
            },
        ] {
            let first = place(D, system_frames(WINDOW_H), WINDOW_H, offset);
            let mut current = first;
            for round in 0..100 {
                current = place(D, current, WINDOW_H, offset);
                assert_eq!(current, first, "round {round} for {offset:?}");
            }
        }
    }

    #[test]
    fn a_system_reset_between_applies_lands_on_the_same_place() {
        // AppKit parks the buttons back at the system frames; the next apply
        // must restore exactly what the first one produced.
        let offset = at(10.0, 13.0);
        let first = place(D, system_frames(WINDOW_H), WINDOW_H, offset);
        for _ in 0..100 {
            let again = place(D, system_frames(WINDOW_H), WINDOW_H, offset);
            assert_eq!(again, first);
        }
    }

    #[test]
    fn zero_offset_at_100_percent_returns_the_captured_defaults_exactly() {
        let system = system_frames(WINDOW_H);
        let moved = place(D, system, WINDOW_H, at(12.0, 15.0));
        assert_ne!(moved, system);
        let back = place(D, moved, WINDOW_H, at(0.0, 0.0));
        assert_eq!(back, system);
        assert_eq!(place(D, system, WINDOW_H, at(0.0, 0.0)), system);
    }

    #[test]
    fn window_height_moves_only_the_container_origin() {
        let offset = at(10.0, 13.0);
        let short = place(D, system_frames(WINDOW_H), WINDOW_H, offset);
        let tall = place(D, short, 1200.0, offset);
        assert_eq!(tall.container.y, 1200.0 - 28.0 - 13.0);
        assert_eq!(tall.container.h, short.container.h);
        assert_eq!(tall.container.x, short.container.x);
        assert_eq!(tall.container.w, short.container.w);
        assert_eq!(tall.buttons, short.buttons);
    }

    #[test]
    fn zoom_keeps_the_button_center_on_the_scaled_top_bar() {
        for (x, y) in [(0.0, 3.0), (10.0, 13.0)] {
            let at_100 =
                button_center_from_top(place(D, system_frames(WINDOW_H), WINDOW_H, at(x, y)));
            for zoom in [1.1, 1.25, 1.5, 2.0] {
                let f = place(D, system_frames(WINDOW_H), WINDOW_H, Offset { x, y, zoom });
                let center = button_center_from_top(f);
                assert!(
                    (center - zoom * at_100).abs() < 1e-9,
                    "zoom {zoom}: center {center}, want {}",
                    zoom * at_100
                );
                // Horizontal position does not follow zoom.
                assert_eq!(f.buttons[0].x, 7.0 + x);
                assert_eq!(f.buttons[0].y, 6.0);
            }
        }
    }

    #[test]
    fn buttons_center_on_the_zoomed_38px_top_bar_at_each_zoom_step() {
        // System center 16 + the renderer's 3px nudge = 19, half the 38px
        // top bar: centered at 100%. Close origin x 9, 23 apart (macOS 27).
        let d = Defaults {
            button_x: 9.0,
            button_y: 9.0,
            button_h: 14.0,
            spacing: 23.0,
            titlebar_h: 32.0,
        };
        const TOPBAR_H: f64 = 38.0;
        for inset in [0.0, 6.0, 10.0, 12.0] {
            for zoom in [0.8, 1.0, 1.25, 1.5] {
                let f = place(
                    d,
                    Frames {
                        container: Rect {
                            x: 0.0,
                            y: WINDOW_H - d.titlebar_h,
                            w: WINDOW_W,
                            h: d.titlebar_h,
                        },
                        buttons: [0.0, 1.0, 2.0].map(|i| Rect {
                            x: d.button_x + i * d.spacing,
                            y: d.button_y,
                            w: 14.0,
                            h: d.button_h,
                        }),
                    },
                    WINDOW_H,
                    Offset {
                        x: inset,
                        y: inset + 3.0,
                        zoom,
                    },
                );
                // The DOM bar's top (inset) and height both scale.
                let bar_center = zoom * (inset + TOPBAR_H / 2.0);
                let center = button_center_from_top(f);
                assert!(
                    (center - bar_center).abs() < 1e-9,
                    "inset {inset} zoom {zoom}: center {center}, bar center {bar_center}"
                );
                // Horizontal inset stays native; the renderer's spacer
                // (stoplightSpacerPx) absorbs the zoom.
                for (i, b) in f.buttons.iter().enumerate() {
                    assert_eq!(b.x, 9.0 + inset + (i as f64) * 23.0, "button {i} x");
                    assert_eq!((b.w, b.h), (14.0, 14.0), "button {i} size");
                }
                assert_eq!(f.buttons[2].x + 14.0, 69.0 + inset, "cluster right edge");
            }
        }
    }

    #[test]
    fn zoom_below_100_never_lifts_the_buttons_out_of_the_window() {
        for zoom in [0.9, 0.5, 0.1] {
            let f = place(
                D,
                system_frames(WINDOW_H),
                WINDOW_H,
                Offset {
                    x: 0.0,
                    y: 3.0,
                    zoom,
                },
            );
            let b = f.buttons[0];
            let top_gap = f.container.h - (b.y + b.h);
            assert!(
                top_gap >= 0.0,
                "zoom {zoom}: button top {top_gap} above the window"
            );
        }
        // 90% with the 3px nudge is still a plain scale, no clamp.
        let at_100 =
            button_center_from_top(place(D, system_frames(WINDOW_H), WINDOW_H, at(0.0, 3.0)));
        let f = place(
            D,
            system_frames(WINDOW_H),
            WINDOW_H,
            Offset {
                x: 0.0,
                y: 3.0,
                zoom: 0.9,
            },
        );
        assert!((button_center_from_top(f) - 0.9 * at_100).abs() < 1e-9);
    }

    #[test]
    fn bad_zoom_and_offsets_fall_back_to_100_percent() {
        for zoom in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(sane_zoom(zoom), 1.0, "zoom {zoom}");
            assert_eq!(
                effective_y(
                    D,
                    Offset {
                        x: 0.0,
                        y: 13.0,
                        zoom
                    }
                ),
                13.0
            );
        }
        let f = place(
            D,
            system_frames(WINDOW_H),
            WINDOW_H,
            Offset {
                x: f64::NAN,
                y: f64::INFINITY,
                zoom: 1.0,
            },
        );
        assert_eq!(f, system_frames(WINDOW_H));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn every_registered_notification_has_an_action() {
        use super::imp::{
            note_action, NoteAction, DISTRIBUTED_NOTES, WINDOW_NOTES, WORKSPACE_NOTES,
        };
        for name in WINDOW_NOTES
            .iter()
            .chain(WORKSPACE_NOTES)
            .chain(DISTRIBUTED_NOTES)
        {
            let name = name.to_str().expect("notification names are ASCII");
            assert!(note_action(name).is_some(), "{name} has no action");
        }
        for name in [
            "NSWindowDidResizeNotification",
            "NSWindowDidEndLiveResizeNotification",
            "NSWindowDidChangeScreenNotification",
            "NSWindowDidChangeBackingPropertiesNotification",
            "NSWindowDidDeminiaturizeNotification",
            "NSWindowDidBecomeMainNotification",
            "NSWindowDidEnterFullScreenNotification",
            "NSWindowDidExitFullScreenNotification",
        ] {
            assert_eq!(note_action(name), Some(NoteAction::ReapplyWindow), "{name}");
        }
        assert_eq!(
            note_action("NSWindowDidBecomeKeyNotification"),
            Some(NoteAction::ReapplyWindowAndRepaint)
        );
        for name in [
            "NSApplicationDidChangeScreenParametersNotification",
            "NSWorkspaceDidWakeNotification",
            "NSWorkspaceScreensDidWakeNotification",
            "AppleInterfaceThemeChangedNotification",
        ] {
            assert_eq!(note_action(name), Some(NoteAction::ReapplyAll), "{name}");
        }
        assert_eq!(
            note_action("NSWindowWillCloseNotification"),
            Some(NoteAction::Forget)
        );
        assert_eq!(note_action("NSWindowDidMoveNotification"), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn guarded_stops_a_panic_and_reports_it_once() {
        let mut reports: Vec<String> = Vec::new();
        super::imp::guarded_with(
            "test",
            || panic!("k2-traffic-light-test-panic {}", 7),
            |line| reports.push(line.to_owned()),
        );
        assert_eq!(
            reports,
            vec!["traffic-lights test panic: k2-traffic-light-test-panic 7"]
        );

        let mut quiet: Vec<String> = Vec::new();
        let mut ran = false;
        super::imp::guarded_with("test", || ran = true, |line| quiet.push(line.to_owned()));
        assert!(ran);
        assert!(quiet.is_empty(), "a clean run reported {quiet:?}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn guarded_catches_an_objc_exception() {
        let mut reports: Vec<String> = Vec::new();
        super::imp::guarded_with(
            "test",
            || unsafe { super::imp::raise_for_test() },
            |line| reports.push(line.to_owned()),
        );
        assert_eq!(reports.len(), 1, "{reports:?}");
        assert!(
            reports[0].starts_with("traffic-lights test objc-exception: ")
                && reports[0].contains("k2-traffic-light-test-exception"),
            "{}",
            reports[0]
        );
    }
}
