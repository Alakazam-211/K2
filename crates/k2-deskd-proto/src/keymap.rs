//! `KeyboardEvent.code` → the host's key: a macOS virtual key code
//! (Carbon `kVK_*`) and a Linux evdev code (`KEY_*`). X11 keycodes are
//! evdev + 8 (the xkb evdev rules every X server uses).
//!
//! Physical keys only: a code names a position, not a character, so a
//! French viewer pressing `KeyQ` types whatever the host's layout puts
//! there. Characters go as `text`.
//!
//! The PRD puts a keymap in `k2-core::desktop` (D1) too. This table is the
//! one deskd posts from; D1's can re-export it, so there is one table.

/// `(code, macOS kVK, evdev)`; `None` = the host has no such key.
const TABLE: &[(&str, Option<u16>, Option<u16>)] = &[
    ("KeyA", Some(0x00), Some(30)),
    ("KeyB", Some(0x0B), Some(48)),
    ("KeyC", Some(0x08), Some(46)),
    ("KeyD", Some(0x02), Some(32)),
    ("KeyE", Some(0x0E), Some(18)),
    ("KeyF", Some(0x03), Some(33)),
    ("KeyG", Some(0x05), Some(34)),
    ("KeyH", Some(0x04), Some(35)),
    ("KeyI", Some(0x22), Some(23)),
    ("KeyJ", Some(0x26), Some(36)),
    ("KeyK", Some(0x28), Some(37)),
    ("KeyL", Some(0x25), Some(38)),
    ("KeyM", Some(0x2E), Some(50)),
    ("KeyN", Some(0x2D), Some(49)),
    ("KeyO", Some(0x1F), Some(24)),
    ("KeyP", Some(0x23), Some(25)),
    ("KeyQ", Some(0x0C), Some(16)),
    ("KeyR", Some(0x0F), Some(19)),
    ("KeyS", Some(0x01), Some(31)),
    ("KeyT", Some(0x11), Some(20)),
    ("KeyU", Some(0x20), Some(22)),
    ("KeyV", Some(0x09), Some(47)),
    ("KeyW", Some(0x0D), Some(17)),
    ("KeyX", Some(0x07), Some(45)),
    ("KeyY", Some(0x10), Some(21)),
    ("KeyZ", Some(0x06), Some(44)),
    ("Digit1", Some(0x12), Some(2)),
    ("Digit2", Some(0x13), Some(3)),
    ("Digit3", Some(0x14), Some(4)),
    ("Digit4", Some(0x15), Some(5)),
    ("Digit5", Some(0x17), Some(6)),
    ("Digit6", Some(0x16), Some(7)),
    ("Digit7", Some(0x1A), Some(8)),
    ("Digit8", Some(0x1C), Some(9)),
    ("Digit9", Some(0x19), Some(10)),
    ("Digit0", Some(0x1D), Some(11)),
    ("Minus", Some(0x1B), Some(12)),
    ("Equal", Some(0x18), Some(13)),
    ("BracketLeft", Some(0x21), Some(26)),
    ("BracketRight", Some(0x1E), Some(27)),
    ("Backslash", Some(0x2A), Some(43)),
    ("Semicolon", Some(0x29), Some(39)),
    ("Quote", Some(0x27), Some(40)),
    ("Backquote", Some(0x32), Some(41)),
    ("Comma", Some(0x2B), Some(51)),
    ("Period", Some(0x2F), Some(52)),
    ("Slash", Some(0x2C), Some(53)),
    ("IntlBackslash", Some(0x0A), Some(86)),
    ("IntlRo", Some(0x5E), Some(89)),
    ("IntlYen", Some(0x5D), Some(124)),
    ("Enter", Some(0x24), Some(28)),
    ("Tab", Some(0x30), Some(15)),
    ("Space", Some(0x31), Some(57)),
    ("Backspace", Some(0x33), Some(14)),
    ("Escape", Some(0x35), Some(1)),
    ("Delete", Some(0x75), Some(111)),
    ("Insert", Some(0x72), Some(110)),
    ("Help", None, Some(138)),
    ("Home", Some(0x73), Some(102)),
    ("End", Some(0x77), Some(107)),
    ("PageUp", Some(0x74), Some(104)),
    ("PageDown", Some(0x79), Some(109)),
    ("ArrowLeft", Some(0x7B), Some(105)),
    ("ArrowRight", Some(0x7C), Some(106)),
    ("ArrowDown", Some(0x7D), Some(108)),
    ("ArrowUp", Some(0x7E), Some(103)),
    ("ShiftLeft", Some(0x38), Some(42)),
    ("ShiftRight", Some(0x3C), Some(54)),
    ("ControlLeft", Some(0x3B), Some(29)),
    ("ControlRight", Some(0x3E), Some(97)),
    ("AltLeft", Some(0x3A), Some(56)),
    ("AltRight", Some(0x3D), Some(100)),
    ("MetaLeft", Some(0x37), Some(125)),
    ("MetaRight", Some(0x36), Some(126)),
    ("CapsLock", Some(0x39), Some(58)),
    ("ContextMenu", Some(0x6E), Some(127)),
    ("NumLock", Some(0x47), Some(69)),
    ("ScrollLock", None, Some(70)),
    ("PrintScreen", None, Some(99)),
    ("Pause", None, Some(119)),
    ("F1", Some(0x7A), Some(59)),
    ("F2", Some(0x78), Some(60)),
    ("F3", Some(0x63), Some(61)),
    ("F4", Some(0x76), Some(62)),
    ("F5", Some(0x60), Some(63)),
    ("F6", Some(0x61), Some(64)),
    ("F7", Some(0x62), Some(65)),
    ("F8", Some(0x64), Some(66)),
    ("F9", Some(0x65), Some(67)),
    ("F10", Some(0x6D), Some(68)),
    ("F11", Some(0x67), Some(87)),
    ("F12", Some(0x6F), Some(88)),
    ("F13", Some(0x69), Some(183)),
    ("F14", Some(0x6B), Some(184)),
    ("F15", Some(0x71), Some(185)),
    ("F16", Some(0x6A), Some(186)),
    ("F17", Some(0x40), Some(187)),
    ("F18", Some(0x4F), Some(188)),
    ("F19", Some(0x50), Some(189)),
    ("F20", Some(0x5A), Some(190)),
    ("Numpad0", Some(0x52), Some(82)),
    ("Numpad1", Some(0x53), Some(79)),
    ("Numpad2", Some(0x54), Some(80)),
    ("Numpad3", Some(0x55), Some(81)),
    ("Numpad4", Some(0x56), Some(75)),
    ("Numpad5", Some(0x57), Some(76)),
    ("Numpad6", Some(0x58), Some(77)),
    ("Numpad7", Some(0x59), Some(71)),
    ("Numpad8", Some(0x5B), Some(72)),
    ("Numpad9", Some(0x5C), Some(73)),
    ("NumpadAdd", Some(0x45), Some(78)),
    ("NumpadSubtract", Some(0x4E), Some(74)),
    ("NumpadMultiply", Some(0x43), Some(55)),
    ("NumpadDivide", Some(0x4B), Some(98)),
    ("NumpadDecimal", Some(0x41), Some(83)),
    ("NumpadEnter", Some(0x4C), Some(96)),
    ("NumpadEqual", Some(0x51), Some(117)),
    ("NumpadComma", Some(0x5F), Some(121)),
    ("Lang1", Some(0x68), Some(122)),
    ("Lang2", Some(0x66), Some(123)),
    ("AudioVolumeMute", Some(0x4A), Some(113)),
    ("AudioVolumeDown", Some(0x49), Some(114)),
    ("AudioVolumeUp", Some(0x48), Some(115)),
];

