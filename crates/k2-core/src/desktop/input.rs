//! Input rules for a desktop viewer (`prd-remote-desktop-tab-v1` §6, D1):
//! size and content checks, the per-viewer rate budget, coalescing, and
//! the held-keys set that lets every drop release what a viewer pressed
//! (vs-live R3).
//!
//! The shape is the sim's (`crate::sim::stream::check_input`,
//! `InputBudget`, `coalesce_keep`) with desktop limits. The tab's
//! `desktop-input.ts` applies the same caps before sending; the daemon
//! (D6) applies these again on every message.

use std::collections::BTreeSet;
use std::time::Instant;

use super::keymap;
use super::wire::{Button, ViewerMsg};

/// One `text` message (R12): the sim's cap.
pub const TEXT_MAX_CHARS: usize = crate::sim::stream::TEXT_MAX_CHARS;
/// One `clipboard-set` (UTF-8 bytes): the sim's paste cap.
pub const CLIPBOARD_MAX_BYTES: usize = crate::sim::stream::PASTE_MAX_BYTES;

/// Pointer moves a second.
pub const POINTER_PER_SEC: f64 = 240.0;
pub const POINTER_BURST: f64 = 240.0;
/// Keys (`key` and `button`): a burst, then this many a second.
pub const KEYS_BURST: f64 = 120.0;
pub const KEYS_PER_SEC: f64 = 60.0;
/// `text` messages a second.
pub const TEXTS_BURST: f64 = 4.0;
pub const TEXTS_PER_SEC: f64 = 4.0;
/// `clipboard-set` a second.
pub const CLIPBOARD_BURST: f64 = 1.0;
pub const CLIPBOARD_PER_SEC: f64 = 1.0;
/// Wheel messages a second (a trackpad sends ~120).
pub const WHEEL_BURST: f64 = 240.0;
pub const WHEEL_PER_SEC: f64 = 240.0;

/// Why an input message was refused (sent back as `error`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputRefusal {
    pub code: &'static str,
    pub message: String,
}

impl InputRefusal {
    fn invalid(m: impl Into<String>) -> Self {
        InputRefusal { code: "desktop_invalid", message: m.into() }
    }
}

fn finite(v: f32) -> bool {
    v.is_finite()
}

