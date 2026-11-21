//! macOS: the self-view's window and its picture.
//!
//! The window is an `NSPanel` of K2's own: borderless, non-activating,
//! rounded, floating above K2's windows (and hidden while K2 isn't the app in
//! front), a child of the K2 window that shows the call so it moves with it,
//! draggable by its picture, with a close button at its top right. It sits at
//! the top right of the call view when the page gives one, else of the
//! window, below the top bar. Made once, on the main thread, and kept for
//! the app's life: shown and hidden, never released.
//!
//! The picture is an `AVSampleBufferDisplayLayer` filling the panel (aspect
//! fill). Each camera frame's `CVPixelBuffer` (borrowed from Sew's frame for
//! the call) goes into a `CMSampleBuffer` marked to show at once and is
//! enqueued from the sink's thread, under one lock: CoreMedia takes its own
//! retains, so nothing here retains or releases the camera's buffer. The
//! mirror is the layer's transform, set on the main thread; while it changes
//! frames are dropped ([`FrameGate`]) so none shows the wrong way round.
//!
//! The layer's own `enqueueSampleBuffer:` / `flush` (deprecated in macOS 15
//! for `sampleBufferRenderer`, which they forward to) work on every macOS K2
//! runs on. Every send is objc2's (`C-unwind`), inside `objc2::exception::catch`,
//! so an Objective-C exception drops that one step instead of aborting.

use std::ffi::{c_void, CStr};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex, OnceLock};

use objc2::encode::{Encode, Encoding, RefEncode};
use objc2::runtime::{AnyClass, AnyObject, ClassBuilder, Sel};
use objc2::{msg_send, sel};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use tauri_plugin_sew_voice::native::NativePayload;

use super::{panel_frame, pts_90k, CssRect, FrameGate, FrameOutcome, Gate, ScreenRect, PANEL_H, PANEL_W};

pub const SUPPORTED: bool = true;

type Obj = *mut AnyObject;

/// The panel's corner radius (points).
const RADIUS: f64 = 12.0;
/// The close button's size and inset (points).
const CLOSE_SIZE: f64 = 20.0;
const CLOSE_INSET: f64 = 6.0;

// ── C types with their Objective-C encodings ─────────────────────────────

#[repr(C)]
struct OpaqueSampleBuffer {
    _p: [u8; 0],
}

// SAFETY: CMSampleBufferRef is `^{opaqueCMSampleBuffer=}`.
unsafe impl RefEncode for OpaqueSampleBuffer {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Encoding::Struct("opaqueCMSampleBuffer", &[]));
}

#[repr(C)]
struct OpaqueCGColor {
    _p: [u8; 0],
}

// SAFETY: CGColorRef is `^{CGColor=}`.
unsafe impl RefEncode for OpaqueCGColor {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Encoding::Struct("CGColor", &[]));
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGAffineTransform {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    tx: f64,
    ty: f64,
}

// SAFETY: the same layout and encoding as CoreGraphics' (64-bit CGFloat).
unsafe impl Encode for CGAffineTransform {
    const ENCODING: Encoding = Encoding::Struct(
        "CGAffineTransform",
        &[f64::ENCODING, f64::ENCODING, f64::ENCODING, f64::ENCODING, f64::ENCODING, f64::ENCODING],
    );
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CMTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

const CM_TIME_VALID: u32 = 1;
const CM_TIME_INVALID: CMTime = CMTime { value: 0, timescale: 0, flags: 0, epoch: 0 };

#[repr(C)]
struct CMSampleTimingInfo {
    duration: CMTime,
    pts: CMTime,
    dts: CMTime,
}

#[link(name = "CoreMedia", kind = "framework")]
extern "C" {
    fn CMVideoFormatDescriptionCreateForImageBuffer(
        allocator: *const c_void,
        image_buffer: *mut c_void,
        format_out: *mut *mut c_void,
    ) -> i32;
    fn CMSampleBufferCreateReadyWithImageBuffer(
        allocator: *const c_void,
        image_buffer: *mut c_void,
        format: *mut c_void,
        timing: *const CMSampleTimingInfo,
        sample_out: *mut *mut OpaqueSampleBuffer,
    ) -> i32;
    fn CMSampleBufferGetSampleAttachmentsArray(sample: *mut OpaqueSampleBuffer, create: u8) -> *const c_void;
    static kCMSampleAttachmentKey_DisplayImmediately: *const c_void;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(cf: *const c_void);
    fn CFArrayGetCount(array: *const c_void) -> isize;
    fn CFArrayGetValueAtIndex(array: *const c_void, index: isize) -> *const c_void;
    fn CFDictionarySetValue(dict: *const c_void, key: *const c_void, value: *const c_void);
    static kCFBooleanTrue: *const c_void;
}

#[link(name = "AVFoundation", kind = "framework")]
extern "C" {
    static AVLayerVideoGravityResizeAspectFill: Obj;
}

// ── Faults ───────────────────────────────────────────────────────────────

/// Run `f` so that it never unwinds: an Objective-C exception or a panic is
/// `Err` (described, for the log).
fn guarded<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    match catch_unwind(AssertUnwindSafe(|| objc2::exception::catch(AssertUnwindSafe(f)))) {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(exception)) => {
            std::mem::forget(exception);
            Err("objc_exception".into())
        }
        Err(payload) => {
            std::mem::forget(payload);
            Err("panic".into())
        }
    }
}

