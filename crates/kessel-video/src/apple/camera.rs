//! Cameras through AVFoundation, on macOS and iOS: the list, the
//! permission, and capture sessions that deliver NV12 frames to a
//! [`SourceShared`]. On iOS the front camera is the default, frames are
//! turned upright and left unmirrored, and interruptions (a phone call, the
//! app in the background, another app taking the camera) are source events.
//!
//! A source ends with `Ended(DeviceLost)` when its device goes away
//! (`AVCaptureDeviceWasDisconnectedNotification`, or `isConnected` turning
//! false) or its session dies: a runtime error on macOS, or a session that
//! stopped without an interruption on either OS. [`Life`] decides, from
//! those signals, which events a source sends.

use super::ffi::{self, cf_const, cm_time, FormatDescriptionArg, SampleBufferArg};
#[cfg(target_os = "ios")]
use super::ios::Rotation;
use super::objc::{
    array_items, error_of, info_plist_has, nsstring, responds, string_of, user_info_integer,
    user_info_object, ObjSend, Observer, Queue,
};
use super::pixel::{cpu_frame, native_frame};
use crate::api::{Done, FrameSource, Wake};
use crate::error::VideoError;
use crate::frame::PixelFormat;
use crate::kit::{lock, now_us, QueuedSource, SourceControl, SourceShared};
use crate::types::*;
use block2::RcBlock;
use objc2::rc::{autoreleasepool, Retained};
use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel};
use objc2::{class, msg_send, sel};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Duration;

// AVFoundation's classes are looked up at run time, so the framework must be
// loaded. Reading one of its constants by name keeps it in the app: a
// linker drops an auto-linked framework (`autolink!`) that no symbol is
// taken from.
#[link(name = "AVFoundation", kind = "framework")]
extern "C" {
    static AVMediaTypeVideo: *const AnyObject;
}

autolink!(
    "AVFoundation",
    "CoreMedia",
    "CoreVideo",
    "CoreFoundation",
    "Foundation"
);

fn media_type_video() -> Option<*const AnyObject> {
    // SAFETY: an NSString constant AVFoundation exports (iOS 4, macOS 10.7),
    // set before any code of ours runs and never written.
    let p = unsafe { AVMediaTypeVideo };
    (!p.is_null()).then_some(p)
}

/// `AVAuthorizationStatus` for video, raw.
fn status_raw() -> isize {
    let Some(mt) = media_type_video() else {
        return 2;
    };
    // SAFETY: a class method taking an AVMediaType and returning NSInteger;
    // it doesn't prompt.
    unsafe { msg_send![class!(AVCaptureDevice), authorizationStatusForMediaType: mt] }
}

fn can_ask() -> bool {
    info_plist_has("NSCameraUsageDescription")
}

pub(crate) fn permission() -> Permission {
    match status_raw() {
        3 => Permission::Granted,
        1 => Permission::Restricted,
        // Never asked, but without a usage description asking would end
        // the app: report Denied rather than crash.
        0 if can_ask() => Permission::NotDetermined,
        _ => Permission::Denied,
    }
}

pub(crate) fn request_permission(done: Done<Permission>) {
    let now = permission();
    if now != Permission::NotDetermined {
        return done(now);
    }
    let Some(mt) = media_type_video() else {
        return done(Permission::Denied);
    };
    let slot = Mutex::new(Some(done));
    let block = RcBlock::new(move |granted: Bool| {
        if let Some(done) = slot.lock().unwrap_or_else(|e| e.into_inner()).take() {
            done(if granted.as_bool() {
                Permission::Granted
            } else {
                Permission::Denied
            });
        }
    });
    autoreleasepool(|_| {
        // SAFETY: a class method taking an AVMediaType and a `void (^)(BOOL)`
        // block, which it copies; it returns at once.
        unsafe {
            let _: () = msg_send![class!(AVCaptureDevice), requestAccessForMediaType: mt, completionHandler: &*block];
        }
    });
}

/// The device types to discover (those this OS has).
fn device_types() -> ffi::Cf {
    let mut types = Vec::new();
    for name in [
        c"AVCaptureDeviceTypeBuiltInWideAngleCamera",
        c"AVCaptureDeviceTypeExternal",
        c"AVCaptureDeviceTypeContinuityCamera",
    ] {
        if let Some(t) = cf_const(name) {
            types.push(t);
        }
    }
    if cf_const(c"AVCaptureDeviceTypeExternal").is_none() {
        if let Some(t) = cf_const(c"AVCaptureDeviceTypeExternalUnknown") {
            types.push(t);
        }
    }
    ffi::array(&types)
}

