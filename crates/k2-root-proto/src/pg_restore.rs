//! Point-in-time restore of named clusters (protocol 19,
//! `namespaces.pgrestore`; prd-app-supabase-compat-v1 §11.1, lane E).
//!
//! A named cluster with WAL archiving ([`crate::pg_named`]) already copies
//! every finished WAL segment into its spool. This adds what K2 itself
//! needs to restore it to a moment:
//!
//! - **Base backups** (`pg basebackup <M>-<name>`): `pg_basebackup -Ft -z
//!   -X stream` as postgres into [`base_dir`], beside the spool under the
//!   same root K2 Backups ships ([`crate::pg_named::WAL_SPOOL_ROOT`]). Each
//!   finished backup gets a [`BackupInfo`] file; the newest [`BASE_KEEP`]
//!   stay on the box.
//! - **Restore to a time** (`pg restore-to <M>-<name> <new> <time>`):
//!   always a NEW named cluster beside the old one, never in place. The
//!   newest base backup that ended at or before the time, then the spool's
//!   WAL replayed to it ([`recovery_conf`]). Refused before anything is
//!   made when the spool has a gap after that backup ([`wal_range`]) or the
//!   time is later than the newest WAL in the spool (no silent "restored
//!   to the end"); failed loudly when Postgres ends recovery before the
//!   time.
//! - **Drills** (`--drill`): the same restore into a throwaway cluster;
//!   `pg drop-drill` removes it again, and only it (or a restore that
//!   failed). The helper still never drops any other cluster.
//!
//! Times on argv are UTC stamps, `YYYYMMDDTHHMMSSZ` ([`UtcTime::parse_stamp`]:
//! helper tokens never carry a `:`); people and the daemon read
//! `YYYY-MM-DDTHH:MM:SSZ` ([`UtcTime::token`]).

use serde::{Deserialize, Serialize};

use crate::pg_named::WAL_SPOOL_ROOT;

/// Base backups of `<M>/<name>`: `<WAL_SPOOL_ROOT>/<M>-<name>.base/<stamp>/`.
pub fn base_dir(m: u32, n: &str) -> String {
    format!("{WAL_SPOOL_ROOT}/{m}-{n}.base")
}
/// The newest base backups kept per cluster (daily = a week).
pub const BASE_KEEP: usize = 7;
/// Written by the helper (as postgres) after `pg_basebackup` finished.
pub const BACKUP_INFO_FILE: &str = "k2-backup.json";
/// What `pg_basebackup` writes beside the tars.
pub const BACKUP_MANIFEST_FILE: &str = "backup_manifest";
/// The largest manifest the helper reads (one line per data file).
pub const MANIFEST_CAP: usize = 64 * 1024 * 1024;
/// A restore's recovery settings in the new cluster's `conf.d`, removed
/// once it is promoted.
pub const RESTORE_CONF_FILE: &str = "96-k2-restore.conf";
/// The helper's list of the clusters `restore-to` made.
pub const RESTORES_STATE: &str = "/var/lib/k2-root-helper/pg-restores.json";
pub const RESTORES_STATE_CAP: usize = 64 * 1024;
/// Seconds the helper waits for a restore to replay and promote.
pub const RECOVERY_WAIT_SECS: u32 = 6 * 3600;
/// Seconds the helper waits for the source to archive its current segment.
pub const ARCHIVE_WAIT_SECS: u32 = 120;
/// WAL segment size K2's clusters use (initdb's default).
pub const WAL_SEG_BYTES: u64 = 16 * 1024 * 1024;
/// The per-cluster lock of these verbs (`/run/k2-root-helper.pgrestore.lock`):
/// one base backup, restore or drop at a time.
pub const LOCK_NS: &str = "pgrestore";

// ── UTC times ───────────────────────────────────────────────────────────

/// A UTC second: argv `YYYY-MM-DDTHH:MM:SSZ`, Postgres `YYYY-MM-DD
/// HH:MM:SS+00`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UtcTime(i64);

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        _ => 28,
    }
}

