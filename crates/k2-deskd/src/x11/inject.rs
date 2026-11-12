//! XTEST posting (D4): pointer, buttons, wheel, keys, and Unicode text
//! through a spare keycode.
//!
//! **Text.** X has no "type this character" request. A character the keymap
//! already has is typed on its own key (Shift for its second level): a
//! client reads a key's character when it gets to the event, with the keymap
//! as it is by then, so a key that never changes is always right (L9). For
//! the rest deskd maps keycodes that have no keysyms (spares), one per
//! character, presses and releases them, and after the message (and a
//! short wait for the client to read the presses) maps them back to
//! nothing (vs-live R15: the remap changes the whole
//! session's keymap, so it never outlives one `text`, and never happens
//! during a local hold: the injector drops input first).
//!
//! Coordinates: display 0 is the root window, in pixels.

use std::sync::Arc;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as _, Keycode, Keysym, Window};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

use k2_deskd_proto::keymap::{self, Modifier};
use k2_deskd_proto::msg::{codes, Button, WheelMode};

use crate::inject::{Post, PostError, Poster, Pressed};

const KEY_PRESS: u8 = 2;
const KEY_RELEASE: u8 = 3;
const BUTTON_PRESS: u8 = 4;
const BUTTON_RELEASE: u8 = 5;
const MOTION_NOTIFY: u8 = 6;
/// Browser pixels per X wheel click.
pub const PX_PER_CLICK: f64 = 50.0;
/// Browser lines per X wheel click (X apps scroll about 3 lines a click).
pub const LINES_PER_CLICK: f64 = 3.0;

pub fn button_number(b: Button) -> u8 {
    match b {
        Button::Left => 1,
        Button::Middle => 2,
        Button::Right => 3,
        Button::Back => 8,
        Button::Forward => 9,
    }
}

/// The keysym for a character: Latin-1 keysyms equal the code point; every
/// other character is `0x0100_0000 + code point` (X11 Unicode keysyms).
pub fn keysym_for(c: char) -> Keysym {
    let cp = c as u32;
    if (0x20..=0x7e).contains(&cp) || (0xa0..=0xff).contains(&cp) {
        cp
    } else {
        0x0100_0000 + cp
    }
}

/// The keysyms of the two Shift keys.
const SHIFT_KEYSYMS: [Keysym; 2] = [0xffe1, 0xffe2];

/// How one character is typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stroke {
    /// A key the keymap already has: its first level, or its second with
    /// Shift held.
    Key { keycode: Keycode, shift: bool },
    /// A keysym the keymap lacks: typed on a spare keycode mapped to it.
    Spare(Keysym),
}

/// How to type `text` on this keymap (`keysyms`, `per` to a keycode from
/// `min`). A client turns a key press into a character when it reads the
/// event, with the keymap as it is by then; a key the keymap already has
/// never changes under it, so only what the keymap lacks goes on a spare
/// (L9: one remapped spare for every character lost most of them).
pub fn plan_text(text: &str, min: Keycode, per: u8, keysyms: &[Keysym]) -> Vec<Stroke> {
    let per = per as usize;
    let has_shift = per > 0 && keysyms.chunks(per).any(|ks| SHIFT_KEYSYMS.contains(&ks[0]));
    let find = |sym: Keysym, level: usize| {
        if per <= level {
            return None;
        }
        keysyms.chunks(per).position(|ks| ks[level] == sym).map(|i| min as usize + i).filter(|k| *k <= 255).map(|k| k as Keycode)
    };
    text.chars()
        .map(|c| {
            let sym = keysym_for(c);
            if let Some(keycode) = find(sym, 0) {
                return Stroke::Key { keycode, shift: false };
            }
            if has_shift {
                if let Some(keycode) = find(sym, 1) {
                    return Stroke::Key { keycode, shift: true };
                }
            }
            Stroke::Spare(sym)
        })
        .collect()
}