/// The video devices now (autoreleased; call inside a pool).
unsafe fn devices() -> Vec<*mut AnyObject> {
    let Some(mt) = media_type_video() else {
        return Vec::new();
    };
    let types = device_types();
    let Some(cls) = AnyClass::get(c"AVCaptureDeviceDiscoverySession") else {
        let all: *mut AnyObject = msg_send![class!(AVCaptureDevice), devicesWithMediaType: mt];
        return array_items(all);
    };
    let session: *mut AnyObject = msg_send![
        cls,
        discoverySessionWithDeviceTypes: types.0 as *const AnyObject,
        mediaType: mt,
        position: 0isize
    ];
    if session.is_null() {
        return Vec::new();
    }
    let list: *mut AnyObject = msg_send![session, devices];
    array_items(list)
}

/// Which of two formats of one size to keep: the faster. On iOS, a format up
/// to 60 fps beats a high-speed (slow-motion) one, which can be binned or
/// cropped.
fn fps_rank(fps: u32) -> (bool, u32) {
    (!cfg!(target_os = "ios") || fps <= 60, fps)
}

/// A device's modes with the format objects behind them.
unsafe fn modes(device: *mut AnyObject) -> Vec<(CaptureMode, *mut AnyObject)> {
    let formats: *mut AnyObject = msg_send![device, formats];
    let mut out: Vec<(CaptureMode, *mut AnyObject)> = Vec::new();
    for f in array_items(formats) {
        let desc: FormatDescriptionArg = msg_send![f, formatDescription];
        if desc.0.is_null() {
            continue;
        }
        let d = ffi::CMVideoFormatDescriptionGetDimensions(desc.0);
        let ranges: *mut AnyObject = msg_send![f, videoSupportedFrameRateRanges];
        let mut max_fps = 0f64;
        for r in array_items(ranges) {
            let m: f64 = msg_send![r, maxFrameRate];
            max_fps = max_fps.max(m);
        }
        if d.width <= 0 || d.height <= 0 || max_fps < 1.0 {
            continue;
        }
        let mode = CaptureMode::new(
            d.width as u32,
            d.height as u32,
            max_fps.round() as u32,
            PixelFormat::Nv12,
        );
        match out
            .iter_mut()
            .find(|(m, _)| (m.width, m.height) == (mode.width, mode.height))
        {
            Some(slot) if fps_rank(slot.0.max_fps) < fps_rank(mode.max_fps) => *slot = (mode, f),
            Some(_) => {}
            None => out.push((mode, f)),
        }
    }
    out.sort_by_key(|(m, _)| std::cmp::Reverse((m.width as u64 * m.height as u64, m.max_fps)));
    out
}

unsafe fn is_equal(a: *mut AnyObject, b: *const AnyObject) -> bool {
    if a.is_null() || b.is_null() {
        return false;
    }
    let r: Bool = msg_send![a, isEqual: b];
    r.as_bool()
}

unsafe fn info_of(device: *mut AnyObject, default_id: &str) -> CameraInfo {
    let id: *mut AnyObject = msg_send![device, uniqueID];
    let name: *mut AnyObject = msg_send![device, localizedName];
    let id = string_of(id);
    let mut info = CameraInfo::new(
        id.clone(),
        string_of(name),
        facing_of(device),
        id == default_id,
    );
    info.modes = modes(device).into_iter().map(|(m, _)| m).collect();
    info.suspended = is_suspended(device);
    info
}

/// `AVCaptureDevice.isSuspended`: the device can't deliver frames now (a
/// MacBook's built-in camera with the lid closed, a shut lens cover). False
/// where the OS doesn't answer.
///
/// # Safety
/// `device` must be a live AVCaptureDevice.
pub(crate) unsafe fn is_suspended(device: *const AnyObject) -> bool {
    if !responds(device, sel!(isSuspended)) {
        return false;
    }
    let b: Bool = msg_send![device, isSuspended];
    b.as_bool()
}

unsafe fn facing_of(device: *mut AnyObject) -> Facing {
    let ty: *mut AnyObject = msg_send![device, deviceType];
    let position: isize = msg_send![device, position];
    let external = [
        c"AVCaptureDeviceTypeExternal",
        c"AVCaptureDeviceTypeExternalUnknown",
        c"AVCaptureDeviceTypeContinuityCamera",
    ]
    .iter()
    .any(|n| cf_const(n).is_some_and(|t| is_equal(ty, t as *const AnyObject)));
    let built_in = cf_const(c"AVCaptureDeviceTypeBuiltInWideAngleCamera")
        .is_some_and(|t| is_equal(ty, t as *const AnyObject));
    match position {
        1 => Facing::Back,
        2 => Facing::Front,
        _ if external => Facing::External,
        _ if built_in => Facing::Front,
        _ => Facing::Unknown,
    }
}

/// The first camera facing `facing` (autoreleased; call inside a pool), or
/// null.
unsafe fn facing_device(facing: Facing) -> *mut AnyObject {
    devices()
        .into_iter()
        .find(|d| facing_of(*d) == facing)
        .unwrap_or(std::ptr::null_mut())
}

