//! Hosted-mail backup, slice **B1** (`prd-hostmail-backup-v1` rev 2, §6.1,
//! §6.2, §6.3, §7, §8.4): the churn observer, the `plan` dry-run, the
//! per-box mode choice (`set`), status and one doctor check.
//!
//! **B1 never stops Stalwart and never copies data.** A mode can be chosen,
//! but nothing runs until B2 (local frozen views) / B3 (offsite). Status says
//! so in plain words.
//!
//! **Churn observer** (one daemon thread, Linux only, skipped while hostmail
//! is not installed or an enable/upgrade is running): ~10 min after boot,
//! then an hourly tick that observes once a day. An observation lists the
//! Stalwart store tree ([`STALWART_DATA_DIR`], no symlinks followed) and
//! records name + size + mtime of the immutable RocksDB files (`*.sst`,
//! `*.blob`), the total of every other (mutable) file, and free/total space
//! of the store's filesystem. `LOG*` (RocksDB's info log) and `LOCK` are
//! excluded. Diffing the previous inventory gives bytes created / deleted
//! per day (compaction + blob GC churn) and growth. 30 daily SUMMARIES are
//! kept in `mail_server.backup_state_json`; only the latest full inventory
//! is kept (`mail_backup_inventory`) to diff against. Migration 0136.
//!
//! **Permissions.** The observer reads the store as the daemon user, with
//! no helper and no sudo: the helper creates `/var/lib/stalwart` 0755
//! (`helper::forced_mode`), RocksDB creates `data/` 0755 and its files 0644
//! under systemd's default `UMask=0022` (the unit and drop-in set no
//! `UMask=`), and listing + `stat` need only `r-x` on the directories. A box
//! where that is not true reports `permission_denied` with the exact error
//! in `lastError` (status + doctor) — never a sudo fallback.
//!
//! **Gate.** `GET /cli/mail/backup` (status) is a plain read, like
//! `/cli/mail/status`. `GET /cli/mail/backup/plan` and `POST
//! /cli/mail/backup/set` are mail-manage surfaces (the box's it-email agent,
//! M5 toggle) plus owner/admin. The offsite destination and restore are not
//! in B1; `offsite-only`/`both` are refused until the owner can set a
//! destination (B3).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use k2_core::log_debug;
use serde::{Deserialize, Serialize};

use crate::cli_response::CliResponse;
use crate::mail::doctor::{DoctorCheck, ST_INFO, ST_WARN};
use crate::mail::supervisor::{
    self, mail_supported, STALWART_CONFIG_DIR, STALWART_DATA_DIR,
};

// ── Constants ───────────────────────────────────────────────────────────

const GIB: u64 = 1 << 30;

/// Where B2's frozen views will live (PRD §8.2). B1 never creates it; it
/// only compares filesystems (hard links need the same `st_dev`).
pub const VIEW_ROOT: &str = "/var/lib/stalwart.k2-views";
/// Daily summaries kept (PRD §6.1).
pub const KEEP_SUMMARIES: usize = 30;
/// One observation per day …
pub const OBSERVE_EVERY_SECS: i64 = 86_400;
/// … due an hour early so an hourly tick never slips a whole day.
pub const OBSERVE_SLACK_SECS: i64 = 3_600;
/// A failed observation is retried after this long (the next tick).
pub const RETRY_AFTER_SECS: i64 = 3_600;
/// A summary needs at least this long between inventories.
pub const MIN_INTERVAL_SECS: i64 = 3_600;
pub const FIRST_TICK_AFTER_SECS: u64 = 10 * 60;
pub const TICK_EVERY_SECS: u64 = 3_600;
/// Churn counts as measured after this many daily summaries (B0: ≥ 3).
pub const MEASURED_MIN_DAYS: usize = 3;
/// Unmeasured churn bound: this share of the store per day (PRD §5).
pub const UNMEASURED_CHURN_FRACTION: f64 = 0.10;
/// restic local cache cap counted into offsite temp space (PRD §5).
pub const RESTIC_CACHE_BYTES: u64 = 2 * GIB;
/// `--bandwidth-mbps` default (nothing is measured in B1).
pub const DEFAULT_BANDWIDTH_MBPS: f64 = 50.0;
/// Free-space floor: `max(5 GiB, 10% of the filesystem)` (PRD §7).
pub const FLOOR_MIN_BYTES: u64 = 5 * GIB;
pub const FLOOR_FS_FRACTION: f64 = 0.10;
/// Downtime estimate: per hard link (conservative; a link is ~µs) …
pub const LINK_SECS_PER_FILE: f64 = 0.002;
/// … plus stop + start + health check.
pub const STOP_START_OVERHEAD_SECS: u64 = 15;
/// First upload compresses/dedupes to somewhere in this share of raw.
pub const FIRST_UPLOAD_LOW_FRACTION: f64 = 0.6;

pub const DEFAULT_WINDOW: &str = "03:30-04:30";
pub const DEFAULT_KEEP_DAILY: u32 = 7;
pub const DEFAULT_KEEP_WEEKLY: u32 = 4;
pub const DEFAULT_OFFSITE_KEEP_DAILY: u32 = 7;
pub const DEFAULT_OFFSITE_KEEP_WEEKLY: u32 = 4;
pub const DEFAULT_OFFSITE_KEEP_MONTHLY: u32 = 6;
/// Changes kept in `history` on the config row.
const HISTORY_KEEP: usize = 20;
const MAX_REASON_CHARS: usize = 500;

/// The modes, in the order `plan` prints them.
pub const MODES: [&str; 4] = ["local", "offsite-only", "both", "none"];

pub const UNCONFIGURED_NOTE: &str = "Backup not chosen — the box's it-email agent should run \
     `k2 hostmail backup plan` and agree a mode with the client";
pub const NOT_RUNNING_NOTE: &str = "mode chosen; backups start in a later release (B2: local \
     frozen views, B3: offsite) — nothing runs yet and Stalwart is never stopped for it";
pub const OFFSITE_UNSET_HINT: &str = "offsite destination not set — the owner sets it in a later \
     release (B3). Choose local or none for now.";

const CONFIG_COL: &str = "backup_config_json";
const STATE_COL: &str = "backup_state_json";

// ── Inventory ───────────────────────────────────────────────────────────

/// How a store file is treated (PRD F5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileClass {
    /// `*.sst` / `*.blob`: written once, only ever deleted. Hard-linked.
    Immutable,
    /// MANIFEST-*, WAL `*.log`, OPTIONS-*, CURRENT, IDENTITY, anything else:
    /// copied for real in a view.
    Mutable,
    /// `LOG*` (RocksDB info log, grows unbounded) and `LOCK`: never backed up.
    Excluded,
}

pub fn classify(name: &str) -> FileClass {
    if name.starts_with("LOG") || name == "LOCK" {
        FileClass::Excluded
    } else if name.ends_with(".sst") || name.ends_with(".blob") {
        FileClass::Immutable
    } else {
        FileClass::Mutable
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileRec {
    pub bytes: u64,
    pub mtime: i64,
}

/// One listing of the store tree.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Inventory {
    /// Immutable files only, keyed by path relative to the store root.
    pub files: BTreeMap<String, FileRec>,
    pub immutable_bytes: u64,
    pub mutable_bytes: u64,
    pub mutable_files: u64,
    pub excluded_bytes: u64,
    /// Symlinks (never followed, never counted).
    pub skipped_links: u64,
}

impl Inventory {
    pub fn store_bytes(&self) -> u64 {
        self.immutable_bytes + self.mutable_bytes
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanError {
    Missing(String),
    PermissionDenied(String),
    Other(String),
}

impl ScanError {
    pub fn status(&self) -> &'static str {
        match self {
            ScanError::Missing(_) => "missing",
            ScanError::PermissionDenied(_) => "permission_denied",
            ScanError::Other(_) => "error",
        }
    }
    pub fn message(&self) -> String {
        match self {
            ScanError::Missing(m) | ScanError::Other(m) => m.clone(),
            ScanError::PermissionDenied(m) => format!(
                "{m} — the daemon user cannot list the Stalwart store (expected 0755 dirs); \
                 the observer never uses sudo. Check the mode of /var/lib/stalwart and \
                 /var/lib/stalwart/data"
            ),
        }
    }
}

const MAX_DEPTH: usize = 8;
const MAX_ENTRIES: u64 = 5_000_000;

fn io_scan_err(what: &str, path: &Path, e: std::io::Error) -> ScanError {
    let msg = format!("{what} {}: {e}", path.display());
    match e.kind() {
        std::io::ErrorKind::NotFound => ScanError::Missing(msg),
        std::io::ErrorKind::PermissionDenied => ScanError::PermissionDenied(msg),
        _ => ScanError::Other(msg),
    }
}

fn mtime_secs(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Walk `root` without following symlinks: `visit(rel_path, file_name,
/// metadata)` for every regular file.
fn walk(
    root: &Path,
    visit: &mut dyn FnMut(&str, &str, &std::fs::Metadata),
) -> Result<u64, ScanError> {
    let meta = std::fs::symlink_metadata(root).map_err(|e| io_scan_err("stat", root, e))?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(ScanError::Other(format!(
            "{} is not a real directory",
            root.display()
        )));
    }
    let mut links = 0u64;
    let mut seen = 0u64;
    let mut stack: Vec<(PathBuf, String, usize)> = vec![(root.to_path_buf(), String::new(), 0)];
    while let Some((dir, rel, depth)) = stack.pop() {
        let rd = std::fs::read_dir(&dir).map_err(|e| io_scan_err("list", &dir, e))?;
        for ent in rd {
            let ent = ent.map_err(|e| io_scan_err("list", &dir, e))?;
            seen += 1;
            if seen > MAX_ENTRIES {
                return Err(ScanError::Other(format!(
                    "{} has more than {MAX_ENTRIES} entries — refusing to list it",
                    root.display()
                )));
            }
            let name = ent.file_name().to_string_lossy().into_owned();
            let path = ent.path();
            let child_rel = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            let m = match std::fs::symlink_metadata(&path) {
                Ok(m) => m,
                // Deleted between readdir and stat (compaction): not there.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(io_scan_err("stat", &path, e)),
            };
            let ft = m.file_type();
            if ft.is_symlink() {
                links += 1;
            } else if ft.is_dir() {
                if depth + 1 >= MAX_DEPTH {
                    return Err(ScanError::Other(format!(
                        "{} is nested deeper than {MAX_DEPTH} levels",
                        path.display()
                    )));
                }
                stack.push((path, child_rel, depth + 1));
            } else if ft.is_file() {
                visit(&child_rel, &name, &m);
            }
        }
    }
    Ok(links)
}

/// List the store tree under `root` (production: [`STALWART_DATA_DIR`]).
pub fn scan_store(root: &Path) -> Result<Inventory, ScanError> {
    let mut inv = Inventory::default();
    let links = walk(root, &mut |rel, name, m| match classify(name) {
        FileClass::Immutable => {
            inv.immutable_bytes += m.len();
            inv.files.insert(rel.to_string(), FileRec { bytes: m.len(), mtime: mtime_secs(m) });
        }
        FileClass::Mutable => {
            inv.mutable_bytes += m.len();
            inv.mutable_files += 1;
        }
        FileClass::Excluded => inv.excluded_bytes += m.len(),
    })?;
    inv.skipped_links = links;
    Ok(inv)
}

/// Total bytes of regular files under `root` (no symlinks followed).
pub fn tree_bytes(root: &Path) -> Result<u64, ScanError> {
    let mut total = 0u64;
    walk(root, &mut |_, _, m| total += m.len())?;
    Ok(total)
}

/// Bytes created / deleted between two inventories. A file whose size or
/// mtime changed (never expected for an immutable file) counts as one
/// delete plus one create.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Diff {
    pub created_bytes: u64,
    pub deleted_bytes: u64,
    pub created_files: u64,
    pub deleted_files: u64,
}

pub fn diff(prev: &BTreeMap<String, FileRec>, cur: &BTreeMap<String, FileRec>) -> Diff {
    let mut d = Diff::default();
    for (name, p) in prev {
        match cur.get(name) {
            Some(c) if c == p => {}
            Some(_) | None => {
                d.deleted_bytes += p.bytes;
                d.deleted_files += 1;
            }
        }
    }
    for (name, c) in cur {
        match prev.get(name) {
            Some(p) if p == c => {}
            Some(_) | None => {
                d.created_bytes += c.bytes;
                d.created_files += 1;
            }
        }
    }
    d
}

// ── Filesystem ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FsStats {
    pub dev: u64,
    /// Bytes available to unprivileged users (`f_bavail`).
    pub free: u64,
    pub total: u64,
}

