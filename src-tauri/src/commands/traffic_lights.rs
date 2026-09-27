//! macOS traffic-light repositioning for the Style System's floating-chrome
//! styles (Glass/Bezel/spacious presets): when the chrome is inset from the
//! window edge, the close/minimize/zoom buttons must move down-right with it.
//!
//! AppKit resets standard-button frames on resize/fullscreen transitions, so
//! the renderer re-invokes this after style changes AND on window-resize
//! events (see stores/style.ts). `extra_x`/`extra_y` are logical px offsets
//! from the system default position; (0, 0) restores the default exactly
//! (defaults are captured on first call, before any modification).
//!
//! When `square` is set, the same call paints those three buttons as sharp
//! squares (style id `square` only). Any other style restores the system
//! cells so AppKit draws circles again. The square is centered in the
//! button frame; frame size and origin gaps are not changed.

#[cfg(target_os = "macos")]
// cocoa/objc are deprecated in favor of objc2 but frozen as house deps for
// now (see the objc2-migration note in Cargo.toml); same allow as
// commands/permissions.rs.
#[allow(deprecated)]
mod imp {
    use std::collections::HashMap;
    use std::ffi::{c_char, c_void, CStr};
    use std::sync::{Mutex, OnceLock};

    use cocoa::appkit::{NSWindow, NSWindowButton};
    use cocoa::base::{id, nil, BOOL, NO, YES};
    use cocoa::foundation::{NSPoint, NSRect, NSSize, NSString};
    use objc::declare::ClassDecl;
    use objc::runtime::{Class, Object, Sel};
    use objc::{class, msg_send, sel, sel_impl};

    #[derive(Clone, Copy)]
    struct Defaults {
        button_x: f64,
        button_y: f64,
        titlebar_h: f64,
    }

    #[derive(Clone, Copy)]
    struct SavedInset {
        x: f64,
        y: f64,
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

    pub unsafe fn position(window: &tauri::Window, extra_x: f64, extra_y: f64, square: bool) {
        let Ok(handle) = window.ns_window() else {
            return;
        };
        position_ns(handle as id, extra_x, extra_y, square);
    }

    unsafe fn position_ns(ns_window: id, extra_x: f64, extra_y: f64, square: bool) {
        if ns_window == nil {
            return;
        }
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
                button_y: close_rect.origin.y,
                titlebar_h: tb_rect.size.height,
            }
        });

        // Grow the title-bar container downward from the top edge. The buttons
        // are pinned back to the captured y so a later call can move them up
        // again; leaving AppKit's autoresize in place kept them at the first drop.
        // `extra_y` already includes the renderer's 3px nudge. Do not add it
        // to origin.y a second time.
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
            r.origin.y = defaults.button_y;
            let _: () = msg_send![*btn, setFrameOrigin: r.origin];
        }

        insets().insert(
            ns_window as usize,
            SavedInset {
                x: extra_x,
                y: extra_y,
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
            if answers == YES { orig } else { nil }
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
        let window: id = unsafe { msg_send![event, window] };
        if window_has_square_paint(window) {
            mark_buttons_dirty(window);
        }
    }

    extern "C" fn on_note(_this: &Object, _: Sel, note: id) {
        let name: id = unsafe { msg_send![note, name] };
        let obj: id = unsafe { msg_send![note, object] };
        if ns_eq(name, "NSWindowWillCloseNotification") {
            if obj != nil {
                insets().remove(&(obj as usize));
            }
            return;
        }
        if ns_eq(name, "NSWindowDidEnterFullScreenNotification")
            || ns_eq(name, "NSWindowDidExitFullScreenNotification")
        {
            reapply_window(obj);
            return;
        }
        if ns_eq(name, "NSWindowDidBecomeKeyNotification")
            || ns_eq(name, "NSWindowDidResignKeyNotification")
        {
            if window_has_square_paint(obj) {
                mark_buttons_dirty(obj);
            }
            return;
        }
        redraw_square_windows();
    }

    extern "C" fn flags_invoke(_block: *mut BlockLiteral, event: id) -> id {
        redraw_square_windows();
        event
    }

    fn ns_eq(s: id, expected: &str) -> bool {
        if s == nil {
            return false;
        }
        unsafe { NSString::isEqualToString(s, expected) }
    }

    fn reapply_window(window: id) {
        if window == nil {
            return;
        }
        let saved = insets().get(&(window as usize)).copied();
        if let Some(saved) = saved {
            unsafe { position_ns(window, saved.x, saved.y, saved.square) };
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
        ONCE.get_or_init(|| unsafe {
      let owner = tracker();
      if owner == nil {
        return;
      }
      let center: id = msg_send![class!(NSNotificationCenter), defaultCenter];
      for name in [
        "NSWindowDidEnterFullScreenNotification",
        "NSWindowDidExitFullScreenNotification",
        "NSWindowDidBecomeKeyNotification",
        "NSWindowDidResignKeyNotification",
        "NSWindowWillCloseNotification",
        "NSApplicationDidBecomeActiveNotification",
        "NSApplicationDidResignActiveNotification",
      ] {
        let nsname = NSString::alloc(nil).init_str(name);
        let _: () = msg_send![center, addObserver: owner selector: sel!(k2OnNote:) name: nsname object: nil];
        let _: () = msg_send![nsname, release];
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
/// the default position. No-op on other platforms.
#[tauri::command]
pub fn set_traffic_light_inset(
    window: tauri::Window,
    x: f64,
    y: f64,
    square: bool,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let w = window.clone();
        window
            .run_on_main_thread(move || unsafe { imp::position(&w, x, y, square) })
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (window, x, y, square);
        Ok(())
    }
}