/// The default camera (autoreleased; call inside a pool), or null. On iOS
/// the front camera, since calls face the person; elsewhere the system's
/// default.
unsafe fn default_device() -> *mut AnyObject {
    if cfg!(target_os = "ios") {
        let d = facing_device(Facing::Front);
        if !d.is_null() {
            return d;
        }
    }
    let Some(mt) = media_type_video() else {
        return std::ptr::null_mut();
    };
    msg_send![class!(AVCaptureDevice), defaultDeviceWithMediaType: mt]
}

unsafe fn default_id() -> String {
    let d = default_device();
    if d.is_null() {
        return String::new();
    }
    let id: *mut AnyObject = msg_send![d, uniqueID];
    string_of(id)
}

pub(crate) fn cameras() -> Vec<CameraInfo> {
    autoreleasepool(|_| {
        // SAFETY: AVFoundation calls with documented signatures, inside a
        // pool that keeps the autoreleased results alive.
        unsafe {
            let def = default_id();
            devices().into_iter().map(|d| info_of(d, &def)).collect()
        }
    })
}

// ───────────────────────────── capture ─────────────────────────────

struct Ctx {
    shared: Arc<SourceShared>,
    native: bool,
}

const IVAR: &std::ffi::CStr = c"kesselVideoCtx";

unsafe extern "C-unwind" fn did_output(
    this: *mut AnyObject,
    _sel: Sel,
    _output: *mut AnyObject,
    sb: SampleBufferArg,
    _conn: *mut AnyObject,
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
        if ctx.shared.idle() {
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

fn delegate_class() -> &'static AnyClass {
    static CLS: OnceLock<&'static AnyClass> = OnceLock::new();
    CLS.get_or_init(|| {
        let mut b = ClassBuilder::new(c"KesselVideoCameraDelegate", class!(NSObject))
            .expect("the delegate class name is free");
        b.add_ivar::<*mut c_void>(IVAR);
        if let Some(p) =
            objc2::runtime::AnyProtocol::get(c"AVCaptureVideoDataOutputSampleBufferDelegate")
        {
            b.add_protocol(p);
        }
        // SAFETY: the signature AVFoundation calls:
        // captureOutput:didOutputSampleBuffer:fromConnection:.
        unsafe {
            b.add_method(
                sel!(captureOutput:didOutputSampleBuffer:fromConnection:),
                did_output
                    as unsafe extern "C-unwind" fn(
                        *mut AnyObject,
                        Sel,
                        *mut AnyObject,
                        SampleBufferArg,
                        *mut AnyObject,
                    ),
            );
        }
        b.register()
    })
}

struct CameraControl {
    session: ObjSend,
    output: ObjSend,
    delegate: ObjSend,
    queue: Queue,
    ctx: *mut Ctx,
    stop: Arc<AtomicBool>,
    watch: Option<JoinHandle<()>>,
    life: Arc<Mutex<Life>>,
    observers: Vec<Observer>,
}

// SAFETY: the objects are thread-safe AVFoundation objects; ctx is freed
// only in Drop, after the queue has drained.
unsafe impl Send for CameraControl {}

impl SourceControl for CameraControl {}

impl Drop for CameraControl {
    fn drop(&mut self) {
        // No events once the caller is closing it. Closing under the lock
        // waits for an observer block that is sending one now, and a block
        // that still runs after removeObserver (it can, on another thread)
        // sends nothing.
        lock(&self.life).close();
        self.observers.clear();
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.watch.take() {
            let _ = t.join();
        }
        autoreleasepool(|_| {
            // SAFETY: live objects; stopRunning blocks until the session has
            // stopped; then no new callbacks are queued, and draining the
            // queue finishes any in flight before ctx is freed.
            unsafe {
                let _: () = msg_send![&*self.session, stopRunning];
                let nil: *mut AnyObject = std::ptr::null_mut();
                let _: () = msg_send![&*self.output, setSampleBufferDelegate: nil, queue: nil];
                self.queue.drain();
                if let Some(ivar) = self.delegate.class().instance_variable(IVAR) {
                    *ivar.load_ptr::<*mut c_void>(&self.delegate) = std::ptr::null_mut();
                }
                drop(Box::from_raw(self.ctx));
            }
        });
    }
}

/// `AVErrorMediaServicesWereReset`: iOS restarted its media services; the
/// session can be started again.
pub(crate) const MEDIA_SERVICES_RESET: i64 = -11819;

pub(crate) fn av_error(e: *mut AnyObject, what: &str) -> VideoError {
    // SAFETY: nil or an NSError.
    let (code, desc) = unsafe { error_of(e) };
    av_error_code(code, &format!("{what}: {desc}"))
}

/// The error for an `AVError` code.
pub(crate) fn av_error_code(code: i64, detail: &str) -> VideoError {
    match code {
        // AVErrorApplicationIsNotAuthorizedToUseDevice
        -11852 => VideoError::NoPermission,
        // AVErrorDeviceAlreadyUsedByAnotherSession,
        // AVErrorDeviceInUseByAnotherApplication
        -11804 | -11815 => VideoError::DeviceBusy,
        // AVErrorDeviceWasDisconnected, AVErrorDeviceNotConnected
        -11808 | -11814 => VideoError::DeviceLost,
        _ => VideoError::os(code, detail.to_string()),
    }
}

/// The interruption for an `AVCaptureSessionInterruptionReason` (iOS).
pub(crate) fn interruption_for_reason(reason: Option<isize>) -> Interruption {
    match reason {
        // VideoDeviceNotAvailableInBackground, and
        // VideoDeviceNotAvailableWithMultipleForegroundApps (iPad
        // multitasking): the app isn't the one the camera serves now.
        Some(1) | Some(4) => Interruption::Background,
        // VideoDeviceNotAvailableDueToSystemPressure (heat, power).
        Some(5) => Interruption::Thermal,
        // Another client has the audio or video device (a phone or FaceTime
        // call, another app), and any reason newer than this code.
        _ => Interruption::InUseByAnother,
    }
}

// ───────────────────────────── a source's life ─────────────────────────────

/// What a camera source hears about its device and session: the
/// notification observers' signals, and the watch thread's look every
/// 250 ms.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Signal {
    /// `AVCaptureDeviceWasDisconnectedNotification` for the source's device.
    Disconnected,
    /// `AVCaptureSessionRuntimeErrorNotification`: the `AVError` code and
    /// the error's description.
    RuntimeError(i64, String),
    /// `AVCaptureSessionWasInterruptedNotification` (iOS), with its
    /// `AVCaptureSessionInterruptionReason` when it has one.
    Interrupted(Option<isize>),
    /// `AVCaptureSessionInterruptionEndedNotification` (iOS).
    InterruptionEnded,
    /// The watch thread's look at the device and the session.
    Poll {
        /// `AVCaptureDevice.isConnected`.
        connected: bool,
        /// `AVCaptureSession.isRunning`.
        running: bool,
        /// `AVCaptureSession.isInterrupted` (iOS; false on macOS).
        interrupted: bool,
        /// `AVCaptureDevice.isSuspended`.
        suspended: bool,
    },
}

