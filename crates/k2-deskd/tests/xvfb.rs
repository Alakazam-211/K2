//! Xvfb integration for D4 (Linux X11). Run with a display and one test at
//! a time (they share the pointer and focus):
//!
//!   Xvfb :97 -screen 0 1920x1080x24 &
//!   DISPLAY=:97 cargo nextest run -p k2-deskd --features xvfb-tests --test xvfb -j 1
//!
//! (`scripts/test/deskd-checks.sh xvfb` does both.) A tiny x11rb window
//! covers the screen and records what reaches it.

#![cfg(all(target_os = "linux", feature = "xvfb-tests"))]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as _, CreateWindowAux, EventMask, InputFocus, WindowClass};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use k2_deskd::hold::LocalHold;
use k2_deskd::inject::{Injector, Outcome};
use k2_deskd::x11::inject::X11Poster;
use k2_deskd::x11::X11;
use k2_deskd_proto::msg::{Button, InputEvent};

/// Records key, button and motion events on a full-screen window.
struct Target {
    log: Arc<Mutex<Vec<String>>>,
}

impl Target {
    fn new() -> Target {
        let (conn, screen) = x11rb::connect(None).expect("DISPLAY must name the Xvfb server");
        let s = conn.setup().roots[screen].clone();
        let win = conn.generate_id().unwrap();
        let mask = EventMask::KEY_PRESS | EventMask::KEY_RELEASE | EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION | EventMask::STRUCTURE_NOTIFY;
        conn.create_window(s.root_depth, win, s.root, 0, 0, s.width_in_pixels, s.height_in_pixels, 0, WindowClass::INPUT_OUTPUT, 0, &CreateWindowAux::new().event_mask(mask).override_redirect(1))
            .unwrap();
        conn.map_window(win).unwrap();
        conn.flush().unwrap();
        // Wait for the map, then take the focus.
        loop {
            if let Event::MapNotify(_) = conn.wait_for_event().unwrap() {
                break;
            }
        }
        conn.set_input_focus(InputFocus::POINTER_ROOT, win, x11rb::CURRENT_TIME).unwrap();
        conn.sync().unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        let l2 = Arc::clone(&log);
        std::thread::spawn(move || {
            let conn: RustConnection = conn;
            loop {
                let line = match conn.wait_for_event() {
                    Ok(Event::KeyPress(e)) => format!("key-press {}", e.detail),
                    Ok(Event::KeyRelease(e)) => format!("key-release {}", e.detail),
                    Ok(Event::ButtonPress(e)) => format!("button-press {} at {},{}", e.detail, e.event_x, e.event_y),
                    Ok(Event::ButtonRelease(e)) => format!("button-release {}", e.detail),
                    Ok(Event::MotionNotify(e)) => format!("motion {},{}", e.event_x, e.event_y),
                    Ok(_) => continue,
                    Err(_) => return,
                };
                l2.lock().unwrap().push(line);
            }
        });
        Target { log }
    }

    fn take_after(&self, settle: Duration) -> Vec<String> {
        std::thread::sleep(settle);
        std::mem::take(&mut *self.log.lock().unwrap())
    }
}

fn injector() -> (Injector, X11) {
    let x = X11::connect().expect("X11");
    let p = X11Poster::new(Arc::clone(&x.conn), x.root, x.width, x.height);
    (Injector::new(Box::new(p)), x)
}

fn key(code: &str, down: bool) -> InputEvent {
    InputEvent::Key { code: code.into(), down }
}

const SETTLE: Duration = Duration::from_millis(300);

