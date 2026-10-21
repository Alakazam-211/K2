//! The disk guard's shared contract (`prd-app-resources-disk-guard-v1`,
//! Slice A0, 0.46.3).
//!
//! One place for everything both lanes build on:
//!
//! - **Mode and lines** ([`Mode`], [`Lines`], [`Settings`]): the owner's
//!   switch and thresholds, kept in `~/.k2/disk-guard.json` (owner-only
//!   route; not `app_settings`, PRD §7.2). `K2_DISK_GUARD=off|warn|on` in
//!   the daemon's environment wins (the instant way back, §7.1). Defaults:
//!   `on` on Linux, `warn` on macOS (APFS reads nearly full), `off` in
//!   tests (`cfg(test)` / the `test-util` feature) so no existing test can
//!   start failing on a runner that happens to have little free disk.
//! - **Filesystems** ([`probe`], [`Fs`], [`group`]): `statvfs` per tracked
//!   path, grouped by device (`st_dev`), with the mount point and fs type.
//!   Counted like `df`: used ÷ (used + available to normal users).
//! - **Levels and the check** ([`Level`], [`check`], [`reserve_ok`]):
//!   `ok` / `warn` / `refuse` / `critical`, "whichever comes first"; unknown
//!   is never a reason to block; mode `warn` turns a refusal into a
//!   warning; mode `off` passes with nothing added (today's behaviour).
//! - **The view's row types** ([`Consumer`], [`Owner`], [`Snapshot`]) and
//!   the slot the scanner publishes into ([`publish_snapshot`],
//!   [`latest_snapshot`]), plus the rescan control ([`request_rescan`],
//!   [`scanner_wait`]). The measuring lane fills them; the view reads them.
//! - **The guard log** (`~/.k2/disk-guard-log.jsonl`, [`log_event`],
//!   [`recent`]): append-only, 0600, rotated at 5 MiB (the `auth_audit`
//!   pattern). A failed append never fails the action.
//! - **Box numbers** ([`box_mem`], [`load5`], [`cores`]).
//! - **A fake-disk seam** for tests ([`fake`]): a fake applies only to
//!   paths under its root, and may carry its own mode, so a guard test
//!   never changes what an unrelated test running beside it sees.
//!
//! Nothing here stops a running App, blocks a restart or deletes data:
//! callers put the check only where *new* disk work starts (PRD §6.1).

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock, RwLock};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value as J};

/// The daemon-environment switch that wins over the setting.
pub const ENV_MODE: &str = "K2_DISK_GUARD";
/// `~/.k2/disk-guard.json`.
pub const SETTINGS_FILE: &str = "disk-guard.json";
/// `~/.k2/disk-guard-log.jsonl`.
pub const LOG_FILE: &str = "disk-guard-log.jsonl";
/// Rotate the guard log at this size (one old copy kept).
pub const LOG_ROTATE_BYTES: u64 = 5 * 1024 * 1024;
/// The view's `recent` length.
pub const RECENT: usize = 10;
/// An override reason is at most this many characters.
pub const MAX_REASON_CHARS: usize = 500;

pub const MIB: u64 = 1024 * 1024;
pub const GIB: u64 = 1024 * MIB;

/// Under this share of free inodes the fs counts as `warn` (never refuse
/// in Slice A, PRD §6.3 / Q6).
pub const INODES_WARN_FREE_PCT: f64 = 5.0;

/// Re-arm distance for crossings (PRD §10.1): 5 points below the line and
/// 5 GiB more free than it.
pub const REARM_POINTS: f64 = 5.0;
pub const REARM_BYTES: u64 = 5 * GIB;

// ── mode ─────────────────────────────────────────────────────────────

/// The guard's owner switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Warn, refuse and critical.
    On,
    /// Warn only: a refusal becomes a warning that says what `on` would do.
    Warn,
    /// Nothing. Answers carry no new fields: today's behaviour, exactly.
    Off,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Mode> {
        match s.trim().to_ascii_lowercase().as_str() {
            "on" => Some(Mode::On),
            "warn" => Some(Mode::Warn),
            "off" => Some(Mode::Off),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Mode::On => "on",
            Mode::Warn => "warn",
            Mode::Off => "off",
        }
    }

    /// The default when neither the environment nor the setting says:
    /// `off` in tests, `warn` on macOS, `on` everywhere else (Linux).
    pub fn platform_default() -> Mode {
        if cfg!(any(test, feature = "test-util")) {
            Mode::Off
        } else if cfg!(target_os = "macos") {
            Mode::Warn
        } else {
            Mode::On
        }
    }
}

/// Where the effective mode came from (`guard.modeSource` in the view).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModeSource {
    Default,
    Setting,
    Env,
}

// ── lines ────────────────────────────────────────────────────────────

/// The thresholds (PRD Q1, §6.3). Percentages are whole numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Lines {
    pub warn_pct: u32,
    pub warn_free_bytes: u64,
    pub refuse_pct: u32,
    pub reserve_bytes: u64,
    pub critical_pct: u32,
    pub critical_free_bytes: u64,
}

impl Default for Lines {
    fn default() -> Self {
        Lines {
            warn_pct: 85,
            warn_free_bytes: 20 * GIB,
            refuse_pct: 95,
            reserve_bytes: 10 * GIB,
            critical_pct: 98,
            critical_free_bytes: 2 * GIB,
        }
    }
}

impl Lines {
    /// The PRD §7.2 rules. The error names the rule that broke (a `400`).
    pub fn validate(&self) -> Result<(), String> {
        if !(self.warn_pct < self.refuse_pct && self.refuse_pct < self.critical_pct && self.critical_pct <= 99) {
            return Err(format!(
                "the percent lines must rise: warn < refuse < critical ≤ 99 (got warn {}, refuse {}, critical {})",
                self.warn_pct, self.refuse_pct, self.critical_pct
            ));
        }
        if self.warn_pct == 0 {
            return Err("the warn line must be above 0%".into());
        }
        if self.critical_free_bytes < 512 * MIB {
            return Err(format!(
                "the critical free line must be at least 512 MiB (got {})",
                human_bytes(self.critical_free_bytes)
            ));
        }
        if self.reserve_bytes < self.critical_free_bytes {
            return Err(format!(
                "the reserve must be at least the critical free line (reserve {}, critical {})",
                human_bytes(self.reserve_bytes),
                human_bytes(self.critical_free_bytes)
            ));
        }
        if self.warn_free_bytes < self.reserve_bytes {
            return Err(format!(
                "the warn free line must be at least the reserve (warn {}, reserve {})",
                human_bytes(self.warn_free_bytes),
                human_bytes(self.reserve_bytes)
            ));
        }
        Ok(())
    }

    pub fn to_json(&self) -> J {
        serde_json::to_value(self).expect("Lines serializes")
    }
}

// ── settings file ────────────────────────────────────────────────────

/// `~/.k2/disk-guard.json`. A missing field takes its default; a missing
/// `mode` means the platform default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<Mode>,
    #[serde(flatten)]
    pub lines: Lines,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_by: Option<String>,
}

pub fn settings_path() -> PathBuf {
    crate::paths::k2_home().join(SETTINGS_FILE)
}

/// The saved settings. Missing, unreadable, unparseable or invalid lines
/// all mean the defaults (logged; never an error: the guard must not stop
/// working because of its own file).
pub fn load_settings() -> Settings {
    let path = settings_path();
    let raw = match std::fs::read_to_string(&path) {
        Ok(r) => r,
        Err(_) => return Settings::default(),
    };
    match serde_json::from_str::<Settings>(&raw) {
        Ok(s) => match s.lines.validate() {
            Ok(()) => s,
            Err(e) => {
                crate::log_debug!("[disk_guard] WARN {} has invalid lines ({e}); using the defaults", path.display());
                Settings { lines: Lines::default(), ..s }
            }
        },
        Err(e) => {
            crate::log_debug!("[disk_guard] WARN {} unreadable ({e}); using the defaults", path.display());
            Settings::default()
        }
    }
}

