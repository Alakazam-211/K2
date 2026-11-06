//! Screens and windows through ScreenCaptureKit (macOS 12.3+), loaded at
//! run time so the app still starts on older systems: the Screen Recording
//! permission, the list of displays and windows, the system picker (macOS
//! 14+, run on the main thread here), and capture streams.

use super::ffi::{self, cf_const, cm_time, SampleBufferArg};
use super::objc::{array_items, error_of, is_main_thread, on_main, string_of, ObjSend, Queue};
use super::pixel::{cpu_frame, native_frame};
use crate::api::{Done, FrameSource, Wake};
use crate::convert::fit_within;
use crate::error::{unsupported, VideoError};
use crate::kit::{now_us, QueuedSource, SourceControl, SourceShared};
use crate::types::*;
use block2::RcBlock;
use objc2::encode::{Encode, Encoding};
use objc2::rc::{autoreleasepool, Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel};
use objc2::{class, msg_send, sel};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::sync_channel;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct CGRect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

// SAFETY: CGRect's runtime encoding, `{CGRect={CGPoint=dd}{CGSize=dd}}`.
unsafe impl Encode for CGRect {
    const ENCODING: Encoding = Encoding::Struct(
        "CGRect",
        &[
            Encoding::Struct("CGPoint", &[f64::ENCODING, f64::ENCODING]),
            Encoding::Struct("CGSize", &[f64::ENCODING, f64::ENCODING]),
        ],
    );
}

/// Load ScreenCaptureKit once. False on a system without it.
fn loaded() -> bool {
    static LOADED: OnceLock<bool> = OnceLock::new();
    *LOADED.get_or_init(|| {
        for path in [
            c"/System/Library/Frameworks/ScreenCaptureKit.framework/ScreenCaptureKit",
            c"/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics",
        ] {
            // SAFETY: dlopen of a system framework by absolute path.
            unsafe { ffi::dlopen(path.as_ptr(), ffi::RTLD_LAZY) };
        }
        AnyClass::get(c"SCStream").is_some()
    })
}

pub(crate) fn supported() -> bool {
    loaded()
}

pub(crate) fn picker_supported() -> bool {
    loaded() && AnyClass::get(c"SCContentSharingPicker").is_some()
}

static ASKED: AtomicBool = AtomicBool::new(false);

fn preflight() -> bool {
    if !loaded() {
        return false;
    }
    let Some(f) = ffi::symbol(c"CGPreflightScreenCaptureAccess") else {
        return false;
    };
    // SAFETY: `bool CGPreflightScreenCaptureAccess(void)`; doesn't prompt.
    let f: extern "C" fn() -> bool = unsafe { std::mem::transmute(f) };
    f()
}

pub(crate) fn permission() -> Permission {
    if preflight() {
        Permission::Granted
    } else if ASKED.load(Ordering::Acquire) {
        Permission::Denied
    } else {
        Permission::NotDetermined
    }
}

pub(crate) fn request_permission(done: Done<Permission>) {
    if preflight() {
        return done(Permission::Granted);
    }
    if !loaded() {
        return done(Permission::Denied);
    }
    let first = !ASKED.swap(true, Ordering::AcqRel);
    if first {
        if let Some(f) = ffi::symbol(c"CGRequestScreenCaptureAccess") {
            // SAFETY: `bool CGRequestScreenCaptureAccess(void)`: shows the
            // OS prompt the first time and returns at once.
            let f: extern "C" fn() -> bool = unsafe { std::mem::transmute(f) };
            if f() {
                return done(Permission::Granted);
            }
        }
    } else {
        // Asked before and still refused: open the Screen Recording pane.
        autoreleasepool(|_| {
            // SAFETY: NSWorkspace / NSURL class methods with documented
            // signatures; nil is checked.
            unsafe {
                let Some(s) = super::objc::nsstring(
                    "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
                ) else {
                    return;
                };
                let url: *mut AnyObject = msg_send![class!(NSURL), URLWithString: &*s];
                if let Some(ws) = AnyClass::get(c"NSWorkspace") {
                    let shared: *mut AnyObject = msg_send![ws, sharedWorkspace];
                    if !shared.is_null() && !url.is_null() {
                        let _: Bool = msg_send![shared, openURL: url];
                    }
                }
            }
        });
    }
    done(permission())
}