fn cls(name: &CStr) -> Result<&'static AnyClass, String> {
    AnyClass::get(name).ok_or_else(|| format!("no_class:{}", name.to_string_lossy()))
}

unsafe fn ns_string(s: &CStr) -> Obj {
    match AnyClass::get(c"NSString") {
        Some(c) => msg_send![c, stringWithUTF8String: s.as_ptr()],
        None => std::ptr::null_mut(),
    }
}

// ── The display: frames, on the sink's thread ────────────────────────────

struct Inner {
    gate: FrameGate,
    /// The AVSampleBufferDisplayLayer (the panel keeps it), 0 until the
    /// panel exists.
    layer: usize,
}

type MainRunner = Box<dyn Fn(Box<dyn FnOnce() + Send>) + Send + Sync>;

pub struct Display {
    inner: Arc<Mutex<Inner>>,
    main: MainRunner,
}

/// The panel's objects (main thread only), made once.
struct Panel {
    panel: usize,
    layer: usize,
    /// The window it is a child of now, 0 while hidden.
    parent: usize,
}

static PANEL: Mutex<Option<Panel>> = Mutex::new(None);

impl Display {
    pub fn new<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Display {
        let main: MainRunner = Box::new(move |f| {
            let _ = app.run_on_main_thread(f);
        });
        Display { inner: Arc::new(Mutex::new(Inner { gate: FrameGate::default(), layer: 0 })), main }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// One camera frame (its buffer borrowed for this call).
    pub fn frame(&self, payload: &NativePayload, mirror: bool, discontinuity: bool, timestamp_us: u64) -> FrameOutcome {
        let Some(buffer) = payload.apple_pixel_buffer().filter(|p| !p.is_null()) else {
            return FrameOutcome::NoBuffer;
        };
        let mut inner = self.lock();
        let layer = inner.layer as Obj;
        if layer.is_null() {
            return FrameOutcome::Dropped;
        }
        match inner.gate.frame(mirror, discontinuity) {
            Gate::Drop => FrameOutcome::Dropped,
            Gate::Mirror(on) => {
                let _ = guarded(|| unsafe { flush_layer(layer, true) });
                let shared = self.inner.clone();
                let layer_addr = layer as usize;
                (self.main)(Box::new(move || {
                    let _ = guarded(|| unsafe { set_mirror(layer_addr as Obj, on) });
                    shared.lock().unwrap_or_else(|e| e.into_inner()).gate.mirrored();
                }));
                FrameOutcome::Dropped
            }
            Gate::Push { flush } => {
                let pts = pts_90k(timestamp_us);
                match guarded(|| unsafe { push(layer, buffer, flush, pts) }) {
                    Ok(true) => FrameOutcome::Shown,
                    _ => FrameOutcome::Dropped,
                }
            }
        }
    }

    /// Drop what the layer holds (`remove_image`: show nothing).
    pub fn flush(&self, remove_image: bool) {
        let inner = self.lock();
        let layer = inner.layer as Obj;
        if !layer.is_null() {
            let _ = guarded(|| unsafe { flush_layer(layer, remove_image) });
        }
    }

    /// Main thread: the panel on `window`'s, placed by `anchor`.
    pub fn show<R: tauri::Runtime>(&self, window: &tauri::Window<R>, anchor: Option<CssRect>) -> Result<(), String> {
        let parent = window.ns_window().map_err(|_| "no_ns_window".to_string())? as Obj;
        if parent.is_null() {
            return Err("no_ns_window".into());
        }
        let mut slot = PANEL.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_none() {
            let made = guarded(|| unsafe { make_panel() })??;
            {
                let mut inner = self.lock();
                inner.layer = made.layer;
                // A new layer shows unmirrored: the gate starts there too.
                inner.gate = FrameGate::default();
            }
            *slot = Some(made);
        }
        let Some(p) = slot.as_mut() else { return Err("no_panel".into()) };
        let (panel, old_parent) = (p.panel as Obj, p.parent as Obj);
        guarded(|| unsafe { place(panel, parent, old_parent, anchor) })?;
        p.parent = parent as usize;
        {
            let mut inner = self.lock();
            inner.gate.set_showing(true);
        }
        Ok(())
    }

    /// Main thread: hide the panel (it stays made).
    pub fn hide(&self) {
        {
            let mut inner = self.lock();
            inner.gate.set_showing(false);
            let layer = inner.layer as Obj;
            if !layer.is_null() {
                let _ = guarded(|| unsafe { flush_layer(layer, true) });
            }
        }
        let mut slot = PANEL.lock().unwrap_or_else(|e| e.into_inner());
        let Some(p) = slot.as_mut() else { return };
        let (panel, parent) = (p.panel as Obj, p.parent as Obj);
        let _ = guarded(|| unsafe {
            if !parent.is_null() {
                let _: () = msg_send![parent, removeChildWindow: panel];
            }
            let null: Obj = std::ptr::null_mut();
            let _: () = msg_send![panel, orderOut: null];
        });
        p.parent = 0;
    }
}

// ── The layer ────────────────────────────────────────────────────────────

unsafe fn flush_layer(layer: Obj, remove_image: bool) {
    if remove_image {
        let _: () = msg_send![layer, flushAndRemoveImage];
    } else {
        let _: () = msg_send![layer, flush];
    }
}

unsafe fn set_mirror(layer: Obj, on: bool) {
    let t = if on {
        CGAffineTransform { a: -1.0, b: 0.0, c: 0.0, d: 1.0, tx: 0.0, ty: 0.0 }
    } else {
        CGAffineTransform { a: 1.0, b: 0.0, c: 0.0, d: 1.0, tx: 0.0, ty: 0.0 }
    };
    let Ok(txn) = cls(c"CATransaction") else { return };
    let _: () = msg_send![txn, begin];
    let _: () = msg_send![txn, setDisableActions: true];
    let _: () = msg_send![layer, setAffineTransform: t];
    let _: () = msg_send![txn, commit];
}

/// One frame onto the layer. False: the layer couldn't take it now.
unsafe fn push(layer: Obj, buffer: *mut c_void, flush_first: bool, pts_90k: i64) -> bool {
    if flush_first {
        flush_layer(layer, false);
    }
    // A failed layer (or one the system interrupted) takes nothing until it
    // is flushed; each camera frame stands alone.
    let status: isize = msg_send![layer, status];
    let needs_flush = {
        let responds: bool = msg_send![layer, respondsToSelector: sel!(requiresFlushToResumeDecoding)];
        responds && {
            let r: bool = msg_send![layer, requiresFlushToResumeDecoding];
            r
        }
    };
    if status == 2 || needs_flush {
        flush_layer(layer, false);
    }
    let ready: bool = msg_send![layer, isReadyForMoreMediaData];
    if !ready {
        return false;
    }
    let mut format: *mut c_void = std::ptr::null_mut();
    if CMVideoFormatDescriptionCreateForImageBuffer(std::ptr::null(), buffer, &mut format) != 0 || format.is_null() {
        return false;
    }
    let timing = CMSampleTimingInfo {
        duration: CM_TIME_INVALID,
        pts: CMTime { value: pts_90k, timescale: 90_000, flags: CM_TIME_VALID, epoch: 0 },
        dts: CM_TIME_INVALID,
    };
    let mut sample: *mut OpaqueSampleBuffer = std::ptr::null_mut();
    let made = CMSampleBufferCreateReadyWithImageBuffer(std::ptr::null(), buffer, format, &timing, &mut sample);
    CFRelease(format);
    if made != 0 || sample.is_null() {
        return false;
    }
    // Live video keeps no clock: show each frame when it arrives.
    let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, 1);
    if !attachments.is_null() && CFArrayGetCount(attachments) > 0 {
        let dict = CFArrayGetValueAtIndex(attachments, 0);
        CFDictionarySetValue(dict, kCMSampleAttachmentKey_DisplayImmediately, kCFBooleanTrue);
    }
    let _: () = msg_send![layer, enqueueSampleBuffer: sample];
    CFRelease(sample as *const c_void);
    true
}

