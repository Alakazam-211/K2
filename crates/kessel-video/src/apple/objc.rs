//! Small Objective-C helpers: strings, arrays, dispatch queues, NSError, and
//! a Send wrapper for objects that are safe to hold across threads.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool};
use objc2::{class, msg_send};
use std::ffi::{c_void, CStr, CString};
use std::os::raw::c_char;

/// An object we hold on another thread. Only used for AVFoundation and
/// ScreenCaptureKit objects whose methods we call are thread-safe
/// (sessions, streams, devices, filters).
pub struct ObjSend(pub Retained<AnyObject>);
// SAFETY: see the type's doc: only thread-safe framework objects.
unsafe impl Send for ObjSend {}
// SAFETY: as above.
unsafe impl Sync for ObjSend {}

impl std::ops::Deref for ObjSend {
    type Target = AnyObject;
    fn deref(&self) -> &AnyObject {
        &self.0
    }
}

/// An autoreleased NSString from Rust text.
pub fn nsstring(s: &str) -> Option<Retained<AnyObject>> {
    let c = CString::new(s).ok()?;
    // SAFETY: a NUL-terminated string that outlives the call.
    unsafe { msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()] }
}

/// Rust text from an NSString (empty for nil).
///
/// # Safety
/// `s` must be nil or an NSString.
pub unsafe fn string_of(s: *mut AnyObject) -> String {
    if s.is_null() {
        return String::new();
    }
    let p: *const c_char = msg_send![s, UTF8String];
    if p.is_null() {
        return String::new();
    }
    CStr::from_ptr(p).to_string_lossy().into_owned()
}

/// The objects of an NSArray (borrowed; valid while the array lives).
///
/// # Safety
/// `a` must be nil or an NSArray.
pub unsafe fn array_items(a: *mut AnyObject) -> Vec<*mut AnyObject> {
    if a.is_null() {
        return Vec::new();
    }
    let n: usize = msg_send![a, count];
    (0..n).map(|i| msg_send![a, objectAtIndex: i]).collect()
}

/// `-[NSError code]` and its description (for nil: 0, "unknown").
///
/// # Safety
/// `e` must be nil or an NSError.
pub unsafe fn error_of(e: *mut AnyObject) -> (i64, String) {
    if e.is_null() {
        return (0, "unknown".into());
    }
    let code: isize = msg_send![e, code];
    let desc: *mut AnyObject = msg_send![e, localizedDescription];
    (code as i64, string_of(desc))
}

/// Does the app's Info.plist have this key? (A test binary has none.)
pub fn info_plist_has(key: &str) -> bool {
    objc2::rc::autoreleasepool(|_| {
        let Some(k) = nsstring(key) else { return false };
        // SAFETY: Foundation methods with documented signatures; each
        // object is checked for nil.
        unsafe {
            let bundle: *mut AnyObject = msg_send![class!(NSBundle), mainBundle];
            if bundle.is_null() {
                return false;
            }
            let v: *mut AnyObject = msg_send![bundle, objectForInfoDictionaryKey: &*k];
            !v.is_null()
        }
    })
}

/// Does `obj` answer `sel`?
///
/// # Safety
/// `obj` must be a live object.
pub unsafe fn responds(obj: *const AnyObject, sel: objc2::runtime::Sel) -> bool {
    let r: Bool = msg_send![obj, respondsToSelector: sel];
    r.as_bool()
}

// libdispatch (libSystem).
extern "C" {
    pub fn dispatch_queue_create(label: *const c_char, attr: *mut c_void) -> *mut AnyObject;
    pub fn dispatch_sync_f(queue: *mut AnyObject, context: *mut c_void, work: extern "C" fn(*mut c_void));
    pub fn dispatch_async_f(queue: *mut AnyObject, context: *mut c_void, work: extern "C" fn(*mut c_void));
    pub fn dispatch_release(object: *mut AnyObject);
    static _dispatch_main_q: u8;
}

/// The main queue (`dispatch_get_main_queue()`).
pub fn main_queue() -> *mut AnyObject {
    // SAFETY: the address of libdispatch's main-queue object.
    unsafe { &_dispatch_main_q as *const u8 as *mut AnyObject }
}

extern "C" fn nothing(_: *mut c_void) {}

/// A serial queue we own.
pub struct Queue(pub *mut AnyObject);
// SAFETY: dispatch queues are thread-safe.
unsafe impl Send for Queue {}
// SAFETY: as above.
unsafe impl Sync for Queue {}

impl Queue {
    pub fn new(label: &CStr) -> Queue {
        // SAFETY: a NUL-terminated label; a null attribute is a serial queue.
        Queue(unsafe { dispatch_queue_create(label.as_ptr(), std::ptr::null_mut()) })
    }

    /// Wait until everything queued so far has run.
    pub fn drain(&self) {
        // SAFETY: a live queue; the work function ignores its context.
        unsafe { dispatch_sync_f(self.0, std::ptr::null_mut(), nothing) }
    }
}

impl Drop for Queue {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: we created it and hold its one reference.
            unsafe { dispatch_release(self.0) }
        }
    }
}

/// Run `f` on the main thread, asynchronously (it needs the app's main run
/// loop, which Tauri and AppKit apps have).
pub fn on_main(f: Box<dyn FnOnce() + Send>) {
    extern "C" fn run(ctx: *mut c_void) {
        // SAFETY: ctx is the Box we leaked below, taken back exactly once.
        let f = unsafe { Box::from_raw(ctx as *mut Box<dyn FnOnce() + Send>) };
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).is_err() {
            // A panic must not unwind into libdispatch.
        }
    }
    let ctx = Box::into_raw(Box::new(f)) as *mut c_void;
    // SAFETY: the main queue; `run` takes ownership of ctx.
    unsafe { dispatch_async_f(main_queue(), ctx, run) }
}

/// Are we on the main thread?
pub fn is_main_thread() -> bool {
    // SAFETY: a class method returning BOOL.
    let b: Bool = unsafe { msg_send![class!(NSThread), isMainThread] };
    b.as_bool()
}

/// The OS version string, for logs ("macOS 15.1").
pub fn os_name() -> String {
    objc2::rc::autoreleasepool(|_| {
        // SAFETY: Foundation methods with documented signatures.
        unsafe {
            let info: *mut AnyObject = msg_send![class!(NSProcessInfo), processInfo];
            let v: *mut AnyObject = msg_send![info, operatingSystemVersionString];
            format!("macOS {}", string_of(v))
        }
    })
}