/// SCShareableContent now (retained), or an error.
fn shareable() -> Result<ObjSend, VideoError> {
    if !loaded() {
        return Err(unsupported("ScreenCaptureKit needs macOS 12.3"));
    }
    let cls =
        AnyClass::get(c"SCShareableContent").ok_or_else(|| unsupported("no SCShareableContent"))?;
    let (tx, rx) = sync_channel::<Result<ObjSend, VideoError>>(1);
    let tx = Mutex::new(Some(tx));
    let block = RcBlock::new(move |content: *mut AnyObject, error: *mut AnyObject| {
        // SAFETY: the completion's arguments: an SCShareableContent or nil,
        // and an NSError or nil.
        let r = unsafe {
            match Retained::retain(content) {
                Some(c) => Ok(ObjSend(c)),
                None => Err(sck_error(error)),
            }
        };
        if let Some(tx) = tx.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = tx.send(r);
        }
    });
    autoreleasepool(|_| {
        // SAFETY: a class method taking two BOOLs and a block it copies.
        unsafe {
            let _: () = msg_send![
                cls,
                getShareableContentExcludingDesktopWindows: Bool::YES,
                onScreenWindowsOnly: Bool::YES,
                completionHandler: &*block
            ];
        }
    });
    rx.recv_timeout(Duration::from_secs(5))
        .map_err(|_| VideoError::os(0, "ScreenCaptureKit didn't list the screens in 5 s"))?
}

unsafe fn sck_error(e: *mut AnyObject) -> VideoError {
    let (code, desc) = error_of(e);
    match code {
        // SCStreamErrorUserDeclined
        -3801 => VideoError::NoPermission,
        -3815 => VideoError::NotFound,
        _ => VideoError::os(code, desc),
    }
}

unsafe fn displays(content: &AnyObject) -> Vec<*mut AnyObject> {
    let a: *mut AnyObject = msg_send![content, displays];
    array_items(a)
}

unsafe fn windows(content: &AnyObject) -> Vec<*mut AnyObject> {
    let a: *mut AnyObject = msg_send![content, windows];
    array_items(a)
}

unsafe fn window_ok(w: *mut AnyObject) -> bool {
    let layer: isize = msg_send![w, windowLayer];
    let frame: CGRect = msg_send![w, frame];
    let title: *mut AnyObject = msg_send![w, title];
    layer == 0 && frame.w >= 64.0 && frame.h >= 64.0 && !string_of(title).is_empty()
}

pub(crate) fn sources() -> Result<Vec<ScreenSourceInfo>, VideoError> {
    let content = shareable()?;
    autoreleasepool(|_| {
        // SAFETY: ScreenCaptureKit getters with documented signatures on the
        // retained content and the objects it holds.
        unsafe {
            let mut out = Vec::new();
            for (i, d) in displays(&content).into_iter().enumerate() {
                let id: u32 = msg_send![d, displayID];
                let w: isize = msg_send![d, width];
                let h: isize = msg_send![d, height];
                out.push(ScreenSourceInfo::new(
                    format!("display:{id}"),
                    ScreenKind::Display,
                    format!("Display {}", i + 1),
                    w as u32,
                    h as u32,
                ));
            }
            for w in windows(&content) {
                if !window_ok(w) {
                    continue;
                }
                let id: u32 = msg_send![w, windowID];
                let title: *mut AnyObject = msg_send![w, title];
                let frame: CGRect = msg_send![w, frame];
                let app: *mut AnyObject = msg_send![w, owningApplication];
                let mut info = ScreenSourceInfo::new(
                    format!("window:{id}"),
                    ScreenKind::Window,
                    string_of(title),
                    frame.w as u32,
                    frame.h as u32,
                );
                if !app.is_null() {
                    let name: *mut AnyObject = msg_send![app, applicationName];
                    info.app = Some(string_of(name)).filter(|s| !s.is_empty());
                }
                out.push(info);
            }
            Ok(out)
        }
    })
}

