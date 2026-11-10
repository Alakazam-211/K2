//! Platform-neutral input injection: the held set, the local hold, and
//! releasing what's held (vs-live R3).
//!
//! deskd, not the daemon, keeps the keys and buttons it pressed and hasn't
//! released, and posts the ups in reverse order on hand-back, lease loss,
//! a local hold starting, view-only, the viewer closing, a display switch,
//! the daemon link closing and SIGTERM. At startup it posts key-up for the
//! eight modifiers and button-up for every button (harmless when none is
//! down; covers a deskd crash).
//!
//! A [`Poster`] is the OS end (CGEvent on macOS, XTEST on X11; a recorder
//! in tests).

use std::time::Instant;

use k2_deskd_proto::keymap::{self, Modifier};
use k2_deskd_proto::msg::{check_text, codes, Button, InputEvent, WheelMode};

use crate::hold::LocalHold;

/// One OS-level post.
#[derive(Debug, Clone, PartialEq)]
pub enum Post<'a> {
    Move { display: u32, x: f64, y: f64 },
    Button { button: Button, down: bool },
    Wheel { dx: f64, dy: f64, mode: WheelMode },
    Key { code: &'a str, down: bool },
    Text(&'a str),
}

/// What is held at the moment of a post (after a down, before... an up has
/// already been taken out), for drag events and modifier flags.
#[derive(Debug, Clone, Copy)]
pub struct Pressed<'a> {
    pub buttons: &'a [Button],
    pub mods: &'a [Modifier],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostError {
    pub code: &'static str,
    pub message: String,
}

impl PostError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        PostError { code, message: message.into() }
    }
}