/// What the watch thread does after a signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Next {
    /// Keep watching.
    Watch,
    /// Start the session again (iOS reset its media services), then keep
    /// watching: a session that still doesn't run ends the source.
    Restart,
    /// The source ended or was closed: stop watching.
    Stop,
}

/// A camera source's life: which events its signals send. The observers
/// (on AVFoundation's threads) and the watch thread share one behind a
/// mutex, so events go out in order, `Ended` once, and nothing after the
/// source is closed.
///
/// - The device going away, or the session dying, is the end:
///   `Ended(DeviceLost)`. A runtime error says so at once on macOS (the
///   session stops on one, and macOS has no media-services reset) and on
///   either OS when its code says the device is gone; otherwise a session
///   that stopped without an interruption says so at the next look.
/// - A pause is not the end. An iOS interruption (a phone call, the app in
///   the background, multitasking, heat) sends `Interrupted(why)` and later
///   `Resumed`; so does a device that reports itself suspended
///   (`Interrupted(Suspended)`: a closed laptop lid).
/// - iOS resetting its media services sends the runtime error as `Error`
///   and starts the session once more.
#[derive(Debug)]
pub(crate) struct Life {
    phone: bool,
    /// The session's interruption (iOS notifications).
    interruption: Option<Interruption>,
    /// The device says it is suspended.
    suspended: bool,
    /// iOS reset its media services: start the session again, once.
    media_reset: bool,
    /// `Ended` was sent, or the source was closed.
    done: bool,
}

impl Life {
    /// A running source's life: `phone` for iOS's rules.
    pub(crate) fn new(phone: bool) -> Life {
        Life {
            phone,
            interruption: None,
            suspended: false,
            media_reset: false,
            done: false,
        }
    }

    /// The caller is closing the source: no more events.
    pub(crate) fn close(&mut self) {
        self.done = true;
    }

    fn paused(&self) -> bool {
        self.interruption.is_some() || self.suspended
    }

    fn end(&mut self, send: &mut dyn FnMut(SourceEvent)) -> Next {
        self.done = true;
        send(SourceEvent::Ended(EndReason::DeviceLost));
        Next::Stop
    }