#[cfg(unix)]
pub fn fs_stats(path: &Path) -> Result<FsStats, String> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    let dev = std::fs::metadata(path)
        .map_err(|e| format!("stat {}: {e}", path.display()))?
        .dev();
    let c = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| "statvfs: bad path".to_string())?;
    // SAFETY: zeroed statvfs is a valid out-param; `c` is NUL-terminated
    // and outlives the call.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(c.as_ptr(), &mut st) };
    if rc != 0 {
        return Err(format!(
            "statvfs {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        ));
    }
    let frsize = st.f_frsize as u64;
    Ok(FsStats {
        dev,
        free: (st.f_bavail as u64).saturating_mul(frsize),
        total: (st.f_blocks as u64).saturating_mul(frsize),
    })
}

#[cfg(not(unix))]
pub fn fs_stats(path: &Path) -> Result<FsStats, String> {
    Err(format!("statvfs {}: not supported on this platform", path.display()))
}

/// Would a view under `view_root` share the store's filesystem? The view
/// root itself when it exists, else its parent (B1 never creates it).
/// `None` when neither can be stat'ed.
pub fn same_fs_for(store_dev: u64, view_root: &Path) -> Option<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let probe = if view_root.exists() { Some(view_root) } else { view_root.parent() };
        probe
            .and_then(|p| std::fs::metadata(p).ok())
            .map(|m| m.dev() == store_dev)
    }
    #[cfg(not(unix))]
    {
        let _ = (store_dev, view_root);
        None
    }
}

/// `max(5 GiB, 10% of the filesystem)` (PRD §7).
pub fn floor_bytes(fs_total: u64) -> u64 {
    FLOOR_MIN_BYTES.max((fs_total as f64 * FLOOR_FS_FRACTION) as u64)
}

/// `ok` / `tight` / `refused` for needing `need` more bytes on a
/// filesystem with `free` available (PRD §7: refused when the run would
/// leave less than the floor; tight when it leaves less than twice it).
pub fn verdict(free: u64, need: u64, floor: u64) -> &'static str {
    let after = free as i128 - need as i128;
    if after < floor as i128 {
        "refused"
    } else if after < 2 * floor as i128 {
        "tight"
    } else {
        "ok"
    }
}

// ── Observer state ──────────────────────────────────────────────────────

/// Store totals at the latest successful observation.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Totals {
    pub at: i64,
    pub store_bytes: u64,
    pub immutable_bytes: u64,
    pub mutable_bytes: u64,
    pub immutable_files: u64,
    pub mutable_files: u64,
    pub excluded_bytes: u64,
    /// Symlinks seen in the store tree (never followed).
    pub skipped_links: u64,
    pub fs_dev: u64,
    pub fs_free: u64,
    pub fs_total: u64,
    pub same_fs: Option<bool>,
}

/// One day's churn (between two consecutive inventories).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DaySummary {
    pub at: i64,
    pub interval_secs: i64,
    pub created_bytes: u64,
    pub deleted_bytes: u64,
    pub created_files: u64,
    pub deleted_files: u64,
    pub store_bytes: u64,
    pub growth_bytes: i64,
    pub fs_free: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ObserverState {
    /// `ok` | `missing` | `permission_denied` | `error`; `None` = never ran.
    pub status: Option<String>,
    pub last_run: Option<i64>,
    pub last_success: Option<i64>,
    pub last_error: Option<String>,
    pub latest: Option<Totals>,
    pub days: Vec<DaySummary>,
}

/// Is an observation due? Once a day after the last success; a failure
/// is retried on the next hourly tick.
pub fn due(state: &ObserverState, now: i64) -> bool {
    let day_ok = state
        .last_success
        .is_none_or(|s| now - s >= OBSERVE_EVERY_SECS - OBSERVE_SLACK_SECS);
    let retry_ok = state.last_run.is_none_or(|r| now - r >= RETRY_AFTER_SECS - 60);
    day_ok && retry_ok
}

/// One observation over `root`. Returns the new state and, when the scan
/// succeeded, the new immutable inventory to persist. `prev_files` must be
/// the inventory `state.latest` describes (the caller drops it when the
/// row count does not match). Never panics; errors land in the state.
pub fn observe(
    root: &Path,
    now: i64,
    state: &ObserverState,
    prev_files: &BTreeMap<String, FileRec>,
    fs_of: &dyn Fn(&Path) -> Result<FsStats, String>,
    same_fs_of: &dyn Fn(u64) -> Option<bool>,
) -> (ObserverState, Option<BTreeMap<String, FileRec>>) {
    let mut next = state.clone();
    next.last_run = Some(now);
    let inv = match scan_store(root) {
        Ok(inv) => inv,
        Err(e) => {
            next.status = Some(e.status().to_string());
            next.last_error = Some(e.message());
            return (next, None);
        }
    };
    let fs = match fs_of(root) {
        Ok(fs) => fs,
        Err(e) => {
            next.status = Some("error".into());
            next.last_error = Some(format!("free space: {e}"));
            return (next, None);
        }
    };
    let totals = Totals {
        at: now,
        store_bytes: inv.store_bytes(),
        immutable_bytes: inv.immutable_bytes,
        mutable_bytes: inv.mutable_bytes,
        immutable_files: inv.files.len() as u64,
        mutable_files: inv.mutable_files,
        excluded_bytes: inv.excluded_bytes,
        skipped_links: inv.skipped_links,
        fs_dev: fs.dev,
        fs_free: fs.free,
        fs_total: fs.total,
        same_fs: same_fs_of(fs.dev),
    };
    if let Some(prev) = state.latest.as_ref() {
        let interval = now - prev.at;
        if interval >= MIN_INTERVAL_SECS {
            let d = diff(prev_files, &inv.files);
            next.days.push(DaySummary {
                at: now,
                interval_secs: interval,
                created_bytes: d.created_bytes,
                deleted_bytes: d.deleted_bytes,
                created_files: d.created_files,
                deleted_files: d.deleted_files,
                store_bytes: totals.store_bytes,
                growth_bytes: totals.store_bytes as i64 - prev.store_bytes as i64,
                fs_free: fs.free,
            });
            let excess = next.days.len().saturating_sub(KEEP_SUMMARIES);
            next.days.drain(..excess);
        }
    }
    next.status = Some("ok".into());
    next.last_success = Some(now);
    next.last_error = None;
    next.latest = Some(totals);
    (next, Some(inv.files))
}

/// Churn rates from the daily summaries.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Churn {
    pub days: usize,
    pub measured: bool,
    pub deleted_per_day: u64,
    pub created_per_day: u64,
    pub max_deleted_per_day: u64,
    pub growth_per_day: i64,
}

pub fn churn_of(days: &[DaySummary]) -> Churn {
    let interval: i64 = days.iter().map(|d| d.interval_secs.max(1)).sum();
    if days.is_empty() || interval <= 0 {
        return Churn::default();
    }
    let per_day = |sum: f64| (sum * 86_400.0 / interval as f64).round();
    let deleted: f64 = days.iter().map(|d| d.deleted_bytes as f64).sum();
    let created: f64 = days.iter().map(|d| d.created_bytes as f64).sum();
    let growth: f64 = days.iter().map(|d| d.growth_bytes as f64).sum();
    let max_deleted = days
        .iter()
        .map(|d| (d.deleted_bytes as f64 * 86_400.0 / d.interval_secs.max(1) as f64).round() as u64)
        .max()
        .unwrap_or(0);
    Churn {
        days: days.len(),
        measured: days.len() >= MEASURED_MIN_DAYS,
        deleted_per_day: per_day(deleted) as u64,
        created_per_day: per_day(created) as u64,
        max_deleted_per_day: max_deleted,
        growth_per_day: per_day(growth) as i64,
    }
}

// ── Config ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HistoryEntry {
    pub at: i64,
    pub by: String,
    pub workspace: Option<String>,
    pub from: String,
    pub to: String,
}

/// `mail_server.backup_config_json`. Absent = unconfigured.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BackupConfig {
    pub mode: String,
    pub reason: Option<String>,
    pub keep_daily: u32,
    pub keep_weekly: u32,
    pub offsite_keep_daily: u32,
    pub offsite_keep_weekly: u32,
    pub offsite_keep_monthly: u32,
    pub window: String,
    pub max_local_gb: Option<f64>,
    pub configured_by: String,
    pub configured_workspace: Option<String>,
    pub configured_at: i64,
    pub plan_hash: String,
    pub history: Vec<HistoryEntry>,
}

/// Retention + window, resolved: explicit value, else the current
/// config's, else the PRD default.
#[derive(Debug, Clone, PartialEq)]
pub struct Retention {
    pub keep_daily: u32,
    pub keep_weekly: u32,
    pub offsite_keep_daily: u32,
    pub offsite_keep_weekly: u32,
    pub offsite_keep_monthly: u32,
    pub window: String,
    pub max_local_gb: Option<f64>,
}

impl Default for Retention {
    fn default() -> Self {
        Self {
            keep_daily: DEFAULT_KEEP_DAILY,
            keep_weekly: DEFAULT_KEEP_WEEKLY,
            offsite_keep_daily: DEFAULT_OFFSITE_KEEP_DAILY,
            offsite_keep_weekly: DEFAULT_OFFSITE_KEEP_WEEKLY,
            offsite_keep_monthly: DEFAULT_OFFSITE_KEEP_MONTHLY,
            window: DEFAULT_WINDOW.to_string(),
            max_local_gb: None,
        }
    }
}

/// Optional retention inputs (`plan` query / `set` body).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RetentionArgs {
    pub keep_daily: Option<u32>,
    pub keep_weekly: Option<u32>,
    pub offsite_keep_daily: Option<u32>,
    pub offsite_keep_weekly: Option<u32>,
    pub offsite_keep_monthly: Option<u32>,
    pub window: Option<String>,
    pub max_local_gb: Option<f64>,
}

pub fn resolve_retention(args: &RetentionArgs, current: Option<&BackupConfig>) -> Retention {
    let base = match current {
        Some(c) if c.keep_daily > 0 => Retention {
            keep_daily: c.keep_daily,
            keep_weekly: c.keep_weekly,
            offsite_keep_daily: c.offsite_keep_daily,
            offsite_keep_weekly: c.offsite_keep_weekly,
            offsite_keep_monthly: c.offsite_keep_monthly,
            window: if c.window.is_empty() { DEFAULT_WINDOW.to_string() } else { c.window.clone() },
            max_local_gb: c.max_local_gb,
        },
        _ => Retention::default(),
    };
    Retention {
        keep_daily: args.keep_daily.unwrap_or(base.keep_daily),
        keep_weekly: args.keep_weekly.unwrap_or(base.keep_weekly),
        offsite_keep_daily: args.offsite_keep_daily.unwrap_or(base.offsite_keep_daily),
        offsite_keep_weekly: args.offsite_keep_weekly.unwrap_or(base.offsite_keep_weekly),
        offsite_keep_monthly: args.offsite_keep_monthly.unwrap_or(base.offsite_keep_monthly),
        window: args.window.clone().unwrap_or(base.window),
        max_local_gb: args.max_local_gb.or(base.max_local_gb),
    }
}

fn check_retention(r: &Retention) -> Result<(), String> {
    if !(1..=366).contains(&r.keep_daily) {
        return Err("--keep-daily must be 1..366 (at least one daily view)".into());
    }
    if r.keep_weekly > 104 {
        return Err("--keep-weekly must be 0..104".into());
    }
    if r.offsite_keep_daily > 366 || r.offsite_keep_weekly > 104 || r.offsite_keep_monthly > 120 {
        return Err(
            "--offsite-keep-daily must be 0..366, --offsite-keep-weekly 0..104, \
             --offsite-keep-monthly 0..120"
                .into(),
        );
    }
    if let Some(gb) = r.max_local_gb {
        if !(gb.is_finite() && gb > 0.0 && gb <= 1_000_000.0) {
            return Err("--max-local-gb must be a positive number of GiB".into());
        }
    }
    parse_window(&r.window)?;
    Ok(())
}

