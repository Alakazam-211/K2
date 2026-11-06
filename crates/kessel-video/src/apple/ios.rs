//! iOS only: notification observers, the app's foreground and background
//! state, and which way the phone is held (for the camera's rotation).

use super::ffi::cf_const;
use super::objc::{is_main_thread, on_main, ObjSend};
use block2::RcBlock;
use objc2::rc::{autoreleasepool, Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{class, msg_send};
use std::ffi::CStr;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

/// A block registered with the default NSNotificationCenter; removed on
/// drop.
pub(crate) struct Observer(Retained<AnyObject>);

// SAFETY: NSNotificationCenter is thread-safe, and the token is only passed
// back to it.
unsafe impl Send for Observer {}
// SAFETY: as above.
unsafe impl Sync for Observer {}

impl Observer {
    /// Call `f` with each `name` notification (from `object`, or from
    /// anyone), on the posting thread. `None` when the OS has no such
    /// notification (a framework that isn't loaded).
    ///
    /// # Safety
    /// `object` must be null or a live object. `f` gets the NSNotification.
    pub(crate) unsafe fn new(
        name: &CStr,
        object: *const AnyObject,
        f: impl Fn(*mut AnyObject) + 'static,
    ) -> Option<Observer> {
        let name = cf_const(name)? as *const AnyObject;
        let block = RcBlock::new(move |n: *mut AnyObject| {
            // A panic must not unwind into Foundation.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(n)));
        });
        autoreleasepool(|_| {
            let center: *mut AnyObject = msg_send![class!(NSNotificationCenter), defaultCenter];
            let nil: *const AnyObject = std::ptr::null();
            let token: Option<Retained<AnyObject>> = msg_send![
                center,
                addObserverForName: name,
                object: object,
                queue: nil,
                usingBlock: &*block
            ];
            token.map(Observer)
        })
    }
}

impl Drop for Observer {
    fn drop(&mut self) {
        autoreleasepool(|_| {
            // SAFETY: the token addObserverForName gave us; after this the
            // block is never called again.
            unsafe {
                let center: *mut AnyObject = msg_send![class!(NSNotificationCenter), defaultCenter];
                let _: () = msg_send![center, removeObserver: &*self.0];
            }
        });
    }
}

/// An integer from a notification's userInfo (`None` when absent).
///
/// # Safety
/// `n` must be an NSNotification.
pub(crate) unsafe fn user_info_integer(n: *mut AnyObject, key: &CStr) -> Option<isize> {
    let key = cf_const(key)? as *const AnyObject;
    let info: *mut AnyObject = msg_send![n, userInfo];
    if info.is_null() {
        return None;
    }
    let v: *mut AnyObject = msg_send![info, objectForKey: key];
    if v.is_null() {
        return None;
    }
    Some(msg_send![v, integerValue])
}

/// An object from a notification's userInfo (borrowed; may be null).
///
/// # Safety
/// `n` must be an NSNotification.
pub(crate) unsafe fn user_info_object(n: *mut AnyObject, key: &CStr) -> *mut AnyObject {
    let Some(key) = cf_const(key) else {
        return std::ptr::null_mut();
    };
    let info: *mut AnyObject = msg_send![n, userInfo];
    if info.is_null() {
        return std::ptr::null_mut();
    }
    msg_send![info, objectForKey: key as *const AnyObject]
}

// ───────────────────────────── foreground and background ─────────────────────────────

/// The app is in the background (process-wide).
static BACKGROUND: AtomicBool = AtomicBool::new(false);
/// `BACKGROUND` has been read or set by a notification.
static KNOWN: AtomicBool = AtomicBool::new(false);
/// One pair of observers for the process.
static APP_OBSERVERS: OnceLock<Vec<Observer>> = OnceLock::new();

/// Called with `true` when the app goes to the background, `false` when it
/// comes back (on the main thread).
pub(crate) type Listener = dyn Fn(bool) + Send + Sync;

fn listeners() -> &'static Mutex<Vec<Weak<Listener>>> {
    static L: OnceLock<Mutex<Vec<Weak<Listener>>>> = OnceLock::new();
    L.get_or_init(Default::default)
}

fn app_changed(background: bool) {
    BACKGROUND.store(background, Ordering::Release);
    KNOWN.store(true, Ordering::Release);
    let live: Vec<Arc<Listener>> = {
        let mut l = listeners().lock().unwrap_or_else(|e| e.into_inner());
        l.retain(|w| w.strong_count() > 0);
        l.iter().filter_map(Weak::upgrade).collect()
    };
    for f in live {
        f(background);
    }
}