impl UtcTime {
    pub fn from_epoch(secs: i64) -> Self {
        Self(secs)
    }
    pub fn epoch(self) -> i64 {
        self.0
    }
    /// Strictly `YYYY-MM-DDTHH:MM:SSZ`, years 2000-2199.
    pub fn parse(t: &str) -> Option<Self> {
        let b = t.as_bytes();
        if b.len() != 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' || b[19] != b'Z' {
            return None;
        }
        let num = |r: std::ops::Range<usize>| -> Option<i64> {
            let s = &t[r];
            s.bytes().all(|c| c.is_ascii_digit()).then(|| s.parse().ok()).flatten()
        };
        let (y, mo, d, h, mi, s) = (num(0..4)?, num(5..7)?, num(8..10)?, num(11..13)?, num(14..16)?, num(17..19)?);
        if !(2000..2200).contains(&y) || !(1..=12).contains(&mo) || d < 1 || d > days_in_month(y, mo) || h > 23 || mi > 59 || s > 59 {
            return None;
        }
        Some(Self(days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + s))
    }
    /// A stamp, `YYYYMMDDTHHMMSSZ` (the helper's argv form).
    pub fn parse_stamp(t: &str) -> Option<Self> {
        if !stamp_ok(t) {
            return None;
        }
        Self::parse(&format!("{}-{}-{}T{}:{}:{}Z", &t[0..4], &t[4..6], &t[6..8], &t[9..11], &t[11..13], &t[13..15]))
    }
    fn parts(self) -> (i64, i64, i64, i64, i64, i64) {
        let days = self.0.div_euclid(86_400);
        let rem = self.0.rem_euclid(86_400);
        let (y, m, d) = civil_from_days(days);
        (y, m, d, rem / 3600, rem % 3600 / 60, rem % 60)
    }
    /// `2026-11-22T10:15:00Z`.
    pub fn token(self) -> String {
        let (y, m, d, h, mi, s) = self.parts();
        format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
    }
    /// `2026-11-22 10:15:00+00` (`recovery_target_time`).
    pub fn pg(self) -> String {
        let (y, m, d, h, mi, s) = self.parts();
        format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}:{s:02}+00")
    }
    /// `20261122T101500Z`: a base backup's directory name.
    pub fn stamp(self) -> String {
        let (y, m, d, h, mi, s) = self.parts();
        format!("{y:04}{m:02}{d:02}T{h:02}{mi:02}{s:02}Z")
    }
}

/// A base backup's directory name ([`UtcTime::stamp`]).
pub fn stamp_ok(s: &str) -> bool {
    crate::pg_major::tag_ok(s)
}

// ── WAL segments ────────────────────────────────────────────────────────

/// A WAL segment file name: timeline, log and segment, 8 upper-case hex
/// each (16 MB segments: the segment part runs 00000000-000000FF).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WalSeg {
    pub tli: u32,
    pub log: u32,
    pub seg: u32,
}

const SEGS_PER_LOG: u32 = (0x1_0000_0000u64 / WAL_SEG_BYTES) as u32;

impl WalSeg {
    pub fn parse(name: &str) -> Option<Self> {
        let b = name.as_bytes();
        if b.len() != 24 || !b.iter().all(|c| c.is_ascii_digit() || (b'A'..=b'F').contains(c)) {
            return None;
        }
        let h = |r: std::ops::Range<usize>| u32::from_str_radix(&name[r], 16).ok();
        let (tli, log, seg) = (h(0..8)?, h(8..16)?, h(16..24)?);
        (tli > 0 && seg < SEGS_PER_LOG).then_some(Self { tli, log, seg })
    }
    /// The segment holding `lsn` (`X/Y` as Postgres prints it) on `tli`.
    pub fn of_lsn(tli: u32, lsn: &str) -> Option<Self> {
        let (hi, lo) = lsn.split_once('/')?;
        let ok = |s: &str| !s.is_empty() && s.len() <= 8 && s.bytes().all(|c| c.is_ascii_hexdigit());
        if !ok(hi) || !ok(lo) || tli == 0 {
            return None;
        }
        let v = (u64::from_str_radix(hi, 16).ok()? << 32) | u64::from_str_radix(lo, 16).ok()?;
        let n = v / WAL_SEG_BYTES;
        Some(Self { tli, log: (n / u64::from(SEGS_PER_LOG)) as u32, seg: (n % u64::from(SEGS_PER_LOG)) as u32 })
    }
    pub fn name(self) -> String {
        format!("{:08X}{:08X}{:08X}", self.tli, self.log, self.seg)
    }
    pub fn next(self) -> Self {
        if self.seg + 1 == SEGS_PER_LOG {
            Self { tli: self.tli, log: self.log + 1, seg: 0 }
        } else {
            Self { tli: self.tli, log: self.log, seg: self.seg + 1 }
        }
    }
}

