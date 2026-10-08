//! The machine owner's word: `policy.toml` and `control.toml` (§8.5,
//! §10.3). Both live in the config dir, which the node user can't write.
//! `k2-node` only reads them; `k2-node pause|drain|resume|policy set`
//! (run as root or a `k2nodectl` member) and the node machine's K2 write
//! them.
//!
//! A policy file that doesn't parse is fail-closed: the node refuses new
//! jobs with `local_paused` and says why in its status.

use std::collections::BTreeMap;
use std::path::Path;

use k2_node_proto::frames::Control;
use serde::{Deserialize, Serialize};

pub const GIB: u64 = 1 << 30;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AvailabilityMode {
    Always,
    Idle,
    PluggedIn,
    IdleAndPluggedIn,
    Window,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnOwnerReturn {
    Finish,
    PauseNow,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowSpec {
    /// `mon-fri`, `sat,sun`, `daily`.
    pub days: String,
    /// `HH:MM` local time.
    pub from: String,
    pub to: String,
}

/// `policy.toml`. Every field optional in the file; [`Policy::resolve`]
/// fills machine-dependent defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyFile {
    pub cpu_cap_percent: Option<u64>,
    pub mem_cap_gb: Option<u64>,
    pub disk_budget_gb: Option<u64>,
    pub disk_floor_gb: Option<u64>,
    pub availability: Option<AvailabilityMode>,
    pub window: Option<Vec<WindowSpec>>,
    pub on_owner_return: Option<OnOwnerReturn>,
    pub max_parallel: Option<u32>,
    pub workspaces: Option<Vec<String>>,
    pub foreign_locks: Option<Vec<String>>,
    pub write_smoke_lock: Option<String>,
    pub network: Option<String>,
}

/// Facts about the machine the defaults depend on.
#[derive(Debug, Clone)]
pub struct MachineFacts {
    pub cores: u64,
    pub mem_bytes: u64,
    pub disk_total_bytes: u64,
    pub has_battery: bool,
    pub is_macos: bool,
}

/// The policy in force.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Policy {
    pub cpu_cap_percent: u64,
    pub mem_cap_bytes: u64,
    pub disk_budget_bytes: u64,
    pub disk_floor_bytes: u64,
    pub availability: AvailabilityMode,
    pub window: Vec<WindowSpec>,
    pub on_owner_return: OnOwnerReturn,
    pub max_parallel: u32,
    pub workspaces: Vec<String>,
    pub foreign_locks: Vec<String>,
    pub write_smoke_lock: Option<String>,
    pub network: String,
}

impl Policy {
    pub fn resolve(f: &PolicyFile, m: &MachineFacts) -> Result<Self, String> {
        let cpu = f.cpu_cap_percent.unwrap_or(75);
        if !(1..=100).contains(&cpu) {
            return Err(format!("cpu_cap_percent must be 1..100, got {cpu}"));
        }
        let mem_default = (m.mem_bytes / GIB * 3 / 4).max(1);
        let floor_default = (m.disk_total_bytes / 10 / GIB).max(30);
        let max_parallel = f.max_parallel.unwrap_or(2);
        if max_parallel == 0 || max_parallel > 64 {
            return Err(format!("max_parallel must be 1..64, got {max_parallel}"));
        }
        let availability = f.availability.clone().unwrap_or(if m.has_battery {
            AvailabilityMode::PluggedIn
        } else {
            AvailabilityMode::Always
        });
        let window = f.window.clone().unwrap_or_default();
        for w in &window {
            parse_days(&w.days).ok_or_else(|| format!("window days {:?} not understood", w.days))?;
            parse_hhmm(&w.from).ok_or_else(|| format!("window from {:?} is not HH:MM", w.from))?;
            parse_hhmm(&w.to).ok_or_else(|| format!("window to {:?} is not HH:MM", w.to))?;
        }
        if availability == AvailabilityMode::Window && window.is_empty() {
            return Err("availability = \"window\" needs at least one [[window]]".into());
        }
        let network = f.network.clone().unwrap_or_else(|| "full".into());
        if network != "full" && network != "none" {
            return Err(format!("network must be \"full\" or \"none\", got {network:?}"));
        }
        let foreign_locks = f.foreign_locks.clone().unwrap_or_else(|| {
            if m.is_macos {
                vec![]
            } else {
                vec!["/home/sew-ci/sew-build/.job.lock".to_string()]
            }
        });
        let write_smoke_lock = match &f.write_smoke_lock {
            Some(s) if s.is_empty() => None,
            Some(s) => Some(s.clone()),
            None => Some("/var/tmp/k2-smoke.lock".to_string()),
        };
        Ok(Self {
            cpu_cap_percent: cpu,
            mem_cap_bytes: f.mem_cap_gb.unwrap_or(mem_default) * GIB,
            disk_budget_bytes: f.disk_budget_gb.unwrap_or(150) * GIB,
            disk_floor_bytes: f.disk_floor_gb.unwrap_or(floor_default) * GIB,
            availability,
            window,
            on_owner_return: f.on_owner_return.clone().unwrap_or(OnOwnerReturn::Finish),
            max_parallel,
            workspaces: f.workspaces.clone().unwrap_or_default(),
            foreign_locks,
            write_smoke_lock,
            network,
        })
    }