/// Save atomically (temp file + rename), 0600. Validates first.
pub fn save_settings(s: &Settings) -> Result<(), String> {
    s.lines.validate()?;
    let path = settings_path();
    let dir = path.parent().ok_or("settings path has no parent")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    let tmp = dir.join(format!(".{SETTINGS_FILE}.{}.tmp", std::process::id()));
    let body = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, body).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    restrict_mode(&tmp);
    std::fs::rename(&tmp, &path).map_err(|e| format!("rename {} → {}: {e}", tmp.display(), path.display()))?;
    Ok(())
}

/// A settings change (`POST /cli/app/resources/settings`): every field is
/// optional; absent fields keep the saved value.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsPatch {
    pub mode: Option<Mode>,
    pub warn_pct: Option<u32>,
    pub warn_free_bytes: Option<u64>,
    pub refuse_pct: Option<u32>,
    pub reserve_bytes: Option<u64>,
    pub critical_pct: Option<u32>,
    pub critical_free_bytes: Option<u64>,
}

impl SettingsPatch {
    pub fn is_empty(&self) -> bool {
        *self == SettingsPatch::default()
    }

    /// The new settings (validated) from `base`.
    pub fn apply(&self, base: &Settings, by: &str, at: i64) -> Result<Settings, String> {
        let mut s = base.clone();
        if let Some(m) = self.mode {
            s.mode = Some(m);
        }
        let l = &mut s.lines;
        macro_rules! set {
            ($f:ident) => {
                if let Some(v) = self.$f {
                    l.$f = v;
                }
            };
        }
        set!(warn_pct);
        set!(warn_free_bytes);
        set!(refuse_pct);
        set!(reserve_bytes);
        set!(critical_pct);
        set!(critical_free_bytes);
        s.lines.validate()?;
        s.updated_at = Some(at);
        s.updated_by = Some(by.to_string());
        Ok(s)
    }
}

/// The guard as it stands: the mode, where it came from, and the lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Guard {
    pub mode: Mode,
    pub source: ModeSource,
    pub lines: Lines,
}

impl Guard {
    /// `{"mode", "modeSource", "lines"}` (the view's `guard`).
    pub fn to_json(&self) -> J {
        json!({ "mode": self.mode, "modeSource": self.source, "lines": self.lines.to_json() })
    }
}

/// The mode from its three sources (pure; the tests drive it). An env
/// value that isn't `on|warn|off` is ignored.
pub fn resolve_mode(env: Option<&str>, setting: Option<Mode>, default: Mode) -> (Mode, ModeSource) {
    if let Some(m) = env.and_then(Mode::parse) {
        return (m, ModeSource::Env);
    }
    if let Some(m) = setting {
        return (m, ModeSource::Setting);
    }
    (default, ModeSource::Default)
}

/// The effective guard now: `K2_DISK_GUARD` > the setting > the default.
pub fn guard() -> Guard {
    let s = load_settings();
    let env = std::env::var(ENV_MODE).ok();
    let (mode, source) = resolve_mode(env.as_deref(), s.mode, Mode::platform_default());
    Guard { mode, source, lines: s.lines }
}

// ── filesystems ──────────────────────────────────────────────────────

/// The raw `statvfs` numbers (what a fake disk supplies).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RawStat {
    pub blocks: u64,
    pub bfree: u64,
    pub bavail: u64,
    pub frsize: u64,
    pub files: u64,
    pub ffree: u64,
}

impl RawStat {
    /// A fake with `total` bytes and `free` available to normal users (no
    /// root reserve), 4 KiB blocks, plenty of inodes.
    pub fn sized(total: u64, free: u64) -> RawStat {
        let bs = 4096;
        RawStat {
            blocks: total / bs,
            bfree: free / bs,
            bavail: free / bs,
            frsize: bs,
            files: 1_000_000,
            ffree: 900_000,
        }
    }
}

/// One mounted filesystem, counted like `df`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fs {
    /// `major:minor` of the device.
    pub id: String,
    pub mount: Option<String>,
    pub fs_type: Option<String>,
    pub total_bytes: u64,
    pub used_bytes: u64,
    /// Bytes a normal user can still write (`f_bavail × f_frsize`).
    pub free_bytes: u64,
    /// used ÷ (used + free), one decimal.
    pub used_pct: f64,
    pub inodes_free_pct: Option<f64>,
}

impl Fs {
    pub fn from_raw(id: String, mount: Option<String>, fs_type: Option<String>, r: RawStat) -> Fs {
        let total = r.blocks.saturating_mul(r.frsize);
        let used = r.blocks.saturating_sub(r.bfree).saturating_mul(r.frsize);
        let free = r.bavail.saturating_mul(r.frsize);
        let denom = used.saturating_add(free);
        let used_pct = if denom == 0 { 0.0 } else { round1(used as f64 * 100.0 / denom as f64) };
        let inodes_free_pct = (r.files > 0).then(|| round1(r.ffree as f64 * 100.0 / r.files as f64));
        Fs { id, mount, fs_type, total_bytes: total, used_bytes: used, free_bytes: free, used_pct, inodes_free_pct }
    }

    /// The fs's own level under `lines` (no action's need counted).
    pub fn level(&self, lines: &Lines) -> Level {
        if self.used_pct >= lines.critical_pct as f64 || self.free_bytes < lines.critical_free_bytes {
            Level::Critical
        } else if self.used_pct >= lines.refuse_pct as f64 || self.free_bytes < lines.reserve_bytes {
            Level::Refuse
        } else if self.used_pct >= lines.warn_pct as f64
            || self.free_bytes < lines.warn_free_bytes
            || self.inodes_free_pct.is_some_and(|p| p < INODES_WARN_FREE_PCT)
        {
            Level::Warn
        } else {
            Level::Ok
        }
    }

    /// The level for an action that writes `need` bytes here: the fs's own
    /// level, raised to `refuse` when free − need would fall under the
    /// reserve.
    pub fn level_for(&self, lines: &Lines, need: u64) -> Level {
        let own = self.level(lines);
        if own < Level::Refuse && self.free_bytes.saturating_sub(need) < lines.reserve_bytes {
            Level::Refuse
        } else {
            own
        }
    }

    /// Printable mount (`/` when unknown is shown as the id).
    pub fn label(&self) -> String {
        self.mount.clone().unwrap_or_else(|| format!("device {}", self.id))
    }
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

/// One level per filesystem, worst last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Ok,
    Warn,
    Refuse,
    Critical,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Ok => "ok",
            Level::Warn => "warn",
            Level::Refuse => "refuse",
            Level::Critical => "critical",
        }
    }
}

/// The filesystem holding `path` (the nearest existing ancestor when the
/// path doesn't exist yet). `None` when the probe fails: unknown never
/// blocks.
pub fn probe(path: &Path) -> Option<Fs> {
    if let Some(f) = fake::lookup(path) {
        return Some(f.fs);
    }
    let existing = nearest_existing(path)?;
    real_probe(&existing)
}

fn nearest_existing(path: &Path) -> Option<PathBuf> {
    let mut p = Some(path);
    while let Some(cur) = p {
        if std::fs::symlink_metadata(cur).is_ok() {
            return Some(cur.to_path_buf());
        }
        p = cur.parent();
    }
    None
}

#[cfg(unix)]
fn real_probe(path: &Path) -> Option<Fs> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).ok()?;
    let dev = meta.dev();
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let raw = RawStat {
        blocks: st.f_blocks as u64,
        bfree: st.f_bfree as u64,
        bavail: st.f_bavail as u64,
        frsize: if st.f_frsize as u64 > 0 { st.f_frsize as u64 } else { st.f_bsize as u64 },
        files: st.f_files as u64,
        ffree: st.f_favail as u64,
    };
    let (maj, min) = dev_parts(dev as u64);
    let id = format!("{maj}:{min}");
    let (mount, fs_type) = mount_of(path, maj, min);
    Some(Fs::from_raw(id, mount, fs_type, raw))
}

#[cfg(not(unix))]
fn real_probe(_path: &Path) -> Option<Fs> {
    None
}

#[cfg(target_os = "linux")]
fn dev_parts(dev: u64) -> (u64, u64) {
    (libc::major(dev as libc::dev_t) as u64, libc::minor(dev as libc::dev_t) as u64)
}