/// `HH:MM-HH:MM` (box time; may wrap midnight; never empty).
pub fn parse_window(w: &str) -> Result<((u32, u32), (u32, u32)), String> {
    let bad = || format!("--window must be HH:MM-HH:MM (box time), got '{w}'");
    let (a, b) = w.split_once('-').ok_or_else(bad)?;
    let hm = |s: &str| -> Option<(u32, u32)> {
        let (h, m) = s.split_once(':')?;
        if h.len() != 2 || m.len() != 2 {
            return None;
        }
        let (h, m) = (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?);
        (h < 24 && m < 60).then_some((h, m))
    };
    let (start, end) = (hm(a).ok_or_else(bad)?, hm(b).ok_or_else(bad)?);
    if start == end {
        return Err(format!("--window start and end are the same ('{w}')"));
    }
    Ok((start, end))
}

// ── Plan (§6.2) ─────────────────────────────────────────────────────────

/// What `plan` computes from. Gathered live by [`live_inputs`]; built by
/// hand in tests.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlanInputs {
    pub immutable_bytes: u64,
    pub mutable_bytes: u64,
    pub immutable_files: u64,
    /// `/etc/stalwart`.
    pub config_bytes: u64,
    /// k2.db (+ WAL), mail-secrets.json, certs/, settings.json, tunnel.json.
    pub k2_half_bytes: u64,
    pub fs_free: u64,
    pub fs_total: u64,
    pub same_fs: Option<bool>,
    pub churn: Churn,
    pub notes: Vec<String>,
}

impl PlanInputs {
    pub fn store_bytes(&self) -> u64 {
        self.immutable_bytes + self.mutable_bytes
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlanOpts {
    pub bandwidth_mbps: f64,
    pub retention: Retention,
}

fn ceil_div(a: u64, b: u64) -> u64 {
    if b == 0 {
        0
    } else {
        a.div_ceil(b)
    }
}

/// Nearest GiB: the plan hash is over rounded numbers, so a few MB of
/// new mail between `plan` and `set` does not invalidate it.
fn gib_round(b: u64) -> u64 {
    (b + GIB / 2) / GIB
}

fn upload_secs(bytes: u64, mbps: f64) -> u64 {
    if mbps <= 0.0 {
        return 0;
    }
    (bytes as f64 * 8.0 / (mbps * 1_000_000.0)).ceil() as u64
}

/// Short sha256 over the canonical hash input.
fn short_hash(v: &serde_json::Value) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(v.to_string().as_bytes());
    digest.iter().take(6).map(|b| format!("{b:02x}")).collect()
}

/// The dry-run: per mode, local disk, offsite sizes and times, temp space,
/// a downtime estimate and the free-space verdict. Changes nothing.
pub fn build_plan(inp: &PlanInputs, opts: &PlanOpts) -> serde_json::Value {
    let r = &opts.retention;
    let s = inp.store_bytes();
    let m = inp.mutable_bytes;
    let c = inp.config_bytes;
    let k = inp.k2_half_bytes;
    let fixed = m + c + k;
    let raw = s + c + k;
    let measured = inp.churn.measured;
    let unmeasured_day = (s as f64 * UNMEASURED_CHURN_FRACTION) as u64;
    let (d_avg, d_max, created) = if measured {
        (inp.churn.deleted_per_day, inp.churn.max_deleted_per_day, inp.churn.created_per_day)
    } else {
        (unmeasured_day, unmeasured_day, unmeasured_day)
    };
    // Churn pinned while a view exists for `days` (PRD §5): measured = the
    // worst observed day's rate × the duration; unmeasured = the PRD's
    // conservative bound (10% of the store), or more for multi-day runs.
    let pinned = |days: f64| -> u64 {
        if measured {
            (d_max as f64 * days).ceil() as u64
        } else {
            unmeasured_day.max((unmeasured_day as f64 * days).ceil() as u64)
        }
    };
    let same_fs = inp.same_fs == Some(true);
    let floor = floor_bytes(inp.fs_total);
    let free = inp.fs_free;
    let mbps = opts.bandwidth_mbps;

    let views = (r.keep_daily + r.keep_weekly) as u64;
    let span_days = r.keep_daily.max(7 * r.keep_weekly) as u64;
    let cap = r.max_local_gb.map(|gb| (gb * GIB as f64) as u64);

    // Downtime: stop + links + mutable copy + K2 half + start/health.
    let copy = crate::mail::upgrade::COPY_BYTES_PER_SEC;
    let downtime = if same_fs {
        STOP_START_OVERHEAD_SECS
            + ceil_div(fixed, copy)
            + (inp.immutable_files as f64 * LINK_SECS_PER_FILE).ceil() as u64
    } else {
        STOP_START_OVERHEAD_SECS + ceil_div(raw, copy)
    };
    let downtime_label = if same_fs {
        "conservative estimate, not measured (B0): stop + hard links + copy of the mutable files, \
         /etc/stalwart and the K2 half + start and health check"
    } else {
        "conservative estimate, not measured (B0): the view root is not on the store's \
         filesystem, so a view is a FULL copy while Stalwart is stopped"
    };

    // local
    let local_first = if same_fs { fixed } else { raw };
    let local_uncapped = if same_fs {
        views * fixed + d_avg * span_days
    } else {
        views * raw
    };
    let local_steady = cap.map_or(local_uncapped, |c| c.min(local_uncapped));
    let cap_too_small = cap.is_some_and(|c| c < local_first);
    let local_peak = local_first.max(local_steady);
    let mut local_verdict = verdict(free, local_peak, floor);
    if cap_too_small {
        local_verdict = "refused";
    }

    // offsite
    let first_upload_high = raw;
    let first_upload_low = (raw as f64 * FIRST_UPLOAD_LOW_FRACTION) as u64;
    let first_secs = upload_secs(first_upload_high, mbps);
    let nightly_upload = created + fixed;
    let nightly_secs = upload_secs(nightly_upload, mbps);
    let view_cost = if same_fs { fixed } else { raw };
    let first_temp = view_cost + pinned(first_secs as f64 / 86_400.0) + RESTIC_CACHE_BYTES;
    let nightly_temp = view_cost + pinned(nightly_secs as f64 / 86_400.0) + RESTIC_CACHE_BYTES;
    let offsite_peak = first_temp.max(nightly_temp);
    let offsite_verdict = verdict(free, offsite_peak, floor);
    let both_peak = local_peak + RESTIC_CACHE_BYTES + pinned(first_secs as f64 / 86_400.0);
    let mut both_verdict = verdict(free, both_peak, floor);
    if cap_too_small {
        both_verdict = "refused";
    }

    let offsite_block = serde_json::json!({
        "firstUploadBytes": {"low": first_upload_low, "high": first_upload_high},
        "firstUploadSecs": first_secs,
        "nightlyUploadBytes": nightly_upload,
        "nightlyUploadSecs": nightly_secs,
        "firstTempBytes": first_temp,
        "nightlyTempBytes": nightly_temp,
        "label": "first upload ≈ the whole store (restic compression/dedup not measured: shown \
                  as a range); nightly ≈ new immutable files + the mutable files + K2 half",
    });
    let unavailable = serde_json::json!(OFFSITE_UNSET_HINT);
    let free_after = |need: u64| free as i128 - need as i128;
    let mut modes = serde_json::Map::new();
    modes.insert(
        "local".into(),
        serde_json::json!({
            "available": true,
            "firstNightLocalBytes": local_first,
            "steadyLocalBytes": local_steady,
            "steadyUncappedBytes": local_uncapped,
            "maxLocalBytes": cap,
            "capTooSmall": cap_too_small,
            "views": views,
            "retainedDays": span_days,
            "peakBytes": local_peak,
            "freeAfterBytes": free_after(local_peak) as i64,
            "downtimeSecs": downtime,
            "downtimeLabel": downtime_label,
            "verdict": local_verdict,
            "protects": "logical damage (bad import/upgrade, deletion, corruption that writes new \
                         files). NOT disk failure or bit rot: a view shares inodes with the live store",
        }),
    );
    modes.insert(
        "offsite-only".into(),
        serde_json::json!({
            "available": false,
            "unavailableReason": unavailable,
            "firstNightLocalBytes": 0,
            "steadyLocalBytes": 0,
            "offsite": offsite_block,
            "peakBytes": offsite_peak,
            "freeAfterBytes": free_after(offsite_peak) as i64,
            "downtimeSecs": downtime,
            "downtimeLabel": downtime_label,
            "verdict": offsite_verdict,
            "protects": "disk and box loss + logical damage (temporary local space only)",
        }),
    );
    modes.insert(
        "both".into(),
        serde_json::json!({
            "available": false,
            "unavailableReason": unavailable,
            "firstNightLocalBytes": local_first,
            "steadyLocalBytes": local_steady,
            "offsite": offsite_block,
            "peakBytes": both_peak,
            "freeAfterBytes": free_after(both_peak) as i64,
            "downtimeSecs": downtime,
            "downtimeLabel": downtime_label,
            "verdict": both_verdict,
            "protects": "all of the above",
        }),
    );
    modes.insert(
        "none".into(),
        serde_json::json!({
            "available": true,
            "firstNightLocalBytes": 0,
            "steadyLocalBytes": 0,
            "peakBytes": 0,
            "freeAfterBytes": free as i64,
            "downtimeSecs": 0,
            "verdict": "ok",
            "protects": "handled elsewhere — --reason is required and recorded",
        }),
    );

    let churn_label = if measured {
        format!("measured over {} days", inp.churn.days)
    } else if inp.churn.days > 0 {
        format!(
            "unmeasured — conservative bound ({}% of the store per day); {} of {} days observed",
            (UNMEASURED_CHURN_FRACTION * 100.0) as u32,
            inp.churn.days,
            MEASURED_MIN_DAYS
        )
    } else {
        format!(
            "unmeasured — conservative bound ({}% of the store per day)",
            (UNMEASURED_CHURN_FRACTION * 100.0) as u32
        )
    };

    // The hash: rounded numbers + verdicts + modes + retention. Times (they
    // depend on --bandwidth-mbps) are left out on purpose.
    let mut hash_modes = Vec::new();
    for name in MODES {
        let mv = &modes[name];
        hash_modes.push(serde_json::json!([
            name,
            mv["available"],
            mv["verdict"],
            gib_round(mv["peakBytes"].as_u64().unwrap_or(0)),
            gib_round(mv["steadyLocalBytes"].as_u64().unwrap_or(0)),
            gib_round(mv["firstNightLocalBytes"].as_u64().unwrap_or(0)),
        ]));
    }
    let hash_input = serde_json::json!({
        "v": 1,
        "modes": hash_modes,
        "store": gib_round(s),
        "free": gib_round(free),
        "floor": gib_round(floor),
        "sameFs": inp.same_fs,
        "measured": measured,
        "days": inp.churn.days,
        "keepDaily": r.keep_daily,
        "keepWeekly": r.keep_weekly,
        "maxLocalGb": r.max_local_gb,
    });
    let plan_hash = short_hash(&hash_input);

    let mut notes = vec![
        "B1 runs no backups: nothing stops Stalwart and nothing is copied until a later release."
            .to_string(),
        "A local view shares inodes with the live store: it protects against logical damage, \
         not disk failure or bit rot. Offsite covers that."
            .to_string(),
    ];
    match inp.same_fs {
        Some(true) => {}
        Some(false) => notes.push(format!(
            "{VIEW_ROOT} would be on another filesystem than the store: each view would be a full \
             copy (no hard links). The numbers above assume that."
        )),
        None => notes.push(format!(
            "could not stat {VIEW_ROOT} or its parent: assuming another filesystem (full copies)"
        )),
    }
    if cap_too_small {
        notes.push("one view alone is larger than --max-local-gb: local and both are refused".into());
    }
    notes.extend(inp.notes.iter().cloned());

    serde_json::json!({
        "ok": true,
        "planHash": plan_hash,
        "store": {
            "path": STALWART_DATA_DIR,
            "bytes": s,
            "immutableBytes": inp.immutable_bytes,
            "mutableBytes": m,
            "immutableFiles": inp.immutable_files,
            "configBytes": c,
            "k2HalfBytes": k,
            "fsTotal": inp.fs_total,
            "fsFree": free,
            "floorBytes": floor,
            "sameFs": inp.same_fs,
            "viewRoot": VIEW_ROOT,
        },
        "churn": {
            "quality": if measured { "measured" } else { "unmeasured" },
            "label": churn_label,
            "days": inp.churn.days,
            "deletedPerDay": d_avg,
            "maxDeletedPerDay": d_max,
            "createdPerDay": created,
            "growthPerDay": inp.churn.growth_per_day,
        },
        "bandwidthMbps": mbps,
        "retention": retention_json(r),
        "window": r.window,
        "modes": serde_json::Value::Object(modes),
        "notes": notes,
    })
}

fn retention_json(r: &Retention) -> serde_json::Value {
    serde_json::json!({
        "local": {"keepDaily": r.keep_daily, "keepWeekly": r.keep_weekly, "maxLocalGb": r.max_local_gb},
        "offsite": {
            "keepDaily": r.offsite_keep_daily,
            "keepWeekly": r.offsite_keep_weekly,
            "keepMonthly": r.offsite_keep_monthly,
        },
    })
}

// ── Persistence ─────────────────────────────────────────────────────────

fn installed() -> bool {
    supervisor::row_field("status").is_some()
}

/// The stored mode. `Ok(None)` = unconfigured; `Err` = unreadable JSON
/// (status/doctor say so, `set` overwrites it).
pub fn load_config() -> Result<Option<BackupConfig>, String> {
    match supervisor::row_field(CONFIG_COL) {
        None => Ok(None),
        Some(raw) => serde_json::from_str(&raw)
            .map(Some)
            .map_err(|e| format!("mail_server.{CONFIG_COL} is unreadable: {e}")),
    }
}

fn update_col(col: &str, value: &str) -> Result<(), String> {
    // `col` is a module constant, never caller input.
    let db = k2_core::db::shared();
    let conn = db.lock();
    let n = conn
        .execute(
            &format!("UPDATE mail_server SET {col} = ?1, updated_at = ?2 WHERE id = 1"),
            rusqlite::params![value, supervisor::now_secs()],
        )
        .map_err(|e| format!("store {col}: {e}"))?;
    if n != 1 {
        return Err("the email server is not installed (no mail_server row)".into());
    }
    Ok(())
}

pub fn load_state() -> ObserverState {
    match supervisor::row_field(STATE_COL) {
        None => ObserverState::default(),
        Some(raw) => serde_json::from_str(&raw).unwrap_or_else(|e| ObserverState {
            status: Some("error".into()),
            last_error: Some(format!("mail_server.{STATE_COL} was unreadable ({e}); restarted")),
            ..ObserverState::default()
        }),
    }
}

fn save_state(state: &ObserverState) -> Result<(), String> {
    let raw = serde_json::to_string(state).map_err(|e| format!("encode state: {e}"))?;
    update_col(STATE_COL, &raw)
}

fn load_inventory() -> Result<BTreeMap<String, FileRec>, String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut stmt = conn
        .prepare("SELECT rel_path, bytes, mtime FROM mail_backup_inventory")
        .map_err(|e| format!("read inventory: {e}"))?;
    let rows = stmt
        .query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))
        })
        .map_err(|e| format!("read inventory: {e}"))?;
    let mut out = BTreeMap::new();
    for row in rows {
        let (p, b, m) = row.map_err(|e| format!("read inventory: {e}"))?;
        out.insert(p, FileRec { bytes: b.max(0) as u64, mtime: m });
    }
    Ok(out)
}