// ── The panel (main thread) ──────────────────────────────────────────────

/// The close button's target: a tiny class of K2's own.
fn close_target() -> Result<Obj, String> {
    static TARGET: OnceLock<usize> = OnceLock::new();
    let addr = *TARGET.get_or_init(|| {
        extern "C-unwind" fn on_close(_this: *mut AnyObject, _sel: Sel, _sender: *mut AnyObject) {
            let _ = catch_unwind(super::dismiss);
        }
        let class = match AnyClass::get(c"K2SelfViewCloseTarget") {
            Some(c) => c,
            None => {
                let Some(base) = AnyClass::get(c"NSObject") else { return 0 };
                let Some(mut builder) = ClassBuilder::new(c"K2SelfViewCloseTarget", base) else { return 0 };
                // SAFETY: `k2SelfViewClose:` is `v@:@`, as `on_close` is.
                unsafe {
                    builder.add_method(
                        sel!(k2SelfViewClose:),
                        on_close as extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject),
                    );
                }
                builder.register()
            }
        };
        // SAFETY: a plain NSObject subclass; kept for the app's life.
        unsafe {
            let obj: Obj = msg_send![class, alloc];
            let obj: Obj = msg_send![obj, init];
            obj as usize
        }
    });
    if addr == 0 {
        Err("no_close_target".into())
    } else {
        Ok(addr as Obj)
    }
}