/// One WAL file in a spool: its segment and when it was archived (mtime,
/// epoch seconds).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpoolSeg {
    pub seg: WalSeg,
    pub archived_at: i64,
}

/// The spool's WAL from `start` on, on `start`'s timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalRange {
    pub first: WalSeg,
    pub last: WalSeg,
    pub count: usize,
    /// When the newest segment of the range was archived.
    pub newest_at: i64,
}

/// The spool's segments on `start`'s timeline from `start` on must run
/// without a hole. `Err(missing)`: the first segment that isn't there
/// (`start` itself when the spool holds nothing from it on).
pub fn wal_range(spool: &[SpoolSeg], start: WalSeg) -> Result<WalRange, WalSeg> {
    let mut segs: Vec<&SpoolSeg> = spool.iter().filter(|s| s.seg.tli == start.tli && s.seg >= start).collect();
    segs.sort_by_key(|s| s.seg);
    segs.dedup_by_key(|s| s.seg);
    let mut want = start;
    for s in &segs {
        if s.seg != want {
            return Err(want);
        }
        want = want.next();
    }
    let last = segs.last().ok_or(start)?;
    Ok(WalRange {
        first: start,
        last: last.seg,
        count: segs.len(),
        newest_at: segs.iter().map(|s| s.archived_at).max().unwrap_or(0),
    })
}

// ── Base backups ────────────────────────────────────────────────────────

/// [`BACKUP_INFO_FILE`]: one finished base backup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BackupInfo {
    pub stamp: String,
    pub major: u32,
    pub name: String,
    /// The source's `system_identifier`: a backup of another cluster of
    /// the same name never counts.
    pub system_id: String,
    pub started_at: i64,
    /// Consistent from this moment on (`pg_basebackup` returned).
    pub ended_at: i64,
    pub start_wal: String,
    pub end_wal: String,
}

impl BackupInfo {
    pub fn valid(&self) -> bool {
        stamp_ok(&self.stamp)
            && crate::pg_named::system_id_ok(&self.system_id)
            && self.ended_at >= self.started_at
            && WalSeg::parse(&self.start_wal).is_some()
            && WalSeg::parse(&self.end_wal).is_some()
    }
    pub fn start(&self) -> WalSeg {
        WalSeg::parse(&self.start_wal).expect("valid")
    }
}

/// The newest valid backup of `system_id` that ended at or before `target`.
pub fn choose_backup<'a>(infos: &'a [BackupInfo], system_id: &str, target: UtcTime) -> Option<&'a BackupInfo> {
    infos
        .iter()
        .filter(|b| b.valid() && b.system_id == system_id && b.ended_at <= target.epoch())
        .max_by_key(|b| b.ended_at)
}

/// The WAL range of a `backup_manifest` (`"WAL-Ranges": [{"Timeline": 1,
/// "Start-LSN": "0/2000028", "End-LSN": "0/2000100"}]`): the segments
/// holding the first range's start and the last range's end.
pub fn manifest_wal(manifest: &[u8]) -> Option<(WalSeg, WalSeg)> {
    let v: serde_json::Value = serde_json::from_slice(manifest).ok()?;
    let ranges = v.get("WAL-Ranges")?.as_array()?;
    let seg = |r: &serde_json::Value, k: &str| -> Option<WalSeg> {
        let tli = u32::try_from(r.get("Timeline")?.as_u64()?).ok()?;
        WalSeg::of_lsn(tli, r.get(k)?.as_str()?)
    };
    // Postgres lists the newest timeline first.
    let start = seg(ranges.last()?, "Start-LSN")?;
    let end = seg(ranges.first()?, "End-LSN")?;
    Some((start, end))
}