fn save_inventory(files: &BTreeMap<String, FileRec>) -> Result<(), String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| format!("write inventory: {e}"))?;
    tx.execute("DELETE FROM mail_backup_inventory", [])
        .map_err(|e| format!("write inventory: {e}"))?;
    {
        let mut ins = tx
            .prepare("INSERT INTO mail_backup_inventory (rel_path, bytes, mtime) VALUES (?1, ?2, ?3)")
            .map_err(|e| format!("write inventory: {e}"))?;
        for (p, f) in files {
            ins.execute(rusqlite::params![p, f.bytes as i64, f.mtime])
                .map_err(|e| format!("write inventory: {e}"))?;
        }
    }
    tx.commit().map_err(|e| format!("write inventory: {e}"))
}

// ── Observer thread ─────────────────────────────────────────────────────

fn busy() -> bool {
    use std::sync::atomic::Ordering;
    supervisor::enable_running().load(Ordering::SeqCst)
        || supervisor::upgrade_running().load(Ordering::SeqCst)
}

/// One tick against the shared DB and `root`: observe when due, persist.
/// `Ok(false)` = skipped (not installed / busy / not due).
pub fn tick_at(
    root: &Path,
    now: i64,
    fs_of: &dyn Fn(&Path) -> Result<FsStats, String>,
    same_fs_of: &dyn Fn(u64) -> Option<bool>,
) -> Result<bool, String> {
    if !installed() || busy() {
        return Ok(false);
    }
    let state = load_state();
    if !due(&state, now) {
        return Ok(false);
    }
    // The stored inventory must be the one `state.latest` describes; a
    // mismatch (crash between the two writes) means "no previous day".
    let mut state_in = state.clone();
    let prev = match (state.latest.as_ref(), load_inventory()) {
        (Some(t), Ok(files)) if files.len() as u64 == t.immutable_files => files,
        (Some(_), Ok(_)) => {
            state_in.latest = None;
            BTreeMap::new()
        }
        (Some(_), Err(e)) => {
            log_debug!("[mail/backup] {e} — observing without a previous day");
            state_in.latest = None;
            BTreeMap::new()
        }
        (None, _) => BTreeMap::new(),
    };
    let (mut next, files) = observe(root, now, &state_in, &prev, fs_of, same_fs_of);
    if let Some(files) = files {
        if let Err(e) = save_inventory(&files) {
            next.status = Some("error".into());
            next.last_error = Some(e);
            next.latest = None;
        }
    }
    save_state(&next)?;
    Ok(true)
}

fn live_same_fs(dev: u64) -> Option<bool> {
    same_fs_for(dev, Path::new(VIEW_ROOT))
}

/// Start the churn observer (one detached thread; a panic is contained
/// per tick). No-op in tests and on non-Linux daemons.
pub fn spawn_observer() {
    if cfg!(test) || !mail_supported() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("k2-mail-backup-observer".into())
        .spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(FIRST_TICK_AFTER_SECS));
            loop {
                let r = std::panic::catch_unwind(|| {
                    tick_at(
                        Path::new(STALWART_DATA_DIR),
                        supervisor::now_secs(),
                        &fs_stats,
                        &live_same_fs,
                    )
                });
                match r {
                    Ok(Err(e)) => log_debug!("[mail/backup] observer: {e}"),
                    Err(_) => log_debug!("[mail/backup] observer tick panicked — next tick"),
                    Ok(Ok(_)) => {}
                }
                std::thread::sleep(std::time::Duration::from_secs(TICK_EVERY_SECS));
            }
        });
}

// ── Status + doctor (§8.4) ──────────────────────────────────────────────

/// `backup` for `/cli/mail/status` and `GET /cli/mail/backup`.
pub fn status_json_from(
    installed: bool,
    config: &Result<Option<BackupConfig>, String>,
    state: &ObserverState,
) -> serde_json::Value {
    let churn = churn_of(&state.days);
    let latest = state.latest.as_ref();
    let observer = serde_json::json!({
        "status": state.status.clone().unwrap_or_else(|| "never".into()),
        "lastRun": state.last_run,
        "lastSuccess": state.last_success,
        "lastError": state.last_error,
        "days": churn.days,
        "churnPerDay": {
            "measured": churn.measured,
            "createdBytes": churn.created_per_day,
            "deletedBytes": churn.deleted_per_day,
            "maxDeletedBytes": churn.max_deleted_per_day,
        },
        "growthPerDay": churn.growth_per_day,
        "storeBytes": latest.map(|t| t.store_bytes),
        "mutableBytes": latest.map(|t| t.mutable_bytes),
        "immutableFiles": latest.map(|t| t.immutable_files),
        "fsFree": latest.map(|t| t.fs_free),
        "fsTotal": latest.map(|t| t.fs_total),
        "sameFs": latest.and_then(|t| t.same_fs),
    });
    let mut out = serde_json::json!({
        "installed": installed,
        "mode": "unconfigured",
        "configuredBy": null,
        "configuredWorkspace": null,
        "configuredAt": null,
        "reason": null,
        "window": null,
        "retention": null,
        "offsiteDestination": null,
        "running": false,
        "observer": observer,
        "note": UNCONFIGURED_NOTE,
    });
    match config {
        Err(e) => {
            out["configError"] = serde_json::json!(e);
        }
        Ok(None) => {}
        Ok(Some(c)) => {
            let r = resolve_retention(&RetentionArgs::default(), Some(c));
            out["mode"] = serde_json::json!(c.mode);
            out["configuredBy"] = serde_json::json!(c.configured_by);
            out["configuredWorkspace"] = serde_json::json!(c.configured_workspace);
            out["configuredAt"] = serde_json::json!(c.configured_at);
            out["reason"] = serde_json::json!(c.reason);
            out["window"] = serde_json::json!(r.window);
            out["retention"] = retention_json(&r);
            out["note"] = if c.mode == "none" {
                serde_json::json!(format!(
                    "backups off by choice: {}",
                    c.reason.as_deref().unwrap_or("(no reason recorded)")
                ))
            } else {
                serde_json::json!(NOT_RUNNING_NOTE)
            };
        }
    }
    out
}

pub fn status_json(installed: bool) -> serde_json::Value {
    if !installed {
        return status_json_from(false, &Ok(None), &ObserverState::default());
    }
    status_json_from(true, &load_config(), &load_state())
}

/// Doctor `mail-backup` (warn only, never gates direct send).
pub fn doctor_check_from(
    config: &Result<Option<BackupConfig>, String>,
    state: &ObserverState,
) -> DoctorCheck {
    let mut warns: Vec<String> = Vec::new();
    let mut infos: Vec<String> = Vec::new();
    match config {
        Err(e) => warns.push(format!("backup config unreadable ({e}) — set the mode again")),
        Ok(None) => warns.push(UNCONFIGURED_NOTE.to_string()),
        Ok(Some(c)) if c.mode == "none" => infos.push(format!(
            "backups off by choice (set by {}): {}",
            c.configured_by,
            c.reason.as_deref().unwrap_or("(no reason recorded)")
        )),
        Ok(Some(c)) => infos.push(format!("mode {} chosen by {} — {NOT_RUNNING_NOTE}", c.mode, c.configured_by)),
    }
    match state.status.as_deref() {
        Some("ok") | None => {}
        Some("permission_denied") => warns.push(format!(
            "churn observer cannot read the store: {}",
            state.last_error.as_deref().unwrap_or("permission denied")
        )),
        Some(_) => warns.push(format!(
            "churn observer failing: {}",
            state.last_error.as_deref().unwrap_or("unknown error")
        )),
    }
    let status = if warns.is_empty() { ST_INFO } else { ST_WARN };
    let detail = warns.into_iter().chain(infos).collect::<Vec<_>>().join(" · ");
    DoctorCheck {
        id: "mail-backup".into(),
        label: "Mail backup".into(),
        status,
        detail,
        gates_direct: false,
    }
}

pub fn doctor_check() -> DoctorCheck {
    doctor_check_from(&load_config(), &load_state())
}

// ── Routes ──────────────────────────────────────────────────────────────

fn err_json(status: &'static str, code: &str, hint: String) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({"ok": false, "error": {"code": code, "hint": hint}}).to_string(),
    }
}

fn usage(hint: impl Into<String>) -> CliResponse {
    err_json("400 Bad Request", "usage", hint.into())
}

/// The live inputs: store listing, filesystem, config dir, K2 half and the
/// observer's churn. Reads only.
pub fn live_inputs() -> Result<PlanInputs, CliResponse> {
    if !mail_supported() {
        return Err(err_json(
            "409 Conflict",
            "unsupported",
            "the email server only works on Linux deployments; this daemon is not Linux".into(),
        ));
    }
    if !installed() {
        return Err(not_installed());
    }
    let root = Path::new(STALWART_DATA_DIR);
    let inv = scan_store(root)
        .map_err(|e| err_json("409 Conflict", "store_unreadable", e.message()))?;
    let fs = fs_stats(root).map_err(|e| err_json("409 Conflict", "store_unreadable", e))?;
    let mut notes = Vec::new();
    let config_bytes = match tree_bytes(Path::new(STALWART_CONFIG_DIR)) {
        Ok(b) => b,
        Err(e) => {
            notes.push(format!("/etc/stalwart size unknown ({}) — counted as 0", e.message()));
            0
        }
    };
    Ok(PlanInputs {
        immutable_bytes: inv.immutable_bytes,
        mutable_bytes: inv.mutable_bytes,
        immutable_files: inv.files.len() as u64,
        config_bytes,
        k2_half_bytes: k2_half_bytes(&k2_core::paths::k2_home()),
        fs_free: fs.free,
        fs_total: fs.total,
        same_fs: live_same_fs(fs.dev),
        churn: churn_of(&load_state().days),
        notes,
    })
}

