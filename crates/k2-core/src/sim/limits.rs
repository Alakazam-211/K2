//! Box limits for devices (`prd-mobile-dev-v1` §3.5), in
//! `~/.k2/sim.json` next to `disk-guard.json`. Owner-editable from
//! Settings → K2 Server (`POST /cli/sim/settings`, owner/Admin).
//!
//! Missing / unreadable / invalid file → defaults (logged, never an error).

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::disk_guard::GIB;

pub const SETTINGS_FILE: &str = "sim.json";

/// Boxes with at most this much RAM get [`DEFAULT_MAX_DEVICES_SMALL`].
pub const SMALL_BOX_RAM: u64 = 32 * GIB;
pub const DEFAULT_MAX_DEVICES_SMALL: u32 = 2;
pub const DEFAULT_MAX_DEVICES_LARGE: u32 = 4;
pub const DEFAULT_MIN_FREE_RAM: u64 = 4 * GIB;
pub const DEFAULT_IDLE_SHUTDOWN_SECS: u64 = 20 * 60;
pub const DEFAULT_UNUSED_DATA_DAYS: u32 = 14;
/// Bytes a first boot needs on disk (SDK + one image ≈ 4.5 GB, measured
/// S0; AVD data grows to ~2 GB, plus a 2 GB Quick Boot snapshot).
pub const FIRST_BOOT_DISK_BYTES: u64 = 10 * GIB;
/// Bytes a boot of an existing device needs.
pub const BOOT_DISK_BYTES: u64 = 3 * GIB;

/// The saved settings; `None` = default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SimSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_devices: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_free_ram_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_shutdown_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unused_data_days: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_by: Option<String>,
}

/// The limits in force.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    pub max_devices: u32,
    pub min_free_ram_bytes: u64,
    pub idle_shutdown_secs: u64,
    pub unused_data_days: u32,
}

impl Limits {
    pub fn idle_shutdown(&self) -> Duration {
        Duration::from_secs(self.idle_shutdown_secs)
    }
}

/// Defaults for a box with `total_ram` bytes (unknown → small box).
pub fn defaults_for(total_ram: Option<u64>) -> Limits {
    let max_devices = match total_ram {
        Some(r) if r > SMALL_BOX_RAM => DEFAULT_MAX_DEVICES_LARGE,
        _ => DEFAULT_MAX_DEVICES_SMALL,
    };
    Limits {
        max_devices,
        min_free_ram_bytes: DEFAULT_MIN_FREE_RAM,
        idle_shutdown_secs: DEFAULT_IDLE_SHUTDOWN_SECS,
        unused_data_days: DEFAULT_UNUSED_DATA_DAYS,
    }
}

impl SimSettings {
    pub fn validate(&self) -> Result<(), String> {
        if let Some(n) = self.max_devices {
            if !(1..=32).contains(&n) {
                return Err(format!("maxDevices must be 1–32 (got {n})"));
            }
        }
        if let Some(s) = self.idle_shutdown_secs {
            if !(60..=7 * 24 * 3600).contains(&s) {
                return Err(format!("idleShutdownSecs must be 60–604800 (got {s})"));
            }
        }
        if let Some(d) = self.unused_data_days {
            if !(1..=3650).contains(&d) {
                return Err(format!("unusedDataDays must be 1–3650 (got {d})"));
            }
        }
        if let Some(b) = self.min_free_ram_bytes {
            if b > 256 * GIB {
                return Err("minFreeRamBytes is over 256 GiB".into());
            }
        }
        Ok(())
    }

    pub fn effective(&self, total_ram: Option<u64>) -> Limits {
        let d = defaults_for(total_ram);
        Limits {
            max_devices: self.max_devices.unwrap_or(d.max_devices),
            min_free_ram_bytes: self.min_free_ram_bytes.unwrap_or(d.min_free_ram_bytes),
            idle_shutdown_secs: self.idle_shutdown_secs.unwrap_or(d.idle_shutdown_secs),
            unused_data_days: self.unused_data_days.unwrap_or(d.unused_data_days),
        }
    }
}

pub fn settings_path() -> std::path::PathBuf {
    super::base().join(SETTINGS_FILE)
}