// ───────────────────────────── the picker ─────────────────────────────

/// What the system picker chose: an SCContentFilter.
pub(crate) struct PickedFilter(pub(crate) ObjSend);

struct PickCtx {
    done: Mutex<Option<Done<Result<ScreenSelection, VideoError>>>>,
}

const PICK_IVAR: &std::ffi::CStr = c"kesselVideoPick";
static PICK_OBSERVER: Mutex<Option<ObjSend>> = Mutex::new(None);

unsafe fn finish_pick(this: &AnyObject, r: Result<ScreenSelection, VideoError>) {
    let Some(ivar) = this.class().instance_variable(PICK_IVAR) else {
        return;
    };
    let slot = ivar.load_ptr::<*mut c_void>(this);
    let p = std::mem::replace(&mut *slot, std::ptr::null_mut());
    if p.is_null() {
        return;
    }
    let ctx = Box::from_raw(p as *mut PickCtx);
    if let Some(cls) = AnyClass::get(c"SCContentSharingPicker") {
        let picker: *mut AnyObject = msg_send![cls, sharedPicker];
        if !picker.is_null() {
            let _: () = msg_send![picker, removeObserver: this];
            let _: () = msg_send![picker, setActive: Bool::NO];
        }
    }
    if let Some(done) = ctx.done.lock().unwrap_or_else(|e| e.into_inner()).take() {
        done(r);
    }
    *PICK_OBSERVER.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

unsafe extern "C-unwind" fn picker_updated(
    this: *mut AnyObject,
    _: Sel,
    _picker: *mut AnyObject,
    filter: *mut AnyObject,
    _stream: *mut AnyObject,
) {
    let this: &AnyObject = &*this;
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let r = match Retained::retain(filter) {
            Some(f) => Ok(ScreenSelection::Picked(Arc::new(PickedFilter(ObjSend(f))))),
            None => Err(VideoError::NotFound),
        };
        finish_pick(this, r);
    }));
}

unsafe extern "C-unwind" fn picker_cancelled(
    this: *mut AnyObject,
    _: Sel,
    _picker: *mut AnyObject,
    _stream: *mut AnyObject,
) {
    let this: &AnyObject = &*this;
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        finish_pick(this, Err(VideoError::NotFound))
    }));
}

unsafe extern "C-unwind" fn picker_failed(this: *mut AnyObject, _: Sel, error: *mut AnyObject) {
    let this: &AnyObject = &*this;
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        finish_pick(this, Err(sck_error(error)))
    }));
}

fn observer_class() -> &'static AnyClass {
    static CLS: OnceLock<&'static AnyClass> = OnceLock::new();
    CLS.get_or_init(|| {
        let mut b = ClassBuilder::new(c"KesselVideoPickerObserver", class!(NSObject))
            .expect("the observer class name is free");
        b.add_ivar::<*mut c_void>(PICK_IVAR);
        if let Some(p) = objc2::runtime::AnyProtocol::get(c"SCContentSharingPickerObserver") {
            b.add_protocol(p);
        }
        // SAFETY: the SCContentSharingPickerObserver method signatures.
        unsafe {
            b.add_method(
                sel!(contentSharingPicker:didUpdateWithFilter:forStream:),
                picker_updated
                    as unsafe extern "C-unwind" fn(
                        *mut AnyObject,
                        Sel,
                        *mut AnyObject,
                        *mut AnyObject,
                        *mut AnyObject,
                    ),
            );
            b.add_method(
                sel!(contentSharingPicker:didCancelForStream:),
                picker_cancelled
                    as unsafe extern "C-unwind" fn(
                        *mut AnyObject,
                        Sel,
                        *mut AnyObject,
                        *mut AnyObject,
                    ),
            );
            b.add_method(
                sel!(contentSharingPickerStartDidFailWithError:),
                picker_failed as unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
        }
        b.register()
    })
}