    /// Take a signal; `send` gets the events it makes.
    pub(crate) fn on(&mut self, signal: Signal, send: &mut dyn FnMut(SourceEvent)) -> Next {
        if self.done {
            return Next::Stop;
        }
        let was_paused = self.paused();
        let next = match signal {
            Signal::Disconnected => return self.end(send),
            Signal::RuntimeError(code, detail) => {
                let e = av_error_code(code, &format!("capture session: {detail}"));
                let lost = e == VideoError::DeviceLost;
                send(SourceEvent::Error(e));
                if self.phone && code == MEDIA_SERVICES_RESET {
                    self.media_reset = true;
                } else if lost || !self.phone {
                    return self.end(send);
                }
                Next::Watch
            }
            Signal::Interrupted(reason) => {
                if self.interruption.is_none() {
                    self.interruption = Some(interruption_for_reason(reason));
                }
                Next::Watch
            }
            Signal::InterruptionEnded => {
                self.interruption = None;
                Next::Watch
            }
            Signal::Poll {
                connected,
                running,
                interrupted,
                suspended,
            } => {
                if !connected {
                    return self.end(send);
                }
                if !running && !interrupted {
                    if std::mem::take(&mut self.media_reset) {
                        return Next::Restart;
                    }
                    return self.end(send);
                }
                self.suspended = suspended;
                Next::Watch
            }
        };
        match (was_paused, self.paused()) {
            (false, true) => send(SourceEvent::Interrupted(
                self.interruption.unwrap_or(Interruption::Suspended),
            )),
            (true, false) => send(SourceEvent::Resumed),
            _ => {}
        }
        next
    }
}

/// Hand `signal` to `life`, sending its events to the source.
pub(crate) fn signal(life: &Mutex<Life>, shared: &SourceShared, signal: Signal) -> Next {
    lock(life).on(signal, &mut |e| shared.event(e))
}

/// The names of the notifications a camera source observes (NSStrings).
/// AVFoundation's in a source; tests post their own names, so that
/// AVFoundation's own observers never see a stand-in object.
pub(crate) struct Notices {
    /// `AVCaptureDeviceWasDisconnectedNotification`.
    pub(crate) disconnected: Option<*const AnyObject>,
    /// `AVCaptureSessionRuntimeErrorNotification`.
    pub(crate) runtime_error: Option<*const AnyObject>,
    /// `AVCaptureSessionWasInterruptedNotification` (iOS).
    pub(crate) interrupted: Option<*const AnyObject>,
    /// `AVCaptureSessionInterruptionEndedNotification` (iOS).
    pub(crate) interruption_ended: Option<*const AnyObject>,
}

impl Notices {
    /// AVFoundation's names, looked up by symbol; `None` for one the OS
    /// lacks. The interruption ones only on iOS.
    pub(crate) fn avfoundation() -> Notices {
        let name = |n: &std::ffi::CStr| cf_const(n).map(|p| p as *const AnyObject);
        let phone = |n: &std::ffi::CStr| {
            if cfg!(target_os = "ios") {
                name(n)
            } else {
                None
            }
        };
        Notices {
            disconnected: name(c"AVCaptureDeviceWasDisconnectedNotification"),
            runtime_error: name(c"AVCaptureSessionRuntimeErrorNotification"),
            interrupted: phone(c"AVCaptureSessionWasInterruptedNotification"),
            interruption_ended: phone(c"AVCaptureSessionInterruptionEndedNotification"),
        }
    }
}

/// Observers for the device's disconnection and the session's runtime
/// errors, and on iOS its interruptions, under `names`. Dropping them
/// removes them.
///
/// # Safety
/// `session` and `device` must be live objects (the session and its
/// device), and each name a live NSString.
pub(crate) unsafe fn observers(
    session: &AnyObject,
    device: &AnyObject,
    life: &Arc<Mutex<Life>>,
    shared: &Arc<SourceShared>,
    names: &Notices,
) -> Vec<Observer> {
    let mut out = Vec::new();
    if let Some(name) = names.disconnected {
        let (l, s) = (life.clone(), shared.clone());
        out.extend(Observer::named(name, device, move |_| {
            signal(&l, &s, Signal::Disconnected);
        }));
    }
    if let Some(name) = names.runtime_error {
        let (l, s) = (life.clone(), shared.clone());
        out.extend(Observer::named(name, session, move |n| {
            // SAFETY: Foundation hands the block an NSNotification; its
            // error is nil or an NSError.
            let (code, detail) =
                unsafe { error_of(user_info_object(n, c"AVCaptureSessionErrorKey")) };
            signal(&l, &s, Signal::RuntimeError(code, detail));
        }));
    }
    if let Some(name) = names.interrupted {
        let (l, s) = (life.clone(), shared.clone());
        out.extend(Observer::named(name, session, move |n| {
            // SAFETY: Foundation hands the block an NSNotification.
            let reason =
                unsafe { user_info_integer(n, c"AVCaptureSessionInterruptionReasonKey") };
            signal(&l, &s, Signal::Interrupted(reason));
        }));
    }
    if let Some(name) = names.interruption_ended {
        let (l, s) = (life.clone(), shared.clone());
        out.extend(Observer::named(name, session, move |_| {
            signal(&l, &s, Signal::InterruptionEnded);
        }));
    }
    out
}