/// k2.db (+ WAL), mail-secrets.json, certs/, settings.json, tunnel.json
/// under `home` (PRD §3 step 3). Missing parts count 0.
pub fn k2_half_bytes(home: &Path) -> u64 {
    let len = |p: PathBuf| std::fs::symlink_metadata(&p).map(|m| m.len()).unwrap_or(0);
    let db = k2_core::db::resolve_home_db_path(home);
    let mut wal = db.clone().into_os_string();
    wal.push("-wal");
    len(db.clone())
        + len(PathBuf::from(wal))
        + len(home.join("mail-secrets.json"))
        + len(home.join("settings.json"))
        + len(home.join("tunnel.json"))
        + tree_bytes(&home.join("certs")).unwrap_or(0)
}

fn not_installed() -> CliResponse {
    err_json(
        "409 Conflict",
        "not_installed",
        "the email server is not installed — there is no mail store to back up".into(),
    )
}

fn parse_u32(v: &str, flag: &str) -> Result<u32, CliResponse> {
    v.trim()
        .parse::<u32>()
        .map_err(|_| usage(format!("{flag} must be a whole number, got '{v}'")))
}

fn parse_gb(v: &str) -> Result<f64, CliResponse> {
    v.trim()
        .parse::<f64>()
        .map_err(|_| usage(format!("--max-local-gb must be a number of GiB, got '{v}'")))
}

/// `GET /cli/mail/backup` — the `backup` status block.
pub fn handle_status_get(_params: &HashMap<String, String>) -> CliResponse {
    let installed = installed();
    CliResponse::ok_json(
        serde_json::json!({"ok": true, "backup": status_json(installed)}).to_string(),
    )
}

/// `GET /cli/mail/backup/plan[?bandwidthMbps=&keepDaily=&keepWeekly=&maxLocalGb=]`.
pub fn handle_plan(params: &HashMap<String, String>) -> CliResponse {
    handle_plan_with(params, &live_inputs)
}

pub fn handle_plan_with(
    params: &HashMap<String, String>,
    inputs: &dyn Fn() -> Result<PlanInputs, CliResponse>,
) -> CliResponse {
    let known = ["bandwidthMbps", "keepDaily", "keepWeekly", "maxLocalGb"];
    // Identity stamps (`project`, `from`, …) ride every request; only the
    // plan's own keys are parsed.
    let get = |k: &str| params.get(k).map(String::as_str).filter(|v| !v.trim().is_empty());
    let mbps = match get("bandwidthMbps") {
        None => DEFAULT_BANDWIDTH_MBPS,
        Some(v) => match v.trim().parse::<f64>() {
            Ok(n) if n.is_finite() && n > 0.0 && n <= 1_000_000.0 => n,
            _ => return usage(format!("--bandwidth-mbps must be a positive number, got '{v}'")),
        },
    };
    let mut args = RetentionArgs::default();
    if let Some(v) = get(known[1]) {
        args.keep_daily = Some(match parse_u32(v, "--keep-daily") {
            Ok(n) => n,
            Err(r) => return r,
        });
    }
    if let Some(v) = get(known[2]) {
        args.keep_weekly = Some(match parse_u32(v, "--keep-weekly") {
            Ok(n) => n,
            Err(r) => return r,
        });
    }
    if let Some(v) = get(known[3]) {
        args.max_local_gb = Some(match parse_gb(v) {
            Ok(n) => n,
            Err(r) => return r,
        });
    }
    let current = match load_config() {
        Ok(c) => c,
        Err(_) => None,
    };
    let retention = resolve_retention(&args, current.as_ref());
    if let Err(e) = check_retention(&retention) {
        return usage(e);
    }
    let inp = match inputs() {
        Ok(i) => i,
        Err(r) => return r,
    };
    let plan = build_plan(&inp, &PlanOpts { bandwidth_mbps: mbps, retention });
    CliResponse::ok_json(plan.to_string())
}

/// Parsed `POST /cli/mail/backup/set` body.
#[derive(Debug, Clone, PartialEq)]
pub struct SetRequest {
    pub mode: String,
    pub reason: Option<String>,
    pub retention: RetentionArgs,
    pub confirm_plan: Option<String>,
}

pub fn parse_set_body(body: &[u8]) -> Result<SetRequest, String> {
    let text = std::str::from_utf8(body).map_err(|_| "body is not UTF-8".to_string())?;
    let v: serde_json::Value = if text.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(text).map_err(|e| format!("invalid JSON body: {e}"))?
    };
    let obj = v.as_object().ok_or_else(|| "body must be a JSON object".to_string())?;
    const KNOWN: &[&str] = &[
        "mode",
        "reason",
        "keepDaily",
        "keepWeekly",
        "offsiteKeepDaily",
        "offsiteKeepWeekly",
        "offsiteKeepMonthly",
        "window",
        "maxLocalGb",
        "confirmPlan",
        // identity stamps some callers add; ignored
        "project",
        "project_path",
        "project_id",
        "from",
    ];
    if let Some(k) = obj.keys().find(|k| !KNOWN.contains(&k.as_str())) {
        return Err(format!("unknown field '{k}'"));
    }
    let s = |k: &str| -> Result<Option<String>, String> {
        match obj.get(k) {
            None | Some(serde_json::Value::Null) => Ok(None),
            Some(serde_json::Value::String(x)) => {
                Ok(Some(x.trim().to_string()).filter(|x| !x.is_empty()))
            }
            Some(_) => Err(format!("{k} must be a string")),
        }
    };
    let n = |k: &str| -> Result<Option<u32>, String> {
        match obj.get(k) {
            None | Some(serde_json::Value::Null) => Ok(None),
            Some(x) => x
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .map(Some)
                .ok_or_else(|| format!("{k} must be a whole number")),
        }
    };
    let max_local_gb = match obj.get("maxLocalGb") {
        None | Some(serde_json::Value::Null) => None,
        Some(x) => Some(x.as_f64().ok_or_else(|| "maxLocalGb must be a number".to_string())?),
    };
    Ok(SetRequest {
        mode: s("mode")?.unwrap_or_default(),
        reason: s("reason")?,
        retention: RetentionArgs {
            keep_daily: n("keepDaily")?,
            keep_weekly: n("keepWeekly")?,
            offsite_keep_daily: n("offsiteKeepDaily")?,
            offsite_keep_weekly: n("offsiteKeepWeekly")?,
            offsite_keep_monthly: n("offsiteKeepMonthly")?,
            window: s("window")?,
            max_local_gb,
        },
        confirm_plan: s("confirmPlan")?,
    })
}

/// Who is setting it: an agent passport → `agent:<workspace>`, else the
/// owner token or an Admin+ login (the route floor) → `owner/admin`.
fn caller_label() -> (String, Option<String>) {
    match crate::caller_workspace::request_principal() {
        Some(p) => {
            let handle =
                k2_core::workspace_session_handles::workspace_address_name_shared(&p.workspace_uuid)
                    .unwrap_or_else(|_| p.workspace_uuid.clone());
            let path = crate::workspace_msg::resolve_workspace(&p.workspace_uuid)
                .unwrap_or_else(|| p.workspace_uuid.clone());
            (format!("agent:{handle}"), Some(path))
        }
        None => ("owner/admin".to_string(), None),
    }
}

fn audit(by: &str, outcome: &str) {
    if cfg!(test) {
        return;
    }
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        "hostmail-backup-set",
        by,
        outcome.to_string(),
        "-",
        "-",
        "cli",
    ));
}

/// `POST /cli/mail/backup/set`.
pub fn handle_set(body: &[u8]) -> CliResponse {
    handle_set_with(body, &live_inputs, supervisor::now_secs())
}

