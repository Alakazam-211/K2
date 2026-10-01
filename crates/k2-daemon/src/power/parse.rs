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

/// Windows `powercfg /q SCHEME_CURRENT SUB_SLEEP RTCWAKE` → (AC, DC)
/// "Allow wake timers" index: 0 disable, 1 enable, 2 important only.
pub fn windows_rtcwake(text: &str) -> (Option<u32>, Option<u32>) {
    windows_setting_index(text)
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

/// Any `powercfg /q` single-setting output -> (AC, DC) current index.
pub fn windows_setting_index(text: &str) -> (Option<u32>, Option<u32>) {
    let read = |label: &str| {
        text.lines()
            .find(|l| l.contains(label))
            .and_then(|l| l.rsplit("0x").next())
            .and_then(|hex| u32::from_str_radix(hex.trim(), 16).ok())
    };
    (read("Current AC Power Setting Index"), read("Current DC Power Setting Index"))
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

    #[test]
    fn windows_rtcwake_indexes_parse() {
        let text = "    Power Setting GUID: bd3b718a-0680-4d9d-8ab2-e1d2b4ac806d  (Allow wake timers)\n      Possible Setting Index: 000\n    Current AC Power Setting Index: 0x00000001\n    Current DC Power Setting Index: 0x00000000\n";
        assert_eq!(windows_rtcwake(text), (Some(1), Some(0)));
        assert_eq!(windows_rtcwake("garbage"), (None, None));
    }

    /// S6 / D14 — the lid action reads the same way.
    #[test]
    fn windows_lidaction_index_parses() {
        let text = "    Power Setting GUID: 5ca83367-6e45-459f-a27b-476b1d01c936  (Lid close action)\n    Current AC Power Setting Index: 0x00000000\n    Current DC Power Setting Index: 0x00000001\n";
        assert_eq!(windows_setting_index(text), (Some(0), Some(1)));
        assert_eq!(windows_lid_action_word(0), "Do nothing");
        assert_eq!(windows_lid_action_word(1), "Sleep");
    }
}