// ── Recovery ────────────────────────────────────────────────────────────

/// The new cluster's recovery settings: WAL from the SOURCE's spool
/// (read only: `cp` out of it, never into it), replay to `target`, then
/// promote. Built from validated tokens only. Archiving stays off on the
/// new cluster (it has no archive conf; `archive_mode` here is belt and
/// braces).
pub fn recovery_conf(m: u32, source: &str, target: UtcTime) -> String {
    let d = crate::pg_named::wal_spool(m, source);
    format!(
        "# Point-in-time restore from {m}/{source} to {t}. Written by k2-root-helper `pg restore-to`;\n\
         # removed once the cluster is promoted.\n\
         restore_command = 'cp {d}/%f %p'\n\
         recovery_target_time = '{pg}'\n\
         recovery_target_inclusive = on\n\
         recovery_target_action = 'promote'\n\
         archive_mode = off\n",
        t = target.token(),
        pg = target.pg()
    )
}

/// A restore's start may take as long as the replay to consistency: the
/// systemd drop-in that lets `systemctl start` wait for it (no start
/// timeout), beside the slice drop-in, removed once the cluster is
/// promoted.
pub const RESTORE_DROPIN_FILE: &str = "k2-restore.conf";
pub fn restore_dropin() -> &'static str {
    "# Point-in-time restore: the start waits for the replay. Written by k2-root-helper `pg restore-to`; removed after promotion.\n\
     [Service]\n\
     TimeoutStartSec=infinity\n"
}
/// ... and `pg_ctlcluster`'s own wait for the start (Debian's
/// `pg_ctl.conf`), as long as the whole replay may take. The file
/// `pg_createcluster` wrote comes back after promotion.
pub const PG_CTL_CONF_FILE: &str = "pg_ctl.conf";
pub fn restore_pg_ctl_conf() -> String {
    format!(
        "# Point-in-time restore: wait for the replay. Written by k2-root-helper `pg restore-to`; replaced after promotion.\n\
         pg_ctl_options = '-t {RECOVERY_WAIT_SECS}'\n"
    )
}
/// A start that returned an error while Postgres keeps replaying is not a
/// failure: the helper keeps polling. Only a postmaster that is gone counts:
/// after it was seen running, or when it never came up in this long.
pub const START_GRACE_SECS: u32 = 120;

/// SQL run on a running source before a restore: a commit stamped now (a
/// time target needs a commit after it to stop at) and a WAL switch; the
/// answer is the segment just closed.
pub const SWITCH_SQL: &str = "SELECT pg_catalog.pg_current_xact_id();\nSELECT pg_catalog.pg_walfile_name(pg_catalog.pg_switch_wal());\n";
/// The archiver's last segment.
pub const LAST_ARCHIVED_SQL: &str = "SELECT coalesce(last_archived_wal, '') FROM pg_catalog.pg_stat_archiver;\n";
pub const IN_RECOVERY_SQL: &str = "SELECT pg_catalog.pg_is_in_recovery()::text;\n";

/// One cluster `restore-to` made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RestoreRec {
    pub major: u32,
    pub name: String,
    /// The source cluster's name (same major).
    pub from: String,
    /// [`UtcTime::token`].
    pub target: String,
    pub base_backup: String,
    pub drill: bool,
    /// Promoted and recorded as a named cluster.
    pub ok: bool,
    pub created_at: i64,
}

/// [`RESTORES_STATE`]'s shape.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestoresState {
    #[serde(default)]
    pub restores: Vec<RestoreRec>,
}

