//! Physical keys: `KeyboardEvent.code` → the host's key numbers
//! (`prd-remote-desktop-tab-v1` §6, D1).
//!
//! - macOS: the virtual key code a `CGEvent` takes (Carbon `kVK_*`).
//! - Linux: the evdev code (`linux/input-event-codes.h`), which the
//!   RemoteDesktop portal takes; X11 keycodes are evdev + 8 ([`x11_keycode`]).
//!
//! One row per code the tab may send. The tab's `desktop-keymap.ts` lists
//! the same codes (its vitest reads this file and fails when the two lists
//! differ). A code with no key on one OS is `None` there (Print Screen on a
//! Mac); deskd refuses it rather than guessing.

/// `(KeyboardEvent.code, macOS virtual key, evdev code)`.
pub const KEYS: &[(&str, Option<u16>, Option<u16>)] = &[
    ("KeyA", Some(0x00), Some(30)),
    ("KeyS", Some(0x01), Some(31)),
    ("KeyD", Some(0x02), Some(32)),
    ("KeyF", Some(0x03), Some(33)),
    ("KeyH", Some(0x04), Some(35)),
    ("KeyG", Some(0x05), Some(34)),
    ("KeyZ", Some(0x06), Some(44)),
    ("KeyX", Some(0x07), Some(45)),
    ("KeyC", Some(0x08), Some(46)),
    ("KeyV", Some(0x09), Some(47)),
    ("IntlBackslash", Some(0x0A), Some(86)),
    ("KeyB", Some(0x0B), Some(48)),
    ("KeyQ", Some(0x0C), Some(16)),
    ("KeyW", Some(0x0D), Some(17)),
    ("KeyE", Some(0x0E), Some(18)),
    ("KeyR", Some(0x0F), Some(19)),
    ("KeyY", Some(0x10), Some(21)),
    ("KeyT", Some(0x11), Some(20)),
    ("Digit1", Some(0x12), Some(2)),
    ("Digit2", Some(0x13), Some(3)),
    ("Digit3", Some(0x14), Some(4)),
    ("Digit4", Some(0x15), Some(5)),
    ("Digit6", Some(0x16), Some(7)),
    ("Digit5", Some(0x17), Some(6)),
    ("Equal", Some(0x18), Some(13)),
    ("Digit9", Some(0x19), Some(10)),
    ("Digit7", Some(0x1A), Some(8)),
    ("Minus", Some(0x1B), Some(12)),
    ("Digit8", Some(0x1C), Some(9)),
    ("Digit0", Some(0x1D), Some(11)),
    ("BracketRight", Some(0x1E), Some(27)),
    ("KeyO", Some(0x1F), Some(24)),
    ("KeyU", Some(0x20), Some(22)),
    ("BracketLeft", Some(0x21), Some(26)),
    ("KeyI", Some(0x22), Some(23)),
    ("KeyP", Some(0x23), Some(25)),
    ("Enter", Some(0x24), Some(28)),
    ("KeyL", Some(0x25), Some(38)),
    ("KeyJ", Some(0x26), Some(36)),
    ("Quote", Some(0x27), Some(40)),
    ("KeyK", Some(0x28), Some(37)),
    ("Semicolon", Some(0x29), Some(39)),
    ("Backslash", Some(0x2A), Some(43)),
    ("Comma", Some(0x2B), Some(51)),
    ("Slash", Some(0x2C), Some(53)),
    ("KeyN", Some(0x2D), Some(49)),
    ("KeyM", Some(0x2E), Some(50)),
    ("Period", Some(0x2F), Some(52)),
    ("Tab", Some(0x30), Some(15)),
    ("Space", Some(0x31), Some(57)),
    ("Backquote", Some(0x32), Some(41)),
    ("Backspace", Some(0x33), Some(14)),
    ("Escape", Some(0x35), Some(1)),
    ("MetaRight", Some(0x36), Some(126)),
    ("MetaLeft", Some(0x37), Some(125)),
    ("ShiftLeft", Some(0x38), Some(42)),
    ("CapsLock", Some(0x39), Some(58)),
    ("AltLeft", Some(0x3A), Some(56)),
    ("ControlLeft", Some(0x3B), Some(29)),
    ("ShiftRight", Some(0x3C), Some(54)),
    ("AltRight", Some(0x3D), Some(100)),
    ("ControlRight", Some(0x3E), Some(97)),
    ("F17", Some(0x40), Some(187)),
    ("NumpadDecimal", Some(0x41), Some(83)),
    ("NumpadMultiply", Some(0x43), Some(55)),
    ("NumpadAdd", Some(0x45), Some(78)),
    ("NumLock", Some(0x47), Some(69)),
    ("AudioVolumeUp", Some(0x48), Some(115)),
    ("AudioVolumeDown", Some(0x49), Some(114)),
    ("AudioVolumeMute", Some(0x4A), Some(113)),
    ("NumpadDivide", Some(0x4B), Some(98)),
    ("NumpadEnter", Some(0x4C), Some(96)),
    ("NumpadSubtract", Some(0x4E), Some(74)),
    ("F18", Some(0x4F), Some(188)),
    ("F19", Some(0x50), Some(189)),
    ("NumpadEqual", Some(0x51), Some(117)),
    ("Numpad0", Some(0x52), Some(82)),
    ("Numpad1", Some(0x53), Some(79)),
    ("Numpad2", Some(0x54), Some(80)),
    ("Numpad3", Some(0x55), Some(81)),
    ("Numpad4", Some(0x56), Some(75)),
    ("Numpad5", Some(0x57), Some(76)),
    ("Numpad6", Some(0x58), Some(77)),
    ("Numpad7", Some(0x59), Some(71)),
    ("F20", Some(0x5A), Some(190)),
    ("Numpad8", Some(0x5B), Some(72)),
    ("Numpad9", Some(0x5C), Some(73)),
    ("IntlYen", Some(0x5D), Some(124)),
    ("IntlRo", Some(0x5E), Some(89)),
    ("NumpadComma", Some(0x5F), Some(121)),
    ("F5", Some(0x60), Some(63)),
    ("F6", Some(0x61), Some(64)),
    ("F7", Some(0x62), Some(65)),
    ("F3", Some(0x63), Some(61)),
    ("F8", Some(0x64), Some(66)),
    ("F9", Some(0x65), Some(67)),
    ("Lang2", Some(0x66), Some(123)),
    ("F11", Some(0x67), Some(87)),
    ("Lang1", Some(0x68), Some(122)),
    ("F13", Some(0x69), Some(183)),
    ("F16", Some(0x6A), Some(186)),
    ("F14", Some(0x6B), Some(184)),
    ("F10", Some(0x6D), Some(68)),
    ("ContextMenu", Some(0x6E), Some(127)),
    ("F12", Some(0x6F), Some(88)),
    ("F15", Some(0x71), Some(185)),
    // A Mac has no Insert; its Help key sits there.
    ("Insert", Some(0x72), Some(110)),
    ("Home", Some(0x73), Some(102)),
    ("PageUp", Some(0x74), Some(104)),
    ("Delete", Some(0x75), Some(111)),
    ("F4", Some(0x76), Some(62)),
    ("End", Some(0x77), Some(107)),
    ("F2", Some(0x78), Some(60)),
    ("PageDown", Some(0x79), Some(109)),
    ("F1", Some(0x7A), Some(59)),
    ("ArrowLeft", Some(0x7B), Some(105)),
    ("ArrowRight", Some(0x7C), Some(106)),
    ("ArrowDown", Some(0x7D), Some(108)),
    ("ArrowUp", Some(0x7E), Some(103)),
    // PC keys a Mac doesn't have.
    ("PrintScreen", None, Some(99)),
    ("ScrollLock", None, Some(70)),
    ("Pause", None, Some(119)),
];