/// Show the system picker (macOS 14+). Runs on the main thread itself:
/// callers on any thread get `done` later, from the main thread.
pub(crate) fn pick(opts: PickOptions, done: Done<Result<ScreenSelection, VideoError>>) {
    if !picker_supported() {
        return done(Err(unsupported("the system screen picker needs macOS 14")));
    }
    let show = move || present(opts, done);
    if is_main_thread() {
        show();
    } else {
        on_main(Box::new(show));
    }
}

fn present(opts: PickOptions, done: Done<Result<ScreenSelection, VideoError>>) {
    autoreleasepool(|_| {
        // SAFETY: on the main thread; SCContentSharingPicker and its
        // configuration with documented signatures; the observer owns ctx
        // until one of its methods runs.
        unsafe {
            let Some(cls) = AnyClass::get(c"SCContentSharingPicker") else {
                return done(Err(unsupported("no SCContentSharingPicker")));
            };
            let picker: *mut AnyObject = msg_send![cls, sharedPicker];
            let Some(cfg_cls) = AnyClass::get(c"SCContentSharingPickerConfiguration") else {
                return done(Err(unsupported("no SCContentSharingPickerConfiguration")));
            };
            let cfg: Option<Retained<AnyObject>> = msg_send![cfg_cls, new];
            let (Some(cfg), false) = (cfg, picker.is_null()) else {
                return done(Err(VideoError::os(0, "the screen picker didn't start")));
            };
            // SCContentSharingPickerMode: single window 1, single display 16.
            let (d, w) = (
                opts.displays || !opts.windows,
                opts.windows || !opts.displays,
            );
            let modes: usize = if d { 16 } else { 0 } | if w { 1 } else { 0 };
            let _: () = msg_send![&*cfg, setAllowedPickerModes: modes];
            let obs: Option<Retained<AnyObject>> = msg_send![observer_class(), new];
            let Some(obs) = obs else {
                return done(Err(VideoError::os(0, "picker observer new")));
            };
            let ctx = Box::into_raw(Box::new(PickCtx {
                done: Mutex::new(Some(done)),
            }));
            if let Some(ivar) = obs.class().instance_variable(PICK_IVAR) {
                *ivar.load_ptr::<*mut c_void>(&obs) = ctx as *mut c_void;
            }
            let _: () = msg_send![picker, setDefaultConfiguration: &*cfg];
            let _: () = msg_send![picker, addObserver: &*obs];
            let _: () = msg_send![picker, setActive: Bool::YES];
            *PICK_OBSERVER.lock().unwrap_or_else(|e| e.into_inner()) = Some(ObjSend(obs));
            let _: () = msg_send![picker, present];
        }
    });
}

// ───────────────────────────── capture ─────────────────────────────

struct Ctx {
    shared: Arc<SourceShared>,
    native: bool,
}

const IVAR: &std::ffi::CStr = c"kesselVideoCtx";

/// The frame-status attachment: 0 is a complete frame (idle and blank
/// frames carry no new pixels).
unsafe fn frame_complete(sb: ffi::CMSampleBufferRef) -> bool {
    let atts = ffi::CMSampleBufferGetSampleAttachmentsArray(sb, 0);
    if atts.is_null() || ffi::CFArrayGetCount(atts) == 0 {
        return true;
    }
    let d = ffi::CFArrayGetValueAtIndex(atts, 0);
    let Some(key) = cf_const(c"SCStreamFrameInfoStatus") else {
        return true;
    };
    let v = ffi::CFDictionaryGetValue(d, key);
    ffi::number_i64(v).is_none_or(|s| s == 0)
}

unsafe extern "C-unwind" fn did_output(
    this: *mut AnyObject,
    _: Sel,
    _stream: *mut AnyObject,
    sb: SampleBufferArg,
    kind: isize,
) {
    let this: &AnyObject = &*this;
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if kind != 0 {
            return;
        }
        let Some(ivar) = this.class().instance_variable(IVAR) else {
            return;
        };
        let p = *ivar.load_ptr::<*mut c_void>(this);
        if p.is_null() {
            return;
        }
        let ctx = &*(p as *const Ctx);
        if ctx.shared.idle() || !frame_complete(sb.0) {
            return;
        }
        let pb = ffi::CMSampleBufferGetImageBuffer(sb.0);
        if pb.is_null() {
            return;
        }
        let at = now_us();
        if ctx.native {
            ctx.shared.deliver(native_frame(pb, at));
        } else {
            match cpu_frame(pb, at) {
                Ok(f) => ctx.shared.deliver(f),
                Err(e) => ctx.shared.event(SourceEvent::Error(e)),
            }
        }
    }));
}