/// Size and content checks (other messages pass).
pub fn check_input(m: &ViewerMsg) -> Result<(), InputRefusal> {
    match m {
        ViewerMsg::Pointer { x, y } => {
            if !finite(*x) || !finite(*y) || *x < 0.0 || *y < 0.0 {
                return Err(InputRefusal::invalid("A pointer position must be a point on the display."));
            }
            Ok(())
        }
        ViewerMsg::Wheel { dx, dy, .. } => {
            if !finite(*dx) || !finite(*dy) {
                return Err(InputRefusal::invalid("A wheel step must be a number."));
            }
            Ok(())
        }
        ViewerMsg::Key { code, .. } => {
            if !keymap::is_known(code) {
                return Err(InputRefusal::invalid(format!("Unknown key '{code}'.")));
            }
            Ok(())
        }
        // Word for word the sim's rule (vs-live R15): Enter, Tab and
        // Backspace come as keys, so one `text` can't run a shell line.
        ViewerMsg::Text { text } => {
            let n = text.chars().count();
            if n == 0 {
                return Err(InputRefusal::invalid("Empty text."));
            }
            if n > TEXT_MAX_CHARS {
                return Err(InputRefusal::invalid(format!(
                    "Typed text is over {TEXT_MAX_CHARS} characters; send it as clipboard text."
                )));
            }
            if text.chars().any(char::is_control) {
                return Err(InputRefusal::invalid(
                    "Typed text can't hold control characters; send Enter, Tab and Backspace as keys.",
                ));
            }
            Ok(())
        }
        ViewerMsg::ClipboardSet { text } => {
            if text.is_empty() {
                return Err(InputRefusal::invalid("The clipboard has no text."));
            }
            if text.len() > CLIPBOARD_MAX_BYTES {
                return Err(InputRefusal::invalid(format!(
                    "The clipboard text is over {} KB; send less at a time.",
                    CLIPBOARD_MAX_BYTES / 1024
                )));
            }
            if text.chars().any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t')) {
                return Err(InputRefusal::invalid("The clipboard text holds control characters."));
            }
            Ok(())
        }
        ViewerMsg::Display { id } => {
            if id.trim().is_empty() || id.len() > 128 {
                return Err(InputRefusal::invalid("A display id is required."));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// A token bucket.
#[derive(Debug, Clone)]
struct Bucket {
    cap: f64,
    per_sec: f64,
    tokens: f64,
    at: Instant,
}

impl Bucket {
    fn new(cap: f64, per_sec: f64, now: Instant) -> Self {
        Bucket { cap, per_sec, tokens: cap, at: now }
    }

    fn take(&mut self, now: Instant) -> bool {
        let dt = now.saturating_duration_since(self.at).as_secs_f64();
        self.at = self.at.max(now);
        self.tokens = (self.tokens + dt * self.per_sec).min(self.cap);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// One viewer's input rate limit.
#[derive(Debug, Clone)]
pub struct InputBudget {
    pointer: Bucket,
    keys: Bucket,
    texts: Bucket,
    clipboard: Bucket,
    wheel: Bucket,
}

impl InputBudget {
    pub fn new(now: Instant) -> Self {
        InputBudget {
            pointer: Bucket::new(POINTER_BURST, POINTER_PER_SEC, now),
            keys: Bucket::new(KEYS_BURST, KEYS_PER_SEC, now),
            texts: Bucket::new(TEXTS_BURST, TEXTS_PER_SEC, now),
            clipboard: Bucket::new(CLIPBOARD_BURST, CLIPBOARD_PER_SEC, now),
            wheel: Bucket::new(WHEEL_BURST, WHEEL_PER_SEC, now),
        }
    }

    /// Spend one token; `Err` when this viewer sends too fast. A key-up
    /// and a button-up are always free: dropping a release would leave a
    /// key held on the person's real desktop.
    pub fn take(&mut self, m: &ViewerMsg, now: Instant) -> Result<(), InputRefusal> {
        let ok = match m {
            ViewerMsg::Pointer { .. } => self.pointer.take(now),
            ViewerMsg::Key { down: false, .. } | ViewerMsg::Button { down: false, .. } => true,
            ViewerMsg::Key { .. } | ViewerMsg::Button { .. } => self.keys.take(now),
            ViewerMsg::Text { .. } => self.texts.take(now),
            ViewerMsg::ClipboardSet { .. } | ViewerMsg::ClipboardGet => self.clipboard.take(now),
            ViewerMsg::Wheel { .. } => self.wheel.take(now),
            _ => true,
        };
        if ok {
            Ok(())
        } else {
            Err(InputRefusal { code: "desktop_rate_limited", message: "Sending input faster than the desktop takes it; slow down.".into() })
        }
    }
}

/// Coalesce a drained input batch: consecutive `pointer` messages collapse
/// to the last, consecutive `wheel` messages (same mode) sum. Never across
/// a `button`, `key`, `text` or anything else.
pub fn coalesce(batch: Vec<ViewerMsg>) -> Vec<ViewerMsg> {
    let mut out: Vec<ViewerMsg> = Vec::with_capacity(batch.len());
    for m in batch {
        let merged = match (out.last_mut(), &m) {
            (Some(ViewerMsg::Pointer { x, y }), ViewerMsg::Pointer { x: nx, y: ny }) => {
                *x = *nx;
                *y = *ny;
                true
            }
            (Some(ViewerMsg::Wheel { dx, dy, mode }), ViewerMsg::Wheel { dx: ndx, dy: ndy, mode: nmode }) if *mode == *nmode => {
                *dx += *ndx;
                *dy += *ndy;
                true
            }
            _ => false,
        };
        if !merged {
            out.push(m);
        }
    }
    out
}

/// One thing a viewer holds down.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Held {
    Key(String),
    Button(ButtonKey),
}

/// [`Button`] with an order (for the held set).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ButtonKey {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

impl From<Button> for ButtonKey {
    fn from(b: Button) -> Self {
        match b {
            Button::Left => ButtonKey::Left,
            Button::Right => ButtonKey::Right,
            Button::Middle => ButtonKey::Middle,
            Button::Back => ButtonKey::Back,
            Button::Forward => ButtonKey::Forward,
        }
    }
}

impl From<ButtonKey> for Button {
    fn from(b: ButtonKey) -> Self {
        match b {
            ButtonKey::Left => Button::Left,
            ButtonKey::Right => Button::Right,
            ButtonKey::Middle => Button::Middle,
            ButtonKey::Back => Button::Back,
            ButtonKey::Forward => Button::Forward,
        }
    }
}

/// The keys and buttons pressed and not yet released, in press order
/// (vs-live R3). deskd keeps one of these per controller; on hand-back,
/// a lost lease, a local hold, view-only, a close, a display switch, a
/// link drop or SIGTERM it posts [`HeldInput::release_all`], newest first.
#[derive(Debug, Clone, Default)]
pub struct HeldInput {
    order: Vec<Held>,
    set: BTreeSet<Held>,
}

impl HeldInput {
    pub fn new() -> Self {
        Self::default()
    }

    /// Note a message that went to the machine.
    pub fn note(&mut self, m: &ViewerMsg) {
        let (h, down) = match m {
            ViewerMsg::Key { code, down } => (Held::Key(code.clone()), *down),
            ViewerMsg::Button { button, down } => (Held::Button((*button).into()), *down),
            _ => return,
        };
        if down {
            if self.set.insert(h.clone()) {
                self.order.push(h);
            }
        } else if self.set.remove(&h) {
            self.order.retain(|x| x != &h);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// The releases to post, newest press first; empties the set.
    pub fn release_all(&mut self) -> Vec<ViewerMsg> {
        let out = self
            .order
            .drain(..)
            .rev()
            .map(|h| match h {
                Held::Key(code) => ViewerMsg::Key { code, down: false },
                Held::Button(b) => ViewerMsg::Button { button: b.into(), down: false },
            })
            .collect();
        self.set.clear();
        out
    }

    /// The releases deskd posts at start (a crash may have left anything
    /// down): every modifier and every button, harmless when nothing is.
    pub fn startup_releases() -> Vec<ViewerMsg> {
        keymap::MODIFIERS
            .iter()
            .map(|c| ViewerMsg::Key { code: (*c).to_string(), down: false })
            .chain(Button::ALL.iter().map(|b| ViewerMsg::Button { button: *b, down: false }))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop::wire::WheelMode;
    use std::time::Duration;

    fn key(code: &str, down: bool) -> ViewerMsg {
        ViewerMsg::Key { code: code.into(), down }
    }

    #[test]
    fn coalescing_keeps_buttons_keys_and_text() {
        let batch = vec![
            ViewerMsg::Pointer { x: 1.0, y: 1.0 },
            ViewerMsg::Pointer { x: 2.0, y: 2.0 },
            ViewerMsg::Button { button: Button::Left, down: true },
            ViewerMsg::Pointer { x: 3.0, y: 3.0 },
            ViewerMsg::Pointer { x: 4.0, y: 4.0 },
            key("KeyA", true),
            key("KeyA", true),
            ViewerMsg::Text { text: "a".into() },
            ViewerMsg::Text { text: "b".into() },
            ViewerMsg::Wheel { dx: 0.0, dy: 1.0, mode: WheelMode::Pixel },
            ViewerMsg::Wheel { dx: 1.0, dy: 2.0, mode: WheelMode::Pixel },
            ViewerMsg::Wheel { dx: 0.0, dy: 1.0, mode: WheelMode::Line },
            ViewerMsg::Button { button: Button::Left, down: false },
            ViewerMsg::Pointer { x: 5.0, y: 5.0 },
        ];
        assert_eq!(
            coalesce(batch),
            vec![
                ViewerMsg::Pointer { x: 2.0, y: 2.0 },
                ViewerMsg::Button { button: Button::Left, down: true },
                ViewerMsg::Pointer { x: 4.0, y: 4.0 },
                key("KeyA", true),
                key("KeyA", true),
                ViewerMsg::Text { text: "a".into() },
                ViewerMsg::Text { text: "b".into() },
                ViewerMsg::Wheel { dx: 1.0, dy: 3.0, mode: WheelMode::Pixel },
                ViewerMsg::Wheel { dx: 0.0, dy: 1.0, mode: WheelMode::Line },
                ViewerMsg::Button { button: Button::Left, down: false },
                ViewerMsg::Pointer { x: 5.0, y: 5.0 },
            ]
        );
    }

    #[test]
    fn text_rules_are_the_sims() {
        assert!(check_input(&ViewerMsg::Text { text: "héllo".into() }).is_ok());
        for bad in ["", "ls\n", "a\tb", "\u{1b}[A"] {
            let e = check_input(&ViewerMsg::Text { text: bad.into() }).expect_err(bad);
            assert_eq!(e.code, "desktop_invalid", "{bad:?}");
        }
        let long: String = "x".repeat(TEXT_MAX_CHARS + 1);
        assert!(check_input(&ViewerMsg::Text { text: long }).is_err());
        assert!(check_input(&ViewerMsg::Text { text: "x".repeat(TEXT_MAX_CHARS) }).is_ok());
        assert!(check_input(&ViewerMsg::ClipboardSet { text: "line\nline\t".into() }).is_ok());
        assert!(check_input(&ViewerMsg::ClipboardSet { text: "x".repeat(CLIPBOARD_MAX_BYTES + 1) }).is_err());
        assert!(check_input(&ViewerMsg::ClipboardSet { text: "\u{7}".into() }).is_err());
        assert!(check_input(&key("NotAKey", true)).is_err());
        assert!(check_input(&ViewerMsg::Pointer { x: f32::NAN, y: 1.0 }).is_err());
        assert!(check_input(&ViewerMsg::Pointer { x: -1.0, y: 1.0 }).is_err());
    }

    #[test]
    fn budget_limits_each_kind_and_never_drops_a_release() {
        let t0 = Instant::now();
        let mut b = InputBudget::new(t0);
        for i in 0..TEXTS_BURST as usize {
            assert!(b.take(&ViewerMsg::Text { text: "a".into() }, t0).is_ok(), "text {i}");
        }
        let e = b.take(&ViewerMsg::Text { text: "a".into() }, t0).expect_err("5th text in the same instant");
        assert_eq!(e.code, "desktop_rate_limited");
        assert!(b.take(&ViewerMsg::Text { text: "a".into() }, t0 + Duration::from_millis(300)).is_ok(), "refills");

        assert!(b.take(&ViewerMsg::ClipboardSet { text: "a".into() }, t0).is_ok());
        assert!(b.take(&ViewerMsg::ClipboardSet { text: "a".into() }, t0).is_err(), "one clipboard-set a second");

        for _ in 0..KEYS_BURST as usize {
            assert!(b.take(&key("KeyA", true), t0).is_ok());
        }
        assert!(b.take(&key("KeyA", true), t0).is_err(), "key burst spent");
        for _ in 0..500 {
            assert!(b.take(&key("KeyA", false), t0).is_ok(), "a release is never refused");
            assert!(b.take(&ViewerMsg::Button { button: Button::Left, down: false }, t0).is_ok());
        }

        for _ in 0..POINTER_BURST as usize {
            assert!(b.take(&ViewerMsg::Pointer { x: 1.0, y: 1.0 }, t0).is_ok());
        }
        assert!(b.take(&ViewerMsg::Pointer { x: 1.0, y: 1.0 }, t0).is_err());
        assert!(b.take(&ViewerMsg::Keyframe, t0).is_ok(), "non-input is free");
    }

    #[test]
    fn held_keys_are_released_newest_first_once() {
        let mut h = HeldInput::new();
        h.note(&key("MetaLeft", true));
        h.note(&ViewerMsg::Button { button: Button::Left, down: true });
        h.note(&key("ShiftLeft", true));
        h.note(&key("ShiftLeft", false));
        h.note(&key("KeyC", true));
        h.note(&key("KeyC", true)); // auto-repeat: still one held key
        h.note(&ViewerMsg::Pointer { x: 1.0, y: 1.0 });
        assert_eq!(
            h.release_all(),
            vec![key("KeyC", false), ViewerMsg::Button { button: Button::Left, down: false }, key("MetaLeft", false)]
        );
        assert!(h.is_empty());
        assert!(h.release_all().is_empty(), "released once");
    }

    #[test]
    fn a_link_drop_with_command_down_releases_exactly_one_command_up() {
        let mut h = HeldInput::new();
        h.note(&key("MetaLeft", true));
        let ups = h.release_all();
        assert_eq!(ups, vec![key("MetaLeft", false)]);
    }

    #[test]
    fn startup_releases_cover_every_modifier_and_button() {
        let r = HeldInput::startup_releases();
        assert_eq!(r.len(), 8 + Button::ALL.len());
        for m in keymap::MODIFIERS {
            assert!(r.contains(&key(m, false)), "{m}");
        }
        assert!(r.iter().all(|m| matches!(m, ViewerMsg::Key { down: false, .. } | ViewerMsg::Button { down: false, .. })));
    }
}