#[cfg(all(unix, not(target_os = "linux")))]
fn dev_parts(dev: u64) -> (u64, u64) {
    // BSD / macOS encoding: 8-bit major, 24-bit minor.
    ((dev >> 24) & 0xff, dev & 0xff_ffff)
}

/// Mount point and fs type: `/proc/self/mountinfo` on Linux (the entry
/// with this device whose mount point is the longest prefix of `path`).
#[cfg(target_os = "linux")]
fn mount_of(path: &Path, maj: u64, min: u64) -> (Option<String>, Option<String>) {
    let Ok(info) = std::fs::read_to_string("/proc/self/mountinfo") else {
        return (None, None);
    };
    let canon = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let want = format!("{maj}:{min}");
    let mut best: Option<(usize, String, String)> = None;
    for line in info.lines() {
        if let Some((mp, ty, dev)) = parse_mountinfo_line(line) {
            if dev != want || !canon.starts_with(&mp) {
                continue;
            }
            if best.as_ref().is_none_or(|(len, _, _)| mp.len() > *len) {
                best = Some((mp.len(), mp, ty));
            }
        }
    }
    match best {
        Some((_, mp, ty)) => (Some(mp), Some(ty)),
        None => (None, None),
    }
}

/// `(mount point, fs type, "major:minor")` from one mountinfo line.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_mountinfo_line(line: &str) -> Option<(String, String, String)> {
    let (left, right) = line.split_once(" - ")?;
    let f: Vec<&str> = left.split(' ').collect();
    let dev = f.get(2)?.to_string();
    let mp = unescape_mount(f.get(4)?);
    let ty = right.split(' ').next()?.to_string();
    Some((mp, ty, dev))
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn unescape_mount(s: &str) -> String {
    s.replace("\\040", " ").replace("\\011", "\t").replace("\\012", "\n").replace("\\134", "\\")
}

#[cfg(all(unix, not(target_os = "linux")))]
fn mount_of(path: &Path, _maj: u64, _min: u64) -> (Option<String>, Option<String>) {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return (None, None);
    };
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return (None, None);
    }
    let cstr = |a: &[libc::c_char]| -> Option<String> {
        let bytes: Vec<u8> = a.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
        (!bytes.is_empty()).then(|| String::from_utf8_lossy(&bytes).into_owned())
    };
    (cstr(&st.f_mntonname), cstr(&st.f_fstypename))
}

/// Tracked paths grouped by filesystem: each fs once, with the labels of
/// the paths it holds (in the order given). Paths whose probe fails are
/// left out.
pub fn group(paths: &[(String, PathBuf)]) -> Vec<(Fs, Vec<String>)> {
    let mut out: Vec<(Fs, Vec<String>)> = Vec::new();
    for (label, p) in paths {
        let Some(fs) = probe(p) else { continue };
        match out.iter_mut().find(|(f, _)| f.id == fs.id) {
            Some((_, labels)) => {
                if !labels.contains(label) {
                    labels.push(label.clone());
                }
            }
            None => out.push((fs, vec![label.clone()])),
        }
    }
    out
}

// ── the check ────────────────────────────────────────────────────────

/// Why an action was warned or refused: the worst filesystem and the
/// numbers behind it.
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    /// The level the action met (`warn`, `refuse` or `critical`).
    pub level: Level,
    pub fs: Fs,
    pub need_bytes: u64,
    pub lines: Lines,
    /// Mode `warn` only: `on` would have refused this.
    pub would_refuse: bool,
}

impl Finding {
    /// The free bytes the action would leave.
    pub fn left_bytes(&self) -> u64 {
        self.fs.free_bytes.saturating_sub(self.need_bytes)
    }

    /// `{"mount","usedPct","freeBytes","needBytes","reserveBytes"}` (the
    /// refusal's `disk`).
    pub fn disk_json(&self) -> J {
        json!({
            "mount": self.fs.label(),
            "usedPct": self.fs.used_pct,
            "freeBytes": self.fs.free_bytes,
            "needBytes": self.need_bytes,
            "reserveBytes": self.lines.reserve_bytes,
        })
    }

    /// The first sentence(s) of a refusal or warning, in words, for an
    /// action named like "deploy" / "build" / "upload".
    pub fn sentence(&self, action: &str) -> String {
        let mount = self.fs.label();
        match self.level {
            Level::Critical => format!(
                "The disk is critical ({}% used, {} free on {mount}). New work waits for the server's owner in person.",
                self.fs.used_pct,
                human_gib(self.fs.free_bytes)
            ),
            Level::Refuse => {
                if self.fs.used_pct >= self.lines.refuse_pct as f64 {
                    format!(
                        "Disk {mount} is {}% full ({} free); new {action}s stop at {}%. This box keeps {} free for Postgres, mail and K2.",
                        self.fs.used_pct,
                        human_gib(self.fs.free_bytes),
                        self.lines.refuse_pct,
                        human_gib(self.lines.reserve_bytes)
                    )
                } else {
                    format!(
                        "This {action} would leave {} free on {mount}. This box keeps {} free for Postgres, mail and K2.",
                        human_gib(self.left_bytes()),
                        human_gib(self.lines.reserve_bytes)
                    )
                }
            }
            Level::Warn | Level::Ok => format!(
                "Disk {mount} is {}% full ({} free). New deploys stop at {}% or {} left.",
                self.fs.used_pct,
                human_gb(self.fs.free_bytes),
                self.lines.refuse_pct,
                human_gib(self.lines.reserve_bytes)
            ),
        }
    }
}

/// What a checkpoint does.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// Go ahead; add nothing.
    Pass,
    /// Go ahead with a `diskWarning`.
    Warn(Finding),
    /// `409 disk_low` (override rules apply).
    Refuse(Finding),
}

impl Verdict {
    pub fn is_refuse(&self) -> bool {
        matches!(self, Verdict::Refuse(_))
    }
}

/// The verdict for an action writing `need` bytes on the filesystems in
/// `disks` under `guard` (pure; [`check`] probes).
pub fn verdict(guard: &Guard, disks: &[Fs], need: u64) -> Verdict {
    if guard.mode == Mode::Off {
        return Verdict::Pass;
    }
    let mut worst: Option<(Level, &Fs)> = None;
    for fs in disks {
        let lvl = fs.level_for(&guard.lines, need);
        let worse = match worst {
            None => true,
            Some((wl, wf)) => lvl > wl || (lvl == wl && fs.free_bytes < wf.free_bytes),
        };
        if worse {
            worst = Some((lvl, fs));
        }
    }
    let Some((level, fs)) = worst else { return Verdict::Pass };
    let finding = |would_refuse| Finding { level, fs: fs.clone(), need_bytes: need, lines: guard.lines, would_refuse };
    match level {
        Level::Ok => Verdict::Pass,
        Level::Warn => Verdict::Warn(finding(false)),
        Level::Refuse | Level::Critical if guard.mode == Mode::Warn => Verdict::Warn(finding(true)),
        Level::Refuse | Level::Critical => Verdict::Refuse(finding(false)),
    }
}

/// The checkpoint: probe each path's filesystem (each fs once; unknown
/// ones don't count) and judge. A fake disk with its own mode decides the
/// mode for a check that touches it (tests only).
pub fn check(paths: &[PathBuf], need: u64) -> Verdict {
    let mut g = guard();
    let mut disks: Vec<Fs> = Vec::new();
    for p in paths {
        if let Some(f) = fake::lookup(p) {
            if let Some(m) = f.mode {
                g.mode = m;
            }
        }
        if let Some(fs) = probe(p) {
            if !disks.iter().any(|d| d.id == fs.id) {
                disks.push(fs);
            }
        }
    }
    verdict(&g, &disks, need)
}