unsafe extern "C-unwind" fn did_stop(
    this: *mut AnyObject,
    _: Sel,
    _stream: *mut AnyObject,
    error: *mut AnyObject,
) {
    let this: &AnyObject = &*this;
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Some(ivar) = this.class().instance_variable(IVAR) else {
            return;
        };
        let p = *ivar.load_ptr::<*mut c_void>(this);
        if p.is_null() {
            return;
        }
        let ctx = &*(p as *const Ctx);
        let (code, _) = error_of(error);
        let reason = match code {
            // SCStreamErrorUserStopped
            -3817 => EndReason::UserStopped,
            // SCStreamErrorNoCaptureSource: the window or display is gone.
            -3815 => EndReason::SourceClosed,
            // SCStreamErrorUserDeclined
            -3801 => EndReason::Revoked,
            _ => EndReason::DeviceLost,
        };
        ctx.shared.event(SourceEvent::Ended(reason));
    }));
}

fn stream_class() -> &'static AnyClass {
    static CLS: OnceLock<&'static AnyClass> = OnceLock::new();
    CLS.get_or_init(|| {
        let mut b = ClassBuilder::new(c"KesselVideoStreamOutput", class!(NSObject))
            .expect("the output class name is free");
        b.add_ivar::<*mut c_void>(IVAR);
        for name in [c"SCStreamOutput", c"SCStreamDelegate"] {
            if let Some(p) = objc2::runtime::AnyProtocol::get(name) {
                b.add_protocol(p);
            }
        }
        // SAFETY: SCStreamOutput's stream:didOutputSampleBuffer:ofType: and
        // SCStreamDelegate's stream:didStopWithError:.
        unsafe {
            b.add_method(
                sel!(stream:didOutputSampleBuffer:ofType:),
                did_output
                    as unsafe extern "C-unwind" fn(
                        *mut AnyObject,
                        Sel,
                        *mut AnyObject,
                        SampleBufferArg,
                        isize,
                    ),
            );
            b.add_method(
                sel!(stream:didStopWithError:),
                did_stop
                    as unsafe extern "C-unwind" fn(
                        *mut AnyObject,
                        Sel,
                        *mut AnyObject,
                        *mut AnyObject,
                    ),
            );
        }
        b.register()
    })
}

struct StreamControl {
    stream: ObjSend,
    handler: ObjSend,
    queue: Queue,
    ctx: *mut Ctx,
}

// SAFETY: SCStream is thread-safe; ctx is freed only in Drop, after the
// stream stopped and the queue drained.
unsafe impl Send for StreamControl {}

impl SourceControl for StreamControl {}

/// Wait for a ScreenCaptureKit completion handler `(NSError *)`.
fn wait_completion(
    start: impl FnOnce(&block2::Block<dyn Fn(*mut AnyObject)>),
) -> Result<(), VideoError> {
    let (tx, rx) = sync_channel::<Result<(), VideoError>>(1);
    let tx = Mutex::new(Some(tx));
    let block = RcBlock::new(move |error: *mut AnyObject| {
        // SAFETY: nil or an NSError.
        let r = if error.is_null() {
            Ok(())
        } else {
            Err(unsafe { sck_error(error) })
        };
        if let Some(tx) = tx.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = tx.send(r);
        }
    });
    start(&block);
    rx.recv_timeout(Duration::from_secs(5))
        .map_err(|_| VideoError::os(0, "ScreenCaptureKit didn't answer in 5 s"))?
}