unsafe fn make_panel() -> Result<Panel, String> {
    let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(PANEL_W, PANEL_H));
    // Borderless (0) | NSWindowStyleMaskNonactivatingPanel (1 << 7).
    let style: usize = 1 << 7;
    let panel: Obj = msg_send![cls(c"NSPanel")?, alloc];
    // NSBackingStoreBuffered = 2.
    let panel: Obj = msg_send![panel, initWithContentRect: rect, styleMask: style, backing: 2usize, defer: false];
    if panel.is_null() {
        return Err("no_panel".into());
    }
    let color = cls(c"NSColor")?;
    let clear: Obj = msg_send![color, clearColor];
    let _: () = msg_send![panel, setReleasedWhenClosed: false];
    let _: () = msg_send![panel, setOpaque: false];
    let _: () = msg_send![panel, setBackgroundColor: clear];
    let _: () = msg_send![panel, setHasShadow: true];
    let _: () = msg_send![panel, setFloatingPanel: true];
    let _: () = msg_send![panel, setHidesOnDeactivate: true];
    let _: () = msg_send![panel, setBecomesKeyOnlyIfNeeded: true];
    let _: () = msg_send![panel, setMovableByWindowBackground: true];
    // NSWindowCollectionBehaviorFullScreenAuxiliary (1 << 8): over a full
    // screen K2 window too.
    let _: () = msg_send![panel, setCollectionBehavior: (1usize << 8)];
    let title = ns_string(c"Your camera");
    if !title.is_null() {
        let _: () = msg_send![panel, setTitle: title];
    }

    // The content: rounded, black until the first frame.
    let content: Obj = msg_send![panel, contentView];
    if content.is_null() {
        return Err("no_content_view".into());
    }
    let _: () = msg_send![content, setWantsLayer: true];
    let root: Obj = msg_send![content, layer];
    if root.is_null() {
        return Err("no_layer".into());
    }
    let black: Obj = msg_send![color, blackColor];
    let black_cg: *mut OpaqueCGColor = msg_send![black, CGColor];
    let _: () = msg_send![root, setBackgroundColor: black_cg];
    let _: () = msg_send![root, setCornerRadius: RADIUS];
    let _: () = msg_send![root, setMasksToBounds: true];

    // The picture.
    let layer: Obj = msg_send![cls(c"AVSampleBufferDisplayLayer")?, new];
    if layer.is_null() {
        return Err("no_display_layer".into());
    }
    let _: () = msg_send![layer, setVideoGravity: AVLayerVideoGravityResizeAspectFill];
    let bounds: NSRect = msg_send![root, bounds];
    let _: () = msg_send![layer, setFrame: bounds];
    // kCALayerWidthSizable (2) | kCALayerHeightSizable (16).
    let _: () = msg_send![layer, setAutoresizingMask: 18u32];
    let _: () = msg_send![root, addSublayer: layer];

    // The close button, top right (the content view's origin is its bottom left).
    let target = close_target()?;
    let action = sel!(k2SelfViewClose:);
    let image_cls = cls(c"NSImage")?;
    let symbol = ns_string(c"xmark.circle.fill");
    let what = ns_string(c"Hide your camera window");
    let image: Obj = msg_send![image_cls, imageWithSystemSymbolName: symbol, accessibilityDescription: what];
    let button_cls = cls(c"NSButton")?;
    let button: Obj = if image.is_null() {
        let x = ns_string(c"\xC3\x97");
        msg_send![button_cls, buttonWithTitle: x, target: target, action: action]
    } else {
        msg_send![button_cls, buttonWithImage: image, target: target, action: action]
    };
    if !button.is_null() {
        let _: () = msg_send![button, setBordered: false];
        // NSImageScaleProportionallyUpOrDown = 3.
        let _: () = msg_send![button, setImageScaling: 3usize];
        let white: Obj = msg_send![color, whiteColor];
        let _: () = msg_send![button, setContentTintColor: white];
        if !what.is_null() {
            let _: () = msg_send![button, setToolTip: what];
            let _: () = msg_send![button, setAccessibilityLabel: what];
        }
        let frame = NSRect::new(
            NSPoint::new(PANEL_W - CLOSE_SIZE - CLOSE_INSET, PANEL_H - CLOSE_SIZE - CLOSE_INSET),
            NSSize::new(CLOSE_SIZE, CLOSE_SIZE),
        );
        let _: () = msg_send![button, setFrame: frame];
        // NSViewMinXMargin (1) | NSViewMinYMargin (8): stays at the top right.
        let _: () = msg_send![button, setAutoresizingMask: 9usize];
        let _: () = msg_send![content, addSubview: button];
    }
    Ok(Panel { panel: panel as usize, layer: layer as usize, parent: 0 })
}

/// On `parent` (a child of it, so it moves with it), placed by `anchor`.
unsafe fn place(panel: Obj, parent: Obj, old_parent: Obj, anchor: Option<CssRect>) {
    if !old_parent.is_null() && old_parent != parent {
        let _: () = msg_send![old_parent, removeChildWindow: panel];
    }
    let content: Obj = msg_send![parent, contentView];
    let content_rect: NSRect = if content.is_null() {
        msg_send![parent, frame]
    } else {
        let local: NSRect = msg_send![content, frame];
        msg_send![parent, convertRectToScreen: local]
    };
    let at = panel_frame(
        ScreenRect {
            x: content_rect.origin.x,
            y: content_rect.origin.y,
            w: content_rect.size.width,
            h: content_rect.size.height,
        },
        anchor,
    );
    let frame = NSRect::new(NSPoint::new(at.x, at.y), NSSize::new(at.w, at.h));
    let _: () = msg_send![panel, setFrame: frame, display: true];
    if old_parent != parent {
        // NSWindowAbove = 1.
        let _: () = msg_send![parent, addChildWindow: panel, ordered: 1isize];
    }
    let null: Obj = std::ptr::null_mut();
    let _: () = msg_send![panel, orderFront: null];
}