    pub fn cpu_cap_millis(&self, cores: u64) -> u64 {
        cores * 1000 * self.cpu_cap_percent / 100
    }
}

/// Read `policy.toml` (missing = all defaults).
pub fn read_policy_file(path: &Path) -> Result<PolicyFile, String> {
    match std::fs::read_to_string(path) {
        Ok(s) => toml::from_str(&s).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(PolicyFile::default()),
        Err(e) => Err(format!("read {}: {e}", path.display())),
    }
}

/// `control.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlFile {
    pub state: Option<String>,
    pub by: Option<String>,
    pub at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ControlState {
    pub control: Control,
    pub by: Option<String>,
    pub at: Option<i64>,
}

pub fn read_control(path: &Path) -> Result<ControlState, String> {
    let f: ControlFile = match std::fs::read_to_string(path) {
        Ok(s) => toml::from_str(&s).map_err(|e| format!("{}: {e}", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => ControlFile::default(),
        Err(e) => return Err(format!("read {}: {e}", path.display())),
    };
    let control = match f.state.as_deref() {
        None => Control::Active,
        Some(s) => Control::parse(s).ok_or_else(|| format!("control state {s:?} is not active|paused|draining|stopped"))?,
    };
    Ok(ControlState { control, by: f.by, at: f.at })
}

/// Write `control.toml` (0664, group from the setgid config dir).
pub fn write_control(path: &Path, control: Control, by: &str, at: i64) -> Result<(), String> {
    let f = ControlFile { state: Some(control.as_str().to_string()), by: Some(by.to_string()), at: Some(at) };
    let body = toml::to_string(&f).map_err(|e| format!("encode control: {e}"))?;
    let text = format!("# Written by k2-node. The machine owner's pause switch.\n{body}");
    crate::util::atomic_write(path, text.as_bytes(), 0o664).map_err(|e| writable_hint(path, e))
}

/// Set `key = value` pairs in `policy.toml`, validating the result.
pub fn set_policy(path: &Path, pairs: &[(String, String)], m: &MachineFacts) -> Result<Policy, String> {
    let current = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("read {}: {e}", path.display())),
    };
    let mut table: toml::Table = toml::from_str(&current).map_err(|e| format!("{}: {e}", path.display()))?;
    for (k, v) in pairs {
        let value: toml::Value = match toml::from_str::<toml::Table>(&format!("x = {v}")) {
            Ok(mut t) => t.remove("x").unwrap_or(toml::Value::String(v.clone())),
            Err(_) => toml::Value::String(v.clone()),
        };
        table.insert(k.clone(), value);
    }
    let text = toml::to_string(&table).map_err(|e| format!("encode policy: {e}"))?;
    let parsed: PolicyFile = toml::from_str(&text).map_err(|e| format!("policy not valid: {e}"))?;
    let policy = Policy::resolve(&parsed, m)?;
    crate::util::atomic_write(path, text.as_bytes(), 0o664).map_err(|e| writable_hint(path, e))?;
    Ok(policy)
}

