//! Pure parsers for each OS's power facts. Compiled on every OS so the
//! tests run everywhere; the OS layers call them.

#![allow(dead_code)]

use super::PowerSource;

/// macOS `pmset -g batt` → power source.
pub fn pmset_batt(text: &str) -> PowerSource {
    let on_ac = if text.contains("'AC Power'") {
        Some(true)
    } else if text.contains("'Battery Power'") {
        Some(false)
    } else {
        None
    };
    let battery_percent = text
        .split(|c: char| c.is_whitespace() || c == ';')
        .find_map(|tok| tok.strip_suffix('%').and_then(|n| n.parse::<u8>().ok()));
    PowerSource { on_ac, battery_percent }
}

/// Linux `/sys/class/power_supply/*` → power source. `entries` =
/// (type, online, capacity) per supply.
pub fn linux_supplies(entries: &[(String, Option<u8>, Option<u8>)]) -> PowerSource {
    let mains_online = entries
        .iter()
        .filter(|(t, _, _)| t == "Mains" || t == "USB")
        .any(|(_, online, _)| *online == Some(1));
    let battery = entries
        .iter()
        .find(|(t, _, _)| t == "Battery")
        .and_then(|(_, _, cap)| *cap);
    let on_ac = if mains_online {
        Some(true)
    } else if battery.is_some() {
        Some(false)
    } else {
        None // desktop / server with no readable supply
    };
    PowerSource { on_ac, battery_percent: battery }
}

/// Linux sysfs `scope`: `Device` marks a peripheral's battery (a pen,
/// mouse or headset). Only `System` or no scope powers this machine.
pub fn linux_supply_powers_system(scope: Option<&str>) -> bool {
    scope != Some("Device")
}

/// S6 — Windows `LIDACTION` index in words (0 Do nothing, 1 Sleep,
/// 2 Hibernate, 3 Shut down).
pub fn windows_lid_action_word(index: u32) -> &'static str {
    match index {
        0 => "Do nothing",
        1 => "Sleep",
        2 => "Hibernate",
        3 => "Shut down",
        _ => "not Do nothing",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pmset_batt_parses_ac_battery_and_desktop() {
        let ac = "Now drawing from 'AC Power'\n -InternalBattery-0 (id=123)\t85%; charged; 0:00 remaining present: true\n";
        assert_eq!(pmset_batt(ac), PowerSource { on_ac: Some(true), battery_percent: Some(85) });
        let batt = "Now drawing from 'Battery Power'\n -InternalBattery-0 (id=123)\t19%; discharging; 1:02 remaining present: true\n";
        assert_eq!(pmset_batt(batt), PowerSource { on_ac: Some(false), battery_percent: Some(19) });
        assert_eq!(
            pmset_batt("Now drawing from 'AC Power'\n"),
            PowerSource { on_ac: Some(true), battery_percent: None }
        );
    }

    #[test]
    fn linux_supplies_map_to_a_source() {
        let s = |t: &str, o: Option<u8>, c: Option<u8>| (t.to_string(), o, c);
        assert_eq!(
            linux_supplies(&[s("Mains", Some(1), None), s("Battery", None, Some(64))]),
            PowerSource { on_ac: Some(true), battery_percent: Some(64) }
        );
        assert_eq!(
            linux_supplies(&[s("Mains", Some(0), None), s("Battery", None, Some(15))]),
            PowerSource { on_ac: Some(false), battery_percent: Some(15) }
        );
        assert_eq!(linux_supplies(&[]), PowerSource { on_ac: None, battery_percent: None });
    }

    /// Seen on z13flow: a stylus battery (`scope` Device, capacity 0)
    /// listed before BAT0 made the laptop read as 0 %, which would pause
    /// wake and Keep awake on battery.
    #[test]
    fn peripheral_batteries_are_not_the_system_battery() {
        assert!(!linux_supply_powers_system(Some("Device")));
        assert!(linux_supply_powers_system(Some("System")));
        assert!(linux_supply_powers_system(None));
        let s = |t: &str, o: Option<u8>, c: Option<u8>| (t.to_string(), o, c);
        let sysfs = [
            (s("Mains", Some(0), None), None),
            (s("Battery", Some(1), Some(0)), Some("Device")),
            (s("Battery", None, Some(64)), None),
        ];
        let kept: Vec<_> = sysfs
            .iter()
            .filter(|(_, scope)| linux_supply_powers_system(*scope))
            .map(|(e, _)| e.clone())
            .collect();
        assert_eq!(linux_supplies(&kept), PowerSource { on_ac: Some(false), battery_percent: Some(64) });
    }

    /// S6 / D14 — the lid action index in words.
    #[test]
    fn windows_lid_action_words() {
        assert_eq!(windows_lid_action_word(0), "Do nothing");
        assert_eq!(windows_lid_action_word(1), "Sleep");
    }
}