/// Uploads (C6): only the reserve, only in mode `on`, no override and no
/// warning. `Err` carries the finding (the caller words it; an App guest
/// gets a message naming nobody and no numbers).
pub fn reserve_ok(path: &Path, need: u64) -> Result<(), Box<Finding>> {
    let mut g = guard();
    if let Some(f) = fake::lookup(path) {
        if let Some(m) = f.mode {
            g.mode = m;
        }
    }
    if g.mode != Mode::On {
        return Ok(());
    }
    let Some(fs) = probe(path) else { return Ok(()) };
    if fs.free_bytes.saturating_sub(need) < g.lines.reserve_bytes {
        let level = fs.level_for(&g.lines, need).max(Level::Refuse);
        return Err(Box::new(Finding { level, fs, need_bytes: need, lines: g.lines, would_refuse: false }));
    }
    Ok(())
}

/// The plain message an owner or agent upload gets when it would cross
/// the reserve (no override, PRD §6.2).
pub fn upload_refusal_text(f: &Finding) -> String {
    format!(
        "Not enough disk space: this upload would leave {} free on {}, and this server keeps {} free for Postgres, mail and K2. See k2 app resources.",
        human_gib(f.left_bytes()),
        f.fs.label(),
        human_gib(f.lines.reserve_bytes)
    )
}

/// The App guest's message: names nobody and no numbers (PRD §6.2).
pub const GUEST_NO_SPACE: &str = "This server is low on disk space. Try again later, or tell the App's owner.";

/// An override reason: trimmed, 1..=500 characters.
pub fn clean_reason(raw: &str) -> Result<String, String> {
    let r = raw.trim();
    if r.is_empty() {
        return Err("--disk-override needs a reason".into());
    }
    if r.chars().count() > MAX_REASON_CHARS {
        return Err(format!("the override reason is at most {MAX_REASON_CHARS} characters"));
    }
    Ok(r.chars().filter(|c| !c.is_control()).collect())
}

/// Re-arm (PRD §10.1): a line crossed earlier re-arms only once the fs is
/// `REARM_POINTS` below its percent **and** has `REARM_BYTES` more free
/// than its free line.
pub fn rearmed(fs: &Fs, line_pct: u32, line_free_bytes: u64) -> bool {
    fs.used_pct <= line_pct as f64 - REARM_POINTS && fs.free_bytes >= line_free_bytes.saturating_add(REARM_BYTES)
}

/// The percent and free-bytes pair behind a level's line (`ok` has none).
pub fn line_of(lines: &Lines, level: Level) -> Option<(u32, u64)> {
    match level {
        Level::Ok => None,
        Level::Warn => Some((lines.warn_pct, lines.warn_free_bytes)),
        Level::Refuse => Some((lines.refuse_pct, lines.reserve_bytes)),
        Level::Critical => Some((lines.critical_pct, lines.critical_free_bytes)),
    }
}

// ── the view's rows ──────────────────────────────────────────────────

/// Who maintains a row. Agents never get `workspaceId` (the view strips
/// it for agent callers along with `Consumer::folder`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Owner {
    /// `agent` | `server`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
    /// What `k2 msg` takes: `<handle>::<host>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    /// Person callers only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
}

impl Owner {
    pub fn server() -> Owner {
        Owner { kind: "server".into(), ..Owner::default() }
    }

    pub fn is_agent(&self) -> bool {
        self.kind == "agent"
    }

    /// The grouping key for `byOwner` (`None` for the server).
    pub fn key(&self) -> Option<String> {
        if !self.is_agent() {
            return None;
        }
        self.workspace_id.clone().or_else(|| self.address.clone()).or_else(|| self.handle.clone())
    }
}

/// One row of the view. `diskBytes` / `memBytes` are `null` when unknown
/// (never 0).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Consumer {
    /// `app` | `service` | `database` | `workspace` | `shared`.
    pub kind: String,
    pub id: String,
    /// Apps: the runtime path (`open` | `walled` | `process`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Services: on the wall or not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub walled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    pub owner: Owner,
    /// The fs id the row lives on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk: Option<String>,
    pub disk_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parts: BTreeMap<String, u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unused_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_bytes: Option<u64>,
    #[serde(default)]
    pub mem_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scanned_at: Option<i64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub partial: bool,
    /// The absolute folder (workspace rows). Person callers only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
}

impl Consumer {
    /// The same row as an agent may see it: no folder, no workspace id (a
    /// workspace row's `id` becomes the agent's handle).
    pub fn for_agent(&self) -> Consumer {
        let mut c = self.clone();
        c.folder = None;
        if c.kind == "workspace" {
            c.id = c.owner.handle.clone().or_else(|| c.owner.address.clone()).unwrap_or_else(|| "workspace".into());
        }
        c.owner.workspace_id = None;
        c
    }
}

/// When each scan last finished, and whether one is running.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanTimes {
    pub disk_at: Option<i64>,
    pub apps_at: Option<i64>,
    pub workspaces_at: Option<i64>,
    pub databases_at: Option<i64>,
    pub running: bool,
    /// The walled engine: `ok` | `missing` | `old` (no `disk-usage` verb).
    pub engine: Option<String>,
}

/// What the next Seed sweep will clear.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WillClear {
    pub seeds_bytes: u64,
    pub seeds: u64,
    pub next_sweep_at: Option<i64>,
}

/// What the scanner publishes (lane M fills it; the view and the guard's
/// hints read it).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub consumers: Vec<Consumer>,
    #[serde(default)]
    pub k2_will_clear: Option<WillClear>,
    #[serde(default)]
    pub scan: ScanTimes,
}

/// One owner's total (`byOwner`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnerTotal {
    pub owner: Owner,
    pub disk_bytes: u64,
}

impl Snapshot {
    /// Every agent's rows added up (Apps, services, databases, folder),
    /// biggest first. Unknown rows add nothing.
    pub fn by_owner(&self) -> Vec<OwnerTotal> {
        let mut map: BTreeMap<String, OwnerTotal> = BTreeMap::new();
        for c in &self.consumers {
            let Some(k) = c.owner.key() else { continue };
            let Some(b) = c.disk_bytes else { continue };
            let e = map.entry(k).or_insert_with(|| OwnerTotal { owner: c.owner.clone(), disk_bytes: 0 });
            e.disk_bytes = e.disk_bytes.saturating_add(b);
        }
        let mut v: Vec<OwnerTotal> = map.into_values().collect();
        v.sort_by(|a, b| b.disk_bytes.cmp(&a.disk_bytes).then_with(|| a.owner.handle.cmp(&b.owner.handle)));
        v
    }

    /// The oldest of the owner scans (what a hint says its names are as of).
    pub fn owners_scanned_at(&self) -> Option<i64> {
        [self.scan.apps_at, self.scan.workspaces_at, self.scan.databases_at].into_iter().flatten().min()
    }
}

fn snapshot_slot() -> &'static RwLock<Option<Arc<Snapshot>>> {
    static SLOT: OnceLock<RwLock<Option<Arc<Snapshot>>>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(None))
}

/// The scanner publishes its latest numbers here.
pub fn publish_snapshot(s: Snapshot) {
    *snapshot_slot().write().unwrap_or_else(|p| p.into_inner()) = Some(Arc::new(s));
}

/// The latest published numbers (`None` until a scanner has published).
pub fn latest_snapshot() -> Option<Arc<Snapshot>> {
    snapshot_slot().read().unwrap_or_else(|p| p.into_inner()).clone()
}

/// Forget the published numbers (tests).
pub fn clear_snapshot() {
    *snapshot_slot().write().unwrap_or_else(|p| p.into_inner()) = None;
}

// ── rescan control ───────────────────────────────────────────────────

/// A rescan is refused within this long of the last full sweep (PRD §5.2).
pub const RESCAN_MIN_GAP_SECS: i64 = 5 * 60;

#[derive(Debug, Default)]
struct ScanState {
    scanner: bool,
    running: bool,
    requested: bool,
    last_full_at: Option<i64>,
}

fn scan_ctl() -> &'static (Mutex<ScanState>, Condvar) {
    static CTL: OnceLock<(Mutex<ScanState>, Condvar)> = OnceLock::new();
    CTL.get_or_init(|| (Mutex::new(ScanState::default()), Condvar::new()))
}

/// The answer to `POST /cli/app/resources/rescan`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rescan {
    /// A sweep was asked for (the scanner picks it up).
    Started,
    /// One is running already.
    Running,
    /// The last full sweep finished `n` seconds ago (under 5 minutes).
    Skipped(i64),
    /// No scanner runs in this daemon (nothing will pick the request up).
    NoScanner,
}