/// Every keycode whose keysyms are all NoSymbol, highest first.
pub fn find_spares(min: Keycode, per: u8, keysyms: &[Keysym]) -> Vec<Keycode> {
    if per == 0 {
        return Vec::new();
    }
    let mut v: Vec<Keycode> = keysyms
        .chunks(per as usize)
        .enumerate()
        .filter(|(_, ks)| ks.iter().all(|k| *k == 0))
        .map(|(i, _)| min as usize + i)
        .filter(|k| *k <= 255)
        .map(|k| k as Keycode)
        .collect();
    v.reverse();
    v
}

/// How long the spares stay mapped after their last press, at the least,
/// so a client that reads its events late still finds the character on
/// them.
pub const SPARE_SETTLE: std::time::Duration = std::time::Duration::from_millis(250);
/// The most a text waits before it puts the spares back.
pub const SPARE_SETTLE_MAX: std::time::Duration = std::time::Duration::from_secs(2);

/// [`SPARE_SETTLE`] plus 4 ms for each key event this text sent: a client
/// reads the events in order, so the more it has to read, the later it
/// gets to the presses on the spares.
pub fn spare_settle(events: u32) -> std::time::Duration {
    (SPARE_SETTLE + std::time::Duration::from_millis(4 * u64::from(events))).min(SPARE_SETTLE_MAX)
}

/// Whole wheel clicks out of browser deltas, keeping the remainder.
#[derive(Debug, Default)]
pub struct WheelClicks {
    x: f64,
    y: f64,
}

impl WheelClicks {
    /// `(buttons to click, count)` pairs: 4 up, 5 down, 6 left, 7 right.
    /// Browser `dy > 0` scrolls down (button 5).
    pub fn take(&mut self, dx: f64, dy: f64, mode: WheelMode) -> Vec<(u8, u32)> {
        let per = match mode {
            WheelMode::Pixel => PX_PER_CLICK,
            WheelMode::Line => LINES_PER_CLICK,
        };
        self.x += dx / per;
        self.y += dy / per;
        let (cx, cy) = (self.x.trunc(), self.y.trunc());
        self.x -= cx;
        self.y -= cy;
        let mut out = Vec::new();
        if cy != 0.0 {
            out.push((if cy > 0.0 { 5 } else { 4 }, cy.abs() as u32));
        }
        if cx != 0.0 {
            out.push((if cx > 0.0 { 7 } else { 6 }, cx.abs() as u32));
        }
        out
    }
}

pub struct X11Poster {
    conn: Arc<RustConnection>,
    root: Window,
    width: u16,
    height: u16,
    wheel: WheelClicks,
}

fn xerr(e: impl std::fmt::Display) -> PostError {
    PostError::new(codes::POST_FAILED, format!("X11: {e}"))
}

impl X11Poster {
    pub fn new(conn: Arc<RustConnection>, root: Window, width: u16, height: u16) -> Self {
        X11Poster { conn, root, width, height, wheel: WheelClicks::default() }
    }

    fn fake(&self, kind: u8, detail: u8, x: i16, y: i16) -> Result<(), PostError> {
        self.conn.xtest_fake_input(kind, detail, x11rb::CURRENT_TIME, self.root, x, y, 0).map_err(xerr)?;
        Ok(())
    }

    fn sync(&self) -> Result<(), PostError> {
        self.conn.get_input_focus().map_err(xerr)?.reply().map_err(xerr)?;
        Ok(())
    }

    fn tap(&self, keycode: Keycode) -> Result<(), PostError> {
        self.fake(KEY_PRESS, keycode, 0, 0)?;
        self.fake(KEY_RELEASE, keycode, 0, 0)
    }

    /// Put `spares` back to no keysyms, after the client had time to read
    /// the presses on them (`events`: key events sent before, [`spare_settle`]).
    fn restore_spares(&self, per: u8, spares: &[Keycode], events: u32) -> Result<(), PostError> {
        if spares.is_empty() {
            return Ok(());
        }
        self.sync()?;
        std::thread::sleep(spare_settle(events));
        let empty = vec![0; per as usize];
        let mut r = Ok(());
        for &k in spares {
            if let Err(e) = self.conn.change_keyboard_mapping(1, k, per, &empty).map_err(xerr) {
                r = Err(e);
            }
        }
        r.and_then(|_| self.sync())
    }