impl Drop for StreamControl {
    fn drop(&mut self) {
        let stream = &self.stream;
        let _ = wait_completion(|block| {
            autoreleasepool(|_| {
                // SAFETY: a live stream; a block it copies.
                unsafe {
                    let _: () = msg_send![&**stream, stopCaptureWithCompletionHandler: block];
                }
            })
        });
        self.queue.drain();
        // SAFETY: the stream has stopped and the queue drained, so no
        // callback is using ctx.
        unsafe {
            if let Some(ivar) = self.handler.class().instance_variable(IVAR) {
                *ivar.load_ptr::<*mut c_void>(&self.handler) = std::ptr::null_mut();
            }
            drop(Box::from_raw(self.ctx));
        }
    }
}

/// The content filter and its size in pixels for a selection.
unsafe fn filter_for(
    sel_: &ScreenSelection,
    c: &CaptureConstraints,
) -> Result<(Retained<AnyObject>, u32, u32), VideoError> {
    let filter_cls =
        AnyClass::get(c"SCContentFilter").ok_or_else(|| unsupported("no SCContentFilter"))?;
    let (filter, w, h): (Retained<AnyObject>, f64, f64) = match sel_ {
        ScreenSelection::Picked(any) => {
            let p = any
                .downcast_ref::<PickedFilter>()
                .ok_or(VideoError::NotFound)?;
            (p.0 .0.clone(), 0.0, 0.0)
        }
        ScreenSelection::Listed(id) => {
            let content = shareable()?;
            if let Some(n) = id
                .strip_prefix("display:")
                .and_then(|n| n.parse::<u32>().ok())
            {
                let d = displays(&content)
                    .into_iter()
                    .find(|d| {
                        let x: u32 = msg_send![*d, displayID];
                        x == n
                    })
                    .ok_or(VideoError::NotFound)?;
                let (dw, dh): (isize, isize) = (msg_send![d, width], msg_send![d, height]);
                let empty = ffi::array(&[]);
                let alloc: Allocated<AnyObject> = msg_send![filter_cls, alloc];
                let f: Option<Retained<AnyObject>> = if c.exclude_own_windows {
                    let pid = std::process::id() as i32;
                    let apps: *mut AnyObject = msg_send![&*content, applications];
                    let own: Vec<ffi::CFTypeRef> = array_items(apps)
                        .into_iter()
                        .filter(|a| {
                            let p: i32 = msg_send![*a, processID];
                            p == pid
                        })
                        .map(|a| a as ffi::CFTypeRef)
                        .collect();
                    let own = ffi::array(&own);
                    msg_send![alloc, initWithDisplay: d, excludingApplications: own.0 as *const AnyObject, exceptingWindows: empty.0 as *const AnyObject]
                } else {
                    msg_send![alloc, initWithDisplay: d, excludingWindows: empty.0 as *const AnyObject]
                };
                (
                    f.ok_or_else(|| VideoError::os(0, "SCContentFilter for a display"))?,
                    dw as f64,
                    dh as f64,
                )
            } else if let Some(n) = id
                .strip_prefix("window:")
                .and_then(|n| n.parse::<u32>().ok())
            {
                let w = windows(&content)
                    .into_iter()
                    .find(|w| {
                        let x: u32 = msg_send![*w, windowID];
                        x == n
                    })
                    .ok_or(VideoError::NotFound)?;
                let frame: CGRect = msg_send![w, frame];
                let alloc: Allocated<AnyObject> = msg_send![filter_cls, alloc];
                let f: Option<Retained<AnyObject>> =
                    msg_send![alloc, initWithDesktopIndependentWindow: w];
                (
                    f.ok_or_else(|| VideoError::os(0, "SCContentFilter for a window"))?,
                    frame.w,
                    frame.h,
                )
            } else {
                return Err(VideoError::NotFound);
            }
        }
    };
    // The content's size in pixels (macOS 14 tells the filter's own).
    let (mut pw, mut ph, mut scale) = (w, h, 1.0f64);
    if super::objc::responds(&*filter, sel!(contentRect)) {
        let r: CGRect = msg_send![&*filter, contentRect];
        let s: f32 = msg_send![&*filter, pointPixelScale];
        (pw, ph, scale) = (r.w, r.h, s as f64);
    }
    let (pw, ph) = (
        (pw * scale).round().max(16.0) as u32,
        (ph * scale).round().max(16.0) as u32,
    );
    let (bw, bh) = (c.max_width.unwrap_or(1920), c.max_height.unwrap_or(1080));
    let (fw, fh) = fit_within(pw, ph, bw, bh);
    Ok((filter, (fw & !1).max(16), (fh & !1).max(16)))
}