pub fn handle_set_with(
    body: &[u8],
    inputs: &dyn Fn() -> Result<PlanInputs, CliResponse>,
    now: i64,
) -> CliResponse {
    let req = match parse_set_body(body) {
        Ok(r) => r,
        Err(e) => return usage(format!("{e} — usage: k2 hostmail backup set --mode local|none … --confirm-plan <hash>")),
    };
    if !MODES.contains(&req.mode.as_str()) {
        return usage(format!(
            "--mode must be one of local, offsite-only, both, none (got '{}')",
            req.mode
        ));
    }
    if req.mode == "none" {
        match req.reason.as_deref() {
            None => {
                return usage(
                    "--mode none needs --reason \"<why>\" (e.g. \"provider snapshots daily\") — \
                     it is recorded with who set it and when",
                )
            }
            Some(r) if r.chars().count() > MAX_REASON_CHARS => {
                return usage(format!("--reason is at most {MAX_REASON_CHARS} characters"))
            }
            Some(_) => {}
        }
    } else if req.reason.is_some() {
        return usage("--reason goes with --mode none only");
    }
    if req.mode == "offsite-only" || req.mode == "both" {
        return err_json("409 Conflict", "offsite_destination_unset", OFFSITE_UNSET_HINT.into());
    }
    let current = match load_config() {
        Ok(c) => c,
        Err(_) => None, // unreadable: overwritten by this set
    };
    let retention = resolve_retention(&req.retention, current.as_ref());
    if let Err(e) = check_retention(&retention) {
        return usage(e);
    }
    let Some(confirm) = req.confirm_plan.clone() else {
        return usage(
            "--confirm-plan <hash> is required: run `k2 hostmail backup plan` (with the same \
             --keep-daily/--keep-weekly/--max-local-gb) and pass its planHash",
        );
    };
    let inp = match inputs() {
        Ok(i) => i,
        Err(r) => return r,
    };
    if !installed() {
        return not_installed();
    }
    let plan = build_plan(
        &inp,
        &PlanOpts { bandwidth_mbps: DEFAULT_BANDWIDTH_MBPS, retention: retention.clone() },
    );
    let hash = plan["planHash"].as_str().unwrap_or_default().to_string();
    if confirm != hash {
        return err_json(
            "409 Conflict",
            "plan_changed",
            format!(
                "--confirm-plan {confirm} is not the current plan (now {hash}) — run `k2 hostmail \
                 backup plan` again with the same retention flags, read it, then pass the new hash"
            ),
        );
    }
    if req.mode == "local" && plan["modes"]["local"]["verdict"] == "refused" {
        return err_json(
            "409 Conflict",
            "refused_disk",
            format!(
                "local backups would not fit: the plan's local verdict is refused (needs {} \
                 bytes, free {}, floor {}). Lower the retention or --max-local-gb, or choose none \
                 with a reason",
                plan["modes"]["local"]["peakBytes"], plan["store"]["fsFree"], plan["store"]["floorBytes"]
            ),
        );
    }
    let (by, workspace) = caller_label();
    let from = current.as_ref().map(|c| c.mode.clone()).unwrap_or_else(|| "unconfigured".into());
    let mut history = current.as_ref().map(|c| c.history.clone()).unwrap_or_default();
    history.push(HistoryEntry {
        at: now,
        by: by.clone(),
        workspace: workspace.clone(),
        from: from.clone(),
        to: req.mode.clone(),
    });
    let excess = history.len().saturating_sub(HISTORY_KEEP);
    history.drain(..excess);
    let cfg = BackupConfig {
        mode: req.mode.clone(),
        reason: req.reason.clone(),
        keep_daily: retention.keep_daily,
        keep_weekly: retention.keep_weekly,
        offsite_keep_daily: retention.offsite_keep_daily,
        offsite_keep_weekly: retention.offsite_keep_weekly,
        offsite_keep_monthly: retention.offsite_keep_monthly,
        window: retention.window.clone(),
        max_local_gb: retention.max_local_gb,
        configured_by: by.clone(),
        configured_workspace: workspace,
        configured_at: now,
        plan_hash: hash,
        history,
    };
    let raw = match serde_json::to_string(&cfg) {
        Ok(r) => r,
        Err(e) => return CliResponse::internal_error(format!("encode backup config: {e}")),
    };
    if let Err(e) = update_col(CONFIG_COL, &raw) {
        return err_json("409 Conflict", "not_installed", e);
    }
    audit(&by, &format!("{from}->{}", req.mode));
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "changed": from != req.mode,
            "from": from,
            "backup": status_json(true),
        })
        .to_string(),
    )
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "k2-backup-{tag}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn put(root: &Path, rel: &str, len: u64) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        let f = std::fs::File::create(&p).unwrap();
        f.set_len(len).unwrap();
    }

    fn fs_fixed(free: u64, total: u64) -> impl Fn(&Path) -> Result<FsStats, String> {
        move |_| Ok(FsStats { dev: 7, free, total })
    }

    #[test]
    fn classify_splits_immutable_mutable_and_excluded() {
        assert_eq!(classify("000123.sst"), FileClass::Immutable);
        assert_eq!(classify("000124.blob"), FileClass::Immutable);
        assert_eq!(classify("000125.log"), FileClass::Mutable, "WAL is mutable");
        assert_eq!(classify("MANIFEST-000005"), FileClass::Mutable);
        assert_eq!(classify("OPTIONS-000007"), FileClass::Mutable);
        assert_eq!(classify("CURRENT"), FileClass::Mutable);
        assert_eq!(classify("IDENTITY"), FileClass::Mutable);
        assert_eq!(classify("LOG"), FileClass::Excluded);
        assert_eq!(classify("LOG.old.1700000000"), FileClass::Excluded);
        assert_eq!(classify("LOCK"), FileClass::Excluded);
    }

    #[test]
    fn scan_counts_classes_excludes_log_and_skips_symlinks() {
        let root = tmp("scan");
        put(&root, "data/000001.sst", 1000);
        put(&root, "data/000002.blob", 500);
        put(&root, "data/000003.log", 40);
        put(&root, "data/MANIFEST-000004", 10);
        put(&root, "data/CURRENT", 16);
        put(&root, "data/LOG", 9_999);
        put(&root, "data/LOG.old.1", 7_777);
        put(&root, "data/LOCK", 0);
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("data/000001.sst"), root.join("data/link.sst")).unwrap();
        let inv = scan_store(&root).expect("scan");
        assert_eq!(inv.immutable_bytes, 1500);
        assert_eq!(inv.files.len(), 2);
        assert!(inv.files.contains_key("data/000001.sst"), "{:?}", inv.files);
        assert_eq!(inv.mutable_bytes, 66);
        assert_eq!(inv.mutable_files, 3);
        assert_eq!(inv.excluded_bytes, 9_999 + 7_777, "LOG* never counted as store");
        assert_eq!(inv.store_bytes(), 1566);
        #[cfg(unix)]
        assert_eq!(inv.skipped_links, 1);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn scan_missing_dir_is_a_loud_missing_error() {
        let root = std::env::temp_dir().join(format!("k2-backup-none-{}", uuid::Uuid::new_v4()));
        let err = scan_store(&root).expect_err("missing dir must fail");
        assert_eq!(err.status(), "missing");
        assert!(err.message().contains(&root.display().to_string()), "{}", err.message());
    }

    #[cfg(unix)]
    #[test]
    fn scan_unreadable_dir_is_permission_denied_unless_root() {
        use std::os::unix::fs::PermissionsExt;
        // SAFETY: geteuid has no preconditions.
        if unsafe { libc::geteuid() } == 0 {
            // root ignores the mode; the classification is covered below.
            let e = io_scan_err(
                "list",
                Path::new("/x"),
                std::io::Error::from(std::io::ErrorKind::PermissionDenied),
            );
            assert_eq!(e.status(), "permission_denied");
            return;
        }
        let root = tmp("perm");
        put(&root, "data/000001.sst", 10);
        std::fs::set_permissions(root.join("data"), std::fs::Permissions::from_mode(0o000)).unwrap();
        let err = scan_store(&root).expect_err("unreadable dir must fail");
        std::fs::set_permissions(root.join("data"), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(err.status(), "permission_denied", "{err:?}");
        assert!(err.message().contains("never uses sudo"), "{}", err.message());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn diff_counts_created_and_deleted_bytes() {
        let rec = |b: u64| FileRec { bytes: b, mtime: 1 };
        let prev: BTreeMap<String, FileRec> = [
            ("a.sst".to_string(), rec(100)),
            ("b.sst".to_string(), rec(200)),
            ("c.blob".to_string(), rec(300)),
        ]
        .into();
        let cur: BTreeMap<String, FileRec> = [
            ("a.sst".to_string(), rec(100)),
            ("c.blob".to_string(), FileRec { bytes: 300, mtime: 2 }),
            ("d.sst".to_string(), rec(50)),
        ]
        .into();
        let d = diff(&prev, &cur);
        // b deleted (200), c changed (300 out, 300 in), d created (50).
        assert_eq!(d.deleted_bytes, 500);
        assert_eq!(d.deleted_files, 2);
        assert_eq!(d.created_bytes, 350);
        assert_eq!(d.created_files, 2);
        assert_eq!(diff(&cur, &cur), Diff::default());
    }

    #[test]
    fn observe_first_run_has_no_day_then_diffs_the_next() {
        let root = tmp("obs");
        put(&root, "data/000001.sst", 1_000);
        put(&root, "data/000002.sst", 2_000);
        put(&root, "data/MANIFEST-1", 10);
        let fs = fs_fixed(50 * GIB, 100 * GIB);
        let (s1, inv1) = observe(&root, 1_000_000, &ObserverState::default(), &BTreeMap::new(), &fs, &|_| Some(true));
        assert_eq!(s1.status.as_deref(), Some("ok"));
        assert!(s1.days.is_empty(), "first inventory has nothing to diff");
        let inv1 = inv1.expect("inventory to persist");
        assert_eq!(s1.latest.as_ref().unwrap().store_bytes, 3_010);
        assert_eq!(s1.latest.as_ref().unwrap().same_fs, Some(true));

        // Compaction: 000001 + 000002 merged into 000003; one new blob.
        std::fs::remove_file(root.join("data/000001.sst")).unwrap();
        std::fs::remove_file(root.join("data/000002.sst")).unwrap();
        put(&root, "data/000003.sst", 2_500);
        put(&root, "data/000004.blob", 700);
        let (s2, _) = observe(&root, 1_000_000 + 86_400, &s1, &inv1, &fs, &|_| Some(true));
        assert_eq!(s2.days.len(), 1);
        let d = &s2.days[0];
        assert_eq!(d.deleted_bytes, 3_000);
        assert_eq!(d.created_bytes, 3_200);
        assert_eq!(d.growth_bytes, 200);
        assert_eq!(d.interval_secs, 86_400);
        let churn = churn_of(&s2.days);
        assert_eq!(churn.deleted_per_day, 3_000);
        assert_eq!(churn.created_per_day, 3_200);
        assert!(!churn.measured, "one day is not measured yet");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn observe_keeps_thirty_daily_summaries() {
        let root = tmp("rot");
        put(&root, "data/000001.sst", 10);
        let fs = fs_fixed(GIB, 10 * GIB);
        let mut state = ObserverState::default();
        let mut files = BTreeMap::new();
        for day in 0..40i64 {
            let (s, inv) = observe(&root, 100 + day * 86_400, &state, &files, &fs, &|_| None);
            state = s;
            files = inv.unwrap();
        }
        assert_eq!(state.days.len(), KEEP_SUMMARIES);
        assert_eq!(state.days.last().unwrap().at, 100 + 39 * 86_400);
        assert_eq!(state.days.first().unwrap().at, 100 + 10 * 86_400, "oldest dropped first");
        assert!(churn_of(&state.days).measured);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn observe_error_keeps_history_and_records_last_error() {
        let fs = fs_fixed(GIB, 10 * GIB);
        let prior = ObserverState {
            status: Some("ok".into()),
            last_success: Some(5),
            days: vec![DaySummary { at: 5, interval_secs: 86_400, ..Default::default() }],
            ..Default::default()
        };
        let missing = std::env::temp_dir().join(format!("k2-backup-gone-{}", uuid::Uuid::new_v4()));
        let (s, inv) = observe(&missing, 99, &prior, &BTreeMap::new(), &fs, &|_| None);
        assert!(inv.is_none());
        assert_eq!(s.status.as_deref(), Some("missing"));
        assert_eq!(s.last_run, Some(99));
        assert_eq!(s.last_success, Some(5));
        assert_eq!(s.days.len(), 1, "history survives a failed run");
        assert!(s.last_error.unwrap().contains("k2-backup-gone"));
        let root = tmp("fserr");
        let (s, inv) = observe(&root, 99, &prior, &BTreeMap::new(), &|_| Err("boom".into()), &|_| None);
        assert!(inv.is_none());
        assert_eq!(s.status.as_deref(), Some("error"));
        assert_eq!(s.last_error.as_deref(), Some("free space: boom"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn due_is_daily_with_hourly_retry() {
        let never = ObserverState::default();
        assert!(due(&never, 0));
        let ok = ObserverState { last_run: Some(1_000), last_success: Some(1_000), ..Default::default() };
        assert!(!due(&ok, 1_000 + 3_600));
        assert!(due(&ok, 1_000 + 86_400 - 3_600));
        let failed = ObserverState { last_run: Some(90_000), last_success: Some(1_000), ..Default::default() };
        assert!(!due(&failed, 90_000 + 60), "a failure waits for the next tick");
        assert!(due(&failed, 90_000 + 3_600));
    }

    #[test]
    fn floor_and_verdicts() {
        assert_eq!(floor_bytes(10 * GIB), 5 * GIB, "small disks: 5 GiB");
        assert_eq!(floor_bytes(512 * GIB), (512.0 * GIB as f64 * 0.1) as u64, "10% of the FS");
        let floor = 5 * GIB;
        assert_eq!(verdict(100 * GIB, 10 * GIB, floor), "ok");
        assert_eq!(verdict(16 * GIB, 7 * GIB, floor), "tight", "9 GiB left < 2 × floor");
        assert_eq!(verdict(10 * GIB, 6 * GIB, floor), "refused", "4 GiB left < floor");
        assert_eq!(verdict(GIB, 10 * GIB, floor), "refused", "negative never wraps");
    }

    fn inputs(store_gib: u64, free_gib: u64, churn: Churn) -> PlanInputs {
        PlanInputs {
            immutable_bytes: store_gib * GIB - 200 * (1 << 20),
            mutable_bytes: 200 * (1 << 20),
            immutable_files: 10_000,
            config_bytes: 1 << 20,
            k2_half_bytes: 50 << 20,
            fs_free: free_gib * GIB,
            fs_total: 512 * GIB,
            same_fs: Some(true),
            churn,
            notes: vec![],
        }
    }

    fn opts() -> PlanOpts {
        PlanOpts { bandwidth_mbps: 50.0, retention: Retention::default() }
    }

    #[test]
    fn plan_unmeasured_uses_the_conservative_bound() {
        let inp = inputs(100, 300, Churn::default());
        let p = build_plan(&inp, &opts());
        assert_eq!(p["churn"]["quality"], "unmeasured");
        assert!(p["churn"]["label"].as_str().unwrap().starts_with("unmeasured — conservative bound"));
        let s = inp.store_bytes();
        let fixed = inp.mutable_bytes + inp.config_bytes + inp.k2_half_bytes;
        let day = (s as f64 * 0.10) as u64;
        assert_eq!(p["churn"]["deletedPerDay"], day);
        let local = &p["modes"]["local"];
        assert_eq!(local["firstNightLocalBytes"], fixed);
        // 11 views (7 daily + 4 weekly), 28 days of retained churn.
        assert_eq!(local["steadyLocalBytes"], 11 * fixed + day * 28);
        assert_eq!(local["available"], true);
        // offsite temp: fixed + pinned (≥ the PRD's 10% bound) + cache.
        let off = &p["modes"]["offsite-only"];
        assert_eq!(off["available"], false);
        assert_eq!(off["unavailableReason"], OFFSITE_UNSET_HINT);
        let nightly_temp = off["offsite"]["nightlyTempBytes"].as_u64().unwrap();
        assert_eq!(nightly_temp, fixed + day + RESTIC_CACHE_BYTES);
        assert_eq!(off["offsite"]["firstUploadBytes"]["high"], s + inp.config_bytes + inp.k2_half_bytes);
        // 100 GiB at 50 Mbit/s ≈ 4.8 h.
        let secs = off["offsite"]["firstUploadSecs"].as_u64().unwrap();
        assert!((17_000..18_000).contains(&secs), "{secs}");
        assert_eq!(p["modes"]["none"]["peakBytes"], 0);
        assert_eq!(p["modes"]["none"]["verdict"], "ok");
        // downtime: 15 s + ceil(251 MiB / 100 MiB/s)=3 + 10000 × 2 ms = 20.
        assert_eq!(local["downtimeSecs"], 15 + 3 + 20);
        assert!(local["downtimeLabel"].as_str().unwrap().contains("not measured"));
        assert_eq!(p["planHash"].as_str().unwrap().len(), 12);
    }

    #[test]
    fn plan_measured_uses_observed_churn() {
        let churn = Churn {
            days: 5,
            measured: true,
            deleted_per_day: 2 * GIB,
            created_per_day: 3 * GIB,
            max_deleted_per_day: 4 * GIB,
            growth_per_day: GIB as i64,
        };
        let inp = inputs(100, 300, churn);
        let p = build_plan(&inp, &opts());
        assert_eq!(p["churn"]["quality"], "measured");
        assert_eq!(p["churn"]["label"], "measured over 5 days");
        let fixed = inp.mutable_bytes + inp.config_bytes + inp.k2_half_bytes;
        assert_eq!(p["modes"]["local"]["steadyLocalBytes"], 11 * fixed + 2 * GIB * 28);
        let off = &p["modes"]["offsite-only"]["offsite"];
        assert_eq!(off["nightlyUploadBytes"], 3 * GIB + fixed);
        let nightly_secs = off["nightlyUploadSecs"].as_u64().unwrap();
        let pinned = (4.0 * GIB as f64 * nightly_secs as f64 / 86_400.0).ceil() as u64;
        assert_eq!(off["nightlyTempBytes"], fixed + pinned + RESTIC_CACHE_BYTES);
    }

    #[test]
    fn plan_cross_fs_is_a_full_copy_and_says_so() {
        let mut inp = inputs(100, 300, Churn::default());
        inp.same_fs = Some(false);
        let p = build_plan(&inp, &opts());
        let raw = inp.store_bytes() + inp.config_bytes + inp.k2_half_bytes;
        assert_eq!(p["modes"]["local"]["firstNightLocalBytes"], raw);
        assert_eq!(p["modes"]["local"]["steadyLocalBytes"], 11 * raw);
        assert_eq!(p["modes"]["local"]["verdict"], "refused", "11 full copies never fit");
        assert!(p["notes"].to_string().contains("full copy"), "{}", p["notes"]);
    }

    #[test]
    fn plan_verdicts_follow_the_floor_and_the_cap() {
        // 512 GiB disk → floor 51.2 GiB. Store 100 GiB, unmeasured local
        // steady ≈ 283 GiB.
        let p = build_plan(&inputs(100, 600, Churn::default()), &opts());
        assert_eq!(p["modes"]["local"]["verdict"], "ok");
        let p = build_plan(&inputs(100, 360, Churn::default()), &opts());
        assert_eq!(p["modes"]["local"]["verdict"], "tight");
        let p = build_plan(&inputs(100, 300, Churn::default()), &opts());
        assert_eq!(p["modes"]["local"]["verdict"], "refused");
        // A cap bounds steady state …
        let mut o = opts();
        o.retention.max_local_gb = Some(20.0);
        let p = build_plan(&inputs(100, 300, Churn::default()), &o);
        assert_eq!(p["modes"]["local"]["steadyLocalBytes"], 20 * GIB);
        assert_eq!(p["modes"]["local"]["verdict"], "ok");
        // … but one view larger than the cap is refused.
        o.retention.max_local_gb = Some(0.1);
        let p = build_plan(&inputs(100, 300, Churn::default()), &o);
        assert_eq!(p["modes"]["local"]["capTooSmall"], true);
        assert_eq!(p["modes"]["local"]["verdict"], "refused");
    }

    #[test]
    fn plan_hash_changes_with_the_numbers_not_with_noise_or_bandwidth() {
        let base = inputs(100, 600, Churn::default());
        let h = |inp: &PlanInputs, o: &PlanOpts| build_plan(inp, o)["planHash"].as_str().unwrap().to_string();
        let h0 = h(&base, &opts());
        let mut noise = base.clone();
        noise.mutable_bytes += 3 << 20;
        noise.fs_free -= 5 << 20;
        assert_eq!(h(&noise, &opts()), h0, "a few MB of new mail keeps the hash");
        let mut o = opts();
        o.bandwidth_mbps = 500.0;
        assert_eq!(h(&base, &o), h0, "bandwidth changes times only");
        let mut bigger = base.clone();
        bigger.immutable_bytes += 20 * GIB;
        assert_ne!(h(&bigger, &opts()), h0, "store size");
        let mut fuller = base.clone();
        fuller.fs_free -= 200 * GIB;
        assert_ne!(h(&fuller, &opts()), h0, "free space");
        let mut measured = base.clone();
        measured.churn = Churn { days: 3, measured: true, ..Churn::default() };
        assert_ne!(h(&measured, &opts()), h0, "churn quality");
        let mut o = opts();
        o.retention.keep_daily = 14;
        assert_ne!(h(&base, &o), h0, "retention");
    }

    #[test]
    fn window_and_retention_validation() {
        assert_eq!(parse_window("03:30-04:30").unwrap(), ((3, 30), (4, 30)));
        assert!(parse_window("23:30-00:30").is_ok(), "may wrap midnight");
        for bad in ["3:30-4:30", "24:00-01:00", "03:60-04:00", "03:30", "03:30-03:30", ""] {
            assert!(parse_window(bad).is_err(), "{bad}");
        }
        let mut r = Retention::default();
        assert!(check_retention(&r).is_ok());
        r.keep_daily = 0;
        assert!(check_retention(&r).unwrap_err().contains("--keep-daily"));
        r = Retention { max_local_gb: Some(-1.0), ..Retention::default() };
        assert!(check_retention(&r).is_err());
        // resolution: explicit > current config > default
        let cur = BackupConfig { keep_daily: 3, keep_weekly: 1, window: "01:00-02:00".into(), ..Default::default() };
        let got = resolve_retention(&RetentionArgs { keep_weekly: Some(9), ..Default::default() }, Some(&cur));
        assert_eq!((got.keep_daily, got.keep_weekly, got.window.as_str()), (3, 9, "01:00-02:00"));
        assert_eq!(resolve_retention(&RetentionArgs::default(), None), Retention::default());
    }

    #[test]
    fn same_fs_compares_the_view_root_or_its_parent() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let root = tmp("samefs");
            let dev = std::fs::metadata(&root).unwrap().dev();
            assert_eq!(same_fs_for(dev, &root.join("not-created-views")), Some(true));
            assert_eq!(same_fs_for(dev.wrapping_add(1), &root.join("not-created-views")), Some(false));
            assert!(!root.join("not-created-views").exists(), "B1 never creates the view root");
            std::fs::remove_dir_all(&root).unwrap();
        }
    }

    #[test]
    fn k2_half_counts_db_secrets_and_certs() {
        let home = tmp("k2half");
        put(&home, "k2.db", 1_000);
        put(&home, "k2.db-wal", 100);
        put(&home, "mail-secrets.json", 10);
        put(&home, "certs/mail.example.com/fullchain.pem", 5);
        put(&home, "unrelated.log", 99_999);
        assert_eq!(k2_half_bytes(&home), 1_115);
        std::fs::remove_dir_all(&home).unwrap();
    }

    // ── DB-backed: routes, status, doctor, tick ────────────────────────

    fn seed_row() {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("DELETE FROM mail_server WHERE id = 1", []).unwrap();
        conn.execute("DELETE FROM mail_backup_inventory", []).unwrap();
        conn.execute(
            "INSERT INTO mail_server (id, status, pinned_version, hostname, updated_at) \
             VALUES (1, 'running', '0.16.20', 'mail.example.com', 1)",
            [],
        )
        .unwrap();
    }

    fn clean_row() {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("DELETE FROM mail_server WHERE id = 1", []).unwrap();
        conn.execute("DELETE FROM mail_backup_inventory", []).unwrap();
    }

    fn fixture_inputs() -> Result<PlanInputs, CliResponse> {
        Ok(inputs(100, 600, Churn::default()))
    }

    fn plan_hash_now() -> String {
        let r = handle_plan_with(&HashMap::new(), &fixture_inputs);
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        v["planHash"].as_str().unwrap().to_string()
    }

    fn set(body: serde_json::Value) -> (CliResponse, serde_json::Value) {
        let r = handle_set_with(body.to_string().as_bytes(), &fixture_inputs, 1_700_000_000);
        let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        (r, v)
    }

    #[test]
    fn set_validates_mode_reason_offsite_and_the_plan_hash() {
        let _g = crate::mail::mail_server_test_lock();
        seed_row();
        let hash = plan_hash_now();

        let (r, v) = set(serde_json::json!({"mode": "weekly", "confirmPlan": hash}));
        assert_eq!(r.status, "400 Bad Request");
        assert_eq!(v["error"]["code"], "usage");

        let (r, v) = set(serde_json::json!({"mode": "none", "confirmPlan": hash}));
        assert_eq!(r.status, "400 Bad Request", "{v}");
        assert!(v["error"]["hint"].as_str().unwrap().contains("--reason"), "{v}");

        for mode in ["offsite-only", "both"] {
            let (r, v) = set(serde_json::json!({"mode": mode, "confirmPlan": hash}));
            assert_eq!(r.status, "409 Conflict", "{v}");
            assert_eq!(v["error"]["code"], "offsite_destination_unset");
            assert!(v["error"]["hint"].as_str().unwrap().contains("B3"), "{v}");
        }

        let (r, v) = set(serde_json::json!({"mode": "local"}));
        assert_eq!(r.status, "400 Bad Request", "{v}");
        assert!(v["error"]["hint"].as_str().unwrap().contains("--confirm-plan"), "{v}");

        let (r, v) = set(serde_json::json!({"mode": "local", "confirmPlan": "000000000000"}));
        assert_eq!(r.status, "409 Conflict", "{v}");
        assert_eq!(v["error"]["code"], "plan_changed");
        assert!(v["error"]["hint"].as_str().unwrap().contains(&hash), "names the current hash: {v}");

        // A plan for other retention flags does not confirm this set.
        let (r, v) = set(serde_json::json!({"mode": "local", "keepDaily": 14, "confirmPlan": hash}));
        assert_eq!(r.status, "409 Conflict", "{v}");
        assert_eq!(v["error"]["code"], "plan_changed");

        let (r, v) = set(serde_json::json!({"mode": "local", "bogus": 1, "confirmPlan": hash}));
        assert_eq!(r.status, "400 Bad Request", "{v}");
        assert!(v["error"]["hint"].as_str().unwrap().contains("bogus"), "{v}");

        // Nothing was written by any refusal.
        assert_eq!(load_config(), Ok(None));
        clean_row();
    }

    #[test]
    fn set_records_mode_who_when_and_history_and_status_says_nothing_runs() {
        let _g = crate::mail::mail_server_test_lock();
        seed_row();
        let hash = plan_hash_now();
        let (r, v) = set(serde_json::json!({"mode": "local", "window": "02:00-03:00", "confirmPlan": hash}));
        assert_eq!(r.status, "200 OK", "{v}");
        assert_eq!(v["from"], "unconfigured");
        assert_eq!(v["backup"]["mode"], "local");
        assert_eq!(v["backup"]["configuredBy"], "owner/admin");
        assert_eq!(v["backup"]["configuredAt"], 1_700_000_000);
        assert_eq!(v["backup"]["window"], "02:00-03:00");
        assert_eq!(v["backup"]["retention"]["local"]["keepDaily"], 7);
        assert_eq!(v["backup"]["running"], false);
        assert_eq!(v["backup"]["note"], NOT_RUNNING_NOTE);
        assert!(NOT_RUNNING_NOTE.contains("later release"));

        // none needs a reason; the plan for the stored retention still matches.
        let (r, v) = set(serde_json::json!({
            "mode": "none", "reason": "provider snapshots daily", "confirmPlan": plan_hash_now()
        }));
        assert_eq!(r.status, "200 OK", "{v}");
        let cfg = load_config().unwrap().unwrap();
        assert_eq!(cfg.mode, "none");
        assert_eq!(cfg.reason.as_deref(), Some("provider snapshots daily"));
        assert_eq!(cfg.window, "02:00-03:00", "window kept from the previous set");
        assert_eq!(cfg.history.len(), 2);
        assert_eq!((cfg.history[1].from.as_str(), cfg.history[1].to.as_str()), ("local", "none"));
        let st = status_json(true);
        assert!(st["note"].as_str().unwrap().contains("provider snapshots daily"), "{st}");
        clean_row();
    }

    #[test]
    fn set_by_an_agent_records_its_workspace() {
        let _g = crate::mail::mail_server_test_lock();
        seed_row();
        let p = crate::session_token::HookPrincipal {
            workspace_uuid: "ws-backup-test".to_string(),
            agent_address: "agent".to_string(),
        };
        let hash = plan_hash_now();
        let (r, v) = crate::caller_workspace::with_request_principal(Some(p), || {
            set(serde_json::json!({"mode": "local", "confirmPlan": hash}))
        });
        assert_eq!(r.status, "200 OK", "{v}");
        assert!(v["backup"]["configuredBy"].as_str().unwrap().starts_with("agent:"), "{v}");
        assert_eq!(v["backup"]["configuredWorkspace"], "ws-backup-test");
        clean_row();
    }

    #[test]
    fn set_refuses_local_when_the_plan_says_it_does_not_fit() {
        let _g = crate::mail::mail_server_test_lock();
        seed_row();
        let tight = || -> Result<PlanInputs, CliResponse> { Ok(inputs(100, 300, Churn::default())) };
        let r = handle_plan_with(&HashMap::new(), &tight);
        let hash = serde_json::from_str::<serde_json::Value>(&r.body).unwrap()["planHash"]
            .as_str()
            .unwrap()
            .to_string();
        let body = serde_json::json!({"mode": "local", "confirmPlan": hash}).to_string();
        let r = handle_set_with(body.as_bytes(), &tight, 1);
        assert_eq!(r.status, "409 Conflict", "{}", r.body);
        assert!(r.body.contains("refused_disk"), "{}", r.body);
        assert_eq!(load_config(), Ok(None));
        clean_row();
    }

    #[test]
    fn set_and_plan_need_an_installed_server() {
        let _g = crate::mail::mail_server_test_lock();
        clean_row();
        let hash = plan_hash_now();
        let (r, v) = set(serde_json::json!({"mode": "local", "confirmPlan": hash}));
        assert_eq!(r.status, "409 Conflict", "{v}");
        assert_eq!(v["error"]["code"], "not_installed");
        // The live gather refuses before any listing on a box without mail.
        let live = live_inputs().expect_err("no mail here");
        assert_eq!(live.status, "409 Conflict");
        let st = status_json(false);
        assert_eq!(st["mode"], "unconfigured");
        assert_eq!(st["installed"], false);
    }

    #[test]
    fn plan_params_are_validated() {
        let _g = crate::mail::mail_server_test_lock();
        seed_row();
        for (k, v) in [("bandwidthMbps", "0"), ("bandwidthMbps", "fast"), ("keepDaily", "0"), ("keepWeekly", "-1"), ("maxLocalGb", "lots")] {
            let params: HashMap<String, String> = [(k.to_string(), v.to_string())].into();
            let r = handle_plan_with(&params, &fixture_inputs);
            assert_eq!(r.status, "400 Bad Request", "{k}={v}: {}", r.body);
        }
        let params: HashMap<String, String> =
            [("bandwidthMbps".to_string(), "100".to_string()), ("keepDaily".to_string(), "3".to_string())].into();
        let v: serde_json::Value =
            serde_json::from_str(&handle_plan_with(&params, &fixture_inputs).body).unwrap();
        assert_eq!(v["bandwidthMbps"], 100.0);
        assert_eq!(v["retention"]["local"]["keepDaily"], 3);
        clean_row();
    }

    #[test]
    fn doctor_check_cases() {
        let st = ObserverState::default();
        let c = doctor_check_from(&Ok(None), &st);
        assert_eq!((c.id.as_str(), c.status), ("mail-backup", ST_WARN));
        assert!(c.detail.contains("k2 hostmail backup plan"), "{}", c.detail);
        assert!(!c.gates_direct);

        let none = BackupConfig { mode: "none".into(), reason: Some("provider snapshots".into()), configured_by: "owner/admin".into(), ..Default::default() };
        let c = doctor_check_from(&Ok(Some(none)), &st);
        assert_eq!(c.status, ST_INFO);
        assert!(c.detail.contains("provider snapshots"), "{}", c.detail);

        let local = BackupConfig { mode: "local".into(), configured_by: "agent:it".into(), ..Default::default() };
        let c = doctor_check_from(&Ok(Some(local.clone())), &st);
        assert_eq!(c.status, ST_INFO);
        assert!(c.detail.contains("later release"), "{}", c.detail);

        let denied = ObserverState {
            status: Some("permission_denied".into()),
            last_error: Some("list /var/lib/stalwart/data: Permission denied".into()),
            ..Default::default()
        };
        let c = doctor_check_from(&Ok(Some(local.clone())), &denied);
        assert_eq!(c.status, ST_WARN);
        assert!(c.detail.contains("cannot read the store"), "{}", c.detail);

        let failing = ObserverState { status: Some("missing".into()), last_error: Some("stat /var/lib/stalwart: gone".into()), ..Default::default() };
        let c = doctor_check_from(&Ok(Some(local)), &failing);
        assert_eq!(c.status, ST_WARN);
        assert!(c.detail.contains("observer failing"), "{}", c.detail);

        let c = doctor_check_from(&Err("bad json".into()), &st);
        assert_eq!(c.status, ST_WARN);
    }

    #[test]
    fn tick_observes_once_a_day_and_persists_the_inventory() {
        let _g = crate::mail::mail_server_test_lock();
        seed_row();
        let root = tmp("tick");
        put(&root, "data/000001.sst", 4_000);
        let fs = fs_fixed(10 * GIB, 100 * GIB);
        assert_eq!(tick_at(&root, 10_000, &fs, &|_| Some(true)), Ok(true));
        assert_eq!(tick_at(&root, 10_000 + 600, &fs, &|_| Some(true)), Ok(false), "not due");
        std::fs::remove_file(root.join("data/000001.sst")).unwrap();
        put(&root, "data/000002.sst", 1_000);
        assert_eq!(tick_at(&root, 10_000 + 86_400, &fs, &|_| Some(true)), Ok(true));
        let state = load_state();
        assert_eq!(state.days.len(), 1);
        assert_eq!(state.days[0].deleted_bytes, 4_000);
        assert_eq!(state.days[0].created_bytes, 1_000);
        let inv = load_inventory().unwrap();
        assert_eq!(inv.keys().collect::<Vec<_>>(), vec!["data/000002.sst"]);
        let st = status_json(true);
        assert_eq!(st["observer"]["status"], "ok");
        assert_eq!(st["observer"]["days"], 1);
        assert_eq!(st["observer"]["storeBytes"], 1_000);
        assert_eq!(st["observer"]["sameFs"], true);

        // A missing store dir is recorded loudly, history kept.
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(tick_at(&root, 10_000 + 2 * 86_400, &fs, &|_| Some(true)), Ok(true));
        let st = status_json(true);
        assert_eq!(st["observer"]["status"], "missing");
        assert_eq!(st["observer"]["days"], 1);
        assert!(st["observer"]["lastError"].as_str().unwrap().contains("k2-backup-tick"));
        let c = doctor_check();
        assert_eq!(c.status, ST_WARN);
        clean_row();
    }

    #[test]
    fn tick_drops_a_mismatched_inventory_instead_of_diffing_it() {
        let _g = crate::mail::mail_server_test_lock();
        seed_row();
        let root = tmp("mismatch");
        put(&root, "data/000001.sst", 4_000);
        let fs = fs_fixed(10 * GIB, 100 * GIB);
        assert_eq!(tick_at(&root, 10_000, &fs, &|_| None), Ok(true));
        // Simulate a crash between the two writes: inventory lost.
        {
            let db = k2_core::db::shared();
            db.lock().execute("DELETE FROM mail_backup_inventory", []).unwrap();
        }
        assert_eq!(tick_at(&root, 10_000 + 86_400, &fs, &|_| None), Ok(true));
        assert!(load_state().days.is_empty(), "no bogus 'everything created' day");
        std::fs::remove_dir_all(&root).unwrap();
        clean_row();
    }

    #[test]
    fn tick_skips_when_not_installed() {
        let _g = crate::mail::mail_server_test_lock();
        clean_row();
        let root = tmp("noinst");
        assert_eq!(tick_at(&root, 1, &fs_fixed(1, 1), &|_| None), Ok(false));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn routes_gate_405_and_agent_verbs() {
        use crate::mail_routes::{dispatch, dispatch_post, is_mail_manage_surface, is_owner_level_mutation, mail_manage_authorized};
        // Set is POST-only through the read chain and the route table.
        let get = dispatch("/cli/mail/backup/set", &HashMap::new()).expect("mail path");
        assert_eq!(get.status, "405 Method Not Allowed", "{}", get.body);
        assert!(crate::routes::route_policy::get_refused("/cli/mail/backup/set", ""));
        assert!(!crate::routes::route_policy::get_refused("/cli/mail/backup/plan", ""));
        assert!(!crate::routes::route_policy::get_refused("/cli/mail/backup", ""));
        assert!(crate::routes::route_policy::post_allowed("/cli/mail/backup/set"));
        assert!(!crate::routes::route_policy::post_allowed("/cli/mail/backup/plan"));
        // The POST arm reaches the handler (a usage refusal, not a 404).
        let r = dispatch_post("/cli/mail/backup/set", b"{}");
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        // Gate placement.
        for p in ["/cli/mail/backup/plan", "/cli/mail/backup/set"] {
            assert!(is_mail_manage_surface(p), "{p}");
            assert!(!is_owner_level_mutation(p), "{p} is mail-manage, not owner-only");
            assert!(crate::session_token::is_agent_verb(p), "{p}");
            assert!(mail_manage_authorized(p, true, None).is_ok(), "owner/admin: {p}");
        }
        assert!(!is_mail_manage_surface("/cli/mail/backup"), "status is a plain read");
        assert!(mail_manage_authorized("/cli/mail/backup", false, None).is_ok());
        assert!(crate::session_token::is_agent_verb("/cli/mail/backup"));
    }

    #[test]
    fn mail_manage_toggle_opens_backup_for_the_it_agent_only() {
        let it_id = uuid::Uuid::new_v4().to_string();
        let plain_id = uuid::Uuid::new_v4().to_string();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            for (id, mm) in [(&it_id, 1), (&plain_id, 0)] {
                conn.execute(
                    "INSERT INTO projects (id, name, path, mail_manage_enabled) VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![
                        id,
                        format!("bk-{}", &id[..12]),
                        format!("/tmp/mail-backup-gate-{}-{}", std::process::id(), &id[..12]),
                        mm
                    ],
                )
                .expect("insert project");
            }
        }
        let principal = |id: &str| crate::session_token::HookPrincipal {
            workspace_uuid: id.to_string(),
            agent_address: "agent".to_string(),
        };
        for p in ["/cli/mail/backup/plan", "/cli/mail/backup/set"] {
            assert!(
                crate::mail_routes::mail_manage_authorized(p, false, Some(&principal(&it_id))).is_ok(),
                "toggle on: {p}"
            );
            let refused = crate::mail_routes::mail_manage_authorized(p, false, Some(&principal(&plain_id)))
                .err()
                .unwrap_or_else(|| panic!("toggle off must refuse {p}"));
            assert!(refused.body.contains("owner_only"), "{p}: {}", refused.body);
            assert!(crate::mail_routes::mail_manage_authorized(p, false, None).is_err(), "no principal, no admin: {p}");
        }
        let db = k2_core::db::shared();
        let conn = db.lock();
        for id in [&it_id, &plain_id] {
            conn.execute("DELETE FROM projects WHERE id = ?1", rusqlite::params![id]).expect("cleanup");
        }
    }

    #[test]
    fn mail_status_carries_the_backup_block() {
        let _g = crate::mail::mail_server_test_lock();
        clean_row();
        let v: serde_json::Value =
            serde_json::from_str(&crate::mail::routes_server::handle_status(&HashMap::new()).body).unwrap();
        assert_eq!(v["backup"]["mode"], "unconfigured", "{v}");
        assert_eq!(v["backup"]["note"], UNCONFIGURED_NOTE);
        seed_row();
        let r = handle_status_get(&HashMap::new());
        let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(v["backup"]["installed"], true);
        assert_eq!(v["backup"]["observer"]["status"], "never");
        clean_row();
    }
}