pub trait Poster: Send {
    fn post(&mut self, p: Post<'_>, pressed: Pressed<'_>) -> Result<(), PostError>;
    /// X11 refuses the chords that kill the server or switch to a console.
    fn refuses_chord(&self, _mods: &[Modifier], _code: &str) -> bool {
        false
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Held {
    Key(String),
    Button(Button),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Posted,
    /// A local hold is on: dropped, never queued.
    DroppedForHold,
    /// An up for something deskd isn't holding (already released): no post.
    Ignored,
}

pub struct Injector {
    poster: Box<dyn Poster>,
    /// In press order; released in reverse.
    held: Vec<Held>,
}

impl Injector {
    pub fn new(poster: Box<dyn Poster>) -> Self {
        Injector { poster, held: Vec::new() }
    }

    fn buttons(&self) -> Vec<Button> {
        self.held.iter().filter_map(|h| if let Held::Button(b) = h { Some(*b) } else { None }).collect()
    }

    fn mods(&self) -> Vec<Modifier> {
        let mut m: Vec<Modifier> = Vec::new();
        for h in &self.held {
            if let Held::Key(c) = h {
                if let Some(x) = keymap::modifier(c) {
                    if !m.contains(&x) {
                        m.push(x);
                    }
                }
            }
        }
        m
    }

    fn post(&mut self, p: Post<'_>) -> Result<(), PostError> {
        let (b, m) = (self.buttons(), self.mods());
        self.poster.post(p, Pressed { buttons: &b, mods: &m })
    }

    pub fn held_count(&self) -> usize {
        self.held.len()
    }

    /// One remote event. `hold` is checked first, so nothing (not even a
    /// key-up) is posted while the person at the machine is using it; the
    /// hold's start already released everything.
    pub fn inject(&mut self, ev: &InputEvent, hold: &LocalHold, now: Instant) -> Result<Outcome, PostError> {
        if hold.held_at(now) {
            return Ok(Outcome::DroppedForHold);
        }
        match ev {
            InputEvent::Pointer { display, x, y } => {
                if !x.is_finite() || !y.is_finite() {
                    return Err(PostError::new(codes::BAD_REQUEST, "pointer coordinates must be finite"));
                }
                self.post(Post::Move { display: *display, x: *x, y: *y })?;
                Ok(Outcome::Posted)
            }
            InputEvent::Wheel { dx, dy, mode } => {
                if !dx.is_finite() || !dy.is_finite() {
                    return Err(PostError::new(codes::BAD_REQUEST, "wheel deltas must be finite"));
                }
                self.post(Post::Wheel { dx: *dx, dy: *dy, mode: *mode })?;
                Ok(Outcome::Posted)
            }
            InputEvent::Button { button, down: true } => {
                let fresh = !self.held.contains(&Held::Button(*button));
                if fresh {
                    self.held.push(Held::Button(*button));
                }
                if let Err(e) = self.post(Post::Button { button: *button, down: true }) {
                    if fresh {
                        self.held.retain(|h| h != &Held::Button(*button));
                    }
                    return Err(e);
                }
                Ok(Outcome::Posted)
            }
            InputEvent::Button { button, down: false } => {
                let Some(i) = self.held.iter().position(|h| h == &Held::Button(*button)) else {
                    return Ok(Outcome::Ignored);
                };
                self.held.remove(i);
                self.post(Post::Button { button: *button, down: false })?;
                Ok(Outcome::Posted)
            }
            InputEvent::Key { code, down: true } => {
                if keymap::mac_vk(code).is_none() && keymap::evdev(code).is_none() {
                    return Err(PostError::new(codes::UNKNOWN_KEY, format!("unknown key code {code:?}")));
                }
                if self.poster.refuses_chord(&self.mods(), code) {
                    return Err(PostError::new(codes::UNSUPPORTED, format!("{code} with Ctrl+Alt is refused on X11 (it would kill the X server or switch to a console)")));
                }
                let key = Held::Key(code.clone());
                let fresh = !self.held.contains(&key);
                if fresh {
                    self.held.push(key.clone());
                }
                if let Err(e) = self.post(Post::Key { code, down: true }) {
                    if fresh {
                        self.held.retain(|h| h != &key);
                    }
                    return Err(e);
                }
                Ok(Outcome::Posted)
            }
            InputEvent::Key { code, down: false } => {
                let Some(i) = self.held.iter().position(|h| matches!(h, Held::Key(c) if c == code)) else {
                    return Ok(Outcome::Ignored);
                };
                self.held.remove(i);
                self.post(Post::Key { code, down: false })?;
                Ok(Outcome::Posted)
            }
            InputEvent::Text { text } => {
                check_text(text).map_err(|m| PostError::new(codes::BAD_TEXT, m))?;
                self.post(Post::Text(text))?;
                Ok(Outcome::Posted)
            }
        }
    }

    /// Posts the up for everything held, newest first. Returns how many.
    /// A failed post still forgets the item (it can't be retried usefully).
    pub fn release_all(&mut self) -> usize {
        let mut n = 0;
        while let Some(h) = self.held.pop() {
            let r = match &h {
                Held::Key(c) => {
                    let c = c.clone();
                    self.post(Post::Key { code: &c, down: false })
                }
                Held::Button(b) => self.post(Post::Button { button: *b, down: false }),
            };
            if r.is_ok() {
                n += 1;
            }
        }
        n
    }

    /// At startup: key-up for the eight modifiers and button-up for every
    /// button, whatever deskd thinks is held (it thinks nothing is).
    pub fn release_at_startup(&mut self) {
        self.held.clear();
        for c in keymap::MODIFIER_CODES {
            let _ = self.post(Post::Key { code: c, down: false });
        }
        for b in Button::ALL {
            let _ = self.post(Post::Button { button: b, down: false });
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    /// Records every post as text: `key MetaLeft down mods=[Meta]`.
    #[derive(Clone, Default)]
    pub struct Recorder {
        pub log: Arc<Mutex<Vec<String>>>,
        pub fail_next: Arc<Mutex<Option<&'static str>>>,
    }

    impl Recorder {
        pub fn take(&self) -> Vec<String> {
            std::mem::take(&mut *self.log.lock().unwrap())
        }
    }

    impl Poster for Recorder {
        fn post(&mut self, p: Post<'_>, pressed: Pressed<'_>) -> Result<(), PostError> {
            if let Some(code) = self.fail_next.lock().unwrap().take() {
                return Err(PostError::new(code, "told to fail"));
            }
            let line = match p {
                Post::Move { display, x, y } => format!("move {display} {x} {y} buttons={:?}", pressed.buttons),
                Post::Button { button, down } => format!("button {button:?} {}", if down { "down" } else { "up" }),
                Post::Wheel { dx, dy, mode } => format!("wheel {dx} {dy} {mode:?}"),
                Post::Key { code, down } => format!("key {code} {} mods={:?}", if down { "down" } else { "up" }, pressed.mods),
                Post::Text(t) => format!("text {t}"),
            };
            self.log.lock().unwrap().push(line);
            Ok(())
        }
    }

    fn key(code: &str, down: bool) -> InputEvent {
        InputEvent::Key { code: code.into(), down }
    }

    fn setup() -> (Injector, Recorder, LocalHold, Instant) {
        let rec = Recorder::default();
        let t0 = Instant::now();
        (Injector::new(Box::new(rec.clone())), rec, LocalHold::new(t0), t0)
    }

    #[test]
    fn a_held_command_key_is_released_exactly_once_on_link_drop() {
        let (mut inj, rec, hold, t0) = setup();
        inj.inject(&key("MetaLeft", true), &hold, t0).unwrap();
        rec.take();
        assert_eq!(inj.release_all(), 1);
        assert_eq!(rec.take(), vec!["key MetaLeft up mods=[]"]);
        // Released already: a second release and a late up post nothing.
        assert_eq!(inj.release_all(), 0);
        assert_eq!(inj.inject(&key("MetaLeft", false), &hold, t0).unwrap(), Outcome::Ignored);
        assert!(rec.take().is_empty());
    }

    #[test]
    fn release_is_in_reverse_press_order_keys_and_buttons_mixed() {
        let (mut inj, rec, hold, t0) = setup();
        inj.inject(&key("ShiftLeft", true), &hold, t0).unwrap();
        inj.inject(&InputEvent::Button { button: Button::Left, down: true }, &hold, t0).unwrap();
        inj.inject(&key("KeyA", true), &hold, t0).unwrap();
        rec.take();
        assert_eq!(inj.release_all(), 3);
        assert_eq!(rec.take(), vec!["key KeyA up mods=[Shift]", "button Left up", "key ShiftLeft up mods=[]"]);
    }

    #[test]
    fn modifier_flags_and_drag_state_reach_the_poster() {
        let (mut inj, rec, hold, t0) = setup();
        inj.inject(&key("MetaLeft", true), &hold, t0).unwrap();
        inj.inject(&key("KeyW", true), &hold, t0).unwrap();
        inj.inject(&InputEvent::Button { button: Button::Left, down: true }, &hold, t0).unwrap();
        inj.inject(&InputEvent::Pointer { display: 1, x: 5.0, y: 6.0 }, &hold, t0).unwrap();
        assert_eq!(
            rec.take(),
            vec!["key MetaLeft down mods=[Meta]", "key KeyW down mods=[Meta]", "button Left down", "move 1 5 6 buttons=[Left]"]
        );
    }

    #[test]
    fn autorepeat_posts_again_but_holds_once() {
        let (mut inj, rec, hold, t0) = setup();
        inj.inject(&key("KeyJ", true), &hold, t0).unwrap();
        inj.inject(&key("KeyJ", true), &hold, t0).unwrap();
        assert_eq!(inj.held_count(), 1);
        assert_eq!(rec.take().len(), 2);
    }

    #[test]
    fn during_a_local_hold_everything_is_dropped_even_text_and_never_typed_later() {
        let (mut inj, rec, hold, t0) = setup();
        hold.note_local_at(t0);
        let t = t0 + Duration::from_millis(500);
        assert_eq!(inj.inject(&InputEvent::Text { text: "rm -rf".into() }, &hold, t).unwrap(), Outcome::DroppedForHold);
        assert_eq!(inj.inject(&key("KeyA", true), &hold, t).unwrap(), Outcome::DroppedForHold);
        assert_eq!(inj.inject(&InputEvent::Pointer { display: 0, x: 1.0, y: 1.0 }, &hold, t).unwrap(), Outcome::DroppedForHold);
        assert_eq!(inj.held_count(), 0);
        // After the hold, nothing queued comes out.
        let after = t0 + Duration::from_secs(4);
        inj.inject(&InputEvent::Pointer { display: 0, x: 2.0, y: 2.0 }, &hold, after).unwrap();
        assert_eq!(rec.take(), vec!["move 0 2 2 buttons=[]"]);
    }

    #[test]
    fn bad_input_is_refused_with_a_code() {
        let (mut inj, _rec, hold, t0) = setup();
        assert_eq!(inj.inject(&key("NoSuchKey", true), &hold, t0).unwrap_err().code, codes::UNKNOWN_KEY);
        assert_eq!(inj.inject(&InputEvent::Text { text: "a\nb".into() }, &hold, t0).unwrap_err().code, codes::BAD_TEXT);
        assert_eq!(inj.inject(&InputEvent::Pointer { display: 0, x: f64::NAN, y: 0.0 }, &hold, t0).unwrap_err().code, codes::BAD_REQUEST);
        assert_eq!(inj.held_count(), 0);
    }

    #[test]
    fn a_failed_down_is_not_held() {
        let (mut inj, rec, hold, t0) = setup();
        *rec.fail_next.lock().unwrap() = Some(codes::NO_ACCESSIBILITY);
        assert_eq!(inj.inject(&key("KeyA", true), &hold, t0).unwrap_err().code, codes::NO_ACCESSIBILITY);
        assert_eq!(inj.held_count(), 0);
        assert_eq!(inj.release_all(), 0);
    }

    #[test]
    fn startup_releases_eight_modifiers_and_five_buttons() {
        let (mut inj, rec, _hold, _t0) = setup();
        inj.release_at_startup();
        let log = rec.take();
        assert_eq!(log.len(), 13);
        assert!(log[..8].iter().all(|l| l.starts_with("key ") && l.contains(" up ")));
        assert!(log[8..].iter().all(|l| l.starts_with("button ") && l.ends_with(" up")));
    }

    struct X11Like(Recorder);
    impl Poster for X11Like {
        fn post(&mut self, p: Post<'_>, pressed: Pressed<'_>) -> Result<(), PostError> {
            self.0.post(p, pressed)
        }
        fn refuses_chord(&self, mods: &[Modifier], code: &str) -> bool {
            keymap::x11_refused_chord(mods, code)
        }
    }

    #[test]
    fn x11_poster_refuses_ctrl_alt_backspace() {
        let rec = Recorder::default();
        let t0 = Instant::now();
        let hold = LocalHold::new(t0);
        let mut inj = Injector::new(Box::new(X11Like(rec.clone())));
        inj.inject(&key("ControlLeft", true), &hold, t0).unwrap();
        inj.inject(&key("AltLeft", true), &hold, t0).unwrap();
        assert_eq!(inj.inject(&key("Backspace", true), &hold, t0).unwrap_err().code, codes::UNSUPPORTED);
        assert_eq!(inj.held_count(), 2);
    }
}
