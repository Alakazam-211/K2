//! Raw FFI to the macOS system frameworks deskd uses (no extra crates,
//! Steward R27): CoreFoundation, CoreGraphics (events, displays, screen
//! capture access), ApplicationServices (Accessibility) and Security (the
//! peer's code signature).

#![allow(non_upper_case_globals, non_camel_case_types, dead_code)]

use std::ffi::c_void;

pub type CFTypeRef = *const c_void;
pub type CFAllocatorRef = *const c_void;
pub type CFStringRef = *const c_void;
pub type CFDataRef = *const c_void;
pub type CFDictionaryRef = *const c_void;
pub type CFBooleanRef = *const c_void;
pub type CFRunLoopRef = *mut c_void;
pub type CFRunLoopSourceRef = *mut c_void;
pub type CFMachPortRef = *mut c_void;
pub type CFIndex = isize;
pub type Boolean = u8;
pub type OSStatus = i32;

pub type CGEventRef = *mut c_void;
pub type CGEventSourceRef = *mut c_void;
pub type CGEventTapProxy = *mut c_void;
pub type CGDirectDisplayID = u32;
pub type CGDisplayModeRef = *mut c_void;

pub type SecCodeRef = *mut c_void;
pub type SecRequirementRef = *mut c_void;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CGPoint {
    pub x: f64,
    pub y: f64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CGSize {
    pub width: f64,
    pub height: f64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CGRect {
    pub origin: CGPoint,
    pub size: CGSize,
}

#[repr(C)]
pub struct CFDictionaryKeyCallBacks {
    _opaque: [u8; 0],
}
#[repr(C)]
pub struct CFDictionaryValueCallBacks {
    _opaque: [u8; 0],
}

pub const kCFStringEncodingUTF8: u32 = 0x0800_0100;

// CGEventType
pub const kCGEventLeftMouseDown: u32 = 1;
pub const kCGEventLeftMouseUp: u32 = 2;
pub const kCGEventRightMouseDown: u32 = 3;
pub const kCGEventRightMouseUp: u32 = 4;
pub const kCGEventMouseMoved: u32 = 5;
pub const kCGEventLeftMouseDragged: u32 = 6;
pub const kCGEventRightMouseDragged: u32 = 7;
pub const kCGEventKeyDown: u32 = 10;
pub const kCGEventKeyUp: u32 = 11;
pub const kCGEventFlagsChanged: u32 = 12;
pub const kCGEventScrollWheel: u32 = 22;
pub const kCGEventOtherMouseDown: u32 = 25;
pub const kCGEventOtherMouseUp: u32 = 26;
pub const kCGEventOtherMouseDragged: u32 = 27;
pub const kCGEventTapDisabledByTimeout: u32 = 0xFFFF_FFFE;
pub const kCGEventTapDisabledByUserInput: u32 = 0xFFFF_FFFF;

// CGMouseButton
pub const kCGMouseButtonLeft: u32 = 0;
pub const kCGMouseButtonRight: u32 = 1;
pub const kCGMouseButtonCenter: u32 = 2;

// CGEventField
pub const kCGMouseEventClickState: u32 = 1;
pub const kCGMouseEventButtonNumber: u32 = 3;
pub const kCGEventSourceUnixProcessID: u32 = 41;
pub const kCGEventSourceUserData: u32 = 42;

// CGEventFlags
pub const kCGEventFlagMaskShift: u64 = 0x0002_0000;
pub const kCGEventFlagMaskControl: u64 = 0x0004_0000;
pub const kCGEventFlagMaskAlternate: u64 = 0x0008_0000;
pub const kCGEventFlagMaskCommand: u64 = 0x0010_0000;

// CGEventTapLocation / placement / options
pub const kCGHIDEventTap: u32 = 0;
pub const kCGSessionEventTap: u32 = 1;
pub const kCGHeadInsertEventTap: u32 = 0;
pub const kCGEventTapOptionDefault: u32 = 0;
pub const kCGEventTapOptionListenOnly: u32 = 1;

// CGEventSourceStateID
pub const kCGEventSourceStatePrivate: i32 = -1;
pub const kCGEventSourceStateCombinedSessionState: i32 = 0;
pub const kCGEventSourceStateHIDSystemState: i32 = 1;
pub const kCGAnyInputEventType: u32 = !0;

// CGScrollEventUnit
pub const kCGScrollEventUnitPixel: u32 = 0;
pub const kCGScrollEventUnitLine: u32 = 1;

pub type CGEventTapCallBack = extern "C" fn(proxy: CGEventTapProxy, kind: u32, event: CGEventRef, user_info: *mut c_void) -> CGEventRef;

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    pub static kCFAllocatorDefault: CFAllocatorRef;
    pub static kCFBooleanTrue: CFBooleanRef;
    pub static kCFTypeDictionaryKeyCallBacks: CFDictionaryKeyCallBacks;
    pub static kCFTypeDictionaryValueCallBacks: CFDictionaryValueCallBacks;
    pub static kCFRunLoopCommonModes: CFStringRef;

    pub fn CFRelease(cf: CFTypeRef);
    pub fn CFDataCreate(alloc: CFAllocatorRef, bytes: *const u8, len: CFIndex) -> CFDataRef;
    pub fn CFStringCreateWithBytes(alloc: CFAllocatorRef, bytes: *const u8, len: CFIndex, encoding: u32, external: Boolean) -> CFStringRef;
    pub fn CFDictionaryCreate(
        alloc: CFAllocatorRef,
        keys: *const *const c_void,
        values: *const *const c_void,
        count: CFIndex,
        key_cb: *const CFDictionaryKeyCallBacks,
        value_cb: *const CFDictionaryValueCallBacks,
    ) -> CFDictionaryRef;
    pub fn CFMachPortCreateRunLoopSource(alloc: CFAllocatorRef, port: CFMachPortRef, order: CFIndex) -> CFRunLoopSourceRef;
    pub fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    pub fn CFRunLoopAddSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);
    pub fn CFRunLoopRun();
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    pub fn CGPreflightScreenCaptureAccess() -> bool;
    pub fn CGRequestScreenCaptureAccess() -> bool;

    pub fn CGEventSourceCreate(state: i32) -> CGEventSourceRef;
    pub fn CGEventSourceSetLocalEventsSuppressionInterval(source: CGEventSourceRef, seconds: f64);
    pub fn CGEventSourceSetUserData(source: CGEventSourceRef, data: i64);
    pub fn CGEventSourceSecondsSinceLastEventType(state: i32, kind: u32) -> f64;

    pub fn CGEventCreate(source: CGEventSourceRef) -> CGEventRef;
    pub fn CGEventGetLocation(event: CGEventRef) -> CGPoint;
    pub fn CGEventCreateMouseEvent(source: CGEventSourceRef, kind: u32, pos: CGPoint, button: u32) -> CGEventRef;
    pub fn CGEventCreateKeyboardEvent(source: CGEventSourceRef, vk: u16, down: bool) -> CGEventRef;
    pub fn CGEventCreateScrollWheelEvent(source: CGEventSourceRef, units: u32, wheel_count: u32, wheel1: i32, ...) -> CGEventRef;
    pub fn CGEventKeyboardSetUnicodeString(event: CGEventRef, len: usize, chars: *const u16);
    pub fn CGEventSetFlags(event: CGEventRef, flags: u64);
    pub fn CGEventSetIntegerValueField(event: CGEventRef, field: u32, value: i64);
    pub fn CGEventGetIntegerValueField(event: CGEventRef, field: u32) -> i64;
    pub fn CGEventPost(tap: u32, event: CGEventRef);

    pub fn CGEventTapCreate(tap: u32, place: u32, options: u32, mask: u64, callback: CGEventTapCallBack, user_info: *mut c_void) -> CFMachPortRef;
    pub fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);

    pub fn CGMainDisplayID() -> CGDirectDisplayID;
    pub fn CGGetActiveDisplayList(max: u32, displays: *mut CGDirectDisplayID, count: *mut u32) -> i32;
    pub fn CGDisplayBounds(display: CGDirectDisplayID) -> CGRect;
    pub fn CGDisplayCopyDisplayMode(display: CGDirectDisplayID) -> CGDisplayModeRef;
    pub fn CGDisplayModeGetPixelWidth(mode: CGDisplayModeRef) -> usize;
    pub fn CGDisplayModeGetPixelHeight(mode: CGDisplayModeRef) -> usize;
    pub fn CGDisplayModeRelease(mode: CGDisplayModeRef);
    pub fn CGDisplayIsBuiltin(display: CGDirectDisplayID) -> u32;
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    pub static kAXTrustedCheckOptionPrompt: CFStringRef;
    pub fn AXIsProcessTrusted() -> Boolean;
    pub fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> Boolean;
}

#[link(name = "Security", kind = "framework")]
extern "C" {
    pub static kSecGuestAttributeAudit: CFStringRef;
    pub fn SecCodeCopyGuestWithAttributes(host: SecCodeRef, attributes: CFDictionaryRef, flags: u32, guest: *mut SecCodeRef) -> OSStatus;
    pub fn SecRequirementCreateWithString(text: CFStringRef, flags: u32, requirement: *mut SecRequirementRef) -> OSStatus;
    pub fn SecCodeCheckValidity(code: SecCodeRef, flags: u32, requirement: SecRequirementRef) -> OSStatus;
}

/// An owned CF object, released on drop.
pub struct Cf(pub CFTypeRef);

impl Drop for Cf {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: we own one retain on it.
            unsafe { CFRelease(self.0) }
        }
    }
}

pub fn cf_string(s: &str) -> Cf {
    // SAFETY: bytes and length come from a live &str.
    Cf(unsafe { CFStringCreateWithBytes(kCFAllocatorDefault, s.as_ptr(), s.len() as CFIndex, kCFStringEncodingUTF8, 0) })
}