#[test]
fn xtest_pointer_clicks_and_keys_land_in_a_window() {
    let t = Target::new();
    let (mut inj, _x) = injector();
    let hold = LocalHold::default();
    let now = Instant::now;
    inj.inject(&InputEvent::Pointer { display: 0, x: 120.0, y: 80.0 }, &hold, now()).unwrap();
    inj.inject(&InputEvent::Button { button: Button::Left, down: true }, &hold, now()).unwrap();
    inj.inject(&InputEvent::Button { button: Button::Left, down: false }, &hold, now()).unwrap();
    inj.inject(&InputEvent::Button { button: Button::Right, down: true }, &hold, now()).unwrap();
    inj.inject(&InputEvent::Button { button: Button::Right, down: false }, &hold, now()).unwrap();
    inj.inject(&key("KeyA", true), &hold, now()).unwrap();
    inj.inject(&key("KeyA", false), &hold, now()).unwrap();
    let log = t.take_after(SETTLE);
    assert!(log.contains(&"motion 120,80".to_string()), "{log:?}");
    assert!(log.contains(&"button-press 1 at 120,80".to_string()), "{log:?}");
    assert!(log.contains(&"button-release 1".to_string()), "{log:?}");
    assert!(log.contains(&"button-press 3 at 120,80".to_string()), "{log:?}");
    // KeyA = evdev 30 = X keycode 38.
    assert!(log.contains(&"key-press 38".to_string()) && log.contains(&"key-release 38".to_string()), "{log:?}");
}

#[test]
fn xtest_wheel_clicks_buttons_four_and_five() {
    let t = Target::new();
    let (mut inj, _x) = injector();
    let hold = LocalHold::default();
    inj.inject(&InputEvent::Wheel { dx: 0.0, dy: 100.0, mode: Default::default() }, &hold, Instant::now()).unwrap();
    inj.inject(&InputEvent::Wheel { dx: 0.0, dy: -50.0, mode: Default::default() }, &hold, Instant::now()).unwrap();
    let presses: Vec<String> = t.take_after(SETTLE).into_iter().filter(|l| l.starts_with("button-press")).map(|l| l.split(" at").next().unwrap().to_string()).collect();
    assert_eq!(presses, vec!["button-press 5", "button-press 5", "button-press 4"]);
}

#[test]
fn text_types_through_a_spare_keycode_and_restores_it() {
    let t = Target::new();
    let (mut inj, x) = injector();
    let hold = LocalHold::default();
    let setup = x.conn.setup();
    let (min, max) = (setup.min_keycode, setup.max_keycode);
    let before = x.conn.get_keyboard_mapping(min, max - min + 1).unwrap().reply().unwrap();
    assert_eq!(inj.inject(&InputEvent::Text { text: "hé€😀".into() }, &hold, Instant::now()).unwrap(), Outcome::Posted);
    let log = t.take_after(SETTLE);
    let presses: Vec<&String> = log.iter().filter(|l| l.starts_with("key-press")).collect();
    assert_eq!(presses.len(), 4, "one press per character: {log:?}");
    let spare = presses[0].clone();
    assert!(presses.iter().all(|p| **p == spare), "all on the one spare keycode: {log:?}");
    let after = x.conn.get_keyboard_mapping(min, max - min + 1).unwrap().reply().unwrap();
    assert_eq!(before.keysyms, after.keysyms, "the keymap must be put back after the text");
}

#[test]
fn xtest_input_never_counts_as_local_input() {
    // vs-live R11: the controller's own XTEST input isn't "someone at the
    // screen". XScreenSaver idle would count it; XInput2 source ids don't.
    let _t = Target::new();
    let hold = Arc::new(LocalHold::default());
    k2_deskd::x11::watch::spawn(Arc::clone(&hold)).expect("XInput2 on Xvfb");
    let (mut inj, x) = injector();
    for i in 0..20 {
        inj.inject(&InputEvent::Pointer { display: 0, x: 10.0 + i as f64, y: 10.0 }, &hold, Instant::now()).unwrap();
        inj.inject(&key("KeyB", true), &hold, Instant::now()).unwrap();
        inj.inject(&key("KeyB", false), &hold, Instant::now()).unwrap();
    }
    std::thread::sleep(SETTLE);
    assert_eq!(hold.local_idle_at(Instant::now()), None, "XTEST input was taken for local input");
    // ... while XScreenSaver's idle clock was reset by it.
    let idle = x.idle_ms().expect("XScreenSaver on Xvfb");
    assert!(idle < 2_000, "XScreenSaver idle {idle} ms after XTEST input");
    let ids = k2_deskd::x11::watch::xtest_ids(&x.conn).unwrap();
    eprintln!("XTEST device ids on this server: {ids:?}");
    assert!(!ids.is_empty());
}