/// Follow the app's state: `on_change` runs while the caller keeps it
/// alive (the list holds it weakly).
pub(crate) fn follow_app_state(on_change: &Arc<Listener>) {
    APP_OBSERVERS.get_or_init(|| {
        let mut observers = Vec::new();
        for (name, bg) in [
            (c"UIApplicationDidEnterBackgroundNotification", true),
            (c"UIApplicationWillEnterForegroundNotification", false),
        ] {
            // SAFETY: a nil object (any sender); the block reads nothing
            // from the notification.
            observers
                .extend(unsafe { Observer::new(name, std::ptr::null(), move |_| app_changed(bg)) });
        }
        observers
    });
    listeners()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(Arc::downgrade(on_change));
    if !KNOWN.load(Ordering::Acquire) {
        // The state before any notification: UIKit answers on the main
        // thread only.
        let read = || {
            if let Some(bg) = application_in_background() {
                if !KNOWN.swap(true, Ordering::AcqRel) {
                    BACKGROUND.store(bg, Ordering::Release);
                }
            }
        };
        if is_main_thread() {
            read();
        } else {
            on_main(Box::new(read));
        }
    }
}

/// `UIApplication.sharedApplication.applicationState == background`, on the
/// main thread; `None` without UIKit or an application (a test binary).
fn application_in_background() -> Option<bool> {
    let cls = AnyClass::get(c"UIApplication")?;
    autoreleasepool(|_| {
        // SAFETY: UIKit class methods with documented signatures, on the
        // main thread (callers make sure).
        unsafe {
            let app: *mut AnyObject = msg_send![cls, sharedApplication];
            if app.is_null() {
                return None;
            }
            // UIApplicationStateBackground
            let state: isize = msg_send![app, applicationState];
            Some(state == 2)
        }
    })
}

/// Is the app in the background now? False until known.
pub(crate) fn in_background() -> bool {
    BACKGROUND.load(Ordering::Acquire)
}

// ───────────────────────────── rotation ─────────────────────────────

/// A reading of which way the phone is held.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Turn {
    /// `videoRotationAngle` in degrees (iOS 17+).
    Angle(f64),
    /// `AVCaptureVideoOrientation` (older iOS).
    Orientation(isize),
}

/// Which way to turn a camera's frames so they come out upright.
pub(crate) enum Rotation {
    /// iOS 17+: `AVCaptureDeviceRotationCoordinator`'s horizon-level angle.
    Coordinator(ObjSend),
    /// Older iOS: `UIDevice.orientation`, read on the main thread.
    Device(Arc<AtomicIsize>),
}

/// `UIDevice.orientation`, on the main thread (0, unknown, without UIKit).
fn device_orientation() -> isize {
    let Some(cls) = AnyClass::get(c"UIDevice") else {
        return 0;
    };
    // SAFETY: UIDevice class and instance methods, on the main thread
    // (callers make sure).
    unsafe {
        let d: *mut AnyObject = msg_send![cls, currentDevice];
        if d.is_null() {
            return 0;
        }
        msg_send![d, orientation]
    }
}

/// Begin or end UIDevice orientation updates, on the main thread.
fn device_orientation_updates(on: bool) {
    let Some(cls) = AnyClass::get(c"UIDevice") else {
        return;
    };
    // SAFETY: as above; Rotation's life balances begin and end.
    unsafe {
        let d: *mut AnyObject = msg_send![cls, currentDevice];
        if d.is_null() {
            return;
        }
        if on {
            let _: () = msg_send![d, beginGeneratingDeviceOrientationNotifications];
        } else {
            let _: () = msg_send![d, endGeneratingDeviceOrientationNotifications];
        }
    }
}

impl Rotation {
    /// For `device` (an AVCaptureDevice).
    ///
    /// # Safety
    /// `device` must be a live AVCaptureDevice.
    pub(crate) unsafe fn new(device: &AnyObject) -> Rotation {
        if let Some(cls) = AnyClass::get(c"AVCaptureDeviceRotationCoordinator") {
            let nil: *mut AnyObject = std::ptr::null_mut();
            let alloc: Allocated<AnyObject> = msg_send![cls, alloc];
            let c: Option<Retained<AnyObject>> =
                msg_send![alloc, initWithDevice: device, previewLayer: nil];
            if let Some(c) = c {
                return Rotation::Coordinator(ObjSend(c));
            }
        }
        on_main(Box::new(|| device_orientation_updates(true)));
        Rotation::Device(Arc::new(AtomicIsize::new(0)))
    }

    /// Ask for a fresh reading (the UIDevice fallback reads on the main
    /// thread, so its answer arrives a moment later).
    pub(crate) fn refresh(&self) {
        if let Rotation::Device(o) = self {
            let o = o.clone();
            on_main(Box::new(move || {
                o.store(device_orientation(), Ordering::Release)
            }));
        }
    }

    /// The turn to set on the capture connection, or `None` while the
    /// phone lies flat or the OS doesn't say yet.
    pub(crate) fn turn(&self) -> Option<Turn> {
        match self {
            Rotation::Coordinator(c) => {
                // SAFETY: a CGFloat (f64 on 64-bit iOS) property of a live
                // coordinator.
                let a: f64 = unsafe { msg_send![&**c, videoRotationAngleForHorizonLevelCapture] };
                Some(Turn::Angle(a))
            }
            Rotation::Device(o) => super::camera::video_orientation_for(o.load(Ordering::Acquire))
                .map(Turn::Orientation),
        }
    }
}

impl Drop for Rotation {
    fn drop(&mut self) {
        if let Rotation::Device(_) = self {
            on_main(Box::new(|| device_orientation_updates(false)));
        }
    }
}