pub(crate) fn open(
    sel_: &ScreenSelection,
    c: CaptureConstraints,
    wake: Wake,
) -> Result<Box<dyn FrameSource>, VideoError> {
    c.validate()?;
    if !loaded() {
        return Err(unsupported("ScreenCaptureKit needs macOS 12.3"));
    }
    if matches!(sel_, ScreenSelection::Listed(_)) && !preflight() {
        return Err(VideoError::NoPermission);
    }
    autoreleasepool(|_| unsafe { open_in_pool(sel_, c, wake) })
}

unsafe fn open_in_pool(
    sel_: &ScreenSelection,
    c: CaptureConstraints,
    wake: Wake,
) -> Result<Box<dyn FrameSource>, VideoError> {
    let (filter, width, height) = filter_for(sel_, &c)?;
    let fps = c.fps.unwrap_or(30);
    let cfg_cls = AnyClass::get(c"SCStreamConfiguration")
        .ok_or_else(|| unsupported("no SCStreamConfiguration"))?;
    let cfg: Option<Retained<AnyObject>> = msg_send![cfg_cls, new];
    let cfg = cfg.ok_or_else(|| VideoError::os(0, "SCStreamConfiguration new"))?;
    let _: () = msg_send![&*cfg, setWidth: width as usize];
    let _: () = msg_send![&*cfg, setHeight: height as usize];
    let _: () = msg_send![&*cfg, setMinimumFrameInterval: cm_time(1, fps as i32)];
    let _: () = msg_send![&*cfg, setPixelFormat: ffi::PIX_420V];
    let _: () = msg_send![&*cfg, setShowsCursor: Bool::new(c.cursor)];
    let _: () = msg_send![&*cfg, setQueueDepth: 5isize];

    let shared = SourceShared::new(c, fps, wake);
    let handler: Option<Retained<AnyObject>> = msg_send![stream_class(), new];
    let handler = ObjSend(handler.ok_or_else(|| VideoError::os(0, "stream output new"))?);
    let ctx = Box::into_raw(Box::new(Ctx {
        shared: shared.clone(),
        native: c.prefer_native,
    }));
    if let Some(ivar) = handler.class().instance_variable(IVAR) {
        *ivar.load_ptr::<*mut c_void>(&handler) = ctx as *mut c_void;
    }
    let stream_cls = AnyClass::get(c"SCStream").ok_or_else(|| unsupported("no SCStream"))?;
    let alloc: Allocated<AnyObject> = msg_send![stream_cls, alloc];
    let stream: Option<Retained<AnyObject>> =
        msg_send![alloc, initWithFilter: &*filter, configuration: &*cfg, delegate: &*handler];
    let Some(stream) = stream else {
        drop(Box::from_raw(ctx));
        return Err(VideoError::os(0, "SCStream init"));
    };
    let queue = Queue::new(c"kessel-video.screen");
    let mut err: *mut AnyObject = std::ptr::null_mut();
    let added: Bool = msg_send![&*stream, addStreamOutput: &*handler, type: 0isize, sampleHandlerQueue: queue.0, error: &mut err];
    if !added.as_bool() {
        drop(Box::from_raw(ctx));
        return Err(sck_error(err));
    }
    let stream = ObjSend(stream);
    let started = wait_completion(|block| {
        let _: () = msg_send![&*stream, startCaptureWithCompletionHandler: block];
    });
    let control = StreamControl {
        stream,
        handler,
        queue,
        ctx,
    };
    started?;
    Ok(Box::new(QueuedSource::new(shared, control)))
}
