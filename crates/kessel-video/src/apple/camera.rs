//! Cameras through AVFoundation: the list, the permission, and capture
//! sessions that deliver NV12 frames to a [`SourceShared`].

use super::ffi::{self, cf_const, cm_time, FormatDescriptionArg, SampleBufferArg};
use super::objc::{array_items, error_of, info_plist_has, nsstring, string_of, ObjSend, Queue};
use super::pixel::{cpu_frame, native_frame};
use crate::api::{Done, FrameSource, Wake};
use crate::error::VideoError;
use crate::frame::PixelFormat;
use crate::kit::{now_us, QueuedSource, SourceControl, SourceShared};
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

#[link(name = "AVFoundation", kind = "framework")]
extern "C" {}

fn media_type_video() -> Option<*const AnyObject> {
    cf_const(c"AVMediaTypeVideo").map(|p| p as *const AnyObject)
}

/// `AVAuthorizationStatus` for video, raw.
fn status_raw() -> isize {
    let Some(mt) = media_type_video() else { return 2 };
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
    let Some(mt) = media_type_video() else { return done(Permission::Denied) };
    let slot = Mutex::new(Some(done));
    let block = RcBlock::new(move |granted: Bool| {
        if let Some(done) = slot.lock().unwrap_or_else(|e| e.into_inner()).take() {
            done(if granted.as_bool() { Permission::Granted } else { Permission::Denied });
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
    let Some(mt) = media_type_video() else { return Vec::new() };
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
        let mode = CaptureMode::new(d.width as u32, d.height as u32, max_fps.round() as u32, PixelFormat::Nv12);
        match out.iter_mut().find(|(m, _)| (m.width, m.height) == (mode.width, mode.height)) {
            Some(slot) if slot.0.max_fps < mode.max_fps => *slot = (mode, f),
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
    let ty: *mut AnyObject = msg_send![device, deviceType];
    let position: isize = msg_send![device, position];
    let external = [c"AVCaptureDeviceTypeExternal", c"AVCaptureDeviceTypeExternalUnknown", c"AVCaptureDeviceTypeContinuityCamera"]
        .iter()
        .any(|n| cf_const(n).is_some_and(|t| is_equal(ty, t as *const AnyObject)));
    let built_in = cf_const(c"AVCaptureDeviceTypeBuiltInWideAngleCamera").is_some_and(|t| is_equal(ty, t as *const AnyObject));
    let facing = match position {
        1 => Facing::Back,
        2 => Facing::Front,
        _ if external => Facing::External,
        _ if built_in => Facing::Front,
        _ => Facing::Unknown,
    };
    let mut info = CameraInfo::new(id.clone(), string_of(name), facing, id == default_id);
    info.modes = modes(device).into_iter().map(|(m, _)| m).collect();
    info
}

unsafe fn default_id() -> String {
    let Some(mt) = media_type_video() else { return String::new() };
    let d: *mut AnyObject = msg_send![class!(AVCaptureDevice), defaultDeviceWithMediaType: mt];
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

unsafe extern "C-unwind" fn did_output(this: *mut AnyObject, _sel: Sel, _output: *mut AnyObject, sb: SampleBufferArg, _conn: *mut AnyObject) {
    let this: &AnyObject = &*this;
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Some(ivar) = this.class().instance_variable(IVAR) else { return };
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
        let mut b = ClassBuilder::new(c"KesselVideoCameraDelegate", class!(NSObject)).expect("the delegate class name is free");
        b.add_ivar::<*mut c_void>(IVAR);
        if let Some(p) = objc2::runtime::AnyProtocol::get(c"AVCaptureVideoDataOutputSampleBufferDelegate") {
            b.add_protocol(p);
        }
        // SAFETY: the signature AVFoundation calls:
        // captureOutput:didOutputSampleBuffer:fromConnection:.
        unsafe {
            b.add_method(
                sel!(captureOutput:didOutputSampleBuffer:fromConnection:),
                did_output as unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject, SampleBufferArg, *mut AnyObject),
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
}

// SAFETY: the objects are thread-safe AVFoundation objects; ctx is freed
// only in Drop, after the queue has drained.
unsafe impl Send for CameraControl {}

impl SourceControl for CameraControl {}

impl Drop for CameraControl {
    fn drop(&mut self) {
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

fn av_error(e: *mut AnyObject, what: &str) -> VideoError {
    // SAFETY: nil or an NSError.
    let (code, desc) = unsafe { error_of(e) };
    match code {
        // AVErrorApplicationIsNotAuthorizedToUseDevice
        -11852 => VideoError::NoPermission,
        // AVErrorDeviceInUseByAnotherApplication
        -11804 => VideoError::DeviceBusy,
        // AVErrorDeviceNotConnected
        -11814 => VideoError::DeviceLost,
        _ => VideoError::os(code, format!("{what}: {desc}")),
    }
}

pub(crate) fn open(id: Option<&str>, c: CaptureConstraints, wake: Wake) -> Result<Box<dyn FrameSource>, VideoError> {
    c.validate()?;
    if permission() != Permission::Granted {
        return Err(VideoError::NoPermission);
    }
    autoreleasepool(|_| unsafe { open_in_pool(id, c, wake) })
}

unsafe fn open_in_pool(id: Option<&str>, c: CaptureConstraints, wake: Wake) -> Result<Box<dyn FrameSource>, VideoError> {
    let mt = media_type_video().ok_or_else(|| VideoError::Unsupported("AVFoundation has no video media type".into()))?;
    let device: *mut AnyObject = match id {
        Some(id) => {
            let s = nsstring(id).ok_or(VideoError::NotFound)?;
            msg_send![class!(AVCaptureDevice), deviceWithUniqueID: &*s]
        }
        None => msg_send![class!(AVCaptureDevice), defaultDeviceWithMediaType: mt],
    };
    if device.is_null() {
        return Err(VideoError::NotFound);
    }
    let device = ObjSend(Retained::retain(device).ok_or(VideoError::NotFound)?);
    let all = modes(Retained::as_ptr(&device.0) as *mut AnyObject);
    let list: Vec<CaptureMode> = all.iter().map(|(m, _)| *m).collect();
    let chosen = nearest_mode(&list, &c);
    let format = chosen.and_then(|m| all.iter().find(|(x, _)| *x == m).map(|(_, f)| *f));
    let fps = match (chosen, c.fps) {
        (Some(m), Some(f)) => f.min(m.max_fps).max(1),
        (Some(m), None) => m.max_fps.min(30),
        (None, f) => f.unwrap_or(30),
    };

    let session: Option<Retained<AnyObject>> = msg_send![class!(AVCaptureSession), new];
    let session = ObjSend(session.ok_or_else(|| VideoError::os(0, "AVCaptureSession new"))?);
    let mut err: *mut AnyObject = std::ptr::null_mut();
    let input: *mut AnyObject = msg_send![class!(AVCaptureDeviceInput), deviceInputWithDevice: &*device, error: &mut err];
    if input.is_null() {
        return Err(av_error(err, "AVCaptureDeviceInput"));
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
    ffi::set_named(&settings, c"kCVPixelBufferPixelFormatTypeKey", ffi::num_i32(ffi::PIX_420V as i32).0);
    let _: () = msg_send![&*output, setVideoSettings: settings.0 as *const AnyObject];
    let _: () = msg_send![&*output, setAlwaysDiscardsLateVideoFrames: Bool::YES];

    let shared = SourceShared::new(c, fps, wake);
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
    };
    let ok: Bool = msg_send![&*control.session, canAddOutput: &*control.output];
    if !ok.as_bool() {
        let _: () = msg_send![&*control.session, commitConfiguration];
        return Err(VideoError::os(0, "the capture session won't take a video output"));
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
    let _: () = msg_send![&*control.session, startRunning];
    let running: Bool = msg_send![&*control.session, isRunning];
    if !running.as_bool() {
        return Err(VideoError::DeviceBusy);
    }

    // Unplugged, or the session died: end the source.
    let (stop, s, dev, sess) = (control.stop.clone(), shared.clone(), device, ObjSend(control.session.0.clone()));
    control.watch = Some(
        std::thread::Builder::new()
            .name("kessel-video-camera-watch".into())
            .spawn(move || {
                while !stop.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_millis(250));
                    let (connected, running) = autoreleasepool(|_| {
                        // SAFETY: live, thread-safe AVFoundation objects.
                        unsafe {
                            let c: Bool = msg_send![&*dev, isConnected];
                            let r: Bool = msg_send![&*sess, isRunning];
                            (c.as_bool(), r.as_bool())
                        }
                    });
                    if stop.load(Ordering::Acquire) {
                        return;
                    }
                    if !connected || !running {
                        s.event(SourceEvent::Ended(EndReason::DeviceLost));
                        return;
                    }
                }
            })
            .map_err(|e| VideoError::os(e.raw_os_error().unwrap_or(0) as i64, format!("spawn: {e}")))?,
    );
    Ok(Box::new(QueuedSource::new(shared, control)))
}