impl Rescan {
    pub fn to_json(&self) -> J {
        match self {
            Rescan::Started => json!({ "ok": true, "started": true }),
            Rescan::Running => json!({ "ok": true, "running": true }),
            Rescan::Skipped(n) => json!({ "ok": true, "skipped": format!("scanned {n} s ago") }),
            Rescan::NoScanner => json!({ "ok": true, "started": false, "scanner": false }),
        }
    }
}

/// Ask for a fresh folder and App scan now. One at a time, and never
/// within 5 minutes of the last full sweep.
pub fn request_rescan(now: i64) -> Rescan {
    let (m, cv) = scan_ctl();
    let mut s = m.lock().unwrap_or_else(|p| p.into_inner());
    if !s.scanner {
        return Rescan::NoScanner;
    }
    if s.running || s.requested {
        return Rescan::Running;
    }
    if let Some(at) = s.last_full_at {
        let ago = now - at;
        if ago < RESCAN_MIN_GAP_SECS {
            return Rescan::Skipped(ago.max(0));
        }
    }
    s.requested = true;
    cv.notify_all();
    Rescan::Started
}

/// The scanner thread says it exists (once, at spawn).
pub fn register_scanner() {
    scan_ctl().0.lock().unwrap_or_else(|p| p.into_inner()).scanner = true;
}

/// The scanner waits up to `timeout` for a rescan request. `true` when one
/// came (it is taken: the flag clears).
pub fn scanner_wait(timeout: std::time::Duration) -> bool {
    let (m, cv) = scan_ctl();
    let s = m.lock().unwrap_or_else(|p| p.into_inner());
    let (mut s, _) = cv.wait_timeout_while(s, timeout, |s| !s.requested).unwrap_or_else(|p| p.into_inner());
    std::mem::take(&mut s.requested)
}

/// The scanner marks a sweep running / done (`done_at` = a full sweep's
/// end, for the 5-minute rule).
pub fn set_scan_running(running: bool, done_at: Option<i64>) {
    let mut s = scan_ctl().0.lock().unwrap_or_else(|p| p.into_inner());
    s.running = running;
    if let Some(at) = done_at {
        s.last_full_at = Some(at);
    }
}

/// Whether a sweep is running now.
pub fn scan_running() -> bool {
    scan_ctl().0.lock().unwrap_or_else(|p| p.into_inner()).running
}

/// Reset the scan control (tests).
pub fn reset_scan_control() {
    *scan_ctl().0.lock().unwrap_or_else(|p| p.into_inner()) = ScanState::default();
}

// ── the guard log ────────────────────────────────────────────────────

pub fn log_path() -> PathBuf {
    crate::paths::k2_home().join(LOG_FILE)
}

fn log_rotated_path() -> PathBuf {
    crate::paths::k2_home().join(format!("{LOG_FILE}.1"))
}

fn log_lock() -> &'static Mutex<()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
}

#[cfg(unix)]
fn restrict_mode(file: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Err(e) = std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600)) {
        crate::log_debug!("[disk_guard] chmod 0600 {}: {e}", file.display());
    }
}

#[cfg(not(unix))]
fn restrict_mode(_file: &Path) {}

/// Append one guard-log line: `{"at": <unix s>, "event": <event>, …fields}`.
/// Events: `refused`, `warned`, `override`, `held`, `notice`, `sweep`,
/// `settings`. Never fails the action (a full disk can't take the line).
pub fn log_event(event: &str, fields: J) {
    if let Err(e) = try_log_event(event, fields) {
        crate::log_debug!("[disk_guard] could not append to the guard log: {e}");
    }
}

/// [`log_event`] with the error surfaced (tests).
pub fn try_log_event(event: &str, fields: J) -> Result<(), String> {
    let _g = log_lock().lock().unwrap_or_else(|p| p.into_inner());
    let file = log_path();
    let dir = file.parent().ok_or("log path has no parent")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    if let Ok(meta) = std::fs::metadata(&file) {
        if meta.len() >= LOG_ROTATE_BYTES {
            std::fs::rename(&file, log_rotated_path()).map_err(|e| format!("rotate {}: {e}", file.display()))?;
        }
    }
    let mut line = json!({ "at": chrono::Utc::now().timestamp(), "event": event });
    if let J::Object(m) = fields {
        for (k, v) in m {
            if k != "at" && k != "event" {
                line[k] = v;
            }
        }
    }
    let created = !file.exists();
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file)
        .map_err(|e| format!("open {}: {e}", file.display()))?;
    if created {
        restrict_mode(&file);
    }
    let mut s = line.to_string();
    s.push('\n');
    f.write_all(s.as_bytes()).map_err(|e| format!("append {}: {e}", file.display()))
}

