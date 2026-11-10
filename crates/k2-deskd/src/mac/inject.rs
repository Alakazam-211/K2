//! CGEvent posting (D4): mouse, buttons, wheel, keys and Unicode text.
//!
//! Every event comes from one private event source whose user data is
//! [`INJECTED_TAG`], so deskd's own event tap ([`super::tap`]) tells them
//! from local input. The source's local-events suppression interval is 0,
//! so posting never blocks the person's own mouse and keyboard.
//!
//! Coordinates arrive in the chosen display's points (origin top left) and
//! are mapped to CG global points with `CGDisplayBounds`.

use std::time::{Duration, Instant};

use k2_deskd_proto::keymap::{self, Modifier};
use k2_deskd_proto::msg::{codes, Button, WheelMode};

use super::ffi::*;
use crate::inject::{Post, PostError, Poster, Pressed};

/// The user-data tag on every event deskd posts ("K2DESKD").
pub const INJECTED_TAG: i64 = 0x4B32_4445_534B_44;
/// `CGEventKeyboardSetUnicodeString` takes at most 20 UTF-16 units per
/// event (vs-live R15).
pub const UNICODE_CHUNK: usize = 20;
const DOUBLE_CLICK: Duration = Duration::from_millis(500);
const DOUBLE_CLICK_SLOP: f64 = 4.0;