/// The watch thread's look at the device and the session.
///
/// # Safety
/// `device` and `session` must be live.
unsafe fn poll(device: &AnyObject, session: &AnyObject) -> Signal {
    let connected: Bool = msg_send![device, isConnected];
    let running: Bool = msg_send![session, isRunning];
    Signal::Poll {
        connected: connected.as_bool(),
        running: running.as_bool(),
        interrupted: interrupted(session),
        suspended: is_suspended(device),
    }
}

/// `AVCaptureVideoOrientation` for a `UIDeviceOrientation`: the same number
/// for the upright and the two sideways ones (UIKit's landscape left is the
/// capture's landscape right, and the two enums are numbered to match),
/// `None` face up, face down or unknown.
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub(crate) fn video_orientation_for(device_orientation: isize) -> Option<isize> {
    (1..=4)
        .contains(&device_orientation)
        .then_some(device_orientation)
}

/// The constraints a phone's (landscape) sensor modes are chosen by: the
/// asked box turned landscape. Frames are then turned upright and fitted to
/// the box turned the same way.
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub(crate) fn landscape_box(c: CaptureConstraints) -> CaptureConstraints {
    let mut out = c;
    if let (Some(w), Some(h)) = (c.max_width, c.max_height) {
        if w < h {
            out.max_width = Some(h);
            out.max_height = Some(w);
        }
    }
    out
}

pub(crate) fn open(
    id: Option<&str>,
    c: CaptureConstraints,
    wake: Wake,
) -> Result<Box<dyn FrameSource>, VideoError> {
    c.validate()?;
    if permission() != Permission::Granted {
        return Err(VideoError::NoPermission);
    }
    // iOS gives no camera to an app in the background.
    #[cfg(target_os = "ios")]
    if super::ios::in_background() {
        return Err(VideoError::ShutDown);
    }
    autoreleasepool(|_| unsafe { open_in_pool(id, c, wake) })
}