/// The last `n` guard-log lines, oldest first (spanning the rotated copy).
/// Torn lines are skipped.
pub fn recent(n: usize) -> Vec<J> {
    let _g = log_lock().lock().unwrap_or_else(|p| p.into_inner());
    let mut lines: Vec<String> = Vec::new();
    for file in [log_rotated_path(), log_path()] {
        if let Ok(raw) = std::fs::read_to_string(&file) {
            lines.extend(raw.lines().filter(|l| !l.trim().is_empty()).map(str::to_string));
        }
    }
    let start = lines.len().saturating_sub(n);
    lines[start..].iter().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

// ── box numbers ──────────────────────────────────────────────────────

/// `(MemTotal, MemAvailable)` in bytes. macOS: total from `hw.memsize`,
/// available `None` (no honest number).
pub fn box_mem() -> (Option<u64>, Option<u64>) {
    #[cfg(target_os = "linux")]
    {
        let Ok(m) = std::fs::read_to_string("/proc/meminfo") else { return (None, None) };
        (meminfo_kib(&m, "MemTotal:").map(|k| k * 1024), meminfo_kib(&m, "MemAvailable:").map(|k| k * 1024))
    }
    #[cfg(target_os = "macos")]
    {
        let mut v: u64 = 0;
        let mut len = std::mem::size_of::<u64>();
        let name = c"hw.memsize";
        let rc = unsafe {
            libc::sysctlbyname(name.as_ptr(), (&mut v as *mut u64).cast(), &mut len, std::ptr::null_mut(), 0)
        };
        ((rc == 0 && v > 0).then_some(v), None)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        (None, None)
    }
}

/// One `/proc/meminfo` field in KiB.
pub fn meminfo_kib(meminfo: &str, key: &str) -> Option<u64> {
    meminfo.lines().find_map(|l| l.strip_prefix(key)).and_then(|r| r.split_whitespace().next()).and_then(|n| n.parse().ok())
}

/// The 5-minute load average.
pub fn load5() -> Option<f64> {
    #[cfg(unix)]
    {
        let mut l = [0f64; 3];
        let n = unsafe { libc::getloadavg(l.as_mut_ptr(), 3) };
        (n >= 2).then(|| round1(l[1]))
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// Logical cores.
pub fn cores() -> Option<usize> {
    std::thread::available_parallelism().ok().map(|n| n.get())
}

/// The memory warning (PRD Q10): `true` when available RAM is under 10%
/// of the total. Unknown is never a warning.
pub fn memory_low(total: Option<u64>, available: Option<u64>) -> bool {
    match (total, available) {
        (Some(t), Some(a)) if t > 0 => a.saturating_mul(10) < t,
        _ => false,
    }
}

// ── words ────────────────────────────────────────────────────────────

/// Decimal size for people: `53 GB`, `1.4 GB`, `50 MB`, `812 KB`.
pub fn human_gb(b: u64) -> String {
    const K: f64 = 1000.0;
    let f = b as f64;
    if f >= K * K * K * K {
        format!("{:.1} TB", f / (K * K * K * K))
    } else if f >= 10.0 * K * K * K {
        format!("{:.0} GB", f / (K * K * K))
    } else if f >= K * K * K {
        format!("{:.1} GB", f / (K * K * K))
    } else if f >= K * K {
        format!("{:.0} MB", f / (K * K))
    } else if f >= K {
        format!("{:.0} KB", f / K)
    } else {
        format!("{b} B")
    }
}

/// Binary size for lines and reserves: `10 GiB`, `7.1 GiB`, `512 MiB`.
pub fn human_gib(b: u64) -> String {
    let f = b as f64;
    let g = f / GIB as f64;
    if f >= GIB as f64 {
        if (g - g.round()).abs() < 0.05 {
            format!("{:.0} GiB", g)
        } else {
            format!("{g:.1} GiB")
        }
    } else {
        format!("{:.0} MiB", f / MIB as f64)
    }
}

/// [`human_gib`] for any size (used in validation errors).
pub fn human_bytes(b: u64) -> String {
    human_gib(b)
}

/// Parse a size the CLI takes: `20G`, `512M`, `10GiB`, `2147483648`.
/// Binary units (G = GiB).
pub fn parse_size(s: &str) -> Option<u64> {
    let t = s.trim();
    let split = t.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(t.len());
    let (num, unit) = t.split_at(split);
    let n: f64 = num.parse().ok()?;
    if n < 0.0 {
        return None;
    }
    let mult = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1.0,
        "k" | "kb" | "kib" => 1024.0,
        "m" | "mb" | "mib" => MIB as f64,
        "g" | "gb" | "gib" => GIB as f64,
        "t" | "tb" | "tib" => GIB as f64 * 1024.0,
        _ => return None,
    };
    Some((n * mult).round() as u64)
}

// ── test seam ────────────────────────────────────────────────────────

/// Fake disks for tests. A fake applies to every path under its root
/// (longest root wins) and may carry a mode that wins for checks touching
/// it, so a guard test never changes what an unrelated test sees.
pub mod fake {
    use super::*;

    #[derive(Debug, Clone)]
    pub struct FakeDisk {
        pub fs: Fs,
        pub mode: Option<Mode>,
    }

    fn table() -> &'static Mutex<Vec<(u64, PathBuf, FakeDisk)>> {
        static T: OnceLock<Mutex<Vec<(u64, PathBuf, FakeDisk)>>> = OnceLock::new();
        T.get_or_init(|| Mutex::new(Vec::new()))
    }

    fn next_id() -> u64 {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    /// Removes its fake on drop.
    #[must_use = "the fake disk is removed when the guard drops"]
    pub struct FakeGuard {
        id: u64,
        root: PathBuf,
    }

    impl FakeGuard {
        /// Change this fake's numbers (keeps the root and mode).
        pub fn set(&self, raw: RawStat) {
            let mut t = table().lock().unwrap_or_else(|p| p.into_inner());
            if let Some((_, _, d)) = t.iter_mut().find(|(i, _, _)| *i == self.id) {
                d.fs = Fs::from_raw(d.fs.id.clone(), d.fs.mount.clone(), d.fs.fs_type.clone(), raw);
            }
        }

        /// Change this fake's mode.
        pub fn set_mode(&self, mode: Option<Mode>) {
            let mut t = table().lock().unwrap_or_else(|p| p.into_inner());
            if let Some((_, _, d)) = t.iter_mut().find(|(i, _, _)| *i == self.id) {
                d.mode = mode;
            }
        }

        pub fn root(&self) -> &Path {
            &self.root
        }
    }

    impl Drop for FakeGuard {
        fn drop(&mut self) {
            table().lock().unwrap_or_else(|p| p.into_inner()).retain(|(i, _, _)| *i != self.id);
        }
    }

    /// Fake the filesystem under `root` (`id` is its `major:minor`, the
    /// mount is shown as `mount`). Paths are compared as given and
    /// canonicalized, so `/var` vs `/private/var` both match.
    pub fn install(root: &Path, id: &str, mount: &str, raw: RawStat, mode: Option<Mode>) -> FakeGuard {
        let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        let id_n = next_id();
        let fs = Fs::from_raw(id.to_string(), Some(mount.to_string()), Some("fake".into()), raw);
        table().lock().unwrap_or_else(|p| p.into_inner()).push((id_n, root.clone(), FakeDisk { fs, mode }));
        FakeGuard { id: id_n, root }
    }

    /// The fake covering `path`, if any.
    pub fn lookup(path: &Path) -> Option<FakeDisk> {
        let t = table().lock().unwrap_or_else(|p| p.into_inner());
        if t.is_empty() {
            return None;
        }
        let canon = super::nearest_existing(path)
            .and_then(|e| {
                let c = std::fs::canonicalize(&e).ok()?;
                Some(match path.strip_prefix(&e) {
                    Ok(rest) if !rest.as_os_str().is_empty() => c.join(rest),
                    _ => c,
                })
            })
            .unwrap_or_else(|| path.to_path_buf());
        t.iter()
            .filter(|(_, root, _)| canon.starts_with(root) || path.starts_with(root))
            .max_by_key(|(_, root, _)| root.as_os_str().len())
            .map(|(_, _, d)| d.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env::{lock, unique_temp_path, EnvVar, TempHome};

    const G: u64 = GIB;

    fn fs(total: u64, free: u64) -> Fs {
        Fs::from_raw("8:1".into(), Some("/".into()), Some("ext4".into()), RawStat::sized(total, free))
    }

    fn on() -> Guard {
        Guard { mode: Mode::On, source: ModeSource::Setting, lines: Lines::default() }
    }

    #[test]
    fn used_pct_matches_df_with_a_root_reserve() {
        // df: 100 blocks, 40 free, 35 available to users (5 root-only):
        // used 60, Use% = 60 / (60 + 35) = 63.2%.
        let raw = RawStat { blocks: 100, bfree: 40, bavail: 35, frsize: 1024 * 1024, files: 200, ffree: 20 };
        let f = Fs::from_raw("8:1".into(), None, None, raw);
        assert_eq!(f.total_bytes, 100 * MIB);
        assert_eq!(f.used_bytes, 60 * MIB);
        assert_eq!(f.free_bytes, 35 * MIB);
        assert_eq!(f.used_pct, 63.2);
        assert_eq!(f.inodes_free_pct, Some(10.0));
        let empty = Fs::from_raw("0:0".into(), None, None, RawStat::default());
        assert_eq!(empty.used_pct, 0.0);
        assert_eq!(empty.inodes_free_pct, None);
    }

    #[test]
    fn each_level_from_each_line_whichever_comes_first() {
        let l = Lines::default();
        // Percent lines on a big disk (plenty of bytes free).
        assert_eq!(fs(1000 * G, 200 * G).level(&l), Level::Ok); // 80%
        assert_eq!(fs(1000 * G, 150 * G).level(&l), Level::Warn); // 85%
        assert_eq!(fs(1000 * G, 50 * G).level(&l), Level::Refuse); // 95%
        assert_eq!(fs(1000 * G, 20 * G).level(&l), Level::Critical); // 98%
        // Byte lines on a small disk (low percent).
        assert_eq!(fs(100 * G, 30 * G).level(&l), Level::Ok);
        assert_eq!(fs(100 * G, 19 * G).level(&l), Level::Warn); // 81%, under 20 GiB
        assert_eq!(fs(30 * G, 9 * G).level(&l), Level::Refuse); // 70%, under 10 GiB
        assert_eq!(fs(30 * G, G).level(&l), Level::Critical); // under 2 GiB
        // Few free inodes: warn, never more.
        let mut f = fs(1000 * G, 500 * G);
        f.inodes_free_pct = Some(4.9);
        assert_eq!(f.level(&l), Level::Warn);
        f.inodes_free_pct = Some(0.0);
        assert_eq!(f.level(&l), Level::Warn);
    }

    #[test]
    fn check_counts_the_need_against_the_reserve() {
        let g = on();
        // 50% used, 12 GiB free: ok alone, but a 3 GiB deploy leaves 9.
        let d = fs(24 * G, 12 * G);
        assert_eq!(d.level(&g.lines), Level::Warn, "12 GiB is under the 20 GiB warn line");
        match verdict(&g, std::slice::from_ref(&d), 3 * G) {
            Verdict::Refuse(f) => {
                assert_eq!(f.level, Level::Refuse);
                assert_eq!(f.left_bytes(), 9 * G);
                assert!(f.sentence("deploy").contains("would leave 9 GiB free on /"), "{}", f.sentence("deploy"));
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
        assert!(matches!(verdict(&g, std::slice::from_ref(&d), G), Verdict::Warn(_)));
        assert_eq!(verdict(&g, &[fs(1000 * G, 500 * G)], 3 * G), Verdict::Pass);
        // No disks known: unknown never blocks.
        assert_eq!(verdict(&g, &[], 100 * G), Verdict::Pass);
    }

    #[test]
    fn the_worst_disk_decides() {
        let g = on();
        let ok = fs(1000 * G, 500 * G);
        let mut bad = fs(1000 * G, 15 * G);
        bad.id = "8:2".into();
        bad.mount = Some("/var/lib/k2app".into());
        match verdict(&g, &[ok, bad], 0) {
            Verdict::Refuse(f) => {
                assert_eq!(f.fs.mount.as_deref(), Some("/var/lib/k2app"));
                assert_eq!(f.level, Level::Critical, "98.5% used is critical");
                assert!(f.sentence("deploy").starts_with("The disk is critical"), "{}", f.sentence("deploy"));
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn mode_warn_turns_refuse_into_warn_and_off_passes() {
        let d = fs(1000 * G, 30 * G); // 97%
        let mut g = on();
        g.mode = Mode::Warn;
        match verdict(&g, std::slice::from_ref(&d), 0) {
            Verdict::Warn(f) => {
                assert!(f.would_refuse);
                assert_eq!(f.level, Level::Refuse);
            }
            other => panic!("expected a warning, got {other:?}"),
        }
        g.mode = Mode::Off;
        assert_eq!(verdict(&g, &[d], 1000 * G), Verdict::Pass);
    }

    #[test]
    fn settings_defaults_validation_and_env_override() {
        assert_eq!(Lines::default().validate(), Ok(()));
        let bad = |f: &dyn Fn(&mut Lines)| {
            let mut l = Lines::default();
            f(&mut l);
            l.validate().expect_err("must be refused")
        };
        assert!(bad(&|l| l.warn_pct = 95).contains("warn < refuse < critical"));
        assert!(bad(&|l| l.critical_pct = 100).contains("≤ 99"));
        assert!(bad(&|l| l.refuse_pct = 99).contains("warn < refuse < critical"));
        assert!(bad(&|l| l.critical_free_bytes = 100 * MIB).contains("at least 512 MiB"));
        assert!(bad(&|l| l.reserve_bytes = G).contains("reserve must be at least the critical"));
        assert!(bad(&|l| l.warn_free_bytes = 5 * G).contains("warn free line must be at least the reserve"));

        assert_eq!(resolve_mode(Some("off"), Some(Mode::On), Mode::Warn), (Mode::Off, ModeSource::Env));
        assert_eq!(resolve_mode(Some(" WARN "), None, Mode::On), (Mode::Warn, ModeSource::Env));
        assert_eq!(resolve_mode(Some("bogus"), Some(Mode::On), Mode::Off), (Mode::On, ModeSource::Setting));
        assert_eq!(resolve_mode(None, None, Mode::Warn), (Mode::Warn, ModeSource::Default));
        assert_eq!(Mode::platform_default(), Mode::Off, "tests default to off (PRD §7.2)");
    }

    #[test]
    fn settings_file_round_trips_and_the_env_wins() {
        let _h = TempHome::new();
        let _env = EnvVar::remove(ENV_MODE);
        // Missing file: defaults, mode from the platform (off in tests).
        assert_eq!(load_settings(), Settings::default());
        assert_eq!(guard().mode, Mode::Off);
        assert_eq!(guard().source, ModeSource::Default);

        let patch: SettingsPatch =
            serde_json::from_value(json!({"mode": "on", "refusePct": 90, "reserveBytes": 12 * G})).unwrap();
        let s = patch.apply(&load_settings(), "owner", 1_792_345_320).unwrap();
        save_settings(&s).unwrap();
        let back = load_settings();
        assert_eq!(back.mode, Some(Mode::On));
        assert_eq!(back.lines.refuse_pct, 90);
        assert_eq!(back.lines.reserve_bytes, 12 * G);
        assert_eq!(back.lines.warn_pct, 85, "untouched fields keep their value");
        assert_eq!(back.updated_by.as_deref(), Some("owner"));
        let raw: J = serde_json::from_str(&std::fs::read_to_string(settings_path()).unwrap()).unwrap();
        assert_eq!(raw["refusePct"], 90);
        assert_eq!(raw["mode"], "on");
        assert_eq!(raw["updatedAt"], 1_792_345_320i64);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let m = std::fs::metadata(settings_path()).unwrap().permissions().mode() & 0o777;
            assert_eq!(m, 0o600);
        }
        assert_eq!(guard().mode, Mode::On);
        assert_eq!(guard().source, ModeSource::Setting);
        {
            let _off = EnvVar::set(ENV_MODE, "off");
            assert_eq!(guard().mode, Mode::Off);
            assert_eq!(guard().source, ModeSource::Env);
        }

        // A bad patch is refused and nothing is written.
        let bad: SettingsPatch = serde_json::from_value(json!({"warnPct": 96})).unwrap();
        let e = bad.apply(&load_settings(), "owner", 1).expect_err("warn above refuse");
        assert!(e.contains("warn < refuse"), "{e}");
        assert_eq!(load_settings().lines.warn_pct, 85);
        // Unknown fields are refused (a typo must not silently do nothing).
        assert!(serde_json::from_value::<SettingsPatch>(json!({"refuse": 90})).is_err());

        // A corrupt file means the defaults.
        std::fs::write(settings_path(), "{not json").unwrap();
        assert_eq!(load_settings(), Settings::default());
        // Invalid lines in the file: default lines, mode kept.
        std::fs::write(settings_path(), r#"{"mode":"warn","warnPct":99,"refusePct":95}"#).unwrap();
        let s = load_settings();
        assert_eq!(s.mode, Some(Mode::Warn));
        assert_eq!(s.lines, Lines::default());
    }

    #[test]
    fn reserve_ok_for_uploads_declared_total_per_chunk_and_unknown() {
        let _h = TempHome::new();
        let root = unique_temp_path("dg-upload");
        std::fs::create_dir_all(&root).unwrap();
        let disk = fake::install(&root, "7:7", "/fake", RawStat::sized(100 * G, 12 * G), Some(Mode::On));
        // 12 GiB free: a 1 GiB upload leaves 11 (ok), a 3 GiB one 9 (no).
        assert_eq!(reserve_ok(&root.join("sub/dir"), G), Ok(()));
        let f = reserve_ok(&root, 3 * G).expect_err("crosses the reserve");
        assert_eq!(f.left_bytes(), 9 * G);
        assert!(upload_refusal_text(&f).contains("would leave 9 GiB free on /fake"), "{}", upload_refusal_text(&f));
        assert!(!GUEST_NO_SPACE.chars().any(|c| c.is_ascii_digit()), "the guest text has no numbers");
        // Per chunk: 10.01 GiB free, a 16 MiB chunk would cross.
        disk.set(RawStat::sized(100 * G, 10 * G + 10 * MIB));
        assert!(reserve_ok(&root, 16 * MIB).is_err());
        assert_eq!(reserve_ok(&root, 4 * MIB), Ok(()));
        // Mode warn and off: uploads only keep the reserve in mode on.
        disk.set_mode(Some(Mode::Warn));
        assert_eq!(reserve_ok(&root, 50 * G), Ok(()));
        disk.set_mode(Some(Mode::Off));
        assert_eq!(reserve_ok(&root, 50 * G), Ok(()));
        drop(disk);
        // Without the fake, tests default to off: nothing is checked.
        assert_eq!(reserve_ok(&root, u64::MAX), Ok(()));
        // An unknown fs never blocks, even in mode on.
        let _on = EnvVar::set(ENV_MODE, "on");
        assert_eq!(reserve_ok(Path::new(""), u64::MAX), Ok(()));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn check_probes_each_fs_once_and_uses_the_fakes_mode() {
        let _h = TempHome::new();
        let root = unique_temp_path("dg-check");
        let a = root.join("a");
        let b = root.join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let _fa = fake::install(&a, "9:1", "/a", RawStat::sized(100 * G, 50 * G), Some(Mode::On));
        let fb = fake::install(&b, "9:2", "/b", RawStat::sized(100 * G, 11 * G), Some(Mode::On));
        assert_eq!(check(&[a.clone()], 0), Verdict::Pass);
        match check(&[a.clone(), b.clone(), b.join("x")], 2 * G) {
            Verdict::Refuse(f) => assert_eq!(f.fs.mount.as_deref(), Some("/b")),
            other => panic!("expected a refusal, got {other:?}"),
        }
        fb.set_mode(Some(Mode::Warn));
        assert!(matches!(check(&[b.clone()], 2 * G), Verdict::Warn(ref f) if f.would_refuse));
        let groups = group(&[("k2Home".into(), a.clone()), ("workspaces".into(), b.clone()), ("mail".into(), b.join("m"))]);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[1].1, vec!["workspaces".to_string(), "mail".to_string()]);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_real_probe_reads_this_machines_disk() {
        let tmp = std::env::temp_dir();
        let f = probe(&tmp).expect("statvfs on the temp dir");
        assert!(f.total_bytes > 0);
        assert!(f.used_pct >= 0.0 && f.used_pct <= 100.0);
        assert!(f.id.contains(':'));
        assert!(f.mount.is_some(), "the mount point is found: {f:?}");
        // A path that doesn't exist yet resolves to its nearest ancestor.
        let g = probe(&tmp.join("k2-no-such-dir/deeper")).expect("ancestor probe");
        assert_eq!(g.id, f.id);
    }

    #[test]
    fn rearm_needs_five_points_and_five_gib() {
        let l = Lines::default();
        let (pct, free) = line_of(&l, Level::Warn).unwrap();
        assert!(!rearmed(&fs(1000 * G, 160 * G), pct, free), "84% is not 5 points below 85");
        assert!(rearmed(&fs(1000 * G, 210 * G), pct, free), "79%, 210 GiB free");
        // A small disk: 75% used but only 22 GiB free (needs 25).
        assert!(!rearmed(&fs(88 * G, 22 * G), pct, free));
        assert_eq!(line_of(&l, Level::Ok), None);
    }

    #[test]
    fn guard_log_appends_rotates_and_reads_recent() {
        let _h = TempHome::new();
        try_log_event("refused", json!({"action": "deploy", "app": "helper", "by": "agent:jobs-2"})).unwrap();
        try_log_event("settings", json!({"by": "owner", "event": "spoofed"})).unwrap();
        let r = recent(RECENT);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0]["event"], "refused");
        assert_eq!(r[0]["app"], "helper");
        assert_eq!(r[1]["event"], "settings", "a field can't overwrite the event");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(log_path()).unwrap().permissions().mode() & 0o777, 0o600);
        }
        // Rotation: a full live file moves to .1 and recent spans both.
        let big = "x".repeat(LOG_ROTATE_BYTES as usize);
        std::fs::OpenOptions::new().append(true).open(log_path()).unwrap().write_all(big.as_bytes()).unwrap();
        try_log_event("sweep", json!({"seeds": 3})).unwrap();
        assert!(log_rotated_path().exists());
        let r = recent(RECENT);
        assert_eq!(r.last().unwrap()["event"], "sweep");
        assert_eq!(r[0]["event"], "refused", "the rotated copy is read first");
    }

    #[test]
    fn by_owner_adds_up_agent_rows_and_skips_unknown() {
        let agent = |h: &str| Owner {
            kind: "agent".into(),
            agent: Some(h.to_uppercase()),
            handle: Some(h.into()),
            address: Some(format!("{h}::box.k2.dev")),
            workspace_id: Some(format!("ws-{h}")),
        };
        let row = |kind: &str, o: Owner, b: Option<u64>| Consumer {
            kind: kind.into(),
            id: "x".into(),
            owner: o,
            disk_bytes: b,
            ..Consumer::default()
        };
        let s = Snapshot {
            consumers: vec![
                row("app", agent("nsi"), Some(6)),
                row("database", agent("nsi"), Some(58)),
                row("workspace", agent("nsi"), Some(38)),
                row("workspace", agent("plans"), Some(14)),
                row("service", agent("plans"), None),
                row("shared", Owner::server(), Some(1000)),
            ],
            ..Snapshot::default()
        };
        let by = s.by_owner();
        assert_eq!(by.len(), 2);
        assert_eq!(by[0].owner.handle.as_deref(), Some("nsi"));
        assert_eq!(by[0].disk_bytes, 102);
        assert_eq!(by[1].disk_bytes, 14);
        // The agent copy of a row has no folder and no workspace id; null
        // stays null (unknown is never 0).
        let mut w = row("workspace", agent("nsi"), None);
        w.folder = Some("/home/k2/AI Projects/nsi".into());
        let a = serde_json::to_value(w.for_agent()).unwrap();
        assert!(a.get("folder").is_none());
        assert!(a["owner"].get("workspaceId").is_none());
        assert!(a["diskBytes"].is_null());
        assert_eq!(a["owner"]["address"], "nsi::box.k2.dev");
        assert_eq!(a["id"], "nsi", "a workspace row's id is the handle for agents");
    }

    #[test]
    fn rescan_is_one_at_a_time_and_rate_limited() {
        let _l = lock();
        reset_scan_control();
        assert_eq!(request_rescan(1000), Rescan::NoScanner);
        register_scanner();
        assert_eq!(request_rescan(1000), Rescan::Started);
        assert_eq!(request_rescan(1001), Rescan::Running, "already asked");
        assert!(scanner_wait(std::time::Duration::from_millis(10)), "the scanner takes the request");
        assert!(!scanner_wait(std::time::Duration::from_millis(10)), "taken once");
        set_scan_running(true, None);
        assert_eq!(request_rescan(1002), Rescan::Running);
        set_scan_running(false, Some(1100));
        assert_eq!(request_rescan(1200), Rescan::Skipped(100));
        assert_eq!(request_rescan(1100 + RESCAN_MIN_GAP_SECS), Rescan::Started);
        assert_eq!(Rescan::Skipped(42).to_json()["skipped"], "scanned 42 s ago");
        reset_scan_control();
    }

    #[test]
    fn words_and_sizes() {
        assert_eq!(human_gb(53_000_000_000), "53 GB");
        assert_eq!(human_gb(1_400_000_000), "1.4 GB");
        assert_eq!(human_gb(50_000_000), "50 MB");
        assert_eq!(human_gib(10 * G), "10 GiB");
        assert_eq!(human_gib(7 * G + G / 10), "7.1 GiB");
        assert_eq!(human_gib(512 * MIB), "512 MiB");
        assert_eq!(parse_size("20G"), Some(20 * G));
        assert_eq!(parse_size("512M"), Some(512 * MIB));
        assert_eq!(parse_size("10GiB"), Some(10 * G));
        assert_eq!(parse_size("2147483648"), Some(2 * G));
        assert_eq!(parse_size("1.5G"), Some(3 * G / 2));
        assert_eq!(parse_size("ten"), None);
        assert_eq!(parse_size("5X"), None);
        assert_eq!(clean_reason("  hotfix for the login bug ").unwrap(), "hotfix for the login bug");
        assert!(clean_reason("   ").is_err());
        assert!(clean_reason(&"x".repeat(501)).is_err());
        assert!(memory_low(Some(100), Some(9)));
        assert!(!memory_low(Some(100), Some(10)));
        assert!(!memory_low(Some(100), None), "unknown is never a warning");
        assert_eq!(meminfo_kib("MemTotal:  32818588 kB\nMemAvailable: 10485760 kB\n", "MemAvailable:"), Some(10_485_760));
        let line = "36 35 98:0 /mnt1 /mnt\\040two rw,noatime master:1 - ext4 /dev/root rw,errors=continue";
        assert_eq!(parse_mountinfo_line(line), Some(("/mnt two".into(), "ext4".into(), "98:0".into())));
    }
}
