//! The macOS clipboard: the general NSPasteboard, plain text only
//! (`public.utf8-plain-text`). Policy (the cap, the reasons, never
//! logging the text) is `crate::clipboard`'s; this is only the OS end.
//!
//! - A password manager's copy (marked `org.nspasteboard.ConcealedType`)
//!   reads as `concealed`, never as its text.
//! - The size is asked before the text is copied out, so a huge clipboard
//!   costs nothing.
//! - macOS 26 has a per-app paste setting: on Ask, the first read may put
//!   a prompt in front of the person at the Mac (the daemon's call then
//!   times out until it's answered); on Deny it reads as `denied`.

use objc2::rc::{autoreleasepool, Retained};
use objc2::runtime::{AnyObject, Bool};
use objc2::{class, msg_send, sel};
use objc2_foundation::NSString;

use k2_deskd_proto::msg::{codes, CLIPBOARD_MAX_BYTES};

use crate::clipboard::ClipRead;

/// `NSPasteboardTypeString`.
const TEXT_TYPE: &str = "public.utf8-plain-text";
/// The nspasteboard.org marker password managers put on secrets.
const CONCEALED_TYPE: &str = "org.nspasteboard.ConcealedType";
/// `NSUTF8StringEncoding`.
const UTF8: usize = 4;
/// `NSPasteboardAccessBehaviorAlwaysDeny` (macOS 26).
const ACCESS_ALWAYS_DENY: isize = 3;

/// The general pasteboard (autoreleased; use inside a pool).
unsafe fn general() -> *mut AnyObject {
    msg_send![class!(NSPasteboard), generalPasteboard]
}

pub fn read() -> ClipRead {
    autoreleasepool(|_| {
        // SAFETY: NSPasteboard's documented class and instance methods, on
        // objects it returns (nil checked); everything stays in this pool.
        unsafe {
            let pb = general();
            if pb.is_null() {
                return ClipRead::Unsupported;
            }
            let responds: Bool = msg_send![pb, respondsToSelector: sel!(accessBehavior)];
            if responds.as_bool() {
                let behavior: isize = msg_send![pb, accessBehavior];
                if behavior == ACCESS_ALWAYS_DENY {
                    return ClipRead::Denied;
                }
            }
            let types: *mut AnyObject = msg_send![pb, types];
            let n: usize = if types.is_null() { 0 } else { msg_send![types, count] };
            if n == 0 {
                return ClipRead::Empty;
            }
            let concealed = NSString::from_str(CONCEALED_TYPE);
            let has: Bool = msg_send![types, containsObject: &*concealed];
            if has.as_bool() {
                return ClipRead::Concealed;
            }
            let ty = NSString::from_str(TEXT_TYPE);
            let s: *mut NSString = msg_send![pb, stringForType: &*ty];
            let Some(s) = Retained::retain(s) else { return ClipRead::NotText };
            let bytes: usize = msg_send![&*s, lengthOfBytesUsingEncoding: UTF8];
            if bytes > CLIPBOARD_MAX_BYTES {
                return ClipRead::TooLarge { bytes };
            }
            ClipRead::Text(s.to_string())
        }
    })
}

pub fn write(text: &str) -> Result<(), (&'static str, String)> {
    autoreleasepool(|_| {
        // SAFETY: as in `read`; the strings outlive the calls.
        unsafe {
            let pb = general();
            if pb.is_null() {
                return Err((codes::UNSUPPORTED, "no pasteboard in this session".into()));
            }
            let _: isize = msg_send![pb, clearContents];
            let s = NSString::from_str(text);
            let ty = NSString::from_str(TEXT_TYPE);
            let ok: Bool = msg_send![pb, setString: &*s, forType: &*ty];
            if ok.as_bool() {
                Ok(())
            } else {
                Err((codes::CLIPBOARD_FAILED, "the pasteboard refused the text".into()))
            }
        }
    })
}