fn writable_hint(path: &Path, e: String) -> String {
    format!(
        "{e}\n{} is the machine owner's: run this as root (sudo) or as a member of the k2nodectl group",
        path.display()
    )
}

/// Days from `mon-fri`, `sat,sun`, `daily`, `*` → bitmask, Sunday = bit 0.
pub fn parse_days(s: &str) -> Option<u8> {
    let s = s.trim().to_ascii_lowercase();
    if s == "daily" || s == "*" || s == "all" {
        return Some(0x7f);
    }
    const NAMES: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];
    let idx = |n: &str| NAMES.iter().position(|x| *x == n.trim());
    let mut mask = 0u8;
    for part in s.split(',') {
        if let Some((a, b)) = part.split_once('-') {
            let (a, b) = (idx(a)?, idx(b)?);
            let mut i = a;
            loop {
                mask |= 1 << i;
                if i == b {
                    break;
                }
                i = (i + 1) % 7;
            }
        } else {
            mask |= 1 << idx(part)?;
        }
    }
    (mask != 0).then_some(mask)
}

/// `HH:MM` → minutes after midnight.
pub fn parse_hhmm(s: &str) -> Option<u32> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// Is local time (weekday 0 = Sunday, minutes) inside any window?
/// A window whose `to` is before `from` wraps past midnight; its day
/// applies to the evening part, the next morning counts for the day after.
pub fn in_window(windows: &[WindowSpec], weekday: u32, minute: u32) -> bool {
    windows.iter().any(|w| {
        let (Some(days), Some(from), Some(to)) = (parse_days(&w.days), parse_hhmm(&w.from), parse_hhmm(&w.to)) else {
            return false;
        };
        let day_ok = |d: u32| days & (1 << (d % 7)) != 0;
        if from <= to {
            day_ok(weekday) && minute >= from && minute < to
        } else {
            (day_ok(weekday) && minute >= from) || (day_ok((weekday + 6) % 7) && minute < to)
        }
    })
}

/// Local weekday (0 = Sunday) and minute of day.
pub fn local_weekday_minute(t: i64) -> (u32, u32) {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let tt: libc::time_t = t as libc::time_t;
    unsafe { libc::localtime_r(&tt, &mut tm) };
    (tm.tm_wday as u32, (tm.tm_hour * 60 + tm.tm_min) as u32)
}

/// A default `policy.toml` for the installer (all fields commented).
pub fn default_policy_toml() -> String {
    r#"# k2-node policy: the machine owner's limits for jobs from the controller.
# The controller can narrow these, never widen them. Edit with
#   sudo k2-node policy set cpu_cap_percent=50
# Uncomment a line to change a default.

# cpu_cap_percent = 75          # of all cores
# mem_cap_gb = 24               # default: 3/4 of RAM
# disk_budget_gb = 150
# disk_floor_gb = 30            # default: max(10% of disk, 30)
# availability = "always"       # always | plugged_in | window (idle modes: later)
#                               # default: plugged_in on machines with a battery
# on_owner_return = "finish"    # finish | pause_now
# max_parallel = 2
# workspaces = []               # empty = whatever the controller grants
# foreign_locks = ["/home/sew-ci/sew-build/.job.lock"]
# write_smoke_lock = "/var/tmp/k2-smoke.lock"
# network = "full"              # full | none
#
# [[window]]
# days = "mon-fri"
# from = "19:00"
# to = "07:00"
"#
    .to_string()
}