unsafe fn open_in_pool(
    id: Option<&str>,
    c: CaptureConstraints,
    wake: Wake,
) -> Result<Box<dyn FrameSource>, VideoError> {
    let device: *mut AnyObject = match (id, c.facing) {
        (Some(id), _) => {
            let s = nsstring(id).ok_or(VideoError::NotFound)?;
            msg_send![class!(AVCaptureDevice), deviceWithUniqueID: &*s]
        }
        (None, Some(facing)) => match facing_device(facing) {
            d if d.is_null() => default_device(),
            d => d,
        },
        (None, None) => default_device(),
    };
    if device.is_null() {
        return Err(VideoError::NotFound);
    }
    let device = ObjSend(Retained::retain(device).ok_or(VideoError::NotFound)?);
    let all = modes(Retained::as_ptr(&device.0) as *mut AnyObject);
    let list: Vec<CaptureMode> = all.iter().map(|(m, _)| *m).collect();
    let by = if cfg!(target_os = "ios") {
        landscape_box(c)
    } else {
        c
    };
    let chosen = nearest_mode(&list, &by);
    let format = chosen.and_then(|m| all.iter().find(|(x, _)| *x == m).map(|(_, f)| *f));
    let fps = match (chosen, c.fps) {
        (Some(m), Some(f)) => f.min(m.max_fps).max(1),
        (Some(m), None) => m.max_fps.min(30),
        (None, f) => f.unwrap_or(30),
    };

    let session: Option<Retained<AnyObject>> = msg_send![class!(AVCaptureSession), new];
    let session = ObjSend(session.ok_or_else(|| VideoError::os(0, "AVCaptureSession new"))?);
    let mut err: *mut AnyObject = std::ptr::null_mut();
    let input: *mut AnyObject =
        msg_send![class!(AVCaptureDeviceInput), deviceInputWithDevice: &*device, error: &mut err];
    if input.is_null() {
        return Err(av_error(err, "AVCaptureDeviceInput"));
    }
    // On iOS the session would otherwise take over the app's audio session
    // (the property defaults to YES). In a call, CallKit owns the audio
    // session, so the camera must leave it alone. The session stays
    // video-only: no audio input is ever added.
    #[cfg(target_os = "ios")]
    {
        let _: () =
            msg_send![&*session, setAutomaticallyConfiguresApplicationAudioSession: Bool::NO];
    }
    let _: () = msg_send![&*session, beginConfiguration];
    let ok: Bool = msg_send![&*session, canAddInput: input];
    if !ok.as_bool() {
        let _: () = msg_send![&*session, commitConfiguration];
        return Err(VideoError::DeviceBusy);
    }
    let _: () = msg_send![&*session, addInput: input];

    let output: Option<Retained<AnyObject>> = msg_send![class!(AVCaptureVideoDataOutput), new];
    let output = ObjSend(output.ok_or_else(|| VideoError::os(0, "AVCaptureVideoDataOutput new"))?);
    let settings = ffi::dict();
    ffi::set_named(
        &settings,
        c"kCVPixelBufferPixelFormatTypeKey",
        ffi::num_i32(ffi::PIX_420V as i32).0,
    );
    let _: () = msg_send![&*output, setVideoSettings: settings.0 as *const AnyObject];
    let _: () = msg_send![&*output, setAlwaysDiscardsLateVideoFrames: Bool::YES];

    let shared = SourceShared::new(c, fps, wake);
    // Frames come out upright, so a held-upright phone's are portrait.
    #[cfg(target_os = "ios")]
    shared.set_box_follows_frame(true);
    let ctx = Box::into_raw(Box::new(Ctx {
        shared: shared.clone(),
        native: c.prefer_native,
    }));
    let delegate: Option<Retained<AnyObject>> = msg_send![delegate_class(), new];
    let Some(delegate) = delegate else {
        drop(Box::from_raw(ctx));
        let _: () = msg_send![&*session, commitConfiguration];
        return Err(VideoError::os(0, "camera delegate new"));
    };
    let delegate = ObjSend(delegate);
    if let Some(ivar) = delegate.class().instance_variable(IVAR) {
        *ivar.load_ptr::<*mut c_void>(&delegate) = ctx as *mut c_void;
    }
    let queue = Queue::new(c"kessel-video.camera");
    let _: () = msg_send![&*output, setSampleBufferDelegate: &*delegate, queue: queue.0];
    // From here on, CameraControl's Drop tears everything down.
    let mut control = CameraControl {
        session,
        output,
        delegate,
        queue,
        ctx,
        stop: Arc::new(AtomicBool::new(false)),
        watch: None,
        life: Arc::new(Mutex::new(Life::new(cfg!(target_os = "ios")))),
        observers: Vec::new(),
    };
    let ok: Bool = msg_send![&*control.session, canAddOutput: &*control.output];
    if !ok.as_bool() {
        let _: () = msg_send![&*control.session, commitConfiguration];
        return Err(VideoError::os(
            0,
            "the capture session won't take a video output",
        ));
    }
    let _: () = msg_send![&*control.session, addOutput: &*control.output];
    let _: () = msg_send![&*control.session, commitConfiguration];

    // The mode and rate: after the input is in the session, or the
    // session's preset would replace them.
    if let Some(format) = format {
        let mut err: *mut AnyObject = std::ptr::null_mut();
        let locked: Bool = msg_send![&*device, lockForConfiguration: &mut err];
        if locked.as_bool() {
            let _: () = msg_send![&*device, setActiveFormat: format];
            let frame = cm_time(1, fps as i32);
            let ranges: *mut AnyObject = msg_send![format, videoSupportedFrameRateRanges];
            let fits = array_items(ranges).into_iter().any(|r| {
                let lo: f64 = msg_send![r, minFrameRate];
                let hi: f64 = msg_send![r, maxFrameRate];
                (lo - 0.01..=hi + 0.01).contains(&(fps as f64))
            });
            if fits {
                let _: () = msg_send![&*device, setActiveVideoMinFrameDuration: frame];
                let _: () = msg_send![&*device, setActiveVideoMaxFrameDuration: frame];
            }
            let _: () = msg_send![&*device, unlockForConfiguration];
        } else {
            shared.event(SourceEvent::Error(av_error(err, "lockForConfiguration")));
        }
    }

    // iOS: upright, unmirrored frames.
    #[cfg(target_os = "ios")]
    let phone = {
        let conn = phone::video_connection(&control.output);
        let rotation = Rotation::new(&device);
        if let Some(conn) = &conn {
            phone::unmirror(conn);
            if let Some(t) = rotation.turn() {
                phone::apply_turn(&control.session, conn, t);
            }
        }
        phone::Watch {
            conn,
            rotation,
            last: None,
        }
    };

    // The device going away and the session's runtime errors (and on iOS
    // its interruptions), as they happen.
    control.observers = observers(
        &control.session,
        &device,
        &control.life,
        &shared,
        &Notices::avfoundation(),
    );

    let _: () = msg_send![&*control.session, startRunning];
    let running: Bool = msg_send![&*control.session, isRunning];
    if !running.as_bool() && !interrupted(&control.session) {
        return Err(VideoError::DeviceBusy);
    }

    // And a look every 250 ms: unplugged, the session stopped, suspended.
    let (stop, life, s, dev, sess) = (
        control.stop.clone(),
        control.life.clone(),
        shared.clone(),
        device,
        ObjSend(control.session.0.clone()),
    );
    control.watch = Some(
        std::thread::Builder::new()
            .name("kessel-video-camera-watch".into())
            .spawn(move || {
                #[cfg(target_os = "ios")]
                let mut phone = phone;
                while !stop.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_millis(250));
                    #[cfg(target_os = "ios")]
                    phone.tick(&sess);
                    // SAFETY: live, thread-safe AVFoundation objects.
                    let look = autoreleasepool(|_| unsafe { poll(&dev, &sess) });
                    if stop.load(Ordering::Acquire) {
                        return;
                    }
                    match signal(&life, &s, look) {
                        Next::Watch => {}
                        // SAFETY: the live session.
                        Next::Restart => autoreleasepool(|_| unsafe { start(&sess) }),
                        Next::Stop => return,
                    }
                }
            })
            .map_err(|e| {
                VideoError::os(e.raw_os_error().unwrap_or(0) as i64, format!("spawn: {e}"))
            })?,
    );
    Ok(Box::new(QueuedSource::new(shared, control)))
}