impl RestoresState {
    pub fn get(&self, major: u32, name: &str) -> Option<&RestoreRec> {
        self.restores.iter().find(|r| r.major == major && r.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_round_trip_and_refuse_anything_else() {
        let t = UtcTime::parse("2026-11-22T10:15:00Z").expect("time");
        assert_eq!(t.token(), "2026-11-22T10:15:00Z");
        assert_eq!(t.pg(), "2026-11-22 10:15:00+00");
        assert_eq!(t.stamp(), "20261122T101500Z");
        assert!(stamp_ok(&t.stamp()));
        assert_eq!(UtcTime::parse_stamp("20261122T101500Z"), Some(t));
        for bad in ["20261322T101500Z", "20261122T241500Z", "2026-11-22T10:15:00Z", "20261122T101500", "19991231T235959Z"] {
            assert_eq!(UtcTime::parse_stamp(bad), None, "{bad}");
        }
        assert_eq!(UtcTime::parse("1970-01-01T00:00:00Z"), None, "before 2000");
        assert_eq!(UtcTime::parse("2000-01-01T00:00:00Z").unwrap().epoch(), 946_684_800);
        assert_eq!(UtcTime::parse("2024-02-29T23:59:59Z").unwrap().epoch(), 1_709_251_199);
        assert_eq!(UtcTime::from_epoch(1_709_251_199).token(), "2024-02-29T23:59:59Z");
        for bad in [
            "2026-11-22T10:15:00",
            "2026-11-22 10:15:00Z",
            "2026-11-22T10:15:00+00:00",
            "2026-13-01T00:00:00Z",
            "2026-02-29T00:00:00Z",
            "2026-11-31T00:00:00Z",
            "2026-11-22T24:00:00Z",
            "2026-11-22T10:60:00Z",
            "2026-11-22T10:15:60Z",
            "2026-11-22T10:15:0aZ",
            "+026-11-22T10:15:00Z",
            "2026-11-22T10:15:00Z ",
            "",
        ] {
            assert_eq!(UtcTime::parse(bad), None, "{bad:?}");
        }
        for e in [946_684_800i64, 1_763_806_500, 4_102_444_799] {
            assert_eq!(UtcTime::parse(&UtcTime::from_epoch(e).token()).unwrap().epoch(), e);
        }
    }

    #[test]
    fn wal_names_lsns_and_order() {
        let s = WalSeg::parse("0000000100000000000000FF").expect("seg");
        assert_eq!(s.next().name(), "000000010000000100000000");
        assert_eq!(WalSeg::parse("000000010000000000000002").unwrap().next().name(), "000000010000000000000003");
        for bad in ["00000001000000000000000", "0000000100000000000000fF", "000000010000000000000100", "000000000000000000000001", "00000001.history", "000000010000000000000002.partial"] {
            assert!(WalSeg::parse(bad).is_none(), "{bad}");
        }
        assert_eq!(WalSeg::of_lsn(1, "0/2000028").unwrap().name(), "000000010000000000000002");
        assert_eq!(WalSeg::of_lsn(2, "1/FF000000").unwrap().name(), "0000000200000001000000FF");
        assert!(WalSeg::of_lsn(1, "0/").is_none());
        assert!(WalSeg::of_lsn(0, "0/1").is_none());
        assert!(WalSeg::parse("000000010000000100000000").unwrap() > WalSeg::parse("0000000100000000000000FF").unwrap());
    }

    fn spool(names: &[(&str, i64)]) -> Vec<SpoolSeg> {
        names.iter().map(|(n, t)| SpoolSeg { seg: WalSeg::parse(n).unwrap(), archived_at: *t }).collect()
    }

    #[test]
    fn a_gap_after_the_backup_is_named() {
        let start = WalSeg::parse("000000010000000000000003").unwrap();
        let ok = spool(&[
            ("000000010000000000000001", 1),
            ("000000010000000000000003", 3),
            ("000000010000000000000004", 4),
            ("000000010000000000000005", 9),
            ("000000020000000000000009", 99),
        ]);
        let r = wal_range(&ok, start).expect("no gap");
        assert_eq!((r.first.name(), r.last.name(), r.count, r.newest_at), ("000000010000000000000003".into(), "000000010000000000000005".into(), 3, 9));
        let gap = spool(&[("000000010000000000000003", 3), ("000000010000000000000005", 5)]);
        assert_eq!(wal_range(&gap, start).unwrap_err().name(), "000000010000000000000004");
        let none = spool(&[("000000010000000000000001", 1)]);
        assert_eq!(wal_range(&none, start).unwrap_err(), start, "nothing from the backup on is a gap at its start");
        let missing_first = spool(&[("000000010000000000000004", 4)]);
        assert_eq!(wal_range(&missing_first, start).unwrap_err(), start);
    }

    fn info(stamp: &str, ended: i64, sys: &str) -> BackupInfo {
        BackupInfo {
            stamp: stamp.into(),
            major: 17,
            name: "feedback".into(),
            system_id: sys.into(),
            started_at: ended - 10,
            ended_at: ended,
            start_wal: "000000010000000000000003".into(),
            end_wal: "000000010000000000000003".into(),
        }
    }

    #[test]
    fn the_newest_backup_ended_before_the_target_of_this_cluster() {
        let infos = vec![info("20261120T000000Z", 100, "7"), info("20261121T000000Z", 200, "7"), info("20261122T000000Z", 300, "7"), info("20261121T120000Z", 250, "8")];
        assert_eq!(choose_backup(&infos, "7", UtcTime::from_epoch(260)).unwrap().stamp, "20261121T000000Z");
        assert_eq!(choose_backup(&infos, "7", UtcTime::from_epoch(300)).unwrap().stamp, "20261122T000000Z");
        assert!(choose_backup(&infos, "7", UtcTime::from_epoch(99)).is_none());
        assert_eq!(choose_backup(&infos, "8", UtcTime::from_epoch(999)).unwrap().stamp, "20261121T120000Z");
        let mut bad = info("x", 150, "7");
        bad.stamp = "not-a-stamp".into();
        assert!(!bad.valid());
    }

    #[test]
    fn manifest_ranges() {
        let m = br#"{"PostgreSQL-Backup-Manifest-Version": 2, "Files": [], "WAL-Ranges": [
            { "Timeline": 1, "Start-LSN": "0/3000028", "End-LSN": "0/4000100" }
        ], "Manifest-Checksum": "x"}"#;
        let (s, e) = manifest_wal(m).expect("ranges");
        assert_eq!((s.name(), e.name()), ("000000010000000000000003".into(), "000000010000000000000004".into()));
        assert!(manifest_wal(b"{}").is_none());
        assert!(manifest_wal(b"not json").is_none());
    }