/// Every code the table knows, in table order.
pub fn codes() -> impl Iterator<Item = &'static str> {
    TABLE.iter().map(|(c, _, _)| *c)
}

fn row(code: &str) -> Option<&'static (&'static str, Option<u16>, Option<u16>)> {
    TABLE.iter().find(|(c, _, _)| *c == code)
}

/// The macOS virtual key code for a `KeyboardEvent.code`.
pub fn mac_vk(code: &str) -> Option<u16> {
    row(code).and_then(|r| r.1)
}

/// The Linux evdev code (`KEY_*`).
pub fn evdev(code: &str) -> Option<u16> {
    row(code).and_then(|r| r.2)
}

/// The X11 keycode (evdev + 8; X keycodes run 8..=255).
pub fn x11_keycode(code: &str) -> Option<u8> {
    evdev(code).and_then(|e| u8::try_from(e + 8).ok())
}

/// The modifiers a held key contributes to every other event's flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Modifier {
    Shift,
    Control,
    Alt,
    Meta,
}

pub fn modifier(code: &str) -> Option<Modifier> {
    match code {
        "ShiftLeft" | "ShiftRight" => Some(Modifier::Shift),
        "ControlLeft" | "ControlRight" => Some(Modifier::Control),
        "AltLeft" | "AltRight" => Some(Modifier::Alt),
        "MetaLeft" | "MetaRight" => Some(Modifier::Meta),
        _ => None,
    }
}

