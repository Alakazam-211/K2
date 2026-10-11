//! Node health facts (`prd-k2-node-admin-v1` §10.1). Collected by the
//! unprivileged node (none need root), sent as a `health` frame every
//! 5 minutes and on change, ≤ [`MAX_HEALTH_BYTES`]. No floats: loads are
//! in thousandths. A fact the node couldn't read is `None`, never a
//! made-up default.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Cap on one `health` frame's JSON.
pub const MAX_HEALTH_BYTES: usize = 16 * 1024;
/// Cadence when nothing changed.
pub const HEALTH_SECS: u64 = 300;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    pub at: i64,
    pub boot_id: String,
    pub uptime_secs: Option<u64>,
    /// 1/5/15-minute load average × 1000.
    pub load_milli: Option<[u64; 3]>,
    pub cores: u64,
    pub mem_total_bytes: u64,
    pub mem_available_bytes: Option<u64>,
    pub swap_used_bytes: Option<u64>,
    /// `normal` | `warn` | `critical` (macOS pressure level, Linux PSI).
    pub memory_pressure: Option<String>,
    pub disks: Vec<DiskFact>,
    pub updates: Option<Updates>,
    pub sleep: Option<Sleep>,
    pub power: Power,
    pub thermal: Option<Thermal>,
    /// `on` | `off` | `unknown` (macOS `fdesetup status`; Linux: `luks` when
    /// /etc/crypttab lists a volume).
    pub disk_encryption: Option<String>,
    pub os: OsFact,
    pub executor: ExecutorFact,
    /// Jobs that look stuck (S9): no output and no CPU for a while. An
    /// alert, never a kill.
    #[serde(default)]
    pub stuck_jobs: Vec<StuckJob>,
    /// Facts that failed to read and why (short codes).
    #[serde(default)]
    pub errors: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskFact {
    /// `root`, `node_home`, …
    pub name: String,
    pub path: String,
    pub total_bytes: u64,
    pub free_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Updates {
    pub pending: Option<u32>,
    pub security: Option<u32>,
    pub reboot_required: Option<bool>,
    /// Unix seconds of the check (the list is cached; daily).
    pub checked_at: i64,
    /// `apt` | `dnf` | `pacman` | `zypper` | `softwareupdate`.
    pub source: String,
}

/// Sleep settings. macOS: `pmset -g` values in minutes (0 = never) and
/// 0/1 switches. Linux: whether the sleep targets are masked.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sleep {
    pub sleep: Option<i64>,
    pub disksleep: Option<i64>,
    pub displaysleep: Option<i64>,
    pub womp: Option<i64>,
    pub autorestart: Option<i64>,
    pub powernap: Option<i64>,
    pub standby: Option<i64>,
    /// Linux: `sleep.target` and `suspend.target` both masked.
    pub targets_masked: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Power {
    pub has_battery: bool,
    pub on_battery: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thermal {
    /// macOS `CPU_Speed_Limit` (100 = no throttling).
    pub cpu_speed_limit: Option<u32>,
    /// Hottest Linux thermal zone, milli-°C.
    pub max_zone_milli_c: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OsFact {
    pub name: String,
    pub version: String,
    pub kernel: String,
    pub arch: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutorFact {
    pub reachable: bool,
    pub version: Option<String>,
    pub remote_admin: Option<String>,
    #[serde(default)]
    pub approval_key_fps: Vec<String>,
    pub auto_update: Option<String>,
    /// Last self-update outcome the executor recorded.
    pub last_update: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StuckJob {
    pub job_id: String,
    pub generation: u32,
    pub quiet_secs: u64,
}

impl Health {
    /// Encoded size check (the frame cap).
    pub fn fits(&self) -> bool {
        serde_json::to_vec(self).map(|v| v.len() <= MAX_HEALTH_BYTES).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_health_is_small_and_round_trips() {
        let h = Health { boot_id: "b".into(), cores: 8, ..Default::default() };
        assert!(h.fits());
        let back: Health = serde_json::from_str(&serde_json::to_string(&h).unwrap()).unwrap();
        assert_eq!(back, h);
        // Canonical JSON works (no floats anywhere).
        assert!(crate::canonical::to_string(&h).is_ok());
    }
}