    fn type_text(&self, text: &str) -> Result<(), PostError> {
        let setup = self.conn.setup();
        let (min, max) = (setup.min_keycode, setup.max_keycode);
        let map = self.conn.get_keyboard_mapping(min, max - min + 1).map_err(xerr)?.reply().map_err(xerr)?;
        let per = map.keysyms_per_keycode;
        let strokes = plan_text(text, min, per, &map.keysyms);
        let free = find_spares(min, per, &map.keysyms);
        if free.is_empty() && strokes.iter().any(|s| matches!(s, Stroke::Spare(_))) {
            return Err(PostError::new(codes::UNSUPPORTED, "no free keycode to type text with"));
        }
        let shift = map.keysyms.chunks(per.max(1) as usize).position(|ks| SHIFT_KEYSYMS.contains(&ks[0])).map(|i| (min as usize + i) as Keycode);
        // Spares mapped in this text: (keysym, keycode).
        let mut mapped: Vec<(Keysym, Keycode)> = Vec::new();
        // Key events sent so far (how much the client has to read).
        let mut events: u32 = 0;
        let mut result = Ok(());
        for s in strokes {
            events = events.saturating_add(if matches!(s, Stroke::Key { shift: true, .. }) { 4 } else { 2 });
            let r = (|| match s {
                Stroke::Key { keycode, shift: false } => self.tap(keycode),
                Stroke::Key { keycode, shift: true } => {
                    let sk = shift.ok_or_else(|| PostError::new(codes::UNSUPPORTED, "no Shift key"))?;
                    self.fake(KEY_PRESS, sk, 0, 0)?;
                    let r = self.tap(keycode);
                    self.fake(KEY_RELEASE, sk, 0, 0)?;
                    r
                }
                Stroke::Spare(sym) => {
                    let kc = match mapped.iter().find(|(k, _)| *k == sym) {
                        Some((_, kc)) => *kc,
                        None => {
                            if mapped.len() == free.len() {
                                // Every spare is in use: let the client
                                // catch up, put them back, start over.
                                let used: Vec<Keycode> = mapped.iter().map(|(_, k)| *k).collect();
                                mapped.clear();
                                self.restore_spares(per, &used, events)?;
                            }
                            let kc = free[mapped.len()];
                            self.conn.change_keyboard_mapping(1, kc, per, &vec![sym; per as usize]).map_err(xerr)?;
                            self.sync()?;
                            mapped.push((sym, kc));
                            kc
                        }
                    };
                    self.tap(kc)
                }
            })();
            if r.is_err() {
                result = r;
                break;
            }
        }
        // Always put the spares back, even after a failure.
        let used: Vec<Keycode> = mapped.iter().map(|(_, k)| *k).collect();
        let restore = self.restore_spares(per, &used, events);
        result.and(restore)
    }
}

impl Poster for X11Poster {
    fn post(&mut self, p: Post<'_>, _pressed: Pressed<'_>) -> Result<(), PostError> {
        match p {
            Post::Move { display, x, y } => {
                if display != 0 {
                    return Err(PostError::new(codes::NO_DISPLAY, format!("display {display}: X11 has one display, 0 (the root window)")));
                }
                let cx = x.round().clamp(0.0, (self.width.max(1) - 1) as f64) as i16;
                let cy = y.round().clamp(0.0, (self.height.max(1) - 1) as f64) as i16;
                self.fake(MOTION_NOTIFY, 0, cx, cy)?;
            }
            Post::Button { button, down } => self.fake(if down { BUTTON_PRESS } else { BUTTON_RELEASE }, button_number(button), 0, 0)?,
            Post::Wheel { dx, dy, mode } => {
                for (b, n) in self.wheel.take(dx, dy, mode) {
                    for _ in 0..n.min(50) {
                        self.fake(BUTTON_PRESS, b, 0, 0)?;
                        self.fake(BUTTON_RELEASE, b, 0, 0)?;
                    }
                }
            }
            Post::Key { code, down } => {
                let kc = keymap::x11_keycode(code).ok_or_else(|| PostError::new(codes::UNKNOWN_KEY, format!("{code} has no X11 key")))?;
                self.fake(if down { KEY_PRESS } else { KEY_RELEASE }, kc, 0, 0)?;
            }
            Post::Text(text) => self.type_text(text)?,
        }
        self.conn.flush().map_err(xerr)?;
        Ok(())
    }