#[test]
fn during_a_local_hold_injected_input_never_lands_even_later() {
    let t = Target::new();
    let (mut inj, _x) = injector();
    let hold = LocalHold::default();
    hold.note_local();
    assert_eq!(inj.inject(&InputEvent::Text { text: "rm -rf".into() }, &hold, Instant::now()).unwrap(), Outcome::DroppedForHold);
    assert_eq!(inj.inject(&key("KeyC", true), &hold, Instant::now()).unwrap(), Outcome::DroppedForHold);
    assert_eq!(inj.inject(&InputEvent::Button { button: Button::Left, down: true }, &hold, Instant::now()).unwrap(), Outcome::DroppedForHold);
    let log = t.take_after(Duration::from_millis(3_300));
    assert!(log.iter().all(|l| !l.starts_with("key-") && !l.starts_with("button-")), "input landed during the hold: {log:?}");
    // After the hold, new input lands again.
    inj.inject(&key("KeyD", true), &hold, Instant::now()).unwrap();
    inj.inject(&key("KeyD", false), &hold, Instant::now()).unwrap();
    let log = t.take_after(SETTLE);
    assert!(log.contains(&"key-press 40".to_string()), "{log:?}");
}

#[test]
fn a_held_button_is_released_when_the_link_drops() {
    use k2_deskd::server::{Deskd, Host};
    use k2_deskd_proto::msg::{Display, FramesVia, Tcc};
    use k2_deskd_proto::wire;
    use std::os::unix::net::UnixStream;

    struct H;
    impl Host for H {
        fn os(&self) -> &'static str {
            "linux"
        }
        fn backend(&self) -> String {
            "x11".into()
        }
        fn frames_via(&self) -> FramesVia {
            FramesVia::Fd3
        }
        fn tcc(&self) -> Tcc {
            Tcc::NOT_REQUIRED
        }
        fn request_tcc(&self) -> Tcc {
            Tcc::NOT_REQUIRED
        }
        fn displays(&self) -> Result<Vec<Display>, String> {
            Ok(vec![])
        }
        fn hid_idle_ms(&self) -> Option<u64> {
            None
        }
        fn banner(&self, _t: Option<String>) {}
    }

    let t = Target::new();
    let (inj, _x) = injector();
    let d = Arc::new(Deskd::new(Arc::new(H), inj, Arc::new(LocalHold::default())));
    let (mut daemon, deskd_end) = UnixStream::pair().unwrap();
    let w = deskd_end.try_clone().unwrap();
    let d2 = Arc::clone(&d);
    let h = std::thread::spawn(move || d2.serve(deskd_end, Box::new(w)));
    for ev in [serde_json::json!({"t": "pointer", "x": 300, "y": 200}), serde_json::json!({"t": "button", "button": "left", "down": true})] {
        wire::write_json(&mut daemon, &serde_json::json!({"nr": true, "op": "inject", "ev": ev})).unwrap();
    }
    let log = t.take_after(SETTLE);
    assert!(log.contains(&"button-press 1 at 300,200".to_string()), "{log:?}");
    // The viewer dies mid-drag: the daemon's link closes.
    drop(daemon);
    h.join().unwrap().unwrap();
    let log = t.take_after(SETTLE);
    assert_eq!(log.iter().filter(|l| *l == "button-release 1").count(), 1, "exactly one release: {log:?}");
}