/// Owner labels are only in controller.json; nothing here.
pub type Labels = BTreeMap<String, String>;

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(battery: bool, mac: bool) -> MachineFacts {
        MachineFacts { cores: 10, mem_bytes: 32 * GIB, disk_total_bytes: 1000 * GIB, has_battery: battery, is_macos: mac }
    }

    #[test]
    fn defaults_follow_the_machine() {
        let p = Policy::resolve(&PolicyFile::default(), &facts(true, true)).unwrap();
        assert_eq!(p.availability, AvailabilityMode::PluggedIn, "laptops lend only when plugged in");
        assert_eq!(p.mem_cap_bytes, 24 * GIB);
        assert_eq!(p.disk_floor_bytes, 100 * GIB);
        assert!(p.foreign_locks.is_empty());
        let p = Policy::resolve(&PolicyFile::default(), &facts(false, false)).unwrap();
        assert_eq!(p.availability, AvailabilityMode::Always);
        assert_eq!(p.foreign_locks, vec!["/home/sew-ci/sew-build/.job.lock".to_string()]);
        assert_eq!(p.write_smoke_lock.as_deref(), Some("/var/tmp/k2-smoke.lock"));
        assert_eq!(p.cpu_cap_millis(10), 7500);
    }

    #[test]
    fn bad_policy_is_refused() {
        let f: Result<PolicyFile, _> = toml::from_str("cpu_cap = 3");
        assert!(f.is_err(), "unknown fields are refused, typos never pass silently");
        let f = PolicyFile { cpu_cap_percent: Some(0), ..Default::default() };
        assert!(Policy::resolve(&f, &facts(false, false)).is_err());
        let f = PolicyFile { availability: Some(AvailabilityMode::Window), ..Default::default() };
        assert!(Policy::resolve(&f, &facts(false, false)).is_err());
        let f: PolicyFile = toml::from_str(&default_policy_toml()).unwrap();
        assert_eq!(f, PolicyFile::default(), "the default file is all comments");
    }

    #[test]
    fn control_round_trip_and_missing_is_active() {
        let d = crate::util::temp_dir("control");
        let p = d.join("control.toml");
        assert_eq!(read_control(&p).unwrap().control, Control::Active);
        write_control(&p, Control::Paused, "rosson", 7).unwrap();
        let c = read_control(&p).unwrap();
        assert_eq!((c.control, c.by.as_deref(), c.at), (Control::Paused, Some("rosson"), Some(7)));
        std::fs::write(&p, "state = \"asleep\"").unwrap();
        assert!(read_control(&p).is_err());
    }

    #[test]
    fn set_policy_parses_values_and_validates() {
        let d = crate::util::temp_dir("policy");
        let p = d.join("policy.toml");
        let pol = set_policy(&p, &[("cpu_cap_percent".into(), "50".into()), ("availability".into(), "always".into())], &facts(true, true)).unwrap();
        assert_eq!(pol.cpu_cap_percent, 50);
        assert_eq!(pol.availability, AvailabilityMode::Always);
        assert!(set_policy(&p, &[("max_parallel".into(), "0".into())], &facts(true, true)).is_err());
        assert_eq!(read_policy_file(&p).unwrap().max_parallel, None, "a refused set leaves the file alone");
    }

    #[test]
    fn windows_and_days() {
        assert_eq!(parse_days("mon-fri"), Some(0b0111110));
        assert_eq!(parse_days("sat,sun"), Some(0b1000001));
        assert_eq!(parse_days("fri-mon"), Some(0b1100011));
        assert_eq!(parse_days("someday"), None);
        let w = vec![WindowSpec { days: "mon-fri".into(), from: "19:00".into(), to: "07:00".into() }];
        assert!(in_window(&w, 1, 20 * 60), "Monday evening");
        assert!(in_window(&w, 2, 6 * 60), "Tuesday early morning counts for Monday night");
        assert!(!in_window(&w, 1, 12 * 60), "Monday noon");
        assert!(!in_window(&w, 1, 6 * 60), "Monday early morning follows Sunday, which is out");
        let day = vec![WindowSpec { days: "daily".into(), from: "09:00".into(), to: "17:00".into() }];
        assert!(in_window(&day, 0, 9 * 60));
        assert!(!in_window(&day, 0, 17 * 60));
    }
}