/// The eight modifier keys (R3: deskd releases them at start and on every
/// drop, in reverse order of pressing).
pub const MODIFIERS: [&str; 8] = [
    "ShiftLeft",
    "ShiftRight",
    "ControlLeft",
    "ControlRight",
    "AltLeft",
    "AltRight",
    "MetaLeft",
    "MetaRight",
];

fn row(code: &str) -> Option<&'static (&'static str, Option<u16>, Option<u16>)> {
    KEYS.iter().find(|(c, _, _)| *c == code)
}

/// Is `code` one the tab may send?
pub fn is_known(code: &str) -> bool {
    row(code).is_some()
}

pub fn is_modifier(code: &str) -> bool {
    MODIFIERS.contains(&code)
}

/// The macOS virtual key for `code`.
pub fn mac_keycode(code: &str) -> Option<u16> {
    row(code).and_then(|r| r.1)
}

/// The evdev code for `code`.
pub fn evdev(code: &str) -> Option<u16> {
    row(code).and_then(|r| r.2)
}

/// The X11 keycode for `code` (evdev + 8).
pub fn x11_keycode(code: &str) -> Option<u16> {
    evdev(code).map(|e| e + 8)
}

/// `code` back from a macOS virtual key.
pub fn code_for_mac(vk: u16) -> Option<&'static str> {
    KEYS.iter().find(|r| r.1 == Some(vk)).map(|r| r.0)
}