/// Splits text into chunks of at most `max` UTF-16 units, never inside a
/// surrogate pair.
pub fn utf16_chunks(text: &str, max: usize) -> Vec<Vec<u16>> {
    let mut out = Vec::new();
    let mut cur: Vec<u16> = Vec::new();
    for ch in text.chars() {
        let mut buf = [0u16; 2];
        let units = ch.encode_utf16(&mut buf);
        if cur.len() + units.len() > max && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        cur.extend_from_slice(units);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

pub fn flags_for(mods: &[Modifier]) -> u64 {
    mods.iter().fold(0, |f, m| {
        f | match m {
            Modifier::Shift => kCGEventFlagMaskShift,
            Modifier::Control => kCGEventFlagMaskControl,
            Modifier::Alt => kCGEventFlagMaskAlternate,
            Modifier::Meta => kCGEventFlagMaskCommand,
        }
    })
}

/// `(down type, up type, drag type, CG button number)`.
pub fn button_types(b: Button) -> (u32, u32, u32, u32) {
    match b {
        Button::Left => (kCGEventLeftMouseDown, kCGEventLeftMouseUp, kCGEventLeftMouseDragged, kCGMouseButtonLeft),
        Button::Right => (kCGEventRightMouseDown, kCGEventRightMouseUp, kCGEventRightMouseDragged, kCGMouseButtonRight),
        Button::Middle => (kCGEventOtherMouseDown, kCGEventOtherMouseUp, kCGEventOtherMouseDragged, kCGMouseButtonCenter),
        Button::Back => (kCGEventOtherMouseDown, kCGEventOtherMouseUp, kCGEventOtherMouseDragged, 3),
        Button::Forward => (kCGEventOtherMouseDown, kCGEventOtherMouseUp, kCGEventOtherMouseDragged, 4),
    }
}

/// Accumulates fractional wheel deltas so slow trackpad scrolls still move.
#[derive(Debug, Default)]
pub struct WheelAcc {
    x: f64,
    y: f64,
}

impl WheelAcc {
    /// Browser deltas in, whole CG wheel units out. Browser `dy > 0` is
    /// wheel-towards-you (content up); CG's wheel1 > 0 is the other way.
    pub fn take(&mut self, dx: f64, dy: f64) -> (i32, i32) {
        self.x -= dx;
        self.y -= dy;
        let (ix, iy) = (self.x.trunc(), self.y.trunc());
        self.x -= ix;
        self.y -= iy;
        (ix as i32, iy as i32)
    }
}

/// Click counting for double and triple clicks.
#[derive(Debug, Default)]
pub struct Clicks {
    last: Option<(Button, Instant, CGPoint, i64)>,
}

impl Clicks {
    pub fn down(&mut self, b: Button, at: Instant, p: CGPoint) -> i64 {
        let n = match self.last {
            Some((lb, lt, lp, n)) if lb == b && at.duration_since(lt) <= DOUBLE_CLICK && (lp.x - p.x).abs() <= DOUBLE_CLICK_SLOP && (lp.y - p.y).abs() <= DOUBLE_CLICK_SLOP => n + 1,
            _ => 1,
        };
        self.last = Some((b, at, p, n));
        n
    }

    pub fn current(&self, b: Button) -> i64 {
        match self.last {
            Some((lb, _, _, n)) if lb == b => n,
            _ => 1,
        }
    }
}

/// Global CG point for display-local points.
pub fn to_global(bounds: CGRect, x: f64, y: f64) -> CGPoint {
    let cx = x.clamp(0.0, (bounds.size.width - 1.0).max(0.0));
    let cy = y.clamp(0.0, (bounds.size.height - 1.0).max(0.0));
    CGPoint { x: bounds.origin.x + cx, y: bounds.origin.y + cy }
}

pub struct MacPoster {
    source: CGEventSourceRef,
    pos: Option<CGPoint>,
    wheel_px: WheelAcc,
    wheel_line: WheelAcc,
    clicks: Clicks,
    /// `AXIsProcessTrusted()` asks tccd; read it at most once a second.
    ax: Option<(Instant, bool)>,
}

// SAFETY: the event source is only used from the thread holding the
// Injector's lock.
unsafe impl Send for MacPoster {}

impl MacPoster {
    pub fn new() -> Result<Self, PostError> {
        // SAFETY: plain CG calls; the source is released in Drop.
        let source = unsafe { CGEventSourceCreate(kCGEventSourceStatePrivate) };
        if source.is_null() {
            return Err(PostError::new(codes::POST_FAILED, "CGEventSourceCreate failed"));
        }
        unsafe {
            CGEventSourceSetLocalEventsSuppressionInterval(source, 0.0);
            CGEventSourceSetUserData(source, INJECTED_TAG);
        }
        Ok(MacPoster { source, pos: None, wheel_px: WheelAcc::default(), wheel_line: WheelAcc::default(), clicks: Clicks::default(), ax: None })
    }

    fn cursor(&mut self) -> CGPoint {
        if let Some(p) = self.pos {
            return p;
        }
        // SAFETY: an event from no source reports the cursor location.
        unsafe {
            let e = CGEventCreate(std::ptr::null_mut());
            if e.is_null() {
                return CGPoint { x: 0.0, y: 0.0 };
            }
            let p = CGEventGetLocation(e);
            CFRelease(e as CFTypeRef);
            p
        }
    }

    fn send(&self, e: CGEventRef, flags: u64) -> Result<(), PostError> {
        if e.is_null() {
            return Err(PostError::new(codes::POST_FAILED, "CoreGraphics could not create the event"));
        }
        // SAFETY: e is a live event we own.
        unsafe {
            CGEventSetFlags(e, flags);
            // The source sets it already; set again in case a type ignores it.
            CGEventSetIntegerValueField(e, kCGEventSourceUserData, INJECTED_TAG);
            CGEventPost(kCGHIDEventTap, e);
            CFRelease(e as CFTypeRef);
        }
        Ok(())
    }

    fn need_accessibility(&mut self) -> Result<(), PostError> {
        let now = Instant::now();
        let trusted = match self.ax {
            Some((at, v)) if now.duration_since(at) < Duration::from_secs(1) => v,
            _ => {
                // SAFETY: plain call.
                let v = unsafe { AXIsProcessTrusted() } != 0;
                self.ax = Some((now, v));
                v
            }
        };
        if !trusted {
            return Err(PostError::new(codes::NO_ACCESSIBILITY, "K2 Screen needs Accessibility to type and click; someone at this Mac has to allow it"));
        }
        Ok(())
    }
}

impl Drop for MacPoster {
    fn drop(&mut self) {
        // SAFETY: we own the source.
        unsafe { CFRelease(self.source as CFTypeRef) }
    }
}

pub fn display_bounds(display: u32) -> CGRect {
    // SAFETY: plain CG calls.
    unsafe {
        let id = if display == 0 { CGMainDisplayID() } else { display };
        CGDisplayBounds(id)
    }
}

impl Poster for MacPoster {
    fn post(&mut self, p: Post<'_>, pressed: Pressed<'_>) -> Result<(), PostError> {
        self.need_accessibility()?;
        let flags = flags_for(pressed.mods);
        match p {
            Post::Move { display, x, y } => {
                let bounds = display_bounds(display);
                if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
                    return Err(PostError::new(codes::NO_DISPLAY, format!("display {display} is not attached")));
                }
                let pt = to_global(bounds, x, y);
                self.pos = Some(pt);
                // While a button is held the move is a drag of the first one.
                let (kind, button) = match pressed.buttons.first() {
                    Some(b) => {
                        let (_, _, drag, n) = button_types(*b);
                        (drag, n)
                    }
                    None => (kCGEventMouseMoved, kCGMouseButtonLeft),
                };
                // SAFETY: creating an event from our source.
                let e = unsafe { CGEventCreateMouseEvent(self.source, kind, pt, button) };
                if let Some(b) = pressed.buttons.first() {
                    let n = self.clicks.current(*b);
                    unsafe {
                        if !e.is_null() {
                            CGEventSetIntegerValueField(e, kCGMouseEventClickState, n);
                        }
                    }
                }
                self.send(e, flags)
            }
            Post::Button { button, down } => {
                let pt = self.cursor();
                let (dn, up, _, n) = button_types(button);
                let count = if down { self.clicks.down(button, Instant::now(), pt) } else { self.clicks.current(button) };
                // SAFETY: creating an event from our source.
                let e = unsafe { CGEventCreateMouseEvent(self.source, if down { dn } else { up }, pt, n) };
                unsafe {
                    if !e.is_null() {
                        CGEventSetIntegerValueField(e, kCGMouseEventButtonNumber, n as i64);
                        CGEventSetIntegerValueField(e, kCGMouseEventClickState, count);
                    }
                }
                self.send(e, flags)
            }
            Post::Wheel { dx, dy, mode } => {
                let (units, (wx, wy)) = match mode {
                    WheelMode::Pixel => (kCGScrollEventUnitPixel, self.wheel_px.take(dx, dy)),
                    WheelMode::Line => (kCGScrollEventUnitLine, self.wheel_line.take(dx, dy)),
                };
                if wx == 0 && wy == 0 {
                    return Ok(());
                }
                // SAFETY: the variadic call passes exactly wheel_count wheels.
                let e = unsafe { CGEventCreateScrollWheelEvent(self.source, units, 2, wy, wx) };
                self.send(e, flags)
            }
            Post::Key { code, down } => {
                let vk = keymap::mac_vk(code).ok_or_else(|| PostError::new(codes::UNKNOWN_KEY, format!("{code} has no key on a Mac")))?;
                // SAFETY: creating an event from our source.
                let e = unsafe { CGEventCreateKeyboardEvent(self.source, vk, down) };
                self.send(e, flags)
            }
            Post::Text(text) => {
                for chunk in utf16_chunks(text, UNICODE_CHUNK) {
                    for down in [true, false] {
                        // SAFETY: the chunk outlives the call that copies it.
                        let e = unsafe { CGEventCreateKeyboardEvent(self.source, 0, down) };
                        if !e.is_null() {
                            unsafe { CGEventKeyboardSetUnicodeString(e, chunk.len(), chunk.as_ptr()) };
                        }
                        // Text ignores held modifiers (⌘ + "a" would be ⌘A).
                        self.send(e, 0)?;
                    }
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_chunks_respect_the_cap_and_surrogates() {
        let s = "a".repeat(45);
        let c = utf16_chunks(&s, 20);
        assert_eq!(c.iter().map(Vec::len).collect::<Vec<_>>(), vec![20, 20, 5]);
        // 19 units then an emoji (2 units): the emoji moves to the next chunk.
        let s = format!("{}😀b", "x".repeat(19));
        let c = utf16_chunks(&s, 20);
        assert_eq!(c[0].len(), 19);
        assert_eq!(String::from_utf16(&c[1]).unwrap(), "😀b");
        let round: String = c.iter().map(|u| String::from_utf16(u).unwrap()).collect();
        assert_eq!(round, s);
        assert!(utf16_chunks("", 20).is_empty());
    }

    #[test]
    fn modifier_flags() {
        assert_eq!(flags_for(&[]), 0);
        assert_eq!(flags_for(&[Modifier::Meta, Modifier::Shift]), kCGEventFlagMaskCommand | kCGEventFlagMaskShift);
    }

    #[test]
    fn wheel_signs_and_fractions() {
        let mut w = WheelAcc::default();
        // Browser dy +3 (content up) → CG wheel1 -3.
        assert_eq!(w.take(0.0, 3.0), (0, -3));
        assert_eq!(w.take(0.4, 0.4), (0, 0));
        assert_eq!(w.take(0.7, 0.7), (-1, -1));
    }

    #[test]
    fn double_and_triple_clicks_count_in_place() {
        let mut c = Clicks::default();
        let t = Instant::now();
        let p = CGPoint { x: 10.0, y: 10.0 };
        assert_eq!(c.down(Button::Left, t, p), 1);
        assert_eq!(c.down(Button::Left, t + Duration::from_millis(200), p), 2);
        assert_eq!(c.down(Button::Left, t + Duration::from_millis(400), CGPoint { x: 12.0, y: 9.0 }), 3);
        assert_eq!(c.down(Button::Left, t + Duration::from_millis(2000), p), 1);
        assert_eq!(c.down(Button::Right, t + Duration::from_millis(2100), p), 1);
        assert_eq!(c.down(Button::Left, t + Duration::from_millis(2200), CGPoint { x: 40.0, y: 10.0 }), 1);
    }

    #[test]
    fn display_points_map_to_global_and_clamp() {
        let b = CGRect { origin: CGPoint { x: -1920.0, y: 0.0 }, size: CGSize { width: 1920.0, height: 1080.0 } };
        assert_eq!(to_global(b, 100.0, 50.0), CGPoint { x: -1820.0, y: 50.0 });
        assert_eq!(to_global(b, 5000.0, -3.0), CGPoint { x: -1.0, y: 0.0 });
    }

    #[test]
    fn button_numbers() {
        assert_eq!(button_types(Button::Back).3, 3);
        assert_eq!(button_types(Button::Forward).3, 4);
        assert_eq!(button_types(Button::Right).0, kCGEventRightMouseDown);
    }
}