/// Start the session again (after iOS reset its media services). It blocks
/// until the session runs or fails; the next look decides which.
///
/// # Safety
/// `session` must be a live AVCaptureSession.
unsafe fn start(session: &AnyObject) {
    let _: () = msg_send![session, startRunning];
}

/// Is the session interrupted (iOS: a call, the background, another app)?
/// Always false on macOS.
unsafe fn interrupted(session: &AnyObject) -> bool {
    if cfg!(target_os = "ios") && responds(session, sel!(isInterrupted)) {
        let b: Bool = msg_send![session, isInterrupted];
        b.as_bool()
    } else {
        false
    }
}

/// iOS: the capture connection's rotation and mirroring.
#[cfg(target_os = "ios")]
mod phone {
    use super::super::ios::{Rotation, Turn};
    use super::super::objc::{responds, ObjSend};
    use super::media_type_video;
    use objc2::rc::{autoreleasepool, Retained};
    use objc2::runtime::{AnyObject, Bool};
    use objc2::{msg_send, sel};

    /// The video output's connection.
    ///
    /// # Safety
    /// `output` must be a live AVCaptureVideoDataOutput.
    pub(super) unsafe fn video_connection(output: &AnyObject) -> Option<ObjSend> {
        let mt = media_type_video()?;
        let c: Option<Retained<AnyObject>> = msg_send![output, connectionWithMediaType: mt];
        c.map(ObjSend)
    }

    /// The far end sees the picture as the camera does; mirroring a front
    /// camera's self-view is the app's choice, in its preview.
    ///
    /// # Safety
    /// `conn` must be a live AVCaptureConnection.
    pub(super) unsafe fn unmirror(conn: &AnyObject) {
        let ok: Bool = msg_send![conn, isVideoMirroringSupported];
        if ok.as_bool() {
            let _: () = msg_send![conn, setAutomaticallyAdjustsVideoMirroring: Bool::NO];
            let _: () = msg_send![conn, setVideoMirrored: Bool::NO];
        }
    }

    /// Turn the connection's frames upright.
    ///
    /// # Safety
    /// `session` and `conn` must be the live session and its connection.
    pub(super) unsafe fn apply_turn(session: &AnyObject, conn: &AnyObject, turn: Turn) {
        let _: () = msg_send![session, beginConfiguration];
        match turn {
            Turn::Angle(a) => {
                if responds(conn, sel!(isVideoRotationAngleSupported:)) {
                    let ok: Bool = msg_send![conn, isVideoRotationAngleSupported: a];
                    if ok.as_bool() {
                        let _: () = msg_send![conn, setVideoRotationAngle: a];
                    }
                }
            }
            Turn::Orientation(o) => {
                let ok: Bool = msg_send![conn, isVideoOrientationSupported];
                if ok.as_bool() {
                    let _: () = msg_send![conn, setVideoOrientation: o];
                }
            }
        }
        let _: () = msg_send![session, commitConfiguration];
    }

    /// What the watch thread keeps for a phone camera.
    pub(super) struct Watch {
        pub(super) conn: Option<ObjSend>,
        pub(super) rotation: Rotation,
        pub(super) last: Option<Turn>,
    }

    impl Watch {
        /// Follow the way the phone is held.
        pub(super) fn tick(&mut self, session: &ObjSend) {
            self.rotation.refresh();
            let (Some(conn), Some(turn)) = (&self.conn, self.rotation.turn()) else {
                return;
            };
            if self.last == Some(turn) {
                return;
            }
            self.last = Some(turn);
            // SAFETY: the live session and its connection.
            autoreleasepool(|_| unsafe { apply_turn(session, conn, turn) });
        }
    }
}