/// `code` back from an evdev code.
pub fn code_for_evdev(ev: u16) -> Option<&'static str> {
    KEYS.iter().find(|r| r.2 == Some(ev)).map(|r| r.0)
}

/// Mac ↔ PC modifiers: when the viewer's OS and the host's differ, ⌘ and
/// Ctrl trade places so ⌘C on a Mac viewer is Ctrl+C on a Linux host.
/// Every other code is unchanged.
pub fn flip_modifier(code: &str) -> &str {
    match code {
        "MetaLeft" => "ControlLeft",
        "MetaRight" => "ControlRight",
        "ControlLeft" => "MetaLeft",
        "ControlRight" => "MetaRight",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn codes_and_numbers_are_unique() {
        let mut codes = HashSet::new();
        let mut mac = HashSet::new();
        let mut ev = HashSet::new();
        for (c, m, e) in KEYS {
            assert!(codes.insert(*c), "code {c} twice");
            if let Some(m) = m {
                assert!(mac.insert(*m), "mac key {m:#x} twice ({c})");
            }
            if let Some(e) = e {
                assert!(ev.insert(*e), "evdev {e} twice ({c})");
            }
            assert!(m.is_some() || e.is_some(), "{c} maps to nothing");
        }
    }

    #[test]
    fn every_code_round_trips_on_each_os() {
        for (c, m, e) in KEYS {
            if let Some(m) = m {
                assert_eq!(mac_keycode(c), Some(*m));
                assert_eq!(code_for_mac(*m), Some(*c), "mac {m:#x}");
            }
            if let Some(e) = e {
                assert_eq!(evdev(c), Some(*e));
                assert_eq!(code_for_evdev(*e), Some(*c), "evdev {e}");
                assert_eq!(x11_keycode(c), Some(e + 8));
            }
        }
    }

    #[test]
    fn known_anchor_values() {
        // Spot checks against kVK_* and input-event-codes.h.
        assert_eq!((mac_keycode("KeyA"), evdev("KeyA")), (Some(0x00), Some(30)));
        assert_eq!((mac_keycode("Enter"), evdev("Enter")), (Some(0x24), Some(28)));
        assert_eq!((mac_keycode("Escape"), evdev("Escape")), (Some(0x35), Some(1)));
        assert_eq!((mac_keycode("MetaLeft"), evdev("MetaLeft")), (Some(0x37), Some(125)));
        assert_eq!((mac_keycode("ArrowUp"), evdev("ArrowUp")), (Some(0x7E), Some(103)));
        assert_eq!((mac_keycode("F12"), evdev("F12")), (Some(0x6F), Some(88)));
        assert_eq!(x11_keycode("KeyA"), Some(38), "X11 'a' is keycode 38");
        assert_eq!(mac_keycode("PrintScreen"), None);
        assert!(!is_known("Fn"), "Fn never reaches a webview");
    }

    #[test]
    fn modifiers_are_all_in_the_table_and_flip_in_pairs() {
        for m in MODIFIERS {
            assert!(is_known(m), "{m}");
            assert!(is_modifier(m));
            assert_eq!(flip_modifier(flip_modifier(m)), m);
        }
        assert_eq!(flip_modifier("MetaLeft"), "ControlLeft");
        assert_eq!(flip_modifier("ControlRight"), "MetaRight");
        assert_eq!(flip_modifier("AltLeft"), "AltLeft");
        assert_eq!(flip_modifier("KeyC"), "KeyC");
        assert!(!is_modifier("CapsLock"));
    }
}