    fn refuses_chord(&self, mods: &[Modifier], code: &str) -> bool {
        keymap::x11_refused_chord(mods, code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keysyms_for_latin1_and_unicode() {
        assert_eq!(keysym_for('a'), 0x61);
        assert_eq!(keysym_for('é'), 0xe9);
        assert_eq!(keysym_for('€'), 0x0100_20ac);
        assert_eq!(keysym_for('😀'), 0x0101_f600);
    }

    #[test]
    fn spares_skip_keycodes_that_have_a_keysym() {
        // min 8, 2 per keycode: 8 full, 9 empty, 10 full, 11 empty.
        let ks = [1, 2, 0, 0, 3, 0, 0, 0];
        assert_eq!(find_spares(8, 2, &ks), vec![11, 9]);
        assert_eq!(find_spares(8, 0, &[]), Vec::<Keycode>::new());
    }

    #[test]
    fn spares_stay_mapped_longer_when_more_was_typed_before_them() {
        assert_eq!(spare_settle(0), SPARE_SETTLE);
        assert_eq!(spare_settle(100), SPARE_SETTLE + std::time::Duration::from_millis(400));
        assert_eq!(spare_settle(100_000), SPARE_SETTLE_MAX);
    }

    /// min 8, 2 per keycode: 8 a/A, 9 1/!, 10 Shift_L, 11 and 12 empty.
    const MAP: [Keysym; 10] = [0x61, 0x41, 0x31, 0x21, 0xffe1, 0, 0, 0, 0, 0];

    #[test]
    fn text_uses_the_keys_the_keymap_has_and_spares_only_for_the_rest() {
        // A client reads a key's character when it gets to the event, with
        // the keymap as it is then: a key the keymap has never changes
        // under it. Only characters it lacks go on a spare.
        assert_eq!(
            plan_text("aA1!é", 8, 2, &MAP),
            vec![
                Stroke::Key { keycode: 8, shift: false },
                Stroke::Key { keycode: 8, shift: true },
                Stroke::Key { keycode: 9, shift: false },
                Stroke::Key { keycode: 9, shift: true },
                Stroke::Spare(0xe9),
            ]
        );
    }

    #[test]
    fn with_no_shift_key_a_second_level_character_goes_on_a_spare() {
        let no_shift = [0x61, 0x41, 0, 0];
        assert_eq!(plan_text("aA", 8, 2, &no_shift), vec![Stroke::Key { keycode: 8, shift: false }, Stroke::Spare(0x41)]);
    }

    #[test]
    fn spares_are_the_highest_empty_keycodes_first() {
        assert_eq!(find_spares(8, 2, &MAP), vec![12, 11]);
        assert_eq!(find_spares(8, 2, &[1, 2, 3, 4]), Vec::<Keycode>::new());
    }

    #[test]
    fn wheel_clicks_and_directions() {
        let mut w = WheelClicks::default();
        assert_eq!(w.take(0.0, 100.0, WheelMode::Pixel), vec![(5, 2)]);
        assert_eq!(w.take(0.0, -30.0, WheelMode::Pixel), vec![]);
        assert_eq!(w.take(0.0, -30.0, WheelMode::Pixel), vec![(4, 1)]);
        assert_eq!(w.take(-6.0, 0.0, WheelMode::Line), vec![(6, 2)]);
        let mut w = WheelClicks::default();
        assert_eq!(w.take(3.0, 3.0, WheelMode::Line), vec![(5, 1), (7, 1)]);
    }

    #[test]
    fn button_numbers() {
        assert_eq!(button_number(Button::Right), 3);
        assert_eq!(button_number(Button::Back), 8);
    }
}