pub fn load_settings() -> SimSettings {
    let path = settings_path();
    let Ok(raw) = std::fs::read_to_string(&path) else { return SimSettings::default() };
    match serde_json::from_str::<SimSettings>(&raw) {
        Ok(s) => match s.validate() {
            Ok(()) => s,
            Err(e) => {
                crate::log_debug!("[sim] WARN {} invalid ({e}); using the defaults", path.display());
                SimSettings::default()
            }
        },
        Err(e) => {
            crate::log_debug!("[sim] WARN {} unreadable ({e}); using the defaults", path.display());
            SimSettings::default()
        }
    }
}

/// Save atomically, 0600.
pub fn save_settings(s: &SimSettings) -> Result<(), String> {
    s.validate()?;
    let path = settings_path();
    let dir = path.parent().ok_or("settings path has no parent")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    let tmp = dir.join(format!(".{SETTINGS_FILE}.{}.tmp", std::process::id()));
    let body = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, body).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, &path).map_err(|e| format!("rename {}: {e}", path.display()))
}

/// The limits in force on this box now.
pub fn current() -> Limits {
    load_settings().effective(crate::disk_guard::box_mem().0)
}

/// Why a boot is refused by the box limits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LimitRefusal {
    TooManyDevices { running: u32, max: u32 },
    LowRam { available: u64, need: u64 },
}

impl LimitRefusal {
    pub fn message(&self) -> String {
        match self {
            LimitRefusal::TooManyDevices { running, max } => format!(
                "this server already runs {running} of its {max} devices. Stop one (`k2 sim stop` in that workspace) or ask the owner to raise the limit in Settings → K2 Server → Mobile devices."
            ),
            LimitRefusal::LowRam { available, need } => format!(
                "not enough free memory to boot a device: {:.1} GB free, {:.1} GB needed. Stop a device or an App, then try again.",
                *available as f64 / GIB as f64,
                *need as f64 / GIB as f64
            ),
        }
    }
}

/// The boot check. `available_ram` `None` (macOS has no honest number) is
/// never a reason to refuse.
pub fn check_boot(l: &Limits, running: u32, available_ram: Option<u64>) -> Result<(), LimitRefusal> {
    if running >= l.max_devices {
        return Err(LimitRefusal::TooManyDevices { running, max: l.max_devices });
    }
    if let Some(a) = available_ram {
        if a < l.min_free_ram_bytes {
            return Err(LimitRefusal::LowRam { available: a, need: l.min_free_ram_bytes });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_follow_box_ram() {
        assert_eq!(defaults_for(Some(32 * GIB)).max_devices, 2);
        assert_eq!(defaults_for(Some(64 * GIB)).max_devices, 4);
        assert_eq!(defaults_for(None).max_devices, 2);
        let d = defaults_for(Some(16 * GIB));
        assert_eq!(d.idle_shutdown_secs, 1200);
        assert_eq!(d.min_free_ram_bytes, 4 * GIB);
        assert_eq!(d.unused_data_days, 14);
    }

    #[test]
    fn third_boot_on_a_cap_of_two_is_refused_with_a_reason() {
        let l = defaults_for(Some(16 * GIB));
        assert_eq!(check_boot(&l, 1, Some(8 * GIB)), Ok(()));
        let r = check_boot(&l, 2, Some(8 * GIB)).expect_err("third boot refused");
        assert_eq!(r, LimitRefusal::TooManyDevices { running: 2, max: 2 });
        assert!(r.message().contains("2 of its 2 devices"), "{}", r.message());
        let low = check_boot(&l, 0, Some(GIB)).expect_err("low RAM refused");
        assert!(matches!(low, LimitRefusal::LowRam { .. }));
        assert_eq!(check_boot(&l, 0, None), Ok(()), "unknown RAM never refuses");
    }

    #[test]
    fn settings_override_and_validate() {
        let s = SimSettings { max_devices: Some(3), idle_shutdown_secs: Some(600), ..Default::default() };
        assert_eq!(s.validate(), Ok(()));
        let l = s.effective(Some(16 * GIB));
        assert_eq!((l.max_devices, l.idle_shutdown_secs), (3, 600));
        assert!(SimSettings { max_devices: Some(0), ..Default::default() }.validate().is_err());
        assert!(SimSettings { idle_shutdown_secs: Some(5), ..Default::default() }.validate().is_err());
        let bad: Result<SimSettings, _> = serde_json::from_str(r#"{"maxDevice": 3}"#);
        assert!(bad.is_err(), "unknown fields are refused");
    }
}