    #[test]
    fn recovery_conf_reads_the_source_spool_and_promotes() {
        let c = recovery_conf(17, "feedback", UtcTime::parse("2026-11-22T10:15:00Z").unwrap());
        assert!(c.contains("restore_command = 'cp /var/lib/postgresql/k2-wal/17-feedback/%f %p'\n"), "{c}");
        assert!(c.contains("recovery_target_time = '2026-11-22 10:15:00+00'\n"));
        assert!(c.contains("recovery_target_action = 'promote'\n"));
        assert!(c.contains("archive_mode = off\n"));
        assert_eq!(base_dir(17, "feedback"), "/var/lib/postgresql/k2-wal/17-feedback.base");
        assert!(restore_dropin().contains("TimeoutStartSec=infinity\n"));
        assert!(restore_pg_ctl_conf().contains(&format!("pg_ctl_options = '-t {RECOVERY_WAIT_SECS}'\n")));
    }

    #[test]
    fn restore_records() {
        let r = RestoreRec {
            major: 17,
            name: "fb_drill".into(),
            from: "feedback".into(),
            target: "2026-11-22T10:15:00Z".into(),
            base_backup: "20261122T000000Z".into(),
            drill: true,
            ok: false,
            created_at: 1,
        };
        let s = RestoresState { restores: vec![r] };
        let j = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<RestoresState>(&j).unwrap(), s);
        assert!(s.get(17, "fb_drill").is_some() && s.get(17, "feedback").is_none());
        assert!(serde_json::from_str::<RestoresState>(r#"{"restores":[],"x":1}"#).is_err());
    }
}