/// The eight modifier keys deskd releases at startup (vs-live R3: covers a
/// crash that left one down).
pub const MODIFIER_CODES: [&str; 8] =
    ["ShiftLeft", "ShiftRight", "ControlLeft", "ControlRight", "AltLeft", "AltRight", "MetaLeft", "MetaRight"];

/// X11 chords deskd refuses (vs-live N5): Ctrl+Alt+Backspace kills the X
/// server, Ctrl+Alt+F1..F12 switch to a console.
pub fn x11_refused_chord(held: &[Modifier], code: &str) -> bool {
    let ctrl_alt = held.contains(&Modifier::Control) && held.contains(&Modifier::Alt);
    if !ctrl_alt {
        return false;
    }
    code == "Backspace" || matches!(code.strip_prefix('F').and_then(|n| n.parse::<u8>().ok()), Some(1..=12))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_code_is_unique_and_round_trips() {
        let mut seen = HashSet::new();
        for c in codes() {
            assert!(seen.insert(c), "{c} listed twice");
            let r = row(c).unwrap();
            assert_eq!(mac_vk(c), r.1);
            assert_eq!(evdev(c), r.2);
        }
        assert!(seen.len() >= 120, "only {} codes", seen.len());
    }

    #[test]
    fn host_codes_are_unique_per_platform() {
        let mut mac = HashSet::new();
        let mut ev = HashSet::new();
        for (c, m, e) in TABLE {
            if let Some(m) = m {
                assert!(mac.insert(*m), "macOS kVK {m:#x} used twice (at {c})");
            }
            if let Some(e) = e {
                assert!(ev.insert(*e), "evdev {e} used twice (at {c})");
            }
        }
    }

    #[test]
    fn every_evdev_code_has_an_x11_keycode() {
        for c in codes() {
            if evdev(c).is_some() {
                let k = x11_keycode(c).unwrap_or_else(|| panic!("{c} has no X11 keycode"));
                assert!(k >= 8);
                assert_eq!(k as u16, evdev(c).unwrap() + 8);
            }
        }
    }

    #[test]
    fn known_values() {
        assert_eq!(mac_vk("KeyA"), Some(0));
        assert_eq!(mac_vk("MetaLeft"), Some(0x37));
        assert_eq!(mac_vk("ArrowUp"), Some(0x7E));
        assert_eq!(evdev("KeyA"), Some(30));
        assert_eq!(x11_keycode("KeyA"), Some(38));
        assert_eq!(x11_keycode("Escape"), Some(9));
        assert_eq!(x11_keycode("Space"), Some(65));
        assert_eq!(mac_vk("NoSuchKey"), None);
        assert_eq!(mac_vk("PrintScreen"), None);
    }

    #[test]
    fn modifiers_cover_all_eight() {
        for c in MODIFIER_CODES {
            assert!(modifier(c).is_some(), "{c}");
            assert!(mac_vk(c).is_some() && evdev(c).is_some(), "{c}");
        }
        assert_eq!(modifier("KeyA"), None);
        assert_eq!(modifier("CapsLock"), None);
    }

    #[test]
    fn x11_refuses_the_server_killing_chords_only() {
        let ca = [Modifier::Control, Modifier::Alt];
        assert!(x11_refused_chord(&ca, "Backspace"));
        assert!(x11_refused_chord(&ca, "F1"));
        assert!(x11_refused_chord(&ca, "F12"));
        assert!(!x11_refused_chord(&ca, "F13"));
        assert!(!x11_refused_chord(&ca, "KeyT"));
        assert!(!x11_refused_chord(&[Modifier::Control], "Backspace"));
    }
}
